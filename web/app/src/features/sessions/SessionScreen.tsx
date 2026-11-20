// `/d/:desktopId/s/:sessionId` (W-07): the header (back, title, the `session-header-chip` slot host), the
// transcript of the conversation and the composer region (Task 5 mounts the composer there).
import { BACKGROUND_CONTAINER_CLASS, ChatBackground } from "@/features/backgrounds";
import { ModelNamesProvider } from "@/features/transcript/modelNames";
import { Link, useNavigate } from "@tanstack/react-router";
import { useMemo, useState } from "react";
import { useCatalog, useConversation } from "@/data";
import { useGuardedReplies } from "@/features/composer/useSendGuard";
import { SessionComposer, SessionQuestionCard, SessionUsageCard } from "@/features/composer/SessionComposer";
import { useMentionPreview } from "@/features/filePreview/useMentionPreview";
import { DocumentDialog } from "@/features/transcript/DocumentDialog";
import { Transcript } from "@/features/transcript/Transcript";
import type { TranscriptActions } from "@/features/transcript/context";
import { latestSessionError } from "@/features/transcript/sessionError";
import { useOlderTurns } from "@/features/transcript/useOlderTurns";
import type { DocumentItem } from "@/features/transcript/turnItems";
import { Badge } from "@/ui/badge";
import { Glass } from "@/ui/Glass";
import { Icon } from "@/ui/Icon";
import { Notice } from "@/ui/Notice";
import { ComposerRegion } from "./ComposerRegion";
import { PaneHeading } from "@/features/layout/tabs/PaneContext";
import { useRememberPlace } from "./lastPlace";
import { useConfirmedMissing } from "./useConfirmedMissing";
import { SessionHeaderMenu } from "./SessionHeaderMenu";
import { SessionHeaderChipSlot } from "./slots";
import { useReadOnly } from "./WorkspaceFrame";

