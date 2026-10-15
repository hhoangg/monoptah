// @vitest-environment happy-dom
import { act, createElement, StrictMode } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("../model/api", () => ({
  readConfig: vi.fn(),
  readMailmap: vi.fn(),
  writeConfig: vi.fn(),
  loadStored: vi.fn(),
  syncEstimate: vi.fn(),
  startSync: vi.fn(),
  cancelSync: vi.fn(),
  syncStatus: vi.fn(),
  clearSynced: vi.fn(),
  listBranches: vi.fn(),
  gitUser: vi.fn(),
}));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn() }));

import { listen } from "@tauri-apps/api/event";
import {
  cancelSync,
  clearSynced,
  gitUser,
  listBranches,
  loadStored,
  readConfig,
  readMailmap,
  startSync,
  syncEstimate,
  syncStatus,
  writeConfig,
} from "../model/api";
import { SYNC_EVENT } from "../model/syncEvents";
import type { SyncEvent } from "../model/syncEvents";
import type { LoadResponse } from "../model/types";
import { DEFAULT_CONFIG, serializeConfig } from "../model/config";
import { RANGE_KEY } from "../model/prefs";
import { currentPeriod, previousPeriod, syncWindow } from "../model/period";
import type { Period } from "../model/period";
import { ANA, file, makeCommit, makeRaw } from "../model/testFixtures";
import {
  contributorsCacheSize,
  resetContributorsCache,
  useContributors,
} from "./useContributors";

type Api = ReturnType<typeof useContributors>;

let container: HTMLDivElement;
let root: Root;
let api: Api;
let cwdCounter = 0;

const isoSeconds = (date: Date) => date.toISOString().replace(".000Z", "Z");

/** The month `n` months before the current one. */
const monthBack = (n: number): Period => {
  let p = currentPeriod("month", new Date());
  for (let i = 0; i < n; i += 1) p = previousPeriod(p);
  return p;
};
const MONTH_A = monthBack(3);
const MONTH_B = monthBack(4);
const saved = (p: Period) => `${p.kind}:${p.id}`;

/** Selects the month `n` months back through the same controls the page uses. */
async function selectMonthBack(n: number) {
  await act(async () => api.setKind("month"));
  for (let i = 0; i < n; i += 1) await act(async () => api.goPrevious());
}
const nextCwd = () => `/repo-${(cwdCounter += 1)}`;

function Probe({ cwd }: { cwd: string }) {
  api = useContributors(cwd);
  return null;
}

const mount = (cwd: string) =>
  act(async () => {
    root.render(createElement(Probe, { cwd }));
  });

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

const rawFixture = () =>
  makeRaw({
    commits: [
      makeCommit("a1", ANA, new Date().toISOString(), {
        files: [file("src/a.ts", 3)],
      }),
    ],
  });

const NOW_SECS = () => Math.floor(Date.now() / 1000);

/** A period the store has fully synced, at the same head as the branch. */
const loadFixture = (over: Partial<LoadResponse> = {}): LoadResponse => {
  const raw = over.raw ?? rawFixture();
  return {
    raw,
    covered: true,
    hasGaps: false,
    missingCommits: 0,
    syncedAt: NOW_SECS() - 300,
    coverageHead: raw.head,
    head: raw.head,
    ghState: "complete",
    ...over,
  };
};

/** A period the store has nothing (or not everything) for. */
const unsyncedFixture = (over: Partial<LoadResponse> = {}) =>
  loadFixture({
    raw: makeRaw(),
    covered: false,
    hasGaps: true,
    missingCommits: 40,
    syncedAt: null,
    coverageHead: null,
    ghState: null,
    ...over,
  });

const ESTIMATE = { commits: 40, storedCommits: 0, firstParent: 30, prs: 9 };

// The mocked `listen` keeps every handler so a test can push events.
const handlers: ((e: { payload: SyncEvent }) => void)[] = [];
const secs = (date: Date) => Math.floor(date.getTime() / 1000);
const windowOf = (p: Period) => ({
  from: secs(syncWindow(p, new Date()).from),
  to: secs(p.end),
});
const syncEvent = (cwd: string, over: Partial<SyncEvent> = {}): SyncEvent => ({
  repoKey: "/repo/.git",
  project: "repo",
  root: cwd,
  branch: "main",
  ...windowOf(currentPeriod("7d", new Date())),
  step: "commits",
  done: 0,
  total: 0,
  percent: 0,
  state: "running",
  error: null,
  ...over,
});
// What Rust would list as running: the store checks its jobs against it.
let listed: SyncEvent[] = [];
const sameJob = (a: SyncEvent, b: SyncEvent) =>
  a.root === b.root && a.from === b.from && a.to === b.to;
const list = (event: SyncEvent) => {
  const rest = listed.filter((other) => !sameJob(other, event));
  listed =
    event.state === "queued" || event.state === "running"
      ? [...rest, event]
      : rest;
};
const emit = (event: SyncEvent) =>
  act(async () => {
    list(event);
    handlers.forEach((handler) => handler({ payload: event }));
  });

beforeEach(() => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  localStorage.clear();
  resetContributorsCache();
  container = document.createElement("div");
  document.body.append(container);
  root = createRoot(container);
  vi.mocked(readConfig).mockResolvedValue(null);
  vi.mocked(readMailmap).mockResolvedValue(null);
  vi.mocked(writeConfig).mockResolvedValue(undefined);
  vi.mocked(listBranches).mockResolvedValue({
    current: "main",
    branches: ["main", "dev"],
  });
  vi.mocked(gitUser).mockResolvedValue({
    name: "Ana",
    email: "ana@example.com",
  });
  vi.mocked(loadStored).mockImplementation(async () => loadFixture());
  vi.mocked(syncEstimate).mockResolvedValue(ESTIMATE);
  listed = [];
  vi.mocked(startSync).mockImplementation(async (cwd, args) => {
    list(
      syncEvent(cwd, {
        branch: "main",
        from: Date.parse(args.from) / 1000,
        to: Date.parse(args.to) / 1000,
        state: "queued",
      }),
    );
  });
  vi.mocked(cancelSync).mockResolvedValue(undefined);
  vi.mocked(clearSynced).mockResolvedValue(undefined);
  vi.mocked(syncStatus).mockImplementation(async () => listed);
  handlers.length = 0;
  vi.mocked(listen).mockImplementation((async (
    _event: string,
    handler: (e: { payload: SyncEvent }) => void,
  ) => {
    handlers.push(handler);
    return () => {
      const at = handlers.indexOf(handler);
      if (at >= 0) handlers.splice(at, 1);
    };
  }) as unknown as typeof listen);
});

