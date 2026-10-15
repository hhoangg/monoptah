// @vitest-environment happy-dom
import { act, createElement, type ComponentProps } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { Sidebar } from "./Sidebar";

// Keep native services out of these menu interaction tests.
vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(async (command: string) =>
    command === "list_external_editors" ? [] : null,
  ),
  convertFileSrc: (path: string) => path,
}));
vi.mock("../../features/source-control/hooks/useProjectDiffStats", () => ({
  useProjectDiffStats: vi.fn(() => null),
}));
vi.mock("../../features/source-control/hooks/useGitFileStatuses", () => ({
  useGitFileStatuses: () => ({ files: new Map(), dirs: new Map() }),
}));
vi.mock("./SidebarUpdate", () => ({ SidebarUpdateFooter: () => null }));
vi.mock("../../features/files/ui/FileTree", () => ({
  FileTree: () => null,
}));

let container: HTMLDivElement;
let root: Root;
let props: ComponentProps<typeof Sidebar>;

beforeEach(() => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  localStorage.clear();
  props = {
    cwd: "/workspace/project",
    open: true,
    sessions: [],
    busySessionIds: new Set(),
    approvalSessionIds: new Set(),
    activeSessionId: "",
    status: "idle",
    pending: false,
    tab: "sessions",
    filesSearchOpen: false,
    recents: [
      { path: "/workspace/project", openedAt: 2 },
      { path: "/workspace/other", openedAt: 1 },
    ],
    onSelectSession: vi.fn(),
    onRenameSession: vi.fn(),
    onOpenFile: vi.fn(),
    onTabChange: vi.fn(),
    onFilesSearchOpenChange: vi.fn(),
    onSelectProject: vi.fn(),
    onOpenProject: vi.fn(),
    onOpenContributors: vi.fn(),
  };
  container = document.createElement("div");
  document.body.append(container);
  root = createRoot(container);
});

afterEach(() => {
  act(() => root.unmount());
  container.remove();
  localStorage.clear();
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

function menuButton(label: string) {
  return [...document.querySelectorAll("button")].find(
    (item) => (item.getAttribute("aria-label") ?? item.textContent) === label,
  );
}

function openMenu(target: Element) {
  act(() =>
    target.dispatchEvent(
      new MouseEvent("contextmenu", {
        bubbles: true,
        cancelable: true,
        clientX: 40,
        clientY: 80,
      }),
    ),
  );
}

describe("Contributors project menu entry", () => {
  const layouts = [
    ["full rail", { projectRailOpen: true, compactProjectRail: false }],
    ["compact rail", { projectRailOpen: false, compactProjectRail: true }],
    ["project picker", { projectRailOpen: false, compactProjectRail: false }],
  ] as const;

  it.each(layouts)(
    "passes onOpenContributors to the project menu (%s)",
    async (_name, layout) => {
      props = { ...props, ...layout };
      await act(async () => root.render(createElement(Sidebar, props)));
      if (layout.projectRailOpen) {
        openMenu(
          container.querySelector('button[aria-label="other"]') ??
            container.querySelector('button[aria-current="true"]')!,
        );
      } else {
        const trigger = container.querySelector<HTMLButtonElement>(
          '[aria-label^="Switch project"]',
        )!;
        act(() => trigger.click());
        openMenu(document.querySelector('button[title="/workspace/other"]')!);
      }
      const item = menuButton("Contributors");
      expect(item).toBeDefined();
      act(() => item!.click());
      expect(props.onOpenContributors).toHaveBeenCalledTimes(1);
    },
  );

  it.each(layouts)(
    "shows no Contributors entry without a callback (%s)",
    async (_name, layout) => {
      props = { ...props, ...layout, onOpenContributors: undefined };
      await act(async () => root.render(createElement(Sidebar, props)));
      if (layout.projectRailOpen) {
        openMenu(container.querySelector('button[aria-current="true"]')!);
      } else {
        const trigger = container.querySelector<HTMLButtonElement>(
          '[aria-label^="Switch project"]',
        )!;
        act(() => trigger.click());
        openMenu(document.querySelector('button[title="/workspace/other"]')!);
      }
      expect(menuButton("Open in editor")).toBeDefined();
      expect(menuButton("Contributors")).toBeUndefined();
    },
  );
});
