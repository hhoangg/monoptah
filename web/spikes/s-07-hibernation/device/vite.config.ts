import { dirname } from "node:path";
import { fileURLToPath } from "node:url";
import { defineConfig } from "vite";

const here = dirname(fileURLToPath(import.meta.url));

// Builds the probe page to device/dist (static files, no inline script; the build is not committed).
export default defineConfig({
  root: here,
  base: "./",
  build: { outDir: "dist", emptyOutDir: true, modulePreload: false, assetsInlineLimit: 0 },
});
