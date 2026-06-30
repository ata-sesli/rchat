// @ts-nocheck
import { describe, expect, test } from "bun:test";
import viteConfig from "../vite.config.js";

function flattenPlugins(plugins) {
  const flat = [];
  for (const plugin of plugins || []) {
    if (Array.isArray(plugin)) {
      flat.push(...flattenPlugins(plugin));
    } else {
      flat.push(plugin);
    }
  }
  return flat;
}

describe("vite plugin ordering", () => {
  test("guards bogus Svelte style virtual modules before Tailwind transforms CSS", async () => {
    const config = await viteConfig();
    const plugins = flattenPlugins(config.plugins);
    const guardIndex = plugins.findIndex(
      (plugin) => plugin?.name === "rchat:svelte-style-css-guard",
    );
    const tailwindIndex = plugins.findIndex(
      (plugin) => plugin?.name === "@tailwindcss/vite:generate:serve",
    );

    expect(guardIndex).toBeGreaterThanOrEqual(0);
    expect(tailwindIndex).toBeGreaterThanOrEqual(0);
    expect(guardIndex).toBeLessThan(tailwindIndex);

    const guard = plugins[guardIndex];
    expect(
      guard.transform(
        '<script lang="ts">import { getCurrent } from "@tauri-apps/plugin-deep-link";</script>',
        "/src/routes/+layout.svelte?svelte&type=style&lang.css",
      ),
    ).toEqual({ code: "", map: null });
    expect(
      guard.transform(
        '<script>let show = true;</script><div class="card"></div><style>.card { color: red; }</style>',
        "/src/components/Card.svelte?svelte&type=style&lang.css",
      ),
    ).toEqual({ code: ".card { color: red; }", map: null });
    expect(
      guard.transform(
        "body { margin: 0; }",
        "/src/components/Foo.svelte?svelte&type=style&lang.css",
      ),
    ).toBeUndefined();
  });
});
