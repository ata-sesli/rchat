<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { listen } from "@tauri-apps/api/event";
  import { onMount, tick } from "svelte";
  import { fade } from "svelte/transition";
  import { goto } from "$app/navigation";

  import SettingsPanel from "../components/SettingsPanel.svelte";

  // Data Models
  type Message = { sender: string; text: string; timestamp: Date };
  type Conversation = Message[];

  // UI State
  let message = "";
  let conversations: Record<string, Conversation> = {
    General: [],
    Me: [],
  };
  let activePeer = "General"; // "General" (Broadcast), "Me", or specific Username
  let currentLogs: Conversation = []; // Helper for template
  let chatContainer: HTMLDivElement;

  // Sidebar State
  let showSettings = false;
  let isSidebarOpen = true;

  // Attachments Menu State
  let showAttachments = false;

  // Data State
  let peers: string[] = [];
  let pinnedPeers: string[] = [];
  let userProfile: { alias: string | null; avatar_path: string | null } = {
    alias: "Me",
    avatar_path: null,
  };

  onMount(async () => {
    try {
      await loadData();

      const status = await invoke<{ is_setup: boolean; is_unlocked: boolean }>(
        "check_auth_status"
      );
      // console.log("[Frontend] Auth Status:", status);
      if (!status.is_setup || !status.is_unlocked) {
        goto("/login");
      }
    } catch (error) {
      console.error("Failed to check auth status:", error);
    }
  });

  async function loadData() {
    try {
      peers = await invoke<string[]>("get_trusted_peers");

      pinnedPeers = await invoke<string[]>("get_pinned_peers");

      userProfile = await invoke("get_user_profile");

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

  function formatTime(date: Date) {
    return date.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
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

  // Computed Peers for Sidebar
  let sortedPeers: string[] = [];
  $: {
    // 1. Pinned
    const pinned = peers.filter((p) => pinnedPeers.includes(p));
    // 2. Others
    const others = peers.filter((p) => !pinnedPeers.includes(p));
    sortedPeers = [...pinned, ...others];
  }
</script>

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
        <div class="relative animate-fade-in-up">
          <input
            type="text"
            placeholder="Search..."
            class="w-full bg-slate-800 text-sm text-slate-300 rounded-lg pl-4 pr-4 py-2.5 border border-slate-700 focus:outline-none focus:border-slate-600 transition-colors placeholder:text-slate-600"
          />
        </div>
      {/if}
    </div>

    <!-- User List -->
    <div
      class="flex-1 overflow-y-auto overflow-x-hidden px-2 space-y-1 pb-4 shrink-0 scrollbar-hide"
    >
      <!-- ME (You) Item -->
      {#if isSidebarOpen}
        <button
          on:click={() => {
            activePeer = "Me";
            showSettings = false;
          }}
          class={`w-full flex items-center gap-3 p-3 rounded-xl cursor-pointer transition-all group border border-transparent
              ${activePeer === "Me" ? "bg-slate-800/80 border-slate-700/50" : "hover:bg-slate-800/30"}`}
        >
          <div class="relative">
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
          </div>
          <div class="flex-1 min-w-0 text-left">
            <span
              class="font-medium text-slate-200 truncate group-hover:text-white transition-colors"
              >Me (You)</span
            >
            <p class="text-xs text-slate-500 truncate">Note to self</p>
          </div>
        </button>

        <!-- General / Broadcast -->
        <button
          on:click={() => {
            activePeer = "General";
            showSettings = false;
          }}
          class={`w-full flex items-center gap-3 p-3 rounded-xl cursor-pointer transition-all group border border-transparent
              ${activePeer === "General" ? "bg-slate-800/80 border-slate-700/50" : "hover:bg-slate-800/30"}`}
        >
          <div
            class="w-10 h-10 rounded-full bg-slate-700 flex items-center justify-center text-slate-300 font-medium group-hover:bg-slate-600 shadow-md"
          >
            #
          </div>
          <div class="flex-1 min-w-0 text-left">
            <span
              class="font-medium text-slate-200 truncate group-hover:text-white transition-colors"
              >General</span
            >
            <p class="text-xs text-slate-500 truncate">Public Broadcast</p>
          </div>
        </button>

        <div class="h-px bg-slate-800/50 my-2 mx-2"></div>
      {:else}
        <!-- Collapsed Me -->
        <button
          on:click={() => {
            activePeer = "Me";
            showSettings = false;
          }}
          class="w-10 h-10 mx-auto rounded-full bg-teal-600 flex items-center justify-center text-xs font-bold text-white shadow-lg shadow-teal-500/20 hover:scale-105 transition-transform mb-2 border-none cursor-pointer"
          title="Me (You)"
          type="button"
        >
          ME
        </button>

        <button
          on:click={() => {
            activePeer = "General";
            showSettings = false;
          }}
          class="w-10 h-10 mx-auto rounded-full bg-slate-700 flex items-center justify-center text-slate-300 font-medium hover:bg-slate-600 transition-colors mb-2 border-none cursor-pointer"
          title="General"
          type="button"
        >
          #
        </button>
      {/if}

      <!-- Dynamic Peers -->
      {#if isSidebarOpen}
        {#each sortedPeers as peer}
          {@const isPinned = pinnedPeers.includes(peer)}
          <div class="relative group/item">
            <button
              on:click={() => {
                activePeer = peer;
                showSettings = false;
              }}
              class={`w-full flex items-center gap-3 p-3 rounded-xl cursor-pointer transition-all border border-transparent
                  ${activePeer === peer ? "bg-slate-800/80 border-slate-700/50" : "hover:bg-slate-800/30"}`}
            >
              <div class="relative">
                <img
                  src={`https://github.com/${peer}.png?size=40`}
                  alt={peer}
                  class="w-10 h-10 rounded-full bg-slate-800 shadow-md ring-2 ring-transparent group-hover:ring-slate-700 transition-all"
                  on:error={(e) =>
                    ((e.currentTarget as HTMLImageElement).src =
                      "https://github.com/github.png?size=40")}
                />
                {#if isPinned}
                  <div
                    class="absolute -top-1 -right-1 bg-yellow-500/90 text-slate-950 p-0.5 rounded-full shadow-sm"
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
              <div class="flex-1 min-w-0 text-left">
                <div class="flex justify-between items-baseline mb-0.5">
                  <span
                    class="font-medium text-slate-200 truncate group-hover:text-white transition-colors"
                    >{peer}</span
                  >
                </div>
                <p class="text-xs text-slate-400 truncate">Connected</p>
              </div>
            </button>

            <!-- Pin Action (Hover) -->
            <button
              on:click|stopPropagation={() => togglePin(peer)}
              class={`absolute right-2 top-1/2 -translate-y-1/2 p-1.5 rounded-lg text-slate-500 hover:text-yellow-400 hover:bg-yellow-400/10 transition-all opacity-0 group-hover/item:opacity-100 ${isPinned ? "text-yellow-500 opacity-100" : ""}`}
              title={isPinned ? "Unpin" : "Pin"}
            >
              <svg
                xmlns="http://www.w3.org/2000/svg"
                class="h-4 w-4"
                viewBox="0 0 20 20"
                fill="currentColor"
              >
                <path d="M5 4a2 2 0 012-2h6a2 2 0 012 2v14l-5-2.5L5 18V4z" />
              </svg>
            </button>
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
    <!-- Conditional View: Settings OR Chat -->
    {#if showSettings}
      <SettingsPanel />
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
</style>
