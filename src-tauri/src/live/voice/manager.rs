use super::*;
use crate::app_state::{CallKind, VoiceCallPhase, VoiceCallState};
use crate::live::voice::protocol::{
    read_voice_stream_frame, read_voice_stream_header, write_voice_stream_frame,
    write_voice_stream_header, VoiceFrameRequest,
};
use crate::network::direct_message::{DirectMessageKind, DirectMessageRequest};
use futures::AsyncWriteExt as _;

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
        if let Some(call) = self.active_call.as_ref() {
            if call.kind == CallKind::Voice {
                let peer = call.remote_peer_id;
                self.log_voice_network_summary("final", &peer);
            }
        }
        self.voice_audio_engine = None;
        self.voice_capture_rx = None;
        self.voice_stream_tx = None;
        self.voice_stream_call_id = None;
        if let Some(handle) = self.voice_stream_writer_handle.take() {
            handle.abort();
        }
        self.voice_next_seq = 0;
        self.voice_jitter_buffer.reset();
        self.voice_opus_encoder = None;
        self.voice_opus_decoder = None;
        self.reset_voice_network_diagnostics();
    }

    pub(super) fn start_voice_audio(&mut self) -> Result<(), String> {
        let encoder = crate::live::voice::codec::VoiceOpusEncoder::new()
            .map_err(|e| format!("failed to start Opus encoder: {}", e))?;
        let decoder = crate::live::voice::codec::VoiceOpusDecoder::new()
            .map_err(|e| format!("failed to start Opus decoder: {}", e))?;
        let (engine, capture_rx) = crate::live::voice::voice::VoiceAudioEngine::start()?;
        self.voice_audio_engine = Some(engine);
        self.voice_capture_rx = Some(capture_rx);
        self.voice_opus_encoder = Some(encoder);
        self.voice_opus_decoder = Some(decoder);
        self.voice_next_seq = 0;
        self.voice_jitter_buffer.reset();
        self.reset_voice_network_diagnostics();
        Ok(())
    }

    pub(super) fn start_voice_stream_writer(&mut self, peer: PeerId, call_id: String) -> bool {
        if self.voice_stream_tx.is_some()
            && self.voice_stream_call_id.as_deref() == Some(call_id.as_str())
        {
            return true;
        }

        let Some(connection_id) = self.voice_quic_connection_id(&peer) else {
            eprintln!(
                "[Voice][QUIC] No QUIC connection id available for voice stream: peer={}",
                peer
            );
            return false;
        };

        self.voice_stream_tx = None;
        self.voice_stream_call_id = None;
        if let Some(handle) = self.voice_stream_writer_handle.take() {
            handle.abort();
        }

        eprintln!(
            "[Voice][Stream] selected outbound QUIC connection peer={} call_id={} connection_id={:?}",
            peer, call_id, connection_id
        );

        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<VoiceFrameRequest>();
        let stream_rx = match self
            .swarm
            .behaviour_mut()
            .voice_call
            .open_stream_on_connection(peer, connection_id)
        {
            Ok(stream_rx) => stream_rx,
            Err(e) => {
                eprintln!(
                    "[Voice][QUIC] Failed to queue voice stream on {} for {}: {}",
                    connection_id, peer, e
                );
                return false;
            }
        };
        let event_tx = self.voice_stream_event_tx.clone();
        let writer_call_id = call_id.clone();
        let handle = tauri::async_runtime::spawn(async move {
            let mut stream = match tokio::time::timeout(
                std::time::Duration::from_secs(5),
                stream_rx,
            )
            .await
            {
                Ok(Ok(Ok(stream))) => {
                    eprintln!(
                        "[Voice][Stream] outbound stream opened peer={} call_id={} connection_id={:?}",
                        peer, writer_call_id, connection_id
                    );
                    stream
                }
                Ok(Ok(Err(e))) => {
                    let _ = event_tx
                        .send(VoiceStreamEvent::OutboundFailure {
                            peer,
                            call_id: writer_call_id.clone(),
                            error: e.to_string(),
                        })
                        .await;
                    return;
                }
                Ok(Err(_)) => {
                    let _ = event_tx
                        .send(VoiceStreamEvent::OutboundFailure {
                            peer,
                            call_id: writer_call_id.clone(),
                            error: "stream open canceled".to_string(),
                        })
                        .await;
                    return;
                }
                Err(e) => {
                    let _ = event_tx
                        .send(VoiceStreamEvent::OutboundFailure {
                            peer,
                            call_id: writer_call_id.clone(),
                            error: format!("stream open timed out: {}", e),
                        })
                        .await;
                    return;
                }
            };

            if let Err(e) = write_voice_stream_header(&mut stream, &writer_call_id).await {
                let _ = event_tx
                    .send(VoiceStreamEvent::OutboundFailure {
                        peer,
                        call_id: writer_call_id.clone(),
                        error: e.to_string(),
                    })
                    .await;
                return;
            }
            eprintln!(
                "[Voice][Stream] outbound header written peer={} call_id={} connection_id={:?}",
                peer, writer_call_id, connection_id
            );

            let mut first_frame_written = false;
            while let Some(frame) = rx.recv().await {
                if let Err(e) = write_voice_stream_frame(
                    &mut stream,
                    frame.seq,
                    frame.timestamp,
                    &frame.payload,
                )
                .await
                {
                    let _ = event_tx
                        .send(VoiceStreamEvent::OutboundFailure {
                            peer,
                            call_id: frame.call_id.clone(),
                            error: e.to_string(),
                        })
                        .await;
                    return;
                }
                if !first_frame_written {
                    eprintln!(
                        "[Voice][Stream] outbound first frame written peer={} call_id={} seq={} bytes={} connection_id={:?}",
                        peer,
                        frame.call_id,
                        frame.seq,
                        frame.payload.len(),
                        connection_id
                    );
                    first_frame_written = true;
                }
            }

            let _ = stream.close().await;
        });

        self.voice_stream_tx = Some(tx);
        self.voice_stream_call_id = Some(call_id);
        self.voice_stream_writer_handle = Some(handle);
        true
    }

    pub(super) async fn transition_to_idle(&mut self, reason: Option<String>) {
        self.stop_video_media();
        self.stop_voice_audio();
        self.active_call = None;
        self.push_idle_call_state(reason).await;
    }

    pub(super) fn send_call_signal(
        &mut self,
        peer: PeerId,
        kind: DirectMessageKind,
        call_id: &str,
    ) {
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
        if let Some(session) = self.active_broadcast.as_ref() {
            if session.phase != ActiveBroadcastPhase::Active || session.peer_chat_id != peer_chat_id
            {
                self.push_idle_call_state(Some("broadcast_conflict".to_string()))
                    .await;
                return;
            }
        }

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
        if !self.ensure_voice_quic_path(&peer_id) {
            self.push_idle_call_state(Some("quic_required".to_string()))
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
            ring_deadline: Some(
                std::time::Instant::now() + std::time::Duration::from_secs(CALL_RING_TIMEOUT_SECS),
            ),
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

        if !self.ensure_voice_quic_path(&call_snapshot.remote_peer_id) {
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
        let _ = self.start_voice_stream_writer(updated.remote_peer_id, updated.call_id.clone());
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
        self.send_call_signal(
            call.remote_peer_id,
            DirectMessageKind::CallReject,
            &call.call_id,
        );
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
        self.send_call_signal(
            call.remote_peer_id,
            DirectMessageKind::CallEnd,
            &call.call_id,
        );
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
        let incoming_chat_id = self
            .resolve_chat_id_for_sender(&request.sender_id, request.sender_alias.as_deref())
            .await;

        match request.msg_type {
            DirectMessageKind::CallOffer | DirectMessageKind::CallOfferVideo => {
                if !matches!(
                    crate::chat_kind::parse_chat_kind(&incoming_chat_id),
                    crate::chat_kind::ChatKind::Direct
                ) {
                    self.send_call_signal(peer, DirectMessageKind::CallReject, &request.id);
                    return Ok(());
                }
                if request.msg_type == DirectMessageKind::CallOfferVideo {
                    let upgrade_call_id = Self::call_id_from_signal(request);
                    if let Some(existing) = self.active_call.as_ref().cloned() {
                        let same_active_voice = existing.phase == ActiveCallPhase::Active
                            && existing.kind == CallKind::Voice
                            && existing.remote_peer_id == peer
                            && existing.call_id == upgrade_call_id;
                        if same_active_voice {
                            if !self.peer_has_quic_path(&peer) {
                                self.send_call_signal(
                                    peer,
                                    DirectMessageKind::CallReject,
                                    &upgrade_call_id,
                                );
                                return Ok(());
                            }
                            self.send_call_signal(
                                peer,
                                DirectMessageKind::CallAcceptVideo,
                                &upgrade_call_id,
                            );
                            let mut updated = existing;
                            updated.kind = CallKind::Video;
                            updated.camera_enabled = false;
                            self.active_call = Some(updated.clone());
                            self.start_video_media(peer, updated.call_id.clone(), false);
                            self.push_active_call_state(&updated, VoiceCallPhase::Active, None)
                                .await;
                            return Ok(());
                        }
                    }
                }
                if request.msg_type == DirectMessageKind::CallOfferVideo
                    && !self.peer_has_quic_path(&peer)
                {
                    self.send_call_signal(peer, DirectMessageKind::CallReject, &request.id);
                    return Ok(());
                }
                if request.msg_type == DirectMessageKind::CallOffer
                    && !self.ensure_voice_quic_path(&peer)
                {
                    self.send_call_signal(peer, DirectMessageKind::CallReject, &request.id);
                    return Ok(());
                }
                if request.msg_type == DirectMessageKind::CallOfferVideo
                    && self.active_broadcast.is_some()
                {
                    self.send_call_signal(peer, DirectMessageKind::CallBusy, &request.id);
                    return Ok(());
                }
                if request.msg_type == DirectMessageKind::CallOffer {
                    if let Some(session) = self.active_broadcast.as_ref() {
                        let same_peer_active = session.phase == ActiveBroadcastPhase::Active
                            && session.peer_chat_id == incoming_chat_id;
                        if !same_peer_active {
                            self.send_call_signal(peer, DirectMessageKind::CallBusy, &request.id);
                            return Ok(());
                        }
                    }
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
                    ring_deadline: Some(
                        std::time::Instant::now()
                            + std::time::Duration::from_secs(CALL_RING_TIMEOUT_SECS),
                    ),
                    ring_expires_at: Some(now + CALL_RING_TIMEOUT_SECS as i64),
                    started_at: None,
                    muted: false,
                    camera_enabled: false,
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
                if call_snapshot.call_id == call_id
                    && call_snapshot.phase == ActiveCallPhase::Active
                    && call_snapshot.kind == expected_kind
                {
                    if expected_kind == CallKind::Video {
                        self.start_video_media(
                            peer,
                            call_snapshot.call_id.clone(),
                            call_snapshot.camera_enabled,
                        );
                    }
                    return Ok(());
                }
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
                if expected_kind == CallKind::Voice && !self.ensure_voice_quic_path(&peer) {
                    self.send_call_signal(peer, DirectMessageKind::CallEnd, &call_snapshot.call_id);
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
                let _ = self.start_voice_stream_writer(peer, updated.call_id.clone());
                if updated.kind == CallKind::Video {
                    self.start_video_media(peer, updated.call_id.clone(), updated.camera_enabled);
                }
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
                        self.transition_to_idle(Some("ended_remote".to_string()))
                            .await;
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
                    self.transition_to_idle(Some("ring_timeout".to_string()))
                        .await;
                    return;
                }
            }
        }

        let (call_id, peer, muted, phase, kind) = match self.active_call.as_ref() {
            Some(c) => (
                c.call_id.clone(),
                c.remote_peer_id,
                c.muted,
                c.phase,
                c.kind.clone(),
            ),
            None => return,
        };
        if phase != ActiveCallPhase::Active {
            return;
        }
        if kind == CallKind::Voice && !self.peer_has_quic_path(&peer) {
            self.transition_to_idle(Some("quic_path_lost".to_string()))
                .await;
            return;
        }

        if self
            .voice_last_summary_at
            .map(|last| last.elapsed() >= std::time::Duration::from_secs(5))
            .unwrap_or(true)
        {
            self.log_voice_network_summary("summary", &peer);
            self.voice_last_summary_at = Some(std::time::Instant::now());
        }

        if self.voice_stream_tx.is_none() {
            let _ = self.start_voice_stream_writer(peer, call_id.clone());
        }

        let Some(voice_stream_tx) = self.voice_stream_tx.as_ref().cloned() else {
            return;
        };

        loop {
            let frame = match self.voice_capture_rx.as_mut() {
                Some(capture_rx) => match capture_rx.try_recv() {
                    Ok(frame) => frame,
                    Err(tokio::sync::mpsc::error::TryRecvError::Empty) => break,
                    Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => break,
                },
                None => return,
            };

            if muted {
                continue;
            }

            let payload = match self.voice_opus_encoder.as_mut() {
                Some(encoder) => match encoder.encode_frame(&frame) {
                    Ok(packet) => packet,
                    Err(_) => {
                        self.voice_network_stats.opus_encode_errors += 1;
                        continue;
                    }
                },
                None => {
                    self.voice_network_stats.opus_encode_errors += 1;
                    break;
                }
            };

            let req = VoiceFrameRequest {
                call_id: call_id.clone(),
                seq: self.voice_next_seq,
                timestamp: Self::now_unix_ts(),
                payload,
            };
            self.voice_next_seq = self.voice_next_seq.wrapping_add(1);
            self.voice_network_stats.outbound_frames += 1;
            self.voice_network_stats.opus_out_bytes += req.payload.len() as u64;
            if voice_stream_tx.send(req).is_err() {
                self.voice_network_stats.outbound_failures += 1;
                if self.voice_stream_call_id.as_deref() == Some(call_id.as_str()) {
                    self.voice_stream_tx = None;
                    self.voice_stream_call_id = None;
                    self.voice_stream_writer_handle = None;
                }
                break;
            }
        }
    }

    pub(super) async fn handle_peer_disconnect_for_voice_call(&mut self, peer_id: &PeerId) {
        if let Some(call) = self.active_call.as_ref() {
            if &call.remote_peer_id == peer_id {
                self.transition_to_idle(Some("disconnected".to_string()))
                    .await;
            }
        }
    }

    pub(super) async fn handle_voice_stream_event(&mut self, event: VoiceStreamEvent) {
        match event {
            VoiceStreamEvent::InboundFrame {
                peer,
                call_id,
                seq,
                payload,
            } => {
                if let Some(call) = self.active_call.as_ref() {
                    if call.phase == ActiveCallPhase::Active
                        && matches!(call.kind, CallKind::Voice | CallKind::Video)
                        && call.call_id == call_id
                        && call.remote_peer_id == peer
                    {
                        self.voice_network_stats.inbound_frames += 1;
                        self.voice_network_stats.opus_in_bytes += payload.len() as u64;
                        if let Some(expected) = self.voice_expected_inbound_seq {
                            if seq != expected {
                                self.voice_network_stats.inbound_seq_gaps += 1;
                                if seq < expected {
                                    self.voice_network_stats.inbound_out_of_order_frames += 1;
                                }
                            }
                        }
                        self.voice_expected_inbound_seq = Some(seq.wrapping_add(1));
                        let pcm = match self.voice_opus_decoder.as_mut() {
                            Some(decoder) => match decoder.decode_packet(&payload) {
                                Ok(pcm) => pcm,
                                Err(_) => {
                                    self.voice_network_stats.opus_decode_errors += 1;
                                    return;
                                }
                            },
                            None => {
                                self.voice_network_stats.opus_decode_errors += 1;
                                return;
                            }
                        };
                        let ordered_frames = self.voice_jitter_buffer.push(seq, pcm);
                        if let Some(engine) = self.voice_audio_engine.as_ref() {
                            for frame in ordered_frames {
                                engine.push_remote_frame(frame);
                            }
                        }
                    }
                }
            }
            VoiceStreamEvent::InboundFailure {
                peer,
                call_id,
                error,
            } => {
                eprintln!("[Voice] Inbound stream failure from {}: {}", peer, error);
                self.voice_network_stats.inbound_failures += 1;
                if self
                    .active_call
                    .as_ref()
                    .map(|call| {
                        call.phase == ActiveCallPhase::Active
                            && call.remote_peer_id == peer
                            && call_id.as_deref() == Some(call.call_id.as_str())
                    })
                    .unwrap_or(false)
                {
                    self.transition_to_idle(Some("stream_failure".to_string()))
                        .await;
                }
            }
            VoiceStreamEvent::OutboundFailure {
                peer,
                call_id,
                error,
            } => {
                eprintln!("[Voice] Outbound stream failure to {}: {}", peer, error);
                self.voice_network_stats.outbound_failures += 1;
                if self.voice_stream_call_id.as_deref() == Some(call_id.as_str()) {
                    self.voice_stream_tx = None;
                    self.voice_stream_call_id = None;
                    self.voice_stream_writer_handle = None;
                }
                if self
                    .active_call
                    .as_ref()
                    .map(|call| {
                        call.phase == ActiveCallPhase::Active
                            && call.remote_peer_id == peer
                            && call.call_id == call_id
                    })
                    .unwrap_or(false)
                {
                    self.transition_to_idle(Some("stream_failure".to_string()))
                        .await;
                }
            }
        }
    }
}

pub(super) fn start_voice_stream_accept_loop(
    incoming: crate::network::voice_stream::IncomingStreams,
    event_tx: tokio::sync::mpsc::Sender<VoiceStreamEvent>,
) {
    tauri::async_runtime::spawn(async move {
        futures::pin_mut!(incoming);
        while let Some((peer, mut stream)) = incoming.next().await {
            let event_tx = event_tx.clone();
            tauri::async_runtime::spawn(async move {
                eprintln!("[Voice][Stream] inbound stream accepted peer={}", peer);
                let call_id = match read_voice_stream_header(&mut stream).await {
                    Ok(call_id) => call_id,
                    Err(e) => {
                        let _ = event_tx
                            .send(VoiceStreamEvent::InboundFailure {
                                peer,
                                call_id: None,
                                error: e.to_string(),
                            })
                            .await;
                        return;
                    }
                };
                eprintln!(
                    "[Voice][Stream] inbound header read peer={} call_id={}",
                    peer, call_id
                );

                let mut first_frame_read = false;
                loop {
                    match read_voice_stream_frame(&mut stream).await {
                        Ok(frame) => {
                            if !first_frame_read {
                                eprintln!(
                                    "[Voice][Stream] inbound first frame read peer={} call_id={} seq={} bytes={}",
                                    peer,
                                    call_id,
                                    frame.seq,
                                    frame.payload.len()
                                );
                                first_frame_read = true;
                            }
                            if event_tx
                                .send(VoiceStreamEvent::InboundFrame {
                                    peer,
                                    call_id: call_id.clone(),
                                    seq: frame.seq,
                                    payload: frame.payload,
                                })
                                .await
                                .is_err()
                            {
                                return;
                            }
                        }
                        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return,
                        Err(e) => {
                            let _ = event_tx
                                .send(VoiceStreamEvent::InboundFailure {
                                    peer,
                                    call_id: Some(call_id.clone()),
                                    error: e.to_string(),
                                })
                                .await;
                            return;
                        }
                    }
                }
            });
        }
    });
}
