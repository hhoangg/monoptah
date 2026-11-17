import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { cloudflareTest, readD1Migrations } from "@cloudflare/vitest-plugin";
import { defineConfig } from "vitest/config";
import { ensureAssets } from "./scripts/ensure-assets.mjs";

const here = dirname(fileURLToPath(import.meta.url));

// Runs the tests inside workerd (Miniflare) with the bindings of ../wrangler.jsonc. Nothing is deployed
// and no Cloudflare account is used.
export default defineConfig(async () => {
  // The assets binding needs its directory; a placeholder is created (git-ignored) if the app is not built.
  ensureAssets();
  const migrations = await readD1Migrations(join(here, "migrations"));
  return {
    plugins: [
      cloudflareTest({
        wrangler: { configPath: "../wrangler.jsonc" },
        miniflare: {
          bindings: {
            // Applied by test/setup.ts.
            TEST_MIGRATIONS: migrations,
            // Secrets are not in wrangler.jsonc; obviously fake values for the tests.
            VAPID_PRIVATE_KEY: "test-vapid-private-key",
            RECOVERY_PEPPER: "test-recovery-pepper",
            AUDIT_IP_SALT: "test-audit-ip-salt",
          },
        },
      }),
    ],
    test: {
      include: ["test/**/*.test.ts"],
      setupFiles: ["./test/setup.ts"],
    },
  };
});
