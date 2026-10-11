//! Sending: the outbox, the pre-send checks, the SES call and what follows it
//! (spec section 8). Logic takes the store and the AWS seams, like `mail.rs`,
//! so tests run without Tauri, a keychain or a network.
//!
//! The order is the point. The message is built and checked first, where every
//! failure is a form error and changes nothing. Then the row is claimed, SES is
//! called once, and only after SES accepts does any bookkeeping happen. From
//! that moment nothing may report failure: an email that went out but looks
//! unsent gets sent again, and that cannot be undone.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use crate::aws_s3::S3ErrorKind;
use crate::aws_ses::SesErrorKind;
use crate::mail::{refresh_identities, AwsClients};
use crate::mail_build::{self, LoadedAttachment, Outgoing, SES_RAW_LIMIT};
use crate::mail_mime::{parse_mail, MailAddress};
use crate::mail_store::{
    self, MailAccount, MailMailbox, OutboxAttachment, OutboxRow, OutboxWrite, Recipient,
    SENT_PREFIX,
};
use crate::mail_thread;
use crate::session_store::{now_millis, SessionStore};

const KINDS: [&str; 4] = ["new", "reply", "reply_all", "forward"];

fn db(error: rusqlite::Error) -> String {
    error.to_string()
}

// ── drafts ─────────────────────────────────────────────────────────

/// What the composer saves. Threading headers are not here: they are derived
/// from `reply_to_message_id` on the way in, so the webview cannot send a
/// reply that links to the wrong conversation.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OutboxDraft {
    /// `Some` rewrites that draft (or failed send); `None` creates one.
    pub id: Option<i64>,
    pub kind: String,
    pub from_addr: String,
    pub reply_to_message_id: Option<i64>,
    #[serde(default)]
    pub to: Vec<Recipient>,
    #[serde(default)]
    pub cc: Vec<Recipient>,
    #[serde(default)]
    pub bcc: Vec<Recipient>,
    #[serde(default)]
    pub subject: String,
    /// The TipTap document.
    pub body_json: Option<serde_json::Value>,
    /// Derived by the composer's serializer, which needs this draft's id for
    /// its content ids: save once to get the id, serialize, save again.
    pub body_text: Option<String>,
    pub body_html: Option<String>,
    pub quoted_html: Option<String>,
    #[serde(default)]
    pub attachments: Vec<OutboxAttachment>,
}

/// Ids in a `References`-style header value, brackets removed.
fn ids_in(value: &str) -> Vec<String> {
    value
        .split_whitespace()
        .map(|t| t.trim_matches(|c| c == '<' || c == '>').trim().to_string())
        .filter(|t| !t.is_empty())
        .collect()
}

struct ReplyHeaders {
    thread_id: Option<i64>,
    in_reply_to: Option<String>,
    references: Vec<String>,
}

/// Thread, `In-Reply-To` and `References` for a reply to this message.
///
/// References are the parent's own `References` header read back from the
/// stored raw headers, then the parent's id. The `mail_message_ref` rows would
/// be easier to read but they list `In-Reply-To` first, which is the wrong end
/// of the chain, so they are only the fallback for a message stored without
/// its headers.
fn reply_headers(
    conn: &rusqlite::Connection,
    kind: &str,
    message_id: i64,
) -> Result<ReplyHeaders, String> {
    use rusqlite::OptionalExtension;
    let found: Option<(Option<String>, i64, Option<String>)> = conn
        .query_row(
            "SELECT message_id_hdr, thread_id, headers_json FROM mail_message WHERE id = ?1",
            [message_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .map_err(db)?;
    let Some((hdr, thread_id, headers_json)) = found else {
        return Err("The message you are replying to no longer exists.".into());
    };
    if kind != "reply" && kind != "reply_all" {
        // A forward starts its own conversation.
        return Ok(ReplyHeaders {
            thread_id: None,
            in_reply_to: None,
            references: Vec::new(),
        });
    }
    let from_headers: Vec<String> = headers_json
        .and_then(|j| serde_json::from_str::<Vec<[String; 2]>>(&j).ok())
        .map(|pairs| {
            pairs
                .iter()
                .filter(|[name, _]| name.eq_ignore_ascii_case("references"))
                .flat_map(|[_, value]| ids_in(value))
                .collect()
        })
        .unwrap_or_default();
    let mut references = if from_headers.is_empty() {
        let mut stmt = conn
            .prepare("SELECT ref_hdr FROM mail_message_ref WHERE message_id = ?1 ORDER BY rowid")
            .map_err(db)?;
        let rows = stmt
            .query_map([message_id], |row| row.get::<_, String>(0))
            .map_err(db)?;
        rows.collect::<rusqlite::Result<Vec<_>>>().map_err(db)?
    } else {
        from_headers
    };
    if let Some(hdr) = hdr.as_deref().filter(|h| !h.is_empty()) {
        if references.last().map(String::as_str) != Some(hdr) {
            references.push(hdr.to_string());
        }
    }
    Ok(ReplyHeaders {
        thread_id: Some(thread_id),
        in_reply_to: hdr.filter(|h| !h.is_empty()),
        references: mail_build::cap_references(&references),
    })
}

pub(crate) fn outbox_save(store: &SessionStore, draft: OutboxDraft) -> Result<i64, String> {
    if !KINDS.contains(&draft.kind.as_str()) {
        return Err(format!("'{}' is not a kind of message.", draft.kind));
    }
    let conn = store.lock_conn()?;
    let mailbox = mail_store::mailbox_by_address(&conn, &draft.from_addr)
        .map_err(db)?
        .ok_or_else(|| {
            format!(
                "{} is not one of your mailboxes, so nothing can be sent from it.",
                draft.from_addr.trim()
            )
        })?;
    let reply = match draft.reply_to_message_id {
        Some(id) => reply_headers(&conn, &draft.kind, id)?,
        None => ReplyHeaders {
            thread_id: None,
            in_reply_to: None,
            references: Vec::new(),
        },
    };
    let trimmed = |list: &[Recipient]| -> Vec<Recipient> {
        list.iter()
            .map(|r| Recipient {
                name: r.name.clone(),
                addr: r.addr.trim().to_string(),
            })
            .collect()
    };
    let write = OutboxWrite {
        account_id: mailbox.account_id,
        mailbox_id: Some(mailbox.id),
        thread_id: reply.thread_id,
        reply_to_message_id: draft.reply_to_message_id,
        kind: draft.kind.clone(),
        from_addr: mailbox.address.clone(),
        to_json: json(&trimmed(&draft.to)),
        cc_json: json(&trimmed(&draft.cc)),
        bcc_json: json(&trimmed(&draft.bcc)),
        subject: draft.subject.clone(),
        body_text: draft.body_text.clone(),
        body_html: draft.body_html.clone(),
        body_json: draft.body_json.as_ref().map(|v| v.to_string()),
        quoted_html: draft.quoted_html.clone(),
        attachments_json: json(&draft.attachments),
        in_reply_to: reply.in_reply_to,
        references_json: json(&reply.references),
    };
    match draft.id {
        None => mail_store::insert_outbox(&conn, &write).map_err(db),
        Some(id) => {
            if mail_store::get_outbox(&conn, id).map_err(db)?.is_none() {
                return Err("That draft no longer exists.".into());
            }
            if mail_store::update_outbox(&conn, id, &write).map_err(db)? {
                Ok(id)
            } else {
                Err(
                    "This message was already sent or is being sent, so it cannot be changed."
                        .into(),
                )
            }
        }
    }
}

/// A list of plain structs always serialises.
fn json<T: Serialize>(value: &T) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "[]".into())
}

pub(crate) fn outbox_list(store: &SessionStore) -> Result<Vec<OutboxRow>, String> {
    let conn = store.lock_conn()?;
    mail_store::list_outbox(&conn).map_err(db)
}

pub(crate) fn outbox_delete(store: &SessionStore, id: i64) -> Result<(), String> {
    let conn = store.lock_conn()?;
    if mail_store::delete_outbox(&conn, id).map_err(db)? {
        return Ok(());
    }
    if mail_store::get_outbox(&conn, id).map_err(db)?.is_some() {
        return Err("This message is being sent and cannot be deleted yet.".into());
    }
    Ok(())
}

/// The deliberate step out of `unconfirmed`. A save is not enough, because a
/// composer saves on a timer.
pub(crate) fn outbox_rearm(store: &SessionStore, id: i64) -> Result<(), String> {
    let conn = store.lock_conn()?;
    if mail_store::rearm_outbox(&conn, id).map_err(db)? {
        Ok(())
    } else {
        Err("This message is not waiting to be confirmed, so there is nothing to confirm.".into())
    }
}

// ── the From picker ────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SendableMailbox {
    #[serde(flatten)]
    pub mailbox: MailMailbox,
    pub account_label: String,
    pub ses_region: String,
    /// This source's configured default From address.
    pub is_default: bool,
}

pub(crate) fn sendable_mailboxes(store: &SessionStore) -> Result<Vec<SendableMailbox>, String> {
    let conn = store.lock_conn()?;
    let mut out = Vec::new();
    for account in mail_store::list_accounts(&conn).map_err(db)? {
        if !account.enabled {
            continue;
        }
        for mailbox in mail_store::list_user_mailboxes(&conn, Some(account.id)).map_err(db)? {
            if !mailbox.is_sendable {
                continue;
            }
            out.push(SendableMailbox {
                is_default: account
                    .default_from
                    .as_deref()
                    .is_some_and(|d| d.eq_ignore_ascii_case(&mailbox.address)),
                mailbox,
                account_label: account.label.clone(),
                ses_region: account.ses_region.clone(),
            });
        }
    }
    Ok(out)
}

// ── checks before sending ──────────────────────────────────────────

/// Something that would make SES refuse this message, found before asking it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SendProblem {
    /// `identityNotVerified | identityPending | sendingDisabled | sendingPaused |
    /// sandboxRecipients`
    pub kind: &'static str,
    pub message: String,
    /// The recipients a `sandboxRecipients` problem is about.
    pub addresses: Vec<String>,
}

fn problem(kind: &'static str, message: String) -> SendProblem {
    SendProblem {
        kind,
        message,
        addresses: Vec::new(),
    }
}

