// Automatic probes: a flow that needs a workstream that has not landed yet is skipped with its TODO marker and
// turns on by itself when the dependency appears. The decisions are pure (lib/pure.ts); this file does the I/O.
import { spawnSync } from "node:child_process";
import { existsSync } from "node:fs";
import type { Page } from "@playwright/test";
import {
  decideDevicesProbe,
  decideHeadlessProbe,
  decidePairingProbe,
  decideUiProbe,
  parseCapabilities,
  resolveHeadlessBin,
  type HeadlessCapabilities,
  type PageSnapshot,
  type Probe,
} from "./pure.ts";
import { repoDir } from "./paths.ts";

async function statusOf(origin: string, path: string, ip: string): Promise<number | null> {
  try {
    const res = await fetch(`${origin}${path}`, {
      method: "POST",
      headers: { "Content-Type": "application/json", "CF-Connecting-IP": ip },
      body: "{}",
      signal: AbortSignal.timeout(10_000),
    });
    return res.status;
  } catch {
    return null;
  }
}

export async function probePairing(origin: string): Promise<Probe> {
  return decidePairingProbe(await statusOf(origin, "/api/pair/start", "198.51.100.240"));
}

export async function probeDevices(origin: string): Promise<Probe> {
  return decideDevicesProbe(await statusOf(origin, "/api/devices", "198.51.100.241"));
}

export const headlessBin = (): string => resolveHeadlessBin(repoDir);

export function headlessCapabilities(): HeadlessCapabilities | null {
  const bin = headlessBin();
  if (!existsSync(bin)) return null;
  const r = spawnSync(bin, ["capabilities"], { encoding: "utf8", timeout: 20_000 });
  return r.status === 0 ? parseCapabilities(r.stdout) : null;
}

export function probeHeadless(): Probe {
  return decideHeadlessProbe(existsSync(headlessBin()), headlessCapabilities());
}

/** Looks at one route of the served app. The page must already be on the worker origin. */
export async function probeUi(page: Page, route: string): Promise<Probe> {
  await page.goto(route, { waitUntil: "load" });
  const snapshot: PageSnapshot = await page
    .waitForSelector("[data-testid=route-stub]", { timeout: 3000 })
    .then(() => true, () => false)
    .then((hasRouteStub) =>
      page.evaluate((hasRouteStub) => {
        const root = document.getElementById("root");
        return { hasRoot: root !== null, hasRouteStub, rootHasContent: root !== null && root.childElementCount > 0 };
      }, hasRouteStub),
    );
  return decideUiProbe(snapshot);
}
