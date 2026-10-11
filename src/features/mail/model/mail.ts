import { invoke } from "@tauri-apps/api/core";
import type {
  AccountInput,
  CredentialUpsertInput,
  FolderInput,
  FolderRow,
  IdentityRow,
  MailCredential,
  MailMailbox,
  MailStatus,
  MailboxInput,
  MatchMode,
  OutboxDraft,
  OutboxRow,
  Page,
  Rule,
  SendProblem,
  SendReport,
  SendableMailbox,
  SourceRow,
  SyncReport,
  SyncScope,
  TestReport,
  ThreadDetail,
  ThreadQuery,
  ThreadRow,
} from "./mailTypes";

/** Rust → webview events. */
export const MAIL_SYNC_STATE_EVENT = "mail://sync-state";
export const MAIL_MESSAGES_CHANGED_EVENT = "mail://messages-changed";
/** Not emitted by a save: only sending, re-arming and deleting a row. */
export const MAIL_OUTBOX_CHANGED_EVENT = "mail://outbox-changed";

export const mailStatus = () => invoke<MailStatus>("mail_status");

export const mailCredentials = () =>
  invoke<MailCredential[]>("mail_credentials");

/** Write-only: the secret goes to the keychain and the result is masked. */
export const mailCredentialUpsert = (input: CredentialUpsertInput) =>
  invoke<MailCredential>("mail_credential_upsert", { input });

/** Refused while a source still uses the credential. */
export const mailCredentialDelete = (id: number) =>
  invoke<void>("mail_credential_delete", { id });

export const mailSources = () => invoke<SourceRow[]>("mail_sources");

export const mailSourceUpsert = (source: AccountInput) =>
  invoke<SourceRow>("mail_source_upsert", { source });

export const mailSourceDelete = (id: number) =>
  invoke<void>("mail_source_delete", { id });

export const mailSourceTest = (id: number) =>
  invoke<TestReport>("mail_source_test", { id });

export const mailSourceDetectPrefixes = (id: number) =>
  invoke<string[]>("mail_source_detect_prefixes", { id });

export const mailMailboxesUpsert = (accountId: number, rows: MailboxInput[]) =>
  invoke<MailMailbox[]>("mail_mailboxes_upsert", { accountId, rows });

export const mailMailboxesDelete = (id: number) =>
  invoke<void>("mail_mailboxes_delete", { id });

/**
 * A `mailbox` scope syncs that mailbox's whole source, because the scheduler
 * works per source. It also rewrites the sync state, last-sync time and error
 * badge of every sibling mailbox, so a per-mailbox button must not promise
 * otherwise. Reports are per source.
 */
export const mailSyncNow = (scope: SyncScope) =>
  invoke<SyncReport[]>("mail_sync_now", { scope });

/**
 * Rust cannot read localStorage, so this push is the only way the interval
 * reaches the scheduler; without it the interval stays at the 3600 default
 * whatever the user sets. App pushes it at startup; settings pushes on change.
 */
export const mailSyncIntervalSet = (secs: number) =>
  invoke<void>("mail_sync_interval_set", { secs });

export const mailIdentities = (accountId?: number | null) =>
  invoke<IdentityRow[]>("mail_identities", { accountId: accountId ?? null });

export const mailThreads = (query: ThreadQuery) =>
  invoke<Page<ThreadRow>>("mail_threads", { query });

export const mailThread = (threadId: number) =>
  invoke<ThreadDetail>("mail_thread", { threadId });

/** The raw RFC 822 source, fetched from S3. */
export const mailMessageRaw = (messageId: number) =>
  invoke<string>("mail_message_raw", { messageId });

export const mailMark = (threadIds: number[], read: boolean) =>
  invoke<void>("mail_mark", { threadIds, read });

export const mailStar = (threadIds: number[], starred: boolean) =>
  invoke<void>("mail_star", { threadIds, starred });

/**
 * Puts archived threads back in the inbox, messages included. Archiving is
 * `mailDelete(ids, false)`.
 */
export const mailUnarchive = (threadIds: number[]) =>
  invoke<void>("mail_unarchive", { threadIds });

/** Downloads if needed and returns the local path. */
export const mailAttachmentOpen = (attachmentId: number) =>
  invoke<string>("mail_attachment_open", { attachmentId });

export const mailAttachmentBytes = (attachmentId: number) =>
  invoke<ArrayBuffer>("mail_attachment_bytes", { attachmentId });

/** Only URLs blocked in that message's sanitised HTML are fetched. */
export const mailRemoteImage = (messageId: number, url: string) =>
  invoke<ArrayBuffer>("mail_remote_image", { messageId, url });

export const mailFolders = () => invoke<FolderRow[]>("mail_folders");

/** Saves the folder and re-files all stored mail against its rules. */
export const mailFolderUpsert = (folder: FolderInput, rules: Rule[]) =>
  invoke<FolderRow>("mail_folder_upsert", { folder, rules });

export const mailFolderDelete = (id: number) =>
  invoke<void>("mail_folder_delete", { id });

/** The mode changes the count, so pass the one the list will use. */
export const mailFolderPreview = (rules: Rule[], matchMode: MatchMode) =>
  invoke<{ count: number }>("mail_folder_preview", { rules, matchMode });

/** Creates a draft, or rewrites the one in `draft.id`. Returns its id. */
export const mailOutboxSave = (draft: OutboxDraft) =>
  invoke<number>("mail_outbox_save", { draft });

/**
 * Sends once and never retries. A failure is recorded on the row and thrown
 * as a SendError; when its `retrySafe` is false the message may have been
 * delivered. A send that succeeded resolves even when the sent/ copy or the local record
 * failed, and says so in the report.
 */
export const mailOutboxSend = (id: number) =>
  invoke<SendReport>("mail_outbox_send", { id });

/**
 * `unconfirmed` -> `draft`, nothing else. Put an explicit "I checked the
 * recipient, send again" in front of it: a save never does this.
 */
export const mailOutboxRearm = (id: number) =>
  invoke<void>("mail_outbox_rearm", { id });

/** Problems SES would reject the message for, without sending it. */
export const mailOutboxCheck = (id: number) =>
  invoke<SendProblem[]>("mail_outbox_check", { id });

export const mailOutboxList = () => invoke<OutboxRow[]>("mail_outbox_list");

export const mailOutboxDelete = (id: number) =>
  invoke<void>("mail_outbox_delete", { id });

/** The From picker: verified mailboxes of enabled sources. */
export const mailSendableMailboxes = () =>
  invoke<SendableMailbox[]>("mail_sendable_mailboxes");
