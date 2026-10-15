//! GitHub (`gh`) enrichment of the Contributors raw data.
//!
//! When `gh` is installed, signed in and the repository has a GitHub remote,
//! the PRs derived from git are replaced by what GitHub reports (author,
//! commits, reviews, closing issues) and the final diff of each PR is read
//! from the local repository. Otherwise the git-derived PRs stay and the note
//! says why. Nothing here scores anything.
//!
//! `gh pr list --json` limits that shape this module:
//! - `commits` makes GitHub count `authors` nodes per commit, so a list call
//!   with it is rejected above ~40 PRs ("exceeds the maximum limit of 500,000
//!   nodes"). The list is therefore fetched without `commits`, and the commits
//!   come from small follow-up calls over narrow `merged:` time ranges.
//! - Inline review comments are not exposed by `--json reviews`; only review
//!   bodies are available, so v1 counts one comment per review body longer
//!   than 20 characters and never counts inline comments.
//! - `closingIssuesReferences` carries no labels in the installed gh, so
//!   `closesBug` is only true when a label list is present in the output.
//!
//! Every call names the repository (`-R owner/name`, from `origin`), because
//! in a fork `gh` would otherwise pick `upstream` and overlay another
//! project's PRs. The whole phase has a wall-clock budget; commit lookups run
//! on a few worker threads and their results are kept per PR in `CommitCache`.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use serde::de::DeserializeOwned;

use super::*;

const NOT_INSTALLED: &str = "GitHub CLI (`gh`) is not installed.";
const NOT_SIGNED_IN: &str = "GitHub CLI is not signed in. Run `gh auth login`.";
const NO_REMOTE: &str = "This repository has no GitHub remote.";
const UNSUPPORTED_HOST: &str = "Only github.com repositories are supported.";
const LOAD_FAILED: &str = "GitHub data could not be loaded";
const REPO_UNREADABLE: &str =
    "GitHub data could not be loaded: the repository is not readable with this account.";
const CLOSED_FAILED_NOTE: &str = "Closed pull requests could not be loaded from GitHub.";
const TOO_SLOW: &str = "GitHub data took too long to load. Try again later.";
/// What `SystemGh` reports when it had to kill a call.
const TIMED_OUT: &str = "gh timed out";

const NOTE_MAX_CHARS: usize = 200;
const MERGED_LIMIT: usize = 1000;
const CLOSED_LIMIT: usize = 500;
/// PRs per follow-up `commits` call. GitHub's node budget allows about 40.
const COMMIT_CHUNK: usize = 25;
const COMMIT_CHUNK_LIMIT: usize = 30;
/// PRs whose final diff is computed with local git; the rest use gh `files`.
const MAX_LOCAL_PRS: usize = 500;
const COMMENT_MIN_CHARS: usize = 20;
/// Wall clock for the whole gh phase; past it the git fallback is kept.
const GH_BUDGET: Duration = Duration::from_secs(90);
const CALL_TIMEOUT: Duration = Duration::from_secs(60);
const STATUS_TIMEOUT: Duration = Duration::from_secs(30);
const WORKERS: usize = 4;
const COMMIT_CACHE_CAP: usize = 5000;
const EMPTY_TREE: &str = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";

const MERGED_FIELDS: &str =
    "number,title,author,mergedAt,mergeCommit,closingIssuesReferences,reviews,baseRefName,files";
const CLOSED_FIELDS: &str = "number,title,author,closedAt,baseRefName";
const COMMIT_FIELDS: &str = "number,commits";

// ---------------------------------------------------------------------------
// runner
// ---------------------------------------------------------------------------

pub(super) enum GhError {
    NotInstalled,
    /// stderr (or stdout when stderr is empty) of a failed run
    Failed(String),
}

/// How `gh` is run. The real one spawns the binary; tests feed canned output.
pub(super) trait GhRunner: Sync {
    /// `timeout` is how long this call may take before it is killed.
    fn run(&self, root: &Path, args: &[&str], timeout: Duration) -> Result<String, GhError>;

    /// Pause before the single retry of a failed call.
    fn retry_delay(&self) -> Duration {
        Duration::from_millis(400)
    }
}

#[cfg_attr(test, allow(dead_code))]
pub(super) struct SystemGh;

impl GhRunner for SystemGh {
    fn run(&self, root: &Path, args: &[&str], timeout: Duration) -> Result<String, GhError> {
        let program = crate::harness::resolve_gui_binary("gh").ok_or(GhError::NotInstalled)?;
        let mut cmd = Command::new(program);
        cmd.current_dir(root)
            .args(args)
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GH_PROMPT_DISABLED", "1")
            .env("GH_PAGER", "cat")
            .env("GIT_PAGER", "cat")
            .env("GH_NO_UPDATE_NOTIFIER", "1")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        // Also hides the console window.
        crate::harness::apply_gui_env(&mut cmd);
        let mut child = cmd.spawn().map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                GhError::NotInstalled
            } else {
                GhError::Failed(e.to_string())
            }
        })?;
        let done = wait_with_deadline(&mut child, timeout).map_err(|failure| match failure {
            WaitFailure::TimedOut => GhError::Failed(TIMED_OUT.to_string()),
            WaitFailure::Io(e) => GhError::Failed(e),
        })?;
        if done.status.success() {
            return Ok(done.stdout);
        }
        Err(GhError::Failed(if done.stderr.trim().is_empty() {
            done.stdout
        } else {
            done.stderr
        }))
    }
}

/// Stands in for `gh` where a test does not care about GitHub.
#[cfg(test)]
pub(super) struct Unavailable;

#[cfg(test)]
impl GhRunner for Unavailable {
    fn run(&self, _: &Path, _: &[&str], _: Duration) -> Result<String, GhError> {
        Err(GhError::NotInstalled)
    }
}

// ---------------------------------------------------------------------------
// notes
// ---------------------------------------------------------------------------

/// Removes GitHub tokens (`ghp_...`, `github_pat_...`) from one word.
fn strip_tokens(word: &str) -> String {
    let mut out = String::new();
    let mut rest = word;
    'scan: loop {
        for (at, _) in rest.char_indices() {
            let tail = &rest[at..];
            let run = |skip: usize, with_underscore: bool| {
                tail[skip..]
                    .bytes()
                    .take_while(|b| b.is_ascii_alphanumeric() || (with_underscore && *b == b'_'))
                    .count()
            };
            let hit = if tail.starts_with("github_pat_") {
                Some(11 + run(11, true)).filter(|len| len - 11 >= 20)
            } else if ["ghp_", "gho_", "ghu_", "ghs_", "ghr_"]
                .iter()
                .any(|p| tail.starts_with(p))
            {
                Some(4 + run(4, false)).filter(|len| len - 4 >= 20)
            } else {
                None
            };
            if let Some(len) = hit {
                out.push_str(&rest[..at]);
                rest = &rest[at + len..];
                continue 'scan;
            }
        }
        out.push_str(rest);
        return out;
    }
}

/// The first line of gh's error output, with secrets removed and cut to 200
/// characters. Never the whole output.
fn sanitize_line(raw: &str) -> String {
    let line = raw
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("");
    let mut words: Vec<String> = Vec::new();
    let mut skip_next = false;
    for word in line.split_whitespace() {
        if skip_next {
            skip_next = false;
            continue;
        }
        if word.eq_ignore_ascii_case("bearer") {
            skip_next = true;
            continue;
        }
        let word = strip_tokens(word);
        if !word.is_empty() {
            words.push(word);
        }
    }
    let text: String = words.join(" ").chars().take(NOTE_MAX_CHARS).collect();
    if text.is_empty() {
        "unknown error".to_string()
    } else {
        text
    }
}

fn load_failure(raw: &str) -> String {
    format!("{LOAD_FAILED}: {}", sanitize_line(raw))
}

fn is_rate_limit(stderr: &str) -> bool {
    stderr.to_ascii_lowercase().contains("rate limit")
}

/// Wall clock left for the gh phase.
struct Budget {
    deadline: Instant,
}

impl Budget {
    fn new(total: Duration) -> Self {
        Self {
            deadline: Instant::now() + total,
        }
    }

    fn left(&self) -> Option<Duration> {
        self.deadline
            .checked_duration_since(Instant::now())
            .filter(|d| !d.is_zero())
    }
}

fn valid_slug(slug: &str) -> bool {
    let mut parts = slug.split('/');
    let (Some(owner), Some(name), None) = (parts.next(), parts.next(), parts.next()) else {
        return false;
    };
    let ok = |part: &str| {
        !part.is_empty()
            && !part.starts_with('-')
            && part
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
    };
    ok(owner) && ok(name)
}

/// What the `origin` remote says about where the repository lives.
#[derive(Debug, PartialEq)]
enum Origin {
    /// a github.com repository, `owner/name`
    Github(String),
    /// a host that is clearly not github.com (GitHub Enterprise, GitLab, ...)
    Foreign,
    /// no remote, or one only ssh can resolve (a host alias like `github-work`)
    Unknown,
}

/// Reads a remote URL (ssh, scp-like or https, with or without `.git`). A host
/// without a dot is taken for an ssh alias, which may well be github.com.
fn parse_origin(url: &str) -> Origin {
    let url = url.trim();
    let (authority, path) = if let Some((_, rest)) = url.split_once("://") {
        match rest.split_once('/') {
            Some(parts) => parts,
            None => return Origin::Unknown,
        }
    } else {
        // scp-like `git@github.com:owner/name.git`
        match url.split_once(':') {
            Some(parts) => parts,
            None => return Origin::Unknown,
        }
    };
    let host = authority.rsplit('@').next().unwrap_or("");
    let host = host.split(':').next().unwrap_or("").to_ascii_lowercase();
    if host.is_empty() {
        return Origin::Unknown;
    }
    if host != "github.com" && host != "ssh.github.com" {
        return if host.contains('.') {
            Origin::Foreign
        } else {
            Origin::Unknown
        };
    }
    let path = path.trim_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);
    if valid_slug(path) {
        Origin::Github(path.to_string())
    } else {
        Origin::Unknown
    }
}

/// `owner/name` of a github.com remote URL; anything else is `None`.
#[cfg(test)]
fn parse_github_slug(url: &str) -> Option<String> {
    match parse_origin(url) {
        Origin::Github(slug) => Some(slug),
        _ => None,
    }
}

fn origin(root: &Path) -> Origin {
    git(root, ["remote", "get-url", "origin"])
        .map(|url| parse_origin(&url))
        .unwrap_or(Origin::Unknown)
}

/// `gh repo view --json nameWithOwner,url` output, accepted only for github.com.
fn parse_repo_view(text: &str) -> Option<String> {
    let json: serde_json::Value = serde_json::from_str(text).ok()?;
    let url = json.get("url")?.as_str()?;
    let slug = json.get("nameWithOwner")?.as_str()?;
    (url.starts_with("https://github.com/") && valid_slug(slug)).then(|| slug.to_string())
}

/// Asks gh which repository this directory is, when `origin` could not say.
/// No `-R` is possible yet; everything after this call carries the answer.
fn ask_gh_for_slug(gh: &dyn GhRunner, root: &Path, budget: &Budget) -> Result<String, String> {
    let left = budget.left().ok_or_else(|| TOO_SLOW.to_string())?;
    match gh.run(
        root,
        &["repo", "view", "--json", "nameWithOwner,url"],
        left.min(STATUS_TIMEOUT),
    ) {
        Ok(text) => parse_repo_view(&text).ok_or_else(|| NO_REMOTE.to_string()),
        Err(GhError::NotInstalled) => Err(NOT_INSTALLED.to_string()),
        Err(GhError::Failed(_)) => Err(NO_REMOTE.to_string()),
    }
}

