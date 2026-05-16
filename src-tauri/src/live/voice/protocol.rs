use futures::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use serde::{Deserialize, Serialize};

pub const VOICE_PROTOCOL: &str = "/rchat/call/audio/1.0.0";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VoiceFrameRequest {
    pub call_id: String,
    pub seq: u32,
    pub timestamp: i64,
    pub payload: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VoiceStreamFrame {
    pub seq: u32,
    pub timestamp: i64,
    pub payload: Vec<u8>,
}

const VOICE_STREAM_FRAME_HEADER_LEN: usize = 14;
const VOICE_STREAM_CALL_HEADER_LEN: usize = 2;

pub fn encode_voice_stream_frame(seq: u32, timestamp: i64, payload: &[u8]) -> Vec<u8> {
    let payload_len = payload.len().min(u16::MAX as usize);
    let mut out = Vec::with_capacity(VOICE_STREAM_FRAME_HEADER_LEN + payload_len);
    out.extend_from_slice(&seq.to_be_bytes());
    out.extend_from_slice(&timestamp.to_be_bytes());
    out.extend_from_slice(&(payload_len as u16).to_be_bytes());
    out.extend_from_slice(&payload[..payload_len]);
    out
}

pub fn decode_voice_stream_frame(bytes: &[u8]) -> Option<VoiceStreamFrame> {
    if bytes.len() < VOICE_STREAM_FRAME_HEADER_LEN {
        return None;
    }

    let seq = u32::from_be_bytes(bytes[0..4].try_into().ok()?);
    let timestamp = i64::from_be_bytes(bytes[4..12].try_into().ok()?);
    let payload_len = u16::from_be_bytes(bytes[12..14].try_into().ok()?) as usize;
    let end = VOICE_STREAM_FRAME_HEADER_LEN.checked_add(payload_len)?;
    if bytes.len() != end {
        return None;
    }

    Some(VoiceStreamFrame {
        seq,
        timestamp,
        payload: bytes[VOICE_STREAM_FRAME_HEADER_LEN..end].to_vec(),
    })
}

pub fn encode_voice_stream_header(call_id: &str) -> Vec<u8> {
    let call_id_bytes = call_id.as_bytes();
    let call_id_len = call_id_bytes.len().min(u16::MAX as usize);
    let mut out = Vec::with_capacity(VOICE_STREAM_CALL_HEADER_LEN + call_id_len);
    out.extend_from_slice(&(call_id_len as u16).to_be_bytes());
    out.extend_from_slice(&call_id_bytes[..call_id_len]);
    out
}

pub fn decode_voice_stream_header(bytes: &[u8]) -> Option<String> {
    if bytes.len() < VOICE_STREAM_CALL_HEADER_LEN {
        return None;
    }
    let call_id_len = u16::from_be_bytes(bytes[0..2].try_into().ok()?) as usize;
    let end = VOICE_STREAM_CALL_HEADER_LEN.checked_add(call_id_len)?;
    if bytes.len() != end {
        return None;
    }
    String::from_utf8(bytes[VOICE_STREAM_CALL_HEADER_LEN..end].to_vec()).ok()
}

pub async fn write_voice_stream_header<W>(writer: &mut W, call_id: &str) -> std::io::Result<()>
where
    W: AsyncWrite + Unpin,
{
    writer.write_all(&encode_voice_stream_header(call_id)).await
}

pub async fn read_voice_stream_header<R>(reader: &mut R) -> std::io::Result<String>
where
    R: AsyncRead + Unpin,
{
    let mut len_buf = [0u8; VOICE_STREAM_CALL_HEADER_LEN];
    reader.read_exact(&mut len_buf).await?;
    let call_id_len = u16::from_be_bytes(len_buf) as usize;
    let mut call_id_buf = vec![0u8; call_id_len];
    reader.read_exact(&mut call_id_buf).await?;
    let mut encoded = Vec::with_capacity(VOICE_STREAM_CALL_HEADER_LEN + call_id_len);
    encoded.extend_from_slice(&len_buf);
    encoded.extend_from_slice(&call_id_buf);
    decode_voice_stream_header(&encoded)
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidData, "invalid call id"))
}

pub async fn write_voice_stream_frame<W>(
    writer: &mut W,
    seq: u32,
    timestamp: i64,
    payload: &[u8],
) -> std::io::Result<()>
where
    W: AsyncWrite + Unpin,
{
    writer
        .write_all(&encode_voice_stream_frame(seq, timestamp, payload))
        .await?;
    writer.flush().await
}

pub async fn read_voice_stream_frame<R>(reader: &mut R) -> std::io::Result<VoiceStreamFrame>
where
    R: AsyncRead + Unpin,
{
    let mut header = [0u8; VOICE_STREAM_FRAME_HEADER_LEN];
    reader.read_exact(&mut header).await?;
    let payload_len = u16::from_be_bytes([header[12], header[13]]) as usize;
    let mut bytes = Vec::with_capacity(VOICE_STREAM_FRAME_HEADER_LEN + payload_len);
    bytes.extend_from_slice(&header);
    bytes.resize(VOICE_STREAM_FRAME_HEADER_LEN + payload_len, 0);
    reader
        .read_exact(&mut bytes[VOICE_STREAM_FRAME_HEADER_LEN..])
        .await?;
    decode_voice_stream_frame(&bytes)
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidData, "invalid voice frame"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stream_frame_round_trips_opaque_media_payload() {
        let payload = vec![1, 0, 255, 127, 0, 128];
        let encoded = encode_voice_stream_frame(42, 1234, &payload);
        let decoded = decode_voice_stream_frame(&encoded).expect("frame decodes");

        assert_eq!(decoded.seq, 42);
        assert_eq!(decoded.timestamp, 1234);
        assert_eq!(decoded.payload, payload);
    }

    #[test]
    fn stream_frame_decode_rejects_truncated_payload() {
        let mut encoded = encode_voice_stream_frame(7, 99, &[1, 2, 3, 4]);
        encoded.pop();

        assert!(decode_voice_stream_frame(&encoded).is_none());
    }

    #[test]
    fn stream_header_round_trips_call_id() {
        let encoded = encode_voice_stream_header("call-123");
        let decoded = decode_voice_stream_header(&encoded).expect("header decodes");

        assert_eq!(decoded, "call-123");
    }
}
