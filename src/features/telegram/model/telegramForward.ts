import { projectName } from "../../../shared/lib/paths";
import { isMonoSession } from "../../monos/model/mono";
import { isRemoteProjectPath } from "../../projects/model/recents";
import {
  sessionDisplayTitle,
  type Session,
} from "../../sessions/model/session";
import { isTuiSession } from "../../sessions/model/tuiSession";
import type { ForwardPayload, TelegramStatus } from "./telegram";
import { sessionTicket } from "./sessionTicket";
import { workflowSessionContext } from "./workflowSessionContext";

/** The wait before a question is sent when the project's setting is not known. */
export const FORWARD_DELAY_MS = 5 * 60_000;

const TOPIC_TITLE_MAX = 128;
const CONTEXT_MAX = 600;

/** Whether a session could ever post questions to Telegram, whatever it is asking now. */
export function isTopicLinkable(
  session: Pick<
    Session,
    "id" | "cwd" | "orchestrationLeadId" | "inboxAsk" | "ephemeral" | "surface"
  >,
): boolean {
  if (session.orchestrationLeadId) return false;
  if (session.inboxAsk) return false;
  if (session.ephemeral) return false;
  if (isTuiSession(session)) return false;
  if (isMonoSession(session.id)) return false;
  if (isRemoteProjectPath(session.cwd)) return false;
  return true;
}

/** Sessions whose questions go to Telegram: ones a person would answer in a project tab. */
export function isForwardable(session: Session): boolean {
  if (!isTopicLinkable(session)) return false;
  // Codex questions that auto-skip never block the agent, so nobody needs the ping.
  if (session.pendingQuestion?.autoResolveAt !== undefined) return false;
  return true;
}

export function sessionTicketId(session: Session): string | undefined {
  const remembered = sessionTicket(session.id);
  if (remembered) return remembered;
  const fromCard = session.inboxCard?.identifier.trim();
  if (fromCard) return fromCard;
  const fromWorkflow = workflowSessionContext(session.id)?.itemId?.trim();
  if (fromWorkflow) return fromWorkflow;
  if (session.linkedWorkItem) return `#${session.linkedWorkItem.number}`;
  return undefined;
}

function chars(text: string): string[] {
  return Array.from(text);
}

/** Inbox sessions are titled "<ticket> <title>"; the ticket is shown on its own, so drop it here. */
function title(session: Session, ticket?: string): string {
  const shown = sessionDisplayTitle(session.title, session.harness);
  const prefix = ticket ? `${ticket} ` : "";
  return prefix && shown.startsWith(prefix)
    ? shown.slice(prefix.length)
    : shown;
}

export function topicTitle(session: Session): string {
  const ticket = sessionTicketId(session);
  const full = ticket
    ? `[${ticket}] ${title(session, ticket)}`
    : title(session);
  return chars(full).slice(0, TOPIC_TITLE_MAX).join("");
}

/** The end of the agent's last message: usually why it is asking. */
export function questionContext(session: Session): string {
  for (let index = session.blocks.length - 1; index >= 0; index--) {
    const block = session.blocks[index];
    if (block.role !== "assistant" || block.internal) continue;
    const text = block.text.trim();
    if (text) return chars(text).slice(-CONTEXT_MAX).join("");
  }
  return "";
}

/** The project's configured wait in milliseconds; 5 minutes while its status is unknown. */
export function forwardDelayMs(
  status: Partial<Pick<TelegramStatus, "delaySecs">> | undefined,
): number {
  const secs = status?.delaySecs;
  return typeof secs === "number" && Number.isFinite(secs) && secs >= 0
    ? secs * 1000
    : FORWARD_DELAY_MS;
}

function headerOf(session: Session): string {
  const ticket = sessionTicketId(session);
  const first = [projectName(session.cwd), title(session, ticket), ticket]
    .filter(Boolean)
    .join(" · ");
  const workflow = workflowSessionContext(session.id);
  if (!workflow) return first;
  const second = [
    `Workflow ${workflow.workflowName}`,
    workflow.nodeTitle,
    workflow.itemLabel,
    workflow.round !== undefined ? `round ${workflow.round}` : undefined,
  ]
    .filter(Boolean)
    .join(" · ");
  return `${first}\n${second}`;
}

export function forwardPayload(
  session: Session,
  project: string,
  delaySecs: number,
): ForwardPayload | undefined {
  const pending = session.pendingQuestion;
  if (!pending) return undefined;
  return {
    project,
    sessionId: session.id,
    requestId: pending.requestId,
    topicTitle: topicTitle(session),
    header: headerOf(session),
    context: questionContext(session),
    delaySecs,
    questions: pending.questions.map((question) => ({
      id: question.id,
      ...(question.header ? { header: question.header } : {}),
      prompt: question.prompt,
      multiSelect: question.multiSelect,
      allowCustom: question.allowCustom,
      options: question.options.map((option) => ({
        id: option.id,
        label: option.label,
        ...(option.description ? { description: option.description } : {}),
      })),
    })),
  };
}

/**
 * Sessions whose current question has waited as long as their project's delay and was
 * not sent yet. A question that was never seen (it appeared while forwarding was off) is
 * never due, whatever the delay.
 */
export function dueForwards(
  sessions: Session[],
  firstSeen: Map<string, number>,
  sent: Set<string>,
  now: number,
  delayMsFor: (session: Session) => number,
): Session[] {
  return sessions.filter((session) => {
    const pending = session.pendingQuestion;
    if (!pending || !isForwardable(session)) return false;
    const key = `${session.id}#${pending.requestId}`;
    if (sent.has(key)) return false;
    const seen = firstSeen.get(key);
    return seen !== undefined && now - seen >= delayMsFor(session);
  });
}
