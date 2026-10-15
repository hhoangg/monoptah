//! Sync engine of the Contributors page: what a sync would cost, the batched
//! ingestion of commits into the store, and the whole sync (commits, PRs from
//! git, GitHub per month, survival, coverage) in its three modes.
//!
//! Everything here runs only because the user pressed Sync (or because the
//! branch head of a synced period moved). Git and GitHub work happens with no
//! database lock held; the write lock is taken for one batch at a time, in one
//! transaction, so a cancelled or crashed job leaves whole batches behind and
//! the next run continues with what is missing.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::time::Duration;

use serde::Serialize;

use super::store::{self, CoverageRow, StoredCommit};
use super::{
    gh, pr_heads, split_records, CancelCheck, Derived, Known, Limits, PatchPipe, RawCommit, RawPr,
    RawReview, RawSurvival, Window,
};
use crate::session_store::SessionStore;

/// Commits read and written per transaction.
pub(super) const COMMIT_BATCH: usize = 500;

/// Hard stop for one local git call over a batch.
const BATCH_TIMEOUT: Duration = Duration::from_secs(300);
/// Hard stop for the GitHub PR count.
const SEARCH_TIMEOUT: Duration = Duration::from_secs(30);
/// Wall clock for the whole GitHub part of an estimate.
const ESTIMATE_BUDGET: Duration = Duration::from_secs(60);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SyncStep {
    Counting,
    Commits,
    PullRequests,
    Github,
    Survival,
}

/// How a running sync tells the outside world what it does, and how the
/// outside world stops it. `cancelled` is also asked while git runs, so it
/// must be cheap.
pub(super) trait Reporter: Sync {
    fn progress(&self, step: SyncStep, done: u64, total: u64);
    fn cancelled(&self) -> bool;
}

// ---------------------------------------------------------------------------
// estimate
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Estimate {
    /// Non-merge commits of the window.
    pub commits: u64,
    /// How many of them are already stored.
    pub stored_commits: u64,
    /// First-parent entries of the window: what the PR derivation walks.
    pub first_parent: u64,
    /// Merged and closed PRs of the window on GitHub; `None` when gh cannot say.
    pub prs: Option<u64>,
}

fn rev_list_count(root: &Path, args: &[&str]) -> Result<u64, String> {
    let out = super::git(root, args)?;
    out.trim()
        .parse()
        .map_err(|_| "git returned an unexpected count".to_string())
}

/// `total_count` of a GitHub search reply.
fn parse_total_count(text: &str) -> Option<u64> {
    serde_json::from_str::<serde_json::Value>(text)
        .ok()?
        .get("total_count")?
        .as_u64()
}

/// PRs of `slug` closed in `from..=to` (merged ones are closed too), from the
/// search API: one call, no 1000 row limit. `None` when the call fails, the
/// budget is spent or the reply is unreadable.
fn count_closed_prs(
    github: &dyn gh::GhRunner,
    root: &Path,
    slug: &str,
    from: i64,
    to: i64,
    budget: &gh::Budget,
) -> Option<u64> {
    let query = format!(
        "q=repo:{slug} is:pr is:closed closed:{}..{}",
        super::epoch_to_iso(from),
        super::epoch_to_iso(to)
    );
    let args = [
        "api",
        "-X",
        "GET",
        "search/issues",
        "-f",
        query.as_str(),
        "-f",
        "per_page=1",
    ];
    let timeout = budget.left()?.min(SEARCH_TIMEOUT);
    github
        .run(root, &args, timeout)
        .ok()
        .and_then(|text| parse_total_count(&text))
}

/// Counts what syncing `from..=to` of `head` involves. Reads hashes and one
/// GitHub count only, never a diff. `repo` is `None` for a repository the store
/// does not know yet. The database is touched for one short read; git and gh
/// run with no lock held.
pub(super) fn estimate(
    root: &Path,
    head: &str,
    from: i64,
    to: i64,
    store: &SessionStore,
    repo: Option<i64>,
    github: &dyn gh::GhRunner,
) -> Result<Estimate, String> {
    let (commits, stored_commits) = match repo {
        Some(repo) => {
            let hashes = store::listed_hashes(root, head, from, to, &["--no-merges"])?;
            let stored = store.with_read_conn(|conn| store::stored_hashes(conn, repo, &hashes))?;
            (hashes.len() as u64, stored.len() as u64)
        }
        None => {
            let since = format!("--since=@{from}");
            let until = format!("--until=@{to}");
            let args = [
                "rev-list",
                "--count",
                "--no-merges",
                since.as_str(),
                until.as_str(),
                head,
                "--",
            ];
            (rev_list_count(root, &args)?, 0)
        }
    };
    let since = format!("--since=@{from}");
    let until = format!("--until=@{to}");
    let first_parent = rev_list_count(
        root,
        &[
            "rev-list",
            "--count",
            "--first-parent",
            since.as_str(),
            until.as_str(),
            head,
            "--",
        ],
    )?;

    let budget = gh::Budget::new(ESTIMATE_BUDGET);
    let prs = gh::gh_status(github, root, &budget)
        .ok()
        .and_then(|slug| count_closed_prs(github, root, &slug, from, to, &budget));

    Ok(Estimate {
        commits,
        stored_commits,
        first_parent,
        prs,
    })
}

// ---------------------------------------------------------------------------
// commit ingestion
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct IngestOutcome {
    /// Commits written by this run.
    pub written: u64,
    /// Written without files because they touched more than the per-commit cap.
    pub oversized: u64,
    /// Stopped because the reporter asked to: between batches, or by killing
    /// the git process of the batch that was running (nothing of it is kept).
    pub cancelled: bool,
}

/// Stores the commits of `hashes` that the store does not have yet.
pub(super) fn ingest_commits(
    store: &SessionStore,
    repo: i64,
    root: &Path,
    hashes: &[String],
    reporter: &dyn Reporter,
) -> Result<IngestOutcome, String> {
    ingest_with(
        store,
        repo,
        root,
        hashes,
        reporter,
        COMMIT_BATCH,
        &Limits::DEFAULT,
    )
}

fn ingest_with(
    store: &SessionStore,
    repo: i64,
    root: &Path,
    hashes: &[String],
    reporter: &dyn Reporter,
    batch_size: usize,
    limits: &Limits,
) -> Result<IngestOutcome, String> {
    let mut seen = HashSet::new();
    let unique: Vec<String> = hashes
        .iter()
        .filter(|h| seen.insert(h.as_str()))
        .cloned()
        .collect();
    let stored = store.with_read_conn(|conn| store::stored_hashes(conn, repo, &unique))?;
    let pending: Vec<String> = unique.into_iter().filter(|h| !stored.contains(h)).collect();
    let total = pending.len() as u64;

    let cancel = || reporter.cancelled();
    let mut outcome = IngestOutcome::default();
    let mut done = 0u64;
    for batch in pending.chunks(batch_size.max(1)) {
        if reporter.cancelled() {
            outcome.cancelled = true;
            return Ok(outcome);
        }
        let Some(commits) = read_batch(root, batch, limits, &cancel)? else {
            outcome.cancelled = true;
            return Ok(outcome);
        };
        let oversized = commits.iter().filter(|c| c.oversized).count() as u64;
        {
            // The lock covers this write only, never the git work above.
            let mut conn = store.lock_conn()?;
            let tx = conn.transaction().map_err(|e| e.to_string())?;
            store::insert_commits(&tx, repo, &commits)?;
            tx.commit().map_err(|e| e.to_string())?;
        }
        outcome.written += commits.len() as u64;
        outcome.oversized += oversized;
        done += batch.len() as u64;
        reporter.progress(SyncStep::Commits, done, total);
    }
    Ok(outcome)
}

/// Reads one batch from git: files and identities first, then patch ids for
/// the commits that kept their files (a commit over the file cap is stored
/// without a patch id and never costs a pass over its diff). `None` when
/// `cancel` killed the git process.
fn read_batch(
    root: &Path,
    batch: &[String],
    limits: &Limits,
    cancel: CancelCheck,
) -> Result<Option<Vec<StoredCommit>>, String> {
    let input = batch.iter().map(|h| format!("{h}\n")).collect::<String>();
    let ran = pr_heads::run_git(
        root,
        &[
            "log",
            "--no-walk",
            "--stdin",
            "--no-show-signature",
            "--numstat",
            "-z",
            "-w",
            "-M",
            "--no-color",
            super::LOG_FORMAT,
        ],
        Some(input),
        BATCH_TIMEOUT,
        &[],
        Some(cancel),
    );
    let output = match ran {
        Err(error) if error == pr_heads::CANCELLED => return Ok(None),
        other => other?,
    };
    let parsed: Vec<_> = split_records(&output)
        .iter()
        .filter_map(|record| super::parse_record(record, limits.max_files_per_commit))
        .collect();

    let feed: Vec<String> = parsed
        .iter()
        .filter(|(_, capped)| !capped)
        .map(|(commit, _)| commit.hash.clone())
        .collect();
    let mut patches = HashMap::new();
    if !patch_ids_of(root, &feed, limits, cancel, &mut patches)? {
        return Ok(None);
    }

    Ok(Some(
        parsed
            .into_iter()
            .map(|(parsed, capped)| StoredCommit {
                patch_id: patches.remove(&parsed.hash),
                reverts: super::reverted_commit(&parsed.subject, &parsed.body),
                oversized: capped,
                commit: RawCommit {
                    pr_id: None,
                    hash: parsed.hash,
                    subject: parsed.subject,
                    author: parsed.author,
                    committer: parsed.committer,
                    authored_at: parsed.authored_at,
                    committed_at: parsed.committed_at,
                    co_authors: parsed.co_authors,
                    files: if capped { Vec::new() } else { parsed.files },
                },
            })
            .collect(),
    ))
}

/// One pass of the patch-id pipe over `hashes`.
fn run_patch_pipe(
    root: &Path,
    hashes: &[String],
    max_bytes: usize,
    cancel: Option<CancelCheck>,
) -> Result<PatchPipe, String> {
    let mut log = super::git_command(root);
    log.args([
        "log",
        "-p",
        "--no-walk",
        "--stdin",
        "--no-color",
        "--no-ext-diff",
        "--no-textconv",
        "--no-show-signature",
    ]);
    super::pipe_patch_ids(root, log, Some(hashes), max_bytes, cancel)
}

/// Patch ids of `hashes` into `out`. A set whose patch text exceeds the byte
/// cap is split in halves; a single commit that is still too big gets none.
/// `Ok(false)` when `cancel` killed the pipe.
fn patch_ids_of(
    root: &Path,
    hashes: &[String],
    limits: &Limits,
    cancel: CancelCheck,
    out: &mut HashMap<String, String>,
) -> Result<bool, String> {
    if hashes.is_empty() {
        return Ok(true);
    }
    match run_patch_pipe(root, hashes, limits.max_patch_bytes, Some(cancel))? {
        PatchPipe::Ids(pairs) => {
            out.extend(pairs.into_iter().map(|(patch, commit)| (commit, patch)));
            Ok(true)
        }
        PatchPipe::Cancelled => Ok(false),
        PatchPipe::TooBig if hashes.len() == 1 => Ok(true),
        PatchPipe::TooBig => {
            let (left, right) = hashes.split_at(hashes.len() / 2);
            Ok(patch_ids_of(root, left, limits, cancel, out)?
                && patch_ids_of(root, right, limits, cancel, out)?)
        }
    }
}

// ---------------------------------------------------------------------------
// progress
// ---------------------------------------------------------------------------

/// What each step counts for in the one percentage of a sync, in the order the
/// steps run. They add up to 100.
const WEIGHTS: [(SyncStep, u64); 5] = [
    (SyncStep::Counting, 2),
    (SyncStep::Commits, 50),
    (SyncStep::PullRequests, 13),
    (SyncStep::Github, 25),
    (SyncStep::Survival, 10),
];

/// The percentage of the whole sync once `done` of `total` of `step` is done.
/// A step with nothing to do (`total` 0) counts as done as soon as it is
/// reported. Steps run in order and `done` only grows, so over a run this
/// never goes down, and it reaches 100 with the end of `Survival`.
pub(super) fn percent(step: SyncStep, done: u64, total: u64) -> u8 {
    let mut before = 0;
    for (known, weight) in WEIGHTS {
        if known == step {
            let part = (weight * done.min(total))
                .checked_div(total)
                .unwrap_or(weight);
            return (before + part).min(100) as u8;
        }
        before += weight;
    }
    100
}

/// Reports the progress of one call that counts from 0 as a stretch of a
/// larger step: `base` is what earlier calls of the step did, `total` the size
/// of the whole step.
struct Stretch<'a> {
    inner: &'a dyn Reporter,
    step: SyncStep,
    base: u64,
    total: u64,
}

impl Reporter for Stretch<'_> {
    fn progress(&self, _: SyncStep, done: u64, _: u64) {
        self.inner
            .progress(self.step, (self.base + done).min(self.total), self.total);
    }

    fn cancelled(&self) -> bool {
        self.inner.cancelled()
    }
}

// ---------------------------------------------------------------------------
// calendar months
// ---------------------------------------------------------------------------

/// Seconds the local time zone is ahead of UTC at `epoch`. Only unix can say;
/// elsewhere months follow UTC, which only moves where one chunk ends.
#[cfg(unix)]
fn local_offset(epoch: i64) -> i64 {
    #[cfg(test)]
    if let Some(fixed) = TEST_OFFSET.with(std::cell::Cell::get) {
        return fixed;
    }
    let at = epoch as libc::time_t;
    // SAFETY: `localtime_r` writes only into the `tm` it is given.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    if unsafe { libc::localtime_r(&at, &mut tm) }.is_null() {
        return 0;
    }
    tm.tm_gmtoff as i64
}

#[cfg(not(unix))]
fn local_offset(_: i64) -> i64 {
    #[cfg(test)]
    if let Some(fixed) = TEST_OFFSET.with(std::cell::Cell::get) {
        return fixed;
    }
    0
}

