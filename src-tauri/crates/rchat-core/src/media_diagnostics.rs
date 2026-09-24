use rchat_video_capture::{
    CaptureConfig, CaptureProfile, CaptureSessionStats, VideoCaptureError, VideoCaptureSession,
};
use std::sync::atomic::{AtomicU16, AtomicU64, Ordering};
use std::sync::{mpsc, Arc};
use std::thread;

use crate::live::voice::voice::start_microphone_diagnostic_session;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaDiagnosticErrorKind {
    PermissionDenied,
    DeviceUnavailable,
    OpenFailed,
    Unsupported,
}

impl MediaDiagnosticErrorKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::PermissionDenied => "Permission denied",
            Self::DeviceUnavailable => "Device unavailable",
            Self::OpenFailed => "Open failed",
            Self::Unsupported => "Unsupported capture",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaDiagnosticError {
    pub kind: MediaDiagnosticErrorKind,
    pub message: String,
}

impl std::fmt::Display for MediaDiagnosticError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}: {}", self.kind.label(), self.message)
    }
}

impl std::error::Error for MediaDiagnosticError {}

impl MediaDiagnosticError {
    pub fn new(kind: MediaDiagnosticErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }
}

impl From<VideoCaptureError> for MediaDiagnosticError {
    fn from(error: VideoCaptureError) -> Self {
        let message = error.to_string();
        let kind = match error {
            VideoCaptureError::UnsupportedPlatform | VideoCaptureError::UnsupportedFormat(_) => {
                MediaDiagnosticErrorKind::Unsupported
            }
            VideoCaptureError::NoDevice => MediaDiagnosticErrorKind::DeviceUnavailable,
            VideoCaptureError::PermissionOrDeviceUnavailable(_) => {
                classify_media_error(&message, MediaDiagnosticErrorKind::DeviceUnavailable)
            }
            VideoCaptureError::Backend(_)
            | VideoCaptureError::Conversion(_)
            | VideoCaptureError::OpenFailed(_) => {
                classify_media_error(&message, MediaDiagnosticErrorKind::OpenFailed)
            }
        };
        Self::new(kind, message)
    }
}

