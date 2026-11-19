import { defineConfig } from "@playwright/test";
import { outputDir } from "./lib/paths.ts";

// Everything Playwright writes (artifacts, traces, screenshots, HTML report, wrangler log) lives outside git.
const out = outputDir();

export default defineConfig({
  testDir: "./tests",
  outputDir: `${out}/artifacts`,
  globalSetup: "./lib/global-setup.ts",
  workers: 1,
  fullyParallel: false,
  retries: 0,
  timeout: 60_000,
  expect: { timeout: 10_000 },
  reporter: [["list"], ["html", { outputFolder: `${out}/report`, open: "never" }]],
  // Traces hold request bodies (invite and recovery codes of the throwaway local run): keep them only for failures.
  use: { trace: "retain-on-failure", screenshot: "on" },
  projects: [{ name: "chromium", use: { browserName: "chromium" } }],
});
