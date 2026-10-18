// @vitest-environment happy-dom
import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("../../../app/shell/WindowControls", () => ({
  WindowControls: () => null,
}));
vi.mock("../hooks/useTicketBoard", () => ({ useTicketBoard: vi.fn() }));
vi.mock("../model/ticketsApi", () => ({
  listWorkspaceSpaces: vi.fn(),
}));

import { currentPeriod } from "../../contributors/model/period";
import { useTicketBoard } from "../hooks/useTicketBoard";
import { buildBoard } from "../model/board";
import type { Board } from "../model/board";
import { DEFAULT_TICKET_POINTS } from "../model/tickets";
import type {
  ExcludedMember,
  TicketAssessment,
  TicketAssignee,
  TicketRecord,
  WorkspaceScope,
} from "../model/tickets";
import { listWorkspaceSpaces } from "../model/ticketsApi";
import type { TicketJobEvent } from "../model/ticketsApi";
import { TicketContributorsView } from "./TicketContributorsView";

type Model = ReturnType<typeof useTicketBoard>;

let container: HTMLDivElement;
let root: Root;

beforeEach(() => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  vi.stubGlobal("requestAnimationFrame", (cb: (t: number) => void) => {
    cb(0);
    return 1;
  });
  vi.stubGlobal("cancelAnimationFrame", () => {});
  container = document.createElement("div");
  document.body.append(container);
  root = createRoot(container);
  vi.mocked(listWorkspaceSpaces).mockReset();
  vi.mocked(listWorkspaceSpaces).mockResolvedValue([
    { id: "s1", name: "Engineering" },
    { id: "s2", name: "Support" },
  ]);
});
afterEach(async () => {
  await act(async () => root.unmount());
  container.remove();
  vi.unstubAllGlobals();
});

const NOW = new Date(2026, 9, 17, 12, 0, 0);

const user = (id: string, name = `User ${id}`): TicketAssignee => ({
  id,
  name,
  email: `${id}@example.com`,
});

const assessment = (score: number): TicketAssessment => ({
  criteria: {
    // The criteria sum picks the level: 2 is S, 6 is L.
    ambiguity: score,
    investigation: 0,
    scope: 0,
    risk: 0,
    coordination: 0,
  },
  reasons: {
    ambiguity: "Unclear brief.",
    investigation: "Some digging.",
    scope: "Two modules.",
    risk: "Low.",
    coordination: "Solo.",
  },
  rubricVersion: 1,
  model: "sonnet",
  runner: "claude-code",
  assessedAt: "2026-10-02T00:00:00Z",
});

function ticket(
  id: string,
  over: Partial<TicketRecord> & { score?: number } = {},
): TicketRecord {
  const { score, ...rest } = over;
  return {
    source: "clickup",
    id,
    title: `Ticket ${id}`,
    url: `https://app.clickup.com/t/${id}`,
    workspaceId: "w1",
    spaceId: "s1",
    status: "closed",
    statusType: "closed",
    updatedAt: "2026-10-05T00:00:00Z",
    doneAt: "2026-10-05T00:00:00Z",
    statusSince: null,
    parentId: null,
    hasSubtasks: false,
    assignees: [user("a", "Alpha")],
    tags: [],
    assessment: score === undefined ? null : assessment(score),
    overrideLevel: null,
    overrideNote: null,
    ...rest,
  };
}

const scope: WorkspaceScope = {
  workspaceId: "w1",
  name: "Acme",
  spaceIds: null,
  excludedStatuses: [],
  deliveredStatuses: [],
  excludedMembers: [],
};

const job = (over: Partial<TicketJobEvent> = {}): TicketJobEvent => ({
  kind: "assess",
  state: "running",
  done: 1,
  total: 4,
  failed: 0,
  error: null,
  jobId: 1,
  pendingFetches: 0,
  ...over,
});

const STATUSES = [
  { name: "closed", type: "closed" },
  { name: "complete", type: "done" },
  { name: "ready for testing", type: "custom" },
];

function model(over: Partial<Model> = {}): Model {
  const shown = [
    ticket("t1", { score: 2 }),
    ticket("t2", { assignees: [user("a", "Alpha"), user("b", "Beta")] }),
    ticket("t3", { assignees: [user("b", "Beta")] }),
    ticket("t4", { assignees: [] }),
  ];
  return {
    connected: true,
    workspaces: [
      { id: "w1", name: "Acme" },
      { id: "w2", name: "Beta Corp" },
    ],
    settings: { scope, points: DEFAULT_TICKET_POINTS, percent: 50 },
    setScope: vi.fn(),
    setPercent: vi.fn(),
    period: currentPeriod("month", NOW, null),
    setKind: vi.fn(),
    goPrevious: vi.fn(),
    goNext: vi.fn(),
    hasNext: true,
    tickets: shown,
    board: buildBoard(shown, DEFAULT_TICKET_POINTS),
    statuses: STATUSES,
    toAssess: 1,
    runIds: ["t2"],
    estimate: { tickets: 1, inputTokens: 12345 },
    workspacesState: "ready",
    workspacesError: null,
    connectError: null,
    retry: vi.fn(),
    ai: { runner: "claude-code", model: "sonnet", hasApiKey: false },
    job: null,
    error: null,
    refresh: vi.fn().mockResolvedValue(undefined),
    pendingFetches: 0,
    exportReport: vi.fn().mockResolvedValue("/tmp/report.html"),
    score: vi.fn().mockResolvedValue(undefined),
    cancel: vi.fn().mockResolvedValue(undefined),
    override: vi.fn().mockResolvedValue(undefined),
    reassess: vi.fn().mockResolvedValue(undefined),
    ...over,
  } as Model;
}

