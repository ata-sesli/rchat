pub const VOICE_SAMPLE_RATE: u32 = 16_000;
pub const VOICE_FRAME_SAMPLES: usize = 320;
pub const VOICE_OPUS_BITRATE: i32 = 24_000;
const VOICE_OPUS_MAX_PACKET_BYTES: usize = 1_275;

pub struct VoiceOpusEncoder {
    encoder: opus::Encoder,
}

impl VoiceOpusEncoder {
    pub fn new() -> Result<Self, String> {
        let mut encoder = opus::Encoder::new(
            VOICE_SAMPLE_RATE,
            opus::Channels::Mono,
            opus::Application::Voip,
        )
        .map_err(|e| e.to_string())?;
        encoder
            .set_bitrate(opus::Bitrate::Bits(VOICE_OPUS_BITRATE))
            .map_err(|e| e.to_string())?;
        encoder.set_vbr(true).map_err(|e| e.to_string())?;
        encoder
            .set_vbr_constraint(true)
            .map_err(|e| e.to_string())?;
        encoder.set_dtx(false).map_err(|e| e.to_string())?;

        Ok(Self { encoder })
    }

    pub fn encode_frame(&mut self, samples: &[i16]) -> Result<Vec<u8>, String> {
        if samples.len() != VOICE_FRAME_SAMPLES {
            return Err(format!(
                "expected {} voice samples, got {}",
                VOICE_FRAME_SAMPLES,
                samples.len()
            ));
        }
        self.encoder
            .encode_vec(samples, VOICE_OPUS_MAX_PACKET_BYTES)
            .map_err(|e| e.to_string())
    }
}

pub struct VoiceOpusDecoder {
    decoder: opus::Decoder,
}

impl VoiceOpusDecoder {
    pub fn new() -> Result<Self, String> {
        let decoder = opus::Decoder::new(VOICE_SAMPLE_RATE, opus::Channels::Mono)
            .map_err(|e| e.to_string())?;
        Ok(Self { decoder })
    }

    pub fn decode_packet(&mut self, packet: &[u8]) -> Result<Vec<i16>, String> {
        let mut output = vec![0; VOICE_FRAME_SAMPLES];
        let decoded = self
            .decoder
            .decode(packet, &mut output, false)
            .map_err(|e| e.to_string())?;
        output.truncate(decoded);
        if output.len() != VOICE_FRAME_SAMPLES {
            return Err(format!(
                "expected {} decoded voice samples, got {}",
                VOICE_FRAME_SAMPLES,
                output.len()
            ));
        }
        Ok(output)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn silence_frame() -> [i16; VOICE_FRAME_SAMPLES] {
        [0; VOICE_FRAME_SAMPLES]
    }

    fn synthetic_voice_frame() -> [i16; VOICE_FRAME_SAMPLES] {
        let mut frame = [0; VOICE_FRAME_SAMPLES];
        for (idx, sample) in frame.iter_mut().enumerate() {
            let phase = (idx as f32 / VOICE_SAMPLE_RATE as f32) * 440.0 * std::f32::consts::TAU;
            *sample = (phase.sin() * 12_000.0) as i16;
        }
        frame
    }

    #[test]
    fn opus_encoder_produces_compact_packet_for_silence() {
        let mut encoder = VoiceOpusEncoder::new().expect("encoder starts");

        let packet = encoder
            .encode_frame(&silence_frame())
            .expect("encodes silence");

        assert!(!packet.is_empty());
        assert!(packet.len() < VOICE_FRAME_SAMPLES * std::mem::size_of::<i16>());
    }

    #[test]
    fn opus_round_trip_decodes_one_voice_frame() {
        let mut encoder = VoiceOpusEncoder::new().expect("encoder starts");
        let mut decoder = VoiceOpusDecoder::new().expect("decoder starts");

        let packet = encoder
            .encode_frame(&synthetic_voice_frame())
            .expect("encodes frame");
        let decoded = decoder.decode_packet(&packet).expect("decodes packet");

        assert_eq!(decoded.len(), VOICE_FRAME_SAMPLES);
        assert!(decoded.iter().any(|sample| *sample != 0));
    }

    #[test]
    fn opus_decoder_rejects_invalid_packet_without_panicking() {
        let mut decoder = VoiceOpusDecoder::new().expect("decoder starts");

        let err = decoder
            .decode_packet(&[0xff, 0x00, 0xff, 0x00])
            .expect_err("invalid packet rejected");

        assert!(!err.to_string().is_empty());
    }
}
