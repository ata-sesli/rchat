<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { listen } from "@tauri-apps/api/event";
  import { onMount, tick } from "svelte";
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
  let chatContainer: HTMLDivElement;

  // Sidebar State
  let showSettings = false;
  let isSidebarOpen = true;

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
    } catch (e) {
      console.error("Load data failed", e);
    }
  }

  // Auto-scroll
  $: if (conversations[activePeer] && chatContainer && !showSettings) {
    tick().then(() => {
      if (chatContainer) chatContainer.scrollTop = chatContainer.scrollHeight;
    });
  }

  async function sendMessage() {
    if (!message.trim()) return;

    // UI Update
    const newMsg = { sender: "Me", text: message, timestamp: new Date() };
    if (!conversations[activePeer]) conversations[activePeer] = [];
    conversations[activePeer] = [...conversations[activePeer], newMsg];

    // Network Send (Only if not "Me")
    if (activePeer !== "Me") {
      // Note: Currently backend is broadcast-only or simple p2p.
      // We send it out; backend handles routing if implemented, or broadcasts.
      await invoke("send_chat_message", { message });
    }

    message = "";
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
      class="flex-1 overflow-y-auto px-2 space-y-1 pb-4 shrink-0 scrollbar-hide"
    >
      <!-- ME (You) Item -->
      {#if isSidebarOpen}
        <button
          on:click={() => (activePeer = "Me")}
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
          on:click={() => (activePeer = "General")}
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
        <div
          on:click={() => (activePeer = "Me")}
          class="w-10 h-10 mx-auto rounded-full bg-teal-600 flex items-center justify-center text-xs font-bold text-white shadow-lg shadow-teal-500/20 cursor-pointer hover:scale-105 transition-transform mb-2"
          title="Me (You)"
        >
          ME
        </div>

        <div
          on:click={() => (activePeer = "General")}
          class="w-10 h-10 mx-auto rounded-full bg-slate-700 flex items-center justify-center text-slate-300 font-medium cursor-pointer hover:bg-slate-600 transition-colors mb-2"
          title="General"
        >
          #
        </div>
      {/if}

      <!-- Dynamic Peers -->
      {#if isSidebarOpen}
        {#each sortedPeers as peer}
          {@const isPinned = pinnedPeers.includes(peer)}
          <div class="relative group/item">
            <button
              on:click={() => (activePeer = peer)}
              class={`w-full flex items-center gap-3 p-3 rounded-xl cursor-pointer transition-all border border-transparent
                  ${activePeer === peer ? "bg-slate-800/80 border-slate-700/50" : "hover:bg-slate-800/30"}`}
            >
              <div class="relative">
                <img
                  src={`https://github.com/${peer}.png?size=40`}
                  alt={peer}
                  class="w-10 h-10 rounded-full bg-slate-800 shadow-md ring-2 ring-transparent group-hover:ring-slate-700 transition-all"
                  on:error={(e) =>
                    (e.currentTarget.src =
                      "https://github.com/github.png?size=40")}
                />
                {#if isPinned}
                  <div
                    class="absolute -top-1 -right-1 bg-yellow-500/90 text-slate-950 p-[2px] rounded-full shadow-sm"
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
            <img
              src={`https://github.com/${peer}.png?size=40`}
              alt={peer}
              class={`w-10 h-10 rounded-full bg-slate-800 cursor-pointer hover:scale-105 transition-transform border-2 border-transparent ${activePeer === peer ? "border-teal-500" : ""}`}
              on:click={() => (activePeer = peer)}
              title={peer}
            />
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
          class="h-6 w-6"
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
          <span>Settings</span>
        {/if}
      </button>
      <!-- Mock Go Back to Chat button inside sidebar if settings is open (optional but helpful UX) -->
      {#if showSettings}
        <button
          on:click={() => (showSettings = false)}
          class="flex items-center justify-center gap-3 text-sm text-slate-400 hover:text-white transition-colors w-full p-2 rounded-lg hover:bg-slate-800 mt-1"
          title="Chats"
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
              d="M8 10h.01M12 10h.01M16 10h.01M9 16H5a2 2 0 01-2-2V6a2 2 0 012-2h14a2 2 0 012 2v8a2 2 0 01-2 2h-5l-5 5v-5z"
            />
          </svg>
          {#if isSidebarOpen}
            <span>Chats</span>
          {/if}
        </button>
      {/if}
    </div>
  </aside>

  <!-- Main Area -->
  <section class="flex-1 flex flex-col relative h-full overflow-hidden">
    <!-- Conditional View: Settings OR Chat -->
    {#if showSettings}
      <SettingsPanel show={showSettings} />
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
        {@const currentLogs = conversations[activePeer] || []}

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
                      (e.currentTarget.src =
                        "https://github.com/github.png?size=32")}
                    alt="Peer"
                  />
                {/if}
              </div>

              <!-- Bubble -->
              <div
                class={`px-4 py-2.5 shadow-md text-sm leading-relaxed break-words
                        ${
                          isMe
                            ? "bg-teal-600/90 text-white rounded-2xl rounded-tr-sm"
                            : "bg-slate-800 text-slate-200 rounded-2xl rounded-tl-sm border border-slate-700/50"
                        }`}
              >
                {msg.text}
              </div>
            </div>
          </div>
        {/each}
      </div>

      <!-- Floating Input Area -->
      <div class="p-6 w-full max-w-4xl mx-auto">
        <div
          class="bg-slate-900/90 backdrop-blur-md border border-slate-700 rounded-2xl p-1.5 shadow-2xl flex items-center gap-2"
        >
          <input
            bind:value={message}
            on:keydown={handleKeydown}
            type="text"
            placeholder={`Message ${activePeer}...`}
            class="flex-1 bg-transparent text-slate-100 placeholder:text-slate-600 px-4 py-2.5 focus:outline-none min-w-0"
          />
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
