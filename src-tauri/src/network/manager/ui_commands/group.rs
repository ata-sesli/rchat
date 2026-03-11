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
}