afterEach(async () => {
  await act(async () => root.unmount());
  container.remove();
  vi.clearAllMocks();
  vi.unstubAllGlobals();
});

describe("useContributors", () => {
  it("goes from loading to ready with raw data, branches and a model", async () => {
    const gate = deferred<LoadResponse>();
    vi.mocked(loadStored).mockReturnValueOnce(gate.promise);
    await mount(nextCwd());
    expect(api.status).toBe("loading");
    expect(api.model).toBeUndefined();

    await act(async () => gate.resolve(loadFixture()));
    expect(api.status).toBe("ready");
    expect(api.raw?.commits).toHaveLength(1);
    expect(api.branches).toEqual(["main", "dev"]);
    expect(api.branch).toBe("main");
    expect(api.model?.stats.kpis.commits.value).toBe(1);
    expect(api.model?.meId).toBe("ana@example.com");
    expect(api.config).toEqual(DEFAULT_CONFIG);
  });

  it("loads the default 7d period from the start of the previous 7 days to the period end", async () => {
    await mount(nextCwd());
    const args = vi.mocked(loadStored).mock.calls[0][1];
    const period = currentPeriod("7d", new Date());
    const window = syncWindow(period, new Date());
    expect(args.since).toBe(isoSeconds(window.from));
    expect(args.until).toBe(isoSeconds(period.end));
    expect(args.branch).toBeUndefined();
    expect(api.period.kind).toBe("7d");
    expect(api.model?.period.id).toBe("7d");
  });

  it("loads a past month from the start of the month before it to its end", async () => {
    await mount(nextCwd());
    await selectMonthBack(3);
    const args = vi.mocked(loadStored).mock.calls.at(-1)![1];
    expect(args.since).toBe(isoSeconds(monthBack(4).start));
    expect(args.until).toBe(isoSeconds(MONTH_A.end));
    expect(api.model?.period.id).toBe(MONTH_A.id);
  });

  it("loads the current month from the start of the previous one to its end", async () => {
    await mount(nextCwd());
    await act(async () => api.setKind("month"));
    const args = vi.mocked(loadStored).mock.calls.at(-1)![1];
    expect(args.until).toBe(isoSeconds(monthBack(0).end));
    expect(args.since).toBe(isoSeconds(monthBack(1).start));
  });

  it("loads again when the period changes, and not when it does not", async () => {
    await mount(nextCwd());
    await act(async () => api.setKind("month"));
    const base = vi.mocked(loadStored).mock.calls.length;
    await act(async () => api.setKind("month"));
    expect(vi.mocked(loadStored).mock.calls.length).toBe(base);
    await act(async () => api.goPrevious());
    expect(vi.mocked(loadStored).mock.calls.length).toBe(base + 1);
  });

  it("restores a saved period, and a saved removed range falls back to 7d", async () => {
    localStorage.setItem(RANGE_KEY, saved(MONTH_A));
    await mount(nextCwd());
    expect(api.period.id).toBe(MONTH_A.id);
    for (const removed of [
      "1y",
      "30d",
      "custom:2026-10-01:2026-10-09",
      "all",
    ]) {
      await act(async () => root.unmount());
      root = createRoot(container);
      localStorage.setItem(RANGE_KEY, removed);
      await mount(nextCwd());
      expect(api.period.kind).toBe("7d");
      expect(api.period.id).toBe("7d");
    }
  });

  it("shows cached data at once when the page is opened again", async () => {
    const cwd = nextCwd();
    await mount(cwd);
    expect(api.status).toBe("ready");
    await act(async () => root.unmount());
    root = createRoot(container);

    const gate = deferred<LoadResponse>();
    vi.mocked(loadStored).mockReturnValueOnce(gate.promise);
    await mount(cwd);
    expect(api.status).toBe("ready");
    expect(api.refreshing).toBe(true);
    await act(async () => gate.resolve(loadFixture()));
    expect(api.refreshing).toBe(false);
  });

  it("surfaces a load failure and recovers on a forced retry", async () => {
    vi.mocked(loadStored).mockRejectedValueOnce("git exploded");
    await mount(nextCwd());
    expect(api.status).toBe("error");
    expect(api.error).toBe("git exploded");

    await act(async () => api.refresh(true));
    expect(api.status).toBe("ready");
    expect(api.error).toBeUndefined();
    expect(vi.mocked(loadStored)).toHaveBeenCalledTimes(2);
  });

  it("drops the old error at once when retrying and shows loading until the answer", async () => {
    vi.mocked(loadStored).mockRejectedValueOnce("git exploded");
    await mount(nextCwd());
    expect(api.status).toBe("error");

    const gate = deferred<LoadResponse>();
    vi.mocked(loadStored).mockReturnValueOnce(gate.promise);
    let retry!: Promise<void>;
    await act(async () => {
      retry = api.refresh(true);
    });
    expect(api.error).toBeUndefined();
    expect(api.status).toBe("loading");
    expect(api.refreshing).toBe(true);

    await act(async () => {
      gate.resolve(loadFixture());
      await retry;
    });
    expect(api.status).toBe("ready");
  });

  it("drops a refresh failure banner at once when refreshing again", async () => {
    await mount(nextCwd());
    vi.mocked(loadStored).mockRejectedValueOnce("offline");
    await act(async () => api.refresh(true));
    expect(api.status).toBe("ready");
    expect(api.error).toBe("offline");

    const gate = deferred<LoadResponse>();
    vi.mocked(loadStored).mockReturnValueOnce(gate.promise);
    await act(async () => {
      void api.refresh(true);
    });
    expect(api.error).toBeUndefined();
    await act(async () => gate.resolve(loadFixture()));
  });

  it("reads the project files once even when the mount effect runs twice (StrictMode)", async () => {
    await act(async () => {
      root.render(
        createElement(
          StrictMode,
          null,
          createElement(Probe, { cwd: nextCwd() }),
        ),
      );
    });
    expect(api.status).toBe("ready");
    expect(readConfig).toHaveBeenCalledTimes(1);
    expect(readMailmap).toHaveBeenCalledTimes(1);
    expect(listBranches).toHaveBeenCalledTimes(1);
    expect(gitUser).toHaveBeenCalledTimes(1);
    expect(loadStored).toHaveBeenCalledTimes(1);
  });

  it("a forced reload reads the files again instead of joining a read in flight", async () => {
    const first = deferred<string | null>();
    vi.mocked(readConfig).mockReturnValueOnce(first.promise);
    await mount(nextCwd());
    await act(async () => {
      void api.refresh(true);
    });
    expect(readConfig).toHaveBeenCalledTimes(2);
    await act(async () => first.resolve(null));
  });

  it("treats an unreadable config as an error instead of using defaults", async () => {
    vi.mocked(readConfig).mockRejectedValueOnce("permission denied");
    await mount(nextCwd());
    expect(api.status).toBe("error");
    expect(api.error).toBe("permission denied");
  });

  it("keeps working when only the mailmap, branches or git user fail", async () => {
    vi.mocked(readMailmap).mockRejectedValueOnce("no");
    vi.mocked(listBranches).mockRejectedValueOnce("no");
    vi.mocked(gitUser).mockRejectedValueOnce("no");
    await mount(nextCwd());
    expect(api.status).toBe("ready");
    expect(api.branches).toEqual([]);
    expect(api.model?.meId).toBeUndefined();
  });

  it("parses the stored config", async () => {
    vi.mocked(readConfig).mockResolvedValue(
      serializeConfig({
        ...DEFAULT_CONFIG,
        people: [{ id: "ana", name: "Ana S", identities: ["ana@example.com"] }],
      }),
    );
    await mount(nextCwd());
    expect(api.config.people[0].name).toBe("Ana S");
    expect(api.model?.stats.rows[0].name).toBe("Ana S");
  });

  it("saves optimistically and writes the serialized config", async () => {
    const cwd = nextCwd();
    await mount(cwd);
    const gate = deferred<void>();
    vi.mocked(writeConfig).mockReturnValueOnce(gate.promise);
    const next = {
      ...api.config,
      weights: { ...api.config.weights, review: 2 },
    };

    let saving!: Promise<void>;
    await act(async () => {
      saving = api.saveConfig(next);
    });
    expect(api.config.weights.review).toBe(2);
    expect(writeConfig).toHaveBeenCalledWith(cwd, serializeConfig(next));

    await act(async () => {
      gate.resolve();
      await saving;
    });
    expect(api.config.weights.review).toBe(2);
    expect(api.saveError).toBeUndefined();
  });

  it("reverts and reports an error when the write fails", async () => {
    await mount(nextCwd());
    vi.mocked(writeConfig).mockRejectedValueOnce("disk full");
    const next = { ...api.config, weights: { ...api.config.weights, care: 3 } };
    await act(async () => api.saveConfig(next));
    expect(api.config.weights.care).toBe(1);
    expect(api.saveError).toBe("disk full");

    await act(async () => api.saveConfig(next));
    expect(api.config.weights.care).toBe(3);
    expect(api.saveError).toBeUndefined();
  });

  it("exposes no config error for a valid or missing file", async () => {
    await mount(nextCwd());
    expect(api.configError).toBeUndefined();
  });

  it.each([
    ["broken JSON", '{ "people": [ ,, ] }', /^Invalid JSON/],
    ["a wrong top-level type", "[]", /^Expected a JSON object/],
  ])(
    "reports %s as a config error and refuses to save over it",
    async (_label, text, reason) => {
      vi.mocked(readConfig).mockResolvedValue(text);
      await mount(nextCwd());
      expect(api.status).toBe("ready");
      expect(api.configError).toMatch(reason);
      // Defaults are still shown so the page renders.
      expect(api.config).toEqual(DEFAULT_CONFIG);

      let failure: unknown;
      await act(async () => {
        failure = await api
          .saveConfig({ ...api.config, ignore: ["x/**"] })
          .then(
            () => undefined,
            (error: unknown) => error,
          );
      });
      expect(failure).toBeInstanceOf(Error);
      expect((failure as Error).message).toMatch(
        /\.monoptah\/contributors\.json could not be read .*Fix the file/,
      );
      expect(writeConfig).not.toHaveBeenCalled();
      // The page-level save alert would only duplicate the config banner.
      expect(api.saveError).toBeUndefined();
      expect(api.config).toEqual(DEFAULT_CONFIG);
    },
  );

  it("clears the config error after the file is fixed and reloaded, then saves again", async () => {
    vi.mocked(readConfig).mockResolvedValueOnce("{ nope");
    await mount(nextCwd());
    expect(api.configError).toBeDefined();

    vi.mocked(readConfig).mockResolvedValue(
      serializeConfig({
        ...DEFAULT_CONFIG,
        people: [{ id: "ana", name: "Ana S", identities: ["ana@example.com"] }],
      }),
    );
    await act(async () => api.refresh(true));
    expect(api.configError).toBeUndefined();
    expect(api.config.people[0].name).toBe("Ana S");

    await act(async () => api.saveConfig({ ...api.config, ignore: ["x/**"] }));
    expect(writeConfig).toHaveBeenCalledTimes(1);
  });

  it("notices a file that turned invalid between loads", async () => {
    await mount(nextCwd());
    expect(api.configError).toBeUndefined();
    vi.mocked(readConfig).mockResolvedValue("<<<<<<< HEAD");
    await act(async () => api.refresh(true));
    expect(api.configError).toMatch(/^Invalid JSON/);
  });

  it("serializes overlapping saves in order", async () => {
    await mount(nextCwd());
    const first = deferred<void>();
    vi.mocked(writeConfig).mockReturnValueOnce(first.promise);
    const a = { ...api.config, weights: { ...api.config.weights, care: 2 } };
    const b = { ...a, weights: { ...a.weights, review: 4 } };
    let saves!: Promise<unknown>;
    await act(async () => {
      saves = Promise.all([api.saveConfig(a), api.saveConfig(b)]);
    });
    expect(writeConfig).toHaveBeenCalledTimes(1);
    await act(async () => {
      first.resolve();
      await saves;
    });
    expect(writeConfig).toHaveBeenCalledTimes(2);
    expect(vi.mocked(writeConfig).mock.calls[1][1]).toBe(serializeConfig(b));
    expect(api.config.weights).toMatchObject({ care: 2, review: 4 });
  });

  it("reads the store again on a forced refresh", async () => {
    await mount(nextCwd());
    await act(async () => api.refresh(true));
    expect(vi.mocked(loadStored)).toHaveBeenCalledTimes(2);
  });

  it("revalidates on window focus", async () => {
    await mount(nextCwd());
    const before = vi.mocked(loadStored).mock.calls.length;
    await act(async () => {
      window.dispatchEvent(new Event("focus"));
    });
    expect(vi.mocked(loadStored).mock.calls.length).toBe(before + 1);
  });

  it("keeps the same raw object when a revalidation returns the same data", async () => {
    await mount(nextCwd());
    const first = api.raw;
    await act(async () => {
      window.dispatchEvent(new Event("focus"));
    });
    expect(api.raw).toBe(first);
  });

  it("collects the chosen branch and keeps it per project", async () => {
    await mount(nextCwd());
    await act(async () => api.setBranch("dev"));
    expect(vi.mocked(loadStored).mock.calls.at(-1)?.[1].branch).toBe("dev");
    expect(api.branch).toBe("dev");
  });

  it("persists the period and the merge toggle", async () => {
    await mount(nextCwd());
    await selectMonthBack(3);
    await act(async () => api.setMerge(false));
    expect(localStorage.getItem(RANGE_KEY)).toBe(saved(MONTH_A));
    expect(localStorage.getItem("monocode.contributors.mergeIdentities")).toBe(
      "false",
    );
    expect(api.period.id).toBe(MONTH_A.id);
    expect(api.merge).toBe(false);
    expect(api.model?.merge).toBe(false);
  });
});