async function render(over: Partial<Model> = {}) {
  const m = model(over);
  vi.mocked(useTicketBoard).mockReturnValue(m);
  const onOpenSettings = vi.fn();
  await act(async () => {
    root.render(
      createElement(TicketContributorsView, {
        besideRail: true,
        compactRail: false,
        onClose: vi.fn(),
        onToggleSidebar: vi.fn(),
        onOpenSettings,
      }),
    );
  });
  return { m, onOpenSettings };
}

const click = (el: Element | null | undefined) =>
  act(async () => {
    (el as HTMLElement).click();
  });
const text = () => container.textContent ?? "";
const byText = (selector: string, needle: string) =>
  Array.from(container.querySelectorAll<HTMLElement>(selector)).find((el) =>
    (el.textContent ?? "").includes(needle),
  );
const option = (label: string) =>
  Array.from(document.querySelectorAll<HTMLElement>('[role="option"]')).find(
    (el) => (el.textContent ?? "").includes(label),
  );
async function pick(selectLabel: string, label: string) {
  await click(container.querySelector(`button[aria-label^="${selectLabel}"]`));
  await click(option(label));
}
const button = (name: string) =>
  container.querySelector<HTMLButtonElement>(`button[aria-label="${name}"]`);
const scoreButton = () =>
  Array.from(container.querySelectorAll<HTMLButtonElement>("button")).find(
    (b) => b.textContent?.trim() === "Score",
  );
const memberButton = (name: string) =>
  byText("tbody button[aria-expanded]", name) as HTMLButtonElement;

describe("TicketContributorsView connection", () => {
  it("shows the Settings button when ClickUp is not connected", async () => {
    const { onOpenSettings } = await render({
      connected: false,
      workspaces: [],
    });
    expect(text()).toContain("Connect ClickUp in Settings to score tickets.");
    expect(container.querySelector('[aria-label="Score coverage"]')).toBeNull();
    await click(byText("button", "Open Settings"));
    expect(onOpenSettings).toHaveBeenCalledTimes(1);
  });

  it("asks to pick a workspace when none is chosen", async () => {
    await render({
      settings: { scope: null, points: DEFAULT_TICKET_POINTS, percent: 100 },
    });
    expect(text()).toContain("Pick a ClickUp workspace.");
    expect(container.querySelector("table")).toBeNull();
  });

  it("picking a workspace sets a fresh scope", async () => {
    const { m } = await render();
    await pick("Workspace", "Beta Corp");
    expect(m.setScope).toHaveBeenCalledWith({
      workspaceId: "w2",
      name: "Beta Corp",
      spaceIds: null,
      excludedStatuses: [],
      deliveredStatuses: [],
      excludedMembers: [],
    });
  });

  it("says the token cannot access a saved workspace", async () => {
    const { onOpenSettings } = await render({
      settings: {
        scope: { ...scope, workspaceId: "gone", name: "Old Co" },
        points: DEFAULT_TICKET_POINTS,
        percent: 50,
      },
    });
    expect(text()).toContain(
      "This ClickUp token cannot access Old Co. Pick another workspace or update the token in Settings.",
    );
    expect(container.querySelector("table")).toBeNull();
    expect(container.querySelector('[aria-label="Score coverage"]')).toBeNull();
    expect(button("Refresh from ClickUp")?.disabled).toBe(true);
    await click(byText("button", "Open Settings"));
    expect(onOpenSettings).toHaveBeenCalledTimes(1);
  });

  it("lets the user pick another workspace when the saved one is gone", async () => {
    const { m } = await render({
      settings: {
        scope: { ...scope, workspaceId: "gone", name: "Old Co" },
        points: DEFAULT_TICKET_POINTS,
        percent: 50,
      },
    });
    await pick("Workspace", "Beta Corp");
    expect(m.setScope).toHaveBeenCalledWith(
      expect.objectContaining({ workspaceId: "w2" }),
    );
  });

  it("does not call the saved workspace unreachable while the list loads", async () => {
    await render({
      workspaces: [],
      workspacesState: "loading",
      settings: {
        scope: { ...scope, workspaceId: "gone", name: "Old Co" },
        points: DEFAULT_TICKET_POINTS,
        percent: 50,
      },
    });
    expect(text()).not.toContain("cannot access");
  });

  it("does not flash 'no workspaces' while the list loads", async () => {
    await render({
      workspaces: [],
      workspacesState: "loading",
      settings: { scope: null, points: DEFAULT_TICKET_POINTS, percent: 100 },
    });
    expect(text()).toContain("Loading workspaces...");
    expect(text()).not.toContain("no workspaces");
  });

  it("says a token without workspaces has none once the list loaded", async () => {
    await render({
      workspaces: [],
      settings: { scope: null, points: DEFAULT_TICKET_POINTS, percent: 100 },
    });
    expect(text()).toContain("This ClickUp token has no workspaces.");
  });

  it("offers Settings and Retry when ClickUp cannot be reached", async () => {
    const { m, onOpenSettings } = await render({
      connected: null,
      connectError: "network down",
      workspaces: [],
    });
    expect(text()).toContain("Could not reach ClickUp. network down");
    expect(text()).not.toContain("Checking ClickUp...");
    await click(byText("button", "Retry"));
    expect(m.retry).toHaveBeenCalledTimes(1);
    await click(byText("button", "Open Settings"));
    expect(onOpenSettings).toHaveBeenCalledTimes(1);
  });

  it("offers Settings and Retry when the workspace list fails", async () => {
    const { m, onOpenSettings } = await render({
      workspaces: [],
      workspacesState: "error",
      workspacesError: "403 forbidden",
    });
    expect(text()).toContain(
      "Could not load ClickUp workspaces. 403 forbidden",
    );
    expect(text()).not.toContain("no workspaces");
    expect(container.querySelector("table")).toBeNull();
    await click(byText("button", "Retry"));
    expect(m.retry).toHaveBeenCalledTimes(1);
    await click(byText("button", "Open Settings"));
    expect(onOpenSettings).toHaveBeenCalledTimes(1);
  });

  it("shows a job or load error", async () => {
    await render({ error: "ClickUp said no" });
    expect(container.querySelector('[role="alert"]')?.textContent).toContain(
      "ClickUp said no",
    );
  });
});

