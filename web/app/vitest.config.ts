import path from "node:path";
import { defineConfig } from "vitest/config";

// Separate from vite.config.ts on purpose: no Cloudflare plugin and no router plugin in unit tests.
// The route tree is the committed src/routeTree.gen.ts.
export default defineConfig({
  resolve: { alias: { "@": path.resolve(import.meta.dirname, "src") } },
  test: {
    environment: "jsdom",
    setupFiles: ["./src/test/setup.ts"],
    include: ["src/**/*.test.{ts,tsx}"],
    css: false,
  },
});
