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
            NetworkCommand::DropConnection { peer_id } => {
                self.handle_drop_connection(&peer_id).await;
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
            NetworkCommand::StartVoiceCall { peer_id } => {
                self.handle_start_voice_call(peer_id).await;
            }
            NetworkCommand::AcceptVoiceCall { call_id } => {
                self.handle_accept_voice_call(call_id).await;
            }
            NetworkCommand::RejectVoiceCall { call_id } => {
                self.handle_reject_voice_call(call_id).await;
            }
            NetworkCommand::EndVoiceCall { call_id } => {
                self.handle_end_voice_call(call_id).await;
            }
            NetworkCommand::SetVoiceCallMuted { call_id, muted } => {
                self.handle_set_voice_call_muted(call_id, muted).await;
            }
            NetworkCommand::StartVideoCall { peer_id } => {
                self.handle_start_video_call(peer_id).await;
            }
            NetworkCommand::AcceptVideoCall { call_id } => {
                self.handle_accept_video_call(call_id).await;
            }
            NetworkCommand::RejectVideoCall { call_id } => {
                self.handle_reject_video_call(call_id).await;
            }
            NetworkCommand::EndVideoCall { call_id } => {
                self.handle_end_video_call(call_id).await;
            }
            NetworkCommand::SetVideoCallMuted { call_id, muted } => {
                self.handle_set_video_call_muted(call_id, muted).await;
            }
            NetworkCommand::SetVideoCallCameraEnabled { call_id, enabled } => {
                self.handle_set_video_call_camera_enabled(call_id, enabled)
                    .await;
            }
            NetworkCommand::SendVideoCallChunk {
                call_id,
                seq,
                timestamp,
                mime,
                codec,
                chunk_type,
                payload,
            } => {
                self.handle_send_video_call_chunk(
                    call_id, seq, timestamp, mime, codec, chunk_type, payload,
                )
                    .await;
            }
            NetworkCommand::StartScreenBroadcast { peer_id } => {
                self.handle_start_screen_broadcast(peer_id).await;
            }
            NetworkCommand::AcceptScreenBroadcast { session_id } => {
                self.handle_accept_screen_broadcast(session_id).await;
            }
            NetworkCommand::RejectScreenBroadcast { session_id } => {
                self.handle_reject_screen_broadcast(session_id).await;
            }
            NetworkCommand::EndScreenBroadcast { session_id } => {
                self.handle_end_screen_broadcast(session_id).await;
            }
            NetworkCommand::SendScreenBroadcastChunk {
                session_id,
                seq,
                timestamp,
                mime,
                codec,
                chunk_type,
                payload,
            } => {
                self.handle_send_screen_broadcast_chunk(
                    session_id,
                    seq,
                    timestamp,
                    mime,
                    codec,
                    chunk_type,
                    payload,
                )
                .await;
            }
        }
    }
}
