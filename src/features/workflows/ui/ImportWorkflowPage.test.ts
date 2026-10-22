// @vitest-environment happy-dom
import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import type { Workflow, WorkflowNode } from "../model/workflow";
import { WORKFLOWS_MAX } from "../model/workflow";
import { exportWorkflow } from "../model/workflowYaml";
import { ImportWorkflowPage } from "./ImportWorkflowPage";
import { SCHEDULE, workflow } from "./workflowUiFixtures";

const files = new Map<string, string>();
const open = vi.hoisted(() => vi.fn(async (): Promise<string | null> => null));
const readTextFile = vi.hoisted(() => vi.fn(async (_path: string) => ""));

vi.mock("@tauri-apps/plugin-dialog", () => ({ open }));
vi.mock("../../../platform/tauri/fs", async (original) => ({
  ...(await original<object>()),
  readTextFile,
}));
vi.mock("../../monos/model/monoFiles", async (original) => ({
  ...(await original<object>()),
  readAgentFile: async (_monoId: string, path: string) => ({
    text: files.get(path) ?? null,
    hash: String(files.get(path)),
  }),
  writeAgentFile: async (_monoId: string, path: string, text: string) => {
    files.set(path, text);
    return "h";
  },
}));

let root: Root;
let container: HTMLElement;
beforeEach(() => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  files.clear();
  open.mockReset();
  readTextFile.mockReset();
  container = document.createElement("div");
  document.body.append(container);
  root = createRoot(container);
});
afterEach(() => {
  act(() => root.unmount());
  container.remove();
  vi.unstubAllGlobals();
});

const PROJECTS = [
  { path: "/work/api", name: "api" },
  { path: "/work/web", name: "web" },
];

const COMMAND = "npm run build && echo 'done' | tee out.txt";

const steps: WorkflowNode[] = [
  { id: "trigger", type: "manual", title: "Start by hand" },
  {
    id: "build",
    type: "command",
    title: "Build it",
    project: "{{inputs.project}}",
    command: COMMAND,
  },
  {
    id: "fix",
    type: "session",
    title: "Fix failures",
    project: "{{inputs.project}}",
    checkout: "root",
    prompt: "Fix what broke in {{inputs.area}}.",
    output: { summary: { type: "string", description: "One line on what changed" } },
  },
  {
    id: "ask",
    type: "askMono",
    title: "Is it fine",
    question: "Should we ship this?",
    answers: ["yes", "no"],
  },
  { id: "report", type: "report", title: "Tell the Mono" },
];

function source(extra: Partial<Workflow> = {}): Workflow {
  return workflow({
    name: "Build and fix",
    description: "Builds the project and repairs it.",
    inputs: {
      project: { type: "project" },
      area: { type: "string", description: "Where to look" },
    },
    nodes: steps,
    edges: [
      { from: "trigger", to: "build" },
      { from: "build", to: "fix" },
      { from: "fix", to: "ask" },
      { from: "ask", to: "report" },
      {
        from: "fix",
        to: "build",
        loop: { maxRounds: 3, decide: '{{fix.summary}} != "clean"' },
      },
    ],
    ...extra,
  });
}

const yamlOf = (w: Workflow = source()) => exportWorkflow(w);

async function renderPage(
  props: { workflowCount?: number; onImported?: (id: string) => void } = {},
) {
  const onImported = props.onImported ?? vi.fn();
  await act(async () =>
    root.render(
      createElement(ImportWorkflowPage, {
        monoId: "mono-1",
        color: "#9c9",
        projects: PROJECTS,
        workflowCount: props.workflowCount ?? 0,
        onBack: () => {},
        onImported,
      }),
    ),
  );
  return onImported;
}

const area = () =>
  container.querySelector<HTMLTextAreaElement>("textarea[aria-label='Workflow file text']")!;
const importButton = () =>
  container.querySelector<HTMLButtonElement>("button[data-import-paused]")!;
const text = () => container.textContent ?? "";

async function paste(value: string) {
  await act(async () => {
    const setter = Object.getOwnPropertyDescriptor(
      HTMLTextAreaElement.prototype,
      "value",
    )!.set!;
    setter.call(area(), value);
    area().dispatchEvent(new Event("input", { bubbles: true }));
  });
}

