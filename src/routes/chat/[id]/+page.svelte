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
        const newMsg = {
          id: msg.msg_id,
          sender: msg.peer_id === "Me" ? "Me" : msg.peer_id,
          text: msg.text_content || "",
          timestamp: new Date(msg.timestamp * 1000),
          status: "delivered", // Incoming messages are "delivered"
        };
        messages = [...messages, newMsg];
      }
    });

    // Listen for message status updates (e.g., delivered -> read)
    unlistenStatus = await listen("message-status-updated", (event: any) => {
      const { msg_id, status } = event.payload;
      console.log("[Chat] Message status update:", msg_id, "->", status);
      // Update the message in the list
      messages = messages.map((m) => (m.id === msg_id ? { ...m, status } : m));
    });
  });

  onDestroy(() => {
    if (unlisten) unlisten();
    if (unlistenStatus) unlistenStatus();
  });

  async function handleSendMessage(text: string) {
    if (!text.trim()) return;

    // Optimistic update
    const newMsg: Message = {
      sender: "Me",
      text,
      timestamp: new Date(),
      status: "pending",
    };
    messages = [...messages, newMsg];

    try {
      if (activePeer === "Me") {
        await invoke("send_message_to_self", { message: text });
      } else {
        await invoke("send_message", { peerId: activePeer, message: text });
      }
    } catch (e) {
      console.error("Send failed:", e);
      // TODO: Show error state?
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
  />
</div>
