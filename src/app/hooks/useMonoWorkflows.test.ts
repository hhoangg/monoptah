// @vitest-environment happy-dom
import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { ControlOutcome } from "../../features/orchestration/model/orchestration";
import type { Block, Session } from "../../features/sessions/model/session";
import type {
  SessionNode,
  Workflow,
  WorkflowEdge,
  WorkflowNode,
  WorkflowRun,
} from "../../features/workflows/model/workflow";
import { validateWorkflow } from "../../features/workflows/model/workflowParse";
import {
  markInterrupted,
  startRun,
} from "../../features/workflows/model/workflowEngine";

const files = new Map<string, { text: string; hash: number }>();
const MONO = {
  id: "mono-1",
  sessionId: "mono",
  mascot: "skull",
  color: "#fff",
  projects: ["/code/app"],
};
const fileFailures = { runs: false };

vi.mock("../../features/monos/model/mono", async (original) => ({
  ...(await original<object>()),
  listMonos: () => [MONO],
  findMono: (id: string) => (id === MONO.id ? MONO : undefined),
}));
vi.mock("../../features/monos/model/monoFiles", async (original) => {
  const actual =
    await original<typeof import("../../features/monos/model/monoFiles")>();
  return {
    ...actual,
    readAgentFile: vi.fn(async (_monoId: string, path: string) => {
      const file = files.get(path);
      return { text: file?.text ?? null, hash: String(file?.hash ?? 0) };
    }),
    writeAgentFile: vi.fn(
      async (_monoId: string, path: string, text: string, expected: string) => {
        if (fileFailures.runs && path.startsWith("workflow-runs/"))
          throw new Error("file too large");
        const current = files.get(path)?.hash ?? 0;
        if (expected !== String(current)) throw new actual.MonoFileConflict();
        files.set(path, { text, hash: current + 1 });
        return String(current + 1);
      },
    ),
  };
});

const { useMonoWorkflows, resumeWorkflowRun } =
  await import("./useMonoWorkflows");
const { checkWorkflowsNow, requestWorkflowStop, workflowRunningId } =
  await import("../../features/workflows/model/workflowStore");
const { setWorkflowRunning, clearWorkflowStop } =
  await import("../../features/workflows/model/workflowStore");
const { relayApproval, dropRelays } =
  await import("../../features/monos/model/monoHabits");

const WF = "wf-abcd1234";
const SCHEDULE = {
  scheduleKind: "daily",
  time: "09:00",
  minute: 0,
  dayOfWeek: 1,
} as const;

function session(
  id: string,
  output: SessionNode["output"] = {},
  extra: Partial<SessionNode> = {},
): SessionNode {
  return {
    id,
    type: "session",
    project: "{{inputs.repo}}",
    prompt: `Do ${id}`,
    checkout: "root",
    output,
    ...extra,
  };
}

function workflow(
  nodes: WorkflowNode[],
  edges: WorkflowEdge[],
  extra: Partial<Workflow> = {},
): Workflow {
  const built: Workflow = {
    id: WF,
    name: "Ship it",
    inputs: { repo: { type: "project" } },
    inputValues: { repo: "/code/app" },
    nodes,
    edges,
    enabled: true,
    createdAt: 0,
    nextRunAt: Date.now() - 60_000,
    ...extra,
  };
  expect(validateWorkflow(built)).toEqual([]);
  return built;
}

const trigger: WorkflowNode = {
  id: "trigger",
  type: "schedule",
  schedule: SCHEDULE,
};
const report: WorkflowNode = { id: "report", type: "report" };

/** trigger -> dev -> report */
function simple(
  output: SessionNode["output"] = {},
  extra: Partial<Workflow> = {},
) {
  return workflow(
    [trigger, session("dev", output), report],
    [
      { from: "trigger", to: "dev" },
      { from: "dev", to: "report" },
    ],
    extra,
  );
}

