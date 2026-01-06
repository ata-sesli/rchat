<script lang="ts">
  import { fade } from "svelte/transition";

  let {
    envelope,
    isDropTarget = false,
    onclick = () => {},
    oncontextmenu = (e: MouseEvent) => {},
  }: {
    envelope: { id: string; name: string; icon?: string };
    isDropTarget?: boolean;
    onclick?: () => void;
    oncontextmenu?: (e: MouseEvent) => void;
  } = $props();

  function handleClick() {
    onclick();
  }

  function handleContextMenu(e: MouseEvent) {
    e.stopPropagation();
    oncontextmenu(e);
  }
</script>

<div transition:fade={{ duration: 150 }}>
  <!-- svelte-ignore a11y-interactive-supports-focus -->
  <!-- svelte-ignore a11y-click-events-have-key-events -->
  <!-- svelte-ignore a11y-no-static-element-interactions -->
  <div
    id={`envelope-drop-zone-${envelope.id}`}
    role="button"
    onclick={handleClick}
    class={`w-full flex items-center gap-3 p-3 rounded-xl cursor-pointer transition-all group border border-dashed text-left relative z-0 select-none
       ${isDropTarget ? "bg-teal-900/40 border-theme-primary-500 scale-[1.02] shadow-lg shadow-teal-500/10" : "border-slate-800/50 hover:bg-slate-800/50"}`}
  >
    <div
      class={`w-10 h-10 rounded-xl flex items-center justify-center shrink-0 transition-colors pointer-events-none
        ${isDropTarget ? "bg-teal-500/20 text-theme-primary-400" : "bg-amber-500/10 text-orange-400"}`}
    >
      {#if envelope.icon}
        {@html envelope.icon}
      {:else}
        <svg
          xmlns="http://www.w3.org/2000/svg"
          class="h-5 w-5 pointer-events-none"
          viewBox="0 0 20 20"
          fill="currentColor"
        >
          <path
            d="M2 6a2 2 0 012-2h5l2 2h5a2 2 0 012 2v6a2 2 0 01-2 2H4a2 2 0 01-2-2V6z"
          />
        </svg>
      {/if}
    </div>
    <div
      class="flex flex-col truncate text-left flex-1 min-w-0 pointer-events-none"
    >
      <span
        class="font-bold text-theme-base-300 group-hover:text-white transition-colors truncate"
        >{envelope.name}</span
      >
      <span class="text-[10px] text-theme-base-500 uppercase tracking-wider"
        >Envelope</span
      >
    </div>

    <!-- Context Menu Button -->
    <button
      onclick={handleContextMenu}
      class="p-1 rounded-lg text-theme-base-500 hover:text-white hover:bg-slate-900/50 transition-all opacity-0 group-hover:opacity-100"
    >
      <svg
        xmlns="http://www.w3.org/2000/svg"
        class="h-4 w-4"
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
