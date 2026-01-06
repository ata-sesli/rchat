<script lang="ts">
  import { onMount } from "svelte";
  import { invoke } from "@tauri-apps/api/core";

  let { onback = () => {} } = $props();

  let peers: string[] = [];
  let newPeer = "";
  let isLoading = false;
  let error = "";
  let isAddPeerOpen = false;

  onMount(loadPeers);

  async function loadPeers() {
    try {
      peers = await invoke<string[]>("get_trusted_peers");
    } catch (e) {
      console.error(e);
      error = "Failed to load peers";
    }
  }

  async function addPeer() {
    if (!newPeer.trim()) return;
    isLoading = true;
    error = "";
    try {
      await invoke("add_trusted_peer", { username: newPeer.trim() });
      await loadPeers();
      newPeer = "";
      isAddPeerOpen = false;
    } catch (e: any) {
      error = e.toString();
    } finally {
      isLoading = false;
    }
  }

  async function removePeer(username: string) {
    if (!confirm(`Remove ${username}?`)) return;
    try {
      await invoke("remove_trusted_peer", { username });
      await loadPeers();
    } catch (e: any) {
      error = e.toString();
    }
  }

  function goBack() {
    onback();
  }
</script>

<!-- Sub-view Header -->
<div class="mb-6 flex items-center gap-4 border-b border-slate-800/50 pb-4">
  <button
    on:click={goBack}
    class="p-2 hover:bg-theme-base-800 rounded-lg text-theme-base-400 hover:text-white transition-colors"
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
  <h2 class="text-xl font-bold text-theme-base-100">Add Peers</h2>
</div>

