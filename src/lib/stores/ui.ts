import { writable } from "svelte/store";

// Sidebar state
export const isSidebarOpen = writable<boolean>(true);
export const showSettings = writable<boolean>(false);
export const showEnvelopeSettings = writable<boolean>(false);
export const envelopeSettingsTargetId = writable<string | null>(null);

// Modal states
export const showCreateMenu = writable<boolean>(false);
export const showNewPersonModal = writable<boolean>(false);
export const showNewGroupModal = writable<boolean>(false);
export const showAttachments = writable<boolean>(false);

// New person modal step
export const newPersonStep = writable<"select-network" | "local-scan" | "online">("select-network");
export const localPeers = writable<{ peer_id: string; addresses: string[] }[]>([]);

// Context menu state
export const showContextMenu = writable<boolean>(false);
export const contextMenuPos = writable<{ x: number; y: number }>({ x: 0, y: 0 });
export const contextMenuTarget = writable<{ type: "peer" | "envelope"; id: string } | null>(null);

// Actions
export function toggleSidebar() {
  isSidebarOpen.update((v) => !v);
}

export function openSettings() {
  showSettings.set(true);
}

export function closeSettings() {
  showSettings.set(false);
}

export function openContextMenu(
  e: MouseEvent,
  type: "peer" | "envelope",
  id: string
) {
  e.preventDefault();
  e.stopPropagation();
  contextMenuPos.set({ x: e.clientX, y: e.clientY });
  contextMenuTarget.set({ type, id });
  showContextMenu.set(true);
}

export function closeContextMenu() {
  showContextMenu.set(false);
  contextMenuTarget.set(null);
}

export function closeAllMenus() {
  showCreateMenu.set(false);
  showContextMenu.set(false);
}
