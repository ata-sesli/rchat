import { writable, derived, get } from "svelte/store";
import { tick } from "svelte";
import { api } from "$lib/tauri/api";

// Types
export type Message = { sender: string; text: string; timestamp: Date };
export type Conversation = Message[];

// State
export const conversations = writable<Record<string, Conversation>>({});
export const activePeer = writable<string>("Me");
export const message = writable<string>("");

// Chat container reference (set from component)
let chatContainerRef: HTMLElement | null = null;
export function setChatContainer(el: HTMLElement | null) {
  chatContainerRef = el;
}

// Derived: current logs for active peer
export const currentLogs = derived(
  [conversations, activePeer],
  ([$conversations, $activePeer]) => $conversations[$activePeer] || []
);

// Actions
export async function sendMessage(peer: string) {
  const text = get(message).trim();
  if (!text) return;

  // Clear input immediately (optimistic)
  message.set("");

  const timestamp = new Date();
  const newMsg: Message = { sender: "Me", text, timestamp };

  // Optimistic update
  conversations.update((c) => ({
    ...c,
    [peer]: [...(c[peer] || []), newMsg],
  }));

  // Scroll to bottom
  await scrollToBottom();

  // Send to backend
  try {
    if (peer === "Me") {
      await api.sendMessageToSelf(text);
    } else {
      await api.sendMessage(peer, text);
    }
  } catch (e) {
    console.error("Failed to send message:", e);
    // Could revert optimistic update here
  }
}

export async function loadChatHistory(peer: string) {
  try {
    const history = await api.getChatHistory(peer === "Me" ? "self" : peer);
    
    conversations.update((c) => ({
      ...c,
      [peer]: history.map((m) => ({
        sender: m.peer_id === "Me" ? "Me" : m.peer_id,
        text: m.text_content || "",
        timestamp: new Date(m.timestamp * 1000),
      })),
    }));
  } catch (e) {
    console.error("Failed to load chat history:", e);
  }
}

export async function scrollToBottom() {
  await tick();
  if (chatContainerRef) {
    chatContainerRef.scrollTo({
      top: chatContainerRef.scrollHeight,
      behavior: "smooth",
    });
  }
}

export function formatTime(date: Date): string {
  return date.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
}
