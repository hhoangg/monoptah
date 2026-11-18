// F-69: the Mono chat at `/d/:desktopId/monos/:monoId`. The conversation id is `mono:<id>`. It mirrors the session
// screen (header, transcript, composer region) with the Mono's mascot and accent in the header, a compact
// composer (no model controls: a Mono's model is edited on its profile) and the Mono's empty state. Opening the
// chat marks the Mono seen (F-17). The Mono's own header cards, chips and documents render through the WS-T
// transcript components (F-43, F-72).
import { Link, useNavigate } from "@tanstack/react-router";
import { useCallback, useEffect, useMemo, useState } from "react";
import { useCatalog, useConversation } from "@/data";
import { assertFitsRequest, toWire } from "@/features/composer/attachments";
import { Composer } from "@/features/composer/Composer";
import { useLinkReady } from "@/features/composer/SessionComposer";
import { useSendGuard } from "@/features/composer/useSendGuard";
import { ComposerRegion } from "@/features/sessions/ComposerRegion";
import { useRememberPlace } from "@/features/sessions/lastPlace";
import { markMonoSeen, markMonoUnread } from "@/features/sessions/monoSeen";
import { useReadOnly } from "@/features/sessions/WorkspaceFrame";
import { DocumentDialog } from "@/features/transcript/DocumentDialog";
import { Transcript } from "@/features/transcript/Transcript";
import type { TranscriptActions } from "@/features/transcript/context";
import { useOlderTurns } from "@/features/transcript/useOlderTurns";
import type { DocumentItem } from "@/features/transcript/turnItems";
import { safeHexColor } from "@/lib/color";
import { useLinkMutation } from "@/data";
import { Badge } from "@/ui/badge";
import { Glass } from "@/ui/Glass";
import { Icon } from "@/ui/Icon";
import { Notice } from "@/ui/Notice";
import { useMonoAccentStyle } from "./monoAccent";
import { MessageActions, type HeldMessage } from "./MessageActions";
import { MonoMascot } from "./MonoMascot";
import { MonoMenu } from "./MonoMenu";

const EXCERPT = 140;
const excerpt = (text: string) => (text.length > EXCERPT ? `${text.slice(0, EXCERPT - 1)}...` : text);
/** The replied-to message as a Markdown quote, so the Mono sees what the reply answers. */
const quote = (text: string) => excerpt(text).split("\n").map((line) => `> ${line}`).join("\n");

export const monoConversationId = (monoId: string) => `mono:${monoId}`;

