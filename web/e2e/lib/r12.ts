// Helpers of the R-12 spec: the headless desktop with a command pipe, a recording socket for the replay case, raw
// WebSocket close-code probes, and a desktop played by the test itself (it holds a token, which the headless binary
// never reveals).
import { spawn, type ChildProcess } from "node:child_process";
import { createHash, randomBytes } from "node:crypto";
import { statSync } from "node:fs";
import { join } from "node:path";
import { createInterface } from "node:readline";
import { FrameType } from "@monoptah/protocol";
import type { FrameSocket } from "../../mock-desktop/src/transport.ts";
import { connectRelay } from "../../mock-desktop/src/transport.ts";
import { generateKeyPair } from "../../mock-desktop/src/noise.ts";
import { headlessBin } from "./probes.ts";
import { base64urlEncode, findCredentialLeaks } from "./pure.ts";

export type DesktopEvent = Record<string, unknown>;

/** The headless desktop: JSON lines out (never a token), JSON-line commands in (see the module doc of the binary). */
export class HeadlessDesktop {
  readonly child: ChildProcess;
  readonly events: DesktopEvent[] = [];
  readonly exited: Promise<number | null>;
  #stderr = "";
  #stderrAll = "";
  #stdout = "";

  constructor(args: string[]) {
    this.child = spawn(headlessBin(), args, { stdio: ["pipe", "pipe", "pipe"] });
    this.child.stdout!.on("data", (chunk: Buffer) => {
      this.#stdout += chunk.toString();
    });
    createInterface({ input: this.child.stdout! }).on("line", (line) => {
      try {
        this.events.push(JSON.parse(line) as DesktopEvent);
      } catch {
        // not a JSON line
      }
    });
    this.child.stderr!.on("data", (chunk: Buffer) => {
      this.#stderr = (this.#stderr + chunk.toString()).slice(-2000);
      this.#stderrAll = (this.#stderrAll + chunk.toString()).slice(-262_144);
    });
    this.child.stdin!.on("error", () => undefined);
    this.exited = new Promise((resolve) => this.child.on("exit", (code) => resolve(code)));
  }

  stderr(): string {
    return this.#stderr;
  }
  /** Everything printed on stderr (the last 256 KiB), for the credential scan. */
  stderrAll(): string {
    return this.#stderrAll;
  }
  stdoutText(): string {
    return this.#stdout;
  }
  command(command: Record<string, unknown>): void {
    this.child.stdin!.write(`${JSON.stringify(command)}\n`);
  }
  matching(event: string, where: Record<string, unknown> = {}): DesktopEvent[] {
    return this.events.filter((e) => e["event"] === event && Object.entries(where).every(([k, v]) => e[k] === v));
  }

  /** Resolves with the first event after `from` (an index into `events`) that matches; fails with stderr on timeout. */
  async waitFor(event: string, where: Record<string, unknown> = {}, timeoutMs = 30_000, from = 0): Promise<DesktopEvent> {
    const deadline = Date.now() + timeoutMs;
    for (;;) {
      const found = this.events.slice(from).find((e) => e["event"] === event && Object.entries(where).every(([k, v]) => e[k] === v));
      if (found) return found;
      if (this.child.exitCode !== null) throw new Error(`desktop exited with ${this.child.exitCode} while waiting for ${event}: ${this.#stderr}`);
      if (Date.now() > deadline) throw new Error(`no ${event} ${JSON.stringify(where)} within ${timeoutMs} ms; got ${JSON.stringify(this.events.map((e) => e["event"]))}; stderr: ${this.#stderr}`);
      await new Promise((resolve) => setTimeout(resolve, 100));
    }
  }

  kill(): void {
    this.child.kill("SIGTERM");
  }
}

/** A FrameSocket that records every frame it sends and forwards everything else to the real socket. */
export class RecordingSocket implements FrameSocket {
  readonly sent: Uint8Array[] = [];
  readonly #inner: FrameSocket;
  constructor(inner: FrameSocket) {
    this.#inner = inner;
  }
  send(bytes: Uint8Array): void {
    this.sent.push(bytes.slice());
    this.#inner.send(bytes);
  }
  sendText(text: string): void {
    this.#inner.sendText(text);
  }
  close(code?: number): void {
    this.#inner.close(code);
  }
  get onmessage(): FrameSocket["onmessage"] {
    return this.#inner.onmessage;
  }
  set onmessage(value: FrameSocket["onmessage"]) {
    this.#inner.onmessage = value;
  }
  get ontext(): FrameSocket["ontext"] {
    return this.#inner.ontext;
  }
  set ontext(value: FrameSocket["ontext"]) {
    this.#inner.ontext = value;
  }
  get onclose(): FrameSocket["onclose"] {
    return this.#inner.onclose;
  }
  set onclose(value: FrameSocket["onclose"]) {
    this.#inner.onclose = value;
  }
  get closed(): boolean {
    return this.#inner.closed;
  }
  /** The last TRANSPORT frame sent. */
  lastTransport(): Uint8Array {
    const frame = [...this.sent].reverse().find((bytes) => bytes[1] === FrameType.TRANSPORT);
    if (!frame) throw new Error("no TRANSPORT frame was sent");
    return frame;
  }
  /** The raw underlying socket, to send bytes that bypass the client. */
  raw(): FrameSocket {
    return this.#inner;
  }
}

