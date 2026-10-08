import type { PtyLaunch } from "../../../platform/tauri/pty";
import { TUI_CAPS } from "../../../integrations/harness/core/tuiLaunch";
import type { InboxComposerCard } from "../../inbox/model/githubTasks";
import { runtimeProviderBinaryPath } from "../../providers/model/providerBinaryPaths";
import {
  DEFAULT_PROVIDER_ACCOUNT_ID,
  supportsProviderAccounts,
} from "../../providers/model/providerAccounts";
import {
  HARNESS_TITLE,
  sessionDisplayTitle,
  type HarnessId,
  type Session,
} from "./session";

/** An exit sooner than this after launch reads as "never really started". */
export const TUI_EARLY_EXIT_MS = 5000;

const MAX_TITLE_LENGTH = 80;

export function isTuiSession(
  session: Pick<Session, "surface"> | undefined,
): boolean {
  return session?.surface === "tui";
}

/** One PTY per session; the prefix keeps it apart from file-pane terminals. */
export function tuiPtyId(sessionId: string): string {
  return `tui:${sessionId}`;
}

/** A deleted worktree no longer exists, so fall back to the project folder. */
export function tuiLaunchCwd(
  session: Pick<Session, "cwd" | "worktreeCwd" | "worktreeRemoved">,
): string {
  return session.worktreeRemoved
    ? session.cwd
    : (session.worktreeCwd ?? session.cwd);
}

/** The default profile needs no env, so only a named account is sent. */
export function tuiProviderAccount(
  harness: HarnessId,
  accountId: string | undefined,
): PtyLaunch["providerAccount"] {
  if (!supportsProviderAccounts(harness)) return undefined;
  if (!accountId || accountId === DEFAULT_PROVIDER_ACCOUNT_ID) return undefined;
  return { provider: harness, id: accountId };
}

/**
 * Identifies the CLI the way a chat spawn does, so the backend can check the
 * program is a resolved harness binary. Antigravity launches the bare `agy`,
 * which has no configured path, so only its provider is sent.
 */
export function tuiBinaryIdentity(
  harness: HarnessId,
): Pick<PtyLaunch, "binaryProvider" | "binaryPath"> {
  if (harness === "antigravity") return { binaryProvider: harness };
  const binaryPath = runtimeProviderBinaryPath(harness);
  return binaryPath
    ? { binaryProvider: harness, binaryPath }
    : { binaryProvider: harness };
}

/** What a terminal session may change about itself, saved with the workspace. */
export type TuiSessionPatch = {
  /** `null` forgets the saved conversation so the next launch starts fresh. */
  providerSessionId?: string | null;
  providerAccountId?: string;
};

export function applyTuiSessionPatch(
  session: Session,
  patch: TuiSessionPatch,
): Session {
  let next = session;
  if (patch.providerSessionId === null) {
    if (next.providerSessionId !== undefined) {
      next = { ...next, providerSessionId: undefined };
    }
  } else if (
    patch.providerSessionId &&
    patch.providerSessionId !== next.providerSessionId
  ) {
    next = { ...next, providerSessionId: patch.providerSessionId };
  }
  if (
    patch.providerAccountId &&
    patch.providerAccountId !== next.providerAccountId
  ) {
    next = { ...next, providerAccountId: patch.providerAccountId };
  }
  return next;
}

/**
 * Provider CLIs prefix the terminal title with a status glyph or spinner frame.
 * Keep the words only, so the tab does not flicker through animation frames.
 */
export function cleanTuiTitle(raw: string): string {
  const words = raw
    .replace(/[\u0000-\u001f\u007f]/g, " ")
    .replace(/^[^\p{L}\p{N}]+/u, "")
    .replace(/\s+/g, " ")
    .trim();
  return words.length > MAX_TITLE_LENGTH
    ? `${words.slice(0, MAX_TITLE_LENGTH - 1).trimEnd()}…`
    : words;
}

