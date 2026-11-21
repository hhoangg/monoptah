// Starts the Worker on a local `wrangler dev --local` (Miniflare: local D1 and Durable Objects) in a temp dir,
// the same approach as web/scripts/api-smoke.mjs. No Cloudflare account, no network beyond localhost.
import { spawn, spawnSync, type ChildProcess } from "node:child_process";
import { randomBytes } from "node:crypto";
import { createWriteStream, existsSync, mkdirSync, mkdtempSync, rmSync } from "node:fs";
import { createServer } from "node:net";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { clearRateLimitsArgs, parseInviteCode } from "./pure.ts";
import { webDir, wranglerBin, wranglerConfig } from "./paths.ts";

export const RP_ID = "localhost";

export interface RunningServer {
  origin: string;
  /** The one-use bootstrap invite. Never print it. */
  invite: string;
  /** More bootstrap invites, one per spec that needs its own account. Never print them. */
  extraInvites: { pairing: string; devices: string; ui: string; w14: string; w14b: string; r12: string; filepreview: string; mlayout: string; selection: string; htmlpreview: string; htmlpreviewwk: string; activityrail: string; terminal: string; avatarslayout: string };
  /**
   * Kills wrangler and starts it again on the same port with the same local persistence (D1 and DO state survive).
   * With `clearRateLimits` the D1 rate limiter table is emptied while wrangler is down (harness only, see
   * `clearRateLimitsArgs`), and the restart also drops the Worker's in-memory per-IP pre-filter: a new window.
   */
  restart(options?: { clearRateLimits?: boolean }): Promise<void>;
  /** Stops wrangler and removes the temp dir. Waits for exit, escalates to SIGKILL, never throws. */
  stop(): Promise<void>;
}

const START_ATTEMPTS = 3;

function freePort(): Promise<number> {
  return new Promise((resolve, reject) => {
    const server = createServer();
    server.once("error", reject);
    server.listen(0, "127.0.0.1", () => {
      const address = server.address();
      const port = typeof address === "object" && address !== null ? address.port : 0;
      server.close(() => resolve(port));
    });
  });
}

function runSync(cmd: string, args: string[], label: string): string {
  const r = spawnSync(cmd, args, { cwd: webDir, encoding: "utf8", env: { ...process.env, WRANGLER_SEND_METRICS: "false" } });
  if (r.status !== 0) throw new Error(`${label} failed (exit ${r.status}): ${(r.stderr || "").slice(-600)}`);
  return r.stdout;
}

const sleep = (ms: number): Promise<void> => new Promise((resolve) => setTimeout(resolve, ms));

function signalGroup(child: ChildProcess, signal: NodeJS.Signals): void {
  if (child.pid === undefined) return;
  try {
    process.kill(-child.pid, signal);
  } catch {
    try {
      child.kill(signal);
    } catch {
      // already gone
    }
  }
}

/** SIGTERM the process group, wait for the parent to exit (10 s), then SIGKILL the whole group. Never throws. */
async function killTree(child: ChildProcess | null): Promise<void> {
  if (!child) return;
  try {
    const exited = new Promise<void>((resolve) => {
      if (child.exitCode !== null || child.signalCode !== null) resolve();
      else child.once("exit", () => resolve());
    });
    signalGroup(child, "SIGTERM");
    await Promise.race([exited, sleep(10_000)]);
    signalGroup(child, "SIGKILL"); // workerd children may outlive the parent; ESRCH is ignored
    await sleep(200);
  } catch {
    // teardown must not fail the run
  }
}

async function waitForHealth(origin: string, wrangler: ChildProcess): Promise<void> {
  const deadline = Date.now() + 120_000;
  while (Date.now() < deadline) {
    if (wrangler.exitCode !== null) throw new Error(`wrangler dev exited early with code ${wrangler.exitCode}`);
    try {
      const res = await fetch(`${origin}/api/health`, { signal: AbortSignal.timeout(2000) });
      if (res.status === 200) return;
    } catch {
      // not up yet
    }
    await sleep(500);
  }
  throw new Error("wrangler dev did not answer /api/health within 120 s");
}

