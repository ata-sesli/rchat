<script lang="ts">
  import { onMount } from "svelte";
  import ColorPicker from "svelte-awesome-color-picker";
  import { api, type PresetInfo, type ThemeConfig } from "$lib/tauri/api";

  let { onback = () => {} } = $props();

  type CreationMode = "simple" | "advanced";
  type EditorMode = "create" | "edit";

  const DEFAULT_PRIMARY = "#14b8a6";
  const DEFAULT_SECONDARY = "#a855f7";
  const DEFAULT_TEXT = "#f1f5f9";

  let loading = $state(true);
  let saving = $state(false);
  let errorMessage = $state<string | null>(null);

  let presets = $state<PresetInfo[]>([]);
  let selectedPreset = $state<string | null>(null);
  let appliedTheme = $state<ThemeConfig | null>(null);

  let editorOpen = $state(false);
  let editorMode = $state<EditorMode>("create");
  let creationMode = $state<CreationMode>("simple");
  let editingKey = $state<string | null>(null);

  let draftTheme = $state<ThemeConfig | null>(null);
  let draftName = $state("");
  let draftDescription = $state("");
  let initialSignature = $state("");

  let simplePrimary = $state(DEFAULT_PRIMARY);
  let simpleSecondary = $state(DEFAULT_SECONDARY);
  let simpleText = $state(DEFAULT_TEXT);

  let sidebarColor = $state("#0f172a");
  let chatSectionColor = $state("#020617");
  let peerMessageColor = $state("#1e293b");
  let userMessageColor = $state("#0d9488");
  let editorTextColor = $state(DEFAULT_TEXT);

  let semanticError = $state<ThemeConfig["error"]>({
    "600": "#dc2626",
    "500": "#ef4444",
    "400": "#f87171",
    "300": "#fca5a5",
  });
  let semanticSuccess = $state<ThemeConfig["success"]>({
    "600": "#16a34a",
    "500": "#22c55e",
    "400": "#4ade80",
    "300": "#86efac",
  });
  let semanticInfo = $state<ThemeConfig["info"]>({
    "600": "#2563eb",
    "500": "#3b82f6",
    "400": "#60a5fa",
    "300": "#93c5fd",
  });
  let semanticWarning = $state<ThemeConfig["warning"]>({
    "600": "#d97706",
    "500": "#f59e0b",
    "400": "#fbbf24",
    "300": "#fcd34d",
  });

  const customPresets = $derived(presets.filter((preset) => preset.source === "custom"));

  const isDraftDirty = $derived(editorOpen && editorSignature() !== initialSignature);

  let simpleGenerateVersion = 0;

  onMount(async () => {
    await loadThemeState();
  });

  function sourceBadgeClass(source: PresetInfo["source"]): string {
    return source === "custom"
      ? "bg-emerald-500/20 text-emerald-300 border-emerald-500/40"
      : "bg-slate-700/40 text-slate-300 border-slate-600/50";
  }

  function formatTs(ts?: number | null): string {
    if (!ts) return "";
    try {
      return new Date(ts * 1000).toLocaleDateString();
    } catch {
      return "";
    }
  }

  async function loadThemeState() {
    loading = true;
    errorMessage = null;
    try {
      const [theme, listedPresets, selected] = await Promise.all([
        api.getTheme(),
        api.listThemePresets(),
        api.getSelectedPreset(),
      ]);
      appliedTheme = theme;
      presets = listedPresets;
      selectedPreset = selected;
      applyThemeToCSS(theme);
    } catch (error) {
      errorMessage = `Failed to load theme settings: ${String(error)}`;
    } finally {
      loading = false;
    }
  }

  async function applyPreset(presetKey: string) {
    if (!(await confirmDiscardIfDirty("Discard unsaved draft and apply this preset?"))) {
      return;
    }

    saving = true;
    errorMessage = null;
    try {
      const theme = await api.applyPreset(presetKey);
      appliedTheme = theme;
      selectedPreset = presetKey;
      applyThemeToCSS(theme);
      closeEditor();
    } catch (error) {
      errorMessage = `Failed to apply preset: ${String(error)}`;
    } finally {
      saving = false;
    }
  }

  async function startCreate(mode: CreationMode) {
    if (!(await confirmDiscardIfDirty("Discard current draft and start a new theme?"))) {
      return;
    }

    editorOpen = true;
    editorMode = "create";
    creationMode = mode;
    editingKey = null;
    draftName = "";
    draftDescription = "";

    const baseTheme = appliedTheme ?? defaultTheme();
    loadColorInputsFromTheme(baseTheme);

    if (mode === "simple") {
      simplePrimary = baseTheme.primary["500"];
      simpleSecondary = baseTheme.secondary["500"];
      simpleText = baseTheme.base["100"];
      await regenerateSimpleTheme();
    } else {
      draftTheme = buildAdvancedTheme();
      applyThemeToCSS(draftTheme);
    }

    initialSignature = editorSignature();
  }

  async function startEdit(preset: PresetInfo) {
    if (preset.source !== "custom") return;
    if (!(await confirmDiscardIfDirty("Discard current draft and edit this custom theme?"))) {
      return;
    }

    const theme = preset.theme;
    if (!theme) {
      errorMessage = "Selected custom theme does not include editable theme data.";
      return;
    }

    editorOpen = true;
    editorMode = "edit";
    creationMode = "advanced";
    editingKey = preset.key;
    draftName = preset.name;
    draftDescription = preset.description || "";
    draftTheme = structuredClone(theme);

    loadColorInputsFromTheme(theme);
    simplePrimary = theme.primary["500"];
    simpleSecondary = theme.secondary["500"];
    simpleText = theme.base["100"];

    applyThemeToCSS(theme);
    initialSignature = editorSignature();
  }

  function loadColorInputsFromTheme(theme: ThemeConfig) {
    sidebarColor = theme.base["900"];
    chatSectionColor = theme.base["950"];
    peerMessageColor = theme.base["800"];
    userMessageColor = theme.primary["600"];
    editorTextColor = theme.base["100"];
    semanticError = { ...theme.error };
    semanticSuccess = { ...theme.success };
    semanticInfo = { ...theme.info };
    semanticWarning = { ...theme.warning };
  }

  async function switchCreationMode(mode: CreationMode) {
    if (mode === creationMode) return;

    if (!(await confirmDiscardIfDirty("Switching modes will discard unsaved draft changes. Continue?"))) {
      return;
    }

    creationMode = mode;
    const baseTheme = appliedTheme ?? defaultTheme();
    loadColorInputsFromTheme(baseTheme);
    simplePrimary = baseTheme.primary["500"];
    simpleSecondary = baseTheme.secondary["500"];
    simpleText = baseTheme.base["100"];

    if (mode === "simple") {
      await regenerateSimpleTheme();
    } else {
      draftTheme = buildAdvancedTheme();
      applyThemeToCSS(draftTheme);
    }

    initialSignature = editorSignature();
  }

  async function saveDraftTheme() {
    if (!draftTheme) return;
    const trimmedName = draftName.trim();
    if (!trimmedName) {
      errorMessage = "Theme title is required.";
      return;
    }

    saving = true;
    errorMessage = null;

    try {
      let saved: PresetInfo;
      const description = draftDescription.trim() || null;

      if (editorMode === "create") {
        saved = await api.createCustomTheme(trimmedName, description, draftTheme);
      } else {
        if (!editingKey) {
          throw new Error("Missing custom theme key for update");
        }
        saved = await api.updateCustomTheme(editingKey, trimmedName, description, draftTheme);
      }

      selectedPreset = saved.key;
      appliedTheme = saved.theme ?? draftTheme;
      applyThemeToCSS(appliedTheme);
      closeEditor();
      await loadThemeState();
    } catch (error) {
      errorMessage = `Failed to save custom theme: ${String(error)}`;
    } finally {
      saving = false;
    }
  }

  async function deleteCustomTheme(preset: PresetInfo) {
    if (preset.source !== "custom") return;
    if (!window.confirm(`Delete custom theme \"${preset.name}\"?`)) return;

    saving = true;
    errorMessage = null;
    try {
      await api.deleteCustomTheme(preset.key);

      if (editingKey === preset.key) {
        closeEditor();
      }

      await loadThemeState();
    } catch (error) {
      errorMessage = `Failed to delete custom theme: ${String(error)}`;
    } finally {
      saving = false;
    }
  }

  function closeEditor() {
    editorOpen = false;
    editorMode = "create";
    editingKey = null;
    draftTheme = null;

    if (appliedTheme) {
      applyThemeToCSS(appliedTheme);
    }
  }

  async function discardDraft() {
    if (!(await confirmDiscardIfDirty("Discard unsaved draft changes?"))) return;
    closeEditor();
  }

  async function goBack() {
    if (!(await confirmDiscardIfDirty("Discard unsaved draft changes and leave Theme Settings?"))) {
      return;
    }
    if (appliedTheme) {
      applyThemeToCSS(appliedTheme);
    }
    onback();
  }

  async function confirmDiscardIfDirty(message: string): Promise<boolean> {
    if (!isDraftDirty) return true;
    return window.confirm(message);
  }

  function editorSignature(): string {
    if (!editorOpen) return "";

    const base = [draftName.trim(), draftDescription.trim(), creationMode];
    if (creationMode === "simple") {
      return JSON.stringify([...base, simplePrimary, simpleSecondary, simpleText]);
    }

    return JSON.stringify([
      ...base,
      sidebarColor,
      chatSectionColor,
      peerMessageColor,
      userMessageColor,
      editorTextColor,
    ]);
  }

  $effect(() => {
    if (!editorOpen || creationMode !== "advanced") return;

    const nextTheme = buildAdvancedTheme();
    draftTheme = nextTheme;
    applyThemeToCSS(nextTheme);
  });

  $effect(() => {
    if (!editorOpen || creationMode !== "simple") return;

    const p = simplePrimary;
    const s = simpleSecondary;
    const t = simpleText;

    const timer = window.setTimeout(() => {
      void regenerateSimpleTheme(p, s, t);
    }, 140);

    return () => window.clearTimeout(timer);
  });

  async function regenerateSimpleTheme(
    primary: string = simplePrimary,
    secondary: string = simpleSecondary,
    text: string = simpleText
  ) {
    if (!editorOpen || creationMode !== "simple") return;

    const version = ++simpleGenerateVersion;
    try {
      const generated = await api.generateSimpleTheme(primary, secondary, text);
      if (version !== simpleGenerateVersion) return;
      draftTheme = generated;
      applyThemeToCSS(generated);
      errorMessage = null;
    } catch (error) {
      if (version !== simpleGenerateVersion) return;
      errorMessage = `Failed to generate simple theme: ${String(error)}`;
    }
  }

  function hexToRgb(hex: string): [number, number, number] {
    const value = Number.parseInt(hex.replace("#", ""), 16);
    return [(value >> 16) & 255, (value >> 8) & 255, value & 255];
  }

  function rgbToHex(r: number, g: number, b: number): string {
    return (
      "#" +
      [r, g, b]
        .map((value) => Math.round(value).toString(16).padStart(2, "0"))
        .join("")
    );
  }

  function blendHex(fromHex: string, toHex: string, ratio: number): string {
    const t = Math.max(0, Math.min(1, ratio));
    const [r1, g1, b1] = hexToRgb(fromHex);
    const [r2, g2, b2] = hexToRgb(toHex);
    return rgbToHex(r1 + (r2 - r1) * t, g1 + (g2 - g1) * t, b1 + (b2 - b1) * t);
  }

  function generateAccentFromAnchor(anchorHex: string, textHex: string): ThemeConfig["primary"] {
    return {
      "600": anchorHex,
      "500": blendHex(anchorHex, textHex, 0.16),
      "400": blendHex(anchorHex, textHex, 0.32),
      "300": blendHex(anchorHex, textHex, 0.48),
    };
  }

  function buildAdvancedTheme(): ThemeConfig {
    const secondarySeed = blendHex(sidebarColor, userMessageColor, 0.55);

    return {
      base: {
        "950": chatSectionColor,
        "900": sidebarColor,
        "800": peerMessageColor,
        "700": blendHex(peerMessageColor, editorTextColor, 0.18),
        "600": blendHex(peerMessageColor, editorTextColor, 0.32),
        "500": blendHex(sidebarColor, editorTextColor, 0.38),
        "400": blendHex(sidebarColor, editorTextColor, 0.54),
        "300": blendHex(sidebarColor, editorTextColor, 0.7),
        "200": blendHex(sidebarColor, editorTextColor, 0.86),
        "100": editorTextColor,
      },
      primary: generateAccentFromAnchor(userMessageColor, editorTextColor),
      secondary: generateAccentFromAnchor(secondarySeed, editorTextColor),
      error: semanticError,
      success: semanticSuccess,
      info: semanticInfo,
      warning: semanticWarning,
    };
  }

  function defaultTheme(): ThemeConfig {
    return {
      base: {
        "950": "#020617",
        "900": "#0f172a",
        "800": "#1e293b",
        "700": "#334155",
        "600": "#475569",
        "500": "#64748b",
        "400": "#94a3b8",
        "300": "#cbd5e1",
        "200": "#e2e8f0",
        "100": "#f1f5f9",
      },
      primary: {
        "600": "#0d9488",
        "500": "#14b8a6",
        "400": "#2dd4bf",
        "300": "#5eead4",
      },
      secondary: {
        "600": "#9333ea",
        "500": "#a855f7",
        "400": "#c084fc",
        "300": "#d8b4fe",
      },
      error: {
        "600": "#dc2626",
        "500": "#ef4444",
        "400": "#f87171",
        "300": "#fca5a5",
      },
      success: {
        "600": "#16a34a",
        "500": "#22c55e",
        "400": "#4ade80",
        "300": "#86efac",
      },
      info: {
        "600": "#2563eb",
        "500": "#3b82f6",
        "400": "#60a5fa",
        "300": "#93c5fd",
      },
      warning: {
        "600": "#d97706",
        "500": "#f59e0b",
        "400": "#fbbf24",
        "300": "#fcd34d",
      },
    };
  }

  function getContrastYIQ(hex: string): string {
    const [r, g, b] = hexToRgb(hex);
    const yiq = (r * 299 + g * 587 + b * 114) / 1000;
    return yiq >= 128 ? "#020617" : "#ffffff";
  }

  function applyThemeToCSS(theme: ThemeConfig) {
    const root = document.documentElement;

    for (const [key, value] of Object.entries(theme.base)) {
      root.style.setProperty(`--color-base-${key}`, value);
    }
    for (const [key, value] of Object.entries(theme.primary)) {
      root.style.setProperty(`--color-primary-${key}`, value);
    }
    for (const [key, value] of Object.entries(theme.secondary)) {
      root.style.setProperty(`--color-secondary-${key}`, value);
    }
    for (const [key, value] of Object.entries(theme.error)) {
      root.style.setProperty(`--color-error-${key}`, value);
    }
    for (const [key, value] of Object.entries(theme.success)) {
      root.style.setProperty(`--color-success-${key}`, value);
    }
    for (const [key, value] of Object.entries(theme.info)) {
      root.style.setProperty(`--color-info-${key}`, value);
    }
    for (const [key, value] of Object.entries(theme.warning)) {
      root.style.setProperty(`--color-warning-${key}`, value);
    }

    root.style.setProperty("--color-on-primary", getContrastYIQ(theme.primary["600"]));
  }
