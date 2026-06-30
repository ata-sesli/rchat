// @ts-nocheck
import { describe, expect, mock, test } from "bun:test";
import { readFileSync } from "node:fs";
import { get } from "svelte/store";

function deferred() {
  let resolve;
  let reject;
  const promise = new Promise((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

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

describe("app session startup", () => {
  test("ignores stale locked refresh results after a newer unlock refresh starts", async () => {
    const checks = [];
    const api = {
      checkAuthStatus: mock(() => {
        const next = deferred();
        checks.push(next);
        return next.promise;
      }),
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

    const { appSession, refreshAppSession } = await import(
      "../src/lib/stores/appSession"
    );

    const staleLockedRefresh = refreshAppSession();
    const unlockRefresh = refreshAppSession();

    checks[1].resolve(unlockedStatus);
    await expect(unlockRefresh).resolves.toBe(true);
    expect(get(appSession).appReady).toBe(true);

    checks[0].resolve(lockedStatus);
    await expect(staleLockedRefresh).resolves.toBe(true);
    expect(get(appSession).authPhase).toBe("unlocked");
    expect(get(appSession).appReady).toBe(true);
  });
});

describe("layout stylesheet boundaries", () => {
  test("keeps global layout CSS out of the Svelte layout style query", () => {
    const layout = readFileSync("src/routes/+layout.svelte", "utf8");
    const appCss = readFileSync("src/app.css", "utf8");

    expect(layout).not.toContain("<style>");
    expect(appCss).toContain("body.is-dragging");
  });
});
