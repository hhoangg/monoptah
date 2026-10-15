//! Raw facts for the Contributors page.
//!
//! This module only gathers facts (git history, PR landings derived from git,
//! survival samples, reverts). It never scores anything: identity merging,
//! ignore globs, scoring and ranking live in the TypeScript model, so editing
//! weights or ignore rules never re-runs git.
//!
//! The GitHub (`gh`) enrichment (`gh::enrich`) runs after the PRs are derived
//! from git and before survival and reverts are computed, so those use the
//! final PR list. The result is cached as a whole.

mod gh;
mod pr_heads;
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) mod store;
#[cfg_attr(not(test), allow(dead_code))]
mod sync;

use std::collections::{HashMap, HashSet};
use std::ffi::OsStr;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::fs::expand_home;

/// Safety caps of the git collection. Exceeding any cap below sets
/// `RawData.truncated`; the survival count caps (`max_survival_prs`,
/// `max_survival_files`) and the 50 files per PR survival cap only thin out
/// quality samples and do not.
///
/// - `max_commits`: more commits (or first-parent entries) in the window
/// - `max_files_per_commit`: a commit (or PR diff) touching more files
/// - `max_branch_commits`: a merge PR carrying more commits
/// - `max_patch_bytes`: the patch stream for duplicate detection; dedupe is skipped
/// - `max_walk_git_calls`: git processes spent deriving PRs from the merge graph
/// - `survival_budget`: wall clock spent on survival blame; only when it is
///   actually hit is the sample cut short and the flag set
///
/// Duplicate detection that cannot run at all (git failed) also sets the flag,
/// because duplicates would then be counted twice.
struct Limits {
    max_commits: usize,
    max_files_per_commit: usize,
    max_branch_commits: usize,
    max_patch_bytes: usize,
    max_walk_git_calls: usize,
    /// Survival samples the newest PRs first, by count, so the same HEAD gives
    /// the same samples on every run.
    max_survival_prs: usize,
    /// Distinct files blamed per collection.
    max_survival_files: usize,
    survival_workers: usize,
    /// Wall-clock safety net only; hitting it sets `truncated`.
    survival_budget: Duration,
}

impl Limits {
    const DEFAULT: Self = Self {
        max_commits: 20_000,
        max_files_per_commit: 2_000,
        max_branch_commits: 2_000,
        max_patch_bytes: 64 * 1024 * 1024,
        max_walk_git_calls: 5_000,
        max_survival_prs: 150,
        max_survival_files: 300,
        survival_workers: 4,
        survival_budget: Duration::from_secs(40),
    };
}

const MAX_SYNC_DEPTH: usize = 32;
const SURVIVAL_MIN_AGE_DAYS: i64 = 30;
const MAX_SURVIVAL_FILES: usize = 50;
const MAX_CONFIG_BYTES: usize = 1024 * 1024;
const CACHE_CAPACITY: usize = 8;
const CONFIG_DIR: &str = ".monoptah";
const CONFIG_FILE: &str = "contributors.json";

