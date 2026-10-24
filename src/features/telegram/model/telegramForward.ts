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
/** Hard cap on the summary, in characters. The summary aims lower, at SUMMARY_TARGET. */
export const CONTEXT_MAX = 300;
const SUMMARY_TARGET = 280;

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

const CUT_MARK = "… ";
const SENTENCE_END = ".!?…";
const CJK_SENTENCE_END = "。！？";
const SENTENCE_CLOSERS = "\"')]”’」）";
const HORIZONTAL_RULE = /^([-*_])(\s*\1){2,}$/;
const TABLE_RULE = /^\|?[\s:|-]*-[\s:|-]*$/;
const LIST_ITEM = /^\s*(?:[-*+]|\d+[.)])\s+(.*)$/;

/** One sentence, heading, table row or list item of the cleaned message. */
interface Unit {
  text: string;
  list: boolean;
}

/** Inline markdown and html reduced to the words they carry. */
function stripInline(text: string): string {
  return text
    .replace(/!\[[^\]]*\]\([^)]*\)/g, "")
    .replace(/\[([^\]]+)\]\([^)]*\)/g, "$1")
    .replace(/<(https?:\/\/[^>\s]+)>/g, "$1")
    .replace(/<\/?[A-Za-z][^<>]*>/g, "")
    .replace(/`+([^`]+)`+/g, "$1")
    .replace(/(\*\*|__)(\S(?:.*?\S)?)\1/g, "$2")
    .replace(/~~(\S(?:.*?\S)?)~~/g, "$1")
    .replace(/\*(\S(?:[^*]*\S)?)\*/g, "$1")
    .replace(/(^|\W)_(\S(?:[^_]*\S)?)_(?!\w)/g, "$1$2")
    .replace(/\s+/g, " ")
    .trim();
}

function splitSentences(text: string): string[] {
  const all = chars(text);
  const sentences: string[] = [];
  let start = 0;
  let i = 0;
  while (i < all.length) {
    const char = all[i];
    let end = -1;
    if (CJK_SENTENCE_END.includes(char)) {
      end = i + 1;
    } else if (SENTENCE_END.includes(char)) {
      let j = i + 1;
      while (j < all.length && SENTENCE_END.includes(all[j])) j++;
      while (j < all.length && SENTENCE_CLOSERS.includes(all[j])) j++;
      if (j >= all.length || /\s/.test(all[j])) end = j;
    }
    if (end < 0) {
      i++;
      continue;
    }
    sentences.push(all.slice(start, end).join("").trim());
    start = end;
    i = end;
  }
  sentences.push(all.slice(start).join("").trim());
  return sentences.filter(Boolean);
}

/** The message as units, with code, rules and markup noise left out. */
function messageUnits(raw: string): Unit[] {
  const units: Unit[] = [];
  let prose: string[] = [];
  let lastItem: Unit | undefined;
  let fence: string | undefined;
  const standalone = (text: string, list = false): Unit | undefined => {
    if (!text) return undefined;
    const unit = { text, list };
    units.push(unit);
    return unit;
  };
  const flush = () => {
    for (const sentence of splitSentences(stripInline(prose.join(" ")))) {
      standalone(sentence);
    }
    prose = [];
  };
  for (const source of raw.split("\n")) {
    const line = source.replace(/^\s*(?:>\s?)+/, "");
    const trimmed = line.trim();
    if (fence) {
      if (trimmed.startsWith(fence)) fence = undefined;
      continue;
    }
    const opener = /^(`{3,}|~{3,})(.*)$/.exec(trimmed);
    // ```x``` on one line is inline code, not a fence.
    if (opener && !(opener[1][0] === "`" && opener[2].includes("`"))) {
      flush();
      lastItem = undefined;
      fence = opener[1];
      continue;
    }
    if (!trimmed || HORIZONTAL_RULE.test(trimmed) || TABLE_RULE.test(trimmed)) {
      flush();
      lastItem = undefined;
      continue;
    }
    const heading = /^#{1,6}\s+(.*?)(?:\s+#+)?$/.exec(trimmed);
    if (heading) {
      flush();
      lastItem = undefined;
      standalone(stripInline(heading[1]));
      continue;
    }
    const item = LIST_ITEM.exec(line);
    if (item) {
      flush();
      const body = stripInline(item[1]);
      lastItem = body ? standalone(`- ${body}`, true) : undefined;
      continue;
    }
    if (lastItem && /^\s/.test(line)) {
      lastItem.text += ` ${stripInline(trimmed)}`;
      continue;
    }
    lastItem = undefined;
    if (trimmed.startsWith("|")) {
      flush();
      standalone(
        trimmed
          .replace(/^\||\|$/g, "")
          .split("|")
          .map(stripInline)
          .filter(Boolean)
          .join(", "),
      );
      continue;
    }
    prose.push(trimmed);
  }
  flush();
  return units;
}

