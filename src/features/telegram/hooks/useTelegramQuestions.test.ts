// @vitest-environment happy-dom
import { act, createElement, StrictMode } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { newSession, type Session } from "../../sessions/model/session";
import type { UserQuestionReply } from "../../sessions/model/userQuestion";
import { FORWARD_DELAY_MS } from "../model/telegramForward";
import { recordQuestionOutcome } from "../model/questionOutcome";
import type { TelegramAnswerEvent, TelegramStatus } from "../model/telegram";
import { useTelegramQuestions } from "./useTelegramQuestions";

const invoke = vi.hoisted(() => vi.fn());
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
const listen = vi.hoisted(() => vi.fn());
const globalListen = vi.hoisted(() => vi.fn());
vi.mock("@tauri-apps/api/event", () => ({ listen: globalListen }));
vi.mock("@tauri-apps/api/webviewWindow", () => ({
  getCurrentWebviewWindow: () => ({ listen }),
}));

const TICK_MS = 15_000;

const enabled: TelegramStatus = {
  configured: true,
  enabled: true,
  botUsername: "bot",
  chatId: -100,
};

function prompt(
  requestId: number,
  autoResolveAt?: number,
): NonNullable<Session["pendingQuestion"]> {
  return {
    requestId,
    ...(autoResolveAt !== undefined ? { autoResolveAt } : {}),
    questions: [
      {
        id: "q1",
        prompt: "Which one?",
        multiSelect: false,
        allowCustom: false,
        options: [{ id: "a", label: "A" }],
      },
    ],
  };
}

function asking(requestId = 1, id = "s1", autoResolveAt?: number): Session {
  return {
    ...newSession("codex", "/repo"),
    id,
    blocks: [],
    pendingQuestion: prompt(requestId, autoResolveAt),
  };
}

function idle(session: Session): Session {
  return { ...session, pendingQuestion: undefined };
}

function Probe({
  sessions,
  onQuestionReply,
  now,
}: {
  sessions: Session[];
  onQuestionReply: (
    sessionId: string,
    requestId: number,
    reply: UserQuestionReply,
  ) => void;
  now: () => number;
}) {
  useTelegramQuestions({ sessions, onQuestionReply, now });
  return null;
}

function callsTo(command: string) {
  return invoke.mock.calls
    .filter(([name]) => name === command)
    .map(([, args]) => args);
}

