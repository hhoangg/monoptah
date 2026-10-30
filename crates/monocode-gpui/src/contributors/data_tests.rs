//! The rules of `useContributors` (useContributors.test.ts) as tests of the
//! native state: a fake backend stands in for git, `gh` and the preferences,
//! and the clock is a cell the tests move. Nothing here touches a real
//! repository, the network or the user's files.
use super::{
    backend::Backend,
    data::{Clock, Snapshot, Status, SyncState, origin_note_text},
    github::GithubView,
    jobs::SyncJobs,
};
use chrono::{DateTime, Local, TimeZone};
use gpui::{AppContext as _, Context, Entity, TestAppContext};
use monocode_core::contributors::{
    ContributorsBranches, GitIdentity, GitUser, RawCommit, RawData, RawFile,
    fetch::FetchOutcome,
    jobs::{JobState, LoadResponse, SyncEvent},
    model::{
        config::{DEFAULT_CONFIG, serialize_config},
        period::{Period, PeriodKind, PeriodPick, current_period, previous_period, sync_window},
        prefs::{read_merge, read_period_pick, serialize_pick},
        sync_jobs::{StartCall, epoch_seconds, iso_seconds},
    },
    sync::{Estimate, SyncStep},
};
use std::{
    cell::Cell,
    collections::VecDeque,
    rc::Rc,
    sync::{Arc, Mutex},
    time::Duration,
};

const CWD: &str = "/repo-1";

// ---- the fake backend ---------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
pub(super) enum Call {
    Config,
    Mailmap,
    Branches,
    User,
    First(Option<String>),
    Load {
        branch: Option<String>,
        since: String,
        until: String,
    },
    Estimate {
        branch: Option<String>,
        from: String,
        to: String,
    },
    Fetch(Option<String>),
    Start(StartCall),
    Cancel,
    Clear,
    Write(String),
}

/// Answers in order, then the default.
pub(super) struct Script<T: Clone> {
    pub(super) once: VecDeque<T>,
    pub(super) default: T,
}
impl<T: Clone> Script<T> {
    fn new(default: T) -> Self {
        Self {
            once: VecDeque::new(),
            default,
        }
    }
    fn next(&mut self) -> T {
        self.once
            .pop_front()
            .unwrap_or_else(|| self.default.clone())
    }
}

pub(super) struct State {
    pub(super) calls: Vec<Call>,
    pub(super) config: Script<Result<Option<String>, String>>,
    pub(super) mailmap: Script<Result<Option<String>, String>>,
    pub(super) branches: Script<Result<ContributorsBranches, String>>,
    pub(super) user: Script<Result<GitUser, String>>,
    pub(super) first: Script<Result<Option<String>, String>>,
    pub(super) load: Script<Result<LoadResponse, String>>,
    pub(super) estimate: Script<Result<Estimate, String>>,
    pub(super) write: Script<Result<(), String>>,
    pub(super) fetch: Script<Result<FetchOutcome, String>>,
    pub(super) start: Script<Result<(), String>>,
    pub(super) clear: Script<Result<(), String>>,
    pub(super) cancel: Script<Result<(), String>>,
    /// What the core would list as running.
    pub(super) listed: Vec<SyncEvent>,
    pub(super) saved_range: Option<String>,
    pub(super) saved_merge: Option<String>,
}

pub(super) struct Fake(Mutex<State>);

fn load_ok(response: LoadResponse) -> Result<LoadResponse, String> {
    Ok(response)
}

impl Fake {
    pub(super) fn new() -> Arc<Self> {
        Arc::new(Self(Mutex::new(State {
            calls: Vec::new(),
            config: Script::new(Ok(None)),
            mailmap: Script::new(Ok(None)),
            branches: Script::new(Ok(ContributorsBranches {
                current: "main".into(),
                branches: vec!["main".into(), "dev".into()],
            })),
            user: Script::new(Ok(GitUser {
                name: "Ana".into(),
                email: "ana@example.com".into(),
            })),
            first: Script::new(Ok(Some(first_commit().to_rfc3339()))),
            load: Script::new(load_ok(synced("h1"))),
            estimate: Script::new(Ok(ESTIMATE)),
            write: Script::new(Ok(())),
            fetch: Script::new(Ok(FetchOutcome::Skipped)),
            start: Script::new(Ok(())),
            clear: Script::new(Ok(())),
            cancel: Script::new(Ok(())),
            listed: Vec::new(),
            saved_range: None,
            saved_merge: None,
        })))
    }

    pub(super) fn with<R>(&self, f: impl FnOnce(&mut State) -> R) -> R {
        f(&mut self.0.lock().unwrap())
    }

    pub(super) fn calls(&self) -> Vec<Call> {
        self.with(|s| s.calls.clone())
    }

    pub(super) fn count(&self, pred: impl Fn(&Call) -> bool) -> usize {
        self.with(|s| s.calls.iter().filter(|c| pred(c)).count())
    }

    pub(super) fn loads(&self) -> Vec<Call> {
        self.calls()
            .into_iter()
            .filter(|c| matches!(c, Call::Load { .. }))
            .collect()
    }

    pub(super) fn starts(&self) -> Vec<StartCall> {
        self.calls()
            .into_iter()
            .filter_map(|c| match c {
                Call::Start(call) => Some(call),
                _ => None,
            })
            .collect()
    }

    pub(super) fn set_load(&self, response: LoadResponse) {
        self.with(|s| s.load.default = Ok(response));
    }

    pub(super) fn list(&self, event: &SyncEvent) {
        self.with(|s| {
            s.listed.retain(|other| {
                !(other.root == event.root && other.from == event.from && other.to == event.to)
            });
            if matches!(event.state, JobState::Queued | JobState::Running) {
                s.listed.push(event.clone());
            }
        });
    }
}

impl Backend for Fake {
    fn read_config(&self, _: &str) -> Result<Option<String>, String> {
        self.with(|s| {
            s.calls.push(Call::Config);
            s.config.next()
        })
    }
    fn write_config(&self, _: &str, content: &str) -> Result<(), String> {
        self.with(|s| {
            s.calls.push(Call::Write(content.into()));
            s.write.next()
        })
    }
    fn read_mailmap(&self, _: &str) -> Result<Option<String>, String> {
        self.with(|s| {
            s.calls.push(Call::Mailmap);
            s.mailmap.next()
        })
    }
    fn branches(&self, _: &str) -> Result<ContributorsBranches, String> {
        self.with(|s| {
            s.calls.push(Call::Branches);
            s.branches.next()
        })
    }
    fn git_user(&self, _: &str) -> Result<GitUser, String> {
        self.with(|s| {
            s.calls.push(Call::User);
            s.user.next()
        })
    }
    fn first_commit(&self, _: &str, branch: Option<&str>) -> Result<Option<String>, String> {
        self.with(|s| {
            s.calls.push(Call::First(branch.map(String::from)));
            s.first.next()
        })
    }
    fn load(
        &self,
        _: &str,
        branch: Option<&str>,
        since: &str,
        until: &str,
    ) -> Result<LoadResponse, String> {
        self.with(|s| {
            s.calls.push(Call::Load {
                branch: branch.map(String::from),
                since: since.into(),
                until: until.into(),
            });
            s.load.next()
        })
    }
    fn estimate(
        &self,
        _: &str,
        branch: Option<&str>,
        from: &str,
        to: &str,
    ) -> Result<Estimate, String> {
        self.with(|s| {
            s.calls.push(Call::Estimate {
                branch: branch.map(String::from),
                from: from.into(),
                to: to.into(),
            });
            s.estimate.next()
        })
    }
    fn fetch_origin(&self, _: &str, branch: Option<&str>) -> Result<FetchOutcome, String> {
        self.with(|s| {
            s.calls.push(Call::Fetch(branch.map(String::from)));
            s.fetch.next()
        })
    }
    fn start(&self, cwd: &str, call: &StartCall) -> Result<(), String> {
        self.with(|s| {
            s.calls.push(Call::Start(call.clone()));
            let answer = s.start.next();
            if answer.is_ok() {
                let secs = |text: &str| {
                    DateTime::parse_from_rfc3339(text)
                        .map(|t| t.timestamp())
                        .unwrap_or(0)
                };
                s.listed.push(event(
                    cwd,
                    (secs(&call.from), secs(&call.to)),
                    JobState::Queued,
                ));
            }
            answer
        })
    }
    fn cancel(&self, _: &str) -> Result<(), String> {
        self.with(|s| {
            s.calls.push(Call::Cancel);
            s.cancel.next()
        })
    }
    fn clear(&self, _: &str) -> Result<(), String> {
        self.with(|s| {
            s.calls.push(Call::Clear);
            s.clear.next()
        })
    }
    fn status(&self) -> Vec<SyncEvent> {
        self.with(|s| s.listed.clone())
    }
    fn load_prefs(&self, now: &DateTime<Local>) -> (PeriodPick, bool) {
        self.with(|s| {
            (
                read_period_pick(s.saved_range.as_deref(), None, now),
                read_merge(s.saved_merge.as_deref(), None),
            )
        })
    }
    fn save_pick(&self, pick: &PeriodPick) {
        self.with(|s| s.saved_range = Some(serialize_pick(pick)));
    }
    fn save_merge(&self, merge: bool) {
        self.with(|s| s.saved_merge = Some(merge.to_string()));
    }
}

// ---- fixtures ----------------------------------------------------------

pub(super) const ESTIMATE: Estimate = Estimate {
    commits: 40,
    stored_commits: 0,
    first_parent: 30,
    prs: Some(9),
};

pub(super) fn local(y: i32, m: u32, d: u32, h: u32, mi: u32) -> DateTime<Local> {
    Local
        .with_ymd_and_hms(y, m, d, h, mi, 0)
        .earliest()
        .unwrap()
}

pub(super) fn first_commit() -> DateTime<Local> {
    local(2023, 5, 17, 15, 40)
}

pub(super) fn first_day() -> DateTime<Local> {
    local(2023, 5, 17, 0, 0)
}

pub(super) fn ana() -> GitIdentity {
    GitIdentity {
        name: "Ana".into(),
        email: "ana@example.com".into(),
        login: None,
    }
}

pub(super) fn raw(head: &str, now: &DateTime<Local>) -> RawData {
    let at = (*now - chrono::Duration::hours(1))
        .with_timezone(&chrono::Utc)
        .to_rfc3339();
    RawData {
        head: head.into(),
        branch: "main".into(),
        since: String::new(),
        until: String::new(),
        gh_available: true,
        gh_note: None,
        commits: vec![RawCommit {
            hash: "a1".into(),
            subject: "work".into(),
            author: ana(),
            committer: ana(),
            authored_at: at.clone(),
            committed_at: at,
            co_authors: vec![],
            files: vec![RawFile {
                path: "src/a.ts".into(),
                added: 3,
                deleted: 0,
            }],
            pr_id: None,
        }],
        prs: vec![],
        reviews: vec![],
        survival: vec![],
        reverted_pr_ids: vec![],
        truncated: false,
    }
}

pub(super) fn start_of_tests() -> DateTime<Local> {
    local(2026, 10, 15, 10, 0)
}

/// A period the store has fully synced, at the same head as the branch.
pub(super) fn synced(head: &str) -> LoadResponse {
    LoadResponse {
        raw: raw(head, &start_of_tests()),
        covered: true,
        has_gaps: false,
        missing_commits: 0,
        synced_at: Some(start_of_tests().timestamp() - 300),
        coverage_head: Some(head.into()),
        head: head.into(),
        gh_state: Some("complete".into()),
    }
}

/// A period the store has nothing (or not everything) for.
pub(super) fn unsynced() -> LoadResponse {
    let mut raw = raw("h1", &start_of_tests());
    raw.commits.clear();
    LoadResponse {
        raw,
        covered: false,
        has_gaps: true,
        missing_commits: 40,
        synced_at: None,
        coverage_head: None,
        head: "h1".into(),
        gh_state: None,
    }
}

pub(super) fn event(root: &str, window: (i64, i64), state: JobState) -> SyncEvent {
    SyncEvent {
        repo_key: "/repo/.git".into(),
        project: "repo".into(),
        root: root.into(),
        branch: "main".into(),
        from: window.0,
        to: window.1,
        step: SyncStep::Commits,
        done: 0,
        total: 0,
        percent: 0,
        state,
        error: None,
    }
}

/// The window the page syncs for a period, in epoch seconds.
pub(super) fn window_of(period: &Period, now: &DateTime<Local>) -> (i64, i64) {
    (
        epoch_seconds(&sync_window(period, now).from),
        epoch_seconds(&period.end),
    )
}

pub(super) fn seven_days() -> Period {
    current_period(PeriodKind::Days7, &start_of_tests(), None)
}

pub(super) fn month_back(n: usize) -> Period {
    let mut period = current_period(PeriodKind::Month, &start_of_tests(), None);
    for _ in 0..n {
        period = previous_period(&period).unwrap();
    }
    period
}

pub(super) fn saved(period: &Period) -> String {
    format!("{}:{}", period.kind.as_str(), period.id)
}

