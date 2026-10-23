// @vitest-environment happy-dom
import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import type { Workflow } from "../../model/workflow";
import { ticketWorkflow } from "../workflowUiFixtures";
import { loopLabel } from "./LoopEdge";
import { WorkflowBoard } from "./WorkflowBoard";

const files = new Map<string, string>();
let writeFailure: Error | undefined;
const TOO_LARGE = new Error(
  "The workflows are too large to save (over 2 MiB in total). Remove a workflow or shorten its steps.",
);
vi.mock("../../../monos/model/monoFiles", async (original) => ({
  ...(await original<object>()),
  readAgentFile: async (_monoId: string, path: string) => ({
    text: files.get(path) ?? null,
    hash: String(files.get(path)?.length ?? 0),
  }),
  writeAgentFile: async (_monoId: string, path: string, text: string) => {
    if (writeFailure) throw writeFailure;
    files.set(path, text);
    return "h";
  },
}));

/** React Flow measures with these; happy-dom has neither. */
class FakeObserver {
  observe() {}
  unobserve() {}
  disconnect() {}
}
class FakeMatrix {
  m22 = 1;
  constructor(_transform?: string) {}
}

let root: Root;
let container: HTMLElement;
beforeEach(() => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  vi.stubGlobal("ResizeObserver", FakeObserver);
  vi.stubGlobal("DOMMatrixReadOnly", FakeMatrix);
  files.clear();
  writeFailure = undefined;
  container = document.createElement("div");
  container.style.width = "420px";
  container.style.height = "800px";
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

const stored = (): Workflow[] => JSON.parse(files.get("workflows.json") ?? "[]");

async function renderBoard(
  w: Workflow | undefined,
  props: { onBack?: () => void; onReview?: (intent: "enable" | "run") => void } = {},
) {
  if (w) files.set("workflows.json", JSON.stringify([w]));
  await act(async () =>
    root.render(
      createElement(WorkflowBoard, {
        monoId: "mono-1",
        workflow: w,
        color: "#9c9",
        projects: PROJECTS,
        onBack: props.onBack ?? (() => {}),
        onReview: props.onReview ?? (() => {}),
      }),
    ),
  );
}

const button = (label: string) =>
  container.querySelector<HTMLButtonElement>(`button[aria-label="${label}"]`)!;
const buttonText = (text: string, within: ParentNode = document.body) =>
  [...within.querySelectorAll("button")].find(
    (b) => b.textContent?.trim() === text,
  );
const dialog = () => document.body.querySelector("[role='dialog']")!;
const nodes = () => [...container.querySelectorAll("[data-board-node]")];

function type(input: HTMLInputElement | HTMLTextAreaElement, value: string) {
  const proto =
    input instanceof HTMLTextAreaElement
      ? HTMLTextAreaElement.prototype
      : HTMLInputElement.prototype;
  Object.getOwnPropertyDescriptor(proto, "value")!.set!.call(input, value);
  input.dispatchEvent(new Event("input", { bubbles: true }));
}

it("renders one node per workflow node", async () => {
  await renderBoard(ticketWorkflow());
  expect(nodes().map((n) => n.getAttribute("data-board-node")).sort()).toEqual(
    ["dev", "each", "fetch", "report", "review", "trigger"],
  );
  // No React Flow chrome: no controls panel, no minimap, no attribution.
  expect(container.querySelector(".react-flow__controls")).toBeNull();
  expect(container.querySelector(".react-flow__minimap")).toBeNull();
  expect(container.querySelector(".react-flow__attribution")).toBeNull();
});

it("labels a loop by who decides and how many rounds", () => {
  expect(loopLabel({ maxRounds: 3, decide: "mono" })).toBe("Mono decides · max 3");
  expect(loopLabel({ maxRounds: 5, decide: "{{review.score}} < 3" })).toBe(
    "If {{review.score}} < 3 · max 5",
  );
  expect(loopLabel({ maxRounds: 2, decide: "x".repeat(60) })).toMatch(/^If x{27}… · max 2$/);
});

it("Save disabled for enabled workflow with issues", async () => {
  const w = ticketWorkflow();
  // The prompt of the first session names an input that does not exist.
  const broken: Workflow = {
    ...w,
    nodes: w.nodes.map((n) =>
      n.id === "fetch" && n.type === "session"
        ? { ...n, prompt: "List {{inputs.nope}}" }
        : n,
    ),
  };
  await renderBoard(broken);
  const name = container.querySelector<HTMLInputElement>("input[aria-label='Workflow name']")!;
  await act(async () => type(name, "Edited"));
  // Changed, yet blocked: it is enabled and has a problem.
  expect(button("Save").disabled).toBe(true);
  const issue = container.querySelector(
    "[data-board-node='fetch'] [data-node-issue]",
  );
  expect(issue?.textContent).toContain("inputs.nope");
  expect(container.querySelector("[data-board-issues]")?.textContent).toContain(
    "1 problem",
  );
  // Saving it paused is still possible.
  await act(async () => buttonText("Save paused")!.click());
  expect(stored()[0].enabled).toBe(false);
});

it("flags a session whose model is not in the catalog and blocks saving it enabled", async () => {
  const w = ticketWorkflow();
  // The free text an older build accepted: not an app model id.
  const typed: Workflow = {
    ...w,
    nodes: w.nodes.map((n) =>
      n.id === "fetch" && n.type === "session" ? { ...n, model: "claude-sonnet-5-5" } : n,
    ),
  };
  await renderBoard(typed);
  const issue = container.querySelector("[data-board-node='fetch'] [data-node-issue]");
  expect(issue?.textContent).toContain('Model "claude-sonnet-5-5" is not available');
  expect(container.querySelector("[data-board-issues]")?.textContent).toContain("1 problem");
  const name = container.querySelector<HTMLInputElement>("input[aria-label='Workflow name']")!;
  await act(async () => type(name, "Edited"));
  expect(button("Save").disabled).toBe(true);
  await act(async () => buttonText("Save paused")!.click());
  expect(stored()[0].enabled).toBe(false);
});

it("saves a valid workflow, keeping the fields the app manages", async () => {
  const w = ticketWorkflow({
    runRequested: true,
    stopRequested: true,
    resumeRunId: "run-9",
    lastRunId: "run-8",
    importedFrom: "a.yaml",
    nextRunAt: 123,
  });
  await renderBoard(w);
  expect(button("Save").disabled).toBe(true);
  const name = container.querySelector<HTMLInputElement>("input[aria-label='Workflow name']")!;
  await act(async () => type(name, "Renamed"));
  expect(button("Save").disabled).toBe(false);
  await act(async () => button("Save").click());
  const [saved] = stored();
  expect(saved.name).toBe("Renamed");
  expect(saved).toMatchObject({
    runRequested: true,
    stopRequested: true,
    resumeRunId: "run-9",
    lastRunId: "run-8",
    importedFrom: "a.yaml",
    createdAt: 1,
  });
  expect(saved.nodes.map((n) => n.id)).toEqual(ticketWorkflow().nodes.map((n) => n.id));
  // Saved, so it is clean again.
  expect(button("Save").disabled).toBe(true);
});

const rename = async () => {
  const name = container.querySelector<HTMLInputElement>("input[aria-label='Workflow name']")!;
  await act(async () => type(name, "Renamed"));
};

it("will not save a flagged workflow enabled, points to the review, and keeps the mark when saved paused", async () => {
  const onReview = vi.fn();
  await renderBoard(ticketWorkflow({ enabled: true, needsReview: true }), { onReview });
  await rename();
  expect(button("Save").disabled).toBe(true);
  expect(container.textContent).toContain("Review this workflow before enabling it");
  await act(async () => buttonText("Review")!.click());
  expect(onReview).toHaveBeenCalledWith("enable");
  expect(stored()[0]).toMatchObject({ enabled: true, needsReview: true });
  await act(async () => buttonText("Save paused")!.click());
  expect(stored()[0]).toMatchObject({ name: "Renamed", enabled: false, needsReview: true });
});

it("saving a flagged workflow that is paused keeps the mark", async () => {
  await renderBoard(ticketWorkflow({ enabled: false, needsReview: true }));
  await rename();
  expect(container.textContent).not.toContain("Review this workflow before enabling it");
  await act(async () => button("Save").click());
  expect(stored()[0]).toMatchObject({ name: "Renamed", enabled: false, needsReview: true });
});

it("refuses to enable a workflow that was flagged after the board loaded it", async () => {
  await renderBoard(ticketWorkflow({ enabled: true }));
  await rename();
  files.set(
    "workflows.json",
    JSON.stringify([ticketWorkflow({ enabled: true, needsReview: true })]),
  );
  await act(async () => button("Save").click());
  expect(stored()[0]).toMatchObject({ name: "ClickUp -> Dev -> Review", needsReview: true });
  expect(container.querySelector("[data-workflow-notice]")?.textContent).toMatch(
    /Review this workflow/,
  );
});

it("Run now and Resume in the menu open the review of a flagged workflow", async () => {
  const onReview = vi.fn();
  await renderBoard(ticketWorkflow({ enabled: false, needsReview: true }), { onReview });
  const menuItem = (label: string) =>
    [...document.body.querySelectorAll<HTMLButtonElement>("[role='menu'] button[role='menuitem']")].find(
      (b) => b.textContent?.includes(label),
    )!;
  await act(async () => button("More").click());
  await act(async () => menuItem("Run now").click());
  expect(onReview).toHaveBeenLastCalledWith("run");
  await act(async () => button("More").click());
  await act(async () => menuItem("Resume").click());
  expect(onReview).toHaveBeenLastCalledWith("enable");
  // Nothing was switched on, in the draft or in the file.
  expect(button("Save").disabled).toBe(true);
  expect(stored()[0]).toMatchObject({ enabled: false, needsReview: true });
  expect(stored()[0].runRequested).toBeUndefined();
});

it("leaving with unsaved changes asks to save or discard", async () => {
  const onBack = vi.fn();
  await renderBoard(ticketWorkflow(), { onBack });
  await act(async () => button("Back").click());
  expect(onBack).toHaveBeenCalledTimes(1);

  onBack.mockClear();
  const name = container.querySelector<HTMLInputElement>("input[aria-label='Workflow name']")!;
  await act(async () => type(name, "Changed"));
  await act(async () => button("Back").click());
  expect(onBack).not.toHaveBeenCalled();
  expect(dialog().textContent).toContain("Save your changes?");
  await act(async () => buttonText("Discard", dialog())!.click());
  expect(onBack).toHaveBeenCalledTimes(1);
  expect(stored()[0].name).toBe("ClickUp -> Dev -> Review");
});

it("Save in the leave dialog saves, then leaves", async () => {
  const onBack = vi.fn();
  await renderBoard(ticketWorkflow(), { onBack });
  const name = container.querySelector<HTMLInputElement>("input[aria-label='Workflow name']")!;
  await act(async () => type(name, "Changed"));
  await act(async () => button("Back").click());
  await act(async () => buttonText("Save", dialog())!.click());
  expect(stored()[0].name).toBe("Changed");
  expect(onBack).toHaveBeenCalledTimes(1);
});

it("starts a new workflow with a manual trigger and a report node, saved paused", async () => {
  await renderBoard(undefined);
  expect(nodes().map((n) => n.getAttribute("data-board-node"))).toEqual(
    expect.arrayContaining(["trigger", "report"]),
  );
  const name = container.querySelector<HTMLInputElement>("input[aria-label='Workflow name']")!;
  expect(name.value).toBe("New workflow");
  await act(async () => type(name, "Mine"));
  await act(async () => button("Save").click());
  const [saved] = stored();
  expect(saved).toMatchObject({
    name: "Mine",
    enabled: false,
    inputs: {},
    inputValues: {},
  });
  expect(saved.id).toMatch(/^wf-[a-z0-9]{8}$/);
  expect(saved.nodes.map((n) => n.type).sort()).toEqual(["manual", "report"]);
});

it("opens the inspector for a node and goes back to the board", async () => {
  await renderBoard(ticketWorkflow());
  const fetch = container.querySelector<HTMLElement>(".react-flow__node[data-id='fetch']")!;
  await act(async () => fetch.click());
  const inspector = container.querySelector("[data-board-inspector]")!;
  expect(inspector.getAttribute("data-open")).toBe("true");
  const prompt = inspector.querySelector<HTMLTextAreaElement>("textarea[aria-label='Prompt']")!;
  expect(prompt.value).toContain("List open tickets");
  await act(async () => type(prompt, "Fetch everything"));
  expect(button("Save").disabled).toBe(false);
  await act(async () => buttonText("Back to board")!.click());
  expect(inspector.getAttribute("data-open")).toBe("false");
});

it("adds a node from the picker, disabling a second trigger and report", async () => {
  await renderBoard(ticketWorkflow());
  await act(async () => button("Add node").click());
  const menu = document.body.querySelector("[role='menu']")!;
  const item = (label: string) =>
    [...menu.querySelectorAll<HTMLButtonElement>("button[role='menuitem']")].find((b) =>
      b.textContent?.includes(label),
    )!;
  expect(item("Schedule").disabled).toBe(true);
  expect(item("Manual").disabled).toBe(true);
  expect(item("Report to Mono").disabled).toBe(true);
  expect(item("Command").disabled).toBe(false);
  expect(menu.textContent).toContain("Trigger");
  expect(menu.textContent).toContain("Action");
  expect(menu.textContent).toContain("Flow");
  expect(menu.textContent).toContain("End");
  await act(async () => item("Command").click());
  expect(nodes()).toHaveLength(7);
  expect(button("Save").disabled).toBe(false);
});

it("follows an edit made elsewhere while the board is clean", async () => {
  await renderBoard(ticketWorkflow());
  const renamed = { ...ticketWorkflow(), name: "By the Mono" };
  files.set("workflows.json", JSON.stringify([renamed]));
  await act(async () => {
    window.dispatchEvent(new CustomEvent("monocode:mono-files-changed"));
  });
  const name = container.querySelector<HTMLInputElement>("input[aria-label='Workflow name']")!;
  expect(name.value).toBe("By the Mono");
  expect(container.querySelector("[data-board-changed]")).toBeNull();
});

it("asks before replacing unsaved work with an edit made elsewhere", async () => {
  await renderBoard(ticketWorkflow());
  const name = () =>
    container.querySelector<HTMLInputElement>("input[aria-label='Workflow name']")!;
  await act(async () => type(name(), "Mine"));
  files.set(
    "workflows.json",
    JSON.stringify([{ ...ticketWorkflow(), name: "By the Mono" }]),
  );
  await act(async () => {
    window.dispatchEvent(new CustomEvent("monocode:mono-files-changed"));
  });
  const notice = container.querySelector("[data-board-changed]")!;
  expect(notice.textContent).toContain("This workflow changed elsewhere");
  expect(name().value).toBe("Mine");
  await act(async () => buttonText("Reload")!.click());
  expect(name().value).toBe("By the Mono");
  expect(container.querySelector("[data-board-changed]")).toBeNull();
});

it("Keep mine dismisses the notice and keeps the draft", async () => {
  await renderBoard(ticketWorkflow());
  const name = () =>
    container.querySelector<HTMLInputElement>("input[aria-label='Workflow name']")!;
  await act(async () => type(name(), "Mine"));
  files.set(
    "workflows.json",
    JSON.stringify([{ ...ticketWorkflow(), name: "By the Mono" }]),
  );
  await act(async () => {
    window.dispatchEvent(new CustomEvent("monocode:mono-files-changed"));
  });
  await act(async () => buttonText("Keep mine")!.click());
  expect(container.querySelector("[data-board-changed]")).toBeNull();
  expect(name().value).toBe("Mine");
  await act(async () => button("Save").click());
  expect(stored()[0].name).toBe("Mine");
});

it("is clean after saving a schedule the save anchored", async () => {
  // An interval schedule with no start date gets one when it is saved.
  const w = ticketWorkflow();
  const interval: Workflow = {
    ...w,
    nodes: w.nodes.map((n) =>
      n.type === "schedule"
        ? { ...n, schedule: { scheduleKind: "interval", time: "09:00", minute: 0, dayOfWeek: 1, intervalHours: 4 } }
        : n,
    ),
  };
  const onBack = vi.fn();
  await renderBoard(interval, { onBack });
  const name = container.querySelector<HTMLInputElement>("input[aria-label='Workflow name']")!;
  await act(async () => type(name, "Renamed"));
  await act(async () => button("Save").click());
  const trigger = stored()[0].nodes.find((n) => n.type === "schedule");
  expect(trigger && "schedule" in trigger && trigger.schedule.intervalFrom).toBeTruthy();
  expect(button("Save").disabled).toBe(true);
  await act(async () => button("Back").click());
  expect(document.body.querySelector("[role='dialog']")).toBeNull();
  expect(onBack).toHaveBeenCalledTimes(1);
});

const nameInput = () =>
  container.querySelector<HTMLInputElement>("input[aria-label='Workflow name']")!;

/** An edit that no files-changed event told this board about (another window, the CLI). */
const editBehindTheBoard = (patch: Partial<Workflow>) =>
  files.set("workflows.json", JSON.stringify([{ ...ticketWorkflow(), ...patch }]));

it("does not save over a definition that changed since the board loaded it, and says so", async () => {
  await renderBoard(ticketWorkflow());
  await act(async () => type(nameInput(), "Mine"));
  editBehindTheBoard({ name: "By the Mono" });
  await act(async () => button("Save").click());
  // Nothing was written.
  expect(stored()[0].name).toBe("By the Mono");
  const banner = container.querySelector("[data-board-changed]")!;
  expect(banner.textContent).toContain("This workflow changed elsewhere");
  expect(buttonText("Reload", banner)).toBeDefined();
  expect(buttonText("Overwrite", banner)).toBeDefined();
  // The draft is still there, and saving again still refuses.
  expect(nameInput().value).toBe("Mine");
  await act(async () => button("Save").click());
  expect(stored()[0].name).toBe("By the Mono");
});

it("Overwrite saves the draft over the version made elsewhere", async () => {
  await renderBoard(ticketWorkflow());
  await act(async () => type(nameInput(), "Mine"));
  editBehindTheBoard({ name: "By the Mono" });
  await act(async () => button("Save").click());
  await act(async () => buttonText("Overwrite")!.click());
  expect(stored()[0].name).toBe("Mine");
  expect(container.querySelector("[data-board-changed]")).toBeNull();
  expect(button("Save").disabled).toBe(true);
});

it("Reload after a refused save drops the draft and takes the stored version", async () => {
  await renderBoard(ticketWorkflow());
  await act(async () => type(nameInput(), "Mine"));
  editBehindTheBoard({ name: "By the Mono" });
  await act(async () => button("Save").click());
  await act(async () => buttonText("Reload")!.click());
  expect(nameInput().value).toBe("By the Mono");
  expect(stored()[0].name).toBe("By the Mono");
  expect(container.querySelector("[data-board-changed]")).toBeNull();
  expect(button("Save").disabled).toBe(true);
});

it("saves normally when only fields the app manages changed since the board loaded", async () => {
  await renderBoard(ticketWorkflow());
  await act(async () => type(nameInput(), "Mine"));
  editBehindTheBoard({ lastRunId: "run-3", runRequested: true });
  await act(async () => button("Save").click());
  expect(stored()[0]).toMatchObject({ name: "Mine", lastRunId: "run-3", runRequested: true });
  expect(container.querySelector("[data-board-changed]")).toBeNull();
});

const filesChanged = () =>
  act(async () => {
    window.dispatchEvent(new CustomEvent("monocode:mono-files-changed"));
  });

it("drops the review banner when the workflow is approved elsewhere", async () => {
  await renderBoard(ticketWorkflow({ enabled: true, needsReview: true }));
  expect(container.textContent).toContain("Review this workflow before enabling it");
  // The approval changes no part of the definition, only the mark.
  files.set("workflows.json", JSON.stringify([ticketWorkflow({ enabled: true })]));
  await filesChanged();
  expect(container.textContent).not.toContain("Review this workflow before enabling it");
  await rename();
  expect(button("Save").disabled).toBe(false);
  await act(async () => button("Save").click());
  expect(stored()[0]).toMatchObject({ name: "Renamed", enabled: true });
  expect(stored()[0].needsReview).toBeUndefined();
});

it("shows the review banner when the workflow is flagged elsewhere", async () => {
  await renderBoard(ticketWorkflow({ enabled: true }));
  expect(container.textContent).not.toContain("Review this workflow before enabling it");
  files.set("workflows.json", JSON.stringify([ticketWorkflow({ enabled: true, needsReview: true })]));
  await filesChanged();
  expect(container.textContent).toContain("Review this workflow before enabling it");
});

it("shows the real reason a save failed", async () => {
  vi.spyOn(console, "warn").mockImplementation(() => {});
  await renderBoard(ticketWorkflow());
  await act(async () => type(nameInput(), "Mine"));
  writeFailure = TOO_LARGE;
  await act(async () => button("Save").click());
  expect(container.querySelector("[data-workflow-notice]")?.textContent).toBe(TOO_LARGE.message);
  expect(stored()[0].name).toBe("ClickUp -> Dev -> Review");
  // An error with no message falls back to the generic text.
  writeFailure = new Error("");
  await act(async () => button("Save").click());
  expect(container.querySelector("[data-workflow-notice]")?.textContent).toBe(
    "Could not save the workflow.",
  );
});
