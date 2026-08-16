use crate::app_state::{
    ActiveTemporaryInvite, AppState, NetworkState, TemporaryChatKind, TemporaryChatSession,
    TemporaryInvitePayload,
};
use crate::network::{
    command::NetworkCommand,
    gossip::{GroupContentType, GroupMessageEnvelope},
};
use crate::storage::db::Message;
use anyhow::{anyhow, Result};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use flate2::{read::GzDecoder, write::GzEncoder, Compression};
use std::io::{Read, Write};

pub const TEMP_INVITE_SCHEME_PREFIX: &str = "rchat://temp/";
pub const TEMP_INVITE_TTL_SECS: u64 = 120;
pub const TEMP_INVITE_VERSION: u8 = 1;

#[derive(Debug, Clone, serde::Serialize)]
pub struct TemporaryInviteView {
    pub deep_link: String,
    pub payload: TemporaryInvitePayload,
    pub remaining_seconds: u64,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct TemporaryChatResult {
    pub chat_id: String,
    pub name: String,
    pub kind: String,
    pub expires_at: u64,
    pub peer_id: Option<String>,
}

pub fn parse_temporary_chat_kind(kind: &str) -> Result<TemporaryChatKind> {
    match kind.trim().to_ascii_lowercase().as_str() {
        "dm" => Ok(TemporaryChatKind::Dm),
        "group" => Ok(TemporaryChatKind::Group),
        _ => Err(anyhow!("Invalid temporary chat kind. Use 'dm' or 'group'")),
    }
}

pub fn temporary_chat_kind_label(kind: &TemporaryChatKind) -> String {
    match kind {
        TemporaryChatKind::Dm => "dm".to_string(),
        TemporaryChatKind::Group => "group".to_string(),
    }
}

pub async fn create_temporary_invite(
    app_state: &AppState,
    net_state: &NetworkState,
    kind: TemporaryChatKind,
    name: Option<&str>,
) -> Result<TemporaryInviteView> {
    let chat_id = match kind {
        TemporaryChatKind::Dm => crate::chat_kind::generate_temp_direct_chat_id(),
        TemporaryChatKind::Group => crate::chat_kind::generate_temp_group_chat_id(),
    };
    let session_name = name
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| match kind {
            TemporaryChatKind::Dm => crate::chat_kind::default_temp_direct_name(&chat_id),
            TemporaryChatKind::Group => crate::chat_kind::default_temp_group_name(&chat_id),
        });

    let inviter_peer_id = net_state
        .local_peer_id
        .lock()
        .await
        .clone()
        .ok_or_else(|| anyhow!("Network is not started yet"))?;
    let inviter_addr = resolve_current_public_address(net_state).await?;
    let inviter_username = {
        let mgr = app_state.config_manager.lock().await;
        let config = mgr.load().await?;
        config
            .system
            .github_username
            .clone()
            .or(config.user.profile.alias.clone())
            .unwrap_or_else(|| "unknown".to_string())
    };

    let created_at = now_unix_secs();
    let expires_at = created_at + TEMP_INVITE_TTL_SECS;
    let payload = TemporaryInvitePayload {
        version: TEMP_INVITE_VERSION,
        kind: kind.clone(),
        chat_id: chat_id.clone(),
        inviter_peer_id,
        inviter_username,
        inviter_addr,
        created_at,
        expires_at,
    };
    let encoded = encode_temporary_payload(&payload)?;
    let deep_link = format!("{}{}", TEMP_INVITE_SCHEME_PREFIX, encoded);

    {
        let mut temp_state = net_state.temporary_state.lock().await;
        temp_state.active_invite = Some(ActiveTemporaryInvite {
            deep_link: deep_link.clone(),
            payload: payload.clone(),
        });
        temp_state.chats.insert(
            chat_id.clone(),
            TemporaryChatSession {
                chat_id: chat_id.clone(),
                name: session_name,
                kind,
                expires_at,
                peer_id: None,
                archived: false,
            },
        );
        temp_state.messages.entry(chat_id).or_default();
    }

    Ok(TemporaryInviteView {
        deep_link,
        payload,
        remaining_seconds: TEMP_INVITE_TTL_SECS,
    })
}

