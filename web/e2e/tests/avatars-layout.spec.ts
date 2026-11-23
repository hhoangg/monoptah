// The layout switch (Classic or Focused, per browser), the desktop avatar (an emoji or an uploaded picture, saved on the
// account and shown in the activity rail) and the Mono avatar (the pixel mascot and color the desktop sends in the
// catalog). One account and one paired headless desktop that publishes a Mono (WEB_LINK_HEADLESS_MONO=1), serial.
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { Page } from "@playwright/test";
import { E2E_CATALOG, E2E_MONO } from "../lib/catalog.ts";
import { clearRateLimits, namedInvite } from "../lib/env.ts";
import { test, expect } from "../lib/fixtures.ts";
import { probePair } from "../lib/probes.ts";
import { HeadlessDesktop } from "../lib/r12.ts";
import { startPairing, type PendingPairing } from "../lib/w14-headless.ts";
import { registerThroughUi } from "../lib/w14-ui.ts";

const HANDLE = "avalayout";
const MACHINE = "avatar-headless";
const ROCKET = "\u{1F680}";
const LAYOUT_KEY = "monoptah.layout.mode";

// A 2x2 opaque PNG: the browser must decode it, crop it and re-encode it to 128 px before anything is uploaded.
const PNG_2X2 = Buffer.from("iVBORw0KGgoAAAANSUhEUgAAAAIAAAACCAIAAAD91JpzAAAAEElEQVR4nGP4zwAE/xkgFAAb8gP91pbyKwAAAABJRU5ErkJggg==", "base64");
const SVG = Buffer.from('<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"><script>window.__pwned = 1</script></svg>');

