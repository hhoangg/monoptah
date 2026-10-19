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
    contributorsActive: false,
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

// Only the context menu's own items: the sidebar has a Contributors entry too.
function menuItem(label: string) {
  return [...document.querySelectorAll('[role="menuitem"]')].find((item) =>
    (item.textContent ?? "").startsWith(label),
  );
}

const entry = () =>
  container.querySelector<HTMLButtonElement>(
    'button[aria-label="Contributors"]',
  );

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

const layouts = [
  ["full rail", { projectRailOpen: true, compactProjectRail: false }],
  ["compact rail", { projectRailOpen: false, compactProjectRail: true }],
  ["project picker", { projectRailOpen: false, compactProjectRail: false }],
] as const;

function openProjectMenu(layout: (typeof layouts)[number][1]) {
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
}

describe("Contributors sidebar entry", () => {
  it.each(layouts)("is labelled Contributors (%s)", async (_name, layout) => {
    props = { ...props, ...layout };
    await act(async () => root.render(createElement(Sidebar, props)));
    expect(entry()).not.toBeNull();
    expect(
      container.querySelector('button[aria-label="Ticket Contributors"]'),
    ).toBeNull();
    expect(container.textContent).not.toContain("Ticket Contributors");
  });

  it.each(layouts)("opens the page (%s)", async (_name, layout) => {
    props = { ...props, ...layout };
    await act(async () => root.render(createElement(Sidebar, props)));
    act(() => entry()!.click());
    expect(props.onOpenContributors).toHaveBeenCalledTimes(1);
  });

  it.each(layouts)(
    "is absent without an opener (%s)",
    async (_name, layout) => {
      props = { ...props, ...layout, onOpenContributors: undefined };
      await act(async () => root.render(createElement(Sidebar, props)));
      expect(entry()).toBeNull();
    },
  );
});

describe("Contributors project menu entry", () => {
  it.each(layouts)(
    "is gone from the project menu (%s)",
    async (_name, layout) => {
      props = { ...props, ...layout };
      await act(async () => root.render(createElement(Sidebar, props)));
      openProjectMenu(layout);
      expect(menuItem("Open in editor")).toBeDefined();
      expect(menuItem("Contributors")).toBeUndefined();
      expect(props.onOpenContributors).not.toHaveBeenCalled();
    },
  );
});
