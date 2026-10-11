import { invoke } from "@tauri-apps/api/core";
import {
  useCallback,
  useEffect,
  useRef,
  useState,
  type PointerEvent as ReactPointerEvent,
} from "react";
import { buildTuiLaunch } from "../../../integrations/harness/core/tuiLaunch";
import { resolveTuiBinary } from "../../../integrations/harness/core/tuiBinary";
import { type PtyLaunch } from "../../../platform/tauri/pty";
import { claimPtyArrival } from "../../terminal/model/ptyHandoff";
import { MOD } from "../../../platform/tauri/platform";
import { prettyCwd } from "../../../shared/lib/paths";
import { FolderTree, GripVertical, X } from "../../../shared/ui/icons";
import { lazySurface } from "../../../shared/ui/lazySurface";
import {
  providerAccountExists,
  selectedProviderAccountId,
  supportsProviderAccounts,
} from "../../providers/model/providerAccounts";
import { IS_WIN } from "../../../platform/tauri/platform";
import { loadClaudeHooks } from "../../settings/model/settings";
import { HARNESS_TITLE, type Session } from "../model/session";
import {
  TUI_EARLY_EXIT_MS,
  isNoConversationOutput,
  canRetitleFromTui,
  canStartFreshAfter,
  cleanTuiTitle,
  tuiBinaryIdentity,
  tuiExitNotice,
  tuiHookNotify,
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

/**
 * How each session's CLI was last launched. A pane that moves to another tab
 * attaches to the running process instead of launching, so the early-exit
 * window must keep counting from the real launch, and the resume affordances
 * must keep reflecting how it started, not what the remount can see.
 */
const launchedAt = new Map<
  string,
  { at: number; reusedId: boolean; claimed: boolean }
>();

/**
 * The exit each session's pane last settled on, until it is launched again. A
 * pane moved after its process ended has nothing to attach to; it shows this
 * instead, so the real code, notice and restart options survive the move.
 */
const settledExits = new Map<string, TuiExit>();

type Prepared = {
  attempt: number;
  /**
   * Attach to the process the pane's previous view left running; no launch is
   * built and nothing is spawned. Set only for a pane that was just moved.
   */
  attach?: boolean;
  launch?: PtyLaunch;
  /** The process had already exited; nothing to mount, only the exit to show. */
  settled?: boolean;
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
  inSplit?: boolean;
  onPaneDragStart?: (event: ReactPointerEvent<HTMLElement>) => void;
  onClose?: (sessionId: string) => void;
  /**
   * The process ran for a while and exited cleanly, so the pane has nothing
   * left to show. The session itself is kept.
   */
  onExitClose?: (sessionId: string) => void;
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
  inSplit = false,
  onPaneDragStart,
  onClose,
  onExitClose,
}: Props) {
  const sessionRef = useRef(session);
  sessionRef.current = session;
  const onSessionChangeRef = useRef(onSessionChange);
  onSessionChangeRef.current = onSessionChange;
  const onTitleChangeRef = useRef(onTitleChange);
  onTitleChangeRef.current = onTitleChange;
  const onLaunchEventRef = useRef(onLaunchEvent);
  onLaunchEventRef.current = onLaunchEvent;
  const onExitCloseRef = useRef(onExitClose);
  onExitCloseRef.current = onExitClose;

  const [attempt, setAttempt] = useState(0);
  const [prepared, setPrepared] = useState<Prepared | null>(null);
  const [exit, setExit] = useState<TuiExit | null>(null);
  const startedAt = useRef(0);
  // Whether this pane was just moved. Claimed once per session id and kept
  // across a StrictMode replay of the effect, which would otherwise find the
  // record already consumed and launch over the running process.
  const arrival = useRef<{ id: string; attach: boolean } | null>(null);
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
      if (arrival.current?.id !== current.id) {
        arrival.current = {
          id: current.id,
          attach: claimPtyArrival(tuiPtyId(current.id)),
        };
      }
      // A pane moved from another tab finds its CLI still running. Attaching
      // sends it nothing, so no launch is built and no saved id is touched.
      // Only the first attempt attaches: a restart means the process is gone.
      if (arrival.current.attach && attempt === 0) {
        const launched = launchedAt.get(current.id);
        const settled = settledExits.get(current.id);
        if (settled) {
          // The process ended before the move, so there is nothing to attach
          // to: a view asked to would report a lost process in place of how
          // it really ended.
          setPrepared({
            attempt,
            settled: true,
            reusedId: launched?.reusedId ?? !!current.providerSessionId,
            claimed: launched?.claimed ?? false,
          });
          setExit(settled);
          return;
        }
        // Without a record, count from now: a quick exit then keeps the pane
        // and its notice instead of closing it unseen.
        startedAt.current = launched?.at ?? Date.now();
        setPrepared({
          attempt,
          attach: true,
          reusedId: launched?.reusedId ?? !!current.providerSessionId,
          claimed: launched?.claimed ?? false,
        });
        return;
      }
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
      // Busy/idle comes from Claude Code hooks calling back into this app. A
      // failed lookup just leaves the session without it.
      const [cliPath, spoolDir] =
        current.harness === "claude" && loadClaudeHooks() && !IS_WIN
          ? await Promise.all([
              invoke<string>("app_cli_path").catch(() => undefined),
              invoke<string>("tui_hook_spool_dir").catch(() => undefined),
            ])
          : [undefined, undefined];
      if (cancelled) return;
      const hookNotify = tuiHookNotify(loadClaudeHooks(), cliPath, spoolDir);
      const claim = claimId.current;
      claimId.current = undefined;
      const built = buildTuiLaunch(
        claim ? { ...current, providerSessionId: undefined } : current,
        claim
          ? {
              binaryPath,
              newProviderSessionId: claim,
              hookNotify,
              isWindows: IS_WIN,
            }
          : {
              binaryPath,
              initialPrompt: current.initialPrompt,
              hookNotify,
              isWindows: IS_WIN,
            },
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
      settledExits.delete(current.id);
      const reusedId = !!current.providerSessionId && !newId;
      launchedAt.set(current.id, {
        at: startedAt.current,
        reusedId,
        claimed: !!claim,
      });
      setPrepared({
        attempt,
        reusedId,
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
      const noConversation = isNoConversationOutput(result.output);
      const exit: TuiExit = {
        code: result.code,
        error: result.error,
        early,
        ...(noConversation ? { noConversation } : {}),
      };
      setExit(exit);
      settledExits.set(sessionRef.current.id, exit);
      onLaunchEventRef.current?.(sessionRef.current.id, { kind: "exit", exit });
      // A failed or early exit keeps the pane so its notice can be read, and so
      // does a late "no conversation" (print mode exits 0 for it). The close
      // lands after the launch event so an automation run settles first.
      if (result.code === 0 && !result.error && !early && !noConversation) {
        onExitCloseRef.current?.(sessionRef.current.id);
      }
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
      <div
        className={`flex h-8 shrink-0 items-center gap-2 border-b border-stroke px-3 text-[12px] text-content/55 ${
          inSplit && onPaneDragStart
            ? "cursor-grab touch-none select-none active:cursor-grabbing"
            : ""
        }`}
        onPointerDown={(event) => {
          if (event.button !== 0 || !inSplit || !onPaneDragStart) return;
          if ((event.target as HTMLElement | null)?.closest("[data-no-drag]")) {
            return;
          }
          onPaneDragStart(event);
        }}
      >
        {inSplit && onPaneDragStart ? (
          <GripVertical
            className="size-3.5 shrink-0 text-content/35"
            strokeWidth={1.75}
          />
        ) : null}
        <HarnessIcon harness={session.harness} className="size-3.5 shrink-0" />
        <span className="shrink-0">{label} terminal</span>
        <span className="min-w-0 flex-1 truncate font-mono text-[11px] text-content/40">
          {prettyCwd(tuiLaunchCwd(session))}
        </span>
        {session.worktreeCwd ? null : (
          <button
            type="button"
            disabled
            data-no-drag
            title="Not available in terminal sessions. A terminal session runs in the current checkout. Use a chat session to start in a new worktree."
            aria-label="New worktree is not available in terminal sessions"
            className="flex h-6 shrink-0 items-center gap-1.5 rounded-md px-1.5 text-content/55 disabled:opacity-40"
          >
            <FolderTree className="size-3.5 shrink-0" />
            <span>New worktree unavailable</span>
          </button>
        )}
        {inSplit && onClose ? (
          <button
            type="button"
            title={`Close Pane (${MOD}W)`}
            aria-label="Close pane"
            data-no-drag
            className="grid size-5 shrink-0 place-items-center rounded text-content/50 hover:bg-content/10 hover:text-content"
            onPointerDown={(e) => e.stopPropagation()}
            onMouseDown={(e) => e.stopPropagation()}
            onClick={(e) => {
              e.stopPropagation();
              onClose(session.id);
            }}
          >
            <X className="size-3" strokeWidth={1.75} />
          </button>
        ) : null}
      </div>
      <div className="relative min-h-0 min-w-0 flex-1">
        {prepared && !prepared.settled ? (
          <TerminalView
            key={prepared.attempt}
            id={tuiPtyId(session.id)}
            cwd={tuiLaunchCwd(session)}
            active={visible && focused && !exit}
            launch={prepared.launch}
            attach={!!prepared.attach}
            onExit={onExit}
            captureExitOutput
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
