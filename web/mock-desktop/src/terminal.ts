// The desktop half of the web terminal for tests: the wire contract of `terminal.open/read/write/resize/close`
// (web/protocol/src/terminal.ts) over a fake shell, no PTY and no process. The shell echoes what is typed, runs a
// few commands (`echo`, `whoami`, `pwd`, `flood`, `exit`) and ends like the real desktop does: with an `exited` reason.
//
// Rules the real desktop keeps and this mock keeps with them: one step opens a terminal when the one-time ticket checks out
// (the desktop spends it at the Worker; the mock asks `verifyTicket`, which a test sets, and fails closed with `fresh_required`
// when none is set or it refuses); a terminal belongs to the device that opened it (another device gets `not_found`); a shell
// survives a dropped link for `graceMs` and is killed after it; a killed terminal stays readable for `killedReadableMs` and is
// then `not_found`; reads are long polls answered as soon as output exists.
// No node imports: this package also runs in the browser (the dev preview of the app).
import {
  TERMINAL_GRACE_MS,
  TERMINAL_IDLE_TIMEOUT_MS,
  TERMINAL_KILLED_READABLE_MS,
  TERMINAL_MAX_DURATION_MS,
  TERMINAL_READ_MAX_BYTES,
  TERMINAL_READ_WAIT_DEFAULT_MS,
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

/** Checks a one-time terminal ticket for a project (the desktop's call to the Worker); true when accepted. */
export type TicketVerifier = (ticket: string, projectId: string) => Promise<boolean>;

export interface MockTerminalOptions {
  clock?: TerminalClock;
  idleTimeoutMs?: number;
  maxDurationMs?: number;
  graceMs?: number;
  killedReadableMs?: number;
  /** Terminals one device may have open (default 2; more answers `busy`). */
  maxTerminals?: number;
  /** Spends a terminal ticket as the desktop does at the Worker; true when accepted. Without one every open is refused. */
  ticketVerifier?: TicketVerifier;
  /** Lines `flood` prints (default 3000, about 250 KB). */
  floodLines?: number;
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
  readonly #maxTerminals: number;
  readonly #floodLines: number;
  readonly #terminals = new Map<string, MockTerminal>();
  #verifications = 0;
  /** Test hook: how the desktop spends a ticket at the Worker. null (the default) refuses every open (fail closed). */
  verifyTicket: TicketVerifier | null;

  constructor(options: MockTerminalOptions = {}) {
    this.#clock = options.clock ?? realClock;
    this.#idleMs = options.idleTimeoutMs ?? TERMINAL_IDLE_TIMEOUT_MS;
    this.#maxMs = options.maxDurationMs ?? TERMINAL_MAX_DURATION_MS;
    this.#graceMs = options.graceMs ?? TERMINAL_GRACE_MS;
    this.#killedReadableMs = options.killedReadableMs ?? TERMINAL_KILLED_READABLE_MS;
    this.#maxTerminals = options.maxTerminals ?? 2;
    this.verifyTicket = options.ticketVerifier ?? null;
    this.#floodLines = options.floodLines ?? 3000;
  }

  // --- Test hooks ---

  /** How many times the desktop asked the verifier to spend a ticket (accepted or refused). */
  ticketVerifications(): number {
    return this.#verifications;
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

  async open(
    device: string,
    params: { projectId: string; subdir?: string; cols: number; rows: number; ticket: string },
    projectName: string,
  ): Promise<OpenedTerminal> {
    if (params.subdir !== undefined && !isSafeTerminalSubdir(params.subdir)) throw new FixtureError("That folder is not allowed", "path_denied");
    if (this.#liveCount(device) >= this.#maxTerminals) throw new FixtureError("Too many terminals", "busy");
    const refused = new FixtureError("Confirm with your passkey to open a terminal", "fresh_required");
    const verify = this.verifyTicket;
    if (verify === null || params.ticket === "") throw refused;
    this.#verifications += 1;
    let accepted: boolean;
    try {
      accepted = await verify(params.ticket, params.projectId);
    } catch {
      accepted = false;
    }
    if (!accepted) throw refused;
    // The wait may have let other opens in.
    if (this.#liveCount(device) >= this.#maxTerminals) throw new FixtureError("Too many terminals", "busy");
    const cwd = params.subdir ? `${projectName}/${params.subdir}` : projectName;
    const terminal = new MockTerminal(randomId("term"), device, cwd, params.cols, params.rows);
    terminal.emit(`Monoptah mock shell in ${cwd}\r\n$ `);
    this.#terminals.set(terminal.id, terminal);
    this.#armIdle(terminal);
    terminal.maxTimer = this.#clock.setTimeout(() => this.#end(terminal, "max"), this.#maxMs);
    return { terminalId: terminal.id, cols: terminal.cols, rows: terminal.rows, cwd, idleTimeoutMs: this.#idleMs, maxDurationMs: this.#maxMs };
  }

  #liveCount(device: string): number {
    return [...this.#terminals.values()].filter((t) => t.device === device && t.exited === null).length;
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