<!-- Peer Management Section -->
<div class="space-y-4 animate-fade-in-up">
  <div
    class="bg-slate-900/50 rounded-xl border border-slate-700/50 overflow-hidden shadow-sm"
  >
    <!-- Accordion Header: Add Peer -->
    <button
      on:click={() => (isAddPeerOpen = !isAddPeerOpen)}
      class="w-full flex items-center justify-between p-4 bg-slate-800/20 hover:bg-slate-800/40 transition-colors text-left group"
    >
      <div class="flex items-center gap-3">
        <div
          class="p-2 rounded-lg bg-teal-500/10 text-theme-primary-400 group-hover:bg-teal-500/20 group-hover:text-theme-primary-300 transition-colors"
        >
          <svg
            xmlns="http://www.w3.org/2000/svg"
            class="h-5 w-5"
            viewBox="0 0 20 20"
            fill="currentColor"
          >
            <path
              fill-rule="evenodd"
              d="M10 3a1 1 0 011 1v5h5a1 1 0 110 2h-5v5a1 1 0 11-2 0v-5H4a1 1 0 110-2h5V4a1 1 0 011-1z"
              clip-rule="evenodd"
            />
          </svg>
        </div>
        <div>
          <span class="block font-medium text-theme-base-200">Add New Peer</span
          >
          <span class="text-xs text-theme-base-500"
            >Connect with a GitHub user</span
          >
        </div>
      </div>

      <svg
        xmlns="http://www.w3.org/2000/svg"
        class={`h-5 w-5 text-theme-base-500 transition-transform duration-300 ${isAddPeerOpen ? "rotate-180" : ""}`}
        viewBox="0 0 20 20"
        fill="currentColor"
      >
        <path
          fill-rule="evenodd"
          d="M5.293 7.293a1 1 0 011.414 0L10 10.586l3.293-3.293a1 1 0 111.414 1.414l-4 4a1 1 0 01-1.414 0l-4-4a1 1 0 010-1.414z"
          clip-rule="evenodd"
        />
      </svg>
    </button>

    <!-- Accordion Body -->
    {#if isAddPeerOpen}
      <div
        class="p-4 bg-slate-950/30 border-t border-slate-700/50 animate-slide-down"
      >
        <div class="flex flex-col sm:flex-row gap-3">
          <div class="flex-1 relative">
            <div
              class="absolute inset-y-0 left-0 pl-3 flex items-center pointer-events-none"
            >
              <span class="text-theme-base-500">@</span>
            </div>
            <input
              type="text"
              bind:value={newPeer}
              placeholder="github_username"
              class="w-full pl-8 pr-4 py-2.5 bg-theme-base-900 border border-theme-base-700 rounded-lg text-theme-base-200 focus:outline-none focus:border-theme-primary-500 focus:ring-1 focus:ring-teal-500 transition-all placeholder:text-theme-base-600"
              on:keydown={(e) => e.key === "Enter" && addPeer()}
            />
          </div>
          <button
            on:click={addPeer}
            disabled={isLoading || !newPeer}
            class="px-6 py-2.5 bg-theme-primary-600 hover:bg-theme-primary-500 text-theme-base-950 font-semibold rounded-lg shadow-lg shadow-teal-500/20 transition-all disabled:opacity-50 disabled:cursor-not-allowed whitespace-nowrap"
          >
            {isLoading ? "Adding..." : "Add Peer"}
          </button>
        </div>
        {#if error}
          <p class="mt-2 text-sm text-theme-error-400">{error}</p>
        {/if}
      </div>
    {/if}
  </div>

  <!-- Peer List -->
  <div
    class="bg-slate-900/50 rounded-xl border border-slate-700/50 shadow-sm mt-4 p-4"
  >
    <h4
      class="text-sm font-semibold text-theme-base-400 uppercase tracking-wider mb-3 px-1"
    >
      Trusted Users ({peers.length})
    </h4>

    <div class="space-y-2">
      {#if peers.length === 0}
        <div
          class="p-8 text-center border-2 border-dashed border-theme-base-800 rounded-lg"
        >
          <p class="text-theme-base-500">No trusted peers yet.</p>
          <p class="text-xs text-theme-base-600 mt-1">
            Add a GitHub username above to start chatting securely.
          </p>
        </div>
      {/if}

      {#each peers as peer}
        <div
          class="flex items-center justify-between p-3 bg-slate-800/40 hover:bg-slate-800/60 rounded-lg border border-slate-700/30 transition-colors group"
        >
          <div class="flex items-center gap-3">
            <img
              src={`https://github.com/${peer}.png?size=40`}
              alt={peer}
              class="w-8 h-8 rounded-full bg-theme-base-800 ring-2 ring-slate-800"
              on:error={(e) =>
                ((e.currentTarget as HTMLImageElement).src =
                  "https://github.com/github.png?size=40")}
            />
            <span class="text-theme-base-200 font-medium">{peer}</span>
          </div>
          <button
            on:click={() => removePeer(peer)}
            class="p-2 text-theme-base-500 hover:text-theme-error-400 hover:bg-red-950/30 rounded-lg transition-all opacity-0 group-hover:opacity-100"
            title="Remove"
          >
            <svg
              xmlns="http://www.w3.org/2000/svg"
              class="h-4 w-4"
              viewBox="0 0 20 20"
              fill="currentColor"
            >
              <path
                fill-rule="evenodd"
                d="M9 2a1 1 0 00-.894.553L7.382 4H4a1 1 0 000 2v10a2 2 0 002 2h8a2 2 0 002-2V6a1 1 0 100-2h-3.382l-.724-1.447A1 1 0 0011 2H9zM7 8a1 1 0 012 0v6a1 1 0 11-2 0V8zm5-1a1 1 0 00-1 1v6a1 1 0 102 0V8a1 1 0 00-1-1z"
                clip-rule="evenodd"
              />
            </svg>
          </button>
        </div>
      {/each}
    </div>
  </div>
</div>

<style>
  @keyframes slide-down {
    from {
      opacity: 0;
      transform: translateY(-10px);
      max-height: 0;
    }
    to {
      opacity: 1;
      transform: translateY(0);
      max-height: 200px;
    }
  }
  .animate-slide-down {
    animation: slide-down 0.2s ease-out forwards;
  }

  @keyframes fade-in-up {
    from {
      opacity: 0;
      transform: translateY(10px);
    }
    to {
      opacity: 1;
      transform: translateY(0);
    }
  }
  .animate-fade-in-up {
    animation: fade-in-up 0.3s ease-out forwards;
  }
</style>
