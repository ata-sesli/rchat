use super::*;
use crate::app_state::{CallKind, VoiceCallPhase, VoiceCallState};
use crate::live::voice::protocol::{VoiceFrameRequest, VoiceFrameResponse};
use crate::network::direct_message::{DirectMessageKind, DirectMessageRequest};
use libp2p::request_response;

const CALL_RING_TIMEOUT_SECS: u64 = 30;

impl NetworkManager {
    pub(super) fn now_unix_ts() -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0)
    }

    pub(super) fn call_id_from_signal(request: &DirectMessageRequest) -> String {
        request
            .text_content
            .clone()
            .filter(|v| !v.trim().is_empty())
            .unwrap_or_else(|| request.id.clone())
    }

    fn encode_pcm16_le(samples: &[i16]) -> Vec<u8> {
        let mut out = Vec::with_capacity(samples.len() * 2);
        for s in samples {
            out.extend_from_slice(&s.to_le_bytes());
        }
        out
    }

    fn decode_pcm16_le(payload: &[u8]) -> Vec<i16> {
        payload
            .chunks_exact(2)
            .map(|c| i16::from_le_bytes([c[0], c[1]]))
            .collect()
    }

    pub(super) async fn push_idle_call_state(&mut self, reason: Option<String>) {
        self.set_voice_call_state(VoiceCallState::default(), reason)
            .await;
    }

    pub(super) async fn push_active_call_state(
        &mut self,
        call: &ActiveCall,
        phase: VoiceCallPhase,
        reason: Option<String>,
    ) {
        self.set_voice_call_state(
            VoiceCallState {
                phase,
                call_kind: Some(call.kind.clone()),
                call_id: Some(call.call_id.clone()),
                peer_id: Some(call.peer_chat_id.clone()),
                started_at: call.started_at,
                ring_expires_at: call.ring_expires_at,
                muted: call.muted,
                camera_enabled: call.camera_enabled,
                reason: None,
            },
            reason,
        )
        .await;
    }

    pub(super) fn stop_voice_audio(&mut self) {
        self.voice_audio_engine = None;
        self.voice_capture_rx = None;
        self.voice_next_seq = 0;
        self.voice_jitter_buffer.reset();
    }

    pub(super) fn start_voice_audio(&mut self) -> Result<(), String> {
        let (engine, capture_rx) = crate::live::voice::voice::VoiceAudioEngine::start()?;
        self.voice_audio_engine = Some(engine);
        self.voice_capture_rx = Some(capture_rx);
        self.voice_next_seq = 0;
        self.voice_jitter_buffer.reset();
        Ok(())
    }

    pub(super) async fn transition_to_idle(&mut self, reason: Option<String>) {
        self.active_call = None;
        self.stop_voice_audio();
        self.push_idle_call_state(reason).await;
    }

    pub(super) fn send_call_signal(&mut self, peer: PeerId, kind: DirectMessageKind, call_id: &str) {
        let now = Self::now_unix_ts();
        let req = DirectMessageRequest {
            id: format!("call-signal-{}-{}", kind.as_str(), now),
            sender_id: self.swarm.local_peer_id().to_string(),
            msg_type: kind,
            text_content: Some(call_id.to_string()),
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

    pub(super) async fn handle_start_voice_call(&mut self, peer_chat_id: String) {
        if self.active_call.is_some() {
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

        let Some(peer_id) = self.resolve_peer_id(&peer_chat_id, "VOICE_CALL").await else {
            self.push_idle_call_state(Some("peer_unresolved".to_string()))
                .await;
            return;
        };

        if !self.swarm.is_connected(&peer_id) {
            self.push_idle_call_state(Some("peer_not_connected".to_string()))
                .await;
            return;
        }

        let now = Self::now_unix_ts();
        let call_id = format!("call-{}-{}", now, rand::random::<u32>());
        let call = ActiveCall {
            call_id: call_id.clone(),
            kind: CallKind::Voice,
            peer_chat_id,
            remote_peer_id: peer_id,
            phase: ActiveCallPhase::OutgoingRinging,
            ring_deadline: Some(std::time::Instant::now() + std::time::Duration::from_secs(CALL_RING_TIMEOUT_SECS)),
            ring_expires_at: Some(now + CALL_RING_TIMEOUT_SECS as i64),
            started_at: None,
            muted: false,
            camera_enabled: false,
        };

        let offer = DirectMessageRequest {
            id: call_id.clone(),
            sender_id: self.swarm.local_peer_id().to_string(),
            msg_type: DirectMessageKind::CallOffer,
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

    pub(super) async fn handle_accept_voice_call(&mut self, call_id: String) {
        let Some(call_snapshot) = self.active_call.as_ref().cloned() else {
            return;
        };
        if call_snapshot.call_id != call_id
            || call_snapshot.phase != ActiveCallPhase::IncomingRinging
            || call_snapshot.kind != CallKind::Voice
        {
            return;
        }

        self.send_call_signal(
            call_snapshot.remote_peer_id,
            DirectMessageKind::CallAccept,
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

    pub(super) async fn handle_reject_voice_call(&mut self, call_id: String) {
        let Some(call) = self.active_call.as_ref().cloned() else {
            return;
        };
        if call.call_id != call_id
            || call.phase != ActiveCallPhase::IncomingRinging
            || call.kind != CallKind::Voice
        {
            return;
        }
        self.send_call_signal(call.remote_peer_id, DirectMessageKind::CallReject, &call.call_id);
        self.transition_to_idle(Some("rejected".to_string())).await;
    }

    pub(super) async fn handle_end_voice_call(&mut self, call_id: String) {
        let Some(call) = self.active_call.as_ref().cloned() else {
            return;
        };
        if call.call_id != call_id || call.kind != CallKind::Voice {
            return;
        }
        self.push_active_call_state(&call, VoiceCallPhase::Ending, None)
            .await;
        self.send_call_signal(call.remote_peer_id, DirectMessageKind::CallEnd, &call.call_id);
        self.transition_to_idle(Some("ended".to_string())).await;
    }

    pub(super) async fn handle_set_voice_call_muted(&mut self, call_id: String, muted: bool) {
        let Some(call_snapshot) = self.active_call.as_ref().cloned() else {
            return;
        };
        if call_snapshot.call_id != call_id
            || call_snapshot.phase != ActiveCallPhase::Active
            || call_snapshot.kind != CallKind::Voice
        {
            return;
        }
        let mut updated = call_snapshot;
        updated.muted = muted;
        self.active_call = Some(updated.clone());
        self.push_active_call_state(&updated, VoiceCallPhase::Active, None)
            .await;
    }

    pub(super) async fn handle_call_signal(
        &mut self,
        peer: PeerId,
        request: &DirectMessageRequest,
    ) -> Result<(), String> {
        let incoming_chat_id = self.resolve_chat_id_for_sender(&request.sender_id).await;

        match request.msg_type {
            DirectMessageKind::CallOffer | DirectMessageKind::CallOfferVideo => {
                if !matches!(
                    crate::chat_kind::parse_chat_kind(&incoming_chat_id),
                    crate::chat_kind::ChatKind::Direct
                ) {
                    self.send_call_signal(peer, DirectMessageKind::CallReject, &request.id);
                    return Ok(());
                }
                if request.msg_type == DirectMessageKind::CallOfferVideo
                    && !self.peer_has_quic_path(&peer)
                {
                    self.send_call_signal(peer, DirectMessageKind::CallReject, &request.id);
                    return Ok(());
                }

                if self.active_call.is_some() {
                    self.send_call_signal(peer, DirectMessageKind::CallBusy, &request.id);
                    return Ok(());
                }

                let now = Self::now_unix_ts();
                let call = ActiveCall {
                    call_id: request.id.clone(),
                    kind: if request.msg_type == DirectMessageKind::CallOfferVideo {
                        CallKind::Video
                    } else {
                        CallKind::Voice
                    },
                    peer_chat_id: incoming_chat_id,
                    remote_peer_id: peer,
                    phase: ActiveCallPhase::IncomingRinging,
                    ring_deadline: Some(std::time::Instant::now() + std::time::Duration::from_secs(CALL_RING_TIMEOUT_SECS)),
                    ring_expires_at: Some(now + CALL_RING_TIMEOUT_SECS as i64),
                    started_at: None,
                    muted: false,
                    camera_enabled: request.msg_type == DirectMessageKind::CallOfferVideo,
                };
                self.push_active_call_state(&call, VoiceCallPhase::IncomingRinging, None)
                    .await;
                self.active_call = Some(call);
            }
            DirectMessageKind::CallAccept | DirectMessageKind::CallAcceptVideo => {
                let call_id = Self::call_id_from_signal(request);
                let Some(call_snapshot) = self.active_call.as_ref().cloned() else {
                    return Ok(());
                };
                let expected_kind = if request.msg_type == DirectMessageKind::CallAcceptVideo {
                    CallKind::Video
                } else {
                    CallKind::Voice
                };
                if call_snapshot.call_id != call_id
                    || call_snapshot.phase != ActiveCallPhase::OutgoingRinging
                    || call_snapshot.kind != expected_kind
                {
                    return Ok(());
                }
                if expected_kind == CallKind::Video && !self.peer_has_quic_path(&peer) {
                    self.transition_to_idle(Some("quic_required".to_string()))
                        .await;
                    return Ok(());
                }
                if let Err(e) = self.start_voice_audio() {
                    self.send_call_signal(peer, DirectMessageKind::CallEnd, &call_snapshot.call_id);
                    self.transition_to_idle(Some(format!("audio_start_failed: {}", e)))
                        .await;
                    return Ok(());
                }
                let mut updated = call_snapshot;
                updated.phase = ActiveCallPhase::Active;
                updated.ring_deadline = None;
                updated.ring_expires_at = None;
                updated.started_at = Some(Self::now_unix_ts());
                self.active_call = Some(updated.clone());
                self.push_active_call_state(&updated, VoiceCallPhase::Active, None)
                    .await;
            }
            DirectMessageKind::CallReject => {
                let call_id = Self::call_id_from_signal(request);
                if let Some(call) = self.active_call.as_ref() {
                    if call.call_id == call_id {
                        self.transition_to_idle(Some("rejected".to_string())).await;
                    }
                }
            }
            DirectMessageKind::CallBusy => {
                let call_id = Self::call_id_from_signal(request);
                if let Some(call) = self.active_call.as_ref() {
                    if call.call_id == call_id {
                        self.transition_to_idle(Some("busy".to_string())).await;
                    }
                }
            }
            DirectMessageKind::CallEnd => {
                let call_id = Self::call_id_from_signal(request);
                if let Some(call) = self.active_call.as_ref() {
                    if call.call_id == call_id {
                        self.transition_to_idle(Some("ended_remote".to_string())).await;
                    }
                }
            }
            _ => {}
        }

        Ok(())
    }

    pub(super) async fn tick_voice_call(&mut self) {
        if let Some(call) = self.active_call.as_ref() {
            if let Some(deadline) = call.ring_deadline {
                if std::time::Instant::now() >= deadline {
                    let call_id = call.call_id.clone();
                    let peer = call.remote_peer_id;
                    self.send_call_signal(peer, DirectMessageKind::CallEnd, &call_id);
                    self.transition_to_idle(Some("ring_timeout".to_string())).await;
                    return;
                }
            }
        }

        let (call_id, peer, muted, phase) = match self.active_call.as_ref() {
            Some(c) => (c.call_id.clone(), c.remote_peer_id, c.muted, c.phase),
            None => return,
        };
        if phase != ActiveCallPhase::Active {
            return;
        }

        let Some(capture_rx) = self.voice_capture_rx.as_mut() else {
            return;
        };

        loop {
            match capture_rx.try_recv() {
                Ok(frame) => {
                    if muted {
                        continue;
                    }
                    let req = VoiceFrameRequest {
                        call_id: call_id.clone(),
                        seq: self.voice_next_seq,
                        timestamp: Self::now_unix_ts(),
                        payload: Self::encode_pcm16_le(&frame),
                    };
                    self.voice_next_seq = self.voice_next_seq.wrapping_add(1);
                    self.swarm
                        .behaviour_mut()
                        .voice_call
                        .send_request(&peer, req);
                }
                Err(tokio::sync::mpsc::error::TryRecvError::Empty) => break,
                Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => break,
            }
        }
    }

    pub(super) async fn handle_peer_disconnect_for_voice_call(&mut self, peer_id: &PeerId) {
        if let Some(call) = self.active_call.as_ref() {
            if &call.remote_peer_id == peer_id {
                self.transition_to_idle(Some("disconnected".to_string())).await;
            }
        }
    }

    pub(super) async fn handle_voice_frame_event(
        &mut self,
        event: request_response::Event<VoiceFrameRequest, VoiceFrameResponse>,
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
                            && call.call_id == request.call_id
                            && call.remote_peer_id == peer
                        {
                            let pcm = Self::decode_pcm16_le(&request.payload);
                            let ordered_frames = self.voice_jitter_buffer.push(request.seq, pcm);
                            if let Some(engine) = self.voice_audio_engine.as_ref() {
                                for frame in ordered_frames {
                                    engine.push_remote_frame(frame);
                                }
                                accepted = true;
                            }
                        }
                    }
                    let _ = self
                        .swarm
                        .behaviour_mut()
                        .voice_call
                        .send_response(channel, VoiceFrameResponse { ok: accepted });
                }
                Message::Response { response, .. } => {
                    if !response.ok {
                        let should_end = self
                            .active_call
                            .as_ref()
                            .map(|call| call.phase == ActiveCallPhase::Active && call.remote_peer_id == peer)
                            .unwrap_or(false);
                        if should_end {
                            self.transition_to_idle(Some("stream_rejected".to_string()))
                                .await;
                        }
                    }
                }
            },
            Event::OutboundFailure { peer, error, .. } => {
                eprintln!("[Voice] Outbound frame failure to {}: {:?}", peer, error);
                let should_end = self
                    .active_call
                    .as_ref()
                    .map(|call| call.phase == ActiveCallPhase::Active && call.remote_peer_id == peer)
                    .unwrap_or(false);
                if should_end {
                    self.transition_to_idle(Some("stream_failure".to_string()))
                        .await;
                }
            }
            Event::InboundFailure { peer, error, .. } => {
                eprintln!("[Voice] Inbound frame failure from {}: {:?}", peer, error);
                let should_end = self
                    .active_call
                    .as_ref()
                    .map(|call| call.phase == ActiveCallPhase::Active && call.remote_peer_id == peer)
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
