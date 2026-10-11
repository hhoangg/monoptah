//! Mail storage: schema and CRUD for credentials, sources, mailboxes,
//! identities and sync bookkeeping. No network, MIME or threading here.
//!
//! Secrets never enter this module. A credential row carries only a masked
//! access key id; the key pair itself lives in the OS keychain.
//!
//! Bookkeeping timestamps (`created_at`, `updated_at`, `refreshed_at`) are epoch
//! milliseconds like the rest of the database; `date_utc` stays in seconds.
// The Tauri command layer that calls these functions arrives in a later task.
#![allow(dead_code)]

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::session_store::now_millis;

const MASK_BULLETS: &str = "\u{2022}\u{2022}\u{2022}\u{2022}";
const MASK_VISIBLE_PREFIX: usize = 4;
const MASK_VISIBLE_SUFFIX: usize = 4;

pub(crate) fn ensure_tables(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS mail_credential (
          id             INTEGER PRIMARY KEY,
          provider       TEXT NOT NULL,
          label          TEXT NOT NULL,
          access_key_masked TEXT NOT NULL,
          default_region TEXT,
          created_at     INTEGER NOT NULL,
          updated_at     INTEGER NOT NULL,
          UNIQUE (provider, label)
        );

        CREATE TABLE IF NOT EXISTS mail_account (
          id             INTEGER PRIMARY KEY,
          provider       TEXT NOT NULL,
          credential_id  INTEGER NOT NULL REFERENCES mail_credential(id) ON DELETE RESTRICT,
          s3_credential_id INTEGER REFERENCES mail_credential(id) ON DELETE RESTRICT,
          label          TEXT NOT NULL,
          ses_region     TEXT NOT NULL,
          bucket         TEXT NOT NULL,
          s3_region      TEXT NOT NULL,
          default_from   TEXT,
          attach_policy  TEXT NOT NULL DEFAULT 'on_demand',
          image_policy   TEXT NOT NULL DEFAULT 'ask',
          enabled        INTEGER NOT NULL DEFAULT 1,
          sort_order     INTEGER NOT NULL DEFAULT 0,
          identities_refreshed_at INTEGER,
          created_at     INTEGER NOT NULL,
          updated_at     INTEGER NOT NULL,
          UNIQUE (credential_id, ses_region, bucket, s3_region)
        );

        CREATE TABLE IF NOT EXISTS mail_identity (
          account_id   INTEGER NOT NULL REFERENCES mail_account(id) ON DELETE CASCADE,
          identity     TEXT NOT NULL,
          kind         TEXT NOT NULL,
          status       TEXT NOT NULL,
          refreshed_at INTEGER NOT NULL,
          PRIMARY KEY (account_id, identity)
        );

        CREATE TABLE IF NOT EXISTS mail_mailbox (
          id           INTEGER PRIMARY KEY,
          account_id   INTEGER NOT NULL REFERENCES mail_account(id) ON DELETE CASCADE,
          address      TEXT NOT NULL,
          domain       TEXT NOT NULL,
          s3_prefix    TEXT NOT NULL,
          display_name TEXT NOT NULL,
          color        TEXT,
          sort_order   INTEGER NOT NULL DEFAULT 0,
          is_sendable  INTEGER NOT NULL DEFAULT 1,
          UNIQUE (address),
          UNIQUE (account_id, s3_prefix)
        );
        CREATE INDEX IF NOT EXISTS mail_mailbox_domain ON mail_mailbox(domain, sort_order);

        CREATE TABLE IF NOT EXISTS mail_object (
          id           INTEGER PRIMARY KEY,
          mailbox_id   INTEGER NOT NULL REFERENCES mail_mailbox(id) ON DELETE CASCADE,
          s3_key       TEXT NOT NULL,
          etag         TEXT,
          size_bytes   INTEGER,
          s3_modified  INTEGER,
          first_seen_at INTEGER NOT NULL,
          last_seen_at INTEGER NOT NULL,
          state        TEXT NOT NULL,
          parse_error  TEXT,
          UNIQUE (mailbox_id, s3_key)
        );
        CREATE INDEX IF NOT EXISTS mail_object_state ON mail_object(state, mailbox_id);

        CREATE TABLE IF NOT EXISTS mail_thread (
          id               INTEGER PRIMARY KEY,
          account_id       INTEGER NOT NULL REFERENCES mail_account(id) ON DELETE CASCADE,
          mailbox_id       INTEGER NOT NULL REFERENCES mail_mailbox(id) ON DELETE CASCADE,
          subject          TEXT NOT NULL,
          subject_key      TEXT NOT NULL,
          participants_json TEXT NOT NULL,
          first_message_at INTEGER NOT NULL,
          last_message_at  INTEGER NOT NULL,
          message_count    INTEGER NOT NULL DEFAULT 0,
          unread_count     INTEGER NOT NULL DEFAULT 0,
          has_attachments  INTEGER NOT NULL DEFAULT 0,
          is_starred       INTEGER NOT NULL DEFAULT 0,
          state            TEXT NOT NULL DEFAULT 'active',
          archived_at      INTEGER
        );
        CREATE INDEX IF NOT EXISTS mail_thread_box ON mail_thread(mailbox_id, state, last_message_at DESC);
        CREATE INDEX IF NOT EXISTS mail_thread_skey ON mail_thread(mailbox_id, subject_key, last_message_at DESC);

        -- mail_message.state: 'archived' is the user's choice, 'gone' means the
        -- object vanished from S3. A sync may set or clear 'gone' but must never
        -- touch 'archived'.
        CREATE TABLE IF NOT EXISTS mail_message (
          id              INTEGER PRIMARY KEY,
          mailbox_id      INTEGER NOT NULL REFERENCES mail_mailbox(id) ON DELETE CASCADE,
          thread_id       INTEGER NOT NULL REFERENCES mail_thread(id) ON DELETE CASCADE,
          object_id       INTEGER REFERENCES mail_object(id) ON DELETE SET NULL,
          direction       TEXT NOT NULL,
          message_id_hdr  TEXT,
          in_reply_to     TEXT,
          subject         TEXT NOT NULL DEFAULT '',
          subject_key     TEXT NOT NULL DEFAULT '',
          from_name       TEXT,
          from_addr       TEXT NOT NULL DEFAULT '',
          reply_to_addr   TEXT,
          to_json         TEXT NOT NULL DEFAULT '[]',
          cc_json         TEXT NOT NULL DEFAULT '[]',
          bcc_json        TEXT NOT NULL DEFAULT '[]',
          date_utc        INTEGER NOT NULL,
          received_at     INTEGER NOT NULL,
          snippet         TEXT NOT NULL DEFAULT '',
          body_text       TEXT,
          body_html       TEXT,
          body_html_raw_key TEXT,
          has_attachments INTEGER NOT NULL DEFAULT 0,
          size_bytes      INTEGER,
          spf             TEXT, dkim TEXT, dmarc TEXT,
          headers_json    TEXT,
          is_read         INTEGER NOT NULL DEFAULT 0,
          is_starred      INTEGER NOT NULL DEFAULT 0,
          state           TEXT NOT NULL DEFAULT 'active',
          UNIQUE (mailbox_id, message_id_hdr)
        );
        CREATE INDEX IF NOT EXISTS mail_message_thread ON mail_message(thread_id, date_utc);
        CREATE INDEX IF NOT EXISTS mail_message_from ON mail_message(from_addr);

        CREATE TABLE IF NOT EXISTS mail_message_ref (
          message_id   INTEGER NOT NULL REFERENCES mail_message(id) ON DELETE CASCADE,
          ref_hdr      TEXT NOT NULL,
          PRIMARY KEY (message_id, ref_hdr)
        );
        CREATE INDEX IF NOT EXISTS mail_message_ref_hdr ON mail_message_ref(ref_hdr);

        CREATE TABLE IF NOT EXISTS mail_attachment (
          id          INTEGER PRIMARY KEY,
          message_id  INTEGER NOT NULL REFERENCES mail_message(id) ON DELETE CASCADE,
          part_index  INTEGER NOT NULL,
          filename    TEXT NOT NULL,
          mime_type   TEXT NOT NULL,
          size_bytes  INTEGER NOT NULL,
          content_id  TEXT,
          is_inline   INTEGER NOT NULL DEFAULT 0,
          local_path  TEXT,
          UNIQUE (message_id, part_index)
        );

        CREATE TABLE IF NOT EXISTS mail_folder (
          id          INTEGER PRIMARY KEY,
          name        TEXT NOT NULL,
          color       TEXT,
          sort_order  INTEGER NOT NULL DEFAULT 0,
          match_mode  TEXT NOT NULL DEFAULT 'all',
          hide_from_mailbox INTEGER NOT NULL DEFAULT 0,
          count_unread INTEGER NOT NULL DEFAULT 1,
          created_at  INTEGER NOT NULL,
          updated_at  INTEGER NOT NULL,
          UNIQUE (name)
        );

        CREATE TABLE IF NOT EXISTS mail_folder_rule (
          id         INTEGER PRIMARY KEY,
          folder_id  INTEGER NOT NULL REFERENCES mail_folder(id) ON DELETE CASCADE,
          position   INTEGER NOT NULL DEFAULT 0,
          field      TEXT NOT NULL,
          op         TEXT NOT NULL,
          value      TEXT NOT NULL DEFAULT '',
          negate     INTEGER NOT NULL DEFAULT 0
        );

        CREATE TABLE IF NOT EXISTS mail_folder_member (
          folder_id  INTEGER NOT NULL REFERENCES mail_folder(id) ON DELETE CASCADE,
          thread_id  INTEGER NOT NULL REFERENCES mail_thread(id) ON DELETE CASCADE,
          PRIMARY KEY (folder_id, thread_id)
        );
        CREATE INDEX IF NOT EXISTS mail_folder_member_thread ON mail_folder_member(thread_id);

        CREATE TABLE IF NOT EXISTS mail_outbox (
          id            INTEGER PRIMARY KEY,
          account_id    INTEGER NOT NULL REFERENCES mail_account(id) ON DELETE CASCADE,
          mailbox_id    INTEGER REFERENCES mail_mailbox(id) ON DELETE SET NULL,
          thread_id     INTEGER REFERENCES mail_thread(id) ON DELETE SET NULL,
          reply_to_message_id INTEGER REFERENCES mail_message(id) ON DELETE SET NULL,
          kind          TEXT NOT NULL,
          from_addr     TEXT NOT NULL,
          to_json       TEXT NOT NULL,
          cc_json       TEXT NOT NULL DEFAULT '[]',
          bcc_json      TEXT NOT NULL DEFAULT '[]',
          subject       TEXT NOT NULL DEFAULT '',
          body_text     TEXT,
          body_html     TEXT,
          body_json     TEXT,
          quoted_html   TEXT,
          attachments_json TEXT NOT NULL DEFAULT '[]',
          in_reply_to   TEXT,
          references_json TEXT NOT NULL DEFAULT '[]',
          state         TEXT NOT NULL DEFAULT 'draft',
          attempts      INTEGER NOT NULL DEFAULT 0,
          last_error    TEXT,
          ses_message_id TEXT,
          created_at    INTEGER NOT NULL,
          updated_at    INTEGER NOT NULL,
          sent_at       INTEGER
        );

        CREATE TABLE IF NOT EXISTS mail_sync_state (
          mailbox_id        INTEGER PRIMARY KEY REFERENCES mail_mailbox(id) ON DELETE CASCADE,
          last_started_at   INTEGER,
          last_finished_at  INTEGER,
          last_error        TEXT,
          objects_seen      INTEGER NOT NULL DEFAULT 0,
          objects_added     INTEGER NOT NULL DEFAULT 0,
          objects_gone      INTEGER NOT NULL DEFAULT 0
        );

        CREATE VIRTUAL TABLE IF NOT EXISTS mail_message_fts USING fts5 (
          subject, body, people,
          content = '',
          tokenize = "unicode61 remove_diacritics 2"
        );
        "#,
    )
}

