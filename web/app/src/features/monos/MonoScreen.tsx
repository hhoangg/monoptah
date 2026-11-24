// F-69: the Mono chat at `/d/:desktopId/monos/:monoId`. The conversation id is `mono:<id>`. It mirrors the session
// screen (header, transcript, composer region) with the Mono's mascot and accent in the header, a compact
// composer (no model controls: a Mono's model is edited on its profile) and the Mono's empty state. Opening the
// chat marks the Mono seen (F-17). The Mono's own header cards, chips and documents render through the WS-T
// transcript components (F-43, F-72).
import { BACKGROUND_CONTAINER_CLASS, ChatBackground, useFirstBackgroundProject } from "@/features/backgrounds";
import { ModelNamesProvider } from "@/features/transcript/modelNames";
import { Link, useNavigate } from "@tanstack/react-router";
import { useCallback, useEffect, useMemo, useState } from "react";
import { useMediaQuery } from "@/ui/useMediaQuery";
import { useCatalog, useConversation } from "@/data";
import { useMonoTools } from "@/features/sessions/workspaceFilter";
import { assertFitsRequest, toWire } from "@/features/composer/attachments";
import { useRequestControl } from "@/features/composer/useRequestControl";
import { readChatDraft, writeChatDraft } from "@/features/composer/chatDraft";
import { Composer } from "@/features/composer/Composer";
import { SessionQuestionCard, SessionUsageCard, useLinkReady } from "@/features/composer/SessionComposer";
import { useGuardedReplies, useSendGuard } from "@/features/composer/useSendGuard";
import { PaneHeading } from "@/features/layout/tabs/PaneContext";
import { ComposerRegion } from "@/features/sessions/ComposerRegion";
import { useMentionPreview } from "@/features/filePreview/useMentionPreview";
import { useRememberPlace } from "@/features/sessions/lastPlace";
import { useConfirmedMissing } from "@/features/sessions/useConfirmedMissing";
import { markMonoSeen, markMonoUnread } from "@/features/sessions/monoSeen";
import { WIDE_QUERY, useReadOnly } from "@/features/sessions/WorkspaceFrame";
import { Transcript } from "@/features/transcript/Transcript";
import type { TranscriptActions } from "@/features/transcript/context";
import { useOlderTurns } from "@/features/transcript/useOlderTurns";
import { safeHexColor } from "@/lib/color";
import { useLinkMutation } from "@/data";
import { Badge } from "@/ui/badge";
import { Glass } from "@/ui/Glass";
import { Icon } from "@/ui/Icon";
import { Notice } from "@/ui/Notice";
import { useMonoAccentStyle } from "./monoAccent";
import { MessageActions, type HeldMessage } from "./MessageActions";
import { MonoAvatar } from "./MonoAvatar";

const EXCERPT = 140;
const excerpt = (text: string) => (text.length > EXCERPT ? `${text.slice(0, EXCERPT - 1)}...` : text);
const QUOTE_MAX = 280;
/** The replied-to message as a one-line Markdown quote (as on mobile), so the Mono sees what the reply answers. */
const quote = (text: string) => {
  const line = text.replace(/\s+/g, " ").trim();
  return `> ${line.length > QUOTE_MAX ? `${line.slice(0, QUOTE_MAX - 1)}…` : line}`;
};

export const monoConversationId = (monoId: string) => `mono:${monoId}`;

