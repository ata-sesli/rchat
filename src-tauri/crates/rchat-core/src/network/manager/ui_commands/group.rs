use super::*;

impl NetworkManager {
    pub(super) fn publish_group_message(
        &mut self,
        envelope: &mut crate::network::gossip::GroupMessageEnvelope,
    ) {
        if let Some(topic) = crate::network::gossip::topic_for_group_id(&envelope.group_id) {
            envelope.sender_id = self.swarm.local_peer_id().to_string();

            let payload = match serde_json::to_vec(envelope) {
                Ok(v) => v,
                Err(e) => {
                    eprintln!("[Group] ❌ Failed to encode publish envelope: {}", e);
                    return;
                }
            };
            let _ = self.swarm.behaviour_mut().gossipsub.subscribe(&topic);
            self.subscribed_group_ids.insert(envelope.group_id.clone());
            match self.swarm.behaviour_mut().gossipsub.publish(topic, payload) {
                Ok(msg_id) => println!("[Group] ✅ Published group message {:?}", msg_id),
                Err(e) => eprintln!("[Group] ❌ Publish failed: {:?}", e),
            }
        } else {
            eprintln!("[Group] ❌ Invalid group id: {}", envelope.group_id);
        }
    }

    pub(super) fn subscribe_group(&mut self, group_id: &str) {
        if !crate::chat_kind::is_group_chat_id(group_id) {
            eprintln!("[Group] ❌ Invalid group id for subscribe: {}", group_id);
            return;
        }
        if self.subscribed_group_ids.contains(group_id) {
            return;
        }
        if let Some(topic) = crate::network::gossip::topic_for_group_id(group_id) {
            match self.swarm.behaviour_mut().gossipsub.subscribe(&topic) {
                Ok(_) => {
                    self.subscribed_group_ids.insert(group_id.to_string());
                    println!("[Group] ✅ Subscribed {}", group_id);
                }
                Err(e) => eprintln!("[Group] ❌ Failed to subscribe {}: {:?}", group_id, e),
            }
        }
    }

    pub(super) fn unsubscribe_group(&mut self, group_id: &str) {
        if !self.subscribed_group_ids.contains(group_id) {
            return;
        }
        if let Some(topic) = crate::network::gossip::topic_for_group_id(group_id) {
            if self.swarm.behaviour_mut().gossipsub.unsubscribe(&topic) {
                self.subscribed_group_ids.remove(group_id);
                println!("[Group] ✅ Unsubscribed {}", group_id);
            } else {
                eprintln!("[Group] ❌ Failed to unsubscribe {}", group_id);
            }
        }
    }

    pub(in crate::network::manager) fn publish_group_record(
        &mut self,
        record: &crate::network::gossip::SignedGroupRecord,
    ) {
        if let Some(topic) = crate::network::gossip::topic_for_group_id(record.group_id()) {
            let payload = match serde_json::to_vec(record) {
                Ok(v) => v,
                Err(e) => {
                    eprintln!("[Group] ❌ Failed to encode group record: {}", e);
                    return;
                }
            };
            let _ = self.swarm.behaviour_mut().gossipsub.subscribe(&topic);
            self.subscribed_group_ids
                .insert(record.group_id().to_string());
            match self.swarm.behaviour_mut().gossipsub.publish(topic, payload) {
                Ok(msg_id) => println!(
                    "[Group] ✅ Published group record {} ({:?})",
                    record.id(),
                    msg_id
                ),
                Err(e) => eprintln!(
                    "[Group] ❌ Record publish failed {}: {:?}",
                    record.id(),
                    e
                ),
            }
        } else {
            eprintln!("[Group] ❌ Invalid group id: {}", record.group_id());
        }
    }

    pub(in crate::network::manager) async fn publish_group_delivered_receipt(
        &mut self,
        group_id: &str,
        message_id: &str,
    ) {
        let record = match crate::chat::group::create_receipt_record(
            &self.app_state,
            group_id.to_string(),
            vec![message_id.to_string()],
            crate::network::gossip::GroupReceiptStatus::Delivered,
        )
        .await
        {
            Ok(record) => record,
            Err(err) => {
                eprintln!(
                    "[Group] Failed to create delivered receipt for {}: {}",
                    message_id, err
                );
                return;
            }
        };

        match crate::chat::group::apply_signed_record(
            &self.app_state,
            Some(&self.event_sink),
            &record,
            true,
        ) {
            Ok(true) => self.publish_group_record(&record),
            Ok(false) => {}
            Err(err) => eprintln!(
                "[Group] Failed to apply delivered receipt {}: {}",
                record.id(),
                err
            ),
        }
    }

