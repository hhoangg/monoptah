// @vitest-environment happy-dom
import { act, createElement, type ComponentProps } from "react";
import { createRoot } from "react-dom/client";
import { expect, it, vi } from "vitest";
import { newEditorPane, newTerminalFile } from "../../workspace/model/layout";
import { FilePane } from "./FilePane";

type Exit = { code: number | null; error?: string; output?: string };
const view = vi.hoisted(() => ({
  onExit: undefined as ((exit: Exit) => void) | undefined,
}));

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(async () => []),
  isTauri: () => false,
  convertFileSrc: (path: string) => path,
}));
vi.mock("@tauri-apps/api/event", () => ({ listen: async () => () => {} }));
vi.mock("../../terminal/ui/TerminalView", () => ({
  TerminalView: (props: { onExit?: (exit: Exit) => void }) => {
    view.onExit = props.onExit;
    return null;
  },
}));

async function mountTerminalPane() {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  const container = document.createElement("div");
  document.body.append(container);
  const root = createRoot(container);
  const file = newTerminalFile("/tmp");
  const pane = newEditorPane(file);
  const onCloseFile = vi.fn();
  const noop = () => {};
  const props: ComponentProps<typeof FilePane> = {
    pane,
    focused: true,
    showTabs: false,
    dirtyFileIds: new Set(),
    fileErrorCounts: new Map(),
    sessions: [],
    onFocus: noop,
    onSelectFile: noop,
    onCloseFile,
    onCloseOtherFiles: noop,
    onDirtyChange: noop,
    onErrorCountChange: noop,
    onReorderFiles: noop,
    onOpenFile: noop,
    onUpdatePlan: noop,
    onBuildPlan: noop,
  };
  await act(async () => root.render(createElement(FilePane, props)));
  await vi.waitFor(() => expect(view.onExit).toBeDefined());
  const cleanup = async () => {
    await act(async () => root.unmount());
    container.remove();
    vi.unstubAllGlobals();
    view.onExit = undefined;
  };
  return { pane, file, onCloseFile, cleanup };
}

it("closes the terminal pane when its process exits", async () => {
  const { pane, file, onCloseFile, cleanup } = await mountTerminalPane();
  act(() => view.onExit?.({ code: 0, output: "" }));
  expect(onCloseFile).toHaveBeenCalledExactlyOnceWith(pane.id, file.id);
  // A repeated exit event must not close it again.
  act(() => view.onExit?.({ code: 0, output: "" }));
  expect(onCloseFile).toHaveBeenCalledTimes(1);
  await cleanup();
});

it("keeps the terminal pane open when the process failed to start", async () => {
  const { onCloseFile, cleanup } = await mountTerminalPane();
  act(() => view.onExit?.({ code: null, error: "Failed to start" }));
  expect(onCloseFile).not.toHaveBeenCalled();
  await cleanup();
});

it("closes the terminal pane on a non-zero exit too", async () => {
  const { pane, file, onCloseFile, cleanup } = await mountTerminalPane();
  act(() => view.onExit?.({ code: 137, output: "" }));
  expect(onCloseFile).toHaveBeenCalledExactlyOnceWith(pane.id, file.id);
  await cleanup();
});
