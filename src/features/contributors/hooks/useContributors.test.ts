// @vitest-environment happy-dom
import { act, createElement, StrictMode } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("../model/api", () => ({
  readConfig: vi.fn(),
  readMailmap: vi.fn(),
  writeConfig: vi.fn(),
  collectRaw: vi.fn(),
  listBranches: vi.fn(),
  gitUser: vi.fn(),
}));

import {
  collectRaw,
  gitUser,
  listBranches,
  readConfig,
  readMailmap,
  writeConfig,
} from "../model/api";
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
  vi.mocked(collectRaw).mockImplementation(async () => rawFixture());
});

afterEach(async () => {
  await act(async () => root.unmount());
  container.remove();
  vi.clearAllMocks();
  vi.unstubAllGlobals();
});

describe("useContributors", () => {
  it("goes from loading to ready with raw data, branches and a model", async () => {
    const gate = deferred<ReturnType<typeof rawFixture>>();
    vi.mocked(collectRaw).mockReturnValueOnce(gate.promise);
    await mount(nextCwd());
    expect(api.status).toBe("loading");
    expect(api.model).toBeUndefined();

    await act(async () => gate.resolve(rawFixture()));
    expect(api.status).toBe("ready");
    expect(api.raw?.commits).toHaveLength(1);
    expect(api.branches).toEqual(["main", "dev"]);
    expect(api.branch).toBe("main");
    expect(api.model?.stats.kpis.commits.value).toBe(1);
    expect(api.model?.meId).toBe("ana@example.com");
    expect(api.config).toEqual(DEFAULT_CONFIG);
  });

  it("collects the window of the default 7d period: from the previous 7 days to tomorrow", async () => {
    await mount(nextCwd());
    const args = vi.mocked(collectRaw).mock.calls[0][1];
    const window = syncWindow(currentPeriod("7d", new Date()), new Date());
    expect(args.since).toBe(isoSeconds(window.from));
    expect(args.until).toBe(isoSeconds(window.to));
    expect(args.force).toBe(false);
    expect(args.branch).toBeUndefined();
    expect(api.period.kind).toBe("7d");
    expect(api.model?.period.id).toBe("7d");
  });

  it("collects a past month from the start of the month before it to its own end", async () => {
    await mount(nextCwd());
    await selectMonthBack(3);
    const args = vi.mocked(collectRaw).mock.calls.at(-1)![1];
    expect(args.since).toBe(isoSeconds(monthBack(4).start));
    expect(args.until).toBe(isoSeconds(monthBack(3).end));
    expect(api.model?.period.id).toBe(MONTH_A.id);
  });

  it("caps the current month at tomorrow", async () => {
    await mount(nextCwd());
    await act(async () => api.setKind("month"));
    const args = vi.mocked(collectRaw).mock.calls.at(-1)![1];
    const today = new Date();
    const tomorrow = new Date(
      today.getFullYear(),
      today.getMonth(),
      today.getDate() + 1,
    );
    expect(args.until).toBe(isoSeconds(tomorrow));
    expect(args.since).toBe(isoSeconds(monthBack(1).start));
  });

  it("collects again when the period changes, and not when it does not", async () => {
    await mount(nextCwd());
    await act(async () => api.setKind("month"));
    const base = vi.mocked(collectRaw).mock.calls.length;
    await act(async () => api.setKind("month"));
    expect(vi.mocked(collectRaw).mock.calls.length).toBe(base);
    await act(async () => api.goPrevious());
    expect(vi.mocked(collectRaw).mock.calls.length).toBe(base + 1);
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

    const gate = deferred<ReturnType<typeof rawFixture>>();
    vi.mocked(collectRaw).mockReturnValueOnce(gate.promise);
    await mount(cwd);
    expect(api.status).toBe("ready");
    expect(api.refreshing).toBe(true);
    await act(async () => gate.resolve(rawFixture()));
    expect(api.refreshing).toBe(false);
  });

  it("surfaces a load failure and recovers on a forced retry", async () => {
    vi.mocked(collectRaw).mockRejectedValueOnce("git exploded");
    await mount(nextCwd());
    expect(api.status).toBe("error");
    expect(api.error).toBe("git exploded");

    await act(async () => api.refresh(true));
    expect(api.status).toBe("ready");
    expect(api.error).toBeUndefined();
    expect(vi.mocked(collectRaw).mock.calls.at(-1)?.[1].force).toBe(true);
  });

  it("drops the old error at once when retrying and shows loading until the answer", async () => {
    vi.mocked(collectRaw).mockRejectedValueOnce("git exploded");
    await mount(nextCwd());
    expect(api.status).toBe("error");

    const gate = deferred<ReturnType<typeof rawFixture>>();
    vi.mocked(collectRaw).mockReturnValueOnce(gate.promise);
    let retry!: Promise<void>;
    await act(async () => {
      retry = api.refresh(true);
    });
    expect(api.error).toBeUndefined();
    expect(api.status).toBe("loading");
    expect(api.refreshing).toBe(true);

    await act(async () => {
      gate.resolve(rawFixture());
      await retry;
    });
    expect(api.status).toBe("ready");
  });

  it("drops a refresh failure banner at once when refreshing again", async () => {
    await mount(nextCwd());
    vi.mocked(collectRaw).mockRejectedValueOnce("offline");
    await act(async () => api.refresh(true));
    expect(api.status).toBe("ready");
    expect(api.error).toBe("offline");

    const gate = deferred<ReturnType<typeof rawFixture>>();
    vi.mocked(collectRaw).mockReturnValueOnce(gate.promise);
    await act(async () => {
      void api.refresh(true);
    });
    expect(api.error).toBeUndefined();
    await act(async () => gate.resolve(rawFixture()));
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
    expect(collectRaw).toHaveBeenCalledTimes(1);
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

  it("forces a refresh through to Rust", async () => {
    await mount(nextCwd());
    await act(async () => api.refresh(true));
    expect(vi.mocked(collectRaw).mock.calls.at(-1)?.[1].force).toBe(true);
  });

  it("revalidates on window focus without forcing", async () => {
    await mount(nextCwd());
    const before = vi.mocked(collectRaw).mock.calls.length;
    await act(async () => {
      window.dispatchEvent(new Event("focus"));
    });
    expect(vi.mocked(collectRaw).mock.calls.length).toBe(before + 1);
    expect(vi.mocked(collectRaw).mock.calls.at(-1)?.[1].force).toBe(false);
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
    expect(vi.mocked(collectRaw).mock.calls.at(-1)?.[1].branch).toBe("dev");
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
    const a = deferred<ReturnType<typeof rawFixture>>();
    const b = deferred<ReturnType<typeof rawFixture>>();
    vi.mocked(collectRaw)
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
    const a = deferred<ReturnType<typeof rawFixture>>();
    vi.mocked(collectRaw)
      .mockReset()
      .mockReturnValueOnce(a.promise)
      .mockRejectedValueOnce("B failed");
    await mount(nextCwd());
    await act(async () => api.goPrevious());
    expect(api.error).toBe("B failed");
    await act(async () => a.resolve(rawFixture()));
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
    const first = deferred<ReturnType<typeof rawFixture>>();
    vi.mocked(collectRaw).mockReset().mockReturnValueOnce(first.promise);
    await mount(nextCwd());
    const forced = deferred<ReturnType<typeof rawFixture>>();
    vi.mocked(collectRaw).mockReturnValueOnce(forced.promise);
    await act(async () => {
      void api.refresh(true);
    });
    await act(async () =>
      forced.resolve({ ...rawFixture(), ghAvailable: true }),
    );
    await act(async () =>
      first.resolve({ ...rawFixture(), ghAvailable: false, ghNote: "old" }),
    );
    expect(api.raw?.ghAvailable).toBe(true);
  });

  it("does not refetch when the current branch is picked explicitly", async () => {
    await mount(nextCwd());
    const calls = vi.mocked(collectRaw).mock.calls.length;
    await act(async () => api.setBranch("main"));
    expect(vi.mocked(collectRaw).mock.calls.length).toBe(calls);
    expect(api.branch).toBe("main");
  });

  it("keeps showing the previous data, marked stale, while a new period loads", async () => {
    await mount(nextCwd());
    const gate = deferred<ReturnType<typeof rawFixture>>();
    vi.mocked(collectRaw).mockReturnValueOnce(gate.promise);
    await act(async () => api.setKind("month"));
    expect(api.status).toBe("ready");
    expect(api.stale).toBe(true);
    expect(api.model).toBeDefined();
    await act(async () => gate.resolve(rawFixture()));
    expect(api.stale).toBe(false);
  });
});

describe("useContributors automatic reloads", () => {
  const T0 = Date.parse("2026-10-15T10:00:00Z");

  it("reloads at most once per 15 seconds, and focus plus visibility count once", async () => {
    const clock = vi.spyOn(Date, "now").mockReturnValue(T0);
    await mount(nextCwd());
    const base = vi.mocked(collectRaw).mock.calls.length;
    await act(async () => {
      window.dispatchEvent(new Event("focus"));
      document.dispatchEvent(new Event("visibilitychange"));
    });
    expect(vi.mocked(collectRaw).mock.calls.length).toBe(base + 1);
    clock.mockReturnValue(T0 + 5_000);
    await act(async () => {
      window.dispatchEvent(new Event("focus"));
    });
    expect(vi.mocked(collectRaw).mock.calls.length).toBe(base + 1);
    clock.mockReturnValue(T0 + 16_000);
    await act(async () => {
      window.dispatchEvent(new Event("focus"));
    });
    expect(vi.mocked(collectRaw).mock.calls.length).toBe(base + 2);
    clock.mockRestore();
  });

  it("lets a manual refresh through the throttle", async () => {
    await mount(nextCwd());
    await act(async () => {
      window.dispatchEvent(new Event("focus"));
    });
    const calls = vi.mocked(collectRaw).mock.calls.length;
    await act(async () => api.refresh(true));
    expect(vi.mocked(collectRaw).mock.calls.length).toBe(calls + 1);
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
    const before = vi.mocked(collectRaw).mock.calls.length;
    await act(async () => api.goPrevious());
    await act(async () => api.goNext());
    expect(vi.mocked(collectRaw).mock.calls.length).toBe(before);
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
    const before = vi.mocked(collectRaw).mock.calls.length;
    await act(async () => api.goNext());
    expect(api.period.id).toBe(currentPeriod("year", new Date()).id);
    expect(vi.mocked(collectRaw).mock.calls.length).toBe(before);
  });

  it("keeps showing the old period, marked stale, while the new one loads", async () => {
    await mount(nextCwd());
    await act(async () => api.setKind("month"));
    const gate = deferred<ReturnType<typeof rawFixture>>();
    vi.mocked(collectRaw).mockReturnValueOnce(gate.promise);
    await act(async () => api.goPrevious());
    expect(api.period.id).toBe(monthBack(1).id);
    expect(api.stale).toBe(true);
    expect(api.model?.period.id).toBe(monthBack(0).id);
    await act(async () => gate.resolve(rawFixture()));
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
    expect(size.raw).toBeLessThanOrEqual(6);
    expect(size.meta).toBeLessThanOrEqual(6);
    expect(size.raw).toBeGreaterThan(0);
  });
});
