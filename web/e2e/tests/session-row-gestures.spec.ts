// The session list of a project on touch and on a mouse, against the dev preview of the app (an in-browser mock
// desktop): the row menu button has vertical dots; on touch the button is gone, a long press opens the same menu and a
// right-to-left swipe reveals Archive and Delete (iOS style) without taking over the vertical scroll; the New session
// button is an icon. Three environments: an iPhone profile in WebKit (mobile), a touch tablet in Chromium, and a
// mouse desktop in Chromium. Chromium gets real touch input through CDP (`Input.dispatchTouchEvent`, so the browser's
// own scroll arbitration is in play); WebKit has no touch injection in Playwright, so its gestures are dispatched as
// touch PointerEvents on the page (the app's logic, not the engine's scroll arbitration).
//
// Set SESSION_LIST_SHOTS=<dir> to also save screenshots of the gesture states there.
import { spawn, type ChildProcess } from "node:child_process";
import { mkdirSync, mkdtempSync, rmSync } from "node:fs";
import { createServer } from "node:net";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { chromium, devices, expect, test, webkit, type Browser, type BrowserContextOptions, type BrowserType, type Page } from "@playwright/test";
import { webDir } from "../lib/paths.ts";

const DESKTOP_ID = "11111111-1111-4111-8111-111111111111";
const PREVIEW = "/src/features/sessions/dev/";
const PROJECT = `${PREVIEW}?to=/d/${DESKTOP_ID}/p/proj-alpha&theme=dark`;
const SESSION = `${PREVIEW}?to=/d/${DESKTOP_ID}/s/sess-local-1&theme=dark`;
const SHOTS = process.env["SESSION_LIST_SHOTS"];

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

interface Environment {
  name: "mobile" | "tablet" | "desktop";
  type: BrowserType;
  options: BrowserContextOptions;
  touch: boolean;
}

const ENVIRONMENTS: Environment[] = [
  { name: "mobile", type: webkit, options: { ...devices["iPhone 14"] }, touch: true },
  { name: "tablet", type: chromium, options: { viewport: { width: 834, height: 1112 }, hasTouch: true, isMobile: true, deviceScaleFactor: 2 }, touch: true },
  { name: "desktop", type: chromium, options: { viewport: { width: 1440, height: 900 } }, touch: false },
];

type Step = { type: "down" | "move" | "up"; x: number; y: number; wait?: number };

/** One finger: real touch input through CDP on Chromium, touch PointerEvents on the page in WebKit. */
async function finger(page: Page, steps: Step[]): Promise<void> {
  if (page.context().browser()?.browserType().name() === "chromium") {
    const cdp = await page.context().newCDPSession(page);
    for (const step of steps) {
      if (step.wait) await page.waitForTimeout(step.wait);
      const type = step.type === "down" ? "touchStart" : step.type === "move" ? "touchMove" : "touchEnd";
      await cdp.send("Input.dispatchTouchEvent", { type, touchPoints: step.type === "up" ? [] : [{ x: step.x, y: step.y, id: 1 }] });
    }
    await cdp.detach();
    return;
  }
  await page.evaluate(async (all) => {
    let target: Element | null = null;
    const first = all[0]!;
    for (const step of all) {
      if (step.wait) await new Promise((resolve) => setTimeout(resolve, step.wait));
      if (step.type === "down") target = document.elementFromPoint(step.x, step.y);
      const init = { bubbles: true, cancelable: true, composed: true, pointerType: "touch", pointerId: 7, isPrimary: true, button: 0, buttons: step.type === "up" ? 0 : 1, clientX: step.x, clientY: step.y };
      target?.dispatchEvent(new PointerEvent(`pointer${step.type}`, init));
      // A real browser clicks only when the finger stayed within the tap slop (a scroll or a swipe makes no click).
      if (step.type === "up" && Math.hypot(step.x - first.x, step.y - first.y) < 10) {
        target?.dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true, composed: true, button: 0, clientX: step.x, clientY: step.y }));
      }
    }
  }, steps);
}

/** A slow horizontal drag across a row from `fromX` to `toX` at the row's middle. */
function swipePath(box: { x: number; y: number; width: number; height: number }, dx: number, { slow = true } = {}): Step[] {
  const y = box.y + box.height / 2;
  const x0 = box.x + box.width - 60;
  const steps: Step[] = [{ type: "down", x: x0, y }];
  const count = 6;
  for (let i = 1; i <= count; i++) steps.push({ type: "move", x: x0 + (dx * i) / count, y, wait: slow ? 40 : 4 });
  steps.push({ type: "up", x: x0 + dx, y, wait: slow ? 250 : 0 });
  return steps;
}

