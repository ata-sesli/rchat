import { defineConfig } from "vite";
import { sveltekit } from "@sveltejs/kit/vite";
import tailwindcss from "@tailwindcss/vite";
const host = process.env.TAURI_DEV_HOST;

/**
 * @param {string} code
 */
function extractSvelteStyle(code) {
  const match = code.match(/<style(?:\s[^>]*)?>([\s\S]*?)<\/style>/i);
  return match?.[1]?.trim() ?? "";
}

/**
 * @returns {import("vite").Plugin}
 */
function svelteStyleCssGuard() {
  return {
    name: "rchat:svelte-style-css-guard",
    enforce: "pre",
    transform(code, id) {
      const isSvelteStyleRequest =
        id.includes("?svelte") &&
        id.includes("type=style") &&
        id.includes("lang.css");
      if (!isSvelteStyleRequest || !code.includes("<script")) return;

      return { code: extractSvelteStyle(code), map: null };
    },
  };
}

// https://vite.dev/config/
export default defineConfig(async () => ({
  plugins: [svelteStyleCssGuard(), tailwindcss(), sveltekit()],

  // Vite options tailored for Tauri development and only applied in `tauri dev` or `tauri build`
  //
  // 1. prevent Vite from obscuring rust errors
  clearScreen: false,
  // 2. tauri expects a fixed port, fail if that port is not available
  server: {
    port: 1420,
    strictPort: true,
    host: host || false,
    hmr: host
      ? {
          protocol: "ws",
          host,
          port: 1421,
        }
      : undefined,
    watch: {
      // 3. tell Vite to ignore watching `src-tauri`
      ignored: ["**/src-tauri/**"],
    },
  },
}));
