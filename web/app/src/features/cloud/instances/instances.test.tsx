// U-17: the cloud instance list over the mock desktop (rows, stop, resume, read-only, forbidden controls).
import { QueryClientProvider } from "@tanstack/react-query";
import { createMemoryHistory, RouterProvider } from "@tanstack/react-router";
import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { FIXTURE_HANDLERS, FixtureError } from "@monoptah/mock-desktop";
import { MemoryPersistAdapter, registerPersistAdapter, type PersistAdapter } from "@/data";
import { stubTranscriptLayout } from "@/features/transcript/layoutStub";
import { resetLinkConnections } from "@/link";
import { clearLinkStores } from "@/link/store.ts";
import { createQueryClient } from "@/lib/queryClient";
import { createAppRouter } from "@/router";
import { clearHooks } from "@/ui/hooks";
import { clearSlots } from "@/ui/slots";
import { resetSessionActivity } from "@/features/sessions/activityStore";
import { DESKTOP_ID, connectMockDesktop, seedWorkspace, type MockLinkOptions } from "@/features/sessions/dev/mockLink";
import { pickResumeSession } from "./InstanceRow";
import {
  INSTANCES_FAST_WINDOW_MS,
  INSTANCES_POLL_CAP_MS,
  INSTANCES_POLL_MS,
  INSTANCES_SLOW_POLL_MS,
  OWN_ACTION_WATCH_MS,
  instancesRefetchInterval,
  type CloudInstancesResult,
} from "./useCloudInstances";
import { lastUsedLabel } from "./copy";

beforeAll(() => {
  window.matchMedia = ((query: string) => ({
    matches: query.includes("min-width: 640px") && query.includes("pointer: fine"),
    media: query,
    onchange: null,
    addEventListener() {},
    removeEventListener() {},
    addListener() {},
    removeListener() {},
    dispatchEvent: () => false,
  })) as never;
  stubTranscriptLayout();
  window.scrollTo = vi.fn() as never;
});

const original = { ...FIXTURE_HANDLERS };
let previous: PersistAdapter;

beforeEach(() => {
  localStorage.clear();
  previous = registerPersistAdapter(new MemoryPersistAdapter());
});
afterEach(() => {
  Object.assign(FIXTURE_HANDLERS, original);
  resetLinkConnections();
  clearLinkStores();
  registerPersistAdapter(previous);
  resetSessionActivity();
  clearSlots();
  clearHooks();
  localStorage.clear();
});

function mount(options: MockLinkOptions = {}) {
  connectMockDesktop({
    ...options,
    seed: (desktop) => {
      seedWorkspace(desktop);
      options.seed?.(desktop);
    },
  });
  const queryClient = createQueryClient();
  const router = createAppRouter({ history: createMemoryHistory({ initialEntries: [`/d/${DESKTOP_ID}/cloud`] }), queryClient });
  render(
    <QueryClientProvider client={queryClient}>
      <RouterProvider router={router} />
    </QueryClientProvider>,
  );
}

const rowOf = async (name: string) => {
  const heading = await screen.findByRole("heading", { name });
  return within(heading.closest("li") as HTMLElement);
};

const override = (instances: Partial<CloudInstancesResult["instances"][number]>[], extra: Record<string, unknown> = {}) => {
  FIXTURE_HANDLERS["cloud.instances"] = (() => ({
    instances: instances.map((i) => ({ kind: "codespace", instance: "x", displayName: "X", state: "available", sessions: [], ...i })),
    ...extra,
  })) as never;
};

