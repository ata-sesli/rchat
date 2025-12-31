<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { createEventDispatcher, onMount } from "svelte";

  export let fileHash: string;

  const dispatch = createEventDispatcher();
  let videoDataUrl: string | null = null;
  let loading = true;
  let error = false;

  onMount(async () => {
    try {
      videoDataUrl = await invoke<string>("get_video_data", { fileHash });
      loading = false;
    } catch (e) {
      console.error("Failed to load video:", e);
      error = true;
      loading = false;
    }
  });

  function close() {
    dispatch("close");
  }

  function handleBackdropClick(e: MouseEvent) {
    if (e.target === e.currentTarget) {
      close();
    }
  }

  function handleKeydown(e: KeyboardEvent) {
    if (e.key === "Escape") {
      close();
    }
  }
</script>

<svelte:window on:keydown={handleKeydown} />

<!-- Fullscreen overlay -->
<div
  class="fixed inset-0 z-50 flex items-center justify-center bg-black/90 backdrop-blur-md animate-fade-in"
  on:click={handleBackdropClick}
  role="dialog"
  aria-modal="true"
>
  <!-- Close button -->
  <button
    on:click={close}
    class="absolute top-4 right-4 w-10 h-10 rounded-full bg-white/10 hover:bg-white/20 flex items-center justify-center transition-colors z-10"
    aria-label="Close"
  >
    <svg
      class="w-6 h-6 text-white"
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

  <!-- Video container -->
  <div class="relative max-w-[90vw] max-h-[90vh] animate-scale-in">
    {#if loading}
      <div
        class="w-64 h-48 bg-slate-800 rounded-lg flex items-center justify-center"
      >
        <div
          class="animate-spin w-8 h-8 border-2 border-white border-t-transparent rounded-full"
        ></div>
      </div>
    {:else if error}
      <div
        class="w-64 h-48 bg-slate-800 rounded-lg flex items-center justify-center text-slate-400"
      >
        Failed to load video
      </div>
    {:else if videoDataUrl}
      <!-- svelte-ignore a11y_media_has_caption -->
      <video
        src={videoDataUrl}
        controls
        autoplay
        class="max-w-[90vw] max-h-[85vh] rounded-lg shadow-2xl"
      ></video>
    {/if}
  </div>
</div>

<style>
  @keyframes fade-in {
    from {
      opacity: 0;
    }
    to {
      opacity: 1;
    }
  }

  @keyframes scale-in {
    from {
      opacity: 0;
      transform: scale(0.95);
    }
    to {
      opacity: 1;
      transform: scale(1);
    }
  }

  .animate-fade-in {
    animation: fade-in 0.2s ease-out;
  }

  .animate-scale-in {
    animation: scale-in 0.2s ease-out;
  }
</style>
