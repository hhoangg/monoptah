//! The sync API of the Contributors page: one function per Tauri sync command
//! (`contributors_read_config` ... `contributors_clear`), with the same
//! arguments and error strings. All of them block (git, disk, SQLite), so the
//! UI calls them on the background executor.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::assessor::{self, AiConfig, AiConfigStatus};
use super::clickup_tickets::TicketLink;
use super::db::ContributorsDb;
use super::fetch::{self, FetchOutcome, FetchRegistry};
use super::jobs::{self, DbRunner, EventSink, JobManager, LoadResponse, Runner, SyncEvent};
use super::sync::Estimate;
use super::ticket_jobs::{
    self, DbDeps, TicketDeps, TicketEstimate, TicketJob, TicketJobManager, TicketJobStatus,
    TicketSink,
};
use super::ticket_store::{self, TicketRecord};
use super::{
    default_gh, first_commit as git_first_commit, list_branches, now_epoch, read_config as config,
    read_git_user, read_mailmap as mailmap, window_epoch, write_config as save_config,
    ContributorsBranches, GitUser,
};
use crate::inbox::clickup::{self, ClickUpSpace};

/// The Contributors state of the app: its database, the sync jobs and the
/// ticket scoring jobs.
///
/// Open it after the shell `Store`: the store creates a new database file with
/// the full schema first, and `ContributorsDb` only adds the `contrib_*`
/// tables to it. Never open it for a read-only store (`--check-data`): it
/// creates tables and starts writing.
pub struct Contributors {
    db: Arc<ContributorsDb>,
    data_dir: PathBuf,
    jobs: JobManager,
    tickets: TicketJobManager,
    fetches: FetchRegistry,
}

impl Contributors {
    /// Opens the Contributors database at `path` (`Store::database_path()`;
    /// `:memory:` gives a private in-memory database). `data_dir` is the app
    /// data directory holding `clickup-config.json` and `contributors-ai.json`.
    /// `sink` receives every sync event and `ticket_sink` every ticket job
    /// event; neither may block.
    pub fn open(
        path: &Path,
        data_dir: &Path,
        sink: Arc<dyn EventSink>,
        ticket_sink: Arc<dyn TicketSink>,
    ) -> Result<Self, String> {
        let db = Arc::new(ContributorsDb::open(path)?);
        Ok(Self::new(db, data_dir, sink, ticket_sink))
    }

    pub fn new(
        db: Arc<ContributorsDb>,
        data_dir: &Path,
        sink: Arc<dyn EventSink>,
        ticket_sink: Arc<dyn TicketSink>,
    ) -> Self {
        let runner = DbRunner(db.clone());
        let deps = DbDeps {
            db: db.clone(),
            data_dir: data_dir.to_path_buf(),
        };
        Self::with_parts(db, data_dir, runner, sink, deps, ticket_sink)
    }

    pub(super) fn with_parts(
        db: Arc<ContributorsDb>,
        data_dir: &Path,
        runner: impl Runner + 'static,
        sink: Arc<dyn EventSink>,
        deps: impl TicketDeps + 'static,
        ticket_sink: Arc<dyn TicketSink>,
    ) -> Self {
        Self {
            db,
            data_dir: data_dir.to_path_buf(),
            jobs: JobManager::new(Arc::new(runner), sink),
            tickets: TicketJobManager::new(Arc::new(deps), ticket_sink),
            fetches: FetchRegistry::default(),
        }
    }

    pub fn db(&self) -> &Arc<ContributorsDb> {
        &self.db
    }

    /// The app data directory (`clickup-config.json`, `contributors-ai.json`).
    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }
}

/// Every cwd a caller gives may start with `~`.
fn cwd_path(cwd: &str) -> PathBuf {
    crate::scm::expand_home(Path::new(cwd))
}

/// `<repo root>/.monoptah/contributors.json`, or `None` when there is none.
pub fn read_config(cwd: &str) -> Result<Option<String>, String> {
    config(&cwd_path(cwd))
}

/// `<repo root>/.mailmap`, or `None` when there is none.
pub fn read_mailmap(cwd: &str) -> Result<Option<String>, String> {
    mailmap(&cwd_path(cwd))
}

pub fn write_config(cwd: &str, content: &str) -> Result<(), String> {
    save_config(&cwd_path(cwd), content)
}

pub fn branches(cwd: &str) -> Result<ContributorsBranches, String> {
    list_branches(&cwd_path(cwd))
}

/// The date (ISO) of the first commit of `branch` (the current one when
/// `None`), or `None` for an empty history.
pub fn first_commit(cwd: &str, branch: Option<&str>) -> Result<Option<String>, String> {
    git_first_commit(&cwd_path(cwd), branch)
}

pub fn git_user(cwd: &str) -> Result<GitUser, String> {
    read_git_user(&cwd_path(cwd))
}

