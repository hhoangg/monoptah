// The desktop half of the web terminal for tests: the wire contract of `terminal.open/read/write/resize/close`
// (web/protocol/src/terminal.ts) over a fake shell, no PTY and no process. The shell echoes what is typed, runs a
// few commands (`echo`, `whoami`, `pwd`, `flood`, `exit`) and ends like the real desktop does: with an `exited` reason.
//
// Rules the real desktop keeps and this mock keeps with them: a one-time code per open, shown only through the test hook
// (`lastCode()`), 3 attempts (the attempt that uses the last one up answers `reauth_required`), a time to live, single use,
// case-insensitive input; a terminal belongs to the device that opened it (another device gets `not_found`); a shell survives
// a dropped link for `graceMs` and is killed after it; a killed terminal stays readable for `killedReadableMs` and is then
// `not_found`; reads are long polls answered as soon as output exists. Not mocked: the lockout after repeated failures.
// No node imports: this package also runs in the browser (the dev preview of the app).
import {
  TERMINAL_CODE_ATTEMPTS,
  TERMINAL_GRACE_MS,
  TERMINAL_IDLE_TIMEOUT_MS,
  TERMINAL_KILLED_READABLE_MS,
  TERMINAL_MAX_DURATION_MS,
  TERMINAL_READ_MAX_BYTES,
  TERMINAL_READ_WAIT_DEFAULT_MS,
  TERMINAL_CODE_TTL_MS,
  USER_CODE_ALPHABET,
  isSafeTerminalSubdir,
  type TerminalExited,
  type TerminalExitReason,
} from "@monoptah/protocol";
import { FixtureError } from "./fixtures.ts";

/** The clock a `MockTerminals` runs on (the mock desktop passes its own, so fake timers work). */
export interface TerminalClock {
  now(): number;
  setTimeout(fn: () => void, ms: number): unknown;
  clearTimeout(handle: unknown): void;
}

const realClock: TerminalClock = {
  now: () => Date.now(),
  setTimeout: (fn, ms) => globalThis.setTimeout(fn, ms),
  clearTimeout: (handle) => globalThis.clearTimeout(handle as ReturnType<typeof setTimeout>),
};

export interface MockTerminalOptions {
  clock?: TerminalClock;
  idleTimeoutMs?: number;
  maxDurationMs?: number;
  graceMs?: number;
  killedReadableMs?: number;
  challengeTtlMs?: number;
  /** Terminals one device may have open (default 2; more answers `busy`). */
  maxTerminals?: number;
  /** Open challenges one device may hold (default 4; more answers `busy`). */
  maxChallenges?: number;
  /** Replaces the random code, for a test that needs a known one. */
  codeGenerator?: () => string;
  /** Lines `flood` prints (default 3000, about 250 KB). */
  floodLines?: number;
}

interface Challenge {
  id: string;
  code: string;
  device: string;
  projectId: string;
  projectName: string;
  subdir: string | undefined;
  cols: number;
  rows: number;
  attemptsLeft: number;
  expiresAt: number;
}

interface Waiter {
  resolve: (value: ReadResult) => void;
  timer: unknown;
  from: number;
  maxBytes: number;
}

export type ReadResult = {
  from: number;
  next: number;
  data: string;
  exited: TerminalExited | null;
};

const encoder = new TextEncoder();

function toBase64(bytes: Uint8Array): string {
  let binary = "";
  for (const byte of bytes) binary += String.fromCharCode(byte);
  return btoa(binary);
}

function fromBase64(text: string): Uint8Array {
  return Uint8Array.from(atob(text), (char) => char.charCodeAt(0));
}

function randomId(prefix: string): string {
  const bytes = crypto.getRandomValues(new Uint8Array(9));
  return `${prefix}-${[...bytes].map((byte) => byte.toString(16).padStart(2, "0")).join("")}`;
}

