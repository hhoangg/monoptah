// Mobile layout regressions of the chat screens (390 px class phones): the transcript must never scroll sideways (wide
// content scrolls inside its own box) and the composer must sit on the bottom edge of the viewport. The desktop is the
// real core code behind `web_link_headless serve`; the fixture is sent as a user message and the fake desktop echoes
// it back as an assistant turn, which renders the markdown. The same conversation is opened as a session and as the
// chat of a Mono (WEB_LINK_HEADLESS_MONO=1), whose messages carry the Message actions button. One account, one paired
// desktop for all tests (serial).
//
// Set MOBILE_LAYOUT_SCREENSHOTS=<dir> to also save screenshots at 390 px there.
import { mkdirSync, mkdtempSync, rmSync } from "node:fs";
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

const HANDLE = "mlayout";
const MACHINE = "layout-headless";
const WIDTHS = [360, 390, 430];
const HEIGHT = 844;

const LONG_URL = `https://example.com/${"segment-with-a-long-name/".repeat(12)}index.html?query=${"abcdef0123456789".repeat(8)}`;
const LONG_PATH = `/Users/someone/workspaces/${"very-long-directory-name-".repeat(8)}/file-name-without-break-points.tsx`;

const FIXTURE = [
  "Chào bạn, đây là bản tóm tắt dài bằng tiếng Việt để kiểm tra việc xuống dòng của khung chat trên điện thoại. Khi một đoạn văn rất dài được hiển thị thì mọi dòng phải nằm gọn trong màn hình và không được cắt ở mép phải.",
  "",
  "1. Bước đầu tiên là kiểm tra lại các khoản thanh toán của khách hàng trong tháng này và so sánh với bảng đối soát của ngân hàng",
  "   - Mục lồng nhau thứ nhất với nội dung khá dài để thử độ thụt lề của danh sách trong khung chat",
  "     - Mục lồng nhau cấp ba với nội dung dài hơn nữa, vẫn phải xuống dòng bình thường trong khung chat",
  "       - Mục cấp bốn cũng phải nằm gọn trong màn hình điện thoại cho dù đã thụt lề rất sâu",
  "2. Bước thứ hai là gửi báo cáo cho bộ phận kế toán",
  "",
  "> Trích dẫn dài: đây là một đoạn trích dẫn có nội dung dài để kiểm tra phần thụt lề của khối trích dẫn trong khung chat.",
  "> > Trích dẫn lồng nhau bên trong khối trích dẫn thứ nhất, cũng có nội dung khá dài.",
  "",
  `Link dài: ${LONG_URL}`,
  "",
  `Đường dẫn dài: ${LONG_PATH}`,
  "",
  "```ts",
  "export const wideLine = 'this line of code is intentionally very wide so that the code block has to scroll sideways inside its own box ........................................';",
  "```",
  "",
  "| Cột một | Cột hai | Cột ba | Cột bốn | Cột năm |",
  "| --- | --- | --- | --- | --- |",
  "| Giá trị dài của ô thứ nhất | Giá trị dài của ô thứ hai | Giá trị dài của ô thứ ba | Giá trị dài của ô thứ tư | Giá trị dài của ô thứ năm |",
  "",
  "## Tiêu đề dài không có khoảng trắng: " + "TieuDeRatDaiKhongCoDauCach".repeat(5),
  "",
  `Mã nội dòng: \`${"some_very_long_identifier_name_".repeat(6)}\` và **${"inđậmrấtdài".repeat(12)}** và [${"liên-kết-dài-".repeat(10)}](https://example.com/a) và ${LONG_PATH}:12`,
  "",
  "- Mục có khối mã bên trong:",
  "",
  "  ```bash",
  "  pnpm --filter @monoptah/app exec vite build --outDir /tmp/some/very/long/output/directory/that/keeps/going --mode production --sourcemap",
  "  ```",
  "",
  "  | a | b | c | d |",
  "  | --- | --- | --- | --- |",
  "  | một giá trị dài | một giá trị dài | một giá trị dài | một giá trị dài |",
  "",
  "- Mục thứ hai có `mã nội dòng rất dài trong danh sách` và đường dẫn " + LONG_PATH,
  "",
  "> - Danh sách trong trích dẫn với nội dung dài để kiểm tra phần thụt lề kết hợp của hai lớp",
  "",
  '<div style="width:900px">HTML thô rộng 900 pixel</div>',
  "",
  "---",
  "",
  "Hết bản tóm tắt.",
].join("\n");