/// Why this From address cannot send, from the cached identities.
fn identity_problem(
    from: &str,
    mailbox: &MailMailbox,
    identities: &[mail_store::MailIdentity],
    region: &str,
) -> SendProblem {
    let covering = identities
        .iter()
        .find(|i| i.identity == mailbox.address || i.identity == mailbox.domain);
    match covering.map(|i| (i.identity.as_str(), i.status.as_str())) {
        // Verified by AWS, switched off for the account. Not a DNS problem and
        // nothing to re-verify; the copy has to say so.
        Some((identity, "sending_disabled")) => problem(
            "sendingDisabled",
            format!(
                "AWS has verified {identity} in {region} but turned sending off for it. \
                 That is a setting in the SES console, not a DNS problem: turn sending back on \
                 there to send from {from}."
            ),
        ),
        Some((identity, "pending")) => problem(
            "identityPending",
            format!(
                "{identity} is still waiting for verification in {region}. \
                 Finish its DNS records in the SES console, then try again."
            ),
        ),
        _ => problem(
            "identityNotVerified",
            format!(
                "{from} is not a verified SES identity in {region}. \
                 Verify {} (or the address) in the SES console, then try again.",
                mailbox.domain
            ),
        ),
    }
}

/// Addresses no verified identity covers, an address or any parent domain.
fn uncovered(addresses: &[String], identities: &[mail_store::MailIdentity]) -> Vec<String> {
    let verified: HashSet<&str> = identities
        .iter()
        .filter(|i| i.status == "verified")
        .map(|i| i.identity.as_str())
        .collect();
    addresses
        .iter()
        .filter(|addr| {
            let lower = addr.to_ascii_lowercase();
            let Some((_, domain)) = lower.rsplit_once('@') else {
                return true;
            };
            let mut candidate = domain;
            loop {
                if verified.contains(candidate) {
                    return false;
                }
                match candidate.split_once('.') {
                    Some((_, parent)) if parent.contains('.') => candidate = parent,
                    _ => break,
                }
            }
            !verified.contains(lower.as_str())
        })
        .cloned()
        .collect()
}

fn all_recipients(row: &OutboxRow) -> Vec<String> {
    let dest = mail_build::destination(&row.to, &row.cc, &row.bcc);
    dest.to.into_iter().chain(dest.cc).chain(dest.bcc).collect()
}

struct Loaded {
    row: OutboxRow,
    account: MailAccount,
    mailbox: MailMailbox,
}

fn load(store: &SessionStore, id: i64) -> Result<Loaded, String> {
    let conn = store.lock_conn()?;
    let row = mail_store::get_outbox(&conn, id)
        .map_err(db)?
        .ok_or_else(|| "That message no longer exists.".to_string())?;
    let mailbox = mail_store::mailbox_by_address(&conn, &row.from_addr)
        .map_err(db)?
        .ok_or_else(|| {
            format!(
                "{} is no longer one of your mailboxes, so nothing can be sent from it.",
                row.from_addr
            )
        })?;
    // The row's own account is what a retry must reuse (spec 8.2); a mailbox
    // that now belongs to another source is a different sender.
    if mailbox.account_id != row.account_id {
        return Err(format!(
            "{} now belongs to another source than this message was written for. \
             Open it and pick the From address again.",
            row.from_addr
        ));
    }
    let account = mail_store::get_account(&conn, row.account_id)
        .map_err(db)?
        .ok_or_else(|| "That source no longer exists.".to_string())?;
    Ok(Loaded {
        row,
        account,
        mailbox,
    })
}

fn find_problems(
    store: &SessionStore,
    clients: &dyn AwsClients,
    loaded: &Loaded,
) -> Result<Vec<SendProblem>, String> {
    let Loaded {
        row,
        account,
        mailbox,
    } = loaded;
    let cached = |store: &SessionStore| -> Result<Vec<mail_store::MailIdentity>, String> {
        let conn = store.lock_conn()?;
        mail_store::list_identities(&conn, account.id).map_err(db)
    };
    if !mailbox.is_sendable {
        return Ok(vec![identity_problem(
            &row.from_addr,
            mailbox,
            &cached(store)?,
            &account.ses_region,
        )]);
    }
    // Unreadable account state is not a reason to block: SES will say.
    let Ok(ses_account) = clients.ses(account)?.get_account() else {
        return Ok(Vec::new());
    };
    if !ses_account.sending_enabled {
        return Ok(vec![problem(
            "sendingPaused",
            format!(
                "Sending is paused for the SES account in {}. Resume it in the SES console.",
                account.ses_region
            ),
        )]);
    }
    if !ses_account.sandbox {
        return Ok(Vec::new());
    }
    let recipients = all_recipients(row);
    let mut missing = uncovered(&recipients, &cached(store)?);
    if !missing.is_empty() {
        // The cache is up to a day old and the person may have just verified
        // an address; ask SES before refusing.
        if refresh_identities(store, clients, account).is_ok() {
            missing = uncovered(&recipients, &cached(store)?);
        }
    }
    if missing.is_empty() {
        return Ok(Vec::new());
    }
    Ok(vec![SendProblem {
        kind: "sandboxRecipients",
        message: format!(
            "The SES account in {} is in sandbox mode, which only delivers to verified addresses. \
             SES would refuse this message because of: {}. Verify them, or ask AWS for production access.",
            account.ses_region,
            missing.join(", ")
        ),
        addresses: missing,
    }])
}

/// For the compose sheet, so it can warn before the person presses Send.
pub(crate) fn outbox_check(
    store: &SessionStore,
    clients: &dyn AwsClients,
    id: i64,
) -> Result<Vec<SendProblem>, String> {
    let loaded = load(store, id)?;
    find_problems(store, clients, &loaded)
}

// ── building ───────────────────────────────────────────────────────

fn too_big() -> String {
    format!(
        "This message is over the {} MB SES accepts once its attachments are encoded. \
         Remove or shrink an attachment.",
        SES_RAW_LIMIT / 1_000_000
    )
}

fn load_attachments(list: &[OutboxAttachment]) -> Result<Vec<LoadedAttachment>, String> {
    // Sizes first: a 2 GB file must be refused before it is read into memory.
    let mut total: u64 = 0;
    for a in list {
        let len = std::fs::metadata(&a.path)
            .map_err(|_| format!("Could not read the attachment '{}'.", a.filename))?
            .len();
        total = total.saturating_add(len);
    }
    if total.saturating_mul(4) / 3 > SES_RAW_LIMIT as u64 {
        return Err(too_big());
    }
    list.iter()
        .map(|a| {
            Ok(LoadedAttachment {
                filename: a.filename.clone(),
                mime: a.mime.clone(),
                cid: a.cid.clone().filter(|c| !c.is_empty()),
                bytes: std::fs::read(&a.path)
                    .map_err(|_| format!("Could not read the attachment '{}'.", a.filename))?,
            })
        })
        .collect()
}

/// `cid:` targets in the body the person wrote. The quoted original is cut out
/// first: it may carry `cid:` references to the original's own images, which
/// were never ours to attach.
fn body_cids(html: &str, quoted: Option<&str>) -> Vec<String> {
    let own = match quoted.filter(|q| !q.is_empty()) {
        Some(q) => html.replacen(q, "", 1),
        None => html.to_string(),
    };
    let lower = own.to_ascii_lowercase();
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(at) = lower[from..].find("cid:") {
        let start = from + at + 4;
        let end = lower[start..]
            .find(|c: char| c == '"' || c == '\'' || c == ')' || c == '>' || c.is_whitespace())
            .map_or(lower.len(), |n| start + n);
        out.push(
            lower[start..end]
                .trim_matches(|c| c == '<' || c == '>')
                .to_string(),
        );
        from = end.max(start);
    }
    out
}

struct Built {
    raw: Vec<u8>,
    key: String,
}

/// Everything that can fail before the send, with no side effect.
fn build_message(loaded: &Loaded) -> Result<Built, String> {
    let row = &loaded.row;
    if row.state == "sent" {
        return Err("This message was already sent.".into());
    }
    if row.state == "sending" {
        return Err("This message is already being sent.".into());
    }
    if row.state == "unconfirmed" {
        return Err(
            "SES may already have delivered this message. Check the recipient, then confirm \
             in the outbox that you want to send it again."
                .into(),
        );
    }
    if !mail_build::valid_address(&row.from_addr) {
        return Err(format!("'{}' is not a valid From address.", row.from_addr));
    }
    if row.to.is_empty() && row.cc.is_empty() && row.bcc.is_empty() {
        return Err("Add at least one recipient.".into());
    }
    if row.to.is_empty() {
        // A message with only Cc or Bcc recipients has no To header to show.
        return Err("Add at least one recipient in To.".into());
    }
    for r in row.to.iter().chain(&row.cc).chain(&row.bcc) {
        if !mail_build::valid_address(r.addr.trim()) {
            return Err(format!("'{}' is not a valid email address.", r.addr));
        }
    }
    let (Some(html), Some(text)) = (row.body_html.as_deref(), row.body_text.as_deref()) else {
        return Err(
            "This message has not been rendered yet. Open it in the composer and send again."
                .into(),
        );
    };
    let attachments = load_attachments(&row.attachments)?;
    let attached: HashSet<String> = attachments
        .iter()
        .filter_map(|a| a.cid.as_ref())
        .map(|c| c.to_ascii_lowercase())
        .collect();
    // The draft id salts our cids so they cannot equal one inside the quoted
    // original. That is a convention of the composer, so check the result: a
    // shared cid makes the recipient's client show the quoted image in place
    // of ours, and nothing else would fail.
    if let Some(quoted) = row.quoted_html.as_deref() {
        let theirs: HashSet<String> = body_cids(quoted, None).into_iter().collect();
        if let Some(clash) = attached.iter().find(|c| theirs.contains(*c)) {
            return Err(format!(
                "An image in this message has the same internal id ({clash}) as one in the \
                 message being quoted, so the recipient would see the wrong picture. \
                 Remove the image and add it again."
            ));
        }
    }
    if let Some(missing) = body_cids(html, row.quoted_html.as_deref())
        .into_iter()
        .find(|c| !attached.contains(c))
    {
        return Err(format!(
            "An image in this message has no file attached to it ({missing}). \
             Remove the image and add it again."
        ));
    }
    let domain = row
        .from_addr
        .rsplit_once('@')
        .map_or("localhost", |(_, d)| d)
        .to_ascii_lowercase();
    let local = uuid::Uuid::new_v4().to_string();
    let message_id = format!("{local}@{domain}");
    let raw = mail_build::build(&Outgoing {
        from: &row.from_addr,
        to: &row.to,
        cc: &row.cc,
        subject: &row.subject,
        text,
        html,
        message_id: &message_id,
        in_reply_to: row.in_reply_to.as_deref(),
        references: &row.references,
        date: now_millis() / 1000,
        attachments: &attachments,
    })?;
    if raw.len() > SES_RAW_LIMIT {
        return Err(too_big());
    }
    Ok(Built {
        raw,
        key: format!("{SENT_PREFIX}{local}.eml"),
    })
}

