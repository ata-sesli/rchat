<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { listen } from "@tauri-apps/api/event";
  import { onMount, tick } from "svelte";
  import { fade } from "svelte/transition";
  import { flip } from "svelte/animate";
  import { goto } from "$app/navigation";
  import SettingsPanel from "../components/SettingsPanel.svelte";

  // Data Models
  type Message = { sender: string; text: string; timestamp: Date };
  type Conversation = Message[];

  // UI State
  let message = "";
  let conversations: Record<string, Conversation> = {
    Me: [],
  };
  let activePeer = "Me"; // "Me" or specific Username
  let currentLogs: Conversation = []; // Helper for template
  let chatContainer: HTMLDivElement;

  // Sidebar State
  let showSettings = false;
  let isSidebarOpen = true;

  // Attachments Menu State
  let showAttachments = false;
  let showCreateMenu = false; // + button dropdown

  // Placeholder Modals
  let showNewPersonModal = false;
  let showNewGroupModal = false;

  // Drag Performance State
  let isDragging = false;
  let pendingReorderIndex: number | null = null; // Deferred reorder target

  // Data State
  let peers: string[] = [];
  let pinnedPeers: string[] = [];
  let peerOrder: string[] = []; // Custom order
  let userProfile: { alias: string | null; avatar_path: string | null } = {
    alias: "Me",
    avatar_path: null,
  };

  onMount(async () => {
    try {
      // 1. Setup Listeners
      console.log("Setting up listeners...");
      await listen("auth-status", (event: any) => {
        console.log("Auth status changed:", event.payload);
        // Refresh data if unlocked
        if (event.payload.is_unlocked) {
          refreshData();
        }
      });

      // 2. Initial Load
      await refreshData();
    } catch (e) {
      console.error("Setup failed:", e);
    }
  });

  async function refreshData() {
    try {
      const auth = await invoke<{ is_setup: boolean; is_unlocked: boolean }>(
        "check_auth_status"
      );
      if (!auth.is_setup || !auth.is_unlocked) {
        goto("/login");
        return; // Stop further data loading if not setup or unlocked
      }

      if (auth.is_unlocked) {
        // Load all data parallel
        const [friends, pinned, order, profile] = await Promise.all([
          invoke<any[]>("get_friends"),
          invoke<string[]>("get_pinned_peers"),
          invoke<string[]>("get_peer_order"),
          invoke<any>("get_user_profile"),
        ]);

        peers = friends.map((f) => f.username);
        pinnedPeers = pinned;
        peerOrder = order;
        userProfile = profile;

        await loadEnvelopes(); // Helper handles its own errors

        // Load 'Me' Chat History
        try {
          console.log("Fetching self history...");
          const selfHistory = await invoke<any[]>("get_chat_history", {
            chatId: "self",
          });
          console.log("Self history fetched:", selfHistory.length, "messages");
          conversations["Me"] = selfHistory.map((m) => ({
            sender: "Me",
            text: m.text_content || "",
            timestamp: new Date(m.timestamp * 1000),
          }));
          conversations = conversations; // Trigger Svelte reactivity
        } catch (e) {
          console.error("Failed to load self history", e);
        }
      }
    } catch (e) {
      console.error("Data refresh failed:", e);
    }
  }

  async function loadData() {
    try {
      peers = await invoke<string[]>("get_trusted_peers");

      pinnedPeers = await invoke<string[]>("get_pinned_peers");

      userProfile = await invoke("get_user_profile");

      await loadEnvelopes();

      // Initialize conversations for peers if not exists
      peers.forEach((p) => {
        if (!conversations[p]) conversations[p] = [];
      });

      // Load 'Me' History
      try {
        console.log("Fetching self history...");
        const selfHistory = await invoke<any[]>("get_chat_history", {
          chatId: "self",
        });
        console.log("Self history fetched:", selfHistory);
        conversations["Me"] = selfHistory.map((m) => ({
          sender: "Me",
          text: m.text_content || "",
          timestamp: new Date(m.timestamp * 1000),
        }));
      } catch (e) {
        console.error("Failed to load self history", e);
      }

      conversations = conversations; // Trigger Svelte Reactivity
    } catch (e) {
      console.error("Load data failed", e);
    }
  }

  // Reactive Logs
  $: currentLogs = conversations[activePeer] || [];

  // Safe Auto-scroll
  $: if (currentLogs && chatContainer && !showSettings) {
    scrollToBottom();
  }

  async function scrollToBottom() {
    await tick();
    if (chatContainer) {
      chatContainer.scrollTo({
        top: chatContainer.scrollHeight,
        behavior: "smooth",
      });
    }
  }

  // Textarea ref
  let textarea: HTMLTextAreaElement;

  async function sendMessage() {
    if (!message.trim()) return;

    const textToSend = message;
    // Clear Input Immediately (Optimistic)
    message = "";
    if (textarea) {
      textarea.style.height = "auto";
    }

    // UI Update
    const newMsg = { sender: "Me", text: textToSend, timestamp: new Date() };
    if (!conversations[activePeer]) conversations[activePeer] = [];
    conversations[activePeer] = [...conversations[activePeer], newMsg];

    try {
      // Network Send (Only if not "Me")
      if (activePeer !== "Me") {
        await invoke("send_chat_message", { message: textToSend });
      } else {
        // Save Note to Self
        await invoke("save_note_to_self", { message: textToSend });
      }
    } catch (e) {
      console.error("Failed to send message:", e);
      // Optional: Show error state on message
    }
  }

  function formatTime(timestamp: number | Date): string {
    const date =
      typeof timestamp === "number" ? new Date(timestamp * 1000) : timestamp;
    return date.toLocaleTimeString([], {
      hour: "2-digit",
      minute: "2-digit",
    });
  }

  // --- Context Menu Logic ---
  let showContextMenu = false;
  let contextMenuPos = { x: 0, y: 0 };
  let contextMenuTarget: { type: "peer" | "envelope"; id: string } | null =
    null;

  function openContextMenu(
    e: MouseEvent,
    type: "peer" | "envelope",
    id: string
  ) {
    e.preventDefault();
    e.stopPropagation();
    contextMenuPos = { x: e.clientX, y: e.clientY };
    contextMenuTarget = { type, id };
    showContextMenu = true;
  }

  function closeContextMenu() {
    showContextMenu = false;
    contextMenuTarget = null;
  }

  // Close context menu on global click
  function handleGlobalClick(e: MouseEvent) {
    if (showContextMenu) closeContextMenu();
  }

  async function handleContextAction(action: string) {
    if (!contextMenuTarget) return;
    const { type, id } = contextMenuTarget;

    console.log(`Context Action: ${action} on ${type}:${id}`);

    try {
      if (type === "peer") {
        if (action === "pin") await togglePin(id);
        if (action === "info") alert(`Peer Info: ${id}`); // Placeholder
        if (action === "remove") {
          try {
            await invoke("move_chat_to_envelope", {
              chatId: id,
              envelopeId: null,
            });
            await loadEnvelopes();
            console.log("Removed chat from envelope");
          } catch (e) {
            console.error("Failed to remove chat:", e);
            alert("Failed to remove: " + e);
          }
        }
      } else if (type === "envelope") {
        if (action === "delete") {
          // Bypass confirm for debugging:
          // if (confirm("Delete this envelope? Chats will move to root."))
          {
            console.log(`[FE] Requesting delete for id: '${id}'`);
            try {
              console.log("[FE] Invoking delete_envelope...");
              await invoke("delete_envelope", { id });
              console.log("[FE] Invoke success. Reloading envelopes...");
              await loadEnvelopes();
              console.log(
                `[FE] Envelopes reloaded. Count: ${envelopes.length}`
              );

              // Debug: Print current envelopes IDs
              console.log(
                "[FE] Current IDs:",
                envelopes.map((e) => e.id)
              );

              // Force reactivity if needed (assignment above shouldtrigger it)
            } catch (e) {
              console.error("[FE] Delete failed with error:", e);
              alert("Delete failed: " + e);
            }
          }
        }
        if (action === "edit") {
          const env = envelopes.find((e) => e.id === id);
          if (env) {
            openEnvelopeModal(env);
          }
        }
        if (action === "more") {
          envelopeSettingsTargetId = id;
          showEnvelopeSettings = true;
          // Close other UI elements
          isSidebarOpen = false; // Optional, maybe we want full screen
        }
      }
    } catch (e) {
      console.error("Context Action Failed:", e);
      alert("Action Failed: " + e);
    }

    closeContextMenu();
  }

  // --- Envelope Settings Screen State ---
  let showEnvelopeSettings = false;
  let envelopeSettingsTargetId: string | null = null;

  // --- Envelopes Logic ---
  type Envelope = { id: string; name: string; icon?: string };
  let envelopes: Envelope[] = [];
  let chatAssignments: Record<string, string> = {}; // chatId -> envelopeId
  let currentEnvelope: string | null = null;
  let dragOverEnvelopeId: string | null = null; // For Highlight Animation

  const AVAILABLE_ICONS = [
    '<svg xmlns="http://www.w3.org/2000/svg" class="h-5 w-5" viewBox="0 0 20 20" fill="currentColor"><path d="M2 6a2 2 0 012-2h5l2 2h5a2 2 0 012 2v6a2 2 0 01-2 2H4a2 2 0 01-2-2V6z" /></svg>', // Folder
    '<svg xmlns="http://www.w3.org/2000/svg" class="h-5 w-5" viewBox="0 0 20 20" fill="currentColor"><path fill-rule="evenodd" d="M6 2a1 1 0 00-1 1v1H4a2 2 0 00-2 2v10a2 2 0 002 2h12a2 2 0 002-2V6a2 2 0 00-2-2h-1V3a1 1 0 10-2 0v1H7V3a1 1 0 00-1-1zm0 5a1 1 0 000 2h8a1 1 0 100-2H6z" clip-rule="evenodd" /></svg>', // Briefcase
    '<svg xmlns="http://www.w3.org/2000/svg" class="h-5 w-5" viewBox="0 0 20 20" fill="currentColor"><path d="M10.894 2.553a1 1 0 00-1.788 0l-7 14a1 1 0 001.169 1.409l5-1.429A1 1 0 009 15.571V11a1 1 0 112 0v4.571a1 1 0 00.725.962l5 1.428a1 1 0 001.17-1.408l-7-14z" /></svg>', // Rocket
    '<svg xmlns="http://www.w3.org/2000/svg" class="h-5 w-5" viewBox="0 0 20 20" fill="currentColor"><path fill-rule="evenodd" d="M3.172 5.172a4 4 0 015.656 0L10 6.343l1.172-1.171a4 4 0 115.656 5.656L10 17.657l-6.828-6.829a4 4 0 010-5.656z" clip-rule="evenodd" /></svg>', // Heart
    '<svg xmlns="http://www.w3.org/2000/svg" class="h-5 w-5" viewBox="0 0 20 20" fill="currentColor"><path d="M9.049 2.927c.3-.921 1.603-.921 1.902 0l1.07 3.292a1 1 0 00.95.69h3.462c.969 0 1.371 1.24.588 1.81l-2.8 2.034a1 1 0 00-.364 1.118l1.07 3.292c.3.921-.755 1.688-1.54 1.118l-2.8-2.034a1 1 0 00-1.175 0l-2.8 2.034c-.784.57-1.838-.197-1.539-1.118l1.07-3.292a1 1 0 00-.364-1.118L2.98 8.72c-.783-.57-.38-1.81.588-1.81h3.461a1 1 0 00.951-.69l1.07-3.292z" /></svg>', // Star
    '<svg xmlns="http://www.w3.org/2000/svg" class="h-5 w-5" viewBox="0 0 20 20" fill="currentColor"><path d="M13 6a3 3 0 11-6 0 3 3 0 016 0zM18 8a2 2 0 11-4 0 2 2 0 014 0zM14 15a4 4 0 00-8 0v3h8v-3zM6 8a2 2 0 11-4 0 2 2 0 014 0zM16 18v-3a5.972 5.972 0 00-.75-2.906A3.005 3.005 0 0119 15v3h-3zM4.75 12.094A5.973 5.973 0 004 15v3H1v-3a3 3 0 013.75-2.906z" /></svg>', // Group
  ];

  async function loadEnvelopes() {
    try {
      envelopes = await invoke<Envelope[]>("get_envelopes");
      const assignments = await invoke<
        { chat_id: string; envelope_id: string }[]
      >("get_envelope_assignments");
      chatAssignments = {};
      assignments.forEach((a) => {
        chatAssignments[a.chat_id] = a.envelope_id;
      });
    } catch (e) {
      console.error("Failed to load envelopes:", e);
    }
  }

  // Modal State
  let showEnvelopeModal = false;
  let newEnvelopeName = "";
  let newEnvelopeIcon = AVAILABLE_ICONS[0];
  let editingEnvelopeId: string | null = null; // Track if we are editing

  function openEnvelopeModal(editData?: Envelope) {
    if (editData) {
      // Edit Mode
      editingEnvelopeId = editData.id;
      newEnvelopeName = editData.name;
      // Check if icon is in our list, if not use default or custom
      newEnvelopeIcon = editData.icon || AVAILABLE_ICONS[0]; // If icon not found in available, use it directly? Or default.
    } else {
      // Create Mode
      editingEnvelopeId = null;
      newEnvelopeName = "";
      newEnvelopeIcon = AVAILABLE_ICONS[0];
    }
    showEnvelopeModal = true;
  }

  async function submitEnvelopeCreation() {
    if (!newEnvelopeName.trim()) return;
    try {
      if (editingEnvelopeId) {
        // Update
        console.log(`Updating envelope: ${editingEnvelopeId}`);
        await invoke("update_envelope", {
          id: editingEnvelopeId,
          name: newEnvelopeName.trim(),
          icon: newEnvelopeIcon,
        });
      } else {
        // Create
        console.log("Creating new envelope");
        await invoke("create_envelope", {
          name: newEnvelopeName.trim(),
          icon: newEnvelopeIcon,
        });
      }

      await loadEnvelopes();
      showEnvelopeModal = false;
      editingEnvelopeId = null; // Reset
    } catch (e) {
      console.error("Failed to save envelope:", e);
      alert("Failed: " + e);
    }
  }

  // --- Slot-Based Drag Reorder ---

  // State
  let draggingPeer: string | null = null;
  let envelopeRects: { id: string; rect: DOMRect }[] = [];
  let peerSlots: { id: string; top: number; bottom: number }[] = [];
  let currentHoverEnvelopeId: string | null = null;

  function handleDragStart(e: PointerEvent, peer: string) {
    e.preventDefault();
    draggingPeer = peer;
    isDragging = true;

    // Capture pointer for smooth tracking
    (e.target as HTMLElement).setPointerCapture(e.pointerId);

    // Cache envelope positions
    envelopeRects = envelopes
      .map((env) => {
        const el = document.getElementById(`envelope-drop-zone-${env.id}`);
        if (el)
          el.classList.remove(
            "border-green-500",
            "bg-green-900/20",
            "border-white",
            "bg-slate-800"
          );
        return {
          id: env.id,
          rect: el ? el.getBoundingClientRect() : new DOMRect(0, 0, 0, 0),
        };
      })
      .filter((e) => e.rect.width > 0);

    // Cache peer slot positions (not the items, the slots)
    peerSlots = sortedPeers.map((p) => {
      const el = document.getElementById(`peer-item-${p}`);
      const rect = el?.getBoundingClientRect();
      return {
        id: p,
        top: rect?.top ?? 0,
        bottom: rect?.bottom ?? 0,
      };
    });
  }

  function handleDragMove(e: PointerEvent) {
    if (!draggingPeer) return;

    const y = e.clientY;
    const x = e.clientX;

    // A. Check Envelope Collision
    let targetEnvId: string | null = null;
    for (const { id, rect } of envelopeRects) {
      if (
        x >= rect.left &&
        x <= rect.right &&
        y >= rect.top &&
        y <= rect.bottom
      ) {
        targetEnvId = id;
        break;
      }
    }

    // Envelope highlight (direct DOM)
    if (currentHoverEnvelopeId !== targetEnvId) {
      if (currentHoverEnvelopeId) {
        const oldEl = document.getElementById(
          `envelope-drop-zone-${currentHoverEnvelopeId}`
        );
        if (oldEl) oldEl.classList.remove("border-white", "bg-slate-800");
      }
      if (targetEnvId) {
        const newEl = document.getElementById(
          `envelope-drop-zone-${targetEnvId}`
        );
        if (newEl) newEl.classList.add("border-white", "bg-slate-800");
      }
      currentHoverEnvelopeId = targetEnvId;
    }

    // B. Slot-Based Reorder (if not over envelope)
    if (!targetEnvId) {
      const currentIndex = peerOrder.indexOf(draggingPeer);
      if (currentIndex === -1) return;

      // Find which slot the cursor is in
      let targetIndex = currentIndex; // Default: stay in place
      for (let i = 0; i < peerSlots.length; i++) {
        const slot = peerSlots[i];
        const slotCenter = (slot.top + slot.bottom) / 2;

        // If cursor is above center and we're moving up, or below center and moving down
        if (y < slotCenter && i < currentIndex) {
          targetIndex = i;
          break;
        } else if (y > slotCenter && i > currentIndex) {
          targetIndex = i;
          // Don't break - find the LAST slot we're past
        }
      }

      // If target changed, reorder!
      if (targetIndex !== currentIndex) {
        const newOrder = [...peerOrder];
        const [removed] = newOrder.splice(currentIndex, 1);
        newOrder.splice(targetIndex, 0, removed);
        peerOrder = newOrder; // Triggers FLIP animation!

        // Update slot cache after reorder (items have moved)
        // Use requestAnimationFrame to get updated positions
        requestAnimationFrame(() => {
          peerSlots = sortedPeers.map((p) => {
            const el = document.getElementById(`peer-item-${p}`);
            const rect = el?.getBoundingClientRect();
            return {
              id: p,
              top: rect?.top ?? 0,
              bottom: rect?.bottom ?? 0,
            };
          });
        });
      }
    }
  }

  async function handleDragEnd(e: PointerEvent) {
    if (!draggingPeer) return;

    const peer = draggingPeer;
    draggingPeer = null;
    isDragging = false;
    (e.target as HTMLElement).releasePointerCapture(e.pointerId);

    // Clean up envelope hover
    if (currentHoverEnvelopeId) {
      const el = document.getElementById(
        `envelope-drop-zone-${currentHoverEnvelopeId}`
      );

      // Drop into envelope
      const target = currentHoverEnvelopeId;
      currentHoverEnvelopeId = null;

      const currentAssignment = chatAssignments[peer] || null;
      if (currentAssignment !== target) {
        chatAssignments[peer] = target;
        chatAssignments = { ...chatAssignments };

        // Success flash - solid light green border (use inline styles to override hover)
        if (el) {
          // Remove drag classes
          el.classList.remove("border-white", "bg-slate-800", "border-dashed");
          // Apply inline styles to override hover
          el.style.borderStyle = "solid";
          el.style.borderColor = "rgb(74, 222, 128)"; // green-400
          el.style.backgroundColor = "rgba(34, 197, 94, 0.1)"; // green-500/10

          setTimeout(() => {
            // Restore original state
            el.style.borderStyle = "";
            el.style.borderColor = "";
            el.style.backgroundColor = "";
            el?.classList.add("border-dashed");
          }, 1000);
        }

        try {
          await invoke("move_chat_to_envelope", {
            chatId: peer,
            envelopeId: target,
          });
        } catch (err) {
          console.error("Move failed:", err);
        }
      } else {
        // Same envelope, just clean up
        if (el) el.classList.remove("border-white", "bg-slate-800");
      }
    } else {
      // Persist reorder
      try {
        await invoke("save_peer_order", { order: peerOrder });
      } catch (err) {
        console.error("Save order failed", err);
      }
    }
  }

  let successEnvelopeId: string | null = null;
  function playSuccessFlash(id: string) {
    successEnvelopeId = id;
    setTimeout(() => (successEnvelopeId = null), 1000);
  }

  function enterEnvelope(envId: string) {
    currentEnvelope = envId;
  }

  function exitEnvelope() {
    currentEnvelope = null;
  }

  listen("p2p-message", (event) => {
    // Incoming message -> Currently treated as "General" or "Peer"
    // Ideally event.payload would contain sender info.
    // Use "General" for now as 'Public/Broadcast'
    const text = event.payload as string;
    const msg = { sender: "Peer", text, timestamp: new Date() };

    conversations["General"] = [...conversations["General"], msg];
  });

  async function togglePin(peer: string) {
    if (peer === "Me" || peer === "General") return;
    try {
      await invoke("toggle_pin_peer", { username: peer });
      pinnedPeers = await invoke("get_pinned_peers");
    } catch (e) {
      console.error(e);
    }
  }

  function handleKeydown(e: KeyboardEvent) {
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      sendMessage();
    }
  }

  function toggleSidebar() {
    isSidebarOpen = !isSidebarOpen;
  }

  // Computed Peers for Sidebar (Filtered by Envelope)
  let sortedPeers: string[] = [];
  $: {
    // 1. All Peers (System + Trusted) - NO MORE "General"
    const allPeers = ["Me", ...peers];

    // 2. Filter based on Current Envelope
    const filtered = allPeers.filter((p) => {
      const assignedEnv = chatAssignments[p];
      if (currentEnvelope) {
        // Show only if assigned to this envelope
        return assignedEnv === currentEnvelope;
      } else {
        // Show only if NOT assigned to any envelope (Root)
        return !assignedEnv;
      }
    });

    // 3. Sort (Pinned first, then custom order)
    const pinned = filtered.filter((p) => pinnedPeers.includes(p));
    const others = filtered.filter((p) => !pinnedPeers.includes(p));

    // Custom sort: Me first, then by peerOrder
    others.sort((a, b) => {
      if (a === "Me") return -1;
      if (b === "Me") return 1;
      // Check peerOrder
      const aIdx = peerOrder.indexOf(a);
      const bIdx = peerOrder.indexOf(b);
      if (aIdx !== -1 && bIdx !== -1) return aIdx - bIdx;
      if (aIdx !== -1) return -1;
      if (bIdx !== -1) return 1;
      return a.localeCompare(b);
    });

    sortedPeers = [...pinned, ...others];
  }
