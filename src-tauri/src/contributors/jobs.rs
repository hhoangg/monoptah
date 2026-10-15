//! Sync jobs of the Contributors page: one worker thread per repository with a
//! queue, progress events for the webview, and the data behind the
//! `contributors_*` sync commands.
//!
//! A job only exists because the user pressed Sync (or the auto top-up of an
//! already synced period asked for one). Nothing here starts work on its own.
//! The jobs live in memory: after a restart there is no ghost, and the next
//! Sync continues from the batches the store already holds.

use std::collections::{BTreeMap, VecDeque};
use std::panic::AssertUnwindSafe;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

use super::sync::{self, Estimate, Reporter, SyncMode, SyncOutcome, SyncRequest, SyncStep};
use super::{gh, lock, repo_root, resolve_branch, store, RawData, Window};
use crate::session_store::SessionStore;

/// Event the webview listens to.
pub(super) const EVENT: &str = "contributors://sync";

/// A running job emits progress at most this often; state changes always go out.
const THROTTLE: Duration = Duration::from_millis(150);

/// GitHub commit lists of PRs, kept for the life of the process so a second
/// sync of a repository does not fetch them again.
static COMMITS: gh::CommitCache = gh::CommitCache::new();

// ---------------------------------------------------------------------------
// events
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum JobState {
    Queued,
    Running,
    Done,
    Error,
    Cancelled,
}

/// What the webview is told about one job.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncEvent {
    /// The repository the job belongs to: the same for every worktree of it.
    pub repo_key: String,
    /// Folder name of the repository, for display.
    pub project: String,
    /// Path the sync was requested from: the project whose Contributors page
    /// shows this job.
    pub root: String,
    pub branch: String,
    /// The period the job works on, inclusive epoch seconds, as `prepare_job`
    /// read it from the request.
    pub from: i64,
    pub to: i64,
    pub step: SyncStep,
    pub done: u64,
    pub total: u64,
    /// Weighted over all steps, 0 to 100.
    pub percent: u8,
    pub state: JobState,
    pub error: Option<String>,
}

/// Where events go. Tests record them; the app emits them to the webview.
pub(super) trait EventSink: Send + Sync {
    fn emit(&self, event: &SyncEvent);
}

pub(super) struct TauriSink(pub AppHandle);

impl EventSink for TauriSink {
    fn emit(&self, event: &SyncEvent) {
        let _ = self.0.emit(EVENT, event);
    }
}

// ---------------------------------------------------------------------------
// jobs
// ---------------------------------------------------------------------------

/// One request to sync a period of a branch.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct JobSpec {
    pub repo_key: String,
    pub project: String,
    /// The working tree the git commands run in.
    pub root: PathBuf,
    /// The path the request came from (a project folder of the webview): jobs
    /// are reported under it so the page can find its own.
    pub cwd: PathBuf,
    pub branch: String,
    /// Both edges inclusive epoch seconds.
    pub from: i64,
    pub to: i64,
    pub mode: SyncMode,
}

impl JobSpec {
    /// Two requests for the same work: running both would only repeat it.
    fn same_work(&self, other: &JobSpec) -> bool {
        self.branch == other.branch
            && self.from == other.from
            && self.to == other.to
            && self.mode == other.mode
    }

    fn event(&self, state: JobState) -> SyncEvent {
        SyncEvent {
            repo_key: self.repo_key.clone(),
            project: self.project.clone(),
            root: self.cwd.to_string_lossy().into_owned(),
            branch: self.branch.clone(),
            from: self.from,
            to: self.to,
            step: SyncStep::Counting,
            done: 0,
            total: 0,
            percent: 0,
            state,
            error: None,
        }
    }
}

/// Runs the sync of one job. The app's runner resolves the branch head and
/// calls `sync::run_sync`; tests plug in a closure.
pub(super) trait Runner: Send + Sync {
    fn run(&self, job: &JobSpec, reporter: &dyn Reporter) -> Result<SyncOutcome, String>;
}

impl<F> Runner for F
where
    F: Fn(&JobSpec, &dyn Reporter) -> Result<SyncOutcome, String> + Send + Sync,
{
    fn run(&self, job: &JobSpec, reporter: &dyn Reporter) -> Result<SyncOutcome, String> {
        self(job, reporter)
    }
}

/// The real runner: the branch head is read when the job starts, not when it
/// was queued, so a job that waited syncs what the branch is by then.
pub(super) struct AppRunner(pub AppHandle);

impl Runner for AppRunner {
    fn run(&self, job: &JobSpec, reporter: &dyn Reporter) -> Result<SyncOutcome, String> {
        let store = self.0.state::<SessionStore>();
        run_job(&store, job, &super::default_gh(), &COMMITS, reporter)
    }
}

pub(super) fn run_job(
    store: &SessionStore,
    job: &JobSpec,
    github: &dyn gh::GhRunner,
    commit_cache: &gh::CommitCache,
    reporter: &dyn Reporter,
) -> Result<SyncOutcome, String> {
    let (_, head) = resolve_branch(&job.root, Some(&job.branch))?;
    sync::run_sync(
        &SyncRequest {
            store,
            root: &job.root,
            branch: &job.branch,
            head: &head,
            from: job.from,
            to: job.to,
            mode: job.mode.clone(),
            gh: github,
            commit_cache,
        },
        reporter,
    )
}

/// What a running job shares with its reporter and with `status`.
struct Live {
    cancel: AtomicBool,
    last: Mutex<Last>,
    sink: Arc<dyn EventSink>,
}

struct Last {
    event: SyncEvent,
    emitted_at: Instant,
}

impl Live {
    /// Records a state change and returns the event to send. Never throttled.
    fn change(&self, state: JobState, error: Option<String>) -> SyncEvent {
        let mut last = lock(&self.last);
        last.event.state = state;
        last.event.error = error;
        if state == JobState::Done {
            last.event.percent = 100;
        }
        last.emitted_at = Instant::now();
        last.event.clone()
    }

    fn snapshot(&self) -> SyncEvent {
        lock(&self.last).event.clone()
    }
}

struct JobReporter(Arc<Live>);

impl Reporter for JobReporter {
    fn progress(&self, step: SyncStep, done: u64, total: u64) {
        let event = {
            let mut last = lock(&self.0.last);
            let step_changed = last.event.step != step;
            last.event.step = step;
            last.event.done = done;
            last.event.total = total;
            last.event.percent = sync::percent(step, done, total);
            if !step_changed && last.emitted_at.elapsed() < THROTTLE {
                return;
            }
            last.emitted_at = Instant::now();
            last.event.clone()
        };
        self.0.sink.emit(&event);
    }

    fn cancelled(&self) -> bool {
        self.0.cancel.load(Ordering::Relaxed)
    }
}

struct Running {
    spec: JobSpec,
    live: Arc<Live>,
}

struct Queued {
    spec: JobSpec,
    event: SyncEvent,
}

