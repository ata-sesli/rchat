use std::collections::{HashMap, HashSet};

use anyhow::{anyhow, Context};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use libp2p::{identity, PeerId};

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
            GroupSettings, SignedGroupRecord,
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
    let timestamp = timestamp_now();
    let genesis_record_id = format!("group-rec-{timestamp}-{}", rand::random::<u32>());
    let founder_peer_id = PeerId::from_public_key(&keypair.public()).to_string();
    let group_id = chat_kind::derive_group_chat_id(&founder_peer_id, &genesis_record_id);
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
        Some(media::store_file_object_from_path(app_state, MediaKind::Image, image_path)?.file_hash)
    } else {
        None
    };
    let record = SignedGroupRecord::new(
        &keypair,
        group_id.clone(),
        genesis_record_id,
        timestamp,
        Vec::new(),
        1,
        GroupRecordBody::GroupCreated {
            name: resolved_name.clone(),
            settings: options.settings,
            image_hash: image_hash.clone(),
        },
    )?;
    apply_signed_record(app_state, None, &record, true)?;
    let file_availability_record = image_hash
        .as_ref()
        .map(|file_hash| {
            sign_record(
                app_state,
                &keypair,
                group_id.clone(),
                GroupRecordBody::FileAvailability {
                    file_hash: file_hash.clone(),
                },
            )
        })
        .transpose()?;
    if let Some(file_record) = &file_availability_record {
        apply_signed_record(app_state, None, file_record, true)?;
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
    let conn = app_state
        .db_conn
        .lock()
        .map_err(|e| anyhow!(e.to_string()))?;
    let records = db::get_all_group_records(&conn, group_id)?;
    derive_group_policy(&records).ok_or_else(|| anyhow!("Group has no valid founder record"))
}

pub fn get_group_roster(
    app_state: &AppState,
    group_id: &str,
) -> anyhow::Result<Vec<db::GroupMemberRow>> {
    let conn = app_state
        .db_conn
        .lock()
        .map_err(|e| anyhow!(e.to_string()))?;
    db::get_group_roster(&conn, group_id)
}

pub fn get_group_image_hash(
    app_state: &AppState,
    group_id: &str,
) -> anyhow::Result<Option<String>> {
    let conn = app_state
        .db_conn
        .lock()
        .map_err(|e| anyhow!(e.to_string()))?;
    db::get_group_image_hash(&conn, group_id)
}

pub fn get_group_message_receipts(
    app_state: &AppState,
    group_id: &str,
    message_id: &str,
) -> anyhow::Result<Vec<db::GroupMessageReceiptRow>> {
    let conn = app_state
        .db_conn
        .lock()
        .map_err(|e| anyhow!(e.to_string()))?;
    db::get_group_message_receipts(&conn, group_id, message_id)
}

pub fn get_group_pending_record_summary(
    app_state: &AppState,
    group_id: &str,
) -> anyhow::Result<db::GroupPendingRecordSummary> {
    let conn = app_state
        .db_conn
        .lock()
        .map_err(|e| anyhow!(e.to_string()))?;
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
    if policy.active_members.contains(peer_id) {
        return Ok(true);
    }
    let conn = app_state
        .db_conn
        .lock()
        .map_err(|error| anyhow!(error.to_string()))?;
    let invitees = if policy.dissolved {
        db::get_group_invitee_peer_ids_for_sync(&conn, group_id)?
    } else {
        db::get_open_group_invitee_peer_ids(&conn, group_id)?
    };
    Ok(invitees.iter().any(|invitee| invitee == peer_id))
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
        return Err(anyhow!(
            "The current admin is already the group administrator"
        ));
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
        let conn = app_state
            .db_conn
            .lock()
            .map_err(|e| anyhow!(e.to_string()))?;
        let group_name = db::get_chat_list(&conn)?
            .into_iter()
            .find(|chat| chat.id == group_id)
            .map(|chat| chat.name)
            .unwrap_or_else(|| chat_kind::default_group_name(&group_id));
        let related_records = db::get_all_group_records(&conn, &group_id)?
            .into_iter()
            .filter(|record| matches!(record.body(), GroupRecordBody::GroupCreated { .. }))
            .collect();
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
        version: crate::network::gossip::GROUP_PROTOCOL_VERSION,
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
        let conn = app_state
            .db_conn
            .lock()
            .map_err(|e| anyhow!(e.to_string()))?;
        db::upsert_group_invite(&conn, &payload, "sent")?;
    }
    apply_signed_record(app_state, None, &invite_record, true)?;

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
        let conn = app_state
            .db_conn
            .lock()
            .map_err(|e| anyhow!(e.to_string()))?;
        db::get_group_invite_payload(&conn, &invite_id)?
            .ok_or_else(|| anyhow!("Unknown group invite: {invite_id}"))?
    };
    validate_group_invite(&invite)?;

    let keypair = load_or_create_local_keypair(app_state).await?;
    let local_peer_id = PeerId::from_public_key(&keypair.public()).to_string();
    if invite.invitee_peer_id != local_peer_id {
        return Err(anyhow!("Group invite was not addressed to this peer"));
    }

    for record in &invite.related_records {
        let _ = apply_signed_record(app_state, None, record, true);
    }
    apply_signed_record(app_state, None, &invite.invite_record, true)?;
    if get_group_policy(app_state, &invite.group_id)?.dissolved {
        let conn = app_state
            .db_conn
            .lock()
            .map_err(|e| anyhow!(e.to_string()))?;
        db::update_group_invite_status(&conn, &invite_id, "revoked")?;
        return Err(anyhow!("This group has been dissolved"));
    }
    if !get_group_policy(app_state, &invite.group_id)?
        .invited_members
        .contains(&local_peer_id)
    {
        return Err(anyhow!(
            "Group invitation is not authorized by the supplied causal history"
        ));
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
        let conn = app_state
            .db_conn
            .lock()
            .map_err(|e| anyhow!(e.to_string()))?;
        db::update_group_invite_status(&conn, &invite_id, "accepted")?;
    }
    apply_signed_record(app_state, None, &joined_record, true)?;

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
    let conn = app_state
        .db_conn
        .lock()
        .map_err(|e| anyhow!(e.to_string()))?;
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
            let transfer = sign_record(
                app_state,
                &keypair,
                group_id.clone(),
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
                &keypair,
                group_id.clone(),
                transfer.timestamp().saturating_add(1),
                vec![transfer.id().to_string()],
                transfer.lamport_counter().saturating_add(1),
                GroupRecordBody::MemberLeft {
                    peer_id: local_peer_id,
                },
            )?;
            apply_signed_record(app_state, None, &leave, true)?;
            send_network_command(
                network_state,
                NetworkCommand::PublishGroupRecord { record: leave },
            )
            .await?;
            outcome = GroupLeaveOutcome::TransferredThenLeft { successor_peer_id };
        } else {
            let invitees = {
                let conn = app_state
                    .db_conn
                    .lock()
                    .map_err(|e| anyhow!(e.to_string()))?;
                db::get_open_group_invitee_peer_ids(&conn, &group_id)?
            };
            let dissolution = sign_record(
                app_state,
                &keypair,
                group_id.clone(),
                GroupRecordBody::GroupDissolved,
            )?;
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
        send_network_command(
            network_state,
            NetworkCommand::PublishGroupRecord { record: leave },
        )
        .await?;
        outcome = GroupLeaveOutcome::Left;
    }
    if !matches!(outcome, GroupLeaveOutcome::Dissolved) {
        let conn = app_state
            .db_conn
            .lock()
            .map_err(|e| anyhow!(e.to_string()))?;
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
        GroupRecordBody::GroupRenamed { name },
    )?;
    apply_signed_record(app_state, None, &record, true)?;
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
        return Err(anyhow!(
            "Group media reference requires a file-backed content type"
        ));
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

