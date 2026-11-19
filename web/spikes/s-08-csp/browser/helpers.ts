import { writeFileSync, mkdirSync } from "node:fs";
import type { Page, TestInfo } from "@playwright/test";

/**
 * Server for a variant. WebKit upgrades http://localhost subresources to https because of
 * `upgrade-insecure-requests` (Chromium and Firefox exempt localhost), so it gets the same build
 * with that single directive removed; everything else in the CSP is identical.
 */
export function server(browserName: string, variant: "exact" | "noStyleAttr" | "raw"): string {
  const webkit = browserName === "webkit";
  const ports = { exact: webkit ? 4184 : 4180, noStyleAttr: webkit ? 4185 : 4181, raw: webkit ? 4183 : 4182 };
  return `http://localhost:${ports[variant]}`;
}

export interface Violation { directive: string; blocked: string; sample: string; disposition: string }
export interface Collected { violations: Violation[]; console: string[] }

/** Records every securitypolicyviolation event (init script) and CSP / Trusted Types console messages. */
export async function collect(page: Page): Promise<() => Promise<Collected>> {
  const logs: string[] = [];
  await page.addInitScript(() => {
    const w = window as unknown as { __v: unknown[] };
    w.__v = [];
    document.addEventListener("securitypolicyviolation", (e) => {
      w.__v.push({ directive: e.violatedDirective, blocked: e.blockedURI, sample: e.sample, disposition: e.disposition });
    });
  });
  const interesting = /content security|trusted type|violat|refused|require-trusted|csp/i;
  page.on("console", (m) => { if (interesting.test(m.text())) logs.push(`${m.type()}: ${m.text()}`.slice(0, 300)); });
  page.on("pageerror", (e) => { logs.push(`pageerror: ${e.message}`.slice(0, 300)); });
  return async () => ({
    violations: await page.evaluate(() => (window as unknown as { __v: Violation[] }).__v),
    console: logs,
  });
}

export function record(info: TestInfo, name: string, data: unknown): void {
  const dir = "/tmp/evidence/ws-q/browser/s08";
  mkdirSync(dir, { recursive: true });
  writeFileSync(`${dir}/${info.project.name}-${name}.json`, JSON.stringify(data, null, 2));
}

export const summarize = (c: Collected): string[] =>
  [...new Set(c.violations.map((v) => `${v.directive} blocked=${v.blocked} sample=${v.sample.slice(0, 40)}`))];
