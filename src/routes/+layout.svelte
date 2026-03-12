<script lang="ts">
  import { listen } from "@tauri-apps/api/event";
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
    api,
    type ConnectivityMode,
    type ConnectivitySettings,
    type VoiceCallState,
  } from "$lib/tauri/api";
  import { defaultGroupName, getChatKind } from "$lib/chatKind";
  import "../app.css"; // Ensure global styles are loaded

  // Components
  import ContextMenu from "../components/sidebar/ContextMenu.svelte";
  import EnvelopeModal from "../components/sidebar/EnvelopeModal.svelte";
  import NewPersonModal from "../components/sidebar/NewPersonModal.svelte";
  import GroupChatModal from "../components/chat/GroupChatModal.svelte";
  import Sidebar from "../components/sidebar/Sidebar.svelte";
  import ThemeProvider from "../components/ThemeProvider.svelte";

  // Types
  type Envelope = { id: string; name: string; icon?: string | null };

  // State
  let peers: string[] = [];
  let peerAliases: Record<string, string | null> = {};
  let chatNames: Record<string, string> = {};
  let groupChats: Record<string, boolean> = {};
  let pinnedPeers: string[] = [];
  let userProfile = {
    alias: "Me" as string | null,
    avatar_path: null as string | null,
  };
  let searchQuery = "";
  let connectivitySettings: ConnectivitySettings = {
    mode: "reachable",
    mdns_enabled: true,
    github_sync_enabled: true,
    nat_keepalive_enabled: true,
    punch_assist_enabled: true,
  };

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
  let newPersonStep:
    | "select-network"
    | "local-scan"
    | "online"
    | "temporary-chat"
    | "create-invite-user"
    | "create-invite-code"
    | "accept-invite-user"
    | "accept-invite-code" = "select-network";
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

  // Unread message counts per chat
  let unreadCounts: Record<string, number> = {};
  const seenDeepLinks = new Set<string>();
  let unlistenOpenUrl: (() => void) | null = null;
  let unlistenVoiceCallState: (() => void) | null = null;
  let voiceCallState: VoiceCallState = { phase: "idle", muted: false };
  let videoCallSupported = true;
  let videoCallUnsupportedReason: string | null = null;
  const autoRejectedUnsupportedVideoCalls = new Set<string>();

  function detectVideoCallSupport(): { supported: boolean; reason: string | null } {
    if (typeof window === "undefined") {
      return { supported: false, reason: "Unavailable in this environment." };
    }
    if (!navigator?.mediaDevices?.getUserMedia) {
      return { supported: false, reason: "Camera capture is unavailable on this device." };
    }
    const w = window as any;
    if (!w.VideoEncoder || !w.VideoDecoder || !w.EncodedVideoChunk || !w.MediaStreamTrackProcessor) {
      return { supported: false, reason: "WebCodecs video support is unavailable on this client." };
    }
    return { supported: true, reason: null };
  }

  $: {
    const incomingUnsupportedVideoCallId =
      voiceCallState.phase === "incoming_ringing" &&
      voiceCallState.call_kind === "video" &&
      voiceCallState.call_id &&
      !videoCallSupported
        ? voiceCallState.call_id
        : null;
    if (
      incomingUnsupportedVideoCallId &&
      !autoRejectedUnsupportedVideoCalls.has(incomingUnsupportedVideoCallId)
    ) {
      autoRejectedUnsupportedVideoCalls.add(incomingUnsupportedVideoCallId);
      void api.rejectVideoCall(incomingUnsupportedVideoCallId).catch((e) => {
        console.error("Failed to auto-reject unsupported incoming video call:", e);
      });
    }
    if (voiceCallState.phase === "idle" && autoRejectedUnsupportedVideoCalls.size > 32) {
      autoRejectedUnsupportedVideoCalls.clear();
    }
  }

  $: sortedPeers = computeSortedPeers(
    peers,
    pinnedPeers,
    latestMessageTimes,
    searchQuery,
    currentEnvelope,
    chatAssignments,
  );

  function computeSortedPeers(
    peers: string[],
    pinnedPeers: string[],
    latestMessageTimes: Record<string, number>,
    searchQuery: string,
    currentEnvelope: string | null,
    chatAssignments: Record<string, string>,
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
  async function redeemTemporaryLinkAndNavigate(link: string): Promise<boolean> {
    try {
      const result = await api.redeemTemporaryInvite(link);
      await refreshData();
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

  onMount(async () => {
    const support = detectVideoCallSupport();
    videoCallSupported = support.supported;
    videoCallUnsupportedReason = support.reason;
    try {
      await listen("auth-status", () => refreshData());
      window.addEventListener("open-chat", async (event: Event) => {
        const peerId = (event as CustomEvent<{ peerId?: string }>).detail?.peerId;
        if (!peerId) return;
        await refreshData();
        goto(`/chat/${peerId}`);
      });
      window.addEventListener("open-temp-invite", async (event: Event) => {
        const link = (event as CustomEvent<{ link?: string }>).detail?.link;
        if (!link) return;
        await redeemTemporaryLinkAndNavigate(link);
      });
      await listen("local-peer-discovered", (event: any) => {
        const peer = event.payload;
        if (!localPeers.find((p) => p.peer_id === peer.peer_id)) {
          localPeers = [...localPeers, peer];
        }
      });
      await listen("local-peer-expired", (event: any) => {
        localPeers = localPeers.filter((p) => p.peer_id !== event.payload);
      });
      // Update chat order and unread counts when new message arrives
      await listen("message-received", (event: any) => {
        const rawChatId = event.payload?.chat_id;
        if (rawChatId) {
          const chatId = rawChatId === "self" ? "Me" : rawChatId;
          const now = Math.floor(Date.now() / 1000);
          latestMessageTimes = { ...latestMessageTimes, [chatId]: now };

          // Increment unread count if user is NOT viewing this chat
          if (chatId !== activePeer) {
            unreadCounts = {
              ...unreadCounts,
              [chatId]: (unreadCounts[chatId] || 0) + 1,
            };
          }
        }
      });

      // Listen for new GitHub chats (handshake received)
      await listen("new-github-chat", (event: any) => {
        const chatId = event.payload?.chat_id;
        console.log("[Layout] Received new-github-chat event:", event.payload);
        if (chatId && !peers.includes(chatId)) {
          peers = [...peers, chatId];
          console.log("[Layout] Added new peer to list:", chatId);
        }
      });
      await listen("temporary-chat-connected", async () => {
        await refreshData();
      });
      await listen("temporary-chat-ended", async (event: any) => {
        const chatId = event.payload?.chat_id;
        if (chatId && activePeer === chatId) {
          goto("/");
        }
        await refreshData();
      });

      // Listen for profile updates from settings
      window.addEventListener("profile-updated", () => {
        console.log("[Layout] Profile updated, refreshing...");
        refreshData();
      });
      window.addEventListener("connectivity-updated", (event: Event) => {
        const next = (event as CustomEvent<ConnectivitySettings>).detail;
        if (!next) return;
        connectivitySettings = next;
      });

      // Force repaint on resize to fix WebKit rendering bug
      window.addEventListener("resize", () => {
        document.body.style.display = "none";
        void document.body.offsetHeight; // Force reflow
        document.body.style.display = "";
      });

      await refreshData();
      try {
        voiceCallState = await api.getVoiceCallState();
      } catch (e) {
        console.warn("Voice call state unavailable yet:", e);
      }
      unlistenVoiceCallState = await listen<VoiceCallState>(
        "voice-call-state-updated",
        (event) => {
          voiceCallState = event.payload;
        },
      );
      try {
        if (!(await isRegistered("rchat"))) {
          await register("rchat");
        }
      } catch {
        // Not all platforms support dynamic registration.
      }
      await handleDeepLinks(await getCurrent());
      unlistenOpenUrl = await onOpenUrl((urls) => {
        void handleDeepLinks(urls);
      });
    } catch (e) {
      console.error("Setup failed:", e);
    }
  });

  onDestroy(() => {
    if (unlistenOpenUrl) {
      unlistenOpenUrl();
      unlistenOpenUrl = null;
    }
    if (unlistenVoiceCallState) {
      unlistenVoiceCallState();
      unlistenVoiceCallState = null;
    }
  });

  async function acceptIncomingCall() {
    if (!voiceCallState.call_id) return;
    try {
      if (voiceCallState.call_kind === "video") {
        if (!videoCallSupported) {
          await api.rejectVideoCall(voiceCallState.call_id);
          return;
        }
        await api.acceptVideoCall(voiceCallState.call_id);
      } else {
        await api.acceptVoiceCall(voiceCallState.call_id);
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
        await api.rejectVideoCall(voiceCallState.call_id);
      } else {
        await api.rejectVoiceCall(voiceCallState.call_id);
      }
    } catch (e) {
      console.error("Failed to reject call:", e);
    }
  }

  async function refreshData() {
    try {
      const auth = await api.checkAuthStatus();
      if (!auth.is_setup || !auth.is_unlocked) return goto("/login");

      // Ensure network is started (handles session restore case)
      console.log("[Layout] Ensuring network is started...");
      await api.startNetwork();

      connectivitySettings = auth.connectivity;

      const chatList = await api.getChatList();
      const nextPeers: string[] = [];
      const nextChatNames: Record<string, string> = {};
      const nextGroupChats: Record<string, boolean> = {};
      for (const chat of chatList) {
        const uiId = chat.id === "self" ? "Me" : chat.id;
        nextPeers.push(uiId);
        nextChatNames[uiId] = chat.name;
        nextGroupChats[uiId] = chat.is_group;
      }
      peers = nextPeers;
      chatNames = nextChatNames;
      groupChats = nextGroupChats;

      peerAliases = await api.getPeerAliases();

      pinnedPeers = await api.getPinnedPeers();
      userProfile = await api.getUserProfile();
      envelopes = await api.getEnvelopes();
      chatAssignments = Object.fromEntries(
        (await api.getEnvelopeAssignments()).map((item) => [
          item.chat_id === "self" ? "Me" : item.chat_id,
          item.envelope_id,
        ]),
      );
      // Load latest message times for sorting
      const latestTimesRaw = await api.getChatLatestTimes();
      latestMessageTimes = Object.fromEntries(
        Object.entries(latestTimesRaw).map(([k, v]) => [k === "self" ? "Me" : k, v]),
      );
      // Load unread message counts for badges
      const unreadRaw = await api.getUnreadCounts("Me");
      unreadCounts = Object.fromEntries(
        Object.entries(unreadRaw).map(([k, v]) => [k === "self" ? "Me" : k, v]),
      );
    } catch (e) {
      console.error("Refresh failed:", e);
    }
  }

  async function handleSetConnectivityMode(mode: ConnectivityMode) {
    try {
      connectivitySettings = await api.setConnectivityMode(mode);
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
          const isPinned = await api.togglePinPeer(id);
          pinnedPeers = isPinned
            ? [...new Set([...pinnedPeers, id])]
            : pinnedPeers.filter((p) => p !== id);
        }
        if (action === "delete-peer") {
          if (getChatKind(id) === "group") {
            await api.leaveGroupChat(id);
          } else if (getChatKind(id) === "tempgroup" || getChatKind(id) === "tempdm") {
            await api.cancelTemporaryInvite().catch(() => {});
          } else {
            await api.deletePeer(id);
          }
          peers = peers.filter((p) => p !== id);
          if (activePeer === id) {
            goto("/");
          }
        }
        if (action === "save-archive") {
          const result = await api.saveTemporaryChatToArchive(id);
          await refreshData();
          goto(`/chat/${result.chat_id}`);
        }
        if (action === "remove") {
          // Remove from envelope = move to root
          await api.moveChatToEnvelope(id === "Me" ? "self" : id, null);
          const updated = { ...chatAssignments };
          delete updated[id];
          chatAssignments = updated;
        }
      } else if (type === "envelope") {
        if (action === "delete") {
          await api.deleteEnvelope(id);
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
        await api.updateEnvelope(editingEnvelopeId, name, icon);
        envelopes = envelopes.map((e) =>
          e.id === editingEnvelopeId ? { ...e, name: name, icon: icon } : e,
        );
      } else {
        const id = `env_${Date.now()}`;
        const newEnv = { id, name: name, icon: icon };
        await api.createEnvelope(id, name, icon);
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
        const chatId = draggingPeer === "Me" ? "self" : draggingPeer;
        await api.moveChatToEnvelope(chatId, dragOverEnvelopeId);
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

  async function handleCreateGroup(name: string) {
    try {
      const result = await api.createGroupChat(name || null);
      showNewGroupModal = false;
      await refreshData();
      goto(`/chat/${result.chat_id}`);
    } catch (e) {
      console.error("Create group failed:", e);
    }
  }

  async function handleJoinGroup(chatId: string, name: string) {
    try {
      const fallback = name || defaultGroupName(chatId);
      const result = await api.joinGroupChat(chatId, fallback);
      showNewGroupModal = false;
      await refreshData();
      goto(`/chat/${result.chat_id}`);
    } catch (e) {
      console.error("Join group failed:", e);
    }
  }

  async function handleTempGroupJoin(chatId: string, _name: string) {
    try {
      showNewGroupModal = false;
      await refreshData();
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
    if (unreadCounts[peer]) {
      const { [peer]: _, ...rest } = unreadCounts;
      unreadCounts = rest;

      // Mark messages as read in backend
      const chatId = peer === "Me" ? "self" : peer;
      api.markMessagesRead(chatId).catch((e) => {
        console.error("Failed to mark messages as read:", e);
      });
    }

    goto(`/chat/${target}`);
  }
</script>

<svelte:window onclick={handleGlobalClick} />

<ThemeProvider>
  <main
    class="flex h-screen bg-theme-base-950 text-theme-base-200 font-sans overflow-hidden selection:bg-teal-500/30"
  >
    <Sidebar
      {isSidebarOpen}
      {currentEnvelope}
      bind:searchQuery
      {showCreateMenu}
      {envelopes}
      {sortedPeers}
      {peerAliases}
      {chatNames}
      {groupChats}
      {pinnedPeers}
      {activePeer}
      {userProfile}
      {dragOverEnvelopeId}
      {isDragging}
      {draggingPeer}
      {connectivitySettings}
      {localPeers}
      {unreadCounts}
      onselectConnectivityMode={handleSetConnectivityMode}
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
        oncreate={handleCreateGroup}
        onjoin={handleJoinGroup}
        ontempjoin={handleTempGroupJoin}
      />

      <ContextMenu
        show={showContextMenu}
        position={contextMenuPos}
        target={contextMenuTarget}
        {pinnedPeers}
        {currentEnvelope}
        onaction={handleContextAction}
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
    </section>
  </main>
</ThemeProvider>

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
