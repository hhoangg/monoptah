//! Thread grouping: References first, subject fallback behind three guards
//! (spec section 6). Threads never cross mailboxes.
// Some helpers are only used by the sync task and the tests.
#![allow(dead_code)]

use std::collections::HashSet;

use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use unicode_normalization::UnicodeNormalization;

use crate::mail_mime::{MailAddress, ParsedMail};
use crate::mail_store::{self, NewAttachment, NewMessage, ThreadAggregates};

const SUBJECT_WINDOW_SECS: i64 = 30 * 24 * 60 * 60;
const MAX_LIST_TAG_CHARS: usize = 40;
const REPLY_PREFIXES: [&str; 12] = [
    "re",
    "aw",
    "sv",
    "antw",
    "res",
    "fwd",
    "fw",
    "vs",
    "tr",
    "rv",
    "trả lời",
    "chuyển tiếp",
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Person {
    name: Option<String>,
    addr: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Ingested {
    pub message_id: i64,
    pub thread_id: i64,
    /// The message was already stored; nothing was changed.
    pub duplicate: bool,
}

fn strip_list_tag(s: &str) -> Option<&str> {
    let rest = s.strip_prefix('[')?;
    let end = rest.find(']')?;
    if end == 0 || rest[..end].chars().count() > MAX_LIST_TAG_CHARS {
        return None;
    }
    Some(rest[end + 1..].trim_start())
}

fn after_reply_prefix(rest: &str) -> Option<&str> {
    let mut rest = rest.trim_start();
    if let Some(counted) = rest.strip_prefix('[') {
        let end = counted.find(']')?;
        let digits = &counted[..end];
        if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        rest = counted[end + 1..].trim_start();
    }
    Some(rest.strip_prefix(':')?.trim_start())
}

fn strip_reply_prefix(s: &str) -> Option<&str> {
    REPLY_PREFIXES
        .iter()
        .find_map(|p| s.strip_prefix(p).and_then(after_reply_prefix))
}

/// Subject with list tags and reply/forward prefixes removed, for grouping.
/// NFKC runs before the prefix match so a decomposed "Trả lời" or a
/// full-width "RE:" is still recognised.
pub(crate) fn subject_key(subject: &str) -> String {
    let normalized: String = subject.nfkc().collect::<String>().to_lowercase();
    let mut s = normalized.trim();
    while let Some(rest) = strip_list_tag(s).or_else(|| strip_reply_prefix(s)) {
        s = rest.trim();
    }
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn clean_addr(addr: &str) -> String {
    // Only case is folded: Gmail-style dots and +tags are distinct recipients.
    addr.trim().to_lowercase()
}

fn person(name: Option<&str>, addr: &str) -> Option<Person> {
    let addr = clean_addr(addr);
    if addr.is_empty() {
        return None;
    }
    Some(Person {
        name: name
            .map(str::trim)
            .filter(|n| !n.is_empty())
            .map(String::from),
        addr,
    })
}

fn people_json(list: &[MailAddress]) -> String {
    let people: Vec<Person> = list
        .iter()
        .filter_map(|a| person(a.name.as_deref(), &a.address))
        .collect();
    serde_json::to_string(&people).unwrap_or_else(|_| "[]".into())
}

fn parse_people(json: &str) -> Vec<Person> {
    serde_json::from_str(json).unwrap_or_default()
}

/// Everyone a conversation is "with": From, Reply-To, To, Cc, in that order,
/// minus the source's own mailboxes. Bcc is left out because it says nothing
/// about who the other side is. Own mailboxes are dropped because every
/// inbound message is addressed to one, so keeping them would make any two
/// strangers writing to the same inbox overlap.
fn counterparties(
    from: Option<Person>,
    reply_to: Option<Person>,
    to: Vec<Person>,
    cc: Vec<Person>,
    own: &[String],
) -> Vec<Person> {
    let mut seen = HashSet::new();
    from.into_iter()
        .chain(reply_to)
        .chain(to)
        .chain(cc)
        .filter(|p| !own.contains(&p.addr) && seen.insert(p.addr.clone()))
        .collect()
}

fn recompute_thread(conn: &Connection, thread_id: i64, own: &[String]) -> rusqlite::Result<()> {
    let rows = mail_store::thread_message_rows(conn, thread_id)?;
    let Some(first) = rows.first() else {
        // Every message is `gone`: nothing is shown, so nothing is counted. The
        // rest is left for the recompute that follows a revive.
        conn.execute(
            "UPDATE mail_thread SET message_count = 0, unread_count = 0 WHERE id = ?1",
            [thread_id],
        )?;
        return Ok(());
    };
    let mut participants: Vec<Person> = Vec::new();
    let mut seen = HashSet::new();
    for r in &rows {
        let people = counterparties(
            person(r.from_name.as_deref(), &r.from_addr),
            r.reply_to_addr.as_deref().and_then(|a| person(None, a)),
            parse_people(&r.to_json),
            parse_people(&r.cc_json),
            own,
        );
        for p in people {
            if seen.insert(p.addr.clone()) {
                participants.push(p);
            }
        }
    }
    mail_store::update_thread_aggregates(
        conn,
        thread_id,
        &ThreadAggregates {
            subject: first.subject.clone(),
            subject_key: first.subject_key.clone(),
            participants_json: serde_json::to_string(&participants).unwrap_or_else(|_| "[]".into()),
            first_message_at: first.date_utc,
            last_message_at: rows
                .iter()
                .map(|r| r.date_utc)
                .max()
                .unwrap_or(first.date_utc),
            message_count: rows.len() as i64,
            unread_count: rows.iter().filter(|r| !r.is_read).count() as i64,
            has_attachments: rows.iter().any(|r| r.has_attachments),
        },
    )
}

/// Covered by `mail_sync`'s
/// `a_vanished_message_leaves_no_unread_badge_and_a_revived_one_restores_it`,
/// including the zero-row branch of `recompute_thread`.
///
/// Recomputes one thread's aggregates on the caller's handle (and so inside the
/// caller's transaction). For state changes that happen outside ingest, such as
/// a message going `gone` or coming back.
pub(crate) fn refresh_thread(conn: &Connection, thread_id: i64) -> rusqlite::Result<()> {
    let account_id: i64 = conn.query_row(
        "SELECT account_id FROM mail_thread WHERE id = ?1",
        [thread_id],
        |row| row.get(0),
    )?;
    let own = mail_store::account_mailbox_addresses(conn, account_id)?;
    recompute_thread(conn, thread_id, &own)
}

/// Folds `losers` into `survivor` and refreshes its aggregates. Safe to repeat:
/// ids that no longer exist are skipped. Callers pass the lowest thread id as
/// the survivor, which is the oldest row.
pub(crate) fn merge_threads(
    conn: &Connection,
    survivor: i64,
    losers: &[i64],
    own: &[String],
) -> rusqlite::Result<()> {
    for &loser in losers.iter().filter(|&&l| l != survivor) {
        mail_store::absorb_thread(conn, survivor, loser)?;
    }
    recompute_thread(conn, survivor, own)
}

fn subject_fallback(
    conn: &Connection,
    mailbox_id: i64,
    key: &str,
    date_utc: i64,
    people: &[Person],
) -> rusqlite::Result<Option<i64>> {
    if key.is_empty() {
        return Ok(None);
    }
    let addrs: HashSet<&str> = people.iter().map(|p| p.addr.as_str()).collect();
    let candidates = mail_store::subject_thread_candidates(
        conn,
        mailbox_id,
        key,
        date_utc - SUBJECT_WINDOW_SECS,
    )?;
    Ok(candidates
        .into_iter()
        .find(|(_, json)| {
            parse_people(json)
                .iter()
                .any(|p| addrs.contains(p.addr.as_str()))
        })
        .map(|(id, _)| id))
}

/// Stores a parsed message in the thread it belongs to, creating or merging
/// threads as needed. `fallback_date` (epoch seconds, e.g. the S3 object time)
/// stands in when the message carries neither a usable `Date` nor `Received`.
/// Runs on the caller's transaction, so a page of messages commits as one.
pub(crate) fn ingest_message(
    conn: &Connection,
    mailbox_id: i64,
    direction: &str,
    object_id: Option<i64>,
    mail: &ParsedMail,
    fallback_date: i64,
) -> rusqlite::Result<Ingested> {
    if let Some(hdr) = mail.message_id.as_deref() {
        if let Some((message_id, thread_id)) =
            mail_store::find_message_by_header(conn, mailbox_id, hdr)?
        {
            return Ok(Ingested {
                message_id,
                thread_id,
                duplicate: true,
            });
        }
    }

    let account_id = mail_store::mailbox_account_id(conn, mailbox_id)?;
    let own = mail_store::account_mailbox_addresses(conn, account_id)?;

    let date_utc = mail.date_utc.or(mail.received_at).unwrap_or(fallback_date);
    let received_at = mail.received_at.unwrap_or(date_utc);
    let key = subject_key(&mail.subject);

    let mut seen = HashSet::new();
    let refs: Vec<String> = mail
        .referenced_ids
        .iter()
        .filter(|r| Some(r.as_str()) != mail.message_id.as_deref() && seen.insert(r.as_str()))
        .cloned()
        .collect();

    let people = counterparties(
        mail.from
            .as_ref()
            .and_then(|a| person(a.name.as_deref(), &a.address)),
        mail.reply_to
            .as_ref()
            .and_then(|a| person(None, &a.address)),
        parse_people(&people_json(&mail.to)),
        parse_people(&people_json(&mail.cc)),
        &own,
    );

    let linked = mail_store::threads_for_refs(conn, mailbox_id, mail.message_id.as_deref(), &refs)?;
    let thread_id = match linked.split_first() {
        Some((&survivor, losers)) => {
            merge_threads(conn, survivor, losers, &own)?;
            survivor
        }
        None => match subject_fallback(conn, mailbox_id, &key, date_utc, &people)? {
            Some(id) => id,
            None => mail_store::insert_thread(conn, mailbox_id, &mail.subject, &key, date_utc)?,
        },
    };

    let from = mail.from.as_ref();
    let message_id = mail_store::insert_message(
        conn,
        &NewMessage {
            mailbox_id,
            thread_id,
            object_id,
            direction,
            message_id_hdr: mail.message_id.as_deref(),
            in_reply_to: mail.in_reply_to.as_deref(),
            subject: &mail.subject,
            subject_key: &key,
            from_name: from.and_then(|a| a.name.as_deref()),
            from_addr: from.map(|a| a.address.as_str()).unwrap_or(""),
            reply_to_addr: mail.reply_to.as_ref().map(|a| a.address.as_str()),
            to_json: &people_json(&mail.to),
            cc_json: &people_json(&mail.cc),
            bcc_json: &people_json(&mail.bcc),
            date_utc,
            received_at,
            snippet: &mail.snippet,
            body_text: mail.body_text.as_deref(),
            body_html: mail.body_html.as_deref(),
            has_attachments: mail.has_attachments(),
            size_bytes: mail.size_bytes as i64,
            spf: mail.spf.as_deref(),
            dkim: mail.dkim.as_deref(),
            dmarc: mail.dmarc.as_deref(),
            headers_json: &mail.headers_json,
        },
    )?;
    mail_store::insert_message_refs(conn, message_id, &refs)?;
    let attachments: Vec<NewAttachment> = mail
        .attachments
        .iter()
        .map(|a| NewAttachment {
            part_index: a.part_index as i64,
            filename: &a.filename,
            mime_type: &a.mime_type,
            size_bytes: a.size_bytes as i64,
            content_id: a.content_id.as_deref(),
            is_inline: a.is_inline,
        })
        .collect();
    mail_store::insert_attachments(conn, message_id, &attachments)?;
    recompute_thread(conn, thread_id, &own)?;
    Ok(Ingested {
        message_id,
        thread_id,
        duplicate: false,
    })
}

/// Re-parse of a changed object: the message is removed and ingested again, so
/// the envelope and the thread follow the new content. The read flag, star and
/// state the user set carry over. A `Message-ID` already used by another
/// message returns `duplicate` and changes nothing. Deleting first is what lets threading see the
/// message as new instead of as its own duplicate.
pub(crate) fn replace_message(
    conn: &Connection,
    mailbox_id: i64,
    message_id: i64,
    mail: &ParsedMail,
    fallback_date: i64,
) -> rusqlite::Result<Ingested> {
    // Checked before the delete: a collision with another message must leave
    // this one in place, not lose it.
    if let Some(hdr) = mail.message_id.as_deref() {
        if let Some((other, thread_id)) = mail_store::find_message_by_header(conn, mailbox_id, hdr)?
        {
            if other != message_id {
                return Ok(Ingested {
                    message_id: other,
                    thread_id,
                    duplicate: true,
                });
            }
        }
    }
    let removed = mail_store::remove_message(conn, message_id)?;
    let ingested = ingest_message(
        conn,
        mailbox_id,
        &removed.direction,
        removed.object_id,
        mail,
        fallback_date,
    )?;
    let account_id = mail_store::mailbox_account_id(conn, mailbox_id)?;
    let own = mail_store::account_mailbox_addresses(conn, account_id)?;
    if !ingested.duplicate {
        mail_store::restore_message_flags(conn, ingested.message_id, &removed)?;
        // A new thread starts `active`; a restored archived/gone message must
        // not leave it showing in the inbox.
        mail_store::archive_thread_if_inactive(
            conn,
            ingested.thread_id,
            crate::session_store::now_millis(),
        )?;
        recompute_thread(conn, ingested.thread_id, &own)?;
    }
    if ingested.thread_id != removed.thread_id
        && !mail_store::delete_thread_if_empty(conn, removed.thread_id)?
    {
        recompute_thread(conn, removed.thread_id, &own)?;
        // Losing its last active message must not leave it in the inbox.
        mail_store::archive_thread_if_inactive(
            conn,
            removed.thread_id,
            crate::session_store::now_millis(),
        )?;
    }
    Ok(ingested)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mail_mime::{DateSource, ParsedAttachment};
    use crate::mail_store::{
        upsert_account, upsert_credential, upsert_mailboxes, AccountInput, CredentialInput,
        MailboxInput,
    };
    use crate::session_store::SessionStore;

    const DAY: i64 = 86_400;
    const T0: i64 = 1_700_000_000;

    #[test]
    fn subject_key_strips_prefixes_and_tags() {
        for (input, want) in [
            ("Re: hello", "hello"),
            ("RE: hello", "hello"),
            ("Fwd: hello", "hello"),
            ("[list] Re: hello", "hello"),
            ("Re: Re: hello", "hello"),
            ("Re[2]: hello", "hello"),
            ("Trả lời: hello", "hello"),
            ("Chuyển tiếp: hello", "hello"),
            ("  Re:   hello \t world  ", "hello world"),
            ("Re: Re: FW: [sales]  Order #A-1182", "order #a-1182"),
            ("Re: Đơn hàng Hoàng", "đơn hàng hoàng"),
            ("Re: Café", "café"),
            ("hello", "hello"),
            ("", ""),
        ] {
            assert_eq!(subject_key(input), want, "input {input:?}");
        }
    }

    #[test]
    fn subject_key_matches_decomposed_and_fullwidth_forms() {
        let decomposed = "Tra\u{0309} l\u{01a1}\u{0300}i: x";
        assert_eq!(subject_key(decomposed), "x");
        assert_eq!(subject_key("\u{ff32}\u{ff25}\u{ff1a} x"), "x");
        assert_eq!(subject_key("Café"), subject_key("Cafe\u{0301}"));
    }

    #[test]
    fn subject_key_keeps_words_that_only_start_like_a_prefix() {
        assert_eq!(subject_key("Resume: draft"), "resume: draft");
        assert_eq!(subject_key("Re"), "re");
        assert_eq!(subject_key("Re[x]: a"), "re[x]: a");
        assert_eq!(subject_key("[sales]"), "");
    }

    struct Env {
        store: SessionStore,
        owner: i64,
        seller: i64,
        owner_other_domain: i64,
    }

    fn env() -> Env {
        let store = SessionStore::open_in_memory().unwrap();
        let (owner, seller, owner_other_domain) = {
            let conn = store.lock_conn().unwrap();
            let cred = upsert_credential(
                &conn,
                &CredentialInput {
                    provider: "aws".into(),
                    label: "c".into(),
                    access_key_masked: "AKIA\u{2022}\u{2022}\u{2022}\u{2022}3K7Q".into(),
                    default_region: None,
                },
            )
            .unwrap();
            let account = upsert_account(
                &conn,
                &AccountInput {
                    id: None,
                    provider: "aws".into(),
                    credential_id: cred,
                    s3_credential_id: None,
                    label: "src".into(),
                    ses_region: "us-east-1".into(),
                    bucket: "b".into(),
                    s3_region: "us-east-1".into(),
                    default_from: None,
                    attach_policy: "on_demand".into(),
                    image_policy: "ask".into(),
                    enabled: true,
                    sort_order: 0,
                },
            )
            .unwrap();
            let mb = |address: &str, prefix: &str| MailboxInput {
                address: address.into(),
                s3_prefix: prefix.into(),
                display_name: address.into(),
                color: None,
                sort_order: 0,
            };
            upsert_mailboxes(
                &conn,
                account,
                &[
                    mb("owner@hhoangg.com", "owner/"),
                    mb("seller@hhoangg.com", "seller/"),
                    mb("owner@byptah.com", "owner-b/"),
                ],
            )
            .unwrap();
            let id = |addr: &str| mail_store_mailbox_id(&conn, addr);
            (
                id("owner@hhoangg.com"),
                id("seller@hhoangg.com"),
                id("owner@byptah.com"),
            )
        };
        Env {
            store,
            owner,
            seller,
            owner_other_domain,
        }
    }

    fn mail_store_mailbox_id(conn: &Connection, address: &str) -> i64 {
        conn.query_row(
            "SELECT id FROM mail_mailbox WHERE address = ?1",
            [address],
            |r| r.get(0),
        )
        .unwrap()
    }

    fn addr(a: &str) -> MailAddress {
        MailAddress {
            name: None,
            address: a.into(),
        }
    }

    fn mail(id: &str, subject: &str, from: &str, to: &str, date: i64) -> ParsedMail {
        ParsedMail {
            message_id: Some(id.into()),
            in_reply_to: None,
            referenced_ids: vec![],
            subject: subject.into(),
            from: Some(addr(from)),
            reply_to: None,
            to: vec![addr(to)],
            cc: vec![],
            bcc: vec![],
            date_utc: Some(date),
            date_source: DateSource::Header,
            received_at: Some(date),
            snippet: String::new(),
            body_text: Some("body".into()),
            body_html: None,
            remote_images_blocked: 0,
            attachments: vec![],
            size_bytes: 10,
            spf: None,
            dkim: None,
            dmarc: None,
            headers_json: "[]".into(),
        }
    }

    fn reply_to(mut m: ParsedMail, refs: &[&str]) -> ParsedMail {
        m.in_reply_to = refs.last().map(|r| r.to_string());
        m.referenced_ids = refs.iter().map(|r| r.to_string()).collect();
        m
    }

    fn put(env: &Env, mailbox: i64, m: &ParsedMail) -> Ingested {
        let conn = env.store.lock_conn().unwrap();
        ingest_message(&conn, mailbox, "inbound", None, m, T0).unwrap()
    }

    fn count(env: &Env, sql: &str) -> i64 {
        let conn = env.store.lock_conn().unwrap();
        conn.query_row(sql, [], |r| r.get(0)).unwrap()
    }

    const OWNER: &str = "owner@hhoangg.com";

    #[test]
    fn reply_links_by_in_reply_to() {
        let e = env();
        let a = put(&e, e.owner, &mail("a@x", "Plan", "bob@x.com", OWNER, T0));
        let b = put(
            &e,
            e.owner,
            &reply_to(
                mail("b@x", "Totally different", "carol@y.com", OWNER, T0 + 5),
                &["a@x"],
            ),
        );
        assert_eq!(a.thread_id, b.thread_id);
        assert_eq!(count(&e, "SELECT COUNT(*) FROM mail_thread"), 1);
    }

    #[test]
    fn message_links_by_mid_chain_reference() {
        let e = env();
        let a = put(&e, e.owner, &mail("a@x", "Plan", "bob@x.com", OWNER, T0));
        // Cites the root and a middle message that never arrived.
        let c = put(
            &e,
            e.owner,
            &reply_to(
                mail("c@x", "Other", "dan@z.com", OWNER, T0 + 9),
                &["a@x", "gone@x"],
            ),
        );
        assert_eq!(a.thread_id, c.thread_id);
        // A later message citing only the missing middle id still lands there.
        let d = put(
            &e,
            e.owner,
            &reply_to(
                mail("d@x", "Another", "eve@z.com", OWNER, T0 + 20),
                &["gone@x"],
            ),
        );
        assert_eq!(a.thread_id, d.thread_id);
    }

    #[test]
    fn out_of_order_reply_then_parent_is_one_thread() {
        let e = env();
        let reply = put(
            &e,
            e.owner,
            &reply_to(
                mail("b@x", "Re: Plan", "carol@y.com", OWNER, T0 + 5),
                &["a@x"],
            ),
        );
        // Different subject and sender, so only the stored ref can join them.
        let parent = put(
            &e,
            e.owner,
            &mail("a@x", "Unrelated title", "bob@x.com", OWNER, T0),
        );
        assert_eq!(reply.thread_id, parent.thread_id);
        assert_eq!(count(&e, "SELECT COUNT(*) FROM mail_thread"), 1);
        assert_eq!(count(&e, "SELECT message_count FROM mail_thread"), 2);
    }

    #[test]
    fn siblings_of_a_missing_parent_share_a_thread() {
        let e = env();
        let a = put(
            &e,
            e.owner,
            &reply_to(mail("a@x", "One", "bob@x.com", OWNER, T0), &["root@x"]),
        );
        let b = put(
            &e,
            e.owner,
            &reply_to(
                mail("b@x", "Two", "carol@y.com", OWNER, T0 + 1),
                &["root@x"],
            ),
        );
        assert_eq!(a.thread_id, b.thread_id);
    }

    #[test]
    fn two_strangers_saying_hello_do_not_merge() {
        let e = env();
        let a = put(&e, e.owner, &mail("a@x", "hello", "bob@x.com", OWNER, T0));
        let b = put(
            &e,
            e.owner,
            &mail("b@y", "hello", "carol@y.com", OWNER, T0 + 60),
        );
        assert_ne!(a.thread_id, b.thread_id);
    }

    #[test]
    fn same_subject_and_participant_within_window_merges() {
        let e = env();
        let a = put(&e, e.owner, &mail("a@x", "invoice", "bob@x.com", OWNER, T0));
        let b = put(
            &e,
            e.owner,
            &mail("b@x", "Re: invoice", "bob@x.com", OWNER, T0 + 29 * DAY),
        );
        assert_eq!(a.thread_id, b.thread_id);
    }

    #[test]
    fn same_subject_same_participants_107_days_apart_do_not_merge() {
        let e = env();
        let a = put(&e, e.owner, &mail("a@x", "invoice", "bob@x.com", OWNER, T0));
        let b = put(
            &e,
            e.owner,
            &mail("b@x", "invoice", "bob@x.com", OWNER, T0 + 107 * DAY),
        );
        assert_ne!(a.thread_id, b.thread_id);
    }

    #[test]
    fn empty_subject_never_merges() {
        let e = env();
        let a = put(&e, e.owner, &mail("a@x", "", "bob@x.com", OWNER, T0));
        let b = put(&e, e.owner, &mail("b@x", "", "bob@x.com", OWNER, T0 + 1));
        let c = put(&e, e.owner, &mail("c@x", "Re:", "bob@x.com", OWNER, T0 + 2));
        assert_ne!(a.thread_id, b.thread_id);
        assert_ne!(b.thread_id, c.thread_id);
    }

    #[test]
    fn participant_overlap_counts_reply_to_and_cc_but_not_own_mailboxes() {
        let e = env();
        let a = put(&e, e.owner, &mail("a@x", "quote", "bob@x.com", OWNER, T0));
        // Only our own address in common: not enough.
        let b = put(
            &e,
            e.owner,
            &mail("b@x", "quote", "carol@y.com", OWNER, T0 + 1),
        );
        assert_ne!(a.thread_id, b.thread_id);
        // Another of our own mailboxes in Cc is not enough either.
        let mut own_cc = mail("c@x", "quote", "dan@z.com", OWNER, T0 + 2);
        own_cc.cc = vec![addr("seller@hhoangg.com")];
        assert_ne!(put(&e, e.owner, &own_cc).thread_id, a.thread_id);
        // Bob in Cc is enough.
        let mut cc = mail("d@x", "quote", "erin@w.com", OWNER, T0 + 3);
        cc.cc = vec![addr("Bob@X.com")];
        assert_eq!(put(&e, e.owner, &cc).thread_id, a.thread_id);
        // So is a Reply-To of bob.
        let mut rt = mail("e@x", "quote", "list@w.com", OWNER, T0 + 4);
        rt.reply_to = Some(addr("bob@x.com"));
        assert_eq!(put(&e, e.owner, &rt).thread_id, a.thread_id);
    }

    #[test]
    fn gmail_dots_and_plus_tags_are_distinct_participants() {
        let e = env();
        let a = put(
            &e,
            e.owner,
            &mail("a@x", "hi there", "bob@gmail.com", OWNER, T0),
        );
        let b = put(
            &e,
            e.owner,
            &mail("b@x", "hi there", "b.ob@gmail.com", OWNER, T0 + 1),
        );
        let c = put(
            &e,
            e.owner,
            &mail("c@x", "hi there", "bob+x@gmail.com", OWNER, T0 + 2),
        );
        assert_ne!(a.thread_id, b.thread_id);
        assert_ne!(a.thread_id, c.thread_id);
    }

    #[test]
    fn same_chain_in_two_prefixes_stays_two_threads() {
        let e = env();
        let a = put(&e, e.owner, &mail("a@x", "Plan", "bob@x.com", OWNER, T0));
        let b = put(
            &e,
            e.seller,
            &mail("a@x", "Plan", "bob@x.com", "seller@hhoangg.com", T0),
        );
        assert!(!a.duplicate && !b.duplicate);
        assert_ne!(a.thread_id, b.thread_id);
        let r1 = put(
            &e,
            e.owner,
            &reply_to(
                mail("r@x", "Re: Plan", "bob@x.com", OWNER, T0 + 9),
                &["a@x"],
            ),
        );
        let r2 = put(
            &e,
            e.seller,
            &reply_to(
                mail("r@x", "Re: Plan", "bob@x.com", "seller@hhoangg.com", T0 + 9),
                &["a@x"],
            ),
        );
        assert_eq!(r1.thread_id, a.thread_id);
        assert_eq!(r2.thread_id, b.thread_id);
    }

    #[test]
    fn same_chain_at_two_domains_stays_two_threads() {
        let e = env();
        let a = put(&e, e.owner, &mail("a@x", "Plan", "bob@x.com", OWNER, T0));
        let b = put(
            &e,
            e.owner_other_domain,
            &mail("a@x", "Plan", "bob@x.com", "owner@byptah.com", T0),
        );
        assert_ne!(a.thread_id, b.thread_id);
        assert_eq!(count(&e, "SELECT COUNT(*) FROM mail_thread"), 2);
    }

    #[test]
    fn three_way_merge_reparents_everything_and_leaves_no_orphan() {
        let e = env();
        // Three unrelated chains, then one message citing all three roots.
        let t1 = put(&e, e.owner, &mail("r1@x", "One", "a@x.com", OWNER, T0)).thread_id;
        let t2 = put(&e, e.owner, &mail("r2@x", "Two", "b@x.com", OWNER, T0 + 1)).thread_id;
        let t3 = put(
            &e,
            e.owner,
            &mail("r3@x", "Three", "c@x.com", OWNER, T0 + 2),
        )
        .thread_id;
        {
            let conn = e.store.lock_conn().unwrap();
            conn.execute("UPDATE mail_thread SET is_starred = 1 WHERE id = ?1", [t3])
                .unwrap();
            conn.execute(
                "INSERT INTO mail_folder (name, created_at, updated_at) VALUES ('f', 0, 0)",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO mail_folder_member (folder_id, thread_id) VALUES (1, ?1)",
                [t3],
            )
            .unwrap();
        }
        let joiner = put(
            &e,
            e.owner,
            &reply_to(
                mail("j@x", "Joined", "d@x.com", OWNER, T0 + 3),
                &["r1@x", "r2@x", "r3@x"],
            ),
        );
        assert_eq!(joiner.thread_id, t1.min(t2).min(t3));
        assert_eq!(count(&e, "SELECT COUNT(*) FROM mail_thread"), 1);
        assert_eq!(
            count(
                &e,
                "SELECT COUNT(*) FROM mail_message WHERE thread_id <> (SELECT id FROM mail_thread)"
            ),
            0
        );
        assert_eq!(count(&e, "SELECT message_count FROM mail_thread"), 4);
        assert_eq!(count(&e, "SELECT unread_count FROM mail_thread"), 4);
        assert_eq!(count(&e, "SELECT is_starred FROM mail_thread"), 1);
        assert_eq!(count(&e, "SELECT COUNT(*) FROM mail_folder_member WHERE thread_id = (SELECT id FROM mail_thread)"), 1);
        assert_eq!(count(&e, "SELECT first_message_at FROM mail_thread"), T0);
        assert_eq!(count(&e, "SELECT last_message_at FROM mail_thread"), T0 + 3);
    }

    #[test]
    fn merge_survives_both_threads_sharing_a_folder_and_moves_outbox_rows() {
        let e = env();
        let t1 = put(&e, e.owner, &mail("r1@x", "One", "a@x.com", OWNER, T0)).thread_id;
        let t2 = put(&e, e.owner, &mail("r2@x", "Two", "b@x.com", OWNER, T0 + 1)).thread_id;
        let conn = e.store.lock_conn().unwrap();
        conn.execute(
            "INSERT INTO mail_folder (name, created_at, updated_at) VALUES ('f', 0, 0)",
            [],
        )
        .unwrap();
        // Smart folders materialise membership per matching thread, so two
        // threads in one folder is the usual case; a plain UPDATE would hit
        // the (folder_id, thread_id) primary key.
        for t in [t1, t2] {
            conn.execute(
                "INSERT INTO mail_folder_member (folder_id, thread_id) VALUES (1, ?1)",
                [t],
            )
            .unwrap();
        }
        conn.execute(
            "INSERT INTO mail_outbox (account_id, thread_id, kind, from_addr, to_json, created_at, updated_at)
             VALUES ((SELECT account_id FROM mail_thread WHERE id = ?1), ?2, 'reply', ?3, '[]', 0, 0)",
            rusqlite::params![t1, t2, OWNER],
        )
        .unwrap();

        merge_threads(&conn, t1, &[t2], &[]).unwrap();

        let q = |sql: &str| -> i64 { conn.query_row(sql, [], |r| r.get(0)).unwrap() };
        assert_eq!(q("SELECT COUNT(*) FROM mail_folder_member"), 1);
        assert_eq!(q("SELECT thread_id FROM mail_folder_member"), t1);
        assert_eq!(q("SELECT COUNT(*) FROM mail_thread"), 1);
        assert_eq!(q("SELECT thread_id FROM mail_outbox"), t1);
    }

    #[test]
    fn merge_threads_is_idempotent() {
        let e = env();
        let t1 = put(&e, e.owner, &mail("r1@x", "One", "a@x.com", OWNER, T0)).thread_id;
        let t2 = put(&e, e.owner, &mail("r2@x", "Two", "b@x.com", OWNER, T0 + 1)).thread_id;
        let conn = e.store.lock_conn().unwrap();
        merge_threads(&conn, t1, &[t2], &[]).unwrap();
        merge_threads(&conn, t1, &[t2], &[]).unwrap();
        let n: i64 = conn
            .query_row("SELECT message_count FROM mail_thread", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 2);
    }

    #[test]
    fn re_ingesting_a_message_is_a_no_op() {
        let e = env();
        let m = mail("a@x", "Plan", "bob@x.com", OWNER, T0);
        let first = put(&e, e.owner, &m);
        let again = put(&e, e.owner, &m);
        assert!(again.duplicate);
        assert_eq!(
            (again.message_id, again.thread_id),
            (first.message_id, first.thread_id)
        );
        assert_eq!(count(&e, "SELECT COUNT(*) FROM mail_message"), 1);
    }

    #[test]
    fn aggregates_and_refs_are_stored() {
        let e = env();
        let mut m = reply_to(
            mail("b@x", "Re: Plan", "bob@x.com", OWNER, T0),
            &["a@x", "b@x", "a@x"],
        );
        m.attachments = vec![ParsedAttachment {
            part_index: 1,
            filename: "f.pdf".into(),
            mime_type: "application/pdf".into(),
            size_bytes: 5,
            content_id: None,
            is_inline: false,
        }];
        put(&e, e.owner, &m);
        // The parent never arrived, but its id is recorded; own id and dupes are not.
        assert_eq!(
            count(
                &e,
                "SELECT COUNT(*) FROM mail_message_ref WHERE ref_hdr = 'a@x'"
            ),
            1
        );
        assert_eq!(count(&e, "SELECT COUNT(*) FROM mail_message_ref"), 1);
        assert_eq!(count(&e, "SELECT has_attachments FROM mail_thread"), 1);
        assert_eq!(count(&e, "SELECT COUNT(*) FROM mail_attachment"), 1);
        let (subject, key, people): (String, String, String) = {
            let conn = e.store.lock_conn().unwrap();
            conn.query_row(
                "SELECT subject, subject_key, participants_json FROM mail_thread",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap()
        };
        assert_eq!((subject.as_str(), key.as_str()), ("Re: Plan", "plan"));
        assert_eq!(people, r#"[{"name":null,"addr":"bob@x.com"}]"#);
    }
}
