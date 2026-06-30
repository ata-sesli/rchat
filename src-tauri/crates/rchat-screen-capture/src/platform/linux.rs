use crate::{
    bgra_to_i420, clamp_to_profile, i420_copy, i420_to_preview_rgba, now_timestamp_us,
    nv12_to_i420, rgba_to_i420, scale_i420_nearest, yuyv_to_i420, CaptureStatsAtomic,
    I420ScreenFrame, LatestSlot, PreviewFrame, ScreenCaptureBackend, ScreenCaptureConfig,
    ScreenCaptureCursorMode, ScreenCaptureError, ScreenCaptureFormatInfo, ScreenCaptureSessionInfo,
    ScreenCaptureSessionStats, ScreenCaptureSupport, PREVIEW_INTERVAL,
};
use ashpd::desktop::{
    screencast::{
        CursorMode, Screencast, SelectSourcesOptions, SourceType, Stream as ScreencastStream,
    },
    PersistMode,
};
use pipewire as pw;
use pw::{properties::properties, spa};
use std::env;
use std::os::fd::OwnedFd;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};
use x11rb::connection::{Connection, RequestConnection};
use x11rb::protocol::{randr, shm, xfixes, xproto};
use x11rb::protocol::randr::ConnectionExt as _;
use x11rb::protocol::shm::ConnectionExt as _;
use x11rb::protocol::xfixes::ConnectionExt as _;
use x11rb::protocol::xproto::ConnectionExt as _;
use x11rb::rust_connection::RustConnection;

pub struct PlatformScreenCaptureSession {
    info: ScreenCaptureSessionInfo,
    i420_slot: LatestSlot<I420ScreenFrame>,
    preview_slot: LatestSlot<PreviewFrame>,
    stop: Arc<AtomicBool>,
    control_tx: Option<pw::channel::Sender<PipeWireControl>>,
    stats: Arc<CaptureStatsAtomic>,
    handle: Option<JoinHandle<()>>,
}

impl PlatformScreenCaptureSession {
    pub fn info(&self) -> &ScreenCaptureSessionInfo {
        &self.info
    }

    pub fn try_recv_latest_i420(&mut self) -> Option<I420ScreenFrame> {
        self.i420_slot.take()
    }

    pub fn try_recv_latest_preview(&mut self) -> Option<PreviewFrame> {
        self.preview_slot.take()
    }

    pub fn stats(&self) -> ScreenCaptureSessionStats {
        self.stats
            .snapshot(self.i420_slot.dropped(), self.preview_slot.dropped())
    }
}

