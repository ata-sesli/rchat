use super::*;

impl NetworkManager {
    pub(super) async fn handle_gossipsub_message(&mut self, message: libp2p::gossipsub::Message) {
        let topic = message.topic.to_string();

        if topic == crate::network::gossip::CONTROL_TOPIC {
            let control: Result<crate::network::gossip::ControlEnvelope, _> =
                serde_json::from_slice(&message.data);
            if let Ok(crate::network::gossip::ControlEnvelope::ConnectionRequest {
                from_peer_id,
                to_peer_id,
            }) = control
            {
                let local = self.swarm.local_peer_id().to_string();
                if to_peer_id == local {
                    if let Ok(from_peer) = from_peer_id.parse::<PeerId>() {
                        self.handle_incoming_connection_request(from_peer);
                    }
                }
            }
            return;
        }

        let Some(topic_group_id) = crate::network::gossip::group_id_from_topic(&topic) else {
            println!("[Gossipsub] Ignoring non-group topic: {}", topic);
            return;
        };

        if let Ok(record) =
            serde_json::from_slice::<crate::network::gossip::SignedGroupRecord>(&message.data)
        {
            if record.group_id() != topic_group_id {
                eprintln!(
                    "[Group] Topic/record mismatch. topic={}, payload={}",
                    topic_group_id,
                    record.group_id()
                );
                return;
            }
            let verified = record.verify();
            if !verified {
                eprintln!("[Group] Unverified group record ignored: {}", record.id());
                let _ = crate::chat::group::apply_signed_record(
                    &self.app_state,
                    Some(&self.event_sink),
                    &record,
                    false,
                );
                return;
            }
            if record.author_peer_id() == self.swarm.local_peer_id().to_string() {
                return;
            }
            let applied_message = matches!(
                record.body(),
                crate::network::gossip::GroupRecordBody::Message { .. }
            );
            match crate::chat::group::apply_signed_record(
                &self.app_state,
                Some(&self.event_sink),
                &record,
                true,
            ) {
                Ok(true) => {
                    if let crate::network::gossip::GroupRecordBody::Message {
                        file_hash: Some(file_hash),
                        ..
                    } = record.body()
                    {
                        self.request_group_file_metadata(
                            record.group_id(),
                            file_hash,
                            record.author_peer_id(),
                        )
                        .await;
                    }
                    if applied_message {
                        self.publish_group_delivered_receipt(record.group_id(), record.id())
                            .await;
                    }
                }
                Ok(false) => {}
                Err(err) => eprintln!("[Group] Failed to apply record {}: {}", record.id(), err),
            }
            return;
        }

        let mut envelope: crate::network::gossip::GroupMessageEnvelope =
            match serde_json::from_slice(&message.data) {
                Ok(v) => v,
                Err(e) => {
                    println!("[Gossipsub] Ignoring non-group payload: {}", e);
                    return;
                }
            };

        if envelope.group_id != topic_group_id {
            eprintln!(
                "[Group] Topic/group mismatch. topic={}, payload={}",
                topic_group_id, envelope.group_id
            );
            return;
        }

        if !crate::chat_kind::is_group_chat_id(&envelope.group_id)
            && !crate::chat_kind::is_temp_group_chat_id(&envelope.group_id)
        {
            eprintln!("[Group] Invalid group id in payload: {}", envelope.group_id);
            return;
        }

        if envelope.sender_id.is_empty() {
            if let Some(source) = message.source {
                envelope.sender_id = source.to_string();
            }
        } else if let Some(source) = message.source {
            // A non-empty sender_id must be the authenticated transport source:
            // anything else is a spoofed publisher identity and is dropped
            // before it can be stored, emitted or trigger a media request.
            if envelope.sender_id != source.to_string() {
                eprintln!(
                    "[Group] Rejecting spoofed sender_id {} from source {}",
                    envelope.sender_id, source
                );
                return;
            }
        }

        if envelope.sender_id == self.swarm.local_peer_id().to_string() {
            return;
        }

        let db_msg = super::super::build_incoming_group_db_message(&envelope);

        let is_temp_group = crate::chat_kind::is_temp_group_chat_id(&envelope.group_id);
        if is_temp_group {
            // A missing, archived, or unauthorized session must not grow
            // phantom history, and only an admitted member may publish into a
            // temporary group. While the session is reserved for archiving the
            // message is dropped: the snapshot was already taken, and letting
            // a late message through would create history the archive can no
            // longer capture coherently.
            let network_state = &self.network_state;
            let mut temp_state = network_state.temporary_state.lock().await;
            let Some(session) = temp_state.chats.get(&envelope.group_id) else {
                return;
            };
            if session.archived {
                return;
            }
            if !session.is_member(&envelope.sender_id) {
                eprintln!(
                    "[TempGroup] Rejecting message from non-member {} for {}",
                    envelope.sender_id, envelope.group_id
                );
                return;
            }
            temp_state
                .messages
                .entry(envelope.group_id.clone())
                .or_default()
                .push(db_msg.clone());
        } else if let Err(e) = self
            .persist_incoming_group_message(&envelope, db_msg.clone())
            .await
        {
            eprintln!(
                "[Group] Failed to save message {} for {}: {}",
                db_msg.id, db_msg.chat_id, e
            );
            return;
        }

        if envelope.content_type.needs_file_transfer() {
            if let Some(ref file_hash) = envelope.file_hash {
                self.request_group_file_metadata(&envelope.group_id, file_hash, &envelope.sender_id)
                    .await;
            }
        }

        self.emit(CoreEvent::MessageReceived(db_msg));
    }

