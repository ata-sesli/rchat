use super::*;
use crate::network::command::NetworkCommand;

mod control;
mod dm;
mod group;

impl NetworkManager {
    pub async fn dispatch_command(&mut self, command: NetworkCommand) {
        match command {
            NetworkCommand::StartPunch {
                multiaddr,
                target_username,
                my_username,
            } => self.handle_start_punch_command(multiaddr, target_username, my_username),
            NetworkCommand::RequestConnection { peer_id } => {
                self.handle_connection_request(&peer_id).await;
            }
            NetworkCommand::RegisterShadow {
                invitee,
                password,
                my_username,
            } => self.register_shadow_poll(&invitee, &password, &my_username),
            NetworkCommand::RegisterTemporarySession {
                chat_id,
                peer_id,
                multiaddr,
                is_group,
            } => {
                self.register_temporary_session(&chat_id, &peer_id, &multiaddr, is_group);
            }
            NetworkCommand::EndTemporarySession { chat_id } => {
                self.end_temporary_session(&chat_id);
            }
            NetworkCommand::SubscribeGroup { group_id } => self.subscribe_group(&group_id),
            NetworkCommand::UnsubscribeGroup { group_id } => self.unsubscribe_group(&group_id),
            NetworkCommand::PublishGroup { mut envelope } => {
                self.publish_group_message(&mut envelope);
            }
            NetworkCommand::SendDirectText {
                target_peer_id,
                msg_id,
                timestamp,
                sender_alias,
                content,
            } => {
                self.send_direct_text(target_peer_id, msg_id, timestamp, sender_alias, content)
                    .await;
            }
            NetworkCommand::SendReadReceipt {
                target_peer_id,
                msg_ids,
            } => self.send_read_receipt(target_peer_id, msg_ids).await,
            NetworkCommand::SendDirectMedia {
                kind,
                target_peer_id,
                file_hash,
                file_name,
                msg_id,
                timestamp,
            } => {
                self.send_direct_media(
                    kind,
                    target_peer_id,
                    file_hash,
                    file_name,
                    msg_id,
                    timestamp,
                )
                .await;
            }
        }
    }
}
