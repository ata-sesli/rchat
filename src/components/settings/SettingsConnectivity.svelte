<script lang="ts">
  import { onMount } from "svelte";
  import {
    api,
    type ConnectivityMode,
    type ConnectivitySettings,
  } from "$lib/tauri/api";

  let { onback = () => {} } = $props();

  let settings = $state<ConnectivitySettings>({
    mode: "reachable",
    mdns_enabled: true,
    github_sync_enabled: true,
    nat_keepalive_enabled: true,
    punch_assist_enabled: true,
  });
  let loading = $state(false);
  let error = $state("");

  onMount(async () => {
    await refresh();
  });

  function emitConnectivityUpdated(next: ConnectivitySettings) {
    window.dispatchEvent(
      new CustomEvent("connectivity-updated", { detail: next }),
    );
  }

  async function refresh() {
    try {
      settings = await api.getConnectivitySettings();
    } catch (e: any) {
      error = e?.toString?.() ?? "Failed to load connectivity settings";
    }
  }

  async function applyMode(mode: ConnectivityMode) {
    loading = true;
    error = "";
    try {
      const next = await api.setConnectivityMode(mode);
      settings = next;
      emitConnectivityUpdated(next);
    } catch (e: any) {
      error = e?.toString?.() ?? "Failed to apply mode";
    } finally {
      loading = false;
    }
  }

  async function updateFlag(
    key:
      | "mdns_enabled"
      | "github_sync_enabled"
      | "nat_keepalive_enabled"
      | "punch_assist_enabled",
    value: boolean,
  ) {
    loading = true;
    error = "";
    try {
      const patch: Record<string, boolean> = { [key]: value };
      const next = await api.updateConnectivitySettings(patch);
      settings = next;
      emitConnectivityUpdated(next);
    } catch (e: any) {
      error = e?.toString?.() ?? "Failed to update connectivity settings";
    } finally {
      loading = false;
    }
  }

  function modeButtonClass(mode: ConnectivityMode): string {
    const active = settings.mode === mode;
    return active
      ? "border-theme-primary-500 bg-theme-primary-500/15 text-theme-primary-300"
      : "border-theme-base-700 bg-theme-base-900 text-theme-base-300 hover:bg-theme-base-800";
  }
</script>

<div class="mb-6 flex items-center gap-4 border-b border-slate-800/50 pb-4">
  <button
    onclick={() => onback()}
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
  <h2 class="text-xl font-bold text-theme-base-100">Connectivity</h2>
</div>

<div class="space-y-5 animate-fade-in-up">
  <div class="rounded-xl border border-theme-base-800 bg-theme-base-900 p-4">
    <div class="text-sm text-theme-base-400 mb-3">
      Preset mode
    </div>
    <div class="grid grid-cols-1 md:grid-cols-3 gap-2">
      <button
        class={`rounded-lg border px-3 py-2 text-sm transition-colors ${modeButtonClass("invisible")}`}
        onclick={() => applyMode("invisible")}
        disabled={loading}
      >
        Invisible
      </button>
      <button
        class={`rounded-lg border px-3 py-2 text-sm transition-colors ${modeButtonClass("lan")}`}
        onclick={() => applyMode("lan")}
        disabled={loading}
      >
        LAN
      </button>
      <button
        class={`rounded-lg border px-3 py-2 text-sm transition-colors ${modeButtonClass("reachable")}`}
        onclick={() => applyMode("reachable")}
        disabled={loading}
      >
        Reachable
      </button>
    </div>
    <div class="mt-3 text-xs text-theme-base-500">
      Current mode: <span class="text-theme-base-300">{settings.mode}</span>
    </div>
  </div>

  <div class="rounded-xl border border-theme-base-800 bg-theme-base-900 p-4 space-y-3">
    <div class="text-sm text-theme-base-400">Advanced</div>

    <label class="flex items-center justify-between text-sm text-theme-base-200">
      <span>mDNS (scan + advertise)</span>
      <input
        type="checkbox"
        checked={settings.mdns_enabled}
        onchange={(e) =>
          updateFlag("mdns_enabled", (e.currentTarget as HTMLInputElement).checked)}
        disabled={loading}
      />
    </label>

    <label class="flex items-center justify-between text-sm text-theme-base-200">
      <span>GitHub sync (discover + publish)</span>
      <input
        type="checkbox"
        checked={settings.github_sync_enabled}
        onchange={(e) =>
          updateFlag(
            "github_sync_enabled",
            (e.currentTarget as HTMLInputElement).checked,
          )}
        disabled={loading}
      />
    </label>

    <label class="flex items-center justify-between text-sm text-theme-base-200">
      <span>NAT keepalive</span>
      <input
        type="checkbox"
        checked={settings.nat_keepalive_enabled}
        onchange={(e) =>
          updateFlag(
            "nat_keepalive_enabled",
            (e.currentTarget as HTMLInputElement).checked,
          )}
        disabled={loading}
      />
    </label>

    <label class="flex items-center justify-between text-sm text-theme-base-200">
      <span>Punch assist</span>
      <input
        type="checkbox"
        checked={settings.punch_assist_enabled}
        onchange={(e) =>
          updateFlag(
            "punch_assist_enabled",
            (e.currentTarget as HTMLInputElement).checked,
          )}
        disabled={loading}
      />
    </label>
  </div>

  {#if error}
    <div class="text-sm text-theme-error-400">{error}</div>
  {/if}
</div>
