use super::*;

impl NetworkManager {
    fn is_persisted_chat_message_id(msg_id: &str) -> bool {
        let mut parts = msg_id.split('-');
        let Some(ts) = parts.next() else {
            return false;
        };
        let Some(rand) = parts.next() else {
            return false;
        };
        if parts.next().is_some() {
            return false;
        }
        ts.parse::<i64>().is_ok() && rand.parse::<u32>().is_ok()
    }

    pub(super) async fn handle_direct_message_event(
        &mut self,
        event: libp2p::request_response::Event<
            crate::network::direct_message::DirectMessageRequest,
            crate::network::direct_message::DirectMessageResponse,
        >,
    ) {
        use libp2p::request_response::{Event, Message};

        match event {
            Event::Message { peer, message, .. } => match message {
                Message::Request {
                    request, channel, ..
                } => {
                    println!("[DM] 📥 Received {:?} from {}", request.msg_type, peer);

                    use crate::network::direct_message::DirectMessageKind;
                    match request.msg_type {
                        DirectMessageKind::Text
                        | DirectMessageKind::Image
                        | DirectMessageKind::Sticker
                        | DirectMessageKind::Document
                        | DirectMessageKind::Video
                        | DirectMessageKind::Audio => {
                            let status = self.handle_incoming_user_message(peer, &request).await;
                            match status {
                                Ok(()) => {
                                    self.send_status_response(
                                        channel,
                                        request.id,
                                        "delivered",
                                        None,
                                    );
                                }
                                Err(err) => {
                                    self.send_status_response(
                                        channel,
                                        request.id,
                                        "error",
                                        Some(err),
                                    );
                                }
                            }
                        }
                        DirectMessageKind::InviteHandshake => {
                            self.handle_invite_handshake(&request).await;
                            self.send_status_response(
                                channel,
                                request.id.clone(),
                                "delivered",
                                None,
                            );
                        }
                        DirectMessageKind::TempHandshake => {
                            self.handle_temp_handshake(peer, &request).await;
                            self.send_status_response(
                                channel,
                                request.id.clone(),
                                "delivered",
                                None,
                            );
                        }
                        DirectMessageKind::CallOffer
                        | DirectMessageKind::CallOfferVideo
                        | DirectMessageKind::CallAccept
                        | DirectMessageKind::CallAcceptVideo
                        | DirectMessageKind::CallReject
                        | DirectMessageKind::CallBusy
                        | DirectMessageKind::CallEnd => {
                            match self.handle_call_signal(peer, &request).await {
                                Ok(()) => self.send_status_response(
                                    channel,
                                    request.id.clone(),
                                    "delivered",
                                    None,
                                ),
                                Err(err) => self.send_status_response(
                                    channel,
                                    request.id.clone(),
                                    "error",
                                    Some(err),
                                ),
                            }
                        }
                        DirectMessageKind::BroadcastOffer
                        | DirectMessageKind::BroadcastAccept
                        | DirectMessageKind::BroadcastReject
                        | DirectMessageKind::BroadcastBusy
                        | DirectMessageKind::BroadcastEnd => {
                            match self.handle_broadcast_signal(peer, &request).await {
                                Ok(()) => self.send_status_response(
                                    channel,
                                    request.id.clone(),
                                    "delivered",
                                    None,
                                ),
                                Err(err) => self.send_status_response(
                                    channel,
                                    request.id.clone(),
                                    "error",
                                    Some(err),
                                ),
                            }
                        }
                        DirectMessageKind::ReadReceipt => {
                            match self.handle_read_receipt(&request).await {
                                Ok(_) => self.send_status_response(
                                    channel,
                                    request.id,
                                    "delivered",
                                    None,
                                ),
                                Err(err) => self.send_status_response(
                                    channel,
                                    request.id,
                                    "error",
                                    Some(err),
                                ),
                            }
                        }
                        DirectMessageKind::GroupInvite => {
                            match self.handle_group_invite(&request).await {
                                Ok(()) => self.send_status_response(
                                    channel,
                                    request.id,
                                    "delivered",
                                    None,
                                ),
                                Err(err) => self.send_status_response(
                                    channel,
                                    request.id,
                                    "error",
                                    Some(err),
                                ),
                            }
                        }
                        DirectMessageKind::GroupSyncRequest => {
                            match self.handle_group_sync_request(peer, &request).await {
                                Ok(()) => self.send_status_response(
                                    channel,
                                    request.id,
                                    "delivered",
                                    None,
                                ),
                                Err(err) => self.send_status_response(
                                    channel,
                                    request.id,
                                    "error",
                                    Some(err),
                                ),
                            }
                        }
                        DirectMessageKind::GroupSyncResponse => {
                            match self.handle_group_sync_response(&request).await {
                                Ok(()) => self.send_status_response(
                                    channel,
                                    request.id,
                                    "delivered",
                                    None,
                                ),
                                Err(err) => self.send_status_response(
                                    channel,
                                    request.id,
                                    "error",
                                    Some(err),
                                ),
                            }
                        }
                        DirectMessageKind::FileMetadataRequest => {
                            self.handle_file_metadata_request(peer, &request).await;
                            self.send_status_response(channel, request.id, "delivered", None);
                        }
                        DirectMessageKind::ChunkRequest => {
                            self.handle_chunk_request(peer, &request).await;
                            self.send_status_response(channel, request.id, "delivered", None);
                        }
                        DirectMessageKind::FileMetadataResponse => {
                            self.handle_file_metadata_response(peer, &request).await;
                            self.send_status_response(channel, request.id, "delivered", None);
                        }
                        DirectMessageKind::ChunkResponse => {
                            self.handle_chunk_response(&request).await;
                            self.send_status_response(channel, request.id, "delivered", None);
                        }
                    }
                }
                Message::Response {
                    request_id,
                    response,
                } => {
                    println!(
                        "[DM] 📦 Response for {:?}: {} for msg {}",
                        request_id, response.status, response.msg_id
                    );

                    if response.status == "delivered"
                        && Self::is_persisted_chat_message_id(&response.msg_id)
                    {
                        match self.persist_delivered_status(response.msg_id.clone()).await {
                            Ok(()) => {
                                self.emit(CoreEvent::MessageStatusUpdated(
                                    crate::events::MessageStatusUpdatedEvent {
                                        msg_id: response.msg_id,
                                        status: "delivered".to_string(),
                                    },
                                ));
                            }
                            Err(err) => {
                                let mut updated_runtime = false;
                                {
                                                                        let network_state =
                                        &self.network_state;
                                    let mut temp_state = network_state.temporary_state.lock().await;
                                    for msgs in temp_state.messages.values_mut() {
                                        if let Some(found) =
                                            msgs.iter_mut().find(|m| m.id == response.msg_id)
                                        {
                                            found.status = "delivered".to_string();
                                            updated_runtime = true;
                                            break;
                                        }
                                    }
                                }

                                if updated_runtime {
                                    self.emit(CoreEvent::MessageStatusUpdated(
                                        crate::events::MessageStatusUpdatedEvent {
                                            msg_id: response.msg_id.clone(),
                                            status: "delivered".to_string(),
                                        },
                                    ));
                                } else {
                                    eprintln!(
                                        "[DM] ❌ Failed to persist delivered status {}: {}",
                                        response.msg_id, err
                                    );
                                }
                            }
                        }
                    }
                }
            },
            Event::OutboundFailure {
                peer,
                request_id,
                error,
                ..
            } => {
                eprintln!(
                    "[DM] Outbound failure to {} for {:?}: {:?}",
                    peer, request_id, error
                );
            }
            Event::InboundFailure { peer, error, .. } => {
                eprintln!("[DM] Inbound failure from {}: {:?}", peer, error);
            }
            _ => {}
        }
    }