impl Drop for PlatformScreenCaptureSession {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(control_tx) = self.control_tx.as_ref() {
            let _ = control_tx.send(PipeWireControl::Stop);
        }
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

pub async fn screen_capture_support() -> ScreenCaptureSupport {
    if selected_linux_backend_from_env() == ScreenCaptureBackend::LinuxX11 {
        return match probe_x11_capture_area() {
            Ok(_) => ScreenCaptureSupport {
                supported: true,
                reason: None,
                backend: ScreenCaptureBackend::LinuxX11,
            },
            Err(error) => ScreenCaptureSupport {
                supported: false,
                reason: Some(format!("Linux X11 screen capture is unavailable: {}", error)),
                backend: ScreenCaptureBackend::LinuxX11,
            },
        };
    }

    match Screencast::new().await {
        Ok(_) => ScreenCaptureSupport {
            supported: true,
            reason: None,
            backend: ScreenCaptureBackend::LinuxPortalPipeWire,
        },
        Err(error) => ScreenCaptureSupport {
            supported: false,
            reason: Some(format!(
                "Linux screen capture portal is unavailable: {}",
                error
            )),
            backend: ScreenCaptureBackend::LinuxPortalPipeWire,
        },
    }
}

pub async fn start_session(
    config: ScreenCaptureConfig,
) -> Result<PlatformScreenCaptureSession, ScreenCaptureError> {
    if selected_linux_backend_from_env() == ScreenCaptureBackend::LinuxX11 {
        return start_x11_session(config);
    }

    let (stream, fd) = open_portal(config.cursor_mode).await?;
    let node_id = stream.pipe_wire_node_id();
    let (width, height) = config.profile.dimensions();
    let info = ScreenCaptureSessionInfo {
        backend: ScreenCaptureBackend::LinuxPortalPipeWire,
        source_label: format!("portal stream node {}", node_id),
        requested_profile: config.profile.label().to_string(),
        format: ScreenCaptureFormatInfo {
            width,
            height,
            fps: config.profile.fps(),
            format: "pipewire-negotiated".to_string(),
        },
    };

    let i420_slot = LatestSlot::default();
    let preview_slot = LatestSlot::default();
    let stats = Arc::new(CaptureStatsAtomic::default());
    let stop = Arc::new(AtomicBool::new(false));
    let (control_tx, control_rx) = pw::channel::channel();
    let thread_i420_slot = i420_slot.clone();
    let thread_preview_slot = preview_slot.clone();
    let thread_stats = Arc::clone(&stats);
    let thread_stop = Arc::clone(&stop);
    let profile = config.profile;
    let handle = thread::Builder::new()
        .name("rchat-screen-capture".to_string())
        .spawn(move || {
            if let Err(error) = run_pipewire_capture(
                node_id,
                fd,
                profile,
                thread_i420_slot,
                thread_preview_slot,
                thread_stats,
                thread_stop,
                control_rx,
            ) {
                eprintln!("[Screen][Capture] PipeWire capture stopped: {}", error);
            }
        })
        .map_err(|e| ScreenCaptureError::Backend(e.to_string()))?;

    Ok(PlatformScreenCaptureSession {
        info,
        i420_slot,
        preview_slot,
        stop,
        control_tx: Some(control_tx),
        stats,
        handle: Some(handle),
    })
}

fn selected_linux_backend_from_env() -> ScreenCaptureBackend {
    crate::select_linux_capture_backend_for_env(
        env::var("XDG_SESSION_TYPE").ok().as_deref(),
        env::var("DISPLAY").ok().as_deref(),
        env::var("WAYLAND_DISPLAY").ok().as_deref(),
    )
}

#[derive(Debug, Clone, Copy)]
struct X11CaptureArea {
    root: xproto::Window,
    x: i16,
    y: i16,
    width: u16,
    height: u16,
    depth: u8,
    bits_per_pixel: u8,
    bit_order: xproto::ImageOrder,
}

impl X11CaptureArea {
    fn source_label(self) -> String {
        format!(
            "X11 primary monitor {}x{} at {},{}",
            self.width, self.height, self.x, self.y
        )
    }
}

fn start_x11_session(
    config: ScreenCaptureConfig,
) -> Result<PlatformScreenCaptureSession, ScreenCaptureError> {
    let area = probe_x11_capture_area()?;
    let (target_width, target_height) = config.profile.dimensions();
    let info = ScreenCaptureSessionInfo {
        backend: ScreenCaptureBackend::LinuxX11,
        source_label: area.source_label(),
        requested_profile: config.profile.label().to_string(),
        format: ScreenCaptureFormatInfo {
            width: target_width,
            height: target_height,
            fps: config.profile.fps(),
            format: "x11-zpixmap".to_string(),
        },
    };

    let i420_slot = LatestSlot::default();
    let preview_slot = LatestSlot::default();
    let stats = Arc::new(CaptureStatsAtomic::default());
    let stop = Arc::new(AtomicBool::new(false));
    let thread_i420_slot = i420_slot.clone();
    let thread_preview_slot = preview_slot.clone();
    let thread_stats = Arc::clone(&stats);
    let thread_stop = Arc::clone(&stop);
    let profile = config.profile;
    let cursor_mode = config.cursor_mode;
    let handle = thread::Builder::new()
        .name("rchat-screen-capture-x11".to_string())
        .spawn(move || {
            if let Err(error) = run_x11_capture(
                area,
                profile,
                cursor_mode,
                thread_i420_slot,
                thread_preview_slot,
                thread_stats,
                thread_stop,
            ) {
                eprintln!("[Screen][Capture] X11 capture stopped: {}", error);
            }
        })
        .map_err(|e| ScreenCaptureError::Backend(e.to_string()))?;

    Ok(PlatformScreenCaptureSession {
        info,
        i420_slot,
        preview_slot,
        stop,
        control_tx: None,
        stats,
        handle: Some(handle),
    })
}

fn probe_x11_capture_area() -> Result<X11CaptureArea, ScreenCaptureError> {
    let (conn, screen_num) =
        x11rb::connect(None).map_err(|e| ScreenCaptureError::Backend(e.to_string()))?;
    let setup = conn.setup();
    let screen = setup
        .roots
        .get(screen_num)
        .ok_or_else(|| ScreenCaptureError::Backend("X11 screen not found".to_string()))?;
    let monitor = select_x11_monitor(&conn, screen.root).unwrap_or((
        0,
        0,
        screen.width_in_pixels,
        screen.height_in_pixels,
    ));
    let pixmap_format = setup
        .pixmap_formats
        .iter()
        .find(|format| format.depth == screen.root_depth)
        .ok_or_else(|| {
            ScreenCaptureError::UnsupportedFormat(format!(
                "X11 pixmap depth {} is not advertised",
                screen.root_depth
            ))
        })?;
    Ok(X11CaptureArea {
        root: screen.root,
        x: monitor.0,
        y: monitor.1,
        width: monitor.2,
        height: monitor.3,
        depth: screen.root_depth,
        bits_per_pixel: pixmap_format.bits_per_pixel,
        bit_order: setup.bitmap_format_bit_order,
    })
}

fn select_x11_monitor(
    conn: &RustConnection,
    root: xproto::Window,
) -> Option<(i16, i16, u16, u16)> {
    if conn
        .extension_information(randr::X11_EXTENSION_NAME)
        .ok()
        .flatten()
        .is_some()
    {
        let reply = conn.randr_get_monitors(root, true).ok()?.reply().ok()?;
        if let Some(primary) = reply
            .monitors
            .iter()
            .find(|monitor| monitor.primary && monitor.width > 0 && monitor.height > 0)
        {
            return Some((primary.x, primary.y, primary.width, primary.height));
        }
        if let Some(first) = reply
            .monitors
            .iter()
            .find(|monitor| monitor.width > 0 && monitor.height > 0)
        {
            return Some((first.x, first.y, first.width, first.height));
        }
    }
    None
}

struct X11ShmSegment {
    shmid: i32,
    seg: shm::Seg,
    ptr: *mut u8,
    size: usize,
}

fn release_x11_shm_segment(shmid: i32, ptr: Option<*mut u8>) {
    unsafe {
        if let Some(ptr) = ptr {
            libc::shmdt(ptr as *mut libc::c_void);
        }
        libc::shmctl(shmid, libc::IPC_RMID, std::ptr::null_mut());
    }
}

impl X11ShmSegment {
    fn new(conn: &RustConnection, size: usize) -> Result<Self, ScreenCaptureError> {
        let shmid = unsafe { libc::shmget(libc::IPC_PRIVATE, size, libc::IPC_CREAT | 0o600) };
        if shmid == -1 {
            return Err(ScreenCaptureError::Backend(
                std::io::Error::last_os_error().to_string(),
            ));
        }
        let ptr = unsafe { libc::shmat(shmid, std::ptr::null(), 0) } as *mut u8;
        if ptr as isize == -1 {
            release_x11_shm_segment(shmid, None);
            return Err(ScreenCaptureError::Backend(
                std::io::Error::last_os_error().to_string(),
            ));
        }
        let seg = match conn.generate_id() {
            Ok(seg) => seg,
            Err(error) => {
                release_x11_shm_segment(shmid, Some(ptr));
                return Err(ScreenCaptureError::Backend(error.to_string()));
            }
        };
        if let Err(error) = conn.shm_attach(seg, shmid as u32, false) {
            release_x11_shm_segment(shmid, Some(ptr));
            return Err(ScreenCaptureError::Backend(error.to_string()));
        }
        if let Err(error) = conn.flush() {
            let _ = conn.shm_detach(seg);
            release_x11_shm_segment(shmid, Some(ptr));
            return Err(ScreenCaptureError::Backend(error.to_string()));
        }
        Ok(Self {
            shmid,
            seg,
            ptr,
            size,
        })
    }

