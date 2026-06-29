use super::*;
use crate::app_state::{BroadcastPhase, CallKind};
use crate::live::broadcast::protocol::{
    BroadcastChunkType, BroadcastFrameEvent, BroadcastFrameRequest, BroadcastFrameResponse,
};
use crate::network::direct_message::{DirectMessageKind, DirectMessageRequest};
use libp2p::request_response;
use serde::Serialize;
use std::time::{Duration, Instant};

const BROADCAST_RING_TIMEOUT_SECS: u64 = 30;
const SCREEN_BROADCAST_FPS: u32 = 15;
const SCREEN_BROADCAST_BITRATE_KBPS: u32 = 1_500;
const SCREEN_BROADCAST_ENCODER_THREADS: u32 = 4;
const SCREEN_BROADCAST_ENCODER_CPU_USED: i32 = 8;
const SCREEN_BROADCAST_KEYFRAME_INTERVAL_FRAMES: u32 = 30;
const SCREEN_BROADCAST_PROFILE: &str = "720p15";
const SCREEN_BROADCAST_MIME: &str = "video/webm;codecs=vp8";
const SCREEN_BROADCAST_CODEC: &str = "vp8";
const SCREEN_BROADCAST_SUMMARY_INTERVAL: Duration = Duration::from_secs(5);

pub(super) struct ScreenCaptureStartTask {
    session_id: String,
    result_rx: tokio::sync::oneshot::Receiver<
        Result<
            rchat_screen_capture::ScreenCaptureSession,
            rchat_screen_capture::ScreenCaptureError,
        >,
    >,
    handle: tauri::async_runtime::JoinHandle<()>,
}

pub(super) struct ScreenBroadcastVp8Encoder {
    width: u32,
    height: u32,
    encoder: rchat_libvpx::Vp8Encoder,
}

impl ScreenBroadcastVp8Encoder {
    fn new(width: u32, height: u32) -> Result<Self, String> {
        let encoder = rchat_libvpx::Vp8Encoder::new(rchat_libvpx::Vp8EncoderConfig {
            width,
            height,
            bitrate_kbps: SCREEN_BROADCAST_BITRATE_KBPS,
            fps: SCREEN_BROADCAST_FPS,
            threads: SCREEN_BROADCAST_ENCODER_THREADS,
            keyframe_interval: SCREEN_BROADCAST_KEYFRAME_INTERVAL_FRAMES,
            cpu_used: SCREEN_BROADCAST_ENCODER_CPU_USED,
        })
        .map_err(|e| e.to_string())?;
        Ok(Self {
            width,
            height,
            encoder,
        })
    }

    fn encode_i420(
        &mut self,
        width: u32,
        height: u32,
        data: &[u8],
        force_keyframe: bool,
    ) -> Result<Vec<rchat_libvpx::EncodedPacket>, String> {
        if width != self.width || height != self.height {
            *self = Self::new(width, height)?;
        }
        let expected_len = rchat_libvpx::expected_i420_len(width, height)
            .ok_or_else(|| "invalid screen frame size".to_string())?;
        if data.len() != expected_len {
            return Err(format!(
                "invalid screen I420 frame length: expected {}, got {}",
                expected_len,
                data.len()
            ));
        }
        self.encoder
            .encode_i420(data, force_keyframe)
            .map_err(|e| e.to_string())
    }
}

#[derive(Debug, Clone, Serialize)]
struct ScreenBroadcastLocalPreviewFrameEvent {
    session_id: String,
    timestamp_us: i64,
    width: u32,
    height: u32,
    rgba: Vec<u8>,
}

