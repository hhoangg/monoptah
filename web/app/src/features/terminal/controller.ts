// The terminal session as plain logic (security.md "Terminal", features.md N-26): the two step open with the one-time code,
// the read loop, the write queue, resize, close, the end of a session and the way back after a dropped link. It talks to
// two ports (port.ts), so it runs in a test with a fake link and a fake screen and needs neither xterm nor a canvas.
//
// Rules this class keeps (they come from the relay limits, 40 msg/s sustained per device, and from the desktop):
//  - at most ONE `terminal.read` and at most ONE `terminal.write` in flight;
//  - keystrokes typed while a write is in flight go out together in the next one (at most one write per flush window);
//  - at least TERMINAL_READ_MIN_GAP_MS between the end of a read that carried data and the next read;
//  - output reaches the screen in slices, and the next read waits until the screen has parsed them (backpressure);
//  - pending input is capped (64 KiB) and dropped with a visible hint, never queued without bound;
//  - a read asks again from `next` after a reconnect: the desktop keeps the shell for a grace window.
// Terminal bytes live in memory only. Nothing here writes them to storage; the resume entry holds ids and offsets.
import {
  TERMINAL_COLS_MAX,
  TERMINAL_COLS_MIN,
  TERMINAL_PENDING_INPUT_MAX_BYTES,
  TERMINAL_READ_MAX_BYTES,
  TERMINAL_READ_MIN_GAP_MS,
  TERMINAL_READ_TIMEOUT_SLACK_MS,
  TERMINAL_READ_WAIT_DEFAULT_MS,
  TERMINAL_RENDER_SLICE_BYTES,
  TERMINAL_ROWS_MAX,
  TERMINAL_ROWS_MIN,
  TERMINAL_WRITE_CHUNK_BYTES,
  TERMINAL_WRITE_FLUSH_MS,
  TerminalReadResultSchema,
  isSafeTerminalSubdir,
  isTerminalOpened,
  type TerminalOpened,
  type TerminalReadResult,
} from "@monoptah/protocol";
import {
  CODE_RESTART_NOTICE,
  INPUT_DROPPED_NOTICE,
  INPUT_LOST_NOTICE,
  endedText,
  openFailedText,
  wrongCodeNotice,
} from "./copy";
import { chunkEnd, fromBase64, toBase64, utf8 } from "./input";
import type { TerminalLinkPort, TerminalScreen, TerminalSize } from "./port";
import type { ResumeEntry, ResumeStore } from "./resume";

export type TerminalPhase = "idle" | "starting" | "code" | "verifying" | "running" | "reconnecting" | "ended" | "failed";

export interface CodeChallenge {
  challengeId: string;
  /** Epoch ms the code stops working. */
  expiresAt: number;
  codeLength: number;
  attempts: number;
  attemptsLeft: number;
}

export interface TerminalSession {
  terminalId: string;
  /** A label for the screen: the project name plus the folder. */
  cwd: string;
  idleTimeoutMs: number;
  maxDurationMs: number;
  openedAt: number;
}

export interface TerminalEnded {
  reason: string;
  code?: number | undefined;
  /** "Session ended: <this>" without the prefix. */
  text: string;
}

export interface TerminalSnapshot {
  phase: TerminalPhase;
  challenge: CodeChallenge | null;
  /** The code of the shown challenge ran out of time: ask for a new one. */
  codeExpired: boolean;
  /** A soft message (a wrong code, a restart); an error that ends an attempt is `error`. */
  notice: string | null;
  error: string | null;
  session: TerminalSession | null;
  ended: TerminalEnded | null;
  /** Input was dropped or may be lost; clears by itself. */
  inputNotice: string | null;
}

export interface OpenTarget {
  projectId: string;
  /** Relative folder inside the project; empty means the project root. */
  subdir?: string | undefined;
}

export interface TerminalControllerOptions {
  link: TerminalLinkPort;
  store?: ResumeStore | null;
  now?: () => number;
}

