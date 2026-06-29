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
}
