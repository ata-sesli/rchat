use serde::{Deserialize, Serialize};

/// Message delivery status
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum MessageStatus {
    Pending,
    Delivered,
    Read,
    Failed,
}

impl MessageStatus {
    pub fn from_str(s: &str) -> Self {
        match s {
            "pending" => Self::Pending,
            "delivered" => Self::Delivered,
            "read" => Self::Read,
            "failed" => Self::Failed,
            _ => Self::Pending,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Delivered => "delivered",
            Self::Read => "read",
            Self::Failed => "failed",
        }
    }
}

/// Cached metadata for media files (stored in content_metadata JSON column)
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ContentMetadata {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_secs: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size_bytes: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub word_count: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub page_count: Option<u32>,
}

/// Message content variants
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum MessageContent {
    Text {
        text: String,
    },
    Photo {
        file_hash: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        caption: Option<String>,
        #[serde(flatten)]
        metadata: ContentMetadata,
    },
    Video {
        file_hash: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        caption: Option<String>,
        #[serde(flatten)]
        metadata: ContentMetadata,
    },
    Document {
        file_hash: String,
        file_name: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        caption: Option<String>,
        #[serde(flatten)]
        metadata: ContentMetadata,
    },
    Voice {
        file_hash: String,
        #[serde(flatten)]
        metadata: ContentMetadata,
    },
}

impl MessageContent {
    /// Get the file hash if this is a file-based message
    pub fn file_hash(&self) -> Option<&str> {
        match self {
            Self::Text { .. } => None,
            Self::Photo { file_hash, .. } => Some(file_hash),
            Self::Video { file_hash, .. } => Some(file_hash),
            Self::Document { file_hash, .. } => Some(file_hash),
            Self::Voice { file_hash, .. } => Some(file_hash),
        }
    }

    /// Get content type string for DB storage
    pub fn content_type(&self) -> &'static str {
        match self {
            Self::Text { .. } => "text",
            Self::Photo { .. } => "photo",
            Self::Video { .. } => "video",
            Self::Document { .. } => "document",
            Self::Voice { .. } => "voice",
        }
    }

    /// Get text content if this is a text message
    pub fn text_content(&self) -> Option<&str> {
        match self {
            Self::Text { text } => Some(text),
            _ => None,
        }
    }

    /// Get mutable reference to metadata for hydration
    pub fn metadata_mut(&mut self) -> Option<&mut ContentMetadata> {
        match self {
            Self::Text { .. } => None,
            Self::Photo { metadata, .. } => Some(metadata),
            Self::Video { metadata, .. } => Some(metadata),
            Self::Document { metadata, .. } => Some(metadata),
            Self::Voice { metadata, .. } => Some(metadata),
        }
    }

    /// Get metadata reference
    pub fn metadata(&self) -> Option<&ContentMetadata> {
        match self {
            Self::Text { .. } => None,
            Self::Photo { metadata, .. } => Some(metadata),
            Self::Video { metadata, .. } => Some(metadata),
            Self::Document { metadata, .. } => Some(metadata),
            Self::Voice { metadata, .. } => Some(metadata),
        }
    }
}

/// Complete message with metadata and content
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub id: String,
    pub chat_id: String,
    pub peer_id: String,
    pub timestamp: i64,
    pub status: MessageStatus,
    pub content: MessageContent,
}

impl Message {
    /// Create a new text message
    pub fn text(id: String, chat_id: String, peer_id: String, timestamp: i64, text: String) -> Self {
        Self {
            id,
            chat_id,
            peer_id,
            timestamp,
            status: MessageStatus::Pending,
            content: MessageContent::Text { text },
        }
    }

    /// Create a new photo message
    pub fn photo(
        id: String,
        chat_id: String,
        peer_id: String,
        timestamp: i64,
        file_hash: String,
        caption: Option<String>,
    ) -> Self {
        Self {
            id,
            chat_id,
            peer_id,
            timestamp,
            status: MessageStatus::Pending,
            content: MessageContent::Photo {
                file_hash,
                caption,
                metadata: ContentMetadata::default(),
            },
        }
    }

    /// Convert from DB row (flat structure) to rich Message
    pub fn from_db_row(db_msg: &crate::storage::db::Message) -> Self {
        // Parse cached content_metadata from JSON if present
        let cached_metadata: ContentMetadata = db_msg
            .content_metadata
            .as_ref()
            .and_then(|json| serde_json::from_str(json).ok())
            .unwrap_or_default();

        let content = match db_msg.content_type.as_str() {
            "text" => MessageContent::Text {
                text: db_msg.text_content.clone().unwrap_or_default(),
            },
            "photo" | "image" => MessageContent::Photo {
                file_hash: db_msg.file_hash.clone().unwrap_or_default(),
                caption: db_msg.text_content.clone(),
                metadata: cached_metadata,
            },
            "video" => MessageContent::Video {
                file_hash: db_msg.file_hash.clone().unwrap_or_default(),
                caption: db_msg.text_content.clone(),
                metadata: cached_metadata,
            },
            "document" => MessageContent::Document {
                file_hash: db_msg.file_hash.clone().unwrap_or_default(),
                file_name: db_msg.text_content.clone().unwrap_or_else(|| "file".to_string()),
                caption: None,
                metadata: cached_metadata,
            },
            "voice" => MessageContent::Voice {
                file_hash: db_msg.file_hash.clone().unwrap_or_default(),
                metadata: cached_metadata,
            },
            _ => MessageContent::Text {
                text: db_msg.text_content.clone().unwrap_or_default(),
            },
        };

        Self {
            id: db_msg.id.clone(),
            chat_id: db_msg.chat_id.clone(),
            peer_id: db_msg.peer_id.clone(),
            timestamp: db_msg.timestamp,
            status: MessageStatus::from_str(&db_msg.status),
            content,
        }
    }