export function MonoScreen({ desktopId, monoId }: { desktopId: string; monoId: string }) {
  useRememberPlace();
  const navigate = useNavigate();
  const readOnly = useReadOnly();
  const ready = useLinkReady();
  const guard = useSendGuard();
  const catalog = useCatalog().data;
  const mono = catalog?.monos.find((candidate) => candidate.id === monoId);
  const monoTools = catalog?.monoTools === true;
  const conversationId = monoConversationId(monoId);
  const conversation = useConversation(conversationId);
  const state = conversation.state;
  const older = useOlderTurns(conversationId, state?.turns ?? [], state?.older ?? false);
  const cancel = useLinkMutation("session.cancel");
  const accentStyle = useMonoAccentStyle(desktopId, monoId);
  const [document, setDocument] = useState<DocumentItem | null>(null);
  // F-71: the message being replied to; the reply is sent with it quoted.
  const [replyTo, setReplyTo] = useState<HeldMessage | null>(null);

  // F-17: the chat on screen is seen, and so is a reply that arrives while it is open.
  useEffect(() => {
    if (mono) markMonoSeen(desktopId, mono);
  }, [desktopId, mono]);

  const { send } = conversation;
  const actions = useMemo<TranscriptActions>(
    () => ({
      onNavigate: (href) => void navigate({ to: href as never }),
      onOpenDocument: setDocument,
      onOpenSession: (_projectId, sessionId) => {
        void navigate({ to: "/d/$desktopId/s/$sessionId", params: { desktopId, sessionId } });
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
    [catalog, desktopId, navigate, readOnly, send],
  );

  const retry = useCallback((id: string) => void conversation.retry(id), [conversation]);

  const submit = async (text: string, attachments: Parameters<typeof toWire>[0]) => {
    assertFitsRequest(text, attachments);
    await guard.check({ desktopId, sessionId: conversationId, target: null });
    const wire = toWire(attachments);
    const quoted = replyTo ? `${quote(replyTo.text)}\n\n${text}` : text;
    setReplyTo(null);
    void send(quoted, wire.length > 0 ? wire : undefined);
  };

  const name = mono?.name ?? "Mono";
  const working = mono?.status === "working" || state?.busy === true;
  const empty = !!state && older.turns.length === 0 && (state.optimistic ?? []).length === 0;
  const monoColor = mono?.color;
  const agent = useMemo(() => ({ name, ...(monoColor ? { color: safeHexColor(monoColor) } : {}) }), [name, monoColor]);

  return (
    // "route-stub" and `data-route` are the outlet marker that test/routes.test.tsx (WS-L) looks for.
    <div
      data-testid="route-stub"
      data-route="/d/:desktopId/monos/:monoId"
      style={accentStyle}
      className="-mb-[calc(env(safe-area-inset-bottom)+2.5rem)] flex h-[calc(100dvh-2.75rem-env(safe-area-inset-top))] flex-col"
    >
      <header className="mx-auto flex w-full max-w-[768px] shrink-0 items-center gap-2 px-2 py-2 min-[24rem]:px-5">
        <Link
          to="/d/$desktopId"
          params={{ desktopId }}
          aria-label="Back"
          className="inline-flex size-11 shrink-0 items-center justify-center rounded-pill bg-fill hover:bg-selection"
        >
          <Icon name="chevronDown" size={20} className="rotate-90" />
        </Link>
        <MonoMascot monoId={monoId} mascot={mono?.mascot} color={mono?.color} working={working} size={32} />
        <div className="min-w-0 flex-1">
          <h1 className="truncate text-h font-semibold">{name}</h1>
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
            <MonoMenu desktopId={desktopId} monoId={monoId} />
          ) : (
            <Link
              to="/d/$desktopId/settings"
              params={{ desktopId }}
              className="inline-flex min-h-11 items-center gap-1.5 rounded-pill bg-fill px-3 text-small hover:bg-selection pointer-fine:min-h-9"
            >
              <Icon name="settings" size={16} />
              Settings
            </Link>
          )}
        </div>
      </header>

      {state?.error && state.turns.length === 0 ? (
        <div className="mx-auto w-full max-w-[768px] px-5">
          <Notice variant="error">{/not[_ ]selected/i.test(state.error) ? "Not on this device's projects" : state.error}</Notice>
        </div>
      ) : null}

      {empty ? (
        <div className="mx-auto flex min-h-0 w-full max-w-[768px] flex-1 flex-col items-center justify-center gap-3 px-5 text-center" data-testid="mono-empty">
          <MonoMascot monoId={monoId} mascot={mono?.mascot} color={mono?.color} working={working} size={96} />
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
          <Transcript
            turns={older.turns}
            optimistic={state?.optimistic ?? []}
            loading={!state}
            hasOlder={older.hasOlder}
            loadingOlder={older.loading}
            olderError={older.error}
            loadOlder={older.loadOlder}
            onRetry={readOnly ? undefined : retry}
            agent={agent}
            busy={state?.busy ?? false}
            actions={actions}
          />
        </MessageActions>
      )}
      <DocumentDialog document={document} onClose={() => setDocument(null)} />
      <ComposerRegion desktopId={desktopId} sessionId={conversationId}>
        <Composer
          onSubmit={submit}
          busy={state?.busy ?? false}
          onStop={async () => {
            await cancel.mutateAsync({ conversationId });
          }}
          readOnly={readOnly}
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
      </ComposerRegion>
    </div>
  );
}
