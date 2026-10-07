// @vitest-environment happy-dom
import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { ClickUpSettings } from "./ClickUpSettings";
import {
  clickupIssueThread,
  loadHiddenClickUpSpaceIds,
  peekClickUpIssueThread,
} from "../../inbox/model/clickup";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/plugin-opener", () => ({ openUrl: vi.fn() }));

const SPACES = [
  { id: "901", name: "Engineering" },
  { id: "902", name: "Design" },
];
const WORKSPACES = [
  { id: "1", name: "Acme" },
  { id: "2", name: "Globex" },
];

let container: HTMLDivElement;
let root: Root;
let workspaces = [WORKSPACES[0]!];
beforeEach(() => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  localStorage.clear();
  workspaces = [WORKSPACES[0]!];
  container = document.createElement("div");
  document.body.append(container);
  root = createRoot(container);
  vi.mocked(invoke).mockReset();
  vi.mocked(invoke).mockImplementation(async (command, args) => {
    if (command === "clickup_status")
      return { connected: false, teamId: "", teamName: "", username: "" };
    if (command === "clickup_list_workspaces") return workspaces;
    if (command === "clickup_set_config") {
      const config = args as { token: string; teamId: string | null };
      const picked =
        workspaces.length === 1
          ? workspaces[0]!
          : workspaces.find((workspace) => workspace.id === config.teamId);
      return {
        connected: Boolean(config.token && picked),
        teamId: picked?.id ?? "",
        teamName: picked?.name ?? "",
        username: config.token ? "ada" : "",
      };
    }
    if (command === "clickup_list_spaces") return SPACES;
    if (command === "clickup_issue_thread")
      return {
        comments: [],
        truncated: false,
        reviewDecision: "",
        baseRefName: "",
        headRefName: "",
      };
    throw new Error(`Unexpected command: ${command}`);
  });
});
afterEach(async () => {
  await act(async () => root.unmount());
  container.remove();
  localStorage.clear();
  vi.unstubAllGlobals();
});

async function input(label: string, value: string) {
  const field = container.querySelector<HTMLInputElement>(
    `input[aria-label="${label}"]`,
  )!;
  await act(async () => {
    Object.getOwnPropertyDescriptor(
      HTMLInputElement.prototype,
      "value",
    )!.set!.call(field, value);
    field.dispatchEvent(new Event("input", { bubbles: true }));
  });
}

async function submit() {
  await act(async () => {
    container
      .querySelector("form")!
      .dispatchEvent(new Event("submit", { bubbles: true, cancelable: true }));
  });
}

function button(label: string) {
  return [...container.querySelectorAll("button")].find(
    (candidate) => candidate.textContent === label,
  )!;
}

it("connects with one workspace, lists spaces, syncs hidden spaces, and disconnects", async () => {
  await act(async () => root.render(createElement(ClickUpSettings)));
  expect(
    container.querySelector('select[aria-label="ClickUp workspace"]'),
  ).toBeNull();
  await input("ClickUp API token", " pk_secret ");
  await submit();
  expect(invoke).toHaveBeenCalledWith("clickup_list_workspaces", {
    token: "pk_secret",
  });
  expect(invoke).toHaveBeenCalledWith("clickup_set_config", {
    token: "pk_secret",
    teamId: null,
  });
  expect(container.textContent).toContain("Acme");
  expect(container.querySelector('input[type="password"]')).toBeNull();
  expect(container.textContent).toContain("newest 100");

  const boxes = [
    ...container.querySelectorAll<HTMLInputElement>('input[type="checkbox"]'),
  ];
  expect(boxes).toHaveLength(2);
  expect(boxes.every((box) => box.checked)).toBe(true);
  await act(async () => boxes[1]!.click());
  expect(loadHiddenClickUpSpaceIds()).toEqual(["902"]);
  expect(boxes[1]!.checked).toBe(false);

  // Seed a cached thread, then check that Disconnect clears it with the config.
  await clickupIssueThread("86abc12");
  expect(peekClickUpIssueThread("86abc12")).not.toBeNull();
  await act(async () => button("Disconnect").click());
  expect(invoke).toHaveBeenCalledWith("clickup_set_config", {
    token: "",
    teamId: null,
  });
  expect(peekClickUpIssueThread("86abc12")).toBeNull();
  expect(
    container.querySelector<HTMLInputElement>('input[type="password"]')!.value,
  ).toBe("");
  expect(container.querySelector('input[type="checkbox"]')).toBeNull();
});

it("shows the workspace picker only for a token with several workspaces", async () => {
  workspaces = WORKSPACES;
  await act(async () => root.render(createElement(ClickUpSettings)));
  await input("ClickUp API token", "pk_secret");
  await submit();
  const select = container.querySelector<HTMLSelectElement>(
    'select[aria-label="ClickUp workspace"]',
  )!;
  expect(select).not.toBeNull();
  expect([...select.options].map((option) => option.textContent)).toEqual([
    "Choose a workspace",
    "Acme",
    "Globex",
  ]);
  // Nothing is saved until a workspace is chosen.
  expect(invoke).not.toHaveBeenCalledWith(
    "clickup_set_config",
    expect.anything(),
  );
  expect(button("Connect").disabled).toBe(true);
  await act(async () => {
    select.value = "2";
    select.dispatchEvent(new Event("change", { bubbles: true }));
  });
  await submit();
  expect(invoke).toHaveBeenCalledWith("clickup_set_config", {
    token: "pk_secret",
    teamId: "2",
  });
  expect(container.textContent).toContain("Globex");
  expect(container.querySelector("select")).toBeNull();
});

it("shows authentication errors without claiming a successful connection", async () => {
  await act(async () => root.render(createElement(ClickUpSettings)));
  await input("ClickUp API token", "bad-token");
  vi.mocked(invoke).mockRejectedValueOnce(
    new Error("ClickUp API token is invalid or expired"),
  );
  await submit();
  expect(container.querySelector('[role="alert"]')?.textContent).toContain(
    "invalid",
  );
  expect(container.querySelector("form")).not.toBeNull();
  expect(container.textContent).not.toContain("Spaces");
  expect(invoke).not.toHaveBeenCalledWith(
    "clickup_set_config",
    expect.anything(),
  );
});
