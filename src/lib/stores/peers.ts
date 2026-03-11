import { writable, derived, get } from "svelte/store";
import { api } from "$lib/tauri/api";

// State
export const peers = writable<string[]>([]);
export const pinnedPeers = writable<string[]>([]);
export const searchQuery = writable<string>("");

// Drag state
export const draggingPeer = writable<string | null>(null);
export const isDragging = writable<boolean>(false);
export const dragOverEnvelopeId = writable<string | null>(null);

// Actions
export async function loadPeers() {
  try {
    const trustedPeers = await api.getTrustedPeers();
    peers.set(trustedPeers);
    
    const pinned = await api.getPinnedPeers();
    pinnedPeers.set(pinned);
  } catch (e) {
    console.error("Failed to load peers:", e);
  }
}

export async function togglePin(peerId: string) {
  try {
    const isPinned = await api.togglePinPeer(peerId);
    if (isPinned) {
      pinnedPeers.update((p) => [...new Set([...p, peerId])]);
      return;
    }
    pinnedPeers.update((p) => p.filter((id) => id !== peerId));
  } catch (e) {
    console.error("Failed to toggle pin:", e);
  }
}

export async function deletePeer(peerId: string) {
  try {
    await api.removeFriend(peerId);
    peers.update((p) => p.filter((id) => id !== peerId));
    pinnedPeers.update((p) => p.filter((id) => id !== peerId));
  } catch (e) {
    console.error("Failed to delete peer:", e);
    throw e;
  }
}
