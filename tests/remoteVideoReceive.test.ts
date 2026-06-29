// @ts-nocheck
import { describe, expect, test } from "bun:test";
import {
  createRemoteVideoReceiveState,
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
});
