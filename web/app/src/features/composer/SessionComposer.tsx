// The composer of an open session (W-08) and the cards that sit above it: the pending question (W-10, F-58) and
// the usage limit card (U-07). `SessionScreen` mounts `SessionQuestionCard` and `SessionUsageCard` in the `above`
// part of the composer region and `SessionComposer` as its children.
import { useCallback, useState } from "react";
import { useNavigate } from "@tanstack/react-router";
import type { QuestionPrompt, QuestionReply, SessionCard, UsageLimit } from "@monoptah/protocol";
import type { SendOptions, SendResult } from "@/data";
import { useLinkMutation } from "@/data";
import { useLinkDesktopId, useLinkState } from "@/link";
import { ModelControls } from "@/features/models/ModelControls";
import { useDefaultModels } from "@/features/models/defaultModel";
import { UsageLimitCard } from "@/features/models/UsageLimitCard";
import { useUsageLimitActions } from "@/features/models/hooks";
import { assertFitsRequest, toWire } from "./attachments";
import { Composer } from "./Composer";
import { readChatDraft, writeChatDraft } from "./chatDraft";
import { useRequestControl } from "./useRequestControl";
import { QuestionForm } from "./QuestionForm";
import { sendErrorMessage } from "./sendErrors";
import { WAKE_REQUIRED_NOTICE } from "@/ui/hooks";
import { SendCancelled, SendRefused, useSendGuard } from "./useSendGuard";

export const QUESTION_READ_ONLY_REASON = "Answer on desktop or use a control device.";

/** True while the link to the desktop is up and its requests can be sent. */
export function useLinkReady(): boolean {
  return useLinkState((state) => state.status) === "ready";
}

export interface SessionComposerProps {
  desktopId: string;
  sessionId: string;
  projectId: string | undefined;
  session: SessionCard | undefined;
  busy: boolean;
  readOnly: boolean;
  /** Sends the text and the attachments as a turn (the data layer's `useConversation().send`). */
  send: (text: string, attachments?: ReturnType<typeof toWire>, options?: SendOptions) => Promise<SendResult>;
  /** The latest session error, shown in place of the field until dismissed. */
  notice?: { text: string; dismiss: () => void } | null;
}

export function SessionComposer({ desktopId, sessionId, projectId, session, busy, readOnly, send, notice }: SessionComposerProps) {
  const ready = useLinkReady();
  const navigate = useNavigate();
  const guard = useSendGuard();
  const requestControl = useRequestControl(desktopId);
  const cancel = useLinkMutation("session.cancel");
  const defaults = useDefaultModels(desktopId).defaults;
  const [configError, setConfigError] = useState<string | null>(null);
  // The typed text survives a remount (a tab switch, a split): it is read once, when the composer mounts.
  const [initialText] = useState(() => readChatDraft(desktopId, sessionId));
  const rememberText = useCallback((text: string) => writeChatDraft(desktopId, sessionId, text), [desktopId, sessionId]);
  const defaultModelId = defaults.harness ? defaults.models[defaults.harness]?.model : undefined;

  const submit = async (text: string, attachments: Parameters<typeof toWire>[0]) => {
    assertFitsRequest(text, attachments);
    const wire = toWire(attachments);
    // The optimistic turn shows the message at once; a failure appears there as "Not sent, retry". Only a
    // `wake_required` refusal rejects here, so the composer keeps the draft until the user confirms the wake
    // cost and the desktop accepts the send.
    const result = await guard.guardedSend({ desktopId, sessionId, target: session?.target ?? null }, (options) =>
      send(text, wire.length > 0 ? wire : undefined, options),
    );
    if (!result.sent && result.error === "wake_required") throw new Error(sendErrorMessage(result.error));
    // The composer may have unmounted while the send was in flight (a tab switch): its own clearing never ran, so the
    // draft would come back and be sent twice. Drop it when it still is the sent text.
    if (readChatDraft(desktopId, sessionId).trim() === text.trim()) writeChatDraft(desktopId, sessionId, "");
  };

  const toolbar = (
    <ModelControls
      conversationId={sessionId}
      projectId={projectId}
      defaultModelId={defaultModelId}
      disabled={readOnly}
      busy={busy}
      onError={setConfigError}
      onSwitchAccount={() => {
        if (projectId) void navigate({ to: "/d/$desktopId/p/$projectId/new", params: { desktopId, projectId } });
      }}
    />
  );

  return (
    <>
      <Composer
        initialText={initialText}
        onTextChange={rememberText}
        onSubmit={submit}
        busy={busy}
        onStop={async () => {
          await cancel.mutateAsync({ conversationId: sessionId });
        }}
        readOnly={readOnly}
        requestControl={requestControl}
        offline={!ready}
        notice={notice ?? null}
        toolbar={toolbar}
        configError={configError}
      />
      {guard.dialog}
    </>
  );
}

