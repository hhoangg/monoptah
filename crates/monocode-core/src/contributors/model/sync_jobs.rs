//! The rules that keep the list of sync jobs right. Port of the pure part of
//! `syncJobs.ts`; the listening, the commands and the focus hook belong to
//! the app, which feeds this store.
//!
//! One `SyncJobStore` per app holds the latest event of every job. A job entry
//! can only be wrong when an event was missed (the job ended before the
//! listener was ready, a status answer arrived after the `done` event, or the
//! core dropped a duplicate request without an event). Two things keep that
//! from leaving a job on screen forever:
//!
//! - An ended job leaves a tombstone, so a status answer that was asked for
//!   before it ended cannot bring it back.
//! - A reconcile asks the core what really runs (`api::sync_status`) and drops
//!   live entries it does not list. The app runs one after the listener is
//!   up, after every start and cancel, and when the window gets focus.
//!
//! A reconcile is two steps around the question, `begin_reconcile` before
//! asking and `finish_reconcile` with the answer, so events that arrive in
//! between are never overruled by an answer that predates them.

use crate::contributors::jobs::{JobState, SyncEvent};
use crate::contributors::sync::{SyncMode, SyncStep};
use chrono::{DateTime, Local};
use std::collections::HashMap;

/// A job that has not ended yet.
pub fn is_live(event: &SyncEvent) -> bool {
    matches!(event.state, JobState::Queued | JobState::Running)
}

pub fn step_label(step: SyncStep) -> &'static str {
    match step {
        SyncStep::Counting => "Counting",
        SyncStep::Commits => "Commits",
        SyncStep::PullRequests => "Pull requests",
        SyncStep::Github => "GitHub",
        SyncStep::Survival => "Survival",
    }
}

/// Identity of a job as the page sees it: project path, branch, and window in epoch seconds.
pub fn job_key(root: &str, branch: &str, from: i64, to: i64) -> String {
    format!("{root}\u{0}{branch}\u{0}{from}\u{0}{to}")
}

pub fn event_key(event: &SyncEvent) -> String {
    job_key(&event.root, &event.branch, event.from, event.to)
}

/// `date.toISOString()` without the milliseconds, the form the core takes.
pub fn iso_seconds(date: &DateTime<Local>) -> String {
    date.with_timezone(&chrono::Utc)
        .format("%Y-%m-%dT%H:%M:%SZ")
        .to_string()
}

pub fn epoch_seconds(date: &DateTime<Local>) -> i64 {
    date.timestamp()
}

/// One request to sync a period of a branch.
#[derive(Clone, Debug, PartialEq)]
pub struct StartJob {
    /// The branch name the core resolves and reports in its events.
    pub branch_name: String,
    /// The branch the user picked; `None` follows the checked-out one.
    pub branch: Option<String>,
    pub from: DateTime<Local>,
    pub to: DateTime<Local>,
    pub mode: SyncMode,
}

/// The arguments of `api::sync_start`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StartCall {
    pub from: String,
    pub to: String,
    pub branch: Option<String>,
    /// `full`, `topup` or `github`.
    pub mode: &'static str,
    pub old_head: Option<String>,
}

impl StartJob {
    pub fn call(&self) -> StartCall {
        let (mode, old_head) = match &self.mode {
            SyncMode::Full => ("full", None),
            SyncMode::TopUp { old_head } => ("topup", Some(old_head.clone())),
            SyncMode::GithubRetry => ("github", None),
        };
        StartCall {
            from: iso_seconds(&self.from),
            to: iso_seconds(&self.to),
            branch: self.branch.clone(),
            mode,
            old_head,
        }
    }

    /// The entry shown at once, before the core has said anything.
    fn queued_event(&self, root: &str) -> SyncEvent {
        SyncEvent {
            repo_key: String::new(),
            project: String::new(),
            root: root.into(),
            branch: self.branch_name.clone(),
            from: epoch_seconds(&self.from),
            to: epoch_seconds(&self.to),
            step: SyncStep::Counting,
            done: 0,
            total: 0,
            percent: 0,
            state: JobState::Queued,
            error: None,
        }
    }
}

