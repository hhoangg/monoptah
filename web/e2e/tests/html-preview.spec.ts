// The preview of an html report a chat mentions, in Chromium (desktop) and in WebKit with an iPhone profile (project
// `webkit-mobile`): the relative and the inline image really display, the dialog fits a narrow screen (a long path must
// not widen it past the edge), and the full screen toggle works. The desktop is the real core code behind
// `web_link_headless serve`. The account is made through the API with a software authenticator and its session cookie
// is handed to the browser, because WebKit has no virtual WebAuthn authenticator (that one is Chromium CDP only).
import { deflateSync } from "node:zlib";
import { mkdirSync, mkdtempSync, readFileSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test as base, expect, type Page } from "@playwright/test";
import { E2E_CATALOG } from "../lib/catalog.ts";
import { clearRateLimits, namedInvite, origin } from "../lib/env.ts";
import { startCookieProxy, type CookieProxy } from "../lib/cookie-proxy.ts";
import { probePair } from "../lib/probes.ts";
import { testIp } from "../lib/pure.ts";
import { HeadlessDesktop } from "../lib/r12.ts";
import { SoftActor } from "../lib/soft-actor.ts";
import { startPairing, type PendingPairing } from "../lib/w14-headless.ts";

const MACHINE = "html-preview-headless";

/** A real, valid 8x8 opaque red PNG. */
function tinyPng(): Buffer {
  const crcTable = Array.from({ length: 256 }, (_, n) => {
    let c = n;
    for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
    return c >>> 0;
  });
  const crc = (buf: Buffer): number => {
    let c = 0xffffffff;
    for (const byte of buf) c = crcTable[(c ^ byte) & 0xff]! ^ (c >>> 8);
    return (c ^ 0xffffffff) >>> 0;
  };
  const chunk = (type: string, data: Buffer): Buffer => {
    const body = Buffer.concat([Buffer.from(type, "ascii"), data]);
    const length = Buffer.alloc(4);
    length.writeUInt32BE(data.length);
    const sum = Buffer.alloc(4);
    sum.writeUInt32BE(crc(body));
    return Buffer.concat([length, body, sum]);
  };
  const size = 8;
  const header = Buffer.alloc(13);
  header.writeUInt32BE(size, 0);
  header.writeUInt32BE(size, 4);
  header[8] = 8;
  header[9] = 2;
  const row = Buffer.concat([Buffer.from([0]), Buffer.concat(Array.from({ length: size }, () => Buffer.from([200, 0, 0])))]);
  const raw = Buffer.concat(Array.from({ length: size }, () => row));
  return Buffer.concat([Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]), chunk("IHDR", header), chunk("IDAT", deflateSync(raw)), chunk("IEND", Buffer.alloc(0))]);
}

const PNG = tinyPng();

/** A valid RGB PNG of about `bytes` bytes: random pixels, stored without compression, so the size is what was asked for. */
function noisePng(bytes: number, seed: number): Buffer {
  const side = Math.max(8, Math.floor(Math.sqrt(bytes / 3)));
  const raw = Buffer.alloc(side * (1 + side * 3));
  let state = seed >>> 0 || 1;
  for (let at = 0; at < raw.length; at++) {
    state = (Math.imul(state, 1664525) + 1013904223) >>> 0;
    raw[at] = at % (1 + side * 3) === 0 ? 0 : state >>> 24;
  }
  const crcTable = Array.from({ length: 256 }, (_, n) => {
    let c = n;
    for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
    return c >>> 0;
  });
  const crc = (buf: Buffer): number => {
    let c = 0xffffffff;
    for (const byte of buf) c = crcTable[(c ^ byte) & 0xff]! ^ (c >>> 8);
    return (c ^ 0xffffffff) >>> 0;
  };
  const chunk = (type: string, data: Buffer): Buffer => {
    const body = Buffer.concat([Buffer.from(type, "ascii"), data]);
    const length = Buffer.alloc(4);
    length.writeUInt32BE(data.length);
    const sum = Buffer.alloc(4);
    sum.writeUInt32BE(crc(body));
    return Buffer.concat([length, body, sum]);
  };
  const header = Buffer.alloc(13);
  header.writeUInt32BE(side, 0);
  header.writeUInt32BE(side, 4);
  header[8] = 8;
  header[9] = 2;
  return Buffer.concat([Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]), chunk("IHDR", header), chunk("IDAT", deflateSync(raw, { level: 0 })), chunk("IEND", Buffer.alloc(0))]);
}

