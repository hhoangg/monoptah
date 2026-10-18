//! SQLite store for the Contributors page.
//!
//! Sync (see the design doc) writes processed commits, PRs, reviews, survival
//! samples and the synced intervals here; `load` assembles a period back into
//! the `RawData` the TypeScript model consumes. Callers take the app database
//! lock only for these short calls and never across git or network work.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use rusqlite::{params, Connection, OptionalExtension, ToSql, Transaction};

use super::{
    epoch_to_iso, git, now_epoch, parse_iso_epoch, pr_heads, select_duplicates, CancelCheck,
    GitIdentity, RawCommit, RawData, RawFile, RawPr, RawReview, RawSurvival,
};

/// How many `?` placeholders one `IN (...)` list may hold.
pub(super) const CHUNK: usize = 400;

/// Hard stop for a hash listing that a cancel check can stop.
const LIST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(300);

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS contrib_repos (
  id INTEGER PRIMARY KEY,
  git_dir TEXT NOT NULL UNIQUE,
  slug TEXT
);
CREATE TABLE IF NOT EXISTS contrib_commits (
  repo_id INTEGER NOT NULL,
  hash TEXT NOT NULL,
  subject TEXT NOT NULL,
  author_json TEXT NOT NULL,
  committer_json TEXT NOT NULL,
  authored_at TEXT NOT NULL,
  committed_at TEXT NOT NULL,
  committed_epoch INTEGER NOT NULL,
  co_authors_json TEXT NOT NULL,
  files_json TEXT NOT NULL,
  patch_id TEXT,
  reverts TEXT,
  oversized INTEGER NOT NULL DEFAULT 0,
  PRIMARY KEY (repo_id, hash)
);
CREATE INDEX IF NOT EXISTS contrib_commits_epoch_idx
  ON contrib_commits (repo_id, committed_epoch);
CREATE TABLE IF NOT EXISTS contrib_prs (
  repo_id INTEGER NOT NULL,
  branch TEXT NOT NULL,
  pr_id TEXT NOT NULL,
  number INTEGER,
  state TEXT NOT NULL,
  source TEXT NOT NULL,
  merged_epoch INTEGER,
  closed_epoch INTEGER,
  merge_commit TEXT,
  pr_json TEXT NOT NULL,
  PRIMARY KEY (repo_id, branch, pr_id)
);
CREATE INDEX IF NOT EXISTS contrib_prs_merged_idx
  ON contrib_prs (repo_id, branch, merged_epoch);
CREATE INDEX IF NOT EXISTS contrib_prs_closed_idx
  ON contrib_prs (repo_id, branch, closed_epoch);
CREATE TABLE IF NOT EXISTS contrib_commit_prs (
  repo_id INTEGER NOT NULL,
  branch TEXT NOT NULL,
  hash TEXT NOT NULL,
  pr_id TEXT NOT NULL,
  PRIMARY KEY (repo_id, branch, hash)
);
CREATE INDEX IF NOT EXISTS contrib_commit_prs_pr_idx
  ON contrib_commit_prs (repo_id, branch, pr_id);
CREATE TABLE IF NOT EXISTS contrib_reviews (
  repo_id INTEGER NOT NULL,
  branch TEXT NOT NULL,
  pr_id TEXT NOT NULL,
  ordinal INTEGER NOT NULL,
  submitted_epoch INTEGER NOT NULL,
  review_json TEXT NOT NULL,
  PRIMARY KEY (repo_id, branch, pr_id, ordinal)
);
CREATE INDEX IF NOT EXISTS contrib_reviews_submitted_idx
  ON contrib_reviews (repo_id, branch, submitted_epoch);
CREATE TABLE IF NOT EXISTS contrib_survival (
  repo_id INTEGER NOT NULL,
  branch TEXT NOT NULL,
  pr_id TEXT NOT NULL,
  written INTEGER NOT NULL,
  alive INTEGER NOT NULL,
  head TEXT NOT NULL,
  computed_at INTEGER NOT NULL,
  PRIMARY KEY (repo_id, branch, pr_id)
);
CREATE TABLE IF NOT EXISTS contrib_coverage (
  repo_id INTEGER NOT NULL,
  branch TEXT NOT NULL,
  from_epoch INTEGER NOT NULL,
  to_epoch INTEGER NOT NULL,
  head TEXT NOT NULL,
  synced_at INTEGER NOT NULL,
  gh_state TEXT NOT NULL,
  gh_note TEXT,
  PRIMARY KEY (repo_id, branch, from_epoch)
);
";

/// Creates the Contributors tables. Safe to run on every start.
pub(crate) fn ensure_tables(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(SCHEMA)?;
    super::ticket_store::ensure_schema(conn)
}

/// A commit as sync stores it. `commit.pr_id` stays `None`: the commit to PR
/// link is per branch and lives in `contrib_commit_prs`.
#[derive(Clone, Debug)]
pub(super) struct StoredCommit {
    pub commit: RawCommit,
    pub patch_id: Option<String>,
    /// The commit a revert commit names.
    pub reverts: Option<String>,
    /// The file list exceeded the per-commit cap and was stored without files.
    pub oversized: bool,
}

/// One synced interval of a branch. Both edges are inclusive epoch seconds.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct CoverageRow {
    pub from_epoch: i64,
    pub to_epoch: i64,
    pub head: String,
    pub synced_at: i64,
    /// `complete | partial | unavailable`
    pub gh_state: String,
    pub gh_note: Option<String>,
}

/// A period read back from the store.
#[derive(Clone, Debug)]
pub(super) struct Loaded {
    pub raw: RawData,
    pub covered: bool,
    /// Part of the window was never synced (or its sync was cancelled), as
    /// opposed to a synced window that only lacks some commits.
    pub has_gaps: bool,
    pub missing_commits: u64,
    pub synced_at: Option<i64>,
    pub coverage_head: Option<String>,
    pub gh_state: Option<String>,
}

pub(super) fn sql(error: rusqlite::Error) -> String {
    error.to_string()
}

pub(super) fn json<T: serde::Serialize>(value: &T) -> Result<String, String> {
    serde_json::to_string(value).map_err(|e| e.to_string())
}

pub(super) fn unjson<T: serde::de::DeserializeOwned>(text: &str) -> Result<T, String> {
    serde_json::from_str(text).map_err(|e| format!("Corrupt Contributors data: {e}"))
}

