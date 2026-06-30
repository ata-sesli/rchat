import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { get } from "svelte/store";
import { writable } from "svelte/store";
import { getChatKind } from "$lib/chatKind";
import { screenBroadcastCapabilitiesFromChecks } from "$lib/stores/liveSupport";
import { isChatConnected, presencePeerKey } from "$lib/stores/presence";
import {
  api,
  type BroadcastState,
  type ScreenBroadcastProfile,
  type VoiceCallState,
} from "$lib/tauri/api";

export type CallAvailability = {
  canStartVoiceCall: boolean;
  canStartVideoCall: boolean;
  canStartScreenBroadcast: boolean;
};

export type LiveState = {
  voiceCallState: VoiceCallState;
  broadcastState: BroadcastState;
  videoCallSupported: boolean;
  videoCallUnsupportedReason: string | null;
  screenBroadcastSupported: boolean;
  screenBroadcastUnsupportedReason: string | null;
  screenBroadcastViewerSupported: boolean;
  screenBroadcastViewerUnsupportedReason: string | null;
  diagnostics: null;
};

const idleVoiceCallState: VoiceCallState = { phase: "idle", muted: false };
const idleBroadcastState: BroadcastState = { phase: "idle", is_host: false };

const defaultLiveState: LiveState = {
  voiceCallState: idleVoiceCallState,
  broadcastState: idleBroadcastState,
  videoCallSupported: false,
  videoCallUnsupportedReason: "Checking native camera support.",
  screenBroadcastSupported: false,
  screenBroadcastUnsupportedReason: "Checking native screen capture support.",
  screenBroadcastViewerSupported: false,
  screenBroadcastViewerUnsupportedReason: "Checking screen share viewer support.",
  diagnostics: null,
};

export const liveState = writable<LiveState>({ ...defaultLiveState });

let initPromise: Promise<UnlistenFn> | null = null;
let activeUnlisten: UnlistenFn | null = null;
const autoRejectedUnsupportedVideoCalls = new Set<string>();
const autoRejectedUnsupportedBroadcasts = new Set<string>();

function logLive(message: string, data?: Record<string, unknown>) {
  const details = data ? ` ${JSON.stringify(data)}` : "";
  const line = `[Live] ${message}${details}`;
  console.log(line);
  void api.frontendLog(line).catch(() => {});
}

async function detectVideoCallSupport(): Promise<{
  supported: boolean;
  reason: string | null;
}> {
  if (typeof window === "undefined") {
    return { supported: false, reason: "Unavailable in this environment." };
  }
  const w = window as any;
  if (!w.VideoDecoder || !w.EncodedVideoChunk) {
    return {
      supported: false,
      reason: "WebCodecs video decode is unavailable on this client.",
    };
  }
  try {
    const support = await api.getVideoCaptureSupport();
    if (!support.supported) {
      return {
        supported: false,
        reason: support.reason || "Native camera capture is unavailable.",
      };
    }
    if (support.devices.length === 0) {
      return {
        supported: false,
        reason: "No camera device was found.",
      };
    }
    return { supported: true, reason: null };
  } catch (e) {
    return {
      supported: false,
      reason: e instanceof Error ? e.message : "Native camera capture check failed.",
    };
  }
}

async function detectScreenBroadcastSupport(): Promise<{
  hostSupported: boolean;
  hostReason: string | null;
  viewerSupported: boolean;
  viewerReason: string | null;
}> {
  if (typeof window === "undefined") {
    return screenBroadcastCapabilitiesFromChecks({
      decodeSupported: false,
      decodeReason: "Unavailable in this environment.",
      captureSupported: false,
      captureReason: "Unavailable in this environment.",
    });
  }
  const w = window as any;
  if (!w.VideoDecoder || !w.EncodedVideoChunk) {
    return screenBroadcastCapabilitiesFromChecks({
      decodeSupported: false,
      decodeReason: "WebCodecs video decode is unavailable on this client.",
      captureSupported: false,
      captureReason: null,
    });
  }
  try {
    const support = await api.getScreenCaptureSupport();
    return screenBroadcastCapabilitiesFromChecks({
      decodeSupported: true,
      decodeReason: null,
      captureSupported: support.supported,
      captureReason: support.supported
        ? null
        : support.reason || "Native screen capture is unavailable.",
    });
  } catch (e) {
    return screenBroadcastCapabilitiesFromChecks({
      decodeSupported: true,
      decodeReason: null,
      captureSupported: false,
      captureReason:
        e instanceof Error ? e.message : "Native screen capture support check failed.",
    });
  }
}

