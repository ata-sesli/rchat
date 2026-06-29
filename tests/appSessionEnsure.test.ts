// @ts-nocheck
import { describe, expect, mock, test } from "bun:test";
import { get } from "svelte/store";

const lockedStatus = {
  is_setup: true,
  is_unlocked: false,
  is_github_connected: false,
  is_online: false,
  connectivity: {
    mode: "reachable",
    mdns_enabled: true,
    github_sync_enabled: true,
    nat_keepalive_enabled: true,
    punch_assist_enabled: true,
  },
};

const unlockedStatus = {
  ...lockedStatus,
  is_unlocked: true,
};

describe("app session readiness", () => {
  test("rechecks backend auth before startup when cached status is locked", async () => {
    const api = {
      checkAuthStatus: mock()
        .mockResolvedValueOnce(lockedStatus)
        .mockResolvedValueOnce(unlockedStatus),
      startNetwork: mock(async () => {}),
      getUserProfile: mock(async () => ({ alias: "Me", avatar_path: null })),
      frontendLog: mock(async () => {}),
    };

    mock.module("$lib/tauri/api", () => ({ api }));
    mock.module("$lib/stores/chat", () => ({
      initChatStore: mock(async () => () => {}),
      resetChatStore: mock(() => {}),
    }));
    mock.module("$lib/stores/live", () => ({
      initLiveStore: mock(async () => () => {}),
      resetLiveStore: mock(() => {}),
    }));
    mock.module("$lib/stores/presence", () => ({
      initPresence: mock(async () => () => {}),
      resetPresence: mock(() => {}),
    }));

    const { appSession, ensureAppReady, refreshAppSession } = await import(
      "../src/lib/stores/appSession"
    );

    await expect(refreshAppSession()).resolves.toBe(false);
    expect(get(appSession).authPhase).toBe("locked");

    await expect(ensureAppReady()).resolves.toBe(true);
    expect(api.startNetwork).toHaveBeenCalledTimes(1);
    expect(get(appSession).appReady).toBe(true);
  });
});
