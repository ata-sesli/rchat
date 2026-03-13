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
                                let _ = self.app_handle.emit(
                                    "message-status-updated",
                                    serde_json::json!({
                                        "msg_id": response.msg_id,
                                        "status": "delivered",
                                    }),
                                );
                            }
                            Err(err) => {
                                let mut updated_runtime = false;
                                {
                                    use tauri::Manager;
                                    let network_state =
                                        self.app_handle.state::<crate::NetworkState>();
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
                                    let _ = self.app_handle.emit(
                                        "message-status-updated",
                                        serde_json::json!({
                                            "msg_id": response.msg_id,
                                            "status": "delivered",
                                        }),
                                    );
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
            use tauri::Manager;
            let network_state = self.app_handle.state::<crate::NetworkState>();
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

        let _ = self.app_handle.emit("message-received", db_msg);
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

            let chat_id = crate::chat_identity::build_github_chat_id(&invitee_github, &invitee_peer_id);
            self.cache_peer_mapping(&invitee_github, &invitee_peer_id);
            self.mark_connected_chat_id(chat_id.clone()).await;

            use tauri::Manager;
            let state = self.app_handle.state::<crate::AppState>();

            {
                let app_handle = self.app_handle.clone();
                let gh_user = invitee_github.clone();
                let peer_id_str = invitee_peer_id.clone();
                tauri::async_runtime::spawn(async move {
                    let state = app_handle.state::<crate::AppState>();
                    let mgr = state.config_manager.lock().await;
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

            let _ = self.app_handle.emit(
                "new-github-chat",
                serde_json::json!({
                    "chat_id": chat_id,
                    "github_username": invitee_github,
                    "peer_id": invitee_peer_id,
                }),
            );

            let peer_info = LocalPeer {
                peer_id: chat_id.clone(),
                addresses: vec![],
            };
            let _ = self.app_handle.emit("local-peer-discovered", peer_info);
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
        let Some(chat_id) = request.text_content.clone() else {
            return;
        };

        if !crate::chat_kind::is_temporary_chat_id(&chat_id) {
            return;
        }

        self.cache_temporary_mapping(&chat_id, &peer.to_string());

        use tauri::Manager;
        let network_state = self.app_handle.state::<crate::NetworkState>();
        let mut temp_state = network_state.temporary_state.lock().await;
        if let Some(session) = temp_state.chats.get_mut(&chat_id) {
            session.peer_id = Some(peer.to_string());
        } else {
            let kind = if crate::chat_kind::is_temp_group_chat_id(&chat_id) {
                crate::app_state::TemporaryChatKind::Group
            } else {
                crate::app_state::TemporaryChatKind::Dm
            };
            let name = if matches!(kind, crate::app_state::TemporaryChatKind::Group) {
                crate::chat_kind::default_temp_group_name(&chat_id)
            } else {
                crate::chat_kind::default_temp_direct_name(&chat_id)
            };
            temp_state.chats.insert(
                chat_id.clone(),
                crate::app_state::TemporaryChatSession {
                    chat_id: chat_id.clone(),
                    name,
                    kind,
                    expires_at: std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_secs() + 120)
                        .unwrap_or(120),
                    peer_id: Some(peer.to_string()),
                    archived: false,
                },
            );
        }

        let _ = self.app_handle.emit(
            "temporary-chat-connected",
            serde_json::json!({
                "chat_id": chat_id,
                "peer_id": peer.to_string(),
            }),
        );
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
                    use tauri::Manager;
                    let network_state = self.app_handle.state::<crate::NetworkState>();
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
                let _ = self.app_handle.emit(
                    "message-status-updated",
                    serde_json::json!({
                        "msg_id": msg_id,
                        "status": "read",
                    }),
                );
            }

            Ok(msg_ids)
        } else {
            Ok(Vec::new())
        }
    }
}
