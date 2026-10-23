// @vitest-environment happy-dom
import { act, createElement, type ReactNode } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import type { Workflow, WorkflowNode } from "../../model/workflow";
import { workflow } from "../workflowUiFixtures";
import { NodeInspector, type MonoModelDefault } from "./NodeInspector";

vi.mock("../../../../integrations/harness/core/availability", () => ({
  getHarnessAvailabilitySnapshot: () => 0,
  hasProbedHarnessAvailability: () => true,
  isHarnessAvailable: () => true,
  probeHarnessAvailability: () => Promise.resolve(),
  subscribeHarnessAvailability: () => () => undefined,
}));
vi.mock("../../../../integrations/harness/core/registry", () => ({
  refreshHarnessCatalogs: () => Promise.resolve(),
}));
// The menus open inline, so a test can reach their options.
vi.mock("../../../../shared/ui/Popover", () => ({
  Popover: ({
    children,
    role,
    "aria-label": label,
  }: {
    children: ReactNode;
    role?: string;
    "aria-label"?: string;
  }) => createElement("div", { role, "aria-label": label }, children),
}));

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

async function renderInspector(
  node: WorkflowNode,
  onNode = vi.fn(),
  monoModel?: MonoModelDefault,
) {
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
        monoModel,
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

const MONO_MODEL: MonoModelDefault = {
  harness: "grok",
  model: "grok:grok-4.6",
  modelSettings: { effort: "high" },
};
const picker = () =>
  container.querySelector<HTMLButtonElement>("button[aria-keyshortcuts]")!;
const effortButton = () =>
  [...container.querySelectorAll<HTMLButtonElement>("button")].find((b) =>
    b.getAttribute("aria-label")?.startsWith("Effort"),
  )!;
const optionNamed = (name: string) =>
  [...container.querySelectorAll<HTMLButtonElement>("button")].find(
    (b) => b.textContent === name,
  )!;

it("shows the Mono's own model, harness and effort until the node picks one", async () => {
  await renderInspector(SESSION, vi.fn(), MONO_MODEL);
  expect(picker().textContent).toContain("Grok 4.6");
  expect(effortButton().textContent).toContain("High");
  expect(container.textContent).toContain("Mono default");
  // The free-text model field is gone.
  expect(container.querySelector("input[aria-label='Model']")).toBeNull();
  expect(container.textContent).not.toContain("Use Mono default");
});

it("picking a model writes its harness and model on the node", async () => {
  const onNode = await renderInspector(SESSION, vi.fn(), MONO_MODEL);
  await act(async () => picker().click());
  await act(async () => optionNamed("Grok 4.5").click());
  expect(onNode).toHaveBeenCalledTimes(1);
  expect(onNode.mock.calls[0][0]).toMatchObject({
    id: "dev",
    harness: "grok",
    model: "grok:grok-4.5",
  });
  expect(onNode.mock.calls[0][0].effort).toBeUndefined();
});

it("picking an effort writes it, with the model it belongs to", async () => {
  const onNode = await renderInspector(SESSION, vi.fn(), MONO_MODEL);
  await act(async () => effortButton().click());
  await act(async () => optionNamed("Extra High").click());
  expect(onNode.mock.calls[0][0]).toMatchObject({
    harness: "grok",
    model: "grok:grok-4.6",
    effort: "xhigh",
  });
});

it("shows what the node picked, and Use Mono default clears it", async () => {
  const picked: WorkflowNode = {
    ...SESSION,
    harness: "grok",
    model: "grok:grok-4.5",
    effort: "medium",
  } as WorkflowNode;
  const onNode = await renderInspector(picked, vi.fn(), MONO_MODEL);
  expect(picker().textContent).toContain("Grok 4.5");
  expect(effortButton().textContent).toContain("Medium");
  expect(
    [...container.querySelectorAll("span")].some((span) => span.textContent === "Mono default"),
  ).toBe(false);
  const reset = [...container.querySelectorAll<HTMLButtonElement>("button")].find(
    (b) => b.textContent === "Use Mono default",
  )!;
  await act(async () => reset.click());
  const next = onNode.mock.calls[0][0];
  expect(next.id).toBe("dev");
  expect(next).not.toHaveProperty("harness");
  expect(next).not.toHaveProperty("model");
  expect(next).not.toHaveProperty("effort");
});
