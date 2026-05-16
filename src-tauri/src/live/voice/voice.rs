use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, Stream, StreamConfig};
use rubato::{
    audioadapter_buffers::direct::SequentialSliceOfVecs, Async, FixedAsync, Resampler,
    SincInterpolationParameters, SincInterpolationType, WindowFunction,
};
use std::collections::VecDeque;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

const TARGET_RATE: u32 = 16_000;
const FRAME_SAMPLES: usize = 320; // 20ms @ 16kHz mono
const VOICE_DIAGNOSTICS_INTERVAL: Duration = Duration::from_secs(5);
const PLAYBACK_TARGET_QUEUE_SAMPLES: usize = FRAME_SAMPLES * 8; // 160ms
const PLAYBACK_LOW_QUEUE_SAMPLES: usize = FRAME_SAMPLES * 4; // 80ms
const MAX_PLAYBACK_QUEUE_SAMPLES: usize = FRAME_SAMPLES * 16; // 320ms
const CONCEALMENT_SAMPLES: usize = FRAME_SAMPLES;
const CONCEALMENT_HOLD_SAMPLES: usize = FRAME_SAMPLES / 4;
const CAPTURE_RATE_MEASURE_INTERVAL: Duration = Duration::from_secs(2);
const PLAYBACK_RATE_MEASURE_INTERVAL: Duration = Duration::from_secs(1);
const PLAYBACK_RATE_EMA_ALPHA: f64 = 0.15;
const MIN_PLAUSIBLE_OUTPUT_RATE_HZ: f64 = 8_000.0;
const MAX_PLAUSIBLE_OUTPUT_RATE_HZ: f64 = 192_000.0;
const OUTPUT_CLOCK_UNSTABLE_THRESHOLD: f64 = 0.10;

#[derive(Debug, Default)]
struct VoiceAudioStats {
    started_at: Option<Instant>,
    capture_callbacks: u64,
    capture_input_frames: u64,
    measured_capture_rate_hz: f64,
    capture_resample_ratio: f64,
    capture_panics: u64,
    generated_frames: u64,
    resampler_errors: u64,
    playback_callbacks: u64,
    output_device_frames: u64,
    playback_declared_rate_hz: f64,
    playback_measured_rate_hz: f64,
    playback_effective_rate_hz: f64,
    output_clock_unstable: bool,
    playback_frames_received: u64,
    playback_samples_consumed: u64,
    playback_samples_dropped: u64,
    playback_queue_trim_events: u64,
    playback_concealed_samples: u64,
    playback_underruns: u64,
    current_playback_queue_samples: usize,
    max_playback_queue_samples: usize,
}

impl VoiceAudioStats {
    fn log_summary(&self, label: &str) {
        let elapsed = self
            .started_at
            .map(|started| started.elapsed().as_secs_f64())
            .unwrap_or(0.0)
            .max(0.001);
        let capture_device_hz = self.capture_input_frames as f64 / elapsed;
        let generated_fps = self.generated_frames as f64 / elapsed;
        let output_device_hz = self.output_device_frames as f64 / elapsed;
        let playback_fps = (self.playback_samples_consumed as f64 / FRAME_SAMPLES as f64) / elapsed;
        eprintln!(
            "[Voice][Audio][{}] capture_callbacks={}, capture_device_hz={:.1}, measured_capture_hz={:.1}, capture_resample_ratio={:.6}, capture_panics={}, generated_frames={}, generated_fps={:.1}, resampler_errors={}, playback_callbacks={}, output_device_hz={:.1}, playback_declared_hz={:.1}, playback_measured_hz={:.1}, playback_effective_hz={:.1}, output_clock_unstable={}, playback_frames_received={}, playback_fps={:.1}, playback_underruns={}, playback_concealed_samples={}, playback_samples_dropped={}, playback_queue_trim_events={}, current_playback_queue_ms={:.1}, max_playback_queue_ms={:.1}",
            label,
            self.capture_callbacks,
            capture_device_hz,
            self.measured_capture_rate_hz,
            self.capture_resample_ratio,
            self.capture_panics,
            self.generated_frames,
            generated_fps,
            self.resampler_errors,
            self.playback_callbacks,
            output_device_hz,
            self.playback_declared_rate_hz,
            self.playback_measured_rate_hz,
            self.playback_effective_rate_hz,
            self.output_clock_unstable,
            self.playback_frames_received,
            playback_fps,
            self.playback_underruns,
            self.playback_concealed_samples,
            self.playback_samples_dropped,
            self.playback_queue_trim_events,
            samples_to_ms(self.current_playback_queue_samples),
            samples_to_ms(self.max_playback_queue_samples),
        );
        if self.output_clock_unstable && self.playback_queue_trim_events > 0 {
            eprintln!(
                "[Voice][Audio][PLAYBACK_CLOCK_MISMATCH][OUTPUT_CALLBACK_STARVATION] playback_declared_hz={:.1}, playback_measured_hz={:.1}, playback_effective_hz={:.1}, playback_queue_ms={:.1}, playback_samples_dropped={}, playback_queue_trim_events={}",
                self.playback_declared_rate_hz,
                self.playback_measured_rate_hz,
                self.playback_effective_rate_hz,
                samples_to_ms(self.current_playback_queue_samples),
                self.playback_samples_dropped,
                self.playback_queue_trim_events,
            );
        }
    }
}

