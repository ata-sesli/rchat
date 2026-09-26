use super::protocol::{write_voice_stream_frame, write_voice_stream_header, VoiceFrameRequest};
use super::queue::VoiceReceiver;
use futures::io::{AsyncWrite, AsyncWriteExt};
use std::{io, time::Duration};

// Covers the complete header or frame (including flush), not individual writes.
// There is at most one in-flight frame in addition to the five queued frames.
const WRITE_DEADLINE: Duration = Duration::from_millis(200);

/// Own the stream so cancellation or a partial-record timeout drops it before
/// the manager receives an error. Never continue framing on an interrupted stream.
pub(crate) async fn run_voice_writer<W: AsyncWrite + Unpin>(
    mut stream: W,
    call_id: &str,
    mut rx: VoiceReceiver<VoiceFrameRequest>,
) -> io::Result<()> {
    tokio::time::timeout(WRITE_DEADLINE, async {
        write_voice_stream_header(&mut stream, call_id).await?;
        stream.flush().await
    })
    .await
    .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "voice header write timed out"))??;
    while let Some(frame) = rx.recv().await {
        let frame = frame.value;
        tokio::time::timeout(
            WRITE_DEADLINE,
            write_voice_stream_frame(&mut stream, frame.seq, frame.timestamp, &frame.payload),
        )
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "voice frame write timed out"))??;
    }
    tokio::time::timeout(WRITE_DEADLINE, stream.close())
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "voice stream close timed out"))?
}
