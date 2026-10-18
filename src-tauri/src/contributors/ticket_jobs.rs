//! Background jobs of the Contributors ticket scoring: fetching the ClickUp
//! tickets updated in a period (every status) and having the AI assess them.
//!
//! One worker thread takes jobs from a FIFO queue, one at a time. The jobs live
//! in memory only: after a restart there is no ghost, and the next Assess
//! continues with the tickets that still have no assessment. Nothing here
//! starts on its own; the webview asks for every job.
//!
//! The database write lock is held only around a write, never across a network
//! call or an AI call.

use std::collections::{HashMap, VecDeque};
use std::panic::AssertUnwindSafe;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rusqlite::Connection;
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

use super::assessor::{self, Assessor, RUNNER_API};
use super::clickup_tickets::{self, is_done_type, ClickUpHttp, LiveClickUp, TicketLink};
use super::rubric::{self, TicketInput};
use super::store::sql;
use super::ticket_store::{self, StoredAssessment, StoredTicket};
use super::{epoch_to_iso, lock, now_epoch};
use crate::clickup::is_rate_limited;
use crate::session_store::SessionStore;

/// Event the webview listens to.
pub(super) const EVENT: &str = "contributors://tickets";

/// A running job emits progress at most this often; state changes always go out.
const THROTTLE: Duration = Duration::from_millis(150);

/// Tickets per AI call.
const ASSESS_GROUP: usize = 20;
/// Groups in flight at once for the Anthropic API runner.
const API_CONCURRENCY: usize = 4;
/// Groups in flight at once for the Claude Code CLI runner.
const CLI_CONCURRENCY: usize = 2;

/// How a ClickUp call that hits a rate limit waits before it tries again. The
/// shared backoff lasts at most two minutes, so the defaults outwait it.
#[derive(Clone, Copy, Debug)]
pub(super) struct RateWait {
    /// Time between two attempts.
    pub(super) pause: Duration,
    /// Attempts after the first one.
    pub(super) retries: u32,
}

impl RateWait {
    const LIVE: RateWait = RateWait {
        pause: Duration::from_secs(2),
        retries: 70,
    };

    /// Runs `call`, retrying while ClickUp answers with a rate-limit error:
    /// the error is transient, so the call waits for the backoff to clear
    /// instead of giving up at once. The wait honours a cancel. Any other
    /// result, and a rate-limit error that outlasts the retries, is returned
    /// as it is.
    fn run<T>(
        &self,
        live: &Live,
        mut call: impl FnMut() -> Result<T, String>,
    ) -> Result<T, String> {
        let mut retries = 0;
        loop {
            match call() {
                Err(error)
                    if is_rate_limited(&error) && retries < self.retries && !live.cancelled() =>
                {
                    retries += 1;
                    self.pause(live);
                }
                other => return other,
            }
        }
    }

    /// Sleeps for the pause in short slices so a cancel is seen quickly.
    fn pause(&self, live: &Live) {
        let deadline = Instant::now() + self.pause;
        while !live.cancelled() {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break;
            }
            std::thread::sleep(left.min(Duration::from_millis(100)));
        }
    }
}

const PANICKED: &str = "The ticket job stopped unexpectedly.";

const KIND_FETCH: &str = "fetch";
const KIND_ASSESS: &str = "assess";
const QUEUED: &str = "queued";
const RUNNING: &str = "running";
const DONE: &str = "done";
const ERROR: &str = "error";
const CANCELLED: &str = "cancelled";

// ---------------------------------------------------------------------------
// events
// ---------------------------------------------------------------------------

/// What the webview is told about a job. `done` counts the items the job has
/// finished with (links for a fetch, tickets for an assessment), failed ones
/// included; `failed` is the part of `done` that did not work out.
#[derive(Clone, Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TicketJobEvent {
    /// `"fetch"` or `"assess"`.
    kind: String,
    /// `"queued"`, `"running"`, `"done"`, `"error"` or `"cancelled"`.
    state: String,
    done: u64,
    total: u64,
    failed: u64,
    error: Option<String>,
    /// The job this event is about; `start` returned it.
    job_id: u64,
    /// Fetch jobs queued or running when the event went out (this one
    /// included until it ends). Set by the manager as the event is sent.
    pending_fetches: u32,
}

/// What `contributors_tickets_status` returns: the running (or next queued)
/// job's last event, and how many fetch jobs are queued or running. Read under
/// one lock, so the two agree.
#[derive(Clone, Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TicketJobStatus {
    pub(crate) event: Option<TicketJobEvent>,
    pub(crate) pending_fetches: u32,
}

impl TicketJobEvent {
    fn new(kind: &str, state: &str, total: u64) -> Self {
        Self {
            kind: kind.to_string(),
            state: state.to_string(),
            done: 0,
            total,
            failed: 0,
            error: None,
            job_id: 0,
            pending_fetches: 0,
        }
    }
}

/// Where events go. Tests record them; the app emits them to the webview.
pub(super) trait TicketSink: Send + Sync {
    fn emit(&self, event: &TicketJobEvent);
}

pub(super) struct TauriSink(pub AppHandle);

impl TicketSink for TauriSink {
    fn emit(&self, event: &TicketJobEvent) {
        let _ = self.0.emit(EVENT, event);
    }
}

// ---------------------------------------------------------------------------
// what a job needs
// ---------------------------------------------------------------------------

/// Everything outside the process a job touches, so tests plug in fakes.
pub(super) trait TicketDeps: Send + Sync {
    fn store(&self) -> &SessionStore;
    fn http(&self) -> Result<Box<dyn ClickUpHttp>, String>;
    fn assessor(&self) -> Result<Box<dyn Assessor>, String>;

    /// How long a rate-limited ClickUp call waits before it retries.
    fn rate_wait(&self) -> RateWait {
        RateWait::LIVE
    }
}

/// The real dependencies: the stored ClickUp token and the AI settings are read
/// when a job starts, so a job that waited uses what is saved by then.
pub(super) struct AppDeps(pub AppHandle);

impl TicketDeps for AppDeps {
    fn store(&self) -> &SessionStore {
        self.0.state::<SessionStore>().inner()
    }

    fn http(&self) -> Result<Box<dyn ClickUpHttp>, String> {
        let config = crate::clickup::require_config(&self.0)?;
        Ok(Box::new(LiveClickUp {
            token: config.token,
        }))
    }

    fn assessor(&self) -> Result<Box<dyn Assessor>, String> {
        assessor::assessor_for(&assessor::read_ai_config(&self.0)?)
    }
}

// ---------------------------------------------------------------------------
// the manager
// ---------------------------------------------------------------------------

pub(crate) enum TicketJob {
    /// Fetches the tickets of each link updated since `from` (epoch seconds,
    /// inclusive), whatever their status.
    Fetch { links: Vec<TicketLink>, from: i64 },
    /// Assesses the tickets among `ids` that need it (all of them with `force`,
    /// except the ones with a manual level).
    Assess { ids: Vec<String>, force: bool },
}

impl TicketJob {
    fn queued_event(&self) -> TicketJobEvent {
        match self {
            TicketJob::Fetch { links, .. } => {
                TicketJobEvent::new(KIND_FETCH, QUEUED, links.len() as u64)
            }
            TicketJob::Assess { .. } => TicketJobEvent::new(KIND_ASSESS, QUEUED, 0),
        }
    }
}

enum Outcome {
    Finished,
    Cancelled,
}

/// What a running job shares with its workers and with `status`.
struct Live {
    cancel: AtomicBool,
    last: Mutex<Last>,
    sink: Arc<dyn TicketSink>,
}

struct Last {
    event: TicketJobEvent,
    emitted_at: Instant,
}

impl Live {
    fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }

    fn snapshot(&self) -> TicketJobEvent {
        lock(&self.last).event.clone()
    }

    /// Records a state change and returns the event to send. Never throttled.
    fn change(&self, state: &str, error: Option<String>) -> TicketJobEvent {
        let mut last = lock(&self.last);
        last.event.state = state.to_string();
        last.event.error = error;
        last.emitted_at = Instant::now();
        last.event.clone()
    }

    fn set_total(&self, total: u64) {
        let event = {
            let mut last = lock(&self.last);
            last.event.total = total;
            last.emitted_at = Instant::now();
            last.event.clone()
        };
        self.sink.emit(&event);
    }

    /// Adds finished items. The counters change under the lock so workers
    /// finishing together cannot overwrite each other.
    fn advance(&self, done: u64, failed: u64) {
        let event = {
            let mut last = lock(&self.last);
            last.event.done += done;
            last.event.failed += failed;
            if last.emitted_at.elapsed() < THROTTLE {
                return;
            }
            last.emitted_at = Instant::now();
            last.event.clone()
        };
        self.sink.emit(&event);
    }
}

struct Queued {
    job: TicketJob,
    event: TicketJobEvent,
}

#[derive(Default)]
struct Jobs {
    /// Id of the last job started.
    last_id: u64,
    running: Option<Arc<Live>>,
    queued: VecDeque<Queued>,
    /// A worker thread exists. Set and cleared under the lock together with the
    /// queue, so a job queued while the worker leaves is never lost.
    worker: bool,
}

struct Shared {
    jobs: Mutex<Jobs>,
    deps: Arc<dyn TicketDeps>,
    sink: Arc<dyn TicketSink>,
    /// Fetch jobs queued or running. Changed only under the `jobs` lock, and
    /// always before the event that reports the change goes out.
    pending: Arc<AtomicU32>,
}

/// Puts the number of pending fetch jobs on every event as it is sent, so
/// progress events (which are sent without the `jobs` lock) carry it too.
struct StampingSink {
    inner: Arc<dyn TicketSink>,
    pending: Arc<AtomicU32>,
}

impl TicketSink for StampingSink {
    fn emit(&self, event: &TicketJobEvent) {
        let mut stamped = event.clone();
        stamped.pending_fetches = self.pending.load(Ordering::SeqCst);
        self.inner.emit(&stamped);
    }
}

#[derive(Clone)]
pub(crate) struct TicketJobManager(Arc<Shared>);

impl TicketJobManager {
    pub(super) fn new(deps: Arc<dyn TicketDeps>, sink: Arc<dyn TicketSink>) -> Self {
        let pending = Arc::new(AtomicU32::new(0));
        Self(Arc::new(Shared {
            jobs: Mutex::new(Jobs::default()),
            deps,
            sink: Arc::new(StampingSink {
                inner: sink,
                pending: pending.clone(),
            }),
            pending,
        }))
    }

    /// Queues a job and returns its id. It runs after the ones queued before it.
    pub(super) fn start(&self, job: TicketJob) -> Result<u64, String> {
        let mut jobs = lock(&self.0.jobs);
        let mut event = job.queued_event();
        let id = jobs.last_id + 1;
        event.job_id = id;
        // Taken before the spawn: a failed start still sent an event with it.
        jobs.last_id = id;
        if !jobs.worker {
            let manager = self.clone();
            // The worker waits for the lock held here, so `queued` always goes
            // out before its `running`.
            let spawned = std::thread::Builder::new()
                .name("contributors-tickets".to_string())
                .spawn(move || manager.work());
            if let Err(error) = spawned {
                let message = format!("Could not start the ticket job: {error}");
                let mut failed = event;
                failed.state = ERROR.to_string();
                failed.error = Some(message.clone());
                self.0.sink.emit(&failed);
                return Err(message);
            }
            jobs.worker = true;
        }
        if matches!(job, TicketJob::Fetch { .. }) {
            self.0.pending.fetch_add(1, Ordering::SeqCst);
        }
        self.0.sink.emit(&event);
        jobs.queued.push_back(Queued { job, event });
        Ok(id)
    }

    /// Stops the running job and drops the ones queued behind it.
    pub(super) fn cancel(&self) {
        let mut jobs = lock(&self.0.jobs);
        for queued in jobs.queued.drain(..) {
            let mut event = queued.event;
            event.state = CANCELLED.to_string();
            if matches!(queued.job, TicketJob::Fetch { .. }) {
                self.0.pending.fetch_sub(1, Ordering::SeqCst);
            }
            self.0.sink.emit(&event);
        }
        if let Some(live) = &jobs.running {
            live.cancel.store(true, Ordering::Relaxed);
        }
    }

    /// The last event of the running (or next queued) job together with the
    /// number of fetch jobs queued or running, read under one lock.
    pub(super) fn status_with_pending(&self) -> TicketJobStatus {
        let jobs = lock(&self.0.jobs);
        let pending_fetches = self.0.pending.load(Ordering::SeqCst);
        let mut event = match &jobs.running {
            Some(live) => Some(live.snapshot()),
            None => jobs.queued.front().map(|queued| queued.event.clone()),
        };
        if let Some(event) = &mut event {
            event.pending_fetches = pending_fetches;
        }
        TicketJobStatus {
            event,
            pending_fetches,
        }
    }

