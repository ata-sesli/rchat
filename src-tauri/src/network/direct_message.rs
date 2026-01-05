//! Direct Message Protocol for 1:1 chats
//! Uses libp2p request-response for reliable message delivery

use serde::{Deserialize, Serialize};

/// Chunk metadata for file transfer
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChunkInfo {
    pub chunk_hash: String,
    pub chunk_order: i64,
    pub chunk_size: i64,
}

/// Direct message request - sent from sender to recipient
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DirectMessageRequest {
    /// Unique message ID
    pub id: String,
    /// Sender's peer ID
    pub sender_id: String,
    /// Message type: "text", "image", "read_receipt", 
    /// "file_metadata_request", "file_metadata_response",
    /// "chunk_request", "chunk_response"
    pub msg_type: String,
    /// Text content (for text messages)
    pub text_content: Option<String>,
    /// File hash (for image messages and file transfer)
    pub file_hash: Option<String>,
    /// Unix timestamp
    pub timestamp: i64,
    
    // === Chunk Transfer Fields ===
    /// Chunk hash (for chunk_request)
    pub chunk_hash: Option<String>,
    /// Base64-encoded chunk data (for chunk_response)
    pub chunk_data: Option<String>,
    /// List of chunks (for file_metadata_response)
    pub chunk_list: Option<Vec<ChunkInfo>>,
    /// Sender's display name/alias
    #[serde(default)]
    pub sender_alias: Option<String>,
}

/// Direct message response - sent back to sender
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DirectMessageResponse {
    /// Original message ID
    pub msg_id: String,
    /// Status: "delivered", "error"
    pub status: String,
    /// Error message if status is "error"
    pub error: Option<String>,
}
