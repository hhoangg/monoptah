// The navigation redesign of the /d routes: from 640 px an activity rail at the far left (the paired desktops as
// avatars on top; Cloud, Desktop settings and the account avatar at the bottom) replaces the top bar; below 640 px the
// top bar stays with the desktop switcher, Search and the account avatar. The avatar menu holds Settings, Desktops and
// Install app, so the monitor and download icons are gone from every top nav. Two real headless desktops are paired and
// confirmed for one account (serial).
//
// Set RAIL_SCREENSHOTS=<dir> and RAIL_SHOT_LABEL=before|after to save screenshots at the three layouts there. The
// "screenshots" test only waits for the shell, so it also runs against a build from before the redesign.
import { mkdirSync } from "node:fs";
import { join } from "node:path";
import type { Page } from "@playwright/test";
import type { Actor } from "../lib/actor.ts";
import { clearRateLimits, namedInvite } from "../lib/env.ts";
import { test, expect } from "../lib/fixtures.ts";
import { probePair } from "../lib/probes.ts";
import { HeadlessDesktop } from "../lib/r12.ts";
import { startPairing, type PendingPairing } from "../lib/w14-headless.ts";
import { registerThroughUi } from "../lib/w14-ui.ts";

const HANDLE = "arail";
const ALPHA = "rail-alpha";
const BRAVO = "rail-bravo";

const LAYOUTS = [
  { name: "desktop", width: 1280, height: 800 },
  { name: "tablet", width: 820, height: 1180 },
  { name: "mobile", width: 390, height: 844 },
] as const;