async function typeInto(label: string, value: string) {
  const input = container.querySelector<HTMLInputElement>(`input[aria-label='${label}']`)!;
  await act(async () => {
    const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!;
    setter.call(input, value);
    input.dispatchEvent(new Event("input", { bubbles: true }));
  });
}

async function chooseProject(label: string, name: string) {
  await act(async () =>
    container.querySelector<HTMLButtonElement>(`button[aria-label^='${label}:']`)!.click(),
  );
  const option = [...document.querySelectorAll<HTMLButtonElement>("[role=option]")].find(
    (o) => o.textContent === name,
  )!;
  await act(async () => option.click());
}

const section = (title: string) =>
  [...container.querySelectorAll("section")].find(
    (s) => s.querySelector("h4")?.textContent === title,
  );
const stored = (): Workflow[] => JSON.parse(files.get("workflows.json") ?? "[]");

it("lists commands verbatim", async () => {
  await renderPage();
  await paste(yamlOf());
  const commands = section("Commands this workflow runs")!;
  expect(commands).toBeDefined();
  const block = [...commands.querySelectorAll("pre")].find((pre) => pre.textContent === COMMAND)!;
  expect(block).toBeDefined();
  expect(block.className).toContain("font-mono");
  expect(commands.textContent).toContain("Build it");
});

it("Import disabled until every input has a value", async () => {
  await renderPage();
  await paste(yamlOf());
  expect(importButton().disabled).toBe(true);
  await chooseProject("project", "api");
  expect(importButton().disabled).toBe(true);
  await typeInto("area", "the parser");
  expect(importButton().disabled).toBe(false);
});

it("imported workflow is saved paused", async () => {
  const onImported = await renderPage();
  await paste(yamlOf(source({ id: "wf-sourceaaa", enabled: true })));
  await chooseProject("project", "web");
  await typeInto("area", "the parser");
  await act(async () => importButton().click());
  const [saved] = stored();
  expect(saved.enabled).toBe(false);
  expect(saved.id).toMatch(/^wf-/);
  expect(saved.id).not.toBe("wf-sourceaaa");
  expect(saved.inputValues).toEqual({ project: "/work/web", area: "the parser" });
  expect(saved.nextRunAt).toBeUndefined();
  expect(onImported).toHaveBeenCalledWith(saved.id);
});

it("shows every prompt with its step, the workflow text, step titles and loop deciders", async () => {
  await renderPage();
  await paste(yamlOf());
  const prompts = section("Prompts")!.textContent!;
  expect(prompts).toContain("Fix failures");
  expect(prompts).toContain("Fix what broke in {{inputs.area}}.");
  expect(prompts).toContain("One line on what changed");
  expect(prompts).toContain("Is it fine");
  expect(prompts).toContain("Should we ship this?");
  expect(text()).toContain("Build and fix");
  expect(text()).toContain("Builds the project and repairs it.");
  const steps = section("Steps")!.textContent!;
  for (const title of ["Start by hand", "Build it", "Fix failures", "Is it fine", "Tell the Mono"])
    expect(steps).toContain(title);
  expect(steps).toContain('{{fix.summary}} != "clean"');
});

it("collapses the prompts on request", async () => {
  await renderPage();
  await paste(yamlOf());
  const toggle = container.querySelector<HTMLButtonElement>("button[data-prompts-toggle]")!;
  expect(toggle.getAttribute("aria-expanded")).toBe("true");
  await act(async () => toggle.click());
  expect(toggle.getAttribute("aria-expanded")).toBe("false");
  expect(section("Prompts")!.textContent).not.toContain("Should we ship this?");
});

it("rejects a version-2 file with the model's message and shows nothing else", async () => {
  await renderPage();
  await paste(yamlOf().replace("monoptahWorkflow: 1", "monoptahWorkflow: 2"));
  const alert = container.querySelector("[role=alert]")!;
  expect(alert.textContent).toContain("Unsupported workflow file version: 2");
  expect(section("Commands this workflow runs")).toBeUndefined();
  expect(section("Prompts")).toBeUndefined();
  expect(importButton().disabled).toBe(true);
});

