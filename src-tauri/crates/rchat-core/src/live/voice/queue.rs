//! Short, latest-audio-first queues shared by capture, playback and the outbound writer.
//! Five 20 ms frames (100 ms) per queue; original capture age survives encoding.
//! Producers never wait for capacity or a lock: contention drops the incoming
//! frame, overflow evicts the oldest. Consumers never return expired audio.
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::Notify;

pub(crate) const VOICE_QUEUE_FRAMES: usize = 5;
pub(crate) const VOICE_MAX_AGE: Duration = Duration::from_millis(100);
pub(crate) const VOICE_FRAMES_PER_TICK: usize = 3;

#[derive(Debug)]
pub(crate) struct TimedFrame<T> {
    pub value: T,
    pub captured_at: Instant,
}

#[derive(Debug, Default)]
pub(crate) struct QueueStats {
    pub overflow: u64,
    pub stale: u64,
    pub contention: u64,
    pub discarded: u64,
}

struct Shared<T> {
    frames: Mutex<VecDeque<TimedFrame<T>>>,
    closed: AtomicBool,
    ready: Notify,
    overflow: AtomicU64,
    stale: AtomicU64,
    contention: AtomicU64,
    discarded: AtomicU64,
}

pub(crate) struct VoiceSender<T>(Arc<Shared<T>>);
pub(crate) struct VoiceReceiver<T>(Arc<Shared<T>>);

pub(crate) fn voice_queue<T>() -> (VoiceSender<T>, VoiceReceiver<T>) {
    let shared = Arc::new(Shared {
        frames: Mutex::new(VecDeque::with_capacity(VOICE_QUEUE_FRAMES)),
        closed: AtomicBool::new(false),
        ready: Notify::new(),
        overflow: AtomicU64::new(0),
        stale: AtomicU64::new(0),
        contention: AtomicU64::new(0),
        discarded: AtomicU64::new(0),
    });
    (VoiceSender(shared.clone()), VoiceReceiver(shared))
}

impl<T> Shared<T> {
    fn stats(&self) -> QueueStats {
        QueueStats {
            overflow: self.overflow.load(Ordering::Relaxed),
            stale: self.stale.load(Ordering::Relaxed),
            contention: self.contention.load(Ordering::Relaxed),
            discarded: self.discarded.load(Ordering::Relaxed),
        }
    }
}

impl<T> VoiceSender<T> {
    pub fn push(&self, value: T) -> Result<(), ()> {
        self.push_frame(TimedFrame {
            value,
            captured_at: Instant::now(),
        })
    }

    /// Err means the receiver has gone away. Deliberate drops are counted.
    pub fn push_frame(&self, frame: TimedFrame<T>) -> Result<(), ()> {
        if self.0.closed.load(Ordering::Acquire) {
            return Err(());
        }
        let Ok(mut frames) = self.0.frames.try_lock() else {
            self.0.contention.fetch_add(1, Ordering::Relaxed);
            return Ok(());
        };
        if self.0.closed.load(Ordering::Acquire) {
            return Err(());
        }
        if frames.len() == VOICE_QUEUE_FRAMES {
            frames.pop_front();
            self.0.overflow.fetch_add(1, Ordering::Relaxed);
        }
        frames.push_back(frame);
        drop(frames);
        self.0.ready.notify_one();
        Ok(())
    }

    pub fn stats(&self) -> QueueStats {
        self.0.stats()
    }
}

impl<T> VoiceReceiver<T> {
    /// Snapshot at most three recent frames for one manager tick/output callback. Muted or
    /// disconnected calls discard the snapshot; no backlog survives recovery.
    pub fn capture_tick(&mut self, enabled: bool) -> Vec<TimedFrame<T>> {
        let Ok(mut frames) = self.0.frames.try_lock() else {
            return Vec::new();
        };
        let mut result = Vec::with_capacity(VOICE_FRAMES_PER_TICK);
        let keep = if enabled { VOICE_FRAMES_PER_TICK } else { 0 };
        while let Some(frame) = frames.pop_front() {
            if frame.captured_at.elapsed() >= VOICE_MAX_AGE {
                self.0.stale.fetch_add(1, Ordering::Relaxed);
            } else if frames.len() >= keep {
                self.0.discarded.fetch_add(1, Ordering::Relaxed);
            } else {
                result.push(frame);
            }
        }
        result
    }

    pub fn try_recv(&mut self) -> Option<TimedFrame<T>> {
        let mut frames = self.0.frames.try_lock().ok()?;
        // At most five entries, regardless of producer rate.
        while let Some(frame) = frames.pop_front() {
            if frame.captured_at.elapsed() < VOICE_MAX_AGE {
                return Some(frame);
            }
            self.0.stale.fetch_add(1, Ordering::Relaxed);
        }
        None
    }

    pub async fn recv(&mut self) -> Option<TimedFrame<T>> {
        loop {
            if let Some(frame) = self.try_recv() {
                return Some(frame);
            }
            if self.0.closed.load(Ordering::Acquire) {
                // The sender may have pushed its last frame between the first
                // empty check and publishing closure.
                return self.try_recv();
            }
            self.0.ready.notified().await;
        }
    }

    pub fn stats(&self) -> QueueStats {
        self.0.stats()
    }
}

impl<T> Drop for VoiceSender<T> {
    fn drop(&mut self) {
        self.0.closed.store(true, Ordering::Release);
        self.0.ready.notify_one();
    }
}

impl<T> Drop for VoiceReceiver<T> {
    fn drop(&mut self) {
        self.0.closed.store(true, Ordering::Release);
        if let Ok(mut frames) = self.0.frames.lock() {
            frames.clear();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn callback_drops_instead_of_waiting_for_a_contended_lock() {
        let (tx, _rx) = voice_queue();
        let _guard = tx.0.frames.lock().unwrap();
        assert!(tx.push(1).is_ok());
        assert_eq!(tx.stats().contention, 1);
    }
}
