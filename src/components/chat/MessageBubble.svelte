<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { listen } from "@tauri-apps/api/event";
  import { save } from "@tauri-apps/plugin-dialog";
  import { onMount, onDestroy } from "svelte";
  import ImageViewer from "./ImageViewer.svelte";
  import VideoViewer from "./VideoViewer.svelte";

  export let msg: {
    sender: string;
    text: string;
    timestamp: Date;
    content_type?: string;
    file_hash?: string;
    status?: string; // 'pending', 'delivered', 'read'
  };
  export let userProfile: { alias: string | null; avatar_path: string | null };
  export let activePeer: string;

  $: isMe = msg.sender === "Me";
  $: isImage = msg.content_type === "image" && msg.file_hash;
  $: isDocument = msg.content_type === "document" && msg.file_hash;
  $: isVideo = msg.content_type === "video" && msg.file_hash;

  let imageDataUrl: string | null = null;
  let loadingImage = false;
  let downloadingImage = false; // File transfer in progress
  let loadError = false;
  let unlistenTransfer: (() => void) | null = null;
  let showViewer = false;
  let showVideoViewer = false;

  // Load image when this is an image message
  $: if (isImage && msg.file_hash && !imageDataUrl && !loadingImage) {
    loadImage(msg.file_hash);
  }

  onMount(async () => {
    // Listen for file transfer completion to reload image
    if (isImage && msg.file_hash) {
      unlistenTransfer = await listen<{ file_hash: string }>(
        "file-transfer-complete",
        (event) => {
          if (event.payload.file_hash === msg.file_hash) {
            console.log("[MessageBubble] Transfer complete for", msg.file_hash);
            downloadingImage = false;
            imageDataUrl = null; // Reset to trigger reload
            loadImage(msg.file_hash!);
          }
        }
      );
    }
  });

  onDestroy(() => {
    if (unlistenTransfer) unlistenTransfer();
  });

  async function loadImage(fileHash: string) {
    if (loadingImage) return;
    loadingImage = true;
    loadError = false;
    try {
      const dataUrl = await invoke<string>("get_image_data", { fileHash });
      // Check if we got valid data (not empty)
      if (dataUrl && dataUrl.startsWith("data:image")) {
        imageDataUrl = dataUrl;
        downloadingImage = false;
      } else {
        // File exists but no data - still downloading
        downloadingImage = true;
        imageDataUrl = null;
      }
    } catch (e) {
      console.error("Failed to load image:", e);
      // Could be still downloading
      downloadingImage = true;
      loadError = true;
    } finally {
      loadingImage = false;
    }
  }

  function formatTime(date: Date): string {
    return new Date(date).toLocaleTimeString([], {
      hour: "2-digit",
      minute: "2-digit",
    });
  }

  // Document download
  let downloadingDocument = false;

  async function downloadDocument() {
    if (!msg.file_hash) return;
    downloadingDocument = true;
    try {
      const fileName = msg.text || "document";
      const targetPath = await save({
        defaultPath: fileName,
        filters: [{ name: "All Files", extensions: ["*"] }],
      });
      if (!targetPath) {
        downloadingDocument = false;
        return; // User cancelled
      }
      await invoke("save_document_to_file", {
        fileHash: msg.file_hash,
        targetPath,
      });
      console.log("Document saved to:", targetPath);
    } catch (e) {
      console.error("Failed to download document:", e);
    } finally {
      downloadingDocument = false;
    }
  }

  function getDocumentIcon(fileName: string): string {
    const ext = fileName.split(".").pop()?.toLowerCase() || "";
    switch (ext) {
      case "pdf":
        return "📕";
      case "doc":
      case "docx":
        return "📘";
      case "xls":
      case "xlsx":
        return "📗";
      case "ppt":
      case "pptx":
        return "📙";
      case "txt":
        return "📄";
      default:
        return "📄";
    }
  }

  function formatFileSize(bytes: number): string {
    if (bytes < 1024) return `${bytes} B`;
    if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
    return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
  }
</script>

<div
  class={`flex w-full ${isMe ? "justify-end" : "justify-start"} animate-fade-in-up`}
