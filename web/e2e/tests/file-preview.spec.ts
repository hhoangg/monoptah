// File mentions in chat open an in-app preview on the web app through `chat.mentionedFile`. The desktop is the real
// core code behind `web_link_headless serve`, seeded with a folder (env WEB_LINK_HEADLESS_MENTION_DIR) and one
// assistant message that mentions the files (WEB_LINK_HEADLESS_MENTION_TEXT). One account, one paired desktop and one
// open session for all tests (serial); every test opens a mention, checks the dialog and closes it.
//
// Set FILE_PREVIEW_SCREENSHOTS=<dir> to also save a full-page screenshot per preview kind there.
import { deflateSync } from "node:zlib";
import { mkdirSync, mkdtempSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { Page } from "@playwright/test";
import { E2E_CATALOG } from "../lib/catalog.ts";
import { clearRateLimits, namedInvite } from "../lib/env.ts";
import { test, expect } from "../lib/fixtures.ts";
import { probePair } from "../lib/probes.ts";
import { HeadlessDesktop } from "../lib/r12.ts";
import { startPairing, type PendingPairing } from "../lib/w14-headless.ts";
import { registerThroughUi } from "../lib/w14-ui.ts";

const HANDLE = "fpreview";
const MACHINE = "preview-headless";

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
  header[8] = 8; // bit depth
  header[9] = 2; // RGB
  const row = Buffer.concat([Buffer.from([0]), Buffer.concat(Array.from({ length: size }, () => Buffer.from([200, 0, 0])))]);
  const raw = Buffer.concat(Array.from({ length: size }, () => row));
  return Buffer.concat([
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    chunk("IHDR", header),
    chunk("IDAT", deflateSync(raw)),
    chunk("IEND", Buffer.alloc(0)),
  ]);
}

const HTML = `<!doctype html>
<html><head><title>Report</title>
<style>h1 { color: rgb(200, 0, 0); }</style>
</head><body>
<h1 id="title">Quarterly report</h1>
<img id="chart" src="img/chart.png" alt="chart" onerror="document.body.setAttribute('data-onerror-ran','1')">
<script>document.body.setAttribute('data-script-ran','1');document.title='script ran';</script>
</body></html>
`;

const SVG = `<svg xmlns="http://www.w3.org/2000/svg" width="64" height="48" viewBox="0 0 64 48">
<script>document.title = "svg script ran";</script>
<rect width="64" height="48" fill="#0a7"/>
<circle cx="32" cy="24" r="12" fill="#fff"/>
</svg>
`;

const CODE = Array.from({ length: 24 }, (_, i) => `export const value${i + 1} = ${i + 1};`).join("\n") + "\n";

