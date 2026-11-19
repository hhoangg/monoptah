import { defineConfig } from "vitest/config";

// Pure helper tests only. The Playwright specs in tests/ run through playwright.config.ts.
export default defineConfig({
  test: {
    include: ["test/unit/**/*.test.ts"],
    exclude: ["**/node_modules/**"],
  },
});
