use sonora::config::{EchoCanceller, HighPassFilter, MaxProcessingRate, Pipeline};
use sonora::{AudioProcessing, Config, StreamConfig};
use std::error::Error;
use std::fmt;

pub const RCHAT_AEC_SAMPLE_RATE: u32 = 48_000;
pub const RCHAT_AEC_CHANNELS: u16 = 1;
pub const RCHAT_AEC_10MS_SAMPLES: usize = 480;
pub const RCHAT_AEC_20MS_SAMPLES: usize = 960;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct EchoCancellerStats {
    pub render_blocks: u64,
    pub capture_blocks: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AudioProcessingError {
    InvalidFrameLength { expected: usize, actual: usize },
    Sonora(String),
}

impl fmt::Display for AudioProcessingError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidFrameLength { expected, actual } => {
                write!(f, "expected {} samples, got {}", expected, actual)
            }
            Self::Sonora(message) => f.write_str(message),
        }
    }
}

impl Error for AudioProcessingError {}

pub struct RchatEchoCanceller {
    processor: AudioProcessing,
    render_out: Vec<i16>,
    capture_out: Vec<i16>,
    stats: EchoCancellerStats,
}

impl RchatEchoCanceller {
    pub fn new_48khz_mono() -> Result<Self, AudioProcessingError> {
        let stream = StreamConfig::new(RCHAT_AEC_SAMPLE_RATE, RCHAT_AEC_CHANNELS);
        let processor = AudioProcessing::builder()
            .config(Config {
                pipeline: Pipeline {
                    maximum_internal_processing_rate: MaxProcessingRate::Rate48kHz,
                    ..Default::default()
                },
                high_pass_filter: Some(HighPassFilter::default()),
                echo_canceller: Some(EchoCanceller::default()),
                ..Default::default()
            })
            .capture_config(stream)
            .render_config(stream)
            .build();

        Ok(Self {
            processor,
            render_out: vec![0; RCHAT_AEC_10MS_SAMPLES],
            capture_out: vec![0; RCHAT_AEC_10MS_SAMPLES],
            stats: EchoCancellerStats::default(),
        })
    }

    pub fn process_render_20ms_i16(
        &mut self,
        samples: &[i16],
    ) -> Result<(), AudioProcessingError> {
        validate_20ms_frame(samples)?;
        for block in samples.chunks_exact(RCHAT_AEC_10MS_SAMPLES) {
            self.processor
                .process_render_i16(block, &mut self.render_out)
                .map_err(|e| AudioProcessingError::Sonora(format!("render: {e:?}")))?;
            self.stats.render_blocks = self.stats.render_blocks.saturating_add(1);
        }
        Ok(())
    }

    pub fn process_capture_20ms_i16(
        &mut self,
        samples: &[i16],
    ) -> Result<Vec<i16>, AudioProcessingError> {
        validate_20ms_frame(samples)?;
        let mut out = Vec::with_capacity(RCHAT_AEC_20MS_SAMPLES);
        for block in samples.chunks_exact(RCHAT_AEC_10MS_SAMPLES) {
            self.processor
                .process_capture_i16(block, &mut self.capture_out)
                .map_err(|e| AudioProcessingError::Sonora(format!("capture: {e:?}")))?;
            out.extend_from_slice(&self.capture_out);
            self.stats.capture_blocks = self.stats.capture_blocks.saturating_add(1);
        }
        Ok(out)
    }

    pub fn set_stream_delay_ms(&mut self, delay_ms: i32) -> Result<(), AudioProcessingError> {
        self.processor
            .set_stream_delay_ms(delay_ms)
            .map_err(|e| AudioProcessingError::Sonora(format!("stream delay: {e:?}")))
    }

    pub fn stats(&self) -> EchoCancellerStats {
        self.stats
    }
}