function normalized(text: string): string {
  return text.toLowerCase().replace(/[^\p{L}\p{N}]+/gu, "");
}

/** A unit that says no more than the question prompt itself. */
function repeatsPrompt(unit: Unit, prompts: string[]): boolean {
  const said = normalized(unit.text);
  if (said.length < 6) return false;
  return prompts.some((prompt) => {
    const asked = normalized(prompt);
    if (asked.length < 6) return false;
    return (
      asked.includes(said) ||
      (said.includes(asked) && said.length <= asked.length + 10)
    );
  });
}

function joinerOf(before: Unit, after: Unit): string {
  if (before.list || after.list) return "\n";
  return CJK_SENTENCE_END.includes(before.text.slice(-1)) ? "" : " ";
}

function joinUnits(units: Unit[]): string {
  return units.reduce(
    (text, unit, index) =>
      index === 0
        ? unit.text
        : text + joinerOf(units[index - 1], unit) + unit.text,
    "",
  );
}

/** The end of one long unit, from a word boundary, behind the cut marker. */
function clipTail(text: string, budget: number): string {
  const room = budget - chars(CUT_MARK).length;
  const words = text.split(" ");
  const kept: string[] = [];
  let used = 0;
  for (let i = words.length - 1; i >= 0; i--) {
    const cost = chars(words[i]).length + (kept.length ? 1 : 0);
    if (used + cost > room) break;
    kept.unshift(words[i]);
    used += cost;
  }
  // A single unbroken run longer than the room has no word boundary to cut at.
  if (kept.length === 0) {
    kept.push(
      chars(words[words.length - 1])
        .slice(-room)
        .join(""),
    );
  }
  return CUT_MARK + kept.join(" ");
}

/**
 * A short summary of what the agent said just before asking: the whole sentences closest
 * to the question, up to about SUMMARY_TARGET characters, as plain text. Deterministic
 * and local (no model call). It starts with an ellipsis when earlier text was left out.
 */
export function questionContext(session: Session): string {
  for (let index = session.blocks.length - 1; index >= 0; index--) {
    const block = session.blocks[index];
    if (block.role !== "assistant" || block.internal) continue;
    if (!block.text.trim()) continue;
    // A message that is only code has no summary; do not fall back to an older one.
    let units = messageUnits(block.text);
    if (units.length === 0) return "";
    // Sentences at the end that only repeat the question add nothing; drop them when
    // something else was said.
    const prompts =
      session.pendingQuestion?.questions.map((q) => q.prompt) ?? [];
    let repeats = 0;
    while (
      repeats < units.length &&
      repeatsPrompt(units[units.length - 1 - repeats], prompts)
    ) {
      repeats++;
    }
    if (repeats < units.length) units = units.slice(0, units.length - repeats);

    const picked: Unit[] = [];
    let used = 0;
    let i = units.length - 1;
    for (; i >= 0; i--) {
      const unit = units[i];
      const cost =
        chars(unit.text).length +
        (picked.length ? chars(joinerOf(unit, picked[0])).length : 0);
      if (used + cost > SUMMARY_TARGET) {
        if (picked.length === 0) return clipTail(unit.text, SUMMARY_TARGET);
        break;
      }
      picked.unshift(unit);
      used += cost;
    }
    return (i >= 0 ? CUT_MARK : "") + joinUnits(picked);
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
