use rchat_libvpx::{EncodedPacket, Vp8Decoder, Vp8Encoder, Vp8EncoderConfig};
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

    pub fn dimensions(self) -> (u32, u32) {
        match self {
            Self::P360 => (640, 360),
            Self::P480 => (854, 480),
            Self::P720 => (1280, 720),
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
pub struct PreparedI420Frame {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
}

pub fn clamp_dimensions_to_profile(
    width: u32,
    height: u32,
    profile: VideoProfile,
) -> Option<(u32, u32)> {
    if width == 0 || height == 0 {
        return None;
    }
    let width = width & !1;
    let height = height & !1;
    if width < 2 || height < 2 {
        return None;
    }

    let (max_width, max_height) = profile.dimensions();
    if width <= max_width && height <= max_height {
        return Some((width, height));
    }

    let scale = (max_width as f64 / width as f64).min(max_height as f64 / height as f64);
    let out_width = ((width as f64 * scale).floor() as u32).max(2) & !1;
    let out_height = ((height as f64 * scale).floor() as u32).max(2) & !1;
    if out_width < 2 || out_height < 2 {
        return None;
    }
    Some((out_width, out_height))
}

pub fn scale_i420_nearest(
    src: &[u8],
    src_width: u32,
    src_height: u32,
    dst_width: u32,
    dst_height: u32,
) -> Result<Vec<u8>, String> {
    let src_len = rchat_libvpx::expected_i420_len(src_width, src_height)
        .ok_or_else(|| "invalid source I420 frame size".to_string())?;
    let dst_len = rchat_libvpx::expected_i420_len(dst_width, dst_height)
        .ok_or_else(|| "invalid destination I420 frame size".to_string())?;
    if src.len() != src_len {
        return Err(format!(
            "I420 scale input length mismatch: expected {}, got {}",
            src_len,
            src.len()
        ));
    }

    let sw = src_width as usize;
    let sh = src_height as usize;
    let dw = dst_width as usize;
    let dh = dst_height as usize;
    let src_y_len = sw * sh;
    let src_uv_width = sw / 2;
    let dst_y_len = dw * dh;
    let dst_uv_width = dw / 2;
    let mut out = vec![0_u8; dst_len];

    for y in 0..dh {
        let sy = y * sh / dh;
        for x in 0..dw {
            let sx = x * sw / dw;
            out[y * dw + x] = src[sy * sw + sx];
        }
    }
    for y in 0..dh / 2 {
        let sy = y * (sh / 2) / (dh / 2);
        for x in 0..dw / 2 {
            let sx = x * (sw / 2) / (dw / 2);
            out[dst_y_len + y * dst_uv_width + x] = src[src_y_len + sy * src_uv_width + sx];
            out[dst_y_len + dst_y_len / 4 + y * dst_uv_width + x] =
                src[src_y_len + src_y_len / 4 + sy * src_uv_width + sx];
        }
    }

    Ok(out)
}

pub fn prepare_i420_for_profile(
    data: &[u8],
    width: u32,
    height: u32,
    profile: VideoProfile,
) -> Result<PreparedI420Frame, String> {
    let expected_len = rchat_libvpx::expected_i420_len(width, height)
        .ok_or_else(|| "invalid I420 frame size".to_string())?;
    if data.len() != expected_len {
        return Err(format!(
            "invalid I420 frame length: expected {}, got {}",
            expected_len,
            data.len()
        ));
    }

    let (target_width, target_height) = clamp_dimensions_to_profile(width, height, profile)
        .ok_or_else(|| "invalid target I420 frame size".to_string())?;
    let prepared_data = if target_width == width && target_height == height {
        data.to_vec()
    } else {
        scale_i420_nearest(data, width, height, target_width, target_height)?
    };

    Ok(PreparedI420Frame {
        width: target_width,
        height: target_height,
        data: prepared_data,
    })
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RgbaVideoFrame {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

pub struct Vp8VideoDecoder {
    decoder: Vp8Decoder,
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

impl Vp8VideoDecoder {
    pub fn new() -> Result<Self, String> {
        Ok(Self {
            decoder: Vp8Decoder::new().map_err(|e| e.to_string())?,
        })
    }

    pub fn decode_rgba(&mut self, payload: &[u8]) -> Result<RgbaVideoFrame, String> {
        let decoded = self.decoder.decode(payload).map_err(|e| e.to_string())?;
        let rgba = i420_to_rgba(&decoded.data, decoded.width, decoded.height)?;
        Ok(RgbaVideoFrame {
            width: decoded.width,
            height: decoded.height,
            rgba,
        })
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

fn clamp_byte(value: i32) -> u8 {
    value.clamp(0, 255) as u8
}

pub fn i420_to_rgba(data: &[u8], width: u32, height: u32) -> Result<Vec<u8>, String> {
    let expected_len = rchat_libvpx::expected_i420_len(width, height)
        .ok_or_else(|| "invalid I420 frame size".to_string())?;
    if data.len() < expected_len {
        return Err(format!(
            "invalid I420 frame length: expected {}, got {}",
            expected_len,
            data.len()
        ));
    }

    let width_usize = width as usize;
    let height_usize = height as usize;
    let y_len = width_usize * height_usize;
    let uv_width = width_usize / 2;
    let u_offset = y_len;
    let v_offset = y_len + y_len / 4;
    let mut rgba = Vec::with_capacity(width_usize * height_usize * 4);

    for y in 0..height_usize {
        for x in 0..width_usize {
            let y_value = data[y * width_usize + x] as i32;
            let uv_index = (y / 2) * uv_width + x / 2;
            let u_value = data[u_offset + uv_index] as i32;
            let v_value = data[v_offset + uv_index] as i32;

            let c = y_value - 16;
            let d = u_value - 128;
            let e = v_value - 128;
            let r = (298 * c + 409 * e + 128) >> 8;
            let g = (298 * c - 100 * d - 208 * e + 128) >> 8;
            let b = (298 * c + 516 * d + 128) >> 8;
            rgba.extend_from_slice(&[clamp_byte(r), clamp_byte(g), clamp_byte(b), 255]);
        }
    }

    Ok(rgba)
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

#[derive(Debug, Clone, Copy, Default)]
pub struct VideoReceiverPreferenceWindow {
    pub seconds: f64,
    pub received_frames: u64,
    pub rendered_frames: u64,
    pub dropped_frames: u64,
    pub decode_errors: u64,
}

#[derive(Debug, Clone)]
pub struct VideoReceiverPreferenceController {
    current_profile: VideoProfile,
    stable_seconds: f64,
}

impl Default for VideoReceiverPreferenceController {
    fn default() -> Self {
        Self {
            current_profile: VideoProfile::P720,
            stable_seconds: 0.0,
        }
    }
}

impl VideoReceiverPreferenceController {
    pub fn current_profile(&self) -> VideoProfile {
        self.current_profile
    }

    pub fn evaluate_window(
        &mut self,
        window: VideoReceiverPreferenceWindow,
    ) -> Option<VideoQualityChangeDecision> {
        if window.received_frames == 0 || window.seconds <= 0.0 {
            self.stable_seconds = 0.0;
            return None;
        }

        let missing_rendered = window
            .received_frames
            .saturating_sub(window.rendered_frames);
        let explicit_pressure = window.dropped_frames.saturating_add(window.decode_errors);
        let pressure =
            missing_rendered.max(explicit_pressure) as f64 / window.received_frames as f64;
        let rendered_fps = window.rendered_frames as f64 / window.seconds;
        let should_downshift = pressure > 0.05
            || (window.rendered_frames > 0 && rendered_fps < 24.0)
            || (explicit_pressure as f64 / window.received_frames as f64) > 0.05;

        if should_downshift {
            self.stable_seconds = 0.0;
            let next = self.current_profile.downshift();
            if next != self.current_profile {
                self.current_profile = next;
                return Some(VideoQualityChangeDecision {
                    profile: next,
                    reason: "receiver_request".to_string(),
                });
            }
            return None;
        }

        let stable = pressure < 0.01
            && rendered_fps >= 28.0
            && window.dropped_frames == 0
            && window.decode_errors == 0;
        if stable {
            self.stable_seconds += window.seconds;
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
                    reason: "receiver_request".to_string(),
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
    fn video_profile_dimensions_match_labels() {
        assert_eq!(VideoProfile::P360.dimensions(), (640, 360));
        assert_eq!(VideoProfile::P480.dimensions(), (854, 480));
        assert_eq!(VideoProfile::P720.dimensions(), (1280, 720));
    }

    #[test]
    fn clamp_dimensions_to_profile_preserves_aspect_ratio() {
        assert_eq!(
            clamp_dimensions_to_profile(1280, 720, VideoProfile::P360),
            Some((640, 360))
        );
        assert_eq!(
            clamp_dimensions_to_profile(640, 480, VideoProfile::P360),
            Some((480, 360))
        );
    }

    #[test]
    fn scale_i420_nearest_downscales_even_frame() {
        let y: Vec<u8> = (0..16).collect();
        let u = vec![100, 101, 102, 103];
        let v = vec![200, 201, 202, 203];
        let mut src = y;
        src.extend(u);
        src.extend(v);

        let out = scale_i420_nearest(&src, 4, 4, 2, 2).expect("scales frame");

        assert_eq!(out, vec![0, 2, 8, 10, 100, 200]);
    }

    #[test]
    fn prepare_i420_for_profile_downscales_capture_frame() {
        let data = synthetic_i420(1280, 720);

        let prepared =
            prepare_i420_for_profile(&data, 1280, 720, VideoProfile::P360).expect("prepares frame");

        assert_eq!(prepared.width, 640);
        assert_eq!(prepared.height, 360);
        assert_eq!(
            prepared.data.len(),
            Vp8VideoEncoder::expected_i420_len(640, 360).unwrap()
        );
    }

    #[test]
    fn receiver_preference_controller_downshifts_and_upshifts() {
        let mut controller = VideoReceiverPreferenceController::default();

        let downshift = controller.evaluate_window(VideoReceiverPreferenceWindow {
            seconds: 5.0,
            received_frames: 150,
            rendered_frames: 120,
            dropped_frames: 0,
            decode_errors: 0,
        });

        assert_eq!(
            downshift.map(|change| (change.profile, change.reason)),
            Some((VideoProfile::P480, "receiver_request".to_string()))
        );

        for _ in 0..6 {
            controller.evaluate_window(VideoReceiverPreferenceWindow {
                seconds: 5.0,
                received_frames: 150,
                rendered_frames: 150,
                dropped_frames: 0,
                decode_errors: 0,
            });
        }

        assert_eq!(controller.current_profile(), VideoProfile::P720);
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
    fn vp8_decoder_outputs_rgba_frames() {
        let profile = VideoProfile::P360;
        let (width, height) = profile_dimensions(profile);
        let mut encoder =
            Vp8VideoEncoder::new_with_dimensions(profile, width, height).expect("encoder starts");
        let data = synthetic_i420(width as usize, height as usize);
        let packets = encoder
            .encode_i420(123, width, height, &data, true)
            .expect("frame encodes");
        let packet = packets.first().expect("packet");
        let mut decoder = Vp8VideoDecoder::new().expect("decoder starts");

        let frame = decoder
            .decode_rgba(&packet.payload)
            .expect("packet decodes");

        assert_eq!(frame.width, width);
        assert_eq!(frame.height, height);
        assert_eq!(frame.rgba.len(), (width * height * 4) as usize);
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
