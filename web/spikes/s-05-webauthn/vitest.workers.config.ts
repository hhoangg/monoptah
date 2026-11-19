import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { cloudflareTest } from "@cloudflare/vitest-plugin";
import { defineConfig } from "vitest/config";

// Runs the S-05 tests inside workerd (Miniflare) with the compatibility date and NO flags of ./wrangler.jsonc.
// Run from web/spikes: vitest run -c s-05-webauthn/vitest.workers.config.ts
const here = dirname(fileURLToPath(import.meta.url));

export default defineConfig({
  root: here,
  plugins: [cloudflareTest({ wrangler: { configPath: join(here, "wrangler.jsonc") } })],
  test: { include: ["test/**/*.workers.test.ts"] },
});
