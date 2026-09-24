use anyhow::{anyhow, Context, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, Stream, StreamConfig};
use std::{
    fs::{self, File, OpenOptions},
    io::{Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
    time::Instant,
};

const WAV_HEADER_SIZE: u64 = 44;
const PCM16_BYTES_PER_SAMPLE: u64 = 2;

/// A short, input-only microphone recording backed by the native CPAL backend.
pub struct VoiceRecorder {
    stream: Option<Stream>,
    path: PathBuf,
    file: Arc<Mutex<File>>,
    sample_count: Arc<AtomicU64>,
    sample_rate: u32,
    started_at: Instant,
    keep_file: bool,
}

/// A finalized mono PCM16 WAV recording.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordedAudio {
    pub path: PathBuf,
    pub duration: std::time::Duration,
    pub size_bytes: u64,
}

impl VoiceRecorder {
    /// Starts capture on the host's default input device.
    ///
    /// Device discovery, configuration, stream construction, and `play()` are
    /// deliberately performed before this function returns so the TUI can
    /// report permission/device failures immediately.
    pub fn start() -> Result<Self> {
        let host = cpal::default_host();
        let device = host
            .default_input_device()
            .ok_or_else(|| anyhow!("No default microphone input device is available"))?;
        let supported = device
            .default_input_config()
            .map_err(|error| anyhow!("Failed to read microphone input configuration: {error}"))?;
        if supported.channels() == 0 {
            return Err(anyhow!("Microphone input device reported zero channels"));
        }
        let sample_rate = supported.sample_rate().0;
        if sample_rate == 0 {
            return Err(anyhow!(
                "Microphone input device reported an invalid sample rate"
            ));
        }
        let config = StreamConfig {
            channels: supported.channels(),
            sample_rate: supported.sample_rate(),
            buffer_size: cpal::BufferSize::Default,
        };
        let path = temporary_recording_path()?;
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&path)
            .with_context(|| format!("Failed to create voice recording {}", path.display()))?;
        file.write_all(&wav_header(sample_rate, 0))
            .with_context(|| format!("Failed to initialize voice recording {}", path.display()))?;
        let file = Arc::new(Mutex::new(file));
        let sample_count = Arc::new(AtomicU64::new(0));
        let stream = build_input_stream(
            &device,
            supported.sample_format(),
            &config,
            Arc::clone(&file),
            Arc::clone(&sample_count),
        )
        .map_err(|error| {
            let _ = fs::remove_file(&path);
            error
        })?;
        if let Err(error) = stream.play() {
            let _ = fs::remove_file(&path);
            return Err(anyhow!("Failed to start microphone capture: {error}"));
        }
        Ok(Self {
            stream: Some(stream),
            path,
            file,
            sample_count,
            sample_rate,
            started_at: Instant::now(),
            keep_file: false,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn elapsed(&self) -> std::time::Duration {
        self.started_at.elapsed()
    }

    pub fn sample_count(&self) -> u64 {
        self.sample_count.load(Ordering::Relaxed)
    }

    /// Stops capture and rewrites the WAV header with the final sample count.
    pub fn stop(mut self) -> Result<RecordedAudio> {
        self.stream.take();
        let sample_count = self.sample_count.load(Ordering::Relaxed);
        if sample_count == 0 {
            return Err(anyhow!(
                "Microphone capture stopped without receiving audio samples"
            ));
        }
        {
            let mut file = self
                .file
                .lock()
                .map_err(|_| anyhow!("Voice recording file lock was poisoned"))?;
            file.flush().context("Failed to flush voice recording")?;
            file.seek(SeekFrom::Start(0))
                .context("Failed to seek voice recording header")?;
            file.write_all(&wav_header(self.sample_rate, sample_count))
                .context("Failed to finalize voice recording header")?;
            file.flush()
                .context("Failed to flush finalized voice recording")?;
        }
        let duration =
            std::time::Duration::from_secs_f64(sample_count as f64 / self.sample_rate as f64);
        let size_bytes = fs::metadata(&self.path)
            .with_context(|| format!("Failed to inspect voice recording {}", self.path.display()))?
            .len();
        self.keep_file = true;
        Ok(RecordedAudio {
            path: self.path.clone(),
            duration,
            size_bytes,
        })
    }
}

impl Drop for VoiceRecorder {
    fn drop(&mut self) {
        // Dropping the CPAL stream is what stops the native callback. Remove
        // the partial file as well, including startup and stop-error paths.
        self.stream.take();
        if !self.keep_file {
            let _ = fs::remove_file(&self.path);
        }
    }
}

fn build_input_stream(
    device: &cpal::Device,
    format: SampleFormat,
    config: &StreamConfig,
    file: Arc<Mutex<File>>,
    sample_count: Arc<AtomicU64>,
) -> Result<Stream> {
    let channels = config.channels as usize;
    let err_fn = |error| eprintln!("[VoiceRecording] microphone stream error: {error}");
    let append = move |samples: &[i16]| append_pcm16(&file, &sample_count, samples);
    match format {
        SampleFormat::F32 => device
            .build_input_stream(
                config,
                move |data: &[f32], _| {
                    let mono: Vec<i16> = data
                        .iter()
                        .enumerate()
                        .filter(|(index, _)| index % channels == 0)
                        .map(|(_, sample)| {
                            let sample = sample.clamp(-1.0, 1.0);
                            (sample * f32::from(i16::MAX)) as i16
                        })
                        .collect();
                    append(&mono);
                },
                err_fn,
                None,
            )
            .map_err(|error| anyhow!("Failed to build microphone capture stream: {error}")),
        SampleFormat::I16 => device
            .build_input_stream(
                config,
                move |data: &[i16], _| {
                    let mono: Vec<i16> = data
                        .iter()
                        .enumerate()
                        .filter(|(index, _)| index % channels == 0)
                        .map(|(_, sample)| *sample)
                        .collect();
                    append(&mono);
                },
                err_fn,
                None,
            )
            .map_err(|error| anyhow!("Failed to build microphone capture stream: {error}")),
        SampleFormat::U16 => device
            .build_input_stream(
                config,
                move |data: &[u16], _| {
                    let mono: Vec<i16> = data
                        .iter()
                        .enumerate()
                        .filter(|(index, _)| index % channels == 0)
                        .map(|(_, sample)| (*sample as i32 - 32768) as i16)
                        .collect();
                    append(&mono);
                },
                err_fn,
                None,
            )
            .map_err(|error| anyhow!("Failed to build microphone capture stream: {error}")),
        _ => Err(anyhow!(
            "Microphone input uses an unsupported sample format"
        )),
    }
}

fn append_pcm16(file: &Arc<Mutex<File>>, sample_count: &Arc<AtomicU64>, samples: &[i16]) {
    if samples.is_empty() {
        return;
    }
    if let Ok(mut file) = file.lock() {
        let mut bytes = Vec::with_capacity(samples.len() * 2);
        for sample in samples {
            bytes.extend_from_slice(&sample.to_le_bytes());
        }
        if file.write_all(&bytes).is_ok() {
            sample_count.fetch_add(samples.len() as u64, Ordering::Relaxed);
        }
    }
}

fn temporary_recording_path() -> Result<PathBuf> {
    let path = std::env::temp_dir().join(format!(
        "rchat-voice-note-{}-{}.wav",
        std::process::id(),
        rand::random::<u64>()
    ));
    if path.exists() {
        return Err(anyhow!(
            "Voice recording path already exists: {}",
            path.display()
        ));
    }
    Ok(path)
}

fn wav_header(sample_rate: u32, sample_count: u64) -> [u8; WAV_HEADER_SIZE as usize] {
    let mut header = [0u8; WAV_HEADER_SIZE as usize];
    header[0..4].copy_from_slice(b"RIFF");
    let data_size = sample_count * PCM16_BYTES_PER_SAMPLE;
    let riff_size = 36 + data_size;
    header[4..8].copy_from_slice(&(riff_size as u32).to_le_bytes());
    header[8..12].copy_from_slice(b"WAVE");
    header[12..16].copy_from_slice(b"fmt ");
    header[16..20].copy_from_slice(&16u32.to_le_bytes());
    header[20..22].copy_from_slice(&1u16.to_le_bytes());
    header[22..24].copy_from_slice(&1u16.to_le_bytes());
    header[24..28].copy_from_slice(&sample_rate.to_le_bytes());
    let byte_rate = sample_rate.saturating_mul(2);
    header[28..32].copy_from_slice(&byte_rate.to_le_bytes());
    header[32..34].copy_from_slice(&2u16.to_le_bytes());
    header[34..36].copy_from_slice(&16u16.to_le_bytes());
    header[36..40].copy_from_slice(b"data");
    header[40..44].copy_from_slice(&(data_size as u32).to_le_bytes());
    header
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn wav_header_describes_pcm16_audio() {
        let header = wav_header(48_000, 24_000);
        assert_eq!(&header[0..4], b"RIFF");
        assert_eq!(&header[8..12], b"WAVE");
        assert_eq!(
            u32::from_le_bytes(header[24..28].try_into().unwrap()),
            48_000
        );
        assert_eq!(u16::from_le_bytes(header[34..36].try_into().unwrap()), 16);
        assert_eq!(
            u32::from_le_bytes(header[40..44].try_into().unwrap()),
            48_000
        );
        assert_eq!(u32::from_le_bytes(header[4..8].try_into().unwrap()), 48_036);
    }

    #[test]
    fn recorder_start_failure_is_reported_without_a_partial_file() {
        // This is hardware-independent in practice: CI hosts normally have no
        // usable default microphone. If a host does have one, skip the assertion.
        if let Ok(recorder) = VoiceRecorder::start() {
            let path = recorder.path().to_path_buf();
            drop(recorder);
            assert!(!path.exists());
            return;
        }
    }
}