    /// Convert rich Message to DB row (flat structure)
    pub fn to_db_row(&self) -> crate::storage::db::Message {
        let (content_type, text_content, file_hash) = match &self.content {
            MessageContent::Text { text } => ("text".to_string(), Some(text.clone()), None),
            MessageContent::Photo { file_hash, caption, .. } => {
                ("photo".to_string(), caption.clone(), Some(file_hash.clone()))
            }
            MessageContent::Video { file_hash, caption, .. } => {
                ("video".to_string(), caption.clone(), Some(file_hash.clone()))
            }
            MessageContent::Document { file_hash, file_name, .. } => {
                ("document".to_string(), Some(file_name.clone()), Some(file_hash.clone()))
            }
            MessageContent::Voice { file_hash, .. } => {
                ("voice".to_string(), None, Some(file_hash.clone()))
            }
        };

        // Serialize metadata to JSON for storage
        let content_metadata = self
            .content
            .metadata()
            .and_then(|m| serde_json::to_string(m).ok());

        crate::storage::db::Message {
            id: self.id.clone(),
            chat_id: self.chat_id.clone(),
            peer_id: self.peer_id.clone(),
            timestamp: self.timestamp,
            content_type,
            text_content,
            file_hash,
            status: self.status.as_str().to_string(),
            content_metadata,
            sender_alias: None, // TODO: add sender_alias field to ChatMessage
        }
    }

    /// Check if metadata needs to be computed (returns true if has file but no cached metadata)
    pub fn needs_hydration(&self) -> bool {
        match &self.content {
            MessageContent::Text { .. } => false,
            MessageContent::Photo { metadata, .. } => metadata.width.is_none(),
            MessageContent::Video { metadata, .. } => metadata.width.is_none() && metadata.duration_secs.is_none(),
            MessageContent::Document { metadata, .. } => metadata.size_bytes.is_none(),
            MessageContent::Voice { metadata, .. } => metadata.duration_secs.is_none(),
        }
    }

    /// Hydrate metadata by computing from file and caching in DB.
    /// Returns true if metadata was updated and should be cached.
    pub fn hydrate(&mut self, conn: &rusqlite::Connection) -> bool {
        // Only hydrate if needed
        if !self.needs_hydration() {
            return false;
        }

        let file_hash = match self.content.file_hash() {
            Some(h) => h.to_string(),
            None => return false,
        };

        // Load file data from chunks
        let file_data = match crate::storage::object::load(conn, &file_hash, None) {
            Ok(data) => data,
            Err(e) => {
                eprintln!("[Hydrate] Failed to load file {}: {}", file_hash, e);
                return false;
            }
        };

        // Compute metadata based on content type
        let updated = match &mut self.content {
            MessageContent::Photo { metadata, .. } => {
                if let Some((width, height)) = compute_image_dimensions(&file_data) {
                    metadata.width = Some(width);
                    metadata.height = Some(height);
                    metadata.size_bytes = Some(file_data.len() as i64);
                    true
                } else {
                    false
                }
            }
            MessageContent::Video { metadata, .. } => {
                // Video dimension/duration extraction would need ffprobe or similar
                // For now, just set size
                metadata.size_bytes = Some(file_data.len() as i64);
                true
            }
            MessageContent::Document { metadata, .. } => {
                metadata.size_bytes = Some(file_data.len() as i64);
                // Word count would need document parsing library
                true
            }
            MessageContent::Voice { metadata, .. } => {
                metadata.size_bytes = Some(file_data.len() as i64);
                // Duration would need audio parsing
                true
            }
            MessageContent::Text { .. } => false,
        };

        // Cache in DB if updated
        if updated {
            if let Some(metadata) = self.content.metadata() {
                if let Ok(json) = serde_json::to_string(metadata) {
                    let _ = crate::storage::db::update_content_metadata(conn, &self.id, &json);
                }
            }
        }

        updated
    }
}

/// Compute image dimensions from raw bytes
fn compute_image_dimensions(data: &[u8]) -> Option<(u32, u32)> {
    use image::io::Reader as ImageReader;
    use std::io::Cursor;

    match ImageReader::new(Cursor::new(data)).with_guessed_format() {
        Ok(reader) => match reader.into_dimensions() {
            Ok((w, h)) => Some((w, h)),
            Err(e) => {
                eprintln!("[Hydrate] Failed to get dimensions: {}", e);
                None
            }
        },
        Err(e) => {
            eprintln!("[Hydrate] Failed to read image: {}", e);
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_message_status_roundtrip() {
        assert_eq!(MessageStatus::from_str("pending"), MessageStatus::Pending);
        assert_eq!(MessageStatus::from_str("delivered"), MessageStatus::Delivered);
        assert_eq!(MessageStatus::from_str("read"), MessageStatus::Read);
        assert_eq!(MessageStatus::Pending.as_str(), "pending");
    }

    #[test]
    fn test_message_content_serialization() {
        let content = MessageContent::Photo {
            file_hash: "abc123".to_string(),
            caption: Some("Test".to_string()),
            metadata: ContentMetadata {
                width: Some(1920),
                height: Some(1080),
                ..Default::default()
            },
        };

        let json = serde_json::to_string(&content).unwrap();
        assert!(json.contains("\"type\":\"photo\""));
        assert!(json.contains("\"width\":1920"));
    }
}