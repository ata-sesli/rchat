<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { listen } from "@tauri-apps/api/event";
  import { onMount } from "svelte";
  import { goto } from "$app/navigation";

  // Components
  import SettingsPanel from "../components/SettingsPanel.svelte";
  import ContextMenu from "../components/ContextMenu.svelte";
  import EnvelopeModal from "../components/EnvelopeModal.svelte";
  import NewPersonModal from "../components/NewPersonModal.svelte";
  import GroupChatModal from "../components/GroupChatModal.svelte";
  import Sidebar from "../components/Sidebar.svelte";
  import ChatArea from "../components/ChatArea.svelte";

  // Types
  type Message = { sender: string; text: string; timestamp: Date };
  type Envelope = { id: string; name: string; icon?: string };

  // State
  let peers: string[] = [];
  let pinnedPeers: string[] = [];
  let peerOrder: string[] = [];
  let userProfile = {
    alias: "Me" as string | null,
    avatar_path: null as string | null,
  };
  let searchQuery = "";
  let activePeer = "Me";

  // Conversations
  let conversations: Record<string, Message[]> = {};
  $: currentLogs = conversations[activePeer] || [];

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
  let showSettings = false;
  let showEnvelopeSettings = false;
  let envelopeSettingsTargetId: string | null = null;
  let showCreateMenu = false;
  let showNewPersonModal = false;
  let showNewGroupModal = false;
  let showAttachments = false;
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
  let pendingReorderIndex: number | null = null;
  let message = "";

  // Sorted peers (reactive)
  $: sortedPeers = computeSortedPeers(
    peers,
    pinnedPeers,
    peerOrder,
    searchQuery,
    currentEnvelope,
    chatAssignments
  );

  function computeSortedPeers(
    peers: string[],
    pinnedPeers: string[],
    peerOrder: string[],
    searchQuery: string,
    currentEnvelope: string | null,
    chatAssignments: Record<string, string>
  ): string[] {
    let allPeers = ["Me", ...peers];

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

    others.sort((a, b) => {
      if (a === "Me") return -1;
      if (b === "Me") return 1;
      const aIdx = peerOrder.indexOf(a);
      const bIdx = peerOrder.indexOf(b);
      if (aIdx !== -1 && bIdx !== -1) return aIdx - bIdx;
      if (aIdx !== -1) return -1;
      if (bIdx !== -1) return 1;
      return a.localeCompare(b);
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

      await listen("message-received", (event: any) => {
        console.log("Frontend received message:", event.payload);
        const msg = event.payload;
        // Map backend message to UI message
        const newMsg = {
          sender: msg.peer_id,
          text: msg.text_content || "",
          timestamp: new Date(msg.timestamp * 1000),
        };

        // Update conversation with the sender (chat_id)
        // Note: msg.chat_id is the peer_id of the sender for incoming messages
        const chatId = msg.chat_id;
        conversations[chatId] = [...(conversations[chatId] || []), newMsg];
        conversations = conversations;
      });

      await refreshData();
    } catch (e) {
      console.error("Setup failed:", e);
    }
  });

  // Load chat history when switching peers
  $: loadChatHistory(activePeer);

  async function loadChatHistory(peerId: string) {
    if (!peerId) return;
    const chatId = peerId === "Me" ? "self" : peerId;
    try {
      const history = await invoke<any[]>("get_chat_history", { chatId });
      const mapped = history.map((m) => ({
        sender: m.peer_id === "Me" ? "Me" : m.peer_id,
        text: m.text_content || "",
        timestamp: new Date(m.timestamp * 1000),
      }));
      conversations[peerId] = mapped;
      // Trigger reactivity if needed (Svelte 4 style)
      conversations = conversations;
    } catch (e) {
      console.error("Failed to load history for", peerId, e);
    }
  }

  async function refreshData() {
    try {
      const auth = await invoke<{ is_setup: boolean; is_unlocked: boolean }>(
        "check_auth_status"
      );
      if (!auth.is_setup || !auth.is_unlocked) return goto("/login");

      peers = await invoke<string[]>("get_trusted_peers");
      pinnedPeers = await invoke<string[]>("get_pinned_peers");
      userProfile = await invoke("get_user_profile");
      envelopes = await invoke<Envelope[]>("get_envelopes");
      chatAssignments = await invoke<Record<string, string>>(
        "get_chat_assignments"
      );

      // We rely on the reactive statement $: loadChatHistory(activePeer) to load current chat
      // But we can force reload "Me" if needed, but it should happen automatically since activePeer="Me" initially.
      if (activePeer) {
        await loadChatHistory(activePeer);
      }
    } catch (e) {
      console.error("Refresh failed:", e);
    }
  }

  // Message sending
  async function handleSendMessage(text: string) {
    if (!text.trim()) return;
    const newMsg: Message = { sender: "Me", text, timestamp: new Date() };
    conversations[activePeer] = [...(conversations[activePeer] || []), newMsg];
    conversations = conversations;

    try {
      if (activePeer === "Me")
        await invoke("send_message_to_self", { message: text });
      else await invoke("send_message", { peerId: activePeer, message: text });
    } catch (e) {
      console.error("Send failed:", e);
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
          await invoke("remove_friend", { peerId: id });
          peers = peers.filter((p) => p !== id);
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

  function handleOpenSettings() {
    showSettings = true;
  }

  function handleSelectPeer(e: CustomEvent<string>) {
    activePeer = e.detail;
    showSettings = false;
  }

  function handleOpenNewPerson() {
    showNewPersonModal = true;
    showCreateMenu = false;
  }

  function handleOpenNewGroup() {
    showNewGroupModal = true;
    showCreateMenu = false;
  }

  function handleOpenEnvelopeModal() {
    openEnvelopeModal();
  }

  function handleToggleCreateMenu() {
    showCreateMenu = !showCreateMenu;
  }

  function handleEnterEnvelope(e: CustomEvent<string>) {
    enterEnvelope(e.detail);
  }

  function handleExitEnvelope() {
    exitEnvelope();
  }

  function handleSearchChange(e: CustomEvent<string>) {
    searchQuery = e.detail;
  }

  function handleSidebarContextMenu(
    e: CustomEvent<{ event: MouseEvent; type: "peer" | "envelope"; id: string }>
  ) {
    openContextMenu(e.detail.event, e.detail.type, e.detail.id);
  }

  function handleSidebarDragStart(
    e: CustomEvent<{ event: PointerEvent; peer: string }>
  ) {
    handleDragStart(e.detail.event, e.detail.peer);
  }

  function handleSidebarDragMove(e: CustomEvent<PointerEvent>) {
    handleDragMove(e.detail);
  }

  function handleSidebarDragEnd(e: CustomEvent<PointerEvent>) {
    handleDragEnd(e.detail);
  }
</script>

<svelte:window on:click={handleGlobalClick} />

<main
  class="flex h-screen bg-slate-950 text-slate-200 font-sans overflow-hidden selection:bg-teal-500/30"
>
  <Sidebar
    {isSidebarOpen}
    {showSettings}
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
    ontoggleSidebar={() => (isSidebarOpen = !isSidebarOpen)}
    onopenSettings={() => (showSettings = true)}
    onselectPeer={(peer: string) => (activePeer = peer)}
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
    {#if showSettings}
      <SettingsPanel
        on:profileUpdated={(e) =>
          (userProfile = {
            alias: e.detail.alias,
            avatar_path: e.detail.avatar_path,
          })}
        on:close={() => (showSettings = false)}
      />
    {:else if showEnvelopeSettings}
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
      <ChatArea
        {activePeer}
        messages={currentLogs}
        {userProfile}
        bind:message
        bind:showAttachments
        on:send={(e) => handleSendMessage(e.detail)}
      />
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
    on:close={() => {
      showNewPersonModal = false;
      newPersonStep = "select-network";
    }}
    on:connect={(e) => console.log("Connect to:", e.detail)}
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
  }
</style>