fn derive_group_policy(records: &[SignedGroupRecord]) -> Option<GroupPolicy> {
    let state = crate::chat::group_state::evaluate_group_records(records).state?;
    let automatic_successor_peer_id = state.automatic_successor_peer_id();
    Some(GroupPolicy {
        admin_peer_id: state.admin_peer_id,
        settings: state.settings,
        active_members: state.active_members.into_iter().collect(),
        invited_members: state.invited_members.into_iter().collect(),
        automatic_successor_peer_id,
        dissolved: state.dissolved,
    })
}

fn can_invite(policy: &GroupPolicy, peer_id: &str) -> bool {
    policy.admin_peer_id == peer_id
        || (policy.settings.members_can_invite && policy.active_members.contains(peer_id))
}

pub fn apply_signed_record(
    app_state: &AppState,
    event_sink: Option<&SharedCoreEventSink>,
    record: &SignedGroupRecord,
    verified: bool,
) -> anyhow::Result<bool> {
    if !verified {
        return Ok(false);
    }
    record.validate_shape().map_err(anyhow::Error::msg)?;
    if !record.verify() {
        return Ok(false);
    }
    let local_peer_id = app_state
        .local_peer_id
        .read()
        .ok()
        .and_then(|peer_id| peer_id.clone());
    let (newly_effective, current_applied, replaced_existing) = {
        let mut conn = app_state
            .db_conn
            .lock()
            .map_err(|error| anyhow!(error.to_string()))?;
        let replacing_existing = if let Some(existing) = db::get_group_record(&conn, record.id())? {
            if existing.group_id() != record.group_id() {
                return Err(anyhow!("Group record id collides with a different group"));
            }
            let existing_json = serde_json::to_vec(&existing)?;
            let candidate_json = serde_json::to_vec(record)?;
            if candidate_json >= existing_json {
                return Ok(false);
            }
            true
        } else {
            false
        };
        let old_records = db::get_all_group_records(&conn, record.group_id())?;
        let old_evaluation = crate::chat::group_state::evaluate_group_records(&old_records);
        if !replacing_existing
            && old_evaluation
                .state
                .as_ref()
                .is_some_and(|state| state.dissolved)
        {
            return Err(anyhow!("Group is dissolved"));
        }
        let old_effective = old_evaluation
            .decisions
            .iter()
            .filter_map(|(id, decision)| {
                matches!(
                    decision,
                    crate::chat::group_state::GroupRecordDecision::Effective
                )
                .then_some(id.as_str())
            })
            .collect::<HashSet<_>>();

        let mut preview_records = old_records.clone();
        if replacing_existing {
            preview_records.retain(|existing| existing.id() != record.id());
        }
        preview_records.push(record.clone());
        let preview = crate::chat::group_state::evaluate_group_records(&preview_records);
        if let Some(crate::chat::group_state::GroupRecordDecision::Rejected(reason)) =
            preview.decisions.get(record.id())
        {
            return Err(anyhow!(reason.clone()));
        }
        if matches!(
            preview.decisions.get(record.id()),
            Some(crate::chat::group_state::GroupRecordDecision::PendingDependencies(_))
        ) {
            let pending_for_group: i64 = conn.query_row(
                "SELECT COUNT(*) FROM group_records WHERE group_id = ?1 AND pending = 1",
                [record.group_id()],
                |row| row.get(0),
            )?;
            let pending_for_author: i64 = conn.query_row(
                "SELECT COUNT(*) FROM group_records
                 WHERE group_id = ?1 AND author_peer_id = ?2 AND pending = 1",
                rusqlite::params![record.group_id(), record.author_peer_id()],
                |row| row.get(0),
            )?;
            if pending_for_group >= crate::chat::group_state::MAX_PENDING_RECORDS_PER_GROUP
                || pending_for_author >= crate::chat::group_state::MAX_PENDING_RECORDS_PER_AUTHOR
            {
                return Err(anyhow!("Group pending-record limit reached"));
            }
        }

        let transaction = conn.transaction()?;
        if replacing_existing {
            db::replace_group_record(&transaction, record, true, true)?;
        } else {
            db::insert_group_record(&transaction, record, true, true)?;
        }
        let records = preview_records;
        let evaluation = preview;
        for (record_id, decision) in &evaluation.decisions {
            let pending = matches!(
                decision,
                crate::chat::group_state::GroupRecordDecision::PendingDependencies(_)
            );
            transaction.execute(
                "UPDATE group_records SET pending = ?2 WHERE id = ?1",
                rusqlite::params![record_id, i64::from(pending)],
            )?;
        }
        reconcile_group_projection(
            &transaction,
            record.group_id(),
            &records,
            &evaluation,
            local_peer_id.as_deref(),
        )?;

        let current_decision = evaluation.decisions.get(record.id()).cloned();
        let current_applied = matches!(
            current_decision,
            Some(crate::chat::group_state::GroupRecordDecision::Effective)
        );
        let newly_effective = records
            .into_iter()
            .filter(|candidate| {
                !old_effective.contains(candidate.id())
                    && matches!(
                        evaluation.decisions.get(candidate.id()),
                        Some(crate::chat::group_state::GroupRecordDecision::Effective)
                    )
            })
            .collect::<Vec<_>>();
        transaction.commit()?;

        if let Some(crate::chat::group_state::GroupRecordDecision::Rejected(reason)) =
            current_decision
        {
            return Err(anyhow!(reason));
        }
        (newly_effective, current_applied, replacing_existing)
    };

    if let Some(event_sink) = event_sink {
        if replaced_existing
            && !newly_effective
                .iter()
                .any(|applied_record| applied_record.id() == record.id())
        {
            event_sink.emit(CoreEvent::GroupRecordApplied(GroupRecordAppliedEvent {
                group_id: record.group_id().to_string(),
                record_id: record.id().to_string(),
                record_type: record.body().kind().to_string(),
            }));
        }
        for applied_record in &newly_effective {
            emit_group_record_events(event_sink, applied_record, local_peer_id.as_deref());
        }
    }
    Ok(current_applied)
}