#[cfg(test)]
thread_local! {
    /// A test's own time zone, so that month boundaries do not depend on the
    /// machine it runs on.
    static TEST_OFFSET: std::cell::Cell<Option<i64>> = const { std::cell::Cell::new(None) };
}

/// The first instant of the local calendar month after the one `epoch` is in.
fn next_month_start(epoch: i64, offset: &dyn Fn(i64) -> i64) -> i64 {
    let local = epoch + offset(epoch);
    let (year, month, _) = super::civil_from_days(local.div_euclid(86_400));
    let (year, month) = if month == 12 {
        (year + 1, 1)
    } else {
        (year, month + 1)
    };
    let midnight = super::days_from_civil(year, month, 1) * 86_400;
    // The offset in force at that midnight, found in two steps because it is
    // asked at an instant that is only close.
    let guess = midnight - offset(midnight);
    (midnight - offset(guess)).max(epoch + 1)
}

fn month_chunks_in(from: i64, to: i64, offset: &dyn Fn(i64) -> i64) -> Vec<(i64, i64)> {
    let mut chunks = Vec::new();
    let mut cursor = from;
    while cursor < to {
        let end = next_month_start(cursor, offset).min(to);
        chunks.push((cursor, end));
        cursor = end;
    }
    chunks
}

/// The local calendar months of `from..to` (`to` excluded) as `(start, end)`
/// pairs, `end` excluded, clipped to the range. Together they cover it once.
pub(super) fn month_chunks(from: i64, to: i64) -> Vec<(i64, i64)> {
    month_chunks_in(from, to, &local_offset)
}

// ---------------------------------------------------------------------------
// the sync
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SyncMode {
    /// Everything the coverage does not have yet.
    Full,
    /// The branch moved since the period was synced: only what is new.
    TopUp { old_head: String },
    /// Only the GitHub step, for the covered intervals that lack GitHub data.
    GithubRetry,
}

pub(super) struct SyncRequest<'a> {
    pub store: &'a SessionStore,
    pub root: &'a Path,
    pub branch: &'a str,
    /// The commit `branch` points to.
    pub head: &'a str,
    /// The period, both edges inclusive epoch seconds.
    pub from: i64,
    pub to: i64,
    pub mode: SyncMode,
    pub gh: &'a dyn gh::GhRunner,
    pub commit_cache: &'a gh::CommitCache,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct SyncOutcome {
    /// Stopped on request. Whatever was stored stays; intervals that had not
    /// finished got no coverage.
    pub cancelled: bool,
    /// `complete | partial | unavailable`: the worst of the intervals.
    pub gh_state: String,
    pub gh_note: Option<String>,
}

/// What a month whose GitHub part failed leaves in the store.
#[derive(Clone, Copy, PartialEq, Eq)]
enum OnGhFailure {
    /// The PRs git derived.
    KeepGit,
    /// Whatever the store has: a retry must not make things worse.
    KeepStored,
}

/// One stretch of time the sync works on and then covers.
struct Task {
    /// Both edges inclusive.
    from: i64,
    to: i64,
    /// Commits to have stored before PRs are derived.
    commits: Vec<String>,
    /// Non-merge commits of `dedupe_from..=to`, as `load` lists them: what
    /// repeated patches are looked for among.
    dedupe: Vec<String>,
    survival: bool,
    on_gh_failure: OnGhFailure,
    /// The coverage row to write: its head and time.
    head: String,
    synced_at: i64,
    /// Rows to say the new head for first, when the sync did not touch them.
    bump: Vec<CoverageRow>,
}

struct Counted {
    task: Task,
    /// Newest first, both edges inclusive.
    chunks: Vec<(i64, i64)>,
    /// First-parent entries of each chunk.
    entries: Vec<u64>,
    pending: u64,
}

/// A month with the PRs derived for it, on their way to the store.
struct Month {
    chunk: (i64, i64),
    derived: Derived,
    /// Every review of the month's GitHub PRs, whenever it was submitted.
    reviews: Vec<RawReview>,
}

/// How the GitHub step went for one month.
enum GhMonth {
    Complete,
    /// Loaded, but not all of it; keeps the note.
    Partial(String),
    /// Nothing from GitHub; keeps the note.
    Failed(String),
}

#[derive(Default)]
struct GhTally {
    unavailable: Option<String>,
    partial: Option<String>,
}

impl GhTally {
    fn state(&self) -> (&'static str, Option<String>) {
        if let Some(note) = &self.unavailable {
            ("unavailable", Some(note.clone()))
        } else if let Some(note) = &self.partial {
            ("partial", Some(note.clone()))
        } else {
            ("complete", None)
        }
    }
}

fn gh_rank(state: &str) -> u8 {
    match state {
        "unavailable" => 2,
        "partial" => 1,
        _ => 0,
    }
}

/// One git call of the plan that a cancel check can kill.
fn git_cancellable(root: &Path, args: &[&str], cancel: CancelCheck) -> Result<String, String> {
    pr_heads::run_git(root, args, None, BATCH_TIMEOUT, &[], Some(cancel))
}

fn is_cancel(error: &str) -> bool {
    error == pr_heads::CANCELLED
}

fn ancestor_of(root: &Path, old: &str, head: &str, cancel: CancelCheck) -> Result<bool, String> {
    if !gh::is_object_id(old) {
        return Ok(false);
    }
    match git_cancellable(root, &["merge-base", "--is-ancestor", old, head], cancel) {
        Ok(_) => Ok(true),
        Err(error) if is_cancel(&error) => Err(error),
        Err(_) => Ok(false),
    }
}

/// Is `head` a commit of this repository?
fn commit_exists(root: &Path, head: &str, cancel: CancelCheck) -> Result<bool, String> {
    if !gh::is_object_id(head) {
        return Ok(false);
    }
    let spec = format!("{head}^{{commit}}");
    match git_cancellable(root, &["cat-file", "-e", spec.as_str()], cancel) {
        Ok(_) => Ok(true),
        Err(error) if is_cancel(&error) => Err(error),
        Err(_) => Ok(false),
    }
}

/// Committer time of each of `hashes`.
fn commit_epochs(
    root: &Path,
    hashes: &[String],
    cancel: CancelCheck,
) -> Result<Vec<(String, i64)>, String> {
    if hashes.is_empty() {
        return Ok(Vec::new());
    }
    let input = hashes.iter().map(|h| format!("{h}\n")).collect::<String>();
    let out = pr_heads::run_git(
        root,
        &[
            "log",
            "--no-walk=unsorted",
            "--stdin",
            "--no-show-signature",
            "--format=%H %ct",
        ],
        Some(input),
        BATCH_TIMEOUT,
        &[],
        Some(cancel),
    )?;
    Ok(out
        .lines()
        .filter_map(|line| {
            let (hash, epoch) = line.split_once(' ')?;
            Some((hash.to_string(), epoch.trim().parse().ok()?))
        })
        .collect())
}

fn clip(row: &CoverageRow, from: i64, to: i64) -> (i64, i64) {
    (row.from_epoch.max(from), row.to_epoch.min(to))
}

/// What is left of `from..=to` once the inclusive `gaps` (sorted, apart) are
/// taken out.
fn outside(from: i64, to: i64, gaps: &[(i64, i64)]) -> Vec<(i64, i64)> {
    let mut pieces = Vec::new();
    let mut cursor = from;
    for &(start, end) in gaps {
        if start > cursor {
            pieces.push((cursor, start - 1));
        }
        cursor = cursor.max(end.saturating_add(1));
    }
    if cursor <= to {
        pieces.push((cursor, to));
    }
    pieces
}

/// A stretch to work on again from scratch: its commits are made sure of and
/// its PRs and GitHub data derived once more. No survival, and when GitHub
/// fails the PRs git derived are kept.
struct Redo<'a> {
    head: &'a str,
    now: i64,
    from: i64,
    to: i64,
    commits: Vec<String>,
    dedupe: Vec<String>,
    bump: Vec<CoverageRow>,
}

impl Redo<'_> {
    fn task(self) -> Task {
        Task {
            from: self.from,
            to: self.to,
            commits: self.commits,
            dedupe: self.dedupe,
            survival: false,
            on_gh_failure: OnGhFailure::KeepGit,
            head: self.head.to_string(),
            synced_at: self.now,
            bump: self.bump,
        }
    }
}

/// The stretches of time this request has to work on. Nothing reaches past
/// `now`: time that has not happened is not covered.
fn plan(
    req: &SyncRequest,
    repo: i64,
    rows: &[CoverageRow],
    now: i64,
    cancel: CancelCheck,
) -> Result<Vec<Task>, String> {
    let list = |head: &str, from: i64, to: i64, flags: &[&str]| {
        store::listed_hashes_cancellable(req.root, head, from, to, flags, Some(cancel))
    };
    let in_window = ["--no-merges", "--date-order"];
    let hi = now.min(req.to);
    match &req.mode {
        SyncMode::Full => {
            if hi < req.from {
                return Ok(Vec::new());
            }
            let gaps = store::missing_intervals(rows, req.from, hi);
            let mut tasks = Vec::new();
            for &(from, to) in &gaps {
                let dedupe = list(req.head, from, to, &in_window)?;
                tasks.push(Task {
                    from,
                    to,
                    commits: dedupe.clone(),
                    dedupe,
                    survival: true,
                    on_gh_failure: OnGhFailure::KeepGit,
                    head: req.head.to_string(),
                    synced_at: now,
                    bump: Vec::new(),
                });
            }

            // Commits of the period that the store lacks although the
            // coverage says their time was synced: a branch rewritten since,
            // or a merge that brought in commits dated before the last sync.
            let window = list(req.head, req.from, hi, &in_window)?;
            let stored = req
                .store
                .with_read_conn(|conn| store::stored_hashes(conn, repo, &window))?;
            let lacking: Vec<String> = window
                .iter()
                .filter(|h| !stored.contains(*h))
                .cloned()
                .collect();
            let epochs = commit_epochs(req.root, &lacking, cancel)?;
            for (from, to) in outside(req.from, hi, &gaps) {
                let inside: Vec<&(String, i64)> = epochs
                    .iter()
                    .filter(|(_, at)| *at >= from && *at <= to)
                    .collect();
                // Whole months from the first to the last of them.
                let months = month_chunks(from, to.saturating_add(1));
                let touched: Vec<&(i64, i64)> = months
                    .iter()
                    .filter(|(start, end)| inside.iter().any(|(_, at)| at >= start && at < end))
                    .collect();
                let (Some(first), Some(last)) = (touched.first(), touched.last()) else {
                    continue;
                };
                let (lo, end) = (first.0, last.1 - 1);
                tasks.push(
                    Redo {
                        head: req.head,
                        now,
                        from: lo,
                        to: end,
                        commits: inside
                            .iter()
                            .filter(|(_, at)| *at >= lo && *at <= end)
                            .map(|(h, _)| h.clone())
                            .collect(),
                        dedupe: window.clone(),
                        bump: Vec::new(),
                    }
                    .task(),
                );
            }
            Ok(tasks)
        }
        SyncMode::GithubRetry => {
            let mut tasks = Vec::new();
            for row in rows.iter().filter(|r| {
                r.gh_state != "complete" && r.from_epoch <= req.to && r.to_epoch >= req.from
            }) {
                let (from, to) = clip(row, req.from, req.to);
                // The row says which head it was synced at; the PRs are
                // derived at that head so the row stays true. A head that is
                // gone is replaced by the current one.
                let head = if commit_exists(req.root, &row.head, cancel)? {
                    row.head.as_str()
                } else {
                    req.head
                };
                let dedupe = list(head, from, to, &in_window)?;
                tasks.push(Task {
                    from,
                    to,
                    commits: dedupe.clone(),
                    dedupe,
                    survival: false,
                    on_gh_failure: OnGhFailure::KeepStored,
                    head: head.to_string(),
                    synced_at: row.synced_at,
                    bump: Vec::new(),
                });
            }
            Ok(tasks)
        }
        SyncMode::TopUp { old_head } => {
            let overlapping: Vec<&CoverageRow> = rows
                .iter()
                .filter(|r| r.from_epoch <= req.to && r.to_epoch >= req.from)
                .collect();
            // PRs merged since the oldest sync of the period; a period that
            // ended before that is only checked at its end.
            let hi = hi.max(req.from);
            let oldest = overlapping.iter().map(|r| r.synced_at).min();
            let window = list(req.head, req.from, hi, &in_window)?;
            let (lo, commits) = if ancestor_of(req.root, old_head, req.head, cancel)? {
                let lo = oldest.map_or(req.from, |at| at.min(hi).max(req.from));
                let range = format!("{old_head}..{}", req.head);
                let mut hashes: Vec<String> = git_cancellable(
                    req.root,
                    &["rev-list", "--no-merges", range.as_str(), "--"],
                    cancel,
                )?
                .lines()
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .map(str::to_string)
                .collect();
                hashes.extend(list(req.head, lo, hi, &["--no-merges"])?);
                (lo, hashes)
            } else {
                // A rewritten branch can differ anywhere in the period: take
                // whatever commits of it the store lacks and derive its PRs again.
                (req.from, window.clone())
            };
            let bump = overlapping
                .iter()
                .filter(|r| r.head != req.head)
                .map(|r| CoverageRow {
                    head: req.head.to_string(),
                    ..(*r).clone()
                })
                .collect();
            Ok(vec![Redo {
                head: req.head,
                now,
                from: lo,
                to: hi,
                commits,
                dedupe: window,
                bump,
            }
            .task()])
        }
    }
}

/// Runs one sync: the commits, PRs, GitHub data and survival the request's
/// mode calls for, stored a batch at a time with no database lock held across
/// git or network work. An interval gets its coverage row only when all of its
/// steps finished, so a cancelled run leaves stored batches but nothing that
/// says the period is complete.
pub(super) fn run_sync(req: &SyncRequest, reporter: &dyn Reporter) -> Result<SyncOutcome, String> {
    match run_steps(req, reporter) {
        Err(error) if is_cancel(&error) => Ok(cancelled("complete", None)),
        other => other,
    }
}