const IDLE: TerminalSnapshot = { phase: "idle", challenge: null, codeExpired: false, notice: null, error: null, session: null, ended: null, inputNotice: null };

const DEFAULT_SIZE: TerminalSize = { cols: 80, rows: 24 };
const RESIZE_DEBOUNCE_MS = 150;
const PERSIST_EVERY_MS = 250;
const BUSY_RETRY_MS = 1_000;
const RETRY_BACKOFF_MS = [500, 1_000, 2_000, 4_000, 5_000] as const;
const MAX_CONSECUTIVE_FAILURES = 6;
const INPUT_NOTICE_MS = 5_000;

const clamp = (value: number, min: number, max: number): number => Math.min(Math.max(Math.round(value), min), max);

function codeOf(error: unknown): string | undefined {
  if (typeof error === "object" && error !== null && "code" in error) {
    const code = (error as { code?: unknown }).code;
    return typeof code === "string" ? code : undefined;
  }
  return undefined;
}

export class TerminalController {
  readonly #link: TerminalLinkPort;
  readonly #store: ResumeStore | null;
  readonly #now: () => number;
  readonly #listeners = new Set<() => void>();
  readonly #sleepers = new Set<() => void>();
  #snapshot: TerminalSnapshot = IDLE;
  #screen: TerminalScreen | null = null;
  #screenWaiters: (() => void)[] = [];
  #unsubscribeReady: (() => void) | null = null;
  /** Bumped to stop every loop of the current run (end, dispose, a new open). */
  #run = 0;
  #target: OpenTarget | null = null;
  #expiryTimer: ReturnType<typeof setTimeout> | null = null;
  #resumed = false;
  #wasDisconnected = false;
  #started = false;

  // read side
  #next = 0;
  #readAbort: AbortController | null = null;

  // write side
  #pending: Uint8Array = new Uint8Array(0);
  #writing = false;
  #lastWriteAt = 0;
  #flushTimer: ReturnType<typeof setTimeout> | null = null;
  #noticeTimer: ReturnType<typeof setTimeout> | null = null;

  // resize and persistence
  #size: TerminalSize = DEFAULT_SIZE;
  #sentSize: TerminalSize | null = null;
  #resizeTimer: ReturnType<typeof setTimeout> | null = null;
  #resizing = false;
  #persistTimer: ReturnType<typeof setTimeout> | null = null;

  constructor(options: TerminalControllerOptions) {
    this.#link = options.link;
    this.#store = options.store ?? null;
    this.#now = options.now ?? Date.now;
  }

  // --- Store for the UI ---

  getSnapshot = (): TerminalSnapshot => this.#snapshot;

  subscribe = (listener: () => void): (() => void) => {
    this.#listeners.add(listener);
    return () => this.#listeners.delete(listener);
  };

  #set(patch: Partial<TerminalSnapshot>): void {
    this.#snapshot = { ...this.#snapshot, ...patch };
    for (const listener of [...this.#listeners]) listener();
  }

  // --- Screen ---

  /** Hands over the screen. The read loop starts only after this, so no output is read before it can be drawn. */
  attachScreen(screen: TerminalScreen): void {
    this.#screen = screen;
    this.#size = this.#clampSize(screen.size());
    const waiters = this.#screenWaiters;
    this.#screenWaiters = [];
    for (const wake of waiters) wake();
  }

  detachScreen(): void {
    this.#screen = null;
  }

  #screenReady(): Promise<void> {
    if (this.#screen) return Promise.resolve();
    return new Promise((resolve) => {
      this.#screenWaiters.push(resolve);
      this.#sleepers.add(resolve);
    });
  }

  #clampSize(size: TerminalSize): TerminalSize {
    return { cols: clamp(size.cols, TERMINAL_COLS_MIN, TERMINAL_COLS_MAX), rows: clamp(size.rows, TERMINAL_ROWS_MIN, TERMINAL_ROWS_MAX) };
  }

  // --- Opening (two steps, a new code on every open) ---