fn with_audio_stats(
    stats: &Arc<Mutex<VoiceAudioStats>>,
    update: impl FnOnce(&mut VoiceAudioStats),
) {
    if let Ok(mut guard) = stats.lock() {
        update(&mut guard);
    }
}

pub struct VoiceAudioEngine {
    playback_tx: mpsc::Sender<Vec<i16>>,
    shutdown_tx: mpsc::Sender<()>,
    thread_handle: Option<thread::JoinHandle<()>>,
}

impl VoiceAudioEngine {
    pub fn start() -> Result<(Self, tokio::sync::mpsc::UnboundedReceiver<Vec<i16>>), String> {
        let (capture_tx, capture_rx) = tokio::sync::mpsc::unbounded_channel::<Vec<i16>>();
        let (playback_tx, playback_rx) = mpsc::channel::<Vec<i16>>();
        let (shutdown_tx, shutdown_rx) = mpsc::channel::<()>();
        let stats = Arc::new(Mutex::new(VoiceAudioStats {
            started_at: Some(Instant::now()),
            ..VoiceAudioStats::default()
        }));
        let thread_stats = stats.clone();

        let thread_handle = thread::Builder::new()
            .name("rchat-voice-audio".to_string())
            .spawn(move || {
                run_audio_thread(capture_tx, playback_rx, shutdown_rx, thread_stats);
            })
            .map_err(|e| format!("Failed to start audio thread: {}", e))?;

        Ok((
            Self {
                playback_tx,
                shutdown_tx,
                thread_handle: Some(thread_handle),
            },
            capture_rx,
        ))
    }

    pub fn push_remote_frame(&self, samples: Vec<i16>) {
        let _ = self.playback_tx.send(samples);
    }
}

impl Drop for VoiceAudioEngine {
    fn drop(&mut self) {
        let _ = self.shutdown_tx.send(());
        if let Some(handle) = self.thread_handle.take() {
            let _ = handle.join();
        }
    }
}

fn run_audio_thread(
    capture_tx: tokio::sync::mpsc::UnboundedSender<Vec<i16>>,
    playback_rx: mpsc::Receiver<Vec<i16>>,
    shutdown_rx: mpsc::Receiver<()>,
    stats: Arc<Mutex<VoiceAudioStats>>,
) {
    let host = cpal::default_host();
    let Some(input_device) = host.default_input_device() else {
        eprintln!("[Voice] No default input device");
        return;
    };
    let Some(output_device) = host.default_output_device() else {
        eprintln!("[Voice] No default output device");
        return;
    };

    let Ok(input_supported) = input_device.default_input_config() else {
        eprintln!("[Voice] Failed to read input config");
        return;
    };
    let Ok(output_supported) = output_device.default_output_config() else {
        eprintln!("[Voice] Failed to read output config");
        return;
    };

    let input_config = StreamConfig {
        channels: input_supported.channels(),
        sample_rate: input_supported.sample_rate(),
        buffer_size: cpal::BufferSize::Default,
    };
    let output_config = StreamConfig {
        channels: output_supported.channels(),
        sample_rate: output_supported.sample_rate(),
        buffer_size: cpal::BufferSize::Default,
    };

    let input_name = input_device
        .name()
        .unwrap_or_else(|_| "unknown".to_string());
    let output_name = output_device
        .name()
        .unwrap_or_else(|_| "unknown".to_string());
    eprintln!(
        "[Voice][Audio] input_device='{}', input_rate={}, input_channels={}, input_format={:?}; output_device='{}', output_rate={}, output_channels={}, output_format={:?}",
        input_name,
        input_config.sample_rate.0,
        input_config.channels,
        input_supported.sample_format(),
        output_name,
        output_config.sample_rate.0,
        output_config.channels,
        output_supported.sample_format(),
    );

    let input_stream = match build_input_stream(
        &input_device,
        &input_supported.sample_format(),
        &input_config,
        capture_tx,
        stats.clone(),
    ) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("[Voice] {}", e);
            return;
        }
    };

    let output_stream = match build_output_stream(
        &output_device,
        &output_supported.sample_format(),
        &output_config,
        playback_rx,
        stats.clone(),
    ) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("[Voice] {}", e);
            return;
        }
    };

    if let Err(e) = input_stream.play() {
        eprintln!("[Voice] Failed to start input stream: {}", e);
        return;
    }
    if let Err(e) = output_stream.play() {
        eprintln!("[Voice] Failed to start output stream: {}", e);
        return;
    }

    let mut last_summary = Instant::now();
    loop {
        if shutdown_rx.try_recv().is_ok() {
            break;
        }
        if last_summary.elapsed() >= VOICE_DIAGNOSTICS_INTERVAL {
            if let Ok(guard) = stats.lock() {
                guard.log_summary("summary");
            }
            last_summary = Instant::now();
        }
        thread::sleep(Duration::from_millis(50));
    }

    if let Ok(guard) = stats.lock() {
        guard.log_summary("final");
    }
}

