use super::*;
use crate::app_state::{CallKind, VoiceCallPhase};
use crate::live::video::codec::{
    prepare_i420_for_profile, should_force_video_keyframe, RgbaVideoFrame, VideoAdaptationWindow,
    VideoProfile, VideoQualityChangeDecision, VideoQualityMode, VideoReceiverPreferenceWindow,
    Vp8EncodedPacket, Vp8VideoDecoder, Vp8VideoEncoder,
};
use crate::live::video::protocol::{
    read_video_stream_header, read_video_stream_record, write_video_stream_header,
    write_video_stream_record, VideoCameraState, VideoChunkType, VideoQualityChange,
    VideoReceiverReport, VideoStreamFrame, VideoStreamRecord,
};
use crate::network::direct_message::{DirectMessageKind, DirectMessageRequest};
use futures::AsyncWriteExt as _;
use rchat_video_capture::{CaptureConfig, CaptureProfile, VideoCaptureError, VideoCaptureSession};
use serde::Serialize;

const CALL_RING_TIMEOUT_SECS: u64 = 30;
const VIDEO_STREAM_QUEUE_CAPACITY: usize = 8;
const VIDEO_ENCODE_QUEUE_CAPACITY: usize = 2;
const VIDEO_ENCODE_EVENT_QUEUE_CAPACITY: usize = 8;
const VIDEO_REMOTE_DECODE_QUEUE_CAPACITY: usize = 4;
const VIDEO_SUMMARY_INTERVAL: std::time::Duration = std::time::Duration::from_secs(5);
const VIDEO_RECEIVER_REQUEST_REASON: &str = "receiver_request";

#[derive(Debug, Clone, Serialize)]
struct VideoQualityEvent {
    call_id: String,
    mode: String,
    profile: String,
    reason: String,
}

#[derive(Debug, Clone, Serialize)]
struct VideoCameraStateEvent {
    call_id: String,
    peer_id: String,
    enabled: bool,
}

#[derive(Debug, Clone, Serialize)]
struct VideoLocalPreviewFrameEvent {
    call_id: String,
    timestamp_us: i64,
    width: u32,
    height: u32,
    rgba: Vec<u8>,
}

#[derive(Debug, Clone, Serialize)]
struct VideoRemoteFrameEvent {
    call_id: String,
    peer_id: String,
    seq: u32,
    timestamp: i64,
    width: u32,
    height: u32,
    rgba: Vec<u8>,
}

#[derive(Debug, Clone, Serialize)]
struct VideoCameraErrorEvent {
    call_id: String,
    message: String,
}

pub(super) struct VideoCaptureStartTask {
    call_id: String,
    profile_label: String,
    handle: tokio::task::JoinHandle<Result<VideoCaptureSession, VideoCaptureError>>,
}

pub(super) struct OutboundVideoEncodeFrame {
    call_id: String,
    generation: u64,
    timestamp_us: i64,
    width: u32,
    height: u32,
    profile: VideoProfile,
    data: Vec<u8>,
    force_keyframe: bool,
}

pub(super) enum OutboundVideoEncodeTask {
    Frame(OutboundVideoEncodeFrame),
    Reset,
}

pub(super) struct OutboundVideoEncodeOutput {
    call_id: String,
    generation: u64,
    timestamp_us: i64,
    width: u32,
    height: u32,
    profile: VideoProfile,
    packets: Vec<Vp8EncodedPacket>,
    encode_micros: u64,
}

pub(super) enum OutboundVideoEncodeEvent {
    Encoded(OutboundVideoEncodeOutput),
    EncodeError {
        call_id: String,
        generation: u64,
        error: String,
    },
}

pub(super) enum RemoteVideoDecodeTask {
    Frame {
        peer: PeerId,
        call_id: String,
        frame: VideoStreamFrame,
    },
    Reset,
}

#[derive(Default)]
struct OutboundVideoEncoderState {
    encoder: Option<Vp8VideoEncoder>,
    encoded_packet_count: u32,
}

impl OutboundVideoEncoderState {
    fn reset(&mut self) {
        self.encoder = None;
        self.encoded_packet_count = 0;
    }

    fn encode_frame(
        &mut self,
        frame: OutboundVideoEncodeFrame,
    ) -> Result<OutboundVideoEncodeOutput, String> {
        let OutboundVideoEncodeFrame {
            call_id,
            generation,
            timestamp_us,
            width: captured_width,
            height: captured_height,
            profile,
            data,
            force_keyframe,
        } = frame;
        let prepared = prepare_i420_for_profile(&data, captured_width, captured_height, profile)?;
        let width = prepared.width;
        let height = prepared.height;
        let data = prepared.data;

        let needs_encoder = self
            .encoder
            .as_ref()
            .map(|encoder| {
                encoder.profile() != profile
                    || encoder.width() != width
                    || encoder.height() != height
            })
            .unwrap_or(true);
        if needs_encoder {
            self.encoder = Some(Vp8VideoEncoder::new_with_dimensions(
                profile, width, height,
            )?);
            self.encoded_packet_count = 0;
        }

        let force_keyframe =
            should_force_video_keyframe(force_keyframe, needs_encoder, self.encoded_packet_count);
        let encode_started = std::time::Instant::now();
        let packets = match self.encoder.as_mut() {
            Some(encoder) => {
                encoder.encode_i420(timestamp_us, width, height, &data, force_keyframe)
            }
            None => Err("VP8 encoder unavailable".to_string()),
        }?;
        let encode_micros = encode_started.elapsed().as_micros().min(u64::MAX as u128) as u64;
        self.encoded_packet_count = self
            .encoded_packet_count
            .wrapping_add(packets.len().try_into().unwrap_or(u32::MAX));

        Ok(OutboundVideoEncodeOutput {
            call_id,
            generation,
            timestamp_us,
            width,
            height,
            profile,
            packets,
            encode_micros,
        })
    }
}

pub(super) fn start_outbound_video_encode_worker() -> (
    tokio::sync::mpsc::Sender<OutboundVideoEncodeTask>,
    tokio::sync::mpsc::Receiver<OutboundVideoEncodeEvent>,
    tauri::async_runtime::JoinHandle<()>,
) {
    let (task_tx, mut task_rx) =
        tokio::sync::mpsc::channel::<OutboundVideoEncodeTask>(VIDEO_ENCODE_QUEUE_CAPACITY);
    let (event_tx, event_rx) =
        tokio::sync::mpsc::channel::<OutboundVideoEncodeEvent>(VIDEO_ENCODE_EVENT_QUEUE_CAPACITY);
    let handle = tauri::async_runtime::spawn(async move {
        let mut state = OutboundVideoEncoderState::default();
        while let Some(task) = task_rx.recv().await {
            match task {
                OutboundVideoEncodeTask::Reset => state.reset(),
                OutboundVideoEncodeTask::Frame(frame) => {
                    let call_id = frame.call_id.clone();
                    let generation = frame.generation;
                    let current_state = std::mem::take(&mut state);
                    let result = tauri::async_runtime::spawn_blocking(move || {
                        let mut state = current_state;
                        let encoded = state.encode_frame(frame);
                        (state, encoded)
                    })
                    .await;

                    let event = match result {
                        Ok((next_state, Ok(output))) => {
                            state = next_state;
                            OutboundVideoEncodeEvent::Encoded(output)
                        }
                        Ok((next_state, Err(error))) => {
                            state = next_state;
                            OutboundVideoEncodeEvent::EncodeError {
                                call_id,
                                generation,
                                error,
                            }
                        }
                        Err(error) => {
                            state.reset();
                            OutboundVideoEncodeEvent::EncodeError {
                                call_id,
                                generation,
                                error: format!("VP8 encode worker failed: {}", error),
                            }
                        }
                    };

                    if event_tx.send(event).await.is_err() {
                        break;
                    }
                }
            }
        }
    });
    (task_tx, event_rx, handle)
}

