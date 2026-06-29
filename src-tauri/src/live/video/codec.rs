use rchat_libvpx::{EncodedPacket, Vp8Encoder, Vp8EncoderConfig};
use serde::{Deserialize, Serialize};

pub const VIDEO_FPS: u32 = 30;
pub const VIDEO_KEYFRAME_INTERVAL_FRAMES: u32 = 60;
pub const VIDEO_ENCODER_THREADS: u32 = 4;
pub const VIDEO_ENCODER_CPU_USED: i32 = 8;

pub fn should_force_video_keyframe(
    force_next_keyframe: bool,
    needs_encoder: bool,
    next_seq: u32,
) -> bool {
    force_next_keyframe
        || needs_encoder
        || next_seq == 0
        || next_seq % VIDEO_KEYFRAME_INTERVAL_FRAMES == 0
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum VideoProfile {
    #[serde(rename = "360p30")]
    P360,
    #[serde(rename = "480p30")]
    P480,
    #[serde(rename = "720p30")]
    P720,
}

impl VideoProfile {
    pub fn bitrate_kbps(self) -> u32 {
        match self {
            Self::P360 => 650,
            Self::P480 => 1_200,
            Self::P720 => 2_500,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::P360 => "360p30",
            Self::P480 => "480p30",
            Self::P720 => "720p30",
        }
    }

    pub fn downshift(self) -> Self {
        match self {
            Self::P720 => Self::P480,
            Self::P480 => Self::P360,
            Self::P360 => Self::P360,
        }
    }

    pub fn upshift(self) -> Self {
        match self {
            Self::P360 => Self::P480,
            Self::P480 => Self::P720,
            Self::P720 => Self::P720,
        }
    }
}

impl Default for VideoProfile {
    fn default() -> Self {
        Self::P720
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum VideoQualityMode {
    #[serde(rename = "auto")]
    Auto,
    #[serde(rename = "360p30")]
    P360,
    #[serde(rename = "480p30")]
    P480,
    #[serde(rename = "720p30")]
    P720,
}

impl VideoQualityMode {
    pub fn selected_profile(self) -> VideoProfile {
        match self {
            Self::Auto => VideoProfile::P720,
            Self::P360 => VideoProfile::P360,
            Self::P480 => VideoProfile::P480,
            Self::P720 => VideoProfile::P720,
        }
    }

    pub fn is_auto(self) -> bool {
        self == Self::Auto
    }

    pub fn from_label(value: &str) -> Option<Self> {
        match value {
            "auto" => Some(Self::Auto),
            "360p30" => Some(Self::P360),
            "480p30" => Some(Self::P480),
            "720p30" => Some(Self::P720),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::P360 => "360p30",
            Self::P480 => "480p30",
            Self::P720 => "720p30",
        }
    }
}

impl Default for VideoQualityMode {
    fn default() -> Self {
        Self::Auto
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Vp8EncodedPacket {
    pub payload: Vec<u8>,
    pub is_key: bool,
}

pub struct Vp8VideoEncoder {
    profile: VideoProfile,
    width: u32,
    height: u32,
    encoder: Vp8Encoder,
}

impl Vp8VideoEncoder {
    pub fn new_with_dimensions(
        profile: VideoProfile,
        width: u32,
        height: u32,
    ) -> Result<Self, String> {
        let encoder = Vp8Encoder::new(Vp8EncoderConfig {
            width,
            height,
            bitrate_kbps: profile.bitrate_kbps(),
            fps: VIDEO_FPS,
            threads: VIDEO_ENCODER_THREADS,
            keyframe_interval: VIDEO_KEYFRAME_INTERVAL_FRAMES,
            cpu_used: VIDEO_ENCODER_CPU_USED,
        })
        .map_err(|e| e.to_string())?;

        Ok(Self {
            profile,
            width,
            height,
            encoder,
        })
    }

    pub fn profile(&self) -> VideoProfile {
        self.profile
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    pub fn expected_i420_len(width: u32, height: u32) -> Option<usize> {
        rchat_libvpx::expected_i420_len(width, height)
    }

    pub fn encode_i420(
        &mut self,
        _timestamp_us: i64,
        width: u32,
        height: u32,
        data: &[u8],
        force_keyframe: bool,
    ) -> Result<Vec<Vp8EncodedPacket>, String> {
        if width != self.width || height != self.height {
            *self = Self::new_with_dimensions(self.profile, width, height)?;
        }
        let expected_len = Self::expected_i420_len(width, height)
            .ok_or_else(|| "invalid frame size".to_string())?;
        if data.len() != expected_len {
            return Err(format!(
                "invalid I420 frame length: expected {}, got {}",
                expected_len,
                data.len()
            ));
        }

        let packets = self
            .encoder
            .encode_i420(data, force_keyframe)
            .map_err(|e| e.to_string())?;

        Ok(packets.into_iter().map(Vp8EncodedPacket::from).collect())
    }
}

impl From<EncodedPacket> for Vp8EncodedPacket {
    fn from(packet: EncodedPacket) -> Self {
        Self {
            payload: packet.payload,
            is_key: packet.is_key,
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct VideoAdaptationWindow {
    pub seconds: f64,
    pub submitted_frames: u64,
    pub encoded_frames: u64,
    pub encoded_queue_drops: u64,
    pub receiver_received_frames: u64,
    pub receiver_rendered_frames: u64,
    pub receiver_dropped_frames: u64,
    pub receiver_decode_errors: u64,
    pub encode_p95_ms: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct VideoQualityChangeDecision {
    pub profile: VideoProfile,
    pub reason: String,
}

#[derive(Debug, Clone)]
pub struct VideoQualityController {
    mode: VideoQualityMode,
    current_profile: VideoProfile,
    stable_seconds: f64,
}

impl VideoQualityController {
    pub fn new(mode: VideoQualityMode) -> Self {
        Self {
            mode,
            current_profile: mode.selected_profile(),
            stable_seconds: 0.0,
        }
    }

    pub fn mode(&self) -> VideoQualityMode {
        self.mode
    }

    pub fn current_profile(&self) -> VideoProfile {
        self.current_profile
    }

    pub fn set_mode(&mut self, mode: VideoQualityMode) -> Option<VideoQualityChangeDecision> {
        self.mode = mode;
        self.stable_seconds = 0.0;
        let next = mode.selected_profile();
        if next != self.current_profile {
            self.current_profile = next;
            return Some(VideoQualityChangeDecision {
                profile: next,
                reason: "manual_quality_selection".to_string(),
            });
        }
        None
    }

    pub fn evaluate_window(
        &mut self,
        window: VideoAdaptationWindow,
    ) -> Option<VideoQualityChangeDecision> {
        if !self.mode.is_auto() {
            return None;
        }

        let submitted_loss = if window.submitted_frames == 0 {
            0.0
        } else {
            let lost = window
                .submitted_frames
                .saturating_sub(window.encoded_frames)
                .saturating_add(window.encoded_queue_drops);
            lost as f64 / window.submitted_frames as f64
        };
        let receiver_loss = if window.receiver_received_frames == 0 {
            0.0
        } else {
            let missing_rendered = window
                .receiver_received_frames
                .saturating_sub(window.receiver_rendered_frames);
            let explicit_failures = window
                .receiver_dropped_frames
                .saturating_add(window.receiver_decode_errors);
            missing_rendered.max(explicit_failures) as f64 / window.receiver_received_frames as f64
        };
        let rendered_fps = if window.seconds > 0.0 {
            window.receiver_rendered_frames as f64 / window.seconds
        } else {
            0.0
        };
        let loss = submitted_loss.max(receiver_loss);

        let should_downshift = loss > 0.05
            || (window.receiver_rendered_frames > 0 && rendered_fps < 24.0)
            || window.encode_p95_ms > 20.0;
        if should_downshift {
            self.stable_seconds = 0.0;
            let next = self.current_profile.downshift();
            if next != self.current_profile {
                self.current_profile = next;
                return Some(VideoQualityChangeDecision {
                    profile: next,
                    reason: "loss_or_encode_pressure".to_string(),
                });
            }
            return None;
        }

        let stable = loss < 0.01
            && (window.receiver_rendered_frames == 0 || rendered_fps >= 28.0)
            && window.encode_p95_ms < 12.0;
        if stable {
            self.stable_seconds += window.seconds.max(0.0);
        } else {
            self.stable_seconds = 0.0;
        }

        if self.stable_seconds >= 30.0 {
            let next = self.current_profile.upshift();
            if next != self.current_profile {
                self.current_profile = next;
                self.stable_seconds = 0.0;
                return Some(VideoQualityChangeDecision {
                    profile: next,
                    reason: "stable_window".to_string(),
                });
            }
        }

        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn synthetic_i420(width: usize, height: usize) -> Vec<u8> {
        let y_len = width * height;
        let uv_len = y_len / 4;
        let mut data = vec![96u8; y_len + uv_len * 2];
        for y in 0..height {
            for x in 0..width {
                data[y * width + x] = ((x + y) % 255) as u8;
            }
        }
        data[y_len..].fill(128);
        data
    }

    fn profile_dimensions(profile: VideoProfile) -> (u32, u32) {
        match profile {
            VideoProfile::P360 => (640, 360),
            VideoProfile::P480 => (854, 480),
            VideoProfile::P720 => (1280, 720),
        }
    }

    #[test]
    fn vp8_encoder_encodes_all_target_profiles() {
        for profile in [VideoProfile::P360, VideoProfile::P480, VideoProfile::P720] {
            let (width, height) = profile_dimensions(profile);
            let mut encoder = Vp8VideoEncoder::new_with_dimensions(profile, width, height)
                .expect("encoder starts");
            let data = synthetic_i420(width as usize, height as usize);
            let packets = encoder
                .encode_i420(123, width, height, &data, true)
                .expect("frame encodes");

            assert!(!packets.is_empty());
            assert!(packets.iter().any(|packet| packet.is_key));
            assert!(packets.iter().all(|packet| !packet.payload.is_empty()));
        }
    }

    #[test]
    fn vp8_packet_decodes_with_libvpx_decoder() {
        let profile = VideoProfile::P360;
        let (width, height) = profile_dimensions(profile);
        let mut encoder =
            Vp8VideoEncoder::new_with_dimensions(profile, width, height).expect("encoder starts");
        let data = synthetic_i420(width as usize, height as usize);
        let packets = encoder
            .encode_i420(123, width, height, &data, true)
            .expect("frame encodes");
        let packet = packets.first().expect("packet");

        rchat_libvpx::probe_vp8_decode(&packet.payload).expect("packet decodes");
    }

    #[test]
    fn vp8_encoder_rejects_invalid_i420_length() {
        let profile = VideoProfile::P360;
        let (width, height) = profile_dimensions(profile);
        let mut encoder =
            Vp8VideoEncoder::new_with_dimensions(profile, width, height).expect("encoder starts");

        let err = encoder
            .encode_i420(123, width, height, &[0, 1, 2, 3], true)
            .expect_err("short frame fails");

        assert!(err.contains("invalid I420 frame length"));
    }

    #[test]
    fn quality_controller_downshifts_on_loss_and_upshifts_after_stable_windows() {
        let mut controller = VideoQualityController::new(VideoQualityMode::Auto);

        let downshift = controller.evaluate_window(VideoAdaptationWindow {
            seconds: 5.0,
            submitted_frames: 150,
            encoded_frames: 120,
            encoded_queue_drops: 0,
            receiver_received_frames: 150,
            receiver_rendered_frames: 140,
            receiver_dropped_frames: 0,
            receiver_decode_errors: 0,
            encode_p95_ms: 10.0,
        });

        assert_eq!(
            downshift.map(|change| change.profile),
            Some(VideoProfile::P480)
        );

        for _ in 0..6 {
            controller.evaluate_window(VideoAdaptationWindow {
                seconds: 5.0,
                submitted_frames: 150,
                encoded_frames: 150,
                encoded_queue_drops: 0,
                receiver_received_frames: 150,
                receiver_rendered_frames: 150,
                receiver_dropped_frames: 0,
                receiver_decode_errors: 0,
                encode_p95_ms: 8.0,
            });
        }

        assert_eq!(controller.current_profile(), VideoProfile::P720);
    }

    #[test]
    fn quality_controller_does_not_double_count_raw_frame_drops() {
        let mut controller = VideoQualityController::new(VideoQualityMode::Auto);

        let decision = controller.evaluate_window(VideoAdaptationWindow {
            seconds: 5.0,
            submitted_frames: 100,
            encoded_frames: 95,
            encoded_queue_drops: 0,
            receiver_received_frames: 0,
            receiver_rendered_frames: 0,
            receiver_dropped_frames: 0,
            receiver_decode_errors: 0,
            encode_p95_ms: 10.0,
        });

        assert_eq!(decision, None);
        assert_eq!(controller.current_profile(), VideoProfile::P720);
    }

    #[test]
    fn quality_controller_uses_receiver_received_frames_for_loss() {
        let mut controller = VideoQualityController::new(VideoQualityMode::Auto);

        let decision = controller.evaluate_window(VideoAdaptationWindow {
            seconds: 5.0,
            submitted_frames: 150,
            encoded_frames: 150,
            encoded_queue_drops: 0,
            receiver_received_frames: 100,
            receiver_rendered_frames: 94,
            receiver_dropped_frames: 0,
            receiver_decode_errors: 0,
            encode_p95_ms: 10.0,
        });

        assert_eq!(
            decision.map(|change| change.profile),
            Some(VideoProfile::P480)
        );
    }

    #[test]
    fn force_keyframe_after_restart_or_frame_drop() {
        assert!(should_force_video_keyframe(false, false, 0));
        assert!(should_force_video_keyframe(
            false,
            false,
            VIDEO_KEYFRAME_INTERVAL_FRAMES
        ));
        assert!(should_force_video_keyframe(false, true, 37));
        assert!(should_force_video_keyframe(true, false, 37));
        assert!(!should_force_video_keyframe(false, false, 37));
    }
}