for (const env of ENVIRONMENTS) {
  test.describe(`session rows: ${env.name}`, () => {
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
      stateDir = mkdtempSync(join(tmpdir(), "monoptah-e2e-rows-"));
      vite = spawn(join(webDir, "app", "node_modules", ".bin", "vite"), ["--port", String(port), "--strictPort", "--host", "localhost"], {
        cwd: join(webDir, "app"),
        env: { ...process.env, VITE_MOCK_API: "1", MONOPTAH_PREVIEW_STATE: stateDir },
        stdio: "ignore",
      });
      await expect
        .poll(async () => (await fetch(`${base}${PROJECT}`).catch(() => null))?.status ?? 0, { timeout: 90_000, intervals: [500] })
        .toBe(200);
      browser = await env.type.launch();
      if (SHOTS) mkdirSync(SHOTS, { recursive: true });
    });

    test.afterAll(async () => {
      await browser?.close();
      vite?.kill("SIGTERM");
      if (stateDir) rmSync(stateDir, { recursive: true, force: true });
    });

    test.beforeEach(async () => {
      const context = await browser.newContext(env.options);
      page = await context.newPage();
    });

    test.afterEach(async () => {
      await page.context().close();
    });

    const cards = () => page.getByTestId("session-card");
    /** A session that is in no folder, so the list rows are all alike. */
    const rowByTitle = (title: string) => cards().filter({ hasText: title }).first();
    const link = (title: string) => rowByTitle(title).getByRole("link").first();

    async function openProject(): Promise<void> {
      await page.goto(`${base}${PROJECT}`);
      await expect(cards().first()).toBeVisible({ timeout: 60_000 });
    }

    async function shot(name: string): Promise<void> {
      // Let the 200 ms slide of a row and the dialogs' fade finish first.
      if (SHOTS) await page.waitForTimeout(500);
      if (SHOTS) await page.screenshot({ path: join(SHOTS, `after-${env.name}-${name}.png`) });
    }

    if (env.touch) {
      test("the row button is gone; the row blocks text selection and the iOS callout", async () => {
        await openProject();
        // Not drawn (visually hidden, 1 px) but still in the accessibility tree.
        const hidden = cards().getByRole("button", { name: /^Actions for / }).first();
        const hiddenBox = (await hidden.boundingBox())!;
        expect(hiddenBox.width).toBeLessThanOrEqual(1);
        const style = await link("Write the example report").evaluate((element) => {
          const surface = element.parentElement!;
          const computed = getComputedStyle(surface);
          return { userSelect: computed.userSelect || computed.webkitUserSelect, touchAction: computed.touchAction, callout: computed.getPropertyValue("-webkit-touch-callout") };
        });
        expect(style.userSelect).toBe("none");
        expect(style.touchAction).toBe("pan-y");
        // Only WebKit knows the property; where it does, the surface must turn the callout off.
        if (await page.evaluate(() => CSS.supports("-webkit-touch-callout", "none"))) expect(style.callout).toBe("none");
      });

      test("a long press opens the same menu and the release does not open the session", async () => {
        await openProject();
        const row = link("Write the example report");
        const box = (await row.boundingBox())!;
        const x = box.x + 80;
        const y = box.y + box.height / 2;
        await finger(page, [
          { type: "down", x, y },
          { type: "up", x, y, wait: 750 },
        ]);
        const menu = page.getByRole("menu");
        await expect(menu).toBeVisible();
        for (const name of ["Open", "Rename", "Archive", "Delete"]) await expect(menu.getByRole("menuitem", { name })).toBeVisible();
        await expect(page).toHaveURL(new RegExp(`/d/${DESKTOP_ID}/p/proj-alpha$`));
        await shot("long-press-menu");
        await page.keyboard.press("Escape");
        await expect(menu).toHaveCount(0);
        // A tap still opens the session.
        await row.click();
        await expect(page).toHaveURL(new RegExp(`/d/${DESKTOP_ID}/s/`));
      });

      test("a short press that moves (a scroll) opens no menu", async () => {
        await openProject();
        const box = (await link("Write the example report").boundingBox())!;
        const x = box.x + 80;
        const y = box.y + box.height / 2;
        await finger(page, [
          { type: "down", x, y },
          { type: "move", x, y: y - 30, wait: 100 },
          { type: "up", x, y: y - 30, wait: 700 },
        ]);
        await expect(page.getByRole("menu")).toHaveCount(0);
      });

      test("swiping a row from right to left reveals Archive and Delete; a tap outside closes them", async () => {
        await openProject();
        const row = rowByTitle("Write the example report");
        const box = (await link("Write the example report").boundingBox())!;
        await finger(page, swipePath(box, -110));
        await expect(row).toHaveAttribute("data-swipe-open", "true");
        const archive = row.getByRole("button", { name: /^Archive / });
        const remove = row.getByRole("button", { name: /^Delete / });
        await expect(archive).toBeVisible();
        await expect(remove).toBeVisible();
        for (const button of [archive, remove]) {
          const size = (await button.boundingBox())!;
          expect(size.height).toBeGreaterThanOrEqual(44);
          expect(size.width).toBeGreaterThanOrEqual(44);
        }
        // Fully on screen, not half hidden.
        const viewport = page.viewportSize()!;
        // (The row slides for 200 ms: measure once it has settled.)
        await expect.poll(async () => { const b = (await remove.boundingBox())!; return b.x + b.width; }).toBeLessThanOrEqual(viewport.width + 0.5);
        await shot("swipe-open");
        // The row did not navigate.
        await expect(page).toHaveURL(new RegExp(`/d/${DESKTOP_ID}/p/proj-alpha$`));
        // A tap outside closes it.
        await page.mouse.click(viewport.width / 2, 20);
        await expect(row).not.toHaveAttribute("data-swipe-open", "true");
      });

      test("swiping back closes, and only one row is open at a time", async () => {
        await openProject();
        const first = rowByTitle("Write the example report");
        const second = rowByTitle("Refactor on the fake codespace");
        let box = (await link("Write the example report").boundingBox())!;
        await finger(page, swipePath(box, -110));
        await expect(first).toHaveAttribute("data-swipe-open", "true");
        box = (await link("Refactor on the fake codespace").boundingBox())!;
        await finger(page, swipePath(box, -110));
        await expect(second).toHaveAttribute("data-swipe-open", "true");
        await expect(first).not.toHaveAttribute("data-swipe-open", "true");
        // Back to the right closes it.
        const open = (await link("Refactor on the fake codespace").boundingBox())!;
        await finger(page, swipePath({ ...open, x: open.x - 110 }, 110));
        await expect(second).not.toHaveAttribute("data-swipe-open", "true");
      });

      // The mock desktop acknowledges session.archive and session.delete but keeps the session in its list, so the
      // proof of success here is the dialog closing with no error notice (the unit tests check the request itself).
      test("Archive asks first, then archives", async () => {
        await openProject();
        const title = "Write the example report";
        const row = rowByTitle(title);
        await finger(page, swipePath((await link(title).boundingBox())!, -110));
        await row.getByRole("button", { name: /^Archive / }).click();
        const dialog = page.getByRole("dialog", { name: "Archive this session?" });
        await expect(dialog).toBeVisible();
        await expect(row).not.toHaveAttribute("data-swipe-open", "true");
        await dialog.getByRole("button", { name: "Archive", exact: true }).click();
        await expect(dialog).toHaveCount(0);
      });

      test("Delete asks for confirmation: Cancel keeps the session, Delete deletes it", async () => {
        await openProject();
        const title = "Write the example report";
        const row = rowByTitle(title);
        await finger(page, swipePath((await link(title).boundingBox())!, -110));
        await row.getByRole("button", { name: /^Delete / }).click();
        const dialog = page.getByRole("dialog", { name: "Delete this session?" });
        await expect(dialog).toBeVisible();
        await shot("delete-confirm");
        await dialog.getByRole("button", { name: "Cancel" }).click();
        await expect(dialog).toHaveCount(0);
        await expect(cards().filter({ hasText: title })).toHaveCount(1);
        await finger(page, swipePath((await link(title).boundingBox())!, -110));
        await row.getByRole("button", { name: /^Delete / }).click();
        const confirm = page.getByRole("dialog", { name: "Delete this session?" });
        await confirm.getByRole("button", { name: "Delete", exact: true }).click();
        await expect(confirm).toHaveCount(0);
      });

      test("a vertical drag on a row scrolls the list and does not move or open the row", async () => {
        await openProject();
        const title = "Write the example report";
        const row = rowByTitle(title);
        const scrollTop = () =>
          page.evaluate(() => {
            let sum = (document.scrollingElement?.scrollTop ?? 0) + window.scrollY;
            for (const element of Array.from(document.querySelectorAll("*"))) sum += element.scrollTop;
            return sum;
          });
        const before = await scrollTop();
        const box = (await link(title).boundingBox())!;
        const x = box.x + box.width - 60;
        const y = box.y + box.height / 2;
        // Mostly vertical, with a little sideways drift like a real thumb.
        const steps: Step[] = [{ type: "down", x, y }];
        for (let i = 1; i <= 8; i++) steps.push({ type: "move", x: x - i * 2, y: y - i * 25, wait: 16 });
        steps.push({ type: "up", x: x - 16, y: y - 200, wait: 300 });
        await finger(page, steps);
        await expect(row).not.toHaveAttribute("data-swipe-open", "true");
        await expect(page).toHaveURL(new RegExp(`/d/${DESKTOP_ID}/p/proj-alpha$`));
        const transform = await link(title).evaluate((element) => element.parentElement!.style.transform);
        expect(transform).toMatch(/^translate3d\(0(px)?, 0(px)?, 0(px)?\)$/);
        // Chromium runs the engine's own scrolling for the injected touch: the list moved. (The WebKit run drives
        // synthetic pointer events, which scroll nothing, so there it proves only that the row stays put.)
        if (env.type === chromium) expect(await scrollTop()).toBeGreaterThan(before);
      });

      test("a read-only device gets no swipe actions", async () => {
        await page.goto(`${base}${PROJECT}&permission=read`);
        await expect(cards().first()).toBeVisible({ timeout: 60_000 });
        const title = "Write the example report";
        await finger(page, swipePath((await link(title).boundingBox())!, -110));
        await expect(rowByTitle(title)).not.toHaveAttribute("data-swipe-open", "true");
        await expect(page.locator("[data-swipe-actions]")).toHaveCount(0);
      });

      test("the folder header keeps its button with vertical dots and is not swipeable", async () => {
        await openProject();
        const header = page.getByTestId("folder-header").first();
        const button = header.getByRole("button", { name: /^Actions for folder/ });
        await expect(button).toBeVisible();
        expect(await button.locator("[data-icon]").getAttribute("data-icon")).toBe("moreVertical");
        const box = (await header.boundingBox())!;
        await finger(page, swipePath(box, -110));
        await expect(header.locator("[data-swipe-actions]")).toHaveCount(0);
      });
    } else {
      test("the row button is a vertical three dots menu and there is no swipe or long press", async () => {
        await openProject();
        const row = rowByTitle("Write the example report");
        const button = row.getByRole("button", { name: /^Actions for / });
        await expect(button).toBeVisible();
        expect(await button.locator("[data-icon]").getAttribute("data-icon")).toBe("moreVertical");
        await expect(page.locator("[data-swipe-actions]")).toHaveCount(0);
        await shot("session-list");
        await button.click();
        const menu = page.getByRole("menu");
        await expect(menu.getByRole("menuitem", { name: "Archive" })).toBeVisible();
        await expect(menu.getByRole("menuitem", { name: "Delete" })).toBeVisible();
        await page.keyboard.press("Escape");
        // A mouse press-and-hold opens nothing, a drag moves nothing.
        const box = (await link("Write the example report").boundingBox())!;
        await page.mouse.move(box.x + 80, box.y + box.height / 2);
        await page.mouse.down();
        await page.waitForTimeout(700);
        await page.mouse.up();
        await expect(menu).toHaveCount(0);
      });

      test("the folder button has vertical dots too", async () => {
        await openProject();
        const button = page.getByTestId("folder-header").first().getByRole("button", { name: /^Actions for folder/ });
        expect(await button.locator("[data-icon]").getAttribute("data-icon")).toBe("moreVertical");
      });
    }

    test("New session is an icon with its accessible name: no label, a tooltip on a mouse, a 44 px target on touch", async () => {
      // From 1024 px the project URL shows the New session page itself, so the list pane's button is on a session.
      await page.goto(`${base}${env.name === "desktop" ? SESSION : PROJECT}`);
      const button = page.getByRole("link", { name: "New session", exact: true }).first();
      await expect(button).toBeVisible({ timeout: 60_000 });
      expect((await button.innerText()).trim()).toBe("");
      expect(await button.locator("svg").count()).toBe(1);
      const box = (await button.boundingBox())!;
      if (env.touch) {
        expect(box.width).toBeGreaterThanOrEqual(44);
        expect(box.height).toBeGreaterThanOrEqual(44);
      } else {
        await button.hover();
        await expect(page.getByRole("tooltip").first()).toContainText("New session");
      }
      await shot("new-session-icon");
    });
  });
}
