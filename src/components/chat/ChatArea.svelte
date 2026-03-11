<script lang="ts">
  import { onDestroy, onMount, tick } from "svelte";
  import { open } from "@tauri-apps/plugin-dialog";
  import {
    BaseDirectory,
    mkdir,
    readDir,
    remove,
    writeFile,
  } from "@tauri-apps/plugin-fs";
  import { appCacheDir, join } from "@tauri-apps/api/path";
  import MessageBubble from "./MessageBubble.svelte";
  import StickerPicker from "./StickerPicker.svelte";
  import { api, type SentMediaResult } from "$lib/tauri/api";
  import { getChatKind } from "$lib/chatKind";

  // Types
  type Message = {
    sender: string;
    text: string;
    timestamp: Date;
    content_type?: string;
    file_hash?: string | null;
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

  $: chatKind = getChatKind(activePeer);
  $: isGroupChat = chatKind === "group";
  $: isArchivedChat = chatKind === "archived";

  // Helper to truncate ID
  function truncateId(id: string, maxLen = 15): string {
    if (id.length <= maxLen) return id;
    return id.substring(0, maxLen) + "...";
  }

  // Callback props
  export let onsend = (msg: string) => {};
  export let ontoggleAttachments = (show: boolean) => {};
  export let onImageSent = (_result: SentMediaResult) => {};
  export let onDocumentSent = (_result: SentMediaResult, _fileName: string) => {};
  export let onVideoSent = (_result: SentMediaResult, _fileName: string) => {};
  export let onAudioSent = (_result: SentMediaResult, _fileName: string) => {};
  export let onStickerSent = (_result: SentMediaResult) => {};

  type RecorderState = "idle" | "recording" | "recorded_pending" | "sending";
  const RECORDING_TMP_DIR = "recordings/tmp";
  const MAX_RECORDING_SECONDS = 60 * 60;
  const MAX_RECORDING_BYTES = 100 * 1024 * 1024;

  let recorderState: RecorderState = "idle";
  let recorderDisabledReason: string | null = null;
  let recordingError: string | null = null;
  let recordingDurationSec = 0;
  let recordingSizeBytes = 0;
  let recordingMimeType = "audio/webm";
  let recordedBlob: Blob | null = null;
  let recordedPreviewUrl: string | null = null;
  let recordedTempRelativePath: string | null = null;
  let recordedTempAbsolutePath: string | null = null;
  let mediaRecorder: MediaRecorder | null = null;
  let recordingStream: MediaStream | null = null;
  let recordingStartedAtMs = 0;
  let recordingTicker: ReturnType<typeof setInterval> | null = null;
  let discardWhenStopping = false;

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
    if (isArchivedChat) return;
    if (recorderState !== "idle") return;

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
    // Send pending audios
    if (pendingAudios.length > 0) {
      sendPendingAudios();
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
    if (isArchivedChat) return;
    if (recorderState !== "idle") return;
    showAttachments = !showAttachments;
    if (showAttachments) {
      showStickerPicker = false;
    }
    ontoggleAttachments(showAttachments);
  }

  let showStickerPicker = false;
  let isSendingSticker = false;

  function toggleStickerPicker() {
    if (isArchivedChat) return;
    if (recorderState !== "idle") return;
    showStickerPicker = !showStickerPicker;
    if (showStickerPicker) {
      showAttachments = false;
      ontoggleAttachments(false);
    }
  }

  async function handleSelectSticker(fileHash: string) {
    if (isSendingSticker) return;
    isSendingSticker = true;
    try {
      const result = await api.sendStickerMessage(activePeer, fileHash);
      onStickerSent(result);
      showStickerPicker = false;
    } catch (e) {
      console.error("Failed to send sticker:", e);
    } finally {
      isSendingSticker = false;
    }
  }

  function handleInput(e: Event) {
    const target = e.currentTarget as HTMLTextAreaElement;
    target.style.height = "auto";
    target.style.height = target.scrollHeight + "px";
  }

  function formatDuration(totalSeconds: number): string {
    const seconds = Math.max(totalSeconds, 0);
    const hh = Math.floor(seconds / 3600);
    const mm = Math.floor((seconds % 3600) / 60);
    const ss = seconds % 60;
    if (hh > 0) {
      return `${String(hh).padStart(2, "0")}:${String(mm).padStart(2, "0")}:${String(ss).padStart(2, "0")}`;
    }
    return `${String(mm).padStart(2, "0")}:${String(ss).padStart(2, "0")}`;
  }

  function formatBytes(bytes: number): string {
    if (bytes >= 1024 * 1024) return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
    if (bytes >= 1024) return `${(bytes / 1024).toFixed(1)} KB`;
    return `${bytes} B`;
  }

  function canUseRecorderApi(): boolean {
    if (typeof window === "undefined") return false;
    return Boolean(
      window.MediaRecorder &&
        navigator?.mediaDevices &&
        navigator.mediaDevices.getUserMedia
    );
  }

  function chooseRecorderMimeType(): string {
    const candidates = [
      "audio/webm;codecs=opus",
      "audio/webm",
      "audio/ogg;codecs=opus",
      "audio/ogg",
    ];

    if (!window.MediaRecorder || !window.MediaRecorder.isTypeSupported) {
      return "audio/webm";
    }

    for (const mimeType of candidates) {
      if (window.MediaRecorder.isTypeSupported(mimeType)) {
        return mimeType;
      }
    }
    return "audio/webm";
  }

  function recordingExtensionFromMime(mimeType: string): string {
    return mimeType.includes("ogg") ? "ogg" : "webm";
  }

  function stopRecordingTicker() {
    if (recordingTicker) {
      clearInterval(recordingTicker);
      recordingTicker = null;
    }
  }

  function stopRecordingStream() {
    if (!recordingStream) return;
    for (const track of recordingStream.getTracks()) {
      track.stop();
    }
    recordingStream = null;
  }

  function clearRecordedPreviewUrl() {
    if (!recordedPreviewUrl) return;
    URL.revokeObjectURL(recordedPreviewUrl);
    recordedPreviewUrl = null;
  }

  async function cleanupTempRecording() {
    if (!recordedTempRelativePath) return;
    try {
      await remove(recordedTempRelativePath, { baseDir: BaseDirectory.AppCache });
    } catch (err) {
      console.debug("Temp recording cleanup skipped:", err);
    } finally {
      recordedTempRelativePath = null;
      recordedTempAbsolutePath = null;
    }
  }

  async function resetRecordedState(removeTemp = true) {
    recordedBlob = null;
    recordingDurationSec = 0;
    recordingSizeBytes = 0;
    recordingMimeType = "audio/webm";
    clearRecordedPreviewUrl();
    if (removeTemp) {
      await cleanupTempRecording();
    }
  }

  async function cleanupStaleTempRecordings() {
    try {
      const entries = await readDir(RECORDING_TMP_DIR, {
        baseDir: BaseDirectory.AppCache,
      });
      for (const entry of entries) {
        const entryPath = `${RECORDING_TMP_DIR}/${entry.name}`;
        await remove(entryPath, {
          baseDir: BaseDirectory.AppCache,
          recursive: entry.isDirectory,
        });
      }
    } catch {
      // Folder may not exist yet; ignore.
    }
  }

  async function persistRecordedBlobToTemp(blob: Blob): Promise<{
    relativePath: string;
    absolutePath: string;
    fileName: string;
  }> {
    await mkdir(RECORDING_TMP_DIR, {
      baseDir: BaseDirectory.AppCache,
      recursive: true,
    });

    const ext = recordingExtensionFromMime(blob.type || recordingMimeType);
    const randomId =
      typeof crypto !== "undefined" && "randomUUID" in crypto
        ? crypto.randomUUID()
        : `${Date.now()}-${Math.floor(Math.random() * 1_000_000)}`;
    const fileName = `recording-${randomId}.${ext}`;
    const relativePath = `${RECORDING_TMP_DIR}/${fileName}`;
    const bytes = new Uint8Array(await blob.arrayBuffer());

    await writeFile(relativePath, bytes, { baseDir: BaseDirectory.AppCache });
    const cacheRoot = await appCacheDir();
    const absolutePath = await join(cacheRoot, relativePath);
    return { relativePath, absolutePath, fileName };
  }

  function stopRecording() {
    if (recorderState !== "recording" || !mediaRecorder) return;
    try {
      mediaRecorder.stop();
    } catch (err) {
      console.error("Failed to stop recorder:", err);
    }
  }

  async function discardRecording() {
    recordingError = null;
    if (recorderState === "recording") {
      discardWhenStopping = true;
      stopRecording();
      return;
    }

    if (recorderState === "sending") return;

    recorderState = "idle";
    await resetRecordedState(true);
  }

  async function sendRecordedClip() {
    if (recorderState !== "recorded_pending" || !recordedBlob) return;
    recorderState = "sending";
    recordingError = null;

    try {
      let filePath = recordedTempAbsolutePath;
      let fileName = `recording.${recordingExtensionFromMime(recordedBlob.type || recordingMimeType)}`;

      if (!filePath) {
        const persisted = await persistRecordedBlobToTemp(recordedBlob);
        recordedTempRelativePath = persisted.relativePath;
        recordedTempAbsolutePath = persisted.absolutePath;
        filePath = persisted.absolutePath;
        fileName = persisted.fileName;
      }

      const result = await api.sendAudioMessage(activePeer, filePath);
      onAudioSent(result, fileName);

      await resetRecordedState(true);
      recorderState = "idle";
    } catch (err: any) {
      console.error("Failed to send recorded audio:", err);
      recorderState = "recorded_pending";
      recordingError = err?.toString?.() || "Failed to send recorded audio";
    }
  }

  async function startRecording() {
    if (isArchivedChat) return;
    if (recorderState !== "idle" && recorderState !== "recorded_pending") return;
    if (recorderDisabledReason) return;

    recordingError = null;
    showStickerPicker = false;
    showAttachments = false;
    ontoggleAttachments(false);

    await resetRecordedState(true);

    if (!canUseRecorderApi()) {
      recorderDisabledReason = "Recording is not supported on this device.";
      return;
    }

    try {
      const stream = await navigator.mediaDevices.getUserMedia({ audio: true });
      const preferredMimeType = chooseRecorderMimeType();
      const recorder = preferredMimeType
        ? new MediaRecorder(stream, { mimeType: preferredMimeType })
        : new MediaRecorder(stream);
      const chunks: BlobPart[] = [];

      recordingStream = stream;
      mediaRecorder = recorder;
      recordingSizeBytes = 0;
      recordingDurationSec = 0;
      recordingMimeType = recorder.mimeType || preferredMimeType || "audio/webm";
      discardWhenStopping = false;

      recorder.ondataavailable = (event: BlobEvent) => {
        if (!event.data || event.data.size === 0) return;
        chunks.push(event.data);
        recordingSizeBytes += event.data.size;

        if (
          recordingSizeBytes >= MAX_RECORDING_BYTES &&
          recorderState === "recording"
        ) {
          recordingError = `Recording reached ${formatBytes(MAX_RECORDING_BYTES)} limit.`;
          stopRecording();
        }
      };

      recorder.onerror = (event: Event) => {
        console.error("MediaRecorder error:", event);
        stopRecordingTicker();
        stopRecordingStream();
        mediaRecorder = null;
        recorderState = "idle";
        recordingError = "Recording failed. Please try again.";
      };

      recorder.onstop = async () => {
        stopRecordingTicker();
        stopRecordingStream();
        mediaRecorder = null;

        if (discardWhenStopping) {
          discardWhenStopping = false;
          recorderState = "idle";
          await resetRecordedState(true);
          return;
        }

        if (chunks.length === 0) {
          recorderState = "idle";
          recordingError = recordingError || "No audio was captured.";
          return;
        }

        const blob = new Blob(chunks, {
          type: recordingMimeType || "audio/webm",
        });
        if (blob.size > MAX_RECORDING_BYTES) {
          recorderState = "idle";
          recordingError = `Recording exceeds ${formatBytes(MAX_RECORDING_BYTES)} limit.`;
          return;
        }

        recordedBlob = blob;
        recordingSizeBytes = blob.size;
        clearRecordedPreviewUrl();
        recordedPreviewUrl = URL.createObjectURL(blob);
        recorderState = "recorded_pending";
      };

      recorder.start(1000);
      recorderState = "recording";
      recordingStartedAtMs = Date.now();
      recordingTicker = setInterval(() => {
        recordingDurationSec = Math.floor((Date.now() - recordingStartedAtMs) / 1000);
        if (
          recordingDurationSec >= MAX_RECORDING_SECONDS &&
          recorderState === "recording"
        ) {
          recordingError = `Recording reached ${formatDuration(MAX_RECORDING_SECONDS)} limit.`;
          stopRecording();
        }
      }, 1000);
    } catch (err: any) {
      console.error("Failed to start recording:", err);
      recorderDisabledReason =
        "Microphone access is blocked or unavailable on this device.";
      recorderState = "idle";
    }
  }

  async function handleRecorderButton() {
    if (recorderState === "recording") {
      stopRecording();
      return;
    }

    if (recorderState === "idle") {
      await startRecording();
    }
  }

  // Pending images to preview before sending
  type PendingImage = { path: string; name: string; dataUrl?: string };
  let pendingImages: PendingImage[] = [];
  let isSendingImage = false;

  async function pickImage() {
    if (isArchivedChat) return;
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
        const dataUrl = await api.getImageFromPath(filePath as string);
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
        const result = await api.sendImageMessage(activePeer, img.path);
        console.log("Image sent:", result);
        onImageSent(result);
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
    if (isArchivedChat) return;
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
        const result = await api.sendDocumentMessage(activePeer, doc.path);
        console.log("Document sent:", result);
        onDocumentSent(result, doc.name);
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
    if (isArchivedChat) return;
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
        const result = await api.sendVideoMessage(activePeer, vid.path);
        console.log("Video sent:", result);
        onVideoSent(result, vid.name);
      }
      pendingVideos = [];
    } catch (e) {
      console.error("Failed to send video:", e);
    } finally {
      isSendingVideo = false;
    }
  }

  // Pending audios to preview before sending
  type PendingAudio = { path: string; name: string };
  let pendingAudios: PendingAudio[] = [];
  let isSendingAudio = false;

  async function pickAudio() {
    if (isArchivedChat) return;
    try {
      const filePath = await open({
        filters: [
          {
            name: "Audio",
            extensions: ["mp3", "m4a", "wav", "ogg", "webm", "opus"],
          },
        ],
        multiple: false,
        directory: false,
      });

      if (!filePath) return; // User cancelled

      const fileName = (filePath as string).split("/").pop() || "audio";
      const newAudio: PendingAudio = {
        path: filePath as string,
        name: fileName,
      };
      pendingAudios = [...pendingAudios, newAudio];
      showAttachments = false;
      console.log("Audio queued:", filePath);
    } catch (e) {
      console.error("Failed to pick audio:", e);
    }
  }

  function removeAudio(index: number) {
    pendingAudios = pendingAudios.filter((_, i) => i !== index);
  }

  async function sendPendingAudios() {
    if (pendingAudios.length === 0) return;
    if (isSendingAudio) return;

    isSendingAudio = true;
    try {
      for (const audio of pendingAudios) {
        console.log("Sending audio:", audio.path);
        const result = await api.sendAudioMessage(activePeer, audio.path);
        console.log("Audio sent:", result);
        onAudioSent(result, audio.name);
      }
      pendingAudios = [];
    } catch (e) {
      console.error("Failed to send audio:", e);
    } finally {
      isSendingAudio = false;
    }
  }

  onMount(() => {
    if (!canUseRecorderApi()) {
      recorderDisabledReason = "Recording is not supported on this device.";
    }
    void cleanupStaleTempRecordings();
  });

  onDestroy(() => {
    stopRecordingTicker();
    if (recorderState === "recording") {
      discardWhenStopping = true;
      stopRecording();
    }
    stopRecordingStream();
    clearRecordedPreviewUrl();
    void cleanupTempRecording();
  });

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
    <span class="text-xl font-bold text-theme-base-100">
      {#if activePeer === "Me"}
        Me (You)
      {:else}
        {peerAlias || activePeer}
      {/if}
    </span>
    {#if activePeer !== "Me" && !isGroupChat}
      <span class="text-xs text-theme-base-500 ml-2"
        >@ {truncateId(activePeer)}</span
      >
    {/if}
    {#if activePeer !== "Me" && !isGroupChat}
      <div
        class="w-2 h-2 rounded-full bg-theme-success-500 shadow-lg shadow-green-500/50"
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
      class="flex flex-col items-center justify-center h-full text-theme-base-500 space-y-4 opacity-0 animate-fade-in-up"
      style="animation-fill-mode: forwards;"
    >
      <div
        class="w-16 h-16 rounded-2xl bg-theme-base-900 border border-theme-base-800 flex items-center justify-center"
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
  {#if isArchivedChat}
    <div
      class="mb-3 rounded-xl border border-theme-base-700 bg-theme-base-900/70 px-3 py-2 text-xs text-theme-base-400"
    >
      Archived transcript is read-only.
    </div>
  {/if}

  <!-- Pending Images Preview -->
  {#if pendingImages.length > 0}
    <div
      class="mb-3 flex gap-2 flex-wrap bg-slate-900/60 border border-slate-700/50 rounded-xl p-3"
    >
      {#each pendingImages as img, index}
        <div class="relative group">
          <div
            class="w-16 h-16 bg-theme-base-800 rounded-lg flex items-center justify-center overflow-hidden border border-theme-base-600 relative"
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
                class="w-8 h-8 text-theme-secondary-400"
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
            onclick={() => removeImage(index)}
            class="absolute -top-2 -right-2 w-5 h-5 bg-theme-error-500 hover:bg-theme-error-400 text-white rounded-full flex items-center justify-center text-xs opacity-0 group-hover:opacity-100 transition-opacity"
          >
            ×
          </button>
          <p class="text-xs text-theme-base-400 mt-1 truncate w-16 text-center">
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
          class="relative group flex items-center gap-2 bg-theme-base-800 rounded-lg p-2 pr-8 border border-theme-base-600"
        >
          <span class="text-xl">
            {#if doc.name.endsWith(".pdf")}📕
            {:else if doc.name.endsWith(".doc") || doc.name.endsWith(".docx")}📘
            {:else if doc.name.endsWith(".xls") || doc.name.endsWith(".xlsx")}📗
            {:else if doc.name.endsWith(".ppt") || doc.name.endsWith(".pptx")}📙
            {:else}📄
            {/if}
          </span>
          <span class="text-xs text-theme-base-300 truncate max-w-[120px]"
            >{doc.name}</span
          >
          <button
            onclick={() => removeDocument(index)}
            class="absolute top-1 right-1 w-5 h-5 bg-theme-error-500 hover:bg-theme-error-400 text-white rounded-full flex items-center justify-center text-xs opacity-0 group-hover:opacity-100 transition-opacity"
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
            class="w-20 h-14 bg-theme-base-800 rounded-lg flex items-center justify-center overflow-hidden border border-theme-base-600 relative"
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
                class="w-8 h-8 text-theme-secondary-400"
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
            onclick={() => removeVideo(index)}
            class="absolute -top-2 -right-2 w-5 h-5 bg-theme-error-500 hover:bg-theme-error-400 text-white rounded-full flex items-center justify-center text-xs opacity-0 group-hover:opacity-100 transition-opacity"
            title="Remove video"
          >
            ×
          </button>
          <p class="text-xs text-theme-base-400 mt-1 truncate w-20 text-center">
            {vid.name}
          </p>
        </div>
      {/each}
    </div>
  {/if}

  <!-- Pending Audios Preview -->
  {#if pendingAudios.length > 0}
    <div
      class="mb-3 flex gap-2 flex-wrap bg-slate-900/60 border border-slate-700/50 rounded-xl p-3"
    >
      {#each pendingAudios as audio, index}
        <div
          class="relative group flex items-center gap-2 bg-theme-base-800 rounded-lg p-2 pr-8 border border-theme-base-600"
        >
          <span class="text-xl">🎵</span>
          <span class="text-xs text-theme-base-300 truncate max-w-[160px]"
            >{audio.name}</span
          >
          <button
            onclick={() => removeAudio(index)}
            class="absolute top-1 right-1 w-5 h-5 bg-theme-error-500 hover:bg-theme-error-400 text-white rounded-full flex items-center justify-center text-xs opacity-0 group-hover:opacity-100 transition-opacity"
            title="Remove audio"
          >
            ×
          </button>
        </div>
      {/each}
    </div>
  {/if}

  {#if recorderState !== "idle" || recordingError || recorderDisabledReason}
    <div class="mb-3 bg-slate-900/60 border border-slate-700/50 rounded-xl p-3 text-theme-base-200">
      {#if recorderState === "recording"}
        <div class="flex items-center justify-between gap-3">
          <div class="flex items-center gap-2 text-sm">
            <span class="w-2.5 h-2.5 rounded-full bg-theme-error-500 animate-pulse"></span>
            <span>Recording {formatDuration(recordingDurationSec)}</span>
            <span class="text-theme-base-400">({formatBytes(recordingSizeBytes)})</span>
          </div>
          <div class="flex items-center gap-2">
            <button
              onclick={stopRecording}
              class="px-3 py-1.5 rounded-lg bg-theme-primary-500 text-theme-base-950 text-xs font-semibold hover:bg-theme-primary-400"
            >
              Stop
            </button>
            <button
              onclick={discardRecording}
              class="px-3 py-1.5 rounded-lg bg-theme-base-700 text-theme-base-200 text-xs font-semibold hover:bg-theme-base-600"
            >
              Discard
            </button>
          </div>
        </div>
      {:else if recorderState === "recorded_pending" && recordedBlob}
        <div class="flex flex-col gap-2">
          <div class="flex items-center justify-between gap-3 text-xs text-theme-base-400">
            <span>Recorded clip ready</span>
            <span>{formatDuration(recordingDurationSec)} • {formatBytes(recordedBlob.size)}</span>
          </div>
          {#if recordedPreviewUrl}
            <!-- svelte-ignore a11y_media_has_caption -->
            <audio controls src={recordedPreviewUrl} class="w-full"></audio>
          {/if}
          <div class="flex items-center gap-2">
            <button
              onclick={sendRecordedClip}
              class="px-3 py-1.5 rounded-lg bg-theme-primary-500 text-theme-base-950 text-xs font-semibold hover:bg-theme-primary-400"
            >
              Send recording
            </button>
            <button
              onclick={discardRecording}
              class="px-3 py-1.5 rounded-lg bg-theme-base-700 text-theme-base-200 text-xs font-semibold hover:bg-theme-base-600"
            >
              Discard
            </button>
          </div>
        </div>
      {:else if recorderState === "sending"}
        <div class="text-sm text-theme-base-300">Sending recorded audio...</div>
      {/if}

      {#if recordingError}
        <p class="text-xs text-theme-error-400 mt-2">{recordingError}</p>
      {/if}
      {#if recorderDisabledReason}
        <p class="text-xs text-theme-warning-400 mt-2">{recorderDisabledReason}</p>
      {/if}
    </div>
  {/if}

  <div
    class="bg-theme-base-900/90 backdrop-blur-md border border-theme-base-700 rounded-2xl p-1.5 shadow-2xl flex items-center gap-2 relative"
  >
    <div class="relative">
      <button
        onclick={toggleStickerPicker}
        class={`p-2 rounded-xl transition-all ${showStickerPicker ? "bg-theme-base-700 text-theme-primary-400" : "text-theme-base-400 hover:text-white hover:bg-theme-base-800"} disabled:opacity-50 disabled:cursor-not-allowed`}
        title="Open sticker picker"
        disabled={isArchivedChat || isSendingSticker || recorderState !== "idle"}
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
            d="M14 10H3m3-6h11l4 4v11a2 2 0 01-2 2h-5M8 16l3 3 5-5"
          />
        </svg>
      </button>

      {#if showStickerPicker}
        <StickerPicker
          onclose={() => (showStickerPicker = false)}
          onselectsticker={handleSelectSticker}
        />
      {/if}
    </div>

    <button
      onclick={handleRecorderButton}
      class={`p-2 rounded-xl transition-all disabled:opacity-50 disabled:cursor-not-allowed ${recorderState === "recording" ? "bg-theme-error-500/20 text-theme-error-400" : "text-theme-base-400 hover:text-white hover:bg-theme-base-800"}`}
      title={
        recorderDisabledReason
          ? recorderDisabledReason
          : recorderState === "recording"
            ? "Stop recording"
            : "Start recording"
      }
      disabled={
        isArchivedChat ||
        Boolean(recorderDisabledReason) ||
        recorderState === "sending" ||
        recorderState === "recorded_pending"
      }
      aria-label={recorderState === "recording" ? "Stop recording" : "Start recording"}
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
          d="M12 1.75a3.25 3.25 0 00-3.25 3.25v6a3.25 3.25 0 106.5 0V5A3.25 3.25 0 0012 1.75zM5.75 10.75a.75.75 0 011.5 0 4.75 4.75 0 009.5 0 .75.75 0 011.5 0 6.25 6.25 0 01-5.5 6.21V20h2a.75.75 0 010 1.5h-6a.75.75 0 010-1.5h2v-3.04a6.25 6.25 0 01-5.5-6.21z"
        />
      </svg>
    </button>

    <!-- Attachments Button -->
    <div class="relative">
      <button
        onclick={toggleAttachments}
        class={`p-2 rounded-xl transition-all ${showAttachments ? "bg-theme-base-700 text-theme-primary-400" : "text-theme-base-400 hover:text-white hover:bg-theme-base-800"}`}
        title="Add Attachment"
        disabled={isArchivedChat || recorderState !== "idle"}
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
          class="absolute bottom-full left-0 mb-2 w-48 bg-theme-base-800 border border-theme-base-700 rounded-xl shadow-xl overflow-hidden z-50 animate-fade-in-up"
        >
          <button
            onclick={pickImage}
            class="w-full text-left px-4 py-3 text-sm text-theme-base-200 hover:bg-theme-base-700 hover:text-white flex items-center gap-3 transition-colors"
            disabled={isSendingImage}
          >
            <svg
              xmlns="http://www.w3.org/2000/svg"
              class="h-5 w-5 text-theme-secondary-400"
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
            onclick={pickVideo}
            class="w-full text-left px-4 py-3 text-sm text-theme-base-200 hover:bg-theme-base-700 hover:text-white flex items-center gap-3 transition-colors"
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
            onclick={pickDocument}
            class="w-full text-left px-4 py-3 text-sm text-theme-base-200 hover:bg-theme-base-700 hover:text-white flex items-center gap-3 transition-colors"
            disabled={isSendingDocument}
          >
            <svg
              xmlns="http://www.w3.org/2000/svg"
              class="h-5 w-5 text-theme-info-400"
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
            onclick={pickAudio}
            class="w-full text-left px-4 py-3 text-sm text-theme-base-200 hover:bg-theme-base-700 hover:text-white flex items-center gap-3 transition-colors"
            disabled={isSendingAudio}
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
            {#if isSendingAudio}
              Sending...
            {:else}
              Audio
            {/if}
          </button>
        </div>
      {/if}
    </div>

    <textarea
      bind:this={textarea}
      bind:value={message}
      onkeydown={handleKeydown}
      oninput={handleInput}
      placeholder={isArchivedChat ? "Archived chat is read-only" : `Message ${activePeer}...`}
      rows="1"
      class="flex-1 bg-transparent text-theme-base-100 placeholder:text-theme-base-600 px-4 py-2.5 focus:outline-none min-w-0 resize-none overflow-hidden max-h-32 self-end mb-1"
      readonly={isArchivedChat}
    ></textarea>

    <button
      onclick={sendMessage}
      class="bg-theme-primary-500 hover:bg-theme-primary-400 text-theme-base-950 p-2.5 rounded-xl font-semibold transition-all hover:scale-105 active:scale-95 shadow-lg shadow-teal-500/20 disabled:opacity-50 disabled:cursor-not-allowed"
      disabled={
        isArchivedChat ||
        recorderState !== "idle" ||
        !message.trim() &&
        pendingImages.length === 0 &&
        pendingDocuments.length === 0 &&
        pendingVideos.length === 0 &&
        pendingAudios.length === 0
      }
      aria-label="Send message"
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
