// @vitest-environment happy-dom
import { act, createElement } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";

const tauri = vi.hoisted(() => ({
  invoke: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => {}),
}));
vi.mock("@tauri-apps/api/core", () => ({ invoke: tauri.invoke }));
vi.mock("@tauri-apps/api/webviewWindow", () => ({
  getCurrentWebviewWindow: () => ({ listen: tauri.listen }),
}));
// The component picks its key and its event source from the platform, and the
// test runner is not a Mac, so the Ctrl+Q path is the one under test here.
vi.mock("../../platform/tauri/platform", () => ({ IS_MAC: false }));

import { QUIT_ARM_MS } from "../model/quitGate";
import { QuitGateNotice } from "./QuitGateNotice";

let host: HTMLDivElement;
let root: ReturnType<typeof createRoot>;

beforeEach(() => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  vi.useFakeTimers();
  tauri.invoke.mockClear();
  host = document.createElement("div");
  document.body.appendChild(host);
  root = createRoot(host);
  act(() => root.render(createElement(QuitGateNotice)));
});

afterEach(() => {
  act(() => root.unmount());
  host.remove();
  vi.useRealTimers();
});

function pressQuit(over: Partial<KeyboardEventInit> & { target?: Element } = {}) {
  const { target, ...init } = over;
  const event = new KeyboardEvent("keydown", {
    key: "q",
    ctrlKey: true,
    bubbles: true,
    cancelable: true,
    ...init,
  });
  act(() => {
    (target ?? window).dispatchEvent(event);
  });
  return event;
}

const notice = () => document.body.textContent ?? "";

it("shows the notice instead of quitting on the first press", () => {
  pressQuit();

  expect(notice()).toContain("Hold Ctrl+Q to Quit");
  expect(tauri.invoke).not.toHaveBeenCalled();
});

it("quits on a second press while the notice is up", () => {
  pressQuit();
  pressQuit();

  expect(tauri.invoke).toHaveBeenCalledWith("quit_now");
});

it("quits when the key is held past the hold", () => {
  pressQuit();
  act(() => {
    vi.advanceTimersByTime(1000);
  });
  pressQuit({ repeat: true });

  expect(tauri.invoke).toHaveBeenCalledWith("quit_now");
});

it("drops the notice, and starts over, once the window passes", () => {
  pressQuit();
  act(() => {
    vi.advanceTimersByTime(QUIT_ARM_MS + 1);
  });
  expect(notice()).not.toContain("Hold Ctrl+Q");

  pressQuit();
  expect(tauri.invoke).not.toHaveBeenCalled();
});

it("leaves Ctrl+Q alone inside a terminal, where it is XON", () => {
  const terminal = document.createElement("div");
  terminal.className = "monocode-terminal";
  document.body.appendChild(terminal);

  const event = pressQuit({ target: terminal });

  expect(event.defaultPrevented).toBe(false);
  expect(notice()).not.toContain("Hold Ctrl+Q");
  terminal.remove();
});