  /** Step 1: asks the desktop for a code. It shows the code on its own screen; the user types it in `submitCode`. */
  async begin(target: OpenTarget): Promise<void> {
    const subdir = target.subdir?.trim() ? target.subdir.trim() : undefined;
    if (subdir !== undefined && !isSafeTerminalSubdir(subdir)) {
      this.#set({ ...IDLE, phase: "failed", error: openFailedText("path_denied") });
      return;
    }
    this.#stopRun();
    this.#target = { projectId: target.projectId, ...(subdir === undefined ? {} : { subdir }) };
    this.#resumed = false;
    this.#wasDisconnected = false;
    await this.#requestChallenge(null);
  }

  async #requestChallenge(notice: string | null): Promise<void> {
    const target = this.#target;
    if (!target) return;
    const run = this.#run;
    this.#clearExpiry();
    this.#set({ ...IDLE, phase: "starting", notice });
    try {
      const result = await this.#link.request("terminal.open", { ...target, ...this.#size });
      if (run !== this.#run) return;
      if (isTerminalOpened(result)) {
        // No code was asked for. Never open a shell without the re-auth step: treat it as a failure.
        this.#set({ ...IDLE, phase: "failed", error: openFailedText(undefined) });
        return;
      }
      const attempts = result.attempts;
      this.#set({
        phase: "code",
        notice,
        error: null,
        codeExpired: false,
        challenge: { challengeId: result.challengeId, expiresAt: this.#now() + result.expiresInMs, codeLength: result.codeLength, attempts, attemptsLeft: attempts },
      });
      this.#armExpiry(result.expiresInMs, run);
    } catch (error) {
      if (run !== this.#run) return;
      this.#set({ ...IDLE, phase: "failed", error: openFailedText(codeOf(error)) });
    }
  }

