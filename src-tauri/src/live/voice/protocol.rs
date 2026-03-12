use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VoiceFrameRequest {
    pub call_id: String,
    pub seq: u32,
    pub timestamp: i64,
    pub payload: Vec<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VoiceFrameResponse {
    pub ok: bool,
}