/// Runs `query` once per chunk of `items`. `query` holds `{in}` where the
/// `IN (...)` placeholders go; `lead` are the parameters before them.
pub(super) fn query_in<T>(
    conn: &Connection,
    query: &str,
    lead: &[&dyn ToSql],
    items: &[String],
    mut map: impl FnMut(&rusqlite::Row) -> rusqlite::Result<T>,
) -> Result<Vec<T>, String> {
    let mut out = Vec::new();
    for chunk in items.chunks(CHUNK) {
        let marks = vec!["?"; chunk.len()].join(",");
        let mut stmt = conn.prepare(&query.replace("{in}", &marks)).map_err(sql)?;
        let args: Vec<&dyn ToSql> = lead
            .iter()
            .copied()
            .chain(chunk.iter().map(|item| item as &dyn ToSql))
            .collect();
        let rows = stmt.query_map(args.as_slice(), &mut map).map_err(sql)?;
        for row in rows {
            out.push(row.map_err(sql)?);
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// repositories
// ---------------------------------------------------------------------------

/// The absolute, canonical `git rev-parse --git-common-dir`: every worktree of
/// one repository answers the same path.
pub(crate) fn git_common_dir(root: &Path) -> Result<String, String> {
    let out = git(root, ["rev-parse", "--git-common-dir"])
        .map_err(|_| "Not a git repository".to_string())?;
    let line = out.trim_end_matches('\n');
    if line.trim().is_empty() {
        return Err("Not a git repository".to_string());
    }
    let path = Path::new(line);
    let absolute: PathBuf = if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    };
    let canonical = absolute.canonicalize().unwrap_or(absolute);
    Ok(canonical.to_string_lossy().into_owned())
}

/// Id of the repository, created on first use. A known slug is only replaced
/// by another one, never cleared.
pub(super) fn repo_id(conn: &Connection, git_dir: &str, slug: Option<&str>) -> Result<i64, String> {
    conn.execute(
        "INSERT OR IGNORE INTO contrib_repos (git_dir, slug) VALUES (?1, ?2)",
        params![git_dir, slug],
    )
    .map_err(sql)?;
    if let Some(slug) = slug {
        conn.execute(
            "UPDATE contrib_repos SET slug = ?2 WHERE git_dir = ?1 AND slug IS NOT ?2",
            params![git_dir, slug],
        )
        .map_err(sql)?;
    }
    conn.query_row(
        "SELECT id FROM contrib_repos WHERE git_dir = ?1",
        [git_dir],
        |row| row.get(0),
    )
    .map_err(sql)
}

pub(super) fn find_repo_id(conn: &Connection, git_dir: &str) -> Result<Option<i64>, String> {
    conn.query_row(
        "SELECT id FROM contrib_repos WHERE git_dir = ?1",
        [git_dir],
        |row| row.get(0),
    )
    .optional()
    .map_err(sql)
}

/// Deletes everything synced for a repository. The repo row stays so an id
/// held by a running job is never handed to another repository.
pub(super) fn clear_repo(conn: &Connection, git_dir: &str) -> Result<(), String> {
    let Some(repo) = find_repo_id(conn, git_dir)? else {
        return Ok(());
    };
    let tx = conn.unchecked_transaction().map_err(sql)?;
    for table in [
        "contrib_commits",
        "contrib_prs",
        "contrib_commit_prs",
        "contrib_reviews",
        "contrib_survival",
        "contrib_coverage",
    ] {
        tx.execute(&format!("DELETE FROM {table} WHERE repo_id = ?1"), [repo])
            .map_err(sql)?;
    }
    tx.commit().map_err(sql)
}

// ---------------------------------------------------------------------------
// writes
// ---------------------------------------------------------------------------

/// Which of `hashes` are stored.
pub(super) fn stored_hashes(
    conn: &Connection,
    repo: i64,
    hashes: &[String],
) -> Result<HashSet<String>, String> {
    let found = query_in(
        conn,
        "SELECT hash FROM contrib_commits WHERE repo_id = ? AND hash IN ({in})",
        &[&repo],
        hashes,
        |row| row.get::<_, String>(0),
    )?;
    Ok(found.into_iter().collect())
}

/// Stores commits that are not stored yet; a commit is immutable.
pub(super) fn insert_commits(
    tx: &Transaction,
    repo: i64,
    commits: &[StoredCommit],
) -> Result<(), String> {
    let mut stmt = tx
        .prepare(
            "INSERT OR IGNORE INTO contrib_commits
               (repo_id, hash, subject, author_json, committer_json, authored_at,
                committed_at, committed_epoch, co_authors_json, files_json, patch_id,
                reverts, oversized)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
        )
        .map_err(sql)?;
    for stored in commits {
        let c = &stored.commit;
        stmt.execute(params![
            repo,
            c.hash,
            c.subject,
            json(&c.author)?,
            json(&c.committer)?,
            c.authored_at,
            c.committed_at,
            parse_iso_epoch(&c.committed_at).unwrap_or(0),
            json(&c.co_authors)?,
            json(&c.files)?,
            stored.patch_id,
            stored.reverts,
            stored.oversized,
        ])
        .map_err(sql)?;
    }
    Ok(())
}

/// Replaces each PR by id, together with the list of commits that landed in it.
pub(super) fn put_prs(
    tx: &Transaction,
    repo: i64,
    branch: &str,
    prs: &[(RawPr, Vec<String>)],
) -> Result<(), String> {
    for (pr, hashes) in prs {
        tx.execute(
            "DELETE FROM contrib_commit_prs WHERE repo_id = ?1 AND branch = ?2 AND pr_id = ?3",
            params![repo, branch, pr.id],
        )
        .map_err(sql)?;
        tx.execute(
            "INSERT OR REPLACE INTO contrib_prs
               (repo_id, branch, pr_id, number, state, source, merged_epoch, closed_epoch,
                merge_commit, pr_json)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                repo,
                branch,
                pr.id,
                pr.number.map(|n| n as i64),
                pr.state,
                pr.source,
                pr.merged_at.as_deref().and_then(parse_iso_epoch),
                pr.closed_at.as_deref().and_then(parse_iso_epoch),
                pr.merge_commit,
                json(pr)?,
            ],
        )
        .map_err(sql)?;
        let mut link = tx
            .prepare(
                "INSERT OR REPLACE INTO contrib_commit_prs (repo_id, branch, hash, pr_id)
                 VALUES (?1, ?2, ?3, ?4)",
            )
            .map_err(sql)?;
        for hash in hashes {
            link.execute(params![repo, branch, hash, pr.id])
                .map_err(sql)?;
        }
    }
    Ok(())
}

/// Replaces every review of the PRs named in `reviews`.
pub(super) fn put_reviews(
    tx: &Transaction,
    repo: i64,
    branch: &str,
    reviews: &[RawReview],
) -> Result<(), String> {
    let mut next: HashMap<&str, i64> = HashMap::new();
    for review in reviews {
        if !next.contains_key(review.pr_id.as_str()) {
            tx.execute(
                "DELETE FROM contrib_reviews WHERE repo_id = ?1 AND branch = ?2 AND pr_id = ?3",
                params![repo, branch, review.pr_id],
            )
            .map_err(sql)?;
        }
        let ordinal = next.entry(review.pr_id.as_str()).or_insert(0);
        tx.execute(
            "INSERT OR REPLACE INTO contrib_reviews
               (repo_id, branch, pr_id, ordinal, submitted_epoch, review_json)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                repo,
                branch,
                review.pr_id,
                *ordinal,
                parse_iso_epoch(&review.submitted_at).unwrap_or(0),
                json(review)?,
            ],
        )
        .map_err(sql)?;
        *ordinal += 1;
    }
    Ok(())
}

pub(super) fn put_survival(
    tx: &Transaction,
    repo: i64,
    branch: &str,
    head: &str,
    rows: &[RawSurvival],
) -> Result<(), String> {
    let now = now_epoch();
    let mut stmt = tx
        .prepare(
            "INSERT OR REPLACE INTO contrib_survival
               (repo_id, branch, pr_id, written, alive, head, computed_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        )
        .map_err(sql)?;
    for row in rows {
        stmt.execute(params![
            repo,
            branch,
            row.pr_id,
            row.written as i64,
            row.alive as i64,
            head,
            now
        ])
        .map_err(sql)?;
    }
    Ok(())
}

/// Deletes the PRs `ids` of a branch with their commit links and reviews.
/// Survival rows stay: they are keyed by PR id and a PR that is written again
/// under the same id keeps its sample.
fn delete_prs(tx: &Transaction, repo: i64, branch: &str, ids: &[String]) -> Result<(), String> {
    for id in ids {
        for table in ["contrib_commit_prs", "contrib_reviews", "contrib_prs"] {
            tx.execute(
                &format!("DELETE FROM {table} WHERE repo_id = ?1 AND branch = ?2 AND pr_id = ?3"),
                params![repo, branch, id],
            )
            .map_err(sql)?;
        }
    }
    Ok(())
}

/// Deletes the PRs of a branch merged or closed within `from..=to`: the stored
/// side of writing that stretch of time again.
pub(super) fn clear_prs_in(
    tx: &Transaction,
    repo: i64,
    branch: &str,
    from: i64,
    to: i64,
) -> Result<(), String> {
    let ids: Vec<String> = {
        let mut stmt = tx
            .prepare(
                "SELECT pr_id FROM contrib_prs
                 WHERE repo_id = ?1 AND branch = ?2
                   AND ((merged_epoch BETWEEN ?3 AND ?4) OR (closed_epoch BETWEEN ?3 AND ?4))",
            )
            .map_err(sql)?;
        let rows = stmt
            .query_map(params![repo, branch, from, to], |row| row.get(0))
            .map_err(sql)?;
        rows.map(|row| row.map_err(sql)).collect::<Result<_, _>>()?
    };
    delete_prs(tx, repo, branch, &ids)
}

/// Deletes the stored PRs, other than `keep`, whose merge commit is one of
/// `merge_commits`: a GitHub PR supersedes the PR git derived for the same
/// landing, also when the two fell on either side of a chunk boundary.
pub(super) fn drop_prs_landing_in(
    tx: &Transaction,
    repo: i64,
    branch: &str,
    merge_commits: &[String],
    keep: &HashSet<String>,
) -> Result<(), String> {
    let found = query_in(
        tx,
        "SELECT pr_id FROM contrib_prs
         WHERE repo_id = ? AND branch = ? AND merge_commit IN ({in})",
        &[&repo, &branch],
        merge_commits,
        |row| row.get::<_, String>(0),
    )?;
    let stale: Vec<String> = found.into_iter().filter(|id| !keep.contains(id)).collect();
    delete_prs(tx, repo, branch, &stale)
}

// ---------------------------------------------------------------------------
// coverage
// ---------------------------------------------------------------------------

pub(super) fn coverage(
    conn: &Connection,
    repo: i64,
    branch: &str,
) -> Result<Vec<CoverageRow>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT from_epoch, to_epoch, head, synced_at, gh_state, gh_note
             FROM contrib_coverage WHERE repo_id = ?1 AND branch = ?2
             ORDER BY from_epoch",
        )
        .map_err(sql)?;
    let rows = stmt
        .query_map(params![repo, branch], |row| {
            Ok(CoverageRow {
                from_epoch: row.get(0)?,
                to_epoch: row.get(1)?,
                head: row.get(2)?,
                synced_at: row.get(3)?,
                gh_state: row.get(4)?,
                gh_note: row.get(5)?,
            })
        })
        .map_err(sql)?;
    rows.map(|row| row.map_err(sql)).collect()
}

/// Rank of a GitHub state: the higher, the less complete the data.
fn gh_rank(state: &str) -> u8 {
    match state {
        "unavailable" => 2,
        "partial" => 1,
        _ => 0,
    }
}

