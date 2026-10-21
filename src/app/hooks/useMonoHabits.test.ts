// @vitest-environment happy-dom
import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Habit } from "../../features/monos/model/monoHabits";
import type { Block, Session } from "../../features/sessions/model/session";

let stored: Habit[] = [];
let fileVersion = 0;
const MONO = {
  id: "mono-1",
  sessionId: "mono",
  mascot: "skull",
  color: "#fff",
  projects: ["/code/app"],
};

vi.mock("../../features/monos/model/mono", async (original) => ({
  ...(await original<object>()),
  listMonos: () => [MONO],
  findMono: (id: string) => (id === MONO.id ? MONO : undefined),
  monoLook: () => ({
    name: "Skull",
    mascot: "skull",
    color: "#fff",
    projects: [{ path: "/code/app", name: "app" }],
  }),
}));
vi.mock("../../features/monos/model/monoFiles", async (original) => {
  const actual =
    await original<typeof import("../../features/monos/model/monoFiles")>();
  return {
    ...actual,
    loadMonoFiles: vi.fn(async () => ({
      id: "a",
      dir: "/data/monos/a",
      soul: "Be brief.",
      soulHash: "s",
      memory: "",
      memoryHash: "m",
      memoryPath: "/data/monos/a/MEMORY.md",
      topics: [],
    })),
    readAgentFile: vi.fn(async () => ({
      text: JSON.stringify(stored),
      hash: String(fileVersion),
    })),
    // Model the native store's atomic compare-and-save, including conflicts
    // between independent window schedulers.
    writeAgentFile: vi.fn(async (_monoId, _path, text, expectedHash) => {
      if (expectedHash !== String(fileVersion))
        throw new actual.MonoFileConflict();
      stored = JSON.parse(text) as Habit[];
      return String(++fileVersion);
    }),
  };
});

const { useMonoHabits } = await import("./useMonoHabits");
const { shouldPersistSession } =
  await import("../../features/sessions/data/sessionStore");
const {
  checkHabitsNow,
  clearHabitStop,
  habitStopRequest,
  noteHabitRunLaunch,
  relayApproval,
  requestHabitStop,
} = await import("../../features/monos/model/monoHabits");
const { loadMonoFiles } = await import("../../features/monos/model/monoFiles");

let root: Root;
let extraRoots: Root[] = [];
const originalWindow = window;
beforeEach(() => {
  fileVersion = 0;
  extraRoots = [];
  vi.clearAllMocks();
  clearHabitStop("h1");
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  const values = new Map<string, string>();
  vi.stubGlobal("localStorage", {
    getItem: (key: string) => values.get(key) ?? null,
    setItem: (key: string, value: string) => values.set(key, value),
  });
  root = createRoot(document.createElement("div"));
});
afterEach(() => {
  vi.stubGlobal("window", originalWindow);
  act(() => root.unmount());
  for (const extra of extraRoots) act(() => extra.unmount());
  vi.unstubAllGlobals();
});

function setup(
  reply: string,
  approve?: { wait: Promise<void> },
  waitingHabit = false,
) {
  stored = [
    {
      id: "h1",
      name: "Morning CI check",
      instructions: "Tell me if CI is red.",
      schedule: {
        scheduleKind: "daily",
        time: "09:00",
        minute: 0,
        dayOfWeek: 1,
      },
      enabled: true,
      createdAt: 0,
      nextRunAt: Date.now() - 60_000,
    },
  ];
  if (waitingHabit)
    stored.push({ ...stored[0], id: "h2", name: "Waiting habit" });
  const host = makeHost(reply, approve);
  mountHost(host);
  return host;
}

