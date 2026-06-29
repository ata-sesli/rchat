// @ts-nocheck
import { describe, expect, test } from "bun:test";
import {
  ensureVideoCallCameraAccess,
  videoCallCameraConstraints,
} from "../src/lib/media/callPermissions";

describe("video call camera permissions", () => {
  test("requests camera-only 720p30-friendly constraints", () => {
    expect(videoCallCameraConstraints()).toEqual({
      audio: false,
      video: {
        width: { ideal: 1280 },
        height: { ideal: 720 },
        frameRate: { ideal: 30, max: 30 },
      },
    });
  });

  test("stops warm-up tracks after permission succeeds", async () => {
    const stopped: string[] = [];
    const calls: unknown[] = [];

    await ensureVideoCallCameraAccess(async (constraints) => {
      calls.push(constraints);
      return {
        getTracks: () => [
          { stop: () => stopped.push("camera") },
        ],
      };
    });

    expect(calls).toEqual([videoCallCameraConstraints()]);
    expect(stopped).toEqual(["camera"]);
  });

  test("turns denied permission into a readable error", async () => {
    await expect(
      ensureVideoCallCameraAccess(async () => {
        throw { name: "NotAllowedError" };
      }),
    ).rejects.toThrow("permission");
  });
});
