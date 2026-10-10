import {
  leafIds,
  newTab,
  type WorkspaceTab,
} from "../../workspace/model/layout";
import type {
  DockSide,
  ProjectTerminalDock,
} from "../../projects/model/projectTerminal";
import { sessionNeedsInput, type Session } from "./session";
import { stopStreaming } from "../../../integrations/harness/core/apply";
import type { ProjectReturnMemory } from "../../projects/model/projectReturn";

export const INTERRUPT_MESSAGE = "Turn interrupted when MonoCode quit.";

export const CONTINUE_PROMPT = "Continue from where you left off.";

export type InFlightRef = {
  sessionId: string;
  cwd: string;
};

export type ResumedWorkspace = {
  sessions: Session[];
  tabs: WorkspaceTab[];
  activeTabId: string;
  projectCwd: string;
  projectTerminals?: ProjectTerminalDock[];
  projectReturnMemory?: ProjectReturnMemory;
  lastDockSide?: DockSide;
};

/** A turn or approval that would be lost if this webview died. */
export function isInFlightSession(session: Session): boolean {
  return (
    !session.worktreeRemoved && (!!session.busy || sessionNeedsInput(session))
  );
}

export function hasInFlightSessions(sessions: Session[]): boolean {
  return sessions.some(isInFlightSession);
}

/**
 * Busy chats in tab order, then parked ones still running after their tab closed.
 * Only persistable sessions can come back after a real quit.
 */
export function inFlightRefs(
  sessions: Session[],
  tabs: WorkspaceTab[],
): InFlightRef[] {
  const byId = new Map(sessions.map((session) => [session.id, session]));
  const seen = new Set<string>();
  const refs: InFlightRef[] = [];

  const push = (session: Session | undefined) => {
    if (!session || session.inboxAsk || seen.has(session.id)) return;
    if (!isInFlightSession(session) || !canResumeAfterQuit(session)) return;
    seen.add(session.id);
    refs.push({ sessionId: session.id, cwd: session.cwd });
  };

  for (const tab of tabs) {
    for (const id of leafIds(tab.layout)) push(byId.get(id));
  }
  for (const session of sessions) push(session);
  return refs;
}

/**
 * Says what is true of each kind: a chat turn is resumed on reopen
 * (`persistQuitState` records it); a terminal turn dies with the CLI and is
 * never re-sent, but its conversation is kept, because the session stores the
 * conversation id minted before the CLI started and relaunches with
 * `claude --resume`.
 *
 * The terminal clause promises the conversation is saved, not that it comes
 * back on its own. Reopening the tab for you needs the session to be in the
 * workspace snapshot, which a parked session or a second window that lost the
 * last-writer race is not, and `close_poll` charges a window that never
 * answered the poll one terminal turn whose kind nobody can vouch for.
 */
export function quitWhileBusyMessage(chats: number, terminals: number): string {
  const chatText = `${chats} ${chats === 1 ? "chat" : "chats"}`;
  const terminalText = `${terminals} terminal ${terminals === 1 ? "turn" : "turns"}`;
  if (terminals === 0) {
    return chats === 1
      ? `${chatText} is still running. Quit anyway? It will resume when you reopen Monoptah.`
      : `${chatText} are still running. Quit anyway? They will resume when you reopen Monoptah.`;
  }
  if (chats === 0) {
    return terminals === 1
      ? `${terminalText} is still running. Quit anyway? It will stop, though its conversation is saved.`
      : `${terminalText} are still running. Quit anyway? They will stop, though their conversations are saved.`;
  }
  const chatClause =
    chats === 1 ? "The chat will resume" : "The chats will resume";
  const terminalClause =
    terminals === 1
      ? "the terminal turn will stop, though its conversation is saved"
      : "the terminal turns will stop, though their conversations are saved";
  return `${chatText} and ${terminalText} are still running. Quit anyway? ${chatClause} when you reopen Monoptah; ${terminalClause}.`;
}

/**
 * Seal streams/tools and record that a real quit cut the turn short.
 * Idempotent for the current turn only — a later turn after Continue still
 * gets a fresh note. Matching any interrupt in the transcript would skip
 * resume on the second quit.
 */
export function markTurnInterrupted(session: Session): Session {
  const sealed = { ...sealOpenWork(stopStreaming(session)), busy: false };
  if (lastBlockIsInterrupt(sealed)) return sealed;
  return {
    ...sealed,
    blocks: [
      ...sealed.blocks,
      {
        id: crypto.randomUUID(),
        role: "system",
        text: INTERRUPT_MESSAGE,
        notice: "interrupt",
      },
    ],
  };
}

export function workspaceFromResumed(
  sessions: Session[],
): ResumedWorkspace | null {
  sessions = sessions.filter((session) => !session.inboxAsk);
  if (sessions.length === 0) return null;
  const tabs = sessions.map((session) => newTab(session.id));
  return {
    sessions,
    tabs,
    activeTabId: tabs[0].id,
    projectCwd: sessions[0].cwd,
  };
}

export function wasTurnInterrupted(session: Session): boolean {
  return lastBlockIsInterrupt(session);
}

/**
 * Provider thread exists and the quit note is still the last block.
 * A Continue (or any later user turn) appends after it, so this stays one-shot.
 */
export function canAutoContinue(session: Session): boolean {
  return (
    !session.worktreeRemoved &&
    !!session.providerSessionId &&
    !session.busy &&
    lastBlockIsInterrupt(session)
  );
}

function lastBlockIsInterrupt(session: Session): boolean {
  const last = session.blocks[session.blocks.length - 1];
  return last?.role === "system" && last.text === INTERRUPT_MESSAGE;
}

export function inFlightSnapshotKey(refs: InFlightRef[]): string {
  return refs.map((ref) => ref.sessionId).join("\n");
}

/**
 * Skip the first idle paint after boot so a restored snapshot is not wiped
 * before the turn is marked busy again. Once this process has seen a live
 * in-flight chat, an empty list is a real "turn finished" and should clear.
 */
export function shouldWriteInFlightSnapshot(
  key: string,
  refs: InFlightRef[],
  previousKey: string | null,
  sawInFlight: boolean,
): boolean {
  if (previousKey === key) return false;
  if (refs.length === 0 && !sawInFlight) return false;
  return true;
}

function canResumeAfterQuit(session: Session): boolean {
  return (
    !session.worktreeRemoved &&
    session.cwd !== "~" &&
    session.blocks.some((block) => block.role === "user")
  );
}

function sealOpenWork(session: Session): Session {
  return {
    ...session,
    busy: false,
    blocks: session.blocks.flatMap((block) => {
      if (block.role === "approval" && !block.approval?.decided) return [];
      if (!shouldCancelTool(block)) return [block];
      const { approval, ...rest } = block;
      return [
        {
          ...rest,
          streaming: false,
          tool: block.tool
            ? { ...block.tool, status: "cancelled" }
            : block.tool,
          ...(approval?.decided ? { approval } : {}),
        },
      ];
    }),
  };
}

function shouldCancelTool(block: Session["blocks"][number]): boolean {
  if (block.approval && !block.approval.decided) return true;
  if (!block.tool) return false;
  if (block.streaming) return true;
  const status = block.tool.status?.toLowerCase() ?? "";
  return (
    status === "in_progress" || status === "pending" || status === "running"
  );
}
