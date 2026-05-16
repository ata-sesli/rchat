use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, Stream, StreamConfig};
use rubato::{
    audioadapter_buffers::direct::SequentialSliceOfVecs, Async, FixedAsync, Resampler,
    SincInterpolationParameters, SincInterpolationType, WindowFunction,
};
use std::collections::VecDeque;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

const TARGET_RATE: u32 = 16_000;
const FRAME_SAMPLES: usize = 320; // 20ms @ 16kHz mono
const VOICE_DIAGNOSTICS_INTERVAL: Duration = Duration::from_secs(5);

#[derive(Debug, Default)]
struct VoiceAudioStats {
    capture_callbacks: u64,
    generated_frames: u64,
    resampler_errors: u64,
    playback_callbacks: u64,
    playback_frames_received: u64,
    playback_underruns: u64,
    max_playback_queue_samples: usize,
}

impl VoiceAudioStats {
    fn log_summary(&self, label: &str) {
        eprintln!(
            "[Voice][Audio][{}] capture_callbacks={}, generated_frames={}, resampler_errors={}, playback_callbacks={}, playback_frames_received={}, playback_underruns={}, max_playback_queue_ms={:.1}",
            label,
            self.capture_callbacks,
            self.generated_frames,
            self.resampler_errors,
            self.playback_callbacks,
            self.playback_frames_received,
            self.playback_underruns,
            samples_to_ms(self.max_playback_queue_samples),
        );
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
        let stats = Arc::new(Mutex::new(VoiceAudioStats::default()));
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
                    send_captured_frames(&capture_tx, &mut assembler, &mono, &stats);
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
                    send_captured_frames(&capture_tx, &mut assembler, &mono, &stats);
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
                    send_captured_frames(&capture_tx, &mut assembler, &mono, &stats);
                },
                err_fn,
                None,
            )
            .map_err(|e| format!("Failed to build u16 input stream: {}", e)),
        _ => Err("Unsupported input sample format".to_string()),
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
    let mut src_cursor: f32 = 0.0;
    let err_fn = |err| eprintln!("[Voice] Output stream error: {}", err);

    match sample_format {
        SampleFormat::F32 => output_device
            .build_output_stream(
                config,
                move |data: &mut [f32], _| {
                    drain_playback_frames(&playback_rx, &mut queue, &stats);
                    write_output_frames_f32(
                        data,
                        channels,
                        out_rate,
                        &mut queue,
                        &mut src_cursor,
                        &stats,
                    );
                },
                err_fn,
                None,
            )
            .map_err(|e| format!("Failed to build f32 output stream: {}", e)),
        SampleFormat::I16 => output_device
            .build_output_stream(
                config,
                move |data: &mut [i16], _| {
                    drain_playback_frames(&playback_rx, &mut queue, &stats);
                    write_output_frames_i16(
                        data,
                        channels,
                        out_rate,
                        &mut queue,
                        &mut src_cursor,
                        &stats,
                    );
                },
                err_fn,
                None,
            )
            .map_err(|e| format!("Failed to build i16 output stream: {}", e)),
        SampleFormat::U16 => output_device
            .build_output_stream(
                config,
                move |data: &mut [u16], _| {
                    drain_playback_frames(&playback_rx, &mut queue, &stats);
                    write_output_frames_u16(
                        data,
                        channels,
                        out_rate,
                        &mut queue,
                        &mut src_cursor,
                        &stats,
                    );
                },
                err_fn,
                None,
            )
            .map_err(|e| format!("Failed to build u16 output stream: {}", e)),
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
    with_audio_stats(stats, |s| {
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
}

impl VoiceFrameAssembler {
    fn new(input_rate: u32) -> Result<Self, String> {
        Ok(Self {
            resampler: VoiceResampler::new(input_rate)?,
            pending: VecDeque::with_capacity(FRAME_SAMPLES * 4),
        })
    }

    fn push_samples(&mut self, samples: &[i16]) -> Vec<Vec<i16>> {
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
}

impl VoiceResampler {
    fn new(input_rate: u32) -> Result<Self, String> {
        if input_rate == TARGET_RATE {
            return Ok(Self {
                mode: VoiceResamplerMode::Bypass,
                errors: 0,
            });
        }

        let input_chunk = input_frames_per_voice_frame(input_rate);
        let params = SincInterpolationParameters {
            sinc_len: 256,
            f_cutoff: 0.95,
            oversampling_factor: 128,
            interpolation: SincInterpolationType::Linear,
            window: WindowFunction::BlackmanHarris2,
        };
        let resampler = Async::<f32>::new_sinc(
            TARGET_RATE as f64 / input_rate as f64,
            1.05,
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
        })
    }

    #[cfg(test)]
    fn uses_bypass(&self) -> bool {
        matches!(self.mode, VoiceResamplerMode::Bypass)
    }

    fn error_count(&self) -> u64 {
        self.errors
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
    if received > 0 {
        let queue_len = queue.len();
        with_audio_stats(stats, |s| {
            s.playback_frames_received += received;
            s.max_playback_queue_samples = s.max_playback_queue_samples.max(queue_len);
        });
    }
}

fn write_output_frames_i16(
    data: &mut [i16],
    channels: usize,
    out_rate: u32,
    queue: &mut VecDeque<i16>,
    src_cursor: &mut f32,
    stats: &Arc<Mutex<VoiceAudioStats>>,
) {
    if channels == 0 {
        return;
    }
    let mut underruns = 0u64;
    let frame_count = data.len() / channels;
    let step = TARGET_RATE as f32 / out_rate as f32;
    for frame_idx in 0..frame_count {
        let src_idx = (*src_cursor).floor() as usize;
        let sample = match queue.get(src_idx).copied() {
            Some(sample) => sample,
            None => {
                underruns += 1;
                0
            }
        };
        *src_cursor += step;
        for ch in 0..channels {
            data[frame_idx * channels + ch] = sample;
        }
    }
    let consumed = (*src_cursor).floor() as usize;
    for _ in 0..consumed {
        let _ = queue.pop_front();
    }
    *src_cursor -= consumed as f32;
    with_audio_stats(stats, |s| {
        s.playback_callbacks += 1;
        s.playback_underruns += underruns;
        s.max_playback_queue_samples = s.max_playback_queue_samples.max(queue.len());
    });
}

fn write_output_frames_f32(
    data: &mut [f32],
    channels: usize,
    out_rate: u32,
    queue: &mut VecDeque<i16>,
    src_cursor: &mut f32,
    stats: &Arc<Mutex<VoiceAudioStats>>,
) {
    if channels == 0 {
        return;
    }
    let mut underruns = 0u64;
    let frame_count = data.len() / channels;
    let step = TARGET_RATE as f32 / out_rate as f32;
    for frame_idx in 0..frame_count {
        let src_idx = (*src_cursor).floor() as usize;
        let sample = match queue.get(src_idx).copied() {
            Some(sample) => sample,
            None => {
                underruns += 1;
                0
            }
        };
        *src_cursor += step;
        let f = i16_to_f32(sample);
        for ch in 0..channels {
            data[frame_idx * channels + ch] = f;
        }
    }
    let consumed = (*src_cursor).floor() as usize;
    for _ in 0..consumed {
        let _ = queue.pop_front();
    }
    *src_cursor -= consumed as f32;
    with_audio_stats(stats, |s| {
        s.playback_callbacks += 1;
        s.playback_underruns += underruns;
        s.max_playback_queue_samples = s.max_playback_queue_samples.max(queue.len());
    });
}

fn write_output_frames_u16(
    data: &mut [u16],
    channels: usize,
    out_rate: u32,
    queue: &mut VecDeque<i16>,
    src_cursor: &mut f32,
    stats: &Arc<Mutex<VoiceAudioStats>>,
) {
    if channels == 0 {
        return;
    }
    let mut underruns = 0u64;
    let frame_count = data.len() / channels;
    let step = TARGET_RATE as f32 / out_rate as f32;
    for frame_idx in 0..frame_count {
        let src_idx = (*src_cursor).floor() as usize;
        let sample = match queue.get(src_idx).copied() {
            Some(sample) => sample,
            None => {
                underruns += 1;
                0
            }
        };
        *src_cursor += step;
        let u = i16_to_u16(sample);
        for ch in 0..channels {
            data[frame_idx * channels + ch] = u;
        }
    }
    let consumed = (*src_cursor).floor() as usize;
    for _ in 0..consumed {
        let _ = queue.pop_front();
    }
    *src_cursor -= consumed as f32;
    with_audio_stats(stats, |s| {
        s.playback_callbacks += 1;
        s.playback_underruns += underruns;
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
}
