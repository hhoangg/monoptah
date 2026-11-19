// Pure helpers of the harness: no I/O, so they are unit tested (test/unit) without a server or a browser.
import { isAbsolute, join } from "node:path";

/** Reads the invite code out of the link `bootstrap.mjs` prints (`<origin>/register#invite=CODE`). */
export function parseInviteCode(output: string): string | null {
  const match = /#invite=([A-Za-z0-9_-]+)/.exec(output);
  return match?.[1] ?? null;
}

export interface HeadlessCapabilities {
  version: number;
  serve: boolean;
  /** The `pair` subcommand works (absent in older binaries: false). */
  pair: boolean;
  reason: string | null;
  checkVectors: boolean;
}

/**
 * Parses the output of `web_link_headless capabilities`: one JSON line
 * `{"version":1,"serve":<bool>,"pair":<bool, optional>,"reason":<string|null>,"checkVectors":true}`. Other lines
 * (logs) are ignored.
 * Returns null when no line has the expected shape.
 */
export function parseCapabilities(stdout: string): HeadlessCapabilities | null {
  for (const line of stdout.split(/\r?\n/)) {
    const text = line.trim();
    if (!text.startsWith("{")) continue;
    let value: unknown;
    try {
      value = JSON.parse(text);
    } catch {
      continue;
    }
    if (typeof value !== "object" || value === null) continue;
    const v = value as Record<string, unknown>;
    if (typeof v["version"] !== "number" || typeof v["serve"] !== "boolean" || typeof v["checkVectors"] !== "boolean") continue;
    const reason = v["reason"];
    if (reason !== null && typeof reason !== "string") continue;
    return { version: v["version"], serve: v["serve"], pair: v["pair"] === true, reason, checkVectors: v["checkVectors"] };
  }
  return null;
}

/** The headless binary path: env `E2E_HEADLESS_BIN` (relative to the repo root), else the native-dev build. */
export function resolveHeadlessBin(repoDir: string, env: NodeJS.ProcessEnv = process.env): string {
  const set = env["E2E_HEADLESS_BIN"];
  if (set && set.length > 0) return isAbsolute(set) ? set : join(repoDir, set);
  return join(repoDir, "crates", "monocode-gpui", "target", "native-dev", "web_link_headless");
}

export function base64urlEncode(bytes: Uint8Array): string {
  return Buffer.from(bytes).toString("base64url");
}

export function base64urlDecode(text: string): Uint8Array {
  if (!/^[A-Za-z0-9_-]*$/.test(text)) throw new Error("not base64url");
  return new Uint8Array(Buffer.from(text, "base64url"));
}

/** A distinct TEST-NET-2 address per actor (198.51.100.1 to .254), so no two actors share a per-IP rate limit. */
export function testIp(n: number): string {
  if (!Number.isInteger(n) || n < 1 || n > 254) throw new Error(`test IP index out of range: ${n}`);
  return `198.51.100.${n}`;
}

export interface Probe {
  /** True when the flow can run now. */
  run: boolean;
  /** Why it is skipped (carries the TODO marker), or a short note when it runs. */
  reason: string;
}

/** What a probe request answered: the HTTP status and the `code` of the JSON error body, if any. */
export interface ProbeAnswer {
  status: number;
  code: string | null;
}

/** Statuses a landed route answers to the probe body `{}`: validation error, conflict, or success. */
const PRESENT_STATUSES: readonly number[] = [200, 201, 400, 409, 422];

/**
 * Decides one route probe. Only a real "route not implemented" answer (501 `unsupported`, or 404) means the
 * dependency has not landed. No answer at all (dead server) and any other unexpected status THROW, so a broken
 * server or a wrong probe request can never look like "not landed" and can never look like "present".
 */
export function decideRouteProbe(label: string, marker: string, path: string, answer: ProbeAnswer | null): Probe {
  if (answer === null) throw new Error(`${label} probe: ${path} gave no answer (is the Worker running?)`);
  if (answer.status === 404 || (answer.status === 501 && answer.code === "unsupported")) {
    return { run: false, reason: `${marker} ${label} routes not landed (${path} answered ${answer.status} ${answer.code ?? ""})`.trimEnd() };
  }
  if (PRESENT_STATUSES.includes(answer.status)) return { run: true, reason: `${label} routes present (${path} answered ${answer.status})` };
  throw new Error(`${label} probe: unexpected answer from ${path}: ${answer.status} ${answer.code ?? ""}`.trimEnd());
}

export function decidePairingProbe(answer: ProbeAnswer | null): Probe {
  return decideRouteProbe("pairing", "TODO(E-01:WS-R R-03)", "POST /api/pair/start", answer);
}

export function decideDevicesProbe(answer: ProbeAnswer | null): Probe {
  return decideRouteProbe("device", "TODO(E-01:WS-R R-04)", "POST /api/devices", answer);
}

/** The headless desktop can pair only when the binary exists and reports `"pair": true`. */
export function decidePairProbe(binExists: boolean, caps: HeadlessCapabilities | null): Probe {
  if (!binExists) return { run: false, reason: "TODO(E-01:WS-D1 R-05) headless binary not built (set E2E_HEADLESS_BIN or build web_link_headless)" };
  if (caps === null) return { run: false, reason: "TODO(E-01:WS-D1 R-05) headless binary did not answer `capabilities` with the expected JSON line" };
  if (!caps.pair) return { run: false, reason: "TODO(E-01:WS-D1 R-05) headless binary reports pair=false" };
  return { run: true, reason: "headless binary reports pair=true" };
}