const CANCELLED_NOTE: &str = "The sync was cancelled before it finished.";

/// What a stopped run reports: never `complete`, whatever it had learned about
/// GitHub by then. Callers still read `cancelled`.
fn cancelled(state: &str, note: Option<String>) -> SyncOutcome {
    let (state, note) = if gh_rank(state) == 0 {
        ("partial", note.or_else(|| Some(CANCELLED_NOTE.to_string())))
    } else {
        (state, note)
    };
    SyncOutcome {
        cancelled: true,
        gh_state: state.to_string(),
        gh_note: note,
    }
}

const SURVIVAL_CUT_NOTE: &str = "Survival sampling was cut short.";

/// The note of a coverage row: GitHub's, and survival's when it was cut short.
fn compose_note(github: Option<String>, survival_cut: bool) -> Option<String> {
    match (github, survival_cut) {
        (note, false) => note,
        (None, true) => Some(SURVIVAL_CUT_NOTE.to_string()),
        (Some(note), true) => Some(format!("{note} {SURVIVAL_CUT_NOTE}")),
    }
}

fn run_steps(req: &SyncRequest, reporter: &dyn Reporter) -> Result<SyncOutcome, String> {
    let stop = || reporter.cancelled();
    let dir = store::git_common_dir(req.root)?;
    let repo = {
        let conn = req.store.lock_conn()?;
        store::repo_id(&conn, &dir, None)?
    };
    let rows = req
        .store
        .with_read_conn(|conn| store::coverage(conn, repo, req.branch))?;
    let now = super::now_epoch();
    let run = Run {
        req,
        repo,
        cancel: &stop,
    };

    // Counting: exact totals for every later step.
    reporter.progress(SyncStep::Counting, 0, 1);
    let mut counted = Vec::new();
    for task in plan(req, repo, &rows, now, &stop)? {
        let chunks: Vec<(i64, i64)> = month_chunks(task.from, task.to.saturating_add(1))
            .into_iter()
            .rev()
            .map(|(start, end)| (start, end - 1))
            .collect();
        let entries = chunks
            .iter()
            .map(|&(start, end)| run.first_parent_entries(&task.head, start, end))
            .collect::<Result<Vec<_>, _>>()?;
        let stored = req
            .store
            .with_read_conn(|conn| store::stored_hashes(conn, repo, &task.commits))?;
        let unique: HashSet<&String> = task.commits.iter().collect();
        let pending = unique.iter().filter(|h| !stored.contains(**h)).count() as u64;
        counted.push(Counted {
            task,
            chunks,
            entries,
            pending,
        });
    }
    reporter.progress(SyncStep::Counting, 1, 1);

    // Commits.
    let commit_total: u64 = counted.iter().map(|c| c.pending).sum();
    reporter.progress(SyncStep::Commits, 0, commit_total);
    let mut base = 0;
    for item in &counted {
        let stretch = Stretch {
            inner: reporter,
            step: SyncStep::Commits,
            base,
            total: commit_total,
        };
        let ingested = ingest_commits(req.store, repo, req.root, &item.task.commits, &stretch)?;
        if ingested.cancelled {
            return Ok(cancelled("complete", None));
        }
        base += item.pending;
    }
    reporter.progress(SyncStep::Commits, commit_total, commit_total);

    // PRs from git, a month at a time, newest first like the first-parent log
    // the walk is made of.
    let entry_total: u64 = counted.iter().flat_map(|c| &c.entries).sum();
    reporter.progress(SyncStep::PullRequests, 0, entry_total);
    let mut entries_done = 0;
    let mut staged: Vec<Vec<Month>> = Vec::new();
    for item in &counted {
        let dropped = req
            .store
            .with_read_conn(|conn| store::dropped_duplicates(conn, repo, &item.task.dedupe))?;
        let mut used = HashSet::new();
        let mut months = Vec::new();
        for (chunk, entries) in item.chunks.iter().zip(&item.entries) {
            if reporter.cancelled() {
                return Ok(cancelled("complete", None));
            }
            let derived = run.derive_month(&item.task.head, *chunk, &dropped, &used)?;
            used.extend(derived.drafts.iter().map(|d| d.pr.id.clone()));
            months.push(Month {
                chunk: *chunk,
                derived,
                reviews: Vec::new(),
            });
            entries_done += entries;
            reporter.progress(SyncStep::PullRequests, entries_done, entry_total);
        }
        staged.push(months);
    }
    reporter.progress(SyncStep::PullRequests, entry_total, entry_total);

    // GitHub, a month at a time.
    let month_total: u64 = staged.iter().map(|m| m.len() as u64).sum();
    reporter.progress(SyncStep::Github, 0, month_total);
    let session = if month_total == 0 {
        Err(String::new())
    } else {
        gh::gh_status(req.gh, req.root, &gh::Budget::new(gh::GH_BUDGET))
    };
    if let Ok(slug) = &session {
        let conn = req.store.lock_conn()?;
        store::repo_id(&conn, &dir, Some(slug))?;
    }
    let mut tallies: Vec<GhTally> = Vec::new();
    let mut months_done = 0;
    let mut blocked: Option<String> = None;
    for (item, months) in counted.iter().zip(staged.iter_mut()) {
        let mut tally = GhTally::default();
        for month in months.iter_mut() {
            if reporter.cancelled() {
                return Ok(cancelled("complete", None));
            }
            let result = match (&session, &blocked) {
                (_, Some(note)) => GhMonth::Failed(note.clone()),
                (Err(note), None) => {
                    tally.unavailable.get_or_insert_with(|| note.clone());
                    GhMonth::Failed(note.clone())
                }
                (Ok(slug), None) => run.enrich_month(&item.task.head, slug, month),
            };
            match &result {
                GhMonth::Complete => {}
                GhMonth::Partial(note) => {
                    tally.partial.get_or_insert_with(|| note.clone());
                }
                GhMonth::Failed(note) => {
                    if session.is_ok() {
                        tally.partial.get_or_insert_with(|| note.clone());
                    }
                    if note.to_ascii_lowercase().contains("rate limit") {
                        blocked = Some(note.clone());
                    }
                }
            }
            let keep_stored = matches!(result, GhMonth::Failed(_))
                && item.task.on_gh_failure == OnGhFailure::KeepStored;
            if !keep_stored {
                run.write_month(month)?;
            }
            months_done += 1;
            reporter.progress(SyncStep::Github, months_done, month_total);
        }
        tallies.push(tally);
    }
    reporter.progress(SyncStep::Github, month_total, month_total);

    // Survival, then the coverage of each interval that got this far.
    let task_total = counted.len() as u64;
    reporter.progress(SyncStep::Survival, 0, task_total);
    let mut worst = ("complete", None::<String>);
    for (index, ((item, months), tally)) in counted.iter().zip(&staged).zip(&tallies).enumerate() {
        if reporter.cancelled() {
            return Ok(cancelled(worst.0, worst.1));
        }
        let (samples, cut) = if item.task.survival {
            // Eligibility is judged now, not at the end of the period.
            let candidates =
                super::survival_candidates(months.iter().flat_map(|m| m.derived.drafts.iter()));
            super::compute_survival(
                req.root,
                &item.task.head,
                candidates,
                now,
                &Limits::DEFAULT,
                Some(&stop),
            )
        } else {
            (Vec::new(), false)
        };
        if reporter.cancelled() {
            return Ok(cancelled(worst.0, worst.1));
        }
        let (state, note) = tally.state();
        let note = compose_note(note, cut);
        run.finish(&item.task, &samples, state, note.clone())?;
        if gh_rank(state) > gh_rank(worst.0)
            || (worst.1.is_none() && gh_rank(state) == gh_rank(worst.0))
        {
            worst = (state, note);
        }
        reporter.progress(SyncStep::Survival, index as u64 + 1, task_total);
    }
    reporter.progress(SyncStep::Survival, task_total, task_total);

    if counted.is_empty() {
        worst = Run::state_of(&rows, req.from, req.to.min(now));
    }
    Ok(SyncOutcome {
        cancelled: false,
        gh_state: worst.0.to_string(),
        gh_note: worst.1,
    })
}

struct Run<'a> {
    req: &'a SyncRequest<'a>,
    repo: i64,
    cancel: CancelCheck<'a>,
}

