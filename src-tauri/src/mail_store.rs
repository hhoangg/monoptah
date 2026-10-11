//! Mail storage: schema and CRUD for credentials, sources, mailboxes,
//! identities, sync bookkeeping, and the thread, message, ref and attachment
//! rows that threading and sync write. No network or MIME parsing here.
//!
//! `message_id_hdr` and `ref_hdr` are always bracket-free (`abc@host`, never
//! `<abc@host>`), as the parser's `trim_id` produces. Any writer, notably
//! outgoing mail that generates its own `<uuid@domain>`, must strip the angle
//! brackets before storing; a bracketed id never matches, so replies to it
//! would silently split into a new thread.
//!
//! Secrets never enter this module. A credential row carries only a masked
//! access key id; the key pair itself lives in the OS keychain.
//!
//! Bookkeeping timestamps (`created_at`, `updated_at`, `refreshed_at`) are epoch
//! milliseconds like the rest of the database; `date_utc` stays in seconds.
// Folder, outbox and delete helpers are consumed by later mail commands.
#![allow(dead_code)]

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::session_store::now_millis;

/// Where sent copies live in a source's bucket. A mailbox with this prefix is
/// the sent archive: the sync reads it so other machines see sent mail, but it
/// is never a From address, whatever the identities say.
pub(crate) const SENT_PREFIX: &str = "sent/";

const MASK_BULLETS: &str = "\u{2022}\u{2022}\u{2022}\u{2022}";
const MASK_VISIBLE_PREFIX: usize = 4;
const MASK_VISIBLE_SUFFIX: usize = 4;

pub(crate) fn ensure_tables(conn: &Connection) -> rusqlite::Result<()> {
    create_tables(conn)?;
    ensure_object_attempts(conn)?;
    ensure_identities_refreshed_at(conn)
}

fn create_tables(conn: &Connection) -> rusqlite::Result<()> {
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
          s3_modified  INTEGER,                -- epoch SECONDS (S3 last-modified)
          first_seen_at INTEGER NOT NULL,      -- epoch MILLISECONDS
          last_seen_at INTEGER NOT NULL,       -- epoch MILLISECONDS
          state        TEXT NOT NULL,
          parse_error  TEXT,
          parse_attempts INTEGER NOT NULL DEFAULT 0,
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

/// `CREATE TABLE IF NOT EXISTS` never adds a column to a table that already
/// exists, so a column added to the DDL after a dev build needs this as well.
fn add_column_if_missing(
    conn: &Connection,
    table: &str,
    column: &str,
    definition: &str,
) -> rusqlite::Result<()> {
    let present: i64 = conn.query_row(
        "SELECT COUNT(*) FROM pragma_table_info(?1) WHERE name = ?2",
        params![table, column],
        |row| row.get(0),
    )?;
    if present == 0 {
        conn.execute(
            &format!("ALTER TABLE {table} ADD COLUMN {column} {definition}"),
            [],
        )?;
    }
    Ok(())
}

/// For databases created before `parse_attempts` was in the DDL.
fn ensure_object_attempts(conn: &Connection) -> rusqlite::Result<()> {
    add_column_if_missing(
        conn,
        "mail_object",
        "parse_attempts",
        "INTEGER NOT NULL DEFAULT 0",
    )
}

/// For databases created before `identities_refreshed_at` was in the DDL.
/// NULL is the right backfill: those databases never fetched identities.
fn ensure_identities_refreshed_at(conn: &Connection) -> rusqlite::Result<()> {
    add_column_if_missing(conn, "mail_account", "identities_refreshed_at", "INTEGER")
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
    check_credential_unused(conn, id)?;
    Ok(conn.execute("DELETE FROM mail_credential WHERE id = ?1", params![id])? > 0)
}

/// Whether the credential exists, erroring (naming the holders) if a source
/// still uses it. Lets a caller refuse before it touches anything else.
pub(crate) fn check_credential_unused(conn: &Connection, id: i64) -> rusqlite::Result<bool> {
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
    conn.query_row(
        "SELECT COUNT(*) FROM mail_credential WHERE id = ?1",
        params![id],
        |row| Ok(row.get::<_, i64>(0)? > 0),
    )
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

/// Forgets the cached SES identities of a source and what they decided. They
/// belong to one (credential, SES region) pair, so after either changes they
/// describe a different account; `is_sendable` returns to its pre-refresh
/// default until the next refresh (the sent archive excepted, see
/// [`resolve_sendable`]).
fn clear_identities(conn: &Connection, account_id: i64) -> rusqlite::Result<()> {
    conn.execute(
        "DELETE FROM mail_identity WHERE account_id = ?1",
        params![account_id],
    )?;
    conn.execute(
        "UPDATE mail_account SET identities_refreshed_at = NULL WHERE id = ?1",
        params![account_id],
    )?;
    conn.execute(
        "UPDATE mail_mailbox SET is_sendable = 1 WHERE account_id = ?1 AND s3_prefix <> ?2",
        params![account_id, SENT_PREFIX],
    )?;
    Ok(())
}

pub(crate) fn upsert_account(conn: &Connection, input: &AccountInput) -> rusqlite::Result<i64> {
    let now = now_millis();
    if let Some(id) = input.id {
        let tx = conn.unchecked_transaction()?;
        let previous: (i64, String) = tx.query_row(
            "SELECT credential_id, ses_region FROM mail_account WHERE id = ?1",
            params![id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        tx.execute(
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
        if previous != (input.credential_id, input.ses_region.clone()) {
            clear_identities(&tx, id)?;
        }
        tx.commit()?;
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

/// Ids of every message stored under a source, for cleaning up what lives
/// outside SQLite (attachment files) after the rows are gone.
pub(crate) fn message_ids_in_account(
    conn: &Connection,
    account_id: i64,
) -> rusqlite::Result<Vec<i64>> {
    let mut stmt = conn.prepare(
        "SELECT m.id FROM mail_message m JOIN mail_mailbox b ON b.id = m.mailbox_id
         WHERE b.account_id = ?1",
    )?;
    let rows = stmt.query_map(params![account_id], |row| row.get(0))?;
    rows.collect()
}

pub(crate) fn message_ids_in_mailbox(
    conn: &Connection,
    mailbox_id: i64,
) -> rusqlite::Result<Vec<i64>> {
    let mut stmt = conn.prepare("SELECT id FROM mail_message WHERE mailbox_id = ?1")?;
    let rows = stmt.query_map(params![mailbox_id], |row| row.get(0))?;
    rows.collect()
}

/// Deletes a source. Its mailboxes, identities, threads, messages and outbox
/// rows go with it (cascade). Messages are removed one by one first, because
/// the contentless FTS table has no triggers and a cascade would orphan its
/// rows. Run it inside a transaction: a failure part way must not leave a
/// source with half its messages.
pub(crate) fn delete_account(conn: &Connection, id: i64) -> rusqlite::Result<bool> {
    for message_id in message_ids_in_account(conn, id)? {
        remove_message(conn, message_id)?;
    }
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
        let prefix_holder: Option<String> = tx
            .query_row(
                "SELECT address FROM mail_mailbox
                 WHERE account_id = ?1 AND s3_prefix = ?2 AND address != ?3",
                params![account_id, mailbox.s3_prefix, address],
                |row| row.get(0),
            )
            .optional()?;
        if let Some(holder) = prefix_holder {
            return Err(user_error(format!(
                "prefix '{}' is already used by {holder}",
                mailbox.s3_prefix
            )));
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

/// Same contract as [`delete_account`]: messages go through `remove_message`
/// first, and the caller supplies the transaction.
pub(crate) fn delete_mailbox(conn: &Connection, id: i64) -> rusqlite::Result<bool> {
    for message_id in message_ids_in_mailbox(conn, id)? {
        remove_message(conn, message_id)?;
    }
    Ok(conn.execute("DELETE FROM mail_mailbox WHERE id = ?1", params![id])? > 0)
}

impl MailMailbox {
    pub(crate) fn is_sent_archive(&self) -> bool {
        self.s3_prefix == SENT_PREFIX
    }
}

/// Mailboxes a person set up, without the sent archive. The sync uses
/// [`list_mailboxes`] because it must read the archive too.
pub(crate) fn list_user_mailboxes(
    conn: &Connection,
    account_id: Option<i64>,
) -> rusqlite::Result<Vec<MailMailbox>> {
    Ok(list_mailboxes(conn, account_id)?
        .into_iter()
        .filter(|m| !m.is_sent_archive())
        .collect())
}

/// Registers the source's `sent/` prefix as a mailbox the first time a copy is
/// written there, so the normal sync reads it. It is created unsendable and
/// [`resolve_sendable`] never touches it. The address is a placeholder that
/// cannot receive mail: `address` must be unique across sources.
pub(crate) fn ensure_sent_mailbox(conn: &Connection, account_id: i64) -> rusqlite::Result<i64> {
    let existing: Option<i64> = conn
        .query_row(
            "SELECT id FROM mail_mailbox WHERE account_id = ?1 AND s3_prefix = ?2",
            params![account_id, SENT_PREFIX],
            |row| row.get(0),
        )
        .optional()?;
    if let Some(id) = existing {
        return Ok(id);
    }
    conn.query_row(
        "INSERT INTO mail_mailbox
           (account_id, address, domain, s3_prefix, display_name, sort_order, is_sendable)
         VALUES (?1, ?2, 'monoptah.invalid', ?3, 'Sent', 1000000, 0)
         RETURNING id",
        params![
            account_id,
            format!("sent-archive-{account_id}@monoptah.invalid"),
            SENT_PREFIX
        ],
        |row| row.get(0),
    )
}

/// Where a sent message is filed: the mailbox of the message it answers when
/// the source holds one (so the reply joins its conversation, since threads
/// never cross mailboxes), else the mailbox it was sent from. `referenced` is
/// `In-Reply-To` first, as the parser lists it.
pub(crate) fn filing_mailbox(
    conn: &Connection,
    account_id: i64,
    from_mailbox: Option<i64>,
    referenced: &[String],
) -> rusqlite::Result<Option<i64>> {
    let mut stmt = conn.prepare(
        "SELECT m.mailbox_id FROM mail_message m
         JOIN mail_mailbox b ON b.id = m.mailbox_id
         WHERE b.account_id = ?1 AND m.message_id_hdr = ?2
         ORDER BY (m.mailbox_id = ?3) DESC, m.id LIMIT 1",
    )?;
    for hdr in referenced {
        let found: Option<i64> = stmt
            .query_row(params![account_id, hdr, from_mailbox], |row| row.get(0))
            .optional()?;
        if found.is_some() {
            return Ok(found);
        }
    }
    Ok(from_mailbox)
}

/// The mailbox of the source that owns this address, never the sent archive.
pub(crate) fn mailbox_by_address(
    conn: &Connection,
    address: &str,
) -> rusqlite::Result<Option<MailMailbox>> {
    let address = address.trim().to_ascii_lowercase();
    Ok(list_mailboxes(conn, None)?
        .into_iter()
        .find(|m| !m.is_sent_archive() && m.address == address))
}

pub(crate) fn message_direction(
    conn: &Connection,
    message_id: i64,
) -> rusqlite::Result<Option<String>> {
    conn.query_row(
        "SELECT direction FROM mail_message WHERE id = ?1",
        params![message_id],
        |row| row.get(0),
    )
    .optional()
}

// ── identities ─────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MailIdentity {
    pub identity: String,
    /// `domain` or `email`.
    pub kind: String,
    /// `verified`, `pending`, `failed`, or `sending_disabled` (AWS verified it but
    /// turned sending off, which is not a verification problem).
    pub status: String,
}

/// Sets `is_sendable` on a source's mailboxes: true when the address, or its
/// domain, is a verified identity. Callers decide whether identities were ever
/// refreshed (`identities_refreshed_at`); this clears the flag unconditionally.
///
/// The sent archive is left out: its `is_sendable = 0` is a decision, not a
/// result, so it must not depend on what its placeholder address happens to
/// match. [`clear_identities`] needs the same exclusion for a plainer reason:
/// it resets every mailbox to the sendable default.
fn resolve_sendable(conn: &Connection, account_id: i64) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE mail_mailbox SET is_sendable = EXISTS (
           SELECT 1 FROM mail_identity i
           WHERE i.account_id = mail_mailbox.account_id AND i.status = 'verified'
             AND (i.identity = mail_mailbox.address OR i.identity = mail_mailbox.domain))
         WHERE account_id = ?1 AND s3_prefix <> ?2",
        params![account_id, SENT_PREFIX],
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
    let id = conn.last_insert_rowid();
    let people = fts_people(m.from_name, m.from_addr, m.to_json, m.cc_json);
    fts_insert(conn, id, m.subject, m.body_text.unwrap_or(""), &people)?;
    Ok(id)
}

/// `Name <addr>` of from, to and cc, the text the `people` FTS column holds.
fn fts_people(from_name: Option<&str>, from_addr: &str, to_json: &str, cc_json: &str) -> String {
    let mut out = Vec::new();
    let mut push = |name: Option<&str>, addr: &str| {
        out.push(match name {
            Some(n) if !n.is_empty() => format!("{n} <{addr}>"),
            _ => addr.to_string(),
        });
    };
    if !from_addr.is_empty() {
        push(from_name, from_addr);
    }
    for json in [to_json, cc_json] {
        let list: Vec<serde_json::Value> = serde_json::from_str(json).unwrap_or_default();
        for p in &list {
            if let Some(addr) = p["addr"].as_str() {
                push(p["name"].as_str(), addr);
            }
        }
    }
    out.join(", ")
}

/// The one place text is folded for the index. `remove_diacritics 2` cannot
/// reach the stroke letters (they have no decomposition), and the insert, the
/// `'delete'` replay and every MATCH query must fold identically or the replay
/// stops matching what was indexed.
pub(crate) fn fold_fts(text: &str) -> String {
    text.replace('Đ', "D").replace('đ', "d")
}

// The FTS table is contentless: no triggers, the rowid is the message id, and
// a row can only be removed by replaying the values it was inserted with.
fn fts_insert(
    conn: &Connection,
    id: i64,
    subject: &str,
    body: &str,
    people: &str,
) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO mail_message_fts(rowid, subject, body, people) VALUES (?1, ?2, ?3, ?4)",
        params![id, fold_fts(subject), fold_fts(body), fold_fts(people)],
    )?;
    Ok(())
}

fn fts_delete(
    conn: &Connection,
    id: i64,
    subject: &str,
    body: &str,
    people: &str,
) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO mail_message_fts(mail_message_fts, rowid, subject, body, people)
         VALUES ('delete', ?1, ?2, ?3, ?4)",
        params![id, fold_fts(subject), fold_fts(body), fold_fts(people)],
    )?;
    Ok(())
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

/// Messages of a thread, oldest first. Guarded by the `mail.rs` tests
/// `marking_*` and by `mail_sync`'s vanished-message badge test. `gone` ones are left out: they are not
/// shown, so they must not count, or an unread badge could outlive every
/// message that can be opened to clear it. `archived` stays in, because
/// archiving is a whole-thread state (spec section 10).
pub(crate) fn thread_message_rows(
    conn: &Connection,
    thread_id: i64,
) -> rusqlite::Result<Vec<ThreadMessageRow>> {
    let mut stmt = conn.prepare(
        "SELECT subject, subject_key, from_name, from_addr, reply_to_addr, to_json,
                cc_json, date_utc, is_read, has_attachments
         FROM mail_message WHERE thread_id = ?1 AND state != 'gone' ORDER BY date_utc, id",
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

// ── sync ledger ────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MailObjectRow {
    pub id: i64,
    pub s3_key: String,
    pub etag: Option<String>,
    /// `pending | parsed | gone | error | deleted`
    pub state: String,
    pub parse_attempts: i64,
}

/// What a listing says about one key. `s3_modified` is epoch seconds.
pub(crate) struct ListedObject<'a> {
    pub s3_key: &'a str,
    pub etag: &'a str,
    pub size_bytes: i64,
    pub s3_modified: Option<i64>,
}

pub(crate) fn list_objects(
    conn: &Connection,
    mailbox_id: i64,
) -> rusqlite::Result<Vec<MailObjectRow>> {
    let mut stmt = conn.prepare(
        "SELECT id, s3_key, etag, state, parse_attempts FROM mail_object WHERE mailbox_id = ?1",
    )?;
    let rows = stmt.query_map(params![mailbox_id], |row| {
        Ok(MailObjectRow {
            id: row.get(0)?,
            s3_key: row.get(1)?,
            etag: row.get(2)?,
            state: row.get(3)?,
            parse_attempts: row.get(4)?,
        })
    })?;
    rows.collect()
}

/// Inserts a `pending` object. `seen_at` is epoch milliseconds.
pub(crate) fn insert_object(
    conn: &Connection,
    mailbox_id: i64,
    o: &ListedObject,
    seen_at: i64,
) -> rusqlite::Result<i64> {
    conn.query_row(
        "INSERT INTO mail_object
           (mailbox_id, s3_key, etag, size_bytes, s3_modified, first_seen_at, last_seen_at, state)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6, 'pending')
         RETURNING id",
        params![
            mailbox_id,
            o.s3_key,
            o.etag,
            o.size_bytes,
            o.s3_modified,
            seen_at
        ],
        |row| row.get(0),
    )
}

pub(crate) fn object_by_key(
    conn: &Connection,
    mailbox_id: i64,
    s3_key: &str,
) -> rusqlite::Result<Option<i64>> {
    conn.query_row(
        "SELECT id FROM mail_object WHERE mailbox_id = ?1 AND s3_key = ?2",
        params![mailbox_id, s3_key],
        |row| row.get(0),
    )
    .optional()
}

/// Records that the listing saw this object again, and moves it to `state`.
pub(crate) fn touch_object(
    conn: &Connection,
    id: i64,
    o: &ListedObject,
    seen_at: i64,
    state: &str,
) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE mail_object SET etag = ?2, size_bytes = ?3, s3_modified = ?4,
           last_seen_at = ?5, state = ?6 WHERE id = ?1",
        params![id, o.etag, o.size_bytes, o.s3_modified, seen_at, state],
    )?;
    Ok(())
}

pub(crate) fn set_object_outcome(
    conn: &Connection,
    id: i64,
    state: &str,
    parse_error: Option<&str>,
    parse_attempts: i64,
) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE mail_object SET state = ?2, parse_error = ?3, parse_attempts = ?4 WHERE id = ?1",
        params![id, state, parse_error, parse_attempts],
    )?;
    Ok(())
}

