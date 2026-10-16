// @vitest-environment happy-dom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("./api", () => ({
  startSync: vi.fn(),
  cancelSync: vi.fn(),
  syncStatus: vi.fn(),
}));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn() }));

import { listen } from "@tauri-apps/api/event";
import { cancelSync, startSync, syncStatus } from "./api";
import type { SyncEvent } from "./syncEvents";
import {
  cancelSyncJobs,
  dismissSyncJob,
  ensureSyncJobs,
  eventKey,
  forgetSyncJobs,
  getSyncJobs,
  jobKey,
  onSyncJobEnded,
  reconcile,
  resetSyncJobs,
  startSyncJob,
  subscribeSyncJobs,
} from "./syncJobs";

const FROM = new Date(2026, 8, 1);
const TO = new Date(2026, 9, 1);
const secs = (d: Date) => Math.floor(d.getTime() / 1000);

const event = (over: Partial<SyncEvent> = {}): SyncEvent => ({
  repoKey: "/r/.git",
  project: "r",
  root: "/work/r",
  branch: "main",
  from: secs(FROM),
  to: secs(TO),
  step: "commits",
  done: 1,
  total: 10,
  percent: 10,
  state: "running",
  error: null,
  ...over,
});

const keyOf = (over: Partial<SyncEvent> = {}) => eventKey(event(over));

const job = {
  branchName: "main",
  window: { from: FROM, to: TO },
  mode: "full" as const,
};

let handlers: ((e: { payload: SyncEvent }) => void)[] = [];
const emit = (e: SyncEvent) => handlers.forEach((h) => h({ payload: e }));
const flush = () => new Promise((resolve) => setTimeout(resolve, 0));

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((res) => {
    resolve = res;
  });
  return { promise, resolve };
}

beforeEach(() => {
  resetSyncJobs();
  handlers = [];
  vi.mocked(listen).mockImplementation((async (
    _name: string,
    handler: (e: { payload: SyncEvent }) => void,
  ) => {
    handlers.push(handler);
    return () => {
      handlers = handlers.filter((h) => h !== handler);
    };
  }) as unknown as typeof listen);
  vi.mocked(syncStatus).mockResolvedValue([]);
  vi.mocked(startSync).mockResolvedValue(undefined);
  vi.mocked(cancelSync).mockResolvedValue(undefined);
});

afterEach(() => {
  resetSyncJobs();
  vi.clearAllMocks();
});

