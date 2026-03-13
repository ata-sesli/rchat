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

/// Wire-level message kind for request-response DMs.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DirectMessageKind {
    Text,
    Image,
    Sticker,
    Document,
    Video,
    Audio,
    ReadReceipt,
    FileMetadataRequest,
    FileMetadataResponse,
    ChunkRequest,
    ChunkResponse,
    InviteHandshake,
    TempHandshake,
    CallOffer,
    CallOfferVideo,
    CallAccept,
    CallAcceptVideo,
    CallReject,
    CallBusy,
    CallEnd,
    BroadcastOffer,
    BroadcastAccept,
    BroadcastReject,
    BroadcastBusy,
    BroadcastEnd,
}

impl DirectMessageKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Image => "image",
            Self::Sticker => "sticker",
            Self::Document => "document",
            Self::Video => "video",
            Self::Audio => "audio",
            Self::ReadReceipt => "read_receipt",
            Self::FileMetadataRequest => "file_metadata_request",
            Self::FileMetadataResponse => "file_metadata_response",
            Self::ChunkRequest => "chunk_request",
            Self::ChunkResponse => "chunk_response",
            Self::InviteHandshake => "invite_handshake",
            Self::TempHandshake => "temp_handshake",
            Self::CallOffer => "call_offer",
            Self::CallOfferVideo => "call_offer_video",
            Self::CallAccept => "call_accept",
            Self::CallAcceptVideo => "call_accept_video",
            Self::CallReject => "call_reject",
            Self::CallBusy => "call_busy",
            Self::CallEnd => "call_end",
            Self::BroadcastOffer => "broadcast_offer",
            Self::BroadcastAccept => "broadcast_accept",
            Self::BroadcastReject => "broadcast_reject",
            Self::BroadcastBusy => "broadcast_busy",
            Self::BroadcastEnd => "broadcast_end",
        }
    }

    pub fn needs_file_transfer(self) -> bool {
        matches!(
            self,
            Self::Image | Self::Sticker | Self::Document | Self::Video | Self::Audio
        )
    }
}

/// Direct message request - sent from sender to recipient
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DirectMessageRequest {
    /// Unique message ID
    pub id: String,
    /// Sender's peer ID
    pub sender_id: String,
    /// Message type.
    pub msg_type: DirectMessageKind,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_message_kind_serialization_is_wire_compatible() {
        let kinds = [
            (DirectMessageKind::Text, "\"text\""),
            (DirectMessageKind::Image, "\"image\""),
            (DirectMessageKind::Sticker, "\"sticker\""),
            (DirectMessageKind::Document, "\"document\""),
            (DirectMessageKind::Video, "\"video\""),
            (DirectMessageKind::Audio, "\"audio\""),
            (DirectMessageKind::ReadReceipt, "\"read_receipt\""),
            (
                DirectMessageKind::FileMetadataRequest,
                "\"file_metadata_request\"",
            ),
            (
                DirectMessageKind::FileMetadataResponse,
                "\"file_metadata_response\"",
            ),
            (DirectMessageKind::ChunkRequest, "\"chunk_request\""),
            (DirectMessageKind::ChunkResponse, "\"chunk_response\""),
            (DirectMessageKind::InviteHandshake, "\"invite_handshake\""),
            (DirectMessageKind::TempHandshake, "\"temp_handshake\""),
            (DirectMessageKind::CallOffer, "\"call_offer\""),
            (DirectMessageKind::CallOfferVideo, "\"call_offer_video\""),
            (DirectMessageKind::CallAccept, "\"call_accept\""),
            (DirectMessageKind::CallAcceptVideo, "\"call_accept_video\""),
            (DirectMessageKind::CallReject, "\"call_reject\""),
            (DirectMessageKind::CallBusy, "\"call_busy\""),
            (DirectMessageKind::CallEnd, "\"call_end\""),
            (DirectMessageKind::BroadcastOffer, "\"broadcast_offer\""),
            (DirectMessageKind::BroadcastAccept, "\"broadcast_accept\""),
            (DirectMessageKind::BroadcastReject, "\"broadcast_reject\""),
            (DirectMessageKind::BroadcastBusy, "\"broadcast_busy\""),
            (DirectMessageKind::BroadcastEnd, "\"broadcast_end\""),
        ];

        for (kind, expected_json) in kinds {
            let encoded = serde_json::to_string(&kind).expect("serialize kind");
            assert_eq!(encoded, expected_json);
        }
    }

    #[test]
    fn test_file_transfer_kinds_cover_document_and_video() {
        assert!(DirectMessageKind::Image.needs_file_transfer());
        assert!(DirectMessageKind::Sticker.needs_file_transfer());
        assert!(DirectMessageKind::Document.needs_file_transfer());
        assert!(DirectMessageKind::Video.needs_file_transfer());
        assert!(DirectMessageKind::Audio.needs_file_transfer());
        assert!(!DirectMessageKind::Text.needs_file_transfer());
    }
}
