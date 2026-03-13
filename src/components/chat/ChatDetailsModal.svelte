<script lang="ts">
  import {
    api,
    type ChatDetailsOverview,
    type ChatFileFilter,
    type ChatFileRow,
    type ChatStats,
  } from "$lib/tauri/api";

  type TabKey = "peer" | "connection" | "stats" | "files";

  const FILE_FILTERS: ChatFileFilter[] = [
    "all",
    "sticker",
    "image",
    "video",
    "document",
    "audio",
  ];

  let {
    show = false,
    chatId = null as string | null,
    onclose = () => {},
  }: {
    show?: boolean;
    chatId?: string | null;
    onclose?: () => void;
  } = $props();

  let activeTab = $state<TabKey>("peer");
  let overview = $state<ChatDetailsOverview | null>(null);
  let stats = $state<ChatStats | null>(null);
  let files = $state<ChatFileRow[]>([]);
  let filesFilter = $state<ChatFileFilter>("all");
  let filesOffset = $state(0);
  let filesHasMore = $state(false);
  let loadingOverview = $state(false);
  let loadingStats = $state(false);
  let loadingFiles = $state(false);
  let actionBusy = $state(false);
  let errorMessage = $state<string | null>(null);
  let lastLoadedChat = $state<string | null>(null);
  let nowUnix = $state(Math.floor(Date.now() / 1000));

  function toShortPeerId(peerId: string): string {
    if (peerId.length <= 20) return peerId;
    return `${peerId.slice(0, 10)}...${peerId.slice(-8)}`;
  }

  function formatTimestamp(unixTs?: number | null): string {
    if (!unixTs) return "-";
    return new Date(unixTs * 1000).toLocaleString();
  }

  function formatDuration(seconds: number | null): string {
    if (!seconds || seconds <= 0) return "0s";
    const hours = Math.floor(seconds / 3600);
    const minutes = Math.floor((seconds % 3600) / 60);
    const secs = seconds % 60;
    if (hours > 0) return `${hours}h ${minutes}m ${secs}s`;
    if (minutes > 0) return `${minutes}m ${secs}s`;
    return `${secs}s`;
  }

  function formatFileSize(size?: number | null): string {
    if (!size || size <= 0) return "Unknown size";
    if (size < 1024) return `${size} B`;
    if (size < 1024 * 1024) return `${(size / 1024).toFixed(1)} KB`;
    return `${(size / (1024 * 1024)).toFixed(1)} MB`;
  }

  function senderLabel(sender: string): string {
    return sender === "Me" ? "You" : "Peer";
  }

  async function loadOverview() {
    if (!chatId) return;
    loadingOverview = true;
    try {
      overview = await api.getChatDetailsOverview(chatId);
      errorMessage = null;
    } catch (e) {
      console.error("Failed to load chat details overview", e);
      errorMessage = "Failed to load chat details.";
    } finally {
      loadingOverview = false;
    }
  }

  async function loadStats() {
    if (!chatId) return;
    loadingStats = true;
    try {
      stats = await api.getChatStats(chatId);
      errorMessage = null;
    } catch (e) {
      console.error("Failed to load chat stats", e);
      errorMessage = "Failed to load chat statistics.";
    } finally {
      loadingStats = false;
    }
  }

  async function loadFiles(reset = false) {
    if (!chatId) return;
    if (loadingFiles) return;

    loadingFiles = true;
    const offset = reset ? 0 : filesOffset;
    const limit = 30;

    try {
      const nextRows = await api.listChatFiles(chatId, filesFilter, limit, offset);
      files = reset ? nextRows : [...files, ...nextRows];
      filesOffset = offset + nextRows.length;
      filesHasMore = nextRows.length === limit;
      errorMessage = null;
    } catch (e) {
      console.error("Failed to load chat files", e);
      errorMessage = "Failed to load file history.";
    } finally {
      loadingFiles = false;
    }
  }

  async function refreshAll() {
    filesOffset = 0;
    await Promise.all([loadOverview(), loadStats(), loadFiles(true)]);
  }

  async function dropConnection() {
    if (!chatId) return;
    actionBusy = true;
    try {
      await api.dropChatConnection(chatId);
      await loadOverview();
    } catch (e) {
      console.error("Failed to drop connection", e);
      errorMessage = "Could not drop connection.";
    } finally {
      actionBusy = false;
    }
  }

  async function forceReconnect() {
    if (!chatId) return;
    actionBusy = true;
    try {
      await api.forceChatReconnect(chatId);
      await loadOverview();
      await loadStats();
    } catch (e) {
      console.error("Failed to force reconnect", e);
      errorMessage = "Could not force reconnect.";
    } finally {
      actionBusy = false;
    }
  }

  function selectFilter(filter: ChatFileFilter) {
    if (filesFilter === filter) return;
    filesFilter = filter;
    filesOffset = 0;
    void loadFiles(true);
  }

  $effect(() => {
    if (!show || !chatId) return;

    if (lastLoadedChat !== chatId) {
      lastLoadedChat = chatId;
      activeTab = "peer";
      filesFilter = "all";
      filesOffset = 0;
      files = [];
      void refreshAll();
    }
  });

  $effect(() => {
    if (!show) return;
    const timer = setInterval(() => {
      nowUnix = Math.floor(Date.now() / 1000);
    }, 1000);

    return () => clearInterval(timer);
  });
