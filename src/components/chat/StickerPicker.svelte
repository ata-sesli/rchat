<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { onMount } from "svelte";

  interface Sticker {
    file_hash: string;
    name: string | null;
    created_at: number;
    size_bytes: number;
  }

  let { onselectsticker = (file_hash: string) => {}, onclose = () => {} } =
    $props();

  let stickers = $state<Sticker[]>([]);
  let stickerImages = $state<Map<string, string>>(new Map());
  let loading = $state(false);

  onMount(async () => {
    await loadStickers();
  });

  async function loadStickers() {
    loading = true;
    try {
      stickers = await invoke<Sticker[]>("get_stickers");
      // Load sticker images
      for (const sticker of stickers) {
        const data = await invoke<number[]>("get_sticker_data", {
          fileHash: sticker.file_hash,
        });
        const blob = new Blob([new Uint8Array(data)], { type: "image/webp" });
        const url = URL.createObjectURL(blob);
        stickerImages.set(sticker.file_hash, url);
      }
      stickerImages = new Map(stickerImages);
    } catch (e) {
      console.error("Failed to load stickers:", e);
    }
    loading = false;
  }

  function selectSticker(fileHash: string) {
    onselectsticker(fileHash);
    onclose();
  }
</script>

<div
  class="absolute bottom-full mb-2 right-0 w-80 max-h-80 bg-theme-base-900 border border-theme-base-700 rounded-xl shadow-2xl overflow-hidden z-50"
>
  <!-- Header -->
  <div
    class="flex items-center justify-between px-4 py-3 border-b border-theme-base-700"
  >
    <span class="text-sm font-medium text-theme-base-200">Stickers</span>
    <button
      onclick={() => onclose()}
      class="text-theme-base-500 hover:text-white transition-colors"
    >
      <svg
        xmlns="http://www.w3.org/2000/svg"
        class="h-5 w-5"
        viewBox="0 0 20 20"
        fill="currentColor"
      >
        <path
          fill-rule="evenodd"
          d="M4.293 4.293a1 1 0 011.414 0L10 8.586l4.293-4.293a1 1 0 111.414 1.414L11.414 10l4.293 4.293a1 1 0 01-1.414 1.414L10 11.414l-4.293 4.293a1 1 0 01-1.414-1.414L8.586 10 4.293 5.707a1 1 0 010-1.414z"
          clip-rule="evenodd"
        />
      </svg>
    </button>
  </div>

  <!-- Sticker Grid -->
  <div class="p-3 overflow-y-auto max-h-64 scrollbar-hide">
    {#if loading}
      <div class="flex items-center justify-center py-8">
        <div
          class="w-6 h-6 border-2 border-theme-primary-500 border-t-transparent rounded-full animate-spin"
        ></div>
      </div>
    {:else if stickers.length === 0}
      <div class="text-center py-8 text-theme-base-500 text-sm">
        <p>No stickers yet</p>
        <p class="text-xs mt-1">Add stickers in Settings</p>
      </div>
    {:else}
      <div class="grid grid-cols-4 gap-2">
        {#each stickers as sticker}
          <button
            onclick={() => selectSticker(sticker.file_hash)}
            class="aspect-square rounded-lg hover:bg-theme-base-700 transition-colors p-1 group"
            title={sticker.name || "Sticker"}
          >
            {#if stickerImages.get(sticker.file_hash)}
              <img
                src={stickerImages.get(sticker.file_hash)}
                alt={sticker.name || "Sticker"}
                class="w-full h-full object-contain group-hover:scale-110 transition-transform"
              />
            {:else}
              <div
                class="w-full h-full bg-theme-base-800 rounded animate-pulse"
              ></div>
            {/if}
          </button>
        {/each}
      </div>
    {/if}
  </div>
</div>