describe("instance rows", () => {
  it("lists each instance with kind, state as text, cost, sessions and otherCount", async () => {
    mount();
    const one = await rowOf("Fake codespace one");
    expect(one.getByText(/GitHub codespace/)).toBeInTheDocument();
    expect(one.getByText("Available")).toBeInTheDocument();
    expect(one.getByText("Cost: about $0.18/hour (estimate)")).toBeInTheDocument();
    expect(one.getByRole("link", { name: "Refactor on the fake codespace" })).toHaveAttribute("href", `/d/${DESKTOP_ID}/s/sess-codespace-1`);
    expect(one.getByText(/Last used: unknown/)).toBeInTheDocument();

    const sandbox = await rowOf("Fake sandbox one");
    expect(sandbox.getByText(/Docker sandbox/)).toBeInTheDocument();
    expect(sandbox.getByText("Stopped")).toBeInTheDocument();
    expect(sandbox.getByText(/Last used: \d+ (hours?|days?) ago/)).toBeInTheDocument();

    expect((await rowOf("Fake codespace two")).getByText("Starting")).toBeInTheDocument();
    expect(screen.getByTestId("cloud-other")).toHaveTextContent("2 other instances on this desktop");
  });

  it("shows the stop report texts", async () => {
    mount();
    const stopping = await rowOf("Fake sandbox two");
    expect(stopping.getByText("Stopping is taking longer than usual")).toBeInTheDocument();
  });

  it.each([
    ["stopping", "Stopping"],
    ["takingLong", "Stopping is taking longer than usual"],
    ["stillRunning", "The instance is still running"],
  ] as const)("renders stopReport %s", async (report, text) => {
    override([{ displayName: "Reporting", state: report === "stillRunning" ? "available" : "stopping", stopReport: report }]);
    mount();
    const row = await rowOf("Reporting");
    expect(row.getByTestId("stop-report")).toHaveTextContent(text);
  });

  it.each([
    ["unknown", "Not connected"],
    ["unavailable", "Unavailable"],
  ] as const)("shows %s as text", async (state, text) => {
    override([{ displayName: "Odd", state }]);
    mount();
    expect((await rowOf("Odd")).getByText(text)).toBeInTheDocument();
  });

  it("marks a busy session and omits the cost when the desktop sends none", async () => {
    override([{ displayName: "Busy one", sessions: [{ sessionId: "s1", title: "Working", busy: true }] }]);
    mount();
    const row = await rowOf("Busy one");
    expect(row.getByText("Busy")).toBeInTheDocument();
    expect(row.queryByText(/estimate/)).toBeNull();
    expect(screen.queryByTestId("cloud-other")).toBeNull();
  });

  it("shows the empty state", async () => {
    override([]);
    mount();
    expect(await screen.findByTestId("cloud-empty")).toBeInTheDocument();
  });

  it("shows an error with a retry for the read", async () => {
    const user = userEvent.setup();
    FIXTURE_HANDLERS["cloud.instances"] = (() => {
      throw new FixtureError("boom");
    }) as never;
    mount();
    expect(await screen.findByText("Could not load the cloud instances.")).toBeInTheDocument();
    override([]);
    await user.click(screen.getByRole("button", { name: "Try again" }));
    expect(await screen.findByTestId("cloud-empty")).toBeInTheDocument();
  });

  it("shows the update notice when the desktop has no cloud capability", async () => {
    mount({ capabilities: ["approvals"] });
    expect(await screen.findByTestId("capability-notice")).toHaveTextContent("Update Monoptah on your desktop to use Cloud sessions");
    expect(screen.queryByTestId("cloud-instance")).toBeNull();
  });
});

describe("polling", () => {
  const base = { kind: "codespace", instance: "a", displayName: "A", sessions: [] } as const;
  it("polls modestly while something moves and stops when idle", () => {
    expect(instancesRefetchInterval({ instances: [{ ...base, state: "starting" }] } as never)).toBe(INSTANCES_POLL_MS);
    expect(instancesRefetchInterval({ instances: [{ ...base, state: "stopping" }] } as never)).toBe(INSTANCES_POLL_MS);
    expect(instancesRefetchInterval({ instances: [{ ...base, state: "available", stopReport: "stillRunning" }] } as never)).toBe(INSTANCES_POLL_MS);
    expect(instancesRefetchInterval({ instances: [{ ...base, state: "available" }, { ...base, state: "stopped" }] } as never)).toBe(false);
    expect(instancesRefetchInterval(undefined)).toBe(false);
    expect(INSTANCES_POLL_MS).toBeGreaterThanOrEqual(5_000);
  });

  it("formats last used in seconds or milliseconds", () => {
    const now = 1_700_000_000_000;
    expect(lastUsedLabel(now - 5 * 60_000, now)).toBe("Last used: 5 minutes ago");
    expect(lastUsedLabel((now - 3 * 3_600_000) / 1000, now)).toBe("Last used: 3 hours ago");
    expect(lastUsedLabel(null, now)).toBe("Last used: unknown");
  });
});

