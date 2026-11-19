import { defineConfig } from "vitest/config";

// Node-side spike tests (pure TS, Node WebCrypto). Workers-runtime tests (`*.workers.test.ts`) run
// through vitest.workers.config.ts inside workerd (Miniflare). Browser runs use playwright.config.ts.
export default defineConfig({
  test: {
    include: ["s-*/test/**/*.test.ts"],
    exclude: ["**/node_modules/**", "**/*.workers.test.ts", "**/*.browser.test.ts"],
  },
});