fn build_input_stream(
    input_device: &cpal::Device,
    sample_format: &SampleFormat,
    config: &StreamConfig,
    capture_tx: tokio::sync::mpsc::UnboundedSender<Vec<i16>>,
    stats: Arc<Mutex<VoiceAudioStats>>,
) -> Result<Stream, String> {
    let channels = config.channels as usize;
    let in_rate = config.sample_rate.0;
    let mut assembler = VoiceFrameAssembler::new(in_rate)?;
    let err_fn = |err| eprintln!("[Voice] Input stream error: {}", err);

    match sample_format {
        SampleFormat::F32 => input_device
            .build_input_stream(
                config,
                move |data: &[f32], _| {
                    with_audio_stats(&stats, |s| s.capture_callbacks += 1);
                    let mono = input_to_mono_i16_f32(data, channels);
                    handle_capture_callback(&capture_tx, &mut assembler, &mono, &stats);
                },
                err_fn,
                None,
            )
            .map_err(|e| format!("Failed to build f32 input stream: {}", e)),
        SampleFormat::I16 => input_device
            .build_input_stream(
                config,
                move |data: &[i16], _| {
                    with_audio_stats(&stats, |s| s.capture_callbacks += 1);
                    let mono = input_to_mono_i16_i16(data, channels);
                    handle_capture_callback(&capture_tx, &mut assembler, &mono, &stats);
                },
                err_fn,
                None,
            )
            .map_err(|e| format!("Failed to build i16 input stream: {}", e)),
        SampleFormat::U16 => input_device
            .build_input_stream(
                config,
                move |data: &[u16], _| {
                    with_audio_stats(&stats, |s| s.capture_callbacks += 1);
                    let mono = input_to_mono_i16_u16(data, channels);
                    handle_capture_callback(&capture_tx, &mut assembler, &mono, &stats);
                },
                err_fn,
                None,
            )
            .map_err(|e| format!("Failed to build u16 input stream: {}", e)),
        _ => Err("Unsupported input sample format".to_string()),
    }
}

fn handle_capture_callback(
    capture_tx: &tokio::sync::mpsc::UnboundedSender<Vec<i16>>,
    assembler: &mut VoiceFrameAssembler,
    mono: &[i16],
    stats: &Arc<Mutex<VoiceAudioStats>>,
) {
    if catch_unwind(AssertUnwindSafe(|| {
        send_captured_frames(capture_tx, assembler, mono, stats);
    }))
    .is_err()
    {
        with_audio_stats(stats, |s| {
            s.capture_panics = s.capture_panics.saturating_add(1);
            s.resampler_errors = s.resampler_errors.saturating_add(1);
        });
        eprintln!("[Voice] Capture processing panicked; skipping callback frame");
    }
}

fn build_output_stream(
    output_device: &cpal::Device,
    sample_format: &SampleFormat,
    config: &StreamConfig,
    playback_rx: mpsc::Receiver<Vec<i16>>,
    stats: Arc<Mutex<VoiceAudioStats>>,
) -> Result<Stream, String> {
    let channels = config.channels as usize;
    let out_rate = config.sample_rate.0;
    let mut queue = VecDeque::<i16>::new();
    let mut playback_state = PlaybackState::new(out_rate);
    let err_fn = |err| eprintln!("[Voice] Output stream error: {}", err);

    match sample_format {
        SampleFormat::F32 => {
            let mut mono = Vec::<i16>::new();
            output_device
                .build_output_stream(
                    config,
                    move |data: &mut [f32], _| {
                        drain_playback_frames(&playback_rx, &mut queue, &stats);
                        write_output_frames_f32(
                            data,
                            channels,
                            &mut queue,
                            &mut playback_state,
                            &mut mono,
                            &stats,
                        );
                    },
                    err_fn,
                    None,
                )
                .map_err(|e| format!("Failed to build f32 output stream: {}", e))
        }
        SampleFormat::I16 => {
            let mut mono = Vec::<i16>::new();
            output_device
                .build_output_stream(
                    config,
                    move |data: &mut [i16], _| {
                        drain_playback_frames(&playback_rx, &mut queue, &stats);
                        write_output_frames_i16(
                            data,
                            channels,
                            &mut queue,
                            &mut playback_state,
                            &mut mono,
                            &stats,
                        );
                    },
                    err_fn,
                    None,
                )
                .map_err(|e| format!("Failed to build i16 output stream: {}", e))
        }
        SampleFormat::U16 => {
            let mut mono = Vec::<i16>::new();
            output_device
                .build_output_stream(
                    config,
                    move |data: &mut [u16], _| {
                        drain_playback_frames(&playback_rx, &mut queue, &stats);
                        write_output_frames_u16(
                            data,
                            channels,
                            &mut queue,
                            &mut playback_state,
                            &mut mono,
                            &stats,
                        );
                    },
                    err_fn,
                    None,
                )
                .map_err(|e| format!("Failed to build u16 output stream: {}", e))
        }
        _ => Err("Unsupported output sample format".to_string()),
    }
}

