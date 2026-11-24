// The API budget of the web app: how many /api calls a page load, a settings navigation and an idle minute cost.
// The Worker allows one session 120 API calls per 60 s (modules/ratelimit/memory.ts); a page that spends most of that
// on one load makes the next reload answer 429. One account and one paired headless desktop, serial. Every request of
// the browser context is recorded (a service worker's too), and a table of calls per `METHOD path` is printed per step,
// duplicates visible, so a regression shows what was fetched twice.
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { Page } from "@playwright/test";
import type { Actor } from "../lib/actor.ts";
import { clearRateLimits, namedInvite } from "../lib/env.ts";
import { test, expect } from "../lib/fixtures.ts";
import { probePair } from "../lib/probes.ts";
import { HeadlessDesktop } from "../lib/r12.ts";
import { startPairing, type PendingPairing } from "../lib/w14-headless.ts";
import { registerThroughUi } from "../lib/w14-ui.ts";

const HANDLE = "apibudget";
const MACHINE = "budget-headless";

/** Time to let polls, retries and lazy queries of a page start after the network went idle. */
const SETTLE_MS = 2_000;
/** Idle window for the polling measurement: two ticks of the 15 s refresh plus margin. */
const IDLE_MS = 31_000;

// Budgets: what a step may cost. Each is the number observed after the fix plus a margin of one call; raise one only
// with a reason (a new screen that needs one more call), never to hide a duplicate.
const COLD_LOAD_BUDGET: Record<string, number> = {
  // The account (/api/me), the desktop list, the link's directory read and its grant request.
  "/": 5,
  "/d/:id": 5,
  "/settings": 3,
  "/settings/profile": 3,
  "/settings/security": 5,
  "/settings/appearance": 3,
  "/settings/notifications": 4,
  "/settings/install": 3,
  "/settings/invites": 4,
  "/settings/desktops": 4,
  // Adds the directory entry twice (the page and the link, which must read the key fresh), the grants and the push prefs.
  "/settings/desktops/:id": 8,
};
/** Client-side steps in the settings nav. Opening a desktop connects its link (directory read and grant request) and reads grants and prefs. */
const NAVIGATION_BUDGET: Record<string, number> = {
  "desktops -> desktop": 6,
  "desktop -> notifications": 2,
  "notifications -> profile": 1,
  "profile -> desktop": 2,
  "desktop -> desktops": 1,
};
/** Calls in one idle window of 33 s (two 15 s ticks): the list, plus the grants on the desktop page, per tick. */
const IDLE_WINDOW_BUDGET: Record<string, number> = { "/settings/desktops/:id": 5, "/d/:id": 3 };
/** The link reads the directory entry itself on connect (it must be fresh for the key check), next to the page's own read. */
const DIRECTORY_READS_PER_LOAD = 2;

interface Call {
  key: string;
  at: number;
}

function table(label: string, calls: Call[]): void {
  const first = calls[0]?.at ?? 0;
  const groups = new Map<string, number[]>();
  for (const call of calls) groups.set(call.key, [...(groups.get(call.key) ?? []), call.at - first]);
  const rows = [...groups.entries()].sort((a, b) => b[1].length - a[1].length || a[0].localeCompare(b[0]));
  const lines = rows.map(([key, times]) => `  ${String(times.length).padStart(3)}  ${key}  (+${times.join("ms, +")}ms)`);
  console.log(`API ${label}: ${calls.length} calls\n${lines.join("\n")}`);
}

/** `METHOD path` with ids kept (so the same URL requested twice shows as a count of 2) and the query kept too. */
function describe(method: string, url: string): string {
  const parsed = new URL(url);
  return `${method} ${parsed.pathname}${parsed.search}`;
}

