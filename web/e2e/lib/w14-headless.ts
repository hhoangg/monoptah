// W-14 helper: runs the headless desktop (`web_link_headless`) as a child process and reads its JSON lines.
// A copy of the spawn pattern of tests/pairing.spec.ts (that file is not shared on purpose). `pairDesktop` starts
// `pair` and waits for the user code; the UI specs open /pair/<userCode> and approve through the page. The second
// pass reuses `runHeadless(["serve", ...])` for the confirm, read, send, stop and revoke flows.
import { spawn, type ChildProcess } from "node:child_process";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { createInterface } from "node:readline";
import { expect } from "@playwright/test";
import { origin } from "./env.ts";
import { headlessBin } from "./probes.ts";

export interface Headless {
  child: ChildProcess;
  /** Every JSON line the process printed on stdout, in order. */
  events: Record<string, unknown>[];
  /** Resolves with the exit code. */
  exited: Promise<number | null>;
  stderr: () => string;
  stdoutText: () => string;
}

/** Spawns the headless binary. Its stdout is JSON lines (never the token); stderr is kept for failure messages. */
export function runHeadless(args: string[]): Headless {
  const child = spawn(headlessBin(), args, { stdio: ["ignore", "pipe", "pipe"] });
  const events: Record<string, unknown>[] = [];
  let outText = "";
  child.stdout!.on("data", (chunk: Buffer) => {
    outText += chunk.toString();
  });
  createInterface({ input: child.stdout! }).on("line", (line) => {
    try {
      events.push(JSON.parse(line) as Record<string, unknown>);
    } catch {
      // not a JSON line: ignored
    }
  });
  let errText = "";
  child.stderr!.on("data", (chunk: Buffer) => {
    errText = (errText + chunk.toString()).slice(-2000);
  });
  const exited = new Promise<number | null>((resolve) => child.on("exit", (code) => resolve(code)));
  return { child, events, exited, stderr: () => errText, stdoutText: () => outText };
}

export const eventOf = (headless: Headless, name: string): Record<string, unknown> | undefined => headless.events.find((e) => e["event"] === name);

export interface PairingStarted {
  userCode: string;
  confirmationCode: string;
  fingerprint: string;
}

export interface PendingPairing {
  process: Headless;
  started: PairingStarted;
  dataDir: string;
  /** Kills the process and removes its data dir. Safe to call twice. */
  dispose: () => void;
}

/** Starts `pair` for a machine called `machineName` and waits until it printed its user code. */
export async function startPairing(machineName: string): Promise<PendingPairing> {
  const dataDir = mkdtempSync(join(tmpdir(), "monoptah-e2e-w14-"));
  const process = runHeadless(["pair", "--server", origin(), "--data-dir", join(dataDir, "data"), "--machine-name", machineName]);
  const dispose = (): void => {
    process.child.kill("SIGTERM");
    rmSync(dataDir, { recursive: true, force: true });
  };
  try {
    await expect
      .poll(
        () => {
          if (eventOf(process, "pairing") === undefined && process.child.exitCode !== null) {
            throw new Error(`pair exited with ${process.child.exitCode} before printing a user code: ${process.stderr()}`);
          }
          return eventOf(process, "pairing") !== undefined;
        },
        { timeout: 15_000 },
      )
      .toBe(true);
  } catch (err) {
    dispose();
    throw err;
  }
  const started = eventOf(process, "pairing") as unknown as PairingStarted;
  return { process, started, dataDir, dispose };
}