#[derive(Default)]
struct RemoteVideoDecoderState {
    call_id: Option<String>,
    decoder: Option<Vp8VideoDecoder>,
}

impl RemoteVideoDecoderState {
    fn reset(&mut self) {
        self.call_id = None;
        self.decoder = None;
    }

    fn decode_frame(
        &mut self,
        call_id: &str,
        frame: &VideoStreamFrame,
    ) -> Result<Option<RgbaVideoFrame>, String> {
        if self.call_id.as_deref() != Some(call_id) {
            self.call_id = Some(call_id.to_string());
            self.decoder = None;
        }

        if frame.chunk_type == VideoChunkType::Key {
            self.decoder = Some(Vp8VideoDecoder::new()?);
        } else if self.decoder.is_none() {
            return Ok(None);
        }

        let decoded = match self.decoder.as_mut() {
            Some(decoder) => decoder.decode_rgba(&frame.payload),
            None => Err("VP8 decoder unavailable".to_string()),
        };
        match decoded {
            Ok(frame) => Ok(Some(frame)),
            Err(e) => {
                self.decoder = None;
                Err(e)
            }
        }
    }
}

pub(super) fn start_remote_video_decode_worker(
    app_handle: AppHandle,
) -> (
    tokio::sync::mpsc::Sender<RemoteVideoDecodeTask>,
    tauri::async_runtime::JoinHandle<()>,
) {
    let (tx, mut rx) =
        tokio::sync::mpsc::channel::<RemoteVideoDecodeTask>(VIDEO_REMOTE_DECODE_QUEUE_CAPACITY);
    let handle = tauri::async_runtime::spawn(async move {
        let mut state = RemoteVideoDecoderState::default();
        while let Some(task) = rx.recv().await {
            match task {
                RemoteVideoDecodeTask::Reset => state.reset(),
                RemoteVideoDecodeTask::Frame {
                    peer,
                    call_id,
                    frame,
                } => {
                    let peer_id = peer.to_string();
                    let seq = frame.seq;
                    let timestamp = frame.timestamp_us;
                    let kind = frame.chunk_type;
                    let bytes = frame.payload.len();
                    let call_id_for_log = call_id.clone();
                    let peer_id_for_log = peer_id.clone();
                    let current_state = std::mem::take(&mut state);
                    let result = tauri::async_runtime::spawn_blocking(move || {
                        let mut state = current_state;
                        let decoded = state.decode_frame(&call_id, &frame).map(|frame| {
                            frame.map(|decoded| VideoRemoteFrameEvent {
                                call_id,
                                peer_id,
                                seq,
                                timestamp,
                                width: decoded.width,
                                height: decoded.height,
                                rgba: decoded.rgba,
                            })
                        });
                        (state, decoded)
                    })
                    .await;

                    match result {
                        Ok((next_state, Ok(Some(event)))) => {
                            state = next_state;
                            let _ = app_handle.emit("video-call-remote-frame", event);
                        }
                        Ok((next_state, Ok(None))) => {
                            state = next_state;
                        }
                        Ok((next_state, Err(e))) => {
                            state = next_state;
                            eprintln!(
                                "[Video][Codec] inbound VP8 decode failed peer={} call_id={} seq={} kind={:?} bytes={} error={}",
                                peer_id_for_log, call_id_for_log, seq, kind, bytes, e
                            );
                        }
                        Err(e) => {
                            state.reset();
                            eprintln!(
                                "[Video][Codec] inbound VP8 decode worker failed peer={} call_id={} seq={} error={}",
                                peer_id_for_log, call_id_for_log, seq, e
                            );
                        }
                    }
                }
            }
        }
    });
    (tx, handle)
}

fn take_finished_video_capture_start(
    task: &mut Option<VideoCaptureStartTask>,
) -> Option<VideoCaptureStartTask> {
    if task
        .as_ref()
        .map(|task| task.handle.is_finished())
        .unwrap_or(false)
    {
        task.take()
    } else {
        None
    }
}

fn effective_video_profile(
    local_profile: VideoProfile,
    remote_requested_profile: VideoProfile,
) -> VideoProfile {
    local_profile.min(remote_requested_profile)
}

fn build_video_receiver_report(
    pending_inbound_frames: &mut u64,
    frontend_received_frames: u64,
    rendered_frames: u64,
    dropped_frames: u64,
    decode_errors: u64,
) -> VideoReceiverReport {
    let received_frames = if *pending_inbound_frames > 0 {
        std::mem::take(pending_inbound_frames)
    } else {
        frontend_received_frames
    };
    VideoReceiverReport {
        received_frames,
        rendered_frames,
        dropped_frames,
        decode_errors,
    }
}

fn should_restart_running_video_capture_for_profile_change(
    current_requested_profile: Option<&str>,
    _current_profile: VideoProfile,
) -> bool {
    current_requested_profile.is_none()
}

impl NetworkManager {
    pub(super) fn reset_video_network_diagnostics(&mut self) {
        self.video_network_stats.reset();
        self.video_window_counters.reset();
        self.video_expected_inbound_seq = None;
        self.video_receiver_report_pending_inbound_frames = 0;
        self.video_window_started_at = Some(std::time::Instant::now());
        self.video_last_summary_at = Some(std::time::Instant::now());
    }

    fn effective_outbound_video_profile(&self) -> VideoProfile {
        effective_video_profile(
            self.video_quality_controller.current_profile(),
            self.video_remote_requested_profile,
        )
    }

    fn reset_outbound_video_encoder(&mut self) {
        self.video_encode_generation = self.video_encode_generation.wrapping_add(1);
        self.video_force_next_keyframe = true;
        let _ = self
            .video_encode_tx
            .try_send(OutboundVideoEncodeTask::Reset);
    }

    pub(super) fn stop_video_media(&mut self) {
        if let Some(call) = self.active_call.as_ref() {
            if call.kind == CallKind::Video {
                let peer = call.remote_peer_id;
                self.log_video_network_summary("final", &peer);
            }
        }
        self.stop_video_capture();
        self.video_stream_tx = None;
        self.video_stream_call_id = None;
        if let Some(handle) = self.video_stream_writer_handle.take() {
            handle.abort();
        }
        self.video_next_seq = 0;
        self.video_force_next_keyframe = true;
        self.video_expected_inbound_seq = None;
        self.reset_outbound_video_encoder();
        let _ = self
            .video_remote_decode_tx
            .try_send(RemoteVideoDecodeTask::Reset);
        self.video_quality_controller =
            crate::live::video::codec::VideoQualityController::new(VideoQualityMode::Auto);
        self.video_remote_requested_profile = VideoProfile::P720;
        self.video_receiver_preference_controller =
            crate::live::video::codec::VideoReceiverPreferenceController::default();
        self.reset_video_network_diagnostics();
    }

