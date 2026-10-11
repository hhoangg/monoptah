/**
 * Mirrors the serde output of src-tauri/src/mail*.rs (camelCase). No type here
 * has a field for a secret coming back: commands return masked key ids only.
 *
 * Timestamp units differ by field and `new Date(seconds)` silently yields
 * January 1970, so every timestamp says which it is.
 */

// ── credentials ────────────────────────────────────────────────────

export type MailCredential = {
  id: number;
  provider: string;
  label: string;
  /** Display only, e.g. `AKIA••••3K7Q`. */
  accessKeyMasked: string;
  defaultRegion: string | null;
  /** Epoch milliseconds. */
  createdAt: number;
  /** Epoch milliseconds. */
  updatedAt: number;
};

/**
 * Write-only: the key pair goes to the OS keychain and is never returned.
 * Nothing in the app may store or log this value.
 */
export type CredentialUpsertInput = {
  label: string;
  accessKeyId: string;
  secretAccessKey: string;
  defaultRegion: string | null;
};

// ── sources and mailboxes ──────────────────────────────────────────

export type AttachPolicy = "on_demand" | "always";
export type ImagePolicy = "ask" | "always" | "never";

export type MailAccount = {
  id: number;
  provider: string;
  credentialId: number;
  s3CredentialId: number | null;
  label: string;
  sesRegion: string;
  bucket: string;
  s3Region: string;
  defaultFrom: string | null;
  attachPolicy: AttachPolicy;
  imagePolicy: ImagePolicy;
  enabled: boolean;
  sortOrder: number;
  /** Epoch milliseconds. */
  createdAt: number;
  /** Epoch milliseconds. */
  updatedAt: number;
};

export type AccountInput = {
  /** Set to edit that source in place; null creates one or matches an identical source. */
  id: number | null;
  provider: string;
  credentialId: number;
  s3CredentialId: number | null;
  label: string;
  sesRegion: string;
  bucket: string;
  s3Region: string;
  defaultFrom: string | null;
  attachPolicy: AttachPolicy;
  imagePolicy: ImagePolicy;
  enabled: boolean;
  sortOrder: number;
};

export type MailMailbox = {
  id: number;
  accountId: number;
  address: string;
  domain: string;
  s3Prefix: string;
  displayName: string;
  color: string | null;
  sortOrder: number;
  /** True when the address, or its domain, is a verified SES identity. */
  isSendable: boolean;
};

export type MailboxInput = {
  address: string;
  s3Prefix: string;
  displayName: string;
  color: string | null;
  sortOrder: number;
};

/** `mail_sources` and `mail_source_upsert` result. */
export type SourceRow = MailAccount & {
  /** Epoch milliseconds; null until identities have been fetched once. */
  identitiesRefreshedAt: number | null;
  mailboxes: MailMailbox[];
};

// ── identities ─────────────────────────────────────────────────────

/**
 * - `verified`: sendable.
 * - `pending` / `failed`: SES verification has not completed or has failed.
 * - `sending_disabled`: AWS verified the identity but sending is switched off
 *   for the account. Deliberately not sendable, and not a verification
 *   problem, so the copy must not tell the user to re-verify.
 */
export type IdentityStatus =
  "verified" | "pending" | "failed" | "sending_disabled";

export type MailIdentity = {
  /** Lowercased address or domain. */
  identity: string;
  kind: "domain" | "email";
  status: IdentityStatus;
};

export type IdentityRow = MailIdentity & { accountId: number };

// ── status and sync ────────────────────────────────────────────────

export type MailSyncState = {
  mailboxId: number;
  /** Epoch milliseconds. */
  lastStartedAt: number | null;
  /** Epoch milliseconds. */
  lastFinishedAt: number | null;
  lastError: string | null;
  objectsSeen: number;
  objectsAdded: number;
  objectsGone: number;
};

export type MailboxStatus = MailMailbox & {
  unread: number;
  sync: MailSyncState | null;
};

