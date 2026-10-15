// @vitest-environment happy-dom
import { act, createElement, type ComponentProps } from "react";
import { createRoot } from "react-dom/client";
import { expect, it, vi } from "vitest";
import { newEditorPane, newFileTab } from "../../workspace/model/layout";
import { FilePane } from "./FilePane";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(async () => []),
  isTauri: () => false,
  convertFileSrc: (path: string) => path,
}));
vi.mock("@tauri-apps/api/event", () => ({ listen: async () => () => {} }));
vi.mock("./FileEditor", async () => {
  const React = await import("react");
  return {
    FileEditor: ({ active, shown }: { active: boolean; shown?: boolean }) =>
      React.createElement(
        "div",
        {
          "data-shared-editor": true,
          "data-active": String(active),
          "data-shown": String(shown),
        },
        "Editor",
      ),
  };
});

it("renders a remote path in the shared editor", async () => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  const container = document.createElement("div");
  document.body.append(container);
  const root = createRoot(container);
  const file = newFileTab("remote://env/repo/src/index.ts", "remote://env/repo");
  const noop = () => {};
  const props: ComponentProps<typeof FilePane> = {
    pane: newEditorPane(file),
    focused: true,
    showTabs: false,
    dirtyFileIds: new Set(),
    fileErrorCounts: new Map(),
    sessions: [],
    onFocus: noop,
    onSelectFile: noop,
    onCloseFile: noop,
    onCloseOtherFiles: noop,
    onDirtyChange: noop,
    onErrorCountChange: noop,
    onReorderFiles: noop,
    onOpenFile: noop,
    onUpdatePlan: noop,
    onBuildPlan: noop,
  };
  await act(async () => root.render(createElement(FilePane, props)));
  expect(container.querySelector("[data-shared-editor]")).not.toBeNull();
  await act(async () => root.unmount());
  container.remove();
  vi.unstubAllGlobals();
});

it("tells the editor whether it is on screen separately from focus", async () => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  const container = document.createElement("div");
  document.body.append(container);
  const root = createRoot(container);
  const file = newFileTab("/repo/src/index.ts", "/repo");
  const noop = () => {};
  const base: ComponentProps<typeof FilePane> = {
    pane: newEditorPane(file),
    focused: false,
    showTabs: false,
    dirtyFileIds: new Set(),
    fileErrorCounts: new Map(),
    sessions: [],
    onFocus: noop,
    onSelectFile: noop,
    onCloseFile: noop,
    onCloseOtherFiles: noop,
    onDirtyChange: noop,
    onErrorCountChange: noop,
    onReorderFiles: noop,
    onOpenFile: noop,
    onUpdatePlan: noop,
    onBuildPlan: noop,
  };
  const flags = async (props: Partial<ComponentProps<typeof FilePane>>) => {
    await act(async () =>
      root.render(createElement(FilePane, { ...base, ...props })),
    );
    const editor = container.querySelector<HTMLElement>("[data-shared-editor]")!;
    return [editor.dataset.active, editor.dataset.shown];
  };
  // Visible in an unfocused pane: on screen, but without keyboard focus.
  expect(await flags({ focused: false, visible: true })).toEqual([
    "false",
    "true",
  ]);
  expect(await flags({ focused: true, visible: true })).toEqual([
    "true",
    "true",
  ]);
  // The containing workspace tab is hidden but stays mounted.
  expect(await flags({ focused: false, visible: false })).toEqual([
    "false",
    "false",
  ]);
  await act(async () => root.unmount());
  container.remove();
  vi.unstubAllGlobals();
});
