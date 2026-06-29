// @ts-nocheck
import { describe, expect, test } from "bun:test";
import {
  describeMediaError,
  formatAudioLevel,
  mediaSupportRows,
} from "../src/lib/media/testDiagnostics";

describe("media test diagnostics", () => {
  test("formats audio levels as percentages", () => {
    expect(formatAudioLevel(0)).toBe("0%");
    expect(formatAudioLevel(0.42)).toBe("42%");
    expect(formatAudioLevel(2)).toBe("100%");
  });

  test("turns browser media errors into useful messages", () => {
    expect(describeMediaError({ name: "NotAllowedError" })).toContain(
      "permission",
    );
    expect(describeMediaError({ name: "NotFoundError" })).toContain(
      "device",
    );
    expect(describeMediaError(new Error("boom"))).toBe("boom");
  });

  test("reports media API support without throwing", () => {
    const rows = mediaSupportRows({
      mediaDevices: true,
      getUserMedia: false,
      mediaRecorder: true,
    });

    expect(rows).toEqual([
      { label: "navigator.mediaDevices", ok: true },
      { label: "getUserMedia", ok: false },
      { label: "MediaRecorder", ok: true },
    ]);
  });
});
