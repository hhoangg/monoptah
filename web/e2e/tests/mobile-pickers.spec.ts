// The composer pickers of the New session screen on an iPhone: the workspace (branch) chip, the "Where it runs" target
// chip, the model chip and the access (permission) chip each open by a finger tap, and a pick takes effect.
//
// Why: a tap sends pointerdown, pointerup, then the compatibility mousedown and click. On iOS WebKit that mousedown moves
// the focus out of the autofocused field, which collapsed the composer card and shrank its chips to 0x0 before the click,
// so the model and target chips (which open on click) opened nothing. The test runs in WebKit with the iPhone 13
// descriptor (touch, 390 px), against the dev preview of the app (an in-browser mock desktop with models, access modes and
// cloud targets), which it serves itself with `vite`. It needs no relay, account or passkey.
//
// Set MOBILE_PICKERS_SCREENSHOTS=<dir> to also save a screenshot of each open picker there.
import { spawn, type ChildProcess } from "node:child_process";
import { mkdirSync, mkdtempSync, rmSync } from "node:fs";
import { createServer } from "node:net";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { devices, expect, test, webkit, type Browser, type Page } from "@playwright/test";
import { webDir } from "../lib/paths.ts";

// The desktop and project the preview seeds (features/sessions/dev/mockLink.ts).
const DESKTOP_ID = "11111111-1111-4111-8111-111111111111";
const NEW_SESSION = `/src/features/sessions/dev/?to=/d/${DESKTOP_ID}/p/proj-alpha/new`;

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

