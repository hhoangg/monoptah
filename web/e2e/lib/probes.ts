// Automatic probes: a flow that needs a workstream that has not landed yet is skipped with its TODO marker and
// turns on by itself when the dependency appears. The decisions are pure (lib/pure.ts); this file does the I/O.
import { spawnSync } from "node:child_process";
import { existsSync } from "node:fs";
import type { Page } from "@playwright/test";
import type { Actor } from "./actor.ts";
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
  type ProbeAnswer,
} from "./pure.ts";
import { repoDir } from "./paths.ts";

function answerOf(status: number, body: unknown): ProbeAnswer {
  const code = typeof body === "object" && body !== null && typeof (body as { code?: unknown }).code === "string" ? (body as { code: string }).code : null;
  return { status, code };
}

/** Public desktop route: no Origin, no cookie, as the desktop app calls it. */
async function publicAnswer(origin: string, path: string, ip: string): Promise<ProbeAnswer | null> {
  try {
    const res = await fetch(`${origin}${path}`, {
      method: "POST",
      headers: { "Content-Type": "application/json", "CF-Connecting-IP": ip },
      body: "{}",
      signal: AbortSignal.timeout(10_000),
    });
    return answerOf(res.status, await res.json().catch(() => null));
  } catch {
    return null;
  }
}

/** Throws (fails the test) when the Worker does not answer; skips only on a real "not implemented". */
export async function probePairing(origin: string): Promise<Probe> {
  return decidePairingProbe(await publicAnswer(origin, "/api/pair/start", "198.51.100.240"));
}

/** `/api/devices` is a session route: the probe goes through an actor with a session (page Origin, CSRF, cookie). */
export async function probeDevices(actor: Actor): Promise<Probe> {
  try {
    const res = await actor.call("POST", "/api/devices", { body: {} });
    return decideDevicesProbe(answerOf(res.status, res.json));
  } catch (err) {
    if (err instanceof Error && err.message.startsWith("device probe")) throw err;
    return decideDevicesProbe(null);
  }
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
  const pageErrors: string[] = [];
  page.on("pageerror", (err) => pageErrors.push(err.message));
  await page.goto(route, { waitUntil: "load" });
  // Wait for the app to render something, the stub, or the placeholder; a boot crash shows none of them.
  await page
    .waitForFunction(
      () => document.querySelector("#root > *") !== null || (document.body.textContent ?? "").includes("web placeholder"),
      undefined,
      { timeout: 10_000 },
    )
    .catch(() => undefined);
  const snapshot: PageSnapshot = await page.evaluate((pageErrors) => {
    const root = document.getElementById("root");
    return {
      isPlaceholder: root === null && (document.body.textContent ?? "").includes("web placeholder"),
      hasRoot: root !== null,
      hasRouteStub: document.querySelector("[data-testid=route-stub]") !== null,
      rootHasContent: root !== null && root.childElementCount > 0,
      pageErrors,
    };
  }, pageErrors);
  return decideUiProbe(snapshot);
}
