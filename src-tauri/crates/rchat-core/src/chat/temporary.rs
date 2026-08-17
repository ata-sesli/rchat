use crate::app_state::{
    ActiveTemporaryInvite, AppState, NetworkState, TemporaryChatKind, TemporaryChatSession,
    TemporaryInvitePayload, TemporaryMembershipOpKind,
};
use crate::network::{
    command::NetworkCommand,
    gossip::{GroupContentType, GroupMessageEnvelope},
};
use crate::storage::db::Message;
use anyhow::{anyhow, Result};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use flate2::{read::GzDecoder, write::GzEncoder, Compression};
use std::collections::HashMap;
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
    // The inviter is the local member of a temporary group; seed the roster
    // with the local peer id so membership is tracked explicitly from birth.
    let local_membership = if matches!(kind, TemporaryChatKind::Group) {
        vec![inviter_peer_id.clone()]
    } else {
        Vec::new()
    };
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
                members: local_membership,
                member_ops: Vec::new(),
                member_op_winners: HashMap::new(),
                next_member_op_seq: 0,
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
    let local_peer_id = net_state.local_peer_id.lock().await.clone();
    let mut temp_state = net_state.temporary_state.lock().await;
    if let Some(active) = temp_state.active_invite.take() {
        if let Some(session) = temp_state.chats.get(&active.payload.chat_id).cloned() {
            let has_messages = temp_state
                .messages
                .get(&active.payload.chat_id)
                .map(|messages| !messages.is_empty())
                .unwrap_or(false);
            // An empty session only ever contains the inviter themselves (a
            // temporary group's local member); anything more means the chat
            // has real history or remote members and must be preserved.
            let only_local = session.peer_id.is_none()
                && session
                    .members
                    .iter()
                    .all(|member| Some(member.as_str()) == local_peer_id.as_deref());
            if only_local && !has_messages {
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

    // Read the local peer id before locking the temporary state so the lock
    // order stays local_peer_id -> temporary_state everywhere.
    let local_peer_id = net_state
        .local_peer_id
        .lock()
        .await
        .clone()
        .unwrap_or_else(|| "Me".to_string());

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
    // A redeemer joins the group as a member alongside the inviter; the local
    // peer id seeds the roster so local and remote members are both explicit.
    let seeded_members = if is_group {
        let mut members = vec![local_peer_id];
        if !members.contains(&payload.inviter_peer_id) {
            members.push(payload.inviter_peer_id.clone());
        }
        members
    } else {
        Vec::new()
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
            members: seeded_members.clone(),
            member_ops: Vec::new(),
            member_op_winners: HashMap::new(),
            next_member_op_seq: 0,
            archived: false,
        });
    entry.name = resolved_name.clone();
    entry.kind = payload.kind.clone();
    entry.expires_at = expires_at;
    entry.peer_id = Some(payload.inviter_peer_id.clone());
    if is_group {
        for member in seeded_members {
            entry.add_member(&member);
        }
    }
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
/// expires or is archived, so this reads them straight from there. The
/// session itself must still be resolvable, active, and unarchived so stale
/// chat ids cannot materialize phantom history entries.
pub async fn get_temporary_group_history(
    net_state: &NetworkState,
    chat_id: &str,
) -> Result<Vec<Message>> {
    validate_temp_group_session(net_state, chat_id).await?;
    let temp_state = net_state.temporary_state.lock().await;
    Ok(temp_state
        .messages
        .get(chat_id)
        .cloned()
        .unwrap_or_default())
}

/// Send a text message through the temporary-group path.
///
/// The message is published on the temporary-group gossip topic and then
/// appended to the in-memory temporary session history, mirroring how the
/// web client routes `TemporaryGroup` text messages. Enqueuing first ensures
/// a closed command channel fails the send without leaving a phantom
/// `delivered` message behind.
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
    validate_temp_group_session(net_state, chat_id).await?;

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

    // Enqueue the publish before storing the message so a closed command
    // channel cannot leave a phantom `delivered` message behind; retrying
    // would otherwise duplicate it.
    let tx = net_state.sender.lock().await;
    tx.send(NetworkCommand::PublishGroup { envelope })
        .await
        .map_err(|error| anyhow!("network command channel is closed: {error}"))?;
    drop(tx);

    let mut temp_state = net_state.temporary_state.lock().await;
    temp_state
        .messages
        .entry(chat_id.to_string())
        .or_default()
        .push(outgoing);

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
    validate_temp_group_session(net_state, chat_id).await?;
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

/// Resolve a temporary-group session and validate it is usable.
///
/// Rejects chat ids that are not temporary groups, and sessions that are
/// missing, of the wrong kind, archived, or expired. Callers use this before
/// sending, loading history, or marking read so a dead session can never
/// create in-memory history entries or publish to a gossip topic.
pub(crate) async fn validate_temp_group_session(
    net_state: &NetworkState,
    chat_id: &str,
) -> Result<TemporaryChatSession> {
    ensure_temp_group_chat_id(chat_id)?;
    let temp_state = net_state.temporary_state.lock().await;
    let session = temp_state
        .chats
        .get(chat_id)
        .cloned()
        .ok_or_else(|| anyhow!("Temporary group chat not found: {chat_id}"))?;
    if !matches!(session.kind, TemporaryChatKind::Group) {
        return Err(anyhow!("Not a temporary group chat id: {chat_id}"));
    }
    if session.archived {
        return Err(anyhow!("Temporary group chat is archived: {chat_id}"));
    }
    if session.expires_at <= now_unix_secs() {
        return Err(anyhow!("Temporary group chat has expired: {chat_id}"));
    }
    Ok(session)
}

/// Remote member peer ids of a validated temporary-group session, excluding
/// the local peer id.
///
/// Per-peer requests (file metadata retries, direct handshakes) fan out to
/// every member returned here so routing targets all eligible remote members
/// without duplicates.
pub async fn temporary_group_remote_members(
    net_state: &NetworkState,
    chat_id: &str,
) -> Result<Vec<String>> {
    let session = validate_temp_group_session(net_state, chat_id).await?;
    let local_peer_id = net_state.local_peer_id.lock().await.clone();
    Ok(session.remote_members(local_peer_id.as_deref()))
}

/// Leave a temporary-group session locally.
///
/// Records a remove tombstone for the local peer, clears the active invite if
/// it points at this chat, and tells the network manager to end the temporary
/// session. The manager broadcasts the updated membership log to the other
/// members and then removes the session; leaving is group-scoped, so shared
/// libp2p connections to the other members are never closed.
pub async fn leave_temporary_group(
    net_state: &NetworkState,
    chat_id: &str,
) -> Result<()> {
    validate_temp_group_session(net_state, chat_id).await?;

    let local_peer_id = net_state.local_peer_id.lock().await.clone();
    {
        let mut temp_state = net_state.temporary_state.lock().await;
        let session = temp_state
            .chats
            .get_mut(chat_id)
            .ok_or_else(|| anyhow!("Temporary group chat not found: {chat_id}"))?;
        if let Some(local) = local_peer_id.as_deref() {
            session.issue_membership_op(
                local,
                TemporaryMembershipOpKind::Remove,
                local,
            );
        }
        if temp_state
            .active_invite
            .as_ref()
            .map(|active| active.payload.chat_id == chat_id)
            .unwrap_or(false)
        {
            temp_state.active_invite = None;
        }
    }

    let tx = net_state.sender.lock().await;
    tx.send(NetworkCommand::EndTemporarySession {
        chat_id: chat_id.to_string(),
    })
    .await
    .map_err(|error| anyhow!("network command channel is closed: {error}"))?;
    Ok(())
}

/// Archive a temporary chat (direct or group) into the durable database.
///
/// Persists the archive inside one transaction first and only removes the
/// live in-memory session after the commit succeeds, so a write failure rolls
/// back and preserves the conversation. The complete member roster of group
/// sessions is preserved as chat members, with the resolved local peer id
/// excluded so the archive never stores duplicate local identities.
pub async fn archive_temporary_chat(
    app_state: &AppState,
    net_state: &NetworkState,
    chat_id: &str,
) -> Result<ArchivedTemporaryChat> {
    if !crate::chat_kind::is_temporary_chat_id(chat_id) {
        return Err(anyhow!("Only temporary chats can be archived"));
    }

    let now = now_unix_secs() as i64;
    let archive_chat_id = format!("archived:{}:{}", chat_id, now);

    // Snapshot the live session without touching it yet; the archive only
    // removes it after the transaction commits.
    let (session, messages) = {
        let temp_state = net_state.temporary_state.lock().await;
        let Some(session) = temp_state.chats.get(chat_id).cloned() else {
            return Err(anyhow!("Temporary chat not found"));
        };
        let messages = temp_state
            .messages
            .get(chat_id)
            .cloned()
            .unwrap_or_default();
        if messages.is_empty() {
            return Err(anyhow!("No temporary messages to archive"));
        }
        (session, messages)
    };

    let local_peer_id = net_state.local_peer_id.lock().await.clone();
    // Resolve the roster so the local identity is stored exactly once: the
    // literal "Me" row plus every remote member (never the local peer id).
    let remote_members = session.remote_members(local_peer_id.as_deref());

    {
        let mut conn = app_state
            .db_conn
            .lock()
            .map_err(|error| anyhow!("database lock failed: {error}"))?;
        let tx = conn
            .transaction()
            .map_err(|error| anyhow!("failed to begin archive transaction: {error}"))?;

        if tx
            .query_row("SELECT 1 FROM envelopes WHERE id = 'archived'", [], |_| Ok(()))
            .is_err()
        {
            crate::storage::db::create_envelope(&tx, "archived", "Archived", None)?;
        }

        let archived_is_group = matches!(session.kind, TemporaryChatKind::Group);
        crate::storage::db::create_chat(&tx, &archive_chat_id, &session.name, archived_is_group)?;
        let _ = crate::storage::db::add_chat_member(&tx, &archive_chat_id, "Me", "member");
        // Preserve the complete member roster of group sessions, excluding
        // the resolved local peer id (already represented by "Me").
        for member in &remote_members {
            if !crate::storage::db::is_peer(&tx, member) {
                let _ = crate::storage::db::add_peer(&tx, member, None, None, "archived");
            }
            let _ = crate::storage::db::add_chat_member(&tx, &archive_chat_id, member, "member");
        }

        for (idx, mut msg) in messages.into_iter().enumerate() {
            msg.id = format!("{}-{}", msg.id, idx);
            msg.chat_id = archive_chat_id.clone();
            msg.status = "read".to_string();

            if msg.peer_id != "Me" && !crate::storage::db::is_peer(&tx, &msg.peer_id) {
                let _ = crate::storage::db::add_peer(&tx, &msg.peer_id, None, None, "archived");
            }

            if let Some(file_hash) = &msg.file_hash {
                let file_exists: bool = tx
                    .query_row(
                        "SELECT 1 FROM files WHERE file_hash = ?1",
                        [file_hash],
                        |_| Ok(true),
                    )
                    .unwrap_or(false);
                if !file_exists {
                    msg.text_content = Some(
                        msg.text_content
                            .clone()
                            .unwrap_or_else(|| "Media unavailable".to_string()),
                    );
                    msg.file_hash = None;
                }
            }

            crate::storage::db::insert_message(&tx, &msg)?;
        }

        crate::storage::db::assign_chat_to_envelope(&tx, &archive_chat_id, Some("archived"))?;
        tx.commit()
            .map_err(|error| anyhow!("failed to commit archive transaction: {error}"))?;
    }

    // The archive is durable; only now remove the live session. On any write
    // failure above, the transaction rolled back and the session survived.
    {
        let mut temp_state = net_state.temporary_state.lock().await;
        temp_state.chats.remove(chat_id);
        temp_state.messages.remove(chat_id);
    }

    let tx = net_state.sender.lock().await;
    let _ = tx
        .send(NetworkCommand::EndTemporarySession {
            chat_id: chat_id.to_string(),
        })
        .await;

    Ok(ArchivedTemporaryChat {
        chat_id: archive_chat_id,
        name: session.name,
    })
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ArchivedTemporaryChat {
    pub chat_id: String,
    pub name: String,
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
        BroadcastState, ChatConnectionRuntime, TemporaryMembershipOp, TemporaryRuntimeState,
        VoiceCallState,
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
        insert_temp_group_session(&net_state, &chat_id, now_unix_secs() + 3600, false).await;

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
        insert_temp_group_session(&net_state, &chat_id, now_unix_secs() + 3600, false).await;
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
        insert_temp_group_session(&net_state, &chat_id, now_unix_secs() + 3600, false).await;
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

    async fn insert_temp_group_session(
        net_state: &NetworkState,
        chat_id: &str,
        expires_at: u64,
        archived: bool,
    ) {
        let mut temp_state = net_state.temporary_state.lock().await;
        temp_state.chats.insert(
            chat_id.to_string(),
            TemporaryChatSession {
                chat_id: chat_id.to_string(),
                name: "Design Crew".to_string(),
                kind: TemporaryChatKind::Group,
                expires_at,
                peer_id: Some(REMOTE_PEER_ID.to_string()),
                members: vec![
                    LOCAL_PEER_ID.to_string(),
                    REMOTE_PEER_ID.to_string(),
                    "12D3KooWHT5qT2qfE9L4q9v4mD4VqQwZgTnNqYp9rXm7kQjY3bWx1".to_string(),
                ],
                member_ops: Vec::new(),
                member_op_winners: HashMap::new(),
                next_member_op_seq: 0,
                archived,
            },
        );
    }

    #[tokio::test]
    async fn temporary_group_text_send_failure_leaves_history_unchanged() {
        let (_temp, app_state) = test_app_state().await;
        let (net_state, rx) = test_network_state();
        let chat_id = crate::chat_kind::generate_temp_group_chat_id();
        insert_temp_group_session(&net_state, &chat_id, now_unix_secs() + 3600, false).await;
        drop(rx);

        let result = send_temporary_group_text(&app_state, &net_state, &chat_id, "hello").await;
        assert!(result.is_err());

        let messages = net_state
            .temporary_state
            .lock()
            .await
            .messages
            .get(&chat_id)
            .cloned()
            .unwrap_or_default();
        assert!(
            messages.is_empty(),
            "failed send must not leave a phantom delivered message"
        );
    }

    #[tokio::test]
    async fn temporary_group_operations_reject_missing_archived_and_expired_sessions() {
        let (_temp, app_state) = test_app_state().await;
        let (net_state, _rx) = test_network_state();
        let chat_id = crate::chat_kind::generate_temp_group_chat_id();

        assert!(send_temporary_group_text(&app_state, &net_state, &chat_id, "hello")
            .await
            .expect_err("missing session")
            .to_string()
            .contains("not found"));
        assert!(get_temporary_group_history(&net_state, &chat_id)
            .await
            .expect_err("missing session")
            .to_string()
            .contains("not found"));
        assert!(mark_temporary_group_messages_read(&net_state, &chat_id)
            .await
            .expect_err("missing session")
            .to_string()
            .contains("not found"));

        insert_temp_group_session(&net_state, &chat_id, now_unix_secs() + 3600, true).await;
        assert!(send_temporary_group_text(&app_state, &net_state, &chat_id, "hello")
            .await
            .expect_err("archived session")
            .to_string()
            .contains("archived"));

        let expired_id = crate::chat_kind::generate_temp_group_chat_id();
        insert_temp_group_session(
            &net_state,
            &expired_id,
            now_unix_secs().saturating_sub(60),
            false,
        )
        .await;
        assert!(send_temporary_group_text(&app_state, &net_state, &expired_id, "hello")
            .await
            .expect_err("expired session")
            .to_string()
            .contains("expired"));
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

    const THIRD_MEMBER_ID: &str = "12D3KooWS2ErH2fC6AB7jZ8V8vmu6jgoP711oD5QV3vA4MbGV3RM";

    #[tokio::test]
    async fn temporary_group_remote_members_excludes_local_without_duplicates() {
        let (_temp, _app_state) = test_app_state().await;
        let (net_state, _rx) = test_network_state();
        let chat_id = crate::chat_kind::generate_temp_group_chat_id();
        {
            let mut temp_state = net_state.temporary_state.lock().await;
            let mut session = TemporaryChatSession {
                chat_id: chat_id.clone(),
                name: "Design Crew".to_string(),
                kind: TemporaryChatKind::Group,
                expires_at: now_unix_secs() + 3600,
                peer_id: Some(REMOTE_PEER_ID.to_string()),
                members: vec![LOCAL_PEER_ID.to_string()],
                member_ops: Vec::new(),
                member_op_winners: HashMap::new(),
                next_member_op_seq: 0,
                archived: false,
            };
            // Add remote members twice: the roster must dedupe.
            assert!(session.add_member(REMOTE_PEER_ID));
            assert!(session.add_member(THIRD_MEMBER_ID));
            assert!(!session.add_member(REMOTE_PEER_ID));
            assert!(!session.add_member(LOCAL_PEER_ID));
            temp_state.chats.insert(chat_id.clone(), session);
        }

        let members = temporary_group_remote_members(&net_state, &chat_id)
            .await
            .expect("remote members");

        // Three-or-more member roster: every remote member exactly once, never
        // the local peer.
        assert_eq!(members.len(), 2);
        assert!(members.contains(&REMOTE_PEER_ID.to_string()));
        assert!(members.contains(&THIRD_MEMBER_ID.to_string()));
        assert!(!members.contains(&LOCAL_PEER_ID.to_string()));
        assert!(!members.contains(&"Me".to_string()));
    }

    #[test]
    fn temporary_group_session_membership_updates_on_join_and_disconnect() {
        let mut session = TemporaryChatSession {
            chat_id: "temp-group:550e8400-e29b-41d4-a716-446655440000".to_string(),
            name: "Design Crew".to_string(),
            kind: TemporaryChatKind::Group,
            expires_at: now_unix_secs() + 3600,
            peer_id: Some(REMOTE_PEER_ID.to_string()),
            members: vec![LOCAL_PEER_ID.to_string(), REMOTE_PEER_ID.to_string()],
            member_ops: Vec::new(),
            member_op_winners: HashMap::new(),
            next_member_op_seq: 0,
            archived: false,
        };

        // Join: a third member is added once and only once.
        assert!(session.add_member(THIRD_MEMBER_ID));
        assert!(!session.add_member(THIRD_MEMBER_ID));
        assert!(session.is_member(THIRD_MEMBER_ID));
        assert_eq!(session.remote_members(Some(LOCAL_PEER_ID)).len(), 2);

        // Disconnect: the third member leaves the roster.
        assert!(session.remove_member(THIRD_MEMBER_ID));
        assert!(!session.remove_member(THIRD_MEMBER_ID));
        assert!(!session.is_member(THIRD_MEMBER_ID));
        assert_eq!(session.remote_members(Some(LOCAL_PEER_ID)), vec![REMOTE_PEER_ID.to_string()]);
    }

    #[tokio::test]
    async fn temporary_group_text_publishes_once_for_multi_member_roster() {
        let (_temp, app_state) = test_app_state().await;
        let (net_state, mut rx) = test_network_state();
        let chat_id = crate::chat_kind::generate_temp_group_chat_id();
        insert_temp_group_session(&net_state, &chat_id, now_unix_secs() + 3600, false).await;

        let msg_id = send_temporary_group_text(&app_state, &net_state, &chat_id, "hello all")
            .await
            .expect("send");

        // A multi-member roster must still produce exactly one topic publish:
        // gossip fans out to every subscriber, so per-member duplicates would
        // double-deliver.
        match rx.recv().await.expect("command") {
            NetworkCommand::PublishGroup { envelope } => assert_eq!(envelope.id, msg_id),
            other => panic!("unexpected command: {other:?}"),
        }
        assert!(
            rx.try_recv().is_err(),
            "a multi-member send must not enqueue extra commands"
        );
    }

    #[tokio::test]
    async fn leave_temporary_group_records_tombstone_without_dropping_connections() {
        let (_temp, _app_state) = test_app_state().await;
        let (net_state, mut rx) = test_network_state();
        let chat_id = crate::chat_kind::generate_temp_group_chat_id();
        insert_temp_group_session(&net_state, &chat_id, now_unix_secs() + 3600, false).await;

        leave_temporary_group(&net_state, &chat_id).await.expect("leave");

        // The session is retained for the manager to clean up, but now carries
        // a remove tombstone for the local peer so the exit propagates
        // group-wide instead of closing shared transport connections.
        let session = net_state
            .temporary_state
            .lock()
            .await
            .chats
            .get(&chat_id)
            .cloned()
            .expect("session retained for manager cleanup");
        assert!(session.member_ops.iter().any(|op| {
            op.op == TemporaryMembershipOpKind::Remove && op.target == LOCAL_PEER_ID
        }));
        assert!(!session.is_member(LOCAL_PEER_ID));

        let mut commands = Vec::new();
        while let Ok(command) = rx.try_recv() {
            commands.push(command);
        }
        assert!(matches!(
            commands.first(),
            Some(NetworkCommand::EndTemporarySession { .. })
        ));
        assert!(
            commands
                .iter()
                .all(|command| !matches!(command, NetworkCommand::DropConnection { .. })),
            "leaving a group must not drop the peer's shared libp2p connections"
        );
    }

    #[tokio::test]
    async fn archive_temporary_group_preserves_complete_member_roster() {
        let (_temp, app_state) = crate::testing::test_app_state().await;
        let (net_state, _rx) = test_network_state();
        let chat_id = crate::chat_kind::generate_temp_group_chat_id();
        {
            let mut temp_state = net_state.temporary_state.lock().await;
            temp_state.chats.insert(
                chat_id.clone(),
                TemporaryChatSession {
                    chat_id: chat_id.clone(),
                    name: "Design Crew".to_string(),
                    kind: TemporaryChatKind::Group,
                    expires_at: now_unix_secs() + 3600,
                    peer_id: Some(REMOTE_PEER_ID.to_string()),
                    members: vec![
                        LOCAL_PEER_ID.to_string(),
                        REMOTE_PEER_ID.to_string(),
                        THIRD_MEMBER_ID.to_string(),
                    ],
                    member_ops: Vec::new(),
                    member_op_winners: HashMap::new(),
                    next_member_op_seq: 0,
                    archived: false,
                },
            );
            temp_state.messages.insert(
                chat_id.clone(),
                vec![
                    temp_group_message(&chat_id, "m1", "hello"),
                    temp_group_message(&chat_id, "m2", "world"),
                ],
            );
        }

        let archived = archive_temporary_chat(&app_state, &net_state, &chat_id)
            .await
            .expect("archive");

        assert!(archived.chat_id.starts_with(&format!("archived:{}:", chat_id)));
        assert!(net_state
            .temporary_state
            .lock()
            .await
            .chats
            .get(&chat_id)
            .is_none());

        let conn = app_state.db_conn.lock().expect("db");
        let mut members: Vec<String> = conn
            .prepare("SELECT peer_id FROM chat_peers WHERE chat_id = ?1")
            .expect("prepare")
            .query_map([archived.chat_id.as_str()], |row| row.get(0))
            .expect("query")
            .collect::<Result<Vec<_>, _>>()
            .expect("rows");
        members.sort();

        // The archive stores the literal "Me" plus every remote member; the
        // resolved local peer id is excluded so identities are not duplicated.
        let mut expected = vec![REMOTE_PEER_ID.to_string(), THIRD_MEMBER_ID.to_string()];
        expected.push("Me".to_string());
        expected.sort();
        assert_eq!(members, expected);
        assert!(!members.contains(&LOCAL_PEER_ID.to_string()));

        let archived_messages = crate::storage::db::get_messages(&conn, &archived.chat_id)
            .expect("archived history");
        assert_eq!(archived_messages.len(), 2);
        assert!(archived_messages.iter().all(|message| message.status == "read"));
    }

    #[tokio::test]
    async fn archive_failure_preserves_live_session() {
        let (_temp, app_state) = crate::testing::test_app_state().await;
        let (net_state, _rx) = test_network_state();
        let chat_id = crate::chat_kind::generate_temp_group_chat_id();
        {
            let mut temp_state = net_state.temporary_state.lock().await;
            temp_state.chats.insert(
                chat_id.clone(),
                TemporaryChatSession {
                    chat_id: chat_id.clone(),
                    name: "Design Crew".to_string(),
                    kind: TemporaryChatKind::Group,
                    expires_at: now_unix_secs() + 3600,
                    peer_id: Some(REMOTE_PEER_ID.to_string()),
                    members: vec![LOCAL_PEER_ID.to_string(), REMOTE_PEER_ID.to_string()],
                    member_ops: Vec::new(),
                    member_op_winners: HashMap::new(),
                    next_member_op_seq: 0,
                    archived: false,
                },
            );
            temp_state.messages.insert(
                chat_id.clone(),
                vec![
                    temp_group_message(&chat_id, "m1", "hello"),
                    temp_group_message(&chat_id, "m2", "world"),
                ],
            );
        }
        // Break the database so the archive transaction fails mid-write.
        {
            let conn = app_state.db_conn.lock().expect("db");
            conn.execute("DROP TABLE messages", [])
                .expect("drop messages table");
        }

        let result = archive_temporary_chat(&app_state, &net_state, &chat_id).await;

        assert!(result.is_err(), "archive must fail when persistence fails");
        let temp_state = net_state.temporary_state.lock().await;
        assert!(
            temp_state.chats.contains_key(&chat_id),
            "rollback must preserve the live session"
        );
        assert_eq!(
            temp_state.messages.get(&chat_id).map(|messages| messages.len()),
            Some(2),
            "rollback must preserve the live history"
        );
    }

    #[test]
    fn temporary_group_roster_removals_converge_with_membership_ops() {
        let a_peer = REMOTE_PEER_ID.to_string();
        let b_peer = THIRD_MEMBER_ID.to_string();
        let new_session = || TemporaryChatSession {
            chat_id: "temp-group:550e8400-e29b-41d4-a716-446655440000".to_string(),
            name: "Design Crew".to_string(),
            kind: TemporaryChatKind::Group,
            expires_at: now_unix_secs() + 3600,
            peer_id: Some(a_peer.clone()),
            members: vec![LOCAL_PEER_ID.to_string(), a_peer.clone()],
            member_ops: Vec::new(),
            member_op_winners: HashMap::new(),
            next_member_op_seq: 0,
            archived: false,
        };
        let stale_add = TemporaryMembershipOp {
            issuer: a_peer.clone(),
            seq: 1,
            op: TemporaryMembershipOpKind::Add,
            target: b_peer.clone(),
        };
        let removal = TemporaryMembershipOp {
            issuer: a_peer.clone(),
            seq: 2,
            op: TemporaryMembershipOpKind::Remove,
            target: b_peer.clone(),
        };

        // A adds B, then removes B with a later op.
        let mut session_a = new_session();
        assert!(session_a.apply_membership_ops(&[stale_add.clone()]));
        assert!(session_a.is_member(&b_peer));
        assert!(session_a.apply_membership_ops(&[removal.clone()]));
        assert!(!session_a.is_member(&b_peer));

        // A stale handshake re-announcing B (union-only merging would
        // resurrect them) must be rejected because the remove is newer.
        assert!(!session_a.apply_membership_ops(&[stale_add.clone()]));
        assert!(!session_a.is_member(&b_peer));

        // A peer that only ever saw the stale add converges when it learns the
        // removal, and its reply cannot resurrect B on A either.
        let mut session_b = new_session();
        assert!(session_b.apply_membership_ops(&[stale_add.clone()]));
        assert!(session_b.is_member(&b_peer));
        assert!(session_b.apply_membership_ops(&[removal]));
        assert!(!session_b.is_member(&b_peer));
        assert!(!session_a.apply_membership_ops(&session_b.member_ops));
        assert!(!session_a.is_member(&b_peer));
    }

    #[test]
    fn temporary_group_members_reject_invalid_entries_and_enforce_cap() {
        let mut session = TemporaryChatSession {
            chat_id: "temp-group:550e8400-e29b-41d4-a716-446655440000".to_string(),
            name: "Design Crew".to_string(),
            kind: TemporaryChatKind::Group,
            expires_at: now_unix_secs() + 3600,
            peer_id: Some(REMOTE_PEER_ID.to_string()),
            members: vec![LOCAL_PEER_ID.to_string()],
            member_ops: Vec::new(),
            member_op_winners: HashMap::new(),
            next_member_op_seq: 0,
            archived: false,
        };

        // Invalid entries are rejected outright.
        assert!(!session.add_member("not-a-peer-id"));
        assert!(!session.add_member(""));
        assert!(!session.issue_membership_op(
            REMOTE_PEER_ID,
            TemporaryMembershipOpKind::Add,
            "not-a-peer-id",
        ));

        // Fill up to the cap with valid peer ids.
        while session.members.len() < crate::app_state::TEMP_GROUP_MAX_MEMBERS {
            let peer = libp2p::identity::Keypair::generate_ed25519()
                .public()
                .to_peer_id()
                .to_string();
            assert!(session.add_member(&peer));
        }
        let extra = libp2p::identity::Keypair::generate_ed25519()
            .public()
            .to_peer_id()
            .to_string();
        assert!(
            !session.add_member(&extra),
            "roster must be bounded at the member cap"
        );
        assert_eq!(
            session.members.len(),
            crate::app_state::TEMP_GROUP_MAX_MEMBERS
        );
    }

}