test.describe("composer pickers on an iPhone", () => {
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
    // The dev server runs its own Miniflare (the Cloudflare plugin): keep its state out of the repo and open no
    // inspector port, so it cannot collide with the harness's wrangler dev (see MONOPTAH_PREVIEW_STATE in vite.config.ts).
    stateDir = mkdtempSync(join(tmpdir(), "monoptah-e2e-preview-"));
    vite = spawn(join(webDir, "app", "node_modules", ".bin", "vite"), ["--port", String(port), "--strictPort", "--host", "localhost"], {
      cwd: join(webDir, "app"),
      env: { ...process.env, VITE_MOCK_API: "1", MONOPTAH_PREVIEW_STATE: stateDir },
      stdio: "ignore",
    });
    await expect
      .poll(async () => (await fetch(`${base}${NEW_SESSION}`).catch(() => null))?.status ?? 0, { timeout: 90_000, intervals: [500] })
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
    await page.goto(`${base}${NEW_SESSION}`);
    await expect(page.getByTestId("new-session-screen")).toBeVisible({ timeout: 60_000 });
    // The field has the focus on open (autoFocus): the state in which the tap on a chip used to be lost.
    await expect(page.getByRole("textbox", { name: "Message" })).toBeFocused();
  });

  test.afterEach(async () => {
    await page.context().close();
  });

  const shotDir = process.env["MOBILE_PICKERS_SCREENSHOTS"];
  async function shot(name: string): Promise<void> {
    if (!shotDir) return;
    mkdirSync(shotDir, { recursive: true });
    await page.screenshot({ path: join(shotDir, `${name}.png`) });
  }

  test("the phone has a coarse pointer and the narrow layout", async () => {
    expect(await page.evaluate(() => [matchMedia("(pointer: coarse)").matches, matchMedia("(min-width: 640px)").matches])).toEqual([true, false]);
  });

  test("workspace (branch) chip: a tap opens the menu and New worktree can be picked", async () => {
    await page.getByRole("button", { name: /^Workspace:/ }).tap();
    const menu = page.getByRole("menu");
    await expect(menu).toBeVisible();
    await shot("workspace-open");
    await menu.getByRole("menuitem", { name: "New worktree" }).tap();
    await expect(menu).toHaveCount(0);
    await expect(page.getByRole("button", { name: "New worktree, remove" })).toBeVisible();
  });

  test("target chip: a tap opens the sheet and a target can be picked", async () => {
    const chip = page.getByTestId("target-chip");
    await expect(chip).toHaveAccessibleName("Where it runs: Local");
    await chip.tap();
    const sheet = page.getByRole("dialog", { name: "Where it runs" });
    await expect(sheet).toBeVisible();
    await shot("target-open");
    await sheet.getByText("Docker sandbox", { exact: true }).tap();
    await sheet.getByRole("button", { name: "Done" }).tap();
    await expect(sheet).toHaveCount(0);
    await expect(chip).toHaveAccessibleName(/^Where it runs: .*Docker/);
  });

  test("model chip: a tap opens the sheet and another model can be picked", async () => {
    const chip = page.locator("[data-model-controls] button[aria-label^='Model:']");
    const before = await chip.getAttribute("aria-label");
    await chip.tap();
    const sheet = page.getByRole("dialog", { name: "Model" });
    await expect(sheet).toBeVisible();
    await shot("model-open");
    // Every provider is a folding section; open the Codex one and take its model.
    await sheet.getByRole("button", { name: /Codex/ }).first().tap();
    const pick = sheet.getByRole("list", { name: "Codex models" }).getByRole("button").first();
    const name = ((await pick.innerText()).split("\n")[0] ?? "").trim();
    expect(name).not.toBe("");
    await pick.tap();
    // The sheet stays open after a pick (more options may follow); dismiss it and check the chip.
    await page.keyboard.press("Escape");
    await expect(sheet).toHaveCount(0);
    await expect(chip).not.toHaveAttribute("aria-label", before ?? "");
    await expect(chip).toHaveAttribute("aria-label", new RegExp(`^Model: ${name.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")}`));
  });

  test("access (permission) chip: a tap opens the menu and a mode can be picked", async () => {
    const chip = page.getByRole("button", { name: /^Access:/ });
    await expect(chip).toHaveAccessibleName("Access: Supervised");
    await chip.tap();
    const menu = page.getByRole("menu");
    await expect(menu).toBeVisible();
    await shot("access-open");
    await menu.getByRole("menuitemradio", { name: /Auto-accept edits/ }).tap();
    await expect(menu).toHaveCount(0);
    await expect(chip).toHaveAccessibleName("Access: Auto-accept edits");
  });

  // The collapse rule: the card opens with the field and closes only on an interaction OUTSIDE the composer, never because
  // focus moved inside it (iOS WebKit blurs the empty field when a chip is tapped, whatever the timing).
  const glass = () => page.getByTestId("composer").locator("[data-shape]");

  test("an empty field and a tap on the model chip: the card stays, the sheet opens, a pick works, the chips keep working", async () => {
    await expect(glass()).toHaveAttribute("data-shape", "card");
    const chip = page.locator("[data-model-controls] button[aria-label^='Model:']");
    const before = await chip.getAttribute("aria-label");
    await chip.tap();
    const sheet = page.getByRole("dialog", { name: "Model" });
    await expect(sheet).toBeVisible();
    await expect(glass()).toHaveAttribute("data-shape", "card");
    await sheet.getByRole("button", { name: /Codex/ }).first().tap();
    await sheet.getByRole("list", { name: "Codex models" }).getByRole("button").first().tap();
    // A tap inside the open sheet (a portaled overlay) does not count as outside.
    await expect(glass()).toHaveAttribute("data-shape", "card");
    await page.keyboard.press("Escape");
    await expect(sheet).toHaveCount(0);
    await expect(chip).not.toHaveAttribute("aria-label", before ?? "");
    await expect(glass()).toHaveAttribute("data-shape", "card");
    // The composer is sane: the next chip opens on its first tap too.
    await page.getByTestId("target-chip").tap();
    await expect(page.getByRole("dialog", { name: "Where it runs" })).toBeVisible();
  });

  test("with the keyboard dismissed (field blurred, empty) the card stays and the chip opens on the first tap", async () => {
    await page.getByRole("textbox", { name: "Message" }).evaluate((element) => (element as HTMLElement).blur());
    await expect(glass()).toHaveAttribute("data-shape", "card");
    await page.locator("[data-model-controls] button[aria-label^='Model:']").tap();
    await expect(page.getByRole("dialog", { name: "Model" })).toBeVisible();
  });

  test("a tap outside the composer collapses it, a tap on the pill opens it again, and its chips work on the first tap", async () => {
    await expect(glass()).toHaveAttribute("data-shape", "card");
    await page.getByText("What should we work on?").tap();
    await expect(glass()).toHaveAttribute("data-shape", "pill");
    await glass().tap();
    await expect(glass()).toHaveAttribute("data-shape", "card");
    await page.locator("[data-model-controls] button[aria-label^='Model:']").tap();
    await expect(page.getByRole("dialog", { name: "Model" })).toBeVisible();
  });
});