fn load_call(period: &Period, now: &DateTime<Local>, branch: Option<&str>) -> Call {
    Call::Load {
        branch: branch.map(String::from),
        since: iso_seconds(&sync_window(period, now).from),
        until: iso_seconds(&period.end),
    }
}

// ---- the rig -----------------------------------------------------------

pub(super) struct Rig {
    jobs: Entity<SyncJobs>,
    view: Entity<GithubView>,
    fake: Arc<Fake>,
    clock: Rc<Cell<DateTime<Local>>>,
}

impl Rig {
    /// A view on a project, wired to the fake, and settled.
    fn new(cx: &mut TestAppContext) -> Self {
        Self::with(cx, Fake::new(), start_of_tests(), false)
    }

    fn with(cx: &mut TestAppContext, fake: Arc<Fake>, now: DateTime<Local>, hold: bool) -> Self {
        let clock = Rc::new(Cell::new(now));
        let source = clock.clone();
        let read: Clock = Rc::new(move || source.get());
        let jobs = cx.new(SyncJobs::new);
        let view = cx.new({
            let jobs = jobs.clone();
            move |cx| GithubView::new(CWD.into(), jobs, read, cx)
        });
        let backend: Arc<dyn Backend> = fake.clone();
        view.update(cx, |view, cx| {
            if hold {
                view.hold_backend_calls();
            }
            view.set_backend(backend, cx);
        });
        cx.run_until_parked();
        Self {
            jobs,
            view,
            fake,
            clock,
        }
    }

    fn snap(&self, cx: &mut TestAppContext) -> Snapshot {
        self.view.read_with(cx, |view, cx| view.snapshot(cx))
    }

    fn act<R>(
        &self,
        cx: &mut TestAppContext,
        f: impl FnOnce(&mut GithubView, &mut Context<GithubView>) -> R,
    ) -> R {
        let out = self.view.update(cx, f);
        cx.run_until_parked();
        out
    }

    /// Stops answering backend calls until the test releases them.
    fn hold(&self, cx: &mut TestAppContext) {
        self.view.update(cx, |view, _| view.hold_backend_calls());
    }

    /// Holds every answer, the fetch of origin included.
    fn hold_fetch(&self, cx: &mut TestAppContext) {
        self.view.update(cx, |view, _| view.hold_fetch_answers());
    }

    fn running(&self, cx: &mut TestAppContext) -> usize {
        self.view.read_with(cx, |view, _| view.running)
    }

    fn fetches(&self) -> Vec<Call> {
        self.fake
            .calls()
            .into_iter()
            .filter(|c| matches!(c, Call::Fetch(_)))
            .collect()
    }

    fn release(&self, cx: &mut TestAppContext, label: &str) {
        self.view.update(cx, |view, cx| view.release(label, cx));
        cx.run_until_parked();
    }

    fn release_nth(&self, cx: &mut TestAppContext, label: &str, nth: usize) {
        self.view
            .update(cx, |view, cx| view.release_nth(label, nth, cx));
        cx.run_until_parked();
    }

    fn release_all(&self, cx: &mut TestAppContext) {
        self.view.update(cx, |view, cx| view.release_all(cx));
        cx.run_until_parked();
    }

    fn held(&self, cx: &mut TestAppContext) -> Vec<&'static str> {
        self.view.read_with(cx, |view, _| view.held_labels())
    }

    /// What the core would send: it lists the job and tells the page.
    fn emit(&self, cx: &mut TestAppContext, event: SyncEvent) {
        self.fake.list(&event);
        self.jobs.update(cx, |jobs, cx| jobs.apply_sync(event, cx));
        cx.run_until_parked();
    }

    fn event(&self, over: impl FnOnce(&mut SyncEvent)) -> SyncEvent {
        let now = self.clock.get();
        let mut e = event(
            CWD,
            window_of(&current_period(PeriodKind::Days7, &now, None), &now),
            JobState::Running,
        );
        over(&mut e);
        e
    }

    fn set_clock(&self, now: DateTime<Local>) {
        self.clock.set(now);
    }

    fn resume(&self, cx: &mut TestAppContext) {
        self.act(cx, |view, cx| view.resume(cx));
    }

    fn pick_month_back(&self, cx: &mut TestAppContext, n: usize) {
        self.act(cx, |view, cx| view.set_kind(PeriodKind::Month, cx));
        for _ in 0..n {
            self.act(cx, |view, cx| view.go_previous(cx));
        }
    }
}

// ---- the tests ----------------------------------------------------------

#[gpui::test]
fn goes_from_loading_to_ready_with_raw_data_branches_and_a_model(cx: &mut TestAppContext) {
    let rig = Rig::with(cx, Fake::new(), start_of_tests(), true);
    assert_eq!(rig.held(cx), ["prefs"]);
    assert_eq!(rig.snap(cx).status, Status::Loading);
    rig.release(cx, "prefs");
    assert_eq!(rig.held(cx), ["meta", "period"]);
    let snap = rig.snap(cx);
    assert_eq!(snap.status, Status::Loading);
    assert!(snap.model.is_none());
    rig.release_all(cx);
    let snap = rig.snap(cx);
    assert_eq!(snap.status, Status::Ready);
    assert_eq!(snap.raw.as_ref().unwrap().commits.len(), 1);
    assert_eq!(snap.branches, ["main", "dev"]);
    assert_eq!(snap.branch, "main");
    let model = snap.model.unwrap();
    assert_eq!(model.stats.kpis.commits.value, 1);
    assert_eq!(model.me_id.as_deref(), Some("ana@example.com"));
    assert_eq!(*snap.config, *DEFAULT_CONFIG);
}

#[gpui::test]
fn loads_the_default_7d_period_from_the_start_of_the_previous_7_days_to_the_period_end(
    cx: &mut TestAppContext,
) {
    let rig = Rig::new(cx);
    let now = start_of_tests();
    assert_eq!(rig.fake.loads(), [load_call(&seven_days(), &now, None)]);
    let snap = rig.snap(cx);
    assert_eq!(snap.period.kind, PeriodKind::Days7);
    assert_eq!(snap.model.unwrap().period.id, "7d");
}

#[gpui::test]
fn loads_a_past_month_from_the_start_of_the_month_before_it_to_its_end(cx: &mut TestAppContext) {
    let rig = Rig::new(cx);
    rig.pick_month_back(cx, 3);
    let (a, b) = (month_back(3), month_back(4));
    let Some(Call::Load { since, until, .. }) = rig.fake.loads().pop() else {
        panic!("no load");
    };
    assert_eq!(since, iso_seconds(&b.start));
    assert_eq!(until, iso_seconds(&a.end));
    assert_eq!(rig.snap(cx).model.unwrap().period.id, a.id);
}

#[gpui::test]
fn loads_the_current_month_from_the_start_of_the_previous_one_to_its_end(cx: &mut TestAppContext) {
    let rig = Rig::new(cx);
    rig.act(cx, |view, cx| view.set_kind(PeriodKind::Month, cx));
    let Some(Call::Load { since, until, .. }) = rig.fake.loads().pop() else {
        panic!("no load");
    };
    assert_eq!(until, iso_seconds(&month_back(0).end));
    assert_eq!(since, iso_seconds(&month_back(1).start));
}

#[gpui::test]
fn loads_again_when_the_period_changes_and_not_when_it_does_not(cx: &mut TestAppContext) {
    let rig = Rig::new(cx);
    rig.act(cx, |view, cx| view.set_kind(PeriodKind::Month, cx));
    let base = rig.fake.loads().len();
    rig.act(cx, |view, cx| view.set_kind(PeriodKind::Month, cx));
    assert_eq!(rig.fake.loads().len(), base);
    rig.act(cx, |view, cx| view.go_previous(cx));
    assert_eq!(rig.fake.loads().len(), base + 1);
}

#[gpui::test]
fn restores_a_saved_period_and_a_saved_removed_range_falls_back_to_7d(cx: &mut TestAppContext) {
    let fake = Fake::new();
    fake.with(|s| s.saved_range = Some(saved(&month_back(3))));
    let rig = Rig::with(cx, fake, start_of_tests(), false);
    assert_eq!(rig.snap(cx).period.id, month_back(3).id);
    for removed in ["1y", "30d", "custom:2026-10-01:2026-10-09"] {
        let fake = Fake::new();
        fake.with(|s| s.saved_range = Some(removed.into()));
        let rig = Rig::with(cx, fake, start_of_tests(), false);
        let snap = rig.snap(cx);
        assert_eq!(snap.period.kind, PeriodKind::Days7, "{removed}");
        assert_eq!(snap.period.id, "7d", "{removed}");
    }
}

#[gpui::test]
fn shows_cached_data_at_once_when_the_page_is_opened_again(cx: &mut TestAppContext) {
    let rig = Rig::new(cx);
    assert_eq!(rig.snap(cx).status, Status::Ready);
    rig.hold(cx);
    // Opening the page again revalidates behind the data that is already there.
    rig.act(cx, |view, cx| view.refresh(false, cx));
    let snap = rig.snap(cx);
    assert_eq!(snap.status, Status::Ready);
    assert!(snap.refreshing);
    rig.release_all(cx);
    assert!(!rig.snap(cx).refreshing);
}

#[gpui::test]
fn never_shows_a_stale_not_synced_when_a_job_ended_while_the_page_was_closed(
    cx: &mut TestAppContext,
) {
    let fake = Fake::new();
    fake.set_load(unsynced());
    let rig = Rig::with(cx, fake, start_of_tests(), false);
    assert_eq!(rig.snap(cx).sync_state, SyncState::NotSynced);
    let estimates = rig.fake.count(|c| matches!(c, Call::Estimate { .. }));
    // The sync finishes with nothing on screen.
    rig.fake.set_load(synced("h1"));
    rig.hold(cx);
    rig.emit(cx, rig.event(|e| e.state = JobState::Done));
    let snap = rig.snap(cx);
    assert_eq!(snap.sync_state, SyncState::Loading);
    assert_eq!(
        rig.fake.count(|c| matches!(c, Call::Estimate { .. })),
        estimates
    );
    rig.release_all(cx);
    assert_eq!(rig.snap(cx).sync_state, SyncState::Synced);
}

#[gpui::test]
fn surfaces_a_load_failure_and_recovers_on_a_forced_retry(cx: &mut TestAppContext) {
    let fake = Fake::new();
    fake.with(|s| s.load.once.push_back(Err("git exploded".into())));
    let rig = Rig::with(cx, fake, start_of_tests(), false);
    let snap = rig.snap(cx);
    assert_eq!(snap.status, Status::Error);
    assert_eq!(snap.error.as_deref(), Some("git exploded"));
    rig.act(cx, |view, cx| view.refresh(true, cx));
    let snap = rig.snap(cx);
    assert_eq!(snap.status, Status::Ready);
    assert!(snap.error.is_none());
    assert_eq!(rig.fake.loads().len(), 2);
}

#[gpui::test]
fn drops_the_old_error_at_once_when_retrying_and_shows_loading_until_the_answer(
    cx: &mut TestAppContext,
) {
    let fake = Fake::new();
    fake.with(|s| s.load.once.push_back(Err("git exploded".into())));
    let rig = Rig::with(cx, fake, start_of_tests(), false);
    assert_eq!(rig.snap(cx).status, Status::Error);
    rig.hold(cx);
    rig.act(cx, |view, cx| view.refresh(true, cx));
    let snap = rig.snap(cx);
    assert!(snap.error.is_none());
    assert_eq!(snap.status, Status::Loading);
    assert!(snap.refreshing);
    rig.release_all(cx);
    assert_eq!(rig.snap(cx).status, Status::Ready);
}

#[gpui::test]
fn drops_a_refresh_failure_banner_at_once_when_refreshing_again(cx: &mut TestAppContext) {
    let rig = Rig::new(cx);
    rig.fake
        .with(|s| s.load.once.push_back(Err("offline".into())));
    rig.act(cx, |view, cx| view.refresh(true, cx));
    let snap = rig.snap(cx);
    assert_eq!(snap.status, Status::Ready);
    assert_eq!(snap.error.as_deref(), Some("offline"));
    rig.hold(cx);
    rig.act(cx, |view, cx| view.refresh(true, cx));
    assert!(rig.snap(cx).error.is_none());
    rig.release_all(cx);
}

#[gpui::test]
fn reads_the_project_files_once_even_when_a_load_is_asked_for_twice(cx: &mut TestAppContext) {
    let rig = Rig::with(cx, Fake::new(), start_of_tests(), true);
    rig.release(cx, "prefs");
    // The same load asked for again while it is in flight joins it.
    rig.act(cx, |view, cx| view.refresh(false, cx));
    rig.release_all(cx);
    assert_eq!(rig.snap(cx).status, Status::Ready);
    let count = |call: Call| rig.fake.count(|c| *c == call);
    assert_eq!(count(Call::Config), 1);
    assert_eq!(count(Call::Mailmap), 1);
    assert_eq!(count(Call::Branches), 1);
    assert_eq!(count(Call::User), 1);
    assert_eq!(rig.fake.loads().len(), 1);
}

