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

// Only the context menu's own items: the rail has a Contributors entry too.
function menuItem(label: string) {
  return [...document.querySelectorAll('[role="menuitem"]')].find((item) =>
    (item.textContent ?? "").startsWith(label),
  );
}

async function openProjectMenu(cwd: string) {
  await act(async () =>
    root.render(
      createElement(ProjectRail, {
        cwd,
        recents: [],
        onSelectProject: vi.fn(),
        onOpenProject: vi.fn(),
        // The page opens from the rail's own entry now, never from this menu.
        onOpenContributors: vi.fn(),
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

it("has no Contributors item in the project context menu", async () => {
  await openProjectMenu("/work/private");
  expect(menuItem("Open in editor")).toBeDefined();
  expect(menuItem("Contributors")).toBeUndefined();
});

it("keeps the rest of the project menu", async () => {
  await openProjectMenu("/work/private");
  expect(menuItem("Mute notifications")).toBeDefined();
});

it("has no Contributors item for a remote project either", async () => {
  const remote = `${REMOTE_PROJECT_PREFIX}host-1/home/dev/app`;
  await openProjectMenu(remote);
  expect(menuItem("Open in editor")).toBeDefined();
  expect(menuItem("Contributors")).toBeUndefined();
});
