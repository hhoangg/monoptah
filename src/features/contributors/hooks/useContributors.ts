import {
  useCallback,
  useEffect,
  useMemo,
  useReducer,
  useRef,
  useState,
} from "react";
import {
  clearSynced,
  gitUser,
  listBranches,
  loadStored,
  readConfig,
  readMailmap,
  syncEstimate,
  writeConfig,
} from "../model/api";
import type { ContributorsBranches, GitUser } from "../model/api";
import {
  CONFIG_PATH,
  parseConfig,
  parseConfigResult,
  serializeConfig,
} from "../model/config";
import { parseMailmap } from "../model/identity";
import type { MailmapPerson } from "../model/identity";
import { buildContributorsModel } from "../model/pipeline";
import type { ContributorsModel } from "../model/pipeline";
import {
  nextPeriod,
  pickOf,
  previousPeriod,
  resolvePick,
  syncWindow,
} from "../model/period";
import type { Period, PeriodKind, PeriodPick } from "../model/period";
import {
  readMerge,
  readPeriodPick,
  writeMerge,
  writePeriodPick,
} from "../model/prefs";
import { isLive } from "../model/syncEvents";
import {
  cancelSyncJobs,
  ensureSyncJobs,
  forgetSyncJobs,
  jobKey,
  onSyncJobEnded,
  resetSyncJobs,
  startSyncJob,
  useSyncJobs,
} from "../model/syncJobs";
import type {
  ContributorsConfig,
  LoadResponse,
  RawData,
  SyncEstimate,
  SyncMode,
} from "../model/types";

type Meta = {
  /** What the page shows; moves ahead of `confirmed` while a save is in flight. */
  config: ContributorsConfig;
  /** The last config known to be on disk. */
  confirmed: ContributorsConfig;
  /** Why the file on disk cannot be used; saving is refused while it is set. */
  configError?: string;
  mailmap: MailmapPerson[];
  branches: ContributorsBranches;
  user?: GitUser;
};

/** Writes of one project's config; module-level so they outlive a project switch. */
type SaveState = {
  pending: number;
  /** Bumped when a save starts and when it ends, so a reload that overlapped one can tell. */
  epoch: number;
  latest: ContributorsConfig | null;
  chain: Promise<void>;
};

const CACHE_LIMIT = 6;
/** Minimum gap between reloads triggered by the window coming back. */
const AUTO_RELOAD_MS = 15_000;

// Module-level so returning to the page is instant: the cached data renders
// first and a revalidation refreshes it in the background. Both caches are
// small LRUs, because the window moves daily and every period and branch adds one.
const metaCache = new Map<string, Meta>();
const loadCache = new Map<string, LoadResponse>();
const inFlight = new Map<string, Promise<LoadResponse>>();
/** Auto top-ups already started, per period and branch head: one per head. */
const toppedUp = new Set<string>();
const saves = new Map<string, SaveState>();
/** One read of a project's files at a time, so a remount does not repeat it. */
const metaInFlight = new Map<
  string,
  { epoch: number; request: Promise<Meta> }
>();
/** Sequence number of the latest read started for a key. */
const startedAt = new Map<string, number>();
let requestSeq = 0;

function remember<V>(cache: Map<string, V>, key: string, value: V) {
  cache.delete(key);
  cache.set(key, value);
  while (cache.size > CACHE_LIMIT) {
    const oldest = cache.keys().next().value;
    if (oldest === undefined) break;
    cache.delete(oldest);
  }
}

function recall<V>(cache: Map<string, V>, key: string): V | undefined {
  const value = cache.get(key);
  if (value !== undefined) {
    cache.delete(key);
    cache.set(key, value);
  }
  return value;
}

function saveState(cwd: string): SaveState {
  let state = saves.get(cwd);
  if (!state) {
    state = { pending: 0, epoch: 0, latest: null, chain: Promise.resolve() };
    saves.set(cwd, state);
  }
  return state;
}

