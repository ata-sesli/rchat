// @ts-nocheck
import { describe, expect, test } from "bun:test";
import { getAuthGateTarget, needsLocalUsername } from "../src/lib/authGate";

const connectivity = {
  mode: "reachable",
  mdns_enabled: true,
  github_sync_enabled: true,
  nat_keepalive_enabled: true,
  punch_assist_enabled: true,
};

describe("auth gate target", () => {
  test("shows setup before the vault exists", () => {
    expect(
      getAuthGateTarget({
        is_setup: false,
        is_unlocked: false,
      }),
    ).toBe("setup");
  });

  test("shows unlock for a locked existing vault", () => {
    expect(
      getAuthGateTarget({
        is_setup: true,
        is_unlocked: false,
      }),
    ).toBe("unlock");
  });

  test("allows the app phase after vault unlock even without GitHub", () => {
    const unlockedWithoutGithub = {
      is_setup: true,
      is_unlocked: true,
      is_github_connected: false,
      is_online: false,
      connectivity,
    };

    expect(getAuthGateTarget(unlockedWithoutGithub)).toBe("app");
  });

  test("requires a local username when GitHub is skipped", () => {
    const unlockedWithoutGithub = {
      is_setup: true,
      is_unlocked: true,
      is_github_connected: false,
      is_online: false,
      connectivity,
    };

    expect(needsLocalUsername(unlockedWithoutGithub, { alias: null })).toBe(
      true,
    );
    expect(needsLocalUsername(unlockedWithoutGithub, { alias: "  " })).toBe(
      true,
    );
    expect(needsLocalUsername(unlockedWithoutGithub, { alias: "Ata" })).toBe(
      false,
    );
  });

  test("does not require a local username when GitHub is connected", () => {
    const unlockedWithGithub = {
      is_setup: true,
      is_unlocked: true,
      is_github_connected: true,
      is_online: true,
      connectivity,
    };

    expect(needsLocalUsername(unlockedWithGithub, { alias: null })).toBe(false);
  });
});
