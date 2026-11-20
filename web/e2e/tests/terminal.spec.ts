// The web terminal (features.md N-26) in a real browser, on the real Worker and relay, against the TypeScript mock desktop
// of web/mock-desktop (it plays the desktop: a fake shell with the desktop's one-time code, attempts, kill reasons and the
// capability rules). One account, one paired mock desktop, one open page for all tests (serial).
//
// What it checks: the capability gating of the entry, the open flow with the code shown "on the desktop" (the test reads
// it from the mock's hook, the browser never sees it), a wrong code then the right one, a used up challenge restarting
// with a new code, typing and output in xterm, a long burst, the desktop killing the shell, a reload that reattaches, the
// phone key row, and that terminal output never becomes markup and never trips the CSP.
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

const HANDLE = "terminal";
const MACHINE = "terminal-mock";
const WITH_TERMINAL = [...KNOWN_CAPABILITIES];
const WITHOUT_TERMINAL = KNOWN_CAPABILITIES.filter((name) => name !== "terminal");

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

  test.beforeAll(clearRateLimits);

  test.afterAll(() => {
    if (keepalive) clearInterval(keepalive);
    desktop?.close();
    socket?.close();
  });

  const terminalPath = (): string => `/d/${desktopId}/terminal`;
  const rows = () => page.locator(".xterm-rows");

  /** Moves inside the loaded SPA (the relay rate limits full page loads per address). */
  async function navigate(path: string): Promise<void> {
    await page.evaluate((to) => {
      window.history.pushState({}, "", to);
      window.dispatchEvent(new PopStateEvent("popstate"));
    }, path);
  }

  /** The code the desktop window would show: read from the mock's hook, never from the page. */
  async function shownCode(): Promise<string> {
    await expect.poll(() => desktop.terminals.lastCode(), { timeout: 15_000 }).not.toBeNull();
    return desktop.terminals.lastCode() ?? "";
  }

  async function typeCode(code: string): Promise<void> {
    const input = page.getByTestId("terminal-code-input");
    await input.fill(code);
    await page.getByTestId("terminal-code-submit").click();
  }

  /** Opens a shell the way a user does and waits for the prompt. */
  async function openShell(project?: string): Promise<void> {
    await navigate(terminalPath());
    if (project) await page.getByTestId("terminal-project").selectOption({ label: project });
    await page.getByTestId("terminal-open").click();
    await expect(page.getByTestId("terminal-code-dialog")).toBeVisible();
    await typeCode(await shownCode());
    await expect(page.getByTestId("terminal-status")).toHaveText("Connected", { timeout: 15_000 });
    await expect(rows()).toContainText("Monoptah mock shell in Example Alpha", { timeout: 15_000 });
  }

  async function type(text: string): Promise<void> {
    await page.getByTestId("terminal-screen").click();
    await page.keyboard.type(text);
    await page.keyboard.press("Enter");
  }

  /** Back to the chooser: the shell must be ended (a kill, an exit or Close terminal) before this. */
  async function leaveEnded(): Promise<void> {
    await expect(page.getByTestId("terminal-ended")).toBeVisible();
    await page.getByRole("button", { name: "Open a new terminal" }).click();
    await expect(page.getByTestId("terminal-open")).toBeVisible();
  }

  test("setup: register, pair a mock desktop that keeps its key, serve it and confirm the browser as a control device", async ({ newSharedActor }) => {
    test.setTimeout(240_000);
    const actor = await newSharedActor();
    page = actor.page;
    await page.addInitScript(() => {
      window.__csp = [];
      document.addEventListener("securitypolicyviolation", (event) => window.__csp?.push(`${event.violatedDirective} ${event.blockedURI}`));
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
  });

  test("the entry follows the capability: hidden and explained without it, in the rail with it, live", async () => {
    await page.setViewportSize({ width: 1280, height: 800 });
    desktop.setCapabilities(WITHOUT_TERMINAL);
    await navigate(terminalPath());
    await expect(page.getByTestId("terminal-disabled")).toContainText("Turn on Allow web terminal for this device in Monoptah on your desktop");
    await expect(page.getByRole("link", { name: "Terminal", exact: true })).toHaveCount(0);
    desktop.setCapabilities(WITH_TERMINAL);
    await expect(page.getByRole("link", { name: "Terminal", exact: true })).toBeVisible();
    await expect(page.getByTestId("terminal-open")).toBeVisible();
  });

  test("a control device whose Allow web terminal is off sees no entry and the explanation, then gets it back", async () => {
    desktop.setDevice(deviceFingerprint, { terminal: false });
    await navigate(`/d/${desktopId}`);
    await expect(page.getByTestId("app-shell")).toBeVisible();
    await expect(page.getByRole("link", { name: "Terminal", exact: true })).toHaveCount(0);
    await navigate(terminalPath());
    await expect(page.getByTestId("terminal-disabled")).toBeVisible();
    desktop.setDevice(deviceFingerprint, { terminal: true });
    await expect(page.getByTestId("terminal-open")).toBeVisible();
  });

  test("opens with the code, shows output in xterm and echoes typed commands", async () => {
    await openShell();
    await expect(page.getByTestId("terminal-cwd")).toHaveText("Example Alpha");
    await expect(page.getByTestId("terminal-limits")).toContainText("Closes after 15 min without typing");
    await type("echo hello-e2e");
    await expect(rows()).toContainText("hello-e2e");
    await type("whoami");
    await expect(rows()).toContainText("example-user");
    expect(desktop.terminals.liveIds()).toHaveLength(1);
  });

  test("xterm styles reach the page without an inline <style> and the CSP saw no violation", async () => {
    await expect(page.locator(".xterm style")).toHaveCount(0);
    expect(await page.evaluate(() => document.adoptedStyleSheets.length)).toBeGreaterThan(0);
    // The adopted stylesheets carry xterm's theme and cell rules.
    expect(await page.evaluate(() => document.adoptedStyleSheets.some((sheet) => [...sheet.cssRules].some((rule) => rule.cssText.includes("xterm-dom-renderer-owner"))))).toBe(true);
    // The terminal background is the stylesheet's, not the browser default.
    expect(await page.locator(".xterm-screen").evaluate((el) => getComputedStyle(el).position)).not.toBe("");
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

  test("a long burst of output does not freeze the page: the last line arrives and the buttons still answer", async () => {
    await type("flood");
    await expect(rows()).toContainText("flood done", { timeout: 60_000 });
    await expect(page.getByRole("button", { name: "Copy" })).toBeEnabled();
    await page.getByRole("button", { name: "Close terminal" }).hover();
    await type("echo still-alive");
    await expect(rows()).toContainText("still-alive");
  });

  test("a reload within the grace window reattaches to the same shell", async () => {
    const [id] = desktop.terminals.liveIds();
    expect(id).toBeTruthy();
    await page.reload();
    await expect(page.getByTestId("terminal-status")).toHaveText("Connected", { timeout: 30_000 });
    await type("echo after-reload");
    await expect(rows()).toContainText("after-reload");
    expect(desktop.terminals.liveIds()).toEqual([id]);
  });

  test("exit ends the session and a new one needs a new code", async () => {
    await type("exit");
    await expect(page.getByTestId("terminal-ended")).toContainText("Session ended: the shell exited with code 0.");
    await expect(page.getByRole("button", { name: "Close terminal" })).toBeDisabled();
    await leaveEnded();
    expect(await page.evaluate(() => sessionStorage.getItem(`monoptah.terminal.${location.pathname.split("/")[2]}`))).toBeNull();
  });

  test("a wrong code is refused with the tries left, then the right code opens", async () => {
    await navigate(terminalPath());
    await page.getByTestId("terminal-open").click();
    const code = await shownCode();
    await typeCode(code === "ZZZZZZ" ? "YYYYYY" : "ZZZZZZ");
    await expect(page.getByText("That code is not right. 2 tries left.")).toBeVisible();
    await typeCode(code.toLowerCase());
    await expect(page.getByTestId("terminal-status")).toHaveText("Connected", { timeout: 15_000 });
    await page.getByRole("button", { name: "Close terminal" }).click();
    await page.getByRole("dialog").getByRole("button", { name: "Close terminal" }).click();
    await expect(page.getByTestId("terminal-ended")).toContainText("you closed the terminal");
    await leaveEnded();
  });

  test("three wrong codes use the challenge up: a new code is shown and the old one is dead", async () => {
    await navigate(terminalPath());
    await page.getByTestId("terminal-open").click();
    const first = await shownCode();
    const wrong = first === "ZZZZZZ" ? "YYYYYY" : "ZZZZZZ";
    await typeCode(wrong);
    await expect(page.getByText("That code is not right. 2 tries left.")).toBeVisible();
    await typeCode(wrong);
    await expect(page.getByText("That code is not right. 1 try left.")).toBeVisible();
    await typeCode(wrong);
    await expect(page.getByText("A new code is now shown on your desktop")).toBeVisible();
    const second = await shownCode();
    await expect(page.getByTestId("terminal-code-input")).toHaveValue("");
    await typeCode(second);
    await expect(page.getByTestId("terminal-status")).toHaveText("Connected", { timeout: 15_000 });
  });

  test("the desktop kills the shell (Kill button): the page says so and stops reading", async () => {
    const [id] = desktop.terminals.liveIds();
    desktop.terminals.kill(id!, "killed");
    await expect(page.getByTestId("terminal-ended")).toContainText("Session ended: the terminal was closed on the desktop.");
    await expect(page.getByRole("button", { name: "Close terminal" })).toBeDisabled();
    await leaveEnded();
  });

  test("the idle limit ends the shell", async () => {
    await openShell();
    const [id] = desktop.terminals.liveIds();
    desktop.terminals.kill(id!, "idle");
    await expect(page.getByTestId("terminal-ended")).toContainText("the terminal was idle for 15 minutes");
    await leaveEnded();
  });

  test("turning Allow web terminal off for the device ends a running shell", async () => {
    await openShell();
    desktop.setDevice(deviceFingerprint, { terminal: false });
    await expect(page.getByTestId("terminal-ended")).toContainText("Allow web terminal was turned off");
    desktop.setDevice(deviceFingerprint, { terminal: true });
    await leaveEnded();
  });

  test("on a phone the key row sends Esc, Ctrl combinations and arrows through the same write path", async () => {
    await page.setViewportSize({ width: 390, height: 844 });
    await openShell();
    const keys = page.getByTestId("terminal-keys");
    await expect(keys).toBeVisible();
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
    await expect.poll(() => desktop.terminals.received(desktop.terminals.liveIds()[0] ?? "").join("")).toContain("\x1b[A");
    expect(desktop.terminals.received(desktop.terminals.liveIds()[0] ?? "").join("")).toContain("\x03");
    await page.getByRole("button", { name: "Close terminal" }).click();
    await page.getByRole("dialog").getByRole("button", { name: "Close terminal" }).click();
    await expect(page.getByTestId("terminal-ended")).toBeVisible();
  });

  test("no CSP violation happened during the whole spec", async () => {
    expect(await page.evaluate(() => window.__csp ?? [])).toEqual([]);
  });
});