/// Is `gh` usable here? Ok is the repository slug, the error the note for the
/// page. The remote is read first so a repository on another host is called
/// unsupported instead of "not signed in".
fn gh_status(gh: &dyn GhRunner, root: &Path, budget: &Budget) -> Result<String, String> {
    let origin = origin(root);
    if origin == Origin::Foreign {
        return Err(UNSUPPORTED_HOST.to_string());
    }
    let left = budget.left().ok_or_else(|| TOO_SLOW.to_string())?;
    match gh.run(
        root,
        &["auth", "status", "--active", "--hostname", "github.com"],
        left.min(STATUS_TIMEOUT),
    ) {
        Ok(_) => {}
        Err(GhError::NotInstalled) => return Err(NOT_INSTALLED.to_string()),
        Err(GhError::Failed(_)) => return Err(NOT_SIGNED_IN.to_string()),
    }
    match origin {
        Origin::Github(slug) => {
            check_readable(gh, root, &slug, budget)?;
            Ok(slug)
        }
        // gh resolved this one itself, so it can read it.
        _ => ask_gh_for_slug(gh, root, budget),
    }
}

/// `gh pr list --search` answers `[]` with exit 0 for a repository that does
/// not exist or that the account cannot see, which would read as "no reviews".
/// Only the exit status matters here.
fn check_readable(
    gh: &dyn GhRunner,
    root: &Path,
    slug: &str,
    budget: &Budget,
) -> Result<(), String> {
    let left = budget.left().ok_or_else(|| TOO_SLOW.to_string())?;
    match gh.run(
        root,
        &["repo", "view", slug, "--json", "nameWithOwner"],
        left.min(STATUS_TIMEOUT),
    ) {
        Ok(_) => Ok(()),
        Err(GhError::NotInstalled) => Err(NOT_INSTALLED.to_string()),
        Err(GhError::Failed(_)) => Err(REPO_UNREADABLE.to_string()),
    }
}

/// What every gh call of one enrichment shares.
struct Ctx<'a> {
    gh: &'a dyn GhRunner,
    root: &'a Path,
    slug: &'a str,
    budget: &'a Budget,
}

impl Ctx<'_> {
    /// Runs `gh` once more after a short pause when it failed, unless the
    /// failure is a rate limit or a timeout (waiting does not help). Nothing
    /// starts after the budget is spent. The error is a note.
    fn run(&self, args: &[&str]) -> Result<String, String> {
        let mut attempt = 0;
        loop {
            let left = self.budget.left().ok_or_else(|| TOO_SLOW.to_string())?;
            match self.gh.run(self.root, args, left.min(CALL_TIMEOUT)) {
                Ok(text) => return Ok(text),
                Err(GhError::NotInstalled) => return Err(NOT_INSTALLED.to_string()),
                Err(GhError::Failed(stderr)) => {
                    attempt += 1;
                    if stderr.trim() == TIMED_OUT && self.budget.left().is_none() {
                        return Err(TOO_SLOW.to_string());
                    }
                    if attempt >= 2 || is_rate_limit(&stderr) || stderr.trim() == TIMED_OUT {
                        return Err(load_failure(&stderr));
                    }
                    std::thread::sleep(self.gh.retry_delay());
                }
            }
        }
    }

    /// `gh pr list -R <repo> <rest>` parsed as a JSON list.
    fn pr_list<T: DeserializeOwned>(&self, rest: &[&str]) -> Result<Vec<T>, String> {
        let mut args = vec!["pr", "list", "-R", self.slug];
        args.extend_from_slice(rest);
        let text = self.run(&args)?;
        serde_json::from_str(&text).map_err(|_| load_failure("unexpected output from gh"))
    }
}

// ---------------------------------------------------------------------------
// gh JSON
// ---------------------------------------------------------------------------

/// `null` and a missing key both read as the default.
fn nullable<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de> + Default,
{
    Ok(Option::<T>::deserialize(deserializer)?.unwrap_or_default())
}

#[derive(Deserialize, Default, Clone)]
#[serde(default)]
struct GhActor {
    login: Option<String>,
    name: Option<String>,
    is_bot: bool,
}

#[derive(Deserialize, Default, Clone)]
#[serde(default)]
struct GhCommitAuthor {
    name: Option<String>,
    email: Option<String>,
    login: Option<String>,
    is_bot: bool,
}

