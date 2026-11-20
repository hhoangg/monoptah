import { defineConfig, devices } from "@playwright/test";
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
  projects: [
    { name: "chromium", use: { browserName: "chromium" } },
    // Real WebKit with an iPhone profile (touch, 390 px, mobile UA) for what iOS Safari renders differently. The other
    // specs drive a Chromium-only virtual WebAuthn authenticator, so only specs that sign in without a browser
    // ceremony (an API session cookie) are listed here.
    { name: "webkit-mobile", use: { ...devices["iPhone 14"] }, testMatch: /(html-preview|push-deeplink)\.spec\.ts$/ },
  ],
});
