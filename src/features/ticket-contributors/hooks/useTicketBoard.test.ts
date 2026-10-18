import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("../model/ticketsApi", () => ({
  TICKETS_EVENT: "contributors://tickets",
  assessTickets: vi.fn(),
  cancelTicketJob: vi.fn(),
  estimateTickets: vi.fn(),
  fetchTickets: vi.fn(),
  getAiConfig: vi.fn(),
  loadTickets: vi.fn(),
  setTicketOverride: vi.fn(),
  windowCovered: vi.fn(),
  ticketJobStatus: vi.fn(),
}));
vi.mock("../../inbox/model/clickup", () => ({
  CLICKUP_CHANGE_EVENT: "monocode:clickup-change",
  clickupConnected: vi.fn(),
  listClickUpWorkspaces: vi.fn(),
}));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn() }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ save: vi.fn() }));
vi.mock("../../../platform/tauri/fs", () => ({
  revealPath: vi.fn(),
  writeTextFile: vi.fn(),
}));

import { listen } from "@tauri-apps/api/event";
import { save } from "@tauri-apps/plugin-dialog";
import { revealPath, writeTextFile } from "../../../platform/tauri/fs";
import {
  fetchTickets,
  loadTickets,
  ticketJobStatus,
  windowCovered,
} from "../model/ticketsApi";
import { currentPeriod, previousPeriod } from "../../contributors/model/period";
import {
  ALL_START,
  buildBoard,
  periodTickets,
  toAssessCount,
} from "../model/board";
import { DEFAULT_TICKET_POINTS } from "../model/tickets";
import type { TicketRecord, WorkspaceScope } from "../model/tickets";
import type { TicketJobEvent } from "../model/ticketsApi";
import {
  exportReportFile,
  fetchKey,
  jobErrorAfter,
  periodOf,
  scopeFetchId,
  shouldFetch,
  windowOf,
} from "./useTicketBoard";

const NOW = new Date(2026, 9, 17, 12, 0, 0);

const event = (
  state: TicketJobEvent["state"],
  over: Partial<TicketJobEvent> = {},
): TicketJobEvent => ({
  kind: "fetch",
  state,
  done: 1,
  total: 2,
  failed: 1,
  error: null,
  jobId: 1,
  pendingFetches: 0,
  ...over,
});

const scope: WorkspaceScope = {
  workspaceId: "w1",
  name: "Workspace",
  spaceIds: null,
  excludedStatuses: [],
  deliveredStatuses: [],
  excludedMembers: [],
};

const ticket = (over: Partial<TicketRecord> = {}): TicketRecord => ({
  source: "clickup",
  id: "t1",
  title: "Ticket",
  url: "https://app.clickup.com/t/t1",
  workspaceId: "w1",
  spaceId: "s1",
  status: "complete",
  statusType: "closed",
  updatedAt: new Date(2026, 9, 5, 10).toISOString(),
  doneAt: new Date(2026, 9, 5, 10).toISOString(),
  statusSince: null,
  parentId: null,
  hasSubtasks: false,
  assignees: [{ id: "u1", name: "Ana", email: "ana@example.com" }],
  tags: [],
  assessment: null,
  overrideLevel: null,
  overrideNote: null,
  ...over,
});

describe("windowOf", () => {
  it("sends the selected period as ISO seconds", () => {
    const period = currentPeriod("month", NOW);
    expect(windowOf(period)).toEqual({
      from: period.start.toISOString().replace(".000Z", "Z"),
      to: period.end.toISOString().replace(".000Z", "Z"),
    });
  });

  it("starts All time at ALL_START, not at a first commit", () => {
    const period = periodOf({ kind: "all", id: null }, NOW);
    expect(period.kind).toBe("all");
    expect(windowOf(period).from).toBe(ALL_START);
    expect(period.start.toISOString()).toBe("2015-01-01T00:00:00.000Z");
    expect(period.end.getTime()).toBeGreaterThan(NOW.getTime());
  });

  it("builds a non-all period from its pick", () => {
    expect(periodOf({ kind: "month", id: "2026-08" }, NOW).id).toBe("2026-08");
    expect(periodOf({ kind: "month", id: null }, NOW).id).toBe("2026-10");
  });
});

