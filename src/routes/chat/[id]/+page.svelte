<script lang="ts">
  import { page } from "$app/stores";
  import ChatArea from "../../../components/chat/ChatArea.svelte";
  import {
    addSentAudioMessage,
    addSentDocumentMessage,
    addSentImageMessage,
    addSentStickerMessage,
    addSentVideoMessage,
    appSession,
    callAvailabilityFor,
    chatState,
    connectedChatIds,
    liveActions,
    liveState,
    sendActiveChatMessage,
    setActiveChat,
  } from "$lib/stores";
  import type { ScreenBroadcastProfile } from "$lib/screenBroadcast/profile";

  let messageInput = "";
  let showAttachments = false;

  $: activePeer = $page.params.id || "";
  $: void setActiveChat(activePeer);
  $: callAvailability = callAvailabilityFor(
    activePeer,
    $connectedChatIds,
    $liveState,
  );
</script>

<div class="h-full flex flex-col bg-theme-base-950">
  <ChatArea
    {activePeer}
    peerAlias={$chatState.peerAlias}
    messages={$chatState.messages}
    userProfile={$appSession.userProfile}
    voiceCallState={$liveState.voiceCallState}
    broadcastState={$liveState.broadcastState}
    canStartVoiceCall={callAvailability.canStartVoiceCall}
    canStartVideoCall={callAvailability.canStartVideoCall}
    canStartScreenBroadcast={callAvailability.canStartScreenBroadcast}
    videoCallSupported={$liveState.videoCallSupported}
    videoCallUnsupportedReason={$liveState.videoCallUnsupportedReason}
    screenBroadcastSupported={$liveState.screenBroadcastSupported}
    screenBroadcastUnsupportedReason={$liveState.screenBroadcastUnsupportedReason}
    screenBroadcastViewerSupported={$liveState.screenBroadcastViewerSupported}
    screenBroadcastViewerUnsupportedReason={$liveState.screenBroadcastViewerUnsupportedReason}
    onStartVoiceCall={async () => {
      try {
        await liveActions.startVoiceCall(activePeer);
      } catch (e) {
        console.error("Failed to start voice call:", e);
      }
    }}
    onStartVideoCall={async () => {
      try {
        await liveActions.startVideoCall(activePeer);
      } catch (e) {
        console.error("Failed to start video call:", e);
      }
    }}
    onStartScreenBroadcast={async (profile: ScreenBroadcastProfile) => {
      try {
        await liveActions.startScreenBroadcast(activePeer, profile);
      } catch (e) {
        console.error("Failed to start screen broadcast:", e);
      }
    }}
    onEndVoiceCall={async (callId) => {
      try {
        await liveActions.endVoiceCall(callId);
      } catch (e) {
        console.error("Failed to end voice call:", e);
      }
    }}
    onEndVideoCall={async (callId) => {
      try {
        await liveActions.endVideoCall(callId);
      } catch (e) {
        console.error("Failed to end video call:", e);
      }
    }}
    onEndScreenBroadcast={async (sessionId) => {
      try {
        await liveActions.endScreenBroadcast(sessionId);
      } catch (e) {
        console.error("Failed to end screen broadcast:", e);
      }
    }}
    onToggleVoiceMute={async (callId, muted) => {
      try {
        await liveActions.setVoiceCallMuted(callId, muted);
      } catch (e) {
        console.error("Failed to toggle mute:", e);
      }
    }}
    onToggleVideoMute={async (callId, muted) => {
      try {
        await liveActions.setVideoCallMuted(callId, muted);
      } catch (e) {
        console.error("Failed to toggle video-call mute:", e);
      }
    }}
    onToggleVideoCamera={async (callId, enabled) => {
      try {
        await liveActions.setVideoCallCameraEnabled(callId, enabled);
      } catch (e) {
        console.error("Failed to toggle camera:", e);
      }
    }}
    bind:message={messageInput}
    bind:showAttachments
    onsend={sendActiveChatMessage}
    onImageSent={addSentImageMessage}
    onDocumentSent={addSentDocumentMessage}
    onVideoSent={addSentVideoMessage}
    onAudioSent={addSentAudioMessage}
    onStickerSent={addSentStickerMessage}
  />
</div>
