// The `/` menu of a session composer (features.md N-27) in a real browser, on the real Worker and relay, against the
// TypeScript mock desktop of web/mock-desktop (it plays the desktop: the command list, the modes and the capability
// rules). One account, one paired mock desktop, one open page for all tests (serial).
//
// What it checks: no menu when the desktop does not advertise `slashCommands`, the menu opening on `/` in the
// session composer with the desktop's commands and skills, filtering while typing, choosing Plan mode with the
// keyboard (the chip shows and the desktop's session is in plan mode), turning it off from the chip, a skill ending up
// in the message, and the phone layout showing a bottom sheet that a tap chooses from.
import type { Page } from "@playwright/test";
import { KNOWN_CAPABILITIES } from "@monoptah/protocol";
import { MockDesktop } from "../../mock-desktop/src/desktop.ts";
import { fingerprint, generateKeyPair, type KeyPair } from "../../mock-desktop/src/noise.ts";
import type { FrameSocket } from "../../mock-desktop/src/transport.ts";
import { clearRateLimits, namedInvite, origin } from "../lib/env.ts";
import { test, expect } from "../lib/fixtures.ts";
import { TsDesktopPairing, connectDesktop } from "../lib/r12.ts";
import { testIp } from "../lib/pure.ts";
import { registerThroughUi } from "../lib/w14-ui.ts";

const HANDLE = "slashcmd";
const MACHINE = "slash-mock";
const SESSION = "sess-local-1";
const WITH = KNOWN_CAPABILITIES.filter((name) => name !== "terminal");
const WITHOUT = WITH.filter((name) => name !== "slashCommands");

