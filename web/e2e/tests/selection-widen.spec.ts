// Selection widening from the web: a project the device does not share is listed as "Not shared" (name only), and a
// Control device adds it after a fresh passkey confirmation (a single-use ticket the desktop verifies with the Worker).
// One account, one paired desktop (the real core code behind `web_link_headless serve`, which confirms this device with
// Control). The virtual authenticator answers the passkey re-auth prompt.
import type { Locator } from "@playwright/test";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { E2E_CATALOG } from "../lib/catalog.ts";
import { clearRateLimits, namedInvite } from "../lib/env.ts";
import { test, expect } from "../lib/fixtures.ts";
import { probePair } from "../lib/probes.ts";
import { HeadlessDesktop } from "../lib/r12.ts";
import { startPairing, type PendingPairing } from "../lib/w14-headless.ts";
import { registerThroughUi } from "../lib/w14-ui.ts";

const HANDLE = "selwiden";
const MACHINE = "selection-headless";

test.describe("selection widening from the web", () => {
  let pending: PendingPairing | null = null;
  let desktop: HeadlessDesktop | null = null;
  let dataRoot: string | null = null;

  test.beforeAll(clearRateLimits);
  test.afterAll(() => {
    desktop?.kill();
    pending?.dispose();
    if (dataRoot) rmSync(dataRoot, { recursive: true, force: true });
  });

  test("an unshared project shows as Not shared and a Control device adds it after the passkey re-auth", async ({ newActor }) => {
    test.setTimeout(240_000);
    dataRoot = mkdtempSync(join(tmpdir(), "monoptah-e2e-selection-"));
    const actor = await newActor();
    const page = actor.page;
    await registerThroughUi(page, namedInvite("selection"), HANDLE, "Selection Widen");

    const headless = probePair();
    if (!headless.run) console.log(`skip reason: ${headless.reason}`);
    test.skip(!headless.run, headless.reason);

    pending = await startPairing(MACHINE);
    const { userCode, fingerprint } = pending.started;
    await page.goto(`/pair/${userCode}#fp=${fingerprint}`);
    await page.getByRole("button", { name: "Approve" }).click();
    await expect(page.getByRole("heading", { name: "Desktop approved" })).toBeVisible();
    expect(await pending.process.exited, pending.process.stderr()).toBe(0);

    // `--confirm-pending control` is the harness switch: this browser becomes a Control device on its own.
    desktop = new HeadlessDesktop(["serve", "--data-dir", join(pending.dataDir, "data"), "--confirm-pending", "control"]);
    const online = await desktop.waitFor("online", {}, 60_000);
    const desktopId = String(online["desktopId"]);
    // Skips the first-run picker: the spec opens the picker from the settings itself.
    await page.evaluate((id) => window.localStorage.setItem(`monoptah.pickerSeen.${id}`, "1"), desktopId);

    const tickets: string[] = [];
    page.on("request", (request) => {
      if (request.method() === "POST" && /\/selection-ticket$/.test(new URL(request.url()).pathname)) tickets.push(request.url());
    });

    await page.goto(`/d/${desktopId}`);
    await expect(page.getByText(E2E_CATALOG.projectName).first()).toBeVisible({ timeout: 60_000 });

    const openPicker = async () => {
      await page.goto(`/settings/desktops/${desktopId}`);
      await page.getByRole("button", { name: /Choose projects/ }).click();
      const dialog = page.getByRole("dialog", { name: "Projects on this device" });
      await expect(dialog).toBeVisible();
      return dialog;
    };
    const project = (dialog: Locator) => dialog.getByRole("checkbox", { name: new RegExp(E2E_CATALOG.projectName) });

    await test.step("narrow: removing the project needs no passkey and no ticket", async () => {
      const dialog = await openPicker();
      await expect(project(dialog)).toBeChecked();
      await project(dialog).uncheck();
      await dialog.getByRole("button", { name: "Save", exact: true }).click();
      await expect(dialog).toHaveCount(0, { timeout: 30_000 });
      expect(tickets).toEqual([]);
    });

    await test.step("the project is still listed, as Not shared, without its id or path", async () => {
      const dialog = await openPicker();
      await expect(project(dialog)).not.toBeChecked();
      await expect(project(dialog)).toBeEnabled();
      await expect(dialog.getByText("Not shared", { exact: true })).toBeVisible();
      await expect(dialog).not.toContainText(E2E_CATALOG.projectId);
      await expect(dialog).not.toContainText("/e2e/project");
    });

    await test.step("widen: checking it asks for the passkey, then it is shared again and appears", async () => {
      const dialog = page.getByRole("dialog", { name: "Projects on this device" });
      await project(dialog).check();
      await dialog.getByRole("button", { name: "Confirm and save" }).click();
      // The virtual authenticator answers the re-auth prompt; the app then gets a ticket and sends the change.
      await expect(dialog).toHaveCount(0, { timeout: 30_000 });
      expect(tickets.length).toBeGreaterThanOrEqual(1);

      const again = await openPicker();
      await expect(project(again)).toBeChecked();
      await expect(again.getByText("Not shared", { exact: true })).toHaveCount(0);
      await page.goto(`/d/${desktopId}`);
      await expect(page.getByText(E2E_CATALOG.projectName).first()).toBeVisible({ timeout: 30_000 });
    });
  });
});