fn send_captured_frames(
    capture_tx: &tokio::sync::mpsc::UnboundedSender<Vec<i16>>,
    assembler: &mut VoiceFrameAssembler,
    samples: &[i16],
    stats: &Arc<Mutex<VoiceAudioStats>>,
) {
    let before_errors = assembler.resampler_error_count();
    let frames = assembler.push_samples(samples);
    let error_delta = assembler
        .resampler_error_count()
        .saturating_sub(before_errors);
    let measured_capture_rate_hz = assembler.measured_input_rate_hz().unwrap_or(0.0);
    let capture_resample_ratio = assembler.resampler_ratio().unwrap_or(0.0);
    with_audio_stats(stats, |s| {
        s.capture_input_frames = s.capture_input_frames.saturating_add(samples.len() as u64);
        if measured_capture_rate_hz > 0.0 {
            s.measured_capture_rate_hz = measured_capture_rate_hz;
        }
        if capture_resample_ratio > 0.0 {
            s.capture_resample_ratio = capture_resample_ratio;
        }
        s.generated_frames += frames.len() as u64;
        s.resampler_errors += error_delta;
    });
    for frame in frames {
        let _ = capture_tx.send(frame);
    }
}

struct VoiceFrameAssembler {
    resampler: VoiceResampler,
    pending: VecDeque<i16>,
    rate_window_started: Instant,
    rate_window_input_samples: u64,
    measured_input_rate_hz: Option<f64>,
}

impl VoiceFrameAssembler {
    fn new(input_rate: u32) -> Result<Self, String> {
        Ok(Self {
            resampler: VoiceResampler::new(input_rate)?,
            pending: VecDeque::with_capacity(FRAME_SAMPLES * 4),
            rate_window_started: Instant::now(),
            rate_window_input_samples: 0,
            measured_input_rate_hz: None,
        })
    }

    fn push_samples(&mut self, samples: &[i16]) -> Vec<Vec<i16>> {
        self.update_measured_input_rate(samples.len());
        for sample in self.resampler.push_mono_i16(samples) {
            self.pending.push_back(sample);
        }

        let mut frames = Vec::new();
        while self.pending.len() >= FRAME_SAMPLES {
            let mut frame = Vec::with_capacity(FRAME_SAMPLES);
            for _ in 0..FRAME_SAMPLES {
                if let Some(sample) = self.pending.pop_front() {
                    frame.push(sample);
                }
            }
            frames.push(frame);
        }
        frames
    }

    fn resampler_error_count(&self) -> u64 {
        self.resampler.error_count()
    }

    fn measured_input_rate_hz(&self) -> Option<f64> {
        self.measured_input_rate_hz
    }

    fn resampler_ratio(&self) -> Option<f64> {
        self.resampler.current_ratio()
    }

    fn update_measured_input_rate(&mut self, input_samples: usize) {
        self.rate_window_input_samples = self
            .rate_window_input_samples
            .saturating_add(input_samples as u64);
        let elapsed = self.rate_window_started.elapsed();
        if elapsed < CAPTURE_RATE_MEASURE_INTERVAL {
            return;
        }

        let measured = self.rate_window_input_samples as f64 / elapsed.as_secs_f64().max(0.001);
        if (8_000.0..=192_000.0).contains(&measured) {
            self.measured_input_rate_hz = Some(measured);
        }
        self.rate_window_started = Instant::now();
        self.rate_window_input_samples = 0;
    }
}

enum VoiceResamplerMode {
    Bypass,
    Rubato {
        resampler: Async<f32>,
        pending_input: VecDeque<f32>,
        input_buffer: Vec<Vec<f32>>,
        output_buffer: Vec<Vec<f32>>,
    },
}

struct VoiceResampler {
    mode: VoiceResamplerMode,
    errors: u64,
    current_ratio: Option<f64>,
}

impl VoiceResampler {
    fn new(input_rate: u32) -> Result<Self, String> {
        if input_rate == TARGET_RATE {
            return Ok(Self {
                mode: VoiceResamplerMode::Bypass,
                errors: 0,
                current_ratio: None,
            });
        }

        let input_chunk = input_frames_per_voice_frame(input_rate);
        let initial_ratio = TARGET_RATE as f64 / input_rate as f64;
        let params = SincInterpolationParameters {
            sinc_len: 256,
            f_cutoff: 0.95,
            oversampling_factor: 128,
            interpolation: SincInterpolationType::Linear,
            window: WindowFunction::BlackmanHarris2,
        };
        let resampler = Async::<f32>::new_sinc(
            initial_ratio,
            1.5,
            &params,
            input_chunk,
            1,
            FixedAsync::Input,
        )
        .map_err(|e| format!("Failed to create voice resampler: {}", e))?;
        let output_capacity = resampler.output_frames_max().max(FRAME_SAMPLES * 2);

        Ok(Self {
            mode: VoiceResamplerMode::Rubato {
                resampler,
                pending_input: VecDeque::with_capacity(input_chunk * 2),
                input_buffer: vec![vec![0.0; input_chunk]],
                output_buffer: vec![vec![0.0; output_capacity]],
            },
            errors: 0,
            current_ratio: Some(initial_ratio),
        })
    }

    #[cfg(test)]
    fn uses_bypass(&self) -> bool {
        matches!(self.mode, VoiceResamplerMode::Bypass)
    }

    fn error_count(&self) -> u64 {
        self.errors
    }

    fn current_ratio(&self) -> Option<f64> {
        self.current_ratio
    }