    fn bytes(&self) -> &[u8] {
        unsafe { std::slice::from_raw_parts(self.ptr, self.size) }
    }
}

impl Drop for X11ShmSegment {
    fn drop(&mut self) {
        unsafe {
            libc::shmdt(self.ptr as *mut libc::c_void);
            libc::shmctl(self.shmid, libc::IPC_RMID, std::ptr::null_mut());
        }
    }
}

struct X11Capturer {
    conn: RustConnection,
    area: X11CaptureArea,
    shm: Option<X11ShmSegment>,
    cursor_mode: ScreenCaptureCursorMode,
}

impl X11Capturer {
    fn new(
        area: X11CaptureArea,
        cursor_mode: ScreenCaptureCursorMode,
    ) -> Result<Self, ScreenCaptureError> {
        let (conn, _) =
            x11rb::connect(None).map_err(|e| ScreenCaptureError::Backend(e.to_string()))?;
        let shm = if conn
            .extension_information(shm::X11_EXTENSION_NAME)
            .ok()
            .flatten()
            .is_some()
        {
            X11ShmSegment::new(&conn, x11_frame_buffer_len(area.width, area.height)).ok()
        } else {
            None
        };
        Ok(Self {
            conn,
            area,
            shm,
            cursor_mode,
        })
    }

