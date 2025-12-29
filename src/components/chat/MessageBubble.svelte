<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { onMount } from "svelte";

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

  let imageDataUrl: string | null = null;
  let loadingImage = false;

  // Load image when this is an image message
  $: if (isImage && msg.file_hash && !imageDataUrl) {
    loadImage(msg.file_hash);
  }

  async function loadImage(fileHash: string) {
    if (loadingImage || imageDataUrl) return;
    loadingImage = true;
    try {
      const dataUrl = await invoke<string>("get_image_data", { fileHash });
      imageDataUrl = dataUrl;
    } catch (e) {
      console.error("Failed to load image:", e);
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
            class="w-8 h-8 rounded-full bg-teal-500 border-2 border-slate-950 object-cover"
            alt="Me"
          />
        {:else}
          <div
            class="w-8 h-8 rounded-full bg-teal-500 shadow-lg shadow-teal-500/20 border-2 border-slate-950"
          ></div>
        {/if}
      {:else}
        <img
          src={`https://github.com/${activePeer}.png?size=32`}
          class="w-8 h-8 rounded-full bg-purple-500 shadow-lg shadow-purple-500/20 border-2 border-slate-950"
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
        ${isMe ? "bg-teal-600/90 text-white rounded-2xl rounded-tr-sm" : "bg-slate-800 text-slate-200 rounded-2xl rounded-tl-sm border border-slate-700/50"}`}
    >
      {#if isImage}
        <!-- Image content -->
        {#if loadingImage}
          <div
            class="w-48 h-32 bg-slate-700 rounded-lg flex items-center justify-center"
          >
            <div
              class="animate-spin w-6 h-6 border-2 border-white border-t-transparent rounded-full"
            ></div>
          </div>
        {:else if imageDataUrl}
          <img
            src={imageDataUrl}
            alt="Sent image"
            class="max-w-[300px] max-h-[300px] rounded-lg cursor-pointer hover:opacity-90 transition-opacity"
            on:click={() => window.open(imageDataUrl!, "_blank")}
          />
        {:else}
          <div class="text-slate-400 italic">Failed to load image</div>
        {/if}
      {:else}
        <!-- Text content -->
        <span>{msg.text}</span>
      {/if}
      <span
        class={`text-[10px] ${isMe ? "text-teal-200" : "text-slate-400"} self-end flex items-center gap-1`}
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
