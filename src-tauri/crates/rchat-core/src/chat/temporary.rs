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
    let (farewell_winners, min_add_counter) = {
        let mut temp_state = net_state.temporary_state.lock().await;
        let session = temp_state
            .chats
            .get_mut(chat_id)
            .ok_or_else(|| anyhow!("Temporary group chat not found: {chat_id}"))?;
        // Reserve the session against new sends atomically with the zero-pending
        // check: every send path rejects an archived/closing session under this
        // same lock, so a send can no longer slip in between the check and the
        // farewell and be torn down under it.
        if session.archived {
            return Err(anyhow!(
                "Temporary chat is already being archived: {chat_id}"
            ));
        }
        // Same finalization precondition as archiving: a send whose dispatch
        // is still unresolved must resolve before the session can be torn
        // down, otherwise the publish could land after the farewell and the
        // send would report success against a deleted history.
        if session.pending_send_count > 0 {
            return Err(anyhow!(
                "Temporary chat has sends in flight; retry leaving: {chat_id}"
            ));
        }
        // Build the farewell remove on a *clone* so the live roster is left
        // untouched if the farewell cannot be queued; the manager removes the
        // session (and the reservation with it) once the farewell goes out.
        let (farewell_winners, min_add_counter) = if let Some(local) = local_peer_id.as_deref() {
            let mut clone = session.clone();
            clone.issue_membership_op(
                local,
                TemporaryMembershipOpKind::Remove,
                local,
                &signer,
            )?;
            (clone.membership_winners(), clone.next_member_op_counter)
        } else {
            (session.membership_winners(), session.next_member_op_counter)
        };
        session.archived = true;
        if temp_state
            .active_invite
            .as_ref()
            .map(|active| active.payload.chat_id == chat_id)
            .unwrap_or(false)
        {
            temp_state.active_invite = None;
        }
        (farewell_winners, min_add_counter)
    };

    let (alive_tx, _alive_rx) = tokio::sync::watch::channel(true);
    let tx = net_state.sender.lock().await;
    if let Err(error) = tx
        .send(NetworkCommand::FreezeTemporaryArchive {
            chat_id: chat_id.to_string(),
            kind: TemporaryChatKind::Group,
            farewell_winners,
            min_add_counter,
            alive: alive_tx.clone(),
            ack: None,
        })
        .await
    {
        // The farewell never went out; clear the closing reservation so the
        // session stays fully usable (membership was never mutated).
        let mut temp_state = net_state.temporary_state.lock().await;
        if let Some(session) = temp_state.chats.get_mut(chat_id) {
            session.archived = false;
        }
        return Err(anyhow!("network command channel is closed: {error}"));
    }
    // Leaving persists nothing, so the freeze is resolved immediately by
    // committing it: the manager tears the session down and emits
    // TemporaryChatEnded. Cancellation before the commit is recovered by the
    // freeze's watchdog, so a leaving caller can never strand a reserved
    // session.
    let _ = tx
        .send(NetworkCommand::CommitTemporaryArchive {
            chat_id: chat_id.to_string(),
        })
        .await;
    Ok(())
}