    fn capture_i420(
        &mut self,
        profile: crate::ScreenCaptureProfile,
    ) -> Result<I420ScreenFrame, ScreenCaptureError> {
        let width = self.area.width as u32;
        let height = self.area.height as u32;
        let stride = x11_stride(self.area.width, self.area.bits_per_pixel);
        let mut data = self.capture_raw_frame()?;
        if self.cursor_mode == ScreenCaptureCursorMode::Embedded {
            self.overlay_cursor(&mut data, stride);
        }
        let i420 = x11_zpixmap_to_i420(
            &data,
            width,
            height,
            stride,
            self.area.bits_per_pixel,
            self.area.bit_order,
        )?;
        let (target_width, target_height) = profile.dimensions();
        let (width, height, data) = if width > target_width || height > target_height {
            let (out_width, out_height) = clamp_to_profile(width, height, profile);
            let data = scale_i420_nearest(&i420, width, height, out_width, out_height)?;
            (out_width, out_height, data)
        } else {
            (width & !1, height & !1, i420)
        };
        Ok(I420ScreenFrame {
            timestamp_us: now_timestamp_us(),
            width,
            height,
            data,
        })
    }

    fn capture_raw_frame(&mut self) -> Result<Vec<u8>, ScreenCaptureError> {
        if let Some(shm) = self.shm.as_ref() {
            let result = match self.conn.shm_get_image(
                self.area.root,
                self.area.x,
                self.area.y,
                self.area.width,
                self.area.height,
                u32::MAX,
                u8::from(xproto::ImageFormat::Z_PIXMAP),
                shm.seg,
                0,
            ) {
                Ok(cookie) => cookie.reply().map_err(|e| e.to_string()),
                Err(error) => Err(error.to_string()),
            };
            match result {
                Ok(_) => return Ok(shm.bytes().to_vec()),
                Err(error) => {
                    eprintln!(
                        "[Screen][Capture] X11 SHM get_image failed, falling back to GetImage: {}",
                        error
                    );
                    self.shm = None;
                }
            }
        }

        let reply = self
            .conn
            .get_image(
                xproto::ImageFormat::Z_PIXMAP,
                    self.area.root,
                    self.area.x,
                    self.area.y,
                    self.area.width,
                    self.area.height,
                    u32::MAX,
            )
            .map_err(|e| ScreenCaptureError::Backend(e.to_string()))?
            .reply()
            .map_err(|e| ScreenCaptureError::Backend(e.to_string()))?;
        Ok(reply.data)
    }

