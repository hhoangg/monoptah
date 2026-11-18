// The composer of an open session (W-08) and the cards that sit above it: the pending question (W-10, F-58) and
// the usage limit card (U-07). `SessionScreen` mounts `SessionQuestionCard` and `SessionUsageCard` in the `above`
// part of the composer region and `SessionComposer` as its children.
import { useState } from "react";
import { useNavigate } from "@tanstack/react-router";
import type { QuestionPrompt, QuestionReply, SessionCard, UsageLimit } from "@monoptah/protocol";
import type { SendResult } from "@/data";
import { useLinkMutation } from "@/data";
import { useLinkState } from "@/link";
import { ModelControls } from "@/features/models/ModelControls";
import { useDefaultModels } from "@/features/models/defaultModel";
import { UsageLimitCard } from "@/features/models/UsageLimitCard";
import { useUsageLimitActions } from "@/features/models/hooks";
import { assertFitsRequest, toWire } from "./attachments";
import { Composer } from "./Composer";
import { QuestionForm } from "./QuestionForm";
import { useSendGuard } from "./useSendGuard";

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
  send: (text: string, attachments?: ReturnType<typeof toWire>) => Promise<SendResult>;
  /** The latest session error, shown in place of the field until dismissed. */
  notice?: { text: string; dismiss: () => void } | null;
}

export function SessionComposer({ desktopId, sessionId, projectId, session, busy, readOnly, send, notice }: SessionComposerProps) {
  const ready = useLinkReady();
  const navigate = useNavigate();
  const guard = useSendGuard();
  const cancel = useLinkMutation("session.cancel");
  const defaults = useDefaultModels(desktopId).defaults;
  const [configError, setConfigError] = useState<string | null>(null);
  const defaultModelId = defaults.harness ? defaults.models[defaults.harness]?.model : undefined;

  const submit = async (text: string, attachments: Parameters<typeof toWire>[0]) => {
    assertFitsRequest(text, attachments);
    await guard.check({ desktopId, sessionId, target: session?.target ?? null });
    // The optimistic turn shows the message at once; a failure appears there as "Not sent, retry".
    const wire = toWire(attachments);
    void send(text, wire.length > 0 ? wire : undefined);
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
        onSubmit={submit}
        busy={busy}
        onStop={async () => {
          await cancel.mutateAsync({ conversationId: sessionId });
        }}
        readOnly={readOnly}
        offline={!ready}
        notice={notice ?? null}
        toolbar={toolbar}
      />
      {configError ? (
        <p role="alert" className="-mt-2 px-4 pb-2 text-meta text-deletions">
          {configError}
        </p>
      ) : null}
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

/** The usage limit card above the composer while the conversation waits for its limit to reset. */
export function SessionUsageCard({ sessionId, state, readOnly }: { sessionId: string; state: UsageLimit; readOnly: boolean }) {
  const ready = useLinkReady();
  const actions = useUsageLimitActions(sessionId, state);
  return (
    <div className="pb-2" data-testid="usage-limit-card">
      <UsageLimitCard
        state={state}
        connected={ready}
        readOnly={readOnly}
        onResume={() => actions.resume(state.token)}
        onResumeAtReset={(enabled) => actions.resumeAtReset(state.token, enabled)}
      />
    </div>
  );
}
