// @vitest-environment happy-dom
import { listen } from "@tauri-apps/api/event";
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
import { writeSettings } from "../model/prefs";
import { DEFAULT_TICKET_POINTS } from "../model/tickets";
import type { WorkspaceScope } from "../model/tickets";
import {
  fetchTickets,
  getAiConfig,
  loadTickets,
  ticketJobStatus,
} from "../model/ticketsApi";
import type { TicketJobEvent, TicketJobStatus } from "../model/ticketsApi";
import { useTicketBoard } from "./useTicketBoard";

type Hook = ReturnType<typeof useTicketBoard>;

let container: HTMLDivElement;
let root: Root;
let latest: Hook;
let emit: (payload: TicketJobEvent) => Promise<void>;
let workspaceCount = 0;

function Probe() {
  latest = useTicketBoard();
  return null;
}

/** The fetched windows live for the session, so each test has its own workspace. */
const freshScope = (): WorkspaceScope => ({
  workspaceId: `w${(workspaceCount += 1)}`,
  name: "Acme",
  spaceIds: null,
  excludedStatuses: [],
  deliveredStatuses: [],
  excludedMembers: [],
});

const event = (
  kind: TicketJobEvent["kind"],
  state: TicketJobEvent["state"],
  over: Partial<TicketJobEvent> = {},
): TicketJobEvent => ({
  kind,
  state,
  done: 0,
  total: 1,
  failed: 0,
  error: null,
  jobId: 1,
  pendingFetches: 0,
  ...over,
});

const idle: TicketJobStatus = { event: null, pendingFetches: 0 };

async function mount(connected = true) {
  vi.mocked(clickupConnected).mockResolvedValue({ connected });
  container = document.createElement("div");
  document.body.append(container);
  root = createRoot(container);
  await act(async () => {
    root.render(createElement(Probe));
  });
}

async function unmount() {
  await act(async () => root.unmount());
  container.remove();
}

beforeEach(() => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  localStorage.clear();
  writeSettings({
    scope: freshScope(),
    points: DEFAULT_TICKET_POINTS,
    percent: 100,
  });
  vi.mocked(listClickUpWorkspaces).mockResolvedValue([]);
  vi.mocked(loadTickets).mockReset().mockResolvedValue([]);
  vi.mocked(fetchTickets).mockReset().mockResolvedValue(1);
  vi.mocked(getAiConfig).mockReset().mockResolvedValue({
    runner: "claude-code",
    model: "sonnet",
    hasApiKey: false,
  });
  vi.mocked(ticketJobStatus).mockReset().mockResolvedValue(idle);
  vi.mocked(listen)
    .mockReset()
    .mockImplementation(async (_name, handler) => {
      emit = (payload) =>
        act(async () => {
          (handler as (e: { payload: TicketJobEvent }) => void)({ payload });
        });
      return () => undefined;
    });
});
afterEach(async () => {
  await unmount();
  vi.unstubAllGlobals();
});

describe("useTicketBoard pending fetches", () => {
  it("takes the count from the backend on reopen, with an assessment running and a fetch queued behind it", async () => {
    vi.mocked(ticketJobStatus).mockResolvedValue({
      event: event("assess", "running", { jobId: 4, pendingFetches: 1 }),
      pendingFetches: 1,
    });
    await mount(false);
    expect(latest.job?.kind).toBe("assess");
    expect(latest.pendingFetches).toBe(1);
  });

  it("takes two pending fetches on reopen, not one", async () => {
    vi.mocked(ticketJobStatus).mockResolvedValue({
      event: event("fetch", "running", { jobId: 4, pendingFetches: 2 }),
      pendingFetches: 2,
    });
    await mount(false);
    expect(latest.pendingFetches).toBe(2);
  });

  it("starts at zero when nothing is queued or running", async () => {
    await mount(false);
    expect(latest.pendingFetches).toBe(0);
  });

  it("follows the count each queued and ended event carries", async () => {
    await mount(false);
    await emit(event("fetch", "queued", { jobId: 1, pendingFetches: 1 }));
    expect(latest.pendingFetches).toBe(1);
    await emit(event("assess", "running", { jobId: 2, pendingFetches: 1 }));
    await emit(event("fetch", "queued", { jobId: 3, pendingFetches: 2 }));
    expect(latest.pendingFetches).toBe(2);
    await emit(event("fetch", "done", { jobId: 1, pendingFetches: 1 }));
    expect(latest.pendingFetches).toBe(1);
    // The assessment ending leaves the queued fetch pending.
    await emit(event("assess", "done", { jobId: 2, pendingFetches: 1 }));
    expect(latest.pendingFetches).toBe(1);
    await emit(event("fetch", "done", { jobId: 3, pendingFetches: 0 }));
    expect(latest.pendingFetches).toBe(0);
  });

  it("ignores the count of a progress event, which can be stale", async () => {
    await mount(false);
    await emit(event("fetch", "queued", { jobId: 1, pendingFetches: 1 }));
    await emit(event("fetch", "running", { jobId: 1, pendingFetches: 0 }));
    expect(latest.pendingFetches).toBe(1);
  });

  it("does not let a status read that began earlier overwrite a newer event", async () => {
    let answer: (status: TicketJobStatus) => void = () => undefined;
    vi.mocked(ticketJobStatus).mockReturnValue(
      new Promise((resolve) => {
        answer = resolve;
      }),
    );
    await mount(false);
    await emit(event("fetch", "queued", { jobId: 1, pendingFetches: 3 }));
    await act(async () => answer(idle));
    expect(latest.pendingFetches).toBe(3);
  });
});

