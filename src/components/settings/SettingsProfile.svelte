<script lang="ts">
  import { onMount } from "svelte";
  import { invoke } from "@tauri-apps/api/core";

  let {
    onprofileUpdated = (data: { alias: string; avatarPath: string }) => {},
    onback = () => {},
  } = $props();

  let alias = "";
  let avatarPath = "";
  let isSavingProfile = false;
  let error = "";

  onMount(loadProfile);

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
      onprofileUpdated({ alias, avatarPath });
      // Dispatch a custom event so the layout can refresh data
      window.dispatchEvent(new CustomEvent("profile-updated"));
    } catch (e: any) {
      error = e.toString();
    } finally {
      isSavingProfile = false;
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
    <p class="text-xs text-slate-500">Enter a URL for your profile picture.</p>
  </div>

  {#if error}
    <p class="text-sm text-red-400">{error}</p>
  {/if}

  <button
    on:click={saveProfile}
    disabled={isSavingProfile}
    class="w-full py-3 bg-blue-600 hover:bg-blue-500 text-white font-semibold rounded-xl shadow-lg shadow-blue-500/20 transition-all disabled:opacity-50"
  >
    {isSavingProfile ? "Saving..." : "Save Profile"}
  </button>
</div>

<style>
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
