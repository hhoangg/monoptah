// The web terminal (features.md N-26) in a real browser, on the real Worker and relay, against the TypeScript mock desktop
// of web/mock-desktop (it plays the desktop: a fake shell, kill reasons and the capability rules). The desktop spends the
// one-time passkey tickets for real: the mock's `terminals.verifyTicket` hook POSTs the Worker's
// /api/desktop/grants/:deviceId/terminal-ticket with the desktop grants token, so single use and the project binding are
// the Worker's. One account, one paired mock desktop, one open page for all tests (serial). The browser's passkey prompt is
// answered by the virtual authenticator of the actor.
//
// What it checks: the capability gating of the footer chip, a terminal opened as a tab from the chip, the project chooser,
// the passkey step, typing and output in xterm, a long burst, a used ticket refused, the desktop killing the shell, a reload
// that reattaches, closing the tab (with confirm) killing the shell, a terminal split next to a chat tab, the phone drawer
// (open, key row, fullscreen and shrink, close), and that terminal output never becomes markup and never trips the CSP.
// With E2E_SHOTS_DIR set it also writes screenshots of the real UI there (nothing is written by default).
import type { Locator, Page } from "@playwright/test";
import { KNOWN_CAPABILITIES } from "@monoptah/protocol";
import { mkdirSync } from "node:fs";
import { join } from "node:path";
import { MockDesktop } from "../../mock-desktop/src/desktop.ts";
import { fingerprint, generateKeyPair, type KeyPair } from "../../mock-desktop/src/noise.ts";
import type { FrameSocket } from "../../mock-desktop/src/transport.ts";
import { clearRateLimits, namedInvite, origin } from "../lib/env.ts";
import { test, expect } from "../lib/fixtures.ts";
import { TsDesktopPairing, connectDesktop } from "../lib/r12.ts";
import { testIp } from "../lib/pure.ts";
import { registerThroughUi } from "../lib/w14-ui.ts";

const HANDLE = "terminal";
const MACHINE = "terminal-mock";
const PROJECT = "Example Alpha";
const WITH_TERMINAL = [...KNOWN_CAPABILITIES];
const WITHOUT_TERMINAL = KNOWN_CAPABILITIES.filter((name) => name !== "terminal");
const SHOTS = process.env["E2E_SHOTS_DIR"] ?? "";

declare global {
  interface Window {
    __csp?: string[];
    __xss?: number;
  }
}