</script>

<svelte:window on:click={handleGlobalClick} />

<main
  class="flex h-screen bg-slate-950 text-slate-200 font-sans overflow-hidden selection:bg-teal-500/30"
>
  <!-- Sidebar -->
  <aside
    class={`flex flex-col bg-slate-900 border-r border-slate-800/50 transition-all duration-300 ease-in-out overflow-hidden h-full
    ${isSidebarOpen ? "w-80 opacity-100" : "w-16 opacity-100"}`}
  >
    <!-- Sidebar Header / Search -->
    <div class="p-5 shrink-0 flex flex-col gap-4">
      <div class="flex items-center justify-between">
        {#if isSidebarOpen}
          <!-- Profile Avatar/Info Small Header -->
          <div class="flex items-center gap-2 overflow-hidden">
            {#if userProfile.avatar_path}
              <img
                src={userProfile.avatar_path}
                class="w-8 h-8 rounded-full bg-slate-800 object-cover"
                alt="Me"
              />
            {:else}
              <div
                class="w-8 h-8 rounded-full bg-teal-600 flex items-center justify-center text-xs font-bold text-white shadow-lg shadow-teal-500/20"
              >
                ME
              </div>
            {/if}
            <span class="font-bold text-slate-200 truncate"
              >{userProfile.alias || "My Chat"}</span
            >
          </div>
        {/if}

        <!-- Toggle Sidebar Button -->
        <button
          on:click={toggleSidebar}
          class={`p-2 text-slate-500 hover:text-white hover:bg-slate-800 rounded-lg transition-colors ${!isSidebarOpen ? "mx-auto" : ""}`}
          title={isSidebarOpen ? "Close Sidebar" : "Open Sidebar"}
        >
          <svg
            xmlns="http://www.w3.org/2000/svg"
            class="h-5 w-5"
            viewBox="0 0 20 20"
            fill="currentColor"
          >
            {#if isSidebarOpen}
              <!-- Left Arrow (Close) -->
              <path
                fill-rule="evenodd"
                d="M12.707 5.293a1 1 0 010 1.414L9.414 10l3.293 3.293a1 1 0 01-1.414 1.414l-4-4a1 1 0 010-1.414l4-4a1 1 0 011.414 0z"
                clip-rule="evenodd"
              />
            {:else}
              <!-- Right Arrow (Open) -->
              <path
                fill-rule="evenodd"
                d="M7.293 14.707a1 1 0 010-1.414L10.586 10 7.293 6.707a1 1 0 011.414-1.414l4 4a1 1 0 010 1.414l-4 4a1 1 0 01-1.414 0z"
                clip-rule="evenodd"
              />
            {/if}
          </svg>
        </button>
      </div>

      {#if isSidebarOpen}
        <div class="relative animate-fade-in-up space-y-2">
          <div class="flex gap-2">
            <input
              type="text"
              placeholder="Search..."
              class="flex-1 bg-slate-800 text-sm text-slate-300 rounded-lg pl-4 pr-4 py-2.5 border border-slate-700 focus:outline-none focus:border-slate-600 transition-colors placeholder:text-slate-600"
            />
            <div class="relative">
              <button
                on:click|stopPropagation={() =>
                  (showCreateMenu = !showCreateMenu)}
                class="p-2 bg-slate-800 hover:bg-slate-700 text-slate-400 hover:text-white rounded-lg border border-slate-700 transition-colors relative"
                title="Create New"
              >
                <svg
                  xmlns="http://www.w3.org/2000/svg"
                  class="h-5 w-5"
                  viewBox="0 0 20 20"
                  fill="currentColor"
                >
                  <path
                    fill-rule="evenodd"
                    d="M10 18a8 8 0 100-16 8 8 0 000 16zm1-11a1 1 0 10-2 0v2H7a1 1 0 100 2h2v2a1 1 0 102 0v-2h2a1 1 0 100-2h-2V7z"
                    clip-rule="evenodd"
                  />
                </svg>
              </button>

              <!-- Create Menu Dropdown -->
              {#if showCreateMenu}
                <div
                  class="absolute top-full right-0 mt-2 w-48 bg-slate-900 border border-slate-700 rounded-lg shadow-xl z-50 py-1"
                  transition:fade={{ duration: 100 }}
                >
                  <button
                    on:click={() => {
                      showNewPersonModal = true;
                      showCreateMenu = false;
                    }}
                    class="w-full text-left px-4 py-2 text-sm text-slate-300 hover:bg-slate-700 hover:text-white transition-colors flex items-center gap-3"
                  >
                    <svg
                      xmlns="http://www.w3.org/2000/svg"
                      class="h-4 w-4"
                      viewBox="0 0 20 20"
                      fill="currentColor"
                    >
                      <path
                        fill-rule="evenodd"
                        d="M10 9a3 3 0 100-6 3 3 0 000 6zm-7 9a7 7 0 1114 0H3z"
                        clip-rule="evenodd"
                      />
                    </svg>
                    New Person
                  </button>
                  <button
                    on:click={() => {
                      showNewGroupModal = true;
                      showCreateMenu = false;
                    }}
                    class="w-full text-left px-4 py-2 text-sm text-slate-300 hover:bg-slate-700 hover:text-white transition-colors flex items-center gap-3"
                  >
                    <svg
                      xmlns="http://www.w3.org/2000/svg"
                      class="h-4 w-4"
                      viewBox="0 0 20 20"
                      fill="currentColor"
                    >
                      <path
                        d="M13 6a3 3 0 11-6 0 3 3 0 016 0zM18 8a2 2 0 11-4 0 2 2 0 014 0zM14 15a4 4 0 00-8 0v3h8v-3zM6 8a2 2 0 11-4 0 2 2 0 014 0zM16 18v-3a5.972 5.972 0 00-.75-2.906A3.005 3.005 0 0119 15v3h-3zM4.75 12.094A5.973 5.973 0 004 15v3H1v-3a3 3 0 013.75-2.906z"
                      />
                    </svg>
                    New Group Chat
                  </button>
                  <div class="h-px bg-slate-700 my-1"></div>
                  <button
                    on:click={() => {
                      openEnvelopeModal();
                      showCreateMenu = false;
                    }}
                    class="w-full text-left px-4 py-2 text-sm text-slate-300 hover:bg-slate-700 hover:text-white transition-colors flex items-center gap-3"
                  >
                    <svg
                      xmlns="http://www.w3.org/2000/svg"
                      class="h-4 w-4"
                      viewBox="0 0 20 20"
                      fill="currentColor"
                    >
                      <path
                        d="M2 6a2 2 0 012-2h5l2 2h5a2 2 0 012 2v6a2 2 0 01-2 2H4a2 2 0 01-2-2V6z"
                      />
                    </svg>
                    New Envelope
                  </button>
                </div>
              {/if}
            </div>
          </div>

          <!-- Back from Envelope -->
          {#if currentEnvelope}
            <button
              on:click={exitEnvelope}
              class="w-full flex items-center gap-2 px-3 py-2 text-sm text-slate-400 hover:text-white bg-slate-800/50 hover:bg-slate-800 rounded-lg transition-colors border border-dashed border-slate-700"
            >
              <svg
                xmlns="http://www.w3.org/2000/svg"
                class="h-4 w-4"
                viewBox="0 0 20 20"
                fill="currentColor"
              >
                <path
                  fill-rule="evenodd"
                  d="M9.707 16.707a1 1 0 01-1.414 0l-6-6a1 1 0 010-1.414l6-6a1 1 0 011.414 1.414L5.414 9H17a1 1 0 110 2H5.414l4.293 4.293a1 1 0 010 1.414z"
                  clip-rule="evenodd"
                />
              </svg>
              <span>Back to All Chats</span>
            </button>
          {/if}
        </div>
      {/if}
    </div>

    <!-- Envelopes List (Only at Root) -->
    {#if !currentEnvelope && isSidebarOpen}
      <div class="px-2 pb-2 space-y-1">
        {#each envelopes as env (env.id)}
          <!-- Envelope Item (Drop Zone) -->
          <!-- svelte-ignore a11y-interactive-supports-focus -->
          <!-- svelte-ignore a11y-click-events-have-key-events -->
          <!-- svelte-ignore a11y-no-static-element-interactions -->
          <div
            id={`envelope-drop-zone-${env.id}`}
            role="button"
            on:click={() => enterEnvelope(env.id)}
            class={`w-full flex items-center gap-3 p-3 rounded-xl cursor-pointer transition-all group border border-dashed text-left relative z-0
               ${dragOverEnvelopeId === env.id ? "bg-teal-900/40 border-teal-500 scale-[1.02] shadow-lg shadow-teal-500/10" : "border-slate-800/50 hover:bg-slate-800/50"}`}
          >
            <div
              class={`w-10 h-10 rounded-xl flex items-center justify-center shrink-0 transition-colors pointer-events-none
                  ${dragOverEnvelopeId === env.id ? "bg-teal-500/20 text-teal-400" : "bg-orange-500/10 text-orange-400"}`}
            >
              {#if env.icon}
                {@html env.icon}
              {:else}
                <svg
                  xmlns="http://www.w3.org/2000/svg"
                  class="h-5 w-5 pointer-events-none"
                  viewBox="0 0 20 20"
                  fill="currentColor"
                >
                  <path
                    d="M2 6a2 2 0 012-2h5l2 2h5a2 2 0 012 2v6a2 2 0 01-2 2H4a2 2 0 01-2-2V6z"
                  />
                </svg>
              {/if}
            </div>
            <div
              class="flex flex-col truncate text-left flex-1 min-w-0 pointer-events-none"
            >
              <span
                class="font-bold text-slate-300 group-hover:text-white transition-colors truncate"
                >{env.name}</span
              >
              <span class="text-[10px] text-slate-500 uppercase tracking-wider"
                >Envelope</span
              >
            </div>

            <!-- Envelope Context Menu (Hover) - KEEP POINTER EVENTS ON -->
            <button
              on:click|stopPropagation={(e) =>
                openContextMenu(e, "envelope", env.id)}
              class="p-1 rounded-lg text-slate-500 hover:text-white hover:bg-slate-900/50 transition-all opacity-0 group-hover:opacity-100"
            >
              <svg
                xmlns="http://www.w3.org/2000/svg"
                class="h-4 w-4"
                viewBox="0 0 20 20"
                fill="currentColor"
              >
                <path
                  d="M10 6a2 2 0 110-4 2 2 0 010 4zM10 12a2 2 0 110-4 2 2 0 010 4zM10 18a2 2 0 110-4 2 2 0 010 4z"
                />
              </svg>
            </button>
          </div>
        {/each}
        {#if envelopes.length > 0}
          <div class="h-px bg-slate-800/50 my-2 mx-2"></div>
        {/if}
      </div>
    {/if}

    <!-- User List -->
    <div
      class="flex-1 overflow-y-auto overflow-x-hidden px-2 space-y-1 pb-4 shrink-0 scrollbar-hide"
    >
      <!-- Dynamic Peers (Now includes Me/General) -->
      {#if isSidebarOpen}
        {#each sortedPeers as peer (peer)}
          {@const isPinned = pinnedPeers.includes(peer)}
          <div animate:flip={{ duration: 200 }} class="relative group/item">
            <!-- svelte-ignore a11y-interactive-supports-focus -->
            <!-- svelte-ignore a11y-click-events-have-key-events -->
            <!-- svelte-ignore a11y-no-static-element-interactions -->
            <div
              on:pointerdown={(e) => handleDragStart(e, peer)}
              on:pointermove={handleDragMove}
              on:pointerup={handleDragEnd}
              on:pointercancel={handleDragEnd}
              role="button"
              id={`peer-item-${peer}`}
              on:click={() => {
                if (!isDragging) {
                  activePeer = peer;
                  showSettings = false;
                }
              }}
              class={`w-full flex items-center gap-3 p-3 rounded-xl cursor-grab transition-all border border-transparent touch-none relative z-10
                  ${activePeer === peer ? "bg-slate-800/80 border-slate-700/50" : "hover:bg-slate-800/30"}
                  ${draggingPeer === peer ? "opacity-50 cursor-grabbing" : ""}`}
            >
              <div class="relative pointer-events-none">
                {#if peer === "Me"}
                  <!-- ME Avatar -->
                  {#if userProfile.avatar_path}
                    <img
                      src={userProfile.avatar_path}
                      class="w-10 h-10 rounded-full bg-slate-800 object-cover shadow-lg shadow-teal-500/10"
                      alt="Me"
                    />
                  {:else}
                    <div
                      class="w-10 h-10 rounded-full bg-teal-600 flex items-center justify-center text-white font-medium shadow-lg shadow-teal-500/20"
                    >
                      ME
                    </div>
                  {/if}
                  <div
                    class="absolute bottom-0 right-0 w-3 h-3 bg-green-500 border-2 border-slate-800 rounded-full"
                  ></div>
                {:else if peer === "General"}
                  <!-- GENERAL Icon -->
                  <div
                    class="w-10 h-10 rounded-full bg-slate-700 flex items-center justify-center text-slate-300 font-medium group-hover:bg-slate-600 shadow-md"
                  >
                    #
                  </div>
                {:else}
                  <!-- PEER Avatar -->
                  <img
                    src={`https://github.com/${peer}.png?size=40`}
                    alt={peer}
                    class="w-10 h-10 rounded-full bg-slate-800 shadow-md ring-2 ring-transparent group-hover:ring-slate-700 transition-all"
                    on:error={(e) =>
                      ((e.currentTarget as HTMLImageElement).src =
                        "https://github.com/github.png?size=40")}
                  />
                {/if}

                <!-- Pin Indicator (Visual only) -->
                {#if isPinned}
                  <div
                    class="absolute -top-1 -right-1 bg-yellow-500/90 text-slate-950 p-0.5 rounded-full shadow-sm pointer-events-none z-30"
                  >
                    <svg
                      xmlns="http://www.w3.org/2000/svg"
                      class="h-3 w-3"
                      viewBox="0 0 20 20"
                      fill="currentColor"
                    >
                      <path
                        d="M5 4a2 2 0 012-2h6a2 2 0 012 2v14l-5-2.5L5 18V4z"
                      />
                    </svg>
                  </div>
                {/if}
              </div>
              <div class="flex-1 min-w-0 text-left pointer-events-none">
                <div class="flex justify-between items-baseline mb-0.5">
                  <span
                    class="font-medium text-slate-200 truncate group-hover:text-white transition-colors"
                    >{peer === "Me" ? "Me (You)" : peer}</span
                  >
                </div>
                <!-- Status/Subtitle -->
                {#if peer === "Me"}
                  <p class="text-xs text-slate-500 truncate">Note to self</p>
                {:else if peer === "General"}
                  <p class="text-xs text-slate-500 truncate">
                    Public Broadcast
                  </p>
                {:else}
                  <p class="text-xs text-slate-400 truncate">Connected</p>
                {/if}
              </div>

              <!-- Context Menu Button (Right Edge) -->
              <button
                on:click|stopPropagation={(e) =>
                  openContextMenu(e, "peer", peer)}
                class="absolute right-0 top-0 bottom-0 w-8 flex items-center justify-center text-slate-500 hover:text-white hover:bg-slate-700/50 transition-all opacity-0 group-hover/item:opacity-100 z-20 pointer-events-auto rounded-r-xl"
                title="Options"
              >
                <svg
                  xmlns="http://www.w3.org/2000/svg"
                  class="h-6 w-6"
                  viewBox="0 0 20 20"
                  fill="currentColor"
                >
                  <path
                    d="M10 6a2 2 0 110-4 2 2 0 010 4zM10 12a2 2 0 110-4 2 2 0 010 4zM10 18a2 2 0 110-4 2 2 0 010 4z"
                  />
                </svg>
              </button>
            </div>
          </div>
        {/each}
      {:else}
        <!-- Collapsed Peers -->
        <div class="flex flex-col gap-2 items-center">
          {#each sortedPeers as peer}
            <button
              on:click={() => {
                activePeer = peer;
                showSettings = false;
              }}
              class={`w-10 h-10 rounded-full bg-slate-800 overflow-hidden border-2 transition-transform hover:scale-105 ${activePeer === peer ? "border-teal-500" : "border-transparent"}`}
              title={peer}
            >
              <img
                src={`https://github.com/${peer}.png?size=40`}
                alt={peer}
                class="w-full h-full object-cover"
                on:error={(e) =>
                  ((e.currentTarget as HTMLImageElement).src =
                    "https://github.com/github.png?size=40")}
              />
            </button>
          {/each}
        </div>
      {/if}
    </div>

    <!-- Sidebar Footer -->
    <div class="p-4 border-t border-slate-800/50 shrink-0">
      <button
        on:click={() => (showSettings = true)}
        class={`flex items-center justify-center gap-3 text-sm text-slate-400 hover:text-white transition-colors w-full p-2 rounded-lg hover:bg-slate-800 ${showSettings ? "bg-slate-800 text-white" : ""}`}
        title="Settings"
      >
        <svg
          xmlns="http://www.w3.org/2000/svg"
          class="h-6 w-6 shrink-0"
          fill="none"
          viewBox="0 0 24 24"
          stroke="currentColor"
        >
          <path
            stroke-linecap="round"
            stroke-linejoin="round"
            stroke-width="2"
            d="M10.325 4.317c.426-1.756 2.924-1.756 3.35 0a1.724 1.724 0 002.573 1.066c1.543-.94 3.31.826 2.37 2.37a1.724 1.724 0 001.065 2.572c1.756.426 1.756 2.924 0 3.35a1.724 1.724 0 00-1.066 2.573c.94 1.543-.826 3.31-2.37 2.37a1.724 1.724 0 00-2.572 1.065c-.426 1.756-2.924 1.756-3.35 0a1.724 1.724 0 00-2.573-1.066c-1.543.94-3.31-.826-2.37-2.37a1.724 1.724 0 00-1.065-2.572c-1.756-.426-1.756-2.924 0-3.35a1.724 1.724 0 001.066-2.573c-.94-1.543.826-3.31 2.37-2.37.996.608 2.296.07 2.572-1.065z"
          />
          <path
            stroke-linecap="round"
            stroke-linejoin="round"
            stroke-width="2"
            d="M15 12a3 3 0 11-6 0 3 3 0 016 0z"
          />
        </svg>
        {#if isSidebarOpen}
          <span
            in:fade={{ duration: 150, delay: 200 }}
            class="whitespace-nowrap"
          >
            Settings
          </span>
        {/if}
      </button>
    </div>
  </aside>

  <!-- Main Area -->
  <section class="flex-1 flex flex-col relative h-full overflow-hidden">
    <!-- Conditional View: Settings, Envelope Settings, OR Chat -->
    {#if showSettings}
      <SettingsPanel />
    {:else if showEnvelopeSettings}
      <!-- Envelope Settings (Inline) -->
      <div class="flex-1 flex flex-col bg-slate-950">
        <!-- Header -->
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
        <!-- Content -->
        <div class="flex-1 flex items-center justify-center text-slate-500">
          <p>Settings for Envelope ID: {envelopeSettingsTargetId}</p>
        </div>
      </div>
    {:else}
      <!-- Chat Area Content -->
      <div
        class="h-16 flex items-center px-6 border-b border-slate-800/50 bg-slate-900/10 backdrop-blur-sm"
      >
        <div class="flex items-center gap-3">
          <span class="text-xl font-bold text-white">
            {#if activePeer === "Me"}
              Me (You)
            {:else if activePeer === "General"}
              # General
            {:else}
              @ {activePeer}
            {/if}
          </span>
          {#if activePeer !== "Me" && activePeer !== "General"}
            <div
              class="w-2 h-2 rounded-full bg-green-500 shadow-lg shadow-green-500/50"
            ></div>
          {/if}
        </div>
      </div>

      <!-- Messages -->
      <div
        bind:this={chatContainer}
        class="flex-1 overflow-y-auto px-6 py-6 space-y-6 scroll-smooth"
      >
        {#if currentLogs.length === 0}
          <div
            class="flex flex-col items-center justify-center h-full text-slate-500 space-y-4 opacity-0 animate-fade-in-up"
            style="animation-fill-mode: forwards;"
          >
            <div
              class="w-16 h-16 rounded-2xl bg-slate-900 border border-slate-800 flex items-center justify-center"
            >
              <span class="text-3xl">👋</span>
            </div>
            <p>
              {#if activePeer === "Me"}
                This is your personal space.
              {:else}
                Start chatting with {activePeer}!
              {/if}
            </p>
          </div>
        {/if}

        {#each currentLogs as msg}
          {@const isMe = msg.sender === "Me"}
          <div
            class={`flex w-full ${isMe ? "justify-end" : "justify-start"} animate-fade-in-up`}
          >
            <div
              class={`flex max-w-[80%] md:max-w-[60%] gap-3 ${isMe ? "flex-row-reverse" : "flex-row"}`}
            >
              <!-- Avatar -->
              <div class="shrink-0 self-end mb-1">
                {#if isMe}
                  {#if userProfile.avatar_path}
                    <img
                      src={userProfile.avatar_path}
                      class="w-8 h-8 rounded-full bg-teal-500 border-2 border-slate-950 object-cover"
                      alt="Me"
                    />
                  {:else}
                    <div
                      class="w-8 h-8 rounded-full bg-teal-500 shadow-lg shadow-teal-500/20 border-2 border-slate-950"
                    ></div>
                  {/if}
                {:else}
                  <img
                    src={`https://github.com/${activePeer}.png?size=32`}
                    class="w-8 h-8 rounded-full bg-purple-500 shadow-lg shadow-purple-500/20 border-2 border-slate-950"
                    on:error={(e) =>
                      ((e.currentTarget as HTMLImageElement).src =
                        "https://github.com/github.png?size=32")}
                    alt="Peer"
                  />
                {/if}
              </div>

              <!-- Bubble -->
              <div
                class={`px-4 py-2.5 shadow-md text-sm leading-relaxed break-words flex flex-col gap-1
                        ${
                          isMe
                            ? "bg-teal-600/90 text-white rounded-2xl rounded-tr-sm"
                            : "bg-slate-800 text-slate-200 rounded-2xl rounded-tl-sm border border-slate-700/50"
                        }`}
              >
                <span>{msg.text}</span>
                <span
                  class={`text-[10px] ${isMe ? "text-teal-200" : "text-slate-400"} self-end`}
                >
                  {formatTime(msg.timestamp)}
                </span>
              </div>
            </div>
          </div>
        {/each}
      </div>

      <!-- Floating Input Area -->
      <div class="p-6 w-full max-w-4xl mx-auto">
        <div
          class="bg-slate-900/90 backdrop-blur-md border border-slate-700 rounded-2xl p-1.5 shadow-2xl flex items-center gap-2 relative"
        >
          <!-- Attachments Button -->
          <div class="relative">
            <button
              on:click={() => (showAttachments = !showAttachments)}
              class={`p-2 rounded-xl transition-all ${showAttachments ? "bg-slate-700 text-teal-400" : "text-slate-400 hover:text-white hover:bg-slate-800"}`}
              title="Add Attachment"
            >
              <svg
                xmlns="http://www.w3.org/2000/svg"
                class="h-6 w-6"
                fill="none"
                viewBox="0 0 24 24"
                stroke="currentColor"
              >
                <path
                  stroke-linecap="round"
                  stroke-linejoin="round"
                  stroke-width="2"
                  d="M15.172 7l-6.586 6.586a2 2 0 102.828 2.828l6.414-6.586a4 4 0 00-5.656-5.656l-6.415 6.585a6 6 0 108.486 8.486L20.5 13"
                />
              </svg>
            </button>

            <!-- Attachments Menu -->
            {#if showAttachments}
              <div
                class="absolute bottom-full left-0 mb-2 w-48 bg-slate-800 border border-slate-700 rounded-xl shadow-xl overflow-hidden z-50 animate-fade-in-up"
              >
                <button
                  class="w-full text-left px-4 py-3 text-sm text-slate-200 hover:bg-slate-700 hover:text-white flex items-center gap-3 transition-colors"
                >
                  <svg
                    xmlns="http://www.w3.org/2000/svg"
                    class="h-5 w-5 text-purple-400"
                    fill="none"
                    viewBox="0 0 24 24"
                    stroke="currentColor"
                  >
                    <path
                      stroke-linecap="round"
                      stroke-linejoin="round"
                      stroke-width="2"
                      d="M4 16l4.586-4.586a2 2 0 012.828 0L16 16m-2-2l1.586-1.586a2 2 0 012.828 0L20 14m-6-6h.01M6 20h12a2 2 0 002-2V6a2 2 0 00-2-2H6a2 2 0 00-2 2v12a2 2 0 002 2z"
                    />
                  </svg>
                  Image / Video
                </button>
                <div class="h-px bg-slate-700/50"></div>
                <button
                  class="w-full text-left px-4 py-3 text-sm text-slate-200 hover:bg-slate-700 hover:text-white flex items-center gap-3 transition-colors"
                >
                  <svg
                    xmlns="http://www.w3.org/2000/svg"
                    class="h-5 w-5 text-blue-400"
                    fill="none"
                    viewBox="0 0 24 24"
                    stroke="currentColor"
                  >
                    <path
                      stroke-linecap="round"
                      stroke-linejoin="round"
                      stroke-width="2"
                      d="M9 12h6m-6 4h6m2 5H7a2 2 0 01-2-2V5a2 2 0 012-2h5.586a1 1 0 01.707.293l5.414 5.414a1 1 0 01.293.707V19a2 2 0 01-2 2z"
                    />
                  </svg>
                  Document
                </button>
                <div class="h-px bg-slate-700/50"></div>
                <button
                  class="w-full text-left px-4 py-3 text-sm text-slate-200 hover:bg-slate-700 hover:text-white flex items-center gap-3 transition-colors"
                >
                  <svg
                    xmlns="http://www.w3.org/2000/svg"
                    class="h-5 w-5 text-pink-400"
                    fill="none"
                    viewBox="0 0 24 24"
                    stroke="currentColor"
                  >
                    <path
                      stroke-linecap="round"
                      stroke-linejoin="round"
                      stroke-width="2"
                      d="M19 11a7 7 0 01-7 7m0 0a7 7 0 01-7-7m7 7v4m0 0H8m4 0h4m-4-8a3 3 0 01-3-3V5a3 3 0 116 0v6a3 3 0 01-3 3z"
                    />
                  </svg>
                  Audio
                </button>
              </div>
            {/if}
          </div>
          <textarea
            bind:this={textarea}
            bind:value={message}
            on:keydown={handleKeydown}
            on:input={(e) => {
              const target = e.currentTarget;
              target.style.height = "auto";
              target.style.height = target.scrollHeight + "px";
            }}
            placeholder={`Message ${activePeer}...`}
            rows="1"
            class="flex-1 bg-transparent text-slate-100 placeholder:text-slate-600 px-4 py-2.5 focus:outline-none min-w-0 resize-none overflow-hidden max-h-32 self-end mb-1"
          ></textarea>
          <button
            on:click={sendMessage}
            class="bg-teal-500 hover:bg-teal-400 text-slate-950 p-2.5 rounded-xl font-semibold transition-all hover:scale-105 active:scale-95 shadow-lg shadow-teal-500/20 disabled:opacity-50 disabled:cursor-not-allowed"
            disabled={!message.trim()}
          >
            <svg
              xmlns="http://www.w3.org/2000/svg"
              viewBox="0 0 20 20"
              fill="currentColor"
              class="w-5 h-5"
            >
              <path
                d="M3.105 2.289a.75.75 0 00-.826.95l1.414 4.925A1.5 1.5 0 005.135 9.25h6.115a.75.75 0 010 1.5H5.135a1.5 1.5 0 00-1.442 1.086l-1.414 4.926a.75.75 0 00.826.95 28.896 28.896 0 0015.293-7.154.75.75 0 000-1.115A28.897 28.897 0 003.105 2.289z"
              />
            </svg>
          </button>
        </div>
      </div>
    {/if}
  </section>
  <!-- Create Envelope Modal -->
  {#if showEnvelopeModal}
    <div
      class="fixed inset-0 bg-black/70 z-50 flex items-center justify-center p-4 animate-fade-in-up"
    >
      <div
        class="bg-slate-900 border border-slate-700 p-6 rounded-2xl w-full max-w-sm shadow-2xl space-y-4"
      >
        <h3 class="text-xl font-bold text-white">New Envelope</h3>
        <p class="text-sm text-slate-400">
          Create a folder to organize your chats.
        </p>

        <input
          type="text"
          bind:value={newEnvelopeName}
          placeholder="e.g. Work, Family, Projects"
          class="w-full bg-slate-800 text-white rounded-xl px-4 py-3 border border-slate-700 focus:outline-none focus:border-teal-500 focus:ring-1 focus:ring-teal-500 transition-all"
          autofocus
          on:keydown={(e) => e.key === "Enter" && submitEnvelopeCreation()}
        />

        <!-- Icon Picker -->
        <div>
          <p class="text-xs text-slate-500 uppercase font-semibold mb-2">
            Select Icon
          </p>
          <div class="grid grid-cols-6 gap-2">
            {#each AVAILABLE_ICONS as icon}
              <button
                on:click={() => (newEnvelopeIcon = icon)}
                class={`p-2 rounded-lg flex items-center justify-center transition-all ${newEnvelopeIcon === icon ? "bg-teal-600 text-white shadow-lg shadow-teal-500/30 ring-2 ring-teal-500/50" : "bg-slate-800 text-slate-400 hover:bg-slate-700 hover:text-white"}`}
              >
                {@html icon}
              </button>
            {/each}
          </div>
        </div>

        <div class="flex justify-end gap-3 pt-2">
          <button
            on:click={() => (showEnvelopeModal = false)}
            class="px-4 py-2 text-slate-400 hover:text-white transition-colors"
          >
            Cancel
          </button>
          <button
            on:click={submitEnvelopeCreation}
            disabled={!newEnvelopeName.trim()}
            class="px-6 py-2 bg-teal-600 hover:bg-teal-500 text-white rounded-lg font-bold shadow-lg shadow-teal-500/20 transition-all disabled:opacity-50 disabled:cursor-not-allowed"
          >
            {editingEnvelopeId ? "Save" : "Create"}
          </button>
        </div>
      </div>
    </div>
  {/if}

  <!-- New Person Modal (Placeholder) -->
  {#if showNewPersonModal}
    <div
      class="fixed inset-0 bg-black/70 z-50 flex items-center justify-center p-4 animate-fade-in-up"
    >
      <div
        class="bg-slate-900 border border-slate-700 p-6 rounded-2xl w-full max-w-sm shadow-2xl space-y-4"
      >
        <h3 class="text-xl font-bold text-white">Add New Person</h3>
        <p class="text-sm text-slate-400">
          This feature is coming soon. You'll be able to add contacts by their
          public key.
        </p>
        <div class="flex justify-end gap-2 pt-2">
          <button
            on:click={() => (showNewPersonModal = false)}
            class="px-4 py-2 text-sm text-slate-400 hover:text-white transition-colors"
          >
            Close
          </button>
        </div>
      </div>
    </div>
  {/if}

  <!-- New Group Chat Modal (Placeholder) -->
  {#if showNewGroupModal}
    <div
      class="fixed inset-0 bg-black/70 z-50 flex items-center justify-center p-4 animate-fade-in-up"
    >
      <div
        class="bg-slate-900 border border-slate-700 p-6 rounded-2xl w-full max-w-sm shadow-2xl space-y-4"
      >
        <h3 class="text-xl font-bold text-white">Create Group Chat</h3>
        <p class="text-sm text-slate-400">
          Group chat functionality is coming soon. You'll be able to create
          encrypted group conversations.
        </p>
        <div class="flex justify-end gap-2 pt-2">
          <button
            on:click={() => (showNewGroupModal = false)}
            class="px-4 py-2 text-sm text-slate-400 hover:text-white transition-colors"
          >
            Close
          </button>
        </div>
      </div>
    </div>
  {/if}

  <!-- Context Menu -->
  {#if showContextMenu}
    <div
      class="fixed z-[100] bg-slate-800 border border-slate-700 rounded-lg shadow-xl py-1 min-w-[140px] animate-fade-in-up"
      style="top: {contextMenuPos.y}px; left: {contextMenuPos.x}px;"
    >
      {#if contextMenuTarget?.type === "peer"}
        <button
          on:click={() => handleContextAction("pin")}
          class="w-full text-left px-4 py-2 text-sm text-slate-300 hover:bg-slate-700 hover:text-white transition-colors"
        >
          {pinnedPeers.includes(contextMenuTarget.id) ? "Unpin" : "Pin"}
        </button>
        {#if currentEnvelope}
          <button
            on:click={() => handleContextAction("remove")}
            class="w-full text-left px-4 py-2 text-sm text-red-400 hover:bg-red-500/10 transition-colors"
          >
            Remove from Envelope
          </button>
        {/if}
        <button
          on:click={() => handleContextAction("info")}
          class="w-full text-left px-4 py-2 text-sm text-slate-300 hover:bg-slate-700 hover:text-white transition-colors"
        >
          Info
        </button>
      {:else if contextMenuTarget?.type === "envelope"}
        <button
          on:click={() => handleContextAction("edit")}
          class="w-full text-left px-4 py-2 text-sm text-slate-300 hover:bg-slate-700 hover:text-white transition-colors"
        >
          Edit
        </button>
        <button
          on:click={() => handleContextAction("delete")}
          class="w-full text-left px-4 py-2 text-sm text-red-400 hover:bg-red-500/10 transition-colors"
        >
          Delete
        </button>
        <div class="h-px bg-slate-700/50 my-1 mx-2"></div>
        <button
          on:click={() => handleContextAction("more")}
          class="w-full text-left px-4 py-2 text-sm text-slate-400 hover:bg-slate-700 hover:text-white transition-colors"
        >
          More...
        </button>
      {/if}
    </div>
  {/if}
</main>

<style>
  :global(body) {
    margin: 0;
    padding: 0;
    overflow: hidden;
  }

  :global(html) {
    overflow: hidden;
  }

  /* Smooth FLIP animations for peer items */
  :global([id^="peer-item-"]) {
    will-change: transform;
  }
</style>
