<script lang="ts">
  let {
    show = false,
    step = $bindable(
      "select-network" as "select-network" | "local-scan" | "online"
    ),
    localPeers = [] as { peer_id: string; addresses: string[] }[],
    onclose = () => {},
    onconnect = (peerId: string) => {},
  } = $props();

  function handleClose() {
    onclose();
  }

  function setStep(newStep: "select-network" | "local-scan" | "online") {
    step = newStep;
  }

  function handleConnect(peerId: string) {
    onconnect(peerId);
  }
</script>

{#if show}
  <div
    class="fixed inset-0 bg-black/70 z-50 flex items-center justify-center p-4 animate-fade-in-up"
    on:click|self={handleClose}
  >
    <div
      class="bg-slate-900 border border-slate-700 p-6 rounded-2xl w-full max-w-md shadow-2xl space-y-4"
    >
      <!-- Header with Back Button -->
      <div class="flex items-center gap-3">
        {#if step !== "select-network"}
          <button
            on:click={() => setStep("select-network")}
            class="p-1 rounded-lg hover:bg-slate-800 text-slate-400 hover:text-white transition-colors"
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
        {/if}
        <h3 class="text-xl font-bold text-white">
          {#if step === "select-network"}
            Add New Person
          {:else if step === "local-scan"}
            Local Network
          {:else}
            Online Discovery
          {/if}
        </h3>
      </div>

      <!-- Step 1: Network Selection -->
      {#if step === "select-network"}
        <p class="text-sm text-slate-400">
          Choose how to find people to connect with.
        </p>
        <div class="space-y-3">
          <button
            on:click={() => setStep("local-scan")}
            class="w-full flex items-center gap-4 p-4 bg-slate-800/50 hover:bg-slate-800 border border-slate-700 rounded-xl transition-colors text-left group"
          >
            <div
              class="w-12 h-12 rounded-xl bg-teal-500/10 text-teal-400 flex items-center justify-center group-hover:bg-teal-500/20 transition-colors"
            >
              <svg
                xmlns="http://www.w3.org/2000/svg"
                class="h-6 w-6"
                viewBox="0 0 20 20"
                fill="currentColor"
              >
                <path
                  fill-rule="evenodd"
                  d="M17.778 8.222c-4.296-4.296-11.26-4.296-15.556 0A1 1 0 01.808 6.808c5.076-5.077 13.308-5.077 18.384 0a1 1 0 01-1.414 1.414zM14.95 11.05a7 7 0 00-9.9 0 1 1 0 01-1.414-1.414 9 9 0 0112.728 0 1 1 0 01-1.414 1.414zM12.12 13.88a3 3 0 00-4.242 0 1 1 0 01-1.415-1.415 5 5 0 017.072 0 1 1 0 01-1.415 1.415zM9 16a1 1 0 011-1h.01a1 1 0 110 2H10a1 1 0 01-1-1z"
                  clip-rule="evenodd"
                />
              </svg>
            </div>
            <div class="flex-1">
              <div class="font-semibold text-white">Local Network (Wi-Fi)</div>
              <div class="text-sm text-slate-400">
                Find RChat users on the same network
              </div>
            </div>
            <svg
              xmlns="http://www.w3.org/2000/svg"
              class="h-5 w-5 text-slate-500"
              viewBox="0 0 20 20"
              fill="currentColor"
            >
              <path
                fill-rule="evenodd"
                d="M7.293 14.707a1 1 0 010-1.414L10.586 10 7.293 6.707a1 1 0 011.414-1.414l4 4a1 1 0 010 1.414l-4 4a1 1 0 01-1.414 0z"
                clip-rule="evenodd"
              />
            </svg>
          </button>

          <button
            on:click={() => setStep("online")}
            class="w-full flex items-center gap-4 p-4 bg-slate-800/50 hover:bg-slate-800 border border-slate-700 rounded-xl transition-colors text-left group"
          >
            <div
              class="w-12 h-12 rounded-xl bg-purple-500/10 text-purple-400 flex items-center justify-center group-hover:bg-purple-500/20 transition-colors"
            >
              <svg
                xmlns="http://www.w3.org/2000/svg"
                class="h-6 w-6"
                viewBox="0 0 20 20"
                fill="currentColor"
              >
                <path
                  fill-rule="evenodd"
                  d="M4.083 9h1.946c.089-1.546.383-2.97.837-4.118A6.004 6.004 0 004.083 9zM10 2a8 8 0 100 16 8 8 0 000-16zm0 2c-.076 0-.232.032-.465.262-.238.234-.497.623-.737 1.182-.389.907-.673 2.142-.766 3.556h3.936c-.093-1.414-.377-2.649-.766-3.556-.24-.56-.5-.948-.737-1.182C10.232 4.032 10.076 4 10 4zm3.971 5c-.089-1.546-.383-2.97-.837-4.118A6.004 6.004 0 0115.917 9h-1.946zm-2.003 2H8.032c.093 1.414.377 2.649.766 3.556.24.56.5.948.737 1.182.233.23.389.262.465.262.076 0 .232-.032.465-.262.238-.234.498-.623.737-1.182.389-.907.673-2.142.766-3.556zm1.166 4.118c.454-1.147.748-2.572.837-4.118h1.946a6.004 6.004 0 01-2.783 4.118zm-6.268 0C6.412 13.97 6.118 12.546 6.03 11H4.083a6.004 6.004 0 002.783 4.118z"
                  clip-rule="evenodd"
                />
              </svg>
            </div>
            <div class="flex-1">
              <div class="font-semibold text-white">Online (GitHub)</div>
              <div class="text-sm text-slate-400">
                Connect with anyone globally
              </div>
            </div>
            <svg
              xmlns="http://www.w3.org/2000/svg"
              class="h-5 w-5 text-slate-500"
              viewBox="0 0 20 20"
              fill="currentColor"
            >
              <path
                fill-rule="evenodd"
                d="M7.293 14.707a1 1 0 010-1.414L10.586 10 7.293 6.707a1 1 0 011.414-1.414l4 4a1 1 0 010 1.414l-4 4a1 1 0 01-1.414 0z"
                clip-rule="evenodd"
              />
            </svg>
          </button>
        </div>
      {/if}

      <!-- Step 2a: Local Network Scan -->
      {#if step === "local-scan"}
        <div class="space-y-4">
          <!-- Scanning Indicator -->
          <div
            class="flex items-center gap-3 p-3 bg-teal-500/10 border border-teal-500/20 rounded-lg text-teal-400"
          >
            <div class="relative">
              <div
                class="w-3 h-3 bg-teal-400 rounded-full animate-ping absolute"
              ></div>
              <div class="w-3 h-3 bg-teal-400 rounded-full"></div>
            </div>
            <span class="text-sm">Scanning local network...</span>
          </div>

          <!-- Peers List -->
          <div class="space-y-2 max-h-64 overflow-y-auto">
            {#if localPeers.length === 0}
              <div class="text-center py-8 text-slate-500">
                <svg
                  xmlns="http://www.w3.org/2000/svg"
                  class="h-12 w-12 mx-auto mb-3 opacity-50"
                  fill="none"
                  viewBox="0 0 24 24"
                  stroke="currentColor"
                >
                  <path
                    stroke-linecap="round"
                    stroke-linejoin="round"
                    stroke-width="1.5"
                    d="M17.982 18.725A7.488 7.488 0 0012 15.75a7.488 7.488 0 00-5.982 2.975m11.963 0a9 9 0 10-11.963 0m11.963 0A8.966 8.966 0 0112 21a8.966 8.966 0 01-5.982-2.275M15 9.75a3 3 0 11-6 0 3 3 0 016 0z"
                  />
                </svg>
                <p class="text-sm">No peers found yet</p>
                <p class="text-xs text-slate-600 mt-1">
                  Make sure RChat is running on other devices
                </p>
              </div>
            {:else}
              {#each localPeers as peer}
                <div
                  class="flex items-center gap-3 p-3 bg-slate-800/50 rounded-lg border border-slate-700"
                >
                  <div
                    class="w-10 h-10 rounded-full bg-gradient-to-br from-teal-400 to-cyan-500 flex items-center justify-center text-white font-bold text-sm"
                  >
                    {peer.peer_id.slice(-4).toUpperCase()}
                  </div>
                  <div class="flex-1 min-w-0">
                    <div class="text-sm font-medium text-white truncate">
                      Peer {peer.peer_id.slice(-8)}
                    </div>
                    <div class="text-xs text-slate-500 truncate">
                      {peer.addresses[0] || "No address"}
                    </div>
                  </div>
                  <button
                    on:click={() => handleConnect(peer.peer_id)}
                    class="px-3 py-1.5 bg-teal-600 hover:bg-teal-500 text-white text-sm rounded-lg font-medium transition-colors"
                  >
                    Connect
                  </button>
                </div>
              {/each}
            {/if}
          </div>
        </div>
      {/if}

      <!-- Step 2b: Online (Placeholder) -->
      {#if step === "online"}
        <div class="text-center py-8">
          <div
            class="w-16 h-16 mx-auto mb-4 rounded-full bg-purple-500/10 flex items-center justify-center"
          >
            <svg
              xmlns="http://www.w3.org/2000/svg"
              class="h-8 w-8 text-purple-400"
              viewBox="0 0 20 20"
              fill="currentColor"
            >
              <path
                fill-rule="evenodd"
                d="M4.083 9h1.946c.089-1.546.383-2.97.837-4.118A6.004 6.004 0 004.083 9zM10 2a8 8 0 100 16 8 8 0 000-16z"
                clip-rule="evenodd"
              />
            </svg>
          </div>
          <h4 class="font-semibold text-white mb-2">Coming Soon</h4>
          <p class="text-sm text-slate-400">
            Online discovery via GitHub will be available in a future update.
          </p>
        </div>
      {/if}

      <!-- Footer -->
      <div class="flex justify-end gap-2 pt-2 border-t border-slate-800">
        <button
          on:click={handleClose}
          class="px-4 py-2 text-sm text-slate-400 hover:text-white transition-colors"
        >
          Close
        </button>
      </div>
    </div>
  </div>
{/if}
