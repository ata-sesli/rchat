use libp2p::PeerId;
use std::collections::HashMap;

pub const GITHUB_CHAT_PREFIX: &str = "gh:";
pub const LOCAL_CHAT_PREFIX: &str = "lh:";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DirectChatScope {
    Github,
    Local,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedScopedDirectChatId {
    pub scope: DirectChatScope,
    pub name: String,
    pub peer_id: String,
}

pub fn normalize_name_component(input: &str) -> String {
    let mut normalized = String::new();
    let mut previous_was_dash = false;

    for ch in input.trim().to_ascii_lowercase().chars() {
        if ch.is_ascii_alphanumeric() {
            normalized.push(ch);
            previous_was_dash = false;
            continue;
        }

        if !previous_was_dash {
            normalized.push('-');
            previous_was_dash = true;
        }
    }

    let normalized = normalized.trim_matches('-').to_string();
    if normalized.is_empty() {
        "peer".to_string()
    } else {
        normalized
    }
}

pub fn build_github_chat_id(name: &str, peer_id: &str) -> String {
    format!(
        "{}{}-{}",
        GITHUB_CHAT_PREFIX,
        normalize_name_component(name),
        peer_id
    )
}

pub fn build_local_chat_id(name: &str, peer_id: &str) -> String {
    format!(
        "{}{}-{}",
        LOCAL_CHAT_PREFIX,
        normalize_name_component(name),
        peer_id
    )
}

fn parse_scoped_chat_id(
    chat_id: &str,
    prefix: &str,
    scope: DirectChatScope,
) -> Option<ParsedScopedDirectChatId> {
    let rest = chat_id.strip_prefix(prefix)?;
    let (name, peer_id) = rest.rsplit_once('-')?;
    if name.trim().is_empty() || peer_id.trim().is_empty() {
        return None;
    }
    if peer_id.parse::<PeerId>().is_err() {
        return None;
    }

    Some(ParsedScopedDirectChatId {
        scope,
        name: name.to_string(),
        peer_id: peer_id.to_string(),
    })
}

pub fn parse_scoped_direct_chat_id(chat_id: &str) -> Option<ParsedScopedDirectChatId> {
    parse_scoped_chat_id(chat_id, GITHUB_CHAT_PREFIX, DirectChatScope::Github).or_else(|| {
        parse_scoped_chat_id(chat_id, LOCAL_CHAT_PREFIX, DirectChatScope::Local)
    })
}

pub fn extract_peer_id_from_chat_id(chat_id: &str) -> Option<String> {
    if let Some(parsed) = parse_scoped_direct_chat_id(chat_id) {
        return Some(parsed.peer_id);
    }

    if chat_id.parse::<PeerId>().is_ok() {
        return Some(chat_id.to_string());
    }

    None
}

pub fn extract_name_from_chat_id(chat_id: &str) -> Option<String> {
    parse_scoped_direct_chat_id(chat_id).map(|parsed| parsed.name)
}

pub fn resolve_peer_id_for_direct_chat_id(chat_id: &str) -> Option<String> {
    if let Some(peer_id) = extract_peer_id_from_chat_id(chat_id) {
        return Some(peer_id);
    }

    None
}

pub fn github_chat_id_for_peer_id(
    peer_id: &str,
    github_peer_mapping: &HashMap<String, String>,
) -> Option<String> {
    github_peer_mapping.iter().find_map(|(github_username, mapped)| {
        if mapped == peer_id {
            Some(build_github_chat_id(github_username, mapped))
        } else {
            None
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const PEER_ID: &str = "12D3KooWLk1GoEB3MbHbRLHTxXrvNGSxC2UALaCuKAgKuYXkXazU";

    #[test]
    fn normalizes_name_component() {
        assert_eq!(normalize_name_component(" professional tester "), "professional-tester");
        assert_eq!(normalize_name_component("..."), "peer");
    }

    #[test]
    fn parses_and_extracts_peer_from_scoped_chat_id() {
        let gh_id = build_github_chat_id("professional-tester", PEER_ID);
        let parsed = parse_scoped_direct_chat_id(&gh_id).expect("parse gh");
        assert_eq!(parsed.scope, DirectChatScope::Github);
        assert_eq!(parsed.peer_id, PEER_ID);

        let lh_id = build_local_chat_id("Ata Sesli", PEER_ID);
        assert_eq!(extract_peer_id_from_chat_id(&lh_id), Some(PEER_ID.to_string()));
    }

    #[test]
    fn does_not_accept_legacy_gh_chat_id() {
        assert_eq!(
            resolve_peer_id_for_direct_chat_id("gh:professional-tester"),
            None
        );
    }
}