describe("useTelegramQuestions", () => {
  let root: Root;
  let container: HTMLDivElement;
  let clock: number;
  let status: TelegramStatus | Error;
  let forwardError: Error | undefined;
  let answerHandler: ((event: { payload: TelegramAnswerEvent }) => void) | null;
  let unlisten: ReturnType<typeof vi.fn>;
  let onQuestionReply: ReturnType<typeof vi.fn>;

  async function render(sessions: Session[]) {
    await act(async () =>
      root.render(
        createElement(
          StrictMode,
          null,
          createElement(Probe, {
            sessions,
            onQuestionReply,
            now: () => clock,
          }),
        ),
      ),
    );
  }

  async function advance(ms: number) {
    clock += ms;
    await act(async () => {
      await vi.advanceTimersByTimeAsync(ms);
    });
  }

  async function deliver(partial: Partial<TelegramAnswerEvent> = {}) {
    const event: TelegramAnswerEvent = {
      key: "pending-1",
      sessionId: "s1",
      requestId: 1,
      reply: { kind: "answered", answers: { q1: ["a"] } },
      by: "@someone",
      ...partial,
    };
    await act(async () => {
      answerHandler?.({ payload: event });
    });
    return event;
  }

  beforeEach(async () => {
    vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
    vi.useFakeTimers();
    clock = 1_000_000;
    status = enabled;
    forwardError = undefined;
    answerHandler = null;
    unlisten = vi.fn();
    onQuestionReply = vi.fn();
    invoke.mockReset();
    invoke.mockImplementation(async (command: string) => {
      if (command === "telegram_status") {
        if (status instanceof Error) throw status;
        return status;
      }
      if (command === "telegram_forward_question" && forwardError) {
        throw forwardError;
      }
    });
    listen.mockReset();
    globalListen.mockReset();
    listen.mockImplementation(
      async (_event: string, handler: typeof answerHandler) => {
        answerHandler = handler;
        return unlisten;
      },
    );
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

  it("forwards after 5 minutes when enabled", async () => {
    await render([asking()]);
    await advance(FORWARD_DELAY_MS - TICK_MS);
    expect(callsTo("telegram_forward_question")).toHaveLength(0);
    await advance(TICK_MS * 2);
    const forwards = callsTo("telegram_forward_question");
    expect(forwards).toHaveLength(1);
    expect(forwards[0]).toMatchObject({
      payload: { sessionId: "s1", requestId: 1 },
    });
    await advance(TICK_MS * 4);
    expect(callsTo("telegram_forward_question")).toHaveLength(1);
  });

  it("does not forward when answered within 5 minutes", async () => {
    const session = asking();
    await render([session]);
    await advance(FORWARD_DELAY_MS - 60_000);
    await render([idle(session)]);
    await advance(FORWARD_DELAY_MS);
    expect(callsTo("telegram_forward_question")).toHaveLength(0);
    expect(callsTo("telegram_question_closed")).toHaveLength(0);
  });

  it("never forwards a question first seen while disabled", async () => {
    status = { ...enabled, enabled: false };
    await render([asking()]);
    await advance(FORWARD_DELAY_MS + TICK_MS);
    status = enabled;
    await advance(120_000);
    expect(callsTo("telegram_forward_question")).toHaveLength(0);
  });

  it("never forwards a question first seen while status is unavailable", async () => {
    status = new Error("offline");
    await render([asking()]);
    await advance(FORWARD_DELAY_MS);
    status = enabled;
    await advance(120_000);
    expect(callsTo("telegram_forward_question")).toHaveLength(0);
  });

  it("never forwards a non-blocking question even after its timer is cleared", async () => {
    const timed = asking(1, "s1", clock + 120_000);
    await render([timed]);
    await advance(TICK_MS);
    await render([
      { ...timed, pendingQuestion: prompt(1) as Session["pendingQuestion"] },
    ]);
    await advance(FORWARD_DELAY_MS + TICK_MS);
    expect(callsTo("telegram_forward_question")).toHaveLength(0);
  });

  it("subscribes through the current webview window, not the global listen", async () => {
    await render([asking()]);
    expect(listen).toHaveBeenCalledWith(
      "monocode-telegram-answer",
      expect.any(Function),
    );
    expect(globalListen).not.toHaveBeenCalled();
  });

  it("routes answer to onQuestionReply", async () => {
    await render([asking()]);
    const event = await deliver();
    expect(onQuestionReply).toHaveBeenCalledWith("s1", 1, event.reply);
    expect(callsTo("telegram_answer_result")).toEqual([
      { key: "pending-1", ok: true },
    ]);
  });

  it("ignores answer for a different requestId", async () => {
    await render([asking(2)]);
    await deliver({ requestId: 1 });
    expect(onQuestionReply).not.toHaveBeenCalled();
    expect(callsTo("telegram_answer_result")).toEqual([
      { key: "pending-1", ok: false },
    ]);
  });

  it("reports stale when no longer pending", async () => {
    await render([idle(asking())]);
    await deliver();
    expect(onQuestionReply).not.toHaveBeenCalled();
    expect(callsTo("telegram_answer_result")).toEqual([
      { key: "pending-1", ok: false },
    ]);
  });

  it("reports stale for an unknown session", async () => {
    await render([]);
    await deliver();
    expect(callsTo("telegram_answer_result")).toEqual([
      { key: "pending-1", ok: false },
    ]);
  });

  it("reports failure once when onQuestionReply throws", async () => {
    onQuestionReply.mockImplementation(() => {
      throw new Error("boom");
    });
    await render([asking()]);
    await deliver();
    expect(callsTo("telegram_answer_result")).toEqual([
      { key: "pending-1", ok: false },
    ]);
  });

  it("answers against the latest sessions, not a stale render", async () => {
    const session = asking();
    await render([session]);
    await render([idle(session)]);
    await deliver();
    expect(onQuestionReply).not.toHaveBeenCalled();
  });

  async function forwarded(session: Session) {
    await render([session]);
    await advance(FORWARD_DELAY_MS + TICK_MS);
    expect(callsTo("telegram_forward_question")).toHaveLength(1);
  }

  it("reports desktop answer as closed", async () => {
    const session = asking();
    await forwarded(session);
    recordQuestionOutcome("s1", 1, "answered");
    await render([idle(session)]);
    expect(callsTo("telegram_question_closed")).toEqual([
      { sessionId: "s1", requestId: 1, status: "Answered on desktop" },
    ]);
    await advance(TICK_MS * 3);
    expect(callsTo("telegram_question_closed")).toHaveLength(1);
  });

  it("reports a skipped question as closed", async () => {
    const session = asking();
    await forwarded(session);
    recordQuestionOutcome("s1", 1, "skipped");
    await render([idle(session)]);
    expect(callsTo("telegram_question_closed")).toEqual([
      { sessionId: "s1", requestId: 1, status: "Skipped" },
    ]);
  });

  it("reports a cancelled turn as closed", async () => {
    const session = asking();
    await forwarded(session);
    recordQuestionOutcome("s1", 1, "cancelled");
    await render([idle(session)]);
    expect(callsTo("telegram_question_closed")).toEqual([
      { sessionId: "s1", requestId: 1, status: "Cancelled" },
    ]);
  });

  it("reports a vanished session as cancelled", async () => {
    await forwarded(asking());
    await render([]);
    expect(callsTo("telegram_question_closed")).toEqual([
      { sessionId: "s1", requestId: 1, status: "Cancelled" },
    ]);
  });

  it("reports a replaced request id as closed", async () => {
    await forwarded(asking(1));
    await render([asking(2)]);
    expect(callsTo("telegram_question_closed")).toEqual([
      { sessionId: "s1", requestId: 1, status: "Cancelled" },
    ]);
  });

  it("does not report closure for its own delivery", async () => {
    const session = asking();
    await forwarded(session);
    await deliver();
    recordQuestionOutcome("s1", 1, "answered");
    await render([idle(session)]);
    await advance(TICK_MS * 2);
    expect(callsTo("telegram_question_closed")).toHaveLength(0);
  });

  it("retries a failed forward once on the next tick, then stops", async () => {
    forwardError = new Error("rate limited");
    await render([asking()]);
    await advance(FORWARD_DELAY_MS + TICK_MS);
    await advance(TICK_MS * 6);
    expect(callsTo("telegram_forward_question")).toHaveLength(2);
    forwardError = undefined;
    await advance(TICK_MS * 6);
    expect(callsTo("telegram_forward_question")).toHaveLength(2);
  });

  it("recovers after a failed forward when the retry succeeds", async () => {
    forwardError = new Error("network");
    await render([asking()]);
    for (let tick = 0; tick < FORWARD_DELAY_MS / TICK_MS; tick++) {
      await advance(TICK_MS);
    }
    expect(callsTo("telegram_forward_question")).toHaveLength(1);
    forwardError = undefined;
    await advance(TICK_MS);
    expect(callsTo("telegram_forward_question")).toHaveLength(2);
    await advance(TICK_MS * 3);
    expect(callsTo("telegram_forward_question")).toHaveLength(2);
    await render([]);
    expect(callsTo("telegram_question_closed")).toHaveLength(1);
  });

  it("caches the project status between ticks", async () => {
    await render([asking()]);
    await advance(TICK_MS * 3);
    expect(callsTo("telegram_status")).toHaveLength(1);
  });

  it("stops listening and ticking after unmount", async () => {
    await render([asking()]);
    await act(async () => root.unmount());
    expect(unlisten).toHaveBeenCalled();
    invoke.mockClear();
    await advance(FORWARD_DELAY_MS * 2);
    expect(invoke).not.toHaveBeenCalled();
    root = createRoot(container);
  });

  it("unlistens when unmounted before the listener is ready", async () => {
    let ready!: (stop: () => void) => void;
    listen.mockImplementation(
      () => new Promise<() => void>((resolve) => (ready = resolve)),
    );
    await render([asking()]);
    await act(async () => root.unmount());
    await act(async () => ready(unlisten));
    expect(unlisten).toHaveBeenCalled();
    root = createRoot(container);
  });
});