</script>

{#if show && chatId}
  <div
    class="fixed inset-0 z-[110] flex items-center justify-center bg-black/55"
    onclick={onclose}
    role="button"
    tabindex="0"
    onkeydown={(e) => {
      if (e.key === "Escape" || e.key === "Enter" || e.key === " ") onclose();
    }}
  >
    <div
      class="w-[min(920px,94vw)] h-[min(700px,88vh)] rounded-2xl border border-theme-base-700 bg-theme-base-900 shadow-2xl flex flex-col overflow-hidden"
      onclick={(e) => e.stopPropagation()}
      onkeydown={(e) => {
        if (e.key === "Escape") onclose();
      }}
      role="dialog"
      aria-modal="true"
      tabindex="0"
    >
      <header class="px-5 py-4 border-b border-theme-base-700 flex items-center justify-between gap-3">
        <div>
          <div class="text-xs uppercase tracking-wide text-theme-base-400">Chat Details</div>
          <div class="text-lg font-semibold text-theme-base-100 truncate">{overview?.peer_name || chatId}</div>
        </div>
        <button
          class="px-3 py-1.5 rounded-lg bg-theme-base-800 hover:bg-theme-base-700 text-theme-base-200"
          onclick={onclose}
        >
          Close
        </button>
      </header>

      <div class="px-4 pt-3 border-b border-theme-base-800 flex gap-2 overflow-x-auto">
        <button
          class={`px-3 py-2 rounded-t-lg text-sm ${activeTab === "peer" ? "bg-theme-base-800 text-theme-base-100" : "text-theme-base-400 hover:text-theme-base-200"}`}
          onclick={() => (activeTab = "peer")}
        >
          Peer Information
        </button>
        <button
          class={`px-3 py-2 rounded-t-lg text-sm ${activeTab === "connection" ? "bg-theme-base-800 text-theme-base-100" : "text-theme-base-400 hover:text-theme-base-200"}`}
          onclick={() => (activeTab = "connection")}
        >
          Connection
        </button>
        <button
          class={`px-3 py-2 rounded-t-lg text-sm ${activeTab === "stats" ? "bg-theme-base-800 text-theme-base-100" : "text-theme-base-400 hover:text-theme-base-200"}`}
          onclick={() => (activeTab = "stats")}
        >
          Stats
        </button>
        <button
          class={`px-3 py-2 rounded-t-lg text-sm ${activeTab === "files" ? "bg-theme-base-800 text-theme-base-100" : "text-theme-base-400 hover:text-theme-base-200"}`}
          onclick={() => {
            activeTab = "files";
            if (files.length === 0) void loadFiles(true);
          }}
        >
          Files
        </button>
      </div>

      {#if errorMessage}
        <div class="px-5 py-2 text-sm text-theme-error-400 border-b border-red-500/20 bg-red-500/5">{errorMessage}</div>
      {/if}

      <section class="flex-1 overflow-auto p-5">
        {#if activeTab === "peer"}
          {#if loadingOverview}
            <p class="text-theme-base-400">Loading peer information...</p>
          {:else if overview}
            <div class="grid gap-4 md:grid-cols-[auto,1fr]">
              <div>
                {#if overview.avatar_url}
                  <img src={overview.avatar_url} alt={overview.peer_name} class="w-24 h-24 rounded-xl object-cover border border-theme-base-700" />
                {:else}
                  <div class="w-24 h-24 rounded-xl bg-theme-base-800 border border-theme-base-700 flex items-center justify-center text-2xl text-theme-base-200">
                    {overview.peer_name.slice(0, 1).toUpperCase()}
                  </div>
                {/if}
              </div>
              <div class="space-y-3 text-sm">
                <div>
                  <div class="text-theme-base-400">Peer Name</div>
                  <div class="text-theme-base-100 text-base">{overview.peer_name}</div>
                </div>
                <div>
                  <div class="text-theme-base-400">Peer Alias</div>
                  <div class="text-theme-base-200">{overview.peer_alias || "-"}</div>
                </div>
                <div>
                  <div class="text-theme-base-400">Peer ID</div>
                  <div class="text-theme-base-200 break-all" title={overview.peer_id}>{toShortPeerId(overview.peer_id)}</div>
                </div>
                <div>
                  <div class="text-theme-base-400">Chat ID</div>
                  <div class="text-theme-base-200 break-all">{overview.chat_id}</div>
                </div>
              </div>
            </div>
          {/if}
        {/if}

        {#if activeTab === "connection"}
          {#if loadingOverview}
            <p class="text-theme-base-400">Loading connection details...</p>
          {:else if overview}
            <div class="space-y-5 text-sm">
              <div class="grid sm:grid-cols-2 gap-4">
                <div class="p-3 rounded-lg bg-theme-base-800 border border-theme-base-700">
                  <div class="text-theme-base-400 mb-1">Status</div>
                  <div class={overview.connection.connected ? "text-theme-success-400" : "text-theme-base-300"}>
                    {overview.connection.connected ? "Connected" : "Disconnected"}
                  </div>
                </div>
                <div class="p-3 rounded-lg bg-theme-base-800 border border-theme-base-700">
                  <div class="text-theme-base-400 mb-1">Connected Duration</div>
                  <div class="text-theme-base-100">
                    {overview.connection.connected
                      ? formatDuration(nowUnix - (overview.connection.connected_since || nowUnix))
                      : "-"}
                  </div>
                </div>
                <div class="p-3 rounded-lg bg-theme-base-800 border border-theme-base-700 sm:col-span-2">
                  <div class="text-theme-base-400 mb-1">Remote Address</div>
                  <div class="text-theme-base-100 break-all">{overview.connection.remote_addr || "-"}</div>
                </div>
                <div class="p-3 rounded-lg bg-theme-base-800 border border-theme-base-700">
                  <div class="text-theme-base-400 mb-1">First Connected</div>
                  <div class="text-theme-base-100">{formatTimestamp(overview.connection.first_connected_at)}</div>
                </div>
                <div class="p-3 rounded-lg bg-theme-base-800 border border-theme-base-700">
                  <div class="text-theme-base-400 mb-1">Last Connected</div>
                  <div class="text-theme-base-100">{formatTimestamp(overview.connection.last_connected_at)}</div>
                </div>
              </div>

              <div class="flex gap-2">
                <button
                  class="px-3 py-2 rounded-lg bg-theme-error-600 hover:bg-theme-error-500 text-white disabled:opacity-60"
                  onclick={dropConnection}
                  disabled={actionBusy || !overview.connection.connected}
                >
                  Drop Connection
                </button>
                <button
                  class="px-3 py-2 rounded-lg bg-theme-primary-600 hover:bg-theme-primary-500 text-white disabled:opacity-60"
                  onclick={forceReconnect}
                  disabled={actionBusy}
                >
                  Force Reconnect
                </button>
              </div>
            </div>
          {/if}
        {/if}

        {#if activeTab === "stats"}
          {#if loadingStats}
            <p class="text-theme-base-400">Loading stats...</p>
          {:else if stats}
            <div class="space-y-4">
              <div class="grid md:grid-cols-3 gap-3">
                <div class="p-3 rounded-lg bg-theme-base-800 border border-theme-base-700">
                  <div class="text-theme-base-400 text-xs">Sent</div>
                  <div class="text-xl text-theme-base-100">{stats.sent_total}</div>
                </div>
                <div class="p-3 rounded-lg bg-theme-base-800 border border-theme-base-700">
                  <div class="text-theme-base-400 text-xs">Received</div>
                  <div class="text-xl text-theme-base-100">{stats.received_total}</div>
                </div>
                <div class="p-3 rounded-lg bg-theme-base-800 border border-theme-base-700">
                  <div class="text-theme-base-400 text-xs">Reconnects</div>
                  <div class="text-xl text-theme-base-100">{stats.reconnect_count}</div>
                </div>
              </div>

              <div class="grid md:grid-cols-2 gap-4">
                <div class="p-3 rounded-lg bg-theme-base-800 border border-theme-base-700">
                  <h4 class="text-theme-base-200 mb-2">Sent by type</h4>
                  <div class="grid grid-cols-2 gap-y-1 text-sm text-theme-base-300">
                    <span>Text</span><span>{stats.sent.text}</span>
                    <span>Sticker</span><span>{stats.sent.sticker}</span>
                    <span>Image</span><span>{stats.sent.image}</span>
                    <span>Video</span><span>{stats.sent.video}</span>
                    <span>Audio</span><span>{stats.sent.audio}</span>
                    <span>Document</span><span>{stats.sent.document}</span>
                  </div>
                </div>
                <div class="p-3 rounded-lg bg-theme-base-800 border border-theme-base-700">
                  <h4 class="text-theme-base-200 mb-2">Received by type</h4>
                  <div class="grid grid-cols-2 gap-y-1 text-sm text-theme-base-300">
                    <span>Text</span><span>{stats.received.text}</span>
                    <span>Sticker</span><span>{stats.received.sticker}</span>
                    <span>Image</span><span>{stats.received.image}</span>
                    <span>Video</span><span>{stats.received.video}</span>
                    <span>Audio</span><span>{stats.received.audio}</span>
                    <span>Document</span><span>{stats.received.document}</span>
                  </div>
                </div>
              </div>
            </div>
          {/if}
        {/if}

        {#if activeTab === "files"}
          <div class="space-y-4">
            <div class="flex flex-wrap gap-2">
              {#each FILE_FILTERS as filter}
                <button
                  class={`px-3 py-1.5 rounded-full text-xs border ${filesFilter === filter ? "bg-theme-primary-600 border-theme-primary-500 text-white" : "bg-theme-base-800 border-theme-base-700 text-theme-base-300 hover:text-theme-base-100"}`}
                  onclick={() => selectFilter(filter)}
                >
                  {filter}
                </button>
              {/each}
            </div>

            {#if loadingFiles && files.length === 0}
              <p class="text-theme-base-400">Loading files...</p>
            {:else if files.length === 0}
              <p class="text-theme-base-400">No files found for this filter.</p>
            {:else}
              <div class="space-y-2">
                {#each files as row}
                  <div class="p-3 rounded-lg border border-theme-base-700 bg-theme-base-800/60">
                    <div class="flex items-start justify-between gap-3">
                      <div>
                        <div class="text-theme-base-100 text-sm">{row.file_name || row.file_hash}</div>
                        <div class="text-xs text-theme-base-400 mt-1">
                          {row.content_type} • {formatFileSize(row.size_bytes)} • {senderLabel(row.sender)}
                        </div>
                      </div>
                      <div class="text-xs text-theme-base-500 whitespace-nowrap">{formatTimestamp(row.timestamp)}</div>
                    </div>
                  </div>
                {/each}
              </div>

              {#if filesHasMore}
                <button
                  class="px-3 py-2 rounded-lg bg-theme-base-800 hover:bg-theme-base-700 text-theme-base-200"
                  onclick={() => loadFiles(false)}
                  disabled={loadingFiles}
                >
                  {loadingFiles ? "Loading..." : "Load More"}
                </button>
              {/if}
            {/if}
          </div>
        {/if}
      </section>
    </div>
  </div>
{/if}