// ── sending ────────────────────────────────────────────────────────

/// Why a send did not complete. `retry_safe` is the part that matters: after
/// a connection that dropped with the request out, SES may have delivered, so
/// a Retry button there sends the email twice.
///
/// `retry_safe` describes THIS call only, not the row. Refusing a row that is
/// already `unconfirmed` is `kind: "form", retry_safe: true`, because that
/// call sent nothing; a UI written as `if retry_safe { show_retry() }` would
/// offer Retry on the most dangerous row in the outbox. Decide what to offer
/// from the row's `state` (`unconfirmed` means "check the recipient first",
/// then an explicit `mail_outbox_rearm`), and use this only for the message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SendError {
    /// `form` for anything caught before SES was called, else the SES error
    /// kind (`rejected`, `unconfirmed`, `network`, ...); `other` when unknown.
    pub kind: &'static str,
    pub message: String,
    /// False when the message may have been delivered. The row is then
    /// `unconfirmed`, not `failed`, and is not sent again without an edit.
    pub retry_safe: bool,
}

impl std::fmt::Display for SendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

/// Deliberately no `From<String>`: a bare `?` on a string error would pick a
/// retry-safety for the caller. Before the SES call an error is `form`; after
/// it nothing may fail at all (see the module comment).
impl SendError {
    pub(crate) fn form(message: String) -> Self {
        Self {
            kind: "form",
            message,
            retry_safe: true,
        }
    }

    /// The outcome is not known (a panicked or cancelled task, for one), so
    /// the message may have been delivered. Fails closed.
    pub(crate) fn unknown(message: String) -> Self {
        Self {
            kind: "other",
            message,
            retry_safe: false,
        }
    }
}

/// Only these mean SES certainly did not take the message. `Other` includes
/// 5xx answers and anything unreadable, which prove nothing.
fn nothing_was_sent(kind: SesErrorKind) -> bool {
    matches!(
        kind,
        SesErrorKind::Rejected
            | SesErrorKind::AccessDenied
            | SesErrorKind::InvalidCredentials
            | SesErrorKind::ClockSkew
            | SesErrorKind::Network
    )
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ArchiveResult {
    /// The raw message is in the source's `sent/` prefix.
    pub stored: bool,
    /// Why it is not, when it is not. The send itself succeeded either way.
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SendReport {
    pub outbox_id: i64,
    pub ses_message_id: String,
    /// The `mail_message` row, once the sent mail is in its thread.
    pub message_id: Option<i64>,
    pub thread_id: Option<i64>,
    pub archive: ArchiveResult,
    /// Set when the message was sent but recording it locally failed. Not a
    /// failed send: the person must not be offered a retry.
    pub bookkeeping_error: Option<String>,
}

fn archive_failure(account: &MailAccount, kind: S3ErrorKind, message: &str) -> String {
    match kind {
        S3ErrorKind::AccessDenied => format!(
            "The key pair for {} cannot write to {}/{SENT_PREFIX} (it needs s3:PutObject), \
             so this copy is kept on this computer only.",
            account.label, account.bucket
        ),
        _ => format!(
            "Could not save the copy to {}/{SENT_PREFIX}: {message} It is kept on this computer only.",
            account.bucket
        ),
    }
}

struct Prepared {
    loaded: Loaded,
    built: Built,
    ses: Box<dyn crate::aws_ses::SesApi + Send>,
    s3: Result<Box<dyn crate::aws_s3::S3Api + Send>, String>,
}

/// Everything before the SES call. Every failure here is a form error that
/// changes nothing; once the row is claimed this function is done.
fn prepare(store: &SessionStore, clients: &dyn AwsClients, id: i64) -> Result<Prepared, String> {
    let loaded = load(store, id)?;
    if let Some(first) = find_problems(store, clients, &loaded)?.into_iter().next() {
        return Err(first.message);
    }
    let built = build_message(&loaded)?;
    // Clients before the claim: a missing key is a form error that must leave
    // the draft a draft.
    let ses = clients.ses(&loaded.account)?;
    let s3 = clients.s3(&loaded.account);
    {
        let conn = store.lock_conn()?;
        if !mail_store::claim_outbox_for_send(&conn, id).map_err(db)? {
            return Err("This message was already sent or is being sent.".into());
        }
    }
    Ok(Prepared {
        loaded,
        built,
        ses,
        s3,
    })
}

pub(crate) fn outbox_send(
    store: &SessionStore,
    clients: &dyn AwsClients,
    id: i64,
) -> Result<SendReport, SendError> {
    let Prepared {
        loaded,
        built,
        ses,
        s3,
    } = prepare(store, clients, id).map_err(SendError::form)?;

    let row = &loaded.row;
    let destination = mail_build::destination(&row.to, &row.cc, &row.bcc);
    let ses_message_id = match ses.send_raw(&row.from_addr, &destination, &built.raw) {
        Ok(ses_id) => ses_id,
        Err(error) => {
            let retry_safe = nothing_was_sent(error.kind);
            if let Ok(conn) = store.lock_conn() {
                let state = if retry_safe { "failed" } else { "unconfirmed" };
                let _ = mail_store::mark_outbox_failed(&conn, id, &error.message, state);
            }
            return Err(SendError {
                kind: crate::mail::ses_kind(error.kind),
                message: error.message,
                retry_safe,
            });
        }
    };

    // SES has the message. Nothing below may turn this into an error.
    let mut report = SendReport {
        outbox_id: id,
        ses_message_id: ses_message_id.clone(),
        message_id: None,
        thread_id: None,
        archive: ArchiveResult {
            stored: false,
            reason: None,
        },
        bookkeeping_error: None,
    };

    // Its own transaction, first, so that whatever fails after it the row
    // already says sent.
    let marked = store
        .lock_conn()
        .and_then(|conn| mail_store::mark_outbox_sent(&conn, id, &ses_message_id).map_err(db));
    let not_recorded = match marked {
        Ok(true) => None,
        Ok(false) => Some("the row was no longer being sent".to_string()),
        Err(error) => Some(error),
    };
    if let Some(error) = not_recorded {
        report.bookkeeping_error = Some(format!(
            "The message was sent, but Monoptah could not record it: {error}"
        ));
    }

    match s3 {
        Ok(s3) => match s3.put_object(&built.key, &built.raw, "message/rfc822") {
            Ok(()) => report.archive.stored = true,
            Err(error) => {
                report.archive.reason =
                    Some(archive_failure(&loaded.account, error.kind, &error.message))
            }
        },
        Err(error) => report.archive.reason = Some(error),
    }

    match record_local(store, &loaded, &built, report.archive.stored) {
        Ok((message_id, thread_id)) => {
            report.message_id = Some(message_id);
            report.thread_id = Some(thread_id);
        }
        Err(error) => {
            let note =
                format!("The message was sent, but it could not be added to its thread: {error}");
            report.bookkeeping_error.get_or_insert(note);
        }
    }
    Ok(report)
}

/// Threads the sent message into the conversation it answers, by the same
/// entry point inbound mail uses, so its References-first matching and guards
/// apply. One transaction: the message, its S3 object, its folders.
fn record_local(
    store: &SessionStore,
    loaded: &Loaded,
    built: &Built,
    archived: bool,
) -> Result<(i64, i64), String> {
    let row = &loaded.row;
    // Our own bytes through the inbound parser: the stored row is exactly what
    // a second machine would read back from `sent/`.
    let mut mail = parse_mail(&built.raw)?;
    // The MIME has no Bcc by design; the local copy keeps who was blind-copied.
    mail.bcc = row
        .bcc
        .iter()
        .map(|r| MailAddress {
            name: r.name.clone(),
            address: r.addr.clone(),
        })
        .collect();
    let now_secs = now_millis() / 1000;

    let conn = store.lock_conn()?;
    let tx = conn.unchecked_transaction().map_err(db)?;
    let target = mail_store::filing_mailbox(
        &tx,
        loaded.account.id,
        Some(loaded.mailbox.id),
        &mail.referenced_ids,
    )
    .map_err(db)?
    .ok_or_else(|| "The mailbox it was sent from no longer exists.".to_string())?;
    let object_id = if archived {
        let sent_box = mail_store::ensure_sent_mailbox(&tx, loaded.account.id).map_err(db)?;
        Some(link_sent_object(&tx, sent_box, built, now_secs).map_err(db)?)
    } else {
        None
    };
    let ingested = mail_thread::ingest_message(&tx, target, "outbound", object_id, &mail, now_secs)
        .map_err(db)?;
    if ingested.duplicate {
        // The sync read the `sent/` copy between the upload and now. It could
        // not know who was blind-copied.
        tx.execute(
            "UPDATE mail_message SET bcc_json = ?2 WHERE id = ?1 AND bcc_json = '[]'",
            rusqlite::params![
                ingested.message_id,
                serde_json::to_string(&row.bcc).unwrap_or_else(|_| "[]".into())
            ],
        )
        .map_err(db)?;
    } else {
        mark_outbound_read(&tx, ingested.message_id, ingested.thread_id).map_err(db)?;
        mail_store::reactivate_thread(&tx, ingested.thread_id).map_err(db)?;
        mail_store::reevaluate_thread(&tx, ingested.thread_id).map_err(db)?;
    }
    mail_store::set_outbox_thread(&tx, row.id, ingested.thread_id).map_err(db)?;
    tx.commit().map_err(db)?;
    Ok((ingested.message_id, ingested.thread_id))
}

/// Your own message is not unread mail.
pub(crate) fn mark_outbound_read(
    conn: &rusqlite::Connection,
    message_id: i64,
    thread_id: i64,
) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE mail_message SET is_read = 1 WHERE id = ?1",
        [message_id],
    )?;
    mail_thread::refresh_thread(conn, thread_id)
}

