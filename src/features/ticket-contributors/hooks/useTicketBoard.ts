import { listen } from "@tauri-apps/api/event";
import { save } from "@tauri-apps/plugin-dialog";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  CLICKUP_CHANGE_EVENT,
  clickupConnected,
  listClickUpWorkspaces,
} from "../../inbox/model/clickup";
import {
  currentPeriod,
  nextPeriod,
  pickOf,
  previousPeriod,
  resolvePick,
} from "../../contributors/model/period";
import { revealPath, writeTextFile } from "../../../platform/tauri/fs";
import type { Period, PeriodPick } from "../../contributors/model/period";
import {
  ALL_START,
  activeTickets,
  buildBoard,
  excludedMembersOf,
  periodTickets,
  pickForRun,
  statusesOf,
  toAssessCount,
} from "../model/board";
import {
  readRange,
  readSettings,
  writeRange,
  writeSettings,
} from "../model/prefs";
import type { TicketBoardSettings } from "../model/prefs";
import { buildReport, renderReportHtml, reportFileName } from "../model/report";
import type { Level, TicketRecord, WorkspaceScope } from "../model/tickets";
import {
  TICKETS_EVENT,
  assessTickets,
  cancelTicketJob,
  estimateTickets,
  fetchTickets,
  getAiConfig,
  loadTickets,
  setTicketOverride,
  ticketJobStatus,
  windowCovered,
} from "../model/ticketsApi";
import type { AiConfigStatus, TicketJobEvent } from "../model/ticketsApi";

const NO_TICKETS: TicketRecord[] = [];
const ESTIMATE_DEBOUNCE_MS = 300;

/** Windows fetched from ClickUp in this app session; they outlive the view. */
const fetchedKeys = new Set<string>();

const errorText = (error: unknown) =>
  error instanceof Error ? error.message : String(error);

const isoSeconds = (date: Date) => date.toISOString().replace(/\.\d{3}Z$/, "Z");

/** The period a pick stands for; All time starts at `ALL_START`, not at a first commit. */
export function periodOf(pick: PeriodPick, now: Date): Period {
  if (pick.kind === "all") {
    return { ...currentPeriod("all", now, null), start: new Date(ALL_START) };
  }
  return resolvePick(pick, now);
}

/** The `from`..`to` strings the Rust commands read for a period. */
export function windowOf(period: Period): { from: string; to: string } {
  return {
    from: period.kind === "all" ? ALL_START : isoSeconds(period.start),
    to: isoSeconds(period.end),
  };
}

export const fetchKey = (workspaceId: string, from: string, to: string) =>
  `${workspaceId}\u0001${from}\u0001${to}`;

const fetchBusy = (job: TicketJobEvent | null) =>
  job !== null &&
  job.kind === "fetch" &&
  (job.state === "queued" || job.state === "running");

/** A window is fetched once per session, and never while another fetch is queued or running. */
export function shouldFetch(
  key: string,
  fetched: ReadonlySet<string>,
  job: TicketJobEvent | null,
): boolean {
  return !fetched.has(key) && !fetchBusy(job);
}

/** A job event that means the store may have changed. */
const ended = (event: TicketJobEvent) =>
  event.state === "done" ||
  event.state === "cancelled" ||
  // A failed fetch of one workspace still stored the others.
  event.state === "error";

/** A job event that ends a job: done, failed or dropped. */
const isEnd = (event: TicketJobEvent) =>
  event.state !== "queued" && event.state !== "running";

/**
 * The job error after `event`: a failed job sets it, and a new job or a
 * finished one clears it. A cancelled job leaves it, since it says nothing
 * about the failure that came before.
 */
export function jobErrorAfter(
  current: string | null,
  event: TicketJobEvent,
): string | null {
  if (event.state === "error") return event.error ?? "The ticket job failed.";
  if (event.state === "cancelled") return current;
  return null;
}

export type ExportReportInput = {
  scope: WorkspaceScope;
  period: Period;
  points: Record<Level, number>;
  tickets: TicketRecord[];
  ai: AiConfigStatus | null;
};

