// @vitest-environment happy-dom
import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import type { Workflow, WorkflowNode } from "../../model/workflow";
import { workflow } from "../workflowUiFixtures";
import { NodeInspector } from "./NodeInspector";

const platform = vi.hoisted(() => ({ win: false }));
vi.mock("../../../../platform/tauri/platform", async (original) => ({
  ...(await original<object>()),
  get IS_WIN() {
    return platform.win;
  },
}));

let root: Root;
let container: HTMLElement;
beforeEach(() => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  platform.win = false;
  container = document.createElement("div");
  document.body.append(container);
  root = createRoot(container);
});
afterEach(() => {
  act(() => root.unmount());
  container.remove();
  vi.unstubAllGlobals();
});

const SESSION: WorkflowNode = {
  id: "dev",
  type: "session",
  project: "{{inputs.project}}",
  prompt: "Do it",
  checkout: "root",
  output: { summary: { type: "string" }, count: { type: "number" } },
};
const COMMAND: WorkflowNode = {
  id: "cmd",
  type: "command",
  project: "{{inputs.project}}",
  command: "npm test",
};

async function renderInspector(node: WorkflowNode, onNode = vi.fn()) {
  const w: Workflow = workflow({
    inputs: { project: { type: "project" } },
    inputValues: { project: "/work/api" },
    nodes: [{ id: "trigger", type: "manual" }, node, { id: "report", type: "report" }],
  });
  await act(async () =>
    root.render(
      createElement(NodeInspector, {
        view: { kind: "node", id: node.id },
        workflow: w,
        projects: [{ path: "/work/api", name: "api" }],
        issues: [],
        onNode,
        onSettings: () => {},
        onLoop: () => {},
        onRemove: () => {},
        onBack: () => {},
      }),
    ),
  );
  return onNode;
}

const inReport = () =>
  [...container.querySelectorAll<HTMLButtonElement>("button[aria-pressed]")].filter(
    (b) => b.textContent?.trim() === "In report",
  );

it("shows a string field as in the report and a number field as not", async () => {
  await renderInspector(SESSION);
  const [summary, count] = inReport();
  expect(summary.getAttribute("aria-pressed")).toBe("true");
  expect(count.getAttribute("aria-pressed")).toBe("false");
});

it("toggling stores the opposite of the effective value", async () => {
  const onNode = await renderInspector(SESSION);
  const [summary, count] = inReport();
  await act(async () => summary.click());
  const first = onNode.mock.calls[0][0];
  expect(first.output.summary.inReport).toBe(false);
  await act(async () => count.click());
  const second = onNode.mock.calls[1][0];
  expect(second.output.count.inReport).toBe(true);
});

it("tells Windows users that command nodes do not run there", async () => {
  platform.win = true;
  await renderInspector(COMMAND);
  expect(container.textContent).toContain("not supported on Windows");
});

it("does not mention Windows on other platforms", async () => {
  await renderInspector(COMMAND);
  expect(container.textContent).not.toContain("Windows");
});
