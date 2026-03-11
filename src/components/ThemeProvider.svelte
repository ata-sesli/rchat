<script lang="ts">
  import { onMount } from "svelte";
  import { api, type ThemeConfig } from "$lib/tauri/api";

  let themeLoaded = false;

  onMount(async () => {
    try {
      const theme = await api.getTheme();
      applyTheme(theme);
      themeLoaded = true;
    } catch (e) {
      console.error("[ThemeProvider] Failed to load theme:", e);
      themeLoaded = true; // Use CSS defaults
    }
  });

  function applyTheme(theme: ThemeConfig) {
    const root = document.documentElement;

    // Base colors
    root.style.setProperty("--color-base-950", theme.base["950"]);
    root.style.setProperty("--color-base-900", theme.base["900"]);
    root.style.setProperty("--color-base-800", theme.base["800"]);
    root.style.setProperty("--color-base-700", theme.base["700"]);
    root.style.setProperty("--color-base-600", theme.base["600"]);
    root.style.setProperty("--color-base-500", theme.base["500"]);
    root.style.setProperty("--color-base-400", theme.base["400"]);
    root.style.setProperty("--color-base-300", theme.base["300"]);
    root.style.setProperty("--color-base-200", theme.base["200"]);
    root.style.setProperty("--color-base-100", theme.base["100"]);

    // Primary colors
    root.style.setProperty("--color-primary-600", theme.primary["600"]);
    root.style.setProperty("--color-primary-500", theme.primary["500"]);
    root.style.setProperty("--color-primary-400", theme.primary["400"]);
    root.style.setProperty("--color-primary-300", theme.primary["300"]);

    // Secondary colors
    root.style.setProperty("--color-secondary-600", theme.secondary["600"]);
    root.style.setProperty("--color-secondary-500", theme.secondary["500"]);
    root.style.setProperty("--color-secondary-400", theme.secondary["400"]);
    root.style.setProperty("--color-secondary-300", theme.secondary["300"]);

    // Error colors
    root.style.setProperty("--color-error-600", theme.error["600"]);
    root.style.setProperty("--color-error-500", theme.error["500"]);
    root.style.setProperty("--color-error-400", theme.error["400"]);
    root.style.setProperty("--color-error-300", theme.error["300"]);

    // Success colors
    root.style.setProperty("--color-success-600", theme.success["600"]);
    root.style.setProperty("--color-success-500", theme.success["500"]);
    root.style.setProperty("--color-success-400", theme.success["400"]);
    root.style.setProperty("--color-success-300", theme.success["300"]);

    // Info colors
    root.style.setProperty("--color-info-600", theme.info["600"]);
    root.style.setProperty("--color-info-500", theme.info["500"]);
    root.style.setProperty("--color-info-400", theme.info["400"]);
    root.style.setProperty("--color-info-300", theme.info["300"]);

    // Warning colors
    root.style.setProperty("--color-warning-600", theme.warning["600"]);
    root.style.setProperty("--color-warning-500", theme.warning["500"]);
    root.style.setProperty("--color-warning-400", theme.warning["400"]);
    root.style.setProperty("--color-warning-300", theme.warning["300"]);
    root.style.setProperty(
      "--color-on-primary",
      getContrastYIQ(theme.primary["600"])
    );

    console.log("[ThemeProvider] Theme applied successfully");
  }

  function getContrastYIQ(hex: string): string {
    const value = hex.replace("#", "");
    const bigint = Number.parseInt(value, 16);
    const r = (bigint >> 16) & 255;
    const g = (bigint >> 8) & 255;
    const b = bigint & 255;
    const yiq = (r * 299 + g * 587 + b * 114) / 1000;
    return yiq >= 128 ? "#020617" : "#ffffff";
  }

  // Export function for external theme updates
  export function refreshTheme() {
    api.getTheme().then(applyTheme);
  }
</script>

{#if themeLoaded}
  <slot />
{:else}
  <!-- Brief loading state while theme loads -->
  <div class="w-full h-full bg-theme-base-950"></div>
{/if}
