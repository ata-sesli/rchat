use super::*;
use crate::app_state::{CallKind, VoiceCallPhase};
use crate::live::video::protocol::{
    VideoChunkType, VideoFrameEvent, VideoFrameRequest, VideoFrameResponse,
};
use crate::network::direct_message::{DirectMessageKind, DirectMessageRequest};
use libp2p::request_response;

const CALL_RING_TIMEOUT_SECS: u64 = 30;

impl NetworkManager {
    fn parse_video_chunk_type(value: &str) -> VideoChunkType {
        if value.eq_ignore_ascii_case("key") {
            VideoChunkType::Key
        } else {
            VideoChunkType::Delta
        }
    }

    pub(super) async fn handle_start_video_call(&mut self, peer_chat_id: String) {
        if self.active_broadcast.is_some() {
            self.push_idle_call_state(Some("broadcast_conflict".to_string()))
                .await;
            return;
        }

        if self.active_call.is_some() {
            self.push_idle_call_state(Some("busy".to_string())).await;
            return;
        }

        if !matches!(
            crate::chat_kind::parse_chat_kind(&peer_chat_id),
            crate::chat_kind::ChatKind::Direct
        ) {
            self.push_idle_call_state(Some("unsupported_chat_type".to_string()))
                .await;
            return;
        }

        let Some(peer_id) = self.resolve_peer_id(&peer_chat_id, "VIDEO_CALL").await else {
            self.push_idle_call_state(Some("peer_unresolved".to_string()))
                .await;
            return;
        };

        if !self.swarm.is_connected(&peer_id) {
            self.push_idle_call_state(Some("peer_not_connected".to_string()))
                .await;
            return;
        }
        if !self.peer_has_quic_path(&peer_id) {
            self.push_idle_call_state(Some("quic_required".to_string()))
                .await;
            return;
        }

        let now = Self::now_unix_ts();
        let call_id = format!("call-{}-{}", now, rand::random::<u32>());
        let call = ActiveCall {
            call_id: call_id.clone(),
            kind: CallKind::Video,
            peer_chat_id,
            remote_peer_id: peer_id,
            phase: ActiveCallPhase::OutgoingRinging,
            ring_deadline: Some(
                std::time::Instant::now() + std::time::Duration::from_secs(CALL_RING_TIMEOUT_SECS),
            ),
            ring_expires_at: Some(now + CALL_RING_TIMEOUT_SECS as i64),
            started_at: None,
            muted: false,
            camera_enabled: true,
        };

        let offer = DirectMessageRequest {
            id: call_id.clone(),
            sender_id: self.swarm.local_peer_id().to_string(),
            msg_type: DirectMessageKind::CallOfferVideo,
            text_content: None,
            file_hash: None,
            timestamp: now,
            chunk_hash: None,
            chunk_data: None,
            chunk_list: None,
            sender_alias: None,
        };
        self.swarm
            .behaviour_mut()
            .direct_message
            .send_request(&call.remote_peer_id, offer);

        self.push_active_call_state(&call, VoiceCallPhase::OutgoingRinging, None)
            .await;
        self.active_call = Some(call);
    }

    pub(super) async fn handle_accept_video_call(&mut self, call_id: String) {
        let Some(call_snapshot) = self.active_call.as_ref().cloned() else {
            return;
        };
        if call_snapshot.call_id != call_id
            || call_snapshot.phase != ActiveCallPhase::IncomingRinging
            || call_snapshot.kind != CallKind::Video
        {
            return;
        }
        if !self.peer_has_quic_path(&call_snapshot.remote_peer_id) {
            self.send_call_signal(
                call_snapshot.remote_peer_id,
                DirectMessageKind::CallEnd,
                &call_snapshot.call_id,
            );
            self.transition_to_idle(Some("quic_required".to_string()))
                .await;
            return;
        }

        self.send_call_signal(
            call_snapshot.remote_peer_id,
            DirectMessageKind::CallAcceptVideo,
            &call_snapshot.call_id,
        );

        if let Err(e) = self.start_voice_audio() {
            self.send_call_signal(
                call_snapshot.remote_peer_id,
                DirectMessageKind::CallEnd,
                &call_snapshot.call_id,
            );
            self.transition_to_idle(Some(format!("audio_start_failed: {}", e)))
                .await;
            return;
        }

        let mut updated = call_snapshot.clone();
        updated.phase = ActiveCallPhase::Active;
        updated.ring_deadline = None;
        updated.ring_expires_at = None;
        updated.started_at = Some(Self::now_unix_ts());
        self.active_call = Some(updated.clone());
        self.push_active_call_state(&updated, VoiceCallPhase::Active, None)
            .await;
    }

    pub(super) async fn handle_reject_video_call(&mut self, call_id: String) {
        let Some(call) = self.active_call.as_ref().cloned() else {
            return;
        };
        if call.call_id != call_id
            || call.phase != ActiveCallPhase::IncomingRinging
            || call.kind != CallKind::Video
        {
            return;
        }
        self.send_call_signal(call.remote_peer_id, DirectMessageKind::CallReject, &call.call_id);
        self.transition_to_idle(Some("rejected".to_string())).await;
    }

