// @ts-nocheck
import { describe, expect, test } from "bun:test";
import { screenBroadcastCapabilitiesFromChecks } from "../src/lib/stores/liveSupport";

describe("live support capabilities", () => {
  test("screen broadcast viewing only requires video decode support", async () => {
    const capabilities = screenBroadcastCapabilitiesFromChecks({
      decodeSupported: true,
      decodeReason: null,
      captureSupported: false,
      captureReason: "Native screen capture is unavailable.",
    });

    expect(capabilities.viewerSupported).toBe(true);
    expect(capabilities.viewerReason).toBeNull();
    expect(capabilities.hostSupported).toBe(false);
    expect(capabilities.hostReason).toContain("Native screen capture");
  });

  test("screen broadcast hosting and viewing both require video decode support", async () => {
    const capabilities = screenBroadcastCapabilitiesFromChecks({
      decodeSupported: false,
      decodeReason: "WebCodecs video decode is unavailable.",
      captureSupported: true,
      captureReason: null,
    });

    expect(capabilities.viewerSupported).toBe(false);
    expect(capabilities.viewerReason).toContain("WebCodecs");
    expect(capabilities.hostSupported).toBe(false);
    expect(capabilities.hostReason).toContain("WebCodecs");
  });
});