describe("fetchKey and shouldFetch", () => {
  it("keys a fetch by workspace and window", () => {
    expect(fetchKey("w1", "a", "b")).toBe(fetchKey("w1", "a", "b"));
    expect(fetchKey("w1", "a", "b")).not.toBe(fetchKey("w2", "a", "b"));
    expect(fetchKey("w1", "a", "b")).not.toBe(fetchKey("w1", "a", "c"));
  });

  it("fetches a window that was not fetched yet", () => {
    expect(shouldFetch("k", new Set(), null)).toBe(true);
    expect(shouldFetch("k", new Set(["other"]), event("done"))).toBe(true);
  });

  it("skips a window that was fetched in this session", () => {
    expect(shouldFetch("k", new Set(["k"]), null)).toBe(false);
  });

  it("skips while a fetch is queued or running", () => {
    expect(shouldFetch("k", new Set(), event("queued"))).toBe(false);
    expect(shouldFetch("k", new Set(), event("running"))).toBe(false);
  });

  it("does not let a running assessment block a fetch", () => {
    expect(
      shouldFetch("k", new Set(), event("running", { kind: "assess" })),
    ).toBe(true);
  });
});

describe("jobErrorAfter", () => {
  it("keeps the error of a failed job, so a reload cannot hide it", () => {
    expect(
      jobErrorAfter(null, event("error", { error: "ClickUp said no" })),
    ).toBe("ClickUp said no");
    expect(jobErrorAfter(null, event("error"))).toBe("The ticket job failed.");
  });

  it("clears the error when a new job starts or one finishes", () => {
    expect(jobErrorAfter("old", event("queued"))).toBeNull();
    expect(jobErrorAfter("old", event("running"))).toBeNull();
    expect(jobErrorAfter("old", event("done"))).toBeNull();
  });

  it("leaves the error alone on cancel", () => {
    expect(jobErrorAfter("old", event("cancelled"))).toBe("old");
  });
});

describe("loaded tickets", () => {
  it("counts an override-only ticket as assessed", () => {
    const period = currentPeriod("month", NOW);
    const shown = periodTickets(
      [ticket({ id: "o", overrideLevel: "L" }), ticket({ id: "plain" })],
      scope,
      period,
    );
    const board = buildBoard(shown, DEFAULT_TICKET_POINTS);
    expect(board.assessed).toBe(1);
    expect(board.members[0].points).toBe(DEFAULT_TICKET_POINTS.L);
    expect(toAssessCount(shown.length, board.assessed, 100)).toBe(1);
  });
});

describe("scopeFetchId", () => {
  it("ignores the status choices, so changing them never refetches", () => {
    const id = scopeFetchId(scope);
    expect(
      scopeFetchId({
        ...scope,
        excludedStatuses: ["dropped"],
        deliveredStatuses: ["ready for testing"],
      }),
    ).toBe(id);
    expect(scopeFetchId({ ...scope, spaceIds: ["s1"] })).not.toBe(id);
  });
});