/// `(message id, thread id)` of the message parsed from this object.
pub(crate) fn message_for_object(
    conn: &Connection,
    object_id: i64,
) -> rusqlite::Result<Option<(i64, i64)>> {
    conn.query_row(
        "SELECT id, thread_id FROM mail_message WHERE object_id = ?1",
        params![object_id],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )
    .optional()
}

/// Marks every non-`gone` object of the mailbox not listed since `sync_start`
/// as `gone`. Their `active` messages become `gone` (never `archived`: that
/// word is the user's, spec section 10), and a thread left with no `active`
/// message leaves the inbox. Nothing is deleted. Returns the object count.
/// The badge consequences (aggregates refreshed in the same transaction) are
/// covered end to end by `mail_sync`'s
/// `a_vanished_message_leaves_no_unread_badge_and_a_revived_one_restores_it`.
pub(crate) fn mark_vanished_objects(
    conn: &Connection,
    mailbox_id: i64,
    sync_start: i64,
    archived_at: i64,
) -> rusqlite::Result<usize> {
    let tx = conn;
    let vanished = tx.execute(
        "UPDATE mail_object SET state = 'gone'
         WHERE mailbox_id = ?1 AND last_seen_at < ?2 AND state NOT IN ('gone', 'deleted')",
        params![mailbox_id, sync_start],
    )?;
    if vanished > 0 {
        let threads: Vec<i64> = {
            let mut stmt = tx.prepare(
                "SELECT DISTINCT m.thread_id FROM mail_message m
                 JOIN mail_object o ON o.id = m.object_id
                 WHERE o.mailbox_id = ?1 AND o.state = 'gone' AND m.state = 'active'",
            )?;
            let rows = stmt.query_map(params![mailbox_id], |row| row.get(0))?;
            rows.collect::<rusqlite::Result<_>>()?
        };
        tx.execute(
            "UPDATE mail_message SET state = 'gone'
             WHERE state = 'active' AND object_id IN
               (SELECT id FROM mail_object WHERE mailbox_id = ?1 AND state = 'gone')",
            params![mailbox_id],
        )?;
        for thread_id in threads {
            // The badge must be right the moment the sync ends, not after the
            // next unrelated recompute.
            crate::mail_thread::refresh_thread(tx, thread_id)?;
            reevaluate_thread(tx, thread_id)?;
            tx.execute(
                "UPDATE mail_thread SET state = 'archived', archived_at = ?2
                 WHERE id = ?1 AND state = 'active'
                   AND NOT EXISTS (SELECT 1 FROM mail_message
                                   WHERE thread_id = ?1 AND state = 'active')",
                params![thread_id, archived_at],
            )?;
        }
    }
    Ok(vanished)
}

/// An object that had vanished is listed again: its `gone` message is active
/// again. A message the user archived is not `gone`, so it is left alone, and
/// the thread comes back only if the sync was what removed it, that is when no
/// message in it is `archived`.
/// Returns whether anything was revived.
pub(crate) fn revive_gone_message(conn: &Connection, object_id: i64) -> rusqlite::Result<bool> {
    let tx = conn;
    let thread: Option<i64> = tx
        .query_row(
            "SELECT thread_id FROM mail_message WHERE object_id = ?1 AND state = 'gone'",
            params![object_id],
            |row| row.get(0),
        )
        .optional()?;
    if let Some(thread_id) = thread {
        tx.execute(
            "UPDATE mail_message SET state = 'active' WHERE object_id = ?1 AND state = 'gone'",
            params![object_id],
        )?;
        crate::mail_thread::refresh_thread(tx, thread_id)?;
        reevaluate_thread(tx, thread_id)?;
        tx.execute(
            "UPDATE mail_thread SET state = 'active', archived_at = NULL
             WHERE id = ?1 AND state = 'archived'
               AND NOT EXISTS (SELECT 1 FROM mail_message
                               WHERE thread_id = ?1 AND state = 'archived')",
            params![thread_id],
        )?;
    }
    Ok(thread.is_some())
}

/// A genuinely new message surfaces its thread again, as Gmail does. This is
/// not the re-listed-key case of spec section 10.
pub(crate) fn reactivate_thread(conn: &Connection, thread_id: i64) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE mail_thread SET state = 'active', archived_at = NULL
         WHERE id = ?1 AND state = 'archived'",
        params![thread_id],
    )?;
    Ok(())
}

/// What survives when a message is replaced by a re-parse of its object.
pub(crate) struct RemovedMessage {
    pub thread_id: i64,
    pub object_id: Option<i64>,
    pub direction: String,
    pub is_read: bool,
    pub is_starred: bool,
    pub state: String,
}

/// Deletes a message and its FTS row, returning what the user had set on it.
pub(crate) fn remove_message(conn: &Connection, id: i64) -> rusqlite::Result<RemovedMessage> {
    let (removed, subject, body, people) = conn.query_row(
        "SELECT thread_id, object_id, direction, is_read, is_starred, state,
                subject, body_text, from_name, from_addr, to_json, cc_json
         FROM mail_message WHERE id = ?1",
        params![id],
        |row| {
            let from_name: Option<String> = row.get(8)?;
            let from_addr: String = row.get(9)?;
            let to_json: String = row.get(10)?;
            let cc_json: String = row.get(11)?;
            Ok((
                RemovedMessage {
                    thread_id: row.get(0)?,
                    object_id: row.get(1)?,
                    direction: row.get(2)?,
                    is_read: row.get(3)?,
                    is_starred: row.get(4)?,
                    state: row.get(5)?,
                },
                row.get::<_, String>(6)?,
                row.get::<_, Option<String>>(7)?,
                fts_people(from_name.as_deref(), &from_addr, &to_json, &cc_json),
            ))
        },
    )?;
    fts_delete(conn, id, &subject, body.as_deref().unwrap_or(""), &people)?;
    conn.execute("DELETE FROM mail_message WHERE id = ?1", params![id])?;
    Ok(removed)
}

pub(crate) fn restore_message_flags(
    conn: &Connection,
    id: i64,
    removed: &RemovedMessage,
) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE mail_message SET is_read = ?2, is_starred = ?3, state = ?4 WHERE id = ?1",
        params![id, removed.is_read, removed.is_starred, removed.state],
    )?;
    Ok(())
}

/// Archives the thread if none of its messages is `active`.
pub(crate) fn archive_thread_if_inactive(
    conn: &Connection,
    thread_id: i64,
    archived_at: i64,
) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE mail_thread SET state = 'archived', archived_at = ?2
         WHERE id = ?1 AND state = 'active'
           AND NOT EXISTS (SELECT 1 FROM mail_message WHERE thread_id = ?1 AND state = 'active')",
        params![thread_id, archived_at],
    )?;
    Ok(())
}

/// The user archives whole threads (spec section 10). Only `active` messages
/// move: a `gone` one stays `gone` so that a revive still finds it, and the
/// aggregates are untouched because `archived` messages still count. A thread
/// that is already archived keeps its original `archived_at`.
pub(crate) fn archive_threads(
    conn: &Connection,
    thread_ids: &[i64],
    archived_at: i64,
) -> rusqlite::Result<()> {
    let tx = conn.unchecked_transaction()?;
    for id in thread_ids {
        tx.execute(
            "UPDATE mail_message SET state = 'archived'
             WHERE thread_id = ?1 AND state = 'active'",
            params![id],
        )?;
        tx.execute(
            "UPDATE mail_thread SET state = 'archived', archived_at = ?2
             WHERE id = ?1 AND state = 'active'",
            params![id, archived_at],
        )?;
        // The caller removes the files, so a path left here would claim a
        // download that no longer exists.
        tx.execute(
            "UPDATE mail_attachment SET local_path = NULL
             WHERE message_id IN (SELECT id FROM mail_message WHERE thread_id = ?1)",
            params![id],
        )?;
        reevaluate_thread(&tx, *id)?;
    }
    tx.commit()
}

/// Whether [`unarchive_threads`] would move this thread. The one definition,
/// read by the restore itself and by the reader's "Move to inbox", so the
/// button cannot offer what the write refuses. Only a thread the user archived
/// qualifies: `archived` itself and an `archived` message. One the sync
/// archived (every message `gone`) would come back with nothing to list and an
/// unread count nobody can clear.
pub(crate) fn is_restorable(conn: &Connection, thread_id: i64) -> rusqlite::Result<bool> {
    conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM mail_thread t WHERE t.id = ?1 AND t.state = 'archived'
                          AND EXISTS (SELECT 1 FROM mail_message m
                                      WHERE m.thread_id = t.id AND m.state = 'archived'))",
        params![thread_id],
        |row| row.get(0),
    )
}

/// Puts archived threads back in the inbox. Archiving wrote both tables, so
/// this writes both: `reactivate_thread` alone flips the thread and leaves its
/// messages `archived`, which lists as an empty thread. Returns how many
/// threads moved; a thread [`is_restorable`] refuses is skipped, not an error.
pub(crate) fn unarchive_threads(conn: &Connection, thread_ids: &[i64]) -> rusqlite::Result<usize> {
    let tx = conn.unchecked_transaction()?;
    let mut moved = 0;
    for id in thread_ids {
        if !is_restorable(&tx, *id)? {
            continue;
        }
        tx.execute(
            "UPDATE mail_message SET state = 'active'
             WHERE thread_id = ?1 AND state = 'archived'",
            params![id],
        )?;
        reactivate_thread(&tx, *id)?;
        reevaluate_thread(&tx, *id)?;
        moved += 1;
    }
    tx.commit()?;
    Ok(moved)
}

pub(crate) fn message_ids_in_threads(
    conn: &Connection,
    thread_ids: &[i64],
) -> rusqlite::Result<Vec<i64>> {
    let mut stmt = conn.prepare("SELECT id FROM mail_message WHERE thread_id = ?1")?;
    let mut ids = Vec::new();
    for thread_id in thread_ids {
        let rows = stmt.query_map(params![thread_id], |row| row.get::<_, i64>(0))?;
        for row in rows {
            ids.push(row?);
        }
    }
    Ok(ids)
}

/// A message of a thread and the S3 object behind it, if any.
#[derive(Debug, Clone)]
pub(crate) struct ThreadMessageObject {
    pub message_id: i64,
    pub thread_id: i64,
    pub object_id: Option<i64>,
    pub s3_key: Option<String>,
}

/// `(thread id, account id)` for each id that exists.
pub(crate) fn thread_accounts(
    conn: &Connection,
    thread_ids: &[i64],
) -> rusqlite::Result<Vec<(i64, i64)>> {
    let mut out = Vec::new();
    for id in thread_ids {
        let account: Option<i64> = conn
            .query_row(
                "SELECT account_id FROM mail_thread WHERE id = ?1",
                params![id],
                |row| row.get(0),
            )
            .optional()?;
        if let Some(account) = account {
            out.push((*id, account));
        }
    }
    Ok(out)
}

/// Every message of the threads, whatever its state: a hard delete removes the
/// whole conversation, including what the sync had marked `gone`.
pub(crate) fn thread_message_objects(
    conn: &Connection,
    thread_ids: &[i64],
) -> rusqlite::Result<Vec<ThreadMessageObject>> {
    let mut stmt = conn.prepare(
        "SELECT m.id, m.thread_id, o.id, o.s3_key FROM mail_message m
         LEFT JOIN mail_object o ON o.id = m.object_id
         WHERE m.thread_id = ?1 ORDER BY m.id",
    )?;
    let mut out = Vec::new();
    for thread_id in thread_ids {
        let rows = stmt.query_map(params![thread_id], |row| {
            Ok(ThreadMessageObject {
                message_id: row.get(0)?,
                thread_id: row.get(1)?,
                object_id: row.get(2)?,
                s3_key: row.get(3)?,
            })
        })?;
        for row in rows {
            out.push(row?);
        }
    }
    Ok(out)
}

/// Marks the ledger row of an object S3 confirmed deleted as `deleted`. The
/// row stays so a listing that still shows the key finds a known object and
/// leaves it alone. Any other state would be wrong: `gone` with no message is
/// what the sync re-ingests, and `parsed` is flipped to `gone` by the next
/// sync, which would also report a vanished object nobody lost.
pub(crate) fn tombstone_object(conn: &Connection, object_id: i64) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE mail_object SET state = 'deleted', parse_error = NULL, parse_attempts = 0
         WHERE id = ?1",
        params![object_id],
    )?;
    Ok(())
}

/// Deletes the thread if it holds no message. Returns whether it did.
pub(crate) fn delete_thread_if_empty(conn: &Connection, thread_id: i64) -> rusqlite::Result<bool> {
    Ok(conn.execute(
        "DELETE FROM mail_thread WHERE id = ?1
           AND NOT EXISTS (SELECT 1 FROM mail_message WHERE thread_id = ?1)",
        params![thread_id],
    )? > 0)
}

// ── lookups for the command layer ──────────────────────────────────

pub(crate) fn get_credential(
    conn: &Connection,
    id: i64,
) -> rusqlite::Result<Option<MailCredential>> {
    Ok(list_credentials(conn)?.into_iter().find(|c| c.id == id))
}

pub(crate) fn get_account(conn: &Connection, id: i64) -> rusqlite::Result<Option<MailAccount>> {
    conn.query_row(
        &format!("SELECT {ACCOUNT_COLUMNS} FROM mail_account WHERE id = ?1"),
        params![id],
        account_from_row,
    )
    .optional()
}

/// `(id, label)` of the source that already has this exact combination, other
/// than `except`. The UNIQUE constraint would say so too, but not usefully.
pub(crate) fn find_account_by_key(
    conn: &Connection,
    credential_id: i64,
    ses_region: &str,
    bucket: &str,
    s3_region: &str,
    except: Option<i64>,
) -> rusqlite::Result<Option<(i64, String)>> {
    conn.query_row(
        "SELECT id, label FROM mail_account
         WHERE credential_id = ?1 AND ses_region = ?2 AND bucket = ?3 AND s3_region = ?4
           AND (?5 IS NULL OR id != ?5)",
        params![credential_id, ses_region, bucket, s3_region, except],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )
    .optional()
}

/// Epoch milliseconds, or `None` when identities were never fetched.
pub(crate) fn identities_refreshed_at(
    conn: &Connection,
    account_id: i64,
) -> rusqlite::Result<Option<i64>> {
    conn.query_row(
        "SELECT identities_refreshed_at FROM mail_account WHERE id = ?1",
        params![account_id],
        |row| row.get(0),
    )
}

/// Unread messages per mailbox, counting only threads that are listed.
pub(crate) fn unread_by_mailbox(
    conn: &Connection,
) -> rusqlite::Result<std::collections::HashMap<i64, i64>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT t.mailbox_id, SUM(t.unread_count) FROM mail_thread t
             WHERE t.state = 'active' AND {} GROUP BY t.mailbox_id",
        not_filed_away()
    ))?;
    let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
    rows.collect()
}

// ── thread listing and reading ─────────────────────────────────────

const DEFAULT_PAGE: u32 = 50;
const MAX_PAGE: u32 = 200;

/// Which threads a listing covers; each variant only says how it narrows
/// [`scope_filter`].
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ThreadScope {
    /// Every enabled source.
    All,
    #[serde(rename_all = "camelCase")]
    Mailbox { mailbox_id: i64 },
    /// A smart folder's materialised members.
    #[serde(rename_all = "camelCase")]
    Folder { folder_id: i64 },
    /// What the user archived, across enabled sources. Not a smart folder: it
    /// has no rules and nothing is stored for it.
    Archived,
    /// Filter chips or the search box, not saved. Empty rules list everything.
    #[serde(rename_all = "camelCase")]
    Rules {
        rules: Vec<Rule>,
        #[serde(default)]
        match_mode: MatchMode,
    },
}