pub async fn get_active_temporary_invite(
    net_state: &NetworkState,
) -> Result<Option<TemporaryInviteView>> {
    let now = now_unix_secs();
    let mut temp_state = net_state.temporary_state.lock().await;

    if let Some(active) = temp_state.active_invite.as_ref() {
        if active.payload.expires_at <= now {
            temp_state.active_invite = None;
            return Ok(None);
        }
    }

    Ok(temp_state
        .active_invite
        .as_ref()
        .map(|active| TemporaryInviteView {
            deep_link: active.deep_link.clone(),
            payload: active.payload.clone(),
            remaining_seconds: active.payload.expires_at.saturating_sub(now),
        }))
}

pub async fn cancel_temporary_invite(net_state: &NetworkState) -> Result<()> {
    let mut temp_state = net_state.temporary_state.lock().await;
    if let Some(active) = temp_state.active_invite.take() {
        if let Some(session) = temp_state.chats.get(&active.payload.chat_id).cloned() {
            let has_messages = temp_state
                .messages
                .get(&active.payload.chat_id)
                .map(|messages| !messages.is_empty())
                .unwrap_or(false);
            if session.peer_id.is_none() && !has_messages {
                temp_state.chats.remove(&active.payload.chat_id);
                temp_state.messages.remove(&active.payload.chat_id);
            }
        }
    }
    Ok(())
}

pub async fn redeem_temporary_invite(
    net_state: &NetworkState,
    deep_link: &str,
) -> Result<TemporaryChatResult> {
    let token = extract_temporary_payload_token(deep_link)?;
    let payload = decode_temporary_payload(&token)?;
    if payload.version != TEMP_INVITE_VERSION {
        return Err(anyhow!(
            "Unsupported temporary invite version: {}",
            payload.version
        ));
    }

    let now = now_unix_secs();
    if payload.expires_at <= now {
        return Err(anyhow!("Temporary invite has expired"));
    }

    let mut temp_state = net_state.temporary_state.lock().await;
    let Some(local_active) = temp_state.active_invite.clone() else {
        return Err(anyhow!(
            "Create a temporary invite first before redeeming one"
        ));
    };
    if local_active.payload.expires_at <= now {
        temp_state.active_invite = None;
        return Err(anyhow!(
            "Your temporary invite has expired. Create a new one first"
        ));
    }
    if local_active.payload.kind != payload.kind {
        return Err(anyhow!("Temporary invite kind mismatch (dm/group)"));
    }

    let is_group = matches!(payload.kind, TemporaryChatKind::Group);
    let resolved_chat_id = if is_group {
        payload.chat_id.clone()
    } else {
        canonical_temp_dm_chat_id(&local_active.payload.chat_id, &payload.chat_id)
    };
    let expires_at = local_active.payload.expires_at.min(payload.expires_at);
    let resolved_name = if is_group {
        crate::chat_kind::default_temp_group_name(&resolved_chat_id)
    } else {
        crate::chat_kind::default_temp_direct_name(&resolved_chat_id)
    };

    if local_active.payload.chat_id != resolved_chat_id {
        temp_state.chats.remove(&local_active.payload.chat_id);
        temp_state.messages.remove(&local_active.payload.chat_id);
    }

    let entry = temp_state
        .chats
        .entry(resolved_chat_id.clone())
        .or_insert_with(|| TemporaryChatSession {
            chat_id: resolved_chat_id.clone(),
            name: resolved_name.clone(),
            kind: payload.kind.clone(),
            expires_at,
            peer_id: Some(payload.inviter_peer_id.clone()),
            archived: false,
        });
    entry.name = resolved_name.clone();
    entry.kind = payload.kind.clone();
    entry.expires_at = expires_at;
    entry.peer_id = Some(payload.inviter_peer_id.clone());
    entry.archived = false;
    temp_state
        .messages
        .entry(resolved_chat_id.clone())
        .or_default();
    drop(temp_state);

    {
        let tx = net_state.sender.lock().await;
        tx.send(NetworkCommand::RegisterTemporarySession {
            chat_id: resolved_chat_id.clone(),
            peer_id: payload.inviter_peer_id.clone(),
            multiaddr: payload.inviter_addr.clone(),
            is_group,
        })
        .await
        .map_err(|error| anyhow!("Failed to start temporary session: {error}"))?;
    }

    Ok(TemporaryChatResult {
        chat_id: resolved_chat_id,
        name: resolved_name,
        kind: temporary_chat_kind_label(&payload.kind),
        expires_at,
        peer_id: Some(payload.inviter_peer_id),
    })
}

