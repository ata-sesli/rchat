use super::*;
use crate::app_state::{BroadcastPhase, CallKind};
use crate::live::broadcast::protocol::{
    read_broadcast_stream_header, read_broadcast_stream_record, write_broadcast_stream_header,
    write_broadcast_stream_record, BroadcastChunkType, BroadcastFrameEvent, BroadcastFrameRequest,
    BroadcastFrameResponse, BroadcastStreamFrame, BroadcastStreamRecord,
};
use crate::network::direct_message::{DirectMessageKind, DirectMessageRequest};
use futures::io::AsyncWriteExt;
use futures::StreamExt;
use libp2p::request_response;
use serde::Serialize;
use std::time::{Duration, Instant};

const BROADCAST_RING_TIMEOUT_SECS: u64 = 30;
const SCREEN_BROADCAST_ENCODER_THREADS: u32 = 4;
const SCREEN_BROADCAST_ENCODER_CPU_USED: i32 = 8;
const SCREEN_BROADCAST_MIME: &str = "video/webm;codecs=vp8";
const SCREEN_BROADCAST_CODEC: &str = "vp8";
const SCREEN_BROADCAST_SUMMARY_INTERVAL: Duration = Duration::from_secs(5);
const SCREEN_BROADCAST_STREAM_QUEUE_CAPACITY: usize = 256;

#[allow(dead_code)]
pub(super) struct ScreenCaptureStartTask {
    pub(super) session_id: String,
    pub(super) result_rx: tokio::sync::oneshot::Receiver<
        Result<
            rchat_screen_capture::ScreenCaptureSession,
            rchat_screen_capture::ScreenCaptureError,
        >,
    >,
    pub(super) handle: tauri::async_runtime::JoinHandle<()>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn screen_broadcast_keyframe_scheduler_forces_next_frame_once() {
        let mut scheduler = ScreenBroadcastKeyframeScheduler::new(60);

        assert!(scheduler.should_force(0));
        assert!(!scheduler.should_force(1));

        scheduler.force_next();

        assert!(scheduler.should_force(2));
        assert!(!scheduler.should_force(3));
    }
}

pub(super) struct ScreenBroadcastVp8Encoder {
    width: u32,
    height: u32,
    profile: rchat_screen_capture::ScreenCaptureProfile,
    encoder: rchat_libvpx::Vp8Encoder,
}