describe("TicketContributorsView period", () => {
  it("offers Month, Quarter, Year and All and reports the pick", async () => {
    const { m } = await render();
    const labels = Array.from(
      container.querySelectorAll('[aria-label="Period"] [role="radio"]'),
    ).map((el) => el.textContent);
    expect(labels).toEqual(["Month", "Quarter", "Year", "All"]);
    await click(byText('[role="radio"]', "Quarter"));
    expect(m.setKind).toHaveBeenCalledWith("quarter");
  });

  it("navigates with the arrows and disables next at the end", async () => {
    const { m } = await render({ hasNext: false });
    await click(button("Previous period"));
    expect(m.goPrevious).toHaveBeenCalledTimes(1);
    expect(button("Next period")?.disabled).toBe(true);
  });

  it("has no arrows for All", async () => {
    await render({ period: currentPeriod("all", NOW, null) });
    expect(button("Previous period")).toBeNull();
  });
});

describe("TicketContributorsView scope", () => {
  const openScope = () => click(byText("button", "Scope"));

  it("lists the spaces and saves a space pick", async () => {
    const { m } = await render();
    await openScope();
    expect(text()).toContain("All spaces");
    const eng = byText("label", "Engineering")?.querySelector("input");
    await click(eng);
    expect(m.setScope).toHaveBeenCalledWith({ ...scope, spaceIds: ["s1"] });
  });

  it("explains an empty space list", async () => {
    vi.mocked(listWorkspaceSpaces).mockResolvedValue([]);
    await render();
    await openScope();
    expect(text()).toContain(
      "This token cannot list spaces. Tickets from every space are included.",
    );
  });

  const group = (name: string) =>
    container.querySelector<HTMLElement>(
      `[role="group"][aria-label="${name}"]`,
    );
  const row = (name: string, status: string) =>
    Array.from(group(name)?.querySelectorAll("label") ?? [])
      .find((el) => (el.textContent ?? "").startsWith(status))
      ?.querySelector("input") as HTMLInputElement | undefined;

  it("excludes a status", async () => {
    const { m } = await render();
    await openScope();
    await click(row("Excluded statuses", "complete"));
    expect(m.setScope).toHaveBeenCalledWith({
      ...scope,
      excludedStatuses: ["complete"],
    });
  });

  it("offers every loaded status for exclusion, open ones too", async () => {
    await render();
    await openScope();
    expect(row("Excluded statuses", "ready for testing")).toBeDefined();
  });

  it("lists the delivered statuses with their type and the help text", async () => {
    await render();
    await openScope();
    const box = group("Delivered statuses")!;
    expect(box.textContent).toContain(
      "Tickets in these statuses count as delivered work.",
    );
    expect(box.textContent).toContain("ready for testing");
    expect(box.textContent).toContain("custom");
    expect(box.textContent).toContain("done");
  });

  it("shows done and closed statuses checked by default, custom ones not", async () => {
    await render();
    await openScope();
    expect(row("Delivered statuses", "closed")?.checked).toBe(true);
    expect(row("Delivered statuses", "complete")?.checked).toBe(true);
    expect(row("Delivered statuses", "ready for testing")?.checked).toBe(false);
  });

  it("adds a custom status to the delivered list when checked", async () => {
    const { m } = await render();
    await openScope();
    await click(row("Delivered statuses", "ready for testing"));
    expect(m.setScope).toHaveBeenCalledWith({
      ...scope,
      deliveredStatuses: ["ready for testing"],
    });
  });

  it("removes a custom status from the delivered list when unchecked", async () => {
    const { m } = await render({
      settings: {
        scope: { ...scope, deliveredStatuses: ["ready for testing", "other"] },
        points: DEFAULT_TICKET_POINTS,
        percent: 50,
      },
    });
    await openScope();
    expect(row("Delivered statuses", "ready for testing")?.checked).toBe(true);
    await click(row("Delivered statuses", "ready for testing"));
    expect(m.setScope).toHaveBeenCalledWith({
      ...scope,
      deliveredStatuses: ["other"],
    });
  });

  it("excludes a done status when it is unchecked", async () => {
    const { m } = await render();
    await openScope();
    await click(row("Delivered statuses", "complete"));
    expect(m.setScope).toHaveBeenCalledWith({
      ...scope,
      excludedStatuses: ["complete"],
    });
  });

  it("re-checking an excluded done status lifts the exclusion", async () => {
    const { m } = await render({
      settings: {
        scope: { ...scope, excludedStatuses: ["complete", "x"] },
        points: DEFAULT_TICKET_POINTS,
        percent: 50,
      },
    });
    await openScope();
    expect(row("Delivered statuses", "complete")?.checked).toBe(false);
    await click(row("Delivered statuses", "complete"));
    expect(m.setScope).toHaveBeenCalledWith({
      ...scope,
      excludedStatuses: ["x"],
    });
  });

  it("checking an excluded custom status delivers it and lifts the exclusion", async () => {
    const { m } = await render({
      settings: {
        scope: {
          ...scope,
          deliveredStatuses: ["ready for testing"],
          excludedStatuses: ["ready for testing"],
        },
        points: DEFAULT_TICKET_POINTS,
        percent: 50,
      },
    });
    await openScope();
    expect(row("Delivered statuses", "ready for testing")?.checked).toBe(false);
    await click(row("Delivered statuses", "ready for testing"));
    expect(m.setScope).toHaveBeenCalledWith({
      ...scope,
      deliveredStatuses: ["ready for testing"],
      excludedStatuses: [],
    });
  });

  it("keeps a chosen status listed even when no loaded ticket has it", async () => {
    await render({
      settings: {
        scope: { ...scope, deliveredStatuses: ["gone status"] },
        points: DEFAULT_TICKET_POINTS,
        percent: 50,
      },
    });
    await openScope();
    expect(row("Delivered statuses", "gone status")?.checked).toBe(true);
  });
});