/// The jobs of one repository. An entry exists exactly while a worker thread
/// serves it, so the worker and the queue can never disagree.
#[derive(Default)]
struct RepoJobs {
    running: Option<Running>,
    queued: VecDeque<Queued>,
}

impl RepoJobs {
    fn has(&self, spec: &JobSpec) -> bool {
        // A job being cancelled is going away: a new request for the same
        // work is a real one and queues behind it.
        let running = self
            .running
            .as_ref()
            .is_some_and(|r| r.spec.same_work(spec) && !r.live.cancel.load(Ordering::Relaxed));
        running || self.queued.iter().any(|q| q.spec.same_work(spec))
    }
}

struct Shared {
    repos: Mutex<BTreeMap<String, RepoJobs>>,
    runner: Arc<dyn Runner>,
    sink: Arc<dyn EventSink>,
}

#[derive(Clone)]
pub struct JobManager(Arc<Shared>);

const PANICKED: &str = "The sync stopped unexpectedly.";

impl JobManager {
    pub(super) fn new(runner: Arc<dyn Runner>, sink: Arc<dyn EventSink>) -> Self {
        Self(Arc::new(Shared {
            repos: Mutex::new(BTreeMap::new()),
            runner,
            sink,
        }))
    }

    /// Queues a job. It runs when no other job of its repository does; a
    /// request for work that is already queued or running is dropped.
    pub(super) fn start(&self, spec: JobSpec) -> Result<(), String> {
        let mut repos = lock(&self.0.repos);
        if let Some(jobs) = repos.get_mut(&spec.repo_key) {
            if jobs.has(&spec) {
                return Ok(());
            }
            let event = spec.event(JobState::Queued);
            self.0.sink.emit(&event);
            jobs.queued.push_back(Queued { spec, event });
            return Ok(());
        }
        let key = spec.repo_key.clone();
        let manager = self.clone();
        let worker_key = key.clone();
        // The worker waits for the lock held here, so `queued` always goes
        // out before its `running`.
        let spawned = std::thread::Builder::new()
            .name("contributors-sync".to_string())
            .spawn(move || manager.work(&worker_key));
        if let Err(error) = spawned {
            let message = format!("Could not start the sync: {error}");
            let mut event = spec.event(JobState::Error);
            event.error = Some(message.clone());
            self.0.sink.emit(&event);
            return Err(message);
        }
        let event = spec.event(JobState::Queued);
        self.0.sink.emit(&event);
        let mut jobs = RepoJobs::default();
        jobs.queued.push_back(Queued { spec, event });
        repos.insert(key, jobs);
        Ok(())
    }

    /// Stops the running job of `repo_key` and drops the ones queued behind it.
    pub(super) fn cancel(&self, repo_key: &str) {
        let mut repos = lock(&self.0.repos);
        let Some(jobs) = repos.get_mut(repo_key) else {
            return;
        };
        for queued in jobs.queued.drain(..) {
            let mut event = queued.event;
            event.state = JobState::Cancelled;
            self.0.sink.emit(&event);
        }
        if let Some(running) = &jobs.running {
            running.live.cancel.store(true, Ordering::Relaxed);
        }
    }

    /// The last event of every running and queued job.
    pub(super) fn status(&self) -> Vec<SyncEvent> {
        let repos = lock(&self.0.repos);
        let mut events = Vec::new();
        for jobs in repos.values() {
            if let Some(running) = &jobs.running {
                events.push(running.live.snapshot());
            }
            events.extend(jobs.queued.iter().map(|q| q.event.clone()));
        }
        events
    }

    /// Runs `action` if no job of `repo_key` runs or waits, and keeps new jobs
    /// from starting until it is done.
    pub(super) fn when_idle<T>(
        &self,
        repo_key: &str,
        action: impl FnOnce() -> Result<T, String>,
    ) -> Result<T, String> {
        let repos = lock(&self.0.repos);
        if repos.contains_key(repo_key) {
            return Err("A sync of this repository is running. Cancel it first.".to_string());
        }
        action()
    }

    /// The worker of one repository: takes its jobs one by one, then leaves.
    fn work(&self, repo_key: &str) {
        loop {
            let (spec, live) = {
                let mut repos = lock(&self.0.repos);
                let Some(jobs) = repos.get_mut(repo_key) else {
                    return;
                };
                let Some(next) = jobs.queued.pop_front() else {
                    repos.remove(repo_key);
                    return;
                };
                let live = Arc::new(Live {
                    cancel: AtomicBool::new(false),
                    last: Mutex::new(Last {
                        event: next.spec.event(JobState::Running),
                        emitted_at: Instant::now(),
                    }),
                    sink: self.0.sink.clone(),
                });
                self.0.sink.emit(&live.snapshot());
                jobs.running = Some(Running {
                    spec: next.spec.clone(),
                    live: live.clone(),
                });
                (next.spec, live)
            };

            let reporter = JobReporter(live.clone());
            let result =
                std::panic::catch_unwind(AssertUnwindSafe(|| self.0.runner.run(&spec, &reporter)));
            let (state, error) = match result {
                Ok(Ok(outcome)) if outcome.cancelled => (JobState::Cancelled, None),
                Ok(Ok(_)) => (JobState::Done, None),
                Ok(Err(error)) => (JobState::Error, Some(error)),
                Err(_) => (JobState::Error, Some(PANICKED.to_string())),
            };

            let mut repos = lock(&self.0.repos);
            if let Some(jobs) = repos.get_mut(repo_key) {
                jobs.running = None;
            }
            self.0.sink.emit(&live.change(state, error));
        }
    }
}

// ---------------------------------------------------------------------------
// what the commands need
// ---------------------------------------------------------------------------

/// Folder name of the repository a key (its git common dir) stands for.
fn project_name(repo_key: &str) -> String {
    let path = Path::new(repo_key);
    let leaf = |p: &Path| p.file_name().map(|n| n.to_string_lossy().into_owned());
    match leaf(path) {
        Some(name) if name == ".git" => path.parent().and_then(leaf),
        Some(name) => Some(name.strip_suffix(".git").unwrap_or(&name).to_string()),
        None => None,
    }
    .filter(|name| !name.is_empty())
    .unwrap_or_else(|| repo_key.to_string())
}

/// The key every command uses for the repository `cwd` is in.
pub(super) fn repo_key(cwd: &Path) -> Result<String, String> {
    store::git_common_dir(&repo_root(cwd)?)
}