// One record per commit, fields separated by NUL. The numstat block (when
// requested) follows the last NUL of the record.
const LOG_FORMAT: &str =
    "--format=%x1e%H%x00%P%x00%an%x00%ae%x00%cn%x00%ce%x00%aI%x00%cI%x00%s%x00%b%x00";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitIdentity {
    pub name: String,
    pub email: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub login: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RawFile {
    pub path: String,
    pub added: u64,
    pub deleted: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RawCommit {
    pub hash: String,
    pub subject: String,
    pub author: GitIdentity,
    pub committer: GitIdentity,
    pub authored_at: String,
    pub committed_at: String,
    pub co_authors: Vec<GitIdentity>,
    pub files: Vec<RawFile>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pr_id: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RawBranchCommit {
    pub authored_at: String,
    pub committed_at: String,
    pub authors: Vec<GitIdentity>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RawPr {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub number: Option<u64>,
    pub title: String,
    /// "merged" | "closed"
    pub state: String,
    /// "gh" | "git"
    pub source: String,
    pub author: GitIdentity,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub committer: Option<GitIdentity>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub merged_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub closed_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub merge_commit: Option<String>,
    pub commit_count: u64,
    pub branch_commits: Vec<RawBranchCommit>,
    pub landing_co_authors: Vec<GitIdentity>,
    pub files: Vec<RawFile>,
    pub closes_bug: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RawReview {
    pub pr_id: String,
    pub reviewer: GitIdentity,
    pub state: String,
    pub submitted_at: String,
    pub comments: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RawSurvival {
    pub pr_id: String,
    pub written: u64,
    pub alive: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RawData {
    pub head: String,
    pub branch: String,
    pub since: String,
    pub until: String,
    pub gh_available: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gh_note: Option<String>,
    pub commits: Vec<RawCommit>,
    pub prs: Vec<RawPr>,
    pub reviews: Vec<RawReview>,
    pub survival: Vec<RawSurvival>,
    pub reverted_pr_ids: Vec<String>,
    pub truncated: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContributorsBranches {
    pub current: String,
    pub branches: Vec<String>,
}

// ---------------------------------------------------------------------------
// git plumbing
// ---------------------------------------------------------------------------

fn git_command(root: &Path) -> Command {
    let mut command = Command::new("git");
    crate::hide_window_console(&mut command);
    command
        .arg("-C")
        .arg(root)
        .args(["-c", "core.quotepath=off", "--no-pager"])
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_OPTIONAL_LOCKS", "0");
    // Tests must not depend on the developer's own git configuration.
    #[cfg(test)]
    command
        .env(
            "GIT_CONFIG_GLOBAL",
            std::env::temp_dir().join("monoptah-contributors-no-gitconfig"),
        )
        .env("GIT_CONFIG_NOSYSTEM", "1");
    command
}

/// A child process that ran to completion.
struct Finished {
    status: std::process::ExitStatus,
    stdout: String,
    stderr: String,
}

/// Why a wait for a child gave up.
enum WaitFailure {
    TimedOut,
    /// The caller's cancel check turned true; the child was killed.
    Cancelled,
    Io(String),
}

/// Asked while a child runs: should it be killed now?
type CancelCheck<'a> = &'a dyn Fn() -> bool;

/// Waits for `child` to exit. The cancel check (when given) is asked on every
/// poll, before the exit status, and a true answer kills the child; so does
/// the deadline (when given). This is the one wait loop of the module.
fn poll_child(
    child: &mut std::process::Child,
    timeout: Option<Duration>,
    cancel: Option<CancelCheck>,
) -> Result<std::process::ExitStatus, WaitFailure> {
    let deadline = timeout.map(|t| Instant::now() + t);
    loop {
        if cancel.is_some_and(|cancelled| cancelled()) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(WaitFailure::Cancelled);
        }
        match child.try_wait() {
            Ok(Some(status)) => return Ok(status),
            Ok(None) if deadline.is_some_and(|d| Instant::now() >= d) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(WaitFailure::TimedOut);
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(e) => return Err(WaitFailure::Io(e.to_string())),
        }
    }
}

/// Waits for a child spawned with piped stdout and stderr. Both pipes are
/// drained on their own threads so a large reply cannot fill them and stall
/// the child; the child is killed when `timeout` passes or when `cancel`
/// (if given) turns true.
fn wait_with_deadline_or_cancel(
    child: &mut std::process::Child,
    timeout: Duration,
    cancel: Option<CancelCheck>,
) -> Result<Finished, WaitFailure> {
    fn drain<R: Read + Send + 'static>(pipe: Option<R>) -> std::sync::mpsc::Receiver<Vec<u8>> {
        let (send, receive) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            if let Some(mut pipe) = pipe {
                let _ = pipe.read_to_end(&mut bytes);
            }
            let _ = send.send(bytes);
        });
        receive
    }
    let stdout = drain(child.stdout.take());
    let stderr = drain(child.stderr.take());
    let status = poll_child(child, Some(timeout), cancel)?;
    // A grandchild that kept a pipe open must not hold the call forever.
    let grace = Duration::from_secs(5);
    Ok(Finished {
        status,
        stdout: String::from_utf8_lossy(&stdout.recv_timeout(grace).unwrap_or_default())
            .into_owned(),
        stderr: String::from_utf8_lossy(&stderr.recv_timeout(grace).unwrap_or_default())
            .into_owned(),
    })
}

fn git<I, S>(root: &Path, args: I) -> Result<String, String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let output = git_command(root)
        .args(args)
        .output()
        .map_err(|e| format!("Could not run git: {e}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(if stderr.is_empty() {
            "git failed".to_string()
        } else {
            stderr
        });
    }
    // Names and subjects may hold bytes that are not UTF-8; keep going.
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn repo_root(cwd: &Path) -> Result<PathBuf, String> {
    let out = git(cwd, ["rev-parse", "--show-toplevel"])
        .map_err(|_| "Not a git repository".to_string())?;
    // Only git's line ending goes: a path may start or end with spaces.
    let trimmed = out.trim_end_matches('\n');
    if trimmed.trim().is_empty() {
        return Err("Not a git repository".to_string());
    }
    Ok(PathBuf::from(trimmed))
}

fn current_branch(root: &Path) -> String {
    git(root, ["symbolic-ref", "--short", "-q", "HEAD"])
        .map(|s| s.trim().to_string())
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "HEAD".to_string())
}

/// Returns the branch name to report and the commit sha it points to.
fn resolve_branch(root: &Path, branch: Option<&str>) -> Result<(String, String), String> {
    let name = match branch.map(str::trim).filter(|b| !b.is_empty()) {
        Some(b) => {
            if b.starts_with('-') {
                return Err(format!("Unknown branch: {b}"));
            }
            b.to_string()
        }
        None => current_branch(root),
    };
    let spec = format!("{name}^{{commit}}");
    let head = git(root, ["rev-parse", "--verify", "--quiet", spec.as_str()])
        .map_err(|_| format!("Unknown branch or no commits yet: {name}"))?;
    let head = head.trim().to_string();
    if head.is_empty() {
        return Err(format!("Unknown branch or no commits yet: {name}"));
    }
    Ok((name, head))
}

// ---------------------------------------------------------------------------
// dates
// ---------------------------------------------------------------------------

struct IsoTime {
    year: i64,
    month: i64,
    day: i64,
    hour: i64,
    minute: i64,
    second: i64,
    offset_minutes: Option<i64>,
    date_only: bool,
}

fn digits(s: &str, range: std::ops::Range<usize>) -> Option<i64> {
    let part = s.get(range)?;
    if part.is_empty() || !part.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    part.parse().ok()
}

fn days_in_month(year: i64, month: i64) -> i64 {
    match month {
        2 if (year % 4 == 0 && year % 100 != 0) || year % 400 == 0 => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// `None` = malformed; `Some(None)` = no zone given; `Some(Some(m))` = offset.
fn parse_zone(zone: &str) -> Option<Option<i64>> {
    if zone.is_empty() {
        return Some(None);
    }
    if zone == "Z" || zone == "z" {
        return Some(Some(0));
    }
    let bytes = zone.as_bytes();
    let sign = match bytes[0] {
        b'+' => 1,
        b'-' => -1,
        _ => return None,
    };
    let hours = digits(zone, 1..3)?;
    let minutes = match zone.len() {
        3 => 0,
        5 => digits(zone, 3..5)?,
        6 if bytes[3] == b':' => digits(zone, 4..6)?,
        _ => return None,
    };
    if hours > 23 || minutes > 59 {
        return None;
    }
    Some(Some(sign * (hours * 60 + minutes)))
}

fn parse_iso(raw: &str) -> Option<IsoTime> {
    let s = raw.trim();
    let bytes = s.as_bytes();
    if bytes.len() < 10 || bytes[4] != b'-' || bytes[7] != b'-' {
        return None;
    }
    let year = digits(s, 0..4)?;
    let month = digits(s, 5..7)?;
    let day = digits(s, 8..10)?;
    if !(1..=12).contains(&month) || day < 1 || day > days_in_month(year, month) {
        return None;
    }
    let mut time = IsoTime {
        year,
        month,
        day,
        hour: 0,
        minute: 0,
        second: 0,
        offset_minutes: None,
        date_only: true,
    };
    if bytes.len() == 10 {
        return Some(time);
    }
    if bytes[10] != b'T' && bytes[10] != b' ' {
        return None;
    }
    let t = s.get(11..)?;
    let tb = t.as_bytes();
    if tb.len() < 5 || tb[2] != b':' {
        return None;
    }
    time.date_only = false;
    time.hour = digits(t, 0..2)?;
    time.minute = digits(t, 3..5)?;
    let mut i = 5;
    if tb.get(i) == Some(&b':') {
        time.second = digits(t, i + 1..i + 3)?;
        i += 3;
    }
    if tb.get(i) == Some(&b'.') {
        i += 1;
        while tb.get(i).is_some_and(|c| c.is_ascii_digit()) {
            i += 1;
        }
    }
    if time.hour > 23 || time.minute > 59 || time.second > 60 {
        return None;
    }
    time.offset_minutes = parse_zone(t.get(i..)?)?;
    Some(time)
}

fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// Unix seconds. A time without a zone is read as UTC (only used for coarse
/// comparisons such as the 30 day survival cutoff).
fn parse_iso_epoch(raw: &str) -> Option<i64> {
    let t = parse_iso(raw)?;
    let days = days_from_civil(t.year, t.month, t.day);
    let secs = days * 86_400 + t.hour * 3_600 + t.minute * 60 + t.second;
    Some(secs - t.offset_minutes.unwrap_or(0) * 60)
}

fn epoch_to_iso(secs: i64) -> String {
    let (y, m, d) = civil_from_days(secs.div_euclid(86_400));
    let rem = secs.rem_euclid(86_400);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3_600,
        rem % 3_600 / 60,
        rem % 60
    )
}

fn now_epoch() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Value for git's `--since` / `--until`. A bare date means the start (or the
/// end, when `end_of_day`) of that local day: git would otherwise fill in the
/// current time of day. A date with an explicit zone becomes an exact instant.
fn git_date_arg(raw: &str, end_of_day: bool) -> Option<String> {
    let t = parse_iso(raw)?;
    if t.offset_minutes.is_some() {
        return Some(format!("@{}", parse_iso_epoch(raw)?));
    }
    let (h, m, s) = if t.date_only {
        if end_of_day {
            (23, 59, 59)
        } else {
            (0, 0, 0)
        }
    } else {
        (t.hour, t.minute, t.second)
    };
    Some(format!(
        "{:04}-{:02}-{:02}T{h:02}:{m:02}:{s:02}",
        t.year, t.month, t.day
    ))
}

/// Epoch of a window edge; a bare `until` date means the last second of its day.
fn window_epoch(raw: &str, end_of_day: bool) -> Option<i64> {
    let epoch = parse_iso_epoch(raw)?;
    let bare = parse_iso(raw)?.date_only;
    Some(if bare && end_of_day {
        epoch + 86_399
    } else {
        epoch
    })
}

/// Epoch git derives from a `--since` / `--until` argument: `rev-parse` prints
/// it as `--max-age=<n>` / `--min-age=<n>`.
fn git_edge_epoch(root: &Path, flag: &str, arg: &str, prefix: &str) -> Option<i64> {
    let out = git(root, ["rev-parse", &format!("{flag}={arg}")]).ok()?;
    out.lines()
        .find_map(|line| line.strip_prefix(prefix)?.trim().parse().ok())
}

/// The requested time window in the forms the collection needs.
struct Window {
    since_arg: String,
    until_arg: String,
    since_epoch: i64,
    until_epoch: i64,
    since_echo: String,
    until_echo: String,
    /// What distinguishes `until` in the cache: the exact text given, or for an
    /// open end `now:` plus the UTC calendar day of the request (taken from the
    /// same instant as `until_echo`). The key is coarse on purpose: a new HEAD
    /// or a new `since` (which follows the local day) already misses, so only
    /// changes seen on GitHub alone (new reviews, PRs merged but not pulled)
    /// can stay stale, until the day turns or the refresh button forces a reload.
    until_key: String,
    /// The edge has no zone, so git reads it in the local zone and the epoch
    /// above is only a UTC approximation until `for_repo` asks git.
    since_local: bool,
    until_local: bool,
}

impl Window {
    fn new(since: &str, until: Option<&str>) -> Result<Self, String> {
        let since = since.trim();
        let invalid = |label: &str, raw: &str| format!("Invalid {label} date: {raw}");
        let since_arg = git_date_arg(since, false).ok_or_else(|| invalid("since", since))?;
        let since_epoch = window_epoch(since, false).ok_or_else(|| invalid("since", since))?;
        let (until_arg, until_epoch, until_echo, until_key) =
            match until.map(str::trim).filter(|u| !u.is_empty()) {
                Some(until) => {
                    let arg = git_date_arg(until, true).ok_or_else(|| invalid("until", until))?;
                    let epoch = window_epoch(until, true).ok_or_else(|| invalid("until", until))?;
                    (arg, epoch, until.to_string(), until.to_string())
                }
                None => {
                    let now = now_epoch();
                    let iso = epoch_to_iso(now);
                    let key = format!("now:{}", &iso[..10]);
                    (format!("@{now}"), now, iso, key)
                }
            };
        if since_epoch > until_epoch {
            return Err("The start date is after the end date.".to_string());
        }
        Ok(Self {
            since_arg,
            until_arg,
            since_epoch,
            until_epoch,
            since_echo: since.to_string(),
            until_echo,
            until_key,
            since_local: parse_iso(since).is_some_and(|t| t.offset_minutes.is_none()),
            until_local: until
                .map(str::trim)
                .filter(|u| !u.is_empty())
                .and_then(parse_iso)
                .is_some_and(|t| t.offset_minutes.is_none()),
        })
    }

    /// Like `new`, but zone-less edges get the exact instant git will use
    /// (the local zone), so window checks agree with the git logs.
    fn for_repo(root: &Path, since: &str, until: Option<&str>) -> Result<Self, String> {
        let mut window = Self::new(since, until)?;
        if window.since_local {
            if let Some(epoch) = git_edge_epoch(root, "--since", &window.since_arg, "--max-age=") {
                window.since_epoch = epoch;
            }
        }
        if window.until_local {
            if let Some(epoch) = git_edge_epoch(root, "--until", &window.until_arg, "--min-age=") {
                window.until_epoch = epoch;
            }
        }
        if window.since_epoch > window.until_epoch {
            return Err("The start date is after the end date.".to_string());
        }
        Ok(window)
    }

    fn log_args(&self) -> [String; 2] {
        [
            format!("--since={}", self.since_arg),
            format!("--until={}", self.until_arg),
        ]
    }
}

// ---------------------------------------------------------------------------
// parsing
// ---------------------------------------------------------------------------

fn login_from_email(email: &str) -> Option<String> {
    const SUFFIX: &str = "@users.noreply.github.com";
    let lower = email.trim().to_ascii_lowercase();
    let local = lower.strip_suffix(SUFFIX)?;
    let original_local = &email.trim()[..local.len()];
    let login = original_local
        .split_once('+')
        .map_or(original_local, |(_, login)| login);
    (!login.is_empty()).then(|| login.to_string())
}

fn identity(name: &str, email: &str) -> GitIdentity {
    let email = email.trim();
    GitIdentity {
        name: name.trim().to_string(),
        email: email.to_string(),
        login: login_from_email(email),
    }
}

/// `Co-authored-by: Name <email>` trailers. The key is case-insensitive and
/// whitespace tolerant; lines without a usable `<email>` are ignored.
fn parse_co_authors(body: &str) -> Vec<GitIdentity> {
    let mut found: Vec<GitIdentity> = Vec::new();
    for line in body.lines() {
        let Some((key, rest)) = line.trim().split_once(':') else {
            continue;
        };
        if !key.trim().eq_ignore_ascii_case("co-authored-by") {
            continue;
        }
        let rest = rest.trim();
        let (Some(open), Some(close)) = (rest.rfind('<'), rest.rfind('>')) else {
            continue;
        };
        if close < open {
            continue;
        }
        let email = rest[open + 1..close].trim();
        if !email.contains('@') || email.contains(char::is_whitespace) {
            continue;
        }
        let mut name = rest[..open].trim().trim_matches('"').trim().to_string();
        if name.is_empty() {
            name = email.split('@').next().unwrap_or(email).to_string();
        }
        let who = identity(&name, email);
        if !found.iter().any(|f| f.email.eq_ignore_ascii_case(email)) {
            found.push(who);
        }
    }
    found
}

/// Parses `git ... --numstat -z` output (NUL separated, paths never quoted).
/// A rename entry is `added\tdeleted\t` followed by the old and new path as two
/// more fields; the new path is kept. Binary files (`-`) and files without any
/// changed line (pure renames, mode changes, whitespace-only edits under `-w`)
/// are omitted. Returns the files and whether more than `max_files` were found.
fn parse_numstat_z(text: &str, max_files: usize) -> (Vec<RawFile>, bool) {
    let mut files = Vec::new();
    let mut truncated = false;
    let mut fields = text.split('\0');
    while let Some(field) = fields.next() {
        let mut parts = field.trim_start_matches('\n').splitn(3, '\t');
        let (Some(added), Some(deleted), Some(path)) = (parts.next(), parts.next(), parts.next())
        else {
            continue;
        };
        let path = if path.is_empty() {
            let _old = fields.next();
            match fields.next() {
                Some(new) => new,
                None => break,
            }
        } else {
            path
        };
        let (Ok(added), Ok(deleted)) = (added.parse::<u64>(), deleted.parse::<u64>()) else {
            continue;
        };
        if (added == 0 && deleted == 0) || path.is_empty() {
            continue;
        }
        if files.len() >= max_files {
            truncated = true;
            break;
        }
        files.push(RawFile {
            path: path.to_string(),
            added,
            deleted,
        });
    }
    (files, truncated)
}

struct ParsedCommit {
    hash: String,
    parents: Vec<String>,
    author: GitIdentity,
    committer: GitIdentity,
    authored_at: String,
    committed_at: String,
    subject: String,
    body: String,
    co_authors: Vec<GitIdentity>,
    files: Vec<RawFile>,
}

/// A record starts with `\x1e`, a full object id and a NUL. Any other `\x1e`
/// (a stray control byte in a message) belongs to the previous record and is
/// dropped, so a commit is never lost to a separator inside its text.
fn split_records(output: &str) -> Vec<String> {
    let mut records: Vec<String> = Vec::new();
    for chunk in output.split('\u{1e}').skip(1) {
        let hex = chunk.bytes().take_while(u8::is_ascii_hexdigit).count();
        let starts_record = (hex == 40 || hex == 64) && chunk.as_bytes().get(hex) == Some(&0);
        if starts_record {
            records.push(chunk.to_string());
        } else if let Some(last) = records.last_mut() {
            last.push_str(chunk);
        }
    }
    records
}

fn looks_like_numstat(field: &str) -> bool {
    let mut parts = field.trim_start_matches('\n').splitn(3, '\t');
    let count = |p: Option<&str>| {
        p.is_some_and(|p| p == "-" || (!p.is_empty() && p.bytes().all(|b| b.is_ascii_digit())))
    };
    count(parts.next()) && count(parts.next()) && parts.next().is_some()
}

fn parse_record(record: &str, max_files: usize) -> Option<(ParsedCommit, bool)> {
    let fields: Vec<&str> = record.split('\0').collect();
    if fields.len() < 9 {
        return None;
    }
    // Fields 0..8 are fixed; the message follows, then the numstat block. A NUL
    // inside the message (git refuses them, but hand-made objects exist) only
    // shifts where the message ends.
    // Field 9 is the whole message for any real commit, so a body that happens
    // to look like a numstat line is never mistaken for files.
    let message_end = fields
        .iter()
        .skip(10)
        .position(|f| looks_like_numstat(f))
        .map_or(fields.len(), |p| p + 10);
    let body = fields[9..message_end].join("\n");
    let (files, capped) = parse_numstat_z(&fields[message_end..].join("\0"), max_files);
    let author = identity(fields[2], fields[3]);
    let mut co_authors = parse_co_authors(&body);
    co_authors.retain(|c| !c.email.eq_ignore_ascii_case(&author.email));
    let commit = ParsedCommit {
        hash: fields[0].trim().to_string(),
        parents: fields[1].split_whitespace().map(str::to_string).collect(),
        author,
        committer: identity(fields[4], fields[5]),
        authored_at: fields[6].to_string(),
        committed_at: fields[7].to_string(),
        subject: fields[8].to_string(),
        body,
        co_authors,
        files,
    };
    Some((commit, capped))
}

/// Parses `git log` output made with `LOG_FORMAT` (plus `--numstat -z` when
/// files are wanted). Returns the commits and whether a file cap was exceeded.
fn parse_log(output: &str, max_files: usize) -> (Vec<ParsedCommit>, bool) {
    let mut commits = Vec::new();
    let mut truncated = false;
    for record in split_records(output) {
        if let Some((commit, capped)) = parse_record(&record, max_files) {
            truncated |= capped;
            commits.push(commit);
        }
    }
    (commits, truncated)
}

fn short_hash(hash: &str) -> String {
    hash.chars().take(7).collect()
}

fn leading_number(text: &str) -> Option<u64> {
    let digits: String = text.chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}

/// `Merge pull request #12 from ...` or a subject ending in `(#12)`.
fn pr_number_from_subject(subject: &str) -> Option<u64> {
    if let Some(rest) = subject.strip_prefix("Merge pull request #") {
        return leading_number(rest);
    }
    let body = subject.trim_end().strip_suffix(')')?;
    let (_, number) = body.rsplit_once("(#")?;
    if number.is_empty() || !number.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    number.parse().ok()
}

fn strip_pr_suffix(subject: &str) -> String {
    let trimmed = subject.trim_end();
    if pr_number_from_subject(trimmed).is_some() && !trimmed.starts_with("Merge pull request #") {
        if let Some((title, _)) = trimmed.rsplit_once("(#") {
            let title = title.trim_end();
            if !title.is_empty() {
                return title.to_string();
            }
        }
    }
    trimmed.to_string()
}

fn merge_title(subject: &str, body: &str) -> String {
    if subject.starts_with("Merge pull request") {
        if let Some(line) = body.lines().map(str::trim).find(|l| !l.is_empty()) {
            return line.to_string();
        }
    }
    subject.trim().to_string()
}

/// GitHub squash bodies list the squashed commits as `* subject` lines.
fn squash_commit_count(body: &str) -> u64 {
    (body.lines().filter(|l| l.starts_with("* ")).count() as u64).max(1)
}

/// For a `Revert "..."` commit, the full sha named by `This reverts commit`.
fn reverted_commit(subject: &str, body: &str) -> Option<String> {
    if !subject.starts_with("Revert \"") {
        return None;
    }
    let (_, rest) = body.split_once("This reverts commit ")?;
    let sha: String = rest.chars().take_while(char::is_ascii_hexdigit).collect();
    (sha.len() >= 40).then_some(sha)
}

/// Which commits to drop so each patch-id keeps only its earliest commit
/// (committer date, ties by first appearance).
fn select_duplicates(
    pairs: &[(String, String)],
    committed: &HashMap<String, i64>,
) -> HashSet<String> {
    let mut best: HashMap<&str, (i64, &str)> = HashMap::new();
    for (patch, hash) in pairs {
        let Some(&epoch) = committed.get(hash) else {
            continue;
        };
        match best.get(patch.as_str()) {
            Some(&(kept, _)) if kept <= epoch => {}
            _ => {
                best.insert(patch.as_str(), (epoch, hash.as_str()));
            }
        }
    }
    let mut dropped = HashSet::new();
    for (patch, hash) in pairs {
        if !committed.contains_key(hash) {
            continue;
        }
        if best
            .get(patch.as_str())
            .is_some_and(|&(_, kept)| kept != hash)
        {
            dropped.insert(hash.clone());
        }
    }
    dropped
}

// ---------------------------------------------------------------------------
// collection
// ---------------------------------------------------------------------------

enum Pump {
    Done,
    TooBig,
    Failed(String),
}

/// Copies `input` to `output` in chunks, giving up past `max_bytes`.
fn pump<R: Read, W: Write>(input: &mut R, output: &mut W, max_bytes: usize) -> Pump {
    let mut buffer = vec![0u8; 64 * 1024];
    let mut total = 0usize;
    loop {
        let read = match input.read(&mut buffer) {
            Ok(0) => return Pump::Done,
            Ok(n) => n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Pump::Failed(format!("Could not read git output: {e}")),
        };
        total += read;
        if total > max_bytes {
            return Pump::TooBig;
        }
        if let Err(e) = output.write_all(&buffer[..read]) {
            return Pump::Failed(format!("Could not pass git output on: {e}"));
        }
    }
}

fn stop(child: &mut std::process::Child) {
    let _ = child.kill();
    let _ = child.wait();
}

/// Streams `git log -p` into `git patch-id --stable` without holding the patch
/// text. Returns `None` when the patch text exceeds the byte cap.
fn patch_ids(
    root: &Path,
    head: &str,
    window: &Window,
    limits: &Limits,
) -> Result<Option<Vec<(String, String)>>, String> {
    let mut log = git_command(root);
    log.args([
        "log",
        "-p",
        "--no-merges",
        "--no-color",
        "--no-ext-diff",
        "--no-textconv",
        "--no-show-signature",
        "-n",
    ])
    .arg((limits.max_commits + 1).to_string())
    .args(window.log_args())
    .args([head, "--"]);
    match pipe_patch_ids(root, log, None, limits.max_patch_bytes, None)? {
        PatchPipe::Ids(ids) => Ok(Some(ids)),
        PatchPipe::TooBig => Ok(None),
        PatchPipe::Cancelled => Err("cancelled".to_string()),
    }
}

/// How a run of the patch-id pipe ended.
enum PatchPipe {
    /// `(patch id, commit)` pairs
    Ids(Vec<(String, String)>),
    /// The patch text exceeded the byte cap.
    TooBig,
    /// The cancel check turned true; both children were killed.
    Cancelled,
}

/// Runs `log` (a `git log -p` command) into `git patch-id --stable`. `input`
/// is written to the log's stdin, one line each, for `--stdin`. Every exit
/// path kills a still-running `git log` and waits for both children, so a
/// failure never leaves a process blocked on a full pipe. `cancel` is asked
/// while the pipe runs; a true answer kills both children.
fn pipe_patch_ids(
    root: &Path,
    mut log: Command,
    input: Option<&[String]>,
    max_bytes: usize,
    cancel: Option<CancelCheck>,
) -> Result<PatchPipe, String> {
    let spawn_error = |e: std::io::Error| format!("Could not run git: {e}");
    log.stdout(Stdio::piped()).stderr(Stdio::null());
    if input.is_some() {
        log.stdin(Stdio::piped());
    }
    let mut log = log.spawn().map_err(spawn_error)?;
    let mut ids = match git_command(root)
        .args(["patch-id", "--stable"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(child) => child,
        Err(e) => {
            stop(&mut log);
            return Err(spawn_error(e));
        }
    };
    let (Some(mut log_out), Some(mut ids_in), Some(mut ids_out)) =
        (log.stdout.take(), ids.stdin.take(), ids.stdout.take())
    else {
        stop(&mut log);
        stop(&mut ids);
        return Err("Could not connect git processes".to_string());
    };
    // Feed the revisions from their own thread: git may answer before it has
    // read them all. A write error only means git stopped early.
    let feeder = match (input, log.stdin.take()) {
        (Some(lines), Some(mut stdin)) => {
            let text: String = lines.iter().map(|l| format!("{l}\n")).collect();
            Some(std::thread::spawn(move || {
                let _ = stdin.write_all(text.as_bytes());
            }))
        }
        _ => None,
    };
    // Drain patch-id output on its own thread so neither pipe can fill up.
    let reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = ids_out.read_to_end(&mut bytes);
        String::from_utf8_lossy(&bytes).into_owned()
    });
    // The copy runs on its own thread so this one can watch for a cancel; it
    // closes patch-id's stdin when it ends, which lets patch-id exit.
    let pumper = std::thread::spawn(move || {
        let outcome = pump(&mut log_out, &mut ids_in, max_bytes);
        drop(ids_in);
        outcome
    });
    let ids_status = poll_child(&mut ids, None, cancel);
    let cancelled = matches!(ids_status, Err(WaitFailure::Cancelled));
    if cancelled {
        // patch-id is gone; killing git log closes the pipe the pump reads.
        stop(&mut log);
    }
    let outcome = pumper
        .join()
        .unwrap_or_else(|_| Pump::Failed("Could not read git output".to_string()));
    if cancelled || !matches!(outcome, Pump::Done) {
        let _ = log.kill();
    }
    let log_status = log.wait();
    if let Some(feeder) = feeder {
        let _ = feeder.join();
    }
    let text = reader.join().unwrap_or_default();
    if cancelled {
        return Ok(PatchPipe::Cancelled);
    }
    match outcome {
        Pump::TooBig => Ok(PatchPipe::TooBig),
        Pump::Failed(error) => Err(error),
        Pump::Done => {
            let ok = log_status.is_ok_and(|s| s.success()) && ids_status.is_ok_and(|s| s.success());
            if !ok {
                return Err("git could not compute patch ids".to_string());
            }
            Ok(PatchPipe::Ids(
                text.lines()
                    .filter_map(|line| {
                        let (patch, commit) = line.split_once(' ')?;
                        Some((patch.to_string(), commit.trim().to_string()))
                    })
                    .collect(),
            ))
        }
    }
}

struct PrDraft {
    pr: RawPr,
    /// Landing commit plus every commit that came in with it.
    hashes: HashSet<String>,
}

struct Derived {
    drafts: Vec<PrDraft>,
    /// commit hash -> id of the PR it landed in
    commit_pr: HashMap<String, String>,
    /// git processes the merge graph walk spent
    git_calls: usize,
}

fn unique_id(candidate: Option<String>, hash: &str, used: &mut HashSet<String>) -> String {
    for id in [candidate, Some(short_hash(hash)), Some(hash.to_string())]
        .into_iter()
        .flatten()
    {
        if used.insert(id.clone()) {
            return id;
        }
    }
    hash.to_string()
}

/// Only GitHub's `Merge pull request #N from ...` is one PR. Any other merge
/// (`Merge branch 'x'`, `git pull`, upstream syncs) is plumbing: the PRs it
/// brought in are derived from the commits behind it.
fn is_pr_merge(subject: &str) -> bool {
    subject.starts_with("Merge pull request #") && pr_number_from_subject(subject).is_some()
}

/// `numbered` is false for PRs that reached the branch through a sync merge:
/// their `#N` belongs to another repository (an upstream), so they are keyed by
/// hash and carry no number.
fn single_parent_pr(landing: &ParsedCommit, used: &mut HashSet<String>, numbered: bool) -> PrDraft {
    let parsed = pr_number_from_subject(&landing.subject);
    let squash = parsed.is_some();
    let candidate = parsed.filter(|_| numbered);
    let id = unique_id(candidate.map(|n| n.to_string()), &landing.hash, used);
    let pr = RawPr {
        number: candidate.filter(|n| n.to_string() == id),
        id,
        title: if squash {
            strip_pr_suffix(&landing.subject)
        } else {
            landing.subject.trim().to_string()
        },
        state: "merged".to_string(),
        source: "git".to_string(),
        author: landing.author.clone(),
        committer: Some(landing.committer.clone()),
        merged_at: Some(landing.committed_at.clone()),
        closed_at: None,
        merge_commit: Some(landing.hash.clone()),
        commit_count: if squash {
            squash_commit_count(&landing.body)
        } else {
            1
        },
        branch_commits: Vec::new(),
        landing_co_authors: landing.co_authors.clone(),
        files: landing.files.clone(),
        closes_bug: false,
    };
    PrDraft {
        pr,
        hashes: HashSet::from([landing.hash.clone()]),
    }
}

struct Deriver<'a> {
    root: &'a Path,
    window: &'a Window,
    limits: &'a Limits,
    by_hash: HashMap<&'a str, &'a ParsedCommit>,
    dropped: &'a HashSet<String>,
    used: HashSet<String>,
    /// First-parent entries already turned into PR units (guards overlaps).
    seen: HashSet<String>,
    derived: Derived,
    truncated: bool,
}

impl Deriver<'_> {
    /// Every git process of the walk goes through here so the budget holds.
    fn run<I, S>(&mut self, args: I) -> Result<String, String>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        if self.derived.git_calls >= self.limits.max_walk_git_calls {
            self.truncated = true;
            return Err("Too many git calls while walking merges".to_string());
        }
        self.derived.git_calls += 1;
        git(self.root, args)
    }

    fn log_entries(&mut self, args: &[String]) -> Result<Vec<ParsedCommit>, String> {
        let mut entries = parse_log(&self.run(args)?, 0).0;
        if entries.len() > self.limits.max_commits {
            entries.truncate(self.limits.max_commits);
            self.truncated = true;
        }
        Ok(entries)
    }

    fn in_window(&self, commit: &ParsedCommit) -> bool {
        parse_iso_epoch(&commit.committed_at)
            .is_some_and(|e| e >= self.window.since_epoch && e <= self.window.until_epoch)
    }

    /// `excludes` are the `^<rev>` arguments of the sync merges being walked:
    /// what they already had must not be credited again by anything nested.
    fn walk(&mut self, entries: &[ParsedCommit], depth: usize, excludes: &[String]) {
        for entry in entries {
            if !self.seen.insert(entry.hash.clone()) {
                continue;
            }
            // The main chain (depth 0) is already limited to the window by git;
            // anything older found deeper contributes nothing and is not walked.
            let relevant = depth == 0 || self.in_window(entry);
            if entry.parents.len() < 2 {
                self.single_unit(entry, depth == 0);
            } else if !relevant {
                continue;
            } else if is_pr_merge(&entry.subject) || depth >= MAX_SYNC_DEPTH {
                self.merge_unit(entry, depth == 0, excludes);
            } else {
                self.sync_merge(entry, depth, excludes);
            }
        }
    }

    /// A merge that is not a PR: walk what its other parents brought in.
    fn sync_merge(&mut self, merge: &ParsedCommit, depth: usize, excludes: &[String]) {
        let mut nested = excludes.to_vec();
        nested.push(format!("^{}", merge.parents[0]));
        for tip in &merge.parents[1..] {
            let mut args: Vec<String> = [
                "log",
                "--first-parent",
                "--no-show-signature",
                "-n",
                &(self.limits.max_commits + 1).to_string(),
                LOG_FORMAT,
            ]
            .map(str::to_string)
            .to_vec();
            // Nothing older than the window is credited, so do not read it.
            args.extend(self.window.log_args());
            args.extend(nested.iter().cloned());
            args.extend([tip.clone(), "--".to_string()]);
            let entries = self.log_entries(&args).unwrap_or_default();
            self.walk(&entries, depth + 1, &nested);
        }
    }

    fn single_unit(&mut self, entry: &ParsedCommit, numbered: bool) {
        let Some(full) = self.by_hash.get(entry.hash.as_str()).copied() else {
            return;
        };
        if self.dropped.contains(&full.hash) {
            return;
        }
        let draft = single_parent_pr(full, &mut self.used, numbered);
        self.derived
            .commit_pr
            .insert(full.hash.clone(), draft.pr.id.clone());
        self.derived.drafts.push(draft);
    }

    fn tip_author(&mut self, merge: &ParsedCommit) -> GitIdentity {
        let tip = merge.parents[1].as_str();
        if let Some(known) = self.by_hash.get(tip) {
            return known.author.clone();
        }
        let args = ["log", "-1", "--no-show-signature", LOG_FORMAT, tip, "--"];
        self.run(args)
            .ok()
            .and_then(|out| parse_log(&out, 0).0.into_iter().next())
            .map_or_else(|| merge.author.clone(), |c| c.author)
    }

    /// Does the branch of a PR merge hold more commits than `left`, the ones
    /// not credited elsewhere?
    fn partly_credited_elsewhere(&mut self, merge: &ParsedCommit, left: usize) -> bool {
        let mut args: Vec<String> = ["rev-list", "--count", "--no-merges"]
            .map(str::to_string)
            .to_vec();
        args.push(format!("^{}", merge.parents[0]));
        args.extend(merge.parents[1..].iter().cloned());
        args.push("--".to_string());
        self.run(&args)
            .ok()
            .and_then(|out| out.trim().parse::<usize>().ok())
            .is_some_and(|all| all > left)
    }

    fn merge_unit(&mut self, merge: &ParsedCommit, numbered: bool, excludes: &[String]) {
        let first = &merge.parents[0];
        let mut args: Vec<String> = [
            "log",
            "--no-merges",
            "--no-show-signature",
            "-n",
            &(self.limits.max_branch_commits + 1).to_string(),
            LOG_FORMAT,
        ]
        .map(str::to_string)
        .to_vec();
        // Inside a sync merge the exclusions may leave only part of the branch.
        // Then the merge diff would count lines of commits credited elsewhere,
        // so the files come from the commits that are left (see below).
        let excluding = !excludes.is_empty();
        if excluding {
            args.extend(["--numstat", "-z", "-w", "-M", "--no-color"].map(str::to_string));
        }
        args.push(format!("^{first}"));
        args.extend(excludes.iter().cloned());
        args.extend(merge.parents[1..].iter().cloned());
        args.push("--".to_string());
        let file_cap = if excluding {
            self.limits.max_files_per_commit
        } else {
            0
        };
        let listed = self.run(&args).map(|out| parse_log(&out, file_cap));
        let mut branch = match listed {
            // Everything it merged was already credited elsewhere.
            Ok((commits, _)) if commits.is_empty() => return,
            Ok((commits, capped)) => {
                self.truncated |= capped;
                commits
            }
            Err(_) => Vec::new(),
        };
        let partial = excluding && self.partly_credited_elsewhere(merge, branch.len());
        if branch.len() > self.limits.max_branch_commits {
            branch.truncate(self.limits.max_branch_commits);
            self.truncated = true;
        }

        let tip_author = self.tip_author(merge);
        let mut hashes: HashSet<String> = branch.iter().map(|c| c.hash.clone()).collect();
        hashes.insert(merge.hash.clone());
        let counted: Vec<&ParsedCommit> = branch
            .iter()
            .filter(|c| !self.dropped.contains(&c.hash))
            .collect();
        let branch_commits = counted
            .iter()
            .map(|c| RawBranchCommit {
                authored_at: c.authored_at.clone(),
                committed_at: c.committed_at.clone(),
                authors: std::iter::once(c.author.clone())
                    .chain(c.co_authors.iter().cloned())
                    .collect(),
            })
            .collect();
        let commit_count = (counted.len() as u64).max(1);

        let diff = [
            "diff",
            "--numstat",
            "-z",
            "-w",
            "-M",
            "--no-color",
            "--no-ext-diff",
            first.as_str(),
            merge.hash.as_str(),
            "--",
        ];
        let files = if partial {
            sum_files(&counted)
        } else {
            match self.run(diff) {
                Ok(out) => {
                    let (files, capped) = parse_numstat_z(&out, self.limits.max_files_per_commit);
                    self.truncated |= capped;
                    files
                }
                Err(_) => Vec::new(),
            }
        };

        let candidate = pr_number_from_subject(&merge.subject).filter(|_| numbered);
        let id = unique_id(
            candidate.map(|n| n.to_string()),
            &merge.hash,
            &mut self.used,
        );
        let pr = RawPr {
            number: candidate.filter(|n| n.to_string() == id),
            id: id.clone(),
            title: merge_title(&merge.subject, &merge.body),
            state: "merged".to_string(),
            source: "git".to_string(),
            author: tip_author,
            committer: Some(merge.committer.clone()),
            merged_at: Some(merge.committed_at.clone()),
            closed_at: None,
            merge_commit: Some(merge.hash.clone()),
            commit_count,
            branch_commits,
            landing_co_authors: merge.co_authors.clone(),
            files,
            closes_bug: false,
        };
        for commit in &branch {
            self.derived
                .commit_pr
                .insert(commit.hash.clone(), id.clone());
        }
        self.derived.drafts.push(PrDraft { pr, hashes });
    }
}

/// Per-file totals over the given commits, in order of first appearance. Used
/// where a PR's final diff would include commits credited to another PR; it
/// can differ from the final diff when the commits rewrite each other's lines.
fn sum_files(commits: &[&ParsedCommit]) -> Vec<RawFile> {
    let mut files: Vec<RawFile> = Vec::new();
    for file in commits.iter().flat_map(|c| &c.files) {
        match files.iter_mut().find(|f| f.path == file.path) {
            Some(total) => {
                total.added += file.added;
                total.deleted += file.deleted;
            }
            None => files.push(file.clone()),
        }
    }
    files
}

