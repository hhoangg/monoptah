// @vitest-environment happy-dom
import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

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

import {
  clickupConnected,
  listClickUpWorkspaces,
} from "../../inbox/model/clickup";
import { DEFAULT_TICKET_POINTS } from "../model/tickets";
import type { TicketRecord, WorkspaceScope } from "../model/tickets";
import { readSettings, writeSettings } from "../model/prefs";
import {
  estimateTickets,
  fetchTickets,
  getAiConfig,
  loadTickets,
  ticketJobStatus,
} from "../model/ticketsApi";
import { useTicketBoard } from "./useTicketBoard";

type Hook = ReturnType<typeof useTicketBoard>;

let container: HTMLDivElement;
let root: Root;
let latest: Hook;

function Probe() {
  latest = useTicketBoard();
  return null;
}

const scope: WorkspaceScope = {
  workspaceId: "w1",
  name: "Acme",
  spaceIds: null,
  excludedStatuses: [],
  deliveredStatuses: [],
  excludedMembers: [],
};

const now = new Date().toISOString();

const ticket = (id: string, over: Partial<TicketRecord>): TicketRecord => ({
  source: "clickup",
  id,
  title: `Ticket ${id}`,
  url: `https://app.clickup.com/t/${id}`,
  workspaceId: "w1",
  spaceId: "s1",
  status: "complete",
  statusType: "closed",
  updatedAt: now,
  doneAt: now,
  statusSince: null,
  parentId: null,
  hasSubtasks: false,
  assignees: [{ id: "a", name: "Alpha", email: "a@example.com" }],
  tags: [],
  assessment: null,
  overrideLevel: null,
  overrideNote: null,
  ...over,
});

const STORED = [
  ticket("done", {}),
  ticket("testing", {
    status: "ready for testing",
    statusType: "custom",
    doneAt: null,
  }),
];

beforeEach(async () => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  localStorage.clear();
  writeSettings({ scope, points: DEFAULT_TICKET_POINTS, percent: 100 });
  vi.mocked(clickupConnected).mockResolvedValue({ connected: true });
  vi.mocked(listClickUpWorkspaces).mockResolvedValue([]);
  vi.mocked(loadTickets).mockReset().mockResolvedValue(STORED);
  vi.mocked(fetchTickets).mockReset().mockResolvedValue(1);
  vi.mocked(getAiConfig).mockReset().mockResolvedValue({
    runner: "claude-code",
    model: "sonnet",
    hasApiKey: false,
  });
  vi.mocked(ticketJobStatus)
    .mockReset()
    .mockResolvedValue({ event: null, pendingFetches: 0 });
  container = document.createElement("div");
  document.body.append(container);
  root = createRoot(container);
  await act(async () => {
    root.render(createElement(Probe));
  });
});
afterEach(async () => {
  await act(async () => root.unmount());
  container.remove();
  vi.unstubAllGlobals();
});

describe("useTicketBoard delivered statuses", () => {
  it("lists every loaded status with its type", () => {
    expect(latest.statuses).toEqual([
      { name: "complete", type: "closed" },
      { name: "ready for testing", type: "custom" },
    ]);
  });

  it("starts with the type default and counts the working status as active", () => {
    expect(latest.board.total).toBe(1);
    expect(latest.board.members[0]).toMatchObject({ tickets: 1, active: 1 });
  });

  it("reclassifies on a scope change without loading or fetching again", async () => {
    const loads = vi.mocked(loadTickets).mock.calls.length;
    const fetches = vi.mocked(fetchTickets).mock.calls.length;
    expect(loads).toBeGreaterThan(0);

    await act(async () => {
      latest.setScope({ ...scope, deliveredStatuses: ["ready for testing"] });
    });
    expect(latest.board.total).toBe(2);
    expect(latest.board.members[0]).toMatchObject({ tickets: 2, active: 0 });

    // Unchecking a done status excludes it; the chosen extra still counts.
    await act(async () => {
      latest.setScope({
        ...scope,
        deliveredStatuses: ["ready for testing"],
        excludedStatuses: ["complete"],
      });
    });
    expect(latest.board.total).toBe(1);
    expect(readSettings().scope?.excludedStatuses).toEqual(["complete"]);

    expect(vi.mocked(loadTickets).mock.calls.length).toBe(loads);
    expect(vi.mocked(fetchTickets).mock.calls.length).toBe(fetches);
  });
});

