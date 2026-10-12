// @vitest-environment happy-dom
import { act, createElement, type ComponentProps } from "react";
import { createRoot } from "react-dom/client";
import { expect, it, vi } from "vitest";
import { newEditorPane, newTerminalFile } from "../../workspace/model/layout";
import { ProjectTerminalDock } from "./ProjectTerminalDock";

type Exit = { code: number | null; error?: string; output?: string };
const view = vi.hoisted(() => ({
  onExit: undefined as ((exit: Exit) => void) | undefined,
}));

vi.mock("./TerminalView", () => ({
  TerminalView: (props: { onExit?: (exit: Exit) => void }) => {
    view.onExit = props.onExit;
    return null;
  },
}));

async function mountDock() {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  const container = document.createElement("div");
  document.body.append(container);
  const root = createRoot(container);
  const file = newTerminalFile("/tmp");
  const onCloseTerminal = vi.fn();
  const noop = () => {};
  const props: ComponentProps<typeof ProjectTerminalDock> = {
    dock: {
      projectPath: "/tmp",
      pane: newEditorPane(file),
      side: "bottom",
      size: 220,
      open: true,
    },
    focused: true,
    onFocus: noop,
    onHide: noop,
    onSideChange: noop,
    onSizePaint: noop,
    onSizeCommit: noop,
    onAddTerminal: noop,
    onSelectTerminal: noop,
    onCloseTerminal,
    onCloseOtherTerminals: noop,
    onReorderTerminals: noop,
  };
  await act(async () => root.render(createElement(ProjectTerminalDock, props)));
  await vi.waitFor(() => expect(view.onExit).toBeDefined());
  const cleanup = async () => {
    await act(async () => root.unmount());
    container.remove();
    vi.unstubAllGlobals();
    view.onExit = undefined;
  };
  return { file, onCloseTerminal, cleanup };
}

it("closes a dock terminal when its process exits", async () => {
  const { file, onCloseTerminal, cleanup } = await mountDock();
  act(() => view.onExit?.({ code: 0, output: "" }));
  expect(onCloseTerminal).toHaveBeenCalledExactlyOnceWith(file.id);
  act(() => view.onExit?.({ code: 0, output: "" }));
  expect(onCloseTerminal).toHaveBeenCalledTimes(1);
  await cleanup();
});

it("keeps a dock terminal open when the process failed to start", async () => {
  const { onCloseTerminal, cleanup } = await mountDock();
  act(() => view.onExit?.({ code: null, error: "Failed to start" }));
  expect(onCloseTerminal).not.toHaveBeenCalled();
  await cleanup();
});

it("closes a dock terminal on a non-zero exit too", async () => {
  const { file, onCloseTerminal, cleanup } = await mountDock();
  act(() => view.onExit?.({ code: 130, output: "" }));
  expect(onCloseTerminal).toHaveBeenCalledExactlyOnceWith(file.id);
  await cleanup();
});