/** Test hook: forget everything cached. */
export function resetContributorsCache() {
  metaCache.clear();
  loadCache.clear();
  inFlight.clear();
  metaInFlight.clear();
  saves.clear();
  startedAt.clear();
  toppedUp.clear();
  resetSyncJobs();
}

/** Test hook: how many entries the caches hold. */
export function contributorsCacheSize() {
  return { loads: loadCache.size, meta: metaCache.size };
}

const NO_BRANCHES: ContributorsBranches = { current: "", branches: [] };

const errorText = (error: unknown) =>
  error instanceof Error ? error.message : String(error);

async function loadMeta(cwd: string): Promise<Meta> {
  // The config must read: saving over a file that could not be read would
  // destroy it. The rest only enriches the page.
  const [text, mailmap, branches, user] = await Promise.all([
    readConfig(cwd),
    readMailmap(cwd).catch(() => null),
    listBranches(cwd).catch(() => NO_BRANCHES),
    gitUser(cwd).catch(() => undefined),
  ]);
  const { config, error } = parseConfigResult(text);
  return {
    config,
    confirmed: config,
    configError: error,
    mailmap: parseMailmap(mailmap ?? ""),
    branches,
    user,
  };
}

/**
 * Reads the files unless a plain reload already is. A forced reload always
 * reads again (the user may just have fixed the file), and so does one that
 * starts after a save began or ended, whose read could predate the write.
 */
function loadMetaShared(
  cwd: string,
  epoch: number,
  force: boolean,
): Promise<Meta> {
  const running = metaInFlight.get(cwd);
  if (!force && running && running.epoch === epoch) return running.request;
  const request = loadMeta(cwd).finally(() => {
    if (metaInFlight.get(cwd)?.request === request) metaInFlight.delete(cwd);
  });
  metaInFlight.set(cwd, { epoch, request });
  return request;
}

/**
 * Reads one period of the sync store. A plain read joins one already running;
 * a fresh one (a forced refresh, or the reload after a sync ended) never does,
 * because the running read may have looked at the store before the change.
 * Whichever read started last decides what is cached.
 */
function loadPeriod(
  cwd: string,
  branch: string | undefined,
  since: string,
  until: string,
  key: string,
  fresh: boolean,
): Promise<LoadResponse> {
  const joined = fresh ? undefined : inFlight.get(key);
  if (joined) return joined;
  const seq = (requestSeq += 1);
  startedAt.set(key, seq);
  const request = loadStored(cwd, { since, until, branch })
    .then((answer) => {
      const previous = loadCache.get(key);
      // A read started after this one was asked for: its data is newer.
      if ((startedAt.get(key) ?? 0) > seq) {
        if (!previous) remember(loadCache, key, answer);
        return loadCache.get(key) ?? answer;
      }
      // Same state of the store: keep the object so a focus revalidation does
      // not rescore.
      const next = previous && sameLoad(previous, answer) ? previous : answer;
      remember(loadCache, key, next);
      return next;
    })
    .finally(() => {
      if (inFlight.get(key) === request) inFlight.delete(key);
    });
  inFlight.set(key, request);
  return request;
}

function sameLoad(a: LoadResponse, b: LoadResponse): boolean {
  return (
    a.covered === b.covered &&
    a.missingCommits === b.missingCommits &&
    a.syncedAt === b.syncedAt &&
    a.coverageHead === b.coverageHead &&
    a.head === b.head &&
    a.ghState === b.ghState &&
    sameData(a.raw, b.raw)
  );
}

function sameData(a: RawData, b: RawData): boolean {
  return (
    a.head === b.head &&
    a.branch === b.branch &&
    a.since === b.since &&
    a.ghAvailable === b.ghAvailable &&
    a.ghNote === b.ghNote &&
    a.truncated === b.truncated &&
    a.commits.length === b.commits.length &&
    a.prs.length === b.prs.length &&
    a.reviews.length === b.reviews.length &&
    a.survival.length === b.survival.length
  );
}

const sameJson = (a: unknown, b: unknown) =>
  JSON.stringify(a) === JSON.stringify(b);