function randomCode(): string {
  const bytes = crypto.getRandomValues(new Uint8Array(6));
  // 256 mod 31 is not 0; the bias does not matter for a mock.
  return [...bytes].map((byte) => USER_CODE_ALPHABET[byte % USER_CODE_ALPHABET.length]).join("");
}

/** One fake shell session. */
class MockTerminal {
  readonly id: string;
  readonly device: string;
  readonly cwd: string;
  cols: number;
  rows: number;
  /** Output not yet acknowledged: `buffer[0]` is the byte at absolute offset `base`. */
  #buffer: Uint8Array = new Uint8Array(0);
  #base = 0;
  exited: TerminalExited | null = null;
  waiter: Waiter | null = null;
  idleTimer: unknown = null;
  maxTimer: unknown = null;
  graceTimer: unknown = null;
  removeTimer: unknown = null;
  /** Everything typed, for tests. */
  readonly received: string[] = [];

  constructor(id: string, device: string, cwd: string, cols: number, rows: number) {
    this.id = id;
    this.device = device;
    this.cwd = cwd;
    this.cols = cols;
    this.rows = rows;
  }

  get total(): number {
    return this.#base + this.#buffer.length;
  }

  emit(text: string): void {
    const bytes = encoder.encode(text);
    const next = new Uint8Array(this.#buffer.length + bytes.length);
    next.set(this.#buffer);
    next.set(bytes, this.#buffer.length);
    this.#buffer = next;
  }

  #typed = "";
  line(): string {
    return this.#typed;
  }
  addChar(char: string): void {
    this.#typed += char;
  }
  dropChar(): boolean {
    if (this.#typed.length === 0) return false;
    this.#typed = this.#typed.slice(0, -1);
    return true;
  }
  clearLine(): void {
    this.#typed = "";
  }

  /** Bytes from absolute offset `from` (the first not yet received), at most `max`; acknowledges everything before `from`. */
  take(from: number, max: number): { from: number; bytes: Uint8Array } {
    const start = Math.min(Math.max(from, this.#base), this.total);
    this.#buffer = this.#buffer.slice(start - this.#base);
    this.#base = start;
    return { from: start, bytes: this.#buffer.slice(0, max) };
  }
}

export class MockTerminals {
  readonly #clock: TerminalClock;
  readonly #idleMs: number;
  readonly #maxMs: number;
  readonly #graceMs: number;
  readonly #killedReadableMs: number;
  readonly #challengeTtlMs: number;
  readonly #maxTerminals: number;
  readonly #maxChallenges: number;
  readonly #codeGenerator: () => string;
  readonly #floodLines: number;
  readonly #challenges = new Map<string, Challenge>();
  readonly #terminals = new Map<string, MockTerminal>();
  #lastCode: string | null = null;
  /** Called with every code the desktop would show (the test hook of the desktop window). */
  onCode: ((code: string, challengeId: string) => void) | null = null;

  constructor(options: MockTerminalOptions = {}) {
    this.#clock = options.clock ?? realClock;
    this.#idleMs = options.idleTimeoutMs ?? TERMINAL_IDLE_TIMEOUT_MS;
    this.#maxMs = options.maxDurationMs ?? TERMINAL_MAX_DURATION_MS;
    this.#graceMs = options.graceMs ?? TERMINAL_GRACE_MS;
    this.#killedReadableMs = options.killedReadableMs ?? TERMINAL_KILLED_READABLE_MS;
    this.#challengeTtlMs = options.challengeTtlMs ?? TERMINAL_CODE_TTL_MS;
    this.#maxTerminals = options.maxTerminals ?? 2;
    this.#maxChallenges = options.maxChallenges ?? 4;
    this.#codeGenerator = options.codeGenerator ?? randomCode;
    this.#floodLines = options.floodLines ?? 3000;
  }

  // --- Test hooks ---

  /** The code of the latest open challenge, as the desktop window shows it; null when none is open. */
  lastCode(): string | null {
    return this.#lastCode;
  }

  /** The ids of the live (not ended) terminals. */
  liveIds(): string[] {
    return [...this.#terminals.values()].filter((t) => t.exited === null).map((t) => t.id);
  }

  /** What was typed into a terminal, one entry per write. */
  received(id: string): string[] {
    return this.#terminals.get(id)?.received ?? [];
  }

  size(id: string): { cols: number; rows: number } | null {
    const terminal = this.#terminals.get(id);
    return terminal ? { cols: terminal.cols, rows: terminal.rows } : null;
  }

  /** Kills a terminal with a reason (revoke, idle, max, toggle off, the Kill button). */
  kill(id: string, reason: TerminalExitReason, code?: number): void {
    const terminal = this.#terminals.get(id);
    if (terminal) this.#end(terminal, reason, code);
  }

  killDevice(device: string, reason: TerminalExitReason): void {
    for (const terminal of this.#terminals.values()) if (terminal.device === device) this.#end(terminal, reason);
  }

  killAll(reason: TerminalExitReason): void {
    for (const terminal of this.#terminals.values()) this.#end(terminal, reason);
  }

  /** The link of the device dropped (no live session is left): its shells die after the grace window. */
  linkLost(device: string): void {
    for (const terminal of this.#terminals.values()) {
      if (terminal.device !== device || terminal.exited || terminal.graceTimer !== null) continue;
      terminal.graceTimer = this.#clock.setTimeout(() => {
        terminal.graceTimer = null;
        this.#end(terminal, "grace");
      }, this.#graceMs);
    }
  }

  // --- The methods ---

  open(
    device: string,
    params: { projectId: string; subdir?: string; cols: number; rows: number; challengeId?: string; code?: string },
    projectName: string,
  ): { challengeId: string; expiresInMs: number; codeLength: number; attempts: number } | OpenedTerminal {
    const { challengeId, code } = params;
    if (challengeId === undefined && code === undefined) return this.#challenge(device, params, projectName);
    if (challengeId === undefined || code === undefined) throw new FixtureError("A code needs its challenge", "reauth_required");
    return this.#redeem(device, params, challengeId, code);
  }

  #challenge(
    device: string,
    params: { projectId: string; subdir?: string; cols: number; rows: number },
    projectName: string,
  ): { challengeId: string; expiresInMs: number; codeLength: number; attempts: number } {
    if (params.subdir !== undefined && !isSafeTerminalSubdir(params.subdir)) throw new FixtureError("That folder is not allowed", "path_denied");
    this.#sweep();
    const open = [...this.#challenges.values()].filter((c) => c.device === device).length;
    if (open >= this.#maxChallenges) throw new FixtureError("Too many open codes", "busy");
    const live = [...this.#terminals.values()].filter((t) => t.device === device && t.exited === null).length;
    if (live >= this.#maxTerminals) throw new FixtureError("Too many terminals", "busy");
    const challenge: Challenge = {
      id: randomId("chal"),
      code: this.#codeGenerator(),
      device,
      projectId: params.projectId,
      projectName,
      subdir: params.subdir,
      cols: params.cols,
      rows: params.rows,
      attemptsLeft: TERMINAL_CODE_ATTEMPTS,
      expiresAt: this.#clock.now() + this.#challengeTtlMs,
    };
    this.#challenges.set(challenge.id, challenge);
    this.#lastCode = challenge.code;
    this.onCode?.(challenge.code, challenge.id);
    return { challengeId: challenge.id, expiresInMs: this.#challengeTtlMs, codeLength: challenge.code.length, attempts: TERMINAL_CODE_ATTEMPTS };
  }

  #redeem(
    device: string,
    params: { projectId: string; subdir?: string; cols: number; rows: number },
    challengeId: string,
    code: string,
  ): OpenedTerminal {
    this.#sweep();
    const challenge = this.#challenges.get(challengeId);
    if (!challenge || challenge.device !== device || challenge.projectId !== params.projectId || challenge.subdir !== params.subdir) {
      throw new FixtureError("The code expired. Start again.", "reauth_required");
    }
    if (code.trim().toUpperCase() !== challenge.code) {
      challenge.attemptsLeft -= 1;
      if (challenge.attemptsLeft <= 0) {
        this.#challenges.delete(challengeId);
        if (this.#lastCode === challenge.code) this.#lastCode = null;
        throw new FixtureError("Too many wrong codes. Start again.", "reauth_required");
      }
      throw new FixtureError("That code is not right", "reauth_failed");
    }
    this.#challenges.delete(challengeId);
    if (this.#lastCode === challenge.code) this.#lastCode = null;
    const live = [...this.#terminals.values()].filter((t) => t.device === device && t.exited === null).length;
    if (live >= this.#maxTerminals) throw new FixtureError("Too many terminals", "busy");
    const cwd = challenge.subdir ? `${challenge.projectName}/${challenge.subdir}` : challenge.projectName;
    const terminal = new MockTerminal(randomId("term"), device, cwd, challenge.cols, challenge.rows);
    terminal.emit(`Monoptah mock shell in ${cwd}\r\n$ `);
    this.#terminals.set(terminal.id, terminal);
    this.#armIdle(terminal);
    terminal.maxTimer = this.#clock.setTimeout(() => this.#end(terminal, "max"), this.#maxMs);
    return { terminalId: terminal.id, cols: terminal.cols, rows: terminal.rows, cwd, idleTimeoutMs: this.#idleMs, maxDurationMs: this.#maxMs };
  }

  read(device: string, params: { terminalId: string; from: number; waitMs?: number; maxBytes?: number }): Promise<ReadResult> {
    const terminal = this.#own(device, params.terminalId);
    // A read is a sign of life: a link that came back reattaches the shell.
    if (terminal.graceTimer !== null) {
      this.#clock.clearTimeout(terminal.graceTimer);
      terminal.graceTimer = null;
    }
    // A newer read replaces an older one, which answers empty (a client never has two in flight; a stale one must not hang).
    if (terminal.waiter) this.#settle(terminal, terminal.waiter);
    const maxBytes = Math.min(params.maxBytes ?? TERMINAL_READ_MAX_BYTES, TERMINAL_READ_MAX_BYTES);
    const waitMs = params.waitMs ?? TERMINAL_READ_WAIT_DEFAULT_MS;
    const ready = this.#answer(terminal, params.from, maxBytes);
    if (ready.data !== "" || ready.exited !== null || waitMs === 0) return Promise.resolve(ready);
    return new Promise<ReadResult>((resolve) => {
      const waiter: Waiter = { resolve, from: params.from, maxBytes, timer: null };
      waiter.timer = this.#clock.setTimeout(() => this.#settle(terminal, waiter), waitMs);
      terminal.waiter = waiter;
    });
  }

  write(device: string, params: { terminalId: string; data: string }): Record<string, never> {
    const terminal = this.#live(device, params.terminalId);
    const text = new TextDecoder().decode(fromBase64(params.data));
    terminal.received.push(text);
    this.#armIdle(terminal);
    for (const char of text) this.#type(terminal, char);
    this.#wake(terminal);
    return {};
  }

  resize(device: string, params: { terminalId: string; cols: number; rows: number }): Record<string, never> {
    const terminal = this.#live(device, params.terminalId);
    terminal.cols = params.cols;
    terminal.rows = params.rows;
    return {};
  }

  close(device: string, params: { terminalId: string }): Record<string, never> {
    const terminal = this.#own(device, params.terminalId);
    if (terminal.exited === null) this.#end(terminal, "closed");
    return {};
  }

  // --- Internals ---

  #own(device: string, id: string): MockTerminal {
    const terminal = this.#terminals.get(id);
    if (!terminal || terminal.device !== device) throw new FixtureError("No such terminal", "not_found");
    return terminal;
  }

  #live(device: string, id: string): MockTerminal {
    const terminal = this.#own(device, id);
    if (terminal.exited !== null) throw new FixtureError("The terminal has ended", "not_found");
    return terminal;
  }

  #sweep(): void {
    const now = this.#clock.now();
    for (const [id, challenge] of this.#challenges) {
      if (challenge.expiresAt <= now) {
        this.#challenges.delete(id);
        if (this.#lastCode === challenge.code) this.#lastCode = null;
      }
    }
  }

  #armIdle(terminal: MockTerminal): void {
    if (terminal.idleTimer !== null) this.#clock.clearTimeout(terminal.idleTimer);
    terminal.idleTimer = this.#clock.setTimeout(() => this.#end(terminal, "idle"), this.#idleMs);
  }

  #answer(terminal: MockTerminal, from: number, maxBytes: number): ReadResult {
    const { from: start, bytes } = terminal.take(from, maxBytes);
    const next = start + bytes.length;
    const drained = next >= terminal.total;
    return { from: start, next, data: toBase64(bytes), exited: drained ? terminal.exited : null };
  }

  #settle(terminal: MockTerminal, waiter: Waiter): void {
    if (terminal.waiter !== waiter) return;
    this.#clock.clearTimeout(waiter.timer);
    terminal.waiter = null;
    waiter.resolve(this.#answer(terminal, waiter.from, waiter.maxBytes));
  }

  #wake(terminal: MockTerminal): void {
    const waiter = terminal.waiter;
    if (waiter && (terminal.total > waiter.from || terminal.exited !== null)) this.#settle(terminal, waiter);
  }

  #end(terminal: MockTerminal, reason: TerminalExitReason, code?: number): void {
    if (terminal.exited !== null) return;
    terminal.exited = { reason, ...(code === undefined ? {} : { code }) };
    for (const timer of [terminal.idleTimer, terminal.maxTimer, terminal.graceTimer]) if (timer !== null) this.#clock.clearTimeout(timer);
    terminal.idleTimer = terminal.maxTimer = terminal.graceTimer = null;
    this.#wake(terminal);
    // A killed terminal stays readable so the last message arrives; then it is forgotten.
    terminal.removeTimer = this.#clock.setTimeout(() => this.#terminals.delete(terminal.id), this.#killedReadableMs);
  }

  /** The fake shell: a line editor with a handful of commands. */
  #type(terminal: MockTerminal, char: string): void {
    if (terminal.exited !== null) return;
    if (char === "\r" || char === "\n") {
      const line = terminal.line();
      terminal.emit("\r\n");
      this.#run(terminal, line.trim());
      return;
    }
    if (char === "\x7f" || char === "\b") {
      if (terminal.dropChar()) terminal.emit("\b \b");
      return;
    }
    if (char === "\x03") {
      terminal.clearLine();
      terminal.emit("^C\r\n$ ");
      return;
    }
    if (char === "\x1b" || char < " ") return;
    terminal.addChar(char);
    terminal.emit(char);
  }

  #run(terminal: MockTerminal, line: string): void {
    terminal.clearLine();
    const [command = "", ...rest] = line.split(/\s+/);
    switch (command) {
      case "":
        break;
      case "exit":
        terminal.emit("logout\r\n");
        this.#end(terminal, "exit", 0);
        return;
      case "flood":
        for (let i = 1; i <= this.#floodLines; i++) terminal.emit(`flood line ${String(i).padStart(5, "0")} ${"x".repeat(60)}\r\n`);
        terminal.emit("flood done\r\n");
        break;
      case "echo":
        terminal.emit(`${rest.join(" ")}\r\n`);
        break;
      case "whoami":
        terminal.emit("example-user\r\n");
        break;
      case "pwd":
        terminal.emit(`/example/${terminal.cwd}\r\n`);
        break;
      default:
        terminal.emit(`sh: ${command}: command not found\r\n`);
    }
    terminal.emit("$ ");
  }
}

export type OpenedTerminal = {
  terminalId: string;
  cols: number;
  rows: number;
  cwd: string;
  idleTimeoutMs: number;
  maxDurationMs: number;
};
