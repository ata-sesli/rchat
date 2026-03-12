use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum VideoChunkType {
    Key,
    Delta,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VideoFrameRequest {
    pub call_id: String,
    pub seq: u32,
    pub timestamp: i64,
    pub mime: String,
    pub codec: String,
    pub chunk_type: VideoChunkType,
    pub payload: Vec<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VideoFrameResponse {
    pub ok: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VideoFrameEvent {
    pub call_id: String,
    pub peer_id: String,
    pub seq: u32,
    pub timestamp: i64,
    pub mime: String,
    pub codec: String,
    pub chunk_type: VideoChunkType,
    pub payload: Vec<u8>,
}