test.describe("activity rail", () => {
  test.describe.configure({ mode: "serial" });
  const probe = probePair();
  if (!probe.run) console.log(`skip reason: ${probe.reason}`);
  test.skip(!probe.run, probe.reason);

  const pendings: PendingPairing[] = [];
  const desktops: HeadlessDesktop[] = [];
  let page: Page;
  let actor: Actor;
  const ids: Record<string, string> = {};
  let watching = false;

  test.beforeAll(clearRateLimits);

  test.afterAll(() => {
    for (const desktop of desktops) desktop.kill();
    for (const pending of pendings) pending.dispose();
  });

  /** Pairs a headless desktop through the UI, serves it, and confirms this browser's device on it. */
  async function pairAndConfirm(machine: string): Promise<void> {
    const pending = await startPairing(machine);
    pendings.push(pending);
    const { userCode, fingerprint } = pending.started;
    await page.goto(`/pair/${userCode}#fp=${fingerprint}`);
    await page.getByRole("button", { name: "Approve" }).click();
    await expect(page.getByRole("heading", { name: "Desktop approved" })).toBeVisible();
    expect(await pending.process.exited, pending.process.stderr()).toBe(0);

    const desktop = new HeadlessDesktop(["serve", "--data-dir", join(pending.dataDir, "data")]);
    desktops.push(desktop);
    const online = await desktop.waitFor("online", {}, 60_000);
    const desktopId = String(online["desktopId"]);
    ids[machine] = desktopId;
    await page.evaluate((id) => window.localStorage.setItem(`monoptah.pickerSeen.${id}`, "1"), desktopId);

    await page.goto(`/d/${desktopId}`);
    const pendingScreen = page.getByRole("alertdialog", { name: "Confirm this device on your desktop" });
    await expect(pendingScreen).toBeVisible({ timeout: 30_000 });
    let deviceId = "";
    await expect
      .poll(async () => {
        const grants = (await actor.call("GET", `/api/desktops/${desktopId}/grants`)).json["grants"] as { deviceId: string; state: string }[];
        deviceId = grants.find((g) => g.state === "pending")?.deviceId ?? "";
        return deviceId !== "";
      })
      .toBe(true);
    await expect
      .poll(
        async () => {
          if (desktop.matching("confirmed", { deviceId }).length > 0) return true;
          desktop.command({ cmd: "approve", deviceId, permission: "control" });
          await new Promise((resolve) => setTimeout(resolve, 1_500));
          return desktop.matching("confirmed", { deviceId }).length > 0;
        },
        { timeout: 45_000, intervals: [100] },
      )
      .toBe(true);
    await expect(pendingScreen).toHaveCount(0, { timeout: 30_000 });
  }

  /**
   * Moves inside the loaded SPA (history push plus the popstate the router listens to) instead of `page.goto`. The relay
   * rate limits requests per address, and every full page load makes several API calls (the rail adds one grant read per
   * desktop), so a spec with a load per test ran into "Too many requests" on a slow CI runner. The setup test does the
   * only full loads.
   */
  async function navigate(path: string): Promise<void> {
    await page.evaluate((to) => {
      window.history.pushState({}, "", to);
      window.dispatchEvent(new PopStateEvent("popstate"));
    }, path);
  }

  async function open(desktop: string, layout: (typeof LAYOUTS)[number]): Promise<void> {
    if (!watching) {
      watching = true;
      page.on("pageerror", (err) => console.log("PAGEERROR", err.message, err.stack?.slice(0, 800)));
    }
    await page.setViewportSize({ width: layout.width, height: layout.height });
    await navigate(`/d/${ids[desktop]}`);
    await expect(page.getByTestId("app-shell")).toBeVisible({ timeout: 30_000 });
  }

  test("setup: register and pair two desktops", async ({ newSharedActor }) => {
    test.setTimeout(360_000);
    actor = await newSharedActor();
    page = actor.page;
    await page.setViewportSize({ width: 1280, height: 800 });
    await registerThroughUi(page, namedInvite("activityrail"), HANDLE, "Activity Rail");
    await pairAndConfirm(ALPHA);
    await pairAndConfirm(BRAVO);
  });

  const shotDir = process.env["RAIL_SCREENSHOTS"];
  const label = process.env["RAIL_SHOT_LABEL"] ?? "after";
  test("screenshots", async () => {
    test.skip(!shotDir, "set RAIL_SCREENSHOTS to save screenshots");
    mkdirSync(shotDir!, { recursive: true });
    // Resizes the loaded page: the rail follows the media query.
    await open(ALPHA, LAYOUTS[0]);
    for (const layout of LAYOUTS) {
      await page.setViewportSize({ width: layout.width, height: layout.height });
      await page.waitForTimeout(1_500);
      await page.screenshot({ path: join(shotDir!, `${label}-${layout.name}.png`) });
      const avatar = page.getByRole("button", { name: "Account" });
      if ((await avatar.count()) > 0) {
        await avatar.first().click();
        await expect(page.getByRole("menu")).toBeVisible();
        await page.waitForTimeout(400); // the menu fades in
        await page.screenshot({ path: join(shotDir!, `${label}-${layout.name}-account-menu.png`) });
        await page.keyboard.press("Escape");
      }
    }
  });

  for (const layout of LAYOUTS.filter((item) => item.name !== "mobile")) {
    test.describe(`${layout.name} ${layout.width}px`, () => {
      test("the rail lists both desktops with initials, status and the current one marked", async () => {
        await open(ALPHA, layout);
        const rail = page.getByTestId("activity-rail");
        await expect(rail).toBeVisible();
        const list = rail.getByRole("list", { name: "Desktops" });
        const alpha = list.getByRole("link", { name: /^rail-alpha/ });
        const bravo = list.getByRole("link", { name: /^rail-bravo/ });
        // The directory list refreshes every 15 s: a desktop that just connected may show online a little later.
        await expect(bravo).toHaveAttribute("aria-label", /^rail-bravo, online/, { timeout: 40_000 });
        await expect(alpha).toHaveAttribute("aria-label", /^rail-alpha, online, current desktop/);
        await expect(alpha).toHaveAttribute("aria-current", "page");
        await expect(bravo).not.toHaveAttribute("aria-current");
        await expect(alpha).toHaveText("RA");
        await expect(bravo).toHaveText("RB");
        await expect(alpha.locator("[data-status=online]")).toBeVisible();
        await expect(rail.getByTestId("rail-indicator")).toHaveCount(1);
        // The rail is the far left column.
        const box = await rail.boundingBox();
        expect(box?.x).toBe(0);
      });

      test("hovering an avatar shows the full name and the status", async () => {
        await open(ALPHA, layout);
        const bravo = page.getByTestId("activity-rail").getByRole("link", { name: /^rail-bravo/ });
        await expect(bravo).toHaveAttribute("aria-label", /^rail-bravo, online/, { timeout: 40_000 });
        // A pointer already resting there from an earlier test would open no tooltip: arrive from elsewhere.
        await page.mouse.move(700, 400);
        await bravo.hover();
        const tip = page.getByRole("tooltip");
        await expect(tip).toContainText("rail-bravo");
        await expect(tip).toContainText("Online");
      });

      test("clicking an avatar switches the desktop and the keyboard reaches it too", async () => {
        await open(ALPHA, layout);
        const rail = page.getByTestId("activity-rail");
        await rail.getByRole("link", { name: /^rail-bravo/ }).click();
        await expect(page).toHaveURL(new RegExp(`/d/${ids[BRAVO]}$`));
        await expect(rail.getByRole("link", { name: /^rail-bravo, online, current desktop/ })).toHaveAttribute("aria-current", "page");
        await expect(rail.getByRole("link", { name: /^rail-alpha/ })).not.toHaveAttribute("aria-current");
        await rail.getByRole("link", { name: /^rail-alpha/ }).focus();
        await page.keyboard.press("Enter");
        await expect(page).toHaveURL(new RegExp(`/d/${ids[ALPHA]}$`));
      });

      test("the avatar menu holds Settings, Desktops and Install app; the rail has no monitor or download icon", async () => {
        await open(ALPHA, layout);
        const rail = page.getByTestId("activity-rail");
        await expect(rail.locator('[data-icon="computer"], [data-icon="download"]')).toHaveCount(0);
        await expect(page.getByRole("navigation", { name: "Main" })).toHaveCount(0);
        await expect(page.getByRole("button", { name: /Switch desktop/ })).toHaveCount(0);
        await rail.getByRole("button", { name: "Account" }).click();
        const menu = page.getByRole("menu");
        await expect(menu.getByRole("menuitem", { name: "Settings" })).toBeVisible();
        await expect(menu.getByRole("menuitem", { name: "Install app" })).toHaveAttribute("href", "/settings/install");
        await menu.getByRole("menuitem", { name: "Desktops" }).click();
        await expect(page).toHaveURL(/\/settings\/desktops$/);
      });

      test("Desktop settings, then the avatar, sit at the bottom of the rail, and the sidebar has no duplicates", async () => {
        await open(ALPHA, layout);
        const rail = page.getByTestId("activity-rail");
        const labels = await rail.getByTestId("rail-actions").locator("a, button").evaluateAll((els) => els.map((el) => el.getAttribute("aria-label")));
        // Cloud comes first when the desktop has it; the last two are fixed.
        expect(labels.slice(-2)).toEqual(["Desktop settings", "Account"]);
        expect(labels.length === 2 || labels[0] === "Cloud").toBe(true);
        const sidebar = page.getByTestId("app-sidebar");
        for (const name of ["All desktops", "Desktop settings", "Cloud"]) await expect(sidebar.getByRole("link", { name })).toHaveCount(0);
        await expect(sidebar.getByRole("button", { name: "Search" })).toBeVisible();
        await rail.getByRole("link", { name: "Desktop settings" }).click();
        await expect(page).toHaveURL(new RegExp(`/settings/desktops/${ids[ALPHA]}$`));
      });
    });
  }

  test("mobile 390px: the top bar keeps the switcher, Search and the avatar, and no rail", async () => {
    const layout = LAYOUTS[2];
    await open(ALPHA, layout);
    await expect(page.getByTestId("activity-rail")).toHaveCount(0);
    await expect(page.getByRole("button", { name: /^Switch desktop, current: rail-alpha/ })).toBeVisible();
    const nav = page.getByRole("navigation", { name: "Main" });
    const labels = await nav.locator("a, button").evaluateAll((els) => els.map((el) => el.getAttribute("aria-label")));
    expect(labels).toEqual(["Search", "Account"]);
    await expect(nav.locator('[data-icon="computer"], [data-icon="download"]')).toHaveCount(0);
    await nav.getByRole("button", { name: "Account" }).click();
    const menu = page.getByRole("menu");
    await expect(menu.getByRole("menuitem", { name: "Install app" })).toHaveAttribute("href", "/settings/install");
    await menu.getByRole("menuitem", { name: "Desktops" }).click();
    await expect(page).toHaveURL(/\/settings\/desktops$/);
  });
});