/** The eight images of a real report (sizes of the one that failed in practice), the last one about 16 MB. */
const GALLERY: ReadonlyArray<readonly [string, number]> = [
  ["composer-before.png", 92_000],
  ["composer-after.png", 91_000],
  ["swipe.png", 248_000],
  ["long-press.png", 244_000],
  ["rail-desktop.png", 29_000],
  ["avatar-menu.png", 23_000],
  ["toast.png", 198_000],
  ["large-noise-16mb.png", 16 * 1024 * 1024],
];

const galleryHtml = `<!doctype html>
<html><head><title>Gallery</title></head><body>
<h1 id="gallery-title">Gallery report</h1>
${GALLERY.map(([name], index) => `<img id="g${index}" src="img/${name}" alt="${name}">`).join("\n")}
</body></html>
`;

/** A dark report: its look comes from a <style> block, its content is centered in a column, and its images are a relative file, an absolute path, a file:// URL and an inline data: image, plus a ../ one outside its folder. */
const html = (dir: string): string => `<!doctype html>
<html><head><title>Report</title>
<style>
body { background: #101820; color: #f0f0f0; font-family: sans-serif; }
h1 { color: rgb(200, 0, 0); }
.column { max-width: 30rem; margin: 0 auto; padding: 1rem; }
</style>
</head><body>
<div class="column" id="column">
<h1 id="title">Quarterly report</h1>
<img id="relative" src="img/chart.png" alt="relative chart">
<img id="inline" src="data:image/png;base64,${PNG.toString("base64")}" alt="inline chart">
<img id="absolute" src="${dir}/img/chart.png" alt="absolute chart">
<img id="fileurl" src="file://${dir}/img/chart.png" alt="file url chart">
<img id="escape" src="../outside.png" alt="outside the folder">
<script>document.body.setAttribute('data-script-ran','1');</script>
</div>
</body></html>
`;

const test = base;

