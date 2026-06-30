export const VIDEO_RENDER_STATS_REPORT_INTERVAL_MS = 5_000;
export const MIN_VIDEO_RENDER_STATS_REPORT_WINDOW_MS = 4_500;

export type VideoRenderStatsCounterSnapshot = {
  received: number;
  rendered: number;
  dropped: number;
  decodeErrors: number;
};

export type VideoRenderStatsReport = {
  stats: {
    received_frames: number;
    rendered_frames: number;
    dropped_frames: number;
    decode_errors: number;
    window_seconds: number;
  };
  snapshot: VideoRenderStatsCounterSnapshot;
  windowStartedAtMs: number;
};

function delta(current: number, previous: number): number {
  return Math.max(0, current - previous);
}

export function buildVideoRenderStatsReport(
  current: VideoRenderStatsCounterSnapshot,
  previous: VideoRenderStatsCounterSnapshot,
  windowStartedAtMs: number,
  nowMs: number,
  minWindowMs = MIN_VIDEO_RENDER_STATS_REPORT_WINDOW_MS,
): VideoRenderStatsReport | null {
  const elapsedMs = Math.max(0, nowMs - windowStartedAtMs);
  if (elapsedMs < minWindowMs) return null;

  return {
    stats: {
      received_frames: delta(current.received, previous.received),
      rendered_frames: delta(current.rendered, previous.rendered),
      dropped_frames: delta(current.dropped, previous.dropped),
      decode_errors: delta(current.decodeErrors, previous.decodeErrors),
      window_seconds: elapsedMs / 1_000,
    },
    snapshot: current,
    windowStartedAtMs: nowMs,
  };
}
