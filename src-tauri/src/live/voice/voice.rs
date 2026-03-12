use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, Stream, StreamConfig};
use std::collections::VecDeque;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

const TARGET_RATE: u32 = 16_000;
const FRAME_SAMPLES: usize = 320; // 20ms @ 16kHz mono

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

        let thread_handle = thread::Builder::new()
            .name("rchat-voice-audio".to_string())
            .spawn(move || {
                run_audio_thread(capture_tx, playback_rx, shutdown_rx);
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

    let input_stream = match build_input_stream(
        &input_device,
        &input_supported.sample_format(),
        &input_config,
        capture_tx,
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

    loop {
        if shutdown_rx.try_recv().is_ok() {
            break;
        }
        thread::sleep(Duration::from_millis(50));
    }
}

fn build_input_stream(
    input_device: &cpal::Device,
    sample_format: &SampleFormat,
    config: &StreamConfig,
    capture_tx: tokio::sync::mpsc::UnboundedSender<Vec<i16>>,
) -> Result<Stream, String> {
    let channels = config.channels as usize;
    let in_rate = config.sample_rate.0;
    let mut pending = VecDeque::<i16>::new();
    let err_fn = |err| eprintln!("[Voice] Input stream error: {}", err);

    match sample_format {
        SampleFormat::F32 => input_device
            .build_input_stream(
                config,
                move |data: &[f32], _| {
                    let mono = input_to_mono_i16_f32(data, channels);
                    let resampled = resample_to_16k(&mono, in_rate);
                    for s in resampled {
                        pending.push_back(s);
                    }
                    while pending.len() >= FRAME_SAMPLES {
                        let mut frame = Vec::with_capacity(FRAME_SAMPLES);
                        for _ in 0..FRAME_SAMPLES {
                            if let Some(v) = pending.pop_front() {
                                frame.push(v);
                            }
                        }
                        let _ = capture_tx.send(frame);
                    }
                },
                err_fn,
                None,
            )
            .map_err(|e| format!("Failed to build f32 input stream: {}", e)),
        SampleFormat::I16 => input_device
            .build_input_stream(
                config,
                move |data: &[i16], _| {
                    let mono = input_to_mono_i16_i16(data, channels);
                    let resampled = resample_to_16k(&mono, in_rate);
                    for s in resampled {
                        pending.push_back(s);
                    }
                    while pending.len() >= FRAME_SAMPLES {
                        let mut frame = Vec::with_capacity(FRAME_SAMPLES);
                        for _ in 0..FRAME_SAMPLES {
                            if let Some(v) = pending.pop_front() {
                                frame.push(v);
                            }
                        }
                        let _ = capture_tx.send(frame);
                    }
                },
                err_fn,
                None,
            )
            .map_err(|e| format!("Failed to build i16 input stream: {}", e)),
        SampleFormat::U16 => input_device
            .build_input_stream(
                config,
                move |data: &[u16], _| {
                    let mono = input_to_mono_i16_u16(data, channels);
                    let resampled = resample_to_16k(&mono, in_rate);
                    for s in resampled {
                        pending.push_back(s);
                    }
                    while pending.len() >= FRAME_SAMPLES {
                        let mut frame = Vec::with_capacity(FRAME_SAMPLES);
                        for _ in 0..FRAME_SAMPLES {
                            if let Some(v) = pending.pop_front() {
                                frame.push(v);
                            }
                        }
                        let _ = capture_tx.send(frame);
                    }
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
                    drain_playback_frames(&playback_rx, &mut queue);
                    write_output_frames_f32(data, channels, out_rate, &mut queue, &mut src_cursor);
                },
                err_fn,
                None,
            )
            .map_err(|e| format!("Failed to build f32 output stream: {}", e)),
        SampleFormat::I16 => output_device
            .build_output_stream(
                config,
                move |data: &mut [i16], _| {
                    drain_playback_frames(&playback_rx, &mut queue);
                    write_output_frames_i16(data, channels, out_rate, &mut queue, &mut src_cursor);
                },
                err_fn,
                None,
            )
            .map_err(|e| format!("Failed to build i16 output stream: {}", e)),
        SampleFormat::U16 => output_device
            .build_output_stream(
                config,
                move |data: &mut [u16], _| {
                    drain_playback_frames(&playback_rx, &mut queue);
                    write_output_frames_u16(data, channels, out_rate, &mut queue, &mut src_cursor);
                },
                err_fn,
                None,
            )
            .map_err(|e| format!("Failed to build u16 output stream: {}", e)),
        _ => Err("Unsupported output sample format".to_string()),
    }
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
    data.chunks(channels).map(|frame| u16_to_i16(frame[0])).collect()
}

fn resample_to_16k(samples: &[i16], in_rate: u32) -> Vec<i16> {
    if samples.is_empty() {
        return Vec::new();
    }
    if in_rate == TARGET_RATE {
        return samples.to_vec();
    }

    let ratio = in_rate as f32 / TARGET_RATE as f32;
    let out_len = ((samples.len() as f32) / ratio).max(1.0) as usize;
    let mut out = Vec::with_capacity(out_len);
    for i in 0..out_len {
        let src_idx = ((i as f32) * ratio) as usize;
        let idx = src_idx.min(samples.len().saturating_sub(1));
        out.push(samples[idx]);
    }
    out
}

fn drain_playback_frames(playback_rx: &mpsc::Receiver<Vec<i16>>, queue: &mut VecDeque<i16>) {
    while let Ok(frame) = playback_rx.try_recv() {
        queue.extend(frame);
    }
}

fn write_output_frames_i16(
    data: &mut [i16],
    channels: usize,
    out_rate: u32,
    queue: &mut VecDeque<i16>,
    src_cursor: &mut f32,
) {
    if channels == 0 {
        return;
    }
    let frame_count = data.len() / channels;
    let step = TARGET_RATE as f32 / out_rate as f32;
    for frame_idx in 0..frame_count {
        let sample = queue
            .get((*src_cursor).floor() as usize)
            .copied()
            .unwrap_or(0);
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
}

fn write_output_frames_f32(
    data: &mut [f32],
    channels: usize,
    out_rate: u32,
    queue: &mut VecDeque<i16>,
    src_cursor: &mut f32,
) {
    if channels == 0 {
        return;
    }
    let frame_count = data.len() / channels;
    let step = TARGET_RATE as f32 / out_rate as f32;
    for frame_idx in 0..frame_count {
        let sample = queue
            .get((*src_cursor).floor() as usize)
            .copied()
            .unwrap_or(0);
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
}

fn write_output_frames_u16(
    data: &mut [u16],
    channels: usize,
    out_rate: u32,
    queue: &mut VecDeque<i16>,
    src_cursor: &mut f32,
) {
    if channels == 0 {
        return;
    }
    let frame_count = data.len() / channels;
    let step = TARGET_RATE as f32 / out_rate as f32;
    for frame_idx in 0..frame_count {
        let sample = queue
            .get((*src_cursor).floor() as usize)
            .copied()
            .unwrap_or(0);
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