pub fn classify_media_error(
    message: &str,
    fallback: MediaDiagnosticErrorKind,
) -> MediaDiagnosticErrorKind {
    let message = message.to_ascii_lowercase();
    if [
        "permission denied",
        "access denied",
        "not authorized",
        "unauthorized",
        "forbidden",
    ]
    .iter()
    .any(|needle| message.contains(needle))
    {
        MediaDiagnosticErrorKind::PermissionDenied
    } else if ["unsupported", "not supported", "unknown format"]
        .iter()
        .any(|needle| message.contains(needle))
    {
        MediaDiagnosticErrorKind::Unsupported
    } else if [
        "not found",
        "no device",
        "unavailable",
        "disconnected",
        "does not exist",
    ]
    .iter()
    .any(|needle| message.contains(needle))
    {
        MediaDiagnosticErrorKind::DeviceUnavailable
    } else {
        fallback
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CameraDiagnosticSnapshot {
    pub requested_device_id: Option<String>,
    pub resolved_device_id: Option<String>,
    pub device_name: String,
    pub backend: String,
    pub requested_profile: String,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub format: String,
    pub warning: Option<String>,
    pub stats: CaptureSessionStats,
}

pub struct CameraDiagnosticSession {
    session: VideoCaptureSession,
    requested_device_id: Option<String>,
    resolved_device_id: Option<String>,
    warning: Option<String>,
}

impl CameraDiagnosticSession {
    pub fn start(selected_device_id: Option<String>) -> Result<Self, MediaDiagnosticError> {
        let selected_device_id = selected_device_id
            .map(|id| id.trim().to_string())
            .filter(|id| !id.is_empty());
        let result = VideoCaptureSession::start_with_device_id(
            CaptureConfig::default_for_profile(CaptureProfile::P720),
            selected_device_id.clone(),
        )?;
        let resolved_device_id = result
            .selection
            .resolved_id
            .clone()
            .or_else(|| result.selection.device_index.map(|index| index.to_string()));
        Ok(Self {
            session: result.session,
            requested_device_id: selected_device_id,
            resolved_device_id,
            warning: result.selection.warning,
        })
    }

    pub fn snapshot(&self) -> CameraDiagnosticSnapshot {
        let info = self.session.info();
        CameraDiagnosticSnapshot {
            requested_device_id: self.requested_device_id.clone(),
            resolved_device_id: self.resolved_device_id.clone(),
            device_name: info.device_name.clone(),
            backend: info.backend.clone(),
            requested_profile: info.requested_profile.clone(),
            width: info.format.width,
            height: info.format.height,
            fps: info.format.fps,
            format: info.format.format.clone(),
            warning: self.warning.clone(),
            stats: self.session.stats(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MicrophoneDiagnosticSnapshot {
    pub device_name: String,
    pub backend: String,
    pub sample_rate: u32,
    pub channels: u16,
    pub sample_format: String,
    pub level_percent: u8,
    pub peak_percent: u8,
    pub captured_frames: u64,
}

pub struct MicrophoneDiagnosticSession {
    shutdown_tx: mpsc::Sender<()>,
    level_percent: Arc<AtomicU16>,
    peak_percent: Arc<AtomicU16>,
    captured_frames: Arc<AtomicU64>,
    thread_handle: Option<thread::JoinHandle<()>>,
    snapshot: MicrophoneDiagnosticSnapshot,
}

impl MicrophoneDiagnosticSession {
    pub fn start() -> Result<Self, MediaDiagnosticError> {
        let level_percent = Arc::new(AtomicU16::new(0));
        let peak_percent = Arc::new(AtomicU16::new(0));
        let captured_frames = Arc::new(AtomicU64::new(0));
        let (_, shutdown_tx, thread_handle, snapshot) = start_microphone_diagnostic_session(
            Arc::clone(&level_percent),
            Arc::clone(&peak_percent),
            Arc::clone(&captured_frames),
        )?;
        Ok(Self {
            shutdown_tx,
            level_percent,
            peak_percent,
            captured_frames,
            thread_handle: Some(thread_handle),
            snapshot,
        })
    }

    pub fn snapshot(&self) -> MicrophoneDiagnosticSnapshot {
        let mut snapshot = self.snapshot.clone();
        snapshot.level_percent = normalize_level(self.level_percent.load(Ordering::Relaxed));
        snapshot.peak_percent = normalize_level(self.peak_percent.load(Ordering::Relaxed));
        snapshot.captured_frames = self.captured_frames.load(Ordering::Relaxed);
        snapshot
    }
}

impl Drop for MicrophoneDiagnosticSession {
    fn drop(&mut self) {
        let _ = self.shutdown_tx.send(());
        if let Some(handle) = self.thread_handle.take() {
            let _ = handle.join();
        }
    }
}

pub(crate) fn normalize_level(peak: u16) -> u8 {
    ((u32::from(peak) * 100 + 16_383) / 32_768).min(100) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn media_errors_have_distinct_hardware_independent_classification() {
        assert_eq!(
            classify_media_error(
                "Access denied by the operating system",
                MediaDiagnosticErrorKind::OpenFailed
            ),
            MediaDiagnosticErrorKind::PermissionDenied
        );
        assert_eq!(
            classify_media_error(
                "Requested device was not found",
                MediaDiagnosticErrorKind::OpenFailed
            ),
            MediaDiagnosticErrorKind::DeviceUnavailable
        );
        assert_eq!(
            classify_media_error(
                "sample format is not supported",
                MediaDiagnosticErrorKind::OpenFailed
            ),
            MediaDiagnosticErrorKind::Unsupported
        );
        assert_eq!(
            classify_media_error("busy", MediaDiagnosticErrorKind::OpenFailed),
            MediaDiagnosticErrorKind::OpenFailed
        );
    }

    #[test]
    fn camera_permission_and_unavailable_errors_remain_distinct() {
        let denied = MediaDiagnosticError::from(VideoCaptureError::Backend(
            "permission denied by camera service".to_string(),
        ));
        let missing = MediaDiagnosticError::from(VideoCaptureError::NoDevice);
        assert_eq!(denied.kind, MediaDiagnosticErrorKind::PermissionDenied);
        assert_eq!(missing.kind, MediaDiagnosticErrorKind::DeviceUnavailable);
    }

    #[test]
    fn level_normalization_is_bounded_for_diagnostic_display() {
        assert_eq!(normalize_level(0), 0);
        assert_eq!(normalize_level(i16::MAX as u16), 100);
        assert_eq!(normalize_level(i16::MIN.unsigned_abs()), 100);
    }
}
