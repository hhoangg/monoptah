import path from "node:path";
import { cloudflare } from "@cloudflare/vite-plugin";
import tailwindcss from "@tailwindcss/vite";
import { tanstackRouter } from "@tanstack/router-plugin/vite";
import react from "@vitejs/plugin-react";
import { defineConfig } from "vite";
import { VitePWA } from "vite-plugin-pwa";
import { headersPlugin } from "./vite-plugins/headers.ts";
import { staticLibraryCss } from "./vite-plugins/staticLibraryCss.ts";

// `vite dev` runs the SPA, the Worker, the Durable Objects and local D1 together through
// @cloudflare/vite-plugin, which reads the Worker config one level up (web/wrangler.jsonc).
// `vite build` writes the SPA to web/app/dist/client, the Worker assets directory (web/wrangler.jsonc),
// and the Worker bundle to web/app/dist/monoptah_web (never served). headersPlugin writes the `_headers`.
// An e2e spec that serves the dev preview next to the harness's own `wrangler dev` sets MONOPTAH_PREVIEW_STATE to a
// temp dir: its Miniflare then keeps its state there (not under web/app/.wrangler) and opens no inspector port.
const previewState = process.env["MONOPTAH_PREVIEW_STATE"];

export default defineConfig({
  plugins: [
    // Must come before the React plugin. Generates src/routeTree.gen.ts (committed).
    tanstackRouter({
      target: "react",
      autoCodeSplitting: true,
      // Same pattern as tsr.config.json: a colocated *.test.tsx is not a route.
      routeFileIgnorePattern: "\\.test\\.tsx?$",
    }),
    react(),
    tailwindcss(),
    cloudflare({ configPath: "../wrangler.jsonc", ...(previewState ? { persistState: { path: previewState }, inspectorPort: false as const } : {}) }),
    headersPlugin("../wrangler.jsonc"),
    // S-08: Sonner and vaul must not append a runtime <style> (blocked by style-src 'self'); see staticLibraryCss.ts.
    staticLibraryCss(),
    // Workbox injectManifest: src/sw.ts is compiled to /sw.js with the precache list injected. The
    // manifest stays public/manifest.webmanifest and registration is src/features/offline/registerSw.ts
    // (Trusted Types aware), so the plugin only builds the worker.
    VitePWA({
      strategies: "injectManifest",
      srcDir: "src",
      filename: "sw.ts",
      registerType: "prompt",
      injectRegister: false,
      manifest: false,
      injectManifest: {
        globPatterns: ["**/*.{js,css,html,woff2,svg,png,ico,webmanifest}"],
        // The shell is a few hundred kB per chunk; a chunk over 2 MiB is a regression, not something to precache.
        maximumFileSizeToCacheInBytes: 2 * 1024 * 1024,
      },
      devOptions: { enabled: false },
    }),
  ],
  resolve: {
    alias: [
      { find: "@", replacement: path.resolve(import.meta.dirname, "src") },
      // S-08: Radix scroll lock (through vaul) would append a runtime <style>; src/patches/style-singleton.ts adopts a
      // constructed stylesheet instead.
      { find: /^react-style-singleton$/, replacement: path.resolve(import.meta.dirname, "src/patches/style-singleton.ts") },
    ],
  },
});