async function applySupportDetection() {
  const video = await detectVideoCallSupport();
  const broadcast = await detectScreenBroadcastSupport();
  liveState.update((state) => ({
    ...state,
    videoCallSupported: video.supported,
    videoCallUnsupportedReason: video.reason,
    screenBroadcastSupported: broadcast.hostSupported,
    screenBroadcastUnsupportedReason: broadcast.hostReason,
    screenBroadcastViewerSupported: broadcast.viewerSupported,
    screenBroadcastViewerUnsupportedReason: broadcast.viewerReason,
  }));
}

function maybeAutoRejectUnsupportedCalls() {
  const state = get(liveState);
  const voice = state.voiceCallState;
  const broadcast = state.broadcastState;
  const incomingUnsupportedVideoCallId =
    voice.phase === "incoming_ringing" &&
    voice.call_kind === "video" &&
    voice.call_id &&
    !state.videoCallSupported
      ? voice.call_id
      : null;

  if (
    incomingUnsupportedVideoCallId &&
    !autoRejectedUnsupportedVideoCalls.has(incomingUnsupportedVideoCallId)
  ) {
    autoRejectedUnsupportedVideoCalls.add(incomingUnsupportedVideoCallId);
    void api.rejectVideoCall(incomingUnsupportedVideoCallId).catch((e) => {
      console.error("Failed to auto-reject unsupported incoming video call:", e);
    });
  }
  if (voice.phase === "idle" && autoRejectedUnsupportedVideoCalls.size > 32) {
    autoRejectedUnsupportedVideoCalls.clear();
  }

  const incomingUnsupportedBroadcastId =
    broadcast.phase === "incoming_ringing" &&
    broadcast.session_id &&
    !state.screenBroadcastViewerSupported
      ? broadcast.session_id
      : null;

  if (
    incomingUnsupportedBroadcastId &&
    !autoRejectedUnsupportedBroadcasts.has(incomingUnsupportedBroadcastId)
  ) {
    autoRejectedUnsupportedBroadcasts.add(incomingUnsupportedBroadcastId);
    void api.rejectScreenBroadcast(incomingUnsupportedBroadcastId).catch((e) => {
      console.error("Failed to auto-reject unsupported incoming screen share:", e);
    });
  }
  if (broadcast.phase === "idle" && autoRejectedUnsupportedBroadcasts.size > 32) {
    autoRejectedUnsupportedBroadcasts.clear();
  }
}

export async function initLiveStore(): Promise<UnlistenFn> {
  if (activeUnlisten) return activeUnlisten;
  if (initPromise) return initPromise;

  initPromise = (async () => {
    await applySupportDetection();
    try {
      const [voiceCallState, broadcastState] = await Promise.all([
        api.getVoiceCallState(),
        api.getBroadcastState(),
      ]);
      liveState.update((state) => ({ ...state, voiceCallState, broadcastState }));
      maybeAutoRejectUnsupportedCalls();
    } catch (e) {
      console.warn("Live call state unavailable yet:", e);
    }

    const cleanups: UnlistenFn[] = [];
    cleanups.push(
      await listen<VoiceCallState>("voice-call-state-updated", (event) => {
        liveState.update((state) => ({
          ...state,
          voiceCallState: event.payload,
        }));
        maybeAutoRejectUnsupportedCalls();
      }),
    );
    cleanups.push(
      await listen<BroadcastState>("broadcast-state-updated", (event) => {
        liveState.update((state) => ({
          ...state,
          broadcastState: event.payload,
        }));
        maybeAutoRejectUnsupportedCalls();
      }),
    );

    activeUnlisten = () => {
      while (cleanups.length > 0) {
        cleanups.pop()?.();
      }
      activeUnlisten = null;
      initPromise = null;
    };
    return activeUnlisten;
  })().catch((e) => {
    initPromise = null;
    throw e;
  });

  return initPromise;
}

