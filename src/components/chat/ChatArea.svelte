<script lang="ts">
  import { tick } from "svelte";
  import { invoke } from "@tauri-apps/api/core";
  import { open } from "@tauri-apps/plugin-dialog";
  import MessageBubble from "./MessageBubble.svelte";

  // Types
  type Message = {
    sender: string;
    text: string;
    timestamp: Date;
    content_type?: string;
    file_hash?: string;
    status?: string;
  };

  // Props
  export let activePeer = "Me";
  export let peerAlias: string | null = null; // Display alias for activePeer
  export let messages: Message[] = [];
  export let userProfile: { alias: string | null; avatar_path: string | null } =
    { alias: null, avatar_path: null };
  export let message = "";
  export let showAttachments = false;

  // Helper to truncate ID
  function truncateId(id: string, maxLen = 15): string {
    if (id.length <= maxLen) return id;
    return id.substring(0, maxLen) + "...";
  }

  // Callback props
  export let onsend = (msg: string) => {};
  export let ontoggleAttachments = (show: boolean) => {};
  export let onImageSent = (fileHash: string) => {};
  export let onDocumentSent = (fileHash: string, fileName: string) => {};
  export let onVideoSent = (fileHash: string, fileName: string) => {};

  // Refs
  let chatContainer: HTMLElement;
  let textarea: HTMLTextAreaElement;

  // Expose scrollToBottom
  export async function scrollToBottom() {
    await tick();
    if (chatContainer) {
      chatContainer.scrollTo({
        top: chatContainer.scrollHeight,
        behavior: "smooth",
      });
    }
  }

  function handleKeydown(e: KeyboardEvent) {
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      sendMessage();
    }
  }

  function sendMessage() {
    // Send pending images first if any
    if (pendingImages.length > 0) {
      sendPendingImages();
    }
    // Send pending documents
    if (pendingDocuments.length > 0) {
      sendPendingDocuments();
    }
    // Send pending videos
    if (pendingVideos.length > 0) {
      sendPendingVideos();
    }

    // Then send text message if any
    if (message.trim()) {
      onsend(message);
      message = "";
      if (textarea) {
        textarea.style.height = "auto";
      }
    }
  }

  function toggleAttachments() {
    showAttachments = !showAttachments;
    ontoggleAttachments(showAttachments);
  }

  function handleInput(e: Event) {
    const target = e.currentTarget as HTMLTextAreaElement;
    target.style.height = "auto";
    target.style.height = target.scrollHeight + "px";
  }

  // Pending images to preview before sending
  type PendingImage = { path: string; name: string; dataUrl?: string };
  let pendingImages: PendingImage[] = [];
  let isSendingImage = false;

  async function pickImage() {
    try {
      const filePath = await open({
        filters: [
          {
            name: "Images",
            extensions: ["png", "jpg", "jpeg", "gif", "webp"],
          },
        ],
        multiple: false,
        directory: false,
      });

      if (!filePath) return; // User cancelled

      // Add to pending images for preview
      const fileName = (filePath as string).split("/").pop() || "image";
      const newImg: PendingImage = { path: filePath as string, name: fileName };

      // Load preview via backend
      try {
        const dataUrl = await invoke<string>("get_image_from_path", {
          filePath: filePath as string,
        });
        newImg.dataUrl = dataUrl;
      } catch (e) {
        console.error("Failed to load preview:", e);
      }

      pendingImages = [...pendingImages, newImg];
      showAttachments = false;
      console.log("Image queued for preview:", filePath);
    } catch (e) {
      console.error("Failed to pick image:", e);
    }
  }

  function removeImage(index: number) {
    pendingImages = pendingImages.filter((_, i) => i !== index);
  }

  async function sendPendingImages() {
    if (pendingImages.length === 0) return;
    if (isSendingImage) return;

    isSendingImage = true;
    try {
      for (const img of pendingImages) {
        console.log("Sending image:", img.path);
        const fileHash = await invoke<string>("send_image_message", {
          peerId: activePeer,
          filePath: img.path,
        });
        console.log("Image sent with hash:", fileHash);
        onImageSent(fileHash);
      }
      pendingImages = [];
    } catch (e) {
      console.error("Failed to send image:", e);
    } finally {
      isSendingImage = false;
    }
  }

  // Pending documents to preview before sending
  type PendingDocument = { path: string; name: string; size: number };
  let pendingDocuments: PendingDocument[] = [];
  let isSendingDocument = false;

  async function pickDocument() {
    try {
      const filePath = await open({
        filters: [
          {
            name: "Documents",
            extensions: [
              "pdf",
              "doc",
              "docx",
              "txt",
              "xls",
              "xlsx",
              "ppt",
              "pptx",
              "csv",
            ],
          },
        ],
        multiple: false,
        directory: false,
      });

      if (!filePath) return; // User cancelled

      const fileName = (filePath as string).split("/").pop() || "document";
      // Get file size via metadata (approximate for now)
      const newDoc: PendingDocument = {
        path: filePath as string,
        name: fileName,
        size: 0,
      };
      pendingDocuments = [...pendingDocuments, newDoc];
      showAttachments = false;
      console.log("Document queued:", filePath);
    } catch (e) {
      console.error("Failed to pick document:", e);
    }
  }

  function removeDocument(index: number) {
    pendingDocuments = pendingDocuments.filter((_, i) => i !== index);
  }

  async function sendPendingDocuments() {
    if (pendingDocuments.length === 0) return;
    if (isSendingDocument) return;

    isSendingDocument = true;
    try {
      for (const doc of pendingDocuments) {
        console.log("Sending document:", doc.path);
        const fileHash = await invoke<string>("send_document_message", {
          peerId: activePeer,
          filePath: doc.path,
        });
        console.log("Document sent with hash:", fileHash);
        onDocumentSent(fileHash, doc.name);
      }
      pendingDocuments = [];
    } catch (e) {
      console.error("Failed to send document:", e);
    } finally {
      isSendingDocument = false;
    }
  }

  // Pending videos to preview before sending
  type PendingVideo = { path: string; name: string; dataUrl?: string };
  let pendingVideos: PendingVideo[] = [];
  let isSendingVideo = false;

  async function pickVideo() {
    try {
      const filePath = await open({
        filters: [
          {
            name: "Videos",
            extensions: ["mp4", "webm", "mov", "avi", "mkv"],
          },
        ],
        multiple: false,
        directory: false,
      });

      if (!filePath) return; // User cancelled

      const fileName = (filePath as string).split("/").pop() || "video.mp4";
      // Create object URL for preview (uses file:// protocol in Tauri)
      const newVid: PendingVideo = {
        path: filePath as string,
        name: fileName,
        dataUrl: `file://${filePath}`, // Tauri allows file:// URLs
      };
      pendingVideos = [...pendingVideos, newVid];
      showAttachments = false;
      console.log("Video queued:", filePath);
    } catch (e) {
      console.error("Failed to pick video:", e);
    }
  }

  function removeVideo(index: number) {
    pendingVideos = pendingVideos.filter((_, i) => i !== index);
  }

  async function sendPendingVideos() {
    if (pendingVideos.length === 0) return;
    if (isSendingVideo) return;

    isSendingVideo = true;
    try {
      for (const vid of pendingVideos) {
        console.log("Sending video:", vid.path);
        const fileHash = await invoke<string>("send_video_message", {
          peerId: activePeer,
          filePath: vid.path,
        });
        console.log("Video sent with hash:", fileHash);
        onVideoSent(fileHash, vid.name);
      }
      pendingVideos = [];
    } catch (e) {
      console.error("Failed to send video:", e);
    } finally {
      isSendingVideo = false;
    }
  }

  // Auto-scroll when messages change
  $: if (messages.length > 0 && chatContainer) {
    scrollToBottom();
  }