describe("TicketContributorsView score card", () => {
  it("counts from the board, not from the loaded window", async () => {
    // `tickets` holds a wider window than the board; the card must ignore it.
    const wide = Array.from({ length: 40 }, (_, i) => ticket(`x${i}`));
    await render({ tickets: wide });
    expect(text()).toContain("4 delivered tickets in Oct 2026.");
    expect(text()).toContain("1 scored, 3 not scored yet.");
  });

  it("says so when every status is excluded", async () => {
    await render({
      settings: {
        scope: {
          ...scope,
          excludedStatuses: ["closed", "complete", "Ready For Testing"],
        },
        points: DEFAULT_TICKET_POINTS,
        percent: 50,
      },
      board: buildBoard([], DEFAULT_TICKET_POINTS),
      toAssess: 0,
    });
    expect(text()).toContain("Every status is excluded, so no ticket counts.");
  });

  it("does not say it while some status is not excluded", async () => {
    await render({
      settings: {
        scope: { ...scope, excludedStatuses: ["closed"] },
        points: DEFAULT_TICKET_POINTS,
        percent: 50,
      },
    });
    expect(text()).not.toContain("Every status is excluded");
  });

  it("shows the approximate dates notice only when flagged", async () => {
    const notice =
      "Some delivery dates are approximate (last update). Turn on the Total time in Status ClickApp in ClickUp for exact dates.";
    await render();
    expect(text()).not.toContain("approximate");
    const shown = [
      ticket("t1", { status: "ready", statusType: "custom", doneAt: null }),
    ];
    const withDates = buildBoard(shown, DEFAULT_TICKET_POINTS);
    expect(withDates.datesApproximate).toBe(true);
    await render({ board: withDates });
    expect(text()).toContain(notice);
  });

  it("says across all time for the All period", async () => {
    await render({ period: currentPeriod("all", NOW, null) });
    expect(text()).toContain("4 delivered tickets across all time.");
    expect(text()).not.toContain("in all time");
  });

  it("states what the next run does", async () => {
    await render();
    expect(text()).toContain(
      "Scoring 1 more ticket with sonnet (Claude Code). About 12,345 input tokens (estimate).",
    );
  });

  it("uses plural and thousands separators", async () => {
    const board: Board = {
      total: 1200,
      assessed: 1,
      members: [],
      unassigned: { tickets: 0, assessed: 0 },
      datesApproximate: false,
    };
    await render({ board, toAssess: 1100, runIds: [] });
    expect(text()).toContain("1,200 delivered tickets");
    expect(text()).toContain("1 scored, 1,199 not scored yet.");
    expect(text()).toContain("Scoring 1,100 more tickets");
  });

  it("says 1 delivered ticket", async () => {
    const shown = [ticket("t1")];
    await render({ board: buildBoard(shown, DEFAULT_TICKET_POINTS) });
    expect(text()).toContain("1 delivered ticket in");
  });

  it("score calls score", async () => {
    const { m } = await render();
    expect(scoreButton()?.disabled).toBe(false);
    await click(scoreButton());
    expect(m.score).toHaveBeenCalledTimes(1);
  });

  it("disables Score and explains when coverage is reached", async () => {
    await render({ toAssess: 0, runIds: [], estimate: null });
    expect(text()).toContain("Coverage already reached. Nothing to score.");
    expect(text()).not.toContain("Scoring 0");
    expect(scoreButton()?.disabled).toBe(true);
  });

  const typeCoverage = async (value: string) => {
    const input = container.querySelector<HTMLInputElement>(
      'input[type="number"][aria-label="Score coverage"]',
    )!;
    await act(async () => {
      const setter = Object.getOwnPropertyDescriptor(
        HTMLInputElement.prototype,
        "value",
      )!.set!;
      setter.call(input, value);
      input.dispatchEvent(new Event("input", { bubbles: true }));
    });
    return input;
  };

  it("commits the coverage on blur, not while typing", async () => {
    const { m } = await render();
    const input = await typeCoverage("80");
    expect(m.setPercent).not.toHaveBeenCalled();
    expect(input.value).toBe("80");
    await act(async () => {
      input.dispatchEvent(new FocusEvent("focusout", { bubbles: true }));
    });
    expect(m.setPercent).toHaveBeenCalledWith(80);
  });

  it("commits the coverage on Enter, clamped to 0..100", async () => {
    const { m } = await render();
    const input = await typeCoverage("250");
    await act(async () => {
      input.dispatchEvent(
        new KeyboardEvent("keydown", { key: "Enter", bubbles: true }),
      );
    });
    expect(m.setPercent).toHaveBeenCalledWith(100);
  });

  it("keeps an emptied coverage field empty until blur, then restores it", async () => {
    const { m } = await render();
    const input = await typeCoverage("");
    expect(m.setPercent).not.toHaveBeenCalled();
    expect(input.value).toBe("");
    await act(async () => {
      input.dispatchEvent(new FocusEvent("focusout", { bubbles: true }));
    });
    expect(m.setPercent).not.toHaveBeenCalled();
    expect(input.value).toBe("50");
  });

  it("shows progress and Cancel while scoring and blocks Score", async () => {
    const { m } = await render({ job: job({ failed: 1 }) });
    const bar = container.querySelector('[role="progressbar"]');
    expect(bar?.getAttribute("aria-valuenow")).toBe("1");
    expect(bar?.getAttribute("aria-valuemax")).toBe("4");
    expect(text()).toContain("1/4");
    expect(text()).toContain("1 failed");
    expect(scoreButton()?.disabled).toBe(true);
    await click(byText("button", "Cancel"));
    expect(m.cancel).toHaveBeenCalledTimes(1);
  });

  it("shows a fetch job and blocks Refresh and Score", async () => {
    await render({ job: job({ kind: "fetch", state: "queued", done: 0 }) });
    expect(text()).toContain("Fetching tickets from ClickUp...");
    expect(text()).toContain("0/4");
    expect(button("Refresh from ClickUp")?.disabled).toBe(true);
    expect(scoreButton()?.disabled).toBe(true);
  });

  it("keeps Refresh enabled during an assess job", async () => {
    await render({ job: job() });
    expect(button("Refresh from ClickUp")?.disabled).toBe(false);
  });

  it("refresh calls refresh", async () => {
    const { m } = await render();
    await click(button("Refresh from ClickUp"));
    expect(m.refresh).toHaveBeenCalledTimes(1);
  });

  it("reports tickets that could not be scored after a run", async () => {
    await render({ job: job({ state: "done", done: 4, failed: 2 }) });
    expect(text()).toContain("2 tickets could not be scored");
    expect(container.querySelector('[role="progressbar"]')).toBeNull();
  });
});