/// Position after the last row of the previous page. Threads sort by
/// `(last_message_at, id)` descending, so this is a keyset, not an offset: a
/// sync that lands between two pages cannot shift rows across the boundary.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadCursor {
    pub last_message_at: i64,
    pub id: i64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadQuery {
    pub scope: ThreadScope,
    pub cursor: Option<ThreadCursor>,
    pub limit: Option<u32>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Page<T> {
    pub rows: Vec<T>,
    pub next_cursor: Option<ThreadCursor>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadRow {
    pub id: i64,
    pub account_id: i64,
    pub mailbox_id: i64,
    pub mailbox_address: String,
    pub domain: String,
    pub subject: String,
    pub snippet: String,
    pub participants: serde_json::Value,
    /// Epoch seconds.
    pub first_message_at: i64,
    /// Epoch seconds.
    pub last_message_at: i64,
    pub message_count: i64,
    pub unread_count: i64,
    pub has_attachments: bool,
    pub is_starred: bool,
}

const THREAD_SELECT: &str = "SELECT t.id, t.account_id, t.mailbox_id, b.address, b.domain,
       t.subject,
       COALESCE((SELECT m.snippet FROM mail_message m
                 WHERE m.thread_id = t.id AND m.state != 'gone'
                 ORDER BY m.state = 'active' DESC, m.date_utc DESC, m.id DESC LIMIT 1), ''),
       t.participants_json, t.first_message_at, t.last_message_at, t.message_count,
       t.unread_count, t.has_attachments, t.is_starred
     FROM mail_thread t
     JOIN mail_mailbox b ON b.id = t.mailbox_id
     JOIN mail_account a ON a.id = t.account_id";

fn thread_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<ThreadRow> {
    let participants: String = row.get(7)?;
    Ok(ThreadRow {
        id: row.get(0)?,
        account_id: row.get(1)?,
        mailbox_id: row.get(2)?,
        mailbox_address: row.get(3)?,
        domain: row.get(4)?,
        subject: row.get(5)?,
        snippet: row.get(6)?,
        participants: serde_json::from_str(&participants)
            .unwrap_or_else(|_| serde_json::Value::Array(Vec::new())),
        first_message_at: row.get(8)?,
        last_message_at: row.get(9)?,
        message_count: row.get(10)?,
        unread_count: row.get(11)?,
        has_attachments: row.get::<_, i64>(12)? != 0,
        is_starred: row.get::<_, i64>(13)? != 0,
    })
}

/// The listing's visibility test, shared by [`list_threads`] and
/// [`count_threads`] so a preview count cannot drift from the list it previews.
/// Folder membership is kept for hidden threads too; this is what keeps them
/// from reappearing in a folder view.
const LISTED: &str = "t.state = 'active'
           AND EXISTS (SELECT 1 FROM mail_message m WHERE m.thread_id = t.id AND m.state = 'active')";

/// The Archive view's visibility test. An `archived` message is what tells a
/// thread the user archived from one the sync archived because every message
/// went `gone`; only the former may be listed or restored.
const ARCHIVED_LISTED: &str = "t.state = 'archived'
           AND EXISTS (SELECT 1 FROM mail_message m WHERE m.thread_id = t.id AND m.state = 'archived')";

/// Operators whose answer depends on the clock. The one list for both places
/// that must treat such a rule specially: [`time_relative_folder`] and
/// [`not_filed_away`]. A new relative operator goes here and nowhere else.
const CLOCK_RELATIVE_OPS: &[&str] = &["within_days"];

/// Threads in a folder that behaves like a real folder leave the mailbox views.
/// A clock-relative folder never hides: its member rows go stale while its own
/// view stays true, and a thread aged out of the folder would then be in
/// neither place. Built from [`CLOCK_RELATIVE_OPS`], static literals only.
fn not_filed_away() -> String {
    let relative: Vec<String> = CLOCK_RELATIVE_OPS
        .iter()
        .map(|o| format!("'{o}'"))
        .collect();
    format!(
        "NOT EXISTS (SELECT 1 FROM mail_folder_member fm
             JOIN mail_folder f ON f.id = fm.folder_id
             WHERE fm.thread_id = t.id AND f.hide_from_mailbox = 1
               AND NOT EXISTS (SELECT 1 FROM mail_folder_rule r
                               WHERE r.folder_id = f.id AND r.op IN ({})))",
        relative.join(", ")
    )
}

/// A SQL condition on `t`, `b` and `a` plus its parameters.
fn scope_filter(
    conn: &Connection,
    scope: &ThreadScope,
    now: i64,
) -> rusqlite::Result<(String, Vec<rusqlite::types::Value>)> {
    Ok(match scope {
        ThreadScope::All => (
            format!("a.enabled = 1 AND {}", not_filed_away()),
            Vec::new(),
        ),
        ThreadScope::Mailbox { mailbox_id } => (
            format!("t.mailbox_id = ? AND {}", not_filed_away()),
            vec![(*mailbox_id).into()],
        ),
        ThreadScope::Folder { folder_id } => match time_relative_folder(conn, *folder_id)? {
            // "Last 7 days" is only true now, so a stored member list would go
            // stale unseen. Compile the rules instead; the member table stays
            // the cheap path for every other folder.
            Some((rules, mode)) => match compile_rules(&rules, mode, now)? {
                Some(c) => (format!("a.enabled = 1 AND {}", c.sql), c.params),
                None => ("0".to_string(), Vec::new()),
            },
            None => (
                "a.enabled = 1 AND t.id IN
                   (SELECT thread_id FROM mail_folder_member WHERE folder_id = ?)"
                    .to_string(),
                vec![(*folder_id).into()],
            ),
        },
        ThreadScope::Archived => ("a.enabled = 1".to_string(), Vec::new()),
        ThreadScope::Rules { rules, match_mode } => match compile_rules(rules, *match_mode, now)? {
            Some(c) => (format!("a.enabled = 1 AND {}", c.sql), c.params),
            None => ("a.enabled = 1".to_string(), Vec::new()),
        },
    })
}

fn listing_conditions(
    conn: &Connection,
    scope: &ThreadScope,
    now: i64,
) -> rusqlite::Result<(String, Vec<rusqlite::types::Value>)> {
    let (scope_sql, values) = scope_filter(conn, scope, now)?;
    let visible = match scope {
        ThreadScope::Archived => ARCHIVED_LISTED,
        _ => LISTED,
    };
    Ok((format!("{visible} AND {scope_sql}"), values))
}

/// How many threads a listing of `scope` holds across all its pages.
pub(crate) fn count_threads(conn: &Connection, scope: &ThreadScope) -> rusqlite::Result<i64> {
    Ok(scope_totals(conn, scope)?.0)
}

/// `(threads, unread messages)` of a listing, from the listing's own condition.
fn scope_totals(conn: &Connection, scope: &ThreadScope) -> rusqlite::Result<(i64, i64)> {
    let (cond, values) = listing_conditions(conn, scope, now_millis() / 1000)?;
    conn.query_row(
        &format!(
            "SELECT COUNT(*), COALESCE(SUM(t.unread_count), 0) FROM mail_thread t
             JOIN mail_mailbox b ON b.id = t.mailbox_id
             JOIN mail_account a ON a.id = t.account_id
             WHERE {cond}"
        ),
        rusqlite::params_from_iter(values),
        |row| Ok((row.get(0)?, row.get(1)?)),
    )
}

/// One page of listed threads, newest first. A thread is listed while it has an
/// `active` message: `gone` (vanished from S3) and `archived` (the user's
/// choice) both drop out, and they stay two states so a sync can still tell
/// them apart (spec section 10).
pub(crate) fn list_threads(
    conn: &Connection,
    query: &ThreadQuery,
) -> rusqlite::Result<Page<ThreadRow>> {
    use rusqlite::types::Value;
    let limit = query.limit.unwrap_or(DEFAULT_PAGE).clamp(1, MAX_PAGE);
    let (cond, mut values) = listing_conditions(conn, &query.scope, now_millis() / 1000)?;
    let mut sql = format!("{THREAD_SELECT} WHERE {cond}");
    if let Some(cursor) = query.cursor {
        sql.push_str(" AND (t.last_message_at < ? OR (t.last_message_at = ? AND t.id < ?))");
        values.push(Value::from(cursor.last_message_at));
        values.push(Value::from(cursor.last_message_at));
        values.push(Value::from(cursor.id));
    }
    sql.push_str(" ORDER BY t.last_message_at DESC, t.id DESC LIMIT ?");
    // One extra row says whether another page exists.
    values.push(Value::from(i64::from(limit) + 1));
    let mut stmt = conn.prepare(&sql)?;
    let mut rows: Vec<ThreadRow> = stmt
        .query_map(rusqlite::params_from_iter(values), thread_from_row)?
        .collect::<rusqlite::Result<_>>()?;
    let next_cursor = if rows.len() > limit as usize {
        rows.truncate(limit as usize);
        rows.last().map(|t| ThreadCursor {
            last_message_at: t.last_message_at,
            id: t.id,
        })
    } else {
        None
    };
    Ok(Page { rows, next_cursor })
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AttachmentRow {
    pub id: i64,
    pub message_id: i64,
    pub part_index: i64,
    pub filename: String,
    pub mime_type: String,
    pub size_bytes: i64,
    pub content_id: Option<String>,
    pub is_inline: bool,
    /// Whether the bytes are already on disk. The path itself stays in Rust.
    pub downloaded: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageRow {
    pub id: i64,
    pub direction: String,
    pub message_id_hdr: Option<String>,
    pub in_reply_to: Option<String>,
    pub subject: String,
    pub from_name: Option<String>,
    pub from_addr: String,
    pub reply_to_addr: Option<String>,
    pub to: serde_json::Value,
    pub cc: serde_json::Value,
    /// Only outgoing mail carries a Bcc list.
    pub bcc: serde_json::Value,
    /// Epoch seconds.
    pub date_utc: i64,
    pub snippet: String,
    pub body_text: Option<String>,
    /// Already sanitised; remote images are in `data-mono-src`.
    pub body_html: Option<String>,
    pub remote_images_blocked: usize,
    pub has_attachments: bool,
    pub size_bytes: Option<i64>,
    pub spf: Option<String>,
    pub dkim: Option<String>,
    pub dmarc: Option<String>,
    pub is_read: bool,
    pub is_starred: bool,
    pub state: String,
    pub attachments: Vec<AttachmentRow>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadDetail {
    pub thread: ThreadRow,
    /// `ask | always | never`, from the thread's source.
    pub image_policy: String,
    /// Whether "Move to inbox" would work. Derived by the same test the restore
    /// applies, because the thread's own state is not inferable from its
    /// messages: a revived message leaves an archived thread with an active one,
    /// and a new message leaves an active thread with archived ones.
    pub restorable: bool,
    pub messages: Vec<MessageRow>,
}

fn json_or_empty(text: &str) -> serde_json::Value {
    serde_json::from_str(text).unwrap_or_else(|_| serde_json::Value::Array(Vec::new()))
}

fn attachment_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<AttachmentRow> {
    Ok(AttachmentRow {
        id: row.get(0)?,
        message_id: row.get(1)?,
        part_index: row.get(2)?,
        filename: row.get(3)?,
        mime_type: row.get(4)?,
        size_bytes: row.get(5)?,
        content_id: row.get(6)?,
        is_inline: row.get::<_, i64>(7)? != 0,
        downloaded: row.get::<_, Option<String>>(8)?.is_some(),
    })
}

const ATTACHMENT_SELECT: &str = "SELECT id, message_id, part_index, filename, mime_type,
       size_bytes, content_id, is_inline, local_path FROM mail_attachment";

/// A thread with its messages and their attachment metadata. Messages that
/// vanished from S3 (`gone`) are left out; ones the user archived are not,
/// because opening a thread is an explicit act and listing is where archive
/// hides things.
pub(crate) fn thread_detail(
    conn: &Connection,
    thread_id: i64,
) -> rusqlite::Result<Option<ThreadDetail>> {
    let thread = conn
        .query_row(
            &format!("{THREAD_SELECT} WHERE t.id = ?1"),
            params![thread_id],
            thread_from_row,
        )
        .optional()?;
    let Some(thread) = thread else {
        return Ok(None);
    };
    let image_policy: String = conn.query_row(
        "SELECT image_policy FROM mail_account WHERE id = ?1",
        params![thread.account_id],
        |row| row.get(0),
    )?;
    let mut attachments: std::collections::HashMap<i64, Vec<AttachmentRow>> =
        std::collections::HashMap::new();
    {
        let mut stmt = conn.prepare(&format!(
            "{ATTACHMENT_SELECT} WHERE message_id IN
               (SELECT id FROM mail_message WHERE thread_id = ?1) ORDER BY part_index"
        ))?;
        for row in stmt.query_map(params![thread_id], attachment_from_row)? {
            let row = row?;
            attachments.entry(row.message_id).or_default().push(row);
        }
    }
    let mut stmt = conn.prepare(
        "SELECT id, direction, message_id_hdr, in_reply_to, subject, from_name, from_addr,
                reply_to_addr, to_json, cc_json, bcc_json, date_utc, snippet, body_text,
                body_html, has_attachments, size_bytes, spf, dkim, dmarc, is_read,
                is_starred, state
         FROM mail_message WHERE thread_id = ?1 AND state != 'gone' ORDER BY date_utc, id",
    )?;
    let messages = stmt
        .query_map(params![thread_id], |row| {
            let id: i64 = row.get(0)?;
            let body_html: Option<String> = row.get(14)?;
            Ok(MessageRow {
                id,
                direction: row.get(1)?,
                message_id_hdr: row.get(2)?,
                in_reply_to: row.get(3)?,
                subject: row.get(4)?,
                from_name: row.get(5)?,
                from_addr: row.get(6)?,
                reply_to_addr: row.get(7)?,
                to: json_or_empty(&row.get::<_, String>(8)?),
                cc: json_or_empty(&row.get::<_, String>(9)?),
                bcc: json_or_empty(&row.get::<_, String>(10)?),
                date_utc: row.get(11)?,
                snippet: row.get(12)?,
                body_text: row.get(13)?,
                remote_images_blocked: body_html
                    .as_deref()
                    .map_or(0, |h| h.matches(BLOCKED_IMAGE_ATTR).count()),
                body_html,
                has_attachments: row.get::<_, i64>(15)? != 0,
                size_bytes: row.get(16)?,
                spf: row.get(17)?,
                dkim: row.get(18)?,
                dmarc: row.get(19)?,
                is_read: row.get::<_, i64>(20)? != 0,
                is_starred: row.get::<_, i64>(21)? != 0,
                state: row.get(22)?,
                attachments: attachments.remove(&id).unwrap_or_default(),
            })
        })?
        .collect::<rusqlite::Result<_>>()?;
    let restorable = is_restorable(conn, thread_id)?;
    Ok(Some(ThreadDetail {
        thread,
        image_policy,
        restorable,
        messages,
    }))
}

/// The attribute `mail_mime` puts the blocked remote URL in.
pub(crate) const BLOCKED_IMAGE_ATTR: &str = "data-mono-src=\"";

/// Marks every message of the threads read or unread and recounts the thread.
pub(crate) fn set_threads_read(
    conn: &Connection,
    thread_ids: &[i64],
    read: bool,
) -> rusqlite::Result<()> {
    let tx = conn.unchecked_transaction()?;
    for id in thread_ids {
        // A `gone` row keeps its read state: the user cannot see it, so this is
        // not a write they asked for, and a revive then restores what it was.
        tx.execute(
            "UPDATE mail_message SET is_read = ?2 WHERE thread_id = ?1 AND state != 'gone'",
            params![id, read],
        )?;
        tx.execute(
            "UPDATE mail_thread SET unread_count =
               (SELECT COUNT(*) FROM mail_message
                WHERE thread_id = ?1 AND is_read = 0 AND state != 'gone')
             WHERE id = ?1",
            params![id],
        )?;
        // `is_unread` rules read the count just rewritten.
        reevaluate_thread(&tx, *id)?;
    }
    tx.commit()
}

pub(crate) fn set_threads_starred(
    conn: &Connection,
    thread_ids: &[i64],
    starred: bool,
) -> rusqlite::Result<()> {
    let tx = conn.unchecked_transaction()?;
    for id in thread_ids {
        tx.execute(
            "UPDATE mail_message SET is_starred = ?2 WHERE thread_id = ?1",
            params![id, starred],
        )?;
        tx.execute(
            "UPDATE mail_thread SET is_starred = ?2 WHERE id = ?1",
            params![id, starred],
        )?;
        reevaluate_thread(&tx, *id)?;
    }
    tx.commit()
}

/// Where a message's raw MIME lives.
pub(crate) struct MessageLocation {
    pub message_id: i64,
    pub account_id: i64,
    /// `None` for mail with no S3 object, such as a draft's sent copy.
    pub s3_key: Option<String>,
    pub body_html: Option<String>,
}

pub(crate) fn message_location(
    conn: &Connection,
    message_id: i64,
) -> rusqlite::Result<Option<MessageLocation>> {
    conn.query_row(
        "SELECT m.id, b.account_id, o.s3_key, m.body_html
         FROM mail_message m
         JOIN mail_mailbox b ON b.id = m.mailbox_id
         LEFT JOIN mail_object o ON o.id = m.object_id
         WHERE m.id = ?1",
        params![message_id],
        |row| {
            Ok(MessageLocation {
                message_id: row.get(0)?,
                account_id: row.get(1)?,
                s3_key: row.get(2)?,
                body_html: row.get(3)?,
            })
        },
    )
    .optional()
}

/// An attachment and the path it was saved to, if it was.
pub(crate) fn get_attachment(
    conn: &Connection,
    attachment_id: i64,
) -> rusqlite::Result<Option<(AttachmentRow, Option<String>)>> {
    conn.query_row(
        &format!("{ATTACHMENT_SELECT} WHERE id = ?1"),
        params![attachment_id],
        |row| Ok((attachment_from_row(row)?, row.get(8)?)),
    )
    .optional()
}

pub(crate) fn set_attachment_path(
    conn: &Connection,
    attachment_id: i64,
    path: Option<&str>,
) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE mail_attachment SET local_path = ?2 WHERE id = ?1",
        params![attachment_id, path],
    )?;
    Ok(())
}

// ── rules and smart folders ────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MatchMode {
    #[default]
    All,
    Any,
}

impl MatchMode {
    fn as_str(self) -> &'static str {
        match self {
            MatchMode::All => "all",
            MatchMode::Any => "any",
        }
    }
}