/// Adds an interval. Stored intervals that overlap or touch it and carry the
/// same GitHub state are folded in (the one that ends last supplies head, time
/// and note, so the live edge of the row stays with its sync; the newer row wins
/// a tie). Intervals with another state
/// are never merged, so a window synced without GitHub keeps saying so: where
/// the new interval overlaps one of them, that one is cut back to make room.
pub(super) fn add_coverage(
    tx: &Transaction,
    repo: i64,
    branch: &str,
    row: &CoverageRow,
) -> Result<(), String> {
    let mut merged = row.clone();
    let mut remainders: Vec<CoverageRow> = Vec::new();
    for other in coverage(tx, repo, branch)? {
        let overlaps = other.from_epoch <= row.to_epoch && other.to_epoch >= row.from_epoch;
        let touches = other.from_epoch <= row.to_epoch.saturating_add(1)
            && other.to_epoch.saturating_add(1) >= row.from_epoch;
        let same_state = other.gh_state == row.gh_state;
        if !(overlaps || (touches && same_state)) {
            continue;
        }
        tx.execute(
            "DELETE FROM contrib_coverage
             WHERE repo_id = ?1 AND branch = ?2 AND from_epoch = ?3",
            params![repo, branch, other.from_epoch],
        )
        .map_err(sql)?;
        if same_state {
            let (from, to) = (
                merged.from_epoch.min(other.from_epoch),
                merged.to_epoch.max(other.to_epoch),
            );
            // The merged row keeps the sync that reaches furthest (the newer
            // one on a tie): its moment and head say how far the row is known
            // to be current. Taking the newest moment alone would pair it
            // with another row's older end and lose a live edge.
            if other.to_epoch > merged.to_epoch
                || (other.to_epoch == merged.to_epoch && other.synced_at > merged.synced_at)
            {
                merged = other;
            }
            merged.from_epoch = from;
            merged.to_epoch = to;
        } else {
            if other.from_epoch < row.from_epoch {
                remainders.push(CoverageRow {
                    to_epoch: row.from_epoch - 1,
                    ..other.clone()
                });
            }
            if other.to_epoch > row.to_epoch {
                remainders.push(CoverageRow {
                    from_epoch: row.to_epoch + 1,
                    ..other
                });
            }
        }
    }
    for interval in remainders.iter().chain(std::iter::once(&merged)) {
        tx.execute(
            "INSERT INTO contrib_coverage
               (repo_id, branch, from_epoch, to_epoch, head, synced_at, gh_state, gh_note)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                repo,
                branch,
                interval.from_epoch,
                interval.to_epoch,
                interval.head,
                interval.synced_at,
                interval.gh_state,
                interval.gh_note
            ],
        )
        .map_err(sql)?;
    }
    Ok(())
}

/// The parts of `from..=to` that no row covers, as inclusive intervals.
pub(super) fn missing_intervals(rows: &[CoverageRow], from: i64, to: i64) -> Vec<(i64, i64)> {
    let mut sorted: Vec<&CoverageRow> = rows.iter().collect();
    sorted.sort_by_key(|r| r.from_epoch);
    let mut gaps = Vec::new();
    let mut cursor = from;
    for row in sorted {
        if cursor > to {
            break;
        }
        if row.to_epoch < cursor {
            continue;
        }
        if row.from_epoch > to {
            break;
        }
        if row.from_epoch > cursor {
            gaps.push((cursor, row.from_epoch - 1));
        }
        cursor = cursor.max(row.to_epoch.saturating_add(1));
    }
    if cursor <= to {
        gaps.push((cursor, to));
    }
    gaps
}

// ---------------------------------------------------------------------------
// read path
// ---------------------------------------------------------------------------

type CommitColumns = (
    String,
    String,
    String,
    String,
    String,
    String,
    String,
    String,
    Option<String>,
    Option<String>,
    bool,
);

/// What git says about a window, gathered without touching the database.
#[derive(Clone, Debug)]
pub(super) struct WindowCommits {
    /// Non-merge commits of the window, `--date-order`.
    pub hashes: Vec<String>,
    /// Every commit of the window, merges included: what a PR's merge commit
    /// must be part of for the PR to still belong to the branch.
    pub reachable: HashSet<String>,
}

/// Hashes `git rev-list <flags>` prints for `since..=until` of `head`.
pub(super) fn listed_hashes(
    root: &Path,
    head: &str,
    since: i64,
    until: i64,
    flags: &[&str],
) -> Result<Vec<String>, String> {
    listed_hashes_cancellable(root, head, since, until, flags, None)
}

/// `listed_hashes` that a cancel check can stop: git is killed and the error
/// is `pr_heads::CANCELLED`.
pub(super) fn listed_hashes_cancellable(
    root: &Path,
    head: &str,
    since: i64,
    until: i64,
    flags: &[&str],
    cancel: Option<CancelCheck>,
) -> Result<Vec<String>, String> {
    let since_arg = format!("--since=@{since}");
    let until_arg = format!("--until=@{until}");
    let mut args = vec!["rev-list"];
    args.extend(flags);
    args.extend([since_arg.as_str(), until_arg.as_str(), head, "--"]);
    let out = match cancel {
        Some(cancel) => pr_heads::run_git(root, &args, None, LIST_TIMEOUT, &[], Some(cancel))?,
        None => git(root, args)?,
    };
    Ok(out
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect())
}

/// The stored commits among `hashes`, as sync's merge graph walk needs them
/// (no message body is stored, so none is returned).
pub(super) fn stored_commits(
    conn: &Connection,
    repo: i64,
    hashes: &[String],
) -> Result<Vec<RawCommit>, String> {
    let rows = query_in(
        conn,
        "SELECT hash, subject, author_json, committer_json, authored_at, committed_at,
                co_authors_json, files_json
         FROM contrib_commits WHERE repo_id = ? AND hash IN ({in})",
        &[&repo],
        hashes,
        |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, String>(6)?,
                row.get::<_, String>(7)?,
            ))
        },
    )?;
    rows.into_iter()
        .map(|row| {
            Ok(RawCommit {
                pr_id: None,
                hash: row.0,
                subject: row.1,
                author: unjson::<GitIdentity>(&row.2)?,
                committer: unjson::<GitIdentity>(&row.3)?,
                authored_at: row.4,
                committed_at: row.5,
                co_authors: unjson::<Vec<GitIdentity>>(&row.6)?,
                files: unjson::<Vec<RawFile>>(&row.7)?,
            })
        })
        .collect()
}

/// The commits among `hashes` (newest first, as listed) that repeat the patch
/// of an earlier commit and so count for nothing: the rule `load_window` applies.
pub(super) fn dropped_duplicates(
    conn: &Connection,
    repo: i64,
    hashes: &[String],
) -> Result<HashSet<String>, String> {
    let rows: HashMap<String, (String, Option<String>)> = query_in(
        conn,
        "SELECT hash, committed_at, patch_id FROM contrib_commits
         WHERE repo_id = ? AND hash IN ({in})",
        &[&repo],
        hashes,
        |row| {
            Ok((
                row.get::<_, String>(0)?,
                (row.get::<_, String>(1)?, row.get::<_, Option<String>>(2)?),
            ))
        },
    )?
    .into_iter()
    .collect();
    let mut pairs: Vec<(String, String)> = Vec::new();
    let mut committed: HashMap<String, i64> = HashMap::new();
    for hash in hashes {
        let Some((at, patch)) = rows.get(hash) else {
            continue;
        };
        if let Some(epoch) = parse_iso_epoch(at) {
            committed.insert(hash.clone(), epoch);
        }
        if let Some(patch) = patch {
            pairs.push((patch.clone(), hash.clone()));
        }
    }
    Ok(select_duplicates(&pairs, &committed))
}

/// Lists the commits of `since..=until` reachable from `head`. Runs git only:
/// call it with no database lock held, then pass the result to `load_window`.
pub(super) fn window_commits(
    root: &Path,
    head: &str,
    since: i64,
    until: i64,
) -> Result<WindowCommits, String> {
    let hashes = listed_hashes(root, head, since, until, &["--no-merges", "--date-order"])?;
    let reachable = listed_hashes(root, head, since, until, &[])?
        .into_iter()
        .collect();
    Ok(WindowCommits { hashes, reachable })
}

/// Assembles the period `since..=until` of `branch` at `head` from the store.
/// Database only, in one read transaction so a batch committed meanwhile
/// cannot tear the snapshot: pass the store's read connection
/// (`SessionStore::with_read_conn`). Nothing here fetches or computes: a
/// missing commit or an uncovered interval only lowers `covered`.
pub(super) fn load_window(
    conn: &Connection,
    repo: i64,
    branch: &str,
    head: &str,
    since: i64,
    until: i64,
    commits: &WindowCommits,
) -> Result<Loaded, String> {
    let tx = conn.unchecked_transaction().map_err(sql)?;
    load_in(&tx, repo, branch, head, since, until, commits)
}

