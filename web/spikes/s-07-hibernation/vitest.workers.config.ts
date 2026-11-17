import { dirname } from "node:path";
import { fileURLToPath } from "node:url";
import { cloudflareTest } from "@cloudflare/vitest-plugin";
import { defineConfig } from "vitest/config";

const here = dirname(fileURLToPath(import.meta.url));

// Runs the S-07 relay prototype inside workerd (Miniflare). Nothing is deployed.
export default defineConfig({
  root: here,
  plugins: [cloudflareTest({ wrangler: { configPath: "./wrangler.jsonc" } })],
  test: { include: ["test/**/*.workers.test.ts"] },
});
