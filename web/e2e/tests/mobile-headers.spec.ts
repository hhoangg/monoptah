// Header and bottom-bar layout on an iPhone (WebKit, iPhone 13 descriptor, 390 px) against the dev preview of the app
// (an in-browser mock desktop): the Mono chat header has the info link and no duplicate menu, the project header is
// tidy (targets of 44 px, no sideways overflow, the branch chip shares a row with the panel tools), and the
// bottom-anchored bars (composer, now-working pill) leave the same gap above the home indicator.
import { spawn, type ChildProcess } from "node:child_process";
import { mkdtempSync, rmSync } from "node:fs";
import { createServer } from "node:net";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { devices, expect, test, webkit, type Browser, type Page } from "@playwright/test";
import { webDir } from "../lib/paths.ts";

const DESKTOP_ID = "11111111-1111-4111-8111-111111111111";
const PREVIEW = "/src/features/sessions/dev/";
const PROJECT = `${PREVIEW}?to=/d/${DESKTOP_ID}/p/proj-alpha&working=2`;
const MONO = `${PREVIEW}?to=/d/${DESKTOP_ID}/monos/mono-example`;
const SESSION = `${PREVIEW}?to=/d/${DESKTOP_ID}/p/proj-alpha/new`;

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

test.describe("headers and bottom bars on an iPhone", () => {
  test.describe.configure({ mode: "serial" });
  let vite: ChildProcess | null = null;
  let browser: Browser;
  let page: Page;
  let base = "";
  let stateDir = "";

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
      .poll(async () => (await fetch(`${base}${PROJECT}`).catch(() => null))?.status ?? 0, { timeout: 90_000, intervals: [500] })
      .toBe(200);
    browser = await webkit.launch();
  });

  test.afterAll(async () => {
    await browser?.close();
    vite?.kill("SIGTERM");
    if (stateDir) rmSync(stateDir, { recursive: true, force: true });
  });

  test.beforeEach(async () => {
    const context = await browser.newContext({ ...devices["iPhone 13"] });
    page = await context.newPage();
  });

  test.afterEach(async () => {
    await page.context().close();
  });

  test("Mono chat header: the info link opens the details and there is no second Mono actions button", async () => {
    await page.goto(`${base}${MONO}`);
    await expect(page.getByRole("link", { name: "Mono profile" })).toBeVisible({ timeout: 60_000 });
    await expect(page.getByRole("button", { name: "Mono actions" })).toHaveCount(0);
    const box = await page.getByRole("link", { name: "Mono profile" }).boundingBox();
    expect(box!.width).toBeGreaterThanOrEqual(44);
    expect(box!.height).toBeGreaterThanOrEqual(44);
  });

  test("project header: targets are 44 px, nothing overflows, the branch chip shares a row with the panel tools", async () => {
    await page.goto(`${base}${PROJECT}`);
    await expect(page.getByRole("heading", { level: 1 })).toBeVisible({ timeout: 60_000 });
    const bar = page.getByTestId("project-bar");
    await expect(bar).toBeVisible();
    const chip = page.getByRole("button", { name: /^Workspace:/ });
    await expect(chip).toBeVisible();

    const tools = await bar.getByRole("button").all();
    expect(tools.length).toBeGreaterThan(1);
    for (const button of [...tools, page.getByRole("link", { name: /new session/i }).or(page.getByRole("button", { name: /new session/i })).first()]) {
      const box = (await button.boundingBox())!;
      expect(box.height, await button.innerText()).toBeGreaterThanOrEqual(43.5);
      expect(box.width).toBeGreaterThanOrEqual(43.5);
    }

    // The branch chip and the panel tools are on one row.
    const chipBox = (await chip.boundingBox())!;
    const barBox = (await bar.boundingBox())!;
    expect(Math.abs(chipBox.y + chipBox.height / 2 - (barBox.y + barBox.height / 2))).toBeLessThanOrEqual(8);
    expect(chipBox.x + chipBox.width).toBeLessThanOrEqual(barBox.x + 1);

    // Nothing sticks out of the viewport or scrolls sideways.
    const found = await page.evaluate(() => {
      const width = document.documentElement.clientWidth;
      const outside: string[] = [];
      for (const el of Array.from(document.querySelectorAll("body *"))) {
        // A marquee (the ticker of the now-working pill) is wider than its box on purpose and clipped by it.
        if (el.parentElement?.closest("[class*='overflow-hidden'], [class*='truncate']")) continue;
        const rect = el.getBoundingClientRect();
        if (rect.width > 1 && rect.height > 1 && (rect.right > width + 1 || rect.left < -1)) outside.push(`${el.tagName.toLowerCase()}.${String(el.className).slice(0, 60)} ${Math.round(rect.left)}..${Math.round(rect.right)}`);
      }
      return { outside: outside.slice(0, 10), doc: [document.documentElement.scrollWidth, width] };
    });
    expect(found.outside).toEqual([]);
    expect(found.doc[0]).toBeLessThanOrEqual(found.doc[1]!);
  });

  test("project header at 320 px: a long branch name shrinks and truncates, the panel tools stay on screen", async () => {
    await page.setViewportSize({ width: 320, height: 700 });
    await page.goto(`${base}${PROJECT}`);
    const chip = page.getByRole("button", { name: /^Workspace:/ });
    await expect(chip).toBeVisible({ timeout: 60_000 });
    // Give the chip a very long label in place, as a long branch name would.
    await chip.locator("span.truncate").evaluate((el) => (el.textContent = "feature/a-very-long-branch-name-that-cannot-fit-in-a-phone-header-row"));
    const bar = page.getByTestId("project-bar");
    const last = (await bar.getByRole("button").last().boundingBox())!;
    expect(last.x + last.width).toBeLessThanOrEqual(320);
    const chipBox = (await chip.boundingBox())!;
    expect(chipBox.x + chipBox.width).toBeLessThanOrEqual((await bar.boundingBox())!.x + 1);
    expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(320);
  });

  async function gapBelow(selector: string): Promise<number> {
    return page.locator(selector).first().evaluate((el) => window.innerHeight - el.getBoundingClientRect().bottom);
  }

  test("bottom bars: the now-working pill and the composer leave the same gap above the viewport bottom", async () => {
    await page.goto(`${base}${PROJECT}`);
    const pill = page.getByTestId("now-working");
    await expect(pill).toBeVisible({ timeout: 60_000 });
    const inset = await page.evaluate(() => {
      const probe = document.createElement("div");
      probe.style.cssText = "position:fixed;visibility:hidden;padding-bottom:var(--app-bottom-inset)";
      document.body.append(probe);
      const value = parseFloat(getComputedStyle(probe).paddingBottom) || 0;
      probe.remove();
      return value;
    });
    const pillGap = await gapBelow('[data-testid="now-working"] > :last-child');
    expect(Math.abs(pillGap - inset)).toBeLessThanOrEqual(1);
    // The wrapper itself carries the shared inset as its bottom padding.
    expect(await pill.evaluate((el) => parseFloat(getComputedStyle(el).paddingBottom))).toBeCloseTo(inset, 0);

    await page.goto(`${base}${SESSION}`);
    await expect(page.getByTestId("composer")).toBeVisible({ timeout: 60_000 });
    const composerGap = await gapBelow('[data-testid="composer"] [data-shape]');
    expect(Math.abs(composerGap - pillGap)).toBeLessThanOrEqual(1);
  });

  test("project header: exactly one h1", async () => {
    await page.goto(`${base}${PROJECT}`);
    await expect(page.getByRole("heading", { level: 1 })).toHaveCount(1, { timeout: 60_000 });
  });

  test("project page with one session: the pill still sits on the inset, not 10rem above it", async () => {
    await page.goto(`${base}${PREVIEW}?to=/d/${DESKTOP_ID}/p/proj-alpha&sessions=1&working=1`);
    await expect(page.getByTestId("now-working")).toBeVisible({ timeout: 60_000 });
    const gap = await page.getByTestId("now-working").evaluate((el) => window.innerHeight - el.getBoundingClientRect().bottom);
    expect(Math.abs(gap)).toBeLessThanOrEqual(1);
  });

  test("Mono chat on a desktop keeps its Mono actions menu", async () => {
    await page.setViewportSize({ width: 1280, height: 800 });
    await page.goto(`${base}${MONO}`);
    await expect(page.getByRole("button", { name: "Mono actions" })).toHaveCount(1, { timeout: 60_000 });
  });

  test("tablet and desktop: header fits, the project entry points stay", async () => {
    for (const viewport of [{ width: 768, height: 1024 }, { width: 1280, height: 800 }]) {
      await page.setViewportSize(viewport);
      await page.goto(`${base}${PROJECT}`);
      // Below 1024px the project page has the New session pill; from 1024px the session list pane does.
      await expect(viewport.width < 1024 ? page.getByRole("link", { name: "New session" }).first() : page.getByTestId("session-list-pane")).toBeVisible({ timeout: 60_000 });
      const [scroll, client] = await page.evaluate(() => [document.documentElement.scrollWidth, document.documentElement.clientWidth]);
      expect(scroll).toBeLessThanOrEqual(client!);
    }
  });
});
