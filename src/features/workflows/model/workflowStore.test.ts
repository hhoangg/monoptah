// @vitest-environment happy-dom
import { beforeEach, expect, it, vi } from "vitest";
import type { Workflow, WorkflowRun } from "./workflow";

const native = vi.hoisted(() => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({
  invoke: native.invoke,
  isTauri: () => true,
}));

import {
  checkWorkflowsNow,
  clearWorkflowStop,
  loadRuns,
  loadWorkflows,
  requestWorkflowStop,
  setWorkflowRunning,
  subscribeCheckWorkflowsNow,
  subscribeWorkflowsRunning,
  updateRuns,
  updateWorkflows,
  WORKFLOWS_FILE_BUDGET,
  workflowRunningId,
  workflowStopRequested,
} from "./workflowStore";

const workflow = (patch: Partial<Workflow> = {}): Workflow => ({
  id: "wf-abcd1234",
  name: "Daily",
  inputs: {},
  inputValues: {},
  nodes: [
    { id: "trigger", type: "manual" },
    { id: "report", type: "report" },
  ],
  edges: [{ from: "trigger", to: "report" }],
  enabled: true,
  createdAt: 1,
  ...patch,
});

const run = (id: string): WorkflowRun => ({
  id,
  workflowId: "wf-abcd1234",
  definition: workflow(),
  status: "done",
  startedAt: 1,
  trigger: "manual",
  items: {},
  nodes: [],
});

beforeEach(() => {
  native.invoke.mockReset();
});

it("updateWorkflows retries once on conflict", async () => {
  let reads = 0;
  const saved: { path: string; text: string; expectedHash: string }[] = [];
  native.invoke.mockImplementation(async (command: string, args: any) => {
    if (command === "mono_read") {
      reads++;
      return { text: JSON.stringify([workflow()]), hash: `h${reads}` };
    }
    saved.push(args);
    if (saved.length === 1) throw "conflict";
    return "new-hash";
  });

  const result = await updateWorkflows("mono-1", (list) => ({
    workflows: [...list, workflow({ id: "wf-zzzz9999", name: "Second" })],
    result: list.length,
  }));

  expect(result).toBe(1);
  expect(reads).toBe(2);
  expect(saved.map((s) => s.expectedHash)).toEqual(["h1", "h2"]);
  expect(saved[1].path).toBe("workflows.json");
  expect(JSON.parse(saved[1].text).map((w: Workflow) => w.id)).toEqual([
    "wf-abcd1234",
    "wf-zzzz9999",
  ]);
  expect(await loadWorkflows("mono-1")).toHaveLength(1);
});

it("updateWorkflows gives up after repeated conflicts", async () => {
  native.invoke.mockImplementation(async (command: string) => {
    if (command === "mono_read") return { text: null, hash: "h" };
    throw "conflict";
  });
  await expect(
    updateWorkflows("mono-1", (list) => ({ workflows: list, result: 0 })),
  ).rejects.toThrow("changed");
  expect(
    native.invoke.mock.calls.filter(([command]) => command === "mono_save"),
  ).toHaveLength(3);
});

it("updateRuns keeps newest 20", async () => {
  const existing = Array.from({ length: 19 }, (_, i) => run(`old${i}`));
  let written = "";
  let path = "";
  native.invoke.mockImplementation(async (command: string, args: any) => {
    if (command === "mono_read") {
      path = args.path;
      return { text: JSON.stringify(existing), hash: "h" };
    }
    written = args.text;
    path = args.path;
    return "new-hash";
  });

  await updateRuns("mono-1", "wf-abcd1234", (runs) => ({
    runs: [run("new1"), run("new2"), run("new3"), ...runs],
    result: undefined,
  }));

  const kept = JSON.parse(written) as WorkflowRun[];
  expect(path).toBe("workflow-runs/wf-abcd1234.json");
  expect(kept).toHaveLength(20);
  expect(kept.slice(0, 3).map((r) => r.id)).toEqual(["new1", "new2", "new3"]);
  expect(kept[19].id).toBe("old16");
});

it("updateRuns drops the oldest runs when the file grows too large", async () => {
  const fat = (id: string): WorkflowRun => ({
    ...run(id),
    nodes: [
      {
        nodeId: "fetch",
        branch: "",
        status: "done",
        round: 1,
        output: { stdout: "x".repeat(1024 * 1024) },
      },
    ],
  });
  let written = "";
  native.invoke.mockImplementation(async (command: string, args: any) => {
    if (command === "mono_read") return { text: null, hash: "h" };
    written = args.text;
    return "new-hash";
  });

  await updateRuns("mono-1", "wf-abcd1234", () => ({
    runs: ["r1", "r2", "r3", "r4", "r5"].map(fat),
    result: undefined,
  }));

  const kept = JSON.parse(written) as WorkflowRun[];
  expect(kept.map((r) => r.id)).toEqual(["r1", "r2", "r3"]);
  expect(written.length).toBeLessThanOrEqual(3.5 * 1024 * 1024);

  // The newest run is kept even when it alone is over the budget.
  await updateRuns("mono-1", "wf-abcd1234", () => ({
    runs: [
      {
        ...run("only"),
        nodes: [
          {
            nodeId: "fetch",
            branch: "",
            status: "done",
            round: 1,
            output: { stdout: "x".repeat(4 * 1024 * 1024) },
          },
        ],
      },
      fat("older"),
    ],
    result: undefined,
  }));
  expect((JSON.parse(written) as WorkflowRun[]).map((r) => r.id)).toEqual([
    "only",
  ]);
});

