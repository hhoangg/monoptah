// S-08: each library of the stub app under the exact CSP and Trusted Types policy. Every test records
// the violations it saw (evidence under /tmp/evidence/ws-q/browser/s08) so the FINDINGS table is data.
import { expect, test, type Page } from "@playwright/test";
import { readdirSync, readFileSync } from "node:fs";
import { collect, record, summarize, server } from "./helpers";

type Lib = "markdown" | "shiki" | "query" | "motion" | "sonner" | "vaul" | "router" | "workbox";

async function drive(page: Page, lib: Lib): Promise<Record<string, unknown>> {
  const facts: Record<string, unknown> = {};
  switch (lib) {
    case "shiki":
      await expect(page.getByTestId("shiki").locator("span").first()).toBeVisible();
      facts["spans"] = await page.getByTestId("shiki").locator("span").count();
      facts["firstSpanColor"] = await page.getByTestId("shiki").locator("span").first().evaluate((e) => getComputedStyle(e).color);
      expect(facts["firstSpanColor"]).toMatch(/^rgb\(\d+, \d+, \d+\)$/);
      expect(facts["firstSpanColor"]).not.toBe(await page.getByTestId("shiki").evaluate((e) => getComputedStyle(e).color));
      break;
    case "markdown": {
      const md = page.getByTestId("markdown");
      await expect(md).toContainText("<img src=x onerror=alert(1)>");
      expect(await md.locator("img, b").count()).toBe(0);
      expect(await md.locator("a").count()).toBe(1);
      expect(await md.locator("a").getAttribute("href")).toBe("https://example.com/");
      break;
    }
    case "query":
      await expect(page.getByTestId("query")).toHaveText("query:world");
      break;
    case "motion": {
      const el = page.getByTestId("motion");
      await expect.poll(() => el.evaluate((e) => getComputedStyle(e).opacity)).toBe("1");
      facts["transform"] = await el.evaluate((e) => getComputedStyle(e).transform);
      facts["inlineStyleAttr"] = await el.evaluate((e) => e.getAttribute("style"));
      expect(facts["transform"]).not.toBe("none");
      break;
    }
    case "sonner": {
      await page.getByTestId("toast-btn").click();
      const toaster = page.locator("[data-sonner-toaster]");
      await expect(page.getByText("Hello toast")).toBeVisible();
      facts["toasterPosition"] = await toaster.evaluate((e) => getComputedStyle(e).position);
      break;
    }
    case "vaul": {
      await page.getByTestId("drawer-open").click();
      const drawer = page.getByTestId("drawer");
      await expect(drawer).toBeVisible();
      facts["touchAction"] = await drawer.evaluate((e) => getComputedStyle(e).touchAction);
      // close with the button, reopen, close by dragging down
      await page.getByTestId("drawer-close").click();
      await expect(drawer).toBeHidden();
      await page.getByTestId("drawer-open").click();
      await expect(drawer).toBeVisible();
      await page.waitForTimeout(700);
      const box = (await drawer.boundingBox())!;
      const x = box.x + box.width / 2;
      // Slow drag that stays inside the viewport (Firefox ignores a fast drag that leaves it, with or without CSP).
      await page.mouse.move(x, box.y + 30);
      await page.mouse.down();
      for (let i = 1; i <= 20; i++) { await page.mouse.move(x, box.y + 30 + i * 13); await page.waitForTimeout(20); }
      await page.mouse.up();
      await expect(drawer).toBeHidden({ timeout: 5000 });
      facts["draggedClosed"] = true;
      break;
    }
    case "router":
      await page.getByTestId("to-about").click();
      await expect(page.getByTestId("about")).toBeVisible();
      expect(new URL(page.url()).pathname).toBe("/about");
      break;
    case "workbox": {
      await page.getByTestId("sw-btn").click();
      await expect(page.getByTestId("sw-state")).toHaveText(/registered|activated/, { timeout: 15000 });
      const reg = await page.evaluate(async () => {
        const r = await navigator.serviceWorker.ready;
        return { scope: r.scope, script: (r.active ?? r.installing ?? r.waiting)?.scriptURL ?? null };
      });
      facts["registration"] = reg;
      expect(reg.script).toMatch(/\/sw\.js$/);
      break;
    }
  }
  return facts;
}

const LIBS: Lib[] = ["markdown", "shiki", "query", "motion", "sonner", "vaul", "workbox", "router"];

