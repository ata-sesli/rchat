import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { get, writable } from "svelte/store";
import {
  api,
  type AuthStatus,
  type ConnectivityMode,
  type ConnectivitySettings,
  type UserProfile,
} from "$lib/tauri/api";
import { initChatStore, resetChatStore } from "$lib/stores/chat";
import { initLiveStore, resetLiveStore } from "$lib/stores/live";
import { initPresence, resetPresence } from "$lib/stores/presence";

export type AuthPhase = "checking" | "locked" | "unlocked" | "error";

export type AppSessionState = {
  authPhase: AuthPhase;
  authChecked: boolean;
  appReady: boolean;
  authStatus: AuthStatus | null;
  userProfile: UserProfile;
  connectivitySettings: ConnectivitySettings;
  startupError: string | null;
};

const defaultConnectivitySettings: ConnectivitySettings = {
  mode: "reachable",
  mdns_enabled: true,
  github_sync_enabled: true,
  nat_keepalive_enabled: true,
  punch_assist_enabled: true,
};

const defaultUserProfile: UserProfile = {
  alias: "Me",
  avatar_path: null,
};

const defaultAppSessionState: AppSessionState = {
  authPhase: "checking",
  authChecked: false,
  appReady: false,
  authStatus: null,
  userProfile: defaultUserProfile,
  connectivitySettings: defaultConnectivitySettings,
  startupError: null,
};

export const appSession = writable<AppSessionState>({
  ...defaultAppSessionState,
});

let initPromise: Promise<UnlistenFn> | null = null;
let activeUnlisten: UnlistenFn | null = null;
let protectedCleanups: UnlistenFn[] = [];
let appReadyPromise: Promise<boolean> | null = null;

export function resetProtectedStores() {
  while (protectedCleanups.length > 0) {
    protectedCleanups.pop()?.();
  }
  resetPresence();
  resetChatStore();
  resetLiveStore();
  appReadyPromise = null;
  appSession.update((state) => ({
    ...state,
    appReady: false,
  }));
}

function protectedStoresStarted(): boolean {
  return protectedCleanups.length > 0;
}

async function startProtectedStores() {
  if (protectedStoresStarted()) return;
  const cleanups = await Promise.all([
    initPresence(),
    initChatStore(),
    initLiveStore(),
  ]);
  protectedCleanups = cleanups;
}

export async function refreshUserProfile(): Promise<UserProfile> {
  const userProfile = await api.getUserProfile();
  appSession.update((state) => ({ ...state, userProfile }));
  return userProfile;
}

export async function refreshAppSession(): Promise<boolean> {
  try {
    const authStatus = await api.checkAuthStatus();
    const unlocked = authStatus.is_setup && authStatus.is_unlocked;

    appSession.update((state) => ({
      ...state,
      authPhase: unlocked ? "unlocked" : "locked",
      authChecked: true,
      appReady: unlocked ? state.appReady : false,
      authStatus,
      connectivitySettings: authStatus.connectivity,
      startupError: null,
    }));

    if (!unlocked) {
      resetProtectedStores();
      return false;
    }

    return ensureAppReady();
  } catch (e) {
    console.error("App session refresh failed:", e);
    resetProtectedStores();
    appSession.update((state) => ({
      ...state,
      authPhase: "error",
      authChecked: true,
      appReady: false,
      startupError: e instanceof Error ? e.message : String(e),
    }));
    return false;
  }
}

export async function ensureAppReady(): Promise<boolean> {
  if (appReadyPromise) return appReadyPromise;
  if (get(appSession).appReady) return true;

  appReadyPromise = (async () => {
    try {
      const authStatus = get(appSession).authStatus ?? (await api.checkAuthStatus());
      const unlocked = authStatus.is_setup && authStatus.is_unlocked;
      appSession.update((state) => ({
        ...state,
        authPhase: unlocked ? "unlocked" : "locked",
        authChecked: true,
        authStatus,
        connectivitySettings: authStatus.connectivity,
        appReady: unlocked ? state.appReady : false,
        startupError: null,
      }));

      if (!unlocked) {
        resetProtectedStores();
        return false;
      }

      await api.startNetwork();
      const userProfile = await api.getUserProfile();
      await startProtectedStores();

      appSession.update((state) => ({
        ...state,
        authPhase: "unlocked",
        authChecked: true,
        appReady: true,
        authStatus,
        userProfile,
        connectivitySettings: authStatus.connectivity,
        startupError: null,
      }));
      return true;
    } catch (e) {
      console.error("App startup failed:", e);
      resetProtectedStores();
      appSession.update((state) => ({
        ...state,
        authPhase: "error",
        authChecked: true,
        appReady: false,
        startupError: e instanceof Error ? e.message : String(e),
      }));
      return false;
    } finally {
      appReadyPromise = null;
    }
  })();

  return appReadyPromise;
}

export async function setConnectivityMode(
  mode: ConnectivityMode,
): Promise<ConnectivitySettings> {
  const connectivitySettings = await api.setConnectivityMode(mode);
  appSession.update((state) => ({
    ...state,
    connectivitySettings,
    authStatus: state.authStatus
      ? { ...state.authStatus, connectivity: connectivitySettings }
      : state.authStatus,
  }));
  return connectivitySettings;
}

export function applyConnectivitySettings(
  connectivitySettings: ConnectivitySettings,
) {
  appSession.update((state) => ({
    ...state,
    connectivitySettings,
    authStatus: state.authStatus
      ? { ...state.authStatus, connectivity: connectivitySettings }
      : state.authStatus,
  }));
}

export async function initAppSession(): Promise<UnlistenFn> {
  if (activeUnlisten) return activeUnlisten;
  if (initPromise) return initPromise;

  initPromise = (async () => {
    const authUnlisten = await listen("auth-status", () => {
      void refreshAppSession();
    });

    activeUnlisten = () => {
      authUnlisten();
      resetProtectedStores();
      appSession.set({ ...defaultAppSessionState });
      activeUnlisten = null;
      initPromise = null;
    };

    void refreshAppSession();
    return activeUnlisten;
  })().catch((e) => {
    initPromise = null;
    throw e;
  });

  return initPromise;
}
