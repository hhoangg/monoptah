// A web push notification tap opens its target (Mono chat or session) on Chromium and on WebKit with an iPhone
// profile, against the dev preview of the app (an in-browser mock desktop).
//
// Playwright cannot raise a real `notificationclick` (no way to show or tap an OS notification), so the worker
// side is covered by the unit tests of src/sw/pushHandlers.ts. What these tests drive is the page side of the same
// contract, with the same inputs the worker produces:
//   - a window that is open gets `{type: "monoptah:navigate", url}` from the service worker (dispatched here on
//     navigator.serviceWorker, which is where the page listens), and
//   - a cold start finds the pending target the worker left in IndexedDB (written here the way the worker does).
// NOT verifiable here: the iOS standalone (home screen) launch itself, where `openWindow` may open the start URL.
import { spawn, type ChildProcess } from "node:child_process";
import { mkdtempSync, rmSync } from "node:fs";
import { createServer } from "node:net";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { expect, test, type Page } from "@playwright/test";
import { webDir } from "../lib/paths.ts";

const DESKTOP_ID = "11111111-1111-4111-8111-111111111111";
const PREVIEW = "/src/features/sessions/dev/";
const START = `${PREVIEW}?to=/d/${DESKTOP_ID}/p/proj-alpha`;
const MONO_URL = `/d/${DESKTOP_ID}/monos/mono-example`;
const SESSION_URL = `/d/${DESKTOP_ID}/s/sess-long`;
const PENDING_DB = "monoptah-pending-target";

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

/** What the service worker posts to an open window on a notification tap. */
async function postNavigate(page: Page, url: string): Promise<void> {
  await page.evaluate((target) => {
    navigator.serviceWorker.dispatchEvent(new MessageEvent("message", { data: { type: "monoptah:navigate", url: target } }));
  }, url);
}

/** What the service worker leaves for a page that is not ready to hear the message (sw/pendingTarget.ts). */
async function savePending(page: Page, url: string): Promise<void> {
  await page.evaluate(
    ([dbName, target]) =>
      new Promise<void>((resolve, reject) => {
        const request = indexedDB.open(dbName!, 1);
        request.onupgradeneeded = () => request.result.createObjectStore("pending");
        request.onerror = () => reject(request.error);
        request.onsuccess = () => {
          const tx = request.result.transaction("pending", "readwrite");
          tx.objectStore("pending").put(JSON.stringify({ url: target, at: Date.now() }), "target");
          tx.oncomplete = () => resolve();
          tx.onerror = () => reject(tx.error);
        };
      }),
    [PENDING_DB, url],
  );
}

// Runs under the `chromium` and `webkit-mobile` projects (webkit-mobile is real WebKit with an iPhone profile).
test.describe("push deep link", () => {
  test.describe.configure({ mode: "serial" });
  let vite: ChildProcess | null = null;
  let base = "";
  let stateDir = "";
  let page: Page;

  test.beforeAll(async () => {
    test.setTimeout(120_000);
    const port = await freePort();
    base = `http://localhost:${port}`;
    stateDir = mkdtempSync(join(tmpdir(), "monoptah-e2e-deeplink-"));
    vite = spawn(join(webDir, "app", "node_modules", ".bin", "vite"), ["--port", String(port), "--strictPort", "--host", "localhost"], {
      cwd: join(webDir, "app"),
      env: { ...process.env, VITE_MOCK_API: "1", MONOPTAH_PREVIEW_STATE: stateDir },
      stdio: "ignore",
    });
    await expect
      .poll(async () => (await fetch(`${base}${START}`).catch(() => null))?.status ?? 0, { timeout: 90_000, intervals: [500] })
      .toBe(200);
  });

  test.afterAll(() => {
    vite?.kill("SIGTERM");
    if (stateDir) rmSync(stateDir, { recursive: true, force: true });
  });

  test.beforeEach(async ({ browser }, testInfo) => {
    const context = await browser.newContext({ ...testInfo.project.use });
    page = await context.newPage();
  });

  test.afterEach(async () => {
    await page.context().close();
  });

  /** Writes the pending target from a page that does not run the app, so no app page can take it first. */
  async function seedPending(url: string): Promise<void> {
    await page.goto(`${base}/package.json`);
    await savePending(page, url);
  }

  test("an open window goes to the Mono chat of the tapped notification", async () => {
    await page.goto(`${base}${START}`);
    await expect(page).toHaveURL(new RegExp(`/d/${DESKTOP_ID}/p/proj-alpha`), { timeout: 60_000 });
    // The listener is installed by a lazily imported module: resend until the page has it (a real tap is one message).
    await expect(async () => {
      await postNavigate(page, MONO_URL);
      await expect(page).toHaveURL(new RegExp(`${MONO_URL}$`), { timeout: 1000 });
    }).toPass({ timeout: 30_000 });
    await expect(page.getByRole("link", { name: "Mono profile" })).toBeVisible({ timeout: 60_000 });
  });

  test("an open window goes to the session of the tapped notification", async () => {
    await page.goto(`${base}${START}`);
    await expect(page).toHaveURL(new RegExp(`/d/${DESKTOP_ID}/p/proj-alpha`), { timeout: 60_000 });
    await expect(async () => {
      await postNavigate(page, SESSION_URL);
      await expect(page).toHaveURL(new RegExp(`${SESSION_URL}$`), { timeout: 1000 });
    }).toPass({ timeout: 30_000 });
    await expect(page.locator('[data-route="/d/:desktopId/s/:sessionId"]')).toBeVisible({ timeout: 60_000 });
  });

  test("a cold start takes the pending Mono target once", async () => {
    await seedPending(MONO_URL);
    await page.goto(`${base}${START}`);
    await expect(page).toHaveURL(new RegExp(`${MONO_URL}$`), { timeout: 60_000 });
    // Consumed: a later start stays where it opens.
    await page.goto(`${base}${START}`);
    await expect(page).toHaveURL(new RegExp(`/d/${DESKTOP_ID}/p/proj-alpha$`), { timeout: 60_000 });
    await page.waitForTimeout(1500);
    expect(new URL(page.url()).pathname).toBe(`/d/${DESKTOP_ID}/p/proj-alpha`);
  });

  test("a cold start takes the pending session target", async () => {
    await seedPending(SESSION_URL);
    await page.goto(`${base}${START}`);
    await expect(page).toHaveURL(new RegExp(`${SESSION_URL}$`), { timeout: 60_000 });
  });
});
