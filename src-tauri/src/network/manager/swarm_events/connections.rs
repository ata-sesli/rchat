use super::*;

impl NetworkManager {
    pub(super) async fn handle_connection_established(
        &mut self,
        peer_id: PeerId,
        endpoint: libp2p::core::ConnectedPoint,
    ) {
        println!("[Swarm] Connected to {}", peer_id);

        let remote_addr = endpoint.get_remote_address().clone();
        self.local_peers
            .entry(peer_id)
            .or_insert_with(Vec::new)
            .push(remote_addr.clone());

        let mut to_remove = Vec::new();
        for (name, (addr, _)) in self.active_punch_targets.iter() {
            let target_ip = addr
                .to_string()
                .split('/')
                .nth(2)
                .unwrap_or("")
                .to_string();
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
        let peer_id_str = peer_id.to_string();

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
                    matched_data = Some((
                        pending_addr.clone(),
                        inviter_user.clone(),
                        my_user.clone(),
                    ));
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
                        println!("[DIAL] ✅ Saved mapping: {} → {}", gh_user, peer_id_for_mapping);
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
            println!("[HANDSHAKE] ✅ Emitted local-peer-discovered for {}", chat_id);
        }
    }

    pub(super) async fn handle_connection_closed(&mut self, peer_id: PeerId, num_established: u32) {
        println!("[Swarm] Disconnected from {}", peer_id);

        if num_established == 0 {
            if self.local_peers.remove(&peer_id).is_some() {
                println!("[Swarm] Peer {} fully disconnected, notifying UI", peer_id);

                let peer_id_str = peer_id.to_string();
                if let Some(chat_id) = self.remove_temporary_by_peer_id(&peer_id_str) {
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
                    let _ = self
                        .app_handle
                        .emit("local-peer-expired", format!("gh:{}", username));
                    return;
                }

                self.refresh_peer_mapping_cache().await;
                if let Some(username) = self.github_by_peer_id.get(&peer_id_str).cloned() {
                    let _ = self
                        .app_handle
                        .emit("local-peer-expired", format!("gh:{}", username));
                } else {
                    let _ = self.app_handle.emit("local-peer-expired", peer_id_str);
                }
            }
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

        if !self.mdns_started
            && address.to_string().contains("/udp/")
            && address.to_string().contains("quic")
        {
            if let Some(port) = crate::network::get_port_from_multiaddr(&address) {
                if port != 0 {
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
            }
        }
    }
}