#[derive(Debug, Clone, Serialize)]
struct ScreenBroadcastCaptureErrorEvent {
    session_id: String,
    message: String,
}

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
        self.stop_screen_broadcast_media("final");
        self.active_broadcast = None;
        self.push_idle_broadcast_state(reason).await;
    }

    fn reset_screen_broadcast_diagnostics(&mut self) {
        self.screen_broadcast_stats.reset();
        self.screen_broadcast_next_seq = 0;
        self.screen_broadcast_force_next_keyframe = true;
        self.screen_broadcast_last_summary_at = None;
        self.screen_capture_last_stats = rchat_screen_capture::ScreenCaptureSessionStats::default();
    }

    fn stop_screen_broadcast_media(&mut self, label: &str) {
        self.log_screen_broadcast_summary(label);
        if let Some(task) = self.screen_capture_start_task.take() {
            task.handle.abort();
        }
        if let Some(session) = self.screen_capture_session.take() {
            self.screen_capture_last_stats = session.stats();
        }
        self.screen_capture_info = None;
        self.screen_capture_started_at = None;
        self.screen_broadcast_vp8_encoder = None;
    }

    fn active_host_broadcast_snapshot(&self) -> Option<ActiveBroadcast> {
        self.active_broadcast.as_ref().and_then(|session| {
            if session.phase == ActiveBroadcastPhase::Active && session.is_host {
                Some(session.clone())
            } else {
                None
            }
        })
    }

    fn screen_capture_stats_snapshot(&self) -> rchat_screen_capture::ScreenCaptureSessionStats {
        self.screen_capture_session
            .as_ref()
            .map(|session| session.stats())
            .unwrap_or(self.screen_capture_last_stats.clone())
    }

    fn log_screen_broadcast_summary(&self, label: &str) {
        let Some(session) = self.active_broadcast.as_ref() else {
            return;
        };
        if session.phase != ActiveBroadcastPhase::Active || !session.is_host {
            return;
        }

        let capture_stats = self.screen_capture_stats_snapshot();
        let info = self
            .screen_capture_session
            .as_ref()
            .map(|session| session.info())
            .or(self.screen_capture_info.as_ref());
        let elapsed_secs = self
            .screen_capture_started_at
            .map(|started| started.elapsed().as_secs_f64().max(0.001))
            .unwrap_or(0.0);
        let captured_fps = if elapsed_secs > 0.0 {
            capture_stats.captured_frames as f64 / elapsed_secs
        } else {
            0.0
        };
        let actual_kbps = if elapsed_secs > 0.0 {
            self.screen_broadcast_stats.outbound_bytes as f64 * 8.0 / elapsed_secs / 1000.0
        } else {
            0.0
        };
        let (backend, source, actual_width, actual_height, actual_fps, format) =
            if let Some(info) = info {
                (
                    info.backend.label(),
                    info.source_label.as_str(),
                    info.format.width,
                    info.format.height,
                    info.format.fps,
                    info.format.format.as_str(),
                )
            } else {
                ("unknown", "unknown", 0, 0, 0, "unknown")
            };

        println!(
            "[Broadcast][Screen][{}] peer={}, backend={}, source='{}', profile={}, actual_width={}, actual_height={}, actual_fps={}, format={}, target_kbps={}, actual_kbps={:.1}, captured_frames={}, captured_fps={:.1}, capture_drops={}, preview_drops={}, conversion_errors={}, preview_frames={}, encoded_frames={}, keyframes={}, delta_frames={}, outbound_bytes={}, encode_errors={}, outbound_failures={}, inbound_failures={}, rejected_responses={}",
            label,
            session.remote_peer_id,
            backend,
            source,
            SCREEN_BROADCAST_PROFILE,
            actual_width,
            actual_height,
            actual_fps,
            format,
            SCREEN_BROADCAST_BITRATE_KBPS,
            actual_kbps,
            capture_stats.captured_frames,
            captured_fps,
            capture_stats.dropped_i420_frames,
            capture_stats.dropped_preview_frames,
            capture_stats.conversion_errors,
            capture_stats.preview_frames,
            self.screen_broadcast_stats.encoded_frames,
            self.screen_broadcast_stats.keyframes,
            self.screen_broadcast_stats.delta_frames,
            self.screen_broadcast_stats.outbound_bytes,
            self.screen_broadcast_stats.encode_errors,
            self.screen_broadcast_stats.outbound_failures,
            self.screen_broadcast_stats.inbound_failures,
            self.screen_broadcast_stats.rejected_responses,
        );
    }

    fn maybe_log_screen_broadcast_summary(&mut self) {
        if self.active_host_broadcast_snapshot().is_none() {
            return;
        }
        let now = Instant::now();
        let should_log = self
            .screen_broadcast_last_summary_at
            .map(|last| now.duration_since(last) >= SCREEN_BROADCAST_SUMMARY_INTERVAL)
            .unwrap_or(true);
        if should_log {
            self.log_screen_broadcast_summary("summary");
            self.screen_broadcast_last_summary_at = Some(now);
        }
    }

    async fn fail_active_screen_capture(&mut self, session: &ActiveBroadcast, message: String) {
        eprintln!(
            "[Broadcast][Screen] capture failure session={} peer={}: {}",
            session.session_id, session.remote_peer_id, message
        );
        let _ = self.app_handle.emit(
            "screen-broadcast-capture-error",
            ScreenBroadcastCaptureErrorEvent {
                session_id: session.session_id.clone(),
                message: message.clone(),
            },
        );
        self.send_broadcast_signal(
            session.remote_peer_id,
            DirectMessageKind::BroadcastEnd,
            &session.session_id,
        );
        self.transition_broadcast_to_idle(Some("screen_capture_failed".to_string()))
            .await;
    }

    async fn poll_screen_capture_start_task(&mut self, session: &ActiveBroadcast) -> bool {
        let Some(mut task) = self.screen_capture_start_task.take() else {
            return true;
        };

        if task.session_id != session.session_id {
            task.handle.abort();
            return false;
        }

        match task.result_rx.try_recv() {
            Ok(Ok(capture_session)) => {
                let info = capture_session.info().clone();
                println!(
                    "[Broadcast][Screen] capture started session={} backend={} source='{}' format={} {}x{}@{}",
                    session.session_id,
                    info.backend.label(),
                    info.source_label,
                    info.format.format,
                    info.format.width,
                    info.format.height,
                    info.format.fps
                );
                self.screen_capture_info = Some(info);
                self.screen_capture_started_at = Some(Instant::now());
                self.screen_capture_session = Some(capture_session);
                true
            }
            Ok(Err(error)) => {
                self.screen_broadcast_stats.capture_start_failures = self
                    .screen_broadcast_stats
                    .capture_start_failures
                    .saturating_add(1);
                self.fail_active_screen_capture(session, error.to_string())
                    .await;
                false
            }
            Err(tokio::sync::oneshot::error::TryRecvError::Empty) => {
                self.screen_capture_start_task = Some(task);
                false
            }
            Err(tokio::sync::oneshot::error::TryRecvError::Closed) => {
                self.screen_broadcast_stats.capture_start_failures = self
                    .screen_broadcast_stats
                    .capture_start_failures
                    .saturating_add(1);
                self.fail_active_screen_capture(session, "screen capture task failed".to_string())
                    .await;
                false
            }
        }
    }

    async fn ensure_screen_capture_running(&mut self, session: &ActiveBroadcast) -> bool {
        if self.screen_capture_session.is_some() {
            return true;
        }
        if self.screen_capture_start_task.is_some() {
            return self.poll_screen_capture_start_task(session).await;
        }

        let session_id = session.session_id.clone();
        println!(
            "[Broadcast][Screen] starting native capture session={} peer={}",
            session.session_id, session.remote_peer_id
        );
        let (result_tx, result_rx) = tokio::sync::oneshot::channel();
        let handle = tauri::async_runtime::spawn(async move {
            let result = rchat_screen_capture::ScreenCaptureSession::start(
                rchat_screen_capture::ScreenCaptureConfig::default(),
            )
            .await;
            let _ = result_tx.send(result);
        });
        self.screen_capture_start_task = Some(ScreenCaptureStartTask {
            session_id,
            result_rx,
            handle,
        });
        false
    }

    fn emit_screen_broadcast_preview(&mut self, session_id: &str) {
        let Some(capture_session) = self.screen_capture_session.as_mut() else {
            return;
        };
        while let Some(preview) = capture_session.try_recv_latest_preview() {
            let _ = self.app_handle.emit(
                "screen-broadcast-local-preview-frame",
                ScreenBroadcastLocalPreviewFrameEvent {
                    session_id: session_id.to_string(),
                    timestamp_us: preview.timestamp_us,
                    width: preview.width,
                    height: preview.height,
                    rgba: preview.rgba,
                },
            );
        }
    }

    fn encode_and_send_screen_frame(
        &mut self,
        session: &ActiveBroadcast,
        frame: rchat_screen_capture::I420ScreenFrame,
    ) {
        let force_keyframe = self.screen_broadcast_force_next_keyframe
            || self.screen_broadcast_next_seq == 0
            || self.screen_broadcast_next_seq % SCREEN_BROADCAST_KEYFRAME_INTERVAL_FRAMES == 0;

        if self.screen_broadcast_vp8_encoder.is_none() {
            match ScreenBroadcastVp8Encoder::new(frame.width, frame.height) {
                Ok(encoder) => {
                    self.screen_broadcast_vp8_encoder = Some(encoder);
                    self.screen_broadcast_force_next_keyframe = true;
                }
                Err(error) => {
                    self.screen_broadcast_stats.encode_errors =
                        self.screen_broadcast_stats.encode_errors.saturating_add(1);
                    eprintln!("[Broadcast][Screen] VP8 encoder init failed: {}", error);
                    return;
                }
            }
        }

        let packets = match self
            .screen_broadcast_vp8_encoder
            .as_mut()
            .expect("encoder was initialized")
            .encode_i420(frame.width, frame.height, &frame.data, force_keyframe)
        {
            Ok(packets) => packets,
            Err(error) => {
                self.screen_broadcast_stats.encode_errors =
                    self.screen_broadcast_stats.encode_errors.saturating_add(1);
                self.screen_broadcast_force_next_keyframe = true;
                eprintln!("[Broadcast][Screen] VP8 encode failed: {}", error);
                return;
            }
        };

        for packet in packets {
            if packet.payload.is_empty() {
                continue;
            }
            let seq = self.screen_broadcast_next_seq;
            self.screen_broadcast_next_seq = self.screen_broadcast_next_seq.wrapping_add(1);
            self.screen_broadcast_stats.encoded_frames =
                self.screen_broadcast_stats.encoded_frames.saturating_add(1);
            self.screen_broadcast_stats.outbound_bytes = self
                .screen_broadcast_stats
                .outbound_bytes
                .saturating_add(packet.payload.len() as u64);
            let chunk_type = if packet.is_key {
                self.screen_broadcast_stats.keyframes =
                    self.screen_broadcast_stats.keyframes.saturating_add(1);
                BroadcastChunkType::Key
            } else {
                self.screen_broadcast_stats.delta_frames =
                    self.screen_broadcast_stats.delta_frames.saturating_add(1);
                BroadcastChunkType::Delta
            };
            self.screen_broadcast_force_next_keyframe = false;

            self.swarm.behaviour_mut().broadcast.send_request(
                &session.remote_peer_id,
                BroadcastFrameRequest {
                    session_id: session.session_id.clone(),
                    seq,
                    timestamp: frame.timestamp_us,
                    mime: SCREEN_BROADCAST_MIME.to_string(),
                    codec: SCREEN_BROADCAST_CODEC.to_string(),
                    profile: SCREEN_BROADCAST_PROFILE.to_string(),
                    width: frame.width,
                    height: frame.height,
                    chunk_type,
                    payload: packet.payload,
                },
            );
        }
    }

    async fn pump_screen_broadcast_capture(&mut self, session: &ActiveBroadcast) {
        self.emit_screen_broadcast_preview(&session.session_id);
        let Some(capture_session) = self.screen_capture_session.as_mut() else {
            return;
        };

        let mut latest = None;
        while let Some(frame) = capture_session.try_recv_latest_i420() {
            latest = Some(frame);
        }
        if let Some(frame) = latest {
            self.encode_and_send_screen_frame(session, frame);
        }
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
            self.push_idle_broadcast_state(Some("busy".to_string()))
                .await;
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

        let Some(peer_id) = self
            .resolve_peer_id(&peer_chat_id, "SCREEN_BROADCAST")
            .await
        else {
            self.push_idle_broadcast_state(Some("peer_unresolved".to_string()))
                .await;
            return;
        };

        if !self.swarm.is_connected(&peer_id) {
            self.push_idle_broadcast_state(Some("peer_not_connected".to_string()))
                .await;
            return;
        }

        self.reset_screen_broadcast_diagnostics();
        let now = Self::now_unix_ts();
        let session_id = format!("broadcast-{}-{}", now, rand::random::<u32>());
        let session = ActiveBroadcast {
            session_id: session_id.clone(),
            peer_chat_id,
            remote_peer_id: peer_id,
            phase: ActiveBroadcastPhase::OutgoingRinging,
            ring_deadline: Some(
                std::time::Instant::now()
                    + std::time::Duration::from_secs(BROADCAST_RING_TIMEOUT_SECS),
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

        self.swarm.behaviour_mut().broadcast.send_request(
            &session_snapshot.remote_peer_id,
            BroadcastFrameRequest {
                session_id,
                seq,
                timestamp,
                mime,
                codec,
                profile: "legacy".to_string(),
                width: 640,
                height: 360,
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
                    self.send_broadcast_signal(
                        peer,
                        DirectMessageKind::BroadcastReject,
                        &request.id,
                    );
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

        let Some(session) = self.active_host_broadcast_snapshot() else {
            if self.screen_capture_session.is_some() || self.screen_capture_start_task.is_some() {
                self.stop_screen_broadcast_media("final");
            }
            return;
        };

        if self.ensure_screen_capture_running(&session).await {
            self.pump_screen_broadcast_capture(&session).await;
            self.maybe_log_screen_broadcast_summary();
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
                                profile: request.profile,
                                width: request.width,
                                height: request.height,
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
                        self.screen_broadcast_stats.rejected_responses = self
                            .screen_broadcast_stats
                            .rejected_responses
                            .saturating_add(1);
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
                self.screen_broadcast_stats.outbound_failures = self
                    .screen_broadcast_stats
                    .outbound_failures
                    .saturating_add(1);
                eprintln!(
                    "[Broadcast] Outbound frame failure to {}: {:?}",
                    peer, error
                );
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
                self.screen_broadcast_stats.inbound_failures = self
                    .screen_broadcast_stats
                    .inbound_failures
                    .saturating_add(1);
                eprintln!(
                    "[Broadcast] Inbound frame failure from {}: {:?}",
                    peer, error
                );
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
