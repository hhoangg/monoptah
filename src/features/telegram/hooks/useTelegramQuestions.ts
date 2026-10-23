import { useEffect, useRef } from "react";
import type { UnlistenFn } from "@tauri-apps/api/event";
import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";
import { projectKey } from "../../../shared/lib/paths";
import type { Session } from "../../sessions/model/session";
import {
  CUSTOM_OPTION_ID,
  type UserQuestionReply,
} from "../../sessions/model/userQuestion";
import {
  discardQuestionOutcome,
  peekQuestionOutcome,
  takeQuestionOutcome,
} from "../model/questionOutcome";
import {
  TELEGRAM_ANSWER_EVENT,
  telegramAnswerResult,
  telegramForwardQuestion,
  telegramQuestionClosed,
  type TelegramAnswerEvent,
  type TelegramQuestionStatus,
  type TelegramStatus,
} from "../model/telegram";
import { cachedTelegramStatus } from "../model/telegramStatusCache";
import {
  dueForwards,
  forwardPayload,
  isForwardable,
} from "../model/telegramForward";

const TICK_MS = 15_000;
const MAX_FORWARD_ATTEMPTS = 2;

type UseTelegramQuestionsInput = {
  sessions: Session[];
  onQuestionReply: (
    sessionId: string,
    requestId: number,
    reply: UserQuestionReply,
  ) => void;
  now?: () => number;
};

/** Whether a question may ever be forwarded, decided when it is first seen. */
type Eligibility = "checking" | "yes" | "no";

type SentQuestion = { sessionId: string; requestId: number };

function questionKey(sessionId: string, requestId: number): string {
  return `${sessionId}#${requestId}`;
}

/**
 * Whether a reply from Telegram fits the question that is pending now. A request
 * id alone is not enough: ids restart after a window reload, so an old message
 * could otherwise answer a different question with the old option ids.
 */
function replyFitsQuestion(
  reply: UserQuestionReply,
  prompt: NonNullable<Session["pendingQuestion"]>,
): boolean {
  if (reply.kind !== "answered") return true;
  const known = new Map(
    prompt.questions.map((question) => [
      question.id,
      new Set(question.options.map((option) => option.id)),
    ]),
  );
  for (const [questionId, optionIds] of Object.entries(reply.answers)) {
    const options = known.get(questionId);
    if (!options) return false;
    if (optionIds.some((id) => id !== CUSTOM_OPTION_ID && !options.has(id))) {
      return false;
    }
  }
  return Object.keys(reply.custom ?? {}).every((id) => known.has(id));
}

function canForward(status: TelegramStatus | undefined): boolean {
  return !!status?.configured && status.enabled && status.chatId != null;
}

function closedStatus(
  sessionId: string,
  requestId: number,
): TelegramQuestionStatus {
  const decision = takeQuestionOutcome(sessionId, requestId);
  if (decision === "answered") return "Answered on desktop";
  if (decision === "skipped") return "Skipped";
  return "Cancelled";
}

/**
 * Forwards questions that stay unanswered for a while to Telegram, mirrors
 * their desktop outcome there, and routes Telegram answers back through the
 * same reply path the in-app question form uses.
 */
