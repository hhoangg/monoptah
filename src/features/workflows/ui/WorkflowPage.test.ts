// @vitest-environment happy-dom
import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import type { Workflow, WorkflowRun } from "../model/workflow";
import { WorkflowPage } from "./WorkflowPage";
import { setWorkflowRunning } from "../model/workflowStore";
import { workflowActions } from "./MonoWorkflows";
import { run, workflow } from "./workflowUiFixtures";

const files = new Map<string, string>();
let runs: WorkflowRun[] = [];
const save = vi.hoisted(() => vi.fn(async () => "/tmp/out.yaml"));
const writeTextFile = vi.hoisted(() => vi.fn(async () => {}));

vi.mock("@tauri-apps/plugin-dialog", () => ({ save }));
vi.mock("../../../platform/tauri/fs", async (original) => ({
  ...(await original<object>()),
  writeTextFile,
}));
vi.mock("../../monos/model/monoFiles", async (original) => ({
  ...(await original<object>()),
  readAgentFile: async (_monoId: string, path: string) =>
    path.startsWith("workflow-runs/")
      ? { text: JSON.stringify(runs), hash: "r" }
      : { text: files.get(path) ?? null, hash: String(files.get(path)) },
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
  runs = [];
  save.mockClear();
  writeTextFile.mockClear();
  container = document.createElement("div");
  document.body.append(container);
  root = createRoot(container);
});
afterEach(() => {
  act(() => root.unmount());
  container.remove();
  vi.unstubAllGlobals();
});

const stored = (): Workflow[] => JSON.parse(files.get("workflows.json") ?? "[]");

async function renderPage(w: Workflow, onOpenRun = vi.fn(), onRemove = vi.fn()) {
  files.set("workflows.json", JSON.stringify([w]));
  await act(async () =>
    root.render(
      createElement(WorkflowPage, {
        monoId: "mono-1",
        workflow: w,
        color: "#9c9",
        projects: [
          { path: "/work/api", name: "api" },
          { path: "/work/web", name: "web" },
        ],
        actions: workflowActions("mono-1"),
        onBack: () => {},
        onEditBoard: () => {},
        onOpenRun,
        onRemove,
      }),
    ),
  );
  return onOpenRun;
}

const button = (label: string) =>
  container.querySelector<HTMLButtonElement>(`button[aria-label="${label}"]`)!;
const notice = () => container.querySelector("[data-workflow-notice]");

it("lists recent runs newest first and opens one", async () => {
  runs = [
    run({ id: "run-2", status: "failed" }),
    run({ id: "run-1", status: "done" }),
  ];
  const onOpenRun = await renderPage(workflow());
  const rows = [...container.querySelectorAll("button[data-run]")];
  expect(rows.map((row) => row.getAttribute("data-run"))).toEqual([
    "run-2",
    "run-1",
  ]);
  expect(rows[0].textContent).toContain("Failed");
  expect(rows[1].textContent).toContain("Finished");
  act(() => (rows[1] as HTMLButtonElement).click());
  expect(onOpenRun).toHaveBeenCalledWith("run-1");
});

it("export refuses on a path problem", async () => {
  await renderPage(workflow({ description: "Runs in /Users/me/secret" }));
  await act(async () => button("Export").click());
  expect(save).not.toHaveBeenCalled();
  expect(notice()?.textContent).toContain("Can't export");
});

it("export saves the definition under a file name made from the workflow's name", async () => {
  await renderPage(workflow({ name: "Nightly Checks!" }));
  await act(async () => button("Export").click());
  expect(save).toHaveBeenCalledWith(
    expect.objectContaining({
      defaultPath: "nightly-checks.monoptah-workflow.yaml",
    }),
  );
  expect(writeTextFile).toHaveBeenCalledWith(
    "/tmp/out.yaml",
    expect.stringContaining("monoptahWorkflow"),
  );
});

it("edits an input value after checking its type", async () => {
  const w = workflow({
    inputs: { limit: { type: "number" }, repo: { type: "project" } },
    inputValues: { limit: 3, repo: "/work/api" },
  });
  await renderPage(w);
  const limit = container.querySelector<HTMLInputElement>(
    "input[aria-label='limit']",
  )!;
  const set = (value: string) => {
    const setter = Object.getOwnPropertyDescriptor(
      HTMLInputElement.prototype,
      "value",
    )!.set!;
    setter.call(limit, value);
    limit.dispatchEvent(new Event("input", { bubbles: true }));
    limit.dispatchEvent(new FocusEvent("focusout", { bubbles: true }));
  };
  await act(async () => set("not a number"));
  expect(stored()[0].inputValues.limit).toBe(3);
  expect(notice()?.textContent).toContain('"limit" must be a number');
  await act(async () => set("7"));
  expect(stored()[0].inputValues.limit).toBe(7);
  expect(notice()).toBeNull();
});

it("Remove closes the page once the workflow is gone", async () => {
  const onRemove = vi.fn();
  await renderPage(workflow(), vi.fn(), onRemove);
  await act(async () => button("Remove").click());
  expect(stored()).toEqual([]);
  expect(onRemove).toHaveBeenCalledTimes(1);
});

it("Remove is refused with a notice while a run is going", async () => {
  const onRemove = vi.fn();
  await renderPage(workflow(), vi.fn(), onRemove);
  act(() => setWorkflowRunning("wf-aaaaaaaa", "run-live"));
  await act(async () => button("Remove").click());
  act(() => setWorkflowRunning("wf-aaaaaaaa", null));
  expect(stored()).toHaveLength(1);
  expect(onRemove).not.toHaveBeenCalled();
  expect(notice()?.textContent).toMatch(/Stop the workflow before removing it/);
});