export type SourceStatus = MailAccount & {
  credential: MailCredential | null;
  s3Credential: MailCredential | null;
  /** Epoch milliseconds. */
  identitiesRefreshedAt: number | null;
  mailboxes: MailboxStatus[];
  unread: number;
  /** Epoch milliseconds; newest finished sync across the mailboxes. */
  lastSyncAt: number | null;
  /** The first mailbox error, so the sidebar can badge the source. */
  lastError: string | null;
};

export type MailStatus = {
  sources: SourceStatus[];
  /** Over enabled sources only. */
  unreadTotal: number;
  syncIntervalSecs: number;
};

export type SyncScope =
  | { kind: "all" }
  | { kind: "source"; id: number }
  | { kind: "mailbox"; id: number };

export type SyncReport = {
  accountId: number;
  inserted: number;
  archived: number;
  reparsed: number;
  failed: number;
  error: string | null;
};

/** `mail://sync-state` event payload. */
export type SyncStateEvent = {
  accountId: number;
  mailboxId: number | null;
  phase: "running" | "done" | "error";
  done: number;
  total: number;
  error: string | null;
};

// ── source test ────────────────────────────────────────────────────

export type CheckResult = {
  ok: boolean;
  /** The IAM action the check needs. */
  action: string;
  /** e.g. `accessDenied`, `wrongRegion`, `clockSkew`. */
  kind: string | null;
  message: string | null;
};

/** One per source; there is deliberately no combined verdict. */
export type TestReport = {
  accountId: number;
  label: string;
  s3: CheckResult;
  sesIdentities: CheckResult;
  sesAccount: CheckResult;
  identityCount: number | null;
  /** True while SES only delivers to verified addresses. */
  sandbox: boolean | null;
};

// ── threads and messages ───────────────────────────────────────────

export type ThreadScope =
  { kind: "all" } | { kind: "mailbox"; mailboxId: number };

/** Keyset position after the last row of the previous page. */
export type ThreadCursor = {
  /** Epoch seconds. */
  lastMessageAt: number;
  id: number;
};

export type ThreadQuery = {
  scope: ThreadScope;
  cursor?: ThreadCursor | null;
  limit?: number | null;
};

export type Page<T> = {
  rows: T[];
  nextCursor: ThreadCursor | null;
};

export type MailAddress = { name: string | null; addr: string };

export type ThreadRow = {
  id: number;
  accountId: number;
  mailboxId: number;
  mailboxAddress: string;
  domain: string;
  subject: string;
  snippet: string;
  participants: MailAddress[];
  /** Epoch seconds. */
  firstMessageAt: number;
  /** Epoch seconds. */
  lastMessageAt: number;
  messageCount: number;
  unreadCount: number;
  hasAttachments: boolean;
  isStarred: boolean;
};

export type AttachmentRow = {
  id: number;
  messageId: number;
  partIndex: number;
  filename: string;
  mimeType: string;
  sizeBytes: number;
  contentId: string | null;
  isInline: boolean;
  /** Whether the bytes are already on disk. The path stays in Rust. */
  downloaded: boolean;
};

export type MessageRow = {
  id: number;
  direction: "inbound" | "outbound";
  messageIdHdr: string | null;
  inReplyTo: string | null;
  subject: string;
  fromName: string | null;
  fromAddr: string;
  replyToAddr: string | null;
  to: MailAddress[];
  cc: MailAddress[];
  /** Only outgoing mail carries a Bcc list. */
  bcc: MailAddress[];
  /** Epoch seconds. */
  dateUtc: number;
  snippet: string;
  bodyText: string | null;
  /** Already sanitised; remote images are in `data-mono-src`. */
  bodyHtml: string | null;
  remoteImagesBlocked: number;
  hasAttachments: boolean;
  sizeBytes: number | null;
  spf: string | null;
  dkim: string | null;
  dmarc: string | null;
  isRead: boolean;
  isStarred: boolean;
  state: "active" | "archived" | "gone";
  attachments: AttachmentRow[];
};

export type ThreadDetail = {
  thread: ThreadRow;
  imagePolicy: ImagePolicy;
  messages: MessageRow[];
};
