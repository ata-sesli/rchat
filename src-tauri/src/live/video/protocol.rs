use futures::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use serde::{Deserialize, Serialize};

use crate::live::video::codec::VideoProfile;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum VideoChunkType {
    Key,
    Delta,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VideoStreamFrame {
    pub seq: u32,
    pub timestamp_us: i64,
    pub chunk_type: VideoChunkType,
    pub profile: VideoProfile,
    pub width: u32,
    pub height: u32,
    pub payload: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct VideoReceiverReport {
    pub received_frames: u64,
    pub rendered_frames: u64,
    pub dropped_frames: u64,
    pub decode_errors: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VideoCameraState {
    pub enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VideoQualityChange {
    pub profile: VideoProfile,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VideoStreamRecord {
    Frame(VideoStreamFrame),
    ReceiverReport(VideoReceiverReport),
    CameraState(VideoCameraState),
    QualityChange(VideoQualityChange),
}

const VIDEO_STREAM_CALL_HEADER_LEN: usize = 2;
const VIDEO_STREAM_RECORD_LEN: usize = 4;
const MAX_VIDEO_STREAM_RECORD_BYTES: usize = 4 * 1024 * 1024;
const RECORD_FRAME: u8 = 1;
const RECORD_RECEIVER_REPORT: u8 = 2;
const RECORD_CAMERA_STATE: u8 = 3;
const RECORD_QUALITY_CHANGE: u8 = 4;

pub fn encode_video_stream_header(call_id: &str) -> Vec<u8> {
    let call_id_bytes = call_id.as_bytes();
    let call_id_len = call_id_bytes.len().min(u16::MAX as usize);
    let mut out = Vec::with_capacity(VIDEO_STREAM_CALL_HEADER_LEN + call_id_len);
    out.extend_from_slice(&(call_id_len as u16).to_be_bytes());
    out.extend_from_slice(&call_id_bytes[..call_id_len]);
    out
}

pub fn decode_video_stream_header(bytes: &[u8]) -> Option<String> {
    if bytes.len() < VIDEO_STREAM_CALL_HEADER_LEN {
        return None;
    }
    let call_id_len = u16::from_be_bytes(bytes[0..2].try_into().ok()?) as usize;
    let end = VIDEO_STREAM_CALL_HEADER_LEN.checked_add(call_id_len)?;
    if bytes.len() != end {
        return None;
    }
    String::from_utf8(bytes[VIDEO_STREAM_CALL_HEADER_LEN..end].to_vec()).ok()
}

pub async fn write_video_stream_header<W>(writer: &mut W, call_id: &str) -> std::io::Result<()>
where
    W: AsyncWrite + Unpin,
{
    writer.write_all(&encode_video_stream_header(call_id)).await
}

pub async fn read_video_stream_header<R>(reader: &mut R) -> std::io::Result<String>
where
    R: AsyncRead + Unpin,
{
    let mut len_buf = [0u8; VIDEO_STREAM_CALL_HEADER_LEN];
    reader.read_exact(&mut len_buf).await?;
    let call_id_len = u16::from_be_bytes(len_buf) as usize;
    let mut call_id_buf = vec![0u8; call_id_len];
    reader.read_exact(&mut call_id_buf).await?;
    let mut encoded = Vec::with_capacity(VIDEO_STREAM_CALL_HEADER_LEN + call_id_len);
    encoded.extend_from_slice(&len_buf);
    encoded.extend_from_slice(&call_id_buf);
    decode_video_stream_header(&encoded)
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidData, "invalid call id"))
}

pub fn encode_video_stream_record(record: &VideoStreamRecord) -> Vec<u8> {
    let mut out = Vec::new();
    match record {
        VideoStreamRecord::Frame(frame) => {
            out.push(RECORD_FRAME);
            out.extend_from_slice(&frame.seq.to_be_bytes());
            out.extend_from_slice(&frame.timestamp_us.to_be_bytes());
            out.push(chunk_type_to_wire(frame.chunk_type));
            out.push(profile_to_wire(frame.profile));
            out.extend_from_slice(&frame.width.to_be_bytes());
            out.extend_from_slice(&frame.height.to_be_bytes());
            out.extend_from_slice(
                &(frame.payload.len().min(u32::MAX as usize) as u32).to_be_bytes(),
            );
            out.extend_from_slice(&frame.payload[..frame.payload.len().min(u32::MAX as usize)]);
        }
        VideoStreamRecord::ReceiverReport(report) => {
            out.push(RECORD_RECEIVER_REPORT);
            out.extend_from_slice(&report.received_frames.to_be_bytes());
            out.extend_from_slice(&report.rendered_frames.to_be_bytes());
            out.extend_from_slice(&report.dropped_frames.to_be_bytes());
            out.extend_from_slice(&report.decode_errors.to_be_bytes());
        }
        VideoStreamRecord::CameraState(state) => {
            out.push(RECORD_CAMERA_STATE);
            out.push(u8::from(state.enabled));
        }
        VideoStreamRecord::QualityChange(change) => {
            out.push(RECORD_QUALITY_CHANGE);
            out.push(profile_to_wire(change.profile));
            let reason = change.reason.as_bytes();
            let len = reason.len().min(u16::MAX as usize);
            out.extend_from_slice(&(len as u16).to_be_bytes());
            out.extend_from_slice(&reason[..len]);
        }
    }
    out
}

pub fn decode_video_stream_record(bytes: &[u8]) -> Option<VideoStreamRecord> {
    let (&kind, mut rest) = bytes.split_first()?;
    match kind {
        RECORD_FRAME => {
            let seq = take_u32(&mut rest)?;
            let timestamp_us = take_i64(&mut rest)?;
            let chunk_type = wire_to_chunk_type(take_u8(&mut rest)?)?;
            let profile = wire_to_profile(take_u8(&mut rest)?)?;
            let width = take_u32(&mut rest)?;
            let height = take_u32(&mut rest)?;
            let payload_len = take_u32(&mut rest)? as usize;
            if rest.len() != payload_len {
                return None;
            }
            Some(VideoStreamRecord::Frame(VideoStreamFrame {
                seq,
                timestamp_us,
                chunk_type,
                profile,
                width,
                height,
                payload: rest.to_vec(),
            }))
        }
        RECORD_RECEIVER_REPORT => {
            let report = VideoReceiverReport {
                received_frames: take_u64(&mut rest)?,
                rendered_frames: take_u64(&mut rest)?,
                dropped_frames: take_u64(&mut rest)?,
                decode_errors: take_u64(&mut rest)?,
            };
            if !rest.is_empty() {
                return None;
            }
            Some(VideoStreamRecord::ReceiverReport(report))
        }
        RECORD_CAMERA_STATE => {
            let enabled = take_u8(&mut rest)? != 0;
            if !rest.is_empty() {
                return None;
            }
            Some(VideoStreamRecord::CameraState(VideoCameraState { enabled }))
        }
        RECORD_QUALITY_CHANGE => {
            let profile = wire_to_profile(take_u8(&mut rest)?)?;
            let reason_len = take_u16(&mut rest)? as usize;
            if rest.len() != reason_len {
                return None;
            }
            let reason = String::from_utf8(rest.to_vec()).ok()?;
            Some(VideoStreamRecord::QualityChange(VideoQualityChange {
                profile,
                reason,
            }))
        }
        _ => None,
    }
}

pub async fn write_video_stream_record<W>(
    writer: &mut W,
    record: &VideoStreamRecord,
) -> std::io::Result<()>
where
    W: AsyncWrite + Unpin,
{
    let encoded = encode_video_stream_record(record);
    let len = encoded.len().min(u32::MAX as usize) as u32;
    writer.write_all(&len.to_be_bytes()).await?;
    writer.write_all(&encoded[..len as usize]).await?;
    writer.flush().await
}

pub async fn read_video_stream_record<R>(reader: &mut R) -> std::io::Result<VideoStreamRecord>
where
    R: AsyncRead + Unpin,
{
    let mut len_buf = [0u8; VIDEO_STREAM_RECORD_LEN];
    reader.read_exact(&mut len_buf).await?;
    let len = u32::from_be_bytes(len_buf) as usize;
    if len > MAX_VIDEO_STREAM_RECORD_BYTES {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "video record too large",
        ));
    }
    let mut bytes = vec![0u8; len];
    reader.read_exact(&mut bytes).await?;
    decode_video_stream_record(&bytes)
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidData, "invalid video record"))
}

