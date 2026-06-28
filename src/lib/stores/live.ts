import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { get } from "svelte/store";
import { writable } from "svelte/store";
import { getChatKind } from "$lib/chatKind";
import { isChatConnected } from "$lib/stores/presence";
import {
  api,
  type BroadcastState,
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
  diagnostics: null;
};

const idleVoiceCallState: VoiceCallState = { phase: "idle", muted: false };
const idleBroadcastState: BroadcastState = { phase: "idle", is_host: false };

const defaultLiveState: LiveState = {
  voiceCallState: idleVoiceCallState,
  broadcastState: idleBroadcastState,
  videoCallSupported: true,
  videoCallUnsupportedReason: null,
  screenBroadcastSupported: true,
  screenBroadcastUnsupportedReason: null,
  diagnostics: null,
};

export const liveState = writable<LiveState>({ ...defaultLiveState });

let initPromise: Promise<UnlistenFn> | null = null;
let activeUnlisten: UnlistenFn | null = null;
const autoRejectedUnsupportedVideoCalls = new Set<string>();
const autoRejectedUnsupportedBroadcasts = new Set<string>();

function detectVideoCallSupport(): { supported: boolean; reason: string | null } {
  if (typeof window === "undefined") {
    return { supported: false, reason: "Unavailable in this environment." };
  }
  if (!navigator?.mediaDevices?.getUserMedia) {
    return {
      supported: false,
      reason: "Camera capture is unavailable on this device.",
    };
  }
  const w = window as any;
  if (!w.VideoDecoder || !w.EncodedVideoChunk || !w.MediaStreamTrackProcessor) {
    return {
      supported: false,
      reason: "WebCodecs video decode support is unavailable on this client.",
    };
  }
  return { supported: true, reason: null };
}

function detectScreenBroadcastSupport(): {
  supported: boolean;
  reason: string | null;
} {
  const base = detectVideoCallSupport();
  if (!base.supported) return base;
  if (!navigator?.mediaDevices?.getDisplayMedia) {
    return {
      supported: false,
      reason: "Screen capture is unavailable on this client.",
    };
  }
  return { supported: true, reason: null };
}

function applySupportDetection() {
  const video = detectVideoCallSupport();
  const broadcast = detectScreenBroadcastSupport();
  liveState.update((state) => ({
    ...state,
    videoCallSupported: video.supported,
    videoCallUnsupportedReason: video.reason,
    screenBroadcastSupported: broadcast.supported,
    screenBroadcastUnsupportedReason: broadcast.reason,
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
    !state.screenBroadcastSupported
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
    applySupportDetection();
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
  const activeVoiceBroadcastCombo =
    voice.phase === "active" &&
    voice.call_kind === "voice" &&
    voice.peer_id === chatId;
  const activeVoiceSamePeer =
    voice.phase === "active" &&
    voice.call_kind === "voice" &&
    voice.peer_id === chatId;

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
  startVideoCall: (peerId: string) => api.startVideoCall(peerId),
  acceptVideoCall: (callId: string) => api.acceptVideoCall(callId),
  rejectVideoCall: (callId: string) => api.rejectVideoCall(callId),
  endVideoCall: (callId: string) => api.endVideoCall(callId),
  setVideoCallMuted: (callId: string, muted: boolean) =>
    api.setVideoCallMuted(callId, muted),
  setVideoCallCameraEnabled: (callId: string, enabled: boolean) =>
    api.setVideoCallCameraEnabled(callId, enabled),
  startScreenBroadcast: (peerId: string) => api.startScreenBroadcast(peerId),
  acceptScreenBroadcast: (sessionId: string) =>
    api.acceptScreenBroadcast(sessionId),
  rejectScreenBroadcast: (sessionId: string) =>
    api.rejectScreenBroadcast(sessionId),
  endScreenBroadcast: (sessionId: string) =>
    api.endScreenBroadcast(sessionId),
};