describe("stop", () => {
  it("confirms naming the sessions, sends kind and instance, then shows the stop report", async () => {
    const user = userEvent.setup();
    const calls: unknown[] = [];
    const real = FIXTURE_HANDLERS["cloud.stop"];
    FIXTURE_HANDLERS["cloud.stop"] = ((p: unknown, ctx: never) => {
      calls.push(p);
      return real(p as never, ctx);
    }) as never;
    mount();
    const row = await rowOf("Fake codespace one");
    await user.click(row.getByRole("button", { name: /Stop/ }));
    const dialog = await screen.findByRole("dialog");
    expect(within(dialog).getByText(/Refactor on the fake codespace/)).toBeInTheDocument();
    expect(calls).toHaveLength(0);
    await user.click(within(dialog).getByRole("button", { name: "Stop instance" }));
    await waitFor(() => expect(calls).toEqual([{ kind: "codespace", instance: "fake-codespace-one" }]));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    await waitFor(async () => expect((await rowOf("Fake codespace one")).getByTestId("stop-report")).toHaveTextContent("Stopping"));
  });

  it("flags busy sessions and shows the desktop warning afterwards", async () => {
    const user = userEvent.setup();
    override([{ displayName: "Busy host", instance: "b", sessions: [{ sessionId: "s1", title: "Long job", busy: true }, { sessionId: "s2", title: "Other job", busy: false }] }]);
    FIXTURE_HANDLERS["cloud.stop"] = (() => ({ stopped: true, warning: "Interrupted 1 busy session(s)" })) as never;
    mount();
    await user.click((await rowOf("Busy host")).getByRole("button", { name: /Stop/ }));
    const dialog = await screen.findByRole("dialog");
    expect(within(dialog).getByText(/"Long job" \(busy, will be interrupted\)/)).toBeInTheDocument();
    expect(within(dialog).getByText(/2 sessions that share it/)).toBeInTheDocument();
    await user.click(within(dialog).getByRole("button", { name: "Stop instance" }));
    expect(await screen.findByText("Interrupted 1 busy session(s)")).toBeInTheDocument();
  });

  it("explains instance_busy", async () => {
    const user = userEvent.setup();
    FIXTURE_HANDLERS["cloud.stop"] = (() => {
      throw new FixtureError("busy on desktop", "instance_busy");
    }) as never;
    mount();
    await user.click((await rowOf("Fake codespace one")).getByRole("button", { name: /Stop/ }));
    await user.click(within(await screen.findByRole("dialog")).getByRole("button", { name: "Stop instance" }));
    expect(await screen.findByText("The desktop is archiving or converting a session on this instance. Try again in a moment.")).toBeInTheDocument();
    expect(screen.getByRole("dialog")).toBeInTheDocument();
  });

  it("offers Stop for a kept (unavailable) instance and for stillRunning, not for stopped", async () => {
    override([
      { displayName: "Kept", instance: "k", state: "unavailable" },
      { displayName: "Still", instance: "t", state: "available", stopReport: "stillRunning" },
      { displayName: "Off", instance: "o", state: "stopped" },
    ]);
    mount();
    expect((await rowOf("Kept")).getByRole("button", { name: /Stop/ })).toBeEnabled();
    expect((await rowOf("Still")).getByRole("button", { name: /Stop/ })).toBeEnabled();
    expect((await rowOf("Off")).queryByRole("button", { name: /Stop/ })).toBeNull();
  });
});

describe("resume", () => {
  it("shows the cost notice, sends the first session id, then follows the refetch", async () => {
    const user = userEvent.setup();
    const calls: unknown[] = [];
    const real = FIXTURE_HANDLERS["cloud.resume"];
    FIXTURE_HANDLERS["cloud.resume"] = ((p: unknown, ctx: never) => {
      calls.push(p);
      return real(p as never, ctx);
    }) as never;
    mount();
    const row = await rowOf("Fake sandbox one");
    await user.click(row.getByRole("button", { name: /Resume/ }));
    const dialog = await screen.findByRole("dialog");
    expect(within(dialog).getByText(/bills while it runs/)).toBeInTheDocument();
    expect(within(dialog).getByText(/about \$0\.05\/hour \(estimate\)/)).toBeInTheDocument();
    expect(calls).toHaveLength(0);
    await user.click(within(dialog).getByRole("button", { name: "Resume and start billing" }));
    await waitFor(() => expect(calls).toEqual([{ sessionId: "sess-sandbox-stopped" }]));
    await waitFor(async () => expect((await rowOf("Fake sandbox one")).getByText("Starting")).toBeInTheDocument());
  });

  it("offers no Resume on a stopped instance without sessions", async () => {
    override([{ displayName: "Empty off", instance: "e", state: "stopped" }]);
    mount();
    expect((await rowOf("Empty off")).queryByRole("button", { name: /Resume/ })).toBeNull();
  });
});

