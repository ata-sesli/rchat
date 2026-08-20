use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use libp2p::{gossipsub::IdentTopic, identity, PeerId};
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

/// Payload carried by `TempHandshake` direct messages.
///
/// Besides identifying the temporary chat, it carries the sender's per-target
/// membership winners (adds and remove tombstones) — the complete,
/// authoritative member-set state, bounded by the member cap. Joining peers
/// merge winners last-writer-wins per target, so rosters converge instead of
/// union-only merging resurrecting removed members. Older peers that only
/// send a bare chat id still parse (winners defaults empty).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TemporaryHandshakePayload {
    pub chat_id: String,
    #[serde(default)]
    pub winners: Vec<crate::app_state::TemporaryMembershipOp>,
    /// Admission certificates (one per target) backing the winner snapshot.
    /// Transferred so a fresh peer can still prove an endorsed member whose
    /// latest winner is a self-add re-announce.
    #[serde(default)]
    pub evidence: Vec<crate::app_state::TemporaryMembershipOp>,
    /// The sender's invitation capability for this group, if any. A receiver
    /// validates it against its own chat id before admitting a non-member's
    /// self-add, so joining requires the invite rather than mere dialing.
    #[serde(default)]
    pub invite: Option<crate::app_state::TemporaryInvitePayload>,
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
    #[serde(default)]
    pub protocol_version: Option<u16>,
    #[serde(default)]
    pub signed_record_id: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum GroupReceiptStatus {
    Delivered,
    Read,
}

impl GroupReceiptStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Delivered => "delivered",
            Self::Read => "read",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GroupSettings {
    #[serde(default)]
    pub members_can_invite: bool,
}

