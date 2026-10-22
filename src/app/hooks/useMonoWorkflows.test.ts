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
  exportWorkflow,
  finishImport,
  previewImport,
} from "../../features/workflows/model/workflowYaml";
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
/** Run-file writes in flight, to show saves never overlap. */
const runWrites = { delayMs: 0, inflight: 0, max: 0, texts: [] as string[] };

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
      const result = {
        text: file?.text ?? null,
        hash: String(file?.hash ?? 0),
      };
      return result;
    }),
    writeAgentFile: vi.fn(
      async (_monoId: string, path: string, text: string, expected: string) => {
        if (fileFailures.runs && path.startsWith("workflow-runs/"))
          throw new Error("file too large");
        const tracked = path.startsWith("workflow-runs/");
        if (tracked) {
          runWrites.inflight++;
          runWrites.max = Math.max(runWrites.max, runWrites.inflight);
          if (runWrites.delayMs)
            await new Promise((r) => setTimeout(r, runWrites.delayMs));
          runWrites.inflight--;
          runWrites.texts.push(text);
        }
        const current = files.get(path)?.hash ?? 0;
        if (expected !== String(current)) throw new actual.MonoFileConflict();
        files.set(path, { text, hash: current + 1 });
        return String(current + 1);
      },
    ),
  };
});

const { useWorkflowDriver } = await import("./workflowDriver");
const { createLiveRunRegistry } = await import("./workflowLiveRuns");
const { readAgentFile } = await import("../../features/monos/model/monoFiles");
const { useMonoWorkflows, resumeWorkflowRun } =
  await import("./useMonoWorkflows");
const { checkWorkflowsNow, requestWorkflowStop, workflowRunningId } =
  await import("../../features/workflows/model/workflowStore");
const {
  setWorkflowRunning,
  clearWorkflowStop,
  stopWorkflowRun,
  takeWorkflowRequests,
  workflowStopRequested,
} = await import("../../features/workflows/model/workflowStore");
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
          folderId: `folder:${req.project}`,
          settled: turn(id, req.prompt),
        };
      },
    ),
    send: vi.fn(turn),
    stop: vi.fn(async () => {}),
    runCommand: vi.fn(async (_cwd: string, command: string) =>
      command.includes("bad")
        ? {
            exitCode: 1,
            stdout: "",
            stderr: "boom",
            timedOut: false,
            cancelled: false,
          }
        : {
            exitCode: 0,
            stdout: "fine",
            stderr: "",
            timedOut: false,
            cancelled: false,
          },
    ),
    cancelCommand: vi.fn(async (_id: string) => true),
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
      input === "/code/app" || input === "/code/lib" ? input : undefined,
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
  Object.assign(runWrites, { delayMs: 0, inflight: 0, max: 0, texts: [] });
  release = [];
  localStorage.clear();
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

