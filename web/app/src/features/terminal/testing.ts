// Test doubles of the two ports of the terminal controller. Not imported by app code.
import type { MethodName, ParamsOf, ResultOf } from "@monoptah/protocol";
import type { TerminalLinkPort, TerminalMethod, TerminalScreen, TerminalSize } from "./port";

export class FakeError extends Error {
  readonly code: string;
  constructor(code: string) {
    super(code);
    this.code = code;
  }
}

export interface Call {
  method: TerminalMethod;
  params: Record<string, unknown>;
  signal: AbortSignal | undefined;
  resolve: (value: unknown) => void;
  reject: (error: unknown) => void;
  settled: boolean;
}

/** A link whose every request waits for the test to answer it. */
export class FakeLink implements TerminalLinkPort {
  ready = true;
  readonly calls: Call[] = [];
  readonly #listeners = new Set<(ready: boolean) => void>();

  request<M extends TerminalMethod>(method: M, params: ParamsOf<M>, options?: { signal?: AbortSignal }): Promise<ResultOf<M>> {
    return new Promise((resolve, reject) => {
      const call: Call = { method, params: params as Record<string, unknown>, signal: options?.signal, resolve: (v) => { call.settled = true; resolve(v as ResultOf<M>); }, reject: (e) => { call.settled = true; reject(e); }, settled: false };
      this.calls.push(call);
      options?.signal?.addEventListener("abort", () => { if (!call.settled) call.reject(new FakeError("aborted")); });
      if (!this.ready) call.reject(new FakeError("not_ready"));
    });
  }

  isReady(): boolean {
    return this.ready;
  }

  subscribeReady(listener: (ready: boolean) => void): () => void {
    this.#listeners.add(listener);
    return () => this.#listeners.delete(listener);
  }

  setReady(ready: boolean): void {
    this.ready = ready;
    for (const listener of [...this.#listeners]) listener(ready);
  }

  /** Unanswered calls of one method, oldest first. */
  open(method: TerminalMethod): Call[] {
    return this.calls.filter((call) => call.method === method && !call.settled);
  }

  all(method: TerminalMethod): Call[] {
    return this.calls.filter((call) => call.method === method);
  }
}

/** A screen that records what is drawn; `hold` keeps the done callbacks until the test releases them. */
export class FakeScreen implements TerminalScreen {
  readonly writes: Uint8Array[] = [];
  readonly held: (() => void)[] = [];
  hold = false;
  dims: TerminalSize = { cols: 80, rows: 24 };

  write(data: Uint8Array, done: () => void): void {
    this.writes.push(data);
    if (this.hold) this.held.push(done);
    else done();
  }

  size(): TerminalSize {
    return this.dims;
  }

  releaseOne(): void {
    this.held.shift()?.();
  }

  /** Everything drawn, as text. */
  text(): string {
    return new TextDecoder().decode(Uint8Array.from(this.writes.flatMap((chunk) => [...chunk])));
  }
}

export const b64 = (text: string): string => btoa(text);

export function readResult(from: number, text: string, exited: { reason: string; code?: number } | null = null): { from: number; next: number; data: string; exited: typeof exited } {
  return { from, next: from + text.length, data: b64(text), exited };
}

export type { MethodName };

// --- A stand-in for the xterm screen of the view ---
import type { ScreenHandlers, XtermScreen } from "./xtermScreen";

export class FakeXterm extends FakeScreen implements XtermScreen {
  handlers: ScreenHandlers | null = null;
  selection = "";
  pasted: string[] = [];
  application = false;
  disposed = false;
  focused = 0;
  focus(): void {
    this.focused += 1;
  }
  hasSelection(): boolean {
    return this.selection !== "";
  }
  getSelection(): string {
    return this.selection;
  }
  paste(text: string): void {
    this.pasted.push(text);
    // xterm answers a paste with the data event, as a typed text.
    this.handlers?.onData(text);
  }
  applicationCursor(): boolean {
    return this.application;
  }
  fit(): void {}
  dispose(): void {
    this.disposed = true;
  }
}

/** A `createScreen` for `TerminalView` that hands out one `FakeXterm` and keeps the handlers the view gave it. */
export function fakeScreenFactory(): { xterm: FakeXterm; create: (host: HTMLElement, handlers: ScreenHandlers) => Promise<XtermScreen> } {
  const xterm = new FakeXterm();
  return {
    xterm,
    create: async (_host, handlers) => {
      xterm.handlers = handlers;
      return xterm;
    },
  };
}
