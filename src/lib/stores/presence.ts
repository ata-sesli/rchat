import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { writable } from "svelte/store";
import { extractPeerIdFromChatId } from "$lib/chatIdentity";
import { api } from "$lib/tauri/api";

export const connectedChatIds = writable<Set<string>>(new Set());

let initPromise: Promise<UnlistenFn> | null = null;
let activeUnlisten: UnlistenFn | null = null;

function normalizeChatId(chatId: string): string {
  return chatId === "self" ? "Me" : chatId;
}

export function normalizeConnectedIds(rawIds: string[]): Set<string> {
  return new Set(rawIds.map(normalizeChatId));
}

export function presencePeerKey(chatId: string): string {
  const normalized = normalizeChatId(chatId);
  const peerId = extractPeerIdFromChatId(normalized);
  return peerId ? `peer:${peerId}` : normalized;
}

export function isChatConnected(
  chatId: string,
  connectedIds: Set<string>,
): boolean {
  const targetKey = presencePeerKey(chatId);
  for (const connectedId of connectedIds) {
    if (presencePeerKey(connectedId) === targetKey) return true;
  }
  return false;
}

export async function initPresence(): Promise<UnlistenFn> {
  if (activeUnlisten) return activeUnlisten;
  if (initPromise) return initPromise;

  initPromise = (async () => {
    connectedChatIds.set(normalizeConnectedIds(await api.getConnectedChatIds()));

    const unlisten = await listen("connected-chat-ids-updated", (event) => {
      const ids = Array.isArray(event.payload) ? event.payload : [];
      connectedChatIds.set(normalizeConnectedIds(ids as string[]));
    });

    activeUnlisten = () => {
      unlisten();
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

export function resetPresence() {
  if (activeUnlisten) {
    activeUnlisten();
  }
  connectedChatIds.set(new Set());
  activeUnlisten = null;
  initPromise = null;
}