/// PRs recovered from the first-parent history of the branch. This is the
/// git-only fallback; the GitHub step may replace or augment it.
///
/// PRs on the branch's own first-parent chain keep their `#N` as id and number;
/// PRs that arrive through a sync merge are keyed by hash (see `single_parent_pr`).
fn derive_prs_from_git(
    root: &Path,
    head: &str,
    window: &Window,
    commits: &[ParsedCommit],
    dropped: &HashSet<String>,
    limits: &Limits,
    truncated: &mut bool,
) -> Result<Derived, String> {
    let mut deriver = Deriver {
        root,
        window,
        limits,
        by_hash: commits.iter().map(|c| (c.hash.as_str(), c)).collect(),
        dropped,
        used: HashSet::new(),
        seen: HashSet::new(),
        derived: Derived {
            drafts: Vec::new(),
            commit_pr: HashMap::new(),
            git_calls: 0,
        },
        truncated: false,
    };
    let [since, until] = window.log_args();
    let args: Vec<String> = [
        "log",
        "--first-parent",
        "--no-show-signature",
        "-n",
        &(limits.max_commits + 1).to_string(),
        &since,
        &until,
        LOG_FORMAT,
        head,
        "--",
    ]
    .map(str::to_string)
    .to_vec();
    let chain = deriver.log_entries(&args)?;
    deriver.walk(&chain, 0, &[]);
    *truncated |= deriver.truncated;
    Ok(deriver.derived)
}

fn reverted_pr_ids(commits: &[ParsedCommit], derived: &Derived) -> Vec<String> {
    let mut by_commit: HashMap<&str, &str> = HashMap::new();
    for draft in &derived.drafts {
        for hash in &draft.hashes {
            by_commit.insert(hash.as_str(), draft.pr.id.as_str());
        }
    }
    let mut reverted: Vec<String> = Vec::new();
    for commit in commits {
        let Some(sha) = reverted_commit(&commit.subject, &commit.body) else {
            continue;
        };
        let Some(&pr_id) = by_commit.get(sha.as_str()) else {
            continue;
        };
        // A revert is itself a PR; it must not flag its own PR.
        if derived.commit_pr.get(&commit.hash).map(String::as_str) == Some(pr_id) {
            continue;
        }
        if !reverted.iter().any(|r| r == pr_id) {
            reverted.push(pr_id.to_string());
        }
    }
    reverted
}

struct SurvivalCandidate {
    pr_id: String,
    /// A landing commit that must be in the history of the tip, or the sample
    /// would blame lines that were never there. Only GitHub PRs set it.
    landing: Option<String>,
    merged_epoch: i64,
    hashes: HashSet<String>,
    files: Vec<RawFile>,
}

/// Lines per blamed commit for one file at `head`; `None` when the file cannot
/// be blamed (deleted, renamed away, binary).
fn blame_counts(root: &Path, head: &str, path: &str) -> Option<HashMap<String, u64>> {
    // `--porcelain` repeats a `<sha> <orig> <final>` header for every line,
    // which is all that is needed here, with far less output than
    // `--line-porcelain`.
    let out = git(
        root,
        [
            "blame",
            "-w",
            "-M",
            "--porcelain",
            "--no-textconv",
            head,
            "--",
            path,
        ],
    )
    .ok()?;
    let mut counts: HashMap<String, u64> = HashMap::new();
    for line in out.lines() {
        let bytes = line.as_bytes();
        if bytes.len() > 41 && bytes[40] == b' ' && bytes[..40].iter().all(u8::is_ascii_hexdigit) {
            *counts.entry(line[..40].to_string()).or_insert(0) += 1;
        }
    }
    Some(counts)
}

/// Blames `paths` on up to `workers` threads. A path is missing from the
/// result when the wall-clock budget ran out before it was reached; a path
/// that cannot be blamed maps to `None`.
fn blame_files(
    root: &Path,
    head: &str,
    paths: &[String],
    workers: usize,
    started: Instant,
    budget: Duration,
) -> HashMap<String, Option<HashMap<String, u64>>> {
    let next = AtomicUsize::new(0);
    let results = Mutex::new(HashMap::new());
    std::thread::scope(|scope| {
        for _ in 0..workers.clamp(1, paths.len().max(1)) {
            scope.spawn(|| loop {
                if started.elapsed() >= budget {
                    break;
                }
                let index = next.fetch_add(1, Ordering::Relaxed);
                let Some(path) = paths.get(index) else {
                    break;
                };
                let counts = blame_counts(root, head, path);
                lock(&results).insert(path.clone(), counts);
            });
        }
    });
    results.into_inner().unwrap_or_else(|e| e.into_inner())
}

/// Survival samples for PRs merged at least 30 days before `until`.
///
/// `written` is the added-line total of the PR's final diff and `alive` the
/// lines of those same files at `head` that blame to the landing commit or to
/// a commit that came in with it. Files that no longer exist at `head` (deleted
/// or renamed) are skipped for both numbers, so a refactor never reads as
/// dead code. Rebased PRs are only approximated by their landing commit.
///
/// What gets sampled is decided by counts alone: the newest PRs, until
/// `max_survival_prs` or `max_survival_files` distinct files would be passed.
/// The same HEAD therefore gives the same samples however fast the machine is.
/// Only the wall-clock safety limit can cut the work short, and only then is
/// the second value `true`.
fn compute_survival(
    root: &Path,
    head: &str,
    mut candidates: Vec<SurvivalCandidate>,
    until_epoch: i64,
    limits: &Limits,
) -> (Vec<RawSurvival>, bool) {
    let cutoff = until_epoch - SURVIVAL_MIN_AGE_DAYS * 86_400;
    candidates.retain(|c| c.merged_epoch <= cutoff);
    candidates.sort_by(|a, b| {
        b.merged_epoch
            .cmp(&a.merged_epoch)
            .then_with(|| a.pr_id.cmp(&b.pr_id))
    });
    candidates.truncate(limits.max_survival_prs);
    if candidates.is_empty() {
        return (Vec::new(), false);
    }

    let started = Instant::now();
    let existing: HashSet<String> = git(root, ["ls-tree", "-r", "-z", "--name-only", head])
        .map(|out| {
            out.split('\0')
                .filter(|p| !p.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();

    // The PRs to sample and the distinct files they need, in a fixed order.
    let mut distinct: Vec<String> = Vec::new();
    let mut known: HashSet<&str> = HashSet::new();
    let mut planned: Vec<(&SurvivalCandidate, Vec<&RawFile>)> = Vec::new();
    for candidate in &candidates {
        if let Some(landing) = &candidate.landing {
            let reachable = gh::is_object_id(landing)
                && git(root, ["merge-base", "--is-ancestor", landing, head]).is_ok();
            if !reachable {
                continue;
            }
        }
        let files: Vec<&RawFile> = candidate
            .files
            .iter()
            .filter(|f| f.added > 0)
            .take(MAX_SURVIVAL_FILES)
            .filter(|f| existing.contains(&f.path))
            .collect();
        let fresh: HashSet<&str> = files
            .iter()
            .map(|f| f.path.as_str())
            .filter(|p| !known.contains(p))
            .collect();
        if distinct.len() + fresh.len() > limits.max_survival_files {
            break;
        }
        for file in &files {
            if known.insert(file.path.as_str()) {
                distinct.push(file.path.clone());
            }
        }
        planned.push((candidate, files));
    }

    let blames = blame_files(
        root,
        head,
        &distinct,
        limits.survival_workers,
        started,
        limits.survival_budget,
    );
    let mut samples = Vec::new();
    for (candidate, files) in planned {
        let (mut written, mut alive) = (0u64, 0u64);
        for file in files {
            let Some(blamed) = blames.get(&file.path) else {
                // The time limit stopped the blaming before this file.
                return (samples, true);
            };
            let Some(counts) = blamed else {
                continue;
            };
            written += file.added;
            alive += candidate
                .hashes
                .iter()
                .map(|h| counts.get(h).copied().unwrap_or(0))
                .sum::<u64>()
                .min(file.added);
        }
        if written > 0 {
            samples.push(RawSurvival {
                pr_id: candidate.pr_id.clone(),
                written,
                alive,
            });
        }
    }
    (samples, false)
}

fn collect_raw(
    root: &Path,
    branch: &str,
    head: &str,
    window: &Window,
    limits: &Limits,
    github: &dyn gh::GhRunner,
    commit_cache: &gh::CommitCache,
) -> Result<RawData, String> {
    let mut truncated = false;

    let output = git(
        root,
        [
            "log",
            "--no-merges",
            "--no-show-signature",
            "--date-order",
            "--numstat",
            "-z",
            "-w",
            "-M",
            "--no-color",
            "-n",
            &(limits.max_commits + 1).to_string(),
            &window.log_args()[0],
            &window.log_args()[1],
            LOG_FORMAT,
            head,
            "--",
        ],
    )?;
    let (mut parsed, files_capped) = parse_log(&output, limits.max_files_per_commit);
    truncated |= files_capped;
    if parsed.len() > limits.max_commits {
        parsed.truncate(limits.max_commits);
        truncated = true;
    }

    let dropped = match patch_ids(root, head, window, limits) {
        Ok(Some(pairs)) => {
            let committed: HashMap<String, i64> = parsed
                .iter()
                .filter_map(|c| Some((c.hash.clone(), parse_iso_epoch(&c.committed_at)?)))
                .collect();
            select_duplicates(&pairs, &committed)
        }
        Ok(None) | Err(_) => {
            truncated = true;
            HashSet::new()
        }
    };

    let mut derived = derive_prs_from_git(
        root,
        head,
        window,
        &parsed,
        &dropped,
        limits,
        &mut truncated,
    )?;
    let github = gh::enrich(
        &gh::Request {
            gh: github,
            commits: commit_cache,
            root,
            branch,
            head,
            window,
            limits,
        },
        &mut derived,
    );
    truncated |= github.truncated;
    let reverted = reverted_pr_ids(&parsed, &derived);

    let candidates = derived
        .drafts
        .iter()
        .filter_map(|d| {
            let landing = if d.pr.source == "gh" {
                Some(d.pr.merge_commit.clone()?)
            } else {
                None
            };
            Some(SurvivalCandidate {
                pr_id: d.pr.id.clone(),
                landing,
                merged_epoch: parse_iso_epoch(d.pr.merged_at.as_deref()?)?,
                hashes: d.hashes.clone(),
                files: d.pr.files.clone(),
            })
        })
        .collect();
    let (survival, survival_cut) =
        compute_survival(root, head, candidates, window.until_epoch, limits);
    truncated |= survival_cut;

    let commits = parsed
        .into_iter()
        .filter(|c| !dropped.contains(&c.hash))
        .map(|c| RawCommit {
            pr_id: derived.commit_pr.get(&c.hash).cloned(),
            hash: c.hash,
            subject: c.subject,
            author: c.author,
            committer: c.committer,
            authored_at: c.authored_at,
            committed_at: c.committed_at,
            co_authors: c.co_authors,
            files: c.files,
        })
        .collect();

    let data = RawData {
        head: head.to_string(),
        branch: branch.to_string(),
        since: window.since_echo.clone(),
        until: window.until_echo.clone(),
        gh_available: github.available,
        gh_note: github.note,
        commits,
        prs: derived.drafts.into_iter().map(|d| d.pr).collect(),
        reviews: github.reviews,
        survival,
        reverted_pr_ids: reverted,
        truncated,
    };
    Ok(data)
}

// ---------------------------------------------------------------------------
// cache
// ---------------------------------------------------------------------------

/// (repo root, branch, head sha, normalized since, normalized until)
type CacheKey = (String, String, String, String, String);

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

struct Cache {
    entries: Mutex<Vec<(CacheKey, Arc<RawData>)>>,
    inflight: Mutex<Vec<(CacheKey, Arc<Mutex<()>>)>>,
    /// GitHub commit lists per PR; they outlive the entries above, so a new
    /// HEAD does not refetch every PR.
    commits: gh::CommitCache,
}

impl Cache {
    const fn new() -> Self {
        Self {
            entries: Mutex::new(Vec::new()),
            inflight: Mutex::new(Vec::new()),
            commits: gh::CommitCache::new(),
        }
    }

    fn get(&self, key: &CacheKey) -> Option<Arc<RawData>> {
        lock(&self.entries)
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.clone())
    }

    fn insert(&self, key: CacheKey, value: Arc<RawData>) {
        let mut entries = lock(&self.entries);
        entries.retain(|(k, _)| *k != key);
        entries.push((key, value));
        while entries.len() > CACHE_CAPACITY {
            entries.remove(0);
        }
    }

    fn remove(&self, key: &CacheKey) {
        lock(&self.entries).retain(|(k, _)| k != key);
    }

    /// One computation per key at a time; later callers wait, then hit the cache.
    fn key_lock(&self, key: &CacheKey) -> Arc<Mutex<()>> {
        let mut inflight = lock(&self.inflight);
        if let Some((_, existing)) = inflight.iter().find(|(k, _)| k == key) {
            return existing.clone();
        }
        let created = Arc::new(Mutex::new(()));
        inflight.push((key.clone(), created.clone()));
        created
    }

    fn release(&self, key: &CacheKey, held: &Arc<Mutex<()>>) {
        let mut inflight = lock(&self.inflight);
        // Only the map and the caller hold it: nobody is waiting.
        if Arc::strong_count(held) <= 2 {
            inflight.retain(|(k, _)| k != key);
        }
    }
}

/// Releases the in-flight slot of a key on every exit path, errors and panics
/// included, so a failed collection can never block later calls for that key.
struct SlotRelease<'a> {
    cache: &'a Cache,
    key: &'a CacheKey,
    gate: &'a Arc<Mutex<()>>,
}

impl Drop for SlotRelease<'_> {
    fn drop(&mut self) {
        self.cache.release(self.key, self.gate);
    }
}

static CACHE: Cache = Cache::new();

/// The `gh` used by the commands. Tests default to "not installed" so they
/// never reach the network; the ones about GitHub inject their own runner.
#[cfg(not(test))]
fn default_gh() -> gh::SystemGh {
    gh::SystemGh
}

#[cfg(test)]
fn default_gh() -> gh::Unavailable {
    gh::Unavailable
}

fn collect_cached(
    cache: &Cache,
    cwd: &Path,
    since: &str,
    until: Option<&str>,
    branch: Option<&str>,
    force: bool,
) -> Result<Arc<RawData>, String> {
    collect_cached_with(cache, &default_gh(), cwd, since, until, branch, force)
}

fn collect_cached_with(
    cache: &Cache,
    github: &dyn gh::GhRunner,
    cwd: &Path,
    since: &str,
    until: Option<&str>,
    branch: Option<&str>,
    force: bool,
) -> Result<Arc<RawData>, String> {
    let root = repo_root(cwd)?;
    let window = Window::for_repo(&root, since, until)?;
    let (name, head) = resolve_branch(&root, branch)?;
    let key: CacheKey = (
        root.to_string_lossy().into_owned(),
        name.clone(),
        head.clone(),
        window.since_echo.clone(),
        window.until_key.clone(),
    );

    let gate = cache.key_lock(&key);
    // Declared before the guard so it is dropped after it.
    let _slot = SlotRelease {
        cache,
        key: &key,
        gate: &gate,
    };
    let _guard = lock(&gate);
    if force {
        cache.remove(&key);
    } else if let Some(hit) = cache.get(&key) {
        return Ok(hit);
    }
    let data = Arc::new(collect_raw(
        &root,
        &name,
        &head,
        &window,
        &Limits::DEFAULT,
        github,
        &cache.commits,
    )?);
    cache.insert(key.clone(), data.clone());
    Ok(data)
}

// ---------------------------------------------------------------------------
// config files
// ---------------------------------------------------------------------------

/// Reads a file the repository controls. A cloned repository can plant a
/// symlink, a FIFO or a device here, so only a regular file of at most 1 MB is
/// read: nothing is followed and nothing can block or fill memory.
fn read_optional(path: &Path) -> Result<Option<String>, String> {
    let not_regular = || format!("{} is not a regular file", path.display());
    let unreadable = |e: std::io::Error| format!("Could not read {}: {e}", path.display());
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.is_file() => {}
        Ok(_) => return Err(not_regular()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(unreadable(e)),
    }
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    // The path may have been swapped for a link or a FIFO since the check
    // above: refuse to follow, and never wait for a writer.
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = match options.open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(unreadable(e)),
    };
    if !file.metadata().is_ok_and(|m| m.is_file()) {
        return Err(not_regular());
    }
    let mut bytes = Vec::new();
    file.take(MAX_CONFIG_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(unreadable)?;
    if bytes.len() > MAX_CONFIG_BYTES {
        return Err(format!("{} is larger than 1 MB", path.display()));
    }
    String::from_utf8(bytes)
        .map(Some)
        .map_err(|_| format!("Could not read {}: not valid UTF-8", path.display()))
}

fn read_config(cwd: &Path) -> Result<Option<String>, String> {
    read_optional(&repo_root(cwd)?.join(CONFIG_DIR).join(CONFIG_FILE))
}

fn read_mailmap(cwd: &Path) -> Result<Option<String>, String> {
    read_optional(&repo_root(cwd)?.join(".mailmap"))
}

fn write_config(cwd: &Path, content: &str) -> Result<(), String> {
    if content.len() > MAX_CONFIG_BYTES {
        return Err("The Contributors config is larger than 1 MB.".to_string());
    }
    let dir = repo_root(cwd)?.join(CONFIG_DIR);
    // A repository could make `.monoptah` a link to somewhere else; `metadata`
    // would follow it, `symlink_metadata` does not.
    match std::fs::symlink_metadata(&dir) {
        Ok(meta) if meta.is_dir() => {}
        Ok(_) => return Err(format!("{} is not a directory", dir.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => std::fs::create_dir(&dir)
            .map_err(|e| format!("Could not create {}: {e}", dir.display()))?,
        Err(e) => return Err(format!("Could not read {}: {e}", dir.display())),
    }
    let temp = dir.join(format!(".{CONFIG_FILE}.{}.tmp", uuid::Uuid::new_v4()));
    let written = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp)
        .and_then(|mut file| {
            file.write_all(content.as_bytes())?;
            file.sync_all()
        });
    if let Err(e) = written {
        let _ = std::fs::remove_file(&temp);
        return Err(format!("Could not write {}: {e}", temp.display()));
    }
    let target = dir.join(CONFIG_FILE);
    std::fs::rename(&temp, &target).map_err(|e| {
        let _ = std::fs::remove_file(&temp);
        format!("Could not write {}: {e}", target.display())
    })
}

fn list_branches(cwd: &Path) -> Result<ContributorsBranches, String> {
    let root = repo_root(cwd)?;
    let current = current_branch(&root);
    let listed = git(
        &root,
        ["for-each-ref", "--format=%(refname:short)", "refs/heads"],
    )?;
    let mut branches: Vec<String> = listed
        .lines()
        .map(str::trim)
        .filter(|b| !b.is_empty())
        .map(str::to_string)
        .collect();
    if let Some(index) = branches.iter().position(|b| *b == current) {
        let name = branches.remove(index);
        branches.insert(0, name);
    }
    Ok(ContributorsBranches { current, branches })
}

/// `user.name` / `user.email` as git would use them here; empty when unset.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitUser {
    pub name: String,
    pub email: String,
}

fn read_git_user(cwd: &Path) -> Result<GitUser, String> {
    let root = repo_root(cwd)?;
    let value = |key: &str| {
        git(&root, ["config", "--get", key])
            .map(|out| out.trim().to_string())
            .unwrap_or_default()
    };
    Ok(GitUser {
        name: value("user.name"),
        email: value("user.email"),
    })
}

// ---------------------------------------------------------------------------
// commands
// ---------------------------------------------------------------------------

