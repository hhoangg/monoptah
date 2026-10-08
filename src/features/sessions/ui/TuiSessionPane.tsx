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
  isNoConversationOutput,
  canRetitleFromTui,
  canStartFreshAfter,
  cleanTuiTitle,
  tuiBinaryIdentity,
  tuiExitNotice,
  tuiLaunchCwd,
  tuiProviderAccount,
  tuiPtyId,
  type TuiExit,
  type TuiLaunchEvent,
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

type Prepared = {
  attempt: number;
  launch: PtyLaunch;
  /** The launch used an id saved earlier, by resuming or by claiming it. */
  reusedId: boolean;
  /** This launch is the automatic retry that claims a never-used saved id. */
  claimed: boolean;
};

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
  /** Reports each launch outcome: started, or settled on an exit. */
  onLaunchEvent?: (sessionId: string, event: TuiLaunchEvent) => void;
};

/** A provider's own interactive CLI in a PTY, in place of the chat transcript. */
export function TuiSessionPane({
  session,
  visible,
  focused,
  onFocus,
  onSessionChange,
  onTitleChange,
  onLaunchEvent,
}: Props) {
  const sessionRef = useRef(session);
  sessionRef.current = session;
  const onSessionChangeRef = useRef(onSessionChange);
  onSessionChangeRef.current = onSessionChange;
  const onTitleChangeRef = useRef(onTitleChange);
  onTitleChangeRef.current = onTitleChange;
  const onLaunchEventRef = useRef(onLaunchEvent);
  onLaunchEventRef.current = onLaunchEvent;

  const [attempt, setAttempt] = useState(0);
  const [prepared, setPrepared] = useState<Prepared | null>(null);
  const [exit, setExit] = useState<TuiExit | null>(null);
  const startedAt = useRef(0);
  const preparedRef = useRef(prepared);
  preparedRef.current = prepared;
  // Set to relaunch once under `--session-id` after a resume found nothing.
  const claimId = useRef<string | undefined>(undefined);
  const claimedAlready = useRef<string | undefined>(undefined);
  const label = HARNESS_TITLE[session.harness];

  const fail = useCallback((error: unknown) => {
    const exit: TuiExit = {
      code: null,
      error: error instanceof Error ? error.message : String(error),
      early: true,
    };
    setExit(exit);
    onLaunchEventRef.current?.(sessionRef.current.id, { kind: "exit", exit });
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
      const claim = claimId.current;
      claimId.current = undefined;
      const built = buildTuiLaunch(
        claim ? { ...current, providerSessionId: undefined } : current,
        claim
          ? { binaryPath, newProviderSessionId: claim }
          : { binaryPath, initialPrompt: current.initialPrompt },
      );
      const pinAccount = accountId && accountId !== current.providerAccountId;
      // Saved before anything can start the CLI. A pane that remounted or an
      // app that died after spawning would otherwise orphan the conversation.
      const newId =
        built.providerSessionId !== current.providerSessionId
          ? built.providerSessionId
          : undefined;
      if (newId || pinAccount) {
        await onSessionChangeRef.current(current.id, {
          providerSessionId: newId,
          providerAccountId: pinAccount ? accountId : undefined,
        });
      }
      if (cancelled) return;
      const providerAccount = tuiProviderAccount(current.harness, accountId);
      startedAt.current = Date.now();
      setPrepared({
        attempt,
        reusedId: !!current.providerSessionId && !newId,
        claimed: !!claim,
        launch: {
          program: built.program,
          args: built.args,
          ...tuiBinaryIdentity(current.harness),
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
    (result: { code: number | null; error?: string; output?: string }) => {
      const early = Date.now() - startedAt.current < TUI_EARLY_EXIT_MS;
      const launched = preparedRef.current;
      const id = sessionRef.current.providerSessionId;
      // Claude only knows a conversation once it has one, so an untouched tab
      // has a saved id that `--resume` rejects. The id cannot be "in use" if
      // Claude never wrote it, so claim it once before showing an error.
      if (
        !result.error &&
        early &&
        id &&
        launched?.reusedId &&
        !launched.claimed &&
        claimedAlready.current !== id &&
        isNoConversationOutput(result.output)
      ) {
        claimedAlready.current = id;
        claimId.current = id;
        setAttempt((count) => count + 1);
        return;
      }
      const exit: TuiExit = { code: result.code, error: result.error, early };
      setExit(exit);
      onLaunchEventRef.current?.(sessionRef.current.id, { kind: "exit", exit });
    },
    [],
  );

  const onSpawn = useCallback(() => {
    onLaunchEventRef.current?.(sessionRef.current.id, { kind: "started" });
  }, []);

  const onTitle = useCallback((raw: string) => {
    const title = cleanTuiTitle(raw);
    if (!title) return;
    const current = sessionRef.current;
    if (title === current.title) return;
    if (!canRetitleFromTui(current, autoTitles.get(current.id))) return;
    autoTitles.set(current.id, title);
    onTitleChangeRef.current(current.id, title);
  }, []);

  const restart = () => {
    // A manual restart starts a new chain, which may claim the id again.
    claimedAlready.current = undefined;
    setAttempt((count) => count + 1);
  };
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
        {session.worktreeCwd ? null : (
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
        )}
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
            onSpawn={onSpawn}
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
          canStartFreshAfter(session.harness, exit, !!prepared?.reusedId) ? (
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