export function resetLiveStore() {
  if (activeUnlisten) {
    activeUnlisten();
  }
  autoRejectedUnsupportedVideoCalls.clear();
  autoRejectedUnsupportedBroadcasts.clear();
  liveState.set({ ...defaultLiveState });
  activeUnlisten = null;
  initPromise = null;
}

export function callAvailabilityFor(
  chatId: string,
  connectedChatIds: Set<string>,
  live: LiveState = get(liveState),
): CallAvailability {
  const kind = getChatKind(chatId);
  const isConnected = isChatConnected(chatId, connectedChatIds);
  const voice = live.voiceCallState;
  const broadcast = live.broadcastState;
  const sameVoicePeer =
    !!voice.peer_id && presencePeerKey(voice.peer_id) === presencePeerKey(chatId);
  const activeVoiceSamePeer =
    voice.phase === "active" &&
    voice.call_kind === "voice" &&
    sameVoicePeer;
  const activeVoiceBroadcastCombo =
    voice.phase === "active" &&
    voice.call_kind === "voice" &&
    sameVoicePeer;

  return {
    canStartVoiceCall:
      kind === "dm" && isConnected && voice.phase === "idle",
    canStartVideoCall:
      kind === "dm" &&
      isConnected &&
      (voice.phase === "idle" || activeVoiceSamePeer) &&
      broadcast.phase === "idle" &&
      live.videoCallSupported,
    canStartScreenBroadcast:
      kind === "dm" &&
      isConnected &&
      broadcast.phase === "idle" &&
      live.screenBroadcastSupported &&
      (voice.phase === "idle" || activeVoiceBroadcastCombo),
  };
}

export const liveActions = {
  startVoiceCall: (peerId: string) => api.startVoiceCall(peerId),
  acceptVoiceCall: (callId: string) => api.acceptVoiceCall(callId),
  rejectVoiceCall: (callId: string) => api.rejectVoiceCall(callId),
  endVoiceCall: (callId: string) => api.endVoiceCall(callId),
  setVoiceCallMuted: (callId: string, muted: boolean) =>
    api.setVoiceCallMuted(callId, muted),
  startVideoCall: async (peerId: string) => {
    logLive("native video call requested", { peer_id: peerId });
    return api.startVideoCall(peerId);
  },
  acceptVideoCall: (callId: string) => api.acceptVideoCall(callId),
  rejectVideoCall: (callId: string) => api.rejectVideoCall(callId),
  endVideoCall: (callId: string) => api.endVideoCall(callId),
  setVideoCallMuted: (callId: string, muted: boolean) =>
    api.setVideoCallMuted(callId, muted),
  setVideoCallCameraEnabled: async (callId: string, enabled: boolean) => {
    logLive("native video camera toggle requested", { call_id: callId, enabled });
    return api.setVideoCallCameraEnabled(callId, enabled);
  },
  startScreenBroadcast: (peerId: string, profile: ScreenBroadcastProfile) =>
    api.startScreenBroadcast(peerId, profile),
  acceptScreenBroadcast: (sessionId: string) =>
    api.acceptScreenBroadcast(sessionId),
  rejectScreenBroadcast: (sessionId: string) =>
    api.rejectScreenBroadcast(sessionId),
  endScreenBroadcast: (sessionId: string) =>
    api.endScreenBroadcast(sessionId),
};