function makeHost(reply: string, approve?: { wait: Promise<void> }) {
  const mono = {
    id: "mono",
    cwd: "/code/app",
    harness: "claude",
    model: "claude:x",
    blocks: [],
  } as unknown as Session;
  const sessions: Session[] = [mono];
  const host = {
    sessions: () => sessions,
    open: vi.fn(async () => mono),
    add: vi.fn((session: Session) => sessions.push(session)),
    run: vi.fn(async (id: string, prompt: string) => {
      const run = sessions.find((session) => session.id === id)!;
      if (approve) {
        run.blocks = [
          { id: "u", role: "user", text: prompt },
          {
            id: "ask",
            role: "approval",
            text: "",
            tool: { title: "git push" },
            approval: { requestId: 7 },
          },
        ];
        await approve.wait;
        run.blocks[1] = {
          ...run.blocks[1],
          approval: { requestId: 7, decided: "allow" },
        };
      }
      run.blocks = [
        ...run.blocks,
        { id: "u", role: "user", text: prompt },
        {
          id: "t",
          role: "assistant",
          text: "",
          tool: { title: "gh run list" },
        },
        { id: "a", role: "assistant", text: reply },
      ];
      return { status: "completed" as const, text: reply };
    }),
    remove: vi.fn(async () => {}),
    stop: vi.fn(async () => {}),
    post: vi.fn(),
    // Like the app: the request is relayed, so later polls skip it.
    askApproval: vi.fn(
      (monoId: string, _habit: Habit, runId: string, block: Block) =>
        relayApproval({
          monoId,
          runId,
          requestId: block.approval!.requestId,
          blockId: "b",
        }),
    ),
    endApprovals: vi.fn(),
  };
  return host;
}

function mountHost(
  host: ReturnType<typeof makeHost>,
  target = root,
  surface = originalWindow,
) {
  function Probe() {
    useMonoHabits(host);
    return null;
  }
  vi.stubGlobal("window", surface);
  act(() => target.render(createElement(Probe)));
  vi.stubGlobal("window", originalWindow);
}

/** Separate native windows have separate owner slots and share the file store. */
function isolatedWindow(): Window {
  let owner: unknown;
  return new Proxy(originalWindow, {
    get(target, key) {
      if (key === "__monoHabitsScheduler") return owner;
      const value = Reflect.get(target, key);
      return typeof value === "function" ? value.bind(target) : value;
    },
    set(target, key, value) {
      if (key === "__monoHabitsScheduler") {
        owner = value;
        return true;
      }
      return Reflect.set(target, key, value);
    },
    deleteProperty(target, key) {
      if (key === "__monoHabitsScheduler") {
        owner = undefined;
        return true;
      }
      return Reflect.deleteProperty(target, key);
    },
  });
}

function deferred() {
  let resolve!: () => void;
  const promise = new Promise<void>((done) => (resolve = done));
  return { promise, resolve };
}

it("runs a due habit in a hidden session and posts what it found", async () => {
  const host = setup("CI is red on main since #712.");
  await vi.waitFor(() => expect(host.post).toHaveBeenCalled());
  const [monoId, habit, text, name] = host.post.mock.calls[0];
  expect([monoId, habit.id, text, name]).toEqual([
    "mono",
    "h1",
    "CI is red on main since #712.",
    "Skull",
  ]);
  const prompt = host.run.mock.calls[0][1] as string;
  expect(prompt.startsWith("/operator ")).toBe(true);
  expect(prompt).toContain("Habit: Morning CI check");
  const runId = host.add.mock.calls[0][0].id;
  // Nobody watches a habit's run, so it never stops to ask before each step.
  expect(host.add.mock.calls[0][0].runtimeMode).toBe("full-access");
  // Never saved, so it can never show up with the project's chats.
  expect(host.add.mock.calls[0][0].ephemeral).toBe(true);
  expect(shouldPersistSession(host.add.mock.calls[0][0])).toBe(false);
  expect(host.endApprovals).toHaveBeenCalledWith(runId);
  await vi.waitFor(() => expect(stored[0].lastOutcome).toBe("posted"));
  expect(stored[0].runs?.[0]).toMatchObject({
    outcome: "posted",
    report: "CI is red on main since #712.",
  });
  expect(host.remove).toHaveBeenCalledWith(runId);
  expect(stored[0].nextRunAt).toBeGreaterThan(Date.now());
});