/// One condition. The same type backs the filter chips, the search box and a
/// saved folder, so "Save as folder" is a copy of data. `value` is a string,
/// a number, or for `contains_any` an array of strings.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Rule {
    pub field: String,
    pub op: String,
    #[serde(default)]
    pub value: serde_json::Value,
    #[serde(default)]
    pub negate: bool,
}

const TEXT_OPS: &[&str] = &[
    "is",
    "contains",
    "contains_any",
    "starts_with",
    "ends_with",
    "matches",
];
const SUBJECT_OPS: &[&str] = &[
    "is",
    "contains",
    "contains_prefix",
    "contains_any",
    "starts_with",
    "ends_with",
    "matches",
];
const FTS_OPS: &[&str] = &["contains", "contains_prefix", "contains_any"];
const SCOPE_OPS: &[&str] = &["is", "contains_any"];
const NAME_OPS: &[&str] = &["is", "contains", "contains_any"];
const FLAG_OPS: &[&str] = &["true"];
const DATE_OPS: &[&str] = &["before", "after", "within_days"];
const MAX_LIST_VALUES: usize = 100;
const MAX_RULES: usize = 50;

/// Every field and the operators that mean something on it. One table, so
/// the lookup and the tests cannot list different fields.
pub(crate) const FIELD_OPS: &[(&str, &[&str])] = &[
    ("from", TEXT_OPS),
    ("to", TEXT_OPS),
    ("cc", TEXT_OPS),
    ("recipient", TEXT_OPS),
    ("subject", SUBJECT_OPS),
    ("body", FTS_OPS),
    ("mailbox", SCOPE_OPS),
    ("domain", NAME_OPS),
    ("account", NAME_OPS),
    ("has_attachment", FLAG_OPS),
    ("is_unread", FLAG_OPS),
    ("is_starred", FLAG_OPS),
    ("date", DATE_OPS),
];

pub(crate) fn allowed_ops(field: &str) -> Option<&'static [&'static str]> {
    FIELD_OPS.iter().find(|(f, _)| *f == field).map(|(_, o)| *o)
}

/// A `WHERE` fragment over the aliases `t` (thread), `b` (mailbox) and `a`
/// (source), with its bound values in placeholder order.
pub(crate) struct Compiled {
    pub sql: String,
    pub params: Vec<rusqlite::types::Value>,
}

fn rule_value_error(rule: &Rule) -> rusqlite::Error {
    user_error(format!("A \"{}\" rule needs a value.", rule.field))
}

fn scalar_value(rule: &Rule) -> rusqlite::Result<String> {
    let text = match &rule.value {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Number(n) => n.to_string(),
        _ => return Err(rule_value_error(rule)),
    };
    if text.trim().is_empty() {
        return Err(rule_value_error(rule));
    }
    Ok(text)
}

/// The values of a `contains_any` rule. The database keeps them as JSON text,
/// so a string holding an array is accepted as well as the array itself.
fn list_value(rule: &Rule) -> rusqlite::Result<Vec<String>> {
    let items: Vec<serde_json::Value> = match &rule.value {
        serde_json::Value::Array(a) => a.clone(),
        serde_json::Value::String(s) => match serde_json::from_str::<serde_json::Value>(s) {
            Ok(serde_json::Value::Array(a)) => a,
            _ => vec![serde_json::Value::String(s.clone())],
        },
        _ => return Err(rule_value_error(rule)),
    };
    let out: Vec<String> = items
        .iter()
        .filter_map(|v| match v {
            serde_json::Value::String(s) => Some(s.clone()),
            serde_json::Value::Number(n) => Some(n.to_string()),
            _ => None,
        })
        .filter(|s| !s.trim().is_empty())
        .collect();
    if out.is_empty() {
        return Err(rule_value_error(rule));
    }
    if out.len() > MAX_LIST_VALUES {
        return Err(user_error(format!(
            "A \"{}\" rule can list at most {MAX_LIST_VALUES} values.",
            rule.field
        )));
    }
    Ok(out)
}

/// `LIKE` treats `%` and `_` as wildcards; a literal match needs them, and the
/// escape character itself, backslashed (paired with `ESCAPE '\'`).
fn escape_like(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if matches!(c, '\\' | '%' | '_') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// Days since 1970-01-01 of a proleptic Gregorian date.
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Epoch seconds from `YYYY-MM-DD` (UTC midnight) or from the number itself.
fn parse_when(text: &str) -> Option<i64> {
    let text = text.trim();
    if let Ok(secs) = text.parse::<i64>() {
        return Some(secs);
    }
    let mut parts = text.split('-');
    let (y, m, d) = (
        parts.next()?.parse::<i64>().ok()?,
        parts.next()?.parse::<i64>().ok()?,
        parts.next()?.parse::<i64>().ok()?,
    );
    if parts.next().is_some() || !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return None;
    }
    Some(days_from_civil(y, m, d) * 86_400)
}

/// One comparison of `expr` against `value`. The operator picks the template;
/// the value is always a bound parameter.
fn text_term(
    expr: &str,
    op: &str,
    value: &str,
    params: &mut Vec<rusqlite::types::Value>,
) -> String {
    use rusqlite::types::Value;
    let (sql, bound) = match op {
        "is" => (format!("{expr} = ? COLLATE NOCASE"), value.to_string()),
        "starts_with" => (
            format!("{expr} LIKE ? ESCAPE '\\'"),
            format!("{}%", escape_like(value)),
        ),
        "ends_with" => (
            format!("{expr} LIKE ? ESCAPE '\\'"),
            format!("%{}", escape_like(value)),
        ),
        "matches" => (format!("{expr} GLOB ?"), value.to_string()),
        // "contains"
        _ => (
            format!("{expr} LIKE ? ESCAPE '\\'"),
            format!("%{}%", escape_like(value)),
        ),
    };
    params.push(Value::Text(bound));
    sql
}

/// An address list column (`to_json`, `cc_json`) is `[{name,addr}]`; searching
/// the JSON text would match its punctuation and keys, so each entry is read.
fn address_term(
    column: &str,
    op: &str,
    value: &str,
    params: &mut Vec<rusqlite::types::Value>,
) -> String {
    let addr = text_term(
        "COALESCE(json_extract(j.value, '$.addr'), '')",
        op,
        value,
        params,
    );
    let name = text_term(
        "COALESCE(json_extract(j.value, '$.name'), '')",
        op,
        value,
        params,
    );
    format!(
        "EXISTS (SELECT 1 FROM json_each(CASE WHEN json_valid({column}) THEN {column} ELSE '[]' END) j
                 WHERE ({addr} OR {name}))"
    )
}

/// FTS5 phrase for `text`: quoted, so `*`, `-`, `:`, `OR` and the like are
/// words and cannot change the query's shape. Folded like the index.
/// With `prefix`, the last token also matches as a prefix (`"inv" *`), which
/// FTS5 does without a `prefix=` option. A one or two character prefix walks a
/// wide slice of the term index, so the caller must debounce and set a minimum
/// length before using `contains_prefix` as-you-type.
fn fts_phrase(text: &str, prefix: bool) -> Option<String> {
    let folded = fold_fts(text);
    if !folded.chars().any(char::is_alphanumeric) {
        return None;
    }
    let star = if prefix { " *" } else { "" };
    Some(format!("\"{}\"{star}", folded.replace('"', "\"\"")))
}

fn fts_term(
    column: &str,
    values: &[String],
    prefix: bool,
    params: &mut Vec<rusqlite::types::Value>,
) -> String {
    let phrases: Vec<String> = values
        .iter()
        .filter_map(|v| fts_phrase(v, prefix))
        .collect();
    if phrases.is_empty() {
        // Nothing the index could hold, so nothing matches.
        return "0".to_string();
    }
    params.push(rusqlite::types::Value::Text(format!(
        "{column} : ({})",
        phrases.join(" OR ")
    )));
    "m.id IN (SELECT rowid FROM mail_message_fts WHERE mail_message_fts MATCH ?)".to_string()
}

/// A thread matches when any of its visible messages satisfies `pred`.
fn any_message(pred: &str) -> String {
    format!(
        "EXISTS (SELECT 1 FROM mail_message m
                 WHERE m.thread_id = t.id AND m.state != 'gone' AND ({pred}))"
    )
}

/// One bound id-or-name test. A number is an id; anything else is looked up by
/// `name_column`.
fn id_or_name(
    id_expr: &str,
    table: &str,
    name_column: &str,
    value: &str,
    params: &mut Vec<rusqlite::types::Value>,
) -> String {
    use rusqlite::types::Value;
    match value.trim().parse::<i64>() {
        Ok(id) => {
            params.push(Value::Integer(id));
            format!("{id_expr} = ?")
        }
        Err(_) => {
            params.push(Value::Text(value.trim().to_string()));
            format!("{id_expr} IN (SELECT id FROM {table} WHERE {name_column} = ? COLLATE NOCASE)")
        }
    }
}

fn compile_rule(
    rule: &Rule,
    now: i64,
    params: &mut Vec<rusqlite::types::Value>,
) -> rusqlite::Result<String> {
    use rusqlite::types::Value;
    let field = rule.field.as_str();
    let ops = allowed_ops(field)
        .ok_or_else(|| user_error(format!("\"{}\" is not a field mail can filter on.", field)))?;
    let op = rule.op.as_str();
    if !ops.contains(&op) {
        return Err(user_error(format!(
            "\"{op}\" does not apply to a \"{field}\" rule."
        )));
    }
    // `contains_any` is `contains` over a list, joined with OR.
    let (op, values) = match op {
        "contains_any" => ("contains", list_value(rule)?),
        "contains_prefix" => ("contains_prefix", vec![scalar_value(rule)?]),
        "true" | "before" | "after" | "within_days" => ("", Vec::new()),
        _ => (op, vec![scalar_value(rule)?]),
    };
    let or_all = |terms: Vec<String>| format!("({})", terms.join(" OR "));
    let clause = match field {
        "from" => any_message(&or_all(
            values
                .iter()
                .map(|v| {
                    let a = text_term("m.from_addr", op, v, params);
                    let n = text_term("COALESCE(m.from_name, '')", op, v, params);
                    format!("({a} OR {n})")
                })
                .collect(),
        )),
        "to" | "cc" | "recipient" => {
            let columns: &[&str] = match field {
                "to" => &["m.to_json"],
                "cc" => &["m.cc_json"],
                _ => &["m.to_json", "m.cc_json"],
            };
            any_message(&or_all(
                values
                    .iter()
                    .flat_map(|v| columns.iter().map(move |c| (*c, v)))
                    .map(|(c, v)| address_term(c, op, v, params))
                    .collect(),
            ))
        }
        "subject" | "body" if op == "contains" || op == "contains_prefix" => {
            any_message(&fts_term(field, &values, op == "contains_prefix", params))
        }
        "subject" => any_message(&or_all(
            values
                .iter()
                .map(|v| text_term("m.subject", op, v, params))
                .collect(),
        )),
        "mailbox" => or_all(
            values
                .iter()
                .map(|v| id_or_name("t.mailbox_id", "mail_mailbox", "address", v, params))
                .collect(),
        ),
        "account" if rule.op == "contains" => or_all(
            values
                .iter()
                .map(|v| {
                    let like = text_term("label", "contains", v, params);
                    format!("t.account_id IN (SELECT id FROM mail_account WHERE {like})")
                })
                .collect(),
        ),
        "account" => or_all(
            values
                .iter()
                .map(|v| id_or_name("t.account_id", "mail_account", "label", v, params))
                .collect(),
        ),
        // `contains_any` stays exact-any, like `mailbox`; only `contains` is a
        // substring.
        "domain" if rule.op == "contains" => or_all(
            values
                .iter()
                .map(|v| {
                    let wanted = v.trim().trim_start_matches('@').to_ascii_lowercase();
                    text_term("b.domain", "contains", &wanted, params)
                })
                .collect(),
        ),
        "domain" => or_all(
            values
                .iter()
                .map(|v| {
                    params.push(Value::Text(
                        v.trim().trim_start_matches('@').to_ascii_lowercase(),
                    ));
                    "b.domain = ?".to_string()
                })
                .collect(),
        ),
        "has_attachment" => "t.has_attachments = 1".to_string(),
        "is_unread" => "t.unread_count > 0".to_string(),
        "is_starred" => "t.is_starred = 1".to_string(),
        "date" => {
            let raw = scalar_value(rule)?;
            match rule.op.as_str() {
                "within_days" => {
                    let days = raw
                        .trim()
                        .parse::<i64>()
                        .ok()
                        .filter(|d| (1..=36_500).contains(d))
                        .ok_or_else(|| user_error("Enter a number of days.".to_string()))?;
                    params.push(Value::Integer(now - days * 86_400));
                    any_message("m.date_utc >= ?")
                }
                bound => {
                    let when = parse_when(&raw).ok_or_else(|| {
                        user_error(format!("\"{}\" is not a date. Use YYYY-MM-DD.", raw.trim()))
                    })?;
                    params.push(Value::Integer(when));
                    any_message(if bound == "before" {
                        "m.date_utc < ?"
                    } else {
                        "m.date_utc >= ?"
                    })
                }
            }
        }
        // A field added to `FIELD_OPS` without an arm here: refuse it, never
        // panic while holding the store lock every feature shares.
        _ => {
            return Err(user_error(format!(
                "\"{field}\" cannot be filtered on yet."
            )))
        }
    };
    Ok(if rule.negate {
        format!("(NOT ({clause}))")
    } else {
        format!("({clause})")
    })
}

/// Compiles rules to one condition. `None` means no rules, which the caller
/// reads as "no restriction" (a listing) or rejects (a folder). Each rule is
/// parenthesised on its own and `negate` binds to that rule alone, so
/// `any` with two negated rules is `(NOT a) OR (NOT b)`, while `all` with the
/// same two is `NOT (a OR b)`. A negated message-level rule means "no message
/// of the thread matches".
pub(crate) fn compile_rules(
    rules: &[Rule],
    mode: MatchMode,
    now: i64,
) -> rusqlite::Result<Option<Compiled>> {
    if rules.is_empty() {
        return Ok(None);
    }
    if rules.len() > MAX_RULES {
        return Err(user_error(format!(
            "A filter can have at most {MAX_RULES} rules."
        )));
    }
    let mut params = Vec::new();
    let mut parts = Vec::with_capacity(rules.len());
    for rule in rules {
        parts.push(compile_rule(rule, now, &mut params)?);
    }
    let joiner = if mode == MatchMode::Any {
        " OR "
    } else {
        " AND "
    };
    Ok(Some(Compiled {
        sql: format!("({})", parts.join(joiner)),
        params,
    }))
}

// ── folders ────────────────────────────────────────────────────────