test.describe("file mention previews on the web app", () => {
  test.describe.configure({ mode: "serial" });
  // Skips the whole describe (every test needs the paired headless desktop) when the binary is missing.
  const probe = probePair();
  if (!probe.run) console.log(`skip reason: ${probe.reason}`);
  test.skip(!probe.run, probe.reason);

  let fixtureDir = "";
  /** A Mono project folder holding the reports a script wrote: a root for relative mentions, not a project of the device. */
  let rootDir = "";
  let legacyFlag = "";
  /** The device grants of one desktop of the account (for the second, old desktop). */
  let grantsOf: (desktopId: string) => Promise<{ deviceId: string; state: string }[]>;
  let oldDesktop: HeadlessDesktop | null = null;
  let oldPending: PendingPairing | null = null;
  let pending: PendingPairing | null = null;
  let desktop: HeadlessDesktop | null = null;
  let page: Page;
  let canonicalDir = "";

  test.beforeAll(clearRateLimits);

  test.afterAll(() => {
    desktop?.kill();
    oldDesktop?.kill();
    pending?.dispose();
    oldPending?.dispose();
    if (fixtureDir) rmSync(fixtureDir, { recursive: true, force: true });
    if (rootDir) rmSync(rootDir, { recursive: true, force: true });
  });

  /** Waits until every finite animation of the page (the dialog fades in) has ended and two frames were painted. */
  async function settle(): Promise<void> {
    await page.evaluate(async () => {
      const finite = () =>
        document.getAnimations().filter((animation) => Number.isFinite(animation.effect?.getComputedTiming().endTime ?? 0));
      while (finite().some((animation) => animation.playState === "running")) {
        await Promise.allSettled(finite().map((animation) => animation.finished));
      }
      await new Promise<void>((resolve) => requestAnimationFrame(() => requestAnimationFrame(() => resolve())));
    });
  }

  /** Clicks once, after the element's bounding box was the same on two polls in a row (the list stopped moving). */
  async function clickWhenStable(target: ReturnType<Page["locator"]>): Promise<void> {
    let previous = "";
    await expect
      .poll(
        async () => {
          const box = await target.boundingBox();
          const current = JSON.stringify(box);
          const stable = box !== null && current === previous;
          previous = current;
          return stable;
        },
        { timeout: 15_000, intervals: [150] },
      )
      .toBe(true);
    await target.click();
  }

  const shotDir = process.env["FILE_PREVIEW_SCREENSHOTS"];
  async function shot(name: string): Promise<void> {
    if (!shotDir) return;
    await settle();
    mkdirSync(shotDir, { recursive: true });
    await page.screenshot({ path: join(shotDir, `${name}.png`), fullPage: true });
  }

  const mention = (file: string) => page.locator(`button[data-testid="file-mention"][data-path$="${file}"]`).first();
  const dialog = () => page.getByTestId("file-preview-dialog");

  async function open(file: string): Promise<void> {
    await mention(file).click();
    await expect(dialog()).toBeVisible();
  }

  test.afterEach(async () => {
    if (!page || page.isClosed()) return;
    if (await dialog().isVisible()) {
      await page.keyboard.press("Escape");
      await expect(dialog()).toHaveCount(0);
    }
  });

  test("setup: register, pair, serve the seeded folder, open the session", async ({ newSharedActor }) => {
    test.setTimeout(240_000);
    fixtureDir = mkdtempSync(join(tmpdir(), "monoptah-e2e-preview-"));
    mkdirSync(join(fixtureDir, "img"));
    writeFileSync(join(fixtureDir, "report.html"), HTML);
    writeFileSync(join(fixtureDir, "img", "chart.png"), tinyPng());
    writeFileSync(join(fixtureDir, "notes.md"), "# Notes\n\nA chart:\n\n![chart](img/chart.png)\n");
    writeFileSync(join(fixtureDir, "diagram.svg"), SVG);
    writeFileSync(join(fixtureDir, "code.ts"), CODE);
    writeFileSync(join(fixtureDir, "data.bin"), Buffer.from(Array.from({ length: 2048 }, (_, i) => (i % 7 === 0 ? 0 : (i * 31) % 256))));
    writeFileSync(join(fixtureDir, ".env"), "FAKE=not-a-secret\n");
    writeFileSync(join(fixtureDir, "unmentioned-secret.txt"), "not mentioned anywhere\n");
    canonicalDir = realpathSync(fixtureDir);
    // Two reports a script wrote, outside the selected project: no Write or Edit tool ever touched them.
    rootDir = mkdtempSync(join(tmpdir(), "monoptah-e2e-preview-root-"));
    mkdirSync(join(rootDir, "casium", "reports"), { recursive: true });
    for (const name of ["made-by-script", "made-twice"]) {
      writeFileSync(join(rootDir, "casium", "reports", `${name}.html`), `<!doctype html><html><body><h1 id="title">Report ${name}</h1></body></html>\n`);
    }
    legacyFlag = join(rootDir, "legacy-desktop.flag");

    const text = [
      "Here are the files:",
      "`{dir}/report.html`, `{dir}/img/chart.png`, `{dir}/notes.md`, `{dir}/diagram.svg`,",
      "`{dir}/code.ts:12`, `{dir}/data.bin` and `{dir}/.env`.",
    ].join(" ") + [
      "",
      "",
      "| Report | File |",
      "|---|---|",
      "| Made by a script | [made-by-script.html](casium/reports/made-by-script.html) |",
      "| Made twice | `casium/reports/made-twice.html` |",
    ].join("\n");

    const actor = await newSharedActor();
    page = actor.page;
    grantsOf = async (id) => (await actor.call("GET", `/api/desktops/${id}/grants`)).json["grants"] as { deviceId: string; state: string }[];
    await registerThroughUi(page, namedInvite("filepreview"), HANDLE, "File Preview");

    pending = await startPairing(MACHINE);
    const { userCode, fingerprint } = pending.started;
    await page.goto(`/pair/${userCode}#fp=${fingerprint}`);
    await page.getByRole("button", { name: "Approve" }).click();
    await expect(page.getByRole("heading", { name: "Desktop approved" })).toBeVisible();
    expect(await pending.process.exited, pending.process.stderr()).toBe(0);

    // The mention seeding is read from the environment of the `serve` process.
    process.env["WEB_LINK_HEADLESS_MENTION_DIR"] = fixtureDir;
    process.env["WEB_LINK_HEADLESS_MENTION_TEXT"] = text;
    // No structured tool row at all: the reports are named only by the command that wrote them.
    process.env["WEB_LINK_HEADLESS_MENTION_TOOL_TEXT"] = "";
    process.env["WEB_LINK_HEADLESS_MENTION_ROOT"] = rootDir;
    process.env["WEB_LINK_HEADLESS_MENTION_BASH_TEXT"] = "python3 gen.py > casium/reports/made-by-script.html && cp casium/reports/made-by-script.html casium/reports/made-twice.html";
    process.env["WEB_LINK_HEADLESS_MENTION_LEGACY_FLAG"] = legacyFlag;
    try {
      desktop = new HeadlessDesktop(["serve", "--data-dir", join(pending.dataDir, "data")]);
    } finally {
      delete process.env["WEB_LINK_HEADLESS_MENTION_DIR"];
      delete process.env["WEB_LINK_HEADLESS_MENTION_TEXT"];
      delete process.env["WEB_LINK_HEADLESS_MENTION_TOOL_TEXT"];
      delete process.env["WEB_LINK_HEADLESS_MENTION_ROOT"];
      delete process.env["WEB_LINK_HEADLESS_MENTION_BASH_TEXT"];
      delete process.env["WEB_LINK_HEADLESS_MENTION_LEGACY_FLAG"];
    }
    const online = await desktop.waitFor("online", {}, 60_000);
    const desktopId = String(online["desktopId"]);
    await page.evaluate((id) => window.localStorage.setItem(`monoptah.pickerSeen.${id}`, "1"), desktopId);

    await page.goto("/desktops");
    await page.getByRole("link", { name: `Open ${MACHINE}` }).click();
    await expect(page).toHaveURL(new RegExp(`/d/${desktopId}$`));
    const pendingScreen = page.getByRole("alertdialog", { name: "Confirm this device on your desktop" });
    await expect(pendingScreen).toBeVisible({ timeout: 30_000 });
    let deviceId = "";
    await expect
      .poll(async () => {
        const grants = await grantsOf(desktopId);
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
    // The headless desktop authorizes paths from the seeded message but its conversation.read does not show it, so
    // the same text is sent as a user message: the fake desktop echoes it as an assistant turn, which renders the
    // chips. The echo itself authorizes nothing; the seeded message does.
    await page.getByRole("textbox", { name: "Message" }).fill(text.replaceAll("{dir}", canonicalDir));
    await page.getByRole("button", { name: "Send", exact: true }).click();
    await expect(mention("report.html")).toBeVisible({ timeout: 30_000 });
    expect(canonicalDir.length).toBeGreaterThan(0);
  });

  test("html opens in a sandboxed blob iframe with its style and image, and no script ran", async () => {
    await open("report.html");
    await expect(page.getByTestId("file-preview-name")).toHaveText("report.html");
    const frame = page.getByTestId("preview-html");
    await expect(frame).toBeVisible();
    expect(await frame.getAttribute("sandbox")).toBe("");
    expect(await frame.getAttribute("src")).toMatch(/^blob:/);

    const inner = page.frameLocator('[data-testid="preview-html"]');
    await expect(inner.locator("h1")).toHaveText("Quarterly report");
    // The <style> rule reached the document as an inline style and applies.
    expect(await inner.locator("h1").evaluate((el) => getComputedStyle(el).color)).toBe("rgb(200, 0, 0)");
    // The asset was inlined and decodes.
    await expect.poll(() => inner.locator("img").evaluate((el) => (el as HTMLImageElement).naturalWidth)).toBeGreaterThan(0);
    // Neither the script nor the onerror handler ran.
    expect(await inner.locator("body").getAttribute("data-script-ran")).toBeNull();
    expect(await inner.locator("body").getAttribute("data-onerror-ran")).toBeNull();
    expect(await inner.locator("script").count()).toBe(0);
    await shot("web-html");
  });

  test("markdown renders its relative image", async () => {
    await open("notes.md");
    const md = page.getByTestId("preview-markdown");
    await expect(md).toContainText("A chart:");
    const image = page.getByTestId("preview-md-image");
    await expect(image).toBeVisible();
    expect(await image.getAttribute("src")).toMatch(/^data:image\/png/);
    await expect.poll(() => image.evaluate((el) => (el as HTMLImageElement).naturalWidth)).toBeGreaterThan(0);
    await shot("web-markdown");
  });

  test("png opens in the image viewer and zoom in changes the zoom", async () => {
    await open("chart.png");
    const image = page.getByTestId("preview-image");
    await expect(image).toBeVisible();
    expect(await image.evaluate((el) => el.tagName)).toBe("IMG");
    expect(await image.getAttribute("src")).toMatch(/^data:image\/png/);
    await expect.poll(() => image.evaluate((el) => (el as HTMLImageElement).naturalWidth)).toBe(8);
    await expect(image).toHaveAttribute("data-zoom", "fit");
    await page.getByTestId("preview-zoom-in").click();
    await expect(image).not.toHaveAttribute("data-zoom", "fit");
    const zoomed = Number(await image.getAttribute("data-zoom"));
    expect(zoomed).toBeGreaterThan(0);
    await shot("web-image");
  });

  test("svg opens in the image viewer as an img and its script does not run", async () => {
    await open("diagram.svg");
    const image = page.getByTestId("preview-image");
    await expect(image).toBeVisible();
    expect(await image.evaluate((el) => el.tagName)).toBe("IMG");
    expect(await image.getAttribute("src")).toMatch(/^data:image\/svg\+xml/);
    await expect.poll(() => image.evaluate((el) => (el as HTMLImageElement).naturalWidth)).toBeGreaterThan(0);
    await expect(image).toHaveAttribute("data-zoom", "fit");
    await page.getByTestId("preview-zoom-in").click();
    await expect(image).not.toHaveAttribute("data-zoom", "fit");
    expect(await page.title()).not.toBe("svg script ran");
    await shot("web-svg");
  });

  test("code.ts:12 opens as text with line 12 highlighted", async () => {
    await open("code.ts");
    await expect(page.getByTestId("preview-lines")).toBeVisible();
    // The syntax highlighter loads on demand: wait for colored tokens before looking at (or capturing) the lines.
    await expect(page.getByTestId("preview-lines").locator(".md-token").first()).toBeVisible();
    const highlighted = page.getByTestId("preview-lines").locator('[data-testid="file-line"][data-highlighted="true"]');
    await expect(highlighted).toHaveCount(1);
    await expect(highlighted).toContainText("value12");
    await expect(highlighted).toHaveAttribute("data-index", "11");
    await shot("web-code");
  });

  test("a binary file shows that it cannot be previewed, with its size and path", async () => {
    await open("data.bin");
    const binary = page.getByTestId("preview-binary");
    await expect(binary).toContainText("Cannot preview this file");
    await expect(binary).toContainText("2 KB - ");
    await expect(binary).toContainText(join(canonicalDir, "data.bin"));
    await shot("web-binary");
  });

  test(".env shows the denied message", async () => {
    await open(".env");
    const error = page.getByTestId("file-preview-error");
    await expect(error).toBeVisible();
    await expect(error).toHaveAttribute("data-problem", "denied");
    await expect(error).toContainText("This file is hidden by Monoptah");
    await expect(page.getByText("FAKE=not-a-secret")).toHaveCount(0);
    await shot("web-denied");
  });

  test("a relative link in a table, to a report a script wrote outside the project, opens", async () => {
    await open("made-by-script.html");
    await expect(page.getByTestId("file-preview-name")).toHaveText("made-by-script.html");
    const inner = page.frameLocator('[data-testid="preview-html"]');
    await expect(inner.locator("h1")).toHaveText("Report made-by-script");
    await shot("web-script-report");
  });

  test("a desktop that does not know the method shows the update notice, not a mention error", async () => {
    // An older desktop: the flag file makes the headless desktop answer `unsupported`, like one without the method.
    writeFileSync(legacyFlag, "");
    try {
      await open("made-twice.html");
      const error = page.getByTestId("file-preview-error");
      await expect(error).toBeVisible();
      await expect(error).toHaveAttribute("data-problem", "update_desktop");
      await expect(error).toContainText("Update Monoptah on your desktop");
    } finally {
      rmSync(legacyFlag, { force: true });
    }
  });

  test("a file no assistant message mentions is refused as not mentioned", async () => {
    // A user message does not authorize a path: the echo turn that quotes it is an assistant message of the fake
    // desktop, which the real mention check ignores, so the chip shows up but the desktop answers `forbidden`.
    const unmentioned = join(canonicalDir, "unmentioned-secret.txt");
    await page.getByRole("textbox", { name: "Message" }).fill(`please read \`${unmentioned}\``);
    await page.getByRole("button", { name: "Send", exact: true }).click();
    const echoed = page.locator(`button[data-testid="file-mention"][data-path$="unmentioned-secret.txt"]`).last();
    await expect(echoed).toBeVisible({ timeout: 30_000 });
    await expect(page.getByRole("button", { name: "Send", exact: true })).toBeVisible();
    // One click, once the chip stopped moving (the transcript scrolls to the bottom while the echo streams in).
    await clickWhenStable(echoed);
    await expect(dialog()).toBeVisible();
    // It stays open: nothing closes it behind the test's back.
    await page.waitForTimeout(750);
    await expect(dialog()).toBeVisible();
    const error = page.getByTestId("file-preview-error");
    await expect(error).toBeVisible();
    await expect(error).toHaveAttribute("data-problem", "not_mentioned");
    await expect(page.getByText("not mentioned anywhere")).toHaveCount(0);
  });

  test("a 0.54.1 desktop (answers forbidden, no mentionedFiles2) shows the update notice, not a mention error", async () => {
    test.setTimeout(240_000);
    // A second desktop of the same account that behaves like release 0.54.1: only structured evidence counts, so the
    // script's report is refused with `forbidden`, and it does not advertise `mentionedFiles2`.
    oldPending = await startPairing("old-headless");
    const { userCode, fingerprint } = oldPending.started;
    await page.goto(`/pair/${userCode}#fp=${fingerprint}`);
    await page.getByRole("button", { name: "Approve" }).click();
    await expect(page.getByRole("heading", { name: "Desktop approved" })).toBeVisible();
    expect(await oldPending.process.exited, oldPending.process.stderr()).toBe(0);

    const text = ["Reports:", "", "| Report | File |", "|---|---|", "| Made by a script | [made-by-script.html](casium/reports/made-by-script.html) |"].join("\n");
    const env = {
      WEB_LINK_HEADLESS_MENTION_DIR: fixtureDir,
      WEB_LINK_HEADLESS_MENTION_TEXT: text,
      WEB_LINK_HEADLESS_MENTION_TOOL_TEXT: "",
      WEB_LINK_HEADLESS_MENTION_ROOT: rootDir,
      WEB_LINK_HEADLESS_MENTION_BASH_TEXT: "python3 gen.py > casium/reports/made-by-script.html",
      WEB_LINK_HEADLESS_MENTION_OLD_RULES: "1",
      WEB_LINK_HEADLESS_HIDE_CAPABILITIES: "mentionedFiles2",
    };
    Object.assign(process.env, env);
    try {
      oldDesktop = new HeadlessDesktop(["serve", "--data-dir", join(oldPending.dataDir, "data")]);
    } finally {
      for (const name of Object.keys(env)) delete process.env[name];
    }
    const online = await oldDesktop.waitFor("online", {}, 60_000);
    const desktopId = String(online["desktopId"]);
    await page.evaluate((id) => window.localStorage.setItem(`monoptah.pickerSeen.${id}`, "1"), desktopId);
    await page.goto("/desktops");
    await page.getByRole("link", { name: "Open old-headless" }).click();
    await expect(page).toHaveURL(new RegExp(`/d/${desktopId}$`));
    const pendingScreen = page.getByRole("alertdialog", { name: "Confirm this device on your desktop" });
    await expect(pendingScreen).toBeVisible({ timeout: 30_000 });
    let deviceId = "";
    await expect
      .poll(async () => {
        deviceId = (await grantsOf(desktopId)).find((g) => g.state === "pending")?.deviceId ?? "";
        return deviceId !== "";
      })
      .toBe(true);
    const confirmed = oldDesktop;
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
    await page.getByRole("textbox", { name: "Message" }).fill(text);
    await page.getByRole("button", { name: "Send", exact: true }).click();
    await expect(mention("made-by-script.html")).toBeVisible({ timeout: 30_000 });
    await expect(page.getByRole("button", { name: "Send", exact: true })).toBeVisible();
    await clickWhenStable(mention("made-by-script.html"));
    await expect(dialog()).toBeVisible();
    const error = page.getByTestId("file-preview-error");
    await expect(error).toBeVisible();
    await expect(error).toHaveAttribute("data-problem", "update_desktop");
    await expect(error).toContainText("Update Monoptah on your desktop");
    // The desktop answered with the same `forbidden` an unmentioned file gets: only the missing capability tells.
    await expect(page.getByText("outside this conversation")).toHaveCount(0);
  });
});
