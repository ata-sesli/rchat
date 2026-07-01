// @ts-nocheck
import { describe, expect, test } from "bun:test";
import {
  clearCanvasAndResetContext,
  createCanvasContextCache,
  getCachedCanvasContext,
  isCurrentRemoteDecodeCallback,
} from "../src/lib/video/canvasContext";

function createCanvas(name: string, width = 320, height = 180) {
  const context = {
    name,
    clearCalls: [] as number[][],
    clearRect(...args: number[]) {
      this.clearCalls.push(args);
    },
  };
  return {
    width,
    height,
    getContextCalls: 0,
    context,
    getContext(kind: "2d") {
      expect(kind).toBe("2d");
      this.getContextCalls += 1;
      return context;
    },
  };
}

describe("canvas context cache", () => {
  test("reuses context while the canvas element is unchanged", () => {
    const cache = createCanvasContextCache();
    const canvas = createCanvas("a");

    expect(getCachedCanvasContext(cache, canvas)).toBe(canvas.context);
    expect(getCachedCanvasContext(cache, canvas)).toBe(canvas.context);
    expect(canvas.getContextCalls).toBe(1);
  });

  test("reacquires context when the canvas element changes", () => {
    const cache = createCanvasContextCache();
    const canvasA = createCanvas("a");
    const canvasB = createCanvas("b");

    expect(getCachedCanvasContext(cache, canvasA)).toBe(canvasA.context);
    expect(getCachedCanvasContext(cache, canvasB)).toBe(canvasB.context);

    expect(canvasA.getContextCalls).toBe(1);
    expect(canvasB.getContextCalls).toBe(1);
  });

  test("reset clears the visible canvas and invalidates the cached context", () => {
    const cache = createCanvasContextCache();
    const canvas = createCanvas("a", 640, 360);

    expect(getCachedCanvasContext(cache, canvas)).toBe(canvas.context);
    clearCanvasAndResetContext(cache, canvas);
    expect(canvas.context.clearCalls).toEqual([[0, 0, 640, 360]]);

    expect(getCachedCanvasContext(cache, canvas)).toBe(canvas.context);
    expect(canvas.getContextCalls).toBe(2);
  });

  test("reset clears the visible canvas even when the cached context is stale", () => {
    const cache = createCanvasContextCache();
    const detachedCanvas = createCanvas("detached");
    const visibleCanvas = createCanvas("visible", 800, 450);

    expect(getCachedCanvasContext(cache, detachedCanvas)).toBe(detachedCanvas.context);
    clearCanvasAndResetContext(cache, visibleCanvas);

    expect(detachedCanvas.context.clearCalls).toEqual([]);
    expect(visibleCanvas.context.clearCalls).toEqual([[0, 0, 800, 450]]);
  });
});

describe("remote decode callback generation", () => {
  test("accepts only callbacks for the current generation and session", () => {
    expect(isCurrentRemoteDecodeCallback(2, 2, "session-a", "session-a")).toBe(true);
    expect(isCurrentRemoteDecodeCallback(1, 2, "session-a", "session-a")).toBe(false);
    expect(isCurrentRemoteDecodeCallback(2, 2, "session-a", "session-b")).toBe(false);
    expect(isCurrentRemoteDecodeCallback(2, 2, null, "session-a")).toBe(false);
  });
});