impl ScreenBroadcastVp8Encoder {
    fn new(
        width: u32,
        height: u32,
        profile: rchat_screen_capture::ScreenCaptureProfile,
    ) -> Result<Self, String> {
        let encoder = rchat_libvpx::Vp8Encoder::new(rchat_libvpx::Vp8EncoderConfig {
            width,
            height,
            bitrate_kbps: profile.bitrate_kbps(),
            fps: profile.fps(),
            threads: SCREEN_BROADCAST_ENCODER_THREADS,
            keyframe_interval: profile.keyframe_interval_frames(),
            cpu_used: SCREEN_BROADCAST_ENCODER_CPU_USED,
        })
        .map_err(|e| e.to_string())?;
        Ok(Self {
            width,
            height,
            profile,
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
            *self = Self::new(width, height, self.profile)?;
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

#[derive(Debug)]
pub(super) enum ScreenBroadcastStreamEvent {
    InboundRecord {
        peer: PeerId,
        session_id: String,
        record: BroadcastStreamRecord,
    },
    InboundFailure {
        peer: PeerId,
        session_id: Option<String>,
        error: String,
    },
    OutboundFailure {
        peer: PeerId,
        session_id: String,
        error: String,
    },
}

#[derive(Debug, Clone, Default)]
pub(super) struct ScreenBroadcastWorkerStats {
    started_at: Option<Instant>,
    capture_stats: rchat_screen_capture::ScreenCaptureSessionStats,
    encoded_frames: u64,
    keyframes: u64,
    delta_frames: u64,
    outbound_bytes: u64,
    encode_errors: u64,
    skipped_frames: u64,
    worker_event_drops: u64,
    encode_micros: Vec<u64>,
}

impl ScreenBroadcastWorkerStats {
    fn capture_fps(&self) -> f64 {
        let Some(started_at) = self.started_at else {
            return 0.0;
        };
        let elapsed = started_at.elapsed().as_secs_f64().max(0.001);
        self.capture_stats.captured_frames as f64 / elapsed
    }

    fn encode_fps(&self) -> f64 {
        let Some(started_at) = self.started_at else {
            return 0.0;
        };
        let elapsed = started_at.elapsed().as_secs_f64().max(0.001);
        self.encoded_frames as f64 / elapsed
    }

    fn actual_kbps(&self) -> f64 {
        let Some(started_at) = self.started_at else {
            return 0.0;
        };
        let elapsed = started_at.elapsed().as_secs_f64().max(0.001);
        self.outbound_bytes as f64 * 8.0 / elapsed / 1000.0
    }

    fn encode_p95_ms(&self) -> f64 {
        if self.encode_micros.is_empty() {
            return 0.0;
        }
        let mut values = self.encode_micros.clone();
        values.sort_unstable();
        let index = ((values.len() - 1) as f64 * 0.95).round() as usize;
        values[index] as f64 / 1000.0
    }
}

#[derive(Debug)]
pub(super) enum ScreenBroadcastWorkerEvent {
    Started {
        session_id: String,
        info: rchat_screen_capture::ScreenCaptureSessionInfo,
    },
    EncodedFrame {
        session_id: String,
        frame: BroadcastStreamFrame,
        encode_micros: u64,
    },
    Stats {
        session_id: String,
        info: rchat_screen_capture::ScreenCaptureSessionInfo,
        stats: ScreenBroadcastWorkerStats,
    },
    Failure {
        session_id: String,
        error: String,
    },
}

#[derive(Debug)]
pub(super) enum ScreenBroadcastWorkerCommand {
    ForceKeyframe,
}

#[derive(Debug)]
struct ScreenBroadcastKeyframeScheduler {
    interval_frames: u32,
    force_next: bool,
}

impl ScreenBroadcastKeyframeScheduler {
    fn new(interval_frames: u32) -> Self {
        Self {
            interval_frames: interval_frames.max(1),
            force_next: true,
        }
    }

    fn force_next(&mut self) {
        self.force_next = true;
    }

    fn should_force(&mut self, seq: u32) -> bool {
        let force_keyframe = self.force_next || seq == 0 || seq % self.interval_frames.max(1) == 0;
        if force_keyframe {
            self.force_next = false;
        }
        force_keyframe
    }
}

fn start_screen_broadcast_worker(
    session_id: String,
    profile: rchat_screen_capture::ScreenCaptureProfile,
    app_handle: AppHandle,
    event_tx: tokio::sync::mpsc::Sender<ScreenBroadcastWorkerEvent>,
    mut control_rx: tokio::sync::mpsc::Receiver<ScreenBroadcastWorkerCommand>,
) -> tauri::async_runtime::JoinHandle<()> {
    tauri::async_runtime::spawn(async move {
        let config = rchat_screen_capture::ScreenCaptureConfig::default_for_profile(profile);
        let mut capture_session =
            match rchat_screen_capture::ScreenCaptureSession::start(config).await {
                Ok(session) => session,
                Err(error) => {
                    let _ = event_tx
                        .send(ScreenBroadcastWorkerEvent::Failure {
                            session_id,
                            error: error.to_string(),
                        })
                        .await;
                    return;
                }
            };

        let info = capture_session.info().clone();
        let _ = event_tx
            .send(ScreenBroadcastWorkerEvent::Started {
                session_id: session_id.clone(),
                info: info.clone(),
            })
            .await;

        let cadence_ms = (1000 / profile.fps().max(1) as u64).max(1);
        let mut cadence = tokio::time::interval(Duration::from_millis(cadence_ms));
        cadence.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut last_stats_at = Instant::now();
        let mut stats = ScreenBroadcastWorkerStats {
            started_at: Some(Instant::now()),
            ..ScreenBroadcastWorkerStats::default()
        };
        let mut encoder: Option<ScreenBroadcastVp8Encoder> = None;
        let mut seq = 0_u32;
        let mut keyframes =
            ScreenBroadcastKeyframeScheduler::new(profile.keyframe_interval_frames());

        loop {
            cadence.tick().await;
            while let Ok(ScreenBroadcastWorkerCommand::ForceKeyframe) = control_rx.try_recv() {
                keyframes.force_next();
            }

            while let Some(preview) = capture_session.try_recv_latest_preview() {
                let _ = app_handle.emit(
                    "screen-broadcast-local-preview-frame",
                    ScreenBroadcastLocalPreviewFrameEvent {
                        session_id: session_id.clone(),
                        timestamp_us: preview.timestamp_us,
                        width: preview.width,
                        height: preview.height,
                        rgba: preview.rgba,
                    },
                );
            }

            let mut latest_frame = None;
            while let Some(frame) = capture_session.try_recv_latest_i420() {
                latest_frame = Some(frame);
            }

            let Some(frame) = latest_frame else {
                stats.skipped_frames = stats.skipped_frames.saturating_add(1);
                stats.capture_stats = capture_session.stats();
                maybe_emit_worker_stats(&event_tx, &session_id, &info, &stats, &mut last_stats_at);
                continue;
            };

            let force_keyframe = keyframes.should_force(seq);
            if encoder.is_none() {
                match ScreenBroadcastVp8Encoder::new(frame.width, frame.height, profile) {
                    Ok(new_encoder) => {
                        encoder = Some(new_encoder);
                    }
                    Err(error) => {
                        stats.encode_errors = stats.encode_errors.saturating_add(1);
                        eprintln!("[Broadcast][Screen] VP8 encoder init failed: {}", error);
                        continue;
                    }
                }
            }

            let encode_started = Instant::now();
            let packets = match encoder
                .as_mut()
                .expect("encoder was initialized")
                .encode_i420(frame.width, frame.height, &frame.data, force_keyframe)
            {
                Ok(packets) => packets,
                Err(error) => {
                    stats.encode_errors = stats.encode_errors.saturating_add(1);
                    eprintln!("[Broadcast][Screen] VP8 encode failed: {}", error);
                    continue;
                }
            };
            let encode_micros = encode_started
                .elapsed()
                .as_micros()
                .min(u128::from(u64::MAX)) as u64;
            stats.encode_micros.push(encode_micros);

            for packet in packets {
                if packet.payload.is_empty() {
                    continue;
                }
                let chunk_type = if packet.is_key {
                    stats.keyframes = stats.keyframes.saturating_add(1);
                    BroadcastChunkType::Key
                } else {
                    stats.delta_frames = stats.delta_frames.saturating_add(1);
                    BroadcastChunkType::Delta
                };
                let payload_len = packet.payload.len() as u64;
                let stream_frame = BroadcastStreamFrame {
                    seq,
                    timestamp_us: frame.timestamp_us,
                    chunk_type,
                    profile,
                    width: frame.width,
                    height: frame.height,
                    payload: packet.payload,
                };
                seq = seq.wrapping_add(1);
                stats.encoded_frames = stats.encoded_frames.saturating_add(1);
                stats.outbound_bytes = stats.outbound_bytes.saturating_add(payload_len);

                match event_tx.try_send(ScreenBroadcastWorkerEvent::EncodedFrame {
                    session_id: session_id.clone(),
                    frame: stream_frame,
                    encode_micros,
                }) {
                    Ok(()) => {}
                    Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
                        stats.worker_event_drops = stats.worker_event_drops.saturating_add(1);
                        keyframes.force_next();
                    }
                    Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => return,
                }
            }

            stats.capture_stats = capture_session.stats();
            maybe_emit_worker_stats(&event_tx, &session_id, &info, &stats, &mut last_stats_at);
        }
    })
}

fn maybe_emit_worker_stats(
    event_tx: &tokio::sync::mpsc::Sender<ScreenBroadcastWorkerEvent>,
    session_id: &str,
    info: &rchat_screen_capture::ScreenCaptureSessionInfo,
    stats: &ScreenBroadcastWorkerStats,
    last_stats_at: &mut Instant,
) {
    if last_stats_at.elapsed() < Duration::from_secs(1) {
        return;
    }
    *last_stats_at = Instant::now();
    let _ = event_tx.try_send(ScreenBroadcastWorkerEvent::Stats {
        session_id: session_id.to_string(),
        info: info.clone(),
        stats: stats.clone(),
    });
}

pub(super) fn start_screen_broadcast_stream_accept_loop(
    incoming: crate::network::voice_stream::IncomingStreams,
    event_tx: tokio::sync::mpsc::Sender<ScreenBroadcastStreamEvent>,
) {
    tauri::async_runtime::spawn(async move {
        futures::pin_mut!(incoming);
        while let Some((peer, mut stream)) = incoming.next().await {
            let event_tx = event_tx.clone();
            tauri::async_runtime::spawn(async move {
                eprintln!("[Broadcast][Stream] inbound stream accepted peer={}", peer);
                let session_id = match read_broadcast_stream_header(&mut stream).await {
                    Ok(session_id) => session_id,
                    Err(error) => {
                        let _ = event_tx
                            .send(ScreenBroadcastStreamEvent::InboundFailure {
                                peer,
                                session_id: None,
                                error: error.to_string(),
                            })
                            .await;
                        return;
                    }
                };
                eprintln!(
                    "[Broadcast][Stream] inbound header read peer={} session_id={}",
                    peer, session_id
                );

                let mut first_frame_read = false;
                loop {
                    match read_broadcast_stream_record(&mut stream).await {
                        Ok(record) => {
                            let BroadcastStreamRecord::Frame(frame) = &record;
                            if !first_frame_read {
                                eprintln!(
                                    "[Broadcast][Stream] inbound first frame read peer={} session_id={} seq={} bytes={} kind={:?} profile={}",
                                    peer,
                                    session_id,
                                    frame.seq,
                                    frame.payload.len(),
                                    frame.chunk_type,
                                    frame.profile.label()
                                );
                                first_frame_read = true;
                            }
                            if event_tx
                                .send(ScreenBroadcastStreamEvent::InboundRecord {
                                    peer,
                                    session_id: session_id.clone(),
                                    record,
                                })
                                .await
                                .is_err()
                            {
                                return;
                            }
                        }
                        Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => return,
                        Err(error) => {
                            let _ = event_tx
                                .send(ScreenBroadcastStreamEvent::InboundFailure {
                                    peer,
                                    session_id: Some(session_id.clone()),
                                    error: error.to_string(),
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

impl NetworkManager {
    fn broadcast_session_id_from_signal(request: &DirectMessageRequest) -> String {
        request
            .text_content
            .clone()
            .filter(|value| !value.trim().is_empty())
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
        self.screen_broadcast_worker_stats = ScreenBroadcastWorkerStats::default();
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
        if let Some(handle) = self.screen_broadcast_worker_handle.take() {
            handle.abort();
        }
        self.screen_broadcast_worker_session_id = None;
        self.screen_broadcast_worker_control_tx = None;
        self.screen_broadcast_stream_tx = None;
        self.screen_broadcast_stream_session_id = None;
        if let Some(handle) = self.screen_broadcast_stream_writer_handle.take() {
            handle.abort();
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

    fn log_screen_broadcast_summary(&self, label: &str) {
        let Some(session) = self.active_broadcast.as_ref() else {
            return;
        };
        if session.phase != ActiveBroadcastPhase::Active || !session.is_host {
            return;
        }

        let info = self.screen_capture_info.as_ref().or_else(|| {
            self.screen_capture_session
                .as_ref()
                .map(|session| session.info())
        });
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
        let stats = &self.screen_broadcast_worker_stats;
        println!(
            "[Broadcast][Screen][{}] peer={}, backend={}, source='{}', profile={}, actual_width={}, actual_height={}, actual_fps={}, format={}, target_kbps={}, actual_kbps={:.1}, captured_frames={}, captured_fps={:.1}, encode_fps={:.1}, encode_p95_ms={:.1}, capture_drops={}, preview_drops={}, conversion_errors={}, preview_frames={}, skipped_frames={}, encoded_frames={}, keyframes={}, delta_frames={}, outbound_bytes={}, encode_errors={}, worker_event_drops={}, stream_queue_drops={}, outbound_failures={}, inbound_failures={}, rejected_responses={}",
            label,
            session.remote_peer_id,
            backend,
            source,
            session.profile.label(),
            actual_width,
            actual_height,
            actual_fps,
            format,
            session.profile.bitrate_kbps(),
            stats.actual_kbps(),
            stats.capture_stats.captured_frames,
            stats.capture_fps(),
            stats.encode_fps(),
            stats.encode_p95_ms(),
            stats.capture_stats.dropped_i420_frames,
            stats.capture_stats.dropped_preview_frames,
            stats.capture_stats.conversion_errors,
            stats.capture_stats.preview_frames,
            stats.skipped_frames,
            stats.encoded_frames,
            stats.keyframes,
            stats.delta_frames,
            stats.outbound_bytes,
            stats.encode_errors,
            stats.worker_event_drops,
            self.screen_broadcast_stats.stream_queue_drops,
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
                message,
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

    fn start_screen_broadcast_stream_writer(&mut self, peer: PeerId, session_id: String) -> bool {
        if self.screen_broadcast_stream_tx.is_some()
            && self.screen_broadcast_stream_session_id.as_deref() == Some(session_id.as_str())
        {
            return true;
        }

        let Some(connection_id) = self.voice_quic_connection_id(&peer) else {
            eprintln!(
                "[Broadcast][QUIC] No QUIC connection id available for screen stream: peer={}",
                peer
            );
            return false;
        };

        self.screen_broadcast_stream_tx = None;
        self.screen_broadcast_stream_session_id = None;
        if let Some(handle) = self.screen_broadcast_stream_writer_handle.take() {
            handle.abort();
        }

        eprintln!(
            "[Broadcast][Stream] selected outbound QUIC connection peer={} session_id={} connection_id={:?}",
            peer, session_id, connection_id
        );

        let (tx, mut rx) = tokio::sync::mpsc::channel::<BroadcastStreamRecord>(
            SCREEN_BROADCAST_STREAM_QUEUE_CAPACITY,
        );
        let stream_rx = match self
            .swarm
            .behaviour_mut()
            .broadcast_stream
            .open_stream_on_connection(peer, connection_id)
        {
            Ok(stream_rx) => stream_rx,
            Err(error) => {
                eprintln!(
                    "[Broadcast][QUIC] Failed to queue screen stream on {} for {}: {}",
                    connection_id, peer, error
                );
                return false;
            }
        };
        let event_tx = self.screen_broadcast_stream_event_tx.clone();
        let writer_session_id = session_id.clone();
        let handle = tauri::async_runtime::spawn(async move {
            let mut stream = match tokio::time::timeout(Duration::from_secs(5), stream_rx).await {
                Ok(Ok(Ok(stream))) => {
                    eprintln!(
                            "[Broadcast][Stream] outbound stream opened peer={} session_id={} connection_id={:?}",
                            peer, writer_session_id, connection_id
                        );
                    stream
                }
                Ok(Ok(Err(error))) => {
                    let _ = event_tx
                        .send(ScreenBroadcastStreamEvent::OutboundFailure {
                            peer,
                            session_id: writer_session_id.clone(),
                            error: error.to_string(),
                        })
                        .await;
                    return;
                }
                Ok(Err(_)) => {
                    let _ = event_tx
                        .send(ScreenBroadcastStreamEvent::OutboundFailure {
                            peer,
                            session_id: writer_session_id.clone(),
                            error: "stream open canceled".to_string(),
                        })
                        .await;
                    return;
                }
                Err(error) => {
                    let _ = event_tx
                        .send(ScreenBroadcastStreamEvent::OutboundFailure {
                            peer,
                            session_id: writer_session_id.clone(),
                            error: format!("stream open timed out: {}", error),
                        })
                        .await;
                    return;
                }
            };

            if let Err(error) = write_broadcast_stream_header(&mut stream, &writer_session_id).await
            {
                let _ = event_tx
                    .send(ScreenBroadcastStreamEvent::OutboundFailure {
                        peer,
                        session_id: writer_session_id.clone(),
                        error: error.to_string(),
                    })
                    .await;
                return;
            }
            eprintln!(
                "[Broadcast][Stream] outbound header written peer={} session_id={} connection_id={:?}",
                peer, writer_session_id, connection_id
            );

            let mut first_frame_written = false;
            while let Some(record) = rx.recv().await {
                let frame_log = match &record {
                    BroadcastStreamRecord::Frame(frame) => Some((
                        frame.seq,
                        frame.payload.len(),
                        frame.chunk_type,
                        frame.profile,
                    )),
                };
                if let Err(error) = write_broadcast_stream_record(&mut stream, &record).await {
                    let _ = event_tx
                        .send(ScreenBroadcastStreamEvent::OutboundFailure {
                            peer,
                            session_id: writer_session_id.clone(),
                            error: error.to_string(),
                        })
                        .await;
                    return;
                }
                if let Some((seq, bytes, chunk_type, profile)) = frame_log {
                    if !first_frame_written {
                        eprintln!(
                            "[Broadcast][Stream] outbound first frame written peer={} session_id={} seq={} bytes={} kind={:?} profile={} connection_id={:?}",
                            peer,
                            writer_session_id,
                            seq,
                            bytes,
                            chunk_type,
                            profile.label(),
                            connection_id
                        );
                        first_frame_written = true;
                    }
                }
            }

            let _ = stream.close().await;
        });

        self.screen_broadcast_stream_tx = Some(tx);
        self.screen_broadcast_stream_session_id = Some(session_id);
        self.screen_broadcast_stream_writer_handle = Some(handle);
        true
    }

    fn queue_screen_broadcast_stream_record(&mut self, record: BroadcastStreamRecord) -> bool {
        let Some(tx) = self.screen_broadcast_stream_tx.as_ref() else {
            return false;
        };
        match tx.try_send(record) {
            Ok(()) => true,
            Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
                self.screen_broadcast_stats.stream_queue_drops = self
                    .screen_broadcast_stats
                    .stream_queue_drops
                    .saturating_add(1);
                false
            }
            Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
                self.screen_broadcast_stats.outbound_failures = self
                    .screen_broadcast_stats
                    .outbound_failures
                    .saturating_add(1);
                self.screen_broadcast_stream_tx = None;
                self.screen_broadcast_stream_session_id = None;
                false
            }
        }
    }

    fn request_screen_broadcast_worker_keyframe(&mut self) {
        let Some(tx) = self.screen_broadcast_worker_control_tx.as_ref() else {
            return;
        };
        match tx.try_send(ScreenBroadcastWorkerCommand::ForceKeyframe) {
            Ok(()) | Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {}
            Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
                self.screen_broadcast_worker_control_tx = None;
            }
        }
    }

    fn ensure_screen_broadcast_worker(&mut self, session: &ActiveBroadcast) {
        if self.screen_broadcast_worker_handle.is_some()
            && self.screen_broadcast_worker_session_id.as_deref()
                == Some(session.session_id.as_str())
        {
            return;
        }
        if let Some(handle) = self.screen_broadcast_worker_handle.take() {
            handle.abort();
        }
        self.screen_broadcast_worker_control_tx = None;
        self.screen_broadcast_worker_session_id = Some(session.session_id.clone());
        self.screen_broadcast_worker_stats = ScreenBroadcastWorkerStats::default();
        eprintln!(
            "[Broadcast][Screen] starting capture worker session={} peer={} profile={}",
            session.session_id,
            session.remote_peer_id,
            session.profile.label()
        );
        let (control_tx, control_rx) = tokio::sync::mpsc::channel(16);
        self.screen_broadcast_worker_control_tx = Some(control_tx);
        self.screen_broadcast_worker_handle = Some(start_screen_broadcast_worker(
            session.session_id.clone(),
            session.profile,
            self.app_handle.clone(),
            self.screen_broadcast_worker_event_tx.clone(),
            control_rx,
        ));
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

    pub(super) async fn handle_start_screen_broadcast(
        &mut self,
        peer_chat_id: String,
        profile: rchat_screen_capture::ScreenCaptureProfile,
    ) {
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
            profile,
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

                if self.active_broadcast.is_some()
                    || self.broadcast_conflict_reason(&incoming_chat_id).is_some()
                {
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
                    profile: rchat_screen_capture::ScreenCaptureProfile::default(),
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
                    return;
                }
            }
        }

        let Some(session) = self.active_host_broadcast_snapshot() else {
            if self.screen_broadcast_worker_handle.is_some()
                || self.screen_broadcast_stream_tx.is_some()
                || self.screen_capture_session.is_some()
                || self.screen_capture_start_task.is_some()
            {
                self.stop_screen_broadcast_media("final");
            }
            return;
        };

        if !self.peer_has_quic_path(&session.remote_peer_id) {
            self.transition_broadcast_to_idle(Some("quic_path_lost".to_string()))
                .await;
            return;
        }

        if self.screen_broadcast_stream_tx.is_none()
            || self.screen_broadcast_stream_session_id.as_deref()
                != Some(session.session_id.as_str())
        {
            let _ = self.start_screen_broadcast_stream_writer(
                session.remote_peer_id,
                session.session_id.clone(),
            );
        }
        self.ensure_screen_broadcast_worker(&session);
        self.maybe_log_screen_broadcast_summary();
    }

    pub(super) async fn handle_screen_broadcast_worker_event(
        &mut self,
        event: ScreenBroadcastWorkerEvent,
    ) {
        match event {
            ScreenBroadcastWorkerEvent::Started { session_id, info } => {
                if self
                    .active_broadcast
                    .as_ref()
                    .map(|session| {
                        session.session_id == session_id
                            && session.phase == ActiveBroadcastPhase::Active
                            && session.is_host
                    })
                    .unwrap_or(false)
                {
                    eprintln!(
                        "[Broadcast][Screen] capture started session={} backend={} source='{}' format={} {}x{}@{}",
                        session_id,
                        info.backend.label(),
                        info.source_label,
                        info.format.format,
                        info.format.width,
                        info.format.height,
                        info.format.fps
                    );
                    self.screen_capture_info = Some(info);
                    self.screen_capture_started_at = Some(Instant::now());
                    self.screen_broadcast_worker_stats.started_at = Some(Instant::now());
                }
            }
            ScreenBroadcastWorkerEvent::EncodedFrame {
                session_id,
                frame,
                encode_micros,
            } => {
                if !self
                    .active_broadcast
                    .as_ref()
                    .map(|session| {
                        session.session_id == session_id
                            && session.phase == ActiveBroadcastPhase::Active
                            && session.is_host
                    })
                    .unwrap_or(false)
                {
                    return;
                }
                self.screen_broadcast_worker_stats.encoded_frames = self
                    .screen_broadcast_worker_stats
                    .encoded_frames
                    .saturating_add(1);
                self.screen_broadcast_worker_stats.outbound_bytes = self
                    .screen_broadcast_worker_stats
                    .outbound_bytes
                    .saturating_add(frame.payload.len() as u64);
                self.screen_broadcast_worker_stats
                    .encode_micros
                    .push(encode_micros);
                match frame.chunk_type {
                    BroadcastChunkType::Key => {
                        self.screen_broadcast_worker_stats.keyframes = self
                            .screen_broadcast_worker_stats
                            .keyframes
                            .saturating_add(1);
                    }
                    BroadcastChunkType::Delta => {
                        self.screen_broadcast_worker_stats.delta_frames = self
                            .screen_broadcast_worker_stats
                            .delta_frames
                            .saturating_add(1);
                    }
                }
                let queued =
                    self.queue_screen_broadcast_stream_record(BroadcastStreamRecord::Frame(frame));
                if !queued {
                    self.request_screen_broadcast_worker_keyframe();
                }
            }
            ScreenBroadcastWorkerEvent::Stats {
                session_id,
                info,
                stats,
            } => {
                if self
                    .active_broadcast
                    .as_ref()
                    .map(|session| session.session_id == session_id && session.is_host)
                    .unwrap_or(false)
                {
                    self.screen_capture_info = Some(info);
                    self.screen_broadcast_worker_stats = stats;
                }
            }
            ScreenBroadcastWorkerEvent::Failure { session_id, error } => {
                let Some(session) = self.active_broadcast.as_ref().cloned() else {
                    return;
                };
                if session.session_id == session_id && session.is_host {
                    self.screen_broadcast_stats.capture_start_failures = self
                        .screen_broadcast_stats
                        .capture_start_failures
                        .saturating_add(1);
                    self.fail_active_screen_capture(&session, error).await;
                }
            }
        }
    }

    pub(super) async fn handle_screen_broadcast_stream_event(
        &mut self,
        event: ScreenBroadcastStreamEvent,
    ) {
        match event {
            ScreenBroadcastStreamEvent::InboundRecord {
                peer,
                session_id,
                record,
            } => match record {
                BroadcastStreamRecord::Frame(frame) => {
                    if !self
                        .active_broadcast
                        .as_ref()
                        .map(|session| {
                            session.phase == ActiveBroadcastPhase::Active
                                && session.session_id == session_id
                                && session.remote_peer_id == peer
                                && !session.is_host
                        })
                        .unwrap_or(false)
                    {
                        return;
                    }
                    self.screen_broadcast_stats.inbound_frames =
                        self.screen_broadcast_stats.inbound_frames.saturating_add(1);
                    self.screen_broadcast_stats.inbound_bytes = self
                        .screen_broadcast_stats
                        .inbound_bytes
                        .saturating_add(frame.payload.len() as u64);
                    let event = BroadcastFrameEvent {
                        session_id,
                        peer_id: peer.to_string(),
                        seq: frame.seq,
                        timestamp: frame.timestamp_us,
                        mime: SCREEN_BROADCAST_MIME.to_string(),
                        codec: SCREEN_BROADCAST_CODEC.to_string(),
                        profile: frame.profile.label().to_string(),
                        width: frame.width,
                        height: frame.height,
                        chunk_type: frame.chunk_type,
                        payload: frame.payload,
                    };
                    let _ = self.app_handle.emit("broadcast-frame", event);
                }
            },
            ScreenBroadcastStreamEvent::InboundFailure {
                peer,
                session_id,
                error,
            } => {
                eprintln!(
                    "[Broadcast][Stream] inbound failure from {}: {}",
                    peer, error
                );
                self.screen_broadcast_stats.inbound_failures = self
                    .screen_broadcast_stats
                    .inbound_failures
                    .saturating_add(1);
                if self
                    .active_broadcast
                    .as_ref()
                    .map(|session| {
                        session.phase == ActiveBroadcastPhase::Active
                            && session.remote_peer_id == peer
                            && session_id.as_deref() == Some(session.session_id.as_str())
                    })
                    .unwrap_or(false)
                {
                    self.transition_broadcast_to_idle(Some("stream_failure".to_string()))
                        .await;
                }
            }
            ScreenBroadcastStreamEvent::OutboundFailure {
                peer,
                session_id,
                error,
            } => {
                eprintln!(
                    "[Broadcast][Stream] outbound failure to {}: {}",
                    peer, error
                );
                self.screen_broadcast_stats.outbound_failures = self
                    .screen_broadcast_stats
                    .outbound_failures
                    .saturating_add(1);
                if self.screen_broadcast_stream_session_id.as_deref() == Some(session_id.as_str()) {
                    self.screen_broadcast_stream_tx = None;
                    self.screen_broadcast_stream_session_id = None;
                    self.screen_broadcast_stream_writer_handle = None;
                }
                if self
                    .active_broadcast
                    .as_ref()
                    .map(|session| {
                        session.phase == ActiveBroadcastPhase::Active
                            && session.remote_peer_id == peer
                            && session.session_id == session_id
                    })
                    .unwrap_or(false)
                {
                    self.transition_broadcast_to_idle(Some("stream_failure".to_string()))
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
                    }
                }
            },
            Event::OutboundFailure { peer, error, .. } => {
                self.screen_broadcast_stats.outbound_failures = self
                    .screen_broadcast_stats
                    .outbound_failures
                    .saturating_add(1);
                eprintln!(
                    "[Broadcast] Legacy outbound frame failure to {}: {:?}",
                    peer, error
                );
            }
            Event::InboundFailure { peer, error, .. } => {
                self.screen_broadcast_stats.inbound_failures = self
                    .screen_broadcast_stats
                    .inbound_failures
                    .saturating_add(1);
                eprintln!(
                    "[Broadcast] Legacy inbound frame failure from {}: {:?}",
                    peer, error
                );
            }
            Event::ResponseSent { .. } => {}
        }
    }
}
