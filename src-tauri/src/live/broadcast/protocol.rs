use futures::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BroadcastChunkType {
    Key,
    Delta,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BroadcastFrameRequest {
    pub session_id: String,
    pub seq: u32,
    pub timestamp: i64,
    pub mime: String,
    pub codec: String,
    pub profile: String,
    pub width: u32,
    pub height: u32,
    pub chunk_type: BroadcastChunkType,
    pub payload: Vec<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BroadcastFrameResponse {
    pub ok: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BroadcastFrameEvent {
    pub session_id: String,
    pub peer_id: String,
    pub seq: u32,
    pub timestamp: i64,
    pub mime: String,
    pub codec: String,
    pub profile: String,
    pub width: u32,
    pub height: u32,
    pub chunk_type: BroadcastChunkType,
    pub payload: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BroadcastStreamFrame {
    pub seq: u32,
    pub timestamp_us: i64,
    pub chunk_type: BroadcastChunkType,
    pub profile: rchat_screen_capture::ScreenCaptureProfile,
    pub width: u32,
    pub height: u32,
    pub payload: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BroadcastStreamRecord {
    Frame(BroadcastStreamFrame),
}

const BROADCAST_STREAM_SESSION_HEADER_LEN: usize = 2;
const BROADCAST_STREAM_RECORD_LEN: usize = 4;
pub const MAX_BROADCAST_STREAM_RECORD_BYTES: usize = 4 * 1024 * 1024;
const RECORD_FRAME: u8 = 1;

pub fn encode_broadcast_stream_header(session_id: &str) -> Vec<u8> {
    let session_id_bytes = session_id.as_bytes();
    let session_id_len = session_id_bytes.len().min(u16::MAX as usize);
    let mut out = Vec::with_capacity(BROADCAST_STREAM_SESSION_HEADER_LEN + session_id_len);
    out.extend_from_slice(&(session_id_len as u16).to_be_bytes());
    out.extend_from_slice(&session_id_bytes[..session_id_len]);
    out
}

pub fn decode_broadcast_stream_header(bytes: &[u8]) -> Option<String> {
    if bytes.len() < BROADCAST_STREAM_SESSION_HEADER_LEN {
        return None;
    }
    let session_id_len = u16::from_be_bytes(bytes[0..2].try_into().ok()?) as usize;
    let end = BROADCAST_STREAM_SESSION_HEADER_LEN.checked_add(session_id_len)?;
    if bytes.len() != end {
        return None;
    }
    String::from_utf8(bytes[BROADCAST_STREAM_SESSION_HEADER_LEN..end].to_vec()).ok()
}

pub async fn write_broadcast_stream_header<W>(
    writer: &mut W,
    session_id: &str,
) -> std::io::Result<()>
where
    W: AsyncWrite + Unpin,
{
    writer
        .write_all(&encode_broadcast_stream_header(session_id))
        .await
}

pub async fn read_broadcast_stream_header<R>(reader: &mut R) -> std::io::Result<String>
where
    R: AsyncRead + Unpin,
{
    let mut len_buf = [0u8; BROADCAST_STREAM_SESSION_HEADER_LEN];
    reader.read_exact(&mut len_buf).await?;
    let session_id_len = u16::from_be_bytes(len_buf) as usize;
    let mut session_id_buf = vec![0u8; session_id_len];
    reader.read_exact(&mut session_id_buf).await?;
    let mut encoded = Vec::with_capacity(BROADCAST_STREAM_SESSION_HEADER_LEN + session_id_len);
    encoded.extend_from_slice(&len_buf);
    encoded.extend_from_slice(&session_id_buf);
    decode_broadcast_stream_header(&encoded)
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidData, "invalid session id"))
}

pub fn encode_broadcast_stream_record(record: &BroadcastStreamRecord) -> Vec<u8> {
    let mut out = Vec::new();
    match record {
        BroadcastStreamRecord::Frame(frame) => {
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
    }
    out
}

pub fn decode_broadcast_stream_record(bytes: &[u8]) -> Option<BroadcastStreamRecord> {
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
            Some(BroadcastStreamRecord::Frame(BroadcastStreamFrame {
                seq,
                timestamp_us,
                chunk_type,
                profile,
                width,
                height,
                payload: rest.to_vec(),
            }))
        }
        _ => None,
    }
}

pub async fn write_broadcast_stream_record<W>(
    writer: &mut W,
    record: &BroadcastStreamRecord,
) -> std::io::Result<()>
where
    W: AsyncWrite + Unpin,
{
    let encoded = encode_broadcast_stream_record(record);
    let len = encoded.len().min(u32::MAX as usize) as u32;
    writer.write_all(&len.to_be_bytes()).await?;
    writer.write_all(&encoded[..len as usize]).await?;
    writer.flush().await
}

pub async fn read_broadcast_stream_record<R>(
    reader: &mut R,
) -> std::io::Result<BroadcastStreamRecord>
where
    R: AsyncRead + Unpin,
{
    let mut len_buf = [0u8; BROADCAST_STREAM_RECORD_LEN];
    reader.read_exact(&mut len_buf).await?;
    let len = u32::from_be_bytes(len_buf) as usize;
    if len > MAX_BROADCAST_STREAM_RECORD_BYTES {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "broadcast record too large",
        ));
    }
    let mut bytes = vec![0u8; len];
    reader.read_exact(&mut bytes).await?;
    decode_broadcast_stream_record(&bytes).ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidData, "invalid broadcast record")
    })
}

