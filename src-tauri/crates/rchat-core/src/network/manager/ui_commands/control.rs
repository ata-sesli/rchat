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

    /// Phase one of two-phase temporary-chat finalization: freeze a session
    /// for archiving.
    ///
    /// The farewell broadcast (groups only) is the leave boundary and always
    /// happens first: messages received while it is in flight are still
    /// appended to the session and captured by the drain below, so no
    /// pre-farewell traffic is silently dropped. The session then stays in the
    /// temporary state with its reservation (`archived`) set — sends and
    /// incoming messages keep being rejected — while the final message set is
    /// drained into a `pending_finalization` entry that also retains the
    /// routing maps, subscription and punch target for later commit or abort.
    /// The drained messages are acknowledged so the caller can persist the
    /// entire archive in a single transaction.
    ///
    /// A watchdog watches the caller's `alive` sender: if it is dropped
    /// without a Commit/Abort resolving the freeze (the caller's task was
    /// cancelled), the freeze is aborted so the conversation is recovered
    /// instead of left reserved forever.
    pub(super) async fn freeze_temporary_archive(
        &mut self,
        chat_id: &str,
        kind: crate::app_state::TemporaryChatKind,
        farewell_winners: Vec<crate::app_state::TemporaryMembershipOp>,
        min_add_counter: u64,
        alive: tokio::sync::watch::Sender<crate::app_state::FreezeResolution>,
        ack: Option<
            tokio::sync::oneshot::Sender<Result<Vec<crate::storage::db::Message>, String>>,
        >,
    ) {
        let network_state = self.network_state.clone();
        if matches!(kind, crate::app_state::TemporaryChatKind::Group) {
            // The farewell is the leave boundary: the archiver's remove
            // tombstone needs no admission evidence (recipients are current
            // members), so the snapshot is broadcast without evidence.
            self.broadcast_temp_group_winners(chat_id, farewell_winners, Vec::new(), None)
                .await;
        }
        let drained = {
            let mut temp_state = network_state.temporary_state.lock().await;
            let Some(session) = temp_state.chats.get(chat_id).cloned() else {
                let message = format!("temporary chat not found for freeze: {chat_id}");
                if let Some(ack) = ack {
                    let _ = ack.send(Err(message));
                }
                return;
            };
            let messages = temp_state.messages.remove(chat_id).unwrap_or_default();
            self.pending_epoch_counter = self.pending_epoch_counter.saturating_add(1);
            let pending = crate::app_state::PendingTemporaryFinalization {
                epoch: self.pending_epoch_counter,
                chat_id: chat_id.to_string(),
                kind: kind.clone(),
                session: session.clone(),
                messages: messages.clone(),
                routing_peers: self
                    .temp_peer_by_chat_id
                    .get(chat_id)
                    .map(|peers| peers.iter().cloned().collect())
                    .unwrap_or_default(),
                was_subscribed: self.subscribed_group_ids.contains(chat_id),
                punch_target: self
                    .active_punch_targets
                    .get(chat_id)
                    .map(|(addr, _)| addr.to_string()),
                min_add_counter,
            };
            // The session stays in the temporary state (reservation set) so
            // sends and incoming traffic keep being rejected while the freeze
            // is unresolved; only the message buffer is drained into the
            // pending record.
            self.pending_finalization.insert(chat_id.to_string(), pending);
            messages
        };
        // Watchdog: the caller's `alive` sender carries its synchronous
        // decision. The watchdog keeps watching for the whole freeze: a
        // successful value change to `Commit` means the archive is already
        // durable, so the watchdog finalizes the teardown itself — even if the
        // caller is then cancelled or blocked before its own direct commit
        // enqueue completes (a duplicate commit is a no-op). A dropped sender
        // (cancellation or ack loss) resolves from the caller's last decision:
        // still `Pending` means the archive never became durable, so abort and
        // recover the conversation; `Commit` means the archive is already
        // persisted, so finalize the teardown instead of reviving a chat that
        // now also exists in the archive. Epoch-scoped so a stale watchdog can
        // never abort a later freeze of the same chat.
        let watchdog_epoch = self.pending_epoch_counter;
        let watchdog_net = self.network_state.clone();
        let watchdog_chat = chat_id.to_string();
        // Only the receiver crosses into the watchdog task: the handler's copy
        // of the sender is dropped immediately, so the channel closes the
        // moment the caller's `alive` sender is dropped (cancellation), which
        // is exactly the event the watchdog must detect. Holding the sender in
        // the task would keep the channel open forever and the watchdog would
        // never fire.
        let mut alive_rx = alive.subscribe();
        drop(alive);
        tokio::spawn(async move {
            loop {
                if alive_rx.changed().await.is_err() {
                    // Caller cancelled without resolving: act on the last
                    // recorded decision.
                    let resolution = *alive_rx.borrow();
                    let sender = watchdog_net.sender.lock().await;
                    match resolution {
                        crate::app_state::FreezeResolution::Pending => {
                            let _ = sender
                                .send(NetworkCommand::AbortTemporaryArchive {
                                    chat_id: watchdog_chat,
                                    epoch: Some(watchdog_epoch),
                                    ack: None,
                                })
                                .await;
                        }
                        crate::app_state::FreezeResolution::Commit => {
                            let _ = sender
                                .send(NetworkCommand::CommitTemporaryArchive {
                                    chat_id: watchdog_chat,
                                })
                                .await;
                        }
                    }
                    return;
                }
                // The caller recorded a decision. A `Commit` means the archive
                // is durable: finalize immediately instead of waiting for the
                // caller's direct commit enqueue, which may never arrive. While
                // the decision is still `Pending`, keep watching for closure or
                // the Commit.
                let resolution = *alive_rx.borrow();
                if matches!(resolution, crate::app_state::FreezeResolution::Commit) {
                    let sender = watchdog_net.sender.lock().await;
                    let _ = sender
                        .send(NetworkCommand::CommitTemporaryArchive {
                            chat_id: watchdog_chat,
                        })
                        .await;
                    return;
                }
            }
        });
        if let Some(ack) = ack {
            let _ = ack.send(Ok(drained));
        }
    }

    /// Phase two of two-phase temporary-chat finalization: commit an archived
    /// session whose persistence already succeeded. Removes the session,
    /// message buffer, routing maps (both directions), gossip subscription and
    /// punch target, then emits `TemporaryChatEnded`. A no-op when no freeze
    /// is pending for the chat (already committed or aborted).
    pub(super) async fn commit_temporary_archive(&mut self, chat_id: &str) {
        if self.pending_finalization.remove(chat_id).is_none() {
            return;
        }
        let network_state = self.network_state.clone();
        {
            let mut temp_state = network_state.temporary_state.lock().await;
            temp_state.chats.remove(chat_id);
            temp_state.messages.remove(chat_id);
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

    /// Recover a frozen temporary session whose archive failed, whose caller
    /// was cancelled, or whose acknowledgement was lost — the safe default
    /// whenever a freeze is never resolved.
    ///
    /// Clears the reservation, restores the drained messages, re-caches the
    /// routing maps (both directions), re-subscribes, re-adds the punch
    /// target, and for group sessions re-broadcasts a signed rejoin add whose
    /// counter exceeds the farewell remove's, so every member that processed
    /// the leave re-admits the local peer. Emits `TemporaryChatRestored`.
    ///
    /// The handler acknowledges only once the session, data and (for groups)
    /// the signed rejoin are all restored. If the keypair is unavailable or
    /// the rejoin cannot be signed, the recovery data is still restored (so
    /// the user does not lose the conversation) but the acknowledgement
    /// carries the error, and the roster is not re-announced — the caller must
    /// not believe the peer rejoined when remote members still treat it as
    /// removed.
    pub(super) async fn abort_temporary_archive(
        &mut self,
        chat_id: &str,
        epoch: Option<u64>,
        ack: Option<tokio::sync::oneshot::Sender<Result<(), String>>>,
    ) {
        let Some(pending) = self.pending_finalization.get(chat_id) else {
            // No freeze pending: nothing to recover. No-op (idempotent) — the
            // freeze was already committed or aborted.
            if let Some(ack) = ack {
                let _ = ack.send(Ok(()));
            }
            return;
        };
        if let Some(epoch) = epoch {
            if pending.epoch != epoch {
                if let Some(ack) = ack {
                    let _ = ack.send(Ok(()));
                }
                return;
            }
        }
        let pending = self.pending_finalization.remove(chat_id).expect("checked");
        let mut session = pending.session;
        session.archived = false;
        if session.next_member_op_counter < pending.min_add_counter {
            session.next_member_op_counter = pending.min_add_counter;
        }
        let mut rejoin_error: Option<String> = None;
        {
            let mut temp_state = self.network_state.temporary_state.lock().await;
            if matches!(pending.kind, crate::app_state::TemporaryChatKind::Group) {
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
            }
            temp_state.chats.insert(chat_id.to_string(), session.clone());
            temp_state.messages.insert(chat_id.to_string(), pending.messages.clone());
        }
        // Restore routing (both directions), subscription and punch target so
        // the session is fully reachable again, exactly as before the freeze.
        for peer in &pending.routing_peers {
            self.cache_temporary_mapping(chat_id, peer);
        }
        if pending.was_subscribed {
            self.subscribe_group(chat_id);
        }
        if let Some(addr) = &pending.punch_target {
            if let Ok(multiaddr) = addr.parse::<Multiaddr>() {
                self.add_punch_target(chat_id, multiaddr);
            }
        }
        let rejoined = rejoin_error.is_none();
        if rejoined && matches!(pending.kind, crate::app_state::TemporaryChatKind::Group) {
            self.broadcast_temp_group_roster(chat_id, None).await;
        }
        self.emit(CoreEvent::TemporaryChatRestored(
            crate::events::TemporaryChatRestoredEvent {
                chat_id: chat_id.to_string(),
                peer_id: self.swarm.local_peer_id().to_string(),
            },
        ));
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