    fn send_status_response(
        &mut self,
        channel: libp2p::request_response::ResponseChannel<
            crate::network::direct_message::DirectMessageResponse,
        >,
        msg_id: String,
        status: &str,
        error: Option<String>,
    ) {
        let response = crate::network::direct_message::DirectMessageResponse {
            msg_id,
            status: status.to_string(),
            error,
        };
        let _ = self
            .swarm
            .behaviour_mut()
            .direct_message
            .send_response(channel, response);
    }

    async fn handle_group_invite(
        &mut self,
        request: &crate::network::direct_message::DirectMessageRequest,
    ) -> Result<(), String> {
        let payload = request
            .text_content
            .as_deref()
            .ok_or_else(|| "missing group invite payload".to_string())?;
        let invite: crate::network::gossip::GroupInvitePayload =
            serde_json::from_str(payload).map_err(|e| format!("invalid group invite: {e}"))?;
        crate::chat::group::store_incoming_invite(
            &self.app_state,
            Some(&self.event_sink),
            &invite,
        )
        .map_err(|e| e.to_string())
    }

    async fn handle_group_sync_request(
        &mut self,
        peer: PeerId,
        request: &crate::network::direct_message::DirectMessageRequest,
    ) -> Result<(), String> {
        let payload = request
            .text_content
            .as_deref()
            .ok_or_else(|| "missing group sync request payload".to_string())?;
        let sync_request: crate::network::gossip::GroupSyncRequest =
            serde_json::from_str(payload).map_err(|e| format!("invalid group sync request: {e}"))?;
        let requester_peer_id = peer.to_string();
        let can_sync = crate::chat::group::can_peer_sync_group_records(
            &self.app_state,
            &sync_request.group_id,
            &requester_peer_id,
        )
        .map_err(|e| e.to_string())?;
        if !can_sync {
            return Err("peer is not an active group member".to_string());
        }
        let records = {
            let conn = self
                .app_state
                .db_conn
                .lock()
                .map_err(|e| e.to_string())?;
            crate::storage::db::get_group_records_for_sync(
                &conn,
                &sync_request.group_id,
                &sync_request.known_record_ids,
                sync_request.limit.clamp(1, 256),
            )
            .map_err(|e| e.to_string())?
        };
        let response = crate::network::gossip::GroupSyncResponse {
            version: 1,
            group_id: sync_request.group_id.clone(),
            records,
        };
        let response_payload = serde_json::to_string(&response)
            .map_err(|e| format!("encode group sync response failed: {e}"))?;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let response_req = crate::network::direct_message::DirectMessageRequest {
            id: format!("group-sync-response-{}-{now}", sync_request.group_id),
            sender_id: self.swarm.local_peer_id().to_string(),
            msg_type: crate::network::direct_message::DirectMessageKind::GroupSyncResponse,
            text_content: Some(response_payload),
            file_hash: None,
            timestamp: now,
            chunk_hash: None,
            chunk_data: None,
            chunk_list: None,
            sender_alias: None,
        };
        self.swarm
            .behaviour_mut()
            .direct_message
            .send_request(&peer, response_req);
        Ok(())
    }

