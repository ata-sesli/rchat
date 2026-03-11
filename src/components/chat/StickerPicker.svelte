<script lang="ts">
  import { onMount } from "svelte";
  import { open } from "@tauri-apps/plugin-dialog";
  import { api, type StickerItem } from "$lib/tauri/api";

  let { onselectsticker = (file_hash: string) => {}, onclose = () => {} } =
    $props();

  let stickers = $state<StickerItem[]>([]);
  let stickerImages = $state<Map<string, string>>(new Map());
  let loading = $state(false);
  let adding = $state(false);
  let error = $state<string | null>(null);

  onMount(async () => {
    await loadStickers();
  });

  async function loadStickers() {
    loading = true;
    error = null;
    try {
      stickers = await api.listStickers();
      const entries = await Promise.all(
        stickers.map(async (sticker) => {
          try {
            const dataUrl = await api.getImageData(sticker.file_hash);
            return [sticker.file_hash, dataUrl] as const;
          } catch {
            return [sticker.file_hash, ""] as const;
          }
        })
      );
      stickerImages = new Map(entries.filter(([, value]) => Boolean(value)));
    } catch (e) {
      console.error("Failed to load stickers:", e);
      error = "Failed to load stickers";
      stickers = [];
      stickerImages = new Map();
    } finally {
      loading = false;
    }
  }

  async function addSingleSticker() {
    adding = true;
    error = null;
    try {
      const selected = await open({
        multiple: false,
        directory: false,
        filters: [
          {
            name: "Sticker Image",
            extensions: ["webp", "png", "jpg", "jpeg"],
          },
        ],
      });

      if (!selected || Array.isArray(selected)) return;

      await api.addSticker(selected);
      await loadStickers();
    } catch (e: any) {
      console.error("Failed to add sticker:", e);
      error = e?.toString?.() || "Failed to add sticker";
    } finally {
      adding = false;
    }
  }

  function selectSticker(fileHash: string) {
    onselectsticker(fileHash);
    onclose();
  }
</script>

<div
  class="absolute bottom-full mb-2 left-0 w-80 max-h-[26rem] bg-theme-base-900 border border-theme-base-700 rounded-xl shadow-2xl overflow-hidden z-[70]"
>
  <div
    class="flex items-center justify-between px-4 py-3 border-b border-theme-base-700"
  >
    <span class="text-sm font-medium text-theme-base-200">Stickers</span>
    <div class="flex items-center gap-1">
      <button
        onclick={addSingleSticker}
        class="px-2 py-1 text-xs rounded-md bg-theme-primary-500/20 text-theme-primary-300 hover:bg-theme-primary-500/30 transition-colors disabled:opacity-50"
        disabled={adding}
        title="Add sticker"
      >
        {adding ? "..." : "+ Add"}
      </button>
      <button
        onclick={() => onclose()}
        class="text-theme-base-500 hover:text-white transition-colors"
        aria-label="Close sticker picker"
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
  </div>

  <div class="p-3 overflow-y-auto max-h-80 scrollbar-hide">
    {#if loading}
      <div class="flex items-center justify-center py-8">
        <div
          class="w-6 h-6 border-2 border-theme-primary-500 border-t-transparent rounded-full animate-spin"
        ></div>
      </div>
    {:else if error}
      <div class="text-center py-6 text-theme-error-400 text-xs">{error}</div>
    {:else if stickers.length === 0}
      <div class="text-center py-8 text-theme-base-500 text-sm">
        <p>No stickers yet</p>
        <p class="text-xs mt-1">Add one here or import batch from Settings</p>
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
