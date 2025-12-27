<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { onMount } from "svelte";

  export let msg: {
    sender: string;
    text: string;
    timestamp: Date;
    content_type?: string;
    file_hash?: string;
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
        class={`text-[10px] ${isMe ? "text-teal-200" : "text-slate-400"} self-end`}
      >
        {formatTime(msg.timestamp)}
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