const MAX_FOLDER_NAME: usize = 80;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FolderInput {
    pub id: Option<i64>,
    pub name: String,
    pub color: Option<String>,
    /// `None` keeps a folder where it is and appends a new one.
    pub sort_order: Option<i64>,
    #[serde(default)]
    pub match_mode: MatchMode,
    #[serde(default)]
    pub hide_from_mailbox: bool,
    #[serde(default = "default_true")]
    pub count_unread: bool,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FolderRow {
    pub id: i64,
    pub name: String,
    pub color: Option<String>,
    pub sort_order: i64,
    pub match_mode: MatchMode,
    pub hide_from_mailbox: bool,
    pub count_unread: bool,
    pub rules: Vec<Rule>,
    /// Listed threads in the folder.
    pub thread_count: i64,
    /// Unread messages in those threads.
    pub unread_count: i64,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewCount {
    pub count: i64,
}

/// `contains_any` is stored as JSON text; every other value as itself.
fn rule_value_to_text(rule: &Rule) -> String {
    match &rule.value {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Null => String::new(),
        other => other.to_string(),
    }
}

fn rule_value_from_text(op: &str, text: String) -> serde_json::Value {
    if op == "contains_any" {
        if let Ok(v @ serde_json::Value::Array(_)) = serde_json::from_str(&text) {
            return v;
        }
    }
    serde_json::Value::String(text)
}

fn folder_rules(conn: &Connection, folder_id: i64) -> rusqlite::Result<Vec<Rule>> {
    let mut stmt = conn.prepare(
        "SELECT field, op, value, negate FROM mail_folder_rule
         WHERE folder_id = ?1 ORDER BY position, id",
    )?;
    let rows = stmt.query_map(params![folder_id], |row| {
        let op: String = row.get(1)?;
        let text: String = row.get(2)?;
        Ok(Rule {
            field: row.get(0)?,
            value: rule_value_from_text(&op, text),
            op,
            negate: row.get::<_, i64>(3)? != 0,
        })
    })?;
    rows.collect()
}

fn folder_match_mode(text: &str) -> MatchMode {
    if text == "any" {
        MatchMode::Any
    } else {
        MatchMode::All
    }
}

fn load_folder(conn: &Connection, id: i64) -> rusqlite::Result<Option<FolderRow>> {
    let head = conn
        .query_row(
            "SELECT id, name, color, sort_order, match_mode, hide_from_mailbox, count_unread
             FROM mail_folder WHERE id = ?1",
            params![id],
            |row| {
                Ok(FolderRow {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    color: row.get(2)?,
                    sort_order: row.get(3)?,
                    match_mode: folder_match_mode(&row.get::<_, String>(4)?),
                    hide_from_mailbox: row.get::<_, i64>(5)? != 0,
                    count_unread: row.get::<_, i64>(6)? != 0,
                    rules: Vec::new(),
                    thread_count: 0,
                    unread_count: 0,
                })
            },
        )
        .optional()?;
    let Some(mut folder) = head else {
        return Ok(None);
    };
    folder.rules = folder_rules(conn, id)?;
    // Counted by the listing's own condition, so a sidebar count cannot
    // disagree with the folder it opens.
    let (threads, unread) = scope_totals(conn, &ThreadScope::Folder { folder_id: id })?;
    folder.thread_count = threads;
    folder.unread_count = unread;
    Ok(Some(folder))
}

pub(crate) fn list_folders(conn: &Connection) -> rusqlite::Result<Vec<FolderRow>> {
    let ids: Vec<i64> = {
        let mut stmt = conn.prepare("SELECT id FROM mail_folder ORDER BY sort_order, id")?;
        let rows = stmt.query_map([], |row| row.get(0))?;
        rows.collect::<rusqlite::Result<_>>()?
    };
    let mut out = Vec::with_capacity(ids.len());
    for id in ids {
        out.extend(load_folder(conn, id)?);
    }
    Ok(out)
}

/// Creates or edits a folder and re-materialises its members from every stored
/// thread, in one transaction. The rules are compiled before anything is
/// written, so a bad rule leaves the folder as it was.
pub(crate) fn upsert_folder(
    conn: &Connection,
    folder: &FolderInput,
    rules: &[Rule],
) -> rusqlite::Result<FolderRow> {
    let name = folder.name.trim();
    if name.is_empty() {
        return Err(user_error("Give the folder a name.".to_string()));
    }
    if name.chars().count() > MAX_FOLDER_NAME {
        return Err(user_error(format!(
            "A folder name can be at most {MAX_FOLDER_NAME} characters."
        )));
    }
    if rules.is_empty() {
        return Err(user_error("A folder needs at least one rule.".to_string()));
    }
    compile_rules(rules, folder.match_mode, now_millis() / 1000)?;
    let clash: Option<String> = conn
        .query_row(
            "SELECT name FROM mail_folder WHERE name = ?1 COLLATE NOCASE AND id IS NOT ?2",
            params![name, folder.id],
            |row| row.get(0),
        )
        .optional()?;
    if let Some(existing) = clash {
        return Err(user_error(format!(
            "A folder named \"{existing}\" already exists."
        )));
    }
    let now = now_millis();
    let tx = conn.unchecked_transaction()?;
    let id = match folder.id {
        Some(id) => {
            let changed = tx.execute(
                "UPDATE mail_folder SET name = ?2, color = ?3, match_mode = ?4,
                   hide_from_mailbox = ?5, count_unread = ?6, updated_at = ?7,
                   sort_order = COALESCE(?8, sort_order)
                 WHERE id = ?1",
                params![
                    id,
                    name,
                    folder.color,
                    folder.match_mode.as_str(),
                    folder.hide_from_mailbox,
                    folder.count_unread,
                    now,
                    folder.sort_order
                ],
            )?;
            if changed == 0 {
                return Err(user_error("That folder no longer exists.".to_string()));
            }
            id
        }
        None => {
            tx.execute(
                "INSERT INTO mail_folder
                   (name, color, sort_order, match_mode, hide_from_mailbox, count_unread,
                    created_at, updated_at)
                 VALUES (?1, ?2,
                   COALESCE(?3, (SELECT COALESCE(MAX(sort_order), -1) + 1 FROM mail_folder)),
                   ?4, ?5, ?6, ?7, ?7)",
                params![
                    name,
                    folder.color,
                    folder.sort_order,
                    folder.match_mode.as_str(),
                    folder.hide_from_mailbox,
                    folder.count_unread,
                    now
                ],
            )?;
            tx.last_insert_rowid()
        }
    };
    tx.execute(
        "DELETE FROM mail_folder_rule WHERE folder_id = ?1",
        params![id],
    )?;
    for (position, rule) in rules.iter().enumerate() {
        tx.execute(
            "INSERT INTO mail_folder_rule (folder_id, position, field, op, value, negate)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                id,
                position as i64,
                rule.field,
                rule.op,
                rule_value_to_text(rule),
                rule.negate
            ],
        )?;
    }
    rematerialize_folder(&tx, id)?;
    tx.commit()?;
    Ok(load_folder(conn, id)?.expect("folder was just written"))
}

pub(crate) fn delete_folder(conn: &Connection, id: i64) -> rusqlite::Result<()> {
    // Rules and members go by cascade; no message is touched.
    conn.execute("DELETE FROM mail_folder WHERE id = ?1", params![id])?;
    Ok(())
}

/// The folder's rules and mode when any of them is relative to the clock.
fn time_relative_folder(
    conn: &Connection,
    folder_id: i64,
) -> rusqlite::Result<Option<(Vec<Rule>, MatchMode)>> {
    let mode: Option<String> = conn
        .query_row(
            "SELECT match_mode FROM mail_folder WHERE id = ?1",
            params![folder_id],
            |row| row.get(0),
        )
        .optional()?;
    let Some(mode) = mode else {
        return Ok(None);
    };
    let rules = folder_rules(conn, folder_id)?;
    Ok(rules
        .iter()
        .any(|r| CLOCK_RELATIVE_OPS.contains(&r.op.as_str()))
        .then(|| (rules, folder_match_mode(&mode))))
}

/// A folder's compiled rules, or `None` when it has none (such a folder holds
/// nothing rather than everything).
fn folder_condition(
    conn: &Connection,
    folder_id: i64,
    mode: MatchMode,
) -> rusqlite::Result<Option<Compiled>> {
    let rules = folder_rules(conn, folder_id)?;
    compile_rules(&rules, mode, now_millis() / 1000)
}

/// Replaces a folder's members with every stored thread its rules match, for
/// a new folder or an edited rule ("Apply to mail already stored"). Takes the
/// caller's connection and opens no transaction.
pub(crate) fn rematerialize_folder(conn: &Connection, folder_id: i64) -> rusqlite::Result<()> {
    let mode: String = conn.query_row(
        "SELECT match_mode FROM mail_folder WHERE id = ?1",
        params![folder_id],
        |row| row.get(0),
    )?;
    conn.execute(
        "DELETE FROM mail_folder_member WHERE folder_id = ?1",
        params![folder_id],
    )?;
    let Some(c) = folder_condition(conn, folder_id, folder_match_mode(&mode))? else {
        return Ok(());
    };
    // Membership ignores listing visibility on purpose: archiving or disabling
    // a source must not lose the folder, and the listing hides what it should.
    let mut values = vec![rusqlite::types::Value::Integer(folder_id)];
    values.extend(c.params);
    conn.execute(
        &format!(
            "INSERT INTO mail_folder_member (folder_id, thread_id)
             SELECT ?, t.id FROM mail_thread t
             JOIN mail_mailbox b ON b.id = t.mailbox_id
             JOIN mail_account a ON a.id = t.account_id
             WHERE {}",
            c.sql
        ),
        rusqlite::params_from_iter(values),
    )?;
    Ok(())
}

/// Re-evaluates one thread against every folder, for a message arriving or a
/// flag changing. A thread that no longer exists is skipped. Runs on the
/// caller's connection, inside its transaction.
pub(crate) fn reevaluate_thread(conn: &Connection, thread_id: i64) -> rusqlite::Result<()> {
    let folders: Vec<(i64, String)> = {
        let mut stmt = conn.prepare("SELECT id, match_mode FROM mail_folder")?;
        let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
        rows.collect::<rusqlite::Result<_>>()?
    };
    if folders.is_empty() {
        return Ok(());
    }
    let exists: bool = conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM mail_thread WHERE id = ?1)",
        params![thread_id],
        |row| row.get(0),
    )?;
    if !exists {
        return Ok(());
    }
    for (folder_id, mode) in folders {
        let matched = match folder_condition(conn, folder_id, folder_match_mode(&mode))? {
            None => false,
            Some(c) => {
                let mut values = vec![rusqlite::types::Value::Integer(thread_id)];
                values.extend(c.params);
                conn.query_row(
                    &format!(
                        "SELECT EXISTS (SELECT 1 FROM mail_thread t
                         JOIN mail_mailbox b ON b.id = t.mailbox_id
                         JOIN mail_account a ON a.id = t.account_id
                         WHERE t.id = ? AND {})",
                        c.sql
                    ),
                    rusqlite::params_from_iter(values),
                    |row| row.get(0),
                )?
            }
        };
        if matched {
            conn.execute(
                "INSERT OR IGNORE INTO mail_folder_member (folder_id, thread_id) VALUES (?1, ?2)",
                params![folder_id, thread_id],
            )?;
        } else {
            conn.execute(
                "DELETE FROM mail_folder_member WHERE folder_id = ?1 AND thread_id = ?2",
                params![folder_id, thread_id],
            )?;
        }
    }
    Ok(())
}

/// The live count in the folder editor: what [`list_threads`] would return,
/// across all pages, for these rules.
pub(crate) fn preview_rules(
    conn: &Connection,
    rules: &[Rule],
    mode: MatchMode,
) -> rusqlite::Result<PreviewCount> {
    let count = count_threads(
        conn,
        &ThreadScope::Rules {
            rules: rules.to_vec(),
            match_mode: mode,
        },
    )?;
    Ok(PreviewCount { count })
}

// ── outbox ─────────────────────────────────────────────────────────

/// One recipient. Same shape as the webview's `MailAddress`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Recipient {
    #[serde(default)]
    pub name: Option<String>,
    pub addr: String,
}

/// A file to attach. `cid` set means an inline image, referenced from the body
/// as `cid:<value>`; the value is the `contentId` the composer's serializer
/// produced, stored without angle brackets.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutboxAttachment {
    pub path: String,
    pub filename: String,
    pub mime: String,
    #[serde(default)]
    pub cid: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OutboxRow {
    pub id: i64,
    pub account_id: i64,
    pub mailbox_id: Option<i64>,
    pub thread_id: Option<i64>,
    pub reply_to_message_id: Option<i64>,
    /// `new | reply | reply_all | forward`
    pub kind: String,
    pub from_addr: String,
    pub to: Vec<Recipient>,
    pub cc: Vec<Recipient>,
    pub bcc: Vec<Recipient>,
    pub subject: String,
    pub body_text: Option<String>,
    pub body_html: Option<String>,
    /// The TipTap document: the draft's source of truth.
    pub body_json: Option<serde_json::Value>,
    pub quoted_html: Option<String>,
    pub attachments: Vec<OutboxAttachment>,
    pub in_reply_to: Option<String>,
    pub references: Vec<String>,
    /// `draft | queued | sending | sent | failed`
    pub state: String,
    pub attempts: i64,
    pub last_error: Option<String>,
    pub ses_message_id: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    pub sent_at: Option<i64>,
}

/// What a save writes. JSON columns are already serialised by the caller.
pub(crate) struct OutboxWrite {
    pub account_id: i64,
    pub mailbox_id: Option<i64>,
    pub thread_id: Option<i64>,
    pub reply_to_message_id: Option<i64>,
    pub kind: String,
    pub from_addr: String,
    pub to_json: String,
    pub cc_json: String,
    pub bcc_json: String,
    pub subject: String,
    pub body_text: Option<String>,
    pub body_html: Option<String>,
    pub body_json: Option<String>,
    pub quoted_html: Option<String>,
    pub attachments_json: String,
    pub in_reply_to: Option<String>,
    pub references_json: String,
}

const OUTBOX_COLUMNS: &str = "id, account_id, mailbox_id, thread_id, reply_to_message_id, kind,
    from_addr, to_json, cc_json, bcc_json, subject, body_text, body_html, body_json,
    quoted_html, attachments_json, in_reply_to, references_json, state, attempts,
    last_error, ses_message_id, created_at, updated_at, sent_at";

fn outbox_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<OutboxRow> {
    fn list<T: serde::de::DeserializeOwned>(text: String) -> Vec<T> {
        serde_json::from_str(&text).unwrap_or_default()
    }
    Ok(OutboxRow {
        id: row.get(0)?,
        account_id: row.get(1)?,
        mailbox_id: row.get(2)?,
        thread_id: row.get(3)?,
        reply_to_message_id: row.get(4)?,
        kind: row.get(5)?,
        from_addr: row.get(6)?,
        to: list(row.get(7)?),
        cc: list(row.get(8)?),
        bcc: list(row.get(9)?),
        subject: row.get(10)?,
        body_text: row.get(11)?,
        body_html: row.get(12)?,
        body_json: row
            .get::<_, Option<String>>(13)?
            .and_then(|t| serde_json::from_str(&t).ok()),
        quoted_html: row.get(14)?,
        attachments: list(row.get(15)?),
        in_reply_to: row.get(16)?,
        references: list(row.get(17)?),
        state: row.get(18)?,
        attempts: row.get(19)?,
        last_error: row.get(20)?,
        ses_message_id: row.get(21)?,
        created_at: row.get(22)?,
        updated_at: row.get(23)?,
        sent_at: row.get(24)?,
    })
}

pub(crate) fn insert_outbox(conn: &Connection, w: &OutboxWrite) -> rusqlite::Result<i64> {
    let now = now_millis();
    conn.query_row(
        "INSERT INTO mail_outbox
           (account_id, mailbox_id, thread_id, reply_to_message_id, kind, from_addr,
            to_json, cc_json, bcc_json, subject, body_text, body_html, body_json,
            quoted_html, attachments_json, in_reply_to, references_json,
            created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17,
                 ?18, ?18)
         RETURNING id",
        params![
            w.account_id,
            w.mailbox_id,
            w.thread_id,
            w.reply_to_message_id,
            w.kind,
            w.from_addr,
            w.to_json,
            w.cc_json,
            w.bcc_json,
            w.subject,
            w.body_text,
            w.body_html,
            w.body_json,
            w.quoted_html,
            w.attachments_json,
            w.in_reply_to,
            w.references_json,
            now
        ],
        |row| row.get(0),
    )
}

/// Rewrites a draft or a failed send. A `failed` row goes back to `draft`
/// (the person edited it, so it is theirs to send again). An `unconfirmed` row
/// keeps its state and error: its content is saved, so no edit is lost, but a
/// save is also what a composer does on a timer or on open, and that must
/// never put a possibly-delivered message back in reach of Send; only
/// [`rearm_outbox`] does that. Any other state is left alone and reported as
/// not updated, because a row being sent or already sent must not be changed
/// under the send.
pub(crate) fn update_outbox(conn: &Connection, id: i64, w: &OutboxWrite) -> rusqlite::Result<bool> {
    Ok(conn.execute(
        "UPDATE mail_outbox SET
           account_id = ?2, mailbox_id = ?3, thread_id = ?4, reply_to_message_id = ?5,
           kind = ?6, from_addr = ?7, to_json = ?8, cc_json = ?9, bcc_json = ?10,
           subject = ?11, body_text = ?12, body_html = ?13, body_json = ?14,
           quoted_html = ?15, attachments_json = ?16, in_reply_to = ?17,
           references_json = ?18,
           state = CASE WHEN state = 'unconfirmed' THEN state ELSE 'draft' END,
           last_error = CASE WHEN state = 'unconfirmed' THEN last_error ELSE NULL END,
           updated_at = ?19
         WHERE id = ?1 AND state IN ('draft', 'queued', 'failed', 'unconfirmed')",
        params![
            id,
            w.account_id,
            w.mailbox_id,
            w.thread_id,
            w.reply_to_message_id,
            w.kind,
            w.from_addr,
            w.to_json,
            w.cc_json,
            w.bcc_json,
            w.subject,
            w.body_text,
            w.body_html,
            w.body_json,
            w.quoted_html,
            w.attachments_json,
            w.in_reply_to,
            w.references_json,
            now_millis()
        ],
    )? > 0)
}

