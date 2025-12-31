<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { save } from "@tauri-apps/plugin-dialog";
  import { createEventDispatcher } from "svelte";

  export let imageDataUrl: string;
  export let fileHash: string;

  const dispatch = createEventDispatcher();

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

  async function saveImage() {
    try {
      // Open save dialog
      const filePath = await save({
        defaultPath: `image-${fileHash.substring(0, 8)}.png`,
        filters: [
          { name: "Images", extensions: ["png", "jpg", "jpeg", "webp"] },
        ],
      });

      if (filePath) {
        // Call backend to save the file
        await invoke("save_image_to_file", {
          fileHash,
          targetPath: filePath,
        });
        console.log("Image saved to:", filePath);
      }
    } catch (e) {
      console.error("Failed to save image:", e);
    }
  }
</script>

<svelte:window on:keydown={handleKeydown} />

<!-- Fullscreen overlay -->
<div
  class="fixed inset-0 z-50 flex items-center justify-center bg-black/80 backdrop-blur-md animate-fade-in"
  on:click={handleBackdropClick}
  role="dialog"
  aria-modal="true"
>
  <!-- Close button -->
  <button
    on:click={close}
    class="absolute top-4 right-4 w-10 h-10 rounded-full bg-white/10 hover:bg-white/20 flex items-center justify-center transition-colors"
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

  <!-- Save button -->
  <button
    on:click={saveImage}
    class="absolute top-4 right-16 w-10 h-10 rounded-full bg-white/10 hover:bg-white/20 flex items-center justify-center transition-colors"
    aria-label="Save image"
    title="Save image"
  >
    <svg
      class="w-5 h-5 text-white"
      fill="none"
      viewBox="0 0 24 24"
      stroke="currentColor"
    >
      <path
        stroke-linecap="round"
        stroke-linejoin="round"
        stroke-width="2"
        d="M4 16v1a3 3 0 003 3h10a3 3 0 003-3v-1m-4-4l-4 4m0 0l-4-4m4 4V4"
      />
    </svg>
  </button>

  <!-- Image container -->
  <div class="relative max-w-[90vw] max-h-[90vh] animate-scale-in">
    <img
      src={imageDataUrl}
      alt="Full size image"
      class="max-w-full max-h-[90vh] object-contain rounded-lg shadow-2xl"
    />
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
