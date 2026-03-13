use super::*;

impl NetworkManager {
    pub(super) fn handle_start_punch_command(
        &mut self,
        multiaddr: String,
        target_username: String,
        my_username: String,
    ) {
        println!(
            "[PUNCH] 🥊 Starting punch to {} at {} (me: {})",
            target_username, multiaddr, my_username
        );

        if let Ok(addr) = multiaddr.parse::<Multiaddr>() {
            self.pending_github_mappings
                .insert(multiaddr, (target_username.clone(), my_username));
            self.add_punch_target(&target_username, addr);
        }
    }

    pub(super) fn register_temporary_session(
        &mut self,
        chat_id: &str,
        peer_id: &str,
        multiaddr: &str,
        is_group: bool,
    ) {
        self.cache_temporary_mapping(chat_id, peer_id);

        if is_group {
            self.subscribe_group(chat_id);
        }

        if let Ok(addr) = multiaddr.parse::<Multiaddr>() {
            self.add_punch_target(chat_id, addr);
        } else {
            eprintln!(
                "[Temp] Invalid multiaddr for temporary session {}: {}",
                chat_id, multiaddr
            );
        }
    }

    pub(super) fn end_temporary_session(&mut self, chat_id: &str) {
        self.remove_temporary_by_chat_id(chat_id);
        self.remove_punch_target(chat_id);
        self.unsubscribe_group(chat_id);
    }

    /// Handle a connection request from UI (user pressed Connect on a peer)
    pub(crate) async fn handle_connection_request(&mut self, peer_id_str: &str) {
        println!("[Handshake] User requested connection to: {}", peer_id_str);

        let peer_id = if let Some(p) = self.resolve_peer_id(peer_id_str, "Handshake").await {
            p
        } else {
            return;
        };

        let already_requested_us = self.incoming_requests.contains(&peer_id);
        if already_requested_us {
            println!("[Handshake] 🤝 Mutual handshake complete with {}!", peer_id);
            self.complete_handshake(peer_id);
        } else {
            self.pending_requests.insert(peer_id);
            println!("[Handshake] ⏳ Waiting for {} to accept...", peer_id);
            let _ = self.app_handle.emit("connection-waiting", peer_id_str);
        }

        let envelope = crate::network::gossip::ControlEnvelope::ConnectionRequest {
            from_peer_id: self.swarm.local_peer_id().to_string(),
            to_peer_id: peer_id.to_string(),
        };
        if let Ok(payload) = serde_json::to_vec(&envelope) {
            let topic = crate::network::gossip::control_topic();
            let _ = self.swarm.behaviour_mut().gossipsub.publish(topic, payload);
        }
    }

    pub(super) async fn handle_drop_connection(&mut self, peer_id_str: &str) {
        let Some(peer_id) = self.resolve_peer_id(peer_id_str, "Disconnect").await else {
            return;
        };

        match self.swarm.disconnect_peer_id(peer_id) {
            Ok(()) => println!("[Connection] 🔌 Disconnect requested for {}", peer_id),
            Err(e) => eprintln!(
                "[Connection] ❌ Failed to disconnect {}: {:?}",
                peer_id, e
            ),
        }
    }

    /// Handle incoming connection request from another peer
    pub(crate) fn handle_incoming_connection_request(&mut self, from_peer_id: PeerId) {
        println!(
            "[Handshake] Received connection request from: {}",
            from_peer_id
        );

        if self.pending_requests.contains(&from_peer_id) {
            println!(
                "[Handshake] 🤝 Mutual handshake complete with {}!",
                from_peer_id
            );
            self.complete_handshake(from_peer_id);
            return;
        }

        self.incoming_requests.insert(from_peer_id);

        let _ = self
            .app_handle
            .emit("connection-request-received", from_peer_id.to_string());
    }

    /// Complete the handshake - both sides have agreed
    fn complete_handshake(&mut self, peer_id: PeerId) {
        self.pending_requests.remove(&peer_id);
        self.incoming_requests.remove(&peer_id);
        self.remember_trusted_peer_id(peer_id);

        use tauri::Manager;
        let state = self.app_handle.state::<crate::AppState>();
        if let Ok(conn) = state.db_conn.lock() {
            if let Err(e) =
                crate::storage::db::add_peer(&conn, &peer_id.to_string(), None, None, "local")
            {
                eprintln!("[Handshake] Failed to save peer: {}", e);
            } else {
                println!("[Handshake] ✅ {} saved to peers table!", peer_id);
            }
        }

        let _ = self.app_handle.emit("peer-connected", peer_id.to_string());
    }
}