type WebSocketCtor = new (url: string, init?: unknown) => WebSocket;

/**
 * Opens a relay WebSocket with `headers`, optionally sends `payload` once open, and resolves with the close code the
 * relay ends it with. A refused upgrade is answered with an accepted socket that is closed at once (architecture.md
 * 4.4), so the code is readable. Rejects when nothing closes the socket within `timeoutMs`.
 */
export function closeCodeOf(url: string, headers: Record<string, string>, payload?: Uint8Array, timeoutMs = 20_000): Promise<number> {
  return new Promise((resolve, reject) => {
    const Ctor = (globalThis as unknown as { WebSocket: WebSocketCtor }).WebSocket;
    const ws = new Ctor(url, { headers });
    ws.binaryType = "arraybuffer";
    const timer = setTimeout(() => {
      ws.close();
      reject(new Error(`${url} was not closed within ${timeoutMs} ms`));
    }, timeoutMs);
    ws.onopen = () => {
      if (payload) ws.send(payload.slice().buffer as ArrayBuffer);
    };
    ws.onclose = (event) => {
      clearTimeout(timer);
      resolve(event.code);
    };
  });
}

export interface TsDesktop {
  desktopId: string;
  /** The bearer token of the desktop. Never log it. */
  token: string;
  userCode: string;
  deviceCode: string;
  verifier: string;
}

/** The `ip` of a public desktop call, so it does not share a per-IP rate limit with the headless binary. */
async function publicPost(origin: string, path: string, ip: string, body: unknown): Promise<{ status: number; json: Record<string, any> }> { // eslint-disable-line @typescript-eslint/no-explicit-any
  const res = await fetch(`${origin}${path}`, { method: "POST", headers: { "Content-Type": "application/json", "CF-Connecting-IP": ip }, body: JSON.stringify(body) });
  return { status: res.status, json: ((await res.json().catch(() => null)) ?? {}) as Record<string, any> }; // eslint-disable-line @typescript-eslint/no-explicit-any
}

export class TsDesktopPairing {
  readonly origin: string;
  readonly ip: string;
  readonly verifier = randomBytes(32);
  userCode = "";
  deviceCode = "";

  constructor(origin: string, ip: string) {
    this.origin = origin;
    this.ip = ip;
  }

  /** `POST /api/pair/start` with a fresh key. Returns the user code the account approves. */
  async start(machineName: string): Promise<string> {
    const challenge = createHash("sha256").update(this.verifier).digest();
    const started = await publicPost(this.origin, "/api/pair/start", this.ip, {
      publicKey: base64urlEncode(generateKeyPair().publicKey),
      machineName,
      os: "linux",
      appVersion: "r12-test",
      challenge: base64urlEncode(challenge),
    });
    if (started.status !== 201) throw new Error(`pair/start answered ${started.status}`);
    this.userCode = String(started.json["userCode"]);
    this.deviceCode = String(started.json["deviceCode"]);
    return this.userCode;
  }

  /** `POST /api/pair/poll`. */
  poll(): Promise<{ status: number; json: Record<string, any> }> { // eslint-disable-line @typescript-eslint/no-explicit-any
    return publicPost(this.origin, "/api/pair/poll", this.ip, { deviceCode: this.deviceCode, verifier: base64urlEncode(this.verifier) });
  }
}

/** Connects a desktop socket with a bearer token. Resolves with the socket and its close code (once known). */
export async function connectDesktop(origin: string, desktopId: string, token: string): Promise<{ socket: FrameSocket; closeCode: () => number | null }> {
  const socket = await connectRelay(`${origin.replace(/^http/, "ws")}/relay/desktop/${desktopId}`, { Authorization: `Bearer ${token}` });
  let code: number | null = null;
  socket.onclose = (value) => {
    code = value;
  };
  return { socket, closeCode: () => code };
}

/** Credential leaks in everything this process printed (stdout lines and stderr); see `findCredentialLeaks`. */
export function leaksOf(desktop: HeadlessDesktop): string[] {
  return findCredentialLeaks(`${desktop.stdoutText()}\n${desktop.stderrAll()}`);
}

/**
 * Files of a desktop data dir that hold or seal the credentials, with a mode other than 0600. Empty on Windows, where
 * POSIX modes mean nothing. A missing file is a problem too: the token is sealed in `store.db` with the key beside it.
 */
export function modeProblems(dataDir: string): string[] {
  if (process.platform === "win32") return [];
  const problems: string[] = [];
  for (const name of ["store.db", "credentials.key"]) {
    try {
      const mode = statSync(join(dataDir, name)).mode & 0o777;
      if (mode !== 0o600) problems.push(`${name} has mode ${mode.toString(8)}, expected 600`);
    } catch {
      problems.push(`${name} is missing`);
    }
  }
  return problems;
}