    async fn handle_group_sync_response(
        &mut self,
        request: &crate::network::direct_message::DirectMessageRequest,
    ) -> Result<(), String> {
        let payload = request
            .text_content
            .as_deref()
            .ok_or_else(|| "missing group sync response payload".to_string())?;
        let response: crate::network::gossip::GroupSyncResponse = serde_json::from_str(payload)
            .map_err(|e| format!("invalid group sync response: {e}"))?;
        let mut applied = 0usize;
        for record in &response.records {
            if record.group_id() != response.group_id {
                continue;
            }
            if !record.verify() {
                let _ = crate::chat::group::apply_signed_record(
                    &self.app_state,
                    Some(&self.event_sink),
                    record,
                    false,
                );
                continue;
            }
            let applied_remote_message = matches!(
                record.body(),
                crate::network::gossip::GroupRecordBody::Message { .. }
            ) && record.author_peer_id() != self.swarm.local_peer_id().to_string();
            match crate::chat::group::apply_signed_record(
                &self.app_state,
                Some(&self.event_sink),
                record,
                true,
            ) {
                Ok(true) => {
                    applied += 1;
                    if applied_remote_message {
                        self.publish_group_delivered_receipt(record.group_id(), record.id())
                            .await;
                    }
                }
                Ok(false) => {}
                Err(err) => eprintln!("[Group] Failed to apply synced record {}: {}", record.id(), err),
            }
        }
        self.emit(CoreEvent::GroupSyncStateUpdated(
            crate::events::GroupSyncStateUpdatedEvent {
                group_id: response.group_id,
                state: "applied".to_string(),
                detail: Some(format!("{applied} new record(s)")),
            },
        ));
        Ok(())
    }

