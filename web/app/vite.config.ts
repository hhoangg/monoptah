import path from "node:path";
import { cloudflare } from "@cloudflare/vite-plugin";
import tailwindcss from "@tailwindcss/vite";
import { tanstackRouter } from "@tanstack/router-plugin/vite";
import react from "@vitejs/plugin-react";
import { defineConfig } from "vite";
import { headersPlugin } from "./vite-plugins/headers.ts";

// `vite dev` runs the SPA, the Worker, the Durable Objects and local D1 together through
// @cloudflare/vite-plugin, which reads the Worker config one level up (web/wrangler.jsonc).
// `vite build` writes the SPA to web/app/dist/client, the Worker assets directory (web/wrangler.jsonc),
// and the Worker bundle to web/app/dist/monoptah_web (never served). headersPlugin writes the `_headers`.
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
    cloudflare({ configPath: "../wrangler.jsonc" }),
    headersPlugin("../wrangler.jsonc"),
  ],
  resolve: { alias: { "@": path.resolve(import.meta.dirname, "src") } },
});
