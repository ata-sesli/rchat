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
    pub chunk_type: BroadcastChunkType,
    pub payload: Vec<u8>,
}