    async fn handle_incoming_user_message(
        &mut self,
        peer: PeerId,
        request: &crate::network::direct_message::DirectMessageRequest,
    ) -> Result<(), String> {
        let chat_id = self
            .resolve_chat_id_for_sender(&request.sender_id, request.sender_alias.as_deref())
            .await;
        println!(
            "[DM] Using chat_id: {} for sender {}",
            chat_id, request.sender_id
        );

        let db_msg = super::super::build_incoming_dm_db_message(request, chat_id.clone());

        let chat_kind = crate::chat_kind::parse_chat_kind(&chat_id);

        if matches!(chat_kind, crate::chat_kind::ChatKind::TemporaryDirect) {
                        let network_state = &self.network_state;
            let mut temp_state = network_state.temporary_state.lock().await;
            temp_state
                .messages
                .entry(chat_id.clone())
                .or_default()
                .push(db_msg.clone());
        } else {
            self.persist_incoming_dm_message(request, chat_id.clone(), db_msg.clone())
                .await
                .map_err(|e| {
                    format!(
                        "Failed to persist {} message (id={}, chat_id={}, peer_id={}, file_hash={:?}): {}",
                        request.msg_type.as_str(),
                        request.id,
                        chat_id,
                        request.sender_id,
                        request.file_hash,
                        e
                    )
                })?;
            println!("[DM] ✅ Message saved");
        }

        if request.msg_type.needs_file_transfer() {
            if let Some(ref file_hash) = request.file_hash {
                println!("[ChunkTransfer] 📤 Requesting metadata for {}", file_hash);

                let metadata_req = crate::network::direct_message::DirectMessageRequest {
                    id: format!("meta-req-{}", file_hash),
                    sender_id: self.swarm.local_peer_id().to_string(),
                    msg_type:
                        crate::network::direct_message::DirectMessageKind::FileMetadataRequest,
                    text_content: None,
                    file_hash: Some(file_hash.clone()),
                    timestamp: std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .as_secs() as i64,
                    chunk_hash: None,
                    chunk_data: None,
                    chunk_list: None,
                    sender_alias: None,
                };

                self.swarm
                    .behaviour_mut()
                    .direct_message
                    .send_request(&peer, metadata_req);
            }
        }

        self.emit(CoreEvent::MessageReceived(db_msg));
        Ok(())
    }

    async fn handle_invite_handshake(
        &mut self,
        request: &crate::network::direct_message::DirectMessageRequest,
    ) {
        if let Some(invitee_github) = request.text_content.clone() {
            let invitee_peer_id = request.sender_id.clone();
            println!(
                "[HANDSHAKE] 🤝 Received handshake from GitHub user: {} (PeerId: {})",
                invitee_github, invitee_peer_id
            );

            let chat_id =
                crate::chat_identity::build_github_chat_id(&invitee_github, &invitee_peer_id);
            self.cache_peer_mapping(&invitee_github, &invitee_peer_id);
            self.mark_connected_chat_id(chat_id.clone()).await;

            let state = &self.app_state;

            {
                let app_state = self.app_state.clone();
                let gh_user = invitee_github.clone();
                let peer_id_str = invitee_peer_id.clone();
                tokio::spawn(async move {
                    let mgr = app_state.config_manager.lock().await;
                    if let Ok(mut config) = mgr.load().await {
                        config
                            .user
                            .github_peer_mapping
                            .insert(gh_user.clone(), peer_id_str.clone());
                        if let Err(e) = mgr.save(&config).await {
                            eprintln!("[HANDSHAKE] Failed to save mapping: {}", e);
                        } else {
                            println!(
                                "[HANDSHAKE] ✅ Saved mapping: {} → {}",
                                gh_user, peer_id_str
                            );
                        }
                    }
                });
            }

            if let Ok(conn) = state.db_conn.lock() {
                if !crate::storage::db::is_peer(&conn, &chat_id) {
                    let _ = crate::storage::db::add_peer(
                        &conn,
                        &chat_id,
                        Some(&invitee_github),
                        None,
                        "github",
                    );
                }
                if !crate::storage::db::chat_exists(&conn, &chat_id) {
                    let _ =
                        crate::storage::db::create_chat(&conn, &chat_id, &invitee_github, false);
                }
                println!("[HANDSHAKE] ✅ Created chat: {}", chat_id);
            }

            self.emit(CoreEvent::NewGithubChat(
                crate::events::NewGithubChatEvent {
                    chat_id: chat_id.clone(),
                    github_username: invitee_github,
                    peer_id: invitee_peer_id,
                },
            ));

            let peer_info = crate::events::LocalPeerEvent {
                peer_id: chat_id.clone(),
                addresses: vec![],
            };
            self.emit(CoreEvent::LocalPeerDiscovered(peer_info));
            println!(
                "[HANDSHAKE] ✅ Emitted local-peer-discovered for {}",
                chat_id
            );
        }
    }

