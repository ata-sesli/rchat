<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { listen } from "@tauri-apps/api/event";
  import { onMount } from "svelte";
  import { goto } from "$app/navigation";
  import { page } from "$app/stores";
  import "../app.css"; // Ensure global styles are loaded

  // Components
  import ContextMenu from "../components/sidebar/ContextMenu.svelte";
  import EnvelopeModal from "../components/sidebar/EnvelopeModal.svelte";
  import NewPersonModal from "../components/sidebar/NewPersonModal.svelte";
  import GroupChatModal from "../components/chat/GroupChatModal.svelte";
  import Sidebar from "../components/sidebar/Sidebar.svelte";

  // Types
  type Message = { sender: string; text: string; timestamp: Date };
  type Envelope = { id: string; name: string; icon?: string };

  // State
  let peers: string[] = [];
  let pinnedPeers: string[] = [];
  let userProfile = {
    alias: "Me" as string | null,
    avatar_path: null as string | null,
  };
  let searchQuery = "";
  let isOnline = false;

  // Active peer is derived from route params
  $: activePeer = $page.params.id || "";

  // Envelopes
  let envelopes: Envelope[] = [];
  let chatAssignments: Record<string, string> = {};
  let currentEnvelope: string | null = null;
  let dragOverEnvelopeId: string | null = null;

  // Envelope modal
  let showEnvelopeModal = false;
  let editingEnvelopeId: string | null = null;
  let newEnvelopeName = "";
  let newEnvelopeIcon = "";
  const AVAILABLE_ICONS = [
    '<svg xmlns="http://www.w3.org/2000/svg" class="h-5 w-5" viewBox="0 0 20 20" fill="currentColor"><path d="M2 6a2 2 0 012-2h5l2 2h5a2 2 0 012 2v6a2 2 0 01-2 2H4a2 2 0 01-2-2V6z" /></svg>',
    '<svg xmlns="http://www.w3.org/2000/svg" class="h-5 w-5" viewBox="0 0 20 20" fill="currentColor"><path fill-rule="evenodd" d="M6 6V5a3 3 0 013-3h2a3 3 0 013 3v1h2a2 2 0 012 2v3.57A22.952 22.952 0 0110 13a22.95 22.95 0 01-8-1.43V8a2 2 0 012-2h2zm2-1a1 1 0 011-1h2a1 1 0 011 1v1H8V5zm1 5a1 1 0 011-1h.01a1 1 0 110 2H10a1 1 0 01-1-1z" clip-rule="evenodd" /></svg>',
    '<svg xmlns="http://www.w3.org/2000/svg" class="h-5 w-5" viewBox="0 0 20 20" fill="currentColor"><path d="M10.707 2.293a1 1 0 00-1.414 0l-7 7a1 1 0 001.414 1.414L4 10.414V17a1 1 0 001 1h2a1 1 0 001-1v-2a1 1 0 011-1h2a1 1 0 011 1v2a1 1 0 001 1h2a1 1 0 001-1v-6.586l.293.293a1 1 0 001.414-1.414l-7-7z" /></svg>',
    '<svg xmlns="http://www.w3.org/2000/svg" class="h-5 w-5" viewBox="0 0 20 20" fill="currentColor"><path fill-rule="evenodd" d="M3.172 5.172a4 4 0 015.656 0L10 6.343l1.172-1.171a4 4 0 115.656 5.656L10 17.657l-6.828-6.829a4 4 0 010-5.656z" clip-rule="evenodd" /></svg>',
    '<svg xmlns="http://www.w3.org/2000/svg" class="h-5 w-5" viewBox="0 0 20 20" fill="currentColor"><path fill-rule="evenodd" d="M11.3 1.046A1 1 0 0112 2v5h4a1 1 0 01.82 1.573l-7 10A1 1 0 018 18v-5H4a1 1 0 01-.82-1.573l7-10a1 1 0 011.12-.38z" clip-rule="evenodd" /></svg>',
  ];

  // UI State
  let isSidebarOpen = true;
  let showEnvelopeSettings = false;
  let envelopeSettingsTargetId: string | null = null;
  let showCreateMenu = false;
  let showNewPersonModal = false;
  let showNewGroupModal = false;
  let newPersonStep: "select-network" | "local-scan" | "online" =
    "select-network";
  let localPeers: { peer_id: string; addresses: string[] }[] = [];

  // Context Menu
  let showContextMenu = false;
  let contextMenuPos = { x: 0, y: 0 };
  let contextMenuTarget: { type: "peer" | "envelope"; id: string } | null =
    null;

  // Drag State
  let isDragging = false;
  let draggingPeer: string | null = null;
  let dragStartY = 0;

  // Sorted peers (reactive) - sorted by latest message time
  let latestMessageTimes: Record<string, number> = {};

  $: sortedPeers = computeSortedPeers(
    peers,
    pinnedPeers,
    latestMessageTimes,
    searchQuery,
    currentEnvelope,
    chatAssignments
  );

  function computeSortedPeers(
    peers: string[],
    pinnedPeers: string[],
    latestMessageTimes: Record<string, number>,
    searchQuery: string,
    currentEnvelope: string | null,
    chatAssignments: Record<string, string>
  ): string[] {
    let allPeers = [...peers];

    // Filter by envelope
    if (currentEnvelope) {
      allPeers = allPeers.filter((p) => chatAssignments[p] === currentEnvelope);
    } else {
      allPeers = allPeers.filter((p) => !chatAssignments[p]);
    }

    // Fuzzy search
    if (searchQuery.trim()) {
      const query = searchQuery.toLowerCase().trim();
      allPeers = allPeers.filter((p) => {
        const name = p.toLowerCase();
        let qi = 0;
        for (let i = 0; i < name.length && qi < query.length; i++) {
          if (name[i] === query[qi]) qi++;
        }
        return qi === query.length;
      });
    }

    // Sort: pinned first
    const pinned = allPeers.filter((p) => pinnedPeers.includes(p));
    const others = allPeers.filter((p) => !pinnedPeers.includes(p));

    // Sort pinned by latest message time (most recent first)
    pinned.sort((a, b) => {
      const aTime = latestMessageTimes[a] || 0;
      const bTime = latestMessageTimes[b] || 0;
      return bTime - aTime; // Most recent first
    });

    // Sort others by latest message time (most recent first)
    others.sort((a, b) => {
      if (a === "Me") return -1;
      if (b === "Me") return 1;
      const aTime = latestMessageTimes[a] || 0;
      const bTime = latestMessageTimes[b] || 0;
      return bTime - aTime; // Most recent first
    });

    return [...pinned, ...others];
  }

  // Lifecycle
  onMount(async () => {
    try {
      await listen("auth-status", () => refreshData());
      await listen("local-peer-discovered", (event: any) => {
        const peer = event.payload;
        if (!localPeers.find((p) => p.peer_id === peer.peer_id)) {
          localPeers = [...localPeers, peer];
        }
      });
      await listen("local-peer-expired", (event: any) => {
        localPeers = localPeers.filter((p) => p.peer_id !== event.payload);
      });
      // Update chat order when new message arrives
      await listen("message-received", (event: any) => {
        const chatId = event.payload?.chat_id;
        if (chatId) {
          const now = Math.floor(Date.now() / 1000);
          latestMessageTimes = { ...latestMessageTimes, [chatId]: now };
        }
      });

      // Listen for profile updates from settings
      window.addEventListener("profile-updated", () => {
        console.log("[Layout] Profile updated, refreshing...");
        refreshData();
      });

      await refreshData();
    } catch (e) {
      console.error("Setup failed:", e);
    }
  });

  async function refreshData() {
    try {
      const auth = await invoke<{
        is_setup: boolean;
        is_unlocked: boolean;
        is_online: boolean;
      }>("check_auth_status");
      if (!auth.is_setup || !auth.is_unlocked) return goto("/login");

      isOnline = auth.is_online; // Sync state

      peers = await invoke<string[]>("get_trusted_peers");
      console.log("[Layout] Fetched peers:", peers);
      pinnedPeers = await invoke<string[]>("get_pinned_peers");
      userProfile = await invoke("get_user_profile");
      envelopes = await invoke<Envelope[]>("get_envelopes");
      chatAssignments = await invoke<Record<string, string>>(
        "get_chat_assignments"
      );
      // Load latest message times for sorting
      latestMessageTimes = await invoke<Record<string, number>>(
        "get_chat_latest_times"
      );
    } catch (e) {
      console.error("Refresh failed:", e);
    }
  }

  async function handleToggleOnline() {
    console.log("Layout: handleToggleOnline called. Current:", isOnline);
    try {
      const newState = !isOnline;
      await invoke("toggle_online_status", { online: newState });
      isOnline = newState;
      console.log("Layout: Toggled to", newState);
    } catch (e) {
      console.error("Toggle online failed:", e);
    }
  }

  // Context menu handlers
  function openContextMenu(
    e: MouseEvent,
    type: "peer" | "envelope",
    id: string
  ) {
    e.preventDefault();
    contextMenuPos = { x: e.clientX, y: e.clientY };
    contextMenuTarget = { type, id };
    showContextMenu = true;
  }

  function closeContextMenu() {
    showContextMenu = false;
    contextMenuTarget = null;
  }

  function handleGlobalClick() {
    if (showContextMenu) closeContextMenu();
    showCreateMenu = false;
  }

  async function handleContextAction(action: string) {
    if (!contextMenuTarget) return;
    const { type, id } = contextMenuTarget;

    try {
      if (type === "peer") {
        if (action === "pin") {
          const isPinned = pinnedPeers.includes(id);
          await invoke("set_peer_pinned", { peerId: id, pinned: !isPinned });
          pinnedPeers = isPinned
            ? pinnedPeers.filter((p) => p !== id)
            : [...pinnedPeers, id];
        }
        if (action === "delete-peer") {
          await invoke("delete_peer", { peerId: id });
          peers = peers.filter((p) => p !== id);
          if (activePeer === id) {
            goto("/");
          }
        }
        if (action === "remove") {
          // Remove from envelope = move to root
          await invoke("move_chat_to_envelope", {
            chatId: id,
            envelopeId: null,
          });
          const updated = { ...chatAssignments };
          delete updated[id];
          chatAssignments = updated;
        }
      } else if (type === "envelope") {
        if (action === "delete") {
          await invoke("delete_envelope", { id });
          envelopes = envelopes.filter((e) => e.id !== id);
          if (currentEnvelope === id) currentEnvelope = null;
        }
        if (action === "edit") {
          const env = envelopes.find((e) => e.id === id);
          if (env) {
            newEnvelopeName = env.name;
            newEnvelopeIcon = env.icon || AVAILABLE_ICONS[0];
            editingEnvelopeId = id;
            showEnvelopeModal = true;
          }
        }
      }
    } catch (e) {
      console.error("Context action failed:", e);
    }
    closeContextMenu();
  }

  // Envelope actions
  function enterEnvelope(id: string) {
    currentEnvelope = id;
  }
  function exitEnvelope() {
    currentEnvelope = null;
  }

  async function submitEnvelopeCreation(data?: { name: string; icon: string }) {
    // Use data passed from callback if available, otherwise fall back to bound variables
    const name = data?.name || newEnvelopeName;
    const icon = data?.icon || newEnvelopeIcon;

    if (!name.trim()) return;

    try {
      if (editingEnvelopeId) {
        await invoke("update_envelope", {
          id: editingEnvelopeId,
          name: name,
          icon: icon,
        });
        envelopes = envelopes.map((e) =>
          e.id === editingEnvelopeId ? { ...e, name: name, icon: icon } : e
        );
      } else {
        const id = `env_${Date.now()}`;
        const newEnv = { id, name: name, icon: icon };
        await invoke("create_envelope", { id, name, icon });
        envelopes = [...envelopes, newEnv];
      }
    } catch (e) {
      console.error("Envelope operation failed:", e);
    }
    showEnvelopeModal = false;
    editingEnvelopeId = null;
    newEnvelopeName = "";
    newEnvelopeIcon = "";
  }

  function openEnvelopeModal() {
    newEnvelopeName = "";
    newEnvelopeIcon = AVAILABLE_ICONS[0];
    editingEnvelopeId = null;
    showEnvelopeModal = true;
    showCreateMenu = false;
  }

  // Drag handlers
  function handleDragStart(e: PointerEvent, peer: string) {
    draggingPeer = peer;
    dragStartY = e.clientY;
  }

  function handleDragMove(e: PointerEvent) {
    if (!draggingPeer) return;
    const dy = Math.abs(e.clientY - dragStartY);
    if (dy > 10) isDragging = true;

    // Check envelope drop zones
    const el = document.elementFromPoint(e.clientX, e.clientY);
    const dropZone = el?.closest('[id^="envelope-drop-zone-"]');
    dragOverEnvelopeId = dropZone
      ? dropZone.id.replace("envelope-drop-zone-", "")
      : null;
  }

  async function handleDragEnd(e: PointerEvent) {
    if (draggingPeer && dragOverEnvelopeId) {
      try {
        await invoke("move_chat_to_envelope", {
          chatId: draggingPeer,
          envelopeId: dragOverEnvelopeId,
        });
        chatAssignments = {
          ...chatAssignments,
          [draggingPeer]: dragOverEnvelopeId,
        };
      } catch (err) {
        console.error("Move failed:", err);
      }
    }
    draggingPeer = null;
    isDragging = false;
    dragOverEnvelopeId = null;
  }

  // Sidebar event handlers - individual functions for each event
  function handleToggleSidebar() {
    isSidebarOpen = !isSidebarOpen;
  }

  function handleOpenNewPerson() {
    showNewPersonModal = true;
    showCreateMenu = false;
  }

  function handleOpenNewGroup() {
    showNewGroupModal = true;
    showCreateMenu = false;
  }

  function handleToggleCreateMenu() {
    showCreateMenu = !showCreateMenu;
  }

  function handleSelectPeer(peer: string) {
    // Use routing for navigation
    const target = peer === "Me" ? "Me" : peer;
    goto(`/chat/${target}`);
  }
