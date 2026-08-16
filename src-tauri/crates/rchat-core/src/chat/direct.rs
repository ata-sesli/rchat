use crate::{
    app_state::{AppState, NetworkState, TemporaryChatKind},
    chat_identity,
    chat_kind::{self, ChatKind},
    network::{command::NetworkCommand, discovery, gist, invite},
    storage,
    storage::config::FriendConfig,
};
use anyhow::{anyhow, Result};
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectChatSummary {
    pub id: String,
    pub name: String,
    pub latest_timestamp: i64,
    pub unread_count: i64,
}

pub fn generate_invite_password() -> String {
    rvault_core::crypto::generate_password(14, false)
}

pub async fn list_direct_chats(
    app_state: &AppState,
    net_state: &NetworkState,
) -> Result<Vec<DirectChatSummary>> {
    let (items, latest_times, unread_counts) = {
        let conn = app_state
            .db_conn
            .lock()
            .map_err(|error| anyhow!("database lock failed: {error}"))?;
        (
            storage::db::get_chat_list(&conn)?,
            storage::db::get_chat_latest_times(&conn)?,
            storage::db::get_unread_counts(&conn, "Me")?,
        )
    };

    let mapped_by_peer = mapped_chat_ids_by_peer(app_state).await;
    let mut summaries = Vec::new();
    for item in items {
        if item.is_group {
            continue;
        }
        summaries.push(direct_summary_from_item(
            item,
            &latest_times,
            &unread_counts,
            &mapped_by_peer,
        ));
    }

    let now = now_unix_timestamp();
    let temp_state = net_state.temporary_state.lock().await;
    for (chat_id, session) in &temp_state.chats {
        if session.archived || !matches!(session.kind, TemporaryChatKind::Dm) {
            continue;
        }
        let latest_timestamp = temp_state
            .messages
            .get(chat_id)
            .and_then(|messages| messages.last())
            .map(|message| message.timestamp)
            .unwrap_or(now);
        if summaries.iter().any(|summary| summary.id == *chat_id) {
            continue;
        }
        summaries.push(DirectChatSummary {
            id: chat_id.clone(),
            name: session.name.clone(),
            latest_timestamp,
            unread_count: 0,
        });
    }

    let mut summaries = dedupe_direct_summaries(summaries);
    summaries.sort_by(|a, b| {
        b.latest_timestamp
            .cmp(&a.latest_timestamp)
            .then_with(|| a.name.cmp(&b.name))
    });
    Ok(summaries)
}

pub async fn get_direct_history(
    app_state: &AppState,
    net_state: &NetworkState,
    chat_id: &str,
) -> Result<Vec<storage::db::Message>> {
    let resolved_chat_id = canonical_direct_or_self_chat_id(app_state, chat_id).await?;
    match chat_kind::parse_chat_kind(&resolved_chat_id) {
        ChatKind::SelfChat | ChatKind::Direct | ChatKind::Archived => {
            let conn = app_state
                .db_conn
                .lock()
                .map_err(|error| anyhow!("database lock failed: {error}"))?;
            storage::db::get_messages(&conn, &db_chat_id(&resolved_chat_id))
        }
        ChatKind::TemporaryDirect => {
            let temp_state = net_state.temporary_state.lock().await;
            Ok(temp_state
                .messages
                .get(&resolved_chat_id)
                .cloned()
                .unwrap_or_default())
        }
        ChatKind::Group | ChatKind::TemporaryGroup => Err(anyhow!("group chats are not supported by rchat-tui")),
    }
}

