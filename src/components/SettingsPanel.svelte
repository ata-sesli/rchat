<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { onMount, createEventDispatcher } from "svelte";

  const dispatch = createEventDispatcher();

  // Navigation State
  type SettingsView = "menu" | "add-peers" | "set-theme" | "profile" | "about";
  let currentView: SettingsView = "menu";

  // Peer State
  let peers: string[] = [];
  let newPeer = "";
  let isLoading = false;
  let error = "";

  // Profile State
  let alias = "";
  let avatarPath = "";
  let isSavingProfile = false;

  // Accordion State (for Add Peers screen)
  let isAddPeerOpen = false;

  onMount(() => {
    loadPeers();
    loadProfile();
  });

  async function loadPeers() {
    try {
      peers = await invoke<string[]>("get_trusted_peers");
    } catch (e) {
      console.error(e);
      error = "Failed to load peers";
    }
  }

  async function loadProfile() {
    try {
      const profile = await invoke<{
        alias: string | null;
        avatar_path: string | null;
      }>("get_user_profile");
      alias = profile.alias || "";
      avatarPath = profile.avatar_path || "";
    } catch (e) {
      console.error("Failed to load profile", e);
    }
  }

  async function saveProfile() {
    isSavingProfile = true;
    try {
      await invoke("update_user_profile", {
        alias: alias || null,
        avatarPath: avatarPath || null,
      });
      // Dispatch event to notify parent of profile change
      dispatch("profileUpdated", { alias, avatarPath });
    } catch (e: any) {
      error = e.toString();
    } finally {
      isSavingProfile = false;
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
    currentView = "menu";
    error = ""; // Clear errors on nav change
  }
</script>

<div
  class="h-full flex flex-col bg-slate-950 text-slate-200 p-8 overflow-y-auto"
>
  <!-- VIEW: MENU -->
  {#if currentView === "menu"}
    <!-- Header -->
    <div class="mb-8 border-b border-slate-800/50 pb-4">
      <h2 class="text-3xl font-bold text-white tracking-tight">Settings</h2>
      <p class="text-slate-400 mt-1">Manage application preferences.</p>
    </div>

    <div class="space-y-3">
      <!-- Profile Button -->
      <button
        on:click={() => (currentView = "profile")}
        class="w-full flex items-center justify-between p-4 bg-slate-900 border border-slate-800 rounded-xl hover:bg-slate-800 transition-all group shadow-sm hover:shadow-md"
      >
        <div class="flex items-center gap-4">
          <div
            class="p-2.5 rounded-lg bg-blue-500/10 text-blue-400 group-hover:bg-blue-500/20 group-hover:text-blue-300 transition-colors"
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
                d="M16 7a4 4 0 11-8 0 4 4 0 018 0zM12 14a7 7 0 00-7 7h14a7 7 0 00-7-7z"
              />
            </svg>
          </div>
          <div class="text-left">
            <span
              class="block text-lg font-medium text-slate-200 group-hover:text-white transition-colors"
              >Profile</span
            >
            <span class="text-sm text-slate-500">Set alias and photo</span>
          </div>
        </div>
        <svg
          xmlns="http://www.w3.org/2000/svg"
          class="h-5 w-5 text-slate-500 group-hover:translate-x-1 transition-transform"
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

      <!-- Add Peers Button -->
      <button
        on:click={() => (currentView = "add-peers")}
        class="w-full flex items-center justify-between p-4 bg-slate-900 border border-slate-800 rounded-xl hover:bg-slate-800 transition-all group shadow-sm hover:shadow-md"
      >
        <div class="flex items-center gap-4">
          <div
            class="p-2.5 rounded-lg bg-teal-500/10 text-teal-400 group-hover:bg-teal-500/20 group-hover:text-teal-300 transition-colors"
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
                d="M12 4.354a4 4 0 110 5.292M15 21H3v-1a6 6 0 0112 0v1zm0 0h6v-1a6 6 0 00-9-5.197M13 7a4 4 0 11-8 0 4 4 0 018 0z"
              />
            </svg>
          </div>
          <div class="text-left">
            <span
              class="block text-lg font-medium text-slate-200 group-hover:text-white transition-colors"
              >Add Peers</span
            >
            <span class="text-sm text-slate-500"
              >Manage trusted connections</span
            >
          </div>
        </div>
        <svg
          xmlns="http://www.w3.org/2000/svg"
          class="h-5 w-5 text-slate-500 group-hover:translate-x-1 transition-transform"
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

      <!-- Set Theme Button -->
      <button
        on:click={() => (currentView = "set-theme")}
        class="w-full flex items-center justify-between p-4 bg-slate-900 border border-slate-800 rounded-xl hover:bg-slate-800 transition-all group shadow-sm hover:shadow-md"
      >
        <div class="flex items-center gap-4">
          <div
            class="p-2.5 rounded-lg bg-purple-500/10 text-purple-400 group-hover:bg-purple-500/20 group-hover:text-purple-300 transition-colors"
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
                d="M7 21a4 4 0 01-4-4V5a2 2 0 012-2h4a2 2 0 012 2v12a4 4 0 01-4 4zm0 0h12a2 2 0 002-2v-4a2 2 0 00-2-2h-2.343M11 7.343l1.657-1.657a2 2 0 012.828 0l2.829 2.829a2 2 0 010 2.828l-8.486 8.485M7 17h.01"
              />
            </svg>
          </div>
          <div class="text-left">
            <span
              class="block text-lg font-medium text-slate-200 group-hover:text-white transition-colors"
              >Set Theme</span
            >
            <span class="text-sm text-slate-500">Customize appearance</span>
          </div>
        </div>
        <svg
          xmlns="http://www.w3.org/2000/svg"
          class="h-5 w-5 text-slate-500 group-hover:translate-x-1 transition-transform"
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

      <!-- About Button -->
      <button
        on:click={() => (currentView = "about")}
        class="w-full flex items-center justify-between p-4 bg-slate-900 border border-slate-800 rounded-xl hover:bg-slate-800 transition-all group shadow-sm hover:shadow-md"
      >
        <div class="flex items-center gap-4">
          <div
            class="p-2.5 rounded-lg bg-orange-500/10 text-orange-400 group-hover:bg-orange-500/20 group-hover:text-orange-300 transition-colors"
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
                d="M13 16h-1v-4h-1m1-4h.01M21 12a9 9 0 11-18 0 9 9 0 0118 0z"
              />
            </svg>
          </div>
          <div class="text-left">
            <span
              class="block text-lg font-medium text-slate-200 group-hover:text-white transition-colors"
              >About RChat</span
            >
            <span class="text-sm text-slate-500">Version and info</span>
          </div>
        </div>
        <svg
          xmlns="http://www.w3.org/2000/svg"
          class="h-5 w-5 text-slate-500 group-hover:translate-x-1 transition-transform"
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

    <!-- VIEW: PROFILE -->
  {:else if currentView === "profile"}
    <!-- Sub-view Header -->
    <div class="mb-6 flex items-center gap-4 border-b border-slate-800/50 pb-4">
      <button
        on:click={goBack}
        class="p-2 hover:bg-slate-800 rounded-lg text-slate-400 hover:text-white transition-colors"
        aria-label="Go Back"
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
      <h2 class="text-xl font-bold text-white">Profile</h2>
    </div>

    <div class="space-y-6 animate-fade-in-up">
      <div class="space-y-2">
        <label class="block">
          <span class="text-sm font-medium text-slate-400"
            >Alias (Display Name)</span
          >
          <input
            type="text"
            bind:value={alias}
            placeholder="Your visual name"
            class="mt-1 w-full bg-slate-900 border border-slate-700 rounded-lg px-4 py-3 text-slate-200 focus:outline-none focus:border-blue-500 transition-colors"
          />
        </label>
      </div>

      <div class="space-y-2">
        <label class="block">
          <span class="text-sm font-medium text-slate-400"
            >Avatar URL (Optional)</span
          >
          <input
            type="text"
            bind:value={avatarPath}
            placeholder="https://..."
            class="mt-1 w-full bg-slate-900 border border-slate-700 rounded-lg px-4 py-3 text-slate-200 focus:outline-none focus:border-blue-500 transition-colors"
          />
        </label>
        <p class="text-xs text-slate-500">
          Enter a URL for your profile picture.
        </p>
      </div>

      <button
        on:click={saveProfile}
        disabled={isSavingProfile}
        class="w-full py-3 bg-blue-600 hover:bg-blue-500 text-white font-semibold rounded-xl shadow-lg shadow-blue-500/20 transition-all disabled:opacity-50"
      >
        {isSavingProfile ? "Saving..." : "Save Profile"}
      </button>
    </div>

    <!-- VIEW: ADD PEERS -->
  {:else if currentView === "add-peers"}
    <!-- Sub-view Header -->
    <div class="mb-6 flex items-center gap-4 border-b border-slate-800/50 pb-4">
      <button
        on:click={goBack}
        class="p-2 hover:bg-slate-800 rounded-lg text-slate-400 hover:text-white transition-colors"
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
      <h2 class="text-xl font-bold text-white">Add Peers</h2>
    </div>

    <!-- Peer Management Section (Reused Code) -->
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
              class="p-2 rounded-lg bg-teal-500/10 text-teal-400 group-hover:bg-teal-500/20 group-hover:text-teal-300 transition-colors"
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
              <span class="block font-medium text-slate-200">Add New Peer</span>
              <span class="text-xs text-slate-500"
                >Connect with a GitHub user</span
              >
            </div>
          </div>

          <svg
            xmlns="http://www.w3.org/2000/svg"
            class={`h-5 w-5 text-slate-500 transition-transform duration-300 ${isAddPeerOpen ? "rotate-180" : ""}`}
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
                  <span class="text-slate-500">@</span>
                </div>
                <input
                  type="text"
                  bind:value={newPeer}
                  placeholder="github_username"
                  class="w-full pl-8 pr-4 py-2.5 bg-slate-900 border border-slate-700 rounded-lg text-slate-200 focus:outline-none focus:border-teal-500 focus:ring-1 focus:ring-teal-500 transition-all placeholder:text-slate-600"
                  on:keydown={(e) => e.key === "Enter" && addPeer()}
                />
              </div>
              <button
                on:click={addPeer}
                disabled={isLoading || !newPeer}
                class="px-6 py-2.5 bg-teal-600 hover:bg-teal-500 text-slate-950 font-semibold rounded-lg shadow-lg shadow-teal-500/20 transition-all disabled:opacity-50 disabled:cursor-not-allowed whitespace-nowrap"
              >
                {isLoading ? "Adding..." : "Add Peer"}
              </button>
            </div>
            {#if error}
              <p class="mt-2 text-sm text-red-400">{error}</p>
            {/if}
          </div>
        {/if}
      </div>

      <!-- Peer List -->
      <div
        class="bg-slate-900/50 rounded-xl border border-slate-700/50 shadow-sm mt-4 p-4"
      >
        <h4
          class="text-sm font-semibold text-slate-400 uppercase tracking-wider mb-3 px-1"
        >
          Trusted Users ({peers.length})
        </h4>

        <div class="space-y-2">
          {#if peers.length === 0}
            <div
              class="p-8 text-center border-2 border-dashed border-slate-800 rounded-lg"
            >
              <p class="text-slate-500">No trusted peers yet.</p>
              <p class="text-xs text-slate-600 mt-1">
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
                  class="w-8 h-8 rounded-full bg-slate-800 ring-2 ring-slate-800"
                  on:error={(e) =>
                    ((e.currentTarget as HTMLImageElement).src =
                      "https://github.com/github.png?size=40")}
                />
                <span class="text-slate-200 font-medium">{peer}</span>
              </div>
              <button
                on:click={() => removePeer(peer)}
                class="p-2 text-slate-500 hover:text-red-400 hover:bg-red-950/30 rounded-lg transition-all opacity-0 group-hover:opacity-100"
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

    <!-- VIEW: SET THEME -->
  {:else if currentView === "set-theme"}
    <!-- Sub-view Header -->
    <div class="mb-6 flex items-center gap-4 border-b border-slate-800/50 pb-4">
      <button
        on:click={goBack}
        class="p-2 hover:bg-slate-800 rounded-lg text-slate-400 hover:text-white transition-colors"
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
      <h2 class="text-xl font-bold text-white">Set Theme</h2>
    </div>

    <div
      class="p-8 text-center bg-slate-900/50 rounded-xl border border-slate-800 border-dashed animate-fade-in-up"
    >
      <div
        class="inline-flex p-4 rounded-full bg-purple-500/10 text-purple-400 mb-4"
      >
        <svg
          xmlns="http://www.w3.org/2000/svg"
          class="h-8 w-8"
          fill="none"
          viewBox="0 0 24 24"
          stroke="currentColor"
        >
          <path
            stroke-linecap="round"
            stroke-linejoin="round"
            stroke-width="2"
            d="M7 21a4 4 0 01-4-4V5a2 2 0 012-2h4a2 2 0 012 2v12a4 4 0 01-4 4zm0 0h12a2 2 0 002-2v-4a2 2 0 00-2-2h-2.343M11 7.343l1.657-1.657a2 2 0 012.828 0l2.829 2.829a2 2 0 010 2.828l-8.486 8.485M7 17h.01"
          />
        </svg>
      </div>
      <h3 class="text-lg font-medium text-white">Theme Customization</h3>
      <p class="text-slate-500 mt-2 max-w-sm mx-auto">
        This feature is coming soon! You will be able to switch between Light,
        Dark, and System themes.
      </p>
    </div>

    <!-- VIEW: ABOUT -->
  {:else if currentView === "about"}
    <!-- Sub-view Header -->
    <div class="mb-6 flex items-center gap-4 border-b border-slate-800/50 pb-4">
      <button
        on:click={goBack}
        class="p-2 hover:bg-slate-800 rounded-lg text-slate-400 hover:text-white transition-colors"
        aria-label="Go Back"
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
      <h2 class="text-xl font-bold text-white">About RChat</h2>
    </div>

    <div class="space-y-6 animate-fade-in-up">
      <!-- Logo -->
      <div class="flex flex-col items-center text-center py-6">
        <img
          src="/logo.svg"
          alt="RChat"
          class="w-20 h-20 rounded-2xl shadow-xl mb-4"
        />
        <h3 class="text-2xl font-bold text-white">RChat</h3>
        <p class="text-slate-400 mt-1">Version 0.1.0</p>
      </div>

      <!-- Info Cards -->
      <div class="space-y-3">
        <div class="p-4 bg-slate-900 border border-slate-800 rounded-xl">
          <h4 class="font-medium text-slate-200 mb-1">About</h4>
          <p class="text-sm text-slate-400">
            RChat is a decentralized, peer-to-peer chat application built with
            privacy and security in mind. Your messages stay yours.
          </p>
        </div>

        <div class="p-4 bg-slate-900 border border-slate-800 rounded-xl">
          <h4 class="font-medium text-slate-200 mb-1">Technology</h4>
          <p class="text-sm text-slate-400">
            Built with Tauri, SvelteKit, Rust, and libp2p for secure
            peer-to-peer communication.
          </p>
        </div>

        <div class="p-4 bg-slate-900 border border-slate-800 rounded-xl">
          <h4 class="font-medium text-slate-200 mb-1">License</h4>
          <p class="text-sm text-slate-400">
            Open source software. MIT License.
          </p>
        </div>
      </div>
    </div>
  {/if}
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
