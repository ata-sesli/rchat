use std::collections::{HashMap, HashSet};

use anyhow::{anyhow, Context};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use libp2p::{identity, PeerId};
use rusqlite::OptionalExtension;

use crate::{
    chat::media::{self, MediaKind},
    chat_kind,
    events::{
        CoreEvent, GroupMessageReceiptUpdatedEvent, GroupRecordAppliedEvent,
        GroupRosterUpdatedEvent, SharedCoreEventSink,
    },
    network::{
        command::NetworkCommand,
        gossip::{
            GroupContentType, GroupInvitePayload, GroupReceiptStatus, GroupRecordBody,
            GroupSettings,
            SignedGroupRecord,
        },
    },
    storage::db,
    AppState, NetworkState,
};

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct GroupChatResult {
    pub chat_id: String,
    pub name: String,
    pub image_hash: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct CreateGroupOptions {
    pub name: Option<String>,
    pub image_path: Option<String>,
    pub settings: Option<GroupSettings>,
    pub require_name: bool,
}

#[derive(Debug, Clone)]
pub struct GroupPolicy {
    pub admin_peer_id: String,
    pub settings: GroupSettings,
    pub active_members: HashSet<String>,
    pub invited_members: HashSet<String>,
    pub automatic_successor_peer_id: Option<String>,
    pub dissolved: bool,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum GroupLeaveOutcome {
    Left,
    TransferredThenLeft { successor_peer_id: String },
    Dissolved,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RecordDisposition {
    Apply,
    PendingDependency,
}

pub async fn create_group(
    app_state: &AppState,
    network_state: Option<&NetworkState>,
    name: Option<String>,
) -> anyhow::Result<GroupChatResult> {
    create_group_with_options(
        app_state,
        network_state,
        CreateGroupOptions {
            name,
            ..Default::default()
        },
    )
    .await
}

pub async fn create_group_with_options(
    app_state: &AppState,
    network_state: Option<&NetworkState>,
    options: CreateGroupOptions,
) -> anyhow::Result<GroupChatResult> {
    let keypair = load_or_create_local_keypair(app_state).await?;
    let local_peer_id = PeerId::from_public_key(&keypair.public()).to_string();
    let group_id = chat_kind::generate_group_chat_id();
    let resolved_name = options
        .name
        .map(|n| n.trim().to_string())
        .unwrap_or_default();
    if options.require_name && resolved_name.is_empty() {
        return Err(anyhow!("Group name is required"));
    }
    let resolved_name = if resolved_name.is_empty() {
        chat_kind::default_group_name(&group_id)
    } else {
        resolved_name
    };
    let image_hash = if let Some(image_path) = options.image_path.filter(|path| !path.is_empty()) {
        Some(
            media::store_file_object_from_path(app_state, MediaKind::Image, image_path)?
                .file_hash,
        )
    } else {
        None
    };
    let record = sign_record(
        app_state,
        &keypair,
        group_id.clone(),
        GroupRecordBody::GroupCreated {
            name: resolved_name.clone(),
            settings: options.settings,
            image_hash: image_hash.clone(),
        },
    )?;
    let file_availability_record = image_hash
        .as_ref()
        .map(|file_hash| {
            // Continuation of the same causal chain: the image availability
            // is announced immediately after creation, so it must occupy the
            // next Lamport position rather than recomputing from the still-
            // empty DB (both records are signed before either is persisted).
            SignedGroupRecord::new(
                &keypair,
                group_id.clone(),
                format!(
                    "group-rec-{}-{}",
                    timestamp_now(),
                    rand::random::<u32>()
                ),
                timestamp_now(),
                vec![record.id().to_string()],
                record.lamport_counter().saturating_add(1),
                GroupRecordBody::FileAvailability {
                    file_hash: file_hash.clone(),
                },
            )
        })
        .transpose()?;

    // Finalize through the shared apply path so the record is validated
    // against its exact parents under the same DB lock that changes it to
    // verified, rather than trusting an earlier precheck.
    crate::chat::group::apply_signed_record(app_state, None, &record, true)?;
    if let (Some(image_hash), Some(file_record)) = (&image_hash, &file_availability_record) {
        let conn = app_state.db_conn.lock().map_err(|e| anyhow!(e.to_string()))?;
        db::upsert_group_file_source(&conn, &group_id, image_hash, &local_peer_id)?;
        db::insert_group_record(&conn, file_record, true, false)?;
    }

    if let Some(network_state) = network_state {
        send_network_command(
            network_state,
            NetworkCommand::PublishGroupRecord {
                record: record.clone(),
            },
        )
        .await?;
        if let Some(file_record) = file_availability_record {
            send_network_command(
                network_state,
                NetworkCommand::PublishGroupRecord {
                    record: file_record,
                },
            )
            .await?;
        }
    }

    Ok(GroupChatResult {
        chat_id: group_id,
        name: resolved_name,
        image_hash,
    })
}

pub fn get_group_policy(app_state: &AppState, group_id: &str) -> anyhow::Result<GroupPolicy> {
    let conn = app_state.db_conn.lock().map_err(|e| anyhow!(e.to_string()))?;
    let records = db::get_all_verified_group_records_ordered(&conn, group_id)?;
    derive_group_policy(&records).ok_or_else(|| anyhow!("Group has no valid founder record"))
}

pub fn get_group_roster(
    app_state: &AppState,
    group_id: &str,
) -> anyhow::Result<Vec<db::GroupMemberRow>> {
    let conn = app_state.db_conn.lock().map_err(|e| anyhow!(e.to_string()))?;
    db::get_group_roster(&conn, group_id)
}

pub fn get_group_image_hash(app_state: &AppState, group_id: &str) -> anyhow::Result<Option<String>> {
    let conn = app_state.db_conn.lock().map_err(|e| anyhow!(e.to_string()))?;
    db::get_group_image_hash(&conn, group_id)
}

pub fn get_group_message_receipts(
    app_state: &AppState,
    group_id: &str,
    message_id: &str,
) -> anyhow::Result<Vec<db::GroupMessageReceiptRow>> {
    let conn = app_state.db_conn.lock().map_err(|e| anyhow!(e.to_string()))?;
    db::get_group_message_receipts(&conn, group_id, message_id)
}

pub fn get_group_pending_record_summary(
    app_state: &AppState,
    group_id: &str,
) -> anyhow::Result<db::GroupPendingRecordSummary> {
    let conn = app_state.db_conn.lock().map_err(|e| anyhow!(e.to_string()))?;
    db::get_group_pending_record_summary(&conn, group_id)
}

pub async fn is_local_group_admin(app_state: &AppState, group_id: &str) -> anyhow::Result<bool> {
    let policy = get_group_policy(app_state, group_id)?;
    let keypair = load_or_create_local_keypair(app_state).await?;
    Ok(PeerId::from_public_key(&keypair.public()).to_string() == policy.admin_peer_id)
}

pub fn can_peer_sync_group_records(
    app_state: &AppState,
    group_id: &str,
    peer_id: &str,
) -> anyhow::Result<bool> {
    let policy = get_group_policy(app_state, group_id)?;
    Ok(policy.active_members.contains(peer_id))
}

pub async fn update_group_settings(
    app_state: &AppState,
    network_state: &NetworkState,
    group_id: String,
    settings: GroupSettings,
) -> anyhow::Result<()> {
    let keypair = load_or_create_local_keypair(app_state).await?;
    let local_peer_id = PeerId::from_public_key(&keypair.public()).to_string();
    let policy = get_group_policy(app_state, &group_id)?;
    if policy.admin_peer_id != local_peer_id {
        return Err(anyhow!("Only the group admin can update group settings"));
    }

    let record = sign_record(
        app_state,
        &keypair,
        group_id,
        GroupRecordBody::GroupSettingsUpdated { settings },
    )?;
    apply_signed_record(app_state, None, &record, true)?;
    send_network_command(network_state, NetworkCommand::PublishGroupRecord { record }).await
}

pub async fn remove_member(
    app_state: &AppState,
    network_state: &NetworkState,
    group_id: String,
    peer_id: String,
) -> anyhow::Result<()> {
    let keypair = load_or_create_local_keypair(app_state).await?;
    let local_peer_id = PeerId::from_public_key(&keypair.public()).to_string();
    let policy = get_group_policy(app_state, &group_id)?;
    if policy.admin_peer_id != local_peer_id {
        return Err(anyhow!("Only the group admin can remove members"));
    }
    if peer_id == policy.admin_peer_id {
        return Err(anyhow!(
            "The current administrator cannot be removed; transfer administration first"
        ));
    }

    let record = sign_record(
        app_state,
        &keypair,
        group_id,
        GroupRecordBody::MemberRemoved { peer_id },
    )?;
    apply_signed_record(app_state, None, &record, true)?;
    send_network_command(network_state, NetworkCommand::PublishGroupRecord { record }).await
}

pub async fn transfer_group_admin(
    app_state: &AppState,
    network_state: &NetworkState,
    group_id: String,
    new_admin_peer_id: String,
) -> anyhow::Result<()> {
    let keypair = load_or_create_local_keypair(app_state).await?;
    let local_peer_id = PeerId::from_public_key(&keypair.public()).to_string();
    let policy = get_group_policy(app_state, &group_id)?;
    if policy.admin_peer_id != local_peer_id {
        return Err(anyhow!("Only the group admin can transfer administration"));
    }
    if new_admin_peer_id == local_peer_id {
        return Err(anyhow!("The current admin is already the group administrator"));
    }
    if !policy.active_members.contains(&new_admin_peer_id) {
        return Err(anyhow!("The new administrator must be an active member"));
    }
    let record = sign_record(
        app_state,
        &keypair,
        group_id,
        GroupRecordBody::AdminTransferred { new_admin_peer_id },
    )?;
    apply_signed_record(app_state, None, &record, true)?;
    send_network_command(network_state, NetworkCommand::PublishGroupRecord { record }).await
}

pub async fn preview_leave_group(
    app_state: &AppState,
    group_id: &str,
) -> anyhow::Result<GroupLeaveOutcome> {
    let keypair = load_or_create_local_keypair(app_state).await?;
    let local_peer_id = PeerId::from_public_key(&keypair.public()).to_string();
    let policy = get_group_policy(app_state, group_id)?;
    if policy.admin_peer_id != local_peer_id {
        return Ok(GroupLeaveOutcome::Left);
    }
    match policy.automatic_successor_peer_id {
        Some(successor_peer_id) => Ok(GroupLeaveOutcome::TransferredThenLeft { successor_peer_id }),
        None => Ok(GroupLeaveOutcome::Dissolved),
    }
}

pub async fn invite_member(
    app_state: &AppState,
    network_state: &NetworkState,
    group_id: String,
    peer_id: String,
) -> anyhow::Result<String> {
    if !chat_kind::is_group_chat_id(&group_id) {
        return Err(anyhow!("Invalid group id. Expected format group:<uuid>"));
    }

    let keypair = load_or_create_local_keypair(app_state).await?;
    let local_peer_id = PeerId::from_public_key(&keypair.public()).to_string();
    let policy = get_group_policy(app_state, &group_id)?;
    if !can_invite(&policy, &local_peer_id) {
        return Err(anyhow!("Only the group admin can invite members"));
    }
    let (group_name, related_records) = {
        let conn = app_state.db_conn.lock().map_err(|e| anyhow!(e.to_string()))?;
        let group_name = db::get_chat_list(&conn)?
            .into_iter()
            .find(|chat| chat.id == group_id)
            .map(|chat| chat.name)
            .unwrap_or_else(|| chat_kind::default_group_name(&group_id));
        let related_records = db::get_group_records_for_sync(&conn, &group_id, &[], 64)?;
        (group_name, related_records)
    };

    let invite_record = sign_record(
        app_state,
        &keypair,
        group_id.clone(),
        GroupRecordBody::MemberInvited {
            peer_id: peer_id.clone(),
            role: "member".to_string(),
        },
    )?;
    let invite_id = invite_record.id().to_string();
    let payload = GroupInvitePayload {
        version: 1,
        invite_id: invite_id.clone(),
        group_id: group_id.clone(),
        group_name,
        inviter_peer_id: local_peer_id,
        invitee_peer_id: peer_id.clone(),
        created_at: timestamp_now(),
        invite_record: invite_record.clone(),
        related_records,
    };

    {
        let conn = app_state.db_conn.lock().map_err(|e| anyhow!(e.to_string()))?;
        db::upsert_group_invite(&conn, &payload, "sent")?;
    }
    crate::chat::group::apply_signed_record(app_state, None, &invite_record, true)?;

    send_network_command(
        network_state,
        NetworkCommand::PublishGroupRecord {
            record: invite_record,
        },
    )
    .await?;
    send_network_command(
        network_state,
        NetworkCommand::SendGroupInvite {
            target_peer_id: peer_id,
            invite: payload,
        },
    )
    .await?;

    Ok(invite_id)
}

pub async fn accept_invite(
    app_state: &AppState,
    network_state: &NetworkState,
    invite_id: String,
) -> anyhow::Result<String> {
    let invite = {
        let conn = app_state.db_conn.lock().map_err(|e| anyhow!(e.to_string()))?;
        db::get_group_invite_payload(&conn, &invite_id)?
            .ok_or_else(|| anyhow!("Unknown group invite: {invite_id}"))?
    };
    if !invite.invite_record.verify() {
        return Err(anyhow!("Group invite signature could not be verified"));
    }

    let keypair = load_or_create_local_keypair(app_state).await?;
    let local_peer_id = PeerId::from_public_key(&keypair.public()).to_string();
    if invite.invitee_peer_id != local_peer_id {
        return Err(anyhow!("Group invite was not addressed to this peer"));
    }

    for record in invite
        .related_records
        .iter()
        .chain(std::iter::once(&invite.invite_record))
    {
        apply_signed_record(app_state, None, record, true)?;
    }
    if get_group_policy(app_state, &invite.group_id)?.dissolved {
        let conn = app_state.db_conn.lock().map_err(|e| anyhow!(e.to_string()))?;
        db::update_group_invite_status(&conn, &invite_id, "revoked")?;
        return Err(anyhow!("This group has been dissolved"));
    }

    let joined_record = sign_record(
        app_state,
        &keypair,
        invite.group_id.clone(),
        GroupRecordBody::MemberJoined {
            peer_id: invite.invitee_peer_id.clone(),
        },
    )?;
    {
        let conn = app_state.db_conn.lock().map_err(|e| anyhow!(e.to_string()))?;
        db::update_group_invite_status(&conn, &invite_id, "accepted")?;
    }
    crate::chat::group::apply_signed_record(app_state, None, &joined_record, true)?;

    send_network_command(
        network_state,
        NetworkCommand::SubscribeGroup {
            group_id: invite.group_id.clone(),
        },
    )
    .await?;
    send_network_command(
        network_state,
        NetworkCommand::PublishGroupRecord {
            record: joined_record,
        },
    )
    .await?;
    send_network_command(
        network_state,
        NetworkCommand::SyncGroup {
            group_id: invite.group_id.clone(),
        },
    )
    .await?;

    Ok(invite.group_id)
}

pub fn reject_invite(app_state: &AppState, invite_id: &str) -> anyhow::Result<()> {
    let conn = app_state.db_conn.lock().map_err(|e| anyhow!(e.to_string()))?;
    db::update_group_invite_status(&conn, invite_id, "rejected")
}

pub async fn leave_group(
    app_state: &AppState,
    network_state: &NetworkState,
    group_id: String,
) -> anyhow::Result<GroupLeaveOutcome> {
    let keypair = load_or_create_local_keypair(app_state).await?;
    let local_peer_id = PeerId::from_public_key(&keypair.public()).to_string();
    let policy = get_group_policy(app_state, &group_id)?;
    let outcome;
    if policy.admin_peer_id == local_peer_id {
        if let Some(successor_peer_id) = policy.automatic_successor_peer_id {
            let transfer_timestamp = timestamp_now();
            let transfer = sign_record_at(
                app_state,
                &keypair,
                group_id.clone(),
                transfer_timestamp,
                Vec::new(),
                GroupRecordBody::AdminTransferred {
                    new_admin_peer_id: successor_peer_id.clone(),
                },
            )?;
            apply_signed_record(app_state, None, &transfer, true)?;
            send_network_command(
                network_state,
                NetworkCommand::PublishGroupRecord {
                    record: transfer.clone(),
                },
            )
            .await?;
            let leave = sign_record_at(
                app_state,
                &keypair,
                group_id.clone(),
                transfer_timestamp.saturating_add(1),
                vec![transfer.id().to_string()],
                GroupRecordBody::MemberLeft {
                    peer_id: local_peer_id,
                },
            )?;
            apply_signed_record(app_state, None, &leave, true)?;
            send_network_command(network_state, NetworkCommand::PublishGroupRecord { record: leave })
                .await?;
            outcome = GroupLeaveOutcome::TransferredThenLeft { successor_peer_id };
        } else {
            let invitees = {
                let conn = app_state.db_conn.lock().map_err(|e| anyhow!(e.to_string()))?;
                db::get_open_group_invitee_peer_ids(&conn, &group_id)?
            };
            let dissolution = sign_record(app_state, &keypair, group_id.clone(), GroupRecordBody::GroupDissolved)?;
            apply_signed_record(app_state, None, &dissolution, true)?;
            send_network_command(
                network_state,
                NetworkCommand::PublishGroupRecord {
                    record: dissolution.clone(),
                },
            )
            .await?;
            for target_peer_id in invitees {
                send_network_command(
                    network_state,
                    NetworkCommand::SendGroupDissolution {
                        target_peer_id,
                        record: dissolution.clone(),
                    },
                )
                .await?;
            }
            outcome = GroupLeaveOutcome::Dissolved;
        }
    } else {
        let leave = sign_record(
            app_state,
            &keypair,
            group_id.clone(),
            GroupRecordBody::MemberLeft {
                peer_id: local_peer_id,
            },
        )?;
        apply_signed_record(app_state, None, &leave, true)?;
        send_network_command(network_state, NetworkCommand::PublishGroupRecord { record: leave })
            .await?;
        outcome = GroupLeaveOutcome::Left;
    }
    if !matches!(outcome, GroupLeaveOutcome::Dissolved) {
        let conn = app_state.db_conn.lock().map_err(|e| anyhow!(e.to_string()))?;
        db::delete_group_chat(&conn, &group_id)?;
    }
    send_network_command(
        network_state,
        NetworkCommand::UnsubscribeGroup {
            group_id: group_id.clone(),
        },
    )
    .await?;
    Ok(outcome)
}

pub async fn rename_group(
    app_state: &AppState,
    network_state: &NetworkState,
    group_id: String,
    name: String,
) -> anyhow::Result<()> {
    let name = name.trim().to_string();
    if name.is_empty() {
        return Err(anyhow!("Group name cannot be empty"));
    }
    let keypair = load_or_create_local_keypair(app_state).await?;
    let local_peer_id = PeerId::from_public_key(&keypair.public()).to_string();
    let policy = get_group_policy(app_state, &group_id)?;
    if policy.admin_peer_id != local_peer_id {
        return Err(anyhow!("Only the group admin can rename the group"));
    }
    let record = sign_record(
        app_state,
        &keypair,
        group_id.clone(),
        GroupRecordBody::GroupRenamed { name: name.clone() },
    )?;
    crate::chat::group::apply_signed_record(app_state, None, &record, true)?;
    send_network_command(network_state, NetworkCommand::PublishGroupRecord { record }).await?;
    Ok(())
}

pub async fn send_group_text(
    app_state: &AppState,
    network_state: &NetworkState,
    group_id: String,
    text: String,
    sender_alias: Option<String>,
) -> anyhow::Result<String> {
    send_group_message_record(
        app_state,
        network_state,
        group_id,
        GroupContentType::Text,
        Some(text),
        None,
        sender_alias,
    )
    .await
}

pub async fn send_group_media_reference(
    app_state: &AppState,
    network_state: &NetworkState,
    group_id: String,
    kind: GroupContentType,
    file_hash: String,
    display_name: Option<String>,
    sender_alias: Option<String>,
) -> anyhow::Result<String> {
    if !kind.needs_file_transfer() {
        return Err(anyhow!("Group media reference requires a file-backed content type"));
    }
    send_group_message_record(
        app_state,
        network_state,
        group_id,
        kind,
        display_name,
        Some(file_hash),
        sender_alias,
    )
    .await
}

pub async fn sync_group(network_state: &NetworkState, group_id: String) -> anyhow::Result<()> {
    send_network_command(network_state, NetworkCommand::SyncGroup { group_id }).await
}

pub async fn mark_read(
    app_state: &AppState,
    network_state: &NetworkState,
    group_id: String,
    message_ids: Vec<String>,
) -> anyhow::Result<()> {
    if message_ids.is_empty() {
        return Ok(());
    }
    let record = create_receipt_record(
        app_state,
        group_id.clone(),
        message_ids,
        GroupReceiptStatus::Read,
    )
    .await?;
    apply_signed_record(app_state, None, &record, true)?;
    send_network_command(network_state, NetworkCommand::PublishGroupRecord { record }).await
}

pub async fn create_receipt_record(
    app_state: &AppState,
    group_id: String,
    message_ids: Vec<String>,
    status: GroupReceiptStatus,
) -> anyhow::Result<SignedGroupRecord> {
    if message_ids.is_empty() {
        return Err(anyhow!("Group receipt requires at least one message id"));
    }
    let keypair = load_or_create_local_keypair(app_state).await?;
    sign_record(
        app_state,
        &keypair,
        group_id,
        GroupRecordBody::Receipt {
            message_ids,
            status,
        },
    )
}

/// The causal total order over a group's records. The signed Lamport counter
/// dominates; `(author_peer_id, id)` only break ties between records that are
/// genuinely concurrent (same counter from different authors) and between
/// pre-counter legacy records. Wall-clock timestamps are never part of the
/// order, so clock skew or a forged clock cannot decide authorization.
fn record_order_key(record: &SignedGroupRecord) -> (u64, &str, &str) {
    (
        record.lamport_counter(),
        record.author_peer_id(),
        record.id(),
    )
}

fn derive_group_policy(records: &[SignedGroupRecord]) -> Option<GroupPolicy> {
    let mut ordered = records.to_vec();
    ordered.sort_by(|a, b| record_order_key(a).cmp(&record_order_key(b)));

    let mut admin_peer_id = None;
    let mut settings = GroupSettings::default();
    let mut active_members = HashSet::new();
    let mut invited_members = HashSet::new();
    let mut membership_order: HashMap<String, (u64, String)> = HashMap::new();
    let mut dissolved = false;

    for record in ordered {
        if dissolved {
            continue;
        }
        match record.body() {
            GroupRecordBody::GroupCreated {
                settings: group_settings,
                ..
            } => {
                if admin_peer_id.is_none() {
                    let author = record.author_peer_id().to_string();
                    admin_peer_id = Some(author.clone());
                    active_members.insert(author.clone());
                    membership_order.insert(
                        author,
                        (record.lamport_counter(), record.id().to_string()),
                    );
                    settings = group_settings.clone().unwrap_or_default();
                }
            }
            GroupRecordBody::MemberInvited { peer_id, .. } => {
                if let Some(admin) = admin_peer_id.as_deref() {
                    if record.author_peer_id() == admin
                        || (settings.members_can_invite
                            && active_members.contains(record.author_peer_id()))
                    {
                        invited_members.insert(peer_id.clone());
                    }
                }
            }
            GroupRecordBody::MemberJoined { peer_id } => {
                if record.author_peer_id() == peer_id && invited_members.contains(peer_id) {
                    active_members.insert(peer_id.clone());
                    invited_members.remove(peer_id);
                    membership_order.insert(
                        peer_id.clone(),
                        (record.lamport_counter(), record.id().to_string()),
                    );
                }
            }
            GroupRecordBody::MemberLeft { peer_id } => {
                if record.author_peer_id() == peer_id
                    && Some(peer_id.as_str()) != admin_peer_id.as_deref()
                {
                    active_members.remove(peer_id);
                    invited_members.remove(peer_id);
                    membership_order.remove(peer_id);
                }
            }
            GroupRecordBody::MemberRemoved { peer_id } => {
                if Some(record.author_peer_id()) == admin_peer_id.as_deref()
                    && Some(peer_id.as_str()) != admin_peer_id.as_deref()
                {
                    active_members.remove(peer_id);
                    invited_members.remove(peer_id);
                    membership_order.remove(peer_id);
                }
            }
            GroupRecordBody::AdminTransferred { new_admin_peer_id } => {
                if Some(record.author_peer_id()) == admin_peer_id.as_deref()
                    && active_members.contains(new_admin_peer_id)
                    && record.author_peer_id() != new_admin_peer_id
                {
                    admin_peer_id = Some(new_admin_peer_id.clone());
                }
            }
            GroupRecordBody::GroupDissolved => {
                if Some(record.author_peer_id()) == admin_peer_id.as_deref()
                    && active_members.len() == 1
                {
                    dissolved = true;
                    active_members.clear();
                    invited_members.clear();
                }
            }
            GroupRecordBody::GroupSettingsUpdated {
                settings: updated_settings,
            } => {
                if Some(record.author_peer_id()) == admin_peer_id.as_deref() {
                    settings = updated_settings.clone();
                }
            }
            GroupRecordBody::GroupRenamed { .. }
            | GroupRecordBody::Message { .. }
            | GroupRecordBody::Receipt { .. }
            | GroupRecordBody::Head { .. }
            | GroupRecordBody::FileAvailability { .. } => {}
        }
    }

    admin_peer_id.map(|admin_peer_id| {
        let automatic_successor_peer_id = active_members
            .iter()
            .filter(|peer_id| peer_id.as_str() != admin_peer_id)
            .min_by_key(|peer_id| {
                membership_order
                    .get(*peer_id)
                    .cloned()
                    .unwrap_or((u64::MAX, (*peer_id).clone()))
            })
            .cloned();
        GroupPolicy {
            admin_peer_id,
            settings,
            active_members,
            invited_members,
            automatic_successor_peer_id,
            dissolved,
        }
    })
}

fn can_invite(policy: &GroupPolicy, peer_id: &str) -> bool {
    policy.admin_peer_id == peer_id
        || (policy.settings.members_can_invite && policy.active_members.contains(peer_id))
}

fn is_policy_changing(body: &GroupRecordBody) -> bool {
    matches!(
        body,
        GroupRecordBody::GroupCreated { .. }
            | GroupRecordBody::MemberInvited { .. }
            | GroupRecordBody::MemberJoined { .. }
            | GroupRecordBody::MemberLeft { .. }
            | GroupRecordBody::MemberRemoved { .. }
            | GroupRecordBody::AdminTransferred { .. }
            | GroupRecordBody::GroupDissolved
            | GroupRecordBody::GroupRenamed { .. }
            | GroupRecordBody::GroupSettingsUpdated { .. }
    )
}

fn group_record_state(
    conn: &rusqlite::Connection,
    record_id: &str,
) -> anyhow::Result<Option<(bool, bool)>> {
    conn.query_row(
        "SELECT verified, pending FROM group_records WHERE id = ?1",
        [record_id],
        |row| Ok((row.get::<_, i64>(0)? != 0, row.get::<_, i64>(1)? != 0)),
    )
    .optional()
    .map_err(Into::into)
}

fn mark_group_record_verified(
    conn: &rusqlite::Connection,
    record_id: &str,
) -> anyhow::Result<()> {
    conn.execute(
        "UPDATE group_records SET verified = 1, pending = 0 WHERE id = ?1",
        [record_id],
    )?;
    Ok(())
}

fn pending_group_records(
    conn: &rusqlite::Connection,
    group_id: &str,
) -> anyhow::Result<Vec<SignedGroupRecord>> {
    let mut stmt = conn.prepare(
        "SELECT payload_json FROM group_records
         WHERE group_id = ?1 AND pending = 1
         ORDER BY timestamp ASC",
    )?;
    let rows = stmt.query_map([group_id], |row| row.get::<_, String>(0))?;
    let mut records = Vec::new();
    for row in rows {
        records.push(serde_json::from_str(&row?)?);
    }
    Ok(records)
}

fn validate_group_record(
    conn: &rusqlite::Connection,
    record: &SignedGroupRecord,
) -> anyhow::Result<RecordDisposition> {
    for parent_id in &record.unsigned.parents {
        let Some(parent) = db::get_group_record(conn, parent_id)? else {
            return Ok(RecordDisposition::PendingDependency);
        };
        if parent.group_id() != record.group_id() {
            return Err(anyhow!(
                "Group record parent {} belongs to different group {}",
                parent_id,
                parent.group_id()
            ));
        }
        if !matches!(group_record_state(conn, parent_id)?, Some((true, false))) {
            return Ok(RecordDisposition::PendingDependency);
        }
    }
    // Mandatory v3: every non-creation record must name its causal head(s).
    // Counter 0/MAX are handled as hard counter errors, not parent errors.
    if !matches!(record.body(), GroupRecordBody::GroupCreated { .. })
        && record.unsigned.parents.is_empty()
        && record.lamport_counter() != 0
        && record.lamport_counter() != u64::MAX
    {
        return Err(anyhow!("Group record must name the policy head it builds on"));
    }
    validate_record_counter(conn, record)?;

    // The causal snapshot this record was signed against. Authorization is
    // evaluated against exactly this closure — never against records the
    // author could not have seen.
    let closure = if matches!(record.body(), GroupRecordBody::GroupCreated { .. }) {
        Vec::new()
    } else {
        collect_parent_closure(conn, record.group_id(), &record.unsigned.parents)?
    };
    if closure.is_empty() && !matches!(record.body(), GroupRecordBody::GroupCreated { .. }) {
        return Ok(RecordDisposition::PendingDependency);
    }
    let closure_ids: std::collections::HashSet<String> =
        closure.iter().map(|r| r.id().to_string()).collect();

    // Multi-head stale-branch check: an authorization-sensitive record must
    // dominate **every** policy-changing record below its own counter, not
    // just one arbitrarily selected "winning" head. Reducing the multi-head
    // causal frontier to a single max_by(order) winner lets an attacker grind
    // the tie-break and extend a concurrent branch that omits, say, their own
    // removal. With the dominate-all rule, a branch that omits a lower policy
    // record can never supersede it; genuinely concurrent policy records at
    // the *same* counter remain allowed and are resolved by the deterministic
    // (counter, author, id) total order inside `derive_group_policy`.
    if !matches!(
        record.body(),
        GroupRecordBody::GroupCreated { .. }
            | GroupRecordBody::Head { .. }
            | GroupRecordBody::FileAvailability { .. }
    ) {
        for policy in db::get_policy_records_before_counter(
            conn,
            record.group_id(),
            record.lamport_counter(),
        )? {
            if !closure_ids.contains(policy.id()) {
                return Err(anyhow!(
                    "Group record at {} must dominate policy record {} at {} (it builds on a stale branch)",
                    record.lamport_counter(),
                    policy.id(),
                    policy.lamport_counter()
                ));
            }
        }
    }

    let existing_records = db::get_all_verified_group_records_ordered(conn, record.group_id())?;
    let current_policy = derive_group_policy(&existing_records);
    let policy_before_record = if matches!(record.body(), GroupRecordBody::GroupCreated { .. }) {
        None
    } else {
        derive_group_policy(&closure)
    };

    if current_policy.as_ref().is_some_and(|policy| policy.dissolved) {
        return Err(anyhow!("Group has been dissolved"));
    }

    match record.body() {
        GroupRecordBody::GroupCreated { .. } => {
            if current_policy.is_some() {
                return Err(anyhow!("Group already has a founder record"));
            }
            Ok(RecordDisposition::Apply)
        }
        GroupRecordBody::MemberInvited { .. } => {
            let Some(policy) = policy_before_record else {
                return Ok(RecordDisposition::PendingDependency);
            };
            if can_invite(&policy, record.author_peer_id()) {
                Ok(RecordDisposition::Apply)
            } else if policy.active_members.contains(record.author_peer_id()) {
                Err(anyhow!("Members cannot invite unless group settings allow it"))
            } else {
                Err(anyhow!("Only members can invite"))
            }
        }
        GroupRecordBody::MemberJoined { peer_id } => {
            let Some(policy) = policy_before_record else {
                return Ok(RecordDisposition::PendingDependency);
            };
            if record.author_peer_id() != peer_id {
                return Err(anyhow!("MemberJoined author must match joined peer"));
            }
            if policy.invited_members.contains(peer_id) {
                Ok(RecordDisposition::Apply)
            } else {
                Err(anyhow!("Member was not invited"))
            }
        }
        GroupRecordBody::MemberLeft { peer_id } => {
            let Some(policy) = policy_before_record else {
                return Ok(RecordDisposition::PendingDependency);
            };
            if record.author_peer_id() != peer_id {
                return Err(anyhow!("Members can only leave for themselves"));
            }
            if policy.admin_peer_id == *peer_id {
                return Err(anyhow!(
                    "The administrator must transfer administration before leaving"
                ));
            }
            if policy.active_members.contains(peer_id) {
                Ok(RecordDisposition::Apply)
            } else {
                Err(anyhow!("Member is not active"))
            }
        }
        GroupRecordBody::GroupRenamed { .. }
        | GroupRecordBody::GroupSettingsUpdated { .. }
        | GroupRecordBody::MemberRemoved { .. } => {
            let Some(policy) = policy_before_record else {
                return Ok(RecordDisposition::PendingDependency);
            };
            if policy.admin_peer_id != record.author_peer_id() {
                return Err(anyhow!("Only the group admin can apply this record"));
            }
            if let GroupRecordBody::MemberRemoved { peer_id } = record.body() {
                if policy.admin_peer_id == *peer_id {
                    return Err(anyhow!(
                        "The current administrator cannot be removed; transfer administration first"
                    ));
                }
            }
            Ok(RecordDisposition::Apply)
        }
        GroupRecordBody::AdminTransferred { new_admin_peer_id } => {
            let Some(policy) = policy_before_record else {
                return Ok(RecordDisposition::PendingDependency);
            };
            if policy.admin_peer_id != record.author_peer_id() {
                return Err(anyhow!("Only the group admin can transfer administration"));
            }
            if new_admin_peer_id == record.author_peer_id() {
                return Err(anyhow!("The current admin is already the group administrator"));
            }
            if !policy.active_members.contains(new_admin_peer_id) {
                return Err(anyhow!("The new administrator must be an active member"));
            }
            Ok(RecordDisposition::Apply)
        }
        GroupRecordBody::GroupDissolved => {
            let Some(policy) = policy_before_record else {
                return Ok(RecordDisposition::PendingDependency);
            };
            if policy.admin_peer_id != record.author_peer_id() {
                return Err(anyhow!("Only the group admin can dissolve the group"));
            }
            if policy.active_members.len() != 1 {
                return Err(anyhow!("A group can only be dissolved by its sole active member"));
            }
            Ok(RecordDisposition::Apply)
        }
        GroupRecordBody::Message { .. }
        | GroupRecordBody::Receipt { .. }
        | GroupRecordBody::Head { .. }
        | GroupRecordBody::FileAvailability { .. } => {
            let Some(policy) = policy_before_record else {
                return Ok(RecordDisposition::PendingDependency);
            };
            if policy.active_members.contains(record.author_peer_id()) {
                Ok(RecordDisposition::Apply)
            } else {
                Err(anyhow!("Only active members can create this record"))
            }
        }
    }
}

/// Enforce the causal counter discipline for current-version records:
/// a nonzero position within the jump cap, and strict monotonicity per
/// author. These checks run before any policy evaluation, so a record can
/// v3 is a mandatory upgrade: every record must carry a causal counter
/// that directly follows its parents, so a claimed ordering position cannot
/// be used to authorize a pre-signed action that did not causally depend on
/// its predecessors.
fn validate_record_counter(
    conn: &rusqlite::Connection,
    record: &SignedGroupRecord,
) -> anyhow::Result<()> {
    let counter = record.lamport_counter();
    if counter == 0 {
        return Err(anyhow!("Group record is missing its causal counter"));
    }
    if counter == u64::MAX {
        return Err(anyhow!("Group record counter exhausted"));
    }
    let is_created = matches!(record.body(), GroupRecordBody::GroupCreated { .. });
    if is_created {
        if !record.unsigned.parents.is_empty() {
            return Err(anyhow!("GroupCreated must have no parents"));
        }
        if counter != 1 {
            return Err(anyhow!("GroupCreated must be at causal position 1"));
        }
    } else if record.unsigned.parents.is_empty() {
        // Empty parents are handled as pending in the outer validator when
        // out-of-order; here the counter is known to follow the current head,
        // so just fall through to the frontier/fork checks.
    } else {
        // Direct parents must already be present (missing parents are
        // handled as PendingDependency before this is called), so we can
        // require the counter to directly follow them.
        let mut max_parent = 0u64;
        for parent_id in &record.unsigned.parents {
            let Some(parent) = db::get_group_record(conn, parent_id)? else {
                return Err(anyhow!("Group record parent {parent_id} not found"));
            };
            max_parent = max_parent.max(parent.lamport_counter());
        }
        if counter != max_parent.saturating_add(1) {
            return Err(anyhow!(
                "Group record counter {counter} must directly follow its parents at {max_parent}"
            ));
        }
    }

    // Frontier check via an indexed MAX without a 10k page — a high counter
    // outside the timestamp-sorted page must still be visible.
    let max_known = db::get_group_max_lamport_counter(conn, record.group_id())?;
    if counter > max_known.saturating_add(SignedGroupRecord::MAX_COUNTER_JUMP) {
        return Err(anyhow!(
            "Group record counter {counter} leaps more than {} past the known frontier {max_known}",
            SignedGroupRecord::MAX_COUNTER_JUMP
        ));
    }
    if db::has_author_counter(
        conn,
        record.group_id(),
        record.author_peer_id(),
        counter,
        record.id(),
    )? {
        return Err(anyhow!(
            "Group record forks author {}'s causal position {counter}",
            record.author_peer_id()
        ));
    }
    Ok(())
}

fn retry_pending_group_records(
    app_state: &AppState,
    event_sink: Option<&SharedCoreEventSink>,
    group_id: &str,
) {
    let pending = {
        let Ok(conn) = app_state.db_conn.lock() else {
            return;
        };
        pending_group_records(&conn, group_id).unwrap_or_default()
    };

    for record in pending {
        if let Err(err) = apply_signed_record(app_state, event_sink, &record, true) {
            eprintln!(
                "[Group] Failed to retry pending record {}: {}",
                record.id(),
                err
            );
        }
    }
}

/// Events produced while projecting a record into materialized state. They
/// are emitted only after the enclosing transaction has committed, so
/// consumers never observe events for state that reconciliation later
/// rolls back.
#[derive(Default)]
struct ProjectionEvents {
    roster: Option<GroupRosterUpdatedEvent>,
    receipts: Vec<GroupMessageReceiptUpdatedEvent>,
    message: Option<db::Message>,
    /// The record dissolved the group; replay must stop here.
    dissolved: bool,
}

/// The single canonical projection of a group record into materialized
/// state. Both the initial apply path and `rebuild_group_materialized_state`
/// use exactly this function, so a valid log always replays into the same
/// state. `local_peer_id` is the explicit local identity: messages authored
/// by the local peer are stored with the `Me` display identity used by the
/// GUI/TUI. Every storage error is propagated — a projection must never
/// silently be partial.
fn project_record_effects(
    conn: &rusqlite::Connection,
    record: &SignedGroupRecord,
    local_peer_id: Option<&str>,
) -> anyhow::Result<ProjectionEvents> {
    let mut events = ProjectionEvents::default();
    let group_id = record.group_id();
    match record.body() {
        GroupRecordBody::GroupCreated {
            name, image_hash, ..
        } => {
            db::upsert_chat(conn, group_id, name, true)?;
            db::update_chat_image_hash(conn, group_id, image_hash.as_deref())?;
            if let Some(image_hash) = image_hash {
                ensure_incomplete_file_row(conn, image_hash)?;
                db::upsert_group_file_source(conn, group_id, image_hash, record.author_peer_id())?;
            }
            ensure_peer(conn, record.author_peer_id(), "group")?;
            db::upsert_chat_member_state(
                conn,
                group_id,
                record.author_peer_id(),
                "admin",
                "joined",
                None,
                Some(record.id()),
            )?;
            events.roster = Some(GroupRosterUpdatedEvent {
                group_id: group_id.to_string(),
                peer_id: record.author_peer_id().to_string(),
                membership_state: "joined".to_string(),
            });
        }
        GroupRecordBody::MemberInvited { peer_id, role } => {
            ensure_peer(conn, peer_id, "group")?;
            db::upsert_chat_member_state(
                conn,
                group_id,
                peer_id,
                role,
                "invited",
                Some(record.author_peer_id()),
                Some(record.id()),
            )?;
            events.roster = Some(GroupRosterUpdatedEvent {
                group_id: group_id.to_string(),
                peer_id: peer_id.clone(),
                membership_state: "invited".to_string(),
            });
        }
        GroupRecordBody::MemberJoined { peer_id } => {
            ensure_peer(conn, peer_id, "group")?;
            db::upsert_chat_member_state(
                conn,
                group_id,
                peer_id,
                "member",
                "joined",
                None,
                Some(record.id()),
            )?;
            events.roster = Some(GroupRosterUpdatedEvent {
                group_id: group_id.to_string(),
                peer_id: peer_id.clone(),
                membership_state: "joined".to_string(),
            });
        }
        GroupRecordBody::MemberLeft { peer_id } => {
            db::remove_chat_member(conn, group_id, peer_id)?;
            events.roster = Some(GroupRosterUpdatedEvent {
                group_id: group_id.to_string(),
                peer_id: peer_id.clone(),
                membership_state: "left".to_string(),
            });
        }
        GroupRecordBody::GroupRenamed { name } => {
            db::upsert_chat(conn, group_id, name, true)?;
        }
        GroupRecordBody::GroupSettingsUpdated { .. } => {}
        GroupRecordBody::MemberRemoved { peer_id } => {
            db::remove_chat_member(conn, group_id, peer_id)?;
            events.roster = Some(GroupRosterUpdatedEvent {
                group_id: group_id.to_string(),
                peer_id: peer_id.clone(),
                membership_state: "removed".to_string(),
            });
        }
        GroupRecordBody::AdminTransferred { new_admin_peer_id } => {
            // Canonical role projection: the new admin is promoted and every
            // other joined member — including the transferring (old) admin —
            // becomes a regular member. Deriving roles from the roster keeps
            // the old admin from retaining the admin role after a replay.
            for member in db::get_group_roster(conn, group_id)? {
                if member.membership_state == "joined" {
                    let role = if member.peer_id == *new_admin_peer_id {
                        "admin"
                    } else {
                        "member"
                    };
                    db::upsert_chat_member_state(
                        conn,
                        group_id,
                        &member.peer_id,
                        role,
                        "joined",
                        member.invited_by.as_deref(),
                        member.last_event_id.as_deref(),
                    )?;
                }
            }
            events.roster = Some(GroupRosterUpdatedEvent {
                group_id: group_id.to_string(),
                peer_id: new_admin_peer_id.clone(),
                membership_state: "admin".to_string(),
            });
        }
        GroupRecordBody::GroupDissolved => {
            // Dissolution revokes outstanding invites and removes the chat.
            // Records after a dissolution can never be valid, so replay
            // must stop here instead of resurrecting the group.
            db::revoke_group_invites(conn, group_id)?;
            db::delete_group_chat(conn, group_id)?;
            events.dissolved = true;
        }
        GroupRecordBody::Message {
            content_type,
            text_content,
            file_hash,
            sender_alias,
        } => {
            let is_local = local_peer_id == Some(record.author_peer_id());
            ensure_peer(conn, record.author_peer_id(), "group")?;
            db::upsert_chat(
                conn,
                group_id,
                &chat_kind::default_group_name(group_id),
                true,
            )?;
            db::upsert_chat_member_state(
                conn,
                group_id,
                "Me",
                "member",
                "joined",
                None,
                None,
            )?;
            db::upsert_chat_member_state(
                conn,
                group_id,
                record.author_peer_id(),
                "member",
                "joined",
                None,
                None,
            )?;
            let mut db_msg = group_record_to_db_message(
                record,
                *content_type,
                text_content.clone(),
                file_hash.clone(),
                sender_alias.clone(),
            );
            if is_local {
                // Local sends are displayed as "Me" for GUI/TUI mapping.
                db_msg.peer_id = "Me".to_string();
            }
            if let Some(file_hash) = file_hash {
                ensure_incomplete_file_row(conn, file_hash)?;
                db::upsert_group_file_source(conn, group_id, file_hash, record.author_peer_id())?;
            }
            match db::insert_message(conn, &db_msg) {
                Ok(()) => events.message = Some(db_msg),
                Err(err) => {
                    let duplicate = err
                        .to_string()
                        .to_ascii_lowercase()
                        .contains("unique constraint");
                    if !duplicate {
                        return Err(err);
                    }
                }
            }
        }
        GroupRecordBody::Receipt {
            message_ids,
            status,
        } => {
            for message_id in message_ids {
                db::upsert_group_message_receipt(
                    conn,
                    group_id,
                    message_id,
                    record.author_peer_id(),
                    status.as_str(),
                    record.timestamp(),
                )?;
                events.receipts.push(GroupMessageReceiptUpdatedEvent {
                    group_id: group_id.to_string(),
                    message_id: message_id.clone(),
                    peer_id: record.author_peer_id().to_string(),
                    status: status.as_str().to_string(),
                });
            }
        }
        GroupRecordBody::Head { .. } => {}
        GroupRecordBody::FileAvailability { file_hash } => {
            ensure_incomplete_file_row(conn, file_hash)?;
            db::upsert_group_file_source(conn, group_id, file_hash, record.author_peer_id())?;
        }
    }
    Ok(events)
}

/// Apply one record inside the caller's open transaction. Returns whether
/// the record was applied (newly verified) and appends the events it
/// produced to `events`; events are emitted by the caller after commit.
fn apply_signed_record_locked(
    conn: &rusqlite::Connection,
    local_peer_id: Option<&str>,
    record: &SignedGroupRecord,
    verified: bool,
    events: &mut Vec<CoreEvent>,
) -> anyhow::Result<bool> {
    if !verified {
        if db::group_record_exists(conn, record.id()) {
            return Ok(false);
        }
        return db::insert_group_record(conn, record, false, true);
    }

    // The caller's `verified` flag is a claim, not a proof: it means "the
    // ingress path checked this", and nothing else. Validation below only
    // reasons about causal structure, so without this check any caller that
    // passes `verified = true` for an unsigned or forged record — the invite
    // payload's `related_records`, or the pending-retry loop re-running rows
    // that originally failed signature verification — would apply it with
    // full policy effect.
    if !record.verify() {
        return Err(anyhow!(
            "Group record {} failed signature verification",
            record.id()
        ));
    }

    let existing_state = group_record_state(conn, record.id())?;
    if let Some((_, false)) = existing_state {
        return Ok(false);
    }

    let disposition = match validate_group_record(conn, record) {
        Ok(d) => d,
        Err(err) => {
            // A locally reserved pending row that fails hard validation
            // (e.g. counter fork or not building on head) must be
            // removed so the position does not stay blocked.
            if let Some((false, true)) = existing_state {
                let _ = conn.execute(
                    "DELETE FROM group_records WHERE id = ?1",
                    [record.id()],
                );
            }
            return Err(err);
        }
    };
    let record_applied = match disposition {
        RecordDisposition::Apply => {
            if existing_state.is_some() {
                mark_group_record_verified(conn, record.id())?;
                true
            } else {
                db::insert_group_record(conn, record, true, false)?
            }
        }
        RecordDisposition::PendingDependency => {
            if existing_state.is_none() {
                db::insert_group_record(conn, record, false, true)?;
            }
            return Ok(false);
        }
    };

    let projection = project_record_effects(conn, record, local_peer_id)?;
    events.push(CoreEvent::GroupRecordApplied(GroupRecordAppliedEvent {
        group_id: record.group_id().to_string(),
        record_id: record.id().to_string(),
        record_type: record.body().kind().to_string(),
    }));
    if let Some(roster) = projection.roster {
        events.push(CoreEvent::GroupRosterUpdated(roster));
    }
    for receipt in projection.receipts {
        events.push(CoreEvent::GroupMessageReceiptUpdated(receipt));
    }
    if let Some(message) = projection.message {
        events.push(CoreEvent::MessageReceived(message));
    }
    Ok(record_applied)
}

pub fn apply_signed_record(
    app_state: &AppState,
    event_sink: Option<&SharedCoreEventSink>,
    record: &SignedGroupRecord,
    verified: bool,
) -> anyhow::Result<bool> {
    let mut events: Vec<CoreEvent> = Vec::new();
    let record_applied;
    {
        let conn = app_state.db_conn.lock().map_err(|e| anyhow!(e.to_string()))?;
        conn.execute("BEGIN IMMEDIATE", [])?;
        // Verification, stale-branch demotion, canonical rebuild, and the
        // record's materialized projection all happen in ONE transaction: if
        // reconciliation fails, the record and its side effects roll back
        // together and no consumer ever observes events for them.
        let outcome = (|| {
            let applied = apply_signed_record_locked(
                &conn,
                app_state.local_peer_id(),
                record,
                verified,
                &mut events,
            )?;
            // Reconciliation is only needed when the policy frontier can
            // change; message/receipt traffic cannot invalidate any other
            // record, so skip the full-history rescan for it.
            if applied && is_policy_changing(record.body()) {
                revalidate_verified_records(&conn, app_state, record.group_id(), &mut events)?;
            }
            Ok::<bool, anyhow::Error>(applied)
        })();
        match outcome {
            Ok(applied) => record_applied = applied,
            Err(err) => {
                let _ = conn.execute("ROLLBACK", []);
                return Err(err);
            }
        }
        conn.execute("COMMIT", [])
            .map_err(|e| anyhow!("failed to commit group record {}: {e}", record.id()))?;
    }

    // The transaction has committed — only now emit events and retry
    // pending dependents.
    if let Some(sink) = event_sink {
        for event in events {
            sink.emit(event);
        }
    }
    if record_applied {
        retry_pending_group_records(app_state, event_sink, record.group_id());
    }

    Ok(record_applied)
}


/// Reconcile any verified records that became stale after a new policy
/// record arrived. Runs inside the caller's transaction. A record is kept
/// verified only if it still satisfies the same rules as fresh validation:
/// parents present, counter follows its parents, and (for authorization-
/// sensitive records) its parent closure dominates **every** policy-changing
/// record at a lower counter. Demoted records move back to pending and the
/// whole group's materialized state is rebuilt from the surviving valid set.
fn revalidate_verified_records(
    conn: &rusqlite::Connection,
    app_state: &AppState,
    group_id: &str,
    events: &mut Vec<CoreEvent>,
) -> anyhow::Result<()> {
    let all_verified = db::get_all_verified_group_records_ordered(conn, group_id)?;
    let mut valid_ids: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut valid_records: Vec<SignedGroupRecord> = Vec::new();
    let mut to_demote = Vec::new();

    // Dominance is tracked as a bitset of *policy ancestors* instead of a
    // parent-closure walk per record, which previously cost one DB traversal
    // plus one SQL scan per record (at least quadratic, approaching cubic).
    // `all_verified` is ordered by (counter, author, id), so a record's
    // parents — which always hold a strictly smaller counter — are processed
    // before it, and a policy ancestor of `rec` is therefore always indexed
    // below `required`. That is what makes the popcount test exact: the mask
    // can only hold bits in [0, required), so it equals that whole prefix
    // precisely when its popcount is `required`.
    let mut policy_index: std::collections::HashMap<&str, usize> =
        std::collections::HashMap::new();
    let mut policy_counters: Vec<u64> = Vec::new();
    for rec in &all_verified {
        if is_policy_changing(rec.body()) {
            policy_index.insert(rec.id(), policy_counters.len());
            policy_counters.push(rec.lamport_counter());
        }
    }
    let words = policy_counters.len().div_ceil(64);
    // Record id -> bitset of the policy records in its parent closure.
    let mut policy_ancestors: std::collections::HashMap<&str, Vec<u64>> =
        std::collections::HashMap::new();
    // Number of policy records strictly below the current record's counter.
    let mut required = 0usize;

    for rec in &all_verified {
        while required < policy_counters.len()
            && policy_counters[required] < rec.lamport_counter()
        {
            required += 1;
        }
        // Parents must all be present and in the survivor set.
        let parents_ok = rec.unsigned.parents.iter().all(|p| valid_ids.contains(p));
        if !parents_ok {
            to_demote.push(rec.id().to_string());
            continue;
        }
        // Causal counter must directly follow the max parent.
        if !rec.unsigned.parents.is_empty() {
            let mut max_parent = 0u64;
            for pid in &rec.unsigned.parents {
                if let Some(parent) = valid_records.iter().find(|r| r.id() == pid) {
                    max_parent = max_parent.max(parent.lamport_counter());
                }
            }
            if rec.lamport_counter() != max_parent.saturating_add(1) {
                to_demote.push(rec.id().to_string());
                continue;
            }
        } else if !matches!(rec.body(), GroupRecordBody::GroupCreated { .. }) {
            // Non-creation with empty parents cannot be valid.
            to_demote.push(rec.id().to_string());
            continue;
        }
        // The policy ancestors of this record: the union of its parents'
        // policy ancestors, plus any parent that is itself a policy record.
        // Computed for every record, including exempt ones, because an exempt
        // record still passes its ancestry on to its children.
        let mut mask = vec![0u64; words];
        for pid in &rec.unsigned.parents {
            if let Some(parent_mask) = policy_ancestors.get(pid.as_str()) {
                for (word, source) in mask.iter_mut().zip(parent_mask.iter()) {
                    *word |= *source;
                }
            }
            if let Some(bit) = policy_index.get(pid.as_str()).copied() {
                mask[bit / 64] |= 1u64 << (bit % 64);
            }
        }
        // Multi-head stale-branch check mirroring validate_group_record. This
        // has to test the whole closure, not just the direct parents: it is
        // tempting to require only the policy records at counter-1, but that
        // is not equivalent, because Head and FileAvailability records are
        // exempt and can therefore be non-dominating parents — a child's
        // closure is then not guaranteed to cover its grandparents'.
        if !matches!(
            rec.body(),
            GroupRecordBody::GroupCreated { .. }
                | GroupRecordBody::Head { .. }
                | GroupRecordBody::FileAvailability { .. }
        ) && mask.iter().map(|word| word.count_ones() as usize).sum::<usize>() != required
        {
            to_demote.push(rec.id().to_string());
            continue;
        }
        policy_ancestors.insert(rec.id(), mask);
        valid_ids.insert(rec.id().to_string());
        valid_records.push(rec.clone());
    }

    if to_demote.is_empty() {
        return Ok(());
    }

    for stale_id in &to_demote {
        conn.execute(
            "UPDATE group_records SET verified = 0, pending = 1 WHERE id = ?1",
            [stale_id],
        )?;
    }
    rebuild_group_materialized_state(conn, app_state, group_id, &valid_records)?;
    for stale_id in &to_demote {
        events.push(CoreEvent::GroupRecordApplied(GroupRecordAppliedEvent {
            group_id: group_id.to_string(),
            record_id: stale_id.clone(),
            record_type: "rebuilt".to_string(),
        }));
    }
    Ok(())
}

/// Rebuild all materialized group state from the valid set using the same
/// canonical projection used by the apply path (`project_record_effects`),
/// so a valid log always replays into exactly the same state. Dissolution
/// stops the replay, transfer demotes the old admin, local messages keep the
/// `Me` identity, and every storage error is propagated.
fn rebuild_group_materialized_state(
    conn: &rusqlite::Connection,
    app_state: &AppState,
    group_id: &str,
    valid_records: &[SignedGroupRecord],
) -> anyhow::Result<()> {
    let local_peer_id = app_state.local_peer_id().map(str::to_string);
    conn.execute("DELETE FROM messages WHERE chat_id = ?1", [group_id])?;
    conn.execute("DELETE FROM group_message_receipts WHERE group_id = ?1", [group_id])?;
    conn.execute("DELETE FROM group_file_sources WHERE group_id = ?1", [group_id])?;
    conn.execute("DELETE FROM chat_peers WHERE chat_id = ?1", [group_id])?;

    for rec in valid_records {
        let projection = project_record_effects(conn, rec, local_peer_id.as_deref())?;
        // Dissolution removes the group; nothing after it may be replayed.
        if projection.dissolved {
            break;
        }
    }
    Ok(())
}

pub fn store_incoming_invite(
    app_state: &AppState,
    event_sink: Option<&SharedCoreEventSink>,
    invite: &GroupInvitePayload,
) -> anyhow::Result<()> {
    if !invite.invite_record.verify() {
        return Err(anyhow!("Group invite signature could not be verified"));
    }
    let conn = app_state.db_conn.lock().map_err(|e| anyhow!(e.to_string()))?;
    db::upsert_group_invite(&conn, invite, "pending")?;
    drop(conn);

    if let Some(event_sink) = event_sink {
        event_sink.emit(CoreEvent::GroupInviteReceived(
            crate::events::GroupInviteReceivedEvent {
                invite_id: invite.invite_id.clone(),
                group_id: invite.group_id.clone(),
                group_name: invite.group_name.clone(),
                inviter_peer_id: invite.inviter_peer_id.clone(),
            },
        ));
    }
    Ok(())
}

async fn send_group_message_record(
    app_state: &AppState,
    network_state: &NetworkState,
    group_id: String,
    content_type: GroupContentType,
    text_content: Option<String>,
    file_hash: Option<String>,
    sender_alias: Option<String>,
) -> anyhow::Result<String> {
    let keypair = load_or_create_local_keypair(app_state).await?;
    let local_peer_id = PeerId::from_public_key(&keypair.public()).to_string();
    if let Ok(policy) = get_group_policy(app_state, &group_id) {
        if !policy.active_members.contains(&local_peer_id) {
            return Err(anyhow!("Only active group members can send group messages"));
        }
    }
    let record = sign_record(
        app_state,
        &keypair,
        group_id.clone(),
        GroupRecordBody::Message {
            content_type,
            text_content: text_content.clone(),
            file_hash: file_hash.clone(),
            sender_alias: sender_alias.clone(),
        },
    )?;
    // Finalize through the shared apply path so the message is validated
    // against its exact parents under the same lock.
    crate::chat::group::apply_signed_record(app_state, None, &record, true)?;
    // Local sends must be visible as "Me" for GUI/TUI sender/status mapping.
    // `apply` inserted with the cryptographic author, so update that row to
    // the local display identity and propagate any failure.
    {
        let conn = app_state.db_conn.lock().map_err(|e| anyhow!(e.to_string()))?;
        conn.execute(
            "UPDATE messages SET peer_id = 'Me' WHERE id = ?1",
            [record.id()],
        )?;
        // Verify the row now has peer_id='Me'.
        let ok: bool = conn
            .query_row(
                "SELECT 1 FROM messages WHERE id = ?1 AND peer_id = 'Me'",
                [record.id()],
                |_| Ok(true),
            )
            .unwrap_or(false);
        if !ok {
            return Err(anyhow!("Local message not stored as Me"));
        }
    }
    let msg_id = record.id().to_string();
    send_network_command(network_state, NetworkCommand::PublishGroupRecord { record }).await?;
    Ok(msg_id)
}

fn group_record_to_db_message(
    record: &SignedGroupRecord,
    content_type: GroupContentType,
    text_content: Option<String>,
    file_hash: Option<String>,
    sender_alias: Option<String>,
) -> db::Message {
    let text_content = match content_type {
        GroupContentType::Text => text_content,
        GroupContentType::Image | GroupContentType::Sticker => None,
        GroupContentType::Document => Some(
            text_content
                .filter(|name| !name.trim().is_empty())
                .unwrap_or_else(|| "document".to_string()),
        ),
        GroupContentType::Video => Some(
            text_content
                .filter(|name| !name.trim().is_empty())
                .unwrap_or_else(|| "video".to_string()),
        ),
        GroupContentType::Audio => Some(
            text_content
                .filter(|name| !name.trim().is_empty())
                .unwrap_or_else(|| "audio".to_string()),
        ),
    };

    db::Message {
        id: record.id().to_string(),
        chat_id: record.group_id().to_string(),
        peer_id: record.author_peer_id().to_string(),
        timestamp: record.timestamp(),
        content_type: content_type.as_str().to_string(),
        text_content,
        file_hash,
        status: "delivered".to_string(),
        content_metadata: None,
        sender_alias,
    }
}

/// The next causal position for a record in `group_id`: one past every
/// counter observed so far — verified *and* pending records alike (the
/// Lamport receive rule). Pending records count because they already occupy
/// a causal position from their author's perspective; ignoring them could
/// reissue the same position and fork the order.
pub fn next_group_record_counter(conn: &rusqlite::Connection, group_id: &str) -> u64 {
    db::get_group_max_lamport_counter(conn, group_id)
        .unwrap_or(0)
        .saturating_add(1)
}

/// Collect the transitive parent closure of `parents` (verified records only;
/// missing parents make the record pending). Used to evaluate authorization
/// against the exact causal snapshot the author built on.
fn collect_parent_closure(
    conn: &rusqlite::Connection,
    group_id: &str,
    parents: &[String],
) -> anyhow::Result<Vec<SignedGroupRecord>> {
    let mut seen = std::collections::HashSet::new();
    let mut stack: Vec<String> = parents.to_vec();
    let mut out = Vec::new();
    while let Some(id) = stack.pop() {
        if !seen.insert(id.clone()) {
            continue;
        }
        let Some(record) = db::get_group_record(conn, &id)? else {
            continue;
        };
        if record.group_id() != group_id {
            return Err(anyhow!(
                "Parent {} belongs to different group {}",
                record.id(),
                record.group_id()
            ));
        }
        for parent in &record.unsigned.parents {
            if !seen.contains(parent) {
                stack.push(parent.clone());
            }
        }
        out.push(record);
    }
    Ok(out)
}

fn sign_record(
    app_state: &AppState,
    keypair: &identity::Keypair,
    group_id: String,
    body: GroupRecordBody,
) -> anyhow::Result<SignedGroupRecord> {
    sign_record_at(
        app_state,
        keypair,
        group_id,
        timestamp_now(),
        Vec::new(),
        body,
    )
}

fn sign_record_at(
    app_state: &AppState,
    keypair: &identity::Keypair,
    group_id: String,
    timestamp: i64,
    parents: Vec<String>,
    body: GroupRecordBody,
) -> anyhow::Result<SignedGroupRecord> {
    let is_created = matches!(body, GroupRecordBody::GroupCreated { .. });
    // Reserve the causal position atomically under the DB lock so two
    // concurrent local operations cannot sign the same (author,counter).
    // Retry on unique-index conflict (INSERT OR IGNORE returns 0).
    for _ in 0..5 {
        let conn = app_state.db_conn.lock().map_err(|e| anyhow!(e.to_string()))?;
        let parents_to_use = if is_created {
            Vec::new()
        } else if parents.is_empty() {
            db::get_group_head_ids(&conn, &group_id)?
        } else {
            parents.clone()
        };
        if !is_created && parents_to_use.is_empty() {
            return Err(anyhow!("Group {group_id} has no head to build on"));
        }
        let lamport_counter = if is_created {
            1
        } else {
            let mut max_parent = 0u64;
            let mut missing = false;
            for pid in &parents_to_use {
                if let Some(rec) = db::get_group_record(&conn, pid)? {
                    max_parent = max_parent.max(rec.lamport_counter());
                } else {
                    missing = true;
                    break;
                }
            }
            if missing {
                return Err(anyhow!("Parent not found for group {group_id}"));
            }
            let global_max = db::get_group_max_lamport_counter(&conn, &group_id).unwrap_or(0);
            if max_parent != global_max {
                return Err(anyhow!(
                    "Group record must build on current head {global_max}, got parent max {max_parent}"
                ));
            }
            max_parent.saturating_add(1)
        };
        if lamport_counter == 0 || lamport_counter == u64::MAX {
            return Err(anyhow!("Group record counter exhausted"));
        }
        let record = SignedGroupRecord::new(
            keypair,
            group_id.clone(),
            format!("group-rec-{}-{}", timestamp, rand::random::<u32>()),
            timestamp,
            parents_to_use.clone(),
            lamport_counter,
            body.clone(),
        )?;
        let payload_json = serde_json::to_string(&record)?;
        let inserted = conn.execute(
            "INSERT OR IGNORE INTO group_records (id, group_id, record_type, author_peer_id, timestamp, lamport_counter, payload_json, public_key_b64, signature_b64, verified, pending, received_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            (
                record.id(),
                record.group_id(),
                record.body().kind(),
                record.author_peer_id(),
                record.timestamp(),
                record.lamport_counter() as i64,
                payload_json,
                &record.public_key_b64,
                &record.signature_b64,
                0,
                1,
                crate::storage::db::unix_now(),
            ),
        )?;
        if inserted == 0 {
            // Another concurrent local operation reserved the same
            // (group, author, counter) — retry with the new frontier.
            continue;
        }
        return Ok(record);
    }
    Err(anyhow!("Failed to allocate causal counter after retries"))
}

pub async fn load_or_create_local_keypair(
    app_state: &AppState,
) -> anyhow::Result<identity::Keypair> {
    let config_manager = app_state.config_manager.lock().await;
    let mut config = config_manager.load().await.unwrap_or_default();
    let cache_local_peer_id = |app_state: &AppState, keypair: &identity::Keypair| {
        let _ = app_state.local_peer_id.set(PeerId::from_public_key(&keypair.public()).to_string());
    };
    if let Some(ref key_b64) = config.user.libp2p_keypair {
        if let Ok(key_bytes) = BASE64.decode(key_b64) {
            if let Ok(keypair) = identity::Keypair::from_protobuf_encoding(&key_bytes) {
                cache_local_peer_id(app_state, &keypair);
                return Ok(keypair);
            }
        }
    }

    let keypair = identity::Keypair::generate_ed25519();
    let key_bytes = keypair
        .to_protobuf_encoding()
        .context("encode generated libp2p keypair")?;
    config.user.libp2p_keypair = Some(BASE64.encode(&key_bytes));
    config_manager.save(&config).await?;
    cache_local_peer_id(app_state, &keypair);
    Ok(keypair)
}

fn timestamp_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

async fn send_network_command(
    network_state: &NetworkState,
    command: NetworkCommand,
) -> anyhow::Result<()> {
    let tx = network_state.sender.lock().await;
    tx.send(command)
        .await
        .map_err(|_| anyhow!("network command channel is closed"))
}

fn ensure_peer(conn: &rusqlite::Connection, peer_id: &str, method: &str) -> anyhow::Result<()> {
    if !db::is_peer(conn, peer_id) {
        db::add_peer(conn, peer_id, None, None, method)?;
    }
    Ok(())
}

fn ensure_incomplete_file_row(conn: &rusqlite::Connection, file_hash: &str) -> anyhow::Result<()> {
    let exists: bool = conn
        .query_row(
            "SELECT 1 FROM files WHERE file_hash = ?1",
            [file_hash],
            |_| Ok(true),
        )
        .unwrap_or(false);
    if !exists {
        conn.execute(
            "INSERT INTO files (file_hash, file_name, mime_type, size_bytes, is_complete)
             VALUES (?1, NULL, 'application/octet-stream', 0, 0)",
            [file_hash],
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::HashMap;

    thread_local! {
        static GROUP_LAST_ID: RefCell<HashMap<String, String>> = RefCell::new(HashMap::new());
        static GROUP_LAST_COUNTER: RefCell<HashMap<String, u64>> = RefCell::new(HashMap::new());
    }

    fn app_state() -> AppState {
        use crate::storage::config::ConfigManager;
        use std::sync::Arc;
        use tokio::sync::Mutex;

        let app_dir = tempfile::tempdir_in("/tmp").expect("temp").keep();
        let conn = rusqlite::Connection::open_in_memory().expect("in-memory db");
        db::create_tables(&conn).expect("schema");
        AppState {
            config_manager: Arc::new(Mutex::new(ConfigManager::new(app_dir.clone()))),
            db_conn: Arc::new(std::sync::Mutex::new(conn)),
            app_dir,
            local_peer_id: Arc::new(std::sync::OnceLock::new()),
        }
    }

    fn keypair() -> identity::Keypair {
        identity::Keypair::generate_ed25519()
    }

    fn peer_id(keypair: &identity::Keypair) -> String {
        PeerId::from_public_key(&keypair.public()).to_string()
    }

    /// A signed v3 record whose causal position is `counter`. The timestamp
    /// is deliberately derived from the counter so tests that pass skewed or
    /// tied timestamps explicitly use [`signed_at`] instead.
    fn signed(
        keypair: &identity::Keypair,
        group_id: &str,
        id: &str,
        counter: u64,
        body: GroupRecordBody,
    ) -> SignedGroupRecord {
        signed_at(
            keypair,
            group_id,
            id,
            1_700_000_000 + counter as i64,
            counter,
            body,
        )
    }

    /// Like [`signed`], but with an explicit wall-clock timestamp — used to
    /// prove that skew, ties, and backdating cannot influence authorization.
    fn signed_at(
        keypair: &identity::Keypair,
        group_id: &str,
        id: &str,
        timestamp: i64,
        counter: u64,
        body: GroupRecordBody,
    ) -> SignedGroupRecord {
        let is_created = matches!(body, GroupRecordBody::GroupCreated { .. });
        let parents = if is_created {
            Vec::new()
        } else {
            GROUP_LAST_ID.with(|m| m.borrow().get(group_id).cloned().map(|id| vec![id]).unwrap_or_default())
        };
        // For out-of-order tests that create a record with a counter that
        // does not follow the current head, the helper would otherwise parent
        // the wrong head and make the record depend on an unrelated pending
        // record. In that case the test should construct the record explicitly
        // via `SignedGroupRecord::new` with the intended parents.
        let record = SignedGroupRecord::new(
            keypair,
            group_id.to_string(),
            format!("test-{id}-{}", rand::random::<u64>()),
            timestamp,
            parents.clone(),
            counter,
            body,
        )
        .expect("record");
        GROUP_LAST_ID.with(|m| m.borrow_mut().insert(group_id.to_string(), record.id().to_string()));
        GROUP_LAST_COUNTER.with(|m| m.borrow_mut().insert(group_id.to_string(), counter));
        record
    }

    fn apply(app_state: &AppState, record: &SignedGroupRecord) -> anyhow::Result<bool> {
        apply_signed_record(app_state, None, record, true)
    }

    /// An exempt record cannot be used to launder a stale branch.
    ///
    /// `Head` and `FileAvailability` are deliberately exempt from the
    /// dominance check, so such a record may itself omit a concurrent policy
    /// head. A child must therefore still be tested against its *whole*
    /// closure: requiring only the policy records at counter-1 as direct
    /// parents would let a removed member keep posting through an exempt
    /// parent that never saw their removal.
    #[test]
    fn exempt_parent_cannot_launder_a_stale_branch() {
        let app = app_state();
        GROUP_LAST_ID.with(|m| m.borrow_mut().clear());
        GROUP_LAST_COUNTER.with(|m| m.borrow_mut().clear());
        let founder = keypair();
        let member = keypair();
        let member_id = peer_id(&member);
        let group_id = chat_kind::generate_group_chat_id();

        let record = |author: &identity::Keypair,
                      id: &str,
                      counter: u64,
                      parents: Vec<String>,
                      body: GroupRecordBody| {
            SignedGroupRecord::new(
                author,
                group_id.clone(),
                format!("test-{id}-{}-{}", id, rand::random::<u64>()),
                1_700_000_000 + counter as i64,
                parents,
                counter,
                body,
            )
            .expect("record")
        };

        let created = record(
            &founder,
            "created",
            1,
            vec![],
            GroupRecordBody::GroupCreated {
                name: "Team".to_string(),
                settings: Some(GroupSettings {
                    members_can_invite: true,
                }),
                image_hash: None,
            },
        );
        let invited = record(
            &founder,
            "invite",
            2,
            vec![created.id().to_string()],
            GroupRecordBody::MemberInvited {
                peer_id: member_id.clone(),
                role: "member".to_string(),
            },
        );
        let joined = record(
            &member,
            "join",
            3,
            vec![invited.id().to_string()],
            GroupRecordBody::MemberJoined {
                peer_id: member_id.clone(),
            },
        );
        for rec in [&created, &invited, &joined] {
            apply(&app, rec).expect("prefix applies");
        }

        // Concurrent at counter 4: the removal, and a message that predates
        // it. Both are legitimate — neither saw the other.
        let removal = record(
            &founder,
            "removal",
            4,
            vec![joined.id().to_string()],
            GroupRecordBody::MemberRemoved {
                peer_id: member_id.clone(),
            },
        );
        let concurrent = record(
            &member,
            "concurrent-message",
            4,
            vec![joined.id().to_string()],
            GroupRecordBody::Message {
                content_type: crate::network::gossip::GroupContentType::Text,
                text_content: Some("before the removal".to_string()),
                file_hash: None,
                sender_alias: None,
            },
        );
        apply(&app, &removal).expect("removal applies");
        apply(&app, &concurrent).expect("concurrent message applies");

        // An exempt FileAvailability building only on the concurrent message:
        // it never saw the removal, which is allowed.
        let file = record(
            &member,
            "file",
            5,
            vec![concurrent.id().to_string()],
            GroupRecordBody::FileAvailability {
                file_hash: "hash".to_string(),
            },
        );
        apply(&app, &file).expect("exempt record applies");

        // A message descending only from that exempt parent still omits the
        // removal from its closure, so it must be rejected.
        let laundered = record(
            &member,
            "laundered",
            6,
            vec![file.id().to_string()],
            GroupRecordBody::Message {
                content_type: crate::network::gossip::GroupContentType::Text,
                text_content: Some("still here".to_string()),
                file_hash: None,
                sender_alias: None,
            },
        );
        assert!(
            apply(&app, &laundered).is_err(),
            "message routed around its own removal through an exempt parent"
        );
        assert!(
            !get_group_policy(&app, &group_id)
                .expect("policy")
                .active_members
                .contains(&member_id),
            "removed member is still active"
        );

        // The same log in the opposite arrival order: the laundered message
        // lands while the removal is still in flight, so it is accepted at
        // first and must be demoted when reconciliation runs. This is the
        // path that revalidates the whole log, so it is the one that would
        // catch a dominance shortcut in `revalidate_verified_records`.
        let late = app_state();
        for rec in [&created, &invited, &joined, &concurrent, &file, &laundered] {
            apply(&late, rec).expect("record applies before the removal");
        }
        assert!(
            db::get_all_verified_group_records_ordered(
                &late.db_conn.lock().expect("db"),
                &group_id
            )
            .expect("query")
            .iter()
            .any(|rec| rec.id() == laundered.id()),
            "laundered message should be verified until the removal arrives"
        );

        apply(&late, &removal).expect("removal applies late");

        let survivors = db::get_all_verified_group_records_ordered(
            &late.db_conn.lock().expect("db"),
            &group_id,
        )
        .expect("query");
        assert!(
            !survivors.iter().any(|rec| rec.id() == laundered.id()),
            "laundered message survived reconciliation"
        );
        assert!(
            !get_group_policy(&late, &group_id)
                .expect("policy")
                .active_members
                .contains(&member_id),
            "removed member is still active after late delivery"
        );
    }

    /// Regression guard: a record whose signature does not verify must never
    /// become verified. Network ingress stores signature-failed records as
    /// `verified = 0, pending = 1`, and `retry_pending_group_records` later
    /// re-runs `apply_signed_record` with `verified = true`; that path must
    /// not resurrect a forgery that validation alone cannot detect.
    #[test]
    fn forged_record_is_not_resurrected_by_pending_retry() {
        let app = app_state();
        GROUP_LAST_ID.with(|m| m.borrow_mut().clear());
        GROUP_LAST_COUNTER.with(|m| m.borrow_mut().clear());
        let (founder, member_b, _member_c, group_id, log) = test_team();
        for record in &log {
            apply(&app, record).expect("team log applies");
        }
        let head_id = log.last().expect("join_c").id().to_string();

        // The attacker signs with their own key but claims the admin's
        // identity, so `verify()` rejects it: the peer id derived from
        // `public_key_b64` does not match `author_peer_id`.
        let attacker = keypair();
        let mut forged = child(
            &attacker,
            &group_id,
            "forged-removal",
            6,
            vec![head_id.clone()],
            GroupRecordBody::MemberRemoved {
                peer_id: peer_id(&member_b),
            },
        );
        forged.unsigned.author_peer_id = peer_id(&founder);
        assert!(!forged.verify(), "fixture must be a genuine forgery");

        // Gossipsub ingress of a signature failure: stored pending.
        apply_signed_record(&app, None, &forged, false).expect("stored as pending");

        // Any later successful apply in this group drains the pending queue.
        let trigger = child(
            &member_b,
            &group_id,
            "trigger",
            6,
            vec![head_id],
            GroupRecordBody::Head {
                heads: Vec::new(),
            },
        );
        apply(&app, &trigger).expect("trigger applies");

        let verified = db::get_all_verified_group_records_ordered(
            &app.db_conn.lock().expect("db"),
            &group_id,
        )
        .expect("query");
        assert!(
            !verified.iter().any(|record| record.id() == forged.id()),
            "forged record was applied despite failing signature verification"
        );
        assert!(
            get_group_policy(&app, &group_id)
                .expect("policy")
                .active_members
                .contains(&peer_id(&member_b)),
            "forged removal took effect"
        );
    }

    #[test]
    fn invite_gated_join_requires_existing_invite_payload() {
        let app_state = app_state();
        GROUP_LAST_ID.with(|m| m.borrow_mut().clear());
        GROUP_LAST_COUNTER.with(|m| m.borrow_mut().clear());
        let conn = app_state.db_conn.lock().expect("db");
        let missing = db::get_group_invite_payload(&conn, "missing").expect("query");
        assert!(missing.is_none());
    }

    #[test]
    fn default_group_policy_makes_founder_admin_and_disables_member_invites() {
        let app_state = app_state();
        GROUP_LAST_ID.with(|m| m.borrow_mut().clear());
        GROUP_LAST_COUNTER.with(|m| m.borrow_mut().clear());
        let founder = keypair();
        let group_id = chat_kind::generate_group_chat_id();
        let created = signed(
            &founder,
            &group_id,
            "created",
            1,
            GroupRecordBody::GroupCreated {
                name: "Test".to_string(),
                settings: None,
                image_hash: None,
            },
        );

        assert!(apply(&app_state, &created).expect("apply"));
        let policy = get_group_policy(&app_state, &group_id).expect("policy");
        assert_eq!(policy.admin_peer_id, peer_id(&founder));
        assert!(!policy.settings.members_can_invite);
    }

    #[test]
    fn administrator_transfer_and_successor_order_are_deterministic() {
        let app_state = app_state();
        GROUP_LAST_ID.with(|m| m.borrow_mut().clear());
        GROUP_LAST_COUNTER.with(|m| m.borrow_mut().clear());
        let founder = keypair();
        let older = keypair();
        let newer = keypair();
        let group_id = chat_kind::generate_group_chat_id();
        let older_id = peer_id(&older);
        let newer_id = peer_id(&newer);
        let founder_id = peer_id(&founder);

        let records = [
            signed(&founder, &group_id, "created", 1, GroupRecordBody::GroupCreated {
                name: "Test".to_string(), settings: None, image_hash: None,
            }),
            signed(&founder, &group_id, "invite-older", 2, GroupRecordBody::MemberInvited {
                peer_id: older_id.clone(), role: "member".to_string(),
            }),
            signed(&older, &group_id, "join-older", 3, GroupRecordBody::MemberJoined {
                peer_id: older_id.clone(),
            }),
            signed(&founder, &group_id, "invite-newer", 4, GroupRecordBody::MemberInvited {
                peer_id: newer_id.clone(), role: "member".to_string(),
            }),
            signed(&newer, &group_id, "join-newer", 5, GroupRecordBody::MemberJoined {
                peer_id: newer_id.clone(),
            }),
        ];
        for record in &records {
            apply(&app_state, record).expect("membership record");
        }

        let policy = get_group_policy(&app_state, &group_id).expect("policy");
        assert_eq!(policy.automatic_successor_peer_id.as_deref(), Some(older_id.as_str()));

        apply(&app_state, &signed(
            &founder,
            &group_id,
            "transfer",
            6,
            GroupRecordBody::AdminTransferred { new_admin_peer_id: newer_id.clone() },
        )).expect("transfer");
        let policy = get_group_policy(&app_state, &group_id).expect("transferred policy");
        assert_eq!(policy.admin_peer_id, newer_id);
        assert_eq!(policy.automatic_successor_peer_id.as_deref(), Some(founder_id.as_str()));
    }

    #[test]
    fn sole_administrator_can_dissolve_and_later_records_are_rejected() {
        let app_state = app_state();
        GROUP_LAST_ID.with(|m| m.borrow_mut().clear());
        GROUP_LAST_COUNTER.with(|m| m.borrow_mut().clear());
        let founder = keypair();
        let group_id = chat_kind::generate_group_chat_id();
        apply(&app_state, &signed(&founder, &group_id, "created", 1, GroupRecordBody::GroupCreated {
            name: "Test".to_string(), settings: None, image_hash: None,
        })).expect("created");
        let dissolved = signed(&founder, &group_id, "dissolved", 2, GroupRecordBody::GroupDissolved);
        let dissolved_id = dissolved.id().to_string();
        apply(&app_state, &dissolved).expect("dissolved");

        let policy = get_group_policy(&app_state, &group_id).expect("policy");
        assert!(policy.dissolved);
        let error = apply(&app_state, &signed(&founder, &group_id, "message", 3, GroupRecordBody::Message {
            content_type: GroupContentType::Text,
            text_content: Some("too late".to_string()),
            file_hash: None,
            sender_alias: Some("Founder".to_string()),
        })).expect_err("records after dissolution must fail");
        assert!(error.to_string().contains("dissolved"));

        // A causally fresh record (new counter) carrying a backdated
        // wall-clock timestamp must still hit the dissolution tombstone:
        // timestamps cannot place a record before the dissolution anymore.
        // Use a different author and same causal parents to avoid fork collision
        // with the previous post-dissolution attempt.
        let outsider = keypair();
        let error = apply(
            &app_state,
            &{
                let id = format!("test-backdated-{}", rand::random::<u64>());
                SignedGroupRecord::new(
                    &outsider,
                    group_id.clone(),
                    id,
                    1,
                    vec![dissolved_id.clone()],
                    3,
                    GroupRecordBody::Message {
                        content_type: GroupContentType::Text,
                        text_content: Some("backdated after tombstone".to_string()),
                        file_hash: None,
                        sender_alias: None,
                    },
                )
                .expect("record")
            },
        )
        .expect_err("a known dissolution must reject backdated records too");
        assert!(error.to_string().contains("dissolved"));
    }

    #[test]
    fn authorization_order_follows_counters_not_timestamps() {
        // Wall-clock timestamps are display metadata: even with perverse,
        // tied, and descending clocks the causal counters decide who is
        // admin and who joins first, so skewed peers converge identically.
        let app_state = app_state();
        GROUP_LAST_ID.with(|m| m.borrow_mut().clear());
        GROUP_LAST_COUNTER.with(|m| m.borrow_mut().clear());
        let founder = keypair();
        let older = keypair();
        let newer = keypair();
        let group_id = chat_kind::generate_group_chat_id();
        let older_id = peer_id(&older);
        let newer_id = peer_id(&newer);

        // Counters ascend causally while timestamps descend and tie.
        let records = [
            signed_at(&founder, &group_id, "created", 900, 1, GroupRecordBody::GroupCreated {
                name: "Test".to_string(), settings: None, image_hash: None,
            }),
            signed_at(&founder, &group_id, "invite-older", 800, 2, GroupRecordBody::MemberInvited {
                peer_id: older_id.clone(), role: "member".to_string(),
            }),
            signed_at(&older, &group_id, "join-older", 700, 3, GroupRecordBody::MemberJoined {
                peer_id: older_id.clone(),
            }),
            signed_at(&founder, &group_id, "invite-newer", 700, 4, GroupRecordBody::MemberInvited {
                peer_id: newer_id.clone(), role: "member".to_string(),
            }),
            signed_at(&newer, &group_id, "join-newer", 100, 5, GroupRecordBody::MemberJoined {
                peer_id: newer_id.clone(),
            }),
        ];
        for record in &records {
            apply(&app_state, record).expect("membership record");
        }

        let policy = get_group_policy(&app_state, &group_id).expect("policy");
        assert_eq!(policy.admin_peer_id, peer_id(&founder));
        // The successor is the earliest *causal* join (counter 3), not the
        // one with the smallest timestamp.
        assert_eq!(
            policy.automatic_successor_peer_id.as_deref(),
            Some(older_id.as_str())
        );
    }

    #[test]
    fn concurrent_records_converge_identically_from_any_arrival_order() {
        // Genuinely concurrent records: two members invite at the same
        // causal position without having seen each other's record yet. The
        // (counter, author) order resolves them identically on every peer,
        // no matter the delivery order or clock readings.
        // Same identities and group on both peers — only arrival order differs.
        let founder = keypair();
        let alice = keypair();
        let bob = keypair();
        let group_id = chat_kind::generate_group_chat_id();
        let founder_id = peer_id(&founder);
        // Sequential prefix up to the head (counter 5).
        let mut records = vec![
            signed(&founder, &group_id, "created", 1, GroupRecordBody::GroupCreated {
                name: "Test".to_string(),
                settings: Some(GroupSettings { members_can_invite: true }),
                image_hash: None,
            }),
            signed(&founder, &group_id, "invite-alice", 2, GroupRecordBody::MemberInvited {
                peer_id: peer_id(&alice), role: "member".to_string(),
            }),
            signed(&alice, &group_id, "join-alice", 3, GroupRecordBody::MemberJoined {
                peer_id: peer_id(&alice),
            }),
            signed(&founder, &group_id, "invite-bob", 4, GroupRecordBody::MemberInvited {
                peer_id: peer_id(&bob), role: "member".to_string(),
            }),
            signed(&bob, &group_id, "join-bob", 5, GroupRecordBody::MemberJoined {
                peer_id: peer_id(&bob),
            }),
        ];
        let head_id = records.last().unwrap().id().to_string();
        // Genuinely concurrent: same parents (head) and same counter from
        // different authors — both must be accepted and converge.
        let concurrent_a = SignedGroupRecord::new(
            &alice,
            group_id.clone(),
            format!("test-concurrent-a-{}", rand::random::<u64>()),
            555,
            vec![head_id.clone()],
            6,
            GroupRecordBody::MemberInvited {
                peer_id: fixed_peer_id(1),
                role: "member".to_string(),
            },
        )
        .expect("record");
        let concurrent_b = SignedGroupRecord::new(
            &bob,
            group_id.clone(),
            format!("test-concurrent-b-{}", rand::random::<u64>()),
            999,
            vec![head_id.clone()],
            6,
            GroupRecordBody::MemberInvited {
                peer_id: fixed_peer_id(2),
                role: "member".to_string(),
            },
        )
        .expect("record");
        records.push(concurrent_a);
        records.push(concurrent_b);

        let app_a = app_state();
        for record in &records {
            apply(&app_a, record).expect("in-order apply");
        }
        let policy_in_order = get_group_policy(&app_a, &group_id).expect("policy");

        // Reverse arrival order on an independent peer — same records, same group.
        let mut reversed = records.clone();
        reversed.reverse();
        let app_b = app_state();
        for record in &reversed {
            apply(&app_b, record).expect("reversed apply");
        }
        let policy_reversed = get_group_policy(&app_b, &group_id).expect("policy");
        // Founder is the same on both peers.
        assert_eq!(founder_id, policy_in_order.admin_peer_id);
        assert_eq!(founder_id, policy_reversed.admin_peer_id);

        assert_eq!(
            policy_in_order.admin_peer_id, policy_reversed.admin_peer_id,
            "concurrent invites must converge to one admin"
        );
        let mut invited_a: Vec<String> =
            policy_in_order.invited_members.iter().cloned().collect();
        let mut invited_b: Vec<String> =
            policy_reversed.invited_members.iter().cloned().collect();
        invited_a.sort();
        invited_b.sort();
        assert_eq!(invited_a.len(), 2, "both concurrent invites survive");
        assert_eq!(invited_a, invited_b, "identical invited set from both orders");
        assert_eq!(policy_in_order.active_members.len(), 3);
        assert_eq!(policy_reversed.active_members.len(), 3);
    }

    /// A syntactically valid, deterministic peer id for concurrent-invite
    /// tests (both simulated peers must name the same invitees).
    fn fixed_peer_id(seed: u8) -> String {
        let keypair =
            identity::Keypair::ed25519_from_bytes([seed; 32]).expect("deterministic keypair");
        PeerId::from_public_key(&keypair.public()).to_string()
    }

    #[test]
    fn causal_counter_abuse_is_rejected() {
        let app_state = app_state();
        GROUP_LAST_ID.with(|m| m.borrow_mut().clear());
        GROUP_LAST_COUNTER.with(|m| m.borrow_mut().clear());
        let founder = keypair();
        let group_id = chat_kind::generate_group_chat_id();
        let created = signed(
            &founder,
            &group_id,
            "created",
            1,
            GroupRecordBody::GroupCreated {
                name: "Test".to_string(),
                settings: None,
                image_hash: None,
            },
        );
        let created_id = created.id().to_string();
        apply(&app_state, &created).expect("created");

        // A current-version record missing its causal counter is refused.
        let missing = SignedGroupRecord::new(
            &founder,
            group_id.clone(),
            format!("test-missing-{}", rand::random::<u64>()),
            1_700_000_100,
            Vec::new(),
            0,
            GroupRecordBody::MemberInvited {
                peer_id: fixed_peer_id(10),
                role: "member".to_string(),
            },
        )
        .expect("record");
        let error = apply(&app_state, &missing).expect_err("zero counter must fail");
        assert!(
            error.to_string().contains("causal counter"),
            "wrong error: {error}"
        );

        // The reserved sentinel is refused.
        let exhausted = SignedGroupRecord::new(
            &founder,
            group_id.clone(),
            format!("test-max-{}", rand::random::<u64>()),
            1_700_000_100,
            Vec::new(),
            u64::MAX,
            GroupRecordBody::MemberInvited {
                peer_id: fixed_peer_id(11),
                role: "member".to_string(),
            },
        )
        .expect("record");
        let error = apply(&app_state, &exhausted).expect_err("MAX sentinel must fail");
        assert!(error.to_string().contains("exhausted"), "wrong error: {error}");

        // A counter that leaps beyond the known frontier is refused.
        let leap = SignedGroupRecord::new(
            &founder,
            group_id.clone(),
            format!("test-leap-{}", rand::random::<u64>()),
            1_700_000_100,
            vec![created_id.clone()],
            10_000_000,
            GroupRecordBody::MemberInvited {
                peer_id: fixed_peer_id(12),
                role: "member".to_string(),
            },
        )
        .expect("record");
        let error = apply(&app_state, &leap).expect_err("far leap must fail");
        assert!(
            error.to_string().contains("leaps")
                || error.to_string().contains("directly follow"),
            "wrong error: {error}"
        );

        // Forking one causal position — two records by the same author at
        // the same counter — is a hard error (no pending, no silent win).
        let fork_a = signed(&founder, &group_id, "fork-a", 2, GroupRecordBody::MemberInvited {
            peer_id: fixed_peer_id(13),
            role: "member".to_string(),
        });
        apply(&app_state, &fork_a).expect("first fork slot wins");
        let fork_b = SignedGroupRecord::new(
            &founder,
            group_id.clone(),
            format!("test-fork-b-{}", rand::random::<u64>()),
            1_700_000_101,
            vec![created_id.clone()],
            2,
            GroupRecordBody::MemberInvited {
                peer_id: fixed_peer_id(14),
                role: "member".to_string(),
            },
        )
        .expect("record");
        let error = apply(&app_state, &fork_b).expect_err("forked position must fail");
        assert!(error.to_string().contains("forks"), "wrong error: {error}");
    }

    #[test]
    fn replayed_record_is_a_noop() {
        let app_state = app_state();
        GROUP_LAST_ID.with(|m| m.borrow_mut().clear());
        GROUP_LAST_COUNTER.with(|m| m.borrow_mut().clear());
        let founder = keypair();
        let group_id = chat_kind::generate_group_chat_id();
        let created = signed(&founder, &group_id, "created", 1, GroupRecordBody::GroupCreated {
            name: "Test".to_string(), settings: None, image_hash: None,
        });
        apply(&app_state, &created).expect("created");
        let policy_before = get_group_policy(&app_state, &group_id).expect("policy");

        // Replaying the same verified record is a no-op.
        assert!(!apply(&app_state, &created).expect("replay"));
        let policy_after = get_group_policy(&app_state, &group_id).expect("policy");
        assert_eq!(policy_before.admin_peer_id, policy_after.admin_peer_id);
        assert_eq!(policy_before.active_members, policy_after.active_members);
    }

    #[test]
    fn admin_transfer_chain_pending_until_predecessor_arrives() {
        // Admin A -> B (counter 4), then B -> C (counter 5). When the
        // successor's transfer (B -> C) arrives before its predecessor's
        // (A -> B), it must stay pending until the predecessor is applied —
        // and then the chain resolves on retry.
        let app_state = app_state();
        GROUP_LAST_ID.with(|m| m.borrow_mut().clear());
        GROUP_LAST_COUNTER.with(|m| m.borrow_mut().clear());
        let founder = keypair();
        let successor = keypair();
        let third = keypair();
        let successor_id = peer_id(&successor);
        let third_id = peer_id(&third);
        let group_id = chat_kind::generate_group_chat_id();
        for record in [
            signed(&founder, &group_id, "created", 1, GroupRecordBody::GroupCreated {
                name: "Test".to_string(), settings: None, image_hash: None,
            }),
            signed(&founder, &group_id, "invite-successor", 2, GroupRecordBody::MemberInvited {
                peer_id: successor_id.clone(), role: "member".to_string(),
            }),
            signed(&successor, &group_id, "join-successor", 3, GroupRecordBody::MemberJoined {
                peer_id: successor_id.clone(),
            }),
            signed(&founder, &group_id, "invite-third", 4, GroupRecordBody::MemberInvited {
                peer_id: third_id.clone(), role: "member".to_string(),
            }),
            signed(&third, &group_id, "join-third", 5, GroupRecordBody::MemberJoined {
                peer_id: third_id.clone(),
            }),
        ] {
            apply(&app_state, &record).expect("setup");
        }

        let transfer_to_successor = signed(
            &founder, &group_id, "to-successor", 6,
            GroupRecordBody::AdminTransferred { new_admin_peer_id: successor_id.clone() },
        );
        let transfer_to_third = signed(
            &successor, &group_id, "to-third", 7,
            GroupRecordBody::AdminTransferred { new_admin_peer_id: third_id.clone() },
        );

        // Out-of-order: the successor's own transfer arrives first. It
        // authorizes against the roster *before* the predecessor, where the
        // author is not yet admin, so it waits.
        assert!(!apply(&app_state, &transfer_to_third).expect("pending successor transfer"));
        let pending = {
            let conn = app_state.db_conn.lock().expect("db");
            db::get_group_record(&conn, transfer_to_third.id())
                .expect("query")
                .is_some()
        };
        assert!(pending, "must be stored pending");

        // Predecessor arrives: becomes admin, and retry cascades the pending
        // successor — final administrator converges to the third member.
        apply(&app_state, &transfer_to_successor).expect("predecessor");
        let policy = get_group_policy(&app_state, &group_id).expect("policy");
        assert_eq!(policy.admin_peer_id, third_id, "chain must resolve to the final successor");
    }

    #[test]
    fn leave_waits_for_parent_admin_transfer() {
        let app_state = app_state();
        GROUP_LAST_ID.with(|m| m.borrow_mut().clear());
        GROUP_LAST_COUNTER.with(|m| m.borrow_mut().clear());
        let founder = keypair();
        let successor = keypair();
        let founder_id = peer_id(&founder);
        let successor_id = peer_id(&successor);
        let group_id = chat_kind::generate_group_chat_id();
        for record in [
            signed(&founder, &group_id, "created", 1, GroupRecordBody::GroupCreated {
                name: "Test".to_string(), settings: None, image_hash: None,
            }),
            signed(&founder, &group_id, "invited", 2, GroupRecordBody::MemberInvited {
                peer_id: successor_id.clone(), role: "member".to_string(),
            }),
            signed(&successor, &group_id, "joined", 3, GroupRecordBody::MemberJoined {
                peer_id: successor_id.clone(),
            }),
        ] {
            apply(&app_state, &record).expect("setup record");
        }
        let transfer = signed(&founder, &group_id, "transfer-parent", 4,
            GroupRecordBody::AdminTransferred { new_admin_peer_id: successor_id.clone() });
        let leave = SignedGroupRecord::new(
            &founder,
            group_id.clone(),
            "leave-child".to_string(),
            5,
            vec![transfer.id().to_string()],
            5,
            GroupRecordBody::MemberLeft { peer_id: founder_id.clone() },
        ).expect("leave record");

        assert!(!apply(&app_state, &leave).expect("pending leave"));
        apply(&app_state, &transfer).expect("transfer");
        let policy = get_group_policy(&app_state, &group_id).expect("policy");
        assert_eq!(policy.admin_peer_id, successor_id);
        assert!(!policy.active_members.contains(&founder_id));
    }

    #[test]
    fn applying_group_created_stores_group_image_hash() {
        let app_state = app_state();
        GROUP_LAST_ID.with(|m| m.borrow_mut().clear());
        GROUP_LAST_COUNTER.with(|m| m.borrow_mut().clear());
        let founder = keypair();
        let group_id = chat_kind::generate_group_chat_id();
        let image_hash = "group-image-hash";

        apply(
            &app_state,
            &signed(
                &founder,
                &group_id,
                "created",
                1,
                GroupRecordBody::GroupCreated {
                    name: "Visual Group".to_string(),
                    settings: None,
                    image_hash: Some(image_hash.to_string()),
                },
            ),
        )
        .expect("created");

        let conn = app_state.db_conn.lock().expect("db");
        let chat = db::get_chat_list(&conn)
            .expect("chat list")
            .into_iter()
            .find(|chat| chat.id == group_id)
            .expect("group chat");
        assert_eq!(chat.image_hash.as_deref(), Some(image_hash));
        let (mime_type, is_complete): (String, i64) = conn
            .query_row(
                "SELECT mime_type, is_complete FROM files WHERE file_hash = ?1",
                [image_hash],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("placeholder file row");
        assert_eq!(mime_type, "application/octet-stream");
        assert_eq!(is_complete, 0);
        let sources = db::get_group_file_sources(&conn, &group_id, image_hash).expect("sources");
        assert_eq!(sources.len(), 1);
        assert_eq!(sources[0].peer_id, peer_id(&founder));
    }

    #[test]
    fn file_availability_creates_placeholder_file_row() {
        let app_state = app_state();
        GROUP_LAST_ID.with(|m| m.borrow_mut().clear());
        GROUP_LAST_COUNTER.with(|m| m.borrow_mut().clear());
        let founder = keypair();
        let group_id = chat_kind::generate_group_chat_id();
        let file_hash = "available-group-image-hash";

        apply(
            &app_state,
            &signed(
                &founder,
                &group_id,
                "created",
                1,
                GroupRecordBody::GroupCreated {
                    name: "Visual Group".to_string(),
                    settings: None,
                    image_hash: None,
                },
            ),
        )
        .expect("created");
        apply(
            &app_state,
            &signed(
                &founder,
                &group_id,
                "available",
                2,
                GroupRecordBody::FileAvailability {
                    file_hash: file_hash.to_string(),
                },
            ),
        )
        .expect("availability");

        let conn = app_state.db_conn.lock().expect("db");
        let is_complete: i64 = conn
            .query_row(
                "SELECT is_complete FROM files WHERE file_hash = ?1",
                [file_hash],
                |row| row.get(0),
            )
            .expect("placeholder file row");
        assert_eq!(is_complete, 0);
        let sources = db::get_group_file_sources(&conn, &group_id, file_hash).expect("sources");
        assert_eq!(sources.len(), 1);
        assert_eq!(sources[0].peer_id, peer_id(&founder));
    }

    #[test]
    fn non_admin_rename_is_rejected() {
        let app_state = app_state();
        GROUP_LAST_ID.with(|m| m.borrow_mut().clear());
        GROUP_LAST_COUNTER.with(|m| m.borrow_mut().clear());
        let founder = keypair();
        let member = keypair();
        let group_id = chat_kind::generate_group_chat_id();

        apply(
            &app_state,
            &signed(
                &founder,
                &group_id,
                "created",
                1,
                GroupRecordBody::GroupCreated {
                    name: "Test".to_string(),
                    settings: None,
                image_hash: None,
                },
            ),
        )
        .expect("created");
        apply(
            &app_state,
            &signed(
                &founder,
                &group_id,
                "invite-member",
                2,
                GroupRecordBody::MemberInvited {
                    peer_id: peer_id(&member),
                    role: "member".to_string(),
                },
            ),
        )
        .expect("invite");
        apply(
            &app_state,
            &signed(
                &member,
                &group_id,
                "member-joined",
                3,
                GroupRecordBody::MemberJoined {
                    peer_id: peer_id(&member),
                },
            ),
        )
        .expect("joined");

        let result = apply(
            &app_state,
            &signed(
                &member,
                &group_id,
                "bad-rename",
                4,
                GroupRecordBody::GroupRenamed {
                    name: "Owned".to_string(),
                },
            ),
        );

        assert!(result.is_err());
    }

    #[test]
    fn member_join_without_invite_is_pending_not_applied() {
        let app_state = app_state();
        GROUP_LAST_ID.with(|m| m.borrow_mut().clear());
        GROUP_LAST_COUNTER.with(|m| m.borrow_mut().clear());
        let founder = keypair();
        let member = keypair();
        let group_id = chat_kind::generate_group_chat_id();

        apply(
            &app_state,
            &signed(
                &founder,
                &group_id,
                "created",
                1,
                GroupRecordBody::GroupCreated {
                    name: "Test".to_string(),
                    settings: None,
                image_hash: None,
                },
            ),
        )
        .expect("created");

        let joined = signed(
            &member,
            &group_id,
            "member-joined",
            2,
            GroupRecordBody::MemberJoined {
                peer_id: peer_id(&member),
            },
        );

        assert!(apply(&app_state, &joined).is_err(), "join without invite must be hard error");
        let policy = get_group_policy(&app_state, &group_id).expect("policy");
        assert!(!policy.active_members.contains(&peer_id(&member)));
    }

    #[test]
    fn member_invite_is_accepted_when_setting_allows_it() {
        let app_state = app_state();
        GROUP_LAST_ID.with(|m| m.borrow_mut().clear());
        GROUP_LAST_COUNTER.with(|m| m.borrow_mut().clear());
        let founder = keypair();
        let member = keypair();
        let invited_by_member = keypair();
        let group_id = chat_kind::generate_group_chat_id();

        apply(
            &app_state,
            &signed(
                &founder,
                &group_id,
                "created",
                1,
                GroupRecordBody::GroupCreated {
                    name: "Test".to_string(),
                    settings: Some(GroupSettings {
                        members_can_invite: true,
                    }),
                image_hash: None,
                },
            ),
        )
        .expect("created");
        apply(
            &app_state,
            &signed(
                &founder,
                &group_id,
                "invite-member",
                2,
                GroupRecordBody::MemberInvited {
                    peer_id: peer_id(&member),
                    role: "member".to_string(),
                },
            ),
        )
        .expect("founder invite");
        apply(
            &app_state,
            &signed(
                &member,
                &group_id,
                "member-joined",
                3,
                GroupRecordBody::MemberJoined {
                    peer_id: peer_id(&member),
                },
            ),
        )
        .expect("member joined");

        assert!(apply(
            &app_state,
            &signed(
                &member,
                &group_id,
                "member-invite",
                4,
                GroupRecordBody::MemberInvited {
                    peer_id: peer_id(&invited_by_member),
                    role: "member".to_string(),
                },
            ),
        )
        .expect("member invite"));
        let policy = get_group_policy(&app_state, &group_id).expect("policy");
        assert!(policy
            .invited_members
            .contains(&peer_id(&invited_by_member)));
    }

    #[test]
    fn pending_join_applies_after_required_invite_arrives() {
        let app_state = app_state();
        GROUP_LAST_ID.with(|m| m.borrow_mut().clear());
        GROUP_LAST_COUNTER.with(|m| m.borrow_mut().clear());
        let founder = keypair();
        let member = keypair();
        let group_id = chat_kind::generate_group_chat_id();

        let created = signed(
            &founder,
            &group_id,
            "created",
            1,
            GroupRecordBody::GroupCreated {
                name: "Test".to_string(),
                settings: None,
                image_hash: None,
            },
        );
        apply(&app_state, &created).expect("created");
        // Create invite and join in causal order so parents are correct,
        // but apply join before invite to test pending.
        let invite = signed(
            &founder,
            &group_id,
            "invite-member",
            2,
            GroupRecordBody::MemberInvited {
                peer_id: peer_id(&member),
                role: "member".to_string(),
            },
        );
        let join = {
            // Join must explicitly parent the invite, not the head at signing
            // time (which would be invite itself if we used the helper's auto
            // parents, but we want to test the out-of-order apply).
            let invite_id = invite.id().to_string();
            SignedGroupRecord::new(
                &member,
                group_id.clone(),
                format!("test-member-joined-{}", rand::random::<u64>()),
                1_700_000_000 + 3,
                vec![invite_id],
                3,
                GroupRecordBody::MemberJoined {
                    peer_id: peer_id(&member),
                },
            )
            .expect("record")
        };
        assert!(!apply(&app_state, &join).expect("pending join"));
        apply(&app_state, &invite).expect("invite");

        let policy = get_group_policy(&app_state, &group_id).expect("policy");
        assert!(policy.active_members.contains(&peer_id(&member)));
    }

    #[test]
    fn pre_join_message_remains_pending_after_member_later_joins() {
        let app_state = app_state();
        GROUP_LAST_ID.with(|m| m.borrow_mut().clear());
        GROUP_LAST_COUNTER.with(|m| m.borrow_mut().clear());
        let founder = keypair();
        let member = keypair();
        let group_id = chat_kind::generate_group_chat_id();
        apply(
            &app_state,
            &signed(
                &founder,
                &group_id,
                "created",
                1,
                GroupRecordBody::GroupCreated {
                    name: "Test".to_string(),
                    settings: None,
                image_hash: None,
                },
            ),
        )
        .expect("created");
        let message = signed(
            &member,
            &group_id,
            "pre-join-message",
            2,
            GroupRecordBody::Message {
                content_type: GroupContentType::Text,
                text_content: Some("too early".to_string()),
                file_hash: None,
                sender_alias: None,
            },
        );

        assert!(
            apply(&app_state, &message).is_err(),
            "pre-join message must be hard error (parents present, not active)"
        );
        apply(
            &app_state,
            &signed(
                &founder,
                &group_id,
                "invite-member",
                3,
                GroupRecordBody::MemberInvited {
                    peer_id: peer_id(&member),
                    role: "member".to_string(),
                },
            ),
        )
        .expect("invite");
        apply(
            &app_state,
            &signed(
                &member,
                &group_id,
                "member-joined",
                4,
                GroupRecordBody::MemberJoined {
                    peer_id: peer_id(&member),
                },
            ),
        )
        .expect("joined");

        let conn = app_state.db_conn.lock().expect("db");
        // Hard error, not pending, so not stored at all.
        assert_eq!(
            group_record_state(&conn, message.id()).expect("state"),
            None
        );
        let inserted: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM messages WHERE id = ?1)",
                [message.id()],
                |row| row.get(0),
            )
            .expect("message exists query");
        assert!(!inserted);
    }

    #[test]
    fn only_active_members_can_sync_group_records() {
        let app_state = app_state();
        GROUP_LAST_ID.with(|m| m.borrow_mut().clear());
        GROUP_LAST_COUNTER.with(|m| m.borrow_mut().clear());
        let founder = keypair();
        let member = keypair();
        let outsider = keypair();
        let group_id = chat_kind::generate_group_chat_id();

        apply(
            &app_state,
            &signed(
                &founder,
                &group_id,
                "created",
                1,
                GroupRecordBody::GroupCreated {
                    name: "Test".to_string(),
                    settings: None,
                image_hash: None,
                },
            ),
        )
        .expect("created");
        apply(
            &app_state,
            &signed(
                &founder,
                &group_id,
                "invite-member",
                2,
                GroupRecordBody::MemberInvited {
                    peer_id: peer_id(&member),
                    role: "member".to_string(),
                },
            ),
        )
        .expect("invite");
        apply(
            &app_state,
            &signed(
                &member,
                &group_id,
                "member-joined",
                3,
                GroupRecordBody::MemberJoined {
                    peer_id: peer_id(&member),
                },
            ),
        )
        .expect("joined");

        assert!(can_peer_sync_group_records(&app_state, &group_id, &peer_id(&founder))
            .expect("founder sync"));
        assert!(can_peer_sync_group_records(&app_state, &group_id, &peer_id(&member))
            .expect("member sync"));
        assert!(!can_peer_sync_group_records(&app_state, &group_id, &peer_id(&outsider))
            .expect("outsider sync"));
    }

    #[test]
    fn admin_leave_is_blocked_until_succession_exists() {
        let app_state = app_state();
        GROUP_LAST_ID.with(|m| m.borrow_mut().clear());
        GROUP_LAST_COUNTER.with(|m| m.borrow_mut().clear());
        let founder = keypair();
        let group_id = chat_kind::generate_group_chat_id();

        apply(
            &app_state,
            &signed(
                &founder,
                &group_id,
                "created",
                1,
                GroupRecordBody::GroupCreated {
                    name: "Test".to_string(),
                    settings: None,
                image_hash: None,
                },
            ),
        )
        .expect("created");

        let result = apply(
            &app_state,
            &signed(
                &founder,
                &group_id,
                "admin-left",
                2,
                GroupRecordBody::MemberLeft {
                    peer_id: peer_id(&founder),
                },
            ),
        );

        assert!(result.is_err());
    }

    #[test]
    fn admin_member_removed_record_removes_active_member() {
        let app_state = app_state();
        GROUP_LAST_ID.with(|m| m.borrow_mut().clear());
        GROUP_LAST_COUNTER.with(|m| m.borrow_mut().clear());
        let founder = keypair();
        let member = keypair();
        let group_id = chat_kind::generate_group_chat_id();

        apply(
            &app_state,
            &signed(
                &founder,
                &group_id,
                "created",
                1,
                GroupRecordBody::GroupCreated {
                    name: "Test".to_string(),
                    settings: None,
                image_hash: None,
                },
            ),
        )
        .expect("created");
        apply(
            &app_state,
            &signed(
                &founder,
                &group_id,
                "invite-member",
                2,
                GroupRecordBody::MemberInvited {
                    peer_id: peer_id(&member),
                    role: "member".to_string(),
                },
            ),
        )
        .expect("invite");
        apply(
            &app_state,
            &signed(
                &member,
                &group_id,
                "member-joined",
                3,
                GroupRecordBody::MemberJoined {
                    peer_id: peer_id(&member),
                },
            ),
        )
        .expect("joined");
        apply(
            &app_state,
            &signed(
                &founder,
                &group_id,
                "member-removed",
                4,
                GroupRecordBody::MemberRemoved {
                    peer_id: peer_id(&member),
                },
            ),
        )
        .expect("removed");

        let policy = get_group_policy(&app_state, &group_id).expect("policy");
        assert!(!policy.active_members.contains(&peer_id(&member)));
    }

    #[test]
    fn delivered_receipt_does_not_downgrade_read() {
        let app_state = app_state();
        GROUP_LAST_ID.with(|m| m.borrow_mut().clear());
        GROUP_LAST_COUNTER.with(|m| m.borrow_mut().clear());
        let founder = keypair();
        let member = keypair();
        let group_id = chat_kind::generate_group_chat_id();
        let message_id = "message-1".to_string();

        apply(
            &app_state,
            &signed(
                &founder,
                &group_id,
                "created",
                1,
                GroupRecordBody::GroupCreated {
                    name: "Test".to_string(),
                    settings: None,
                image_hash: None,
                },
            ),
        )
        .expect("created");
        apply(
            &app_state,
            &signed(
                &founder,
                &group_id,
                "invite-member",
                2,
                GroupRecordBody::MemberInvited {
                    peer_id: peer_id(&member),
                    role: "member".to_string(),
                },
            ),
        )
        .expect("invite");
        apply(
            &app_state,
            &signed(
                &member,
                &group_id,
                "member-joined",
                3,
                GroupRecordBody::MemberJoined {
                    peer_id: peer_id(&member),
                },
            ),
        )
        .expect("joined");

        apply(
            &app_state,
            &signed(
                &member,
                &group_id,
                "read-receipt",
                4,
                GroupRecordBody::Receipt {
                    message_ids: vec![message_id.clone()],
                    status: GroupReceiptStatus::Read,
                },
            ),
        )
        .expect("read");
        apply(
            &app_state,
            &signed(
                &member,
                &group_id,
                "delivered-receipt",
                5,
                GroupRecordBody::Receipt {
                    message_ids: vec![message_id.clone()],
                    status: GroupReceiptStatus::Delivered,
                },
            ),
        )
        .expect("delivered");

        let conn = app_state.db_conn.lock().expect("db");
        let stored: String = conn
            .query_row(
                "SELECT status FROM group_message_receipts WHERE message_id = ?1 AND peer_id = ?2",
                (&message_id, peer_id(&member)),
                |row| row.get(0),
            )
            .expect("receipt");
        assert_eq!(stored, "read");
    }

    #[test]
    fn cross_group_parent_is_rejected() {
        let app_state = app_state();
        GROUP_LAST_ID.with(|m| m.borrow_mut().clear());
        GROUP_LAST_COUNTER.with(|m| m.borrow_mut().clear());
        let founder_g = keypair();
        let founder_h = keypair();
        let group_g = chat_kind::generate_group_chat_id();
        let group_h = chat_kind::generate_group_chat_id();
        let created_g = signed(&founder_g, &group_g, "created-g", 1, GroupRecordBody::GroupCreated {
            name: "G".to_string(), settings: None, image_hash: None,
        });
        let created_h = signed(&founder_h, &group_h, "created-h", 1, GroupRecordBody::GroupCreated {
            name: "H".to_string(), settings: None, image_hash: None,
        });
        apply(&app_state, &created_g).expect("created g");
        apply(&app_state, &created_h).expect("created h");
        // Try to create a G record that parents H's record.
        let cross = SignedGroupRecord::new(
            &founder_g,
            group_g.clone(),
            format!("test-cross-{}", rand::random::<u64>()),
            1_700_000_100,
            vec![created_h.id().to_string()],
            2,
            GroupRecordBody::MemberInvited {
                peer_id: fixed_peer_id(20),
                role: "member".to_string(),
            },
        )
        .expect("record");
        let err = apply(&app_state, &cross).expect_err("cross-group parent must be rejected");
        assert!(err.to_string().contains("different group"), "wrong error: {err}");
    }

    #[test]
    fn concurrent_heads_both_remain_in_later_closure() {
        let app_state = app_state();
        GROUP_LAST_ID.with(|m| m.borrow_mut().clear());
        GROUP_LAST_COUNTER.with(|m| m.borrow_mut().clear());
        let founder = keypair();
        let alice = keypair();
        let bob = keypair();
        let group_id = chat_kind::generate_group_chat_id();
        // Sequential prefix.
        for rec in [
            signed(&founder, &group_id, "created", 1, GroupRecordBody::GroupCreated {
                name: "Test".to_string(), settings: Some(GroupSettings { members_can_invite: true }), image_hash: None,
            }),
            signed(&founder, &group_id, "invite-alice", 2, GroupRecordBody::MemberInvited {
                peer_id: peer_id(&alice), role: "member".to_string(),
            }),
            signed(&alice, &group_id, "join-alice", 3, GroupRecordBody::MemberJoined {
                peer_id: peer_id(&alice),
            }),
            signed(&founder, &group_id, "invite-bob", 4, GroupRecordBody::MemberInvited {
                peer_id: peer_id(&bob), role: "member".to_string(),
            }),
            signed(&bob, &group_id, "join-bob", 5, GroupRecordBody::MemberJoined {
                peer_id: peer_id(&bob),
            }),
        ] {
            apply(&app_state, &rec).expect("prefix");
        }
        let head_before = {
            let conn = app_state.db_conn.lock().expect("db");
            db::get_group_head_ids(&conn, &group_id).expect("heads")
        };
        assert_eq!(head_before.len(), 1, "single head before concurrent");
        let head_id = head_before[0].clone();
        // Two concurrent invites at same counter, same parents.
        let invite_a = SignedGroupRecord::new(
            &alice,
            group_id.clone(),
            format!("test-concurrent-a-{}", rand::random::<u64>()),
            1_700_000_200,
            vec![head_id.clone()],
            6,
            GroupRecordBody::MemberInvited {
                peer_id: fixed_peer_id(30),
                role: "member".to_string(),
            },
        )
        .expect("record");
        let invite_b = SignedGroupRecord::new(
            &bob,
            group_id.clone(),
            format!("test-concurrent-b-{}", rand::random::<u64>()),
            1_700_000_201,
            vec![head_id.clone()],
            6,
            GroupRecordBody::MemberInvited {
                peer_id: fixed_peer_id(31),
                role: "member".to_string(),
            },
        )
        .expect("record");
        apply(&app_state, &invite_a).expect("concurrent a");
        apply(&app_state, &invite_b).expect("concurrent b");
        let heads_after = {
            let conn = app_state.db_conn.lock().expect("db");
            db::get_group_head_ids(&conn, &group_id).expect("heads")
        };
        assert_eq!(heads_after.len(), 2, "both concurrent invites are heads");
        // Later record parents both heads, so both invites remain in its closure.
        let later = SignedGroupRecord::new(
            &founder,
            group_id.clone(),
            format!("test-later-{}", rand::random::<u64>()),
            1_700_000_300,
            heads_after.clone(),
            7,
            GroupRecordBody::MemberInvited {
                peer_id: fixed_peer_id(32),
                role: "member".to_string(),
            },
        )
        .expect("record");
        apply(&app_state, &later).expect("later");
        let closure = {
            let conn = app_state.db_conn.lock().expect("db");
            collect_parent_closure(&conn, &group_id, &vec![later.id().to_string()]).expect("closure")
        };
        let closure_ids: std::collections::HashSet<String> =
            closure.iter().map(|r| r.id().to_string()).collect();
        assert!(closure_ids.contains(invite_a.id()), "later must contain invite_a");
        assert!(closure_ids.contains(invite_b.id()), "later must contain invite_b");
    }

    #[test]
    fn missing_parent_high_counter_does_not_poison_local_head() {
        let app_state = app_state();
        GROUP_LAST_ID.with(|m| m.borrow_mut().clear());
        GROUP_LAST_COUNTER.with(|m| m.borrow_mut().clear());
        let founder = keypair();
        let group_id = chat_kind::generate_group_chat_id();
        apply(
            &app_state,
            &signed(&founder, &group_id, "created", 1, GroupRecordBody::GroupCreated {
                name: "Test".to_string(), settings: None, image_hash: None,
            }),
        )
        .expect("created");
        // High-counter record with missing parent stays pending and does not
        // become the head for local issuance.
        let created_id_for_ghost = {
            let conn = app_state.db_conn.lock().expect("db");
            let recs = db::get_all_verified_group_records_ordered(&conn, &group_id).expect("records");
            recs[0].id().to_string()
        };
        let missing_id = format!("test-missing-{}", rand::random::<u64>());
        let ghost = SignedGroupRecord::new(
            &founder,
            group_id.clone(),
            missing_id.clone(),
            1_700_000_100,
            vec!["nonexistent-parent".to_string()],
            10,
            GroupRecordBody::MemberInvited {
                peer_id: fixed_peer_id(40),
                role: "member".to_string(),
            },
        )
        .expect("record");
        assert!(!apply(&app_state, &ghost).expect("pending ghost"));
        let heads_before = {
            let conn = app_state.db_conn.lock().expect("db");
            db::get_group_head_ids(&conn, &group_id).expect("heads")
        };
        assert_eq!(heads_before.len(), 1);
        assert_eq!(heads_before[0], created_id_for_ghost);
        // Valid local record should still be able to advance from the verified head.
        let valid = signed(&founder, &group_id, "valid-after-ghost", 2, GroupRecordBody::MemberInvited {
            peer_id: fixed_peer_id(41),
            role: "member".to_string(),
        });
        // The helper's auto parents will be [created], counter 2, which is correct.
        // It should apply, not be blocked by the pending ghost.
        apply(&app_state, &valid).expect("valid after ghost");
        let policy = get_group_policy(&app_state, &group_id).expect("policy");
        assert!(policy.invited_members.contains(&fixed_peer_id(41)));
    }

    #[test]
    fn policy_reconstruction_beyond_ten_thousand_records() {
        let app_state = app_state();
        GROUP_LAST_ID.with(|m| m.borrow_mut().clear());
        GROUP_LAST_COUNTER.with(|m| m.borrow_mut().clear());
        let founder = keypair();
        let group_id = chat_kind::generate_group_chat_id();
        apply(
            &app_state,
            &signed(&founder, &group_id, "created", 1, GroupRecordBody::GroupCreated {
                name: "Test".to_string(), settings: None, image_hash: None,
            }),
        )
        .expect("created");
        // Create >10k records directly via DB inserts to avoid per-record validation
        // overhead, then verify policy reconstruction sees beyond the old 10k page.
        // Use Head records chained via parents to keep a single head.
        let mut last_id = {
            let conn = app_state.db_conn.lock().expect("db");
            let recs = db::get_all_verified_group_records_ordered(&conn, &group_id).expect("records");
            recs.last().unwrap().id().to_string()
        };
        let conn = app_state.db_conn.lock().expect("db");
        for i in 2..=10002 {
            let kp = keypair();
            let rec = SignedGroupRecord::new(
                &kp,
                group_id.clone(),
                format!("test-bulk-{i}-{}", rand::random::<u64>()),
                1_700_000_000 + i as i64,
                vec![last_id.clone()],
                i as u64,
                GroupRecordBody::Head { heads: vec![] },
            )
            .expect("bulk record");
            db::insert_group_record(&conn, &rec, true, false).expect("insert bulk");
            last_id = rec.id().to_string();
        }
        drop(conn);
        // Now a real policy-changing record at the frontier should still be
        // reconstructible even though the group has >10k records.
        let head_id = {
            let conn = app_state.db_conn.lock().expect("db");
            let heads = db::get_group_head_ids(&conn, &group_id).expect("heads");
            heads[0].clone()
        };
        let invite_explicit = SignedGroupRecord::new(
            &founder,
            group_id.clone(),
            format!("test-final-invite-explicit-{}", rand::random::<u64>()),
            1_700_010_000,
            vec![head_id],
            10003,
            GroupRecordBody::MemberInvited {
                peer_id: fixed_peer_id(51),
                role: "member".to_string(),
            },
        )
        .expect("invite");
        apply(&app_state, &invite_explicit).expect("final invite");
        let policy = get_group_policy(&app_state, &group_id).expect("policy");
        assert!(policy.invited_members.contains(&fixed_peer_id(51)));
    }

    #[tokio::test]
    async fn create_invite_join_two_messages_end_to_end() {
        let (temp_dir, app_state) = crate::testing::test_app_state().await;
        let (network_state, mut rx) = crate::testing::test_network_state();
        // Drain network commands so publish does not block.
        tokio::spawn(async move { while rx.recv().await.is_some() {} });
        // Persist founder keypair as local peer.
        let founder = keypair();
        {
            let mgr = app_state.config_manager.lock().await;
            let mut cfg = mgr.load().await.unwrap();
            cfg.user.libp2p_keypair = Some(
                BASE64.encode(&founder.to_protobuf_encoding().unwrap()),
            );
            mgr.save(&cfg).await.unwrap();
        }
        *network_state.local_peer_id.lock().await =
            Some(peer_id(&founder));
        let group_id = create_group_with_options(
            &app_state,
            Some(&network_state),
            crate::chat::group::CreateGroupOptions {
                name: Some("Test Group".to_string()),
                settings: None,
                image_path: None,
                require_name: false,
            },
        )
        .await
        .unwrap()
        .chat_id;
        let member = keypair();
        let member_id = peer_id(&member);
        invite_member(&app_state, &network_state, group_id.clone(), member_id.clone())
            .await
            .unwrap();
        // Simulate member joining via the invite: apply the join that parents the invite.
        let invite_rec = {
            let conn = app_state.db_conn.lock().unwrap();
            let recs = db::get_all_verified_group_records_ordered(&conn, &group_id).unwrap();
            recs.into_iter()
                .find(|r| matches!(r.body(), GroupRecordBody::MemberInvited { peer_id, .. } if peer_id == &member_id))
                .unwrap()
        };
        let join = SignedGroupRecord::new(
            &member,
            group_id.clone(),
            format!("test-join-{}", rand::random::<u64>()),
            1_700_000_003,
            vec![invite_rec.id().to_string()],
            invite_rec.lamport_counter() + 1,
            GroupRecordBody::MemberJoined {
                peer_id: member_id.clone(),
            },
        )
        .unwrap();
        apply(&app_state, &join).unwrap();
        // Two consecutive local messages must both be stored as Me.
        let msg1 = send_group_text(&app_state, &network_state, group_id.clone(), "hello".to_string(), None)
            .await
            .unwrap();
        let msg2 = send_group_text(&app_state, &network_state, group_id.clone(), "world".to_string(), None)
            .await
            .unwrap();
        assert_ne!(msg1, msg2);
        let conn = app_state.db_conn.lock().unwrap();
        for msg_id in [msg1, msg2] {
            let peer: String = conn
                .query_row("SELECT peer_id FROM messages WHERE id = ?1", [&msg_id], |r| r.get(0))
                .unwrap();
            assert_eq!(peer, "Me", "local message should be stored as Me, got {peer}");
        }
        drop(temp_dir);
    }

    #[tokio::test]
    async fn concurrent_local_allocations_do_not_fork() {
        let (temp_dir, app_state) = crate::testing::test_app_state().await;
        let founder = keypair();
        {
            let mgr = app_state.config_manager.lock().await;
            let mut cfg = mgr.load().await.unwrap();
            cfg.user.libp2p_keypair = Some(BASE64.encode(&founder.to_protobuf_encoding().unwrap()));
            mgr.save(&cfg).await.unwrap();
        }
        let (network_state, mut rx) = crate::testing::test_network_state();
        *network_state.local_peer_id.lock().await = Some(peer_id(&founder));
        tokio::spawn(async move { while rx.recv().await.is_some() {} });
        let group_id = create_group_with_options(
            &app_state,
            Some(&network_state),
            crate::chat::group::CreateGroupOptions {
                name: Some("Concurrent".to_string()),
                settings: None,
                image_path: None,
                require_name: false,
            },
        )
        .await
        .unwrap()
        .chat_id;
        // Two concurrent renames that both read the same head.
        let barrier = std::sync::Arc::new(tokio::sync::Barrier::new(2));
        let app1 = app_state.clone();
        let net1 = network_state.clone();
        let gid1 = group_id.clone();
        let b1 = barrier.clone();
        let h1 = tokio::spawn(async move {
            b1.wait().await;
            rename_group(&app1, &net1, gid1, "Renamed A".to_string()).await
        });
        let app2 = app_state.clone();
        let net2 = network_state.clone();
        let gid2 = group_id.clone();
        let b2 = barrier.clone();
        let h2 = tokio::spawn(async move {
            b2.wait().await;
            rename_group(&app2, &net2, gid2, "Renamed B".to_string()).await
        });
        let r1 = h1.await.unwrap();
        let r2 = h2.await.unwrap();
        // At least one must succeed; the other may succeed with a retried counter
        // or be rejected, but they must not fork the same (author,counter).
        assert!(
            r1.is_ok() || r2.is_ok(),
            "at least one concurrent rename should succeed: {r1:?} {r2:?}"
        );
        let conn = app_state.db_conn.lock().unwrap();
        let recs = db::get_all_verified_group_records_ordered(&conn, &group_id).unwrap();
        let mut seen = std::collections::HashSet::new();
        for r in recs {
            let key = (r.author_peer_id().to_string(), r.lamport_counter());
            assert!(seen.insert(key.clone()), "fork detected: same author/counter {:?}", key);
        }
        drop(temp_dir);
    }

/// A deterministic team: founder + two members, with a group already created
    /// and joined. Returns the keypairs, group id, and the causal rec-ord list.
    fn test_team() -> (
        identity::Keypair,
        identity::Keypair,
        identity::Keypair,
        String,
        Vec<SignedGroupRecord>,
    ) {
        std::thread_local! {
            static TID: std::cell::RefCell<u64> = std::cell::RefCell::new(0);
        }
        let tid = TID.with(|t| {
            let mut v = t.borrow_mut();
            *v += 1;
            *v
        });
        let founder = keypair();
        let member_b = keypair();
        let member_c = keypair();
        let group_id = chat_kind::generate_group_chat_id();
        let b_id = peer_id(&member_b);
        let c_id = peer_id(&member_c);
        let created = SignedGroupRecord::new(
            &founder,
            group_id.clone(),
            format!("team-created-{tid}"),
            1_700_000_001i64 + tid as i64,
            vec![],
            1,
            GroupRecordBody::GroupCreated {
                name: "Team".to_string(),
                settings: Some(GroupSettings { members_can_invite: true }),
                image_hash: None,
            },
        )
        .unwrap();
        let invite_b = SignedGroupRecord::new(
            &founder,
            group_id.clone(),
            format!("team-invite-b-{tid}"),
            1_700_000_002i64 + tid as i64,
            vec![created.id().to_string()],
            2,
            GroupRecordBody::MemberInvited {
                peer_id: b_id.clone(),
                role: "member".to_string(),
            },
        )
        .unwrap();
        let join_b = SignedGroupRecord::new(
            &member_b,
            group_id.clone(),
            format!("team-join-b-{tid}"),
            1_700_000_003i64 + tid as i64,
            vec![invite_b.id().to_string()],
            3,
            GroupRecordBody::MemberJoined { peer_id: b_id },
        )
        .unwrap();
        let invite_c = SignedGroupRecord::new(
            &founder,
            group_id.clone(),
            format!("team-invite-c-{tid}"),
            1_700_000_004i64 + tid as i64,
            vec![join_b.id().to_string()],
            4,
            GroupRecordBody::MemberInvited {
                peer_id: c_id.clone(),
                role: "member".to_string(),
            },
        )
        .unwrap();
        let join_c = SignedGroupRecord::new(
            &member_c,
            group_id.clone(),
            format!("team-join-c-{tid}"),
            1_700_000_005i64 + tid as i64,
            vec![invite_c.id().to_string()],
            5,
            GroupRecordBody::MemberJoined { peer_id: c_id },
        )
        .unwrap();
        (
            founder,
            member_b,
            member_c,
            group_id,
            vec![created, invite_b, join_b, invite_c, join_c],
        )
    }

    /// A child record at `counter` from `author` over exactly `parents`.
    fn child(
        author: &identity::Keypair,
        group_id: &str,
        tag: &str,
        counter: u64,
        parents: Vec<String>,
        body: GroupRecordBody,
    ) -> SignedGroupRecord {
        SignedGroupRecord::new(
            author,
            group_id.to_string(),
            format!("{tag}-{}", rand::random::<u64>()),
            1_700_000_000i64 + counter as i64,
            parents,
            counter,
            body,
        )
        .unwrap()
    }
/// Materialized-state snapshot used to compare two databases after applying
    /// an identical record log in different orders.
    #[derive(PartialEq, Debug)]
    struct GroupSnapshot {
        verified_ids: std::collections::HashSet<String>,
        chat: Option<String>,
        roster: Vec<(String, String, String)>,
        messages: Vec<(String, String, String)>,
        receipts: Vec<(String, String, String)>,
        file_sources: Vec<(String, String)>,
    }

    fn snapshot(app_state: &AppState, group_id: &str) -> (GroupSnapshot, std::collections::HashSet<String>) {
        let conn = app_state.db_conn.lock().unwrap();
        let mut verified = std::collections::HashSet::new();
        for rec in db::get_all_verified_group_records_ordered(&conn, group_id).unwrap() {
            verified.insert(rec.id().to_string());
        }
        let chat = conn
            .query_row("SELECT name FROM chats WHERE id = ?1", [group_id], |r| {
                r.get::<_, String>(0)
            })
            .ok();
        let mut roster = Vec::new();
        for m in db::get_group_roster(&conn, group_id).unwrap() {
            roster.push((m.peer_id, m.membership_state, m.role));
        }
        roster.sort();
        let mut messages = Vec::new();
        {
            let mut stmt = conn
                .prepare("SELECT id, peer_id, content_type FROM messages WHERE chat_id = ?1 ORDER BY id")
                .unwrap();
            let rows = stmt
                .query_map([group_id], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                })
                .unwrap();
            for row in rows {
                messages.push(row.unwrap());
            }
        }
        let mut receipts = Vec::new();
        {
            let mut stmt = conn
                .prepare("SELECT message_id, peer_id, status FROM group_message_receipts WHERE group_id = ?1 ORDER BY message_id, peer_id")
                .unwrap();
            let rows = stmt
                .query_map([group_id], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                })
                .unwrap();
            for row in rows {
                receipts.push(row.unwrap());
            }
        }
        let mut file_sources = Vec::new();
        {
            let mut stmt = conn
                .prepare("SELECT file_hash, peer_id FROM group_file_sources WHERE group_id = ?1 ORDER BY file_hash")
                .unwrap();
            let rows = stmt
                .query_map([group_id], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })
                .unwrap();
            for row in rows {
                file_sources.push(row.unwrap());
            }
        }
        let pending = {
            let mut stmt = conn
                .prepare("SELECT id FROM group_records WHERE group_id = ?1 AND pending = 1")
                .unwrap();
            let rows = stmt.query_map([group_id], |row| row.get::<_, String>(0)).unwrap();
            let mut s = std::collections::HashSet::new();
            for row in rows {
                s.insert(row.unwrap());
            }
            s
        };
        (
            GroupSnapshot {
                verified_ids: verified,
                chat,
                roster,
                messages,
                receipts,
                file_sources,
            },
            pending,
        )
    }
/// Multi-head attack: admin A removes B while B (still active at sign time)
    /// concurrently invites someone. A continuation extending only B's branch
    /// while omitting the removal R must be rejected — the causal frontier may
    /// never be reduced to one arbitrary winner. Naming both leaves is valid.
    #[test]
    fn concurrent_policy_leaf_cannot_hide_removal() {
        let (founder, member_b, member_c, group_id, team) = test_team();
        let app = app_state();
        for rec in &team {
            apply(&app, rec).unwrap();
        }
        let b_id = peer_id(&member_b);
        let c_id = peer_id(&member_c);
        let head = team.last().unwrap();
        let removal = child(
            &founder,
            &group_id,
            "removal",
            6,
            vec![head.id().to_string()],
            GroupRecordBody::MemberRemoved { peer_id: b_id.clone() },
        );
        let x_invite = child(
            &member_b,
            &group_id,
            "x-invite",
            6,
            vec![head.id().to_string()],
            GroupRecordBody::MemberInvited {
                peer_id: fixed_peer_id(9),
                role: "member".to_string(),
            },
        );
        apply(&app, &removal).unwrap();
        apply(&app, &x_invite).unwrap();
        // Extension of B's branch omitting R must fail.
        let y_from_x_only = child(
            &member_b,
            &group_id,
            "y-omit-removal",
            7,
            vec![x_invite.id().to_string()],
            GroupRecordBody::MemberInvited {
                peer_id: fixed_peer_id(10),
                role: "member".to_string(),
            },
        );
        let err = apply(&app, &y_from_x_only)
            .expect_err("omitting a concurrent policy branch must be rejected");
        assert!(
            err.to_string().contains("dominate") || err.to_string().contains("active"),
            "wrong rejection: {err}"
        );
        // A merge naming BOTH concurrent policy leaves is legitimate.
        let merge = child(
            &founder,
            &group_id,
            "merge",
            7,
            vec![removal.id().to_string(), x_invite.id().to_string()],
            GroupRecordBody::GroupRenamed { name: "Merged".to_string() },
        );
        apply(&app, &merge).unwrap();
        let policy = get_group_policy(&app, &group_id).unwrap();
        assert!(!policy.active_members.contains(&b_id), "B stays removed after the merge");
        assert!(policy.active_members.contains(&c_id));
    }
/// One immutable fixture applied to two fresh databases in opposite orders
    /// must converge to identical materialized state (verified ids, chats,
    /// roster, messages, receipts, file sources) — genuine convergence.
    #[test]
    fn immutable_fixture_converges_across_arrival_orders() {
        let (founder, member_b, member_c, group_id, team) = test_team();
        let b_id = peer_id(&member_b);
        let c_id = peer_id(&member_c);
        let records = {
            let mut v = team;
            let base = v.last().unwrap().clone();
            let msg1 = child(
                &member_b,
                &group_id,
                "msg1",
                6,
                vec![base.id().to_string()],
                GroupRecordBody::Message {
                    content_type: GroupContentType::Text,
                    text_content: Some("hello".to_string()),
                    file_hash: Some("file-hash-1".to_string()),
                    sender_alias: None,
                },
            );
            let receipt = child(
                &founder,
                &group_id,
                "read1",
                7,
                vec![msg1.id().to_string()],
                GroupRecordBody::Receipt {
                    message_ids: vec![msg1.id().to_string()],
                    status: GroupReceiptStatus::Read,
                },
            );
            let availability = child(
                &member_b,
                &group_id,
                "avail",
                8,
                vec![receipt.id().to_string()],
                GroupRecordBody::FileAvailability {
                    file_hash: "file-hash-1".to_string(),
                },
            );
            let transfer1 = child(
                &founder,
                &group_id,
                "t1",
                9,
                vec![availability.id().to_string()],
                GroupRecordBody::AdminTransferred {
                    new_admin_peer_id: b_id.clone(),
                },
            );
            let message2 = child(
                &member_c,
                &group_id,
                "msg2",
                10,
                vec![transfer1.id().to_string()],
                GroupRecordBody::Message {
                    content_type: GroupContentType::Text,
                    text_content: Some("second".to_string()),
                    file_hash: None,
                    sender_alias: None,
                },
            );
            v.push(msg1);
            v.push(receipt);
            v.push(availability);
            v.push(transfer1);
            v.push(message2);
            v
        };

        let app_a = app_state();
        for rec in &records {
            apply(&app_a, rec).unwrap();
        }
        let (snap_a, pending_a) = snapshot(&app_a, &group_id);

        let app_b = app_state();
        for rec in records.iter().rev() {
            apply(&app_b, rec).unwrap();
        }
        let (snap_b, pending_b) = snapshot(&app_b, &group_id);

        assert_eq!(snap_a, snap_b, "materialized state must converge across orders");
        assert_eq!(pending_a, pending_b, "pending set must converge across orders");
        assert_eq!(snap_a.verified_ids.len(), records.len(), "every fixture record verified");
        assert!(
            snap_a
                .file_sources
                .iter()
                .any(|(h, p)| h == "file-hash-1" && p == &b_id),
            "file source retained"
        );
    }
/// The canonical rebuild reproduces what the apply path produced: transfer
    /// demotes the old admin, local messages stay `Me`, receipts and file
    /// sources survive.
    #[test]
    fn rebuild_reproduces_canonical_projection() {
        let (founder, member_a, _member_c, group_id, team) = test_team();
        let a_id = peer_id(&member_a);
        let founder_id = peer_id(&founder);
        let app = app_state();
        let _ = app.local_peer_id.set(founder_id.clone());
        for rec in &team {
            apply(&app, rec).unwrap();
        }
        let head = team.last().unwrap();
        let msg = child(
            &founder,
            &group_id,
            "local-msg",
            6,
            vec![head.id().to_string()],
            GroupRecordBody::Message {
                content_type: GroupContentType::Text,
                text_content: Some("mine".to_string()),
                file_hash: Some("fh-local".to_string()),
                sender_alias: None,
            },
        );
        apply(&app, &msg).unwrap();
        let receipt = child(
            &member_a,
            &group_id,
            "a-read",
            7,
            vec![msg.id().to_string()],
            GroupRecordBody::Receipt {
                message_ids: vec![msg.id().to_string()],
                status: GroupReceiptStatus::Read,
            },
        );
        apply(&app, &receipt).unwrap();
        let transfer = child(
            &founder,
            &group_id,
            "transfer",
            8,
            vec![receipt.id().to_string()],
            GroupRecordBody::AdminTransferred {
                new_admin_peer_id: a_id.clone(),
            },
        );
        apply(&app, &transfer).unwrap();

        let (before, _) = snapshot(&app, &group_id);
        let conn = app.db_conn.lock().unwrap();
        let valid = db::get_all_verified_group_records_ordered(&conn, &group_id).unwrap();
        drop(conn);
        {
            let conn = app.db_conn.lock().unwrap();
            rebuild_group_materialized_state(&conn, &app, &group_id, &valid).unwrap();
        }
        let (after, _) = snapshot(&app, &group_id);
        assert_eq!(before, after, "rebuild must reproduce the same materialized state");

        let conn = app.db_conn.lock().unwrap();
        let peer: String = conn
            .query_row(
                "SELECT peer_id FROM messages WHERE chat_id = ?1 AND text_content = 'mine'",
                [&group_id],
                |r| r.get(0),
            )
            .unwrap();
        drop(conn);
        assert_eq!(peer, "Me", "local message must replay as Me");

        let conn = app.db_conn.lock().unwrap();
        let mut admins: Vec<String> = conn
            .prepare(
                "SELECT peer_id FROM chat_peers WHERE chat_id = ?1 AND role = 'admin' AND membership_state = 'joined'",
            )
            .unwrap()
            .query_map([&group_id], |r| r.get(0))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        admins.sort();
        drop(conn);
        assert_eq!(admins, vec![a_id], "old admin must be demoted after transfer");
    }

    /// A late dissolution must resurrect nothing: after the canonical rebuild
    /// the group chat is gone.
    #[test]
    fn rebuild_dissolution_removes_chat() {
        let founder = keypair();
        let founder_id = peer_id(&founder);
        let group_id = chat_kind::generate_group_chat_id();
        let app = app_state();
        let _ = app.local_peer_id.set(founder_id.clone());
        // Founder-only group (sole active member can dissolve).
        let created = SignedGroupRecord::new(
            &founder,
            group_id.clone(),
            format!("dissolve-created-{}", rand::random::<u64>()),
            1_700_000_001,
            vec![],
            1,
            GroupRecordBody::GroupCreated {
                name: "Solo".to_string(),
                settings: None,
                image_hash: None,
            },
        )
        .unwrap();
        apply(&app, &created).unwrap();
        let dissolved = child(
            &founder,
            &group_id,
            "dissolve",
            2,
            vec![created.id().to_string()],
            GroupRecordBody::GroupDissolved,
        );
        apply(&app, &dissolved).unwrap();
        let conn = app.db_conn.lock().unwrap();
        let valid = db::get_all_verified_group_records_ordered(&conn, &group_id).unwrap();
        drop(conn);
        {
            let conn = app.db_conn.lock().unwrap();
            rebuild_group_materialized_state(&conn, &app, &group_id, &valid).unwrap();
        }
        let conn = app.db_conn.lock().unwrap();
        let chat_exists = conn
            .query_row("SELECT 1 FROM chats WHERE id = ?1", [&group_id], |_| Ok(true))
            .is_ok();
        drop(conn);
        assert!(!chat_exists, "dissolved group chat must not be resurrected");
    }
}