describe("sync jobs store", () => {
  it("listens once for the whole app and seeds from Rust", async () => {
    vi.mocked(syncStatus).mockResolvedValue([event({ percent: 40 })]);
    await ensureSyncJobs();
    await ensureSyncJobs();
    expect(listen).toHaveBeenCalledTimes(1);
    expect(getSyncJobs()[keyOf()]?.percent).toBe(40);
  });

  it("follows events, and notifies subscribers", async () => {
    await ensureSyncJobs();
    const seen = vi.fn();
    const stop = subscribeSyncJobs(seen);
    emit(event({ state: "queued" }));
    emit(event({ percent: 55 }));
    expect(getSyncJobs()[keyOf()]?.percent).toBe(55);
    expect(seen).toHaveBeenCalledTimes(2);
    stop();
    emit(event({ percent: 56 }));
    expect(seen).toHaveBeenCalledTimes(2);
  });

  it("removes an ended job, tells the listeners, and keeps a failure to show", async () => {
    await ensureSyncJobs();
    const ended = vi.fn();
    onSyncJobEnded(ended);
    emit(event());
    emit(event({ state: "done", percent: 100 }));
    expect(getSyncJobs()[keyOf()]).toBeUndefined();
    emit(event({ from: 1, to: 2 }));
    emit(event({ from: 1, to: 2, state: "error", error: "boom" }));
    expect(getSyncJobs()[keyOf({ from: 1, to: 2 })]).toMatchObject({
      state: "error",
      error: "boom",
    });
    expect(ended).toHaveBeenCalledTimes(2);
  });

  it("stuck path (b): a status answer that lands after the done event does not bring the job back", async () => {
    await ensureSyncJobs();
    emit(event());
    const late = deferred<SyncEvent[]>();
    vi.mocked(syncStatus).mockReturnValueOnce(late.promise);
    const pending = reconcile();
    // Rust listed the job, then it ended, and the event reached us first.
    emit(event({ state: "done", percent: 100 }));
    late.resolve([event()]);
    await pending;
    expect(getSyncJobs()[keyOf()]).toBeUndefined();
  });

  it("stuck path (b): a job that ended before listening was ready is not left behind", async () => {
    const registering = deferred<() => void>();
    vi.mocked(listen).mockReturnValueOnce(registering.promise);
    const ready = ensureSyncJobs();
    // Rust accepted the request, and the job finished before `listen` was ready.
    await startSyncJob("/work/r", job);
    expect(getSyncJobs()[keyOf({ state: "queued" })]).toBeUndefined();
    registering.resolve(() => undefined);
    await ready;
    expect(Object.keys(getSyncJobs())).toEqual([]);
  });

  it("stuck path (a): a duplicate request Rust dropped leaves no queued entry", async () => {
    await ensureSyncJobs();
    // Another worktree's identical job runs under its own root.
    vi.mocked(syncStatus).mockResolvedValue([event({ root: "/work/other" })]);
    await startSyncJob("/work/r", job);
    expect(getSyncJobs()[keyOf({ state: "queued" })]).toBeUndefined();
  });

  it("a started job shows queued at once and stays while Rust lists it", async () => {
    await ensureSyncJobs();
    const during = deferred<SyncEvent[]>();
    vi.mocked(syncStatus).mockReturnValueOnce(during.promise);
    const starting = startSyncJob("/work/r", job);
    expect(getSyncJobs()[keyOf()]?.state).toBe("queued");
    during.resolve([event({ state: "running", percent: 3 })]);
    await starting;
    expect(getSyncJobs()[keyOf()]).toMatchObject({
      state: "running",
      percent: 3,
    });
    expect(vi.mocked(startSync).mock.calls[0][1]).toMatchObject({
      from: expect.any(String),
      mode: "full",
    });
  });

  it("a refused start becomes an error entry, not a live job", async () => {
    await ensureSyncJobs();
    vi.mocked(startSync).mockRejectedValueOnce("Could not start the sync");
    await startSyncJob("/work/r", job);
    expect(getSyncJobs()[keyOf()]).toMatchObject({
      state: "error",
      error: "Could not start the sync",
    });
    expect(syncStatus).toHaveBeenCalledTimes(2);
  });

  it("reconcile drops a live entry Rust does not list, and only that one", async () => {
    await ensureSyncJobs();
    emit(event());
    emit(event({ from: 7, to: 8 }));
    emit(event({ from: 9, to: 10, state: "error", error: "x" }));
    const ended = vi.fn();
    onSyncJobEnded(ended);
    vi.mocked(syncStatus).mockResolvedValue([event({ from: 7, to: 8 })]);
    await reconcile();
    const jobs = getSyncJobs();
    expect(jobs[keyOf()]).toBeUndefined();
    expect(jobs[keyOf({ from: 7, to: 8 })]).toBeDefined();
    expect(jobs[keyOf({ from: 9, to: 10 })]?.state).toBe("error");
    expect(ended).toHaveBeenCalledTimes(1);
    expect(ended.mock.calls[0][0]).toMatchObject({ from: secs(FROM) });
  });

  it("reconcile leaves an entry that changed after the question was asked", async () => {
    await ensureSyncJobs();
    const slow = deferred<SyncEvent[]>();
    vi.mocked(syncStatus).mockReturnValueOnce(slow.promise);
    const pending = reconcile();
    // Created while the answer is on its way: the answer cannot know it.
    emit(event({ state: "queued" }));
    slow.resolve([]);
    await pending;
    expect(getSyncJobs()[keyOf()]?.state).toBe("queued");
  });

  it("a new job under the same key after it ended is accepted again", async () => {
    await ensureSyncJobs();
    emit(event());
    emit(event({ state: "done", percent: 100 }));
    emit(event({ state: "running", percent: 5 }));
    vi.mocked(syncStatus).mockResolvedValue([event({ percent: 5 })]);
    await reconcile();
    expect(getSyncJobs()[keyOf()]?.percent).toBe(5);
  });

  it("cancel asks Rust, then checks what still runs", async () => {
    await ensureSyncJobs();
    emit(event());
    vi.mocked(syncStatus).mockResolvedValue([]);
    await cancelSyncJobs("/work/r");
    expect(cancelSync).toHaveBeenCalledWith("/work/r");
    expect(getSyncJobs()[keyOf()]).toBeUndefined();
  });

  it("cancel still reconciles when the command fails, and reports the failure", async () => {
    await ensureSyncJobs();
    emit(event());
    vi.mocked(cancelSync).mockRejectedValueOnce("no repo");
    vi.mocked(syncStatus).mockResolvedValue([]);
    await expect(cancelSyncJobs("/work/r")).rejects.toBe("no repo");
    expect(getSyncJobs()[keyOf()]).toBeUndefined();
  });

  it("checks with Rust again when the window gets focus", async () => {
    await ensureSyncJobs();
    emit(event());
    vi.mocked(syncStatus).mockResolvedValue([]);
    window.dispatchEvent(new Event("focus"));
    await flush();
    expect(getSyncJobs()[keyOf()]).toBeUndefined();
  });

  it("forgets the entries of one project", async () => {
    await ensureSyncJobs();
    emit(event());
    emit(event({ root: "/work/other" }));
    forgetSyncJobs("/work/r");
    expect(Object.keys(getSyncJobs())).toEqual([
      jobKey("/work/other", "main", secs(FROM), secs(TO)),
    ]);
  });

  it("dismisses a failure but never a running job", async () => {
    await ensureSyncJobs();
    emit(event({ from: 1, to: 2, state: "error", error: "boom" }));
    emit(event());
    dismissSyncJob(event());
    expect(getSyncJobs()[keyOf()]).toBeDefined();
    dismissSyncJob(event({ from: 1, to: 2 }));
    expect(getSyncJobs()[keyOf({ from: 1, to: 2 })]).toBeUndefined();
  });
});
