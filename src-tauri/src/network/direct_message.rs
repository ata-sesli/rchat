//! Direct Message Protocol for 1:1 chats
//! Uses libp2p request-response for reliable message delivery

use serde::{Deserialize, Serialize};

/// Direct message request - sent from sender to recipient
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DirectMessageRequest {
    /// Unique message ID
    pub id: String,
    /// Sender's peer ID
    pub sender_id: String,
    /// Message type: "text", "image", "read_receipt"
    pub msg_type: String,
    /// Text content (for text messages)
    pub text_content: Option<String>,
    /// File hash (for image messages)
    pub file_hash: Option<String>,
    /// Unix timestamp
    pub timestamp: i64,
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