/// A user-facing error carried through `rusqlite::Error` so callers keep a
/// single error type. Its `Display` is exactly the message.
fn user_error(message: String) -> rusqlite::Error {
    rusqlite::Error::ToSqlConversionFailure(message.into())
}

/// Masks an access key id for display: first four characters, four bullets,
/// last four characters (`AKIA••••3K7Q`). Ids too short to hide anything
/// behind that shape collapse to bullets only, so a short value never leaks.
pub(crate) fn mask_access_key_id(access_key_id: &str) -> String {
    let chars: Vec<char> = access_key_id.trim().chars().collect();
    if chars.len() < MASK_VISIBLE_PREFIX + MASK_VISIBLE_SUFFIX + 4 {
        return MASK_BULLETS.to_string();
    }
    let prefix: String = chars[..MASK_VISIBLE_PREFIX].iter().collect();
    let suffix: String = chars[chars.len() - MASK_VISIBLE_SUFFIX..].iter().collect();
    format!("{prefix}{MASK_BULLETS}{suffix}")
}

// ── credentials ────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MailCredential {
    pub id: i64,
    pub provider: String,
    pub label: String,
    /// Display only, e.g. `AKIA••••3K7Q`. The secret is never stored here.
    pub access_key_masked: String,
    pub default_region: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CredentialInput {
    pub provider: String,
    pub label: String,
    /// Must already be masked with [`mask_access_key_id`].
    pub access_key_masked: String,
    pub default_region: Option<String>,
}