it("posts nothing when the run has nothing to say", async () => {
  const host = setup("NOTHING_TO_REPORT");
  await vi.waitFor(() => expect(stored[0].lastOutcome).toBe("quiet"));
  expect(host.post).not.toHaveBeenCalled();
  expect(host.remove).toHaveBeenCalled();
});

it("puts the sessions a run started on its report", async () => {
  const host = setup("Started #910 in a background session.");
  const run = host.run.getMockImplementation()!;
  const launch = {
    sessionId: "app-910",
    cwd: "/code/app",
    title: "Fix #910",
    harness: "claude" as const,
    model: "claude:x",
  };
  host.run.mockImplementation(async (id: string, prompt: string) => {
    noteHabitRunLaunch(id, launch);
    // A retried request must not list it twice.
    noteHabitRunLaunch(id, launch);
    return run(id, prompt);
  });
  await vi.waitFor(() => expect(host.post).toHaveBeenCalled());
  expect(host.post.mock.calls[0][2]).toBe(
    "Started #910 in a background session.",
  );
  expect(host.post.mock.calls[0][5]).toEqual([launch]);
});

it("posts the sessions a quiet run started", async () => {
  const host = setup("NOTHING_TO_REPORT");
  const run = host.run.getMockImplementation()!;
  host.run.mockImplementation(async (id: string, prompt: string) => {
    noteHabitRunLaunch(id, {
      sessionId: "app-1",
      cwd: "/code/app",
      title: "Look into CI",
      harness: "claude",
      model: "claude:x",
    });
    return run(id, prompt);
  });
  await vi.waitFor(() => expect(host.post).toHaveBeenCalled());
  expect(host.post.mock.calls[0][2]).toBe("Started a session.");
  expect(host.post.mock.calls[0][5]).toHaveLength(1);
});

it("lets a long-running habit finish and post after thirty minutes", async () => {
  vi.useFakeTimers();
  const hold = deferred();
  const host = setup("PR review complete.");
  const run = host.run.getMockImplementation()!;
  host.run.mockImplementationOnce(async (id, prompt) => {
    await hold.promise;
    return run(id, prompt);
  });
  try {
    await act(async () => {
      await vi.advanceTimersByTimeAsync(30 * 60 * 1000);
    });
    expect(host.run).toHaveBeenCalledTimes(1);
    expect(host.remove).not.toHaveBeenCalled();
    expect(stored[0].lastOutcome).toBeUndefined();

    await act(async () => hold.resolve());
    expect(host.post).toHaveBeenCalledWith(
      "mono",
      expect.objectContaining({ id: "h1" }),
      "PR review complete.",
      "Skull",
      [],
      [],
    );
    expect(stored[0].lastOutcome).toBe("posted");
    expect(host.remove).toHaveBeenCalledTimes(1);
  } finally {
    await act(async () => hold.resolve());
    vi.useRealTimers();
  }
});

it("stops and records a habit that is still running after one hour", async () => {
  vi.useFakeTimers();
  const hold = deferred();
  const host = setup("PR review complete.");
  host.run.mockImplementationOnce(async () => {
    await hold.promise;
    return { status: "completed", text: "PR review complete." };
  });
  try {
    await act(async () => {
      await vi.advanceTimersByTimeAsync(60 * 60 * 1000 - 2_000);
    });
    expect(host.run).toHaveBeenCalledTimes(1);
    expect(host.remove).not.toHaveBeenCalled();

    await act(async () => {
      await vi.advanceTimersByTimeAsync(2_000);
    });
    expect(host.remove).toHaveBeenCalledTimes(1);
    expect(host.post).not.toHaveBeenCalled();
    expect(stored[0]).toMatchObject({
      lastOutcome: "failed",
      lastError: "It took too long and was stopped.",
    });
  } finally {
    await act(async () => hold.resolve());
    vi.useRealTimers();
  }
});