/// Load the in-memory history of a temporary-group chat.
///
/// Temporary-group messages live only in `temporary_state` until the session
/// expires or is archived, so this reads them straight from there.
pub async fn get_temporary_group_history(
    net_state: &NetworkState,
    chat_id: &str,
) -> Result<Vec<Message>> {
    ensure_temp_group_chat_id(chat_id)?;
    let temp_state = net_state.temporary_state.lock().await;
    Ok(temp_state
        .messages
        .get(chat_id)
        .cloned()
        .unwrap_or_default())
}

/// Send a text message through the temporary-group path.
///
/// The message is appended to the in-memory temporary session history and then
/// published on the temporary-group gossip topic, mirroring how the web client
/// routes `TemporaryGroup` text messages.
pub async fn send_temporary_group_text(
    app_state: &AppState,
    net_state: &NetworkState,
    chat_id: &str,
    message: &str,
) -> Result<String> {
    let message = message.trim();
    if message.is_empty() {
        return Err(anyhow!("message is empty"));
    }
    ensure_temp_group_chat_id(chat_id)?;

    let my_alias = {
        let mgr = app_state.config_manager.lock().await;
        mgr.load().await?.user.profile.alias
    };
    let timestamp = now_unix_secs() as i64;
    let msg_id = format!("{}-{}", timestamp, rand::random::<u32>());
    let outgoing = Message {
        id: msg_id.clone(),
        chat_id: chat_id.to_string(),
        peer_id: "Me".to_string(),
        timestamp,
        content_type: "text".to_string(),
        text_content: Some(message.to_string()),
        file_hash: None,
        status: "delivered".to_string(),
        content_metadata: None,
        sender_alias: my_alias.clone(),
    };

    {
        let mut temp_state = net_state.temporary_state.lock().await;
        temp_state
            .messages
            .entry(chat_id.to_string())
            .or_default()
            .push(outgoing);
    }

    let envelope = GroupMessageEnvelope {
        id: msg_id.clone(),
        group_id: chat_id.to_string(),
        sender_id: "Me".to_string(),
        sender_alias: my_alias,
        timestamp,
        content_type: GroupContentType::Text,
        text_content: Some(message.to_string()),
        file_hash: None,
        protocol_version: None,
        signed_record_id: None,
    };
    let tx = net_state.sender.lock().await;
    tx.send(NetworkCommand::PublishGroup { envelope })
        .await
        .map_err(|error| anyhow!("network command channel is closed: {error}"))?;

    Ok(msg_id)
}

/// Mark incoming temporary-group messages as read in the in-memory session.
///
/// Returns the ids of the messages whose status changed. Own messages are
/// skipped, matching how temporary-direct chats are marked read.
pub async fn mark_temporary_group_messages_read(
    net_state: &NetworkState,
    chat_id: &str,
) -> Result<Vec<String>> {
    ensure_temp_group_chat_id(chat_id)?;
    let mut temp_state = net_state.temporary_state.lock().await;
    let messages = temp_state.messages.entry(chat_id.to_string()).or_default();
    let mut ids = Vec::new();
    for message in messages {
        if message.peer_id != "Me" && message.status != "read" {
            message.status = "read".to_string();
            ids.push(message.id.clone());
        }
    }
    Ok(ids)
}

fn ensure_temp_group_chat_id(chat_id: &str) -> Result<()> {
    if crate::chat_kind::is_temp_group_chat_id(chat_id) {
        Ok(())
    } else {
        Err(anyhow!("Not a temporary group chat id: {chat_id}"))
    }
}

fn now_unix_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

fn canonical_temp_dm_chat_id(a: &str, b: &str) -> String {
    if a <= b {
        a.to_string()
    } else {
        b.to_string()
    }
}

fn encode_temporary_payload(payload: &TemporaryInvitePayload) -> Result<String> {
    let json = serde_json::to_vec(payload)?;
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(&json)?;
    let compressed = encoder.finish()?;
    Ok(URL_SAFE_NO_PAD.encode(compressed))
}

fn decode_temporary_payload(encoded: &str) -> Result<TemporaryInvitePayload> {
    let gzipped = URL_SAFE_NO_PAD
        .decode(encoded)
        .map_err(|error| anyhow!("Invalid temporary invite payload: {error}"))?;
    let mut decoder = GzDecoder::new(gzipped.as_slice());
    let mut json = Vec::new();
    decoder
        .read_to_end(&mut json)
        .map_err(|error| anyhow!("Failed to gunzip temporary invite payload: {error}"))?;
    let payload: TemporaryInvitePayload = serde_json::from_slice(&json)
        .map_err(|error| anyhow!("Failed to parse temporary invite payload: {error}"))?;
    Ok(payload)
}