describe("useTicketBoard excluded members", () => {
  const shared = ticket("shared", {
    assignees: [
      { id: "a", name: "Alpha", email: "a@example.com" },
      { id: "b", name: "Beta", email: "b@example.com" },
    ],
    overrideLevel: "S",
  });

  it("applies an exclusion without loading or fetching again", async () => {
    vi.mocked(loadTickets).mockResolvedValue([...STORED, shared]);
    await act(async () => root.unmount());
    root = createRoot(container);
    await act(async () => {
      root.render(createElement(Probe));
    });
    const before = latest.board.members.find((m) => m.id === "a");
    expect(before?.tickets).toBe(2);
    const loads = vi.mocked(loadTickets).mock.calls.length;
    const fetches = vi.mocked(fetchTickets).mock.calls.length;

    await act(async () => {
      latest.setScope({
        ...scope,
        excludedMembers: [{ id: "b", name: "Beta" }],
      });
    });
    // Alpha keeps the whole shared ticket; Beta is gone from the board.
    expect(latest.board.members.map((m) => m.id)).toEqual(["a"]);
    expect(latest.board.members[0]).toMatchObject({ tickets: 2, points: 8 });
    expect(latest.board.total).toBe(2);
    expect(readSettings().scope?.excludedMembers).toEqual([
      { id: "b", name: "Beta" },
    ]);

    expect(vi.mocked(loadTickets).mock.calls.length).toBe(loads);
    expect(vi.mocked(fetchTickets).mock.calls.length).toBe(fetches);
  });

  it("drops a ticket whose assignees are all excluded from N, the run and the estimate", async () => {
    expect(latest.board.total).toBe(1);
    expect(latest.runIds).toEqual(["done"]);

    await act(async () => {
      latest.setScope({
        ...scope,
        excludedMembers: [{ id: "a", name: "Alpha" }],
      });
    });
    expect(latest.board.total).toBe(0);
    expect(latest.board.members).toEqual([]);
    expect(latest.toAssess).toBe(0);
    expect(latest.runIds).toEqual([]);
    expect(latest.estimate).toBeNull();

    // Restoring brings the ticket back.
    await act(async () => {
      latest.setScope({ ...scope, excludedMembers: [] });
    });
    expect(latest.board.total).toBe(1);
    expect(latest.runIds).toEqual(["done"]);
  });

  it("sends a restored member's tickets to the estimate again", async () => {
    vi.mocked(estimateTickets)
      .mockReset()
      .mockResolvedValue({ tickets: 1, inputTokens: 10 });
    const settle = () =>
      act(async () => {
        await new Promise((resolve) => setTimeout(resolve, 400));
      });

    await act(async () => {
      latest.setScope({
        ...scope,
        excludedMembers: [{ id: "a", name: "Alpha" }],
      });
    });
    await settle();
    expect(estimateTickets).not.toHaveBeenCalled();

    await act(async () => {
      latest.setScope({ ...scope, excludedMembers: [] });
    });
    await settle();
    expect(estimateTickets).toHaveBeenCalledWith(["done"]);
    expect(latest.estimate).toEqual({ tickets: 1, inputTokens: 10 });
  });

  it("saves a newer name of an excluded member seen in the loaded tickets", async () => {
    await act(async () => {
      latest.setScope({
        ...scope,
        excludedMembers: [
          { id: "a", name: "Old Alpha" },
          { id: "left", name: "Left Lee" },
        ],
      });
    });
    expect(readSettings().scope?.excludedMembers).toEqual([
      { id: "a", name: "Alpha" },
      { id: "left", name: "Left Lee" },
    ]);
  });
});