it(
  "puts an approval the run still asks for to the user, once",
  { timeout: 15_000 },
  async () => {
    let allow!: () => void;
    const wait = new Promise<void>((resolve) => (allow = resolve));
    const host = setup("Pushed the fix.", { wait });
    await vi.waitFor(() => expect(host.askApproval).toHaveBeenCalled(), {
      timeout: 5_000,
    });
    const [monoId, habit, runId, block] = host.askApproval.mock.calls[0];
    expect([monoId, habit.id, block.approval.requestId]).toEqual([
      "mono",
      "h1",
      7,
    ]);
    expect(runId).toBe(host.add.mock.calls[0][0].id);
    // Still waiting on the user through the next poll: not asked again.
    await new Promise((resolve) => setTimeout(resolve, 2_500));
    allow();
    await vi.waitFor(() => expect(host.post).toHaveBeenCalled(), {
      timeout: 5_000,
    });
    expect(host.askApproval).toHaveBeenCalledTimes(1);
  },
);

it("starts a habit asked to run now without waiting for the next minute", async () => {
  const host = setup("CI is red.");
  await vi.waitFor(() => expect(stored[0].lastOutcome).toBe("posted"));
  const scheduled = stored[0].nextRunAt;
  stored = [{ ...stored[0], runRequested: true }];
  checkHabitsNow();
  await vi.waitFor(() => expect(host.run).toHaveBeenCalledTimes(2));
  await vi.waitFor(() => expect(stored[0].runs).toHaveLength(2));
  expect(stored[0].runRequested).toBeUndefined();
  expect(stored[0].nextRunAt).toBe(scheduled);
});

it("launches a due habit only once across independent window schedulers", async () => {
  const first = setup("NOTHING_TO_REPORT");
  const second = makeHost("NOTHING_TO_REPORT");
  const extra = createRoot(document.createElement("div"));
  extraRoots.push(extra);
  mountHost(second, extra, isolatedWindow());
  await vi.waitFor(() => expect(stored[0].lastOutcome).toBe("quiet"));
  await act(async () => {});
  expect(first.run.mock.calls.length + second.run.mock.calls.length).toBe(1);
  expect(stored[0].runs).toHaveLength(1);
});

it.each(["removed", "paused", "rescheduled"] as const)(
  "does not launch a waiting habit that was %s during an earlier run",
  async (change) => {
    const hold = deferred();
    const host = setup("NOTHING_TO_REPORT", undefined, true);
    host.run.mockImplementationOnce(async () => {
      await hold.promise;
      return { status: "completed", text: "NOTHING_TO_REPORT" };
    });
    try {
      await vi.waitFor(() => expect(host.run).toHaveBeenCalledTimes(1));
      stored = stored.flatMap((habit) => {
        if (habit.id !== "h2") return [habit];
        if (change === "removed") return [];
        return [
          {
            ...habit,
            ...(change === "paused"
              ? { enabled: false }
              : { nextRunAt: Date.now() + 60_000 }),
          },
        ];
      });
      fileVersion++;
    } finally {
      hold.resolve();
    }
    await vi.waitFor(() => expect(stored[0].lastOutcome).toBe("quiet"));
    await act(async () => {});
    expect(host.run).toHaveBeenCalledTimes(1);
  },
);

it("rechecks a habit removed while its files were being prepared", async () => {
  const hold = deferred();
  const load = vi.mocked(loadMonoFiles);
  const files = await load(MONO.id);
  load.mockImplementationOnce(async () => {
    await hold.promise;
    return files;
  });
  const host = setup("NOTHING_TO_REPORT");
  try {
    await vi.waitFor(() => expect(load).toHaveBeenCalledTimes(2));
    stored = [];
    fileVersion++;
  } finally {
    hold.resolve();
  }
  await act(async () => {});
  expect(host.run).not.toHaveBeenCalled();
  expect(host.add).not.toHaveBeenCalled();
});