/// A period read from the store. Starts no git or GitHub work beyond listing
/// the period's commit hashes.
pub fn load(
    contributors: &Contributors,
    cwd: &str,
    branch: Option<&str>,
    since: &str,
    until: &str,
) -> Result<LoadResponse, String> {
    jobs::load_response(
        &contributors.db,
        &cwd_path(cwd),
        branch,
        since,
        until,
        now_epoch(),
    )
}

pub fn sync_estimate(
    contributors: &Contributors,
    cwd: &str,
    branch: Option<&str>,
    from: &str,
    to: &str,
) -> Result<Estimate, String> {
    jobs::estimate_response(
        &contributors.db,
        &default_gh(),
        &cwd_path(cwd),
        branch,
        from,
        to,
        now_epoch(),
    )
}

/// Queues a sync. Returns at once; progress comes through the event sink.
/// `mode` is `full`, `github` or `topup` (which needs `old_head`).
pub fn sync_start(
    contributors: &Contributors,
    cwd: &str,
    branch: Option<&str>,
    from: &str,
    to: &str,
    mode: &str,
    old_head: Option<&str>,
) -> Result<(), String> {
    let spec = jobs::prepare_job(&cwd_path(cwd), branch, from, to, mode, old_head)?;
    contributors.jobs.start(spec)
}

/// Brings the remote-tracking ref of `branch` (the default branch when `None`)
/// up to date from `origin`. It is the only call that fetches a branch; loads,
/// estimates and sync jobs never do (a job may still ask the remote for PR
/// heads). `Err` only for a path that is not a repository.
pub fn fetch_origin(
    contributors: &Contributors,
    cwd: &str,
    branch: Option<&str>,
) -> Result<FetchOutcome, String> {
    fetch::fetch_origin(&contributors.fetches, &cwd_path(cwd), branch)
}

/// Stops the running sync of the repository and drops the ones queued behind
/// it, and the fetch of `origin` when one runs.
pub fn sync_cancel(contributors: &Contributors, cwd: &str) -> Result<(), String> {
    let key = jobs::repo_key(&cwd_path(cwd))?;
    contributors.fetches.cancel(&key);
    contributors.jobs.cancel(&key);
    Ok(())
}

/// The last event of every running and queued sync, for a view that was
/// opened after they started.
pub fn sync_status(contributors: &Contributors) -> Vec<SyncEvent> {
    contributors.jobs.status()
}

/// Deletes everything synced for the repository; refused while a sync of it runs.
pub fn clear(contributors: &Contributors, cwd: &str) -> Result<(), String> {
    jobs::clear_repo(&contributors.jobs, &contributors.db, &cwd_path(cwd))
}

// ---------------------------------------------------------------------------
// ticket scoring
// ---------------------------------------------------------------------------

/// A period given as date strings, as epoch seconds with both edges inclusive
/// (a bare end date means the last second of its day).
fn ticket_period(from: &str, to: &str) -> Result<(i64, i64), String> {
    let from =
        window_epoch(from.trim(), false).ok_or_else(|| format!("Invalid start date: {from}"))?;
    let to = window_epoch(to.trim(), true).ok_or_else(|| format!("Invalid end date: {to}"))?;
    if from > to {
        return Err("The start date is after the end date.".to_string());
    }
    Ok((from, to))
}

/// ClickUp ids go into request paths, so only their usual characters pass.
fn clickup_id(id: &str) -> Result<&str, String> {
    let id = id.trim();
    if id.is_empty()
        || !id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(format!("Invalid ClickUp id: {id}"));
    }
    Ok(id)
}

/// Queues a fetch of the tickets of each link updated since `from`, whatever
/// their status. `to` only validates the period: there is no upper bound.
/// Returns the job id at once; progress comes through the ticket sink.
pub fn tickets_fetch(
    contributors: &Contributors,
    links: Vec<TicketLink>,
    from: &str,
    to: &str,
) -> Result<u64, String> {
    let (from, _) = ticket_period(from, to)?;
    for link in &links {
        clickup_id(&link.workspace_id)?;
        for space in link.space_ids.iter().flatten() {
            clickup_id(space)?;
        }
    }
    contributors.tickets.start(TicketJob::Fetch { links, from })
}

/// Queues an assessment of the tickets among `ids` that need one (all of them
/// with `force`, except the ones with a manual level).
pub fn tickets_assess(
    contributors: &Contributors,
    ids: Vec<String>,
    force: bool,
) -> Result<u64, String> {
    contributors.tickets.start(TicketJob::Assess { ids, force })
}

/// A rough size of assessing the tickets among `ids`: how many would really
/// be assessed and the input tokens that would take. Calls nothing.
pub fn tickets_estimate(
    contributors: &Contributors,
    ids: &[String],
) -> Result<TicketEstimate, String> {
    contributors
        .db
        .with_read_conn(|conn| ticket_jobs::estimate(conn, ids))
}