fn validate_20ms_frame(samples: &[i16]) -> Result<(), AudioProcessingError> {
    if samples.len() == RCHAT_AEC_20MS_SAMPLES {
        Ok(())
    } else {
        Err(AudioProcessingError::InvalidFrameLength {
            expected: RCHAT_AEC_20MS_SAMPLES,
            actual: samples.len(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn silence_frame() -> [i16; RCHAT_AEC_20MS_SAMPLES] {
        [0; RCHAT_AEC_20MS_SAMPLES]
    }

    fn synthetic_render_frame() -> [i16; RCHAT_AEC_20MS_SAMPLES] {
        let mut frame = [0; RCHAT_AEC_20MS_SAMPLES];
        for (idx, sample) in frame.iter_mut().enumerate() {
            let phase =
                (idx as f32 / RCHAT_AEC_SAMPLE_RATE as f32) * 440.0 * std::f32::consts::TAU;
            *sample = (phase.sin() * 10_000.0) as i16;
        }
        frame
    }

    fn rms(samples: &[i16]) -> f64 {
        let energy = samples
            .iter()
            .map(|sample| {
                let sample = *sample as f64;
                sample * sample
            })
            .sum::<f64>();
        (energy / samples.len().max(1) as f64).sqrt()
    }

    #[test]
    fn processes_48khz_mono_20ms_frame() {
        let mut aec = RchatEchoCanceller::new_48khz_mono().expect("aec starts");
        let frame = silence_frame();

        aec.process_render_20ms_i16(&frame).expect("render works");
        let processed = aec
            .process_capture_20ms_i16(&frame)
            .expect("capture works");

        assert_eq!(processed.len(), RCHAT_AEC_20MS_SAMPLES);
    }

    #[test]
    fn splits_20ms_into_two_10ms_blocks() {
        let mut aec = RchatEchoCanceller::new_48khz_mono().expect("aec starts");
        let frame = silence_frame();

        aec.process_render_20ms_i16(&frame).expect("render works");
        let processed = aec
            .process_capture_20ms_i16(&frame)
            .expect("capture works");
        let stats = aec.stats();

        assert_eq!(processed.len(), RCHAT_AEC_20MS_SAMPLES);
        assert_eq!(stats.render_blocks, 2);
        assert_eq!(stats.capture_blocks, 2);
    }

    #[test]
    fn rejects_non_960_sample_frames() {
        let mut aec = RchatEchoCanceller::new_48khz_mono().expect("aec starts");

        let err = aec
            .process_capture_20ms_i16(&[0; RCHAT_AEC_20MS_SAMPLES - 1])
            .expect_err("invalid frame length rejected");

        assert!(matches!(err, AudioProcessingError::InvalidFrameLength { .. }));
    }

    #[test]
    fn echo_only_48khz_mono_does_not_panic() {
        let mut aec = RchatEchoCanceller::new_48khz_mono().expect("aec starts");
        let render = synthetic_render_frame();

        for _ in 0..600 {
            aec.process_render_20ms_i16(&render).expect("render works");
            let capture = render
                .iter()
                .map(|sample| ((*sample as f32) * 0.45) as i16)
                .collect::<Vec<_>>();
            let _ = aec
                .process_capture_20ms_i16(&capture)
                .expect("capture works");
        }
    }

    #[test]
    fn i16_echo_reduction_smoke_test() {
        let mut aec = RchatEchoCanceller::new_48khz_mono().expect("aec starts");
        let render = synthetic_render_frame();
        let mut input = Vec::new();
        let mut output = Vec::new();

        for frame_idx in 0..800 {
            aec.process_render_20ms_i16(&render).expect("render works");
            let capture = render
                .iter()
                .map(|sample| ((*sample as f32) * 0.45) as i16)
                .collect::<Vec<_>>();
            let processed = aec
                .process_capture_20ms_i16(&capture)
                .expect("capture works");
            if frame_idx >= 200 {
                input.extend(capture);
                output.extend(processed);
            }
        }

        assert!(rms(&output) < rms(&input) * 0.25);
    }
}