    fn push_mono_i16(&mut self, samples: &[i16]) -> Vec<i16> {
        match &mut self.mode {
            VoiceResamplerMode::Bypass => samples.to_vec(),
            VoiceResamplerMode::Rubato {
                resampler,
                pending_input,
                input_buffer,
                output_buffer,
            } => {
                for sample in samples {
                    pending_input.push_back(i16_to_f32(*sample));
                }

                let mut out = Vec::new();
                loop {
                    let needed = resampler.input_frames_next();
                    if pending_input.len() < needed {
                        break;
                    }

                    for idx in 0..needed {
                        input_buffer[0][idx] = pending_input.pop_front().unwrap_or(0.0);
                    }

                    let input_adapter = match SequentialSliceOfVecs::new(input_buffer, 1, needed) {
                        Ok(adapter) => adapter,
                        Err(e) => {
                            self.errors = self.errors.saturating_add(1);
                            eprintln!("[Voice] Failed to prepare resampler input: {}", e);
                            break;
                        }
                    };
                    let output_len = output_buffer[0].len();
                    let mut output_adapter =
                        match SequentialSliceOfVecs::new_mut(output_buffer, 1, output_len) {
                            Ok(adapter) => adapter,
                            Err(e) => {
                                self.errors = self.errors.saturating_add(1);
                                eprintln!("[Voice] Failed to prepare resampler output: {}", e);
                                break;
                            }
                        };

                    match resampler.process_into_buffer(&input_adapter, &mut output_adapter, None) {
                        Ok((_read, written)) => {
                            out.extend(output_buffer[0][..written].iter().copied().map(f32_to_i16));
                        }
                        Err(e) => {
                            self.errors = self.errors.saturating_add(1);
                            eprintln!("[Voice] Resampler error: {}", e);
                            break;
                        }
                    }
                }
                out
            }
        }
    }
}

fn input_frames_per_voice_frame(input_rate: u32) -> usize {
    ((input_rate as u64 * FRAME_SAMPLES as u64 + (TARGET_RATE as u64 / 2)) / TARGET_RATE as u64)
        .max(1) as usize
}

fn input_to_mono_i16_f32(data: &[f32], channels: usize) -> Vec<i16> {
    if channels == 0 {
        return Vec::new();
    }
    data.chunks(channels)
        .map(|frame| f32_to_i16(frame[0]))
        .collect()
}

fn input_to_mono_i16_i16(data: &[i16], channels: usize) -> Vec<i16> {
    if channels == 0 {
        return Vec::new();
    }
    data.chunks(channels).map(|frame| frame[0]).collect()
}

fn input_to_mono_i16_u16(data: &[u16], channels: usize) -> Vec<i16> {
    if channels == 0 {
        return Vec::new();
    }
    data.chunks(channels)
        .map(|frame| u16_to_i16(frame[0]))
        .collect()
}

fn drain_playback_frames(
    playback_rx: &mpsc::Receiver<Vec<i16>>,
    queue: &mut VecDeque<i16>,
    stats: &Arc<Mutex<VoiceAudioStats>>,
) {
    let mut received = 0u64;
    while let Ok(frame) = playback_rx.try_recv() {
        received += 1;
        queue.extend(frame);
    }
    let dropped = trim_playback_queue_to_cap(queue);
    if received > 0 {
        let queue_len = queue.len();
        with_audio_stats(stats, |s| {
            s.playback_frames_received += received;
            s.current_playback_queue_samples = queue_len;
            s.max_playback_queue_samples = s.max_playback_queue_samples.max(queue_len);
        });
    }
    if dropped > 0 {
        with_audio_stats(stats, |s| {
            s.playback_samples_dropped = s.playback_samples_dropped.saturating_add(dropped as u64);
            s.playback_queue_trim_events = s.playback_queue_trim_events.saturating_add(1);
            s.current_playback_queue_samples = queue.len();
        });
    }
}

fn trim_playback_queue_to_cap(queue: &mut VecDeque<i16>) -> usize {
    if queue.len() <= MAX_PLAYBACK_QUEUE_SAMPLES {
        return 0;
    }

    let drop_count = FRAME_SAMPLES.min(queue.len());
    for _ in 0..drop_count {
        let _ = queue.pop_front();
    }
    drop_count
}

struct PlaybackState {
    src_cursor: f32,
    last_sample: i16,
    consecutive_underrun_samples: usize,
    declared_output_rate_hz: f64,
    measured_output_rate_hz: Option<f64>,
    effective_output_rate_hz: f64,
    output_clock_unstable: bool,
    output_rate_window_started: Instant,
    output_rate_window_frames: u64,
}

impl PlaybackState {
    fn new(declared_output_rate_hz: u32) -> Self {
        let declared_output_rate_hz = declared_output_rate_hz as f64;
        Self {
            src_cursor: 0.0,
            last_sample: 0,
            consecutive_underrun_samples: 0,
            declared_output_rate_hz,
            measured_output_rate_hz: None,
            effective_output_rate_hz: declared_output_rate_hz,
            output_clock_unstable: false,
            output_rate_window_started: Instant::now(),
            output_rate_window_frames: 0,
        }
    }

    fn note_output_callback_samples(&mut self, sample_count: usize, channels: usize) {
        if channels == 0 {
            return;
        }
        self.note_output_callback(sample_count / channels);
    }