/// Every attachment path any outbox row still points at, sent rows included.
pub(crate) fn outbox_attachment_paths(conn: &Connection) -> rusqlite::Result<Vec<String>> {
    let mut stmt = conn.prepare("SELECT attachments_json FROM mail_outbox")?;
    let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
    let mut out = Vec::new();
    for json in rows {
        // A row whose JSON does not parse lists nothing here, the same as
        // `outbox_from_row` shows it. In a delete path that is fail-open (its
        // files look unused), accepted because the row is already unreadable
        // everywhere else and nothing writes it except `serde_json`.
        let list: Vec<OutboxAttachment> = serde_json::from_str(&json?).unwrap_or_default();
        out.extend(list.into_iter().map(|a| a.path));
    }
    Ok(out)
}

/// The one way out of `unconfirmed`.
pub(crate) fn rearm_outbox(conn: &Connection, id: i64) -> rusqlite::Result<bool> {
    Ok(conn.execute(
        "UPDATE mail_outbox SET state = 'draft', last_error = NULL, updated_at = ?2
         WHERE id = ?1 AND state = 'unconfirmed'",
        params![id, now_millis()],
    )? > 0)
}

pub(crate) fn get_outbox(conn: &Connection, id: i64) -> rusqlite::Result<Option<OutboxRow>> {
    conn.query_row(
        &format!("SELECT {OUTBOX_COLUMNS} FROM mail_outbox WHERE id = ?1"),
        params![id],
        outbox_from_row,
    )
    .optional()
}

/// Newest first. Capped, because sent rows are bookkeeping that only grows.
pub(crate) fn list_outbox(conn: &Connection) -> rusqlite::Result<Vec<OutboxRow>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {OUTBOX_COLUMNS} FROM mail_outbox ORDER BY updated_at DESC, id DESC LIMIT 200"
    ))?;
    let rows = stmt.query_map([], outbox_from_row)?;
    rows.collect()
}

/// A row being sent cannot be deleted: the send would then record its result
/// against nothing.
pub(crate) fn delete_outbox(conn: &Connection, id: i64) -> rusqlite::Result<bool> {
    Ok(conn.execute(
        "DELETE FROM mail_outbox WHERE id = ?1 AND state <> 'sending'",
        params![id],
    )? > 0)
}

/// Moves a sendable row to `sending`. Exactly one caller wins: a double click,
/// or two windows, must not put the same email on the wire twice.
pub(crate) fn claim_outbox_for_send(conn: &Connection, id: i64) -> rusqlite::Result<bool> {
    Ok(conn.execute(
        "UPDATE mail_outbox SET state = 'sending', attempts = attempts + 1,
           last_error = NULL, updated_at = ?2
         WHERE id = ?1 AND state IN ('draft', 'queued', 'failed')",
        params![id, now_millis()],
    )? > 0)
}

pub(crate) fn mark_outbox_sent(
    conn: &Connection,
    id: i64,
    ses_message_id: &str,
) -> rusqlite::Result<bool> {
    let now = now_millis();
    Ok(conn.execute(
        "UPDATE mail_outbox SET state = 'sent', ses_message_id = ?2, sent_at = ?3,
           updated_at = ?3, last_error = NULL WHERE id = ?1 AND state = 'sending'",
        params![id, ses_message_id, now],
    )? > 0)
}

pub(crate) fn set_outbox_thread(
    conn: &Connection,
    id: i64,
    thread_id: i64,
) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE mail_outbox SET thread_id = ?2 WHERE id = ?1",
        params![id, thread_id],
    )?;
    Ok(())
}

/// `state` is `failed` when SES certainly did not take the message, and
/// `unconfirmed` when it may have. The second is not claimable: sending it
/// again takes an explicit save, so a Retry button cannot double-send.
pub(crate) fn mark_outbox_failed(
    conn: &Connection,
    id: i64,
    error: &str,
    state: &str,
) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE mail_outbox SET state = ?4, last_error = ?2, updated_at = ?3
         WHERE id = ?1 AND state = 'sending'",
        params![id, error, now_millis(), state],
    )?;
    Ok(())
}

