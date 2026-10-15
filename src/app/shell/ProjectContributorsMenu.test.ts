// @vitest-environment happy-dom
import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { ProjectRail } from "./ProjectRail";
import { REMOTE_PROJECT_PREFIX } from "../../features/projects/model/recents";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(async (command: string) => {
    if (command === "list_external_editors") return [];
    return { root: "/work/private", commonDir: null, remote: null };
  }),
  convertFileSrc: (path: string) => path,
}));
vi.mock("../../features/source-control/hooks/useProjectDiffStats", () => ({
  useProjectDiffStats: () => null,
}));

let container: HTMLDivElement;
let root: Root;
beforeEach(() => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  localStorage.clear();
  container = document.createElement("div");
  document.body.append(container);
  root = createRoot(container);
});
afterEach(() => {
  act(() => root.unmount());
  container.remove();
  vi.unstubAllGlobals();
});

function menuButton(label: string) {
  return [...document.querySelectorAll("button")].find(
    (item) => (item.getAttribute("aria-label") ?? item.textContent) === label,
  );
}

async function openProjectMenu(cwd: string, onOpenContributors?: () => void) {
  await act(async () =>
    root.render(
      createElement(ProjectRail, {
        cwd,
        recents: [],
        onSelectProject: vi.fn(),
        onOpenProject: vi.fn(),
        ...(onOpenContributors ? { onOpenContributors } : {}),
      }),
    ),
  );
  act(() =>
    container.querySelector('button[aria-current="true"]')!.dispatchEvent(
      new MouseEvent("contextmenu", {
        bubbles: true,
        clientX: 40,
        clientY: 80,
      }),
    ),
  );
}

it("opens the contributors page for the project from its context menu", async () => {
  const onOpenContributors = vi.fn();
  await openProjectMenu("/work/private", onOpenContributors);
  const item = menuButton("Contributors");
  expect(item).toBeDefined();
  act(() => item!.click());
  expect(onOpenContributors).toHaveBeenCalledTimes(1);
  expect(onOpenContributors).toHaveBeenCalledWith("/work/private");
});

it("lists Contributors before the notification items", async () => {
  await openProjectMenu("/work/private", vi.fn());
  const labels = [...document.querySelectorAll('[role="menuitem"]')].map(
    (item) => item.textContent ?? "",
  );
  const contributors = labels.findIndex((label) =>
    label.startsWith("Contributors"),
  );
  const mute = labels.findIndex((label) =>
    label.startsWith("Mute notifications"),
  );
  expect(contributors).toBeGreaterThanOrEqual(0);
  expect(mute).toBeGreaterThan(contributors);
});

it("has no Contributors item without a callback", async () => {
  await openProjectMenu("/work/private");
  expect(menuButton("Open in editor")).toBeDefined();
  expect(menuButton("Contributors")).toBeUndefined();
});

it("has no Contributors item for a remote project", async () => {
  const remote = `${REMOTE_PROJECT_PREFIX}host-1/home/dev/app`;
  await openProjectMenu(remote, vi.fn());
  expect(menuButton("Open in editor")).toBeDefined();
  expect(menuButton("Contributors")).toBeUndefined();
});