test.describe("API call budget", () => {
  test.describe.configure({ mode: "serial" });
  const probe = probePair();
  if (!probe.run) console.log(`skip reason: ${probe.reason}`);
  test.skip(!probe.run, probe.reason);

  let dataRoot = "";
  let pending: PendingPairing | undefined;
  let desktop: HeadlessDesktop | undefined;
  let desktopId = "";
  let page: Page;
  let actor: Actor;
  let recorded: Call[] = [];
  const throttled: string[] = [];
  let recording = false;

  test.beforeAll(clearRateLimits);
  test.afterAll(() => {
    desktop?.kill();
    pending?.dispose();
    if (dataRoot) rmSync(dataRoot, { recursive: true, force: true });
  });

  /**
   * A fresh rate limit window. Clearing the limits restarts the Worker, which drops the relay rooms, so wait until the
   * headless desktop is online again: a link that connects to an offline desktop retries and would inflate the counts.
   */
  async function freshWindow(): Promise<void> {
    await clearRateLimits();
    await expect
      .poll(async () => (await actor.call("GET", `/api/desktops/${desktopId}`)).json["online"], { timeout: 60_000, intervals: [500] })
      .toBe(true);
  }

  /** Runs `step` and returns every /api call it caused. */
  async function measure(label: string, step: () => Promise<void>): Promise<Call[]> {
    recorded = [];
    recording = true;
    try {
      await step();
      await page.waitForLoadState("networkidle");
      await page.waitForTimeout(SETTLE_MS);
    } finally {
      recording = false;
    }
    const calls = recorded;
    table(label, calls);
    return calls;
  }

  function expectNoRepeatedGet(calls: Call[]): void {
    const directory = `GET /api/desktops/${desktopId}`;
    const gets = calls.map((call) => call.key).filter((key) => key.startsWith("GET "));
    const repeated = gets.filter((key, index) => key !== directory && gets.indexOf(key) !== index);
    expect.soft(repeated, "a GET requested twice").toEqual([]);
    expect.soft(gets.filter((key) => key === directory).length, "directory reads").toBeLessThanOrEqual(DIRECTORY_READS_PER_LOAD);
  }

  test("setup: register, pair a desktop, confirm this browser", async ({ newSharedActor }) => {
    test.setTimeout(240_000);
    dataRoot = mkdtempSync(join(tmpdir(), "monoptah-e2e-apibudget-"));
    actor = await newSharedActor();
    page = actor.page;
    await page.setViewportSize({ width: 1280, height: 800 });
    page.context().on("response", (response) => {
      if (response.status() === 429 && new URL(response.url()).pathname.startsWith("/api/")) throttled.push(`${response.request().method()} ${response.url()}`);
    });
    page.context().on("request", (request) => {
      if (!recording) return;
      const url = new URL(request.url());
      if (url.pathname.startsWith("/api/")) recorded.push({ key: describe(request.method(), request.url()), at: Date.now() });
    });
    await registerThroughUi(page, namedInvite("apibudget"), HANDLE, "Api Budget");

    pending = await startPairing(MACHINE);
    const { userCode, fingerprint } = pending.started;
    await page.goto(`/pair/${userCode}#fp=${fingerprint}`);
    await page.getByRole("button", { name: "Approve" }).click();
    await expect(page.getByRole("heading", { name: "Desktop approved" })).toBeVisible();
    expect(await pending.process.exited, pending.process.stderr()).toBe(0);

    desktop = new HeadlessDesktop(["serve", "--data-dir", join(pending.dataDir, "data")]);
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

  test("cold loads: no GET twice, and each page within its budget", async () => {
    test.setTimeout(240_000);
    await freshWindow();
    const totals: Record<string, number> = {};
    for (const route of Object.keys(COLD_LOAD_BUDGET)) {
      const path = route.replace(":id", desktopId);
      const calls = await measure(`cold load ${route}`, async () => {
        await page.goto(path);
      });
      totals[route] = calls.length;
      expectNoRepeatedGet(calls);
    }
    console.log(`API cold load totals: ${JSON.stringify(totals)}`);
    for (const [route, budget] of Object.entries(COLD_LOAD_BUDGET)) {
      expect.soft(totals[route], `cold load of ${route}`).toBeLessThanOrEqual(budget);
    }
  });

  test("settings navigation: client-side steps stay within the budget", async () => {
    test.setTimeout(120_000);
    await freshWindow();
    await page.goto("/settings/desktops");
    await page.waitForLoadState("networkidle");
    const nav = page.getByRole("navigation", { name: "Settings" });
    const steps: Array<[string, () => Promise<void>]> = [
      ["desktops -> desktop", () => nav.getByRole("link", { name: new RegExp(`^${MACHINE}`) }).click()],
      ["desktop -> notifications", () => nav.getByRole("link", { name: "Notifications" }).click()],
      ["notifications -> profile", () => nav.getByRole("link", { name: "Profile" }).click()],
      ["profile -> desktop", () => nav.getByRole("link", { name: new RegExp(`^${MACHINE}`) }).click()],
      ["desktop -> desktops", () => nav.getByRole("link", { name: "All desktops" }).click()],
    ];
    const counts: Record<string, number> = {};
    for (const [label, step] of steps) {
      const calls = await measure(`navigation ${label}`, step);
      counts[label] = calls.length;
    }
    console.log(`API navigation totals: ${JSON.stringify(counts)}`);
    for (const [label, count] of Object.entries(counts)) {
      expect.soft(count, `navigation ${label}`).toBeLessThanOrEqual(NAVIGATION_BUDGET[label] ?? 0);
    }
  });

  test("idle: polling on the desktop settings page and on the desktop", async () => {
    test.setTimeout(180_000);
    await freshWindow();
    const perWindow: Record<string, number> = {};
    for (const route of Object.keys(IDLE_WINDOW_BUDGET)) {
      await page.goto(route.replace(":id", desktopId));
      await page.waitForLoadState("networkidle");
      await page.waitForTimeout(SETTLE_MS);
      const calls = await measure(`idle ${route}`, async () => {
        await page.waitForTimeout(IDLE_MS);
      });
      perWindow[route] = calls.length;
    }
    console.log(`API idle calls per ${IDLE_MS + SETTLE_MS} ms: ${JSON.stringify(perWindow)}`);
    for (const [route, budget] of Object.entries(IDLE_WINDOW_BUDGET)) {
      expect.soft(perWindow[route], `idle calls on ${route}`).toBeLessThanOrEqual(budget);
    }
  });

  test("avatar: the picture is fetched once and a reload serves it from the browser cache", async () => {
    test.setTimeout(120_000);
    await freshWindow();
    const png = "iVBORw0KGgoAAAANSUhEUgAAAAIAAAACCAIAAAD91JpzAAAAEElEQVR4nGP4zwAE/xkgFAAb8gP91pbyKwAAAABJRU5ErkJggg==";
    const patched = await actor.call("PATCH", `/api/desktops/${desktopId}`, { body: { avatar: { kind: "image", mime: "image/png", data: png } } });
    expect(patched.status).toBe(200);
    const avatarPath = new RegExp(`^/api/desktops/${desktopId}/avatar\\?v=[0-9a-f]{16}$`);
    // No page.route here: it turns the HTTP cache off in Playwright. The DevTools protocol reports where each response came from.
    const cdp = await page.context().newCDPSession(page);
    await cdp.send("Network.enable");
    const loads = new Map<string, { url: string; cached: boolean }>();
    const isAvatar = (url: string) => avatarPath.test(new URL(url).pathname + new URL(url).search);
    cdp.on("Network.requestWillBeSent", (event) => {
      if (isAvatar(event.request.url)) loads.set(event.requestId, { url: event.request.url, cached: false });
    });
    cdp.on("Network.requestServedFromCache", (event) => {
      const load = loads.get(event.requestId);
      if (load) load.cached = true;
    });
    cdp.on("Network.responseReceived", (event) => {
      const load = loads.get(event.requestId);
      if (load && event.response.fromDiskCache) load.cached = true;
    });
    const railImage = () => page.getByTestId("activity-rail").getByRole("link", { name: new RegExp(`^${MACHINE}`) }).getByTestId("avatar-image");
    const rendered = async () => {
      await expect(railImage()).toHaveAttribute("src", avatarPath, { timeout: 30_000 });
      await expect.poll(() => railImage().evaluate((element: HTMLImageElement) => element.complete && element.naturalWidth > 0)).toBe(true);
      await page.waitForLoadState("networkidle");
    };
    try {
      await page.goto(`/d/${desktopId}`);
      await rendered();
      // Control: the first load did reach the network, so the observation below is not blind.
      expect([...loads.values()].filter((load) => !load.cached).length, "first load fetches the picture").toBeGreaterThanOrEqual(1);

      loads.clear();
      await page.reload();
      await rendered();
      const refetched = [...loads.values()].filter((load) => !load.cached).map((load) => load.url);
      expect(refetched, "avatar requests that went to the network after the reload").toEqual([]);
      const timing = await page.evaluate(() =>
        performance
          .getEntriesByType("resource")
          .filter((entry) => /\/avatar\?v=/.test(entry.name))
          .map((entry) => ({ transferSize: (entry as PerformanceResourceTiming).transferSize, decodedBodySize: (entry as PerformanceResourceTiming).decodedBodySize })),
      );
      expect(timing.length, "the reloaded page requested the picture").toBeGreaterThanOrEqual(1);
      for (const entry of timing) {
        expect(entry.transferSize).toBe(0);
        expect(entry.decodedBodySize).toBeGreaterThan(0);
      }
    } finally {
      await cdp.detach();
      const cleared = await actor.call("PATCH", `/api/desktops/${desktopId}`, { body: { avatar: null } });
      expect(cleared.status).toBe(200);
    }
  });

  test("no /api response was a 429 during the whole spec", () => {
    expect(throttled).toEqual([]);
  });
});
