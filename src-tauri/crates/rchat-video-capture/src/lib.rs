use image::ImageFormat;
use nokhwa::utils::{
    ApiBackend, CameraFormat, CameraIndex, FrameFormat, RequestedFormat, RequestedFormatType,
    Resolution,
};
use nokhwa::{query, Camera};
use std::borrow::Cow;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub const CAPTURE_FPS: u32 = 30;
pub const PREVIEW_MAX_WIDTH: u32 = 320;
pub const PREVIEW_INTERVAL: Duration = Duration::from_millis(200);
const SUPPORTED_CAMERA_FRAME_FORMATS: &[FrameFormat] = &[
    FrameFormat::YUYV,
    FrameFormat::NV12,
    FrameFormat::RAWRGB,
    FrameFormat::RAWBGR,
    FrameFormat::MJPEG,
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureProfile {
    P360,
    P480,
    P720,
}

impl CaptureProfile {
    pub fn dimensions(self) -> (u32, u32) {
        match self {
            Self::P360 => (640, 360),
            Self::P480 => (854, 480),
            Self::P720 => (1280, 720),
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::P360 => "360p30",
            Self::P480 => "480p30",
            Self::P720 => "720p30",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaptureConfig {
    pub profile: CaptureProfile,
    pub device_index: Option<u32>,
}

impl CaptureConfig {
    pub fn default_for_profile(profile: CaptureProfile) -> Self {
        Self {
            profile,
            device_index: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaptureDeviceInfo {
    pub id: String,
    pub index: u32,
    pub name: String,
    pub description: String,
    pub backend: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaptureFormatInfo {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub format: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct I420Frame {
    pub timestamp_us: i64,
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreviewFrame {
    pub timestamp_us: i64,
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CaptureSessionStats {
    pub captured_frames: u64,
    pub dropped_i420_frames: u64,
    pub dropped_preview_frames: u64,
    pub conversion_errors: u64,
    pub preview_frames: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaptureSessionInfo {
    pub backend: String,
    pub device_name: String,
    pub requested_profile: String,
    pub format: CaptureFormatInfo,
}

#[derive(Debug, thiserror::Error)]
pub enum VideoCaptureError {
    #[error("native camera capture is unavailable on this platform")]
    UnsupportedPlatform,
    #[error("no camera device found")]
    NoDevice,
    #[error("camera permission denied or device unavailable: {0}")]
    PermissionOrDeviceUnavailable(String),
    #[error("camera format unsupported: {0}")]
    UnsupportedFormat(String),
    #[error("camera frame conversion failed: {0}")]
    Conversion(String),
    #[error("camera backend error: {0}")]
    Backend(String),
}

pub fn initialize_platform() -> Result<(), VideoCaptureError> {
    if native_backend().is_none() {
        return Err(VideoCaptureError::UnsupportedPlatform);
    }
    Ok(())
}

pub fn list_devices() -> Result<Vec<CaptureDeviceInfo>, VideoCaptureError> {
    let backend = native_backend().ok_or(VideoCaptureError::UnsupportedPlatform)?;
    let devices = query(backend).map_err(map_nokhwa_error)?;
    Ok(devices
        .into_iter()
        .enumerate()
        .map(|(fallback_index, info)| {
            let index = camera_index_as_u32(info.index()).unwrap_or(fallback_index as u32);
            CaptureDeviceInfo {
                id: info.misc(),
                index,
                name: info.human_name(),
                description: info.description().to_string(),
                backend: backend_label(backend).to_string(),
            }
        })
        .collect())
}

pub fn probe_default_camera(
    profile: CaptureProfile,
) -> Result<CaptureSessionInfo, VideoCaptureError> {
    let config = CaptureConfig::default_for_profile(profile);
    let camera = open_camera(&config)?;
    let format = camera.camera_format();
    let info = camera.info().human_name();
    Ok(CaptureSessionInfo {
        backend: backend_label(camera.backend()).to_string(),
        device_name: info,
        requested_profile: profile.label().to_string(),
        format: capture_format_info(format),
    })
}

pub struct VideoCaptureSession {
    info: CaptureSessionInfo,
    i420_slot: LatestSlot<I420Frame>,
    preview_slot: LatestSlot<PreviewFrame>,
    stop: Arc<AtomicBool>,
    stats: Arc<CaptureStatsAtomic>,
    handle: Option<JoinHandle<()>>,
}

impl VideoCaptureSession {
    pub fn start(config: CaptureConfig) -> Result<Self, VideoCaptureError> {
        let i420_slot = LatestSlot::default();
        let preview_slot = LatestSlot::default();
        let stop = Arc::new(AtomicBool::new(false));
        let stats = Arc::new(CaptureStatsAtomic::default());
        let thread_stop = Arc::clone(&stop);
        let thread_stats = Arc::clone(&stats);
        let thread_i420_slot = i420_slot.clone();
        let thread_preview_slot = preview_slot.clone();
        let (init_tx, init_rx) = mpsc::sync_channel(1);
        let handle = thread::Builder::new()
            .name("rchat-video-capture".to_string())
            .spawn(move || {
                let mut camera = match open_camera(&config) {
                    Ok(camera) => camera,
                    Err(error) => {
                        let _ = init_tx.send(Err(error));
                        return;
                    }
                };
                let format = camera.camera_format();
                let info = CaptureSessionInfo {
                    backend: backend_label(camera.backend()).to_string(),
                    device_name: camera.info().human_name(),
                    requested_profile: config.profile.label().to_string(),
                    format: capture_format_info(format),
                };
                if let Err(error) = camera.open_stream().map_err(map_nokhwa_error) {
                    let _ = init_tx.send(Err(error));
                    return;
                }
                let _ = init_tx.send(Ok(info));
                capture_loop(
                    camera,
                    thread_i420_slot,
                    thread_preview_slot,
                    thread_stop,
                    thread_stats,
                    format,
                );
            })
            .map_err(|e| VideoCaptureError::Backend(e.to_string()))?;
        let info = match init_rx.recv() {
            Ok(Ok(info)) => info,
            Ok(Err(error)) => {
                stop.store(true, Ordering::Relaxed);
                let _ = handle.join();
                return Err(error);
            }
            Err(e) => {
                stop.store(true, Ordering::Relaxed);
                let _ = handle.join();
                return Err(VideoCaptureError::Backend(e.to_string()));
            }
        };

        Ok(Self {
            info,
            i420_slot,
            preview_slot,
            stop,
            stats,
            handle: Some(handle),
        })
    }

    pub fn info(&self) -> &CaptureSessionInfo {
        &self.info
    }

    pub fn try_recv_latest_i420(&self) -> Option<I420Frame> {
        self.i420_slot.take()
    }

    pub fn try_recv_latest_preview(&self) -> Option<PreviewFrame> {
        self.preview_slot.take()
    }

    pub fn stats(&self) -> CaptureSessionStats {
        self.stats.snapshot()
    }
}

impl Drop for VideoCaptureSession {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

#[derive(Default)]
struct CaptureStatsAtomic {
    captured_frames: AtomicU64,
    dropped_i420_frames: AtomicU64,
    dropped_preview_frames: AtomicU64,
    conversion_errors: AtomicU64,
    preview_frames: AtomicU64,
}

struct LatestSlot<T>(Arc<Mutex<Option<T>>>);

impl<T> Default for LatestSlot<T> {
    fn default() -> Self {
        Self(Arc::new(Mutex::new(None)))
    }
}

impl<T> Clone for LatestSlot<T> {
    fn clone(&self) -> Self {
        Self(Arc::clone(&self.0))
    }
}

impl<T> LatestSlot<T> {
    fn replace(&self, value: T) -> bool {
        let mut guard = self.0.lock().unwrap();
        let dropped = guard.is_some();
        *guard = Some(value);
        dropped
    }

    fn take(&self) -> Option<T> {
        self.0.lock().unwrap().take()
    }
}

impl CaptureStatsAtomic {
    fn snapshot(&self) -> CaptureSessionStats {
        CaptureSessionStats {
            captured_frames: self.captured_frames.load(Ordering::Relaxed),
            dropped_i420_frames: self.dropped_i420_frames.load(Ordering::Relaxed),
            dropped_preview_frames: self.dropped_preview_frames.load(Ordering::Relaxed),
            conversion_errors: self.conversion_errors.load(Ordering::Relaxed),
            preview_frames: self.preview_frames.load(Ordering::Relaxed),
        }
    }
}

fn capture_loop(
    mut camera: Camera,
    i420_slot: LatestSlot<I420Frame>,
    preview_slot: LatestSlot<PreviewFrame>,
    stop: Arc<AtomicBool>,
    stats: Arc<CaptureStatsAtomic>,
    format: CameraFormat,
) {
    let mut last_preview = Instant::now()
        .checked_sub(PREVIEW_INTERVAL)
        .unwrap_or_else(Instant::now);
    while !stop.load(Ordering::Relaxed) {
        match camera.frame_raw() {
            Ok(raw) => {
                stats.captured_frames.fetch_add(1, Ordering::Relaxed);
                match raw_to_i420_frame(&raw, format) {
                    Ok(frame) => {
                        if i420_slot.replace(frame.clone()) {
                            stats.dropped_i420_frames.fetch_add(1, Ordering::Relaxed);
                        }
                        if last_preview.elapsed() >= PREVIEW_INTERVAL {
                            match i420_to_preview(&frame, PREVIEW_MAX_WIDTH) {
                                Ok(preview) => {
                                    if preview_slot.replace(preview) {
                                        stats
                                            .dropped_preview_frames
                                            .fetch_add(1, Ordering::Relaxed);
                                    }
                                    stats.preview_frames.fetch_add(1, Ordering::Relaxed);
                                }
                                Err(_) => {
                                    stats.conversion_errors.fetch_add(1, Ordering::Relaxed);
                                }
                            }
                            last_preview = Instant::now();
                        }
                    }
                    Err(_) => {
                        stats.conversion_errors.fetch_add(1, Ordering::Relaxed);
                    }
                }
            }
            Err(_) => {
                thread::sleep(Duration::from_millis(5));
            }
        }
    }
    let _ = camera.stop_stream();
}

fn open_camera(config: &CaptureConfig) -> Result<Camera, VideoCaptureError> {
    initialize_platform()?;
    let backend = native_backend().ok_or(VideoCaptureError::UnsupportedPlatform)?;
    let index = CameraIndex::Index(config.device_index.unwrap_or(0));
    let requested = initial_camera_open_request();
    let mut camera = Camera::with_backend(index, requested, backend).map_err(map_nokhwa_error)?;
    let format = select_camera_format(&mut camera, config.profile)?;
    camera
        .set_camera_requset(exact_camera_format_request(format))
        .map_err(map_nokhwa_error)?;
    Ok(camera)
}

fn select_camera_format(
    camera: &mut Camera,
    profile: CaptureProfile,
) -> Result<CameraFormat, VideoCaptureError> {
    let formats = match camera.compatible_camera_formats() {
        Ok(formats) if !formats.is_empty() => formats,
        _ => {
            let (width, height) = profile.dimensions();
            return Ok(CameraFormat::new(
                Resolution::new(width, height),
                FrameFormat::YUYV,
                CAPTURE_FPS,
            ));
        }
    };

    select_camera_format_from_formats(&formats, profile)
}

fn select_camera_format_from_formats(
    formats: &[CameraFormat],
    profile: CaptureProfile,
) -> Result<CameraFormat, VideoCaptureError> {
    let (target_width, target_height) = profile.dimensions();
    let target_area = target_width as u64 * target_height as u64;
    let mut best: Option<(u64, CameraFormat)> = None;
    for format in formats.iter().copied() {
        let Some(format_rank) = frame_format_rank(format.format()) else {
            continue;
        };
        if format.width() % 2 != 0 || format.height() % 2 != 0 {
            continue;
        }
        let area = format.width() as u64 * format.height() as u64;
        let area_delta = target_area.abs_diff(area);
        let dimension_delta = target_width.abs_diff(format.width()) as u64
            + target_height.abs_diff(format.height()) as u64;
        let fps_delta = CAPTURE_FPS.abs_diff(format.frame_rate()) as u64;
        let undersize_penalty = if format.width() < target_width || format.height() < target_height
        {
            250_000
        } else {
            0
        };
        let score = area_delta.saturating_mul(10)
            + dimension_delta.saturating_mul(100)
            + fps_delta.saturating_mul(1_000)
            + format_rank.saturating_mul(10_000)
            + undersize_penalty;
        if best
            .map(|(best_score, _)| score < best_score)
            .unwrap_or(true)
        {
            best = Some((score, format));
        }
    }
    best.map(|(_, format)| format)
        .ok_or_else(|| VideoCaptureError::UnsupportedFormat("no supported camera format".into()))
}

fn frame_format_rank(format: FrameFormat) -> Option<u64> {
    match format {
        FrameFormat::YUYV => Some(0),
        FrameFormat::NV12 => Some(1),
        FrameFormat::RAWRGB => Some(2),
        FrameFormat::RAWBGR => Some(3),
        FrameFormat::MJPEG => Some(4),
        _ => None,
    }
}

fn initial_camera_open_request() -> RequestedFormat<'static> {
    RequestedFormat::with_formats(RequestedFormatType::None, SUPPORTED_CAMERA_FRAME_FORMATS)
}

fn exact_camera_format_request(format: CameraFormat) -> RequestedFormat<'static> {
    RequestedFormat::with_formats(
        RequestedFormatType::Exact(format),
        SUPPORTED_CAMERA_FRAME_FORMATS,
    )
}

fn raw_to_i420_frame(
    raw: &Cow<'_, [u8]>,
    format: CameraFormat,
) -> Result<I420Frame, VideoCaptureError> {
    let width = format.width();
    let height = format.height();
    let data = match format.format() {
        FrameFormat::YUYV => yuyv_to_i420(raw, width, height)?,
        FrameFormat::NV12 => nv12_to_i420(raw, width, height)?,
        FrameFormat::RAWRGB => rgb_to_i420(raw, width, height, false)?,
        FrameFormat::RAWBGR => rgb_to_i420(raw, width, height, true)?,
        FrameFormat::MJPEG => mjpeg_to_i420(raw, width, height)?,
        other => {
            return Err(VideoCaptureError::UnsupportedFormat(other.to_string()));
        }
    };
    Ok(I420Frame {
        timestamp_us: now_us(),
        width,
        height,
        data,
    })
}

pub fn expected_i420_len(width: u32, height: u32) -> Option<usize> {
    if width == 0 || height == 0 || width % 2 != 0 || height % 2 != 0 {
        return None;
    }
    let pixels = width.checked_mul(height)? as usize;
    Some(pixels + pixels / 2)
}

pub fn yuyv_to_i420(data: &[u8], width: u32, height: u32) -> Result<Vec<u8>, VideoCaptureError> {
    let width_usize = width as usize;
    let height_usize = height as usize;
    if expected_i420_len(width, height).is_none() {
        return Err(VideoCaptureError::Conversion("invalid dimensions".into()));
    }
    let expected = width_usize
        .checked_mul(height_usize)
        .and_then(|px| px.checked_mul(2))
        .ok_or_else(|| VideoCaptureError::Conversion("frame too large".into()))?;
    if data.len() < expected {
        return Err(VideoCaptureError::Conversion("short YUYV frame".into()));
    }
    let y_len = width_usize * height_usize;
    let uv_len = y_len / 4;
    let mut out = vec![0u8; y_len + uv_len * 2];
    let (y_plane, uv) = out.split_at_mut(y_len);
    let (u_plane, v_plane) = uv.split_at_mut(uv_len);
    for row in 0..height_usize {
        for col in (0..width_usize).step_by(2) {
            let src = (row * width_usize + col) * 2;
            y_plane[row * width_usize + col] = data[src];
            y_plane[row * width_usize + col + 1] = data[src + 2];
            if row % 2 == 0 {
                let uv_index = (row / 2) * (width_usize / 2) + col / 2;
                u_plane[uv_index] = data[src + 1];
                v_plane[uv_index] = data[src + 3];
            }
        }
    }
    Ok(out)
}

pub fn nv12_to_i420(data: &[u8], width: u32, height: u32) -> Result<Vec<u8>, VideoCaptureError> {
    let width_usize = width as usize;
    let height_usize = height as usize;
    let expected_len = expected_i420_len(width, height)
        .ok_or_else(|| VideoCaptureError::Conversion("invalid dimensions".into()))?;
    if data.len() < expected_len {
        return Err(VideoCaptureError::Conversion("short NV12 frame".into()));
    }
    let y_len = width_usize * height_usize;
    let uv_len = y_len / 4;
    let mut out = vec![0u8; expected_len];
    out[..y_len].copy_from_slice(&data[..y_len]);
    let (u_plane, v_plane) = out[y_len..].split_at_mut(uv_len);
    let uv_src = &data[y_len..y_len + uv_len * 2];
    for i in 0..uv_len {
        u_plane[i] = uv_src[i * 2];
        v_plane[i] = uv_src[i * 2 + 1];
    }
    Ok(out)
}

fn rgb_to_i420(
    data: &[u8],
    width: u32,
    height: u32,
    bgr: bool,
) -> Result<Vec<u8>, VideoCaptureError> {
    let expected_rgb = (width as usize)
        .checked_mul(height as usize)
        .and_then(|px| px.checked_mul(3))
        .ok_or_else(|| VideoCaptureError::Conversion("frame too large".into()))?;
    if data.len() < expected_rgb {
        return Err(VideoCaptureError::Conversion("short RGB frame".into()));
    }
    rgba_like_to_i420(data, width, height, 3, bgr)
}

fn mjpeg_to_i420(data: &[u8], width: u32, height: u32) -> Result<Vec<u8>, VideoCaptureError> {
    let decoded = image::load_from_memory_with_format(data, ImageFormat::Jpeg)
        .map_err(|e| VideoCaptureError::Conversion(e.to_string()))?
        .to_rgba8();
    if decoded.width() != width || decoded.height() != height {
        return Err(VideoCaptureError::Conversion(
            "MJPEG decoded dimensions mismatch".into(),
        ));
    }
    rgba_like_to_i420(decoded.as_raw(), width, height, 4, false)
}

fn rgba_like_to_i420(
    data: &[u8],
    width: u32,
    height: u32,
    stride: usize,
    bgr: bool,
) -> Result<Vec<u8>, VideoCaptureError> {
    let expected_len = expected_i420_len(width, height)
        .ok_or_else(|| VideoCaptureError::Conversion("invalid dimensions".into()))?;
    let width_usize = width as usize;
    let height_usize = height as usize;
    if data.len() < width_usize * height_usize * stride {
        return Err(VideoCaptureError::Conversion("short RGBA frame".into()));
    }
    let y_len = width_usize * height_usize;
    let uv_len = y_len / 4;
    let mut out = vec![0u8; expected_len];
    let (y_plane, uv) = out.split_at_mut(y_len);
    let (u_plane, v_plane) = uv.split_at_mut(uv_len);

    for row in 0..height_usize {
        for col in 0..width_usize {
            let (r, g, b) = read_rgb(data, row, col, width_usize, stride, bgr);
            y_plane[row * width_usize + col] = rgb_to_y(r, g, b);
        }
    }
    for row in (0..height_usize).step_by(2) {
        for col in (0..width_usize).step_by(2) {
            let mut r_sum = 0u16;
            let mut g_sum = 0u16;
            let mut b_sum = 0u16;
            for dy in 0..2 {
                for dx in 0..2 {
                    let (r, g, b) = read_rgb(data, row + dy, col + dx, width_usize, stride, bgr);
                    r_sum += r as u16;
                    g_sum += g as u16;
                    b_sum += b as u16;
                }
            }
            let r = (r_sum / 4) as u8;
            let g = (g_sum / 4) as u8;
            let b = (b_sum / 4) as u8;
            let uv_index = (row / 2) * (width_usize / 2) + col / 2;
            u_plane[uv_index] = rgb_to_u(r, g, b);
            v_plane[uv_index] = rgb_to_v(r, g, b);
        }
    }
    Ok(out)
}

fn read_rgb(
    data: &[u8],
    row: usize,
    col: usize,
    width: usize,
    stride: usize,
    bgr: bool,
) -> (u8, u8, u8) {
    let idx = (row * width + col) * stride;
    if bgr {
        (data[idx + 2], data[idx + 1], data[idx])
    } else {
        (data[idx], data[idx + 1], data[idx + 2])
    }
}

fn i420_to_preview(frame: &I420Frame, max_width: u32) -> Result<PreviewFrame, VideoCaptureError> {
    let scale = (frame.width as f64 / max_width as f64).ceil().max(1.0) as u32;
    let width = (frame.width / scale).max(1);
    let height = (frame.height / scale).max(1);
    let mut rgba = Vec::with_capacity(width as usize * height as usize * 4);
    for y in 0..height {
        let src_y = (y * scale).min(frame.height - 1);
        for x in 0..width {
            let src_x = (x * scale).min(frame.width - 1);
            let (r, g, b) =
                i420_pixel_to_rgb(&frame.data, frame.width, frame.height, src_x, src_y)?;
            rgba.extend_from_slice(&[r, g, b, 255]);
        }
    }
    Ok(PreviewFrame {
        timestamp_us: frame.timestamp_us,
        width,
        height,
        rgba,
    })
}

fn i420_pixel_to_rgb(
    data: &[u8],
    width: u32,
    height: u32,
    x: u32,
    y: u32,
) -> Result<(u8, u8, u8), VideoCaptureError> {
    let expected_len = expected_i420_len(width, height)
        .ok_or_else(|| VideoCaptureError::Conversion("invalid preview dimensions".into()))?;
    if data.len() < expected_len {
        return Err(VideoCaptureError::Conversion(
            "short I420 preview frame".into(),
        ));
    }
    let width_usize = width as usize;
    let height_usize = height as usize;
    let y_len = width_usize * height_usize;
    let uv_len = y_len / 4;
    let y_index = y as usize * width_usize + x as usize;
    let uv_index = (y as usize / 2) * (width_usize / 2) + (x as usize / 2);
    let yy = data[y_index] as i32;
    let uu = data[y_len + uv_index] as i32 - 128;
    let vv = data[y_len + uv_len + uv_index] as i32 - 128;
    let c = yy - 16;
    let r = clamp_u8((298 * c + 409 * vv + 128) >> 8);
    let g = clamp_u8((298 * c - 100 * uu - 208 * vv + 128) >> 8);
    let b = clamp_u8((298 * c + 516 * uu + 128) >> 8);
    Ok((r, g, b))
}

fn rgb_to_y(r: u8, g: u8, b: u8) -> u8 {
    clamp_u8(((66 * r as i32 + 129 * g as i32 + 25 * b as i32 + 128) >> 8) + 16)
}

fn rgb_to_u(r: u8, g: u8, b: u8) -> u8 {
    clamp_u8(((-38 * r as i32 - 74 * g as i32 + 112 * b as i32 + 128) >> 8) + 128)
}

fn rgb_to_v(r: u8, g: u8, b: u8) -> u8 {
    clamp_u8(((112 * r as i32 - 94 * g as i32 - 18 * b as i32 + 128) >> 8) + 128)
}

fn clamp_u8(value: i32) -> u8 {
    value.clamp(0, 255) as u8
}

fn capture_format_info(format: CameraFormat) -> CaptureFormatInfo {
    CaptureFormatInfo {
        width: format.width(),
        height: format.height(),
        fps: format.frame_rate(),
        format: format.format().to_string(),
    }
}

fn native_backend() -> Option<ApiBackend> {
    nokhwa::native_api_backend()
}

fn backend_label(backend: ApiBackend) -> &'static str {
    match backend {
        ApiBackend::AVFoundation => "avfoundation",
        ApiBackend::Video4Linux => "v4l2",
        ApiBackend::MediaFoundation => "mediafoundation",
        ApiBackend::Auto => "auto",
        _ => "other",
    }
}

fn camera_index_as_u32(index: &CameraIndex) -> Option<u32> {
    match index {
        CameraIndex::Index(index) => Some(*index),
        CameraIndex::String(value) => value.parse().ok(),
    }
}

fn map_nokhwa_error(error: nokhwa::NokhwaError) -> VideoCaptureError {
    let message = error.to_string();
    let lower = message.to_lowercase();
    if lower.contains("permission") || lower.contains("denied") || lower.contains("busy") {
        VideoCaptureError::PermissionOrDeviceUnavailable(message)
    } else if lower.contains("not found") || lower.contains("no device") {
        VideoCaptureError::NoDevice
    } else {
        VideoCaptureError::Backend(message)
    }
}

fn now_us() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_micros()
        .min(i64::MAX as u128) as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn yuyv_to_i420_2x2_matches_expected_planes() {
        let yuyv = [10, 100, 20, 150, 30, 110, 40, 160];
        let i420 = yuyv_to_i420(&yuyv, 2, 2).expect("converts");
        assert_eq!(i420, vec![10, 20, 30, 40, 100, 150]);
    }

    #[test]
    fn nv12_to_i420_deinterleaves_chroma() {
        let nv12 = [1, 2, 3, 4, 90, 140];
        let i420 = nv12_to_i420(&nv12, 2, 2).expect("converts");
        assert_eq!(i420, vec![1, 2, 3, 4, 90, 140]);
    }

    #[test]
    fn rejects_odd_or_empty_dimensions() {
        assert!(expected_i420_len(0, 2).is_none());
        assert!(expected_i420_len(3, 2).is_none());
        assert!(yuyv_to_i420(&[], 3, 2).is_err());
        assert!(nv12_to_i420(&[], 2, 3).is_err());
    }

    #[test]
    fn latest_frame_queue_drops_stale_frames() {
        let slot = LatestSlot::default();
        assert!(!slot.replace(1));
        assert!(slot.replace(2));
        assert_eq!(slot.take(), Some(2));
        assert_eq!(slot.take(), None);
    }

    #[test]
    fn profile_selection_prefers_720p30_then_fallbacks() {
        let formats = [
            CameraFormat::new(Resolution::new(640, 360), FrameFormat::YUYV, 30),
            CameraFormat::new(Resolution::new(1280, 720), FrameFormat::YUYV, 30),
            CameraFormat::new(Resolution::new(1280, 720), FrameFormat::MJPEG, 30),
        ];
        let mut best = None;
        for format in formats {
            if format.format() == FrameFormat::YUYV
                && format.width() == 1280
                && format.height() == 720
            {
                best = Some(format);
            }
        }
        assert_eq!(best.unwrap().format(), FrameFormat::YUYV);
    }

    #[test]
    fn profile_selection_prefers_requested_resolution_before_format_family() {
        let formats = [
            CameraFormat::new(Resolution::new(1280, 720), FrameFormat::YUYV, 30),
            CameraFormat::new(Resolution::new(640, 360), FrameFormat::MJPEG, 30),
        ];
        let selected =
            select_camera_format_from_formats(&formats, CaptureProfile::P360).expect("selects");

        assert_eq!(selected.width(), 640);
        assert_eq!(selected.height(), 360);
        assert_eq!(selected.format(), FrameFormat::MJPEG);
    }

    #[test]
    fn initial_camera_open_request_accepts_non_yuyv_formats() {
        let formats = [CameraFormat::new(
            Resolution::new(640, 360),
            FrameFormat::MJPEG,
            30,
        )];
        let selected = initial_camera_open_request()
            .fulfill(&formats)
            .expect("initial request accepts available supported format");

        assert_eq!(selected.format(), FrameFormat::MJPEG);
    }

    #[test]
    #[ignore]
    fn hardware_smoke_test() {
        if std::env::var("RCHAT_CAMERA_TEST").ok().as_deref() != Some("1") {
            return;
        }
        let session =
            VideoCaptureSession::start(CaptureConfig::default_for_profile(CaptureProfile::P360))
                .expect("camera starts");
        std::thread::sleep(Duration::from_millis(250));
        assert!(session.try_recv_latest_i420().is_some());
    }
}
