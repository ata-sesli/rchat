use super::*;

impl NetworkManager {
    fn record_chat_reconnection(&self, chat_id: &str, connected_at: i64) {
        use tauri::Manager;
        let state = self.app_handle.state::<crate::AppState>();
        let Ok(conn) = state.db_conn.lock() else {
            return;
        };
        if let Err(e) =
            crate::storage::db::record_chat_connection_established(&conn, chat_id, connected_at)
        {
            eprintln!(
                "[Connection] Failed to update reconnect counters for {}: {}",
                chat_id, e
            );
        }
    }

    pub(super) async fn handle_connection_established(
        &mut self,
        peer_id: PeerId,
        endpoint: libp2p::core::ConnectedPoint,
    ) {
        println!("[Swarm] Connected to {}", peer_id);

        let remote_addr = endpoint.get_remote_address().clone();
        self.note_peer_transport_connected(peer_id, &remote_addr);
        self.local_peers
            .entry(peer_id)
            .or_insert_with(Vec::new)
            .push(remote_addr.clone());

        let mut to_remove = Vec::new();
        for (name, (addr, _)) in self.active_punch_targets.iter() {
            let target_ip = addr.to_string().split('/').nth(2).unwrap_or("").to_string();
            let connected_ip = remote_addr
                .to_string()
                .split('/')
                .nth(2)
                .unwrap_or("")
                .to_string();

            if !target_ip.is_empty() && target_ip == connected_ip {
                to_remove.push(name.clone());
            }
        }

        for name in to_remove {
            self.remove_punch_target(&name);
        }

        let remote_addr_str = remote_addr.to_string();
        let connected_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let peer_id_str = peer_id.to_string();
        self.mark_connected_chat_id(peer_id_str.clone()).await;
        let transitioned = self
            .note_chat_connection_established(&peer_id_str, &remote_addr_str, connected_at)
            .await;
        if transitioned {
            self.record_chat_reconnection(&peer_id_str, connected_at);
        }
        if let Some(username) = self.github_by_peer_id.get(&peer_id_str).cloned() {
            let chat_id = format!("gh:{}", username);
            self.mark_connected_chat_id(chat_id.clone()).await;
            let transitioned = self
                .note_chat_connection_established(&chat_id, &remote_addr_str, connected_at)
                .await;
            if transitioned {
                self.record_chat_reconnection(&chat_id, connected_at);
            }
        }

        if let Some(chat_id) = self.temp_chat_by_peer_id.get(&peer_id_str).cloned() {
            use crate::network::direct_message::{DirectMessageKind, DirectMessageRequest};
            let handshake = DirectMessageRequest {
                id: format!(
                    "temp-handshake-{}",
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .as_secs()
                ),
                sender_id: self.swarm.local_peer_id().to_string(),
                msg_type: DirectMessageKind::TempHandshake,
                text_content: Some(chat_id.clone()),
                file_hash: None,
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
                .send_request(&peer_id, handshake);

            let _ = self.app_handle.emit(
                "temporary-chat-connected",
                serde_json::json!({
                    "chat_id": chat_id,
                    "peer_id": peer_id_str,
                }),
            );
        }

        let mut matched_data = None;
        for (pending_addr, (inviter_user, my_user)) in self.pending_github_mappings.iter() {
            if remote_addr_str.starts_with("/ip4/") && pending_addr.starts_with("/ip4/") {
                let pending_ip = pending_addr.split('/').nth(2);
                let remote_ip = remote_addr_str.split('/').nth(2);
                if pending_ip == remote_ip && pending_ip.is_some() {
                    matched_data =
                        Some((pending_addr.clone(), inviter_user.clone(), my_user.clone()));
                    break;
                }
            }
        }

        if let Some((addr_key, inviter_github_user, my_username)) = matched_data {
            self.pending_github_mappings.remove(&addr_key);
            println!(
                "[DIAL] ✅ GitHub user {} connected with PeerId {}",
                inviter_github_user, peer_id_str
            );
            self.cache_peer_mapping(&inviter_github_user, &peer_id_str);
            self.mark_connected_chat_id(format!("gh:{}", inviter_github_user))
                .await;
            let chat_id = format!("gh:{}", inviter_github_user);
            let transitioned = self
                .note_chat_connection_established(&chat_id, &remote_addr_str, connected_at)
                .await;
            if transitioned {
                self.record_chat_reconnection(&chat_id, connected_at);
            }

            let app_handle = self.app_handle.clone();
            let gh_user = inviter_github_user.clone();
            let peer_id_for_mapping = peer_id_str.clone();
            tauri::async_runtime::spawn(async move {
                let state = app_handle.state::<crate::AppState>();
                let mgr = state.config_manager.lock().await;
                if let Ok(mut config) = mgr.load().await {
                    config
                        .user
                        .github_peer_mapping
                        .insert(gh_user.clone(), peer_id_for_mapping.clone());
                    if let Err(e) = mgr.save(&config).await {
                        eprintln!("[DIAL] Failed to save GitHub peer mapping: {}", e);
                    } else {
                        println!(
                            "[DIAL] ✅ Saved mapping: {} → {}",
                            gh_user, peer_id_for_mapping
                        );
                    }
                }
            });

            println!(
                "[HANDSHAKE] 🤝 Sending invite_handshake to {} with my username: {}",
                peer_id, my_username
            );

            use crate::network::direct_message::{DirectMessageKind, DirectMessageRequest};
            let handshake = DirectMessageRequest {
                id: format!(
                    "handshake-{}",
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .as_secs()
                ),
                sender_id: self.swarm.local_peer_id().to_string(),
                msg_type: DirectMessageKind::InviteHandshake,
                text_content: Some(my_username),
                file_hash: None,
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
                .send_request(&peer_id, handshake);
            println!("[HANDSHAKE] ✅ Handshake sent to {}", peer_id);

            let chat_id = format!("gh:{}", inviter_github_user);
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

    pub(super) async fn handle_connection_closed(
        &mut self,
        peer_id: PeerId,
        num_established: u32,
        endpoint: libp2p::core::ConnectedPoint,
    ) {
        println!("[Swarm] Disconnected from {}", peer_id);
        let remote_addr = endpoint.get_remote_address().clone();
        let quic_path_lost = self.note_peer_transport_disconnected(peer_id, &remote_addr);
        if quic_path_lost {
            let end_video_call = self
                .active_call
                .as_ref()
                .map(|call| {
                    call.kind == crate::app_state::CallKind::Video && call.remote_peer_id == peer_id
                })
                .unwrap_or(false);
            if end_video_call {
                self.transition_to_idle(Some("quic_path_lost".to_string()))
                    .await;
            }
        }

        if num_established == 0 {
            self.handle_peer_disconnect_for_voice_call(&peer_id).await;
            if self.local_peers.remove(&peer_id).is_some() {
                println!("[Swarm] Peer {} fully disconnected, notifying UI", peer_id);

                let peer_id_str = peer_id.to_string();
                self.unmark_connected_chat_id(&peer_id_str).await;
                self.note_chat_connection_closed(&peer_id_str).await;
                if let Some(chat_id) = self.remove_temporary_by_peer_id(&peer_id_str) {
                    self.unmark_connected_chat_id(&chat_id).await;
                    self.note_chat_connection_closed(&chat_id).await;
                    let _ = self.app_handle.emit(
                        "temporary-chat-ended",
                        serde_json::json!({
                            "chat_id": chat_id,
                            "peer_id": peer_id_str,
                        }),
                    );
                    return;
                }

                if let Some(username) = self.github_by_peer_id.get(&peer_id_str).cloned() {
                    let chat_id = format!("gh:{}", username);
                    self.unmark_connected_chat_id(&chat_id).await;
                    self.note_chat_connection_closed(&chat_id).await;
                    let _ = self
                        .app_handle
                        .emit("local-peer-expired", format!("gh:{}", username));
                    return;
                }

                self.refresh_peer_mapping_cache().await;
                if let Some(username) = self.github_by_peer_id.get(&peer_id_str).cloned() {
                    let chat_id = format!("gh:{}", username);
                    self.unmark_connected_chat_id(&chat_id).await;
                    self.note_chat_connection_closed(&chat_id).await;
                    let _ = self
                        .app_handle
                        .emit("local-peer-expired", format!("gh:{}", username));
                } else {
                    let _ = self.app_handle.emit("local-peer-expired", peer_id_str);
                }
            }
        }
    }

    fn try_start_mdns_on_port(&mut self, port: u16) {
        if self.mdns_started || port == 0 {
            return;
        }

        println!(
            "[NetworkManager] Found QUIC listen port: {}, starting mDNS...",
            port
        );
        let peer_id = *self.swarm.local_peer_id();

        let user_alias = {
            use tauri::Manager;
            let state = self.app_handle.state::<crate::AppState>();
            state
                .config_manager
                .try_lock()
                .ok()
                .and_then(|mgr| mgr.load_sync().ok())
                .and_then(|c| c.user.profile.alias.clone())
        };

        if let Err(e) = crate::network::mdns::start_mdns_service(
            peer_id,
            port,
            self.mdns_tx.clone(),
            user_alias,
        )
        .map(|handle| {
            self.mdns_handle = Some(handle);
        }) {
            eprintln!("[NetworkManager] Failed to start mDNS: {}", e);
        } else {
            self.mdns_started = true;
            println!("[NetworkManager] mDNS started (advertising + browsing)");
        }
    }

    pub(crate) fn reconcile_mdns_runtime(&mut self) {
        let mdns_enabled = self.is_mdns_enabled();

        if !mdns_enabled {
            if self.mdns_started {
                if let Some(mut handle) = self.mdns_handle.take() {
                    handle.stop();
                }
                self.mdns_started = false;

                let expired_peers: Vec<String> =
                    self.local_peers.keys().map(|p| p.to_string()).collect();
                self.local_peers.clear();
                for peer_id in expired_peers {
                    let _ = self.app_handle.emit("local-peer-expired", peer_id);
                }
            }
            return;
        }

        if self.mdns_started {
            return;
        }

        let listen_port = self
            .swarm
            .listeners()
            .find(|addr| addr.to_string().contains("/udp/") && addr.to_string().contains("quic"))
            .and_then(crate::network::get_port_from_multiaddr);

        if let Some(port) = listen_port {
            self.try_start_mdns_on_port(port);
        }
    }

    pub(super) fn handle_new_listen_addr(&mut self, address: Multiaddr) {
        println!("[Swarm] Listening on: {}", address);

        let addr_str = address.to_string();
        if !addr_str.contains("127.0.0.1") && !addr_str.contains("::1") {
            let app_handle = self.app_handle.clone();
            let addr_clone = addr_str.clone();
            tauri::async_runtime::spawn(async move {
                let state = app_handle.state::<crate::NetworkState>();
                let mut addrs = state.listening_addresses.lock().await;
                if !addrs.contains(&addr_clone) {
                    addrs.push(addr_clone);
                }
            });
        }

        if self.is_mdns_enabled()
            && address.to_string().contains("/udp/")
            && address.to_string().contains("quic")
        {
            if let Some(port) = crate::network::get_port_from_multiaddr(&address) {
                self.try_start_mdns_on_port(port);
            }
        }
    }
}