/// Turns a Sync request into a job: validates the period and the branch and
/// fixes the branch name, so the same request always compares equal.
pub(super) fn prepare_job(
    cwd: &Path,
    branch: Option<&str>,
    from: &str,
    to: &str,
    mode: &str,
    old_head: Option<&str>,
) -> Result<JobSpec, String> {
    let mode = match mode {
        "full" => SyncMode::Full,
        "github" => SyncMode::GithubRetry,
        "topup" => SyncMode::TopUp {
            old_head: old_head
                .map(str::trim)
                .filter(|h| !h.is_empty())
                .ok_or("A top-up needs the head the period was synced at.")?
                .to_string(),
        },
        other => return Err(format!("Unknown sync mode: {other}")),
    };
    let root = repo_root(cwd)?;
    let window = Window::for_repo(&root, from, Some(to))?;
    let (branch, _) = resolve_branch(&root, branch)?;
    let repo_key = store::git_common_dir(&root)?;
    Ok(JobSpec {
        project: project_name(&repo_key),
        repo_key,
        root,
        cwd: cwd.to_path_buf(),
        branch,
        from: window.since_epoch,
        to: window.until_epoch,
        mode,
    })
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LoadResponse {
    pub raw: RawData,
    /// The whole period is synced and nothing in it is missing.
    pub covered: bool,
    /// Part of the period was never synced or its sync was cancelled. False
    /// with `covered` false means a synced period that lacks some commits (a
    /// merge brought older ones in): it can be repaired in the background.
    pub has_gaps: bool,
    /// Commits of the period that the store does not have.
    pub missing_commits: u64,
    pub synced_at: Option<i64>,
    /// Branch head the synced period was last brought up to.
    pub coverage_head: Option<String>,
    /// Branch head now.
    pub head: String,
    /// `complete | partial | unavailable`; `None` when nothing is synced.
    pub gh_state: Option<String>,
}

/// A repository id no row has: reads of a repository the store has never seen
/// find nothing, and nothing is written for a page that was only opened.
const UNKNOWN_REPO: i64 = -1;

/// A row written by a sync that reached the present: it ends at the moment it
/// was synced. A row that ends earlier ends at a fixed past date (the
/// previous-period half of another sync, or a merge of older rows into a newer
/// head), so no time after it is known to be synced.
fn reached_present(row: &store::CoverageRow) -> bool {
    row.to_epoch.saturating_add(PRESENT_SLACK_SECS) >= row.synced_at
}

/// Clock skew between taking `now` and writing a row.
const PRESENT_SLACK_SECS: i64 = 5;

/// A period of the store, for `contributors_load`. Git is asked for hashes
/// only, with no database lock held, then the store answers in one read
/// transaction. Nothing is synced or fetched.
///
/// The period never extends past `now`. When it reaches the present, the end
/// is also pulled back to where the sync covered it: coverage stops at the
/// moment of the last sync (only rows that reached the present count, see
/// `reached_present`), so without this a current period would stop being
/// covered a second after it was synced. What happened since shows up through
/// the top-up that the moved head asks for.
pub(super) fn load_response(
    store: &SessionStore,
    cwd: &Path,
    branch: Option<&str>,
    since: &str,
    until: &str,
    now: i64,
) -> Result<LoadResponse, String> {
    let root = repo_root(cwd)?;
    let window = Window::for_repo(&root, since, Some(until))?;
    let (name, head) = resolve_branch(&root, branch)?;
    let dir = store::git_common_dir(&root)?;
    let repo = store
        .with_read_conn(|conn| store::find_repo_id(conn, &dir))?
        .unwrap_or(UNKNOWN_REPO);

    let since = window.since_epoch;
    let mut until = window.until_epoch.min(now);
    if window.until_epoch >= now {
        let rows = store.with_read_conn(|conn| store::coverage(conn, repo, &name))?;
        let edge = rows
            .iter()
            .filter(|r| r.to_epoch >= since && r.from_epoch <= until)
            .filter(|r| reached_present(r))
            .map(|r| r.to_epoch)
            .max();
        if let Some(edge) = edge {
            until = until.min(edge);
        }
    }

    let commits = store::window_commits(&root, &head, since, until)?;
    let loaded = store.with_read_conn(|conn| {
        store::load_window(conn, repo, &name, &head, since, until, &commits)
    })?;
    Ok(LoadResponse {
        raw: loaded.raw,
        covered: loaded.covered,
        has_gaps: loaded.has_gaps,
        missing_commits: loaded.missing_commits,
        synced_at: loaded.synced_at,
        coverage_head: loaded.coverage_head,
        head,
        gh_state: loaded.gh_state,
    })
}

/// What syncing a period involves, for `contributors_sync_estimate`.
pub(super) fn estimate_response(
    store: &SessionStore,
    github: &dyn gh::GhRunner,
    cwd: &Path,
    branch: Option<&str>,
    from: &str,
    to: &str,
    now: i64,
) -> Result<Estimate, String> {
    let root = repo_root(cwd)?;
    let window = Window::for_repo(&root, from, Some(to))?;
    let (_, head) = resolve_branch(&root, branch)?;
    let dir = store::git_common_dir(&root)?;
    let repo = store.with_read_conn(|conn| store::find_repo_id(conn, &dir))?;
    sync::estimate(
        &root,
        &head,
        window.since_epoch,
        window.until_epoch.min(now),
        store,
        repo,
        github,
    )
}

/// Deletes everything synced for the repository `cwd` is in, unless a job of
/// it runs or waits.
pub(super) fn clear_repo(
    jobs: &JobManager,
    store: &SessionStore,
    cwd: &Path,
) -> Result<(), String> {
    let key = repo_key(cwd)?;
    jobs.when_idle(&key, || {
        let conn = store.lock_conn()?;
        store::clear_repo(&conn, &key)
    })
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::sync::atomic::AtomicUsize;
    use std::sync::Condvar;

    use super::super::tests::{TestRepo, ALICE};
    use super::*;

    const WAIT: Duration = Duration::from_secs(10);

    // -- harness ----------------------------------------------------------

    #[derive(Default)]
    struct Recording {
        events: Mutex<Vec<SyncEvent>>,
        changed: Condvar,
    }

    impl EventSink for Recording {
        fn emit(&self, event: &SyncEvent) {
            self.events.lock().unwrap().push(event.clone());
            self.changed.notify_all();
        }
    }

    impl Recording {
        fn all(&self) -> Vec<SyncEvent> {
            self.events.lock().unwrap().clone()
        }

        /// Waits until `ready` holds for the events so far.
        fn wait(&self, what: &str, ready: impl Fn(&[SyncEvent]) -> bool) {
            let deadline = Instant::now() + WAIT;
            let mut events = self.events.lock().unwrap();
            while !ready(&events) {
                let left = deadline.saturating_duration_since(Instant::now());
                assert!(!left.is_zero(), "timed out waiting for {what}: {events:?}");
                events = self.changed.wait_timeout(events, left).unwrap().0;
            }
        }

        fn wait_state(&self, branch: &str, state: JobState) {
            self.wait(&format!("{branch} {state:?}"), |events| {
                events
                    .iter()
                    .any(|e| e.branch == branch && e.state == state)
            });
        }

        fn states(&self, branch: &str) -> Vec<JobState> {
            let mut states: Vec<JobState> = Vec::new();
            for event in self.all().iter().filter(|e| e.branch == branch) {
                if states.last() != Some(&event.state) {
                    states.push(event.state);
                }
            }
            states
        }
    }

    /// What the fake sync does: a job (named by its branch) runs until it is
    /// let go or cancelled.
    #[derive(Default)]
    struct Control {
        released: Mutex<HashSet<String>>,
        log: Mutex<Vec<String>>,
        active: AtomicUsize,
        max_active: AtomicUsize,
    }

    impl Control {
        fn release(&self, branch: &str) {
            self.released.lock().unwrap().insert(branch.to_string());
        }

        fn log(&self) -> Vec<String> {
            self.log.lock().unwrap().clone()
        }

        fn started(&self, branch: &str) -> bool {
            self.log().contains(&format!("start {branch}"))
        }
    }

    fn outcome(cancelled: bool) -> SyncOutcome {
        SyncOutcome {
            cancelled,
            gh_state: "complete".to_string(),
            gh_note: None,
        }
    }

    struct Harness {
        jobs: JobManager,
        sink: Arc<Recording>,
        ctl: Arc<Control>,
    }

    impl Harness {
        fn new() -> Self {
            let ctl = Arc::new(Control::default());
            let sink = Arc::new(Recording::default());
            let run_ctl = ctl.clone();
            let runner = move |job: &JobSpec, reporter: &dyn Reporter| {
                let ctl = &run_ctl;
                let now = ctl.active.fetch_add(1, Ordering::SeqCst) + 1;
                ctl.max_active.fetch_max(now, Ordering::SeqCst);
                ctl.log
                    .lock()
                    .unwrap()
                    .push(format!("start {}", job.branch));
                reporter.progress(SyncStep::Commits, 1, 4);
                let cancelled = loop {
                    if reporter.cancelled() {
                        break true;
                    }
                    if ctl.released.lock().unwrap().contains(&job.branch) {
                        break false;
                    }
                    std::thread::sleep(Duration::from_millis(2));
                };
                ctl.log.lock().unwrap().push(format!("end {}", job.branch));
                ctl.active.fetch_sub(1, Ordering::SeqCst);
                Ok(outcome(cancelled))
            };
            Self {
                jobs: JobManager::new(Arc::new(runner), sink.clone()),
                sink,
                ctl,
            }
        }

        fn with_runner(runner: impl Runner + 'static) -> (JobManager, Arc<Recording>) {
            let sink = Arc::new(Recording::default());
            (JobManager::new(Arc::new(runner), sink.clone()), sink)
        }
    }

    fn spec(repo: &str, branch: &str) -> JobSpec {
        JobSpec {
            repo_key: repo.to_string(),
            project: repo.to_string(),
            root: PathBuf::from("/nowhere"),
            cwd: PathBuf::from("/page/nowhere"),
            branch: branch.to_string(),
            from: 100,
            to: 200,
            mode: SyncMode::Full,
        }
    }

    // -- queue ------------------------------------------------------------

    #[test]
    fn two_starts_for_one_repo_run_one_after_the_other() {
        let h = Harness::new();
        h.jobs.start(spec("repo", "a")).unwrap();
        h.jobs.start(spec("repo", "b")).unwrap();

        h.sink.wait_state("a", JobState::Running);
        // `b` waits while `a` runs.
        std::thread::sleep(Duration::from_millis(60));
        assert!(!h.ctl.started("b"));
        assert_eq!(h.sink.states("b"), vec![JobState::Queued]);

        h.ctl.release("a");
        h.sink.wait_state("b", JobState::Running);
        h.ctl.release("b");
        h.sink.wait_state("b", JobState::Done);

        assert_eq!(h.ctl.log(), ["start a", "end a", "start b", "end b"]);
        assert_eq!(h.ctl.max_active.load(Ordering::SeqCst), 1);
        assert_eq!(
            h.sink.states("a"),
            vec![JobState::Queued, JobState::Running, JobState::Done]
        );
        assert_eq!(
            h.sink.states("b"),
            vec![JobState::Queued, JobState::Running, JobState::Done]
        );
    }

    #[test]
    fn jobs_of_different_repos_run_at_the_same_time() {
        let h = Harness::new();
        h.jobs.start(spec("one", "a")).unwrap();
        h.jobs.start(spec("two", "b")).unwrap();
        h.sink.wait_state("a", JobState::Running);
        h.sink.wait_state("b", JobState::Running);
        h.ctl.release("a");
        h.ctl.release("b");
        h.sink.wait_state("a", JobState::Done);
        h.sink.wait_state("b", JobState::Done);
        assert_eq!(h.ctl.max_active.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn identical_request_is_a_no_op_while_queued_or_running() {
        let h = Harness::new();
        h.jobs.start(spec("repo", "a")).unwrap();
        h.sink.wait_state("a", JobState::Running);
        h.jobs.start(spec("repo", "a")).unwrap();
        h.jobs.start(spec("repo", "b")).unwrap();
        h.jobs.start(spec("repo", "b")).unwrap();

        let states: Vec<_> = h.jobs.status().iter().map(|e| e.state).collect();
        assert_eq!(states, vec![JobState::Running, JobState::Queued]);

        h.ctl.release("a");
        h.ctl.release("b");
        h.sink.wait_state("b", JobState::Done);
        let starts = h
            .ctl
            .log()
            .iter()
            .filter(|l| l.starts_with("start"))
            .count();
        assert_eq!(starts, 2);
    }

    #[test]
    fn same_branch_with_another_period_or_mode_is_a_different_request() {
        let h = Harness::new();
        h.jobs.start(spec("repo", "a")).unwrap();
        let mut later = spec("repo", "a");
        later.from = 300;
        h.jobs.start(later).unwrap();
        let mut retry = spec("repo", "a");
        retry.mode = SyncMode::GithubRetry;
        h.jobs.start(retry).unwrap();
        assert_eq!(h.jobs.status().len(), 3);
        h.jobs.cancel("repo");
        h.sink.wait_state("a", JobState::Cancelled);
    }

    #[test]
    fn a_repo_can_sync_again_after_its_jobs_finished() {
        let h = Harness::new();
        h.ctl.release("a");
        h.jobs.start(spec("repo", "a")).unwrap();
        h.sink.wait("first done", |e| {
            e.iter().any(|e| e.state == JobState::Done)
        });
        h.jobs.start(spec("repo", "a")).unwrap();
        h.sink.wait("second done", |e| {
            e.iter().filter(|e| e.state == JobState::Done).count() == 2
        });
        assert!(h.jobs.status().is_empty());
    }

    // -- cancel -----------------------------------------------------------

    #[test]
    fn cancel_stops_the_running_job_and_drops_the_queue() {
        let h = Harness::new();
        h.jobs.start(spec("repo", "a")).unwrap();
        h.jobs.start(spec("repo", "b")).unwrap();
        h.jobs.start(spec("other", "c")).unwrap();
        h.sink.wait_state("a", JobState::Running);
        h.sink.wait_state("c", JobState::Running);

        h.jobs.cancel("repo");
        h.sink.wait_state("a", JobState::Cancelled);
        h.sink.wait_state("b", JobState::Cancelled);

        assert!(!h.ctl.started("b"), "a dropped job must never run");
        assert_eq!(
            h.sink.states("b"),
            vec![JobState::Queued, JobState::Cancelled]
        );
        // The other repository is untouched.
        assert_eq!(
            h.sink.states("c"),
            vec![JobState::Queued, JobState::Running]
        );
        let left: Vec<_> = h.jobs.status().iter().map(|e| e.branch.clone()).collect();
        assert_eq!(left, ["c"]);
        h.ctl.release("c");
        h.sink.wait_state("c", JobState::Done);
    }

    #[test]
    fn a_request_for_work_being_cancelled_queues_again() {
        let h = Harness::new();
        h.jobs.start(spec("repo", "a")).unwrap();
        h.sink.wait_state("a", JobState::Running);
        h.jobs.cancel("repo");
        // Whether or not the cancel has landed yet, the new request must run.
        h.jobs.start(spec("repo", "a")).unwrap();
        h.sink.wait("second run", |_| h.ctl.log().len() >= 3);
        h.ctl.release("a");
        h.sink
            .wait("done", |e| e.iter().any(|e| e.state == JobState::Done));
    }

    #[test]
    fn cancel_of_an_idle_repo_does_nothing() {
        let h = Harness::new();
        h.jobs.cancel("nothing");
        assert!(h.sink.all().is_empty());
    }

    // -- status -----------------------------------------------------------

    #[test]
    fn status_lists_running_with_its_progress_and_queued() {
        let h = Harness::new();
        h.jobs.start(spec("repo", "a")).unwrap();
        h.jobs.start(spec("repo", "b")).unwrap();
        h.jobs.start(spec("other", "c")).unwrap();
        h.sink.wait_state("a", JobState::Running);
        h.sink.wait_state("c", JobState::Running);
        h.sink.wait("progress", |_| {
            h.jobs
                .status()
                .iter()
                .filter(|e| e.state == JobState::Running)
                .all(|e| e.done == 1)
        });

        let status = h.jobs.status();
        let find = |branch: &str| status.iter().find(|e| e.branch == branch).unwrap().clone();
        // Status carries the latest progress even though events are throttled.
        let a = find("a");
        assert_eq!(
            (a.state, a.step, a.done, a.total),
            (JobState::Running, SyncStep::Commits, 1, 4)
        );
        assert_eq!(a.percent, sync::percent(SyncStep::Commits, 1, 4));
        assert_eq!(a.root, "/page/nowhere");
        assert_eq!((a.from, a.to), (100, 200));
        assert_eq!(find("b").state, JobState::Queued);
        assert_eq!(find("c").state, JobState::Running);
        assert_eq!(status.len(), 3);

        h.jobs.cancel("repo");
        h.jobs.cancel("other");
        h.sink.wait_state("c", JobState::Cancelled);
    }

    // -- clear ------------------------------------------------------------

    #[test]
    fn clear_is_refused_while_a_job_of_the_repo_runs_or_waits() {
        let h = Harness::new();
        h.jobs.start(spec("repo", "a")).unwrap();
        h.sink.wait_state("a", JobState::Running);

        let mut ran = false;
        let refused = h.jobs.when_idle("repo", || {
            ran = true;
            Ok(())
        });
        assert!(refused.is_err());
        assert!(!ran);
        // Another repository is not in the way.
        assert!(h.jobs.when_idle("other", || Ok(())).is_ok());

        h.ctl.release("a");
        h.sink.wait_state("a", JobState::Done);
        // The worker leaves right after its last event.
        let deadline = Instant::now() + WAIT;
        while !h.jobs.status().is_empty() || h.jobs.when_idle("repo", || Ok(())).is_err() {
            assert!(Instant::now() < deadline, "the worker never left");
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    #[test]
    fn clear_removes_the_stored_rows_of_an_idle_repo() {
        let repo = TestRepo::new();
        repo.commit(
            &[("a.txt", "a\n")],
            "one",
            ALICE,
            "2026-01-05T10:00:00+0000",
        );
        let store = SessionStore::open_in_memory().unwrap();
        let (jobs, _) = Harness::with_runner(|_: &JobSpec, _: &dyn Reporter| Ok(outcome(false)));
        let spec = prepare_job(
            repo.path(),
            None,
            "2026-01-01T00:00:00Z",
            "2026-01-31T00:00:00Z",
            "full",
            None,
        )
        .unwrap();
        run_job(
            &store,
            &spec,
            &gh::Unavailable,
            &gh::CommitCache::new(),
            &NoReport,
        )
        .unwrap();
        let loaded = |store: &SessionStore| {
            load_response(
                store,
                repo.path(),
                None,
                "2026-01-01T00:00:00Z",
                "2026-01-31T00:00:00Z",
                now_secs(),
            )
            .unwrap()
        };
        assert!(loaded(&store).covered);

        clear_repo(&jobs, &store, repo.path()).unwrap();
        let after = loaded(&store);
        assert!(!after.covered);
        assert!(after.raw.commits.is_empty());
        assert_eq!(after.missing_commits, 1);
    }

    // -- events -----------------------------------------------------------

    #[test]
    fn progress_is_throttled_but_state_changes_never_are() {
        let (jobs, sink) = Harness::with_runner(|_: &JobSpec, reporter: &dyn Reporter| {
            for done in 0..=1000 {
                reporter.progress(SyncStep::Commits, done, 1000);
            }
            Ok(outcome(false))
        });
        jobs.start(spec("repo", "a")).unwrap();
        sink.wait_state("a", JobState::Done);

        let events = sink.all();
        let running = events
            .iter()
            .filter(|e| e.state == JobState::Running)
            .count();
        // The change to running, and at most one progress event in the same
        // 150 ms: not one per call.
        assert!(running <= 3, "{running} running events");
        assert_eq!(
            sink.states("a"),
            vec![JobState::Queued, JobState::Running, JobState::Done]
        );
        // The end carries the final counts and 100 percent.
        let last = events.last().unwrap();
        assert_eq!(last.state, JobState::Done);
        assert_eq!((last.done, last.total, last.percent), (1000, 1000, 100));
    }

    #[test]
    fn progress_after_the_throttle_window_is_emitted() {
        let (jobs, sink) = Harness::with_runner(|_: &JobSpec, reporter: &dyn Reporter| {
            reporter.progress(SyncStep::Counting, 1, 10);
            std::thread::sleep(THROTTLE + Duration::from_millis(60));
            reporter.progress(SyncStep::Counting, 5, 10);
            reporter.progress(SyncStep::Counting, 6, 10);
            Ok(outcome(false))
        });
        jobs.start(spec("repo", "a")).unwrap();
        sink.wait_state("a", JobState::Done);

        let seen: Vec<u64> = sink
            .all()
            .iter()
            .filter(|e| e.state == JobState::Running)
            .map(|e| e.done)
            .collect();
        // The call after the pause goes out; the one right behind it is held
        // back (a stall of 150 ms in between could let it through, hence <= 3).
        assert_eq!(seen[0], 0);
        assert!(seen.contains(&5), "{seen:?}");
        assert!(seen.len() <= 3, "{seen:?}");
    }

    #[test]
    fn a_change_of_step_is_never_held_back_by_the_throttle() {
        let (jobs, sink) = Harness::with_runner(|_: &JobSpec, reporter: &dyn Reporter| {
            reporter.progress(SyncStep::Counting, 1, 1);
            reporter.progress(SyncStep::Commits, 0, 7);
            reporter.progress(SyncStep::PullRequests, 0, 3);
            reporter.progress(SyncStep::Github, 0, 2);
            Ok(outcome(false))
        });
        jobs.start(spec("repo", "a")).unwrap();
        sink.wait_state("a", JobState::Done);

        let steps: Vec<SyncStep> = sink
            .all()
            .iter()
            .filter(|e| e.state == JobState::Running)
            .map(|e| e.step)
            .collect();
        assert_eq!(
            steps,
            [
                SyncStep::Counting,
                SyncStep::Commits,
                SyncStep::PullRequests,
                SyncStep::Github
            ]
        );
    }

    #[test]
    fn an_error_is_reported_and_the_next_job_still_runs() {
        let (jobs, sink) = Harness::with_runner(|job: &JobSpec, _: &dyn Reporter| {
            if job.branch == "bad" {
                Err("git exploded".to_string())
            } else {
                Ok(outcome(false))
            }
        });
        jobs.start(spec("repo", "bad")).unwrap();
        jobs.start(spec("repo", "good")).unwrap();
        sink.wait_state("good", JobState::Done);

        let failed = sink
            .all()
            .into_iter()
            .find(|e| e.state == JobState::Error)
            .unwrap();
        assert_eq!(failed.error.as_deref(), Some("git exploded"));
        assert_eq!(failed.branch, "bad");
    }

    #[test]
    fn a_panicking_sync_is_an_error_not_a_stuck_repository() {
        let (jobs, sink) = Harness::with_runner(|job: &JobSpec, _: &dyn Reporter| {
            if job.branch == "bad" {
                panic!("boom");
            }
            Ok(outcome(false))
        });
        jobs.start(spec("repo", "bad")).unwrap();
        jobs.start(spec("repo", "good")).unwrap();
        sink.wait_state("good", JobState::Done);
        let failed = sink
            .all()
            .into_iter()
            .find(|e| e.state == JobState::Error)
            .unwrap();
        assert_eq!(failed.error.as_deref(), Some(PANICKED));
    }

    #[test]
    fn events_serialize_as_the_webview_expects() {
        let event = SyncEvent {
            repo_key: "/r/.git".to_string(),
            project: "r".to_string(),
            root: "/work/r".to_string(),
            branch: "main".to_string(),
            from: 1_700_000_000,
            to: 1_700_086_400,
            step: SyncStep::PullRequests,
            done: 3,
            total: 9,
            percent: 61,
            state: JobState::Cancelled,
            error: None,
        };
        assert_eq!(
            serde_json::to_value(&event).unwrap(),
            serde_json::json!({
                "repoKey": "/r/.git",
                "project": "r",
                "root": "/work/r",
                "branch": "main",
                "from": 1_700_000_000,
                "to": 1_700_086_400,
                "step": "pullRequests",
                "done": 3,
                "total": 9,
                "percent": 61,
                "state": "cancelled",
                "error": null,
            })
        );
    }

    // -- requests and reads against a real repository -----------------------

    struct NoReport;

    impl Reporter for NoReport {
        fn progress(&self, _: SyncStep, _: u64, _: u64) {}
        fn cancelled(&self) -> bool {
            false
        }
    }

    fn now_secs() -> i64 {
        super::super::now_epoch()
    }

    fn repo_with_commits() -> TestRepo {
        let repo = TestRepo::new();
        repo.commit(
            &[("a.txt", "a\n")],
            "one",
            ALICE,
            "2026-01-05T10:00:00+0000",
        );
        repo.commit(
            &[("b.txt", "b\n")],
            "two",
            ALICE,
            "2026-01-06T10:00:00+0000",
        );
        repo
    }

    const SINCE: &str = "2026-01-01T00:00:00Z";
    const UNTIL: &str = "2026-01-31T23:59:59Z";

    #[test]
    fn prepare_job_fixes_the_branch_and_reads_the_period() {
        let repo = repo_with_commits();
        let a = prepare_job(repo.path(), None, SINCE, UNTIL, "full", None).unwrap();
        let b = prepare_job(repo.path(), Some("main"), SINCE, UNTIL, "full", None).unwrap();
        assert_eq!(a, b);
        assert_eq!(a.branch, "main");
        assert_eq!(a.to - a.from, 31 * 86_400 - 1);
        assert!(a.repo_key.ends_with(".git"));
        assert!(!a.project.is_empty() && !a.project.ends_with(".git"));
        assert_eq!(a.cwd, repo.path());

        // Events carry the path the page asked from, not the repository top.
        let sub = repo.path().join("nested");
        std::fs::create_dir(&sub).unwrap();
        let nested = prepare_job(&sub, None, SINCE, UNTIL, "full", None).unwrap();
        assert_eq!(nested.event(JobState::Queued).root, sub.to_string_lossy());
        assert_eq!(nested.event(JobState::Queued).from, a.from);

        let top = prepare_job(repo.path(), None, SINCE, UNTIL, "topup", Some("abc")).unwrap();
        assert_eq!(
            top.mode,
            SyncMode::TopUp {
                old_head: "abc".to_string()
            }
        );
        assert_eq!(
            prepare_job(repo.path(), None, SINCE, UNTIL, "github", None)
                .unwrap()
                .mode,
            SyncMode::GithubRetry
        );
    }

    #[test]
    fn prepare_job_rejects_bad_requests() {
        let repo = repo_with_commits();
        let err = |mode: &str, old: Option<&str>, branch: Option<&str>, from: &str| {
            prepare_job(repo.path(), branch, from, UNTIL, mode, old).unwrap_err()
        };
        assert!(err("topup", None, None, SINCE).contains("top-up"));
        assert!(err("topup", Some("  "), None, SINCE).contains("top-up"));
        assert!(err("weekly", None, None, SINCE).contains("Unknown sync mode"));
        assert!(err("full", None, Some("nope"), SINCE).contains("Unknown branch"));
        assert!(err("full", None, Some("-x"), SINCE).contains("Unknown branch"));
        assert!(err("full", None, None, "not a date").contains("Invalid"));
        let outside =
            std::env::temp_dir().join(format!("monoptah-not-a-repo-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&outside).unwrap();
        let not_repo = prepare_job(&outside, None, SINCE, UNTIL, "full", None);
        std::fs::remove_dir_all(&outside).unwrap();
        assert_eq!(not_repo.unwrap_err(), "Not a git repository");
    }

    #[test]
    fn worktrees_of_one_repository_share_a_key() {
        let repo = repo_with_commits();
        let tree = std::env::temp_dir().join(format!("monoptah-worktree-{}", uuid::Uuid::new_v4()));
        repo.run(
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "side",
                tree.to_str().unwrap(),
            ],
            ALICE,
            "2026-01-07T10:00:00+0000",
        );
        let main = repo_key(repo.path()).unwrap();
        let side = repo_key(&tree).unwrap();
        std::fs::remove_dir_all(&tree).unwrap();
        assert_eq!(main, side);
    }

    #[test]
    fn project_name_is_the_repository_folder() {
        assert_eq!(project_name("/work/monoptah/.git"), "monoptah");
        assert_eq!(project_name("/srv/git/tools.git"), "tools");
        assert_eq!(project_name("/"), "/");
    }

    #[test]
    fn load_of_a_never_synced_period_is_empty_and_writes_nothing() {
        let repo = repo_with_commits();
        let store = SessionStore::open_in_memory().unwrap();
        let head = repo.run(&["rev-parse", "HEAD"], ALICE, "2026-01-07T10:00:00+0000");

        let loaded = load_response(&store, repo.path(), None, SINCE, UNTIL, now_secs()).unwrap();
        assert!(!loaded.covered);
        assert!(loaded.has_gaps, "never synced is a gap, not a repair");
        assert_eq!(loaded.missing_commits, 2);
        assert!(loaded.raw.commits.is_empty());
        assert_eq!(loaded.head, head);
        assert_eq!(
            (loaded.synced_at, loaded.coverage_head, loaded.gh_state),
            (None, None, None)
        );
        let rows: i64 = store
            .lock_conn()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM contrib_repos", [], |r| r.get(0))
            .unwrap();
        assert_eq!(rows, 0);
    }

    #[test]
    fn sync_then_load_round_trips_through_the_store() {
        let repo = repo_with_commits();
        let store = SessionStore::open_in_memory().unwrap();
        let spec = prepare_job(repo.path(), None, SINCE, UNTIL, "full", None).unwrap();
        let outcome = run_job(
            &store,
            &spec,
            &gh::Unavailable,
            &gh::CommitCache::new(),
            &NoReport,
        )
        .unwrap();
        assert!(!outcome.cancelled);

        let loaded = load_response(&store, repo.path(), None, SINCE, UNTIL, now_secs()).unwrap();
        assert!(loaded.covered);
        assert_eq!(loaded.missing_commits, 0);
        assert_eq!(loaded.raw.commits.len(), 2);
        assert_eq!(loaded.coverage_head.as_deref(), Some(loaded.head.as_str()));
        assert!(loaded.synced_at.is_some());
        assert_eq!(loaded.gh_state.as_deref(), Some("unavailable"));
    }

    #[test]
    fn a_cancelled_sync_leaves_the_period_incomplete() {
        struct Stop;
        impl Reporter for Stop {
            fn progress(&self, _: SyncStep, _: u64, _: u64) {}
            fn cancelled(&self) -> bool {
                true
            }
        }
        let repo = repo_with_commits();
        let store = SessionStore::open_in_memory().unwrap();
        let spec = prepare_job(repo.path(), None, SINCE, UNTIL, "full", None).unwrap();
        let outcome = run_job(
            &store,
            &spec,
            &gh::Unavailable,
            &gh::CommitCache::new(),
            &Stop,
        )
        .unwrap();
        assert!(outcome.cancelled);
        let loaded = load_response(&store, repo.path(), None, SINCE, UNTIL, now_secs()).unwrap();
        assert!(!loaded.covered);
        assert!(loaded.has_gaps, "a cancelled sync leaves a gap");
    }

    #[test]
    fn a_period_reaching_the_present_stays_covered_after_the_sync() {
        let repo = repo_with_commits();
        let store = SessionStore::open_in_memory().unwrap();
        // A period that ends in the future, like the current week.
        let future = "2099-01-01T00:00:00Z";
        let spec = prepare_job(repo.path(), None, SINCE, future, "full", None).unwrap();
        run_job(
            &store,
            &spec,
            &gh::Unavailable,
            &gh::CommitCache::new(),
            &NoReport,
        )
        .unwrap();

        // Seconds later the coverage still ends at the sync, and that must not
        // read as a gap.
        for later in [now_secs(), now_secs() + 3_600] {
            let loaded = load_response(&store, repo.path(), None, SINCE, future, later).unwrap();
            assert!(loaded.covered, "at {later}");
            assert_eq!(loaded.raw.commits.len(), 2);
        }
    }

    #[test]
    fn an_unsynced_start_of_a_current_period_is_still_a_gap() {
        let repo = repo_with_commits();
        let store = SessionStore::open_in_memory().unwrap();
        let spec = prepare_job(
            repo.path(),
            None,
            "2026-01-05T00:00:00Z",
            "2099-01-01T00:00:00Z",
            "full",
            None,
        )
        .unwrap();
        run_job(
            &store,
            &spec,
            &gh::Unavailable,
            &gh::CommitCache::new(),
            &NoReport,
        )
        .unwrap();
        let loaded = load_response(
            &store,
            repo.path(),
            None,
            "2026-01-01T00:00:00Z",
            "2099-01-01T00:00:00Z",
            now_secs() + 60,
        )
        .unwrap();
        assert!(!loaded.covered);
        assert!(loaded.has_gaps);
    }

    /// Replaces what the store knows about the coverage of `main` with `rows`
    /// (from, to, synced_at), added the way a sync adds them.
    fn set_coverage(store: &SessionStore, repo: &TestRepo, head: &str, rows: &[(i64, i64, i64)]) {
        let dir = store::git_common_dir(repo.path()).unwrap();
        let mut conn = store.lock_conn().unwrap();
        let id = store::repo_id(&conn, &dir, None).unwrap();
        conn.execute("DELETE FROM contrib_coverage", []).unwrap();
        let tx = conn.transaction().unwrap();
        for &(from_epoch, to_epoch, synced_at) in rows {
            store::add_coverage(
                &tx,
                id,
                "main",
                &store::CoverageRow {
                    from_epoch,
                    to_epoch,
                    head: head.to_string(),
                    synced_at,
                    gh_state: "unavailable".to_string(),
                    gh_note: None,
                },
            )
            .unwrap();
        }
        tx.commit().unwrap();
    }

    /// A store that holds both commits of the repository, with no coverage.
    fn stored_commits_only(repo: &TestRepo) -> (SessionStore, String) {
        let store = SessionStore::open_in_memory().unwrap();
        let spec = prepare_job(repo.path(), None, SINCE, UNTIL, "full", None).unwrap();
        run_job(
            &store,
            &spec,
            &gh::Unavailable,
            &gh::CommitCache::new(),
            &NoReport,
        )
        .unwrap();
        let head = repo.run(&["rev-parse", "HEAD"], ALICE, "2026-01-07T10:00:00+0000");
        (store, head)
    }

    fn at(iso: &str) -> i64 {
        super::super::parse_iso_epoch(iso).unwrap()
    }

    #[test]
    fn a_past_row_of_another_sync_does_not_cover_the_present() {
        // Scenario A: a month synced a few days ago, whose row ends at the end
        // of that month, then the last 7 days opened. The days after the
        // month are not synced, and the head has not moved, so nothing else
        // would ever ask for a sync.
        let repo = repo_with_commits();
        let (store, head) = stored_commits_only(&repo);
        let now = at("2026-01-25T12:00:00Z");
        set_coverage(
            &store,
            &repo,
            &head,
            &[(at("2025-12-01T00:00:00Z"), at("2026-01-20T23:59:59Z"), now)],
        );
        let loaded = load_response(
            &store,
            repo.path(),
            None,
            "2026-01-18T00:00:00Z",
            "2099-01-01T00:00:00Z",
            now,
        )
        .unwrap();
        assert_eq!(loaded.missing_commits, 0);
        assert!(!loaded.covered);
    }

    #[test]
    fn a_row_merged_into_a_newer_sync_does_not_cover_the_present() {
        // Scenario B: the current month synced up to the present, then an
        // older stretch synced later; the two rows merge into one whose end is
        // the first sync's moment but whose time is the second sync's.
        let repo = repo_with_commits();
        let (store, head) = stored_commits_only(&repo);
        let first = at("2026-01-25T12:00:00Z");
        let second = at("2026-01-28T12:00:00Z");
        set_coverage(
            &store,
            &repo,
            &head,
            &[
                (at("2026-01-01T00:00:00Z"), first, first),
                (
                    at("2025-10-01T00:00:00Z"),
                    at("2025-12-31T23:59:59Z"),
                    second,
                ),
            ],
        );
        let rows = store
            .with_read_conn(|conn| {
                let dir = store::git_common_dir(repo.path())?;
                let id = store::find_repo_id(conn, &dir)?.unwrap();
                store::coverage(conn, id, "main")
            })
            .unwrap();
        assert_eq!(rows.len(), 1, "the rows must have merged: {rows:?}");
        assert_eq!(rows[0].synced_at, second);

        let loaded = load_response(
            &store,
            repo.path(),
            None,
            "2026-01-01T00:00:00Z",
            "2099-01-01T00:00:00Z",
            second,
        )
        .unwrap();
        assert!(!loaded.covered);
    }

    #[test]
    fn a_row_that_reached_the_present_covers_it_even_with_old_neighbours() {
        let repo = repo_with_commits();
        let (store, head) = stored_commits_only(&repo);
        let synced = at("2026-01-25T12:00:00Z");
        set_coverage(
            &store,
            &repo,
            &head,
            &[(at("2025-12-01T00:00:00Z"), synced, synced)],
        );
        let loaded = load_response(
            &store,
            repo.path(),
            None,
            "2026-01-01T00:00:00Z",
            "2099-01-01T00:00:00Z",
            synced + 3_600,
        )
        .unwrap();
        assert!(loaded.covered);
    }

    #[test]
    fn a_new_commit_after_the_sync_makes_the_period_not_covered() {
        let repo = repo_with_commits();
        let store = SessionStore::open_in_memory().unwrap();
        let spec = prepare_job(repo.path(), None, SINCE, UNTIL, "full", None).unwrap();
        run_job(
            &store,
            &spec,
            &gh::Unavailable,
            &gh::CommitCache::new(),
            &NoReport,
        )
        .unwrap();
        repo.commit(
            &[("c.txt", "c\n")],
            "three",
            ALICE,
            "2026-01-08T10:00:00+0000",
        );
        let loaded = load_response(&store, repo.path(), None, SINCE, UNTIL, now_secs()).unwrap();
        assert!(!loaded.covered);
        assert!(!loaded.has_gaps);
        assert_eq!(loaded.missing_commits, 1);
        assert_ne!(loaded.coverage_head.as_deref(), Some(loaded.head.as_str()));
    }

    #[test]
    fn a_merge_bringing_older_commits_into_a_synced_period_is_a_repair_not_a_gap() {
        let repo = repo_with_commits();
        let store = SessionStore::open_in_memory().unwrap();
        let spec = prepare_job(repo.path(), None, SINCE, UNTIL, "full", None).unwrap();
        run_job(
            &store,
            &spec,
            &gh::Unavailable,
            &gh::CommitCache::new(),
            &NoReport,
        )
        .unwrap();
        // A commit dated before the sync, as a non-fast-forward merge brings.
        repo.commit(
            &[("old.txt", "o\n")],
            "dated earlier",
            ALICE,
            "2026-01-02T10:00:00+0000",
        );
        let loaded = load_response(&store, repo.path(), None, SINCE, UNTIL, now_secs()).unwrap();
        assert!(!loaded.covered);
        assert!(!loaded.has_gaps, "the coverage is intact");
        assert_eq!(loaded.missing_commits, 1);
        assert!(loaded.coverage_head.is_some());
        assert!(
            !loaded.raw.commits.is_empty(),
            "the stored data stays loadable"
        );
    }

    #[test]
    fn estimate_counts_without_syncing_anything() {
        let repo = repo_with_commits();
        let store = SessionStore::open_in_memory().unwrap();
        let estimate = estimate_response(
            &store,
            &gh::Unavailable,
            repo.path(),
            None,
            SINCE,
            UNTIL,
            now_secs(),
        )
        .unwrap();
        assert_eq!((estimate.commits, estimate.stored_commits), (2, 0));
        assert_eq!(estimate.prs, None);
        let rows: i64 = store
            .lock_conn()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM contrib_commits", [], |r| r.get(0))
            .unwrap();
        assert_eq!(rows, 0);
    }

    #[test]
    fn estimate_and_load_do_not_read_past_now() {
        let repo = repo_with_commits();
        let store = SessionStore::open_in_memory().unwrap();
        // "Now" is before the second commit.
        let now = 1_767_600_000; // 2026-01-05T08:00:00Z
        let estimate = estimate_response(
            &store,
            &gh::Unavailable,
            repo.path(),
            None,
            SINCE,
            UNTIL,
            now,
        )
        .unwrap();
        assert_eq!(estimate.commits, 0);
        let loaded = load_response(&store, repo.path(), None, SINCE, UNTIL, now).unwrap();
        assert_eq!(loaded.missing_commits, 0);
    }

    #[test]
    fn serialized_shapes_are_stable() {
        let repo = repo_with_commits();
        let store = SessionStore::open_in_memory().unwrap();
        let loaded = load_response(&store, repo.path(), None, SINCE, UNTIL, now_secs()).unwrap();
        let value = serde_json::to_value(&loaded).unwrap();
        let mut keys: Vec<&str> = value
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            [
                "coverageHead",
                "covered",
                "ghState",
                "hasGaps",
                "head",
                "missingCommits",
                "raw",
                "syncedAt"
            ]
        );
        assert!(value["syncedAt"].is_null() && value["ghState"].is_null());
    }
}