test.describe("the / menu of a session composer", () => {
  test.describe.configure({ mode: "serial" });

  let page: Page;
  let desktop: MockDesktop;
  let socket: FrameSocket | null = null;
  let keepalive: ReturnType<typeof setInterval> | null = null;
  let desktopId = "";
  let desktopKey: KeyPair;

  test.beforeAll(clearRateLimits);

  test.afterAll(() => {
    if (keepalive) clearInterval(keepalive);
    desktop?.close();
    socket?.close();
  });

  const field = () => page.getByRole("textbox", { name: "Message" });
  const menu = () => page.getByTestId("slash-menu");
  const ids = () => menu().locator('[role="option"]').evaluateAll((nodes) => nodes.map((node) => node.getAttribute("data-command")));

  test("setup: register, pair a mock desktop and confirm the browser as a control device", async ({ newSharedActor }) => {
    test.setTimeout(240_000);
    const actor = await newSharedActor();
    page = actor.page;
    await registerThroughUi(page, namedInvite("slashcmd"), HANDLE, "Slash");

    desktopKey = generateKeyPair();
    const pairing = new TsDesktopPairing(origin(), testIp(206));
    const userCode = await pairing.start(MACHINE, desktopKey.publicKey);
    await page.goto(`/pair/${userCode}#fp=${fingerprint(desktopKey.publicKey)}`);
    await page.getByRole("button", { name: "Approve" }).click();
    await expect(page.getByRole("heading", { name: "Desktop approved" })).toBeVisible();

    let token = "";
    await expect
      .poll(
        async () => {
          const polled = await pairing.poll();
          if (polled.json["status"] === "approved") {
            token = String(polled.json["desktopToken"]);
            desktopId = String(polled.json["desktopId"]);
          }
          return token !== "";
        },
        { timeout: 60_000, intervals: [3_500] },
      )
      .toBe(true);

    desktop = new MockDesktop({ desktopId, staticKey: desktopKey, name: MACHINE, capabilities: WITH });
    const connected = await connectDesktop(origin(), desktopId, token);
    socket = connected.socket;
    desktop.attach(socket);
    keepalive = setInterval(() => socket?.sendText("ping"), 25_000);
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
    await expect.poll(() => [...desktop.devices.values()].some((device) => device.status === "pending")).toBe(true);
    const deviceFingerprint = [...desktop.devices.values()].find((device) => device.status === "pending")?.fingerprint ?? "";
    desktop.confirmDevice(deviceFingerprint, "control");
    const grant = await fetch(`${origin()}/api/desktop/grants/${deviceId}/state`, {
      method: "POST",
      headers: { "Content-Type": "application/json", Authorization: `Bearer ${token}` },
      body: JSON.stringify({ state: "confirmed", permission: "control" }),
    });
    expect(grant.status).toBe(200);
    await expect(pendingScreen).toHaveCount(0, { timeout: 30_000 });
  });

  test("an older desktop without slashCommands shows no menu; one that advertises it does", async () => {
    await page.setViewportSize({ width: 1280, height: 800 });
    desktop.setCapabilities(WITHOUT);
    await page.goto(`/d/${desktopId}/s/${SESSION}`);
    await field().fill("/");
    await expect(menu()).toHaveCount(0);
    await field().fill("");
    desktop.setCapabilities(WITH);
    await field().fill("/");
    await expect(menu()).toBeVisible();
    await field().fill("");
    await expect(menu()).toHaveCount(0);
  });

  test("/ lists the desktop's commands and skills; typing filters; Enter chooses Plan mode on the desktop; the chip turns it off", async () => {
    await page.goto(`/d/${desktopId}/s/${SESSION}`);
    await field().click();
    await page.keyboard.type("/");
    await expect(menu()).toHaveAttribute("data-layout", "popover");
    await expect.poll(ids).toEqual(["draft", "orchestrator", "operator", "plan", "btw", "compact", "add-to-folder", "lint", "review"]);
    await page.keyboard.type("pla");
    await expect.poll(ids).toEqual(["plan"]);
    await page.keyboard.press("Enter");
    await expect(menu()).toHaveCount(0);
    await expect(page.getByTestId("slash-chip-plan")).toBeVisible();
    await expect(field()).toHaveValue("");
    await expect.poll(() => desktop.world.slashState(SESSION).mode).toBe("plan");
    await page.getByTestId("slash-chip-plan").click();
    await expect(page.getByTestId("slash-chip-plan")).toHaveCount(0);
    await expect.poll(() => desktop.world.slashState(SESSION).mode).toBeNull();
  });

  test("arrow keys and Escape drive the menu, and a skill lands in the message that is sent", async () => {
    await page.goto(`/d/${desktopId}/s/${SESSION}`);
    await field().click();
    await page.keyboard.type("/");
    await expect(menu()).toBeVisible();
    await page.keyboard.press("ArrowDown");
    await expect(menu().locator('[aria-selected="true"]')).toHaveAttribute("data-command", "orchestrator");
    await page.keyboard.press("Escape");
    await expect(menu()).toHaveCount(0);
    await expect(field()).toHaveValue("/");
    await page.keyboard.type("rev");
    await page.keyboard.press("Tab");
    await expect(field()).toHaveValue("/review ");
    await page.keyboard.type("the parser");
    await page.keyboard.press("Enter");
    await expect(page.getByTestId("transcript-scroller").getByText("/review the parser")).toBeVisible();
  });

  test("on a phone the menu is a bottom sheet and a press chooses a row", async () => {
    await page.setViewportSize({ width: 390, height: 780 });
    await page.goto(`/d/${desktopId}/s/${SESSION}`);
    await field().click();
    await page.keyboard.type("/");
    await expect(menu()).toHaveAttribute("data-layout", "sheet");
    const box = await menu().boundingBox();
    expect(box?.width ?? 0).toBeGreaterThan(300);
    await menu().locator('[data-command="operator"]').click();
    await expect(page.getByTestId("slash-chip-operator")).toBeVisible();
    await expect.poll(() => desktop.world.slashState(SESSION).mode).toBe("operator");
    await page.getByTestId("slash-chip-operator").click();
    await expect.poll(() => desktop.world.slashState(SESSION).mode).toBeNull();
  });
});