/// A row still `sending` at startup was cut off by a quit or a crash, and
/// nobody knows whether SES took it. It becomes `unconfirmed`, so the
/// person decides; it is never retried on its own.
pub(crate) fn fail_interrupted_sends(conn: &Connection) -> rusqlite::Result<usize> {
    conn.execute(
        "UPDATE mail_outbox SET state = 'unconfirmed', updated_at = ?1,
           last_error = 'Monoptah quit while this was being sent. It may have been delivered, so check the recipient before sending it again.'
         WHERE state = 'sending'",
        params![now_millis()],
    )
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
    fn ddl_alone_carries_parse_attempts() {
        // Not `ensure_tables`: its ALTER would add the column regardless.
        let conn = Connection::open_in_memory().unwrap();
        create_tables(&conn).unwrap();
        let n: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('mail_object') WHERE name='parse_attempts'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 1);
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
    fn ensure_object_attempts_adds_the_column_to_an_older_database() {
        let conn = Connection::open_in_memory().unwrap();
        // mail_object as it was before `parse_attempts`.
        conn.execute_batch(
            "CREATE TABLE mail_object (id INTEGER PRIMARY KEY, mailbox_id INTEGER NOT NULL,
               s3_key TEXT NOT NULL, etag TEXT, size_bytes INTEGER, s3_modified INTEGER,
               first_seen_at INTEGER NOT NULL, last_seen_at INTEGER NOT NULL,
               state TEXT NOT NULL, parse_error TEXT, UNIQUE (mailbox_id, s3_key));
             INSERT INTO mail_object (mailbox_id, s3_key, first_seen_at, last_seen_at, state)
             VALUES (1, 'k', 1, 1, 'parsed');",
        )
        .unwrap();
        let columns = |conn: &Connection| -> i64 {
            conn.query_row(
                "SELECT COUNT(*) FROM pragma_table_info('mail_object') WHERE name='parse_attempts'",
                [],
                |r| r.get(0),
            )
            .unwrap()
        };
        assert_eq!(columns(&conn), 0);
        ensure_object_attempts(&conn).unwrap();
        ensure_object_attempts(&conn).unwrap();
        assert_eq!(columns(&conn), 1);
        let attempts: i64 = conn
            .query_row("SELECT parse_attempts FROM mail_object", [], |r| r.get(0))
            .unwrap();
        assert_eq!(attempts, 0);
    }

    #[test]
    fn ensure_tables_adds_identities_refreshed_at_to_an_older_database() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        // mail_account as the first dev builds created it, before the column.
        conn.execute_batch(
            "CREATE TABLE mail_credential (id INTEGER PRIMARY KEY, provider TEXT NOT NULL,
               label TEXT NOT NULL, access_key_masked TEXT NOT NULL, default_region TEXT,
               created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL, UNIQUE (provider, label));
             CREATE TABLE mail_account (id INTEGER PRIMARY KEY, provider TEXT NOT NULL,
               credential_id INTEGER NOT NULL REFERENCES mail_credential(id) ON DELETE RESTRICT,
               s3_credential_id INTEGER REFERENCES mail_credential(id) ON DELETE RESTRICT,
               label TEXT NOT NULL, ses_region TEXT NOT NULL, bucket TEXT NOT NULL,
               s3_region TEXT NOT NULL, default_from TEXT,
               attach_policy TEXT NOT NULL DEFAULT 'on_demand',
               image_policy TEXT NOT NULL DEFAULT 'ask', enabled INTEGER NOT NULL DEFAULT 1,
               sort_order INTEGER NOT NULL DEFAULT 0, created_at INTEGER NOT NULL,
               updated_at INTEGER NOT NULL,
               UNIQUE (credential_id, ses_region, bucket, s3_region));",
        )
        .unwrap();
        ensure_tables(&conn).unwrap();
        ensure_tables(&conn).unwrap();
        let cred = upsert_credential(&conn, &credential("a")).unwrap();
        let id = upsert_account(&conn, &account(cred, "A", "bucket-a")).unwrap();
        upsert_mailboxes(&conn, id, &[mailbox("o@example.com", "o/")]).unwrap();
        replace_identities(&conn, id, &[]).unwrap();
        assert!(identities_refreshed_at(&conn, id).unwrap().is_some());
    }

    #[test]
    fn two_addresses_cannot_share_a_prefix_within_a_source() {
        let store = SessionStore::open_in_memory().unwrap();
        let conn = store.lock_conn().unwrap();
        let cred = upsert_credential(&conn, &credential("a")).unwrap();
        let id = upsert_account(&conn, &account(cred, "A", "bucket-a")).unwrap();
        upsert_mailboxes(&conn, id, &[mailbox("o@example.com", "o/")]).unwrap();
        let error = upsert_mailboxes(&conn, id, &[mailbox("p@example.com", "o/")]).unwrap_err();
        assert!(error.to_string().contains("o@example.com"), "{error}");
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

    // ── rules and smart folders ────────────────────────────────────

    struct Msg<'a> {
        subject: &'a str,
        from_name: &'a str,
        from_addr: &'a str,
        to: &'a str,
        cc: &'a str,
        body: &'a str,
        date: i64,
    }

    impl Default for Msg<'_> {
        fn default() -> Self {
            Msg {
                subject: "s",
                from_name: "",
                from_addr: "x@example.com",
                to: "[]",
                cc: "[]",
                body: "",
                date: 1_700_000_000,
            }
        }
    }

    /// One credential, one source, `boxes` mailboxes; returns the mailbox ids.
    fn seed_boxes(conn: &Connection, boxes: &[(&str, &str)]) -> Vec<i64> {
        let cred = upsert_credential(conn, &credential("a")).unwrap();
        let acct = upsert_account(conn, &account(cred, "Alpha mail", "bucket-a")).unwrap();
        let inputs: Vec<MailboxInput> = boxes.iter().map(|(a, p)| mailbox(a, p)).collect();
        upsert_mailboxes(conn, acct, &inputs).unwrap();
        list_mailboxes(conn, Some(acct))
            .unwrap()
            .iter()
            .map(|m| m.id)
            .collect()
    }

    /// A one-message thread, the way ingest leaves it, then re-evaluated
    /// against the folders as the sync loop does.
    fn arrive(conn: &Connection, mailbox_id: i64, m: &Msg) -> (i64, i64) {
        let thread = insert_thread(conn, mailbox_id, m.subject, m.subject, m.date).unwrap();
        let hdr = format!("<{}-{}@t>", m.subject, thread);
        let from_name = (!m.from_name.is_empty()).then_some(m.from_name);
        let id = insert_message(
            conn,
            &NewMessage {
                mailbox_id,
                thread_id: thread,
                object_id: None,
                direction: "inbound",
                message_id_hdr: Some(&hdr),
                in_reply_to: None,
                subject: m.subject,
                subject_key: m.subject,
                from_name,
                from_addr: m.from_addr,
                reply_to_addr: None,
                to_json: m.to,
                cc_json: m.cc,
                bcc_json: "[]",
                date_utc: m.date,
                received_at: m.date,
                snippet: "",
                body_text: Some(m.body),
                body_html: None,
                has_attachments: false,
                size_bytes: 1,
                spf: None,
                dkim: None,
                dmarc: None,
                headers_json: "[]",
            },
        )
        .unwrap();
        crate::mail_thread::refresh_thread(conn, thread).unwrap();
        reevaluate_thread(conn, thread).unwrap();
        (id, thread)
    }

    fn rule(field: &str, op: &str, value: serde_json::Value) -> Rule {
        Rule {
            field: field.into(),
            op: op.into(),
            value,
            negate: false,
        }
    }

    fn not(mut r: Rule) -> Rule {
        r.negate = true;
        r
    }

    fn s(text: &str) -> serde_json::Value {
        serde_json::Value::String(text.into())
    }

    fn list_rules(conn: &Connection, rules: &[Rule], mode: MatchMode) -> Vec<String> {
        let page = list_threads(
            conn,
            &ThreadQuery {
                scope: ThreadScope::Rules {
                    rules: rules.to_vec(),
                    match_mode: mode,
                },
                cursor: None,
                limit: Some(200),
            },
        )
        .unwrap();
        let mut subjects: Vec<String> = page.rows.into_iter().map(|t| t.subject).collect();
        subjects.sort();
        subjects
    }

    fn folder(name: &str) -> FolderInput {
        FolderInput {
            id: None,
            name: name.into(),
            color: None,
            sort_order: None,
            match_mode: MatchMode::All,
            hide_from_mailbox: false,
            count_unread: true,
        }
    }

    fn members(conn: &Connection, folder_id: i64) -> Vec<String> {
        let mut stmt = conn
            .prepare(
                "SELECT t.subject FROM mail_folder_member fm
                 JOIN mail_thread t ON t.id = fm.thread_id
                 WHERE fm.folder_id = ?1 ORDER BY t.subject",
            )
            .unwrap();
        let rows = stmt.query_map([folder_id], |r| r.get(0)).unwrap();
        rows.map(|r| r.unwrap()).collect()
    }

    fn folder_listing(conn: &Connection, folder_id: i64) -> Vec<String> {
        let page = list_threads(
            conn,
            &ThreadQuery {
                scope: ThreadScope::Folder { folder_id },
                cursor: None,
                limit: Some(200),
            },
        )
        .unwrap();
        let mut subjects: Vec<String> = page.rows.into_iter().map(|t| t.subject).collect();
        subjects.sort();
        subjects
    }

    #[test]
    fn every_field_and_operator_pair_compiles_to_bound_sql() {
        let store = SessionStore::open_in_memory().unwrap();
        let conn = store.lock_conn().unwrap();
        let fields: Vec<&str> = FIELD_OPS.iter().map(|(f, _)| *f).collect();
        // Every operator any field admits, so a new one is exercised by default.
        let mut ops: Vec<&str> = Vec::new();
        for (_, field_ops) in FIELD_OPS {
            for op in *field_ops {
                if !ops.contains(op) {
                    ops.push(op);
                }
            }
        }
        for relative in CLOCK_RELATIVE_OPS {
            assert!(ops.contains(relative), "{relative} is not an operator");
        }
        let mut valid = 0;
        assert_eq!(fields.len(), 13);
        for field in fields {
            let allowed = allowed_ops(field).unwrap();
            for op in &ops {
                let op = *op;
                let value = match (field, op) {
                    (_, "contains_any") => serde_json::json!(["SENTINEL'a", "SENTINEL%b"]),
                    ("date", "within_days") => serde_json::json!(30),
                    ("date", _) => s("2024-01-31"),
                    (_, "true") => serde_json::Value::Null,
                    _ => s("SENTINEL'x*%_"),
                };
                let r = rule(field, op, value);
                let compiled =
                    compile_rules(std::slice::from_ref(&r), MatchMode::All, 1_700_000_000);
                if !allowed.contains(&op) {
                    let err = compiled.err().unwrap_or_else(|| panic!("{field} {op}"));
                    assert!(err.to_string().contains(op), "{err}");
                    continue;
                }
                valid += 1;
                for negate in [false, true] {
                    let mut r = r.clone();
                    r.negate = negate;
                    let c = compile_rules(&[r], MatchMode::All, 1_700_000_000)
                        .unwrap_or_else(|e| panic!("{field} {op}: {e}"))
                        .unwrap();
                    assert!(!c.sql.contains("SENTINEL"), "{field} {op}: {}", c.sql);
                    assert_eq!(
                        c.sql.matches('?').count(),
                        c.params.len(),
                        "{field} {op}: {}",
                        c.sql
                    );
                    // The database is the judge of "valid SQL".
                    let sql = format!(
                        "SELECT COUNT(*) FROM mail_thread t
                         JOIN mail_mailbox b ON b.id = t.mailbox_id
                         JOIN mail_account a ON a.id = t.account_id WHERE {}",
                        c.sql
                    );
                    conn.query_row(&sql, rusqlite::params_from_iter(c.params), |r| {
                        r.get::<_, i64>(0)
                    })
                    .unwrap_or_else(|e| panic!("{field} {op}: {e}\n{sql}"));
                }
            }
        }
        // from/to/cc/recipient 6 each, subject 7, body 3, mailbox 2,
        // domain and account 3 each, three flags, date 3.
        assert_eq!(valid, 4 * 6 + 7 + 3 + 2 + 3 + 3 + 3 + 3);
        assert!(allowed_ops("nonsense").is_none());
        let err = compile_rules(&[rule("nonsense", "is", s("x"))], MatchMode::All, 0).err();
        assert!(err.unwrap().to_string().contains("nonsense"));
        assert!(compile_rules(&[rule("from", "is", s("  "))], MatchMode::All, 0).is_err());
        assert!(compile_rules(&[], MatchMode::All, 0).unwrap().is_none());
    }

    #[test]
    fn like_wildcards_quotes_and_stars_match_literally() {
        let store = SessionStore::open_in_memory().unwrap();
        let conn = store.lock_conn().unwrap();
        let mb = seed_boxes(&conn, &[("o@example.com", "o/")])[0];
        let to = |name: &str| format!(r#"[{{"name":"{name}","addr":"p@example.com"}}]"#);
        let to_literal = to("50%_off*'");
        let to_wild = to("50xyoff*'");
        let to_back = to(r"back\\slash");
        for (subject, name, to_json) in [
            ("literal", "50%_off*'", "[]"),
            ("wild", "50xyoff*'", "[]"),
            ("backslash", r"back\slash", "[]"),
            ("to-literal", "", to_literal.as_str()),
            ("to-wild", "", to_wild.as_str()),
            ("to-back", "", to_back.as_str()),
        ] {
            arrive(
                &conn,
                mb,
                &Msg {
                    subject,
                    from_name: name,
                    to: to_json,
                    ..Msg::default()
                },
            );
        }
        let find = |field: &str, op: &str, v: &str| {
            list_rules(&conn, &[rule(field, op, s(v))], MatchMode::All)
        };
        for field in ["from", "to"] {
            let (lit, wild) = if field == "from" {
                ("literal", "wild")
            } else {
                ("to-literal", "to-wild")
            };
            // `%` and `_` are wildcards to LIKE: they would also match `wild`.
            assert_eq!(find(field, "contains", "50%_off*'"), [lit]);
            assert_eq!(find(field, "starts_with", "50%_"), [lit]);
            assert_eq!(find(field, "ends_with", "off*'"), [lit, wild]);
            assert_eq!(find(field, "ends_with", "%_off*'"), [lit]);
            assert_eq!(find(field, "is", "50%_off*'"), [lit]);
            assert_eq!(find(field, "contains", "%"), [lit]);
            assert_eq!(find(field, "contains", "_"), [lit]);
            // `*` is an operator to GLOB and FTS, not to `contains`.
            assert_eq!(find(field, "contains", "f*'"), [lit, wild]);
            assert!(find(field, "contains", "5*").is_empty());
        }
        assert_eq!(find("from", "contains", r"\"), ["backslash"]);
        assert_eq!(find("to", "contains", r"\"), ["to-back"]);
        // `matches` is GLOB on purpose: here `*` is the user's wildcard.
        assert_eq!(find("from", "matches", "50*off*'").len(), 2);
    }

    #[test]
    fn fts_values_cannot_change_the_query() {
        let store = SessionStore::open_in_memory().unwrap();
        let conn = store.lock_conn().unwrap();
        let mb = seed_boxes(&conn, &[("o@example.com", "o/")])[0];
        for subject in ["foo bar", "fo", "alpha or beta", "alpha", "50%_off*'"] {
            arrive(
                &conn,
                mb,
                &Msg {
                    subject,
                    ..Msg::default()
                },
            );
        }
        let find =
            |v: &str| list_rules(&conn, &[rule("subject", "contains", s(v))], MatchMode::All);
        // No prefix operator: `fo*` is the word `fo`, not `foo`.
        assert_eq!(find("fo*"), ["fo"]);
        // `OR` inside a value is a word, so it is not an alternative.
        assert_eq!(find("alpha OR beta"), ["alpha or beta"]);
        // Quote, paren, colon and minus are all just separators.
        for hostile in [
            "\"",
            "a\" OR \"b",
            "(",
            ")",
            "subject:",
            "-x",
            "NEAR(",
            "^a",
        ] {
            find(hostile);
        }
        // A value with no indexable character matches nothing, without an error.
        assert!(find("%").is_empty());
        assert_eq!(find("50%_off*'"), ["50%_off*'"]);
    }

    #[test]
    fn negate_and_match_mode_combine_with_the_right_precedence() {
        let store = SessionStore::open_in_memory().unwrap();
        let conn = store.lock_conn().unwrap();
        let mb = seed_boxes(&conn, &[("o@example.com", "o/")])[0];
        // (from, subject): A alice/invoice, B bob/invoice, C alice/hello,
        // D bob/hello, E carol/other.
        for (label, addr, subject_word) in [
            ("A", "alice@example.com", "invoice"),
            ("B", "bob@example.com", "invoice"),
            ("C", "alice@example.com", "hello"),
            ("D", "bob@example.com", "hello"),
            ("E", "carol@example.com", "other"),
        ] {
            let subject: &'static str = Box::leak(format!("{label} {subject_word}").into());
            arrive(
                &conn,
                mb,
                &Msg {
                    subject,
                    from_addr: addr,
                    ..Msg::default()
                },
            );
        }
        let alice = rule("from", "is", s("alice@example.com"));
        let invoice = rule("subject", "contains", s("invoice"));
        let letters = |rules: &[Rule], mode| -> String {
            list_rules(&conn, rules, mode)
                .iter()
                .map(|s| s.chars().next().unwrap())
                .collect()
        };
        assert_eq!(
            letters(&[alice.clone(), invoice.clone()], MatchMode::All),
            "A"
        );
        assert_eq!(
            letters(&[alice.clone(), invoice.clone()], MatchMode::Any),
            "ABC"
        );
        // NOT (alice OR invoice): both negated under `all`.
        let both_not = [not(alice.clone()), not(invoice.clone())];
        assert_eq!(letters(&both_not, MatchMode::All), "DE");
        // (NOT alice) OR (NOT invoice): the same rules under `any`.
        assert_eq!(letters(&both_not, MatchMode::Any), "BCDE");
        // Negating one rule only: alice, but not an invoice.
        assert_eq!(
            letters(&[alice.clone(), not(invoice.clone())], MatchMode::All),
            "C"
        );
        // A negated list is NOT (a OR b).
        let hello_or_other = rule(
            "subject",
            "contains_any",
            serde_json::json!(["hello", "other"]),
        );
        assert_eq!(letters(&[not(hello_or_other)], MatchMode::All), "AB");
    }

    #[test]
    fn a_negated_rule_means_no_message_of_the_thread_matches() {
        let store = SessionStore::open_in_memory().unwrap();
        let conn = store.lock_conn().unwrap();
        let mb = seed_boxes(&conn, &[("o@example.com", "o/")])[0];
        let (_, mixed) = arrive(
            &conn,
            mb,
            &Msg {
                subject: "mixed",
                from_addr: "alice@example.com",
                ..Msg::default()
            },
        );
        let second = Msg {
            subject: "mixed",
            from_addr: "bob@example.com",
            ..Msg::default()
        };
        // A reply from bob in the same thread.
        insert_message(
            &conn,
            &NewMessage {
                mailbox_id: mb,
                thread_id: mixed,
                object_id: None,
                direction: "inbound",
                message_id_hdr: Some("<reply@t>"),
                in_reply_to: None,
                subject: "mixed",
                subject_key: "mixed",
                from_name: None,
                from_addr: second.from_addr,
                reply_to_addr: None,
                to_json: "[]",
                cc_json: "[]",
                bcc_json: "[]",
                date_utc: second.date + 10,
                received_at: second.date + 10,
                snippet: "",
                body_text: Some(""),
                body_html: None,
                has_attachments: false,
                size_bytes: 1,
                spf: None,
                dkim: None,
                dmarc: None,
                headers_json: "[]",
            },
        )
        .unwrap();
        let r = rule("from", "is", s("alice@example.com"));
        assert_eq!(
            list_rules(&conn, std::slice::from_ref(&r), MatchMode::All),
            ["mixed"]
        );
        assert!(list_rules(&conn, &[not(r)], MatchMode::All).is_empty());
        // The other sender is also "in" the thread.
        let bob = rule("from", "is", s("bob@example.com"));
        assert_eq!(list_rules(&conn, &[bob], MatchMode::All), ["mixed"]);
    }

    #[test]
    fn a_gone_message_cannot_make_a_thread_match() {
        let store = SessionStore::open_in_memory().unwrap();
        let conn = store.lock_conn().unwrap();
        let mb = seed_boxes(&conn, &[("o@example.com", "o/")])[0];
        let (msg, thread) = arrive(
            &conn,
            mb,
            &Msg {
                subject: "ghost",
                from_addr: "alice@example.com",
                body: "zebra",
                ..Msg::default()
            },
        );
        let rules = [rule("from", "is", s("alice@example.com"))];
        let by_body = [rule("body", "contains", s("zebra"))];
        assert_eq!(list_rules(&conn, &rules, MatchMode::All), ["ghost"]);
        conn.execute("UPDATE mail_message SET state='gone' WHERE id=?1", [msg])
            .unwrap();
        // Compare on the compiled condition, not the listing, which would hide
        // the thread anyway.
        for r in [&rules, &by_body] {
            let c = compile_rules(r, MatchMode::All, 0).unwrap().unwrap();
            let mut p = vec![rusqlite::types::Value::Integer(thread)];
            p.extend(c.params);
            let hit: bool = conn
                .query_row(
                    &format!(
                        "SELECT EXISTS (SELECT 1 FROM mail_thread t
                         JOIN mail_mailbox b ON b.id = t.mailbox_id
                         JOIN mail_account a ON a.id = t.account_id
                         WHERE t.id = ? AND {})",
                        c.sql
                    ),
                    rusqlite::params_from_iter(p),
                    |row| row.get(0),
                )
                .unwrap();
            assert!(!hit);
        }
    }

    #[test]
    fn address_scope_flag_and_date_rules_select_the_right_threads() {
        let store = SessionStore::open_in_memory().unwrap();
        let conn = store.lock_conn().unwrap();
        seed_boxes(&conn, &[("o@example.com", "o/"), ("p@other.org", "p/")]);
        let ids: Vec<i64> = ["o@example.com", "p@other.org"]
            .iter()
            .map(|a| {
                conn.query_row("SELECT id FROM mail_mailbox WHERE address = ?1", [a], |r| {
                    r.get(0)
                })
                .unwrap()
            })
            .collect();
        let now = now_millis() / 1000;
        let day = 86_400;
        let to_ann = r#"[{"name":"Ann","addr":"ann@x.com"}]"#;
        let cc_cy = r#"[{"name":"","addr":"cy@x.com"}]"#;
        arrive(
            &conn,
            ids[0],
            &Msg {
                subject: "recent",
                to: to_ann,
                cc: cc_cy,
                date: now - 2 * day,
                ..Msg::default()
            },
        );
        let (_, old) = arrive(
            &conn,
            ids[1],
            &Msg {
                subject: "old",
                date: 1_704_067_200,
                ..Msg::default()
            },
        );
        conn.execute("UPDATE mail_thread SET is_starred = 1 WHERE id = ?1", [old])
            .unwrap();
        let find = |r: Rule| list_rules(&conn, &[r], MatchMode::All);
        assert_eq!(find(rule("to", "is", s("ann@x.com"))), ["recent"]);
        assert_eq!(find(rule("to", "contains", s("ann"))), ["recent"]);
        assert!(find(rule("to", "is", s("cy@x.com"))).is_empty());
        assert_eq!(find(rule("cc", "is", s("cy@x.com"))), ["recent"]);
        assert_eq!(find(rule("recipient", "is", s("cy@x.com"))), ["recent"]);
        assert_eq!(find(rule("recipient", "is", s("ann@x.com"))), ["recent"]);
        assert_eq!(find(rule("mailbox", "is", s(&ids[1].to_string()))), ["old"]);
        assert_eq!(find(rule("mailbox", "is", s("O@example.com"))), ["recent"]);
        assert_eq!(find(rule("domain", "is", s("@Other.org"))), ["old"]);
        assert_eq!(
            find(rule(
                "domain",
                "contains_any",
                serde_json::json!(["other.org", "example.com"])
            ))
            .len(),
            2
        );
        assert_eq!(find(rule("account", "is", s("alpha MAIL"))).len(), 2);
        assert_eq!(find(rule("domain", "contains", s("THER"))), ["old"]);
        assert_eq!(find(rule("domain", "contains", s("@example"))), ["recent"]);
        assert!(find(rule("domain", "contains", s("%"))).is_empty());
        assert_eq!(find(rule("account", "contains", s("lpha"))).len(), 2);
        assert!(find(rule("account", "contains", s("_"))).is_empty());
        assert_eq!(find(rule("is_starred", "true", s(""))), ["old"]);
        assert_eq!(find(rule("is_unread", "true", s(""))).len(), 2);
        assert!(find(rule("has_attachment", "true", s(""))).is_empty());
        assert_eq!(
            find(rule("date", "within_days", serde_json::json!(7))),
            ["recent"]
        );
        // `after` includes its own midnight and `before` excludes it.
        assert_eq!(find(rule("date", "after", s("2024-01-01"))).len(), 2);
        assert!(find(rule("date", "before", s("2024-01-01"))).is_empty());
        assert_eq!(find(rule("date", "before", s("2024-01-02"))), ["old"]);
        assert_eq!(find(rule("date", "after", s("1704067201"))), ["recent"]);
        assert!(compile_rules(&[rule("date", "after", s("soon"))], MatchMode::All, 0).is_err());
    }

    #[test]
    fn diacritic_searches_match_in_either_spelling() {
        let store = SessionStore::open_in_memory().unwrap();
        let conn = store.lock_conn().unwrap();
        let mb = seed_boxes(&conn, &[("o@example.com", "o/")])[0];
        arrive(
            &conn,
            mb,
            &Msg {
                subject: "Đơn hàng đã được giao",
                body: "Hàng đến nơi",
                ..Msg::default()
            },
        );
        let find = |field: &str, v: &str| {
            list_rules(&conn, &[rule(field, "contains", s(v))], MatchMode::All).len()
        };
        for word in ["Đơn", "đơn", "don", "DON", "được", "duoc", "Đơn hàng"] {
            assert_eq!(find("subject", word), 1, "subject {word}");
        }
        for word in ["đến", "den", "Đến", "hang"] {
            assert_eq!(find("body", word), 1, "body {word}");
        }
        assert_eq!(find("subject", "dơn"), 1);
        assert_eq!(find("subject", "dan"), 0);
    }

    #[test]
    fn a_new_folder_collects_stored_mail_and_later_arrivals() {
        let store = SessionStore::open_in_memory().unwrap();
        let conn = store.lock_conn().unwrap();
        let mb = seed_boxes(&conn, &[("o@example.com", "o/")])[0];
        let billing = |subject| Msg {
            subject,
            from_addr: "pay@billing.test",
            ..Msg::default()
        };
        arrive(&conn, mb, &billing("stored invoice"));
        arrive(
            &conn,
            mb,
            &Msg {
                subject: "chat",
                ..Msg::default()
            },
        );
        let f = upsert_folder(
            &conn,
            &folder("Billing"),
            &[rule("from", "ends_with", s("@billing.test"))],
        )
        .unwrap();
        assert_eq!(members(&conn, f.id), ["stored invoice"]);
        assert_eq!((f.thread_count, f.unread_count), (1, 1));

        // Mail that arrives afterwards is filed on arrival.
        arrive(&conn, mb, &billing("later invoice"));
        arrive(
            &conn,
            mb,
            &Msg {
                subject: "chat 2",
                ..Msg::default()
            },
        );
        assert_eq!(members(&conn, f.id), ["later invoice", "stored invoice"]);
        assert_eq!(
            folder_listing(&conn, f.id),
            ["later invoice", "stored invoice"]
        );
        let listed = list_folders(&conn).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].thread_count, 2);
        assert_eq!(listed[0].rules.len(), 1);
    }

    #[test]
    fn editing_a_rule_rematerialises_the_members() {
        let store = SessionStore::open_in_memory().unwrap();
        let conn = store.lock_conn().unwrap();
        let mb = seed_boxes(&conn, &[("o@example.com", "o/")])[0];
        for (subject, addr) in [("from ann", "ann@x.com"), ("from bob", "bob@x.com")] {
            arrive(
                &conn,
                mb,
                &Msg {
                    subject,
                    from_addr: addr,
                    ..Msg::default()
                },
            );
        }
        let f = upsert_folder(
            &conn,
            &folder("People"),
            &[rule("from", "is", s("ann@x.com"))],
        )
        .unwrap();
        assert_eq!(members(&conn, f.id), ["from ann"]);
        let mut edit = folder("People");
        edit.id = Some(f.id);
        let f2 = upsert_folder(&conn, &edit, &[rule("from", "is", s("bob@x.com"))]).unwrap();
        assert_eq!(f2.id, f.id);
        assert_eq!(members(&conn, f.id), ["from bob"]);
        assert_eq!(f2.rules.len(), 1);

        // A rejected edit changes neither rules nor members.
        let bad = upsert_folder(&conn, &edit, &[rule("from", "true", s(""))]);
        assert!(bad.is_err());
        assert_eq!(members(&conn, f.id), ["from bob"]);
        assert_eq!(
            list_folders(&conn).unwrap()[0].rules[0].value,
            s("bob@x.com")
        );
        // Switching to `any` with a second rule widens it.
        edit.match_mode = MatchMode::Any;
        upsert_folder(
            &conn,
            &edit,
            &[
                rule("from", "is", s("bob@x.com")),
                rule("from", "is", s("ann@x.com")),
            ],
        )
        .unwrap();
        assert_eq!(members(&conn, f.id), ["from ann", "from bob"]);
    }

    #[test]
    fn contains_any_survives_storage_as_an_array() {
        let store = SessionStore::open_in_memory().unwrap();
        let conn = store.lock_conn().unwrap();
        let mb = seed_boxes(&conn, &[("o@example.com", "o/")])[0];
        arrive(
            &conn,
            mb,
            &Msg {
                subject: "tax form",
                ..Msg::default()
            },
        );
        arrive(
            &conn,
            mb,
            &Msg {
                subject: "receipt",
                ..Msg::default()
            },
        );
        arrive(
            &conn,
            mb,
            &Msg {
                subject: "lunch",
                ..Msg::default()
            },
        );
        let f = upsert_folder(
            &conn,
            &folder("Money"),
            &[rule(
                "subject",
                "contains_any",
                serde_json::json!(["tax", "receipt"]),
            )],
        )
        .unwrap();
        assert_eq!(f.rules[0].value, serde_json::json!(["tax", "receipt"]));
        assert_eq!(members(&conn, f.id), ["receipt", "tax form"]);
    }

    #[test]
    fn folder_validation_and_delete() {
        let store = SessionStore::open_in_memory().unwrap();
        let conn = store.lock_conn().unwrap();
        let mb = seed_boxes(&conn, &[("o@example.com", "o/")])[0];
        let (msg, _) = arrive(
            &conn,
            mb,
            &Msg {
                subject: "keep",
                ..Msg::default()
            },
        );
        let any = [rule("from", "contains", s("example"))];
        assert!(upsert_folder(&conn, &folder("  "), &any).is_err());
        assert!(upsert_folder(&conn, &folder("Empty"), &[]).is_err());
        let f = upsert_folder(&conn, &folder("Inbox2"), &any).unwrap();
        let clash = upsert_folder(&conn, &folder("inbox2"), &any).unwrap_err();
        assert!(clash.to_string().contains("Inbox2"), "{clash}");
        let second = upsert_folder(&conn, &folder("Other"), &any).unwrap();
        assert_eq!(second.sort_order, f.sort_order + 1);
        delete_folder(&conn, f.id).unwrap();
        let left = |sql: &str| -> i64 { conn.query_row(sql, [], |r| r.get(0)).unwrap() };
        assert_eq!(
            left(&format!(
                "SELECT COUNT(*) FROM mail_folder_rule WHERE folder_id={}",
                f.id
            )),
            0
        );
        assert_eq!(
            left(&format!(
                "SELECT COUNT(*) FROM mail_folder_member WHERE folder_id={}",
                f.id
            )),
            0
        );
        // The mail itself is untouched.
        assert_eq!(
            left(&format!("SELECT COUNT(*) FROM mail_message WHERE id={msg}")),
            1
        );
        assert_eq!(members(&conn, second.id), ["keep"]);
    }

    #[test]
    fn membership_never_brings_back_a_hidden_thread() {
        let store = SessionStore::open_in_memory().unwrap();
        let conn = store.lock_conn().unwrap();
        let mb = seed_boxes(&conn, &[("o@example.com", "o/")])[0];
        let mut threads = Vec::new();
        for subject in ["shown", "archived", "gone"] {
            threads.push(arrive(
                &conn,
                mb,
                &Msg {
                    subject,
                    ..Msg::default()
                },
            ));
        }
        let f = upsert_folder(
            &conn,
            &folder("All"),
            &[rule("from", "contains", s("example"))],
        )
        .unwrap();
        conn.execute(
            "UPDATE mail_message SET state='archived' WHERE id=?1",
            [threads[1].0],
        )
        .unwrap();
        conn.execute(
            "UPDATE mail_thread SET state='archived' WHERE id=?1",
            [threads[1].1],
        )
        .unwrap();
        conn.execute(
            "UPDATE mail_message SET state='gone' WHERE id=?1",
            [threads[2].0],
        )
        .unwrap();
        // A re-evaluation (a sync touching the thread) must not change that.
        for (_, t) in &threads {
            reevaluate_thread(&conn, *t).unwrap();
        }
        assert_eq!(folder_listing(&conn, f.id), ["shown"]);
        assert_eq!(list_folders(&conn).unwrap()[0].thread_count, 1);
    }

    #[test]
    fn a_folder_that_hides_from_the_mailbox_takes_threads_out_of_mailbox_views() {
        let store = SessionStore::open_in_memory().unwrap();
        let conn = store.lock_conn().unwrap();
        let mb = seed_boxes(&conn, &[("o@example.com", "o/")])[0];
        arrive(
            &conn,
            mb,
            &Msg {
                subject: "promo",
                from_addr: "ads@spam.test",
                ..Msg::default()
            },
        );
        arrive(
            &conn,
            mb,
            &Msg {
                subject: "real",
                ..Msg::default()
            },
        );
        arrive(
            &conn,
            mb,
            &Msg {
                subject: "ancient",
                date: 1_262_304_000,
                ..Msg::default()
            },
        );
        let mut input = folder("Promos");
        input.hide_from_mailbox = true;
        let f =
            upsert_folder(&conn, &input, &[rule("from", "ends_with", s("@spam.test"))]).unwrap();
        // A fixed-instant date rule is not clock-relative, so this folder hides
        // too. Guards `CLOCK_RELATIVE_OPS` against `before`/`after` being added.
        let mut old = folder("Old");
        old.hide_from_mailbox = true;
        upsert_folder(&conn, &old, &[rule("date", "before", s("2020-01-01"))]).unwrap();
        let listed = |scope| {
            let mut v: Vec<String> = list_threads(
                &conn,
                &ThreadQuery {
                    scope,
                    cursor: None,
                    limit: None,
                },
            )
            .unwrap()
            .rows
            .into_iter()
            .map(|t| t.subject)
            .collect();
            v.sort();
            v
        };
        assert_eq!(listed(ThreadScope::All), ["real"]);
        assert_eq!(listed(ThreadScope::Mailbox { mailbox_id: mb }), ["real"]);
        assert_eq!(unread_by_mailbox(&conn).unwrap()[&mb], 1);
        assert_eq!(listed(ThreadScope::Folder { folder_id: f.id }), ["promo"]);
        // Searching still finds it: hiding is for the mailbox views only.
        assert_eq!(
            list_rules(
                &conn,
                &[rule("subject", "contains", s("promo"))],
                MatchMode::All
            ),
            ["promo"]
        );
        // Deleting the folder returns the thread to the mailbox.
        delete_folder(&conn, f.id).unwrap();
        assert_eq!(listed(ThreadScope::All), ["promo", "real"]);
    }

    #[test]
    fn flag_rules_follow_mark_read_and_star() {
        let store = SessionStore::open_in_memory().unwrap();
        let conn = store.lock_conn().unwrap();
        let mb = seed_boxes(&conn, &[("o@example.com", "o/")])[0];
        let (_, t) = arrive(
            &conn,
            mb,
            &Msg {
                subject: "flagged",
                ..Msg::default()
            },
        );
        let unread = upsert_folder(
            &conn,
            &folder("Unread"),
            &[rule("is_unread", "true", s(""))],
        )
        .unwrap();
        let starred = upsert_folder(
            &conn,
            &folder("Starred"),
            &[rule("is_starred", "true", s(""))],
        )
        .unwrap();
        assert_eq!(members(&conn, unread.id), ["flagged"]);
        assert!(members(&conn, starred.id).is_empty());
        set_threads_read(&conn, &[t], true).unwrap();
        set_threads_starred(&conn, &[t], true).unwrap();
        assert!(members(&conn, unread.id).is_empty());
        assert_eq!(members(&conn, starred.id), ["flagged"]);
        set_threads_read(&conn, &[t], false).unwrap();
        assert_eq!(members(&conn, unread.id), ["flagged"]);
    }

    #[test]
    fn preview_count_equals_the_rows_the_listing_returns() {
        let store = SessionStore::open_in_memory().unwrap();
        let conn = store.lock_conn().unwrap();
        let mb = seed_boxes(&conn, &[("o@example.com", "o/")])[0];
        let mut hidden = Vec::new();
        for i in 0..7 {
            let subject: &'static str = Box::leak(format!("invoice {i}").into());
            let (m, t) = arrive(
                &conn,
                mb,
                &Msg {
                    subject,
                    date: 1_700_000_000 + i / 2,
                    ..Msg::default()
                },
            );
            hidden.push((m, t));
        }
        arrive(
            &conn,
            mb,
            &Msg {
                subject: "other",
                ..Msg::default()
            },
        );
        // Two matches the listing must not show.
        conn.execute(
            "UPDATE mail_message SET state='gone' WHERE id=?1",
            [hidden[0].0],
        )
        .unwrap();
        conn.execute(
            "UPDATE mail_message SET state='archived' WHERE id=?1",
            [hidden[1].0],
        )
        .unwrap();
        conn.execute(
            "UPDATE mail_thread SET state='archived' WHERE id=?1",
            [hidden[1].1],
        )
        .unwrap();
        for (rules, mode) in [
            (
                vec![rule("subject", "contains", s("invoice"))],
                MatchMode::All,
            ),
            (
                vec![not(rule("subject", "contains", s("invoice")))],
                MatchMode::All,
            ),
            (
                vec![
                    rule("subject", "contains", s("other")),
                    rule("subject", "contains", s("invoice")),
                ],
                MatchMode::Any,
            ),
            (vec![], MatchMode::All),
        ] {
            let previewed = preview_rules(&conn, &rules, mode).unwrap().count;
            // Walk every page, two rows at a time.
            let mut walked = 0;
            let mut cursor = None;
            loop {
                let page = list_threads(
                    &conn,
                    &ThreadQuery {
                        scope: ThreadScope::Rules {
                            rules: rules.clone(),
                            match_mode: mode,
                        },
                        cursor,
                        limit: Some(2),
                    },
                )
                .unwrap();
                walked += page.rows.len() as i64;
                cursor = page.next_cursor;
                if cursor.is_none() {
                    break;
                }
            }
            assert_eq!(previewed, walked, "{rules:?}");
        }
        let invoices = preview_rules(
            &conn,
            &[rule("subject", "contains", s("invoice"))],
            MatchMode::All,
        )
        .unwrap();
        assert_eq!(invoices.count, 5);
    }

    #[test]
    fn rule_json_from_the_webview_deserialises() {
        let rules: Vec<Rule> = serde_json::from_str(
            r#"[{"field":"from","op":"contains","value":"x"},
                {"field":"subject","op":"contains_any","value":["a","b"],"negate":true},
                {"field":"is_unread","op":"true"}]"#,
        )
        .unwrap();
        assert!(!rules[0].negate && rules[1].negate);
        let scope: ThreadScope =
            serde_json::from_str(r#"{"kind":"rules","rules":[{"field":"is_unread","op":"true"}]}"#)
                .unwrap();
        assert!(matches!(
            scope,
            ThreadScope::Rules {
                match_mode: MatchMode::All,
                ..
            }
        ));
        let scope: ThreadScope = serde_json::from_str(r#"{"kind":"folder","folderId":4}"#).unwrap();
        assert!(matches!(scope, ThreadScope::Folder { folder_id: 4 }));
    }

    #[test]
    fn contains_prefix_finds_half_typed_words_and_contains_does_not() {
        let store = SessionStore::open_in_memory().unwrap();
        let conn = store.lock_conn().unwrap();
        let mb = seed_boxes(&conn, &[("o@example.com", "o/")])[0];
        arrive(
            &conn,
            mb,
            &Msg {
                subject: "Đơn hàng invoice",
                body: "Your invoicing statement",
                ..Msg::default()
            },
        );
        arrive(
            &conn,
            mb,
            &Msg {
                subject: "lunch",
                ..Msg::default()
            },
        );
        let find = |field: &str, op: &str, v: &str| {
            list_rules(&conn, &[rule(field, op, s(v))], MatchMode::All).len()
        };
        for field in ["subject", "body"] {
            assert_eq!(find(field, "contains", "invo"), 0, "{field}");
            let word = if field == "subject" {
                "invoi"
            } else {
                "invoic"
            };
            assert_eq!(find(field, "contains_prefix", word), 1, "{field}");
            assert_eq!(find(field, "contains_prefix", "INVO"), 1, "{field}");
            assert_eq!(find(field, "contains_prefix", "zzz"), 0, "{field}");
        }
        // Folded like the index, and only the last token is a prefix.
        assert_eq!(find("subject", "contains_prefix", "đơ"), 1);
        assert_eq!(find("subject", "contains_prefix", "đơn hàng inv"), 1);
        assert_eq!(find("subject", "contains_prefix", "đ hàng inv"), 0);
        // Operators inside the value stay words.
        assert_eq!(find("subject", "contains_prefix", "inv*\" OR x"), 0);
    }

    #[test]
    fn a_folder_with_a_relative_date_rule_is_judged_by_the_clock_not_by_stale_members() {
        let store = SessionStore::open_in_memory().unwrap();
        let conn = store.lock_conn().unwrap();
        let mb = seed_boxes(&conn, &[("o@example.com", "o/")])[0];
        let now = now_millis() / 1000;
        let day = 86_400;
        let (m_recent, t_recent) = arrive(
            &conn,
            mb,
            &Msg {
                subject: "recent",
                date: now - day,
                ..Msg::default()
            },
        );
        arrive(
            &conn,
            mb,
            &Msg {
                subject: "old",
                date: now - 30 * day,
                ..Msg::default()
            },
        );
        let week = upsert_folder(
            &conn,
            &folder("Week"),
            &[rule("date", "within_days", serde_json::json!(7))],
        )
        .unwrap();
        let plain = upsert_folder(
            &conn,
            &folder("Plain"),
            &[rule("subject", "contains", s("recent"))],
        )
        .unwrap();
        assert_eq!(folder_listing(&conn, week.id), ["recent"]);
        assert_eq!(members(&conn, week.id), ["recent"]);

        // A month passes: the stored member is now 30 days old, and nothing
        // has touched the thread, so the member table still lists it.
        conn.execute(
            "UPDATE mail_message SET date_utc = ?2 WHERE id = ?1",
            params![m_recent, now - 30 * day],
        )
        .unwrap();
        assert_eq!(members(&conn, week.id), ["recent"]);
        assert!(folder_listing(&conn, week.id).is_empty());
        let row = list_folders(&conn)
            .unwrap()
            .into_iter()
            .find(|f| f.id == week.id)
            .unwrap();
        assert_eq!((row.thread_count, row.unread_count), (0, 0));
        // The same rule typed into the box agrees with the folder.
        let rules = [rule("date", "within_days", serde_json::json!(7))];
        assert_eq!(
            preview_rules(&conn, &rules, MatchMode::All).unwrap().count,
            0
        );

        // A folder without a time rule still reads its members.
        conn.execute(
            "DELETE FROM mail_folder_member WHERE folder_id = ?1",
            [plain.id],
        )
        .unwrap();
        assert!(folder_listing(&conn, plain.id).is_empty());
        conn.execute(
            "INSERT INTO mail_folder_member VALUES (?1, ?2)",
            [plain.id, t_recent],
        )
        .unwrap();
        assert_eq!(folder_listing(&conn, plain.id), ["recent"]);
    }

    #[test]
    fn a_relative_folder_that_hides_never_strands_a_thread() {
        let store = SessionStore::open_in_memory().unwrap();
        let conn = store.lock_conn().unwrap();
        let mb = seed_boxes(&conn, &[("o@example.com", "o/")])[0];
        let now = now_millis() / 1000;
        let day = 86_400;
        let (msg, _) = arrive(
            &conn,
            mb,
            &Msg {
                subject: "triage",
                date: now - day,
                ..Msg::default()
            },
        );
        let mut input = folder("Triage");
        input.hide_from_mailbox = true;
        let f = upsert_folder(
            &conn,
            &input,
            &[rule("date", "within_days", serde_json::json!(7))],
        )
        .unwrap();
        let listed = |scope| {
            list_threads(
                &conn,
                &ThreadQuery {
                    scope,
                    cursor: None,
                    limit: None,
                },
            )
            .unwrap()
            .rows
            .len()
        };
        // Fresh: in the folder, and the mailbox view does not hide it.
        assert_eq!(listed(ThreadScope::Folder { folder_id: f.id }), 1);
        assert_eq!(listed(ThreadScope::All), 1);
        // It ages out with the member row left behind.
        conn.execute(
            "UPDATE mail_message SET date_utc = ?2 WHERE id = ?1",
            params![msg, now - 30 * day],
        )
        .unwrap();
        assert_eq!(members(&conn, f.id), ["triage"]);
        assert_eq!(listed(ThreadScope::Folder { folder_id: f.id }), 0);
        assert_eq!(listed(ThreadScope::All), 1);
        assert_eq!(listed(ThreadScope::Mailbox { mailbox_id: mb }), 1);
        assert_eq!(unread_by_mailbox(&conn).unwrap()[&mb], 1);
    }
}