fn chunk_type_to_wire(value: VideoChunkType) -> u8 {
    match value {
        VideoChunkType::Key => 1,
        VideoChunkType::Delta => 2,
    }
}

fn wire_to_chunk_type(value: u8) -> Option<VideoChunkType> {
    match value {
        1 => Some(VideoChunkType::Key),
        2 => Some(VideoChunkType::Delta),
        _ => None,
    }
}

fn profile_to_wire(value: VideoProfile) -> u8 {
    match value {
        VideoProfile::P360 => 1,
        VideoProfile::P480 => 2,
        VideoProfile::P720 => 3,
    }
}

fn wire_to_profile(value: u8) -> Option<VideoProfile> {
    match value {
        1 => Some(VideoProfile::P360),
        2 => Some(VideoProfile::P480),
        3 => Some(VideoProfile::P720),
        _ => None,
    }
}

fn take_u8(bytes: &mut &[u8]) -> Option<u8> {
    let (&value, rest) = bytes.split_first()?;
    *bytes = rest;
    Some(value)
}

fn take_u16(bytes: &mut &[u8]) -> Option<u16> {
    if bytes.len() < 2 {
        return None;
    }
    let value = u16::from_be_bytes(bytes[..2].try_into().ok()?);
    *bytes = &bytes[2..];
    Some(value)
}

