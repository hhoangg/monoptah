/**
 * Pure rules for starting a message: who it goes to, what it is called, which
 * mailbox it leaves from and what is quoted. Kept out of the composer so the
 * lazy editor chunk is not needed to decide any of it, and so each rule has a
 * test that does not mount an editor.
 */
import { domainOf } from "./mailDomain";
import { buildTextQuote, quotableHtml } from "./mailQuote";
import type {
  IdentityRow,
  MailAddress,
  MessageRow,
  OutboxKind,
  OutboxRow,
  SendableMailbox,
  SourceStatus,
  ThreadRow,
} from "./mailTypes";

export type ComposeRequest =
  | { kind: "new" }
  | {
      kind: "reply" | "reply_all" | "forward";
      /** The message answered or forwarded; the thread's newest from the reader. */
      message: MessageRow;
      thread: ThreadRow;
    }
  | { kind: "draft"; row: OutboxRow };

// ---------- subject ----------

/**
 * The prefix words thread grouping strips (`REPLY_PREFIXES` in mail_thread.rs).
 * Split here only by which composer action they belong to; a test reads the
 * Rust list and fails when the two drift.
 */
export const FORWARD_WORDS = ["fwd", "fw", "chuyển tiếp"];
export const REPLY_WORDS = [
  "re",
  "aw",
  "sv",
  "antw",
  "res",
  "vs",
  "tr",
  "rv",
  "trả lời",
];

const prefixPattern = (words: string[]) =>
  new RegExp(`^\\s*(?:${words.join("|")})\\s*(?:\\[\\d+\\])?\\s*:`, "i");
const REPLY_PREFIX = prefixPattern(REPLY_WORDS);
const FORWARD_PREFIX = prefixPattern(FORWARD_WORDS);

/** A reply to a reply keeps one prefix, whatever language the other client used. */
export function prefixedSubject(subject: string, kind: OutboxKind): string {
  // Matching is done on NFKC text, as subject_key in mail_thread.rs does (so a
  // decomposed "Trả lời" or a full-width "ＲＥ：" counts as a prefix), but the
  // returned subject is only NFC: the person's own characters are not rewritten.
  const plain = subject.trim().normalize("NFC");
  if (kind === "new") return plain;
  const has = kind === "forward" ? FORWARD_PREFIX : REPLY_PREFIX;
  if (has.test(plain.normalize("NFKC"))) return plain;
  return `${kind === "forward" ? "Fwd" : "Re"}: ${plain}`.trimEnd();
}

// ---------- recipients ----------

const key = (a: MailAddress) => a.addr.trim().toLowerCase();

function unique(list: MailAddress[]): MailAddress[] {
  const seen = new Set<string>();
  return list.filter((a) => {
    const k = key(a);
    if (k === "" || seen.has(k)) return false;
    seen.add(k);
    return true;
  });
}

/**
 * `own` is every address of the person's own mailboxes plus the chosen From:
 * a reply-all that included them would mail themselves.
 */
export function recipientsFor(
  kind: OutboxKind,
  message: MessageRow,
  own: readonly string[],
): { to: MailAddress[]; cc: MailAddress[] } {
  if (kind === "new" || kind === "forward") return { to: [], cc: [] };
  const mine = new Set(own.map((a) => a.trim().toLowerCase()));
  // Answering something we sent goes to whoever we sent it to.
  const base: MailAddress[] =
    message.direction === "outbound"
      ? message.to
      : [
          message.replyToAddr
            ? {
                name:
                  message.replyToAddr === message.fromAddr
                    ? message.fromName
                    : null,
                addr: message.replyToAddr,
              }
            : { name: message.fromName, addr: message.fromAddr },
        ];
  if (kind === "reply") return { to: unique(base), cc: [] };
  const to = unique(
    [...base, ...(message.direction === "outbound" ? [] : message.to)].filter(
      (a) => !mine.has(key(a)),
    ),
  );
  const taken = new Set(to.map(key));
  const cc = unique(message.cc).filter(
    (a) => !mine.has(key(a)) && !taken.has(key(a)),
  );
  return { to, cc };
}