fn extract_temporary_payload_token(input: &str) -> Result<String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(anyhow!("Temporary invite link is empty"));
    }
    if let Some(token) = trimmed.strip_prefix(TEMP_INVITE_SCHEME_PREFIX) {
        if token.is_empty() {
            return Err(anyhow!("Temporary invite link payload is empty"));
        }
        return Ok(token.to_string());
    }
    Ok(trimmed.to_string())
}

async fn resolve_current_public_address(net_state: &NetworkState) -> Result<String> {
    let v4_stun = net_state.public_address_v4.lock().await.clone();
    let stun_port = *net_state.stun_external_port.lock().await;

    if let (Some(ip), Some(port)) = (v4_stun, stun_port) {
        return Ok(format!("/ip4/{ip}/udp/{port}/quic-v1"));
    }

    let addrs = net_state.listening_addresses.lock().await;
    addrs
        .iter()
        .find(|addr| {
            addr.contains("/udp/")
                && addr.contains("/quic-v1")
                && !addr.contains("127.0.0.1")
                && !addr.contains("::1")
        })
        .or_else(|| {
            addrs.iter().find(|addr| {
                addr.contains("/tcp/") && !addr.contains("127.0.0.1") && !addr.contains("::1")
            })
        })
        .or_else(|| addrs.first())
        .cloned()
        .ok_or_else(|| anyhow!("No listening address available. Is the network started?"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app_state::{
        BroadcastState, ChatConnectionRuntime, TemporaryRuntimeState, VoiceCallState,
    };
    use crate::storage::config::{ConfigManager, ConnectivitySettings};
    use std::collections::{HashMap, HashSet};
    use std::sync::Arc;
    use tokio::sync::{mpsc, Mutex};

    const LOCAL_PEER_ID: &str = "12D3KooWLk1GoEB3MbHbRLHTxXrvNGSxC2UALaCuKAgKuYXkXazU";
    const REMOTE_PEER_ID: &str = "12D3KooWAKrRudfV7S7XK418Jg4c8SvCkcnjwjhoATAQ1J6NAw86";

    async fn test_app_state() -> (tempfile::TempDir, AppState) {
        let temp = tempfile::tempdir().expect("tempdir");
        let app_dir = temp.path().join("rchat-data");
        std::fs::create_dir_all(&app_dir).expect("app dir");
        let mut manager = ConfigManager::new(app_dir.clone());
        let mut config = manager.init("password").await.expect("init config");
        config.system.github_username = Some("local-user".to_string());
        manager.save(&config).await.expect("save config");
        let conn = rusqlite::Connection::open_in_memory().expect("in-memory db");

        (
            temp,
            AppState {
                config_manager: Arc::new(Mutex::new(manager)),
                db_conn: Arc::new(std::sync::Mutex::new(conn)),
                app_dir,
            },
        )
    }

    fn test_network_state() -> (NetworkState, mpsc::Receiver<NetworkCommand>) {
        let (tx, rx) = mpsc::channel(8);
        (
            NetworkState {
                sender: Arc::new(Mutex::new(tx)),
                local_peer_id: Arc::new(Mutex::new(Some(LOCAL_PEER_ID.to_string()))),
                listening_addresses: Arc::new(Mutex::new(vec![
                    "/ip4/192.168.1.10/udp/5000/quic-v1".to_string(),
                ])),
                public_address_v6: Arc::new(Mutex::new(None)),
                public_address_v4: Arc::new(Mutex::new(None)),
                stun_external_port: Arc::new(Mutex::new(None)),
                temporary_state: Arc::new(Mutex::new(TemporaryRuntimeState::default())),
                connected_chat_ids: Arc::new(Mutex::new(HashSet::new())),
                chat_connections: Arc::new(Mutex::new(
                    HashMap::<String, ChatConnectionRuntime>::new(),
                )),
                voice_call_state: Arc::new(Mutex::new(VoiceCallState::default())),
                broadcast_state: Arc::new(Mutex::new(BroadcastState::default())),
                connectivity: Arc::new(Mutex::new(ConnectivitySettings::default())),
            },
            rx,
        )
    }

    fn remote_invite(chat_id: &str) -> String {
        let now = now_unix_secs();
        let payload = TemporaryInvitePayload {
            version: TEMP_INVITE_VERSION,
            kind: TemporaryChatKind::Dm,
            chat_id: chat_id.to_string(),
            inviter_peer_id: REMOTE_PEER_ID.to_string(),
            inviter_username: "remote".to_string(),
            inviter_addr: "/ip4/192.168.1.11/udp/5001/quic-v1".to_string(),
            created_at: now,
            expires_at: now + TEMP_INVITE_TTL_SECS,
        };
        format!(
            "{}{}",
            TEMP_INVITE_SCHEME_PREFIX,
            encode_temporary_payload(&payload).expect("encode")
        )
    }

    #[tokio::test]
    async fn temporary_dm_invite_create_get_cancel_round_trips() {
        let (_temp, app_state) = test_app_state().await;
        let (net_state, _rx) = test_network_state();

        let created = create_temporary_invite(&app_state, &net_state, TemporaryChatKind::Dm, None)
            .await
            .expect("create");
        assert!(created.deep_link.starts_with(TEMP_INVITE_SCHEME_PREFIX));
        assert_eq!(created.remaining_seconds, TEMP_INVITE_TTL_SECS);

        let active = get_active_temporary_invite(&net_state)
            .await
            .expect("get")
            .expect("active invite");
        assert_eq!(active.deep_link, created.deep_link);

        cancel_temporary_invite(&net_state).await.expect("cancel");
        assert!(get_active_temporary_invite(&net_state)
            .await
            .expect("get")
            .is_none());
    }

    #[tokio::test]
    async fn redeem_rejects_empty_malformed_and_missing_local_active_links() {
        let (net_state, _rx) = test_network_state();

        assert!(redeem_temporary_invite(&net_state, "")
            .await
            .expect_err("empty")
            .to_string()
            .contains("empty"));
        assert!(redeem_temporary_invite(&net_state, "not-a-valid-token")
            .await
            .expect_err("malformed")
            .to_string()
            .contains("Invalid temporary invite payload"));
        assert!(redeem_temporary_invite(&net_state, &remote_invite("temp:dm-b"))
            .await
            .expect_err("missing local active")
            .to_string()
            .contains("Create a temporary invite first"));
    }

    #[tokio::test]
    async fn redeem_temporary_dm_registers_session_command() {
        let (_temp, app_state) = test_app_state().await;
        let (net_state, mut rx) = test_network_state();
        let local = create_temporary_invite(&app_state, &net_state, TemporaryChatKind::Dm, None)
            .await
            .expect("create local");
        let remote_link = remote_invite("temp:dm-z");

        let result = redeem_temporary_invite(&net_state, &remote_link)
            .await
            .expect("redeem");

        assert_eq!(result.kind, "dm");
        assert_eq!(
            result.chat_id,
            if local.payload.chat_id <= "temp:dm-z".to_string() {
                local.payload.chat_id
            } else {
                "temp:dm-z".to_string()
            }
        );
        match rx.recv().await.expect("command") {
            NetworkCommand::RegisterTemporarySession {
                chat_id,
                peer_id,
                multiaddr,
                is_group,
            } => {
                assert_eq!(chat_id, result.chat_id);
                assert_eq!(peer_id, REMOTE_PEER_ID);
                assert_eq!(multiaddr, "/ip4/192.168.1.11/udp/5001/quic-v1");
                assert!(!is_group);
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn temporary_invite_kind_parser_accepts_dm_and_group() {
        assert_eq!(
            parse_temporary_chat_kind("dm").unwrap(),
            TemporaryChatKind::Dm
        );
        assert_eq!(
            parse_temporary_chat_kind("GROUP").unwrap(),
            TemporaryChatKind::Group
        );
        assert!(parse_temporary_chat_kind("room").is_err());
    }

    #[test]
    fn temporary_payload_extracts_prefixed_or_raw_tokens() {
        assert_eq!(
            extract_temporary_payload_token("abc").unwrap(),
            "abc".to_string()
        );
        assert_eq!(
            extract_temporary_payload_token("rchat://temp/abc").unwrap(),
            "abc".to_string()
        );
        assert!(extract_temporary_payload_token("rchat://temp/").is_err());
    }

    #[tokio::test]
    async fn temporary_group_text_round_trips_through_temp_state_and_publish() {
        let (_temp, app_state) = test_app_state().await;
        let (net_state, mut rx) = test_network_state();
        let chat_id = crate::chat_kind::generate_temp_group_chat_id();

        let msg_id =
            send_temporary_group_text(&app_state, &net_state, &chat_id, "  hello group  ")
                .await
                .expect("send");

        let messages = net_state
            .temporary_state
            .lock()
            .await
            .messages
            .get(&chat_id)
            .cloned()
            .unwrap_or_default();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].id, msg_id);
        assert_eq!(messages[0].peer_id, "Me");
        assert_eq!(messages[0].text_content.as_deref(), Some("hello group"));
        assert_eq!(messages[0].status, "delivered");

        match rx.recv().await.expect("command") {
            NetworkCommand::PublishGroup { envelope } => {
                assert_eq!(envelope.id, msg_id);
                assert_eq!(envelope.group_id, chat_id);
                assert_eq!(envelope.sender_id, "Me");
                assert_eq!(envelope.content_type, GroupContentType::Text);
                assert_eq!(envelope.text_content.as_deref(), Some("hello group"));
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[tokio::test]
    async fn temporary_group_text_rejects_empty_and_non_group_ids() {
        let (_temp, app_state) = test_app_state().await;
        let (net_state, _rx) = test_network_state();

        assert!(send_temporary_group_text(&app_state, &net_state, "peer-1", "hello")
            .await
            .expect_err("not a temp group")
            .to_string()
            .contains("Not a temporary group chat id"));
        assert!(send_temporary_group_text(
            &app_state,
            &net_state,
            &crate::chat_kind::generate_temp_group_chat_id(),
            "   "
        )
        .await
        .expect_err("empty message")
        .to_string()
        .contains("empty"));
    }

    #[tokio::test]
    async fn temporary_group_history_returns_stored_messages() {
        let (_temp, _app_state) = test_app_state().await;
        let (net_state, _rx) = test_network_state();
        let chat_id = crate::chat_kind::generate_temp_group_chat_id();
        {
            let mut temp_state = net_state.temporary_state.lock().await;
            temp_state.messages.insert(
                chat_id.clone(),
                vec![
                    temp_group_message(&chat_id, "m1", "first"),
                    temp_group_message(&chat_id, "m2", "second"),
                ],
            );
        }

        let history = get_temporary_group_history(&net_state, &chat_id)
            .await
            .expect("history");

        assert_eq!(history.len(), 2);
        assert_eq!(history[0].text_content.as_deref(), Some("first"));
        assert_eq!(history[1].text_content.as_deref(), Some("second"));
    }

    #[tokio::test]
    async fn temporary_group_history_rejects_non_group_ids() {
        let (net_state, _rx) = test_network_state();

        assert!(get_temporary_group_history(&net_state, "peer-1")
            .await
            .expect_err("not a temp group")
            .to_string()
            .contains("Not a temporary group chat id"));
    }

    #[tokio::test]
    async fn temporary_group_mark_read_only_marks_incoming_messages() {
        let (_temp, _app_state) = test_app_state().await;
        let (net_state, _rx) = test_network_state();
        let chat_id = crate::chat_kind::generate_temp_group_chat_id();
        {
            let mut temp_state = net_state.temporary_state.lock().await;
            temp_state.messages.insert(
                chat_id.clone(),
                vec![
                    temp_group_message(&chat_id, "m1", "from peer"),
                    temp_group_message(&chat_id, "m2", "from me"),
                ],
            );
            temp_state
                .messages
                .get_mut(&chat_id)
                .expect("messages")
                .iter_mut()
                .for_each(|message| {
                    if message.id == "m2" {
                        message.peer_id = "Me".to_string();
                    }
                });
        }

        let marked = mark_temporary_group_messages_read(&net_state, &chat_id)
            .await
            .expect("mark read");

        assert_eq!(marked, vec!["m1"]);
        let messages = net_state
            .temporary_state
            .lock()
            .await
            .messages
            .get(&chat_id)
            .cloned()
            .unwrap_or_default();
        assert_eq!(messages[0].status, "read");
        assert_eq!(messages[1].status, "pending");
    }

    fn temp_group_message(chat_id: &str, id: &str, text: &str) -> Message {
        Message {
            id: id.to_string(),
            chat_id: chat_id.to_string(),
            peer_id: REMOTE_PEER_ID.to_string(),
            timestamp: 1_700_000_000,
            content_type: "text".to_string(),
            text_content: Some(text.to_string()),
            file_hash: None,
            status: "pending".to_string(),
            content_metadata: None,
            sender_alias: None,
        }
    }

}