    /// The worker: takes jobs one by one, then leaves.
    fn work(&self) {
        loop {
            let (job, live) = {
                let mut jobs = lock(&self.0.jobs);
                let Some(next) = jobs.queued.pop_front() else {
                    jobs.worker = false;
                    return;
                };
                let mut event = next.event;
                event.state = RUNNING.to_string();
                let live = Arc::new(Live {
                    cancel: AtomicBool::new(false),
                    last: Mutex::new(Last {
                        event,
                        emitted_at: Instant::now(),
                    }),
                    sink: self.0.sink.clone(),
                });
                self.0.sink.emit(&live.snapshot());
                jobs.running = Some(live.clone());
                (next.job, live)
            };

            let result = std::panic::catch_unwind(AssertUnwindSafe(|| {
                run_job(self.0.deps.as_ref(), &job, &live)
            }));
            let (state, error) = match result {
                Ok(Ok(Outcome::Finished)) => (DONE, None),
                Ok(Ok(Outcome::Cancelled)) => (CANCELLED, None),
                Ok(Err(error)) => (ERROR, Some(error)),
                Err(_) => (ERROR, Some(PANICKED.to_string())),
            };

            let mut jobs = lock(&self.0.jobs);
            jobs.running = None;
            if matches!(job, TicketJob::Fetch { .. }) {
                self.0.pending.fetch_sub(1, Ordering::SeqCst);
            }
            self.0.sink.emit(&live.change(state, error));
        }
    }
}

fn run_job(deps: &dyn TicketDeps, job: &TicketJob, live: &Live) -> Result<Outcome, String> {
    match job {
        TicketJob::Fetch { links, from } => run_fetch(deps, live, links, *from),
        TicketJob::Assess { ids, force } => run_assess(deps, live, ids, *force),
    }
}

// ---------------------------------------------------------------------------
// fetch
// ---------------------------------------------------------------------------

/// Whether ClickUp's status history can be read for one workspace during a
/// fetch run.
#[derive(Clone, Copy, PartialEq)]
enum History {
    /// No answer yet.
    Untried,
    /// ClickUp answered with at least one date.
    Available,
    /// The first answer had no date at all (the "Total time in Status"
    /// ClickApp is off in that workspace) or a call failed: the rest of the run
    /// does without for that workspace.
    Unavailable,
}

/// Fetches each link's tickets updated since `from` and stores them, one
/// transaction per link. A link that fails does not stop the others; the job
/// then ends in error, naming the first failure, with the working links
/// already stored.
fn run_fetch(
    deps: &dyn TicketDeps,
    live: &Live,
    links: &[TicketLink],
    from: i64,
) -> Result<Outcome, String> {
    if links.is_empty() {
        return Ok(Outcome::Finished);
    }
    let http = deps.http()?;
    let rate_wait = deps.rate_wait();
    // ClickUp compares strictly; `tasks_path` asks one millisecond before this,
    // so a ticket updated exactly at `from` is returned.
    let from_ms = from.saturating_mul(1000);
    // The ClickApp is enabled per workspace, so each workspace has its own state.
    let mut histories: HashMap<String, History> = HashMap::new();
    let mut failed = 0u64;
    let mut first_error: Option<String> = None;
    for link in links {
        if live.cancelled() {
            return Ok(Outcome::Cancelled);
        }
        // A ticket updated after the listing began may be missing from it, so
        // the link is covered up to this moment, not up to the commit.
        let listing_start = now_epoch();
        let result =
            clickup_tickets::fetch_tasks(http.as_ref(), link, from_ms, &|| live.cancelled())
                .and_then(|tickets| {
                    let history = histories
                        .entry(link.workspace_id.clone())
                        .or_insert(History::Untried);
                    store_link(
                        deps.store(),
                        http.as_ref(),
                        rate_wait,
                        live,
                        history,
                        link,
                        from,
                        listing_start,
                        tickets,
                    )
                });
        match result {
            Ok(()) => live.advance(1, 0),
            // A cancelled fetch returns an error, never a partial list.
            Err(_) if live.cancelled() => return Ok(Outcome::Cancelled),
            Err(error) => {
                failed += 1;
                first_error.get_or_insert(error);
                live.advance(1, 1);
            }
        }
    }
    match first_error {
        None => Ok(Outcome::Finished),
        Some(error) => Err(format!(
            "Could not fetch {failed} of {} ClickUp links: {error}",
            links.len()
        )),
    }
}

/// What ClickUp says about a stored ticket the listing did not return.
enum Recheck {
    /// Still there (archived, say): its current state replaces the stored one.
    Present(Box<StoredTicket>),
    /// ClickUp no longer finds it: it must stop scoring.
    Gone,
    /// Could not tell: the row stays.
    Unknown,
}

/// ClickUp answers a deleted or inaccessible task with "Task not found" (or the
/// generic 404 text of `clickup_get`).
fn is_not_found(error: &str) -> bool {
    let error = error.to_lowercase();
    error.contains("not found") || error.contains("could not find that item")
}

/// A lookup that stays rate limited after the retries is `Unknown`, so the row
/// stays.
fn recheck(
    http: &dyn ClickUpHttp,
    rate_wait: RateWait,
    live: &Live,
    id: &str,
    workspace_id: &str,
) -> Recheck {
    match rate_wait.run(live, || http.get(&format!("/task/{id}"))) {
        Ok(node) => match clickup_tickets::parse_task(&node, workspace_id) {
            Some(ticket) => Recheck::Present(Box::new(ticket)),
            // Unreadable (no id or no update date): not proof of anything.
            None => Recheck::Unknown,
        },
        Err(error) if is_not_found(&error) => Recheck::Gone,
        Err(_) => Recheck::Unknown,
    }
}

/// Sets `status_since` on the tickets that are not done or closed (a done
/// ticket has its done date). The first answer with no date at all means the
/// history is not available; so does any error other than a rate limit, and
/// the rest of the run stops asking. A rate limit is waited out, and one that
/// does not clear is an error, so tickets are not stored without dates they
/// could have had.
fn add_status_since(
    http: &dyn ClickUpHttp,
    rate_wait: RateWait,
    live: &Live,
    history: &mut History,
    tickets: &mut [StoredTicket],
) -> Result<(), String> {
    let ids: Vec<String> = tickets
        .iter()
        .filter(|ticket| !is_done_type(&ticket.status_type))
        .map(|ticket| ticket.id.clone())
        .collect();
    let mut since: HashMap<String, String> = HashMap::new();
    for chunk in ids.chunks(clickup_tickets::BULK_IDS) {
        if *history == History::Unavailable {
            break;
        }
        if live.cancelled() {
            return Err("Ticket fetch cancelled".to_string());
        }
        match rate_wait.run(live, || clickup_tickets::fetch_status_since(http, chunk)) {
            Ok(found) if found.is_empty() && *history == History::Untried => {
                *history = History::Unavailable;
            }
            Ok(found) => {
                *history = History::Available;
                since.extend(found);
            }
            Err(error) if is_rate_limited(&error) => return Err(error),
            Err(_) => *history = History::Unavailable,
        }
    }
    for ticket in tickets {
        if let Some(at) = since.remove(&ticket.id) {
            ticket.status_since = Some(at);
        }
    }
    Ok(())
}

/// A ticket that is not done and got no date this run (the history was
/// unavailable or a call failed) keeps the date already stored, as long as it
/// is still in the same status: a stored date from another status says nothing
/// about this one. Writing the ticket with `None` would erase it.
fn keep_stored_status_since(
    store: &SessionStore,
    tickets: &mut [StoredTicket],
) -> Result<(), String> {
    let wanted: Vec<String> = tickets
        .iter()
        .filter(|ticket| !is_done_type(&ticket.status_type) && ticket.status_since.is_none())
        .map(|ticket| ticket.id.clone())
        .collect();
    if wanted.is_empty() {
        return Ok(());
    }
    let mut stored =
        store.with_read_conn(|conn| ticket_store::stored_status_since(conn, &wanted))?;
    for ticket in tickets {
        if ticket.status_since.is_some() {
            continue;
        }
        if let Some((status, since)) = stored.remove(&ticket.id) {
            if status == ticket.status {
                ticket.status_since = Some(since);
            }
        }
    }
    Ok(())
}

/// Stores a completed fetch of one link. A stored ticket updated since `from`
/// that the listing no longer shows is looked up on its own before anything is
/// deleted: the listing leaves out archived tasks, so a missing row is no
/// proof it is gone. Only a ticket ClickUp no longer finds is dropped; a lookup
/// that fails any other way keeps the row. The lookups run before the write
/// lock is taken; the writes share one transaction.
#[allow(clippy::too_many_arguments)]
fn store_link(
    store: &SessionStore,
    http: &dyn ClickUpHttp,
    rate_wait: RateWait,
    live: &Live,
    history: &mut History,
    link: &TicketLink,
    from: i64,
    listing_start: i64,
    mut tickets: Vec<StoredTicket>,
) -> Result<(), String> {
    let spaces = link
        .space_ids
        .as_deref()
        .filter(|spaces| !spaces.is_empty());
    let missing = store.with_read_conn(|conn| {
        ticket_store::unreturned_ids(conn, &link.workspace_id, spaces, from, &tickets)
    })?;
    let mut gone = Vec::new();
    for id in missing {
        if live.cancelled() {
            return Err("Ticket fetch cancelled".to_string());
        }
        match recheck(http, rate_wait, live, &id, &link.workspace_id) {
            Recheck::Present(ticket) => tickets.push(*ticket),
            Recheck::Gone => gone.push(id),
            Recheck::Unknown => {}
        }
    }
    add_status_since(http, rate_wait, live, history, &mut tickets)?;
    keep_stored_status_since(store, &mut tickets)?;
    let conn = store.lock_conn()?;
    let tx = conn.unchecked_transaction().map_err(sql)?;
    ticket_store::put_tickets(&tx, &tickets)?;
    ticket_store::delete_tickets(&tx, &gone)?;
    // The link completed: its spaces now have everything updated from `from`
    // until the listing began.
    ticket_store::mark_covered(
        &tx,
        &link.workspace_id,
        &ticket_store::spaces_key(link.space_ids.as_deref()),
        from,
        listing_start,
        now_epoch(),
    )?;
    tx.commit().map_err(sql)
}

// ---------------------------------------------------------------------------
// assess
// ---------------------------------------------------------------------------

type Item = (StoredTicket, Vec<String>);

const REDACTED: &str = "[assignee]";

/// Parts of a name or email shorter than this are not hidden on their own.
const MIN_PART: usize = 3;

/// The names and emails of a ticket's assignees, longest first so an email is
/// replaced before a name that is part of it. Each word of a name and the part
/// of an email before the `@` are hidden on their own too ("Ada" in "Ada
/// Lovelace"), when they have at least three characters. One-character full
/// values are left alone: they would only mangle ordinary text.
fn hidden_words(ticket: &StoredTicket) -> Vec<Vec<char>> {
    let mut words: Vec<&str> = Vec::new();
    for assignee in &ticket.assignees {
        let name = assignee.name.trim();
        let email = assignee.email.trim();
        words.extend(
            [name, email]
                .into_iter()
                .filter(|word| word.chars().count() > 1),
        );
        words.extend(
            name.split_whitespace()
                .map(|part| part.trim_matches(|c: char| !c.is_alphanumeric()))
                .filter(|part| part.chars().count() >= MIN_PART),
        );
        if let Some((local, _)) = email.split_once('@') {
            let local = local.trim();
            if local.chars().count() >= MIN_PART {
                words.push(local);
            }
        }
    }
    let mut words: Vec<Vec<char>> = words
        .into_iter()
        .map(|word| word.chars().collect())
        .collect();
    words.sort_by_key(|word| std::cmp::Reverse(word.len()));
    words.dedup();
    words
}