/// Inserts a credential, or updates the one with the same (provider, label).
pub(crate) fn upsert_credential(
    conn: &Connection,
    input: &CredentialInput,
) -> rusqlite::Result<i64> {
    if !input.access_key_masked.contains('\u{2022}') {
        return Err(user_error(
            "access key id must be masked before it is stored".to_string(),
        ));
    }
    let now = now_millis();
    conn.query_row(
        "INSERT INTO mail_credential
           (provider, label, access_key_masked, default_region, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?5)
         ON CONFLICT (provider, label) DO UPDATE SET
           access_key_masked = excluded.access_key_masked,
           default_region = excluded.default_region,
           updated_at = excluded.updated_at
         RETURNING id",
        params![
            input.provider,
            input.label,
            input.access_key_masked,
            input.default_region,
            now
        ],
        |row| row.get(0),
    )
}

pub(crate) fn list_credentials(conn: &Connection) -> rusqlite::Result<Vec<MailCredential>> {
    let mut stmt = conn.prepare(
        "SELECT id, provider, label, access_key_masked, default_region, created_at, updated_at
         FROM mail_credential ORDER BY provider, label",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(MailCredential {
            id: row.get(0)?,
            provider: row.get(1)?,
            label: row.get(2)?,
            access_key_masked: row.get(3)?,
            default_region: row.get(4)?,
            created_at: row.get(5)?,
            updated_at: row.get(6)?,
        })
    })?;
    rows.collect()
}

/// Deletes a credential nobody uses. A credential still referenced by a source
/// (as its SES or S3 key) is refused with an error that names those sources.
/// The check is explicit so the message is useful; `ON DELETE RESTRICT` backs
/// it up.
pub(crate) fn delete_credential(conn: &Connection, id: i64) -> rusqlite::Result<bool> {
    let mut stmt = conn.prepare(
        "SELECT label FROM mail_account
         WHERE credential_id = ?1 OR s3_credential_id = ?1 ORDER BY label",
    )?;
    let holders: Vec<String> = stmt
        .query_map(params![id], |row| row.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    if !holders.is_empty() {
        return Err(user_error(format!(
            "credential is still used by: {}",
            holders.join(", ")
        )));
    }
    Ok(conn.execute("DELETE FROM mail_credential WHERE id = ?1", params![id])? > 0)
}

// ── accounts (sources) ─────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MailAccount {
    pub id: i64,
    pub provider: String,
    pub credential_id: i64,
    pub s3_credential_id: Option<i64>,
    pub label: String,
    pub ses_region: String,
    pub bucket: String,
    pub s3_region: String,
    pub default_from: Option<String>,
    pub attach_policy: String,
    pub image_policy: String,
    pub enabled: bool,
    pub sort_order: i64,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountInput {
    /// `Some` edits that source in place (including the fields that identify
    /// it). `None` creates one, or updates the source that already points at
    /// the same (credential, SES region, bucket, S3 region).
    pub id: Option<i64>,
    pub provider: String,
    pub credential_id: i64,
    pub s3_credential_id: Option<i64>,
    pub label: String,
    pub ses_region: String,
    pub bucket: String,
    pub s3_region: String,
    pub default_from: Option<String>,
    pub attach_policy: String,
    pub image_policy: String,
    pub enabled: bool,
    pub sort_order: i64,
}

const ACCOUNT_COLUMNS: &str = "id, provider, credential_id, s3_credential_id, label, ses_region, \
     bucket, s3_region, default_from, attach_policy, image_policy, enabled, sort_order, \
     created_at, updated_at";

fn account_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<MailAccount> {
    Ok(MailAccount {
        id: row.get(0)?,
        provider: row.get(1)?,
        credential_id: row.get(2)?,
        s3_credential_id: row.get(3)?,
        label: row.get(4)?,
        ses_region: row.get(5)?,
        bucket: row.get(6)?,
        s3_region: row.get(7)?,
        default_from: row.get(8)?,
        attach_policy: row.get(9)?,
        image_policy: row.get(10)?,
        enabled: row.get::<_, i64>(11)? != 0,
        sort_order: row.get(12)?,
        created_at: row.get(13)?,
        updated_at: row.get(14)?,
    })
}

pub(crate) fn upsert_account(conn: &Connection, input: &AccountInput) -> rusqlite::Result<i64> {
    let now = now_millis();
    if let Some(id) = input.id {
        let changed = conn.execute(
            "UPDATE mail_account SET provider = ?1, credential_id = ?2, s3_credential_id = ?3,
               label = ?4, ses_region = ?5, bucket = ?6, s3_region = ?7, default_from = ?8,
               attach_policy = ?9, image_policy = ?10, enabled = ?11, sort_order = ?12,
               updated_at = ?13
             WHERE id = ?14",
            params![
                input.provider,
                input.credential_id,
                input.s3_credential_id,
                input.label,
                input.ses_region,
                input.bucket,
                input.s3_region,
                input.default_from,
                input.attach_policy,
                input.image_policy,
                input.enabled as i64,
                input.sort_order,
                now,
                id
            ],
        )?;
        if changed == 0 {
            return Err(rusqlite::Error::QueryReturnedNoRows);
        }
        return Ok(id);
    }
    conn.query_row(
        "INSERT INTO mail_account
           (provider, credential_id, s3_credential_id, label, ses_region, bucket, s3_region,
            default_from, attach_policy, image_policy, enabled, sort_order, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?13)
         ON CONFLICT (credential_id, ses_region, bucket, s3_region) DO UPDATE SET
           provider = excluded.provider,
           s3_credential_id = excluded.s3_credential_id,
           label = excluded.label,
           default_from = excluded.default_from,
           attach_policy = excluded.attach_policy,
           image_policy = excluded.image_policy,
           enabled = excluded.enabled,
           sort_order = excluded.sort_order,
           updated_at = excluded.updated_at
         RETURNING id",
        params![
            input.provider,
            input.credential_id,
            input.s3_credential_id,
            input.label,
            input.ses_region,
            input.bucket,
            input.s3_region,
            input.default_from,
            input.attach_policy,
            input.image_policy,
            input.enabled as i64,
            input.sort_order,
            now
        ],
        |row| row.get(0),
    )
}

pub(crate) fn list_accounts(conn: &Connection) -> rusqlite::Result<Vec<MailAccount>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {ACCOUNT_COLUMNS} FROM mail_account ORDER BY sort_order, id"
    ))?;
    let rows = stmt.query_map([], account_from_row)?;
    rows.collect()
}

/// Deletes a source. Its mailboxes, identities, threads, messages and outbox
/// rows go with it (cascade).
pub(crate) fn delete_account(conn: &Connection, id: i64) -> rusqlite::Result<bool> {
    Ok(conn.execute("DELETE FROM mail_account WHERE id = ?1", params![id])? > 0)
}

