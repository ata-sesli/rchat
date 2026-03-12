use libp2p::gossipsub::IdentTopic;
use serde::{Deserialize, Serialize};

use crate::chat_kind;

pub const CONTROL_TOPIC: &str = "rchat:control";
pub const GROUP_TOPIC_PREFIX: &str = "rchat:group:";
pub const TEMP_GROUP_TOPIC_PREFIX: &str = "rchat:temp-group:";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ControlEnvelope {
    ConnectionRequest {
        from_peer_id: String,
        to_peer_id: String,
    },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum GroupContentType {
    Text,
    Image,
    Sticker,
    Document,
    Video,
    Audio,
}

impl GroupContentType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Image => "image",
            Self::Sticker => "sticker",
            Self::Document => "document",
            Self::Video => "video",
            Self::Audio => "audio",
        }
    }

    pub fn needs_file_transfer(self) -> bool {
        !matches!(self, Self::Text)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroupMessageEnvelope {
    pub id: String,
    pub group_id: String,
    pub sender_id: String,
    #[serde(default)]
    pub sender_alias: Option<String>,
    pub timestamp: i64,
    pub content_type: GroupContentType,
    #[serde(default)]
    pub text_content: Option<String>,
    #[serde(default)]
    pub file_hash: Option<String>,
}

pub fn control_topic() -> IdentTopic {
    IdentTopic::new(CONTROL_TOPIC)
}

pub fn topic_for_group_id(group_id: &str) -> Option<IdentTopic> {
    if let Some(uuid) = chat_kind::group_uuid_from_chat_id(group_id) {
        return Some(IdentTopic::new(format!("{}{}", GROUP_TOPIC_PREFIX, uuid)));
    }
    if let Some(uuid) = chat_kind::temp_group_uuid_from_chat_id(group_id) {
        return Some(IdentTopic::new(format!(
            "{}{}",
            TEMP_GROUP_TOPIC_PREFIX, uuid
        )));
    }
    None
}

pub fn group_id_from_topic(topic: &str) -> Option<String> {
    if let Some(uuid) = topic.strip_prefix(GROUP_TOPIC_PREFIX) {
        let candidate = format!("group:{}", uuid);
        if chat_kind::is_group_chat_id(&candidate) {
            return Some(candidate);
        }
    }
    if let Some(uuid) = topic.strip_prefix(TEMP_GROUP_TOPIC_PREFIX) {
        let candidate = format!("temp-group:{}", uuid);
        if chat_kind::is_temp_group_chat_id(&candidate) {
            return Some(candidate);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_group_id_to_topic_and_back() {
        let group_id = "group:550e8400-e29b-41d4-a716-446655440000";
        assert!(topic_for_group_id(group_id).is_some());
        let recovered = group_id_from_topic("rchat:group:550e8400-e29b-41d4-a716-446655440000")
            .expect("recover id");
        assert_eq!(recovered, group_id);
    }

    #[test]
    fn rejects_invalid_group_id_for_topic() {
        assert!(topic_for_group_id("group:not-a-uuid").is_none());
    }
}