export type ExportReportDeps = {
  now?: Date;
  /** How long to wait for the previous window's fetch before giving up. */
  timeoutMs?: number;
  /** Fetch jobs queued or running right now, as the backend last reported. */
  pendingFetches?: () => number;
  /** Whether a job that is not a fetch (an assessment) is queued or running. */
  otherJobLive?: () => Promise<boolean>;
};

/**
 * An assessment holds the one job queue, so a fetch queued now would wait for
 * the whole run. Read from the backend; with no fetch pending, whatever job is
 * live is not a fetch.
 */
async function assessLive(): Promise<boolean> {
  try {
    const status = await ticketJobStatus();
    return !!status?.event && status.pendingFetches === 0;
  } catch {
    return false;
  }
}

const PREVIOUS_FETCH_TIMEOUT_MS = 120_000;

/** Whether the store is complete for a window of the scope, as the backend tracks it. */
async function coversWindow(scope: WorkspaceScope, from: string, to: string) {
  try {
    return await windowCovered(scope.workspaceId, scope.spaceIds, from, to);
  } catch {
    return false;
  }
}

/**
 * Queues a fetch of a window and waits until the fetch jobs have drained: the
 * first fetch-kind end of this job or a later one that leaves none pending (a
 * fetch queued behind ours keeps us waiting for it too). Gives up after
 * `timeoutMs`. Resolves without a verdict; the caller reads the coverage. It
 * does nothing when a fetch is already pending, since a fetch queued ahead
 * would make the first end ambiguous. The listener is registered before the
 * fetch is queued and the pending count is read again just before, so no end
 * is missed and no fetch slips in ahead.
 */
async function fetchAndWait(
  scope: WorkspaceScope,
  from: string,
  to: string,
  timeoutMs: number,
  pendingFetches: () => number,
): Promise<void> {
  let wake: () => void = () => undefined;
  const woken = new Promise<void>((resolve) => {
    wake = resolve;
  });
  const timer = setTimeout(() => wake(), timeoutMs);
  // Ends seen before the job id is known: the event can beat the command's answer.
  const drained: number[] = [];
  let ourId: number | null = null;
  const check = () => {
    if (ourId !== null && drained.some((id) => id >= ourId!)) wake();
  };
  let unlisten: (() => void) | undefined;
  try {
    unlisten = await listen<TicketJobEvent>(TICKETS_EVENT, ({ payload }) => {
      if (payload.kind !== "fetch" || !isEnd(payload)) return;
      if (payload.pendingFetches === 0) {
        drained.push(payload.jobId);
        check();
      }
    });
    if (pendingFetches() > 0) return;
    ourId = await fetchTickets([scope], from, to);
    check();
    await woken;
  } catch {
    // The caller reads the coverage, which says what the fetch achieved.
  } finally {
    clearTimeout(timer);
    unlisten?.();
  }
}

/**
 * Builds the HTML report for a period, asks where to save it, writes it and
 * reveals it. The previous period counts as complete when the backend's
 * completed fetches cover its whole span for the scope's spaces (the store
 * would otherwise hold only some of its tickets and the comparison would be
 * inflated). When it does not, and no fetch is pending, the window is fetched
 * and waited for; if the coverage still falls short, the report is built
 * without previous tickets so the comparison is left out. All time has no
 * previous period. Returns the saved path, or null when the save dialog was
 * cancelled.
 */