    pub(super) async fn handle_end_video_call(&mut self, call_id: String) {
        let Some(call) = self.active_call.as_ref().cloned() else {
            return;
        };
        if call.call_id != call_id || call.kind != CallKind::Video {
            return;
        }
        self.push_active_call_state(&call, VoiceCallPhase::Ending, None)
            .await;
        self.send_call_signal(call.remote_peer_id, DirectMessageKind::CallEnd, &call.call_id);
        self.transition_to_idle(Some("ended".to_string())).await;
    }

    pub(super) async fn handle_set_video_call_muted(&mut self, call_id: String, muted: bool) {
        let Some(call_snapshot) = self.active_call.as_ref().cloned() else {
            return;
        };
        if call_snapshot.call_id != call_id
            || call_snapshot.phase != ActiveCallPhase::Active
            || call_snapshot.kind != CallKind::Video
        {
            return;
        }
        let mut updated = call_snapshot;
        updated.muted = muted;
        self.active_call = Some(updated.clone());
        self.push_active_call_state(&updated, VoiceCallPhase::Active, None)
            .await;
    }

    pub(super) async fn handle_set_video_call_camera_enabled(
        &mut self,
        call_id: String,
        enabled: bool,
    ) {
        let Some(call_snapshot) = self.active_call.as_ref().cloned() else {
            return;
        };
        if call_snapshot.call_id != call_id
            || call_snapshot.phase != ActiveCallPhase::Active
            || call_snapshot.kind != CallKind::Video
        {
            return;
        }
        let mut updated = call_snapshot;
        updated.camera_enabled = enabled;
        self.active_call = Some(updated.clone());
        self.push_active_call_state(&updated, VoiceCallPhase::Active, None)
            .await;
    }

    pub(super) async fn handle_send_video_call_chunk(
        &mut self,
        call_id: String,
        seq: u32,
        timestamp: i64,
        mime: String,
        codec: String,
        chunk_type: String,
        payload: Vec<u8>,
    ) {
        let Some(call_snapshot) = self.active_call.as_ref().cloned() else {
            return;
        };
        if call_snapshot.call_id != call_id
            || call_snapshot.phase != ActiveCallPhase::Active
            || call_snapshot.kind != CallKind::Video
            || !call_snapshot.camera_enabled
        {
            return;
        }
        if !self.peer_has_quic_path(&call_snapshot.remote_peer_id) {
            self.transition_to_idle(Some("quic_path_lost".to_string()))
                .await;
            return;
        }

        self.swarm
            .behaviour_mut()
            .video_call
            .send_request(
                &call_snapshot.remote_peer_id,
                VideoFrameRequest {
                    call_id,
                    seq,
                    timestamp,
                    mime,
                    codec,
                    chunk_type: Self::parse_video_chunk_type(&chunk_type),
                    payload,
                },
            );
    }

    pub(super) async fn handle_video_frame_event(
        &mut self,
        event: request_response::Event<VideoFrameRequest, VideoFrameResponse>,
    ) {
        use request_response::{Event, Message};

        match event {
            Event::Message { peer, message, .. } => match message {
                Message::Request {
                    request, channel, ..
                } => {
                    let mut accepted = false;
                    if let Some(call) = self.active_call.as_ref() {
                        if call.phase == ActiveCallPhase::Active
                            && call.kind == CallKind::Video
                            && call.call_id == request.call_id
                            && call.remote_peer_id == peer
                        {
                            let event = VideoFrameEvent {
                                call_id: request.call_id,
                                peer_id: peer.to_string(),
                                seq: request.seq,
                                timestamp: request.timestamp,
                                mime: request.mime,
                                codec: request.codec,
                                chunk_type: request.chunk_type,
                                payload: request.payload,
                            };
                            let _ = self.app_handle.emit(
                                "video-call-frame",
                                event,
                            );
                            accepted = true;
                        }
                    }
                    let _ = self
                        .swarm
                        .behaviour_mut()
                        .video_call
                        .send_response(channel, VideoFrameResponse { ok: accepted });
                }
                Message::Response { response, .. } => {
                    if !response.ok {
                        let should_end = self
                            .active_call
                            .as_ref()
                            .map(|call| {
                                call.phase == ActiveCallPhase::Active
                                    && call.kind == CallKind::Video
                                    && call.remote_peer_id == peer
                            })
                            .unwrap_or(false);
                        if should_end {
                            self.transition_to_idle(Some("stream_rejected".to_string()))
                                .await;
                        }
                    }
                }
            },
            Event::OutboundFailure { peer, error, .. } => {
                eprintln!("[Video] Outbound frame failure to {}: {:?}", peer, error);
                let should_end = self
                    .active_call
                    .as_ref()
                    .map(|call| {
                        call.phase == ActiveCallPhase::Active
                            && call.kind == CallKind::Video
                            && call.remote_peer_id == peer
                    })
                    .unwrap_or(false);
                if should_end {
                    self.transition_to_idle(Some("stream_failure".to_string()))
                        .await;
                }
            }
            Event::InboundFailure { peer, error, .. } => {
                eprintln!("[Video] Inbound frame failure from {}: {:?}", peer, error);
                let should_end = self
                    .active_call
                    .as_ref()
                    .map(|call| {
                        call.phase == ActiveCallPhase::Active
                            && call.kind == CallKind::Video
                            && call.remote_peer_id == peer
                    })
                    .unwrap_or(false);
                if should_end {
                    self.transition_to_idle(Some("stream_failure".to_string()))
                        .await;
                }
            }
            Event::ResponseSent { .. } => {}
        }
    }
}