/// Stops the running ticket job and drops the ones queued behind it.
pub fn tickets_cancel(contributors: &Contributors) {
    contributors.tickets.cancel();
}

/// The last event of the running (or next queued) ticket job, for a view that
/// was opened after it started, with the number of fetch jobs queued or running.
pub fn tickets_status(contributors: &Contributors) -> TicketJobStatus {
    contributors.tickets.status_with_pending()
}

/// Whether the stored tickets of the workspace's spaces (`space_ids` of `None`
/// or empty means every space) are complete for the window `from`..`to`: the
/// completed fetches cover it from its start up to its end, or up to now when
/// the window has not ended. A fetch covers only the span from its start to
/// the moment its listing began, and a fetch of narrowed spaces only those
/// spaces (a fetch of every space satisfies any set).
pub fn tickets_window_covered(
    contributors: &Contributors,
    workspace_id: &str,
    space_ids: Option<&[String]>,
    from: &str,
    to: &str,
) -> Result<bool, String> {
    let (from, to) = ticket_period(from, to)?;
    contributors.db.with_read_conn(|conn| {
        ticket_store::window_covered(conn, workspace_id, space_ids, from, to.min(now_epoch()))
    })
}

/// The stored tickets of the workspaces updated since the start of the period
/// (every status, subtasks included), with their assessments and manual levels.
/// There is no upper bound: a ticket delivered in the period and edited after it
/// still has to be loaded. Fetches nothing.
pub fn tickets_load(
    contributors: &Contributors,
    workspace_ids: &[String],
    since: &str,
) -> Result<Vec<TicketRecord>, String> {
    let since =
        window_epoch(since.trim(), false).ok_or_else(|| format!("Invalid start date: {since}"))?;
    contributors
        .db
        .with_read_conn(|conn| ticket_store::load_records(conn, workspace_ids, since))
}

/// Sets (or with `level` of `None` clears) the manual level of a ticket.
pub fn ticket_override(
    contributors: &Contributors,
    ticket_id: &str,
    level: Option<&str>,
    note: Option<&str>,
) -> Result<(), String> {
    let note = note.map(str::trim).filter(|note| !note.is_empty());
    let conn = contributors.db.lock_conn()?;
    ticket_store::set_override(&conn, ticket_id, level, note)
}

/// The AI settings without the key: the caller learns only whether one is set.
pub fn ai_config_get(contributors: &Contributors) -> Result<AiConfigStatus, String> {
    ai_config_get_at(&contributors.data_dir)
}

/// [`ai_config_get`] for a data directory, for Settings: it needs neither the
/// database nor the job queue.
pub fn ai_config_get_at(data_dir: &Path) -> Result<AiConfigStatus, String> {
    Ok(assessor::read_ai_config(data_dir)?.status())
}

/// Saves the AI settings. `api_key` of `None` keeps the stored key, `Some("")`
/// deletes it and any other value replaces it.
pub fn ai_config_set(
    contributors: &Contributors,
    runner: &str,
    model: &str,
    api_key: Option<&str>,
) -> Result<AiConfigStatus, String> {
    ai_config_set_at(&contributors.data_dir, runner, model, api_key)
}

/// [`ai_config_set`] for a data directory.
pub fn ai_config_set_at(
    dir: &Path,
    runner: &str,
    model: &str,
    api_key: Option<&str>,
) -> Result<AiConfigStatus, String> {
    // Keeping the stored key needs the stored settings: if they cannot be
    // read, say so rather than save over them and lose the key. When a new
    // key is given (or the old one is being deleted) an unreadable file is
    // replaced, so the user is never locked out of fixing it.
    let mut config = match assessor::read_ai_config(dir) {
        Ok(config) => config,
        Err(error) if api_key.is_none() => return Err(error),
        Err(_) => AiConfig::default(),
    };
    config.runner = runner.to_string();
    config.model = model.to_string();
    match api_key {
        None => {}
        Some(key) if key.trim().is_empty() => config.api_key = None,
        Some(key) => config.api_key = Some(key.to_string()),
    }
    assessor::write_ai_config(dir, &config)?;
    Ok(assessor::read_ai_config(dir)?.status())
}

