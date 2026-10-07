// @vitest-environment happy-dom
import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import {
  createAutomationTrigger,
  newAutomationDraft,
  notifyAutomationsChanged,
  type Automation,
} from "../model/automations";
import { AutomationsView } from "./AutomationsView";

const invoke = vi.hoisted(() => vi.fn());
vi.mock("@tauri-apps/api/core", async (original) => ({
  ...(await original<typeof import("@tauri-apps/api/core")>()),
  invoke,
}));
vi.mock("@tauri-apps/api/event", () => ({ listen: async () => () => {} }));
vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: () => ({
    isMaximized: async () => false,
    onResized: async () => () => {},
  }),
}));

let container: HTMLDivElement;
let root: Root;

const automation: Automation = {
  ...newAutomationDraft("/work/project", "codex", "model"),
  id: "clickup-automation",
  name: "Triage ClickUp",
  prompt: "Triage the task",
  triggerKind: "clickup",
  triggerEvent: "issue_created",
  triggers: [createAutomationTrigger("clickup", "issue_created")],
  nextRunAt: 0,
  createdAt: 1,
  updatedAt: 1,
};

beforeEach(() => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  vi.stubGlobal(
    "ResizeObserver",
    class {
      observe() {}
      disconnect() {}
    },
  );
  notifyAutomationsChanged();
  invoke.mockReset();
  invoke.mockImplementation(async (command: string) => {
    if (command === "automations_list") return [automation];
    if (command === "clickup_status")
      return {
        connected: true,
        teamId: "1",
        teamName: "Acme",
        username: "ada",
      };
    if (command === "clickup_list_spaces")
      return [
        { id: "901", name: "Engineering" },
        { id: "902", name: "Design" },
      ];
    if (command === "automations_upsert") return automation;
    if (command.endsWith("_status")) return { connected: false };
    return [];
  });
  container = document.createElement("div");
  document.body.append(container);
  root = createRoot(container);
});

afterEach(async () => {
  await act(async () => root.unmount());
  container.remove();
  document
    .querySelectorAll("[data-dialog-popover]")
    .forEach((element) => element.parentElement?.remove());
  vi.unstubAllGlobals();
});

const pill = (label: string) =>
  document.querySelector<HTMLButtonElement>(`button[aria-label^="${label}:"]`);

it("offers ClickUp -> Task appeared with an optional space filter", async () => {
  await act(async () =>
    root.render(
      createElement(AutomationsView, {
        cwd: "/work/project",
        recents: [],
        onClose: vi.fn(),
        onLaunch: vi.fn(),
        onOpenSession: vi.fn(),
      }),
    ),
  );
  await act(async () =>
    container
      .querySelector<HTMLButtonElement>('[aria-label="Open Triage ClickUp"]')!
      .click(),
  );
  expect(document.body.textContent).toContain("Task appeared");
  expect(pill("Space")?.getAttribute("aria-label")).toBe("Space: Any space");

  await act(async () => pill("Space")!.click());
  const options = [
    ...document.querySelectorAll<HTMLButtonElement>('[role="option"]'),
  ];
  expect(options.map((option) => option.textContent)).toEqual([
    "Any space",
    "Engineering",
    "Design",
  ]);
  await act(async () =>
    options.find((option) => option.textContent === "Design")!.click(),
  );
  expect(pill("Space")?.getAttribute("aria-label")).toBe("Space: Design");

  // The trigger keeps the space id, so a later rename does not break the filter.
  await act(async () =>
    [...document.querySelectorAll("button")]
      .find((button) => button.textContent === "Save")!
      .click(),
  );
  const saved = invoke.mock.calls.find(
    ([command]) => command === "automations_upsert",
  )![1] as { automation: Automation };
  expect(saved.automation.triggers?.[0]).toMatchObject({
    kind: "clickup",
    event: "issue_created",
    repos: ["902"],
  });
});

it("shows a saved space filter under the space's current name", async () => {
  const saved: Automation = {
    ...automation,
    triggers: [
      {
        ...createAutomationTrigger("clickup", "issue_created"),
        repos: ["901"],
      },
    ],
  };
  invoke.mockImplementation(async (command: string) => {
    if (command === "automations_list") return [saved];
    if (command === "clickup_status")
      return {
        connected: true,
        teamId: "1",
        teamName: "Acme",
        username: "ada",
      };
    // Space 901 was called "Engineering" when the filter was saved.
    if (command === "clickup_list_spaces")
      return [{ id: "901", name: "Platform" }];
    if (command.endsWith("_status")) return { connected: false };
    return [];
  });
  await act(async () =>
    root.render(
      createElement(AutomationsView, {
        cwd: "/work/project",
        recents: [],
        onClose: vi.fn(),
        onLaunch: vi.fn(),
        onOpenSession: vi.fn(),
      }),
    ),
  );
  await act(async () =>
    container
      .querySelector<HTMLButtonElement>('[aria-label="Open Triage ClickUp"]')!
      .click(),
  );
  expect(pill("Space")?.getAttribute("aria-label")).toBe("Space: Platform");
});
