import {
  useCallback,
  useEffect,
  useMemo,
  useReducer,
  useRef,
  useState,
} from "react";
import {
  collectRaw,
  gitUser,
  listBranches,
  readConfig,
  readMailmap,
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
import type { ContributorsConfig, RawData } from "../model/types";

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
const rawCache = new Map<string, RawData>();
const inFlight = new Map<string, Promise<RawData>>();
const saves = new Map<string, SaveState>();
/** One read of a project's files at a time, so a remount does not repeat it. */
const metaInFlight = new Map<
  string,
  { epoch: number; request: Promise<Meta> }
>();
const forcedAt = new Map<string, number>();
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
  rawCache.clear();
  inFlight.clear();
  metaInFlight.clear();
  saves.clear();
  forcedAt.clear();
}

/** Test hook: how many entries the caches hold. */
export function contributorsCacheSize() {
  return { raw: rawCache.size, meta: metaCache.size };
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

function loadRaw(
  cwd: string,
  branch: string | undefined,
  since: string,
  key: string,
  force: boolean,
): Promise<RawData> {
  const joined =
    inFlight.get(`${key}|force`) ?? (force ? undefined : inFlight.get(key));
  if (joined) return joined;
  const slot = force ? `${key}|force` : key;
  const seq = (requestSeq += 1);
  if (force) forcedAt.set(key, seq);
  const request = collectRaw(cwd, { since, branch, force })
    .then((fresh) => {
      const previous = rawCache.get(key);
      // A forced refresh started after this answer was asked for: its data is
      // newer (gh may have changed), so this one must not replace it.
      if (!force && (forcedAt.get(key) ?? 0) > seq) {
        if (!previous) remember(rawCache, key, fresh);
        return rawCache.get(key) ?? fresh;
      }
      // Same HEAD and same sizes: keep the object so a focus revalidation does
      // not rescore. A forced refresh always takes the new data.
      const raw =
        !force && previous && sameData(previous, fresh) ? previous : fresh;
      remember(rawCache, key, raw);
      return raw;
    })
    .finally(() => {
      if (inFlight.get(slot) === request) inFlight.delete(slot);
    });
  inFlight.set(slot, request);
  return request;
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
    a.reviews.length === b.reviews.length
  );
}

const sameJson = (a: unknown, b: unknown) =>
  JSON.stringify(a) === JSON.stringify(b);

/** Whole seconds, so the cache key does not depend on milliseconds. */
const isoSeconds = (date: Date) => date.toISOString().replace(/\.\d{3}Z$/, "Z");

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
  // Caches live outside React; this re-renders after they change.
  const [, bump] = useReducer((n: number) => n + 1, 0);

  const branch = picked.cwd === cwd ? picked.branch : undefined;
  // A pick without an id follows the current period as the days pass.
  const period = useMemo(() => resolvePick(pick, now), [pick, now]);
  // The end stays open: survival only samples PRs old enough relative to the
  // end, and a PR merged after a past period still credits that period's days.
  const since = useMemo(
    () => isoSeconds(syncWindow(period, now).from),
    [period, now],
  );
  const key = `${cwd}\u0000${branch ?? ""}\u0000${since}`;
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
    async (force: boolean) => {
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
      const raw = loadRaw(cwd, branch, since, key, force).then(
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
    [cwd, branch, since, key, metaScope, fail, clearFailure],
  );

  useEffect(() => {
    void load(false);
  }, [load]);

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
  const current = recall(rawCache, key);
  // While a new period or branch loads, keep the last data of this project on
  // screen (marked stale) instead of blanking the page.
  const previouslyShown =
    lastShown.current?.cwd === cwd ? lastShown.current : undefined;
  const shown = current ?? previouslyShown?.raw;
  // The old data may not reach the newly picked period, so the period shown
  // stays with the data until the new request answers.
  const shownPeriod = current ? period : (previouslyShown?.period ?? period);
  if (shown) lastShown.current = { cwd, raw: shown, period: shownPeriod };
  const failed = failures[key] ?? failures[metaScope];
  const status: "loading" | "ready" | "error" =
    meta && shown ? "ready" : failed ? "error" : "loading";

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
    stale: !current && !!shown,
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
