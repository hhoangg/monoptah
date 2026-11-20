// The two seams of the terminal logic: the link (requests and readiness) and the screen (what draws the output).
// Both are interfaces so the session controller is tested with fakes and needs no canvas and no WebSocket.
import type { ParamsOf, ResultOf } from "@monoptah/protocol";
import type { LinkConnection } from "@/link";

export type TerminalMethod = "terminal.open" | "terminal.read" | "terminal.write" | "terminal.resize" | "terminal.close";

export interface TerminalLinkPort {
  request<M extends TerminalMethod>(method: M, params: ParamsOf<M>, options?: { signal?: AbortSignal }): Promise<ResultOf<M>>;
  isReady(): boolean;
  /** Calls `listener` whenever readiness changes; returns the unsubscribe function. */
  subscribeReady(listener: (ready: boolean) => void): () => void;
}

/** The link of the open desktop as the terminal sees it. */
export function linkPortOf(connection: Pick<LinkConnection, "request" | "store">): TerminalLinkPort {
  const ready = (): boolean => connection.store.getState().status === "ready";
  return {
    request: (method, params, options) => connection.request(method, params, options ?? {}),
    isReady: ready,
    subscribeReady(listener) {
      let last = ready();
      return connection.store.subscribe(() => {
        const now = ready();
        if (now === last) return;
        last = now;
        listener(now);
      });
    },
  };
}

export interface TerminalSize {
  cols: number;
  rows: number;
}

/** What draws the output (xterm behind `xtermScreen.ts`, a fake in tests). */
export interface TerminalScreen {
  /** Draws `data`; calls `done` once it has been parsed, which is the signal to send more (flow control). */
  write(data: Uint8Array, done: () => void): void;
  size(): TerminalSize;
}
