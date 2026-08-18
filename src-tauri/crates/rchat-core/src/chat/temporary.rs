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
                member_op_winners: HashMap::new(),
                next_member_op_counter: 0,
                archived: false,
                pending_send_count: 0,
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
            member_op_winners: HashMap::new(),
            next_member_op_counter: 0,
            archived: false,
            pending_send_count: 0,
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
/// The message is reserved in the in-memory history under the lock and before
/// any network dispatch: an archiving session is rejected up front, so the
/// reservation check can never happen after the remote peer already received
/// the message. If the publish then fails (closed command channel), the
/// pending message is rolled back so retrying cannot duplicate it.
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

    // Reserve a slot in the live history AND an in-flight send token under
    // the lock and before any network dispatch: an archiving session is
    // rejected here, so the reservation check can never happen after the
    // remote peer has already received the message, and the token keeps
    // archive from snapshotting a message whose delivery is still unresolved.
    // The token is released once dispatch resolves, rolling the pending
    // message back on failure so a closed command channel cannot leave a
    // phantom `delivered` message behind.
    {
        let mut temp_state = net_state.temporary_state.lock().await;
        let reserved = temp_state
            .chats
            .get(chat_id)
            .map(|session| session.archived)
            .unwrap_or(true);
        if reserved {
            return Err(anyhow!(
                "Temporary group chat is being archived: {chat_id}"
            ));
        }
        if let Some(session) = temp_state.chats.get_mut(chat_id) {
            session.pending_send_count = session
                .pending_send_count
                .checked_add(1)
                .ok_or_else(|| anyhow!("too many concurrent temporary-group sends"))?;
        }
        temp_state
            .messages
            .entry(chat_id.to_string())
            .or_default()
            .push(outgoing.clone());
    }

    let tx = net_state.sender.lock().await;
    let dispatch = tx.send(NetworkCommand::PublishGroup { envelope }).await;
    drop(tx);

    // Resolve the in-flight send token under the lock and roll the message
    // back when dispatch failed. The session may already be gone (leave ran
    // concurrently); the token is then moot.
    {
        let mut temp_state = net_state.temporary_state.lock().await;
        if let Some(session) = temp_state.chats.get_mut(chat_id) {
            session.pending_send_count = session.pending_send_count.saturating_sub(1);
        }
        if let Err(error) = dispatch {
            if let Some(messages) = temp_state.messages.get_mut(chat_id) {
                messages.retain(|message| message.id != msg_id);
            }
            return Err(anyhow!("network command channel is closed: {error}"));
        }
    }

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
/// Records a signed remove tombstone for the local peer, clears the active
/// invite if it points at this chat, and tells the network manager to end the
/// temporary session. The manager broadcasts the updated membership state to
/// the other members and then removes the session; leaving is group-scoped, so
/// shared libp2p connections to the other members are never closed.
pub async fn leave_temporary_group(
    app_state: &AppState,
    net_state: &NetworkState,
    chat_id: &str,
) -> Result<()> {
    validate_temp_group_session(net_state, chat_id).await?;

    let signer = crate::chat::group::load_or_create_local_keypair(app_state).await?;
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
                &signer,
            )?;
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
        farewell_winners: None,
    })
    .await
    .map_err(|error| anyhow!("network command channel is closed: {error}"))?;
    Ok(())
}