// ── mailboxes ──────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MailMailbox {
    pub id: i64,
    pub account_id: i64,
    pub address: String,
    pub domain: String,
    pub s3_prefix: String,
    pub display_name: String,
    pub color: Option<String>,
    pub sort_order: i64,
    pub is_sendable: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MailboxInput {
    pub address: String,
    pub s3_prefix: String,
    pub display_name: String,
    pub color: Option<String>,
    pub sort_order: i64,
}

/// Lowercased domain part of an address, or an error if there is none.
fn domain_of(address: &str) -> rusqlite::Result<String> {
    match address.rsplit_once('@') {
        Some((local, domain)) if !local.is_empty() && !domain.is_empty() => {
            Ok(domain.to_ascii_lowercase())
        }
        _ => Err(user_error(format!("'{address}' is not an email address"))),
    }
}

/// Upserts mailboxes under one source, all or nothing. The address is the key
/// and is globally unique: if another source already owns it the call fails and
/// names that source. `is_sendable` is resolved against the cached identities
/// when there are any; before the first identity refresh it keeps its default.
pub(crate) fn upsert_mailboxes(
    conn: &Connection,
    account_id: i64,
    mailboxes: &[MailboxInput],
) -> rusqlite::Result<()> {
    let tx = conn.unchecked_transaction()?;
    for mailbox in mailboxes {
        let address = mailbox.address.trim().to_ascii_lowercase();
        let domain = domain_of(&address)?;
        let owner: Option<(i64, String)> = tx
            .query_row(
                "SELECT a.id, a.label FROM mail_mailbox m
                 JOIN mail_account a ON a.id = m.account_id WHERE m.address = ?1",
                params![address],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        if let Some((owner_id, owner_label)) = owner {
            if owner_id != account_id {
                return Err(user_error(format!(
                    "{address} already belongs to source '{owner_label}'"
                )));
            }
        }
        tx.execute(
            "INSERT INTO mail_mailbox
               (account_id, address, domain, s3_prefix, display_name, color, sort_order)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT (address) DO UPDATE SET
               domain = excluded.domain,
               s3_prefix = excluded.s3_prefix,
               display_name = excluded.display_name,
               color = excluded.color,
               sort_order = excluded.sort_order",
            params![
                account_id,
                address,
                domain,
                mailbox.s3_prefix,
                mailbox.display_name,
                mailbox.color,
                mailbox.sort_order
            ],
        )?;
    }
    // NULL means identities were never fetched, so sendability is unknown. A
    // fetch that returned zero identities is different: nothing is verified.
    let refreshed: bool = tx.query_row(
        "SELECT identities_refreshed_at IS NOT NULL FROM mail_account WHERE id = ?1",
        params![account_id],
        |row| row.get(0),
    )?;
    if refreshed {
        resolve_sendable(&tx, account_id)?;
    }
    tx.commit()
}

/// Lists mailboxes, for one source or for all of them.
pub(crate) fn list_mailboxes(
    conn: &Connection,
    account_id: Option<i64>,
) -> rusqlite::Result<Vec<MailMailbox>> {
    let mut stmt = conn.prepare(
        "SELECT id, account_id, address, domain, s3_prefix, display_name, color, sort_order,
                is_sendable
         FROM mail_mailbox WHERE (?1 IS NULL OR account_id = ?1)
         ORDER BY domain, sort_order, id",
    )?;
    let rows = stmt.query_map(params![account_id], |row| {
        Ok(MailMailbox {
            id: row.get(0)?,
            account_id: row.get(1)?,
            address: row.get(2)?,
            domain: row.get(3)?,
            s3_prefix: row.get(4)?,
            display_name: row.get(5)?,
            color: row.get(6)?,
            sort_order: row.get(7)?,
            is_sendable: row.get::<_, i64>(8)? != 0,
        })
    })?;
    rows.collect()
}

pub(crate) fn delete_mailbox(conn: &Connection, id: i64) -> rusqlite::Result<bool> {
    Ok(conn.execute("DELETE FROM mail_mailbox WHERE id = ?1", params![id])? > 0)
}

// ── identities ─────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MailIdentity {
    pub identity: String,
    /// `domain` or `email`.
    pub kind: String,
    /// `verified`, `pending` or `failed`.
    pub status: String,
}

/// Sets `is_sendable` on a source's mailboxes: true when the address, or its
/// domain, is a verified identity. Callers decide whether identities were ever
/// refreshed (`identities_refreshed_at`); this clears the flag unconditionally.
fn resolve_sendable(conn: &Connection, account_id: i64) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE mail_mailbox SET is_sendable = EXISTS (
           SELECT 1 FROM mail_identity i
           WHERE i.account_id = mail_mailbox.account_id AND i.status = 'verified'
             AND (i.identity = mail_mailbox.address OR i.identity = mail_mailbox.domain))
         WHERE account_id = ?1",
        params![account_id],
    )?;
    Ok(())
}

/// Replaces the cached SES identities of one source, then re-resolves
/// `is_sendable` for its mailboxes: sendable when the address, or its domain,
/// is a verified identity.
pub(crate) fn replace_identities(
    conn: &Connection,
    account_id: i64,
    identities: &[MailIdentity],
) -> rusqlite::Result<()> {
    let now = now_millis();
    let tx = conn.unchecked_transaction()?;
    tx.execute(
        "DELETE FROM mail_identity WHERE account_id = ?1",
        params![account_id],
    )?;
    for identity in identities {
        tx.execute(
            "INSERT INTO mail_identity (account_id, identity, kind, status, refreshed_at)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT (account_id, identity) DO UPDATE SET
               kind = excluded.kind, status = excluded.status,
               refreshed_at = excluded.refreshed_at",
            params![
                account_id,
                identity.identity.to_ascii_lowercase(),
                identity.kind,
                identity.status,
                now
            ],
        )?;
    }
    tx.execute(
        "UPDATE mail_account SET identities_refreshed_at = ?1 WHERE id = ?2",
        params![now, account_id],
    )?;
    resolve_sendable(&tx, account_id)?;
    tx.commit()
}

pub(crate) fn list_identities(
    conn: &Connection,
    account_id: i64,
) -> rusqlite::Result<Vec<MailIdentity>> {
    let mut stmt = conn.prepare(
        "SELECT identity, kind, status FROM mail_identity WHERE account_id = ?1 ORDER BY identity",
    )?;
    let rows = stmt.query_map(params![account_id], |row| {
        Ok(MailIdentity {
            identity: row.get(0)?,
            kind: row.get(1)?,
            status: row.get(2)?,
        })
    })?;
    rows.collect()
}

