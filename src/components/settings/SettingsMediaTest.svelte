<script lang="ts">
  import { onDestroy, onMount } from "svelte";
  import { api } from "$lib/tauri/api";
  import {
    currentMediaSupport,
    describeMediaError,
    formatAudioLevel,
    mediaSupportRows,
    type MediaSupportRow,
  } from "$lib/media/testDiagnostics";

  type TestMode = "audio-video" | "audio" | "video";

  let { onback = () => {} } = $props();

  let previewVideo = $state<HTMLVideoElement | null>(null);
  let stream = $state<MediaStream | null>(null);
  let supportRows = $state<MediaSupportRow[]>([]);
  let devices = $state<MediaDeviceInfo[]>([]);
  let status = $state<"idle" | "starting" | "running">("idle");
  let error = $state("");
  let audioLevel = $state(0);
  let activeAudioTrack = $state("");
  let activeVideoTrack = $state("");
  let secureContext = $state(false);

  let audioContext: AudioContext | null = null;
  let analyser: AnalyserNode | null = null;
  let audioSource: MediaStreamAudioSourceNode | null = null;
  let meterFrame = 0;
  let testRunSeq = 0;

  let hasAudioTrack = $derived(!!stream?.getAudioTracks().length);
  let hasVideoTrack = $derived(!!stream?.getVideoTracks().length);

  onMount(async () => {
    refreshSupport();
    await refreshDevices();
  });

  onDestroy(() => {
    stopTest(false);
  });

  function goBack() {
    stopTest(false);
    onback();
  }

  function refreshSupport() {
    supportRows = mediaSupportRows(currentMediaSupport());
    secureContext = typeof window !== "undefined" && window.isSecureContext;
  }

  async function refreshDevices() {
    if (typeof navigator === "undefined" || !navigator.mediaDevices?.enumerateDevices) {
      devices = [];
      return;
    }

    try {
      devices = (await navigator.mediaDevices.enumerateDevices()).filter(
        (device) => device.kind === "audioinput" || device.kind === "videoinput",
      );
    } catch (e) {
      logMediaTest("enumerate devices failed", { error: describeMediaError(e) });
    }
  }

  function constraintsFor(mode: TestMode): MediaStreamConstraints {
    return {
      audio:
        mode === "audio" || mode === "audio-video"
          ? {
              echoCancellation: true,
              noiseSuppression: true,
              autoGainControl: true,
            }
          : false,
      video:
        mode === "video" || mode === "audio-video"
          ? {
              width: { ideal: 1280 },
              height: { ideal: 720 },
              frameRate: { ideal: 30, max: 30 },
            }
          : false,
    };
  }

  async function startTest(mode: TestMode) {
    stopTest(false, false);
    const runId = ++testRunSeq;
    refreshSupport();
    status = "starting";
    error = "";
    activeAudioTrack = "";
    activeVideoTrack = "";
    audioLevel = 0;

    if (!navigator.mediaDevices?.getUserMedia) {
      error = "getUserMedia is not available in this WebView.";
      status = "idle";
      logMediaTest("start failed", { mode, error });
      return;
    }

    try {
      const nextStream = await navigator.mediaDevices.getUserMedia(
        constraintsFor(mode),
      );
      if (runId !== testRunSeq) {
        stopMediaStream(nextStream);
        return;
      }

      stream = nextStream;
      activeAudioTrack = describeTrack(nextStream.getAudioTracks()[0]);
      activeVideoTrack = describeTrack(nextStream.getVideoTracks()[0]);

      if (previewVideo && nextStream.getVideoTracks().length > 0) {
        previewVideo.srcObject = nextStream;
        await previewVideo.play().catch((e) => {
          logMediaTest("preview play failed", { error: describeMediaError(e) });
        });
      }
      if (runId !== testRunSeq) return;

      if (nextStream.getAudioTracks().length > 0) {
        startAudioMeter(nextStream);
      }

      await refreshDevices();
      if (runId !== testRunSeq) return;
      status = "running";
      logMediaTest("started", {
        mode,
        audio: activeAudioTrack || "none",
        video: activeVideoTrack || "none",
      });
    } catch (e) {
      if (runId !== testRunSeq) return;
      error = describeMediaError(e);
      status = "idle";
      logMediaTest("start failed", { mode, error });
      stopTest(false, false);
    }
  }

  function stopTest(emitLog = true, invalidatePending = true) {
    if (invalidatePending) {
      testRunSeq += 1;
    }

    if (meterFrame) {
      cancelAnimationFrame(meterFrame);
      meterFrame = 0;
    }

    audioSource?.disconnect();
    analyser?.disconnect();
    audioSource = null;
    analyser = null;

    if (audioContext) {
      void audioContext.close().catch(() => {});
      audioContext = null;
    }

    if (previewVideo) {
      previewVideo.pause();
      previewVideo.srcObject = null;
    }

    stopMediaStream(stream);
    stream = null;
    status = "idle";
    audioLevel = 0;
    activeAudioTrack = "";
    activeVideoTrack = "";

    if (emitLog) {
      logMediaTest("stopped");
    }
  }

  function stopMediaStream(mediaStream: MediaStream | null) {
    mediaStream?.getTracks().forEach((track) => track.stop());
  }

  function startAudioMeter(nextStream: MediaStream) {
    const AudioContextCtor =
      window.AudioContext || (window as typeof window & { webkitAudioContext?: typeof AudioContext }).webkitAudioContext;

    if (!AudioContextCtor) {
      logMediaTest("audio meter unavailable", { reason: "AudioContext missing" });
      return;
    }

    audioContext = new AudioContextCtor();
    analyser = audioContext.createAnalyser();
    analyser.fftSize = 512;
    audioSource = audioContext.createMediaStreamSource(nextStream);
    audioSource.connect(analyser);

    const samples = new Uint8Array(analyser.fftSize);
    const tick = () => {
      if (!analyser) return;
      analyser.getByteTimeDomainData(samples);
      let sum = 0;
      for (const sample of samples) {
        const centered = (sample - 128) / 128;
        sum += centered * centered;
      }
      audioLevel = Math.min(1, Math.sqrt(sum / samples.length) * 4);
      meterFrame = requestAnimationFrame(tick);
    };
    meterFrame = requestAnimationFrame(tick);
  }

  function describeTrack(track: MediaStreamTrack | undefined): string {
    if (!track) return "";
    const settings = track.getSettings();
    const details =
      track.kind === "video"
        ? [settings.width, settings.height].every(Boolean)
          ? `${settings.width}x${settings.height} @ ${settings.frameRate ?? "?"}fps`
          : ""
        : settings.sampleRate
          ? `${settings.sampleRate} Hz`
          : "";
    return [track.label || "Unnamed device", details].filter(Boolean).join(" · ");
  }

  function deviceLabel(device: MediaDeviceInfo): string {
    return device.label || "Device label hidden until permission is granted";
  }

  function logMediaTest(message: string, data?: Record<string, unknown>) {
    const details = data ? ` ${JSON.stringify(data)}` : "";
    const line = `[MediaTest] ${message}${details}`;
    console.log(line);
    void api.frontendLog(line).catch(() => {});
  }
