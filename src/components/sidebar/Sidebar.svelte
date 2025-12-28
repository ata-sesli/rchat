<script lang="ts">
  import { fade } from "svelte/transition";
  import { flip } from "svelte/animate";
  import EnvelopeItem from "./EnvelopeItem.svelte";

  // Props with callbacks
  let {
    isSidebarOpen = true,
    currentEnvelope = null as string | null,
    searchQuery = $bindable(""),
    showCreateMenu = false,
    envelopes = [] as { id: string; name: string; icon?: string }[],
    sortedPeers = [] as string[],
    pinnedPeers = [] as string[],
    activePeer = "Me",
    userProfile = {
      alias: null as string | null,
      avatar_path: null as string | null,
    },
    dragOverEnvelopeId = null as string | null,
    isDragging = false,
    draggingPeer = null as string | null,
    isOnline = false,
    localPeers = [] as { peer_id: string; address: string }[],
    // Callbacks
    ontoggleOnline = () => {},
    ontoggleSidebar = () => {},
    onopenSettings = () => {},
    onselectPeer = (peer: string) => {},
    oncontextMenu = (data: {
      event: MouseEvent;
      type: "peer" | "envelope";
      id: string;
    }) => {},
    onenterEnvelope = (id: string) => {},
    onexitEnvelope = () => {},
    onopenNewPerson = () => {},
    onopenNewGroup = () => {},
    onopenEnvelopeModal = () => {},
    ontoggleCreateMenu = () => {},
    onsearchChange = (query: string) => {},
    ondragStart = (data: { event: PointerEvent; peer: string }) => {},
    ondragMove = (e: PointerEvent) => {},
    ondragEnd = (e: PointerEvent) => {},
  } = $props();

  // Event handlers that call callbacks
  function toggleSidebar() {
    ontoggleSidebar();
  }

  function openSettings() {
    onopenSettings();
  }

  function selectPeer(peer: string) {
    if (!isDragging) {
      onselectPeer(peer);
    }
  }

  function openContextMenu(
    e: MouseEvent,
    type: "peer" | "envelope",
    id: string
  ) {
    oncontextMenu({ event: e, type, id });
  }

  function enterEnvelope(id: string) {
    onenterEnvelope(id);
  }

  function exitEnvelope() {
    onexitEnvelope();
  }

  function openNewPerson() {
    onopenNewPerson();
  }

  function openNewGroup() {
    onopenNewGroup();
  }

  function openEnvelopeModal() {
    onopenEnvelopeModal();
  }

  function toggleCreateMenu() {
    ontoggleCreateMenu();
  }

  function handleDragStart(e: PointerEvent, peer: string) {
    // Prevent default to avoid text selection
    e.preventDefault();
    // Capture pointer to track drag even if it leaves the element
    (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
    ondragStart({ event: e, peer });
  }

  function handleDragMove(e: PointerEvent) {
    ondragMove(e);
  }

  function handleDragEnd(e: PointerEvent) {
    ondragEnd(e);
  }

  // Helper to check if a peer is online
  function isPeerOnline(peerId: string) {
    if (peerId === "Me") return true;
    if (peerId === "General") return true; // General is always "available"
    return localPeers.some((p) => p.peer_id === peerId);
  }
</script>

<aside
  class={`flex flex-col bg-slate-900 border-r border-slate-800/50 transition-all duration-300 ease-in-out overflow-hidden h-full select-none
  ${isSidebarOpen ? "w-80 opacity-100" : "w-16 opacity-100"}`}
>
  <!-- Sidebar Header / Search -->
  <div class="p-5 shrink-0 flex flex-col gap-4">
    <div class="flex items-center justify-between">
      {#if isSidebarOpen}
        <div class="flex items-center gap-2 overflow-hidden">
          {#if userProfile.avatar_path}
            <img
              src={userProfile.avatar_path}
              alt="Avatar"
              class="w-8 h-8 rounded-lg object-cover shadow-lg"
              draggable="false"
            />
          {:else}
            <img
              src="/logo.svg"
              alt="RChat"
              class="w-8 h-8 rounded-lg shadow-lg"
            />
          {/if}
          <span class="font-bold text-slate-200 truncate"
            >{userProfile.alias || "RChat"}</span
          >
        </div>
      {/if}

      <button
        onclick={toggleSidebar}
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
            <path
              fill-rule="evenodd"
              d="M12.707 5.293a1 1 0 010 1.414L9.414 10l3.293 3.293a1 1 0 01-1.414 1.414l-4-4a1 1 0 010-1.414l4-4a1 1 0 011.414 0z"
              clip-rule="evenodd"
            />
          {:else}
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
        <!-- Offline/Online Switch -->
        <div class="flex items-center justify-between px-1 mb-2">
          <span
            class="text-xs font-semibold uppercase tracking-wider text-slate-500"
          >
            {isOnline ? "Online" : "Offline"}
          </span>
          <button
            onclick={() => {
              console.log("Sidebar: Toggle clicked");
              ontoggleOnline();
            }}
            class={`w-10 h-5 rounded-full relative transition-colors duration-300 z-50 cursor-pointer ${isOnline ? "bg-teal-500" : "bg-slate-700"}`}
            title={isOnline ? "Go Offline" : "Go Online"}
          >
            <div
              class={`absolute top-1 w-3 h-3 rounded-full bg-white transition-all duration-300 shadow-sm pointer-events-none ${isOnline ? "left-6" : "left-1"}`}
            ></div>
          </button>
        </div>

        <div class="flex gap-2">
          <input
            type="text"
            placeholder="Search..."
            bind:value={searchQuery}
            oninput={() => onsearchChange(searchQuery)}
            class="flex-1 bg-slate-800 text-sm text-slate-300 rounded-lg pl-4 pr-4 py-2.5 border border-slate-700 focus:outline-none focus:border-slate-600 transition-colors placeholder:text-slate-600"
          />
          <div class="relative">
            <button
              onclick={(e) => {
                e.stopPropagation();
                toggleCreateMenu();
              }}
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

            {#if showCreateMenu}
              <div
                class="absolute top-full right-0 mt-2 w-48 bg-slate-900 border border-slate-700 rounded-lg shadow-xl z-50 py-1"
                transition:fade={{ duration: 100 }}
              >
                <button
                  onclick={openNewPerson}
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
                  onclick={openNewGroup}
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
                  onclick={openEnvelopeModal}
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

        {#if currentEnvelope}
          <button
            onclick={exitEnvelope}
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
        <EnvelopeItem
          envelope={env}
          isDropTarget={dragOverEnvelopeId === env.id}
          onclick={() => enterEnvelope(env.id)}
          oncontextmenu={(e) => openContextMenu(e, "envelope", env.id)}
        />
      {/each}
      {#if envelopes.length > 0}
        <div class="h-px bg-slate-800/50 my-2 mx-2"></div>
      {/if}
    </div>
  {/if}

  <!-- User List -->
  <div
    class="flex-1 overflow-y-auto overflow-x-hidden px-2 space-y-1 pb-4 shrink-0 scrollbar-hide select-none"
  >
    {#if isSidebarOpen}
      {#each sortedPeers as peer (peer)}
        {@const isPinned = pinnedPeers.includes(peer)}
        <div
          animate:flip={{ duration: 200 }}
          transition:fade={{ duration: 150 }}
          class="relative group/item"
        >
          <!-- svelte-ignore a11y-interactive-supports-focus -->
          <!-- svelte-ignore a11y-click-events-have-key-events -->
          <!-- svelte-ignore a11y-no-static-element-interactions -->
          <div
            onpointerdown={(e) => handleDragStart(e, peer)}
            onpointermove={handleDragMove}
            onpointerup={handleDragEnd}
            onpointercancel={handleDragEnd}
            role="button"
            id={`peer-item-${peer}`}
            onclick={() => selectPeer(peer)}
            class={`w-full flex items-center gap-3 p-3 rounded-xl cursor-grab transition-all border border-transparent touch-none relative z-10 select-none
                ${activePeer === peer ? "bg-slate-800/80 border-slate-700/50" : "hover:bg-slate-800/30"}
                ${draggingPeer === peer ? "opacity-50 cursor-grabbing" : ""}`}
          >
            <div class="relative pointer-events-none">
              {#if peer === "Me"}
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
                <div
                  class="w-10 h-10 rounded-full bg-slate-700 flex items-center justify-center text-slate-300 font-medium group-hover:bg-slate-600 shadow-md"
                >
                  #
                </div>
              {:else}
                <!-- Default avatar with initials -->
                <div
                  class="w-10 h-10 rounded-full bg-gradient-to-br from-indigo-500 to-purple-600 flex items-center justify-center text-white font-bold shadow-md ring-2 ring-transparent group-hover:ring-slate-700 transition-all"
                >
                  {peer.slice(0, 2).toUpperCase()}
                </div>
              {/if}

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
              {#if peer === "Me"}
                <p class="text-xs text-slate-500 truncate">Note to self</p>
              {:else if peer === "General"}
                <p class="text-xs text-slate-500 truncate">Public Broadcast</p>
              {:else}
                <p class="text-xs text-slate-400 truncate">Connected</p>
              {/if}
            </div>

            <button
              onclick={(e) => {
                e.stopPropagation();
                openContextMenu(e, "peer", peer);
              }}
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
      <div class="flex flex-col gap-2 items-center">
        {#each sortedPeers as peer}
          <button
            onclick={() => selectPeer(peer)}
            class={`w-10 h-10 rounded-full bg-slate-800 overflow-hidden border-2 transition-transform hover:scale-105 ${activePeer === peer ? "border-teal-500" : "border-transparent"}`}
            title={peer}
          >
            <img
              src={`https://github.com/${peer}.png?size=40`}
              alt={peer}
              class="w-full h-full object-cover"
              draggable="false"
              onerror={(e) =>
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
      onclick={openSettings}
      class="flex items-center justify-center gap-3 text-sm text-slate-400 hover:text-white transition-colors w-full p-2 rounded-lg hover:bg-slate-800"
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
        <span in:fade={{ duration: 150, delay: 200 }} class="whitespace-nowrap"
          >Settings</span
        >
      {/if}
    </button>
  </div>
</aside>