// ── sync state ─────────────────────────────────────────────────────

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MailSyncState {
    pub mailbox_id: i64,
    pub last_started_at: Option<i64>,
    pub last_finished_at: Option<i64>,
    pub last_error: Option<String>,
    pub objects_seen: i64,
    pub objects_added: i64,
    pub objects_gone: i64,
}

pub(crate) fn get_sync_state(
    conn: &Connection,
    mailbox_id: i64,
) -> rusqlite::Result<Option<MailSyncState>> {
    conn.query_row(
        "SELECT mailbox_id, last_started_at, last_finished_at, last_error,
                objects_seen, objects_added, objects_gone
         FROM mail_sync_state WHERE mailbox_id = ?1",
        params![mailbox_id],
        |row| {
            Ok(MailSyncState {
                mailbox_id: row.get(0)?,
                last_started_at: row.get(1)?,
                last_finished_at: row.get(2)?,
                last_error: row.get(3)?,
                objects_seen: row.get(4)?,
                objects_added: row.get(5)?,
                objects_gone: row.get(6)?,
            })
        },
    )
    .optional()
}

pub(crate) fn set_sync_state(conn: &Connection, state: &MailSyncState) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO mail_sync_state
           (mailbox_id, last_started_at, last_finished_at, last_error,
            objects_seen, objects_added, objects_gone)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
         ON CONFLICT (mailbox_id) DO UPDATE SET
           last_started_at = excluded.last_started_at,
           last_finished_at = excluded.last_finished_at,
           last_error = excluded.last_error,
           objects_seen = excluded.objects_seen,
           objects_added = excluded.objects_added,
           objects_gone = excluded.objects_gone",
        params![
            state.mailbox_id,
            state.last_started_at,
            state.last_finished_at,
            state.last_error,
            state.objects_seen,
            state.objects_added,
            state.objects_gone
        ],
    )?;
    Ok(())
}

// ── threads and messages ───────────────────────────────────────────

pub(crate) struct NewMessage<'a> {
    pub mailbox_id: i64,
    pub thread_id: i64,
    pub object_id: Option<i64>,
    pub direction: &'a str,
    pub message_id_hdr: Option<&'a str>,
    pub in_reply_to: Option<&'a str>,
    pub subject: &'a str,
    pub subject_key: &'a str,
    pub from_name: Option<&'a str>,
    pub from_addr: &'a str,
    pub reply_to_addr: Option<&'a str>,
    pub to_json: &'a str,
    pub cc_json: &'a str,
    pub bcc_json: &'a str,
    /// Epoch seconds.
    pub date_utc: i64,
    /// Epoch seconds.
    pub received_at: i64,
    pub snippet: &'a str,
    pub body_text: Option<&'a str>,
    /// Already sanitised.
    pub body_html: Option<&'a str>,
    pub has_attachments: bool,
    pub size_bytes: i64,
    pub spf: Option<&'a str>,
    pub dkim: Option<&'a str>,
    pub dmarc: Option<&'a str>,
    pub headers_json: &'a str,
}

pub(crate) struct NewAttachment<'a> {
    pub part_index: i64,
    pub filename: &'a str,
    pub mime_type: &'a str,
    pub size_bytes: i64,
    pub content_id: Option<&'a str>,
    pub is_inline: bool,
}

/// One message as the thread aggregates need it.
pub(crate) struct ThreadMessageRow {
    pub subject: String,
    pub subject_key: String,
    pub from_name: Option<String>,
    pub from_addr: String,
    pub reply_to_addr: Option<String>,
    pub to_json: String,
    pub cc_json: String,
    pub date_utc: i64,
    pub is_read: bool,
    pub has_attachments: bool,
}

pub(crate) struct ThreadAggregates {
    pub subject: String,
    pub subject_key: String,
    pub participants_json: String,
    pub first_message_at: i64,
    pub last_message_at: i64,
    pub message_count: i64,
    pub unread_count: i64,
    pub has_attachments: bool,
}

pub(crate) fn mailbox_account_id(conn: &Connection, mailbox_id: i64) -> rusqlite::Result<i64> {
    conn.query_row(
        "SELECT account_id FROM mail_mailbox WHERE id = ?1",
        params![mailbox_id],
        |row| row.get(0),
    )
}

/// Addresses of every mailbox under the source, lowercased.
pub(crate) fn account_mailbox_addresses(
    conn: &Connection,
    account_id: i64,
) -> rusqlite::Result<Vec<String>> {
    let mut stmt = conn.prepare("SELECT address FROM mail_mailbox WHERE account_id = ?1")?;
    let rows = stmt.query_map(params![account_id], |row| row.get::<_, String>(0))?;
    rows.map(|r| r.map(|a| a.to_ascii_lowercase())).collect()
}

/// `(message id, thread id)` of a stored message with this `Message-ID`.
pub(crate) fn find_message_by_header(
    conn: &Connection,
    mailbox_id: i64,
    message_id_hdr: &str,
) -> rusqlite::Result<Option<(i64, i64)>> {
    conn.query_row(
        "SELECT id, thread_id FROM mail_message
         WHERE mailbox_id = ?1 AND message_id_hdr = ?2",
        params![mailbox_id, message_id_hdr],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )
    .optional()
}

/// Threads in this mailbox that already hold a message related to the new one,
/// ascending. A stored message relates when it is one of `refs`, when it
/// references the new message's own id, or when it references any of `refs`
/// (siblings whose shared parent has not arrived yet).
pub(crate) fn threads_for_refs(
    conn: &Connection,
    mailbox_id: i64,
    own_hdr: Option<&str>,
    refs: &[String],
) -> rusqlite::Result<Vec<i64>> {
    let mut found = std::collections::BTreeSet::new();
    let mut by_hdr = conn.prepare(
        "SELECT thread_id FROM mail_message WHERE mailbox_id = ?1 AND message_id_hdr = ?2",
    )?;
    let mut by_ref = conn.prepare(
        "SELECT DISTINCT m.thread_id FROM mail_message_ref r
         JOIN mail_message m ON m.id = r.message_id
         WHERE m.mailbox_id = ?1 AND r.ref_hdr = ?2",
    )?;
    for hdr in refs {
        for id in by_hdr.query_map(params![mailbox_id, hdr], |row| row.get::<_, i64>(0))? {
            found.insert(id?);
        }
        for id in by_ref.query_map(params![mailbox_id, hdr], |row| row.get::<_, i64>(0))? {
            found.insert(id?);
        }
    }
    if let Some(own) = own_hdr {
        for id in by_ref.query_map(params![mailbox_id, own], |row| row.get::<_, i64>(0))? {
            found.insert(id?);
        }
    }
    Ok(found.into_iter().collect())
}

