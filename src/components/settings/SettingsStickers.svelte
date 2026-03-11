<script lang="ts">
  import { onMount } from "svelte";
  import { open } from "@tauri-apps/plugin-dialog";
  import { api, type StickerBatchImportResult, type StickerItem } from "$lib/tauri/api";

  let { onback = () => {} }: { onback?: () => void } = $props();

  let stickers = $state<StickerItem[]>([]);
  let stickerImages = $state<Map<string, string>>(new Map());
  let loading = $state(false);
  let adding = $state(false);
  let error = $state<string | null>(null);
  let batchResult = $state<StickerBatchImportResult | null>(null);

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
      error = "Failed to load sticker library";
      stickers = [];
      stickerImages = new Map();
    } finally {
      loading = false;
    }
  }

  async function addStickersBatch() {
    adding = true;
    error = null;
    batchResult = null;
    try {
      const selected = await open({
        multiple: true,
        directory: false,
        filters: [
          {
            name: "Sticker Images",
            extensions: ["webp", "png", "jpg", "jpeg"],
          },
        ],
      });

      if (!selected) return;

      const filePaths = Array.isArray(selected) ? selected : [selected];
      if (filePaths.length === 0) return;

      batchResult = await api.addStickersBatch(filePaths);
      await loadStickers();
    } catch (e) {
      console.error("Batch sticker import failed:", e);
      error = "Sticker import failed";
    } finally {
      adding = false;
    }
  }

  async function deleteSticker(fileHash: string) {
    if (!confirm("Delete this sticker from your library?")) return;
    try {
      await api.deleteSticker(fileHash);
      stickers = stickers.filter((s) => s.file_hash !== fileHash);
      stickerImages.delete(fileHash);
      stickerImages = new Map(stickerImages);
    } catch (e) {
      console.error("Failed to delete sticker:", e);
      error = "Failed to delete sticker";
    }
  }

  function formatSize(bytes: number): string {
    if (bytes < 1024) return `${bytes} B`;
    if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
    return `${(bytes / (1024 * 1024)).toFixed(2)} MB`;
  }

  function formatDate(ts: number): string {
    return new Date(ts * 1000).toLocaleDateString();
  }
</script>

<div class="space-y-6">
  <div class="mb-6 flex items-center gap-4 border-b border-slate-800/50 pb-4">
    <button
      onclick={onback}
      class="p-2 hover:bg-theme-base-800 rounded-lg text-theme-base-400 hover:text-white transition-colors"
      aria-label="Go back"
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
    <div class="flex-1">
      <h2 class="text-xl font-semibold text-white">Stickers</h2>
      <p class="text-sm text-theme-base-500 mt-1">
        Import and manage your personal sticker library
      </p>
    </div>
    <button
      onclick={addStickersBatch}
      disabled={adding}
      class="px-4 py-2 bg-theme-primary-500 hover:bg-theme-primary-400 text-theme-base-950 rounded-lg font-medium transition-colors disabled:opacity-50"
    >
      {#if adding}
        Importing...
      {:else}
        + Import Stickers
      {/if}
    </button>
  </div>

  {#if batchResult}
    <div class="rounded-xl border border-theme-base-700 bg-theme-base-900 p-4 space-y-2">
      <p class="text-sm text-theme-base-200">
        Imported {batchResult.success_count} / {batchResult.results.length} files.
      </p>
      {#if batchResult.failure_count > 0}
        <div class="space-y-1">
          {#each batchResult.results.filter((r) => r.error) as failed}
            <p class="text-xs text-theme-error-400 truncate" title={failed.error || ""}>
              {failed.file_path.split("/").pop()}: {failed.error}
            </p>
          {/each}
        </div>
      {/if}
    </div>
  {/if}

  {#if error}
    <p class="text-sm text-theme-error-400">{error}</p>
  {/if}

  <div class="bg-theme-base-800/50 rounded-xl border border-theme-base-700 p-6">
    {#if loading}
      <div class="flex items-center justify-center py-12">
        <div
          class="w-8 h-8 border-2 border-theme-primary-500 border-t-transparent rounded-full animate-spin"
        ></div>
      </div>
    {:else if stickers.length === 0}
      <div class="text-center py-12 text-theme-base-500">
        <p class="text-lg">No stickers yet</p>
        <p class="text-sm mt-2">
          Import WebP directly or PNG/JPEG (auto-converted to WebP).
        </p>
      </div>
    {:else}
      <div class="grid grid-cols-3 sm:grid-cols-5 md:grid-cols-7 gap-4">
        {#each stickers as sticker}
          <div class="group relative">
            <div
              class="aspect-square rounded-xl bg-theme-base-700 p-2 hover:bg-theme-base-600 transition-colors"
            >
              {#if stickerImages.get(sticker.file_hash)}
                <img
                  src={stickerImages.get(sticker.file_hash)}
                  alt={sticker.name || "Sticker"}
                  class="w-full h-full object-contain"
                />
              {:else}
                <div
                  class="w-full h-full bg-theme-base-600 rounded animate-pulse"
                ></div>
              {/if}
            </div>
            <button
              onclick={() => deleteSticker(sticker.file_hash)}
              class="absolute -top-2 -right-2 w-6 h-6 bg-theme-error-500 hover:bg-theme-error-400 text-white rounded-full flex items-center justify-center opacity-0 group-hover:opacity-100 transition-opacity shadow-lg"
              aria-label="Delete sticker"
              title="Delete sticker"
            >
              ×
            </button>
            <p class="text-[11px] text-theme-base-400 mt-1 truncate px-1" title={sticker.name || "Sticker"}>
              {sticker.name || "Sticker"}
            </p>
            <p class="text-[10px] text-theme-base-600 px-1">
              {formatSize(sticker.size_bytes)} • {formatDate(sticker.created_at)}
            </p>
          </div>
        {/each}
      </div>
    {/if}
  </div>

  <p class="text-xs text-theme-base-600">
    No marketplace or default packs. Stickers are local to your library.
  </p>
</div>
