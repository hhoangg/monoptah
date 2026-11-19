import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { spawn, type ChildProcess } from "node:child_process";
import { createServer } from "node:net";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const spikes = resolve(dirname(fileURLToPath(import.meta.url)), "../..");

/** A free TCP port on localhost (bind to 0, read, release). */
export function freePort(): Promise<number> {
  return new Promise((ok, fail) => {
    const srv = createServer();
    srv.once("error", fail);
    srv.listen(0, "127.0.0.1", () => {
      const addr = srv.address();
      const port = typeof addr === "object" && addr ? addr.port : 0;
      srv.close(() => ok(port));
    });
  });
}

export interface DevServer {
  url: string;
  wsUrl: string;
  stop(): Promise<void>;
}

/** Starts `wrangler dev --local` for the config in `configDir` on a free port and waits until it answers. */
export async function startWranglerDev(configDir: string): Promise<DevServer> {
  const port = await freePort();
  const inspector = await freePort();
  // Own state dir per run: parallel runs must not share one .wrangler/state (they corrupt each other).
  const persist = mkdtempSync(resolve(tmpdir(), "spike-wrangler-"));
  const child: ChildProcess = spawn(
    resolve(spikes, "node_modules/.bin/wrangler"),
    ["dev", "--local", "--port", String(port), "--inspector-port", String(inspector), "--ip", "127.0.0.1", "--persist-to", persist],
    { cwd: configDir, stdio: ["ignore", "pipe", "pipe"], env: { ...process.env, WRANGLER_SEND_METRICS: "false", NO_COLOR: "1" } },
  );
  let log = "";
  child.stdout?.on("data", (d: Buffer) => (log += d.toString()));
  child.stderr?.on("data", (d: Buffer) => (log += d.toString()));
  const url = `http://127.0.0.1:${port}`;
  const deadline = Date.now() + 60_000;
  for (;;) {
    if (child.exitCode !== null) throw new Error(`wrangler dev exited early:\n${log}`);
    try {
      const res = await fetch(`${url}/stats`);
      await res.text();
      break;
    } catch {
      if (Date.now() > deadline) {
        child.kill("SIGKILL");
        throw new Error(`wrangler dev did not start:\n${log}`);
      }
      await new Promise((r) => setTimeout(r, 300));
    }
  }
  return {
    url,
    wsUrl: `ws://127.0.0.1:${port}`,
    stop: () =>
      new Promise((ok) => {
        const done = () => {
          rmSync(persist, { recursive: true, force: true });
          ok();
        };
        if (child.exitCode !== null) return done();
        child.once("exit", done);
        child.kill("SIGTERM");
        setTimeout(() => child.kill("SIGKILL"), 5000).unref();
      }),
  };
}