/**
 * The CLI may only retitle a tab that still shows a placeholder or its own
 * earlier title. An Inbox title or a rename the user typed stays.
 */
export function canRetitleFromTui(
  session: Pick<Session, "title" | "harness">,
  lastAutoTitle: string | undefined,
): boolean {
  if (lastAutoTitle !== undefined && session.title === lastAutoTitle) {
    return true;
  }
  return sessionDisplayTitle(session.title, session.harness) === "New session";
}

export type TuiExit = {
  code: number | null;
  /** The process never started, so there is no exit code. */
  error?: string;
  /** Exited within `TUI_EARLY_EXIT_MS` of launch. */
  early: boolean;
};

/**
 * What a terminal pane reports about its CLI launch: `started` once the PTY
 * has spawned the process, `exit` when the pane settles on an exit.
 */
export type TuiLaunchEvent =
  { kind: "started" } | { kind: "exit"; exit: TuiExit };

export type TuiExitNotice = {
  message: string;
  hint?: string;
};

/** Plain-language bar copy; an instant exit is how "no interactive mode" shows. */
export function tuiExitNotice(
  harness: HarnessId,
  exit: TuiExit,
): TuiExitNotice {
  const label = HARNESS_TITLE[harness];
  if (exit.error) {
    return {
      message: `Could not start ${label}`,
      hint: exit.error,
    };
  }
  const message =
    exit.code == null ? "Process exited" : `Process exited (code ${exit.code})`;
  if (!exit.early) return { message };
  return {
    message,
    hint: `${label} stopped right after it started. It may not have an interactive mode, may need you to sign in, or may be missing. Check the output above.`,
  };
}

/**
 * Terminal output as the words a reader sees. Claude draws each word at an
 * absolute column (`ESC[4G`) with no space between, so cursor moves must turn
 * into a space before the escapes are dropped.
 */
export function plainTerminalText(raw: string): string {
  return raw
    .replace(/\u001b\][^\u0007\u001b]*(?:\u0007|\u001b\\)/g, "")
    .replace(/\u001b\[[0-9;?]*[ABCDEFGHJKf`d]/g, " ")
    .replace(/\u001b\[[0-9;?<>=! ]*[@-~]/g, "")
    .replace(/\u001b[()][A-Za-z0-9]/g, "")
    .replace(/\u001b[=>78]/g, "")
    .replace(/[\u0000-\u0008\u000b-\u001f\u007f]/g, " ")
    .replace(/\s+/g, " ")
    .trim();
}

/**
 * Claude's answer to `--resume <id>` when it never wrote that conversation.
 * The exit code is not reliable for this (print mode exits 0), so match the
 * message itself.
 */
export function isNoConversationOutput(raw: string | undefined): boolean {
  return (
    !!raw &&
    /no conversation found with session id/i.test(plainTerminalText(raw))
  );
}

/**
 * Resuming an id the CLI never saved fails at once. Offer a fresh start for
 * exactly that case, so a bad saved id cannot trap the session.
 */
export function canStartFreshAfter(
  harness: HarnessId,
  exit: TuiExit,
  resumed: boolean,
): boolean {
  return !exit.error && exit.early && resumed && TUI_CAPS[harness].resume;
}

/**
 * Inbox "Start work". A chat shows the card above its composer; a terminal has
 * no composer, so the composed prompt becomes the CLI's first prompt. A CLI
 * that cannot take a first prompt would drop it, so that session stays chat.
 */
export function applyInboxStart(
  session: Session,
  card: InboxComposerCard,
): Session {
  if (session.surface !== "tui") return { ...session, inboxCard: card };
  if (!TUI_CAPS[session.harness].initialPrompt) {
    return { ...session, surface: "chat", inboxCard: card };
  }
  const prompt = card.prompt.trim();
  return prompt ? { ...session, initialPrompt: prompt } : session;
}
