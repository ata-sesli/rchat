<script lang="ts">
  import { page } from "$app/stores";
  import { onMount, onDestroy } from "svelte";
  import { listen } from "@tauri-apps/api/event";
  import ChatArea from "../../../components/chat/ChatArea.svelte";
  import { api, type DbMessage, type VoiceCallState } from "$lib/tauri/api";
  import { getChatKind } from "$lib/chatKind";
  import { extractPeerIdFromChatId } from "$lib/chatIdentity";

  // Types
  type Message = {
    id?: string;
    sender: string;
    text: string;
    timestamp: Date;
    status?: string;
    content_type?: string;
    file_hash?: string | null;
  };

  // Props/State
  let activePeer: string = "";
  $: activePeer = $page.params.id || "";
  $: activeChatKind = getChatKind(activePeer);
  $: outgoingStatus = activeChatKind === "dm" || activeChatKind === "tempdm" ? "pending" : "delivered";

  let messages: Message[] = [];
  let peerAlias: string | null = null;
  let userProfile = {
    alias: "Me" as string | null,
    avatar_path: null as string | null,
  };
  let messageInput = "";
  let showAttachments = false;
  let unlisten: () => void;
  let unlistenStatus: () => void;
  let unlistenVoiceCall: () => void;
  let unlistenConnectedChatIds: () => void;
  let voiceCallState: VoiceCallState = { phase: "idle", muted: false };
  let connectedChatIds = new Set<string>();
  let activeConversationIds = new Set<string>();

  function peerKey(chatId: string): string {
    const normalized = chatId === "self" ? "Me" : chatId;
    return extractPeerIdFromChatId(normalized) || normalized;
  }

  function isChatConnected(chatId: string): boolean {
    const target = peerKey(chatId);
    for (const connectedId of connectedChatIds) {
      if (peerKey(connectedId) === target) return true;
    }
    return false;
  }

  // Cache for status updates that arrive before we've swapped tempId → msgId
  let pendingStatusCache: Record<string, string> = {};

  // Reactive loading
  $: loadChatHistory(activePeer);

  async function loadChatHistory(peerId: string) {
    if (!peerId) return;
    activeConversationIds = new Set([peerId]);
    try {
      // Map "Me" in URL to "self" for DB
      const chatId = peerId === "Me" ? "self" : peerId;
      const history = await api.getChatHistory(chatId);
      const canonicalChatId = history[0]?.chat_id || chatId;
      const canonicalUiPeer = canonicalChatId === "self" ? "Me" : canonicalChatId;
      activeConversationIds = new Set([peerId, canonicalUiPeer]);

      messages = history.map((m) => ({
        id: m.id,
        sender: m.peer_id === "Me" ? "Me" : m.sender_alias || m.peer_id,
        text: m.text_content || "",
        timestamp: new Date(m.timestamp * 1000),
        status: m.status || "delivered",
        content_type: m.content_type,
        file_hash: m.file_hash,
      }));

      if (peerId !== "Me") {
        const chatList = await api.getChatList();
        const chatRow = chatList.find((c) => (c.id === "self" ? "Me" : c.id) === peerId);
        if (chatRow?.name) {
          peerAlias = chatRow.name;
        } else {
          const aliases = await api.getPeerAliases();
          peerAlias = aliases[peerId] || null;
        }
      } else {
        peerAlias = null;
      }

    } catch (e) {
      console.error("Failed to load history for", peerId, e);
      messages = [];
    }
  }

  onMount(async () => {
    // Fetch user profile locally for this page
    try {
      userProfile = await api.getUserProfile();
    } catch (e) {
      console.error("Failed to fetch profile", e);
    }

    // Listen for incoming messages
    unlisten = await listen<DbMessage>("message-received", (event) => {
      const msg = event.payload;
      // Check if this message belongs to the current chat
      // Incoming messages have chat_id = sender_id
      // Outgoing messages have chat_id = recipient_id
      // We only care if msg.chat_id matches our activePeer (or if it's "self" and activePeer is "Me")

      let relatedPeer = msg.chat_id;
      if (relatedPeer === "self") relatedPeer = "Me";
      const relatedPeerKey = peerKey(relatedPeer);

      if (activeConversationIds.has(relatedPeer) || peerKey(activePeer) === relatedPeerKey) {
        const newMsg: Message = {
          id: msg.id,
          sender: msg.peer_id === "Me" ? "Me" : msg.sender_alias || msg.peer_id,
          text: msg.text_content || "",
          timestamp: new Date(msg.timestamp * 1000),
          status: msg.status || "delivered",
          content_type: msg.content_type || "text",
          file_hash: msg.file_hash,
        };
        messages = [...messages, newMsg];

        // Send read receipt since we're actively viewing this chat
        if (msg.peer_id !== "Me") {
          api.markMessagesRead(msg.chat_id).catch((e) => {
            console.error("Failed to send read receipt:", e);
          });
        }
      }
    });

    try {
      voiceCallState = await api.getVoiceCallState();
      connectedChatIds = new Set(await api.getConnectedChatIds());
    } catch (e) {
      console.warn("Voice call state unavailable:", e);
    }

    unlistenVoiceCall = await listen<VoiceCallState>("voice-call-state-updated", (event) => {
      voiceCallState = event.payload;
    });
    unlistenConnectedChatIds = await listen("connected-chat-ids-updated", (event: any) => {
      const ids = Array.isArray(event.payload) ? event.payload : [];
      connectedChatIds = new Set(
        ids.map((id: string) => (id === "self" ? "Me" : id)),
      );
    });

    // Listen for message status updates (e.g., delivered -> read)
    unlistenStatus = await listen("message-status-updated", (event: any) => {
      const { msg_id, status } = event.payload;
      console.log("[Chat] Message status update:", msg_id, "->", status);

      // Check if message exists in our list
      const msgExists = messages.some((m) => m.id === msg_id);

      if (msgExists) {
        // Update the message in the list
        messages = messages.map((m) =>
          m.id === msg_id ? { ...m, status } : m
        );
      } else {
        // Cache for later - message id swap might not have happened yet
        pendingStatusCache[msg_id] = status;
        console.log("[Chat] Cached status for", msg_id, "->", status);
      }
    });
  });

  onDestroy(() => {
    if (unlisten) unlisten();
    if (unlistenStatus) unlistenStatus();
    if (unlistenVoiceCall) unlistenVoiceCall();
    if (unlistenConnectedChatIds) unlistenConnectedChatIds();
  });

  async function handleSendMessage(text: string) {
    if (!text.trim()) return;
    if (activeChatKind === "archived") return;

    // Generate a temporary id for tracking
    const tempId = `temp-${Date.now()}-${Math.random().toString(36).slice(2)}`;

    // Optimistic update with temp id
    const tempMsg: Message = {
      id: tempId,
      sender: "Me",
      text,
      timestamp: new Date(),
      status: outgoingStatus,
    };
    messages = [...messages, tempMsg];

    try {
      if (activePeer === "Me") {
        await api.sendMessageToSelf(text);
        // Self messages are always "read"
        messages = messages.map((m) =>
          m.id === tempId ? { ...m, status: "read" } : m
        );
      } else {
        // Get the msg_id from backend
        const msgId = await api.sendMessage(activePeer, text);

        // Check if we have a cached status update for this msg_id (race condition fix)
        const cachedStatus = pendingStatusCache[msgId];
        if (cachedStatus) {
          console.log(
            "[Chat] Applying cached status for",
            msgId,
            "->",
            cachedStatus
          );
          delete pendingStatusCache[msgId];
        }

        // Replace temp id with real id and apply cached status if any
        messages = messages.map((m) =>
          m.id === tempId
            ? {
                ...m,
                id: msgId,
                status:
                  cachedStatus ||
                  (activeChatKind === "dm" || activeChatKind === "tempdm" ? m.status : "delivered"),
              }
            : m
        );
      }
    } catch (e) {
      console.error("Send failed:", e);
      // Mark message as failed
      messages = messages.map((m) =>
        m.id === tempId ? { ...m, status: "failed" } : m
      );
    }
  }