    fn note_output_callback(&mut self, frame_count: usize) {
        self.output_rate_window_frames = self
            .output_rate_window_frames
            .saturating_add(frame_count as u64);

        let elapsed = self.output_rate_window_started.elapsed();
        if elapsed < PLAYBACK_RATE_MEASURE_INTERVAL {
            return;
        }

        let measured = self.output_rate_window_frames as f64 / elapsed.as_secs_f64().max(0.001);
        self.output_rate_window_started = Instant::now();
        self.output_rate_window_frames = 0;

        if !(MIN_PLAUSIBLE_OUTPUT_RATE_HZ..=MAX_PLAUSIBLE_OUTPUT_RATE_HZ).contains(&measured) {
            return;
        }

        self.measured_output_rate_hz = Some(measured);
        self.output_clock_unstable = ((measured - self.declared_output_rate_hz).abs()
            / self.declared_output_rate_hz)
            > OUTPUT_CLOCK_UNSTABLE_THRESHOLD;
        self.effective_output_rate_hz = if self.measured_output_rate_hz == Some(measured)
            && (self.effective_output_rate_hz - self.declared_output_rate_hz).abs() < f64::EPSILON
        {
            measured
        } else {
            (self.effective_output_rate_hz * (1.0 - PLAYBACK_RATE_EMA_ALPHA))
                + (measured * PLAYBACK_RATE_EMA_ALPHA)
        };
    }

    fn declared_output_rate_hz(&self) -> f64 {
        self.declared_output_rate_hz
    }

    fn measured_output_rate_hz(&self) -> Option<f64> {
        self.measured_output_rate_hz
    }

    fn effective_output_rate_hz(&self) -> f64 {
        self.effective_output_rate_hz
    }

    fn output_clock_unstable(&self) -> bool {
        self.output_clock_unstable
    }

    fn conceal_sample(&mut self) -> i16 {
        let sample = if self.consecutive_underrun_samples < CONCEALMENT_SAMPLES {
            let fade_pos = self
                .consecutive_underrun_samples
                .saturating_sub(CONCEALMENT_HOLD_SAMPLES);
            let fade_len = CONCEALMENT_SAMPLES.saturating_sub(CONCEALMENT_HOLD_SAMPLES);
            let remaining = fade_len.saturating_sub(fade_pos);
            let gain = if fade_len == 0 {
                0.0
            } else {
                remaining as f32 / fade_len as f32
            };
            (self.last_sample as f32 * gain) as i16
        } else {
            0
        };
        self.consecutive_underrun_samples = self.consecutive_underrun_samples.saturating_add(1);
        sample
    }

    fn note_played_sample(&mut self, sample: i16) {
        self.last_sample = sample;
        self.consecutive_underrun_samples = 0;
    }
}

fn playback_step(effective_output_rate_hz: f64, queued_samples: usize) -> f32 {
    let base = TARGET_RATE as f32 / effective_output_rate_hz.max(1.0) as f32;
    let correction = if queued_samples > PLAYBACK_TARGET_QUEUE_SAMPLES {
        1.015
    } else if queued_samples < PLAYBACK_LOW_QUEUE_SAMPLES {
        0.985
    } else {
        1.0
    };
    base * correction
}

struct PlaybackRenderStats {
    underruns: u64,
    consumed_samples: usize,
}

fn render_playback_mono_samples(
    frame_count: usize,
    queue: &mut VecDeque<i16>,
    state: &mut PlaybackState,
    out: &mut Vec<i16>,
) -> PlaybackRenderStats {
    out.clear();
    out.reserve(frame_count);
    let step = playback_step(state.effective_output_rate_hz(), queue.len());
    let mut underruns = 0u64;
    for _ in 0..frame_count {
        let src_idx = state.src_cursor.floor() as usize;
        let sample = match queue.get(src_idx).copied() {
            Some(sample) => {
                state.note_played_sample(sample);
                sample
            }
            None => {
                underruns += 1;
                state.conceal_sample()
            }
        };
        state.src_cursor += step;
        out.push(sample);
    }

    let desired_consumed = state.src_cursor.floor() as usize;
    let actual_consumed = desired_consumed.min(queue.len());
    for _ in 0..actual_consumed {
        let _ = queue.pop_front();
    }
    state.src_cursor -= desired_consumed as f32;
    PlaybackRenderStats {
        underruns,
        consumed_samples: actual_consumed,
    }
}

fn write_output_frames_i16(
    data: &mut [i16],
    channels: usize,
    queue: &mut VecDeque<i16>,
    playback_state: &mut PlaybackState,
    mono: &mut Vec<i16>,
    stats: &Arc<Mutex<VoiceAudioStats>>,
) {
    if channels == 0 {
        return;
    }
    let frame_count = data.len() / channels;
    playback_state.note_output_callback_samples(data.len(), channels);
    let render_stats = render_playback_mono_samples(frame_count, queue, playback_state, mono);
    for frame_idx in 0..frame_count {
        let sample = mono[frame_idx];
        for ch in 0..channels {
            data[frame_idx * channels + ch] = sample;
        }
    }
    with_audio_stats(stats, |s| {
        s.playback_callbacks += 1;
        s.output_device_frames = s.output_device_frames.saturating_add(frame_count as u64);
        s.playback_declared_rate_hz = playback_state.declared_output_rate_hz();
        s.playback_measured_rate_hz = playback_state.measured_output_rate_hz().unwrap_or(0.0);
        s.playback_effective_rate_hz = playback_state.effective_output_rate_hz();
        s.output_clock_unstable = playback_state.output_clock_unstable();
        s.playback_samples_consumed = s
            .playback_samples_consumed
            .saturating_add(render_stats.consumed_samples as u64);
        s.playback_underruns += render_stats.underruns;
        s.playback_concealed_samples = s
            .playback_concealed_samples
            .saturating_add(render_stats.underruns);
        s.current_playback_queue_samples = queue.len();
        s.max_playback_queue_samples = s.max_playback_queue_samples.max(queue.len());
    });
}

