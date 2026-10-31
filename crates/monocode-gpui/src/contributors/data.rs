//! The rules of the GitHub tab: what `useContributors.ts` does, as plain
//! state on `GithubView`. Every outside call goes through the `Backend` on
//! the background executor; every answer comes back through a generation
//! check, so a late answer for an abandoned request cannot change what the
//! page shows.
use super::backend::ColorOverrides;
use super::chart::ChartKind;
use super::github::GithubView;
use super::jobs::{JobsEvent, SyncJobs};
use chrono::{DateTime, Datelike, Local, TimeZone};
use gpui::{Context, Entity};
use monocode_core::contributors::{
    ContributorsBranches, GitUser, RawData,
    fetch::FetchOutcome,
    jobs::{LoadResponse, SyncEvent},
    model::{
        config::{CONFIG_PATH, DEFAULT_CONFIG, parse_config_result, serialize_config},
        identity::{MailmapPerson, parse_mailmap},
        period::{
            Period, PeriodKind, PeriodPick, next_period, parse_iso, pick_of, previous_period,
            resolve_pick, sync_window,
        },
        pipeline::{ContributorsModel, ModelInput, build_contributors_model},
        sync_jobs::{StartJob, epoch_seconds, is_live, iso_seconds, job_key},
        types::ContributorsConfig,
    },
    sync::{Estimate, SyncMode},
};
use std::{cell::Cell, collections::VecDeque, rc::Rc, sync::Arc, time::Duration};

/// Loads and first-commit reads kept per project: the window moves daily and
/// every period and branch adds one.
pub(super) const CACHE_LIMIT: usize = 6;
/// Minimum gap between reloads triggered by the window coming back.
const AUTO_RELOAD_MS: i64 = 15_000;

/// The wall clock; the tests move it.
pub(crate) type Clock = Rc<dyn Fn() -> DateTime<Local>>;

pub(crate) fn system_clock() -> Clock {
    Rc::new(Local::now)
}

/// A small least-recently-used map.
pub(super) struct Lru<V> {
    items: Vec<(String, V)>,
}
impl<V> Default for Lru<V> {
    fn default() -> Self {
        Self { items: Vec::new() }
    }
}
impl<V: Clone> Lru<V> {
    pub(super) fn remember(&mut self, key: &str, value: V) {
        self.items.retain(|(k, _)| k != key);
        self.items.push((key.to_string(), value));
        while self.items.len() > CACHE_LIMIT {
            self.items.remove(0);
        }
    }
    pub(super) fn peek(&self, key: &str) -> Option<&V> {
        self.items.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }
    pub(super) fn has(&self, key: &str) -> bool {
        self.peek(key).is_some()
    }
    /// Marks `key` as the most recently used.
    pub(super) fn touch(&mut self, key: &str) {
        if let Some(at) = self.items.iter().position(|(k, _)| k == key) {
            let item = self.items.remove(at);
            self.items.push(item);
        }
    }
    pub(super) fn remove_prefix(&mut self, prefix: &str) {
        self.items.retain(|(k, _)| !k.starts_with(prefix));
    }
    #[cfg(test)]
    pub(super) fn len(&self) -> usize {
        self.items.len()
    }
}

/// One period read from the sync store, with its data shared (model builds
/// and the "last shown" memory hold the same `Arc`).
#[derive(Clone)]
pub(crate) struct Loaded {
    /// Unique per stored answer: a changed `id` is a changed store.
    pub id: u64,
    pub raw: Arc<RawData>,
    pub covered: bool,
    pub has_gaps: bool,
    pub missing_commits: u64,
    pub synced_at: Option<i64>,
    pub coverage_head: Option<String>,
    pub head: String,
    pub gh_state: Option<String>,
}
impl Loaded {
    fn new(id: u64, response: LoadResponse) -> Self {
        Self {
            id,
            raw: Arc::new(response.raw),
            covered: response.covered,
            has_gaps: response.has_gaps,
            missing_commits: response.missing_commits,
            synced_at: response.synced_at,
            coverage_head: response.coverage_head,
            head: response.head,
            gh_state: response.gh_state,
        }
    }
    /// The same state of the store: keep the object so a focus revalidation
    /// does not rescore.
    fn same_as(&self, other: &Self) -> bool {
        self.covered == other.covered
            && self.missing_commits == other.missing_commits
            && self.synced_at == other.synced_at
            && self.coverage_head == other.coverage_head
            && self.head == other.head
            && self.gh_state == other.gh_state
            && same_data(&self.raw, &other.raw)
    }
}

fn same_data(a: &RawData, b: &RawData) -> bool {
    a.head == b.head
        && a.branch == b.branch
        && a.since == b.since
        && a.gh_available == b.gh_available
        && a.gh_note == b.gh_note
        && a.truncated == b.truncated
        && a.commits.len() == b.commits.len()
        && a.prs.len() == b.prs.len()
        && a.reviews.len() == b.reviews.len()
        && a.survival.len() == b.survival.len()
}

/// What the project's files said.
#[derive(Clone)]
pub(super) struct Meta {
    /// What the page shows; moves ahead of `confirmed` while a save is in flight.
    pub config: Arc<ContributorsConfig>,
    /// The last config known to be on disk.
    pub confirmed: Arc<ContributorsConfig>,
    /// Why the file on disk cannot be used; saving is refused while it is set.
    pub config_error: Option<String>,
    pub mailmap: Arc<Vec<MailmapPerson>>,
    pub branches: ContributorsBranches,
    pub user: Option<GitUser>,
}

struct MetaRead {
    config: ContributorsConfig,
    error: Option<String>,
    mailmap: Vec<MailmapPerson>,
    branches: ContributorsBranches,
    user: Option<GitUser>,
}

/// Writes of the project's config, one at a time and in order.
#[derive(Default)]
pub(super) struct SaveState {
    pub pending: usize,
    /// Bumped when a save starts and when it ends, so a reload that overlapped one can tell.
    pub epoch: u64,
    pub latest: Option<Arc<ContributorsConfig>>,
    queue: VecDeque<Arc<ContributorsConfig>>,
    writing: bool,
}

pub(super) struct EstimateSlot {
    pub key: String,
    pub value: Option<Estimate>,
    pub error: Option<String>,
}

