// S-08: each library of the stub app under the exact CSP and Trusted Types policy. Every test records
// the violations it saw (evidence under /tmp/evidence/ws-q/browser/s08) so the FINDINGS table is data.
import { expect, test, type Page } from "@playwright/test";
import { collect, record, summarize, server } from "./helpers";

type Lib = "shiki" | "query" | "motion" | "sonner" | "vaul" | "router" | "workbox";

async function drive(page: Page, lib: Lib): Promise<Record<string, unknown>> {
  const facts: Record<string, unknown> = {};
  switch (lib) {
    case "shiki":
      await expect(page.getByTestId("shiki").locator("span").first()).toBeVisible();
      facts["spans"] = await page.getByTestId("shiki").locator("span").count();
      facts["firstSpanColor"] = await page.getByTestId("shiki").locator("span").first().evaluate((e) => getComputedStyle(e).color);
      break;
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

const LIBS: Lib[] = ["shiki", "query", "motion", "sonner", "vaul", "router", "workbox"];

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
  for (const lib of ["shiki", "query", "motion", "sonner", "vaul", "workbox", "router"] as const) await drive(page, lib);
  const c = await get();
  record(info, "exact-all", c);
  expect(summarize(c)).toEqual([]);
  expect(c.console).toEqual([]);
});

// Variants: observation only. The recorded JSON feeds FINDINGS.md.
for (const [variant, key] of [["no-style-attr", "noStyleAttr"], ["raw-unpatched", "raw"]] as const) {
  for (const lib of LIBS) {
    test(`${variant}: ${lib} (recorded)`, async ({ page, browserName }, info) => {
      const get = await collect(page);
      await page.goto(`${server(browserName, key)}/`);
      let outcome = "functional";
      let facts: Record<string, unknown> = {};
      try { facts = await drive(page, lib); } catch (e) { outcome = `assertion failed: ${(e as Error).message.split("\n")[0]}`; }
      const c = await get();
      record(info, `${variant}-${lib}`, { lib, variant, outcome, facts, violations: summarize(c), console: c.console });
    });
  }
}

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
  if (r["trustedTypesApi"] === "true") {
    expect(r["setHTMLUnsafe"]).toContain("HTML sinks are disabled");
    expect(r["scriptSrcOffOrigin"]).toContain("Blocked script URL");
    expect(r["scriptSrcAppAsset"]).toBe("allowed");
  }
});

test("code-string sinks (eval, new Function, setTimeout string) are refused for page scripts", async ({ page, browserName }, info) => {
  const get = await collect(page);
  await page.goto(`${server(browserName, "exact")}/__probe.html`);
  const probe = await page.evaluate(() => (window as unknown as { __probe: Record<string, string> }).__probe);
  const c = await get();
  record(info, "code-string-sinks", { probe, ...c });
  expect(probe["eval"]).not.toBe("allowed");
  expect(probe["new Function"]).not.toBe("allowed");
  expect(probe["setTimeout string"]).not.toBe("allowed");
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
    for (const lib of ["shiki", "query", "motion", "sonner", "vaul", "router"] as const) await drive(page, lib);
    const c = await get();
    record(info, "https-exact-all", { ...c, browserName });
    expect(summarize(c)).toEqual([]);
    expect(c.console).toEqual([]);
  });
});

test("control: no CSP at all, every library works (isolates browser quirks from CSP effects)", async ({ page }, info) => {
  const get = await collect(page);
  await page.goto("http://localhost:4187/");
  for (const lib of ["shiki", "query", "motion", "sonner", "vaul", "router"] as const) await drive(page, lib);
  const c = await get();
  record(info, "control-no-csp", c);
  expect(c.violations).toEqual([]);
});
