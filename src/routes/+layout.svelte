<script lang="ts">
  import {
    getCurrent,
    isRegistered,
    onOpenUrl,
    register,
  } from "@tauri-apps/plugin-deep-link";
  import { onDestroy, onMount } from "svelte";
  import { goto } from "$app/navigation";
  import { page } from "$app/stores";
  import {
    type ConnectivityMode,
  } from "$lib/tauri/api";
  import { defaultGroupName, getChatKind } from "$lib/chatKind";
  import {
    appSession,
    applyConnectivitySettings,
    chatState,
    clearClosedChatMarker,
    connectedChatIds,
    createEnvelope,
    createGroup,
    deleteChat,
    deleteEnvelope,
    ensureAppReady,
    initAppSession,
    joinGroup,
    liveActions,
    liveState,
    markChatRead,
    moveChatToEnvelope,
    openTemporaryGroup,
    redeemTemporaryInvite,
    refreshChats,
    refreshUserProfile,
    saveTemporaryChatToArchive,
    selectEnvelope,
    setConnectivityMode,
    setSearchQuery,
    sortedPeers,
    togglePinPeer,
    updateEnvelope,
  } from "$lib/stores";
  import "../app.css"; // Ensure global styles are loaded

  // Components
  import ContextMenu from "../components/sidebar/ContextMenu.svelte";
  import EnvelopeModal from "../components/sidebar/EnvelopeModal.svelte";
  import NewPersonModal from "../components/sidebar/NewPersonModal.svelte";
  import GroupChatModal from "../components/chat/GroupChatModal.svelte";
  import ChatDetailsModal from "../components/chat/ChatDetailsModal.svelte";
  import Sidebar from "../components/sidebar/Sidebar.svelte";
  import ThemeProvider from "../components/ThemeProvider.svelte";

  $: activePeer = $page.params.id || "";
  let dragOverEnvelopeId: string | null = null;

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

  let isSidebarOpen = true;
  let showEnvelopeSettings = false;
  let envelopeSettingsTargetId: string | null = null;
  let showCreateMenu = false;
  let showNewPersonModal = false;
  let showNewGroupModal = false;
  let showChatDetailsModal = false;
  let chatDetailsChatId: string | null = null;
  let newPersonStep:
    | "select-network"
    | "local-scan"
    | "online"
    | "temporary-chat"
    | "create-invite-user"
    | "create-invite-code"
    | "accept-invite-user"
    | "accept-invite-code" = "select-network";

  let showContextMenu = false;
  let contextMenuPos = { x: 0, y: 0 };
  let contextMenuTarget: { type: "peer" | "envelope"; id: string } | null =
    null;

  let isDragging = false;
  let draggingPeer: string | null = null;
  let dragStartY = 0;

  const seenDeepLinks = new Set<string>();
  let layoutCleanups: Array<() => void> = [];
  let protectedStartupInFlight = false;
  $: isLoginRoute = $page.url.pathname === "/login";
  $: voiceCallState = $liveState.voiceCallState;
  $: broadcastState = $liveState.broadcastState;
  $: videoCallSupported = $liveState.videoCallSupported;
  $: videoCallUnsupportedReason = $liveState.videoCallUnsupportedReason;
  $: screenBroadcastSupported = $liveState.screenBroadcastSupported;
  $: screenBroadcastUnsupportedReason =
    $liveState.screenBroadcastUnsupportedReason;
  $: if (
    !isLoginRoute &&
    $appSession.authChecked &&
    $appSession.authPhase === "locked"
  ) {
    void goto("/login");
  }
  $: if (
    !isLoginRoute &&
    $appSession.authChecked &&
    $appSession.authPhase === "unlocked" &&
    !$appSession.appReady &&
    !protectedStartupInFlight
  ) {
    protectedStartupInFlight = true;
    void ensureAppReady().finally(() => {
      protectedStartupInFlight = false;
    });
  }
  $: if ($chatState.closedChatId && $chatState.closedChatId === activePeer) {
    clearClosedChatMarker();
    void goto("/");
  }

  function addLayoutCleanup(cleanup: () => void) {
    let called = false;
    layoutCleanups.push(() => {
      if (called) return;
      called = true;
      cleanup();
    });
  }

  function addWindowCleanup(type: string, listener: EventListener) {
    window.addEventListener(type, listener);
    addLayoutCleanup(() => window.removeEventListener(type, listener));
  }

  function cleanupLayout() {
    while (layoutCleanups.length > 0) {
      layoutCleanups.pop()?.();
    }
  }

  // Lifecycle
  async function redeemTemporaryLinkAndNavigate(link: string): Promise<boolean> {
    if (!(await ensureAppReady())) return false;
    try {
      const result = await redeemTemporaryInvite(link);
      goto(`/chat/${result.chat_id}`);
      return true;
    } catch (e) {
      console.error("Failed to redeem temporary invite from deep link:", e);
      return false;
    }
  }

  async function handleDeepLinks(urls: string[] | null | undefined) {
    if (!urls?.length) return;
    for (const url of urls) {
      const normalized = url.trim();
      if (!normalized || seenDeepLinks.has(normalized)) continue;
      seenDeepLinks.add(normalized);
      if (normalized.startsWith("rchat://temp/")) {
        const redeemed = await redeemTemporaryLinkAndNavigate(normalized);
        if (!redeemed) {
          seenDeepLinks.delete(normalized);
        }
      }
    }
  }

  async function setupDeepLinks() {
    try {
      if (!(await isRegistered("rchat"))) {
        await register("rchat");
      }
    } catch {
      // Not all platforms support dynamic registration.
    }

    await handleDeepLinks(await getCurrent());
    const unlistenOpenUrl = await onOpenUrl((urls) => {
      void handleDeepLinks(urls);
    });
    addLayoutCleanup(unlistenOpenUrl);
  }

  onMount(async () => {
    try {
      addLayoutCleanup(await initAppSession());

      addWindowCleanup("open-chat", async (event: Event) => {
        const peerId = (event as CustomEvent<{ peerId?: string }>).detail?.peerId;
        if (!peerId) return;
        if (!(await ensureAppReady())) return;
        goto(`/chat/${peerId}`);
      });

      addWindowCleanup("open-temp-invite", async (event: Event) => {
        const link = (event as CustomEvent<{ link?: string }>).detail?.link;
        if (!link) return;
        await redeemTemporaryLinkAndNavigate(link);
      });

      addWindowCleanup("profile-updated", () => {
        console.log("[Layout] Profile updated, refreshing...");
        void refreshUserProfile();
      });

      addWindowCleanup("connectivity-updated", (event: Event) => {
        const next = (event as CustomEvent<typeof $appSession.connectivitySettings>).detail;
        if (!next) return;
        applyConnectivitySettings(next);
      });

      // Force repaint on resize to fix WebKit rendering bug
      addWindowCleanup("resize", () => {
        document.body.style.display = "none";
        void document.body.offsetHeight; // Force reflow
        document.body.style.display = "";
      });

      await setupDeepLinks();
      if ($page.url.pathname !== "/login") {
        await ensureAppReady();
      }
    } catch (e) {
      console.error("Setup failed:", e);
    }
  });

  onDestroy(() => {
    cleanupLayout();
  });

  async function acceptIncomingCall() {
    if (!voiceCallState.call_id) return;
    try {
      if (voiceCallState.call_kind === "video") {
        if (!videoCallSupported) {
          await liveActions.rejectVideoCall(voiceCallState.call_id);
          return;
        }
        await liveActions.acceptVideoCall(voiceCallState.call_id);
      } else {
        await liveActions.acceptVoiceCall(voiceCallState.call_id);
      }
      if (voiceCallState.peer_id) {
        goto(`/chat/${voiceCallState.peer_id}`);
      }
    } catch (e) {
      console.error("Failed to accept call:", e);
    }
  }

  async function rejectIncomingCall() {
    if (!voiceCallState.call_id) return;
    try {
      if (voiceCallState.call_kind === "video") {
        await liveActions.rejectVideoCall(voiceCallState.call_id);
      } else {
        await liveActions.rejectVoiceCall(voiceCallState.call_id);
      }
    } catch (e) {
      console.error("Failed to reject call:", e);
    }
  }

  async function acceptIncomingBroadcast() {
    if (!broadcastState.session_id) return;
    try {
      await liveActions.acceptScreenBroadcast(broadcastState.session_id);
      if (broadcastState.peer_id) {
        goto(`/chat/${broadcastState.peer_id}`);
      }
    } catch (e) {
      console.error("Failed to accept screen share:", e);
    }
  }

  async function rejectIncomingBroadcast() {
    if (!broadcastState.session_id) return;
    try {
      await liveActions.rejectScreenBroadcast(broadcastState.session_id);
    } catch (e) {
      console.error("Failed to reject screen share:", e);
    }
  }

  async function handleSetConnectivityMode(mode: ConnectivityMode) {
    try {
      await setConnectivityMode(mode);
    } catch (e) {
      console.error("Connectivity mode update failed:", e);
    }
  }

  // Context menu handlers
  function openContextMenu(
    e: MouseEvent,
    type: "peer" | "envelope",
    id: string,
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
          await togglePinPeer(id);
        }
        if (action === "delete-peer") {
          await deleteChat(id);
          if (activePeer === id) {
            goto("/");
          }
        }
        if (action === "save-archive") {
          const result = await saveTemporaryChatToArchive(id);
          goto(`/chat/${result.chat_id}`);
        }
        if (action === "more") {
          if (getChatKind(id) === "dm") {
            chatDetailsChatId = id;
            showChatDetailsModal = true;
          }
        }
        if (action === "remove") {
          await moveChatToEnvelope(id, null);
        }
      } else if (type === "envelope") {
        if (action === "delete") {
          await deleteEnvelope(id);
        }
        if (action === "edit") {
          const env = $chatState.envelopes.find((e) => e.id === id);
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
    selectEnvelope(id);
  }
  function exitEnvelope() {
    selectEnvelope(null);
  }

  async function submitEnvelopeCreation(data?: { name: string; icon: string }) {
    // Use data passed from callback if available, otherwise fall back to bound variables
    const name = data?.name || newEnvelopeName;
    const icon = data?.icon || newEnvelopeIcon;

    if (!name.trim()) return;

    try {
      if (editingEnvelopeId) {
        await updateEnvelope(editingEnvelopeId, name, icon);
      } else {
        await createEnvelope(name, icon);
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
        await moveChatToEnvelope(draggingPeer, dragOverEnvelopeId);
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

  async function handleCreateGroup(name: string) {
    try {
      const result = await createGroup(name || "");
      showNewGroupModal = false;
      goto(`/chat/${result.chat_id}`);
    } catch (e) {
      console.error("Create group failed:", e);
    }
  }

  async function handleJoinGroup(chatId: string, name: string) {
    try {
      const fallback = name || defaultGroupName(chatId);
      const result = await joinGroup(chatId, fallback);
      showNewGroupModal = false;
      goto(`/chat/${result.chat_id}`);
    } catch (e) {
      console.error("Join group failed:", e);
    }
  }

  async function handleTempGroupJoin(chatId: string, _name: string) {
    try {
      showNewGroupModal = false;
      await openTemporaryGroup(chatId);
      goto(`/chat/${chatId}`);
    } catch (e) {
      console.error("Open temporary group failed:", e);
    }
  }

  function handleToggleCreateMenu() {
    showCreateMenu = !showCreateMenu;
  }

  function handleSelectPeer(peer: string) {
    // Use routing for navigation
    const target = peer === "Me" ? "Me" : peer;

    // Clear unread count for this chat
    if ($chatState.unreadCounts[peer]) {
      markChatRead(peer).catch((e) => {
        console.error("Failed to mark messages as read:", e);
      });
    }

    goto(`/chat/${target}`);
  }
</script>

<svelte:window onclick={handleGlobalClick} />

{#if isLoginRoute}
  <slot />
{:else if !$appSession.authChecked || !$appSession.appReady}
  <div
    class="flex h-screen items-center justify-center bg-theme-base-950 text-theme-base-300"
  >
    <div
      class="h-8 w-8 rounded-full border-2 border-theme-base-700 border-t-theme-primary-500 animate-spin"
      aria-label="Loading"
    ></div>
  </div>
{:else}
  <ThemeProvider>
  <main
    class="flex h-screen bg-theme-base-950 text-theme-base-200 font-sans overflow-hidden selection:bg-teal-500/30"
  >
    <Sidebar
      {isSidebarOpen}
      currentEnvelope={$chatState.currentEnvelope}
      searchQuery={$chatState.searchQuery}
      {showCreateMenu}
      envelopes={$chatState.envelopes}
      sortedPeers={$sortedPeers}
      peerAliases={$chatState.peerAliases}
      chatNames={$chatState.chatNames}
      groupChats={$chatState.groupChats}
      pinnedPeers={$chatState.pinnedPeers}
      {activePeer}
      userProfile={$appSession.userProfile}
      {dragOverEnvelopeId}
      {isDragging}
      {draggingPeer}
      connectivitySettings={$appSession.connectivitySettings}
      connectedChatIds={$connectedChatIds}
      unreadCounts={$chatState.unreadCounts}
      onselectConnectivityMode={handleSetConnectivityMode}
      ontoggleSidebar={() => (isSidebarOpen = !isSidebarOpen)}
      onopenSettings={() => goto("/settings")}
      onselectPeer={handleSelectPeer}
      onopenNewPerson={() => (showNewPersonModal = true)}
      onopenNewGroup={() => (showNewGroupModal = true)}
      onopenEnvelopeModal={openEnvelopeModal}
      ontoggleCreateMenu={() => (showCreateMenu = !showCreateMenu)}
      onenterEnvelope={selectEnvelope}
      onexitEnvelope={() => selectEnvelope(null)}
      onsearchChange={setSearchQuery}
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
          <div class="flex-1 flex flex-col bg-theme-base-950">
            <div
              class="h-16 flex items-center px-6 border-b border-slate-800/50 bg-slate-900/10 backdrop-blur-sm gap-4"
            >
              <button
                onclick={() => (showEnvelopeSettings = false)}
                class="p-2 rounded-lg hover:bg-theme-base-800 text-theme-base-400 hover:text-white transition-colors"
                aria-label="Back to chat"
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
              <h2 class="text-xl font-bold text-theme-base-100">
                Envelope Settings
              </h2>
            </div>
            <div
              class="flex-1 flex items-center justify-center text-theme-base-500"
            >
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
        localPeers={$chatState.localPeers}
        onclose={() => {
          showNewPersonModal = false;
          newPersonStep = "select-network";
        }}
        onconnect={async (peerId: string) => {
          console.log("Peer connected:", peerId);
          // Refresh data to show the new peer (already in known_devices from backend)
          await refreshChats();
        }}
      />

      <GroupChatModal
        show={showNewGroupModal}
        onclose={() => (showNewGroupModal = false)}
        oncreate={handleCreateGroup}
        onjoin={handleJoinGroup}
        ontempjoin={handleTempGroupJoin}
      />

      <ContextMenu
        show={showContextMenu}
        position={contextMenuPos}
        target={contextMenuTarget}
        pinnedPeers={$chatState.pinnedPeers}
        currentEnvelope={$chatState.currentEnvelope}
        onaction={handleContextAction}
      />

      <ChatDetailsModal
        show={showChatDetailsModal}
        chatId={chatDetailsChatId}
        onclose={() => {
          showChatDetailsModal = false;
          chatDetailsChatId = null;
        }}
      />

      {#if voiceCallState.phase === "incoming_ringing" && voiceCallState.call_id}
        <div class="fixed inset-0 z-[120] flex items-center justify-center bg-black/45">
          <div class="w-full max-w-sm rounded-2xl border border-theme-base-700 bg-theme-base-900 p-5 shadow-2xl">
            <div class="text-sm text-theme-base-400 mb-1">
              Incoming {voiceCallState.call_kind === "video" ? "video" : "voice"} call
            </div>
            <div class="text-lg font-semibold text-theme-base-100 mb-4 truncate">
              {voiceCallState.peer_id || "Unknown peer"}
            </div>
            {#if voiceCallState.call_kind === "video" && !videoCallSupported}
              <div class="text-xs text-theme-base-400 mb-3">
                {videoCallUnsupportedReason || "Video calls are not supported on this client."}
              </div>
            {/if}
            <div class="flex gap-2 justify-end">
              <button
                onclick={rejectIncomingCall}
                class="px-4 py-2 rounded-lg bg-theme-base-800 hover:bg-theme-base-700 text-theme-base-200"
              >
                Reject
              </button>
              <button
                onclick={acceptIncomingCall}
                class="px-4 py-2 rounded-lg bg-theme-success-600 hover:bg-theme-success-500 text-white"
                disabled={voiceCallState.call_kind === "video" && !videoCallSupported}
              >
                Accept
              </button>
            </div>
          </div>
        </div>
      {/if}

      {#if broadcastState.phase === "incoming_ringing" && broadcastState.session_id}
        <div class="fixed inset-0 z-[120] flex items-center justify-center bg-black/45">
          <div class="w-full max-w-sm rounded-2xl border border-theme-base-700 bg-theme-base-900 p-5 shadow-2xl">
            <div class="text-sm text-theme-base-400 mb-1">
              Incoming screen share
            </div>
            <div class="text-lg font-semibold text-theme-base-100 mb-2 truncate">
              {broadcastState.peer_id || "Unknown peer"}
            </div>
            {#if !screenBroadcastSupported}
              <div class="text-xs text-theme-base-400 mb-2">
                {screenBroadcastUnsupportedReason || "Screen share is not supported on this client."}
              </div>
            {/if}
            <div class="text-xs text-theme-base-400 mb-4">
              {#if broadcastState.ring_expires_at}
                Expires in {Math.max(0, broadcastState.ring_expires_at - Math.floor(Date.now() / 1000))}s
              {/if}
            </div>
            <div class="flex gap-2 justify-end">
              <button
                onclick={rejectIncomingBroadcast}
                class="px-4 py-2 rounded-lg bg-theme-base-800 hover:bg-theme-base-700 text-theme-base-200"
              >
                Reject
              </button>
              <button
                onclick={acceptIncomingBroadcast}
                class="px-4 py-2 rounded-lg bg-theme-success-600 hover:bg-theme-success-500 text-white"
                disabled={!screenBroadcastSupported}
              >
                Accept
              </button>
            </div>
          </div>
        </div>
      {/if}
    </section>
  </main>
  </ThemeProvider>
{/if}

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