describe("exportReportFile", () => {
  const month = currentPeriod("month", NOW);
  const before = previousPeriod(month)!;
  const beforeFrom = windowOf(before).from;
  const input = {
    scope: { ...scope, name: "Acme Co" },
    period: month,
    points: DEFAULT_TICKET_POINTS,
    tickets: [ticket()],
    ai: null,
  };
  // Delivered in the previous month: counts against the current one.
  const previous = (id: string) =>
    ticket({
      id,
      doneAt: new Date(2026, 8, 5, 10).toISOString(),
      updatedAt: new Date(2026, 8, 5, 10).toISOString(),
    });
  let emit: (payload: TicketJobEvent) => void = () => undefined;

  beforeEach(() => {
    vi.mocked(loadTickets).mockReset().mockResolvedValue([]);
    vi.mocked(fetchTickets).mockReset().mockResolvedValue(7);
    vi.mocked(ticketJobStatus)
      .mockReset()
      .mockResolvedValue({ event: null, pendingFetches: 0 });
    vi.mocked(windowCovered).mockReset().mockResolvedValue(false);
    vi.mocked(listen)
      .mockReset()
      .mockImplementation(async (_name, handler) => {
        emit = (payload) =>
          (handler as (e: { payload: TicketJobEvent }) => void)({ payload });
        return () => undefined;
      });
    vi.mocked(save).mockReset();
    vi.mocked(writeTextFile).mockReset().mockResolvedValue(undefined);
    vi.mocked(revealPath).mockReset().mockResolvedValue(undefined);
  });

  const written = () => vi.mocked(writeTextFile).mock.calls[0][1];
  const end = (over: Partial<TicketJobEvent> = {}) =>
    event("done", { failed: 0, jobId: 7, pendingFetches: 0, ...over });

  it("uses the stored coverage and fetches nothing when it reaches back far enough", async () => {
    vi.mocked(save).mockResolvedValue("/tmp/report.html");
    vi.mocked(loadTickets).mockResolvedValue([previous("p1")]);
    vi.mocked(windowCovered).mockResolvedValue(true);
    const path = await exportReportFile(input, { now: NOW });
    expect(path).toBe("/tmp/report.html");
    expect(windowCovered).toHaveBeenCalledWith(
      "w1",
      null,
      beforeFrom,
      windowOf(before).to,
    );
    expect(fetchTickets).not.toHaveBeenCalled();
    expect(loadTickets).toHaveBeenCalledWith(["w1"], beforeFrom);
    expect(save).toHaveBeenCalledWith(
      expect.objectContaining({
        defaultPath: "ticket-contributors-acme-co-2026-10.html",
        filters: [{ name: "HTML", extensions: ["html"] }],
      }),
    );
    expect(written()).toContain("<html");
    expect(written()).toContain("vs ");
    expect(revealPath).toHaveBeenCalledWith("/tmp/report.html");
  });

  it("decides from the backend coverage alone", async () => {
    vi.mocked(save).mockResolvedValue("/tmp/report.html");
    vi.mocked(loadTickets).mockResolvedValue([previous("p1")]);
    // Nothing in the session decides this: only the backend's coverage does.
    await exportReportFile(input, { now: NOW, timeoutMs: 10 });
    expect(windowCovered).toHaveBeenCalled();
    expect(loadTickets).not.toHaveBeenCalled();
    expect(written()).not.toContain("vs ");
  });

  it("asks about the scope's spaces", async () => {
    vi.mocked(save).mockResolvedValue("/tmp/report.html");
    vi.mocked(windowCovered).mockResolvedValue(true);
    await exportReportFile(
      { ...input, scope: { ...input.scope, spaceIds: ["s2", "s1"] } },
      { now: NOW },
    );
    expect(windowCovered).toHaveBeenCalledWith(
      "w1",
      ["s2", "s1"],
      beforeFrom,
      windowOf(before).to,
    );
  });

  it("fetches the previous window when the coverage starts later, waits, then re-reads it", async () => {
    vi.mocked(save).mockResolvedValue("/tmp/report.html");
    vi.mocked(loadTickets).mockResolvedValue([previous("p1")]);
    vi.mocked(windowCovered)
      .mockResolvedValueOnce(false)
      .mockResolvedValue(true);
    vi.mocked(fetchTickets).mockImplementation(async () => {
      setTimeout(() => emit(end()), 5);
      return 7;
    });
    await exportReportFile(input, { now: NOW });
    expect(fetchTickets).toHaveBeenCalledWith(
      [input.scope],
      beforeFrom,
      windowOf(before).to,
    );
    expect(windowCovered).toHaveBeenCalledTimes(2);
    expect(loadTickets).toHaveBeenCalledWith(["w1"], beforeFrom);
    expect(written()).toContain("vs ");
  });

  it("sees the end that arrives before the command's answer", async () => {
    vi.mocked(save).mockResolvedValue("/tmp/report.html");
    vi.mocked(loadTickets).mockResolvedValue([previous("p1")]);
    vi.mocked(windowCovered)
      .mockResolvedValueOnce(false)
      .mockResolvedValue(true);
    vi.mocked(fetchTickets).mockImplementation(async () => {
      emit(end());
      return 7;
    });
    await exportReportFile(input, { now: NOW, timeoutMs: 5000 });
    expect(written()).toContain("vs ");
  });

  it("leaves the comparison out when the coverage is still short after the fetch", async () => {
    vi.mocked(save).mockResolvedValue("/tmp/report.html");
    vi.mocked(loadTickets).mockResolvedValue([previous("p1")]);
    vi.mocked(windowCovered).mockResolvedValue(false);
    vi.mocked(fetchTickets).mockImplementation(async () => {
      setTimeout(() => emit(end({ state: "error" })), 0);
      return 7;
    });
    await exportReportFile(input, { now: NOW });
    expect(loadTickets).not.toHaveBeenCalled();
    expect(writeTextFile).toHaveBeenCalledTimes(1);
    expect(written()).not.toContain("vs ");
  });

  it("leaves the comparison out when the wait times out", async () => {
    vi.mocked(save).mockResolvedValue("/tmp/report.html");
    await exportReportFile(input, { now: NOW, timeoutMs: 20 });
    expect(fetchTickets).toHaveBeenCalledTimes(1);
    expect(loadTickets).not.toHaveBeenCalled();
    expect(written()).not.toContain("vs ");
  });

  it("does not wait for an earlier job's end, or one that leaves fetches pending", async () => {
    vi.mocked(save).mockResolvedValue("/tmp/report.html");
    vi.mocked(windowCovered).mockResolvedValue(null);
    vi.mocked(fetchTickets).mockImplementation(async () => {
      // An earlier job (id 3) ending, and ours ending with another still pending.
      setTimeout(() => emit(end({ jobId: 3 })), 0);
      setTimeout(() => emit(end({ jobId: 7, pendingFetches: 1 })), 0);
      return 7;
    });
    const started = Date.now();
    await exportReportFile(input, { now: NOW, timeoutMs: 60 });
    expect(Date.now() - started).toBeGreaterThanOrEqual(50);
  });

  it("waits for a fetch queued behind its own", async () => {
    vi.mocked(save).mockResolvedValue("/tmp/report.html");
    vi.mocked(windowCovered)
      .mockResolvedValueOnce(false)
      .mockResolvedValue(true);
    vi.mocked(loadTickets).mockResolvedValue([previous("p1")]);
    vi.mocked(fetchTickets).mockImplementation(async () => {
      setTimeout(() => emit(end({ jobId: 7, pendingFetches: 1 })), 0);
      setTimeout(() => emit(end({ jobId: 8, pendingFetches: 0 })), 30);
      return 7;
    });
    const started = Date.now();
    await exportReportFile(input, { now: NOW, timeoutMs: 5000 });
    expect(Date.now() - started).toBeGreaterThanOrEqual(25);
    expect(written()).toContain("vs ");
  });

  it("queues no fetch while an assessment is live, and says why there is no comparison", async () => {
    vi.mocked(save).mockResolvedValue("/tmp/report.html");
    vi.mocked(loadTickets).mockResolvedValue([previous("p1")]);
    vi.mocked(ticketJobStatus).mockResolvedValue({
      event: event("running", { kind: "assess" }),
      pendingFetches: 0,
    });
    const started = Date.now();
    await exportReportFile(input, { now: NOW });
    expect(Date.now() - started).toBeLessThan(1000);
    expect(fetchTickets).not.toHaveBeenCalled();
    expect(loadTickets).not.toHaveBeenCalled();
    expect(written()).not.toContain("vs ");
    expect(written()).toContain("No comparison with Sep 2026");
  });

  it("shows the comparison and no note when the previous window is covered", async () => {
    vi.mocked(save).mockResolvedValue("/tmp/report.html");
    vi.mocked(loadTickets).mockResolvedValue([previous("p1")]);
    vi.mocked(windowCovered).mockResolvedValue(true);
    // Even with an assessment live: nothing needs fetching.
    vi.mocked(ticketJobStatus).mockResolvedValue({
      event: event("running", { kind: "assess" }),
      pendingFetches: 0,
    });
    await exportReportFile(input, { now: NOW });
    expect(written()).toContain("vs ");
    expect(written()).not.toContain("No comparison with");
  });

  it("notes the missing comparison whenever the previous window stays uncovered", async () => {
    vi.mocked(save).mockResolvedValue("/tmp/report.html");
    await exportReportFile(input, { now: NOW, pendingFetches: () => 1 });
    expect(written()).toContain("No comparison with Sep 2026");
  });

  it("queues nothing while a fetch is pending", async () => {
    vi.mocked(save).mockResolvedValue("/tmp/report.html");
    await exportReportFile(input, { now: NOW, pendingFetches: () => 1 });
    expect(fetchTickets).not.toHaveBeenCalled();
    expect(loadTickets).not.toHaveBeenCalled();
    expect(written()).not.toContain("vs ");
  });

  it("does not queue when a fetch appeared while it was listening", async () => {
    vi.mocked(save).mockResolvedValue("/tmp/report.html");
    let pending = 0;
    vi.mocked(listen).mockImplementationOnce(async () => {
      pending = 1;
      return () => undefined;
    });
    await exportReportFile(input, { now: NOW, pendingFetches: () => pending });
    expect(fetchTickets).not.toHaveBeenCalled();
    expect(written()).not.toContain("vs ");
  });

  it("skips the previous period for All time", async () => {
    vi.mocked(save).mockResolvedValue("/tmp/all.html");
    await exportReportFile(
      { ...input, period: periodOf({ kind: "all", id: null }, NOW) },
      { now: NOW },
    );
    expect(windowCovered).not.toHaveBeenCalled();
    expect(fetchTickets).not.toHaveBeenCalled();
    expect(loadTickets).not.toHaveBeenCalled();
    expect(writeTextFile).toHaveBeenCalledTimes(1);
  });

  it("treats a failing coverage read as not covered", async () => {
    vi.mocked(save).mockResolvedValue("/tmp/report.html");
    vi.mocked(windowCovered).mockRejectedValue(new Error("db busy"));
    await exportReportFile(input, { now: NOW, timeoutMs: 10 });
    expect(loadTickets).not.toHaveBeenCalled();
    expect(written()).not.toContain("vs ");
  });

  it("writes nothing and returns null when the dialog is cancelled", async () => {
    vi.mocked(save).mockResolvedValue(null);
    vi.mocked(windowCovered).mockResolvedValue(true);
    expect(await exportReportFile(input, { now: NOW })).toBeNull();
    expect(writeTextFile).not.toHaveBeenCalled();
    expect(revealPath).not.toHaveBeenCalled();
  });

  it("rejects when the file cannot be written", async () => {
    vi.mocked(save).mockResolvedValue("/tmp/report.html");
    vi.mocked(windowCovered).mockResolvedValue(true);
    vi.mocked(writeTextFile).mockRejectedValue(new Error("disk full"));
    await expect(exportReportFile(input, { now: NOW })).rejects.toThrow(
      "disk full",
    );
    expect(revealPath).not.toHaveBeenCalled();
  });

  it("still succeeds when revealing the file fails", async () => {
    vi.mocked(save).mockResolvedValue("/tmp/report.html");
    vi.mocked(windowCovered).mockResolvedValue(true);
    vi.mocked(revealPath).mockRejectedValue(new Error("no finder"));
    expect(await exportReportFile(input, { now: NOW })).toBe(
      "/tmp/report.html",
    );
  });
});