fn load_in(
    conn: &Connection,
    repo: i64,
    branch: &str,
    head: &str,
    since: i64,
    until: i64,
    window: &WindowCommits,
) -> Result<Loaded, String> {
    let hashes = &window.hashes;

    let rows: Vec<CommitColumns> = query_in(
        conn,
        "SELECT hash, subject, author_json, committer_json, authored_at, committed_at,
                co_authors_json, files_json, patch_id, reverts, oversized
         FROM contrib_commits WHERE repo_id = ? AND hash IN ({in})",
        &[&repo],
        hashes,
        |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
                row.get(5)?,
                row.get(6)?,
                row.get(7)?,
                row.get(8)?,
                row.get(9)?,
                row.get(10)?,
            ))
        },
    )?;
    let mut by_hash: HashMap<String, CommitColumns> =
        rows.into_iter().map(|r| (r.0.clone(), r)).collect();
    let missing_commits = hashes.iter().filter(|h| !by_hash.contains_key(*h)).count() as u64;

    // Keep each patch once: the earliest commit, as the live collector does.
    let mut pairs: Vec<(String, String)> = Vec::new();
    let mut committed: HashMap<String, i64> = HashMap::new();
    for hash in hashes {
        let Some(row) = by_hash.get(hash) else {
            continue;
        };
        if let Some(epoch) = parse_iso_epoch(&row.5) {
            committed.insert(hash.clone(), epoch);
        }
        if let Some(patch) = &row.8 {
            pairs.push((patch.clone(), hash.clone()));
        }
    }
    let dropped = select_duplicates(&pairs, &committed);

    let pr_of: HashMap<String, String> = query_in(
        conn,
        "SELECT hash, pr_id FROM contrib_commit_prs
         WHERE repo_id = ? AND branch = ? AND hash IN ({in})",
        &[&repo, &branch],
        hashes,
        |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
    )?
    .into_iter()
    .collect();

    let mut prs: Vec<RawPr> = {
        let mut stmt = conn
            .prepare(
                "SELECT pr_json FROM contrib_prs
                 WHERE repo_id = ?1 AND branch = ?2
                   AND ((merged_epoch BETWEEN ?3 AND ?4) OR (closed_epoch BETWEEN ?3 AND ?4))
                 ORDER BY COALESCE(merged_epoch, closed_epoch) DESC, pr_id",
            )
            .map_err(sql)?;
        let rows = stmt
            .query_map(params![repo, branch, since, until], |row| {
                row.get::<_, String>(0)
            })
            .map_err(sql)?;
        let mut prs = Vec::new();
        for row in rows {
            prs.push(unjson::<RawPr>(&row.map_err(sql)?)?);
        }
        prs
    };
    // A merged PR whose merge commit is no longer on the branch (force push,
    // rebase, reset) is gone with it. Closed PRs have no merge commit.
    let gone: HashSet<String> = prs
        .iter()
        .filter(|pr| {
            pr.state == "merged"
                && pr
                    .merge_commit
                    .as_deref()
                    .is_some_and(|c| !c.is_empty() && !window.reachable.contains(c))
        })
        .map(|pr| pr.id.clone())
        .collect();
    prs.retain(|pr| !gone.contains(&pr.id));

    let mut commits = Vec::new();
    let mut reverts: Vec<(String, String)> = Vec::new();
    for hash in hashes {
        let Some(row) = by_hash.remove(hash) else {
            continue;
        };
        if let Some(reverted) = &row.9 {
            reverts.push((hash.clone(), reverted.clone()));
        }
        if dropped.contains(hash) {
            continue;
        }
        commits.push(RawCommit {
            pr_id: pr_of.get(hash).filter(|id| !gone.contains(*id)).cloned(),
            hash: row.0,
            subject: row.1,
            author: unjson::<GitIdentity>(&row.2)?,
            committer: unjson::<GitIdentity>(&row.3)?,
            authored_at: row.4,
            committed_at: row.5,
            co_authors: unjson::<Vec<GitIdentity>>(&row.6)?,
            files: unjson::<Vec<RawFile>>(&row.7)?,
        });
    }

    let reviews: Vec<RawReview> = {
        let mut stmt = conn
            .prepare(
                "SELECT review_json FROM contrib_reviews
                 WHERE repo_id = ?1 AND branch = ?2 AND submitted_epoch BETWEEN ?3 AND ?4
                 ORDER BY submitted_epoch, pr_id, ordinal",
            )
            .map_err(sql)?;
        let rows = stmt
            .query_map(params![repo, branch, since, until], |row| {
                row.get::<_, String>(0)
            })
            .map_err(sql)?;
        let mut reviews = Vec::new();
        for row in rows {
            let review = unjson::<RawReview>(&row.map_err(sql)?)?;
            if !gone.contains(&review.pr_id) {
                reviews.push(review);
            }
        }
        reviews
    };

    let pr_ids: Vec<String> = prs.iter().map(|p| p.id.clone()).collect();
    let survival_rows = query_in(
        conn,
        "SELECT pr_id, written, alive FROM contrib_survival
         WHERE repo_id = ? AND branch = ? AND pr_id IN ({in})",
        &[&repo, &branch],
        &pr_ids,
        |row| {
            Ok(RawSurvival {
                pr_id: row.get(0)?,
                written: row.get::<_, i64>(1)?.max(0) as u64,
                alive: row.get::<_, i64>(2)?.max(0) as u64,
            })
        },
    )?;
    let mut survival_by_pr: HashMap<String, RawSurvival> = survival_rows
        .into_iter()
        .map(|s| (s.pr_id.clone(), s))
        .collect();
    let survival: Vec<RawSurvival> = pr_ids
        .iter()
        .filter_map(|id| survival_by_pr.remove(id))
        .collect();

    // A reverted commit may predate the window, so look its PR up on its own.
    let reverted_hashes: Vec<String> = reverts.iter().map(|(_, sha)| sha.clone()).collect();
    let reverted_pr_of: HashMap<String, String> = query_in(
        conn,
        "SELECT hash, pr_id FROM contrib_commit_prs
         WHERE repo_id = ? AND branch = ? AND hash IN ({in})",
        &[&repo, &branch],
        &reverted_hashes,
        |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
    )?
    .into_iter()
    .collect();
    let mut reverted_pr_ids: Vec<String> = Vec::new();
    for (commit, sha) in &reverts {
        let Some(pr_id) = reverted_pr_of.get(sha).filter(|id| !gone.contains(*id)) else {
            continue;
        };
        // A revert is itself a PR; it must not flag its own PR.
        if pr_of.get(commit) == Some(pr_id) {
            continue;
        }
        if !reverted_pr_ids.contains(pr_id) {
            reverted_pr_ids.push(pr_id.clone());
        }
    }

    let intervals = coverage(conn, repo, branch)?;
    let has_gaps = !missing_intervals(&intervals, since, until).is_empty();
    let covered = !has_gaps && missing_commits == 0;
    let overlapping: Vec<&CoverageRow> = intervals
        .iter()
        .filter(|r| r.from_epoch <= until && r.to_epoch >= since)
        .collect();
    // The window is only as complete as its weakest part: report the worst
    // GitHub state (and that row's note), and the oldest sync for "synced at"
    // and for where a top-up has to start.
    let worst = overlapping
        .iter()
        .max_by_key(|r| (gh_rank(&r.gh_state), r.synced_at));
    let oldest = overlapping.iter().min_by_key(|r| r.synced_at);
    // A window that mixes rows without GitHub (a small top-up while gh was
    // offline) with rows that have it still has GitHub data: report it as
    // partial, with the unavailable row's note, so the reviews that were
    // synced are not thrown away. Only a window with no GitHub data at all is
    // unavailable.
    let has_github_rows = overlapping.iter().any(|r| r.gh_state != "unavailable");
    let gh_state = worst.map(|r| {
        if r.gh_state == "unavailable" && has_github_rows {
            "partial".to_string()
        } else {
            r.gh_state.clone()
        }
    });

    Ok(Loaded {
        raw: RawData {
            head: head.to_string(),
            branch: branch.to_string(),
            since: epoch_to_iso(since),
            until: epoch_to_iso(until),
            gh_available: gh_state.as_deref().is_some_and(|s| s != "unavailable"),
            gh_note: worst.and_then(|r| r.gh_note.clone()),
            commits,
            prs,
            reviews,
            survival,
            reverted_pr_ids,
            truncated: false,
        },
        covered,
        has_gaps,
        missing_commits,
        synced_at: oldest.map(|r| r.synced_at),
        coverage_head: oldest.map(|r| r.head.clone()),
        gh_state,
    })
}

#[cfg(test)]
mod tests {
    use super::super::tests::{TestRepo, ALICE};
    use super::*;
    use crate::session_store::SessionStore;

    const DAY: i64 = 86_400;

    fn epoch(iso: &str) -> i64 {
        parse_iso_epoch(iso).unwrap()
    }

    fn who(name: &str) -> GitIdentity {
        GitIdentity {
            name: name.to_string(),
            email: format!("{}@example.com", name.to_lowercase()),
            login: None,
        }
    }

    fn stored(hash: &str, subject: &str, date: &str, patch_id: Option<&str>) -> StoredCommit {
        StoredCommit {
            commit: RawCommit {
                hash: hash.to_string(),
                subject: subject.to_string(),
                author: who("Alice"),
                committer: who("Alice"),
                authored_at: date.to_string(),
                committed_at: date.to_string(),
                co_authors: vec![who("Bob")],
                files: vec![RawFile {
                    path: format!("{subject}.txt"),
                    added: 3,
                    deleted: 1,
                }],
                pr_id: None,
            },
            patch_id: patch_id.map(str::to_string),
            reverts: None,
            oversized: false,
        }
    }

    fn pr(id: &str, state: &str, merged_at: Option<&str>, closed_at: Option<&str>) -> RawPr {
        RawPr {
            id: id.to_string(),
            number: Some(7),
            title: format!("PR {id}"),
            state: state.to_string(),
            source: "gh".to_string(),
            author: who("Alice"),
            committer: None,
            merged_at: merged_at.map(str::to_string),
            closed_at: closed_at.map(str::to_string),
            merge_commit: None,
            commit_count: 2,
            branch_commits: Vec::new(),
            landing_co_authors: Vec::new(),
            files: Vec::new(),
            closes_bug: false,
        }
    }

    fn review(pr_id: &str, at: &str) -> RawReview {
        RawReview {
            pr_id: pr_id.to_string(),
            reviewer: who("Bob"),
            state: "APPROVED".to_string(),
            submitted_at: at.to_string(),
            comments: 2,
        }
    }

    fn coverage_row(from: i64, to: i64, head: &str, synced_at: i64) -> CoverageRow {
        CoverageRow {
            from_epoch: from,
            to_epoch: to,
            head: head.to_string(),
            synced_at,
            gh_state: "complete".to_string(),
            gh_note: None,
        }
    }

    fn write<T>(
        store: &SessionStore,
        f: impl FnOnce(&Transaction) -> Result<T, String>,
    ) -> Result<T, String> {
        let mut conn = store.lock_conn()?;
        let tx = conn.transaction().map_err(|e| e.to_string())?;
        let out = f(&tx)?;
        tx.commit().map_err(|e| e.to_string())?;
        Ok(out)
    }

