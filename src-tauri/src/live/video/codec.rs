use std::num::NonZero;

use serde::{Deserialize, Serialize};
use vpx_rs::enc;
use vpx_rs::enc::ctrl::EncoderControlSet;
use vpx_rs::{
    Encoder, EncoderConfig, EncoderFrameFlags, EncodingDeadline, ImageFormat, Packet, RateControl,
    Timebase, YUVImageData,
};

pub const VIDEO_FPS: u32 = 30;
pub const VIDEO_KEYFRAME_INTERVAL_FRAMES: u32 = 60;
pub const VIDEO_ENCODER_THREADS: u32 = 4;
pub const VIDEO_ENCODER_CPU_USED: i32 = 8;

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
    frame_index: i64,
    encoder: Encoder<u8>,
}

impl Vp8VideoEncoder {
    pub fn new_with_dimensions(
        profile: VideoProfile,
        width: u32,
        height: u32,
    ) -> Result<Self, String> {
        let mut config = EncoderConfig::<u8>::new(
            enc::CodecId::VP8,
            width,
            height,
            Timebase {
                num: NonZero::new(1).expect("non-zero numerator"),
                den: NonZero::new(VIDEO_FPS).expect("non-zero denominator"),
            },
            RateControl::ConstantBitRate(profile.bitrate_kbps()),
        )
        .map_err(|e| e.to_string())?;
        config.threads = VIDEO_ENCODER_THREADS;
        config.lag_in_frames = 0;
        config.rc_dropframe_thresh = 0;
        config.rc_resize_allowed = None;
        config.kf_mode = enc::KeyFrameMode::Auto {
            min_dist: VIDEO_KEYFRAME_INTERVAL_FRAMES,
            max_dist: VIDEO_KEYFRAME_INTERVAL_FRAMES,
        };

        let mut encoder = Encoder::new(config).map_err(|e| e.to_string())?;
        encoder
            .codec_control_set(EncoderControlSet::CpuUsed(VIDEO_ENCODER_CPU_USED))
            .map_err(|e| e.to_string())?;
        encoder
            .codec_control_set(EncoderControlSet::MaxIntraBitratePct(450))
            .map_err(|e| e.to_string())?;

        Ok(Self {
            profile,
            width,
            height,
            frame_index: 0,
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
        let pixels = width.checked_mul(height)? as usize;
        Some(pixels + pixels / 2)
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

        let image = YUVImageData::<u8>::from_raw_data(
            ImageFormat::I420,
            width as usize,
            height as usize,
            data,
        )
        .map_err(|e| e.to_string())?;
        let flags = if force_keyframe {
            EncoderFrameFlags::FORCE_KF
        } else {
            EncoderFrameFlags::empty()
        };
        let packets = self
            .encoder
            .encode(
                self.frame_index,
                1,
                image,
                EncodingDeadline::Realtime,
                flags,
            )
            .map_err(|e| e.to_string())?;
        self.frame_index = self.frame_index.saturating_add(1);

        let mut out = Vec::new();
        for packet in packets {
            if let Packet::CompressedFrame(frame) = packet {
                out.push(Vp8EncodedPacket {
                    payload: frame.data,
                    is_key: frame.flags.is_key,
                });
            }
        }
        Ok(out)
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
            missing_rendered.max(explicit_failures) as f64
                / window.receiver_received_frames as f64
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

        let mut decoder = vpx_rs::Decoder::new(vpx_rs::DecoderConfig::new(
            vpx_rs::dec::CodecId::VP8,
            width,
            height,
        ))
        .expect("decoder starts");
        let decoded = decoder.decode(&packet.payload).expect("packet decodes");

        assert!(decoded.into_iter().next().is_some());
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
}