it("uses a waiting habit's latest instructions when it launches", async () => {
  const hold = deferred();
  const host = setup("NOTHING_TO_REPORT", undefined, true);
  host.run.mockImplementationOnce(async () => {
    await hold.promise;
    return { status: "completed", text: "NOTHING_TO_REPORT" };
  });
  try {
    await vi.waitFor(() => expect(host.run).toHaveBeenCalledTimes(1));
    stored = stored.map((habit) =>
      habit.id === "h2"
        ? { ...habit, instructions: "Inspect only the release branch." }
        : habit,
    );
    fileVersion++;
  } finally {
    hold.resolve();
  }
  await vi.waitFor(() => expect(host.run).toHaveBeenCalledTimes(2));
  expect(host.run.mock.calls[1][1]).toContain(
    "Inspect only the release branch.",
  );
  await vi.waitFor(() => expect(stored[1].lastOutcome).toBe("quiet"));
});

describe("stopping a running habit", () => {
  const launch = (sessionId: string) => ({
    sessionId,
    cwd: "/code/app",
    title: `Work on ${sessionId}`,
    harness: "claude" as const,
    model: "claude:x",
  });

  /** A run that has started the given sessions and then never finishes. */
  async function startHanging(sessionIds: string[], messaged: string[] = []) {
    vi.useFakeTimers();
    const host = setup("never reported");
    host.run.mockImplementationOnce(async (id: string) => {
      for (const sessionId of messaged)
        noteHabitRunLaunch(id, launch(sessionId));
      for (const sessionId of sessionIds)
        noteHabitRunLaunch(id, launch(sessionId), true);
      return new Promise<never>(() => {});
    });
    await act(async () => {
      await vi.advanceTimersByTimeAsync(2_000);
    });
    expect(host.run).toHaveBeenCalledTimes(1);
    return host;
  }

  async function stopWith(mode: "run" | "all") {
    requestHabitStop("h1", mode);
    await act(async () => {
      await vi.advanceTimersByTimeAsync(2_000);
    });
  }

  afterEach(() => {
    vi.useRealTimers();
  });

  it("ends the run, keeps its sessions and says where they are", async () => {
    const host = await startHanging(["app-1", "app-2"]);
    await stopWith("run");
    expect(host.remove).toHaveBeenCalledTimes(1);
    expect(host.stop).not.toHaveBeenCalled();
    expect(host.post).toHaveBeenCalledTimes(1);
    const [monoId, habit, text, , , launches] = host.post.mock.calls[0];
    expect([monoId, habit.id]).toEqual(["mono", "h1"]);
    expect(text).toBe("Stopped. Started 2 sessions that keep running.");
    expect(
      launches.map((entry: { sessionId: string }) => entry.sessionId),
    ).toEqual(["app-1", "app-2"]);
    expect(stored[0]).toMatchObject({
      lastOutcome: "failed",
      lastError: "Stopped by the user.",
    });
    expect(habitStopRequest("h1")).toBeUndefined();
  });

  it("posts nothing when it stops a run that started no sessions", async () => {
    const host = await startHanging([]);
    await stopWith("run");
    expect(host.remove).toHaveBeenCalledTimes(1);
    expect(host.stop).not.toHaveBeenCalled();
    expect(host.post).not.toHaveBeenCalled();
    expect(stored[0]).toMatchObject({
      lastOutcome: "failed",
      lastError: "Stopped by the user.",
    });
    expect(habitStopRequest("h1")).toBeUndefined();
  });

  it("names a single kept session in the singular", async () => {
    const host = await startHanging(["app-1"]);
    await stopWith("run");
    expect(host.post.mock.calls[0][2]).toBe(
      "Stopped. Started 1 session that keeps running.",
    );
  });

  it("stops the sessions it started too, and says which would not stop", async () => {
    const host = await startHanging(["app-1", "app-2", "app-3"]);
    host.stop.mockImplementation(async (sessionId: string) => {
      if (sessionId === "app-2") throw new Error("already gone");
    });
    await stopWith("all");
    expect(host.remove).toHaveBeenCalledTimes(1);
    expect(host.stop.mock.calls).toEqual([
      ["app-1", "mono"],
      ["app-2", "mono"],
      ["app-3", "mono"],
    ]);
    // Only the one it could not stop is left to find.
    expect(host.post).toHaveBeenCalledTimes(1);
    const [, , text, , , launches] = host.post.mock.calls[0];
    expect(text).toBe("Could not stop 1 session: app-2.");
    expect(
      launches.map((entry: { sessionId: string }) => entry.sessionId),
    ).toEqual(["app-2"]);
    expect(stored[0]).toMatchObject({
      lastOutcome: "failed",
      lastError: "Stopped by the user.",
    });
    expect(habitStopRequest("h1")).toBeUndefined();
  });

  it("stops only the sessions the run started, not ones it messaged", async () => {
    const host = await startHanging(["app-1"], ["reviewer"]);
    await stopWith("all");
    expect(host.stop.mock.calls).toEqual([["app-1", "mono"]]);
    expect(host.post).not.toHaveBeenCalled();
  });

  it("does not count a session it only messaged as one that keeps running", async () => {
    const host = await startHanging(["app-1"], ["reviewer"]);
    await stopWith("run");
    expect(host.stop).not.toHaveBeenCalled();
    expect(host.post.mock.calls[0][2]).toBe(
      "Stopped. Started 1 session that keeps running.",
    );
  });

  it("stops the sessions even when the run finished as the user chose", async () => {
    vi.useFakeTimers();
    const host = setup("CI is red.");
    const run = host.run.getMockImplementation()!;
    host.run.mockImplementationOnce(async (id, prompt) => {
      noteHabitRunLaunch(id, launch("app-1"), true);
      requestHabitStop("h1", "all");
      return run(id, prompt);
    });
    await act(async () => {
      await vi.advanceTimersByTimeAsync(2_000);
    });
    expect(host.stop.mock.calls).toEqual([["app-1", "mono"]]);
    // It finished by itself, so its findings still reach the user.
    expect(host.post.mock.calls[0][2]).toBe("CI is red.");
    expect(stored[0].lastOutcome).toBe("posted");
    expect(habitStopRequest("h1")).toBeUndefined();
  });

  it("lists a session started while the run is being removed", async () => {
    vi.useFakeTimers();
    const host = setup("NOTHING_TO_REPORT");
    host.remove.mockImplementationOnce(async (id: string) => {
      noteHabitRunLaunch(id, launch("late-1"), true);
    });
    await act(async () => {
      await vi.advanceTimersByTimeAsync(2_000);
    });
    expect(host.post.mock.calls[0][2]).toBe("Started a session.");
    expect(host.post.mock.calls[0][5]).toEqual([launch("late-1")]);
  });

  it("ignores a stop request left over from before the run started", async () => {
    vi.useFakeTimers();
    requestHabitStop("h1", "all");
    const host = setup("CI is red.");
    const run = host.run.getMockImplementation()!;
    // Long enough for the run to be polled, where a stale request would bite.
    host.run.mockImplementationOnce(async (id, prompt) => {
      await new Promise((resolve) => setTimeout(resolve, 5_000));
      return run(id, prompt);
    });
    await act(async () => {
      await vi.advanceTimersByTimeAsync(10_000);
    });
    expect(host.post.mock.calls[0][2]).toBe("CI is red.");
    expect(host.stop).not.toHaveBeenCalled();
    expect(stored[0].lastOutcome).toBe("posted");
    expect(habitStopRequest("h1")).toBeUndefined();
  });
});