</script>

<div class="h-full flex flex-col bg-theme-base-950">
  <ChatArea
    {activePeer}
    {peerAlias}
    {messages}
    {userProfile}
    voiceCallState={voiceCallState}
    canStartVoiceCall={activeChatKind === "dm" && isChatConnected(activePeer) && voiceCallState.phase === "idle"}
    canStartVideoCall={activeChatKind === "dm" && isChatConnected(activePeer) && voiceCallState.phase === "idle"}
    onStartVoiceCall={async () => {
      try {
        await api.startVoiceCall(activePeer);
      } catch (e) {
        console.error("Failed to start voice call:", e);
      }
    }}
    onStartVideoCall={async () => {
      try {
        await api.startVideoCall(activePeer);
      } catch (e) {
        console.error("Failed to start video call:", e);
      }
    }}
    onEndVoiceCall={async (callId) => {
      try {
        await api.endVoiceCall(callId);
      } catch (e) {
        console.error("Failed to end voice call:", e);
      }
    }}
    onEndVideoCall={async (callId) => {
      try {
        await api.endVideoCall(callId);
      } catch (e) {
        console.error("Failed to end video call:", e);
      }
    }}
    onToggleVoiceMute={async (callId, muted) => {
      try {
        await api.setVoiceCallMuted(callId, muted);
      } catch (e) {
        console.error("Failed to toggle mute:", e);
      }
    }}
    onToggleVideoMute={async (callId, muted) => {
      try {
        await api.setVideoCallMuted(callId, muted);
      } catch (e) {
        console.error("Failed to toggle video-call mute:", e);
      }
    }}
    onToggleVideoCamera={async (callId, enabled) => {
      try {
        await api.setVideoCallCameraEnabled(callId, enabled);
      } catch (e) {
        console.error("Failed to toggle camera:", e);
      }
    }}
    bind:message={messageInput}
    bind:showAttachments
    onsend={handleSendMessage}
    onImageSent={(fileHash) => {
      // Add sent image to messages list
      const newMsg: Message = {
        id: fileHash.msg_id,
        sender: "Me",
        text: "",
        timestamp: new Date(),
        status: outgoingStatus,
        content_type: "image",
        file_hash: fileHash.file_hash,
      };
      messages = [...messages, newMsg];
    }}
    onDocumentSent={(result, fileName) => {
      // Add sent document to messages list
      const newMsg: Message = {
        id: result.msg_id,
        sender: "Me",
        text: fileName,
        timestamp: new Date(),
        status: outgoingStatus,
        content_type: "document",
        file_hash: result.file_hash,
      };
      messages = [...messages, newMsg];
    }}
    onVideoSent={(result, fileName) => {
      // Add sent video to messages list
      const newMsg: Message = {
        id: result.msg_id,
        sender: "Me",
        text: fileName,
        timestamp: new Date(),
        status: outgoingStatus,
        content_type: "video",
        file_hash: result.file_hash,
      };
      messages = [...messages, newMsg];
    }}
    onAudioSent={(result, fileName) => {
      const newMsg: Message = {
        id: result.msg_id,
        sender: "Me",
        text: fileName,
        timestamp: new Date(),
        status: outgoingStatus,
        content_type: "audio",
        file_hash: result.file_hash,
      };
      messages = [...messages, newMsg];
    }}
    onStickerSent={(result) => {
      const newMsg: Message = {
        id: result.msg_id,
        sender: "Me",
        text: "",
        timestamp: new Date(),
        status: outgoingStatus,
        content_type: "sticker",
        file_hash: result.file_hash,
      };
      messages = [...messages, newMsg];
    }}
  />
</div>