/// Archive a temporary chat (direct or group) into the durable database.
///
/// The session's `archived` flag is used as an in-progress reservation: under
/// the temporary-state lock the archive rejects duplicate attempts, marks the
/// session as reserved, and snapshots it; every send/receive path rejects or
/// drops writes to a reserved session, so no messages can slip in after the
/// snapshot. The archive is persisted inside a single transaction and only
/// after the commit succeeds is the reservation converted into removal of the
/// live session. A write failure rolls the transaction back and clears the
/// reservation, preserving the live conversation. The complete member roster
/// of group sessions is preserved as chat members, with the resolved local
/// peer id excluded so the archive never stores duplicate local identities.
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

    let local_peer_id = net_state.local_peer_id.lock().await.clone();
    let signer = crate::chat::group::load_or_create_local_keypair(app_state).await?;

    // Reserve the session under the lock, reject duplicate attempts and
    // in-flight sends, and snapshot the messages. The farewell remove
    // tombstone is built on a *clone* of the session so the live membership
    // state is never mutated: a failed archive only clears the reservation and
    // the live conversation (roster included) is exactly as it was. Incoming
    // messages received while the reservation is held stay in the live history
    // and are drained into the archive after the main transaction commits.
    let (session, messages, farewell_winners) = {
        let mut temp_state = net_state.temporary_state.lock().await;
        let messages = temp_state
            .messages
            .get(chat_id)
            .cloned()
            .unwrap_or_default();
        let Some(session) = temp_state.chats.get_mut(chat_id) else {
            return Err(anyhow!("Temporary chat not found"));
        };
        if session.archived {
            return Err(anyhow!(
                "Temporary chat is already being archived: {chat_id}"
            ));
        }
        if session.pending_send_count > 0 {
            return Err(anyhow!(
                "Temporary chat has sends in flight; retry archiving: {chat_id}"
            ));
        }
        if messages.is_empty() {
            return Err(anyhow!("No temporary messages to archive"));
        }
        // Build the farewell roster on a clone so the live session's
        // membership is untouched by archiving: a failed archive preserves the
        // conversation exactly, and the local member is never removed from a
        // group that survives.
        let farewell_winners = if let Some(local) = local_peer_id.as_deref() {
            let mut clone = session.clone();
            clone.issue_membership_op(
                local,
                TemporaryMembershipOpKind::Remove,
                local,
                &signer,
            )?;
            clone.membership_winners()
        } else {
            session.membership_winners()
        };
        session.archived = true;
        (session.clone(), messages, farewell_winners)
    };

    // Resolve the roster so the local identity is stored exactly once: the
    // literal "Me" row plus every remote member (never the local peer id).
    let remote_members = session.remote_members(local_peer_id.as_deref());

    // Ids of the authoritative snapshot, used to distinguish messages that
    // arrive while persistence runs (and must be drained into the archive)
    // from the ones already persisted.
    let snapshot_ids: std::collections::HashSet<String> =
        messages.iter().map(|msg| msg.id.clone()).collect();

    // Persist the archive in one transaction. Every statement's error is
    // propagated so a real failure rolls back the entire archive instead of
    // committing an incomplete one.
    let persist_result = (|| -> Result<()> {
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
        crate::storage::db::add_chat_member(&tx, &archive_chat_id, "Me", "member")?;
        // Preserve the complete member roster of group sessions, excluding
        // the resolved local peer id (already represented by "Me").
        for member in &remote_members {
            if !crate::storage::db::is_peer(&tx, member) {
                crate::storage::db::add_peer(&tx, member, None, None, "archived")?;
            }
            crate::storage::db::add_chat_member(&tx, &archive_chat_id, member, "member")?;
        }

        persist_archive_messages(&tx, &archive_chat_id, messages)?;

        crate::storage::db::assign_chat_to_envelope(&tx, &archive_chat_id, Some("archived"))?;
        tx.commit()
            .map_err(|error| anyhow!("failed to commit archive transaction: {error}"))?;
        Ok(())
    })();

    if let Err(error) = persist_result {
        // The transaction rolled back; release the reservation so the live
        // session stays fully usable. Membership was never mutated, so only
        // the reservation needs clearing.
        let mut temp_state = net_state.temporary_state.lock().await;
        if let Some(session) = temp_state.chats.get_mut(chat_id) {
            session.archived = false;
        }
        return Err(error);
    }

    // Drain messages that arrived while persistence ran: peers do not know we
    // left yet, so their sends must not vanish. The drain and the session
    // removal happen under one lock so nothing can be appended in between;
    // anything that races in after sees no session and is dropped only after
    // the leave boundary established by the farewell queued below.
    let buffered_tail = {
        let mut temp_state = net_state.temporary_state.lock().await;
        drain_post_snapshot_tail(&mut temp_state, chat_id, &snapshot_ids)
    };

    // The leave boundary: enqueue the farewell broadcast first so anything
    // dropped from this point on is strictly post-leave. The winners are
    // carried in the command because the handler cannot read them from the
    // (already removed) session.
    let tx = net_state.sender.lock().await;
    let _ = tx
        .send(NetworkCommand::EndTemporarySession {
            chat_id: chat_id.to_string(),
            farewell_winners: Some(farewell_winners),
        })
        .await;
    drop(tx);

    // Persist the buffered tail best-effort: the authoritative snapshot
    // already committed, and a write failure here only loses the small tail
    // (logged, never silent).
    if !buffered_tail.is_empty() {
        let tail_result = (|| -> Result<()> {
            let mut conn = app_state
                .db_conn
                .lock()
                .map_err(|error| anyhow!("database lock failed: {error}"))?;
            let tx = conn
                .transaction()
                .map_err(|error| anyhow!("failed to begin archive tail transaction: {error}"))?;
            persist_archive_messages(&tx, &archive_chat_id, buffered_tail)?;
            tx.commit()
                .map_err(|error| anyhow!("failed to commit archive tail transaction: {error}"))
        })();
        if let Err(error) = tail_result {
            eprintln!(
                "[Archive] failed to persist {chat_id} buffered tail into {archive_chat_id}: {error}"
            );
        }
    }

    Ok(ArchivedTemporaryChat {
        chat_id: archive_chat_id,
        name: session.name,
    })
}