    pub(super) async fn send_group_invite(
        &mut self,
        target_peer_id: String,
        invite: crate::network::gossip::GroupInvitePayload,
    ) {
        let Some(peer_id) = self.resolve_peer_id(&target_peer_id, "GROUP_INVITE").await else {
            return;
        };
        let payload = match serde_json::to_string(&invite) {
            Ok(payload) => payload,
            Err(err) => {
                eprintln!("[Group] ❌ Failed to encode invite {}: {}", invite.invite_id, err);
                return;
            }
        };
        let request = crate::network::direct_message::DirectMessageRequest {
            id: format!("group-invite-{}", invite.invite_id),
            sender_id: self.swarm.local_peer_id().to_string(),
            msg_type: crate::network::direct_message::DirectMessageKind::GroupInvite,
            text_content: Some(payload),
            file_hash: None,
            timestamp: invite.created_at,
            chunk_hash: None,
            chunk_data: None,
            chunk_list: None,
            sender_alias: None,
        };
        self.swarm
            .behaviour_mut()
            .direct_message
            .send_request(&peer_id, request);
        println!(
            "[Group] ✅ Sent invite {} for {} to {}",
            invite.invite_id, invite.group_id, peer_id
        );
    }

    pub(super) async fn send_group_dissolution(
        &mut self,
        target_peer_id: String,
        record: crate::network::gossip::SignedGroupRecord,
    ) {
        let Some(peer_id) = self.resolve_peer_id(&target_peer_id, "GROUP_DISSOLUTION").await else {
            return;
        };
        let Ok(payload) = serde_json::to_string(&record) else {
            return;
        };
        let request = crate::network::direct_message::DirectMessageRequest {
            id: format!("group-dissolution-{}", record.id()),
            sender_id: self.swarm.local_peer_id().to_string(),
            msg_type: crate::network::direct_message::DirectMessageKind::GroupDissolution,
            text_content: Some(payload),
            file_hash: None,
            timestamp: record.timestamp(),
            chunk_hash: None,
            chunk_data: None,
            chunk_list: None,
            sender_alias: None,
        };
        self.swarm
            .behaviour_mut()
            .direct_message
            .send_request(&peer_id, request);
    }

    pub(super) async fn request_group_sync(&mut self, group_id: &str) {
        let request = crate::network::gossip::GroupSyncRequest {
            version: crate::network::gossip::GROUP_PROTOCOL_VERSION,
            group_id: group_id.to_string(),
            wanted_record_ids: Vec::new(),
            cursor: None,
            limit: 256,
        };
        let peers = {
            let state = &self.app_state;
            match state.db_conn.lock() {
                Ok(conn) => {
                    crate::storage::db::get_group_member_peer_ids(&conn, group_id)
                        .unwrap_or_default()
                }
                Err(_) => Vec::new(),
            }
        };
        let mut sent = 0usize;
        for peer_id in peers {
            let Ok(peer) = peer_id.parse() else {
                continue;
            };
            self.begin_group_sync(group_id, &peer);
            if self
                .send_group_sync_request_to_peer(&peer, &request)
                .is_ok()
            {
                sent += 1;
            }
        }
        let (state, detail) = if sent == 0 {
            (
                "failed",
                "no connected group peers are available for history sync".to_string(),
            )
        } else {
            ("requested", format!("sent to {sent} peer(s)"))
        };
        self.emit(crate::events::CoreEvent::GroupSyncStateUpdated(
            crate::events::GroupSyncStateUpdatedEvent {
                group_id: group_id.to_string(),
                state: state.to_string(),
                detail: Some(detail),
            },
        ));
    }

    pub(crate) fn send_group_sync_request_to_peer(
        &mut self,
        peer: &libp2p::PeerId,
        request: &crate::network::gossip::GroupSyncRequest,
    ) -> Result<(), String> {
        request.validate()?;
        if request.cursor.is_none() && !request.wanted_record_ids.is_empty() {
            let key = Self::group_sync_progress_key(&request.group_id, peer);
            let progress = self.group_sync_explicit_requests.entry(key).or_default();
            super::record_group_sync_request(progress, request);
        }
        let payload = serde_json::to_string(request)
            .map_err(|error| format!("failed to encode group sync request: {error}"))?;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default();
        let direct_request = crate::network::direct_message::DirectMessageRequest {
            id: format!("group-sync-{}-{}", request.group_id, now.as_nanos()),
            sender_id: self.swarm.local_peer_id().to_string(),
            msg_type: crate::network::direct_message::DirectMessageKind::GroupSyncRequest,
            text_content: Some(payload),
            file_hash: None,
            timestamp: now.as_secs() as i64,
            chunk_hash: None,
            chunk_data: None,
            chunk_list: None,
            sender_alias: None,
        };
        self.swarm
            .behaviour_mut()
            .direct_message
            .send_request(peer, direct_request);
        Ok(())
    }
}
