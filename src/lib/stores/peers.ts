import { writable, derived, get } from "svelte/store";
import { invoke } from "@tauri-apps/api/core";

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
    const trustedPeers = await invoke<string[]>("get_trusted_peers");
    peers.set(trustedPeers);
    
    const pinned = await invoke<string[]>("get_pinned_peers");
    pinnedPeers.set(pinned);
  } catch (e) {
    console.error("Failed to load peers:", e);
  }
}

export async function togglePin(peerId: string) {
  const $pinnedPeers = get(pinnedPeers);
  const isPinned = $pinnedPeers.includes(peerId);
  
  try {
    await invoke("set_peer_pinned", { peerId, pinned: !isPinned });
    
    if (isPinned) {
      pinnedPeers.update((p) => p.filter((id) => id !== peerId));
    } else {
      pinnedPeers.update((p) => [...p, peerId]);
    }
  } catch (e) {
    console.error("Failed to toggle pin:", e);
  }
}

export async function deletePeer(peerId: string) {
  try {
    await invoke("remove_friend", { peerId });
    peers.update((p) => p.filter((id) => id !== peerId));
    pinnedPeers.update((p) => p.filter((id) => id !== peerId));
  } catch (e) {
    console.error("Failed to delete peer:", e);
    throw e;
  }
}