/// `(thread id, participants_json)` of threads with this key whose last
/// message is at or after `since`, newest first.
pub(crate) fn subject_thread_candidates(
    conn: &Connection,
    mailbox_id: i64,
    subject_key: &str,
    since: i64,
) -> rusqlite::Result<Vec<(i64, String)>> {
    let mut stmt = conn.prepare(
        "SELECT id, participants_json FROM mail_thread
         WHERE mailbox_id = ?1 AND subject_key = ?2 AND last_message_at >= ?3
         ORDER BY last_message_at DESC, id DESC",
    )?;
    let rows = stmt.query_map(params![mailbox_id, subject_key, since], |row| {
        Ok((row.get(0)?, row.get(1)?))
    })?;
    rows.collect()
}

/// Creates an empty thread; the caller fills the aggregates after inserting
/// the first message.
pub(crate) fn insert_thread(
    conn: &Connection,
    mailbox_id: i64,
    subject: &str,
    subject_key: &str,
    first_message_at: i64,
) -> rusqlite::Result<i64> {
    let account_id = mailbox_account_id(conn, mailbox_id)?;
    conn.execute(
        "INSERT INTO mail_thread
           (account_id, mailbox_id, subject, subject_key, participants_json,
            first_message_at, last_message_at)
         VALUES (?1, ?2, ?3, ?4, '[]', ?5, ?5)",
        params![
            account_id,
            mailbox_id,
            subject,
            subject_key,
            first_message_at
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

pub(crate) fn insert_message(conn: &Connection, m: &NewMessage) -> rusqlite::Result<i64> {
    conn.execute(
        "INSERT INTO mail_message
           (mailbox_id, thread_id, object_id, direction, message_id_hdr, in_reply_to,
            subject, subject_key, from_name, from_addr, reply_to_addr,
            to_json, cc_json, bcc_json, date_utc, received_at, snippet,
            body_text, body_html, has_attachments, size_bytes, spf, dkim, dmarc,
            headers_json)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15,
                 ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25)",
        params![
            m.mailbox_id,
            m.thread_id,
            m.object_id,
            m.direction,
            m.message_id_hdr,
            m.in_reply_to,
            m.subject,
            m.subject_key,
            m.from_name,
            m.from_addr,
            m.reply_to_addr,
            m.to_json,
            m.cc_json,
            m.bcc_json,
            m.date_utc,
            m.received_at,
            m.snippet,
            m.body_text,
            m.body_html,
            m.has_attachments,
            m.size_bytes,
            m.spf,
            m.dkim,
            m.dmarc,
            m.headers_json,
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

pub(crate) fn insert_message_refs(
    conn: &Connection,
    message_id: i64,
    refs: &[String],
) -> rusqlite::Result<()> {
    let mut stmt = conn
        .prepare("INSERT OR IGNORE INTO mail_message_ref (message_id, ref_hdr) VALUES (?1, ?2)")?;
    for hdr in refs {
        stmt.execute(params![message_id, hdr])?;
    }
    Ok(())
}

pub(crate) fn insert_attachments(
    conn: &Connection,
    message_id: i64,
    attachments: &[NewAttachment],
) -> rusqlite::Result<()> {
    let mut stmt = conn.prepare(
        "INSERT INTO mail_attachment
           (message_id, part_index, filename, mime_type, size_bytes, content_id, is_inline)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
    )?;
    for a in attachments {
        stmt.execute(params![
            message_id,
            a.part_index,
            a.filename,
            a.mime_type,
            a.size_bytes,
            a.content_id,
            a.is_inline,
        ])?;
    }
    Ok(())
}

/// Messages of a thread, oldest first.
pub(crate) fn thread_message_rows(
    conn: &Connection,
    thread_id: i64,
) -> rusqlite::Result<Vec<ThreadMessageRow>> {
    let mut stmt = conn.prepare(
        "SELECT subject, subject_key, from_name, from_addr, reply_to_addr, to_json,
                cc_json, date_utc, is_read, has_attachments
         FROM mail_message WHERE thread_id = ?1 ORDER BY date_utc, id",
    )?;
    let rows = stmt.query_map(params![thread_id], |row| {
        Ok(ThreadMessageRow {
            subject: row.get(0)?,
            subject_key: row.get(1)?,
            from_name: row.get(2)?,
            from_addr: row.get(3)?,
            reply_to_addr: row.get(4)?,
            to_json: row.get(5)?,
            cc_json: row.get(6)?,
            date_utc: row.get(7)?,
            is_read: row.get(8)?,
            has_attachments: row.get(9)?,
        })
    })?;
    rows.collect()
}

pub(crate) fn update_thread_aggregates(
    conn: &Connection,
    thread_id: i64,
    a: &ThreadAggregates,
) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE mail_thread SET subject = ?2, subject_key = ?3, participants_json = ?4,
           first_message_at = ?5, last_message_at = ?6, message_count = ?7,
           unread_count = ?8, has_attachments = ?9
         WHERE id = ?1",
        params![
            thread_id,
            a.subject,
            a.subject_key,
            a.participants_json,
            a.first_message_at,
            a.last_message_at,
            a.message_count,
            a.unread_count,
            a.has_attachments,
        ],
    )?;
    Ok(())
}

/// Moves everything hanging off `loser` onto `survivor` and deletes `loser`.
/// A star on the loser carries over; folder rows the survivor already has are
/// dropped with the loser by cascade. Aggregates are the caller's to refresh.
pub(crate) fn absorb_thread(conn: &Connection, survivor: i64, loser: i64) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE mail_message SET thread_id = ?1 WHERE thread_id = ?2",
        params![survivor, loser],
    )?;
    conn.execute(
        "UPDATE OR IGNORE mail_folder_member SET thread_id = ?1 WHERE thread_id = ?2",
        params![survivor, loser],
    )?;
    conn.execute(
        "UPDATE mail_outbox SET thread_id = ?1 WHERE thread_id = ?2",
        params![survivor, loser],
    )?;
    conn.execute(
        "UPDATE mail_thread SET is_starred = 1
         WHERE id = ?1 AND (SELECT is_starred FROM mail_thread WHERE id = ?2) = 1",
        params![survivor, loser],
    )?;
    conn.execute("DELETE FROM mail_thread WHERE id = ?1", params![loser])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session_store::SessionStore;

    const KEY_ID: &str = "AKIAIOSFODNN7EXAMPLE";

    fn credential(label: &str) -> CredentialInput {
        CredentialInput {
            provider: "aws".into(),
            label: label.into(),
            access_key_masked: mask_access_key_id(KEY_ID),
            default_region: Some("us-east-1".into()),
        }
    }

    fn account(credential_id: i64, label: &str, bucket: &str) -> AccountInput {
        AccountInput {
            id: None,
            provider: "aws".into(),
            credential_id,
            s3_credential_id: None,
            label: label.into(),
            ses_region: "us-east-1".into(),
            bucket: bucket.into(),
            s3_region: "us-east-1".into(),
            default_from: None,
            attach_policy: "on_demand".into(),
            image_policy: "ask".into(),
            enabled: true,
            sort_order: 0,
        }
    }

    fn mailbox(address: &str, prefix: &str) -> MailboxInput {
        MailboxInput {
            address: address.into(),
            s3_prefix: prefix.into(),
            display_name: "Owner".into(),
            color: None,
            sort_order: 0,
        }
    }

    fn table_count(conn: &Connection) -> i64 {
        conn.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE name LIKE 'mail_%'",
            [],
            |row| row.get(0),
        )
        .unwrap()
    }

    #[test]
    fn ensure_tables_is_idempotent() {
        let store = SessionStore::open_in_memory().unwrap();
        let conn = store.lock_conn().unwrap();
        let before = table_count(&conn);
        assert!(before > 0);
        ensure_tables(&conn).unwrap();
        assert_eq!(table_count(&conn), before);
    }

    #[test]
    fn fts_matches_without_diacritics() {
        let store = SessionStore::open_in_memory().unwrap();
        let conn = store.lock_conn().unwrap();
        conn.execute(
            "INSERT INTO mail_message_fts (rowid, subject, body, people) VALUES (7, ?1, ?2, ?3)",
            params!["Hello", "Xin chao", "Hoàng <h@example.com>"],
        )
        .unwrap();
        let hit: i64 = conn
            .query_row(
                "SELECT rowid FROM mail_message_fts WHERE mail_message_fts MATCH 'hoang'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(hit, 7);
    }

    #[test]
    fn duplicate_source_is_merged_by_upsert_and_rejected_by_raw_insert() {
        let store = SessionStore::open_in_memory().unwrap();
        let conn = store.lock_conn().unwrap();
        let cred = upsert_credential(&conn, &credential("personal")).unwrap();
        let a = upsert_account(&conn, &account(cred, "one", "bucket-a")).unwrap();
        let b = upsert_account(&conn, &account(cred, "two", "bucket-b")).unwrap();
        assert_ne!(a, b);
        assert_eq!(list_accounts(&conn).unwrap().len(), 2);

        // Same identifying tuple: the upsert updates the existing source.
        let again = upsert_account(&conn, &account(cred, "renamed", "bucket-a")).unwrap();
        assert_eq!(again, a);
        assert_eq!(list_accounts(&conn).unwrap().len(), 2);

        // The database constraint itself rejects a second row.
        let raw = conn.execute(
            "INSERT INTO mail_account (provider, credential_id, label, ses_region, bucket,
               s3_region, created_at, updated_at)
             VALUES ('aws', ?1, 'dup', 'us-east-1', 'bucket-a', 'us-east-1', 0, 0)",
            params![cred],
        );
        assert!(raw.is_err());

        // Editing a source into another one's identity is rejected.
        let mut clash = account(cred, "two", "bucket-a");
        clash.id = Some(b);
        assert!(upsert_account(&conn, &clash).is_err());
    }

    #[test]
    fn mailbox_address_is_unique_across_sources() {
        let store = SessionStore::open_in_memory().unwrap();
        let conn = store.lock_conn().unwrap();
        let cred = upsert_credential(&conn, &credential("personal")).unwrap();
        let a = upsert_account(&conn, &account(cred, "Alpha mail", "bucket-a")).unwrap();
        let b = upsert_account(&conn, &account(cred, "Beta mail", "bucket-b")).unwrap();
        upsert_mailboxes(&conn, a, &[mailbox("Owner@Example.com", "owner/")]).unwrap();

        let err = upsert_mailboxes(&conn, b, &[mailbox("owner@example.com", "x/")]).unwrap_err();
        assert!(err.to_string().contains("Alpha mail"), "{err}");

        // The owner may re-save it, and the failed call left nothing behind.
        upsert_mailboxes(&conn, a, &[mailbox("owner@example.com", "owner2/")]).unwrap();
        let all = list_mailboxes(&conn, None).unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].domain, "example.com");
        assert_eq!(all[0].address, "owner@example.com");
        assert_eq!(all[0].s3_prefix, "owner2/");
        assert!(list_mailboxes(&conn, Some(b)).unwrap().is_empty());
    }

    #[test]
    fn mailbox_batch_is_all_or_nothing() {
        let store = SessionStore::open_in_memory().unwrap();
        let conn = store.lock_conn().unwrap();
        let cred = upsert_credential(&conn, &credential("personal")).unwrap();
        let a = upsert_account(&conn, &account(cred, "Alpha", "bucket-a")).unwrap();
        let b = upsert_account(&conn, &account(cred, "Beta", "bucket-b")).unwrap();
        upsert_mailboxes(&conn, a, &[mailbox("taken@example.com", "t/")]).unwrap();
        let result = upsert_mailboxes(
            &conn,
            b,
            &[
                mailbox("fresh@example.com", "f/"),
                mailbox("taken@example.com", "t/"),
            ],
        );
        assert!(result.is_err());
        assert!(list_mailboxes(&conn, Some(b)).unwrap().is_empty());
    }

    #[test]
    fn credential_in_use_cannot_be_deleted() {
        let store = SessionStore::open_in_memory().unwrap();
        let conn = store.lock_conn().unwrap();
        let used = upsert_credential(&conn, &credential("used")).unwrap();
        let s3_only = upsert_credential(&conn, &credential("s3-only")).unwrap();
        let free = upsert_credential(&conn, &credential("free")).unwrap();
        let mut input = account(used, "Alpha mail", "bucket-a");
        input.s3_credential_id = Some(s3_only);
        let account_id = upsert_account(&conn, &input).unwrap();

        let err = delete_credential(&conn, used).unwrap_err();
        assert!(err.to_string().contains("Alpha mail"), "{err}");
        let err = delete_credential(&conn, s3_only).unwrap_err();
        assert!(err.to_string().contains("Alpha mail"), "{err}");
        assert!(delete_credential(&conn, free).unwrap());
        assert_eq!(list_credentials(&conn).unwrap().len(), 2);

        // The database itself also refuses (foreign keys are on).
        assert!(conn
            .execute("DELETE FROM mail_credential WHERE id = ?1", params![used])
            .is_err());

        assert!(delete_account(&conn, account_id).unwrap());
        assert!(delete_credential(&conn, used).unwrap());
    }

    #[test]
    fn masking_shape_and_no_secret_in_rows() {
        assert_eq!(
            mask_access_key_id(KEY_ID),
            "AKIA\u{2022}\u{2022}\u{2022}\u{2022}MPLE"
        );
        assert_eq!(
            mask_access_key_id("short"),
            "\u{2022}\u{2022}\u{2022}\u{2022}"
        );

        let store = SessionStore::open_in_memory().unwrap();
        let conn = store.lock_conn().unwrap();
        upsert_credential(&conn, &credential("personal")).unwrap();
        let listed = format!("{:?}", list_credentials(&conn).unwrap());
        assert!(listed.contains("AKIA\u{2022}\u{2022}\u{2022}\u{2022}MPLE"));
        assert!(!listed.contains(KEY_ID));

        // An unmasked value is refused before it reaches the table.
        let mut bad = credential("raw");
        bad.access_key_masked = KEY_ID.into();
        assert!(upsert_credential(&conn, &bad).is_err());
    }

    #[test]
    fn credential_upsert_keeps_id_and_created_at() {
        let store = SessionStore::open_in_memory().unwrap();
        let conn = store.lock_conn().unwrap();
        let first = upsert_credential(&conn, &credential("personal")).unwrap();
        let mut changed = credential("personal");
        changed.default_region = Some("eu-west-1".into());
        let second = upsert_credential(&conn, &changed).unwrap();
        assert_eq!(first, second);
        let rows = list_credentials(&conn).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].default_region.as_deref(), Some("eu-west-1"));
    }

    #[test]
    fn identities_replace_and_resolve_sendable() {
        let store = SessionStore::open_in_memory().unwrap();
        let conn = store.lock_conn().unwrap();
        let cred = upsert_credential(&conn, &credential("personal")).unwrap();
        let a = upsert_account(&conn, &account(cred, "Alpha", "bucket-a")).unwrap();
        upsert_mailboxes(
            &conn,
            a,
            &[
                mailbox("owner@example.com", "o/"),
                mailbox("sales@other.org", "s/"),
            ],
        )
        .unwrap();
        let verified = |identity: &str, kind: &str| MailIdentity {
            identity: identity.into(),
            kind: kind.into(),
            status: "verified".into(),
        };
        replace_identities(&conn, a, &[verified("example.com", "domain")]).unwrap();
        let boxes = list_mailboxes(&conn, Some(a)).unwrap();
        let sendable = |addr: &str| {
            boxes
                .iter()
                .find(|m| m.address == addr)
                .unwrap()
                .is_sendable
        };
        assert!(sendable("owner@example.com"));
        assert!(!sendable("sales@other.org"));

        replace_identities(&conn, a, &[verified("sales@other.org", "email")]).unwrap();
        assert_eq!(list_identities(&conn, a).unwrap().len(), 1);
        let boxes = list_mailboxes(&conn, Some(a)).unwrap();
        assert!(
            !boxes
                .iter()
                .find(|m| m.address == "owner@example.com")
                .unwrap()
                .is_sendable
        );
        assert!(
            boxes
                .iter()
                .find(|m| m.address == "sales@other.org")
                .unwrap()
                .is_sendable
        );
    }

    #[test]
    fn new_mailbox_is_resolved_against_cached_identities() {
        let store = SessionStore::open_in_memory().unwrap();
        let conn = store.lock_conn().unwrap();
        let cred = upsert_credential(&conn, &credential("personal")).unwrap();
        let a = upsert_account(&conn, &account(cred, "Alpha", "bucket-a")).unwrap();

        // Nothing cached yet: the default is kept.
        upsert_mailboxes(&conn, a, &[mailbox("early@example.com", "e/")]).unwrap();
        assert!(list_mailboxes(&conn, Some(a)).unwrap()[0].is_sendable);

        let verified = MailIdentity {
            identity: "example.com".into(),
            kind: "domain".into(),
            status: "verified".into(),
        };
        replace_identities(&conn, a, &[verified]).unwrap();
        upsert_mailboxes(
            &conn,
            a,
            &[
                mailbox("late@example.com", "l/"),
                mailbox("stray@unverified.org", "s/"),
            ],
        )
        .unwrap();
        let boxes = list_mailboxes(&conn, Some(a)).unwrap();
        let sendable = |addr: &str| {
            boxes
                .iter()
                .find(|m| m.address == addr)
                .unwrap()
                .is_sendable
        };
        assert!(sendable("late@example.com"));
        assert!(!sendable("stray@unverified.org"));
    }

    #[test]
    fn empty_identity_refresh_makes_new_mailboxes_unsendable() {
        let store = SessionStore::open_in_memory().unwrap();
        let conn = store.lock_conn().unwrap();
        let cred = upsert_credential(&conn, &credential("personal")).unwrap();
        let a = upsert_account(&conn, &account(cred, "Alpha", "bucket-a")).unwrap();
        upsert_mailboxes(&conn, a, &[mailbox("old@example.com", "o/")]).unwrap();

        replace_identities(&conn, a, &[]).unwrap();
        upsert_mailboxes(&conn, a, &[mailbox("new@example.com", "n/")]).unwrap();
        let boxes = list_mailboxes(&conn, Some(a)).unwrap();
        assert_eq!(boxes.len(), 2);
        assert!(boxes.iter().all(|m| !m.is_sendable));
    }

    #[test]
    fn sync_state_round_trips_and_cascades() {
        let store = SessionStore::open_in_memory().unwrap();
        let conn = store.lock_conn().unwrap();
        let cred = upsert_credential(&conn, &credential("personal")).unwrap();
        let a = upsert_account(&conn, &account(cred, "Alpha", "bucket-a")).unwrap();
        upsert_mailboxes(&conn, a, &[mailbox("owner@example.com", "o/")]).unwrap();
        let mailbox_id = list_mailboxes(&conn, Some(a)).unwrap()[0].id;

        assert_eq!(get_sync_state(&conn, mailbox_id).unwrap(), None);
        let state = MailSyncState {
            mailbox_id,
            last_started_at: Some(10),
            last_finished_at: Some(12),
            last_error: Some("boom".into()),
            objects_seen: 5,
            objects_added: 3,
            objects_gone: 1,
        };
        set_sync_state(&conn, &state).unwrap();
        assert_eq!(
            get_sync_state(&conn, mailbox_id).unwrap(),
            Some(state.clone())
        );
        let next = MailSyncState {
            objects_seen: 9,
            ..state
        };
        set_sync_state(&conn, &next).unwrap();
        assert_eq!(get_sync_state(&conn, mailbox_id).unwrap(), Some(next));

        assert!(delete_mailbox(&conn, mailbox_id).unwrap());
        assert_eq!(get_sync_state(&conn, mailbox_id).unwrap(), None);
    }

    #[test]
    fn folder_names_are_unique() {
        let store = SessionStore::open_in_memory().unwrap();
        let conn = store.lock_conn().unwrap();
        let insert =
            "INSERT INTO mail_folder (name, created_at, updated_at) VALUES ('Invoices', 0, 0)";
        conn.execute(insert, []).unwrap();
        assert!(conn.execute(insert, []).is_err());
    }
}
