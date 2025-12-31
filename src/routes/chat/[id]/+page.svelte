<script lang="ts">
  import { page } from "$app/stores";
  import { onMount, onDestroy } from "svelte";
  import { invoke } from "@tauri-apps/api/core";
  import { listen } from "@tauri-apps/api/event";
  import ChatArea from "../../../components/chat/ChatArea.svelte";

  // Types
  type Message = {
    id?: string;
    sender: string;
    text: string;
    timestamp: Date;
    status?: string;
    content_type?: string;
    file_hash?: string;
  };

  // Props/State
  let activePeer: string = "";
  $: activePeer = $page.params.id || "";

  let messages: Message[] = [];
  let userProfile = {
    alias: "Me" as string | null,
    avatar_path: null as string | null,
  };
  let messageInput = "";
  let showAttachments = false;
  let unlisten: () => void;
  let unlistenStatus: () => void;

  // Cache for status updates that arrive before we've swapped tempId → msgId
  let pendingStatusCache: Record<string, string> = {};

  // Reactive loading
  $: loadChatHistory(activePeer);

  async function loadChatHistory(peerId: string) {
    if (!peerId) return;
    try {
      // Map "Me" in URL to "self" for DB
      const chatId = peerId === "Me" ? "self" : peerId;
      const history = await invoke<any[]>("get_chat_history", { chatId });

      messages = history.map((m) => ({
        id: m.id,
        sender: m.peer_id === "Me" ? "Me" : m.peer_id,
        text: m.text_content || "",
        timestamp: new Date(m.timestamp * 1000),
        status: m.status || "delivered",
        content_type: m.content_type,
        file_hash: m.file_hash,
      }));
    } catch (e) {
      console.error("Failed to load history for", peerId, e);
      messages = [];
    }
  }

  onMount(async () => {
    // Fetch user profile locally for this page
    try {
      userProfile = await invoke("get_user_profile");
    } catch (e) {
      console.error("Failed to fetch profile", e);
    }

    // Listen for incoming messages
    unlisten = await listen("message-received", (event: any) => {
      const msg = event.payload;
      // Check if this message belongs to the current chat
      // Incoming messages have chat_id = sender_id
      // Outgoing messages have chat_id = recipient_id
      // We only care if msg.chat_id matches our activePeer (or if it's "self" and activePeer is "Me")

      let relatedPeer = msg.chat_id;
      if (relatedPeer === "self") relatedPeer = "Me";

      if (relatedPeer === activePeer) {
        const newMsg: Message = {
          id: msg.msg_id,
          sender: msg.peer_id === "Me" ? "Me" : msg.peer_id,
          text: msg.text_content || "",
          timestamp: new Date(msg.timestamp * 1000),
          status: "read",
          content_type: msg.file_hash ? "image" : "text",
          file_hash: msg.file_hash,
        };
        messages = [...messages, newMsg];

        // Send read receipt since we're actively viewing this chat
        if (msg.peer_id !== "Me") {
          invoke("mark_messages_read", { chatId: msg.chat_id }).catch((e) => {
            console.error("Failed to send read receipt:", e);
          });
        }
      }
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
  });

  async function handleSendMessage(text: string) {
    if (!text.trim()) return;

    // Generate a temporary id for tracking
    const tempId = `temp-${Date.now()}-${Math.random().toString(36).slice(2)}`;

    // Optimistic update with temp id
    const tempMsg: Message = {
      id: tempId,
      sender: "Me",
      text,
      timestamp: new Date(),
      status: "pending",
    };
    messages = [...messages, tempMsg];

    try {
      if (activePeer === "Me") {
        await invoke("send_message_to_self", { message: text });
        // Self messages are always "read"
        messages = messages.map((m) =>
          m.id === tempId ? { ...m, status: "read" } : m
        );
      } else {
        // Get the msg_id from backend
        const msgId = await invoke<string>("send_message", {
          peerId: activePeer,
          message: text,
        });

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
            ? { ...m, id: msgId, status: cachedStatus || m.status }
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

<div class="h-full flex flex-col bg-slate-950">
  <ChatArea
    {activePeer}
    {messages}
    {userProfile}
    bind:message={messageInput}
    bind:showAttachments
    onsend={handleSendMessage}
    onImageSent={(fileHash) => {
      // Add sent image to messages list
      const newMsg: Message = {
        id: `img-${Date.now()}`,
        sender: "Me",
        text: "",
        timestamp: new Date(),
        status: "delivered",
        content_type: "image",
        file_hash: fileHash,
      };
      messages = [...messages, newMsg];
    }}
    onDocumentSent={(fileHash, fileName) => {
      // Add sent document to messages list
      const newMsg: Message = {
        id: `doc-${Date.now()}`,
        sender: "Me",
        text: fileName,
        timestamp: new Date(),
        status: "delivered",
        content_type: "document",
        file_hash: fileHash,
      };
      messages = [...messages, newMsg];
    }}
  />
</div>