/** `logFile`: where wrangler's stdout and stderr go (it holds no secret; the fake secrets are never printed). */
export async function startServer(logFile: string): Promise<RunningServer> {
  if (!existsSync(wranglerBin)) throw new Error("wrangler not found in web/e2e/node_modules/.bin: run `pnpm install` in web/ first");
  const tmp = mkdtempSync(join(tmpdir(), "monoptah-e2e-"));
  const persist = join(tmp, "state");
  let wrangler: ChildProcess | null = null;
  let stopped: Promise<void> | null = null;
  const stop = (): Promise<void> => {
    stopped ??= (async () => {
      await killTree(wrangler);
      try {
        rmSync(tmp, { recursive: true, force: true, maxRetries: 3, retryDelay: 200 });
      } catch {
        // a leftover temp dir is not worth failing the run
      }
    })();
    return stopped;
  };
  try {
    // E2E_ASSETS_DIR (a build outside the repo, see scripts/e2e-web.sh) replaces the assets directory of
    // wrangler.jsonc. Otherwise the placeholder is ensured, as for the other local runs.
    const assetsDir = process.env["E2E_ASSETS_DIR"];
    if (!assetsDir) runSync(process.execPath, [join(webDir, "worker", "scripts", "ensure-assets.mjs")], "ensure-assets");
    else if (!existsSync(join(assetsDir, "index.html"))) throw new Error(`E2E_ASSETS_DIR has no index.html: ${assetsDir}`);
    runSync(wranglerBin, ["d1", "migrations", "apply", "monoptah", "--local", "--persist-to", persist, "-c", wranglerConfig], "d1 migrations apply");
    const bootstrap = (): string => {
      const printed = runSync(process.execPath, [join(webDir, "scripts", "bootstrap.mjs"), "--local", "--persist-to", persist, "--origin", "http://localhost"], "bootstrap");
      const code = parseInviteCode(printed);
      if (code === null) throw new Error("bootstrap printed no invite link");
      return code;
    };
    const invite = bootstrap();
    const extraInvites = { pairing: bootstrap(), devices: bootstrap(), ui: bootstrap(), w14: bootstrap(), w14b: bootstrap(), r12: bootstrap(), filepreview: bootstrap(), mlayout: bootstrap(), selection: bootstrap(), htmlpreview: bootstrap(), htmlpreviewwk: bootstrap(), activityrail: bootstrap(), terminal: bootstrap(), avatarslayout: bootstrap() };

    // Obviously fake secrets, passed with --var so a developer's web/.dev.vars is never read.
    const fake = {
      VAPID_PRIVATE_KEY: `e2e-fake-vapid-${randomBytes(12).toString("hex")}`,
      RECOVERY_PEPPER: `e2e-fake-pepper-${randomBytes(12).toString("hex")}`,
      AUDIT_IP_SALT: `e2e-fake-salt-${randomBytes(12).toString("hex")}`,
    };
    mkdirSync(dirname(logFile), { recursive: true });
    const log = createWriteStream(logFile);

    const launch = async (port: number): Promise<ChildProcess> => {
      const args = [
        "dev", "--local", "--persist-to", persist, "--port", String(port), "--ip", "127.0.0.1", "-c", wranglerConfig,
        "--show-interactive-dev-session=false",
        // wrangler.jsonc has a custom_domain route: without this, wrangler forwards to that zone's host and the Worker
        // sees another Origin than RP_ORIGIN (every browser POST answers 403 bad_origin).
        "--local-upstream", `localhost:${port}`,
        ...(assetsDir ? ["--assets", assetsDir] : []),
        "--var", `RP_ID:${RP_ID}`, "--var", `RP_ORIGIN:http://localhost:${port}`,
        ...Object.entries(fake).flatMap(([name, value]) => ["--var", `${name}:${value}`]),
      ];
      const child = spawn(wranglerBin, args, {
        cwd: webDir,
        detached: true,
        stdio: ["ignore", "pipe", "pipe"],
        env: { ...process.env, WRANGLER_SEND_METRICS: "false", WRANGLER_LOG_PATH: join(tmp, "wrangler-logs"), NO_COLOR: "1" },
      });
      wrangler = child;
      child.stdout?.pipe(log, { end: false });
      child.stderr?.pipe(log, { end: false });
      try {
        await waitForHealth(`http://localhost:${port}`, child);
      } catch (err) {
        await killTree(child);
        wrangler = null;
        throw err;
      }
      return child;
    };

    // The free port can be taken between the probe and wrangler's bind: retry with a new port.
    let origin = "";
    let lastError: unknown;
    for (let attempt = 1; attempt <= START_ATTEMPTS; attempt++) {
      const port = await freePort();
      origin = `http://localhost:${port}`;
      try {
        await launch(port);
        lastError = undefined;
        break;
      } catch (err) {
        lastError = err;
      }
    }
    if (lastError !== undefined) throw lastError;

    const restart = async (options: { clearRateLimits?: boolean } = {}): Promise<void> => {
      const port = Number(new URL(origin).port);
      await killTree(wrangler);
      wrangler = null;
      if (options.clearRateLimits) runSync(wranglerBin, clearRateLimitsArgs(persist, wranglerConfig), "d1 clear rate_limits");
      let failure: unknown;
      for (let attempt = 1; attempt <= START_ATTEMPTS; attempt++) {
        try {
          await launch(port);
          return;
        } catch (err) {
          failure = err;
          await sleep(1000);
        }
      }
      throw failure;
    };

    const onSignal = (code: number) => (): void => {
      void stop().then(() => process.exit(code));
    };
    process.once("SIGINT", onSignal(130));
    process.once("SIGTERM", onSignal(143));
    return { origin, invite, extraInvites, restart, stop };
  } catch (err) {
    await stop();
    throw err;
  }
}