export function useTelegramQuestions(input: UseTelegramQuestionsInput): void {
  const { sessions, onQuestionReply, now = Date.now } = input;
  const sessionsRef = useRef(sessions);
  sessionsRef.current = sessions;
  const replyRef = useRef(onQuestionReply);
  replyRef.current = onQuestionReply;
  const nowRef = useRef(now);
  nowRef.current = now;
  const runRef = useRef<{ sync: () => void; forward: () => void } | null>(null);

  useEffect(() => {
    let disposed = false;
    const firstSeen = new Map<string, number>();
    const eligibility = new Map<string, Eligibility>();
    const sent = new Set<string>();
    const sentInfo = new Map<string, SentQuestion>();
    const inFlight = new Set<string>();
    const attempts = new Map<string, number>();
    const byTelegram = new Set<string>();
    const checking = new Set<string>();

    function statusFor(project: string): Promise<TelegramStatus | undefined> {
      return cachedTelegramStatus(project, nowRef.current());
    }

    /**
     * Decides eligibility from a successful status. A failed call decides
     * nothing: the question stays "checking" and the next tick asks again, so
     * only a status that actually says "off" rules a question out for good.
     */
    function checkEligibility(key: string, session: Session) {
      if (checking.has(key)) return;
      checking.add(key);
      void statusFor(projectKey(session.cwd)).then((status) => {
        checking.delete(key);
        if (disposed || eligibility.get(key) !== "checking") return;
        if (status === undefined) return;
        eligibility.set(key, canForward(status) ? "yes" : "no");
      });
    }

    function forget(key: string) {
      firstSeen.delete(key);
      eligibility.delete(key);
      attempts.delete(key);
      inFlight.delete(key);
      checking.delete(key);
    }

    function sync(retryChecks = false) {
      if (disposed) return;
      const pending = new Set<string>();
      for (const session of sessionsRef.current) {
        const question = session.pendingQuestion;
        if (!question) continue;
        const key = questionKey(session.id, question.requestId);
        pending.add(key);
        if (eligibility.has(key)) {
          if (retryChecks && eligibility.get(key) === "checking") {
            checkEligibility(key, session);
          }
          continue;
        }
        // Request ids restart per Live, so an outcome stored for this key
        // belongs to an earlier question and must not describe this one.
        discardQuestionOutcome(session.id, question.requestId);
        // Decided once, when first seen: a later change must not flip it.
        // Questions with an auto-skip deadline never block the agent.
        if (question.autoResolveAt != null || !isForwardable(session)) {
          eligibility.set(key, "no");
          continue;
        }
        eligibility.set(key, "checking");
        firstSeen.set(key, nowRef.current());
        checkEligibility(key, session);
      }

      for (const key of [...sent]) {
        const info = sentInfo.get(key);
        if (!info || pending.has(key)) continue;
        sent.delete(key);
        sentInfo.delete(key);
        const status = closedStatus(info.sessionId, info.requestId);
        if (byTelegram.delete(key)) continue;
        void telegramQuestionClosed(
          info.sessionId,
          info.requestId,
          status,
        ).catch(() => undefined);
      }

      for (const key of [...eligibility.keys()]) {
        if (!pending.has(key) && !inFlight.has(key)) forget(key);
      }
    }

    async function forwardOne(session: Session, key: string) {
      const question = session.pendingQuestion;
      if (!question) return;
      inFlight.add(key);
      try {
        const project = projectKey(session.cwd);
        if (!canForward(await statusFor(project)) || disposed) return;
        const latest = sessionsRef.current.find(
          (item) => item.id === session.id,
        );
        if (latest?.pendingQuestion?.requestId !== question.requestId) return;
        const payload = forwardPayload(latest, project);
        if (!payload) return;
        try {
          await telegramForwardQuestion(payload);
          sent.add(key);
          sentInfo.set(key, {
            sessionId: session.id,
            requestId: question.requestId,
          });
        } catch {
          attempts.set(key, (attempts.get(key) ?? 0) + 1);
        }
      } finally {
        inFlight.delete(key);
        sync();
      }
    }

    function forward() {
      if (disposed) return;
      const candidates = sessionsRef.current.filter((session) => {
        const question = session.pendingQuestion;
        if (!question) return false;
        const key = questionKey(session.id, question.requestId);
        return (
          eligibility.get(key) === "yes" &&
          !inFlight.has(key) &&
          (attempts.get(key) ?? 0) < MAX_FORWARD_ATTEMPTS
        );
      });
      for (const session of dueForwards(
        candidates,
        firstSeen,
        sent,
        nowRef.current(),
      )) {
        const key = questionKey(session.id, session.pendingQuestion!.requestId);
        void forwardOne(session, key);
      }
    }

    runRef.current = { sync, forward };
    sync();
    const timer = window.setInterval(() => {
      sync(true);
      forward();
    }, TICK_MS);

    let stopListening: UnlistenFn | undefined;
    function onAnswer(event: { payload: TelegramAnswerEvent }) {
      const { key: resultKey, sessionId, requestId, reply } = event.payload;
      const session = sessionsRef.current.find((item) => item.id === sessionId);
      let ok = false;
      const pending = session?.pendingQuestion;
      // A desktop answer records its outcome when it is queued, before the
      // session state drops the question, so a recorded outcome means the
      // question is already resolved even though it still looks pending.
      if (
        pending?.requestId === requestId &&
        peekQuestionOutcome(sessionId, requestId) === undefined &&
        replyFitsQuestion(reply, pending)
      ) {
        const key = questionKey(sessionId, requestId);
        byTelegram.add(key);
        try {
          replyRef.current(sessionId, requestId, reply);
          ok = true;
        } catch {
          byTelegram.delete(key);
        }
      }
      void telegramAnswerResult(resultKey, ok).catch(() => undefined);
    }
    try {
      // Scoped to this window: a global `listen` is registered as `Any` and
      // would also receive answers emitted to the window that owns the question.
      void getCurrentWebviewWindow()
        .listen<TelegramAnswerEvent>(TELEGRAM_ANSWER_EVENT, onAnswer)
        .then((stop) => {
          if (disposed) stop();
          else stopListening = stop;
        })
        .catch(() => undefined);
    } catch {
      // Outside Tauri there is no event bus.
    }

    return () => {
      disposed = true;
      runRef.current = null;
      window.clearInterval(timer);
      stopListening?.();
      stopListening = undefined;
    };
  }, []);

  useEffect(() => {
    runRef.current?.sync();
    runRef.current?.forward();
  }, [sessions]);
}
