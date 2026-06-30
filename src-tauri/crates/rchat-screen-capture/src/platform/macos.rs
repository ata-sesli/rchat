use crate::{
    bgra_to_i420, clamp_to_profile, i420_to_preview_rgba, now_timestamp_us, scale_i420_nearest,
    CaptureStatsAtomic, I420ScreenFrame, PreviewFrame, ScreenCaptureBackend, ScreenCaptureConfig,
    ScreenCaptureCursorMode, ScreenCaptureError, ScreenCaptureFormatInfo, ScreenCaptureSessionInfo,
    ScreenCaptureSessionStats, ScreenCaptureSupport, PREVIEW_INTERVAL,
};
use screencapturekit::async_api::{AsyncSCContentSharingPicker, AsyncSCStream};
use screencapturekit::cm::{CMSampleBufferExt, CMSampleBufferSCExt, CMTime, SCFrameStatus};
use screencapturekit::content_sharing_picker::{
    SCContentSharingPickerConfiguration, SCContentSharingPickerMode, SCPickerOutcome,
};
use screencapturekit::cv::CVPixelBufferLockFlags;
use screencapturekit::stream::configuration::{PixelFormat, SCStreamConfiguration};
use screencapturekit::stream::output_type::SCStreamOutputType;
use std::sync::atomic::Ordering;
use std::time::Instant;

pub struct PlatformScreenCaptureSession {
    info: ScreenCaptureSessionInfo,
    stream: AsyncSCStream,
    stats: CaptureStatsAtomic,
    preview_slot: crate::LatestSlot<PreviewFrame>,
    last_preview_at: Option<Instant>,
}

impl PlatformScreenCaptureSession {
    pub fn info(&self) -> &ScreenCaptureSessionInfo {
        &self.info
    }

    pub fn try_recv_latest_i420(&mut self) -> Option<I420ScreenFrame> {
        let mut latest = None;
        while let Some(sample) = self.stream.try_next() {
            match self.convert_sample(sample) {
                Ok(Some(frame)) => latest = Some(frame),
                Ok(None) => {}
                Err(error) => {
                    self.stats.conversion_errors.fetch_add(1, Ordering::Relaxed);
                    eprintln!("[Screen][Capture] macOS frame conversion failed: {}", error);
                }
            }
        }
        latest
    }

    pub fn try_recv_latest_preview(&mut self) -> Option<PreviewFrame> {
        self.preview_slot.take()
    }

    pub fn stats(&self) -> ScreenCaptureSessionStats {
        self.stats.snapshot(0, self.preview_slot.dropped())
    }

    fn convert_sample(
        &mut self,
        sample: screencapturekit::cm::CMSampleBuffer,
    ) -> Result<Option<I420ScreenFrame>, ScreenCaptureError> {
        if should_skip_sample_status(sample.frame_status()) {
            return Ok(None);
        }

        let pixel_buffer = sample.image_buffer().ok_or_else(|| {
            ScreenCaptureError::Conversion("sample has no image buffer".to_string())
        })?;
        let guard = pixel_buffer
            .lock(CVPixelBufferLockFlags::READ_ONLY)
            .map_err(|e| {
                ScreenCaptureError::Conversion(format!("pixel buffer lock failed: {e:?}"))
            })?;
        let width = guard.width() as u32;
        let height = guard.height() as u32;
        let stride = guard.bytes_per_row();
        let (out_width, out_height) =
            clamp_to_profile(width, height, crate::ScreenCaptureProfile::P720);
        let i420 = bgra_to_i420(guard.as_slice(), width, height, stride)?;
        let i420 = if out_width != width || out_height != height {
            scale_i420_nearest(&i420, width, height, out_width, out_height)?
        } else {
            i420
        };
        let frame = I420ScreenFrame {
            timestamp_us: now_timestamp_us(),
            width: out_width,
            height: out_height,
            data: i420,
        };
        self.stats.captured_frames.fetch_add(1, Ordering::Relaxed);

        let should_preview = self
            .last_preview_at
            .map(|last| last.elapsed() >= PREVIEW_INTERVAL)
            .unwrap_or(true);
        if should_preview {
            match i420_to_preview_rgba(&frame.data, frame.width, frame.height) {
                Ok(mut preview) => {
                    preview.timestamp_us = frame.timestamp_us;
                    self.preview_slot.replace(preview);
                    self.stats.preview_frames.fetch_add(1, Ordering::Relaxed);
                    self.last_preview_at = Some(Instant::now());
                }
                Err(error) => {
                    self.stats.conversion_errors.fetch_add(1, Ordering::Relaxed);
                    eprintln!(
                        "[Screen][Capture] macOS preview conversion failed: {}",
                        error
                    );
                }
            }
        }

        Ok(Some(frame))
    }
}