for (const lib of LIBS) {
  test(`patched build, exact CSP: ${lib} has no violation`, async ({ page, browserName }, info) => {
    const get = await collect(page);
    await page.goto(`${server(browserName, "exact")}/`);
    const facts = await drive(page, lib);
    const c = await get();
    record(info, `exact-${lib}`, { lib, facts, ...c });
    expect(summarize(c)).toEqual([]);
    expect(c.console).toEqual([]);
  });
}

test("patched build, exact CSP: every library in one session, no violation", async ({ page, browserName }, info) => {
  const get = await collect(page);
  await page.goto(`${server(browserName, "exact")}/`);
  for (const lib of LIBS) await drive(page, lib);
  const c = await get();
  record(info, "exact-all", c);
  expect(summarize(c)).toEqual([]);
  expect(c.console).toEqual([]);
});

// Variant without style-src-attr: the served header is asserted, every library is driven with zero
// violations, and the positive control below proves the directive is what would block style attributes.
for (const lib of LIBS) {
  test(`no style-src-attr variant: ${lib} works with zero violations`, async ({ page, browserName }, info) => {
    const get = await collect(page);
    const resp = await page.goto(`${server(browserName, "noStyleAttr")}/`);
    const csp = resp!.headers()["content-security-policy"]!;
    expect(csp).not.toContain("style-src-attr");
    expect(csp).not.toContain("unsafe-inline");
    expect(csp).toContain("style-src 'self'");
    const facts = await drive(page, lib);
    const c = await get();
    record(info, `no-style-attr-${lib}`, { lib, facts, ...c });
    expect(summarize(c)).toEqual([]);
    expect(c.console).toEqual([]);
  });
}

test("positive control: HTML and script-set style attributes are blocked without style-src-attr and allowed with it", async ({ page, browserName }, info) => {
  const red = "rgb(255, 0, 0)";
  const outcome: Record<string, unknown> = {};
  for (const variant of ["exact", "noStyleAttr"] as const) {
    const fresh = await page.context().newPage();
    const get = await collect(fresh);
    const resp = await fresh.goto(`${server(browserName, variant)}/__probe-style.html`);
    expect(resp!.headers()["content-security-policy"]!.includes("style-src-attr 'unsafe-inline'")).toBe(variant === "exact");
    await fresh.waitForTimeout(300);
    const colors = await fresh.evaluate(() => ({
      parsed: getComputedStyle(document.getElementById("parsed")!).color,
      scripted: getComputedStyle(document.getElementById("scripted")!).color,
    }));
    const c = await get();
    outcome[variant] = { colors, violations: summarize(c) };
    if (variant === "exact") {
      expect(colors).toEqual({ parsed: red, scripted: red });
      expect(c.violations).toEqual([]);
    } else {
      expect(colors.parsed).not.toBe(red);
      expect(colors.scripted).not.toBe(red);
      expect(c.violations.length).toBeGreaterThanOrEqual(2);
      expect(c.violations.every((v) => v.directive.startsWith("style-src"))).toBe(true);
    }
    await fresh.close();
  }
  record(info, "style-attr-control", outcome);
});

// Unpatched build: the patches are necessary (violation at load, library visibly broken).
for (const lib of LIBS) {
  test(`raw unpatched build: ${lib} page has the runtime <style> violation`, async ({ page, browserName }, info) => {
    const get = await collect(page);
    await page.goto(`${server(browserName, "raw")}/`);
    if (lib === "sonner") {
      await page.getByTestId("toast-btn").click();
      await expect(page.getByText("Hello toast")).toBeVisible();
      expect(await page.locator("[data-sonner-toaster]").evaluate((e) => getComputedStyle(e).position)).toBe("static");
    }
    if (lib === "vaul") {
      await page.getByTestId("drawer-open").click();
      await expect(page.getByTestId("drawer")).toBeVisible();
      expect(await page.getByTestId("drawer").evaluate((e) => getComputedStyle(e).touchAction)).toBe("auto");
    }
    const c = await get();
    record(info, `raw-unpatched-${lib}`, { lib, violations: summarize(c), console: c.console });
    expect(c.violations.some((v) => v.directive === "style-src-elem" && v.blocked === "inline")).toBe(true);
  });
}

