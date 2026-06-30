// @ts-nocheck
import { describe, expect, test } from "bun:test";
import {
  DEFAULT_SCREEN_BROADCAST_PROFILE,
  buildScreenBroadcastProfile,
} from "../src/lib/screenBroadcast/profile";

describe("screen broadcast profile selection", () => {
  test("defaults to 720p15", () => {
    expect(DEFAULT_SCREEN_BROADCAST_PROFILE).toBe("720p15");
  });

  test("combines resolution and fps into the backend profile label", () => {
    expect(buildScreenBroadcastProfile("480p", 30)).toBe("480p30");
    expect(buildScreenBroadcastProfile("720p", 15)).toBe("720p15");
  });
});