function save(workflows: Workflow[]) {
  const hash = (files.get("workflows.json")?.hash ?? 0) + 1;
  files.set("workflows.json", { text: JSON.stringify(workflows), hash });
}
const stored = (): Workflow[] =>
  JSON.parse(files.get("workflows.json")?.text ?? "[]");
const storedRuns = (): WorkflowRun[] =>
  JSON.parse(files.get(`workflow-runs/${WF}.json`)?.text ?? "[]");
function saveRuns(runs: WorkflowRun[]) {
  files.set(`workflow-runs/${WF}.json`, {
    text: JSON.stringify(runs),
    hash: (files.get(`workflow-runs/${WF}.json`)?.hash ?? 0) + 1,
  });
}

const fenced = (value: unknown) =>
  `Done.\n\`\`\`workflow-output\n${JSON.stringify(value)}\n\`\`\``;

function deferred() {
  let resolve!: () => void;
  const promise = new Promise<void>((done) => (resolve = done));
  return { promise, resolve };
}

type Script = {
  /** Replies handed out, one per turn (start or send), in call order. */
  replies: string[];
  /** Turns wait for this before they answer. */
  hold?: Promise<void>;
  approval?: boolean;
};

function makeHost(script: Script) {
  const mono = {
    id: "mono",
    cwd: "/code/app",
    blocks: [],
  } as unknown as Session;
  const sessions: Session[] = [mono];
  let counter = 0;
  const turn = async (id: string, prompt: string): Promise<ControlOutcome> => {
    const target = sessions.find((entry) => entry.id === id)!;
    target.blocks = [
      ...target.blocks,
      { id: `u${counter++}`, role: "user", text: prompt },
    ];
    if (script.approval) {
      target.blocks = [
        ...target.blocks,
        {
          id: "ask",
          role: "approval",
          text: "",
          tool: { title: "git push" },
          approval: { requestId: 7 },
        },
      ];
    }
    await script.hold;
    if (script.approval)
      target.blocks = target.blocks.map((block) =>
        block.id === "ask"
          ? { ...block, approval: { requestId: 7, decided: "allow" as const } }
          : block,
      );
    const reply = script.replies.shift() ?? "";
    target.blocks = [
      ...target.blocks,
      { id: `a${counter++}`, role: "assistant", text: reply },
    ];
    return { status: "completed", text: reply };
  };
  const host = {
    sessions: () => sessions,
    startSession: vi.fn(
      async (
        _monoId: string,
        req: { project: string; prompt: string; folderId?: string },
      ) => {
        const id = `s${sessions.length}`;
        sessions.push({
          id,
          cwd: req.project,
          title: `Session ${id}`,
          harness: "claude",
          model: "claude:x",
          blocks: [],
        } as unknown as Session);
        return {
          sessionId: id,
          folderId: "folder-1",
          settled: turn(id, req.prompt),
        };
      },
    ),
    send: vi.fn(turn),
    stop: vi.fn(async () => {}),
    runCommand: vi.fn(async (_cwd: string, command: string) =>
      command.includes("bad")
        ? { exitCode: 1, stdout: "", stderr: "boom", timedOut: false }
        : { exitCode: 0, stdout: "fine", stderr: "", timedOut: false },
    ),
    askMono: vi.fn(async (): Promise<string> => fenced({ answer: "exit" })),
    post: vi.fn(),
    // Like the app: the request is relayed, so later polls skip it.
    askApproval: vi.fn(
      (
        monoId: string,
        _workflow: Workflow,
        _runId: string,
        sessionId: string,
        block: Block,
      ) =>
        relayApproval({
          monoId,
          runId: sessionId,
          requestId: block.approval!.requestId,
          blockId: "b",
        }),
    ),
    endApprovals: vi.fn(),
    projectPath: vi.fn((_monoId: string, input: string) =>
      input === "/code/app" ? input : undefined,
    ),
    worktreeOf: vi.fn((id: string) => `/code/app-wt-${id}`),
  };
  return host;
}

