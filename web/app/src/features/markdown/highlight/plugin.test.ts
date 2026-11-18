import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { requestCodeHighlight, resetHighlighting } from "./plugin";
import { WORKER_JOB_TIMEOUT_MS } from "./types";

/** A Worker that says it is ready and then never answers a job: a grammar stuck in a runaway match. */
class StuckWorker {
  static instances: StuckWorker[] = [];
  onmessage: ((event: MessageEvent) => void) | null = null;
  onerror: (() => void) | null = null;
  terminated = false;
  posted: Array<{ id: number }> = [];
  constructor() {
    StuckWorker.instances.push(this);
    queueMicrotask(() => this.onmessage?.({ data: { ready: true } } as MessageEvent));
  }
  postMessage(message: { id: number }) {
    this.posted.push(message);
  }
  terminate() {
    this.terminated = true;
  }
}

beforeEach(() => {
  StuckWorker.instances = [];
  vi.useFakeTimers();
  vi.stubGlobal("Worker", StuckWorker);
});
afterEach(() => {
  resetHighlighting();
  vi.useRealTimers();
  vi.unstubAllGlobals();
});

describe("highlight Worker time limit", () => {
  it("terminates a Worker stuck on a block, leaves it plain, and moves on to the next block", async () => {
    const stuck = vi.fn();
    const next = vi.fn();
    requestCodeHighlight({ code: "<".repeat(1_900), language: "sh" }, stuck);
    requestCodeHighlight({ code: "echo hi # a different block of more than three hundred characters".padEnd(400, " "), language: "sh" }, next);
    await vi.advanceTimersByTimeAsync(0);
    const first = StuckWorker.instances[0]!;
    expect(first.posted).toHaveLength(1);
    // Never on the main thread, whatever the size: nothing was tokenized inline.
    expect(stuck).not.toHaveBeenCalled();
    await vi.advanceTimersByTimeAsync(WORKER_JOB_TIMEOUT_MS + 10);
    expect(first.terminated).toBe(true);
    expect(stuck).not.toHaveBeenCalled();
    // The next block got a fresh Worker.
    expect(StuckWorker.instances).toHaveLength(2);
    expect(StuckWorker.instances[1]!.posted).toHaveLength(1);
    // The block that hit the limit is not asked for again.
    requestCodeHighlight({ code: "<".repeat(1_900), language: "sh" }, stuck);
    await vi.advanceTimersByTimeAsync(0);
    expect(StuckWorker.instances[1]!.posted).toHaveLength(1);
  });

  it("does not highlight a block above the inline size when there is no Worker", async () => {
    vi.unstubAllGlobals();
    const callback = vi.fn();
    requestCodeHighlight({ code: "x".repeat(400), language: "sh" }, callback);
    await vi.advanceTimersByTimeAsync(50);
    expect(callback).not.toHaveBeenCalled();
  });
});