pub async fn send_direct_text(
    app_state: &AppState,
    net_state: &NetworkState,
    chat_id: &str,
    message: &str,
) -> Result<String> {
    let message = message.trim();
    if message.is_empty() {
        return Err(anyhow!("message is empty"));
    }

    let canonical_chat_id = canonical_direct_or_self_chat_id(app_state, chat_id).await?;
    let chat_kind = chat_kind::parse_chat_kind(&canonical_chat_id);
    if matches!(chat_kind, ChatKind::Group | ChatKind::TemporaryGroup) {
        return Err(anyhow!("group chats are not supported by rchat-tui"));
    }
    if matches!(chat_kind, ChatKind::Archived) {
        return Err(anyhow!("archived chats are read-only"));
    }

    let my_alias = {
        let mgr = app_state.config_manager.lock().await;
        mgr.load().await?.user.profile.alias
    };
    let msg_id = format!("{}-{}", now_unix_timestamp(), rand::random::<u32>());
    let timestamp = now_unix_timestamp();
    let resolved_target = resolve_peer_id_for_chat(&canonical_chat_id)
        .unwrap_or_else(|| canonical_chat_id.clone());
    let is_temporary = matches!(chat_kind, ChatKind::TemporaryDirect);
    let db_chat_id = if matches!(chat_kind, ChatKind::SelfChat) {
        "self".to_string()
    } else {
        canonical_chat_id.clone()
    };

    let outgoing = storage::db::Message {
        id: msg_id.clone(),
        chat_id: db_chat_id.clone(),
        peer_id: "Me".to_string(),
        timestamp,
        content_type: "text".to_string(),
        text_content: Some(message.to_string()),
        file_hash: None,
        status: outgoing_status(chat_kind).to_string(),
        content_metadata: None,
        sender_alias: my_alias.clone(),
    };

    if is_temporary {
        let mut temp_state = net_state.temporary_state.lock().await;
        temp_state
            .messages
            .entry(canonical_chat_id.clone())
            .or_default()
            .push(outgoing);
    } else {
        let conn = app_state
            .db_conn
            .lock()
            .map_err(|error| anyhow!("database lock failed: {error}"))?;
        ensure_direct_chat_rows(&conn, &canonical_chat_id, &resolved_target)?;
        storage::db::insert_message(&conn, &outgoing)?;
    }

    if !matches!(chat_kind, ChatKind::SelfChat) {
        let tx = net_state.sender.lock().await;
        tx.send(NetworkCommand::SendDirectText {
            target_peer_id: resolved_target,
            msg_id: msg_id.clone(),
            timestamp,
            sender_alias: my_alias,
            content: message.to_string(),
        })
        .await
        .map_err(|error| anyhow!("network command channel is closed: {error}"))?;
    }

    Ok(msg_id)
}

pub async fn mark_direct_messages_read(
    app_state: &AppState,
    net_state: &NetworkState,
    chat_id: &str,
) -> Result<Vec<String>> {
    let resolved_chat_id = canonical_direct_or_self_chat_id(app_state, chat_id).await?;
    let chat_kind = chat_kind::parse_chat_kind(&resolved_chat_id);
    if !matches!(
        chat_kind,
        ChatKind::SelfChat
            | ChatKind::Direct
            | ChatKind::TemporaryDirect
            | ChatKind::TemporaryGroup
    ) {
        return Ok(Vec::new());
    }

    let marked_ids = if matches!(
        chat_kind,
        ChatKind::TemporaryDirect | ChatKind::TemporaryGroup
    ) {
        let mut temp_state = net_state.temporary_state.lock().await;
        let messages = temp_state
            .messages
            .entry(resolved_chat_id.clone())
            .or_default();
        let mut ids = Vec::new();
        for message in messages {
            if message.peer_id != "Me" && message.status != "read" {
                message.status = "read".to_string();
                ids.push(message.id.clone());
            }
        }
        ids
    } else {
        let sender_id = resolve_peer_id_for_chat(&resolved_chat_id)
            .unwrap_or_else(|| resolved_chat_id.clone());
        let conn = app_state
            .db_conn
            .lock()
            .map_err(|error| anyhow!("database lock failed: {error}"))?;
        storage::db::mark_messages_read(&conn, &db_chat_id(&resolved_chat_id), &sender_id)?
    };

    if !marked_ids.is_empty() && matches!(chat_kind, ChatKind::Direct | ChatKind::TemporaryDirect)
    {
        let target_peer_id =
            resolve_peer_id_for_chat(&resolved_chat_id).unwrap_or_else(|| resolved_chat_id.clone());
        let tx = net_state.sender.lock().await;
        let _ = tx
            .send(NetworkCommand::SendReadReceipt {
                target_peer_id,
                msg_ids: marked_ids.clone(),
            })
            .await;
    }

    Ok(marked_ids)
}