describe("TicketContributorsView leaderboard", () => {
  it("ranks members with points, tickets, coverage and levels", async () => {
    await render();
    const rows = Array.from(container.querySelectorAll("tbody > tr")).map((r) =>
      Array.from(r.querySelectorAll(":scope > td")).map((c) =>
        c.textContent?.trim(),
      ),
    );
    // Alpha: t1 (score 2 -> S = 8) + half of t2 (unscored).
    expect(rows[0].slice(0, 5)).toEqual(["1", "Alpha", "8", "2", "1/2"]);
    expect(rows[0].slice(6)).toEqual(["0", "1", "0", "0", "0"]);
    expect(rows[1].slice(0, 5)).toEqual(["2", "Beta", "0", "2", "0/2"]);
    expect(rows[2][1]).toBe("Unassigned");
    expect(rows[2][3]).toBe("1");
    expect(rows[2][4]).toBe("0/1");
  });

  it("has an Active column explained by its tooltip", async () => {
    await render();
    const header = Array.from(container.querySelectorAll("thead th")).find(
      (th) => th.textContent === "Active",
    );
    expect(header?.getAttribute("title")).toBe(
      "In a working status during this period. Not scored.",
    );
  });

  it("shows each member's active count, 0 when none", async () => {
    const shown = [ticket("t1", { score: 2 })];
    const active = [
      ticket("w1", { statusType: "custom", assignees: [user("a", "Alpha")] }),
      ticket("w2", { statusType: "custom", assignees: [user("a", "Alpha")] }),
      ticket("w3", { statusType: "custom", assignees: [user("c", "Cy")] }),
    ];
    await render({ board: buildBoard(shown, DEFAULT_TICKET_POINTS, active) });
    const cells = (name: string) =>
      Array.from(container.querySelectorAll("tbody > tr"))
        .find(
          (r) =>
            r.querySelector(":scope > td:nth-child(2)")?.textContent === name,
        )
        ?.querySelectorAll(":scope > td");
    expect(cells("Alpha")?.[5].textContent).toBe("2");
    // Cy delivered nothing, but is working on one ticket.
    expect(cells("Cy")?.[5].textContent).toBe("1");
    expect(cells("Cy")?.[2].textContent).toBe("0");
  });

  it("says an active-only member has no delivered tickets", async () => {
    const active = [ticket("w3", { statusType: "custom" })];
    await render({
      board: buildBoard([], DEFAULT_TICKET_POINTS, active),
      toAssess: 0,
    });
    await click(memberButton("Alpha"));
    expect(text()).toContain("No delivered tickets in this period.");
  });

  it("rounds a fractional share to one decimal", async () => {
    const shown = [
      ticket("t1", {
        score: 6,
        assignees: [user("a", "Alpha"), user("b", "Beta"), user("c", "Cy")],
      }),
    ];
    await render({
      board: buildBoard(shown, { ...DEFAULT_TICKET_POINTS, L: 16 }),
    });
    // 16 / 3 = 5.333...
    expect(text()).toContain("5.3");
    expect(text()).not.toContain("5.33");
  });

  it("has no Unassigned row when every ticket has an assignee", async () => {
    const shown = [ticket("t1")];
    await render({ board: buildBoard(shown, DEFAULT_TICKET_POINTS) });
    expect(text()).not.toContain("Unassigned");
  });

  it("says so when no ticket closed in the period", async () => {
    await render({ board: buildBoard([], DEFAULT_TICKET_POINTS), toAssess: 0 });
    expect(text()).toContain("No delivered tickets in this period.");
    expect(scoreButton()?.disabled).toBe(true);
  });

  it("expands a member to their tickets", async () => {
    await render();
    expect(container.querySelector("a[href$='/t1']")).toBeNull();
    await click(memberButton("Alpha"));
    const link = container.querySelector<HTMLAnchorElement>("a[href$='/t1']");
    expect(link?.textContent).toBe("Ticket t1");
    expect(container.querySelector("a[href$='/t2']")).not.toBeNull();
    // Another member's ticket is not listed.
    expect(container.querySelector("a[href$='/t3']")).toBeNull();
    await click(memberButton("Alpha"));
    expect(container.querySelector("a[href$='/t1']")).toBeNull();
  });

  it("shows the level, edited mark and the points share", async () => {
    await render({
      board: buildBoard(
        [ticket("t1", { score: 2, overrideLevel: "L" })],
        DEFAULT_TICKET_POINTS,
      ),
    });
    await click(memberButton("Alpha"));
    const row = container.querySelector('tr[data-ticket="t1"]')!;
    expect(row.textContent).toContain("L");
    expect(row.textContent).toContain("edited");
    expect(row.textContent).toContain("32");
  });

  it("expands the reasons of a criterion", async () => {
    await render();
    await click(memberButton("Alpha"));
    expect(text()).not.toContain("Unclear brief.");
    await click(
      container.querySelector(
        'tr[data-ticket="t1"] button[aria-label^="Ambiguity 2"]',
      ),
    );
    expect(text()).toContain("Unclear brief.");
    expect(text()).toContain("Two modules.");
  });

  it("overrides a level", async () => {
    const { m } = await render();
    await click(memberButton("Alpha"));
    await pick("Level of Ticket t1", "XL");
    expect(m.override).toHaveBeenCalledWith("t1", "XL", null);
  });

  it("clears an override with AI level", async () => {
    const { m } = await render({
      board: buildBoard(
        [ticket("t1", { score: 2, overrideLevel: "L", overrideNote: "big" })],
        DEFAULT_TICKET_POINTS,
      ),
    });
    await click(memberButton("Alpha"));
    await pick("Level of Ticket t1", "AI level");
    expect(m.override).toHaveBeenCalledWith("t1", null, null);
  });

  it("re-assesses the selected tickets", async () => {
    const { m } = await render();
    await click(memberButton("Alpha"));
    const reassess = byText(
      "button",
      "Re-assess selected",
    ) as HTMLButtonElement;
    expect(reassess.disabled).toBe(true);
    await click(
      container.querySelector('input[aria-label="Select Ticket t1"]'),
    );
    expect(reassess.disabled).toBe(false);
    await click(reassess);
    expect(m.reassess).toHaveBeenCalledWith(["t1"]);
  });

  it("disables Re-assess selected while a job runs", async () => {
    await render({ job: job() });
    await click(memberButton("Alpha"));
    await click(
      container.querySelector('input[aria-label="Select Ticket t1"]'),
    );
    expect(
      (byText("button", "Re-assess selected") as HTMLButtonElement).disabled,
    ).toBe(true);
  });
});

