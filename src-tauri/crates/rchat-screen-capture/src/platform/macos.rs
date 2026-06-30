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
use screencapturekit::cv::{CVPixelBuffer, CVPixelBufferLockFlags};
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
        let mut latest_pixel_buffer = None;
        while let Some((sample, output_type)) = self.stream.try_next_typed() {
            let status = sample.frame_status();
            let pixel_buffer = if output_type == SCStreamOutputType::Screen
                && is_convertible_sample_status(status)
            {
                sample.image_buffer()
            } else {
                None
            };
            let decision = classify_macos_sample(output_type, status, pixel_buffer.is_some());
            record_macos_sample_stats(&self.stats, output_type, status, decision);
            if decision == MacosSampleDecision::Convert {
                latest_pixel_buffer = pixel_buffer;
            }
        }

        latest_pixel_buffer.and_then(|pixel_buffer| match self.convert_pixel_buffer(pixel_buffer) {
            Ok(frame) => Some(frame),
            Err(error) => {
                self.stats.conversion_errors.fetch_add(1, Ordering::Relaxed);
                eprintln!("[Screen][Capture] macOS frame conversion failed: {}", error);
                None
            }
        })
    }

    pub fn try_recv_latest_preview(&mut self) -> Option<PreviewFrame> {
        self.preview_slot.take()
    }

    pub fn stats(&self) -> ScreenCaptureSessionStats {
        self.stats.snapshot(0, self.preview_slot.dropped())
    }

    fn convert_pixel_buffer(
        &mut self,
        pixel_buffer: CVPixelBuffer,
    ) -> Result<I420ScreenFrame, ScreenCaptureError> {
        let guard = pixel_buffer
            .lock(CVPixelBufferLockFlags::READ_ONLY)
            .map_err(|e| {
                ScreenCaptureError::Conversion(format!("pixel buffer lock failed: {e:?}"))
            })?;
        let width = guard.width() as u32;
        let height = guard.height() as u32;
        let stride = guard.bytes_per_row();
        let (out_width, out_height) = clamp_to_profile(width, height, self.info_profile());
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

        Ok(frame)
    }

    fn info_profile(&self) -> crate::ScreenCaptureProfile {
        crate::ScreenCaptureProfile::from_label(&self.info.requested_profile)
            .unwrap_or_default()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MacosSampleDecision {
    Convert,
    Skip(MacosSampleSkipReason),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MacosSampleSkipReason {
    NonScreen,
    Started,
    Idle,
    Blank,
    Suspended,
    Stopped,
    UnknownStatus,
    NoImageBuffer,
}

#[cfg(test)]
#[derive(Debug, Clone, Copy)]
struct MacosSampleDescriptor {
    output_type: SCStreamOutputType,
    status: Option<SCFrameStatus>,
    has_image_buffer: bool,
}

fn is_convertible_sample_status(status: Option<SCFrameStatus>) -> bool {
    matches!(status, Some(SCFrameStatus::Complete))
}

#[cfg(test)]
fn should_skip_sample_status(status: Option<SCFrameStatus>) -> bool {
    !is_convertible_sample_status(status)
}

fn classify_macos_sample(
    output_type: SCStreamOutputType,
    status: Option<SCFrameStatus>,
    has_image_buffer: bool,
) -> MacosSampleDecision {
    if output_type != SCStreamOutputType::Screen {
        return MacosSampleDecision::Skip(MacosSampleSkipReason::NonScreen);
    }

    match status {
        Some(SCFrameStatus::Complete) if has_image_buffer => MacosSampleDecision::Convert,
        Some(SCFrameStatus::Complete) => {
            MacosSampleDecision::Skip(MacosSampleSkipReason::NoImageBuffer)
        }
        Some(SCFrameStatus::Started) => MacosSampleDecision::Skip(MacosSampleSkipReason::Started),
        Some(SCFrameStatus::Idle) => MacosSampleDecision::Skip(MacosSampleSkipReason::Idle),
        Some(SCFrameStatus::Blank) => MacosSampleDecision::Skip(MacosSampleSkipReason::Blank),
        Some(SCFrameStatus::Suspended) => {
            MacosSampleDecision::Skip(MacosSampleSkipReason::Suspended)
        }
        Some(SCFrameStatus::Stopped) => MacosSampleDecision::Skip(MacosSampleSkipReason::Stopped),
        None => MacosSampleDecision::Skip(MacosSampleSkipReason::UnknownStatus),
    }
}

fn record_macos_sample_stats(
    stats: &CaptureStatsAtomic,
    output_type: SCStreamOutputType,
    status: Option<SCFrameStatus>,
    decision: MacosSampleDecision,
) {
    stats.raw_samples.fetch_add(1, Ordering::Relaxed);

    if output_type != SCStreamOutputType::Screen {
        stats.non_screen_samples.fetch_add(1, Ordering::Relaxed);
        return;
    }

    stats.screen_samples.fetch_add(1, Ordering::Relaxed);
    match status {
        Some(SCFrameStatus::Complete) => {
            stats.complete_samples.fetch_add(1, Ordering::Relaxed);
        }
        Some(SCFrameStatus::Started) => {
            stats.started_samples.fetch_add(1, Ordering::Relaxed);
        }
        Some(SCFrameStatus::Idle) => {
            stats.idle_samples.fetch_add(1, Ordering::Relaxed);
        }
        Some(SCFrameStatus::Blank) => {
            stats.blank_samples.fetch_add(1, Ordering::Relaxed);
        }
        Some(SCFrameStatus::Suspended) => {
            stats.suspended_samples.fetch_add(1, Ordering::Relaxed);
        }
        Some(SCFrameStatus::Stopped) => {
            stats.stopped_samples.fetch_add(1, Ordering::Relaxed);
        }
        None => {
            stats
                .unknown_status_samples
                .fetch_add(1, Ordering::Relaxed);
        }
    }

    if decision == MacosSampleDecision::Skip(MacosSampleSkipReason::NoImageBuffer) {
        stats
            .no_image_buffer_samples
            .fetch_add(1, Ordering::Relaxed);
    }
}

#[cfg(test)]
fn latest_convertible_sample_index(samples: &[MacosSampleDescriptor]) -> Option<usize> {
    samples
        .iter()
        .enumerate()
        .filter_map(|(index, sample)| {
            (classify_macos_sample(
                sample.output_type,
                sample.status,
                sample.has_image_buffer,
            ) == MacosSampleDecision::Convert)
                .then_some(index)
        })
        .last()
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
    fn only_complete_screencapturekit_samples_are_convertible() {
        assert!(is_convertible_sample_status(Some(SCFrameStatus::Complete)));
        assert!(!is_convertible_sample_status(None));
        assert!(!is_convertible_sample_status(Some(SCFrameStatus::Started)));
        assert!(!is_convertible_sample_status(Some(SCFrameStatus::Idle)));
        assert!(!is_convertible_sample_status(Some(SCFrameStatus::Blank)));
        assert!(!is_convertible_sample_status(Some(SCFrameStatus::Suspended)));
        assert!(!is_convertible_sample_status(Some(SCFrameStatus::Stopped)));
    }

    #[test]
    fn classifies_complete_screen_sample_with_image_as_convertible() {
        assert_eq!(
            classify_macos_sample(
                SCStreamOutputType::Screen,
                Some(SCFrameStatus::Complete),
                true,
            ),
            MacosSampleDecision::Convert
        );
    }

    #[test]
    fn classifies_started_screen_sample_as_skipped() {
        assert_eq!(
            classify_macos_sample(
                SCStreamOutputType::Screen,
                Some(SCFrameStatus::Started),
                true,
            ),
            MacosSampleDecision::Skip(MacosSampleSkipReason::Started)
        );
    }

    #[test]
    fn classifies_non_complete_statuses_as_skipped() {
        assert_eq!(
            classify_macos_sample(SCStreamOutputType::Screen, Some(SCFrameStatus::Idle), true),
            MacosSampleDecision::Skip(MacosSampleSkipReason::Idle)
        );
        assert_eq!(
            classify_macos_sample(SCStreamOutputType::Screen, Some(SCFrameStatus::Blank), true),
            MacosSampleDecision::Skip(MacosSampleSkipReason::Blank)
        );
        assert_eq!(
            classify_macos_sample(
                SCStreamOutputType::Screen,
                Some(SCFrameStatus::Suspended),
                true,
            ),
            MacosSampleDecision::Skip(MacosSampleSkipReason::Suspended)
        );
        assert_eq!(
            classify_macos_sample(
                SCStreamOutputType::Screen,
                Some(SCFrameStatus::Stopped),
                true,
            ),
            MacosSampleDecision::Skip(MacosSampleSkipReason::Stopped)
        );
        assert_eq!(
            classify_macos_sample(SCStreamOutputType::Screen, None, true),
            MacosSampleDecision::Skip(MacosSampleSkipReason::UnknownStatus)
        );
    }

    #[test]
    fn classifies_complete_screen_sample_without_image_as_skipped() {
        assert_eq!(
            classify_macos_sample(
                SCStreamOutputType::Screen,
                Some(SCFrameStatus::Complete),
                false,
            ),
            MacosSampleDecision::Skip(MacosSampleSkipReason::NoImageBuffer)
        );
    }

    #[test]
    fn classifies_non_screen_samples_as_skipped() {
        assert_eq!(
            classify_macos_sample(SCStreamOutputType::Audio, Some(SCFrameStatus::Complete), true),
            MacosSampleDecision::Skip(MacosSampleSkipReason::NonScreen)
        );
        assert_eq!(
            classify_macos_sample(
                SCStreamOutputType::Microphone,
                Some(SCFrameStatus::Complete),
                true,
            ),
            MacosSampleDecision::Skip(MacosSampleSkipReason::NonScreen)
        );
    }

    #[test]
    fn selects_latest_convertible_macos_sample() {
        let samples = [
            MacosSampleDescriptor {
                output_type: SCStreamOutputType::Screen,
                status: Some(SCFrameStatus::Complete),
                has_image_buffer: true,
            },
            MacosSampleDescriptor {
                output_type: SCStreamOutputType::Screen,
                status: Some(SCFrameStatus::Started),
                has_image_buffer: true,
            },
            MacosSampleDescriptor {
                output_type: SCStreamOutputType::Audio,
                status: Some(SCFrameStatus::Complete),
                has_image_buffer: true,
            },
            MacosSampleDescriptor {
                output_type: SCStreamOutputType::Screen,
                status: Some(SCFrameStatus::Complete),
                has_image_buffer: false,
            },
            MacosSampleDescriptor {
                output_type: SCStreamOutputType::Screen,
                status: Some(SCFrameStatus::Complete),
                has_image_buffer: true,
            },
        ];

        assert_eq!(latest_convertible_sample_index(&samples), Some(4));
    }

    #[test]
    fn skips_non_content_screencapturekit_samples() {
        assert!(should_skip_sample_status(Some(SCFrameStatus::Idle)));
        assert!(should_skip_sample_status(Some(SCFrameStatus::Blank)));
        assert!(should_skip_sample_status(Some(SCFrameStatus::Suspended)));
        assert!(should_skip_sample_status(Some(SCFrameStatus::Stopped)));
        assert!(should_skip_sample_status(Some(SCFrameStatus::Started)));
        assert!(should_skip_sample_status(None));
        assert!(!should_skip_sample_status(Some(SCFrameStatus::Complete)));
    }
}
