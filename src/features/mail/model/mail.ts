import { invoke } from "@tauri-apps/api/core";
import type {
  AccountInput,
  CredentialUpsertInput,
  IdentityRow,
  MailCredential,
  MailMailbox,
  MailStatus,
  MailboxInput,
  Page,
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

/** Downloads if needed and returns the local path. */
export const mailAttachmentOpen = (attachmentId: number) =>
  invoke<string>("mail_attachment_open", { attachmentId });

export const mailAttachmentBytes = (attachmentId: number) =>
  invoke<ArrayBuffer>("mail_attachment_bytes", { attachmentId });

/** Only URLs blocked in that message's sanitised HTML are fetched. */
export const mailRemoteImage = (messageId: number, url: string) =>
  invoke<ArrayBuffer>("mail_remote_image", { messageId, url });
