<script lang="ts">
  import { fade } from "svelte/transition";

  let { show = false, onclose = () => {} } = $props();
</script>

{#if show}
  <div
    class="fixed inset-0 bg-black/70 z-50 flex items-center justify-center p-4 animate-fade-in-up"
    transition:fade={{ duration: 150 }}
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
          onclick={() => onclose()}
          class="px-4 py-2 text-sm text-slate-400 hover:text-white transition-colors"
        >
          Close
        </button>
      </div>
    </div>
  </div>
{/if}
