// @ts-nocheck
import { describe, expect, test } from "bun:test";
import {
  createRemoteVideoReceiveQueue,
  createRemoteVideoReceiveState,
  createRemoteVideoDecoderConfigCandidates,
  enqueueRemoteVideoReceiveTask,
  markRemoteVideoDecoderFailed,
  markRemoteVideoSequenceGap,
  shouldDecodeRemoteVideoFrame,
} from "../src/lib/video/remoteReceive";

describe("remote video receive state", () => {
  test("drops delta frames until a keyframe initializes the stream", () => {
    const state = createRemoteVideoReceiveState();

    expect(shouldDecodeRemoteVideoFrame(state, "delta")).toEqual({
      decode: false,
      waitingForKeyframe: true,
    });
    expect(state.hasKeyframe).toBe(false);

    expect(shouldDecodeRemoteVideoFrame(state, "key")).toEqual({
      decode: true,
      waitingForKeyframe: false,
    });
    expect(state.hasKeyframe).toBe(true);

    expect(shouldDecodeRemoteVideoFrame(state, "delta")).toEqual({
      decode: true,
      waitingForKeyframe: false,
    });
  });

  test("waits for the next keyframe after a decoder failure", () => {
    const state = createRemoteVideoReceiveState();
    shouldDecodeRemoteVideoFrame(state, "key");

    markRemoteVideoDecoderFailed(state);

    expect(shouldDecodeRemoteVideoFrame(state, "delta")).toEqual({
      decode: false,
      waitingForKeyframe: true,
    });
    expect(shouldDecodeRemoteVideoFrame(state, "key")).toEqual({
      decode: true,
      waitingForKeyframe: false,
    });
  });

  test("waits for the next keyframe after a sequence gap", () => {
    const state = createRemoteVideoReceiveState();
    shouldDecodeRemoteVideoFrame(state, "key");

    markRemoteVideoSequenceGap(state);

    expect(shouldDecodeRemoteVideoFrame(state, "delta")).toEqual({
      decode: false,
      waitingForKeyframe: true,
    });
    expect(shouldDecodeRemoteVideoFrame(state, "key")).toEqual({
      decode: true,
      waitingForKeyframe: false,
    });
  });

  test("serializes async frame processing in arrival order", async () => {
    const queue = createRemoteVideoReceiveQueue();
    const events: string[] = [];
    let releaseFirst = () => {};
    const firstCanFinish = new Promise<void>((resolve) => {
      releaseFirst = resolve;
    });

    const first = enqueueRemoteVideoReceiveTask(queue, async () => {
      events.push("first:start");
      await firstCanFinish;
      events.push("first:end");
    });
    const second = enqueueRemoteVideoReceiveTask(queue, async () => {
      events.push("second:start");
    });

    await Promise.resolve();
    expect(events).toEqual(["first:start"]);

    releaseFirst();
    await Promise.all([first, second]);

    expect(events).toEqual(["first:start", "first:end", "second:start"]);
  });

  test("builds frame-aware VP8 decoder config candidates", () => {
    expect(createRemoteVideoDecoderConfigCandidates("vp8", 1280, 720)).toEqual([
      {
        codec: "vp8",
        codedWidth: 1280,
        codedHeight: 720,
        optimizeForLatency: true,
        hardwareAcceleration: "prefer-software",
      },
      {
        codec: "vp8",
        codedWidth: 1280,
        codedHeight: 720,
        optimizeForLatency: true,
      },
      {
        codec: "vp8",
        optimizeForLatency: true,
      },
    ]);
  });
});
