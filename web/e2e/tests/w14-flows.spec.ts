// W-14 flows through the real UI: register, login, pair. The desktop-dependent flows (confirm, read, send, stop,
// revoke) need `web_link_headless serve` and are added in the second pass; they reuse lib/w14-headless.ts
// (`runHeadless(["serve", ...])`) and the account of this spec.
import { namedInvite } from "../lib/env.ts";
import { test, expect } from "../lib/fixtures.ts";
import { probePair } from "../lib/probes.ts";
import { eventOf, startPairing } from "../lib/w14-headless.ts";
import { registerThroughUi } from "../lib/w14-ui.ts";

const HANDLE = "w14flows";
const MACHINE = "w14-headless";

test.describe("W-14 flows through the UI", () => {
  // One account, one browser context and one virtual authenticator for the three steps, so a step uses what the
  // previous one created (the passkey, then the session). Each step is reported separately by Playwright.
  test("register, login, pair through the real pages", async ({ newActor }) => {
    const actor = await newActor();
    const page = actor.page;

    await test.step("register: invite, passkey in the virtual authenticator, recovery codes step", async () => {
      const codes = await registerThroughUi(page, namedInvite("w14"), HANDLE, "W14 Flows");
      expect(new Set(codes).size).toBe(10);
      expect((await actor.authenticator.credentials()).length).toBe(1);
      expect((await actor.sessionCookie())?.httpOnly).toBe(true);
      const me = await actor.call("GET", "/api/me");
      expect(me.json["user"]).toMatchObject({ handle: HANDLE, role: "admin" });
      expect(me.json["recoveryAckAt"], "the recovery codes step acknowledges the codes").not.toBeNull();
    });

    await test.step("login: /admin sends a signed-out visitor to /login, the passkey signs in and returns", async () => {
      await actor.context.clearCookies();
      await page.goto("/admin");
      await expect(page).toHaveURL(/\/login\?redirect=%2Fadmin/);
      await page.getByRole("button", { name: "Sign in with passkey" }).click();
      await expect(page).toHaveURL(/\/admin$/);
      await expect(page.getByRole("heading", { name: "Admin" })).toBeVisible();
      expect((await actor.call("GET", "/api/me")).status).toBe(200);
    });

    const headless = probePair();
    if (!headless.run) console.log(`skip reason: ${headless.reason}`);
    test.skip(!headless.run, headless.reason);

    await test.step("pair: /pair/<code> shows the desktop, Approve re-authenticates, the desktop is listed", async () => {
      const pending = await startPairing(MACHINE);
      try {
        const { userCode, confirmationCode, fingerprint } = pending.started;
        await page.goto(`/pair/${userCode}#fp=${fingerprint}`);
        await expect(page.getByRole("heading", { name: "Approve this desktop?" })).toBeVisible();
        await expect(page.getByText("The fingerprint matches your link")).toBeVisible();
        const details = page.getByRole("definition");
        await expect(details.filter({ hasText: confirmationCode })).toBeVisible();
        await expect(details.filter({ hasText: MACHINE })).toBeVisible();
        // The page prints the first 32 hex characters of the fingerprint as 8 groups of 4, two lines of 4.
        const hex = fingerprint.replace(/[^0-9a-f]/gi, "").toLowerCase();
        const groups = hex.slice(0, 32).match(/.{4}/g) ?? [];
        await expect(details.filter({ hasText: groups.slice(0, 4).join(" ") })).toContainText(groups.slice(4, 8).join(" "));

        // The reauth passkey prompt is answered by the virtual authenticator (automatic presence).
        await page.getByRole("button", { name: "Approve" }).click();
        await expect(page.getByRole("heading", { name: "Desktop approved" })).toBeVisible();

        expect(await pending.process.exited, pending.process.stderr()).toBe(0);
        expect(eventOf(pending.process, "paired")).toMatchObject({ account: HANDLE, fingerprint });

        await page.getByRole("link", { name: "Go to my desktops" }).click();
        await expect(page).toHaveURL(/\/desktops$/);
        await expect(page.getByRole("link", { name: `Open ${MACHINE}` })).toBeVisible();
      } finally {
        pending.dispose();
      }
    });
  });
});