#[gpui::test]
fn a_forced_reload_reads_the_files_again_instead_of_joining_a_read_in_flight(
    cx: &mut TestAppContext,
) {
    let rig = Rig::with(cx, Fake::new(), start_of_tests(), true);
    rig.release(cx, "prefs");
    rig.act(cx, |view, cx| view.refresh(true, cx));
    rig.release_all(cx);
    assert_eq!(rig.fake.count(|c| *c == Call::Config), 2);
}

#[gpui::test]
fn treats_an_unreadable_config_as_an_error_instead_of_using_defaults(cx: &mut TestAppContext) {
    let fake = Fake::new();
    fake.with(|s| s.config.default = Err("permission denied".into()));
    let rig = Rig::with(cx, fake, start_of_tests(), false);
    let snap = rig.snap(cx);
    assert_eq!(snap.status, Status::Error);
    assert_eq!(snap.error.as_deref(), Some("permission denied"));
}

#[gpui::test]
fn keeps_working_when_only_the_mailmap_branches_or_git_user_fail(cx: &mut TestAppContext) {
    let fake = Fake::new();
    fake.with(|s| {
        s.mailmap.default = Err("no".into());
        s.branches.default = Err("no".into());
        s.user.default = Err("no".into());
    });
    let rig = Rig::with(cx, fake, start_of_tests(), false);
    let snap = rig.snap(cx);
    assert_eq!(snap.status, Status::Ready);
    assert!(snap.branches.is_empty());
    assert!(snap.model.unwrap().me_id.is_none());
}

#[gpui::test]
fn parses_the_stored_config(cx: &mut TestAppContext) {
    let fake = Fake::new();
    let mut config = DEFAULT_CONFIG.clone();
    config.people = vec![monocode_core::contributors::model::types::Person {
        id: "ana".into(),
        name: "Ana S".into(),
        identities: vec!["ana@example.com".into()],
    }];
    fake.with(|s| s.config.default = Ok(Some(serialize_config(&config))));
    let rig = Rig::with(cx, fake, start_of_tests(), false);
    let snap = rig.snap(cx);
    assert_eq!(snap.config.people[0].name, "Ana S");
    assert_eq!(snap.model.unwrap().stats.rows[0].name, "Ana S");
}

#[gpui::test]
fn saves_optimistically_and_writes_the_serialized_config(cx: &mut TestAppContext) {
    let rig = Rig::new(cx);
    rig.hold(cx);
    let mut next = (*rig.snap(cx).config).clone();
    next.weights.review = 2.;
    rig.act(cx, |view, cx| view.save_config(next.clone(), cx))
        .unwrap();
    assert_eq!(rig.snap(cx).config.weights.review, 2.);
    assert_eq!(rig.held(cx), ["write-config"]);
    rig.release_all(cx);
    assert_eq!(
        rig.fake
            .count(|c| *c == Call::Write(serialize_config(&next))),
        1
    );
    let snap = rig.snap(cx);
    assert_eq!(snap.config.weights.review, 2.);
    assert!(snap.save_error.is_none());
}

#[gpui::test]
fn reverts_and_reports_an_error_when_the_write_fails(cx: &mut TestAppContext) {
    let rig = Rig::new(cx);
    rig.fake
        .with(|s| s.write.once.push_back(Err("disk full".into())));
    let mut next = (*rig.snap(cx).config).clone();
    next.weights.care = 3.;
    rig.act(cx, |view, cx| view.save_config(next.clone(), cx))
        .unwrap();
    let snap = rig.snap(cx);
    assert_eq!(snap.config.weights.care, 1.);
    assert_eq!(snap.save_error.as_deref(), Some("disk full"));
    rig.act(cx, |view, cx| view.save_config(next, cx)).unwrap();
    let snap = rig.snap(cx);
    assert_eq!(snap.config.weights.care, 3.);
    assert!(snap.save_error.is_none());
}

#[gpui::test]
fn exposes_no_config_error_for_a_valid_or_missing_file(cx: &mut TestAppContext) {
    let rig = Rig::new(cx);
    assert!(rig.snap(cx).config_error.is_none());
}