    async fn handle_temp_handshake(
        &mut self,
        peer: PeerId,
        request: &crate::network::direct_message::DirectMessageRequest,
    ) {
        let Some(handshake_text) = request.text_content.clone() else {
            return;
        };
        let (chat_id, announced_members) = match serde_json::from_str::<
            crate::network::gossip::TemporaryHandshakePayload,
        >(&handshake_text) {
            Ok(payload) => (payload.chat_id, payload.members),
            // Older peers send a bare chat id; treat it as a roster-less join.
            Err(_) => (handshake_text, Vec::new()),
        };

        if !crate::chat_kind::is_temporary_chat_id(&chat_id) {
            return;
        }

        self.cache_temporary_mapping(&chat_id, &peer.to_string());

        let peer_id_str = peer.to_string();
        let is_group = crate::chat_kind::is_temp_group_chat_id(&chat_id);
        let mut roster_changed = false;
        let mut respond_to_sender = false;
        {
            let network_state = &self.network_state;
            let mut temp_state = network_state.temporary_state.lock().await;
            if let Some(session) = temp_state.chats.get_mut(&chat_id) {
                if is_group {
                    let mut sender_known: std::collections::HashSet<String> =
                        announced_members.iter().cloned().collect();
                    sender_known.insert(peer_id_str.clone());
                    for member in announced_members {
                        if session.add_member(&member) {
                            roster_changed = true;
                        }
                    }
                    if session.add_member(&peer_id_str) {
                        roster_changed = true;
                    }
                    // Tell the sender about members they haven't announced yet
                    // (e.g. peers that joined while they were away), but only
                    // when that is actually new information so handshakes do
                    // not ping-pong forever.
                    respond_to_sender = session
                        .members
                        .iter()
                        .any(|member| !sender_known.contains(member));
                }
                if !is_group {
                    session.peer_id = Some(peer_id_str.clone());
                } else if session.peer_id.is_none() {
                    session.peer_id = Some(peer_id_str.clone());
                }
            } else {
                let kind = if is_group {
                    crate::app_state::TemporaryChatKind::Group
                } else {
                    crate::app_state::TemporaryChatKind::Dm
                };
                let name = if is_group {
                    crate::chat_kind::default_temp_group_name(&chat_id)
                } else {
                    crate::chat_kind::default_temp_direct_name(&chat_id)
                };
                let mut session = crate::app_state::TemporaryChatSession {
                    chat_id: chat_id.clone(),
                    name,
                    kind,
                    expires_at: std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_secs() + 120)
                        .unwrap_or(120),
                    peer_id: Some(peer_id_str.clone()),
                    members: Vec::new(),
                    archived: false,
                };
                if is_group {
                    // Seed the roster with the local peer plus everything the
                    // sender announced, then the sender itself.
                    session.add_member(&self.swarm.local_peer_id().to_string());
                    for member in announced_members {
                        session.add_member(&member);
                    }
                    session.add_member(&peer_id_str);
                    roster_changed = true;
                    respond_to_sender = true;
                }
                temp_state.chats.insert(chat_id.clone(), session);
            }
        }

