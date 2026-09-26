use super::queue::{voice_queue, TimedFrame, VOICE_MAX_AGE, VOICE_QUEUE_FRAMES};
use std::time::Instant;

#[test]
fn delayed_consumer_keeps_only_the_five_newest_frames() {
    let (tx, mut rx) = voice_queue();
    for value in 0..1000 {
        assert!(tx.push(value).is_ok());
    }
    assert_eq!(tx.stats().overflow, 1000 - VOICE_QUEUE_FRAMES as u64);
    for value in 995..1000 {
        assert_eq!(rx.try_recv().unwrap().value, value);
    }
    assert!(rx.try_recv().is_none());
}

#[test]
fn stale_capture_is_not_replayed_after_stream_recovery() {
    let (tx, mut rx) = voice_queue();
    tx.push_frame(TimedFrame {
        value: 1,
        captured_at: Instant::now() - VOICE_MAX_AGE * 2,
    })
    .unwrap();
    assert!(rx.try_recv().is_none());
    assert_eq!(tx.stats().stale, 1);
    tx.push(2).unwrap();
    assert_eq!(rx.try_recv().unwrap().value, 2);
}

#[tokio::test]
async fn shutdown_wakes_waiting_writer_and_restart_has_no_old_frames() {
    let (tx, mut rx) = voice_queue::<u8>();
    drop(tx);
    assert!(rx.recv().await.is_none());
    let (tx, rx) = voice_queue();
    tx.push(1).unwrap();
    drop(rx);
    assert!(tx.push(2).is_err());
    let (_new_tx, mut new_rx) = voice_queue::<u8>();
    assert!(new_rx.try_recv().is_none());
}

#[test]
fn capture_tick_limits_work_and_discards_muted_or_disconnected_audio() {
    let (tx, mut rx) = voice_queue();
    for value in 0..5 {
        tx.push(value).unwrap();
    }
    let frames = rx.capture_tick(true);
    assert_eq!(
        frames
            .into_iter()
            .map(|frame| frame.value)
            .collect::<Vec<_>>(),
        vec![2, 3, 4]
    );
    assert_eq!(rx.stats().discarded, 2);
    for _ in 0..100 {
        for value in 0..5 {
            tx.push(value).unwrap();
        }
        assert!(rx.capture_tick(false).is_empty());
    }
    assert!(rx.capture_tick(true).is_empty());
    assert_eq!(rx.stats().discarded, 502);
}
