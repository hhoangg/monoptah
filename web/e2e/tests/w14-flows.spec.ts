// W-14 flows through the real UI: register, login, pair, then (with the headless desktop serving the paired data
// dir) confirm, read, send, stop and revoke. One account, one browser context and one virtual authenticator for all
// steps; the desktop side is `web_link_headless serve`, driven with its stdin commands (lib/r12.ts HeadlessDesktop).
import { join } from "node:path";
import { E2E_CATALOG } from "../lib/catalog.ts";
import { clearRateLimits, namedInvite } from "../lib/env.ts";
import { test, expect } from "../lib/fixtures.ts";
import { probePair } from "../lib/probes.ts";
import { HeadlessDesktop } from "../lib/r12.ts";
import { eventOf, startPairing, type PendingPairing } from "../lib/w14-headless.ts";
import { checkScreen, registerThroughUi } from "../lib/w14-ui.ts";

const HANDLE = "w14flows";
const MACHINE = "w14-headless";

test.describe("W-14 flows through the UI", () => {
  let pending: PendingPairing | null = null;
  let serving: HeadlessDesktop | null = null;

  // A fresh pair/start window: the suite pairs more headless desktops than the per-IP limit allows (lib/env.ts).
  test.beforeAll(clearRateLimits);

  test.afterAll(() => {
    serving?.kill();
    pending?.dispose();
  });

  // One account, one browser context and one virtual authenticator for all steps, so a step uses what the
  // previous one created (the passkey, then the session). Each step is reported separately by Playwright.
  test("register, login, pair, confirm, read, send, stop, revoke through the real pages", async ({ newActor }) => {
    test.setTimeout(480_000);
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

    const openDesktop = page.getByRole("link", { name: `Open ${MACHINE}` });

    const paired = await test.step("pair: /pair/<code> shows the desktop, Approve re-authenticates, the desktop is listed", async () => {
      const started = await startPairing(MACHINE);
      pending = started;
      const { userCode, confirmationCode, fingerprint } = started.started;
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

      expect(await started.process.exited, started.process.stderr()).toBe(0);
      expect(eventOf(started.process, "paired")).toMatchObject({ account: HANDLE, fingerprint });

      await page.getByRole("link", { name: "Go to my desktops" }).click();
      await expect(page).toHaveURL(/\/desktops$/);
      await expect(openDesktop).toBeVisible();
      return started;
    });

    // The desktop serves the paired data dir WITHOUT --confirm-pending: a device stays pending until `approve`.
    const desktop = new HeadlessDesktop(["serve", "--data-dir", join(paired.dataDir, "data")]);
    serving = desktop;
    let desktopId = "";
    let deviceId = "";
    let deviceName = "";

    await test.step("serve: the desktop comes online and the directory lists it", async () => {
      const online = await desktop.waitFor("online", {}, 60_000);
      desktopId = String(online["desktopId"]);
      const listed = await actor.call("GET", "/api/desktops");
      expect((listed.json["desktops"] as { id: string; name: string }[]).find((d) => d.id === desktopId)?.name).toBe(MACHINE);
      // Skips the first-run "Projects on this device" overlay (it needs selection calls, which are not part of this gate).
      await page.evaluate((id) => window.localStorage.setItem(`monoptah.pickerSeen.${id}`, "1"), desktopId);
    });

    await test.step("confirm: opening the desktop registers this device and shows the pending screen until the desktop approves", async () => {
      await page.goto("/desktops");
      await openDesktop.click();
      await expect(page).toHaveURL(new RegExp(`/d/${desktopId}$`));
      const pendingScreen = page.getByRole("alertdialog", { name: "Confirm this device on your desktop" });
      await expect(pendingScreen).toBeVisible({ timeout: 30_000 });
      await expect(pendingScreen.getByText("Desktop fingerprint")).toBeVisible();
      await expect(pendingScreen.getByText("This device's fingerprint")).toBeVisible();
      await expect(pendingScreen.getByText(`Waiting for confirmation on ${MACHINE}`, { exact: false })).toBeVisible();

      // The server mirror lists this device as pending.
      await expect
        .poll(async () => {
          const grants = (await actor.call("GET", `/api/desktops/${desktopId}/grants`)).json["grants"] as { deviceId: string; deviceName: string; state: string }[];
          const found = grants.find((g) => g.state === "pending");
          if (found) {
            deviceId = found.deviceId;
            deviceName = found.deviceName;
          }
          return found !== undefined;
        })
        .toBe(true);
      expect(desktop.matching("confirmed")).toEqual([]);

      // Approve from the desktop side with control. A device that has not reached the desktop yet is not pending there
      // (the command answers an error event), so the command is repeated until the desktop prints `confirmed`.
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
      expect(desktop.matching("confirmed", { deviceId })[0]).toMatchObject({ permission: "control" });
      await expect(pendingScreen).toHaveCount(0, { timeout: 30_000 });
    });

    await test.step("catalog: the project and the session of the desktop are listed", async () => {
      await expect(page.getByRole("heading", { level: 1, name: "Workspace" })).toBeVisible();
      await expect(page.getByText(E2E_CATALOG.projectName).first()).toBeVisible();
      await expect(page.getByRole("link", { name: E2E_CATALOG.sessionTitle }).first()).toBeVisible();
    });

    const message = page.getByRole("textbox", { name: "Message" });
    const send = page.getByRole("button", { name: "Send", exact: true });
    const stop = page.getByRole("button", { name: "Stop", exact: true });

    await test.step("read: opening the session shows the two seeded turns", async () => {
      await page.getByRole("link", { name: E2E_CATALOG.sessionTitle }).first().click();
      await expect(page).toHaveURL(new RegExp(`/d/${desktopId}/s/${E2E_CATALOG.sessionId}$`));
      await expect(page.getByRole("heading", { level: 1, name: E2E_CATALOG.sessionTitle })).toBeVisible();
      await expect(page.getByText("Please look at the E2E task.")).toBeVisible();
      await expect(page.getByText("Canned E2E answer.")).toBeVisible();
      expect(desktop.matching("request", { method: "conversation.read" }).length).toBeGreaterThan(0);
      // Control: the composer is enabled and there is no read-only chip.
      await expect(message).toBeEnabled();
      await expect(page.getByTestId("read-only-chip")).toHaveCount(0);
    });

    await test.step("send: the composer sends a message and the echo arrives over the watch", async () => {
      await expect(send).toBeDisabled();
      await message.fill("hello from the web");
      await send.click();
      await expect(page.getByText("hello from the web", { exact: true }).first()).toBeVisible();
      await expect(page.getByText("Echo: hello from the web", { exact: true })).toBeVisible({ timeout: 20_000 });
      expect(desktop.matching("request", { method: "conversation.send" }).length).toBe(1);
      await expect(message).toHaveValue("");
    });

    await test.step("stop: a [hold] message streams until Stop ends it", async () => {
      await message.fill("[hold] keep going");
      await send.click();
      await expect(stop).toBeVisible({ timeout: 20_000 });
      await expect(page.getByText("[hold] keep going", { exact: true }).first()).toBeVisible();
      // Longer than the 400 ms reply delay: a held message must not be answered.
      await page.waitForTimeout(1_500);
      await expect(page.getByText("Echo: [hold] keep going")).toHaveCount(0);
      await stop.click();
      await expect(stop).toHaveCount(0, { timeout: 20_000 });
      await expect(send).toBeVisible();
      expect(desktop.matching("request", { method: "session.cancel" }).length).toBeGreaterThan(0);
      await expect(page.getByText("Echo: [hold] keep going")).toHaveCount(0);
    });

    await test.step("visual and axe: the session screen at 390, 768, 1280 and 200 percent zoom", async () => {
      await checkScreen(page, {
        name: "session",
        path: `/d/${desktopId}/s/${E2E_CATALOG.sessionId}`,
        key: (p) => p.getByRole("heading", { level: 1, name: E2E_CATALOG.sessionTitle }),
        ready: (p) => p.getByText("Canned E2E answer."),
      });
    });

    await test.step("visual and axe: the desktop detail screen, with this device confirmed", async () => {
      await checkScreen(page, {
        name: "desktop-detail",
        path: `/desktops/${desktopId}`,
        key: (p) => p.getByRole("heading", { level: 1, name: MACHINE }),
        ready: (p) => p.getByRole("button", { name: `Revoke access for ${deviceName}` }),
      });
      await expect(page.getByText("This device", { exact: true })).toBeVisible();
      await expect(page.getByText("Confirmed", { exact: true })).toBeVisible();
      await expect(page.getByText("Permission: Control", { exact: false })).toBeVisible();
    });

    await test.step("revoke (a): Revoke access for this device, the desktop prints revoked, the link shows the revoked screen", async () => {
      await page.goto(`/desktops/${desktopId}`);
      await page.getByRole("button", { name: `Revoke access for ${deviceName}` }).click();
      const dialog = page.getByRole("dialog", { name: `Revoke access for ${deviceName}?` });
      await expect(dialog).toBeVisible();
      // The passkey re-auth of this fresh route is answered by the virtual authenticator.
      await dialog.getByRole("button", { name: "Revoke access", exact: true }).click();
      await expect(dialog).toHaveCount(0, { timeout: 20_000 });
      await expect(page.getByText("Revoked", { exact: true })).toBeVisible();
      await expect(page.getByRole("button", { name: `Revoke access for ${deviceName}` })).toHaveCount(0);
      const gone = await desktop.waitFor("revoked", { scope: "device", deviceId }, 30_000);
      expect(gone["deviceId"]).toBe(deviceId);
      const grants = (await actor.call("GET", `/api/desktops/${desktopId}/grants`)).json["grants"] as { deviceId: string; state: string }[];
      expect(grants.find((g) => g.deviceId === deviceId)?.state).toBe("revoked");

      await page.getByRole("link", { name: "Open desktop" }).click();
      const revoked = page.getByRole("alertdialog", { name: "Access to this desktop was revoked" });
      await expect(revoked).toBeVisible({ timeout: 30_000 });
      await expect(revoked.getByRole("link", { name: "Go to desktops" })).toBeVisible();
    });

    await test.step("revoke (b): Revoke desktop removes it from the account and the desktop prints revoked", async () => {
      await page.goto(`/desktops/${desktopId}`);
      await page.getByRole("button", { name: "Revoke desktop" }).click();
      const dialog = page.getByRole("dialog", { name: `Revoke ${MACHINE}?` });
      await expect(dialog).toBeVisible();
      await dialog.getByRole("button", { name: "Revoke desktop" }).click();
      await expect(page).toHaveURL(/\/desktops$/, { timeout: 20_000 });
      await expect(openDesktop).toHaveCount(0);
      await desktop.waitFor("revoked", { scope: "desktop" }, 60_000);
      const listed = await actor.call("GET", "/api/desktops");
      expect((listed.json["desktops"] as { id: string }[]).some((d) => d.id === desktopId)).toBe(false);
    });
  });
});
