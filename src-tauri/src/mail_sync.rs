//! The reconcile loop: list each mailbox prefix, diff it against `mail_object`,
//! fetch what is new or changed, mark what vanished (spec section 5).
// The command layer that starts this arrives in a later task.
#![allow(dead_code)]

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use mail_parser::DateTime;
use rusqlite::Connection;
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

use crate::aws_s3::{ObjectInfo, S3Api, S3ErrorKind};
use crate::mail_mime::parse_mail;
use crate::mail_store::{
    self, ListedObject, MailAccount, MailMailbox, NewAttachment, RefreshedBody,
};
use crate::mail_thread;
use crate::session_store::{now_millis, SessionStore};

pub(crate) const SYNC_STATE_EVENT: &str = "mail://sync-state";
pub(crate) const MESSAGES_CHANGED_EVENT: &str = "mail://messages-changed";

const DEFAULT_INTERVAL_SECS: u64 = 3600;
/// Stops a bad setting from turning the loop into a hot spin.
const MIN_INTERVAL_SECS: u64 = 60;
const TICK: Duration = Duration::from_secs(60);
const MAX_PARSE_ATTEMPTS: i64 = 3;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SyncReport {
    pub account_id: i64,
    pub inserted: usize,
    pub archived: usize,
    pub reparsed: usize,
    pub failed: usize,
    pub error: Option<String>,
}

/// `mail://sync-state` payload. It carries the source id so the UI updates one
/// domain group instead of re-rendering every mailbox.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SyncStateEvent {
    pub account_id: i64,
    pub mailbox_id: Option<i64>,
    /// `running | done | error`
    pub phase: &'static str,
    pub done: usize,
    pub total: usize,
    pub error: Option<String>,
}

/// Where events go. Tests record them; the app emits them to the webview.
pub(crate) trait SyncEvents: Send + Sync {
    fn sync_state(&self, event: &SyncStateEvent);
    fn messages_changed(&self, account_id: i64);
}

pub(crate) struct TauriEvents(pub AppHandle);

impl SyncEvents for TauriEvents {
    fn sync_state(&self, event: &SyncStateEvent) {
        let _ = self.0.emit(SYNC_STATE_EVENT, event);
    }

    fn messages_changed(&self, account_id: i64) {
        let _ = self.0.emit(MESSAGES_CHANGED_EVENT, account_id);
    }
}

/// Builds the S3 client for a source. Keychain lookup lives behind this, so the
/// loop never sees a secret and tests never need one.
pub(crate) trait ClientFactory: Send + Sync {
    fn client(&self, account: &MailAccount) -> Result<Box<dyn S3Api + Send>, String>;
}

pub(crate) struct SyncService {
    locks: Mutex<HashMap<i64, Arc<Mutex<()>>>>,
    interval_secs: AtomicU64,
}

enum Scope {
    Now(Option<i64>),
    Due { now_ms: i64 },
}

struct Job {
    account: MailAccount,
    mailboxes: Vec<MailMailbox>,
    staleness: i64,
}

#[derive(Default)]
struct Counts {
    seen: usize,
    inserted: usize,
    archived: usize,
    reparsed: usize,
    revived: usize,
    failed: usize,
}

struct Work {
    object_id: i64,
    key: String,
    s3_modified: Option<i64>,
    /// The body changed under an existing message.
    reparse: bool,
    attempts: i64,
}

impl Default for SyncService {
    fn default() -> Self {
        Self {
            locks: Mutex::new(HashMap::new()),
            interval_secs: AtomicU64::new(DEFAULT_INTERVAL_SECS),
        }
    }
}

impl SyncService {
    /// The webview owns the setting (`monocode.mail.syncIntervalSeconds`); the
    /// command layer pushes it here.
    pub(crate) fn set_interval_secs(&self, secs: u64) {
        self.interval_secs
            .store(secs.max(MIN_INTERVAL_SECS), Ordering::Relaxed);
    }

    pub(crate) fn interval_secs(&self) -> u64 {
        self.interval_secs.load(Ordering::Relaxed)
    }

    /// On-demand sync of one source, or of all of them.
    pub(crate) fn sync_now(
        &self,
        store: &SessionStore,
        factory: &dyn ClientFactory,
        events: &dyn SyncEvents,
        account_id: Option<i64>,
    ) -> Vec<SyncReport> {
        self.run(store, factory, events, Scope::Now(account_id))
    }

    /// Syncs the mailboxes whose last finish is older than the interval.
    pub(crate) fn sync_due(
        &self,
        store: &SessionStore,
        factory: &dyn ClientFactory,
        events: &dyn SyncEvents,
        now_ms: i64,
    ) -> Vec<SyncReport> {
        self.run(store, factory, events, Scope::Due { now_ms })
    }

    fn lock_for(&self, credential_id: i64) -> Arc<Mutex<()>> {
        let mut locks = self.locks.lock().unwrap_or_else(|e| e.into_inner());
        locks.entry(credential_id).or_default().clone()
    }

