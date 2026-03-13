use super::*;
use crate::app_state::{BroadcastPhase, CallKind};
use crate::live::broadcast::protocol::{
    BroadcastChunkType, BroadcastFrameEvent, BroadcastFrameRequest, BroadcastFrameResponse,
};
use crate::network::direct_message::{DirectMessageKind, DirectMessageRequest};
use libp2p::request_response;

const BROADCAST_RING_TIMEOUT_SECS: u64 = 30;

impl NetworkManager {
    fn parse_broadcast_chunk_type(value: &str) -> BroadcastChunkType {
        if value.eq_ignore_ascii_case("key") {
            BroadcastChunkType::Key
        } else {
            BroadcastChunkType::Delta
        }
    }

    fn broadcast_session_id_from_signal(request: &DirectMessageRequest) -> String {
        request
            .text_content
            .clone()
            .filter(|v| !v.trim().is_empty())
            .unwrap_or_else(|| request.id.clone())
    }

    fn send_broadcast_signal(&mut self, peer: PeerId, kind: DirectMessageKind, session_id: &str) {
        let now = Self::now_unix_ts();
        let req = DirectMessageRequest {
            id: format!("broadcast-signal-{}-{}", kind.as_str(), now),
            sender_id: self.swarm.local_peer_id().to_string(),
            msg_type: kind,
            text_content: Some(session_id.to_string()),
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
            .send_request(&peer, req);
    }

    async fn push_idle_broadcast_state(&mut self, reason: Option<String>) {
        self.set_broadcast_state(crate::app_state::BroadcastState::default(), reason)
            .await;
    }

    async fn push_active_broadcast_state(
        &mut self,
        session: &ActiveBroadcast,
        phase: BroadcastPhase,
        reason: Option<String>,
    ) {
        self.set_broadcast_state(
            crate::app_state::BroadcastState {
                phase,
                session_id: Some(session.session_id.clone()),
                peer_id: Some(session.peer_chat_id.clone()),
                started_at: session.started_at,
                ring_expires_at: session.ring_expires_at,
                is_host: session.is_host,
                reason: None,
            },
            reason,
        )
        .await;
    }

    async fn transition_broadcast_to_idle(&mut self, reason: Option<String>) {
        self.active_broadcast = None;
        self.push_idle_broadcast_state(reason).await;
    }

    fn broadcast_conflict_reason(&self, peer_chat_id: &str) -> Option<String> {
        let Some(call) = self.active_call.as_ref() else {
            return None;
        };

        match call.kind {
            CallKind::Video => Some("video_call_conflict".to_string()),
            CallKind::Voice => {
                if call.phase != ActiveCallPhase::Active {
                    Some("call_conflict".to_string())
                } else if call.peer_chat_id != peer_chat_id {
                    Some("voice_peer_conflict".to_string())
                } else {
                    None
                }
            }
        }
    }

    pub(super) async fn handle_start_screen_broadcast(&mut self, peer_chat_id: String) {
        if self.active_broadcast.is_some() {
            self.push_idle_broadcast_state(Some("busy".to_string())).await;
            return;
        }

        if !matches!(
            crate::chat_kind::parse_chat_kind(&peer_chat_id),
            crate::chat_kind::ChatKind::Direct
        ) {
            self.push_idle_broadcast_state(Some("unsupported_chat_type".to_string()))
                .await;
            return;
        }

        if let Some(reason) = self.broadcast_conflict_reason(&peer_chat_id) {
            self.push_idle_broadcast_state(Some(reason)).await;
            return;
        }

        let Some(peer_id) = self.resolve_peer_id(&peer_chat_id, "SCREEN_BROADCAST").await else {
            self.push_idle_broadcast_state(Some("peer_unresolved".to_string()))
                .await;
            return;
        };

        if !self.swarm.is_connected(&peer_id) {
            self.push_idle_broadcast_state(Some("peer_not_connected".to_string()))
                .await;
            return;
        }

        let now = Self::now_unix_ts();
        let session_id = format!("broadcast-{}-{}", now, rand::random::<u32>());
        let session = ActiveBroadcast {
            session_id: session_id.clone(),
            peer_chat_id,
            remote_peer_id: peer_id,
            phase: ActiveBroadcastPhase::OutgoingRinging,
            ring_deadline: Some(
                std::time::Instant::now() + std::time::Duration::from_secs(BROADCAST_RING_TIMEOUT_SECS),
            ),
            ring_expires_at: Some(now + BROADCAST_RING_TIMEOUT_SECS as i64),
            started_at: None,
            is_host: true,
        };

        let offer = DirectMessageRequest {
            id: session_id,
            sender_id: self.swarm.local_peer_id().to_string(),
            msg_type: DirectMessageKind::BroadcastOffer,
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
            .send_request(&session.remote_peer_id, offer);

        self.push_active_broadcast_state(&session, BroadcastPhase::OutgoingRinging, None)
            .await;
        self.active_broadcast = Some(session);
    }

    pub(super) async fn handle_accept_screen_broadcast(&mut self, session_id: String) {
        let Some(session_snapshot) = self.active_broadcast.as_ref().cloned() else {
            return;
        };
        if session_snapshot.session_id != session_id
            || session_snapshot.phase != ActiveBroadcastPhase::IncomingRinging
            || session_snapshot.is_host
        {
            return;
        }

        self.send_broadcast_signal(
            session_snapshot.remote_peer_id,
            DirectMessageKind::BroadcastAccept,
            &session_snapshot.session_id,
        );

        let mut updated = session_snapshot;
        updated.phase = ActiveBroadcastPhase::Active;
        updated.ring_deadline = None;
        updated.ring_expires_at = None;
        updated.started_at = Some(Self::now_unix_ts());
        self.active_broadcast = Some(updated.clone());
        self.push_active_broadcast_state(&updated, BroadcastPhase::Active, None)
            .await;
    }

    pub(super) async fn handle_reject_screen_broadcast(&mut self, session_id: String) {
        let Some(session) = self.active_broadcast.as_ref().cloned() else {
            return;
        };
        if session.session_id != session_id
            || session.phase != ActiveBroadcastPhase::IncomingRinging
            || session.is_host
        {
            return;
        }

        self.send_broadcast_signal(
            session.remote_peer_id,
            DirectMessageKind::BroadcastReject,
            &session.session_id,
        );
        self.transition_broadcast_to_idle(Some("rejected".to_string()))
            .await;
    }

    pub(super) async fn handle_end_screen_broadcast(&mut self, session_id: String) {
        let Some(session) = self.active_broadcast.as_ref().cloned() else {
            return;
        };
        if session.session_id != session_id {
            return;
        }

        self.push_active_broadcast_state(&session, BroadcastPhase::Ending, None)
            .await;
        self.send_broadcast_signal(
            session.remote_peer_id,
            DirectMessageKind::BroadcastEnd,
            &session.session_id,
        );
        self.transition_broadcast_to_idle(Some("ended".to_string()))
            .await;
    }

    pub(super) async fn handle_send_screen_broadcast_chunk(
        &mut self,
        session_id: String,
        seq: u32,
        timestamp: i64,
        mime: String,
        codec: String,
        chunk_type: String,
        payload: Vec<u8>,
    ) {
        let Some(session_snapshot) = self.active_broadcast.as_ref().cloned() else {
            return;
        };
        if session_snapshot.session_id != session_id
            || session_snapshot.phase != ActiveBroadcastPhase::Active
            || !session_snapshot.is_host
        {
            return;
        }

        if !self.swarm.is_connected(&session_snapshot.remote_peer_id) {
            self.transition_broadcast_to_idle(Some("disconnected".to_string()))
                .await;
            return;
        }

        self.swarm
            .behaviour_mut()
            .broadcast
            .send_request(
                &session_snapshot.remote_peer_id,
                BroadcastFrameRequest {
                    session_id,
                    seq,
                    timestamp,
                    mime,
                    codec,
                    chunk_type: Self::parse_broadcast_chunk_type(&chunk_type),
                    payload,
                },
            );
    }

    pub(super) async fn handle_broadcast_signal(
        &mut self,
        peer: PeerId,
        request: &DirectMessageRequest,
    ) -> Result<(), String> {
        let incoming_chat_id = self
            .resolve_chat_id_for_sender(&request.sender_id, request.sender_alias.as_deref())
            .await;

        match request.msg_type {
            DirectMessageKind::BroadcastOffer => {
                if !matches!(
                    crate::chat_kind::parse_chat_kind(&incoming_chat_id),
                    crate::chat_kind::ChatKind::Direct
                ) {
                    self.send_broadcast_signal(peer, DirectMessageKind::BroadcastReject, &request.id);
                    return Ok(());
                }

                if self.active_broadcast.is_some() {
                    self.send_broadcast_signal(peer, DirectMessageKind::BroadcastBusy, &request.id);
                    return Ok(());
                }

                if self.broadcast_conflict_reason(&incoming_chat_id).is_some() {
                    self.send_broadcast_signal(peer, DirectMessageKind::BroadcastBusy, &request.id);
                    return Ok(());
                }

                let now = Self::now_unix_ts();
                let session = ActiveBroadcast {
                    session_id: request.id.clone(),
                    peer_chat_id: incoming_chat_id,
                    remote_peer_id: peer,
                    phase: ActiveBroadcastPhase::IncomingRinging,
                    ring_deadline: Some(
                        std::time::Instant::now()
                            + std::time::Duration::from_secs(BROADCAST_RING_TIMEOUT_SECS),
                    ),
                    ring_expires_at: Some(now + BROADCAST_RING_TIMEOUT_SECS as i64),
                    started_at: None,
                    is_host: false,
                };
                self.push_active_broadcast_state(&session, BroadcastPhase::IncomingRinging, None)
                    .await;
                self.active_broadcast = Some(session);
            }
            DirectMessageKind::BroadcastAccept => {
                let session_id = Self::broadcast_session_id_from_signal(request);
                let Some(session_snapshot) = self.active_broadcast.as_ref().cloned() else {
                    return Ok(());
                };
                if session_snapshot.session_id != session_id
                    || session_snapshot.phase != ActiveBroadcastPhase::OutgoingRinging
                    || !session_snapshot.is_host
                {
                    return Ok(());
                }

                let mut updated = session_snapshot;
                updated.phase = ActiveBroadcastPhase::Active;
                updated.ring_deadline = None;
                updated.ring_expires_at = None;
                updated.started_at = Some(Self::now_unix_ts());
                self.active_broadcast = Some(updated.clone());
                self.push_active_broadcast_state(&updated, BroadcastPhase::Active, None)
                    .await;
            }
            DirectMessageKind::BroadcastReject => {
                let session_id = Self::broadcast_session_id_from_signal(request);
                if let Some(session) = self.active_broadcast.as_ref() {
                    if session.session_id == session_id {
                        self.transition_broadcast_to_idle(Some("rejected".to_string()))
                            .await;
                    }
                }
            }
            DirectMessageKind::BroadcastBusy => {
                let session_id = Self::broadcast_session_id_from_signal(request);
                if let Some(session) = self.active_broadcast.as_ref() {
                    if session.session_id == session_id {
                        self.transition_broadcast_to_idle(Some("busy".to_string()))
                            .await;
                    }
                }
            }
            DirectMessageKind::BroadcastEnd => {
                let session_id = Self::broadcast_session_id_from_signal(request);
                if let Some(session) = self.active_broadcast.as_ref() {
                    if session.session_id == session_id {
                        self.transition_broadcast_to_idle(Some("ended_remote".to_string()))
                            .await;
                    }
                }
            }
            _ => {}
        }

        Ok(())
    }

    pub(super) async fn tick_broadcast(&mut self) {
        if let Some(session) = self.active_broadcast.as_ref() {
            if let Some(deadline) = session.ring_deadline {
                if std::time::Instant::now() >= deadline {
                    let session_id = session.session_id.clone();
                    let peer = session.remote_peer_id;
                    self.send_broadcast_signal(peer, DirectMessageKind::BroadcastEnd, &session_id);
                    self.transition_broadcast_to_idle(Some("ring_timeout".to_string()))
                        .await;
                }
            }
        }
    }

    pub(super) async fn handle_peer_disconnect_for_broadcast(&mut self, peer_id: &PeerId) {
        if let Some(session) = self.active_broadcast.as_ref() {
            if &session.remote_peer_id == peer_id {
                self.transition_broadcast_to_idle(Some("disconnected".to_string()))
                    .await;
            }
        }
    }

    pub(super) async fn handle_broadcast_frame_event(
        &mut self,
        event: request_response::Event<BroadcastFrameRequest, BroadcastFrameResponse>,
    ) {
        use request_response::{Event, Message};

        match event {
            Event::Message { peer, message, .. } => match message {
                Message::Request {
                    request, channel, ..
                } => {
                    let mut accepted = false;
                    if let Some(session) = self.active_broadcast.as_ref() {
                        if session.phase == ActiveBroadcastPhase::Active
                            && session.session_id == request.session_id
                            && session.remote_peer_id == peer
                            && !session.is_host
                        {
                            let frame_event = BroadcastFrameEvent {
                                session_id: request.session_id,
                                peer_id: peer.to_string(),
                                seq: request.seq,
                                timestamp: request.timestamp,
                                mime: request.mime,
                                codec: request.codec,
                                chunk_type: request.chunk_type,
                                payload: request.payload,
                            };
                            let _ = self.app_handle.emit("broadcast-frame", frame_event);
                            accepted = true;
                        }
                    }
                    let _ = self
                        .swarm
                        .behaviour_mut()
                        .broadcast
                        .send_response(channel, BroadcastFrameResponse { ok: accepted });
                }
                Message::Response { response, .. } => {
                    if !response.ok {
                        let should_end = self
                            .active_broadcast
                            .as_ref()
                            .map(|session| {
                                session.phase == ActiveBroadcastPhase::Active
                                    && session.remote_peer_id == peer
                                    && session.is_host
                            })
                            .unwrap_or(false);
                        if should_end {
                            self.transition_broadcast_to_idle(Some("stream_rejected".to_string()))
                                .await;
                        }
                    }
                }
            },
            Event::OutboundFailure { peer, error, .. } => {
                eprintln!("[Broadcast] Outbound frame failure to {}: {:?}", peer, error);
                let should_end = self
                    .active_broadcast
                    .as_ref()
                    .map(|session| {
                        session.phase == ActiveBroadcastPhase::Active
                            && session.remote_peer_id == peer
                    })
                    .unwrap_or(false);
                if should_end {
                    self.transition_broadcast_to_idle(Some("stream_failure".to_string()))
                        .await;
                }
            }
            Event::InboundFailure { peer, error, .. } => {
                eprintln!("[Broadcast] Inbound frame failure from {}: {:?}", peer, error);
                let should_end = self
                    .active_broadcast
                    .as_ref()
                    .map(|session| {
                        session.phase == ActiveBroadcastPhase::Active
                            && session.remote_peer_id == peer
                    })
                    .unwrap_or(false);
                if should_end {
                    self.transition_broadcast_to_idle(Some("stream_failure".to_string()))
                        .await;
                }
            }
            Event::ResponseSent { .. } => {}
        }
    }
}
