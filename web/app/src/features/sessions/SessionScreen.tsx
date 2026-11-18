// `/d/:desktopId/s/:sessionId` (W-07): the header (back, title, the `session-header-chip` slot host), the
// transcript of the conversation and the composer region (Task 5 mounts the composer there).
import { Link, useNavigate } from "@tanstack/react-router";
import { useCallback, useMemo, useState } from "react";
import { useCatalog, useConversation } from "@/data";
import { SessionComposer, SessionQuestionCard, SessionUsageCard } from "@/features/composer/SessionComposer";
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
import { useRememberPlace } from "./lastPlace";
import { SessionHeaderMenu } from "./SessionHeaderMenu";
import { SessionHeaderChipSlot } from "./slots";
import { useReadOnly } from "./WorkspaceFrame";

export function SessionScreen({ desktopId, sessionId }: { desktopId: string; sessionId: string }) {
  useRememberPlace();
  const navigate = useNavigate();
  const readOnly = useReadOnly();
  const catalog = useCatalog().data;
  const session = catalog?.sessions.find((candidate) => candidate.id === sessionId);
  const conversation = useConversation(sessionId);
  const state = conversation.state;
  const older = useOlderTurns(sessionId, state?.turns ?? [], state?.older ?? false);
  const [document, setDocument] = useState<DocumentItem | null>(null);
  // F-41: the latest error takes the composer's place until it is tapped away (for this run of the screen).
  const [dismissedError, setDismissedError] = useState<string | null>(null);
  const error = state ? latestSessionError(state.turns) : undefined;
  const errorKey = error ? `${state?.turns.at(-1)?.id ?? ""}:${error.text}` : null;

  const { send } = conversation;
  const projectId = session?.projectId;
  const actions = useMemo<TranscriptActions>(
    () => ({
      onOpenFile: (path) => {
        if (!projectId) return;
        void navigate({ to: "/d/$desktopId/p/$projectId/files/$", params: { desktopId, projectId, _splat: path } });
      },
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
            onChoose: async (text: string) => {
              const result = await send(text);
              if (!result.sent) throw new Error(result.error ?? "not_sent");
            },
          }),
    }),
    [catalog, desktopId, navigate, projectId, readOnly, send],
  );

  const retry = useCallback((id: string) => void conversation.retry(id), [conversation]);
  const title = session?.title ?? "Session";

  return (
    // "route-stub" and `data-route` are kept as the outlet marker that data/lastPlace.test.tsx and
    // test/routes.test.tsx (WS-L) look for; drop them when their owner switches to a real marker (as the home screen).
    <div
      data-testid="route-stub"
      data-route="/d/:desktopId/s/:sessionId"
      className="-mb-[calc(env(safe-area-inset-bottom)+2.5rem)] flex h-[calc(100dvh-2.75rem-env(safe-area-inset-top))] flex-col"
    >
      <header className="mx-auto flex w-full max-w-[768px] shrink-0 items-center gap-2 px-2 py-2 min-[24rem]:px-5">
        <Link
          to={projectId ? "/d/$desktopId/p/$projectId" : "/d/$desktopId"}
          params={projectId ? { desktopId, projectId } : { desktopId }}
          aria-label="Back"
          className="inline-flex size-11 shrink-0 items-center justify-center rounded-pill bg-fill hover:bg-selection"
        >
          <Icon name="chevronDown" size={20} className="rotate-90" />
        </Link>
        <h1 className="min-w-0 flex-1 truncate text-h font-semibold">{title}</h1>
        <div className="flex shrink-0 items-center gap-2">
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

      {state?.error && state.turns.length === 0 ? (
        <div className="mx-auto w-full max-w-[768px] px-5">
          <Notice variant="error">{/not[_ ]selected/i.test(state.error) ? "Not on this device's projects" : state.error}</Notice>
        </div>
      ) : null}

      <Transcript
        turns={older.turns}
        optimistic={state?.optimistic ?? []}
        loading={!state}
        hasOlder={older.hasOlder}
        loadingOlder={older.loading}
        olderError={older.error}
        loadOlder={older.loadOlder}
        onRetry={readOnly ? undefined : retry}
        busy={state?.busy ?? false}
        actions={actions}
      />
      <DocumentDialog document={document} onClose={() => setDocument(null)} />
      <ComposerRegion
        desktopId={desktopId}
        sessionId={sessionId}
        above={
          <>
            {state?.usageLimit ? <SessionUsageCard sessionId={sessionId} state={state.usageLimit} readOnly={readOnly} /> : null}
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
          send={send}
          notice={error && errorKey !== dismissedError ? { text: error.text, dismiss: () => setDismissedError(errorKey) } : null}
        />
      </ComposerRegion>
    </div>
  );
}