pub async fn create_github_invite(
    app_state: &AppState,
    net_state: &NetworkState,
    invitee: &str,
    password: &str,
) -> Result<()> {
    let (my_username, token) = {
        let mgr = app_state.config_manager.lock().await;
        let config = mgr.load().await?;
        let username = config
            .system
            .github_username
            .clone()
            .ok_or_else(|| anyhow!("GitHub username not set"))?;
        let token = config
            .system
            .github_token
            .clone()
            .ok_or_else(|| anyhow!("GitHub token not set"))?;
        (username, token)
    };

    let local_peer_id = net_state
        .local_peer_id
        .lock()
        .await
        .clone()
        .ok_or_else(|| anyhow!("Network peer id not available. Is the network started?"))?;
    let my_address = resolve_current_public_address(net_state).await?;
    let encrypted_invite = invite::generate_invite(
        password,
        &my_username,
        invitee,
        &my_address,
        &local_peer_id,
        120,
    )?;
    let tracked = gist::track_invite(encrypted_invite);

    {
        let mgr = app_state.config_manager.lock().await;
        let mut config = mgr.load().await?;
        if config.user.pending_invitations.is_none() {
            config.user.pending_invitations = Some(Vec::new());
        }
        if let Some(invites) = &mut config.user.pending_invitations {
            invites.push(serde_json::to_string(&tracked)?);
        }
        mgr.save(&config).await?;
    }

    discovery::publish_peer_info(&token, vec![], app_state).await?;
    let tx = net_state.sender.lock().await;
    let _ = tx.send(NetworkCommand::RegisterShadow {
        invitee: invitee.to_string(),
        password: password.to_string(),
        my_username,
    })
    .await;
    Ok(())
}

pub async fn redeem_github_invite(
    app_state: &AppState,
    net_state: &NetworkState,
    inviter: &str,
    password: &str,
) -> Result<String> {
    let my_username = {
        let mgr = app_state.config_manager.lock().await;
        let config = mgr.load().await?;
        config
            .system
            .github_username
            .clone()
            .ok_or_else(|| anyhow!("GitHub username not set"))?
    };

    let encrypted_invites = gist::get_friend_invitations(inviter).await?;
    if encrypted_invites.is_empty() {
        return Err(anyhow!("No invitations found from this user"));
    }

    let Some((payload, _index)) =
        invite::process_invites(&encrypted_invites, password, inviter, &my_username)?
    else {
        return Err(anyhow!(
            "No valid invitation found for you. Check password and usernames."
        ));
    };

    let github_username = inviter.to_string();
    let existing_peer_id = {
        let mgr = app_state.config_manager.lock().await;
        mgr.load().await.ok().and_then(|config| {
            config
                .user
                .github_peer_mapping
                .get(&github_username)
                .cloned()
        })
    };
    let invite_peer_id = payload.inviter_peer_id.clone().and_then(|candidate| {
        if candidate.parse::<libp2p::PeerId>().is_ok() {
            Some(candidate)
        } else {
            None
        }
    });
    let resolved_peer_id = existing_peer_id.or(invite_peer_id).ok_or_else(|| {
        anyhow!("Invitation is missing inviter peer id. Ask the inviter to generate a new invite.")
    })?;
    let chat_id = chat_identity::build_github_chat_id(&github_username, &resolved_peer_id);

    {
        let mgr = app_state.config_manager.lock().await;
        let mut config = mgr.load().await?;
        if !config
            .user
            .friends
            .iter()
            .any(|friend| friend.username == github_username)
        {
            config.user.friends.push(FriendConfig {
                username: github_username.clone(),
                alias: None,
                x25519_pubkey: None,
                ed25519_pubkey: None,
                leaf_index: 0,
                encrypted_leaf_key: None,
                nonce: None,
            });
            mgr.save(&config).await?;
        }
    }

    let timestamp = now_unix_timestamp();
    {
        let conn = app_state
            .db_conn
            .lock()
            .map_err(|error| anyhow!("database lock failed: {error}"))?;
        if !storage::db::is_peer(&conn, &chat_id) {
            storage::db::add_peer(&conn, &chat_id, Some(&github_username), None, "github")?;
        }
        if !storage::db::chat_exists(&conn, &chat_id) {
            storage::db::create_chat(&conn, &chat_id, &github_username, false)?;
        }
        storage::db::insert_message(
            &conn,
            &storage::db::Message {
                id: format!("{}-{}", timestamp, rand::random::<u32>()),
                chat_id: chat_id.clone(),
                peer_id: "Me".to_string(),
                timestamp,
                content_type: "text".to_string(),
                text_content: Some("Hi!".to_string()),
                file_hash: None,
                status: "delivered".to_string(),
                content_metadata: None,
                sender_alias: None,
            },
        )?;
    }

    if let Some(token) = {
        let mgr = app_state.config_manager.lock().await;
        mgr.load().await?.system.github_token.clone()
    } {
        let my_address = resolve_current_public_address(net_state)
            .await
            .unwrap_or_else(|_| "unknown".to_string());
        if let Ok(shadow) =
            invite::generate_shadow_invite(password, inviter, &my_username, &my_address, "pending")
        {
            let _ = gist::publish_shadow_invite(&token, shadow).await;
        }
    }

    let tx = net_state.sender.lock().await;
    let _ = tx.send(NetworkCommand::StartPunch {
        multiaddr: payload.ip_address.clone(),
        target_username: github_username,
        my_username,
    })
    .await;

    Ok(chat_id)
}