#[tauri::command]
pub async fn contributors_read_config(cwd: String) -> Result<Option<String>, String> {
    tauri::async_runtime::spawn_blocking(move || read_config(&expand_home(&cwd)))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn contributors_read_mailmap(cwd: String) -> Result<Option<String>, String> {
    tauri::async_runtime::spawn_blocking(move || read_mailmap(&expand_home(&cwd)))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn contributors_write_config(cwd: String, content: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || write_config(&expand_home(&cwd), &content))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn contributors_branches(cwd: String) -> Result<ContributorsBranches, String> {
    tauri::async_runtime::spawn_blocking(move || list_branches(&expand_home(&cwd)))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn contributors_git_user(cwd: String) -> Result<GitUser, String> {
    tauri::async_runtime::spawn_blocking(move || read_git_user(&expand_home(&cwd)))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn contributors_collect(
    cwd: String,
    since: String,
    until: Option<String>,
    branch: Option<String>,
    force: Option<bool>,
) -> Result<RawData, String> {
    tauri::async_runtime::spawn_blocking(move || {
        collect_cached(
            &CACHE,
            &expand_home(&cwd),
            &since,
            until.as_deref(),
            branch.as_deref(),
            force.unwrap_or(false),
        )
        .map(|data| (*data).clone())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;

    pub(super) type Who = (&'static str, &'static str);

    pub(super) const ALICE: Who = ("Alice", "alice@example.com");
    const BOB: Who = ("Bob", "bob@example.com");
    const SINCE: &str = "2026-01-01T00:00:00Z";
    const UNTIL: &str = "2026-06-30T00:00:00Z";

    pub(super) struct TestRepo {
        dir: PathBuf,
    }

    impl Drop for TestRepo {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    impl TestRepo {
        pub(super) fn new() -> Self {
            let dir = std::env::temp_dir().join(format!(
                "monoptah-contributors-test-{}",
                uuid::Uuid::new_v4()
            ));
            std::fs::create_dir_all(&dir).unwrap();
            let repo = Self { dir };
            repo.run(
                &["init", "-q", "-b", "main"],
                ALICE,
                "2026-01-01T00:00:00+0000",
            );
            repo
        }

        pub(super) fn path(&self) -> &Path {
            &self.dir
        }

        pub(super) fn run(&self, args: &[&str], who: Who, date: &str) -> String {
            let output = Command::new("git")
                .arg("-C")
                .arg(&self.dir)
                .args([
                    "-c",
                    &format!("user.name={}", who.0),
                    "-c",
                    &format!("user.email={}", who.1),
                    "-c",
                    "commit.gpgsign=false",
                    "-c",
                    "core.autocrlf=false",
                    "-c",
                    "init.templateDir=",
                    "-c",
                ])
                .arg(format!(
                    "core.hooksPath={}",
                    self.dir.join(".no-hooks").display()
                ))
                .args(args)
                .env("GIT_CONFIG_GLOBAL", self.dir.join(".no-gitconfig"))
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env("GIT_AUTHOR_DATE", date)
                .env("GIT_COMMITTER_DATE", date)
                .env("GIT_TERMINAL_PROMPT", "0")
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "git {args:?} failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            String::from_utf8_lossy(&output.stdout).trim().to_string()
        }

        fn write(&self, rel: &str, content: &str) {
            let path = self.dir.join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, content).unwrap();
        }

        pub(super) fn commit(
            &self,
            files: &[(&str, &str)],
            message: &str,
            who: Who,
            date: &str,
        ) -> String {
            for (path, content) in files {
                self.write(path, content);
            }
            self.run(&["add", "-A"], who, date);
            self.run(&["commit", "-q", "-m", message], who, date);
            self.run(&["rev-parse", "HEAD"], who, date)
        }

        fn checkout_new(&self, name: &str) {
            self.run(
                &["checkout", "-q", "-b", name],
                ALICE,
                "2026-01-01T00:00:00+0000",
            );
        }

        fn checkout(&self, name: &str) {
            self.run(&["checkout", "-q", name], ALICE, "2026-01-01T00:00:00+0000");
        }

        fn collect(&self) -> RawData {
            collect_cached(&Cache::new(), self.path(), SINCE, Some(UNTIL), None, false)
                .unwrap()
                .as_ref()
                .clone()
        }
    }

    fn commit_by<'a>(data: &'a RawData, subject: &str) -> &'a RawCommit {
        data.commits
            .iter()
            .find(|c| c.subject == subject)
            .unwrap_or_else(|| panic!("no commit {subject:?}"))
    }

    fn pr_by_title<'a>(data: &'a RawData, title: &str) -> &'a RawPr {
        data.prs
            .iter()
            .find(|p| p.title == title)
            .unwrap_or_else(|| panic!("no pr titled {title:?}"))
    }

    fn pr_by_id<'a>(data: &'a RawData, id: &str) -> &'a RawPr {
        data.prs
            .iter()
            .find(|p| p.id == id)
            .unwrap_or_else(|| panic!("no pr {id:?}"))
    }

    // pure parsers

    #[test]
    fn co_author_trailers_are_parsed_tolerantly() {
        let body = "Did a thing.\n\nCo-authored-by: Carol Dev <carol@example.com>\n  co-AUTHORED-by :  \"Dan\"   <dan@example.com>  \nCo-authored-by: no email here\nCo-authored-by: Bad <not-an-email>\nCo-authored-by: Carol Again <CAROL@example.com>\nCo-authored-by: <ghost@example.com>\nCo-authored-by: Bot <12345+copilot@users.noreply.github.com>\n";
        let found = parse_co_authors(body);
        let emails: Vec<&str> = found.iter().map(|c| c.email.as_str()).collect();
        assert_eq!(
            emails,
            [
                "carol@example.com",
                "dan@example.com",
                "ghost@example.com",
                "12345+copilot@users.noreply.github.com"
            ]
        );
        assert_eq!(found[1].name, "Dan");
        assert_eq!(found[2].name, "ghost");
        assert_eq!(found[3].login.as_deref(), Some("copilot"));
        assert!(found[0].login.is_none());
    }

    #[test]
    fn login_comes_from_noreply_emails_only() {
        assert_eq!(
            login_from_email("octo@users.noreply.github.com").as_deref(),
            Some("octo")
        );
        assert_eq!(
            login_from_email("42+Octo-Cat@users.noreply.github.com").as_deref(),
            Some("Octo-Cat")
        );
        assert_eq!(login_from_email("octo@example.com"), None);
    }

    #[test]
    fn numstat_z_keeps_exact_paths_and_the_new_name_of_renames() {
        let text = concat!(
            "\n3\t1\tsrc/a b.rs\0",
            "-\t-\timg/logo.png\0",
            "0\t0\t\0x.rs\0y.rs\0",
            "2\t0\t\0src/{a}/{id}.ts\0src/{b}/{id}.ts\0",
            "5\t0\tdir/h\u{e9}llo \"q\".txt\0",
            "0\t0\tmode-only.sh\0",
        );
        let (files, truncated) = parse_numstat_z(text, 100);
        assert!(!truncated);
        let got: Vec<(&str, u64, u64)> = files
            .iter()
            .map(|f| (f.path.as_str(), f.added, f.deleted))
            .collect();
        assert_eq!(
            got,
            [
                ("src/a b.rs", 3, 1),
                ("src/{b}/{id}.ts", 2, 0),
                ("dir/h\u{e9}llo \"q\".txt", 5, 0),
            ]
        );
    }

    #[test]
    fn numstat_z_flags_only_an_exceeded_cap() {
        let text: String = (0..3).map(|i| format!("1\t0\tf{i}.txt\0")).collect();
        let (files, truncated) = parse_numstat_z(&text, 3);
        assert_eq!((files.len(), truncated), (3, false));
        let (files, truncated) = parse_numstat_z(&text, 2);
        assert_eq!((files.len(), truncated), (2, true));
    }

    fn record(hash: &str, subject: &str, body: &str) -> String {
        format!(
            "\u{1e}{hash}\0\0N\0n@x.io\0N\0n@x.io\02026-01-01T00:00:00Z\02026-01-01T00:00:00Z\0{subject}\0{body}\0\n1\t0\tf.txt\0"
        )
    }

    #[test]
    fn separator_bytes_in_a_message_never_drop_a_commit() {
        let (h1, h2) = ("a".repeat(40), "b".repeat(40));
        let output =
            record(&h1, "first\u{1e}half", "body\u{1e}with\nsep") + &record(&h2, "second", "");
        let (commits, truncated) = parse_log(&output, 10);
        assert!(!truncated);
        assert_eq!(commits.len(), 2);
        assert_eq!(commits[0].subject, "firsthalf");
        assert_eq!(commits[0].body, "bodywith\nsep");
        assert_eq!(commits[1].subject, "second");
        assert!(commits.iter().all(|c| c.files.len() == 1));
    }

    #[test]
    fn a_nul_inside_a_message_only_shifts_the_message_end() {
        let h = "c".repeat(40);
        let (commits, _) = parse_log(&record(&h, "subject", "one\0two"), 10);
        assert_eq!(commits.len(), 1);
        assert_eq!(commits[0].body, "one\ntwo");
        assert_eq!(commits[0].files.len(), 1);
    }

    struct Hiccup {
        interrupted: bool,
        data: &'static [u8],
    }

    impl Read for Hiccup {
        fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
            if !self.interrupted {
                self.interrupted = true;
                return Err(std::io::ErrorKind::Interrupted.into());
            }
            let n = self.data.len().min(out.len());
            out[..n].copy_from_slice(&self.data[..n]);
            self.data = &self.data[n..];
            Ok(n)
        }
    }

    struct BrokenPipe;

    impl Write for BrokenPipe {
        fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
            Err(std::io::ErrorKind::BrokenPipe.into())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn pump_retries_interrupts_and_reports_every_failure() {
        let mut sink = Vec::new();
        let mut input = Hiccup {
            interrupted: false,
            data: b"hello",
        };
        assert!(matches!(pump(&mut input, &mut sink, 100), Pump::Done));
        assert_eq!(sink, b"hello");

        let mut input = Hiccup {
            interrupted: true,
            data: b"hello",
        };
        assert!(matches!(pump(&mut input, &mut Vec::new(), 4), Pump::TooBig));

        let mut input = Hiccup {
            interrupted: true,
            data: b"hello",
        };
        assert!(matches!(
            pump(&mut input, &mut BrokenPipe, 100),
            Pump::Failed(_)
        ));
    }

    #[test]
    fn a_failed_collection_releases_its_in_flight_slot() {
        let cache = Cache::new();
        let key: CacheKey = ("r".into(), "b".into(), "h".into(), "s".into(), "u".into());
        let attempt = || -> Result<(), String> {
            let gate = cache.key_lock(&key);
            let _slot = SlotRelease {
                cache: &cache,
                key: &key,
                gate: &gate,
            };
            let _guard = lock(&gate);
            Err("boom".to_string())?;
            Ok(())
        };
        assert!(attempt().is_err());
        assert!(cache.inflight.lock().unwrap().is_empty());
        assert!(attempt().is_err());
    }

    #[test]
    fn squash_bullets_give_the_commit_count() {
        assert_eq!(squash_commit_count("* one\n\n* two\n\n* three\n"), 3);
        assert_eq!(squash_commit_count("no bullets"), 1);
        assert_eq!(squash_commit_count("  * indented\n"), 1);
    }

    #[test]
    fn pr_numbers_and_titles_come_from_subjects() {
        assert_eq!(
            pr_number_from_subject("Merge pull request #12 from a/b"),
            Some(12)
        );
        assert_eq!(pr_number_from_subject("Add thing (#345)"), Some(345));
        assert_eq!(pr_number_from_subject("Revert \"Add thing (#345)\""), None);
        assert_eq!(pr_number_from_subject("Fix issue #9 properly"), None);
        assert_eq!(strip_pr_suffix("Add thing (#345)"), "Add thing");
        assert_eq!(strip_pr_suffix("Plain subject"), "Plain subject");
        assert_eq!(
            merge_title("Merge pull request #1 from a/b", "\nReal title\n\nmore"),
            "Real title"
        );
        assert_eq!(
            merge_title("Merge branch 'x' into main", "body"),
            "Merge branch 'x' into main"
        );
    }

    #[test]
    fn revert_bodies_name_the_reverted_commit() {
        let sha = "a".repeat(40);
        let body = format!("This reverts commit {sha}, reversing changes.");
        assert_eq!(reverted_commit("Revert \"x\"", &body), Some(sha.clone()));
        assert_eq!(reverted_commit("Not a revert", &body), None);
        assert_eq!(
            reverted_commit("Revert \"x\"", "This reverts commit abc"),
            None
        );
    }

    #[test]
    fn iso_times_round_trip_and_honour_offsets() {
        assert_eq!(parse_iso_epoch("1970-01-01"), Some(0));
        assert_eq!(parse_iso_epoch("1970-01-02T00:00:00Z"), Some(86_400));
        assert_eq!(
            parse_iso_epoch("2026-10-15T12:00:00+02:00"),
            parse_iso_epoch("2026-10-15T10:00:00.123Z")
        );
        assert_eq!(
            parse_iso_epoch("2026-10-15T12:00:00-0130"),
            parse_iso_epoch("2026-10-15T13:30:00Z")
        );
        let epoch = parse_iso_epoch("2026-02-28T23:59:58Z").unwrap();
        assert_eq!(epoch_to_iso(epoch), "2026-02-28T23:59:58Z");
        assert_eq!(epoch_to_iso(epoch + 2), "2026-03-01T00:00:00Z");
        assert_eq!(parse_iso("garbage").map(|_| ()), None);
        assert_eq!(parse_iso_epoch("2026-13-01"), None);
        assert_eq!(parse_iso_epoch("2026-02-31"), None);
        assert_eq!(parse_iso_epoch("2026-02-28T10:00:00+05:00junk"), None);
        assert!(parse_iso_epoch("2028-02-29").is_some());
    }

    #[test]
    fn git_date_args_never_use_the_current_time_of_day() {
        assert_eq!(
            git_date_arg("2026-03-05", false).unwrap(),
            "2026-03-05T00:00:00"
        );
        assert_eq!(
            git_date_arg("2026-03-05", true).unwrap(),
            "2026-03-05T23:59:59"
        );
        assert_eq!(
            git_date_arg("2026-03-05T10:20:30", false).unwrap(),
            "2026-03-05T10:20:30"
        );
        let epoch = parse_iso_epoch("2026-03-05T10:20:30+02:00").unwrap();
        assert_eq!(
            git_date_arg("2026-03-05T10:20:30+02:00", false).unwrap(),
            format!("@{epoch}")
        );
        assert!(git_date_arg("--output=/tmp/x", false).is_none());
    }

    #[test]
    fn duplicate_selection_keeps_the_earliest_commit() {
        let pairs: Vec<(String, String)> = [
            ("p1", "late"),
            ("p1", "early"),
            ("p2", "solo"),
            ("p1", "tie"),
        ]
        .iter()
        .map(|(p, c)| (p.to_string(), c.to_string()))
        .collect();
        let committed: HashMap<String, i64> =
            [("late", 30), ("early", 10), ("solo", 5), ("tie", 10)]
                .iter()
                .map(|(c, t)| (c.to_string(), *t))
                .collect();
        let dropped = select_duplicates(&pairs, &committed);
        let mut names: Vec<&str> = dropped.iter().map(String::as_str).collect();
        names.sort();
        assert_eq!(names, ["late", "tie"]);
    }

    // git-backed collection

    #[test]
    fn plain_commits_carry_numstat_renames_and_whitespace_rules() {
        let repo = TestRepo::new();
        let body: String = (1..=12).map(|i| format!("line {i}\n")).collect();
        repo.commit(
            &[("src/old.rs", &body), ("README.md", "hello\nworld\n")],
            "Add files",
            ALICE,
            "2026-02-02T10:00:00+0000",
        );
        let renamed = body.replace("line 12", "line twelve");
        repo.run(
            &["mv", "src/old.rs", "src/new.rs"],
            ALICE,
            "2026-02-03T10:00:00+0000",
        );
        repo.commit(
            &[("src/new.rs", &renamed)],
            "Rename with edit",
            ALICE,
            "2026-02-03T10:00:00+0000",
        );
        repo.run(
            &["mv", "src/new.rs", "src/moved.rs"],
            ALICE,
            "2026-02-04T10:00:00+0000",
        );
        repo.commit(&[], "Pure rename", ALICE, "2026-02-04T10:00:00+0000");
        repo.commit(
            &[("README.md", "hello  \nworld\t\n")],
            "Whitespace only",
            ALICE,
            "2026-02-05T10:00:00+0000",
        );

        let data = repo.collect();
        assert_eq!(data.branch, "main");
        assert_eq!(
            data.head,
            repo.run(&["rev-parse", "HEAD"], ALICE, "2026-02-05T10:00:00+0000")
        );
        assert!(!data.gh_available);
        assert_eq!(
            data.gh_note.as_deref(),
            Some("GitHub CLI (`gh`) is not installed.")
        );
        assert!(data.reviews.is_empty());
        assert!(!data.truncated);
        assert_eq!(data.since, SINCE);
        assert_eq!(data.until, UNTIL);

        let add = commit_by(&data, "Add files");
        assert_eq!(add.author.email, "alice@example.com");
        assert_eq!(add.authored_at, "2026-02-02T10:00:00Z");
        assert_eq!(add.committed_at, "2026-02-02T10:00:00Z");
        let mut files = add.files.clone();
        files.sort_by(|a, b| a.path.cmp(&b.path));
        assert_eq!(
            files,
            vec![
                RawFile {
                    path: "README.md".into(),
                    added: 2,
                    deleted: 0
                },
                RawFile {
                    path: "src/old.rs".into(),
                    added: 12,
                    deleted: 0
                },
            ]
        );
        assert_eq!(
            commit_by(&data, "Rename with edit").files,
            vec![RawFile {
                path: "src/new.rs".into(),
                added: 1,
                deleted: 1
            }]
        );
        assert!(commit_by(&data, "Pure rename").files.is_empty());
        assert!(commit_by(&data, "Whitespace only").files.is_empty());
        // Newest first.
        assert_eq!(data.commits[0].subject, "Whitespace only");
    }

    #[test]
    fn commits_outside_the_window_are_left_out() {
        let repo = TestRepo::new();
        repo.commit(
            &[("a.txt", "a\n")],
            "Too early",
            ALICE,
            "2025-12-01T10:00:00+0000",
        );
        repo.commit(
            &[("b.txt", "b\n")],
            "In window",
            ALICE,
            "2026-02-01T10:00:00+0000",
        );
        repo.commit(
            &[("c.txt", "c\n")],
            "Too late",
            ALICE,
            "2026-08-01T10:00:00+0000",
        );
        let data = repo.collect();
        let subjects: Vec<&str> = data.commits.iter().map(|c| c.subject.as_str()).collect();
        assert_eq!(subjects, ["In window"]);
    }

    #[test]
    fn co_authors_committers_and_logins_are_recorded() {
        let repo = TestRepo::new();
        repo.commit(
            &[("a.txt", "a\n")],
            "Pair work\n\nCo-authored-by: Bob <bob@example.com>\nCo-authored-by: Alice <7+ALICE-gh@users.noreply.github.com>",
            ("Alice", "7+alice-gh@users.noreply.github.com"),
            "2026-02-01T10:00:00+0000",
        );
        let data = repo.collect();
        let commit = commit_by(&data, "Pair work");
        assert_eq!(commit.author.login.as_deref(), Some("alice-gh"));
        assert_eq!(commit.co_authors.len(), 1);
        assert_eq!(commit.co_authors[0].email, "bob@example.com");
    }

    #[test]
    fn a_direct_commit_is_its_own_pr() {
        let repo = TestRepo::new();
        let hash = repo.commit(
            &[("a.txt", "a\nb\n")],
            "Direct work",
            BOB,
            "2026-02-01T10:00:00+0000",
        );
        let data = repo.collect();
        assert_eq!(data.prs.len(), 1);
        let pr = &data.prs[0];
        assert_eq!(pr.id, hash[..7]);
        assert_eq!(pr.number, None);
        assert_eq!(pr.title, "Direct work");
        assert_eq!(pr.state, "merged");
        assert_eq!(pr.source, "git");
        assert_eq!(pr.commit_count, 1);
        assert!(pr.branch_commits.is_empty());
        assert_eq!(pr.author.email, "bob@example.com");
        assert_eq!(pr.merge_commit.as_deref(), Some(hash.as_str()));
        assert_eq!(pr.merged_at.as_deref(), Some("2026-02-01T10:00:00Z"));
        assert!(!pr.closes_bug);
        assert_eq!(
            pr.files,
            vec![RawFile {
                path: "a.txt".into(),
                added: 2,
                deleted: 0
            }]
        );
        assert_eq!(data.commits[0].pr_id.as_deref(), Some(pr.id.as_str()));
    }

    #[test]
    fn a_squash_style_commit_is_a_pr_with_bullet_commit_count() {
        let repo = TestRepo::new();
        repo.commit(
            &[("a.txt", "a\n")],
            "Add thing (#12)\n\n* first\n\n* second\n\n* third\n\nCo-authored-by: Bob <bob@example.com>",
            ALICE,
            "2026-02-01T10:00:00+0000",
        );
        let data = repo.collect();
        let pr = pr_by_id(&data, "12");
        assert_eq!(pr.number, Some(12));
        assert_eq!(pr.title, "Add thing");
        assert_eq!(pr.commit_count, 3);
        assert!(pr.branch_commits.is_empty());
        assert_eq!(pr.landing_co_authors.len(), 1);
        assert_eq!(pr.landing_co_authors[0].email, "bob@example.com");
        assert_eq!(pr.author.email, "alice@example.com");
        assert_eq!(data.commits[0].pr_id.as_deref(), Some("12"));
    }

    #[test]
    fn a_merge_commit_pr_collects_branch_commits_and_the_final_diff() {
        let repo = TestRepo::new();
        repo.commit(
            &[("base.txt", "base\n")],
            "Base",
            ALICE,
            "2026-02-01T10:00:00+0000",
        );
        repo.checkout_new("feature");
        let first = repo.commit(
            &[("widget.txt", "one\ntwo\nthree\n")],
            "Start widget",
            BOB,
            "2026-02-02T10:00:00+0000",
        );
        let second = repo.commit(
            &[("widget.txt", "one\nthree\nfour\nfive\n")],
            "Finish widget",
            BOB,
            "2026-02-03T10:00:00+0000",
        );
        repo.checkout("main");
        repo.commit(
            &[("other.txt", "o\n")],
            "Mainline work",
            ALICE,
            "2026-02-03T12:00:00+0000",
        );
        repo.run(
            &[
                "merge",
                "--no-ff",
                "-q",
                "-m",
                "Merge pull request #7 from bob/widget\n\nAdd widget",
                "feature",
            ],
            ALICE,
            "2026-02-04T10:00:00+0000",
        );

        let data = repo.collect();
        let pr = pr_by_id(&data, "7");
        assert_eq!(pr.number, Some(7));
        assert_eq!(pr.title, "Add widget");
        assert_eq!(pr.commit_count, 2);
        assert_eq!(pr.branch_commits.len(), 2);
        assert_eq!(pr.branch_commits[0].authors[0].email, "bob@example.com");
        assert_eq!(pr.author.email, "bob@example.com");
        assert_eq!(pr.committer.as_ref().unwrap().email, "alice@example.com");
        assert_eq!(pr.merged_at.as_deref(), Some("2026-02-04T10:00:00Z"));
        assert!(pr.merge_commit.is_some());
        // Final diff: widget.txt is 4 lines added net of the intermediate edit.
        assert_eq!(
            pr.files,
            vec![RawFile {
                path: "widget.txt".into(),
                added: 4,
                deleted: 0
            }]
        );
        assert_eq!(commit_by(&data, "Start widget").hash, first);
        assert_eq!(commit_by(&data, "Start widget").pr_id.as_deref(), Some("7"));
        assert_eq!(commit_by(&data, "Finish widget").hash, second);
        assert_eq!(
            commit_by(&data, "Finish widget").pr_id.as_deref(),
            Some("7")
        );
        // Mainline work lands as its own direct PR, and the merge adds no commit.
        let mainline = commit_by(&data, "Mainline work");
        assert_ne!(mainline.pr_id.as_deref(), Some("7"));
        assert_eq!(data.commits.len(), 4);
        assert_eq!(data.prs.len(), 3);
    }

    #[test]
    fn a_cherry_picked_commit_is_counted_once() {
        let repo = TestRepo::new();
        repo.commit(
            &[("base.txt", "base\n")],
            "Base",
            ALICE,
            "2026-02-01T10:00:00+0000",
        );
        repo.checkout_new("feature");
        let original = repo.commit(
            &[("feat.txt", "feature\n")],
            "Feature work",
            BOB,
            "2026-02-02T10:00:00+0000",
        );
        repo.checkout("main");
        repo.commit(
            &[("m.txt", "m\n")],
            "Main work",
            ALICE,
            "2026-02-03T10:00:00+0000",
        );
        repo.run(
            &["cherry-pick", &original],
            ALICE,
            "2026-02-04T10:00:00+0000",
        );
        repo.run(
            &["merge", "--no-ff", "-q", "-m", "Merge feature", "feature"],
            ALICE,
            "2026-02-05T10:00:00+0000",
        );

        let data = repo.collect();
        let feature: Vec<&RawCommit> = data
            .commits
            .iter()
            .filter(|c| c.subject == "Feature work")
            .collect();
        assert_eq!(feature.len(), 1, "duplicate patch must be dropped");
        assert_eq!(feature[0].hash, original, "the earliest commit is kept");
        assert_eq!(data.commits.len(), 3);
        // The cherry-pick no longer shows up as a PR of its own: the merge is a
        // plain sync, so the original commit is the only "Feature work" PR.
        assert_eq!(
            data.prs
                .iter()
                .filter(|p| p.title == "Feature work")
                .count(),
            1
        );
        assert!(data.prs.iter().all(|p| p.title != "Merge feature"));
        assert!(!data.truncated);
    }

    #[test]
    fn a_reverted_pr_is_reported() {
        let repo = TestRepo::new();
        let landed = repo.commit(
            &[("a.txt", "a\n")],
            "Add thing (#20)",
            ALICE,
            "2026-02-01T10:00:00+0000",
        );
        repo.commit(
            &[("b.txt", "b\n")],
            "Unrelated (#21)",
            ALICE,
            "2026-02-02T10:00:00+0000",
        );
        repo.run(
            &["revert", "--no-edit", &landed],
            BOB,
            "2026-02-03T10:00:00+0000",
        );

        let data = repo.collect();
        assert_eq!(data.reverted_pr_ids, ["20"]);
        // The revert is its own PR and is not flagged itself.
        assert!(data.prs.iter().any(|p| p.title.starts_with("Revert ")));
    }

    #[test]
    fn a_reverted_merge_commit_pr_is_reported() {
        let repo = TestRepo::new();
        repo.commit(
            &[("base.txt", "base\n")],
            "Base",
            ALICE,
            "2026-02-01T10:00:00+0000",
        );
        repo.checkout_new("feature");
        repo.commit(
            &[("f.txt", "f\n")],
            "Feature commit",
            BOB,
            "2026-02-02T10:00:00+0000",
        );
        repo.checkout("main");
        repo.commit(
            &[("m.txt", "m\n")],
            "Main commit",
            ALICE,
            "2026-02-03T10:00:00+0000",
        );
        repo.run(
            &[
                "merge",
                "--no-ff",
                "-q",
                "-m",
                "Merge pull request #5 from bob/f\n\nFeature",
                "feature",
            ],
            ALICE,
            "2026-02-04T10:00:00+0000",
        );
        let merge = repo.run(&["rev-parse", "HEAD"], ALICE, "2026-02-04T10:00:00+0000");
        repo.run(
            &["revert", "-m", "1", "--no-edit", &merge],
            ALICE,
            "2026-02-05T10:00:00+0000",
        );

        let data = repo.collect();
        assert_eq!(data.reverted_pr_ids, ["5"]);
    }

    #[test]
    fn survival_samples_old_prs_only() {
        let repo = TestRepo::new();
        repo.commit(
            &[("lib.txt", "one\ntwo\nthree\nfour\n")],
            "Add lib (#5)",
            ALICE,
            "2026-02-01T10:00:00+0000",
        );
        repo.commit(
            &[("gone.txt", "x\ny\n")],
            "Add gone (#6)",
            ALICE,
            "2026-02-02T10:00:00+0000",
        );
        repo.run(&["rm", "-q", "gone.txt"], BOB, "2026-03-01T10:00:00+0000");
        repo.commit(
            &[("lib.txt", "one\nTWO\nthree\nfour\n")],
            "Tweak lib",
            BOB,
            "2026-03-01T10:00:00+0000",
        );
        // Newer than 30 days before `until`: no sample.
        repo.commit(
            &[("new.txt", "n\n")],
            "Recent (#7)",
            ALICE,
            "2026-06-20T10:00:00+0000",
        );

        let data = repo.collect();
        // PR 5: 4 lines written, 3 still blamed to it. PR 6: its only file is
        // gone, so it is skipped rather than counted as dead. PR 7 is too recent.
        let sample = |id: &str| data.survival.iter().find(|s| s.pr_id == id).cloned();
        assert_eq!(
            sample("5"),
            Some(RawSurvival {
                pr_id: "5".into(),
                written: 4,
                alive: 3
            })
        );
        assert_eq!(sample("6"), None);
        assert_eq!(sample("7"), None);
        assert!(!data.truncated);
    }

    #[test]
    fn survival_stops_when_the_time_budget_is_spent() {
        let repo = TestRepo::new();
        let hash = repo.commit(
            &[("a.txt", "a\n")],
            "Old (#1)",
            ALICE,
            "2026-02-01T10:00:00+0000",
        );
        let candidate = SurvivalCandidate {
            pr_id: "1".into(),
            landing: None,
            merged_epoch: parse_iso_epoch("2026-02-01T10:00:00Z").unwrap(),
            hashes: HashSet::from([hash.clone()]),
            files: vec![RawFile {
                path: "a.txt".into(),
                added: 1,
                deleted: 0,
            }],
        };
        let until = parse_iso_epoch(UNTIL).unwrap();
        let spent = limits(|l| l.survival_budget = Duration::ZERO);
        let (samples, cut) = compute_survival(repo.path(), &hash, vec![candidate], until, &spent);
        assert!(samples.is_empty());
        assert!(cut);
    }

    /// Six old PRs, one file each, merged on Feb 1 .. Feb 6.
    fn six_old_prs() -> TestRepo {
        let repo = TestRepo::new();
        for day in 1..=6 {
            repo.commit(
                &[(&format!("f{day}.txt"), "a\nb\n")],
                &format!("Add f{day} (#{day})"),
                ALICE,
                &format!("2026-02-0{day}T10:00:00+0000"),
            );
        }
        repo
    }

    fn sampled_ids(data: &RawData) -> Vec<String> {
        data.survival.iter().map(|s| s.pr_id.clone()).collect()
    }

    #[test]
    fn survival_takes_the_newest_prs_up_to_the_count_caps() {
        let repo = six_old_prs();
        let all = repo.collect_with(&Limits::DEFAULT);
        assert_eq!(all.survival.len(), 6);
        assert!(!all.truncated);

        let few_prs = repo.collect_with(&limits(|l| l.max_survival_prs = 3));
        let mut ids = sampled_ids(&few_prs);
        ids.sort();
        assert_eq!(ids, ["4", "5", "6"]);
        // A count cap is not a time cut: no banner.
        assert!(!few_prs.truncated);

        let few_files = repo.collect_with(&limits(|l| l.max_survival_files = 2));
        let mut ids = sampled_ids(&few_files);
        ids.sort();
        assert_eq!(ids, ["5", "6"]);
        assert!(!few_files.truncated);
    }

    #[test]
    fn survival_is_the_same_on_every_run_whatever_the_worker_count() {
        let repo = six_old_prs();
        let run = |workers: usize| {
            let data = repo.collect_with(&limits(|l| {
                l.max_survival_files = 4;
                l.survival_workers = workers;
            }));
            data.survival
        };
        let first = run(1);
        assert_eq!(first.len(), 4);
        for _ in 0..3 {
            assert_eq!(run(4), first);
            assert_eq!(run(2), first);
        }
    }

    #[test]
    fn a_pr_that_does_not_fit_the_file_cap_ends_the_sample() {
        let repo = TestRepo::new();
        repo.commit(
            &[("a.txt", "a\n"), ("b.txt", "b\n"), ("c.txt", "c\n")],
            "Big (#1)",
            ALICE,
            "2026-02-01T10:00:00+0000",
        );
        repo.commit(
            &[("d.txt", "d\n")],
            "Small (#2)",
            ALICE,
            "2026-02-02T10:00:00+0000",
        );
        // Newest first: "Small" fits, "Big" would push past 2 files and stops.
        let data = repo.collect_with(&limits(|l| l.max_survival_files = 2));
        assert_eq!(sampled_ids(&data), ["2"]);
    }

    #[test]
    fn blame_ignores_textconv_drivers() {
        let repo = TestRepo::new();
        repo.commit(
            &[(".gitattributes", "*.txt diff=loud\n"), ("a.txt", "a\nb\n")],
            "Add a (#1)",
            ALICE,
            "2026-02-01T10:00:00+0000",
        );
        let marker = repo.path().join("textconv-ran");
        repo.run(
            &[
                "config",
                "diff.loud.textconv",
                &format!("sh -c 'touch {}; cat \"$1\"' sh", marker.display()),
            ],
            ALICE,
            "2026-02-01T10:00:00+0000",
        );
        let head = repo.run(&["rev-parse", "HEAD"], ALICE, "2026-02-01T10:00:00+0000");
        let counts = blame_counts(repo.path(), &head, "a.txt").unwrap();
        assert_eq!(counts.values().sum::<u64>(), 2);
        assert!(!marker.exists());
    }

    #[test]
    fn collected_data_serializes_to_the_camel_case_contract() {
        let repo = TestRepo::new();
        repo.commit(
            &[("a.txt", "a\n")],
            "Work",
            ALICE,
            "2026-02-01T10:00:00+0000",
        );
        let json = serde_json::to_value(repo.collect()).unwrap();
        assert_eq!(json["ghAvailable"], false);
        assert_eq!(json["ghNote"], "GitHub CLI (`gh`) is not installed.");
        assert_eq!(json["revertedPrIds"], serde_json::json!([]));
        let commit = &json["commits"][0];
        assert!(commit["committedAt"].is_string());
        assert!(commit["coAuthors"].is_array());
        assert!(commit["author"].get("login").is_none());
        assert!(json["prs"][0]["landingCoAuthors"].is_array());
        assert!(json["prs"][0].get("closedAt").is_none());
    }

    #[test]
    fn a_branch_argument_overrides_the_checked_out_branch() {
        let repo = TestRepo::new();
        repo.commit(
            &[("a.txt", "a\n")],
            "On main",
            ALICE,
            "2026-02-01T10:00:00+0000",
        );
        repo.checkout_new("side");
        repo.commit(
            &[("b.txt", "b\n")],
            "On side",
            ALICE,
            "2026-02-02T10:00:00+0000",
        );
        repo.checkout("main");

        let cache = Cache::new();
        let side =
            collect_cached(&cache, repo.path(), SINCE, Some(UNTIL), Some("side"), false).unwrap();
        assert_eq!(side.branch, "side");
        assert_eq!(side.commits.len(), 2);
        let main = collect_cached(&cache, repo.path(), SINCE, Some(UNTIL), None, false).unwrap();
        assert_eq!(main.branch, "main");
        assert_eq!(main.commits.len(), 1);

        let err = collect_cached(&cache, repo.path(), SINCE, Some(UNTIL), Some("nope"), false)
            .unwrap_err();
        assert!(err.contains("nope"), "{err}");
        let err = collect_cached(
            &cache,
            repo.path(),
            SINCE,
            Some(UNTIL),
            Some("--all"),
            false,
        )
        .unwrap_err();
        assert!(err.contains("--all"), "{err}");
        assert!(
            collect_cached(&cache, repo.path(), "not a date", Some(UNTIL), None, false).is_err()
        );
    }

    #[test]
    fn the_cache_hits_until_forced_or_the_head_moves() {
        let repo = TestRepo::new();
        repo.commit(
            &[("a.txt", "a\n")],
            "One",
            ALICE,
            "2026-02-01T10:00:00+0000",
        );
        let cache = Cache::new();
        let run = |force| {
            collect_cached(
                &cache,
                repo.path(),
                "2026-01-01",
                Some("2026-06-30"),
                None,
                force,
            )
            .unwrap()
        };
        let first = run(false);
        let second = run(false);
        assert!(Arc::ptr_eq(&first, &second));
        let forced = run(true);
        assert!(!Arc::ptr_eq(&first, &forced));
        assert!(Arc::ptr_eq(&forced, &run(false)));

        repo.commit(
            &[("b.txt", "b\n")],
            "Two",
            ALICE,
            "2026-02-02T10:00:00+0000",
        );
        let moved = run(false);
        assert!(!Arc::ptr_eq(&forced, &moved));
        assert_eq!(moved.commits.len(), 2);
        assert!(cache.inflight.lock().unwrap().is_empty());
    }

    #[test]
    fn the_cache_keeps_at_most_eight_entries_dropping_the_oldest() {
        let cache = Cache::new();
        let data = Arc::new(RawData {
            head: String::new(),
            branch: String::new(),
            since: String::new(),
            until: String::new(),
            gh_available: false,
            gh_note: None,
            commits: Vec::new(),
            prs: Vec::new(),
            reviews: Vec::new(),
            survival: Vec::new(),
            reverted_pr_ids: Vec::new(),
            truncated: false,
        });
        let key = |n: usize| -> CacheKey {
            (
                "r".into(),
                "b".into(),
                format!("h{n}"),
                "s".into(),
                "u".into(),
            )
        };
        for n in 0..9 {
            cache.insert(key(n), data.clone());
        }
        assert!(cache.get(&key(0)).is_none());
        assert!((1..9).all(|n| cache.get(&key(n)).is_some()));
        assert_eq!(cache.entries.lock().unwrap().len(), CACHE_CAPACITY);
    }

    #[test]
    fn config_round_trips_and_creates_its_directory() {
        let repo = TestRepo::new();
        assert_eq!(read_config(repo.path()).unwrap(), None);
        write_config(repo.path(), "{\"version\":1}").unwrap();
        assert_eq!(
            read_config(repo.path()).unwrap().as_deref(),
            Some("{\"version\":1}")
        );
        write_config(repo.path(), "{\"version\":1,\"x\":2}").unwrap();
        assert_eq!(
            read_config(repo.path()).unwrap().as_deref(),
            Some("{\"version\":1,\"x\":2}")
        );
        let names: Vec<String> = std::fs::read_dir(repo.path().join(CONFIG_DIR))
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, [CONFIG_FILE], "no temp file is left behind");
    }

    #[test]
    fn oversized_config_is_rejected_without_writing() {
        let repo = TestRepo::new();
        let err = write_config(repo.path(), &"x".repeat(MAX_CONFIG_BYTES + 1)).unwrap_err();
        assert!(err.contains("1 MB"), "{err}");
        assert!(!repo.path().join(CONFIG_DIR).exists());
        write_config(repo.path(), &"x".repeat(MAX_CONFIG_BYTES)).unwrap();
    }

    #[test]
    fn config_is_found_from_a_subdirectory() {
        let repo = TestRepo::new();
        write_config(repo.path(), "{}").unwrap();
        let sub = repo.path().join("deep/er");
        std::fs::create_dir_all(&sub).unwrap();
        assert_eq!(read_config(&sub).unwrap().as_deref(), Some("{}"));
    }

    #[test]
    fn mailmap_is_read_from_the_repo_root() {
        let repo = TestRepo::new();
        assert_eq!(read_mailmap(repo.path()).unwrap(), None);
        repo.write(".mailmap", "Alice <alice@example.com> <old@example.com>\n");
        assert_eq!(
            read_mailmap(repo.path()).unwrap().as_deref(),
            Some("Alice <alice@example.com> <old@example.com>\n")
        );
    }

    #[test]
    fn branches_list_the_current_one_first() {
        let repo = TestRepo::new();
        repo.commit(
            &[("a.txt", "a\n")],
            "Init",
            ALICE,
            "2026-02-01T10:00:00+0000",
        );
        repo.checkout_new("alpha");
        repo.checkout_new("zeta");
        repo.checkout("alpha");
        let branches = list_branches(repo.path()).unwrap();
        assert_eq!(branches.current, "alpha");
        assert_eq!(branches.branches, ["alpha", "main", "zeta"]);
    }

    #[test]
    fn a_directory_that_is_not_a_repo_is_rejected() {
        let dir = std::env::temp_dir().join(format!(
            "monoptah-contributors-nogit-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let outcomes = [
            read_config(&dir).unwrap_err(),
            read_mailmap(&dir).unwrap_err(),
            write_config(&dir, "{}").unwrap_err(),
            list_branches(&dir).unwrap_err(),
            collect_cached(&Cache::new(), &dir, SINCE, None, None, false).unwrap_err(),
        ];
        let _ = std::fs::remove_dir_all(&dir);
        for err in outcomes {
            assert_eq!(err, "Not a git repository");
        }
    }

    #[test]
    fn an_empty_repository_reports_a_clear_error() {
        let repo = TestRepo::new();
        let err = collect_cached(&Cache::new(), repo.path(), SINCE, Some(UNTIL), None, false)
            .unwrap_err();
        assert!(err.contains("no commits"), "{err}");
    }

    const CAROL: Who = ("Carol", "carol@example.com");

    fn merge_pr_branch(repo: &TestRepo, branch: &str, message: &str, date: &str) {
        repo.run(
            &["merge", "--no-ff", "-q", "-m", message, branch],
            ALICE,
            date,
        );
    }

    fn limits(change: impl FnOnce(&mut Limits)) -> Limits {
        let mut limits = Limits { ..Limits::DEFAULT };
        change(&mut limits);
        limits
    }

    impl TestRepo {
        fn collect_with(&self, limits: &Limits) -> RawData {
            let root = repo_root(self.path()).unwrap();
            let (name, head) = resolve_branch(&root, None).unwrap();
            let window = Window::for_repo(&root, SINCE, Some(UNTIL)).unwrap();
            collect_raw(
                &root,
                &name,
                &head,
                &window,
                limits,
                &gh::Unavailable,
                &gh::CommitCache::new(),
            )
            .unwrap()
        }
    }

    #[test]
    fn special_paths_are_reported_exactly() {
        let repo = TestRepo::new();
        repo.commit(
            &[
                ("src/{a}/{id}.ts", "1\n2\n3\n4\n5\n6\n"),
                ("old name.txt", "1\n2\n3\n4\n5\n6\n"),
                ("h\u{e9}llo w\u{f6}rld.txt", "x\n"),
            ],
            "Add files",
            ALICE,
            "2026-02-01T10:00:00+0000",
        );
        repo.run(
            &["mv", "src/{a}", "src/{b}"],
            ALICE,
            "2026-02-02T10:00:00+0000",
        );
        repo.run(
            &["mv", "old name.txt", "new name.txt"],
            ALICE,
            "2026-02-02T10:00:00+0000",
        );
        repo.commit(
            &[
                ("src/{b}/{id}.ts", "1\n2\n3\n4\n5\n6\n7\n"),
                ("new name.txt", "1\n2\n3\n4\n5\n6\n8\n"),
            ],
            "Move and edit",
            ALICE,
            "2026-02-02T10:00:00+0000",
        );
        let data = repo.collect();
        let mut added: Vec<&str> = commit_by(&data, "Add files")
            .files
            .iter()
            .map(|f| f.path.as_str())
            .collect();
        added.sort();
        assert_eq!(
            added,
            [
                "h\u{e9}llo w\u{f6}rld.txt",
                "old name.txt",
                "src/{a}/{id}.ts"
            ]
        );
        let mut moved: Vec<(&str, u64, u64)> = commit_by(&data, "Move and edit")
            .files
            .iter()
            .map(|f| (f.path.as_str(), f.added, f.deleted))
            .collect();
        moved.sort();
        assert_eq!(moved, [("new name.txt", 1, 0), ("src/{b}/{id}.ts", 1, 0)]);
    }

    #[cfg(unix)]
    #[test]
    fn paths_git_would_quote_are_not_quoted() {
        let repo = TestRepo::new();
        repo.commit(
            &[("we\"ird name.txt", "a\n"), ("tab\there.txt", "b\n")],
            "Odd names",
            ALICE,
            "2026-02-01T10:00:00+0000",
        );
        let data = repo.collect();
        let mut paths: Vec<&str> = commit_by(&data, "Odd names")
            .files
            .iter()
            .map(|f| f.path.as_str())
            .collect();
        paths.sort();
        assert_eq!(paths, ["tab\there.txt", "we\"ird name.txt"]);
    }

    #[test]
    fn a_separator_byte_in_a_commit_message_keeps_the_commit() {
        let repo = TestRepo::new();
        repo.commit(
            &[("a.txt", "a\n")],
            "Before",
            ALICE,
            "2026-02-01T10:00:00+0000",
        );
        repo.commit(
            &[("b.txt", "b\n")],
            "Odd\u{1e}subject\n\nOdd \u{1e} body",
            ALICE,
            "2026-02-02T10:00:00+0000",
        );
        repo.commit(
            &[("c.txt", "c\n")],
            "After",
            ALICE,
            "2026-02-03T10:00:00+0000",
        );
        let data = repo.collect();
        let subjects: Vec<&str> = data.commits.iter().map(|c| c.subject.as_str()).collect();
        assert_eq!(subjects, ["After", "Oddsubject", "Before"]);
        assert_eq!(data.prs.len(), 3);
    }

    #[test]
    fn dates_are_validated_with_the_offending_argument() {
        let err = |since: &str, until: Option<&str>| Window::new(since, until).err().unwrap();
        assert_eq!(
            err("2026-03-01", Some("2026-02-01")),
            "The start date is after the end date."
        );
        assert!(Window::new("2026-03-01", Some("2026-03-01")).is_ok());
        assert!(err("2026-02-31", None).contains("since date: 2026-02-31"));
        assert!(err("2026-02-01", Some("nope")).contains("until date: nope"));
        assert!(err("2026-02-01", Some("2026-02-01T10:00:00+05:00junk")).contains("junk"));
        let repo = TestRepo::new();
        repo.commit(
            &[("a.txt", "a\n")],
            "One",
            ALICE,
            "2026-02-01T10:00:00+0000",
        );
        let reversed = collect_cached(&Cache::new(), repo.path(), UNTIL, Some(SINCE), None, false);
        assert_eq!(
            reversed.err().unwrap(),
            "The start date is after the end date."
        );
    }

    #[test]
    fn the_cache_key_uses_the_exact_window() {
        let repo = TestRepo::new();
        repo.commit(
            &[("a.txt", "a\n")],
            "One",
            ALICE,
            "2026-02-01T10:00:00+0000",
        );
        let cache = Cache::new();
        let run = |since: &str, until: Option<&str>| {
            collect_cached(&cache, repo.path(), since, until, None, false).unwrap()
        };
        let base = run("2026-01-01T00:00:00Z", Some("2026-06-30T00:00:00Z"));
        assert!(Arc::ptr_eq(
            &base,
            &run("2026-01-01T00:00:00Z", Some("2026-06-30T00:00:00Z"))
        ));
        assert!(!Arc::ptr_eq(
            &base,
            &run("2026-01-01T12:00:00Z", Some("2026-06-30T00:00:00Z"))
        ));
        assert!(!Arc::ptr_eq(
            &base,
            &run("2026-01-01T00:00:00Z", Some("2026-06-30T12:00:00Z"))
        ));
        let today = epoch_to_iso(now_epoch())[..10].to_string();
        let open = run("2026-01-01T00:00:00Z", None);
        assert!(!Arc::ptr_eq(
            &open,
            &run("2026-01-01T00:00:00Z", Some(&today))
        ));
        assert!(Arc::ptr_eq(&open, &run("2026-01-01T00:00:00Z", None)));
    }

    #[test]
    fn a_sync_merge_keeps_the_prs_it_brought_in() {
        let repo = TestRepo::new();
        repo.commit(
            &[("base.txt", "base\n")],
            "Base",
            ALICE,
            "2026-02-01T10:00:00+0000",
        );
        repo.checkout_new("upstream");
        repo.commit(
            &[("a.txt", "a\n")],
            "Add a (#31)",
            BOB,
            "2026-02-02T10:00:00+0000",
        );
        repo.commit(
            &[("b.txt", "b\n")],
            "Add b (#32)",
            CAROL,
            "2026-02-03T10:00:00+0000",
        );
        repo.checkout("main");
        repo.commit(
            &[("m.txt", "m\n")],
            "Local work",
            ALICE,
            "2026-02-03T12:00:00+0000",
        );
        merge_pr_branch(
            &repo,
            "upstream",
            "Merge branch 'upstream' into main",
            "2026-02-04T10:00:00+0000",
        );

        let data = repo.collect();
        let a = pr_by_title(&data, "Add a");
        assert_eq!(a.author.email, "bob@example.com");
        assert_eq!(
            pr_by_title(&data, "Add b").author.email,
            "carol@example.com"
        );
        // Reached through a sync: keyed by hash, never by an upstream number.
        assert_eq!((a.number, a.id.len()), (None, 7));
        assert!(data.prs.iter().all(|p| !p.title.starts_with("Merge")));
        assert_eq!(data.prs.len(), 4, "Base, Local work and the two PRs");
        assert_eq!(
            commit_by(&data, "Add a (#31)").pr_id.as_deref(),
            Some(a.id.as_str())
        );
        assert_eq!(
            commit_by(&data, "Add b (#32)").pr_id.as_deref(),
            Some(pr_by_title(&data, "Add b").id.as_str())
        );
    }

    #[test]
    fn a_sync_merge_expands_nested_pull_request_merges() {
        let repo = TestRepo::new();
        repo.commit(
            &[("base.txt", "base\n")],
            "Base",
            ALICE,
            "2026-02-01T10:00:00+0000",
        );
        repo.checkout_new("upstream");
        repo.checkout_new("pr5");
        repo.commit(
            &[("p1.txt", "1\n")],
            "Part one",
            BOB,
            "2026-02-02T10:00:00+0000",
        );
        repo.commit(
            &[("p2.txt", "2\n")],
            "Part two",
            BOB,
            "2026-02-03T10:00:00+0000",
        );
        repo.checkout("upstream");
        merge_pr_branch(
            &repo,
            "pr5",
            "Merge pull request #5 from bob/pr5\n\nBob feature",
            "2026-02-04T10:00:00+0000",
        );
        repo.commit(
            &[("c.txt", "c\n")],
            "Add c (#6)",
            CAROL,
            "2026-02-05T10:00:00+0000",
        );
        repo.checkout("main");
        repo.commit(
            &[("m.txt", "m\n")],
            "Local work",
            ALICE,
            "2026-02-05T12:00:00+0000",
        );
        merge_pr_branch(
            &repo,
            "upstream",
            "Merge remote-tracking branch 'origin/main'",
            "2026-02-06T10:00:00+0000",
        );

        let data = repo.collect();
        let five = pr_by_title(&data, "Bob feature");
        assert_eq!(five.number, None);
        assert_eq!(
            Some(five.id.as_str()),
            five.merge_commit.as_deref().map(|h| &h[..7])
        );
        assert_eq!(five.commit_count, 2);
        assert_eq!(five.branch_commits.len(), 2);
        assert_eq!(five.author.email, "bob@example.com");
        assert_eq!(
            pr_by_title(&data, "Add c").author.email,
            "carol@example.com"
        );
        assert_eq!(
            commit_by(&data, "Part one").pr_id.as_deref(),
            Some(five.id.as_str())
        );
        assert!(data.prs.iter().all(|p| !p.title.starts_with("Merge")));
        assert_eq!(data.prs.len(), 4);
    }

    #[test]
    fn a_pull_request_merge_is_one_pr_even_if_its_commits_look_like_squashes() {
        let repo = TestRepo::new();
        repo.commit(
            &[("base.txt", "base\n")],
            "Base",
            ALICE,
            "2026-02-01T10:00:00+0000",
        );
        repo.checkout_new("y");
        repo.commit(
            &[("a.txt", "a\n")],
            "Tidy (#90)",
            BOB,
            "2026-02-02T10:00:00+0000",
        );
        repo.commit(&[("b.txt", "b\n")], "More", BOB, "2026-02-03T10:00:00+0000");
        repo.commit(&[("c.txt", "c\n")], "Last", BOB, "2026-02-03T11:00:00+0000");
        repo.checkout("main");
        merge_pr_branch(
            &repo,
            "y",
            "Merge pull request #7 from x/y\n\nTidy things",
            "2026-02-04T10:00:00+0000",
        );
        let data = repo.collect();
        let pr = pr_by_id(&data, "7");
        assert_eq!(pr.commit_count, 3);
        assert_eq!(pr.title, "Tidy things");
        assert!(data.prs.iter().all(|p| p.id != "90"));
        assert_eq!(data.prs.len(), 2);
    }

    #[test]
    fn a_duplicate_patch_inside_a_merge_pr_lowers_its_commit_count() {
        let repo = TestRepo::new();
        let base = repo.commit(
            &[("base.txt", "base\n")],
            "Base",
            ALICE,
            "2026-02-01T10:00:00+0000",
        );
        repo.checkout_new("hotfix");
        let fix = repo.commit(
            &[("x.txt", "fix\n")],
            "Hotfix",
            BOB,
            "2026-02-02T10:00:00+0000",
        );
        repo.checkout("main");
        merge_pr_branch(
            &repo,
            "hotfix",
            "Merge pull request #8 from bob/hotfix\n\nHotfix",
            "2026-02-03T10:00:00+0000",
        );
        repo.run(
            &["checkout", "-q", "-b", "feature", &base],
            ALICE,
            "2026-02-03T11:00:00+0000",
        );
        repo.run(&["cherry-pick", &fix], ALICE, "2026-02-04T10:00:00+0000");
        repo.commit(
            &[("f.txt", "f\n")],
            "Feature part two",
            CAROL,
            "2026-02-05T10:00:00+0000",
        );
        repo.checkout("main");
        merge_pr_branch(
            &repo,
            "feature",
            "Merge pull request #9 from carol/feature\n\nFeature",
            "2026-02-06T10:00:00+0000",
        );

        let data = repo.collect();
        assert_eq!(
            data.commits
                .iter()
                .filter(|c| c.subject == "Hotfix")
                .count(),
            1
        );
        let nine = pr_by_id(&data, "9");
        assert_eq!(
            nine.commit_count, 1,
            "the cherry-picked copy is not counted again"
        );
        assert_eq!(nine.branch_commits.len(), 1);
        assert_eq!(nine.author.email, "carol@example.com");
        assert_eq!(pr_by_id(&data, "8").commit_count, 1);
    }

    #[test]
    fn survival_of_a_merge_commit_pr_follows_its_branch_commits() {
        let repo = TestRepo::new();
        repo.commit(
            &[("base.txt", "base\n")],
            "Base",
            ALICE,
            "2026-02-01T10:00:00+0000",
        );
        repo.checkout_new("topic");
        repo.commit(
            &[("a.txt", "l1\nl2\nl3\nl4\n")],
            "Topic one",
            BOB,
            "2026-02-02T10:00:00+0000",
        );
        repo.commit(
            &[("a.txt", "l1\nl2\nl3\nl4\nl5\nl6\n")],
            "Topic two",
            BOB,
            "2026-02-03T10:00:00+0000",
        );
        repo.checkout("main");
        merge_pr_branch(
            &repo,
            "topic",
            "Merge pull request #12 from bob/topic\n\nTopic",
            "2026-02-04T10:00:00+0000",
        );
        repo.commit(
            &[("a.txt", "L1\nl2\nl3\nl4\nl5\nl6\n")],
            "Later edit",
            CAROL,
            "2026-03-01T10:00:00+0000",
        );

        let data = repo.collect();
        let sample = data.survival.iter().find(|s| s.pr_id == "12").cloned();
        assert_eq!(
            sample,
            Some(RawSurvival {
                pr_id: "12".into(),
                written: 6,
                alive: 5
            })
        );
    }

    fn cherry_pick_repo() -> TestRepo {
        let repo = TestRepo::new();
        repo.commit(
            &[("base.txt", "base\n")],
            "Base",
            ALICE,
            "2026-02-01T10:00:00+0000",
        );
        repo.checkout_new("feature");
        let original = repo.commit(
            &[("feat.txt", "feature\n")],
            "Feature work",
            BOB,
            "2026-02-02T10:00:00+0000",
        );
        repo.checkout("main");
        repo.run(
            &["cherry-pick", &original],
            ALICE,
            "2026-02-04T10:00:00+0000",
        );
        merge_pr_branch(
            &repo,
            "feature",
            "Merge feature",
            "2026-02-05T10:00:00+0000",
        );
        repo
    }

    #[test]
    fn every_exceeded_cap_sets_truncated_and_nothing_else_does() {
        // Within every cap: no flag.
        let repo = cherry_pick_repo();
        let calm = repo.collect_with(&Limits::DEFAULT);
        assert!(!calm.truncated);
        assert_eq!(
            calm.commits
                .iter()
                .filter(|c| c.subject == "Feature work")
                .count(),
            1
        );

        // Too many commits.
        let data = repo.collect_with(&limits(|l| l.max_commits = 2));
        assert!(data.truncated);
        assert!(data.commits.len() <= 2);
        assert!(!repo.collect_with(&limits(|l| l.max_commits = 3)).truncated);

        // Too much patch text: dedupe is skipped, so the duplicate stays.
        let data = repo.collect_with(&limits(|l| l.max_patch_bytes = 10));
        assert!(data.truncated);
        assert_eq!(
            data.commits
                .iter()
                .filter(|c| c.subject == "Feature work")
                .count(),
            2
        );

        // Survival ran out of time.
        let data = repo.collect_with(&limits(|l| l.survival_budget = Duration::ZERO));
        assert!(data.truncated);
        assert!(data.survival.is_empty());
    }

    #[test]
    fn the_file_and_branch_commit_caps_set_truncated() {
        let repo = TestRepo::new();
        repo.commit(
            &[("base.txt", "base\n")],
            "Base",
            ALICE,
            "2026-02-01T10:00:00+0000",
        );
        repo.checkout_new("topic");
        repo.commit(&[("a.txt", "a\n")], "One", BOB, "2026-02-02T10:00:00+0000");
        repo.commit(
            &[("b.txt", "b\n"), ("c.txt", "c\n"), ("d.txt", "d\n")],
            "Three files",
            BOB,
            "2026-02-03T10:00:00+0000",
        );
        repo.checkout("main");
        merge_pr_branch(
            &repo,
            "topic",
            "Merge pull request #3 from bob/topic\n\nTopic",
            "2026-02-04T10:00:00+0000",
        );

        assert!(!repo.collect_with(&Limits::DEFAULT).truncated);
        assert!(
            !repo
                .collect_with(&limits(|l| l.max_files_per_commit = 4))
                .truncated
        );
        let data = repo.collect_with(&limits(|l| l.max_files_per_commit = 2));
        assert!(data.truncated);
        assert_eq!(commit_by(&data, "Three files").files.len(), 2);

        let data = repo.collect_with(&limits(|l| l.max_branch_commits = 1));
        assert!(data.truncated);
        assert_eq!(pr_by_id(&data, "3").commit_count, 1);
        assert!(
            !repo
                .collect_with(&limits(|l| l.max_branch_commits = 2))
                .truncated
        );
    }

    impl TestRepo {
        fn derive_with(&self, limits: &Limits) -> (Derived, bool) {
            let root = repo_root(self.path()).unwrap();
            let (_, head) = resolve_branch(&root, None).unwrap();
            let window = Window::for_repo(&root, SINCE, Some(UNTIL)).unwrap();
            let [since, until] = window.log_args();
            let out = git(
                &root,
                [
                    "log",
                    "--no-merges",
                    "--numstat",
                    "-z",
                    "-w",
                    "-M",
                    &since,
                    &until,
                    LOG_FORMAT,
                    &head,
                    "--",
                ],
            )
            .unwrap();
            let (parsed, _) = parse_log(&out, limits.max_files_per_commit);
            let mut truncated = false;
            let derived = derive_prs_from_git(
                &root,
                &head,
                &window,
                &parsed,
                &HashSet::new(),
                limits,
                &mut truncated,
            )
            .unwrap();
            (derived, truncated)
        }
    }

    #[test]
    fn sync_merges_outside_the_window_are_not_walked() {
        let repo = TestRepo::new();
        repo.commit(
            &[("base.txt", "b\n")],
            "Base",
            ALICE,
            "2025-03-01T10:00:00+0000",
        );
        repo.checkout_new("feature");
        repo.commit(
            &[("f.txt", "f\n")],
            "Feature one",
            BOB,
            "2025-03-10T10:00:00+0000",
        );
        repo.checkout("main");
        repo.commit(
            &[("m.txt", "m\n")],
            "Main early",
            ALICE,
            "2025-03-15T10:00:00+0000",
        );
        repo.checkout("feature");
        merge_pr_branch(
            &repo,
            "main",
            "Merge branch 'main' into feature",
            "2025-04-01T10:00:00+0000",
        );
        repo.checkout("main");
        merge_pr_branch(
            &repo,
            "feature",
            "Merge branch 'feature'",
            "2026-02-01T10:00:00+0000",
        );

        let (derived, truncated) = repo.derive_with(&Limits::DEFAULT);
        assert!(!truncated);
        assert!(
            derived.drafts.is_empty(),
            "all the work landed before the window"
        );
        // One log for the main chain plus the single in-window sync; the old
        // `Merge branch 'main' into feature` is not followed.
        assert_eq!(derived.git_calls, 2);
    }

    #[test]
    fn the_walk_has_a_git_call_budget() {
        let repo = TestRepo::new();
        repo.commit(
            &[("base.txt", "b\n")],
            "Base",
            ALICE,
            "2026-02-01T10:00:00+0000",
        );
        repo.checkout_new("side");
        repo.commit(
            &[("s.txt", "s\n")],
            "Side work",
            BOB,
            "2026-02-02T10:00:00+0000",
        );
        repo.checkout("main");
        repo.checkout_new("upstream");
        repo.commit(
            &[("u.txt", "u\n")],
            "Up work",
            CAROL,
            "2026-02-03T10:00:00+0000",
        );
        merge_pr_branch(
            &repo,
            "side",
            "Merge branch 'side'",
            "2026-02-04T10:00:00+0000",
        );
        repo.checkout("main");
        repo.commit(
            &[("m.txt", "m\n")],
            "Local work",
            ALICE,
            "2026-02-05T10:00:00+0000",
        );
        merge_pr_branch(
            &repo,
            "upstream",
            "Merge upstream",
            "2026-02-06T10:00:00+0000",
        );

        let data = repo.collect_with(&Limits::DEFAULT);
        assert!(!data.truncated);
        for title in ["Side work", "Up work", "Local work"] {
            pr_by_title(&data, title);
        }
        let data = repo.collect_with(&limits(|l| l.max_walk_git_calls = 2));
        assert!(data.truncated);
    }

    #[test]
    fn a_branch_merged_locally_and_again_upstream_is_credited_once() {
        let repo = TestRepo::new();
        let base = repo.commit(
            &[("base.txt", "b\n")],
            "Base",
            ALICE,
            "2026-02-01T10:00:00+0000",
        );
        repo.checkout_new("feature");
        repo.commit(
            &[("f1.txt", "1\n")],
            "Feature one",
            BOB,
            "2026-02-02T10:00:00+0000",
        );
        repo.commit(
            &[("f2.txt", "2\n")],
            "Feature two",
            BOB,
            "2026-02-03T10:00:00+0000",
        );
        repo.checkout("main");
        repo.commit(
            &[("m.txt", "m\n")],
            "Local work",
            ALICE,
            "2026-02-03T12:00:00+0000",
        );
        merge_pr_branch(
            &repo,
            "feature",
            "Merge branch 'feature'",
            "2026-02-04T10:00:00+0000",
        );
        repo.run(
            &["checkout", "-q", "-b", "upstream", &base],
            ALICE,
            "2026-02-04T11:00:00+0000",
        );
        merge_pr_branch(
            &repo,
            "feature",
            "Merge pull request #9 from bob/feature\n\nFeature",
            "2026-02-05T10:00:00+0000",
        );
        repo.checkout("main");
        merge_pr_branch(
            &repo,
            "upstream",
            "Merge upstream",
            "2026-02-06T10:00:00+0000",
        );

        let data = repo.collect();
        assert_eq!(data.commits.len(), 4);
        let credited: u64 = data.prs.iter().map(|p| p.commit_count).sum();
        assert_eq!(credited, 4, "no commit may be counted by two PRs");
        assert!(data.prs.iter().all(|p| p.title != "Feature"));
        assert!(data.commits.iter().all(|c| c.pr_id.is_some()));
        assert_eq!(
            data.prs.iter().filter(|p| p.title == "Feature one").count(),
            1
        );
    }

    #[test]
    fn a_partly_overlapping_nested_pull_request_merge_counts_only_what_is_new() {
        let repo = TestRepo::new();
        repo.commit(
            &[("base.txt", "b\n")],
            "Base",
            ALICE,
            "2026-02-01T10:00:00+0000",
        );
        repo.checkout_new("upstream");
        repo.checkout_new("pr5");
        repo.commit(
            &[("p1.txt", "1\n")],
            "Part one",
            BOB,
            "2026-02-02T10:00:00+0000",
        );
        repo.commit(
            &[("p2.txt", "2\n2\n")],
            "Part two",
            BOB,
            "2026-02-03T10:00:00+0000",
        );
        repo.checkout("upstream");
        merge_pr_branch(
            &repo,
            "pr5",
            "Merge pull request #5 from bob/pr5\n\nBob feature",
            "2026-02-04T10:00:00+0000",
        );
        repo.checkout("main");
        // Main already took the first commit of the branch.
        merge_pr_branch(
            &repo,
            "pr5~1",
            "Merge branch 'part one'",
            "2026-02-04T12:00:00+0000",
        );
        merge_pr_branch(
            &repo,
            "upstream",
            "Merge upstream",
            "2026-02-05T10:00:00+0000",
        );

        let data = repo.collect();
        let feature = pr_by_title(&data, "Bob feature");
        assert_eq!(feature.commit_count, 1);
        let files: Vec<(&str, u64)> = feature
            .files
            .iter()
            .map(|f| (f.path.as_str(), f.added))
            .collect();
        assert_eq!(files, [("p2.txt", 2)]);
        // Every added line is credited once.
        let in_prs: u64 = data
            .prs
            .iter()
            .flat_map(|p| &p.files)
            .map(|f| f.added)
            .sum();
        let in_commits: u64 = data
            .commits
            .iter()
            .flat_map(|c| &c.files)
            .map(|f| f.added)
            .sum();
        assert_eq!(in_prs, in_commits);
    }

    #[test]
    fn a_nested_pull_request_merge_ignores_what_the_enclosing_sync_already_had() {
        let repo = TestRepo::new();
        repo.commit(
            &[("base.txt", "b\n")],
            "Base",
            ALICE,
            "2026-02-01T10:00:00+0000",
        );
        repo.checkout_new("upstream");
        repo.checkout_new("pr5");
        repo.commit(
            &[("p1.txt", "1\n")],
            "Part one",
            BOB,
            "2026-02-02T10:00:00+0000",
        );
        repo.commit(
            &[("p2.txt", "2\n")],
            "Part two",
            BOB,
            "2026-02-03T10:00:00+0000",
        );
        repo.checkout("upstream");
        merge_pr_branch(
            &repo,
            "pr5",
            "Merge pull request #5 from bob/pr5\n\nBob feature",
            "2026-02-04T10:00:00+0000",
        );
        repo.checkout("main");
        // Main already has Part one (cherry-picked in a real fork it would be
        // the same commit); here the whole branch arrives first.
        merge_pr_branch(
            &repo,
            "pr5",
            "Merge branch 'pr5'",
            "2026-02-04T12:00:00+0000",
        );
        merge_pr_branch(
            &repo,
            "upstream",
            "Merge upstream",
            "2026-02-05T10:00:00+0000",
        );

        let data = repo.collect();
        let credited: u64 = data.prs.iter().map(|p| p.commit_count).sum();
        assert_eq!(credited as usize, data.commits.len());
    }

    #[test]
    fn a_body_that_looks_like_numstat_is_not_read_as_files() {
        let h = "d".repeat(40);
        let body = "120\t4\tsrc/big.ts\nCo-authored-by: Bob <bob@example.com>";
        let (commits, _) = parse_log(&record(&h, "subject", body), 10);
        assert_eq!(commits[0].files.len(), 1);
        assert_eq!(commits[0].files[0].path, "f.txt");
        assert_eq!(commits[0].co_authors.len(), 1);
    }

    #[test]
    fn bare_date_edges_follow_the_local_day_git_uses() {
        let repo = TestRepo::new();
        repo.commit(
            &[("a.txt", "a\n")],
            "One",
            ALICE,
            "2026-02-01T10:00:00+0000",
        );
        let root = repo_root(repo.path()).unwrap();
        let utc = Window::new("2026-03-05", Some("2026-03-06")).unwrap();
        let local = Window::for_repo(&root, "2026-03-05", Some("2026-03-06")).unwrap();
        // Local midnight differs from UTC midnight by the zone offset only.
        assert!((local.since_epoch - utc.since_epoch).abs() <= 14 * 3600);
        assert!((local.until_epoch - utc.until_epoch).abs() <= 14 * 3600);
        assert!(local.since_epoch < local.until_epoch);
        // Exact instants are untouched.
        let exact =
            Window::for_repo(&root, "2026-03-05T00:00:00Z", Some("2026-03-06T00:00:00Z")).unwrap();
        assert_eq!(exact.since_epoch, utc.since_epoch);
    }

    #[test]
    fn equivalent_window_spellings_do_not_share_a_cache_entry() {
        let repo = TestRepo::new();
        repo.commit(
            &[("a.txt", "a\n")],
            "One",
            ALICE,
            "2026-02-01T10:00:00+0000",
        );
        let cache = Cache::new();
        let run = |since: &str| {
            collect_cached(&cache, repo.path(), since, Some("2026-06-30"), None, false).unwrap()
        };
        let date = run("2026-01-01");
        let datetime = run("2026-01-01T00:00:00");
        assert!(!Arc::ptr_eq(&date, &datetime));
        assert_eq!(date.since, "2026-01-01");
        assert_eq!(datetime.since, "2026-01-01T00:00:00");
    }

    #[test]
    fn nested_syncs_keep_the_exclusions_of_every_enclosing_merge() {
        let repo = TestRepo::new();
        repo.commit(
            &[("base.txt", "b\n")],
            "Base",
            ALICE,
            "2026-02-01T10:00:00+0000",
        );
        repo.checkout_new("upstream");
        repo.commit(
            &[("u.txt", "u\n")],
            "Upstream work (#50)",
            CAROL,
            "2026-02-03T10:00:00+0000",
        );
        repo.checkout("main");
        repo.commit(
            &[("x.txt", "x\n")],
            "Main work (#40)",
            ALICE,
            "2026-02-02T10:00:00+0000",
        );
        repo.checkout("upstream");
        merge_pr_branch(
            &repo,
            "main",
            "Merge branch 'main' of origin",
            "2026-02-04T10:00:00+0000",
        );
        repo.checkout("main");
        merge_pr_branch(
            &repo,
            "upstream",
            "Merge upstream",
            "2026-02-05T10:00:00+0000",
        );

        let data = repo.collect();
        // Main's own commit keeps its number: the nested sync must not claim it.
        let own = pr_by_id(&data, "40");
        assert_eq!(own.number, Some(40));
        assert_eq!(data.prs.len(), 3);
        let credited: u64 = data.prs.iter().map(|p| p.commit_count).sum();
        assert_eq!(credited as usize, data.commits.len());
        assert_eq!(pr_by_title(&data, "Upstream work").number, None);
    }

    // GitHub enrichment (git side and end to end; the parsers are tested in gh.rs)

    fn probes(headlines: &[&str]) -> Vec<gh::CommitProbe> {
        headlines
            .iter()
            .map(|h| gh::CommitProbe {
                headline: h.to_string(),
                email: String::new(),
            })
            .collect()
    }

    fn landing(repo: &TestRepo, sha: &str, headlines: &[&str]) -> Option<gh::Landing> {
        let root = repo_root(repo.path()).unwrap();
        gh::inspect_landing(&root, sha, &probes(headlines), &Limits::DEFAULT)
    }

    fn file_stats(files: &[RawFile]) -> Vec<(&str, u64, u64)> {
        files
            .iter()
            .map(|f| (f.path.as_str(), f.added, f.deleted))
            .collect()
    }

    #[test]
    fn a_squash_landing_diffs_against_its_parent() {
        let repo = TestRepo::new();
        repo.commit(
            &[("a.txt", "1\n")],
            "base",
            ALICE,
            "2026-01-05T10:00:00+0000",
        );
        let squash = repo.commit(
            &[("b.txt", "1\n2\n3\n")],
            "Add parser (#7)\n\n* one\n\n* two\n\nCo-authored-by: Dan <dan@example.com>",
            ALICE,
            "2026-01-10T10:00:00+0000",
        );
        let found = landing(&repo, &squash, &["one", "two"]).unwrap();
        assert_eq!(file_stats(&found.files.unwrap()), [("b.txt", 3, 0)]);
        assert_eq!(found.hashes, [squash]);
        assert_eq!(found.committer.name, "Alice");
        assert_eq!(found.co_authors.len(), 1);
        assert_eq!(found.co_authors[0].email, "dan@example.com");
    }

    #[test]
    fn a_rebase_landing_spans_every_commit_with_matching_headlines() {
        let repo = TestRepo::new();
        repo.commit(
            &[("a.txt", "1\n")],
            "base",
            ALICE,
            "2026-01-05T10:00:00+0000",
        );
        let mut shas = Vec::new();
        for (i, name) in ["r1", "r2", "r3"].iter().enumerate() {
            shas.push(repo.commit(
                &[(&format!("{name}.txt"), "x\n")],
                name,
                BOB,
                &format!("2026-01-1{}T10:00:00+0000", i + 2),
            ));
        }
        let found = landing(&repo, &shas[2], &["r1", "r2", "r3"]).unwrap();
        assert_eq!(
            file_stats(&found.files.unwrap()),
            [("r1.txt", 1, 0), ("r2.txt", 1, 0), ("r3.txt", 1, 0)]
        );
        let mut hashes = found.hashes.clone();
        hashes.sort();
        let mut expected = shas.clone();
        expected.sort();
        assert_eq!(hashes, expected);

        // Same count, different headlines: this is a squash of the last commit.
        let squash = landing(&repo, &shas[2], &["r1", "other", "r3"]).unwrap();
        assert_eq!(file_stats(&squash.files.unwrap()), [("r3.txt", 1, 0)]);
        assert_eq!(squash.hashes, [shas[2].clone()]);
    }

    #[test]
    fn a_merge_commit_landing_diffs_against_the_first_parent() {
        let repo = TestRepo::new();
        repo.commit(
            &[("a.txt", "1\n")],
            "base",
            ALICE,
            "2026-01-05T10:00:00+0000",
        );
        repo.checkout_new("feature");
        let f1 = repo.commit(&[("f1.txt", "x\n")], "f1", BOB, "2026-01-06T10:00:00+0000");
        let f2 = repo.commit(
            &[("f2.txt", "x\ny\n")],
            "f2",
            BOB,
            "2026-01-07T10:00:00+0000",
        );
        repo.checkout("main");
        repo.commit(
            &[("m1.txt", "m\n")],
            "m1",
            ALICE,
            "2026-01-08T10:00:00+0000",
        );
        merge_pr_branch(
            &repo,
            "feature",
            "Merge pull request #9 from bob/feature",
            "2026-01-09T10:00:00+0000",
        );
        let merge = repo.run(&["rev-parse", "HEAD"], ALICE, "2026-01-09T10:00:00+0000");
        let found = landing(&repo, &merge, &["f1", "f2"]).unwrap();
        assert_eq!(
            file_stats(&found.files.unwrap()),
            [("f1.txt", 1, 0), ("f2.txt", 2, 0)]
        );
        let mut hashes = found.hashes.clone();
        hashes.sort();
        let mut expected = vec![merge, f1, f2];
        expected.sort();
        assert_eq!(hashes, expected);
    }

    #[test]
    fn a_commit_missing_locally_has_no_landing() {
        let repo = TestRepo::new();
        repo.commit(
            &[("a.txt", "1\n")],
            "base",
            ALICE,
            "2026-01-05T10:00:00+0000",
        );
        assert!(landing(&repo, &"ab".repeat(20), &[]).is_none());
        assert!(landing(&repo, "--output=x", &[]).is_none());
        assert!(landing(&repo, "HEAD", &[]).is_none());
    }

    fn actor(login: &str, name: &str) -> serde_json::Value {
        serde_json::json!({ "id": "x", "is_bot": false, "login": login, "name": name })
    }

    fn gh_commit(headline: &str, login: &str, email: &str, at: &str) -> serde_json::Value {
        serde_json::json!({
            "oid": "0".repeat(40),
            "messageHeadline": headline,
            "authoredDate": at,
            "committedDate": at,
            "authors": [{ "name": login, "email": email, "login": login, "id": "x" }],
        })
    }

    /// Answers the calls of `gh::enrich` from fixed JSON.
    fn fake_github(
        merged: serde_json::Value,
        merged_commits: serde_json::Value,
        closed: serde_json::Value,
        closed_commits: serde_json::Value,
    ) -> gh::FakeGh {
        gh::FakeGh::new(move |args| {
            let value = |flag: &str| {
                args.iter()
                    .position(|a| *a == flag)
                    .map(|i| args[i + 1])
                    .unwrap_or("")
            };
            let (state, fields) = (value("--state"), value("--json"));
            let reply = match (args[0], state, fields) {
                ("auth", _, _) => return Ok("ok".to_string()),
                ("repo", _, _) => {
                    return Ok(serde_json::json!({
                        "nameWithOwner": "acme/widgets",
                        "url": "https://github.com/acme/widgets"
                    })
                    .to_string())
                }
                ("pr", "merged", "number,commits") => &merged_commits,
                ("pr", "closed", "number,commits") => &closed_commits,
                ("pr", "merged", _) => &merged,
                ("pr", "closed", _) => &closed,
                _ => panic!("unexpected gh call {args:?}"),
            };
            Ok(reply.to_string())
        })
    }

    #[test]
    fn github_data_replaces_derived_prs_end_to_end() {
        let repo = TestRepo::new();
        repo.commit(
            &[("a.txt", "1\n")],
            "base",
            ALICE,
            "2026-01-05T10:00:00+0000",
        );
        let squash = repo.commit(
            &[("parser.txt", "1\n2\n3\n")],
            "Add parser (#7)\n\n* one\n\n* two",
            ALICE,
            "2026-01-10T10:00:00+0000",
        );
        repo.commit(
            &[("direct.txt", "d\n")],
            "Direct push",
            BOB,
            "2026-01-11T10:00:00+0000",
        );
        let mut rebased = Vec::new();
        for (i, name) in ["r1", "r2", "r3"].iter().enumerate() {
            rebased.push(repo.commit(
                &[(&format!("{name}.txt"), "x\n")],
                name,
                BOB,
                &format!("2026-01-1{}T10:00:00+0000", i + 2),
            ));
        }
        repo.commit(
            &[("direct.txt", "d\ne\n")],
            &format!("Revert \"Add parser (#7)\"\n\nThis reverts commit {squash}."),
            ALICE,
            "2026-01-16T10:00:00+0000",
        );

        let merged = serde_json::json!([
            {
                "number": 7,
                "title": "Add parser",
                "author": actor("alice", "Alice A"),
                "mergedAt": "2026-01-10T10:00:00Z",
                "mergeCommit": { "oid": squash },
                "baseRefName": "main",
                "closingIssuesReferences": [{ "number": 3, "labels": [{ "name": "Bug" }] }],
                "files": [{ "path": "parser.txt", "additions": 99, "deletions": 0 }],
                "reviews": [
                    { "author": { "login": "carol" }, "state": "APPROVED",
                      "submittedAt": "2026-01-10T09:00:00Z", "body": "Looks great to me, nice work here" },
                    { "author": { "login": "Alice" }, "state": "COMMENTED",
                      "submittedAt": "2026-01-10T09:30:00Z", "body": "Replying to my own review thread" },
                    { "author": { "login": "dan" }, "state": "COMMENTED",
                      "submittedAt": "2026-01-10T09:40:00Z", "body": "ok" },
                    { "author": { "login": "erin" }, "state": "PENDING",
                      "submittedAt": null, "body": "draft" }
                ]
            },
            {
                "number": 8,
                "title": "Three small steps",
                "author": actor("bob", "Bob B"),
                "mergedAt": "2026-01-14T10:00:00Z",
                "mergeCommit": { "oid": rebased[2] },
                "baseRefName": "main",
                "closingIssuesReferences": [],
                "files": [],
                "reviews": []
            },
            {
                "number": 99,
                "title": "Other branch",
                "author": actor("bob", "Bob B"),
                "mergedAt": "2026-01-14T11:00:00Z",
                "mergeCommit": { "oid": "f".repeat(40) },
                "baseRefName": "release",
                "closingIssuesReferences": [],
                "files": [],
                "reviews": []
            }
        ]);
        let merged_commits = serde_json::json!([
            { "number": 7, "commits": [
                gh_commit("one", "alice", "alice@example.com", "2026-01-09T10:00:00Z"),
                gh_commit("two", "alice", "alice@example.com", "2026-01-10T08:00:00Z"),
            ] },
            { "number": 8, "commits": [
                gh_commit("r1", "bob", "bob@example.com", "2026-01-12T10:00:00Z"),
                gh_commit("r2", "bob", "bob@example.com", "2026-01-13T10:00:00Z"),
                gh_commit("r3", "bob", "bob@example.com", "2026-01-14T10:00:00Z"),
            ] }
        ]);
        let closed = serde_json::json!([{
            "number": 9, "title": "Abandoned", "author": actor("eve", "Eve"),
            "closedAt": "2026-01-20T10:00:00Z", "baseRefName": "main"
        }]);
        let closed_commits = serde_json::json!([{
            "number": 9,
            "commits": [gh_commit("wip", "eve", "eve@example.com", "2026-01-19T10:00:00Z")]
        }]);
        let github = fake_github(merged, merged_commits, closed, closed_commits);
        // A fork: gh must be pointed at origin, whatever it would pick itself.
        repo.run(
            &["remote", "add", "origin", "git@github.com:acme/widgets.git"],
            ALICE,
            "2026-01-01T00:00:00+0000",
        );
        // No network in tests: the PR head lookup fails at once, so the commits
        // come from the gh fallback.
        repo.run(
            &["config", "protocol.allow", "never"],
            ALICE,
            "2026-01-01T00:00:00+0000",
        );
        repo.run(
            &[
                "remote",
                "add",
                "upstream",
                "https://github.com/upstream/project.git",
            ],
            ALICE,
            "2026-01-01T00:00:00+0000",
        );

        let data = collect_cached_with(
            &Cache::new(),
            &github,
            repo.path(),
            SINCE,
            Some(UNTIL),
            None,
            false,
        )
        .unwrap();

        assert!(data.gh_available);
        assert_eq!(data.gh_note, None);
        let calls = github.calls.lock().unwrap().clone();
        // Every `gh repo view` names the repository; a bare one could pick upstream.
        let views: Vec<&Vec<String>> = calls.iter().filter(|c| c[0] == "repo").collect();
        assert!(!views.is_empty());
        assert!(views
            .iter()
            .all(|c| c[..3] == ["repo", "view", "acme/widgets"]));
        let pr_calls: Vec<&Vec<String>> = calls.iter().filter(|c| c[0] == "pr").collect();
        assert!(!pr_calls.is_empty());
        for call in pr_calls {
            assert_eq!(call[..4], ["pr", "list", "-R", "acme/widgets"], "{call:?}");
        }

        let seven = pr_by_id(&data, "7");
        assert_eq!(seven.source, "gh");
        assert_eq!(seven.number, Some(7));
        assert_eq!(seven.title, "Add parser");
        assert_eq!(seven.author.email, "alice@example.com");
        assert_eq!(seven.author.login.as_deref(), Some("alice"));
        assert_eq!(seven.commit_count, 2);
        assert_eq!(seven.branch_commits.len(), 2);
        assert!(seven.closes_bug);
        assert_eq!(seven.merge_commit.as_deref(), Some(squash.as_str()));
        // gh `files` is not read; the final diff is the local one.
        assert_eq!(file_stats(&seven.files), [("parser.txt", 3, 0)]);

        let eight = pr_by_id(&data, "8");
        assert_eq!(eight.commit_count, 3);
        assert_eq!(
            file_stats(&eight.files),
            [("r1.txt", 1, 0), ("r2.txt", 1, 0), ("r3.txt", 1, 0)]
        );

        let closed = pr_by_id(&data, "9");
        assert_eq!(closed.state, "closed");
        assert_eq!(closed.closed_at.as_deref(), Some("2026-01-20T10:00:00Z"));
        assert!(closed.files.is_empty());

        // PRs into another branch are ignored; derived PRs that gh replaced are gone.
        assert!(data.prs.iter().all(|p| p.id != "99"));
        let mut git_titles: Vec<&str> = data
            .prs
            .iter()
            .filter(|p| p.source == "git")
            .map(|p| p.title.as_str())
            .collect();
        git_titles.sort();
        assert_eq!(
            git_titles,
            ["Direct push", "Revert \"Add parser (#7)\"", "base"]
        );

        assert_eq!(
            commit_by(&data, "Add parser (#7)").pr_id.as_deref(),
            Some("7")
        );
        for name in ["r1", "r2", "r3"] {
            assert_eq!(commit_by(&data, name).pr_id.as_deref(), Some("8"), "{name}");
        }
        let direct = commit_by(&data, "Direct push");
        assert_eq!(
            direct.pr_id.as_deref(),
            Some(short_hash(&direct.hash).as_str())
        );
        assert_eq!(data.reverted_pr_ids, ["7"]);

        let mut reviews: Vec<(&str, &str, u64)> = data
            .reviews
            .iter()
            .map(|r| {
                (
                    r.reviewer.login.as_deref().unwrap(),
                    r.pr_id.as_str(),
                    r.comments,
                )
            })
            .collect();
        reviews.sort();
        assert_eq!(reviews, [("carol", "7", 1), ("dan", "7", 0)]);
        let ids: HashSet<&str> = data.prs.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(ids.len(), data.prs.len());
    }

    #[test]
    fn a_gh_failure_keeps_the_git_prs_and_a_clean_note() {
        let repo = TestRepo::new();
        repo.commit(
            &[("a.txt", "1\n")],
            "Add thing (#4)",
            ALICE,
            "2026-01-05T10:00:00+0000",
        );
        let github = gh::FakeGh::new(|args| {
            if args[0] == "pr" {
                Err(gh::GhError::Failed(
                    "GraphQL: API rate limit exceeded ghp_abcdefghijklmnopqrstuvwxyz0123\nsecond line"
                        .to_string(),
                ))
            } else {
                Ok(
                    r#"{"nameWithOwner":"acme/widgets","url":"https://github.com/acme/widgets"}"#
                        .to_string(),
                )
            }
        });
        let data = collect_cached_with(
            &Cache::new(),
            &github,
            repo.path(),
            SINCE,
            Some(UNTIL),
            None,
            false,
        )
        .unwrap();
        assert!(!data.gh_available);
        assert_eq!(
            data.gh_note.as_deref(),
            Some("GitHub data could not be loaded: GraphQL: API rate limit exceeded")
        );
        assert!(data.prs.iter().all(|p| p.source == "git"));
        assert_eq!(pr_by_id(&data, "4").title, "Add thing");
        assert!(data.reviews.is_empty());
        // A rate limit is not retried.
        let pr_calls = github
            .calls
            .lock()
            .unwrap()
            .iter()
            .filter(|c| c[0] == "pr")
            .count();
        assert_eq!(pr_calls, 1);
    }

    #[test]
    fn an_unmatched_git_pr_never_shares_an_id_with_a_gh_pr() {
        let repo = TestRepo::new();
        repo.commit(
            &[("a.txt", "1\n")],
            "Add thing (#4)",
            ALICE,
            "2026-01-05T10:00:00+0000",
        );
        let merged = serde_json::json!([{
            "number": 4, "title": "Add thing", "author": actor("alice", "Alice"),
            "mergedAt": "2026-01-05T10:00:00Z", "mergeCommit": { "oid": "e".repeat(40) },
            "baseRefName": "main", "closingIssuesReferences": [], "files": [], "reviews": []
        }]);
        let github = fake_github(
            merged,
            serde_json::json!([]),
            serde_json::json!([]),
            serde_json::json!([]),
        );
        let data = collect_cached_with(
            &Cache::new(),
            &github,
            repo.path(),
            SINCE,
            Some(UNTIL),
            None,
            false,
        )
        .unwrap();
        assert!(data.gh_available);
        assert_eq!(data.prs.len(), 2);
        let by_source = |source: &str| data.prs.iter().find(|p| p.source == source).unwrap();
        assert_eq!(by_source("gh").id, "4");
        let git_pr = by_source("git");
        assert_ne!(git_pr.id, "4");
        assert_eq!(git_pr.number, None);
        let commit = commit_by(&data, "Add thing (#4)");
        assert_eq!(commit.pr_id.as_deref(), Some(git_pr.id.as_str()));
    }

    #[test]
    fn a_rebase_landing_tolerates_truncated_headlines_and_checks_authors() {
        let repo = TestRepo::new();
        repo.commit(
            &[("a.txt", "1\n")],
            "base",
            ALICE,
            "2026-01-05T10:00:00+0000",
        );
        let long = "Treat S3 3xx as an error, keep partial delete results, add list delimiter";
        assert!(long.chars().count() > 72);
        let subjects = ["short one", long, "short three"];
        let mut shas = Vec::new();
        for (i, subject) in subjects.iter().enumerate() {
            shas.push(repo.commit(
                &[(&format!("f{i}.txt"), "x\n")],
                subject,
                BOB,
                &format!("2026-01-1{}T10:00:00+0000", i + 2),
            ));
        }
        // What gh reports: the long headline cut to 68 characters plus an ellipsis.
        let cut = format!("{}\u{2026}", &long[..68]);
        let found = landing(&repo, &shas[2], &["short one", &cut, "short three"]).unwrap();
        assert_eq!(found.hashes.len(), 3);
        assert_eq!(
            file_stats(&found.files.unwrap()),
            [("f0.txt", 1, 0), ("f1.txt", 1, 0), ("f2.txt", 1, 0)]
        );

        // Authors are compared when both sides know the email.
        let root = repo_root(repo.path()).unwrap();
        let with_email = |emails: [&str; 3]| -> Vec<gh::CommitProbe> {
            ["short one", cut.as_str(), "short three"]
                .iter()
                .zip(emails)
                .map(|(h, e)| gh::CommitProbe {
                    headline: h.to_string(),
                    email: e.to_string(),
                })
                .collect()
        };
        let same = with_email(["BOB@example.com", "bob@example.com", ""]);
        let found = gh::inspect_landing(&root, &shas[2], &same, &Limits::DEFAULT).unwrap();
        assert_eq!(found.hashes.len(), 3);
        let other = with_email(["bob@example.com", "carol@example.com", "bob@example.com"]);
        let found = gh::inspect_landing(&root, &shas[2], &other, &Limits::DEFAULT).unwrap();
        assert_eq!(found.hashes, [shas[2].clone()]);
    }

    #[test]
    fn a_merge_landing_beyond_the_branch_commit_cap_is_flagged() {
        let repo = TestRepo::new();
        repo.commit(
            &[("a.txt", "1\n")],
            "base",
            ALICE,
            "2026-01-05T10:00:00+0000",
        );
        repo.checkout_new("feature");
        repo.commit(&[("f1.txt", "x\n")], "f1", BOB, "2026-01-06T10:00:00+0000");
        repo.commit(&[("f2.txt", "x\n")], "f2", BOB, "2026-01-07T10:00:00+0000");
        repo.checkout("main");
        merge_pr_branch(
            &repo,
            "feature",
            "Merge pull request #9 from bob/feature",
            "2026-01-09T10:00:00+0000",
        );
        let merge = repo.run(&["rev-parse", "HEAD"], ALICE, "2026-01-09T10:00:00+0000");
        let root = repo_root(repo.path()).unwrap();
        let small = limits(|l| l.max_branch_commits = 1);
        let found = gh::inspect_landing(&root, &merge, &probes(&["f1", "f2"]), &small).unwrap();
        assert!(found.capped);
        assert_eq!(found.hashes.len(), 2);
        let found = landing(&repo, &merge, &["f1", "f2"]).unwrap();
        assert!(!found.capped);
    }

    #[test]
    fn a_gh_pr_without_a_landing_in_this_history_has_no_survival_sample() {
        let repo = TestRepo::new();
        repo.commit(
            &[("a.txt", "1\n2\n3\n")],
            "Add a (#7)",
            ALICE,
            "2026-01-10T10:00:00+0000",
        );
        let local = repo.run(&["rev-parse", "HEAD"], ALICE, "2026-01-10T10:00:00+0000");
        let pr = |number: u64, oid: &str| {
            serde_json::json!({
                "number": number, "title": format!("PR {number}"), "author": actor("alice", "Alice"),
                "mergedAt": "2026-01-10T10:00:00Z", "mergeCommit": { "oid": oid },
                "baseRefName": "main", "closingIssuesReferences": [], "reviews": [],
                "files": [{ "path": "a.txt", "additions": 3, "deletions": 0 }]
            })
        };
        let github = fake_github(
            serde_json::json!([pr(7, &local), pr(8, &"d".repeat(40))]),
            serde_json::json!([]),
            serde_json::json!([]),
            serde_json::json!([]),
        );
        let data = collect_cached_with(
            &Cache::new(),
            &github,
            repo.path(),
            SINCE,
            Some(UNTIL),
            None,
            false,
        )
        .unwrap();
        assert!(data.gh_available);
        let sampled: Vec<&str> = data.survival.iter().map(|s| s.pr_id.as_str()).collect();
        assert_eq!(sampled, ["7"]);
        assert_eq!(data.survival[0].alive, 3);
    }

    /// A bare repository that stands in for GitHub's side of `origin`: it
    /// holds `refs/pull/<n>/head` for the PRs.
    struct Bare(PathBuf);

    impl Bare {
        fn attach(repo: &TestRepo) -> Self {
            let dir =
                std::env::temp_dir().join(format!("monoptah-origin-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir_all(&dir).unwrap();
            let when = "2026-01-01T00:00:00+0000";
            repo.run(
                &["init", "-q", "--bare", dir.to_str().unwrap()],
                ALICE,
                when,
            );
            repo.run(
                &["remote", "add", "origin", dir.to_str().unwrap()],
                ALICE,
                when,
            );
            Self(dir)
        }

        fn publish(&self, repo: &TestRepo, number: u64, sha: &str) {
            repo.run(
                &[
                    "push",
                    "-q",
                    "origin",
                    &format!("{sha}:refs/pull/{number}/head"),
                ],
                ALICE,
                "2026-01-01T00:00:00+0000",
            );
        }
    }

    impl Drop for Bare {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// Drops the objects no ref reaches any more, so a PR branch that was
    /// only pushed to the origin is not in the repository.
    fn forget_unreachable(repo: &TestRepo) {
        let when = "2026-01-01T00:00:00+0000";
        repo.run(&["reflog", "expire", "--expire=now", "--all"], ALICE, when);
        repo.run(&["prune", "--expire=now"], ALICE, when);
    }

    fn has_object(repo: &TestRepo, sha: &str) -> bool {
        git(
            repo.path(),
            ["cat-file", "-e", &format!("{sha}^{{commit}}")],
        )
        .is_ok()
    }

    /// Like `fake_github`, but any `commits` follow-up call fails: commits must
    /// come from local git.
    fn fake_github_without_commit_calls(
        merged: serde_json::Value,
        closed: serde_json::Value,
    ) -> gh::FakeGh {
        gh::FakeGh::new(move |args| {
            let value = |flag: &str| {
                args.iter()
                    .position(|a| *a == flag)
                    .map(|i| args[i + 1])
                    .unwrap_or("")
            };
            let (state, fields) = (value("--state"), value("--json"));
            match (args[0], state, fields) {
                ("auth", _, _) => Ok("ok".to_string()),
                ("repo", _, _) => Ok(serde_json::json!({
                    "nameWithOwner": "acme/widgets",
                    "url": "https://github.com/acme/widgets"
                })
                .to_string()),
                ("pr", _, "number,commits") => Err(gh::GhError::Failed(
                    "commits must come from local git".to_string(),
                )),
                ("pr", "merged", _) => Ok(merged.to_string()),
                ("pr", "closed", _) => Ok(closed.to_string()),
                _ => panic!("unexpected gh call {args:?}"),
            }
        })
    }

    fn gh_pr(number: u64, login: &str, merged_at: &str, oid: &str) -> serde_json::Value {
        serde_json::json!({
            "number": number, "title": format!("PR {number}"), "author": actor(login, login),
            "mergedAt": merged_at, "mergeCommit": { "oid": oid },
            "baseRefName": "main", "closingIssuesReferences": [], "reviews": []
        })
    }

    #[test]
    fn pr_commits_and_diffs_come_from_local_git_without_gh_commit_calls() {
        let repo = TestRepo::new();
        let bare = Bare::attach(&repo);
        repo.commit(
            &[("a.txt", "1\n")],
            "base",
            ALICE,
            "2026-01-05T10:00:00+0000",
        );

        // PR 7, squash merged: its branch only exists on the origin.
        repo.checkout_new("pr7");
        let s1 = repo.commit(
            &[("parser.txt", "1\n")],
            "parse one",
            ALICE,
            "2026-01-06T10:00:00+0000",
        );
        let s2 = repo.commit(
            &[("parser.txt", "1\n2\n3\n")],
            "parse all",
            ALICE,
            "2026-01-07T10:00:00+0000",
        );
        bare.publish(&repo, 7, &s2);
        repo.checkout("main");
        repo.run(
            &["branch", "-q", "-D", "pr7"],
            ALICE,
            "2026-01-01T00:00:00+0000",
        );
        let squash = repo.commit(
            &[("parser.txt", "1\n2\n3\n")],
            "Add parser (#7)",
            ALICE,
            "2026-01-10T10:00:00+0000",
        );

        // PR 9, merged with a merge commit: its commits are already local.
        repo.checkout_new("feature");
        repo.commit(&[("f1.txt", "x\n")], "f1", BOB, "2026-01-11T10:00:00+0000");
        let f2 = repo.commit(
            &[("f2.txt", "x\ny\n")],
            "f2",
            BOB,
            "2026-01-12T10:00:00+0000",
        );
        bare.publish(&repo, 9, &f2);
        repo.checkout("main");
        repo.commit(
            &[("m1.txt", "m\n")],
            "m1",
            ALICE,
            "2026-01-13T10:00:00+0000",
        );
        merge_pr_branch(
            &repo,
            "feature",
            "Merge pull request #9 from bob/feature",
            "2026-01-14T10:00:00+0000",
        );
        let merge = repo.run(&["rev-parse", "HEAD"], ALICE, "2026-01-14T10:00:00+0000");

        // PR 11, closed without merging: only on the origin.
        repo.checkout_new("closed11");
        repo.commit(
            &[("wip.txt", "w\n")],
            "wip one",
            BOB,
            "2026-01-15T10:00:00+0000",
        );
        let c2 = repo.commit(
            &[("wip.txt", "w\nw\n")],
            "wip two",
            BOB,
            "2026-01-16T10:00:00+0000",
        );
        bare.publish(&repo, 11, &c2);
        repo.checkout("main");
        repo.run(
            &["branch", "-q", "-D", "closed11"],
            ALICE,
            "2026-01-01T00:00:00+0000",
        );
        forget_unreachable(&repo);
        assert!(!has_object(&repo, &s2) && !has_object(&repo, &c2));

        let merged = serde_json::json!([
            gh_pr(7, "alice", "2026-01-10T10:00:00Z", &squash),
            gh_pr(9, "bob", "2026-01-14T10:00:00Z", &merge),
        ]);
        let closed = serde_json::json!([{
            "number": 11, "title": "Abandoned", "author": actor("bob", "Bob"),
            "closedAt": "2026-01-20T10:00:00Z", "baseRefName": "main"
        }]);
        let github = fake_github_without_commit_calls(merged, closed);
        let cache = Cache::new();
        let collect = |force: bool| {
            collect_cached_with(
                &cache,
                &github,
                repo.path(),
                SINCE,
                Some(UNTIL),
                None,
                force,
            )
            .unwrap()
        };
        let data = collect(false);

        // A failing `commits` call would have made gh unavailable.
        assert!(data.gh_available, "{:?}", data.gh_note);
        assert_eq!(data.gh_note, None);
        assert!(github
            .calls
            .lock()
            .unwrap()
            .iter()
            .all(|c| !c.contains(&"number,commits".to_string())));

        let seven = pr_by_id(&data, "7");
        assert_eq!(seven.source, "gh");
        assert_eq!(seven.commit_count, 2);
        assert_eq!(seven.branch_commits.len(), 2);
        assert_eq!(
            seven.branch_commits[0].authors[0].email,
            "alice@example.com"
        );
        // No commit carries the login, but all of them share one email.
        assert_eq!(seven.author.email, "alice@example.com");
        assert_eq!(file_stats(&seven.files), [("parser.txt", 3, 0)]);
        assert!(has_object(&repo, &s1) && has_object(&repo, &s2));

        let nine = pr_by_id(&data, "9");
        assert_eq!(nine.commit_count, 2);
        assert_eq!(nine.branch_commits.len(), 2);
        assert_eq!(nine.branch_commits[0].authors[0].email, "bob@example.com");
        assert_eq!(
            file_stats(&nine.files),
            [("f1.txt", 1, 0), ("f2.txt", 2, 0)]
        );

        let eleven = pr_by_id(&data, "11");
        assert_eq!(eleven.state, "closed");
        assert_eq!(eleven.commit_count, 2);
        assert_eq!(eleven.branch_commits.len(), 2);
        assert!(!data.truncated);

        // Fetching added objects and nothing else: no ref, no FETCH_HEAD.
        let refs = repo.run(
            &["for-each-ref", "--format=%(refname)"],
            ALICE,
            "2026-01-01T00:00:00+0000",
        );
        assert!(!refs.contains("refs/pull"), "{refs}");
        assert!(!repo.path().join(".git").join("FETCH_HEAD").exists());

        // The commit lists are cached: a refresh needs neither the origin nor gh.
        drop(bare);
        let again = collect(true);
        assert!(again.gh_available, "{:?}", again.gh_note);
        assert_eq!(pr_by_id(&again, "7").commit_count, 2);
        assert_eq!(pr_by_id(&again, "11").commit_count, 2);
    }

    #[test]
    fn a_closed_pr_does_not_inherit_the_history_the_local_branch_is_missing() {
        let repo = TestRepo::new();
        let bare = Bare::attach(&repo);
        let when = "2026-01-01T00:00:00+0000";
        let base = repo.commit(
            &[("a.txt", "1\n")],
            "base",
            ALICE,
            "2026-01-05T10:00:00+0000",
        );
        // Two commits other people landed on main; the clone has them only as
        // `origin/main`, its own `main` is behind.
        repo.commit(
            &[("m1.txt", "m\n")],
            "main one",
            ALICE,
            "2026-01-08T10:00:00+0000",
        );
        repo.commit(
            &[("m2.txt", "m\n")],
            "main two",
            ALICE,
            "2026-01-09T10:00:00+0000",
        );
        repo.checkout_new("closed11");
        let wip = repo.commit(
            &[("wip.txt", "w\n")],
            "wip",
            BOB,
            "2026-01-15T10:00:00+0000",
        );
        bare.publish(&repo, 11, &wip);
        repo.checkout("main");
        repo.run(&["push", "-q", "origin", "main"], ALICE, when);
        repo.run(&["fetch", "-q", "origin"], ALICE, when);
        repo.run(&["reset", "-q", "--hard", &base], ALICE, when);
        repo.run(&["branch", "-q", "-D", "closed11"], ALICE, when);
        forget_unreachable(&repo);
        assert!(!has_object(&repo, &wip));

        let closed = serde_json::json!([{
            "number": 11, "title": "Abandoned", "author": actor("bob", "Bob"),
            "closedAt": "2026-01-20T10:00:00Z", "baseRefName": "main"
        }]);
        let github = fake_github_without_commit_calls(serde_json::json!([]), closed);
        let data = collect_cached_with(
            &Cache::new(),
            &github,
            repo.path(),
            SINCE,
            Some(UNTIL),
            None,
            false,
        )
        .unwrap();

        assert!(data.gh_available, "{:?}", data.gh_note);
        let eleven = pr_by_id(&data, "11");
        assert_eq!(eleven.state, "closed");
        // Only Bob's own commit; main one and main two are other people's.
        assert_eq!(eleven.branch_commits.len(), 1);
        assert_eq!(eleven.branch_commits[0].authors[0].email, "bob@example.com");
        assert_eq!(eleven.commit_count, 1);
    }

    #[test]
    fn a_pr_whose_head_cannot_be_fetched_still_gets_commits_from_gh() {
        let repo = TestRepo::new();
        let bare = Bare::attach(&repo);
        repo.commit(
            &[("a.txt", "1\n")],
            "base",
            ALICE,
            "2026-01-05T10:00:00+0000",
        );
        repo.checkout_new("pr7");
        let head = repo.commit(
            &[("p.txt", "1\n")],
            "local work",
            ALICE,
            "2026-01-06T10:00:00+0000",
        );
        bare.publish(&repo, 7, &head);
        repo.checkout("main");
        repo.run(
            &["branch", "-q", "-D", "pr7"],
            ALICE,
            "2026-01-01T00:00:00+0000",
        );
        let squash = repo.commit(
            &[("p.txt", "1\n")],
            "Add p (#7)",
            ALICE,
            "2026-01-10T10:00:00+0000",
        );
        let other = repo.commit(
            &[("q.txt", "1\n")],
            "Add q (#8)",
            ALICE,
            "2026-01-12T10:00:00+0000",
        );
        forget_unreachable(&repo);

        // PR 8 has no head on the origin, so gh is asked for its commits only.
        let merged = serde_json::json!([
            gh_pr(7, "alice", "2026-01-10T10:00:00Z", &squash),
            gh_pr(8, "alice", "2026-01-12T10:00:00Z", &other),
        ]);
        let merged_commits = serde_json::json!([
            { "number": 8, "commits": [
                gh_commit("q one", "alice", "alice@example.com", "2026-01-11T10:00:00Z"),
                gh_commit("q two", "alice", "alice@example.com", "2026-01-11T11:00:00Z"),
                gh_commit("q three", "alice", "alice@example.com", "2026-01-11T12:00:00Z"),
            ] }
        ]);
        let github = fake_github(
            merged,
            merged_commits,
            serde_json::json!([]),
            serde_json::json!([]),
        );
        let data = collect_cached_with(
            &Cache::new(),
            &github,
            repo.path(),
            SINCE,
            Some(UNTIL),
            None,
            false,
        )
        .unwrap();
        assert!(data.gh_available, "{:?}", data.gh_note);
        assert_eq!(pr_by_id(&data, "7").commit_count, 1);
        assert_eq!(pr_by_id(&data, "8").commit_count, 3);
        let searches: Vec<String> = github
            .calls
            .lock()
            .unwrap()
            .iter()
            .filter(|c| c.contains(&"number,commits".to_string()) && c[5] == "merged")
            .map(|c| c[c.iter().position(|a| a == "--search").unwrap() + 1].clone())
            .collect();
        assert_eq!(
            searches,
            ["merged:2026-01-12T10:00:00Z..2026-01-12T10:00:00Z base:main"]
        );
    }

    #[test]
    fn git_user_reads_the_repository_identity() {
        let repo = TestRepo::new();
        let unset = read_git_user(repo.path()).unwrap();
        assert_eq!(
            unset,
            GitUser {
                name: String::new(),
                email: String::new()
            }
        );
        repo.run(
            &["config", "user.name", "Local Dev"],
            ALICE,
            "2026-01-01T00:00:00+0000",
        );
        repo.run(
            &["config", "user.email", "dev@example.com"],
            ALICE,
            "2026-01-01T00:00:00+0000",
        );
        let set = read_git_user(repo.path()).unwrap();
        assert_eq!(set.name, "Local Dev");
        assert_eq!(set.email, "dev@example.com");
        let json = serde_json::to_value(&set).unwrap();
        assert_eq!(json["name"], "Local Dev");
        assert_eq!(json["email"], "dev@example.com");
        let dir = std::env::temp_dir().join(format!("monoptah-no-repo-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let err = read_git_user(&dir).unwrap_err();
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(err, "Not a git repository");
    }
    // safe config reads and writes

    fn config_path(repo: &TestRepo) -> PathBuf {
        repo.path().join(CONFIG_DIR).join(CONFIG_FILE)
    }

    /// Runs `f` on a thread so a read that blocks fails the test instead of
    /// hanging it.
    #[cfg(unix)]
    fn within_a_few_seconds<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
        let (send, receive) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = send.send(f());
        });
        receive
            .recv_timeout(Duration::from_secs(5))
            .expect("the read blocked")
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_config_or_mailmap_is_refused() {
        let repo = TestRepo::new();
        let outside =
            std::env::temp_dir().join(format!("monoptah-outside-{}", uuid::Uuid::new_v4()));
        std::fs::write(&outside, "SECRET").unwrap();
        std::fs::create_dir_all(repo.path().join(CONFIG_DIR)).unwrap();
        std::os::unix::fs::symlink(&outside, config_path(&repo)).unwrap();
        std::os::unix::fs::symlink(&outside, repo.path().join(".mailmap")).unwrap();

        let config = read_config(repo.path()).unwrap_err();
        let mailmap = read_mailmap(repo.path()).unwrap_err();
        let _ = std::fs::remove_file(&outside);
        assert!(config.ends_with("is not a regular file"), "{config}");
        assert!(mailmap.ends_with("is not a regular file"), "{mailmap}");
        assert!(!config.contains("SECRET"));
    }

    #[cfg(unix)]
    #[test]
    fn a_fifo_config_or_mailmap_is_refused_without_blocking() {
        use std::os::unix::ffi::OsStrExt;
        let repo = TestRepo::new();
        std::fs::create_dir_all(repo.path().join(CONFIG_DIR)).unwrap();
        for path in [config_path(&repo), repo.path().join(".mailmap")] {
            let c_path = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
            assert_eq!(unsafe { libc::mkfifo(c_path.as_ptr(), 0o600) }, 0);
        }
        let root = repo.path().to_path_buf();
        let (config, mailmap) =
            within_a_few_seconds(move || (read_config(&root), read_mailmap(&root)));
        assert!(config.unwrap_err().ends_with("is not a regular file"));
        assert!(mailmap.unwrap_err().ends_with("is not a regular file"));
    }

    #[test]
    fn a_directory_in_place_of_the_file_is_refused() {
        let repo = TestRepo::new();
        std::fs::create_dir_all(repo.path().join(".mailmap")).unwrap();
        let err = read_mailmap(repo.path()).unwrap_err();
        assert!(err.ends_with("is not a regular file"), "{err}");
    }

    #[test]
    fn a_file_over_1_mb_is_refused_but_exactly_1_mb_is_read() {
        let repo = TestRepo::new();
        std::fs::create_dir_all(repo.path().join(CONFIG_DIR)).unwrap();
        std::fs::write(config_path(&repo), "x".repeat(MAX_CONFIG_BYTES)).unwrap();
        assert_eq!(
            read_config(repo.path()).unwrap().map(|t| t.len()),
            Some(MAX_CONFIG_BYTES)
        );
        std::fs::write(config_path(&repo), "x".repeat(MAX_CONFIG_BYTES + 1)).unwrap();
        let err = read_config(repo.path()).unwrap_err();
        assert!(err.ends_with("is larger than 1 MB"), "{err}");
        std::fs::write(
            repo.path().join(".mailmap"),
            "y".repeat(MAX_CONFIG_BYTES + 1),
        )
        .unwrap();
        let err = read_mailmap(repo.path()).unwrap_err();
        assert!(err.ends_with("is larger than 1 MB"), "{err}");
    }

    #[test]
    fn a_missing_config_is_still_none_and_bad_utf8_is_an_error() {
        let repo = TestRepo::new();
        assert_eq!(read_config(repo.path()).unwrap(), None);
        std::fs::write(repo.path().join(".mailmap"), [0xff, 0xfe, b'x']).unwrap();
        let err = read_mailmap(repo.path()).unwrap_err();
        assert!(err.contains("UTF-8"), "{err}");
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_config_directory_is_never_written_through() {
        let repo = TestRepo::new();
        let outside =
            std::env::temp_dir().join(format!("monoptah-outside-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&outside).unwrap();
        std::os::unix::fs::symlink(&outside, repo.path().join(CONFIG_DIR)).unwrap();

        let err = write_config(repo.path(), "{}").unwrap_err();
        let leaked: Vec<_> = std::fs::read_dir(&outside).unwrap().collect();
        let _ = std::fs::remove_dir_all(&outside);
        assert!(err.contains("not a directory"), "{err}");
        assert!(leaked.is_empty());
    }

    #[test]
    fn a_file_named_like_the_config_directory_is_never_replaced() {
        let repo = TestRepo::new();
        std::fs::write(repo.path().join(CONFIG_DIR), "mine").unwrap();
        let err = write_config(repo.path(), "{}").unwrap_err();
        assert!(err.contains("not a directory"), "{err}");
        assert_eq!(
            std::fs::read_to_string(repo.path().join(CONFIG_DIR)).unwrap(),
            "mine"
        );
    }

    #[test]
    fn a_failed_rename_leaves_no_temp_file_behind() {
        let repo = TestRepo::new();
        // A non-empty directory where the file belongs makes the rename fail.
        let target = config_path(&repo);
        std::fs::create_dir_all(target.join("inner")).unwrap();
        let err = write_config(repo.path(), "{}").unwrap_err();
        assert!(err.contains("Could not write"), "{err}");
        let names: Vec<String> = std::fs::read_dir(repo.path().join(CONFIG_DIR))
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, [CONFIG_FILE]);
    }

    // repository root

    #[cfg(unix)]
    #[test]
    fn only_the_trailing_newline_is_trimmed_from_the_repository_root() {
        let dir = std::env::temp_dir().join(format!(" monoptah root {} ", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        git(&dir, ["init", "-q"]).unwrap();
        let root = repo_root(&dir);
        let expected = std::fs::canonicalize(&dir).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(root.unwrap(), expected);
    }
}
