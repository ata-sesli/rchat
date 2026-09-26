use super::{outbound::run_voice_writer, protocol::VoiceFrameRequest, queue::voice_queue};
use futures::io::AsyncWrite;
use std::future::Future;
use std::{
    io,
    pin::Pin,
    sync::{Arc, Mutex},
    task::{Context, Poll},
    time::Duration,
};

struct Writer {
    bytes: Arc<Mutex<Vec<u8>>>,
    limit: usize,
    stall_flush: Option<usize>,
    dropped: Arc<std::sync::atomic::AtomicBool>,
}
impl Drop for Writer {
    fn drop(&mut self) {
        self.dropped
            .store(true, std::sync::atomic::Ordering::SeqCst);
    }
}
impl AsyncWrite for Writer {
    fn poll_write(
        self: Pin<&mut Self>,
        _: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<io::Result<usize>> {
        let mut out = self.bytes.lock().unwrap();
        let available = self.limit.saturating_sub(out.len());
        if available == 0 {
            return Poll::Pending;
        }
        let n = available.min(bytes.len());
        out.extend_from_slice(&bytes[..n]);
        Poll::Ready(Ok(n))
    }
    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        if self
            .stall_flush
            .is_some_and(|limit| self.bytes.lock().unwrap().len() > limit)
        {
            Poll::Pending
        } else {
            Poll::Ready(Ok(()))
        }
    }
    fn poll_close(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

#[tokio::test]
async fn stalled_header_partial_frame_and_flush_drop_the_stream() {
    for (limit, stall_flush) in [
        (0, None),
        (5, None),
        (usize::MAX, Some(0)),
        (usize::MAX, Some(3)),
    ] {
        let bytes = Arc::new(Mutex::new(Vec::new()));
        let dropped = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let writer = Writer {
            bytes: bytes.clone(),
            limit,
            stall_flush,
            dropped: dropped.clone(),
        };
        let (tx, rx) = voice_queue();
        for seq in 0..5 {
            tx.push(VoiceFrameRequest {
                call_id: "c".into(),
                seq,
                timestamp: 0,
                payload: vec![1; 10],
            })
            .unwrap();
        }
        let error = tokio::time::timeout(Duration::from_secs(2), run_voice_writer(writer, "c", rx))
            .await
            .unwrap()
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        assert!(dropped.load(std::sync::atomic::Ordering::SeqCst));
        // No second framed record is written after a partial write or flush timeout.
        assert!(bytes.lock().unwrap().len() <= 3 + 14 + 10);
        assert!(tx
            .push(VoiceFrameRequest {
                call_id: "c".into(),
                seq: 99,
                timestamp: 0,
                payload: vec![]
            })
            .is_err());
    }
}

#[tokio::test]
async fn overflow_preserves_sequence_gaps_and_order() {
    let bytes = Arc::new(Mutex::new(Vec::new()));
    let writer = Writer {
        bytes: bytes.clone(),
        limit: usize::MAX,
        stall_flush: None,
        dropped: Default::default(),
    };
    let (tx, rx) = voice_queue();
    for seq in 0..50 {
        tx.push(VoiceFrameRequest {
            call_id: "c".into(),
            seq,
            timestamp: 0,
            payload: vec![1],
        })
        .unwrap();
    }
    drop(tx);
    run_voice_writer(writer, "c", rx).await.unwrap();
    let bytes = bytes.lock().unwrap();
    let sequences: Vec<_> = bytes[3..]
        .as_chunks::<15>()
        .0
        .iter()
        .map(|chunk| {
            super::protocol::decode_voice_stream_frame(chunk)
                .unwrap()
                .seq
        })
        .collect();
    assert_eq!(sequences, vec![45, 46, 47, 48, 49]);
}

struct SlowWriter {
    inner: Writer,
    delay: Option<Pin<Box<tokio::time::Sleep>>>,
}
impl AsyncWrite for SlowWriter {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.inner).poll_write(cx, bytes)
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let delay = self
            .delay
            .get_or_insert_with(|| Box::pin(tokio::time::sleep(Duration::from_millis(60))));
        if delay.as_mut().poll(cx).is_pending() {
            return Poll::Pending;
        }
        self.delay = None;
        Poll::Ready(Ok(()))
    }
    fn poll_close(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_close(cx)
    }
}

#[tokio::test]
async fn sustained_slow_writer_discards_backlog_and_recovers_with_recent_audio() {
    let bytes = Arc::new(Mutex::new(Vec::new()));
    let writer = SlowWriter {
        inner: Writer {
            bytes: bytes.clone(),
            limit: usize::MAX,
            stall_flush: None,
            dropped: Default::default(),
        },
        delay: None,
    };
    let (tx, rx) = voice_queue();
    let task = tokio::spawn(run_voice_writer(writer, "c", rx));
    for seq in 0..30 {
        tx.push(VoiceFrameRequest {
            call_id: "c".into(),
            seq,
            timestamp: 0,
            payload: vec![1],
        })
        .unwrap();
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    // Let the small backlog expire, then send one new frame after recovery.
    tokio::time::sleep(Duration::from_millis(250)).await;
    let stats = tx.stats();
    assert!(stats.overflow + stats.stale > 0);
    tx.push(VoiceFrameRequest {
        call_id: "c".into(),
        seq: 100,
        timestamp: 0,
        payload: vec![1],
    })
    .unwrap();
    drop(tx);
    tokio::time::timeout(Duration::from_secs(2), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let bytes = bytes.lock().unwrap();
    let sequences: Vec<_> = bytes[3..]
        .as_chunks::<15>()
        .0
        .iter()
        .map(|chunk| {
            super::protocol::decode_voice_stream_frame(chunk)
                .unwrap()
                .seq
        })
        .collect();
    assert_eq!(sequences.last(), Some(&100));
    assert!(sequences.len() < 30);
    assert!(sequences.windows(2).all(|pair| pair[0] < pair[1]));
}

#[tokio::test]
async fn call_teardown_cancels_an_inflight_write_and_releases_the_queue() {
    let dropped = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let writer = Writer {
        bytes: Default::default(),
        limit: 0,
        stall_flush: None,
        dropped: dropped.clone(),
    };
    let (tx, rx) = voice_queue();
    let task = tokio::spawn(run_voice_writer(writer, "c", rx));
    tokio::task::yield_now().await;
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    assert!(dropped.load(std::sync::atomic::Ordering::SeqCst));
    assert!(tx
        .push(VoiceFrameRequest {
            call_id: "c".into(),
            seq: 0,
            timestamp: 0,
            payload: vec![1]
        })
        .is_err());
}
