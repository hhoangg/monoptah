//! Sync engine of the Contributors page: what a sync would cost, and the
//! batched ingestion of commits into the store.
//!
//! Everything here runs only because the user pressed Sync. Git and GitHub work
//! happens with no database lock held; the write lock is taken for one batch at
//! a time, in one transaction, so a cancelled or crashed job leaves whole
//! batches behind and the next run continues with what is missing.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::time::Duration;

use serde::Serialize;

use super::store::{self, StoredCommit};
use super::{gh, pr_heads, split_records, CancelCheck, Limits, PatchPipe, RawCommit};
use crate::session_store::SessionStore;

/// Commits read and written per transaction.
pub(super) const COMMIT_BATCH: usize = 500;

/// Hard stop for one local git call over a batch.
const BATCH_TIMEOUT: Duration = Duration::from_secs(300);
/// Hard stop for the GitHub PR count.
const SEARCH_TIMEOUT: Duration = Duration::from_secs(30);
/// Wall clock for the whole GitHub part of an estimate.
const ESTIMATE_BUDGET: Duration = Duration::from_secs(60);

/// The steps after `Commits` are reported by the later parts of the sync engine.
#[allow(dead_code)]
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

#[cfg(test)]
mod tests {
    use std::io::Write;
    use std::process::{Command, Stdio};
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::Instant;

    use super::super::gh::{FakeGh, GhError, Unavailable};
    use super::super::tests::{TestRepo, ALICE};
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
}
