// Browser spikes (S-03, S-08): chromium, firefox and webkit against local static servers that send the
// exact CSP of web/worker/src/csp.ts. Reports and traces go to /tmp/evidence/ws-q/browser (never git).
import { defineConfig, devices } from "@playwright/test";

const OUT = "/tmp/evidence/ws-q/browser";

export default defineConfig({
  testDir: ".",
  testMatch: ["**/*.browser.spec.ts", "s-0*-*/browser/**/*.spec.ts"],
  testIgnore: ["**/node_modules/**", "**/dist/**"],
  outputDir: `${OUT}/results`,
  reporter: [["list"], ["json", { outputFile: `${OUT}/report.json` }], ["html", { outputFolder: `${OUT}/html`, open: "never" }]],
  fullyParallel: false,
  workers: 1,
  timeout: 90_000,
  retries: 0,
  use: { trace: "retain-on-failure", screenshot: "only-on-failure" },
  projects: [
    { name: "chromium", use: { ...devices["Desktop Chrome"] } },
    { name: "firefox", use: { ...devices["Desktop Firefox"] } },
    { name: "webkit", use: { ...devices["Desktop Safari"] } },
  ],
  webServer: {
    command: "node s-08-csp/serve-all.ts",
    url: "http://localhost:4180/",
    timeout: 180_000,
    reuseExistingServer: process.env["SPIKE_REUSE_SERVER"] === "1",
    stdout: "pipe",
  },
});
