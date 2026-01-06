<script lang="ts">
  import { onMount } from "svelte";
  import { invoke } from "@tauri-apps/api/core";
  import ColorPicker from "svelte-awesome-color-picker";

  let { onback = () => {} } = $props();

  // --- TYPES ---
  type AccentColors = {
    "600": string;
    "500": string;
    "400": string;
    "300": string;
  };

  type BaseColors = {
    "950": string;
    "900": string;
    "800": string;
    "700": string;
    "600": string;
    "500": string;
    "400": string;
    "300": string;
    "200": string;
    "100": string;
  };

  type ThemeConfig = {
    base: BaseColors;
    primary: AccentColors;
    secondary: AccentColors;
    error: AccentColors;
    success: AccentColors;
    info: AccentColors;
    warning: AccentColors;
  };

  type PresetInfo = {
    key: string;
    name: string;
    description: string;
  };

  // --- STATE ---
  let theme = $state<ThemeConfig | null>(null);
  let presets = $state<PresetInfo[]>([]);
  let loading = $state(true);
  let saving = $state(false);
  let showManual = $state(false);
  let expandedSection = $state<string | null>(null);

  // Manual mode state
  let baseColor = $state("#64748b");
  let primaryColor = $state("#14b8a6");
  let secondaryColor = $state("#a855f7");
  let errorColor = $state("#ef4444");
  let successColor = $state("#22c55e");
  let infoColor = $state("#3b82f6");
  let warningColor = $state("#f59e0b");
  let selectedPreset = $state<string | null>(null);

  onMount(async () => {
    try {
      // Load current theme, available presets, and selected preset
      const [loadedTheme, loadedPresets, loadedSelected] = await Promise.all([
        invoke<ThemeConfig>("get_theme"),
        invoke<PresetInfo[]>("list_theme_presets"),
        invoke<string | null>("get_selected_preset"),
      ]);
      theme = loadedTheme;
      presets = loadedPresets;
      selectedPreset = loadedSelected;
      if (theme) {
        updateManualStateFromTheme(theme);
      }
    } catch (e) {
      console.error("Failed to load theme:", e);
    } finally {
      loading = false;
    }
  });

  function updateManualStateFromTheme(t: ThemeConfig) {
    baseColor = t.base["500"];
    primaryColor = t.primary["500"];
    secondaryColor = t.secondary["500"];
    errorColor = t.error["500"];
    successColor = t.success["500"];
    infoColor = t.info["500"];
    warningColor = t.warning["500"];
  }

  // Format preset name for display (snake_case to Title Case)
  function formatPresetName(name: string): string {
    return name
      .split("_")
      .map((word) => word.charAt(0).toUpperCase() + word.slice(1))
      .join(" ");
  }

  // --- APPLY PRESET VIA BACKEND ---
  async function applyPreset(presetName: string) {
    if (saving) return;
    saving = true;
    try {
      const newTheme = await invoke<ThemeConfig>("apply_preset", {
        name: presetName,
      });
      theme = newTheme;
      selectedPreset = presetName;
      updateManualStateFromTheme(newTheme);
      applyThemeToCSS(newTheme);
    } catch (e) {
      console.error("Failed to apply preset:", e);
    } finally {
      saving = false;
    }
  }

  // --- HELPERS FOR PRESETS ---
  function hexToRgb(hex: string): [number, number, number] {
    const bigint = parseInt(hex.replace("#", ""), 16);
    return [(bigint >> 16) & 255, (bigint >> 8) & 255, bigint & 255];
  }

  function rgbToHex(r: number, g: number, b: number): string {
    return (
      "#" +
      [r, g, b].map((x) => Math.round(x).toString(16).padStart(2, "0")).join("")
    );
  }

  function interpolateColor(c1: string, c2: string, factor: number): string {
    const [r1, g1, b1] = hexToRgb(c1);
    const [r2, g2, b2] = hexToRgb(c2);
    const r = r1 + (r2 - r1) * factor;
    const g = g1 + (g2 - g1) * factor;
    const b = b1 + (b2 - b1) * factor;
    return rgbToHex(r, g, b);
  }

  // Standard HSL helpers for accents
  function hexToHsl(hex: string): [number, number, number] {
    const [r, g, b] = hexToRgb(hex).map((v) => v / 255);
    const max = Math.max(r, g, b),
      min = Math.min(r, g, b);
    let h = 0,
      s = 0,
      l = (max + min) / 2;
    if (max !== min) {
      const d = max - min;
      s = l > 0.5 ? d / (2 - max - min) : d / (max + min);
      switch (max) {
        case r:
          h = ((g - b) / d + (g < b ? 6 : 0)) / 6;
          break;
        case g:
          h = ((b - r) / d + 2) / 6;
          break;
        case b:
          h = ((r - g) / d + 4) / 6;
          break;
      }
    }
    return [h * 360, s * 100, l * 100];
  }

  function hslToHex(h: number, s: number, l: number): string {
    s /= 100;
    l /= 100;
    const a = s * Math.min(l, 1 - l);
    const f = (n: number) => {
      const k = (n + h / 30) % 12;
      const color = l - a * Math.max(Math.min(k - 3, 9 - k, 1), -1);
      return Math.round(255 * color)
        .toString(16)
        .padStart(2, "0");
    };
    return `#${f(0)}${f(8)}${f(4)}`;
  }

  function generateAccentShades(baseHex: string): AccentColors {
    const [h, s] = hexToHsl(baseHex);
    return {
      "600": hslToHex(h, Math.min(s + 10, 100), 40),
      "500": hslToHex(h, s, 50),
      "400": hslToHex(h, Math.max(s - 5, 0), 62),
      "300": hslToHex(h, Math.max(s - 10, 0), 75),
    };
  }

  // --- MANUAL MODE --
  function generateBaseShadesManual(baseHex: string): BaseColors {
    // Current simple logic for manual mode (mostly for dark themes)
    const [h, s] = hexToHsl(baseHex);
    return {
      "950": hslToHex(h, s, 3),
      "900": hslToHex(h, s, 8),
      "800": hslToHex(h, s, 15),
      "700": hslToHex(h, s, 25),
      "600": hslToHex(h, s, 35),
      "500": hslToHex(h, s, 45),
      "400": hslToHex(h, s, 60),
      "300": hslToHex(h, s, 75),
      "200": hslToHex(h, s, 88),
      "100": hslToHex(h, s, 95),
    };
  }

  // Reactive effects for MANUAL mode live preview
  $effect(() => {
    // Only auto-update if IN manual mode
    if (showManual && theme) theme.base = generateBaseShadesManual(baseColor);
  });
  $effect(() => {
    if (showManual && theme) theme.primary = generateAccentShades(primaryColor);
  });
  $effect(() => {
    if (showManual && theme)
      theme.secondary = generateAccentShades(secondaryColor);
  });
  $effect(() => {
    if (showManual && theme) theme.error = generateAccentShades(errorColor);
  });
  $effect(() => {
    if (showManual && theme) theme.success = generateAccentShades(successColor);
  });
  $effect(() => {
    if (showManual && theme) theme.info = generateAccentShades(infoColor);
  });
  $effect(() => {
    if (showManual && theme) theme.warning = generateAccentShades(warningColor);
  });

  async function saveThemeManual() {
    if (theme) await saveThemeImpl(theme);
  }

  async function saveThemeImpl(t: ThemeConfig) {
    saving = true;
    try {
      await invoke("update_theme", { theme: t });
      applyThemeToCSS(t);
    } catch (e) {
      console.error("Failed to save theme:", e);
    } finally {
      saving = false;
    }
  }

  async function resetTheme() {
    // Just re-apply 'midnightNeon' as roughly default? or hardcoded default?
    // Default in backend is hardcoded.
    // Let's use Manual Reset logic but set state variables.
    baseColor = "#64748b";
    primaryColor = "#14b8a6";
    secondaryColor = "#a855f7";
    errorColor = "#ef4444";
    successColor = "#22c55e";
    infoColor = "#3b82f6";
    warningColor = "#f59e0b";

    // This triggers effects if in Manual mode.
    // If in Preset mode, we should manually construct default theme.
    if (!showManual) {
      // Force manual mode for a sec to trigger effects? or just construct it.
      const t: ThemeConfig = {
        base: generateBaseShadesManual(baseColor),
        primary: generateAccentShades(primaryColor),
        secondary: generateAccentShades(secondaryColor),
        error: generateAccentShades(errorColor),
        success: generateAccentShades(successColor),
        info: generateAccentShades(infoColor),
        warning: generateAccentShades(warningColor),
      };
      theme = t;
      await saveThemeImpl(t);
    }
  }

  function getContrastYIQ(hex: string): string {
    const [r, g, b] = hexToRgb(hex);
    const yiq = (r * 299 + g * 587 + b * 114) / 1000;
    return yiq >= 128 ? "#020617" : "#FFFFFF";
  }

  function applyThemeToCSS(t: ThemeConfig) {
    const root = document.documentElement;
    Object.entries(t.base).forEach(([k, v]) =>
      root.style.setProperty(`--color-base-${k}`, v)
    );
    Object.entries(t.primary).forEach(([k, v]) =>
      root.style.setProperty(`--color-primary-${k}`, v)
    );
    Object.entries(t.secondary).forEach(([k, v]) =>
      root.style.setProperty(`--color-secondary-${k}`, v)
    );
    Object.entries(t.error).forEach(([k, v]) =>
      root.style.setProperty(`--color-error-${k}`, v)
    );
    Object.entries(t.success).forEach(([k, v]) =>
      root.style.setProperty(`--color-success-${k}`, v)
    );
    Object.entries(t.info).forEach(([k, v]) =>
      root.style.setProperty(`--color-info-${k}`, v)
    );
    Object.entries(t.warning).forEach(([k, v]) =>
      root.style.setProperty(`--color-warning-${k}`, v)
    );

    // Set contrast text color for primary bubble
    root.style.setProperty(
      "--color-on-primary",
      getContrastYIQ(t.primary["600"])
    );
  }