fn take_u32(bytes: &mut &[u8]) -> Option<u32> {
    if bytes.len() < 4 {
        return None;
    }
    let value = u32::from_be_bytes(bytes[..4].try_into().ok()?);
    *bytes = &bytes[4..];
    Some(value)
}

fn take_i64(bytes: &mut &[u8]) -> Option<i64> {
    if bytes.len() < 8 {
        return None;
    }
    let value = i64::from_be_bytes(bytes[..8].try_into().ok()?);
    *bytes = &bytes[8..];
    Some(value)
}

fn take_u64(bytes: &mut &[u8]) -> Option<u64> {
    if bytes.len() < 8 {
        return None;
    }
    let value = u64::from_be_bytes(bytes[..8].try_into().ok()?);
    *bytes = &bytes[8..];
    Some(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn video_stream_frame_round_trips_vp8_payload() {
        let record = VideoStreamRecord::Frame(VideoStreamFrame {
            seq: 42,
            timestamp_us: 1_234_567,
            chunk_type: VideoChunkType::Key,
            profile: crate::live::video::codec::VideoProfile::P720,
            width: 1280,
            height: 720,
            payload: vec![0, 1, 2, 3, 255],
        });

        let encoded = encode_video_stream_record(&record);
        let decoded = decode_video_stream_record(&encoded).expect("record decodes");

        assert_eq!(decoded, record);
    }

    #[test]
    fn video_quality_change_round_trips_receiver_request() {
        let record = VideoStreamRecord::QualityChange(VideoQualityChange {
            profile: crate::live::video::codec::VideoProfile::P480,
            reason: "receiver_request".to_string(),
        });

        let encoded = encode_video_stream_record(&record);
        let decoded = decode_video_stream_record(&encoded).expect("record decodes");

        assert_eq!(decoded, record);
    }

    #[test]
    fn video_stream_record_rejects_truncated_payload() {
        let record = VideoStreamRecord::Frame(VideoStreamFrame {
            seq: 7,
            timestamp_us: 99,
            chunk_type: VideoChunkType::Delta,
            profile: crate::live::video::codec::VideoProfile::P360,
            width: 640,
            height: 360,
            payload: vec![9, 8, 7, 6],
        });
        let mut encoded = encode_video_stream_record(&record);
        encoded.pop();

        assert!(decode_video_stream_record(&encoded).is_none());
    }

    #[tokio::test]
    async fn video_stream_record_rejects_oversized_length_before_payload_read() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&((MAX_VIDEO_STREAM_RECORD_BYTES as u32) + 1).to_be_bytes());
        let err = read_video_stream_record(&mut futures::io::Cursor::new(bytes))
            .await
            .expect_err("oversized record should fail");

        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
    }
}