impl Run<'_> {
    /// The worst GitHub state of the rows that touch `from..=to`, for a sync
    /// that found nothing to do.
    fn state_of(rows: &[CoverageRow], from: i64, to: i64) -> (&'static str, Option<String>) {
        let worst = rows
            .iter()
            .filter(|r| r.from_epoch <= to && r.to_epoch >= from)
            .max_by_key(|r| gh_rank(&r.gh_state));
        match worst.map(|r| (gh_rank(&r.gh_state), r)) {
            Some((2, row)) => ("unavailable", row.gh_note.clone()),
            Some((1, row)) => ("partial", row.gh_note.clone()),
            _ => ("complete", None),
        }
    }

    fn first_parent_entries(&self, head: &str, from: i64, to: i64) -> Result<u64, String> {
        let since = format!("--since=@{from}");
        let until = format!("--until=@{to}");
        let out = git_cancellable(
            self.req.root,
            &[
                "rev-list",
                "--count",
                "--first-parent",
                since.as_str(),
                until.as_str(),
                head,
                "--",
            ],
            self.cancel,
        )?;
        out.trim()
            .parse()
            .map_err(|_| "git returned an unexpected count".to_string())
    }

    /// The PRs git derives for one month, from the commits in the store.
    fn derive_month(
        &self,
        head: &str,
        chunk: (i64, i64),
        dropped: &HashSet<String>,
        used: &HashSet<String>,
    ) -> Result<Derived, String> {
        let req = self.req;
        let hashes = store::listed_hashes_cancellable(
            req.root,
            head,
            chunk.0,
            chunk.1,
            &["--no-merges"],
            Some(self.cancel),
        )?;
        let commits = req
            .store
            .with_read_conn(|conn| store::stored_commits(conn, self.repo, &hashes))?;
        let known = commits
            .iter()
            .map(|c| {
                (
                    c.hash.as_str(),
                    Known {
                        author: &c.author,
                        files: &c.files,
                    },
                )
            })
            .collect();
        let window = Window::from_epochs(chunk.0, chunk.1);
        super::derive_prs_known(
            req.root,
            head,
            &window,
            known,
            dropped,
            &Limits::DEFAULT,
            used.clone(),
            Some(self.cancel),
            &mut false,
        )
    }

    /// Overlays GitHub's PRs of the month on the ones git derived and keeps
    /// the reviews for the write.
    fn enrich_month(&self, head: &str, slug: &str, month: &mut Month) -> GhMonth {
        let req = self.req;
        let window = Window::from_epochs(month.chunk.0, month.chunk.1);
        let request = gh::Request {
            gh: req.gh,
            commits: req.commit_cache,
            root: req.root,
            branch: req.branch,
            head,
            window: &window,
            limits: &Limits::DEFAULT,
        };
        let budget = gh::Budget::new(gh::GH_BUDGET);
        let outcome = gh::enrich_checked(&request, slug, &budget, &mut month.derived);
        month.reviews = outcome.reviews;
        match (outcome.available, outcome.note) {
            (false, note) => GhMonth::Failed(note.unwrap_or_default()),
            (true, Some(note)) => GhMonth::Partial(note),
            (true, None) => GhMonth::Complete,
        }
    }
    /// Replaces what the store holds for the month by its PRs and reviews, in
    /// one transaction.
    fn write_month(&self, month: &Month) -> Result<(), String> {
        let req = self.req;
        let derived = &month.derived;
        let mut by_pr: HashMap<&str, Vec<String>> = HashMap::new();
        for (hash, id) in &derived.commit_pr {
            by_pr.entry(id.as_str()).or_default().push(hash.clone());
        }
        let prs: Vec<(RawPr, Vec<String>)> = derived
            .drafts
            .iter()
            .map(|draft| {
                let mut hashes = by_pr.remove(draft.pr.id.as_str()).unwrap_or_default();
                // A merge commit is no commit of the window, but a revert of it
                // has to find the PR.
                if let Some(merge) = &draft.pr.merge_commit {
                    if !derived.commit_pr.contains_key(merge) {
                        hashes.push(merge.clone());
                    }
                }
                (draft.pr.clone(), hashes)
            })
            .collect();
        let keep: HashSet<String> = prs.iter().map(|(pr, _)| pr.id.clone()).collect();
        let landings: Vec<String> = derived
            .drafts
            .iter()
            .filter(|d| d.pr.source == "gh")
            .flat_map(|d| d.hashes.iter().cloned())
            .collect();

        let mut conn = req.store.lock_conn()?;
        let tx = conn.transaction().map_err(|e| e.to_string())?;
        store::clear_prs_in(&tx, self.repo, req.branch, month.chunk.0, month.chunk.1)?;
        store::put_prs(&tx, self.repo, req.branch, &prs)?;
        store::put_reviews(&tx, self.repo, req.branch, &month.reviews)?;
        store::drop_prs_landing_in(&tx, self.repo, req.branch, &landings, &keep)?;
        tx.commit().map_err(|e| e.to_string())
    }

    /// Stores the survival samples of an interval and its coverage row, so the
    /// interval counts as synced only once everything else about it is.
    fn finish(
        &self,
        task: &Task,
        samples: &[RawSurvival],
        gh_state: &str,
        gh_note: Option<String>,
    ) -> Result<(), String> {
        let req = self.req;
        let mut conn = req.store.lock_conn()?;
        let tx = conn.transaction().map_err(|e| e.to_string())?;
        if task.survival {
            store::put_survival(&tx, self.repo, req.branch, &task.head, samples)?;
        }
        // The rows the new one cuts back go first, so they cannot cut it.
        for row in &task.bump {
            store::add_coverage(&tx, self.repo, req.branch, row)?;
        }
        let row = CoverageRow {
            from_epoch: task.from,
            to_epoch: task.to,
            head: task.head.clone(),
            synced_at: task.synced_at,
            gh_state: gh_state.to_string(),
            gh_note,
        };
        store::add_coverage(&tx, self.repo, req.branch, &row)?;
        tx.commit().map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;
    use std::process::{Command, Stdio};
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::Instant;

    use super::super::gh::{FakeGh, GhError, Unavailable};
    use super::super::tests::{TestRepo, ALICE, BOB};
    use super::super::{pipe_patch_ids, wait_with_deadline_or_cancel, WaitFailure};
    use super::*;

    const EPOCH_DATE: &str = "2026-01-01T00:00:00+0000";

    /// Records progress and cancels once `cancel_after` batches are reported.
    struct Recorder {
        events: Mutex<Vec<(SyncStep, u64, u64)>>,
        cancel_after: Option<usize>,
    }

    impl Recorder {
        fn new() -> Self {
            Self {
                events: Mutex::new(Vec::new()),
                cancel_after: None,
            }
        }

        fn cancelling_after(batches: usize) -> Self {
            Self {
                cancel_after: Some(batches),
                ..Self::new()
            }
        }

        fn done_counts(&self) -> Vec<u64> {
            self.events.lock().unwrap().iter().map(|e| e.1).collect()
        }
    }

    impl Reporter for Recorder {
        fn progress(&self, step: SyncStep, done: u64, total: u64) {
            self.events.lock().unwrap().push((step, done, total));
        }

        fn cancelled(&self) -> bool {
            self.cancel_after
                .is_some_and(|n| self.events.lock().unwrap().len() >= n)
        }
    }

    /// `count` commits on `main`, one file each, made with `git fast-import`.
    fn bulk_repo(count: usize) -> (TestRepo, Vec<String>) {
        let repo = TestRepo::new();
        let mut script = String::new();
        for i in 0..count {
            let message = format!("commit {i}");
            let content = format!("content {i}\n");
            script.push_str(&format!(
                "commit refs/heads/main\nmark :{mark}\n\
                 author Alice <alice@example.com> {time} +0000\n\
                 committer Alice <alice@example.com> {time} +0000\n\
                 data {mlen}\n{message}\n",
                mark = i + 1,
                time = 1_767_225_600 + i as i64 * 60,
                mlen = message.len(),
            ));
            if i > 0 {
                script.push_str(&format!("from :{i}\n"));
            }
            script.push_str(&format!(
                "M 100644 inline f{i}.txt\ndata {}\n{content}\n",
                content.len()
            ));
        }
        let mut child = Command::new("git")
            .arg("-C")
            .arg(repo.path())
            .args(["fast-import", "--quiet"])
            .env("GIT_CONFIG_GLOBAL", repo.path().join(".no-gitconfig"))
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .stdin(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(script.as_bytes())
            .unwrap();
        assert!(child.wait().unwrap().success());
        repo.run(&["reset", "-q", "--hard", "main"], ALICE, EPOCH_DATE);
        let listed = repo.run(&["rev-list", "--reverse", "main"], ALICE, EPOCH_DATE);
        let hashes = listed.lines().map(str::to_string).collect();
        (repo, hashes)
    }

    fn setup(repo: &TestRepo) -> (SessionStore, i64) {
        let store = SessionStore::open_in_memory().unwrap();
        let dir = store::git_common_dir(repo.path()).unwrap();
        let id = {
            let conn = store.lock_conn().unwrap();
            store::repo_id(&conn, &dir, None).unwrap()
        };
        (store, id)
    }

    fn row_count(store: &SessionStore, repo: i64) -> i64 {
        store
            .lock_conn()
            .unwrap()
            .query_row(
                "SELECT COUNT(*) FROM contrib_commits WHERE repo_id = ?1",
                [repo],
                |row| row.get(0),
            )
            .unwrap()
    }

    fn patch_of(store: &SessionStore, repo: i64, hash: &str) -> Option<String> {
        store
            .lock_conn()
            .unwrap()
            .query_row(
                "SELECT patch_id FROM contrib_commits WHERE repo_id = ?1 AND hash = ?2",
                rusqlite::params![repo, hash],
                |row| row.get(0),
            )
            .unwrap()
    }

    /// Git lists nothing for `--since=@N` with a tiny N, so start in 2001.
    const WIDE: (i64, i64) = (1_000_000_000, 4_000_000_000);

    fn estimate_of(
        repo: &TestRepo,
        store: &SessionStore,
        id: Option<i64>,
        github: &dyn gh::GhRunner,
    ) -> Estimate {
        estimate(repo.path(), "main", WIDE.0, WIDE.1, store, id, github).unwrap()
    }

    fn with_origin(repo: &TestRepo) {
        repo.run(
            &[
                "remote",
                "add",
                "origin",
                "https://github.com/acme/widgets.git",
            ],
            ALICE,
            EPOCH_DATE,
        );
    }

    #[test]
    fn estimate_counts_match_git_and_drop_after_ingest() {
        let (repo, hashes) = bulk_repo(7);
        let (store, id) = setup(&repo);
        let git_count: u64 = repo
            .run(&["rev-list", "--count", "main"], ALICE, EPOCH_DATE)
            .parse()
            .unwrap();

        let before = estimate_of(&repo, &store, Some(id), &Unavailable);
        assert_eq!(before.commits, git_count);
        assert_eq!(before.stored_commits, 0);
        assert_eq!(before.first_parent, git_count);
        assert_eq!(before.prs, None);

        // A repository the store does not know yet gives the same answer.
        assert_eq!(estimate_of(&repo, &store, None, &Unavailable), before);

        let outcome = ingest_commits(&store, id, repo.path(), &hashes, &Recorder::new()).unwrap();
        assert_eq!(outcome.written, 7);

        let after = estimate_of(&repo, &store, Some(id), &Unavailable);
        assert_eq!(after.commits, git_count);
        assert_eq!(after.stored_commits, git_count);
    }

    #[test]
    fn estimate_respects_the_window() {
        let repo = TestRepo::new();
        repo.commit(&[("a", "1")], "a", ALICE, "2026-02-01T10:00:00+0000");
        repo.commit(&[("b", "1")], "b", ALICE, "2026-02-10T10:00:00+0000");
        repo.commit(&[("c", "1")], "c", ALICE, "2026-03-01T10:00:00+0000");
        let (store, id) = setup(&repo);
        let from = super::super::parse_iso_epoch("2026-02-05T00:00:00Z").unwrap();
        let to = super::super::parse_iso_epoch("2026-02-20T00:00:00Z").unwrap();
        let got = estimate(
            repo.path(),
            "main",
            from,
            to,
            &store,
            Some(id),
            &Unavailable,
        )
        .unwrap();
        assert_eq!((got.commits, got.first_parent), (1, 1));
    }

    #[test]
    fn estimate_counts_closed_prs_with_one_search() {
        let repo = TestRepo::new();
        repo.commit(&[("a", "1")], "a", ALICE, "2026-02-01T10:00:00+0000");
        with_origin(&repo);
        let (store, id) = setup(&repo);
        let github = FakeGh::new(|args| {
            if args.starts_with(&["api"]) {
                Ok(r#"{"total_count": 42, "items": []}"#.to_string())
            } else {
                Ok(String::new())
            }
        });
        let got = estimate_of(&repo, &store, Some(id), &github);
        assert_eq!(got.prs, Some(42));
        assert_eq!(
            github
                .calls
                .lock()
                .unwrap()
                .iter()
                .filter(|c| c[0] == "api")
                .count(),
            1
        );

        let calls = github.calls.lock().unwrap();
        let api = calls.iter().find(|c| c[0] == "api").unwrap();
        assert!(api.contains(&"GET".to_string()), "{api:?}");
        assert!(api.contains(&"search/issues".to_string()));
        let query = api.iter().find(|a| a.starts_with("q=")).unwrap();
        assert!(
            query.starts_with("q=repo:acme/widgets is:pr is:closed closed:2001-09-09T01:46:40Z.."),
            "{query}"
        );
    }

    #[test]
    fn estimate_has_no_pr_count_when_the_search_fails() {
        let repo = TestRepo::new();
        repo.commit(&[("a", "1")], "a", ALICE, "2026-02-01T10:00:00+0000");
        with_origin(&repo);
        let (store, id) = setup(&repo);
        let github = FakeGh::new(|args| {
            if args.starts_with(&["api"]) {
                Err(GhError::Failed("API rate limit exceeded".to_string()))
            } else {
                Ok(String::new())
            }
        });
        assert_eq!(estimate_of(&repo, &store, Some(id), &github).prs, None);
    }

    #[test]
    fn ingest_writes_batches_and_reports_progress() {
        let (repo, hashes) = bulk_repo(1_200);
        let (store, id) = setup(&repo);
        let reporter = Recorder::new();
        let outcome = ingest_commits(&store, id, repo.path(), &hashes, &reporter).unwrap();
        assert_eq!(
            outcome,
            IngestOutcome {
                written: 1_200,
                oversized: 0,
                cancelled: false
            }
        );
        assert_eq!(reporter.done_counts(), vec![500, 1_000, 1_200]);
        assert!(reporter
            .events
            .lock()
            .unwrap()
            .iter()
            .all(|e| e.0 == SyncStep::Commits && e.2 == 1_200));
        assert_eq!(row_count(&store, id), 1_200);

        // Everything is stored now: nothing is read or written again.
        let again = Recorder::new();
        let outcome = ingest_commits(&store, id, repo.path(), &hashes, &again).unwrap();
        assert_eq!(outcome, IngestOutcome::default());
        assert!(again.done_counts().is_empty());
        assert_eq!(row_count(&store, id), 1_200);
    }

    #[test]
    fn ingest_stores_files_co_authors_and_reverts() {
        let repo = TestRepo::new();
        let first = repo.commit(
            &[("a.txt", "one\ntwo\n")],
            "first",
            ALICE,
            "2026-02-01T10:00:00+0000",
        );
        let revert = repo.commit(
            &[("a.txt", "one\n")],
            &format!(
                "Revert \"first\"\n\nThis reverts commit {first}.\n\nCo-authored-by: Bob <bob@example.com>"
            ),
            ALICE,
            "2026-02-02T10:00:00+0000",
        );
        let (store, id) = setup(&repo);
        ingest_commits(
            &store,
            id,
            repo.path(),
            &[first.clone(), revert.clone()],
            &Recorder::new(),
        )
        .unwrap();
        let loaded = store
            .with_read_conn(|conn| {
                let window = store::window_commits(repo.path(), "main", WIDE.0, WIDE.1)?;
                store::load_window(conn, id, "main", "main", WIDE.0, WIDE.1, &window)
            })
            .unwrap();
        assert_eq!(loaded.missing_commits, 0);
        let by_subject = |s: &str| {
            loaded
                .raw
                .commits
                .iter()
                .find(|c| c.subject.starts_with(s))
                .unwrap()
        };
        let a = by_subject("first");
        assert_eq!(a.files.len(), 1);
        assert_eq!((a.files[0].added, a.files[0].deleted), (2, 0));
        assert_eq!(a.author.email, "alice@example.com");
        assert_eq!(by_subject("Revert").co_authors.len(), 1);
        let reverts: Option<String> = store
            .lock_conn()
            .unwrap()
            .query_row(
                "SELECT reverts FROM contrib_commits WHERE repo_id = ?1 AND hash = ?2",
                rusqlite::params![id, revert],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(reverts, Some(first));
    }

    #[test]
    fn cancel_after_the_first_batch_keeps_it() {
        let (repo, hashes) = bulk_repo(1_200);
        let (store, id) = setup(&repo);
        let reporter = Recorder::cancelling_after(1);
        let outcome = ingest_commits(&store, id, repo.path(), &hashes, &reporter).unwrap();
        assert!(outcome.cancelled);
        assert_eq!(outcome.written, 500);
        assert_eq!(reporter.done_counts(), vec![500]);
        assert_eq!(row_count(&store, id), 500);

        // The next run continues with what is missing.
        let rest = ingest_commits(&store, id, repo.path(), &hashes, &Recorder::new()).unwrap();
        assert_eq!((rest.written, rest.cancelled), (700, false));
        assert_eq!(row_count(&store, id), 1_200);
    }

    #[test]
    fn a_commit_over_the_file_cap_is_flagged_and_the_batch_succeeds() {
        let repo = TestRepo::new();
        let small = repo.commit(
            &[("s.txt", "x\n")],
            "small",
            ALICE,
            "2026-02-01T10:00:00+0000",
        );
        let big = repo.commit(
            &[("a", "1\n"), ("b", "1\n"), ("c", "1\n"), ("d", "1\n")],
            "big",
            ALICE,
            "2026-02-02T10:00:00+0000",
        );
        let (store, id) = setup(&repo);
        let limits = Limits {
            max_files_per_commit: 3,
            ..Limits::DEFAULT
        };
        let outcome = ingest_with(
            &store,
            id,
            repo.path(),
            &[small.clone(), big.clone()],
            &Recorder::new(),
            COMMIT_BATCH,
            &limits,
        )
        .unwrap();
        assert_eq!((outcome.written, outcome.oversized), (2, 1));
        let flags = |hash: &str| -> (String, bool) {
            let conn = store.lock_conn().unwrap();
            conn.query_row(
                "SELECT files_json, oversized FROM contrib_commits
                 WHERE repo_id = ?1 AND hash = ?2",
                rusqlite::params![id, hash],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap()
        };
        assert_eq!(flags(&big), ("[]".to_string(), true));
        let (files, flagged) = flags(&small);
        assert!(!flagged);
        assert!(files.contains("s.txt"));
        // A commit over the file cap is never fed to the patch pipe: it has no
        // patch id although its diff is far below the byte cap.
        assert_eq!(patch_of(&store, id, &big), None);
        assert!(patch_of(&store, id, &small).is_some());
    }

    #[test]
    fn a_byte_cap_below_the_batch_splits_it_and_every_commit_gets_a_patch_id() {
        let repo = TestRepo::new();
        let hashes: Vec<String> = (0..6)
            .map(|i| {
                repo.commit(
                    &[(&format!("f{i}.txt"), &format!("line {i}\n"))],
                    &format!("c{i}"),
                    ALICE,
                    &format!("2026-02-0{}T10:00:00+0000", i + 1),
                )
            })
            .collect();
        let (store, id) = setup(&repo);
        // One patch is a few hundred bytes; the six together do not fit.
        let limits = Limits {
            max_patch_bytes: 400,
            ..Limits::DEFAULT
        };
        assert!(
            matches!(
                run_patch_pipe(repo.path(), &hashes, limits.max_patch_bytes, None).unwrap(),
                PatchPipe::TooBig
            ),
            "the whole batch must exceed the cap"
        );
        for hash in &hashes {
            assert!(
                matches!(
                    run_patch_pipe(
                        repo.path(),
                        std::slice::from_ref(hash),
                        limits.max_patch_bytes,
                        None
                    )
                    .unwrap(),
                    PatchPipe::Ids(_)
                ),
                "one commit must fit the cap"
            );
        }
        let outcome = ingest_with(
            &store,
            id,
            repo.path(),
            &hashes,
            &Recorder::new(),
            COMMIT_BATCH,
            &limits,
        )
        .unwrap();
        assert_eq!(outcome.written, 6);
        for hash in &hashes {
            assert!(
                patch_of(&store, id, hash).is_some(),
                "no patch id for {hash}"
            );
        }
    }

    #[test]
    fn a_single_commit_over_the_byte_cap_has_no_patch_id() {
        let repo = TestRepo::new();
        let big = repo.commit(
            &[("big.txt", &"a long line of text\n".repeat(100))],
            "big",
            ALICE,
            "2026-02-01T10:00:00+0000",
        );
        let small = repo.commit(
            &[("s.txt", "x\n")],
            "small",
            ALICE,
            "2026-02-02T10:00:00+0000",
        );
        let (store, id) = setup(&repo);
        let limits = Limits {
            max_patch_bytes: 600,
            ..Limits::DEFAULT
        };
        let outcome = ingest_with(
            &store,
            id,
            repo.path(),
            &[big.clone(), small.clone()],
            &Recorder::new(),
            COMMIT_BATCH,
            &limits,
        )
        .unwrap();
        assert_eq!((outcome.written, outcome.oversized), (2, 0));
        assert_eq!(patch_of(&store, id, &big), None);
        assert!(patch_of(&store, id, &small).is_some());
    }

    #[test]
    fn a_cherry_pick_shares_the_patch_id_of_its_original() {
        let repo = TestRepo::new();
        repo.commit(
            &[("base.txt", "base\n")],
            "base",
            ALICE,
            "2026-02-01T10:00:00+0000",
        );
        repo.run(
            &["checkout", "-q", "-b", "side"],
            ALICE,
            "2026-02-02T10:00:00+0000",
        );
        let original = repo.commit(
            &[("f.txt", "change\n")],
            "feature",
            ALICE,
            "2026-02-02T10:00:00+0000",
        );
        repo.run(
            &["checkout", "-q", "main"],
            ALICE,
            "2026-02-03T10:00:00+0000",
        );
        repo.commit(
            &[("other.txt", "o\n")],
            "other",
            ALICE,
            "2026-02-03T10:00:00+0000",
        );
        repo.run(
            &["cherry-pick", &original],
            ALICE,
            "2026-02-04T10:00:00+0000",
        );
        let picked = repo.run(&["rev-parse", "HEAD"], ALICE, "2026-02-04T10:00:00+0000");
        assert_ne!(original, picked);

        let (store, id) = setup(&repo);
        // One commit per batch: the id must not depend on its batch mates.
        ingest_with(
            &store,
            id,
            repo.path(),
            &[original.clone(), picked.clone()],
            &Recorder::new(),
            1,
            &Limits::DEFAULT,
        )
        .unwrap();
        let first = patch_of(&store, id, &original);
        assert!(first.is_some());
        assert_eq!(first, patch_of(&store, id, &picked));
    }

    #[test]
    fn duplicate_input_hashes_are_written_once() {
        let (repo, hashes) = bulk_repo(3);
        let (store, id) = setup(&repo);
        let doubled: Vec<String> = hashes.iter().chain(hashes.iter()).cloned().collect();
        let reporter = Recorder::new();
        let outcome = ingest_commits(&store, id, repo.path(), &doubled, &reporter).unwrap();
        assert_eq!(outcome.written, 3);
        assert_eq!(reporter.done_counts(), vec![3]);
    }

    /// Reports whether the database lock was free each time it is asked or told.
    struct LockProbe<'a> {
        store: &'a SessionStore,
        locked_at_some_point: AtomicBool,
        calls: AtomicUsize,
    }

    impl LockProbe<'_> {
        fn check(&self) {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if !self.store.conn_is_free() {
                self.locked_at_some_point.store(true, Ordering::SeqCst);
            }
        }
    }

    impl Reporter for LockProbe<'_> {
        fn progress(&self, _: SyncStep, _: u64, _: u64) {
            self.check();
        }

        fn cancelled(&self) -> bool {
            self.check();
            false
        }
    }

    #[test]
    fn ingest_does_not_hold_the_database_lock_while_git_runs() {
        let (repo, hashes) = bulk_repo(5);
        let (store, id) = setup(&repo);
        let probe = LockProbe {
            store: &store,
            locked_at_some_point: AtomicBool::new(false),
            calls: AtomicUsize::new(0),
        };
        // Two batches. `cancelled` is asked between batches and while each git
        // process runs, so every probe call is made during or around git work.
        let outcome = ingest_with(
            &store,
            id,
            repo.path(),
            &hashes,
            &probe,
            3,
            &Limits::DEFAULT,
        )
        .unwrap();
        assert_eq!(outcome.written, 5);
        assert!(probe.calls.load(Ordering::SeqCst) >= 6);
        assert!(!probe.locked_at_some_point.load(Ordering::SeqCst));
    }

    #[test]
    fn estimate_does_not_hold_the_database_lock_while_gh_runs() {
        let repo = TestRepo::new();
        repo.commit(&[("a", "1")], "a", ALICE, "2026-02-01T10:00:00+0000");
        with_origin(&repo);
        let (store, id) = setup(&repo);
        let store = Arc::new(store);
        let held = Arc::new(AtomicBool::new(false));
        let github = {
            let (store, held) = (store.clone(), held.clone());
            FakeGh::new(move |_| {
                if !store.conn_is_free() {
                    held.store(true, Ordering::SeqCst);
                }
                Ok(r#"{"total_count": 1}"#.to_string())
            })
        };
        let got = estimate_of(&repo, &store, Some(id), &github);
        assert_eq!(got.prs, Some(1));
        assert!(!github.calls.lock().unwrap().is_empty());
        assert!(!held.load(Ordering::SeqCst));
    }

    /// Cancels the batch that is running (not the one before it).
    struct MidBatch {
        progressed: AtomicUsize,
        asked_since: AtomicUsize,
        done: Mutex<Vec<u64>>,
    }

    impl Reporter for MidBatch {
        fn progress(&self, _: SyncStep, done: u64, _: u64) {
            self.progressed.fetch_add(1, Ordering::SeqCst);
            self.asked_since.store(0, Ordering::SeqCst);
            self.done.lock().unwrap().push(done);
        }

        fn cancelled(&self) -> bool {
            // After the first batch: the first question is the check between
            // batches (no), the next one comes from inside the running git call.
            self.progressed.load(Ordering::SeqCst) > 0
                && self.asked_since.fetch_add(1, Ordering::SeqCst) >= 1
        }
    }

    #[test]
    fn cancel_during_a_batch_kills_git_and_writes_none_of_it() {
        let (repo, hashes) = bulk_repo(4);
        let (store, id) = setup(&repo);
        let reporter = MidBatch {
            progressed: AtomicUsize::new(0),
            asked_since: AtomicUsize::new(0),
            done: Mutex::new(Vec::new()),
        };
        let outcome = ingest_with(
            &store,
            id,
            repo.path(),
            &hashes,
            &reporter,
            2,
            &Limits::DEFAULT,
        )
        .unwrap();
        assert!(outcome.cancelled);
        assert_eq!(outcome.written, 2);
        assert_eq!(*reporter.done.lock().unwrap(), vec![2]);
        assert_eq!(row_count(&store, id), 2);
    }

    #[cfg(unix)]
    #[test]
    fn a_cancel_kills_a_child_that_is_still_running() {
        let started = Instant::now();
        let mut child = Command::new("sleep")
            .arg("30")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let cancel = || started.elapsed() > Duration::from_millis(150);
        let result =
            wait_with_deadline_or_cancel(&mut child, Duration::from_secs(60), Some(&cancel));
        assert!(matches!(result, Err(WaitFailure::Cancelled)));
        assert!(started.elapsed() < Duration::from_secs(10));
        assert!(child.try_wait().unwrap().is_some(), "child was not reaped");
    }

    #[cfg(unix)]
    #[test]
    fn a_cancel_kills_both_children_of_the_patch_pipe() {
        let repo = TestRepo::new();
        repo.commit(&[("a", "1")], "a", ALICE, "2026-02-01T10:00:00+0000");
        let mut log = Command::new("sleep");
        log.arg("30");
        let started = Instant::now();
        let cancel = || started.elapsed() > Duration::from_millis(150);
        let result = pipe_patch_ids(repo.path(), log, None, 1 << 20, Some(&cancel)).unwrap();
        assert!(matches!(result, PatchPipe::Cancelled));
        assert!(started.elapsed() < Duration::from_secs(10));
    }

    // -----------------------------------------------------------------------
    // the sync
    // -----------------------------------------------------------------------

    use serde_json::{json, Value};

    /// Months follow UTC for the rest of the test, whatever the machine says.
    fn utc() {
        TEST_OFFSET.with(|fixed| fixed.set(Some(0)));
    }

    fn at(date: &str) -> i64 {
        super::super::parse_iso_epoch(date).unwrap()
    }

    /// Cancels as soon as `step` reported `done`.
    struct StopAt {
        events: Mutex<Vec<(SyncStep, u64, u64)>>,
        step: SyncStep,
        done: u64,
    }

    impl StopAt {
        fn new(step: SyncStep, done: u64) -> Self {
            Self {
                events: Mutex::new(Vec::new()),
                step,
                done,
            }
        }
    }

    impl Reporter for StopAt {
        fn progress(&self, step: SyncStep, done: u64, total: u64) {
            self.events.lock().unwrap().push((step, done, total));
        }

        fn cancelled(&self) -> bool {
            self.events
                .lock()
                .unwrap()
                .iter()
                .any(|e| e.0 == self.step && e.1 >= self.done)
        }
    }

    /// January and February 2026 on `main`: PR 7 (squash), PR 9 (merge commit),
    /// PR 12 (squash), a direct push and a revert of PR 12.
    struct History {
        repo: TestRepo,
        squash7: String,
        merge9: String,
        squash12: String,
    }

    fn history() -> History {
        let repo = TestRepo::new();
        repo.commit(
            &[("a.txt", "1\n")],
            "base",
            ALICE,
            "2026-01-05T10:00:00+0000",
        );
        let squash7 = repo.commit(
            &[("parser.txt", "1\n2\n3\n")],
            "Add parser (#7)\n\n* one\n\n* two",
            ALICE,
            "2026-01-10T10:00:00+0000",
        );
        let when = "2026-01-11T10:00:00+0000";
        repo.run(&["checkout", "-q", "-b", "feature"], ALICE, when);
        repo.commit(&[("f1.txt", "x\n")], "f1", BOB, when);
        repo.commit(
            &[("f2.txt", "x\ny\n")],
            "f2",
            BOB,
            "2026-01-12T10:00:00+0000",
        );
        repo.run(&["checkout", "-q", "main"], ALICE, when);
        let merged = "2026-01-14T10:00:00+0000";
        repo.run(
            &[
                "merge",
                "--no-ff",
                "-q",
                "-m",
                "Merge pull request #9 from bob/feature",
                "feature",
            ],
            ALICE,
            merged,
        );
        let merge9 = repo.run(&["rev-parse", "HEAD"], ALICE, merged);
        let squash12 = repo.commit(
            &[("lexer.txt", "l\nl\n")],
            "Add lexer (#12)",
            ALICE,
            "2026-02-10T10:00:00+0000",
        );
        repo.commit(
            &[("direct.txt", "d\n")],
            "Direct push",
            BOB,
            "2026-02-12T10:00:00+0000",
        );
        repo.commit(
            &[("lexer.txt", "l\n")],
            &format!("Revert \"Add lexer (#12)\"\n\nThis reverts commit {squash12}."),
            ALICE,
            "2026-02-14T10:00:00+0000",
        );
        History {
            repo,
            squash7,
            merge9,
            squash12,
        }
    }

    fn head_of(repo: &TestRepo) -> String {
        repo.run(&["rev-parse", "main"], ALICE, EPOCH_DATE)
    }

    fn gh_user(login: &str) -> Value {
        json!({ "id": "x", "is_bot": false, "login": login, "name": login })
    }

    fn gh_merged(number: u64, login: &str, merged_at: &str, oid: &str, reviews: Value) -> Value {
        json!({
            "number": number, "title": format!("PR {number}"), "author": gh_user(login),
            "mergedAt": merged_at, "mergeCommit": { "oid": oid },
            "baseRefName": "main", "closingIssuesReferences": [], "reviews": reviews
        })
    }

    fn gh_review(login: &str, at: &str) -> Value {
        json!({ "author": gh_user(login), "state": "APPROVED", "submittedAt": at, "body": "" })
    }

    /// The merged PRs of `history()` as GitHub lists them.
    fn history_prs(h: &History) -> Vec<Value> {
        vec![
            gh_merged(7, "alice", "2026-01-10T10:00:00Z", &h.squash7, json!([])),
            gh_merged(
                9,
                "bob",
                "2026-01-14T10:00:00Z",
                &h.merge9,
                json!([gh_review("carol", "2026-01-13T09:00:00Z")]),
            ),
            gh_merged(
                12,
                "alice",
                "2026-02-10T10:00:00Z",
                &h.squash12,
                json!([gh_review("carol", "2026-02-11T09:00:00Z")]),
            ),
        ]
    }

    /// GitHub as far as the sync can tell: lists filtered by the `merged:` /
    /// `closed:` range of the search like the real one, one commit per PR.
    /// Merged lists whose range starts with `fail_from` fail.
    fn hub(merged: Vec<Value>, closed: Vec<Value>, fail_from: Option<&'static str>) -> FakeGh {
        FakeGh::new(move |args| {
            let value = |flag: &str| {
                args.iter()
                    .position(|a| *a == flag)
                    .map(|i| args[i + 1])
                    .unwrap_or("")
            };
            match args[0] {
                "auth" => return Ok("ok".to_string()),
                "repo" => {
                    return Ok(json!({
                        "nameWithOwner": "acme/widgets",
                        "url": "https://github.com/acme/widgets"
                    })
                    .to_string())
                }
                "pr" => {}
                other => panic!("unexpected gh call {other}"),
            }
            let (state, fields, search) = (value("--state"), value("--json"), value("--search"));
            let range = search
                .split_once(':')
                .and_then(|(_, rest)| rest.split_whitespace().next())
                .and_then(|r| r.split_once(".."))
                .unwrap_or_else(|| panic!("no range in {search:?}"));
            if state == "merged" && fields != "number,commits" {
                if let Some(prefix) = fail_from {
                    if range.0.starts_with(prefix) {
                        return Err(GhError::Failed("boom".to_string()));
                    }
                }
            }
            let (lo, hi) = (at(range.0), at(range.1));
            let (list, key) = if state == "merged" {
                (&merged, "mergedAt")
            } else {
                (&closed, "closedAt")
            };
            let inside: Vec<&Value> = list
                .iter()
                .filter(|pr| {
                    pr[key]
                        .as_str()
                        .map(at)
                        .is_some_and(|epoch| epoch >= lo && epoch <= hi)
                })
                .collect();
            let reply: Vec<Value> = if fields == "number,commits" {
                inside
                    .iter()
                    .map(|pr| {
                        let login = pr["author"]["login"].as_str().unwrap();
                        json!({ "number": pr["number"], "commits": [{
                            "oid": "0".repeat(40),
                            "messageHeadline": "work",
                            "authoredDate": pr[key],
                            "committedDate": pr[key],
                            "authors": [{
                                "name": login, "email": format!("{login}@example.com"),
                                "login": login, "id": "x"
                            }],
                        }] })
                    })
                    .collect()
            } else {
                inside.into_iter().cloned().collect()
            };
            Ok(Value::Array(reply).to_string())
        })
    }

    fn run_with(
        h: &History,
        store: &SessionStore,
        github: &dyn gh::GhRunner,
        mode: SyncMode,
        (from, to): (i64, i64),
        reporter: &dyn Reporter,
    ) -> SyncOutcome {
        let head = head_of(&h.repo);
        let cache = gh::CommitCache::new();
        run_sync(
            &SyncRequest {
                store,
                root: h.repo.path(),
                branch: "main",
                head: &head,
                from,
                to,
                mode,
                gh: github,
                commit_cache: &cache,
            },
            reporter,
        )
        .unwrap()
    }

    fn full(
        h: &History,
        store: &SessionStore,
        github: &dyn gh::GhRunner,
        window: (i64, i64),
    ) -> SyncOutcome {
        run_with(h, store, github, SyncMode::Full, window, &Recorder::new())
    }

    const JAN_FEB: (&str, &str) = ("2026-01-01T00:00:00Z", "2026-02-28T23:59:59Z");

    fn jan_feb() -> (i64, i64) {
        (at(JAN_FEB.0), at(JAN_FEB.1))
    }

    fn load(h: &History, store: &SessionStore, (from, to): (i64, i64)) -> store::Loaded {
        let head = head_of(&h.repo);
        let window = store::window_commits(h.repo.path(), &head, from, to).unwrap();
        let dir = store::git_common_dir(h.repo.path()).unwrap();
        store
            .with_read_conn(|conn| {
                let id = store::find_repo_id(conn, &dir)?.unwrap();
                store::load_window(conn, id, "main", &head, from, to, &window)
            })
            .unwrap()
    }

    fn coverage_rows(h: &History, store: &SessionStore) -> Vec<CoverageRow> {
        let dir = store::git_common_dir(h.repo.path()).unwrap();
        store
            .with_read_conn(|conn| {
                let id = store::find_repo_id(conn, &dir)?.unwrap();
                store::coverage(conn, id, "main")
            })
            .unwrap()
    }

    fn set_synced_at(store: &SessionStore, synced_at: i64) {
        store
            .lock_conn()
            .unwrap()
            .execute("UPDATE contrib_coverage SET synced_at = ?1", [synced_at])
            .unwrap();
    }

    /// The `merged:` search of every PR list call (not the commit follow-ups).
    fn merged_searches(github: &FakeGh) -> Vec<String> {
        let mut searches: Vec<String> = github
            .calls
            .lock()
            .unwrap()
            .iter()
            .filter(|c| c.iter().any(|a| a == "merged") && !c.iter().any(|a| a == "number,commits"))
            .filter_map(|c| {
                let at = c.iter().position(|a| a == "--search")?;
                Some(c[at + 1].clone())
            })
            .collect();
        searches.sort();
        searches
    }

    fn events_of(reporter: &Recorder, step: SyncStep) -> Vec<(u64, u64)> {
        reporter
            .events
            .lock()
            .unwrap()
            .iter()
            .filter(|e| e.0 == step)
            .map(|e| (e.1, e.2))
            .collect()
    }

    /// The ids of the PRs that carry a number; git also derives a unit for
    /// every commit that landed without one (the first, a direct push).
    fn pr_ids(loaded: &store::Loaded) -> Vec<String> {
        let mut ids: Vec<String> = loaded
            .raw
            .prs
            .iter()
            .map(|p| p.id.clone())
            .filter(|id| id.bytes().all(|b| b.is_ascii_digit()))
            .collect();
        ids.sort();
        ids
    }

    #[test]
    fn a_two_month_window_asks_github_once_per_month() {
        utc();
        let h = history();
        let (store, _) = setup(&h.repo);
        let github = hub(history_prs(&h), vec![], None);
        let outcome = full(&h, &store, &github, jan_feb());
        assert!(!outcome.cancelled);
        assert_eq!(outcome.gh_state, "complete", "{:?}", outcome.gh_note);
        assert_eq!(
            merged_searches(&github),
            [
                "merged:2026-01-01T00:00:00Z..2026-01-31T23:59:59Z base:main",
                "merged:2026-02-01T00:00:00Z..2026-02-28T23:59:59Z base:main",
            ]
        );
    }

    #[test]
    fn a_month_whose_github_call_fails_keeps_its_git_prs_and_is_partial() {
        utc();
        let h = history();
        let (store, _) = setup(&h.repo);
        let github = hub(history_prs(&h), vec![], Some("2026-02"));
        let outcome = full(&h, &store, &github, jan_feb());
        assert_eq!(outcome.gh_state, "partial");
        assert!(outcome.gh_note.unwrap().contains("boom"));

        let loaded = load(&h, &store, jan_feb());
        assert!(loaded.covered);
        assert_eq!(loaded.gh_state.as_deref(), Some("partial"));
        let source = |id: &str| {
            loaded
                .raw
                .prs
                .iter()
                .find(|p| p.id == id)
                .unwrap_or_else(|| panic!("no PR {id}"))
                .source
                .clone()
        };
        assert_eq!(source("7"), "gh");
        assert_eq!(source("9"), "gh");
        assert_eq!(source("12"), "git", "February keeps what git derived");
        let rows = coverage_rows(&h, &store);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].gh_state, "partial");
    }

    #[test]
    fn without_github_the_git_prs_are_stored_and_the_state_is_unavailable() {
        utc();
        let h = history();
        let (store, _) = setup(&h.repo);
        let outcome = full(&h, &store, &Unavailable, jan_feb());
        assert_eq!(outcome.gh_state, "unavailable");
        assert!(outcome.gh_note.unwrap().contains("not installed"));
        let loaded = load(&h, &store, jan_feb());
        assert!(loaded.covered);
        assert_eq!(pr_ids(&loaded), ["12", "7", "9"].map(String::from));
        assert!(loaded.raw.prs.iter().all(|p| p.source == "git"));
        assert!(!loaded.raw.gh_available);
    }

    #[test]
    fn cancel_during_github_leaves_no_coverage_and_the_next_run_resumes() {
        utc();
        let h = history();
        let (store, repo) = setup(&h.repo);
        let github = hub(history_prs(&h), vec![], None);
        let stopper = StopAt::new(SyncStep::Github, 1);
        let outcome = run_with(&h, &store, &github, SyncMode::Full, jan_feb(), &stopper);
        assert!(outcome.cancelled);
        assert_ne!(outcome.gh_state, "complete");
        assert!(coverage_rows(&h, &store).is_empty());
        assert!(!load(&h, &store, jan_feb()).covered);
        assert!(row_count(&store, repo) > 0, "stored batches stay");

        let again = Recorder::new();
        let outcome = run_with(&h, &store, &github, SyncMode::Full, jan_feb(), &again);
        assert!(!outcome.cancelled);
        assert_eq!(
            events_of(&again, SyncStep::Commits).last().copied(),
            Some((0, 0)),
            "the commits were stored by the first run"
        );
        assert!(load(&h, &store, jan_feb()).covered);
    }

    #[test]
    fn a_complete_window_is_synced_again_without_any_work() {
        utc();
        let h = history();
        let (store, repo) = setup(&h.repo);
        full(&h, &store, &hub(history_prs(&h), vec![], None), jan_feb());
        let rows = row_count(&store, repo);

        let github = hub(history_prs(&h), vec![], None);
        let report = Recorder::new();
        let outcome = run_with(&h, &store, &github, SyncMode::Full, jan_feb(), &report);
        assert_eq!(outcome.gh_state, "complete");
        assert!(!outcome.cancelled);
        assert!(github.calls.lock().unwrap().is_empty(), "no gh call");
        for step in [SyncStep::Commits, SyncStep::PullRequests, SyncStep::Github] {
            assert!(
                events_of(&report, step).iter().all(|e| *e == (0, 0)),
                "{step:?} had work"
            );
        }
        assert_eq!(row_count(&store, repo), rows);
    }

    #[test]
    fn a_larger_window_only_syncs_what_is_not_covered() {
        utc();
        let h = history();
        let (store, _) = setup(&h.repo);
        let january = (at("2026-01-01T00:00:00Z"), at("2026-01-31T23:59:59Z"));
        full(&h, &store, &hub(history_prs(&h), vec![], None), january);
        assert!(!load(&h, &store, jan_feb()).covered);

        let github = hub(history_prs(&h), vec![], None);
        let report = Recorder::new();
        run_with(&h, &store, &github, SyncMode::Full, jan_feb(), &report);
        // Only February: its commits (lexer, direct push, revert) and one list.
        assert_eq!(
            events_of(&report, SyncStep::Commits).last().copied(),
            Some((3, 3))
        );
        assert_eq!(
            merged_searches(&github),
            ["merged:2026-02-01T00:00:00Z..2026-02-28T23:59:59Z base:main"]
        );
        let loaded = load(&h, &store, jan_feb());
        assert!(loaded.covered);
        assert_eq!(coverage_rows(&h, &store).len(), 1, "the rows were merged");
    }

    #[test]
    fn load_after_a_full_sync_is_covered_and_has_prs_reviews_and_survival() {
        utc();
        let h = history();
        let (store, _) = setup(&h.repo);
        full(&h, &store, &hub(history_prs(&h), vec![], None), jan_feb());
        let loaded = load(&h, &store, jan_feb());
        assert!(loaded.covered);
        assert_eq!(loaded.missing_commits, 0);
        assert_eq!(loaded.gh_state.as_deref(), Some("complete"));
        assert!(loaded.raw.gh_available);
        assert_eq!(pr_ids(&loaded), ["12", "7", "9"].map(String::from));
        assert_eq!(loaded.raw.reviews.len(), 2);
        let sampled = |id: &str| loaded.raw.survival.iter().any(|s| s.pr_id == id);
        assert!(sampled("7") && sampled("9") && sampled("12"));
        assert_eq!(loaded.raw.reverted_pr_ids, ["12"]);
        let seven = loaded.raw.survival.iter().find(|s| s.pr_id == "7").unwrap();
        assert_eq!((seven.written, seven.alive), (3, 3));
    }

    #[test]
    fn reviews_of_a_pr_listed_in_one_month_are_kept_from_the_month_before() {
        utc();
        let h = history();
        let (store, _) = setup(&h.repo);
        let mut prs = history_prs(&h);
        // Reviewed in January, merged in February.
        prs[2]["reviews"] = json!([gh_review("carol", "2026-01-30T09:00:00Z")]);
        full(&h, &store, &hub(prs, vec![], None), jan_feb());
        let loaded = load(&h, &store, jan_feb());
        let review = loaded.raw.reviews.iter().find(|r| r.pr_id == "12").unwrap();
        assert_eq!(review.submitted_at, "2026-01-30T09:00:00Z");
    }

    #[test]
    fn percent_never_goes_down_over_a_run_and_ends_at_100() {
        utc();
        assert_eq!(percent(SyncStep::Counting, 0, 1), 0);
        assert_eq!(percent(SyncStep::Commits, 0, 10), 2);
        assert_eq!(percent(SyncStep::Commits, 10, 10), 52);
        assert_eq!(percent(SyncStep::Github, 0, 4), 65);
        assert_eq!(percent(SyncStep::Github, 0, 0), 90);
        assert_eq!(percent(SyncStep::Survival, 1, 1), 100);
        assert_eq!(percent(SyncStep::Survival, 0, 0), 100);

        let h = history();
        let (store, _) = setup(&h.repo);
        // Two stretches to work on: February, then January and the rest.
        let january = (at("2026-01-01T00:00:00Z"), at("2026-01-31T23:59:59Z"));
        full(&h, &store, &hub(history_prs(&h), vec![], None), january);
        let report = Recorder::new();
        run_with(
            &h,
            &store,
            &hub(history_prs(&h), vec![], None),
            SyncMode::Full,
            jan_feb(),
            &report,
        );
        let marks: Vec<u8> = report
            .events
            .lock()
            .unwrap()
            .iter()
            .map(|&(step, done, total)| percent(step, done, total))
            .collect();
        assert!(marks.len() > 6);
        assert!(marks.windows(2).all(|w| w[0] <= w[1]), "{marks:?}");
        assert_eq!(marks.last(), Some(&100));
    }

    #[test]
    fn the_steps_report_exact_totals() {
        utc();
        let h = history();
        let (store, _) = setup(&h.repo);
        let report = Recorder::new();
        run_with(
            &h,
            &store,
            &hub(history_prs(&h), vec![], None),
            SyncMode::Full,
            jan_feb(),
            &report,
        );
        // base, #7, f1, f2, #12, direct push, revert.
        assert_eq!(
            events_of(&report, SyncStep::Commits).last().copied(),
            Some((7, 7))
        );
        // Months: January and February.
        assert_eq!(
            events_of(&report, SyncStep::Github).last().copied(),
            Some((2, 2))
        );
        for step in [SyncStep::PullRequests, SyncStep::Commits] {
            let events = events_of(&report, step);
            let (done, total) = *events.last().unwrap();
            assert_eq!(done, total, "{step:?}");
            assert!(events.iter().all(|e| e.0 <= e.1), "{step:?} {events:?}");
        }
    }

    fn add_late_pr(h: &History, number: u64) -> String {
        let when = "2026-03-02T10:00:00+0000";
        let branch = format!("late{number}");
        h.repo.run(&["checkout", "-q", "-b", &branch], ALICE, when);
        h.repo.commit(&[("l1.txt", "x\n")], "late one", BOB, when);
        h.repo.commit(
            &[("l2.txt", "x\n")],
            "late two",
            BOB,
            "2026-03-02T11:00:00+0000",
        );
        h.repo.run(&["checkout", "-q", "main"], ALICE, when);
        let merged = "2026-03-03T10:00:00+0000";
        h.repo.run(
            &[
                "merge",
                "--no-ff",
                "-q",
                "-m",
                &format!("Merge pull request #{number} from bob/{branch}"),
                &branch,
            ],
            ALICE,
            merged,
        );
        head_of(&h.repo)
    }

    #[test]
    fn a_top_up_after_one_new_merge_ingests_exactly_its_commits_and_moves_the_head() {
        utc();
        let h = history();
        let (store, repo) = setup(&h.repo);
        let window = (at("2026-01-01T00:00:00Z"), at("2026-03-05T00:00:00Z"));
        full(&h, &store, &hub(history_prs(&h), vec![], None), window);
        let old_head = head_of(&h.repo);
        // The period was synced on March 1st.
        set_synced_at(&store, at("2026-03-01T00:00:00Z"));
        let before = row_count(&store, repo);

        let new_head = add_late_pr(&h, 15);
        let mut prs = history_prs(&h);
        prs.push(gh_merged(
            15,
            "bob",
            "2026-03-03T10:00:00Z",
            &new_head,
            json!([]),
        ));
        let github = hub(prs, vec![], None);
        let report = Recorder::new();
        run_with(
            &h,
            &store,
            &github,
            SyncMode::TopUp { old_head },
            window,
            &report,
        );

        assert_eq!(row_count(&store, repo), before + 2);
        assert_eq!(
            events_of(&report, SyncStep::Commits).last().copied(),
            Some((2, 2))
        );
        assert_eq!(
            merged_searches(&github),
            ["merged:2026-03-01T00:00:00Z..2026-03-05T00:00:00Z base:main"]
        );
        // No survival work: the third and last step has no sample to write.
        let rows = coverage_rows(&h, &store);
        assert_eq!(rows.len(), 1, "{rows:?}");
        assert_eq!(rows[0].head, new_head);
        assert_eq!(rows[0].to_epoch, window.1);
        assert_eq!(rows[0].from_epoch, window.0);
        let loaded = load(&h, &store, window);
        assert!(loaded.covered);
        assert_eq!(pr_ids(&loaded), ["12", "15", "7", "9"].map(String::from));
        assert!(loaded.raw.survival.iter().all(|s| s.pr_id != "15"));
    }

    #[test]
    fn a_top_up_after_a_force_push_syncs_the_commits_the_store_lacks() {
        utc();
        let h = history();
        let (store, repo) = setup(&h.repo);
        let window = (at("2026-01-01T00:00:00Z"), at("2026-03-05T00:00:00Z"));
        full(&h, &store, &hub(history_prs(&h), vec![], None), window);
        let old_head = head_of(&h.repo);
        set_synced_at(&store, at("2026-03-01T00:00:00Z"));
        let before = row_count(&store, repo);

        // Drop the revert and put another commit in its place.
        let when = "2026-02-15T10:00:00+0000";
        h.repo
            .run(&["reset", "-q", "--hard", "HEAD~1"], ALICE, when);
        h.repo
            .commit(&[("other.txt", "o\n")], "Replacement push", ALICE, when);

        let report = Recorder::new();
        run_with(
            &h,
            &store,
            &hub(history_prs(&h), vec![], None),
            SyncMode::TopUp {
                old_head: old_head.clone(),
            },
            window,
            &report,
        );
        assert_eq!(
            events_of(&report, SyncStep::Commits).last().copied(),
            Some((1, 1)),
            "only the missing commit"
        );
        assert_eq!(row_count(&store, repo), before + 1);
        let loaded = load(&h, &store, window);
        assert!(loaded.covered);
        let subjects: Vec<&str> = loaded
            .raw
            .commits
            .iter()
            .map(|c| c.subject.as_str())
            .collect();
        assert!(subjects.contains(&"Replacement push"));
        assert!(
            !subjects.iter().any(|s| s.starts_with("Revert")),
            "a commit that left the branch is not shown"
        );
        assert!(loaded.raw.reverted_pr_ids.is_empty());
        let head = head_of(&h.repo);
        assert!(
            loaded
                .raw
                .prs
                .iter()
                .any(|pr| pr.merge_commit.as_deref() == Some(head.as_str())),
            "the new commit has its PR"
        );
        assert_eq!(coverage_rows(&h, &store)[0].head, head);
        assert_ne!(head_of(&h.repo), old_head);
    }

    #[test]
    fn a_github_retry_only_runs_the_github_step_and_only_for_incomplete_intervals() {
        utc();
        let h = history();
        let (store, repo) = setup(&h.repo);
        // First without GitHub at all.
        full(&h, &store, &Unavailable, jan_feb());
        assert_eq!(coverage_rows(&h, &store)[0].gh_state, "unavailable");
        let commits = row_count(&store, repo);

        // A retry that fails keeps what is stored.
        let failing = hub(history_prs(&h), vec![], Some("2026-"));
        let outcome = run_with(
            &h,
            &store,
            &failing,
            SyncMode::GithubRetry,
            jan_feb(),
            &Recorder::new(),
        );
        assert_eq!(outcome.gh_state, "partial");
        let loaded = load(&h, &store, jan_feb());
        assert!(loaded.raw.prs.iter().all(|p| p.source == "git"));
        assert_eq!(loaded.gh_state.as_deref(), Some("partial"));

        // One that works replaces the git PRs by GitHub's.
        let github = hub(history_prs(&h), vec![], None);
        let report = Recorder::new();
        let outcome = run_with(
            &h,
            &store,
            &github,
            SyncMode::GithubRetry,
            jan_feb(),
            &report,
        );
        assert_eq!(outcome.gh_state, "complete");
        assert_eq!(
            events_of(&report, SyncStep::Commits).last().copied(),
            Some((0, 0))
        );
        assert_eq!(row_count(&store, repo), commits);
        let loaded = load(&h, &store, jan_feb());
        assert_eq!(loaded.gh_state.as_deref(), Some("complete"));
        assert_eq!(pr_ids(&loaded), ["12", "7", "9"].map(String::from));
        let numbered = |p: &&RawPr| p.id.bytes().all(|b| b.is_ascii_digit());
        assert!(loaded
            .raw
            .prs
            .iter()
            .filter(numbered)
            .all(|p| p.source == "gh"));
        assert_eq!(loaded.raw.reviews.len(), 2);
        assert_eq!(coverage_rows(&h, &store).len(), 1);

        // Nothing is left to retry.
        let idle = hub(history_prs(&h), vec![], None);
        run_with(
            &h,
            &store,
            &idle,
            SyncMode::GithubRetry,
            jan_feb(),
            &Recorder::new(),
        );
        assert!(idle.calls.lock().unwrap().is_empty());
    }

    #[test]
    fn a_pr_that_github_matched_replaces_the_one_git_stored_on_the_other_side_of_a_boundary() {
        utc();
        let h = history();
        let (store, repo) = setup(&h.repo);
        // January alone: the squash of #12 is not part of it.
        let january = (at("2026-01-01T00:00:00Z"), at("2026-01-31T23:59:59Z"));
        full(&h, &store, &Unavailable, january);
        // A git PR for PR 9's merge commit sits in the store under another id,
        // as if derived in the month before GitHub's list had it.
        let mut stale = loaded_pr(&h, &store, "9");
        stale.id = "deadbee".to_string();
        stale.number = None;
        stale.merged_at = Some("2025-12-31T23:59:59Z".to_string());
        {
            let mut conn = store.lock_conn().unwrap();
            let tx = conn.transaction().unwrap();
            store::put_prs(&tx, repo, "main", &[(stale, vec![])]).unwrap();
            tx.commit().unwrap();
        }
        let december_through_january = (at("2025-12-01T00:00:00Z"), january.1);
        let before = load(&h, &store, december_through_january);
        assert!(before.raw.prs.iter().any(|p| p.id == "deadbee"));

        let retry = Recorder::new();
        run_with(
            &h,
            &store,
            &hub(history_prs(&h), vec![], None),
            SyncMode::GithubRetry,
            january,
            &retry,
        );
        let after = load(&h, &store, december_through_january);
        assert!(
            after.raw.prs.iter().all(|p| p.id != "deadbee"),
            "{:?}",
            pr_ids(&after)
        );
        assert!(after.raw.prs.iter().any(|p| p.id == "9"));
    }

    fn loaded_pr(h: &History, store: &SessionStore, id: &str) -> RawPr {
        load(h, store, jan_feb())
            .raw
            .prs
            .into_iter()
            .find(|p| p.id == id)
            .unwrap()
    }

    #[test]
    fn a_full_sync_stores_what_the_one_shot_collector_produces() {
        utc();
        let h = history();
        let (store, _) = setup(&h.repo);
        let now = super::super::now_epoch();
        let window = (at("2026-01-01T00:00:00Z"), now);
        let prs = history_prs(&h);
        full(&h, &store, &hub(prs.clone(), vec![], None), window);
        let loaded = load(&h, &store, window);
        assert!(loaded.covered);

        let github = hub(prs, vec![], None);
        let old = super::super::collect_cached_with(
            &super::super::Cache::new(),
            &github,
            h.repo.path(),
            "2026-01-01T00:00:00Z",
            None,
            None,
            false,
        )
        .unwrap();
        let new = &loaded.raw;

        let sorted = |mut prs: Vec<RawPr>| {
            prs.sort_by(|a, b| a.id.cmp(&b.id));
            serde_json::to_value(prs).unwrap()
        };
        assert_eq!(sorted(new.prs.clone()), sorted(old.prs.clone()));

        let reviews = |reviews: &[RawReview]| {
            let mut sorted = reviews.to_vec();
            sorted.sort_by(|a, b| (&a.pr_id, &a.submitted_at).cmp(&(&b.pr_id, &b.submitted_at)));
            serde_json::to_value(sorted).unwrap()
        };
        assert_eq!(reviews(&new.reviews), reviews(&old.reviews));
        assert_eq!(new.reviews.len(), 2);

        let survival = |rows: &[RawSurvival]| {
            let mut sorted = rows.to_vec();
            sorted.sort_by(|a, b| a.pr_id.cmp(&b.pr_id));
            sorted
        };
        assert_eq!(survival(&new.survival), survival(&old.survival));
        assert!(new.survival.len() >= 3);

        let by_hash = |commits: &[RawCommit]| {
            let mut sorted = commits.to_vec();
            sorted.sort_by(|a, b| a.hash.cmp(&b.hash));
            serde_json::to_value(sorted).unwrap()
        };
        assert_eq!(by_hash(&new.commits), by_hash(&old.commits));
        assert!(new.commits.iter().any(|c| c.pr_id.is_some()));

        let mut reverted_new = new.reverted_pr_ids.clone();
        let mut reverted_old = old.reverted_pr_ids.clone();
        reverted_new.sort();
        reverted_old.sort();
        assert_eq!(reverted_new, reverted_old);
        assert_eq!(reverted_new, ["12"]);
        assert_eq!(new.gh_available, old.gh_available);
    }

    #[test]
    fn a_full_sync_without_github_stores_what_the_one_shot_collector_produces() {
        utc();
        let h = history();
        let (store, _) = setup(&h.repo);
        let now = super::super::now_epoch();
        let window = (at("2026-01-01T00:00:00Z"), now);
        full(&h, &store, &Unavailable, window);
        let new = load(&h, &store, window).raw;
        let old = super::super::collect_cached_with(
            &super::super::Cache::new(),
            &Unavailable,
            h.repo.path(),
            "2026-01-01T00:00:00Z",
            None,
            None,
            false,
        )
        .unwrap();
        let sorted = |mut prs: Vec<RawPr>| {
            prs.sort_by(|a, b| a.id.cmp(&b.id));
            serde_json::to_value(prs).unwrap()
        };
        assert_eq!(sorted(new.prs.clone()), sorted(old.prs.clone()));
        assert_eq!(
            new.prs.len(),
            6,
            "#7, #9, #12, the base, the direct push, the revert"
        );
        let mut a = new.survival.clone();
        let mut b = old.survival.clone();
        a.sort_by(|x, y| x.pr_id.cmp(&y.pr_id));
        b.sort_by(|x, y| x.pr_id.cmp(&y.pr_id));
        assert_eq!(a, b);
        let ids = |commits: &[RawCommit]| {
            let mut ids: Vec<(String, Option<String>)> = commits
                .iter()
                .map(|c| (c.hash.clone(), c.pr_id.clone()))
                .collect();
            ids.sort();
            ids
        };
        assert_eq!(ids(&new.commits), ids(&old.commits));
    }

    #[test]
    fn a_sync_does_not_hold_the_database_lock_while_github_runs() {
        utc();
        let h = history();
        let (store, _) = setup(&h.repo);
        let store = Arc::new(store);
        let probe = store.clone();
        let github = FakeGh::new(move |args| {
            // Another thread of the app writes meanwhile: the lock is free.
            assert!(probe.lock_conn().is_ok());
            match args[0] {
                "auth" => Ok("ok".to_string()),
                "repo" => Ok(json!({
                    "nameWithOwner": "acme/widgets",
                    "url": "https://github.com/acme/widgets"
                })
                .to_string()),
                _ => Ok("[]".to_string()),
            }
        });
        full(&h, &store, &github, jan_feb());
        assert!(!github.calls.lock().unwrap().is_empty());
    }

    #[test]
    fn months_follow_the_calendar_across_a_dst_change() {
        // Berlin in 2026: UTC+1, UTC+2 from March 29th 01:00Z to October 25th 01:00Z.
        let berlin = |epoch: i64| {
            if epoch >= at("2026-03-29T01:00:00Z") && epoch < at("2026-10-25T01:00:00Z") {
                7_200
            } else {
                3_600
            }
        };
        let from = at("2026-03-10T12:00:00Z");
        let to = at("2026-05-15T08:00:00Z");
        let chunks = month_chunks_in(from, to, &berlin);
        assert_eq!(
            chunks,
            [
                (from, at("2026-03-31T22:00:00Z")),
                (at("2026-03-31T22:00:00Z"), at("2026-04-30T22:00:00Z")),
                (at("2026-04-30T22:00:00Z"), to),
            ]
        );
        // The chunk that ends the DST period.
        let autumn = month_chunks_in(
            at("2026-10-20T00:00:00Z"),
            at("2026-11-20T00:00:00Z"),
            &berlin,
        );
        assert_eq!(autumn[0].1, at("2026-10-31T23:00:00Z"));
    }

    #[test]
    fn months_roll_over_a_year_end_and_cover_the_range_once() {
        let zone = |_: i64| -5 * 3_600;
        let from = at("2026-12-15T00:00:00Z");
        let to = at("2027-02-10T00:00:00Z");
        let chunks = month_chunks_in(from, to, &zone);
        assert_eq!(
            chunks,
            [
                (from, at("2027-01-01T05:00:00Z")),
                (at("2027-01-01T05:00:00Z"), at("2027-02-01T05:00:00Z")),
                (at("2027-02-01T05:00:00Z"), to),
            ]
        );
        assert!(month_chunks_in(to, from, &zone).is_empty());
        assert!(month_chunks_in(from, from, &zone).is_empty());
    }

    #[test]
    fn the_local_months_cover_a_range_without_gaps() {
        let (from, to) = (at("2026-01-20T08:00:00Z"), at("2027-01-05T08:00:00Z"));
        let chunks = month_chunks(from, to);
        assert_eq!(chunks.first().unwrap().0, from);
        assert_eq!(chunks.last().unwrap().1, to);
        assert!(chunks.windows(2).all(|w| w[0].1 == w[1].0));
        assert!(chunks.iter().all(|(a, b)| a < b));
        assert!(chunks.len() == 12 || chunks.len() == 13);
    }

    // -----------------------------------------------------------------------
    // fix round 1
    // -----------------------------------------------------------------------

    fn git_date(epoch: i64) -> String {
        format!("{epoch} +0000")
    }

    #[test]
    fn a_full_sync_after_a_force_push_makes_the_period_covered_again() {
        utc();
        let h = history();
        let (store, _) = setup(&h.repo);
        full(&h, &store, &hub(history_prs(&h), vec![], None), jan_feb());
        assert!(load(&h, &store, jan_feb()).covered);

        let when = "2026-02-15T10:00:00+0000";
        h.repo
            .run(&["reset", "-q", "--hard", "HEAD~1"], ALICE, when);
        h.repo
            .commit(&[("other.txt", "o\n")], "Replacement push", ALICE, when);
        let stale = load(&h, &store, jan_feb());
        assert!(!stale.covered);
        assert_eq!(stale.missing_commits, 1);

        let report = Recorder::new();
        run_with(
            &h,
            &store,
            &hub(history_prs(&h), vec![], None),
            SyncMode::Full,
            jan_feb(),
            &report,
        );
        assert_eq!(
            events_of(&report, SyncStep::Commits).last().copied(),
            Some((1, 1)),
            "only the commit the store lacks"
        );
        let loaded = load(&h, &store, jan_feb());
        assert!(loaded.covered);
        let head = head_of(&h.repo);
        assert!(loaded
            .raw
            .prs
            .iter()
            .any(|pr| pr.merge_commit.as_deref() == Some(head.as_str())));
        assert_eq!(coverage_rows(&h, &store).len(), 1);
    }

    #[test]
    fn a_merge_that_brings_in_commits_dated_before_the_last_sync_is_repaired() {
        utc();
        let h = history();
        let (store, _) = setup(&h.repo);
        let january = (at("2026-01-01T00:00:00Z"), at("2026-01-31T23:59:59Z"));
        full(&h, &store, &hub(history_prs(&h), vec![], None), january);

        // Later, a PR merges whose commits were written in January.
        let when = "2026-01-25T10:00:00+0000";
        h.repo
            .run(&["checkout", "-q", "-b", "old", &h.squash7], ALICE, when);
        h.repo.commit(&[("old.txt", "x\n")], "old work", BOB, when);
        h.repo.run(&["checkout", "-q", "main"], ALICE, when);
        let merged = "2026-02-20T10:00:00+0000";
        h.repo.run(
            &[
                "merge",
                "--no-ff",
                "-q",
                "-m",
                "Merge pull request #20 from bob/old",
                "old",
            ],
            ALICE,
            merged,
        );
        let merge20 = head_of(&h.repo);
        let mut prs = history_prs(&h);
        prs.push(gh_merged(
            20,
            "bob",
            "2026-02-20T10:00:00Z",
            &merge20,
            json!([]),
        ));

        let report = Recorder::new();
        let github = hub(prs, vec![], None);
        run_with(&h, &store, &github, SyncMode::Full, jan_feb(), &report);
        let loaded = load(&h, &store, jan_feb());
        assert!(loaded.covered, "missing {}", loaded.missing_commits);
        assert!(loaded.raw.commits.iter().any(|c| c.subject == "old work"));
        assert!(loaded.raw.prs.iter().any(|p| p.id == "20"));
        // January's commit and PR list were looked at again.
        assert!(merged_searches(&github)
            .iter()
            .any(|s| s.starts_with("merged:2026-01-01")));
        // Neither of the two top-ups the page would offer is needed.
        assert_eq!(
            loaded.coverage_head.as_deref(),
            Some(head_of(&h.repo).as_str())
        );
    }

    #[test]
    fn a_sync_never_covers_time_that_has_not_happened() {
        utc();
        let h = history();
        let (store, _) = setup(&h.repo);
        let from = at("2026-01-01T00:00:00Z");
        let period_end = super::super::now_epoch() + 3_600;
        full(&h, &store, &Unavailable, (from, period_end));
        let row = coverage_rows(&h, &store).remove(0);
        assert!(row.to_epoch <= super::super::now_epoch(), "{row:?}");
        assert!(row.to_epoch < period_end);
        assert!(!load(&h, &store, (from, period_end)).covered);

        // A commit just after the covered time: the page must see it missing.
        let late = row.to_epoch + 1;
        let date = git_date(late);
        h.repo
            .commit(&[("late.txt", "x\n")], "Late push", ALICE, &date);
        let window = (from, late + 1);
        let seen = load(&h, &store, window);
        assert!(!seen.covered);
        assert_eq!(seen.missing_commits, 1);

        while super::super::now_epoch() < window.1 {
            std::thread::sleep(Duration::from_millis(100));
        }
        full(&h, &store, &Unavailable, window);
        assert!(load(&h, &store, window).covered);
    }

    #[test]
    fn a_cancel_during_the_pull_request_step_stops_at_once_and_covers_nothing() {
        utc();
        let h = history();
        let (store, _) = setup(&h.repo);
        let started = Instant::now();
        let stopper = StopAt::new(SyncStep::PullRequests, 1);
        let outcome = run_with(
            &h,
            &store,
            &hub(history_prs(&h), vec![], None),
            SyncMode::Full,
            jan_feb(),
            &stopper,
        );
        assert!(outcome.cancelled);
        assert_ne!(outcome.gh_state, "complete");
        assert!(outcome.gh_note.is_some());
        assert!(started.elapsed() < Duration::from_secs(10));
        assert!(coverage_rows(&h, &store).is_empty());
        let github_ran = stopper
            .events
            .lock()
            .unwrap()
            .iter()
            .any(|e| e.0 == SyncStep::Github);
        assert!(!github_ran);
    }

    #[test]
    fn the_merge_walk_and_the_survival_blame_obey_a_cancel_check() {
        let h = history();
        let root = h.repo.path();
        let head = head_of(&h.repo);
        let derived = super::super::derive_prs_known(
            root,
            &head,
            &Window::from_epochs(at("2026-01-01T00:00:00Z"), at("2026-03-01T00:00:00Z")),
            HashMap::new(),
            &HashSet::new(),
            &Limits::DEFAULT,
            HashSet::new(),
            Some(&|| true),
            &mut false,
        );
        assert_eq!(derived.err().as_deref(), Some(pr_heads::CANCELLED));

        let candidate = super::super::SurvivalCandidate {
            pr_id: "7".to_string(),
            landing: None,
            merged_epoch: at("2026-01-10T10:00:00Z"),
            hashes: HashSet::from([h.squash7.clone()]),
            files: vec![super::super::RawFile {
                path: "parser.txt".to_string(),
                added: 3,
                deleted: 0,
            }],
        };
        let (samples, _) = super::super::compute_survival(
            root,
            &head,
            vec![candidate],
            super::super::now_epoch(),
            &Limits::DEFAULT,
            Some(&|| true),
        );
        assert!(samples.is_empty(), "no file is blamed once cancelled");
    }

    #[test]
    fn a_github_retry_derives_and_stamps_the_head_its_row_was_synced_at() {
        utc();
        let h = history();
        let (store, _) = setup(&h.repo);
        let synced_head = head_of(&h.repo);
        full(&h, &store, &Unavailable, jan_feb());
        h.repo.commit(
            &[("later.txt", "x\n")],
            "Later push",
            ALICE,
            "2026-02-20T10:00:00+0000",
        );
        assert_ne!(head_of(&h.repo), synced_head);

        let outcome = run_with(
            &h,
            &store,
            &hub(history_prs(&h), vec![], None),
            SyncMode::GithubRetry,
            jan_feb(),
            &Recorder::new(),
        );
        assert_eq!(outcome.gh_state, "complete");
        let rows = coverage_rows(&h, &store);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].head, synced_head);
        assert_eq!(rows[0].gh_state, "complete");
        let stored = store
            .lock_conn()
            .unwrap()
            .query_row(
                "SELECT COUNT(*) FROM contrib_commits WHERE subject = 'Later push'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap();
        assert_eq!(stored, 0, "the retry did not move to the new head");
    }

    #[test]
    fn a_cut_short_survival_is_said_in_the_note() {
        assert_eq!(compose_note(None, false), None);
        assert_eq!(
            compose_note(Some("gh said no".to_string()), false).as_deref(),
            Some("gh said no")
        );
        assert_eq!(compose_note(None, true).as_deref(), Some(SURVIVAL_CUT_NOTE));
        let both = compose_note(Some("gh said no".to_string()), true).unwrap();
        assert!(both.starts_with("gh said no") && both.ends_with(SURVIVAL_CUT_NOTE));
    }

    #[test]
    fn a_stopped_run_never_reports_a_complete_state() {
        assert_eq!(cancelled("complete", None).gh_state, "partial");
        assert!(cancelled("complete", None).gh_note.is_some());
        assert_eq!(cancelled("unavailable", None).gh_state, "unavailable");
        assert!(cancelled("partial", None).cancelled);
    }

    #[test]
    fn the_parts_of_a_period_outside_some_gaps() {
        assert_eq!(outside(0, 100, &[]), [(0, 100)]);
        assert_eq!(outside(0, 100, &[(0, 10), (50, 60)]), [(11, 49), (61, 100)]);
        assert_eq!(outside(0, 100, &[(90, 100)]), [(0, 89)]);
        assert!(outside(0, 100, &[(0, 100)]).is_empty());
    }
}