    fn run(
        &self,
        store: &SessionStore,
        factory: &dyn ClientFactory,
        events: &dyn SyncEvents,
        scope: Scope,
    ) -> Vec<SyncReport> {
        let Ok(mut jobs) = self.plan(store, &scope) else {
            return Vec::new();
        };
        // Oldest first, so one large bucket cannot starve the rest.
        jobs.sort_by_key(|j| (j.staleness, j.account.id));
        // The S3 key is what is contended, so it picks the lock.
        let mut groups: BTreeMap<i64, Vec<Job>> = BTreeMap::new();
        for job in jobs {
            let credential = job
                .account
                .s3_credential_id
                .unwrap_or(job.account.credential_id);
            groups.entry(credential).or_default().push(job);
        }
        std::thread::scope(|scope| {
            let handles: Vec<_> = groups
                .into_iter()
                .map(|(credential, jobs)| {
                    let ids: Vec<i64> = jobs.iter().map(|j| j.account.id).collect();
                    let lock = self.lock_for(credential);
                    let handle = scope.spawn(move || {
                        let _held = lock.lock().unwrap_or_else(|e| e.into_inner());
                        jobs.iter()
                            .map(|j| sync_account(store, factory, events, j))
                            .collect::<Vec<_>>()
                    });
                    (ids, handle)
                })
                .collect();
            handles
                .into_iter()
                .flat_map(|(ids, handle)| {
                    handle.join().unwrap_or_else(|_| {
                        ids.into_iter()
                            .map(|account_id| SyncReport {
                                account_id,
                                error: Some("The sync stopped unexpectedly.".into()),
                                ..Default::default()
                            })
                            .collect()
                    })
                })
                .collect()
        })
    }

    fn plan(&self, store: &SessionStore, scope: &Scope) -> Result<Vec<Job>, String> {
        let conn = store.lock_conn()?;
        let interval_ms = self.interval_secs() as i64 * 1000;
        let mut jobs = Vec::new();
        for account in mail_store::list_accounts(&conn).map_err(|e| e.to_string())? {
            if !account.enabled {
                continue;
            }
            if let Scope::Now(Some(id)) = scope {
                if *id != account.id {
                    continue;
                }
            }
            let mut mailboxes = Vec::new();
            let mut staleness = i64::MAX;
            for mailbox in
                mail_store::list_mailboxes(&conn, Some(account.id)).map_err(|e| e.to_string())?
            {
                let finished = mail_store::get_sync_state(&conn, mailbox.id)
                    .map_err(|e| e.to_string())?
                    .and_then(|s| s.last_finished_at);
                let due = match scope {
                    Scope::Now(_) => true,
                    Scope::Due { now_ms } => finished.is_none_or(|t| now_ms - t >= interval_ms),
                };
                if due {
                    staleness = staleness.min(finished.unwrap_or(0));
                    mailboxes.push(mailbox);
                }
            }
            if !mailboxes.is_empty() {
                jobs.push(Job {
                    account,
                    mailboxes,
                    staleness,
                });
            }
        }
        Ok(jobs)
    }
}

/// One thread for the life of the app. `sync_now` calls from commands share
/// `service`, so they queue behind a running scheduled sync per credential.
pub(crate) fn spawn_scheduler(
    app: AppHandle,
    service: Arc<SyncService>,
    factory: Arc<dyn ClientFactory>,
) {
    std::thread::spawn(move || loop {
        std::thread::sleep(TICK);
        let store = app.state::<SessionStore>();
        service.sync_due(&store, &*factory, &TauriEvents(app.clone()), now_millis());
    });
}

fn sync_account(
    store: &SessionStore,
    factory: &dyn ClientFactory,
    events: &dyn SyncEvents,
    job: &Job,
) -> SyncReport {
    let account_id = job.account.id;
    let mut report = SyncReport {
        account_id,
        ..Default::default()
    };
    let mut changed = false;
    match factory.client(&job.account) {
        Err(error) => {
            for mailbox in &job.mailboxes {
                record_state(
                    store,
                    mailbox.id,
                    now_millis(),
                    &Counts::default(),
                    Some(&error),
                );
            }
            report.error = Some(error);
        }
        Ok(s3) => {
            for mailbox in &job.mailboxes {
                let started = now_millis();
                let mut counts = Counts::default();
                let result = sync_mailbox(store, &*s3, mailbox, events, &mut counts);
                record_state(store, mailbox.id, started, &counts, result.as_ref().err());
                report.inserted += counts.inserted;
                report.archived += counts.archived;
                report.reparsed += counts.reparsed;
                report.failed += counts.failed;
                changed |= counts.inserted + counts.archived + counts.reparsed + counts.revived > 0;
                if let Err(error) = result {
                    report.error.get_or_insert(error);
                }
            }
        }
    }
    events.sync_state(&SyncStateEvent {
        account_id,
        mailbox_id: None,
        phase: if report.error.is_some() {
            "error"
        } else {
            "done"
        },
        done: 0,
        total: 0,
        error: report.error.clone(),
    });
    if changed {
        events.messages_changed(account_id);
    }
    report
}

fn record_state(
    store: &SessionStore,
    mailbox_id: i64,
    started: i64,
    counts: &Counts,
    error: Option<&String>,
) {
    let Ok(conn) = store.lock_conn() else {
        return;
    };
    let _ = mail_store::set_sync_state(
        &conn,
        &mail_store::MailSyncState {
            mailbox_id,
            last_started_at: Some(started),
            last_finished_at: Some(now_millis()),
            last_error: error.cloned(),
            objects_seen: counts.seen as i64,
            objects_added: counts.inserted as i64,
            objects_gone: counts.archived as i64,
        },
    );
}