#[gpui::test]
fn reports_a_broken_file_as_a_config_error_and_refuses_to_save_over_it(cx: &mut TestAppContext) {
    for (text, reason) in [
        (r#"{ "people": [ ,, ] }"#, "Invalid JSON"),
        ("[]", "Expected a JSON object"),
    ] {
        let fake = Fake::new();
        fake.with(|s| s.config.default = Ok(Some(text.into())));
        let rig = Rig::with(cx, fake, start_of_tests(), false);
        let snap = rig.snap(cx);
        assert_eq!(snap.status, Status::Ready);
        assert!(snap.config_error.as_deref().unwrap().starts_with(reason));
        // Defaults are still shown so the page renders.
        assert_eq!(*snap.config, *DEFAULT_CONFIG);
        let mut next = (*snap.config).clone();
        next.ignore = vec!["x/**".into()];
        let refused = rig
            .act(cx, |view, cx| view.save_config(next, cx))
            .unwrap_err();
        assert!(
            refused.starts_with(".monoptah/contributors.json could not be read ")
                && refused.contains("Fix the file"),
            "{refused}"
        );
        assert_eq!(rig.fake.count(|c| matches!(c, Call::Write(_))), 0);
        // The page-level save alert would only duplicate the config banner.
        let snap = rig.snap(cx);
        assert!(snap.save_error.is_none());
        assert_eq!(*snap.config, *DEFAULT_CONFIG);
    }
}

#[gpui::test]
fn clears_the_config_error_after_the_file_is_fixed_and_reloaded_then_saves_again(
    cx: &mut TestAppContext,
) {
    let fake = Fake::new();
    fake.with(|s| s.config.once.push_back(Ok(Some("{ nope".into()))));
    let rig = Rig::with(cx, fake, start_of_tests(), false);
    assert!(rig.snap(cx).config_error.is_some());
    let mut fixed = DEFAULT_CONFIG.clone();
    fixed.people = vec![monocode_core::contributors::model::types::Person {
        id: "ana".into(),
        name: "Ana S".into(),
        identities: vec!["ana@example.com".into()],
    }];
    rig.fake
        .with(|s| s.config.default = Ok(Some(serialize_config(&fixed))));
    rig.act(cx, |view, cx| view.refresh(true, cx));
    let snap = rig.snap(cx);
    assert!(snap.config_error.is_none());
    assert_eq!(snap.config.people[0].name, "Ana S");
    let mut next = (*snap.config).clone();
    next.ignore = vec!["x/**".into()];
    rig.act(cx, |view, cx| view.save_config(next, cx)).unwrap();
    assert_eq!(rig.fake.count(|c| matches!(c, Call::Write(_))), 1);
}

#[gpui::test]
fn notices_a_file_that_turned_invalid_between_loads(cx: &mut TestAppContext) {
    let rig = Rig::new(cx);
    assert!(rig.snap(cx).config_error.is_none());
    rig.fake
        .with(|s| s.config.default = Ok(Some("<<<<<<< HEAD".into())));
    rig.act(cx, |view, cx| view.refresh(true, cx));
    assert!(
        rig.snap(cx)
            .config_error
            .unwrap()
            .starts_with("Invalid JSON")
    );
}

#[gpui::test]
fn serializes_overlapping_saves_in_order(cx: &mut TestAppContext) {
    let rig = Rig::new(cx);
    rig.hold(cx);
    let mut a = (*rig.snap(cx).config).clone();
    a.weights.care = 2.;
    let mut b = a.clone();
    b.weights.review = 4.;
    rig.act(cx, |view, cx| {
        view.save_config(a.clone(), cx).unwrap();
        view.save_config(b.clone(), cx).unwrap();
    });
    // One write at a time.
    assert_eq!(rig.held(cx), ["write-config"]);
    rig.release(cx, "write-config");
    assert_eq!(rig.held(cx), ["write-config"]);
    rig.release(cx, "write-config");
    let writes: Vec<Call> = rig
        .fake
        .calls()
        .into_iter()
        .filter(|c| matches!(c, Call::Write(_)))
        .collect();
    assert_eq!(
        writes,
        [
            Call::Write(serialize_config(&a)),
            Call::Write(serialize_config(&b))
        ]
    );
    let snap = rig.snap(cx);
    assert_eq!(
        (snap.config.weights.care, snap.config.weights.review),
        (2., 4.)
    );
}

#[gpui::test]
fn reads_the_store_again_on_a_forced_refresh(cx: &mut TestAppContext) {
    let rig = Rig::new(cx);
    rig.act(cx, |view, cx| view.refresh(true, cx));
    assert_eq!(rig.fake.loads().len(), 2);
}

#[gpui::test]
fn revalidates_on_window_focus(cx: &mut TestAppContext) {
    let rig = Rig::new(cx);
    let before = rig.fake.loads().len();
    rig.resume(cx);
    assert_eq!(rig.fake.loads().len(), before + 1);
}

#[gpui::test]
fn keeps_the_same_raw_object_when_a_revalidation_returns_the_same_data(cx: &mut TestAppContext) {
    let rig = Rig::new(cx);
    let first = rig.snap(cx).raw.unwrap();
    rig.resume(cx);
    assert!(Arc::ptr_eq(&first, &rig.snap(cx).raw.unwrap()));
}

#[gpui::test]
fn collects_the_chosen_branch(cx: &mut TestAppContext) {
    let rig = Rig::new(cx);
    rig.act(cx, |view, cx| view.set_branch(Some("dev".into()), cx));
    let Some(Call::Load { branch, .. }) = rig.fake.loads().pop() else {
        panic!("no load");
    };
    assert_eq!(branch.as_deref(), Some("dev"));
    assert_eq!(rig.snap(cx).branch, "dev");
}

#[gpui::test]
fn persists_the_period_and_the_merge_toggle(cx: &mut TestAppContext) {
    let rig = Rig::new(cx);
    rig.pick_month_back(cx, 3);
    rig.act(cx, |view, cx| view.set_merge(false, cx));
    assert_eq!(
        rig.fake.with(|s| s.saved_range.clone()),
        Some(saved(&month_back(3)))
    );
    assert_eq!(
        rig.fake.with(|s| s.saved_merge.clone()).as_deref(),
        Some("false")
    );
    let snap = rig.snap(cx);
    assert_eq!(snap.period.id, month_back(3).id);
    assert!(!snap.merge);
    assert!(!snap.model.unwrap().merge);
}

#[gpui::test]
fn restores_a_saved_merge_toggle(cx: &mut TestAppContext) {
    let fake = Fake::new();
    fake.with(|s| s.saved_merge = Some("false".into()));
    let rig = Rig::with(cx, fake, start_of_tests(), false);
    assert!(!rig.snap(cx).merge);
}

// ---- races --------------------------------------------------------------

#[gpui::test]
fn keeps_a_save_made_while_a_focus_reload_was_still_reading_the_config(cx: &mut TestAppContext) {
    let rig = Rig::new(cx);
    rig.hold(cx);
    rig.resume(cx);
    assert!(rig.held(cx).contains(&"meta"));
    let mut next = (*rig.snap(cx).config).clone();
    next.weights.review = 5.;
    rig.act(cx, |view, cx| view.save_config(next, cx)).unwrap();
    // The reload answers with the file as it was before the save.
    rig.release_all(cx);
    assert_eq!(rig.snap(cx).config.weights.review, 5.);
    // The next edit is built from the shown config and keeps the earlier one.
    let mut again = (*rig.snap(cx).config).clone();
    again.weights.care = 3.;
    rig.act(cx, |view, cx| view.save_config(again, cx)).unwrap();
    let Some(Call::Write(text)) = rig
        .fake
        .calls()
        .into_iter()
        .rfind(|c| matches!(c, Call::Write(_)))
    else {
        panic!("no write");
    };
    assert!(text.contains("\"review\": 5"), "{text}");
}

#[gpui::test]
fn keeps_a_pending_save_when_a_reload_that_started_during_it_finishes(cx: &mut TestAppContext) {
    let rig = Rig::new(cx);
    rig.hold(cx);
    let mut next = (*rig.snap(cx).config).clone();
    next.weights.care = 4.;
    rig.act(cx, |view, cx| view.save_config(next, cx)).unwrap();
    rig.act(cx, |view, cx| view.refresh(false, cx));
    // The write finishes first, then the slow read of the old file.
    rig.release(cx, "write-config");
    rig.release_all(cx);
    assert_eq!(rig.snap(cx).config.weights.care, 4.);
}

#[gpui::test]
fn still_picks_up_an_edited_file_when_no_save_was_involved(cx: &mut TestAppContext) {
    let rig = Rig::new(cx);
    let mut edited = DEFAULT_CONFIG.clone();
    edited.weights.breadth = 2.;
    rig.fake
        .with(|s| s.config.once.push_back(Ok(Some(serialize_config(&edited)))));
    rig.act(cx, |view, cx| view.refresh(false, cx));
    assert_eq!(rig.snap(cx).config.weights.breadth, 2.);
}

#[gpui::test]
fn ignores_a_late_failure_of_an_abandoned_request(cx: &mut TestAppContext) {
    let fake = Fake::new();
    fake.with(|s| s.saved_range = Some(saved(&month_back(3))));
    fake.with(|s| {
        s.load.once.push_back(Err("A failed".into()));
        s.load.once.push_back(Err("B failed".into()));
    });
    let rig = Rig::with(cx, fake, start_of_tests(), true);
    rig.release(cx, "prefs");
    rig.act(cx, |view, cx| view.go_previous(cx));
    assert_eq!(rig.held(cx), ["meta", "period", "save-pick", "period"]);
    // The project files are in; B (the newer period) fails first, then A.
    rig.release(cx, "meta");
    rig.release_nth(cx, "period", 1);
    let snap = rig.snap(cx);
    assert_eq!(snap.status, Status::Error);
    assert_eq!(snap.error.as_deref(), Some("B failed"));
    rig.release_nth(cx, "period", 0);
    let snap = rig.snap(cx);
    assert_eq!(snap.status, Status::Error);
    assert_eq!(snap.error.as_deref(), Some("B failed"));
}

#[gpui::test]
fn does_not_clear_the_current_failure_when_an_abandoned_request_succeeds(cx: &mut TestAppContext) {
    let fake = Fake::new();
    fake.with(|s| s.saved_range = Some(saved(&month_back(3))));
    fake.with(|s| {
        s.load.once.push_back(Ok(synced("h1")));
        s.load.once.push_back(Err("B failed".into()));
    });
    let rig = Rig::with(cx, fake, start_of_tests(), true);
    rig.release(cx, "prefs");
    rig.act(cx, |view, cx| view.go_previous(cx));
    // The newer request (B) fails first; the abandoned one (A) then succeeds.
    rig.release(cx, "meta");
    rig.release_nth(cx, "period", 1);
    assert_eq!(rig.snap(cx).error.as_deref(), Some("B failed"));
    rig.release_nth(cx, "period", 0);
    let snap = rig.snap(cx);
    assert_eq!(snap.error.as_deref(), Some("B failed"));
    assert_eq!(snap.status, Status::Error);
}

#[gpui::test]
fn drops_an_older_unforced_answer_that_lands_after_a_forced_one(cx: &mut TestAppContext) {
    let fake = Fake::new();
    let mut first = synced("h1");
    first.raw.gh_available = false;
    first.raw.gh_note = Some("old".into());
    let mut forced = synced("h1");
    forced.raw.gh_available = true;
    fake.with(|s| {
        s.load.once.push_back(Ok(first));
        s.load.once.push_back(Ok(forced));
    });
    let rig = Rig::with(cx, fake, start_of_tests(), true);
    rig.release(cx, "prefs");
    rig.act(cx, |view, cx| view.refresh(true, cx));
    // The forced read answers first.
    rig.release_nth(cx, "period", 1);
    rig.release_all(cx);
    assert!(rig.snap(cx).raw.unwrap().gh_available);
}

#[gpui::test]
fn does_not_refetch_when_the_current_branch_is_picked_explicitly(cx: &mut TestAppContext) {
    let rig = Rig::new(cx);
    let calls = rig.fake.loads().len();
    rig.act(cx, |view, cx| view.set_branch(Some("main".into()), cx));
    assert_eq!(rig.fake.loads().len(), calls);
    assert_eq!(rig.snap(cx).branch, "main");
}

#[gpui::test]
fn keeps_showing_the_previous_data_marked_stale_while_a_new_period_loads(cx: &mut TestAppContext) {
    let rig = Rig::new(cx);
    rig.hold(cx);
    rig.act(cx, |view, cx| view.set_kind(PeriodKind::Month, cx));
    let snap = rig.snap(cx);
    assert_eq!(snap.status, Status::Ready);
    assert!(snap.stale);
    assert!(snap.model.is_some());
    rig.release_all(cx);
    assert!(!rig.snap(cx).stale);
}

// ---- automatic reloads --------------------------------------------------

#[gpui::test]
fn reloads_at_most_once_per_15_seconds(cx: &mut TestAppContext) {
    let t0 = local(2026, 10, 15, 10, 0);
    let rig = Rig::with(cx, Fake::new(), t0, false);
    let base = rig.fake.loads().len();
    rig.resume(cx);
    rig.resume(cx);
    assert_eq!(rig.fake.loads().len(), base + 1);
    rig.set_clock(t0 + chrono::Duration::seconds(5));
    rig.resume(cx);
    assert_eq!(rig.fake.loads().len(), base + 1);
    rig.set_clock(t0 + chrono::Duration::seconds(16));
    rig.resume(cx);
    assert_eq!(rig.fake.loads().len(), base + 2);
}

#[gpui::test]
fn lets_a_manual_refresh_through_the_throttle(cx: &mut TestAppContext) {
    let rig = Rig::new(cx);
    rig.resume(cx);
    let calls = rig.fake.loads().len();
    rig.act(cx, |view, cx| view.refresh(true, cx));
    assert_eq!(rig.fake.loads().len(), calls + 1);
}

#[gpui::test]
fn moves_the_7d_period_to_the_new_day_after_midnight_without_a_refocus(cx: &mut TestAppContext) {
    let rig = Rig::with(cx, Fake::new(), local(2026, 10, 15, 23, 59), false);
    assert_eq!(rig.snap(cx).period.end, local(2026, 10, 16, 0, 0));
    rig.set_clock(local(2026, 10, 16, 0, 1));
    cx.executor().advance_clock(Duration::from_secs(120));
    cx.run_until_parked();
    assert_eq!(rig.snap(cx).period.end, local(2026, 10, 17, 0, 0));
    assert_eq!(
        rig.snap(cx).model.unwrap().period.end,
        local(2026, 10, 17, 0, 0)
    );
}

#[gpui::test]
fn follows_the_calendar_when_the_current_month_turns_but_not_a_pinned_past_month(
    cx: &mut TestAppContext,
) {
    let rig = Rig::with(cx, Fake::new(), local(2026, 10, 31, 23, 59), false);
    rig.act(cx, |view, cx| view.set_kind(PeriodKind::Month, cx));
    assert_eq!(rig.snap(cx).period.id, "2026-10");
    rig.set_clock(local(2026, 11, 1, 0, 1));
    cx.executor().advance_clock(Duration::from_secs(120));
    cx.run_until_parked();
    let snap = rig.snap(cx);
    assert_eq!(snap.period.id, "2026-11");
    assert_eq!(snap.model.unwrap().period.id, "2026-11");
    rig.act(cx, |view, cx| view.go_previous(cx));
    rig.act(cx, |view, cx| view.go_previous(cx));
    assert_eq!(rig.snap(cx).period.id, "2026-09");
    rig.set_clock(local(2026, 11, 30, 23, 59));
    cx.executor()
        .advance_clock(Duration::from_secs(60 * 60 * 24 * 30));
    cx.run_until_parked();
    assert_eq!(rig.snap(cx).period.id, "2026-09");
}

// ---- period -------------------------------------------------------------

#[gpui::test]
fn defaults_to_the_last_7_days_with_nothing_ahead_and_no_stepping_back(cx: &mut TestAppContext) {
    let rig = Rig::new(cx);
    let snap = rig.snap(cx);
    assert_eq!(snap.period.kind, PeriodKind::Days7);
    assert!(!snap.has_next);
    let before = rig.fake.loads().len();
    rig.act(cx, |view, cx| view.go_previous(cx));
    rig.act(cx, |view, cx| view.go_next(cx));
    assert_eq!(rig.fake.loads().len(), before);
    assert_eq!(rig.snap(cx).period.kind, PeriodKind::Days7);
}

#[gpui::test]
fn selects_the_current_period_of_a_kind_and_remembers_the_kind_alone(cx: &mut TestAppContext) {
    let rig = Rig::new(cx);
    for kind in [PeriodKind::Month, PeriodKind::Quarter, PeriodKind::Year] {
        rig.act(cx, |view, cx| view.set_kind(kind, cx));
        let snap = rig.snap(cx);
        let expected = current_period(kind, &start_of_tests(), None);
        assert_eq!(snap.period.id, expected.id);
        assert_eq!(snap.model.unwrap().period.id, expected.id);
        assert!(!snap.has_next);
        assert_eq!(
            rig.fake.with(|s| s.saved_range.clone()).as_deref(),
            Some(kind.as_str())
        );
    }
}

#[gpui::test]
fn steps_back_and_forward_and_ranks_the_stepped_period(cx: &mut TestAppContext) {
    let rig = Rig::new(cx);
    rig.act(cx, |view, cx| view.set_kind(PeriodKind::Quarter, cx));
    let current = current_period(PeriodKind::Quarter, &start_of_tests(), None);
    let before = previous_period(&current).unwrap();
    rig.act(cx, |view, cx| view.go_previous(cx));
    let snap = rig.snap(cx);
    assert_eq!(snap.period.id, before.id);
    let model = snap.model.unwrap();
    assert_eq!(model.period.id, before.id);
    assert_eq!(
        model.previous_period.as_ref().map(|p| p.id.clone()),
        previous_period(&before).map(|p| p.id)
    );
    assert!(rig.snap(cx).has_next);
    assert_eq!(
        rig.fake.with(|s| s.saved_range.clone()),
        Some(format!("quarter:{}", before.id))
    );
    rig.act(cx, |view, cx| view.go_next(cx));
    let snap = rig.snap(cx);
    assert_eq!(snap.period.id, current.id);
    assert!(!snap.has_next);
    assert_eq!(
        rig.fake.with(|s| s.saved_range.clone()).as_deref(),
        Some("quarter")
    );
}

#[gpui::test]
fn does_not_step_past_the_current_period(cx: &mut TestAppContext) {
    let rig = Rig::new(cx);
    rig.act(cx, |view, cx| view.set_kind(PeriodKind::Year, cx));
    let before = rig.fake.loads().len();
    rig.act(cx, |view, cx| view.go_next(cx));
    assert_eq!(
        rig.snap(cx).period.id,
        current_period(PeriodKind::Year, &start_of_tests(), None).id
    );
    assert_eq!(rig.fake.loads().len(), before);
}

#[gpui::test]
fn keeps_showing_the_old_period_marked_stale_while_the_new_one_loads(cx: &mut TestAppContext) {
    let rig = Rig::new(cx);
    rig.act(cx, |view, cx| view.set_kind(PeriodKind::Month, cx));
    rig.hold(cx);
    rig.act(cx, |view, cx| view.go_previous(cx));
    let snap = rig.snap(cx);
    assert_eq!(snap.period.id, month_back(1).id);
    assert!(snap.stale);
    assert_eq!(snap.model.as_ref().unwrap().period.id, month_back(0).id);
    rig.release_all(cx);
    let snap = rig.snap(cx);
    assert!(!snap.stale);
    assert_eq!(snap.model.unwrap().period.id, month_back(1).id);
}

// ---- cache --------------------------------------------------------------

#[gpui::test]
fn keeps_only_the_most_recent_entries(cx: &mut TestAppContext) {
    let rig = Rig::new(cx);
    rig.act(cx, |view, cx| view.set_kind(PeriodKind::Month, cx));
    for _ in 0..8 {
        rig.act(cx, |view, cx| view.go_previous(cx));
    }
    let size = rig.view.read_with(cx, |view, _| view.loads.len());
    assert!((1..=6).contains(&size), "{size}");
}

// ---- sync store states --------------------------------------------------

fn unsynced_rig(cx: &mut TestAppContext) -> Rig {
    let fake = Fake::new();
    fake.set_load(unsynced());
    Rig::with(cx, fake, start_of_tests(), false)
}

#[gpui::test]
fn a_never_synced_period_only_loads_and_estimates_and_starts_nothing(cx: &mut TestAppContext) {
    let rig = unsynced_rig(cx);
    let snap = rig.snap(cx);
    assert_eq!(snap.status, Status::Ready);
    assert_eq!(snap.sync_state, SyncState::NotSynced);
    assert_eq!(snap.estimate, Some(ESTIMATE));
    // Cheap read of the store, one estimate, and nothing else.
    assert_eq!(rig.fake.loads().len(), 1);
    assert_eq!(rig.fake.count(|c| matches!(c, Call::Estimate { .. })), 1);
    assert!(rig.fake.starts().is_empty());
    assert_eq!(rig.fake.count(|c| *c == Call::Cancel), 0);
    // Numbers are never shown without a sync.
    assert!(snap.model.is_none());
    assert!(snap.raw.is_none());
}

#[gpui::test]
fn keeps_the_reason_when_the_estimate_fails_and_sync_stays_available(cx: &mut TestAppContext) {
    let fake = Fake::new();
    fake.set_load(unsynced());
    fake.with(|s| s.estimate.default = Err("git is not responding".into()));
    let rig = Rig::with(cx, fake, start_of_tests(), false);
    let snap = rig.snap(cx);
    assert_eq!(snap.sync_state, SyncState::NotSynced);
    assert!(snap.estimate.is_none());
    assert_eq!(
        snap.estimate_error.as_deref(),
        Some("git is not responding")
    );
    rig.act(cx, |view, cx| view.start_sync(cx));
    assert_eq!(rig.fake.starts().len(), 1);
}

#[gpui::test]
fn drops_a_queued_entry_that_the_core_never_turned_into_a_job(cx: &mut TestAppContext) {
    let rig = unsynced_rig(cx);
    // The core accepted the request but lists nothing for this path.
    rig.fake.with(|s| s.start.default = Ok(()));
    rig.act(cx, |view, cx| view.start_sync(cx));
    rig.fake.with(|s| s.listed.clear());
    rig.act(cx, |view, cx| view.start_sync(cx));
    rig.fake.with(|s| s.listed.clear());
    rig.jobs
        .update(cx, |jobs, cx| jobs.reconcile_with(Vec::new, cx));
    cx.run_until_parked();
    assert!(rig.snap(cx).job.is_none());
}

#[gpui::test]
fn estimates_the_window_the_sync_would_use(cx: &mut TestAppContext) {
    let rig = unsynced_rig(cx);
    let now = start_of_tests();
    let period = seven_days();
    let Some(Call::Estimate { branch, from, to }) = rig
        .fake
        .calls()
        .into_iter()
        .find(|c| matches!(c, Call::Estimate { .. }))
    else {
        panic!("no estimate");
    };
    assert_eq!(branch, None);
    assert_eq!(from, iso_seconds(&sync_window(&period, &now).from));
    assert_eq!(to, iso_seconds(&period.end));
}

#[gpui::test]
fn an_incomplete_period_shows_the_estimate_of_what_is_left_never_partial_numbers(
    cx: &mut TestAppContext,
) {
    let fake = Fake::new();
    let mut partial = unsynced();
    partial.raw = raw("h1", &start_of_tests());
    partial.missing_commits = 12;
    partial.synced_at = Some(start_of_tests().timestamp() - 60);
    partial.coverage_head = Some("abc123".into());
    fake.set_load(partial);
    let rig = Rig::with(cx, fake, start_of_tests(), false);
    let snap = rig.snap(cx);
    assert_eq!(snap.sync_state, SyncState::NotSynced);
    assert!(snap.model.is_none());
    assert!(snap.raw.is_none());
    assert_eq!(snap.estimate, Some(ESTIMATE));
    assert!(rig.fake.starts().is_empty());
}

#[gpui::test]
fn a_covered_period_shows_its_data_and_the_sync_time(cx: &mut TestAppContext) {
    let rig = Rig::new(cx);
    let snap = rig.snap(cx);
    assert_eq!(snap.sync_state, SyncState::Synced);
    assert_eq!(snap.synced_at, Some(start_of_tests().timestamp() - 300));
    assert!(snap.model.is_some());
    assert_eq!(rig.fake.count(|c| matches!(c, Call::Estimate { .. })), 0);
    assert!(rig.fake.starts().is_empty());
}

#[gpui::test]
fn sync_starts_a_full_job_for_the_period_window(cx: &mut TestAppContext) {
    let rig = unsynced_rig(cx);
    rig.act(cx, |view, cx| view.start_sync(cx));
    let now = start_of_tests();
    let period = seven_days();
    assert_eq!(
        rig.fake.starts(),
        [StartCall {
            from: iso_seconds(&sync_window(&period, &now).from),
            to: iso_seconds(&period.end),
            branch: None,
            mode: "full",
            old_head: None,
        }]
    );
}

#[gpui::test]
fn progress_events_of_this_project_update_the_job_and_done_reloads(cx: &mut TestAppContext) {
    let rig = unsynced_rig(cx);
    rig.act(cx, |view, cx| view.start_sync(cx));
    assert_eq!(rig.snap(cx).job.unwrap().state, JobState::Queued);
    rig.emit(
        cx,
        rig.event(|e| {
            e.done = 30;
            e.total = 120;
            e.percent = 12;
        }),
    );
    let job = rig.snap(cx).job.unwrap();
    assert_eq!(
        (job.state, job.step, job.done, job.total, job.percent),
        (JobState::Running, SyncStep::Commits, 30, 120, 12)
    );
    // Nothing is reloaded while the job runs.
    assert_eq!(rig.fake.loads().len(), 1);
    rig.fake.set_load(synced("h1"));
    rig.emit(
        cx,
        rig.event(|e| {
            e.state = JobState::Done;
            e.percent = 100;
        }),
    );
    assert_eq!(rig.fake.loads().len(), 2);
    let snap = rig.snap(cx);
    assert_eq!(snap.sync_state, SyncState::Synced);
    assert!(snap.job.is_none());
    assert!(snap.model.is_some());
}

#[gpui::test]
fn ignores_events_of_other_projects_and_of_other_periods(cx: &mut TestAppContext) {
    let rig = unsynced_rig(cx);
    rig.emit(cx, rig.event(|e| e.root = "/somewhere/else".into()));
    assert!(rig.snap(cx).job.is_none());
    let now = start_of_tests();
    let month = window_of(&current_period(PeriodKind::Month, &now, None), &now);
    rig.emit(
        cx,
        rig.event(|e| {
            e.from = month.0;
            e.to = month.1;
        }),
    );
    assert!(rig.snap(cx).job.is_none());
    rig.emit(cx, rig.event(|e| e.branch = "dev".into()));
    assert!(rig.snap(cx).job.is_none());
    rig.emit(cx, rig.event(|_| {}));
    assert_eq!(rig.snap(cx).job.unwrap().state, JobState::Running);
    assert_eq!(rig.fake.loads().len(), 1);
}

#[gpui::test]
fn picks_up_a_job_that_was_already_running_when_the_page_opened(cx: &mut TestAppContext) {
    let rig = unsynced_rig(cx);
    let running = rig.event(|e| {
        e.step = SyncStep::PullRequests;
        e.done = 2;
        e.total = 9;
        e.percent = 55;
    });
    rig.fake
        .with(|s| s.listed = vec![running, event("/elsewhere", (1, 2), JobState::Running)]);
    let fake = rig.fake.clone();
    rig.jobs.update(cx, |jobs, cx| {
        jobs.reconcile_with(move || fake.status(), cx)
    });
    cx.run_until_parked();
    let job = rig.snap(cx).job.unwrap();
    assert_eq!((job.step, job.percent), (SyncStep::PullRequests, 55));
}

#[gpui::test]
fn a_failed_job_shows_its_error_and_sync_can_be_pressed_again(cx: &mut TestAppContext) {
    let rig = unsynced_rig(cx);
    rig.emit(
        cx,
        rig.event(|e| {
            e.state = JobState::Error;
            e.error = Some("git exploded".into());
        }),
    );
    let snap = rig.snap(cx);
    assert!(snap.job.is_none());
    assert_eq!(snap.job_error.as_deref(), Some("git exploded"));
    rig.act(cx, |view, cx| view.start_sync(cx));
    let snap = rig.snap(cx);
    assert!(snap.job_error.is_none());
    assert_eq!(snap.job.unwrap().state, JobState::Queued);
}

#[gpui::test]
fn the_sync_error_can_be_dismissed_and_the_store_forgets_it(cx: &mut TestAppContext) {
    let rig = unsynced_rig(cx);
    rig.emit(
        cx,
        rig.event(|e| {
            e.state = JobState::Error;
            e.error = Some("git exploded".into());
        }),
    );
    assert_eq!(rig.snap(cx).job_error.as_deref(), Some("git exploded"));
    rig.act(cx, |view, cx| view.dismiss_job_error(cx));
    assert!(rig.snap(cx).job_error.is_none());
}

#[gpui::test]
fn a_rejected_start_is_reported_as_the_job_error(cx: &mut TestAppContext) {
    let rig = unsynced_rig(cx);
    rig.fake.with(|s| {
        s.start
            .once
            .push_back(Err("Could not start the sync".into()))
    });
    rig.act(cx, |view, cx| view.start_sync(cx));
    let snap = rig.snap(cx);
    assert!(snap.job.is_none());
    assert_eq!(snap.job_error.as_deref(), Some("Could not start the sync"));
}

#[gpui::test]
fn a_cancelled_job_reloads_what_was_stored_and_shows_the_new_estimate(cx: &mut TestAppContext) {
    let rig = unsynced_rig(cx);
    rig.act(cx, |view, cx| view.start_sync(cx));
    let mut now_missing = unsynced();
    now_missing.missing_commits = 15;
    rig.fake.set_load(now_missing);
    rig.fake.with(|s| {
        s.estimate.default = Ok(Estimate {
            stored_commits: 25,
            ..ESTIMATE
        })
    });
    rig.emit(cx, rig.event(|e| e.state = JobState::Cancelled));
    let snap = rig.snap(cx);
    assert_eq!(snap.sync_state, SyncState::NotSynced);
    assert!(snap.job.is_none());
    assert_eq!(snap.estimate.unwrap().stored_commits, 25);
}

#[gpui::test]
fn cancel_asks_the_core_to_stop_the_jobs_of_the_project(cx: &mut TestAppContext) {
    let rig = unsynced_rig(cx);
    rig.act(cx, |view, cx| view.cancel_sync(cx));
    assert_eq!(rig.fake.count(|c| *c == Call::Cancel), 1);
}

#[gpui::test]
fn a_failed_cancel_is_reported_and_still_checks_what_runs(cx: &mut TestAppContext) {
    let rig = unsynced_rig(cx);
    rig.act(cx, |view, cx| view.start_sync(cx));
    rig.fake
        .with(|s| s.cancel.once.push_back(Err("no such job".into())));
    // The core ended the job meanwhile: the check after the failed cancel drops it.
    rig.fake.with(|s| s.listed.clear());
    rig.act(cx, |view, cx| view.cancel_sync(cx));
    let snap = rig.snap(cx);
    assert_eq!(snap.action_error.as_deref(), Some("no such job"));
    assert!(snap.job.is_none());
}

#[gpui::test]
fn switching_period_while_syncing_keeps_the_job_and_shows_the_new_periods_state(
    cx: &mut TestAppContext,
) {
    let rig = unsynced_rig(cx);
    rig.act(cx, |view, cx| view.start_sync(cx));
    rig.emit(cx, rig.event(|e| e.percent = 40));
    assert_eq!(rig.snap(cx).job.unwrap().percent, 40);
    rig.act(cx, |view, cx| view.set_kind(PeriodKind::Month, cx));
    assert_eq!(rig.fake.count(|c| *c == Call::Cancel), 0);
    let snap = rig.snap(cx);
    assert_eq!(snap.sync_state, SyncState::NotSynced);
    assert!(snap.job.is_none());
    // The job of the first period is still known when the page returns to it.
    rig.act(cx, |view, cx| view.set_kind(PeriodKind::Days7, cx));
    assert_eq!(rig.snap(cx).job.unwrap().percent, 40);
}

#[gpui::test]
fn a_second_sync_for_another_period_queues_instead_of_replacing_the_running_one(
    cx: &mut TestAppContext,
) {
    let rig = unsynced_rig(cx);
    rig.act(cx, |view, cx| view.start_sync(cx));
    rig.emit(cx, rig.event(|e| e.percent = 40));
    rig.act(cx, |view, cx| view.set_kind(PeriodKind::Month, cx));
    rig.act(cx, |view, cx| view.start_sync(cx));
    assert_eq!(rig.snap(cx).job.unwrap().state, JobState::Queued);
    assert_eq!(rig.fake.starts().len(), 2);
    rig.act(cx, |view, cx| view.set_kind(PeriodKind::Days7, cx));
    assert_eq!(rig.snap(cx).job.unwrap().state, JobState::Running);
}

#[gpui::test]
fn keeps_the_data_on_screen_with_a_job_spinner_while_a_re_sync_runs(cx: &mut TestAppContext) {
    let rig = Rig::new(cx);
    rig.act(cx, |view, cx| view.start_sync(cx));
    assert_eq!(rig.fake.starts()[0].mode, "full");
    let snap = rig.snap(cx);
    assert_eq!(snap.sync_state, SyncState::Synced);
    assert_eq!(snap.job.unwrap().state, JobState::Queued);
    assert!(snap.model.is_some());
}

// ---- top-up -------------------------------------------------------------

fn moved(head: &str) -> LoadResponse {
    let mut response = synced(head);
    response.coverage_head = Some("old1".into());
    response
}

#[gpui::test]
fn starts_one_top_up_from_the_coverage_head_when_the_branch_head_moved(cx: &mut TestAppContext) {
    let fake = Fake::new();
    fake.set_load(moved("new2"));
    let rig = Rig::with(cx, fake, start_of_tests(), false);
    let now = start_of_tests();
    let period = seven_days();
    assert_eq!(
        rig.fake.starts(),
        [StartCall {
            from: iso_seconds(&sync_window(&period, &now).from),
            to: iso_seconds(&period.end),
            branch: None,
            mode: "topup",
            old_head: Some("old1".into()),
        }]
    );
    let snap = rig.snap(cx);
    assert!(snap.model.is_some());
    assert_eq!(snap.job.unwrap().state, JobState::Queued);
}

#[gpui::test]
fn does_not_start_another_one_for_the_same_head_on_a_reload(cx: &mut TestAppContext) {
    let fake = Fake::new();
    fake.set_load(moved("new2"));
    let rig = Rig::with(cx, fake, start_of_tests(), false);
    rig.act(cx, |view, cx| view.refresh(true, cx));
    rig.resume(cx);
    assert_eq!(rig.fake.starts().len(), 1);
}

#[gpui::test]
fn starts_another_one_when_the_head_moves_again_and_the_first_one_ended(cx: &mut TestAppContext) {
    let fake = Fake::new();
    fake.set_load(moved("new2"));
    let rig = Rig::with(cx, fake, start_of_tests(), false);
    rig.emit(
        cx,
        rig.event(|e| {
            e.state = JobState::Done;
            e.percent = 100;
        }),
    );
    rig.fake.set_load(moved("new3"));
    rig.act(cx, |view, cx| view.refresh(true, cx));
    let starts = rig.fake.starts();
    assert_eq!(starts.len(), 2);
    assert_eq!(starts[1].old_head.as_deref(), Some("old1"));
}

#[gpui::test]
fn does_nothing_when_the_coverage_head_equals_the_head(cx: &mut TestAppContext) {
    let rig = Rig::new(cx);
    assert!(rig.fake.starts().is_empty());
}

#[gpui::test]
fn does_not_top_up_a_period_that_is_not_covered_even_with_different_heads(cx: &mut TestAppContext) {
    let fake = Fake::new();
    let mut response = unsynced();
    response.coverage_head = Some("old1".into());
    response.head = "new2".into();
    fake.set_load(response);
    let rig = Rig::with(cx, fake, start_of_tests(), false);
    assert!(rig.fake.starts().is_empty());
}

fn lacking(head: &str) -> LoadResponse {
    let mut response = synced(head);
    response.covered = false;
    response.missing_commits = 3;
    response.coverage_head = Some("old1".into());
    response
}

#[gpui::test]
fn a_synced_period_that_lacks_commits_keeps_its_data_and_is_repaired_by_one_full_sync_per_head(
    cx: &mut TestAppContext,
) {
    let fake = Fake::new();
    fake.set_load(lacking("new2"));
    let rig = Rig::with(cx, fake, start_of_tests(), false);
    // Numbers stay on screen: the stored commits are shown while it updates.
    let snap = rig.snap(cx);
    assert_eq!(snap.sync_state, SyncState::Synced);
    assert!(snap.model.is_some());
    assert_eq!(rig.fake.count(|c| matches!(c, Call::Estimate { .. })), 0);
    assert_eq!(rig.fake.starts().len(), 1);
    assert_eq!(rig.fake.starts()[0].mode, "full");
    assert_eq!(snap.job.unwrap().state, JobState::Queued);
    // Not again for the same head, even when the job ended and it still lacks.
    rig.emit(
        cx,
        rig.event(|e| {
            e.state = JobState::Done;
            e.percent = 100;
        }),
    );
    rig.act(cx, |view, cx| view.refresh(true, cx));
    assert_eq!(rig.fake.starts().len(), 1);
    // A moved head is a new reason to repair.
    rig.fake.set_load(lacking("new3"));
    rig.act(cx, |view, cx| view.refresh(true, cx));
    assert_eq!(rig.fake.starts().len(), 2);
}

#[gpui::test]
fn repairs_a_lack_at_the_same_head_as_well(cx: &mut TestAppContext) {
    let fake = Fake::new();
    let mut response = synced("same");
    response.covered = false;
    response.missing_commits = 2;
    fake.set_load(response);
    let rig = Rig::with(cx, fake, start_of_tests(), false);
    assert_eq!(rig.fake.starts()[0].mode, "full");
    assert!(rig.snap(cx).model.is_some());
}

#[gpui::test]
fn a_gap_stays_manual_even_with_a_coverage_head(cx: &mut TestAppContext) {
    let fake = Fake::new();
    let mut response = unsynced();
    response.raw = raw("h1", &start_of_tests());
    response.coverage_head = Some("old1".into());
    response.head = "new2".into();
    response.missing_commits = 12;
    fake.set_load(response);
    let rig = Rig::with(cx, fake, start_of_tests(), false);
    let snap = rig.snap(cx);
    assert_eq!(snap.sync_state, SyncState::NotSynced);
    assert!(snap.model.is_none());
    assert!(rig.fake.starts().is_empty());
}

#[gpui::test]
fn a_repair_of_nothing_missing_is_not_started(cx: &mut TestAppContext) {
    let fake = Fake::new();
    let mut response = synced("h1");
    response.covered = false;
    response.missing_commits = 0;
    fake.set_load(response);
    let rig = Rig::with(cx, fake, start_of_tests(), false);
    assert!(rig.fake.starts().is_empty());
}

#[gpui::test]
fn reloads_when_the_top_up_is_done_and_drops_the_spinner(cx: &mut TestAppContext) {
    let fake = Fake::new();
    fake.set_load(moved("new2"));
    let rig = Rig::with(cx, fake, start_of_tests(), false);
    assert!(rig.snap(cx).job.is_some());
    rig.fake.set_load(synced("new2"));
    rig.emit(
        cx,
        rig.event(|e| {
            e.state = JobState::Done;
            e.percent = 100;
        }),
    );
    assert!(rig.snap(cx).job.is_none());
    assert_eq!(rig.fake.starts().len(), 1);
}

#[gpui::test]
fn does_not_top_up_a_remote_project(cx: &mut TestAppContext) {
    let fake = Fake::new();
    fake.set_load(moved("new2"));
    let clock: Clock = Rc::new(start_of_tests);
    let jobs = cx.new(SyncJobs::new);
    let view = cx.new({
        let jobs = jobs.clone();
        move |cx| GithubView::new("remote://box/work/app".into(), jobs, clock, cx)
    });
    let backend: Arc<dyn Backend> = fake.clone();
    view.update(cx, |view, cx| view.set_backend(backend, cx));
    cx.run_until_parked();
    assert!(view.read_with(cx, |view, _| view.is_remote()));
    assert!(fake.calls().is_empty());
}

// ---- GitHub state and clearing -------------------------------------------

#[gpui::test]
fn flags_partial_and_unavailable_github_data_and_retry_github_starts_a_github_job(
    cx: &mut TestAppContext,
) {
    for state in ["partial", "unavailable"] {
        let fake = Fake::new();
        let mut response = synced("h1");
        response.gh_state = Some(state.into());
        fake.set_load(response);
        let rig = Rig::with(cx, fake, start_of_tests(), false);
        let snap = rig.snap(cx);
        assert!(snap.gh_incomplete, "{state}");
        assert!(snap.model.is_some());
        rig.act(cx, |view, cx| view.retry_github(cx));
        assert_eq!(rig.fake.starts()[0].mode, "github", "{state}");
    }
}

#[gpui::test]
fn does_not_flag_complete_github_data(cx: &mut TestAppContext) {
    let rig = Rig::new(cx);
    assert!(!rig.snap(cx).gh_incomplete);
}

#[gpui::test]
fn clearing_deletes_the_synced_data_and_shows_the_empty_state_after_the_reload(
    cx: &mut TestAppContext,
) {
    let rig = Rig::new(cx);
    assert_eq!(rig.snap(cx).sync_state, SyncState::Synced);
    rig.fake.set_load(unsynced());
    rig.act(cx, |view, cx| view.clear_synced_data(cx));
    assert_eq!(rig.fake.count(|c| *c == Call::Clear), 1);
    let snap = rig.snap(cx);
    assert_eq!(snap.sync_state, SyncState::NotSynced);
    assert!(snap.model.is_none());
}

#[gpui::test]
fn clearing_allows_a_top_up_for_the_same_head_again(cx: &mut TestAppContext) {
    let fake = Fake::new();
    fake.set_load(moved("new2"));
    let rig = Rig::with(cx, fake, start_of_tests(), false);
    assert_eq!(rig.fake.starts().len(), 1);
    rig.act(cx, |view, cx| view.clear_synced_data(cx));
    rig.act(cx, |view, cx| view.refresh(true, cx));
    assert_eq!(rig.fake.starts().len(), 2);
}

#[gpui::test]
fn reports_why_clearing_was_refused_and_keeps_the_data(cx: &mut TestAppContext) {
    let rig = Rig::new(cx);
    rig.fake.with(|s| {
        s.clear.once.push_back(Err(
            "A sync of this repository is running. Cancel it first.".into(),
        ))
    });
    rig.act(cx, |view, cx| view.clear_synced_data(cx));
    let snap = rig.snap(cx);
    assert_eq!(
        snap.action_error.as_deref(),
        Some("A sync of this repository is running. Cancel it first.")
    );
    assert!(snap.model.is_some());
}

// ---- All time -----------------------------------------------------------

fn select_all(rig: &Rig, cx: &mut TestAppContext) {
    rig.act(cx, |view, cx| view.set_kind(PeriodKind::All, cx));
}

fn first_calls(rig: &Rig) -> Vec<Option<String>> {
    rig.fake
        .calls()
        .into_iter()
        .filter_map(|c| match c {
            Call::First(branch) => Some(branch),
            _ => None,
        })
        .collect()
}

fn tomorrow() -> DateTime<Local> {
    local(2026, 10, 16, 0, 0)
}

#[gpui::test]
fn reads_the_first_commit_and_loads_from_its_day_to_tomorrow_with_no_previous_period(
    cx: &mut TestAppContext,
) {
    let rig = Rig::new(cx);
    select_all(&rig, cx);
    assert_eq!(first_calls(&rig), [None]);
    let Some(Call::Load {
        since,
        until,
        branch,
    }) = rig.fake.loads().pop()
    else {
        panic!("no load");
    };
    assert_eq!(branch, None);
    assert_eq!(since, iso_seconds(&first_day()));
    assert_eq!(until, iso_seconds(&tomorrow()));
    let snap = rig.snap(cx);
    assert_eq!(snap.period.kind, PeriodKind::All);
    assert_eq!(snap.period.label, "All time");
    assert_eq!(snap.status, Status::Ready);
    let model = snap.model.unwrap();
    assert_eq!(model.period.id, "all");
    assert!(model.previous_period.is_none());
    assert_eq!(
        rig.fake.with(|s| s.saved_range.clone()).as_deref(),
        Some("all")
    );
}

#[gpui::test]
fn restores_a_saved_all_and_shows_loading_without_loading_the_store_until_the_first_commit_is_known(
    cx: &mut TestAppContext,
) {
    let fake = Fake::new();
    fake.with(|s| s.saved_range = Some("all".into()));
    let rig = Rig::with(cx, fake, start_of_tests(), true);
    rig.release(cx, "prefs");
    rig.release(cx, "meta");
    // The meta answer tells the branch, which makes the first commit known to ask for.
    assert_eq!(rig.held(cx), ["first-commit"]);
    let snap = rig.snap(cx);
    assert_eq!(snap.period.kind, PeriodKind::All);
    assert_eq!(snap.status, Status::Loading);
    assert!(snap.model.is_none());
    assert!(rig.fake.loads().is_empty());
    assert_eq!(rig.fake.count(|c| matches!(c, Call::Estimate { .. })), 0);
    rig.release_all(cx);
    assert_eq!(rig.snap(cx).status, Status::Ready);
    assert_eq!(rig.fake.loads().len(), 1);
    let Some(Call::Load { since, .. }) = rig.fake.loads().pop() else {
        panic!("no load");
    };
    assert_eq!(since, iso_seconds(&first_day()));
}

#[gpui::test]
fn keeps_the_page_that_was_showing_dimmed_while_the_first_commit_loads(cx: &mut TestAppContext) {
    let rig = Rig::new(cx);
    let before = rig.snap(cx).model.unwrap();
    rig.hold(cx);
    select_all(&rig, cx);
    let snap = rig.snap(cx);
    assert!(snap.stale);
    assert!(Arc::ptr_eq(&before, snap.model.as_ref().unwrap()));
    rig.release_all(cx);
    let snap = rig.snap(cx);
    assert!(!snap.stale);
    assert_eq!(snap.model.unwrap().period.kind, PeriodKind::All);
}

#[gpui::test]
fn does_not_ask_for_the_first_commit_for_any_other_period(cx: &mut TestAppContext) {
    let rig = Rig::new(cx);
    rig.act(cx, |view, cx| view.set_kind(PeriodKind::Year, cx));
    assert!(first_calls(&rig).is_empty());
}

#[gpui::test]
fn asks_once_per_project_and_branch_however_often_all_time_is_picked(cx: &mut TestAppContext) {
    let rig = Rig::new(cx);
    select_all(&rig, cx);
    rig.act(cx, |view, cx| view.set_kind(PeriodKind::Month, cx));
    select_all(&rig, cx);
    assert_eq!(first_calls(&rig).len(), 1);
    rig.act(cx, |view, cx| view.set_branch(Some("dev".into()), cx));
    assert_eq!(first_calls(&rig), [None, Some("dev".into())]);
    // The checked-out branch is the same key as the first call.
    rig.act(cx, |view, cx| view.set_branch(Some("main".into()), cx));
    assert_eq!(first_calls(&rig).len(), 2);
}

#[gpui::test]
fn asks_again_on_a_forced_refresh_not_on_a_focus_revalidation(cx: &mut TestAppContext) {
    let rig = Rig::new(cx);
    select_all(&rig, cx);
    rig.resume(cx);
    assert_eq!(first_calls(&rig).len(), 1);
    rig.act(cx, |view, cx| view.refresh(true, cx));
    assert_eq!(first_calls(&rig).len(), 2);
}

#[gpui::test]
fn reads_the_first_commit_again_when_the_branch_head_moves_not_when_it_stays(
    cx: &mut TestAppContext,
) {
    let fake = Fake::new();
    fake.set_load(synced("h1"));
    let rig = Rig::with(cx, fake, start_of_tests(), false);
    select_all(&rig, cx);
    rig.resume(cx);
    assert_eq!(first_calls(&rig).len(), 1);
    // A merge of an older unrelated history moves the start.
    rig.fake.set_load(synced("h2"));
    let older = local(2019, 1, 9, 0, 0);
    rig.fake
        .with(|s| s.first.default = Ok(Some(older.to_rfc3339())));
    rig.act(cx, |view, cx| view.refresh(false, cx));
    assert_eq!(first_calls(&rig).len(), 2);
    assert_eq!(rig.snap(cx).period.start, older);
    let Some(Call::Load { since, .. }) = rig.fake.loads().pop() else {
        panic!("no load");
    };
    assert_eq!(since, iso_seconds(&older));
    rig.act(cx, |view, cx| view.refresh(false, cx));
    assert_eq!(first_calls(&rig).len(), 2);
}

#[gpui::test]
fn keeps_the_newer_first_commit_answer_when_an_older_read_finishes_last(cx: &mut TestAppContext) {
    let rig = Rig::new(cx);
    rig.hold(cx);
    let newer = local(2019, 1, 9, 0, 0);
    rig.fake.with(|s| {
        s.first
            .once
            .push_back(Ok(Some(local(2023, 5, 17, 0, 0).to_rfc3339())));
        s.first.once.push_back(Ok(Some(newer.to_rfc3339())));
    });
    select_all(&rig, cx);
    assert_eq!(
        rig.held(cx)
            .iter()
            .filter(|l| **l == "first-commit")
            .count(),
        1
    );
    // The forced refresh asks again and gets the newer answer first.
    rig.act(cx, |view, cx| view.refresh(true, cx));
    rig.release_nth(cx, "first-commit", 1);
    assert_eq!(rig.snap(cx).period.start, newer);
    rig.release_nth(cx, "first-commit", 0);
    assert_eq!(rig.snap(cx).period.start, newer);
}

#[gpui::test]
fn shows_the_error_state_not_the_empty_one_for_a_date_it_cannot_read(cx: &mut TestAppContext) {
    let fake = Fake::new();
    fake.with(|s| {
        s.first.default = Ok(Some("not a date".into()));
        s.saved_range = Some("all".into());
    });
    let rig = Rig::with(cx, fake, start_of_tests(), false);
    let snap = rig.snap(cx);
    assert_eq!(snap.status, Status::Error);
    assert!(snap.error.unwrap().contains("not a date"));
    assert!(rig.fake.loads().is_empty());
}

#[gpui::test]
fn covers_today_only_for_a_repository_without_commits(cx: &mut TestAppContext) {
    let fake = Fake::new();
    fake.with(|s| s.first.default = Ok(None));
    let rig = Rig::with(cx, fake, start_of_tests(), false);
    select_all(&rig, cx);
    let Some(Call::Load { since, until, .. }) = rig.fake.loads().pop() else {
        panic!("no load");
    };
    assert_eq!(since, iso_seconds(&local(2026, 10, 15, 0, 0)));
    assert_eq!(until, iso_seconds(&tomorrow()));
    assert_eq!(rig.snap(cx).status, Status::Ready);
}

#[gpui::test]
fn reports_a_failed_first_commit_lookup_and_recovers_on_a_forced_retry(cx: &mut TestAppContext) {
    let fake = Fake::new();
    fake.with(|s| {
        s.first.once.push_back(Err("git is not responding".into()));
        s.saved_range = Some("all".into());
    });
    let rig = Rig::with(cx, fake, start_of_tests(), false);
    let snap = rig.snap(cx);
    assert_eq!(snap.status, Status::Error);
    assert_eq!(snap.error.as_deref(), Some("git is not responding"));
    assert!(rig.fake.loads().is_empty());
    rig.act(cx, |view, cx| view.refresh(true, cx));
    let snap = rig.snap(cx);
    assert!(snap.error.is_none());
    assert_eq!(snap.status, Status::Ready);
}

#[gpui::test]
fn all_time_has_no_arrows_to_follow(cx: &mut TestAppContext) {
    let rig = Rig::new(cx);
    select_all(&rig, cx);
    let calls = rig.fake.loads().len();
    assert!(!rig.snap(cx).has_next);
    rig.act(cx, |view, cx| view.go_previous(cx));
    rig.act(cx, |view, cx| view.go_next(cx));
    assert_eq!(rig.snap(cx).period.kind, PeriodKind::All);
    assert_eq!(rig.fake.loads().len(), calls);
    assert_eq!(
        rig.fake.with(|s| s.saved_range.clone()).as_deref(),
        Some("all")
    );
}

#[gpui::test]
fn estimates_and_syncs_the_window_from_the_first_commit_to_tomorrow(cx: &mut TestAppContext) {
    let rig = unsynced_rig(cx);
    select_all(&rig, cx);
    let Some(Call::Estimate { from, to, branch }) = rig
        .fake
        .calls()
        .into_iter()
        .rfind(|c| matches!(c, Call::Estimate { .. }))
    else {
        panic!("no estimate");
    };
    assert_eq!(
        (from.as_str(), to.as_str(), branch),
        (
            iso_seconds(&first_day()).as_str(),
            iso_seconds(&tomorrow()).as_str(),
            None
        )
    );
    rig.act(cx, |view, cx| view.start_sync(cx));
    assert_eq!(
        rig.fake.starts()[0],
        StartCall {
            from: iso_seconds(&first_day()),
            to: iso_seconds(&tomorrow()),
            branch: None,
            mode: "full",
            old_head: None,
        }
    );
}

#[gpui::test]
fn tops_up_all_time_from_the_coverage_head_when_the_branch_head_moved(cx: &mut TestAppContext) {
    let fake = Fake::new();
    fake.set_load(moved("new2"));
    let rig = Rig::with(cx, fake, start_of_tests(), false);
    assert_eq!(rig.fake.starts().len(), 1);
    select_all(&rig, cx);
    let topups: Vec<StartCall> = rig
        .fake
        .starts()
        .into_iter()
        .filter(|call| call.from == iso_seconds(&first_day()))
        .collect();
    assert_eq!(
        topups,
        [StartCall {
            from: iso_seconds(&first_day()),
            to: iso_seconds(&tomorrow()),
            branch: None,
            mode: "topup",
            old_head: Some("old1".into()),
        }]
    );
}

#[gpui::test]
fn all_time_matches_the_job_of_its_own_window_so_progress_shows(cx: &mut TestAppContext) {
    let fake = Fake::new();
    fake.set_load(unsynced());
    fake.with(|s| s.saved_range = Some("all".into()));
    let rig = Rig::with(cx, fake, start_of_tests(), false);
    rig.act(cx, |view, cx| view.start_sync(cx));
    assert_eq!(rig.snap(cx).job.unwrap().state, JobState::Queued);
    // A job of another window of the same project is not this page's job.
    rig.emit(cx, rig.event(|e| e.state = JobState::Running));
    assert_eq!(rig.snap(cx).job.unwrap().state, JobState::Queued);
}

// ---- fetching origin ------------------------------------------------------

fn script_fetch(rig_fake: &Fake, answers: Vec<Result<FetchOutcome, String>>) {
    rig_fake.with(|s| s.fetch.once = answers.into());
}

#[gpui::test]
fn refresh_asks_origin_once_for_the_picked_branch_or_for_none(cx: &mut TestAppContext) {
    let rig = Rig::new(cx);
    assert!(rig.fetches().is_empty(), "opening the page never fetches");
    rig.act(cx, |view, cx| view.refresh(true, cx));
    assert_eq!(rig.fetches(), [Call::Fetch(None)]);
    rig.act(cx, |view, cx| view.set_branch(Some("dev".into()), cx));
    rig.act(cx, |view, cx| view.refresh(true, cx));
    assert_eq!(
        rig.fetches(),
        [Call::Fetch(None), Call::Fetch(Some("dev".into()))]
    );
    // The branch a pick-less request follows is the same as no pick.
    rig.act(cx, |view, cx| view.set_branch(Some("main".into()), cx));
    rig.act(cx, |view, cx| view.refresh(true, cx));
    assert_eq!(rig.fetches().last(), Some(&Call::Fetch(None)));
    assert_eq!(rig.running(cx), 0);
}

#[gpui::test]
fn no_automatic_path_asks_origin(cx: &mut TestAppContext) {
    let fake = Fake::new();
    fake.set_load(moved("new2"));
    let mut gh = moved("new2");
    gh.gh_state = Some("partial".into());
    let rig = Rig::with(cx, fake, start_of_tests(), false);
    // The open itself topped the period up: a job, and still no fetch.
    assert_eq!(rig.fake.starts().len(), 1);
    rig.act(cx, |view, cx| view.refresh(false, cx));
    rig.set_clock(local(2026, 10, 15, 10, 5));
    rig.resume(cx);
    rig.act(cx, |view, cx| view.set_kind(PeriodKind::Month, cx));
    rig.act(cx, |view, cx| view.go_previous(cx));
    rig.act(cx, |view, cx| view.set_branch(Some("dev".into()), cx));
    rig.act(cx, |view, cx| view.set_kind(PeriodKind::All, cx));
    rig.emit(cx, rig.event(|e| e.state = JobState::Done));
    rig.fake.set_load(gh);
    rig.act(cx, |view, cx| view.retry_github(cx));
    // Midnight.
    rig.set_clock(local(2026, 10, 16, 0, 1));
    cx.executor().advance_clock(Duration::from_secs(36_000));
    cx.run_until_parked();
    let backend: Arc<dyn Backend> = rig.fake.clone();
    rig.act(cx, |view, cx| view.set_backend(backend, cx));
    assert!(rig.fetches().is_empty(), "{:?}", rig.fake.calls());
}

#[gpui::test]
fn a_remote_project_never_asks_origin(cx: &mut TestAppContext) {
    let fake = Fake::new();
    let clock: Clock = Rc::new(start_of_tests);
    let jobs = cx.new(SyncJobs::new);
    let view = cx.new({
        let jobs = jobs.clone();
        move |cx| GithubView::new("remote://box/work/app".into(), jobs, clock, cx)
    });
    let backend: Arc<dyn Backend> = fake.clone();
    view.update(cx, |view, cx| view.set_backend(backend, cx));
    view.update(cx, |view, cx| {
        view.refresh(true, cx);
        view.start_sync(cx);
    });
    cx.run_until_parked();
    assert!(fake.calls().is_empty());
    assert_eq!(view.read_with(cx, |view, _| view.running), 0);
}

#[gpui::test]
fn a_moved_tracking_ref_reads_again_and_tops_the_period_up(cx: &mut TestAppContext) {
    let rig = Rig::new(cx);
    rig.hold_fetch(cx);
    let loads = rig.fake.loads().len();
    script_fetch(&rig.fake, vec![Ok(FetchOutcome::Fetched { moved: true })]);
    rig.fake.with(|s| {
        s.load.once.push_back(Ok(synced("h1")));
        s.load.once.push_back(Ok(moved("h2")));
    });
    rig.act(cx, |view, cx| view.refresh(true, cx));
    // The refresh itself never waits for the network.
    assert_eq!(rig.fake.loads().len(), loads + 1);
    rig.release_all(cx);
    assert_eq!(rig.fake.loads().len(), loads + 2);
    let starts = rig.fake.starts();
    assert_eq!(starts.len(), 1);
    assert_eq!(
        (starts[0].mode, starts[0].old_head.as_deref()),
        ("topup", Some("old1"))
    );
    assert_eq!(rig.running(cx), 0);
}

#[gpui::test]
fn a_tracking_ref_that_did_not_move_reads_nothing_more(cx: &mut TestAppContext) {
    let rig = Rig::new(cx);
    rig.hold_fetch(cx);
    let loads = rig.fake.loads().len();
    script_fetch(&rig.fake, vec![Ok(FetchOutcome::Fetched { moved: false })]);
    rig.act(cx, |view, cx| view.refresh(true, cx));
    rig.release_all(cx);
    assert_eq!(rig.fake.loads().len(), loads + 1);
    assert!(rig.fake.starts().is_empty());
}

#[gpui::test]
fn all_time_reads_its_first_commit_again_when_the_ref_moved(cx: &mut TestAppContext) {
    let fake = Fake::new();
    fake.with(|s| s.saved_range = Some("all".into()));
    let rig = Rig::with(cx, fake, start_of_tests(), false);
    rig.hold_fetch(cx);
    script_fetch(&rig.fake, vec![Ok(FetchOutcome::Fetched { moved: true })]);
    rig.act(cx, |view, cx| view.refresh(true, cx));
    let firsts = rig.fake.count(|c| matches!(c, Call::First(_)));
    rig.release_all(cx);
    assert_eq!(rig.fake.count(|c| matches!(c, Call::First(_))), firsts + 1);
}

#[gpui::test]
fn a_failed_fetch_keeps_the_data_and_shows_a_note_with_the_time(cx: &mut TestAppContext) {
    let rig = Rig::new(cx);
    script_fetch(
        &rig.fake,
        vec![Ok(FetchOutcome::Failed(
            "fatal: unable to access 'https://user:secret@host/r.git'".into(),
        ))],
    );
    rig.act(cx, |view, cx| view.refresh(true, cx));
    let snap = rig.snap(cx);
    assert_eq!(
        snap.origin_note.as_deref(),
        Some("Data as of 09:55, could not reach origin")
    );
    assert_eq!(snap.status, Status::Ready);
    assert!(snap.error.is_none() && snap.model.is_some());
    assert!(!snap.fetching);
    assert_eq!(rig.running(cx), 0);
    // The next success clears it.
    rig.act(cx, |view, cx| view.refresh(true, cx));
    assert!(rig.snap(cx).origin_note.is_none());
}

#[gpui::test]
fn an_error_of_the_call_is_a_failed_fetch_too(cx: &mut TestAppContext) {
    let rig = Rig::new(cx);
    script_fetch(&rig.fake, vec![Err("Not a git repository".into())]);
    rig.act(cx, |view, cx| view.refresh(true, cx));
    let snap = rig.snap(cx);
    assert!(snap.origin_note.is_some());
    assert!(snap.error.is_none());
}

#[gpui::test]
fn the_note_belongs_to_the_branch_that_failed(cx: &mut TestAppContext) {
    let rig = Rig::new(cx);
    script_fetch(&rig.fake, vec![Ok(FetchOutcome::Failed("x".into()))]);
    rig.act(cx, |view, cx| view.refresh(true, cx));
    assert!(rig.snap(cx).origin_note.is_some());
    rig.act(cx, |view, cx| view.set_branch(Some("dev".into()), cx));
    assert!(rig.snap(cx).origin_note.is_none());
    // A good fetch of another branch must not wipe main's note.
    rig.act(cx, |view, cx| view.refresh(true, cx));
    assert!(rig.snap(cx).origin_note.is_none());
    rig.act(cx, |view, cx| view.set_branch(Some("main".into()), cx));
    assert!(rig.snap(cx).origin_note.is_some());
    // Without a time of the last sync the note still says it.
    rig.fake.set_load(unsynced());
    rig.act(cx, |view, cx| view.refresh(false, cx));
    rig.act(cx, |view, cx| view.set_kind(PeriodKind::Month, cx));
    assert_eq!(
        rig.snap(cx).origin_note.as_deref(),
        Some("Could not reach origin")
    );
}

#[gpui::test]
fn skipped_and_cancelled_fetches_clear_the_note(cx: &mut TestAppContext) {
    let rig = Rig::new(cx);
    for outcome in [FetchOutcome::Skipped, FetchOutcome::Cancelled] {
        script_fetch(
            &rig.fake,
            vec![Ok(FetchOutcome::Failed("x".into())), Ok(outcome.clone())],
        );
        rig.act(cx, |view, cx| view.refresh(true, cx));
        assert!(rig.snap(cx).origin_note.is_some());
        rig.act(cx, |view, cx| view.refresh(true, cx));
        assert!(rig.snap(cx).origin_note.is_none(), "{outcome:?}");
    }
}

#[gpui::test]
fn a_second_refresh_joins_the_fetch_and_the_spinner_stops_on_every_outcome(
    cx: &mut TestAppContext,
) {
    let outcomes: Vec<Result<FetchOutcome, String>> = vec![
        Ok(FetchOutcome::Fetched { moved: false }),
        Ok(FetchOutcome::Fetched { moved: true }),
        Ok(FetchOutcome::Skipped),
        Ok(FetchOutcome::Busy),
        Ok(FetchOutcome::Cancelled),
        Ok(FetchOutcome::Failed("x".into())),
        Err("boom".into()),
    ];
    for outcome in outcomes {
        let rig = Rig::new(cx);
        rig.hold_fetch(cx);
        script_fetch(&rig.fake, vec![outcome.clone()]);
        rig.act(cx, |view, cx| view.refresh(true, cx));
        rig.act(cx, |view, cx| view.refresh(true, cx));
        assert_eq!(rig.fetches().len(), 1, "{outcome:?}");
        assert!(rig.snap(cx).fetching && rig.snap(cx).refreshing);
        rig.release_all(cx);
        let snap = rig.snap(cx);
        assert!(!snap.fetching && !snap.refreshing, "{outcome:?}");
        assert_eq!(rig.running(cx), 0, "{outcome:?}");
    }
}

#[gpui::test]
fn sync_waits_for_the_fetch_and_then_starts_a_full_job(cx: &mut TestAppContext) {
    let rig = Rig::new(cx);
    rig.hold_fetch(cx);
    rig.act(cx, |view, cx| view.start_sync(cx));
    assert!(rig.fake.starts().is_empty());
    assert!(rig.snap(cx).fetching);
    // A press while it runs joins it.
    rig.act(cx, |view, cx| view.start_sync(cx));
    assert_eq!(rig.fetches().len(), 1);
    rig.release(cx, "fetch");
    let starts = rig.fake.starts();
    assert_eq!(starts.len(), 1);
    assert_eq!(starts[0].mode, "full");
    let calls = rig.fake.calls();
    let at = |pred: fn(&Call) -> bool| calls.iter().position(pred).unwrap();
    assert!(at(|c| matches!(c, Call::Fetch(_))) < at(|c| matches!(c, Call::Start(_))));
}

#[gpui::test]
fn sync_starts_on_what_is_there_after_a_failed_fetch(cx: &mut TestAppContext) {
    for outcome in [
        Ok(FetchOutcome::Failed("x".into())),
        Err("boom".into()),
        Ok(FetchOutcome::Skipped),
        Ok(FetchOutcome::Busy),
        Ok(FetchOutcome::Fetched { moved: false }),
    ] {
        let rig = Rig::new(cx);
        script_fetch(&rig.fake, vec![outcome.clone()]);
        rig.act(cx, |view, cx| view.start_sync(cx));
        assert_eq!(rig.fake.starts().len(), 1, "{outcome:?}");
    }
}

#[gpui::test]
fn sync_does_not_start_after_a_cancel_or_when_the_period_changed(cx: &mut TestAppContext) {
    let rig = Rig::new(cx);
    rig.hold_fetch(cx);
    script_fetch(&rig.fake, vec![Ok(FetchOutcome::Cancelled)]);
    rig.act(cx, |view, cx| view.start_sync(cx));
    rig.release_all(cx);
    assert!(rig.fake.starts().is_empty());

    let rig = Rig::new(cx);
    rig.hold_fetch(cx);
    rig.act(cx, |view, cx| view.start_sync(cx));
    rig.act(cx, |view, cx| view.set_kind(PeriodKind::Month, cx));
    rig.release_all(cx);
    assert!(rig.fake.starts().is_empty());
}

#[gpui::test]
fn cancel_reaches_the_core_while_only_a_fetch_runs(cx: &mut TestAppContext) {
    let rig = Rig::new(cx);
    rig.hold_fetch(cx);
    rig.act(cx, |view, cx| view.refresh(true, cx));
    let snap = rig.snap(cx);
    assert!(snap.fetching && snap.job.is_none());
    rig.act(cx, |view, cx| view.cancel_sync(cx));
    assert_eq!(rig.fake.count(|c| matches!(c, Call::Cancel)), 1);
}

#[gpui::test]
fn retry_github_does_not_fetch(cx: &mut TestAppContext) {
    let rig = Rig::new(cx);
    rig.act(cx, |view, cx| view.retry_github(cx));
    assert!(rig.fetches().is_empty());
    assert_eq!(rig.fake.starts()[0].mode, "github");
}

#[test]
fn the_origin_note_names_the_time_of_the_sync() {
    use super::data::OriginIssue::{Gone, Unreachable};
    let now = local(2026, 10, 15, 10, 0);
    let at = |t: DateTime<Local>| Some(t.timestamp());
    assert_eq!(
        origin_note_text(Unreachable, at(local(2026, 10, 15, 8, 5)), &now),
        "Data as of 08:05, could not reach origin"
    );
    assert_eq!(
        origin_note_text(Unreachable, at(local(2026, 10, 3, 14, 30)), &now),
        "Data as of Oct 3, 14:30, could not reach origin"
    );
    assert_eq!(
        origin_note_text(Unreachable, None, &now),
        "Could not reach origin"
    );
    assert_eq!(
        origin_note_text(Gone, at(local(2026, 10, 15, 14, 30)), &now),
        "Data as of 14:30, origin no longer has this branch"
    );
    assert_eq!(
        origin_note_text(Gone, None, &now),
        "Origin no longer has this branch"
    );
}

#[gpui::test]
fn a_branch_origin_dropped_shows_its_own_note_and_keeps_the_data(cx: &mut TestAppContext) {
    let rig = Rig::new(cx);
    script_fetch(&rig.fake, vec![Ok(FetchOutcome::Gone)]);
    rig.act(cx, |view, cx| view.refresh(true, cx));
    let snap = rig.snap(cx);
    assert_eq!(
        snap.origin_note.as_deref(),
        Some("Data as of 09:55, origin no longer has this branch")
    );
    assert!(snap.error.is_none() && snap.model.is_some());
    rig.act(cx, |view, cx| view.refresh(true, cx));
    assert!(rig.snap(cx).origin_note.is_none());
}

#[gpui::test]
fn a_busy_fetch_leaves_the_note_as_it_was(cx: &mut TestAppContext) {
    let rig = Rig::new(cx);
    script_fetch(
        &rig.fake,
        vec![Ok(FetchOutcome::Failed("x".into())), Ok(FetchOutcome::Busy)],
    );
    rig.act(cx, |view, cx| view.refresh(true, cx));
    rig.act(cx, |view, cx| view.refresh(true, cx));
    assert!(rig.snap(cx).origin_note.is_some());
}

#[gpui::test]
fn sync_of_a_covered_period_whose_branch_moved_leaves_the_work_to_the_top_up(
    cx: &mut TestAppContext,
) {
    let rig = Rig::new(cx);
    rig.hold_fetch(cx);
    script_fetch(&rig.fake, vec![Ok(FetchOutcome::Fetched { moved: true })]);
    rig.fake.with(|s| s.load.once.push_back(Ok(moved("h2"))));
    let loads = rig.fake.loads().len();
    rig.act(cx, |view, cx| view.start_sync(cx));
    rig.release_all(cx);
    // The reload happened, and the incremental sync is the top-up, not Full.
    assert_eq!(rig.fake.loads().len(), loads + 1);
    let modes: Vec<&str> = rig.fake.starts().iter().map(|c| c.mode).collect();
    assert_eq!(modes, ["topup"]);
}

#[gpui::test]
fn sync_of_a_covered_period_whose_branch_did_not_move_still_syncs(cx: &mut TestAppContext) {
    let rig = Rig::new(cx);
    script_fetch(&rig.fake, vec![Ok(FetchOutcome::Fetched { moved: false })]);
    rig.act(cx, |view, cx| view.start_sync(cx));
    let modes: Vec<&str> = rig.fake.starts().iter().map(|c| c.mode).collect();
    assert_eq!(modes, ["full"]);
}

#[gpui::test]
fn sync_of_an_unsynced_period_syncs_even_when_the_branch_moved(cx: &mut TestAppContext) {
    let fake = Fake::new();
    fake.set_load(unsynced());
    let rig = Rig::with(cx, fake, start_of_tests(), false);
    script_fetch(&rig.fake, vec![Ok(FetchOutcome::Fetched { moved: true })]);
    rig.act(cx, |view, cx| view.start_sync(cx));
    let modes: Vec<&str> = rig.fake.starts().iter().map(|c| c.mode).collect();
    assert_eq!(modes, ["full"]);
}

#[gpui::test]
fn cancel_while_the_fetch_runs_stops_the_waiting_sync(cx: &mut TestAppContext) {
    let rig = Rig::new(cx);
    rig.hold_fetch(cx);
    rig.act(cx, |view, cx| view.start_sync(cx));
    rig.act(cx, |view, cx| view.cancel_sync(cx));
    // The fetch finished before it saw the cancel.
    rig.release_all(cx);
    assert!(rig.fake.starts().is_empty());
}
