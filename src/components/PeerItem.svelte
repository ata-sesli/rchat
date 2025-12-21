<script lang="ts">
  import { createEventDispatcher } from "svelte";
  import { fade } from "svelte/transition";

  export let peer: string;
  export let isPinned = false;
  export let isActive = false;
  export let isDragging = false;
  export let userProfile: { alias: string | null; avatar_path: string | null } =
    { alias: null, avatar_path: null };

  const dispatch = createEventDispatcher();

  function handleClick() {
    if (!isDragging) {
      dispatch("click");
    }
  }

  function handleContextMenu(e: MouseEvent) {
    dispatch("contextmenu", e);
  }

  function handlePointerDown(e: PointerEvent) {
    dispatch("pointerdown", e);
  }

  function handlePointerMove(e: PointerEvent) {
    dispatch("pointermove", e);
  }

  function handlePointerUp(e: PointerEvent) {
    dispatch("pointerup", e);
  }

  function handlePointerCancel(e: PointerEvent) {
    dispatch("pointercancel", e);
  }
</script>

<div transition:fade={{ duration: 150 }} class="relative group/item">
  <!-- svelte-ignore a11y-interactive-supports-focus -->
  <!-- svelte-ignore a11y-click-events-have-key-events -->
  <!-- svelte-ignore a11y-no-static-element-interactions -->
  <div
    on:pointerdown={handlePointerDown}
    on:pointermove={handlePointerMove}
    on:pointerup={handlePointerUp}
    on:pointercancel={handlePointerCancel}
    role="button"
    id={`peer-item-${peer}`}
    on:click={handleClick}
    class={`w-full flex items-center gap-3 p-3 rounded-xl cursor-grab transition-all border border-transparent touch-none relative z-10
        ${isActive ? "bg-slate-800/80 border-slate-700/50" : "hover:bg-slate-800/30"}
        ${isDragging ? "opacity-50 cursor-grabbing" : ""}`}
  >
    <div class="relative pointer-events-none">
      {#if peer === "Me"}
        <!-- ME Avatar -->
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
        <!-- GENERAL Icon -->
        <div
          class="w-10 h-10 rounded-full bg-slate-700 flex items-center justify-center text-slate-300 font-medium group-hover:bg-slate-600 shadow-md"
        >
          #
        </div>
      {:else}
        <!-- PEER Avatar -->
        <img
          src={`https://github.com/${peer}.png?size=40`}
          alt={peer}
          class="w-10 h-10 rounded-full bg-slate-800 shadow-md ring-2 ring-transparent group-hover:ring-slate-700 transition-all"
          on:error={(e) =>
            ((e.currentTarget as HTMLImageElement).src =
              "https://github.com/github.png?size=40")}
        />
      {/if}

      <!-- Pin Indicator -->
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
            <path d="M5 4a2 2 0 012-2h6a2 2 0 012 2v14l-5-2.5L5 18V4z" />
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
      <!-- Status/Subtitle -->
      {#if peer === "Me"}
        <p class="text-xs text-slate-500 truncate">Note to self</p>
      {:else if peer === "General"}
        <p class="text-xs text-slate-500 truncate">Public Broadcast</p>
      {:else}
        <p class="text-xs text-slate-400 truncate">Connected</p>
      {/if}
    </div>

    <!-- Context Menu Button -->
    <button
      on:click|stopPropagation={handleContextMenu}
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
