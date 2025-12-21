<script lang="ts">
  let {
    show = false,
    position = { x: 0, y: 0 },
    target = null as { type: "peer" | "envelope"; id: string } | null,
    pinnedPeers = [] as string[],
    currentEnvelope = null as string | null,
    onaction = (action: string) => {},
  } = $props();

  function handleAction(action: string) {
    onaction(action);
  }
</script>

{#if show}
  <div
    class="fixed z-[100] bg-slate-800 border border-slate-700 rounded-lg shadow-xl py-1 min-w-[140px] animate-fade-in-up"
    style="top: {position.y}px; left: {position.x}px;"
  >
    {#if target?.type === "peer"}
      <button
        onclick={() => handleAction("pin")}
        class="w-full text-left px-4 py-2 text-sm text-slate-300 hover:bg-slate-700 hover:text-white transition-colors"
      >
        {pinnedPeers.includes(target.id) ? "Unpin" : "Pin"}
      </button>
      {#if currentEnvelope}
        <button
          onclick={() => handleAction("remove")}
          class="w-full text-left px-4 py-2 text-sm text-red-400 hover:bg-red-500/10 transition-colors"
        >
          Remove from Envelope
        </button>
      {/if}
      <button
        onclick={() => handleAction("info")}
        class="w-full text-left px-4 py-2 text-sm text-slate-300 hover:bg-slate-700 hover:text-white transition-colors"
      >
        Info
      </button>
      <div class="h-px bg-slate-700/50 my-1 mx-2"></div>
      <button
        onclick={() => handleAction("delete-peer")}
        class="w-full text-left px-4 py-2 text-sm text-red-400 hover:bg-red-500/10 transition-colors"
      >
        Delete
      </button>
    {:else if target?.type === "envelope"}
      <button
        onclick={() => handleAction("edit")}
        class="w-full text-left px-4 py-2 text-sm text-slate-300 hover:bg-slate-700 hover:text-white transition-colors"
      >
        Edit
      </button>
      <button
        onclick={() => handleAction("delete")}
        class="w-full text-left px-4 py-2 text-sm text-red-400 hover:bg-red-500/10 transition-colors"
      >
        Delete
      </button>
      <div class="h-px bg-slate-700/50 my-1 mx-2"></div>
      <button
        onclick={() => handleAction("more")}
        class="w-full text-left px-4 py-2 text-sm text-slate-400 hover:bg-slate-700 hover:text-white transition-colors"
      >
        More...
      </button>
    {/if}
  </div>
{/if}