    pub(super) fn start_video_media(
        &mut self,
        peer: PeerId,
        call_id: String,
        camera_enabled: bool,
    ) -> bool {
        self.reset_video_network_diagnostics();
        self.video_remote_requested_profile = VideoProfile::P720;
        self.video_receiver_preference_controller =
            crate::live::video::codec::VideoReceiverPreferenceController::default();
        self.reset_outbound_video_encoder();
        let _ = self
            .video_remote_decode_tx
            .try_send(RemoteVideoDecodeTask::Reset);
        let started = self.start_video_stream_writer(peer, call_id.clone());
        if started {
            self.queue_video_stream_record(VideoStreamRecord::CameraState(VideoCameraState {
                enabled: camera_enabled,
            }));
            self.queue_video_stream_record(VideoStreamRecord::QualityChange(VideoQualityChange {
                profile: self.video_quality_controller.current_profile(),
                reason: "initial".to_string(),
            }));
            self.queue_receiver_video_profile_request();
            self.emit_video_quality_event(&call_id, "initial");
        }
        started
    }

    fn stop_video_capture(&mut self) {
        if let Some(task) = self.video_capture_start_task.take() {
            task.handle.abort();
        }
        if let Some(session) = self.video_capture_session.take() {
            self.video_capture_last_stats = session.stats();
        }
        self.video_capture_info = None;
        self.video_capture_started_at = None;
    }

    async fn ensure_video_capture_running(&mut self, call_snapshot: &ActiveCall) {
        if call_snapshot.phase != ActiveCallPhase::Active
            || call_snapshot.kind != CallKind::Video
            || !call_snapshot.camera_enabled
        {
            self.stop_video_capture();
            return;
        }

        self.complete_video_capture_start_if_ready(call_snapshot)
            .await;
        if self
            .active_call
            .as_ref()
            .map(|call| call.call_id == call_snapshot.call_id && !call.camera_enabled)
            .unwrap_or(false)
        {
            self.stop_video_capture();
            return;
        }

        let current_profile = self.video_quality_controller.current_profile();
        let needs_restart = self
            .video_capture_info
            .as_ref()
            .map(|info| {
                should_restart_running_video_capture_for_profile_change(
                    Some(info.requested_profile.as_str()),
                    current_profile,
                )
            })
            .unwrap_or(true);
        if self.video_capture_session.is_some() && !needs_restart {
            return;
        }
        if self
            .video_capture_start_task
            .as_ref()
            .map(|task| {
                task.call_id == call_snapshot.call_id
                    && task.profile_label == current_profile.label()
            })
            .unwrap_or(false)
        {
            return;
        }
        self.stop_video_capture();

        let config =
            CaptureConfig::default_for_profile(capture_profile_from_video_profile(current_profile));
        eprintln!(
            "[Video][Capture] start queued call_id={} requested_profile={}",
            call_snapshot.call_id,
            current_profile.label(),
        );
        let handle = tokio::task::spawn_blocking(move || VideoCaptureSession::start(config));
        self.video_capture_start_task = Some(VideoCaptureStartTask {
            call_id: call_snapshot.call_id.clone(),
            profile_label: current_profile.label().to_string(),
            handle,
        });
    }

    async fn complete_video_capture_start_if_ready(&mut self, call_snapshot: &ActiveCall) {
        let Some(task) = take_finished_video_capture_start(&mut self.video_capture_start_task)
        else {
            return;
        };
        let task_call_id = task.call_id.clone();
        let task_profile_label = task.profile_label.clone();
        let result = match task.handle.await {
            Ok(result) => result,
            Err(error) => Err(VideoCaptureError::Backend(error.to_string())),
        };
        let current_profile = self.video_quality_controller.current_profile();
        if task_call_id != call_snapshot.call_id
            || call_snapshot.phase != ActiveCallPhase::Active
            || call_snapshot.kind != CallKind::Video
            || !call_snapshot.camera_enabled
            || task_profile_label != current_profile.label()
        {
            return;
        }

        match result {
            Ok(session) => {
                let info = session.info().clone();
                eprintln!(
                    "[Video][Capture] started backend={} device='{}' requested_profile={} actual={}x{}@{} format={}",
                    info.backend,
                    info.device_name,
                    info.requested_profile,
                    info.format.width,
                    info.format.height,
                    info.format.fps,
                    info.format.format,
                );
                self.video_capture_info = Some(info);
                self.video_capture_started_at = Some(std::time::Instant::now());
                self.video_capture_last_stats = rchat_video_capture::CaptureSessionStats::default();
                self.video_capture_session = Some(session);
            }
            Err(error) => {
                self.handle_video_capture_start_failure(call_snapshot, error)
                    .await;
            }
        }
    }

    async fn handle_video_capture_start_failure(
        &mut self,
        call_snapshot: &ActiveCall,
        error: VideoCaptureError,
    ) {
        self.video_network_stats.capture_start_failures = self
            .video_network_stats
            .capture_start_failures
            .saturating_add(1);
        let message = error.to_string();
        eprintln!(
            "[Video][Capture] start failed call_id={} error={}",
            call_snapshot.call_id, message
        );
        self.stop_video_capture();
        self.queue_video_stream_record(VideoStreamRecord::CameraState(VideoCameraState {
            enabled: false,
        }));
        self.emit_video_camera_error(&call_snapshot.call_id, &message);
        let mut updated = call_snapshot.clone();
        updated.camera_enabled = false;
        self.active_call = Some(updated.clone());
        self.push_active_call_state(&updated, VoiceCallPhase::Active, None)
            .await;
    }

    fn emit_video_camera_error(&self, call_id: &str, message: &str) {
        let event = VideoCameraErrorEvent {
            call_id: call_id.to_string(),
            message: message.to_string(),
        };
        let _ = self.app_handle.emit("video-call-camera-error", event);
    }

    fn pump_native_video_capture(&mut self, call_id: &str) {
        let Some(session) = self.video_capture_session.as_ref() else {
            return;
        };
        let preview = session.try_recv_latest_preview();
        let frame = session.try_recv_latest_i420();
        if let Some(preview) = preview {
            let event = VideoLocalPreviewFrameEvent {
                call_id: call_id.to_string(),
                timestamp_us: preview.timestamp_us,
                width: preview.width,
                height: preview.height,
                rgba: preview.rgba,
            };
            let _ = self
                .app_handle
                .emit("video-call-local-preview-frame", event);
        }
        if let Some(frame) = frame {
            self.queue_outbound_video_encode_frame(
                call_id,
                frame.timestamp_us,
                frame.width,
                frame.height,
                frame.data,
            );
        }
    }