/** The headless desktop can serve only when the binary exists and reports `"serve": true`. */
export function decideHeadlessProbe(binExists: boolean, caps: HeadlessCapabilities | null): Probe {
  if (!binExists) return { run: false, reason: "TODO(E-01:WS-D1 R-05) headless binary not built (set E2E_HEADLESS_BIN or build web_link_headless)" };
  if (caps === null) return { run: false, reason: "TODO(E-01:WS-D1 R-05) headless binary did not answer `capabilities` with the expected JSON line" };
  if (!caps.serve) return { run: false, reason: `TODO(E-01:WS-D1 R-05, R-15) headless binary reports serve=false (${caps.reason ?? "no reason given"})` };
  return { run: true, reason: "headless binary reports serve=true" };
}

/** What the page at a route shows, as far as the probe can tell. */
export interface PageSnapshot {
  /** The page is the placeholder `ensure-assets.mjs` writes when web/app is not built. */
  isPlaceholder: boolean;
  /** An element `#root` exists. */
  hasRoot: boolean;
  /** An element `[data-testid=route-stub]` exists (the WS-S Wave 0 placeholder, web/app/src/ui/RouteStub.tsx). */
  hasRouteStub: boolean;
  /** `#root` has rendered children. */
  rootHasContent: boolean;
  /** Uncaught page errors seen while loading (messages only). */
  pageErrors: string[];
}

/**
 * UI flows skip only for the placeholder asset page and for a visible RouteStub. They run when `#root` renders
 * something else. A served app that renders neither (empty `#root`, no `#root`, boot crash) THROWS: that is the
 * failure a UI E2E exists to catch.
 */
export function decideUiProbe(snapshot: PageSnapshot): Probe {
  if (snapshot.hasRouteStub) return { run: false, reason: "TODO(E-01:WS-S W-02) the page still renders RouteStub" };
  if (snapshot.isPlaceholder) return { run: false, reason: "TODO(E-01:WS-S W-02) web/app is not built (placeholder asset page)" };
  if (!snapshot.hasRoot || !snapshot.rootHasContent) {
    const errors = snapshot.pageErrors.length > 0 ? ` Page errors: ${snapshot.pageErrors.join(" | ")}` : "";
    throw new Error(`the served app renders neither RouteStub nor a real page (root present: ${snapshot.hasRoot}, root has content: ${snapshot.rootHasContent}).${errors}`);
  }
  return { run: true, reason: "the page renders the real app" };
}

/**
 * The `wrangler d1 execute` arguments that empty the local rate limiter table (harness only). The limits are D1 rows
 * (security.md 3.5, `rate_limits`) that survive a Worker restart, and every headless `pair` reaches the Worker from
 * the same address, so a suite with more than 5 pairings inside 600 s would be answered 429. The table holds only
 * counters (hashed keys, a window start, a count): clearing it between spec files changes no Worker code and no
 * product behaviour, it only starts a new window.
 */
export function clearRateLimitsArgs(persistDir: string, wranglerConfig: string): string[] {
  return ["d1", "execute", "monoptah", "--local", "--persist-to", persistDir, "-c", wranglerConfig, "--command", "DELETE FROM rate_limits"];
}

/** Words of the credential fields (`desktopToken`, the pairing `deviceCode` and `verifier`, an Authorization bearer). */
const TOKEN_WORDS = /desktopToken|deviceCode|verifier|Bearer|Authorization/i;
/** A run that looks like key material or a token: 32 or more base64url or hex characters in a row. */
const SECRET_RUN = /[A-Za-z0-9_-]{32,}/;
/** Event fields that are public by design: ids and fingerprints are not secrets, and the link holds a fingerprint. */
const PUBLIC_FIELDS = new Set(["fingerprint", "desktopId", "deviceId", "link"]);

/**
 * Looks for a credential in what a headless process printed (stdout lines and stderr). There is no test-only reader of
 * the sealed token, so the scan is structural: none of the credential field names, and no 32+ character base64url or
 * hex run once the public fields (ids, fingerprints, the pairing link) are set aside. Returns what it found, never the
 * matched text itself (it would put a secret into a failure message).
 */
export function findCredentialLeaks(text: string): string[] {
  const found: string[] = [];
  for (const [index, raw] of text.split(/\r?\n/).entries()) {
    let line = raw;
    try {
      const value: unknown = JSON.parse(raw);
      if (typeof value === "object" && value !== null && !Array.isArray(value)) {
        line = JSON.stringify(Object.fromEntries(Object.entries(value).filter(([key]) => !PUBLIC_FIELDS.has(key))));
      }
    } catch {
      // not a JSON line: scanned as it is
    }
    if (TOKEN_WORDS.test(line)) found.push(`line ${index + 1}: a credential field name`);
    if (SECRET_RUN.test(line)) found.push(`line ${index + 1}: a 32+ character base64url or hex run`);
  }
  return found;
}
