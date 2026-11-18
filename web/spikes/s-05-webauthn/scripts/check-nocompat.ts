// S-05 automated "no compatibility flags needed" check. Starts `wrangler dev --local` (real workerd, not the vitest pool,
// which forces nodejs_compat) for each control config, and asserts that /probe reports no Node globals where Node compat is off
// and that registration and authentication verify for ES256, RS256 and Ed25519. Not part of `pnpm test` (it starts wrangler,
// about 15 s per config). Run from web/spikes:  node s-05-webauthn/scripts/check-nocompat.ts
import { spawn, type ChildProcess } from "node:child_process";
import { createServer } from "node:net";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const spikes = join(here, "..", "..");

const CASES = [
  { config: "s-05-webauthn/wrangler.nonode.jsonc", expectNodeGlobals: false, why: "date 2026-09-01 with no_nodejs_compat + no_nodejs_compat_v2" },
  { config: "s-05-webauthn/wrangler.olddate.jsonc", expectNodeGlobals: false, why: "date 2025-09-01, no flags" },
  { config: "s-05-webauthn/wrangler.jsonc", expectNodeGlobals: true, why: "date 2026-09-01, no flags (nodejs_compat is default from 2026-08-04)" },
];

function freePort(): Promise<number> {
  return new Promise((resolve, reject) => {
    const server = createServer();
    server.listen(0, "127.0.0.1", () => {
      const address = server.address();
      server.close(() => (typeof address === "object" && address ? resolve(address.port) : reject(new Error("no port"))));
    });
  });
}

async function waitReady(child: ChildProcess, port: number): Promise<void> {
  const deadline = Date.now() + 90_000;
  while (Date.now() < deadline) {
    if (child.exitCode !== null) throw new Error(`wrangler exited with ${child.exitCode}`);
    try {
      if ((await fetch(`http://127.0.0.1:${port}/probe`)).ok) return;
    } catch {
      // not up yet
    }
    await new Promise((r) => setTimeout(r, 500));
  }
  throw new Error("wrangler dev did not become ready in 90 s");
}

function assert(condition: boolean, message: string): void {
  if (!condition) throw new Error(`FAILED: ${message}`);
}

let failed = false;
for (const item of CASES) {
  const port = await freePort();
  const inspector = await freePort();
  const child = spawn("npx", ["wrangler", "dev", "--local", "-c", item.config, "--port", String(port), "--inspector-port", String(inspector)], {
    cwd: spikes,
    detached: true,
    stdio: "ignore",
  });
  try {
    await waitReady(child, port);
    const probe = (await (await fetch(`http://127.0.0.1:${port}/probe`)).json()) as { Buffer: string; process: string };
    const present = probe.Buffer === "function" && probe.process === "object";
    assert(present === item.expectNodeGlobals, `${item.config}: Node globals present=${String(present)}, expected ${String(item.expectNodeGlobals)}`);
    for (const alg of ["ES256", "RS256", "Ed25519"]) {
      for (const op of ["auth", "reg"]) {
        // The first request per algorithm builds the fixture; both must verify.
        for (let i = 0; i < 2; i += 1) {
          const body = (await (await fetch(`http://127.0.0.1:${port}/verify?alg=${alg}&op=${op}`)).json()) as { verified: boolean };
          assert(body.verified === true, `${item.config}: ${alg} ${op} did not verify`);
        }
      }
    }
    console.log(`PASS ${item.config} (${item.why}): node globals ${present ? "present" : "absent"}, ES256/RS256/Ed25519 register + login verify`);
  } catch (error) {
    failed = true;
    console.error(`FAIL ${item.config}: ${error instanceof Error ? error.message : String(error)}`);
  } finally {
    if (child.pid !== undefined) {
      try {
        process.kill(-child.pid, "SIGTERM");
      } catch {
        // already gone
      }
    }
  }
}
process.exit(failed ? 1 : 0);
