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

    pub(super) async fn end_temporary_session(
        &mut self,
        chat_id: &str,
        farewell_winners: Option<Vec<crate::app_state::TemporaryMembershipOp>>,
        ack: Option<tokio::sync::oneshot::Sender<Vec<crate::storage::db::Message>>>,
    ) {
        // Group-scoped exit: broadcast the farewell roster (including any
        // remove tombstones issued by leave or archive) so remaining members
        // drop us, without ever closing the peer's shared libp2p connections.
        // The archive path passes the winners explicitly because it removes
        // the session before queueing this command; the leave path leaves the
        // session in place and reads the current roster from it.
        //
        // The farewell broadcast is the leave boundary and always happens
        // first: messages received while the broadcast is in flight are still
        // appended to the session (they were sent before we announced our
        // leave) and are captured by the drain below, so no pre-farewell
        // traffic is silently dropped.
        let network_state = self.network_state.clone();
        let is_group = if farewell_winners.is_some() {
            true
        } else {
            let temp_state = network_state.temporary_state.lock().await;
            temp_state
                .chats
                .get(chat_id)
                .map(|session| {
                    matches!(
                        session.kind,
                        crate::app_state::TemporaryChatKind::Group
                    )
                })
                .unwrap_or(false)
        };
        if is_group {
            match &farewell_winners {
                Some(winners) => {
                    self.broadcast_temp_group_winners(chat_id, winners.clone(), None)
                        .await;
                }
                None => self.broadcast_temp_group_roster(chat_id, None).await,
            }
        }
        // Tear the session down under one lock. For the archive path the final
        // message set — everything received up to and during the farewell
        // broadcast — is drained and acknowledged so the caller can persist
        // the entire archive in one transaction; for the plain leave path the
        // runtime state is simply removed.
        {
            let mut temp_state = network_state.temporary_state.lock().await;
            if let Some(ack) = ack {
                let messages = temp_state.messages.remove(chat_id).unwrap_or_default();
                temp_state.chats.remove(chat_id);
                let _ = ack.send(messages);
            } else {
                temp_state.chats.remove(chat_id);
                temp_state.messages.remove(chat_id);
            }
        }
        self.remove_temporary_by_chat_id(chat_id);
        self.remove_punch_target(chat_id);
        self.unsubscribe_group(chat_id);
        self.emit(CoreEvent::TemporaryChatEnded(
            crate::events::TemporaryChatEndedEvent {
                chat_id: chat_id.to_string(),
                peer_id: self.swarm.local_peer_id().to_string(),
            },
        ));
    }

    /// Re-establish a temporary session whose archive failed after the
    /// farewell was broadcast.
    ///
    /// Re-inserts the session (reservation cleared) and its messages,
    /// re-subscribes, and re-announces the roster with a fresh signed add
    /// tombstone whose counter exceeds the farewell remove's, so every member
    /// that processed our leave re-admits the local peer. A failed archive
    /// therefore preserves the live conversation and its membership instead of
    /// leaving the user removed from a group that survives.
    ///
    /// The handler acknowledges only once reinsertion, re-subscription and the
    /// signed rejoin have all succeeded. If the keypair is unavailable or the
    /// rejoin add cannot be signed, the recovery data is still re-inserted (so
    /// the user does not lose the conversation) but the acknowledgement
    /// carries the error, and the roster is not re-announced — the caller must
    /// not believe the peer rejoined when remote members still treat it as
    /// removed.
    pub(super) async fn restore_temporary_session(
        &mut self,
        chat_id: &str,
        session: crate::app_state::TemporaryChatSession,
        messages: Vec<crate::storage::db::Message>,
        min_add_counter: u64,
        ack: Option<tokio::sync::oneshot::Sender<Result<(), String>>>,
    ) {
        let mut session = session;
        session.archived = false;
        if session.next_member_op_counter < min_add_counter {
            session.next_member_op_counter = min_add_counter;
        }
        let mut rejoin_error: Option<String> = None;
        {
            let mut temp_state = self.network_state.temporary_state.lock().await;
            match crate::chat::group::load_or_create_local_keypair(&self.app_state).await {
                Ok(signer) => {
                    let local = self.swarm.local_peer_id().to_string();
                    if let Err(error) = session.issue_membership_op(
                        &local,
                        crate::app_state::TemporaryMembershipOpKind::Add,
                        &local,
                        &signer,
                    ) {
                        rejoin_error = Some(format!(
                            "[TempGroup] failed to rejoin {chat_id} after failed archive: {error}"
                        ));
                    }
                }
                Err(error) => {
                    rejoin_error = Some(format!(
                        "[TempGroup] no keypair available to rejoin {chat_id} after failed archive: {error}"
                    ));
                }
            }
            temp_state.chats.insert(chat_id.to_string(), session.clone());
            temp_state.messages.insert(chat_id.to_string(), messages);
        }
        let rejoined = rejoin_error.is_none();
        if rejoined && matches!(session.kind, crate::app_state::TemporaryChatKind::Group) {
            self.subscribe_group(chat_id);
            self.broadcast_temp_group_roster(chat_id, None).await;
        }
        if let Some(ack) = ack {
            if let Some(error) = rejoin_error {
                let _ = ack.send(Err(error));
            } else {
                let _ = ack.send(Ok(()));
            }
        }
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
            self.emit(CoreEvent::ConnectionWaiting(peer_id_str.to_string()));
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
            Err(e) => eprintln!("[Connection] ❌ Failed to disconnect {}: {:?}", peer_id, e),
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

        self.emit(CoreEvent::ConnectionRequestReceived(
            from_peer_id.to_string(),
        ));
    }

    /// Complete the handshake - both sides have agreed
    fn complete_handshake(&mut self, peer_id: PeerId) {
        self.pending_requests.remove(&peer_id);
        self.incoming_requests.remove(&peer_id);
        self.remember_trusted_peer_id(peer_id);

        let state = &self.app_state;
        if let Ok(conn) = state.db_conn.lock() {
            if let Err(e) =
                crate::storage::db::add_peer(&conn, &peer_id.to_string(), None, None, "local")
            {
                eprintln!("[Handshake] Failed to save peer: {}", e);
            } else {
                println!("[Handshake] ✅ {} saved to peers table!", peer_id);
            }
        }

        self.emit(CoreEvent::PeerConnected(peer_id.to_string()));
    }
}