const ADDRESS = /^[^\s@<>,;]+@[^\s@<>,;]+\.[^\s@<>,;]+$/;

export const isAddress = (addr: string) => ADDRESS.test(addr.trim());

/** "Ann <ann@x.org>, bob@y.org" -> recipients; what is not an address is returned apart. */
export function parseRecipients(text: string): {
  valid: MailAddress[];
  invalid: string[];
} {
  const valid: MailAddress[] = [];
  const invalid: string[] = [];
  for (const raw of text.split(/[,;\n]+/)) {
    const token = raw.trim();
    if (token === "") continue;
    const m = /^(.*)<([^<>]*)>$/.exec(token);
    const addr = (m ? m[2] : token).trim();
    const name = m ? m[1].trim().replace(/^"|"$/g, "") || null : null;
    if (isAddress(addr)) valid.push({ name, addr });
    else invalid.push(token);
  }
  return { valid, invalid };
}

export const addressLabel = (a: MailAddress) =>
  a.name ? `${a.name} <${a.addr}>` : a.addr;

// ---------- From ----------

export type SenderProblem =
  "notVerified" | "pending" | "sendingDisabled" | "sourceFailing";

export type SenderEntry = {
  address: string;
  domain: string;
  color: string | null;
  sourceLabel: string;
  region: string;
  isDefault: boolean;
  problem: SenderProblem | null;
};

export type SenderGroup = {
  domain: string;
  /** "label - region" of each source behind this domain, in order. */
  sources: string[];
  entries: SenderEntry[];
};

export const senderOk = (e: SenderEntry) => e.problem === null;

/**
 * Words for the greyed state. `sendingDisabled` is deliberately not "not
 * verified": AWS verified the identity and sending is off for the account, so
 * re-verifying the domain would change nothing.
 */
export const SENDER_PROBLEM_LABEL: Record<SenderProblem, string> = {
  notVerified: "not verified",
  pending: "verification pending",
  sendingDisabled: "sending disabled by AWS",
  sourceFailing: "source failing",
};

/**
 * Every mailbox of an enabled source grouped by domain, with the reason it
 * cannot send when it cannot. `sendable` is the authority on who may send
 * (`mail_sendable_mailboxes`); the sources and identities only explain why not.
 */
export function senderGroups(
  sources: readonly SourceStatus[],
  sendable: readonly SendableMailbox[],
  identities: readonly IdentityRow[],
): SenderGroup[] {
  const ok = new Map(sendable.map((s) => [s.address.toLowerCase(), s]));
  const groups = new Map<string, SenderGroup>();
  const ordered = sources
    .filter((s) => s.enabled)
    .sort((a, b) => a.sortOrder - b.sortOrder || a.id - b.id);
  for (const source of ordered) {
    const mailboxes = [...source.mailboxes].sort(
      (a, b) => a.sortOrder - b.sortOrder || a.id - b.id,
    );
    for (const mailbox of mailboxes) {
      const address = mailbox.address.toLowerCase();
      const domain = domainOf(address);
      const identity = (name: string) =>
        identities.find(
          (i) => i.accountId === source.id && i.identity === name,
        );
      const status = (identity(address) ?? identity(domain))?.status;
      const problem: SenderProblem | null = !ok.has(address)
        ? status === "sending_disabled"
          ? "sendingDisabled"
          : status === "pending"
            ? "pending"
            : "notVerified"
        : source.lastError
          ? "sourceFailing"
          : null;
      let group = groups.get(domain);
      if (!group) {
        group = { domain, sources: [], entries: [] };
        groups.set(domain, group);
      }
      const label = `${source.label} · ${source.sesRegion}`;
      if (!group.sources.includes(label)) group.sources.push(label);
      group.entries.push({
        address: mailbox.address,
        domain,
        color: mailbox.color,
        sourceLabel: source.label,
        region: source.sesRegion,
        isDefault: ok.get(address)?.isDefault ?? false,
        problem,
      });
    }
  }
  return [...groups.values()];
}