fn reconcile_group_projection(
    conn: &rusqlite::Connection,
    group_id: &str,
    records: &[SignedGroupRecord],
    evaluation: &crate::chat::group_state::GroupEvaluation,
    local_peer_id: Option<&str>,
) -> anyhow::Result<()> {
    let Some(state) = evaluation.state.as_ref() else {
        return Ok(());
    };
    if state.dissolved {
        db::revoke_group_invites(conn, group_id)?;
        db::delete_group_chat(conn, group_id)?;
        conn.execute(
            "DELETE FROM group_message_receipts WHERE group_id = ?1",
            [group_id],
        )?;
        conn.execute(
            "DELETE FROM group_file_sources WHERE group_id = ?1",
            [group_id],
        )?;
        return Ok(());
    }

    let mut metadata_by_message_id = HashMap::new();
    {
        let mut statement = conn.prepare(
            "SELECT id, content_metadata FROM messages
             WHERE chat_id = ?1 AND content_metadata IS NOT NULL",
        )?;
        let rows = statement.query_map([group_id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        for row in rows {
            let (message_id, metadata) = row?;
            metadata_by_message_id.insert(message_id, metadata);
        }
    }

    db::upsert_chat(conn, group_id, &state.name, true)?;
    db::update_chat_image_hash(conn, group_id, state.image_hash.as_deref())?;
    conn.execute("DELETE FROM chat_peers WHERE chat_id = ?1", [group_id])?;
    conn.execute("DELETE FROM messages WHERE chat_id = ?1", [group_id])?;
    conn.execute(
        "DELETE FROM group_message_receipts WHERE group_id = ?1",
        [group_id],
    )?;
    conn.execute(
        "DELETE FROM group_file_sources WHERE group_id = ?1",
        [group_id],
    )?;

    for peer_id in &state.active_members {
        let stored_peer_id = stored_peer_id(peer_id, local_peer_id);
        if stored_peer_id != "Me" {
            ensure_peer(conn, stored_peer_id, "group")?;
        }
        db::upsert_chat_member_state(
            conn,
            group_id,
            stored_peer_id,
            if peer_id == &state.admin_peer_id {
                "admin"
            } else {
                "member"
            },
            "joined",
            None,
            None,
        )?;
    }
    for peer_id in &state.invited_members {
        let stored_peer_id = stored_peer_id(peer_id, local_peer_id);
        if stored_peer_id != "Me" {
            ensure_peer(conn, stored_peer_id, "group")?;
        }
        db::upsert_chat_member_state(
            conn,
            group_id,
            stored_peer_id,
            "member",
            "invited",
            None,
            None,
        )?;
    }

    let mut effective = records
        .iter()
        .filter(|candidate| {
            matches!(
                evaluation.decisions.get(candidate.id()),
                Some(crate::chat::group_state::GroupRecordDecision::Effective)
            )
        })
        .collect::<Vec<_>>();
    effective.sort_by(|left, right| {
        left.lamport_counter()
            .cmp(&right.lamport_counter())
            .then_with(|| left.author_peer_id().cmp(right.author_peer_id()))
            .then_with(|| left.id().cmp(right.id()))
    });
    for applied_record in effective {
        match applied_record.body() {
            GroupRecordBody::GroupCreated {
                image_hash: Some(file_hash),
                ..
            }
            | GroupRecordBody::FileAvailability { file_hash } => {
                ensure_incomplete_file_row(conn, file_hash)?;
                db::upsert_group_file_source(
                    conn,
                    group_id,
                    file_hash,
                    stored_peer_id(applied_record.author_peer_id(), local_peer_id),
                )?;
            }
            GroupRecordBody::Message {
                content_type,
                text_content,
                file_hash,
                sender_alias,
            } => {
                let mut message = group_record_to_db_message(
                    applied_record,
                    *content_type,
                    text_content.clone(),
                    file_hash.clone(),
                    sender_alias.clone(),
                );
                message.peer_id =
                    stored_peer_id(applied_record.author_peer_id(), local_peer_id).to_string();
                message.content_metadata = metadata_by_message_id.remove(applied_record.id());
                if message.peer_id != "Me" {
                    ensure_peer(conn, &message.peer_id, "group")?;
                }
                if let Some(file_hash) = file_hash {
                    ensure_incomplete_file_row(conn, file_hash)?;
                    db::upsert_group_file_source(conn, group_id, file_hash, &message.peer_id)?;
                }
                db::insert_message(conn, &message)?;
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
                        applied_record.author_peer_id(),
                        status.as_str(),
                        applied_record.timestamp(),
                    )?;
                }
            }
            _ => {}
        }
    }
    Ok(())
}

fn stored_peer_id<'a>(peer_id: &'a str, local_peer_id: Option<&str>) -> &'a str {
    if local_peer_id == Some(peer_id) {
        "Me"
    } else {
        peer_id
    }
}

fn emit_group_record_events(
    event_sink: &SharedCoreEventSink,
    record: &SignedGroupRecord,
    local_peer_id: Option<&str>,
) {
    event_sink.emit(CoreEvent::GroupRecordApplied(GroupRecordAppliedEvent {
        group_id: record.group_id().to_string(),
        record_id: record.id().to_string(),
        record_type: record.body().kind().to_string(),
    }));
    let roster = match record.body() {
        GroupRecordBody::GroupCreated { .. } => Some((record.author_peer_id(), "joined")),
        GroupRecordBody::MemberInvited { peer_id, .. } => Some((peer_id.as_str(), "invited")),
        GroupRecordBody::MemberJoined { peer_id } => Some((peer_id.as_str(), "joined")),
        GroupRecordBody::MemberLeft { peer_id } => Some((peer_id.as_str(), "left")),
        GroupRecordBody::MemberRemoved { peer_id } => Some((peer_id.as_str(), "removed")),
        GroupRecordBody::AdminTransferred { new_admin_peer_id } => {
            Some((new_admin_peer_id.as_str(), "admin"))
        }
        _ => None,
    };
    if let Some((peer_id, membership_state)) = roster {
        event_sink.emit(CoreEvent::GroupRosterUpdated(GroupRosterUpdatedEvent {
            group_id: record.group_id().to_string(),
            peer_id: stored_peer_id(peer_id, local_peer_id).to_string(),
            membership_state: membership_state.to_string(),
        }));
    }
    if let GroupRecordBody::Receipt {
        message_ids,
        status,
    } = record.body()
    {
        for message_id in message_ids {
            event_sink.emit(CoreEvent::GroupMessageReceiptUpdated(
                GroupMessageReceiptUpdatedEvent {
                    group_id: record.group_id().to_string(),
                    message_id: message_id.clone(),
                    peer_id: record.author_peer_id().to_string(),
                    status: status.as_str().to_string(),
                },
            ));
        }
    }
    if let GroupRecordBody::Message {
        content_type,
        text_content,
        file_hash,
        sender_alias,
    } = record.body()
    {
        let mut message = group_record_to_db_message(
            record,
            *content_type,
            text_content.clone(),
            file_hash.clone(),
            sender_alias.clone(),
        );
        message.peer_id = stored_peer_id(record.author_peer_id(), local_peer_id).to_string();
        event_sink.emit(CoreEvent::MessageReceived(message));
    }
}