describe("useContributors races", () => {
  it("keeps a save made while a focus reload was still reading the config", async () => {
    await mount(nextCwd());
    const slow = deferred<string | null>();
    vi.mocked(readConfig).mockReturnValueOnce(slow.promise);
    await act(async () => {
      window.dispatchEvent(new Event("focus"));
    });
    const next = {
      ...api.config,
      weights: { ...api.config.weights, review: 5 },
    };
    await act(async () => api.saveConfig(next));
    await act(async () => slow.resolve(null));
    expect(api.config.weights.review).toBe(5);

    // The next edit is built from the shown config and keeps the earlier one.
    await act(async () =>
      api.saveConfig({
        ...api.config,
        weights: { ...api.config.weights, care: 3 },
      }),
    );
    expect(vi.mocked(writeConfig).mock.calls.at(-1)?.[1]).toContain(
      '"review": 5',
    );
  });

  it("keeps a pending save when a reload that started during it finishes", async () => {
    await mount(nextCwd());
    const write = deferred<void>();
    vi.mocked(writeConfig).mockReturnValueOnce(write.promise);
    const next = { ...api.config, weights: { ...api.config.weights, care: 4 } };
    let saving!: Promise<void>;
    await act(async () => {
      saving = api.saveConfig(next);
    });
    const slow = deferred<string | null>();
    vi.mocked(readConfig).mockReturnValueOnce(slow.promise);
    await act(async () => {
      void api.refresh(false);
    });
    await act(async () => {
      write.resolve();
      await saving;
    });
    await act(async () => slow.resolve(null));
    expect(api.config.weights.care).toBe(4);
  });

  it("still picks up an edited file when no save was involved", async () => {
    await mount(nextCwd());
    vi.mocked(readConfig).mockResolvedValueOnce(
      serializeConfig({
        ...DEFAULT_CONFIG,
        weights: { ...DEFAULT_CONFIG.weights, breadth: 2 },
      }),
    );
    await act(async () => api.refresh(false));
    expect(api.config.weights.breadth).toBe(2);
  });

  it("ignores a late failure of an abandoned request", async () => {
    localStorage.setItem(RANGE_KEY, saved(MONTH_A));
    const a = deferred<LoadResponse>();
    const b = deferred<LoadResponse>();
    vi.mocked(loadStored)
      .mockReset()
      .mockReturnValueOnce(a.promise)
      .mockReturnValueOnce(b.promise);
    await mount(nextCwd());
    await act(async () => api.goPrevious());
    await act(async () => b.reject("B failed"));
    expect(api.status).toBe("error");
    expect(api.error).toBe("B failed");
    await act(async () => a.reject("A failed"));
    expect(api.status).toBe("error");
    expect(api.error).toBe("B failed");
  });

  it("does not clear the current failure when an abandoned request succeeds", async () => {
    localStorage.setItem(RANGE_KEY, saved(MONTH_A));
    const a = deferred<LoadResponse>();
    vi.mocked(loadStored)
      .mockReset()
      .mockReturnValueOnce(a.promise)
      .mockRejectedValueOnce("B failed");
    await mount(nextCwd());
    await act(async () => api.goPrevious());
    expect(api.error).toBe("B failed");
    await act(async () => a.resolve(loadFixture()));
    expect(api.error).toBe("B failed");
    expect(api.status).toBe("error");
  });

  it("loads the new project when switching while a save is in flight", async () => {
    await mount(nextCwd());
    const write = deferred<void>();
    vi.mocked(writeConfig).mockReturnValueOnce(write.promise);
    await act(async () => {
      void api.saveConfig({
        ...api.config,
        weights: { ...api.config.weights, care: 2 },
      });
    });
    await mount(nextCwd());
    expect(api.status).toBe("ready");
    await act(async () => write.resolve());
  });

  it("drops an older unforced answer that lands after a forced one", async () => {
    const first = deferred<LoadResponse>();
    vi.mocked(loadStored).mockReset().mockReturnValueOnce(first.promise);
    await mount(nextCwd());
    const forced = deferred<LoadResponse>();
    vi.mocked(loadStored).mockReturnValueOnce(forced.promise);
    await act(async () => {
      void api.refresh(true);
    });
    await act(async () =>
      forced.resolve(
        loadFixture({ raw: { ...rawFixture(), ghAvailable: true } }),
      ),
    );
    await act(async () =>
      first.resolve(
        loadFixture({
          raw: { ...rawFixture(), ghAvailable: false, ghNote: "old" },
        }),
      ),
    );
    expect(api.raw?.ghAvailable).toBe(true);
  });

  it("does not refetch when the current branch is picked explicitly", async () => {
    await mount(nextCwd());
    const calls = vi.mocked(loadStored).mock.calls.length;
    await act(async () => api.setBranch("main"));
    expect(vi.mocked(loadStored).mock.calls.length).toBe(calls);
    expect(api.branch).toBe("main");
  });

  it("keeps showing the previous data, marked stale, while a new period loads", async () => {
    await mount(nextCwd());
    const gate = deferred<LoadResponse>();
    vi.mocked(loadStored).mockReturnValueOnce(gate.promise);
    await act(async () => api.setKind("month"));
    expect(api.status).toBe("ready");
    expect(api.stale).toBe(true);
    expect(api.model).toBeDefined();
    await act(async () => gate.resolve(loadFixture()));
    expect(api.stale).toBe(false);
  });
});

