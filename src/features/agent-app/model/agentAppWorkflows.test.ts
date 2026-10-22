// @vitest-environment happy-dom
import { describe, expect, it, vi } from "vitest";
import { newSession } from "../../sessions/model/session";
import type { Workflow, WorkflowRun } from "../../workflows/model/workflow";
import { validateWorkflow } from "../../workflows/model/workflowParse";
import { handleAgentApp, type AgentAppHost } from "./agentApp";

const NOW = new Date(2026, 9, 22, 10, 0, 0);

/** A small valid definition: schedule -> session -> report. */
function definition(extra: Record<string, unknown> = {}) {
  return {
    name: "Ticket review",
    inputs: { project: { type: "project" } },
    inputValues: { project: "/code/app" },
    nodes: [
      { id: "trigger", type: "schedule", schedule: { kind: "weekdays", time: "09:00" } },
      {
        id: "fetch",
        type: "session",
        project: "{{inputs.project}}",
        prompt: "List my open tickets.",
        checkout: "root",
        output: { tickets: { type: "list" } },
      },
      { id: "report", type: "report" },
    ],
    edges: [
      { from: "trigger", to: "fetch" },
      { from: "fetch", to: "report" },
    ],
    ...extra,
  };
}

function run(id: string, status: WorkflowRun["status"], workflowId = "wf-aaaaaaaa") {
  return {
    id,
    workflowId,
    definition: {} as Workflow,
    status,
    startedAt: NOW.getTime() - 60_000,
    trigger: "manual",
    items: {},
    nodes: [
      { nodeId: "fetch", branch: "", status: "done", round: 1 },
      { nodeId: "dev", branch: "each#0", status: "failed", round: 1, error: "boom" },
    ],
  } as WorkflowRun;
}

function fixture(kind: "mono" | "session" = "mono") {
  const source = newSession("codex", "/Users/me", "codex:test");
  source.id = "chat";
  let stored: Workflow[] = [];
  const runs = new Map<string, WorkflowRun[]>();
  const live = new Map<string, string>();
  const checkNow = vi.fn();
  const stop = vi.fn(async () => {});
  const resumeRun = vi.fn(async () => {});
  const host = {
    isMono: (id: string) => kind === "mono" && id === source.id,
    monoOf: () => ({ id: "mono-1", projects: [] }),
    now: () => NOW,
    workflows: {
      load: async () => structuredClone(stored),
      update: async (
        _monoId: string,
        edit: (list: Workflow[]) => { workflows: Workflow[]; result: unknown },
      ) => {
        const { workflows, result } = edit(structuredClone(stored));
        stored = workflows;
        return result;
      },
      runs: async (_monoId: string, workflowId: string) => runs.get(workflowId) ?? [],
      running: (workflowId: string) => live.get(workflowId),
      stop,
      resumeRun,
      checkNow,
    },
  } as unknown as AgentAppHost;
  return {
    call: (action: string, input: Record<string, unknown> = {}) =>
      handleAgentApp(source, "req", action, input, host),
    stored: () => stored,
    seed: (list: Workflow[]) => {
      stored = list;
    },
    runs,
    live,
    checkNow,
    stop,
    resumeRun,
  };
}

type Added = { id: string; enabled: boolean; issues: { message: string }[] };