/// An error that would repeat for every other key, so the mailbox stops.
fn is_fatal(kind: S3ErrorKind) -> bool {
    !matches!(kind, S3ErrorKind::NoSuchKey | S3ErrorKind::Other)
}

fn list_all(s3: &dyn S3Api, prefix: &str) -> Result<Vec<ObjectInfo>, String> {
    let mut all = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let page = s3
            .list_objects_v2(prefix, None, token.as_deref())
            .map_err(|e| e.message)?;
        all.extend(page.objects);
        match page.next_token {
            Some(next) => token = Some(next),
            None => return Ok(all),
        }
    }
}

fn sync_mailbox(
    store: &SessionStore,
    s3: &dyn S3Api,
    mailbox: &MailMailbox,
    events: &dyn SyncEvents,
    counts: &mut Counts,
) -> Result<(), String> {
    let account_id = mailbox.account_id;
    let emit = |phase, done, total, error: Option<String>| {
        events.sync_state(&SyncStateEvent {
            account_id,
            mailbox_id: Some(mailbox.id),
            phase,
            done,
            total,
            error,
        });
    };
    emit("running", 0, 0, None);
    let sync_start = now_millis();
    // A failed listing returns here, before anything is marked gone: a partial
    // list must never read as "everything vanished".
    let listed: Vec<ObjectInfo> = list_all(s3, &mailbox.s3_prefix)?
        .into_iter()
        .filter(|o| !o.key.ends_with('/'))
        .collect();
    counts.seen = listed.len();

    let db = |e: rusqlite::Error| e.to_string();
    let (work, revived) = {
        let conn = store.lock_conn()?;
        reconcile_ledger(&conn, mailbox.id, &listed, sync_start).map_err(db)?
    };
    counts.revived = revived;
    counts.archived = {
        let conn = store.lock_conn()?;
        mail_store::mark_vanished_objects(&conn, mailbox.id, sync_start, now_millis())
            .map_err(db)?
    };

    let total = work.len();
    for (index, item) in work.iter().enumerate() {
        emit("running", index, total, None);
        let raw = match s3.get_object(&item.key) {
            Ok(raw) => raw,
            Err(e) if is_fatal(e.kind) => return Err(e.message),
            Err(_) => {
                counts.failed += 1;
                continue;
            }
        };
        let conn = store.lock_conn()?;
        ingest_one(&conn, mailbox, item, &raw, counts).map_err(db)?;
    }
    emit("done", total, total, None);
    Ok(())
}

/// Brings `mail_object` in line with the listing and returns what to fetch,
/// plus how many vanished messages came back.
fn reconcile_ledger(
    conn: &Connection,
    mailbox_id: i64,
    listed: &[ObjectInfo],
    sync_start: i64,
) -> rusqlite::Result<(Vec<Work>, usize)> {
    let known: HashMap<String, mail_store::MailObjectRow> =
        mail_store::list_objects(conn, mailbox_id)?
            .into_iter()
            .map(|r| (r.s3_key.clone(), r))
            .collect();
    let mut work = Vec::new();
    let mut revived = 0;
    for o in listed {
        let s3_modified = DateTime::parse_rfc3339(&o.last_modified).map(|d| d.to_timestamp());
        let entry = ListedObject {
            s3_key: &o.key,
            etag: &o.etag,
            size_bytes: o.size as i64,
            s3_modified,
        };
        let job = |object_id, reparse, attempts| Work {
            object_id,
            key: o.key.clone(),
            s3_modified,
            reparse,
            attempts,
        };
        let Some(row) = known.get(&o.key) else {
            let id = mail_store::insert_object(conn, mailbox_id, &entry, sync_start)?;
            work.push(job(id, false, 0));
            continue;
        };
        let changed = row.etag.as_deref() != Some(o.etag.as_str());
        let has_message = mail_store::message_for_object(conn, row.id)?.is_some();
        match row.state.as_str() {
            "gone" => {
                // The user's `archived` is never touched here, only `gone`.
                mail_store::revive_gone_message(conn, row.id)?;
                revived += 1;
                let state = if has_message { "parsed" } else { "pending" };
                mail_store::touch_object(conn, row.id, &entry, sync_start, state)?;
                if changed || !has_message {
                    work.push(job(row.id, has_message, row.parse_attempts));
                }
            }
            "parsed" => {
                mail_store::touch_object(conn, row.id, &entry, sync_start, "parsed")?;
                if changed {
                    work.push(job(row.id, has_message, row.parse_attempts));
                }
            }
            "error" => {
                mail_store::touch_object(conn, row.id, &entry, sync_start, "error")?;
                if changed {
                    mail_store::set_object_outcome(conn, row.id, "pending", None, 0)?;
                    work.push(job(row.id, has_message, 0));
                } else if row.parse_attempts < MAX_PARSE_ATTEMPTS {
                    work.push(job(row.id, has_message, row.parse_attempts));
                }
            }
            _ => {
                mail_store::touch_object(conn, row.id, &entry, sync_start, "pending")?;
                work.push(job(row.id, has_message, row.parse_attempts));
            }
        }
    }
    Ok((work, revived))
}