        // Respond with our roster so the sender learns about the other
        // members of the group, and push roster growth to every other
        // connected member so join/leave/disconnect membership stays
        // consistent group-wide. Both are gated on new information so the
        // handshake exchange converges instead of ping-ponging.
        if is_group && respond_to_sender {
            self.send_temp_handshake_to(&peer, &chat_id).await;
        }
        if is_group && roster_changed {
            self.broadcast_temp_group_roster(&chat_id, Some(&peer)).await;
        }

        self.emit(CoreEvent::TemporaryChatConnected(
            crate::events::TemporaryChatConnectedEvent {
                chat_id,
                peer_id: peer_id_str,
            },
        ));
    }

    /// Send a `TempHandshake` to `peer` carrying the current member roster of
    /// `chat_id` so both sides converge on the same member set.
    pub(crate) async fn send_temp_handshake_to(&mut self, peer: &PeerId, chat_id: &str) {
        use crate::network::direct_message::{DirectMessageKind, DirectMessageRequest};
        let members = {
            let network_state = &self.network_state;
            let temp_state = network_state.temporary_state.lock().await;
            temp_state
                .chats
                .get(chat_id)
                .map(|session| session.members.clone())
                .unwrap_or_default()
        };
        let payload = crate::network::gossip::TemporaryHandshakePayload {
            chat_id: chat_id.to_string(),
            members,
        };
        let text_content =
            serde_json::to_string(&payload).unwrap_or_else(|_| chat_id.to_string());
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let handshake = DirectMessageRequest {
            id: format!("temp-handshake-{}", now),
            sender_id: self.swarm.local_peer_id().to_string(),
            msg_type: DirectMessageKind::TempHandshake,
            text_content: Some(text_content),
            file_hash: None,
            timestamp: now,
            chunk_hash: None,
            chunk_data: None,
            chunk_list: None,
            sender_alias: None,
        };
        self.swarm
            .behaviour_mut()
            .direct_message
            .send_request(peer, handshake);
    }

    /// Push the current roster to every connected member of a temporary
    /// group, optionally skipping one peer (the sender of the triggering
    /// handshake).
    pub(crate) async fn broadcast_temp_group_roster(
        &mut self,
        chat_id: &str,
        except: Option<&PeerId>,
    ) {
        let peers = self.connected_temp_members(chat_id);
        for peer in peers {
            if Some(&peer) != except {
                self.send_temp_handshake_to(&peer, chat_id).await;
            }
        }
    }

    async fn handle_read_receipt(
        &mut self,
        request: &crate::network::direct_message::DirectMessageRequest,
    ) -> Result<Vec<String>, String> {
        if let Some(ref msg_ids_str) = request.text_content {
            let msg_ids: Vec<String> = msg_ids_str
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();

            if let Err(e) = self.persist_read_statuses(msg_ids.clone()).await {
                let mut updated_runtime = false;
                {
                    let network_state = &self.network_state;
                    let mut temp_state = network_state.temporary_state.lock().await;
                    for msg_id in &msg_ids {
                        for msgs in temp_state.messages.values_mut() {
                            if let Some(found) = msgs.iter_mut().find(|m| m.id == *msg_id) {
                                found.status = "read".to_string();
                                updated_runtime = true;
                            }
                        }
                    }
                }
                if !updated_runtime {
                    return Err(e);
                }
            }

            for msg_id in &msg_ids {
                println!("[READ_RECEIPT] 📥 Marked {} as read", msg_id);
                self.emit(CoreEvent::MessageStatusUpdated(
                    crate::events::MessageStatusUpdatedEvent {
                        msg_id: msg_id.clone(),
                        status: "read".to_string(),
                    },
                ));
            }

            Ok(msg_ids)
        } else {
            Ok(Vec::new())
        }
    }
}
