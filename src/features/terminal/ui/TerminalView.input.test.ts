// @vitest-environment happy-dom
import { act, createElement } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, expect, it, vi } from "vitest";

const pty = vi.hoisted(() => ({
  spawnPty: vi.fn(async () => {}),
  killPty: vi.fn(async () => {}),
  resizePty: vi.fn(async () => {}),
  writePty: vi.fn(async () => {}),
  subscribePty: vi.fn(() => () => {}),
  getPtyStatus: vi.fn(async () => ({ foreground: null })),
}));
vi.mock("../../../platform/tauri/pty", () => pty);
vi.mock("../model/terminalLayout", () => ({
  fitTerminal: () => null,
  applyTerminalChrome: () => {},
}));
import { TerminalView } from "./TerminalView";

afterEach(() => {
  vi.clearAllMocks();
});

// Real xterm on purpose: the bug lives in how xterm treats a multi-character
// keydown, which a stub cannot reproduce.
async function mountTerminal() {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  vi.stubGlobal(
    "ResizeObserver",
    class {
      observe() {}
      disconnect() {}
    },
  );
  const host = document.createElement("div");
  document.body.appendChild(host);
  const root = createRoot(host);
  await act(async () => {
    root.render(
      createElement(TerminalView, { id: "ime", cwd: "/tmp", active: true }),
    );
  });
  const textarea = host.querySelector("textarea");
  if (!textarea) throw new Error("xterm did not create its textarea");
  return {
    textarea,
    async cleanup() {
      await act(async () => {
        root.unmount();
      });
      host.remove();
      vi.unstubAllGlobals();
    },
  };
}

function imeKeydown(textarea: HTMLTextAreaElement, key: string) {
  // The sequence observed in the real app for GoTiengViet.
  const event = new KeyboardEvent("keydown", {
    key,
    code: "KeyA",
    keyCode: 65,
    isComposing: false,
    bubbles: true,
    cancelable: true,
  });
  textarea.dispatchEvent(event);
  return event;
}

function written() {
  return pty.writePty.mock.calls.map((call) => (call as unknown[])[1]);
}

it("sends the whole string of a multi-character keydown to the PTY", async () => {
  const { textarea, cleanup } = await mountTerminal();
  try {
    const event = imeKeydown(textarea, "ếng");
    await act(async () => {});
    expect(event.defaultPrevented).toBe(true);
    expect(written()).toEqual(["ếng"]);
  } finally {
    await cleanup();
  }
});

// What the browser may send after the keydown. In a real browser none of this
// follows a keydown that was preventDefault()ed; it is dispatched anyway so the
// test also covers xterm's own emit paths if that guarantee ever fails.
function followUps(
  textarea: HTMLTextAreaElement,
  options: { composed: boolean; keypress: boolean },
) {
  const { composed, keypress } = options;
  if (keypress) {
    textarea.dispatchEvent(
      new KeyboardEvent("keypress", {
        key: "ếng",
        code: "KeyA",
        charCode: "ế".charCodeAt(0),
        keyCode: "ế".charCodeAt(0),
        bubbles: true,
        cancelable: true,
      }),
    );
  }
  textarea.dispatchEvent(
    new InputEvent("beforeinput", {
      inputType: "insertText",
      data: "ếng",
      composed,
      bubbles: true,
      cancelable: true,
    }),
  );
  textarea.value = "ếng";
  textarea.dispatchEvent(
    new InputEvent("input", {
      inputType: "insertText",
      data: "ếng",
      composed,
      bubbles: true,
    }),
  );
  textarea.dispatchEvent(
    new KeyboardEvent("keyup", { key: "ếng", code: "KeyA", bubbles: true }),
  );
}

it("sends the text once when the browser follows up as it does for a keydown that was not prevented", async () => {
  const { textarea, cleanup } = await mountTerminal();
  try {
    imeKeydown(textarea, "ếng");
    // The sequence from the observed log: keypress, beforeinput, input.
    followUps(textarea, { composed: true, keypress: true });
    await act(async () => {});
    expect(written()).toEqual(["ếng"]);
  } finally {
    await cleanup();
  }
});

it("honours preventDefault so no follow-up events are needed to stay at one copy", async () => {
  const { textarea, cleanup } = await mountTerminal();
  try {
    const event = imeKeydown(textarea, "ếng");
    // A browser sends nothing after a prevented keydown, only the keyup.
    expect(event.defaultPrevented).toBe(true);
    textarea.dispatchEvent(
      new KeyboardEvent("keyup", { key: "ếng", code: "KeyA", bubbles: true }),
    );
    await act(async () => {});
    expect(written()).toEqual(["ếng"]);
  } finally {
    await cleanup();
  }
});

it("sends the text once when a non-composed input event would follow", async () => {
  const { textarea, cleanup } = await mountTerminal();
  try {
    const event = imeKeydown(textarea, "ếng");
    // xterm would emit a non-composed insertText itself, so the only guard is
    // preventDefault: a browser then sends no beforeinput/input at all.
    if (!event.defaultPrevented) {
      followUps(textarea, { composed: false, keypress: false });
    }
    await act(async () => {});
    expect(written()).toEqual(["ếng"]);
  } finally {
    await cleanup();
  }
});

it("sends the text once for a keypress after the keydown", async () => {
  const { textarea, cleanup } = await mountTerminal();
  try {
    imeKeydown(textarea, "ếng");
    textarea.dispatchEvent(
      new KeyboardEvent("keypress", {
        key: "ếng",
        code: "KeyA",
        charCode: "ế".charCodeAt(0),
        keyCode: "ế".charCodeAt(0),
        bubbles: true,
        cancelable: true,
      }),
    );
    await act(async () => {});
    expect(written()).toEqual(["ếng"]);
  } finally {
    await cleanup();
  }
});

it("still sends a named key as its control sequence", async () => {
  const { textarea, cleanup } = await mountTerminal();
  try {
    textarea.dispatchEvent(
      new KeyboardEvent("keydown", {
        key: "Enter",
        code: "Enter",
        keyCode: 13,
        bubbles: true,
        cancelable: true,
      }),
    );
    await act(async () => {});
    expect(written()).toEqual(["\r"]);
  } finally {
    await cleanup();
  }
});

it("leaves a single character to xterm", async () => {
  const { textarea, cleanup } = await mountTerminal();
  try {
    textarea.dispatchEvent(
      new KeyboardEvent("keydown", {
        key: "a",
        code: "KeyA",
        keyCode: 65,
        bubbles: true,
        cancelable: true,
      }),
    );
    await act(async () => {});
    expect(written()).toEqual(["a"]);
  } finally {
    await cleanup();
  }
});