</script>

<svelte:window on:click={handleGlobalClick} />

<main
  class="flex h-screen bg-slate-950 text-slate-200 font-sans overflow-hidden selection:bg-teal-500/30"
>
  <Sidebar
    {isSidebarOpen}
    {currentEnvelope}
    bind:searchQuery
    {showCreateMenu}
    {envelopes}
    {sortedPeers}
    {pinnedPeers}
    {activePeer}
    {userProfile}
    {dragOverEnvelopeId}
    {isDragging}
    {draggingPeer}
    {isOnline}
    {localPeers}
    ontoggleOnline={handleToggleOnline}
    ontoggleSidebar={() => (isSidebarOpen = !isSidebarOpen)}
    onopenSettings={() => goto("/settings")}
    onselectPeer={handleSelectPeer}
    onopenNewPerson={() => (showNewPersonModal = true)}
    onopenNewGroup={() => (showNewGroupModal = true)}
    onopenEnvelopeModal={openEnvelopeModal}
    ontoggleCreateMenu={() => (showCreateMenu = !showCreateMenu)}
    onenterEnvelope={(id: string) => (currentEnvelope = id)}
    onexitEnvelope={() => (currentEnvelope = null)}
    onsearchChange={(query: string) => (searchQuery = query)}
    oncontextMenu={(data: {
      event: MouseEvent;
      type: "peer" | "envelope";
      id: string;
    }) => {
      contextMenuTarget = { type: data.type, id: data.id };
      contextMenuPos = { x: data.event.clientX, y: data.event.clientY };
      showContextMenu = true;
    }}
    ondragStart={(data: { event: PointerEvent; peer: string }) =>
      handleDragStart(data.event, data.peer)}
    ondragMove={handleDragMove}
    ondragEnd={handleDragEnd}
  />

  <section class="flex-1 flex flex-col relative h-full overflow-hidden">
    <section class="flex-1 flex flex-col relative h-full overflow-hidden">
      {#if showEnvelopeSettings}
        <div class="flex-1 flex flex-col bg-slate-950">
          <div
            class="h-16 flex items-center px-6 border-b border-slate-800/50 bg-slate-900/10 backdrop-blur-sm gap-4"
          >
            <button
              on:click={() => (showEnvelopeSettings = false)}
              class="p-2 rounded-lg hover:bg-slate-800 text-slate-400 hover:text-white transition-colors"
            >
              <svg
                xmlns="http://www.w3.org/2000/svg"
                class="h-5 w-5"
                viewBox="0 0 20 20"
                fill="currentColor"
              >
                <path
                  fill-rule="evenodd"
                  d="M12.707 5.293a1 1 0 010 1.414L9.414 10l3.293 3.293a1 1 0 01-1.414 1.414l-4-4a1 1 0 010-1.414l4-4a1 1 0 011.414 0z"
                  clip-rule="evenodd"
                />
              </svg>
            </button>
            <h2 class="text-xl font-bold text-white">Envelope Settings</h2>
          </div>
          <div class="flex-1 flex items-center justify-center text-slate-500">
            <p>Settings for Envelope ID: {envelopeSettingsTargetId}</p>
          </div>
        </div>
      {:else}
        <slot />
      {/if}
    </section>

    <!-- Modals -->
    <EnvelopeModal
      show={showEnvelopeModal}
      bind:name={newEnvelopeName}
      bind:selectedIcon={newEnvelopeIcon}
      editingId={editingEnvelopeId}
      icons={AVAILABLE_ICONS}
      onclose={() => (showEnvelopeModal = false)}
      onsubmit={submitEnvelopeCreation}
    />

    <NewPersonModal
      show={showNewPersonModal}
      bind:step={newPersonStep}
      {localPeers}
      onclose={() => {
        showNewPersonModal = false;
        newPersonStep = "select-network";
      }}
      onconnect={async (peerId: string) => {
        console.log("Peer connected:", peerId);
        // Refresh data to show the new peer (already in known_devices from backend)
        await refreshData();
      }}
    />

    <GroupChatModal
      show={showNewGroupModal}
      onclose={() => (showNewGroupModal = false)}
    />

    <ContextMenu
      show={showContextMenu}
      position={contextMenuPos}
      target={contextMenuTarget}
      {pinnedPeers}
      {currentEnvelope}
      onaction={handleContextAction}
    />
  </section>
</main>

<svelte:body class:is-dragging={isDragging} />

<style>
  :global(body) {
    margin: 0;
    padding: 0;
    overflow: hidden;
  }
  :global(body.is-dragging) {
    user-select: none;
    -webkit-user-select: none;
    cursor: grabbing !important;
  }
  :global(body.is-dragging *) {
    user-select: none;
    -webkit-user-select: none;
    cursor: grabbing !important;
  }
  :global(html) {
    overflow: hidden;
  }
  :global([id^="peer-item-"]) {
    will-change: transform;
    user-select: none;
    -webkit-user-select: none;
    cursor: grabbing !important;
  }
</style>
