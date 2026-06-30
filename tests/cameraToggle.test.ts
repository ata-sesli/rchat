// @ts-nocheck
import { describe, expect, test } from "bun:test";
import {
  createLocalCameraToggleState,
  markLocalCameraToggleSettled,
  requestLocalCameraToggle,
  shouldRenderLocalPreviewCanvas,
} from "../src/lib/video/cameraToggle";
import {
  buildVideoRenderStatsReport,
  MIN_VIDEO_RENDER_STATS_REPORT_WINDOW_MS,
} from "../src/lib/video/renderStats";

describe("local camera toggle state", () => {
  test("blocks the opposite toggle while camera startup is pending", () => {
    const state = createLocalCameraToggleState();

    const first = requestLocalCameraToggle(state, false);
    expect(first.command).toEqual({ enabled: true });
    expect(first.state.starting).toBe(true);

    const second = requestLocalCameraToggle(first.state, true);
    expect(second.command).toBeNull();
    expect(second.state.starting).toBe(true);
  });

  test("allows camera off after startup settles", () => {
    const state = createLocalCameraToggleState();
    const first = requestLocalCameraToggle(state, false);

    const settled = markLocalCameraToggleSettled(first.state);
    const second = requestLocalCameraToggle(settled, true);

    expect(second.command).toEqual({ enabled: false });
    expect(second.state.starting).toBe(false);
  });

  test("keeps the local preview canvas mounted while startup is pending", () => {
    expect(
      shouldRenderLocalPreviewCanvas({
        cameraEnabled: true,
        hasPreviewError: false,
      }),
    ).toBe(true);
  });
});

describe("video render stats windows", () => {
  test("skips reports until a full render window has elapsed", () => {
    const last = { received: 0, rendered: 0, dropped: 0, decodeErrors: 0 };
    const current = { received: 90, rendered: 90, dropped: 0, decodeErrors: 0 };

    expect(
      buildVideoRenderStatsReport(
        current,
        last,
        1_000,
        1_000 + MIN_VIDEO_RENDER_STATS_REPORT_WINDOW_MS - 1,
      ),
    ).toBeNull();
  });

  test("reports deltas with the measured render window duration", () => {
    const last = { received: 10, rendered: 8, dropped: 1, decodeErrors: 0 };
    const current = { received: 160, rendered: 150, dropped: 3, decodeErrors: 1 };

    expect(buildVideoRenderStatsReport(current, last, 2_000, 7_000)).toEqual({
      stats: {
        received_frames: 150,
        rendered_frames: 142,
        dropped_frames: 2,
        decode_errors: 1,
        window_seconds: 5,
      },
      snapshot: current,
      windowStartedAtMs: 7_000,
    });
  });
});