/// The ledger row for the object just uploaded, so a sync that lists it finds
/// a known object (and a hard delete knows what to remove) instead of ingesting
/// a second copy. Reuses a row the sync may already have made.
fn link_sent_object(
    conn: &rusqlite::Connection,
    sent_box: i64,
    built: &Built,
    now_secs: i64,
) -> rusqlite::Result<i64> {
    if let Some(existing) = mail_store::object_by_key(conn, sent_box, &built.key)? {
        return Ok(existing);
    }
    // No etag: S3's reply to a PutObject is not read here, and the empty one
    // differs from any real one, so the sync records the real one on its next
    // listing. It does not re-parse an outbound message (see `ingest_one`).
    let id = mail_store::insert_object(
        conn,
        sent_box,
        &mail_store::ListedObject {
            s3_key: &built.key,
            etag: "",
            size_bytes: built.raw.len() as i64,
            s3_modified: Some(now_secs),
        },
        now_millis(),
    )?;
    mail_store::set_object_outcome(conn, id, "parsed", None, 0)?;
    Ok(id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::aws_s3::{DeleteOutcome, ObjectPage, S3Api, S3Error};
    use crate::aws_ses::{Destination, SesAccount, SesApi, SesError, SesErrorKind};
    use crate::mail_store::{
        upsert_account, upsert_credential, upsert_mailboxes, AccountInput, CredentialInput,
        MailIdentity, MailboxInput,
    };
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};

    struct Sent {
        bucket: String,
        from: String,
        dest: Destination,
        raw: Vec<u8>,
    }

    #[derive(Default)]
    struct Log {
        /// `(bucket, key)` of every PutObject that was accepted.
        puts: Vec<(String, String)>,
        /// What a later listing of the bucket shows.
        objects: std::collections::BTreeMap<(String, String), Vec<u8>>,
        sends: Vec<Sent>,
        refreshes: usize,
    }

    #[derive(Default)]
    struct Fake {
        log: Mutex<Log>,
        deny_put: AtomicBool,
        sandbox: AtomicBool,
        paused: AtomicBool,
        ses_error: Mutex<Option<SesError>>,
        on_send: Mutex<Option<Box<dyn Fn() + Send>>>,
        identities: Mutex<HashMap<i64, Vec<MailIdentity>>>,
    }

    struct FakeS3 {
        fake: Arc<Fake>,
        bucket: String,
    }

    impl S3Api for FakeS3 {
        // Only the sync reads: sending itself never lists or fetches.
        fn list_objects_v2(
            &self,
            prefix: &str,
            _: Option<&str>,
            _: Option<&str>,
        ) -> Result<ObjectPage, S3Error> {
            let log = self.fake.log.lock().unwrap();
            let objects = log
                .objects
                .iter()
                .filter(|((bucket, key), _)| *bucket == self.bucket && key.starts_with(prefix))
                .map(|((_, key), bytes)| crate::aws_s3::ObjectInfo {
                    key: key.clone(),
                    size: bytes.len() as u64,
                    etag: format!("etag-{}", bytes.len()),
                    last_modified: "2024-10-01T10:00:00.000Z".into(),
                })
                .collect();
            Ok(ObjectPage {
                objects,
                common_prefixes: vec![],
                next_token: None,
            })
        }
        fn get_object(&self, key: &str) -> Result<Vec<u8>, S3Error> {
            self.fake
                .log
                .lock()
                .unwrap()
                .objects
                .get(&(self.bucket.clone(), key.to_string()))
                .cloned()
                .ok_or(S3Error {
                    kind: S3ErrorKind::NoSuchKey,
                    code: None,
                    message: "gone".into(),
                })
        }
        fn put_object(&self, key: &str, body: &[u8], content_type: &str) -> Result<(), S3Error> {
            assert_eq!(content_type, "message/rfc822");
            if self.fake.deny_put.load(Ordering::SeqCst) {
                return Err(S3Error {
                    kind: S3ErrorKind::AccessDenied,
                    code: Some("AccessDenied".into()),
                    message: "Access denied.".into(),
                });
            }
            let mut log = self.fake.log.lock().unwrap();
            log.puts.push((self.bucket.clone(), key.to_string()));
            log.objects
                .insert((self.bucket.clone(), key.to_string()), body.to_vec());
            Ok(())
        }
        fn delete_objects(&self, _: &[String]) -> Vec<DeleteOutcome> {
            unreachable!("sending never deletes")
        }
    }

    struct FakeSes {
        fake: Arc<Fake>,
        account_id: i64,
        bucket: String,
    }

    impl SesApi for FakeSes {
        fn list_identities(&self) -> Result<Vec<MailIdentity>, SesError> {
            self.fake.log.lock().unwrap().refreshes += 1;
            Ok(self
                .fake
                .identities
                .lock()
                .unwrap()
                .get(&self.account_id)
                .cloned()
                .unwrap_or_default())
        }
        fn get_account(&self) -> Result<SesAccount, SesError> {
            Ok(SesAccount {
                sandbox: self.fake.sandbox.load(Ordering::SeqCst),
                sending_enabled: !self.fake.paused.load(Ordering::SeqCst),
            })
        }
        fn send_raw(
            &self,
            from: &str,
            destination: &Destination,
            raw: &[u8],
        ) -> Result<String, SesError> {
            self.fake.log.lock().unwrap().sends.push(Sent {
                bucket: self.bucket.clone(),
                from: from.to_string(),
                dest: destination.clone(),
                raw: raw.to_vec(),
            });
            if let Some(hook) = self.fake.on_send.lock().unwrap().as_ref() {
                hook();
            }
            match self.fake.ses_error.lock().unwrap().clone() {
                Some(error) => Err(error),
                None => Ok("ses-message-1".into()),
            }
        }
    }

    impl AwsClients for Arc<Fake> {
        fn s3(&self, account: &MailAccount) -> Result<Box<dyn S3Api + Send>, String> {
            Ok(Box::new(FakeS3 {
                fake: self.clone(),
                bucket: account.bucket.clone(),
            }))
        }
        fn ses(&self, account: &MailAccount) -> Result<Box<dyn SesApi + Send>, String> {
            Ok(Box::new(FakeSes {
                fake: self.clone(),
                account_id: account.id,
                bucket: account.bucket.clone(),
            }))
        }
    }

    impl crate::mail_sync::ClientFactory for Arc<Fake> {
        fn client(&self, account: &MailAccount) -> Result<Box<dyn S3Api + Send>, String> {
            self.s3(account)
        }
    }

    struct Quiet;
    impl crate::mail_sync::SyncEvents for Quiet {
        fn sync_state(&self, _: &crate::mail_sync::SyncStateEvent) {}
        fn messages_changed(&self, _: i64) {}
    }

    fn sync(w: &World) -> Vec<crate::mail_sync::SyncReport> {
        crate::mail_sync::SyncService::default().sync_now(&w.store, &w.fake, &Quiet, None)
    }

    fn verified(identity: &str) -> MailIdentity {
        MailIdentity {
            identity: identity.into(),
            kind: "domain".into(),
            status: "verified".into(),
        }
    }

    struct World {
        store: Arc<SessionStore>,
        fake: Arc<Fake>,
        /// Source A: byptah.com, two mailboxes. Source B: acme.com.
        a: i64,
        b: i64,
        owner: i64,
    }

    fn world() -> World {
        let store = Arc::new(SessionStore::open_in_memory().unwrap());
        let fake = Arc::new(Fake::default());
        let (a, b, owner) = {
            let conn = store.lock_conn().unwrap();
            let cred = upsert_credential(
                &conn,
                &CredentialInput {
                    provider: "aws".into(),
                    label: "personal".into(),
                    access_key_masked: "AKIA\u{2022}\u{2022}\u{2022}\u{2022}MPLE".into(),
                    default_region: Some("us-east-1".into()),
                },
            )
            .unwrap();
            let account = |label: &str, bucket: &str| AccountInput {
                id: None,
                provider: "aws".into(),
                credential_id: cred,
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
            };
            let mailbox = |address: &str, prefix: &str| MailboxInput {
                address: address.into(),
                s3_prefix: prefix.into(),
                display_name: "Box".into(),
                color: None,
                sort_order: 0,
            };
            let a = upsert_account(&conn, &account("Byptah", "bucket-a")).unwrap();
            let b = upsert_account(&conn, &account("Acme", "bucket-b")).unwrap();
            upsert_mailboxes(
                &conn,
                a,
                &[
                    mailbox("owner@byptah.com", "owner/"),
                    mailbox("support@byptah.com", "support/"),
                ],
            )
            .unwrap();
            upsert_mailboxes(&conn, b, &[mailbox("sales@acme.com", "sales/")]).unwrap();
            mail_store::replace_identities(&conn, a, &[verified("byptah.com")]).unwrap();
            mail_store::replace_identities(&conn, b, &[verified("acme.com")]).unwrap();
            let id = |address: &str| {
                mail_store::mailbox_by_address(&conn, address)
                    .unwrap()
                    .unwrap()
                    .id
            };
            (a, b, id("owner@byptah.com"))
        };
        fake.identities
            .lock()
            .unwrap()
            .insert(a, vec![verified("byptah.com")]);
        fake.identities
            .lock()
            .unwrap()
            .insert(b, vec![verified("acme.com")]);
        World {
            store,
            fake,
            a,
            b,
            owner,
        }
    }

    fn who(addr: &str) -> Recipient {
        Recipient {
            name: None,
            addr: addr.into(),
        }
    }

    fn draft(from: &str, to: &str) -> OutboxDraft {
        OutboxDraft {
            id: None,
            kind: "new".into(),
            from_addr: from.into(),
            reply_to_message_id: None,
            to: vec![who(to)],
            cc: vec![],
            bcc: vec![],
            subject: "Hello there".into(),
            body_json: Some(serde_json::json!({"type": "doc", "content": []})),
            body_text: Some("hi text".into()),
            body_html: Some("<p>hi html</p>".into()),
            quoted_html: None,
            attachments: vec![],
        }
    }

    fn send(w: &World, d: OutboxDraft) -> Result<SendReport, SendError> {
        let id = outbox_save(&w.store, d).map_err(SendError::form)?;
        outbox_send(&w.store, &w.fake, id)
    }

    impl SendError {
        fn contains(&self, needle: &str) -> bool {
            self.message.contains(needle)
        }
    }

    fn count(w: &World, sql: &str) -> i64 {
        w.store
            .lock_conn()
            .unwrap()
            .query_row(sql, [], |row| row.get(0))
            .unwrap()
    }

    fn text(w: &World, sql: &str) -> String {
        w.store
            .lock_conn()
            .unwrap()
            .query_row(sql, [], |row| row.get(0))
            .unwrap()
    }

    /// An inbound message, ingested the way the sync does.
    fn arrive(w: &World, mailbox: i64, raw: &str) -> (i64, i64) {
        let mail = parse_mail(raw.as_bytes()).unwrap();
        let conn = w.store.lock_conn().unwrap();
        let done =
            mail_thread::ingest_message(&conn, mailbox, "inbound", None, &mail, 1_700_000_000)
                .unwrap();
        (done.message_id, done.thread_id)
    }

    fn inbound(id: &str, subject: &str, extra: &str) -> String {
        format!(
            "From: Ann <ann@example.org>\r\nTo: owner@byptah.com\r\nSubject: {subject}\r\n\
             Message-ID: <{id}@example.org>\r\nDate: Tue, 01 Oct 2024 10:00:00 +0000\r\n{extra}\
             Content-Type: text/plain; charset=utf-8\r\n\r\nbody of {id}\r\n"
        )
    }

    #[test]
    fn a_sent_copy_lands_in_the_sending_sources_prefix_and_not_another() {
        let w = world();
        let report = send(&w, draft("owner@byptah.com", "ann@example.org")).unwrap();
        assert!(report.archive.stored, "{:?}", report.archive);
        let log = w.fake.log.lock().unwrap();
        assert_eq!(log.puts.len(), 1);
        assert_eq!(log.puts[0].0, "bucket-a");
        assert!(log.puts[0].1.starts_with("sent/") && log.puts[0].1.ends_with(".eml"));
        assert!(log.puts.iter().all(|(bucket, _)| bucket != "bucket-b"));
        assert_eq!(log.sends[0].bucket, "bucket-a");
        drop(log);
        // Only the sending source has a sent archive, and it is not a From address.
        assert_eq!(
            count(
                &w,
                &format!(
                    "SELECT COUNT(*) FROM mail_mailbox WHERE s3_prefix = 'sent/' AND account_id = {}",
                    w.a
                )
            ),
            1
        );
        assert_eq!(
            count(
                &w,
                &format!(
                    "SELECT COUNT(*) FROM mail_mailbox WHERE s3_prefix = 'sent/' AND account_id = {}",
                    w.b
                )
            ),
            0
        );
        assert_eq!(
            count(
                &w,
                "SELECT is_sendable FROM mail_mailbox WHERE s3_prefix = 'sent/'"
            ),
            0
        );
        // The message is linked to its object, so a hard delete can find it.
        assert_eq!(
            count(
                &w,
                "SELECT COUNT(*) FROM mail_message m JOIN mail_object o ON o.id = m.object_id
                 WHERE o.s3_key LIKE 'sent/%.eml' AND m.direction = 'outbound'"
            ),
            1
        );
    }

    #[test]
    fn a_denied_put_object_still_reports_the_send_and_flags_the_copy_separately() {
        let w = world();
        w.fake.deny_put.store(true, Ordering::SeqCst);
        let report = send(&w, draft("owner@byptah.com", "ann@example.org")).unwrap();
        assert_eq!(report.ses_message_id, "ses-message-1");
        assert!(report.bookkeeping_error.is_none());
        assert!(!report.archive.stored);
        let reason = report.archive.reason.expect("a reason");
        assert!(reason.contains("s3:PutObject"), "{reason}");
        assert!(reason.contains("this computer only"), "{reason}");
        assert_eq!(w.fake.log.lock().unwrap().sends.len(), 1);
        assert_eq!(text(&w, "SELECT state FROM mail_outbox"), "sent");
        // Local-only: in the thread, backed by no object, no archive registered.
        assert_eq!(
            count(
                &w,
                "SELECT COUNT(*) FROM mail_message WHERE direction = 'outbound' AND object_id IS NULL"
            ),
            1
        );
        assert_eq!(
            count(
                &w,
                "SELECT COUNT(*) FROM mail_mailbox WHERE s3_prefix = 'sent/'"
            ),
            0
        );
    }

    #[test]
    fn a_reply_is_threaded_into_the_conversation_it_answers() {
        let w = world();
        let (parent, thread) = arrive(&w, w.owner, &inbound("p1", "Question", ""));
        let mut d = draft("owner@byptah.com", "ann@example.org");
        d.kind = "reply".into();
        d.subject = "Re: Question".into();
        d.reply_to_message_id = Some(parent);
        let report = send(&w, d).unwrap();
        assert_eq!(report.thread_id, Some(thread));
        assert_eq!(count(&w, "SELECT COUNT(*) FROM mail_thread"), 1);
        assert_eq!(
            count(
                &w,
                &format!("SELECT message_count FROM mail_thread WHERE id = {thread}")
            ),
            2
        );
        // Our own message does not count as unread; the parent still does.
        assert_eq!(
            count(
                &w,
                &format!("SELECT unread_count FROM mail_thread WHERE id = {thread}")
            ),
            1
        );
        let raw = &w.fake.log.lock().unwrap().sends[0].raw.clone();
        let sent = String::from_utf8_lossy(raw);
        assert!(sent.contains("In-Reply-To: <p1@example.org>"), "{sent}");
        assert!(sent.contains("References: <p1@example.org>"), "{sent}");
        assert_eq!(
            text(&w, "SELECT in_reply_to FROM mail_outbox"),
            "p1@example.org"
        );
    }

    #[test]
    fn a_reply_sent_from_another_mailbox_of_the_source_still_joins_the_thread() {
        let w = world();
        let (parent, thread) = arrive(&w, w.owner, &inbound("p1", "Question", ""));
        let mut d = draft("support@byptah.com", "ann@example.org");
        d.kind = "reply".into();
        d.reply_to_message_id = Some(parent);
        let report = send(&w, d).unwrap();
        assert_eq!(report.thread_id, Some(thread));
        assert_eq!(
            count(
                &w,
                &format!(
                    "SELECT COUNT(*) FROM mail_message WHERE thread_id = {thread} AND mailbox_id = {}",
                    w.owner
                )
            ),
            2
        );
    }

    #[test]
    fn our_message_id_is_stored_without_brackets_so_a_reply_to_it_links() {
        let w = world();
        let report = send(&w, draft("owner@byptah.com", "ann@example.org")).unwrap();
        let stored = text(
            &w,
            "SELECT message_id_hdr FROM mail_message WHERE direction = 'outbound'",
        );
        assert!(!stored.contains('<') && !stored.contains('>'), "{stored}");
        // The wire form has them.
        let sent = String::from_utf8_lossy(&w.fake.log.lock().unwrap().sends[0].raw).to_string();
        assert!(sent.contains(&format!("Message-ID: <{stored}>")), "{sent}");
        // Ann answers our message.
        let (_, reply_thread) = arrive(
            &w,
            w.owner,
            &inbound(
                "r1",
                "Re: Hello there",
                &format!("In-Reply-To: <{stored}>\r\nReferences: <{stored}>\r\n"),
            ),
        );
        assert_eq!(Some(reply_thread), report.thread_id);
        assert_eq!(count(&w, "SELECT COUNT(*) FROM mail_thread"), 1);
    }

    #[test]
    fn bcc_goes_to_ses_and_to_the_local_copy_but_never_into_the_message() {
        let w = world();
        let mut d = draft("owner@byptah.com", "ann@example.org");
        d.bcc = vec![who("secret@hidden.org")];
        send(&w, d).unwrap();
        let log = w.fake.log.lock().unwrap();
        assert_eq!(log.sends[0].dest.bcc, vec!["secret@hidden.org".to_string()]);
        assert_eq!(log.sends[0].dest.to, vec!["ann@example.org".to_string()]);
        let raw = String::from_utf8_lossy(&log.sends[0].raw).to_lowercase();
        assert!(!raw.contains("secret@hidden.org"), "{raw}");
        assert!(!raw.contains("bcc:"), "{raw}");
        drop(log);
        assert!(text(&w, "SELECT bcc_json FROM mail_message").contains("secret@hidden.org"));
    }

    #[test]
    fn a_rejected_send_is_recorded_once_and_not_retried_or_threaded() {
        let w = world();
        *w.fake.ses_error.lock().unwrap() = Some(SesError {
            kind: SesErrorKind::Rejected,
            message: "SES refused the message: Email address is not verified.".into(),
        });
        let id = outbox_save(&w.store, draft("owner@byptah.com", "ann@example.org")).unwrap();
        let error = outbox_send(&w.store, &w.fake, id).unwrap_err();
        assert!(error.contains("not verified"), "{error}");
        assert_eq!(w.fake.log.lock().unwrap().sends.len(), 1);
        assert_eq!(text(&w, "SELECT state FROM mail_outbox"), "failed");
        assert!(text(&w, "SELECT last_error FROM mail_outbox").contains("not verified"));
        assert_eq!(count(&w, "SELECT COUNT(*) FROM mail_message"), 0);
        assert!(w.fake.log.lock().unwrap().puts.is_empty());
        // An explicit retry is allowed and is a second, visible attempt.
        *w.fake.ses_error.lock().unwrap() = None;
        outbox_send(&w.store, &w.fake, id).unwrap();
        assert_eq!(text(&w, "SELECT state FROM mail_outbox"), "sent");
        assert_eq!(count(&w, "SELECT attempts FROM mail_outbox"), 2);
    }

    #[test]
    fn a_sent_message_cannot_be_sent_again() {
        let w = world();
        let id = outbox_save(&w.store, draft("owner@byptah.com", "ann@example.org")).unwrap();
        outbox_send(&w.store, &w.fake, id).unwrap();
        assert!(outbox_send(&w.store, &w.fake, id).is_err());
        assert_eq!(w.fake.log.lock().unwrap().sends.len(), 1);
        let mut edit = draft("owner@byptah.com", "ann@example.org");
        edit.id = Some(id);
        assert!(outbox_save(&w.store, edit).is_err());
    }

    #[test]
    fn a_row_already_being_sent_is_refused() {
        let w = world();
        let id = outbox_save(&w.store, draft("owner@byptah.com", "ann@example.org")).unwrap();
        w.store
            .lock_conn()
            .unwrap()
            .execute("UPDATE mail_outbox SET state = 'sending'", [])
            .unwrap();
        assert!(outbox_send(&w.store, &w.fake, id).is_err());
        assert!(w.fake.log.lock().unwrap().sends.is_empty());
        assert!(outbox_delete(&w.store, id).is_err());
    }

    #[test]
    fn a_send_cut_off_by_a_quit_comes_back_unconfirmed_not_as_a_draft() {
        let w = world();
        let id = outbox_save(&w.store, draft("owner@byptah.com", "ann@example.org")).unwrap();
        let conn = w.store.lock_conn().unwrap();
        conn.execute("UPDATE mail_outbox SET state = 'sending'", [])
            .unwrap();
        assert_eq!(mail_store::fail_interrupted_sends(&conn).unwrap(), 1);
        let row = mail_store::get_outbox(&conn, id).unwrap().unwrap();
        assert_eq!(row.state, "unconfirmed");
        assert!(row.last_error.unwrap().contains("may have been delivered"));
        drop(conn);
        // And it is not offered for sending again as it stands.
        let error = outbox_send(&w.store, &w.fake, id).unwrap_err();
        assert!(error.message.contains("may already have delivered"));
        assert!(w.fake.log.lock().unwrap().sends.is_empty());
    }

    #[test]
    fn bookkeeping_that_fails_after_a_send_is_not_reported_as_a_failed_send() {
        let w = world();
        let store = w.store.clone();
        let owner = w.owner;
        *w.fake.on_send.lock().unwrap() = Some(Box::new(move || {
            // The mailbox disappears while the request is in flight.
            store
                .lock_conn()
                .unwrap()
                .execute("DELETE FROM mail_mailbox WHERE id = ?1", [owner])
                .unwrap();
        }));
        let report = send(&w, draft("owner@byptah.com", "ann@example.org")).unwrap();
        assert_eq!(report.ses_message_id, "ses-message-1");
        assert!(report.bookkeeping_error.is_some());
        assert_eq!(text(&w, "SELECT state FROM mail_outbox"), "sent");
        assert_eq!(count(&w, "SELECT COUNT(*) FROM mail_message"), 0);
        // And nothing offers a second send.
        let id = count(&w, "SELECT id FROM mail_outbox");
        assert!(outbox_send(&w.store, &w.fake, id).is_err());
        assert_eq!(w.fake.log.lock().unwrap().sends.len(), 1);
    }

    fn sparse_attachment(dir: &std::path::Path, bytes: u64) -> OutboxAttachment {
        std::fs::create_dir_all(dir).unwrap();
        let path = dir.join("big.bin");
        std::fs::File::create(&path)
            .unwrap()
            .set_len(bytes)
            .unwrap();
        OutboxAttachment {
            path: path.to_string_lossy().into_owned(),
            filename: "big.bin".into(),
            mime: "application/octet-stream".into(),
            cid: None,
        }
    }

    #[test]
    fn an_attachment_that_cannot_fit_is_refused_without_being_read() {
        let dir = std::env::temp_dir().join(format!("mono-send-{}", uuid::Uuid::new_v4()));
        // 31 MB is 41 MB once base64 has inflated it.
        let error = load_attachments(&[sparse_attachment(&dir, 31_000_000)])
            .err()
            .expect("refused");
        assert!(error.contains("40 MB"), "{error}");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_message_over_the_ses_limit_is_a_form_error_before_the_call() {
        let w = world();
        let dir = std::env::temp_dir().join(format!("mono-send-{}", uuid::Uuid::new_v4()));
        // 29.5 MB is 39.3 MB by the 4/3 rule, so it passes the early check;
        // the line breaks base64 adds are what push the message past 40 MB.
        let mut d = draft("owner@byptah.com", "ann@example.org");
        d.attachments = vec![sparse_attachment(&dir, 29_500_000)];
        let id = outbox_save(&w.store, d).unwrap();
        let error = outbox_send(&w.store, &w.fake, id).unwrap_err();
        assert!(error.contains("40 MB"), "{error}");
        assert!(w.fake.log.lock().unwrap().sends.is_empty());
        // Still a draft: fixing the attachment and sending again is possible.
        assert_eq!(text(&w, "SELECT state FROM mail_outbox"), "draft");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn attachments_reach_ses_intact() {
        let w = world();
        let dir = std::env::temp_dir().join(format!("mono-send-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("data.bin");
        let bytes: Vec<u8> = (0..=255u8).cycle().take(5000).collect();
        std::fs::write(&file, &bytes).unwrap();
        let mut d = draft("owner@byptah.com", "ann@example.org");
        d.attachments = vec![OutboxAttachment {
            path: file.to_string_lossy().into_owned(),
            filename: "data.bin".into(),
            mime: "application/octet-stream".into(),
            cid: None,
        }];
        send(&w, d).unwrap();
        let raw = w.fake.log.lock().unwrap().sends[0].raw.clone();
        let parsed = mail_parser::MessageParser::default().parse(&raw).unwrap();
        assert_eq!(parsed.attachment(0).unwrap().contents(), bytes.as_slice());
        assert_eq!(
            count(&w, "SELECT has_attachments FROM mail_message"),
            1,
            "the thread shows a paperclip"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn an_inline_image_without_its_file_is_refused_instead_of_sent_broken() {
        let w = world();
        let mut d = draft("owner@byptah.com", "ann@example.org");
        d.body_html = Some(r#"<p><img src="cid:mono-1.att@monoptah.local"></p>"#.into());
        let id = outbox_save(&w.store, d).unwrap();
        let error = outbox_send(&w.store, &w.fake, id).unwrap_err();
        assert!(error.contains("mono-1.att@monoptah.local"), "{error}");
        assert!(w.fake.log.lock().unwrap().sends.is_empty());
    }

    #[test]
    fn a_cid_inside_the_quoted_original_is_not_ours_to_attach() {
        let w = world();
        let quoted = r#"<blockquote><img src="cid:their-logo@example.org"></blockquote>"#;
        let mut d = draft("owner@byptah.com", "ann@example.org");
        d.quoted_html = Some(quoted.into());
        d.body_html = Some(format!("<p>reply</p>{quoted}"));
        send(&w, d).unwrap();
        assert_eq!(w.fake.log.lock().unwrap().sends.len(), 1);
    }

    #[test]
    fn recipients_are_checked_before_anything_is_sent() {
        let w = world();
        let mut none = draft("owner@byptah.com", "ann@example.org");
        none.to.clear();
        assert!(send(&w, none).unwrap_err().contains("recipient"));
        let mut bad = draft("owner@byptah.com", "not an address");
        bad.cc = vec![who("also bad")];
        let error = send(&w, bad).unwrap_err();
        assert!(error.contains("not a valid email address"), "{error}");
        // A bad blind copy fails the same way, not as an SES error afterwards.
        let mut bad_bcc = draft("owner@byptah.com", "ann@example.org");
        bad_bcc.bcc = vec![who("x@y.com>, z@w.com")];
        assert!(send(&w, bad_bcc)
            .unwrap_err()
            .contains("not a valid email address"));
        assert!(w.fake.log.lock().unwrap().sends.is_empty());
    }

    #[test]
    fn the_from_address_must_be_one_of_our_mailboxes() {
        let w = world();
        let error = outbox_save(&w.store, draft("stranger@elsewhere.com", "a@b.com")).unwrap_err();
        assert!(error.contains("not one of your mailboxes"), "{error}");
    }

    #[test]
    fn the_from_address_picks_the_source_that_sends() {
        let w = world();
        send(&w, draft("sales@acme.com", "ann@example.org")).unwrap();
        let log = w.fake.log.lock().unwrap();
        assert_eq!(log.sends[0].bucket, "bucket-b");
        assert_eq!(log.sends[0].from, "sales@acme.com");
        assert_eq!(log.puts[0].0, "bucket-b");
    }

    #[test]
    fn a_reply_gets_the_parents_references_then_the_parents_id_capped_at_twenty() {
        let w = world();
        let chain: Vec<String> = (1..=30).map(|i| format!("<c{i}@x>")).collect();
        let (parent, _) = arrive(
            &w,
            w.owner,
            &inbound(
                "p30",
                "Long thread",
                &format!(
                    "In-Reply-To: <c30@x>\r\nReferences: {}\r\n",
                    chain.join(" ")
                ),
            ),
        );
        let mut d = draft("owner@byptah.com", "ann@example.org");
        d.kind = "reply".into();
        d.reply_to_message_id = Some(parent);
        let id = outbox_save(&w.store, d).unwrap();
        let row = mail_store::get_outbox(&w.store.lock_conn().unwrap(), id)
            .unwrap()
            .unwrap();
        assert_eq!(row.references.len(), 20);
        assert_eq!(row.references[0], "c1@x");
        // Oldest first, parent last: the order the next reader expects.
        assert_eq!(row.references[19], "p30@example.org");
        assert_eq!(row.references[18], "c30@x");
        assert_eq!(row.in_reply_to.as_deref(), Some("p30@example.org"));
    }

    #[test]
    fn a_forward_starts_its_own_conversation() {
        let w = world();
        let (parent, _) = arrive(&w, w.owner, &inbound("p1", "Question", ""));
        let mut d = draft("owner@byptah.com", "ann@example.org");
        d.kind = "forward".into();
        d.subject = "Fwd: Question".into();
        d.reply_to_message_id = Some(parent);
        let id = outbox_save(&w.store, d).unwrap();
        let row = mail_store::get_outbox(&w.store.lock_conn().unwrap(), id)
            .unwrap()
            .unwrap();
        assert!(row.in_reply_to.is_none() && row.references.is_empty());
        assert!(row.thread_id.is_none());
    }

    #[test]
    fn an_unverified_sender_is_stopped_with_the_reason_that_is_true() {
        let w = world();
        let set = |status: &str| {
            let conn = w.store.lock_conn().unwrap();
            mail_store::replace_identities(
                &conn,
                w.a,
                &[MailIdentity {
                    identity: "byptah.com".into(),
                    kind: "domain".into(),
                    status: status.into(),
                }],
            )
            .unwrap();
        };
        let id = outbox_save(&w.store, draft("owner@byptah.com", "ann@example.org")).unwrap();
        let first = |w: &World| {
            outbox_check(&w.store, &w.fake, id)
                .unwrap()
                .into_iter()
                .next()
                .unwrap()
        };
        set("sending_disabled");
        let disabled = first(&w);
        assert_eq!(disabled.kind, "sendingDisabled");
        assert!(disabled.message.contains("turned sending off"));
        assert!(disabled.message.contains("not a DNS problem"));
        set("pending");
        assert_eq!(first(&w).kind, "identityPending");
        set("failed");
        assert_eq!(first(&w).kind, "identityNotVerified");
        // The same reason stops the send itself.
        let error = outbox_send(&w.store, &w.fake, id).unwrap_err();
        assert!(error.contains("byptah.com"), "{error}");
        assert!(w.fake.log.lock().unwrap().sends.is_empty());
    }

    #[test]
    fn a_sandboxed_region_blocks_unverified_recipients_before_the_call() {
        let w = world();
        w.fake.sandbox.store(true, Ordering::SeqCst);
        let mut d = draft("owner@byptah.com", "ann@example.org");
        d.cc = vec![who("friend@byptah.com")];
        let id = outbox_save(&w.store, d).unwrap();
        let problems = outbox_check(&w.store, &w.fake, id).unwrap();
        assert_eq!(problems.len(), 1);
        assert_eq!(problems[0].kind, "sandboxRecipients");
        // The verified domain's address is fine; only the stranger is named.
        assert_eq!(problems[0].addresses, vec!["ann@example.org".to_string()]);
        assert!(problems[0].message.contains("us-east-1"));
        let error = outbox_send(&w.store, &w.fake, id).unwrap_err();
        assert!(error.contains("sandbox"), "{error}");
        assert!(w.fake.log.lock().unwrap().sends.is_empty());
        assert_eq!(text(&w, "SELECT state FROM mail_outbox"), "draft");
    }

    #[test]
    fn the_sandbox_is_judged_per_source_not_per_account_flag_alone() {
        let w = world();
        w.fake.sandbox.store(false, Ordering::SeqCst);
        let id = outbox_save(&w.store, draft("owner@byptah.com", "ann@example.org")).unwrap();
        assert!(outbox_check(&w.store, &w.fake, id).unwrap().is_empty());
    }

    #[test]
    fn a_recipient_verified_since_the_cache_was_taken_is_not_refused() {
        let w = world();
        w.fake.sandbox.store(true, Ordering::SeqCst);
        // SES knows ann's address now; the local cache does not.
        w.fake.identities.lock().unwrap().insert(
            w.a,
            vec![verified("byptah.com"), verified("ann@example.org")],
        );
        let id = outbox_save(&w.store, draft("owner@byptah.com", "ann@example.org")).unwrap();
        assert!(outbox_check(&w.store, &w.fake, id).unwrap().is_empty());
        assert_eq!(w.fake.log.lock().unwrap().refreshes, 1);
    }

    #[test]
    fn a_paused_ses_account_is_named_not_left_to_a_rejection() {
        let w = world();
        w.fake.paused.store(true, Ordering::SeqCst);
        let id = outbox_save(&w.store, draft("owner@byptah.com", "ann@example.org")).unwrap();
        let problems = outbox_check(&w.store, &w.fake, id).unwrap();
        assert_eq!(problems[0].kind, "sendingPaused");
    }

    #[test]
    fn the_from_picker_lists_only_verified_mailboxes_and_never_the_sent_archive() {
        let w = world();
        send(&w, draft("owner@byptah.com", "ann@example.org")).unwrap();
        {
            let conn = w.store.lock_conn().unwrap();
            // A mailbox on an unverified domain.
            upsert_mailboxes(
                &conn,
                w.b,
                &[MailboxInput {
                    address: "x@unverified.org".into(),
                    s3_prefix: "x/".into(),
                    display_name: "X".into(),
                    color: None,
                    sort_order: 0,
                }],
            )
            .unwrap();
        }
        let addresses: Vec<String> = sendable_mailboxes(&w.store)
            .unwrap()
            .into_iter()
            .map(|m| m.mailbox.address)
            .collect();
        assert_eq!(
            addresses,
            vec![
                "owner@byptah.com".to_string(),
                "support@byptah.com".to_string(),
                "sales@acme.com".to_string()
            ]
        );
    }

    #[test]
    fn the_sent_archive_stays_unsendable_through_every_identity_change() {
        let w = world();
        send(&w, draft("owner@byptah.com", "ann@example.org")).unwrap();
        let archive = |w: &World| {
            count(
                w,
                "SELECT is_sendable FROM mail_mailbox WHERE s3_prefix = 'sent/'",
            )
        };
        assert_eq!(archive(&w), 0);
        {
            let conn = w.store.lock_conn().unwrap();
            // A refresh that happens to verify the archive's placeholder domain.
            mail_store::replace_identities(
                &conn,
                w.a,
                &[verified("byptah.com"), verified("monoptah.invalid")],
            )
            .unwrap();
        }
        assert_eq!(archive(&w), 0);
        {
            // Repointing the source clears identities and resets every other
            // mailbox to the sendable default.
            let conn = w.store.lock_conn().unwrap();
            let mut input = AccountInput {
                id: Some(w.a),
                provider: "aws".into(),
                credential_id: count_cred(&conn),
                s3_credential_id: None,
                label: "Byptah".into(),
                ses_region: "eu-west-1".into(),
                bucket: "bucket-a".into(),
                s3_region: "us-east-1".into(),
                default_from: None,
                attach_policy: "on_demand".into(),
                image_policy: "ask".into(),
                enabled: true,
                sort_order: 0,
            };
            input.ses_region = "eu-west-1".into();
            mail_store::upsert_account(&conn, &input).unwrap();
        }
        assert_eq!(archive(&w), 0);
        assert_eq!(
            count(
                &w,
                "SELECT is_sendable FROM mail_mailbox WHERE address = 'owner@byptah.com'"
            ),
            1
        );
    }

    #[test]
    fn a_later_sync_updates_the_sent_message_instead_of_duplicating_it() {
        let w = world();
        let mut d = draft("owner@byptah.com", "ann@example.org");
        d.bcc = vec![who("secret@hidden.org")];
        send(&w, d).unwrap();
        for _ in 0..2 {
            let reports = sync(&w);
            assert!(reports.iter().all(|r| r.error.is_none()), "{reports:?}");
            assert_eq!(reports.iter().map(|r| r.inserted).sum::<usize>(), 0);
            assert_eq!(reports.iter().map(|r| r.reparsed).sum::<usize>(), 0);
        }
        assert_eq!(count(&w, "SELECT COUNT(*) FROM mail_message"), 1);
        assert_eq!(count(&w, "SELECT COUNT(*) FROM mail_thread"), 1);
        // A re-parse of the stored copy would have lost who was blind-copied.
        assert!(text(&w, "SELECT bcc_json FROM mail_message").contains("secret@hidden.org"));
        assert_eq!(
            text(
                &w,
                "SELECT state FROM mail_object WHERE s3_key LIKE 'sent/%'"
            ),
            "parsed"
        );
    }

    #[test]
    fn another_machine_files_the_sent_copy_into_the_conversation_it_answers() {
        let w = world();
        let (parent, thread) = arrive(&w, w.owner, &inbound("p1", "Question", ""));
        let mut d = draft("owner@byptah.com", "ann@example.org");
        d.kind = "reply".into();
        d.reply_to_message_id = Some(parent);
        send(&w, d).unwrap();
        // This machine never had the local row: only the object in S3.
        {
            let conn = w.store.lock_conn().unwrap();
            let id: i64 = conn
                .query_row(
                    "SELECT id FROM mail_message WHERE direction = 'outbound'",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            mail_store::remove_message(&conn, id).unwrap();
            conn.execute("DELETE FROM mail_object WHERE s3_key LIKE 'sent/%'", [])
                .unwrap();
        }
        let reports = sync(&w);
        assert_eq!(reports.iter().map(|r| r.inserted).sum::<usize>(), 1);
        assert_eq!(count(&w, "SELECT COUNT(*) FROM mail_thread"), 1);
        assert_eq!(
            count(
                &w,
                &format!(
                    "SELECT COUNT(*) FROM mail_message
                     WHERE thread_id = {thread} AND mailbox_id = {} AND direction = 'outbound'
                       AND is_read = 1",
                    w.owner
                )
            ),
            1
        );
        // Nothing is filed under the archive mailbox itself.
        assert_eq!(
            count(
                &w,
                "SELECT COUNT(*) FROM mail_thread t JOIN mail_mailbox b ON b.id = t.mailbox_id
                 WHERE b.s3_prefix = 'sent/'"
            ),
            0
        );
        assert_eq!(
            count(
                &w,
                &format!("SELECT unread_count FROM mail_thread WHERE id = {thread}")
            ),
            1
        );
    }

    #[test]
    fn no_prefix_that_overlaps_the_sent_archive_can_be_a_mailbox() {
        let w = world();
        for prefix in ["sent/", "sent", "s", "", "se", "sent/2024/", "sent/x"] {
            let error = crate::mail::mailboxes_upsert(
                &w.store,
                w.a,
                vec![MailboxInput {
                    address: "x@byptah.com".into(),
                    s3_prefix: prefix.into(),
                    display_name: "X".into(),
                    color: None,
                    sort_order: 0,
                }],
            )
            .unwrap_err();
            assert!(error.contains("reserved"), "{prefix:?}: {error}");
        }
        // Neighbours that S3 would not confuse with it are fine.
        for prefix in ["sentinel/", "owner/", "a/sent/"] {
            crate::mail::mailboxes_upsert(
                &w.store,
                w.a,
                vec![MailboxInput {
                    address: format!(
                        "{}@byptah.com",
                        prefix.trim_end_matches('/').replace('/', "-")
                    ),
                    s3_prefix: prefix.into(),
                    display_name: "X".into(),
                    color: None,
                    sort_order: 0,
                }],
            )
            .unwrap_or_else(|e| panic!("{prefix}: {e}"));
        }
    }

    #[test]
    fn the_settings_and_sidebar_never_list_the_sent_archive() {
        let w = world();
        send(&w, draft("owner@byptah.com", "ann@example.org")).unwrap();
        assert_eq!(
            count(
                &w,
                "SELECT COUNT(*) FROM mail_mailbox WHERE s3_prefix = 'sent/'"
            ),
            1
        );
        let listed = crate::mail::sources(&w.store).unwrap();
        assert!(listed
            .iter()
            .flat_map(|s| &s.mailboxes)
            .all(|m| m.s3_prefix != "sent/"));
        let status = crate::mail::status(&w.store, 3600).unwrap();
        assert!(status
            .sources
            .iter()
            .flat_map(|s| &s.mailboxes)
            .all(|m| m.mailbox.s3_prefix != "sent/"));
    }

    #[test]
    fn a_sent_message_re_evaluates_the_smart_folders_of_its_thread() {
        let w = world();
        let (parent, thread) = arrive(&w, w.owner, &inbound("p1", "Question", ""));
        let folder = {
            let conn = w.store.lock_conn().unwrap();
            mail_store::upsert_folder(
                &conn,
                &serde_json::from_str(r#"{"name":"Zebras"}"#).unwrap(),
                &[mail_store::Rule {
                    field: "body".into(),
                    op: "contains".into(),
                    value: serde_json::Value::String("zebra".into()),
                    negate: false,
                }],
            )
            .unwrap()
            .id
        };
        let members = |w: &World| {
            count(
                w,
                &format!(
                    "SELECT COUNT(*) FROM mail_folder_member WHERE folder_id = {folder} AND thread_id = {thread}"
                ),
            )
        };
        assert_eq!(members(&w), 0);
        // Only our reply mentions the word, so only its arrival can file the thread.
        let mut d = draft("owner@byptah.com", "ann@example.org");
        d.kind = "reply".into();
        d.reply_to_message_id = Some(parent);
        d.body_text = Some("a zebra crossed".into());
        send(&w, d).unwrap();
        assert_eq!(members(&w), 1);
    }

    #[test]
    fn only_one_caller_can_claim_a_row_to_send() {
        // The build step refuses a row that is already sending, but two sends
        // can both get past it before either claims; the claim is the lock.
        let w = world();
        let id = outbox_save(&w.store, draft("owner@byptah.com", "ann@example.org")).unwrap();
        let conn = w.store.lock_conn().unwrap();
        assert!(mail_store::claim_outbox_for_send(&conn, id).unwrap());
        assert!(!mail_store::claim_outbox_for_send(&conn, id).unwrap());
        mail_store::mark_outbox_sent(&conn, id, "x").unwrap();
        assert!(!mail_store::claim_outbox_for_send(&conn, id).unwrap());
    }

    #[test]
    fn a_verified_domain_or_address_covers_its_recipients() {
        let ids = [verified("byptah.com"), verified("solo@example.org")];
        let none = |list: &[&str]| {
            let list: Vec<String> = list.iter().map(|s| s.to_string()).collect();
            uncovered(&list, &ids)
        };
        assert!(none(&["a@byptah.com", "A@BYPTAH.COM", "x@mail.byptah.com"]).is_empty());
        assert!(none(&["solo@example.org"]).is_empty());
        assert_eq!(none(&["other@example.org"]), vec!["other@example.org"]);
        assert_eq!(none(&["a@notbyptah.com"]), vec!["a@notbyptah.com"]);
        assert_eq!(none(&["nonsense"]), vec!["nonsense"]);
    }

    #[test]
    fn a_draft_keeps_its_document_and_a_failed_send_goes_back_to_a_draft_when_edited() {
        let w = world();
        let doc = serde_json::json!({"type":"doc","content":[
            {"type":"paragraph","content":[{"type":"text","text":"héllo \"quoted\" \u{1F600}"}]}]});
        let mut d = draft("Owner@Byptah.com", "ann@example.org");
        d.body_json = Some(doc.clone());
        let id = outbox_save(&w.store, d.clone()).unwrap();
        let rows = outbox_list(&w.store).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].body_json.as_ref(), Some(&doc));
        // Stored as the mailbox spells it, whatever case the picker sent.
        assert_eq!(rows[0].from_addr, "owner@byptah.com");

        *w.fake.ses_error.lock().unwrap() = Some(SesError {
            kind: SesErrorKind::Rejected,
            message: "no".into(),
        });
        assert!(outbox_send(&w.store, &w.fake, id).is_err());
        assert_eq!(outbox_list(&w.store).unwrap()[0].state, "failed");
        d.id = Some(id);
        d.subject = "Edited".into();
        outbox_save(&w.store, d).unwrap();
        let row = &outbox_list(&w.store).unwrap()[0];
        assert_eq!(
            (row.state.as_str(), row.last_error.as_deref()),
            ("draft", None)
        );
        assert_eq!(row.subject, "Edited");

        outbox_delete(&w.store, id).unwrap();
        assert!(outbox_list(&w.store).unwrap().is_empty());
    }

    #[test]
    fn a_send_that_may_have_been_delivered_cannot_be_retried_as_it_stands() {
        for kind in [SesErrorKind::Unconfirmed, SesErrorKind::Other] {
            let w = world();
            *w.fake.ses_error.lock().unwrap() = Some(SesError {
                kind,
                message: "The connection dropped.".into(),
            });
            let id = outbox_save(&w.store, draft("owner@byptah.com", "ann@example.org")).unwrap();
            let error = outbox_send(&w.store, &w.fake, id).unwrap_err();
            assert!(!error.retry_safe, "{kind:?}");
            assert_eq!(text(&w, "SELECT state FROM mail_outbox"), "unconfirmed");
            // The next click is refused and SES hears nothing more.
            *w.fake.ses_error.lock().unwrap() = None;
            let again = outbox_send(&w.store, &w.fake, id).unwrap_err();
            assert!(again.message.contains("may already have delivered"));
            assert_eq!(w.fake.log.lock().unwrap().sends.len(), 1);
            // Saving does not re-arm it, however often a composer does that.
            let mut edit = draft("owner@byptah.com", "ann@example.org");
            edit.id = Some(id);
            edit.subject = "Edited while unconfirmed".into();
            outbox_save(&w.store, edit.clone()).unwrap();
            outbox_save(&w.store, edit).unwrap();
            assert_eq!(text(&w, "SELECT state FROM mail_outbox"), "unconfirmed");
            assert_eq!(
                text(&w, "SELECT subject FROM mail_outbox"),
                "Edited while unconfirmed",
                "the edit is kept"
            );
            assert!(text(&w, "SELECT last_error FROM mail_outbox").contains("dropped"));
            assert!(outbox_send(&w.store, &w.fake, id).is_err());
            assert_eq!(w.fake.log.lock().unwrap().sends.len(), 1);
            // Only the explicit step does.
            outbox_rearm(&w.store, id).unwrap();
            assert!(outbox_rearm(&w.store, id).is_err(), "and only from there");
            outbox_send(&w.store, &w.fake, id).unwrap();
            assert_eq!(w.fake.log.lock().unwrap().sends.len(), 2);
        }
    }

    #[test]
    fn a_refusal_says_it_is_safe_to_retry_and_carries_its_kind() {
        let w = world();
        *w.fake.ses_error.lock().unwrap() = Some(SesError {
            kind: SesErrorKind::Rejected,
            message: "no".into(),
        });
        let id = outbox_save(&w.store, draft("owner@byptah.com", "ann@example.org")).unwrap();
        let error = outbox_send(&w.store, &w.fake, id).unwrap_err();
        assert_eq!((error.kind, error.retry_safe), ("rejected", true));
        assert_eq!(text(&w, "SELECT state FROM mail_outbox"), "failed");
        let form = outbox_send(&w.store, &w.fake, 9999).unwrap_err();
        assert_eq!((form.kind, form.retry_safe), ("form", true));
    }

    #[test]
    fn an_image_sharing_a_cid_with_the_quoted_original_is_refused() {
        let w = world();
        let dir = std::env::temp_dir().join(format!("mono-send-{}", uuid::Uuid::new_v4()));
        let mut att = sparse_attachment(&dir, 8);
        // What a composer given the same draft id for every draft would emit.
        let cid = "mono-new.att1@monoptah.local";
        att.cid = Some(cid.into());
        let quoted = format!(r#"<blockquote><img src="cid:{cid}"></blockquote>"#);
        let mut d = draft("owner@byptah.com", "ann@example.org");
        d.attachments = vec![att];
        d.body_html = Some(format!(r#"<p><img src="cid:{cid}"></p>{quoted}"#));
        d.quoted_html = Some(quoted);
        let id = outbox_save(&w.store, d).unwrap();
        let error = outbox_send(&w.store, &w.fake, id).unwrap_err();
        assert!(
            error.message.contains("same internal id"),
            "{}",
            error.message
        );
        assert!(w.fake.log.lock().unwrap().sends.is_empty());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn an_existing_bucket_root_mailbox_does_not_swallow_sent_objects_as_inbound() {
        let w = world();
        // Set up before the prefix was refused: it lists every key in the bucket.
        w.store
            .lock_conn()
            .unwrap()
            .execute(
                "INSERT INTO mail_mailbox (account_id, address, domain, s3_prefix, display_name)
                 VALUES (?1, 'root@byptah.com', 'byptah.com', '', 'Root')",
                [w.a],
            )
            .unwrap();
        send(&w, draft("owner@byptah.com", "ann@example.org")).unwrap();
        for _ in 0..2 {
            let reports = sync(&w);
            assert!(reports.iter().all(|r| r.error.is_none()), "{reports:?}");
        }
        assert_eq!(count(&w, "SELECT COUNT(*) FROM mail_message"), 1);
        assert_eq!(
            count(
                &w,
                "SELECT COUNT(*) FROM mail_message WHERE direction = 'inbound' OR is_read = 0"
            ),
            0
        );
        assert_eq!(
            count(
                &w,
                "SELECT COUNT(*) FROM mail_thread t JOIN mail_mailbox b ON b.id = t.mailbox_id
                 WHERE b.address = 'root@byptah.com'"
            ),
            0
        );
    }

    #[test]
    fn a_prefix_error_names_the_row_that_caused_it() {
        let w = world();
        let row = |address: &str, prefix: &str| MailboxInput {
            address: address.into(),
            s3_prefix: prefix.into(),
            display_name: "X".into(),
            color: None,
            sort_order: 0,
        };
        let error = crate::mail::mailboxes_upsert(
            &w.store,
            w.a,
            vec![
                row("a@byptah.com", "fine/"),
                row("b@byptah.com", "sent/2024/"),
            ],
        )
        .unwrap_err();
        assert!(error.contains("'sent/2024/'"), "{error}");
        assert!(!error.contains("'fine/'"), "{error}");
    }

    #[test]
    fn a_row_can_only_become_sent_from_sending() {
        let w = world();
        let id = outbox_save(&w.store, draft("owner@byptah.com", "ann@example.org")).unwrap();
        let conn = w.store.lock_conn().unwrap();
        for state in ["draft", "failed", "unconfirmed", "sent"] {
            conn.execute("UPDATE mail_outbox SET state = ?1", [state])
                .unwrap();
            assert!(
                !mail_store::mark_outbox_sent(&conn, id, "x").unwrap(),
                "{state}"
            );
            let now: String = conn
                .query_row("SELECT state FROM mail_outbox", [], |r| r.get(0))
                .unwrap();
            assert_eq!(now, state);
        }
        conn.execute("UPDATE mail_outbox SET state = 'sending'", [])
            .unwrap();
        assert!(mail_store::mark_outbox_sent(&conn, id, "x").unwrap());
    }

    fn count_cred(conn: &rusqlite::Connection) -> i64 {
        conn.query_row("SELECT id FROM mail_credential", [], |row| row.get(0))
            .unwrap()
    }
}