</script>

<div class="mb-6 flex items-center gap-4 border-b border-slate-800/50 pb-4">
  <button
    onclick={goBack}
    class="p-2 hover:bg-theme-base-800 rounded-lg text-theme-base-400 hover:text-white transition-colors"
    aria-label="Go Back"
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
  <h2 class="text-xl font-bold text-theme-base-100">Test Voice / Camera</h2>
</div>

<div class="space-y-5 animate-fade-in-up">
  <div class="rounded-xl border border-theme-base-800 bg-theme-base-900 p-4">
    <div class="flex flex-wrap gap-2">
      <button
        onclick={() => startTest("audio-video")}
        disabled={status === "starting"}
        class="rounded-lg bg-theme-primary-600 px-3 py-2 text-sm font-semibold text-white hover:bg-theme-primary-500 disabled:opacity-50"
      >
        Test Camera + Mic
      </button>
      <button
        onclick={() => startTest("audio")}
        disabled={status === "starting"}
        class="rounded-lg border border-theme-base-700 bg-theme-base-800 px-3 py-2 text-sm text-theme-base-100 hover:bg-theme-base-700 disabled:opacity-50"
      >
        Mic Only
      </button>
      <button
        onclick={() => startTest("video")}
        disabled={status === "starting"}
        class="rounded-lg border border-theme-base-700 bg-theme-base-800 px-3 py-2 text-sm text-theme-base-100 hover:bg-theme-base-700 disabled:opacity-50"
      >
        Camera Only
      </button>
      <button
        onclick={() => stopTest()}
        disabled={status === "idle"}
        class="rounded-lg border border-theme-base-700 px-3 py-2 text-sm text-theme-base-300 hover:bg-theme-base-800 disabled:opacity-40"
      >
        Stop
      </button>
    </div>

    {#if error}
      <div class="mt-3 rounded-lg border border-theme-error-500/40 bg-theme-error-500/10 px-3 py-2 text-sm text-theme-error-300">
        {error}
      </div>
    {/if}
  </div>

  <div class="grid grid-cols-1 xl:grid-cols-[minmax(0,1.4fr)_minmax(320px,0.8fr)] gap-5">
    <div class="rounded-xl border border-theme-base-800 bg-theme-base-900 p-4">
      <div class="mb-3 flex items-center justify-between">
        <h3 class="text-sm font-semibold text-theme-base-200">Camera</h3>
        <span class={`text-xs ${hasVideoTrack ? "text-theme-success-400" : "text-theme-base-500"}`}>
          {hasVideoTrack ? "Active" : "Inactive"}
        </span>
      </div>
      <div class="aspect-video overflow-hidden rounded-lg border border-theme-base-800 bg-theme-base-950">
        <video
          bind:this={previewVideo}
          autoplay
          muted
          playsinline
          class={`h-full w-full object-cover ${hasVideoTrack ? "" : "hidden"}`}
        ></video>
        {#if !hasVideoTrack}
          <div class="flex h-full items-center justify-center text-sm text-theme-base-500">
            No camera preview
          </div>
        {/if}
      </div>
      {#if activeVideoTrack}
        <div class="mt-3 text-xs text-theme-base-400">{activeVideoTrack}</div>
      {/if}
    </div>

    <div class="space-y-5">
      <div class="rounded-xl border border-theme-base-800 bg-theme-base-900 p-4">
        <div class="mb-3 flex items-center justify-between">
          <h3 class="text-sm font-semibold text-theme-base-200">Microphone</h3>
          <span class={`text-xs ${hasAudioTrack ? "text-theme-success-400" : "text-theme-base-500"}`}>
            {hasAudioTrack ? "Active" : "Inactive"}
          </span>
        </div>
        <div class="h-3 overflow-hidden rounded-full bg-theme-base-800">
          <div
            class="h-full rounded-full bg-theme-success-500 transition-[width]"
            style={`width: ${formatAudioLevel(audioLevel)}`}
          ></div>
        </div>
        <div class="mt-2 text-xs text-theme-base-500">
          Level: <span class="text-theme-base-300">{formatAudioLevel(audioLevel)}</span>
        </div>
        {#if activeAudioTrack}
          <div class="mt-3 text-xs text-theme-base-400">{activeAudioTrack}</div>
        {/if}
      </div>

      <div class="rounded-xl border border-theme-base-800 bg-theme-base-900 p-4">
        <h3 class="mb-3 text-sm font-semibold text-theme-base-200">WebView Support</h3>
        <div class="space-y-2">
          {#each supportRows as row}
            <div class="flex items-center justify-between text-xs">
              <span class="text-theme-base-400">{row.label}</span>
              <span class={row.ok ? "text-theme-success-400" : "text-theme-error-400"}>
                {row.ok ? "OK" : "Missing"}
              </span>
            </div>
          {/each}
          <div class="flex items-center justify-between text-xs">
            <span class="text-theme-base-400">secure context</span>
            <span class={secureContext ? "text-theme-success-400" : "text-theme-error-400"}>
              {secureContext ? "OK" : "Missing"}
            </span>
          </div>
        </div>
      </div>
    </div>
  </div>

  <div class="rounded-xl border border-theme-base-800 bg-theme-base-900 p-4">
    <h3 class="mb-3 text-sm font-semibold text-theme-base-200">Detected Devices</h3>
    {#if devices.length === 0}
      <div class="text-sm text-theme-base-500">No devices reported.</div>
    {:else}
      <div class="space-y-2">
        {#each devices as device}
          <div class="flex items-center justify-between gap-3 rounded-lg bg-theme-base-950 px-3 py-2 text-xs">
            <span class="text-theme-base-300">{deviceLabel(device)}</span>
            <span class="shrink-0 text-theme-base-500">
              {device.kind === "audioinput" ? "Microphone" : "Camera"}
            </span>
          </div>
        {/each}
      </div>
    {/if}
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