describe("read-only device", () => {
  it("disables Stop and Resume and explains why", async () => {
    mount({ permission: "read" });
    const available = await rowOf("Fake codespace one");
    const stopped = await rowOf("Fake sandbox one");
    expect(available.getByRole("button", { name: /Stop/ })).toBeDisabled();
    expect(stopped.getByRole("button", { name: /Resume/ })).toBeDisabled();
    expect(stopped.getByText("This device has read-only access. Cloud actions need a device with control access.")).toBeInTheDocument();
  });
});

describe("forbidden controls", () => {
  it("has no delete, create, rebuild or open-in-browser control on the screen or in the dialogs", async () => {
    const user = userEvent.setup();
    mount();
    await rowOf("Fake codespace one");
    const forbidden = /delete|remove|create|new |rebuild|open in|browser|always|full.?access/i;
    const check = () => {
      for (const control of [...screen.queryAllByRole("button"), ...screen.queryAllByRole("link"), ...screen.queryAllByRole("menuitem")]) {
        const name = control.textContent ?? "";
        // Session titles are links to the session screen; control names are what is checked.
        if (control.tagName === "A") continue;
        expect(name).not.toMatch(forbidden);
      }
    };
    check();
    await user.click((await rowOf("Fake codespace one")).getByRole("button", { name: /Stop/ }));
    await screen.findByRole("dialog");
    check();
  });
});

describe("polling window and backoff (SPEC I-4, BILL M-8)", () => {
  const moving = { instances: [{ kind: "codespace", instance: "a", displayName: "A", sessions: [], state: "starting" }] } as never;
  const idle = { instances: [{ kind: "codespace", instance: "a", displayName: "A", sessions: [], state: "stopped" }] } as never;

  it("polls an idle-looking list while this device's own action is watched", () => {
    expect(instancesRefetchInterval(idle)).toBe(false);
    expect(instancesRefetchInterval(idle, { watching: true })).toBe(INSTANCES_POLL_MS);
  });

  it("backs off after the fast window and stops at the cap", () => {
    expect(instancesRefetchInterval(moving, { elapsedMs: 0 })).toBe(INSTANCES_POLL_MS);
    expect(instancesRefetchInterval(moving, { elapsedMs: INSTANCES_FAST_WINDOW_MS - 1 })).toBe(INSTANCES_POLL_MS);
    expect(instancesRefetchInterval(moving, { elapsedMs: INSTANCES_FAST_WINDOW_MS })).toBe(INSTANCES_SLOW_POLL_MS);
    expect(instancesRefetchInterval(moving, { elapsedMs: INSTANCES_POLL_CAP_MS })).toBe(false);
    expect(OWN_ACTION_WATCH_MS).toBeGreaterThanOrEqual(120_000);
    expect(OWN_ACTION_WATCH_MS).toBeLessThanOrEqual(180_000);
  });
});

describe("pickResumeSession (SPEC I-4)", () => {
  const instance = (sessions: { sessionId: string; busy: boolean }[]) => ({ kind: "sandbox", instance: "i", displayName: "I", state: "stopped", sessions: sessions.map((s) => ({ ...s, title: s.sessionId })) }) as never;

  it("prefers an idle session that the catalog still lists", () => {
    const picked = pickResumeSession(instance([{ sessionId: "gone", busy: false }, { sessionId: "busy", busy: true }, { sessionId: "listed", busy: false }]), new Set(["busy", "listed"]));
    expect(picked).toBe("listed");
  });

  it("falls back to an idle session, then to the first", () => {
    expect(pickResumeSession(instance([{ sessionId: "busy", busy: true }, { sessionId: "idle", busy: false }]), undefined)).toBe("idle");
    expect(pickResumeSession(instance([{ sessionId: "a", busy: true }, { sessionId: "b", busy: true }]), new Set())).toBe("a");
    expect(pickResumeSession(instance([]), undefined)).toBeUndefined();
  });
});