test("build output: patched assets never inject a <style> element at runtime, the raw build does", () => {
  const scan = (dir: string) => readdirSync(dir).filter((f) => f.endsWith(".js")).map((f) => readFileSync(`${dir}/${f}`, "utf8"));
  const root = new URL("../app/dist/", import.meta.url).pathname;
  // Same pattern as RUNTIME_STYLE in app/vite.config.ts (library-style runtime injection; React DOM's unused path excluded).
  const re = /createElement\(["'`]style["'`]\)\s*[;,]\s*\w+\.type\s*=\s*["'`]text\/css/;
  expect(scan(`${root}patched/assets`).some((s) => re.test(s))).toBe(false);
  expect(scan(`${root}raw/assets`).some((s) => re.test(s))).toBe(true);
});

test("Trusted Types default policy refuses HTML and script sinks", async ({ page, browserName }, info) => {
  const get = await collect(page);
  await page.goto(`${server(browserName, "exact")}/`);
  await expect(page.getByTestId("shiki")).toBeVisible();
  const r = await page.evaluate(() => {
    const out: Record<string, string> = { trustedTypesApi: String(typeof window.trustedTypes !== "undefined") };
    const attempt = (name: string, fn: () => void) => { try { fn(); out[name] = "allowed"; } catch (e) { out[name] = `${(e as Error).name}: ${(e as Error).message}`.slice(0, 80); } };
    attempt("setHTMLUnsafe", () => { document.createElement("div").setHTMLUnsafe("<b>x</b>"); });
    attempt("scriptSrcOffOrigin", () => { document.createElement("script").src = "https://example.com/x.js"; });
    attempt("scriptSrcAppAsset", () => { document.createElement("script").src = "/assets/nope.js"; });
    return out;
  });
  const c = await get();
  record(info, "tt-sinks", { r, ...c });
  expect(r["trustedTypesApi"]).toBe("true");
  expect(r["setHTMLUnsafe"]).toContain("HTML sinks are disabled");
  expect(r["scriptSrcOffOrigin"]).toContain("Blocked script URL");
  expect(r["scriptSrcAppAsset"]).toBe("allowed");
});

test("code-string sinks (eval, new Function, setTimeout string) are refused for page scripts", async ({ page, browserName }, info) => {
  const get = await collect(page);
  await page.goto(`${server(browserName, "exact")}/__probe.html`);
  const probe = await page.evaluate(() => (window as unknown as { __probe: Record<string, string> }).__probe);
  const c = await get();
  record(info, "code-string-sinks", { probe, ...c });
  const sinks = ["eval", "new Function", "setTimeout string", "innerHTML", "insertAdjacentHTML", "createContextualFragment",
    "DOMParser.parseFromString", "script.textContent", "script.src offsite", "iframe.srcdoc", "new Worker offsite"];
  expect(Object.keys(probe).sort()).toEqual([...sinks].sort());
  for (const s of sinks) expect(probe[s], s).not.toBe("allowed");
});

test("upgrade-insecure-requests on http://localhost (recorded)", async ({ page, browserName }, info) => {
  const get = await collect(page);
  const failed: string[] = [];
  page.on("requestfailed", (rq) => failed.push(`${rq.url()} ${rq.failure()?.errorText ?? ""}`));
  const urls: string[] = [];
  page.on("request", (rq) => urls.push(rq.url()));
  await page.goto("http://localhost:4180/").catch(() => undefined);
  await page.waitForTimeout(2000);
  const c = await get().catch(() => ({ violations: [], console: ["page not scriptable"] }));
  const upgraded = urls.some((u) => u.startsWith("https://localhost"));
  record(info, "uir-localhost", { upgraded, urls, failed, ...c });
  // Chromium and Firefox exempt localhost; WebKit upgrades subresources and the load breaks over plain http.
  expect(upgraded).toBe(browserName === "webkit");
});

test.describe("exact CSP over https (self-signed, the production shape)", () => {
  test.use({ ignoreHTTPSErrors: true });
  test("every library except the service worker, upgrade-insecure-requests active, no violation", async ({ page, browserName }, info) => {
    const get = await collect(page);
    await page.goto("https://localhost:4186/");
    for (const lib of LIBS.filter((l) => l !== "workbox")) await drive(page, lib);
    const c = await get();
    record(info, "https-exact-all", { ...c, browserName });
    expect(summarize(c)).toEqual([]);
    expect(c.console).toEqual([]);
  });
});

test("control: no CSP at all, every library works (isolates browser quirks from CSP effects)", async ({ page }, info) => {
  const get = await collect(page);
  await page.goto("http://localhost:4187/");
  for (const lib of LIBS.filter((l) => l !== "workbox")) await drive(page, lib);
  const c = await get();
  record(info, "control-no-csp", c);
  expect(c.violations).toEqual([]);
});
