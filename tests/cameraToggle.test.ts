// @ts-nocheck
import { describe, expect, test } from "bun:test";
import {
  createLocalCameraToggleState,
  markLocalCameraToggleSettled,
  requestLocalCameraToggle,
} from "../src/lib/video/cameraToggle";

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
});