it("loadRuns returns [] for a missing file", async () => {
  native.invoke.mockResolvedValue({ text: null, hash: "empty" });
  expect(await loadRuns("mono-1", "wf-abcd1234")).toEqual([]);
  expect(native.invoke).toHaveBeenCalledWith(
    "mono_read",
    expect.objectContaining({ path: "workflow-runs/wf-abcd1234.json" }),
  );
});

it("loadRuns drops malformed entries and unreadable files", async () => {
  native.invoke.mockResolvedValueOnce({
    text: JSON.stringify([run("ok"), { id: "x" }, 3, null]),
    hash: "h",
  });
  expect((await loadRuns("mono-1", "wf-abcd1234")).map((r) => r.id)).toEqual([
    "ok",
  ]);
  native.invoke.mockResolvedValueOnce({ text: "not json", hash: "h" });
  expect(await loadRuns("mono-1", "wf-abcd1234")).toEqual([]);
});

it("loadRuns keeps per-round history, drops malformed records and the output of old records", async () => {
  const history = [
    {
      round: 1,
      status: "done",
      sessionId: "s1",
      decision: { value: "loop", reason: "needs tests" },
      startedAt: 1,
      endedAt: 2,
    },
  ];
  // Runs saved before a round stopped keeping its output still carry it.
  const old = [{ ...history[0], output: { pr_url: "u1" } }];
  const withHistory = run("with");
  withHistory.nodes = [
    { nodeId: "dev", branch: "", status: "pending", round: 2, history: old } as WorkflowRun["nodes"][number],
    {
      nodeId: "review",
      branch: "",
      status: "pending",
      round: 3,
      history: [old[0], { round: "x", status: "done" }, { round: 2, status: "nope" }, 7, null],
    } as unknown as WorkflowRun["nodes"][number],
  ];
  const without = run("without");
  without.nodes = [{ nodeId: "dev", branch: "", status: "done", round: 1 }];
  native.invoke.mockResolvedValueOnce({
    text: JSON.stringify([withHistory, without]),
    hash: "h",
  });
  const [a, b] = await loadRuns("mono-1", "wf-abcd1234");
  expect(a.nodes[0].history).toEqual(history);
  expect(a.nodes[1].history).toEqual(history);
  expect(b.nodes[0].history).toBeUndefined();
  expect("history" in b.nodes[0]).toBe(false);
});

it("tracks running workflows and notifies subscribers", () => {
  const listener = vi.fn();
  const off = subscribeWorkflowsRunning(listener);
  setWorkflowRunning("wf-abcd1234", "run-1");
  expect(workflowRunningId("wf-abcd1234")).toBe("run-1");
  setWorkflowRunning("wf-abcd1234", null);
  expect(workflowRunningId("wf-abcd1234")).toBeUndefined();
  expect(listener).toHaveBeenCalledTimes(2);
  off();
  setWorkflowRunning("wf-abcd1234", "run-2");
  expect(listener).toHaveBeenCalledTimes(2);
  setWorkflowRunning("wf-abcd1234", null);
});

it("remembers stop requests until cleared", () => {
  const listener = vi.fn();
  const off = subscribeWorkflowsRunning(listener);
  expect(workflowStopRequested("wf-abcd1234")).toBe(false);
  requestWorkflowStop("wf-abcd1234");
  expect(workflowStopRequested("wf-abcd1234")).toBe(true);
  clearWorkflowStop("wf-abcd1234");
  expect(workflowStopRequested("wf-abcd1234")).toBe(false);
  clearWorkflowStop("wf-abcd1234");
  expect(listener).toHaveBeenCalledTimes(2);
  off();
});

it("check-now reaches subscribers", () => {
  const listener = vi.fn();
  const off = subscribeCheckWorkflowsNow(listener);
  checkWorkflowsNow();
  expect(listener).toHaveBeenCalledTimes(1);
  off();
  checkWorkflowsNow();
  expect(listener).toHaveBeenCalledTimes(1);
});

it("updateWorkflows refuses a list that would pass the file cap, with a clear message", async () => {
  const saved: unknown[] = [];
  native.invoke.mockImplementation(async (command: string) => {
    if (command === "mono_read") return { text: "[]", hash: "h1" };
    saved.push(command);
    return "new-hash";
  });
  const big = workflow({ description: "x".repeat(WORKFLOWS_FILE_BUDGET) });
  const error = await updateWorkflows("mono-1", () => ({
    workflows: [big],
    result: null,
  })).catch((e: unknown) => e);
  expect(error).toBeInstanceOf(Error);
  expect((error as Error).message).toMatch(/too large to save/);
  expect(saved).toEqual([]);
});