describe("useContributors automatic reloads", () => {
  const T0 = Date.parse("2026-10-15T10:00:00Z");

  it("reloads at most once per 15 seconds, and focus plus visibility count once", async () => {
    const clock = vi.spyOn(Date, "now").mockReturnValue(T0);
    await mount(nextCwd());
    const base = vi.mocked(loadStored).mock.calls.length;
    await act(async () => {
      window.dispatchEvent(new Event("focus"));
      document.dispatchEvent(new Event("visibilitychange"));
    });
    expect(vi.mocked(loadStored).mock.calls.length).toBe(base + 1);
    clock.mockReturnValue(T0 + 5_000);
    await act(async () => {
      window.dispatchEvent(new Event("focus"));
    });
    expect(vi.mocked(loadStored).mock.calls.length).toBe(base + 1);
    clock.mockReturnValue(T0 + 16_000);
    await act(async () => {
      window.dispatchEvent(new Event("focus"));
    });
    expect(vi.mocked(loadStored).mock.calls.length).toBe(base + 2);
    clock.mockRestore();
  });

  it("lets a manual refresh through the throttle", async () => {
    await mount(nextCwd());
    await act(async () => {
      window.dispatchEvent(new Event("focus"));
    });
    const calls = vi.mocked(loadStored).mock.calls.length;
    await act(async () => api.refresh(true));
    expect(vi.mocked(loadStored).mock.calls.length).toBe(calls + 1);
  });

  it("moves the 7d period to the new day after midnight without a refocus", async () => {
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout", "Date"] });
    try {
      vi.setSystemTime(new Date(2026, 9, 15, 23, 59, 0));
      await mount(nextCwd());
      expect(api.model?.period.end).toEqual(new Date(2026, 9, 16));
      await act(async () => {
        await vi.advanceTimersByTimeAsync(2 * 60_000);
      });
      expect(api.model?.period.end).toEqual(new Date(2026, 9, 17));
    } finally {
      vi.useRealTimers();
    }
  });
});