fn write_output_frames_f32(
    data: &mut [f32],
    channels: usize,
    queue: &mut VecDeque<i16>,
    playback_state: &mut PlaybackState,
    mono: &mut Vec<i16>,
    stats: &Arc<Mutex<VoiceAudioStats>>,
) {
    if channels == 0 {
        return;
    }
    let frame_count = data.len() / channels;
    playback_state.note_output_callback_samples(data.len(), channels);
    let render_stats = render_playback_mono_samples(frame_count, queue, playback_state, mono);
    for frame_idx in 0..frame_count {
        let f = i16_to_f32(mono[frame_idx]);
        for ch in 0..channels {
            data[frame_idx * channels + ch] = f;
        }
    }
    with_audio_stats(stats, |s| {
        s.playback_callbacks += 1;
        s.output_device_frames = s.output_device_frames.saturating_add(frame_count as u64);
        s.playback_declared_rate_hz = playback_state.declared_output_rate_hz();
        s.playback_measured_rate_hz = playback_state.measured_output_rate_hz().unwrap_or(0.0);
        s.playback_effective_rate_hz = playback_state.effective_output_rate_hz();
        s.output_clock_unstable = playback_state.output_clock_unstable();
        s.playback_samples_consumed = s
            .playback_samples_consumed
            .saturating_add(render_stats.consumed_samples as u64);
        s.playback_underruns += render_stats.underruns;
        s.playback_concealed_samples = s
            .playback_concealed_samples
            .saturating_add(render_stats.underruns);
        s.current_playback_queue_samples = queue.len();
        s.max_playback_queue_samples = s.max_playback_queue_samples.max(queue.len());
    });
}

fn write_output_frames_u16(
    data: &mut [u16],
    channels: usize,
    queue: &mut VecDeque<i16>,
    playback_state: &mut PlaybackState,
    mono: &mut Vec<i16>,
    stats: &Arc<Mutex<VoiceAudioStats>>,
) {
    if channels == 0 {
        return;
    }
    let frame_count = data.len() / channels;
    playback_state.note_output_callback_samples(data.len(), channels);
    let render_stats = render_playback_mono_samples(frame_count, queue, playback_state, mono);
    for frame_idx in 0..frame_count {
        let u = i16_to_u16(mono[frame_idx]);
        for ch in 0..channels {
            data[frame_idx * channels + ch] = u;
        }
    }
    with_audio_stats(stats, |s| {
        s.playback_callbacks += 1;
        s.output_device_frames = s.output_device_frames.saturating_add(frame_count as u64);
        s.playback_declared_rate_hz = playback_state.declared_output_rate_hz();
        s.playback_measured_rate_hz = playback_state.measured_output_rate_hz().unwrap_or(0.0);
        s.playback_effective_rate_hz = playback_state.effective_output_rate_hz();
        s.output_clock_unstable = playback_state.output_clock_unstable();
        s.playback_samples_consumed = s
            .playback_samples_consumed
            .saturating_add(render_stats.consumed_samples as u64);
        s.playback_underruns += render_stats.underruns;
        s.playback_concealed_samples = s
            .playback_concealed_samples
            .saturating_add(render_stats.underruns);
        s.current_playback_queue_samples = queue.len();
        s.max_playback_queue_samples = s.max_playback_queue_samples.max(queue.len());
    });
}

fn samples_to_ms(samples: usize) -> f32 {
    samples as f32 * 1000.0 / TARGET_RATE as f32
}

fn f32_to_i16(v: f32) -> i16 {
    let clamped = v.clamp(-1.0, 1.0);
    (clamped * (i16::MAX as f32)) as i16
}

fn i16_to_f32(v: i16) -> f32 {
    (v as f32) / (i16::MAX as f32)
}

fn u16_to_i16(v: u16) -> i16 {
    (v as i32 - 32768) as i16
}

