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
            while self.pending.contains_key(&expected.wrapping_add(contiguous as u32))
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
    use super::VoiceJitterBuffer;

    fn frame(sample: i16) -> Vec<i16> {
        vec![sample; 320]
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
}