/// Archive a temporary chat (direct or group) into the durable database.
///
/// The session's `archived` flag is used as an in-progress reservation: under
/// the temporary-state lock the archive rejects duplicate attempts, rejects
/// in-flight sends, and marks the session as reserved. The leave boundary is
/// established by the network manager, which broadcasts the farewell, drains
/// the final message set (everything received up to and during that broadcast)
/// and removes the session in a single event-loop step, acknowledging the
/// messages back. The caller then persists the entire archive — session and
/// messages — inside a single transaction, so there is no separate best-effort
/// tail to lose. A write failure returns an error and restores the live
/// session through the manager, which re-announces a signed add tombstone that
/// outranks the farewell remove, so the conversation and its membership
/// survive an unsuccessful archive. The complete member roster of group
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

    let local_peer_id = net_state.local_peer_id.lock().await.clone();
    let signer = crate::chat::group::load_or_create_local_keypair(app_state).await?;

    // Reserve the session under the lock, rejecting duplicate attempts and
    // in-flight sends. The farewell remove tombstone is built on a *clone* so
    // the live membership state is never mutated by archiving; the session
    // (and its messages) are torn down by the network manager below and
    // restored by it if persistence fails.
    let (session, farewell_winners, min_add_counter) = {
        let mut temp_state = net_state.temporary_state.lock().await;
        if temp_state
            .messages
            .get(chat_id)
            .map(|messages| messages.is_empty())
            .unwrap_or(true)
        {
            return Err(anyhow!("No temporary messages to archive"));
        }
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
        // The farewell remove tombstone is only meaningful for groups (a DM
        // has no roster, and the manager broadcasts winners only for group
        // sessions); mint it on a *clone* so the live membership state is
        // never mutated by archiving. The session (and its messages) are torn
        // down by the network manager below and restored by it if persistence
        // fails.
        let (farewell_winners, min_add_counter) =
            if let (Some(local), TemporaryChatKind::Group) =
                (local_peer_id.as_deref(), session.kind.clone())
            {
                let mut clone = session.clone();
                clone.issue_membership_op(
                    local,
                    TemporaryMembershipOpKind::Remove,
                    local,
                    &signer,
                )?;
                (clone.membership_winners(), clone.next_member_op_counter)
            } else {
                (session.membership_winners(), session.next_member_op_counter)
            };
        session.archived = true;
        (session.clone(), farewell_winners, min_add_counter)
    };

    // Establish the leave boundary through the manager (phase one of the
    // two-phase finalization): it broadcasts the farewell (groups only),
    // drains the final message set into its pending-finalization record and
    // acknowledges the messages back, while retaining the session, routing,
    // subscription and punch target for a later commit or abort. Everything
    // the peers sent before our leave announcement is included; nothing
    // received after it can be, because the frozen session rejects incoming
    // traffic. The caller keeps `alive` held until the freeze is resolved, so
    // a cancellation while awaiting the ack makes the manager's watchdog
    // abort the archive and recover the conversation.
    let (alive_tx, _alive_rx) = tokio::sync::watch::channel(true);
    let (ack_tx, ack_rx) = tokio::sync::oneshot::channel();
    {
        let tx = net_state.sender.lock().await;
        if let Err(error) = tx
            .send(NetworkCommand::FreezeTemporaryArchive {
                chat_id: chat_id.to_string(),
                kind: session.kind.clone(),
                farewell_winners: farewell_winners.clone(),
                min_add_counter,
                alive: alive_tx.clone(),
                ack: Some(ack_tx),
            })
            .await
        {
            // The freeze never reached the manager (no send or ack ever
            // happens); release the reservation so the session stays usable
            // and nothing is left half-torn-down.
            clear_archive_reservation(net_state, chat_id).await;
            return Err(anyhow!("network command channel is closed: {error}"));
        }
    }
    let messages = match ack_rx.await {
        Ok(Ok(messages)) => messages,
        Ok(Err(manager_error)) => {
            // The manager refused the freeze and did not create a pending
            // record; the session is still reserved, so release it.
            clear_archive_reservation(net_state, chat_id).await;
            return Err(anyhow!(
                "network manager refused the archive freeze: {manager_error}"
            ));
        }
        Err(_) => {
            // The manager dropped the acknowledgement without confirming. If
            // the freeze was processed, its watchdog aborts on our `alive`
            // drop and recovers the session; if it never was, the session is
            // still reserved and releasing it here is the safe recovery.
            // Either way the conversation is preserved.
            clear_archive_reservation(net_state, chat_id).await;
            return Err(anyhow!(
                "network manager dropped the archive freeze acknowledgement"
            ));
        }
    };

    // Resolve the roster so the local identity is stored exactly once: the
    // literal "Me" row plus every remote member (never the local peer id).
    let remote_members = session.remote_members(local_peer_id.as_deref());

    // Persist the archive in one transaction. Every statement's error is
    // propagated so a real failure rolls back the entire archive instead of
    // committing an incomplete one.
    let persist_session = session.clone();
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

        let archived_is_group = matches!(persist_session.kind, TemporaryChatKind::Group);
        crate::storage::db::create_chat(
            &tx,
            &archive_chat_id,
            &persist_session.name,
            archived_is_group,
        )?;
        crate::storage::db::add_chat_member(&tx, &archive_chat_id, "Me", "member")?;
        // Preserve the complete member roster of group sessions, excluding
        // the resolved local peer id (already represented by "Me").
        for member in &remote_members {
            if !crate::storage::db::is_peer(&tx, member) {
                crate::storage::db::add_peer(&tx, member, None, None, "archived")?;
            }
            crate::storage::db::add_chat_member(&tx, &archive_chat_id, member, "member")?;
        }

        persist_archive_messages(&tx, &archive_chat_id, messages.clone())?;

        crate::storage::db::assign_chat_to_envelope(&tx, &archive_chat_id, Some("archived"))?;
        tx.commit()
            .map_err(|error| anyhow!("failed to commit archive transaction: {error}"))?;
        Ok(())
    })();

    if let Err(persist_error) = persist_result {
        // The farewell already told the other members we left, so a bare
        // reservation rollback is not enough: abort the freeze through the
        // manager, which clears the reservation, restores the drained messages
        // and routing, and re-announces a rejoin add that outranks the
        // farewell remove, preserving the conversation and its membership
        // exactly. The abort is acknowledged so we only return once it has
        // completed, and the recovery copies are preserved if it can never be
        // queued or acknowledged. If the caller is cancelled while awaiting
        // this, the manager's watchdog performs the same recovery.
        let (abort_ack_tx, abort_ack_rx) = tokio::sync::oneshot::channel();
        let send_error = {
            let tx = net_state.sender.lock().await;
            tx.send(NetworkCommand::AbortTemporaryArchive {
                chat_id: chat_id.to_string(),
                epoch: None,
                ack: Some(abort_ack_tx),
            })
            .await
            .err()
        };
        if let Some(send_error) = send_error {
            // The freeze already drained the session and the manager is gone,
            // so the abort was never processed. Re-insert the recovery state
            // locally rather than dropping the only copies.
            preserve_restore_state(net_state, chat_id, session, messages, min_add_counter).await;
            return Err(anyhow!(
                "archive failed: {persist_error}; recovery not queued: {send_error}"
            ));
        }
        match abort_ack_rx.await {
            Ok(Ok(())) => {
                // The manager fully restored the session (rejoin announced); the
                // caller can surface the archive failure and keep the chat live.
                Err(persist_error)
            }
            Ok(Err(abort_error)) => {
                // The manager preserved the data but could not announce the
                // rejoin; surface that so the caller does not believe the peer
                // rejoined while remote members still treat it as removed.
                Err(anyhow!(
                    "archive failed: {persist_error}; recovery incomplete: {abort_error}"
                ))
            }
            Err(_) => {
                // The manager dropped the acknowledgement without confirming;
                // the session state is unknown, so preserve the recovery state
                // ourselves to avoid losing the conversation.
                preserve_restore_state(net_state, chat_id, session, messages, min_add_counter).await;
                Err(anyhow!(
                    "archive failed: {persist_error}; recovery acknowledgement lost"
                ))
            }
        }
    } else {
        // The archive is durably persisted; commit the freeze (phase two): the
        // manager performs the final teardown and emits TemporaryChatEnded.
        // Fire-and-forget is safe — the pending record guarantees the session
        // is recovered if the commit never arrives.
        let _ = {
            let tx = net_state.sender.lock().await;
            tx.send(NetworkCommand::CommitTemporaryArchive {
                chat_id: chat_id.to_string(),
            })
            .await
        };
        Ok(ArchivedTemporaryChat {
            chat_id: archive_chat_id,
            name: persist_session.name,
        })
    }
}

/// Clear a temporary session's archive reservation, leaving it fully usable
/// again. Used when a freeze can never be (or never was) established, so the
/// conversation is never stranded in a half-torn-down state.
async fn clear_archive_reservation(net_state: &NetworkState, chat_id: &str) {
    let mut temp_state = net_state.temporary_state.lock().await;
    if let Some(session) = temp_state.chats.get_mut(chat_id) {
        session.archived = false;
    }
}

/// Re-insert a temporary session and its messages as a live (non-archived)
/// session so the recovery data survives when the network manager is gone or
/// never acknowledged the restore. The membership counter is advanced past the
/// farewell remove so a later rejoin (or retry) can outrank it.
async fn preserve_restore_state(
    net_state: &NetworkState,
    chat_id: &str,
    mut session: crate::app_state::TemporaryChatSession,
    messages: Vec<crate::storage::db::Message>,
    min_add_counter: u64,
) {
    session.archived = false;
    if session.next_member_op_counter < min_add_counter {
        session.next_member_op_counter = min_add_counter;
    }
    let mut temp_state = net_state.temporary_state.lock().await;
    temp_state.chats.insert(chat_id.to_string(), session);
    temp_state.messages.insert(chat_id.to_string(), messages);
}

