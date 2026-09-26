use std::collections::BTreeMap;

const START_PREBUFFER_FRAMES: usize = 3;
const MAX_BUFFERED_FRAMES: usize = 32;
const MAX_LATE_FRAMES: u32 = 12;
const MAX_GAP_BEFORE_RESYNC: u32 = 10;

#[derive(Debug, Default)]
pub struct VoiceJitterBuffer {
    expected_seq: Option<u32>,
    prebuffering: bool,
    pending: BTreeMap<u32, Vec<i16>>,
}

impl VoiceJitterBuffer {
    pub fn new() -> Self {
        Self {
            expected_seq: None,
            prebuffering: true,
            pending: BTreeMap::new(),
        }
    }

    pub fn reset(&mut self) {
        self.expected_seq = None;
        self.prebuffering = true;
        self.pending.clear();
    }

    /// Receive a frame from a reliable ordered stream. Sequence gaps represent
    /// discarded audio, not packets that can arrive later. Retain the initial
    /// three-frame prebuffer, then release each frame without waiting on gaps.
    /// Use this instead of `push` for the whole stream, resetting between calls.
    pub fn push_ordered(&mut self, seq: u32, frame: Vec<i16>) -> Vec<Vec<i16>> {
        let first = *self.expected_seq.get_or_insert(seq);
        // Serial-number arithmetic also rejects duplicates/late frames after
        // playback crosses u32::MAX. Gaps must be less than half the sequence space.
        if seq.wrapping_sub(first) >= (1 << 31) {
            return Vec::new();
        }
        self.pending.entry(seq).or_insert(frame);
        if self.prebuffering && self.pending.len() < START_PREBUFFER_FRAMES {
            return Vec::new();
        }
        self.prebuffering = false;
        // At most three entries. Numeric BTreeMap order alone is wrong at wrap.
        let mut ready: Vec<_> = std::mem::take(&mut self.pending).into_iter().collect();
        ready.sort_by_key(|(seq, _)| seq.wrapping_sub(first));
        self.expected_seq = ready.last().map(|(seq, _)| seq.wrapping_add(1));
        ready.into_iter().map(|(_, frame)| frame).collect()
    }

    pub fn push(&mut self, seq: u32, frame: Vec<i16>) -> Vec<Vec<i16>> {
        if self.expected_seq.is_none() {
            self.expected_seq = Some(seq);
        }

        let expected = match self.expected_seq {
            Some(v) => v,
            None => return Vec::new(),
        };

        if seq < expected && expected.saturating_sub(seq) > MAX_LATE_FRAMES {
            return Vec::new();
        }

        if self.pending.len() >= MAX_BUFFERED_FRAMES {
            if let Some(oldest_key) = self.pending.keys().next().copied() {
                self.pending.remove(&oldest_key);
            }
        }

        self.pending.entry(seq).or_insert(frame);
        self.drain_ready_frames()
    }

    fn drain_ready_frames(&mut self) -> Vec<Vec<i16>> {
        let mut ready = Vec::new();
        let mut expected = match self.expected_seq {
            Some(v) => v,
            None => return ready,
        };

        if self.prebuffering {
            let mut contiguous = 0usize;
            while self
                .pending
                .contains_key(&expected.wrapping_add(contiguous as u32))
                && contiguous < START_PREBUFFER_FRAMES
            {
                contiguous += 1;
            }

            if contiguous < START_PREBUFFER_FRAMES {
                if self.pending.len() >= START_PREBUFFER_FRAMES * 2 {
                    if let Some((&lowest, _)) = self.pending.iter().next() {
                        expected = lowest;
                        self.expected_seq = Some(lowest);
                    }
                } else {
                    return ready;
                }
            }

            self.prebuffering = false;
        }

        while let Some(frame) = self.pending.remove(&expected) {
            ready.push(frame);
            expected = expected.wrapping_add(1);
        }

        if ready.is_empty() {
            if let Some((&lowest, _)) = self.pending.iter().next() {
                if lowest > expected && lowest - expected > MAX_GAP_BEFORE_RESYNC {
                    expected = lowest;
                    self.prebuffering = true;
                }
            }
        }

        self.expected_seq = Some(expected);
        ready
    }
}

#[cfg(test)]
mod tests {
    use crate::live::voice::codec::VOICE_FRAME_SAMPLES;

    use super::VoiceJitterBuffer;

    fn frame(sample: i16) -> Vec<i16> {
        vec![sample; VOICE_FRAME_SAMPLES]
    }

    #[test]
    fn reorders_with_small_jitter_window() {
        let mut jitter = VoiceJitterBuffer::new();
        assert!(jitter.push(10, frame(10)).is_empty());
        assert!(jitter.push(12, frame(12)).is_empty());
        let out = jitter.push(11, frame(11));
        assert_eq!(out.len(), 3);
        assert_eq!(out[0][0], 10);
        assert_eq!(out[1][0], 11);
        assert_eq!(out[2][0], 12);
    }

    #[test]
    fn drops_frames_that_are_too_late() {
        let mut jitter = VoiceJitterBuffer::new();
        assert!(jitter.push(100, frame(100)).is_empty());
        assert!(jitter.push(101, frame(101)).is_empty());
        let _ = jitter.push(102, frame(102));
        let out = jitter.push(80, frame(80));
        assert!(out.is_empty());
    }

    #[test]
    fn reset_clears_state() {
        let mut jitter = VoiceJitterBuffer::new();
        assert!(jitter.push(5, frame(5)).is_empty());
        jitter.reset();
        assert!(jitter.push(42, frame(42)).is_empty());
        assert!(jitter.push(43, frame(43)).is_empty());
        let out = jitter.push(44, frame(44));
        assert_eq!(out.len(), 3);
        assert_eq!(out[0][0], 42);
    }

    #[test]
    fn ordered_startup_counts_received_frames_not_contiguous_sequences() {
        let mut jitter = VoiceJitterBuffer::new();
        assert!(jitter.push_ordered(0, frame(0)).is_empty());
        assert!(jitter.push_ordered(2, frame(2)).is_empty());
        let out = jitter.push_ordered(4, frame(4));
        assert_eq!(out.iter().map(|f| f[0]).collect::<Vec<_>>(), vec![0, 2, 4]);
        assert_eq!(jitter.push_ordered(100, frame(100))[0][0], 100);
        assert!(jitter.push_ordered(100, frame(100)).is_empty());
        assert!(jitter.push_ordered(99, frame(99)).is_empty());
        jitter.reset();
        assert!(jitter.push_ordered(0, frame(0)).is_empty());
    }

    #[test]
    fn ordered_startup_and_gaps_handle_sequence_wrap() {
        let mut jitter = VoiceJitterBuffer::new();
        assert!(jitter.push_ordered(u32::MAX - 1, frame(1)).is_empty());
        assert!(jitter.push_ordered(u32::MAX, frame(2)).is_empty());
        let out = jitter.push_ordered(0, frame(3));
        assert_eq!(out.iter().map(|f| f[0]).collect::<Vec<_>>(), vec![1, 2, 3]);
        assert_eq!(jitter.push_ordered(2, frame(4))[0][0], 4);
        assert!(jitter.push_ordered(u32::MAX, frame(5)).is_empty());
    }
}