pub(crate) async fn canonical_direct_or_self_chat_id(
    app_state: &AppState,
    chat_id: &str,
) -> Result<String> {
    let normalized = db_chat_id(chat_id);
    if matches!(chat_kind::parse_chat_kind(&normalized), ChatKind::SelfChat) {
        return Ok("self".to_string());
    }
    if !matches!(
        chat_kind::parse_chat_kind(&normalized),
        ChatKind::Direct | ChatKind::TemporaryDirect | ChatKind::Archived
    ) {
        return Ok(normalized);
    }
    if normalized.starts_with("gh:") || normalized.starts_with("lh:") {
        let conn = app_state
            .db_conn
            .lock()
            .map_err(|error| anyhow!("database lock failed: {error}"))?;
        if let Some(existing) = existing_direct_chat_id_for_chat(&conn, &normalized)? {
            return Ok(existing);
        }
        return Ok(normalized);
    }

    if let Some(mapped) = mapped_github_chat_id_for_peer(app_state, &normalized).await {
        return Ok(mapped);
    }

    let conn = app_state
        .db_conn
        .lock()
        .map_err(|error| anyhow!("database lock failed: {error}"))?;
    if let Some(existing) = storage::db::find_existing_local_chat_id_for_peer(&conn, &normalized)? {
        return Ok(existing);
    }
    let local_name = storage::db::get_peer_alias(&conn, &normalized)?
        .filter(|name| !name.trim().is_empty() && name != &normalized)
        .unwrap_or_else(|| "peer".to_string());
    Ok(chat_identity::build_local_chat_id(&local_name, &normalized))
}

async fn resolve_current_public_address(net_state: &NetworkState) -> Result<String> {
    let v4_stun = net_state.public_address_v4.lock().await.clone();
    let stun_port = *net_state.stun_external_port.lock().await;

    if let (Some(ip), Some(port)) = (v4_stun, stun_port) {
        return Ok(format!("/ip4/{}/udp/{}/quic-v1", ip, port));
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
            addrs
                .iter()
                .find(|addr| addr.contains("/tcp/") && !addr.contains("127.0.0.1") && !addr.contains("::1"))
        })
        .or_else(|| addrs.first())
        .cloned()
        .ok_or_else(|| anyhow!("No listening address available. Is the network started?"))
}

async fn mapped_github_chat_id_for_peer(app_state: &AppState, peer_id: &str) -> Option<String> {
    let mgr = app_state.config_manager.lock().await;
    let Ok(config) = mgr.load().await else {
        return None;
    };
    chat_identity::github_chat_id_for_peer_id(peer_id, &config.user.github_peer_mapping)
}