describe("useContributors period", () => {
  it("defaults to the last 7 days, with nothing ahead and no stepping back", async () => {
    await mount(nextCwd());
    expect(api.period.kind).toBe("7d");
    expect(api.hasNext).toBe(false);
    const before = vi.mocked(loadStored).mock.calls.length;
    await act(async () => api.goPrevious());
    await act(async () => api.goNext());
    expect(vi.mocked(loadStored).mock.calls.length).toBe(before);
    expect(api.period.kind).toBe("7d");
  });

  it("selects the current period of a kind and remembers the kind alone", async () => {
    await mount(nextCwd());
    for (const kind of ["month", "quarter", "year"] as const) {
      await act(async () => api.setKind(kind));
      expect(api.period.id).toBe(currentPeriod(kind, new Date()).id);
      expect(api.model?.period.id).toBe(api.period.id);
      expect(api.hasNext).toBe(false);
      expect(localStorage.getItem(RANGE_KEY)).toBe(kind);
    }
  });

  it("steps back and forward and ranks the stepped period", async () => {
    await mount(nextCwd());
    await act(async () => api.setKind("quarter"));
    const current = currentPeriod("quarter", new Date());
    await act(async () => api.goPrevious());
    expect(api.period.id).toBe(previousPeriod(current).id);
    expect(api.model?.period.id).toBe(previousPeriod(current).id);
    expect(api.model?.previousPeriod.id).toBe(
      previousPeriod(previousPeriod(current)).id,
    );
    expect(api.hasNext).toBe(true);
    expect(localStorage.getItem(RANGE_KEY)).toBe(
      `quarter:${previousPeriod(current).id}`,
    );
    await act(async () => api.goNext());
    expect(api.period.id).toBe(current.id);
    expect(api.hasNext).toBe(false);
    expect(localStorage.getItem(RANGE_KEY)).toBe("quarter");
  });

  it("does not step past the current period", async () => {
    await mount(nextCwd());
    await act(async () => api.setKind("year"));
    const before = vi.mocked(loadStored).mock.calls.length;
    await act(async () => api.goNext());
    expect(api.period.id).toBe(currentPeriod("year", new Date()).id);
    expect(vi.mocked(loadStored).mock.calls.length).toBe(before);
  });

  it("keeps showing the old period, marked stale, while the new one loads", async () => {
    await mount(nextCwd());
    await act(async () => api.setKind("month"));
    const gate = deferred<LoadResponse>();
    vi.mocked(loadStored).mockReturnValueOnce(gate.promise);
    await act(async () => api.goPrevious());
    expect(api.period.id).toBe(monthBack(1).id);
    expect(api.stale).toBe(true);
    expect(api.model?.period.id).toBe(monthBack(0).id);
    await act(async () => gate.resolve(loadFixture()));
    expect(api.stale).toBe(false);
    expect(api.model?.period.id).toBe(monthBack(1).id);
  });

  it("follows the calendar when the current month turns, but not a pinned past month", async () => {
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout", "Date"] });
    try {
      vi.setSystemTime(new Date(2026, 9, 31, 23, 59, 0));
      await mount(nextCwd());
      await act(async () => api.setKind("month"));
      expect(api.period.id).toBe("2026-10");
      await act(async () => {
        await vi.advanceTimersByTimeAsync(2 * 60_000);
      });
      expect(api.period.id).toBe("2026-11");
      expect(api.model?.period.id).toBe("2026-11");
      await act(async () => api.goPrevious());
      await act(async () => api.goPrevious());
      expect(api.period.id).toBe("2026-09");
      vi.setSystemTime(new Date(2026, 10, 30, 23, 59, 0));
      await act(async () => {
        await vi.advanceTimersByTimeAsync(2 * 60_000);
      });
      expect(api.period.id).toBe("2026-09");
    } finally {
      vi.useRealTimers();
    }
  });
});