    pub(super) fn start_video_stream_writer(&mut self, peer: PeerId, call_id: String) -> bool {
        if self.video_stream_tx.is_some()
            && self.video_stream_call_id.as_deref() == Some(call_id.as_str())
        {
            return true;
        }

        let Some(connection_id) = self.voice_quic_connection_id(&peer) else {
            eprintln!(
                "[Video][QUIC] No QUIC connection id available for video stream: peer={}",
                peer
            );
            return false;
        };

        self.video_stream_tx = None;
        self.video_stream_call_id = None;
        if let Some(handle) = self.video_stream_writer_handle.take() {
            handle.abort();
        }

        eprintln!(
            "[Video][Stream] selected outbound QUIC connection peer={} call_id={} connection_id={:?}",
            peer, call_id, connection_id
        );

        let (tx, mut rx) =
            tokio::sync::mpsc::channel::<VideoStreamRecord>(VIDEO_STREAM_QUEUE_CAPACITY);
        let stream_rx = match self
            .swarm
            .behaviour_mut()
            .video_call
            .open_stream_on_connection(peer, connection_id)
        {
            Ok(stream_rx) => stream_rx,
            Err(e) => {
                eprintln!(
                    "[Video][QUIC] Failed to queue video stream on {} for {}: {}",
                    connection_id, peer, e
                );
                return false;
            }
        };
        let event_tx = self.video_stream_event_tx.clone();
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
                            "[Video][Stream] outbound stream opened peer={} call_id={} connection_id={:?}",
                            peer, writer_call_id, connection_id
                        );
                    stream
                }
                Ok(Ok(Err(e))) => {
                    let _ = event_tx
                        .send(VideoStreamEvent::OutboundFailure {
                            peer,
                            call_id: writer_call_id.clone(),
                            error: e.to_string(),
                        })
                        .await;
                    return;
                }
                Ok(Err(_)) => {
                    let _ = event_tx
                        .send(VideoStreamEvent::OutboundFailure {
                            peer,
                            call_id: writer_call_id.clone(),
                            error: "stream open canceled".to_string(),
                        })
                        .await;
                    return;
                }
                Err(e) => {
                    let _ = event_tx
                        .send(VideoStreamEvent::OutboundFailure {
                            peer,
                            call_id: writer_call_id.clone(),
                            error: format!("stream open timed out: {}", e),
                        })
                        .await;
                    return;
                }
            };

            if let Err(e) = write_video_stream_header(&mut stream, &writer_call_id).await {
                let _ = event_tx
                    .send(VideoStreamEvent::OutboundFailure {
                        peer,
                        call_id: writer_call_id.clone(),
                        error: e.to_string(),
                    })
                    .await;
                return;
            }
            eprintln!(
                "[Video][Stream] outbound header written peer={} call_id={} connection_id={:?}",
                peer, writer_call_id, connection_id
            );

            let mut first_frame_written = false;
            while let Some(record) = rx.recv().await {
                let frame_log = match &record {
                    VideoStreamRecord::Frame(frame) => {
                        Some((frame.seq, frame.payload.len(), frame.chunk_type))
                    }
                    _ => None,
                };
                if let Err(e) = write_video_stream_record(&mut stream, &record).await {
                    let _ = event_tx
                        .send(VideoStreamEvent::OutboundFailure {
                            peer,
                            call_id: writer_call_id.clone(),
                            error: e.to_string(),
                        })
                        .await;
                    return;
                }
                if let Some((seq, bytes, chunk_type)) = frame_log {
                    if !first_frame_written {
                        eprintln!(
                            "[Video][Stream] outbound first frame written peer={} call_id={} seq={} bytes={} kind={:?} connection_id={:?}",
                            peer, writer_call_id, seq, bytes, chunk_type, connection_id
                        );
                        first_frame_written = true;
                    }
                }
            }

            let _ = stream.close().await;
        });

        self.video_stream_tx = Some(tx);
        self.video_stream_call_id = Some(call_id);
        self.video_stream_writer_handle = Some(handle);
        self.reset_outbound_video_encoder();
        true
    }

    fn queue_video_stream_record(&mut self, record: VideoStreamRecord) -> bool {
        let Some(tx) = self.video_stream_tx.as_ref() else {
            return false;
        };
        match tx.try_send(record) {
            Ok(()) => true,
            Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
                self.video_network_stats.encoded_queue_drops += 1;
                self.video_window_counters.encoded_queue_drops += 1;
                false
            }
            Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
                self.video_network_stats.outbound_failures += 1;
                self.video_stream_tx = None;
                self.video_stream_call_id = None;
                false
            }
        }
    }

    fn emit_video_quality_event(&self, call_id: &str, reason: &str) {
        let event = VideoQualityEvent {
            call_id: call_id.to_string(),
            mode: self.video_quality_controller.mode().label().to_string(),
            profile: self.effective_outbound_video_profile().label().to_string(),
            reason: reason.to_string(),
        };
        let _ = self.app_handle.emit("video-call-quality-updated", event);
    }

    fn queue_receiver_video_profile_request(&mut self) -> bool {
        self.queue_video_stream_record(VideoStreamRecord::QualityChange(VideoQualityChange {
            profile: self.video_receiver_preference_controller.current_profile(),
            reason: VIDEO_RECEIVER_REQUEST_REASON.to_string(),
        }))
    }

    fn emit_remote_camera_state(&self, call_id: &str, peer_id: &PeerId, enabled: bool) {
        let event = VideoCameraStateEvent {
            call_id: call_id.to_string(),
            peer_id: peer_id.to_string(),
            enabled,
        };
        let _ = self.app_handle.emit("video-call-camera-state", event);
    }

    fn queue_remote_video_decode_frame(
        &mut self,
        peer: PeerId,
        call_id: String,
        frame: VideoStreamFrame,
    ) {
        match self
            .video_remote_decode_tx
            .try_send(RemoteVideoDecodeTask::Frame {
                peer,
                call_id,
                frame,
            }) {
            Ok(()) => {}
            Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
                self.video_network_stats.local_dropped_frames = self
                    .video_network_stats
                    .local_dropped_frames
                    .saturating_add(1);
            }
            Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
                self.video_network_stats.local_decode_errors = self
                    .video_network_stats
                    .local_decode_errors
                    .saturating_add(1);
            }
        }
    }

    pub(super) async fn handle_start_video_call(&mut self, peer_chat_id: String) {
        if self.active_broadcast.is_some() {
            self.push_idle_call_state(Some("broadcast_conflict".to_string()))
                .await;
            return;
        }

        if let Some(call_snapshot) = self.active_call.as_ref().cloned() {
            let same_active_voice = call_snapshot.phase == ActiveCallPhase::Active
                && call_snapshot.kind == CallKind::Voice
                && call_snapshot.peer_chat_id == peer_chat_id;
            if same_active_voice {
                if !self.peer_has_quic_path(&call_snapshot.remote_peer_id) {
                    self.transition_to_idle(Some("quic_required".to_string()))
                        .await;
                    return;
                }
                self.send_call_signal(
                    call_snapshot.remote_peer_id,
                    DirectMessageKind::CallOfferVideo,
                    &call_snapshot.call_id,
                );
                let mut updated = call_snapshot;
                updated.kind = CallKind::Video;
                updated.camera_enabled = true;
                self.active_call = Some(updated.clone());
                self.start_video_media(
                    updated.remote_peer_id,
                    updated.call_id.clone(),
                    updated.camera_enabled,
                );
                self.push_active_call_state(&updated, VoiceCallPhase::Active, None)
                    .await;
                return;
            }

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
            text_content: Some(call_id),
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
        updated.camera_enabled = false;
        self.active_call = Some(updated.clone());
        let _ = self.start_voice_stream_writer(updated.remote_peer_id, updated.call_id.clone());
        self.start_video_media(
            updated.remote_peer_id,
            updated.call_id.clone(),
            updated.camera_enabled,
        );
        self.push_active_call_state(&updated, VoiceCallPhase::Active, None)
            .await;
    }

    pub(super) async fn handle_reject_video_call(&mut self, call_id: String) {
        let Some(call) = self.active_call.as_ref().cloned() else {
            return;
        };
        if call.call_id != call_id || call.kind != CallKind::Video {
            return;
        }
        self.send_call_signal(
            call.remote_peer_id,
            DirectMessageKind::CallReject,
            &call.call_id,
        );
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
        self.send_call_signal(
            call.remote_peer_id,
            DirectMessageKind::CallEnd,
            &call.call_id,
        );
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
        self.queue_video_stream_record(VideoStreamRecord::CameraState(VideoCameraState {
            enabled,
        }));
        if !enabled {
            self.stop_video_capture();
            self.reset_outbound_video_encoder();
        } else {
            self.ensure_video_capture_running(&updated).await;
            if self
                .active_call
                .as_ref()
                .map(|call| call.call_id == updated.call_id && !call.camera_enabled)
                .unwrap_or(false)
            {
                return;
            }
        }
        if let Some(current) = self.active_call.as_ref().cloned() {
            self.push_active_call_state(&current, VoiceCallPhase::Active, None)
                .await;
        }
    }

    pub(super) async fn handle_set_video_call_quality(&mut self, call_id: String, mode: String) {
        let Some(call) = self.active_call.as_ref() else {
            return;
        };
        if call.call_id != call_id || call.kind != CallKind::Video {
            return;
        }
        let Some(mode) = VideoQualityMode::from_label(&mode) else {
            return;
        };
        if let Some(change) = self.video_quality_controller.set_mode(mode) {
            self.apply_video_quality_change(&call_id, change);
        }
    }

    pub(super) async fn handle_send_video_call_chunk(
        &mut self,
        _call_id: String,
        _seq: u32,
        _timestamp: i64,
        _mime: String,
        _codec: String,
        _chunk_type: String,
        _payload: Vec<u8>,
    ) {
        self.video_network_stats.raw_frames_dropped += 1;
    }

    pub(super) async fn handle_submit_video_call_i420_frame(
        &mut self,
        call_id: String,
        timestamp_us: i64,
        width: u32,
        height: u32,
        _profile: String,
        data: Vec<u8>,
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
        if self.video_stream_tx.is_none() {
            let _ = self.start_video_stream_writer(call_snapshot.remote_peer_id, call_id.clone());
        }

        self.queue_outbound_video_encode_frame(&call_id, timestamp_us, width, height, data);
    }

    fn queue_outbound_video_encode_frame(
        &mut self,
        call_id: &str,
        timestamp_us: i64,
        width: u32,
        height: u32,
        data: Vec<u8>,
    ) {
        self.video_network_stats.submitted_frames += 1;
        self.video_window_counters.submitted_frames += 1;

        let current_profile = self.effective_outbound_video_profile();
        let expected_len = Vp8VideoEncoder::expected_i420_len(width, height).unwrap_or(0);
        if expected_len == 0 || data.len() != expected_len {
            self.video_network_stats.raw_frames_dropped += 1;
            self.video_window_counters.raw_frames_dropped += 1;
            return;
        }

        let queued = self
            .video_encode_tx
            .try_send(OutboundVideoEncodeTask::Frame(OutboundVideoEncodeFrame {
                call_id: call_id.to_string(),
                generation: self.video_encode_generation,
                timestamp_us,
                width,
                height,
                profile: current_profile,
                data,
                force_keyframe: self.video_force_next_keyframe,
            }));
        match queued {
            Ok(()) => {}
            Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
                self.video_network_stats.raw_frames_dropped += 1;
                self.video_window_counters.raw_frames_dropped += 1;
            }
            Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
                self.video_network_stats.encode_errors += 1;
            }
        }
    }

    pub(super) fn handle_outbound_video_encode_event(&mut self, event: OutboundVideoEncodeEvent) {
        let output = match event {
            OutboundVideoEncodeEvent::Encoded(output) => output,
            OutboundVideoEncodeEvent::EncodeError {
                call_id,
                generation,
                error,
            } => {
                if generation == self.video_encode_generation
                    && self
                        .active_call
                        .as_ref()
                        .map(|call| call.call_id == call_id && call.kind == CallKind::Video)
                        .unwrap_or(false)
                {
                    eprintln!("[Video][Codec] VP8 encode failed: {}", error);
                    self.video_network_stats.encode_errors += 1;
                }
                return;
            }
        };
        if output.generation != self.video_encode_generation
            || self.video_stream_call_id.as_deref() != Some(output.call_id.as_str())
            || !self
                .active_call
                .as_ref()
                .map(|call| {
                    call.phase == ActiveCallPhase::Active
                        && call.kind == CallKind::Video
                        && call.call_id == output.call_id
                        && call.camera_enabled
                })
                .unwrap_or(false)
        {
            return;
        }

        self.video_window_counters
            .encode_micros
            .push(output.encode_micros);
        let mut queued_keyframe = false;
        let mut dropped_video_frame = false;
        for packet in output.packets {
            let packet_is_key = packet.is_key;
            let chunk_type = if packet.is_key {
                self.video_network_stats.keyframes += 1;
                VideoChunkType::Key
            } else {
                self.video_network_stats.delta_frames += 1;
                VideoChunkType::Delta
            };
            let payload_len = packet.payload.len() as u64;
            let record = VideoStreamRecord::Frame(VideoStreamFrame {
                seq: self.video_next_seq,
                timestamp_us: output.timestamp_us,
                chunk_type,
                profile: output.profile,
                width: output.width,
                height: output.height,
                payload: packet.payload,
            });
            self.video_next_seq = self.video_next_seq.wrapping_add(1);
            self.video_network_stats.encoded_frames += 1;
            self.video_network_stats.outbound_bytes = self
                .video_network_stats
                .outbound_bytes
                .saturating_add(payload_len);
            self.video_network_stats.last_encoded_width = Some(output.width);
            self.video_network_stats.last_encoded_height = Some(output.height);
            self.video_window_counters.encoded_frames += 1;
            let queued = self.queue_video_stream_record(record);
            if queued && packet_is_key {
                queued_keyframe = true;
            } else if !queued {
                dropped_video_frame = true;
            }
        }
        if queued_keyframe {
            self.video_force_next_keyframe = false;
        }
        if dropped_video_frame {
            self.video_force_next_keyframe = true;
        }
    }

    pub(super) async fn handle_report_video_call_render_stats(
        &mut self,
        call_id: String,
        received_frames: u64,
        rendered_frames: u64,
        dropped_frames: u64,
        decode_errors: u64,
    ) {
        let Some(call) = self.active_call.as_ref() else {
            return;
        };
        if call.call_id != call_id || call.kind != CallKind::Video {
            return;
        }
        self.video_network_stats.local_rendered_frames = self
            .video_network_stats
            .local_rendered_frames
            .saturating_add(rendered_frames);
        self.video_network_stats.local_dropped_frames = self
            .video_network_stats
            .local_dropped_frames
            .saturating_add(dropped_frames);
        self.video_network_stats.local_decode_errors = self
            .video_network_stats
            .local_decode_errors
            .saturating_add(decode_errors);
        let report = build_video_receiver_report(
            &mut self.video_receiver_report_pending_inbound_frames,
            received_frames,
            rendered_frames,
            dropped_frames,
            decode_errors,
        );
        if let Some(change) = self.video_receiver_preference_controller.evaluate_window(
            VideoReceiverPreferenceWindow {
                seconds: VIDEO_SUMMARY_INTERVAL.as_secs_f64(),
                received_frames: report.received_frames,
                rendered_frames: report.rendered_frames,
                dropped_frames: report.dropped_frames,
                decode_errors: report.decode_errors,
            },
        ) {
            self.queue_receiver_video_profile_request();
            eprintln!(
                "[Video][ReceiverQuality] call_id={} requested_profile={} reason={}",
                call_id,
                change.profile.label(),
                change.reason
            );
        }
        self.queue_video_stream_record(VideoStreamRecord::ReceiverReport(report));
    }

    pub(super) async fn tick_video_call(&mut self) {
        let Some(call_snapshot) = self.active_call.as_ref().cloned() else {
            return;
        };
        if call_snapshot.phase != ActiveCallPhase::Active || call_snapshot.kind != CallKind::Video {
            return;
        }
        if !self.peer_has_quic_path(&call_snapshot.remote_peer_id) {
            self.transition_to_idle(Some("quic_path_lost".to_string()))
                .await;
            return;
        }
        if self.video_stream_tx.is_none() {
            let _ = self.start_video_stream_writer(
                call_snapshot.remote_peer_id,
                call_snapshot.call_id.clone(),
            );
        }
        self.ensure_video_capture_running(&call_snapshot).await;
        self.pump_native_video_capture(&call_snapshot.call_id);
        if self
            .video_last_summary_at
            .map(|last| last.elapsed() >= VIDEO_SUMMARY_INTERVAL)
            .unwrap_or(true)
        {
            self.evaluate_video_quality_window(&call_snapshot.call_id);
            self.log_video_network_summary("summary", &call_snapshot.remote_peer_id);
            self.video_last_summary_at = Some(std::time::Instant::now());
            self.video_window_started_at = Some(std::time::Instant::now());
            self.video_window_counters.reset();
        }
    }

    fn evaluate_video_quality_window(&mut self, call_id: &str) {
        let seconds = self
            .video_window_started_at
            .map(|started| started.elapsed().as_secs_f64())
            .unwrap_or_else(|| VIDEO_SUMMARY_INTERVAL.as_secs_f64());
        let window = VideoAdaptationWindow {
            seconds,
            submitted_frames: self.video_window_counters.submitted_frames,
            encoded_frames: self.video_window_counters.encoded_frames,
            encoded_queue_drops: self.video_window_counters.encoded_queue_drops,
            receiver_received_frames: self.video_window_counters.receiver_received_frames,
            receiver_rendered_frames: self.video_window_counters.receiver_rendered_frames,
            receiver_dropped_frames: self.video_window_counters.receiver_dropped_frames,
            receiver_decode_errors: self.video_window_counters.receiver_decode_errors,
            encode_p95_ms: self.video_window_counters.encode_p95_ms(),
        };
        if let Some(change) = self.video_quality_controller.evaluate_window(window) {
            self.apply_video_quality_change(call_id, change);
        }
    }

    fn apply_video_quality_change(&mut self, call_id: &str, change: VideoQualityChangeDecision) {
        self.video_network_stats.quality_changes += 1;
        self.reset_outbound_video_encoder();
        self.queue_video_stream_record(VideoStreamRecord::QualityChange(VideoQualityChange {
            profile: change.profile,
            reason: change.reason.clone(),
        }));
        self.emit_video_quality_event(call_id, &change.reason);
        eprintln!(
            "[Video][Quality] call_id={} profile={} reason={}",
            call_id,
            change.profile.label(),
            change.reason
        );
    }

    pub(super) async fn handle_video_stream_event(&mut self, event: VideoStreamEvent) {
        match event {
            VideoStreamEvent::InboundRecord {
                peer,
                call_id,
                record,
            } => match record {
                VideoStreamRecord::Frame(frame) => {
                    if !self
                        .active_call
                        .as_ref()
                        .map(|call| {
                            call.phase == ActiveCallPhase::Active
                                && call.kind == CallKind::Video
                                && call.call_id == call_id
                                && call.remote_peer_id == peer
                        })
                        .unwrap_or(false)
                    {
                        return;
                    }
                    self.video_network_stats.inbound_frames += 1;
                    self.video_network_stats.inbound_bytes = self
                        .video_network_stats
                        .inbound_bytes
                        .saturating_add(frame.payload.len() as u64);
                    self.video_receiver_report_pending_inbound_frames = self
                        .video_receiver_report_pending_inbound_frames
                        .saturating_add(1);
                    self.video_window_counters.inbound_frames += 1;
                    if let Some(expected) = self.video_expected_inbound_seq {
                        if frame.seq != expected {
                            self.video_network_stats.inbound_seq_gaps += 1;
                            if frame.seq < expected {
                                self.video_network_stats.inbound_out_of_order_frames += 1;
                            }
                        }
                    }
                    self.video_expected_inbound_seq = Some(frame.seq.wrapping_add(1));

                    self.queue_remote_video_decode_frame(peer, call_id, frame);
                }
                VideoStreamRecord::ReceiverReport(report) => {
                    self.video_network_stats.receiver_received_frames = self
                        .video_network_stats
                        .receiver_received_frames
                        .saturating_add(report.received_frames);
                    self.video_network_stats.receiver_rendered_frames = self
                        .video_network_stats
                        .receiver_rendered_frames
                        .saturating_add(report.rendered_frames);
                    self.video_network_stats.receiver_dropped_frames = self
                        .video_network_stats
                        .receiver_dropped_frames
                        .saturating_add(report.dropped_frames);
                    self.video_network_stats.receiver_decode_errors = self
                        .video_network_stats
                        .receiver_decode_errors
                        .saturating_add(report.decode_errors);
                    self.video_window_counters.receiver_received_frames = self
                        .video_window_counters
                        .receiver_received_frames
                        .saturating_add(report.received_frames);
                    self.video_window_counters.receiver_rendered_frames = self
                        .video_window_counters
                        .receiver_rendered_frames
                        .saturating_add(report.rendered_frames);
                    self.video_window_counters.receiver_dropped_frames = self
                        .video_window_counters
                        .receiver_dropped_frames
                        .saturating_add(report.dropped_frames);
                    self.video_window_counters.receiver_decode_errors = self
                        .video_window_counters
                        .receiver_decode_errors
                        .saturating_add(report.decode_errors);
                }
                VideoStreamRecord::CameraState(state) => {
                    self.emit_remote_camera_state(&call_id, &peer, state.enabled);
                }
                VideoStreamRecord::QualityChange(change) => {
                    if change.reason == VIDEO_RECEIVER_REQUEST_REASON {
                        let previous = self.video_remote_requested_profile;
                        self.video_remote_requested_profile = change.profile;
                        if change.profile != previous {
                            self.reset_outbound_video_encoder();
                            self.emit_video_quality_event(&call_id, VIDEO_RECEIVER_REQUEST_REASON);
                        }
                    }
                    eprintln!(
                        "[Video][RemoteQuality] peer={} call_id={} profile={} reason={}",
                        peer,
                        call_id,
                        change.profile.label(),
                        change.reason
                    );
                }
            },
            VideoStreamEvent::InboundFailure {
                peer,
                call_id,
                error,
            } => {
                eprintln!("[Video] Inbound stream failure from {}: {}", peer, error);
                self.video_network_stats.inbound_failures += 1;
                if self
                    .active_call
                    .as_ref()
                    .map(|call| {
                        call.phase == ActiveCallPhase::Active
                            && call.kind == CallKind::Video
                            && call.remote_peer_id == peer
                            && call_id.as_deref() == Some(call.call_id.as_str())
                    })
                    .unwrap_or(false)
                {
                    self.transition_to_idle(Some("stream_failure".to_string()))
                        .await;
                }
            }
            VideoStreamEvent::OutboundFailure {
                peer,
                call_id,
                error,
            } => {
                eprintln!("[Video] Outbound stream failure to {}: {}", peer, error);
                self.video_network_stats.outbound_failures += 1;
                if self.video_stream_call_id.as_deref() == Some(call_id.as_str()) {
                    self.video_stream_tx = None;
                    self.video_stream_call_id = None;
                    self.video_stream_writer_handle = None;
                }
                if self
                    .active_call
                    .as_ref()
                    .map(|call| {
                        call.phase == ActiveCallPhase::Active
                            && call.kind == CallKind::Video
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

    pub(super) fn log_video_network_summary(&mut self, label: &str, peer_id: &PeerId) {
        let (quic_count, tcp_count) = self.peer_transport_counts(peer_id);
        let duration_secs = self
            .active_call
            .as_ref()
            .and_then(|call| call.started_at)
            .map(|started| (Self::now_unix_ts() - started).max(1) as f64)
            .unwrap_or(1.0);
        let actual_kbps =
            self.video_network_stats.outbound_bytes as f64 * 8.0 / duration_secs / 1000.0;
        let avg_out_bytes = if self.video_network_stats.encoded_frames == 0 {
            0.0
        } else {
            self.video_network_stats.outbound_bytes as f64
                / self.video_network_stats.encoded_frames as f64
        };
        let avg_in_bytes = if self.video_network_stats.inbound_frames == 0 {
            0.0
        } else {
            self.video_network_stats.inbound_bytes as f64
                / self.video_network_stats.inbound_frames as f64
        };
        let local_profile = self.video_quality_controller.current_profile();
        let remote_requested_profile = self.video_remote_requested_profile;
        let effective_profile = self.effective_outbound_video_profile();
        let encoded_actual = match (
            self.video_network_stats.last_encoded_width,
            self.video_network_stats.last_encoded_height,
        ) {
            (Some(width), Some(height)) => format!("{}x{}", width, height),
            _ => "none".to_string(),
        };
        let capture_stats = self
            .video_capture_session
            .as_ref()
            .map(VideoCaptureSession::stats)
            .unwrap_or_else(|| self.video_capture_last_stats.clone());
        let capture_fps = self
            .video_capture_started_at
            .map(|started| {
                let elapsed = started.elapsed().as_secs_f64().max(0.001);
                capture_stats.captured_frames as f64 / elapsed
            })
            .unwrap_or(0.0);
        let (capture_backend, capture_device, capture_format, capture_requested, capture_actual) =
            self.video_capture_info
                .as_ref()
                .map(|info| {
                    (
                        info.backend.as_str(),
                        info.device_name.as_str(),
                        info.format.format.as_str(),
                        info.requested_profile.as_str(),
                        format!(
                            "{}x{}@{}",
                            info.format.width, info.format.height, info.format.fps
                        ),
                    )
                })
                .unwrap_or(("none", "none", "none", "none", "none".to_string()));
        eprintln!(
            "[Video][Network][{}] peer={}, quic_connections={}, tcp_connections={}, profile={}, local_profile={}, remote_requested_profile={}, effective_profile={}, target_kbps={}, actual_kbps={:.1}, encoded_actual={}, capture_backend={}, capture_device='{}', capture_requested_profile={}, capture_actual={}, capture_format={}, captured_frames={}, captured_fps={:.1}, capture_dropped_i420={}, capture_dropped_preview={}, capture_conversion_errors={}, capture_preview_frames={}, capture_start_failures={}, submitted_frames={}, raw_frames_dropped={}, encoded_frames={}, keyframes={}, delta_frames={}, inbound_frames={}, inbound_seq_gaps={}, inbound_out_of_order_frames={}, outbound_failures={}, inbound_failures={}, encode_errors={}, encoded_queue_drops={}, local_rendered_frames={}, local_dropped_frames={}, local_decode_errors={}, receiver_received_frames={}, receiver_rendered_frames={}, receiver_dropped_frames={}, receiver_decode_errors={}, quality_changes={}, outbound_bytes={}, inbound_bytes={}, avg_out_bytes={:.1}, avg_in_bytes={:.1}, encode_p95_ms={:.1}",
            label,
            peer_id,
            quic_count,
            tcp_count,
            effective_profile.label(),
            local_profile.label(),
            remote_requested_profile.label(),
            effective_profile.label(),
            effective_profile.bitrate_kbps(),
            actual_kbps,
            encoded_actual,
            capture_backend,
            capture_device,
            capture_requested,
            capture_actual,
            capture_format,
            capture_stats.captured_frames,
            capture_fps,
            capture_stats.dropped_i420_frames,
            capture_stats.dropped_preview_frames,
            capture_stats.conversion_errors,
            capture_stats.preview_frames,
            self.video_network_stats.capture_start_failures,
            self.video_network_stats.submitted_frames,
            self.video_network_stats.raw_frames_dropped,
            self.video_network_stats.encoded_frames,
            self.video_network_stats.keyframes,
            self.video_network_stats.delta_frames,
            self.video_network_stats.inbound_frames,
            self.video_network_stats.inbound_seq_gaps,
            self.video_network_stats.inbound_out_of_order_frames,
            self.video_network_stats.outbound_failures,
            self.video_network_stats.inbound_failures,
            self.video_network_stats.encode_errors,
            self.video_network_stats.encoded_queue_drops,
            self.video_network_stats.local_rendered_frames,
            self.video_network_stats.local_dropped_frames,
            self.video_network_stats.local_decode_errors,
            self.video_network_stats.receiver_received_frames,
            self.video_network_stats.receiver_rendered_frames,
            self.video_network_stats.receiver_dropped_frames,
            self.video_network_stats.receiver_decode_errors,
            self.video_network_stats.quality_changes,
            self.video_network_stats.outbound_bytes,
            self.video_network_stats.inbound_bytes,
            avg_out_bytes,
            avg_in_bytes,
            self.video_window_counters.encode_p95_ms(),
        );
    }
}

pub(super) fn start_video_stream_accept_loop(
    incoming: crate::network::voice_stream::IncomingStreams,
    event_tx: tokio::sync::mpsc::Sender<VideoStreamEvent>,
) {
    tauri::async_runtime::spawn(async move {
        futures::pin_mut!(incoming);
        while let Some((peer, mut stream)) = incoming.next().await {
            let event_tx = event_tx.clone();
            tauri::async_runtime::spawn(async move {
                eprintln!("[Video][Stream] inbound stream accepted peer={}", peer);
                let call_id = match read_video_stream_header(&mut stream).await {
                    Ok(call_id) => call_id,
                    Err(e) => {
                        let _ = event_tx
                            .send(VideoStreamEvent::InboundFailure {
                                peer,
                                call_id: None,
                                error: e.to_string(),
                            })
                            .await;
                        return;
                    }
                };
                eprintln!(
                    "[Video][Stream] inbound header read peer={} call_id={}",
                    peer, call_id
                );

                let mut first_frame_read = false;
                loop {
                    match read_video_stream_record(&mut stream).await {
                        Ok(record) => {
                            if let VideoStreamRecord::Frame(frame) = &record {
                                if !first_frame_read {
                                    eprintln!(
                                        "[Video][Stream] inbound first frame read peer={} call_id={} seq={} bytes={} kind={:?}",
                                        peer,
                                        call_id,
                                        frame.seq,
                                        frame.payload.len(),
                                        frame.chunk_type
                                    );
                                    first_frame_read = true;
                                }
                            }
                            if event_tx
                                .send(VideoStreamEvent::InboundRecord {
                                    peer,
                                    call_id: call_id.clone(),
                                    record,
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
                                .send(VideoStreamEvent::InboundFailure {
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

fn capture_profile_from_video_profile(profile: VideoProfile) -> CaptureProfile {
    match profile {
        VideoProfile::P360 => CaptureProfile::P360,
        VideoProfile::P480 => CaptureProfile::P480,
        VideoProfile::P720 => CaptureProfile::P720,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn synthetic_i420(width: usize, height: usize) -> Vec<u8> {
        let y_len = width * height;
        let uv_len = y_len / 4;
        let mut data = vec![96u8; y_len + uv_len * 2];
        data[y_len..].fill(128);
        data
    }

    fn encoded_keyframe(seq: u32) -> VideoStreamFrame {
        let width = 640;
        let height = 360;
        let mut encoder = Vp8VideoEncoder::new_with_dimensions(VideoProfile::P360, width, height)
            .expect("encoder starts");
        let packets = encoder
            .encode_i420(
                123,
                width,
                height,
                &synthetic_i420(width as usize, height as usize),
                true,
            )
            .expect("frame encodes");
        let packet = packets.first().expect("packet");
        VideoStreamFrame {
            seq,
            timestamp_us: 123,
            chunk_type: VideoChunkType::Key,
            profile: VideoProfile::P360,
            width,
            height,
            payload: packet.payload.clone(),
        }
    }

    #[tokio::test]
    async fn pending_video_capture_start_is_not_awaited() {
        let handle = tokio::spawn(async {
            tokio::time::sleep(Duration::from_millis(50)).await;
            Err::<VideoCaptureSession, VideoCaptureError>(VideoCaptureError::NoDevice)
        });
        let mut task = Some(VideoCaptureStartTask {
            call_id: "call-1".to_string(),
            profile_label: "720p30".to_string(),
            handle,
        });

        assert!(take_finished_video_capture_start(&mut task).is_none());
        assert!(task.is_some());
    }

    #[test]
    fn remote_video_decoder_state_waits_for_keyframe_when_call_changes() {
        let mut state = RemoteVideoDecoderState::default();
        let decoded = state
            .decode_frame("call-1", &encoded_keyframe(0))
            .expect("keyframe decodes");
        assert!(decoded.is_some());

        let delta = VideoStreamFrame {
            seq: 1,
            timestamp_us: 456,
            chunk_type: VideoChunkType::Delta,
            profile: VideoProfile::P360,
            width: 640,
            height: 360,
            payload: vec![1, 2, 3],
        };

        assert_eq!(
            state
                .decode_frame("call-2", &delta)
                .expect("delta waits for keyframe"),
            None
        );
    }

    #[test]
    fn running_video_capture_survives_quality_profile_change() {
        assert!(!should_restart_running_video_capture_for_profile_change(
            Some("720p30"),
            VideoProfile::P480,
        ));
        assert!(!should_restart_running_video_capture_for_profile_change(
            Some("720p30"),
            VideoProfile::P720,
        ));
        assert!(should_restart_running_video_capture_for_profile_change(
            None,
            VideoProfile::P720,
        ));
    }

    #[test]
    fn outbound_video_encoder_state_forces_keyframe_after_reset() {
        let mut state = OutboundVideoEncoderState::default();
        let width = 640;
        let height = 360;

        let first = state
            .encode_frame(OutboundVideoEncodeFrame {
                call_id: "call-1".to_string(),
                generation: 0,
                timestamp_us: 1,
                width,
                height,
                profile: VideoProfile::P360,
                data: synthetic_i420(width as usize, height as usize),
                force_keyframe: false,
            })
            .expect("first frame encodes");
        assert!(first.packets.iter().any(|packet| packet.is_key));

        state.reset();

        let after_reset = state
            .encode_frame(OutboundVideoEncodeFrame {
                call_id: "call-1".to_string(),
                generation: 0,
                timestamp_us: 2,
                width,
                height,
                profile: VideoProfile::P360,
                data: synthetic_i420(width as usize, height as usize),
                force_keyframe: false,
            })
            .expect("frame after reset encodes");
        assert!(after_reset.packets.iter().any(|packet| packet.is_key));
    }

    #[test]
    fn outbound_video_encoder_state_downscales_to_profile() {
        let mut state = OutboundVideoEncoderState::default();

        let output = state
            .encode_frame(OutboundVideoEncodeFrame {
                call_id: "call-1".to_string(),
                generation: 0,
                timestamp_us: 1,
                width: 1280,
                height: 720,
                profile: VideoProfile::P360,
                data: synthetic_i420(1280, 720),
                force_keyframe: false,
            })
            .expect("frame encodes");

        assert_eq!((output.width, output.height), (640, 360));
        assert_eq!(output.profile, VideoProfile::P360);
        assert!(output.packets.iter().any(|packet| packet.is_key));
    }

    #[test]
    fn outbound_video_encoder_state_forces_keyframe_after_profile_change() {
        let mut state = OutboundVideoEncoderState::default();

        state
            .encode_frame(OutboundVideoEncodeFrame {
                call_id: "call-1".to_string(),
                generation: 0,
                timestamp_us: 1,
                width: 1280,
                height: 720,
                profile: VideoProfile::P720,
                data: synthetic_i420(1280, 720),
                force_keyframe: false,
            })
            .expect("first frame encodes");

        let downshifted = state
            .encode_frame(OutboundVideoEncodeFrame {
                call_id: "call-1".to_string(),
                generation: 0,
                timestamp_us: 2,
                width: 1280,
                height: 720,
                profile: VideoProfile::P360,
                data: synthetic_i420(1280, 720),
                force_keyframe: false,
            })
            .expect("downshifted frame encodes");

        assert_eq!((downshifted.width, downshifted.height), (640, 360));
        assert!(downshifted.packets.iter().any(|packet| packet.is_key));
    }

    #[test]
    fn effective_video_profile_uses_receiver_cap() {
        assert_eq!(
            effective_video_profile(VideoProfile::P720, VideoProfile::P360),
            VideoProfile::P360
        );
        assert_eq!(
            effective_video_profile(VideoProfile::P360, VideoProfile::P720),
            VideoProfile::P360
        );
        assert_eq!(
            effective_video_profile(VideoProfile::P480, VideoProfile::P480),
            VideoProfile::P480
        );
    }

    #[test]
    fn receiver_report_uses_pending_inbound_frame_count() {
        let mut pending_inbound_frames = 12;

        let report = build_video_receiver_report(&mut pending_inbound_frames, 3, 8, 2, 1);

        assert_eq!(report.received_frames, 12);
        assert_eq!(report.rendered_frames, 8);
        assert_eq!(report.dropped_frames, 2);
        assert_eq!(report.decode_errors, 1);
        assert_eq!(pending_inbound_frames, 0);
    }
}