fn i16_to_u16(v: i16) -> u16 {
    (v as i32 + 32768) as u16
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ramp(len: usize) -> Vec<i16> {
        (0..len)
            .map(|idx| ((idx % 1000) as i16).saturating_sub(500))
            .collect()
    }

    #[test]
    fn voice_resampler_bypasses_when_input_is_already_16khz() {
        let mut resampler = VoiceResampler::new(TARGET_RATE).expect("resampler");
        let input = ramp(FRAME_SAMPLES * 3);
        let output = resampler.push_mono_i16(&input);

        assert_eq!(output, input);
        assert!(resampler.uses_bypass());
    }

    #[test]
    fn voice_frame_assembler_emits_20ms_frames_at_16khz() {
        let mut assembler = VoiceFrameAssembler::new(TARGET_RATE).expect("assembler");
        let frames = assembler.push_samples(&ramp(FRAME_SAMPLES * 2 + 17));

        assert_eq!(frames.len(), 2);
        assert!(frames.iter().all(|frame| frame.len() == FRAME_SAMPLES));
    }

    #[test]
    fn voice_frame_assembler_produces_stable_frames_from_48khz() {
        let mut assembler = VoiceFrameAssembler::new(48_000).expect("assembler");
        let frames = assembler.push_samples(&ramp(960 * 6));

        assert_eq!(frames.len(), 5);
        assert!(frames.iter().all(|frame| frame.len() == FRAME_SAMPLES));
    }

    #[test]
    fn voice_frame_assembler_produces_frames_from_44100hz_without_callback_drift() {
        let mut assembler = VoiceFrameAssembler::new(44_100).expect("assembler");
        let mut frames = Vec::new();

        for chunk in ramp(441 * 12).chunks(147) {
            frames.extend(assembler.push_samples(chunk));
        }

        assert_eq!(frames.len(), 5);
        assert!(frames.iter().all(|frame| frame.len() == FRAME_SAMPLES));
    }

    #[test]
    fn playback_queue_drops_one_frame_at_a_time() {
        let mut queue: VecDeque<i16> = (0..(FRAME_SAMPLES * 40)).map(|idx| idx as i16).collect();

        let dropped = trim_playback_queue_to_cap(&mut queue);

        assert_eq!(dropped, FRAME_SAMPLES);
        assert_eq!(queue.len(), FRAME_SAMPLES * 39);
    }

    #[test]
    fn playback_output_conceals_short_underruns_with_last_sample() {
        let mut queue = VecDeque::new();
        let mut state = PlaybackState::new(44_100);
        state.last_sample = 1234;
        let mut out = Vec::new();

        let render_stats = render_playback_mono_samples(8, &mut queue, &mut state, &mut out);

        assert_eq!(render_stats.underruns, 8);
        assert_eq!(out, vec![1234; 8]);
    }

    #[test]
    fn playback_render_reports_only_samples_removed_from_queue() {
        let mut queue: VecDeque<i16> = vec![1, 2].into();
        let mut state = PlaybackState::new(16_000);
        let mut out = Vec::new();

        let render_stats = render_playback_mono_samples(32, &mut queue, &mut state, &mut out);

        assert_eq!(render_stats.consumed_samples, 2);
        assert!(queue.is_empty());
        assert!(render_stats.underruns > 0);
    }

    #[test]
    fn playback_uses_measured_output_rate_when_plausible() {
        let mut state = PlaybackState::new(44_100);
        state.output_rate_window_started = Instant::now() - PLAYBACK_RATE_MEASURE_INTERVAL;

        state.note_output_callback(22_000);

        let measured = state.measured_output_rate_hz().expect("measured output");
        assert!((measured - 22_000.0).abs() < 100.0);
        assert!(state.effective_output_rate_hz() < 30_000.0);
        assert!(
            playback_step(
                state.effective_output_rate_hz(),
                PLAYBACK_TARGET_QUEUE_SAMPLES
            ) > 0.7
        );
    }

    #[test]
    fn playback_falls_back_to_declared_rate_when_measured_rate_is_implausible() {
        let mut state = PlaybackState::new(44_100);
        state.output_rate_window_started = Instant::now() - PLAYBACK_RATE_MEASURE_INTERVAL;

        state.note_output_callback(1_000);

        assert!(state.measured_output_rate_hz().is_none());
        assert_eq!(state.effective_output_rate_hz(), 44_100.0);
    }

    #[test]
    fn output_clock_unstable_detects_large_declared_measured_divergence() {
        let mut state = PlaybackState::new(44_100);
        state.output_rate_window_started = Instant::now() - PLAYBACK_RATE_MEASURE_INTERVAL;

        state.note_output_callback(22_000);

        assert!(state.output_clock_unstable());

        let mut stable = PlaybackState::new(44_100);
        stable.output_rate_window_started = Instant::now() - PLAYBACK_RATE_MEASURE_INTERVAL;
        stable.note_output_callback(42_000);

        assert!(!stable.output_clock_unstable());
    }

    #[test]
    fn measured_output_rate_counts_frames_not_channels() {
        let mut state = PlaybackState::new(44_100);

        for _ in 0..99 {
            state.note_output_callback_samples(882, 2);
        }
        state.output_rate_window_started = Instant::now() - PLAYBACK_RATE_MEASURE_INTERVAL;
        state.note_output_callback_samples(882, 2);

        let measured = state.measured_output_rate_hz().expect("measured output");
        assert!((measured - 44_100.0).abs() < 100.0);
    }

    #[test]
    fn measured_input_rate_does_not_mutate_capture_resampler_ratio() {
        let mut assembler = VoiceFrameAssembler::new(44_100).expect("assembler");
        assembler.rate_window_started = Instant::now() - CAPTURE_RATE_MEASURE_INTERVAL;
        assembler.rate_window_input_samples = 26_000;
        let before = assembler.resampler_ratio().expect("rubato ratio");

        assembler.update_measured_input_rate(0);

        let measured = assembler.measured_input_rate_hz().expect("measured rate");
        assert!((measured - 13_000.0).abs() < 50.0);
        assert_eq!(assembler.resampler_ratio(), Some(before));
    }
}