async fn mapped_chat_ids_by_peer(app_state: &AppState) -> HashMap<String, String> {
    let mgr = app_state.config_manager.lock().await;
    mgr.load()
        .await
        .map(|config| {
            config
                .user
                .github_peer_mapping
                .into_iter()
                .map(|(github, peer_id)| {
                    let chat_id = chat_identity::build_github_chat_id(&github, &peer_id);
                    (peer_id, chat_id)
                })
                .collect()
        })
        .unwrap_or_default()
}

fn direct_summary_from_item(
    item: storage::db::ChatListItem,
    latest_times: &HashMap<String, i64>,
    unread_counts: &HashMap<String, i64>,
    mapped_by_peer: &HashMap<String, String>,
) -> DirectChatSummary {
    let db_id = item.id;
    let display_id = mapped_by_peer
        .get(&db_id)
        .cloned()
        .unwrap_or_else(|| db_id.clone());
    let id = ui_chat_id(&display_id);

    DirectChatSummary {
        latest_timestamp: canonical_value(latest_times, &db_id, &id, mapped_by_peer),
        unread_count: canonical_value(unread_counts, &db_id, &id, mapped_by_peer),
        id,
        name: item.name,
    }
}

fn existing_direct_chat_id_for_chat(
    conn: &rusqlite::Connection,
    chat_id: &str,
) -> Result<Option<String>> {
    let Some(peer_id) = resolve_peer_id_for_chat(chat_id) else {
        return Ok(None);
    };
    storage::db::find_existing_direct_chat_id_for_peer(conn, &peer_id)
}

fn canonical_value(
    values: &HashMap<String, i64>,
    db_id: &str,
    ui_id: &str,
    mapped_by_peer: &HashMap<String, String>,
) -> i64 {
    values
        .get(db_id)
        .or_else(|| values.get(ui_id))
        .or_else(|| mapped_by_peer.get(db_id).and_then(|id| values.get(id)))
        .copied()
        .unwrap_or(0)
}

fn dedupe_direct_summaries(summaries: Vec<DirectChatSummary>) -> Vec<DirectChatSummary> {
    let mut by_key: HashMap<String, DirectChatSummary> = HashMap::new();
    for summary in summaries {
        let key = direct_summary_key(&summary.id);
        by_key
            .entry(key)
            .and_modify(|existing| merge_direct_summary(existing, &summary))
            .or_insert(summary);
    }
    by_key.into_values().collect()
}

fn direct_summary_key(chat_id: &str) -> String {
    resolve_peer_id_for_chat(chat_id)
        .map(|peer_id| format!("peer:{peer_id}"))
        .unwrap_or_else(|| chat_id.to_string())
}

fn merge_direct_summary(existing: &mut DirectChatSummary, incoming: &DirectChatSummary) {
    existing.latest_timestamp = existing.latest_timestamp.max(incoming.latest_timestamp);
    existing.unread_count = existing.unread_count.max(incoming.unread_count);

    if is_preferred_chat_id(&incoming.id, &existing.id) {
        existing.id = incoming.id.clone();
    }
    if is_preferred_chat_name(&incoming.name, &existing.name, &existing.id) {
        existing.name = incoming.name.clone();
    }
}

fn is_preferred_chat_id(candidate: &str, current: &str) -> bool {
    let candidate_kind = direct_id_rank(candidate);
    let current_kind = direct_id_rank(current);
    candidate_kind > current_kind || (candidate_kind == current_kind && candidate.len() < current.len())
}

fn direct_id_rank(chat_id: &str) -> u8 {
    if chat_id.starts_with("gh:") {
        3
    } else if chat_id.starts_with("lh:") {
        2
    } else if chat_id == "Me" || chat_id == "self" {
        2
    } else if looks_like_peer_id(chat_id) {
        0
    } else {
        1
    }
}

fn is_preferred_chat_name(candidate: &str, current: &str, chat_id: &str) -> bool {
    let candidate = candidate.trim();
    let current = current.trim();
    if candidate.is_empty() || candidate == chat_id || looks_like_peer_id(candidate) {
        return false;
    }
    current.is_empty() || current == chat_id || looks_like_peer_id(current) || candidate.len() < current.len()
}

fn looks_like_peer_id(value: &str) -> bool {
    value.parse::<libp2p::PeerId>().is_ok()
}