</script>

<!-- Chat Header -->
<div
  class="h-16 flex items-center px-6 border-b border-slate-800/50 bg-slate-900/10 backdrop-blur-sm"
>
  <div class="flex items-center gap-3">
    <span class="text-xl font-bold text-white">
      {#if activePeer === "Me"}
        Me (You)
      {:else if activePeer === "General"}
        # General
      {:else}
        {peerAlias || activePeer}
      {/if}
    </span>
    {#if activePeer !== "Me" && activePeer !== "General"}
      <span class="text-xs text-slate-500 ml-2">@ {truncateId(activePeer)}</span
      >
    {/if}
    {#if activePeer !== "Me" && activePeer !== "General"}
      <div
        class="w-2 h-2 rounded-full bg-green-500 shadow-lg shadow-green-500/50"
      ></div>
    {/if}
  </div>
</div>

<!-- Messages -->
<div
  bind:this={chatContainer}
  class="flex-1 overflow-y-auto px-6 py-6 space-y-6 scroll-smooth"
>
  {#if messages.length === 0}
    <div
      class="flex flex-col items-center justify-center h-full text-slate-500 space-y-4 opacity-0 animate-fade-in-up"
      style="animation-fill-mode: forwards;"
    >
      <div
        class="w-16 h-16 rounded-2xl bg-slate-900 border border-slate-800 flex items-center justify-center"
      >
        <span class="text-3xl">👋</span>
      </div>
      <p>
        {#if activePeer === "Me"}
          This is your personal space.
        {:else}
          Start chatting with {activePeer}!
        {/if}
      </p>
    </div>
  {/if}

  {#each messages as msg}
    <MessageBubble {msg} {userProfile} {activePeer} />
  {/each}
</div>

<!-- Input Area -->
<div class="p-6 w-full max-w-4xl mx-auto">
  <!-- Pending Images Preview -->
  {#if pendingImages.length > 0}
    <div
      class="mb-3 flex gap-2 flex-wrap bg-slate-900/60 border border-slate-700/50 rounded-xl p-3"
    >
      {#each pendingImages as img, index}
        <div class="relative group">
          <div
            class="w-16 h-16 bg-slate-800 rounded-lg flex items-center justify-center overflow-hidden border border-slate-600 relative"
          >
            {#if img.dataUrl}
              <!-- Actual image preview -->
              <img
                src={img.dataUrl}
                alt={img.name}
                class="w-full h-full object-cover"
              />
            {:else}
              <!-- Fallback icon when loading or no dataUrl -->
              <svg
                class="w-8 h-8 text-purple-400"
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
            {/if}
          </div>
          <button
            on:click={() => removeImage(index)}
            class="absolute -top-2 -right-2 w-5 h-5 bg-red-500 hover:bg-red-400 text-white rounded-full flex items-center justify-center text-xs opacity-0 group-hover:opacity-100 transition-opacity"
          >
            ×
          </button>
          <p class="text-xs text-slate-400 mt-1 truncate w-16 text-center">
            {img.name}
          </p>
        </div>
      {/each}
    </div>
  {/if}

  <!-- Pending Documents Preview -->
  {#if pendingDocuments.length > 0}
    <div
      class="mb-3 flex gap-2 flex-wrap bg-slate-900/60 border border-slate-700/50 rounded-xl p-3"
    >
      {#each pendingDocuments as doc, index}
        <div
          class="relative group flex items-center gap-2 bg-slate-800 rounded-lg p-2 pr-8 border border-slate-600"
        >
          <span class="text-xl">
            {#if doc.name.endsWith(".pdf")}📕
            {:else if doc.name.endsWith(".doc") || doc.name.endsWith(".docx")}📘
            {:else if doc.name.endsWith(".xls") || doc.name.endsWith(".xlsx")}📗
            {:else if doc.name.endsWith(".ppt") || doc.name.endsWith(".pptx")}📙
            {:else}📄
            {/if}
          </span>
          <span class="text-xs text-slate-300 truncate max-w-[120px]"
            >{doc.name}</span
          >
          <button
            on:click={() => removeDocument(index)}
            class="absolute top-1 right-1 w-5 h-5 bg-red-500 hover:bg-red-400 text-white rounded-full flex items-center justify-center text-xs opacity-0 group-hover:opacity-100 transition-opacity"
          >
            ×
          </button>
        </div>
      {/each}
    </div>
  {/if}

  <!-- Pending Videos Preview -->
  {#if pendingVideos.length > 0}
    <div
      class="mb-3 flex gap-2 flex-wrap bg-slate-900/60 border border-slate-700/50 rounded-xl p-3"
    >
      {#each pendingVideos as vid, index}
        <div class="relative group">
          <div
            class="w-20 h-14 bg-slate-800 rounded-lg flex items-center justify-center overflow-hidden border border-slate-600 relative"
          >
            {#if vid.dataUrl}
              <!-- svelte-ignore a11y_media_has_caption -->
              <video src={vid.dataUrl} class="w-full h-full object-cover" muted
              ></video>
              <!-- Play icon overlay -->
              <div
                class="absolute inset-0 flex items-center justify-center bg-black/30"
              >
                <svg
                  class="w-6 h-6 text-white"
                  fill="currentColor"
                  viewBox="0 0 24 24"
                >
                  <path d="M8 5v14l11-7z" />
                </svg>
              </div>
            {:else}
              <!-- Fallback icon -->
              <svg
                class="w-8 h-8 text-purple-400"
                fill="none"
                viewBox="0 0 24 24"
                stroke="currentColor"
              >
                <path
                  stroke-linecap="round"
                  stroke-linejoin="round"
                  stroke-width="2"
                  d="M14.752 11.168l-3.197-2.132A1 1 0 0010 9.87v4.263a1 1 0 001.555.832l3.197-2.132a1 1 0 000-1.664z"
                />
                <path
                  stroke-linecap="round"
                  stroke-linejoin="round"
                  stroke-width="2"
                  d="M21 12a9 9 0 11-18 0 9 9 0 0118 0z"
                />
              </svg>
            {/if}
          </div>
          <button
            on:click={() => removeVideo(index)}
            class="absolute -top-2 -right-2 w-5 h-5 bg-red-500 hover:bg-red-400 text-white rounded-full flex items-center justify-center text-xs opacity-0 group-hover:opacity-100 transition-opacity"
            title="Remove video"
          >
            ×
          </button>
          <p class="text-xs text-slate-400 mt-1 truncate w-20 text-center">
            {vid.name}
          </p>
        </div>
      {/each}
    </div>
  {/if}

  <div
    class="bg-slate-900/90 backdrop-blur-md border border-slate-700 rounded-2xl p-1.5 shadow-2xl flex items-center gap-2 relative"
  >
    <!-- Attachments Button -->
    <div class="relative">
      <button
        on:click={toggleAttachments}
        class={`p-2 rounded-xl transition-all ${showAttachments ? "bg-slate-700 text-teal-400" : "text-slate-400 hover:text-white hover:bg-slate-800"}`}
        title="Add Attachment"
      >
        <svg
          xmlns="http://www.w3.org/2000/svg"
          class="h-6 w-6"
          fill="none"
          viewBox="0 0 24 24"
          stroke="currentColor"
        >
          <path
            stroke-linecap="round"
            stroke-linejoin="round"
            stroke-width="2"
            d="M15.172 7l-6.586 6.586a2 2 0 102.828 2.828l6.414-6.586a4 4 0 00-5.656-5.656l-6.415 6.585a6 6 0 108.486 8.486L20.5 13"
          />
        </svg>
      </button>

      {#if showAttachments}
        <div
          class="absolute bottom-full left-0 mb-2 w-48 bg-slate-800 border border-slate-700 rounded-xl shadow-xl overflow-hidden z-50 animate-fade-in-up"
        >
          <button
            on:click={pickImage}
            class="w-full text-left px-4 py-3 text-sm text-slate-200 hover:bg-slate-700 hover:text-white flex items-center gap-3 transition-colors"
            disabled={isSendingImage}
          >
            <svg
              xmlns="http://www.w3.org/2000/svg"
              class="h-5 w-5 text-purple-400"
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
            {#if isSendingImage}
              Sending...
            {:else}
              Image
            {/if}
          </button>
          <div class="h-px bg-slate-700/50"></div>
          <button
            on:click={pickVideo}
            class="w-full text-left px-4 py-3 text-sm text-slate-200 hover:bg-slate-700 hover:text-white flex items-center gap-3 transition-colors"
            disabled={isSendingVideo}
          >
            <svg
              xmlns="http://www.w3.org/2000/svg"
              class="h-5 w-5 text-pink-400"
              fill="none"
              viewBox="0 0 24 24"
              stroke="currentColor"
            >
              <path
                stroke-linecap="round"
                stroke-linejoin="round"
                stroke-width="2"
                d="M14.752 11.168l-3.197-2.132A1 1 0 0010 9.87v4.263a1 1 0 001.555.832l3.197-2.132a1 1 0 000-1.664z"
              />
              <path
                stroke-linecap="round"
                stroke-linejoin="round"
                stroke-width="2"
                d="M21 12a9 9 0 11-18 0 9 9 0 0118 0z"
              />
            </svg>
            {#if isSendingVideo}
              Sending...
            {:else}
              Video
            {/if}
          </button>
          <div class="h-px bg-slate-700/50"></div>
          <button
            on:click={pickDocument}
            class="w-full text-left px-4 py-3 text-sm text-slate-200 hover:bg-slate-700 hover:text-white flex items-center gap-3 transition-colors"
            disabled={isSendingDocument}
          >
            <svg
              xmlns="http://www.w3.org/2000/svg"
              class="h-5 w-5 text-blue-400"
              fill="none"
              viewBox="0 0 24 24"
              stroke="currentColor"
            >
              <path
                stroke-linecap="round"
                stroke-linejoin="round"
                stroke-width="2"
                d="M9 12h6m-6 4h6m2 5H7a2 2 0 01-2-2V5a2 2 0 012-2h5.586a1 1 0 01.707.293l5.414 5.414a1 1 0 01.293.707V19a2 2 0 01-2 2z"
              />
            </svg>
            {#if isSendingDocument}
              Sending...
            {:else}
              Document
            {/if}
          </button>
          <div class="h-px bg-slate-700/50"></div>
          <button
            class="w-full text-left px-4 py-3 text-sm text-slate-200 hover:bg-slate-700 hover:text-white flex items-center gap-3 transition-colors"
          >
            <svg
              xmlns="http://www.w3.org/2000/svg"
              class="h-5 w-5 text-pink-400"
              fill="none"
              viewBox="0 0 24 24"
              stroke="currentColor"
            >
              <path
                stroke-linecap="round"
                stroke-linejoin="round"
                stroke-width="2"
                d="M19 11a7 7 0 01-7 7m0 0a7 7 0 01-7-7m7 7v4m0 0H8m4 0h4m-4-8a3 3 0 01-3-3V5a3 3 0 116 0v6a3 3 0 01-3 3z"
              />
            </svg>
            Audio
          </button>
        </div>
      {/if}
    </div>

    <textarea
      bind:this={textarea}
      bind:value={message}
      on:keydown={handleKeydown}
      on:input={handleInput}
      placeholder={`Message ${activePeer}...`}
      rows="1"
      class="flex-1 bg-transparent text-slate-100 placeholder:text-slate-600 px-4 py-2.5 focus:outline-none min-w-0 resize-none overflow-hidden max-h-32 self-end mb-1"
    ></textarea>

    <button
      on:click={sendMessage}
      class="bg-teal-500 hover:bg-teal-400 text-slate-950 p-2.5 rounded-xl font-semibold transition-all hover:scale-105 active:scale-95 shadow-lg shadow-teal-500/20 disabled:opacity-50 disabled:cursor-not-allowed"
      disabled={!message.trim() && pendingImages.length === 0}
    >
      <svg
        xmlns="http://www.w3.org/2000/svg"
        viewBox="0 0 20 20"
        fill="currentColor"
        class="w-5 h-5"
      >
        <path
          d="M3.105 2.289a.75.75 0 00-.826.95l1.414 4.925A1.5 1.5 0 005.135 9.25h6.115a.75.75 0 010 1.5H5.135a1.5 1.5 0 00-1.442 1.086l-1.414 4.926a.75.75 0 00.826.95 28.896 28.896 0 0015.293-7.154.75.75 0 000-1.115A28.897 28.897 0 003.105 2.289z"
        />
      </svg>
    </button>
  </div>
</div>
