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
            envelope.sender_id = message.source.map(|p| p.to_string()).unwrap_or_default();
        }

        if envelope.sender_id == self.swarm.local_peer_id().to_string() {
            return;
        }

        let db_msg = super::super::build_incoming_group_db_message(&envelope);

        let is_temp_group = crate::chat_kind::is_temp_group_chat_id(&envelope.group_id);
        if is_temp_group {
            use tauri::Manager;
            let network_state = self.app_handle.state::<crate::NetworkState>();
            let mut temp_state = network_state.temporary_state.lock().await;
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
                if let Ok(sender_peer_id) = envelope.sender_id.parse::<PeerId>() {
                    use crate::network::direct_message::{DirectMessageKind, DirectMessageRequest};
                    let metadata_req = DirectMessageRequest {
                        id: format!("meta-req-{}", file_hash),
                        sender_id: self.swarm.local_peer_id().to_string(),
                        msg_type: DirectMessageKind::FileMetadataRequest,
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
                        .send_request(&sender_peer_id, metadata_req);
                }
            }
        }

        let _ = self.app_handle.emit("message-received", db_msg);
    }
}