/// The spaces of a ClickUp workspace, for narrowing a project's link. Uses the
/// stored ClickUp token.
pub fn clickup_spaces(
    contributors: &Contributors,
    team_id: &str,
) -> Result<Vec<ClickUpSpace>, String> {
    let team_id = clickup_id(team_id)?;
    let config = clickup::require_config(&contributors.data_dir)?;
    clickup::fetch_spaces_for(&config.token, team_id)
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;
    use std::time::{Duration, Instant};

    use super::super::jobs::JobState;
    use super::super::sync::{Reporter, SyncOutcome};
    use super::super::tests::{TestRepo, ALICE};
    use super::*;

    const SINCE: &str = "2026-01-01T00:00:00Z";
    const UNTIL: &str = "2026-01-31T23:59:59Z";

    #[derive(Default)]
    struct Recording(Mutex<Vec<SyncEvent>>);

    impl EventSink for Recording {
        fn emit(&self, event: &SyncEvent) {
            self.0.lock().unwrap().push(event.clone());
        }
    }

    impl Recording {
        fn has(&self, state: JobState) -> bool {
            self.0.lock().unwrap().iter().any(|e| e.state == state)
        }
    }

    /// Polls `ready` for up to 10 s.
    fn wait_for(what: &str, ready: impl Fn() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !ready() {
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn repo() -> TestRepo {
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

    struct NoTicketEvents;

    impl TicketSink for NoTicketEvents {
        fn emit(&self, _: &ticket_jobs::TicketJobEvent) {}
    }

    #[derive(Default)]
    struct TicketEvents(Mutex<Vec<ticket_jobs::TicketJobEvent>>);

    impl TicketSink for TicketEvents {
        fn emit(&self, event: &ticket_jobs::TicketJobEvent) {
            self.0.lock().unwrap().push(event.clone());
        }
    }

    impl TicketEvents {
        fn ended(&self) -> Option<ticket_jobs::TicketJobEvent> {
            let events = self.0.lock().unwrap();
            let end = |e: &&ticket_jobs::TicketJobEvent| {
                ["done", "error", "cancelled"].contains(&e.state.as_str())
            };
            events.iter().find(end).cloned()
        }
    }

    /// An app over an in-memory database and a private data dir, with the
    /// ticket events recorded. Ticket jobs read ClickUp and the AI settings
    /// from the data dir, so nothing here can reach the network or `claude`
    /// unless a test writes a token and an API key.
    fn ticket_app() -> (Contributors, Arc<TicketEvents>, tempfile::TempDir) {
        let events = Arc::new(TicketEvents::default());
        let db = Arc::new(ContributorsDb::open_in_memory().unwrap());
        let dir = tempfile::tempdir().unwrap();
        let app = Contributors::new(
            db,
            dir.path(),
            Arc::new(Recording::default()),
            events.clone(),
        );
        (app, events, dir)
    }

    fn stored_ticket(id: &str, title: &str) -> ticket_store::StoredTicket {
        ticket_store::StoredTicket {
            source: "clickup".to_string(),
            id: id.to_string(),
            custom_id: None,
            title: title.to_string(),
            url: format!("https://app.clickup.com/t/{id}"),
            workspace_id: "w1".to_string(),
            space_id: "s1".to_string(),
            status: "complete".to_string(),
            status_type: "closed".to_string(),
            updated_at: "2026-01-10T10:00:00.000Z".to_string(),
            updated_epoch: 1_768_039_200,
            done_at: Some("2026-01-10T10:00:00.000Z".to_string()),
            status_since: None,
            parent_id: None,
            assignees: Vec::new(),
            tags: Vec::new(),
            description: "Fix the login flow".to_string(),
        }
    }

    fn seed_tickets(app: &Contributors, tickets: &[ticket_store::StoredTicket]) {
        let conn = app.db().lock_conn().unwrap();
        let tx = conn.unchecked_transaction().unwrap();
        ticket_store::put_tickets(&tx, tickets).unwrap();
        tx.commit().unwrap();
    }

    fn write_clickup_config(dir: &Path) {
        std::fs::write(
            dir.join("clickup-config.json"),
            r#"{"token":"tk","teamId":"w1","teamName":"Team"}"#,
        )
        .unwrap();
    }

    fn app() -> (Contributors, Arc<Recording>) {
        let sink = Arc::new(Recording::default());
        let db = Arc::new(ContributorsDb::open_in_memory().unwrap());
        let dir = tempfile::tempdir().unwrap();
        let app = Contributors::new(db, dir.path(), sink.clone(), Arc::new(NoTicketEvents));
        (app, sink)
    }

    fn cwd(repo: &TestRepo) -> String {
        repo.path().to_string_lossy().into_owned()
    }

    #[test]
    fn an_in_memory_path_opens_one_database_that_reads_see() {
        let repo = repo();
        let cwd = cwd(&repo);
        let sink = Arc::new(Recording::default());
        let dir = tempfile::tempdir().unwrap();
        let app = Contributors::open(
            Path::new(":memory:"),
            dir.path(),
            sink.clone(),
            Arc::new(NoTicketEvents),
        )
        .unwrap();
        sync_start(&app, &cwd, None, SINCE, UNTIL, "full", None).unwrap();
        wait_for("the sync to finish", || sink.has(JobState::Done));
        wait_for("the worker to leave", || sync_status(&app).is_empty());
        assert!(load(&app, &cwd, None, SINCE, UNTIL).unwrap().covered);
        assert_eq!(
            sync_estimate(&app, &cwd, None, SINCE, UNTIL)
                .unwrap()
                .stored_commits,
            2
        );
    }

    #[test]
    fn config_and_mailmap_round_trip_through_the_repo_root() {
        let repo = repo();
        let cwd = cwd(&repo);
        assert_eq!(read_config(&cwd).unwrap(), None);
        assert_eq!(read_mailmap(&cwd).unwrap(), None);
        write_config(&cwd, "{\"ignore\":[]}").unwrap();
        assert_eq!(
            read_config(&cwd).unwrap().as_deref(),
            Some("{\"ignore\":[]}")
        );
        std::fs::write(repo.path().join(".mailmap"), "A <a@x>\n").unwrap();
        assert_eq!(read_mailmap(&cwd).unwrap().as_deref(), Some("A <a@x>\n"));
        let too_big = "x".repeat(1024 * 1024 + 1);
        assert_eq!(
            write_config(&cwd, &too_big).unwrap_err(),
            "The Contributors config is larger than 1 MB."
        );
    }

    #[test]
    fn git_questions_are_answered_for_the_repo() {
        let repo = repo();
        let cwd = cwd(&repo);
        let listed = branches(&cwd).unwrap();
        assert_eq!(listed.current, "main");
        assert_eq!(listed.branches, ["main"]);
        let first = first_commit(&cwd, None).unwrap().unwrap();
        assert!(first.starts_with("2026-01-05T10:00:00"), "{first}");
        assert!(first_commit(&cwd, Some("main")).unwrap().is_some());
        assert!(git_user(&cwd).is_ok());
    }

    #[test]
    fn a_folder_that_is_not_a_repo_is_an_error_everywhere() {
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path().to_string_lossy().into_owned();
        let (app, _) = app();
        assert!(read_config(&cwd).is_err());
        assert!(read_mailmap(&cwd).is_err());
        assert!(write_config(&cwd, "{}").is_err());
        assert!(branches(&cwd).is_err());
        assert!(first_commit(&cwd, None).is_err());
        assert!(git_user(&cwd).is_err());
        assert!(load(&app, &cwd, None, SINCE, UNTIL).is_err());
        assert!(sync_estimate(&app, &cwd, None, SINCE, UNTIL).is_err());
        assert!(sync_start(&app, &cwd, None, SINCE, UNTIL, "full", None).is_err());
        assert!(sync_cancel(&app, &cwd).is_err());
        assert!(clear(&app, &cwd).is_err());
        assert!(sync_status(&app).is_empty());
    }

    #[test]
    fn sync_start_validates_the_request_before_queueing() {
        let repo = repo();
        let cwd = cwd(&repo);
        let (app, sink) = app();
        let start =
            |branch, from, to, mode, head| sync_start(&app, &cwd, branch, from, to, mode, head);
        assert_eq!(
            start(None, SINCE, UNTIL, "weekly", None).unwrap_err(),
            "Unknown sync mode: weekly"
        );
        assert_eq!(
            start(None, SINCE, UNTIL, "topup", None).unwrap_err(),
            "A top-up needs the head the period was synced at."
        );
        assert_eq!(
            start(None, SINCE, UNTIL, "topup", Some("  ")).unwrap_err(),
            "A top-up needs the head the period was synced at."
        );
        assert!(start(None, "not a date", UNTIL, "full", None).is_err());
        assert!(start(None, UNTIL, SINCE, "full", None).is_err());
        assert!(start(Some("no-such-branch"), SINCE, UNTIL, "full", None).is_err());
        assert!(sync_status(&app).is_empty());
        assert!(sink.0.lock().unwrap().is_empty());
    }

    #[test]
    fn a_sync_loads_estimates_and_clears_through_the_facade() {
        let repo = repo();
        let cwd = cwd(&repo);
        let (app, sink) = app();

        let before = load(&app, &cwd, None, SINCE, UNTIL).unwrap();
        assert!(!before.covered);
        assert_eq!(before.missing_commits, 2);
        let estimate = sync_estimate(&app, &cwd, None, SINCE, UNTIL).unwrap();
        assert_eq!((estimate.commits, estimate.stored_commits), (2, 0));

        sync_start(&app, &cwd, None, SINCE, UNTIL, "full", None).unwrap();
        wait_for("the sync to finish", || sink.has(JobState::Done));
        wait_for("the worker to leave", || sync_status(&app).is_empty());
        let events = sink.0.lock().unwrap().clone();
        assert_eq!(events[0].state, JobState::Queued);
        assert_eq!(events[0].root, cwd);
        assert_eq!(events[0].branch, "main");

        let after = load(&app, &cwd, None, SINCE, UNTIL).unwrap();
        assert!(after.covered);
        assert_eq!(after.raw.commits.len(), 2);
        let estimate = sync_estimate(&app, &cwd, None, SINCE, UNTIL).unwrap();
        assert_eq!(estimate.stored_commits, 2);

        clear(&app, &cwd).unwrap();
        let cleared = load(&app, &cwd, None, SINCE, UNTIL).unwrap();
        assert!(!cleared.covered);
        assert_eq!(cleared.missing_commits, 2);
    }

    #[test]
    fn clear_is_refused_while_a_sync_runs_and_cancel_ends_it() {
        let repo = repo();
        let cwd = cwd(&repo);
        let sink = Arc::new(Recording::default());
        let db = Arc::new(ContributorsDb::open_in_memory().unwrap());
        let until_cancelled = |_: &jobs::JobSpec, reporter: &dyn Reporter| {
            while !reporter.cancelled() {
                std::thread::sleep(Duration::from_millis(2));
            }
            Ok(SyncOutcome {
                cancelled: true,
                gh_state: "complete".to_string(),
                gh_note: None,
            })
        };
        let dir = tempfile::tempdir().unwrap();
        let deps = DbDeps {
            db: db.clone(),
            data_dir: dir.path().to_path_buf(),
        };
        let app = Contributors::with_parts(
            db,
            dir.path(),
            until_cancelled,
            sink.clone(),
            deps,
            Arc::new(NoTicketEvents),
        );

        sync_start(&app, &cwd, None, SINCE, UNTIL, "full", None).unwrap();
        wait_for("the job to run", || sink.has(JobState::Running));
        assert_eq!(sync_status(&app).len(), 1);
        assert_eq!(
            clear(&app, &cwd).unwrap_err(),
            "A sync of this repository is running. Cancel it first."
        );

        sync_cancel(&app, &cwd).unwrap();
        wait_for("the job to be cancelled", || sink.has(JobState::Cancelled));
        wait_for("clear to be allowed", || clear(&app, &cwd).is_ok());
    }

    // ---- ticket scoring ----

    #[test]
    fn ticket_period_parses_both_edges_inclusive() {
        let (from, to) = ticket_period(" 2026-01-01 ", "2026-01-31").unwrap();
        assert_eq!(from, window_epoch("2026-01-01", false).unwrap());
        assert_eq!(to, window_epoch("2026-01-31", true).unwrap());
        assert!(
            to - from > 30 * 86_400,
            "a bare end date is the end of its day"
        );
        assert_eq!(
            ticket_period("2026-01-01", "2026-01-01").map(|(f, t)| t > f),
            Ok(true)
        );
        assert_eq!(
            ticket_period("2026-02-01", "2026-01-31").unwrap_err(),
            "The start date is after the end date."
        );
        assert_eq!(
            ticket_period("nope", "2026-01-31").unwrap_err(),
            "Invalid start date: nope"
        );
        assert_eq!(
            ticket_period("2026-01-01", "later").unwrap_err(),
            "Invalid end date: later"
        );
    }

    #[test]
    fn clickup_ids_keep_to_path_safe_characters() {
        assert_eq!(clickup_id(" 9012-ab_C ").unwrap(), "9012-ab_C");
        for bad in ["", "  ", "a/b", "a b", "a?x=1", "..", "a%2Fb", "t\u{e9}am"] {
            assert_eq!(
                clickup_id(bad).unwrap_err(),
                format!("Invalid ClickUp id: {}", bad.trim()),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn tickets_fetch_validates_before_queueing() {
        let (app, events, _dir) = ticket_app();
        let link = |workspace: &str, spaces: Option<&[&str]>| TicketLink {
            workspace_id: workspace.to_string(),
            space_ids: spaces.map(|s| s.iter().map(|id| id.to_string()).collect()),
        };
        let fetch = |links| tickets_fetch(&app, links, "2026-01-01", "2026-01-31");
        assert_eq!(
            tickets_fetch(&app, vec![link("w1", None)], "x", "2026-01-31").unwrap_err(),
            "Invalid start date: x"
        );
        assert_eq!(
            fetch(vec![link("w/1", None)]).unwrap_err(),
            "Invalid ClickUp id: w/1"
        );
        assert_eq!(
            fetch(vec![link("w1", Some(&["s1", "s 2"]))]).unwrap_err(),
            "Invalid ClickUp id: s 2"
        );
        assert!(tickets_status(&app).event.is_none());
        assert!(events.0.lock().unwrap().is_empty());
    }

    #[test]
    fn a_fetch_without_a_clickup_token_ends_in_an_error_event() {
        let (app, events, _dir) = ticket_app();
        let link = TicketLink {
            workspace_id: "w1".to_string(),
            space_ids: Some(vec!["s1".to_string()]),
        };
        let id = tickets_fetch(&app, vec![link], "2026-01-01", "2026-01-31").unwrap();
        wait_for("the fetch to end", || events.ended().is_some());
        let end = events.ended().unwrap();
        assert_eq!((end.kind.as_str(), end.state.as_str()), ("fetch", "error"));
        assert_eq!(end.job_id, id);
        assert_eq!(end.error.as_deref(), Some("Connect ClickUp in Settings"));
        wait_for("the worker to leave", || {
            tickets_status(&app)
                .event
                .is_none_or(|e| e.state != "running")
        });
    }

    #[test]
    fn an_assessment_reads_the_ai_settings_of_the_data_dir() {
        let (app, events, dir) = ticket_app();
        seed_tickets(&app, &[stored_ticket("a", "Title a")]);
        write_clickup_config(dir.path());
        ai_config_set(&app, "api", "claude-opus-5-5", None).unwrap();
        tickets_assess(&app, vec!["a".to_string()], false).unwrap();
        wait_for("the assessment to end", || events.ended().is_some());
        let end = events.ended().unwrap();
        assert_eq!((end.kind.as_str(), end.state.as_str()), ("assess", "error"));
        assert_eq!(
            end.error.as_deref(),
            Some("Add an Anthropic API key in Settings to assess with the API.")
        );
    }

    #[test]
    fn stored_tickets_load_estimate_and_take_overrides() {
        let (app, _events, _dir) = ticket_app();
        seed_tickets(
            &app,
            &[stored_ticket("a", "Title a"), stored_ticket("b", "Title b")],
        );
        let workspaces = ["w1".to_string()];
        let loaded = tickets_load(&app, &workspaces, "2026-01-01").unwrap();
        let ids: Vec<&str> = loaded.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(ids, ["a", "b"]);
        assert!(tickets_load(&app, &workspaces, "2026-02-01")
            .unwrap()
            .is_empty());
        assert_eq!(
            tickets_load(&app, &workspaces, "soon").unwrap_err(),
            "Invalid start date: soon"
        );

        let ids = ["a".to_string(), "b".to_string()];
        let estimate = tickets_estimate(&app, &ids).unwrap();
        assert_eq!(estimate.tickets, 2);
        assert!(estimate.input_tokens > 0);

        // A manual level takes the ticket out of the estimate; a blank note is dropped.
        ticket_override(&app, "a", Some("M"), Some("  ")).unwrap();
        let loaded = tickets_load(&app, &workspaces, "2026-01-01").unwrap();
        assert_eq!(loaded[0].override_level.as_deref(), Some("M"));
        assert_eq!(loaded[0].override_note, None);
        assert_eq!(tickets_estimate(&app, &ids).unwrap().tickets, 1);
        ticket_override(&app, "a", Some("L"), Some(" because ")).unwrap();
        let loaded = tickets_load(&app, &workspaces, "2026-01-01").unwrap();
        assert_eq!(loaded[0].override_note.as_deref(), Some("because"));
        assert_eq!(
            ticket_override(&app, "a", Some("HUGE"), None).unwrap_err(),
            "Unknown ticket level: HUGE"
        );
        ticket_override(&app, "a", None, Some("ignored")).unwrap();
        let loaded = tickets_load(&app, &workspaces, "2026-01-01").unwrap();
        assert_eq!(loaded[0].override_level, None);
        assert_eq!(tickets_estimate(&app, &ids).unwrap().tickets, 2);
    }

    #[test]
    fn window_coverage_follows_the_fetches_and_validates_the_period() {
        let (app, _events, _dir) = ticket_app();
        let covered = |spaces: Option<&[String]>| {
            tickets_window_covered(&app, "w1", spaces, "2026-01-01", "2026-01-31")
        };
        assert_eq!(covered(None), Ok(false));
        {
            let conn = app.db().lock_conn().unwrap();
            let tx = conn.unchecked_transaction().unwrap();
            let from = window_epoch("2026-01-01", false).unwrap();
            let until = window_epoch("2026-01-31", true).unwrap();
            let spaces_key = ticket_store::spaces_key(None);
            ticket_store::mark_covered(&tx, "w1", &spaces_key, from, until, until).unwrap();
            tx.commit().unwrap();
        }
        assert_eq!(covered(None), Ok(true));
        assert_eq!(
            tickets_window_covered(&app, "w1", None, "2026-01-01", "2026-03-01"),
            Ok(false)
        );
        assert_eq!(
            tickets_window_covered(&app, "w1", None, "2026-02-01", "2026-01-01").unwrap_err(),
            "The start date is after the end date."
        );
        assert_eq!(
            tickets_window_covered(&app, "w2", None, "2026-01-01", "2026-01-31"),
            Ok(false)
        );
    }

    #[test]
    fn a_window_that_has_not_ended_is_covered_up_to_now() {
        let (app, _events, _dir) = ticket_app();
        let from = window_epoch("2020-01-01", false).unwrap();
        let now = now_epoch();
        {
            let conn = app.db().lock_conn().unwrap();
            let tx = conn.unchecked_transaction().unwrap();
            let all = ticket_store::spaces_key(None);
            ticket_store::mark_covered(&tx, "w1", &all, from, now, now).unwrap();
            tx.commit().unwrap();
        }
        let covered = |to| tickets_window_covered(&app, "w1", None, "2020-01-01", to);
        assert_eq!(covered("2099-12-31"), Ok(true));
        assert_eq!(
            covered("2019-12-31").unwrap_err(),
            "The start date is after the end date."
        );
    }

    #[test]
    fn the_ai_config_hides_the_key_and_keeps_deletes_or_replaces_it() {
        let (app, _events, dir) = ticket_app();
        let file = dir.path().join("contributors-ai.json");
        let status = |s: &AiConfigStatus| serde_json::to_value(s).unwrap();

        let fresh = ai_config_get(&app).unwrap();
        assert_eq!(
            status(&fresh),
            serde_json::json!({"runner":"claude-code","model":"claude-opus-5-5","hasApiKey":false})
        );

        let set = ai_config_set(&app, "api", " claude-haiku-5-5 ", Some(" sk-secret ")).unwrap();
        assert_eq!(
            status(&set),
            serde_json::json!({"runner":"api","model":"claude-haiku-5-5","hasApiKey":true})
        );
        assert!(!status(&set).to_string().contains("sk-secret"));
        let saved: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
        assert_eq!(saved["apiKey"], "sk-secret");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&file).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }

        // None keeps the key.
        let kept = ai_config_set(&app, "claude-code", "claude-opus-5-5", None).unwrap();
        assert!(kept.has_api_key);
        assert_eq!(kept.runner, "claude-code");
        // A new key replaces it.
        ai_config_set(&app, "api", "m", Some("sk-other")).unwrap();
        let saved: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
        assert_eq!(saved["apiKey"], "sk-other");
        // A blank string deletes it.
        let deleted = ai_config_set(&app, "api", "m", Some("  ")).unwrap();
        assert!(!deleted.has_api_key);
        let saved: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
        assert!(saved.get("apiKey").is_none() || saved["apiKey"].is_null());
        // An unknown runner falls back to the default.
        let fallback = ai_config_set(&app, "gpt", "", None).unwrap();
        assert_eq!(
            (fallback.runner.as_str(), fallback.model.as_str()),
            ("claude-code", "claude-opus-5-5")
        );
    }

    #[test]
    fn the_ai_config_can_be_read_and_saved_by_data_directory_alone() {
        let (_app, _events, dir) = ticket_app();
        let set = ai_config_set_at(dir.path(), "api", "m", Some("sk-x")).unwrap();
        assert_eq!(
            (set.runner.as_str(), set.model.as_str(), set.has_api_key),
            ("api", "m", true)
        );
        assert_eq!(ai_config_get_at(dir.path()).unwrap(), set);
        let removed = ai_config_set_at(dir.path(), "api", "m", Some("")).unwrap();
        assert!(!removed.has_api_key);
    }

    #[test]
    fn an_unreadable_ai_config_is_not_overwritten_unless_the_key_is_given() {
        let (app, _events, dir) = ticket_app();
        let file = dir.path().join("contributors-ai.json");
        std::fs::write(&file, "not json").unwrap();
        let invalid = "The assessment AI settings are invalid";
        assert_eq!(ai_config_get(&app).unwrap_err(), invalid);
        assert_eq!(ai_config_set(&app, "api", "m", None).unwrap_err(), invalid);
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "not json");
        let fixed = ai_config_set(&app, "api", "m", Some("sk-new")).unwrap();
        assert!(fixed.has_api_key);
        let again = {
            std::fs::write(&file, "not json").unwrap();
            ai_config_set(&app, "api", "m", Some("")).unwrap()
        };
        assert!(!again.has_api_key);
    }

    #[test]
    fn clickup_spaces_validates_the_id_and_needs_a_token() {
        let (app, _events, _dir) = ticket_app();
        assert_eq!(
            clickup_spaces(&app, "a/b").unwrap_err(),
            "Invalid ClickUp id: a/b"
        );
        assert_eq!(
            clickup_spaces(&app, "w1").unwrap_err(),
            "Connect ClickUp in Settings"
        );
    }

    #[test]
    fn cancel_and_status_with_no_ticket_job_are_quiet() {
        let (app, events, _dir) = ticket_app();
        tickets_cancel(&app);
        let status = tickets_status(&app);
        assert!(status.event.is_none());
        assert_eq!(status.pending_fetches, 0);
        assert!(events.0.lock().unwrap().is_empty());
    }
}