fn should_skip_sample_status(status: Option<SCFrameStatus>) -> bool {
    matches!(status, Some(status) if !status.has_content())
}

impl Drop for PlatformScreenCaptureSession {
    fn drop(&mut self) {
        let _ = self.stream.clear_buffer();
    }
}

pub async fn screen_capture_support() -> ScreenCaptureSupport {
    ScreenCaptureSupport {
        supported: true,
        reason: None,
        backend: ScreenCaptureBackend::MacosScreenCaptureKit,
    }
}

pub async fn start_session(
    config: ScreenCaptureConfig,
) -> Result<PlatformScreenCaptureSession, ScreenCaptureError> {
    let mut picker_config = SCContentSharingPickerConfiguration::default_from_system();
    picker_config.set_allowed_picker_modes(&[
        SCContentSharingPickerMode::SingleDisplay,
        SCContentSharingPickerMode::SingleWindow,
        SCContentSharingPickerMode::SingleApplication,
    ]);
    picker_config.set_allows_changing_selected_content(false);

    let outcome = AsyncSCContentSharingPicker::show(&picker_config).await;
    let picked = match outcome {
        SCPickerOutcome::Picked(result) => result,
        SCPickerOutcome::Cancelled => return Err(ScreenCaptureError::Cancelled),
        SCPickerOutcome::Error(error) => {
            return Err(ScreenCaptureError::PermissionOrSourceUnavailable(error))
        }
    };

    let (target_width, target_height) = config.profile.dimensions();
    let frame_interval = CMTime {
        value: 1,
        timescale: config.profile.fps() as i32,
        flags: 0,
        epoch: 0,
    };
    let stream_config = SCStreamConfiguration::new()
        .with_width(target_width)
        .with_height(target_height)
        .with_pixel_format(PixelFormat::BGRA)
        .with_shows_cursor(config.cursor_mode == ScreenCaptureCursorMode::Embedded)
        .with_minimum_frame_interval(&frame_interval);

    let stream = AsyncSCStream::new(
        &picked.filter(),
        &stream_config,
        3,
        SCStreamOutputType::Screen,
    );
    stream
        .start_capture()
        .await
        .map_err(|e| ScreenCaptureError::Backend(e.to_string()))?;

    let (picked_width, picked_height) = picked.pixel_size();
    let info = ScreenCaptureSessionInfo {
        backend: ScreenCaptureBackend::MacosScreenCaptureKit,
        source_label: format!("macOS picker source {}x{}", picked_width, picked_height),
        requested_profile: config.profile.label().to_string(),
        format: ScreenCaptureFormatInfo {
            width: target_width,
            height: target_height,
            fps: config.profile.fps(),
            format: PixelFormat::BGRA.to_string(),
        },
    };

    Ok(PlatformScreenCaptureSession {
        info,
        stream,
        stats: CaptureStatsAtomic::default(),
        preview_slot: crate::LatestSlot::default(),
        last_preview_at: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skips_non_content_screencapturekit_samples() {
        assert!(!should_skip_sample_status(None));
        assert!(!should_skip_sample_status(Some(SCFrameStatus::Complete)));
        assert!(!should_skip_sample_status(Some(SCFrameStatus::Started)));
        assert!(should_skip_sample_status(Some(SCFrameStatus::Idle)));
        assert!(should_skip_sample_status(Some(SCFrameStatus::Blank)));
        assert!(should_skip_sample_status(Some(SCFrameStatus::Suspended)));
        assert!(should_skip_sample_status(Some(SCFrameStatus::Stopped)));
    }
}
