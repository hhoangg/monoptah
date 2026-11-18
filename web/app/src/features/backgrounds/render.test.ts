// Which path dithers a photo (worker or main thread), the worker timeout and the decode size.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const processImage = vi.hoisted(() => vi.fn());
vi.mock("./process", () => ({ processImage, domCanvas: {}, offscreenCanvas: {} }));

import { renderBackground, WORKER_TIMEOUT_MS } from "./render";

const png = () => new Blob(["x"], { type: "image/png" });

class FakeWorker {
  static instances: FakeWorker[] = [];
  static behaviour: "answer" | "silent" | "fail" = "answer";
  onmessage: ((event: MessageEvent) => void) | null = null;
  onerror: (() => void) | null = null;
  terminated = false;
  constructor() {
    FakeWorker.instances.push(this);
  }
  postMessage(request: { id: number }) {
    if (FakeWorker.behaviour === "answer") queueMicrotask(() => this.onmessage?.({ data: { id: request.id, blob: png() } } as MessageEvent));
    else if (FakeWorker.behaviour === "fail") queueMicrotask(() => this.onerror?.());
  }
  terminate() {
    this.terminated = true;
  }
}

beforeEach(() => {
  FakeWorker.instances = [];
  FakeWorker.behaviour = "answer";
  processImage.mockReset();
  processImage.mockResolvedValue(png());
});
afterEach(() => {
  vi.unstubAllGlobals();
  vi.useRealTimers();
});

describe("renderBackground", () => {
  it("uses a worker when Worker and OffscreenCanvas exist", async () => {
    vi.stubGlobal("Worker", FakeWorker);
    vi.stubGlobal("OffscreenCanvas", class {});
    await renderBackground(png());
    expect(FakeWorker.instances).toHaveLength(1);
    expect(FakeWorker.instances[0]?.terminated).toBe(true);
    expect(processImage).not.toHaveBeenCalled();
  });

  it("dithers on the main thread without OffscreenCanvas", async () => {
    vi.stubGlobal("Worker", FakeWorker);
    vi.stubGlobal("OffscreenCanvas", undefined);
    await renderBackground(png());
    expect(FakeWorker.instances).toHaveLength(0);
    expect(processImage).toHaveBeenCalledOnce();
  });

  it("falls back to the main thread when the worker fails", async () => {
    FakeWorker.behaviour = "fail";
    vi.stubGlobal("Worker", FakeWorker);
    vi.stubGlobal("OffscreenCanvas", class {});
    await renderBackground(png());
    expect(processImage).toHaveBeenCalledOnce();
  });

  it("gives up on a worker that never answers after the timeout and falls back", async () => {
    vi.useFakeTimers();
    FakeWorker.behaviour = "silent";
    vi.stubGlobal("Worker", FakeWorker);
    vi.stubGlobal("OffscreenCanvas", class {});
    const done = renderBackground(png());
    await vi.advanceTimersByTimeAsync(WORKER_TIMEOUT_MS - 1);
    expect(processImage).not.toHaveBeenCalled();
    await vi.advanceTimersByTimeAsync(2);
    await done;
    expect(FakeWorker.instances[0]?.terminated).toBe(true);
    expect(processImage).toHaveBeenCalledOnce();
  });
});