>
  <div
    class={`flex max-w-[80%] md:max-w-[60%] gap-3 ${isMe ? "flex-row-reverse" : "flex-row"}`}
  >
    <!-- Avatar -->
    <div class="shrink-0 self-end mb-1">
      {#if isMe}
        {#if userProfile.avatar_path}
          <img
            src={userProfile.avatar_path}
            class="w-8 h-8 rounded-full bg-theme-primary-500 border-2 border-theme-base-950 object-cover"
            alt="Me"
          />
        {:else}
          <div
            class="w-8 h-8 rounded-full bg-theme-primary-500 shadow-lg shadow-teal-500/20 border-2 border-theme-base-950"
          ></div>
        {/if}
      {:else}
        <img
          src={`https://github.com/${activePeer}.png?size=32`}
          class="w-8 h-8 rounded-full bg-theme-secondary-500 shadow-lg shadow-purple-500/20 border-2 border-theme-base-950"
          on:error={(e) =>
            ((e.currentTarget as HTMLImageElement).src =
              "https://github.com/github.png?size=32")}
          alt="Peer"
        />
      {/if}
    </div>

    <!-- Bubble -->
    <div
      class={`px-4 py-2.5 shadow-md text-sm leading-relaxed break-words flex flex-col gap-1
        ${isMe ? "bg-theme-primary-600 text-[var(--color-on-primary)] rounded-2xl rounded-tr-sm" : "bg-theme-base-800 text-theme-base-200 rounded-2xl rounded-tl-sm border border-theme-base-700"}`}
    >
      {#if isImage}
        <!-- Image content -->
        {#if loadingImage}
          <div
            class="w-48 h-32 bg-theme-base-700 rounded-lg flex items-center justify-center"
          >
            <div
              class="animate-spin w-6 h-6 border-2 border-white border-t-transparent rounded-full"
            ></div>
          </div>
        {:else if downloadingImage}
          <!-- Downloading from peer -->
          <div
            class="w-48 h-32 bg-slate-700/50 rounded-lg flex flex-col items-center justify-center gap-2 border border-theme-base-600"
          >
            <svg
              class="w-8 h-8 text-theme-base-400"
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
            <span class="text-xs text-theme-base-400">Downloading...</span>
            <div
              class="w-24 h-1 bg-theme-base-600 rounded-full overflow-hidden"
            >
              <div
                class="h-full bg-theme-secondary-500 animate-pulse"
                style="width: 60%"
              ></div>
            </div>
          </div>
        {:else if imageDataUrl}
          <img
            src={imageDataUrl}
            alt="Sent image"
            class="max-w-[300px] max-h-[300px] rounded-lg cursor-pointer hover:opacity-90 transition-opacity"
            on:click={() => (showViewer = true)}
          />
        {:else}
          <div
            class="w-48 h-32 bg-slate-700/50 rounded-lg flex flex-col items-center justify-center gap-1 border border-theme-base-600"
          >
            <svg
              class="w-6 h-6 text-theme-base-500"
              fill="none"
              viewBox="0 0 24 24"
              stroke="currentColor"
            >
              <path
                stroke-linecap="round"
                stroke-linejoin="round"
                stroke-width="2"
                d="M4 16l4.586-4.586a2 2 0 012.828 0L16 16m-2-2l1.586-1.586a2 2 0 012.828 0L20 14m-6-6h.01M6 20h12a2 2 0 002-2V6a2 2 0 00-2-2H6a2 2 0 00-2 2v12a2 2 0 002 2z"
              />
            </svg>
            <span class="text-xs text-theme-base-400">Image not available</span>
          </div>
        {/if}
      {:else if isDocument}
        <!-- Document content -->
        <button
          on:click={downloadDocument}
          disabled={downloadingDocument}
          class="flex items-center gap-3 p-3 bg-slate-700/50 rounded-lg hover:bg-slate-600/50 transition-colors cursor-pointer border border-theme-base-600 min-w-[200px]"
        >
          <span class="text-2xl">{getDocumentIcon(msg.text || "document")}</span
          >
          <div class="flex flex-col items-start flex-1 min-w-0">
            <span class="text-sm font-medium text-white truncate max-w-[180px]"
              >{msg.text || "Document"}</span
            >
            <span class="text-xs text-theme-base-400">
              {#if downloadingDocument}
                Downloading...
              {:else}
                Click to download
              {/if}
            </span>
          </div>
          {#if downloadingDocument}
            <div
              class="animate-spin w-5 h-5 border-2 border-white border-t-transparent rounded-full"
            ></div>
          {:else}
            <svg
              class="w-5 h-5 text-theme-base-400"
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
          {/if}
        </button>
      {:else if isVideo}
        <!-- Video content -->
        <button
          on:click={() => (showVideoViewer = true)}
          class="relative w-48 h-32 bg-theme-base-800 rounded-lg overflow-hidden cursor-pointer hover:opacity-90 transition-opacity border border-theme-base-600"
        >
          <!-- Video icon/thumbnail -->
          <div
            class="absolute inset-0 flex items-center justify-center bg-gradient-to-br from-pink-500/20 to-purple-500/20"
          >
            <div
              class="w-12 h-12 rounded-full bg-white/20 flex items-center justify-center backdrop-blur-sm"
            >
              <svg
                class="w-6 h-6 text-white ml-1"
                fill="currentColor"
                viewBox="0 0 24 24"
              >
                <path d="M8 5v14l11-7z" />
              </svg>
            </div>
          </div>
          <!-- Filename label -->
          <div class="absolute bottom-0 left-0 right-0 bg-black/60 px-2 py-1">
            <span class="text-xs text-white truncate block"
              >{msg.text || "Video"}</span
            >
          </div>
        </button>
      {:else}
        <!-- Text content -->
        <span>{msg.text}</span>
      {/if}
      <span
        class={`text-[10px] ${isMe ? "text-theme-primary-200" : "text-theme-base-400"} self-end flex items-center gap-1`}
      >
        {formatTime(msg.timestamp)}
        {#if isMe}
          {#if msg.status === "pending"}
            <!-- Pending: loading spinner -->
            <svg class="w-3 h-3 animate-spin" viewBox="0 0 24 24" fill="none">
              <circle
                class="opacity-25"
                cx="12"
                cy="12"
                r="10"
                stroke="currentColor"
                stroke-width="3"
              ></circle>
              <path
                class="opacity-75"
                fill="currentColor"
                d="M4 12a8 8 0 018-8V0C5.373 0 0 5.373 0 12h4zm2 5.291A7.962 7.962 0 014 12H0c0 3.042 1.135 5.824 3 7.938l3-2.647z"
              ></path>
            </svg>
          {:else if msg.status === "delivered"}
            <!-- Delivered: single check -->
            <svg
              class="w-3 h-3"
              viewBox="0 0 24 24"
              fill="none"
              stroke="currentColor"
              stroke-width="3"
            >
              <polyline points="20 6 9 17 4 12"></polyline>
            </svg>
          {:else}
            <!-- Read: double check -->
            <svg
              class="w-4 h-3"
              viewBox="0 0 28 24"
              fill="none"
              stroke="currentColor"
              stroke-width="2.5"
            >
              <polyline points="2 12 8 18 20 6"></polyline>
              <polyline points="8 12 14 18 26 6"></polyline>
            </svg>
          {/if}
        {/if}
      </span>
    </div>
  </div>
</div>

<!-- Fullscreen image viewer -->
{#if showViewer && imageDataUrl && msg.file_hash}
  <ImageViewer
    {imageDataUrl}
    fileHash={msg.file_hash}
    on:close={() => (showViewer = false)}
  />
{/if}

<!-- Fullscreen video viewer -->
{#if showVideoViewer && msg.file_hash}
  <VideoViewer
    fileHash={msg.file_hash}
    on:close={() => (showVideoViewer = false)}
  />
{/if}

<style>
  @keyframes fade-in-up {
    from {
      opacity: 0;
      transform: translateY(10px);
    }
    to {
      opacity: 1;
      transform: translateY(0);
    }
  }
  .animate-fade-in-up {
    animation: fade-in-up 0.3s ease-out forwards;
  }
</style>