export async function exportReportFile(
  input: ExportReportInput,
  deps: ExportReportDeps = {},
): Promise<string | null> {
  const { scope, period } = input;
  const now = deps.now ?? new Date();
  const before = previousPeriod(period);
  let previousTickets = NO_TICKETS;
  let previousIncomplete = false;
  if (before) {
    const { from, to } = windowOf(before);
    let complete = await coversWindow(scope, from, to);
    // No fetch is queued behind a live assessment: export at once, without the comparison.
    if (!complete && !(await (deps.otherJobLive ?? assessLive)())) {
      await fetchAndWait(
        scope,
        from,
        to,
        deps.timeoutMs ?? PREVIOUS_FETCH_TIMEOUT_MS,
        deps.pendingFetches ?? (() => 0),
      );
      complete = await coversWindow(scope, from, to);
    }
    if (complete) {
      previousTickets = await loadTickets([scope.workspaceId], from);
    } else {
      previousIncomplete = true;
    }
  }
  const html = renderReportHtml(
    buildReport({
      workspaceName: scope.name || scope.workspaceId,
      period,
      previousPeriod: before,
      scope,
      points: input.points,
      tickets: input.tickets,
      previousTickets,
      ai: input.ai,
      generatedAt: now,
      previousIncomplete,
    }),
  );
  const path = await save({
    title: "Export report",
    defaultPath: reportFileName(scope.name || scope.workspaceId, period),
    filters: [{ name: "HTML", extensions: ["html"] }],
  });
  if (!path) return null;
  await writeTextFile(path, html);
  // The file is saved already; failing to reveal it must not report a failed export.
  await revealPath(path).catch(() => undefined);
  return path;
}

/** The workspace plus its chosen spaces: what a fetch actually covers. */
export const scopeFetchId = (scope: WorkspaceScope) =>
  scope.spaceIds === null
    ? scope.workspaceId
    : `${scope.workspaceId}:${[...scope.spaceIds].sort().join(",")}`;

/**
 * State of the Ticket Contributors view: the ClickUp connection, the
 * workspace scope and period, the stored tickets (kept fresh as the Rust
 * ticket jobs progress), the board built from them and the scoring actions.
 */