describe("rows: labels, untrusted text, resume session and own actions", () => {
  it("titles the Stop dialog with the human label", async () => {
    const user = userEvent.setup();
    mount();
    const row = await rowOf("Fake codespace one");
    await user.click(row.getByRole("button", { name: /Stop/ }));
    expect(await screen.findByRole("dialog", { name: "Stop GitHub codespace?" })).toBeInTheDocument();
  });

  it("caps names and removes bidi controls in the row, the session links and the dialog", async () => {
    const user = userEvent.setup();
    const evil = `prod‮${"z".repeat(300)}`;
    override([{ kind: "codespace", instance: "e", displayName: evil, state: "available", sessions: [{ sessionId: "s1", title: `‮title ${"q".repeat(300)}`, busy: false }] }]);
    mount();
    const heading = await screen.findByRole("heading", { level: 2 });
    expect(heading.textContent).not.toContain("‮");
    expect(heading.textContent!.length).toBeLessThanOrEqual(64);
    const row = within(heading.closest("li") as HTMLElement);
    expect(row.getByRole("link").textContent!.length).toBeLessThanOrEqual(64);
    await user.click(row.getByRole("button", { name: /Stop/ }));
    const dialog = await screen.findByRole("dialog");
    expect(dialog.textContent).not.toContain("‮");
    expect(dialog.textContent).not.toContain("z".repeat(100));
  });

  it("resumes with an idle session, not blindly the first", async () => {
    const user = userEvent.setup();
    const calls: unknown[] = [];
    FIXTURE_HANDLERS["cloud.resume"] = ((p: unknown) => {
      calls.push(p);
      return { accepted: true };
    }) as never;
    override([
      {
        kind: "sandbox",
        instance: "r",
        displayName: "Resumable",
        state: "stopped",
        sessions: [
          { sessionId: "first-busy", title: "A", busy: true },
          { sessionId: "second-idle", title: "B", busy: false },
        ],
      },
    ]);
    mount();
    const row = await rowOf("Resumable");
    await user.click(row.getByRole("button", { name: /Resume/ }));
    await user.click(within(await screen.findByRole("dialog")).getByRole("button", { name: "Resume and start billing" }));
    await waitFor(() => expect(calls).toEqual([{ sessionId: "second-idle" }]));
  });

  it("after its own Resume the row waits, polls on, and the action stays off until the list shows the change", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    try {
      const user = userEvent.setup({ advanceTimers: (ms) => vi.advanceTimersByTime(ms) });
      let resumed = false;
      let readsAfter = 0;
      const resumeCalls = vi.fn();
      FIXTURE_HANDLERS["cloud.resume"] = (() => {
        resumed = true;
        resumeCalls();
        return { accepted: true };
      }) as never;
      FIXTURE_HANDLERS["cloud.instances"] = (() => {
        if (resumed) readsAfter += 1;
        const state = resumed && readsAfter > 2 ? "starting" : "stopped";
        return { instances: [{ kind: "sandbox", instance: "r", displayName: "Resumable", state, sessions: [{ sessionId: "s1", title: "S", busy: false }] }] };
      }) as never;
      mount();
      const row = await rowOf("Resumable");
      await user.click(row.getByRole("button", { name: /Resume/ }));
      await user.click(within(await screen.findByRole("dialog")).getByRole("button", { name: "Resume and start billing" }));
      await waitFor(() => expect(resumeCalls).toHaveBeenCalledTimes(1));
      // The refetch right after the action still says stopped: Resume is off and the row says it is waiting.
      await waitFor(async () => expect((await rowOf("Resumable")).getByTestId("own-action-note")).toBeInTheDocument());
      expect((await rowOf("Resumable")).getByRole("button", { name: /Resume/ })).toBeDisabled();
      // The poll keeps going until the list shows the change.
      await act(async () => void (await vi.advanceTimersByTimeAsync(INSTANCES_POLL_MS * 3 + 500)));
      await waitFor(async () => expect((await rowOf("Resumable")).getByText("Starting")).toBeInTheDocument());
      expect((await rowOf("Resumable")).queryByTestId("own-action-note")).toBeNull();
      expect(resumeCalls).toHaveBeenCalledTimes(1);
    } finally {
      vi.useRealTimers();
    }
  });
});
