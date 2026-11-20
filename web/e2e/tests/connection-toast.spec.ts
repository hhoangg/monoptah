// The connection banner is an overlay toast: toggling the connection state must not move the tab bar or the
// transcript (layout shift = 0), on a phone (WebKit, iPhone 13) and on a desktop-sized window (Chromium). The app is the
// dev preview (an in-browser mock desktop); "offline" is the browser going offline, which the banner shows.
//
// Set CONNECTION_TOAST_SHOTS=<dir> and CONNECTION_TOAST_PHASE=before|after to also save screenshots there.
import { spawn, type ChildProcess } from "node:child_process";
import { mkdirSync, mkdtempSync, rmSync } from "node:fs";
import { createServer } from "node:net";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { chromium, devices, expect, test, webkit, type Browser, type BrowserContextOptions, type Page } from "@playwright/test";
import { webDir } from "../lib/paths.ts";

const DESKTOP_ID = "11111111-1111-4111-8111-111111111111";
const PREVIEW = "/src/features/sessions/dev/";
const SESSION = `${PREVIEW}?to=/d/${DESKTOP_ID}/s/sess-long`;
const SHOTS = process.env["CONNECTION_TOAST_SHOTS"];
const PHASE = process.env["CONNECTION_TOAST_PHASE"] ?? "after";

async function freePort(): Promise<number> {
  return new Promise((resolve, reject) => {
    const server = createServer();
    server.once("error", reject);
    server.listen(0, "127.0.0.1", () => {
      const address = server.address();
      const port = typeof address === "object" && address !== null ? address.port : 0;
      server.close(() => resolve(port));
    });
  });
}

interface Box {
  x: number;
  y: number;
  width: number;
  height: number;
}

async function box(page: Page, testId: string): Promise<Box> {
  const found = await page.getByTestId(testId).first().boundingBox();
  if (!found) throw new Error(`${testId} has no box`);
  return found;
}

async function boxes(page: Page, ids: string[]): Promise<Box[]> {
  return Promise.all(ids.map((id) => box(page, id)));
}

test.describe("connection toast does not shift the layout", () => {
  let vite: ChildProcess | null = null;
  let base = "";
  let stateDir = "";
  const browsers: Browser[] = [];

  test.beforeAll(async () => {
    test.setTimeout(120_000);
    const port = await freePort();
    base = `http://localhost:${port}`;
    stateDir = mkdtempSync(join(tmpdir(), "monoptah-e2e-preview-"));
    vite = spawn(join(webDir, "app", "node_modules", ".bin", "vite"), ["--port", String(port), "--strictPort", "--host", "localhost"], {
      cwd: join(webDir, "app"),
      env: { ...process.env, VITE_MOCK_API: "1", MONOPTAH_PREVIEW_STATE: stateDir },
      stdio: "ignore",
    });
    await expect
      .poll(async () => (await fetch(`${base}${SESSION}`).catch(() => null))?.status ?? 0, { timeout: 90_000, intervals: [500] })
      .toBe(200);
  });

  test.afterAll(async () => {
    for (const browser of browsers) await browser.close();
    vite?.kill("SIGTERM");
    if (stateDir) rmSync(stateDir, { recursive: true, force: true });
  });

  const targets: { name: string; launch: () => Promise<Browser>; context: BrowserContextOptions }[] = [
    { name: "mobile", launch: () => webkit.launch(), context: { ...devices["iPhone 13"] } },
    { name: "tablet", launch: () => chromium.launch(), context: { viewport: { width: 820, height: 1180 }, hasTouch: true } },
    { name: "desktop", launch: () => chromium.launch(), context: { viewport: { width: 1280, height: 800 } } },
  ];

  for (const target of targets) {
    test(`${target.name}: going offline and back moves neither the tab bar nor the transcript`, async () => {
      const browser = await target.launch();
      browsers.push(browser);
      const context = await browser.newContext(target.context);
      const page = await context.newPage();
      await page.goto(`${base}${SESSION}`);
      await expect(page.getByTestId("transcript").first()).toBeVisible({ timeout: 60_000 });
      // Wait for the link: the first connect moves the layout by itself, which is not what is measured here.
      await expect(page.getByTestId("transcript-turn").first()).toBeVisible({ timeout: 60_000 });
      // The tab strip exists once a tab is open (wide windows); the top bar above the content is on every screen size.
      const bars = (await page.getByTestId("tab-strip").count()) > 0 ? ["desktop-top-bar", "tab-strip"] : ["desktop-top-bar"];
      await expect(page.getByTestId("desktop-top-bar").first()).toBeVisible();
      const banner = page.getByTestId("connection-banner");
      await expect(banner).toBeEmpty();
      const barsBefore = await boxes(page, bars);
      const transcriptBefore = await box(page, "transcript");
      if (SHOTS) {
        mkdirSync(SHOTS, { recursive: true });
        await page.screenshot({ path: join(SHOTS, `${PHASE}-${target.name}-online.png`) });
      }

      await context.setOffline(true);
      await expect(banner).toContainText("You are offline", { timeout: 5_000 });
      // The toast has a delay and an enter animation: let both finish before measuring.
      await page.waitForTimeout(1_200);
      if (SHOTS) await page.screenshot({ path: join(SHOTS, `${PHASE}-${target.name}-offline.png`) });
      expect(await boxes(page, bars)).toEqual(barsBefore);
      expect(await box(page, "transcript")).toEqual(transcriptBefore);

      await context.setOffline(false);
      await expect(banner).toBeEmpty({ timeout: 5_000 });
      expect(await boxes(page, bars)).toEqual(barsBefore);
      expect(await box(page, "transcript")).toEqual(transcriptBefore);

      // The relay link state "Reconnecting..." (with Retry) is an overlay too.
      await page.evaluate(() => (window as unknown as { __monoptahLinkStore: { setState: (s: object) => void } }).__monoptahLinkStore.setState({ status: "reconnecting" }));
      await expect(banner).toContainText("Reconnecting...", { timeout: 5_000 });
      await expect(banner.getByRole("button", { name: "Retry" })).toBeVisible();
      await page.waitForTimeout(600);
      if (SHOTS) await page.screenshot({ path: join(SHOTS, `${PHASE}-${target.name}-reconnecting.png`) });
      expect(await boxes(page, bars)).toEqual(barsBefore);
      expect(await box(page, "transcript")).toEqual(transcriptBefore);
      await context.close();
    });
  }
});