pub fn store_incoming_invite(
    app_state: &AppState,
    event_sink: Option<&SharedCoreEventSink>,
    invite: &GroupInvitePayload,
) -> anyhow::Result<()> {
    validate_group_invite(invite)?;
    for record in &invite.related_records {
        apply_signed_record(app_state, None, record, true)?;
    }
    apply_signed_record(app_state, None, &invite.invite_record, true)?;

    let conn = app_state
        .db_conn
        .lock()
        .map_err(|e| anyhow!(e.to_string()))?;
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

fn validate_group_invite(invite: &GroupInvitePayload) -> anyhow::Result<()> {
    if invite.version != crate::network::gossip::GROUP_PROTOCOL_VERSION {
        return Err(anyhow!(
            "Unsupported group invite protocol version {}",
            invite.version
        ));
    }
    if invite.related_records.len() > 8 {
        return Err(anyhow!("Group invite bootstrap exceeds its record limit"));
    }
    invite
        .invite_record
        .validate_shape()
        .map_err(anyhow::Error::msg)?;
    if !invite.invite_record.verify() {
        return Err(anyhow!("Group invite signature could not be verified"));
    }
    if invite.invite_record.id() != invite.invite_id
        || invite.invite_record.group_id() != invite.group_id
        || invite.invite_record.author_peer_id() != invite.inviter_peer_id
    {
        return Err(anyhow!(
            "Group invite metadata does not match its signed record"
        ));
    }
    match invite.invite_record.body() {
        GroupRecordBody::MemberInvited { peer_id, .. } if peer_id == &invite.invitee_peer_id => {}
        _ => {
            return Err(anyhow!(
                "Group invite does not contain a matching invitation record"
            ))
        }
    }
    if invite.related_records.iter().any(|record| {
        record.validate_shape().is_err() || !record.verify() || record.group_id() != invite.group_id
    }) {
        return Err(anyhow!("Group invite history contains an invalid record"));
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
    apply_signed_record(app_state, None, &record, true)?;
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

pub(crate) fn sign_record(
    app_state: &AppState,
    keypair: &identity::Keypair,
    group_id: String,
    body: GroupRecordBody,
) -> anyhow::Result<SignedGroupRecord> {
    let (lamport_counter, parents) = {
        let conn = app_state
            .db_conn
            .lock()
            .map_err(|error| anyhow!(error.to_string()))?;
        let records = db::get_all_group_records(&conn, &group_id)?;
        crate::chat::group_state::evaluate_group_records(&records)
            .next_frontier()
            .unwrap_or((1, Vec::new()))
    };
    sign_record_at(
        keypair,
        group_id,
        timestamp_now(),
        parents,
        lamport_counter,
        body,
    )
}

fn sign_record_at(
    keypair: &identity::Keypair,
    group_id: String,
    timestamp: i64,
    parents: Vec<String>,
    lamport_counter: u64,
    body: GroupRecordBody,
) -> anyhow::Result<SignedGroupRecord> {
    SignedGroupRecord::new(
        keypair,
        group_id,
        format!("group-rec-{}-{}", timestamp, rand::random::<u32>()),
        timestamp,
        parents,
        lamport_counter,
        body,
    )
}

pub async fn load_or_create_local_keypair(
    app_state: &AppState,
) -> anyhow::Result<identity::Keypair> {
    let config_manager = app_state.config_manager.lock().await;
    let mut config = config_manager.load().await.unwrap_or_default();
    if let Some(ref key_b64) = config.user.libp2p_keypair {
        if let Ok(key_bytes) = BASE64.decode(key_b64) {
            if let Ok(keypair) = identity::Keypair::from_protobuf_encoding(&key_bytes) {
                if let Ok(mut local_peer_id) = app_state.local_peer_id.write() {
                    *local_peer_id = Some(PeerId::from_public_key(&keypair.public()).to_string());
                }
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
    if let Ok(mut local_peer_id) = app_state.local_peer_id.write() {
        *local_peer_id = Some(PeerId::from_public_key(&keypair.public()).to_string());
    }
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
    use rusqlite::OptionalExtension;
    use std::cell::RefCell;

    thread_local! {
        static TEST_FRONTIERS: RefCell<HashMap<String, (u64, String)>> = RefCell::new(HashMap::new());
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
            local_peer_id: Arc::new(std::sync::RwLock::new(None)),
            app_dir,
        }
    }

    fn keypair() -> identity::Keypair {
        identity::Keypair::generate_ed25519()
    }

    fn peer_id(keypair: &identity::Keypair) -> String {
        PeerId::from_public_key(&keypair.public()).to_string()
    }

    const TEST_GENESIS_RECORD_ID: &str = "test-genesis";

    fn test_group_id(founder: &identity::Keypair) -> String {
        test_group_id_for_record(founder, TEST_GENESIS_RECORD_ID)
    }

    fn test_group_id_for_record(founder: &identity::Keypair, record_id: &str) -> String {
        chat_kind::derive_group_chat_id(&peer_id(founder), record_id)
    }

    fn signed(
        keypair: &identity::Keypair,
        group_id: &str,
        id: &str,
        timestamp: i64,
        body: GroupRecordBody,
    ) -> SignedGroupRecord {
        let record_id = if matches!(&body, GroupRecordBody::GroupCreated { .. }) {
            TEST_GENESIS_RECORD_ID.to_string()
        } else {
            format!("test-{id}-{}", rand::random::<u64>())
        };
        let (counter, parents) = TEST_FRONTIERS.with(|frontiers| {
            frontiers
                .borrow()
                .get(group_id)
                .map(|(counter, parent)| (counter + 1, vec![parent.clone()]))
                .unwrap_or((1, Vec::new()))
        });
        let record = SignedGroupRecord::new(
            keypair,
            group_id.to_string(),
            record_id.clone(),
            timestamp,
            parents,
            counter,
            body,
        )
        .expect("record");
        TEST_FRONTIERS.with(|frontiers| {
            frontiers
                .borrow_mut()
                .insert(group_id.to_string(), (counter, record_id));
        });
        record
    }

    fn signed_at(
        keypair: &identity::Keypair,
        group_id: &str,
        id: &str,
        timestamp: i64,
        counter: u64,
        parents: Vec<String>,
        body: GroupRecordBody,
    ) -> SignedGroupRecord {
        let record_id = if counter == 1 && matches!(&body, GroupRecordBody::GroupCreated { .. }) {
            id.to_string()
        } else {
            format!("test-{id}-{}", rand::random::<u64>())
        };
        SignedGroupRecord::new(
            keypair,
            group_id.to_string(),
            record_id,
            timestamp,
            parents,
            counter,
            body,
        )
        .expect("record")
    }

    fn apply(app_state: &AppState, record: &SignedGroupRecord) -> anyhow::Result<bool> {
        apply_signed_record(app_state, None, record, true)
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

    #[test]
    fn invite_gated_join_requires_existing_invite_payload() {
        let app_state = app_state();
        let conn = app_state.db_conn.lock().expect("db");
        let missing = db::get_group_invite_payload(&conn, "missing").expect("query");
        assert!(missing.is_none());
    }

    #[test]
    fn group_invite_requires_exact_v3_metadata_and_signed_invitation() {
        let founder = keypair();
        let invitee = keypair();
        let group_id = test_group_id(&founder);
        let invitee_peer_id = peer_id(&invitee);
        let invite_record = SignedGroupRecord::new(
            &founder,
            group_id.clone(),
            "invite-record".to_string(),
            2,
            vec!["created".to_string()],
            2,
            GroupRecordBody::MemberInvited {
                peer_id: invitee_peer_id.clone(),
                role: "member".to_string(),
            },
        )
        .expect("invite record");
        let mut invite = GroupInvitePayload {
            version: crate::network::gossip::GROUP_PROTOCOL_VERSION,
            invite_id: invite_record.id().to_string(),
            group_id,
            group_name: "Test".to_string(),
            inviter_peer_id: peer_id(&founder),
            invitee_peer_id,
            created_at: 2,
            invite_record,
            related_records: Vec::new(),
        };

        validate_group_invite(&invite).expect("valid v3 invite");
        invite.version = 2;
        assert!(validate_group_invite(&invite)
            .expect_err("v2 must be rejected")
            .to_string()
            .contains("protocol version"));
    }

    #[test]
    fn incoming_invite_bootstraps_genesis_and_keeps_missing_history_pending() {
        let app_state = app_state();
        let founder = keypair();
        let invitee = keypair();
        let group_id = test_group_id(&founder);
        let genesis = SignedGroupRecord::new(
            &founder,
            group_id.clone(),
            TEST_GENESIS_RECORD_ID.to_string(),
            1,
            Vec::new(),
            1,
            GroupRecordBody::GroupCreated {
                name: "Test".to_string(),
                settings: None,
                image_hash: None,
            },
        )
        .expect("genesis");
        let invite_record = SignedGroupRecord::new(
            &founder,
            group_id.clone(),
            "invite-record".to_string(),
            3,
            vec!["missing-policy-record".to_string()],
            3,
            GroupRecordBody::MemberInvited {
                peer_id: peer_id(&invitee),
                role: "member".to_string(),
            },
        )
        .expect("invite");
        let payload = GroupInvitePayload {
            version: crate::network::gossip::GROUP_PROTOCOL_VERSION,
            invite_id: invite_record.id().to_string(),
            group_id: group_id.clone(),
            group_name: "Test".to_string(),
            inviter_peer_id: peer_id(&founder),
            invitee_peer_id: peer_id(&invitee),
            created_at: 3,
            invite_record,
            related_records: vec![genesis],
        };

        store_incoming_invite(&app_state, None, &payload).expect("store invite");

        assert_eq!(
            get_group_policy(&app_state, &group_id)
                .expect("genesis policy")
                .admin_peer_id,
            peer_id(&founder)
        );
        let pending = get_group_pending_record_summary(&app_state, &group_id).expect("pending");
        assert_eq!(pending.count, 1);
    }

    #[test]
    fn incoming_invite_rejects_a_causally_unauthorized_inviter() {
        let app_state = app_state();
        let founder = keypair();
        let attacker = keypair();
        let invitee = keypair();
        let group_id = test_group_id(&founder);
        let genesis = signed(
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
        let invite_record = SignedGroupRecord::new(
            &attacker,
            group_id.clone(),
            "unauthorized-invite".to_string(),
            2,
            vec![genesis.id().to_string()],
            2,
            GroupRecordBody::MemberInvited {
                peer_id: peer_id(&invitee),
                role: "member".to_string(),
            },
        )
        .expect("invite record");
        let payload = GroupInvitePayload {
            version: crate::network::gossip::GROUP_PROTOCOL_VERSION,
            invite_id: invite_record.id().to_string(),
            group_id,
            group_name: "Test".to_string(),
            inviter_peer_id: peer_id(&attacker),
            invitee_peer_id: peer_id(&invitee),
            created_at: 2,
            invite_record,
            related_records: vec![genesis],
        };

        assert!(store_incoming_invite(&app_state, None, &payload).is_err());
        let conn = app_state.db_conn.lock().expect("db");
        assert!(db::get_group_invite_payload(&conn, &payload.invite_id)
            .expect("invite query")
            .is_none());
    }

    #[test]
    fn pending_records_are_bounded_per_author_before_storage() {
        let app_state = app_state();
        let author = keypair();
        let group_id = test_group_id(&author);
        {
            let conn = app_state.db_conn.lock().expect("db");
            for index in 0..crate::chat::group_state::MAX_PENDING_RECORDS_PER_AUTHOR {
                let record = SignedGroupRecord::new(
                    &author,
                    group_id.clone(),
                    format!("pending-{index}"),
                    index,
                    vec![format!("missing-{index}")],
                    2,
                    GroupRecordBody::Head { heads: Vec::new() },
                )
                .expect("pending record");
                db::insert_group_record(&conn, &record, true, true).expect("store pending");
            }
        }
        let overflow = SignedGroupRecord::new(
            &author,
            group_id,
            "pending-overflow".to_string(),
            65,
            vec!["still-missing".to_string()],
            2,
            GroupRecordBody::Head { heads: Vec::new() },
        )
        .expect("overflow record");

        assert!(apply(&app_state, &overflow)
            .expect_err("pending cap must reject")
            .to_string()
            .contains("pending-record limit"));
        let conn = app_state.db_conn.lock().expect("db");
        assert!(!db::group_record_exists(&conn, overflow.id()));
    }

    #[test]
    fn rejected_records_are_not_persisted() {
        let app_state = app_state();
        let founder = keypair();
        let attacker = keypair();
        let group_id = test_group_id(&founder);
        let genesis = signed(
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
        apply(&app_state, &genesis).expect("created");
        let unauthorized = SignedGroupRecord::new(
            &attacker,
            group_id,
            "unauthorized-rename".to_string(),
            2,
            vec![genesis.id().to_string()],
            2,
            GroupRecordBody::GroupRenamed {
                name: "Hijacked".to_string(),
            },
        )
        .expect("record");

        assert!(apply(&app_state, &unauthorized).is_err());
        let conn = app_state.db_conn.lock().expect("db");
        assert!(!db::group_record_exists(&conn, unauthorized.id()));
    }

    #[test]
    fn default_group_policy_makes_founder_admin_and_disables_member_invites() {
        let app_state = app_state();
        let founder = keypair();
        let group_id = test_group_id(&founder);
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
        let founder = keypair();
        let older = keypair();
        let newer = keypair();
        let group_id = test_group_id(&founder);
        let older_id = peer_id(&older);
        let newer_id = peer_id(&newer);
        let founder_id = peer_id(&founder);

        let records = [
            signed(
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
            signed(
                &founder,
                &group_id,
                "invite-older",
                2,
                GroupRecordBody::MemberInvited {
                    peer_id: older_id.clone(),
                    role: "member".to_string(),
                },
            ),
            signed(
                &older,
                &group_id,
                "join-older",
                3,
                GroupRecordBody::MemberJoined {
                    peer_id: older_id.clone(),
                },
            ),
            signed(
                &founder,
                &group_id,
                "invite-newer",
                4,
                GroupRecordBody::MemberInvited {
                    peer_id: newer_id.clone(),
                    role: "member".to_string(),
                },
            ),
            signed(
                &newer,
                &group_id,
                "join-newer",
                5,
                GroupRecordBody::MemberJoined {
                    peer_id: newer_id.clone(),
                },
            ),
        ];
        for record in &records {
            apply(&app_state, record).expect("membership record");
        }

        let policy = get_group_policy(&app_state, &group_id).expect("policy");
        assert_eq!(
            policy.automatic_successor_peer_id.as_deref(),
            Some(older_id.as_str())
        );

        apply(
            &app_state,
            &signed(
                &founder,
                &group_id,
                "transfer",
                6,
                GroupRecordBody::AdminTransferred {
                    new_admin_peer_id: newer_id.clone(),
                },
            ),
        )
        .expect("transfer");
        let policy = get_group_policy(&app_state, &group_id).expect("transferred policy");
        assert_eq!(policy.admin_peer_id, newer_id);
        assert_eq!(
            policy.automatic_successor_peer_id.as_deref(),
            Some(founder_id.as_str())
        );
    }

    #[test]
    fn sole_administrator_can_dissolve_and_later_records_are_rejected() {
        let app_state = app_state();
        let founder = keypair();
        let group_id = test_group_id(&founder);
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
                "dissolved",
                2,
                GroupRecordBody::GroupDissolved,
            ),
        )
        .expect("dissolved");

        let policy = get_group_policy(&app_state, &group_id).expect("policy");
        assert!(policy.dissolved);
        let error = apply(
            &app_state,
            &signed(
                &founder,
                &group_id,
                "message",
                3,
                GroupRecordBody::Message {
                    content_type: GroupContentType::Text,
                    text_content: Some("too late".to_string()),
                    file_hash: None,
                    sender_alias: Some("Founder".to_string()),
                },
            ),
        )
        .expect_err("records after dissolution must fail");
        assert!(
            error.to_string().contains("dissolved"),
            "unexpected post-dissolution error: {error}"
        );

        let error = apply(
            &app_state,
            &signed(
                &founder,
                &group_id,
                "backdated",
                1,
                GroupRecordBody::Message {
                    content_type: GroupContentType::Text,
                    text_content: Some("backdated after tombstone".to_string()),
                    file_hash: None,
                    sender_alias: None,
                },
            ),
        )
        .expect_err("a known dissolution must reject backdated records too");
        assert!(
            error.to_string().contains("dissolved"),
            "unexpected backdated error: {error}"
        );
    }

    #[test]
    fn revoked_invitee_can_sync_the_dissolution_proof() {
        let app_state = app_state();
        let founder = keypair();
        let invitee = keypair();
        let invitee_id = peer_id(&invitee);
        let group_id = test_group_id(&founder);
        let genesis = signed(
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
        apply(&app_state, &genesis).expect("created");
        let invite_record = signed(
            &founder,
            &group_id,
            "invite",
            2,
            GroupRecordBody::MemberInvited {
                peer_id: invitee_id.clone(),
                role: "member".to_string(),
            },
        );
        apply(&app_state, &invite_record).expect("invite");
        let payload = GroupInvitePayload {
            version: crate::network::gossip::GROUP_PROTOCOL_VERSION,
            invite_id: invite_record.id().to_string(),
            group_id: group_id.clone(),
            group_name: "Test".to_string(),
            inviter_peer_id: peer_id(&founder),
            invitee_peer_id: invitee_id.clone(),
            created_at: 2,
            invite_record,
            related_records: vec![genesis],
        };
        {
            let conn = app_state.db_conn.lock().expect("db");
            db::upsert_group_invite(&conn, &payload, "sent").expect("store invite");
        }
        let dissolution = signed(
            &founder,
            &group_id,
            "dissolved",
            3,
            GroupRecordBody::GroupDissolved,
        );
        apply(&app_state, &dissolution).expect("dissolve");

        assert!(
            can_peer_sync_group_records(&app_state, &group_id, &invitee_id).expect("sync access")
        );
    }

    #[test]
    fn revoked_invitee_cannot_sync_an_active_group() {
        let app_state = app_state();
        let founder = keypair();
        let invitee = keypair();
        let invitee_id = peer_id(&invitee);
        let group_id = test_group_id(&founder);
        let genesis = signed(
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
        apply(&app_state, &genesis).expect("created");
        let invite_record = signed(
            &founder,
            &group_id,
            "invite",
            2,
            GroupRecordBody::MemberInvited {
                peer_id: invitee_id.clone(),
                role: "member".to_string(),
            },
        );
        apply(&app_state, &invite_record).expect("invite");
        let payload = GroupInvitePayload {
            version: crate::network::gossip::GROUP_PROTOCOL_VERSION,
            invite_id: invite_record.id().to_string(),
            group_id: group_id.clone(),
            group_name: "Test".to_string(),
            inviter_peer_id: peer_id(&founder),
            invitee_peer_id: invitee_id.clone(),
            created_at: 2,
            invite_record,
            related_records: vec![genesis],
        };
        {
            let conn = app_state.db_conn.lock().expect("db");
            db::upsert_group_invite(&conn, &payload, "sent").expect("store invite");
            db::revoke_group_invites(&conn, &group_id).expect("revoke invite");
        }

        assert!(
            !can_peer_sync_group_records(&app_state, &group_id, &invitee_id).expect("sync access")
        );
    }

    #[test]
    fn leave_waits_for_parent_admin_transfer() {
        let app_state = app_state();
        let founder = keypair();
        let successor = keypair();
        let founder_id = peer_id(&founder);
        let successor_id = peer_id(&successor);
        let group_id = test_group_id(&founder);
        for record in [
            signed(
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
            signed(
                &founder,
                &group_id,
                "invited",
                2,
                GroupRecordBody::MemberInvited {
                    peer_id: successor_id.clone(),
                    role: "member".to_string(),
                },
            ),
            signed(
                &successor,
                &group_id,
                "joined",
                3,
                GroupRecordBody::MemberJoined {
                    peer_id: successor_id.clone(),
                },
            ),
        ] {
            apply(&app_state, &record).expect("setup record");
        }
        let transfer = signed(
            &founder,
            &group_id,
            "transfer-parent",
            4,
            GroupRecordBody::AdminTransferred {
                new_admin_peer_id: successor_id.clone(),
            },
        );
        let leave = SignedGroupRecord::new(
            &founder,
            group_id.clone(),
            "leave-child".to_string(),
            5,
            vec![transfer.id().to_string()],
            transfer.lamport_counter().saturating_add(1),
            GroupRecordBody::MemberLeft {
                peer_id: founder_id.clone(),
            },
        )
        .expect("leave record");

        assert!(!apply(&app_state, &leave).expect("pending leave"));
        apply(&app_state, &transfer).expect("transfer");
        let policy = get_group_policy(&app_state, &group_id).expect("policy");
        assert_eq!(policy.admin_peer_id, successor_id);
        assert!(!policy.active_members.contains(&founder_id));
    }

    #[test]
    fn applying_group_created_stores_group_image_hash() {
        let app_state = app_state();
        let founder = keypair();
        let group_id = test_group_id(&founder);
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
        let founder = keypair();
        let group_id = test_group_id(&founder);
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
        let founder = keypair();
        let member = keypair();
        let group_id = test_group_id(&founder);

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
    fn member_join_without_invite_is_rejected() {
        let app_state = app_state();
        let founder = keypair();
        let member = keypair();
        let group_id = test_group_id(&founder);

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

        assert!(apply(&app_state, &joined).is_err());
        let policy = get_group_policy(&app_state, &group_id).expect("policy");
        assert!(!policy.active_members.contains(&peer_id(&member)));
    }

    #[test]
    fn member_invite_is_accepted_when_setting_allows_it() {
        let app_state = app_state();
        let founder = keypair();
        let member = keypair();
        let invited_by_member = keypair();
        let group_id = test_group_id(&founder);

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
        let founder = keypair();
        let member = keypair();
        let group_id = test_group_id(&founder);

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
        let invite = signed(
            &founder,
            &group_id,
            "invite-member",
            3,
            GroupRecordBody::MemberInvited {
                peer_id: peer_id(&member),
                role: "member".to_string(),
            },
        );
        let joined = signed(
            &member,
            &group_id,
            "member-joined",
            4,
            GroupRecordBody::MemberJoined {
                peer_id: peer_id(&member),
            },
        );
        assert!(!apply(&app_state, &joined).expect("pending join"));
        apply(&app_state, &invite).expect("invite");

        let policy = get_group_policy(&app_state, &group_id).expect("policy");
        assert!(policy.active_members.contains(&peer_id(&member)));
    }

    #[test]
    fn pre_join_message_remains_rejected_after_member_later_joins() {
        let app_state = app_state();
        let founder = keypair();
        let member = keypair();
        let group_id = test_group_id(&founder);
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
        let message = signed_at(
            &member,
            &group_id,
            "pre-join-message",
            2,
            2,
            vec![created.id().to_string()],
            GroupRecordBody::Message {
                content_type: GroupContentType::Text,
                text_content: Some("too early".to_string()),
                file_hash: None,
                sender_alias: None,
            },
        );
        assert!(apply(&app_state, &message).is_err());
        let invite = signed_at(
            &founder,
            &group_id,
            "invite-member",
            3,
            2,
            vec![created.id().to_string()],
            GroupRecordBody::MemberInvited {
                peer_id: peer_id(&member),
                role: "member".to_string(),
            },
        );
        apply(&app_state, &invite).expect("invite");
        let joined = signed_at(
            &member,
            &group_id,
            "member-joined",
            4,
            3,
            vec![invite.id().to_string()],
            GroupRecordBody::MemberJoined {
                peer_id: peer_id(&member),
            },
        );
        apply(&app_state, &joined).expect("joined");

        let conn = app_state.db_conn.lock().expect("db");
        assert_eq!(
            group_record_state(&conn, message.id()).expect("state"),
            None,
            "causally rejected records must not consume durable storage"
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
    fn persisted_projection_converges_for_reverse_arrival_order() {
        let ordered_state = app_state();
        let reversed_state = app_state();
        let founder = keypair();
        let member = keypair();
        let member_id = peer_id(&member);
        let group_id = test_group_id_for_record(&founder, "created");
        let created = signed_at(
            &founder,
            &group_id,
            "created",
            1,
            1,
            Vec::new(),
            GroupRecordBody::GroupCreated {
                name: "Initial".into(),
                settings: None,
                image_hash: None,
            },
        );
        let invite = signed_at(
            &founder,
            &group_id,
            "invite",
            2,
            2,
            vec![created.id().to_string()],
            GroupRecordBody::MemberInvited {
                peer_id: member_id.clone(),
                role: "member".into(),
            },
        );
        let join = signed_at(
            &member,
            &group_id,
            "join",
            3,
            3,
            vec![invite.id().to_string()],
            GroupRecordBody::MemberJoined {
                peer_id: member_id.clone(),
            },
        );
        let rename = signed_at(
            &founder,
            &group_id,
            "rename",
            4,
            4,
            vec![join.id().to_string()],
            GroupRecordBody::GroupRenamed {
                name: "Converged".into(),
            },
        );
        let message = signed_at(
            &member,
            &group_id,
            "message",
            4,
            4,
            vec![join.id().to_string()],
            GroupRecordBody::Message {
                content_type: GroupContentType::Text,
                text_content: Some("hello".into()),
                file_hash: None,
                sender_alias: Some("Member".into()),
            },
        );
        let records = vec![created, invite, join, rename, message];

        for record in &records {
            apply(&ordered_state, record).expect("ordered apply");
        }
        for record in records.iter().rev() {
            apply(&reversed_state, record).expect("reverse apply");
        }

        let ordered_policy = get_group_policy(&ordered_state, &group_id).expect("ordered policy");
        let reversed_policy =
            get_group_policy(&reversed_state, &group_id).expect("reversed policy");
        assert_eq!(ordered_policy.admin_peer_id, reversed_policy.admin_peer_id);
        assert_eq!(
            ordered_policy.active_members,
            reversed_policy.active_members
        );
        assert_eq!(
            get_group_roster(&ordered_state, &group_id).expect("ordered roster"),
            get_group_roster(&reversed_state, &group_id).expect("reversed roster")
        );
        for state in [&ordered_state, &reversed_state] {
            let conn = state.db_conn.lock().expect("db");
            let messages = db::get_messages(&conn, &group_id).expect("messages");
            assert_eq!(messages.len(), 1);
            assert_eq!(messages[0].text_content.as_deref(), Some("hello"));
            assert_eq!(messages[0].sender_alias.as_deref(), Some("Member"));
            let chat = db::get_chat_list(&conn)
                .expect("chats")
                .into_iter()
                .find(|chat| chat.id == group_id)
                .expect("group chat");
            assert_eq!(chat.name, "Converged");
        }
    }

    #[test]
    fn conflicting_payloads_with_the_same_record_id_converge_canonically() {
        let first_state = app_state();
        let second_state = app_state();
        let founder = keypair();
        let group_id = test_group_id_for_record(&founder, "genesis");
        let genesis = SignedGroupRecord::new(
            &founder,
            group_id.clone(),
            "genesis".to_string(),
            1,
            Vec::new(),
            1,
            GroupRecordBody::GroupCreated {
                name: "Test".to_string(),
                settings: None,
                image_hash: None,
            },
        )
        .expect("genesis");
        let left = SignedGroupRecord::new(
            &founder,
            group_id.clone(),
            "equivocated".to_string(),
            2,
            vec![genesis.id().to_string()],
            2,
            GroupRecordBody::Message {
                content_type: GroupContentType::Text,
                text_content: Some("left".to_string()),
                file_hash: None,
                sender_alias: None,
            },
        )
        .expect("left");
        let right = SignedGroupRecord::new(
            &founder,
            group_id.clone(),
            "equivocated".to_string(),
            2,
            vec![genesis.id().to_string()],
            2,
            GroupRecordBody::Message {
                content_type: GroupContentType::Text,
                text_content: Some("right".to_string()),
                file_hash: None,
                sender_alias: None,
            },
        )
        .expect("right");
        let (canonical, other) = if serde_json::to_vec(&left).expect("left json")
            < serde_json::to_vec(&right).expect("right json")
        {
            (left, right)
        } else {
            (right, left)
        };

        for (state, records) in [
            (&first_state, vec![other.clone(), canonical.clone()]),
            (&second_state, vec![canonical.clone(), other]),
        ] {
            apply(state, &genesis).expect("apply genesis");
            for record in records {
                let _ = apply(state, &record);
            }
        }

        for state in [&first_state, &second_state] {
            let conn = state.db_conn.lock().expect("db");
            let messages = db::get_messages(&conn, &group_id).expect("messages");
            assert_eq!(messages.len(), 1);
            let expected_text = match canonical.body() {
                GroupRecordBody::Message { text_content, .. } => text_content,
                _ => unreachable!(),
            };
            assert_eq!(messages[0].text_content.as_ref(), expected_text.as_ref());
        }
    }

    #[test]
    fn canonical_record_replacement_still_converges_after_dissolution() {
        let app_state = app_state();
        let founder = keypair();
        let group_id = test_group_id_for_record(&founder, "genesis");
        let genesis = signed_at(
            &founder,
            &group_id,
            "genesis",
            1,
            1,
            Vec::new(),
            GroupRecordBody::GroupCreated {
                name: "Test".to_string(),
                settings: None,
                image_hash: None,
            },
        );
        let left = SignedGroupRecord::new(
            &founder,
            group_id.clone(),
            "equivocated".to_string(),
            2,
            vec![genesis.id().to_string()],
            2,
            GroupRecordBody::Message {
                content_type: GroupContentType::Text,
                text_content: Some("left".to_string()),
                file_hash: None,
                sender_alias: None,
            },
        )
        .expect("left");
        let right = SignedGroupRecord::new(
            &founder,
            group_id.clone(),
            "equivocated".to_string(),
            2,
            vec![genesis.id().to_string()],
            2,
            GroupRecordBody::Message {
                content_type: GroupContentType::Text,
                text_content: Some("right".to_string()),
                file_hash: None,
                sender_alias: None,
            },
        )
        .expect("right");
        let (canonical, other) = if serde_json::to_vec(&left).expect("left json")
            < serde_json::to_vec(&right).expect("right json")
        {
            (left, right)
        } else {
            (right, left)
        };
        let dissolution = signed_at(
            &founder,
            &group_id,
            "dissolved",
            3,
            3,
            vec![other.id().to_string()],
            GroupRecordBody::GroupDissolved,
        );

        apply(&app_state, &genesis).expect("genesis");
        apply(&app_state, &other).expect("first payload");
        apply(&app_state, &dissolution).expect("dissolution");
        apply(&app_state, &canonical).expect("canonical replacement");

        let conn = app_state.db_conn.lock().expect("db");
        let stored = db::get_group_record(&conn, canonical.id())
            .expect("record query")
            .expect("stored record");
        assert_eq!(
            serde_json::to_vec(&stored).expect("stored json"),
            serde_json::to_vec(&canonical).expect("canonical json")
        );
    }

    #[test]
    fn active_members_and_open_invitees_can_sync_group_records() {
        let app_state = app_state();
        let founder = keypair();
        let member = keypair();
        let outsider = keypair();
        let group_id = test_group_id(&founder);

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

        assert!(
            can_peer_sync_group_records(&app_state, &group_id, &peer_id(&founder))
                .expect("founder sync")
        );
        assert!(
            can_peer_sync_group_records(&app_state, &group_id, &peer_id(&member))
                .expect("member sync")
        );
        assert!(
            !can_peer_sync_group_records(&app_state, &group_id, &peer_id(&outsider))
                .expect("outsider sync")
        );

        let invite_record = signed(
            &founder,
            &group_id,
            "invite-outsider",
            4,
            GroupRecordBody::MemberInvited {
                peer_id: peer_id(&outsider),
                role: "member".to_string(),
            },
        );
        let payload = GroupInvitePayload {
            version: crate::network::gossip::GROUP_PROTOCOL_VERSION,
            invite_id: invite_record.id().to_string(),
            group_id: group_id.clone(),
            group_name: "Test".to_string(),
            inviter_peer_id: peer_id(&founder),
            invitee_peer_id: peer_id(&outsider),
            created_at: 4,
            invite_record,
            related_records: Vec::new(),
        };
        {
            let conn = app_state.db_conn.lock().expect("db");
            db::upsert_group_invite(&conn, &payload, "sent").expect("sent invite");
        }
        assert!(
            can_peer_sync_group_records(&app_state, &group_id, &peer_id(&outsider))
                .expect("invitee sync")
        );
    }

    #[test]
    fn admin_leave_is_blocked_until_succession_exists() {
        let app_state = app_state();
        let founder = keypair();
        let group_id = test_group_id(&founder);

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
        let founder = keypair();
        let member = keypair();
        let group_id = test_group_id(&founder);

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
        let founder = keypair();
        let member = keypair();
        let group_id = test_group_id(&founder);
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
}