describe("useTicketBoard fetched windows", () => {
  it("marks the window fetched only when its own fetch ends done cleanly", async () => {
    vi.mocked(fetchTickets).mockResolvedValue(11);
    await mount();
    expect(fetchTickets).toHaveBeenCalledTimes(1);
    // Queued is not fetched: a reopened view would still fetch it.
    await unmount();
    await mount();
    await emit(event("fetch", "queued", { jobId: 11, pendingFetches: 1 }));
    expect(fetchTickets).toHaveBeenCalledTimes(2);
    await emit(event("fetch", "done", { jobId: 11 }));
    await unmount();
    vi.mocked(fetchTickets).mockClear();
    await mount();
    expect(fetchTickets).not.toHaveBeenCalled();
  });

  it("marks it when the end arrives before the command's answer", async () => {
    vi.mocked(fetchTickets).mockImplementation(async () => {
      await emit(event("fetch", "done", { jobId: 12 }));
      return 12;
    });
    await mount();
    await unmount();
    vi.mocked(fetchTickets).mockClear();
    await mount();
    expect(fetchTickets).not.toHaveBeenCalled();
  });

  it.each([
    ["an error", event("fetch", "error", { jobId: 13, error: "boom" })],
    ["a cancel", event("fetch", "cancelled", { jobId: 13 })],
    [
      "a done with a failed link",
      event("fetch", "done", { jobId: 13, failed: 1 }),
    ],
  ])(
    "does not mark it after %s, and does not retry on its own",
    async (_name, end) => {
      vi.mocked(fetchTickets).mockResolvedValue(13);
      await mount();
      await emit(end);
      expect(fetchTickets).toHaveBeenCalledTimes(1);
      await unmount();
      vi.mocked(fetchTickets).mockClear();
      await mount();
      expect(fetchTickets).toHaveBeenCalledTimes(1);
    },
  );

  it("does not mark a window because another job ended", async () => {
    vi.mocked(fetchTickets).mockResolvedValue(14);
    await mount();
    await emit(event("fetch", "done", { jobId: 99 }));
    await emit(event("assess", "done", { jobId: 14 }));
    await unmount();
    vi.mocked(fetchTickets).mockClear();
    await mount();
    expect(fetchTickets).toHaveBeenCalledTimes(1);
  });

  it("Refresh adds nothing while that window's fetch is pending", async () => {
    vi.mocked(fetchTickets).mockResolvedValue(15);
    await mount();
    await act(async () => {
      await latest.refresh();
    });
    expect(fetchTickets).toHaveBeenCalledTimes(1);
    await emit(event("fetch", "done", { jobId: 15 }));
    await act(async () => {
      await latest.refresh();
    });
    expect(fetchTickets).toHaveBeenCalledTimes(2);
  });

  it("forgets a window whose fetch the command refused", async () => {
    vi.mocked(fetchTickets).mockRejectedValue(new Error("busy"));
    await mount(false);
    await act(async () => {
      await expect(latest.refresh()).resolves.toBeUndefined();
    });
    expect(latest.error).toBe("busy");
    vi.mocked(fetchTickets).mockResolvedValue(16);
    await act(async () => {
      await latest.refresh();
    });
    expect(fetchTickets).toHaveBeenCalledTimes(2);
  });
});
