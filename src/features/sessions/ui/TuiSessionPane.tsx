import { useCallback, useEffect, useRef, useState } from "react";
import { buildTuiLaunch } from "../../../integrations/harness/core/tuiLaunch";
import { resolveTuiBinary } from "../../../integrations/harness/core/tuiBinary";
import type { PtyLaunch } from "../../../platform/tauri/pty";
import { prettyCwd } from "../../../shared/lib/paths";
import { FolderTree } from "../../../shared/ui/icons";
import { lazySurface } from "../../../shared/ui/lazySurface";
import {
  providerAccountExists,
  selectedProviderAccountId,
  supportsProviderAccounts,
} from "../../providers/model/providerAccounts";
import { HARNESS_TITLE, type Session } from "../model/session";
import {
  TUI_EARLY_EXIT_MS,
  canRetitleFromTui,
  canStartFreshAfter,
  cleanTuiTitle,
  tuiExitNotice,
  tuiLaunchCwd,
  tuiProviderAccount,
  tuiPtyId,
  type TuiExit,
  type TuiSessionPatch,
} from "../model/tuiSession";
import { HarnessIcon } from "./HarnessIcon";

const TerminalView = lazySurface(async () => {
  const module = await import("../../terminal/ui/TerminalView");
  return { default: module.TerminalView };
});

/**
 * Last title this pane wrote per session. It outlives a remount, so a moved
 * pane still knows the title is the CLI's own and may keep following it.
 */
const autoTitles = new Map<string, string>();

type Prepared = { attempt: number; launch: PtyLaunch; resumed: boolean };

type Props = {
  session: Session;
  visible: boolean;
  focused: boolean;
  onFocus: (sessionId: string) => void;
  /**
   * Save what the launch decided (conversation id, account) before the CLI
   * starts, so a restart or a crash cannot lose the conversation.
   */
  onSessionChange: (sessionId: string, patch: TuiSessionPatch) => Promise<void>;
  onTitleChange: (sessionId: string, title: string) => void;
};