fn chunk_type_to_wire(value: BroadcastChunkType) -> u8 {
    match value {
        BroadcastChunkType::Key => 1,
        BroadcastChunkType::Delta => 2,
    }
}

fn wire_to_chunk_type(value: u8) -> Option<BroadcastChunkType> {
    match value {
        1 => Some(BroadcastChunkType::Key),
        2 => Some(BroadcastChunkType::Delta),
        _ => None,
    }
}

fn profile_to_wire(value: rchat_screen_capture::ScreenCaptureProfile) -> u8 {
    match value {
        rchat_screen_capture::ScreenCaptureProfile::P480F15 => 1,
        rchat_screen_capture::ScreenCaptureProfile::P480F30 => 2,
        rchat_screen_capture::ScreenCaptureProfile::P720F15 => 3,
        rchat_screen_capture::ScreenCaptureProfile::P720F30 => 4,
    }
}

fn wire_to_profile(value: u8) -> Option<rchat_screen_capture::ScreenCaptureProfile> {
    match value {
        1 => Some(rchat_screen_capture::ScreenCaptureProfile::P480F15),
        2 => Some(rchat_screen_capture::ScreenCaptureProfile::P480F30),
        3 => Some(rchat_screen_capture::ScreenCaptureProfile::P720F15),
        4 => Some(rchat_screen_capture::ScreenCaptureProfile::P720F30),
        _ => None,
    }
}

fn take_u8(bytes: &mut &[u8]) -> Option<u8> {
    let (&value, rest) = bytes.split_first()?;
    *bytes = rest;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn broadcast_frame_round_trips_vp8_metadata() {
        let request = BroadcastFrameRequest {
            session_id: "broadcast-1".to_string(),
            seq: 42,
            timestamp: 1_234_567,
            mime: "video/webm;codecs=vp8".to_string(),
            codec: "vp8".to_string(),
            profile: "720p15".to_string(),
            width: 1280,
            height: 720,
            chunk_type: BroadcastChunkType::Key,
            payload: vec![1, 2, 3, 4],
        };

        let encoded = serde_json::to_vec(&request).expect("request serializes");
        let decoded: BroadcastFrameRequest =
            serde_json::from_slice(&encoded).expect("request deserializes");

        assert_eq!(decoded.session_id, request.session_id);
        assert_eq!(decoded.seq, request.seq);
        assert_eq!(decoded.timestamp, request.timestamp);
        assert_eq!(decoded.mime, request.mime);
        assert_eq!(decoded.codec, request.codec);
        assert_eq!(decoded.profile, "720p15");
        assert_eq!(decoded.width, 1280);
        assert_eq!(decoded.height, 720);
        assert_eq!(decoded.chunk_type, BroadcastChunkType::Key);
        assert_eq!(decoded.payload, request.payload);
    }

    #[test]
    fn broadcast_stream_header_round_trips_session_id() {
        let encoded = encode_broadcast_stream_header("broadcast-123");
        let decoded = decode_broadcast_stream_header(&encoded).expect("header decodes");

        assert_eq!(decoded, "broadcast-123");
    }

    #[test]
    fn broadcast_stream_frame_round_trips_vp8_payload() {
        let record = BroadcastStreamRecord::Frame(BroadcastStreamFrame {
            seq: 7,
            timestamp_us: 44_000,
            chunk_type: BroadcastChunkType::Key,
            profile: rchat_screen_capture::ScreenCaptureProfile::P720F15,
            width: 1280,
            height: 720,
            payload: vec![1, 2, 3, 4, 5],
        });

        let encoded = encode_broadcast_stream_record(&record);
        let decoded = decode_broadcast_stream_record(&encoded).expect("record decodes");

        assert_eq!(decoded, record);
    }

    #[test]
    fn broadcast_stream_record_rejects_truncated_payload() {
        let record = BroadcastStreamRecord::Frame(BroadcastStreamFrame {
            seq: 9,
            timestamp_us: 55_000,
            chunk_type: BroadcastChunkType::Delta,
            profile: rchat_screen_capture::ScreenCaptureProfile::P480F30,
            width: 854,
            height: 480,
            payload: vec![8, 7, 6, 5],
        });
        let mut encoded = encode_broadcast_stream_record(&record);
        encoded.pop();

        assert!(decode_broadcast_stream_record(&encoded).is_none());
    }

    #[tokio::test]
    async fn broadcast_stream_record_rejects_oversized_length_before_payload_read() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&((MAX_BROADCAST_STREAM_RECORD_BYTES as u32) + 1).to_be_bytes());
        let err = read_broadcast_stream_record(&mut futures::io::Cursor::new(bytes))
            .await
            .expect_err("oversized record should fail");

        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
    }
}