/// Persist a set of messages into an archive chat, rewriting their chat id
/// and marking them read, and registering any remote peer that owns them.
///
/// Shared by the main archive transaction and the best-effort drain of the
/// messages that arrived while persistence ran, so both write through the
/// exact same transformation.
fn persist_archive_messages(
    conn: &rusqlite::Connection,
    archive_chat_id: &str,
    messages: Vec<Message>,
) -> Result<()> {
    for (idx, mut msg) in messages.into_iter().enumerate() {
        msg.id = format!("{}-{}", msg.id, idx);
        msg.chat_id = archive_chat_id.to_string();
        msg.status = "read".to_string();

        if msg.peer_id != "Me" && !crate::storage::db::is_peer(conn, &msg.peer_id) {
            crate::storage::db::add_peer(conn, &msg.peer_id, None, None, "archived")?;
        }

        if let Some(file_hash) = &msg.file_hash {
            let file_exists: bool = conn
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

        crate::storage::db::insert_message(conn, &msg)?;
    }
    Ok(())
}

/// Extract and remove the messages that arrived after an archive snapshot,
/// and tear the live session down with them, all under one lock so nothing
/// can be appended in between. Messages appended after this point see no
/// session and are dropped at the leave boundary.
fn drain_post_snapshot_tail(
    temp_state: &mut crate::app_state::TemporaryRuntimeState,
    chat_id: &str,
    snapshot_ids: &std::collections::HashSet<String>,
) -> Vec<Message> {
    let live = temp_state
        .messages
        .get(chat_id)
        .cloned()
        .unwrap_or_default();
    let tail = live
        .into_iter()
        .filter(|msg| !snapshot_ids.contains(&msg.id))
        .collect();
    temp_state.chats.remove(chat_id);
    temp_state.messages.remove(chat_id);
    tail
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
                member_op_winners: HashMap::new(),
                next_member_op_counter: 0,
                archived,
                pending_send_count: 0,
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
                member_op_winners: HashMap::new(),
                next_member_op_counter: 0,
                archived: false,
                pending_send_count: 0,
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
            member_op_winners: HashMap::new(),
            next_member_op_counter: 0,
            archived: false,
            pending_send_count: 0,
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
        let (_temp, app_state) = test_app_state().await;
        let (net_state, mut rx) = test_network_state();
        // The local peer id in the live session must match the config keypair
        // that signs the leave tombstone, exactly as in production.
        let keypair = crate::chat::group::load_or_create_local_keypair(&app_state)
            .await
            .expect("keypair");
        let local_peer_id = libp2p::PeerId::from_public_key(&keypair.public()).to_string();
        *net_state.local_peer_id.lock().await = Some(local_peer_id.clone());
        let chat_id = crate::chat_kind::generate_temp_group_chat_id();
        insert_temp_group_session(&net_state, &chat_id, now_unix_secs() + 3600, false).await;

        leave_temporary_group(&app_state, &net_state, &chat_id)
            .await
            .expect("leave");

        // The session is retained for the manager to clean up, but now carries
        // a signed remove tombstone for the local peer so the exit propagates
        // group-wide instead of closing shared transport connections.
        let session = net_state
            .temporary_state
            .lock()
            .await
            .chats
            .get(&chat_id)
            .cloned()
            .expect("session retained for manager cleanup");
        let tombstone = session
            .membership_winners()
            .iter()
            .find(|op| {
                op.op == TemporaryMembershipOpKind::Remove && op.target == local_peer_id
            })
            .cloned()
            .expect("remove tombstone recorded");
        assert!(
            tombstone.verify(),
            "the leave tombstone must be signed so it propagates"
        );
        assert!(!session.is_member(&local_peer_id));

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
        let (net_state, mut rx) = test_network_state();
        // Align the local peer id with the config keypair (as in production)
        // so the farewell tombstone is signed by the identity it claims.
        let keypair = crate::chat::group::load_or_create_local_keypair(&app_state)
            .await
            .expect("keypair");
        let local_peer_id = libp2p::PeerId::from_public_key(&keypair.public()).to_string();
        *net_state.local_peer_id.lock().await = Some(local_peer_id.clone());
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
                        local_peer_id.clone(),
                        REMOTE_PEER_ID.to_string(),
                        THIRD_MEMBER_ID.to_string(),
                    ],
                    member_op_winners: HashMap::new(),
                    next_member_op_counter: 0,
                    archived: false,
                    pending_send_count: 0,
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

        // The farewell command must carry the signed remove tombstone for the
        // local peer so remaining members drop the archiving member even
        // though the shared libp2p connections stay open.
        let farewell = loop {
            match rx.try_recv() {
                Ok(NetworkCommand::EndTemporarySession {
                    farewell_winners: Some(winners),
                    ..
                }) => break winners,
                Ok(_) => continue,
                Err(_) => panic!("no farewell EndTemporarySession command queued"),
            }
        };
        let tombstone = farewell
            .iter()
            .find(|op| {
                op.op == TemporaryMembershipOpKind::Remove && op.target == local_peer_id
            })
            .expect("farewell must include the local remove tombstone");
        assert!(tombstone.verify(), "farewell tombstone must be signed");

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
        assert!(!members.contains(&local_peer_id));

        let archived_messages = crate::storage::db::get_messages(&conn, &archived.chat_id)
            .expect("archived history");
        assert_eq!(archived_messages.len(), 2);
        assert!(archived_messages.iter().all(|message| message.status == "read"));
    }

    #[tokio::test]
    async fn archive_failure_preserves_live_session() {
        let (_temp, app_state) = crate::testing::test_app_state().await;
        let (net_state, _rx) = test_network_state();
        // Align the local peer id with the config keypair (as in production)
        // so the farewell remove tombstone would be issued against the live
        // session if archiving mutated it.
        let keypair = crate::chat::group::load_or_create_local_keypair(&app_state)
            .await
            .expect("keypair");
        let local_peer_id = libp2p::PeerId::from_public_key(&keypair.public()).to_string();
        *net_state.local_peer_id.lock().await = Some(local_peer_id.clone());
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
                    members: vec![local_peer_id.clone(), REMOTE_PEER_ID.to_string()],
                    member_op_winners: HashMap::new(),
                    next_member_op_counter: 0,
                    archived: false,
                    pending_send_count: 0,
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
        let session = temp_state.chats.get(&chat_id).expect("session kept");
        assert!(
            temp_state.chats.contains_key(&chat_id),
            "rollback must preserve the live session"
        );
        assert_eq!(
            temp_state.messages.get(&chat_id).map(|messages| messages.len()),
            Some(2),
            "rollback must preserve the live history"
        );
        assert!(
            !session.archived,
            "failure must release the archive reservation"
        );
        // The farewell tombstone is built on a clone, so the live roster is
        // never mutated by archiving: a failed archive leaves the local member
        // present and the winner/counter state untouched.
        assert!(
            session.is_member(&local_peer_id),
            "rollback must keep the local member on the roster"
        );
        assert!(
            session.is_member(REMOTE_PEER_ID),
            "rollback must keep the remote member on the roster"
        );
        assert!(
            session.member_op_winners.is_empty(),
            "rollback must not leave membership ops behind"
        );
        assert_eq!(
            session.next_member_op_counter, 0,
            "rollback must not advance the membership counter"
        );
    }

    #[tokio::test]
    async fn archive_rejects_duplicate_attempts_while_reserved() {
        let (_temp, app_state) = test_app_state().await;
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
                    member_op_winners: HashMap::new(),
                    next_member_op_counter: 0,
                    // Simulate an in-progress archive: the reservation is set
                    // and the snapshot is being persisted.
                    archived: true,
                    pending_send_count: 0,
                },
            );
            temp_state.messages.insert(
                chat_id.clone(),
                vec![temp_group_message(&chat_id, "m1", "hello")],
            );
        }

        // A second archive attempt while the session is reserved is rejected
        // and must not disturb the reserved session.
        let result = archive_temporary_chat(&app_state, &net_state, &chat_id).await;
        assert!(
            result.expect_err("duplicate archive").to_string().contains("already being archived"),
            "duplicate archive attempts must be rejected"
        );
        let temp_state = net_state.temporary_state.lock().await;
        assert!(
            temp_state.chats.contains_key(&chat_id),
            "a rejected duplicate must leave the reserved session in place"
        );
        assert_eq!(
            temp_state.messages.get(&chat_id).map(|messages| messages.len()),
            Some(1),
            "a rejected duplicate must not disturb the reserved history"
        );
    }

    #[tokio::test]
    async fn archive_error_in_member_writes_rolls_back_and_releases_reservation() {
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
                    member_op_winners: HashMap::new(),
                    next_member_op_counter: 0,
                    archived: false,
                    pending_send_count: 0,
                },
            );
            temp_state.messages.insert(
                chat_id.clone(),
                vec![temp_group_message(&chat_id, "m1", "hello")],
            );
        }
        // Break the chat_peers table so `add_chat_member` fails mid-transaction:
        // the archive must roll back instead of committing an incomplete one.
        {
            let conn = app_state.db_conn.lock().expect("db");
            conn.execute("DROP TABLE chat_peers", [])
                .expect("drop chat_peers table");
        }

        let result = archive_temporary_chat(&app_state, &net_state, &chat_id).await;

        assert!(
            result.is_err(),
            "a member/peer write failure must fail the whole archive"
        );
        let temp_state = net_state.temporary_state.lock().await;
        let session = temp_state.chats.get(&chat_id).expect("live session kept");
        assert!(
            !session.archived,
            "rollback must release the reservation so the session stays usable"
        );
        assert_eq!(
            temp_state.messages.get(&chat_id).map(|messages| messages.len()),
            Some(1),
            "rollback must preserve the live history"
        );
        drop(temp_state);
        // No partial archive rows may remain: the chats table must not contain
        // the archive id.
        let conn = app_state.db_conn.lock().expect("db");
        let leaked: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM chats WHERE id LIKE ?1)",
                [format!("archived:{}:%", chat_id)],
                |row| row.get::<_, bool>(0),
            )
            .unwrap_or(false);
        assert!(!leaked, "rollback must leave no partial archive rows");
    }

    #[tokio::test]
    async fn temporary_group_archive_refuses_while_send_in_flight() {
        let (_temp, app_state) = test_app_state().await;
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
                    member_op_winners: HashMap::new(),
                    next_member_op_counter: 0,
                    archived: false,
                    // A send is still being dispatched; archive must refuse to
                    // reserve so it can never snapshot a message whose delivery
                    // is unresolved.
                    pending_send_count: 1,
                },
            );
            temp_state.messages.insert(
                chat_id.clone(),
                vec![temp_group_message(&chat_id, "m1", "hello")],
            );
        }

        let result = archive_temporary_chat(&app_state, &net_state, &chat_id).await;
        assert!(
            result
                .expect_err("archive with in-flight send")
                .to_string()
                .contains("sends in flight"),
            "archive must refuse to reserve while a send is in flight"
        );
        let session = net_state.temporary_state.lock().await;
        assert!(
            !session.chats.get(&chat_id).expect("session").archived,
            "a refused archive must not reserve the session"
        );
        assert_eq!(
            session.messages.get(&chat_id).map(|messages| messages.len()),
            Some(1),
            "a refused archive must not disturb the live history"
        );
    }

    #[tokio::test]
    async fn temporary_group_text_send_releases_token_on_success_and_failure() {
        let (_temp, app_state) = test_app_state().await;
        let (net_state, mut rx) = test_network_state();
        let chat_id = crate::chat_kind::generate_temp_group_chat_id();
        insert_temp_group_session(&net_state, &chat_id, now_unix_secs() + 3600, false).await;

        send_temporary_group_text(&app_state, &net_state, &chat_id, "hello")
            .await
            .expect("send");
        let _ = rx.recv().await.expect("command");
        let session = net_state.temporary_state.lock().await;
        assert_eq!(
            session.chats.get(&chat_id).expect("session").pending_send_count,
            0,
            "a successful send must release its in-flight token"
        );
        drop(session);

        // A second send whose dispatch fails (command channel closed) must
        // release the token too and leave no phantom message behind.
        drop(rx);
        let result = send_temporary_group_text(&app_state, &net_state, &chat_id, "world").await;
        assert!(result.is_err());
        let temp_state = net_state.temporary_state.lock().await;
        assert_eq!(
            temp_state.chats.get(&chat_id).expect("session").pending_send_count,
            0,
            "a failed send must release its in-flight token"
        );
        assert_eq!(
            temp_state.messages.get(&chat_id).map(|messages| messages.len()),
            Some(1),
            "a failed send must not leave a phantom delivered message"
        );
    }

    #[test]
    fn temporary_group_archive_drains_buffered_tail() {
        let chat_id = crate::chat_kind::generate_temp_group_chat_id();
        let mut temp_state = crate::app_state::TemporaryRuntimeState::default();
        temp_state.chats.insert(
            chat_id.clone(),
            TemporaryChatSession {
                chat_id: chat_id.clone(),
                name: "Design Crew".to_string(),
                kind: TemporaryChatKind::Group,
                expires_at: now_unix_secs() + 3600,
                peer_id: Some(REMOTE_PEER_ID.to_string()),
                members: vec![LOCAL_PEER_ID.to_string()],
                member_op_winners: HashMap::new(),
                next_member_op_counter: 0,
                archived: true,
                pending_send_count: 0,
            },
        );
        temp_state.messages.insert(
            chat_id.clone(),
            vec![
                temp_group_message(&chat_id, "m1", "snapshot"),
                temp_group_message(&chat_id, "m2", "arrived during persistence"),
                temp_group_message(&chat_id, "m3", "also arrived"),
            ],
        );
        let snapshot_ids = std::collections::HashSet::from(["m1".to_string()]);

        let tail = drain_post_snapshot_tail(&mut temp_state, &chat_id, &snapshot_ids);

        // The post-snapshot arrivals are drained (never silently dropped) and
        // the live session is removed atomically with them.
        assert_eq!(tail.len(), 2);
        assert!(tail.iter().all(|msg| msg.id != "m1"));
        assert!(tail.iter().any(|msg| msg.id == "m2"));
        assert!(tail.iter().any(|msg| msg.id == "m3"));
        assert!(temp_state.chats.is_empty());
        assert!(temp_state.messages.is_empty());
    }

    #[test]
    fn temporary_group_issue_requires_signer_and_propagates_errors() {
        let local_keypair = libp2p::identity::Keypair::generate_ed25519();
        let local_peer = libp2p::PeerId::from_public_key(&local_keypair.public()).to_string();
        let other_keypair = libp2p::identity::Keypair::generate_ed25519();
        let target = THIRD_MEMBER_ID.to_string();
        let mut session = TemporaryChatSession {
            chat_id: "temp-group:550e8400-e29b-41d4-a716-446655440000".to_string(),
            name: "Design Crew".to_string(),
            kind: TemporaryChatKind::Group,
            expires_at: now_unix_secs() + 3600,
            peer_id: Some(local_peer.clone()),
            members: vec![local_peer.clone()],
            member_op_winners: HashMap::new(),
            next_member_op_counter: 0,
            archived: false,
            pending_send_count: 0,
        };

        // A keypair that does not match the actor cannot sign the op: it must
        // fail loudly instead of mutating the roster with an op every remote
        // peer would reject.
        let error = session
            .issue_membership_op(
                &local_peer,
                TemporaryMembershipOpKind::Add,
                &target,
                &other_keypair,
            )
            .expect_err("mismatched keypair must fail");
        assert!(
            error.to_string().contains("does not match actor"),
            "signing failure must be surfaced"
        );
        assert!(!session.is_member(&target));
        assert!(session.member_op_winners.is_empty());

        // The matching keypair signs and applies the op.
        assert!(session
            .issue_membership_op(
                &local_peer,
                TemporaryMembershipOpKind::Add,
                &target,
                &local_keypair,
            )
            .expect("matching keypair must sign"));
        assert!(session.is_member(&target));
        let winner = session.member_op_winners.get(&target).expect("winner");
        assert!(winner.verify(), "the issued op must be signed");
    }

    fn signed_op(
        keypair: &libp2p::identity::Keypair,
        counter: u64,
        op: TemporaryMembershipOpKind,
        target: &str,
    ) -> TemporaryMembershipOp {
        let mut op = TemporaryMembershipOp {
            actor: libp2p::PeerId::from_public_key(&keypair.public()).to_string(),
            counter,
            op,
            target: target.to_string(),
            public_key_b64: String::new(),
            signature_b64: String::new(),
        };
        assert!(op.sign(keypair), "test op must sign");
        op
    }

    #[test]
    fn temporary_group_roster_removals_converge_with_membership_ops() {
        let a_keypair = libp2p::identity::Keypair::generate_ed25519();
        let a_peer = libp2p::PeerId::from_public_key(&a_keypair.public()).to_string();
        let b_peer = THIRD_MEMBER_ID.to_string();
        let new_session = || TemporaryChatSession {
            chat_id: "temp-group:550e8400-e29b-41d4-a716-446655440000".to_string(),
            name: "Design Crew".to_string(),
            kind: TemporaryChatKind::Group,
            expires_at: now_unix_secs() + 3600,
            peer_id: Some(a_peer.clone()),
            members: vec![LOCAL_PEER_ID.to_string(), a_peer.clone()],
            member_op_winners: HashMap::new(),
            next_member_op_counter: 0,
            archived: false,
            pending_send_count: 0,
        };
        let stale_add = signed_op(&a_keypair, 1, TemporaryMembershipOpKind::Add, &b_peer);
        let removal = signed_op(&a_keypair, 2, TemporaryMembershipOpKind::Remove, &b_peer);

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
        // removal, and its reply cannot resurrect B on A either: merging the
        // winner snapshot is last-writer-wins per target.
        let mut session_b = new_session();
        assert!(session_b.apply_membership_ops(&[stale_add.clone()]));
        assert!(session_b.is_member(&b_peer));
        assert!(session_b.apply_membership_ops(&[removal]));
        assert!(!session_b.is_member(&b_peer));
        assert!(!session_a.apply_membership_ops(&session_b.membership_winners()));
        assert!(!session_a.is_member(&b_peer));
    }

    #[test]
    fn temporary_group_ops_require_valid_signatures() {
        let a_keypair = libp2p::identity::Keypair::generate_ed25519();
        let a_peer = libp2p::PeerId::from_public_key(&a_keypair.public()).to_string();
        let mut session = TemporaryChatSession {
            chat_id: "temp-group:550e8400-e29b-41d4-a716-446655440000".to_string(),
            name: "Design Crew".to_string(),
            kind: TemporaryChatKind::Group,
            expires_at: now_unix_secs() + 3600,
            peer_id: Some(a_peer.clone()),
            members: vec![LOCAL_PEER_ID.to_string()],
            member_op_winners: HashMap::new(),
            next_member_op_counter: 0,
            archived: false,
            pending_send_count: 0,
        };

        // An unsigned op cannot be applied at all.
        let unsigned = TemporaryMembershipOp {
            actor: a_peer.clone(),
            counter: 1,
            op: TemporaryMembershipOpKind::Remove,
            target: LOCAL_PEER_ID.to_string(),
            public_key_b64: String::new(),
            signature_b64: String::new(),
        };
        assert!(!session.apply_membership_ops(&[unsigned]));
        assert!(session.is_member(LOCAL_PEER_ID));
        assert!(session.membership_winners().is_empty());

        // An op claiming to be the local peer but signed by a different key
        // fails verification (its embedded public key derives to a different
        // peer id), even with a huge counter that would otherwise win forever.
        let mut forged = TemporaryMembershipOp {
            actor: LOCAL_PEER_ID.to_string(),
            counter: u64::MAX,
            op: TemporaryMembershipOpKind::Remove,
            target: LOCAL_PEER_ID.to_string(),
            public_key_b64: String::new(),
            signature_b64: String::new(),
        };
        assert!(
            !forged.sign(&a_keypair),
            "signing with a non-matching keypair must fail"
        );
        assert!(!session.apply_membership_ops(&[forged]));
        assert!(session.is_member(LOCAL_PEER_ID));

        // The same op from its real actor is accepted and wins by Lamport
        // ordering without any wall-clock involvement.
        let own_remove = signed_op(&a_keypair, 1, TemporaryMembershipOpKind::Remove, LOCAL_PEER_ID);
        assert!(session.apply_membership_ops(&[own_remove.clone()]));
        assert!(!session.is_member(LOCAL_PEER_ID));

        // A later op for the same target from a different actor beats an
        // earlier one by (counter, actor) comparison — no wall clocks.
        let mut re_add = session.clone();
        let c_keypair = libp2p::identity::Keypair::generate_ed25519();
        let add_from_c = signed_op(&c_keypair, 7, TemporaryMembershipOpKind::Add, LOCAL_PEER_ID);
        assert!(re_add.apply_membership_ops(&[add_from_c.clone()]));
        assert!(re_add.is_member(LOCAL_PEER_ID));
        // And a replay of the older op cannot beat it.
        assert!(!re_add.apply_membership_ops(&[own_remove]));
        assert!(re_add.is_member(LOCAL_PEER_ID));
    }

    #[test]
    fn temporary_group_forwarded_winners_converge_transitively() {
        // A signs its operations; B forwards A's winner snapshot to C. C must
        // accept A's ops even though the immediate sender is B, because the
        // signatures bind the ops to A, not to whoever delivered them.
        let a_keypair = libp2p::identity::Keypair::generate_ed25519();
        let a_peer = libp2p::PeerId::from_public_key(&a_keypair.public()).to_string();
        let b_peer = THIRD_MEMBER_ID.to_string();
        let mut a_session = TemporaryChatSession {
            chat_id: "temp-group:550e8400-e29b-41d4-a716-446655440000".to_string(),
            name: "Design Crew".to_string(),
            kind: TemporaryChatKind::Group,
            expires_at: now_unix_secs() + 3600,
            peer_id: Some(a_peer.clone()),
            members: vec![LOCAL_PEER_ID.to_string(), a_peer.clone()],
            member_op_winners: HashMap::new(),
            next_member_op_counter: 0,
            archived: false,
            pending_send_count: 0,
        };
        // A removes B and re-adds C in its own session.
        a_session.apply_membership_ops(&[signed_op(
            &a_keypair,
            1,
            TemporaryMembershipOpKind::Add,
            &b_peer,
        )]);
        a_session.apply_membership_ops(&[signed_op(
            &a_keypair,
            2,
            TemporaryMembershipOpKind::Remove,
            &b_peer,
        )]);
        assert!(!a_session.is_member(&b_peer));
        let forwarded = a_session.membership_winners();
        assert!(!forwarded.is_empty());

        // C (a fresh peer) applies the snapshot forwarded by B: every op is
        // verified against A's embedded public key, so the roster converges
        // without A ever talking to C directly.
        let mut c_session = TemporaryChatSession {
            chat_id: "temp-group:550e8400-e29b-41d4-a716-446655440000".to_string(),
            name: "Design Crew".to_string(),
            kind: TemporaryChatKind::Group,
            expires_at: now_unix_secs() + 3600,
            peer_id: Some(a_peer.clone()),
            members: vec![a_peer.clone(), b_peer.clone()],
            member_op_winners: HashMap::new(),
            next_member_op_counter: 0,
            archived: false,
            pending_send_count: 0,
        };
        assert!(c_session.apply_membership_ops(&forwarded));
        assert!(
            !c_session.is_member(&b_peer),
            "forwarded removals must converge transitively"
        );
    }

    #[test]
    fn temporary_group_lamport_receive_rule_advances_local_clock() {
        let a_keypair = libp2p::identity::Keypair::generate_ed25519();
        let a_peer = libp2p::PeerId::from_public_key(&a_keypair.public()).to_string();
        let b_peer = THIRD_MEMBER_ID.to_string();
        let mut session = TemporaryChatSession {
            chat_id: "temp-group:550e8400-e29b-41d4-a716-446655440000".to_string(),
            name: "Design Crew".to_string(),
            kind: TemporaryChatKind::Group,
            expires_at: now_unix_secs() + 3600,
            peer_id: Some(a_peer.clone()),
            members: vec![LOCAL_PEER_ID.to_string(), a_peer.clone(), b_peer.clone()],
            member_op_winners: HashMap::new(),
            next_member_op_counter: 0,
            archived: false,
            pending_send_count: 0,
        };

        // A high-counter remote remove arrives: the local clock must advance
        // past it (Lamport receive rule).
        let remote_remove = signed_op(
            &a_keypair,
            1_000,
            TemporaryMembershipOpKind::Remove,
            &b_peer,
        );
        assert!(session.apply_membership_ops(&[remote_remove.clone()]));
        assert!(!session.is_member(&b_peer));

        // The reconnecting peer B re-joins: the local add issued now must
        // carry a counter beyond the received remove, so it wins and B is
        // back on the roster. Without the Lamport rule the add would lose.
        let local_keypair = libp2p::identity::Keypair::generate_ed25519();
        assert!(session
            .issue_membership_op(
                &libp2p::PeerId::from_public_key(&local_keypair.public()).to_string(),
                TemporaryMembershipOpKind::Add,
                &b_peer,
                &local_keypair,
            )
            .expect("issue must sign"));
        assert!(session.is_member(&b_peer));
        let winner = session
            .member_op_winners
            .get(&b_peer)
            .expect("winner recorded");
        assert!(
            winner.counter > remote_remove.counter,
            "causally-later local op must supersede the remote remove"
        );
    }

    #[test]
    fn temporary_group_winners_snapshot_survives_op_churn_without_trimming() {
        let a_keypair = libp2p::identity::Keypair::generate_ed25519();
        let a_peer = libp2p::PeerId::from_public_key(&a_keypair.public()).to_string();
        let mut session = TemporaryChatSession {
            chat_id: "temp-group:550e8400-e29b-41d4-a716-446655440000".to_string(),
            name: "Design Crew".to_string(),
            kind: TemporaryChatKind::Group,
            expires_at: now_unix_secs() + 3600,
            peer_id: Some(a_peer.clone()),
            members: vec![LOCAL_PEER_ID.to_string()],
            member_op_winners: HashMap::new(),
            next_member_op_counter: 0,
            archived: false,
            pending_send_count: 0,
        };

        // Churn a single target well past any log-truncation threshold: the
        // winner snapshot must stay complete (one winning op per target, never
        // trimmed) so a fresh peer can reconstruct the roster.
        let mut last_remove = None;
        for round in 0..(crate::app_state::TEMP_GROUP_MAX_MEMBERSHIP_OPS + 100) {
            let add = signed_op(
                &a_keypair,
                (round * 2) as u64 + 1,
                TemporaryMembershipOpKind::Add,
                LOCAL_PEER_ID,
            );
            let remove = signed_op(
                &a_keypair,
                (round * 2) as u64 + 2,
                TemporaryMembershipOpKind::Remove,
                LOCAL_PEER_ID,
            );
            session.apply_membership_ops(&[add]);
            session.apply_membership_ops(&[remove.clone()]);
            last_remove = Some(remove);
        }

        // The winner for the target is the final remove: the authoritative
        // state survived, and the transferred snapshot reconstructs it.
        let winners = session.membership_winners();
        assert_eq!(winners.len(), 1, "one winning op per target");
        assert_eq!(winners[0].target, LOCAL_PEER_ID);
        assert_eq!(winners[0].op, TemporaryMembershipOpKind::Remove);
        assert_eq!(winners[0], last_remove.expect("last op"));
        assert!(!session.is_member(LOCAL_PEER_ID));

        // A peer that only ever saw an older add (its roster still contains
        // the removed member) converges when given the snapshot, and a truly
        // fresh peer records the winning tombstone so a stale add cannot
        // resurrect the removed member later.
        let mut fresh = TemporaryChatSession {
            chat_id: session.chat_id.clone(),
            name: "Design Crew".to_string(),
            kind: TemporaryChatKind::Group,
            expires_at: now_unix_secs() + 3600,
            peer_id: Some(a_peer.clone()),
            members: vec![a_peer.clone(), LOCAL_PEER_ID.to_string()],
            member_op_winners: HashMap::new(),
            next_member_op_counter: 0,
            archived: false,
            pending_send_count: 0,
        };
        assert!(fresh.apply_membership_ops(&winners));
        assert!(!fresh.is_member(LOCAL_PEER_ID));
        let stale_add = signed_op(&a_keypair, 1, TemporaryMembershipOpKind::Add, LOCAL_PEER_ID);
        assert!(!fresh.apply_membership_ops(&[stale_add]));
        assert!(!fresh.is_member(LOCAL_PEER_ID));
    }

    #[test]
    fn temporary_group_winners_missing_from_detects_stale_snapshots() {
        let a_keypair = libp2p::identity::Keypair::generate_ed25519();
        let a_peer = libp2p::PeerId::from_public_key(&a_keypair.public()).to_string();
        let b_peer = THIRD_MEMBER_ID.to_string();
        let mut session = TemporaryChatSession {
            chat_id: "temp-group:550e8400-e29b-41d4-a716-446655440000".to_string(),
            name: "Design Crew".to_string(),
            kind: TemporaryChatKind::Group,
            expires_at: now_unix_secs() + 3600,
            peer_id: Some(a_peer.clone()),
            members: vec![LOCAL_PEER_ID.to_string(), a_peer.clone()],
            member_op_winners: HashMap::new(),
            next_member_op_counter: 0,
            archived: false,
            pending_send_count: 0,
        };
        let add_b = signed_op(&a_keypair, 1, TemporaryMembershipOpKind::Add, &b_peer);
        session.apply_membership_ops(&[add_b.clone()]);

        // The sender's snapshot lacks B entirely: our winner is missing.
        assert!(session.winners_missing_from(&[]));
        // The sender holds a stale add for B: our winner is newer.
        assert!(session.winners_missing_from(&[signed_op(
            &a_keypair,
            0,
            TemporaryMembershipOpKind::Add,
            &b_peer,
        )]));
        // The sender is fully up to date: nothing missing.
        assert!(!session.winners_missing_from(&session.membership_winners()));
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
            member_op_winners: HashMap::new(),
            next_member_op_counter: 0,
            archived: false,
            pending_send_count: 0,
        };

        // Invalid entries are rejected outright (before any signing).
        assert!(!session.add_member("not-a-peer-id"));
        assert!(!session.add_member(""));
        let keypair = libp2p::identity::Keypair::generate_ed25519();
        assert!(matches!(
            session.issue_membership_op(
                REMOTE_PEER_ID,
                TemporaryMembershipOpKind::Add,
                "not-a-peer-id",
                &keypair,
            ),
            Ok(false)
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
