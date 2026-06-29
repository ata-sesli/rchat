use crate::{
    bgra_to_i420, clamp_to_profile, i420_copy, i420_to_preview_rgba, nv12_to_i420,
    now_timestamp_us, rgba_to_i420, scale_i420_nearest, yuyv_to_i420, CaptureStatsAtomic,
    I420ScreenFrame, LatestSlot, PreviewFrame, ScreenCaptureBackend, ScreenCaptureConfig,
    ScreenCaptureCursorMode, ScreenCaptureError, ScreenCaptureFormatInfo,
    ScreenCaptureSessionInfo, ScreenCaptureSessionStats, ScreenCaptureSupport, PREVIEW_INTERVAL,
};
use ashpd::desktop::{
    screencast::{
        CursorMode, Screencast, SelectSourcesOptions, SourceType, Stream as ScreencastStream,
    },
    PersistMode,
};
use pipewire as pw;
use pw::{properties::properties, spa};
use std::os::fd::OwnedFd;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Instant;

pub struct PlatformScreenCaptureSession {
    info: ScreenCaptureSessionInfo,
    i420_slot: LatestSlot<I420ScreenFrame>,
    preview_slot: LatestSlot<PreviewFrame>,
    stop: Arc<AtomicBool>,
    control_tx: pw::channel::Sender<PipeWireControl>,
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
        let _ = self.control_tx.send(PipeWireControl::Stop);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

pub fn screen_capture_support() -> ScreenCaptureSupport {
    ScreenCaptureSupport {
        supported: true,
        reason: None,
        backend: ScreenCaptureBackend::LinuxPortalPipeWire,
    }
}

pub async fn start_session(
    config: ScreenCaptureConfig,
) -> Result<PlatformScreenCaptureSession, ScreenCaptureError> {
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
        control_tx,
        stats,
        handle: Some(handle),
    })
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
            let Ok((media_type, media_subtype)) =
                pw::spa::param::format_utils::parse_format(param)
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
            let Some(bytes) = data.data() else {
                return;
            };
            let chunk = data.chunk();
            let offset = chunk.offset() as usize;
            let size = chunk.size() as usize;
            if offset >= bytes.len() || size == 0 {
                return;
            }
            let end = offset.saturating_add(size).min(bytes.len());
            let frame_bytes = &bytes[offset..end];
            match convert_pipewire_frame(frame_bytes, chunk.stride(), &user_data.format, user_data.profile) {
                Ok(frame) => {
                    user_data.stats.captured_frames.fetch_add(1, Ordering::Relaxed);
                    let should_preview = user_data
                        .last_preview_at
                        .map(|last| last.elapsed() >= PREVIEW_INTERVAL)
                        .unwrap_or(true);
                    if should_preview {
                        match i420_to_preview_rgba(&frame.data, frame.width, frame.height) {
                            Ok(mut preview) => {
                                preview.timestamp_us = frame.timestamp_us;
                                user_data.preview_slot.replace(preview);
                                user_data.stats.preview_frames.fetch_add(1, Ordering::Relaxed);
                                user_data.last_preview_at = Some(Instant::now());
                            }
                            Err(error) => {
                                user_data.stats.conversion_errors.fetch_add(1, Ordering::Relaxed);
                                eprintln!("[Screen][Capture] PipeWire preview conversion failed: {}", error);
                            }
                        }
                    }
                    user_data.i420_slot.replace(frame);
                }
                Err(error) => {
                    user_data.stats.conversion_errors.fetch_add(1, Ordering::Relaxed);
                    eprintln!("[Screen][Capture] PipeWire frame conversion failed: {}", error);
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
            pw::spa::utils::Rectangle { width: 2, height: 2 },
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
                return Err(ScreenCaptureError::Conversion("PipeWire I420 frame too small".to_string()));
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
                return Err(ScreenCaptureError::Conversion("PipeWire NV12 frame too small".to_string()));
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