export function MonoScreen({ desktopId, monoId }: { desktopId: string; monoId: string }) {
  useRememberPlace();
  const navigate = useNavigate();
  // From 640 px the sidebar lists the Monos, so the chat has no Back to the list there.
  const wide = useMediaQuery(WIDE_QUERY);
  const readOnly = useReadOnly();
  const ready = useLinkReady();
  const guard = useSendGuard();
  const catalogQuery = useCatalog();
  const catalog = catalogQuery.data;
  const mono = catalog?.monos.find((candidate) => candidate.id === monoId);
  const monoTools = useMonoTools();
  const conversationId = monoConversationId(monoId);
  const conversation = useConversation(conversationId);
  const state = conversation.state;
  const missing = useConfirmedMissing(catalogQuery, catalog !== undefined, !mono) && (state?.turns.length ?? 0) === 0;
  const [initialText] = useState(() => readChatDraft(desktopId, conversationId));
  const rememberText = useCallback((text: string) => writeChatDraft(desktopId, conversationId, text), [desktopId, conversationId]);
  const older = useOlderTurns(conversationId, state?.turns ?? [], state?.older ?? false);
  const cancel = useLinkMutation("session.cancel");
  const accentStyle = useMonoAccentStyle(desktopId, monoId);
  const monoBackgroundProject = useFirstBackgroundProject(mono?.projectIds);
  const requestControl = useRequestControl(desktopId);
  // F-71: the message being replied to; the reply is sent with it quoted.
  const [replyTo, setReplyTo] = useState<HeldMessage | null>(null);

  // F-17: the chat on screen is seen, and so is a reply that arrives while it is open.
  useEffect(() => {
    if (mono) markMonoSeen(desktopId, mono);
  }, [desktopId, mono]);

  const { send, retry: retryTurn } = conversation;
  // Retry and a tapped choice go through the send-guard like a typed message.
  const replies = useGuardedReplies({ desktopId, sessionId: conversationId, target: null, send, retry: retryTurn });
  // A Mono chat has no project: every file mention opens the preview.
  const mentions = useMentionPreview({ desktopId, conversationId });
  const actions = useMemo<TranscriptActions>(
    () => ({
      onOpenFile: mentions.onOpenFile,
      onNavigate: (href) => void navigate({ to: href as never }),
      // A document card opens the profile's document page: it reads the document's own body and visual tree.
      onOpenDocument: (item) => {
        void navigate({ to: "/d/$desktopId/monos/$monoId/profile/file/$name", params: { desktopId, monoId, name: item.documentId } });
      },
      onOpenSession: (_projectId, sessionId) => {
        void navigate({ to: "/d/$desktopId/s/$sessionId", params: { desktopId, sessionId } });
      },
      isProjectAvailable: (id) => !catalog || catalog.projects.some((project) => project.id === id),
      ...(readOnly
        ? {}
        : {
            onChoose: replies.onChoose,
          }),
    }),
    [catalog, desktopId, mentions.onOpenFile, monoId, navigate, readOnly, replies.onChoose],
  );


  const submit = async (text: string, attachments: Parameters<typeof toWire>[0]) => {
    assertFitsRequest(text, attachments);
    await guard.check({ desktopId, sessionId: conversationId, target: null });
    const wire = toWire(attachments);
    const quoted = replyTo ? `${quote(replyTo.text)}\n\n${text}` : text;
    setReplyTo(null);
    replies.clearError();
    void send(quoted, wire.length > 0 ? wire : undefined);
    // Same as the session composer: a composer that unmounted meanwhile cannot clear the draft itself.
    if (readChatDraft(desktopId, conversationId).trim() === text.trim()) writeChatDraft(desktopId, conversationId, "");
  };

  const name = mono?.name ?? "Mono";
  const working = mono?.status === "working" || state?.busy === true;
  const empty = !!state && older.turns.length === 0 && (state.optimistic ?? []).length === 0;
  const monoColor = mono?.color;
  const monoMascot = mono?.mascot;
  const agent = useMemo(
    () => ({ id: monoId, name, ...(monoMascot ? { mascot: monoMascot } : {}), ...(monoColor ? { color: safeHexColor(monoColor) } : {}) }),
    [monoId, name, monoColor, monoMascot],
  );

  return (
    // "route-stub" and `data-route` are the outlet marker that test/routes.test.tsx (WS-L) looks for.
    <div
      data-testid="route-stub"
      data-route="/d/:desktopId/monos/:monoId"
      style={accentStyle}
      className={`${BACKGROUND_CONTAINER_CLASS} flex h-full min-h-0 flex-1 flex-col`}
    >
      {/* A Mono works across projects (`projectIds`) and has no background of its own: its chat shows the background of the first of its projects that has one. */}
      <ChatBackground projectId={monoBackgroundProject} />
      <header className="flex w-full shrink-0 items-center gap-2 px-2 py-2 min-[24rem]:px-5">
        {wide ? null : (
          <Link
            to="/d/$desktopId"
            params={{ desktopId }}
            aria-label="Back"
            className="inline-flex size-11 shrink-0 items-center justify-center rounded-pill bg-fill hover:bg-selection"
          >
            <Icon name="chevronDown" size={20} className="rotate-90" />
          </Link>
        )}
        <MonoAvatar monoId={monoId} name={name} mascot={mono?.mascot} color={mono?.color} working={working} size={32} />
        <div className="min-w-0 flex-1">
          <PaneHeading className="truncate text-h font-semibold">{name}</PaneHeading>
          {working ? <p className="truncate text-meta text-muted-text">Working</p> : null}
        </div>
        <div className="flex shrink-0 items-center gap-2">
          {readOnly ? (
            <Glass className="flex min-h-9 items-center rounded-pill px-3">
              <Badge variant="outline" className="border-0 px-0 text-meta" data-testid="read-only-chip">
                Read only
              </Badge>
            </Glass>
          ) : null}
          {monoTools ? (
            <Link
              to="/d/$desktopId/monos/$monoId/profile"
              params={{ desktopId, monoId }}
              aria-label="Mono profile"
              className="inline-flex size-11 shrink-0 items-center justify-center rounded-pill bg-fill hover:bg-selection pointer-fine:size-9"
            >
              <Icon name="info" size={18} />
            </Link>
          ) : (
            <Link
              to="/settings/desktops/$id"
              params={{ id: desktopId }}
              className="inline-flex min-h-11 items-center gap-1.5 rounded-pill bg-fill px-3 text-small hover:bg-selection pointer-fine:min-h-9"
            >
              <Icon name="settings" size={16} />
              Settings
            </Link>
          )}
        </div>
      </header>

      {missing ? (
        <div className="mx-auto w-full max-w-[768px] px-5">
          <Notice variant="error">
            This Mono is not available on this device. It may have been deleted, or this device has no access to it.
          </Notice>
        </div>
      ) : null}

      {state?.error && state.turns.length === 0 ? (
        <div className="mx-auto w-full max-w-[768px] px-5">
          <Notice variant="error">{/not[_ ]selected/i.test(state.error) ? "Not on this device's projects" : state.error}</Notice>
        </div>
      ) : null}

      {empty ? (
        <div className="mx-auto flex min-h-0 w-full max-w-[768px] flex-1 flex-col items-center justify-center gap-3 px-5 text-center" data-testid="mono-empty">
          <MonoAvatar monoId={monoId} name={name} mascot={mono?.mascot} color={mono?.color} working={working} size={96} />
          <p className="text-h font-semibold">{name}</p>
          <p className="max-w-sm text-body text-muted-text">{readOnly ? "No messages yet." : `Say hello. ${name} works in the background and replies here.`}</p>
        </div>
      ) : (
        <MessageActions
          desktopId={desktopId}
          monoId={monoId}
          readOnly={readOnly}
          className="relative flex min-h-0 flex-1 flex-col"
          onReply={setReplyTo}
          onMarkUnread={() => {
            if (!mono) return;
            markMonoUnread(desktopId, mono);
            void navigate({ to: "/d/$desktopId", params: { desktopId } });
          }}
        >
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
              agent={agent}
              busy={state?.busy ?? false}
              actions={actions}
            />
          </ModelNamesProvider>
        </MessageActions>
      )}
      <ComposerRegion
        desktopId={desktopId}
        sessionId={conversationId}
        above={
          <>
            {state?.usageLimit ? <SessionUsageCard sessionId={conversationId} state={state.usageLimit} readOnly={readOnly} /> : null}
            {replies.error ? (
              <p role="alert" className="px-4 pb-2 text-meta text-deletions" data-testid="send-refused">
                {replies.error}
              </p>
            ) : null}
            {state?.question ? <SessionQuestionCard sessionId={conversationId} prompt={state.question} readOnly={readOnly} /> : null}
          </>
        }
      >
        <Composer
          initialText={initialText}
          onTextChange={rememberText}
          onSubmit={submit}
          busy={state?.busy ?? false}
          onStop={async () => {
            await cancel.mutateAsync({ conversationId });
          }}
          readOnly={readOnly}
          requestControl={requestControl}
          offline={!ready}
          topRow={
            replyTo ? (
              <>
                <span className="min-w-0 flex-1 truncate text-small text-muted-text" data-testid="reply-bar">
                  Replying to: {excerpt(replyTo.text)}
                </span>
                <button type="button" aria-label="Cancel reply" className="inline-flex size-8 items-center justify-center rounded-pill hover:bg-selection" onClick={() => setReplyTo(null)}>
                  <Icon name="close" size={14} />
                </button>
              </>
            ) : null
          }
        />
        {guard.dialog}
        {replies.dialog}
      </ComposerRegion>
      {mentions.dialog}
    </div>
  );
}