/** A provider's own interactive CLI in a PTY, in place of the chat transcript. */
export function TuiSessionPane({
  session,
  visible,
  focused,
  onFocus,
  onSessionChange,
  onTitleChange,
}: Props) {
  const sessionRef = useRef(session);
  sessionRef.current = session;
  const onSessionChangeRef = useRef(onSessionChange);
  onSessionChangeRef.current = onSessionChange;
  const onTitleChangeRef = useRef(onTitleChange);
  onTitleChangeRef.current = onTitleChange;

  const [attempt, setAttempt] = useState(0);
  const [prepared, setPrepared] = useState<Prepared | null>(null);
  const [exit, setExit] = useState<TuiExit | null>(null);
  const startedAt = useRef(0);
  const label = HARNESS_TITLE[session.harness];

  const fail = useCallback((error: unknown) => {
    setExit({
      code: null,
      error: error instanceof Error ? error.message : String(error),
      early: true,
    });
  }, []);

  useEffect(() => {
    let cancelled = false;
    setPrepared(null);
    setExit(null);
    void (async () => {
      const current = sessionRef.current;
      const provider = supportsProviderAccounts(current.harness)
        ? current.harness
        : undefined;
      // A conversation belongs to the account that started it, so pin the
      // account the same way the first chat send does.
      const accountId = provider
        ? (current.providerAccountId ??
          selectedProviderAccountId(provider, current.cwd))
        : undefined;
      if (
        provider &&
        accountId &&
        !providerAccountExists(provider, accountId)
      ) {
        throw new Error(
          "This session uses a removed provider account. Open a new session with another account.",
        );
      }
      const binaryPath = await resolveTuiBinary(current.harness);
      if (cancelled) return;
      const built = buildTuiLaunch(current, {
        binaryPath,
        initialPrompt: current.initialPrompt,
      });
      const pinAccount = accountId && accountId !== current.providerAccountId;
      // Saved before anything can start the CLI. A pane that remounted or an
      // app that died after spawning would otherwise orphan the conversation.
      if (built.providerSessionId || pinAccount) {
        await onSessionChangeRef.current(current.id, {
          providerSessionId: built.providerSessionId,
          providerAccountId: pinAccount ? accountId : undefined,
        });
      }
      if (cancelled) return;
      const providerAccount = tuiProviderAccount(current.harness, accountId);
      startedAt.current = Date.now();
      setPrepared({
        attempt,
        // No id was minted, so the CLI reopens the one already saved.
        resumed: !built.providerSessionId && !!current.providerSessionId,
        launch: {
          program: built.program,
          args: built.args,
          ...(providerAccount ? { providerAccount } : {}),
        },
      });
    })().catch((error: unknown) => {
      if (!cancelled) fail(error);
    });
    return () => {
      cancelled = true;
    };
  }, [session.id, attempt, fail]);

  const onExit = useCallback(
    (result: { code: number | null; error?: string }) => {
      setExit({
        ...result,
        early: Date.now() - startedAt.current < TUI_EARLY_EXIT_MS,
      });
    },
    [],
  );

  const onTitle = useCallback((raw: string) => {
    const title = cleanTuiTitle(raw);
    if (!title) return;
    const current = sessionRef.current;
    if (title === current.title) return;
    if (!canRetitleFromTui(current, autoTitles.get(current.id))) return;
    autoTitles.set(current.id, title);
    onTitleChangeRef.current(current.id, title);
  }, []);

  const restart = () => setAttempt((count) => count + 1);
  const startFresh = () => {
    // The app applies the patch before this settles, so the restart reads a
    // session with no saved id.
    onSessionChangeRef
      .current(session.id, { providerSessionId: null })
      .then(restart, fail);
  };

  const notice = exit ? tuiExitNotice(session.harness, exit) : null;

  return (
    <div
      data-tui-pane={session.id}
      className="flex h-full min-h-0 min-w-0 flex-1 flex-col"
      onMouseDown={() => onFocus(session.id)}
    >
      <div className="flex h-8 shrink-0 items-center gap-2 border-b border-stroke px-3 text-[12px] text-content/55">
        <HarnessIcon harness={session.harness} className="size-3.5 shrink-0" />
        <span className="shrink-0">{label} terminal</span>
        <span className="min-w-0 flex-1 truncate font-mono text-[11px] text-content/40">
          {prettyCwd(tuiLaunchCwd(session))}
        </span>
        <button
          type="button"
          disabled
          title="Not available in terminal sessions. A terminal session runs in the current checkout. Use a chat session to start in a new worktree."
          aria-label="New worktree is not available in terminal sessions"
          className="flex h-6 shrink-0 items-center gap-1.5 rounded-md px-1.5 text-content/55 disabled:opacity-40"
        >
          <FolderTree className="size-3.5 shrink-0" />
          <span>New worktree unavailable</span>
        </button>
      </div>
      <div className="relative min-h-0 min-w-0 flex-1">
        {prepared ? (
          <TerminalView
            key={prepared.attempt}
            id={tuiPtyId(session.id)}
            cwd={tuiLaunchCwd(session)}
            active={visible && focused && !exit}
            launch={prepared.launch}
            onExit={onExit}
            onTitleChange={onTitle}
          />
        ) : exit ? null : (
          <p className="p-3 text-[12px] text-content/45">Starting {label}...</p>
        )}
      </div>
      {notice ? (
        <div
          role="status"
          data-tui-exit
          className="flex shrink-0 items-start gap-3 border-t border-stroke px-3 py-2 text-[12px] text-content/80"
        >
          <div className="min-w-0 flex-1">
            <p className="font-medium">{notice.message}</p>
            {notice.hint ? (
              <p className="mt-0.5 break-words text-content/55">
                {notice.hint}
              </p>
            ) : null}
          </div>
          {exit &&
          canStartFreshAfter(session.harness, exit, !!prepared?.resumed) ? (
            <button
              type="button"
              onClick={startFresh}
              className="h-6 shrink-0 rounded-md px-2 text-content/65 hover:bg-content/8 hover:text-content"
            >
              Start new conversation
            </button>
          ) : null}
          <button
            type="button"
            onClick={restart}
            className="h-6 shrink-0 rounded-md bg-content/8 px-2 text-content hover:bg-content/12"
          >
            Restart
          </button>
        </div>
      ) : null}
    </div>
  );
}
