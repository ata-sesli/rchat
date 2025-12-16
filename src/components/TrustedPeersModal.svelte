<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { onMount, createEventDispatcher } from "svelte";

  export let show = false;
  const dispatch = createEventDispatcher();

  let peers: string[] = [];
  let newPeer = "";
  let isLoading = false;
  let error = "";

  onMount(() => {
    if (show) loadPeers();
  });

  $: if (show) loadPeers();

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

  function close() {
    dispatch("close");
  }
</script>

{#if show}
  <div
    class="fixed inset-0 z-50 flex items-center justify-center p-4 bg-black/50 backdrop-blur-sm"
  >
    <div
      class="bg-gray-800 rounded-xl shadow-2xl w-full max-w-md border border-gray-700 overflow-hidden"
    >
      <!-- Header -->
      <div
        class="flex items-center justify-between px-6 py-4 border-b border-gray-700 bg-gray-800"
      >
        <h3 class="text-xl font-bold text-white">Trusted Peers</h3>
        <button
          on:click={close}
          class="text-gray-400 hover:text-white transition"
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
              d="M6 18L18 6M6 6l12 12"
            />
          </svg>
        </button>
      </div>

      <!-- Body -->
      <div class="p-6 space-y-6">
        <!-- Add Peer Form -->
        <div class="space-y-2">
          <label class="text-sm font-medium text-gray-300"
            >Add GitHub User</label
          >
          <div class="flex gap-2">
            <input
              type="text"
              bind:value={newPeer}
              placeholder="username"
              class="flex-1 bg-gray-700 border border-gray-600 rounded-lg px-4 py-2 text-white placeholder-gray-400 focus:outline-none focus:ring-2 focus:ring-blue-500"
              on:keydown={(e) => e.key === "Enter" && addPeer()}
            />
            <button
              on:click={addPeer}
              disabled={isLoading || !newPeer}
              class="bg-blue-600 hover:bg-blue-700 disabled:opacity-50 disabled:cursor-not-allowed text-white px-4 py-2 rounded-lg font-medium transition"
            >
              Add
            </button>
          </div>
          {#if error}
            <p class="text-red-400 text-sm">{error}</p>
          {/if}
        </div>

        <!-- Peers List -->
        <div class="space-y-2">
          <label class="text-sm font-medium text-gray-300"
            >Trusted Users ({peers.length})</label
          >
          <div
            class="max-h-60 overflow-y-auto space-y-2 rounded-lg bg-gray-900/50 p-2"
          >
            {#if peers.length === 0}
              <p class="text-gray-500 text-sm text-center py-4">
                No trusted peers yet.
              </p>
            {/if}
            {#each peers as peer}
              <div
                class="flex items-center justify-between bg-gray-800 p-3 rounded-md border border-gray-700 group"
              >
                <div class="flex items-center gap-3">
                  <img
                    src={`https://github.com/${peer}.png?size=40`}
                    alt={peer}
                    class="w-8 h-8 rounded-full bg-gray-700"
                    on:error={(e) =>
                      ((e.currentTarget as HTMLImageElement).src =
                        "https://github.com/github.png?size=40")}
                  />
                  <span class="text-white font-medium">{peer}</span>
                </div>
                <button
                  on:click={() => removePeer(peer)}
                  class="text-gray-500 hover:text-red-400 opacity-0 group-hover:opacity-100 transition"
                  title="Remove"
                >
                  <svg
                    xmlns="http://www.w3.org/2000/svg"
                    class="h-5 w-5"
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
    </div>
  </div>
{/if}