/// What `apply` did.
#[derive(Clone, Debug, PartialEq)]
pub struct Applied {
    /// The list of jobs is different now: notify the views.
    pub changed: bool,
    /// The job ended (done, cancelled or failed): whoever stored its result can read it.
    pub ended: Option<SyncEvent>,
}

/// The question a reconcile asks the core; keep it until the answer is in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReconcileTicket {
    asked: u64,
    epoch: u64,
}

/// The latest event of every job, in the order the jobs first appeared.
#[derive(Debug, Default)]
pub struct SyncJobStore {
    jobs: Vec<(String, SyncEvent)>,
    /// Marker of the last change of a key.
    touched: HashMap<String, u64>,
    /// Marker at which a key ended. A status answer older than it must not revive the key.
    tombstones: HashMap<String, u64>,
    marker: u64,
    epoch: u64,
}

impl SyncJobStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Every entry, oldest first. Feed it to `summarize_sync`.
    pub fn jobs(&self) -> Vec<SyncEvent> {
        self.jobs.iter().map(|(_, e)| e.clone()).collect()
    }

    pub fn keys(&self) -> Vec<String> {
        self.jobs.iter().map(|(k, _)| k.clone()).collect()
    }

    pub fn get(&self, key: &str) -> Option<&SyncEvent> {
        self.jobs.iter().find(|(k, _)| k == key).map(|(_, e)| e)
    }

    fn bump(&mut self) -> u64 {
        self.marker += 1;
        self.marker
    }

    fn put(&mut self, event: SyncEvent) {
        let key = event_key(&event);
        let marker = self.bump();
        self.touched.insert(key.clone(), marker);
        match self.jobs.iter_mut().find(|(k, _)| *k == key) {
            Some(slot) => slot.1 = event,
            None => self.jobs.push((key, event)),
        }
    }

    fn remove(&mut self, key: &str) -> bool {
        let before = self.jobs.len();
        self.jobs.retain(|(k, _)| k != key);
        self.jobs.len() != before
    }

    /// Applies an event from the core: the authority on its own job.
    pub fn apply(&mut self, event: SyncEvent) -> Applied {
        let key = event_key(&event);
        if is_live(&event) {
            self.tombstones.remove(&key);
            self.put(event);
            return Applied {
                changed: true,
                ended: None,
            };
        }
        let marker = self.bump();
        self.tombstones.insert(key.clone(), marker);
        let changed = if event.state == JobState::Error {
            // Kept so the page can say why; it is not a running job any more.
            self.put(event.clone());
            true
        } else {
            self.touched.remove(&key);
            self.remove(&key)
        };
        Applied {
            changed,
            ended: Some(event),
        }
    }

    /// A job was requested: it shows as queued at once, and is replaced by the
    /// real events. If the core refuses or silently drops the request the
    /// reconcile that follows removes it again.
    pub fn queue(&mut self, root: &str, job: &StartJob) -> String {
        let event = job.queued_event(root);
        let key = event_key(&event);
        self.tombstones.remove(&key);
        self.put(event);
        key
    }

    /// The core refused the request: an error entry, not a live job.
    pub fn queue_failed(&mut self, root: &str, job: &StartJob, error: &str) {
        let mut event = job.queued_event(root);
        event.state = JobState::Error;
        event.error = Some(error.to_string());
        self.put(event);
    }

    /// Call before asking the core what runs.
    pub fn begin_reconcile(&self) -> ReconcileTicket {
        ReconcileTicket {
            asked: self.marker,
            epoch: self.epoch,
        }
    }

    /// Makes the jobs match what the core lists: live entries it does not know
    /// are dropped, listed ones are added. Entries changed (or ended) after the
    /// question was asked are left alone, since the answer may predate them.
    /// Returns the entries dropped, as cancelled, so listeners can tell.
    pub fn finish_reconcile(
        &mut self,
        ticket: ReconcileTicket,
        running: &[SyncEvent],
    ) -> Vec<SyncEvent> {
        if ticket.epoch != self.epoch {
            return vec![];
        }
        let asked = ticket.asked;
        let mut listed: Vec<String> = Vec::with_capacity(running.len());
        for event in running {
            let key = event_key(event);
            listed.push(key.clone());
            if self.tombstones.get(&key).copied().unwrap_or(0) > asked {
                continue;
            }
            if self.touched.get(&key).copied().unwrap_or(0) > asked {
                continue;
            }
            if self.get(&key).is_some_and(|have| have.state == event.state) {
                continue;
            }
            self.put(event.clone());
        }
        let mut ended = vec![];
        let stale: Vec<(String, SyncEvent)> = self
            .jobs
            .iter()
            .filter(|(key, entry)| {
                is_live(entry)
                    && !listed.contains(key)
                    && self.touched.get(key).copied().unwrap_or(0) <= asked
            })
            .cloned()
            .collect();
        for (key, entry) in stale {
            self.touched.remove(&key);
            self.remove(&key);
            ended.push(SyncEvent {
                state: JobState::Cancelled,
                ..entry
            });
        }
        ended
    }

    /// Drops a failed job's entry once the user has seen it. A job that still runs is left alone.
    pub fn dismiss(&mut self, event: &SyncEvent) -> bool {
        let key = event_key(event);
        if self.get(&key).is_some_and(|e| e.state == JobState::Error) {
            return self.remove(&key);
        }
        false
    }

    /// Forgets every entry of a project, e.g. after its synced data was cleared.
    pub fn forget(&mut self, root: &str) -> bool {
        let prefix = format!("{root}\u{0}");
        let before = self.jobs.len();
        self.jobs.retain(|(key, _)| !key.starts_with(&prefix));
        self.jobs.len() != before
    }

    /// Back to a fresh start; a question still in flight is ignored.
    pub fn reset(&mut self) {
        self.epoch += 1;
        self.jobs.clear();
        self.touched.clear();
        self.tombstones.clear();
        self.marker = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contributors::jobs::JobState;
    use crate::contributors::model::test_fixtures::local_date;
    use crate::contributors::sync::SyncStep;

    fn from() -> DateTime<Local> {
        local_date(2026, 8, 1)
    }

    fn to() -> DateTime<Local> {
        local_date(2026, 9, 1)
    }

    fn secs(d: &DateTime<Local>) -> i64 {
        d.timestamp()
    }

    fn event(patch: impl FnOnce(&mut SyncEvent)) -> SyncEvent {
        let mut e = SyncEvent {
            repo_key: "/r/.git".into(),
            project: "r".into(),
            root: "/work/r".into(),
            branch: "main".into(),
            from: secs(&from()),
            to: secs(&to()),
            step: SyncStep::Commits,
            done: 1,
            total: 10,
            percent: 10,
            state: JobState::Running,
            error: None,
        };
        patch(&mut e);
        e
    }

    fn key_of(patch: impl FnOnce(&mut SyncEvent)) -> String {
        event_key(&event(patch))
    }

    fn job() -> StartJob {
        StartJob {
            branch_name: "main".into(),
            branch: None,
            from: from(),
            to: to(),
            mode: SyncMode::Full,
        }
    }

    fn percent_at(store: &SyncJobStore, key: &str) -> Option<u8> {
        store.get(key).map(|e| e.percent)
    }

    fn state_at(store: &SyncJobStore, key: &str) -> Option<JobState> {
        store.get(key).map(|e| e.state)
    }

    /// `ensureSyncJobs`' first reconcile: nothing asked in between.
    fn reconcile(store: &mut SyncJobStore, running: &[SyncEvent]) -> Vec<SyncEvent> {
        let ticket = store.begin_reconcile();
        store.finish_reconcile(ticket, running)
    }

    #[test]
    fn job_key_joins_root_branch_and_window_with_nul() {
        assert_eq!(
            job_key("/work/r", "main", 1, 2),
            "/work/r\u{0}main\u{0}1\u{0}2"
        );
        assert_eq!(
            key_of(|_| {}),
            job_key("/work/r", "main", secs(&from()), secs(&to()))
        );
    }

    #[test]
    fn sync_jobs_store_seeds_from_what_rust_lists() {
        let mut store = SyncJobStore::new();
        reconcile(&mut store, &[event(|e| e.percent = 40)]);
        assert_eq!(percent_at(&store, &key_of(|_| {})), Some(40));
    }

    #[test]
    fn sync_jobs_store_follows_events_and_says_when_a_subscriber_has_something_new() {
        let mut store = SyncJobStore::new();
        assert!(store.apply(event(|e| e.state = JobState::Queued)).changed);
        assert!(store.apply(event(|e| e.percent = 55)).changed);
        assert_eq!(percent_at(&store, &key_of(|_| {})), Some(55));
        // an event for a job that is not there and ended changes nothing
        assert!(
            !store
                .apply(event(|e| {
                    e.root = "/work/none".into();
                    e.state = JobState::Done;
                }))
                .changed
        );
    }

    #[test]
    fn sync_jobs_store_removes_an_ended_job_tells_the_listeners_and_keeps_a_failure_to_show() {
        let mut store = SyncJobStore::new();
        store.apply(event(|_| {}));
        let done = store.apply(event(|e| {
            e.state = JobState::Done;
            e.percent = 100;
        }));
        assert_eq!(store.get(&key_of(|_| {})), None);
        assert_eq!(done.ended.as_ref().map(|e| e.state), Some(JobState::Done));
        let other = |e: &mut SyncEvent| {
            e.from = 1;
            e.to = 2;
        };
        store.apply(event(other));
        let failed = store.apply(event(|e| {
            e.from = 1;
            e.to = 2;
            e.state = JobState::Error;
            e.error = Some("boom".into());
        }));
        let kept = store.get(&key_of(other)).unwrap();
        assert_eq!(kept.state, JobState::Error);
        assert_eq!(kept.error.as_deref(), Some("boom"));
        assert_eq!(
            failed.ended.as_ref().map(|e| e.state),
            Some(JobState::Error)
        );
    }

    #[test]
    fn sync_jobs_store_stuck_path_b_a_status_answer_that_lands_after_the_done_event_does_not_bring_the_job_back(
    ) {
        let mut store = SyncJobStore::new();
        store.apply(event(|_| {}));
        let ticket = store.begin_reconcile();
        // Rust listed the job, then it ended, and the event reached us first.
        store.apply(event(|e| {
            e.state = JobState::Done;
            e.percent = 100;
        }));
        store.finish_reconcile(ticket, &[event(|_| {})]);
        assert_eq!(store.get(&key_of(|_| {})), None);
    }

    #[test]
    fn sync_jobs_store_stuck_path_b_a_job_that_ended_before_listening_was_ready_is_not_left_behind()
    {
        let mut store = SyncJobStore::new();
        // Rust accepted the request, and the job finished before the event
        // listener was ready: the queued entry is the only trace of it.
        store.queue("/work/r", &job());
        assert_eq!(state_at(&store, &key_of(|_| {})), Some(JobState::Queued));
        let ended = reconcile(&mut store, &[]);
        assert!(store.jobs().is_empty());
        assert_eq!(ended.len(), 1);
    }

    #[test]
    fn sync_jobs_store_stuck_path_a_a_duplicate_request_rust_dropped_leaves_no_queued_entry() {
        let mut store = SyncJobStore::new();
        store.queue("/work/r", &job());
        // Another worktree's identical job runs under its own root.
        reconcile(&mut store, &[event(|e| e.root = "/work/other".into())]);
        assert_eq!(store.get(&key_of(|_| {})), None);
        assert_eq!(
            state_at(&store, &key_of(|e| e.root = "/work/other".into())),
            Some(JobState::Running)
        );
    }

    #[test]
    fn sync_jobs_store_a_started_job_shows_queued_at_once_and_stays_while_rust_lists_it() {
        let mut store = SyncJobStore::new();
        store.queue("/work/r", &job());
        assert_eq!(state_at(&store, &key_of(|_| {})), Some(JobState::Queued));
        // the reconcile after the start command asks once the entry exists
        reconcile(&mut store, &[event(|e| e.percent = 3)]);
        let got = store.get(&key_of(|_| {})).unwrap();
        assert_eq!((got.state, got.percent), (JobState::Running, 3));
        // what the start command receives
        let call = job().call();
        assert_eq!(call.mode, "full");
        assert_eq!(call.from, iso_seconds(&from()));
        assert_eq!(call.to, iso_seconds(&to()));
        assert_eq!(call.branch, None);
        assert_eq!(call.old_head, None);
    }

    #[test]
    fn start_job_call_carries_the_branch_the_mode_and_the_old_head() {
        let top_up = StartJob {
            branch: Some("dev".into()),
            mode: SyncMode::TopUp {
                old_head: "abc".into(),
            },
            ..job()
        };
        let call = top_up.call();
        assert_eq!(call.branch.as_deref(), Some("dev"));
        assert_eq!(call.mode, "topup");
        assert_eq!(call.old_head.as_deref(), Some("abc"));
        let retry = StartJob {
            mode: SyncMode::GithubRetry,
            ..job()
        };
        assert_eq!(retry.call().mode, "github");
    }

    #[test]
    fn iso_seconds_prints_utc_without_milliseconds() {
        use chrono::TimeZone;
        let t = chrono::Utc
            .with_ymd_and_hms(2026, 9, 1, 7, 5, 9)
            .unwrap()
            .with_timezone(&Local);
        assert_eq!(iso_seconds(&t), "2026-09-01T07:05:09Z");
        assert_eq!(epoch_seconds(&t), t.timestamp());
    }

    #[test]
    fn sync_jobs_store_a_refused_start_becomes_an_error_entry_not_a_live_job() {
        let mut store = SyncJobStore::new();
        store.queue("/work/r", &job());
        store.queue_failed("/work/r", &job(), "Could not start the sync");
        let got = store.get(&key_of(|_| {})).unwrap();
        assert_eq!(got.state, JobState::Error);
        assert_eq!(got.error.as_deref(), Some("Could not start the sync"));
        // the reconcile that follows keeps the failure: it is not live
        reconcile(&mut store, &[]);
        assert_eq!(state_at(&store, &key_of(|_| {})), Some(JobState::Error));
    }

    #[test]
    fn sync_jobs_store_reconcile_drops_a_live_entry_rust_does_not_list_and_only_that_one() {
        let mut store = SyncJobStore::new();
        store.apply(event(|_| {}));
        store.apply(event(|e| {
            e.from = 7;
            e.to = 8;
        }));
        store.apply(event(|e| {
            e.from = 9;
            e.to = 10;
            e.state = JobState::Error;
            e.error = Some("x".into());
        }));
        let ended = reconcile(
            &mut store,
            &[event(|e| {
                e.from = 7;
                e.to = 8;
            })],
        );
        assert_eq!(store.get(&key_of(|_| {})), None);
        assert!(store
            .get(&key_of(|e| {
                e.from = 7;
                e.to = 8;
            }))
            .is_some());
        assert_eq!(
            state_at(
                &store,
                &key_of(|e| {
                    e.from = 9;
                    e.to = 10;
                })
            ),
            Some(JobState::Error)
        );
        assert_eq!(ended.len(), 1);
        assert_eq!(ended[0].from, secs(&from()));
        assert_eq!(ended[0].state, JobState::Cancelled);
    }

    #[test]
    fn sync_jobs_store_reconcile_leaves_an_entry_that_changed_after_the_question_was_asked() {
        let mut store = SyncJobStore::new();
        let ticket = store.begin_reconcile();
        // Created while the answer is on its way: the answer cannot know it.
        store.apply(event(|e| e.state = JobState::Queued));
        store.finish_reconcile(ticket, &[]);
        assert_eq!(state_at(&store, &key_of(|_| {})), Some(JobState::Queued));
    }

    #[test]
    fn sync_jobs_store_a_new_job_under_the_same_key_after_it_ended_is_accepted_again() {
        let mut store = SyncJobStore::new();
        store.apply(event(|_| {}));
        store.apply(event(|e| {
            e.state = JobState::Done;
            e.percent = 100;
        }));
        store.apply(event(|e| e.percent = 5));
        reconcile(&mut store, &[event(|e| e.percent = 5)]);
        assert_eq!(percent_at(&store, &key_of(|_| {})), Some(5));
    }

    #[test]
    fn sync_jobs_store_a_cancelled_job_is_dropped_by_the_reconcile_that_follows() {
        // cancelSyncJobs asks Rust to stop, then reconciles: a job Rust no
        // longer lists goes away and is reported as cancelled.
        let mut store = SyncJobStore::new();
        store.apply(event(|_| {}));
        let ended = reconcile(&mut store, &[]);
        assert_eq!(store.get(&key_of(|_| {})), None);
        assert_eq!(ended[0].state, JobState::Cancelled);
    }

    #[test]
    fn sync_jobs_store_forgets_the_entries_of_one_project() {
        let mut store = SyncJobStore::new();
        store.apply(event(|_| {}));
        store.apply(event(|e| e.root = "/work/other".into()));
        assert!(store.forget("/work/r"));
        assert_eq!(
            store
                .jobs()
                .iter()
                .map(|e| e.root.as_str())
                .collect::<Vec<_>>(),
            ["/work/other"]
        );
        assert_eq!(
            store.keys(),
            [job_key("/work/other", "main", secs(&from()), secs(&to()))]
        );
        // a root that is only a prefix of another one is not forgotten with it
        store.apply(event(|e| e.root = "/work/other2".into()));
        assert!(store.forget("/work/other"));
        assert_eq!(store.jobs().len(), 1);
        assert!(!store.forget("/work/nothing"));
    }

    #[test]
    fn sync_jobs_store_dismisses_a_failure_but_never_a_running_job() {
        let mut store = SyncJobStore::new();
        let failed = event(|e| {
            e.from = 1;
            e.to = 2;
            e.state = JobState::Error;
            e.error = Some("boom".into());
        });
        store.apply(failed.clone());
        store.apply(event(|_| {}));
        assert!(!store.dismiss(&event(|_| {})));
        assert!(store.get(&key_of(|_| {})).is_some());
        assert!(store.dismiss(&failed));
        assert_eq!(store.get(&event_key(&failed)), None);
    }

    #[test]
    fn sync_jobs_store_keeps_jobs_in_the_order_they_first_appeared() {
        let mut store = SyncJobStore::new();
        store.apply(event(|e| e.root = "/work/a".into()));
        store.apply(event(|e| e.root = "/work/b".into()));
        store.apply(event(|e| {
            e.root = "/work/a".into();
            e.percent = 99;
        }));
        let jobs = store.jobs();
        let roots: Vec<&str> = jobs.iter().map(|e| e.root.as_str()).collect();
        assert_eq!(roots, ["/work/a", "/work/b"]);
        assert_eq!(jobs[0].percent, 99);
    }

    #[test]
    fn sync_jobs_store_reset_drops_everything_and_orphans_a_question_in_flight() {
        let mut store = SyncJobStore::new();
        store.apply(event(|_| {}));
        let ticket = store.begin_reconcile();
        store.reset();
        assert!(store.jobs().is_empty());
        // the answer of the old question must not seed the fresh store
        assert!(store.finish_reconcile(ticket, &[event(|_| {})]).is_empty());
        assert!(store.jobs().is_empty());
    }
}