test.describe("web terminal", () => {
  test.describe.configure({ mode: "serial" });

  let page: Page;
  let desktop: MockDesktop;
  let socket: FrameSocket | null = null;
  let keepalive: ReturnType<typeof setInterval> | null = null;
  let desktopId = "";
  let desktopKey: KeyPair;
  let deviceFingerprint = "";
  /** The tickets the browser got from the Worker, in order (the desktop spends them below). */
  const tickets: string[] = [];
  /** What the desktop's verifier saw: the Worker's answer for each ticket it was asked about. */
  const verdicts: { ticket: string; projectId: string; ok: boolean }[] = [];

  /** How the desktop spends a ticket at the Worker (set by the setup test), also called by the replay test. */
  let spendTicket: (ticket: string, projectId: string) => Promise<boolean> = () => Promise.reject(new Error("setup did not run"));

  test.beforeAll(clearRateLimits);

  test.afterAll(() => {
    if (keepalive) clearInterval(keepalive);
    desktop?.close();
    socket?.close();
  });

  const rows = () => page.locator(".xterm-rows").first();
  const chip = () => page.getByTestId("terminal-chip");
  const terminalTabs = () => page.getByRole("tab", { name: /^Terminal - / });

  async function shot(name: string): Promise<void> {
    if (!SHOTS) return;
    mkdirSync(SHOTS, { recursive: true });
    // Let the layout and the xterm cursor settle.
    await page.waitForTimeout(400);
    await page.screenshot({ path: join(SHOTS, `${name}.png`) });
  }

  /** Opens a terminal the way a user does from the footer chip, chooses the project, and waits for the prompt. */
  async function openShell(project = PROJECT): Promise<void> {
    const before = desktop.terminals.liveIds().length;
    await chip().click();
    await page.getByTestId("terminal-project").selectOption({ label: project });
    await page.getByTestId("terminal-open").click();
    await expect.poll(() => desktop.terminals.liveIds().length, { timeout: 30_000 }).toBe(before + 1);
    await expect(rows()).toContainText(`Monoptah mock shell in ${project}`, { timeout: 30_000 });
  }

  async function type(text: string): Promise<void> {
    await page.getByTestId("terminal-screen").last().click();
    await page.keyboard.type(text);
    await page.keyboard.press("Enter");
  }

  /** Closes the terminal tab that is shown, answering the confirm. */
  async function closeTabWithConfirm(): Promise<void> {
    await page.locator('[data-testid="tab-close"][data-tab-key^="t:"]').click({ force: true });
    const dialog = page.getByRole("dialog");
    await expect(dialog).toContainText("This ends the shell on your desktop");
    await dialog.getByRole("button", { name: "Close terminal" }).click();
  }

  test("setup: register, pair a mock desktop whose tickets the Worker verifies, and confirm the browser as a control device", async ({ newSharedActor }) => {
    test.setTimeout(240_000);
    const actor = await newSharedActor();
    page = actor.page;
    // The approved mockup is dark: screenshots are taken in the dark scheme.
    if (SHOTS) await page.emulateMedia({ colorScheme: "dark" });
    await page.addInitScript(() => {
      window.__csp = [];
      document.addEventListener("securitypolicyviolation", (event) => window.__csp?.push(`${event.violatedDirective} ${event.blockedURI}`));
    });
    page.on("response", (response) => {
      if (response.request().method() !== "POST" || !/\/api\/desktops\/[^/]+\/grants\/[^/]+\/terminal-ticket$/.test(new URL(response.url()).pathname)) return;
      void response
        .json()
        .then((json: { ticket?: string }) => {
          if (typeof json.ticket === "string") tickets.push(json.ticket);
        })
        .catch(() => undefined);
    });
    await registerThroughUi(page, namedInvite("terminal"), HANDLE, "Terminal");

    desktopKey = generateKeyPair();
    const pairing = new TsDesktopPairing(origin(), testIp(205));
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

    // The desktop: a mock holding the key the browser pinned, short limits so a test can wait for them, the terminal on.
    desktop = new MockDesktop({ desktopId, staticKey: desktopKey, name: MACHINE, capabilities: WITH_TERMINAL, terminal: { graceMs: 4_000, floodLines: 2000 } });
    const connected = await connectDesktop(origin(), desktopId, token);
    socket = connected.socket;
    desktop.attach(socket);
    // The relay drops a desktop socket without a keepalive ping.
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
    deviceFingerprint = [...desktop.devices.values()].find((device) => device.status === "pending")?.fingerprint ?? "";
    // What the real desktop does when its user confirms: the mock answers the browser, the server learns the grant.
    desktop.confirmDevice(deviceFingerprint, "control");
    const grant = await fetch(`${origin()}/api/desktop/grants/${deviceId}/state`, {
      method: "POST",
      headers: { "Content-Type": "application/json", Authorization: `Bearer ${token}` },
      body: JSON.stringify({ state: "confirmed", permission: "control" }),
    });
    expect(grant.status).toBe(200);
    await expect(pendingScreen).toHaveCount(0, { timeout: 30_000 });

    // The desktop spends a ticket at the Worker like the real one: single use, bound to this desktop, device and project.
    const spend = async (ticket: string, projectId: string): Promise<boolean> => {
      const res = await fetch(`${origin()}/api/desktop/grants/${deviceId}/terminal-ticket`, {
        method: "POST",
        headers: { "Content-Type": "application/json", Authorization: `Bearer ${token}` },
        body: JSON.stringify({ ticket, projectId }),
      });
      const ok = res.status === 200 && ((await res.json()) as { ok?: boolean }).ok === true;
      verdicts.push({ ticket, projectId, ok });
      return ok;
    };
    desktop.terminals.verifyTicket = spend;
    spendTicket = spend;
  });

  test("the footer chip follows the capability: hidden without it, live, and hidden when this device's Allow web terminal is off", async () => {
    await page.setViewportSize({ width: 1280, height: 800 });
    await page.goto(`/d/${desktopId}`);
    await expect(page.getByTestId("app-shell")).toHaveAttribute("data-layout", "desktop");
    await expect(page.getByTestId("app-footer")).toBeVisible();
    await expect(chip()).toBeVisible();
    desktop.setCapabilities(WITHOUT_TERMINAL);
    await expect(chip()).toHaveCount(0);
    desktop.setCapabilities(WITH_TERMINAL);
    await expect(chip()).toBeVisible();
    desktop.setDevice(deviceFingerprint, { terminal: false });
    await expect(chip()).toHaveCount(0);
    desktop.setDevice(deviceFingerprint, { terminal: true });
    await expect(chip()).toBeVisible();
    // No terminal is open yet: no count.
    await expect(page.getByTestId("terminal-count")).toHaveCount(0);
  });

  test("the chip opens a terminal tab with the project chooser; the passkey step opens the shell and spends one ticket", async () => {
    await chip().click();
    await expect(terminalTabs()).toHaveCount(0);
    await expect(page.getByTestId("terminal-chooser")).toBeVisible();
    await expect(page.getByTestId("terminal-project")).toBeVisible();
    await expect(page.getByTestId("terminal-project").locator("option", { hasText: PROJECT })).toHaveCount(1);
    await shot("desktop-chooser");
    await page.getByTestId("terminal-project").selectOption({ label: PROJECT });
    await page.getByTestId("terminal-open").click();
    // The virtual authenticator answers the passkey prompt; the browser gets a ticket and the desktop spends it.
    await expect.poll(() => desktop.terminals.liveIds().length, { timeout: 30_000 }).toBe(1);
    await expect(rows()).toContainText(`Monoptah mock shell in ${PROJECT}`, { timeout: 30_000 });
    expect(tickets).toHaveLength(1);
    expect(desktop.terminals.ticketVerifications()).toBe(1);
    expect(verdicts).toEqual([{ ticket: tickets[0], projectId: "proj-alpha", ok: true }]);
    await expect(terminalTabs()).toHaveCount(1);
    await expect(terminalTabs().first()).toHaveAttribute("aria-selected", "true");
    await expect(page.getByTestId("terminal-dot").first()).toHaveAttribute("data-state", "on");
    await expect(page.getByTestId("terminal-count")).toHaveText("1");
  });

  test("a ticket the desktop already spent is refused by the Worker, also for another project", async () => {
    const spend = spendTicket;
    expect(await spend(tickets[0]!, "proj-alpha")).toBe(false);
    expect(await spend(tickets[0]!, "proj-beta")).toBe(false);
    expect(await spend("not-a-ticket", "proj-alpha")).toBe(false);
    // The shell the first ticket opened is untouched.
    expect(desktop.terminals.liveIds()).toHaveLength(1);
  });

  test("shell works: typing and output in xterm", async () => {
    await type("echo hello-e2e");
    await expect(rows()).toContainText("hello-e2e");
    await type("whoami");
    await expect(rows()).toContainText("example-user");
    await shot("desktop-single");
  });

  test("xterm styles reach the page without an inline <style> and the CSP saw no violation", async () => {
    await expect(page.locator(".xterm style")).toHaveCount(0);
    expect(await page.evaluate(() => document.adoptedStyleSheets.length)).toBeGreaterThan(0);
    expect(await page.evaluate(() => document.adoptedStyleSheets.some((sheet) => [...sheet.cssRules].some((rule) => rule.cssText.includes("xterm-dom-renderer-owner"))))).toBe(true);
    expect(await page.evaluate(() => window.__csp ?? [])).toEqual([]);
    // Control: the strict policy is really in force and the listener sees a violation, so the empty list above means something.
    const policy = await page.evaluate(async () => (await fetch("/")).headers.get("content-security-policy"));
    expect(policy).toContain("style-src 'self'; style-src-attr 'unsafe-inline'");
    expect(policy).toContain("require-trusted-types-for 'script'");
    await page.evaluate(() => {
      const style = document.createElement("style");
      style.textContent = "html { outline: 0 }";
      document.head.appendChild(style);
    });
    await expect.poll(() => page.evaluate(() => window.__csp ?? [])).toEqual([expect.stringContaining("style-src")]);
    await page.evaluate(() => {
      window.__csp = [];
    });
  });

  test("terminal output is text: markup and scripts in it never reach the page", async () => {
    await type("echo <img src=x onerror=window.__xss=1> <script>window.__xss=2</script> <b>bold</b>");
    await expect(rows()).toContainText("<b>bold</b>");
    expect(await page.evaluate(() => window.__xss)).toBeUndefined();
    expect(await page.locator('[data-testid="terminal-screen"] img, [data-testid="terminal-screen"] script, [data-testid="terminal-screen"] b').count()).toBe(0);
    expect(await page.evaluate(() => window.__csp ?? [])).toEqual([]);
  });

  test("a long burst of output does not freeze the page: the last line arrives and the page still answers", async () => {
    await type("flood");
    await expect(rows()).toContainText("flood done", { timeout: 60_000 });
    await expect(chip()).toBeEnabled();
    await type("echo still-alive");
    await expect(rows()).toContainText("still-alive");
  });

  test("a reload within the grace window reattaches to the same shell without a new passkey step", async () => {
    const [id] = desktop.terminals.liveIds();
    expect(id).toBeTruthy();
    const issued = tickets.length;
    await page.reload();
    await expect(terminalTabs()).toHaveCount(1);
    await expect(page.getByTestId("terminal-dot").first()).toHaveAttribute("data-state", "on", { timeout: 30_000 });
    await type("echo after-reload");
    await expect(rows()).toContainText("after-reload");
    expect(desktop.terminals.liveIds()).toEqual([id]);
    expect(tickets).toHaveLength(issued);
  });

  test("the desktop kills the shell: the tab says so, and New terminal returns to the chooser", async () => {
    const [id] = desktop.terminals.liveIds();
    desktop.terminals.kill(id!, "killed");
    await expect(page.getByTestId("terminal-ended")).toContainText("Session ended: the terminal was closed on the desktop.");
    await expect(page.getByTestId("terminal-dot").first()).toHaveAttribute("data-state", "off");
    await page.getByTestId("terminal-ended").getByRole("button", { name: "New terminal" }).click();
    await expect(page.getByTestId("terminal-chooser")).toBeVisible();
    // Closing the now empty chooser tab asks nothing: no shell runs in it.
    await page.getByTestId("tab-close").first().click({ force: true });
    await expect(terminalTabs()).toHaveCount(0);
    await expect(page.getByRole("dialog")).toHaveCount(0);
  });

  test("closing a terminal tab asks, Cancel keeps the shell, Close terminal kills it", async () => {
    await openShell();
    expect(desktop.terminals.liveIds()).toHaveLength(1);
    await page.getByTestId("tab-close").first().click({ force: true });
    const dialog = page.getByRole("dialog");
    await expect(dialog).toContainText("Close this terminal?");
    await dialog.getByRole("button", { name: "Cancel" }).click();
    await expect(dialog).toHaveCount(0);
    expect(desktop.terminals.liveIds()).toHaveLength(1);
    await expect(terminalTabs()).toHaveCount(1);
    await closeTabWithConfirm();
    await expect(terminalTabs()).toHaveCount(0);
    await expect.poll(() => desktop.terminals.liveIds().length).toBe(0);
  });

  test("the idle limit and turning Allow web terminal off end a running shell", async () => {
    await openShell();
    const [idle] = desktop.terminals.liveIds();
    desktop.terminals.kill(idle!, "idle");
    await expect(page.getByTestId("terminal-ended")).toContainText("the terminal was idle for 15 minutes");
    await closeEnded();
    await openShell();
    desktop.setDevice(deviceFingerprint, { terminal: false });
    await expect(page.getByTestId("terminal-ended")).toContainText("Allow web terminal was turned off");
    desktop.setDevice(deviceFingerprint, { terminal: true });
    await closeEnded();
  });

  async function closeEnded(): Promise<void> {
    await expect(page.getByTestId("terminal-ended")).toBeVisible();
    // The shell has ended: closing the tab asks nothing.
    await page.getByTestId("tab-close").first().click({ force: true });
    await expect(terminalTabs()).toHaveCount(0);
    await expect(page.getByRole("dialog")).toHaveCount(0);
  }

  test("a terminal tab splits next to a chat tab with the existing tab drag", async () => {
    await page.goto(`/d/${desktopId}`);
    // A chat tab: the first session of the catalog.
    await page.getByText("Fix the example parser").first().click({ timeout: 30_000 });
    await expect(page.getByRole("tab", { name: "Fix the example parser" })).toBeVisible();
    await openShell();
    const terminalTab = terminalTabs().first();
    const body = page.getByTestId("pane-body").first();
    const box = (await body.boundingBox())!;
    await terminalTab.dragTo(body, { targetPosition: { x: box.width - 20, y: box.height / 2 } });
    await expect(page.getByTestId("pane-split")).toHaveAttribute("data-dir", "row");
    await expect(page.getByTestId("tabbed-chat")).toHaveAttribute("data-panes", "2");
    await expect(page.getByRole("tab", { selected: true })).toHaveCount(2);
    await expect(terminalTabs()).toHaveCount(1);
    await expect(rows()).toContainText(`Monoptah mock shell in ${PROJECT}`);
    await type("echo in-split");
    await expect(rows()).toContainText("in-split");
    await shot("desktop-split");
    await page.setViewportSize({ width: 1024, height: 768 });
    await shot("tablet-split");
    await page.setViewportSize({ width: 1280, height: 800 });
    await closeTabWithConfirm();
    await expect(terminalTabs()).toHaveCount(0);
  });

  test("on a phone the footer chip opens the drawer with the key row; fullscreen, shrink and close work", async () => {
    await page.setViewportSize({ width: 390, height: 844 });
    await page.goto(`/d/${desktopId}`);
    await expect(page.getByTestId("app-shell")).toHaveAttribute("data-layout", "mobile");
    await expect(chip()).toBeVisible();
    await shot("mobile-footer");
    await chip().click();
    const drawer = page.getByTestId("terminal-drawer");
    await expect(drawer).toBeVisible();
    await expect(page.getByTestId("terminal-chooser")).toBeVisible();
    await page.getByTestId("terminal-project").selectOption({ label: PROJECT });
    await page.getByTestId("terminal-open").click();
    await expect(rows()).toContainText(`Monoptah mock shell in ${PROJECT}`, { timeout: 30_000 });
    await expect(page.getByTestId("terminal-drawer-title")).toContainText(PROJECT);
    await expect(drawer).toHaveAttribute("data-fullscreen", "false");
    const keys = page.getByTestId("terminal-keys");
    await expect(keys).toBeVisible();
    await shot("mobile-drawer");
    // The header never pushes the close button off a 390px screen.
    const closeBox = (await page.getByRole("button", { name: "Close drawer" }).boundingBox())!;
    expect(closeBox.x + closeBox.width).toBeLessThanOrEqual(390);

    await type("echo phone");
    await expect(rows()).toContainText("phone");
    // Type a few letters, then Ctrl and c: the mock shell prints ^C and shows a fresh prompt.
    await page.getByTestId("terminal-screen").click();
    await page.keyboard.type("zzz");
    await keys.getByRole("button", { name: "Ctrl" }).click();
    await expect(keys.getByRole("button", { name: "Ctrl" })).toHaveAttribute("aria-pressed", "true");
    await page.keyboard.type("c");
    await expect(rows()).toContainText("^C");
    await expect(keys.getByRole("button", { name: "Ctrl" })).toHaveAttribute("aria-pressed", "false");
    await keys.getByRole("button", { name: "Esc" }).click();
    await keys.getByRole("button", { name: "Up arrow" }).click();
    const liveId = () => desktop.terminals.liveIds()[0] ?? "";
    await expect.poll(() => desktop.terminals.received(liveId()).join("")).toContain("\x1b[A");
    expect(desktop.terminals.received(liveId()).join("")).toContain("\x03");

    const box = async (locator: Locator) => (await locator.boundingBox())!;
    const shrunk = await box(drawer);
    await page.getByRole("button", { name: "Fullscreen", exact: true }).click();
    await expect(drawer).toHaveAttribute("data-fullscreen", "true");
    const full = await box(drawer);
    expect(full.height).toBeGreaterThan(shrunk.height);
    expect(full.y).toBe(0);
    await expect(keys).toBeVisible();
    await shot("mobile-fullscreen");
    await page.getByRole("button", { name: "Shrink", exact: true }).click();
    await expect(drawer).toHaveAttribute("data-fullscreen", "false");
    expect((await box(drawer)).height).toBe(shrunk.height);

    // Closing the drawer leaves the shell running; the chip shows the count and reopens it.
    await page.getByRole("button", { name: "Close drawer" }).click();
    await expect(drawer).toHaveCount(0);
    expect(desktop.terminals.liveIds()).toHaveLength(1);
    await expect(page.getByTestId("terminal-count")).toHaveText("1");
    await chip().click();
    await expect(drawer).toBeVisible();
    await expect(rows()).toContainText("phone");

    // The header has no end button; the switcher (opened from the project name) ends a shell, after asking.
    await expect(page.getByRole("button", { name: "End this terminal" })).toHaveCount(0);
    await page.getByTestId("terminal-drawer-title").getByRole("button").click();
    await page.getByRole("button", { name: /^End / }).click();
    await page.getByRole("dialog").getByRole("button", { name: "Close terminal" }).click();
    await expect.poll(() => desktop.terminals.liveIds().length).toBe(0);
  });

  test("every open spent exactly one ticket at the Worker: the desktop asked once per ticket the browser got, and all were accepted", async () => {
    // The three refused calls are the direct replays of the replay test; they did not go through the mock's open.
    const viaOpen = verdicts.filter((v) => tickets.includes(v.ticket) && v.ok);
    expect(viaOpen).toHaveLength(tickets.length);
    expect(new Set(tickets).size).toBe(tickets.length);
    expect(desktop.terminals.ticketVerifications()).toBe(tickets.length);
    expect(verdicts.filter((v) => !v.ok)).toHaveLength(3);
  });

  test("no CSP violation happened during the whole spec", async () => {
    expect(await page.evaluate(() => window.__csp ?? [])).toEqual([]);
  });
});