export function useTicketBoard() {
  const [now] = useState(() => new Date());
  const [pick, setPick] = useState<PeriodPick>(() => readRange());
  const [settings, setSettings] = useState<TicketBoardSettings>(() =>
    readSettings(),
  );
  const [connected, setConnected] = useState<boolean | null>(null);
  const [workspaces, setWorkspaces] = useState<{ id: string; name: string }[]>(
    [],
  );
  const [loaded, setLoaded] = useState<{
    key: string;
    tickets: TicketRecord[];
  } | null>(null);
  const [job, setJob] = useState<TicketJobEvent | null>(null);
  const [ai, setAi] = useState<AiConfigStatus | null>(null);
  const [estimate, setEstimate] = useState<{
    tickets: number;
    inputTokens: number;
  } | null>(null);
  // Kept apart so a successful reload does not hide a failed job.
  const [loadError, setLoadError] = useState<string | null>(null);
  const [jobError, setJobError] = useState<string | null>(null);
  const [connectError, setConnectError] = useState<string | null>(null);
  const [workspacesState, setWorkspacesState] = useState<
    "loading" | "ready" | "error"
  >("loading");
  const [workspacesError, setWorkspacesError] = useState<string | null>(null);
  const [attempt, setAttempt] = useState(0);
  // Fetch jobs queued or running, as the backend reports them.
  const [pendingFetches, setPendingFetches] = useState(0);

  const scope = settings.scope;
  const workspaceId = scope?.workspaceId ?? "";
  const fetchId = scope ? scopeFetchId(scope) : "";
  const period = useMemo(() => periodOf(pick, now), [pick, now]);
  const { from, to } = useMemo(() => windowOf(period), [period]);
  const windowKey = `${workspaceId}\u0001${from}\u0001${to}`;

  const latest = useRef({ workspaceId, from, to, windowKey, scope, fetchId });
  latest.current = { workspaceId, from, to, windowKey, scope, fetchId };
  const request = useRef(0);
  const jobRef = useRef<TicketJobEvent | null>(null);
  // Windows with a fetch of ours queued or running; the job id -> window of
  // each; and jobs that ended before the command's answer named them.
  const pendingKeys = useRef(new Set<string>());
  const jobKeys = useRef(new Map<number, string>());
  const endedJobs = useRef(new Map<number, boolean>());
  // A window whose fetch job failed: not retried on its own until the
  // connection changes or another window was shown.
  const failed = useRef<{ key: string; connected: boolean | null } | null>(
    null,
  );
  jobRef.current = job;
  const connectedRef = useRef<boolean | null>(null);
  connectedRef.current = connected;

  const pendingRef = useRef(0);
  // Bumped on every change, so a status read that began earlier can tell it is stale.
  const pendingSeq = useRef(0);
  const setPending = useCallback((count: number) => {
    pendingRef.current = count;
    pendingSeq.current += 1;
    setPendingFetches(count);
  }, []);

  /** A window's fetch ended: only a clean `done` makes it fetched. */
  const settleWindow = useCallback((key: string, clean: boolean) => {
    pendingKeys.current.delete(key);
    if (clean) fetchedKeys.add(key);
    else failed.current = { key, connected: connectedRef.current };
  }, []);

  /** Queues the fetch of a window and follows it until its job ends. */
  const queueWindow = useCallback(
    async (key: string, scope: WorkspaceScope, from: string, to: string) => {
      pendingKeys.current.add(key);
      let id: number;
      try {
        id = await fetchTickets([scope], from, to);
      } catch (error) {
        pendingKeys.current.delete(key);
        throw error;
      }
      const clean = endedJobs.current.get(id);
      if (clean === undefined) {
        jobKeys.current.set(id, key);
      } else {
        endedJobs.current.delete(id);
        settleWindow(key, clean);
      }
    },
    [settleWindow],
  );

  const reload = useCallback(async () => {
    const { workspaceId: id, from, windowKey } = latest.current;
    const seq = (request.current += 1);
    if (!id) {
      setLoaded({ key: windowKey, tickets: NO_TICKETS });
      setLoadError(null);
      return;
    }
    try {
      const tickets = await loadTickets([id], from);
      // A newer read (or a new window) decides what is shown.
      if (seq !== request.current) return;
      setLoaded({ key: windowKey, tickets });
      setLoadError(null);
    } catch (error) {
      if (seq === request.current) setLoadError(errorText(error));
    }
  }, []);

  // The stored tickets show at once; the fetch below tops them up.
  useEffect(() => {
    void reload();
  }, [windowKey, reload]);

  const startFetch = useCallback(
    (key: string) => {
      const { scope, from, to } = latest.current;
      if (!scope) return Promise.resolve();
      return queueWindow(key, scope, from, to).catch((error) => {
        setJobError(errorText(error));
      });
    },
    [queueWindow],
  );

  const busy = fetchBusy(job);
  useEffect(() => {
    // Without a connection the job would only fail; connecting re-runs this.
    if (connected !== true) failed.current = null;
    if (!latest.current.scope || connected !== true) return;
    const key = fetchKey(fetchId, from, to);
    if (failed.current && failed.current.key !== key) failed.current = null;
    if (failed.current?.connected === connected) return;
    // Re-runs when a running fetch ends, so a window skipped meanwhile is fetched.
    if (
      shouldFetch(key, fetchedKeys, jobRef.current) &&
      !pendingKeys.current.has(key)
    ) {
      setJobError(null);
      void startFetch(key);
    }
  }, [fetchId, from, to, busy, connected, startFetch]);

  useEffect(() => {
    let stale = false;
    let stop: (() => void) | undefined;
    const onEvent = (event: TicketJobEvent) => {
      setJob(event);
      // Progress events are sent without the queue lock and can be stale;
      // queuing and ending are exact.
      if (event.state !== "running") setPending(event.pendingFetches);
      setJobError((current) => jobErrorAfter(current, event));
      if (event.kind === "fetch" && isEnd(event)) {
        // Only a clean done makes the window fetched; a cancel emits one end
        // per dropped job, each naming its own.
        const clean = event.state === "done" && event.failed === 0;
        const key = jobKeys.current.get(event.jobId);
        if (key !== undefined) {
          jobKeys.current.delete(event.jobId);
          settleWindow(key, clean);
        } else {
          // The command's answer may still be on its way.
          endedJobs.current.set(event.jobId, clean);
          if (endedJobs.current.size > 50) {
            endedJobs.current.delete(endedJobs.current.keys().next().value!);
          }
        }
      }
      if (ended(event)) void reload();
    };
    // Registered before the status read, so an event cannot slip between them.
    let seq = 0;
    Promise.resolve(
      listen<TicketJobEvent>(TICKETS_EVENT, (e) => onEvent(e.payload)),
    )
      .then((unlisten) => {
        if (stale) unlisten?.();
        else stop = unlisten;
        seq = pendingSeq.current;
        return ticketJobStatus();
      })
      .then((status) => {
        if (stale || !status) return;
        // A live job from before this mount; an event that already arrived is newer.
        if (status.event) {
          setJob((current) => current ?? status.event);
        }
        if (pendingSeq.current === seq) setPending(status.pendingFetches);
      })
      .catch(() => undefined);
    return () => {
      stale = true;
      stop?.();
    };
  }, [reload, setPending, settleWindow]);

  // ClickUp connection and workspaces; they change when the token does.
  useEffect(() => {
    let stale = false;
    const check = async () => {
      let status: { connected: boolean };
      try {
        status = await clickupConnected();
      } catch (error) {
        if (!stale) setConnectError(errorText(error));
        return;
      }
      if (stale) return;
      setConnected(status.connected);
      setConnectError(null);
      if (!status.connected) {
        setWorkspaces([]);
        setWorkspacesState("ready");
        setWorkspacesError(null);
        return;
      }
      // A refresh of a list already shown does not blank the view.
      setWorkspacesState((state) => (state === "ready" ? state : "loading"));
      try {
        const list = await listClickUpWorkspaces();
        if (stale) return;
        setWorkspaces(list.map((w) => ({ id: w.id, name: w.name })));
        setWorkspacesState("ready");
        setWorkspacesError(null);
      } catch (error) {
        if (stale) return;
        setWorkspacesState("error");
        setWorkspacesError(errorText(error));
      }
    };
    void check();
    const onChange = () => void check();
    window.addEventListener(CLICKUP_CHANGE_EVENT, onChange);
    return () => {
      stale = true;
      window.removeEventListener(CLICKUP_CHANGE_EVENT, onChange);
    };
  }, [attempt]);

  /** Tries the connection check and the workspace list again after a failure. */
  const retry = useCallback(() => {
    setConnected(null);
    setConnectError(null);
    setWorkspacesState("loading");
    setWorkspacesError(null);
    setAttempt((n) => n + 1);
  }, []);

  useEffect(() => {
    let stale = false;
    getAiConfig()
      .then((config) => {
        if (!stale) setAi(config);
      })
      .catch(() => undefined);
    return () => {
      stale = true;
    };
  }, []);

  // Tickets of another window or workspace are not shown under this one.
  const tickets = loaded?.key === windowKey ? loaded.tickets : NO_TICKETS;

  const shown = useMemo(
    () => (scope ? periodTickets(tickets, scope, period) : NO_TICKETS),
    [tickets, scope, period],
  );
  // Both are classified here, so changing the delivered statuses needs no refetch.
  const active = useMemo(
    () => (scope ? activeTickets(tickets, scope, period) : NO_TICKETS),
    [tickets, scope, period],
  );
  const board = useMemo(
    () => buildBoard(shown, settings.points, active),
    [shown, settings.points, active],
  );
  // Every status of the loaded tickets, so a delivered or excluded one can be picked.
  const statuses = useMemo(() => statusesOf(tickets), [tickets]);
  const toAssess = toAssessCount(
    shown.length,
    board.assessed,
    settings.percent,
  );
  const runIds = useMemo(() => pickForRun(shown, toAssess), [shown, toAssess]);
  const runKey = runIds.join("\u0000");

  useEffect(() => {
    if (runKey === "") {
      setEstimate(null);
      return;
    }
    let stale = false;
    const timer = setTimeout(() => {
      estimateTickets(runKey.split("\u0000"))
        .then((e) => {
          if (!stale) setEstimate(e);
        })
        .catch(() => {
          if (!stale) setEstimate(null);
        });
    }, ESTIMATE_DEBOUNCE_MS);
    return () => {
      stale = true;
      clearTimeout(timer);
    };
  }, [runKey]);

  const update = useCallback((patch: Partial<TicketBoardSettings>) => {
    // Re-read first, so points edited in Settings are not overwritten.
    const next = { ...readSettings(), ...patch };
    writeSettings(next);
    setSettings(next);
  }, []);
  const setScope = useCallback(
    (next: WorkspaceScope | null) => update({ scope: next }),
    [update],
  );
  // An excluded member's name follows ClickUp: save a newer name a loaded ticket carries.
  useEffect(() => {
    if (!scope) return;
    const refreshed = excludedMembersOf(tickets, scope);
    if (refreshed !== scope.excludedMembers) {
      setScope({ ...scope, excludedMembers: refreshed });
    }
  }, [tickets, scope, setScope]);
  const setPercent = useCallback(
    (p: number) =>
      update({
        percent: Number.isFinite(p)
          ? Math.min(100, Math.max(0, Math.round(p)))
          : 0,
      }),
    [update],
  );

  const choose = useCallback((next: PeriodPick) => {
    writeRange(next);
    setPick(next);
  }, []);
  const setKind = useCallback(
    (kind: "month" | "quarter" | "year" | "all") => choose({ kind, id: null }),
    [choose],
  );
  const goPrevious = useCallback(() => {
    const before = previousPeriod(period);
    if (before) choose(pickOf(before, now));
  }, [period, now, choose]);
  const goNext = useCallback(() => {
    const after = nextPeriod(period, now);
    if (after) choose(pickOf(after, now));
  }, [period, now, choose]);
  const hasNext = useMemo(
    () => nextPeriod(period, now) !== null,
    [period, now],
  );

  const run = useCallback(async (action: () => Promise<void>) => {
    try {
      setJobError(null);
      await action();
    } catch (error) {
      setJobError(errorText(error));
    }
  }, []);

  const refresh = useCallback(
    () =>
      run(async () => {
        const { scope, fetchId, from, to } = latest.current;
        if (!scope) return;
        const key = fetchKey(fetchId, from, to);
        // Already queued or running for this window: nothing to add.
        if (pendingKeys.current.has(key)) return;
        failed.current = null;
        await queueWindow(key, scope, from, to);
      }),
    [run, queueWindow],
  );
  const exportReport = useCallback(async (): Promise<string | null> => {
    if (!scope) return null;
    return exportReportFile(
      {
        scope,
        period,
        points: settings.points,
        tickets,
        ai,
      },
      { pendingFetches: () => pendingRef.current },
    );
  }, [scope, period, settings.points, tickets, ai]);
  const score = useCallback(async () => {
    if (runIds.length === 0) return;
    await run(() => assessTickets(runIds, false));
  }, [run, runIds]);
  const cancel = useCallback(
    () =>
      run(async () => {
        // The cancel drops queued jobs too; each one's end event settles its window.
        await cancelTicketJob();
      }),
    [run],
  );
  const override = useCallback(
    (ticketId: string, level: Level | null, note: string | null) =>
      run(async () => {
        await setTicketOverride(ticketId, level, note);
        await reload();
      }),
    [run, reload],
  );
  const reassess = useCallback(
    (ids: string[]) => run(() => assessTickets(ids, true)),
    [run],
  );

  return {
    connected,
    workspaces,
    settings,
    setScope,
    setPercent,
    period,
    setKind,
    goPrevious,
    goNext,
    hasNext,
    tickets,
    board,
    statuses,
    toAssess,
    runIds,
    estimate,
    ai,
    job,
    workspacesState,
    workspacesError,
    connectError,
    retry,
    error: loadError ?? jobError,
    refresh,
    pendingFetches,
    exportReport,
    score,
    cancel,
    override,
    reassess,
  };
}