test.describe("mobile chat layout", () => {
  test.describe.configure({ mode: "serial" });
  const probe = probePair();
  if (!probe.run) console.log(`skip reason: ${probe.reason}`);
  test.skip(!probe.run, probe.reason);

  let dataRoot = "";
  let pending: PendingPairing | null = null;
  let desktop: HeadlessDesktop | null = null;
  let page: Page;
  let desktopId = "";

  test.beforeAll(clearRateLimits);

  test.afterAll(() => {
    desktop?.kill();
    pending?.dispose();
    if (dataRoot) rmSync(dataRoot, { recursive: true, force: true });
  });

  const shotDir = process.env["MOBILE_LAYOUT_SCREENSHOTS"];
  async function shot(name: string): Promise<void> {
    if (!shotDir) return;
    mkdirSync(shotDir, { recursive: true });
    await page.screenshot({ path: join(shotDir, `${name}.png`) });
  }

  async function resize(width: number): Promise<void> {
    await page.setViewportSize({ width, height: HEIGHT });
    await page.evaluate(() => new Promise<void>((resolve) => requestAnimationFrame(() => requestAnimationFrame(() => resolve()))));
  }

  /**
   * Everything that is wider than where it is shown, measured in the page:
   * - `scrolling`: every element the user can drag sideways (overflow-x `auto` or `scroll`) that has more content than
   *   room (scrollWidth over clientWidth), except the two boxes that are meant to scroll (`.md-code-body`,
   *   `.md-table-wrap`). This covers the transcript scroller, the chat pane, the shell and the document, wherever the
   *   overflow comes from. (`hidden` boxes are not draggable: `truncate` text and `sr-only` links are such on purpose.)
   * - `outside`: every element that sticks out of the viewport, except what sits inside those two boxes or inside a
   *   box of 1px or less (the visually hidden `sr-only` and `size-0` holders).
   */
  async function overflow(): Promise<{ scrolling: string[]; outside: string[]; doc: number[]; body: number[]; scroller: number[] }> {
    return page.evaluate(() => {
      const own = ".md-code-body, .md-table-wrap";
      const name = (el: Element) => `${el.tagName.toLowerCase()}.${String(el.className).slice(0, 90)}`;
      const width = document.documentElement.clientWidth;
      const scrolling: string[] = [];
      const outside: string[] = [];
      for (const el of Array.from(document.querySelectorAll("body *"))) {
        if (el.matches(own)) continue;
        if (/^(auto|scroll)$/.test(getComputedStyle(el).overflowX) && el.scrollWidth > el.clientWidth) {
          scrolling.push(`${name(el)} scrollW=${el.scrollWidth} clientW=${el.clientWidth}`);
        }
        if (el.closest(own)) continue;
        let hidden = false;
        for (let a: Element | null = el; a && a !== document.body; a = a.parentElement) {
          if (a.getBoundingClientRect().width <= 1) hidden = true;
        }
        if (hidden) continue;
        const rect = el.getBoundingClientRect();
        if (rect.width > 0 && rect.right > width + 1) outside.push(`${name(el)} right=${Math.round(rect.right)} w=${Math.round(rect.width)}`);
      }
      const scroller = document.querySelector<HTMLElement>('[data-testid="transcript-scroller"]');
      return {
        scrolling: scrolling.slice(0, 25),
        outside: outside.slice(0, 25),
        doc: [document.documentElement.scrollWidth, document.documentElement.clientWidth],
        body: [document.body.scrollWidth, document.body.clientWidth],
        scroller: scroller ? [scroller.scrollWidth, scroller.clientWidth] : [],
      };
    });
  }

  async function openChat(path: string): Promise<void> {
    await page.goto(`/d/${desktopId}${path}`);
    await expect(page.locator(".agent-markdown .md-table-wrap").first()).toBeVisible({ timeout: 30_000 });
  }

  test("setup: register, pair, serve, open the session and send the fixture", async ({ newSharedActor }) => {
    test.setTimeout(240_000);
    dataRoot = mkdtempSync(join(tmpdir(), "monoptah-e2e-mlayout-"));
    const actor = await newSharedActor();
    page = actor.page;
    await page.setViewportSize({ width: 390, height: HEIGHT });
    await registerThroughUi(page, namedInvite("mlayout"), HANDLE, "Mobile Layout");

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

    await page.goto("/desktops");
    await page.getByRole("link", { name: `Open ${MACHINE}` }).click();
    await expect(page).toHaveURL(new RegExp(`/d/${desktopId}$`));
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
    await page.getByRole("textbox", { name: "Message" }).fill(FIXTURE);
    await page.getByRole("button", { name: "Send", exact: true }).click();
    // The echoed assistant turn renders the markdown: wait for its table (the last block of the fixture).
    await expect(page.locator(".agent-markdown .md-table-wrap").first()).toBeVisible({ timeout: 30_000 });
    await expect(page.getByRole("button", { name: "Send", exact: true })).toBeVisible();
  });

  const CHATS = [
    { kind: "session", path: `/s/${E2E_CATALOG.sessionId}` },
    { kind: "mono", path: "/monos/e2e-mono" },
  ];

  for (const chat of CHATS) {
    test(`${chat.kind}: open the chat`, async () => {
      await openChat(chat.path);
      if (chat.kind === "mono") {
        // The Message actions button of every message is what made the Mono chat scroll sideways.
        await expect(page.getByRole("button", { name: "Message actions" }).first()).toBeAttached();
      }
    });

    for (const width of WIDTHS) {
      test(`${chat.kind} ${width}px: nothing scrolls sideways but the code and table boxes, which scroll in their own box`, async () => {
        await resize(width);
        await page.locator(".agent-markdown .md-table-wrap").first().scrollIntoViewIfNeeded();
        const found = await overflow();
        const hint = JSON.stringify(found, null, 1);
        expect(found.scrolling, `an element scrolls sideways\n${hint}`).toEqual([]);
        expect(found.outside, `an element sticks out of the viewport\n${hint}`).toEqual([]);
        expect(found.doc[0], `document\n${hint}`).toBeLessThanOrEqual(found.doc[1]!);
        expect(found.body[0], `body\n${hint}`).toBeLessThanOrEqual(found.body[1]!);
        expect(found.scroller.length, "transcript scroller exists").toBe(2);
        expect(found.scroller[0], `transcript\n${hint}`).toBeLessThanOrEqual(found.scroller[1]!);
        // The wide blocks stay inside the viewport and scroll in their own box.
        for (const selector of [".md-code-body", ".md-table-wrap"]) {
          const block = await page.locator(selector).first().evaluate((el) => ({ scrollWidth: el.scrollWidth, clientWidth: el.clientWidth, right: el.getBoundingClientRect().right, viewport: window.innerWidth }));
          expect(block.right, `${selector} right edge`).toBeLessThanOrEqual(block.viewport + 1);
          expect(block.scrollWidth, `${selector} scrolls in its own box`).toBeGreaterThan(block.clientWidth);
        }
        if (width === 390) await shot(`${chat.kind}-390-transcript`);
      });

      test(`${chat.kind} ${width}px: the composer pill sits on the bottom edge, only the safe area below it`, async () => {
        await resize(width);
        const region = page.getByTestId("composer-region");
        await expect(region).toBeVisible();
        const m = await region.evaluate((el) => {
          const composer = el.querySelector<HTMLElement>('[data-testid="composer"]')!;
          const pill = composer.querySelector<HTMLElement>("[data-shape]")!;
          const probe = document.createElement("div");
          probe.style.cssText = "position:fixed;visibility:hidden;padding-bottom:env(safe-area-inset-bottom)";
          document.body.append(probe);
          const inset = parseFloat(getComputedStyle(probe).paddingBottom) || 0;
          probe.remove();
          return {
            pillBottom: pill.getBoundingClientRect().bottom,
            innerHeight: window.innerHeight,
            inset,
            regionPadding: parseFloat(getComputedStyle(el).paddingBottom),
            composerPadding: parseFloat(getComputedStyle(composer).paddingBottom),
            composerMargin: parseFloat(getComputedStyle(composer).marginBottom),
            docHeight: document.documentElement.scrollHeight,
          };
        });
        // The gap below the pill is max(safe area, 0.75rem), applied once: in Chromium the safe area is 0, so 12px.
        const gap = Math.max(m.inset, 12);
        expect(Math.abs(m.innerHeight - m.pillBottom - gap)).toBeLessThanOrEqual(1);
        expect(m.composerPadding).toBeCloseTo(gap, 1);
        expect(m.regionPadding).toBe(0);
        expect(m.composerMargin).toBe(0);
        expect(m.docHeight).toBeLessThanOrEqual(m.innerHeight);
        if (width === 390) await shot(`${chat.kind}-390-composer`);
      });
    }
  }

  // Chromium has no on-screen keyboard, so a visual viewport that is shorter than the layout is simulated. It runs
  // last: the init script stays on the page. iOS reports such a viewport after the keyboard closed in a standalone
  // app (and, with browser chrome, even without a keyboard); with no field focused it must not lift the composer,
  // and a visual viewport left panned (offsetTop > 0) must put the document back at the top.
  test("a short visual viewport without a focused field leaves no gap; focusing the field lifts the composer", async () => {
    await resize(390);
    await page.addInitScript(() => {
      const stub = Object.assign(new EventTarget(), { offsetTop: 0, scale: 1, get height() { return window.innerHeight - 240; } });
      Object.defineProperty(window, "visualViewport", { value: stub, configurable: true });
      const w = window as unknown as { __vv: typeof stub; __scrolled: unknown[][] };
      w.__vv = stub;
      w.__scrolled = [];
      window.scrollTo = ((...args: unknown[]) => void w.__scrolled.push(args)) as typeof window.scrollTo;
    });
    await page.reload();
    const pill = page.getByTestId("composer").locator("[data-shape]");
    await expect(pill).toBeVisible({ timeout: 30_000 });
    await page.evaluate(() => (document.activeElement as HTMLElement | null)?.blur());
    const gap = () => pill.evaluate((el) => window.innerHeight - el.getBoundingClientRect().bottom);
    // With nothing focused: the bottom gap is the normal 12px, not lifted by the 240px the viewport is short by.
    await expect.poll(async () => Math.round(await gap())).toBe(12);
    // A panned visual viewport (iOS after the keyboard closed) puts the document back at the top, nothing focused.
    await page.evaluate(() => {
      const w = window as unknown as { __vv: EventTarget & { offsetTop: number }; __scrolled: unknown[][] };
      w.__scrolled.length = 0;
      w.__vv.offsetTop = 40;
      w.__vv.dispatchEvent(new Event("resize"));
    });
    await expect.poll(() => page.evaluate(() => (window as unknown as { __scrolled: unknown[][] }).__scrolled.some((a) => a[0] === 0 && a[1] === 0))).toBe(true);
    // Focusing the field: the composer is lifted by the keyboard (12px + 240px).
    await page.evaluate(() => ((window as unknown as { __vv: { offsetTop: number } }).__vv.offsetTop = 0));
    await page.getByRole("textbox", { name: "Message" }).focus();
    await expect.poll(async () => Math.round(await gap())).toBe(252);
  });
});