/// Persist a set of messages into an archive chat, rewriting their chat id
/// and marking them read, and registering any remote peer that owns them.
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
        BroadcastState, ChatConnectionRuntime, PendingTemporaryFinalization,
        TemporaryMembershipOp, TemporaryRuntimeState, VoiceCallState,
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

    /// Simulate the network manager's two-phase archive finalizer.
    ///
    /// Mirrors `freeze_temporary_archive` (drain the final message set into a
    /// pending-finalization record and acknowledge the messages while keeping
    /// the session reserved, and spawn a watchdog that aborts the freeze if the
    /// caller's `alive` sender is dropped without resolving it),
    /// `commit_temporary_archive` (remove the session and its messages) and
    /// `abort_temporary_archive` (re-insert the session, clear the
    /// reservation, bump the counter past the farewell remove and re-add the
    /// local member with a fresh signed op). `inject_tail` appends a message
    /// right before the drain, as if a peer sent it while the farewell was
    /// being broadcast.
    ///
    /// Watchdogs recover cancelled freezes through a dedicated channel instead
    /// of the command channel, so the simulation loop never holds a command
    /// sender and dropping the test's `NetworkState` still closes the command
    /// channel.
    fn drive_archive_manager(
        temp_state: Arc<tokio::sync::Mutex<TemporaryRuntimeState>>,
        app_state: AppState,
        local_peer_id: Option<String>,
        inject_tail: Option<(String, String)>,
        mut rx: mpsc::Receiver<NetworkCommand>,
    ) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            let (wd_tx, mut wd_rx) = mpsc::channel::<(String, u64)>(8);
            let mut pending: HashMap<String, PendingTemporaryFinalization> = HashMap::new();
            loop {
                tokio::select! {
                    command = rx.recv() => {
                        let Some(command) = command else { break };
                        match command {
                            NetworkCommand::FreezeTemporaryArchive {
                                chat_id,
                                kind,
                                farewell_winners: _,
                                min_add_counter,
                                alive,
                                ack,
                            } => {
                                let mut temp_state = temp_state.lock().await;
                                if let Some((id, text)) = inject_tail.as_ref() {
                                    temp_state
                                        .messages
                                        .entry(chat_id.clone())
                                        .or_default()
                                        .push(temp_group_message(&chat_id, id, text));
                                }
                                let Some(session) = temp_state.chats.get(&chat_id).cloned() else {
                                    if let Some(ack) = ack {
                                        let _ = ack.send(Err(format!(
                                            "temporary chat not found for freeze: {chat_id}"
                                        )));
                                    }
                                    continue;
                                };
                                let messages = temp_state.messages.remove(&chat_id).unwrap_or_default();
                                pending.insert(
                                    chat_id.clone(),
                                    PendingTemporaryFinalization {
                                        epoch: 0,
                                        chat_id: chat_id.clone(),
                                        kind: kind.clone(),
                                        session: session.clone(),
                                        messages: messages.clone(),
                                        routing_peers: Vec::new(),
                                        was_subscribed: false,
                                        punch_target: None,
                                        min_add_counter,
                                    },
                                );
                                // Mirror the manager's watchdog: a dropped
                                // `alive` sender (caller cancelled) recovers
                                // the freeze. Only the receiver crosses into
                                // the task; the arm's copy of the sender is
                                // dropped immediately so the channel closes
                                // the moment the caller drops its sender.
                                let wd_sender = wd_tx.clone();
                                let watchdog_chat = chat_id.clone();
                                let mut alive_rx = alive.subscribe();
                                drop(alive);
                                tokio::spawn(async move {
                                    if alive_rx.changed().await.is_err() {
                                        let _ = wd_sender.send((watchdog_chat, 0)).await;
                                    }
                                });
                                if let Some(ack) = ack {
                                    let _ = ack.send(Ok(messages));
                                }
                            }
                            NetworkCommand::CommitTemporaryArchive { chat_id } => {
                                pending.remove(&chat_id);
                                let mut temp_state = temp_state.lock().await;
                                temp_state.chats.remove(&chat_id);
                                temp_state.messages.remove(&chat_id);
                            }
                            NetworkCommand::AbortTemporaryArchive { chat_id, epoch, ack } => {
                                drive_abort_pending(
                                    temp_state.clone(),
                                    &mut pending,
                                    &chat_id,
                                    epoch,
                                    &app_state,
                                    &local_peer_id,
                                    ack,
                                )
                                .await;
                            }
                            _ => {}
                        }
                    }
                    watchdog = wd_rx.recv() => {
                        let Some((chat_id, epoch)) = watchdog else {
                            continue;
                        };
                        drive_abort_pending(
                            temp_state.clone(),
                            &mut pending,
                            &chat_id,
                            Some(epoch),
                            &app_state,
                            &local_peer_id,
                            None,
                        )
                        .await;
                    }
                }
            }
        })
    }

    /// Recover a frozen temporary session: clears the reservation, restores
    /// the drained messages and re-issues a signed rejoin add (group sessions
    /// only) that outranks the farewell remove. Mirrors the manager's
    /// `abort_temporary_archive`. No-op when no freeze with a matching epoch
    /// is pending.
    async fn drive_abort_pending(
        temp_state: Arc<tokio::sync::Mutex<TemporaryRuntimeState>>,
        pending: &mut HashMap<String, PendingTemporaryFinalization>,
        chat_id: &str,
        epoch: Option<u64>,
        app_state: &AppState,
        local_peer_id: &Option<String>,
        ack: Option<tokio::sync::oneshot::Sender<Result<(), String>>>,
    ) {
        let Some(frozen) = pending.get(chat_id) else {
            if let Some(ack) = ack {
                let _ = ack.send(Ok(()));
            }
            return;
        };
        if let Some(epoch) = epoch {
            if frozen.epoch != epoch {
                if let Some(ack) = ack {
                    let _ = ack.send(Ok(()));
                }
                return;
            }
        }
        let pending = pending.remove(chat_id).expect("the pending entry was just checked");
        let mut temp_state = temp_state.lock().await;
        let mut session = pending.session;
        session.archived = false;
        if session.next_member_op_counter < pending.min_add_counter {
            session.next_member_op_counter = pending.min_add_counter;
        }
        let mut rejoin_error = None;
        if let Some(local) = local_peer_id.as_ref() {
            if matches!(pending.kind, TemporaryChatKind::Group) {
                match crate::chat::group::load_or_create_local_keypair(app_state).await {
                    Ok(keypair) => {
                        if let Err(error) = session.issue_membership_op(
                            local,
                            TemporaryMembershipOpKind::Add,
                            local,
                            &keypair,
                        ) {
                            rejoin_error = Some(error.to_string());
                        }
                    }
                    Err(error) => rejoin_error = Some(format!("no keypair: {error}")),
                }
            }
        }
        temp_state.chats.insert(pending.chat_id.clone(), session);
        temp_state.messages.insert(pending.chat_id, pending.messages);
        if let Some(ack) = ack {
            if let Some(error) = rejoin_error {
                let _ = ack.send(Err(error));
            } else {
                let _ = ack.send(Ok(()));
            }
        }
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

        // The session is retained for the manager to clean up, but is only
        // reserved (archived) here: the farewell remove tombstone is built on a
        // *clone* so a failed command delivery never mutates the live roster.
        // The exit propagates group-wide via the farewell carried on the
        // command, instead of closing shared transport connections.
        let session = net_state
            .temporary_state
            .lock()
            .await
            .chats
            .get(&chat_id)
            .cloned()
            .expect("session retained for manager cleanup");
        assert!(
            session.archived,
            "the session must be reserved against new sends while leaving"
        );

        let mut commands = Vec::new();
        while let Ok(command) = rx.try_recv() {
            commands.push(command);
        }
        let NetworkCommand::FreezeTemporaryArchive {
            kind,
            farewell_winners,
            ..
        } = commands
            .first()
            .expect("leave must enqueue a freeze command")
        else {
            panic!("leave must enqueue a freeze command");
        };
        assert_eq!(
            kind,
            &TemporaryChatKind::Group,
            "leave is group-scoped and must carry the explicit group kind"
        );
        let tombstone = farewell_winners
            .iter()
            .find(|op| {
                op.op == TemporaryMembershipOpKind::Remove && op.target == local_peer_id
            })
            .cloned()
            .expect("remove tombstone recorded in the farewell");
        assert!(
            tombstone.verify(),
            "the leave tombstone must be signed so it propagates"
        );
        assert!(
            commands
                .iter()
                .any(|command| matches!(command, NetworkCommand::CommitTemporaryArchive { .. })),
            "leave resolves the freeze immediately with a commit"
        );
        assert!(
            session.is_member(LOCAL_PEER_ID),
            "the farewell is built on a clone; the retained session must be untouched"
        );
        assert!(
            commands
                .iter()
                .all(|command| !matches!(command, NetworkCommand::DropConnection { .. })),
            "leaving a group must not drop the peer's shared libp2p connections"
        );
    }

    #[tokio::test]
    async fn temporary_group_leave_refuses_while_send_in_flight() {
        let (_temp, app_state) = test_app_state().await;
        let (net_state, _rx) = test_network_state();
        let keypair = crate::chat::group::load_or_create_local_keypair(&app_state)
            .await
            .expect("keypair");
        let local_peer_id = libp2p::PeerId::from_public_key(&keypair.public()).to_string();
        *net_state.local_peer_id.lock().await = Some(local_peer_id.clone());
        let chat_id = crate::chat_kind::generate_temp_group_chat_id();
        insert_temp_group_session(&net_state, &chat_id, now_unix_secs() + 3600, false).await;
        {
            let mut temp_state = net_state.temporary_state.lock().await;
            temp_state
                .chats
                .get_mut(&chat_id)
                .expect("session")
                .pending_send_count = 1;
        }

        // Leaving while a send is unresolved must be rejected just like
        // archiving: the pending publish could otherwise land after the
        // farewell has torn the session down, and the send would report
        // success against a deleted history.
        let error = leave_temporary_group(&app_state, &net_state, &chat_id)
            .await
            .expect_err("leave must be rejected while a send is in flight");
        assert!(
            error.to_string().contains("sends in flight"),
            "the rejection must name the in-flight precondition"
        );
        let temp_state = net_state.temporary_state.lock().await;
        let session = temp_state
            .chats
            .get(&chat_id)
            .expect("session kept");
        assert!(
            session.is_member(LOCAL_PEER_ID),
            "a rejected leave must not remove the local member"
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

        // Witness the freeze command the manager would receive, then play the
        // manager's two-phase finalizer: acknowledge the drained message set
        // so the archive can persist everything in one transaction, then tear
        // the session down when the caller commits the freeze.
        let farewell_witness = std::sync::Arc::new(std::sync::Mutex::new(None));
        let witness = farewell_witness.clone();
        let net = net_state.clone();
        let driver = tokio::spawn(async move {
            let (chat_id, kind, farewell_winners, ack) = match rx
                .recv()
                .await
                .expect("the archive freeze command must arrive")
            {
                NetworkCommand::FreezeTemporaryArchive {
                    chat_id,
                    kind,
                    farewell_winners,
                    ack,
                    ..
                } => (chat_id, kind, farewell_winners, ack),
                other => panic!("unexpected command: {other:?}"),
            };
            assert_eq!(
                kind,
                TemporaryChatKind::Group,
                "a group archive must carry the explicit group kind"
            );
            *witness.lock().unwrap() = Some(farewell_winners);
            let messages = {
                let mut temp_state = net.temporary_state.lock().await;
                temp_state.messages.remove(&chat_id).unwrap_or_default()
            };
            if let Some(ack) = ack {
                let _ = ack.send(Ok(messages));
            }
            match rx
                .recv()
                .await
                .expect("the archive commit command must arrive")
            {
                NetworkCommand::CommitTemporaryArchive {
                    chat_id: committed,
                } => {
                    assert_eq!(committed, chat_id);
                    let mut temp_state = net.temporary_state.lock().await;
                    temp_state.chats.remove(&chat_id);
                    temp_state.messages.remove(&chat_id);
                }
                other => panic!("unexpected command: {other:?}"),
            }
        });

        let archived = archive_temporary_chat(&app_state, &net_state, &chat_id)
            .await
            .expect("archive");
        driver.await.expect("driver");

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
        let farewell = farewell_witness.lock().unwrap();
        let farewell = farewell.as_ref().expect("farewell command was witnessed");
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
    async fn archive_temporary_dm_carries_dm_kind_and_no_group_farewell() {
        let (_temp, app_state) = crate::testing::test_app_state().await;
        let (net_state, mut rx) = test_network_state();
        let chat_id = crate::chat_kind::generate_temp_direct_chat_id();
        {
            let mut temp_state = net_state.temporary_state.lock().await;
            temp_state.chats.insert(
                chat_id.clone(),
                TemporaryChatSession {
                    chat_id: chat_id.clone(),
                    name: crate::chat_kind::default_temp_direct_name(&chat_id),
                    kind: TemporaryChatKind::Dm,
                    expires_at: now_unix_secs() + 3600,
                    peer_id: Some(REMOTE_PEER_ID.to_string()),
                    members: Vec::new(),
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

        // Play the manager's two-phase finalizer and witness the freeze: a DM
        // archive must carry the explicit DM kind with no group membership
        // winners, so the manager never broadcasts a group-style roster
        // handshake to the peer.
        let net = net_state.clone();
        let driver = tokio::spawn(async move {
            let (chat_id, kind, farewell_winners, ack) = match rx
                .recv()
                .await
                .expect("the archive freeze command must arrive")
            {
                NetworkCommand::FreezeTemporaryArchive {
                    chat_id,
                    kind,
                    farewell_winners,
                    ack,
                    ..
                } => (chat_id, kind, farewell_winners, ack),
                other => panic!("unexpected command: {other:?}"),
            };
            assert_eq!(
                kind,
                TemporaryChatKind::Dm,
                "a DM archive must carry the DM kind"
            );
            assert!(
                farewell_winners.is_empty(),
                "a DM archive must not mint group membership winners"
            );
            let messages = {
                let mut temp_state = net.temporary_state.lock().await;
                temp_state.messages.remove(&chat_id).unwrap_or_default()
            };
            if let Some(ack) = ack {
                let _ = ack.send(Ok(messages));
            }
            match rx
                .recv()
                .await
                .expect("the archive commit command must arrive")
            {
                NetworkCommand::CommitTemporaryArchive {
                    chat_id: committed,
                } => {
                    assert_eq!(committed, chat_id);
                    let mut temp_state = net.temporary_state.lock().await;
                    temp_state.chats.remove(&chat_id);
                    temp_state.messages.remove(&chat_id);
                }
                other => panic!("unexpected command: {other:?}"),
            }
        });

        let archived = archive_temporary_chat(&app_state, &net_state, &chat_id)
            .await
            .expect("archive");
        driver.await.expect("driver");

        assert!(archived.chat_id.starts_with(&format!("archived:{}:", chat_id)));
        assert!(net_state
            .temporary_state
            .lock()
            .await
            .chats
            .get(&chat_id)
            .is_none());
        let conn = app_state.db_conn.lock().expect("db");
        let archived_messages = crate::storage::db::get_messages(&conn, &archived.chat_id)
            .expect("archived history");
        assert_eq!(archived_messages.len(), 1);
    }

    #[tokio::test]
    async fn archive_failure_preserves_live_session() {
        let (_temp, app_state) = crate::testing::test_app_state().await;
        let (net_state, rx) = test_network_state();
        // Align the local peer id with the config keypair (as in production)
        // so the farewell remove tombstone is signed by the identity it
        // claims and the rejoin add can be issued by the same keypair.
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

        // Drive the manager finalizer and its restore handler: the farewell is
        // acknowledged with the final messages, the single-transaction persist
        // fails, and the session is restored with a rejoin add that outranks
        // the farewell remove.
        let temp_state_handle = net_state.temporary_state.clone();
        let driver = drive_archive_manager(
            net_state.temporary_state.clone(),
            app_state.clone(),
            Some(local_peer_id.clone()),
            None,
            rx,
        );
        let result = archive_temporary_chat(&app_state, &net_state, &chat_id).await;
        assert!(result.is_err(), "archive must fail when persistence fails");

        // Closing the command channel lets the mock manager finish the restore
        // (and exit), so the restored session is observable below.
        drop(net_state);
        driver.await.expect("driver");

        let temp_state = temp_state_handle.lock().await;
        let session = temp_state.chats.get(&chat_id).expect("session restored");
        assert_eq!(
            temp_state.messages.get(&chat_id).map(|messages| messages.len()),
            Some(2),
            "a failed archive must restore the live history"
        );
        assert!(
            !session.archived,
            "a failed archive must clear the reservation"
        );
        assert!(
            session.is_member(&local_peer_id),
            "a failed archive must keep the local member on the roster"
        );
        assert!(
            session.is_member(REMOTE_PEER_ID),
            "a failed archive must keep the remote member on the roster"
        );
        // The farewell already went out, so the restore must re-announce a
        // fresh signed add (not leave the remove tombstone standing) that
        // outranks the farewell remove on every peer.
        let winner = session
            .member_op_winners
            .get(&local_peer_id)
            .expect("the restored session must carry a rejoin add");
        assert_eq!(winner.op, TemporaryMembershipOpKind::Add);
        assert!(winner.verify(), "the rejoin add must be signed");
        assert_eq!(
            session.next_member_op_counter, winner.counter,
            "the restored session clock must reflect the rejoin add"
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
        let (net_state, rx) = test_network_state();
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

        // Drive the manager finalizer (acknowledge the farewell with the final
        // messages) and its restore handler (re-insert the session after the
        // single-transaction persist fails).
        let temp_state_handle = net_state.temporary_state.clone();
        let driver = drive_archive_manager(
            net_state.temporary_state.clone(),
            app_state.clone(),
            Some(LOCAL_PEER_ID.to_string()),
            None,
            rx,
        );
        let result = archive_temporary_chat(&app_state, &net_state, &chat_id).await;
        assert!(
            result.is_err(),
            "a member/peer write failure must fail the whole archive"
        );

        // Closing the command channel lets the mock manager finish the restore
        // (and exit), so the restored session is observable below.
        drop(net_state);
        driver.await.expect("driver");

        let temp_state = temp_state_handle.lock().await;
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
    async fn temporary_group_archive_releases_reservation_when_finalizer_channel_closed() {
        let (_temp, app_state) = test_app_state().await;
        let (net_state, rx) = test_network_state();
        let keypair =
            crate::chat::group::load_or_create_local_keypair(&app_state).await.expect("keypair");
        let local_peer_id = libp2p::PeerId::from_public_key(&keypair.public()).to_string();
        *net_state.local_peer_id.lock().await = Some(local_peer_id.clone());
        let chat_id = crate::chat_kind::generate_temp_group_chat_id();
        insert_temp_group_session(&net_state, &chat_id, now_unix_secs() + 3600, false).await;
        {
            let mut temp_state = net_state.temporary_state.lock().await;
            temp_state.messages.insert(
                chat_id.clone(),
                vec![temp_group_message(&chat_id, "m1", "hello")],
            );
        }
        // Close the command channel so the finalizer can never be handed to
        // the manager; the reservation must be released, not left half-torn.
        drop(rx);
        let result = archive_temporary_chat(&app_state, &net_state, &chat_id).await;
        assert!(
            result
                .expect_err("archive with closed finalizer channel")
                .to_string()
                .contains("command channel is closed"),
            "a closed finalizer channel must surface as an error"
        );
        let session = net_state.temporary_state.lock().await;
        assert!(
            !session.chats.get(&chat_id).expect("session").archived,
            "a failed finalizer handoff must clear the reservation"
        );
        assert_eq!(
            session.messages.get(&chat_id).map(|messages| messages.len()),
            Some(1),
            "a failed finalizer handoff must leave the live history intact"
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

    #[tokio::test]
    async fn temporary_group_archive_drains_buffered_tail() {
        let (_temp, app_state) = crate::testing::test_app_state().await;
        let (net_state, rx) = test_network_state();
        let chat_id = crate::chat_kind::generate_temp_group_chat_id();
        let keypair = crate::chat::group::load_or_create_local_keypair(&app_state)
            .await
            .expect("keypair");
        let local_peer_id = libp2p::PeerId::from_public_key(&keypair.public()).to_string();
        *net_state.local_peer_id.lock().await = Some(local_peer_id.clone());
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
                    members: vec![local_peer_id.clone()],
                    member_op_winners: HashMap::new(),
                    next_member_op_counter: 0,
                    archived: false,
                    pending_send_count: 0,
                },
            );
            temp_state.messages.insert(
                chat_id.clone(),
                vec![temp_group_message(&chat_id, "m1", "snapshot")],
            );
        }

        // Simulate a peer message that lands after the reservation but before
        // the manager drains (i.e. while the farewell broadcast is in flight):
        // the finalizer must capture it, never silently drop it, and the
        // single-transaction persist must store it alongside the snapshot.
        let temp_state_handle = net_state.temporary_state.clone();
        let driver = drive_archive_manager(
            net_state.temporary_state.clone(),
            app_state.clone(),
            Some(local_peer_id.clone()),
            Some(("m2".to_string(), "arrived during farewell".to_string())),
            rx,
        );
        let archived = archive_temporary_chat(&app_state, &net_state, &chat_id)
            .await
            .expect("archive");

        let conn = app_state.db_conn.lock().expect("db");
        let archived_messages = crate::storage::db::get_messages(&conn, &archived.chat_id)
            .expect("archived history");
        assert_eq!(
            archived_messages.len(),
            2,
            "a message received during the farewell broadcast must be persisted, never dropped"
        );
        assert!(
            archived_messages.iter().any(|message| message.id.starts_with("m2")),
            "the buffered tail must be stored in the same archive transaction"
        );
        drop(conn);

        // Closing the command channel lets the mock manager finish and exit.
        drop(net_state);
        driver.await.expect("driver");
        assert!(temp_state_handle.lock().await.chats.get(&chat_id).is_none());
    }

    #[tokio::test]
    async fn archive_freeze_cancelled_before_ack_recovers_via_watchdog() {
        let (_temp, app_state) = crate::testing::test_app_state().await;
        let (net_state, rx) = test_network_state();
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
                vec![temp_group_message(&chat_id, "m1", "hello")],
            );
        }

        let temp_state_handle = net_state.temporary_state.clone();
        let driver = drive_archive_manager(
            net_state.temporary_state.clone(),
            app_state.clone(),
            Some(local_peer_id.clone()),
            None,
            rx,
        );

        // The archive caller reserves the session, sends the freeze, then is
        // cancelled before the manager acknowledges it.
        let (alive_tx, _alive_rx) = tokio::sync::watch::channel(true);
        let (ack_tx, _ack_rx) = tokio::sync::oneshot::channel();
        {
            let mut temp_state = net_state.temporary_state.lock().await;
            temp_state.chats.get_mut(&chat_id).expect("session").archived = true;
        }
        net_state
            .sender
            .lock()
            .await
            .send(NetworkCommand::FreezeTemporaryArchive {
                chat_id: chat_id.clone(),
                kind: TemporaryChatKind::Group,
                farewell_winners: Vec::new(),
                min_add_counter: 0,
                alive: alive_tx.clone(),
                ack: Some(ack_tx),
            })
            .await
            .expect("freeze command sent");
        // Cancelling the caller drops the final `alive` sender, tripping the
        // manager watchdog, which must abort the freeze and restore the
        // session instead of leaving it half-torn or committing it.
        drop(alive_tx);

        let mut restored = false;
        for _ in 0..10_000 {
            if temp_state_handle
                .lock()
                .await
                .chats
                .get(&chat_id)
                .map(|session| !session.archived)
                .unwrap_or(false)
            {
                restored = true;
                break;
            }
            tokio::task::yield_now().await;
        }
        assert!(restored, "watchdog recovery never completed");

        let state = temp_state_handle.lock().await;
        let session = state.chats.get(&chat_id).expect("session restored");
        assert!(!session.archived, "cancelled freeze must clear the reservation");
        assert!(
            session.is_member(&local_peer_id),
            "cancelled freeze must keep the local member on the roster"
        );
        assert_eq!(
            state.messages.get(&chat_id).map(|messages| messages.len()),
            Some(1),
            "cancelled freeze must restore the live history"
        );
        // The farewell already drained to the pending record, so the restore
        // must re-announce a signed rejoin add that outranks the farewell
        // remove rather than leaving the remove tombstone standing.
        let winner = session
            .member_op_winners
            .get(&local_peer_id)
            .expect("restored session must carry a rejoin add");
        assert_eq!(winner.op, TemporaryMembershipOpKind::Add);
        assert!(winner.verify(), "rejoin add must be signed");
        drop(state);

        drop(net_state);
        driver.await.expect("driver");
    }

    #[tokio::test]
    async fn archive_freeze_cancelled_after_ack_before_commit_recovers_via_watchdog() {
        let (_temp, app_state) = crate::testing::test_app_state().await;
        let (net_state, rx) = test_network_state();
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
                vec![temp_group_message(&chat_id, "m1", "hello")],
            );
        }

        let temp_state_handle = net_state.temporary_state.clone();
        let driver = drive_archive_manager(
            net_state.temporary_state.clone(),
            app_state.clone(),
            Some(local_peer_id.clone()),
            None,
            rx,
        );

        // The caller reserves the session, sends the freeze, receives the
        // drained message set, then is cancelled while persisting (before it
        // can send the commit): the watchdog must still recover the session.
        let (alive_tx, _alive_rx) = tokio::sync::watch::channel(true);
        let (ack_tx, ack_rx) = tokio::sync::oneshot::channel();
        {
            let mut temp_state = net_state.temporary_state.lock().await;
            temp_state.chats.get_mut(&chat_id).expect("session").archived = true;
        }
        net_state
            .sender
            .lock()
            .await
            .send(NetworkCommand::FreezeTemporaryArchive {
                chat_id: chat_id.clone(),
                kind: TemporaryChatKind::Group,
                farewell_winners: Vec::new(),
                min_add_counter: 0,
                alive: alive_tx.clone(),
                ack: Some(ack_tx),
            })
            .await
            .expect("freeze command sent");
        let drained = ack_rx
            .await
            .expect("freeze acknowledgement")
            .expect("freeze succeeded");
        assert_eq!(drained.len(), 1, "the freeze must acknowledge the drained history");
        // The caller vanishes before the commit lands.
        drop(alive_tx);

        let mut restored = false;
        for _ in 0..10_000 {
            if temp_state_handle
                .lock()
                .await
                .chats
                .get(&chat_id)
                .map(|session| !session.archived)
                .unwrap_or(false)
            {
                restored = true;
                break;
            }
            tokio::task::yield_now().await;
        }
        assert!(restored, "watchdog recovery never completed");

        let state = temp_state_handle.lock().await;
        let session = state.chats.get(&chat_id).expect("session restored");
        assert!(!session.archived, "cancelled freeze must clear the reservation");
        assert_eq!(
            state.messages.get(&chat_id).map(|messages| messages.len()),
            Some(1),
            "cancelled freeze must restore the live history"
        );
        assert_eq!(
            session.next_member_op_counter,
            session
                .member_op_winners
                .get(&local_peer_id)
                .expect("rejoin add")
                .counter,
            "the restored session clock must reflect the rejoin add"
        );
        drop(state);

        drop(net_state);
        driver.await.expect("driver");
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
            chat_id: "temp-group:550e8400-e29b-41d4-a716-446655440000".to_string(),
            domain: crate::app_state::TEMP_GROUP_PROTOCOL_DOMAIN.to_string(),
            version: crate::app_state::TEMP_GROUP_PROTOCOL_VERSION,
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
            members: vec![LOCAL_PEER_ID.to_string(), a_peer.clone(), b_peer.clone()],
            member_op_winners: HashMap::new(),
            next_member_op_counter: 0,
            archived: false,
            pending_send_count: 0,
        };
        // A (a member) re-announces itself, then leaves with a later self-remove.
        let stale_add = signed_op(&a_keypair, 1, TemporaryMembershipOpKind::Add, &a_peer);
        let removal = signed_op(&a_keypair, 2, TemporaryMembershipOpKind::Remove, &a_peer);

        let mut session_a = new_session();
        assert!(session_a.apply_membership_ops(&[stale_add.clone()]));
        assert!(session_a.is_member(&a_peer));
        assert!(session_a.apply_membership_ops(&[removal.clone()]));
        assert!(!session_a.is_member(&a_peer));

        // A stale handshake re-announcing A (union-only merging would
        // resurrect them) must be rejected: the remove is newer, and a removed
        // member cannot re-add itself without a fresh invitation.
        assert!(!session_a.apply_membership_ops(&[stale_add.clone()]));
        assert!(!session_a.is_member(&a_peer));

        // A peer that only ever saw the stale add converges when it learns the
        // removal, and its reply cannot resurrect A on session_a either:
        // merging the winner snapshot is last-writer-wins per target.
        let mut session_b = new_session();
        assert!(session_b.apply_membership_ops(&[stale_add.clone()]));
        assert!(session_b.is_member(&a_peer));
        assert!(session_b.apply_membership_ops(&[removal]));
        assert!(!session_b.is_member(&a_peer));
        assert!(!session_a.apply_membership_ops(&session_b.membership_winners()));
        assert!(!session_a.is_member(&a_peer));
    }

    #[test]
    fn temporary_group_ops_require_valid_signatures() {
        let a_keypair = libp2p::identity::Keypair::generate_ed25519();
        let a_peer = libp2p::PeerId::from_public_key(&a_keypair.public()).to_string();
        let c_keypair = libp2p::identity::Keypair::generate_ed25519();
        let c_peer = libp2p::PeerId::from_public_key(&c_keypair.public()).to_string();
        let mut session = TemporaryChatSession {
            chat_id: "temp-group:550e8400-e29b-41d4-a716-446655440000".to_string(),
            name: "Design Crew".to_string(),
            kind: TemporaryChatKind::Group,
            expires_at: now_unix_secs() + 3600,
            peer_id: Some(a_peer.clone()),
            members: vec![
                LOCAL_PEER_ID.to_string(),
                a_peer.clone(),
                c_peer.clone(),
            ],
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
            target: a_peer.clone(),
            chat_id: "temp-group:550e8400-e29b-41d4-a716-446655440000".to_string(),
            domain: crate::app_state::TEMP_GROUP_PROTOCOL_DOMAIN.to_string(),
            version: crate::app_state::TEMP_GROUP_PROTOCOL_VERSION,
            public_key_b64: String::new(),
            signature_b64: String::new(),
        };
        assert!(!session.apply_membership_ops(&[unsigned]));
        assert!(session.is_member(&a_peer));
        assert!(session.membership_winners().is_empty());

        // An op claiming to be the local peer but signed by a different key
        // fails verification (its embedded public key derives to a different
        // peer id), even with a huge counter that would otherwise win forever.
        let mut forged = TemporaryMembershipOp {
            actor: LOCAL_PEER_ID.to_string(),
            counter: u64::MAX,
            op: TemporaryMembershipOpKind::Remove,
            target: LOCAL_PEER_ID.to_string(),
            chat_id: "temp-group:550e8400-e29b-41d4-a716-446655440000".to_string(),
            domain: crate::app_state::TEMP_GROUP_PROTOCOL_DOMAIN.to_string(),
            version: crate::app_state::TEMP_GROUP_PROTOCOL_VERSION,
            public_key_b64: String::new(),
            signature_b64: String::new(),
        };
        assert!(
            !forged.sign(&a_keypair),
            "signing with a non-matching keypair must fail"
        );
        assert!(!session.apply_membership_ops(&[forged]));
        assert!(session.is_member(LOCAL_PEER_ID));

        // Removing another member is never authorized, even with a valid
        // signature: only a peer can remove itself.
        let endorse_remove = signed_op(
            &a_keypair,
            1,
            TemporaryMembershipOpKind::Remove,
            &c_peer,
        );
        assert!(
            !session.apply_membership_ops(&[endorse_remove]),
            "a member cannot evict another member"
        );
        assert!(session.is_member(&c_peer));

        // A member's own signed self-remove is accepted.
        let own_remove = signed_op(&a_keypair, 2, TemporaryMembershipOpKind::Remove, &a_peer);
        assert!(session.apply_membership_ops(&[own_remove.clone()]));
        assert!(!session.is_member(&a_peer));

        // A removed member cannot re-add itself without a fresh invitation.
        let stale_add = signed_op(&a_keypair, 3, TemporaryMembershipOpKind::Add, &a_peer);
        assert!(
            !session.apply_membership_ops(&[stale_add]),
            "removed-member re-entry requires an invitation"
        );
        assert!(!session.is_member(&a_peer));

        // A different member may endorse-add the removed peer back, and the
        // (counter, actor) order picks the greater winner — no wall clocks.
        let add_from_c = signed_op(&c_keypair, 7, TemporaryMembershipOpKind::Add, &a_peer);
        assert!(session.apply_membership_ops(&[add_from_c.clone()]));
        assert!(session.is_member(&a_peer));
        // And a replay of the older op cannot beat it.
        assert!(!session.apply_membership_ops(&[own_remove]));
        assert!(session.is_member(&a_peer));
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
            members: vec![LOCAL_PEER_ID.to_string(), a_peer.clone(), b_peer.clone()],
            member_op_winners: HashMap::new(),
            next_member_op_counter: 0,
            archived: false,
            pending_send_count: 0,
        };
        // A re-announces itself, then leaves with a self-remove.
        a_session.apply_membership_ops(&[signed_op(
            &a_keypair,
            1,
            TemporaryMembershipOpKind::Add,
            &a_peer,
        )]);
        a_session.apply_membership_ops(&[signed_op(
            &a_keypair,
            2,
            TemporaryMembershipOpKind::Remove,
            &a_peer,
        )]);
        assert!(!a_session.is_member(&a_peer));
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
            members: vec![LOCAL_PEER_ID.to_string(), a_peer.clone(), b_peer.clone()],
            member_op_winners: HashMap::new(),
            next_member_op_counter: 0,
            archived: false,
            pending_send_count: 0,
        };
        assert!(c_session.apply_membership_ops(&forwarded));
        assert!(
            !c_session.is_member(&a_peer),
            "forwarded removals must converge transitively"
        );
    }

    #[test]
    fn temporary_group_lamport_receive_rule_advances_local_clock() {
        let a_keypair = libp2p::identity::Keypair::generate_ed25519();
        let a_peer = libp2p::PeerId::from_public_key(&a_keypair.public()).to_string();
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

        // A high-counter remote self-remove arrives: the local clock must
        // advance past it (Lamport receive rule).
        let remote_remove = signed_op(
            &a_keypair,
            1_000,
            TemporaryMembershipOpKind::Remove,
            &a_peer,
        );
        assert!(session.apply_membership_ops(&[remote_remove.clone()]));
        assert!(!session.is_member(&a_peer));

        // The reconnecting peer A rejoins via a member endorsement issued
        // locally now: the add must carry a counter beyond the received
        // remove, so it wins and A is back on the roster. Without the Lamport
        // rule the add would lose.
        let local_keypair = libp2p::identity::Keypair::generate_ed25519();
        assert!(session
            .issue_membership_op(
                &libp2p::PeerId::from_public_key(&local_keypair.public()).to_string(),
                TemporaryMembershipOpKind::Add,
                &a_peer,
                &local_keypair,
            )
            .expect("issue must sign"));
        assert!(session.is_member(&a_peer));
        let winner = session
            .member_op_winners
            .get(&a_peer)
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
            members: vec![LOCAL_PEER_ID.to_string(), a_peer.clone()],
            member_op_winners: HashMap::new(),
            next_member_op_counter: 0,
            archived: false,
            pending_send_count: 0,
        };

        // Churn a single target well past any log-truncation threshold: each
        // re-entry is invite-gated (the invited admission) and the leave is a
        // self-remove, so every round is legitimate. The winner snapshot must
        // stay complete (one winning op per target, never trimmed) so a fresh
        // peer can reconstruct the roster.
        let mut last_remove = None;
        for round in 0..(crate::app_state::TEMP_GROUP_MAX_MEMBERSHIP_OPS + 100) {
            let add = signed_op(
                &a_keypair,
                (round * 2) as u64 + 1,
                TemporaryMembershipOpKind::Add,
                &a_peer,
            );
            let remove = signed_op(
                &a_keypair,
                (round * 2) as u64 + 2,
                TemporaryMembershipOpKind::Remove,
                &a_peer,
            );
            assert!(
                session.apply_membership_ops_admitted(
                    &[add],
                    crate::app_state::MembershipOpAdmission::Invited
                ),
                "an invited member may re-join"
            );
            assert!(session.apply_membership_ops(&[remove.clone()]));
            last_remove = Some(remove);
        }

        // The winner for the target is the final remove: the authoritative
        // state survived, and the transferred snapshot reconstructs it.
        let winners = session.membership_winners();
        assert_eq!(winners.len(), 1, "one winning op per target");
        assert_eq!(winners[0].target, a_peer);
        assert_eq!(winners[0].op, TemporaryMembershipOpKind::Remove);
        assert_eq!(winners[0], last_remove.expect("last op"));
        assert!(!session.is_member(&a_peer));

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
            members: vec![LOCAL_PEER_ID.to_string(), a_peer.clone()],
            member_op_winners: HashMap::new(),
            next_member_op_counter: 0,
            archived: false,
            pending_send_count: 0,
        };
        assert!(fresh.apply_membership_ops(&winners));
        assert!(!fresh.is_member(&a_peer));
        let stale_add = signed_op(&a_keypair, 1, TemporaryMembershipOpKind::Add, &a_peer);
        assert!(!fresh.apply_membership_ops(&[stale_add]));
        assert!(!fresh.is_member(&a_peer));
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

    fn signed_op_for_chat(
        keypair: &libp2p::identity::Keypair,
        counter: u64,
        op: TemporaryMembershipOpKind,
        target: &str,
        chat_id: &str,
    ) -> TemporaryMembershipOp {
        let mut op = TemporaryMembershipOp {
            actor: libp2p::PeerId::from_public_key(&keypair.public()).to_string(),
            counter,
            op,
            target: target.to_string(),
            chat_id: chat_id.to_string(),
            domain: crate::app_state::TEMP_GROUP_PROTOCOL_DOMAIN.to_string(),
            version: crate::app_state::TEMP_GROUP_PROTOCOL_VERSION,
            public_key_b64: String::new(),
            signature_b64: String::new(),
        };
        assert!(op.sign(keypair), "test op must sign");
        op
    }

    #[test]
    fn temporary_group_ops_reject_cross_group_replay() {
        let a_keypair = libp2p::identity::Keypair::generate_ed25519();
        let a_peer = libp2p::PeerId::from_public_key(&a_keypair.public()).to_string();
        let this_chat = "temp-group:550e8400-e29b-41d4-a716-446655440000";
        let other_chat = "temp-group:11111111-2222-3333-4444-555555555555";
        let mut session = TemporaryChatSession {
            chat_id: this_chat.to_string(),
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

        // An op minted and signed for another group verifies cryptographically
        // (its signature is valid) but must never leak into this session's
        // winner state: the op is bound to its own chat id.
        let replay = signed_op_for_chat(
            &a_keypair,
            1,
            TemporaryMembershipOpKind::Remove,
            &a_peer,
            other_chat,
        );
        assert!(replay.verify(), "the op is genuinely signed");
        assert!(
            !session.apply_membership_ops(&[replay]),
            "a verified op from another group must be rejected"
        );
        assert!(session.is_member(&a_peer));
        assert!(session.membership_winners().is_empty());
    }

    #[test]
    fn temporary_group_rejects_non_member_ops() {
        let stranger_keypair = libp2p::identity::Keypair::generate_ed25519();
        let stranger = libp2p::PeerId::from_public_key(&stranger_keypair.public()).to_string();
        let target = THIRD_MEMBER_ID.to_string();
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

        // A stranger (not a member) cannot endorse-add anyone.
        let stranger_endorse = signed_op(
            &stranger_keypair,
            1,
            TemporaryMembershipOpKind::Add,
            &target,
        );
        assert!(
            !session.apply_membership_ops(&[stranger_endorse]),
            "a non-member cannot mutate the roster"
        );
        assert!(!session.is_member(&target));

        // Nor can a stranger self-add without a valid invitation.
        let stranger_self = signed_op(
            &stranger_keypair,
            2,
            TemporaryMembershipOpKind::Add,
            &stranger,
        );
        assert!(
            !session.apply_membership_ops(&[stranger_self]),
            "a non-member self-add needs an invitation"
        );
        assert!(!session.is_member(&stranger));
        assert!(session.membership_winners().is_empty());
    }

    #[test]
    fn temporary_group_rejects_counter_sentinel_and_jumps() {
        let a_keypair = libp2p::identity::Keypair::generate_ed25519();
        let a_peer = libp2p::PeerId::from_public_key(&a_keypair.public()).to_string();
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

        // A signed op carrying the reserved u64::MAX sentinel is rejected even
        // though it verifies and would otherwise win forever.
        let sentinel = signed_op(&a_keypair, u64::MAX, TemporaryMembershipOpKind::Remove, &a_peer);
        assert!(sentinel.verify());
        assert!(!session.apply_membership_ops(&[sentinel]));
        assert!(session.is_member(&a_peer));

        // Establish a legitimate winner (counter 1) for the target.
        let add = signed_op(&a_keypair, 1, TemporaryMembershipOpKind::Add, &a_peer);
        assert!(session.apply_membership_ops(&[add.clone()]));

        // An op that jumps absurdly far above the current winner is rejected,
        // so an attacker-controlled counter cannot dominate the winner map.
        let jumped = signed_op(
            &a_keypair,
            1 + crate::app_state::MAX_MEMBERSHIP_OP_COUNTER_JUMP + 5,
            TemporaryMembershipOpKind::Remove,
            &a_peer,
        );
        assert!(!session.apply_membership_ops(&[jumped]));
        assert!(session.is_member(&a_peer));
        assert_eq!(
            session.member_op_winners.get(&a_peer).expect("winner").counter,
            1,
            "the legitimate winner must be retained"
        );
    }

    #[test]
    fn temporary_group_winner_change_propagates_without_roster_change() {
        let a_keypair = libp2p::identity::Keypair::generate_ed25519();
        let a_peer = libp2p::PeerId::from_public_key(&a_keypair.public()).to_string();
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

        // A re-announces itself: a newer winner is accepted (winner state
        // changes, so it must propagate) even though the rendered roster does
        // not change.
        let re_announce = signed_op(&a_keypair, 1, TemporaryMembershipOpKind::Add, &a_peer);
        assert!(
            session.apply_membership_ops(&[re_announce.clone()]),
            "a newer winner must propagate even without a roster change"
        );
        assert!(session.is_member(&a_peer));

        // Replaying the identical op changes nothing.
        assert!(!session.apply_membership_ops(&[re_announce]));
        assert!(session.is_member(&a_peer));
    }

    #[test]
    fn temporary_group_roster_cap_promotes_queued_adds() {
        let a_keypair = libp2p::identity::Keypair::generate_ed25519();
        let a_peer = libp2p::PeerId::from_public_key(&a_keypair.public()).to_string();
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

        // A (a member) endorse-adds peers until the roster cap is reached.
        let mut counter = 1u64;
        let mut added = Vec::new();
        while session.members.len() < crate::app_state::TEMP_GROUP_MAX_MEMBERS {
            let peer = libp2p::identity::Keypair::generate_ed25519()
                .public()
                .to_peer_id()
                .to_string();
            let add = signed_op(
                &a_keypair,
                counter,
                TemporaryMembershipOpKind::Add,
                &peer,
            );
            assert!(session.apply_membership_ops(&[add]));
            added.push(peer);
            counter += 1;
        }
        assert_eq!(
            session.members.len(),
            crate::app_state::TEMP_GROUP_MAX_MEMBERS
        );

        // One more admitted winner is stored (the winner map holds it) but the
        // roster is already full, so it stays queued.
        let queued_peer = libp2p::identity::Keypair::generate_ed25519()
            .public()
            .to_peer_id()
            .to_string();
        let queued_add = signed_op(
            &a_keypair,
            counter,
            TemporaryMembershipOpKind::Add,
            &queued_peer,
        );
        assert!(
            session.apply_membership_ops(&[queued_add]),
            "the winner is stored even when the roster is full"
        );
        assert!(!session.is_member(&queued_peer), "queued at the cap");
        assert_eq!(
            session.members.len(),
            crate::app_state::TEMP_GROUP_MAX_MEMBERS
        );

        // The member A leaves: the freed slot promotes the queued add
        // deterministically.
        let a_leaves = signed_op(
            &a_keypair,
            counter + 1,
            TemporaryMembershipOpKind::Remove,
            &a_peer,
        );
        assert!(session.apply_membership_ops(&[a_leaves]));
        assert!(!session.is_member(&a_peer));
        assert!(
            session.is_member(&queued_peer),
            "freeing a slot must promote the queued add"
        );
        assert_eq!(
            session.members.len(),
            crate::app_state::TEMP_GROUP_MAX_MEMBERS
        );
    }

}