pub(crate) fn resolve_peer_id_for_chat(chat_id: &str) -> Option<String> {
    chat_identity::resolve_peer_id_for_direct_chat_id(chat_id)
}

pub(crate) fn ensure_direct_chat_rows(
    conn: &rusqlite::Connection,
    chat_id: &str,
    resolved_peer_id: &str,
) -> Result<()> {
    if matches!(chat_kind::parse_chat_kind(chat_id), ChatKind::SelfChat) {
        return Ok(());
    }
    let name = default_direct_chat_name(chat_id);
    if !storage::db::is_peer(conn, chat_id) {
        storage::db::add_peer(
            conn,
            chat_id,
            Some(&name),
            None,
            if chat_id.starts_with("gh:") {
                "github"
            } else {
                "local"
            },
        )?;
    }
    if resolved_peer_id != chat_id && !storage::db::is_peer(conn, resolved_peer_id) {
        storage::db::add_peer(conn, resolved_peer_id, Some(&name), None, "local")?;
    }
    if !storage::db::chat_exists(conn, chat_id) {
        storage::db::create_chat(conn, chat_id, &name, false)?;
    }
    let _ = storage::db::add_chat_member(conn, chat_id, "Me", "member");
    let _ = storage::db::add_chat_member(conn, chat_id, resolved_peer_id, "member");
    Ok(())
}

fn default_direct_chat_name(chat_id: &str) -> String {
    chat_identity::extract_name_from_chat_id(chat_id)
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(|| "peer".to_string())
}

fn ui_chat_id(chat_id: &str) -> String {
    if chat_id == "self" {
        "Me".to_string()
    } else {
        chat_id.to_string()
    }
}

pub(crate) fn db_chat_id(chat_id: &str) -> String {
    if chat_id == "Me" {
        "self".to_string()
    } else {
        chat_id.to_string()
    }
}

pub(crate) fn outgoing_status(kind: ChatKind) -> &'static str {
    match kind {
        ChatKind::SelfChat => "read",
        ChatKind::Direct | ChatKind::TemporaryDirect => "pending",
        ChatKind::Group | ChatKind::TemporaryGroup => "delivered",
        ChatKind::Archived => "read",
    }
}

pub(crate) fn now_unix_timestamp() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    const PEER_ID: &str = "12D3KooWLk1GoEB3MbHbRLHTxXrvNGSxC2UALaCuKAgKuYXkXazU";

    #[test]
    fn mapped_summary_preserves_raw_chat_activity() {
        let item = storage::db::ChatListItem {
            id: PEER_ID.to_string(),
            name: "fedora".to_string(),
            is_group: false,
        };
        let mapped_id = chat_identity::build_github_chat_id("ata", PEER_ID);
        let mapped_by_peer = HashMap::from([(PEER_ID.to_string(), mapped_id.clone())]);
        let latest_times = HashMap::from([(PEER_ID.to_string(), 123)]);
        let unread_counts = HashMap::from([(PEER_ID.to_string(), 2)]);

        let summary = direct_summary_from_item(item, &latest_times, &unread_counts, &mapped_by_peer);

        assert_eq!(summary.id, mapped_id);
        assert_eq!(summary.latest_timestamp, 123);
        assert_eq!(summary.unread_count, 2);
    }

    #[test]
    fn scoped_chat_id_resolves_existing_raw_chat_history() {
        let conn = rusqlite::Connection::open_in_memory().expect("in-memory db");
        conn.execute(
            "CREATE TABLE chats (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                is_group INTEGER NOT NULL,
                encryption_key BLOB NOT NULL
            )",
            [],
        )
        .expect("create chats");
        conn.execute(
            "INSERT INTO chats (id, name, is_group, encryption_key) VALUES (?1, 'fedora', 0, ?2)",
            (PEER_ID, vec![0u8; 32]),
        )
        .expect("insert raw chat");

        let mapped_id = chat_identity::build_github_chat_id("ata", PEER_ID);

        assert_eq!(
            existing_direct_chat_id_for_chat(&conn, &mapped_id).unwrap(),
            Some(PEER_ID.to_string())
        );
    }

    #[test]
    fn generated_invite_password_has_expected_length() {
        assert_eq!(generate_invite_password().chars().count(), 14);
    }
}