export function SessionScreen({ desktopId, sessionId }: { desktopId: string; sessionId: string }) {
  useRememberPlace();
  const navigate = useNavigate();
  const readOnly = useReadOnly();
  const catalogQuery = useCatalog();
  const catalog = catalogQuery.data;
  const session = catalog?.sessions.find((candidate) => candidate.id === sessionId);
  const conversation = useConversation(sessionId);
  const state = conversation.state;
  const older = useOlderTurns(sessionId, state?.turns ?? [], state?.older ?? false);
  const [document, setDocument] = useState<DocumentItem | null>(null);
  // F-41: the latest error takes the composer's place until it is tapped away (for this run of the screen).
  const [dismissedError, setDismissedError] = useState<string | null>(null);
  const error = state ? latestSessionError(state.turns) : undefined;
  const errorKey = error ? `${state?.turns.at(-1)?.id ?? ""}:${error.text}` : null;

  const { send, retry: retryTurn } = conversation;
  const projectId = session?.projectId;
  const mentions = useMentionPreview({ desktopId, conversationId: sessionId, projectId });
  // Retry and a tapped choice go through the send-guard like a typed message.
  const replies = useGuardedReplies({ desktopId, sessionId, target: session?.target ?? null, send, retry: retryTurn });
  const actions = useMemo<TranscriptActions>(
    () => ({
      onOpenFile: mentions.onOpenFile,
      onNavigate: (href) => void navigate({ to: href as never }),
      onOpenDocument: setDocument,
      onOpenSession: (otherProject, otherSession) => {
        void otherProject;
        void navigate({ to: "/d/$desktopId/s/$sessionId", params: { desktopId, sessionId: otherSession } });
      },
      isProjectAvailable: (id) => !catalog || catalog.projects.some((project) => project.id === id),
      ...(readOnly
        ? {}
        : {
            onChoose: replies.onChoose,
          }),
    }),
    [catalog, desktopId, mentions.onOpenFile, navigate, readOnly, replies.onChoose],
  );

  // An archived or just-created session may be absent from a stale catalog, so the notice also needs a conversation without turns.
  const missing = useConfirmedMissing(catalogQuery, catalog !== undefined, !session) && (state?.turns.length ?? 0) === 0;
  const title = session?.title ?? "Session";

  return (
    // "route-stub" and `data-route` are kept as the outlet marker that data/lastPlace.test.tsx and
    // test/routes.test.tsx (WS-L) look for; drop them when their owner switches to a real marker (as the home screen).
    <div
      data-testid="route-stub"
      data-route="/d/:desktopId/s/:sessionId"
      className={`${BACKGROUND_CONTAINER_CLASS} flex h-full min-h-0 flex-1 flex-col`}
    >
      {/* The project's background sits behind the whole chat, like on the New session page. */}
      <ChatBackground projectId={projectId} />
      {/* The header spans the chat pane; the transcript and composer keep their own 768 px column. */}
      <header className="flex w-full shrink-0 items-center gap-2 px-2 py-2 min-[24rem]:px-5">
        <Link
          to={projectId ? "/d/$desktopId/p/$projectId" : "/d/$desktopId"}
          params={projectId ? { desktopId, projectId } : { desktopId }}
          aria-label="Back"
          // From 1024 px the project's session list is beside the chat, so there is nothing to go back to.
          className={`inline-flex size-11 shrink-0 items-center justify-center rounded-pill bg-fill hover:bg-selection ${projectId ? "lg:hidden" : ""}`}
        >
          <Icon name="chevronDown" size={20} className="rotate-90" />
        </Link>
        <PaneHeading className="min-w-0 flex-1 truncate text-h font-semibold">{title}</PaneHeading>
        {/* The chip gives way (ellipsis) before the title does: the group never takes more than 55% of the header. */}
        <div className="flex max-w-[55%] min-w-0 shrink items-center gap-2">
          {readOnly ? (
            <Glass className="flex min-h-9 items-center rounded-pill px-3">
              <Badge variant="outline" className="border-0 px-0 text-meta" data-testid="read-only-chip">
                Read only
              </Badge>
            </Glass>
          ) : null}
          {session ? <SessionHeaderChipSlot desktopId={desktopId} session={session} /> : null}
          {session ? <SessionHeaderMenu desktopId={desktopId} session={session} readOnly={readOnly} /> : null}
        </div>
      </header>

      {missing ? (
        <div className="mx-auto w-full max-w-[768px] px-5">
          <Notice variant="error">
            This session is not available on this device. It may have been deleted, or this device has no access to it.
          </Notice>
        </div>
      ) : null}

      {state?.error && state.turns.length === 0 ? (
        <div className="mx-auto w-full max-w-[768px] px-5">
          <Notice variant="error">{/not[_ ]selected/i.test(state.error) ? "Not on this device's projects" : state.error}</Notice>
        </div>
      ) : null}

      <ModelNamesProvider>
        <Transcript
          turns={older.turns}
          optimistic={state?.optimistic ?? []}
          loading={!state}
          hasOlder={older.hasOlder}
          loadingOlder={older.loading}
          olderError={older.error}
          loadOlder={older.loadOlder}
          onRetry={readOnly ? undefined : replies.onRetry}
          busy={state?.busy ?? false}
          actions={actions}
        />
      </ModelNamesProvider>
      <DocumentDialog document={document} onClose={() => setDocument(null)} />
      {mentions.dialog}
      <ComposerRegion
        desktopId={desktopId}
        sessionId={sessionId}
        above={
          <>
            {state?.usageLimit ? <SessionUsageCard sessionId={sessionId} state={state.usageLimit} readOnly={readOnly} desktopId={desktopId} target={session?.target ?? null} /> : null}
            {replies.error ? (
              <p role="alert" className="px-4 pb-2 text-meta text-deletions" data-testid="send-refused">
                {replies.error}
              </p>
            ) : null}
            {state?.question ? <SessionQuestionCard sessionId={sessionId} prompt={state.question} readOnly={readOnly} /> : null}
          </>
        }
      >
        <SessionComposer
          desktopId={desktopId}
          sessionId={sessionId}
          projectId={projectId}
          session={session}
          busy={state?.busy ?? false}
          readOnly={readOnly}
          send={(text, attachments, options) => {
            replies.clearError();
            return send(text, attachments, options);
          }}
          notice={error && errorKey !== dismissedError ? { text: error.text, dismiss: () => setDismissedError(errorKey) } : null}
        />
      </ComposerRegion>
      {replies.dialog}
    </div>
  );
}