describe("TicketContributorsView export", () => {
  const exportButton = () =>
    byText("button", "Export report") as HTMLButtonElement;

  it("enables Export report when delivered tickets are shown", async () => {
    await render();
    expect(exportButton().disabled).toBe(false);
  });

  it("disables it without a workspace", async () => {
    await render({
      settings: { scope: null, points: DEFAULT_TICKET_POINTS, percent: 100 },
    });
    expect(exportButton().disabled).toBe(true);
  });

  it("disables it while a fetch runs", async () => {
    await render({ job: job({ kind: "fetch" }) });
    expect(exportButton().disabled).toBe(true);
  });

  it("stays disabled while a fetch is queued behind an assessment", async () => {
    // The latest event is an assessment, so the view is not "fetching".
    await render({ job: job({ kind: "assess" }), pendingFetches: 1 });
    expect(exportButton().disabled).toBe(true);
    expect(button("Refresh from ClickUp")?.disabled).toBe(false);
  });

  it("disables it when no ticket was delivered", async () => {
    await render({ board: buildBoard([], DEFAULT_TICKET_POINTS) });
    expect(exportButton().disabled).toBe(true);
  });

  it("exports and says the report was saved", async () => {
    const { m } = await render();
    expect(text()).not.toContain("Report saved.");
    await click(exportButton());
    expect(m.exportReport).toHaveBeenCalledTimes(1);
    expect(text()).toContain("Report saved.");
  });

  it("shows nothing when the save dialog is cancelled", async () => {
    const { m } = await render({
      exportReport: vi.fn().mockResolvedValue(null),
    });
    await click(exportButton());
    expect(m.exportReport).toHaveBeenCalledTimes(1);
    expect(text()).not.toContain("Report saved.");
    expect(container.querySelector('[role="alert"]')).toBeNull();
  });

  it("shows Preparing report... on the button while it runs", async () => {
    let finish: (path: string) => void = () => undefined;
    await render({
      exportReport: vi.fn(
        () => new Promise<string | null>((resolve) => (finish = resolve)),
      ),
    });
    await click(exportButton());
    const busy = byText("button", "Preparing report...") as HTMLButtonElement;
    expect(busy.disabled).toBe(true);
    await act(async () => finish("/tmp/report.html"));
    expect(exportButton().disabled).toBe(false);
    expect(text()).toContain("Report saved.");
  });

  it("clears the saved message when the period changes", async () => {
    const { m } = await render();
    await click(exportButton());
    expect(text()).toContain("Report saved.");
    vi.mocked(useTicketBoard).mockReturnValue({
      ...m,
      period: currentPeriod("month", new Date(2026, 8, 5), null),
    });
    await act(async () => {
      root.render(
        createElement(TicketContributorsView, {
          besideRail: true,
          compactRail: false,
          onClose: vi.fn(),
          onToggleSidebar: vi.fn(),
          onOpenSettings: vi.fn(),
        }),
      );
    });
    expect(text()).not.toContain("Report saved.");
  });

  it("ignores an export that finishes after the period changed", async () => {
    let finish: (path: string) => void = () => undefined;
    const { m } = await render({
      exportReport: vi.fn(
        () => new Promise<string | null>((resolve) => (finish = resolve)),
      ),
    });
    await click(exportButton());
    vi.mocked(useTicketBoard).mockReturnValue({
      ...m,
      period: currentPeriod("month", new Date(2026, 8, 5), null),
    });
    await act(async () => {
      root.render(
        createElement(TicketContributorsView, {
          besideRail: true,
          compactRail: false,
          onClose: vi.fn(),
          onToggleSidebar: vi.fn(),
          onOpenSettings: vi.fn(),
        }),
      );
    });
    await act(async () => finish("/tmp/report.html"));
    expect(text()).not.toContain("Report saved.");
  });

  it("shows the export error next to a job error", async () => {
    await render({
      error: "ClickUp said no",
      exportReport: vi.fn().mockRejectedValue(new Error("disk full")),
    });
    await click(exportButton());
    const alerts = Array.from(container.querySelectorAll('[role="alert"]')).map(
      (el) => el.textContent,
    );
    expect(alerts).toEqual(["ClickUp said no", "disk full"]);
  });

  it("shows the error when the export fails", async () => {
    await render({
      exportReport: vi.fn().mockRejectedValue(new Error("disk full")),
    });
    await click(exportButton());
    expect(container.querySelector('[role="alert"]')?.textContent).toContain(
      "disk full",
    );
    expect(text()).not.toContain("Report saved.");
  });
});