function mount(host: ReturnType<typeof makeHost>, driver = true) {
  function Probe() {
    useMonoWorkflows(host, driver);
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
      folderId: "folder:/code/app",
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

  it("an imported manual workflow runs on request and posts its report", async () => {
    const source = workflow(
      [
        { id: "trigger", type: "manual" },
        session("dev", { summary: { type: "string", inReport: true } }),
        report,
      ],
      [
        { from: "trigger", to: "dev" },
        { from: "dev", to: "report" },
      ],
      { nextRunAt: undefined },
    );
    const read = previewImport(exportWorkflow(source));
    if (!read.ok) throw new Error(read.error);
    const imported = finishImport(read.preview, { repo: "/code/app" }, Date.now());
    expect(imported.enabled).toBe(false);
    save([{ ...imported, id: WF, runRequested: true }]);
    const host = makeHost({ replies: [fenced({ summary: "echoed" })] });
    mount(host);
    await vi.waitFor(() => expect(host.post).toHaveBeenCalled());
    const [, , , title, text] = host.post.mock.calls[0];
    expect(title).toBe("Ship it · completed");
    expect(text).toContain("echoed");
    await vi.waitFor(() => expect(storedRuns()[0]?.status).toBe("done"));
    expect(storedRuns()[0].trigger).toBe("manual");
    expect(storedRuns()[0].nodes.find((n) => n.nodeId === "dev")).toMatchObject({
      status: "done",
      output: { summary: "echoed" },
    });
    // A manual workflow stays paused; only the request ran it.
    expect(stored()[0].enabled).toBe(false);
  });

  it("skips a tick while previous run is running", async () => {
    const info = vi.spyOn(console, "info").mockImplementation(() => {});
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
    expect(info).toHaveBeenCalledWith(expect.stringContaining("still running"));

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
    // An interrupted run says nothing until it is resumed.
    expect(host.post).not.toHaveBeenCalled();
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
    // A stopped run reports once.
    expect(host.post).toHaveBeenCalledTimes(1);
    expect(host.post.mock.calls[0][3]).toBe("Ship it · stopped");
    expect(host.post.mock.calls[0][4]).toContain("stopped");
    await vi.waitFor(() => expect(workflowRunningId(WF)).toBeUndefined());
    expect(host.post).toHaveBeenCalledTimes(1);
  });

  const commandWorkflow = () =>
    workflow(
      [
        trigger,
        {
          id: "cmd",
          type: "command",
          project: "{{inputs.repo}}",
          command: "./deploy.sh",
        },
        report,
      ],
      [
        { from: "trigger", to: "cmd" },
        { from: "cmd", to: "report" },
      ],
    );

  it("stop cancels the running command and the run is released only after it settled", async () => {
    save([commandWorkflow()]);
    const host = makeHost({ replies: [] });
    const finish = deferred();
    host.runCommand.mockImplementationOnce(async () => {
      await finish.promise;
      return {
        exitCode: null,
        stdout: "partial",
        stderr: "",
        timedOut: false,
        cancelled: true,
      };
    });
    mount(host);
    await vi.waitFor(() => expect(host.runCommand).toHaveBeenCalledTimes(1));
    const id = host.runCommand.mock.calls[0][3];

    requestWorkflowStop(WF);
    await vi.waitFor(() => expect(host.cancelCommand).toHaveBeenCalledWith(id));
    await vi.waitFor(() => expect(storedRuns()[0].status).toBe("stopped"));
    expect(storedRuns()[0].nodes.find((n) => n.nodeId === "cmd")!.status).toBe(
      "stopped",
    );
    // The command is still alive: another run of the workflow must not start yet.
    await new Promise((r) => setTimeout(r, 100));
    expect(workflowRunningId(WF)).toBeDefined();

    finish.resolve();
    await vi.waitFor(() => expect(workflowRunningId(WF)).toBeUndefined());
    expect(host.post).toHaveBeenCalledTimes(1);
    expect(storedRuns()[0].status).toBe("stopped");
  });

  it("stop does not wait for a pending Ask Mono question", async () => {
    save([
      workflow(
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
      ),
    ]);
    const host = makeHost({ replies: [] });
    host.askMono.mockImplementationOnce(() => new Promise<string>(() => {}));
    mount(host);
    await vi.waitFor(() => expect(host.askMono).toHaveBeenCalledTimes(1));

    requestWorkflowStop(WF);
    await vi.waitFor(() => expect(storedRuns()[0].status).toBe("stopped"));
    await vi.waitFor(() => expect(workflowRunningId(WF)).toBeUndefined());
  });

  it("stop does not wait for a session that never finishes starting", async () => {
    save([simple()]);
    const host = makeHost({ replies: ["ok"] });
    let started!: (value: {
      sessionId: string;
      settled: Promise<ControlOutcome>;
    }) => void;
    host.startSession.mockImplementationOnce(
      () => new Promise((resolve) => (started = resolve)),
    );
    mount(host);
    await vi.waitFor(() => expect(host.startSession).toHaveBeenCalledTimes(1));

    requestWorkflowStop(WF);
    await vi.waitFor(() => expect(storedRuns()[0].status).toBe("stopped"));
    await vi.waitFor(() => expect(workflowRunningId(WF)).toBeUndefined());

    // A start that finally comes through after the stop is still ended.
    started({ sessionId: "late", settled: new Promise(() => {}) });
    await vi.waitFor(() => expect(host.stop).toHaveBeenCalledWith("late"));
  });

  it("a command cancelled from outside the run fails its node", async () => {
    save([commandWorkflow()]);
    const host = makeHost({ replies: [] });
    host.runCommand.mockResolvedValueOnce({
      exitCode: null,
      stdout: "",
      stderr: "",
      timedOut: false,
      cancelled: true,
    });
    mount(host);
    await vi.waitFor(() => expect(storedRuns()[0]?.status).toBe("failed"));
    expect(
      storedRuns()[0].nodes.find((n) => n.nodeId === "cmd")!.error,
    ).toContain("cancelled");
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
      expect.any(String),
    );
    // Every command gets its own id.
    const ids = host.runCommand.mock.calls.map((call) => call[3]);
    expect(new Set(ids).size).toBe(ids.length);
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
      cancelled: false,
    });
    mount(host);
    await vi.waitFor(() => expect(storedRuns()[0]?.status).toBe("failed"));
    expect(host.runCommand.mock.calls[0][2]).toBe(2 * 60_000);
    expect(
      storedRuns()[0].nodes.find((n) => n.nodeId === "cmd")!.error,
    ).toContain("timed out");
  });

  it("a command the host refuses fails its node with the host's message", async () => {
    const def = workflow(
      [
        trigger,
        {
          id: "cmd",
          type: "command",
          project: "{{inputs.repo}}",
          command: "echo hi",
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
    // Tauri rejects with a bare string, as the Windows build does for command nodes.
    host.runCommand.mockRejectedValueOnce(
      "Command nodes are not supported on Windows yet",
    );
    mount(host);
    await vi.waitFor(() => expect(storedRuns()[0]?.status).toBe("failed"));
    expect(
      storedRuns()[0].nodes.find((n) => n.nodeId === "cmd")!.error,
    ).toContain("Command nodes are not supported on Windows yet");
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

  it("coalesces saves to the latest state and never writes concurrently", async () => {
    save([simple({})]);
    runWrites.delayMs = 20;
    const host = makeHost({ replies: ["ok"] });
    mount(host);
    await vi.waitFor(() => expect(storedRuns()[0]?.status).toBe("done"));
    await vi.waitFor(() => expect(workflowRunningId(WF)).toBeUndefined());
    // Start, node started, then everything that happened while that write was going.
    expect(runWrites.max).toBe(1);
    expect(runWrites.texts.length).toBeGreaterThan(1);
    expect(runWrites.texts.length).toBeLessThanOrEqual(3);
    const states = runWrites.texts.map(
      (text) => (JSON.parse(text) as WorkflowRun[])[0],
    );
    expect(states[states.length - 1].status).toBe("done");
    expect(
      states[states.length - 1].nodes.find((n) => n.nodeId === "dev"),
    ).toMatchObject({ status: "done", sessionId: "s1" });
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

    await resumeWorkflowRun("mono-1", WF, "run-old");
    await vi.waitFor(() => expect(storedRuns()[0].status).toBe("done"));
    expect(storedRuns()).toHaveLength(1);
    expect(host.startSession).toHaveBeenCalledTimes(1);
    expect(host.post).toHaveBeenCalledTimes(1);
    // The request is answered, so it is not repeated.
    expect(stored()[0].resumeRunId).toBeUndefined();
  });

  it("a resume asked for in another window is picked up by the driver on its next look", async () => {
    const def = simple({}, { enabled: false });
    save([def]);
    saveRuns([markInterrupted(startRun(def, "manual", 1_000, "run-old"))]);
    const host = makeHost({ replies: ["ok"] });
    mount(host);
    await act(async () => {
      await new Promise((r) => setTimeout(r, 30));
    });
    expect(storedRuns()[0].status).toBe("interrupted");
    // Another window can only write the request; the driver's own look finds it.
    save([{ ...stored()[0], resumeRunId: "run-old" }]);
    checkWorkflowsNow();
    await vi.waitFor(() => expect(storedRuns()[0].status).toBe("done"));
    expect(stored()[0].resumeRunId).toBeUndefined();
  });

  it("fails a run of an invalid workflow without starting sessions", async () => {
    const def = simple({});
    save([{ ...def, nodes: def.nodes.filter((n) => n.type !== "report") }]);
    const host = makeHost({ replies: [] });
    mount(host);
    await vi.waitFor(() => expect(storedRuns()[0]?.status).toBe("failed"));
    expect(storedRuns()[0].error).toContain("Report");
    expect(host.startSession).not.toHaveBeenCalled();
    // Scheduled breakage is visible in the chat, not only in the run history.
    expect(host.post).toHaveBeenCalledTimes(1);
    expect(host.post.mock.calls[0][3]).toBe("Ship it · failed");
    expect(host.post.mock.calls[0][4]).toContain("could not start");
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
      folderId: "folder:/code/app",
    });
    expect(second.folderName).toBe(
      host.startSession.mock.calls[0][1].folderName,
    );
  });

  it("a window that is not the driver never ticks, sweeps, resumes or claims", async () => {
    const def = simple({}, { stopRequested: true, resumeRunId: "run-old" });
    save([def]);
    saveRuns([startRun(def, "manual", 1_000, "run-old")]);
    const host = makeHost({ replies: ["ok"] });
    mount(host, false);
    checkWorkflowsNow();
    await act(async () => {
      await new Promise((r) => setTimeout(r, 80));
    });
    expect(host.startSession).not.toHaveBeenCalled();
    expect(host.post).not.toHaveBeenCalled();
    // The running run is not swept to interrupted and the requests are left for the driver.
    expect(storedRuns()[0].status).toBe("running");
    expect(stored()[0]).toMatchObject({
      stopRequested: true,
      resumeRunId: "run-old",
    });
    expect(stored()[0].nextRunAt).toBeLessThan(Date.now());
    expect(workflowRunningId(WF)).toBeUndefined();
  });

  it("a window that lost the lease still tells the others its run is live and applies a stop from the file", async () => {
    vi.useFakeTimers({
      toFake: [
        "setTimeout",
        "setInterval",
        "clearTimeout",
        "clearInterval",
        "Date",
      ],
    });
    save([simple()]);
    const script: Script = { replies: ["ok"] };
    hold(script);
    const host = makeHost(script);
    mount(host, true);
    await act(async () => {
      await vi.advanceTimersByTimeAsync(1_000);
    });
    expect(host.startSession).toHaveBeenCalledTimes(1);
    const runId = storedRuns()[0].id;
    const entry = () =>
      JSON.parse(localStorage.getItem("monocode.workflowLiveRuns") ?? "{}")[
        runId
      ];
    expect(entry()).toMatchObject({ workflowId: WF });
    const first = entry().at;

    // The lease goes to another window: this hook is switched off, the run is not.
    mount(host, false);
    await act(async () => {
      await vi.advanceTimersByTimeAsync(12_000);
    });
    expect(entry().at).toBeGreaterThan(first);
    expect(workflowRunningId(WF)).toBe(runId);

    // The new driver can only write the request; the run's own window applies it and clears it.
    save([{ ...stored()[0], stopRequested: true }]);
    await act(async () => {
      await vi.advanceTimersByTimeAsync(6_000);
    });
    expect(storedRuns()[0].status).toBe("stopped");
    expect(host.stop).toHaveBeenCalledWith("s1");
    expect(stored()[0].stopRequested).toBeUndefined();
    expect(entry()).toBeUndefined();
  });

  it("leaves alone a run another window is still driving", async () => {
    vi.spyOn(console, "info").mockImplementation(() => {});
    const def = simple({}, { stopRequested: true });
    save([def]);
    const running = startRun(def, "manual", 1_000, "run-a");
    saveRuns([running]);
    // Window A lost the lease but its run is live: it refreshed its entry just now.
    createLiveRunRegistry({ storage: localStorage, owner: "window-a" }).touch(
      "run-a",
      WF,
    );
    const host = makeHost({ replies: ["ok"] });
    mount(host);
    await act(async () => {
      await new Promise((r) => setTimeout(r, 100));
    });
    // Not swept to interrupted, and the due schedule did not start a second run.
    expect(storedRuns()[0].status).toBe("running");
    expect(host.startSession).not.toHaveBeenCalled();
    expect(stored()[0].nextRunAt).toBeGreaterThan(Date.now());
    // The stop written for A's run is still there for A to apply.
    expect(stored()[0].stopRequested).toBe(true);

    // A Resume is refused while A is driving the run (here as if it had been marked interrupted).
    saveRuns([markInterrupted(running)]);
    await resumeWorkflowRun("mono-1", WF, "run-a");
    await act(async () => {
      await new Promise((r) => setTimeout(r, 100));
    });
    expect(host.startSession).not.toHaveBeenCalled();
    expect(storedRuns()[0].status).toBe("interrupted");
    expect(stored()[0].resumeRunId).toBeUndefined();
    // Taking the resume request did not take the stop request meant for A's live run.
    expect(stored()[0].stopRequested).toBe(true);
  });

  it("takes a run over once its window stopped refreshing it", async () => {
    const def = simple({}, { enabled: false });
    save([def]);
    saveRuns([startRun(def, "manual", 1_000, "run-a")]);
    // Last refreshed 31 s ago: that window is gone.
    createLiveRunRegistry({
      storage: localStorage,
      owner: "window-a",
      now: () => Date.now() - 31_000,
    }).touch("run-a", WF);
    const host = makeHost({ replies: ["ok"] });
    mount(host);
    await vi.waitFor(() => expect(storedRuns()[0].status).toBe("interrupted"));
    await resumeWorkflowRun("mono-1", WF, "run-a");
    await vi.waitFor(() => expect(storedRuns()[0].status).toBe("done"));
    expect(host.startSession).toHaveBeenCalledTimes(1);
  });

  it("keeps looking for a closed window's run until its entry is gone, then stops looking", async () => {
    const def = simple({}, { enabled: false });
    save([def]);
    saveRuns([startRun(def, "manual", 1_000, "run-a")]);
    // The window closed a moment ago: its entry has not gone stale yet.
    const closing = createLiveRunRegistry({
      storage: localStorage,
      owner: "window-a",
    });
    closing.touch("run-a", WF);
    const host = makeHost({ replies: ["ok"] });
    const runFileReads = () =>
      vi
        .mocked(readAgentFile)
        .mock.calls.filter(([, path]) =>
          String(path).startsWith("workflow-runs/"),
        ).length;
    const settle = (ms: number) =>
      act(async () => {
        await new Promise((r) => setTimeout(r, ms));
      });
    mount(host);
    await settle(80);
    // The first look skipped the run, and nothing was written for it.
    expect(storedRuns()[0].status).toBe("running");
    expect(runWrites.texts).toHaveLength(0);

    // Later ticks look again while the entry is fresh, still without writing.
    const reads = runFileReads();
    checkWorkflowsNow();
    await settle(80);
    expect(runFileReads()).toBeGreaterThan(reads);
    expect(storedRuns()[0].status).toBe("running");
    expect(runWrites.texts).toHaveLength(0);

    // The entry goes (the window finished closing): the next tick marks the run.
    closing.remove("run-a");
    checkWorkflowsNow();
    await vi.waitFor(() => expect(storedRuns()[0].status).toBe("interrupted"));

    // Nothing was skipped this time, so later ticks do not look at the runs again.
    await settle(30);
    const settled = runFileReads();
    const writes = runWrites.texts.length;
    checkWorkflowsNow();
    await settle(60);
    checkWorkflowsNow();
    await settle(60);
    expect(runFileReads()).toBe(settled);
    expect(runWrites.texts).toHaveLength(writes);

    // And the run can be resumed.
    await resumeWorkflowRun("mono-1", WF, "run-a");
    await vi.waitFor(() => expect(storedRuns()[0].status).toBe("done"));
  });

  it("an entry that goes stale also lets the next tick mark the run", async () => {
    const def = simple({}, { enabled: false });
    save([def]);
    saveRuns([startRun(def, "manual", 1_000, "run-a")]);
    // Refreshed 29 s ago: still fresh now, stale a couple of seconds from now.
    createLiveRunRegistry({
      storage: localStorage,
      owner: "window-a",
      now: () => Date.now() - 29_000,
    }).touch("run-a", WF);
    const host = makeHost({ replies: [] });
    mount(host);
    await act(async () => {
      await new Promise((r) => setTimeout(r, 60));
    });
    expect(storedRuns()[0].status).toBe("running");
    await act(async () => {
      await new Promise((r) => setTimeout(r, 1_100));
    });
    await act(async () => {
      await new Promise((r) => setTimeout(r, 1_100));
    });
    checkWorkflowsNow();
    await vi.waitFor(() => expect(storedRuns()[0].status).toBe("interrupted"));
  });

  it("still ends a run's bookkeeping when withdrawing its approvals fails", async () => {
    save([simple()]);
    const host = makeHost({ replies: ["ok"] });
    host.endApprovals.mockImplementation(() => {
      throw new Error("boom");
    });
    vi.spyOn(console, "warn").mockImplementation(() => {});
    mount(host);
    await vi.waitFor(() => expect(host.post).toHaveBeenCalled());
    await vi.waitFor(() => expect(workflowRunningId(WF)).toBeUndefined());
    expect(
      JSON.parse(localStorage.getItem("monocode.workflowLiveRuns") ?? "{}"),
    ).toEqual({});
  });

  it("gives each resume request to exactly one of several drivers", async () => {
    save([simple({}, { enabled: false, resumeRunId: "run-old" })]);
    const results = await Promise.all([
      takeWorkflowRequests("mono-1", () => false),
      takeWorkflowRequests("mono-1", () => false),
      takeWorkflowRequests("mono-1", () => false),
    ]);
    expect(results.flat()).toEqual([
      { monoId: "mono-1", workflowId: WF, runId: "run-old" },
    ]);
    expect(stored()[0].resumeRunId).toBeUndefined();
  });

  it("stopWorkflowRun writes the request for the driver and stops at once only where the run is", async () => {
    save([simple({}, { enabled: false })]);
    // A window that drives no run leaves no stop flag behind in memory.
    await stopWorkflowRun("mono-1", WF);
    expect(stored()[0].stopRequested).toBe(true);
    expect(workflowStopRequested(WF)).toBe(false);
    // The window that is driving the run stops it right away.
    setWorkflowRunning(WF, "run-x");
    await stopWorkflowRun("mono-1", WF);
    expect(workflowStopRequested(WF)).toBe(true);
  });

  it("clears a stop request when no run is live", async () => {
    save([simple({}, { enabled: false, stopRequested: true })]);
    const host = makeHost({ replies: [] });
    mount(host);
    await vi.waitFor(() => expect(stored()[0].stopRequested).toBeUndefined());
    expect(workflowStopRequested(WF)).toBe(false);
  });

  it("marks the runs of a window that is gone when it takes over as the driver", async () => {
    const def = simple({}, { enabled: false });
    save([def]);
    saveRuns([startRun(def, "manual", 1_000, "run-dead")]);
    const host = makeHost({ replies: [] });
    mount(host, false);
    await act(async () => {
      await new Promise((r) => setTimeout(r, 40));
    });
    expect(storedRuns()[0].status).toBe("running");
    // Becoming the driver sweeps, though this window swept (or never swept) before.
    mount(host, true);
    await vi.waitFor(() => expect(storedRuns()[0].status).toBe("interrupted"));
  });

  it("takes over from a stale lease and drives; a fresh lease of another window blocks it", async () => {
    const def = simple({}, { enabled: false });
    save([def]);
    saveRuns([startRun(def, "manual", 1_000, "run-dead")]);
    const host = makeHost({ replies: [] });
    function Elected() {
      useMonoWorkflows(host, useWorkflowDriver());
      return null;
    }
    // Another window holds the lease and renewed it just now: this one must stay out.
    localStorage.setItem(
      "monocode.workflowDriver",
      JSON.stringify({ id: "other", at: Date.now() }),
    );
    act(() => root.render(createElement(Elected)));
    await act(async () => {
      await new Promise((r) => setTimeout(r, 60));
    });
    expect(storedRuns()[0].status).toBe("running");
    act(() => root.unmount());

    // That window is gone: its lease is stale, so a new window takes over and sweeps.
    localStorage.setItem(
      "monocode.workflowDriver",
      JSON.stringify({ id: "other", at: Date.now() - 31_000 }),
    );
    root = createRoot(document.createElement("div"));
    act(() => root.render(createElement(Elected)));
    await vi.waitFor(() => expect(storedRuns()[0].status).toBe("interrupted"));
    expect(
      JSON.parse(localStorage.getItem("monocode.workflowDriver")!).id,
    ).not.toBe("other");
  });

  it("does not launch starts queued behind another one once the run is stopped", async () => {
    const def = workflow(
      [
        trigger,
        session("fetch", { tickets: { type: "list" } }),
        { id: "each", type: "forEach", in: "{{fetch.tickets}}", as: "item" },
        { ...session("work", {}), parent: "each" },
        report,
      ],
      [
        { from: "trigger", to: "fetch" },
        { from: "fetch", to: "each" },
        { from: "each", to: "report" },
      ],
    );
    save([def]);
    const host = makeHost({ replies: [fenced({ tickets: ["a", "b"] })] });
    const gate = deferred();
    release.push(gate.resolve);
    const original = host.startSession.getMockImplementation()!;
    let calls = 0;
    host.startSession.mockImplementation(async (monoId, req) => {
      // The first branch's start is slow, so the second one waits in the queue behind it.
      if (++calls === 2) await gate.promise;
      return original(monoId, req);
    });
    mount(host);
    await vi.waitFor(() => expect(calls).toBe(2));

    requestWorkflowStop(WF);
    await vi.waitFor(() => expect(storedRuns()[0].status).toBe("stopped"));
    gate.resolve();
    await vi.waitFor(() => expect(workflowRunningId(WF)).toBeUndefined());
    await act(async () => {
      await new Promise((r) => setTimeout(r, 50));
    });
    // fetch and the first branch only; the first branch's session is stopped as it appears.
    expect(host.startSession).toHaveBeenCalledTimes(2);
    expect(calls).toBe(2);
    expect(host.stop).toHaveBeenCalledWith("s2");
  });

  it("keeps one folder per project for a run", async () => {
    const def = workflow(
      [
        trigger,
        session("dev", {}),
        session("docs", {}, { project: "{{inputs.lib}}" }),
        session("more", {}, { project: "{{inputs.lib}}" }),
        report,
      ],
      [
        { from: "trigger", to: "dev" },
        { from: "dev", to: "docs" },
        { from: "docs", to: "more" },
        { from: "more", to: "report" },
      ],
      {
        inputs: { repo: { type: "project" }, lib: { type: "project" } },
        inputValues: { repo: "/code/app", lib: "/code/lib" },
      },
    );
    save([def]);
    const host = makeHost({ replies: ["ok", "ok", "ok"] });
    mount(host);
    await vi.waitFor(() => expect(storedRuns()[0]?.status).toBe("done"));
    const requests = host.startSession.mock.calls.map(
      (call) =>
        call[1] as {
          project: string;
          folderId?: string;
          folderName?: string;
        },
    );
    // First session: no folder yet, so it names one.
    expect(requests[0]).toMatchObject({ project: "/code/app" });
    expect(requests[0].folderId).toBeUndefined();
    // First session in another project: the name makes a folder there.
    expect(requests[1].project).toBe("/code/lib");
    expect(requests[1].folderName).toBe(requests[0].folderName);
    // Later sessions reuse the folder of their own project, not the first project's.
    expect(requests[2]).toMatchObject({
      project: "/code/lib",
      folderId: "folder:/code/lib",
    });
    expect(storedRuns()[0].folderId).toBe("folder:/code/app");
  });

  it("caps the reason the Mono gives", async () => {
    const def = workflow(
      [
        trigger,
        {
          id: "ask",
          type: "askMono",
          question: "Ship?",
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
    host.askMono.mockResolvedValueOnce(
      fenced({ answer: "yes", reason: "r".repeat(20_000) }),
    );
    mount(host);
    await vi.waitFor(() => expect(storedRuns()[0]?.status).toBe("done"));
    const output = storedRuns()[0].nodes.find((n) => n.nodeId === "ask")!
      .output as { answer: string; reason: string };
    expect(output.answer).toBe("yes");
    expect(output.reason.length).toBeLessThan(16_200);
    expect(output.reason).toContain("truncated");
  });
});