    fn overlay_cursor(&self, raw: &mut [u8], stride: usize) {
        if self.area.bits_per_pixel != 32 {
            return;
        }
        if self
            .conn
            .extension_information(xfixes::X11_EXTENSION_NAME)
            .ok()
            .flatten()
            .is_none()
        {
            return;
        }
        let reply = match self.conn.xfixes_get_cursor_image() {
            Ok(cookie) => match cookie.reply() {
                Ok(reply) => reply,
                Err(_) => return,
            },
            Err(_) => return,
        };
        let cursor_left = reply.x.saturating_sub(reply.xhot as i16);
        let cursor_top = reply.y.saturating_sub(reply.yhot as i16);
        let capture_left = self.area.x;
        let capture_top = self.area.y;
        let capture_right = capture_left.saturating_add(self.area.width as i16);
        let capture_bottom = capture_top.saturating_add(self.area.height as i16);

        for cy in 0..reply.height as i16 {
            let py = cursor_top.saturating_add(cy);
            if py < capture_top || py >= capture_bottom {
                continue;
            }
            for cx in 0..reply.width as i16 {
                let px = cursor_left.saturating_add(cx);
                if px < capture_left || px >= capture_right {
                    continue;
                }
                let cursor_index = (cy as usize * reply.width as usize) + cx as usize;
                let argb = reply.cursor_image.get(cursor_index).copied().unwrap_or(0);
                let alpha = ((argb >> 24) & 0xff) as u8;
                if alpha == 0 {
                    continue;
                }
                let src_r = ((argb >> 16) & 0xff) as u8;
                let src_g = ((argb >> 8) & 0xff) as u8;
                let src_b = (argb & 0xff) as u8;
                let dst_x = (px - capture_left) as usize;
                let dst_y = (py - capture_top) as usize;
                let offset = dst_y * stride + dst_x * 4;
                if offset + 2 >= raw.len() {
                    continue;
                }
                blend_bgrx_pixel(&mut raw[offset..offset + 4], src_r, src_g, src_b, alpha);
            }
        }
    }
}

impl Drop for X11Capturer {
    fn drop(&mut self) {
        if let Some(shm) = self.shm.as_ref() {
            let _ = self.conn.shm_detach(shm.seg);
            let _ = self.conn.flush();
        }
    }
}

fn run_x11_capture(
    area: X11CaptureArea,
    profile: crate::ScreenCaptureProfile,
    cursor_mode: ScreenCaptureCursorMode,
    i420_slot: LatestSlot<I420ScreenFrame>,
    preview_slot: LatestSlot<PreviewFrame>,
    stats: Arc<CaptureStatsAtomic>,
    stop: Arc<AtomicBool>,
) -> Result<(), ScreenCaptureError> {
    let mut capturer = X11Capturer::new(area, cursor_mode)?;
    let frame_interval = Duration::from_millis((1_000 / profile.fps().max(1)) as u64);
    let mut last_preview_at: Option<Instant> = None;
    while !stop.load(Ordering::Relaxed) {
        let started = Instant::now();
        match capturer.capture_i420(profile) {
            Ok(frame) => {
                stats.captured_frames.fetch_add(1, Ordering::Relaxed);
                let should_preview = last_preview_at
                    .map(|last| last.elapsed() >= PREVIEW_INTERVAL)
                    .unwrap_or(true);
                if should_preview {
                    match i420_to_preview_rgba(&frame.data, frame.width, frame.height) {
                        Ok(mut preview) => {
                            preview.timestamp_us = frame.timestamp_us;
                            preview_slot.replace(preview);
                            stats.preview_frames.fetch_add(1, Ordering::Relaxed);
                            last_preview_at = Some(Instant::now());
                        }
                        Err(error) => {
                            stats.conversion_errors.fetch_add(1, Ordering::Relaxed);
                            eprintln!("[Screen][Capture] X11 preview conversion failed: {}", error);
                        }
                    }
                }
                i420_slot.replace(frame);
            }
            Err(error) => {
                stats.conversion_errors.fetch_add(1, Ordering::Relaxed);
                eprintln!("[Screen][Capture] X11 frame conversion failed: {}", error);
            }
        }
        let elapsed = started.elapsed();
        if elapsed < frame_interval {
            thread::sleep(frame_interval - elapsed);
        }
    }
    Ok(())
}

fn x11_frame_buffer_len(width: u16, height: u16) -> usize {
    width as usize * height as usize * 4
}

fn x11_stride(width: u16, bits_per_pixel: u8) -> usize {
    ((width as usize * bits_per_pixel as usize + 31) / 32) * 4
}

fn x11_zpixmap_to_i420(
    src: &[u8],
    width: u32,
    height: u32,
    stride: usize,
    bits_per_pixel: u8,
    bit_order: xproto::ImageOrder,
) -> Result<Vec<u8>, ScreenCaptureError> {
    match bits_per_pixel {
        32 => bgra_to_i420(src, width, height, stride),
        24 => x11_rgb24_to_i420(src, width, height, stride, bit_order),
        other => Err(ScreenCaptureError::UnsupportedFormat(format!(
            "unsupported X11 bits-per-pixel: {}",
            other
        ))),
    }
}

fn x11_rgb24_to_i420(
    src: &[u8],
    width: u32,
    height: u32,
    stride: usize,
    bit_order: xproto::ImageOrder,
) -> Result<Vec<u8>, ScreenCaptureError> {
    let mut bgra = vec![0_u8; width as usize * height as usize * 4];
    for y in 0..height as usize {
        for x in 0..width as usize {
            let src_offset = y * stride + x * 3;
            let dst_offset = (y * width as usize + x) * 4;
            if src_offset + 2 >= src.len() {
                return Err(ScreenCaptureError::Conversion(
                    "X11 RGB24 buffer too small".to_string(),
                ));
            }
            if bit_order == xproto::ImageOrder::LSB_FIRST {
                bgra[dst_offset] = src[src_offset];
                bgra[dst_offset + 1] = src[src_offset + 1];
                bgra[dst_offset + 2] = src[src_offset + 2];
            } else {
                bgra[dst_offset] = src[src_offset + 2];
                bgra[dst_offset + 1] = src[src_offset + 1];
                bgra[dst_offset + 2] = src[src_offset];
            }
            bgra[dst_offset + 3] = 255;
        }
    }
    bgra_to_i420(&bgra, width, height, width as usize * 4)
}

fn blend_bgrx_pixel(pixel: &mut [u8], src_r: u8, src_g: u8, src_b: u8, alpha: u8) {
    let alpha = alpha as u16;
    let inv_alpha = 255_u16.saturating_sub(alpha);
    pixel[0] = (((src_b as u16 * alpha) + (pixel[0] as u16 * inv_alpha)) / 255) as u8;
    pixel[1] = (((src_g as u16 * alpha) + (pixel[1] as u16 * inv_alpha)) / 255) as u8;
    pixel[2] = (((src_r as u16 * alpha) + (pixel[2] as u16 * inv_alpha)) / 255) as u8;
}

async fn open_portal(
    cursor_mode: ScreenCaptureCursorMode,
) -> Result<(ScreencastStream, OwnedFd), ScreenCaptureError> {
    let proxy = Screencast::new()
        .await
        .map_err(|e| ScreenCaptureError::Backend(e.to_string()))?;
    let session = proxy
        .create_session(Default::default())
        .await
        .map_err(|e| ScreenCaptureError::Backend(e.to_string()))?;
    let portal_cursor = match cursor_mode {
        ScreenCaptureCursorMode::Hidden => CursorMode::Hidden,
        ScreenCaptureCursorMode::Embedded => CursorMode::Embedded,
    };
    proxy
        .select_sources(
            &session,
            SelectSourcesOptions::default()
                .set_cursor_mode(portal_cursor)
                .set_sources(SourceType::Monitor | SourceType::Window)
                .set_multiple(false)
                .set_restore_token(None)
                .set_persist_mode(PersistMode::DoNot),
        )
        .await
        .map_err(|e| ScreenCaptureError::PermissionOrSourceUnavailable(e.to_string()))?;

    let response = proxy
        .start(&session, None, Default::default())
        .await
        .map_err(|e| ScreenCaptureError::PermissionOrSourceUnavailable(e.to_string()))?
        .response()
        .map_err(|e| ScreenCaptureError::PermissionOrSourceUnavailable(e.to_string()))?;
    let stream = response
        .streams()
        .first()
        .cloned()
        .ok_or_else(|| ScreenCaptureError::Cancelled)?;
    let fd = proxy
        .open_pipe_wire_remote(&session, Default::default())
        .await
        .map_err(|e| ScreenCaptureError::Backend(e.to_string()))?;
    Ok((stream, fd))
}

struct PipeWireUserData {
    format: spa::param::video::VideoInfoRaw,
    last_preview_at: Option<Instant>,
    i420_slot: LatestSlot<I420ScreenFrame>,
    preview_slot: LatestSlot<PreviewFrame>,
    stats: Arc<CaptureStatsAtomic>,
    stop: Arc<AtomicBool>,
    profile: crate::ScreenCaptureProfile,
}

enum PipeWireControl {
    Stop,
}

fn run_pipewire_capture(
    node_id: u32,
    fd: OwnedFd,
    profile: crate::ScreenCaptureProfile,
    i420_slot: LatestSlot<I420ScreenFrame>,
    preview_slot: LatestSlot<PreviewFrame>,
    stats: Arc<CaptureStatsAtomic>,
    stop: Arc<AtomicBool>,
    control_rx: pw::channel::Receiver<PipeWireControl>,
) -> Result<(), ScreenCaptureError> {
    pw::init();
    let mainloop = pw::main_loop::MainLoopRc::new(None)
        .map_err(|e| ScreenCaptureError::Backend(e.to_string()))?;
    let context = pw::context::ContextBox::new(mainloop.loop_(), None)
        .map_err(|e| ScreenCaptureError::Backend(e.to_string()))?;
    let core = context
        .connect_fd(fd, None)
        .map_err(|e| ScreenCaptureError::Backend(e.to_string()))?;

    let stream = pw::stream::StreamBox::new(
        &core,
        "rchat-screen-capture",
        properties! {
            *pw::keys::MEDIA_TYPE => "Video",
            *pw::keys::MEDIA_CATEGORY => "Capture",
            *pw::keys::MEDIA_ROLE => "Screen",
        },
    )
    .map_err(|e| ScreenCaptureError::Backend(e.to_string()))?;

    let _control_receiver = control_rx.attach(mainloop.loop_(), {
        let mainloop = mainloop.clone();
        move |message| match message {
            PipeWireControl::Stop => mainloop.quit(),
        }
    });

    let data = PipeWireUserData {
        format: Default::default(),
        last_preview_at: None,
        i420_slot,
        preview_slot,
        stats,
        stop,
        profile,
    };

    let _listener = stream
        .add_local_listener_with_user_data(data)
        .param_changed(|_, user_data, id, param| {
            let Some(param) = param else {
                return;
            };
            if id != pw::spa::param::ParamType::Format.as_raw() {
                return;
            }
            let Ok((media_type, media_subtype)) = pw::spa::param::format_utils::parse_format(param)
            else {
                return;
            };
            if media_type != pw::spa::param::format::MediaType::Video
                || media_subtype != pw::spa::param::format::MediaSubtype::Raw
            {
                return;
            }
            let _ = user_data.format.parse(param);
        })
        .process(move |stream, user_data| {
            if user_data.stop.load(Ordering::Relaxed) {
                return;
            }
            let Some(mut buffer) = stream.dequeue_buffer() else {
                return;
            };
            let datas = buffer.datas_mut();
            if datas.is_empty() {
                return;
            }
            let data = &mut datas[0];
            let chunk = data.chunk();
            let offset = chunk.offset() as usize;
            let size = chunk.size() as usize;
            let stride = chunk.stride();
            let Some(bytes) = data.data() else {
                return;
            };
            if offset >= bytes.len() || size == 0 {
                return;
            }
            let end = offset.saturating_add(size).min(bytes.len());
            let frame_bytes = &bytes[offset..end];
            match convert_pipewire_frame(frame_bytes, stride, &user_data.format, user_data.profile)
            {
                Ok(frame) => {
                    user_data
                        .stats
                        .captured_frames
                        .fetch_add(1, Ordering::Relaxed);
                    let should_preview = user_data
                        .last_preview_at
                        .map(|last| last.elapsed() >= PREVIEW_INTERVAL)
                        .unwrap_or(true);
                    if should_preview {
                        match i420_to_preview_rgba(&frame.data, frame.width, frame.height) {
                            Ok(mut preview) => {
                                preview.timestamp_us = frame.timestamp_us;
                                user_data.preview_slot.replace(preview);
                                user_data
                                    .stats
                                    .preview_frames
                                    .fetch_add(1, Ordering::Relaxed);
                                user_data.last_preview_at = Some(Instant::now());
                            }
                            Err(error) => {
                                user_data
                                    .stats
                                    .conversion_errors
                                    .fetch_add(1, Ordering::Relaxed);
                                eprintln!(
                                    "[Screen][Capture] PipeWire preview conversion failed: {}",
                                    error
                                );
                            }
                        }
                    }
                    user_data.i420_slot.replace(frame);
                }
                Err(error) => {
                    user_data
                        .stats
                        .conversion_errors
                        .fetch_add(1, Ordering::Relaxed);
                    eprintln!(
                        "[Screen][Capture] PipeWire frame conversion failed: {}",
                        error
                    );
                }
            }
        })
        .register()
        .map_err(|e| ScreenCaptureError::Backend(e.to_string()))?;

    let (target_width, target_height) = profile.dimensions();
    let obj = pw::spa::pod::object!(
        pw::spa::utils::SpaTypes::ObjectParamFormat,
        pw::spa::param::ParamType::EnumFormat,
        pw::spa::pod::property!(
            pw::spa::param::format::FormatProperties::MediaType,
            Id,
            pw::spa::param::format::MediaType::Video
        ),
        pw::spa::pod::property!(
            pw::spa::param::format::FormatProperties::MediaSubtype,
            Id,
            pw::spa::param::format::MediaSubtype::Raw
        ),
        pw::spa::pod::property!(
            pw::spa::param::format::FormatProperties::VideoFormat,
            Choice,
            Enum,
            Id,
            pw::spa::param::video::VideoFormat::I420,
            pw::spa::param::video::VideoFormat::I420,
            pw::spa::param::video::VideoFormat::YUY2,
            pw::spa::param::video::VideoFormat::NV12,
            pw::spa::param::video::VideoFormat::BGRx,
            pw::spa::param::video::VideoFormat::RGBx,
            pw::spa::param::video::VideoFormat::RGBA,
            pw::spa::param::video::VideoFormat::RGB,
        ),
        pw::spa::pod::property!(
            pw::spa::param::format::FormatProperties::VideoSize,
            Choice,
            Range,
            Rectangle,
            pw::spa::utils::Rectangle {
                width: target_width,
                height: target_height
            },
            pw::spa::utils::Rectangle {
                width: 2,
                height: 2
            },
            pw::spa::utils::Rectangle {
                width: target_width,
                height: target_height
            }
        ),
        pw::spa::pod::property!(
            pw::spa::param::format::FormatProperties::VideoFramerate,
            Choice,
            Range,
            Fraction,
            pw::spa::utils::Fraction {
                num: profile.fps(),
                denom: 1
            },
            pw::spa::utils::Fraction { num: 0, denom: 1 },
            pw::spa::utils::Fraction {
                num: profile.fps(),
                denom: 1
            }
        ),
    );
    let values = pw::spa::pod::serialize::PodSerializer::serialize(
        std::io::Cursor::new(Vec::new()),
        &pw::spa::pod::Value::Object(obj),
    )
    .map_err(|e| ScreenCaptureError::Backend(e.to_string()))?
    .0
    .into_inner();
    let mut params = [spa::pod::Pod::from_bytes(&values).ok_or_else(|| {
        ScreenCaptureError::Backend("failed to build PipeWire stream format parameter".to_string())
    })?];

    stream
        .connect(
            spa::utils::Direction::Input,
            Some(node_id),
            pw::stream::StreamFlags::AUTOCONNECT | pw::stream::StreamFlags::MAP_BUFFERS,
            &mut params,
        )
        .map_err(|e| ScreenCaptureError::Backend(e.to_string()))?;

    mainloop.run();
    Ok(())
}

fn convert_pipewire_frame(
    frame_bytes: &[u8],
    stride: i32,
    format: &spa::param::video::VideoInfoRaw,
    profile: crate::ScreenCaptureProfile,
) -> Result<I420ScreenFrame, ScreenCaptureError> {
    let width = format.size().width;
    let height = format.size().height;
    let stride = if stride > 0 {
        stride as usize
    } else {
        default_stride(format.format(), width)
    };
    let data = match format.format() {
        spa::param::video::VideoFormat::I420 => {
            let y_size = stride * height as usize;
            let uv_stride = stride / 2;
            let uv_size = uv_stride * (height as usize / 2);
            if frame_bytes.len() < y_size + uv_size * 2 {
                return Err(ScreenCaptureError::Conversion(
                    "PipeWire I420 frame too small".to_string(),
                ));
            }
            i420_copy(
                &frame_bytes[..y_size],
                stride,
                &frame_bytes[y_size..y_size + uv_size],
                uv_stride,
                &frame_bytes[y_size + uv_size..y_size + uv_size * 2],
                uv_stride,
                width,
                height,
            )?
        }
        spa::param::video::VideoFormat::NV12 => {
            let y_size = stride * height as usize;
            if frame_bytes.len() < y_size {
                return Err(ScreenCaptureError::Conversion(
                    "PipeWire NV12 frame too small".to_string(),
                ));
            }
            nv12_to_i420(
                &frame_bytes[..y_size],
                stride,
                &frame_bytes[y_size..],
                stride,
                width,
                height,
            )?
        }
        spa::param::video::VideoFormat::YUY2 => yuyv_to_i420(frame_bytes, width, height, stride)?,
        spa::param::video::VideoFormat::BGRx => bgra_to_i420(frame_bytes, width, height, stride)?,
        spa::param::video::VideoFormat::RGBx | spa::param::video::VideoFormat::RGBA => {
            rgba_to_i420(frame_bytes, width, height, stride)?
        }
        spa::param::video::VideoFormat::RGB => {
            return Err(ScreenCaptureError::UnsupportedFormat(
                "packed RGB PipeWire frames are not supported yet".to_string(),
            ));
        }
        other => {
            return Err(ScreenCaptureError::UnsupportedFormat(format!(
                "unsupported PipeWire video format: {:?}",
                other
            )));
        }
    };
    let (target_width, target_height) = profile.dimensions();
    let (width, height, data) = if width > target_width || height > target_height {
        let (out_width, out_height) = clamp_to_profile(width, height, profile);
        let data = scale_i420_nearest(&data, width, height, out_width, out_height)?;
        (out_width, out_height, data)
    } else {
        (width, height, data)
    };
    Ok(I420ScreenFrame {
        timestamp_us: now_timestamp_us(),
        width,
        height,
        data,
    })
}

fn default_stride(format: spa::param::video::VideoFormat, width: u32) -> usize {
    match format {
        spa::param::video::VideoFormat::I420 | spa::param::video::VideoFormat::NV12 => {
            width as usize
        }
        spa::param::video::VideoFormat::YUY2 => width as usize * 2,
        _ => width as usize * 4,
    }
}