/// The inputs of the last model build, to tell when another is due.
#[derive(Clone)]
pub(super) struct ModelInputs {
    raw: Arc<RawData>,
    config: Arc<ContributorsConfig>,
    mailmap: Arc<Vec<MailmapPerson>>,
    merge: bool,
    period: Period,
    now: DateTime<Local>,
    user: Option<GitUser>,
    color_overrides: ColorOverrides,
}
impl ModelInputs {
    fn same_as(&self, other: &Self) -> bool {
        self.color_overrides == other.color_overrides && self.same_but_colors(other)
    }

    /// Everything but the colors the user picked.
    fn same_but_colors(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.raw, &other.raw)
            && (Arc::ptr_eq(&self.config, &other.config) || self.config == other.config)
            && (Arc::ptr_eq(&self.mailmap, &other.mailmap) || self.mailmap == other.mailmap)
            && self.merge == other.merge
            && self.period == other.period
            && self.now == other.now
            && self.user == other.user
    }
}

/// Which effects ran for which inputs, so a settle runs one only when its
/// inputs changed (the `useEffect` dependency lists).
#[derive(Default)]
pub(super) struct Effects {
    load: Option<(String, bool)>,
    first: Option<String>,
    estimate: Option<(String, u64)>,
}

/// The fetch of `origin` in flight.
pub(super) struct Fetching {
    /// The branch key it was started for; a failure is remembered under it.
    pub first_key: String,
    /// A Sync pressed meanwhile: it starts when the fetch ends.
    pub sync_after: Option<SyncAfter>,
    /// What the note of `first_key` was before this fetch cleared it, put back
    /// when the fetch found out nothing (`Busy`).
    pub prior: Option<OriginIssue>,
}

/// A Sync waiting for the fetch.
pub(super) struct SyncAfter {
    /// The period key at the press; a different one at the end drops the sync.
    pub key: String,
    /// The period was fully synced at the press.
    pub covered: bool,
}

/// What the last fetch of a branch found out, shown as a note.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum OriginIssue {
    Unreachable,
    /// Origin no longer has the branch.
    Gone,
}

