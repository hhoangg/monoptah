// W-14 helpers: UI registration, theme, viewports, overflow check, axe scan. Shared by tests/w14-*.spec.ts.
import AxeBuilder from "@axe-core/playwright";
import type { Page } from "@playwright/test";
import { expect } from "@playwright/test";
import { mkdirSync } from "node:fs";
import { join } from "node:path";
import { outputDir } from "./paths.ts";

export type Scheme = "light" | "dark";

/**
 * Registers through the /register page up to the end of the second-passkey step (skipped) and returns the 10
 * recovery codes the page showed. Leaves the page on the first screen after sign-up with the codes acknowledged.
 */
export async function registerThroughUi(page: Page, invite: string, handle: string, displayName: string): Promise<string[]> {
  await page.goto("/register");
  await page.getByLabel("Invite code").fill(invite);
  await page.getByLabel("Handle").fill(handle);
  await page.getByLabel("Display name").fill(displayName);
  await page.getByRole("button", { name: /create passkey/i }).click();
  await expect(page.getByRole("heading", { name: "Save your recovery codes" })).toBeVisible();
  const items = page.getByRole("list", { name: "Recovery codes" }).getByRole("listitem");
  await expect(items).toHaveCount(10);
  const codes = (await items.locator("span").evaluateAll((spans) => spans.filter((s) => !s.hasAttribute("aria-hidden")).map((s) => s.textContent ?? ""))).map((c) => c.trim());
  expect(codes).toHaveLength(10);
  await expect(page.getByRole("button", { name: "Continue" })).toBeDisabled();
  await page.getByLabel("I saved these codes").check();
  await page.getByRole("button", { name: "Continue" }).click();
  await expect(page.getByRole("heading", { name: "Add a second passkey" })).toBeVisible();
  await page.getByRole("button", { name: "Skip for now" }).click();
  await expect(page).not.toHaveURL(/\/register/);
  return codes;
}

/** Stores the appearance the way the app does (`monoptah.appearance`, read by public/theme-init.js on load). */
export async function setScheme(page: Page, scheme: Scheme): Promise<void> {
  await page.evaluate((scheme) => window.localStorage.setItem("monoptah.appearance", scheme), scheme);
}

/** Opens `path` in the given scheme and waits until the app set `data-scheme` and the network is idle. */
export async function openIn(page: Page, path: string, scheme: Scheme): Promise<void> {
  await setScheme(page, scheme);
  await page.goto(path, { waitUntil: "load" });
  await expect(page.locator("html")).toHaveAttribute("data-scheme", scheme);
  await page.waitForLoadState("networkidle");
}

export interface Viewport {
  name: string;
  /** CSS px width the page lays out at. */
  width: number;
  height: number;
  /** CSS-pixel ratio: 2 only for the zoom emulation. */
  scale: number;
}

/**
 * Real browser zoom is not exposed to Playwright, and `deviceScaleFactor` alone only adds sharpness. What zoom does to
 * layout is shrink the CSS viewport: Chromium at 200 percent zoom on a 1280 px window lays the page out at 640 CSS px
 * and paints each CSS px as 2 device px. `zoom-200` therefore sets a 640 x 400 CSS viewport with a device scale factor
 * of 2 (the screenshot is 1280 x 800 device px), which is the layout and the rasterisation a user at 200 percent sees.
 */
export const VIEWPORTS: Viewport[] = [
  { name: "390", width: 390, height: 844, scale: 1 },
  { name: "768", width: 768, height: 1024, scale: 1 },
  { name: "1280", width: 1280, height: 800, scale: 1 },
  { name: "zoom-200", width: 640, height: 400, scale: 2 },
];

/** Applies a viewport through CDP so the device scale factor can change on a page that already exists. */
export async function applyViewport(page: Page, viewport: Viewport): Promise<void> {
  const cdp = await page.context().newCDPSession(page);
  await cdp.send("Emulation.setDeviceMetricsOverride", { width: viewport.width, height: viewport.height, deviceScaleFactor: viewport.scale, mobile: false });
  await cdp.detach().catch(() => undefined);
  const seen = await page.evaluate(() => ({ width: window.innerWidth, ratio: window.devicePixelRatio }));
  expect(seen).toEqual({ width: viewport.width, ratio: viewport.scale });
}

/** Back to the Playwright default viewport (1280 x 720, scale 1). */
export async function resetViewport(page: Page): Promise<void> {
  await applyViewport(page, { name: "default", width: 1280, height: 720, scale: 1 });
}

/** No horizontal scrolling: the document is not wider than the viewport. */
export async function expectNoHorizontalOverflow(page: Page, label: string): Promise<void> {
  const sizes = await page.evaluate(() => ({ scroll: document.documentElement.scrollWidth, client: document.documentElement.clientWidth }));
  expect(sizes.scroll, `${label}: scrollWidth ${sizes.scroll} > clientWidth ${sizes.client}`).toBeLessThanOrEqual(sizes.client);
}

export function visualDir(): string {
  const dir = join(outputDir(), "w14-visual");
  mkdirSync(dir, { recursive: true });
  return dir;
}

export interface AxeSummary {
  rule: string;
  impact: string | null | undefined;
  nodes: string[];
}

/** Runs axe (wcag2a, wcag2aa) and returns the violations of impact serious or critical. */
export async function axeSeriousViolations(page: Page): Promise<AxeSummary[]> {
  const results = await new AxeBuilder({ page }).withTags(["wcag2a", "wcag2aa"]).analyze();
  return results.violations
    .filter((v) => v.impact === "serious" || v.impact === "critical")
    .map((v) => ({ rule: v.id, impact: v.impact, nodes: v.nodes.map((n) => `${n.target.join(" ")} :: ${n.failureSummary?.split("\n").slice(0, 3).join(" | ") ?? ""}`) }));
}

/** True when the active element has a visible focus indicator (outline, ring shadow or border change). */
export async function focusIsVisible(page: Page): Promise<boolean> {
  return page.evaluate(() => {
    const el = document.activeElement;
    if (!el || el === document.body) return false;
    const style = getComputedStyle(el);
    const outline = style.outlineStyle !== "none" && parseFloat(style.outlineWidth) > 0;
    const ring = style.boxShadow !== "none" && style.boxShadow !== "";
    return outline || ring;
  });
}