    fn count(conn: &Connection, table: &str, repo: i64) -> i64 {
        conn.query_row(
            &format!("SELECT COUNT(*) FROM {table} WHERE repo_id = ?1"),
            [repo],
            |row| row.get(0),
        )
        .unwrap()
    }

    struct Fixture {
        repo: TestRepo,
        store: SessionStore,
        id: i64,
        hashes: Vec<String>,
    }

    /// Three commits in February 2026, all stored, one PR over the last two,
    /// one review, one survival row and coverage of the whole of 2026.
    fn synced_fixture() -> Fixture {
        let repo = TestRepo::new();
        let dates = [
            "2026-02-01T10:00:00+0000",
            "2026-02-02T10:00:00+0000",
            "2026-02-03T10:00:00+0000",
        ];
        let hashes: Vec<String> = dates
            .iter()
            .enumerate()
            .map(|(i, date)| {
                repo.commit(
                    &[(&format!("f{i}.txt"), "x")],
                    &format!("c{i}"),
                    ALICE,
                    date,
                )
            })
            .collect();
        let store = SessionStore::open_in_memory().unwrap();
        let dir = git_common_dir(repo.path()).unwrap();
        let id = {
            let conn = store.lock_conn().unwrap();
            repo_id(&conn, &dir, Some("o/r")).unwrap()
        };
        write(&store, |tx| {
            let commits: Vec<StoredCommit> = hashes
                .iter()
                .enumerate()
                .map(|(i, h)| {
                    stored(
                        h,
                        &format!("c{i}"),
                        &epoch_to_iso(epoch("2026-02-01T10:00:00Z") + i as i64 * DAY),
                        Some(&format!("patch{i}")),
                    )
                })
                .collect();
            insert_commits(tx, id, &commits)?;
            put_prs(
                tx,
                id,
                "main",
                &[(
                    pr("pr-1", "merged", Some("2026-02-03T11:00:00Z"), None),
                    vec![hashes[1].clone(), hashes[2].clone()],
                )],
            )?;
            put_reviews(tx, id, "main", &[review("pr-1", "2026-02-03T12:00:00Z")])?;
            put_survival(
                tx,
                id,
                "main",
                &hashes[2],
                &[RawSurvival {
                    pr_id: "pr-1".to_string(),
                    written: 10,
                    alive: 8,
                }],
            )?;
            add_coverage(
                tx,
                id,
                "main",
                &coverage_row(
                    epoch("2026-01-01T00:00:00Z"),
                    epoch("2026-12-31T00:00:00Z"),
                    &hashes[2],
                    1_000,
                ),
            )
        })
        .unwrap();
        Fixture {
            repo,
            store,
            id,
            hashes,
        }
    }

    fn load_between(f: &Fixture, head: &str, since: i64, until: i64) -> Loaded {
        let commits = window_commits(f.repo.path(), head, since, until).unwrap();
        f.store
            .with_read_conn(|conn| load_window(conn, f.id, "main", head, since, until, &commits))
            .unwrap()
    }

    fn load_all(f: &Fixture, head: &str) -> Loaded {
        load_between(
            f,
            head,
            epoch("2026-02-01T00:00:00Z"),
            epoch("2026-03-01T00:00:00Z"),
        )
    }

    // migration