describe("TicketContributorsView excluded members", () => {
  const BETA_AND_GONE: ExcludedMember[] = [
    { id: "b", name: "Beta" },
    { id: "gone", name: "Gone Gus" },
  ];
  const withScope = (excludedMembers: ExcludedMember[]) => ({
    settings: {
      scope: { ...scope, excludedMembers },
      points: DEFAULT_TICKET_POINTS,
      percent: 50,
    },
  });
  const group = () =>
    container.querySelector<HTMLElement>(
      '[role="group"][aria-label="Excluded members"]',
    );
  const openScope = () => click(byText("button", "Scope"));

  it("gives every member row a keyboard reachable Exclude button", async () => {
    await render();
    const exclude = button("Exclude Beta from the board");
    expect(exclude?.tagName).toBe("BUTTON");
    expect(exclude?.getAttribute("type")).toBe("button");
    expect(exclude?.tabIndex).not.toBe(-1);
    expect(button("Exclude Alpha from the board")).not.toBeNull();
  });

  it("excludes a member through setScope without a refetch", async () => {
    const { m } = await render(withScope([{ id: "z", name: "Zed" }]));
    await click(button("Exclude Beta from the board"));
    expect(m.setScope).toHaveBeenCalledTimes(1);
    expect(m.setScope).toHaveBeenCalledWith({
      ...scope,
      excludedMembers: [
        { id: "z", name: "Zed" },
        { id: "b", name: "Beta" },
      ],
    });
    expect(m.refresh).not.toHaveBeenCalled();
  });

  it("does not expand the row when Exclude is used", async () => {
    await render();
    await click(button("Exclude Beta from the board"));
    expect(container.querySelector("[data-member-tickets]")).toBeNull();
  });

  it("lists the excluded members in Scope with their names", async () => {
    await render(withScope(BETA_AND_GONE));
    await openScope();
    const box = group();
    expect(box?.textContent).toContain("Excluded members");
    expect(box?.textContent).toContain("Beta");
    // No loaded ticket carries this member: the stored name shows, not the id.
    expect(box?.textContent).toContain("Gone Gus");
    expect(box?.textContent).not.toContain("gone");
  });

  it("says nobody is excluded when the list is empty", async () => {
    await render();
    await openScope();
    expect(group()?.textContent).toContain("Nobody is excluded");
    expect(group()?.querySelector("button")).toBeNull();
  });

  it("restores a member through setScope without a refetch", async () => {
    const { m } = await render(withScope(BETA_AND_GONE));
    await openScope();
    await click(
      group()?.querySelector('button[aria-label="Restore Beta to the board"]'),
    );
    expect(m.setScope).toHaveBeenCalledTimes(1);
    expect(m.setScope).toHaveBeenCalledWith({
      ...scope,
      excludedMembers: [{ id: "gone", name: "Gone Gus" }],
    });
    expect(m.refresh).not.toHaveBeenCalled();
  });

  it("names a restore button after the stored name when no ticket has the member", async () => {
    await render(withScope(BETA_AND_GONE));
    await openScope();
    expect(
      group()?.querySelector(
        'button[aria-label="Restore Gone Gus to the board"]',
      ),
    ).not.toBeNull();
  });

  it("collapses an expanded row when its member is excluded", async () => {
    await render();
    await click(memberButton("Beta"));
    expect(container.querySelector('[data-member-tickets="b"]')).not.toBeNull();
    await click(button("Exclude Beta from the board"));
    expect(container.querySelector("[data-member-tickets]")).toBeNull();
  });
});