/** Serves `text` for every read and records what is saved. */
function serve(text: string | null) {
  const saved: string[] = [];
  native.invoke.mockImplementation(async (command: string, args: any) => {
    if (command === "mono_read") return { text, hash: "h" };
    saved.push(args.text);
    return "new-hash";
  });
  return saved;
}

it("updateWorkflows keeps an entry it cannot read, untouched", async () => {
  const strange = {
    id: "wf-strange01",
    name: "From a newer app",
    nodes: [{ id: "trigger", type: "manual" }, { id: "x", type: "teleport", beam: [1, 2] }],
    edges: [],
    futureField: { keep: true },
  };
  const saved = serve(JSON.stringify([workflow(), strange]));
  await updateWorkflows("mono-1", (list) => ({
    workflows: list.map((w) => ({ ...w, name: "Renamed" })),
    result: null,
  }));
  const written = JSON.parse(saved[0]);
  expect(written).toHaveLength(2);
  expect(written[0]).toMatchObject({ id: "wf-abcd1234", name: "Renamed" });
  expect(written[1]).toEqual(strange);
  // The panel still lists only what it can read.
  serve(JSON.stringify([workflow(), strange]));
  expect((await loadWorkflows("mono-1")).map((w) => w.id)).toEqual(["wf-abcd1234"]);
});

it("updateWorkflows keeps unreadable entries that are not objects", async () => {
  const saved = serve(JSON.stringify([3, null, workflow(), "text"]));
  await updateWorkflows("mono-1", (list) => ({ workflows: list, result: null }));
  expect(JSON.parse(saved[0])).toEqual([workflow(), 3, null, "text"]);
});

it("updateWorkflows refuses to add an id that an unreadable entry already has", async () => {
  const saved = serve(JSON.stringify([{ id: "wf-zzzz9999", nodes: "broken" }]));
  const error = await updateWorkflows("mono-1", (list) => ({
    workflows: [...list, workflow({ id: "wf-zzzz9999" })],
    result: null,
  })).catch((e: unknown) => e);
  expect((error as Error).message).toContain("wf-zzzz9999");
  expect(saved).toEqual([]);
});

it("updateWorkflows refuses to write over a file that is not valid JSON", async () => {
  const saved = serve("{ not json");
  const edit = vi.fn((list: Workflow[]) => ({ workflows: list, result: 1 }));
  await expect(updateWorkflows("mono-1", edit)).rejects.toThrow(
    "workflows.json could not be read; fix or remove the file before changing workflows",
  );
  expect(saved).toEqual([]);
  expect(edit).not.toHaveBeenCalled();
});

it("updateWorkflows refuses to write over a file that is not a list", async () => {
  const saved = serve('{"workflows": []}');
  await expect(
    updateWorkflows("mono-1", (list) => ({ workflows: list, result: 1 })),
  ).rejects.toThrow("could not be read");
  expect(saved).toEqual([]);
});

it("updateWorkflows treats a missing or blank file as an empty list", async () => {
  let saved = serve(null);
  await updateWorkflows("mono-1", () => ({ workflows: [workflow()], result: 1 }));
  expect(JSON.parse(saved[0])).toHaveLength(1);
  saved = serve("  \n");
  await updateWorkflows("mono-1", () => ({ workflows: [workflow()], result: 1 }));
  expect(JSON.parse(saved[0])).toHaveLength(1);
});

it("updateRuns refuses to write over a runs file that is not valid JSON", async () => {
  const saved = serve("[{ broken");
  await expect(
    updateRuns("mono-1", "wf-abcd1234", (runs) => ({ runs, result: 1 })),
  ).rejects.toThrow("could not be read");
  expect(saved).toEqual([]);
});

it("updateRuns keeps run entries it cannot read", async () => {
  const odd = { id: "run-odd", note: "from a newer app" };
  const saved = serve(JSON.stringify([run("a"), odd]));
  await updateRuns("mono-1", "wf-abcd1234", (runs) => ({
    runs: [run("b"), ...runs],
    result: 1,
  }));
  expect((JSON.parse(saved[0]) as { id: string }[]).map((r) => r.id)).toEqual(["b", "a", "run-odd"]);
});

it("updateWorkflows drops a duplicate-id leftover together with the removed workflow", async () => {
  const twin = { id: "wf-abcd1234", name: "Twin", nodes: "broken" };
  const other = { id: "wf-other000", nodes: "broken" };
  const saved = serve(JSON.stringify([workflow(), twin, other]));
  await updateWorkflows("mono-1", () => ({ workflows: [], result: null }));
  // The unreadable entry with another id stays; the twin of the removed workflow goes.
  expect(JSON.parse(saved[0])).toEqual([other]);
  // While the readable one stays, so does its twin.
  const kept = serve(JSON.stringify([workflow(), twin]));
  await updateWorkflows("mono-1", (list) => ({ workflows: list, result: null }));
  expect(JSON.parse(kept[0])).toEqual([workflow(), twin]);
});
