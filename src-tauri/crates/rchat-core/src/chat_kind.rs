use rand::RngCore;
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChatKind {
    SelfChat,
    Direct,
    Group,
    TemporaryDirect,
    TemporaryGroup,
    Archived,
}

pub fn parse_chat_kind(chat_id: &str) -> ChatKind {
    if is_self_chat(chat_id) {
        ChatKind::SelfChat
    } else if is_group_chat_id(chat_id) {
        ChatKind::Group
    } else if is_temp_group_chat_id(chat_id) {
        ChatKind::TemporaryGroup
    } else if is_temp_direct_chat_id(chat_id) {
        ChatKind::TemporaryDirect
    } else if is_archived_chat_id(chat_id) {
        ChatKind::Archived
    } else {
        ChatKind::Direct
    }
}

pub fn is_self_chat(chat_id: &str) -> bool {
    chat_id == "self" || chat_id == "Me"
}

pub fn is_group_chat_id(chat_id: &str) -> bool {
    group_uuid_from_chat_id(chat_id).is_some()
}

pub fn is_temp_group_chat_id(chat_id: &str) -> bool {
    temp_group_uuid_from_chat_id(chat_id).is_some()
}

pub fn is_temp_direct_chat_id(chat_id: &str) -> bool {
    temp_direct_uuid_from_chat_id(chat_id).is_some()
}

pub fn is_temporary_chat_id(chat_id: &str) -> bool {
    is_temp_direct_chat_id(chat_id) || is_temp_group_chat_id(chat_id)
}

pub fn is_archived_chat_id(chat_id: &str) -> bool {
    chat_id.starts_with("archived:")
}

pub fn group_uuid_from_chat_id(chat_id: &str) -> Option<&str> {
    let uuid = chat_id.strip_prefix("group:")?;
    if is_uuid_like(uuid) {
        Some(uuid)
    } else {
        None
    }
}

pub fn temp_group_uuid_from_chat_id(chat_id: &str) -> Option<&str> {
    let uuid = chat_id.strip_prefix("temp-group:")?;
    if is_uuid_like(uuid) {
        Some(uuid)
    } else {
        None
    }
}

pub fn temp_direct_uuid_from_chat_id(chat_id: &str) -> Option<&str> {
    let uuid = chat_id.strip_prefix("tempdm:")?;
    if is_uuid_like(uuid) {
        Some(uuid)
    } else {
        None
    }
}

pub fn default_group_name(chat_id: &str) -> String {
    let suffix = group_uuid_from_chat_id(chat_id)
        .map(|u| u.split('-').next().unwrap_or(u))
        .unwrap_or("unknown");
    format!("Group {}", suffix)
}

pub fn default_temp_group_name(chat_id: &str) -> String {
    let suffix = temp_group_uuid_from_chat_id(chat_id)
        .map(|u| u.split('-').next().unwrap_or(u))
        .unwrap_or("unknown");
    format!("Temporary Group {}", suffix)
}

pub fn default_temp_direct_name(chat_id: &str) -> String {
    let suffix = temp_direct_uuid_from_chat_id(chat_id)
        .map(|u| u.split('-').next().unwrap_or(u))
        .unwrap_or("unknown");
    format!("Temporary Chat {}", suffix)
}

pub fn generate_group_chat_id() -> String {
    format!("group:{}", generate_uuid_v4())
}

pub fn derive_group_chat_id(founder_peer_id: &str, genesis_record_id: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"rchat-group-v3\0");
    hasher.update(founder_peer_id.as_bytes());
    hasher.update([0]);
    hasher.update(genesis_record_id.as_bytes());
    let digest = hasher.finalize();
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    bytes[6] = (bytes[6] & 0x0f) | 0x50;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    format!("group:{}", format_uuid(bytes))
}

pub fn generate_temp_group_chat_id() -> String {
    format!("temp-group:{}", generate_uuid_v4())
}

pub fn generate_temp_direct_chat_id() -> String {
    format!("tempdm:{}", generate_uuid_v4())
}

fn generate_uuid_v4() -> String {
    let mut bytes = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut bytes);

    // UUID v4 bits
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;

    format_uuid(bytes)
}

fn format_uuid(bytes: [u8; 16]) -> String {
    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes[0],
        bytes[1],
        bytes[2],
        bytes[3],
        bytes[4],
        bytes[5],
        bytes[6],
        bytes[7],
        bytes[8],
        bytes[9],
        bytes[10],
        bytes[11],
        bytes[12],
        bytes[13],
        bytes[14],
        bytes[15]
    )
}

fn is_uuid_like(input: &str) -> bool {
    if input.len() != 36 {
        return false;
    }

    for (idx, ch) in input.chars().enumerate() {
        let should_be_dash = matches!(idx, 8 | 13 | 18 | 23);
        if should_be_dash {
            if ch != '-' {
                return false;
            }
            continue;
        }
        if !ch.is_ascii_hexdigit() {
            return false;
        }
    }

    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_chat_kinds() {
        assert_eq!(parse_chat_kind("self"), ChatKind::SelfChat);
        assert_eq!(parse_chat_kind("Me"), ChatKind::SelfChat);
        assert_eq!(parse_chat_kind("peer-1"), ChatKind::Direct);
        assert_eq!(
            parse_chat_kind("group:550e8400-e29b-41d4-a716-446655440000"),
            ChatKind::Group
        );
        assert_eq!(
            parse_chat_kind("temp-group:550e8400-e29b-41d4-a716-446655440000"),
            ChatKind::TemporaryGroup
        );
        assert_eq!(
            parse_chat_kind("tempdm:550e8400-e29b-41d4-a716-446655440000"),
            ChatKind::TemporaryDirect
        );
        assert_eq!(
            parse_chat_kind("archived:temp-group:550e8400-e29b-41d4-a716-446655440000"),
            ChatKind::Archived
        );
        assert_eq!(parse_chat_kind("group:not-a-uuid"), ChatKind::Direct);
    }

    #[test]
    fn group_id_generation_is_valid() {
        let chat_id = generate_group_chat_id();
        assert!(is_group_chat_id(&chat_id));
    }

    #[test]
    fn temp_id_generation_is_valid() {
        let temp_group = generate_temp_group_chat_id();
        assert!(is_temp_group_chat_id(&temp_group));
        let temp_dm = generate_temp_direct_chat_id();
        assert!(is_temp_direct_chat_id(&temp_dm));
        assert!(is_temporary_chat_id(&temp_group));
        assert!(is_temporary_chat_id(&temp_dm));
    }
}