describe("useContributors cache", () => {
  it("keeps only the most recent entries", async () => {
    for (let i = 0; i < 9; i += 1) await mount(nextCwd());
    const size = contributorsCacheSize();
    expect(size.loads).toBeLessThanOrEqual(6);
    expect(size.meta).toBeLessThanOrEqual(6);
    expect(size.loads).toBeGreaterThan(0);
  });
});

describe("useContributors sync store states", () => {
  it("a never synced period only loads and estimates, and starts nothing", async () => {
    vi.mocked(loadStored).mockResolvedValue(unsyncedFixture());
    await mount(nextCwd());
    expect(api.status).toBe("ready");
    expect(api.sync.state).toBe("notSynced");
    expect(api.sync.estimate).toEqual(ESTIMATE);
    // Cheap read of the store, one estimate, and nothing else.
    expect(loadStored).toHaveBeenCalledTimes(1);
    expect(syncEstimate).toHaveBeenCalledTimes(1);
    expect(startSync).not.toHaveBeenCalled();
    expect(cancelSync).not.toHaveBeenCalled();
    // Numbers are never shown without a sync.
    expect(api.model).toBeUndefined();
    expect(api.raw).toBeUndefined();
  });

  it("keeps the reason when the estimate fails, and Sync stays available", async () => {
    vi.mocked(loadStored).mockResolvedValue(unsyncedFixture());
    vi.mocked(syncEstimate).mockRejectedValue("git is not responding");
    await mount(nextCwd());
    expect(api.sync.state).toBe("notSynced");
    expect(api.sync.estimate).toBeNull();
    expect(api.sync.estimateError).toBe("git is not responding");
    await act(async () => api.startSync());
    expect(startSync).toHaveBeenCalledTimes(1);
  });

  it("drops a queued entry that Rust never turned into a job (a duplicate request)", async () => {
    vi.mocked(loadStored).mockResolvedValue(unsyncedFixture());
    // Rust accepted the request but lists nothing for this path.
    vi.mocked(startSync).mockResolvedValue(undefined);
    vi.mocked(syncStatus).mockResolvedValue([]);
    await mount(nextCwd());
    await act(async () => api.startSync());
    expect(api.sync.job).toBeNull();
  });

  it("estimates the window the sync would use", async () => {
    vi.mocked(loadStored).mockResolvedValue(unsyncedFixture());
    await mount(nextCwd());
    const period = currentPeriod("7d", new Date());
    expect(vi.mocked(syncEstimate).mock.calls[0][1]).toEqual({
      from: isoSeconds(syncWindow(period, new Date()).from),
      to: isoSeconds(period.end),
      branch: undefined,
    });
  });

  it("an incomplete period shows the estimate of what is left, never partial numbers", async () => {
    vi.mocked(loadStored).mockResolvedValue(
      unsyncedFixture({
        raw: rawFixture(),
        missingCommits: 12,
        syncedAt: NOW_SECS() - 60,
        coverageHead: "abc123",
      }),
    );
    await mount(nextCwd());
    expect(api.sync.state).toBe("notSynced");
    expect(api.model).toBeUndefined();
    expect(api.raw).toBeUndefined();
    expect(api.sync.estimate).toEqual(ESTIMATE);
    expect(startSync).not.toHaveBeenCalled();
  });

  it("a covered period shows its data and the sync time", async () => {
    const at = NOW_SECS() - 300;
    vi.mocked(loadStored).mockResolvedValue(loadFixture({ syncedAt: at }));
    await mount(nextCwd());
    expect(api.sync.state).toBe("synced");
    expect(api.sync.syncedAt).toBe(at);
    expect(api.model).toBeDefined();
    expect(syncEstimate).not.toHaveBeenCalled();
    expect(startSync).not.toHaveBeenCalled();
  });

  it("Sync starts a full job for the period window", async () => {
    vi.mocked(loadStored).mockResolvedValue(unsyncedFixture());
    await mount(nextCwd());
    await act(async () => api.startSync());
    const period = currentPeriod("7d", new Date());
    expect(vi.mocked(startSync)).toHaveBeenCalledTimes(1);
    expect(vi.mocked(startSync).mock.calls[0][1]).toEqual({
      from: isoSeconds(syncWindow(period, new Date()).from),
      to: isoSeconds(period.end),
      branch: undefined,
      mode: "full",
    });
  });

  it("progress events of this project update the job and done reloads", async () => {
    const cwd = nextCwd();
    vi.mocked(loadStored).mockResolvedValue(unsyncedFixture());
    await mount(cwd);
    await act(async () => api.startSync());
    expect(api.sync.job?.state).toBe("queued");

    await emit(
      syncEvent(cwd, { step: "commits", done: 30, total: 120, percent: 12 }),
    );
    expect(api.sync.job).toMatchObject({
      state: "running",
      step: "commits",
      done: 30,
      total: 120,
      percent: 12,
    });
    // Nothing is reloaded while the job runs.
    expect(loadStored).toHaveBeenCalledTimes(1);

    vi.mocked(loadStored).mockResolvedValue(loadFixture());
    await emit(syncEvent(cwd, { state: "done", percent: 100 }));
    expect(loadStored).toHaveBeenCalledTimes(2);
    expect(api.sync.state).toBe("synced");
    expect(api.sync.job).toBeNull();
    expect(api.model).toBeDefined();
  });

  it("ignores events of other projects and of other periods", async () => {
    const cwd = nextCwd();
    vi.mocked(loadStored).mockResolvedValue(unsyncedFixture());
    await mount(cwd);
    await emit(syncEvent("/somewhere/else"));
    expect(api.sync.job).toBeNull();
    const month = windowOf(currentPeriod("month", new Date()));
    await emit(syncEvent(cwd, month));
    expect(api.sync.job).toBeNull();
    await emit(syncEvent(cwd, { branch: "dev" }));
    expect(api.sync.job).toBeNull();
    await emit(syncEvent(cwd));
    expect(api.sync.job?.state).toBe("running");
    expect(loadStored).toHaveBeenCalledTimes(1);
  });

  it("picks up a job that was already running when the page opened", async () => {
    const cwd = nextCwd();
    vi.mocked(loadStored).mockResolvedValue(unsyncedFixture());
    vi.mocked(syncStatus).mockResolvedValue([
      syncEvent(cwd, { step: "pullRequests", done: 2, total: 9, percent: 55 }),
      syncEvent("/elsewhere"),
    ]);
    await mount(cwd);
    expect(api.sync.job).toMatchObject({ step: "pullRequests", percent: 55 });
  });

  it("a failed job shows its error and Sync can be pressed again", async () => {
    const cwd = nextCwd();
    vi.mocked(loadStored).mockResolvedValue(unsyncedFixture());
    await mount(cwd);
    await emit(syncEvent(cwd, { state: "error", error: "git exploded" }));
    expect(api.sync.job).toBeNull();
    expect(api.sync.jobError).toBe("git exploded");
    await act(async () => api.startSync());
    expect(api.sync.jobError).toBeUndefined();
    expect(api.sync.job?.state).toBe("queued");
  });

  it("a rejected start is reported as the job error", async () => {
    vi.mocked(loadStored).mockResolvedValue(unsyncedFixture());
    vi.mocked(startSync).mockRejectedValueOnce("Could not start the sync");
    await mount(nextCwd());
    await act(async () => api.startSync());
    expect(api.sync.job).toBeNull();
    expect(api.sync.jobError).toBe("Could not start the sync");
  });

  it("a cancelled job reloads what was stored and shows the new estimate", async () => {
    const cwd = nextCwd();
    vi.mocked(loadStored).mockResolvedValue(unsyncedFixture());
    await mount(cwd);
    await act(async () => api.startSync());
    vi.mocked(loadStored).mockResolvedValue(
      unsyncedFixture({ missingCommits: 15 }),
    );
    vi.mocked(syncEstimate).mockResolvedValue({
      ...ESTIMATE,
      storedCommits: 25,
    });
    await emit(syncEvent(cwd, { state: "cancelled" }));
    expect(api.sync.state).toBe("notSynced");
    expect(api.sync.job).toBeNull();
    expect(api.sync.estimate?.storedCommits).toBe(25);
  });

  it("cancel asks Rust to stop the jobs of the project", async () => {
    const cwd = nextCwd();
    vi.mocked(loadStored).mockResolvedValue(unsyncedFixture());
    await mount(cwd);
    await act(async () => api.cancelSync());
    expect(cancelSync).toHaveBeenCalledWith(cwd);
  });

  it("switching period while syncing keeps the job and shows the new period's state", async () => {
    const cwd = nextCwd();
    vi.mocked(loadStored).mockResolvedValue(unsyncedFixture());
    await mount(cwd);
    await act(async () => api.startSync());
    await emit(syncEvent(cwd, { percent: 40 }));
    expect(api.sync.job?.percent).toBe(40);

    await act(async () => api.setKind("month"));
    expect(cancelSync).not.toHaveBeenCalled();
    expect(api.sync.state).toBe("notSynced");
    expect(api.sync.job).toBeNull();

    // The job of the first period is still known when the page returns to it.
    await act(async () => api.setKind("7d"));
    expect(api.sync.job?.percent).toBe(40);
  });

  it("a second Sync for another period queues instead of replacing the running one", async () => {
    const cwd = nextCwd();
    vi.mocked(loadStored).mockResolvedValue(unsyncedFixture());
    await mount(cwd);
    await act(async () => api.startSync());
    await emit(syncEvent(cwd, { percent: 40 }));
    await act(async () => api.setKind("month"));
    await act(async () => api.startSync());
    expect(api.sync.job?.state).toBe("queued");
    expect(startSync).toHaveBeenCalledTimes(2);
    await act(async () => api.setKind("7d"));
    expect(api.sync.job?.state).toBe("running");
  });

  it("keeps the data on screen with a job spinner while a re-sync runs", async () => {
    const cwd = nextCwd();
    await mount(cwd);
    await act(async () => api.startSync());
    expect(vi.mocked(startSync).mock.calls[0][1].mode).toBe("full");
    expect(api.sync.state).toBe("synced");
    expect(api.sync.job?.state).toBe("queued");
    expect(api.model).toBeDefined();
  });
});