  #armExpiry(ms: number, run: number): void {
    this.#clearExpiry();
    this.#expiryTimer = setTimeout(() => {
      this.#expiryTimer = null;
      if (run === this.#run && this.#snapshot.phase === "code") this.#set({ codeExpired: true });
    }, ms);
  }

  #clearExpiry(): void {
    if (this.#expiryTimer !== null) clearTimeout(this.#expiryTimer);
    this.#expiryTimer = null;
  }

  /** The user asks for a new code (the old one ran out of time). */
  async restart(): Promise<void> {
    if (this.#snapshot.phase !== "code" && this.#snapshot.phase !== "failed") return;
    await this.#requestChallenge(null);
  }

  /** Step 2: the code the user read on the desktop. A code of the wrong shape is not sent: it would burn an attempt. */
  async submitCode(raw: string): Promise<void> {
    const challenge = this.#snapshot.challenge;
    const target = this.#target;
    if (this.#snapshot.phase !== "code" || !challenge || !target || this.#snapshot.codeExpired) return;
    const code = raw.replace(/\s+/g, "").toUpperCase();
    if (code.length !== challenge.codeLength) {
      this.#set({ notice: `Enter all ${challenge.codeLength} characters of the code.` });
      return;
    }
    const run = this.#run;
    this.#clearExpiry();
    this.#set({ phase: "verifying", notice: null });
    try {
      const result = await this.#link.request("terminal.open", { ...target, ...this.#size, challengeId: challenge.challengeId, code });
      if (run !== this.#run) return;
      if (!isTerminalOpened(result)) {
        this.#set({ ...IDLE, phase: "failed", error: openFailedText(undefined) });
        return;
      }
      this.#startSession(result);
    } catch (error) {
      if (run !== this.#run) return;
      const failure = codeOf(error);
      if (failure === "reauth_failed") {
        const attemptsLeft = Math.max(challenge.attemptsLeft - 1, 0);
        this.#set({ phase: "code", notice: wrongCodeNotice(attemptsLeft), challenge: { ...challenge, attemptsLeft } });
        this.#armExpiry(Math.max(challenge.expiresAt - this.#now(), 0), run);
        return;
      }
      if (failure === "reauth_required") {
        // Unknown, expired, used up or locked out: start again with a new code, once, for this typed attempt.
        await this.#requestChallenge(CODE_RESTART_NOTICE);
        return;
      }
      this.#set({ ...IDLE, phase: "failed", error: openFailedText(failure) });
    }
  }

  /** The user backs out of the code step. The desktop lets the code expire by itself. */
  cancel(): void {
    if (this.#snapshot.phase !== "code" && this.#snapshot.phase !== "starting" && this.#snapshot.phase !== "verifying") return;
    this.#stopRun();
    this.#set({ ...IDLE });
  }

  /** Back to the project chooser after a session ended or an open failed. */
  reset(): void {
    this.#stopRun();
    this.#store?.clear();
    this.#set({ ...IDLE });
  }

  // --- A running session ---

  #startSession(opened: TerminalOpened): void {
    this.#stopRun();
    const run = this.#run;
    const session: TerminalSession = {
      terminalId: opened.terminalId,
      cwd: opened.cwd,
      idleTimeoutMs: opened.idleTimeoutMs,
      maxDurationMs: opened.maxDurationMs,
      openedAt: this.#now(),
    };
    this.#next = 0;
    this.#size = { cols: opened.cols, rows: opened.rows };
    this.#sentSize = { cols: opened.cols, rows: opened.rows };
    this.#set({ ...IDLE, phase: "running", session });
    this.#persist();
    this.#begin(run);
  }

  /** Reattaches to a shell a reload left behind: the desktop keeps it for its grace window. */
  resume(entry: ResumeEntry): void {
    this.#stopRun();
    const run = this.#run;
    this.#resumed = true;
    this.#next = entry.next;
    this.#set({
      ...IDLE,
      phase: "reconnecting",
      session: { terminalId: entry.terminalId, cwd: entry.cwd, idleTimeoutMs: entry.idleTimeoutMs, maxDurationMs: entry.maxDurationMs, openedAt: entry.openedAt },
    });
    this.#begin(run);
  }

  #begin(run: number): void {
    this.#started = true;
    this.#unsubscribeReady = this.#link.subscribeReady((ready) => {
      if (ready) this.#scheduleFlush();
      this.#wakeSleepers();
    });
    void this.#readLoop(run);
  }

  // --- Read side ---

  async #readLoop(run: number): Promise<void> {
    let failures = 0;
    while (run === this.#run) {
      await this.#screenReady();
      if (run !== this.#run) return;
      if (!this.#link.isReady()) {
        this.#wasDisconnected = true;
        if (this.#snapshot.phase === "running") this.#set({ phase: "reconnecting" });
        await this.#waitReady(run);
        continue;
      }
      const session = this.#snapshot.session;
      if (!session) return;
      // The first read after a reload or a dropped link does not wait: it answers at once, which is how the page learns the
      // shell is still there ("running") instead of showing "Reconnecting" for a whole long poll.
      const waitMs = this.#snapshot.phase === "reconnecting" ? 0 : TERMINAL_READ_WAIT_DEFAULT_MS;
      const abort = new AbortController();
      this.#readAbort = abort;
      // The link times a request out after 35 s; a read is a long poll, so give up on a silent one after waitMs plus slack.
      const giveUp = setTimeout(() => abort.abort(), TERMINAL_READ_WAIT_DEFAULT_MS + TERMINAL_READ_TIMEOUT_SLACK_MS);
      let result: TerminalReadResult;
      try {
        const raw = await this.#link.request(
          "terminal.read",
          { terminalId: session.terminalId, from: this.#next, waitMs, maxBytes: TERMINAL_READ_MAX_BYTES },
          { signal: abort.signal },
        );
        const parsed = TerminalReadResultSchema.safeParse(raw);
        if (!parsed.success) throw new Error("bad read result");
        result = parsed.data;
      } catch (error) {
        clearTimeout(giveUp);
        this.#readAbort = null;
        if (run !== this.#run) return;
        const failure = codeOf(error);
        if (failure === "not_ready" || failure === "session_ended") {
          // The link dropped under the read: wait for it, then ask again from the same offset.
          this.#wasDisconnected = true;
          // The status may lag the failure by a moment: do not spin on a link that still says ready.
          if (this.#link.isReady()) await this.#sleep(250, run);
          continue;
        }
        const outcome = this.#readFailure(failure);
        if (outcome === "end") return;
        failures += 1;
        if (outcome === "busy") await this.#sleep(BUSY_RETRY_MS, run);
        else if (failures >= MAX_CONSECUTIVE_FAILURES) return this.#end("error");
        else await this.#sleep(RETRY_BACKOFF_MS[Math.min(failures - 1, RETRY_BACKOFF_MS.length - 1)] ?? 5_000, run);
        continue;
      }
      clearTimeout(giveUp);
      this.#readAbort = null;
      if (run !== this.#run) return;
      failures = 0;
      if (this.#snapshot.phase === "reconnecting") this.#set({ phase: "running" });
      const more = await this.#deliver(result, run);
      if (!more || run !== this.#run) return;
      // The relay allows 40 messages a second: while data keeps flowing, leave a gap between reads.
      if (result.data !== "") await this.#sleep(TERMINAL_READ_MIN_GAP_MS, run);
    }
  }

  /** What a failed read means: end the session, retry soon, or retry after a pause. */
  #readFailure(code: string | undefined): "end" | "retry" | "busy" {
    switch (code) {
      case "not_found":
        this.#end(this.#resumed || this.#wasDisconnected ? "grace" : "gone");
        return "end";
      case "disabled":
        this.#end("toggle_off");
        return "end";
      case "revoked":
      case "forbidden":
      case "read_only":
        this.#end("revoked");
        return "end";
      case "busy":
        return "busy";
      default:
        return "retry";
    }
  }

  /** Draws what a read returned in slices, waiting for the screen after each, then moves `next`. False when the session ended. */
  async #deliver(result: TerminalReadResult, run: number): Promise<boolean> {
    const decoded = fromBase64(result.data);
    if (decoded === null) {
      this.#end("error");
      return false;
    }
    // The desktop may start later than asked (a reattach after a gap) or, defensively, earlier: draw only what is new.
    const skip = result.from < this.#next ? Math.min(this.#next - result.from, decoded.length) : 0;
    const bytes = skip > 0 ? decoded.subarray(skip) : decoded;
    for (let at = 0; at < bytes.length; at += TERMINAL_RENDER_SLICE_BYTES) {
      await this.#draw(bytes.subarray(at, at + TERMINAL_RENDER_SLICE_BYTES), run);
      if (run !== this.#run) return false;
    }
    this.#next = Math.max(this.#next, result.next);
    this.#persistSoon();
    if (result.exited) {
      this.#end(result.exited.reason, result.exited.code);
      return false;
    }
    return true;
  }

  #draw(bytes: Uint8Array, run: number): Promise<void> {
    return new Promise((resolve) => {
      const screen = this.#screen;
      if (!screen || run !== this.#run) return resolve();
      this.#sleepers.add(resolve);
      screen.write(bytes, () => {
        this.#sleepers.delete(resolve);
        resolve();
      });
    });
  }

  // --- Write side ---

  /** Keys typed (a string from the keyboard) or bytes. Ignored when no session is up. */
  input(data: string | Uint8Array): void {
    const phase = this.#snapshot.phase;
    if (phase !== "running" && phase !== "reconnecting") return;
    const bytes = typeof data === "string" ? utf8(data) : data;
    if (bytes.length === 0) return;
    if (this.#pending.length + bytes.length > TERMINAL_PENDING_INPUT_MAX_BYTES) {
      this.#showInputNotice(INPUT_DROPPED_NOTICE);
      return;
    }
    const merged = new Uint8Array(this.#pending.length + bytes.length);
    merged.set(this.#pending);
    merged.set(bytes, this.#pending.length);
    this.#pending = merged;
    this.#scheduleFlush();
  }

  #scheduleFlush(): void {
    if (this.#writing || this.#flushTimer !== null || this.#pending.length === 0) return;
    if (!this.#started || this.#snapshot.session === null || !this.#link.isReady()) return;
    // One write per flush window: the first key of a burst goes out at once, the rest of it together afterwards.
    const wait = Math.max(0, this.#lastWriteAt + TERMINAL_WRITE_FLUSH_MS - this.#now());
    if (wait === 0) {
      void this.#flush();
      return;
    }
    this.#flushTimer = setTimeout(() => {
      this.#flushTimer = null;
      void this.#flush();
    }, wait);
  }

  async #flush(): Promise<void> {
    const session = this.#snapshot.session;
    const run = this.#run;
    if (this.#writing || !session || this.#pending.length === 0 || !this.#link.isReady()) return;
    const end = chunkEnd(this.#pending, TERMINAL_WRITE_CHUNK_BYTES);
    const chunk = this.#pending.slice(0, end);
    this.#pending = this.#pending.slice(end);
    this.#writing = true;
    this.#lastWriteAt = this.#now();
    try {
      await this.#link.request("terminal.write", { terminalId: session.terminalId, data: toBase64(chunk) });
    } catch (error) {
      this.#writing = false;
      if (run !== this.#run) return;
      const failure = codeOf(error);
      if (failure === "not_ready") {
        // Never left the browser: keep it for when the link is back.
        this.#pending = concat(chunk, this.#pending);
        return;
      }
      if (failure === "busy") {
        this.#pending = concat(chunk, this.#pending);
        this.#flushTimer = setTimeout(() => {
          this.#flushTimer = null;
          this.#scheduleFlush();
        }, BUSY_RETRY_MS);
        return;
      }
      if (failure === "not_found") return this.#end(this.#resumed || this.#wasDisconnected ? "grace" : "gone");
      if (failure === "disabled") return this.#end("toggle_off");
      if (failure === "revoked" || failure === "forbidden" || failure === "read_only") return this.#end("revoked");
      // Any other failure after the request left: it may have been typed or not. Never send it twice.
      this.#showInputNotice(INPUT_LOST_NOTICE);
      this.#scheduleFlush();
      return;
    }
    this.#writing = false;
    if (run === this.#run) this.#scheduleFlush();
  }

  #showInputNotice(text: string): void {
    this.#set({ inputNotice: text });
    if (this.#noticeTimer !== null) clearTimeout(this.#noticeTimer);
    this.#noticeTimer = setTimeout(() => {
      this.#noticeTimer = null;
      this.#set({ inputNotice: null });
    }, INPUT_NOTICE_MS);
  }

  // --- Resize ---

  resize(cols: number, rows: number): void {
    const size = this.#clampSize({ cols, rows });
    this.#size = size;
    if (!this.#snapshot.session || (this.#sentSize && this.#sentSize.cols === size.cols && this.#sentSize.rows === size.rows)) return;
    if (this.#resizeTimer !== null) clearTimeout(this.#resizeTimer);
    this.#resizeTimer = setTimeout(() => {
      this.#resizeTimer = null;
      void this.#sendResize();
    }, RESIZE_DEBOUNCE_MS);
  }

  async #sendResize(): Promise<void> {
    const session = this.#snapshot.session;
    if (!session || this.#resizing) return;
    if (!this.#link.isReady()) return;
    const size = this.#size;
    if (this.#sentSize && this.#sentSize.cols === size.cols && this.#sentSize.rows === size.rows) return;
    const run = this.#run;
    this.#resizing = true;
    let sent = false;
    try {
      await this.#link.request("terminal.resize", { terminalId: session.terminalId, ...size });
      this.#sentSize = size;
      sent = true;
    } catch {
      // The size is cosmetic: the next change tries again, and a dead terminal ends through the read loop.
    }
    this.#resizing = false;
    // The window changed again while this one was in flight.
    if (sent && run === this.#run && (size.cols !== this.#size.cols || size.rows !== this.#size.rows)) this.resize(this.#size.cols, this.#size.rows);
  }

  // --- End ---

  /** Kills the shell on the desktop now. The session ends here even when the link cannot carry the request. */
  async close(): Promise<void> {
    const session = this.#snapshot.session;
    if (!session || this.#snapshot.ended) return;
    // Stop reading and writing first: nothing may follow the close request.
    this.#stopRun();
    try {
      await this.#link.request("terminal.close", { terminalId: session.terminalId });
    } catch {
      // Nothing more to do: a dead link ends the shell after the desktop's grace window.
    }
    this.#end("closed");
  }

  #end(reason: string, code?: number): void {
    if (this.#snapshot.ended) return;
    const session = this.#snapshot.session;
    const idleMinutes = session ? Math.round(session.idleTimeoutMs / 60_000) : undefined;
    const ended: TerminalEnded = { reason, code, text: endedText({ reason, code, idleMinutes }) };
    this.#stopRun();
    this.#store?.clear();
    this.#set({ phase: "ended", ended, inputNotice: null });
  }

  /** Stops loops and timers of the current run, leaving the snapshot as it is. */
  #stopRun(): void {
    this.#run += 1;
    this.#started = false;
    this.#readAbort?.abort();
    this.#readAbort = null;
    this.#clearExpiry();
    for (const timer of [this.#flushTimer, this.#resizeTimer, this.#persistTimer, this.#noticeTimer]) if (timer !== null) clearTimeout(timer);
    this.#flushTimer = this.#resizeTimer = this.#persistTimer = this.#noticeTimer = null;
    this.#pending = new Uint8Array(0);
    this.#writing = false;
    this.#resizing = false;
    this.#unsubscribeReady?.();
    this.#unsubscribeReady = null;
    this.#wakeSleepers();
  }

  /** The page goes away. The shell is NOT killed: it stays for a reattach until idle, the grace window or Close. */
  dispose(): void {
    this.flushResume();
    this.#stopRun();
  }

  // --- Sleeping and waiting ---

  #sleep(ms: number, run: number): Promise<void> {
    return new Promise((resolve) => {
      if (run !== this.#run) return resolve();
      const done = (): void => {
        clearTimeout(timer);
        this.#sleepers.delete(done);
        resolve();
      };
      const timer = setTimeout(done, ms);
      this.#sleepers.add(done);
    });
  }

  #waitReady(run: number): Promise<void> {
    return new Promise((resolve) => {
      if (run !== this.#run || this.#link.isReady()) return resolve();
      this.#sleepers.add(resolve);
    });
  }

  #wakeSleepers(): void {
    const sleepers = [...this.#sleepers];
    this.#sleepers.clear();
    for (const wake of sleepers) wake();
  }

  // --- Resume entry (ids and offsets, never output) ---

  #persist(): void {
    const session = this.#snapshot.session;
    if (!session || !this.#store) return;
    this.#store.save({ terminalId: session.terminalId, next: this.#next, cwd: session.cwd, idleTimeoutMs: session.idleTimeoutMs, maxDurationMs: session.maxDurationMs, openedAt: session.openedAt });
  }

  #persistSoon(): void {
    if (this.#persistTimer !== null || !this.#store) return;
    this.#persistTimer = setTimeout(() => {
      this.#persistTimer = null;
      this.#persist();
    }, PERSIST_EVERY_MS);
  }

  /** Writes the resume entry now (the page is being hidden or unloaded). */
  flushResume(): void {
    if (this.#snapshot.session && !this.#snapshot.ended) this.#persist();
  }
}

function concat(first: Uint8Array, second: Uint8Array): Uint8Array {
  const out = new Uint8Array(first.length + second.length);
  out.set(first);
  out.set(second, first.length);
  return out;
}