impl Default for GroupSettings {
    fn default() -> Self {
        Self {
            members_can_invite: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "record_type", rename_all = "snake_case")]
pub enum GroupRecordBody {
    GroupCreated {
        name: String,
        #[serde(default)]
        settings: Option<GroupSettings>,
    },
    MemberInvited {
        peer_id: String,
        role: String,
    },
    MemberJoined {
        peer_id: String,
    },
    MemberLeft {
        peer_id: String,
    },
    GroupRenamed {
        name: String,
    },
    GroupSettingsUpdated {
        settings: GroupSettings,
    },
    MemberRemoved {
        peer_id: String,
    },
    Message {
        content_type: GroupContentType,
        #[serde(default)]
        text_content: Option<String>,
        #[serde(default)]
        file_hash: Option<String>,
        #[serde(default)]
        sender_alias: Option<String>,
    },
    Receipt {
        message_ids: Vec<String>,
        status: GroupReceiptStatus,
    },
    Head {
        heads: Vec<String>,
    },
    FileAvailability {
        file_hash: String,
    },
}

impl GroupRecordBody {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::GroupCreated { .. } => "group_created",
            Self::MemberInvited { .. } => "member_invited",
            Self::MemberJoined { .. } => "member_joined",
            Self::MemberLeft { .. } => "member_left",
            Self::GroupRenamed { .. } => "group_renamed",
            Self::GroupSettingsUpdated { .. } => "group_settings_updated",
            Self::MemberRemoved { .. } => "member_removed",
            Self::Message { .. } => "message",
            Self::Receipt { .. } => "receipt",
            Self::Head { .. } => "head",
            Self::FileAvailability { .. } => "file_availability",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UnsignedGroupRecord {
    pub version: u16,
    pub id: String,
    pub group_id: String,
    pub author_peer_id: String,
    pub timestamp: i64,
    #[serde(default)]
    pub parents: Vec<String>,
    pub body: GroupRecordBody,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SignedGroupRecord {
    #[serde(flatten)]
    pub unsigned: UnsignedGroupRecord,
    pub public_key_b64: String,
    pub signature_b64: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GroupInvitePayload {
    pub version: u16,
    pub invite_id: String,
    pub group_id: String,
    pub group_name: String,
    pub inviter_peer_id: String,
    pub invitee_peer_id: String,
    pub created_at: i64,
    pub invite_record: SignedGroupRecord,
    #[serde(default)]
    pub related_records: Vec<SignedGroupRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GroupSyncRequest {
    pub version: u16,
    pub group_id: String,
    #[serde(default)]
    pub known_record_ids: Vec<String>,
    #[serde(default)]
    pub wanted_record_ids: Vec<String>,
    pub limit: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GroupSyncResponse {
    pub version: u16,
    pub group_id: String,
    pub records: Vec<SignedGroupRecord>,
}

impl SignedGroupRecord {
    pub const VERSION: u16 = 1;

    pub fn new(
        keypair: &identity::Keypair,
        group_id: String,
        id: String,
        timestamp: i64,
        parents: Vec<String>,
        body: GroupRecordBody,
    ) -> anyhow::Result<Self> {
        let author_peer_id = PeerId::from_public_key(&keypair.public()).to_string();
        let unsigned = UnsignedGroupRecord {
            version: Self::VERSION,
            id,
            group_id,
            author_peer_id,
            timestamp,
            parents,
            body,
        };
        let canonical = serde_json::to_vec(&unsigned)?;
        let signature = keypair.sign(&canonical)?;
        Ok(Self {
            unsigned,
            public_key_b64: BASE64.encode(keypair.public().encode_protobuf()),
            signature_b64: BASE64.encode(signature),
        })
    }

    pub fn verify(&self) -> bool {
        let Ok(public_key_bytes) = BASE64.decode(&self.public_key_b64) else {
            return false;
        };
        let Ok(signature) = BASE64.decode(&self.signature_b64) else {
            return false;
        };
        let Ok(public_key) = identity::PublicKey::try_decode_protobuf(&public_key_bytes) else {
            return false;
        };
        let derived_peer_id = PeerId::from_public_key(&public_key).to_string();
        if derived_peer_id != self.unsigned.author_peer_id {
            return false;
        }
        let Ok(canonical) = serde_json::to_vec(&self.unsigned) else {
            return false;
        };
        public_key.verify(&canonical, &signature)
    }

    pub fn id(&self) -> &str {
        &self.unsigned.id
    }

    pub fn group_id(&self) -> &str {
        &self.unsigned.group_id
    }

    pub fn author_peer_id(&self) -> &str {
        &self.unsigned.author_peer_id
    }

    pub fn timestamp(&self) -> i64 {
        self.unsigned.timestamp
    }

    pub fn body(&self) -> &GroupRecordBody {
        &self.unsigned.body
    }
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

    #[test]
    fn signed_group_record_roundtrips_and_verifies() {
        let key = identity::Keypair::generate_ed25519();
        let record = SignedGroupRecord::new(
            &key,
            "group:550e8400-e29b-41d4-a716-446655440000".to_string(),
            "rec-1".to_string(),
            42,
            Vec::new(),
            GroupRecordBody::GroupCreated {
                name: "Test".to_string(),
                settings: None,
            },
        )
        .expect("sign");

        let json = serde_json::to_string(&record).expect("serialize");
        let decoded: SignedGroupRecord = serde_json::from_str(&json).expect("decode");
        assert!(decoded.verify());
    }

    #[test]
    fn legacy_group_envelope_still_decodes_without_signature_fields() {
        let raw = r#"{
            "id":"m1",
            "group_id":"group:550e8400-e29b-41d4-a716-446655440000",
            "sender_id":"peer",
            "timestamp":1,
            "content_type":"text",
            "text_content":"hello"
        }"#;

        let decoded: GroupMessageEnvelope = serde_json::from_str(raw).expect("legacy decode");
        assert_eq!(decoded.id, "m1");
        assert_eq!(decoded.protocol_version, None);
        assert_eq!(decoded.signed_record_id, None);
    }
}
