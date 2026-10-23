// @vitest-environment happy-dom

import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { copyText } from "../../../platform/tauri/clipboard";
import { LinkTelegramTopicDialog } from "./LinkTelegramTopicDialog";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("../../../platform/tauri/clipboard", () => ({
  copyText: vi.fn(async () => undefined),
}));

const PROJECT = "/work/alpha";
const SESSION = "session-1";
const FIFTEEN_MINUTES = 15 * 60_000;

let root: Root;
let container: HTMLDivElement;
let topic: number | null;
let codes: string[];
let linkError: string | null;

function mockInvoke() {
  vi.mocked(invoke).mockImplementation(async (command) => {
    if (command === "telegram_session_topic") return topic;
    if (command === "telegram_link_code") {
      if (linkError) throw linkError;
      const code = codes.shift() ?? "LATE";
      return { code, command: `/link@alpha_bot ${code}` };
    }
    throw new Error(`Unexpected command: ${String(command)}`);
  });
}

async function renderDialog(onClose = vi.fn()) {
  await act(async () =>
    root.render(
      createElement(LinkTelegramTopicDialog, {
        project: PROJECT,
        sessionId: SESSION,
        sessionTitle: "Fix login",
        onClose,
      }),
    ),
  );
}

function dialog() {
  return document.querySelector<HTMLElement>('[role="dialog"]')!;
}

function button(label: string) {
  return [...dialog().querySelectorAll("button")].find((candidate) =>
    candidate.textContent?.includes(label),
  );
}

function topicCalls() {
  return vi
    .mocked(invoke)
    .mock.calls.filter(([command]) => command === "telegram_session_topic");
}

beforeEach(() => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  topic = null;
  codes = ["CODE1", "CODE2"];
  linkError = null;
  vi.mocked(invoke).mockReset();
  vi.mocked(copyText).mockClear();
  mockInvoke();
  container = document.createElement("div");
  document.body.append(container);
  root = createRoot(container);
});

afterEach(async () => {
  await act(async () => root.unmount());
  container.remove();
  vi.useRealTimers();
  vi.unstubAllGlobals();
});

describe("link telegram topic dialog", () => {
  it("shows link command", async () => {
    await renderDialog();

    expect(invoke).toHaveBeenCalledWith("telegram_link_code", {
      project: PROJECT,
      sessionId: SESSION,
    });
    expect(dialog().textContent).toContain(
      "Send this in the topic you want to use:",
    );
    expect(dialog().querySelector("code")?.textContent).toBe(
      "/link@alpha_bot CODE1",
    );

    await act(async () => button("Copy")!.click());
    expect(copyText).toHaveBeenCalledWith("/link@alpha_bot CODE1");
    expect(dialog().textContent).toContain("Copied");
  });

  it("shows linked when binding appears", async () => {
    vi.useFakeTimers();
    topic = 11;
    await renderDialog();
    expect(dialog().textContent).not.toContain("Linked");

    // The same binding is not a new link.
    await act(async () => {
      await vi.advanceTimersByTimeAsync(3000);
    });
    expect(dialog().textContent).not.toContain("Linked");

    topic = 42;
    await act(async () => {
      await vi.advanceTimersByTimeAsync(3000);
    });
    expect(dialog().textContent).toContain("Linked");
    expect(dialog().querySelector("code")).toBeNull();
  });

  it("shows linked when a session without a topic gets one", async () => {
    vi.useFakeTimers();
    await renderDialog();

    topic = 7;
    await act(async () => {
      await vi.advanceTimersByTimeAsync(3000);
    });
    expect(dialog().textContent).toContain("Linked");
  });

  it("offers a new code after 15 minutes", async () => {
    vi.useFakeTimers();
    await renderDialog();
    expect(button("New code")).toBeUndefined();

    await act(async () => {
      await vi.advanceTimersByTimeAsync(FIFTEEN_MINUTES);
    });
    expect(dialog().textContent).toContain("This code has expired");
    expect(button("New code")).toBeDefined();

    await act(async () => button("New code")!.click());
    expect(dialog().querySelector("code")?.textContent).toBe(
      "/link@alpha_bot CODE2",
    );
    expect(button("New code")).toBeUndefined();
    expect(
      vi
        .mocked(invoke)
        .mock.calls.filter(([command]) => command === "telegram_link_code"),
    ).toHaveLength(2);
  });

  it("stops polling when closed", async () => {
    vi.useFakeTimers();
    await renderDialog();
    await act(async () => {
      await vi.advanceTimersByTimeAsync(3000);
    });
    expect(topicCalls().length).toBeGreaterThan(1);

    await act(async () => root.unmount());
    vi.mocked(invoke).mockClear();
    await vi.advanceTimersByTimeAsync(FIFTEEN_MINUTES);
    expect(invoke).not.toHaveBeenCalled();
    root = createRoot(container);
  });

  it("shows an inline error when the code cannot be created", async () => {
    linkError = "Telegram is not connected";
    await renderDialog();

    expect(dialog().querySelector('[role="alert"]')?.textContent).toBe(
      "Telegram is not connected",
    );
    expect(dialog().querySelector("code")).toBeNull();
  });
});
