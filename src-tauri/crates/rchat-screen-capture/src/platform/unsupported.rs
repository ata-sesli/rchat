use crate::{
    I420ScreenFrame, PreviewFrame, ScreenCaptureBackend, ScreenCaptureConfig, ScreenCaptureError,
    ScreenCaptureSessionInfo, ScreenCaptureSessionStats, ScreenCaptureSupport,
};

pub struct PlatformScreenCaptureSession {
    info: ScreenCaptureSessionInfo,
}

impl PlatformScreenCaptureSession {
    pub fn info(&self) -> &ScreenCaptureSessionInfo {
        &self.info
    }

    pub fn try_recv_latest_i420(&mut self) -> Option<I420ScreenFrame> {
        None
    }

    pub fn try_recv_latest_preview(&mut self) -> Option<PreviewFrame> {
        None
    }

    pub fn stats(&self) -> ScreenCaptureSessionStats {
        ScreenCaptureSessionStats::default()
    }
}

pub async fn screen_capture_support() -> ScreenCaptureSupport {
    ScreenCaptureSupport {
        supported: false,
        reason: Some("native screen capture is supported only on macOS and Linux".to_string()),
        backend: ScreenCaptureBackend::Unsupported,
    }
}

pub async fn start_session(
    _config: ScreenCaptureConfig,
) -> Result<PlatformScreenCaptureSession, ScreenCaptureError> {
    Err(ScreenCaptureError::UnsupportedPlatform)
}
