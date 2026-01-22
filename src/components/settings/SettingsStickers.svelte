<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { open } from "@tauri-apps/plugin-dialog";
  import { onMount } from "svelte";

  interface Sticker {
    file_hash: string;
    name: string | null;
    created_at: number;
    size_bytes: number;
  }

  let stickers = $state<Sticker[]>([]);
  let stickerImages = $state<Map<string, string>>(new Map());
  let loading = $state(false);
  let adding = $state(false);

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

  async function addSticker() {
    try {
      const filePath = await open({
        filters: [{ name: "WebP Images", extensions: ["webp"] }],
        multiple: false,
      });

      if (!filePath) return;

      adding = true;
      // Read file and send to backend
      const response = await fetch(`file://${filePath}`);
      const arrayBuffer = await response.arrayBuffer();
      const webpData = Array.from(new Uint8Array(arrayBuffer));

      const fileName = (filePath as string).split("/").pop() || "sticker";
      await invoke("add_sticker", {
        webpData,
        name: fileName.replace(".webp", ""),
      });

      await loadStickers();
    } catch (e) {
      console.error("Failed to add sticker:", e);
      alert("Failed to add sticker: " + e);
    } finally {
      adding = false;
    }
  }

  async function deleteSticker(fileHash: string) {
    if (!confirm("Delete this sticker?")) return;
    try {
      await invoke("delete_sticker", { fileHash });
      await loadStickers();
    } catch (e) {
      console.error("Failed to delete sticker:", e);
    }
  }
</script>

<div class="space-y-6">
  <!-- Header -->
  <div class="flex items-center justify-between">
    <div>
      <h2 class="text-xl font-semibold text-white">Stickers</h2>
      <p class="text-sm text-theme-base-500 mt-1">
        Manage your sticker collection
      </p>
    </div>
    <button
      onclick={addSticker}
      disabled={adding}
      class="px-4 py-2 bg-theme-primary-500 hover:bg-theme-primary-400 text-theme-base-950 rounded-lg font-medium transition-colors disabled:opacity-50"
    >
      {#if adding}
        Adding...
      {:else}
        + Add Sticker
      {/if}
    </button>
  </div>

  <!-- Sticker Grid -->
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
          Click "Add Sticker" to upload WebP stickers (max 1MB)
        </p>
      </div>
    {:else}
      <div class="grid grid-cols-4 sm:grid-cols-6 md:grid-cols-8 gap-4">
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
            <!-- Delete button -->
            <button
              onclick={() => deleteSticker(sticker.file_hash)}
              class="absolute -top-2 -right-2 w-6 h-6 bg-theme-error-500 hover:bg-theme-error-400 text-white rounded-full flex items-center justify-center opacity-0 group-hover:opacity-100 transition-opacity shadow-lg"
              title="Delete sticker"
            >
              ×
            </button>
            {#if sticker.name}
              <p
                class="text-xs text-theme-base-400 text-center mt-1 truncate px-1"
              >
                {sticker.name}
              </p>
            {/if}
          </div>
        {/each}
      </div>
    {/if}
  </div>

  <!-- Info -->
  <p class="text-xs text-theme-base-600">
    Stickers must be WebP format (supports animation) and under 1MB.
  </p>
</div>