#[derive(Deserialize, Default, Clone)]
#[serde(default, rename_all = "camelCase")]
struct GhCommit {
    message_headline: Option<String>,
    authored_date: Option<String>,
    committed_date: Option<String>,
    #[serde(deserialize_with = "nullable")]
    authors: Vec<GhCommitAuthor>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct GhFile {
    path: String,
    additions: u64,
    deletions: u64,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct GhLabel {
    name: String,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct GhIssueRef {
    #[serde(deserialize_with = "nullable")]
    labels: Vec<GhLabel>,
}

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct GhReview {
    author: Option<GhActor>,
    state: Option<String>,
    submitted_at: Option<String>,
    body: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct GhOid {
    oid: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct GhPr {
    number: u64,
    title: Option<String>,
    author: Option<GhActor>,
    merged_at: Option<String>,
    closed_at: Option<String>,
    merge_commit: Option<GhOid>,
    base_ref_name: Option<String>,
    #[serde(deserialize_with = "nullable")]
    closing_issues_references: Vec<GhIssueRef>,
    #[serde(deserialize_with = "nullable")]
    reviews: Vec<GhReview>,
    #[serde(deserialize_with = "nullable")]
    files: Vec<GhFile>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct GhPrCommits {
    number: u64,
    #[serde(deserialize_with = "nullable")]
    commits: Vec<GhCommit>,
}

// ---------------------------------------------------------------------------
// mapping
// ---------------------------------------------------------------------------

fn non_empty(text: &Option<String>) -> Option<&str> {
    text.as_deref().map(str::trim).filter(|t| !t.is_empty())
}

/// `app/dependabot` and `dependabot` of a bot account both read `dependabot[bot]`,
/// the form git uses for bot commits, so the default `*[bot]*` patterns match.
fn bot_login(login: &str) -> String {
    let bare = login.strip_prefix("app/").unwrap_or(login);
    if bare.ends_with("[bot]") {
        bare.to_string()
    } else {
        format!("{bare}[bot]")
    }
}

/// Lowercase login without the `app/` prefix and the `[bot]` suffix: the same
/// account whichever way gh spells it.
fn bare_login(login: &str) -> String {
    let lower = login.trim().to_ascii_lowercase();
    let bare = lower.strip_prefix("app/").unwrap_or(&lower);
    bare.strip_suffix("[bot]").unwrap_or(bare).to_string()
}

/// gh reports review authors as a bare login, without `is_bot` or `app/`, so
/// bots that review are recognised by name: a `-bot` or `[bot]` suffix, or one
/// of the well-known review services.
fn is_review_bot(login: &str) -> bool {
    const KNOWN: [&str; 11] = [
        "coderabbitai",
        "copilot-pull-request-reviewer",
        "github-actions",
        "sonarcloud",
        "codecov",
        "codecov-commenter",
        "gemini-code-assist",
        "sourcery-ai",
        "github-advanced-security",
        "vercel",
        "netlify",
    ];
    let lower = login.trim().to_ascii_lowercase();
    lower.ends_with("-bot") || lower.ends_with("[bot]") || KNOWN.contains(&lower.as_str())
}

/// The login of a gh actor as it is reported to TypeScript.
fn actor_login(actor: &GhActor, login: &str) -> String {
    if actor.is_bot {
        bot_login(login)
    } else {
        login.to_string()
    }
}

/// GitHub shows a deleted account as "ghost", and so does this.
fn author_login(actor: &Option<GhActor>) -> String {
    let Some(actor) = actor else {
        return "ghost".to_string();
    };
    match non_empty(&actor.login) {
        Some(login) => actor_login(actor, login),
        None => "ghost".to_string(),
    }
}

/// The PR author. The email comes from the PR's own commits written by the
/// same login (first non-empty one); without it the email stays empty and the
/// TypeScript side resolves the person through `gh:<login>`.
fn pr_author(actor: &Option<GhActor>, commits: &[GhCommit]) -> GitIdentity {
    let login = author_login(actor);
    let name = actor
        .as_ref()
        .and_then(|a| non_empty(&a.name))
        .unwrap_or(&login)
        .to_string();
    let bare = bare_login(&login);
    let email = commits
        .iter()
        .flat_map(|c| c.authors.iter())
        .filter(|a| non_empty(&a.login).is_some_and(|l| bare_login(l) == bare))
        .find_map(|a| non_empty(&a.email))
        .unwrap_or("")
        .to_string();
    GitIdentity {
        name,
        email,
        login: Some(login),
    }
}

/// A commit author. The PR author's own commits take the PR author's login
/// (a bot's commits carry `dependabot`, its PR `app/dependabot`).
fn commit_author(author: &GhCommitAuthor, pr_author: &GitIdentity) -> GitIdentity {
    let raw = non_empty(&author.login);
    let login = match raw {
        Some(raw)
            if pr_author
                .login
                .as_deref()
                .is_some_and(|l| bare_login(l) == bare_login(raw)) =>
        {
            pr_author.login.clone()
        }
        Some(raw) if author.is_bot => Some(bot_login(raw)),
        Some(raw) => Some(raw.to_string()),
        None => None,
    };
    GitIdentity {
        name: non_empty(&author.name).unwrap_or("").to_string(),
        email: non_empty(&author.email).unwrap_or("").to_string(),
        login,
    }
}

fn branch_commits(commits: &[GhCommit], pr_author: &GitIdentity) -> Vec<RawBranchCommit> {
    commits
        .iter()
        .map(|c| {
            let authored = non_empty(&c.authored_date)
                .or_else(|| non_empty(&c.committed_date))
                .unwrap_or("")
                .to_string();
            let committed = non_empty(&c.committed_date)
                .unwrap_or(&authored)
                .to_string();
            let mut authors: Vec<GitIdentity> = c
                .authors
                .iter()
                .map(|a| commit_author(a, pr_author))
                .collect();
            if authors.is_empty() {
                authors.push(pr_author.clone());
            }
            RawBranchCommit {
                authored_at: authored,
                committed_at: committed,
                authors,
            }
        })
        .collect()
}

fn gh_files(files: &[GhFile]) -> Vec<RawFile> {
    files
        .iter()
        .filter(|f| !f.path.is_empty() && (f.additions > 0 || f.deletions > 0))
        .map(|f| RawFile {
            path: f.path.clone(),
            added: f.additions,
            deleted: f.deletions,
        })
        .collect()
}

fn closes_bug(pr: &GhPr) -> bool {
    pr.closing_issues_references
        .iter()
        .flat_map(|issue| issue.labels.iter())
        .any(|label| label.name.trim().eq_ignore_ascii_case("bug"))
}

/// One review per (PR, review). Own reviews, unsubmitted ones and ones outside
/// the window are dropped.
fn map_reviews(pr: &GhPr, id: &str, since_epoch: i64, until_epoch: i64) -> Vec<RawReview> {
    let author = bare_login(&author_login(&pr.author));
    pr.reviews
        .iter()
        .filter_map(|review| {
            let actor = review.author.as_ref()?;
            let raw = non_empty(&actor.login)?;
            if bare_login(raw) == author {
                return None;
            }
            let reviewer = if is_review_bot(raw) {
                bot_login(raw)
            } else {
                actor_login(actor, raw)
            };
            let submitted = non_empty(&review.submitted_at)?;
            let epoch = parse_iso_epoch(submitted)?;
            if epoch < since_epoch || epoch > until_epoch {
                return None;
            }
            let name = non_empty(&actor.name).unwrap_or(&reviewer).to_string();
            let long_body = review
                .body
                .as_deref()
                .is_some_and(|b| b.trim().chars().count() > COMMENT_MIN_CHARS);
            Some(RawReview {
                pr_id: id.to_string(),
                reviewer: GitIdentity {
                    name,
                    email: String::new(),
                    login: Some(reviewer),
                },
                state: non_empty(&review.state).unwrap_or("COMMENTED").to_string(),
                submitted_at: submitted.to_string(),
                comments: u64::from(long_body),
            })
        })
        .collect()
}

/// What the local repository knows about a landed PR.
pub(super) struct Landing {
    /// Final diff; `None` when it could not be computed
    pub files: Option<Vec<RawFile>>,
    pub capped: bool,
    pub committer: GitIdentity,
    pub co_authors: Vec<GitIdentity>,
    /// The landing commit plus every commit that came in with it
    pub hashes: Vec<String>,
}

/// What gh says about one commit of a PR, to recognise it in git history.
pub(super) struct CommitProbe {
    pub headline: String,
    /// author email; empty when gh did not report one
    pub email: String,
}

/// GitHub cuts a commit headline to about 70 characters and appends "…", and
/// gh gives the first line only where `%s` joins the first paragraph. So two
/// headlines match when one is a prefix of the other once a trailing ellipsis
/// is dropped.
fn headlines_match(git: &str, gh: &str) -> bool {
    let normal = |text: &str| {
        let text = text.trim();
        let text = text
            .strip_suffix('\u{2026}')
            .or_else(|| text.strip_suffix("..."))
            .unwrap_or(text);
        text.trim_end().to_string()
    };
    let (git, gh) = (normal(git), normal(gh));
    !git.is_empty() && !gh.is_empty() && (git.starts_with(&gh) || gh.starts_with(&git))
}

pub(super) fn is_object_id(text: &str) -> bool {
    matches!(text.len(), 40 | 64) && text.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Reads the landing of `merge_commit` from the local repository, `None` when
/// the commit is not there. One `git log -1` both proves the commit exists and
/// reads its parents, committer and body.
///
/// - merge commit (two parents): diff against the first parent; the commits
///   that came in are those reachable from the other parents only
/// - rebase merge: the PR had `n > 1` commits and the `n` latest first-parent
///   commits match them in order (headlines tolerant of GitHub's truncation,
///   authors equal when both sides know the email); diff over those `n`
/// - otherwise a squash (or a single commit): diff against the parent
pub(super) fn inspect_landing(
    root: &Path,
    merge_commit: &str,
    probes: &[CommitProbe],
    limits: &Limits,
) -> Option<Landing> {
    if !is_object_id(merge_commit) {
        return None;
    }
    let info = git(
        root,
        [
            "log",
            "-1",
            "--no-show-signature",
            "--format=%P%x00%cn%x00%ce%x00%B",
            &format!("{merge_commit}^{{commit}}"),
            "--",
        ],
    )
    .ok()?;
    let mut fields = info.splitn(4, '\0');
    let parents: Vec<String> = fields
        .next()?
        .split_whitespace()
        .map(str::to_string)
        .collect();
    let committer = identity(fields.next()?, fields.next()?);
    let body = fields.next().unwrap_or("");

    let mut hashes = vec![merge_commit.to_string()];
    let mut branch_capped = false;
    let base = match parents.len() {
        0 => EMPTY_TREE.to_string(),
        1 => {
            let n = probes.len();
            let rebased = if n > 1 {
                git(
                    root,
                    [
                        "log",
                        "--first-parent",
                        "--no-show-signature",
                        "-n",
                        &n.to_string(),
                        "--format=%H%x00%ae%x00%s",
                        merge_commit,
                        "--",
                    ],
                )
                .ok()
                .map(|out| {
                    out.lines()
                        .filter_map(|l| {
                            let mut parts = l.splitn(3, '\0');
                            Some((
                                parts.next()?.to_string(),
                                parts.next()?.trim().to_string(),
                                parts.next()?.to_string(),
                            ))
                        })
                        .rev()
                        .collect::<Vec<_>>()
                })
                .filter(|chain| {
                    chain.len() == n
                        && chain
                            .iter()
                            .zip(probes)
                            .all(|((_, email, subject), probe)| {
                                headlines_match(subject, &probe.headline)
                                    && (email.is_empty()
                                        || probe.email.is_empty()
                                        || email.eq_ignore_ascii_case(&probe.email))
                            })
                })
            } else {
                None
            };
            match rebased {
                Some(chain) => {
                    hashes = chain.into_iter().map(|(h, _, _)| h).collect();
                    format!("{merge_commit}~{n}")
                }
                None => format!("{merge_commit}^"),
            }
        }
        _ => {
            let mut args: Vec<String> = ["rev-list", "--no-merges", "-n"]
                .map(str::to_string)
                .to_vec();
            args.push((limits.max_branch_commits + 1).to_string());
            args.push(format!("^{}", parents[0]));
            args.extend(parents[1..].iter().cloned());
            args.push("--".to_string());
            if let Ok(out) = git(root, &args) {
                let came_in: Vec<&str> = out
                    .lines()
                    .map(str::trim)
                    .filter(|h| is_object_id(h))
                    .collect();
                branch_capped = came_in.len() > limits.max_branch_commits;
                hashes.extend(
                    came_in
                        .into_iter()
                        .take(limits.max_branch_commits)
                        .map(str::to_string),
                );
            }
            parents[0].clone()
        }
    };

    let diff = git(
        root,
        [
            "diff",
            "--numstat",
            "-z",
            "-w",
            "-M",
            "--no-color",
            "--no-ext-diff",
            base.as_str(),
            merge_commit,
            "--",
        ],
    )
    .ok()
    .map(|out| parse_numstat_z(&out, limits.max_files_per_commit));
    let (files, capped) = match diff {
        Some((files, capped)) => (Some(files), capped),
        None => (None, false),
    };
    Some(Landing {
        files,
        capped: capped || branch_capped,
        committer,
        co_authors: parse_co_authors(body),
        hashes,
    })
}

// ---------------------------------------------------------------------------
// fetching
// ---------------------------------------------------------------------------

/// `base:<branch>` narrows the search to PRs into the collected branch when the
/// name is plain; odd names rely on the local `baseRefName` check alone.
fn base_clause(branch: &str) -> String {
    let plain = !branch.is_empty()
        && branch
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-' | b'/'));
    if plain && branch != "HEAD" {
        format!(" base:{branch}")
    } else {
        String::new()
    }
}

fn matches_branch(pr: &GhPr, branch: &str) -> bool {
    branch == "HEAD" || non_empty(&pr.base_ref_name).is_none_or(|base| base == branch)
}

type CommitsByPr = HashMap<u64, Arc<Vec<GhCommit>>>;

/// (repository slug, PR number, merged or closed time)
type CommitKey = (String, u64, String);

/// Rows of one search, or the note of its failure.
type SearchResult = Result<Vec<GhPrCommits>, String>;

/// Commit lists of PRs already fetched, kept for the life of the process and
/// keyed by (repository, PR number, merged or closed time): a refresh after the
/// branch moved reuses them, and a PR that changed gets a new key.
pub(super) struct CommitCache {
    entries: Mutex<BTreeMap<CommitKey, Arc<Vec<GhCommit>>>>,
    cap: usize,
}

impl CommitCache {
    pub(super) const fn new() -> Self {
        Self::with_cap(COMMIT_CACHE_CAP)
    }

    const fn with_cap(cap: usize) -> Self {
        Self {
            entries: Mutex::new(BTreeMap::new()),
            cap,
        }
    }

    fn get(&self, key: &CommitKey) -> Option<Arc<Vec<GhCommit>>> {
        lock(&self.entries).get(key).cloned()
    }

    /// Over the cap the smallest keys (oldest PR numbers) go first.
    fn insert(&self, key: CommitKey, commits: Arc<Vec<GhCommit>>) {
        let mut entries = lock(&self.entries);
        entries.insert(key, commits);
        while entries.len() > self.cap {
            entries.pop_first();
        }
    }
}

/// One `gh` call per search, on up to `WORKERS` threads. Rows come back in the
/// order of `searches`; the first failure (in that order) is the result.
fn run_searches(
    ctx: &Ctx,
    state: &str,
    searches: &[String],
) -> Result<Vec<Vec<GhPrCommits>>, String> {
    let next = AtomicUsize::new(0);
    let failed = AtomicBool::new(false);
    let slots: Vec<Mutex<Option<SearchResult>>> =
        searches.iter().map(|_| Mutex::new(None)).collect();
    let limit = COMMIT_CHUNK_LIMIT.to_string();
    std::thread::scope(|scope| {
        for _ in 0..WORKERS.min(searches.len()) {
            scope.spawn(|| loop {
                if failed.load(Ordering::Relaxed) {
                    break;
                }
                let index = next.fetch_add(1, Ordering::Relaxed);
                let Some(search) = searches.get(index) else {
                    break;
                };
                let rows = ctx.pr_list(&[
                    "--state",
                    state,
                    "--search",
                    search,
                    "--limit",
                    &limit,
                    "--json",
                    COMMIT_FIELDS,
                ]);
                if rows.is_err() {
                    failed.store(true, Ordering::Relaxed);
                }
                *lock(&slots[index]) = Some(rows);
            });
        }
    });
    let mut all = Vec::new();
    for slot in slots {
        match slot.into_inner().unwrap_or_else(|e| e.into_inner()) {
            Some(Ok(rows)) => all.push(rows),
            Some(Err(note)) => return Err(note),
            None => return Err(TOO_SLOW.to_string()),
        }
    }
    Ok(all)
}

/// Commits of the given PRs: from the cache, else in a few calls over narrow
/// time ranges. `kind` is the search qualifier (`merged` or `closed`), `state`
/// the gh state and `extra` further search terms.
fn fetch_commits(
    ctx: &Ctx,
    cache: &CommitCache,
    state: &str,
    kind: &str,
    extra: &str,
    items: &[(u64, String)],
) -> Result<(CommitsByPr, bool), String> {
    let mut found: CommitsByPr = HashMap::new();
    // In time order, runs of PRs that are not cached. A cached PR ends a run:
    // a `merged:lo..hi` range spanning it would return it again and could push
    // the PRs that are wanted out of the 30 rows.
    let mut ordered: Vec<&(u64, String)> = items.iter().collect();
    ordered.sort_by_key(|(_, at)| parse_iso_epoch(at).unwrap_or(0));
    let mut missing: Vec<&(u64, String)> = Vec::new();
    let mut runs: Vec<Vec<&(u64, String)>> = Vec::new();
    let mut run: Vec<&(u64, String)> = Vec::new();
    for item in ordered {
        let key = (ctx.slug.to_string(), item.0, item.1.clone());
        match cache.get(&key) {
            Some(hit) => {
                found.insert(item.0, hit);
                if !run.is_empty() {
                    runs.push(std::mem::take(&mut run));
                }
            }
            None => {
                missing.push(item);
                run.push(item);
            }
        }
    }
    if !run.is_empty() {
        runs.push(run);
    }
    let searches: Vec<String> = runs
        .iter()
        .flat_map(|run| run.chunks(COMMIT_CHUNK))
        .map(|group| {
            let (lo, hi) = (&group[0].1, &group[group.len() - 1].1);
            format!("{kind}:{lo}..{hi}{extra}")
        })
        .collect();
    let wanted: HashMap<u64, &String> = missing.iter().map(|(n, at)| (*n, at)).collect();
    let mut truncated = false;
    for rows in run_searches(ctx, state, &searches)? {
        truncated |= rows.len() >= COMMIT_CHUNK_LIMIT;
        for row in rows {
            let Some(at) = wanted.get(&row.number) else {
                continue;
            };
            let commits = Arc::new(row.commits);
            cache.insert(
                (ctx.slug.to_string(), row.number, (*at).clone()),
                commits.clone(),
            );
            found.insert(row.number, commits);
        }
    }
    Ok((found, truncated))
}

struct Fetched {
    merged: Vec<GhPr>,
    merged_commits: CommitsByPr,
    closed: Vec<GhPr>,
    closed_commits: CommitsByPr,
    truncated: bool,
    /// The closed-PR fetch failed; the merged data is still complete.
    closed_failed: bool,
}

fn fetch_closed(
    ctx: &Ctx,
    cache: &CommitCache,
    branch: &str,
    range: &str,
) -> Result<(Vec<GhPr>, CommitsByPr, bool), String> {
    let base = base_clause(branch);
    let search = format!("closed:{range} -is:merged{base}");
    let limit = CLOSED_LIMIT.to_string();
    let mut closed: Vec<GhPr> = ctx.pr_list(&[
        "--state",
        "closed",
        "--search",
        &search,
        "--limit",
        &limit,
        "--json",
        CLOSED_FIELDS,
    ])?;
    let cut_list = closed.len() >= CLOSED_LIMIT;
    closed.retain(|pr| matches_branch(pr, branch) && non_empty(&pr.closed_at).is_some());
    let wanted: Vec<(u64, String)> = closed
        .iter()
        .filter_map(|pr| Some((pr.number, non_empty(&pr.closed_at)?.to_string())))
        .collect();
    let extra = format!(" -is:merged{base}");
    let (commits, cut_commits) = fetch_commits(ctx, cache, "closed", "closed", &extra, &wanted)?;
    Ok((closed, commits, cut_list || cut_commits))
}

fn fetch_all(
    ctx: &Ctx,
    cache: &CommitCache,
    branch: &str,
    window: &Window,
) -> Result<Fetched, String> {
    let range = format!(
        "{}..{}",
        epoch_to_iso(window.since_epoch),
        epoch_to_iso(window.until_epoch)
    );
    let base = base_clause(branch);
    let mut truncated = false;

    let search = format!("merged:{range}{base}");
    let limit = MERGED_LIMIT.to_string();
    let mut merged: Vec<GhPr> = ctx.pr_list(&[
        "--state",
        "merged",
        "--search",
        &search,
        "--limit",
        &limit,
        "--json",
        MERGED_FIELDS,
    ])?;
    truncated |= merged.len() >= MERGED_LIMIT;
    merged.retain(|pr| matches_branch(pr, branch) && non_empty(&pr.merged_at).is_some());
    let wanted: Vec<(u64, String)> = merged
        .iter()
        .filter_map(|pr| Some((pr.number, non_empty(&pr.merged_at)?.to_string())))
        .collect();
    let (merged_commits, cut) = fetch_commits(ctx, cache, "merged", "merged", &base, &wanted)?;
    truncated |= cut;

    // The merged fetch is what makes gh usable; closed PRs only add to it. A
    // failure there is not a reason to drop everything. It is not "history too
    // large" either, so it gets its own note instead of `truncated`.
    let mut closed_failed = false;
    let (closed, closed_commits) = match fetch_closed(ctx, cache, branch, &range) {
        Ok((closed, commits, cut)) => {
            truncated |= cut;
            (closed, commits)
        }
        // Out of time is out of time for everything.
        Err(note) if note == TOO_SLOW => return Err(note),
        Err(_) => {
            closed_failed = true;
            (Vec::new(), HashMap::new())
        }
    };
    Ok(Fetched {
        merged,
        merged_commits,
        closed,
        closed_commits,
        truncated,
        closed_failed,
    })
}

// ---------------------------------------------------------------------------
// enrichment
// ---------------------------------------------------------------------------

pub(super) struct Outcome {
    pub available: bool,
    pub note: Option<String>,
    pub reviews: Vec<RawReview>,
    pub truncated: bool,
}

impl Outcome {
    fn unavailable(note: String) -> Self {
        Self {
            available: false,
            note: Some(note),
            reviews: Vec::new(),
            truncated: false,
        }
    }
}

fn merged_pr(pr: &GhPr, commits: &[GhCommit], landing: Option<&Landing>) -> RawPr {
    let author = pr_author(&pr.author, commits);
    let mut co_authors = landing.map(|l| l.co_authors.clone()).unwrap_or_default();
    if !author.email.is_empty() {
        co_authors.retain(|c| !c.email.eq_ignore_ascii_case(&author.email));
    }
    RawPr {
        id: pr.number.to_string(),
        number: Some(pr.number),
        title: non_empty(&pr.title).unwrap_or("").to_string(),
        state: "merged".to_string(),
        source: "gh".to_string(),
        committer: landing.map(|l| l.committer.clone()),
        merged_at: non_empty(&pr.merged_at).map(str::to_string),
        closed_at: None,
        merge_commit: pr
            .merge_commit
            .as_ref()
            .and_then(|m| non_empty(&m.oid))
            .map(str::to_string),
        commit_count: (commits.len() as u64).max(1),
        branch_commits: branch_commits(commits, &author),
        landing_co_authors: co_authors,
        files: landing
            .and_then(|l| l.files.clone())
            .unwrap_or_else(|| gh_files(&pr.files)),
        closes_bug: closes_bug(pr),
        author,
    }
}

fn closed_pr(pr: &GhPr, commits: &[GhCommit]) -> RawPr {
    let author = pr_author(&pr.author, commits);
    RawPr {
        id: pr.number.to_string(),
        number: Some(pr.number),
        title: non_empty(&pr.title).unwrap_or("").to_string(),
        state: "closed".to_string(),
        source: "gh".to_string(),
        committer: None,
        merged_at: None,
        closed_at: non_empty(&pr.closed_at).map(str::to_string),
        merge_commit: None,
        commit_count: (commits.len() as u64).max(1),
        branch_commits: branch_commits(commits, &author),
        landing_co_authors: Vec::new(),
        files: Vec::new(),
        closes_bug: false,
        author,
    }
}

/// Replaces the git-derived PRs by GitHub's. Git PRs whose landing commit is
/// part of a GitHub PR disappear (a rebase merge was several of them); the
/// rest, such as direct pushes, stay as `source: "git"`.
fn overlay(derived: &mut Derived, gh_drafts: Vec<PrDraft>) {
    let tagged: HashSet<&str> = gh_drafts
        .iter()
        .flat_map(|d| d.hashes.iter().map(String::as_str))
        .collect();
    derived.drafts.retain(|d| {
        !d.pr
            .merge_commit
            .as_deref()
            .is_some_and(|m| tagged.contains(m))
    });

    // A git PR keyed `#N` that GitHub did not match must not share the id of a
    // GitHub PR: it falls back to its landing hash.
    let gh_ids: HashSet<String> = gh_drafts.iter().map(|d| d.pr.id.clone()).collect();
    let mut used: HashSet<String> = derived
        .drafts
        .iter()
        .map(|d| d.pr.id.clone())
        .chain(gh_ids.iter().cloned())
        .collect();
    for draft in derived.drafts.iter_mut() {
        // With GitHub's numbers available, a `#N` read from a git subject is
        // not trusted as a PR number any more (it may belong to an upstream).
        draft.pr.number = None;
        if !gh_ids.contains(&draft.pr.id) {
            continue;
        }
        let landing = draft.pr.merge_commit.clone().unwrap_or_default();
        let fresh = [short_hash(&landing), landing]
            .into_iter()
            .find(|c| !c.is_empty() && !used.contains(c))
            .unwrap_or_else(|| format!("git-{}", draft.pr.id));
        used.insert(fresh.clone());
        draft.pr.id = fresh.clone();
        for hash in &draft.hashes {
            derived.commit_pr.insert(hash.clone(), fresh.clone());
        }
    }

    for draft in &gh_drafts {
        for hash in &draft.hashes {
            derived.commit_pr.insert(hash.clone(), draft.pr.id.clone());
        }
    }
    derived.drafts.extend(gh_drafts);
    let live: HashSet<&str> = derived.drafts.iter().map(|d| d.pr.id.as_str()).collect();
    derived.commit_pr.retain(|_, id| live.contains(id.as_str()));
}

/// What `enrich` works with.
pub(super) struct Request<'a> {
    pub gh: &'a dyn GhRunner,
    pub commits: &'a CommitCache,
    pub root: &'a Path,
    pub branch: &'a str,
    pub window: &'a Window,
    pub limits: &'a Limits,
}

/// Fetches GitHub's view of the window and overlays it on `derived`. On any
/// failure, or when the wall-clock budget runs out, `derived` is left as the
/// git fallback and the outcome says why.
pub(super) fn enrich(request: &Request, derived: &mut Derived) -> Outcome {
    enrich_within(request, derived, GH_BUDGET)
}

fn enrich_within(request: &Request, derived: &mut Derived, total: Duration) -> Outcome {
    let Request {
        gh,
        commits: cache,
        root,
        branch,
        window,
        limits,
    } = *request;
    let budget = Budget::new(total);
    let slug = match gh_status(gh, root, &budget) {
        Ok(slug) => slug,
        Err(note) => return Outcome::unavailable(note),
    };
    let ctx = Ctx {
        gh,
        root,
        slug: &slug,
        budget: &budget,
    };
    let fetched = match fetch_all(&ctx, cache, branch, window) {
        Ok(fetched) => fetched,
        Err(note) => return Outcome::unavailable(note),
    };
    let mut truncated = fetched.truncated;

    // Newest first, so the PRs that miss the local-diff cap are the oldest.
    let mut order: Vec<&GhPr> = fetched.merged.iter().collect();
    order.sort_by_key(|pr| {
        std::cmp::Reverse(parse_iso_epoch(pr.merged_at.as_deref().unwrap_or("")))
    });
    let no_commits: Arc<Vec<GhCommit>> = Arc::new(Vec::new());
    let mut drafts: Vec<PrDraft> = Vec::new();
    let mut reviews: Vec<RawReview> = Vec::new();
    for (index, pr) in order.into_iter().enumerate() {
        if budget.left().is_none() {
            return Outcome::unavailable(TOO_SLOW.to_string());
        }
        let commits = fetched
            .merged_commits
            .get(&pr.number)
            .unwrap_or(&no_commits);
        let landing = if index < MAX_LOCAL_PRS {
            let oid = pr.merge_commit.as_ref().and_then(|m| non_empty(&m.oid));
            let probes: Vec<CommitProbe> = commits
                .iter()
                .map(|c| CommitProbe {
                    headline: non_empty(&c.message_headline).unwrap_or("").to_string(),
                    email: c
                        .authors
                        .first()
                        .and_then(|a| non_empty(&a.email))
                        .unwrap_or("")
                        .to_string(),
                })
                .collect();
            oid.and_then(|oid| inspect_landing(root, oid, &probes, limits))
        } else {
            None
        };
        if let Some(landing) = &landing {
            truncated |= landing.capped;
        }
        let raw = merged_pr(pr, commits, landing.as_ref());
        reviews.extend(map_reviews(
            pr,
            &raw.id,
            window.since_epoch,
            window.until_epoch,
        ));
        let mut hashes: HashSet<String> = landing
            .map(|l| l.hashes.into_iter().collect())
            .unwrap_or_default();
        if let Some(mc) = &raw.merge_commit {
            hashes.insert(mc.clone());
        }
        drafts.push(PrDraft { pr: raw, hashes });
    }
    for pr in &fetched.closed {
        let commits = fetched
            .closed_commits
            .get(&pr.number)
            .unwrap_or(&no_commits);
        drafts.push(PrDraft {
            pr: closed_pr(pr, commits),
            hashes: HashSet::new(),
        });
    }
    overlay(derived, drafts);
    Outcome {
        available: true,
        note: fetched
            .closed_failed
            .then(|| CLOSED_FAILED_NOTE.to_string()),
        reviews,
        truncated,
    }
}

#[cfg(test)]
type Handler = Box<dyn Fn(&[&str]) -> Result<String, GhError> + Send + Sync>;

#[cfg(test)]
pub(super) struct FakeGh {
    handler: Handler,
    pub calls: Mutex<Vec<Vec<String>>>,
}

#[cfg(test)]
impl FakeGh {
    pub fn new(
        handler: impl Fn(&[&str]) -> Result<String, GhError> + Send + Sync + 'static,
    ) -> Self {
        Self {
            handler: Box::new(handler),
            calls: Mutex::new(Vec::new()),
        }
    }
}

/// `gh repo view [<repository>] --json <fields>`: the repository is positional
/// (`repo view` has no `-R`), and nothing else is accepted. The real gh fails
/// on any other shape, so the fake must not let one pass.
#[cfg(test)]
fn strict_repo_view(args: &[&str]) -> Result<(), String> {
    let rest = &args[2..];
    let rest = match rest.first() {
        Some(first) if !first.starts_with('-') => {
            if !valid_slug(first) {
                return Err(format!("invalid repository argument {first:?}"));
            }
            &rest[1..]
        }
        _ => rest,
    };
    match rest {
        ["--json", fields] if !fields.is_empty() && !fields.starts_with('-') => Ok(()),
        _ => Err(format!("unsupported gh repo view arguments {args:?}")),
    }
}

#[cfg(test)]
impl GhRunner for FakeGh {
    fn run(&self, _: &Path, args: &[&str], _: Duration) -> Result<String, GhError> {
        if args.starts_with(&["repo", "view"]) {
            if let Err(e) = strict_repo_view(args) {
                panic!("{e}");
            }
        }
        lock(&self.calls).push(args.iter().map(|a| a.to_string()).collect());
        (self.handler)(args)
    }

    fn retry_delay(&self) -> Duration {
        Duration::ZERO
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SINCE: &str = "2026-01-01T00:00:00Z";
    const UNTIL: &str = "2026-06-30T00:00:00Z";

    fn failed(text: &str) -> GhError {
        GhError::Failed(text.to_string())
    }

    fn window() -> Window {
        Window::new(SINCE, Some(UNTIL)).unwrap()
    }

    fn parse<T: DeserializeOwned>(json: serde_json::Value) -> Vec<T> {
        serde_json::from_value(json).unwrap()
    }

    fn only_pr(json: serde_json::Value) -> GhPr {
        parse::<GhPr>(serde_json::json!([json])).remove(0)
    }

    fn scratch_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("monoptah-gh-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn repo_with_origin(url: &str) -> PathBuf {
        let dir = scratch_dir();
        git(&dir, ["init", "-q"]).unwrap();
        git(&dir, ["remote", "add", "origin", url]).unwrap();
        dir
    }

    fn budget() -> Budget {
        Budget::new(Duration::from_secs(60))
    }

    fn with_ctx<R>(github: &dyn GhRunner, total: Duration, f: impl FnOnce(&Ctx) -> R) -> R {
        let budget = Budget::new(total);
        let ctx = Ctx {
            gh: github,
            root: Path::new("."),
            slug: "acme/widgets",
            budget: &budget,
        };
        f(&ctx)
    }

    fn fetch(github: &FakeGh, cache: &CommitCache, branch: &str) -> Result<Fetched, String> {
        with_ctx(github, Duration::from_secs(60), |ctx| {
            fetch_all(ctx, cache, branch, &window())
        })
    }

    fn repo_view() -> String {
        serde_json::json!({
            "nameWithOwner": "acme/widgets", "url": "https://github.com/acme/widgets"
        })
        .to_string()
    }

    // availability notes

    #[test]
    fn availability_maps_each_failure_to_its_fixed_note() {
        let root = scratch_dir();
        let ok = FakeGh::new(|args| {
            Ok(if args[0] == "repo" {
                repo_view()
            } else {
                String::new()
            })
        });
        assert_eq!(
            gh_status(&ok, &root, &budget()),
            Ok("acme/widgets".to_string())
        );
        let calls = lock(&ok.calls).clone();
        assert_eq!(
            calls[0],
            ["auth", "status", "--active", "--hostname", "github.com"]
        );
        assert_eq!(calls[1], ["repo", "view", "--json", "nameWithOwner,url"]);

        assert_eq!(
            gh_status(&Unavailable, &root, &budget()),
            Err("GitHub CLI (`gh`) is not installed.".to_string())
        );
        let signed_out =
            FakeGh::new(|_| Err(failed("You are not logged in ghp_aaaaaaaaaaaaaaaaaaaaaaaa")));
        assert_eq!(
            gh_status(&signed_out, &root, &budget()),
            Err("GitHub CLI is not signed in. Run `gh auth login`.".to_string())
        );
        let no_remote = FakeGh::new(|args| {
            if args[0] == "repo" {
                Err(failed("no git remotes found"))
            } else {
                Ok(String::new())
            }
        });
        assert_eq!(
            gh_status(&no_remote, &root, &budget()),
            Err("This repository has no GitHub remote.".to_string())
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn github_remote_urls_give_the_repository_slug() {
        let slug = |url: &str| parse_github_slug(url);
        for url in [
            "git@github.com:acme/widgets.git",
            "git@github.com:acme/widgets",
            "https://github.com/acme/widgets.git",
            "https://github.com/acme/widgets/",
            "https://user:token@github.com/acme/widgets",
            "ssh://git@github.com/acme/widgets.git",
            "ssh://git@github.com:22/acme/widgets.git",
            "  https://GitHub.com/acme/widgets.git\n",
        ] {
            assert_eq!(slug(url).as_deref(), Some("acme/widgets"), "{url}");
        }
        assert_eq!(
            slug("git@github.com:my-org/my.repo_1.git").as_deref(),
            Some("my-org/my.repo_1")
        );
        for url in [
            "https://ghe.example.com/acme/widgets.git",
            "git@ghe.example.com:acme/widgets.git",
            "https://github.com.evil.example/acme/widgets",
            "https://evilgithub.com/acme/widgets",
            "https://github.com/acme",
            "https://github.com/acme/widgets/extra",
            "https://github.com/-x/widgets",
            "https://github.com/acme/wid gets",
            "/local/path/widgets",
            "file:///tmp/widgets",
            "",
        ] {
            assert_eq!(slug(url), None, "{url}");
        }
    }

    #[test]
    fn the_origin_remote_names_the_repository_without_asking_gh() {
        let root = repo_with_origin("git@github.com:acme/widgets.git");
        let github = FakeGh::new(|_| Ok(String::new()));
        assert_eq!(
            gh_status(&github, &root, &budget()),
            Ok("acme/widgets".to_string())
        );
        // Sign-in, then a readability check that names the repository (as a
        // positional argument, `repo view` has no `-R`): never a bare
        // `gh repo view`, which could pick upstream.
        let calls = lock(&github.calls).clone();
        assert_eq!(calls.len(), 2);
        assert_eq!(
            calls[1],
            ["repo", "view", "acme/widgets", "--json", "nameWithOwner"]
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn an_unparseable_origin_falls_back_to_what_gh_resolves() {
        // An ssh host alias: github.com behind a name only ssh knows.
        let alias = repo_with_origin("git@github-work:acme/widgets.git");
        let resolves = FakeGh::new(|args| {
            Ok(if args[0] == "repo" {
                repo_view()
            } else {
                String::new()
            })
        });
        assert_eq!(
            gh_status(&resolves, &alias, &budget()),
            Ok("acme/widgets".to_string())
        );
        assert_eq!(lock(&resolves.calls).len(), 2);

        // gh resolving to another host is not usable.
        let foreign = FakeGh::new(|args| {
            Ok(if args[0] == "repo" {
                serde_json::json!({
                    "nameWithOwner": "acme/widgets", "url": "https://ghe.example.com/acme/widgets"
                })
                .to_string()
            } else {
                String::new()
            })
        });
        assert_eq!(
            gh_status(&foreign, &alias, &budget()),
            Err("This repository has no GitHub remote.".to_string())
        );
        let unusable = FakeGh::new(|args| {
            if args[0] == "repo" {
                Ok("not json".to_string())
            } else {
                Ok(String::new())
            }
        });
        assert_eq!(
            gh_status(&unusable, &alias, &budget()),
            Err("This repository has no GitHub remote.".to_string())
        );
        let _ = std::fs::remove_dir_all(&alias);
    }

    #[test]
    fn the_fake_gh_accepts_only_argument_shapes_the_real_one_accepts() {
        assert!(strict_repo_view(&["repo", "view", "--json", "nameWithOwner,url"]).is_ok());
        assert!(
            strict_repo_view(&["repo", "view", "acme/widgets", "--json", "nameWithOwner"]).is_ok()
        );
        for bad in [
            &[
                "repo",
                "view",
                "-R",
                "acme/widgets",
                "--json",
                "nameWithOwner",
            ][..],
            &[
                "repo",
                "view",
                "--repo",
                "acme/widgets",
                "--json",
                "nameWithOwner",
            ],
            &["repo", "view", "acme/widgets"],
            &["repo", "view", "--json"],
            &["repo", "view", "-x", "--json", "nameWithOwner"],
            &["repo", "view", "a b", "--json", "nameWithOwner"],
            &[
                "repo",
                "view",
                "acme/widgets",
                "--json",
                "nameWithOwner",
                "extra",
            ],
        ] {
            assert!(strict_repo_view(bad).is_err(), "{bad:?}");
        }
        let github = FakeGh::new(|_| Ok(String::new()));
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            github.run(
                Path::new("."),
                &[
                    "repo",
                    "view",
                    "-R",
                    "acme/widgets",
                    "--json",
                    "nameWithOwner",
                ],
                Duration::from_secs(1),
            )
        }));
        assert!(outcome.is_err());
    }

    #[test]
    fn a_repository_gh_cannot_read_is_not_available() {
        // `gh pr list --search` answers `[]` for a repository that does not
        // exist or that the token cannot see, so readability is checked first.
        let root = repo_with_origin("git@github.com:acme/widgets.git");
        let unreadable = FakeGh::new(|args| {
            if args[0] == "repo" {
                Err(failed(
                    "Could not resolve to a Repository ghp_aaaaaaaaaaaaaaaaaaaaaaaa",
                ))
            } else {
                Ok(String::new())
            }
        });
        let note = gh_status(&unreadable, &root, &budget()).unwrap_err();
        assert_eq!(
            note,
            "GitHub data could not be loaded: the repository is not readable with this account."
        );

        let cache = CommitCache::new();
        let limits = Limits::DEFAULT;
        let win = window();
        let request = Request {
            gh: &unreadable,
            commits: &cache,
            root: &root,
            branch: "main",
            window: &win,
            limits: &limits,
        };
        let mut derived = no_derived();
        let outcome = enrich_within(&request, &mut derived, Duration::from_secs(30));
        assert!(!outcome.available);
        assert_eq!(outcome.note.as_deref(), Some(note.as_str()));
        assert!(lock(&unreadable.calls).iter().all(|c| c[0] != "pr"));

        let missing = FakeGh::new(|args| {
            if args[0] == "repo" {
                Err(GhError::NotInstalled)
            } else {
                Ok(String::new())
            }
        });
        assert_eq!(
            gh_status(&missing, &root, &budget()),
            Err("GitHub CLI (`gh`) is not installed.".to_string())
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    fn no_derived() -> Derived {
        Derived {
            drafts: Vec::new(),
            commit_pr: HashMap::new(),
            git_calls: 0,
        }
    }

    fn git_pr(number: u64) -> PrDraft {
        PrDraft {
            pr: RawPr {
                id: number.to_string(),
                number: Some(number),
                title: format!("Change {number}"),
                state: "merged".to_string(),
                source: "git".to_string(),
                author: GitIdentity {
                    name: "Ann".to_string(),
                    email: "ann@example.com".to_string(),
                    login: None,
                },
                committer: None,
                merged_at: Some("2026-02-01T10:00:00Z".to_string()),
                closed_at: None,
                merge_commit: Some(format!("{number:040x}")),
                commit_count: 1,
                branch_commits: Vec::new(),
                landing_co_authors: Vec::new(),
                files: Vec::new(),
                closes_bug: false,
            },
            hashes: HashSet::new(),
        }
    }

    fn enrich_with_git_prs(github: &FakeGh, git_prs: u64) -> (Outcome, Derived) {
        let root = repo_with_origin("git@github.com:acme/widgets.git");
        let cache = CommitCache::new();
        let limits = Limits::DEFAULT;
        let win = window();
        let request = Request {
            gh: github,
            commits: &cache,
            root: &root,
            branch: "main",
            window: &win,
            limits: &limits,
        };
        let mut derived = no_derived();
        derived.drafts = (1..=git_prs).map(git_pr).collect();
        let outcome = enrich_within(&request, &mut derived, Duration::from_secs(30));
        let _ = std::fs::remove_dir_all(&root);
        (outcome, derived)
    }

    #[test]
    fn another_host_is_called_unsupported_before_sign_in_is_asked() {
        for url in [
            "https://ghe.example.com/acme/widgets.git",
            "git@git.corp.example.com:acme/widgets.git",
            "ssh://git@gitlab.com/acme/widgets.git",
        ] {
            let root = repo_with_origin(url);
            let github = FakeGh::new(|_| Err(failed("not logged in")));
            assert_eq!(
                gh_status(&github, &root, &budget()),
                Err("Only github.com repositories are supported.".to_string()),
                "{url}"
            );
            assert!(lock(&github.calls).is_empty(), "{url}");
            let _ = std::fs::remove_dir_all(&root);
        }
        // ssh over 443 is still github.com.
        assert_eq!(
            parse_github_slug("ssh://git@ssh.github.com:443/acme/widgets.git").as_deref(),
            Some("acme/widgets")
        );
    }

    #[test]
    fn calls_after_an_alias_lookup_carry_the_resolved_repository() {
        let alias = repo_with_origin("git@github-work:acme/widgets.git");
        let cache = CommitCache::new();
        let limits = Limits::DEFAULT;
        let win = window();
        let github = FakeGh::new(|args| {
            Ok(if args[0] == "repo" {
                repo_view()
            } else if args[0] == "pr" {
                "[]".to_string()
            } else {
                String::new()
            })
        });
        let request = Request {
            gh: &github,
            commits: &cache,
            root: &alias,
            branch: "main",
            window: &win,
            limits: &limits,
        };
        let mut derived = Derived {
            drafts: Vec::new(),
            commit_pr: HashMap::new(),
            git_calls: 0,
        };
        let outcome = enrich_within(&request, &mut derived, Duration::from_secs(30));
        assert!(outcome.available);
        let calls = lock(&github.calls).clone();
        let pr_calls: Vec<&Vec<String>> = calls.iter().filter(|c| c[0] == "pr").collect();
        assert!(!pr_calls.is_empty());
        for call in pr_calls {
            assert_eq!(call[..4], ["pr", "list", "-R", "acme/widgets"], "{call:?}");
        }
        let _ = std::fs::remove_dir_all(&alias);
    }

    #[test]
    fn notes_keep_only_one_clean_line() {
        let raw = "  \nGraphQL: Bearer abc123 failed for ghp_abcdefghijklmnopqrstuvwxyz and github_pat_11ABCDEFG0123456789abcdefghij!\nsecond line with secrets";
        assert_eq!(sanitize_line(raw), "GraphQL: failed for and !");
        assert_eq!(sanitize_line("token=gho_abcdefghijklmnopqrstu."), "token=.");
        // Too short to be a token: kept.
        assert_eq!(sanitize_line("ghp_short"), "ghp_short");
        assert_eq!(sanitize_line("   \n\n"), "unknown error");
        let long = "x".repeat(500);
        assert_eq!(sanitize_line(&long).chars().count(), 200);
        assert_eq!(
            load_failure("boom"),
            "GitHub data could not be loaded: boom"
        );
    }

    #[test]
    fn a_failed_call_is_retried_once_but_a_rate_limit_is_not() {
        let run = |github: &FakeGh| {
            with_ctx(github, Duration::from_secs(60), |ctx| {
                ctx.run(&["pr", "list"])
            })
        };
        let attempts = AtomicUsize::new(0);
        let flaky = FakeGh::new(move |_| {
            if attempts.fetch_add(1, Ordering::SeqCst) == 0 {
                Err(failed("connection reset"))
            } else {
                Ok("[]".to_string())
            }
        });
        assert_eq!(run(&flaky), Ok("[]".to_string()));
        assert_eq!(lock(&flaky.calls).len(), 2);

        let down = FakeGh::new(|_| Err(failed("dial tcp: no route")));
        assert_eq!(
            run(&down),
            Err("GitHub data could not be loaded: dial tcp: no route".to_string())
        );
        assert_eq!(lock(&down.calls).len(), 2);

        let limited = FakeGh::new(|_| Err(failed("API rate limit exceeded for user")));
        assert!(run(&limited).is_err());
        assert_eq!(lock(&limited.calls).len(), 1);

        let slow = FakeGh::new(|_| Err(failed(TIMED_OUT)));
        assert!(run(&slow).is_err());
        assert_eq!(lock(&slow.calls).len(), 1);

        let garbled = FakeGh::new(|_| Ok("not json".to_string()));
        let err = with_ctx(&garbled, Duration::from_secs(60), |ctx| {
            ctx.pr_list::<GhPr>(&["--state", "merged"])
        })
        .err()
        .unwrap();
        assert_eq!(
            err,
            "GitHub data could not be loaded: unexpected output from gh"
        );
    }

    #[test]
    fn every_pr_call_names_the_repository() {
        let github = FakeGh::new(|_| Ok("[]".to_string()));
        fetch(&github, &CommitCache::new(), "main").ok().unwrap();
        let calls = lock(&github.calls).clone();
        assert!(!calls.is_empty());
        for call in calls {
            assert_eq!(call[..4], ["pr", "list", "-R", "acme/widgets"], "{call:?}");
        }
    }

    #[test]
    fn nothing_starts_once_the_budget_is_spent() {
        let derived = || Derived {
            drafts: Vec::new(),
            commit_pr: HashMap::new(),
            git_calls: 0,
        };
        let cache = CommitCache::new();
        let limits = Limits::DEFAULT;
        let win = window();
        let root = scratch_dir();

        let github = FakeGh::new(|_| Ok("[]".to_string()));
        let request = Request {
            gh: &github,
            commits: &cache,
            root: &root,
            branch: "main",
            window: &win,
            limits: &limits,
        };
        let outcome = enrich_within(&request, &mut derived(), Duration::ZERO);
        assert!(!outcome.available);
        assert_eq!(
            outcome.note.as_deref(),
            Some("GitHub data took too long to load. Try again later.")
        );
        assert!(lock(&github.calls).is_empty());

        // The budget runs out while the merged list is being read.
        let origin = repo_with_origin("git@github.com:acme/widgets.git");
        let list = serde_json::json!([pr_row(1, "2026-02-01T10:00:00Z", "main")]).to_string();
        let slow = FakeGh::new(move |args| {
            if args[0] == "pr" {
                std::thread::sleep(Duration::from_millis(150));
                Ok(list.clone())
            } else {
                Ok(String::new())
            }
        });
        let request = Request {
            gh: &slow,
            commits: &cache,
            root: &origin,
            branch: "main",
            window: &win,
            limits: &limits,
        };
        let mut untouched = derived();
        let outcome = enrich_within(&request, &mut untouched, Duration::from_millis(100));
        assert!(!outcome.available);
        assert_eq!(outcome.note.as_deref(), Some(TOO_SLOW));
        assert!(untouched.drafts.is_empty());
        let pr_calls = lock(&slow.calls).iter().filter(|c| c[0] == "pr").count();
        assert_eq!(pr_calls, 1);
        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&origin);
    }

    #[test]
    fn commit_lookups_run_in_parallel_but_never_beyond_the_worker_limit() {
        let rows: Vec<serde_json::Value> = (1..=200)
            .map(|n| {
                pr_row(
                    n,
                    &format!("2026-02-{:02}T{:02}:00:00Z", 1 + n / 24, n % 24),
                    "main",
                )
            })
            .collect();
        let list = serde_json::Value::Array(rows).to_string();
        let live = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let (live2, peak2) = (live.clone(), peak.clone());
        let github = FakeGh::new(move |args| {
            if args.contains(&COMMIT_FIELDS) && !args.contains(&"closed") {
                let now = live2.fetch_add(1, Ordering::SeqCst) + 1;
                peak2.fetch_max(now, Ordering::SeqCst);
                std::thread::sleep(Duration::from_millis(40));
                live2.fetch_sub(1, Ordering::SeqCst);
                Ok("[]".to_string())
            } else if args.contains(&"closed") {
                Ok("[]".to_string())
            } else {
                Ok(list.clone())
            }
        });
        fetch(&github, &CommitCache::new(), "main").ok().unwrap();
        let calls = lock(&github.calls)
            .iter()
            .filter(|c| c.contains(&COMMIT_FIELDS.to_string()) && c[5] == "merged")
            .count();
        assert_eq!(calls, 8);
        let peak = peak.load(Ordering::SeqCst);
        assert!((2..=WORKERS).contains(&peak), "peak {peak}");
    }

    #[test]
    fn a_failing_commit_lookup_fails_the_fetch_with_its_note() {
        let rows: Vec<serde_json::Value> = (1..=60)
            .map(|n| pr_row(n, &format!("2026-02-{:02}T10:00:00Z", n % 28 + 1), "main"))
            .collect();
        let list = serde_json::Value::Array(rows).to_string();
        let github = FakeGh::new(move |args| {
            if args.contains(&COMMIT_FIELDS) {
                Err(failed("HTTP 502"))
            } else {
                Ok(list.clone())
            }
        });
        let err = fetch(&github, &CommitCache::new(), "main").err().unwrap();
        assert_eq!(err, "GitHub data could not be loaded: HTTP 502");
    }

    #[test]
    fn commit_lists_are_reused_across_fetches() {
        let list = serde_json::json!([
            pr_row(1, "2026-02-01T10:00:00Z", "main"),
            pr_row(2, "2026-02-02T10:00:00Z", "main")
        ])
        .to_string();
        let commits = serde_json::json!([
            { "number": 1, "commits": [{ "messageHeadline": "one" }] },
            { "number": 2, "commits": [{ "messageHeadline": "a" }, { "messageHeadline": "b" }] }
        ])
        .to_string();
        let github = FakeGh::new(move |args| {
            if args.contains(&"closed") {
                Ok("[]".to_string())
            } else if args.contains(&COMMIT_FIELDS) {
                Ok(commits.clone())
            } else {
                Ok(list.clone())
            }
        });
        let cache = CommitCache::new();
        let commit_calls = || {
            lock(&github.calls)
                .iter()
                .filter(|c| c.contains(&COMMIT_FIELDS.to_string()) && c[5] == "merged")
                .count()
        };
        let first = fetch(&github, &cache, "main").ok().unwrap();
        assert_eq!(commit_calls(), 1);
        assert_eq!(first.merged_commits[&2].len(), 2);
        let again = fetch(&github, &cache, "main").ok().unwrap();
        assert_eq!(commit_calls(), 1);
        assert_eq!(again.merged_commits[&1].len(), 1);
        assert_eq!(again.merged_commits[&2].len(), 2);

        // A PR merged at another time is another entry.
        let moved = serde_json::json!([pr_row(1, "2026-02-03T10:00:00Z", "main")]).to_string();
        let changed = FakeGh::new(move |args| {
            if args.contains(&"closed") || args.contains(&COMMIT_FIELDS) {
                Ok("[]".to_string())
            } else {
                Ok(moved.clone())
            }
        });
        fetch(&changed, &cache, "main").ok().unwrap();
        assert!(lock(&changed.calls)
            .iter()
            .any(|c| c.contains(&COMMIT_FIELDS.to_string())));
    }

    #[test]
    fn cached_prs_split_the_ranges_so_only_uncached_ones_are_requested() {
        let rows: Vec<serde_json::Value> = (1..=7)
            .map(|n| pr_row(n, &format!("2026-02-01T{:02}:00:00Z", n), "main"))
            .collect();
        let list = serde_json::Value::Array(rows).to_string();
        let github = FakeGh::new(move |args| {
            if args.contains(&"closed") || args.contains(&COMMIT_FIELDS) {
                Ok("[]".to_string())
            } else {
                Ok(list.clone())
            }
        });
        let cache = CommitCache::new();
        for n in [3u64, 5] {
            cache.insert(
                (
                    "acme/widgets".to_string(),
                    n,
                    format!("2026-02-01T{n:02}:00:00Z"),
                ),
                Arc::new(Vec::new()),
            );
        }
        fetch(&github, &cache, "main").ok().unwrap();
        let mut searches: Vec<String> = lock(&github.calls)
            .iter()
            .filter(|c| c.contains(&COMMIT_FIELDS.to_string()) && c[5] == "merged")
            .map(|c| c[c.iter().position(|a| a == "--search").unwrap() + 1].clone())
            .collect();
        searches.sort();
        assert_eq!(
            searches,
            [
                "merged:2026-02-01T01:00:00Z..2026-02-01T02:00:00Z base:main",
                "merged:2026-02-01T04:00:00Z..2026-02-01T04:00:00Z base:main",
                "merged:2026-02-01T06:00:00Z..2026-02-01T07:00:00Z base:main",
            ]
        );
    }

    #[test]
    fn the_commit_cache_is_bounded() {
        let cache = CommitCache::with_cap(2);
        for number in 1..=3u64 {
            cache.insert(
                ("acme/widgets".to_string(), number, "t".to_string()),
                Arc::new(Vec::new()),
            );
        }
        assert_eq!(lock(&cache.entries).len(), 2);
        assert!(cache
            .get(&("acme/widgets".to_string(), 1, "t".to_string()))
            .is_none());
        assert!(cache
            .get(&("acme/widgets".to_string(), 3, "t".to_string()))
            .is_some());
    }

    // mapping

    #[test]
    fn the_author_email_comes_from_the_authors_own_commits() {
        let commits = vec![
            GhCommit {
                authors: vec![
                    GhCommitAuthor {
                        name: Some("Claude".into()),
                        email: Some("noreply@anthropic.com".into()),
                        login: Some("claude".into()),
                        ..GhCommitAuthor::default()
                    },
                    GhCommitAuthor {
                        name: Some("Octo".into()),
                        email: Some("".into()),
                        login: Some("Octo".into()),
                        ..GhCommitAuthor::default()
                    },
                ],
                ..GhCommit::default()
            },
            GhCommit {
                authors: vec![GhCommitAuthor {
                    name: Some("Octo".into()),
                    email: Some("octo@example.com".into()),
                    login: Some("octo".into()),
                    ..GhCommitAuthor::default()
                }],
                ..GhCommit::default()
            },
        ];
        let actor = Some(GhActor {
            login: Some("OCTO".into()),
            name: Some("".into()),
            ..GhActor::default()
        });
        let who = pr_author(&actor, &commits);
        assert_eq!(who.email, "octo@example.com");
        assert_eq!(who.login.as_deref(), Some("OCTO"));
        assert_eq!(who.name, "OCTO");

        let stranger = pr_author(&actor, &commits[..1]);
        assert_eq!(stranger.email, "");
    }

    #[test]
    fn a_deleted_user_becomes_ghost() {
        let pr = only_pr(serde_json::json!({
            "number": 5, "title": "Orphan", "author": null,
            "mergedAt": "2026-01-10T10:00:00Z", "mergeCommit": null,
            "baseRefName": "main", "closingIssuesReferences": null,
            "reviews": null, "files": null
        }));
        let raw = merged_pr(&pr, &[], None);
        assert_eq!(raw.author.login.as_deref(), Some("ghost"));
        assert_eq!(raw.author.name, "ghost");
        assert_eq!(raw.author.email, "");
        assert_eq!(raw.merge_commit, None);
        assert_eq!(raw.commit_count, 1);
        assert!(raw.branch_commits.is_empty());
        assert!(!raw.closes_bug);
        assert!(map_reviews(&pr, "5", 0, i64::MAX).is_empty());
    }

    #[test]
    fn commits_become_branch_commits_with_co_authors() {
        let pr = only_pr(serde_json::json!({
            "number": 6, "title": "Pair work", "author": { "login": "ann", "name": "Ann" },
            "mergedAt": "2026-01-10T10:00:00Z", "baseRefName": "main"
        }));
        let commits: Vec<GhCommit> = serde_json::from_value(serde_json::json!([
            {
                "messageHeadline": "first",
                "authoredDate": "2026-01-08T10:00:00Z",
                "committedDate": "2026-01-09T10:00:00Z",
                "authors": [
                    { "name": "Ann", "email": "ann@example.com", "login": "ann" },
                    { "name": "Ben", "email": "ben@example.com", "login": null }
                ]
            },
            { "messageHeadline": "second", "authoredDate": "2026-01-09T11:00:00Z", "authors": null }
        ]))
        .unwrap();
        let raw = merged_pr(&pr, &commits, None);
        assert_eq!(raw.commit_count, 2);
        assert_eq!(raw.author.email, "ann@example.com");
        let first = &raw.branch_commits[0];
        assert_eq!(first.authored_at, "2026-01-08T10:00:00Z");
        assert_eq!(first.committed_at, "2026-01-09T10:00:00Z");
        assert_eq!(first.authors.len(), 2);
        assert_eq!(first.authors[1].login, None);
        // No authors reported: the PR author stands in; a missing committed date reuses the authored one.
        let second = &raw.branch_commits[1];
        assert_eq!(second.committed_at, "2026-01-09T11:00:00Z");
        assert_eq!(second.authors[0].login.as_deref(), Some("ann"));
    }

    #[test]
    fn gh_files_are_the_fallback_without_a_local_commit() {
        let pr = only_pr(serde_json::json!({
            "number": 7, "title": "Remote only", "author": { "login": "ann" },
            "mergedAt": "2026-01-10T10:00:00Z", "baseRefName": "main",
            "files": [
                { "path": "a.rs", "additions": 5, "deletions": 2, "changeType": "MODIFIED" },
                { "path": "moved.rs", "additions": 0, "deletions": 0 }
            ]
        }));
        let raw = merged_pr(&pr, &[], None);
        assert_eq!(
            raw.files,
            [RawFile {
                path: "a.rs".into(),
                added: 5,
                deleted: 2
            }]
        );
        assert_eq!(raw.source, "gh");
        assert_eq!(raw.id, "7");
        assert_eq!(raw.number, Some(7));
    }

    #[test]
    fn only_a_bug_labelled_closing_issue_marks_a_fix() {
        let with = |labels: serde_json::Value| {
            only_pr(serde_json::json!({
                "number": 1, "closingIssuesReferences": [
                    { "number": 1, "labels": labels },
                    { "number": 2 }
                ]
            }))
        };
        assert!(closes_bug(&with(serde_json::json!([{ "name": "BUG" }]))));
        assert!(!closes_bug(&with(
            serde_json::json!([{ "name": "feature" }])
        )));
        assert!(!closes_bug(&with(serde_json::Value::Null)));
    }

    #[test]
    fn a_closed_pr_is_mapped_without_files() {
        let pr = only_pr(serde_json::json!({
            "number": 12, "title": "Nope", "author": { "login": "eve", "name": "Eve" },
            "closedAt": "2026-02-01T10:00:00Z", "baseRefName": "main"
        }));
        let raw = closed_pr(&pr, &[]);
        assert_eq!(raw.state, "closed");
        assert_eq!(raw.closed_at.as_deref(), Some("2026-02-01T10:00:00Z"));
        assert_eq!(raw.merged_at, None);
        assert_eq!(raw.merge_commit, None);
        assert_eq!(raw.commit_count, 1);
        assert!(raw.files.is_empty());
    }

    #[test]
    fn reviews_count_one_comment_for_a_long_body_and_skip_own_and_unsubmitted() {
        let pr = only_pr(serde_json::json!({
            "number": 3, "author": { "login": "Ann" },
            "reviews": [
                { "author": { "login": "bob" }, "state": "APPROVED",
                  "submittedAt": "2026-01-10T10:00:00Z", "body": "x".repeat(21) },
                { "author": { "login": "bob" }, "state": "COMMENTED",
                  "submittedAt": "2026-01-10T11:00:00Z", "body": format!("  {}  ", "y".repeat(20)) },
                { "author": { "login": "cat", "name": "Cat" }, "state": "CHANGES_REQUESTED",
                  "submittedAt": "2026-01-10T12:00:00Z", "body": null },
                { "author": { "login": "ann" }, "state": "COMMENTED",
                  "submittedAt": "2026-01-10T13:00:00Z", "body": "my own long comment on the thread" },
                { "author": { "login": "dee" }, "state": "PENDING", "submittedAt": null, "body": "later" },
                { "author": null, "state": "APPROVED", "submittedAt": "2026-01-10T14:00:00Z" },
                { "author": { "login": "eli" }, "state": "APPROVED",
                  "submittedAt": "2025-12-31T23:59:59Z", "body": "before the window starts" }
            ]
        }));
        let since = parse_iso_epoch(SINCE).unwrap();
        let until = parse_iso_epoch(UNTIL).unwrap();
        let reviews = map_reviews(&pr, "3", since, until);
        let got: Vec<(&str, &str, u64)> = reviews
            .iter()
            .map(|r| {
                (
                    r.reviewer.login.as_deref().unwrap(),
                    r.state.as_str(),
                    r.comments,
                )
            })
            .collect();
        assert_eq!(
            got,
            [
                ("bob", "APPROVED", 1),
                ("bob", "COMMENTED", 0),
                ("cat", "CHANGES_REQUESTED", 0)
            ]
        );
        assert!(reviews
            .iter()
            .all(|r| r.pr_id == "3" && r.reviewer.email.is_empty()));
        assert_eq!(reviews[2].reviewer.name, "Cat");
        assert_eq!(reviews[0].submitted_at, "2026-01-10T10:00:00Z");
    }

    // fetching

    fn pr_row(number: u64, merged_at: &str, base: &str) -> serde_json::Value {
        serde_json::json!({
            "number": number, "title": format!("PR {number}"), "author": { "login": "ann" },
            "mergedAt": merged_at, "baseRefName": base
        })
    }

    #[test]
    fn commits_are_fetched_in_chunks_over_narrow_ranges() {
        let rows: Vec<serde_json::Value> = (1..=60)
            .map(|n| {
                pr_row(
                    n,
                    &format!("2026-02-{:02}T{:02}:00:00Z", 1 + n / 24, n % 24),
                    "main",
                )
            })
            .collect();
        let list = serde_json::Value::Array(rows).to_string();
        let github = FakeGh::new(move |args| {
            let json = args[args.iter().position(|a| *a == "--json").unwrap() + 1];
            if json == COMMIT_FIELDS {
                Ok("[]".to_string())
            } else {
                Ok(list.clone())
            }
        });
        let fetched = fetch(&github, &CommitCache::new(), "main").ok().unwrap();
        assert_eq!(fetched.merged.len(), 60);
        let calls = github.calls.lock().unwrap().clone();
        let chunk_calls: Vec<&Vec<String>> = calls
            .iter()
            .filter(|c| c.contains(&COMMIT_FIELDS.to_string()))
            .collect();
        // 60 merged PRs in chunks of 25, plus no closed PRs (the fake serves the same list,
        // which is filtered by `closedAt`).
        assert_eq!(chunk_calls.iter().filter(|c| c[5] == "merged").count(), 3);
        // The calls run in parallel, so their order is not fixed.
        let mut searches: Vec<&String> = chunk_calls
            .iter()
            .filter(|c| c[5] == "merged")
            .map(|c| &c[c.iter().position(|a| a == "--search").unwrap() + 1])
            .collect();
        searches.sort();
        assert!(
            searches[0].starts_with("merged:2026-02-01T"),
            "{}",
            searches[0]
        );
        for search in searches {
            assert!(search.contains("..2026-02-"), "{search}");
            assert!(search.ends_with(" base:main"), "{search}");
        }
        let list_call = calls
            .iter()
            .find(|c| c.contains(&MERGED_FIELDS.to_string()))
            .unwrap();
        let search = &list_call[list_call.iter().position(|a| a == "--search").unwrap() + 1];
        assert_eq!(
            search,
            "merged:2026-01-01T00:00:00Z..2026-06-30T00:00:00Z base:main"
        );
        assert!(list_call.contains(&"1000".to_string()));
    }

    #[test]
    fn prs_into_other_branches_are_dropped_unless_the_head_is_detached() {
        let list = serde_json::json!([
            pr_row(1, "2026-02-01T10:00:00Z", "main"),
            pr_row(2, "2026-02-02T10:00:00Z", "release"),
            { "number": 3, "mergedAt": null, "baseRefName": "main" }
        ])
        .to_string();
        let github = FakeGh::new(move |args| {
            if args.contains(&"closed") || args.contains(&COMMIT_FIELDS) {
                Ok("[]".to_string())
            } else {
                Ok(list.clone())
            }
        });
        let on_main = fetch(&github, &CommitCache::new(), "main").ok().unwrap();
        assert_eq!(
            on_main.merged.iter().map(|p| p.number).collect::<Vec<_>>(),
            [1]
        );
        let detached = fetch(&github, &CommitCache::new(), "HEAD").ok().unwrap();
        assert_eq!(detached.merged.len(), 2);
        assert_eq!(base_clause("HEAD"), "");
        assert_eq!(base_clause("feature/x-1.2"), " base:feature/x-1.2");
        assert_eq!(base_clause("we ird"), "");
    }

    #[test]
    fn a_full_list_marks_the_data_truncated() {
        let rows: Vec<serde_json::Value> = (0..MERGED_LIMIT as u64)
            .map(|n| pr_row(n + 1, "2026-02-01T10:00:00Z", "main"))
            .collect();
        let list = serde_json::Value::Array(rows).to_string();
        let github = FakeGh::new(move |args| {
            if args.contains(&"closed") || args.contains(&COMMIT_FIELDS) {
                Ok("[]".to_string())
            } else {
                Ok(list.clone())
            }
        });
        let fetched = fetch(&github, &CommitCache::new(), "main").ok().unwrap();
        assert!(fetched.truncated);
    }

    #[test]
    fn a_closed_pr_failure_keeps_the_merged_data_and_says_so_without_truncating() {
        let github = FakeGh::new(|args| {
            if args.contains(&"closed") {
                Err(failed("HTTP 502"))
            } else {
                Ok("[]".to_string())
            }
        });
        let fetched = fetch(&github, &CommitCache::new(), "main").ok().unwrap();
        assert!(fetched.closed.is_empty());
        assert!(fetched.closed_failed);
        // `truncated` is the "history too large" banner; this is another thing.
        assert!(!fetched.truncated);

        let (outcome, _) = enrich_with_git_prs(&github, 0);
        assert!(outcome.available);
        assert!(!outcome.truncated);
        assert_eq!(
            outcome.note.as_deref(),
            Some("Closed pull requests could not be loaded from GitHub.")
        );
    }

    #[test]
    fn a_merged_list_failure_fails_the_fetch() {
        let github = FakeGh::new(|_| Err(failed("HTTP 502")));
        let err = fetch(&github, &CommitCache::new(), "main").err().unwrap();
        assert_eq!(err, "GitHub data could not be loaded: HTTP 502");
    }

    // bots and headlines

    #[test]
    fn bot_accounts_read_as_bot_logins_everywhere() {
        let pr = only_pr(serde_json::json!({
            "number": 4, "title": "Bump serde",
            "author": { "id": "x", "is_bot": true, "login": "app/dependabot" },
            "mergedAt": "2026-01-10T10:00:00Z", "baseRefName": "main",
            "reviews": [
                { "author": { "login": "coderabbitai" },
                  "state": "COMMENTED", "submittedAt": "2026-01-10T11:00:00Z",
                  "body": "Automated summary of the change set" },
                { "author": { "login": "copilot-pull-request-reviewer" },
                  "state": "COMMENTED", "submittedAt": "2026-01-10T11:10:00Z" },
                { "author": { "login": "release-helper-bot" },
                  "state": "COMMENTED", "submittedAt": "2026-01-10T11:20:00Z" },
                { "author": { "login": "dependabot[bot]" },
                  "state": "COMMENTED", "submittedAt": "2026-01-10T11:30:00Z",
                  "body": "Automated note from the author itself" },
                { "author": { "login": "app/looks-like-a-bot-but-is-not" },
                  "state": "APPROVED", "submittedAt": "2026-01-10T12:00:00Z" },
                { "author": { "login": "robotnik" },
                  "state": "APPROVED", "submittedAt": "2026-01-10T12:30:00Z" }
            ]
        }));
        let commits: Vec<GhCommit> = serde_json::from_value(serde_json::json!([{
            "messageHeadline": "Bump serde",
            "authors": [
                { "name": "dependabot[bot]", "login": "dependabot[bot]",
                  "email": "49699333+dependabot[bot]@users.noreply.github.com" },
                { "name": "Ann", "login": "ann", "email": "ann@example.com" }
            ]
        }]))
        .unwrap();
        let raw = merged_pr(&pr, &commits, None);
        assert_eq!(raw.author.login.as_deref(), Some("dependabot[bot]"));
        assert_eq!(raw.author.name, "dependabot[bot]");
        assert_eq!(
            raw.author.email,
            "49699333+dependabot[bot]@users.noreply.github.com"
        );
        let commit_authors = &raw.branch_commits[0].authors;
        assert_eq!(commit_authors[0].login.as_deref(), Some("dependabot[bot]"));
        assert_eq!(commit_authors[1].login.as_deref(), Some("ann"));

        let reviews = map_reviews(&pr, "4", 0, i64::MAX);
        let logins: Vec<&str> = reviews
            .iter()
            .map(|r| r.reviewer.login.as_deref().unwrap())
            .collect();
        // gh gives review authors as a bare login: known review bots and
        // `-bot` names read as bots, the PR author's own review is excluded and
        // everyone else keeps their login.
        assert_eq!(
            logins,
            [
                "coderabbitai[bot]",
                "copilot-pull-request-reviewer[bot]",
                "release-helper-bot[bot]",
                "app/looks-like-a-bot-but-is-not",
                "robotnik"
            ]
        );
        assert_eq!(reviews[0].reviewer.name, "coderabbitai[bot]");
        assert!(is_review_bot("CodeRabbitAI"));
        assert!(is_review_bot("github-actions"));
        assert!(is_review_bot("vercel"));
        assert!(!is_review_bot("vercelous"));

        assert_eq!(bot_login("dependabot[bot]"), "dependabot[bot]");
        assert_eq!(bot_login("app/renovate"), "renovate[bot]");
        assert_eq!(bare_login("App/Renovate[bot]"), "renovate");
    }

    #[test]
    fn headlines_survive_githubs_truncation() {
        let long = "Treat S3 3xx as an error, keep partial delete results, add list delimiter";
        assert!(long.chars().count() > 72);
        let truncated = format!("{}\u{2026}", &long[..68]);
        assert!(headlines_match(long, &truncated));
        assert!(headlines_match(long, &format!("{}...", &long[..68])));
        assert!(headlines_match(long, long));
        // `%s` joins a first paragraph that gh reports as one line only.
        assert!(headlines_match(
            "Fix the thing and then more words",
            "Fix the thing"
        ));
        assert!(headlines_match(
            "Fix the thing",
            "Fix the thing and then more words"
        ));
        assert!(!headlines_match(long, "Treat S3 4xx as an error"));
        assert!(!headlines_match(long, ""));
        assert!(!headlines_match("", "Fix"));
        assert!(!headlines_match("Add parser", "Add lexer\u{2026}"));
    }
}