/** Whole seconds, so the cache key does not depend on milliseconds. */
const isoSeconds = (date: Date) => date.toISOString().replace(/\.\d{3}Z$/, "Z");

const epochSeconds = (date: Date) => Math.floor(date.getTime() / 1000);

const sameDay = (a: Date, b: Date) =>
  a.getFullYear() === b.getFullYear() &&
  a.getMonth() === b.getMonth() &&
  a.getDate() === b.getDate();

export function useContributorsModel(input: {
  raw?: RawData;
  config: ContributorsConfig;
  mailmap: MailmapPerson[];
  merge: boolean;
  period: Period;
  now: Date;
  user?: GitUser;
}): ContributorsModel | undefined {
  const { raw, config, mailmap, merge, period, now, user } = input;
  return useMemo(
    () =>
      raw
        ? buildContributorsModel({
            raw,
            config,
            mailmap,
            merge,
            period,
            now,
            user,
          })
        : undefined,
    [raw, config, mailmap, merge, period, now, user],
  );
}

export function useContributors(cwd: string) {
  const [pick, setPickState] = useState<PeriodPick>(() => readPeriodPick());
  const [merge, setMergeState] = useState(readMerge);
  const [picked, setPicked] = useState<{ cwd: string; branch?: string }>({
    cwd,
  });
  const [now, setNow] = useState(() => new Date());
  // One message per request scope, so a late answer for an abandoned request
  // can only touch its own entry.
  const [failures, setFailures] = useState<Record<string, string>>({});
  const [saveError, setSaveError] = useState<string | undefined>();
  const [refreshing, setRefreshing] = useState(false);
  // Jobs of every project, from the app-wide store; this page reads its own.
  const jobs = useSyncJobs();
  const [estimate, setEstimate] = useState<{
    key: string;
    value: SyncEstimate | null;
    error?: string;
  } | null>(null);
  const [actionError, setActionError] = useState<string | undefined>();
  // Caches live outside React; this re-renders after they change.
  const [, bump] = useReducer((n: number) => n + 1, 0);

  const branch = picked.cwd === cwd ? picked.branch : undefined;
  // A pick without an id follows the current period as the days pass.
  const period = useMemo(() => resolvePick(pick, now), [pick, now]);
  // The store is read from the start of the previous period (rank movement and
  // deltas compare with it) to the end of the selected one; Rust clips an end
  // in the future to now.
  const since = useMemo(
    () => isoSeconds(syncWindow(period, now).from),
    [period, now],
  );
  const until = useMemo(() => isoSeconds(period.end), [period]);
  const key = `${cwd}\u0000${branch ?? ""}\u0000${since}\u0000${until}`;
  const metaScope = `meta\u0000${cwd}`;

  const running = useRef(0);
  const lastAutoReload = useRef(0);
  const lastShown = useRef<{
    cwd: string;
    raw: RawData;
    period: Period;
  } | null>(null);

  const fail = useCallback((scope: string, message: string) => {
    setFailures((all) => ({ ...all, [scope]: message }));
  }, []);
  const clearFailure = useCallback((scope: string) => {
    setFailures((all) => {
      if (!(scope in all)) return all;
      const { [scope]: _gone, ...rest } = all;
      return rest;
    });
  }, []);

  const load = useCallback(
    async (force: boolean, fresh = force) => {
      if (!cwd) return;
      running.current += 1;
      setRefreshing(true);
      // A reload that overlaps a save of this project may have read the file
      // before or after the write, so it must not decide the config.
      const saved = saveState(cwd);
      const epoch = saved.epoch;
      const savingAtStart = saved.pending > 0;
      // A retry starts clean: the old message would sit next to the spinner.
      if (force) {
        clearFailure(key);
        clearFailure(metaScope);
      }
      const meta = loadMetaShared(cwd, epoch, force).then(
        (loaded) => {
          const previous = recall(metaCache, cwd);
          const now = saveState(cwd);
          const configSettled =
            !savingAtStart && now.pending === 0 && now.epoch === epoch;
          // Keep object identities when nothing changed, so revalidating on
          // focus does not rescore.
          const keep = <T>(old: T | undefined, next: T) =>
            old !== undefined && sameJson(old, next) ? old : next;
          const config =
            configSettled || !previous
              ? keep(previous?.config, loaded.config)
              : previous.config;
          remember(metaCache, cwd, {
            config,
            confirmed: configSettled || !previous ? config : previous.confirmed,
            configError:
              configSettled || !previous
                ? loaded.configError
                : previous.configError,
            mailmap: keep(previous?.mailmap, loaded.mailmap),
            branches: keep(previous?.branches, loaded.branches),
            user: keep(previous?.user, loaded.user),
          });
          clearFailure(metaScope);
        },
        (error) => {
          fail(metaScope, errorText(error));
        },
      );
      const raw = loadPeriod(cwd, branch, since, until, key, fresh).then(
        () => {
          clearFailure(key);
        },
        (error) => {
          fail(key, errorText(error));
        },
      );
      await Promise.all([meta, raw]);
      running.current -= 1;
      if (running.current === 0) setRefreshing(false);
      bump();
    },
    [cwd, branch, since, until, key, metaScope, fail, clearFailure],
  );

  useEffect(() => {
    void load(false);
  }, [load]);

  // Listen to the sync jobs, and read what a job that ended stored.
  const reloadRef = useRef<() => void>(() => undefined);
  reloadRef.current = () => void load(false, true);
  useEffect(() => {
    if (!cwd) return;
    void ensureSyncJobs();
    return onSyncJobEnded((event) => {
      if (event.root === cwd) reloadRef.current();
    });
  }, [cwd]);

  useEffect(() => {
    if (!cwd) return;
    const resume = () => {
      if (document.hidden) return;
      // Focus and visibilitychange fire together; the throttle makes them one.
      const at = Date.now();
      if (at - lastAutoReload.current < AUTO_RELOAD_MS) return;
      lastAutoReload.current = at;
      const fresh = new Date();
      setNow((current) => (sameDay(current, fresh) ? current : fresh));
      void load(false);
    };
    window.addEventListener("focus", resume);
    document.addEventListener("visibilitychange", resume);
    return () => {
      window.removeEventListener("focus", resume);
      document.removeEventListener("visibilitychange", resume);
    };
  }, [cwd, load]);

  // A window left open across midnight would keep ending the period at the old
  // "tomorrow" and miss the new day's commits.
  useEffect(() => {
    const nextDay = new Date(
      now.getFullYear(),
      now.getMonth(),
      now.getDate() + 1,
      0,
      0,
      1,
    );
    const wait = Math.max(1000, nextDay.getTime() - Date.now());
    const timer = setTimeout(() => {
      const fresh = new Date();
      setNow((current) => (sameDay(current, fresh) ? current : fresh));
    }, wait);
    return () => clearTimeout(timer);
  }, [now]);

  const meta = recall(metaCache, cwd);
  const loaded = recall(loadCache, key);
  // A synced period that lacks some commits (a merge brought older ones in)
  // keeps its stored numbers on screen while a background sync repairs it.
  const repair =
    !!loaded &&
    !loaded.covered &&
    !loaded.hasGaps &&
    loaded.missingCommits > 0 &&
    loaded.coverageHead !== null;
  // Only a synced period has numbers; a gap shows the sync state instead.
  const current = loaded?.covered || repair ? loaded?.raw : undefined;
  // While a new period or branch loads, keep the last data of this project on
  // screen (marked stale) instead of blanking the page.
  const previouslyShown =
    lastShown.current?.cwd === cwd ? lastShown.current : undefined;
  const shown = loaded ? current : previouslyShown?.raw;
  // The old data may not reach the newly picked period, so the period shown
  // stays with the data until the new request answers.
  const shownPeriod = loaded ? period : (previouslyShown?.period ?? period);
  if (shown) lastShown.current = { cwd, raw: shown, period: shownPeriod };
  const failed = failures[key] ?? failures[metaScope];
  const status: "loading" | "ready" | "error" =
    meta && (loaded || shown) ? "ready" : failed ? "error" : "loading";

  const windowFrom = epochSeconds(syncWindow(period, now).from);
  const windowTo = epochSeconds(period.end);
  const branchName = loaded?.raw.branch ?? "";
  const thisJob = loaded
    ? jobs[jobKey(cwd, branchName, windowFrom, windowTo)]
    : undefined;
  const liveJob = thisJob && isLive(thisJob) ? thisJob : null;
  const jobError =
    thisJob?.state === "error"
      ? (thisJob.error ?? "The sync failed.")
      : undefined;
  const notSynced = !!loaded && !loaded.covered && !repair;

  // What is left to read, for a period that has to be synced by hand.
  useEffect(() => {
    if (!cwd || !notSynced || liveJob) return;
    let stale = false;
    syncEstimate(cwd, { from: since, to: until, branch }).then(
      (value) => {
        if (!stale) setEstimate({ key, value });
      },
      (error) => {
        if (!stale) setEstimate({ key, value: null, error: errorText(error) });
      },
    );
    return () => {
      stale = true;
    };
    // `loaded` is the trigger: a changed store means a changed estimate.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [cwd, key, loaded, liveJob === null]);

  const begin = useCallback(
    (mode: SyncMode, oldHead?: string) => {
      if (!cwd || !loaded) return Promise.resolve();
      setActionError(undefined);
      return startSyncJob(cwd, {
        branchName: loaded.raw.branch,
        branch,
        window: { from: syncWindow(period, now).from, to: period.end },
        mode,
        oldHead,
      });
    },
    [cwd, loaded, branch, period, now],
  );

  // A synced period whose branch moved since the sync is topped up once per
  // head. A gap at the same head is not a top-up: the user syncs it again.
  useEffect(() => {
    if (!cwd || !loaded?.covered || !loaded.coverageHead) return;
    if (loaded.coverageHead === loaded.head || liveJob) return;
    const once = `${key}\u0000${loaded.head}`;
    if (toppedUp.has(once)) return;
    toppedUp.add(once);
    void begin("topup", loaded.coverageHead);
  }, [cwd, key, loaded, liveJob, begin]);

  // Commits the store lacks inside a synced period are read in the
  // background, once per head; a full sync only repairs what is missing.
  useEffect(() => {
    if (!cwd || !loaded || !repair || liveJob) return;
    const once = `${key}\u0000repair\u0000${loaded.head}`;
    if (toppedUp.has(once)) return;
    toppedUp.add(once);
    void begin("full");
  }, [cwd, key, loaded, repair, liveJob, begin]);

  const select = useCallback((next: PeriodPick) => {
    writePeriodPick(next);
    setPickState(next);
  }, []);
  const setKind = useCallback(
    (kind: PeriodKind) => select({ kind, id: null }),
    [select],
  );
  const goPrevious = useCallback(() => {
    if (period.kind === "7d") return;
    select(pickOf(previousPeriod(period), now));
  }, [period, now, select]);
  const goNext = useCallback(() => {
    const next = nextPeriod(period, now);
    if (next) select(pickOf(next, now));
  }, [period, now, select]);
  const setMerge = useCallback((next: boolean) => {
    writeMerge(next);
    setMergeState(next);
  }, []);
  const setBranch = useCallback(
    (next: string | undefined) => {
      // Picking the checked-out branch is the default, not a new selection.
      const isCurrent = next === metaCache.get(cwd)?.branches.current;
      setPicked({ cwd, branch: isCurrent ? undefined : next });
    },
    [cwd],
  );
  const refresh = useCallback((force: boolean) => load(force), [load]);

  const startFullSync = useCallback(() => begin("full"), [begin]);
  const retryGithub = useCallback(() => begin("github"), [begin]);
  const cancelSync = useCallback(async () => {
    if (!cwd) return;
    try {
      await cancelSyncJobs(cwd);
    } catch (error) {
      setActionError(errorText(error));
    }
  }, [cwd]);
  const clearSyncedData = useCallback(async () => {
    if (!cwd) return;
    setActionError(undefined);
    try {
      await clearSynced(cwd);
    } catch (error) {
      setActionError(errorText(error));
      return;
    }
    // Everything this project had in the page is gone with the store.
    const mine = `${cwd}\u0000`;
    for (const k of [...loadCache.keys()]) {
      if (k.startsWith(mine)) loadCache.delete(k);
    }
    for (const k of [...toppedUp]) if (k.startsWith(mine)) toppedUp.delete(k);
    forgetSyncJobs(cwd);
    lastShown.current = null;
    await load(true);
  }, [cwd, load]);

  const saveConfig = useCallback(
    async (next: ContributorsConfig) => {
      const current = metaCache.get(cwd);
      if (!current) return;
      // The file on disk could not be read; writing now would replace it.
      if (current.configError) {
        throw new Error(
          `${CONFIG_PATH} could not be read (${current.configError}). Fix the file in your repo, then reload.`,
        );
      }
      const state = saveState(cwd);
      state.latest = next;
      state.pending += 1;
      state.epoch += 1;
      remember(metaCache, cwd, { ...current, config: next });
      setSaveError(undefined);
      bump();
      const write = state.chain.then(async () => {
        try {
          await writeConfig(cwd, serializeConfig(next));
          const after = metaCache.get(cwd);
          if (after) remember(metaCache, cwd, { ...after, confirmed: next });
          if (state.latest === next) setSaveError(undefined);
        } catch (error) {
          // A later save already carries this change; only the newest reverts.
          if (state.latest === next) {
            const after = metaCache.get(cwd);
            if (after) {
              remember(metaCache, cwd, { ...after, config: after.confirmed });
            }
          }
          setSaveError(errorText(error));
        } finally {
          state.pending -= 1;
          state.epoch += 1;
          bump();
        }
      });
      state.chain = write;
      await write;
    },
    [cwd],
  );

  const config = meta?.config ?? DEFAULT_CONFIG_VALUE;
  const mailmap = meta?.mailmap ?? EMPTY_MAILMAP;
  const model = useContributorsModel({
    raw: shown,
    config,
    mailmap,
    merge,
    period: shownPeriod,
    now,
    user: meta?.user,
  });

  return {
    status,
    error: failed,
    refreshing,
    /** The data on screen is for the previous period or branch while the new one loads. */
    stale: !loaded && !!shown,
    /** What the sync store knows about the selected period. */
    sync: {
      state: !loaded
        ? ("loading" as const)
        : notSynced
          ? ("notSynced" as const)
          : ("synced" as const),
      /** What is left to read; only for `notSynced`. */
      estimate: notSynced && estimate?.key === key ? estimate.value : null,
      /** Why the estimate is missing, when it could not be made. */
      estimateError:
        notSynced && estimate?.key === key ? estimate.error : undefined,
      /** Epoch seconds of the last sync of the period. */
      syncedAt: loaded?.syncedAt ?? null,
      /** The data is shown but GitHub's part is missing or partial. */
      ghIncomplete:
        !!loaded &&
        !notSynced &&
        (loaded.ghState === "partial" || loaded.ghState === "unavailable"),
      /** The queued or running job of this period, or null. */
      job: liveJob,
      jobError,
      actionError,
    },
    startSync: startFullSync,
    cancelSync,
    retryGithub,
    clearSyncedData,
    raw: shown,
    model,
    config,
    saveConfig,
    saveError,
    /** Set while `.monoptah/contributors.json` exists but cannot be read; editing is blocked. */
    configError: meta?.configError,
    refresh,
    branches: meta?.branches.branches ?? [],
    branch: branch ?? shown?.branch ?? meta?.branches.current ?? "",
    setBranch,
    /** The selected period; the model shows it unless `stale`. */
    period,
    setKind,
    goPrevious,
    goNext,
    /** False for 7d and for the current period: nothing lies ahead. */
    hasNext: nextPeriod(period, now) !== null,
    merge,
    setMerge,
  };
}

const EMPTY_MAILMAP: MailmapPerson[] = [];
const DEFAULT_CONFIG_VALUE = parseConfig(null);