/// Everything the effects and the view read, worked out from the state.
pub(super) struct Derived {
    pub period: Period,
    pub since: String,
    pub until: String,
    pub key: String,
    pub first_key: String,
    pub first_scope: String,
    pub meta_scope: String,
    pub wants_all: bool,
    pub awaiting_first: bool,
    pub meta_ready: bool,
    pub loaded: Option<Arc<Loaded>>,
    pub repair: bool,
    pub not_synced: bool,
    pub shown: Option<Arc<RawData>>,
    pub shown_period: Period,
    pub this_job: Option<SyncEvent>,
    pub live_job: Option<SyncEvent>,
    pub failed: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Status {
    Loading,
    Ready,
    Error,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SyncState {
    Loading,
    NotSynced,
    Synced,
}

/// What `useContributors` returns, for the render and the tabs.
pub(crate) struct Snapshot {
    pub status: Status,
    pub error: Option<String>,
    pub refreshing: bool,
    /// A fetch of `origin` runs (and maybe no job): the bar offers Cancel.
    pub fetching: bool,
    /// The last fetch for this branch could not reach origin; the data shown may be behind.
    pub origin_note: Option<String>,
    /// The data on screen is for the previous period or branch while the new one loads.
    pub stale: bool,
    pub sync_state: SyncState,
    /// What is left to read; only for `NotSynced`.
    pub estimate: Option<Estimate>,
    pub estimate_error: Option<String>,
    /// Epoch seconds of the last sync of the period.
    pub synced_at: Option<i64>,
    /// The data is shown but GitHub's part is missing or partial.
    pub gh_incomplete: bool,
    /// The queued or running job of this period.
    pub job: Option<SyncEvent>,
    pub job_error: Option<String>,
    pub action_error: Option<String>,
    #[allow(dead_code)] // The tabs read it (6.8 to 6.10).
    pub raw: Option<Arc<RawData>>,
    pub model: Option<Arc<ContributorsModel>>,
    /// The model for the current inputs is still being built.
    pub model_pending: bool,
    #[allow(dead_code)] // The tabs read it (6.8 to 6.10).
    pub config: Arc<ContributorsConfig>,
    pub save_error: Option<String>,
    pub config_error: Option<String>,
    pub branches: Vec<String>,
    pub branch: String,
    /// The selected period; the model shows it unless `stale`.
    pub period: Period,
    /// False for 7d, All time and the current period: nothing lies ahead.
    pub has_next: bool,
    pub merge: bool,
}

fn same_day(a: &DateTime<Local>, b: &DateTime<Local>) -> bool {
    a.year() == b.year() && a.month() == b.month() && a.day() == b.day()
}

/// From `clock_now` to one second past the local midnight that ends `now`'s day.
fn until_next_day(now: &DateTime<Local>, clock_now: &DateTime<Local>) -> Duration {
    let next = now
        .date_naive()
        .succ_opt()
        .and_then(|day| day.and_hms_opt(0, 0, 1))
        .and_then(|time| Local.from_local_datetime(&time).earliest());
    let ms = next.map_or(60_000, |next| (next - *clock_now).num_milliseconds());
    Duration::from_millis(ms.max(1000) as u64)
}

/// "Data as of 14:30, could not reach origin": the time the period was last
/// synced, as a clock time when that was today (by `now`), else with the date.
pub(super) fn origin_note_text(
    issue: OriginIssue,
    synced_at: Option<i64>,
    now: &DateTime<Local>,
) -> String {
    let reason = match issue {
        OriginIssue::Unreachable => "could not reach origin",
        OriginIssue::Gone => "origin no longer has this branch",
    };
    let at = synced_at
        .and_then(|secs| DateTime::from_timestamp(secs, 0))
        .map(|at| at.with_timezone(&Local));
    let Some(at) = at else {
        let mut text = reason.to_string();
        text[..1].make_ascii_uppercase();
        return text;
    };
    let stamp = if same_day(&at, now) {
        at.format("%H:%M")
    } else {
        at.format("%b %-d, %H:%M")
    };
    format!("Data as of {stamp}, {reason}")
}

fn meta_scope(cwd: &str) -> String {
    format!("meta\u{0}{cwd}")
}

impl GithubView {
    // ---- plumbing -------------------------------------------------------

    /// Runs `work` on the background executor and hands its answer to
    /// `finish` on the UI thread. The tests can hold the answers back and deliver
    /// them in any order (`manual`).
    pub(super) fn spawn_job<T: Send + 'static>(
        &mut self,
        label: &'static str,
        cx: &mut Context<Self>,
        work: impl FnOnce(Arc<dyn super::backend::Backend>) -> T + Send + 'static,
        finish: impl FnOnce(&mut Self, T, &mut Context<Self>) + 'static,
    ) {
        let Some(backend) = self.backend.clone() else {
            return;
        };
        #[cfg(test)]
        if self.manual {
            // The call is made now; only its answer is held back.
            let answer = work(backend);
            if label == "fetch" && !self.hold_fetch {
                finish(self, answer, cx);
                return;
            }
            self.held.push(super::github::Held {
                label,
                run: Some(Box::new(move |this, cx| finish(this, answer, cx))),
            });
            return;
        }
        let _ = label;
        let task = cx.background_executor().spawn(async move { work(backend) });
        cx.spawn(async move |this, cx| {
            let answer = task.await;
            let _ = this.update(cx, |this, cx| finish(this, answer, cx));
        })
        .detach();
    }

    fn can_run(&self) -> bool {
        !self.cwd.is_empty() && self.backend.is_some() && self.prefs_ready
    }

    fn next_seq(&mut self) -> u64 {
        self.seq += 1;
        self.seq
    }

    fn fail(&mut self, scope: String, message: String) {
        self.failures.insert(scope, message);
    }

    fn clear_failure(&mut self, scope: &str) {
        self.failures.remove(scope);
    }

    // ---- derived state --------------------------------------------------

    pub(super) fn derive(&self, cx: &gpui::App) -> Derived {
        let cwd = &self.cwd;
        let branch = self.picked_branch.as_deref();
        let current_branch = self.meta.as_ref().map(|m| m.branches.current.as_str());
        let first_key = format!("{cwd}\u{0}{}", branch.or(current_branch).unwrap_or(""));
        let wants_all = self.pick.kind == PeriodKind::All;
        // Outer `None`: not known yet. Inner `None`: the repository has no commits.
        let first_iso = wants_all
            .then(|| self.first.peek(&first_key).cloned())
            .flatten();
        let first_date: Option<Option<DateTime<Local>>> =
            first_iso.map(|iso| iso.and_then(|iso| parse_iso(&iso)));
        let awaiting_first = wants_all && first_date.is_none();
        let period = resolve_pick(
            &self.pick,
            &self.now,
            first_date.as_ref().and_then(Option::as_ref),
        );
        let window = sync_window(&period, &self.now);
        let since = iso_seconds(&window.from);
        let until = iso_seconds(&period.end);
        let key = format!("{cwd}\u{0}{}\u{0}{since}\u{0}{until}", branch.unwrap_or(""));
        let meta_ready = self.meta.is_some();
        let loaded = if awaiting_first {
            None
        } else {
            self.loads.peek(&key).cloned()
        };
        // A synced period that lacks some commits (a merge brought older ones
        // in) keeps its stored numbers on screen while a background sync
        // repairs it.
        let repair = loaded.as_ref().is_some_and(|l| {
            !l.covered && !l.has_gaps && l.missing_commits > 0 && l.coverage_head.is_some()
        });
        // Only a synced period has numbers; a gap shows the sync state instead.
        let current = loaded
            .as_ref()
            .filter(|l| l.covered || repair)
            .map(|l| l.raw.clone());
        // While a new period or branch loads, keep the last data on screen
        // (marked stale) instead of blanking the page.
        let previous = self.last_shown.as_ref();
        let shown = if loaded.is_some() {
            current
        } else {
            previous.map(|(raw, _)| raw.clone())
        };
        let shown_period = if loaded.is_some() {
            period.clone()
        } else {
            previous
                .map(|(_, period)| period.clone())
                .unwrap_or_else(|| period.clone())
        };
        let first_scope = format!("first\u{0}{first_key}");
        let meta_scope = meta_scope(cwd);
        let failed = self
            .failures
            .get(&key)
            .or_else(|| self.failures.get(&meta_scope))
            .or_else(|| {
                awaiting_first
                    .then(|| self.failures.get(&first_scope))
                    .flatten()
            })
            .cloned();
        let this_job = loaded.as_ref().and_then(|l| {
            let job = job_key(
                cwd,
                &l.raw.branch,
                epoch_seconds(&window.from),
                epoch_seconds(&period.end),
            );
            self.jobs.read(cx).job(&job).cloned()
        });
        let live_job = this_job.clone().filter(is_live);
        let not_synced = loaded.is_some() && !loaded.as_ref().is_some_and(|l| l.covered) && !repair;
        Derived {
            period,
            since,
            until,
            key,
            first_key,
            first_scope,
            meta_scope,
            wants_all,
            awaiting_first,
            meta_ready,
            loaded,
            repair,
            not_synced,
            shown,
            shown_period,
            this_job,
            live_job,
            failed,
        }
    }

    pub(crate) fn snapshot(&self, cx: &gpui::App) -> Snapshot {
        let d = self.derive(cx);
        let status = if self.meta.is_some() && (d.loaded.is_some() || d.shown.is_some()) {
            Status::Ready
        } else if d.failed.is_some() {
            Status::Error
        } else {
            Status::Loading
        };
        let loaded = d.loaded.as_deref();
        let slot = self.estimate.as_ref().filter(|e| e.key == d.key);
        let job_error = d
            .this_job
            .as_ref()
            .filter(|job| job.state == monocode_core::contributors::jobs::JobState::Error)
            .map(|job| {
                job.error
                    .clone()
                    .unwrap_or_else(|| "The sync failed.".into())
            });
        let meta = self.meta.as_ref();
        Snapshot {
            status,
            error: d.failed.clone(),
            refreshing: self.running > 0,
            fetching: self.fetching.is_some(),
            origin_note: self.origin_issues.get(&d.first_key).map(|issue| {
                origin_note_text(*issue, loaded.and_then(|l| l.synced_at), &(self.clock)())
            }),
            stale: d.loaded.is_none() && d.shown.is_some(),
            sync_state: if d.loaded.is_none() {
                SyncState::Loading
            } else if d.not_synced {
                SyncState::NotSynced
            } else {
                SyncState::Synced
            },
            estimate: d
                .not_synced
                .then(|| slot.and_then(|s| s.value.clone()))
                .flatten(),
            estimate_error: d
                .not_synced
                .then(|| slot.and_then(|s| s.error.clone()))
                .flatten(),
            synced_at: loaded.and_then(|l| l.synced_at),
            gh_incomplete: loaded.is_some_and(|l| {
                !d.not_synced && matches!(l.gh_state.as_deref(), Some("partial" | "unavailable"))
            }),
            job: d.live_job.clone(),
            job_error,
            action_error: self.action_error.clone(),
            raw: d.shown.clone(),
            model: self.model.clone(),
            // A recolor rebuilds the model too, but nothing on screen is out of date.
            model_pending: self.model_requested != self.model_done && !self.model_quiet,
            config: meta.map_or_else(
                || Arc::new(DEFAULT_CONFIG.clone()),
                |meta| meta.config.clone(),
            ),
            save_error: self.save_error.clone(),
            config_error: meta.and_then(|meta| meta.config_error.clone()),
            branches: meta.map_or_else(Vec::new, |meta| meta.branches.branches.clone()),
            branch: self
                .picked_branch
                .clone()
                .or_else(|| d.shown.as_ref().map(|raw| raw.branch.clone()))
                .or_else(|| meta.map(|meta| meta.branches.current.clone()))
                .unwrap_or_default(),
            has_next: next_period(&d.period, &self.now).is_some(),
            period: d.period,
            merge: self.merge,
        }
    }

    // ---- the effects ----------------------------------------------------

    /// Brings everything in line with the state: runs the effects whose
    /// inputs changed, refreshes the model and redraws. Call it after every
    /// change of state.
    pub(super) fn settle(&mut self, cx: &mut Context<Self>) {
        let d = self.derive(cx);
        if let Some(raw) = &d.shown {
            self.last_shown = Some((raw.clone(), d.shown_period.clone()));
        }
        if d.loaded.is_some() {
            self.loads.touch(&d.key);
        }
        self.arm_midnight(cx);
        if self.can_run() {
            self.effect_load(&d, cx);
            self.effect_first(&d, cx);
            self.effect_estimate(&d, cx);
            self.effect_topup(&d, cx);
            self.effect_repair(&d, cx);
            self.effect_head(&d, cx);
        }
        self.refresh_model(&d, cx);
        cx.notify();
    }

    fn effect_load(&mut self, d: &Derived, cx: &mut Context<Self>) {
        let sig = (d.key.clone(), d.awaiting_first);
        if self.effects.load.as_ref() != Some(&sig) {
            self.effects.load = Some(sig);
            self.load(false, false, cx);
        }
    }

    fn effect_first(&mut self, d: &Derived, cx: &mut Context<Self>) {
        let sig = (d.wants_all && d.meta_ready).then(|| d.first_key.clone());
        if sig != self.effects.first {
            self.effects.first = sig.clone();
            if let Some(key) = sig
                && !self.first.has(&key)
            {
                self.fetch_first(false, cx);
            }
        }
    }

    /// What is left to read, for a period that has to be synced by hand.
    fn effect_estimate(&mut self, d: &Derived, cx: &mut Context<Self>) {
        let sig = (d.not_synced && d.live_job.is_none())
            .then(|| (d.key.clone(), d.loaded.as_ref().map_or(0, |l| l.id)));
        if sig == self.effects.estimate {
            return;
        }
        self.effects.estimate = sig.clone();
        // An answer for the previous inputs is stale now.
        self.estimate_gen += 1;
        if sig.is_none() {
            return;
        }
        let generation = self.estimate_gen;
        let (cwd, branch) = (self.cwd.clone(), self.picked_branch.clone());
        let (from, to, key) = (d.since.clone(), d.until.clone(), d.key.clone());
        self.spawn_job(
            "estimate",
            cx,
            move |backend| backend.estimate(&cwd, branch.as_deref(), &from, &to),
            move |this, answer, cx| {
                if this.estimate_gen == generation {
                    this.estimate = Some(match answer {
                        Ok(value) => EstimateSlot {
                            key,
                            value: Some(value),
                            error: None,
                        },
                        Err(error) => EstimateSlot {
                            key,
                            value: None,
                            error: Some(error),
                        },
                    });
                    this.settle(cx);
                }
            },
        );
    }

    /// A synced period whose branch moved since the sync is topped up once
    /// per head. A gap at the same head is not a top-up: the user syncs it
    /// again.
    fn effect_topup(&mut self, d: &Derived, cx: &mut Context<Self>) {
        let Some(loaded) = d.loaded.as_deref() else {
            return;
        };
        let Some(old_head) = loaded.coverage_head.clone() else {
            return;
        };
        if !loaded.covered || old_head == loaded.head || d.live_job.is_some() {
            return;
        }
        if self
            .topped_up
            .insert(format!("{}\u{0}{}", d.key, loaded.head))
        {
            self.begin(SyncMode::TopUp { old_head }, d, cx);
        }
    }

    /// Commits the store lacks inside a synced period are read in the
    /// background, once per head; a full sync only repairs what is missing.
    fn effect_repair(&mut self, d: &Derived, cx: &mut Context<Self>) {
        let Some(loaded) = d.loaded.as_deref() else {
            return;
        };
        if !d.repair || d.live_job.is_some() {
            return;
        }
        if self
            .topped_up
            .insert(format!("{}\u{0}repair\u{0}{}", d.key, loaded.head))
        {
            self.begin(SyncMode::Full, d, cx);
        }
    }

    /// A merge of an older unrelated history moves the start of All time, and
    /// it arrives as a new branch head: read the first commit again when it
    /// changes.
    fn effect_head(&mut self, d: &Derived, cx: &mut Context<Self>) {
        let Some(loaded) = d.loaded.as_deref() else {
            return;
        };
        if !d.wants_all {
            return;
        }
        let seen = self
            .first_head
            .insert(d.first_key.clone(), loaded.head.clone());
        if seen.is_some_and(|seen| seen != loaded.head) {
            self.fetch_first(true, cx);
        }
    }

    // ---- reading --------------------------------------------------------

    /// Reads the project's files and the selected period. A plain read joins
    /// one already running; a forced reload always reads the files again (the
    /// user may just have fixed the file), and `fresh` also reads the store
    /// again. Counts as one refresh until every part it started has answered.
    pub(super) fn load(&mut self, force: bool, fresh: bool, cx: &mut Context<Self>) {
        if !self.can_run() {
            return;
        }
        let d = self.derive(cx);
        // A reload that overlaps a save of this project may have read the
        // file before or after the write, so it must not decide the config.
        let epoch = self.saves.epoch;
        let saving_at_start = self.saves.pending > 0;
        // A retry starts clean: the old message would sit next to the spinner.
        if force {
            self.clear_failure(&d.key);
            self.clear_failure(&d.meta_scope);
        }
        let remaining = Rc::new(Cell::new(0u8));
        let mut started = 0u8;

        let join_meta = !force && self.meta_inflight.is_some_and(|(e, _)| e == epoch);
        if !join_meta {
            started += 1;
            let seq = self.next_seq();
            self.meta_inflight = Some((epoch, seq));
            let cwd = self.cwd.clone();
            let done = remaining.clone();
            self.spawn_job(
                "meta",
                cx,
                move |backend| read_meta(backend.as_ref(), &cwd),
                move |this, answer, cx| {
                    if this.meta_inflight.is_some_and(|(_, s)| s == seq) {
                        this.meta_inflight = None;
                    }
                    this.finish_meta(answer, epoch, saving_at_start);
                    this.finish_part(&done, cx);
                },
            );
        }

        // The window of All time is unknown until its first commit is; the
        // project files above are still read, they tell which branch it is.
        if !d.awaiting_first {
            let join_period = !fresh && self.load_inflight.contains_key(&d.key);
            if !join_period {
                started += 1;
                let seq = self.next_seq();
                self.load_started.insert(d.key.clone(), seq);
                self.load_inflight.insert(d.key.clone(), seq);
                let (cwd, branch) = (self.cwd.clone(), self.picked_branch.clone());
                let (since, until, key) = (d.since.clone(), d.until.clone(), d.key.clone());
                let done = remaining.clone();
                self.spawn_job(
                    "period",
                    cx,
                    move |backend| backend.load(&cwd, branch.as_deref(), &since, &until),
                    move |this, answer, cx| {
                        if this.load_inflight.get(&key) == Some(&seq) {
                            this.load_inflight.remove(&key);
                        }
                        this.finish_period(&key, seq, answer);
                        this.finish_part(&done, cx);
                    },
                );
            }
        }
        if started > 0 {
            self.running += 1;
            remaining.set(started);
        }
    }

    fn finish_part(&mut self, remaining: &Rc<Cell<u8>>, cx: &mut Context<Self>) {
        remaining.set(remaining.get().saturating_sub(1));
        if remaining.get() == 0 {
            self.running = self.running.saturating_sub(1);
        }
        self.settle(cx);
    }

    fn finish_meta(&mut self, answer: Result<MetaRead, String>, epoch: u64, saving_at_start: bool) {
        let scope = meta_scope(&self.cwd);
        let read = match answer {
            Ok(read) => read,
            Err(error) => {
                self.fail(scope, error);
                return;
            }
        };
        let previous = self.meta.take();
        let settled = !saving_at_start && self.saves.pending == 0 && self.saves.epoch == epoch;
        let keep_config = settled || previous.is_none();
        // Keep object identities when nothing changed, so revalidating on
        // focus does not rescore.
        let keep = |old: Option<&Arc<ContributorsConfig>>, next: ContributorsConfig| match old {
            Some(old) if **old == next => old.clone(),
            _ => Arc::new(next),
        };
        let config = if keep_config {
            keep(previous.as_ref().map(|m| &m.config), read.config)
        } else {
            previous.as_ref().map(|m| m.config.clone()).unwrap()
        };
        let mailmap = match &previous {
            Some(m) if *m.mailmap == read.mailmap => m.mailmap.clone(),
            _ => Arc::new(read.mailmap),
        };
        let user = match &previous {
            Some(m) if m.user == read.user => m.user.clone(),
            _ => read.user,
        };
        self.meta = Some(Meta {
            confirmed: if keep_config {
                config.clone()
            } else {
                previous.as_ref().map(|m| m.confirmed.clone()).unwrap()
            },
            config_error: if keep_config {
                read.error
            } else {
                previous.as_ref().and_then(|m| m.config_error.clone())
            },
            config,
            mailmap,
            branches: read.branches,
            user,
        });
        self.clear_failure(&scope);
    }

    fn finish_period(&mut self, key: &str, seq: u64, answer: Result<LoadResponse, String>) {
        match answer {
            Ok(response) => {
                let id = self.next_loaded_id();
                let incoming = Arc::new(Loaded::new(id, response));
                let previous = self.loads.peek(key).cloned();
                if self.load_started.get(key).copied().unwrap_or(0) > seq {
                    // A read started after this one was asked for: its data is newer.
                    if previous.is_none() {
                        self.loads.remember(key, incoming);
                    }
                } else {
                    let next = match previous {
                        Some(previous) if previous.same_as(&incoming) => previous,
                        _ => incoming,
                    };
                    self.loads.remember(key, next);
                }
                self.clear_failure(key);
            }
            Err(error) => self.fail(key.to_string(), error),
        }
    }

    fn next_loaded_id(&mut self) -> u64 {
        self.loaded_ids += 1;
        self.loaded_ids
    }

    /// Reads where All time starts. A plain read joins one already running; a
    /// fresh one (a forced refresh, or a moved branch head) never does.
    /// Whichever read started last decides what is cached, and an answer that
    /// is not a date is an error, not an empty repository.
    pub(super) fn fetch_first(&mut self, fresh: bool, cx: &mut Context<Self>) {
        if !self.can_run() {
            return;
        }
        let d = self.derive(cx);
        let key = d.first_key.clone();
        if !fresh && self.first_inflight.contains_key(&key) {
            return;
        }
        let seq = self.next_seq();
        self.first_started.insert(key.clone(), seq);
        self.first_inflight.insert(key.clone(), seq);
        let (cwd, branch) = (self.cwd.clone(), self.picked_branch.clone());
        let scope = d.first_scope;
        self.spawn_job(
            "first-commit",
            cx,
            move |backend| backend.first_commit(&cwd, branch.as_deref()),
            move |this, answer, cx| {
                if this.first_inflight.get(&key) == Some(&seq) {
                    this.first_inflight.remove(&key);
                }
                let answer = answer.and_then(|iso| match &iso {
                    Some(text) if parse_iso(text).is_none() => {
                        Err(format!("Could not read the first commit date: {text}"))
                    }
                    _ => Ok(iso),
                });
                match answer {
                    Ok(iso) => {
                        // A read started after this one was asked for: its answer is newer.
                        let newer = this.first_started.get(&key).copied().unwrap_or(0) > seq;
                        if !newer || !this.first.has(&key) {
                            this.first.remember(&key, iso);
                        }
                        this.clear_failure(&scope);
                    }
                    Err(error) => this.fail(scope, error),
                }
                this.settle(cx);
            },
        );
    }

    // ---- the model ------------------------------------------------------

    /// Builds the model off the UI thread when its inputs changed. The one on
    /// screen stays until the new one is ready; an answer for older inputs is
    /// dropped.
    fn refresh_model(&mut self, d: &Derived, cx: &mut Context<Self>) {
        let Some(raw) = d.shown.clone() else {
            self.model = None;
            self.model_inputs = None;
            self.model_requested = self.model_done;
            self.model_gen += 1;
            return;
        };
        let (config, mailmap, user) = match &self.meta {
            Some(meta) => (meta.config.clone(), meta.mailmap.clone(), meta.user.clone()),
            None => (Arc::new(DEFAULT_CONFIG.clone()), Arc::default(), None),
        };
        let inputs = ModelInputs {
            raw,
            config,
            mailmap,
            merge: self.merge,
            period: d.shown_period.clone(),
            now: self.now,
            user,
            color_overrides: self.color_overrides.clone(),
        };
        if self
            .model_inputs
            .as_ref()
            .is_some_and(|last| last.same_as(&inputs))
        {
            return;
        }
        // Only a color changed, and the page is not waiting on anything else.
        let pending = self.model_requested != self.model_done;
        let recolor = self
            .model_inputs
            .as_ref()
            .is_some_and(|last| last.same_but_colors(&inputs));
        self.model_quiet = recolor && (!pending || self.model_quiet);
        self.model_gen += 1;
        let generation = self.model_gen;
        self.model_requested = generation;
        self.model_inputs = Some(inputs.clone());
        let work = cx.background_executor().spawn(async move {
            build_contributors_model(ModelInput {
                raw: inputs.raw,
                config: inputs.config,
                mailmap: (*inputs.mailmap).clone(),
                merge: inputs.merge,
                period: inputs.period,
                now: inputs.now,
                user: inputs.user,
                color_overrides: inputs.color_overrides,
            })
        });
        cx.spawn(async move |this, cx| {
            let model = work.await;
            let _ = this.update(cx, |this, cx| {
                if this.model_gen == generation {
                    this.model = Some(Arc::new(model));
                    this.model_done = generation;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    // ---- the clock ------------------------------------------------------

    /// A window left open across midnight would keep ending the period at the
    /// old "tomorrow" and miss the new day's commits.
    fn arm_midnight(&mut self, cx: &mut Context<Self>) {
        if self.midnight_for == Some(self.now) {
            return;
        }
        self.midnight_for = Some(self.now);
        let wait = until_next_day(&self.now, &(self.clock)());
        self.midnight = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(wait).await;
            let _ = this.update(cx, |this, cx| this.new_day(cx));
        }));
    }

    fn new_day(&mut self, cx: &mut Context<Self>) {
        let fresh = (self.clock)();
        if !same_day(&self.now, &fresh) {
            self.now = fresh;
        }
        // Armed again even when the day did not change.
        self.midnight_for = None;
        self.settle(cx);
    }

    /// The window came back to the front: revalidate, at most once per 15
    /// seconds, and move to a new day if one began.
    pub(crate) fn resume(&mut self, cx: &mut Context<Self>) {
        if self.cwd.is_empty() {
            return;
        }
        let fresh = (self.clock)();
        let at = fresh.timestamp_millis();
        if self
            .last_auto_reload
            .is_some_and(|last| at - last < AUTO_RELOAD_MS)
        {
            return;
        }
        self.last_auto_reload = Some(at);
        if !same_day(&self.now, &fresh) {
            self.now = fresh;
        }
        self.load(false, false, cx);
        self.settle(cx);
    }

    // ---- what the user does --------------------------------------------

    /// Hands the view its backend (the core, once it is open) and starts it.
    pub(crate) fn set_backend(
        &mut self,
        backend: Arc<dyn super::backend::Backend>,
        cx: &mut Context<Self>,
    ) {
        self.backend = Some(backend);
        if self.cwd.is_empty() || self.prefs_ready {
            self.settle(cx);
            return;
        }
        // The saved period and merge toggle, before anything is loaded.
        let now = self.now;
        let project = self.project.clone();
        self.spawn_job(
            "prefs",
            cx,
            move |backend| {
                (
                    backend.load_prefs(&now),
                    backend.load_chart_kind(),
                    backend.load_colors(&project),
                )
            },
            |this, ((pick, merge), kind, colors), cx| {
                if !this.pick_touched {
                    this.pick = pick;
                }
                if !this.merge_touched {
                    this.merge = merge;
                }
                // The first page to read it decides; the choice is app-wide.
                this.chart_prefs.update(cx, |prefs, cx| {
                    if !prefs.settled {
                        prefs.settled = true;
                        prefs.kind = kind
                            .as_deref()
                            .and_then(ChartKind::parse)
                            .unwrap_or_default();
                        cx.notify();
                    }
                });
                if !this.colors_touched {
                    this.color_overrides = colors;
                }
                this.prefs_ready = true;
                this.settle(cx);
            },
        );
    }

    fn select_pick(&mut self, next: PeriodPick, cx: &mut Context<Self>) {
        let saved = next.clone();
        self.spawn_job(
            "save-pick",
            cx,
            move |backend| backend.save_pick(&saved),
            |_, (), _| {},
        );
        self.pick_touched = true;
        self.pick = next;
        self.settle(cx);
    }

    pub(crate) fn set_kind(&mut self, kind: PeriodKind, cx: &mut Context<Self>) {
        self.select_pick(PeriodPick { kind, id: None }, cx);
    }

    /// The chart type every chart on the page shares; saved for next time.
    pub(super) fn set_chart_kind(&mut self, kind: ChartKind, cx: &mut Context<Self>) {
        let changed = self.chart_prefs.update(cx, |prefs, cx| {
            prefs.settled = true;
            let changed = prefs.kind != kind;
            if changed {
                prefs.kind = kind;
                cx.notify();
            }
            changed
        });
        if changed {
            self.spawn_job(
                "save-chart-kind",
                cx,
                move |backend| backend.save_chart_kind(kind.id()),
                |_, (), _| {},
            );
        }
        self.settle(cx);
    }

    /// A color picked from the palette for one person of this project. It is
    /// saved, and the model is rebuilt with it so every tab shows it.
    pub(super) fn set_person_color(
        &mut self,
        person_id: &str,
        hex: &'static str,
        cx: &mut Context<Self>,
    ) {
        self.colors_touched = true;
        self.color_overrides.insert(person_id.to_string(), hex);
        let (project, colors) = (self.project.clone(), self.color_overrides.clone());
        self.spawn_job(
            "save-colors",
            cx,
            move |backend| backend.save_colors(&project, &colors),
            |_, (), _| {},
        );
        self.settle(cx);
    }

    pub(crate) fn go_previous(&mut self, cx: &mut Context<Self>) {
        let period = self.derive(cx).period;
        let before = if period.kind == PeriodKind::Days7 {
            None
        } else {
            previous_period(&period)
        };
        if let Some(before) = before {
            let pick = pick_of(&before, &self.now);
            self.select_pick(pick, cx);
        }
    }

    pub(crate) fn go_next(&mut self, cx: &mut Context<Self>) {
        let period = self.derive(cx).period;
        if let Some(next) = next_period(&period, &self.now) {
            let pick = pick_of(&next, &self.now);
            self.select_pick(pick, cx);
        }
    }

    pub(crate) fn set_merge(&mut self, merge: bool, cx: &mut Context<Self>) {
        self.spawn_job(
            "save-merge",
            cx,
            move |backend| backend.save_merge(merge),
            |_, (), _| {},
        );
        self.merge_touched = true;
        self.merge = merge;
        self.settle(cx);
    }

    /// `None`, or the branch a `None` pick follows (the default branch of
    /// `origin`, else the checked-out one), follows that branch.
    pub(crate) fn set_branch(&mut self, branch: Option<String>, cx: &mut Context<Self>) {
        let is_current =
            branch.as_deref() == self.meta.as_ref().map(|m| m.branches.current.as_str());
        self.picked_branch = if is_current { None } else { branch };
        self.settle(cx);
    }

    pub(crate) fn refresh(&mut self, force: bool, cx: &mut Context<Self>) {
        // The first commit moves only with a rewrite of history; a retry reads it again.
        let d = self.derive(cx);
        if force && d.wants_all && d.meta_ready {
            self.clear_failure(&d.first_scope);
            self.fetch_first(true, cx);
        }
        self.load(force, force, cx);
        // Only the user's Refresh asks origin; it never delays the read above.
        if force {
            self.start_fetch(None, cx);
        }
        self.settle(cx);
    }

    /// Asks `origin` for the branch the page reads, or joins the fetch already
    /// running. `sync_after` (what Sync saw when pressed) decides, when it ends,
    /// whether a full sync starts. False when there is nothing to ask (a remote
    /// project, no backend yet): the caller then carries on without it.
    fn start_fetch(&mut self, sync_after: Option<SyncAfter>, cx: &mut Context<Self>) -> bool {
        if !self.can_run() {
            return false;
        }
        if let Some(run) = self.fetching.as_mut() {
            if sync_after.is_some() {
                run.sync_after = sync_after;
            }
            return true;
        }
        let first_key = self.derive(cx).first_key;
        let prior = self.origin_issues.remove(&first_key);
        self.running += 1;
        self.fetching = Some(Fetching {
            first_key,
            sync_after,
            prior,
        });
        let (cwd, branch) = (self.cwd.clone(), self.picked_branch.clone());
        self.spawn_job(
            "fetch",
            cx,
            move |backend| backend.fetch_origin(&cwd, branch.as_deref()),
            |this, answer, cx| this.finish_fetch(answer, cx),
        );
        true
    }

    fn finish_fetch(&mut self, answer: Result<FetchOutcome, String>, cx: &mut Context<Self>) {
        let Some(run) = self.fetching.take() else {
            return;
        };
        self.running = self.running.saturating_sub(1);
        // The text of a failure is for logs: git's stderr can hold credentials.
        // Only the branch this fetch was for gets a note, or loses one.
        let issue = match &answer {
            Ok(FetchOutcome::Failed(_)) | Err(_) => Some(OriginIssue::Unreachable),
            Ok(FetchOutcome::Gone) => Some(OriginIssue::Gone),
            // Nothing was learned: the earlier note stands.
            Ok(FetchOutcome::Busy) => run.prior,
            Ok(_) => None,
        };
        if let Some(issue) = issue {
            self.origin_issues.insert(run.first_key.clone(), issue);
        }
        let moved = matches!(answer, Ok(FetchOutcome::Fetched { moved: true }));
        if moved {
            // The head moved: read it again; the effects top up or repair.
            let d = self.derive(cx);
            self.load(true, true, cx);
            if d.wants_all && d.meta_ready {
                self.fetch_first(true, cx);
            }
        }
        if let Some(press) = run.sync_after {
            let d = self.derive(cx);
            // A failed fetch must not block the sync; a cancel or a move to
            // another period or branch means the press no longer applies.
            // A covered period whose branch moved is brought up to date by the
            // reload above (top-up or repair); only a top-up bumps the
            // coverage head, which a Full sync does not.
            // Covered and unmoved still syncs: for a period still open, Sync
            // extends the coverage to now and reads what is new since the last
            // sync, GitHub data included; for a closed one it finds nothing.
            let incremental = moved && press.covered;
            if !matches!(answer, Ok(FetchOutcome::Cancelled)) && d.key == press.key && !incremental
            {
                self.begin(SyncMode::Full, &d, cx);
            }
        }
        self.settle(cx);
    }

    /// Queues a sync of the selected period.
    fn begin(&mut self, mode: SyncMode, d: &Derived, cx: &mut Context<Self>) {
        let Some(loaded) = d.loaded.as_deref() else {
            return;
        };
        if !self.can_run() {
            return;
        }
        self.action_error = None;
        let window = sync_window(&d.period, &self.now);
        let job = StartJob {
            branch_name: loaded.raw.branch.clone(),
            branch: self.picked_branch.clone(),
            from: window.from,
            to: d.period.end,
            mode,
        };
        let cwd = self.cwd.clone();
        self.jobs.update(cx, |jobs, cx| {
            jobs.queue(&cwd, &job, cx);
        });
        let call = job.call();
        let jobs = self.jobs.clone();
        self.spawn_job(
            "sync-start",
            cx,
            {
                let cwd = cwd.clone();
                move |backend| backend.start(&cwd, &call)
            },
            move |this, answer, cx| {
                this.after_job_request(&jobs, answer.err().map(|e| (cwd, job, e)), cx);
                this.settle(cx);
            },
        );
        cx.notify();
    }

    /// A rejected start is the job's error; either way the core is asked what
    /// really runs, which drops a request it silently refused.
    fn after_job_request(
        &mut self,
        jobs: &Entity<SyncJobs>,
        failure: Option<(String, StartJob, String)>,
        cx: &mut Context<Self>,
    ) {
        let backend = self.backend.clone();
        jobs.update(cx, |jobs, cx| {
            if let Some((cwd, job, error)) = failure {
                jobs.queue_failed(&cwd, &job, &error, cx);
            }
            if let Some(backend) = backend {
                jobs.reconcile_with(move || backend.status(), cx);
            }
        });
    }

    /// A job resolves its head when it starts, so origin is asked first.
    pub(crate) fn start_sync(&mut self, cx: &mut Context<Self>) {
        let d = self.derive(cx);
        let press = SyncAfter {
            key: d.key.clone(),
            covered: d.loaded.as_deref().is_some_and(|l| l.covered),
        };
        if self.start_fetch(Some(press), cx) {
            return;
        }
        self.begin(SyncMode::Full, &d, cx);
    }

    pub(crate) fn retry_github(&mut self, cx: &mut Context<Self>) {
        let d = self.derive(cx);
        self.begin(SyncMode::GithubRetry, &d, cx);
    }

    /// Stops the jobs of the project's repository, then checks what still runs.
    pub(crate) fn cancel_sync(&mut self, cx: &mut Context<Self>) {
        if self.cwd.is_empty() {
            return;
        }
        // A fetch that never sees this cancel must not start a job afterwards.
        if let Some(run) = self.fetching.as_mut() {
            run.sync_after = None;
        }
        let cwd = self.cwd.clone();
        let jobs = self.jobs.clone();
        self.spawn_job(
            "sync-cancel",
            cx,
            move |backend| backend.cancel(&cwd),
            move |this, answer, cx| {
                if let Err(error) = &answer {
                    this.action_error = Some(error.clone());
                }
                this.after_job_request(&jobs, None, cx);
                this.settle(cx);
            },
        );
    }

    /// Drops a failed job's entry once the user has seen it.
    pub(crate) fn dismiss_job_error(&mut self, cx: &mut Context<Self>) {
        let failed = self
            .derive(cx)
            .this_job
            .filter(|job| job.state == monocode_core::contributors::jobs::JobState::Error);
        if let Some(job) = failed {
            self.jobs.update(cx, |jobs, cx| {
                jobs.dismiss(&job, cx);
            });
        }
    }

    pub(crate) fn clear_synced_data(&mut self, cx: &mut Context<Self>) {
        if self.cwd.is_empty() {
            return;
        }
        self.action_error = None;
        let cwd = self.cwd.clone();
        let own = cwd.clone();
        self.spawn_job(
            "clear",
            cx,
            move |backend| backend.clear(&cwd),
            move |this, answer, cx| {
                if let Err(error) = answer {
                    this.action_error = Some(error);
                    this.settle(cx);
                    return;
                }
                // Everything this project had in the page is gone with the store.
                let mine = format!("{own}\u{0}");
                this.loads.remove_prefix(&mine);
                this.topped_up.retain(|k| !k.starts_with(&mine));
                this.jobs.update(cx, |jobs, cx| {
                    jobs.forget(&own, cx);
                });
                this.last_shown = None;
                this.load(true, true, cx);
                this.settle(cx);
            },
        );
        cx.notify();
    }

    /// A job of this project ended: whatever it stored is read again.
    pub(super) fn job_ended(&mut self, event: &JobsEvent, cx: &mut Context<Self>) {
        let JobsEvent::Ended(event) = event else {
            return;
        };
        if event.root != self.cwd {
            return;
        }
        // What a job stored is not in the cached loads: one that said "not
        // synced" would flash "No data" when shown again.
        let mine = format!("{}\u{0}", self.cwd);
        self.loads.remove_prefix(&mine);
        self.forget_activity_loads();
        self.load_inflight.clear();
        self.load(false, true, cx);
        self.settle(cx);
    }

    /// Saves `next` as the project's config. The page shows it at once; a
    /// failed write puts the confirmed one back. Refused while the file on
    /// disk cannot be read: writing now would replace it.
    #[allow(dead_code)] // The Leaderboard and Identities tabs save through it (6.9, 6.10).
    pub(crate) fn save_config(
        &mut self,
        next: ContributorsConfig,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        let Some(meta) = self.meta.as_mut() else {
            return Ok(());
        };
        if let Some(error) = &meta.config_error {
            return Err(format!(
                "{CONFIG_PATH} could not be read ({error}). Fix the file in your repo, then reload."
            ));
        }
        let next = Arc::new(next);
        self.saves.latest = Some(next.clone());
        self.saves.pending += 1;
        self.saves.epoch += 1;
        meta.config = next.clone();
        self.save_error = None;
        self.saves.queue.push_back(next);
        self.pump_writes(cx);
        self.settle(cx);
        Ok(())
    }

    fn pump_writes(&mut self, cx: &mut Context<Self>) {
        if self.saves.writing {
            return;
        }
        let Some(next) = self.saves.queue.pop_front() else {
            return;
        };
        self.saves.writing = true;
        let (cwd, text) = (self.cwd.clone(), serialize_config(&next));
        self.spawn_job(
            "write-config",
            cx,
            move |backend| backend.write_config(&cwd, &text),
            move |this, answer, cx| {
                this.saves.writing = false;
                let latest = this
                    .saves
                    .latest
                    .as_ref()
                    .is_some_and(|latest| Arc::ptr_eq(latest, &next));
                match answer {
                    Ok(()) => {
                        if let Some(meta) = this.meta.as_mut() {
                            meta.confirmed = next.clone();
                        }
                        if latest {
                            this.save_error = None;
                        }
                    }
                    Err(error) => {
                        // A later save already carries this change; only the newest reverts.
                        if latest && let Some(meta) = this.meta.as_mut() {
                            meta.config = meta.confirmed.clone();
                        }
                        this.save_error = Some(error);
                    }
                }
                this.saves.pending = this.saves.pending.saturating_sub(1);
                this.saves.epoch += 1;
                this.pump_writes(cx);
                this.settle(cx);
            },
        );
    }
}

/// Reads the project's files. The config must read: saving over a file that
/// could not be read would destroy it. The rest only enriches the page.
fn read_meta(backend: &dyn super::backend::Backend, cwd: &str) -> Result<MetaRead, String> {
    let text = backend.read_config(cwd)?;
    let mailmap = backend.read_mailmap(cwd).ok().flatten();
    let branches = backend.branches(cwd).unwrap_or(ContributorsBranches {
        current: String::new(),
        branches: Vec::new(),
    });
    let user = backend.git_user(cwd).ok();
    let result = parse_config_result(text.as_deref());
    Ok(MetaRead {
        config: result.config,
        error: result.error,
        mailmap: parse_mailmap(mailmap.as_deref().unwrap_or("")),
        branches,
        user,
    })
}