/// Replaces every whole, case-insensitive occurrence of a hidden word with
/// `[assignee]`. "Whole" means the characters around it are not letters or
/// digits, so `@Ada Lovelace` and `ada@example.com,` match but `Adams` does
/// not match `Ada`.
fn redact(text: &str, hidden: &[Vec<char>]) -> String {
    if hidden.is_empty() {
        return text.to_string();
    }
    let chars: Vec<char> = text.chars().collect();
    let same = |a: char, b: char| a == b || a.to_lowercase().eq(b.to_lowercase());
    let mut out = String::with_capacity(text.len());
    let mut at = 0;
    while at < chars.len() {
        let hit = hidden.iter().find(|word| {
            let end = at + word.len();
            end <= chars.len()
                && chars[at..end]
                    .iter()
                    .zip(word.iter())
                    .all(|(a, b)| same(*a, *b))
                && (at == 0 || !chars[at - 1].is_alphanumeric())
                && (end == chars.len() || !chars[end].is_alphanumeric())
        });
        match hit {
            Some(word) => {
                out.push_str(REDACTED);
                at += word.len();
            }
            None => {
                out.push(chars[at]);
                at += 1;
            }
        }
    }
    out
}

/// What the model reads about one ticket: the free text with the assignees'
/// names and emails hidden (they can show up anywhere, a comment @mentioning
/// them, say; none may reach the AI), then cut to the size limits. The
/// estimate and the job both build their inputs here, so the estimate counts
/// what would really be sent.
fn ticket_input(
    key: String,
    ticket: &StoredTicket,
    subtasks: &[String],
    comments: &[String],
    hours_in_progress: Option<f64>,
) -> TicketInput {
    let hidden = hidden_words(ticket);
    let redact = |text: &str| redact(text, &hidden);
    let mut input = TicketInput {
        key,
        title: redact(&ticket.title),
        description: redact(&ticket.description),
        tags: ticket.tags.iter().map(|tag| redact(tag)).collect(),
        subtask_titles: subtasks.iter().map(|title| redact(title)).collect(),
        comments: comments.iter().map(|comment| redact(comment)).collect(),
        hours_in_progress,
    };
    rubric::truncate_input(&mut input);
    input
}

/// A rough size of an assessment run, shown before it starts.
#[derive(Clone, Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TicketEstimate {
    /// Tickets that would really be assessed (not assessed yet, no manual level).
    tickets: u32,
    input_tokens: u64,
}

/// Rough tokens of a text: one per four characters, rounded up.
fn tokens(chars: usize) -> u64 {
    chars.div_ceil(4) as u64
}

/// What assessing the tickets among `ids` would send: for each ticket the
/// title, description, tags and subtask titles as the job builds them (hidden
/// assignees, cut to the limits; comments are left out, since they are only
/// fetched during the run), plus the system prompt once per group. Tickets
/// already assessed or with a manual level are not counted.
pub(crate) fn estimate(conn: &Connection, ids: &[String]) -> Result<TicketEstimate, String> {
    let items = ticket_store::tickets_for_assessment(conn, ids, false)?;
    let mut input_tokens = 0;
    for (ticket, subtasks) in &items {
        let input = ticket_input(String::new(), ticket, subtasks, &[], None);
        let chars: usize = [&input.title, &input.description]
            .into_iter()
            .chain(&input.tags)
            .chain(&input.subtask_titles)
            .map(|text| text.chars().count())
            .sum();
        input_tokens += tokens(chars);
    }
    let groups = items.len().div_ceil(ASSESS_GROUP) as u64;
    input_tokens += groups * tokens(rubric::system_prompt().chars().count());
    Ok(TicketEstimate {
        tickets: u32::try_from(items.len()).unwrap_or(u32::MAX),
        input_tokens,
    })
}

fn concurrency(runner: &str) -> usize {
    if runner == RUNNER_API {
        API_CONCURRENCY
    } else {
        CLI_CONCURRENCY
    }
}

/// Assesses the tickets that need it, group by group. Each finished group is
/// written at once, so a cancel or a crash keeps what is done and the next run
/// sends only the rest.
fn run_assess(
    deps: &dyn TicketDeps,
    live: &Live,
    ids: &[String],
    force: bool,
) -> Result<Outcome, String> {
    let items = deps
        .store()
        .with_read_conn(|conn| ticket_store::tickets_for_assessment(conn, ids, force))?;
    live.set_total(items.len() as u64);
    if items.is_empty() {
        return Ok(Outcome::Finished);
    }
    let http = deps.http()?;
    let assessor = deps.assessor()?;
    let (runner, model) = assessor.label();
    let run = AssessRun {
        store: deps.store(),
        http: http.as_ref(),
        rate_wait: deps.rate_wait(),
        assessor: assessor.as_ref(),
        live,
        runner,
        model,
        assessed_at: epoch_to_iso(now_epoch()),
        time_disabled: Mutex::new(false),
        fatal: Mutex::new(None),
    };

    let groups: Vec<&[Item]> = items.chunks(ASSESS_GROUP).collect();
    let workers = concurrency(runner).min(groups.len());
    let next = AtomicUsize::new(0);
    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| loop {
                if live.cancelled() || lock(&run.fatal).is_some() {
                    break;
                }
                let Some(group) = groups.get(next.fetch_add(1, Ordering::Relaxed)) else {
                    break;
                };
                run.run_group(group);
            });
        }
    });

    if let Some(error) = lock(&run.fatal).take() {
        return Err(error);
    }
    Ok(if live.cancelled() {
        Outcome::Cancelled
    } else {
        Outcome::Finished
    })
}

struct AssessRun<'a> {
    store: &'a SessionStore,
    http: &'a dyn ClickUpHttp,
    rate_wait: RateWait,
    assessor: &'a dyn Assessor,
    live: &'a Live,
    runner: &'static str,
    model: String,
    assessed_at: String,
    /// Set by the first time-in-status failure; held across that call so groups
    /// running together cannot all repeat it.
    time_disabled: Mutex<bool>,
    /// The error that stops the whole run.
    fatal: Mutex<Option<String>>,
}