    #[test]
    fn migration_from_version_19_adds_the_tables() {
        let path = std::env::temp_dir().join(format!(
            "monoptah-contributors-store-{}.db",
            uuid::Uuid::new_v4()
        ));
        drop(SessionStore::open(path.clone()).unwrap());
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(
                "DROP TABLE contrib_repos; DROP TABLE contrib_commits; DROP TABLE contrib_prs;
                 DROP TABLE contrib_commit_prs; DROP TABLE contrib_reviews;
                 DROP TABLE contrib_survival; DROP TABLE contrib_coverage;
                 DELETE FROM schema_migrations WHERE version >= 20;",
            )
            .unwrap();
        }
        let store = SessionStore::open(path.clone()).unwrap();
        let conn = store.lock_conn().unwrap();
        let tables: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name LIKE 'contrib_%'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let version: i64 = conn
            .query_row("SELECT MAX(version) FROM schema_migrations", [], |row| {
                row.get(0)
            })
            .unwrap();
        drop(conn);
        drop(store);
        for suffix in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
        }
        assert_eq!(tables, 11);
        // The fork records no schema version of its own: upstream owns the
        // numbers, and the tables come from `ensure_tables`.
        assert_eq!(version, 19);
    }

    #[test]
    fn a_fresh_database_gets_the_tables_and_stays_at_version_19() {
        let store = SessionStore::open_in_memory().unwrap();
        let conn = store.lock_conn().unwrap();
        let tables: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name LIKE 'contrib_%'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let version: i64 = conn
            .query_row("SELECT MAX(version) FROM schema_migrations", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(tables, 11);
        assert_eq!(version, 19);
    }

    #[test]
    fn tables_are_created_even_when_version_20_is_already_recorded() {
        let path = std::env::temp_dir().join(format!(
            "monoptah-contributors-store-{}.db",
            uuid::Uuid::new_v4()
        ));
        drop(SessionStore::open(path.clone()).unwrap());
        {
            // Another build used version 20 for something else.
            let conn = Connection::open(&path).unwrap();
            conn.execute(
                "INSERT OR IGNORE INTO schema_migrations (version, applied_at) VALUES (20, 0)",
                [],
            )
            .unwrap();
            conn.execute_batch(
                "DROP TABLE contrib_repos; DROP TABLE contrib_commits; DROP TABLE contrib_prs;
                 DROP TABLE contrib_commit_prs; DROP TABLE contrib_reviews;
                 DROP TABLE contrib_survival; DROP TABLE contrib_coverage;",
            )
            .unwrap();
            let version: i64 = conn
                .query_row("SELECT MAX(version) FROM schema_migrations", [], |row| {
                    row.get(0)
                })
                .unwrap();
            assert!(version >= 20);
        }
        let store = SessionStore::open(path.clone()).unwrap();
        let conn = store.lock_conn().unwrap();
        let tables: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name LIKE 'contrib_%'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        drop(conn);
        drop(store);
        for suffix in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
        }
        assert_eq!(tables, 11);
    }

    // repo identity

    #[test]
    fn repo_id_is_stable_and_shared_by_worktrees() {
        let repo = TestRepo::new();
        repo.commit(
            &[("a.txt", "a")],
            "first",
            ALICE,
            "2026-02-01T10:00:00+0000",
        );
        let worktree = std::env::temp_dir().join(format!("monoptah-wt-{}", uuid::Uuid::new_v4()));
        repo.run(
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "other",
                worktree.to_str().unwrap(),
            ],
            ALICE,
            "2026-02-01T10:00:00+0000",
        );
        let main_dir = git_common_dir(repo.path()).unwrap();
        let wt_dir = git_common_dir(&worktree).unwrap();
        let _ = std::fs::remove_dir_all(&worktree);
        assert_eq!(main_dir, wt_dir);
        assert!(Path::new(&main_dir).is_absolute());

        let store = SessionStore::open_in_memory().unwrap();
        let conn = store.lock_conn().unwrap();
        assert_eq!(find_repo_id(&conn, &main_dir).unwrap(), None);
        let first = repo_id(&conn, &main_dir, None).unwrap();
        let second = repo_id(&conn, &wt_dir, Some("o/r")).unwrap();
        assert_eq!(first, second);
        assert_eq!(find_repo_id(&conn, &main_dir).unwrap(), Some(first));
        let slug: Option<String> = conn
            .query_row(
                "SELECT slug FROM contrib_repos WHERE id = ?1",
                [first],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(slug.as_deref(), Some("o/r"));
        // A later call without a slug keeps the known one.
        repo_id(&conn, &main_dir, None).unwrap();
        let slug: Option<String> = conn
            .query_row(
                "SELECT slug FROM contrib_repos WHERE id = ?1",
                [first],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(slug.as_deref(), Some("o/r"));
        assert_ne!(repo_id(&conn, "/elsewhere/.git", None).unwrap(), first);
    }

    // commits

    #[test]
    fn inserted_commits_round_trip_and_are_written_once() {
        let store = SessionStore::open_in_memory().unwrap();
        let id = {
            let conn = store.lock_conn().unwrap();
            repo_id(&conn, "/r/.git", None).unwrap()
        };
        let h = |c: char| c.to_string().repeat(40);
        write(&store, |tx| {
            insert_commits(
                tx,
                id,
                &[
                    stored(&h('a'), "one", "2026-02-01T10:00:00Z", None),
                    stored(&h('b'), "two", "2026-02-02T10:00:00Z", Some("p")),
                ],
            )?;
            // The same hash again is ignored: a commit is immutable.
            insert_commits(
                tx,
                id,
                &[stored(&h('a'), "changed", "2026-02-09T10:00:00Z", None)],
            )
        })
        .unwrap();
        let conn = store.lock_conn().unwrap();
        let wanted = vec![h('a'), h('b'), h('c')];
        let found = stored_hashes(&conn, id, &wanted).unwrap();
        assert_eq!(found, HashSet::from([h('a'), h('b')]));
        let subject: String = conn
            .query_row(
                "SELECT subject FROM contrib_commits WHERE repo_id = ?1 AND hash = ?2",
                params![id, h('a')],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(subject, "one");
        assert!(stored_hashes(&conn, id + 1, &wanted).unwrap().is_empty());
        assert!(stored_hashes(&conn, id, &[]).unwrap().is_empty());
    }

    #[test]
    fn stored_hashes_handles_more_hashes_than_one_query_holds() {
        let store = SessionStore::open_in_memory().unwrap();
        let conn = store.lock_conn().unwrap();
        let id = repo_id(&conn, "/r/.git", None).unwrap();
        let hashes: Vec<String> = (0..1_300).map(|i| format!("{i:040}")).collect();
        drop(conn);
        write(&store, |tx| {
            let commits: Vec<StoredCommit> = hashes
                .iter()
                .step_by(2)
                .map(|h| stored(h, "s", "2026-02-01T10:00:00Z", None))
                .collect();
            insert_commits(tx, id, &commits)
        })
        .unwrap();
        let conn = store.lock_conn().unwrap();
        assert_eq!(stored_hashes(&conn, id, &hashes).unwrap().len(), 650);
    }

    // coverage

    #[test]
    fn coverage_merges_overlapping_and_adjacent_intervals() {
        let store = SessionStore::open_in_memory().unwrap();
        let id = {
            let conn = store.lock_conn().unwrap();
            repo_id(&conn, "/r/.git", None).unwrap()
        };
        let add =
            |row: CoverageRow| write(&store, |tx| add_coverage(tx, id, "main", &row)).unwrap();
        let read = |branch: &str| {
            let conn = store.lock_conn().unwrap();
            coverage(&conn, id, branch).unwrap()
        };
        add(coverage_row(100, 200, "h1", 10));
        add(coverage_row(400, 500, "h2", 20));
        let rows = read("main");
        assert_eq!(
            rows.iter()
                .map(|r| (r.from_epoch, r.to_epoch))
                .collect::<Vec<_>>(),
            [(100, 200), (400, 500)]
        );
        // Adjacent (201 follows 200): merged, the row that ends last wins.
        add(coverage_row(201, 300, "h3", 30));
        let rows = read("main");
        assert_eq!(rows.len(), 2);
        assert_eq!((rows[0].from_epoch, rows[0].to_epoch), (100, 300));
        assert_eq!(rows[0].head, "h3");
        assert_eq!(rows[0].synced_at, 30);
        // Overlaps both: everything folds into one, and the row that ends
        // last (400..500) gives its head and time, even though h3 is newer.
        add(coverage_row(250, 450, "h0", 5));
        let rows = read("main");
        assert_eq!(rows.len(), 1);
        assert_eq!((rows[0].from_epoch, rows[0].to_epoch), (100, 500));
        assert_eq!(rows[0].head, "h2");
        assert_eq!(rows[0].synced_at, 20);
        // A branch has its own intervals.
        assert!(read("other").is_empty());
    }

    fn state_row(from: i64, to: i64, state: &str, synced_at: i64) -> CoverageRow {
        let mut row = coverage_row(from, to, &format!("h{synced_at}"), synced_at);
        row.gh_state = state.to_string();
        row.gh_note = (state != "complete").then(|| format!("{state} note"));
        row
    }

    #[test]
    fn intervals_with_different_github_states_are_not_merged() {
        let store = SessionStore::open_in_memory().unwrap();
        let id = {
            let conn = store.lock_conn().unwrap();
            repo_id(&conn, "/r/.git", None).unwrap()
        };
        write(&store, |tx| {
            add_coverage(tx, id, "main", &state_row(100, 199, "unavailable", 10))?;
            add_coverage(tx, id, "main", &state_row(200, 299, "complete", 20))
        })
        .unwrap();
        let conn = store.lock_conn().unwrap();
        let rows = coverage(&conn, id, "main").unwrap();
        assert_eq!(
            rows.iter()
                .map(|r| (r.from_epoch, r.to_epoch, r.gh_state.as_str()))
                .collect::<Vec<_>>(),
            [(100, 199, "unavailable"), (200, 299, "complete")]
        );
        assert!(missing_intervals(&rows, 100, 299).is_empty());
    }

    #[test]
    fn a_new_interval_replaces_the_overlap_of_another_state() {
        let store = SessionStore::open_in_memory().unwrap();
        let id = {
            let conn = store.lock_conn().unwrap();
            repo_id(&conn, "/r/.git", None).unwrap()
        };
        write(&store, |tx| {
            add_coverage(tx, id, "main", &state_row(100, 400, "unavailable", 10))?;
            add_coverage(tx, id, "main", &state_row(200, 299, "complete", 20))
        })
        .unwrap();
        let conn = store.lock_conn().unwrap();
        let rows = coverage(&conn, id, "main").unwrap();
        assert_eq!(
            rows.iter()
                .map(|r| (r.from_epoch, r.to_epoch, r.gh_state.as_str(), r.synced_at))
                .collect::<Vec<_>>(),
            [
                (100, 199, "unavailable", 10),
                (200, 299, "complete", 20),
                (300, 400, "unavailable", 10)
            ]
        );
        assert!(missing_intervals(&rows, 100, 400).is_empty());
    }

    #[test]
    fn the_row_that_ends_last_wins_when_coverage_merges() {
        let store = SessionStore::open_in_memory().unwrap();
        let id = {
            let conn = store.lock_conn().unwrap();
            repo_id(&conn, "/r/.git", None).unwrap()
        };
        write(&store, |tx| {
            add_coverage(tx, id, "main", &coverage_row(100, 200, "new", 50))?;
            add_coverage(tx, id, "main", &coverage_row(150, 300, "old", 10))
        })
        .unwrap();
        let conn = store.lock_conn().unwrap();
        let rows = coverage(&conn, id, "main").unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!((rows[0].from_epoch, rows[0].to_epoch), (100, 300));
        assert_eq!(rows[0].head, "old");
        assert_eq!(rows[0].synced_at, 10);
    }

    #[test]
    fn the_newer_row_wins_when_merged_rows_end_together() {
        let store = SessionStore::open_in_memory().unwrap();
        let id = {
            let conn = store.lock_conn().unwrap();
            repo_id(&conn, "/r/.git", None).unwrap()
        };
        write(&store, |tx| {
            add_coverage(tx, id, "main", &coverage_row(100, 300, "new", 50))?;
            add_coverage(tx, id, "main", &coverage_row(150, 300, "old", 10))
        })
        .unwrap();
        let conn = store.lock_conn().unwrap();
        let rows = coverage(&conn, id, "main").unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].head, "new");
        assert_eq!(rows[0].synced_at, 50);
    }

    #[test]
    fn missing_intervals_lists_the_gaps() {
        let rows = vec![
            coverage_row(300, 400, "h", 1),
            coverage_row(100, 200, "h", 1),
        ];
        assert_eq!(missing_intervals(&rows, 100, 400), [(201, 299)]);
        assert_eq!(missing_intervals(&rows, 150, 350), [(201, 299)]);
        assert_eq!(
            missing_intervals(&rows, 0, 500),
            [(0, 99), (201, 299), (401, 500)]
        );
        assert!(missing_intervals(&rows, 120, 180).is_empty());
        assert_eq!(missing_intervals(&rows, 500, 600), [(500, 600)]);
        assert_eq!(missing_intervals(&[], 5, 9), [(5, 9)]);
        assert!(missing_intervals(&rows, 9, 5).is_empty());
    }

    // read path

    #[test]
    fn load_returns_what_was_stored_for_a_synced_period() {
        let f = synced_fixture();
        let loaded = load_all(&f, &f.hashes[2]);
        assert!(loaded.covered);
        assert_eq!(loaded.missing_commits, 0);
        assert_eq!(loaded.synced_at, Some(1_000));
        assert_eq!(loaded.coverage_head.as_deref(), Some(f.hashes[2].as_str()));
        assert_eq!(loaded.gh_state.as_deref(), Some("complete"));
        let raw = loaded.raw;
        assert_eq!(raw.head, f.hashes[2]);
        assert_eq!(raw.branch, "main");
        assert_eq!(raw.since, "2026-02-01T00:00:00Z");
        assert_eq!(raw.until, "2026-03-01T00:00:00Z");
        assert!(raw.gh_available);
        assert!(!raw.truncated);
        // Newest first, as git lists them.
        let subjects: Vec<&str> = raw.commits.iter().map(|c| c.subject.as_str()).collect();
        assert_eq!(subjects, ["c2", "c1", "c0"]);
        let pr_ids: Vec<Option<&str>> = raw.commits.iter().map(|c| c.pr_id.as_deref()).collect();
        assert_eq!(pr_ids, [Some("pr-1"), Some("pr-1"), None]);
        let first = &raw.commits[2];
        assert_eq!(first.author, who("Alice"));
        assert_eq!(first.co_authors, vec![who("Bob")]);
        assert_eq!(first.files[0].path, "c0.txt");
        assert_eq!((first.files[0].added, first.files[0].deleted), (3, 1));
        assert_eq!(raw.prs.len(), 1);
        assert_eq!(raw.prs[0].id, "pr-1");
        assert_eq!(raw.prs[0].number, Some(7));
        assert_eq!(raw.reviews.len(), 1);
        assert_eq!(raw.reviews[0].pr_id, "pr-1");
        assert_eq!(raw.reviews[0].comments, 2);
        assert_eq!(
            raw.survival,
            [RawSurvival {
                pr_id: "pr-1".to_string(),
                written: 10,
                alive: 8
            }]
        );
        assert!(raw.reverted_pr_ids.is_empty());
    }

    #[test]
    fn load_keeps_only_prs_and_reviews_inside_the_window() {
        let f = synced_fixture();
        write(&f.store, |tx| {
            put_prs(
                tx,
                f.id,
                "main",
                &[
                    (
                        pr("old", "merged", Some("2026-01-10T10:00:00Z"), None),
                        Vec::new(),
                    ),
                    (
                        pr("shut", "closed", None, Some("2026-02-20T10:00:00Z")),
                        Vec::new(),
                    ),
                ],
            )?;
            put_reviews(
                tx,
                f.id,
                "main",
                &[
                    review("old", "2026-01-11T10:00:00Z"),
                    review("shut", "2026-02-21T10:00:00Z"),
                ],
            )?;
            // Another branch never leaks in.
            put_prs(
                tx,
                f.id,
                "dev",
                &[(
                    pr("dev-1", "merged", Some("2026-02-10T10:00:00Z"), None),
                    Vec::new(),
                )],
            )
        })
        .unwrap();
        let raw = load_all(&f, &f.hashes[2]).raw;
        let mut ids: Vec<&str> = raw.prs.iter().map(|p| p.id.as_str()).collect();
        ids.sort_unstable();
        assert_eq!(ids, ["pr-1", "shut"]);
        let mut reviewed: Vec<&str> = raw.reviews.iter().map(|r| r.pr_id.as_str()).collect();
        reviewed.sort_unstable();
        assert_eq!(reviewed, ["pr-1", "shut"]);
    }

    #[test]
    fn put_prs_and_reviews_replace_earlier_rows() {
        let f = synced_fixture();
        write(&f.store, |tx| {
            put_prs(
                tx,
                f.id,
                "main",
                &[(
                    pr("pr-1", "merged", Some("2026-02-03T11:00:00Z"), None),
                    vec![f.hashes[2].clone()],
                )],
            )?;
            put_reviews(
                tx,
                f.id,
                "main",
                &[
                    review("pr-1", "2026-02-03T12:00:00Z"),
                    review("pr-1", "2026-02-03T13:00:00Z"),
                ],
            )
        })
        .unwrap();
        let raw = load_all(&f, &f.hashes[2]).raw;
        assert_eq!(raw.prs.len(), 1);
        assert_eq!(raw.reviews.len(), 2);
        // The earlier hash list is gone, not merged with the new one.
        let pr_ids: Vec<Option<&str>> = raw.commits.iter().map(|c| c.pr_id.as_deref()).collect();
        assert_eq!(pr_ids, [Some("pr-1"), None, None]);
    }

    #[test]
    fn clearing_a_stretch_of_time_removes_its_prs_with_links_and_reviews_only() {
        let f = synced_fixture();
        let epoch = |text: &str| parse_iso_epoch(text).unwrap();
        write(&f.store, |tx| {
            let mut closed = pr("pr-3", "closed", None, Some("2026-02-20T10:00:00Z"));
            closed.merge_commit = None;
            put_prs(
                tx,
                f.id,
                "main",
                &[
                    (
                        pr("pr-1", "merged", Some("2026-01-03T11:00:00Z"), None),
                        vec![f.hashes[0].clone()],
                    ),
                    (
                        pr("pr-2", "merged", Some("2026-02-03T11:00:00Z"), None),
                        vec![f.hashes[1].clone()],
                    ),
                    (closed, vec![]),
                ],
            )?;
            put_reviews(
                tx,
                f.id,
                "main",
                &[
                    review("pr-1", "2026-01-04T12:00:00Z"),
                    review("pr-2", "2026-02-04T12:00:00Z"),
                ],
            )?;
            put_survival(
                tx,
                f.id,
                "main",
                "head",
                &[RawSurvival {
                    pr_id: "pr-2".to_string(),
                    written: 4,
                    alive: 3,
                }],
            )?;
            clear_prs_in(
                tx,
                f.id,
                "main",
                epoch("2026-02-01T00:00:00Z"),
                epoch("2026-02-28T23:59:59Z"),
            )
        })
        .unwrap();
        let conn = f.store.lock_conn().unwrap();
        let ids: Vec<String> = conn
            .prepare("SELECT pr_id FROM contrib_prs WHERE repo_id = ?1")
            .unwrap()
            .query_map([f.id], |row| row.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert_eq!(ids, ["pr-1"], "the closed PR of February went too");
        assert_eq!(count(&conn, "contrib_commit_prs", f.id), 1);
        assert_eq!(count(&conn, "contrib_reviews", f.id), 1);
        let sampled: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM contrib_survival WHERE repo_id = ?1 AND pr_id = 'pr-2'",
                [f.id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            sampled, 1,
            "a sample stays for a PR that is written again under its id"
        );
    }

    #[test]
    fn a_landing_in_another_stretch_is_dropped_unless_its_pr_is_kept() {
        let f = synced_fixture();
        write(&f.store, |tx| {
            let landing = |id: &str, at: &str, commit: &str| {
                let mut pr = pr(id, "merged", Some(at), None);
                pr.merge_commit = Some(commit.to_string());
                pr
            };
            put_prs(
                tx,
                f.id,
                "main",
                &[
                    (
                        landing("old", "2026-01-31T23:59:59Z", "abc"),
                        vec!["abc".to_string()],
                    ),
                    (
                        landing("mine", "2026-02-01T00:00:00Z", "abc"),
                        vec!["abd".to_string()],
                    ),
                    (landing("other", "2026-01-30T10:00:00Z", "zzz"), vec![]),
                ],
            )?;
            drop_prs_landing_in(
                tx,
                f.id,
                "main",
                &["abc".to_string()],
                &HashSet::from(["mine".to_string()]),
            )
        })
        .unwrap();
        let conn = f.store.lock_conn().unwrap();
        let mut ids: Vec<String> = conn
            .prepare("SELECT pr_id FROM contrib_prs WHERE repo_id = ?1")
            .unwrap()
            .query_map([f.id], |row| row.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        ids.sort();
        // `pr-1` comes with the fixture.
        assert_eq!(ids, ["mine", "other", "pr-1"]);
    }

    #[test]
    fn load_marks_a_period_with_an_unstored_commit_as_not_covered() {
        let f = synced_fixture();
        {
            let conn = f.store.lock_conn().unwrap();
            conn.execute(
                "DELETE FROM contrib_commits WHERE repo_id = ?1 AND hash = ?2",
                params![f.id, f.hashes[0]],
            )
            .unwrap();
        }
        let loaded = load_all(&f, &f.hashes[2]);
        assert!(!loaded.covered);
        // Synced, but a commit is not stored: not a gap in the coverage.
        assert!(!loaded.has_gaps);
        assert_eq!(loaded.missing_commits, 1);
        assert_eq!(loaded.raw.commits.len(), 2);
    }

    #[test]
    fn load_marks_a_period_with_a_coverage_gap_as_not_covered() {
        let f = synced_fixture();
        {
            let conn = f.store.lock_conn().unwrap();
            conn.execute("DELETE FROM contrib_coverage WHERE repo_id = ?1", [f.id])
                .unwrap();
        }
        write(&f.store, |tx| {
            add_coverage(
                tx,
                f.id,
                "main",
                &coverage_row(
                    epoch("2026-02-01T00:00:00Z"),
                    epoch("2026-02-10T00:00:00Z"),
                    "h",
                    5,
                ),
            )
        })
        .unwrap();
        let loaded = load_all(&f, &f.hashes[2]);
        assert!(!loaded.covered);
        assert!(loaded.has_gaps);
        assert_eq!(loaded.missing_commits, 0);
        assert_eq!(loaded.synced_at, Some(5));
    }

    #[test]
    fn load_without_any_coverage_reports_nothing_synced() {
        let f = synced_fixture();
        {
            let conn = f.store.lock_conn().unwrap();
            conn.execute("DELETE FROM contrib_coverage WHERE repo_id = ?1", [f.id])
                .unwrap();
        }
        let loaded = load_all(&f, &f.hashes[2]);
        assert!(!loaded.covered);
        assert!(loaded.has_gaps);
        assert_eq!(loaded.synced_at, None);
        assert_eq!(loaded.coverage_head, None);
        assert_eq!(loaded.gh_state, None);
        assert!(!loaded.raw.gh_available);
    }

    #[test]
    fn load_reports_a_missing_github_in_the_data() {
        let f = synced_fixture();
        write(&f.store, |tx| {
            let mut row = coverage_row(0, i64::MAX / 2, "h", 2_000);
            row.gh_state = "unavailable".to_string();
            row.gh_note = Some("gh is not installed".to_string());
            add_coverage(tx, f.id, "main", &row)
        })
        .unwrap();
        let loaded = load_all(&f, &f.hashes[2]);
        assert!(loaded.covered);
        assert_eq!(loaded.gh_state.as_deref(), Some("unavailable"));
        assert!(!loaded.raw.gh_available);
        assert_eq!(loaded.raw.gh_note.as_deref(), Some("gh is not installed"));
    }

    #[test]
    fn load_never_returns_commits_that_left_the_branch() {
        let f = synced_fixture();
        f.repo.run(
            &["reset", "-q", "--hard", "HEAD~1"],
            ALICE,
            "2026-02-04T10:00:00+0000",
        );
        let loaded = load_all(&f, &f.hashes[1]);
        let hashes: Vec<&str> = loaded.raw.commits.iter().map(|c| c.hash.as_str()).collect();
        assert_eq!(hashes, [f.hashes[1].as_str(), f.hashes[0].as_str()]);
        assert_eq!(loaded.missing_commits, 0);
        assert!(loaded.covered);
    }

    #[test]
    fn load_leaves_out_commits_dated_outside_the_window() {
        let f = synced_fixture();
        let loaded = load_between(
            &f,
            &f.hashes[2],
            epoch("2026-02-02T00:00:00Z"),
            epoch("2026-02-02T23:00:00Z"),
        );
        let subjects: Vec<&str> = loaded
            .raw
            .commits
            .iter()
            .map(|c| c.subject.as_str())
            .collect();
        assert_eq!(subjects, ["c1"]);
    }

    #[test]
    fn load_drops_later_duplicates_by_patch_id() {
        let f = synced_fixture();
        {
            // c0 and c2 carry the same patch; the earlier one is kept.
            let conn = f.store.lock_conn().unwrap();
            conn.execute(
                "UPDATE contrib_commits SET patch_id = 'same' WHERE repo_id = ?1 AND hash IN (?2, ?3)",
                params![f.id, f.hashes[0], f.hashes[2]],
            )
            .unwrap();
        }
        let raw = load_all(&f, &f.hashes[2]).raw;
        let subjects: Vec<&str> = raw.commits.iter().map(|c| c.subject.as_str()).collect();
        assert_eq!(subjects, ["c1", "c0"]);
    }

    #[test]
    fn load_flags_prs_whose_commit_was_reverted_by_another_pr() {
        let f = synced_fixture();
        {
            // c0 reverts c2 (PR pr-1) and belongs to no PR.
            let conn = f.store.lock_conn().unwrap();
            conn.execute(
                "UPDATE contrib_commits SET reverts = ?3 WHERE repo_id = ?1 AND hash = ?2",
                params![f.id, f.hashes[0], f.hashes[2]],
            )
            .unwrap();
        }
        assert_eq!(load_all(&f, &f.hashes[2]).raw.reverted_pr_ids, ["pr-1"]);
        {
            // A revert inside the same PR must not flag it.
            let conn = f.store.lock_conn().unwrap();
            conn.execute(
                "UPDATE contrib_commits SET reverts = NULL WHERE repo_id = ?1 AND hash = ?2",
                params![f.id, f.hashes[0]],
            )
            .unwrap();
            conn.execute(
                "UPDATE contrib_commits SET reverts = ?3 WHERE repo_id = ?1 AND hash = ?2",
                params![f.id, f.hashes[2], f.hashes[1]],
            )
            .unwrap();
        }
        assert!(load_all(&f, &f.hashes[2]).raw.reverted_pr_ids.is_empty());
    }

    #[test]
    fn load_reports_the_worst_github_state_of_the_window() {
        let f = synced_fixture();
        {
            let conn = f.store.lock_conn().unwrap();
            conn.execute("DELETE FROM contrib_coverage WHERE repo_id = ?1", [f.id])
                .unwrap();
        }
        let (jan, feb, mar) = (
            epoch("2026-01-01T00:00:00Z"),
            epoch("2026-02-01T00:00:00Z"),
            epoch("2026-03-01T00:00:00Z"),
        );
        write(&f.store, |tx| {
            add_coverage(
                tx,
                f.id,
                "main",
                &state_row(jan, feb - 1, "unavailable", 100),
            )?;
            add_coverage(tx, f.id, "main", &state_row(feb, mar - 1, "complete", 200))?;
            add_coverage(tx, f.id, "main", &state_row(mar, mar + 99, "partial", 300))
        })
        .unwrap();
        // February alone is complete.
        let feb_only = load_between(&f, &f.hashes[2], feb, mar - 1);
        assert!(feb_only.covered);
        assert_eq!(feb_only.gh_state.as_deref(), Some("complete"));
        assert!(feb_only.raw.gh_available);
        // January and February together still have GitHub data for February:
        // partial, with January's note, never unavailable.
        let both = load_between(&f, &f.hashes[2], jan, mar - 1);
        assert!(both.covered);
        assert_eq!(both.gh_state.as_deref(), Some("partial"));
        assert!(both.raw.gh_available);
        assert_eq!(both.raw.gh_note.as_deref(), Some("unavailable note"));
        assert_eq!(both.synced_at, Some(100));
        // Complete plus partial reports the partial row and its note.
        let late = load_between(&f, &f.hashes[2], feb, mar + 10);
        assert_eq!(late.gh_state.as_deref(), Some("partial"));
        assert!(late.raw.gh_available);
        assert_eq!(late.raw.gh_note.as_deref(), Some("partial note"));
    }

    #[test]
    fn a_complete_row_with_an_unavailable_top_up_reports_partial() {
        let f = synced_fixture();
        {
            let conn = f.store.lock_conn().unwrap();
            conn.execute("DELETE FROM contrib_coverage WHERE repo_id = ?1", [f.id])
                .unwrap();
        }
        let (jan, feb, mar) = (
            epoch("2026-01-01T00:00:00Z"),
            epoch("2026-02-01T00:00:00Z"),
            epoch("2026-03-01T00:00:00Z"),
        );
        write(&f.store, |tx| {
            add_coverage(tx, f.id, "main", &state_row(jan, feb - 1, "complete", 100))?;
            add_coverage(
                tx,
                f.id,
                "main",
                &state_row(feb, mar - 1, "unavailable", 200),
            )
        })
        .unwrap();
        let loaded = load_between(&f, &f.hashes[2], jan, mar - 1);
        assert_eq!(loaded.gh_state.as_deref(), Some("partial"));
        assert!(loaded.raw.gh_available);
        assert_eq!(loaded.raw.gh_note.as_deref(), Some("unavailable note"));
    }

    #[test]
    fn rows_that_are_all_unavailable_report_unavailable() {
        let f = synced_fixture();
        {
            let conn = f.store.lock_conn().unwrap();
            conn.execute("DELETE FROM contrib_coverage WHERE repo_id = ?1", [f.id])
                .unwrap();
        }
        let (jan, feb, mar) = (
            epoch("2026-01-01T00:00:00Z"),
            epoch("2026-02-01T00:00:00Z"),
            epoch("2026-03-01T00:00:00Z"),
        );
        write(&f.store, |tx| {
            // Two rows that stay apart (there is a gap between them).
            add_coverage(
                tx,
                f.id,
                "main",
                &state_row(jan, jan + 99, "unavailable", 100),
            )?;
            add_coverage(
                tx,
                f.id,
                "main",
                &state_row(feb, mar - 1, "unavailable", 200),
            )
        })
        .unwrap();
        let loaded = load_between(&f, &f.hashes[2], jan, mar - 1);
        assert_eq!(loaded.gh_state.as_deref(), Some("unavailable"));
        assert!(!loaded.raw.gh_available);
    }

    #[test]
    fn load_drops_merged_prs_whose_merge_commit_left_the_branch() {
        let f = synced_fixture();
        let mut gone = pr("gone", "merged", Some("2026-02-03T11:00:00Z"), None);
        gone.merge_commit = Some(f.hashes[2].clone());
        let mut kept = pr("kept", "merged", Some("2026-02-02T11:00:00Z"), None);
        kept.merge_commit = Some(f.hashes[1].clone());
        let shut = pr("shut", "closed", None, Some("2026-02-02T12:00:00Z"));
        write(&f.store, |tx| {
            put_prs(
                tx,
                f.id,
                "main",
                &[
                    (gone, vec![f.hashes[2].clone()]),
                    (kept, vec![f.hashes[1].clone()]),
                    (shut, Vec::new()),
                ],
            )?;
            put_reviews(tx, f.id, "main", &[review("gone", "2026-02-03T12:00:00Z")])
        })
        .unwrap();
        let before = load_all(&f, &f.hashes[2]).raw;
        assert!(before.prs.iter().any(|p| p.id == "gone"));
        f.repo.run(
            &["reset", "-q", "--hard", "HEAD~1"],
            ALICE,
            "2026-02-04T10:00:00+0000",
        );
        let raw = load_all(&f, &f.hashes[1]).raw;
        let mut ids: Vec<&str> = raw.prs.iter().map(|p| p.id.as_str()).collect();
        ids.sort_unstable();
        // pr-1 has no merge commit recorded, so nothing says it left.
        assert_eq!(ids, ["kept", "pr-1", "shut"]);
        assert!(raw.reviews.iter().all(|r| r.pr_id != "gone"));
        assert!(raw
            .commits
            .iter()
            .all(|c| c.pr_id.as_deref() != Some("gone")));
    }

    // clearing

    #[test]
    fn clear_repo_removes_every_row_of_that_repo_only() {
        let f = synced_fixture();
        let other = {
            let conn = f.store.lock_conn().unwrap();
            repo_id(&conn, "/other/.git", None).unwrap()
        };
        write(&f.store, |tx| {
            insert_commits(
                tx,
                other,
                &[stored(&"e".repeat(40), "x", "2026-02-01T10:00:00Z", None)],
            )?;
            put_prs(
                tx,
                other,
                "main",
                &[(
                    pr("p", "merged", Some("2026-02-01T10:00:00Z"), None),
                    vec!["e".repeat(40)],
                )],
            )?;
            put_reviews(tx, other, "main", &[review("p", "2026-02-01T11:00:00Z")])?;
            put_survival(
                tx,
                other,
                "main",
                "h",
                &[RawSurvival {
                    pr_id: "p".to_string(),
                    written: 1,
                    alive: 1,
                }],
            )?;
            add_coverage(tx, other, "main", &coverage_row(1, 2, "h", 3))
        })
        .unwrap();
        let dir = git_common_dir(f.repo.path()).unwrap();
        let conn = f.store.lock_conn().unwrap();
        clear_repo(&conn, &dir).unwrap();
        let tables = [
            "contrib_commits",
            "contrib_prs",
            "contrib_commit_prs",
            "contrib_reviews",
            "contrib_survival",
            "contrib_coverage",
        ];
        for table in tables {
            assert_eq!(count(&conn, table, f.id), 0, "{table} of the cleared repo");
            assert_eq!(count(&conn, table, other), 1, "{table} of the other repo");
        }
        // The repo row stays so the id a running job holds is never reused.
        assert_eq!(find_repo_id(&conn, &dir).unwrap(), Some(f.id));
        // Unknown repos are fine.
        clear_repo(&conn, "/never/seen/.git").unwrap();
    }
}
