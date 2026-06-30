use std::error::Error;
use std::ffi::CStr;
use std::fmt;
use std::ptr::NonNull;
use std::slice;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Vp8EncoderConfig {
    pub width: u32,
    pub height: u32,
    pub bitrate_kbps: u32,
    pub fps: u32,
    pub threads: u32,
    pub keyframe_interval: u32,
    pub cpu_used: i32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncodedPacket {
    pub payload: Vec<u8>,
    pub is_key: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VpxError {
    code: i32,
    message: String,
}

impl VpxError {
    fn from_status(code: i32) -> Self {
        let message = unsafe {
            let ptr = rchat_vpx_status_message(code);
            if ptr.is_null() {
                "unknown libvpx error".to_string()
            } else {
                CStr::from_ptr(ptr).to_string_lossy().into_owned()
            }
        };
        Self { code, message }
    }

    fn message(message: impl Into<String>) -> Self {
        Self {
            code: RCHAT_VPX_INVALID_ARGUMENT,
            message: message.into(),
        }
    }

    pub fn code(&self) -> i32 {
        self.code
    }
}

impl fmt::Display for VpxError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl Error for VpxError {}

pub struct Vp8Encoder {
    raw: NonNull<RchatVpxEncoder>,
    config: Vp8EncoderConfig,
}

unsafe impl Send for Vp8Encoder {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedI420Frame {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
}

pub struct Vp8Decoder {
    raw: NonNull<RchatVpxDecoder>,
}

unsafe impl Send for Vp8Decoder {}

impl Vp8Encoder {
    pub fn new(config: Vp8EncoderConfig) -> Result<Self, VpxError> {
        if expected_i420_len(config.width, config.height).is_none()
            || config.bitrate_kbps == 0
            || config.fps == 0
            || config.keyframe_interval == 0
        {
            return Err(VpxError::message("invalid VP8 encoder config"));
        }

        let mut raw = std::ptr::null_mut();
        let status = unsafe {
            rchat_vpx_encoder_new(
                config.width,
                config.height,
                config.bitrate_kbps,
                config.fps,
                config.threads,
                config.keyframe_interval,
                config.cpu_used,
                &mut raw,
            )
        };
        if status != RCHAT_VPX_OK {
            return Err(VpxError::from_status(status));
        }

        let raw =
            NonNull::new(raw).ok_or_else(|| VpxError::message("libvpx returned null encoder"))?;
        Ok(Self { raw, config })
    }

    pub fn config(&self) -> Vp8EncoderConfig {
        self.config
    }

    pub fn encode_i420(
        &mut self,
        data: &[u8],
        force_keyframe: bool,
    ) -> Result<Vec<EncodedPacket>, VpxError> {
        let expected_len = expected_i420_len(self.config.width, self.config.height)
            .ok_or_else(|| VpxError::message("invalid frame size"))?;
        if data.len() != expected_len {
            return Err(VpxError::message(format!(
                "invalid I420 frame length: expected {}, got {}",
                expected_len,
                data.len()
            )));
        }

        let mut packets = RchatVpxPacketList {
            packets: std::ptr::null_mut(),
            len: 0,
        };
        let status = unsafe {
            rchat_vpx_encoder_encode_i420(
                self.raw.as_ptr(),
                data.as_ptr(),
                data.len(),
                i32::from(force_keyframe),
                &mut packets,
            )
        };
        if status != RCHAT_VPX_OK {
            return Err(VpxError::from_status(status));
        }

        let _guard = PacketListGuard(&mut packets);
        let raw_packets = if packets.packets.is_null() || packets.len == 0 {
            &[]
        } else {
            unsafe { slice::from_raw_parts(packets.packets, packets.len) }
        };

        let mut out = Vec::with_capacity(raw_packets.len());
        for packet in raw_packets {
            let payload = if packet.data.is_null() || packet.len == 0 {
                Vec::new()
            } else {
                unsafe { slice::from_raw_parts(packet.data, packet.len).to_vec() }
            };
            out.push(EncodedPacket {
                payload,
                is_key: packet.is_key != 0,
            });
        }

        Ok(out)
    }
}

impl Vp8Decoder {
    pub fn new() -> Result<Self, VpxError> {
        let mut raw = std::ptr::null_mut();
        let status = unsafe { rchat_vpx_decoder_new(&mut raw) };
        if status != RCHAT_VPX_OK {
            return Err(VpxError::from_status(status));
        }

        let raw =
            NonNull::new(raw).ok_or_else(|| VpxError::message("libvpx returned null decoder"))?;
        Ok(Self { raw })
    }

    pub fn decode(&mut self, data: &[u8]) -> Result<DecodedI420Frame, VpxError> {
        if data.is_empty() {
            return Err(VpxError::message("empty VP8 packet"));
        }

        let mut frame = RchatVpxDecodedFrame {
            data: std::ptr::null_mut(),
            len: 0,
            width: 0,
            height: 0,
        };
        let status = unsafe {
            rchat_vpx_decoder_decode_i420(self.raw.as_ptr(), data.as_ptr(), data.len(), &mut frame)
        };
        if status != RCHAT_VPX_OK {
            return Err(VpxError::from_status(status));
        }

        let _guard = DecodedFrameGuard(&mut frame);
        let decoded = if frame.data.is_null() || frame.len == 0 {
            Vec::new()
        } else {
            unsafe { slice::from_raw_parts(frame.data, frame.len).to_vec() }
        };
        Ok(DecodedI420Frame {
            width: frame.width,
            height: frame.height,
            data: decoded,
        })
    }
}

impl Drop for Vp8Decoder {
    fn drop(&mut self) {
        unsafe {
            rchat_vpx_decoder_free(self.raw.as_ptr());
        }
    }
}

impl Drop for Vp8Encoder {
    fn drop(&mut self) {
        unsafe {
            rchat_vpx_encoder_free(self.raw.as_ptr());
        }
    }
}

struct PacketListGuard(*mut RchatVpxPacketList);

impl Drop for PacketListGuard {
    fn drop(&mut self) {
        unsafe {
            rchat_vpx_packet_list_free(self.0);
        }
    }
}

struct DecodedFrameGuard(*mut RchatVpxDecodedFrame);

impl Drop for DecodedFrameGuard {
    fn drop(&mut self) {
        unsafe {
            rchat_vpx_decoded_frame_free(self.0);
        }
    }
}

pub fn expected_i420_len(width: u32, height: u32) -> Option<usize> {
    if width == 0 || height == 0 || width % 2 != 0 || height % 2 != 0 {
        return None;
    }
    let pixels = width.checked_mul(height)? as usize;
    Some(pixels + pixels / 2)
}

pub fn probe_vp8_decode(payload: &[u8]) -> Result<(), VpxError> {
    let status = unsafe { rchat_vpx_probe_vp8_decode(payload.as_ptr(), payload.len()) };
    if status == RCHAT_VPX_OK {
        Ok(())
    } else {
        Err(VpxError::from_status(status))
    }
}

#[repr(C)]
struct RchatVpxEncoder {
    _private: [u8; 0],
}

#[repr(C)]
struct RchatVpxDecoder {
    _private: [u8; 0],
}

#[repr(C)]
struct RchatVpxPacket {
    data: *mut u8,
    len: usize,
    is_key: i32,
}

#[repr(C)]
struct RchatVpxPacketList {
    packets: *mut RchatVpxPacket,
    len: usize,
}

#[repr(C)]
struct RchatVpxDecodedFrame {
    data: *mut u8,
    len: usize,
    width: u32,
    height: u32,
}

const RCHAT_VPX_OK: i32 = 0;
const RCHAT_VPX_INVALID_ARGUMENT: i32 = 1;

extern "C" {
    fn rchat_vpx_encoder_new(
        width: u32,
        height: u32,
        bitrate_kbps: u32,
        fps: u32,
        threads: u32,
        keyframe_interval: u32,
        cpu_used: i32,
        out_encoder: *mut *mut RchatVpxEncoder,
    ) -> i32;

    fn rchat_vpx_encoder_free(encoder: *mut RchatVpxEncoder);

    fn rchat_vpx_encoder_encode_i420(
        encoder: *mut RchatVpxEncoder,
        data: *const u8,
        data_len: usize,
        force_keyframe: i32,
        out_packets: *mut RchatVpxPacketList,
    ) -> i32;

    fn rchat_vpx_packet_list_free(packets: *mut RchatVpxPacketList);

    fn rchat_vpx_decoder_new(out_decoder: *mut *mut RchatVpxDecoder) -> i32;

    fn rchat_vpx_decoder_free(decoder: *mut RchatVpxDecoder);

    fn rchat_vpx_decoder_decode_i420(
        decoder: *mut RchatVpxDecoder,
        data: *const u8,
        data_len: usize,
        out_frame: *mut RchatVpxDecodedFrame,
    ) -> i32;

    fn rchat_vpx_decoded_frame_free(frame: *mut RchatVpxDecodedFrame);

    fn rchat_vpx_probe_vp8_decode(data: *const u8, data_len: usize) -> i32;

    fn rchat_vpx_status_message(status: i32) -> *const std::os::raw::c_char;
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

    fn config(width: u32, height: u32, bitrate_kbps: u32) -> Vp8EncoderConfig {
        Vp8EncoderConfig {
            width,
            height,
            bitrate_kbps,
            fps: 30,
            threads: 4,
            keyframe_interval: 60,
            cpu_used: 8,
        }
    }

    #[test]
    fn encodes_all_target_profiles() {
        for (width, height, bitrate_kbps) in
            [(640, 360, 650), (854, 480, 1_200), (1280, 720, 2_500)]
        {
            let mut encoder =
                Vp8Encoder::new(config(width, height, bitrate_kbps)).expect("encoder starts");
            let data = synthetic_i420(width as usize, height as usize);
            let packets = encoder.encode_i420(&data, true).expect("frame encodes");

            assert!(!packets.is_empty());
            assert!(packets.iter().any(|packet| packet.is_key));
            assert!(packets.iter().all(|packet| !packet.payload.is_empty()));
        }
    }

    #[test]
    fn encoded_keyframe_decodes() {
        let mut encoder = Vp8Encoder::new(config(640, 360, 650)).expect("encoder starts");
        let data = synthetic_i420(640, 360);
        let packets = encoder.encode_i420(&data, true).expect("frame encodes");
        let packet = packets.first().expect("packet");

        probe_vp8_decode(&packet.payload).expect("packet decodes");
    }

    #[test]
    fn decoder_outputs_i420_frames() {
        let mut encoder = Vp8Encoder::new(config(640, 360, 650)).expect("encoder starts");
        let data = synthetic_i420(640, 360);
        let packets = encoder.encode_i420(&data, true).expect("frame encodes");
        let packet = packets.first().expect("packet");
        let mut decoder = Vp8Decoder::new().expect("decoder starts");

        let frame = decoder.decode(&packet.payload).expect("packet decodes");

        assert_eq!(frame.width, 640);
        assert_eq!(frame.height, 360);
        assert_eq!(frame.data.len(), expected_i420_len(640, 360).unwrap());
    }

    #[test]
    fn invalid_i420_length_returns_error() {
        let mut encoder = Vp8Encoder::new(config(640, 360, 650)).expect("encoder starts");
        let err = encoder
            .encode_i420(&[0, 1, 2, 3], true)
            .expect_err("short frame fails");

        assert!(err.to_string().contains("invalid I420 frame length"));
    }
}