/** F-58: the pending question above the composer. The reply goes through `session.answerQuestion`. */
export function SessionQuestionCard({ sessionId, prompt, readOnly }: { sessionId: string; prompt: QuestionPrompt; readOnly: boolean }) {
  const answer = useLinkMutation("session.answerQuestion");
  const keepOpen = useLinkMutation("session.keepQuestionOpen");
  return (
    <div className="pb-2" data-testid="question-card">
      <QuestionForm
        prompt={prompt}
        {...(readOnly ? { disabledReason: QUESTION_READ_ONLY_REASON } : {})}
        onReply={async (requestId: number, reply: QuestionReply) => {
          await answer.mutateAsync({ conversationId: sessionId, requestId, reply });
        }}
        onKeepOpen={async (requestId: number) => {
          await keepOpen.mutateAsync({ conversationId: sessionId, requestId });
        }}
      />
    </div>
  );
}

/** True when the desktop refused the request because the cloud instance is stopped and no wake was confirmed. */
function isWakeRequired(reason: unknown): boolean {
  return typeof reason === "object" && reason !== null && (reason as { code?: unknown }).code === "wake_required";
}

/**
 * The usage limit card above the composer while the conversation waits for its limit to reset. Resume and
 * Resume at reset run through the send-guard like a message: a stopped cloud instance asks for the wake cost
 * first, and the request carries `wake: true` only after that confirmation (once per confirmation).
 */
export function SessionUsageCard({
  sessionId,
  state,
  readOnly,
  desktopId,
  target,
}: {
  sessionId: string;
  state: UsageLimit;
  readOnly: boolean;
  desktopId?: string;
  target?: SessionCard["target"] | null;
}) {
  const ready = useLinkReady();
  const actions = useUsageLimitActions(sessionId, state);
  const guard = useSendGuard();
  const linkDesktopId = useLinkDesktopId();

  const guarded = async (call: (options: { wake?: boolean }) => Promise<void>): Promise<void> => {
    try {
      const { wake } = await guard.check({ desktopId: desktopId ?? linkDesktopId, sessionId, target: target ?? null });
      try {
        await call(wake ? { wake: true } : {});
      } catch (reason) {
        if (wake || !isWakeRequired(reason)) throw reason;
        await guard.confirm(WAKE_REQUIRED_NOTICE);
        await call({ wake: true });
      }
    } catch (reason) {
      if (reason instanceof SendCancelled) return;
      if (reason instanceof SendRefused) throw new Error(reason.message, { cause: reason });
      if (isWakeRequired(reason)) throw new Error(sendErrorMessage("wake_required"), { cause: reason });
      throw reason;
    }
  };

  return (
    <div className="pb-2" data-testid="usage-limit-card">
      <UsageLimitCard
        state={state}
        connected={ready}
        readOnly={readOnly}
        onResume={() => guarded((options) => actions.resume(state.token, options))}
        onResumeAtReset={(enabled) => guarded((options) => actions.resumeAtReset(state.token, enabled, options))}
      />
      {guard.dialog}
    </div>
  );
}