let root: Root;
const originalWindow = window;
let release: Array<() => void> = [];

beforeEach(() => {
  files.clear();
  fileFailures.runs = false;
  release = [];
  delete (window as unknown as Record<string, unknown>).__monoWorkflowsSwept;
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  root = createRoot(document.createElement("div"));
});
afterEach(() => {
  for (const done of release) done();
  vi.stubGlobal("window", originalWindow);
  act(() => root.unmount());
  setWorkflowRunning(WF, null);
  clearWorkflowStop(WF);
  dropRelays("s1");
  vi.useRealTimers();
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

function mount(host: ReturnType<typeof makeHost>) {
  function Probe() {
    useMonoWorkflows(host);
    return null;
  }
  act(() => root.render(createElement(Probe)));
}

function hold(script: Script) {
  const gate = deferred();
  script.hold = gate.promise;
  release.push(gate.resolve);
  return gate;
}

describe("useMonoWorkflows", () => {
  it("due schedule workflow starts a run and posts a report", async () => {
    save([simple({ summary: { type: "string" } })]);
    const script: Script = { replies: [fenced({ summary: "all good" })] };
    const host = makeHost(script);
    mount(host);
    await vi.waitFor(() => expect(host.post).toHaveBeenCalled());

    const [monoSessionId, posted, runId, title, text, launches] =
      host.post.mock.calls[0];
    expect(monoSessionId).toBe("mono");
    expect(posted.id).toBe(WF);
    expect(title).toBe("Ship it · completed");
    expect(text).toContain("all good");
    expect(launches).toEqual([
      {
        sessionId: "s1",
        cwd: "/code/app",
        title: "Session s1",
        harness: "claude",
        model: "claude:x",
      },
    ]);

    const request = host.startSession.mock.calls[0];
    expect(request[0]).toBe("mono-1");
    expect(request[1]).toMatchObject({
      project: "/code/app",
      workspaceMode: "current",
    });
    expect(request[1].prompt).toContain("Do dev");
    expect(request[1].prompt).toContain("workflow-output");
    expect(request[1].folderName).toMatch(
      /^Ship it · \d{4}-\d{2}-\d{2} \d{2}:\d{2}$/,
    );

    await vi.waitFor(() => expect(storedRuns()[0]?.status).toBe("done"));
    expect(storedRuns()[0]).toMatchObject({
      id: runId,
      trigger: "schedule",
      folderId: "folder-1",
    });
    expect(storedRuns()[0].nodes.find((n) => n.nodeId === "dev")).toMatchObject(
      { status: "done", sessionId: "s1", output: { summary: "all good" } },
    );
    const after = stored()[0];
    expect(after.nextRunAt).toBeGreaterThan(Date.now());
    expect(after.runRequested).toBeUndefined();
    expect(after.lastRunId).toBe(runId);
    expect(workflowRunningId(WF)).toBeUndefined();
  });

  it("a requested run starts even when the schedule is not due", async () => {
    save([
      simple({}, { runRequested: true, nextRunAt: Date.now() + 3_600_000 }),
    ]);
    const host = makeHost({ replies: ["ok"] });
    mount(host);
    await vi.waitFor(() => expect(host.post).toHaveBeenCalled());
    await vi.waitFor(() => expect(storedRuns()[0]?.status).toBe("done"));
    expect(storedRuns()[0].trigger).toBe("manual");
    expect(stored()[0].runRequested).toBeUndefined();
    // A requested run leaves the schedule where it was.
    expect(stored()[0].nextRunAt).toBeGreaterThan(Date.now() + 3_000_000);
  });

  it("skips a tick while previous run is running", async () => {
    save([simple()]);
    const script: Script = { replies: ["ok"] };
    const gate = hold(script);
    const host = makeHost(script);
    mount(host);
    await vi.waitFor(() => expect(host.startSession).toHaveBeenCalledTimes(1));
    expect(workflowRunningId(WF)).toBeDefined();

    save([{ ...stored()[0], nextRunAt: Date.now() - 1_000 }]);
    checkWorkflowsNow();
    await vi.waitFor(() =>
      expect(stored()[0].nextRunAt).toBeGreaterThan(Date.now()),
    );
    expect(host.startSession).toHaveBeenCalledTimes(1);

    gate.resolve();
    await vi.waitFor(() => expect(storedRuns()[0].status).toBe("done"));
    expect(storedRuns()).toHaveLength(1);
  });

  it("marks stored running runs interrupted on start", async () => {
    const def = simple({}, { enabled: false });
    save([def]);
    const running = startRun(def, "manual", 1_000, "run-old");
    expect(running.status).toBe("running");
    saveRuns([running]);
    const host = makeHost({ replies: [] });
    mount(host);
    await vi.waitFor(() => expect(storedRuns()[0].status).toBe("interrupted"));
    expect(host.startSession).not.toHaveBeenCalled();
  });

  it("leaves a stored running run alone when this window is driving it", async () => {
    const def = simple({}, { enabled: false });
    save([def]);
    const running = startRun(def, "manual", 1_000, "run-live");
    saveRuns([running]);
    setWorkflowRunning(WF, "run-live");
    const host = makeHost({ replies: [] });
    mount(host);
    // Give the first tick time to finish.
    await act(async () => {
      await new Promise((r) => setTimeout(r, 50));
    });
    expect(storedRuns()[0].status).toBe("running");
  });

  it("re-asks once for missing output then fails the node", async () => {
    save([simple({ summary: { type: "string" } })]);
    const script: Script = { replies: ["I did it.", "Still no block."] };
    const host = makeHost(script);
    mount(host);
    await vi.waitFor(() => expect(host.post).toHaveBeenCalled());

    expect(host.send).toHaveBeenCalledTimes(1);
    expect(host.send.mock.calls[0][0]).toBe("s1");
    expect(host.send.mock.calls[0][1]).toContain(
      "did not satisfy the output contract",
    );
    await vi.waitFor(() => expect(storedRuns()[0].status).toBe("failed"));
    const dev = storedRuns()[0].nodes.find((n) => n.nodeId === "dev")!;
    expect(dev.status).toBe("failed");
    expect(dev.reasked).toBe(true);
    expect(dev.error).toContain("workflow-output");
    expect(host.post.mock.calls[0][3]).toBe("Ship it · failed");
  });

  it("accepts the re-asked reply when it has the block", async () => {
    save([simple({ summary: { type: "string" } })]);
    const host = makeHost({
      replies: ["no block", fenced({ summary: "second try" })],
    });
    mount(host);
    await vi.waitFor(() => expect(storedRuns()[0]?.status).toBe("done"));
    expect(host.send).toHaveBeenCalledTimes(1);
    expect(
      storedRuns()[0].nodes.find((n) => n.nodeId === "dev")!.output,
    ).toEqual({ summary: "second try" });
  });

  it("never re-asks a node that declares no output", async () => {
    save([simple({})]);
    const host = makeHost({ replies: ["just prose, no block"] });
    mount(host);
    await vi.waitFor(() => expect(storedRuns()[0]?.status).toBe("done"));
    expect(host.send).not.toHaveBeenCalled();
  });

  it("loop reuses the dev session via send", async () => {
    const def = workflow(
      [
        trigger,
        session("dev", { summary: { type: "string" } }),
        session("review", { verdict: { type: "string" } }),
        report,
      ],
      [
        { from: "trigger", to: "dev" },
        { from: "dev", to: "review" },
        {
          from: "review",
          to: "dev",
          loop: { maxRounds: 3, decide: '{{review.verdict}} != "approve"' },
        },
        { from: "review", to: "report" },
      ],
    );
    save([def]);
    const host = makeHost({
      replies: [
        fenced({ summary: "v1" }),
        fenced({ verdict: "changes" }),
        fenced({ summary: "v2" }),
        fenced({ verdict: "approve" }),
      ],
    });
    mount(host);
    await vi.waitFor(() => expect(storedRuns()[0]?.status).toBe("done"));

    expect(host.startSession).toHaveBeenCalledTimes(2);
    expect(host.send).toHaveBeenCalledTimes(2);
    // Round 2 continues the dev session, then the review session.
    expect(host.send.mock.calls.map((call) => call[0])).toEqual(["s1", "s2"]);
    expect(host.send.mock.calls[0][1]).toContain("Do dev");
    expect(host.post.mock.calls[0][4]).toContain("Loop rounds used: 2");
  });

  it("a Mono-decided loop asks the Mono and goes round again", async () => {
    const def = workflow(
      [
        trigger,
        session("dev", { summary: { type: "string" } }),
        session("review", { notes: { type: "string" } }),
        report,
      ],
      [
        { from: "trigger", to: "dev" },
        { from: "dev", to: "review" },
        { from: "review", to: "report" },
        { from: "review", to: "dev", loop: { maxRounds: 3, decide: "mono" } },
      ],
    );
    save([def]);
    const host = makeHost({
      replies: [
        fenced({ summary: "a" }),
        fenced({ notes: "meh" }),
        fenced({ summary: "b" }),
        fenced({ notes: "good" }),
      ],
    });
    host.askMono
      .mockResolvedValueOnce(fenced({ answer: "loop", reason: "not enough" }))
      .mockResolvedValueOnce(fenced({ answer: "exit", reason: "good" }));
    mount(host);
    await vi.waitFor(() => expect(storedRuns()[0]?.status).toBe("done"));
    expect(host.askMono).toHaveBeenCalledTimes(2);
    expect(host.askMono.mock.calls[0][0]).toBe("mono-1");
    expect(host.askMono.mock.calls[0][1]).toContain("Allowed answers");
    // The second round continues both sessions.
    expect(host.send).toHaveBeenCalledTimes(2);
    const review = storedRuns()[0].nodes.find((n) => n.nodeId === "review")!;
    expect(review.decision).toEqual({ value: "exit", reason: "good" });
  });

  it("fails the node when the Mono's answer cannot be read", async () => {
    const def = workflow(
      [
        trigger,
        {
          id: "ask",
          type: "askMono",
          question: "Ship it?",
          answers: ["yes", "no"],
        },
        report,
      ],
      [
        { from: "trigger", to: "ask" },
        { from: "ask", to: "report" },
      ],
    );
    save([def]);
    const host = makeHost({ replies: [] });
    host.askMono.mockResolvedValueOnce("I think so, not sure.");
    mount(host);
    await vi.waitFor(() => expect(storedRuns()[0]?.status).toBe("failed"));
    const ask = storedRuns()[0].nodes.find((n) => n.nodeId === "ask")!;
    expect(ask.status).toBe("failed");
    expect(ask.error).toContain("workflow-output");
  });

  it("stop stops running sessions and ends as stopped", async () => {
    save([simple()]);
    const script: Script = { replies: ["ok"] };
    hold(script);
    const host = makeHost(script);
    mount(host);
    await vi.waitFor(() => expect(host.startSession).toHaveBeenCalledTimes(1));
    await vi.waitFor(() =>
      expect(
        storedRuns()[0]?.nodes.find((n) => n.nodeId === "dev")?.sessionId,
      ).toBe("s1"),
    );

    requestWorkflowStop(WF);
    await vi.waitFor(() => expect(storedRuns()[0].status).toBe("stopped"));
    expect(host.stop).toHaveBeenCalledWith("s1");
    expect(host.endApprovals).toHaveBeenCalledWith("s1");
    expect(storedRuns()[0].nodes.find((n) => n.nodeId === "dev")!.status).toBe(
      "stopped",
    );
    expect(host.post).not.toHaveBeenCalled();
    await vi.waitFor(() => expect(workflowRunningId(WF)).toBeUndefined());
  });

  it("command node failure fails only its branch", async () => {
    const def = workflow(
      [
        trigger,
        session("fetch", { tickets: { type: "list" } }),
        { id: "each", type: "forEach", in: "{{fetch.tickets}}", as: "item" },
        {
          id: "cmd",
          type: "command",
          project: "{{inputs.repo}}",
          command: "echo {{item}}",
          parent: "each",
        },
        report,
      ],
      [
        { from: "trigger", to: "fetch" },
        { from: "fetch", to: "each" },
        { from: "each", to: "report" },
      ],
    );
    save([def]);
    const host = makeHost({ replies: [fenced({ tickets: ["ok", "bad"] })] });
    mount(host);
    await vi.waitFor(() =>
      expect(storedRuns()[0]?.status).toBe("doneWithFailures"),
    );

    const nodes = storedRuns()[0].nodes.filter((n) => n.nodeId === "cmd");
    expect(nodes.map((n) => [n.branch, n.status])).toEqual([
      ["each#0", "done"],
      ["each#1", "failed"],
    ]);
    expect(nodes[0].output).toEqual({
      exitCode: 0,
      stdout: "fine",
      stderr: "",
    });
    expect(nodes[1].error).toContain("boom");
    expect(host.runCommand).toHaveBeenCalledWith(
      "/code/app",
      "echo 'ok'",
      10 * 60_000,
    );
    expect(host.post.mock.calls[0][3]).toBe(
      "Ship it · completed with failures",
    );
  });

  it("a command that times out fails its node", async () => {
    const def = workflow(
      [
        trigger,
        {
          id: "cmd",
          type: "command",
          project: "{{inputs.repo}}",
          command: "sleep 100",
          timeoutMinutes: 2,
        },
        report,
      ],
      [
        { from: "trigger", to: "cmd" },
        { from: "cmd", to: "report" },
      ],
    );
    save([def]);
    const host = makeHost({ replies: [] });
    host.runCommand.mockResolvedValueOnce({
      exitCode: null,
      stdout: "",
      stderr: "",
      timedOut: true,
    });
    mount(host);
    await vi.waitFor(() => expect(storedRuns()[0]?.status).toBe("failed"));
    expect(host.runCommand.mock.calls[0][2]).toBe(2 * 60_000);
    expect(
      storedRuns()[0].nodes.find((n) => n.nodeId === "cmd")!.error,
    ).toContain("timed out");
  });

  it("truncates long output strings before storing them", async () => {
    save([simple({ summary: { type: "string" } })]);
    const host = makeHost({
      replies: [fenced({ summary: "x".repeat(20_000) })],
    });
    mount(host);
    await vi.waitFor(() => expect(storedRuns()[0]?.status).toBe("done"));
    const output = storedRuns()[0].nodes.find((n) => n.nodeId === "dev")!
      .output as { summary: string };
    expect(output.summary.length).toBeLessThan(16_200);
    expect(output.summary).toContain("truncated");
  });

  it("keeps going in memory when the run cannot be saved", async () => {
    save([simple({})]);
    fileFailures.runs = true;
    const error = vi.spyOn(console, "error").mockImplementation(() => {});
    const host = makeHost({ replies: ["ok"] });
    mount(host);
    await vi.waitFor(() => expect(host.post).toHaveBeenCalled());
    expect(error).toHaveBeenCalled();
    // The report mentions that the history could not be written.
    expect(host.post.mock.calls[0][4]).toContain("Run error");
    await vi.waitFor(() => expect(workflowRunningId(WF)).toBeUndefined());
  });

  it("coalesces saves to the latest state", async () => {
    save([simple({})]);
    const host = makeHost({ replies: ["ok"] });
    mount(host);
    await vi.waitFor(() => expect(storedRuns()[0]?.status).toBe("done"));
    const { writeAgentFile } =
      await import("../../features/monos/model/monoFiles");
    const runWrites = vi
      .mocked(writeAgentFile)
      .mock.calls.filter(([, path]) =>
        String(path).startsWith("workflow-runs/"),
      );
    // Fewer writes than state changes, never concurrent, last one is the final state.
    expect(runWrites.length).toBeGreaterThan(0);
    const last = JSON.parse(runWrites[runWrites.length - 1][2] as string);
    expect(last[0].status).toBe("done");
  });

  it("relays approvals to the Mono chat and shows the node as waiting", async () => {
    vi.useFakeTimers({
      toFake: [
        "setTimeout",
        "setInterval",
        "clearTimeout",
        "clearInterval",
        "Date",
      ],
    });
    save([simple({})]);
    const script: Script = { replies: ["ok"], approval: true };
    const gate = hold(script);
    const host = makeHost(script);
    mount(host);
    await act(async () => {
      await vi.advanceTimersByTimeAsync(5_000);
    });
    expect(host.askApproval).toHaveBeenCalledTimes(1);
    expect(host.askApproval.mock.calls[0][0]).toBe("mono");
    expect(host.askApproval.mock.calls[0][2]).toBe(storedRuns()[0].id);
    expect(host.askApproval.mock.calls[0][3]).toBe("s1");
    expect(host.askApproval.mock.calls[0][4].approval.requestId).toBe(7);
    expect(storedRuns()[0].nodes.find((n) => n.nodeId === "dev")!.status).toBe(
      "waitingApproval",
    );

    gate.resolve();
    await act(async () => {
      await vi.advanceTimersByTimeAsync(5_000);
    });
    expect(storedRuns()[0].status).toBe("done");
    expect(host.endApprovals).toHaveBeenCalledWith("s1");
  });

  it("resumes an interrupted run on request", async () => {
    const def = simple({}, { enabled: false });
    save([def]);
    const interrupted = markInterrupted(
      startRun(def, "manual", 1_000, "run-old"),
    );
    expect(interrupted.status).toBe("interrupted");
    saveRuns([interrupted]);
    const host = makeHost({ replies: ["ok"] });
    mount(host);
    await act(async () => {
      await new Promise((r) => setTimeout(r, 30));
    });
    expect(storedRuns()[0].status).toBe("interrupted");

    resumeWorkflowRun("mono-1", WF, "run-old");
    await vi.waitFor(() => expect(storedRuns()[0].status).toBe("done"));
    expect(storedRuns()).toHaveLength(1);
    expect(host.startSession).toHaveBeenCalledTimes(1);
    expect(host.post).toHaveBeenCalledTimes(1);
  });

  it("fails a run of an invalid workflow without starting sessions", async () => {
    const def = simple({});
    save([{ ...def, nodes: def.nodes.filter((n) => n.type !== "report") }]);
    const host = makeHost({ replies: [] });
    mount(host);
    await vi.waitFor(() => expect(storedRuns()[0]?.status).toBe("failed"));
    expect(storedRuns()[0].error).toContain("Report");
    expect(host.startSession).not.toHaveBeenCalled();
  });

  it("nests under another node's session and uses its worktree", async () => {
    const def = workflow(
      [
        trigger,
        session("dev", {}),
        session(
          "review",
          {},
          { nestUnder: "dev", checkout: { worktreeOf: "dev" } },
        ),
        report,
      ],
      [
        { from: "trigger", to: "dev" },
        { from: "dev", to: "review" },
        { from: "review", to: "report" },
      ],
    );
    save([def]);
    const host = makeHost({ replies: ["ok", "ok"] });
    mount(host);
    await vi.waitFor(() => expect(storedRuns()[0]?.status).toBe("done"));
    const second = host.startSession.mock.calls[1][1] as Record<
      string,
      unknown
    >;
    expect(second).toMatchObject({
      workspaceMode: "current",
      parentSessionId: "s1",
      worktreeCwd: "/code/app-wt-s1",
      folderId: "folder-1",
    });
    expect(second.folderName).toBe(host.startSession.mock.calls[0][1].folderName);
  });
});