describe("useContributors top-up", () => {
  const moved = (over: Partial<LoadResponse> = {}) =>
    loadFixture({ coverageHead: "old1", head: "new2", ...over });

  it("starts one top-up from the coverage head when the branch head moved", async () => {
    vi.mocked(loadStored).mockResolvedValue(moved());
    await mount(nextCwd());
    expect(startSync).toHaveBeenCalledTimes(1);
    const period = currentPeriod("7d", new Date());
    expect(vi.mocked(startSync).mock.calls[0][1]).toEqual({
      from: isoSeconds(syncWindow(period, new Date()).from),
      to: isoSeconds(period.end),
      branch: undefined,
      mode: "topup",
      oldHead: "old1",
    });
    expect(api.model).toBeDefined();
    expect(api.sync.job?.state).toBe("queued");
  });

  it("does not start another one for the same head on a reload or a remount", async () => {
    const cwd = nextCwd();
    vi.mocked(loadStored).mockResolvedValue(moved());
    await mount(cwd);
    await act(async () => api.refresh(true));
    await act(async () => {
      window.dispatchEvent(new Event("focus"));
    });
    await act(async () => root.unmount());
    root = createRoot(container);
    await mount(cwd);
    expect(startSync).toHaveBeenCalledTimes(1);
  });

  it("starts another one when the head moves again and the first one ended", async () => {
    const cwd = nextCwd();
    vi.mocked(loadStored).mockResolvedValue(moved());
    await mount(cwd);
    await emit(syncEvent(cwd, { state: "done", percent: 100 }));
    vi.mocked(loadStored).mockResolvedValue(moved({ head: "new3" }));
    await act(async () => api.refresh(true));
    expect(startSync).toHaveBeenCalledTimes(2);
    expect(vi.mocked(startSync).mock.calls[1][1].oldHead).toBe("old1");
  });

  it("does nothing when the coverage head equals the head", async () => {
    vi.mocked(loadStored).mockResolvedValue(
      loadFixture({ coverageHead: "same", head: "same" }),
    );
    await mount(nextCwd());
    expect(startSync).not.toHaveBeenCalled();
  });

  it("does not top up a period that is not covered, even with different heads", async () => {
    vi.mocked(loadStored).mockResolvedValue(
      unsyncedFixture({ coverageHead: "old1", head: "new2" }),
    );
    await mount(nextCwd());
    expect(startSync).not.toHaveBeenCalled();
  });

  it("a synced period that lacks commits keeps its data and is repaired by one full sync per head", async () => {
    const cwd = nextCwd();
    const lacking = (over: Partial<LoadResponse> = {}) =>
      loadFixture({
        covered: false,
        hasGaps: false,
        missingCommits: 3,
        coverageHead: "old1",
        head: "new2",
        ...over,
      });
    vi.mocked(loadStored).mockResolvedValue(lacking());
    await mount(cwd);
    // Numbers stay on screen: the stored commits are shown while it updates.
    expect(api.sync.state).toBe("synced");
    expect(api.model).toBeDefined();
    expect(syncEstimate).not.toHaveBeenCalled();
    expect(startSync).toHaveBeenCalledTimes(1);
    expect(vi.mocked(startSync).mock.calls[0][1].mode).toBe("full");
    expect(api.sync.job?.state).toBe("queued");

    // Not again for the same head, even when the job ended and it still lacks.
    await emit(syncEvent(cwd, { state: "done", percent: 100 }));
    await act(async () => api.refresh(true));
    expect(startSync).toHaveBeenCalledTimes(1);

    // A moved head is a new reason to repair.
    vi.mocked(loadStored).mockResolvedValue(lacking({ head: "new3" }));
    await act(async () => api.refresh(true));
    expect(startSync).toHaveBeenCalledTimes(2);
  });

  it("repairs a lack at the same head as well", async () => {
    vi.mocked(loadStored).mockResolvedValue(
      loadFixture({
        covered: false,
        hasGaps: false,
        missingCommits: 2,
        coverageHead: "same",
        head: "same",
      }),
    );
    await mount(nextCwd());
    expect(vi.mocked(startSync).mock.calls[0][1].mode).toBe("full");
    expect(api.model).toBeDefined();
  });

  it("a gap (never synced or cancelled) stays manual even with a coverage head", async () => {
    vi.mocked(loadStored).mockResolvedValue(
      unsyncedFixture({
        raw: rawFixture(),
        coverageHead: "old1",
        head: "new2",
        missingCommits: 12,
      }),
    );
    await mount(nextCwd());
    expect(api.sync.state).toBe("notSynced");
    expect(api.model).toBeUndefined();
    expect(startSync).not.toHaveBeenCalled();
  });

  it("a repair of nothing missing is not started", async () => {
    vi.mocked(loadStored).mockResolvedValue(
      loadFixture({ covered: false, hasGaps: false, missingCommits: 0 }),
    );
    await mount(nextCwd());
    expect(startSync).not.toHaveBeenCalled();
  });

  it("reloads when the top-up is done and drops the spinner", async () => {
    const cwd = nextCwd();
    vi.mocked(loadStored).mockResolvedValue(moved());
    await mount(cwd);
    expect(api.sync.job).not.toBeNull();
    vi.mocked(loadStored).mockResolvedValue(
      loadFixture({ coverageHead: "new2", head: "new2" }),
    );
    await emit(syncEvent(cwd, { state: "done", percent: 100 }));
    expect(api.sync.job).toBeNull();
    expect(startSync).toHaveBeenCalledTimes(1);
  });

  it("does not top up a remote project (empty cwd)", async () => {
    vi.mocked(loadStored).mockResolvedValue(moved());
    await mount("");
    expect(loadStored).not.toHaveBeenCalled();
    expect(startSync).not.toHaveBeenCalled();
  });
});