</script>

<!-- Header -->
<div class="mb-6 flex items-center gap-4 border-b border-slate-800/50 pb-4">
  <button
    onclick={() => onback()}
    class="p-2 hover:bg-slate-800 rounded-lg text-slate-400 hover:text-white transition-colors"
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

{#if loading}
  <div class="flex items-center justify-center py-12">
    <div
      class="animate-spin rounded-full h-8 w-8 border-b-2 border-teal-500"
    ></div>
  </div>
{:else if !showManual}
  <!-- PRESETS MODE -->
  <div class="space-y-6">
    <div class="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-3 gap-4">
      {#each presets as preset}
        <button
          onclick={() => applyPreset(preset.key)}
          disabled={saving}
          class={`rounded-xl p-4 transition-all text-left group disabled:opacity-50 relative ${
            selectedPreset === preset.key
              ? "bg-teal-500/10 border-2 border-teal-500 ring-2 ring-teal-500/20"
              : "bg-slate-900/50 border border-slate-800 hover:border-teal-500/50 hover:bg-slate-800/50"
          }`}
        >
          {#if selectedPreset === preset.key}
            <div
              class="absolute top-2 right-2 w-5 h-5 bg-teal-500 rounded-full flex items-center justify-center"
            >
              <svg
                class="w-3 h-3 text-white"
                fill="none"
                stroke="currentColor"
                viewBox="0 0 24 24"
              >
                <path
                  stroke-linecap="round"
                  stroke-linejoin="round"
                  stroke-width="3"
                  d="M5 13l4 4L19 7"
                />
              </svg>
            </div>
          {/if}
          <div
            class={`font-medium transition-colors mb-1 ${
              selectedPreset === preset.key
                ? "text-teal-400"
                : "text-white group-hover:text-teal-400"
            }`}
          >
            {preset.name}
          </div>
          <div class="text-xs text-slate-500">{preset.description}</div>
        </button>
      {/each}

      <!-- Manual Entry Button -->
      <button
        onclick={() => (showManual = true)}
        class="bg-slate-900/30 border border-dashed border-slate-700 rounded-xl p-4 hover:border-slate-500 hover:bg-slate-800/30 transition-all text-left flex flex-col justify-center min-h-[100px]"
      >
        <div class="font-medium text-slate-300 mb-1">Manual Theme Creation</div>
        <div class="text-xs text-slate-500">
          Build a custom palette from scratch
        </div>
      </button>
    </div>
  </div>
{:else if theme}
  <!-- MANUAL MODE -->
  <div class="mb-4">
    <button
      onclick={() => (showManual = false)}
      class="text-sm text-slate-400 hover:text-white flex items-center gap-1"
    >
      <svg class="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24"
        ><path
          stroke-linecap="round"
          stroke-linejoin="round"
          stroke-width="2"
          d="M15 19l-7-7 7-7"
        /></svg
      >
      Back to Presets
    </button>
  </div>

  <div class="space-y-4">
    <!-- Base Colors -->
    <div
      class="bg-slate-900/50 rounded-xl border border-slate-800 overflow-hidden"
    >
      <button
        onclick={() =>
          (expandedSection = expandedSection === "base" ? null : "base")}
        class="w-full p-4 flex items-center justify-between gap-4 hover:bg-slate-800/30 transition-colors"
      >
        <div class="text-left">
          <div class="font-medium text-white">Base</div>
          <div class="text-xs text-slate-500">Backgrounds & text</div>
        </div>
        <div class="flex gap-1">
          {#each ["950", "800", "600", "400", "200"] as shade}
            <div
              class="w-5 h-5 rounded"
              style="background-color: {theme.base[shade as keyof BaseColors]}"
            ></div>
          {/each}
        </div>
        <svg
          class="w-5 h-5 text-slate-400 transition-transform {expandedSection ===
          'base'
            ? 'rotate-180'
            : ''}"
          fill="none"
          stroke="currentColor"
          viewBox="0 0 24 24"
        >
          <path
            stroke-linecap="round"
            stroke-linejoin="round"
            stroke-width="2"
            d="M19 9l-7 7-7-7"
          />
        </svg>
      </button>
      {#if expandedSection === "base"}
        <div class="p-4 border-t border-slate-800/50 min-h-[280px]">
          <ColorPicker bind:hex={baseColor} />
        </div>
      {/if}
    </div>

    <!-- Primary -->
    <div
      class="bg-slate-900/50 rounded-xl border border-slate-800 overflow-hidden"
    >
      <button
        onclick={() =>
          (expandedSection = expandedSection === "primary" ? null : "primary")}
        class="w-full p-4 flex items-center justify-between gap-4 hover:bg-slate-800/30 transition-colors"
      >
        <div class="text-left">
          <div class="font-medium text-white">Primary</div>
          <div class="text-xs text-slate-500">Main accent</div>
        </div>
        <div class="flex gap-1">
          {#each ["600", "500", "400", "300"] as shade}
            <div
              class="w-5 h-5 rounded"
              style="background-color: {theme.primary[
                shade as keyof AccentColors
              ]}"
            ></div>
          {/each}
        </div>
        <svg
          class="w-5 h-5 text-slate-400 transition-transform {expandedSection ===
          'primary'
            ? 'rotate-180'
            : ''}"
          fill="none"
          stroke="currentColor"
          viewBox="0 0 24 24"
        >
          <path
            stroke-linecap="round"
            stroke-linejoin="round"
            stroke-width="2"
            d="M19 9l-7 7-7-7"
          />
        </svg>
      </button>
      {#if expandedSection === "primary"}
        <div class="p-4 border-t border-slate-800/50 min-h-[280px]">
          <ColorPicker bind:hex={primaryColor} />
        </div>
      {/if}
    </div>

    <!-- Secondary -->
    <div
      class="bg-slate-900/50 rounded-xl border border-slate-800 overflow-hidden"
    >
      <button
        onclick={() =>
          (expandedSection =
            expandedSection === "secondary" ? null : "secondary")}
        class="w-full p-4 flex items-center justify-between gap-4 hover:bg-slate-800/30 transition-colors"
      >
        <div class="text-left">
          <div class="font-medium text-white">Secondary</div>
          <div class="text-xs text-slate-500">Secondary accent</div>
        </div>
        <div class="flex gap-1">
          {#each ["600", "500", "400", "300"] as shade}
            <div
              class="w-5 h-5 rounded"
              style="background-color: {theme.secondary[
                shade as keyof AccentColors
              ]}"
            ></div>
          {/each}
        </div>
        <svg
          class="w-5 h-5 text-slate-400 transition-transform {expandedSection ===
          'secondary'
            ? 'rotate-180'
            : ''}"
          fill="none"
          stroke="currentColor"
          viewBox="0 0 24 24"
        >
          <path
            stroke-linecap="round"
            stroke-linejoin="round"
            stroke-width="2"
            d="M19 9l-7 7-7-7"
          />
        </svg>
      </button>
      {#if expandedSection === "secondary"}
        <div class="p-4 border-t border-slate-800/50 min-h-[280px]">
          <ColorPicker bind:hex={secondaryColor} />
        </div>
      {/if}
    </div>

    <!-- Error -->
    <div
      class="bg-slate-900/50 rounded-xl border border-slate-800 overflow-hidden"
    >
      <button
        onclick={() =>
          (expandedSection = expandedSection === "error" ? null : "error")}
        class="w-full p-4 flex items-center justify-between gap-4 hover:bg-slate-800/30 transition-colors"
      >
        <div class="text-left">
          <div class="font-medium text-white">Error</div>
          <div class="text-xs text-slate-500">Errors & delete</div>
        </div>
        <div class="flex gap-1">
          {#each ["600", "500", "400", "300"] as shade}
            <div
              class="w-5 h-5 rounded"
              style="background-color: {theme.error[
                shade as keyof AccentColors
              ]}"
            ></div>
          {/each}
        </div>
        <svg
          class="w-5 h-5 text-slate-400 transition-transform {expandedSection ===
          'error'
            ? 'rotate-180'
            : ''}"
          fill="none"
          stroke="currentColor"
          viewBox="0 0 24 24"
        >
          <path
            stroke-linecap="round"
            stroke-linejoin="round"
            stroke-width="2"
            d="M19 9l-7 7-7-7"
          />
        </svg>
      </button>
      {#if expandedSection === "error"}
        <div class="p-4 border-t border-slate-800/50 min-h-[280px]">
          <ColorPicker bind:hex={errorColor} />
        </div>
      {/if}
    </div>

    <!-- Success -->
    <div
      class="bg-slate-900/50 rounded-xl border border-slate-800 overflow-hidden"
    >
      <button
        onclick={() =>
          (expandedSection = expandedSection === "success" ? null : "success")}
        class="w-full p-4 flex items-center justify-between gap-4 hover:bg-slate-800/30 transition-colors"
      >
        <div class="text-left">
          <div class="font-medium text-white">Success</div>
          <div class="text-xs text-slate-500">Online & success</div>
        </div>
        <div class="flex gap-1">
          {#each ["600", "500", "400", "300"] as shade}
            <div
              class="w-5 h-5 rounded"
              style="background-color: {theme.success[
                shade as keyof AccentColors
              ]}"
            ></div>
          {/each}
        </div>
        <svg
          class="w-5 h-5 text-slate-400 transition-transform {expandedSection ===
          'success'
            ? 'rotate-180'
            : ''}"
          fill="none"
          stroke="currentColor"
          viewBox="0 0 24 24"
        >
          <path
            stroke-linecap="round"
            stroke-linejoin="round"
            stroke-width="2"
            d="M19 9l-7 7-7-7"
          />
        </svg>
      </button>
      {#if expandedSection === "success"}
        <div class="p-4 border-t border-slate-800/50 min-h-[280px]">
          <ColorPicker bind:hex={successColor} />
        </div>
      {/if}
    </div>

    <!-- Info -->
    <div
      class="bg-slate-900/50 rounded-xl border border-slate-800 overflow-hidden"
    >
      <button
        onclick={() =>
          (expandedSection = expandedSection === "info" ? null : "info")}
        class="w-full p-4 flex items-center justify-between gap-4 hover:bg-slate-800/30 transition-colors"
      >
        <div class="text-left">
          <div class="font-medium text-white">Info</div>
          <div class="text-xs text-slate-500">Information</div>
        </div>
        <div class="flex gap-1">
          {#each ["600", "500", "400", "300"] as shade}
            <div
              class="w-5 h-5 rounded"
              style="background-color: {theme.info[
                shade as keyof AccentColors
              ]}"
            ></div>
          {/each}
        </div>
        <svg
          class="w-5 h-5 text-slate-400 transition-transform {expandedSection ===
          'info'
            ? 'rotate-180'
            : ''}"
          fill="none"
          stroke="currentColor"
          viewBox="0 0 24 24"
        >
          <path
            stroke-linecap="round"
            stroke-linejoin="round"
            stroke-width="2"
            d="M19 9l-7 7-7-7"
          />
        </svg>
      </button>
      {#if expandedSection === "info"}
        <div class="p-4 border-t border-slate-800/50 min-h-[280px]">
          <ColorPicker bind:hex={infoColor} />
        </div>
      {/if}
    </div>

    <!-- Warning -->
    <div
      class="bg-slate-900/50 rounded-xl border border-slate-800 overflow-hidden"
    >
      <button
        onclick={() =>
          (expandedSection = expandedSection === "warning" ? null : "warning")}
        class="w-full p-4 flex items-center justify-between gap-4 hover:bg-slate-800/30 transition-colors"
      >
        <div class="text-left">
          <div class="font-medium text-white">Warning</div>
          <div class="text-xs text-slate-500">Warnings</div>
        </div>
        <div class="flex gap-1">
          {#each ["600", "500", "400", "300"] as shade}
            <div
              class="w-5 h-5 rounded"
              style="background-color: {theme.warning[
                shade as keyof AccentColors
              ]}"
            ></div>
          {/each}
        </div>
        <svg
          class="w-5 h-5 text-slate-400 transition-transform {expandedSection ===
          'warning'
            ? 'rotate-180'
            : ''}"
          fill="none"
          stroke="currentColor"
          viewBox="0 0 24 24"
        >
          <path
            stroke-linecap="round"
            stroke-linejoin="round"
            stroke-width="2"
            d="M19 9l-7 7-7-7"
          />
        </svg>
      </button>
      {#if expandedSection === "warning"}
        <div class="p-4 border-t border-slate-800/50 min-h-[280px]">
          <ColorPicker bind:hex={warningColor} />
        </div>
      {/if}
    </div>
  </div>

  <!-- Actions -->
  <div class="mt-6 flex gap-3 border-t border-slate-800/50 pt-4">
    <button
      onclick={resetTheme}
      class="px-4 py-2 bg-slate-800 hover:bg-slate-700 text-slate-300 rounded-lg transition-colors"
    >
      Reset to Default
    </button>
    <button
      onclick={saveThemeManual}
      disabled={saving}
      class="px-4 py-2 bg-teal-600 hover:bg-teal-500 text-white rounded-lg transition-colors disabled:opacity-50"
    >
      {saving ? "Saving..." : "Save Theme"}
    </button>
  </div>
{/if}

<style>
  /* Color picker now displays inline within expanded sections */
</style>