it("shows validation issues plainly without blocking the import", async () => {
  await renderPage();
  const broken = source({
    nodes: [steps[0], steps[1], steps[4]],
    edges: [{ from: "trigger", to: "build" }],
    inputs: { project: { type: "project" } },
  });
  await paste(yamlOf(broken));
  const issues = section("Problems")!;
  expect(issues).toBeDefined();
  expect(issues.textContent!.length).toBeGreaterThan("Problems".length);
  await chooseProject("project", "api");
  expect(importButton().disabled).toBe(false);
});

it("is disabled and explains itself at the workflow limit", async () => {
  await renderPage({ workflowCount: WORKFLOWS_MAX });
  await paste(yamlOf(source({ inputs: {}, nodes: [steps[0], steps[4]], edges: [{ from: "trigger", to: "report" }] })));
  expect(importButton().disabled).toBe(true);
  expect(text()).toContain(`at most ${WORKFLOWS_MAX} workflows`);
});

it("re-checks the limit when saving, over what is on disk", async () => {
  const full = Array.from({ length: WORKFLOWS_MAX }, (_, i) =>
    workflow({ id: `wf-full${i}` }),
  );
  files.set("workflows.json", JSON.stringify(full));
  const onImported = await renderPage({ workflowCount: 1 });
  await paste(yamlOf(source({ inputs: {}, nodes: [steps[0], steps[4]], edges: [{ from: "trigger", to: "report" }] })));
  await act(async () => importButton().click());
  expect(stored()).toHaveLength(WORKFLOWS_MAX);
  expect(onImported).not.toHaveBeenCalled();
  expect(container.querySelector("[role=alert]")!.textContent).toContain(`at most ${WORKFLOWS_MAX}`);
});

it("reads a picked file into the same preview", async () => {
  open.mockResolvedValue("/home/me/flow.monoptah-workflow.yaml");
  readTextFile.mockResolvedValue(yamlOf());
  await renderPage();
  await act(async () =>
    container.querySelector<HTMLButtonElement>("button[data-choose-file]")!.click(),
  );
  expect(readTextFile).toHaveBeenCalledWith("/home/me/flow.monoptah-workflow.yaml");
  expect(section("Commands this workflow runs")!.textContent).toContain(COMMAND);
  expect(area().value).toBe(yamlOf());
});

it("keeps what is shown when the file dialog is cancelled", async () => {
  open.mockResolvedValue(null);
  await renderPage();
  await paste(yamlOf());
  await act(async () =>
    container.querySelector<HTMLButtonElement>("button[data-choose-file]")!.click(),
  );
  expect(readTextFile).not.toHaveBeenCalled();
  expect(section("Prompts")).toBeDefined();
});

it("says why a file could not be read", async () => {
  open.mockResolvedValue("/home/me/big.yaml");
  readTextFile.mockRejectedValue("File is too large to edit (maximum 8 MB).");
  await renderPage();
  await act(async () =>
    container.querySelector<HTMLButtonElement>("button[data-choose-file]")!.click(),
  );
  expect(container.querySelector("[role=alert]")!.textContent).toContain(
    "File is too large to edit",
  );
});

it("shows the model's error for a file over 512 KiB", async () => {
  open.mockResolvedValue("/home/me/big.yaml");
  readTextFile.mockResolvedValue("a: 1\n".repeat(120_000));
  await renderPage();
  await act(async () =>
    container.querySelector<HTMLButtonElement>("button[data-choose-file]")!.click(),
  );
  expect(container.querySelector("[role=alert]")!.textContent).toContain("too large");
  expect(importButton().disabled).toBe(true);
});

it("anchors an interval schedule the way the CLI import does", async () => {
  await renderPage();
  const timed = source({
    inputs: {},
    nodes: [
      {
        id: "trigger",
        type: "schedule",
        schedule: { ...SCHEDULE, scheduleKind: "interval", intervalHours: 4 },
      },
      steps[4],
    ],
    edges: [{ from: "trigger", to: "report" }],
  });
  await paste(yamlOf(timed));
  await act(async () => importButton().click());
  const [saved] = stored();
  const trigger = saved.nodes[0];
  expect(trigger.type === "schedule" && trigger.schedule.intervalFrom).toMatch(
    /^\d{4}-\d{2}-\d{2}$/,
  );
  expect(saved.enabled).toBe(false);
});