    pub(super) async fn request_group_file_metadata(
        &mut self,
        group_id: &str,
        file_hash: &str,
        preferred_peer_id: &str,
    ) {
        // Media routing into a temporary group is only authorized for admitted
        // members of a live (non-archived) session.
        if crate::chat_kind::is_temp_group_chat_id(group_id) {
            let network_state = &self.network_state;
            let temp_state = network_state.temporary_state.lock().await;
            let Some(session) = temp_state.chats.get(group_id) else {
                return;
            };
            if session.archived {
                return;
            }
            if !session.is_member(preferred_peer_id) {
                return;
            }
        }
        let mut candidates = vec![preferred_peer_id.to_string()];
        if let Ok(conn) = self.app_state.db_conn.lock() {
            if let Ok(sources) =
                crate::storage::db::get_group_file_sources(&conn, group_id, file_hash)
            {
                for source in sources {
                    if !candidates.iter().any(|p| p == &source.peer_id) {
                        candidates.push(source.peer_id);
                    }
                }
            }
        }
        // Temporary-group members are all eligible sources; add every remote
        // member once so media routing reaches the whole member set.
        if crate::chat_kind::is_temp_group_chat_id(group_id) {
            let local = self.swarm.local_peer_id().to_string();
            let network_state = &self.network_state;
            let temp_state = network_state.temporary_state.lock().await;
            if let Some(session) = temp_state.chats.get(group_id) {
                for member in session.remote_members(Some(&local)) {
                    if !candidates.iter().any(|p| p == &member) {
                        candidates.push(member);
                    }
                }
            }
        }

        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        for candidate in candidates {
            let Ok(peer_id) = candidate.parse::<PeerId>() else {
                continue;
            };
            use crate::network::direct_message::{DirectMessageKind, DirectMessageRequest};
            let metadata_req = DirectMessageRequest {
                id: format!("group-meta-req-{}-{}", file_hash, timestamp),
                sender_id: self.swarm.local_peer_id().to_string(),
                msg_type: DirectMessageKind::FileMetadataRequest,
                text_content: Some(group_id.to_string()),
                file_hash: Some(file_hash.to_string()),
                timestamp,
                chunk_hash: None,
                chunk_data: None,
                chunk_list: None,
                sender_alias: None,
            };
            self.swarm
                .behaviour_mut()
                .direct_message
                .send_request(&peer_id, metadata_req);
        }
    }
}
