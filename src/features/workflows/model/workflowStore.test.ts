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
