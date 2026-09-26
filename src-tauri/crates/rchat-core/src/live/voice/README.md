# Voice queue budgets

The microphone-to-manager and manager-to-QUIC queues each hold at most five
20 ms frames (100 ms). Capture PCM storage is at most 9,600 sample bytes per
queue, excluding container overhead. The outbound queue contains encoded Opus
packets. The writer can additionally hold one in-flight packet, and the manager
can hold a three-frame processing snapshot.

- Overflow evicts the oldest queued frame to retain recent speech.
- Producers use `try_lock`, never waiting for capacity or queue access. If the
  queue is briefly locked, the incoming frame is discarded and counted.
- Both consumers reject frames aged 100 ms or more. The original monotonic
  capture timestamp survives encoding, so the outbound queue does not restart
  that age budget.
- Each 20 ms manager tick encodes at most three recent frames. Excess capture
  frames are discarded. Muted calls and calls without a writer discard capture
  instead of saving speech for a later unmute/reconnection.
- Header writes (including flush), frame writes (including flush), and graceful
  close each have a 200 ms deadline. Stream opening retains its five-second
  deadline. A failed or timed-out write drops the owned stream; no subsequent
  record is written on that potentially partial stream. The existing call
  failure handling ends the call rather than silently restarting framing.
- Call teardown aborts the writer and releases both queues. Dropping the network
  manager also aborts its writer instead of detaching the task.

`[Voice][Queues]` summaries report `capture_overflow`, `capture_stale`,
`capture_contention`, `capture_discarded`, `outbound_overflow`, `outbound_stale`,
and `outbound_contention`. Discarded counts cover the per-tick budget, mute, and
missing-writer cases. Packet sequence numbers are not renumbered after outbound
drops, so receivers can observe gaps. These counters are local diagnostics;
the wire format is unchanged.

The receive path uses ordered-stream jitter buffering: after the initial three
received frames, each decoded frame is immediately playable even across sequence
gaps. Missing sequence numbers cannot arrive later on the same reliable ordered
stream, so the receiver does not build a stale backlog waiting for them. Startup
also counts received frames rather than requiring three contiguous sequences.

These are application-queue budgets, not an end-to-end network latency promise.
QUIC, device, and playback buffers have separate behavior. Tests exercise the
queues and framed writer without microphones, including saturation, old frames,
slow writes, partial-write/flush stalls, recovery, and cancellation. Real-device
and real-network call testing remains separate.