impl AssessRun<'_> {
    fn fail(&self, error: String) {
        lock(&self.fatal).get_or_insert(error);
    }

    /// Hours each ticket spent in progress. After the first error the rest of
    /// the run goes without them: the field is optional for the rubric. A rate
    /// limit is no such error: it is waited out, and one that does not clear
    /// is `Err`, since the tickets must not be assessed without the hours.
    fn hours_in_progress(&self, ids: &[String]) -> Result<Option<HashMap<String, f64>>, String> {
        let mut disabled = lock(&self.time_disabled);
        if *disabled {
            return Ok(None);
        }
        match self.rate_wait.run(self.live, || {
            clickup_tickets::fetch_hours_in_progress(self.http, ids)
        }) {
            Ok(hours) => Ok(Some(hours)),
            Err(error) if is_rate_limited(&error) => Err(error),
            Err(_) => {
                *disabled = true;
                Ok(None)
            }
        }
    }

    /// What the model reads about each ticket of a group, under the keys
    /// `t1..tN`. Assignees never appear here, and neither does code size.
    /// `None` when the group cannot be built: ClickUp stayed rate limited, so
    /// the tickets would go out without comments or hours, or the job was
    /// cancelled.
    fn build_inputs(&self, group: &[Item]) -> Option<Vec<TicketInput>> {
        let ids: Vec<String> = group.iter().map(|(ticket, _)| ticket.id.clone()).collect();
        let hours = self.hours_in_progress(&ids).ok()?;
        let mut inputs = Vec::with_capacity(group.len());
        for (index, (ticket, subtasks)) in group.iter().enumerate() {
            if self.live.cancelled() {
                return None;
            }
            // Comments only add context: a ticket whose comments cannot be
            // read is still assessed from the rest. A rate limit is not
            // that case: it is waited out, and one that does not clear
            // leaves the group unassessed for the next run.
            let comments = match self.rate_wait.run(self.live, || {
                clickup_tickets::fetch_comments(self.http, &ticket.id)
            }) {
                Ok(comments) => comments,
                Err(error) if is_rate_limited(&error) => return None,
                Err(_) => Vec::new(),
            };
            inputs.push(ticket_input(
                format!("t{}", index + 1),
                ticket,
                subtasks,
                &comments,
                hours
                    .as_ref()
                    .and_then(|hours| hours.get(&ticket.id).copied()),
            ));
        }
        Some(inputs)
    }

    fn ask(&self, inputs: &[TicketInput]) -> Result<rubric::Parsed, String> {
        let reply = self.assessor.assess(
            rubric::system_prompt(),
            &rubric::user_prompt(inputs),
            &|| self.live.cancelled(),
        )?;
        let keys: Vec<String> = inputs.iter().map(|input| input.key.clone()).collect();
        Ok(rubric::parse_reply(&reply, &keys))
    }

    /// One group: ask, retry the tickets that came back unusable once in a
    /// smaller group of their own, write what worked, count what did not. An
    /// error of the AI call itself is retried once too; two in a row for the
    /// same group stop the run, since the next group would meet the same problem.
    fn run_group(&self, group: &[Item]) {
        let Some(inputs) = self.build_inputs(group) else {
            // Cancelled tickets are not failures: the next run picks them up.
            let failed = if self.live.cancelled() {
                0
            } else {
                group.len() as u64
            };
            self.live.advance(failed, failed);
            return;
        };
        let mut rows: Vec<(String, StoredAssessment)> = Vec::new();
        let mut pending: Vec<usize> = (0..group.len()).collect();
        let mut errored_before = false;
        for _ in 0..2 {
            if pending.is_empty() || self.live.cancelled() {
                break;
            }
            // A retry renumbers its tickets: it is a group of its own.
            let batch: Vec<TicketInput> = pending
                .iter()
                .enumerate()
                .map(|(number, &index)| TicketInput {
                    key: format!("t{}", number + 1),
                    ..inputs[index].clone()
                })
                .collect();
            let origin: HashMap<&str, usize> = batch
                .iter()
                .zip(&pending)
                .map(|(input, &index)| (input.key.as_str(), index))
                .collect();
            match self.ask(&batch) {
                Ok(parsed) => {
                    for (key, criteria, reasons) in parsed.ok {
                        rows.push((
                            group[origin[key.as_str()]].0.id.clone(),
                            StoredAssessment {
                                criteria,
                                reasons,
                                rubric_version: rubric::RUBRIC_VERSION,
                                model: self.model.clone(),
                                runner: self.runner.to_string(),
                                assessed_at: self.assessed_at.clone(),
                            },
                        ));
                    }
                    pending = parsed
                        .failed
                        .iter()
                        .map(|key| origin[key.as_str()])
                        .collect();
                }
                Err(_) if self.live.cancelled() => break,
                Err(error) if errored_before => {
                    self.fail(error);
                    break;
                }
                Err(_) => errored_before = true,
            }
        }

        let written = rows.len() as u64;
        if let Err(error) = self.write(&rows) {
            self.fail(error);
            self.live.advance(group.len() as u64, group.len() as u64);
        } else if self.live.cancelled() {
            // Unfinished tickets are not failures: the next run picks them up.
            self.live.advance(written, 0);
        } else {
            self.live.advance(group.len() as u64, pending.len() as u64);
        }
    }

    fn write(&self, rows: &[(String, StoredAssessment)]) -> Result<(), String> {
        if rows.is_empty() {
            return Ok(());
        }
        let conn = self.store.lock_conn()?;
        let tx = conn.unchecked_transaction().map_err(sql)?;
        ticket_store::put_assessments(&tx, rows)?;
        tx.commit().map_err(sql)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contributors::ticket_store::TicketAssignee;
    use serde_json::{json, Value};
    use std::collections::HashSet;
    use std::sync::atomic::AtomicUsize;
    use std::sync::{Condvar, OnceLock};

    // ---- fakes ----

    struct FakeHttp {
        tasks: Mutex<Vec<Value>>,
        fetch_error: Mutex<Option<String>>,
        /// Answers of `GET /task/{id}`; an id without one is "Task not found".
        details: Mutex<HashMap<String, Result<Value, String>>>,
        comment: Mutex<String>,
        paths: Mutex<Vec<String>>,
        time_calls: AtomicUsize,
        time_fails: AtomicBool,
        /// Whether the time-in-status answer carries `current_status.total_time.since`
        /// (the ClickApp is on) or nulls (it is off).
        history_on: AtomicBool,
        /// Task ids starting with this get no entry in the time-in-status
        /// answer (an answer with none at all is the live `{}`).
        history_off_prefix: Mutex<Option<String>>,
        /// Listing per workspace id; other workspaces get `tasks`.
        team_tasks: Mutex<HashMap<String, Vec<Value>>>,
        /// The next this many comment / time-in-status / task-detail calls
        /// fail with the ClickUp rate-limit error.
        comment_limited: AtomicUsize,
        time_limited: AtomicUsize,
        detail_limited: AtomicUsize,
        /// While set, a listing call waits, so a fetch stays running.
        hold: AtomicBool,
    }

    /// Takes one from `left` and says whether there was one to take.
    fn take_one(left: &AtomicUsize) -> bool {
        // A compare-exchange loop rather than `fetch_update`, which newer
        // toolchains deprecate in favour of a method older ones lack.
        let mut n = left.load(Ordering::SeqCst);
        loop {
            if n == 0 {
                return false;
            }
            match left.compare_exchange(n, n - 1, Ordering::SeqCst, Ordering::SeqCst) {
                Ok(_) => return true,
                Err(current) => n = current,
            }
        }
    }

    impl FakeHttp {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                tasks: Mutex::new(Vec::new()),
                fetch_error: Mutex::new(None),
                details: Mutex::new(HashMap::new()),
                comment: Mutex::new("Looks fine to me".to_string()),
                paths: Mutex::new(Vec::new()),
                time_calls: AtomicUsize::new(0),
                time_fails: AtomicBool::new(false),
                history_on: AtomicBool::new(true),
                history_off_prefix: Mutex::new(None),
                team_tasks: Mutex::new(HashMap::new()),
                comment_limited: AtomicUsize::new(0),
                time_limited: AtomicUsize::new(0),
                detail_limited: AtomicUsize::new(0),
                hold: AtomicBool::new(false),
            })
        }
    }

    struct SharedHttp(Arc<FakeHttp>);

    impl ClickUpHttp for SharedHttp {
        fn get(&self, path: &str) -> Result<Value, String> {
            let fake = &self.0;
            lock(&fake.paths).push(path.to_string());
            if let Some(query) = path.strip_prefix("/task/bulk_time_in_status/task_ids?") {
                fake.time_calls.fetch_add(1, Ordering::SeqCst);
                if take_one(&fake.time_limited) {
                    return Err(crate::clickup::RATE_LIMITED.into());
                }
                if fake.time_fails.load(Ordering::SeqCst) {
                    return Err("time in status is unavailable".into());
                }
                let history_on = fake.history_on.load(Ordering::SeqCst);
                let off_prefix = lock(&fake.history_off_prefix).clone();
                let mut by_task = serde_json::Map::new();
                for id in query
                    .split('&')
                    .filter_map(|pair| pair.strip_prefix("task_ids="))
                {
                    if off_prefix.as_deref().is_some_and(|p| id.starts_with(p)) {
                        continue;
                    }
                    by_task.insert(
                        id.to_string(),
                        json!({
                            "current_status": if history_on {
                                json!({ "status": "x", "total_time": { "by_minute": 5, "since": SINCE_MS } })
                            } else {
                                json!({ "status": null, "total_time": null })
                            },
                            "status_history": [
                                { "type": "custom", "total_time": { "by_minute": 90 } }
                            ]
                        }),
                    );
                }
                Ok(Value::Object(by_task))
            } else if path.ends_with("/comment") {
                if take_one(&fake.comment_limited) {
                    return Err(crate::clickup::RATE_LIMITED.into());
                }
                Ok(json!({ "comments": [{ "comment_text": lock(&fake.comment).clone() }] }))
            } else if path.starts_with("/team/") {
                let deadline = Instant::now() + Duration::from_secs(20);
                while fake.hold.load(Ordering::SeqCst) && Instant::now() < deadline {
                    std::thread::sleep(Duration::from_millis(5));
                }
                if let Some(error) = lock(&fake.fetch_error).clone() {
                    return Err(error);
                }
                // The listing is one page, like a short result.
                let team = path
                    .strip_prefix("/team/")
                    .and_then(|rest| rest.split('/').next())
                    .unwrap_or_default();
                let tasks = if path.contains("&page=0") {
                    lock(&fake.team_tasks)
                        .get(team)
                        .cloned()
                        .unwrap_or_else(|| lock(&fake.tasks).clone())
                } else {
                    Vec::new()
                };
                Ok(json!({ "tasks": tasks }))
            } else if let Some(id) = path.strip_prefix("/task/") {
                if take_one(&fake.detail_limited) {
                    return Err(crate::clickup::RATE_LIMITED.into());
                }
                lock(&fake.details)
                    .get(id)
                    .cloned()
                    .unwrap_or_else(|| Err("Task not found".to_string()))
            } else {
                Err(format!("no fixture for {path}"))
            }
        }
    }

    /// Answers every ticket it is shown, except the titles it was told to
    /// garble (a number of times each), and records every prompt.
    struct Scripted {
        calls: AtomicUsize,
        systems: Mutex<Vec<String>>,
        users: Mutex<Vec<String>>,
        garble: Mutex<HashMap<String, usize>>,
        fail_all: AtomicBool,
        /// Every call after the first one fails.
        fail_after_first_call: AtomicBool,
        /// While set, the first call to return cancels the job (through
        /// `manager`) and the others wait for that.
        cancel_after_first: AtomicBool,
        manager: OnceLock<TicketJobManager>,
        first_done: AtomicBool,
    }

    impl Scripted {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                calls: AtomicUsize::new(0),
                systems: Mutex::new(Vec::new()),
                users: Mutex::new(Vec::new()),
                garble: Mutex::new(HashMap::new()),
                fail_all: AtomicBool::new(false),
                fail_after_first_call: AtomicBool::new(false),
                cancel_after_first: AtomicBool::new(false),
                manager: OnceLock::new(),
                first_done: AtomicBool::new(false),
            })
        }

        fn garble(&self, title: &str, times: usize) {
            lock(&self.garble).insert(title.to_string(), times);
        }

        /// The titles of every ticket shown, one list per call.
        fn titles_per_call(&self) -> Vec<Vec<String>> {
            lock(&self.users)
                .iter()
                .map(|user| titles_of(user))
                .collect()
        }

        fn reply(&self, user: &str) -> String {
            let mut assessments = Vec::new();
            for ticket in tickets_of(user) {
                let title = ticket["title"].as_str().unwrap_or_default().to_string();
                let mut garble = lock(&self.garble);
                if let Some(left) = garble.get_mut(&title) {
                    if *left > 0 {
                        *left -= 1;
                        continue;
                    }
                }
                assessments.push(json!({
                    "key": ticket["key"],
                    "criteria": {
                        "ambiguity": 1, "investigation": 0, "scope": 2, "risk": 1, "coordination": 0
                    },
                    "reasons": {
                        "ambiguity": "a", "investigation": "i", "scope": "s",
                        "risk": "r", "coordination": "c"
                    }
                }));
            }
            json!({ "assessments": assessments }).to_string()
        }
    }

    fn tickets_of(user: &str) -> Vec<Value> {
        let body = user.strip_prefix("Assess these tickets:\n").unwrap();
        serde_json::from_str(body).unwrap()
    }

    fn titles_of(user: &str) -> Vec<String> {
        tickets_of(user)
            .iter()
            .map(|ticket| ticket["title"].as_str().unwrap().to_string())
            .collect()
    }

    struct SharedAssessor(Arc<Scripted>);

    impl Assessor for SharedAssessor {
        fn assess(
            &self,
            system: &str,
            user: &str,
            cancelled: &dyn Fn() -> bool,
        ) -> Result<String, String> {
            let fake = &self.0;
            let number = fake.calls.fetch_add(1, Ordering::SeqCst);
            lock(&fake.systems).push(system.to_string());
            lock(&fake.users).push(user.to_string());
            if fake.fail_all.load(Ordering::SeqCst)
                || (number > 0 && fake.fail_after_first_call.load(Ordering::SeqCst))
            {
                return Err("The Anthropic API key was rejected.".into());
            }
            if fake.cancel_after_first.load(Ordering::SeqCst) {
                if number == 0 {
                    let reply = fake.reply(user);
                    fake.manager.get().unwrap().cancel();
                    fake.first_done.store(true, Ordering::SeqCst);
                    return Ok(reply);
                }
                while !fake.first_done.load(Ordering::SeqCst) {
                    std::thread::sleep(Duration::from_millis(1));
                }
                if cancelled() {
                    return Err("Assessment cancelled".into());
                }
            }
            Ok(fake.reply(user))
        }

        fn label(&self) -> (&'static str, String) {
            (RUNNER_API, "test-model".to_string())
        }
    }

    struct Deps {
        store: SessionStore,
        http: Arc<FakeHttp>,
        assessor: Arc<Scripted>,
    }

    impl TicketDeps for Deps {
        fn store(&self) -> &SessionStore {
            &self.store
        }

        fn http(&self) -> Result<Box<dyn ClickUpHttp>, String> {
            Ok(Box::new(SharedHttp(self.http.clone())))
        }

        fn assessor(&self) -> Result<Box<dyn Assessor>, String> {
            Ok(Box::new(SharedAssessor(self.assessor.clone())))
        }

        fn rate_wait(&self) -> RateWait {
            RateWait {
                pause: Duration::from_millis(1),
                retries: RETRIES,
            }
        }
    }

    /// `current_status.total_time.since` the fake reports, and its ISO form.
    const SINCE_MS: &str = "1700001000000";
    const SINCE_ISO: &str = "2023-11-14T22:30:00.000Z";

    /// Retries a rate-limited call gets in these tests.
    const RETRIES: u32 = 5;

    #[derive(Default)]
    struct RecordingSink {
        events: Mutex<Vec<TicketJobEvent>>,
        changed: Condvar,
    }

    impl TicketSink for RecordingSink {
        fn emit(&self, event: &TicketJobEvent) {
            lock(&self.events).push(event.clone());
            self.changed.notify_all();
        }
    }

    impl RecordingSink {
        /// The `count`th event that ends a job.
        fn wait_for_end(&self, count: usize) -> TicketJobEvent {
            let deadline = Instant::now() + Duration::from_secs(20);
            let mut events = lock(&self.events);
            loop {
                let ended: Vec<&TicketJobEvent> = events
                    .iter()
                    .filter(|e| [DONE, ERROR, CANCELLED].contains(&e.state.as_str()))
                    .collect();
                if let Some(event) = ended.get(count - 1) {
                    return (*event).clone();
                }
                let left = deadline
                    .checked_duration_since(Instant::now())
                    .expect("the job did not end in time");
                events = self.changed.wait_timeout(events, left).unwrap().0;
            }
        }

        fn states(&self) -> Vec<String> {
            lock(&self.events).iter().map(|e| e.state.clone()).collect()
        }
    }

    struct Harness {
        manager: TicketJobManager,
        deps: Arc<Deps>,
        sink: Arc<RecordingSink>,
    }

    fn harness_with(assessor: Arc<Scripted>) -> Harness {
        let deps = Arc::new(Deps {
            store: SessionStore::open_in_memory().unwrap(),
            http: FakeHttp::new(),
            assessor,
        });
        let sink = Arc::new(RecordingSink::default());
        let manager = TicketJobManager::new(deps.clone(), sink.clone());
        Harness {
            manager,
            deps,
            sink,
        }
    }

    fn harness() -> Harness {
        harness_with(Scripted::new())
    }

    // ---- data ----

    fn ticket(id: &str, title: &str) -> StoredTicket {
        StoredTicket {
            source: "clickup".to_string(),
            id: id.to_string(),
            custom_id: Some(format!("CU-{id}")),
            title: title.to_string(),
            url: format!("https://app.clickup.com/t/{id}"),
            workspace_id: "w1".to_string(),
            space_id: "s1".to_string(),
            status: "complete".to_string(),
            status_type: "closed".to_string(),
            updated_at: "2023-11-14T22:15:23.000Z".to_string(),
            updated_epoch: 1_700_000_123,
            done_at: Some("2023-11-14T22:15:23.000Z".to_string()),
            status_since: None,
            parent_id: None,
            assignees: vec![TicketAssignee {
                id: "u-4242".to_string(),
                name: "Alice Example".to_string(),
                email: "alice.example@corp.test".to_string(),
            }],
            tags: vec!["bug".to_string()],
            description: "Fix the login flow, reported by Alice Example (ALICE.EXAMPLE@corp.test)"
                .to_string(),
        }
    }

    /// `count` tickets titled `Title 0`..`Title n`, ids `t0`..`tn`.
    fn seed(deps: &Deps, count: usize) -> Vec<String> {
        let tickets: Vec<StoredTicket> = (0..count)
            .map(|n| ticket(&format!("t{n}"), &format!("Title {n}")))
            .collect();
        let conn = deps.store.lock_conn().unwrap();
        let tx = conn.unchecked_transaction().unwrap();
        ticket_store::put_tickets(&tx, &tickets).unwrap();
        tx.commit().unwrap();
        tickets.into_iter().map(|t| t.id).collect()
    }

    fn records(deps: &Deps) -> Vec<ticket_store::TicketRecord> {
        deps.store
            .with_read_conn(|conn| ticket_store::load_records(conn, &["w1".to_string()], 0))
            .unwrap()
    }

    fn assessed(deps: &Deps) -> usize {
        records(deps)
            .iter()
            .filter(|record| record.assessment.is_some())
            .count()
    }

    fn done_task(id: &str, date_done_ms: &str) -> Value {
        json!({
            "id": id,
            "name": format!("Task {id}"),
            "url": format!("https://app.clickup.com/t/{id}"),
            "space": { "id": "s1" },
            "status": { "status": "complete", "type": "closed" },
            "date_updated": "1700000500000",
            "date_done": date_done_ms,
            "assignees": [],
            "tags": []
        })
    }

    fn link() -> TicketLink {
        TicketLink {
            workspace_id: "w1".to_string(),
            space_ids: None,
        }
    }

    const FROM: i64 = 1_700_000_000;

    fn fetch_job() -> TicketJob {
        TicketJob::Fetch {
            links: vec![link()],
            from: FROM,
        }
    }

    /// A task in a working status, updated inside the window.
    fn working_task(id: &str, status: &str, status_type: &str) -> Value {
        let mut node = done_task(id, "1700000123000");
        node["status"] = json!({ "status": status, "type": status_type });
        node["date_done"] = Value::Null;
        node
    }

    fn stored(h: &Harness, id: &str) -> ticket_store::TicketRecord {
        records(&h.deps)
            .into_iter()
            .find(|record| record.id == id)
            .unwrap_or_else(|| panic!("{id} is not stored"))
    }

    fn assess_job(ids: Vec<String>, force: bool) -> TicketJob {
        TicketJob::Assess { ids, force }
    }

    // ---- tests ----

    #[test]
    fn fetch_job_stores_tickets_and_reports_done() {
        let h = harness();
        *lock(&h.deps.http.tasks) = vec![
            done_task("a", "1700000123000"),
            done_task("b", "1700000456000"),
        ];
        h.manager.start(fetch_job()).unwrap();
        let end = h.sink.wait_for_end(1);

        assert_eq!(end.kind, "fetch");
        assert_eq!(end.state, "done");
        assert_eq!((end.done, end.total, end.failed), (1, 1, 0));
        assert_eq!(end.error, None);
        assert_eq!(h.sink.states(), vec!["queued", "running", "done"]);
        let ids: Vec<String> = records(&h.deps).into_iter().map(|r| r.id).collect();
        assert_eq!(ids, vec!["a", "b"]);
        // The API compares strictly, so the first second of the window is
        // reached by asking one millisecond earlier. There is no upper bound.
        let paths = lock(&h.deps.http.paths).clone();
        assert!(
            paths[0].contains("date_updated_gt=1699999999999"),
            "{}",
            paths[0]
        );
        assert!(!paths[0].contains("date_done"), "{}", paths[0]);
        assert!(!paths[0].contains("date_updated_lt"), "{}", paths[0]);
    }

    #[test]
    fn fetch_job_stores_every_status_and_dates_the_ones_still_in_progress() {
        let h = harness();
        *lock(&h.deps.http.tasks) = vec![
            done_task("shipped", "1700000123000"),
            working_task("testing", "ready for casium testing", "custom"),
            working_task("backlog", "backlog", "open"),
        ];
        h.manager.start(fetch_job()).unwrap();
        let end = h.sink.wait_for_end(1);
        assert_eq!(end.state, "done");

        let shipped = stored(&h, "shipped");
        assert_eq!(shipped.done_at.as_deref(), Some("2023-11-14T22:15:23.000Z"));
        assert_eq!(shipped.status_since, None);
        let testing = stored(&h, "testing");
        assert_eq!(testing.status, "ready for casium testing");
        assert_eq!(testing.status_type, "custom");
        assert_eq!(testing.done_at, None);
        assert_eq!(testing.status_since.as_deref(), Some(SINCE_ISO));
        assert_eq!(
            stored(&h, "backlog").status_since.as_deref(),
            Some(SINCE_ISO)
        );
        // One call, and only for the tickets that are not done or closed.
        let paths = lock(&h.deps.http.paths).clone();
        let history: Vec<&String> = paths
            .iter()
            .filter(|path| path.starts_with("/task/bulk_time_in_status"))
            .collect();
        assert_eq!(history.len(), 1);
        assert!(history[0].contains("task_ids=testing"));
        assert!(history[0].contains("task_ids=backlog"));
        assert!(!history[0].contains("task_ids=shipped"));
    }

    #[test]
    fn fetch_job_never_asks_for_history_when_nothing_is_in_progress() {
        let h = harness();
        *lock(&h.deps.http.tasks) = vec![done_task("a", "1700000123000")];
        h.manager.start(fetch_job()).unwrap();
        h.sink.wait_for_end(1);
        assert_eq!(h.deps.http.time_calls.load(Ordering::SeqCst), 0);
    }

    fn many_working_tasks(count: usize) -> Vec<Value> {
        (0..count)
            .map(|n| working_task(&format!("w{n}"), "in progress", "custom"))
            .collect()
    }

    #[test]
    fn fetch_job_stops_asking_for_history_after_an_empty_first_answer() {
        let h = harness();
        h.deps.http.history_on.store(false, Ordering::SeqCst);
        // 250 tickets are three calls of 100 ids when the history works.
        *lock(&h.deps.http.tasks) = many_working_tasks(250);
        h.manager.start(fetch_job()).unwrap();
        let end = h.sink.wait_for_end(1);

        assert_eq!(end.state, "done");
        assert_eq!(h.deps.http.time_calls.load(Ordering::SeqCst), 1);
        assert_eq!(records(&h.deps).len(), 250);
        assert_eq!(stored(&h, "w0").status_since, None);
    }

    #[test]
    fn fetch_job_asks_for_history_in_batches_of_one_hundred() {
        let h = harness();
        *lock(&h.deps.http.tasks) = many_working_tasks(250);
        h.manager.start(fetch_job()).unwrap();
        h.sink.wait_for_end(1);

        assert_eq!(h.deps.http.time_calls.load(Ordering::SeqCst), 3);
        assert_eq!(stored(&h, "w249").status_since.as_deref(), Some(SINCE_ISO));
    }

    #[test]
    fn fetch_job_stops_asking_for_history_after_an_error_but_still_stores() {
        let h = harness();
        h.deps.http.time_fails.store(true, Ordering::SeqCst);
        *lock(&h.deps.http.tasks) = many_working_tasks(250);
        h.manager.start(fetch_job()).unwrap();
        let end = h.sink.wait_for_end(1);

        assert_eq!(end.state, "done");
        assert_eq!(end.error, None);
        assert_eq!(h.deps.http.time_calls.load(Ordering::SeqCst), 1);
        assert_eq!(records(&h.deps).len(), 250);
        assert_eq!(stored(&h, "w0").status_since, None);
    }

    #[test]
    fn fetch_job_treats_the_live_empty_body_as_history_unavailable() {
        let h = harness();
        // Measured live: HTTP 200 with `{}` when the ClickApp is off.
        *lock(&h.deps.http.history_off_prefix) = Some("w".to_string());
        *lock(&h.deps.http.tasks) = many_working_tasks(250);
        h.manager.start(fetch_job()).unwrap();
        let end = h.sink.wait_for_end(1);

        assert_eq!(end.state, "done");
        assert_eq!(h.deps.http.time_calls.load(Ordering::SeqCst), 1);
        assert_eq!(records(&h.deps).len(), 250);
        assert_eq!(stored(&h, "w0").status_since, None);
    }

    #[test]
    fn history_is_tracked_per_workspace_within_one_run() {
        let h = harness();
        // Workspace w1 has the ClickApp off (its ids get `{}`), w2 has it on.
        *lock(&h.deps.http.history_off_prefix) = Some("off".to_string());
        {
            let mut teams = lock(&h.deps.http.team_tasks);
            teams.insert(
                "w1".to_string(),
                vec![working_task("off1", "in progress", "custom")],
            );
            teams.insert(
                "w2".to_string(),
                vec![working_task("on1", "in progress", "custom")],
            );
        }
        let second = TicketLink {
            workspace_id: "w2".to_string(),
            space_ids: None,
        };
        h.manager
            .start(TicketJob::Fetch {
                links: vec![link(), second],
                from: FROM,
            })
            .unwrap();
        let end = h.sink.wait_for_end(1);

        assert_eq!(end.state, "done");
        assert_eq!(h.deps.http.time_calls.load(Ordering::SeqCst), 2);
        let both = h
            .deps
            .store
            .with_read_conn(|conn| {
                ticket_store::load_records(conn, &["w1".to_string(), "w2".to_string()], 0)
            })
            .unwrap();
        let since = |id: &str| {
            both.iter()
                .find(|record| record.id == id)
                .unwrap()
                .status_since
                .clone()
        };
        assert_eq!(since("off1"), None);
        assert_eq!(since("on1").as_deref(), Some(SINCE_ISO));
    }

    #[test]
    fn a_failed_history_call_keeps_the_stored_date_of_the_same_status() {
        let h = harness();
        *lock(&h.deps.http.tasks) = vec![
            working_task("same", "in progress", "custom"),
            working_task("moved", "in progress", "custom"),
        ];
        h.manager.start(fetch_job()).unwrap();
        h.sink.wait_for_end(1);
        assert_eq!(stored(&h, "same").status_since.as_deref(), Some(SINCE_ISO));

        // Next run the history call fails, and `moved` is in another status.
        h.deps.http.time_fails.store(true, Ordering::SeqCst);
        *lock(&h.deps.http.tasks) = vec![
            working_task("same", "in progress", "custom"),
            working_task("moved", "code review pr", "custom"),
        ];
        h.manager.start(fetch_job()).unwrap();
        let end = h.sink.wait_for_end(2);

        assert_eq!(end.state, "done");
        assert_eq!(stored(&h, "same").status_since.as_deref(), Some(SINCE_ISO));
        assert_eq!(stored(&h, "moved").status, "code review pr");
        assert_eq!(stored(&h, "moved").status_since, None);
    }

    #[test]
    fn an_empty_history_answer_keeps_the_stored_date_of_the_same_status() {
        let h = harness();
        *lock(&h.deps.http.tasks) = vec![working_task("a", "in progress", "custom")];
        h.manager.start(fetch_job()).unwrap();
        h.sink.wait_for_end(1);

        *lock(&h.deps.http.history_off_prefix) = Some("a".to_string());
        h.manager.start(fetch_job()).unwrap();
        h.sink.wait_for_end(2);
        assert_eq!(stored(&h, "a").status_since.as_deref(), Some(SINCE_ISO));
    }

    #[test]
    fn fetch_job_waits_out_a_rate_limit_on_the_history_call() {
        let h = harness();
        h.deps.http.time_limited.store(2, Ordering::SeqCst);
        *lock(&h.deps.http.tasks) = vec![working_task("a", "in progress", "custom")];
        h.manager.start(fetch_job()).unwrap();
        let end = h.sink.wait_for_end(1);

        assert_eq!(end.state, "done");
        assert_eq!(h.deps.http.time_calls.load(Ordering::SeqCst), 3);
        assert_eq!(stored(&h, "a").status_since.as_deref(), Some(SINCE_ISO));
    }

    #[test]
    fn fetch_job_stores_nothing_when_the_history_stays_rate_limited() {
        let h = harness();
        h.deps.http.time_limited.store(1000, Ordering::SeqCst);
        *lock(&h.deps.http.tasks) = vec![working_task("a", "in progress", "custom")];
        h.manager.start(fetch_job()).unwrap();
        let end = h.sink.wait_for_end(1);

        assert_eq!(end.state, "error");
        assert!(records(&h.deps).is_empty());
    }

    /// Fetches `a` and `b`, then fetches again with a listing of `a` only.
    fn refetch_without_b(h: &Harness) {
        *lock(&h.deps.http.tasks) = vec![
            done_task("a", "1700000123000"),
            done_task("b", "1700000456000"),
        ];
        h.manager.start(fetch_job()).unwrap();
        h.sink.wait_for_end(1);
        assert_eq!(records(&h.deps).len(), 2);

        *lock(&h.deps.http.tasks) = vec![done_task("a", "1700000123000")];
        h.manager.start(fetch_job()).unwrap();
        h.sink.wait_for_end(2);
    }

    fn stored_ids(h: &Harness) -> Vec<String> {
        records(&h.deps).into_iter().map(|r| r.id).collect()
    }

    #[test]
    fn refetch_keeps_tickets_that_moved_to_a_working_status() {
        let h = harness();
        // The listing leaves `b` out, but ClickUp still has it, now in a
        // working status: every status is kept, with its new state.
        let mut reopened = done_task("b", "1700000456000");
        reopened["status"] = json!({ "status": "in progress", "type": "custom" });
        reopened["date_done"] = Value::Null;
        lock(&h.deps.http.details).insert("b".to_string(), Ok(reopened));
        refetch_without_b(&h);
        assert_eq!(stored_ids(&h), vec!["a", "b"]);
        let b = stored(&h, "b");
        assert_eq!(b.status, "in progress");
        assert_eq!(b.done_at, None);
    }

    #[test]
    fn refetch_drops_tickets_clickup_no_longer_finds() {
        let h = harness();
        // No answer for `b`: the fake says "Task not found".
        refetch_without_b(&h);
        assert_eq!(stored_ids(&h), vec!["a"]);
    }

    #[test]
    fn refetch_keeps_tickets_missing_from_the_listing_but_still_there() {
        let h = harness();
        // Archived: the listing leaves `b` out, but ClickUp still has it.
        lock(&h.deps.http.details).insert("b".to_string(), Ok(done_task("b", "1800000000000")));
        refetch_without_b(&h);
        assert_eq!(stored_ids(&h), vec!["a", "b"]);
        let paths = lock(&h.deps.http.paths).clone();
        assert!(paths.contains(&"/task/b".to_string()));
    }

    #[test]
    fn refetch_keeps_tickets_when_the_lookup_fails_for_another_reason() {
        let h = harness();
        lock(&h.deps.http.details).insert(
            "b".to_string(),
            Err("ClickUp is rate limiting requests".to_string()),
        );
        refetch_without_b(&h);
        assert_eq!(stored_ids(&h), vec!["a", "b"]);
    }

    #[test]
    fn fetch_job_errors_when_a_link_fails_but_keeps_the_others() {
        let h = harness();
        *lock(&h.deps.http.tasks) = vec![done_task("a", "1700000123000")];
        h.manager.start(fetch_job()).unwrap();
        h.sink.wait_for_end(1);

        *lock(&h.deps.http.fetch_error) = Some("ClickUp is down".to_string());
        h.manager.start(fetch_job()).unwrap();
        let end = h.sink.wait_for_end(2);
        assert_eq!(end.state, "error");
        assert_eq!((end.done, end.total, end.failed), (1, 1, 1));
        assert!(end.error.unwrap().contains("ClickUp is down"));
        // A failed fetch never wipes what was stored before.
        assert_eq!(records(&h.deps).len(), 1);
    }

    #[test]
    fn assess_job_stores_every_ok_ticket_and_counts_failures() {
        let h = harness();
        let ids = seed(&h.deps, 5);
        h.deps.assessor.garble("Title 3", usize::MAX);
        h.manager.start(assess_job(ids, false)).unwrap();
        let end = h.sink.wait_for_end(1);

        assert_eq!(end.kind, "assess");
        assert_eq!(end.state, "done");
        assert_eq!((end.done, end.total, end.failed), (5, 5, 1));
        let records = records(&h.deps);
        assert_eq!(assessed(&h.deps), 4);
        let stored = records
            .iter()
            .find_map(|record| record.assessment.as_ref())
            .unwrap();
        assert_eq!(stored.runner, "api");
        assert_eq!(stored.model, "test-model");
        assert_eq!(stored.rubric_version, rubric::RUBRIC_VERSION);
        assert!(stored.assessed_at.ends_with('Z'));
        assert_eq!(stored.criteria.scope, 2);
        assert!(records
            .iter()
            .find(|record| record.id == "t3")
            .unwrap()
            .assessment
            .is_none());
    }

    #[test]
    fn assess_job_retries_failed_keys_once() {
        let h = harness();
        let ids = seed(&h.deps, 3);
        h.deps.assessor.garble("Title 1", 1);
        h.manager.start(assess_job(ids, false)).unwrap();
        let end = h.sink.wait_for_end(1);

        assert_eq!((end.done, end.total, end.failed), (3, 3, 0));
        assert_eq!(assessed(&h.deps), 3);
        let calls = h.deps.assessor.titles_per_call();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[1], vec!["Title 1"]);
        // The retry is a group of its own, renumbered from t1.
        assert!(lock(&h.deps.assessor.users)[1].contains("\"key\":\"t1\""));
    }

    #[test]
    fn assess_job_stops_with_an_error_when_the_ai_call_fails_twice() {
        let h = harness();
        let ids = seed(&h.deps, 3);
        h.deps.assessor.fail_all.store(true, Ordering::SeqCst);
        h.manager.start(assess_job(ids, false)).unwrap();
        let end = h.sink.wait_for_end(1);

        assert_eq!(end.state, "error");
        assert!(end.error.unwrap().contains("API key was rejected"));
        assert_eq!(h.deps.assessor.calls.load(Ordering::SeqCst), 2);
        assert_eq!(assessed(&h.deps), 0);
    }

    #[test]
    fn assess_job_continues_without_time_in_status_after_one_error() {
        let h = harness();
        let ids = seed(&h.deps, 45);
        h.deps.http.time_fails.store(true, Ordering::SeqCst);
        h.manager.start(assess_job(ids, false)).unwrap();
        let end = h.sink.wait_for_end(1);

        assert_eq!(end.state, "done");
        assert_eq!((end.done, end.total, end.failed), (45, 45, 0));
        assert_eq!(h.deps.http.time_calls.load(Ordering::SeqCst), 1);
        assert_eq!(assessed(&h.deps), 45);
        for user in lock(&h.deps.assessor.users).iter() {
            assert!(user.contains("\"hoursInProgress\":null"));
            assert!(!user.contains("\"hoursInProgress\":1.5"));
        }
    }

    #[test]
    fn assess_job_sends_hours_in_progress_when_available() {
        let h = harness();
        let ids = seed(&h.deps, 2);
        h.manager.start(assess_job(ids, false)).unwrap();
        h.sink.wait_for_end(1);
        assert!(lock(&h.deps.assessor.users)[0].contains("\"hoursInProgress\":1.5"));
        assert!(lock(&h.deps.assessor.users)[0].contains("Looks fine to me"));
    }

    #[test]
    fn cancel_keeps_finished_groups_and_next_run_sends_only_the_rest() {
        let h = harness();
        let assessor = &h.deps.assessor;
        assessor.manager.set(h.manager.clone()).ok().unwrap();
        assessor.cancel_after_first.store(true, Ordering::SeqCst);
        // Three groups: 20 + 20 + 10.
        let ids = seed(&h.deps, 50);
        h.manager.start(assess_job(ids.clone(), false)).unwrap();
        let end = h.sink.wait_for_end(1);

        assert_eq!(end.state, "cancelled");
        // Whichever group returned first is the one kept: 20 or 10 tickets.
        let kept_count = assessed(&h.deps);
        assert!(kept_count == 20 || kept_count == 10, "{kept_count}");
        assert_eq!((end.done, end.total), (kept_count as u64, 50));
        let kept: HashSet<String> = records(&h.deps)
            .into_iter()
            .filter(|record| record.assessment.is_some())
            .map(|record| record.title)
            .collect();

        // The next run sends only what is not assessed yet.
        assessor.cancel_after_first.store(false, Ordering::SeqCst);
        lock(&assessor.users).clear();
        h.manager.start(assess_job(ids, false)).unwrap();
        let end = h.sink.wait_for_end(2);

        let rest = 50 - kept_count;
        assert_eq!(
            (end.state.as_str(), end.done, end.total),
            ("done", rest as u64, rest as u64)
        );
        let sent: Vec<String> = assessor.titles_per_call().into_iter().flatten().collect();
        assert_eq!(sent.len(), rest);
        assert!(sent.iter().all(|title| !kept.contains(title)));
        assert_eq!(assessed(&h.deps), 50);
    }

    #[test]
    fn forced_reassess_skips_overridden_tickets() {
        let h = harness();
        let ids = seed(&h.deps, 3);
        // t0 is assessed, t1 has a manual level, t2 is fresh.
        h.manager
            .start(assess_job(vec!["t0".to_string()], false))
            .unwrap();
        h.sink.wait_for_end(1);
        {
            let conn = h.deps.store.lock_conn().unwrap();
            ticket_store::set_override(&conn, "t1", Some("L"), None).unwrap();
        }

        h.manager.start(assess_job(ids, true)).unwrap();
        let end = h.sink.wait_for_end(2);

        assert_eq!((end.done, end.total), (2, 2));
        let last = h.deps.assessor.titles_per_call().pop().unwrap();
        assert_eq!(last, vec!["Title 0", "Title 2"]);
        let records = records(&h.deps);
        assert!(records[1].assessment.is_none());
        assert_eq!(records[1].override_level.as_deref(), Some("L"));
    }

    #[test]
    fn assess_job_never_sends_assignee_data() {
        let h = harness();
        let ids = seed(&h.deps, 3);
        *lock(&h.deps.http.comment) =
            "Ask @alice example or Alice.Example@corp.test about it. Alice, can you check?"
                .to_string();
        h.manager.start(assess_job(ids, false)).unwrap();
        h.sink.wait_for_end(1);

        let mut sent = lock(&h.deps.assessor.users).clone();
        sent.extend(lock(&h.deps.assessor.systems).clone());
        assert!(!sent.is_empty());
        for text in sent {
            let lower = text.to_lowercase();
            for secret in ["alice", "example", "corp.test", "u-4242"] {
                assert!(!lower.contains(secret), "{secret} reached the AI");
            }
        }
        // The description and the comment were redacted, not dropped.
        let user = lock(&h.deps.assessor.users)[0].clone();
        assert!(user.contains("reported by [assignee] ([assignee])"));
        assert!(user.contains("Ask @[assignee] or [assignee] about it"));
        // A first name alone is hidden too.
        assert!(user.contains("[assignee], can you check?"));
    }

    #[test]
    fn hidden_words_include_name_parts_and_the_email_local_part() {
        let mut ticket = ticket("x", "T");
        ticket.assignees = vec![TicketAssignee {
            id: "u1".to_string(),
            name: "Ada Q. Lovelace".to_string(),
            email: "countess@example.com".to_string(),
        }];
        let words: HashSet<String> = hidden_words(&ticket)
            .into_iter()
            .map(|word| word.into_iter().collect())
            .collect();
        for expected in [
            "Ada Q. Lovelace",
            "countess@example.com",
            "Ada",
            "Lovelace",
            "countess",
        ] {
            assert!(words.contains(expected), "{expected} is not hidden");
        }
        // Parts shorter than three characters are left alone.
        assert!(!words.contains("Q."));
        assert!(!words.contains("Q"));
        // Longest first, so a full name goes before its parts.
        let hidden = hidden_words(&ticket);
        assert!(hidden.windows(2).all(|pair| pair[0].len() >= pair[1].len()));
    }

    #[test]
    fn a_short_rate_limit_is_waited_out_and_the_comments_still_reach_the_ai() {
        let h = harness();
        let ids = seed(&h.deps, 3);
        h.deps.http.comment_limited.store(3, Ordering::SeqCst);
        h.deps.http.time_limited.store(2, Ordering::SeqCst);
        h.manager.start(assess_job(ids, false)).unwrap();
        let end = h.sink.wait_for_end(1);

        assert_eq!(end.state, "done");
        assert_eq!((end.done, end.total, end.failed), (3, 3, 0));
        assert_eq!(assessed(&h.deps), 3);
        let users = lock(&h.deps.assessor.users).clone();
        assert!(!users.is_empty());
        for user in users {
            assert!(user.contains("Looks fine to me"));
            assert!(user.contains("\"hoursInProgress\":1.5"));
        }
    }

    #[test]
    fn rate_limited_comments_leave_the_group_unassessed_for_the_next_run() {
        let h = harness();
        let ids = seed(&h.deps, 3);
        h.deps
            .http
            .comment_limited
            .store(usize::MAX, Ordering::SeqCst);
        h.manager.start(assess_job(ids.clone(), false)).unwrap();
        let end = h.sink.wait_for_end(1);

        assert_eq!(end.state, "done");
        assert_eq!((end.done, end.total, end.failed), (3, 3, 3));
        // Nothing went to the AI and nothing was stored.
        assert_eq!(h.deps.assessor.calls.load(Ordering::SeqCst), 0);
        assert_eq!(assessed(&h.deps), 0);

        // Once ClickUp answers again the same tickets are assessed, with comments.
        h.deps.http.comment_limited.store(0, Ordering::SeqCst);
        h.manager.start(assess_job(ids, false)).unwrap();
        let end = h.sink.wait_for_end(2);
        assert_eq!((end.done, end.total, end.failed), (3, 3, 0));
        assert_eq!(assessed(&h.deps), 3);
        assert!(lock(&h.deps.assessor.users)[0].contains("Looks fine to me"));
    }

    #[test]
    fn a_persistent_rate_limit_on_time_in_status_fails_the_group_without_disabling_hours() {
        let h = harness();
        // Three groups: 20 + 20 + 5. The groups take the time-in-status call
        // one after the other, so the first one spends every limited call
        // (the first try and its retries) and fails; the others get hours.
        let ids = seed(&h.deps, 45);
        h.deps
            .http
            .time_limited
            .store(RETRIES as usize + 1, Ordering::SeqCst);
        h.manager.start(assess_job(ids, false)).unwrap();
        let end = h.sink.wait_for_end(1);

        assert_eq!(end.state, "done");
        assert!(end.failed == 20 || end.failed == 5, "{}", end.failed);
        assert_eq!(end.done, 45);
        assert_eq!(assessed(&h.deps) as u64, 45 - end.failed);
        // Hours were not switched off for the rest of the run.
        let users = lock(&h.deps.assessor.users).clone();
        assert!(!users.is_empty());
        for user in users {
            assert!(user.contains("\"hoursInProgress\":1.5"));
            assert!(!user.contains("\"hoursInProgress\":null"));
        }
    }

    #[test]
    fn a_rate_limit_waits_and_then_a_recheck_sees_the_moved_ticket() {
        let h = harness();
        let mut reopened = done_task("b", "1700000456000");
        reopened["status"] = json!({ "status": "in progress", "type": "custom" });
        reopened["date_done"] = Value::Null;
        lock(&h.deps.http.details).insert("b".to_string(), Ok(reopened));
        h.deps.http.detail_limited.store(0, Ordering::SeqCst);
        *lock(&h.deps.http.tasks) = vec![
            done_task("a", "1700000123000"),
            done_task("b", "1700000456000"),
        ];
        h.manager.start(fetch_job()).unwrap();
        h.sink.wait_for_end(1);

        *lock(&h.deps.http.tasks) = vec![done_task("a", "1700000123000")];
        h.deps.http.detail_limited.store(2, Ordering::SeqCst);
        h.manager.start(fetch_job()).unwrap();
        h.sink.wait_for_end(2);
        // The lookup was retried and then answered: the ticket is kept with
        // its new status.
        assert_eq!(stored_ids(&h), vec!["a", "b"]);
        assert_eq!(stored(&h, "b").status, "in progress");
        let lookups = lock(&h.deps.http.paths)
            .iter()
            .filter(|path| path.as_str() == "/task/b")
            .count();
        assert_eq!(lookups, 3);
    }

    #[test]
    fn a_recheck_that_stays_rate_limited_keeps_the_row() {
        let h = harness();
        // `b` was reopened, but ClickUp cannot be asked: the row stays.
        let mut reopened = done_task("b", "1700000456000");
        reopened["status"] = json!({ "status": "in progress", "type": "custom" });
        lock(&h.deps.http.details).insert("b".to_string(), Ok(reopened));
        *lock(&h.deps.http.tasks) = vec![
            done_task("a", "1700000123000"),
            done_task("b", "1700000456000"),
        ];
        h.manager.start(fetch_job()).unwrap();
        h.sink.wait_for_end(1);

        *lock(&h.deps.http.tasks) = vec![done_task("a", "1700000123000")];
        h.deps
            .http
            .detail_limited
            .store(usize::MAX, Ordering::SeqCst);
        h.manager.start(fetch_job()).unwrap();
        h.sink.wait_for_end(2);
        assert_eq!(stored_ids(&h), vec!["a", "b"]);
        let lookups = lock(&h.deps.http.paths)
            .iter()
            .filter(|path| path.as_str() == "/task/b")
            .count();
        assert_eq!(lookups, RETRIES as usize + 1);
    }

    #[test]
    fn a_rate_limit_wait_ends_when_the_job_is_cancelled() {
        let live = Live {
            cancel: AtomicBool::new(false),
            last: Mutex::new(Last {
                event: TicketJobEvent::new(KIND_ASSESS, RUNNING, 0),
                emitted_at: Instant::now(),
            }),
            sink: Arc::new(RecordingSink::default()),
        };
        let wait = RateWait {
            pause: Duration::from_secs(30),
            retries: 10,
        };
        let calls = AtomicUsize::new(0);
        let started = Instant::now();
        let result: Result<(), String> = std::thread::scope(|scope| {
            scope.spawn(|| {
                std::thread::sleep(Duration::from_millis(50));
                live.cancel.store(true, Ordering::Relaxed);
            });
            wait.run(&live, || {
                calls.fetch_add(1, Ordering::SeqCst);
                Err(crate::clickup::RATE_LIMITED.to_string())
            })
        });
        assert_eq!(result.unwrap_err(), crate::clickup::RATE_LIMITED);
        assert!(started.elapsed() < Duration::from_secs(5));
        // One try, the wait cut short by the cancel, and a last look.
        assert!(calls.load(Ordering::SeqCst) <= 2);
    }

    #[test]
    fn a_rate_limit_wait_does_not_hide_other_errors() {
        let live = Live {
            cancel: AtomicBool::new(false),
            last: Mutex::new(Last {
                event: TicketJobEvent::new(KIND_ASSESS, RUNNING, 0),
                emitted_at: Instant::now(),
            }),
            sink: Arc::new(RecordingSink::default()),
        };
        let wait = RateWait {
            pause: Duration::from_millis(1),
            retries: 3,
        };
        let calls = AtomicUsize::new(0);
        let result: Result<(), String> = wait.run(&live, || {
            calls.fetch_add(1, Ordering::SeqCst);
            Err("ClickUp could not find that item".to_string())
        });
        assert!(result.is_err());
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn redact_hides_whole_case_insensitive_occurrences_only() {
        let hidden = vec![
            "Ada Lovelace".chars().collect(),
            "ada@example.com".chars().collect(),
            "Ada".chars().collect(),
        ];
        let hidden = {
            let mut hidden: Vec<Vec<char>> = hidden;
            hidden.sort_by_key(|word| std::cmp::Reverse(word.len()));
            hidden
        };
        assert_eq!(
            redact(
                "@ADA LOVELACE, mail ada@example.com. Adams and Ada!",
                &hidden
            ),
            "@[assignee], mail [assignee]. Adams and [assignee]!"
        );
        assert_eq!(redact("nothing here", &[]), "nothing here");
    }

    #[test]
    fn a_failing_ai_call_after_a_good_one_does_not_stop_the_run() {
        // Attempt 0 answers but leaves a ticket out; attempt 1 is a failing
        // call: that is one error, not two in a row.
        let h = harness();
        let ids = seed(&h.deps, 3);
        h.deps.assessor.garble("Title 1", usize::MAX);
        h.deps
            .assessor
            .fail_after_first_call
            .store(true, Ordering::SeqCst);
        h.manager.start(assess_job(ids, false)).unwrap();
        let end = h.sink.wait_for_end(1);
        assert_eq!(end.state, "done");
        assert_eq!((end.done, end.total, end.failed), (3, 3, 1));
        assert_eq!(assessed(&h.deps), 2);
    }

    #[test]
    fn status_is_none_when_no_job_is_running_or_queued() {
        let h = harness();
        assert_eq!(h.manager.status_with_pending().event, None);
        let ids = seed(&h.deps, 2);
        h.manager.start(assess_job(ids, false)).unwrap();
        h.sink.wait_for_end(1);
        assert_eq!(h.manager.status_with_pending().event, None);
    }

    #[test]
    fn ticket_job_event_serializes_as_the_webview_expects() {
        let event = TicketJobEvent {
            kind: "assess".to_string(),
            state: "running".to_string(),
            done: 3,
            total: 10,
            failed: 1,
            error: None,
            job_id: 7,
            pending_fetches: 2,
        };
        assert_eq!(
            serde_json::to_value(&event).unwrap(),
            json!({
                "kind": "assess",
                "state": "running",
                "done": 3,
                "total": 10,
                "failed": 1,
                "error": null,
                "jobId": 7,
                "pendingFetches": 2
            })
        );
        let failed = TicketJobEvent {
            error: Some("boom".to_string()),
            ..event
        };
        assert_eq!(serde_json::to_value(&failed).unwrap()["error"], "boom");
    }

    #[test]
    fn status_serializes_with_the_pending_fetches_next_to_the_event() {
        let none = TicketJobStatus {
            event: None,
            pending_fetches: 0,
        };
        assert_eq!(
            serde_json::to_value(&none).unwrap(),
            json!({ "event": null, "pendingFetches": 0 })
        );
    }

    // ---- pending fetches and job ids ----

    /// A fetch of two workspaces: after the first one finishes, a cancel is
    /// seen before the second one starts.
    fn two_link_fetch() -> TicketJob {
        TicketJob::Fetch {
            links: vec![
                link(),
                TicketLink {
                    workspace_id: "w2".to_string(),
                    space_ids: None,
                },
            ],
            from: FROM,
        }
    }

    /// Waits until the job `id` is the running one.
    fn wait_running(h: &Harness, id: u64) {
        let deadline = Instant::now() + Duration::from_secs(20);
        let mut events = lock(&h.sink.events);
        while !events.iter().any(|e| e.job_id == id && e.state == RUNNING) {
            let left = deadline
                .checked_duration_since(Instant::now())
                .expect("the job did not start in time");
            events = h.sink.changed.wait_timeout(events, left).unwrap().0;
        }
    }

    fn pending_of(h: &Harness, id: u64, state: &str) -> u32 {
        lock(&h.sink.events)
            .iter()
            .find(|e| e.job_id == id && e.state == state)
            .unwrap_or_else(|| panic!("no {state} event for job {id}"))
            .pending_fetches
    }

    #[test]
    fn start_returns_increasing_ids_that_the_events_carry() {
        let h = harness();
        let first = h.manager.start(fetch_job()).unwrap();
        h.sink.wait_for_end(1);
        let second = h.manager.start(fetch_job()).unwrap();
        h.sink.wait_for_end(2);
        assert!(second > first);
        let events = lock(&h.sink.events).clone();
        assert!(events
            .iter()
            .filter(|e| e.job_id == first)
            .all(|e| e.kind == "fetch"));
        assert_eq!(events.iter().filter(|e| e.job_id == second).count(), 3);
    }

    #[test]
    fn pending_fetches_count_queued_and_running_fetch_jobs() {
        let h = harness();
        h.deps.http.hold.store(true, Ordering::SeqCst);
        assert_eq!(h.manager.status_with_pending().pending_fetches, 0);
        let first = h.manager.start(fetch_job()).unwrap();
        wait_running(&h, first);
        let second = h.manager.start(fetch_job()).unwrap();
        assert_eq!(h.manager.status_with_pending().pending_fetches, 2);

        h.deps.http.hold.store(false, Ordering::SeqCst);
        h.sink.wait_for_end(2);
        assert_eq!(pending_of(&h, first, QUEUED), 1);
        assert_eq!(pending_of(&h, second, QUEUED), 2);
        // The first one ended while the second was still pending.
        assert_eq!(pending_of(&h, first, DONE), 1);
        assert_eq!(pending_of(&h, second, DONE), 0);
        assert_eq!(h.manager.status_with_pending().pending_fetches, 0);
    }

    #[test]
    fn a_fetch_queued_behind_an_assessment_is_pending_and_the_assessment_is_not() {
        let h = harness();
        let ids = seed(&h.deps, 2);
        // The fetch ahead keeps the worker busy while the others queue.
        h.deps.http.hold.store(true, Ordering::SeqCst);
        let ahead = h.manager.start(fetch_job()).unwrap();
        wait_running(&h, ahead);
        let assess = h.manager.start(assess_job(ids, false)).unwrap();
        let behind = h.manager.start(fetch_job()).unwrap();
        assert_eq!(pending_of(&h, assess, QUEUED), 1);
        assert_eq!(pending_of(&h, behind, QUEUED), 2);

        h.deps.http.hold.store(false, Ordering::SeqCst);
        h.sink.wait_for_end(3);
        // Once the fetch ahead is over, the queued fetch is still pending while
        // the assessment runs and ends.
        assert_eq!(pending_of(&h, ahead, DONE), 1);
        assert_eq!(pending_of(&h, assess, DONE), 1);
        assert_eq!(pending_of(&h, behind, DONE), 0);
    }

    #[test]
    fn status_reports_pending_fetches_while_the_front_job_is_an_assessment() {
        let h = harness();
        let ids = seed(&h.deps, 2);
        h.deps.http.hold.store(true, Ordering::SeqCst);
        let ahead = h.manager.start(fetch_job()).unwrap();
        wait_running(&h, ahead);
        h.manager.start(assess_job(ids, false)).unwrap();
        h.manager.start(fetch_job()).unwrap();
        // A page opened now sees the running fetch, and both fetches pending.
        let status = h.manager.status_with_pending();
        assert_eq!(
            status.event.as_ref().map(|e| e.kind.as_str()),
            Some("fetch")
        );
        assert_eq!(status.pending_fetches, 2);
        assert_eq!(status.event.unwrap().pending_fetches, 2);
        h.deps.http.hold.store(false, Ordering::SeqCst);
        h.sink.wait_for_end(3);
    }

    #[test]
    fn cancel_ends_every_dropped_fetch_with_its_own_event_and_count() {
        let h = harness();
        h.deps.http.hold.store(true, Ordering::SeqCst);
        let running = h.manager.start(two_link_fetch()).unwrap();
        wait_running(&h, running);
        let second = h.manager.start(fetch_job()).unwrap();
        let third = h.manager.start(fetch_job()).unwrap();

        h.manager.cancel();
        h.deps.http.hold.store(false, Ordering::SeqCst);
        h.sink.wait_for_end(3);

        let cancelled: Vec<(u64, u32)> = lock(&h.sink.events)
            .iter()
            .filter(|e| e.state == CANCELLED)
            .map(|e| (e.job_id, e.pending_fetches))
            .collect();
        assert_eq!(cancelled, vec![(second, 2), (third, 1), (running, 0)]);
        assert_eq!(h.manager.status_with_pending().pending_fetches, 0);
    }

    #[test]
    fn a_failed_fetch_job_is_no_longer_pending() {
        let h = harness();
        *lock(&h.deps.http.fetch_error) = Some("ClickUp is down".to_string());
        let id = h.manager.start(fetch_job()).unwrap();
        let end = h.sink.wait_for_end(1);
        assert_eq!((end.state.as_str(), end.job_id), ("error", id));
        assert_eq!(end.pending_fetches, 0);
        assert_eq!(h.manager.status_with_pending().pending_fetches, 0);
    }

    // ---- coverage ----

    fn interval_of(h: &Harness, workspace: &str, spaces: &str) -> Option<(i64, i64)> {
        h.deps
            .store
            .with_read_conn(|conn| {
                conn.query_row(
                    "SELECT covered_since, covered_until FROM contrib_ticket_coverage
                     WHERE workspace_id = ?1 AND spaces_key = ?2",
                    rusqlite::params![workspace, spaces],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .map(Some)
                .or_else(|error| match error {
                    rusqlite::Error::QueryReturnedNoRows => Ok(None),
                    other => Err(sql(other)),
                })
            })
            .unwrap()
    }

    /// Records an interval as an earlier fetch would have left it.
    fn preset(h: &Harness, spaces: &str, from: i64, until: i64) {
        let conn = h.deps.store.lock_conn().unwrap();
        let tx = conn.unchecked_transaction().unwrap();
        ticket_store::mark_covered(&tx, "w1", spaces, from, until, until).unwrap();
        tx.commit().unwrap();
    }

    #[test]
    fn a_clean_fetch_covers_its_spaces_from_the_start_until_the_listing_began() {
        let h = harness();
        assert_eq!(interval_of(&h, "w1", "*"), None);
        *lock(&h.deps.http.tasks) = vec![done_task("a", "1700000123000")];
        let before = now_epoch();
        h.manager.start(fetch_job()).unwrap();
        h.sink.wait_for_end(1);
        let after = now_epoch();
        let (since, until) = interval_of(&h, "w1", "*").unwrap();
        assert_eq!(since, FROM);
        assert!((before..=after).contains(&until), "{until}");
        assert_eq!(interval_of(&h, "w2", "*"), None);
    }

    #[test]
    fn a_fetch_that_reaches_back_into_the_covered_range_extends_it() {
        let h = harness();
        preset(&h, "*", FROM - 10_000, FROM + 100);
        h.manager.start(fetch_job()).unwrap();
        h.sink.wait_for_end(1);
        let (since, until) = interval_of(&h, "w1", "*").unwrap();
        assert_eq!(since, FROM - 10_000);
        assert!(until >= now_epoch() - 60, "{until}");
        // A fetch from earlier still extends backwards.
        h.manager
            .start(TicketJob::Fetch {
                links: vec![link()],
                from: FROM - 20_000,
            })
            .unwrap();
        h.sink.wait_for_end(2);
        assert_eq!(interval_of(&h, "w1", "*").unwrap().0, FROM - 20_000);
    }

    #[test]
    fn a_fetch_after_a_gap_replaces_the_covered_range() {
        let h = harness();
        // Covered up to FROM + 100; this fetch starts at FROM + 1000, so
        // FROM + 101 .. FROM + 999 was never fetched.
        preset(&h, "*", FROM - 10_000, FROM + 100);
        h.manager
            .start(TicketJob::Fetch {
                links: vec![link()],
                from: FROM + 1_000,
            })
            .unwrap();
        h.sink.wait_for_end(1);
        let (since, _) = interval_of(&h, "w1", "*").unwrap();
        assert_eq!(since, FROM + 1_000);
    }

    #[test]
    fn a_fetch_of_narrowed_spaces_covers_only_that_set() {
        let h = harness();
        h.manager
            .start(TicketJob::Fetch {
                links: vec![TicketLink {
                    workspace_id: "w1".to_string(),
                    space_ids: Some(vec!["s2".to_string(), "s1".to_string()]),
                }],
                from: FROM,
            })
            .unwrap();
        h.sink.wait_for_end(1);
        assert_eq!(interval_of(&h, "w1", "*"), None);
        assert_eq!(interval_of(&h, "w1", "s1"), None);
        assert_eq!(interval_of(&h, "w1", "s1,s2").unwrap().0, FROM);
    }

    #[test]
    fn a_fetch_with_an_empty_space_list_covers_every_space() {
        let h = harness();
        h.manager
            .start(TicketJob::Fetch {
                links: vec![TicketLink {
                    workspace_id: "w1".to_string(),
                    space_ids: Some(vec![]),
                }],
                from: FROM,
            })
            .unwrap();
        h.sink.wait_for_end(1);
        assert_eq!(interval_of(&h, "w1", "*").unwrap().0, FROM);
    }

    #[test]
    fn a_failed_fetch_leaves_the_coverage_alone() {
        let h = harness();
        *lock(&h.deps.http.fetch_error) = Some("ClickUp is down".to_string());
        h.manager.start(fetch_job()).unwrap();
        assert_eq!(h.sink.wait_for_end(1).state, "error");
        assert_eq!(interval_of(&h, "w1", "*"), None);

        // A failure after a covered range does not change it either.
        preset(&h, "*", FROM, FROM + 100);
        h.manager
            .start(TicketJob::Fetch {
                links: vec![link()],
                from: FROM - 9_000,
            })
            .unwrap();
        assert_eq!(h.sink.wait_for_end(2).state, "error");
        assert_eq!(interval_of(&h, "w1", "*"), Some((FROM, FROM + 100)));
    }

    #[test]
    fn a_cancelled_fetch_leaves_the_coverage_alone() {
        let h = harness();
        h.deps.http.hold.store(true, Ordering::SeqCst);
        let id = h.manager.start(two_link_fetch()).unwrap();
        wait_running(&h, id);
        h.manager.cancel();
        h.deps.http.hold.store(false, Ordering::SeqCst);
        assert_eq!(h.sink.wait_for_end(1).state, "cancelled");
        // The first workspace finished before the cancel was seen; the second
        // never started.
        assert_eq!(interval_of(&h, "w1", "*").unwrap().0, FROM);
        assert_eq!(interval_of(&h, "w2", "*"), None);
    }

    #[test]
    fn only_the_links_that_worked_are_covered() {
        let h = harness();
        *lock(&h.deps.http.team_tasks) =
            HashMap::from([("w2".to_string(), vec![done_task("b", "1700000123000")])]);
        h.manager.start(two_link_fetch()).unwrap();
        h.sink.wait_for_end(1);
        assert_eq!(interval_of(&h, "w1", "*").unwrap().0, FROM);
        assert_eq!(interval_of(&h, "w2", "*").unwrap().0, FROM);
    }

    // ---- estimate ----

    fn put_all(store: &SessionStore, tickets: &[StoredTicket]) {
        let conn = store.lock_conn().unwrap();
        let tx = conn.unchecked_transaction().unwrap();
        ticket_store::put_tickets(&tx, tickets).unwrap();
        tx.commit().unwrap();
    }

    /// A ticket with exactly `chars` characters of text, none of it an assignee.
    fn plain(id: &str, chars: usize) -> StoredTicket {
        StoredTicket {
            assignees: vec![],
            tags: vec![],
            title: "x".repeat(chars),
            description: String::new(),
            ..ticket(id, "")
        }
    }

    fn prompt_tokens() -> u64 {
        tokens(rubric::system_prompt().chars().count())
    }

    fn estimate_of(store: &SessionStore, ids: &[String]) -> TicketEstimate {
        store.with_read_conn(|conn| estimate(conn, ids)).unwrap()
    }

    #[test]
    fn estimate_counts_only_unassessed_tickets() {
        let store = SessionStore::open_in_memory().unwrap();
        put_all(
            &store,
            &[plain("fresh", 40), plain("done", 400), plain("manual", 400)],
        );
        {
            let conn = store.lock_conn().unwrap();
            let tx = conn.unchecked_transaction().unwrap();
            let row = StoredAssessment {
                criteria: ticket_store::Criteria {
                    ambiguity: 1,
                    investigation: 1,
                    scope: 1,
                    risk: 1,
                    coordination: 1,
                },
                reasons: ticket_store::Reasons {
                    ambiguity: "a".to_string(),
                    investigation: "i".to_string(),
                    scope: "s".to_string(),
                    risk: "r".to_string(),
                    coordination: "c".to_string(),
                },
                rubric_version: rubric::RUBRIC_VERSION,
                model: "m".to_string(),
                runner: "api".to_string(),
                assessed_at: "2023-11-14T22:15:23Z".to_string(),
            };
            ticket_store::put_assessments(&tx, &[("done".to_string(), row)]).unwrap();
            tx.commit().unwrap();
            ticket_store::set_override(&conn, "manual", Some("S"), None).unwrap();
        }
        let ids: Vec<String> = ["fresh", "done", "manual", "missing"]
            .map(String::from)
            .to_vec();
        let estimate = estimate_of(&store, &ids);
        assert_eq!(estimate.tickets, 1);
        assert_eq!(estimate.input_tokens, 10 + prompt_tokens());
    }

    #[test]
    fn estimate_adds_the_system_prompt_once_per_group_of_20() {
        let store = SessionStore::open_in_memory().unwrap();
        let tickets: Vec<StoredTicket> = (0..21).map(|n| plain(&format!("t{n}"), 8)).collect();
        put_all(&store, &tickets);
        let ids: Vec<String> = tickets.iter().map(|t| t.id.clone()).collect();
        let estimate = estimate_of(&store, &ids);
        assert_eq!(estimate.tickets, 21);
        assert_eq!(estimate.input_tokens, 21 * 2 + 2 * prompt_tokens());
        let one = estimate_of(&store, &ids[..20]);
        assert_eq!(one.input_tokens, 20 * 2 + prompt_tokens());
        let none = estimate_of(&store, &[]);
        assert_eq!((none.tickets, none.input_tokens), (0, 0));
    }

    #[test]
    fn estimate_uses_truncated_redacted_text() {
        let store = SessionStore::open_in_memory().unwrap();
        let long = StoredTicket {
            description: "y".repeat(10_000),
            ..plain("long", 0)
        };
        // "Alice Example" (13 chars) becomes "[assignee]" (10 chars); the tag and
        // the subtask title count too.
        let redacted = StoredTicket {
            assignees: ticket("a", "").assignees,
            title: "Alice Example".to_string(),
            tags: vec!["abcd".to_string()],
            ..plain("redacted", 0)
        };
        let sub = StoredTicket {
            parent_id: Some("redacted".to_string()),
            ..plain("sub", 8)
        };
        put_all(&store, &[long, redacted, sub]);
        let ids = |ids: &[&str]| ids.iter().map(|id| id.to_string()).collect::<Vec<_>>();
        let long = estimate_of(&store, &ids(&["long"]));
        assert_eq!(long.input_tokens, 2000 / 4 + prompt_tokens());
        let redacted = estimate_of(&store, &ids(&["redacted"]));
        assert_eq!(redacted.input_tokens, tokens(10 + 4 + 8) + prompt_tokens());
    }
}