test.describe("layout switch, desktop avatar and Mono avatar", () => {
  test.describe.configure({ mode: "serial" });
  const probe = probePair();
  if (!probe.run) console.log(`skip reason: ${probe.reason}`);
  test.skip(!probe.run, probe.reason);

  let dataRoot = "";
  let pending: PendingPairing | undefined;
  let desktop: HeadlessDesktop | undefined;
  let desktopId = "";
  let page: Page;

  test.beforeAll(clearRateLimits);
  test.afterAll(() => {
    desktop?.kill();
    pending?.dispose();
    if (dataRoot) rmSync(dataRoot, { recursive: true, force: true });
  });

  test("setup: register, pair a desktop that publishes a Mono, confirm this browser", async ({ newSharedActor }) => {
    test.setTimeout(240_000);
    dataRoot = mkdtempSync(join(tmpdir(), "monoptah-e2e-avatars-"));
    const actor = await newSharedActor();
    page = actor.page;
    await page.setViewportSize({ width: 1280, height: 800 });
    await registerThroughUi(page, namedInvite("avatarslayout"), HANDLE, "Avatars Layout");

    pending = await startPairing(MACHINE);
    const { userCode, fingerprint } = pending.started;
    await page.goto(`/pair/${userCode}#fp=${fingerprint}`);
    await page.getByRole("button", { name: "Approve" }).click();
    await expect(page.getByRole("heading", { name: "Desktop approved" })).toBeVisible();
    expect(await pending.process.exited, pending.process.stderr()).toBe(0);

    process.env["WEB_LINK_HEADLESS_MONO"] = "1";
    try {
      desktop = new HeadlessDesktop(["serve", "--data-dir", join(pending.dataDir, "data")]);
    } finally {
      delete process.env["WEB_LINK_HEADLESS_MONO"];
    }
    const online = await desktop.waitFor("online", {}, 60_000);
    desktopId = String(online["desktopId"]);
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
    const served = desktop;
    await expect
      .poll(
        async () => {
          if (served.matching("confirmed", { deviceId }).length > 0) return true;
          served.command({ cmd: "approve", deviceId, permission: "control" });
          await new Promise((resolve) => setTimeout(resolve, 1_500));
          return served.matching("confirmed", { deviceId }).length > 0;
        },
        { timeout: 45_000, intervals: [100] },
      )
      .toBe(true);
    await expect(pendingScreen).toHaveCount(0, { timeout: 30_000 });
  });

  test("layout: Classic by default; Focused leaves out the session list and survives a reload", async () => {
    await page.setViewportSize({ width: 1280, height: 800 });
    await page.goto(`/d/${desktopId}/p/${E2E_CATALOG.projectId}`);
    await expect(page.getByTestId("session-list-pane")).toBeVisible({ timeout: 30_000 });

    await page.goto("/settings/appearance");
    const group = page.getByRole("radiogroup", { name: "Layout" });
    await expect(group.getByRole("radio", { name: "Classic" })).toBeChecked();
    await group.getByRole("radio", { name: "Focused" }).click();
    expect(await page.evaluate((key) => window.localStorage.getItem(key), LAYOUT_KEY)).toBe("focused");

    await page.goto(`/d/${desktopId}/p/${E2E_CATALOG.projectId}`);
    await expect(page.getByTestId("app-shell")).toBeVisible({ timeout: 30_000 });
    await expect(page.getByTestId("workspace-frame")).toBeVisible();
    await expect(page.getByTestId("session-list-pane")).toHaveCount(0);

    await page.goto("/settings/appearance");
    await expect(page.getByRole("radiogroup", { name: "Layout" }).getByRole("radio", { name: "Focused" })).toBeChecked();
    await page.getByRole("radiogroup", { name: "Layout" }).getByRole("radio", { name: "Classic" }).click();
    await page.goto(`/d/${desktopId}/p/${E2E_CATALOG.projectId}`);
    await expect(page.getByTestId("session-list-pane")).toBeVisible({ timeout: 30_000 });
  });

  test("layout: the picker is disabled on a tablet and absent on a phone", async () => {
    await page.goto("/settings/appearance");
    await page.setViewportSize({ width: 820, height: 1180 });
    await expect(page.getByRole("radiogroup", { name: "Layout" }).getByRole("radio", { name: "Focused" })).toBeDisabled();
    await page.setViewportSize({ width: 390, height: 844 });
    await expect(page.getByRole("radiogroup", { name: "Layout" })).toHaveCount(0);
    await expect(page.getByRole("radiogroup", { name: "Colour scheme" })).toBeVisible();
  });

  test("desktop avatar: an emoji shows in the rail, on reload, and goes back to initials", async () => {
    await page.setViewportSize({ width: 1280, height: 800 });
    await page.goto(`/settings/desktops/${desktopId}`);
    await page.getByRole("radiogroup", { name: "Avatar emoji" }).getByRole("radio", { name: `Emoji ${ROCKET}` }).click({ force: true });
    await expect(page.getByTestId("avatar-preview").getByTestId("avatar-emoji")).toHaveText(ROCKET);

    await page.goto(`/d/${desktopId}`);
    const link = page.getByTestId("activity-rail").getByRole("link", { name: new RegExp(`^${MACHINE}`) });
    await expect(link.getByTestId("avatar-emoji")).toHaveText(ROCKET, { timeout: 30_000 });

    await page.goto(`/settings/desktops/${desktopId}`);
    await page.getByRole("button", { name: "Use initials" }).click();
    await page.goto(`/d/${desktopId}`);
    await expect(page.getByTestId("activity-rail").getByRole("link", { name: new RegExp(`^${MACHINE}`) })).toHaveText("AH", { timeout: 30_000 });
  });

  test("desktop avatar: an uploaded picture is re-encoded, served by the Worker and rendered in an <img>; an SVG is refused", async () => {
    await page.goto(`/settings/desktops/${desktopId}`);
    const upload = page.getByLabel("Avatar image file");
    const sent: string[] = [];
    page.on("request", (request) => {
      if (request.method() === "PATCH" && request.url().includes(`/api/desktops/${desktopId}`)) sent.push(request.postData() ?? "");
    });
    await upload.setInputFiles({ name: "me.png", mimeType: "image/png", buffer: PNG_2X2 });
    await expect(page.getByTestId("avatar-preview").getByTestId("avatar-image")).toBeVisible({ timeout: 15_000 });
    expect(sent).toHaveLength(1);
    const body = JSON.parse(sent[0] ?? "{}") as { avatar?: { kind: string; mime: string; data: string } };
    expect(body.avatar?.kind).toBe("image");
    expect(["image/webp", "image/jpeg", "image/png"]).toContain(body.avatar?.mime);
    // Re-encoded at 128 px by the browser, not the 2x2 original, and within the cap.
    expect(Buffer.from(body.avatar?.data ?? "", "base64").length).toBeLessThanOrEqual(20 * 1024);
    expect(Buffer.from(body.avatar?.data ?? "", "base64").equals(PNG_2X2)).toBe(false);

    await page.goto(`/d/${desktopId}`);
    const image = page.getByTestId("activity-rail").getByRole("link", { name: new RegExp(`^${MACHINE}`) }).getByTestId("avatar-image");
    await expect(image).toHaveAttribute("src", new RegExp(`^/api/desktops/${desktopId}/avatar\\?v=[0-9a-f]{16}$`), { timeout: 30_000 });
    await expect.poll(() => image.evaluate((element: HTMLImageElement) => element.complete && element.naturalWidth > 0)).toBe(true);
    expect(await image.evaluate((element: HTMLImageElement) => element.naturalWidth)).toBe(128);

    await page.goto(`/settings/desktops/${desktopId}`);
    await upload.setInputFiles({ name: "evil.svg", mimeType: "image/svg+xml", buffer: SVG });
    await expect(page.getByText("Use a PNG, JPEG or WebP image.")).toBeVisible();
    expect(await page.evaluate(() => (window as unknown as { __pwned?: number }).__pwned)).toBeUndefined();
    await page.getByRole("button", { name: "Use initials" }).click();
  });

  test("Mono avatar: the list, the chat header and the signature draw the mascot and color the desktop sent", async () => {
    // Every test above reloads full pages in one session (the desktop settings page polls its list, detail, grants and
    // push prefs), so the session reaches the Worker's 120 calls per minute just before this test: start a new window.
    test.setTimeout(180_000);
    await clearRateLimits();
    await page.setViewportSize({ width: 1280, height: 800 });
    await page.goto(`/d/${desktopId}`);
    const sidebar = page.getByTestId("app-sidebar");
    const row = sidebar.getByRole("link", { name: new RegExp(E2E_MONO.name) });
    const avatar = row.getByTestId("mono-avatar");
    await expect(avatar).toHaveAttribute("data-kind", "mascot", { timeout: 30_000 });
    await expect(avatar.locator("svg")).toHaveAttribute("data-mascot", E2E_MONO.mascot);
    await expect(avatar.locator("svg path")).toHaveAttribute("fill", E2E_MONO.color);
    await expect(avatar).not.toContainText(E2E_MONO.name.charAt(0));

    await row.click();
    await expect(page).toHaveURL(new RegExp(`/monos/${E2E_MONO.id}$`));
    await expect(page.locator(`svg[data-mascot="${E2E_MONO.mascot}"]`).first()).toBeVisible();
  });
});