describe("workflows.add", () => {
  it("add rejects unknown fields", async () => {
    const { call, stored } = fixture();
    await expect(call("workflows.add", { workflow: definition({ extra: 1 }) })).rejects.toThrow(
      /Unknown workflow fields: extra/,
    );
    for (const field of ["id", "createdAt", "runRequested", "stopRequested", "resumeRunId", "nextRunAt", "lastRunId"])
      await expect(
        call("workflows.add", { workflow: definition({ [field]: "x" }) }),
      ).rejects.toThrow(`Unknown workflow fields: ${field}`);
    await expect(call("workflows.add", { workflow: definition(), bogus: 1 })).rejects.toThrow(
      /Unknown workflows.add fields: bogus/,
    );
    expect(stored()).toEqual([]);
  });

  it("saves a valid workflow enabled, with its next run time and a generated id", async () => {
    const { call, stored } = fixture();
    const result = (await call("workflows.add", { workflow: definition() })) as Added;
    expect(result).toMatchObject({ enabled: true, issues: [] });
    expect(result.id).toMatch(/^wf-[a-z0-9]{8}$/);
    const [saved] = stored();
    expect(saved).toMatchObject({
      id: result.id,
      name: "Ticket review",
      enabled: true,
      createdAt: NOW.getTime(),
      inputValues: { project: "/code/app" },
    });
    // Weekdays 09:00, and it is Thursday 10:00: next is Friday.
    expect(new Date(saved.nextRunAt!).getDay()).toBe(5);
    expect(saved.nodes[0]).toMatchObject({
      schedule: { scheduleKind: "weekdays", time: "09:00" },
    });
    expect(validateWorkflow(saved)).toEqual([]);
  });

  it("add with issues saves paused and returns issues", async () => {
    const { call, stored } = fixture();
    const broken = definition({
      nodes: definition().nodes.filter((node) => node.type !== "report"),
      edges: [{ from: "trigger", to: "fetch" }],
    });
    const result = (await call("workflows.add", { workflow: broken })) as Added;
    expect(result.enabled).toBe(false);
    expect(result.issues.map((issue) => issue.message).join(" ")).toMatch(/Report/);
    expect(stored()[0]).toMatchObject({ id: result.id, enabled: false });
    expect(stored()[0].nextRunAt).toBeUndefined();
  });

  it("pauses a workflow whose input has no value and says which", async () => {
    const { call } = fixture();
    const { inputValues: _values, ...bare } = definition();
    const result = (await call("workflows.add", { workflow: bare })) as Added;
    expect(result.enabled).toBe(false);
    expect(result.issues[0].message).toBe('Missing a value for input "project"');
  });

  it("explains a node it cannot read and enforces limits", async () => {
    const { call, seed } = fixture();
    await expect(
      call("workflows.add", {
        workflow: definition({ nodes: [{ id: "x", type: "teleport" }] }),
      }),
    ).rejects.toThrow(/nodes\[0\] \("x"\)/);
    await expect(
      call("workflows.add", { workflow: definition({ name: "n".repeat(81) }) }),
    ).rejects.toThrow(/name must be a non-empty string under 80/);
    await expect(call("workflows.add", {})).rejects.toThrow(/either workflow/);
    seed(Array.from({ length: 20 }, (_, i) => ({ id: `wf-${i}` }) as Workflow));
    await expect(call("workflows.add", { workflow: definition() })).rejects.toThrow(
      /at most 20 workflows/,
    );
  });

  it("saves a YAML file paused with the commands to review, and keeps the values it is given", async () => {
    const source = fixture();
    const { id } = (await source.call("workflows.add", { workflow: definition() })) as Added;
    const { yaml } = (await source.call("workflows.export", { id })) as { yaml: string };
    const other = fixture();
    const result = (await other.call("workflows.add", {
      yaml,
      values: { project: "/code/other" },
    })) as Added & { commands: unknown[]; prompts: { nodeId: string }[] };
    expect(result).toMatchObject({ enabled: false, issues: [], commands: [] });
    expect(result.prompts[0].nodeId).toBe("fetch");
    expect(other.stored()[0]).toMatchObject({
      enabled: false,
      inputValues: { project: "/code/other" },
    });
    expect(other.stored()[0].nextRunAt).toBeUndefined();
  });

  it("anchors an interval schedule the same way from YAML and from JSON", async () => {
    const interval = { kind: "interval", hours: 4, time: "09:00" };
    const withInterval = definition({
      nodes: definition().nodes.map((node) =>
        node.type === "schedule" ? { ...node, schedule: interval } : node,
      ),
    });
    const json = fixture();
    const { id } = (await json.call("workflows.add", { workflow: withInterval })) as Added;
    const { yaml } = (await json.call("workflows.export", { id })) as { yaml: string };
    const fromYaml = fixture();
    await fromYaml.call("workflows.add", { yaml, values: { project: "/code/app" } });
    for (const store of [json, fromYaml])
      expect(store.stored()[0].nodes[0]).toMatchObject({
        schedule: { scheduleKind: "interval", intervalHours: 4, intervalFrom: "2026-10-22" },
      });
  });

  it("turns bad, mistyped and undeclared input values into issues and saves paused", async () => {
    const { call, stored } = fixture();
    const cases: [Record<string, unknown>, RegExp][] = [
      [{ project: "" }, /needs a project/],
      [{ project: 7 }, /needs a project/],
      [{ project: "/code/app", stray: "x" }, /Unknown input "stray"/],
    ];
    for (const [inputValues, message] of cases) {
      const result = (await call("workflows.add", {
        workflow: definition({ inputValues }),
      })) as Added;
      expect(result.enabled).toBe(false);
      expect(result.issues.map((issue) => issue.message).join(" ")).toMatch(message);
    }
    expect(stored().every((w) => !w.enabled && w.nextRunAt === undefined)).toBe(true);
  });

  it("only a Mono's own conversation may manage workflows", async () => {
    const { call } = fixture("session");
    await expect(call("workflows.list")).rejects.toThrow(/Only a Mono's own conversation/);
  });
});

describe("workflows.update", () => {
  it("replaces the definition but keeps id, createdAt, inputValues, enabled and run state", async () => {
    const { call, stored, seed } = fixture();
    const { id } = (await call("workflows.add", { workflow: definition() })) as Added;
    const added = stored()[0];
    seed([
      {
        ...added,
        runRequested: true,
        stopRequested: true,
        resumeRunId: "run-1",
        lastRunId: "run-0",
        importedFrom: "somewhere",
      },
    ]);
    const edited = definition({ name: "Renamed" });
    delete (edited as Record<string, unknown>).inputValues;
    const result = (await call("workflows.update", { id, workflow: edited })) as Added;
    expect(result).toEqual({ id, enabled: true, issues: [] });
    expect(stored()[0]).toMatchObject({
      id,
      name: "Renamed",
      createdAt: added.createdAt,
      inputValues: { project: "/code/app" },
      enabled: true,
      nextRunAt: added.nextRunAt,
      runRequested: true,
      stopRequested: true,
      resumeRunId: "run-1",
      lastRunId: "run-0",
      importedFrom: "somewhere",
    });
  });

  it("drops a description the new definition leaves out, and takes inputValues and enabled when given", async () => {
    const { call, stored } = fixture();
    const { id } = (await call("workflows.add", {
      workflow: definition({ description: "old" }),
    })) as Added;
    await call("workflows.update", {
      id,
      workflow: definition({ inputValues: { project: "/code/new" }, enabled: false }),
    });
    expect(stored()[0].description).toBeUndefined();
    expect(stored()[0]).toMatchObject({
      inputValues: { project: "/code/new" },
      enabled: false,
    });
  });

  it("pauses a workflow that no longer validates and returns the issues", async () => {
    const { call, stored } = fixture();
    const { id } = (await call("workflows.add", { workflow: definition() })) as Added;
    const result = (await call("workflows.update", {
      id,
      workflow: definition({ edges: [] }),
    })) as Added;
    expect(result.enabled).toBe(false);
    expect(result.issues.length).toBeGreaterThan(0);
    expect(stored()[0].enabled).toBe(false);
  });

  it("pauses an update whose input values are bad, mistyped or undeclared", async () => {
    const { call, stored } = fixture();
    const { id } = (await call("workflows.add", { workflow: definition() })) as Added;
    for (const inputValues of [{ project: "" }, { project: 7 }, { project: "/p", stray: 1 }]) {
      await call("workflows.update", {
        id,
        workflow: definition({ inputValues: { project: "/code/app" }, enabled: true }),
      });
      expect(stored()[0].enabled).toBe(true);
      const result = (await call("workflows.update", {
        id,
        workflow: definition({ inputValues }),
      })) as Added;
      expect(result.enabled).toBe(false);
      expect(result.issues.length).toBeGreaterThan(0);
      expect(stored()[0].enabled).toBe(false);
    }
  });

  it("refuses an unknown id and runtime fields", async () => {
    const { call } = fixture();
    await expect(
      call("workflows.update", { id: "wf-nope", workflow: definition() }),
    ).rejects.toThrow(/No workflow with that id/);
    const { id } = (await call("workflows.add", { workflow: definition() })) as Added;
    await expect(
      call("workflows.update", { id, workflow: definition({ runRequested: true }) }),
    ).rejects.toThrow(/Unknown workflow fields: runRequested/);
  });
});

describe("export and import", () => {
  it("export then import round-trips and imports paused", async () => {
    const { call, stored } = fixture();
    const { id } = (await call("workflows.add", { workflow: definition() })) as Added;
    const { yaml } = (await call("workflows.export", { id })) as { yaml: string };
    expect(yaml).toContain("monoptahWorkflow: 1");
    expect(yaml).not.toContain("/code/app");
    const imported = (await call("workflows.import", {
      yaml,
      values: { project: "/code/app" },
    })) as { id: string; enabled: boolean; issues: unknown[] };
    expect(imported).toMatchObject({ enabled: false, issues: [] });
    expect(imported.id).not.toBe(id);
    const [original, copy] = stored();
    expect(copy).toMatchObject({ id: imported.id, enabled: false, inputValues: { project: "/code/app" } });
    expect(copy.name).toBe(original.name);
    expect(copy.inputs).toEqual(original.inputs);
    expect(copy.nodes).toEqual(original.nodes);
    expect(copy.edges).toEqual(original.edges);
  });

  it("names the input that has no value when importing", async () => {
    const { call, stored } = fixture();
    const { id } = (await call("workflows.add", { workflow: definition() })) as Added;
    const { yaml } = (await call("workflows.export", { id })) as { yaml: string };
    await expect(call("workflows.import", { yaml })).rejects.toThrow(/"project"/);
    await expect(call("workflows.import", { yaml: "nope: 1" })).rejects.toThrow(
      /Not a Monoptah workflow|Unsupported/,
    );
    expect(stored()).toHaveLength(1);
  });

  it("refuses to export a workflow with an absolute path in it", async () => {
    const { call } = fixture();
    const { id } = (await call("workflows.add", {
      workflow: definition({ description: "Work in /Users/me/secret" }),
    })) as Added;
    await expect(call("workflows.export", { id })).rejects.toThrow(/absolute path/);
  });
});

describe("running", () => {
  it("run sets runRequested and calls checkNow", async () => {
    const { call, stored, checkNow } = fixture();
    const { id } = (await call("workflows.add", { workflow: definition() })) as Added;
    await call("workflows.run", { id });
    expect(stored()[0].runRequested).toBe(true);
    expect(checkNow).toHaveBeenCalledTimes(1);
  });

  it("run refuses a workflow that is running or cannot run", async () => {
    const { call, stored, live, checkNow } = fixture();
    const { id } = (await call("workflows.add", { workflow: definition() })) as Added;
    live.set(id, "run-1");
    await expect(call("workflows.run", { id })).rejects.toThrow(/already running/);
    live.clear();
    const broken = (await call("workflows.add", {
      workflow: definition({ edges: [] }),
    })) as Added;
    await expect(call("workflows.run", { id: broken.id })).rejects.toThrow(/cannot run yet/);
    expect(stored().some((w) => w.runRequested)).toBe(false);
    expect(checkNow).not.toHaveBeenCalled();
  });

  it("list reports running state", async () => {
    const { call, live, runs } = fixture();
    const first = (await call("workflows.add", { workflow: definition() })) as Added;
    const second = (await call("workflows.add", {
      workflow: definition({ name: "Other" }),
    })) as Added;
    live.set(first.id, "run-1");
    runs.set(first.id, [run("run-1", "running", first.id)]);
    const { workflows } = (await call("workflows.list")) as {
      workflows: Record<string, unknown>[];
    };
    expect(workflows).toHaveLength(2);
    expect(workflows[0]).toMatchObject({
      id: first.id,
      name: "Ticket review",
      enabled: true,
      trigger: { type: "schedule" },
      running: true,
      lastRun: { id: "run-1", status: "running" },
    });
    expect(workflows[1]).toMatchObject({ id: second.id, running: false, lastRun: null });
    expect(typeof workflows[0].nextRunAt).toBe("string");
  });

  it("stop goes through the host so any window can stop, and only a running workflow", async () => {
    const { call, live, stop } = fixture();
    const { id } = (await call("workflows.add", { workflow: definition() })) as Added;
    await expect(call("workflows.stop", { id })).rejects.toThrow(/not running/);
    live.set(id, "run-1");
    await call("workflows.stop", { id });
    expect(stop).toHaveBeenCalledWith("mono-1", id);
  });

  it("resumeRun only resumes an interrupted run", async () => {
    const { call, runs, resumeRun } = fixture();
    const { id } = (await call("workflows.add", { workflow: definition() })) as Added;
    runs.set(id, [run("run-2", "done", id), run("run-1", "interrupted", id)]);
    await expect(call("workflows.resumeRun", { id, runId: "nope" })).rejects.toThrow(/No run/);
    await expect(call("workflows.resumeRun", { id, runId: "run-2" })).rejects.toThrow(
      /interrupted/,
    );
    await call("workflows.resumeRun", { id, runId: "run-1" });
    expect(resumeRun).toHaveBeenCalledWith("mono-1", id, "run-1");
  });
});

describe("pause, resume, setInputs, remove, get and runs", () => {
  it("pauses and resumes, computing the next time on resume", async () => {
    const { call, stored } = fixture();
    const { id } = (await call("workflows.add", { workflow: definition() })) as Added;
    await call("workflows.pause", { id });
    expect(stored()[0].enabled).toBe(false);
    await call("workflows.resume", { id });
    expect(stored()[0]).toMatchObject({ enabled: true });
    expect(stored()[0].nextRunAt).toBeGreaterThan(NOW.getTime());
  });

  it("will not resume a workflow with issues", async () => {
    const { call } = fixture();
    const { id } = (await call("workflows.add", {
      workflow: definition({ edges: [] }),
    })) as Added;
    await expect(call("workflows.resume", { id })).rejects.toThrow(/cannot be resumed/);
  });

  it("sets declared inputs with their type and rejects others", async () => {
    const { call, stored } = fixture();
    const { id } = (await call("workflows.add", {
      workflow: definition({
        inputs: { project: { type: "project" }, count: { type: "number" } },
      }),
    })) as Added;
    expect(stored()[0].enabled).toBe(false);
    const result = (await call("workflows.setInputs", {
      id,
      values: { count: 3 },
    })) as { issues: unknown[]; inputValues: Record<string, unknown> };
    expect(result.inputValues).toEqual({ project: "/code/app", count: 3 });
    expect(result.issues).toEqual([]);
    await expect(
      call("workflows.setInputs", { id, values: { count: "x" } }),
    ).rejects.toThrow(/must be a number/);
    await expect(call("workflows.setInputs", { id, values: { nope: 1 } })).rejects.toThrow(
      /Unknown input "nope"/,
    );
    await expect(call("workflows.setInputs", { id, values: { project: "" } })).rejects.toThrow(
      /needs a project/,
    );
    await expect(call("workflows.setInputs", { id, values: { project: 7 } })).rejects.toThrow(
      /needs a project/,
    );
    await expect(
      call("workflows.setInputs", { id, values: { toString: "x" } }),
    ).rejects.toThrow(/Unknown input "toString"/);
    expect(stored()[0].inputValues).toEqual({ project: "/code/app", count: 3 });
  });

  it("removes a workflow unless it is running", async () => {
    const { call, stored, live } = fixture();
    const { id } = (await call("workflows.add", { workflow: definition() })) as Added;
    live.set(id, "run-1");
    await expect(call("workflows.remove", { id })).rejects.toThrow(/Stop the workflow/);
    live.clear();
    expect(await call("workflows.remove", { id })).toEqual({ removed: "Ticket review" });
    expect(stored()).toEqual([]);
  });

  it("get returns the definition and the last five runs; runs shows branches", async () => {
    const { call, runs } = fixture();
    const { id } = (await call("workflows.add", { workflow: definition() })) as Added;
    runs.set(
      id,
      Array.from({ length: 7 }, (_, i) => run(`run-${i}`, "done", id)),
    );
    const got = (await call("workflows.get", { id })) as {
      nodes: unknown[];
      runs: unknown[];
      issues: unknown[];
      inputValues: unknown;
    };
    expect(got.nodes).toHaveLength(3);
    expect(got.runs).toHaveLength(5);
    expect(got.issues).toEqual([]);
    expect(got.inputValues).toEqual({ project: "/code/app" });
    const { runs: summaries } = (await call("workflows.runs", { id })) as {
      runs: { nodes: Record<string, unknown>[] }[];
    };
    expect(summaries).toHaveLength(5);
    expect(summaries[0].nodes[1]).toEqual({
      nodeId: "dev",
      branch: "each#0",
      status: "failed",
      round: 1,
      error: "boom",
    });
    expect(
      ((await call("workflows.runs", { id, limit: 2 })) as { runs: unknown[] }).runs,
    ).toHaveLength(2);
    await expect(call("workflows.runs", { id, limit: 0 })).rejects.toThrow(/limit/);
  });
});
