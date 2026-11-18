// W-14 visual and accessibility checks of the main screens.
//
// Per screen, in the light and the dark theme: screenshots at 390, 768 and 1280 CSS px and at 1280 with 200 percent
// zoom (emulated, see VIEWPORTS in lib/w14-ui.ts), no horizontal overflow, the key control visible, and axe
// (wcag2a, wcag2aa) with zero violations of impact serious or critical. Screenshots go to <output>/w14-visual and are
// reviewed by eye; there are no committed pixel baselines. Keyboard checks follow in the last tests.
import type { Page } from "@playwright/test";
import { namedInvite } from "../lib/env.ts";
import { test, expect } from "../lib/fixtures.ts";
import { probePair } from "../lib/probes.ts";
import { startPairing, type PendingPairing } from "../lib/w14-headless.ts";
import { checkScreen, focusIsVisible, registerThroughUi, type Screen } from "../lib/w14-ui.ts";

// Visits `path`, tabs through the page and returns the accessible names of what got focus, in order.
async function tabThrough(page: Page, path: string, primary: RegExp): Promise<void> {
  await page.goto(path);
  await page.waitForLoadState("networkidle");
  const seen = new Set<string>();
  let reached = false;
  for (let i = 0; i < 40; i++) {
    await page.keyboard.press("Tab");
    const info = await page.evaluate(() => {
      const el = document.activeElement;
      if (!el || el === document.body) return null;
      const name = el.getAttribute("aria-label") ?? el.textContent?.trim() ?? "";
      return { id: `${el.tagName}:${name}`.slice(0, 80), name };
    });
    if (info === null) continue;
    expect(await focusIsVisible(page), `${path}: focus indicator on ${info.id}`).toBe(true);
    if (primary.test(info.name)) reached = true;
    if (seen.has(info.id)) break; // the cycle closed: nothing trapped focus before it came round
    seen.add(info.id);
  }
  expect(reached, `${path}: Tab reaches ${primary}`).toBe(true);
}


const SIGNED_OUT: Screen[] = [
  { name: "register", path: "/register", key: (p) => p.getByRole("button", { name: /create passkey/i }) },
  { name: "login", path: "/login", key: (p) => p.getByRole("button", { name: "Sign in with passkey" }) },
  { name: "recover", path: "/recover", key: (p) => p.getByRole("button", { name: "Recover account" }) },
];

const SIGNED_IN: Screen[] = [
  { name: "desktops", path: "/desktops", key: (p) => p.getByRole("heading", { name: "Desktops" }) },
  { name: "settings", path: "/settings", key: (p) => p.getByRole("button", { name: "Sign out" }) },
  { name: "admin", path: "/admin", key: (p) => p.getByRole("switch", { name: "Pause invites" }) },
];

test.describe.serial("W-14 screens: visual, axe and keyboard", () => {
  let pending: PendingPairing | null = null;

  test.afterAll(() => pending?.dispose());

  test("signed-out screens: register, login, recover", async ({ newActor }) => {
    const actor = await newActor();
    await actor.page.goto("/");
    for (const screen of SIGNED_OUT) await checkScreen(actor.page, screen);
    await test.step("keyboard: Tab reaches the primary action with a visible focus", async () => {
      await tabThrough(actor.page, "/register", /create passkey/i);
      await tabThrough(actor.page, "/login", /sign in with passkey/i);
      await tabThrough(actor.page, "/recover", /recover account/i);
    });
  });

  test("signed-in screens: desktops (with a paired desktop), pair page, settings, admin", async ({ newActor }) => {
    const actor = await newActor();
    await registerThroughUi(actor.page, namedInvite("w14b"), "w14screens", "W14 Screens");

    const headless = probePair();
    if (!headless.run) console.log(`skip reason: pair page and a listed desktop: ${headless.reason}`);
    const screens: Screen[] = [...SIGNED_IN];
    if (headless.run) {
      // One desktop is paired (approved through the API) so the list has a row; a second request stays pending for /pair.
      const first = await startPairing("w14-desktop");
      try {
        expect((await actor.reauth()).status).toBe(200);
        expect((await actor.call("POST", `/api/pair/${first.started.userCode}/approve`, { body: {} })).status).toBe(200);
        expect(await first.process.exited, first.process.stderr()).toBe(0);
      } finally {
        first.dispose();
      }
      pending = await startPairing("w14-pending-desktop");
      const { userCode, fingerprint } = pending.started;
      screens.push({ name: "pair", path: `/pair/${userCode}#fp=${fingerprint}`, key: (p) => p.getByRole("button", { name: "Approve" }) });
      const desktops = screens.find((s) => s.name === "desktops");
      if (desktops) desktops.key = (p) => p.getByRole("link", { name: "Open w14-desktop" });
    }
    for (const screen of screens) await checkScreen(actor.page, screen);

    await test.step("keyboard: settings and the pair page, Escape closes the confirm dialog", async () => {
      const page = actor.page;
      await tabThrough(page, "/settings", /sign out/i);

      await page.goto("/settings");
      await page.getByRole("button", { name: "Generate new codes" }).focus();
      await page.keyboard.press("Enter");
      const dialog = page.getByRole("dialog", { name: "Replace your recovery codes?" });
      await expect(dialog).toBeVisible();
      // A modal keeps focus inside (that trap is wanted); Escape is the way out.
      for (let i = 0; i < 6; i++) {
        await page.keyboard.press("Tab");
        expect(await page.evaluate(() => document.activeElement?.closest("[role=dialog]") !== null)).toBe(true);
      }
      await page.keyboard.press("Escape");
      await expect(dialog).toHaveCount(0);
      await expect(page.getByRole("button", { name: "Generate new codes" })).toBeFocused();

      if (pending !== null) {
        const { userCode, fingerprint } = pending.started;
        await tabThrough(page, `/pair/${userCode}#fp=${fingerprint}`, /^approve$/i);
      } else {
        console.log("skip reason: keyboard on /pair needs the headless binary");
      }
    });
  });
});