/** The mailbox the message arrived in, else the default, else any that can send. */
export function defaultFrom(
  groups: readonly SenderGroup[],
  message: MessageRow | null,
  thread: ThreadRow | null,
): string {
  const usable = groups.flatMap((g) => g.entries).filter(senderOk);
  const find = (addr: string) =>
    usable.find((e) => e.address.toLowerCase() === addr.toLowerCase());
  const addressed = message
    ? [...message.to, ...message.cc].map((a) => a.addr)
    : [];
  for (const addr of [...addressed, thread?.mailboxAddress ?? ""]) {
    const hit = addr ? find(addr) : undefined;
    if (hit) return hit.address;
  }
  return (usable.find((e) => e.isDefault) ?? usable[0])?.address ?? "";
}

// ---------- the quoted original ----------

export type Quote = {
  /** One line under the editor, e.g. "On <date>, Ann wrote:". */
  attribution: string;
  /** Sanitised by Rust, then made sendable by `quotableHtml`; appended untouched. */
  html: string;
  /** Plain-text twin for the text part. */
  text: string;
  /** What the collapsed block shows. */
  preview: string;
  /** The original as plain text, for "Quote inline". */
  plain: string;
};

const esc = (s: string) =>
  s
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;")
    .replace(/'/g, "&#39;");

const senderOf = (m: MessageRow) => m.fromName?.trim() || m.fromAddr;

/** Whole text part of a received message; the snippet only when it has none. */
const plainOf = (m: MessageRow) => (m.bodyText ?? m.snippet ?? "").trim();

export function buildQuote(
  kind: OutboxKind,
  message: MessageRow,
  formatDate: (seconds: number) => string = (s) =>
    new Date(s * 1000).toLocaleString(),
): Quote | null {
  if (kind === "new") return null;
  const date = formatDate(message.dateUtc);
  const body = message.bodyHtml
    ? quotableHtml(message.bodyHtml)
    : `<div style="white-space:pre-wrap">${esc(plainOf(message))}</div>`;
  const plain = plainOf(message);
  const preview = plain.replace(/\s+/g, " ").slice(0, 160);
  if (kind === "forward") {
    const lines = [
      [
        "From",
        addressLabel({ name: message.fromName, addr: message.fromAddr }),
      ],
      ["Date", date],
      ["Subject", message.subject],
      ["To", message.to.map(addressLabel).join(", ")],
    ];
    const head = "---------- Forwarded message ----------";
    return {
      attribution: "Forwarded message",
      html:
        `<div>${esc(head)}<br>` +
        lines.map(([k, v]) => `${k}: ${esc(v)}`).join("<br>") +
        `</div><br>${body}`,
      text: `${head}\n${lines.map(([k, v]) => `${k}: ${v}`).join("\n")}\n\n${plain}`,
      preview,
      plain,
    };
  }
  const attribution = `On ${date}, ${senderOf(message)} wrote:`;
  return {
    attribution,
    html:
      `<div>${esc(attribution)}</div>` +
      `<blockquote style="margin:0 0 0 .5em;padding-left:12px;border-left:2px solid #d0d0d0">${body}</blockquote>`,
    text: buildTextQuote(plain, attribution),
    preview,
    plain,
  };
}

// ---------- what a stored row allows ----------

export type SendMode =
  | { kind: "send"; label: string }
  /** SES may have delivered it: no Send, no Retry, only the guarded way out. */
  | { kind: "checkFirst" }
  | { kind: "busy" }
  | { kind: "done" };

/**
 * What the composer may offer, decided from the row's state and from nothing
 * else. `SendError.retrySafe` describes one call, not the row: refusing an
 * `unconfirmed` row is a `form` error with `retrySafe: true`, so branching on
 * it would put a Retry button on the one message that may already be delivered.
 */
export function sendMode(state: OutboxRow["state"] | null): SendMode {
  switch (state) {
    case "unconfirmed":
      return { kind: "checkFirst" };
    case "sending":
      return { kind: "busy" };
    case "sent":
      return { kind: "done" };
    case "failed":
      return { kind: "send", label: "Send again" };
    default:
      return { kind: "send", label: "Send" };
  }
}