</script>

<div class="mb-6 flex items-center gap-4 border-b border-slate-800/50 pb-4">
  <button
    onclick={goBack}
    class="p-2 hover:bg-slate-800 rounded-lg text-slate-400 hover:text-white transition-colors"
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
  <h2 class="text-xl font-bold text-theme-base-100">Theme Settings</h2>
</div>

{#if errorMessage}
  <div class="mb-4 rounded-lg border border-red-500/40 bg-red-500/10 px-3 py-2 text-sm text-red-200">
    {errorMessage}
  </div>
{/if}

{#if loading}
  <div class="flex items-center justify-center py-12">
    <div class="animate-spin rounded-full h-8 w-8 border-b-2 border-teal-500"></div>
  </div>
{:else}
  <div class="space-y-6">
    <div class="flex flex-wrap items-center justify-between gap-3">
      <div>
        <h3 class="text-sm font-semibold uppercase tracking-wide text-theme-base-300">Preset Library</h3>
        <p class="text-xs text-theme-base-400">Built-in presets and your custom saved themes.</p>
      </div>
      <div class="flex items-center gap-2">
        <button
          onclick={() => startCreate("simple")}
          class="px-3 py-2 rounded-lg bg-teal-600 hover:bg-teal-500 text-white text-sm disabled:opacity-50"
          disabled={saving}
        >
          New Simple Theme
        </button>
        <button
          onclick={() => startCreate("advanced")}
          class="px-3 py-2 rounded-lg bg-slate-800 hover:bg-slate-700 text-theme-base-200 text-sm disabled:opacity-50"
          disabled={saving}
        >
          New Advanced Theme
        </button>
      </div>
    </div>

    <div class="grid grid-cols-1 md:grid-cols-2 xl:grid-cols-3 gap-3">
      {#each presets as preset}
        <div
          class={`rounded-xl border p-4 transition-colors ${
            selectedPreset === preset.key
              ? "border-teal-500 bg-teal-500/10"
              : "border-slate-800 bg-slate-900/50"
          }`}
        >
          <div class="mb-2 flex items-start justify-between gap-2">
            <div>
              <div class="text-sm font-semibold text-theme-base-100">{preset.name}</div>
              <div class="text-xs text-theme-base-400">{preset.description || "No description"}</div>
            </div>
            <span class={`rounded-full border px-2 py-0.5 text-[10px] uppercase tracking-wide ${sourceBadgeClass(preset.source)}`}>
              {preset.source}
            </span>
          </div>

          {#if preset.source === "custom"}
            <div class="mb-2 text-[11px] text-theme-base-400">
              Updated {formatTs(preset.updated_at) || "-"}
            </div>
          {/if}

          <div class="flex flex-wrap gap-2">
            <button
              onclick={() => applyPreset(preset.key)}
              class="rounded-lg bg-slate-800 hover:bg-slate-700 px-3 py-1.5 text-xs text-theme-base-100"
              disabled={saving}
            >
              Apply
            </button>
            {#if preset.source === "custom"}
              <button
                onclick={() => startEdit(preset)}
                class="rounded-lg bg-emerald-600/20 hover:bg-emerald-600/30 border border-emerald-500/40 px-3 py-1.5 text-xs text-emerald-200"
                disabled={saving}
              >
                Edit
              </button>
              <button
                onclick={() => deleteCustomTheme(preset)}
                class="rounded-lg bg-red-600/20 hover:bg-red-600/30 border border-red-500/40 px-3 py-1.5 text-xs text-red-200"
                disabled={saving}
              >
                Delete
              </button>
            {/if}
          </div>
        </div>
      {/each}
    </div>

    {#if editorOpen && draftTheme}
      <div class="rounded-2xl border border-slate-700/80 bg-slate-950/80 p-4 space-y-4">
        <div class="flex flex-wrap items-center justify-between gap-3 border-b border-slate-800/80 pb-3">
          <div>
            <h3 class="text-base font-semibold text-theme-base-100">
              {editorMode === "create" ? "Create Custom Theme" : "Edit Custom Theme"}
            </h3>
            <p class="text-xs text-theme-base-400">Choose a mode, adjust colors, then save.</p>
          </div>
          <div class="inline-flex rounded-lg border border-slate-700 overflow-hidden">
            <button
              onclick={() => switchCreationMode("simple")}
              class={`px-3 py-1.5 text-sm ${
                creationMode === "simple"
                  ? "bg-teal-600 text-white"
                  : "bg-slate-900 text-theme-base-300"
              }`}
            >
              Simple
            </button>
            <button
              onclick={() => switchCreationMode("advanced")}
              class={`px-3 py-1.5 text-sm ${
                creationMode === "advanced"
                  ? "bg-teal-600 text-white"
                  : "bg-slate-900 text-theme-base-300"
              }`}
            >
              Advanced
            </button>
          </div>
        </div>

        <div class="grid grid-cols-1 lg:grid-cols-3 gap-4">
          <div class="lg:col-span-2 space-y-4">
            <div class="grid grid-cols-1 sm:grid-cols-2 gap-3">
              <label class="flex flex-col gap-1">
                <span class="text-xs text-theme-base-300">Theme title</span>
                <input
                  bind:value={draftName}
                  type="text"
                  maxlength="64"
                  class="rounded-lg border border-slate-700 bg-slate-900 px-3 py-2 text-sm text-theme-base-100"
                  placeholder="My Custom Theme"
                />
              </label>

              <label class="flex flex-col gap-1">
                <span class="text-xs text-theme-base-300">Description (optional)</span>
                <input
                  bind:value={draftDescription}
                  type="text"
                  maxlength="160"
                  class="rounded-lg border border-slate-700 bg-slate-900 px-3 py-2 text-sm text-theme-base-100"
                  placeholder="Optional note"
                />
              </label>
            </div>

            {#if creationMode === "simple"}
              <div class="grid grid-cols-1 xl:grid-cols-3 gap-3">
                <div class="rounded-xl border border-slate-800 bg-slate-900/60 p-3">
                  <div class="mb-2 text-xs text-theme-base-300">Primary</div>
                  <ColorPicker bind:hex={simplePrimary} />
                </div>
                <div class="rounded-xl border border-slate-800 bg-slate-900/60 p-3">
                  <div class="mb-2 text-xs text-theme-base-300">Secondary</div>
                  <ColorPicker bind:hex={simpleSecondary} />
                </div>
                <div class="rounded-xl border border-slate-800 bg-slate-900/60 p-3">
                  <div class="mb-2 text-xs text-theme-base-300">Text</div>
                  <ColorPicker bind:hex={simpleText} />
                </div>
              </div>
            {:else}
              <div class="grid grid-cols-1 md:grid-cols-2 xl:grid-cols-3 gap-3">
                <div class="rounded-xl border border-slate-800 bg-slate-900/60 p-3">
                  <div class="mb-2 text-xs text-theme-base-300">Left Sidebar Color</div>
                  <ColorPicker bind:hex={sidebarColor} />
                </div>
                <div class="rounded-xl border border-slate-800 bg-slate-900/60 p-3">
                  <div class="mb-2 text-xs text-theme-base-300">Chat Section Color</div>
                  <ColorPicker bind:hex={chatSectionColor} />
                </div>
                <div class="rounded-xl border border-slate-800 bg-slate-900/60 p-3">
                  <div class="mb-2 text-xs text-theme-base-300">Peer Message Box</div>
                  <ColorPicker bind:hex={peerMessageColor} />
                </div>
                <div class="rounded-xl border border-slate-800 bg-slate-900/60 p-3">
                  <div class="mb-2 text-xs text-theme-base-300">User Message Box</div>
                  <ColorPicker bind:hex={userMessageColor} />
                </div>
                <div class="rounded-xl border border-slate-800 bg-slate-900/60 p-3">
                  <div class="mb-2 text-xs text-theme-base-300">Text Color</div>
                  <ColorPicker bind:hex={editorTextColor} />
                </div>
              </div>
            {/if}
          </div>

          <div class="rounded-xl border border-slate-800 bg-slate-900/60 p-3 space-y-3">
            <div class="text-xs uppercase tracking-wide text-theme-base-400">Live Preview</div>
            <div class="rounded-lg border p-2" style={`background: ${draftTheme.base["900"]}; border-color: ${draftTheme.base["700"]};`}>
              <div class="grid grid-cols-[72px_1fr] gap-2">
                <div class="rounded-md p-2 text-[10px] font-semibold" style={`background: ${draftTheme.base["900"]}; color: ${draftTheme.base["100"]};`}>
                  Sidebar
                </div>
                <div class="rounded-md p-3" style={`background: ${draftTheme.base["950"]};`}>
                  <div class="mb-2 text-xs font-semibold" style={`color: ${draftTheme.base["100"]};`}>
                    Chat Section
                  </div>
                  <div class="space-y-2 text-xs">
                    <div class="max-w-[85%] rounded-xl rounded-bl-sm border px-3 py-2" style={`background: ${draftTheme.base["800"]}; border-color: ${draftTheme.base["700"]}; color: ${draftTheme.base["100"]};`}>
                      Peer Message Box
                    </div>
                    <div class="ml-auto max-w-[85%] rounded-xl rounded-br-sm px-3 py-2" style={`background: ${draftTheme.primary["600"]}; color: ${getContrastYIQ(draftTheme.primary["600"])};`}>
                      User Message Box
                    </div>
                  </div>
                </div>
              </div>
              <div class="mt-3 flex gap-2 text-[11px]">
                <span class="rounded px-2 py-1" style={`background: ${draftTheme.success["600"]}; color: ${getContrastYIQ(draftTheme.success["600"])};`}>Success</span>
                <span class="rounded px-2 py-1" style={`background: ${draftTheme.error["600"]}; color: ${getContrastYIQ(draftTheme.error["600"])};`}>Error</span>
                <span class="rounded px-2 py-1" style={`background: ${draftTheme.secondary["600"]}; color: ${getContrastYIQ(draftTheme.secondary["600"])};`}>Secondary</span>
              </div>
            </div>
            <div class="text-xs text-theme-base-400">
              {isDraftDirty ? "Unsaved changes" : "No unsaved changes"}
            </div>
          </div>
        </div>

        <div class="flex flex-wrap justify-end gap-2 border-t border-slate-800/80 pt-3">
          <button
            onclick={discardDraft}
            class="px-4 py-2 rounded-lg border border-slate-700 bg-slate-900 text-theme-base-200 hover:bg-slate-800"
            disabled={saving}
          >
            Discard
          </button>
          <button
            onclick={saveDraftTheme}
            class="px-4 py-2 rounded-lg bg-teal-600 text-white hover:bg-teal-500 disabled:opacity-50"
            disabled={saving || !draftName.trim()}
          >
            {saving ? "Saving..." : editorMode === "create" ? "Create Theme" : "Save Changes"}
          </button>
        </div>
      </div>
    {/if}

    {#if customPresets.length === 0}
      <div class="rounded-xl border border-dashed border-slate-700 bg-slate-900/30 px-4 py-3 text-sm text-theme-base-400">
        No custom themes yet. Create one in Simple or Advanced mode.
      </div>
    {/if}
  </div>
{/if}

<style>
  :global(.sacp-root) {
    width: 100%;
  }
</style>