fn ingest_one(
    conn: &Connection,
    mailbox: &MailMailbox,
    item: &Work,
    raw: &[u8],
    counts: &mut Counts,
) -> rusqlite::Result<()> {
    let mail = match parse_mail(raw) {
        Ok(mail) => mail,
        Err(message) => {
            let attempts = item.attempts + 1;
            mail_store::set_object_outcome(
                conn,
                item.object_id,
                "error",
                Some(&message),
                attempts,
            )?;
            counts.failed += 1;
            return Ok(());
        }
    };
    let existing = mail_store::message_for_object(conn, item.object_id)?;
    match existing {
        Some((message_id, thread_id)) if item.reparse => {
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
            mail_store::refresh_message_body(
                conn,
                message_id,
                &RefreshedBody {
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
                &attachments,
            )?;
            // No losers: this only refreshes the thread's aggregates.
            let own = mail_store::account_mailbox_addresses(conn, mailbox.account_id)?;
            mail_thread::merge_threads(conn, thread_id, &[], &own)?;
            counts.reparsed += 1;
        }
        // Crash between ingest and the state update: the message is there.
        Some(_) => {}
        None => {
            // Last resort when the message carries neither `Date` nor
            // `Received`: the object's own time, else now.
            let fallback = item.s3_modified.unwrap_or_else(|| now_millis() / 1000);
            let ingested = mail_thread::ingest_message(
                conn,
                mailbox.id,
                "inbound",
                Some(item.object_id),
                &mail,
                fallback,
            )?;
            if !ingested.duplicate {
                // A new message surfaces its thread again. This is not the
                // re-listed-key case, which never un-archives (spec section 10).
                mail_store::reactivate_thread(conn, ingested.thread_id)?;
                counts.inserted += 1;
            }
        }
    }
    mail_store::set_object_outcome(conn, item.object_id, "parsed", None, 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::aws_s3::{DeleteOutcome, ObjectPage, S3Error};
    use crate::mail_store::{
        upsert_account, upsert_credential, upsert_mailboxes, AccountInput, CredentialInput,
        MailboxInput,
    };
    use std::sync::Condvar;

    type Log = Arc<Mutex<Vec<String>>>;

    struct Rendezvous {
        want: usize,
        entered: Mutex<usize>,
        cv: Condvar,
        met: Mutex<bool>,
    }

    impl Rendezvous {
        fn new(want: usize) -> Arc<Self> {
            Arc::new(Self {
                want,
                entered: Mutex::new(0),
                cv: Condvar::new(),
                met: Mutex::new(false),
            })
        }

        /// Blocks until `want` callers are inside, or gives up after 2 s. `met` is
        /// set only by a caller that was woken, never by one arriving after a timeout.
        fn arrive(&self) {
            let mut n = self.entered.lock().unwrap();
            *n += 1;
            if *n >= self.want {
                self.cv.notify_all();
                return;
            }
            let (_n, timeout) = self
                .cv
                .wait_timeout_while(n, Duration::from_secs(2), |n| *n < self.want)
                .unwrap();
            if !timeout.timed_out() {
                *self.met.lock().unwrap() = true;
            }
        }

        fn met(&self) -> bool {
            *self.met.lock().unwrap()
        }
    }

    struct MockS3 {
        label: String,
        log: Log,
        objects: Mutex<BTreeMap<String, (Vec<u8>, String)>>,
        list_error: Mutex<Option<S3Error>>,
        page_size: usize,
        rendezvous: Option<Arc<Rendezvous>>,
    }

    impl MockS3 {
        fn new(label: &str, log: &Log) -> Arc<Self> {
            Arc::new(Self {
                label: label.into(),
                log: log.clone(),
                objects: Mutex::new(BTreeMap::new()),
                list_error: Mutex::new(None),
                page_size: 1000,
                rendezvous: None,
            })
        }

        fn put(&self, key: &str, etag: &str, raw: &str) {
            self.objects
                .lock()
                .unwrap()
                .insert(key.into(), (raw.as_bytes().to_vec(), etag.into()));
        }

        fn remove(&self, key: &str) {
            self.objects.lock().unwrap().remove(key);
        }
    }

    struct MockClient(Arc<MockS3>);

    impl S3Api for MockClient {
        fn list_objects_v2(
            &self,
            prefix: &str,
            _delimiter: Option<&str>,
            token: Option<&str>,
        ) -> Result<ObjectPage, S3Error> {
            let m = &self.0;
            m.log
                .lock()
                .unwrap()
                .push(format!("{}:list:{prefix}", m.label));
            if let Some(r) = &m.rendezvous {
                r.arrive();
            }
            if let Some(e) = m.list_error.lock().unwrap().clone() {
                return Err(e);
            }
            let all: Vec<ObjectInfo> = m
                .objects
                .lock()
                .unwrap()
                .iter()
                .filter(|(k, _)| k.starts_with(prefix))
                .map(|(k, (body, etag))| ObjectInfo {
                    key: k.clone(),
                    size: body.len() as u64,
                    etag: etag.clone(),
                    last_modified: "2024-03-01T10:00:00.000Z".into(),
                })
                .collect();
            let start: usize = token.map(|t| t.parse().unwrap()).unwrap_or(0);
            let end = (start + m.page_size).min(all.len());
            Ok(ObjectPage {
                objects: all[start..end].to_vec(),
                common_prefixes: vec![],
                next_token: (end < all.len()).then(|| end.to_string()),
            })
        }

        fn get_object(&self, key: &str) -> Result<Vec<u8>, S3Error> {
            let m = &self.0;
            m.log.lock().unwrap().push(format!("{}:get:{key}", m.label));
            m.objects
                .lock()
                .unwrap()
                .get(key)
                .map(|(b, _)| b.clone())
                .ok_or(S3Error {
                    kind: S3ErrorKind::NoSuchKey,
                    code: None,
                    message: "no such key".into(),
                })
        }

        fn put_object(&self, _: &str, _: &[u8], _: &str) -> Result<(), S3Error> {
            unreachable!("sync never writes")
        }

        fn delete_objects(&self, _: &[String]) -> Vec<DeleteOutcome> {
            unreachable!("sync never deletes")
        }
    }

    struct MockFactory(HashMap<i64, Arc<MockS3>>);

    impl ClientFactory for MockFactory {
        fn client(&self, account: &MailAccount) -> Result<Box<dyn S3Api + Send>, String> {
            self.0
                .get(&account.id)
                .map(|m| Box::new(MockClient(m.clone())) as Box<dyn S3Api + Send>)
                .ok_or_else(|| "no client".to_string())
        }
    }

    #[derive(Default)]
    struct Recorder(Mutex<Vec<String>>);

    impl SyncEvents for Recorder {
        fn sync_state(&self, e: &SyncStateEvent) {
            self.0.lock().unwrap().push(format!(
                "state:{}:{}:{:?}",
                e.account_id,
                e.phase,
                e.mailbox_id.is_some()
            ));
        }

        fn messages_changed(&self, account_id: i64) {
            self.0.lock().unwrap().push(format!("changed:{account_id}"));
        }
    }

    fn mime(id: &str, subject: &str, body: &str) -> String {
        format!(
            "Message-ID: <{id}@example.com>\r\nFrom: Bob <bob@example.com>\r\n\
             To: owner@acme.test\r\nSubject: {subject}\r\n\
             Date: Fri, 01 Mar 2024 09:00:00 +0000\r\n\
             Content-Type: text/plain; charset=utf-8\r\n\r\n{body}\r\n"
        )
    }

    struct Fixture {
        store: SessionStore,
        service: SyncService,
        log: Log,
    }

    impl Fixture {
        fn new() -> Self {
            Self {
                store: SessionStore::open_in_memory().unwrap(),
                service: SyncService::default(),
                log: Log::default(),
            }
        }

        fn credential(&self, label: &str) -> i64 {
            let conn = self.store.lock_conn().unwrap();
            upsert_credential(
                &conn,
                &CredentialInput {
                    provider: "aws".into(),
                    label: label.into(),
                    access_key_masked: "AKIA\u{2022}\u{2022}\u{2022}\u{2022}3K7Q".into(),
                    default_region: None,
                },
            )
            .unwrap()
        }

        /// Returns `(account id, mailbox id)`.
        fn source(&self, credential: i64, name: &str) -> (i64, i64) {
            let conn = self.store.lock_conn().unwrap();
            let account = upsert_account(
                &conn,
                &AccountInput {
                    id: None,
                    provider: "aws".into(),
                    credential_id: credential,
                    s3_credential_id: None,
                    label: name.into(),
                    ses_region: "us-east-1".into(),
                    bucket: format!("bucket-{name}"),
                    s3_region: "us-east-1".into(),
                    default_from: None,
                    attach_policy: "on_demand".into(),
                    image_policy: "ask".into(),
                    enabled: true,
                    sort_order: 0,
                },
            )
            .unwrap();
            upsert_mailboxes(
                &conn,
                account,
                &[MailboxInput {
                    address: format!("owner@{name}.test"),
                    s3_prefix: "owner/".into(),
                    display_name: "Owner".into(),
                    color: None,
                    sort_order: 0,
                }],
            )
            .unwrap();
            let mailbox = mail_store::list_mailboxes(&conn, Some(account)).unwrap()[0].id;
            (account, mailbox)
        }

        fn sync(&self, mocks: &[(i64, &Arc<MockS3>)]) -> Vec<SyncReport> {
            let factory = MockFactory(mocks.iter().map(|(i, m)| (*i, (*m).clone())).collect());
            let mut reports =
                self.service
                    .sync_now(&self.store, &factory, &Recorder::default(), None);
            reports.sort_by_key(|r| r.account_id);
            reports
        }

        fn scalar(&self, sql: &str) -> i64 {
            let conn = self.store.lock_conn().unwrap();
            conn.query_row(sql, [], |r| r.get(0)).unwrap()
        }

        fn text(&self, sql: &str) -> String {
            let conn = self.store.lock_conn().unwrap();
            conn.query_row(sql, [], |r| r.get(0)).unwrap()
        }
    }

    fn one_source(f: &Fixture) -> (i64, i64, Arc<MockS3>) {
        let cred = f.credential("main");
        let (account, mailbox) = f.source(cred, "acme");
        (account, mailbox, MockS3::new("a", &f.log))
    }

    #[test]
    fn new_key_is_inserted_and_threaded() {
        let f = Fixture::new();
        let (account, _, s3) = one_source(&f);
        s3.put("owner/1", "e1", &mime("m1", "Hello", "first"));
        s3.put("owner/2", "e2", &mime("m2", "Other", "second"));
        let reports = f.sync(&[(account, &s3)]);
        assert_eq!(reports[0].inserted, 2);
        assert_eq!(reports[0].error, None);
        assert_eq!(f.scalar("SELECT COUNT(*) FROM mail_message"), 2);
        assert_eq!(f.scalar("SELECT COUNT(*) FROM mail_thread"), 2);
        assert_eq!(
            f.scalar("SELECT COUNT(*) FROM mail_object WHERE state='parsed'"),
            2
        );
        let again = f.sync(&[(account, &s3)]);
        assert_eq!((again[0].inserted, again[0].reparsed), (0, 0));
        assert_eq!(f.scalar("SELECT COUNT(*) FROM mail_message"), 2);
    }

    #[test]
    fn listing_follows_continuation_tokens() {
        let f = Fixture::new();
        let (account, _, _) = one_source(&f);
        let s3 = Arc::new(MockS3 {
            page_size: 2,
            ..Arc::try_unwrap(MockS3::new("a", &f.log)).ok().unwrap()
        });
        for i in 0..5 {
            s3.put(
                &format!("owner/{i}"),
                "e",
                &mime(&format!("m{i}"), "S", "b"),
            );
        }
        let reports = f.sync(&[(account, &s3)]);
        assert_eq!(reports[0].inserted, 5);
    }

    #[test]
    fn vanished_key_is_marked_not_deleted() {
        let f = Fixture::new();
        let (account, mailbox, s3) = one_source(&f);
        s3.put("owner/1", "e1", &mime("m1", "Hello", "first"));
        f.sync(&[(account, &s3)]);
        s3.remove("owner/1");
        let reports = f.sync(&[(account, &s3)]);
        assert_eq!(reports[0].archived, 1);
        assert_eq!(f.scalar("SELECT COUNT(*) FROM mail_message"), 1);
        assert_eq!(f.text("SELECT state FROM mail_object"), "gone");
        assert_eq!(f.text("SELECT state FROM mail_message"), "gone");
        assert_eq!(f.text("SELECT state FROM mail_thread"), "archived");
        let conn = f.store.lock_conn().unwrap();
        let state = mail_store::get_sync_state(&conn, mailbox).unwrap().unwrap();
        assert_eq!((state.objects_seen, state.objects_gone), (0, 1));
    }

    #[test]
    fn failed_listing_archives_nothing() {
        let f = Fixture::new();
        let (account, _, s3) = one_source(&f);
        s3.put("owner/1", "e1", &mime("m1", "Hello", "first"));
        f.sync(&[(account, &s3)]);
        *s3.list_error.lock().unwrap() = Some(S3Error {
            kind: S3ErrorKind::Network,
            code: None,
            message: "offline".into(),
        });
        let reports = f.sync(&[(account, &s3)]);
        assert_eq!(reports[0].error.as_deref(), Some("offline"));
        assert_eq!(f.text("SELECT state FROM mail_object"), "parsed");
        assert_eq!(f.text("SELECT state FROM mail_message"), "active");
    }

    #[test]
    fn relisted_key_does_not_unarchive_user_archive() {
        let f = Fixture::new();
        let (account, _, s3) = one_source(&f);
        s3.put("owner/1", "e1", &mime("m1", "Hello", "first"));
        f.sync(&[(account, &s3)]);
        // What Task 6's archive command does, spec section 10.
        {
            let conn = f.store.lock_conn().unwrap();
            conn.execute("UPDATE mail_message SET state='archived'", [])
                .unwrap();
            conn.execute("UPDATE mail_thread SET state='archived', archived_at=1", [])
                .unwrap();
        }
        f.sync(&[(account, &s3)]);
        assert_eq!(f.text("SELECT state FROM mail_message"), "archived");
        assert_eq!(f.text("SELECT state FROM mail_thread"), "archived");
        // The key vanishes and returns: still the user's archive.
        s3.remove("owner/1");
        f.sync(&[(account, &s3)]);
        assert_eq!(f.text("SELECT state FROM mail_message"), "archived");
        s3.put("owner/1", "e1", &mime("m1", "Hello", "first"));
        f.sync(&[(account, &s3)]);
        assert_eq!(f.text("SELECT state FROM mail_message"), "archived");
        assert_eq!(f.text("SELECT state FROM mail_thread"), "archived");
        assert_eq!(f.text("SELECT state FROM mail_object"), "parsed");
    }

    #[test]
    fn vanished_then_relisted_key_comes_back() {
        let f = Fixture::new();
        let (account, _, s3) = one_source(&f);
        s3.put("owner/1", "e1", &mime("m1", "Hello", "first"));
        f.sync(&[(account, &s3)]);
        s3.remove("owner/1");
        f.sync(&[(account, &s3)]);
        s3.put("owner/1", "e1", &mime("m1", "Hello", "first"));
        let reports = f.sync(&[(account, &s3)]);
        assert_eq!(reports[0].inserted, 0);
        assert_eq!(f.text("SELECT state FROM mail_message"), "active");
        assert_eq!(f.text("SELECT state FROM mail_thread"), "active");
        assert_eq!(f.scalar("SELECT COUNT(*) FROM mail_message"), 1);
    }

    #[test]
    fn new_message_surfaces_archived_thread() {
        let f = Fixture::new();
        let (account, _, s3) = one_source(&f);
        s3.put("owner/1", "e1", &mime("m1", "Hello", "first"));
        f.sync(&[(account, &s3)]);
        {
            let conn = f.store.lock_conn().unwrap();
            conn.execute("UPDATE mail_message SET state='archived'", [])
                .unwrap();
            conn.execute("UPDATE mail_thread SET state='archived', archived_at=1", [])
                .unwrap();
        }
        let reply = mime("m2", "Re: Hello", "reply").replace(
            "Subject:",
            "In-Reply-To: <m1@example.com>\r\nReferences: <m1@example.com>\r\nSubject:",
        );
        s3.put("owner/2", "e2", &reply);
        f.sync(&[(account, &s3)]);
        assert_eq!(f.scalar("SELECT COUNT(*) FROM mail_thread"), 1);
        assert_eq!(f.text("SELECT state FROM mail_thread"), "active");
        assert_eq!(
            f.text("SELECT state FROM mail_message WHERE message_id_hdr='m1@example.com'"),
            "archived"
        );
    }

    #[test]
    fn etag_change_reparses_and_keeps_read_state() {
        let f = Fixture::new();
        let (account, _, s3) = one_source(&f);
        s3.put("owner/1", "e1", &mime("m1", "Hello", "old body"));
        f.sync(&[(account, &s3)]);
        f.store
            .lock_conn()
            .unwrap()
            .execute("UPDATE mail_message SET is_read=1", [])
            .unwrap();
        s3.put("owner/1", "e2", &mime("m1", "Hello", "new body"));
        let reports = f.sync(&[(account, &s3)]);
        assert_eq!((reports[0].reparsed, reports[0].inserted), (1, 0));
        assert!(f
            .text("SELECT body_text FROM mail_message")
            .contains("new body"));
        assert_eq!(f.scalar("SELECT is_read FROM mail_message"), 1);
        assert_eq!(f.scalar("SELECT COUNT(*) FROM mail_message"), 1);
        assert_eq!(f.text("SELECT etag FROM mail_object"), "e2");
    }

    #[test]
    fn unreadable_object_retries_three_times_then_rests() {
        let f = Fixture::new();
        let (account, _, s3) = one_source(&f);
        s3.put("owner/1", "e1", "not an email at all");
        for _ in 0..3 {
            assert_eq!(f.sync(&[(account, &s3)])[0].failed, 1);
        }
        assert_eq!(f.text("SELECT state FROM mail_object"), "error");
        let gets = |f: &Fixture| {
            f.log
                .lock()
                .unwrap()
                .iter()
                .filter(|l| l.contains(":get:"))
                .count()
        };
        assert_eq!(gets(&f), 3);
        assert_eq!(f.sync(&[(account, &s3)])[0].failed, 0);
        assert_eq!(gets(&f), 3);
        // A changed object starts over.
        s3.put("owner/1", "e2", &mime("m1", "Fixed", "ok"));
        assert_eq!(f.sync(&[(account, &s3)])[0].inserted, 1);
    }

    #[test]
    fn message_without_any_date_sorts_by_object_time() {
        let f = Fixture::new();
        let (account, _, s3) = one_source(&f);
        let raw = "Message-ID: <nodate@example.com>\r\nFrom: a@example.com\r\n\
                   To: owner@acme.test\r\nSubject: x\r\n\r\nbody\r\n";
        s3.put("owner/1", "e1", raw);
        f.sync(&[(account, &s3)]);
        // 2024-03-01T10:00:00Z, from the mock's last-modified.
        assert_eq!(f.scalar("SELECT date_utc FROM mail_message"), 1_709_287_200);
        assert_eq!(
            f.scalar("SELECT received_at FROM mail_message"),
            1_709_287_200
        );
    }

    #[test]
    fn sources_sharing_a_credential_run_one_after_another() {
        let f = Fixture::new();
        let cred = f.credential("shared");
        let (a, _) = f.source(cred, "acme");
        let (b, _) = f.source(cred, "beta");
        let (ma, mb) = (MockS3::new("A", &f.log), MockS3::new("B", &f.log));
        for (m, n) in [(&ma, "a"), (&mb, "b")] {
            for i in 0..3 {
                m.put(
                    &format!("owner/{i}"),
                    "e",
                    &mime(&format!("{n}{i}"), "S", "b"),
                );
            }
        }
        f.sync(&[(a, &ma), (b, &mb)]);
        let log = f.log.lock().unwrap().clone();
        let first = log[0].chars().next().unwrap();
        let split = log.iter().position(|l| !l.starts_with(first)).unwrap();
        assert!(log[..split].iter().all(|l| l.starts_with(first)));
        assert!(log[split..].iter().all(|l| !l.starts_with(first)));
        assert_eq!(log.len(), 8);
    }

    #[test]
    fn sources_on_different_credentials_overlap() {
        let f = Fixture::new();
        let (c1, c2) = (f.credential("one"), f.credential("two"));
        let (a, _) = f.source(c1, "acme");
        let (b, _) = f.source(c2, "beta");
        let rendezvous = Rendezvous::new(2);
        let make = |label: &str| {
            Arc::new(MockS3 {
                rendezvous: Some(rendezvous.clone()),
                ..Arc::try_unwrap(MockS3::new(label, &f.log)).ok().unwrap()
            })
        };
        let (ma, mb) = (make("A"), make("B"));
        f.sync(&[(a, &ma), (b, &mb)]);
        assert!(
            rendezvous.met(),
            "both listings were never in flight together"
        );
    }

    #[test]
    fn s3_credential_decides_the_lock_not_the_ses_one() {
        let f = Fixture::new();
        let (ses1, ses2, s3key) = (
            f.credential("ses1"),
            f.credential("ses2"),
            f.credential("s3"),
        );
        let (a, _) = f.source(ses1, "acme");
        let (b, _) = f.source(ses2, "beta");
        f.store
            .lock_conn()
            .unwrap()
            .execute("UPDATE mail_account SET s3_credential_id = ?1", [s3key])
            .unwrap();
        let rendezvous = Rendezvous::new(2);
        let make = |label: &str| {
            Arc::new(MockS3 {
                rendezvous: Some(rendezvous.clone()),
                ..Arc::try_unwrap(MockS3::new(label, &f.log)).ok().unwrap()
            })
        };
        let (ma, mb) = (make("A"), make("B"));
        f.sync(&[(a, &ma), (b, &mb)]);
        assert!(!rendezvous.met(), "same S3 key must serialise");
    }

    #[test]
    fn one_failing_source_leaves_the_others_clean() {
        let f = Fixture::new();
        let (c1, c2) = (f.credential("one"), f.credential("two"));
        let (a, mba) = f.source(c1, "acme");
        let (b, mbb) = f.source(c2, "beta");
        let (bad, good) = (MockS3::new("A", &f.log), MockS3::new("B", &f.log));
        *bad.list_error.lock().unwrap() = Some(S3Error {
            kind: S3ErrorKind::AccessDenied,
            code: Some("AccessDenied".into()),
            message: "Access denied to the bucket.".into(),
        });
        good.put("owner/1", "e1", &mime("g1", "Hello", "fine"));
        let reports = f.sync(&[(a, &bad), (b, &good)]);
        assert_eq!(
            reports[0].error.as_deref(),
            Some("Access denied to the bucket.")
        );
        assert_eq!(reports[1].error, None);
        assert_eq!(reports[1].inserted, 1);
        let conn = f.store.lock_conn().unwrap();
        let sa = mail_store::get_sync_state(&conn, mba).unwrap().unwrap();
        let sb = mail_store::get_sync_state(&conn, mbb).unwrap().unwrap();
        assert_eq!(
            sa.last_error.as_deref(),
            Some("Access denied to the bucket.")
        );
        assert_eq!(sb.last_error, None);
        let readable: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM mail_thread t JOIN mail_mailbox m ON m.id=t.mailbox_id
                 WHERE m.account_id = ?1",
                [b],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(readable, 1);
    }

    #[test]
    fn missing_client_marks_that_source_only() {
        let f = Fixture::new();
        let (c1, c2) = (f.credential("one"), f.credential("two"));
        let (a, mba) = f.source(c1, "acme");
        let (b, _) = f.source(c2, "beta");
        let good = MockS3::new("B", &f.log);
        let reports = f.sync(&[(b, &good)]);
        assert_eq!(reports.len(), 2);
        assert_eq!(reports[0].account_id, a);
        assert_eq!(reports[0].error.as_deref(), Some("no client"));
        assert_eq!(reports[1].error, None);
        let conn = f.store.lock_conn().unwrap();
        let state = mail_store::get_sync_state(&conn, mba).unwrap().unwrap();
        assert_eq!(state.last_error.as_deref(), Some("no client"));
    }

    #[test]
    fn events_report_progress_and_content_change() {
        let f = Fixture::new();
        let (account, _, s3) = one_source(&f);
        s3.put("owner/1", "e1", &mime("m1", "Hello", "first"));
        let factory = MockFactory(HashMap::from([(account, s3.clone())]));
        let events = Recorder::default();
        f.service.sync_now(&f.store, &factory, &events, None);
        let seen = events.0.lock().unwrap().clone();
        assert_eq!(
            seen.first().unwrap(),
            &format!("state:{account}:running:true")
        );
        assert!(seen.contains(&format!("state:{account}:done:false")));
        assert_eq!(seen.last().unwrap(), &format!("changed:{account}"));
        let quiet = Recorder::default();
        f.service.sync_now(&f.store, &factory, &quiet, None);
        assert!(!quiet
            .0
            .lock()
            .unwrap()
            .iter()
            .any(|e| e.starts_with("changed")));
    }

    #[test]
    fn scheduled_sync_skips_fresh_mailboxes() {
        let f = Fixture::new();
        let (account, _, s3) = one_source(&f);
        s3.put("owner/1", "e1", &mime("m1", "Hello", "first"));
        let factory = MockFactory(HashMap::from([(account, s3.clone())]));
        let ev = Recorder::default();
        assert_eq!(f.service.interval_secs(), 3600);
        let now = now_millis();
        assert_eq!(f.service.sync_due(&f.store, &factory, &ev, now).len(), 1);
        assert!(f
            .service
            .sync_due(&f.store, &factory, &ev, now + 1000)
            .is_empty());
        let later = now + 3600 * 1000 + 5000;
        assert_eq!(f.service.sync_due(&f.store, &factory, &ev, later).len(), 1);
        f.service.set_interval_secs(1);
        assert_eq!(f.service.interval_secs(), MIN_INTERVAL_SECS);
    }
}
