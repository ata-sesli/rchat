use super::*;
use crate::network::command::DirectMediaKind;

impl NetworkManager {
    pub(super) async fn send_direct_text(
        &mut self,
        target_peer_id: String,
        msg_id: String,
        timestamp: i64,
        sender_alias: Option<String>,
        content: String,
    ) {
        println!(
            "[DM] 📤 Sending direct message to {} (alias: {}): {}",
            target_peer_id,
            sender_alias.as_deref().unwrap_or_default(),
            content
        );

        if let Some(peer_id) = self.resolve_peer_id(&target_peer_id, "DM").await {
            use crate::network::direct_message::{DirectMessageKind, DirectMessageRequest};
            let request = DirectMessageRequest {
                id: msg_id,
                sender_id: self.swarm.local_peer_id().to_string(),
                msg_type: DirectMessageKind::Text,
                text_content: Some(content),
                file_hash: None,
                timestamp,
                chunk_hash: None,
                chunk_data: None,
                chunk_list: None,
                sender_alias,
            };

            self.swarm
                .behaviour_mut()
                .direct_message
                .send_request(&peer_id, request);
            println!("[DM] ✅ Request sent to {}", peer_id);
        }
    }

    pub(super) async fn send_read_receipt(&mut self, target_peer_id: String, msg_ids: Vec<String>) {
        println!(
            "[READ_RECEIPT] 📤 Sending read receipt to {}",
            target_peer_id
        );

        if let Some(peer_id) = self.resolve_peer_id(&target_peer_id, "READ_RECEIPT").await {
            use crate::network::direct_message::{DirectMessageKind, DirectMessageRequest};
            let request = DirectMessageRequest {
                id: format!(
                    "read-receipt-{}",
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .as_secs()
                ),
                sender_id: self.swarm.local_peer_id().to_string(),
                msg_type: DirectMessageKind::ReadReceipt,
                text_content: Some(msg_ids.join(",")),
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
                .send_request(&peer_id, request);
            println!("[READ_RECEIPT] ✅ Sent to {}", peer_id);
        }
    }

    pub(super) async fn send_direct_media(
        &mut self,
        kind: DirectMediaKind,
        target_peer_id: String,
        file_hash: String,
        file_name: Option<String>,
        msg_id: String,
        timestamp: i64,
    ) {
        let context = match kind {
            DirectMediaKind::Image => "Image",
            DirectMediaKind::Sticker => "Sticker",
            DirectMediaKind::Document => "Document",
            DirectMediaKind::Video => "Video",
            DirectMediaKind::Audio => "Audio",
        };

        println!(
            "[{}] 📤 Sending {} {} to {}",
            context,
            context.to_ascii_lowercase(),
            file_hash,
            target_peer_id
        );

        if let Some(peer_id) = self.resolve_peer_id(&target_peer_id, context).await {
            use crate::network::direct_message::{DirectMessageKind, DirectMessageRequest};
            let (msg_type, text_content) = match kind {
                DirectMediaKind::Image => (DirectMessageKind::Image, None),
                DirectMediaKind::Sticker => (DirectMessageKind::Sticker, None),
                DirectMediaKind::Document => (
                    DirectMessageKind::Document,
                    Some(file_name.unwrap_or_else(|| "document".to_string())),
                ),
                DirectMediaKind::Video => (
                    DirectMessageKind::Video,
                    Some(file_name.unwrap_or_else(|| "video.mp4".to_string())),
                ),
                DirectMediaKind::Audio => (
                    DirectMessageKind::Audio,
                    Some(file_name.unwrap_or_else(|| "audio".to_string())),
                ),
            };

            let request = DirectMessageRequest {
                id: msg_id,
                sender_id: self.swarm.local_peer_id().to_string(),
                msg_type,
                text_content,
                file_hash: Some(file_hash),
                timestamp,
                chunk_hash: None,
                chunk_data: None,
                chunk_list: None,
                sender_alias: None,
            };

            self.swarm
                .behaviour_mut()
                .direct_message
                .send_request(&peer_id, request);
            println!("[{}] ✅ Direct request sent to {}", context, peer_id);
        }
    }
}