describe("useContributors GitHub state and clearing", () => {
  it.each(["partial", "unavailable"] as const)(
    "flags %s GitHub data and Retry GitHub starts a github job",
    async (ghState) => {
      vi.mocked(loadStored).mockResolvedValue(loadFixture({ ghState }));
      await mount(nextCwd());
      expect(api.sync.ghIncomplete).toBe(true);
      expect(api.model).toBeDefined();
      await act(async () => api.retryGithub());
      expect(vi.mocked(startSync).mock.calls[0][1].mode).toBe("github");
    },
  );

  it("does not flag complete GitHub data", async () => {
    await mount(nextCwd());
    expect(api.sync.ghIncomplete).toBe(false);
  });

  it("clearing deletes the synced data and shows the empty state after the reload", async () => {
    const cwd = nextCwd();
    await mount(cwd);
    expect(api.sync.state).toBe("synced");
    vi.mocked(loadStored).mockResolvedValue(unsyncedFixture());
    await act(async () => api.clearSyncedData());
    expect(clearSynced).toHaveBeenCalledWith(cwd);
    expect(api.sync.state).toBe("notSynced");
    expect(api.model).toBeUndefined();
  });

  it("clearing allows a top-up for the same head again", async () => {
    const cwd = nextCwd();
    vi.mocked(loadStored).mockResolvedValue(
      loadFixture({ coverageHead: "old1", head: "new2" }),
    );
    await mount(cwd);
    expect(startSync).toHaveBeenCalledTimes(1);
    await act(async () => api.clearSyncedData());
    await act(async () => api.refresh(true));
    expect(startSync).toHaveBeenCalledTimes(2);
  });

  it("reports why clearing was refused and keeps the data", async () => {
    vi.mocked(clearSynced).mockRejectedValueOnce(
      "A sync of this repository is running. Cancel it first.",
    );
    await mount(nextCwd());
    await act(async () => api.clearSyncedData());
    expect(api.sync.actionError).toBe(
      "A sync of this repository is running. Cancel it first.",
    );
    expect(api.model).toBeDefined();
  });
});