test.describe("html preview on the web app", () => {
  test.describe.configure({ mode: "serial" });
  const probe = probePair();
  if (!probe.run) console.log(`skip reason: ${probe.reason}`);
  test.skip(!probe.run, probe.reason);

  let fixtureDir = "";
  let reportFile = "";
  let pending: PendingPairing | null = null;
  let desktop: HeadlessDesktop | null = null;
  let proxy: CookieProxy | null = null;
  let page: Page;

  test.beforeAll(clearRateLimits);
  test.afterAll(() => {
    desktop?.kill();
    pending?.dispose();
    if (fixtureDir) rmSync(fixtureDir, { recursive: true, force: true });
  });

  const mention = () => page.locator('button[data-testid="file-mention"][data-path$="report.html"]').first();
  const dialog = () => page.getByTestId("file-preview-dialog");
  const inner = () => page.frameLocator('[data-testid="preview-html"]');

  async function openReport(): Promise<void> {
    await mention().click();
    await expect(dialog()).toBeVisible();
    await expect(inner().locator("h1")).toHaveText("Quarterly report");
  }

  /** Waits until the dialog's open animation ended, so box measurements are final. */
  async function settle(): Promise<void> {
    await page.evaluate(async () => {
      const finite = () => document.getAnimations().filter((a) => Number.isFinite(a.effect?.getComputedTiming().endTime ?? 0));
      while (finite().some((a) => a.playState === "running")) await Promise.allSettled(finite().map((a) => a.finished));
      await new Promise<void>((resolve) => requestAnimationFrame(() => requestAnimationFrame(() => resolve())));
    });
  }

  test.afterEach(async () => {
    if (!page || page.isClosed()) return;
    // Back to the project's own viewport and the plain dialog, then closed.
    if (await dialog().isVisible()) {
      await page.keyboard.press("Escape");
      if (await dialog().isVisible()) await page.keyboard.press("Escape");
      await expect(dialog()).toHaveCount(0);
    }
  });

  test.afterAll(async () => {
    await page?.context().close();
    await proxy?.close();
  });

  test("setup: account, paired desktop, open the session", async ({ browser }, testInfo) => {
    test.setTimeout(240_000);
    const webkit = testInfo.project.name === "webkit-mobile";
    fixtureDir = mkdtempSync(join(tmpdir(), "monoptah-e2e-htmlpreview-"));
    // A deep, long folder name: the path line of the dialog has no break point and is far wider than a phone.
    const deep = join(fixtureDir, "casium", "reports", "868mx2djq-fn-list-performance-report-with-a-very-long-name");
    mkdirSync(join(deep, "img"), { recursive: true });
    const dir = join(realpathSync(fixtureDir), "casium", "reports", "868mx2djq-fn-list-performance-report-with-a-very-long-name");
    reportFile = join(deep, "report.html");
    writeFileSync(reportFile, html(dir));
    writeFileSync(join(deep, "img", "chart.png"), PNG);
    writeFileSync(join(deep, "gallery.html"), galleryHtml);
    GALLERY.forEach(([name, bytes], index) => writeFileSync(join(deep, "img", name), noisePng(bytes, index + 1)));
    // Beside the report's folder, so a `../` reference reaches a real image that must still be refused (not a project here).
    writeFileSync(join(deep, "..", "outside.png"), PNG);

    const owner = new SoftActor({ origin: origin(), rpId: "localhost", ip: testIp(webkit ? 202 : 201) });
    const registered = await owner.register(namedInvite(webkit ? "htmlpreviewwk" : "htmlpreview"), webkit ? "hpreviewwk" : "hpreview");
    expect(registered.status).toBe(200);
    // The saved-codes step the register page asks for, so the app does not stop at "Get new recovery codes".
    expect((await owner.call("POST", "/api/recovery-codes/ack", { body: { batch: registered.json["batch"] } })).status).toBe(200);
    const session = owner.sessionCookie();
    expect(session).toBeDefined();

    pending = await startPairing(MACHINE);
    const { userCode } = pending.started;
    expect((await owner.reauth()).status).toBe(200);
    expect((await owner.call("POST", `/api/pair/${userCode}/approve`, { body: {} })).status).toBe(200);
    expect(await pending.process.exited, pending.process.stderr()).toBe(0);

    const text = `The report is \`${dir}/report.html\` and the gallery is \`${dir}/gallery.html\`.`;
    process.env["WEB_LINK_HEADLESS_MENTION_DIR"] = deep;
    process.env["WEB_LINK_HEADLESS_MENTION_TEXT"] = text.replaceAll(dir, "{dir}");
    process.env["WEB_LINK_HEADLESS_MENTION_TOOL_TEXT"] = "";
    try {
      desktop = new HeadlessDesktop(["serve", "--data-dir", join(pending.dataDir, "data")]);
    } finally {
      delete process.env["WEB_LINK_HEADLESS_MENTION_DIR"];
      delete process.env["WEB_LINK_HEADLESS_MENTION_TEXT"];
      delete process.env["WEB_LINK_HEADLESS_MENTION_TOOL_TEXT"];
    }
    const online = await desktop.waitFor("online", {}, 60_000);
    const desktopId = String(online["desktopId"]);

    // A fresh context of the project's own profile (iPhone for WebKit), signed in with the session of the owner.
    // WebKit goes through a proxy that adds the session cookie to the relay websocket handshake (see lib/cookie-proxy.ts).
    if (webkit) proxy = await startCookieProxy(origin(), `${session!.name}=${session!.value}`);
    const site = proxy?.origin ?? origin();
    const context = await browser.newContext({ ...testInfo.project.use, baseURL: site, serviceWorkers: "block" });
    if (webkit) {
      // The app CSP carries `upgrade-insecure-requests`. Chromium leaves localhost alone, WebKit upgrades every
      // http://localhost sub-resource and websocket to https and the local run (plain http) never loads. Only that one
      // directive is dropped from the local response; every other directive stays in force.
      await context.route(
        (url) => url.protocol === "http:" && url.origin === site,
        async (route) => {
          const response = await route.fetch();
          const headers = response.headers();
          const csp = headers["content-security-policy"];
          if (csp) headers["content-security-policy"] = csp.split(";").map((part) => part.trim()).filter((part) => part !== "upgrade-insecure-requests").join("; ");
          await route.fulfill({ response, headers });
        },
      );
    }
    await context.addCookies([{ name: session!.name, value: session!.value, domain: new URL(origin()).hostname, path: "/", httpOnly: true, secure: true }]);
    page = await context.newPage();
    await page.goto("/");
    await page.evaluate((id) => window.localStorage.setItem(`monoptah.pickerSeen.${id}`, "1"), desktopId);
    // WebKit can interrupt a load that the app's own redirect from the first page is still replacing: ask again.
    await expect(async () => {
      await page.goto(`/d/${desktopId}`);
    }).toPass({ timeout: 20_000 });

    const pendingScreen = page.getByRole("alertdialog", { name: "Confirm this device on your desktop" });
    await expect(pendingScreen).toBeVisible({ timeout: 30_000 });
    let deviceId = "";
    await expect
      .poll(async () => {
        const grants = (await owner.call("GET", `/api/desktops/${desktopId}/grants`)).json["grants"] as { deviceId: string; state: string }[];
        deviceId = grants.find((g) => g.state === "pending")?.deviceId ?? "";
        return deviceId !== "";
      })
      .toBe(true);
    const confirmed = desktop;
    await expect
      .poll(
        async () => {
          if (confirmed.matching("confirmed", { deviceId }).length > 0) return true;
          confirmed.command({ cmd: "approve", deviceId, permission: "control" });
          await new Promise((resolve) => setTimeout(resolve, 1_500));
          return confirmed.matching("confirmed", { deviceId }).length > 0;
        },
        { timeout: 45_000, intervals: [100] },
      )
      .toBe(true);
    await expect(pendingScreen).toHaveCount(0, { timeout: 30_000 });

    await page.goto(`/d/${desktopId}/s/${E2E_CATALOG.sessionId}`);
    await expect(page.getByRole("heading", { level: 1, name: E2E_CATALOG.sessionTitle })).toBeVisible();
    // The fake desktop echoes a user message as an assistant turn, which renders the chip.
    await page.getByRole("textbox", { name: "Message" }).fill(text);
    await page.getByRole("button", { name: "Send", exact: true }).click();
    await expect(mention()).toBeVisible({ timeout: 30_000 });
  });

  test("the html shows its style, its relative image and its inline image, and no script runs", async () => {
    await openReport();
    const frame = page.getByTestId("preview-html");
    expect(await frame.getAttribute("sandbox")).toBe("");
    expect(await inner().locator("h1").evaluate((el) => getComputedStyle(el).color)).toBe("rgb(200, 0, 0)");
    // The dark page background reached the frame (the report is not a white box with white text).
    expect(await inner().locator("body").evaluate((el) => getComputedStyle(el.firstElementChild as Element).backgroundColor)).toBe("rgb(16, 24, 32)");
    for (const id of ["relative", "inline", "absolute", "fileurl"]) {
      await expect.poll(() => inner().locator(`#${id}`).evaluate((el) => (el as HTMLImageElement).naturalWidth), { message: `${id} image` }).toBe(8);
    }
    // Outside the document's folder (no project holds it here): refused, so it shows the "Image not loaded" placeholder (not the picture, not a broken image) and the notice counts it.
    expect(await inner().locator("#escape").evaluate((el) => (el as HTMLImageElement).src.startsWith("data:image/svg+xml;base64,"))).toBe(true);
    expect(await inner().locator("#escape").evaluate((el) => (el as HTMLImageElement).naturalWidth)).toBe(240);
    await expect(page.getByTestId("preview-skipped-notice")).toContainText("1 not loaded");
    expect(await inner().locator("body").getAttribute("data-script-ran")).toBeNull();
  });

  test("a page with 8 relative images, one of 16 MB, shows every one of them: none is dropped or left broken", async () => {
    test.setTimeout(180_000);
    await page.setViewportSize({ width: 1100, height: 760 });
    // Every download waits 2.5 s: the page must be on screen long before its images are.
    let slow = true;
    await page.route("**/api/transfers/**", async (route) => {
      if (slow) await new Promise((resolve) => setTimeout(resolve, 2_500));
      await route.continue();
    });
    try {
      await page.locator('button[data-testid="file-mention"][data-path$="gallery.html"]').first().click();
      await expect(dialog()).toBeVisible();
      await expect(inner().locator("#gallery-title")).toHaveText("Gallery report", { timeout: 20_000 });
      // Text first: at this moment no image has arrived, each one is a data: placeholder (loading), not a broken picture.
      expect(await inner().locator("img").evaluateAll((images) => images.length > 0 && images.every((img) => (img as HTMLImageElement).src.startsWith("data:image/svg+xml") && (img as HTMLImageElement).naturalWidth === 240))).toBe(true);
    } finally {
      // Later downloads go at full speed; the ones already waiting still finish.
      slow = false;
    }
    for (let index = 0; index < GALLERY.length; index++) {
      const name = GALLERY[index]![0];
      // Swapped in as it arrives: the real picture (a png data URI that decodes), no longer the loading placeholder.
      await expect
        .poll(async () => {
          // On a touch screen the frame stops reloading by itself after a few seconds (nothing inside it can tell the app the reader is scrolling): the ready images wait behind Show.
          const show = page.getByTestId("preview-html-ready-show");
          if (await show.isVisible()) await show.click();
          // The frame navigates to the next document as images arrive: a read that lands in between is simply asked again.
          return inner()
            .locator(`#g${index}`)
            .evaluate((el) => ((el as HTMLImageElement).src.startsWith("data:image/png") ? (el as HTMLImageElement).naturalWidth : 0))
            .catch(() => 0);
        }, { message: name, timeout: 120_000 })
        .toBeGreaterThan(0);
    }
    expect(await inner().locator("img").evaluateAll((images) => images.every((img) => (img as HTMLImageElement).src.startsWith("data:image/png")))).toBe(true);
    await expect(page.getByTestId("preview-skipped-notice")).toHaveCount(0);
    await page.unroute("**/api/transfers/**", undefined);
  });

  test("Download of an html file saves the file as it is on disk, not the sanitized page", async () => {
    await openReport();
    const [download] = await Promise.all([page.waitForEvent("download"), page.getByTestId("file-preview-download").click()]);
    expect(download.suggestedFilename()).toBe("report.html");
    const saved = readFileSync(await download.path());
    expect(saved.equals(readFileSync(reportFile))).toBe(true);
    // The script and the relative image references the sanitizer removes or rewrites are all still there.
    expect(saved.toString("utf8")).toContain("data-script-ran");
    expect(saved.toString("utf8")).not.toContain("monoptah-asset:");
  });

  test("the page's images are listed outside the frame, and each downloads or opens in the zoom viewer", async () => {
    await page.setViewportSize({ width: 1100, height: 760 });
    await openReport();
    // A click inside the sandboxed frame cannot reach the app: the list beside it is how an image is reached.
    await page.getByTestId("preview-html-images-toggle").click();
    const row = page.getByTestId("preview-html-image-row").filter({ hasText: "chart.png" }).first();
    await expect(row).toHaveAttribute("data-state", "loaded", { timeout: 20_000 });
    const [download] = await Promise.all([page.waitForEvent("download"), row.getByTestId("preview-html-image-download").click()]);
    expect(download.suggestedFilename()).toBe("chart.png");
    expect(readFileSync(await download.path()).subarray(0, 4)).toEqual(Buffer.from([0x89, 0x50, 0x4e, 0x47]));
    await row.getByTestId("preview-html-image-open").click();
    const lightbox = page.getByTestId("image-lightbox");
    await expect(lightbox.getByTestId("preview-image")).toBeVisible();
    await expect(lightbox.getByTestId("preview-image-download")).toBeVisible();
    await page.keyboard.press("Escape");
    await expect(lightbox).toHaveCount(0);
    // The sandbox is untouched.
    expect(await page.getByTestId("preview-html").getAttribute("sandbox")).toBe("");
  });

  test("the dialog fits a 320 px wide screen: no sideways overflow, the title and path truncate, the close button is clear, the report column is on screen", async () => {
    await page.setViewportSize({ width: 320, height: 640 });
    await openReport();
    await settle();
    const width = 320;
    const box = (await dialog().boundingBox())!;
    expect(box.x).toBeGreaterThanOrEqual(0);
    expect(box.x + box.width).toBeLessThanOrEqual(width + 0.5);
    const frameBox = (await page.getByTestId("preview-html").boundingBox())!;
    expect(frameBox.x).toBeGreaterThanOrEqual(0);
    expect(frameBox.x + frameBox.width).toBeLessThanOrEqual(width + 0.5);
    // The path line is far longer than the screen, yet the page does not scroll sideways.
    const path = page.getByTestId("file-preview-path");
    expect((await path.textContent())!.length).toBeGreaterThan(80);
    const pathBox = (await path.boundingBox())!;
    expect(pathBox.x + pathBox.width).toBeLessThanOrEqual(width + 0.5);
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
    // The title never runs under the close button or the full screen button.
    const name = (await page.getByTestId("file-preview-name").boundingBox())!;
    const close = (await dialog().getByRole("button", { name: "Close" }).boundingBox())!;
    const fullscreen = (await page.getByTestId("file-preview-fullscreen").boundingBox())!;
    expect(name.x + name.width).toBeLessThanOrEqual(fullscreen.x + 0.5);
    expect(fullscreen.x + fullscreen.width).toBeLessThanOrEqual(close.x + 0.5);
    // The centered report column is inside the visible frame, not off to the right.
    const column = (await inner().locator("#column").boundingBox())!;
    expect(column.x).toBeGreaterThanOrEqual(0);
    expect(column.x + column.width).toBeLessThanOrEqual(width + 0.5);
    await page.screenshot({ path: test.info().outputPath("narrow.png") });
  });

  test("full screen fills the viewport, toggling back restores the dialog, and Escape leaves full screen before it closes the dialog", async () => {
    await page.setViewportSize({ width: 390, height: 700 });
    await openReport();
    await settle();
    const toggle = page.getByTestId("file-preview-fullscreen");
    await expect(toggle).toHaveAccessibleName("Full screen");
    await expect(toggle).toHaveAttribute("aria-pressed", "false");
    const plain = (await dialog().boundingBox())!;
    expect(plain.x).toBeGreaterThan(0);

    await toggle.click();
    await expect(dialog()).toHaveAttribute("data-fullscreen", "true");
    await expect(toggle).toHaveAccessibleName("Exit full screen");
    await expect(toggle).toHaveAttribute("aria-pressed", "true");
    await settle();
    const full = (await dialog().boundingBox())!;
    expect(Math.round(full.x)).toBe(0);
    expect(Math.round(full.y)).toBe(0);
    expect(Math.round(full.width)).toBe(390);
    expect(Math.round(full.height)).toBe(700);
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
    // The frame grew with it and the report is still there.
    await expect(inner().locator("h1")).toHaveText("Quarterly report");
    const bigFrame = (await page.getByTestId("preview-html").boundingBox())!;
    expect(bigFrame.height).toBeGreaterThan(plain.height - 200);
    expect(bigFrame.x + bigFrame.width).toBeLessThanOrEqual(390 + 0.5);

    // Escape leaves full screen and keeps the dialog.
    await page.keyboard.press("Escape");
    await expect(dialog()).toHaveAttribute("data-fullscreen", "false");
    await expect(dialog()).toBeVisible();
    await settle();
    const restored = (await dialog().boundingBox())!;
    expect(Math.round(restored.x)).toBe(Math.round(plain.x));
    expect(Math.round(restored.width)).toBe(Math.round(plain.width));

    // Toggle in and out with the button, then Escape closes.
    await toggle.click();
    await expect(dialog()).toHaveAttribute("data-fullscreen", "true");
    await toggle.click();
    await expect(dialog()).toHaveAttribute("data-fullscreen", "false");
    await page.keyboard.press("Escape");
    await expect(dialog()).toHaveCount(0);
  });

  test("full screen on a desktop sized window covers it and the dialog closes with the close button", async () => {
    await page.setViewportSize({ width: 1100, height: 760 });
    await openReport();
    await settle();
    await page.getByTestId("file-preview-fullscreen").click();
    await settle();
    const full = (await dialog().boundingBox())!;
    expect([Math.round(full.x), Math.round(full.y), Math.round(full.width), Math.round(full.height)]).toEqual([0, 0, 1100, 760]);
    await dialog().getByRole("button", { name: "Close" }).click();
    await expect(dialog()).toHaveCount(0);
    // A new open is the plain dialog again.
    await openReport();
    await expect(dialog()).toHaveAttribute("data-fullscreen", "false");
  });
});
