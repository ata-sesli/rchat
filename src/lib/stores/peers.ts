import { writable, derived, get } from "svelte/store";
import { invoke } from "@tauri-apps/api/core";

// State
export const peers = writable<string[]>([]);
export const pinnedPeers = writable<string[]>([]);
export const peerOrder = writable<string[]>([]);
export const searchQuery = writable<string>("");

// Drag state
export const draggingPeer = writable<string | null>(null);
export const isDragging = writable<boolean>(false);
export const dragOverEnvelopeId = writable<string | null>(null);

// Derived: sorted peers with pinned first, filtered by search
export const sortedPeers = derived(
  [peers, pinnedPeers, peerOrder, searchQuery],
  ([$peers, $pinnedPeers, $peerOrder, $searchQuery]) => {
    // Start with all peers
    let allPeers = ["Me", ...$peers];

    // Filter by envelope (handled separately)
    let filtered = allPeers;

    // Fuzzy search filter
    if ($searchQuery.trim()) {
      const query = $searchQuery.toLowerCase().trim();
      filtered = filtered.filter((p) => {
        const name = p.toLowerCase();
        let qi = 0;
        for (let i = 0; i < name.length && qi < query.length; i++) {
          if (name[i] === query[qi]) qi++;
        }
        return qi === query.length;
      });
    }

    // Sort: pinned first, then custom order
    const pinned = filtered.filter((p) => $pinnedPeers.includes(p));
    const others = filtered.filter((p) => !$pinnedPeers.includes(p));

    others.sort((a, b) => {
      if (a === "Me") return -1;
      if (b === "Me") return 1;
      const aIdx = $peerOrder.indexOf(a);
      const bIdx = $peerOrder.indexOf(b);
      if (aIdx !== -1 && bIdx !== -1) return aIdx - bIdx;
      if (aIdx !== -1) return -1;
      if (bIdx !== -1) return 1;
      return a.localeCompare(b);
    });

    return [...pinned, ...others];
  }
);

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

export function reorderPeers(fromIdx: number, toIdx: number) {
  peerOrder.update((order) => {
    const newOrder = [...order];
    const [moved] = newOrder.splice(fromIdx, 1);
    newOrder.splice(toIdx, 0, moved);
    return newOrder;
  });
}
