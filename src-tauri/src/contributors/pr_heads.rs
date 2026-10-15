//! Pull request heads from the local repository.
//!
//! GitHub keeps every PR head at `refs/pull/<N>/head` on `origin`. Listing
//! those refs and fetching the missing objects (without creating any ref)
//! lets the commits of a PR be read with `git log` instead of one `gh` call
//! per PR. Every network call here has a hard timeout, and every failure only
//! means "this PR is not available locally".

use super::*;

/// Most refspecs handed to one `git fetch` process, so the command line and
/// the negotiation stay small on repositories with thousands of PRs.
const FETCH_CHUNK: usize = 200;

/// Local object lookups never touch the network; this only stops a stuck
/// process from holding the caller forever.
const LOCAL_TIMEOUT: Duration = Duration::from_secs(30);

const TIMED_OUT: &str = "git timed out";

/// One commit of a PR branch, read from the local repository.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(not(test), allow(dead_code))]
pub(super) struct LocalCommit {
    pub hash: String,
    pub headline: String,
    pub author: GitIdentity,
    pub co_authors: Vec<GitIdentity>,
    pub authored_at: String,
    pub committed_at: String,
}

/// Runs git, feeding `input` on stdin when given. Both output pipes are
/// drained on their own threads so a large reply cannot stall the child; the
/// child is killed when `timeout` passes.
fn run_git(
    root: &Path,
    args: &[&str],
    input: Option<String>,
    timeout: Duration,
) -> Result<String, String> {
    let mut child = git_command(root)
        .args(args)
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("Could not run git: {e}"))?;
    if let (Some(input), Some(mut stdin)) = (input, child.stdin.take()) {
        // A thread, so a child that stops reading cannot block the deadline.
        std::thread::spawn(move || {
            let _ = stdin.write_all(input.as_bytes());
        });
    }
    let done = wait_with_deadline(&mut child, timeout).map_err(|failure| match failure {
        WaitFailure::TimedOut => TIMED_OUT.to_string(),
        WaitFailure::Io(e) => e,
    })?;
    if done.status.success() {
        return Ok(done.stdout);
    }
    let err = done.stderr.trim();
    Err(if err.is_empty() {
        "git failed".to_string()
    } else {
        err.to_string()
    })
}

/// `git <args>` with a hard timeout. A timeout kills the process and returns
/// `Err("git timed out")`.
#[cfg_attr(not(test), allow(dead_code))]
pub(super) fn run_with_timeout(
    root: &Path,
    args: &[&str],
    timeout: Duration,
) -> Result<String, String> {
    run_git(root, args, None, timeout)
}

/// Parses `<sha>\trefs/pull/<N>/head` lines; anything else is skipped.
fn parse_pr_heads(text: &str) -> HashMap<u64, String> {
    let mut heads = HashMap::new();
    for line in text.lines() {
        let Some((sha, name)) = line.trim().split_once('\t') else {
            continue;
        };
        let number = name
            .strip_prefix("refs/pull/")
            .and_then(|rest| rest.strip_suffix("/head"));
        let Some(number) =
            number.filter(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
        else {
            continue;
        };
        if !gh::is_object_id(sha) {
            continue;
        }
        if let Ok(number) = number.parse() {
            heads.insert(number, sha.to_string());
        }
    }
    heads
}

/// The head commit of every PR the `origin` remote knows, by PR number. One
/// network call.
#[cfg_attr(not(test), allow(dead_code))]
pub(super) fn remote_pr_heads(
    root: &Path,
    timeout: Duration,
) -> Result<HashMap<u64, String>, String> {
    let out = run_with_timeout(root, &["ls-remote", "origin", "refs/pull/*/head"], timeout)?;
    Ok(parse_pr_heads(&out))
}

/// The subset of `shas` that exist as commits in the local repository. One
/// `git cat-file --batch-check` process.
fn commits_present(root: &Path, shas: &[&str]) -> HashSet<String> {
    if shas.is_empty() {
        return HashSet::new();
    }
    let mut input = shas.join("\n");
    input.push('\n');
    let Ok(out) = run_git(
        root,
        &["cat-file", "--batch-check"],
        Some(input),
        LOCAL_TIMEOUT,
    ) else {
        return HashSet::new();
    };
    // `<sha> <type> <size>` for an object, `<sha> missing` otherwise.
    out.lines()
        .filter_map(|line| {
            let mut parts = line.split_whitespace();
            let sha = parts.next()?;
            (parts.next() == Some("commit")).then(|| sha.to_string())
        })
        .collect()
}

/// Makes the head commits of `heads` (PR number, sha) available locally and
/// returns the PR numbers whose head commit is there afterwards. Only the
/// missing commits are fetched, with no destination ref, so the repository
/// gains objects and nothing else. All fetches share the one `timeout`; a
/// failed or skipped fetch just leaves its PRs out of the result.
#[cfg_attr(not(test), allow(dead_code))]
pub(super) fn ensure_local(
    root: &Path,
    heads: &[(u64, String)],
    timeout: Duration,
) -> HashSet<u64> {
    let valid: Vec<&(u64, String)> = heads
        .iter()
        .filter(|(_, sha)| gh::is_object_id(sha))
        .collect();
    let mut shas: Vec<&str> = valid.iter().map(|(_, sha)| sha.as_str()).collect();
    shas.sort_unstable();
    shas.dedup();
    let mut present = commits_present(root, &shas);
    let missing: Vec<u64> = valid
        .iter()
        .filter(|(_, sha)| !present.contains(sha))
        .map(|(number, _)| *number)
        .collect();
    if !missing.is_empty() {
        let deadline = Instant::now() + timeout;
        for chunk in missing.chunks(FETCH_CHUNK) {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break;
            }
            let refs: Vec<String> = chunk
                .iter()
                .map(|n| format!("refs/pull/{n}/head"))
                .collect();
            let mut args = vec![
                "fetch",
                "--no-tags",
                "--no-write-fetch-head",
                "--no-recurse-submodules",
                "--quiet",
                // A configured pull refspec would otherwise update
                // remote-tracking refs opportunistically.
                "--refmap=",
                "origin",
            ];
            args.extend(refs.iter().map(String::as_str));
            // Not an error: the PRs of a failed chunk fail the re-check below.
            let _ = run_with_timeout(root, &args, left);
        }
        let absent: Vec<&str> = shas
            .iter()
            .copied()
            .filter(|sha| !present.contains(*sha))
            .collect();
        present.extend(commits_present(root, &absent));
    }
    valid
        .iter()
        .filter(|(_, sha)| present.contains(sha))
        .map(|(number, _)| *number)
        .collect()
}

/// The commits of the PR branch ending at `head`, oldest first, leaving out
/// everything `exclude` reaches. At most `max` are returned and the flag says
/// whether there were more (the newest `max` are kept). `None` when git
/// fails, for example because `head` is not available locally.
#[cfg_attr(not(test), allow(dead_code))]
pub(super) fn pr_commits(
    root: &Path,
    head: &str,
    exclude: &[String],
    max: usize,
) -> Option<(Vec<LocalCommit>, bool)> {
    if !gh::is_object_id(head) {
        return None;
    }
    let limit = max.saturating_add(1).to_string();
    let mut args: Vec<String> = [
        "log",
        "--no-merges",
        "--no-show-signature",
        "-n",
        limit.as_str(),
        LOG_FORMAT,
        head,
    ]
    .map(str::to_string)
    .to_vec();
    args.extend(exclude.iter().map(|sha| format!("^{sha}")));
    args.push("--".to_string());
    let out = git(root, &args).ok()?;
    let (parsed, _) = parse_log(&out, 0);
    let truncated = parsed.len() > max;
    // `git log` is newest first and `-n` cuts the oldest, so cut then flip.
    let commits = parsed
        .into_iter()
        .take(max)
        .rev()
        .map(|c| LocalCommit {
            hash: c.hash,
            headline: c.subject,
            author: c.author,
            co_authors: c.co_authors,
            authored_at: c.authored_at,
            committed_at: c.committed_at,
        })
        .collect();
    Some((commits, truncated))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHA_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const SHA_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    struct Scratch(PathBuf);

    impl Scratch {
        fn new() -> Self {
            let dir = std::env::temp_dir()
                .join(format!("monoptah-pr-heads-test-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }

        fn sub(&self, name: &str) -> PathBuf {
            self.0.join(name)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// `git` with an identity, run from `dir`.
    fn g(dir: &Path, args: &[&str]) -> String {
        let mut full = vec![
            "-c",
            "user.name=Alice",
            "-c",
            "user.email=alice@example.com",
            "-c",
            "commit.gpgsign=false",
        ];
        full.extend_from_slice(args);
        git(dir, full).unwrap().trim().to_string()
    }

    fn commit(dir: &Path, message: &str) -> String {
        g(dir, &["commit", "-q", "--allow-empty", "-m", message]);
        g(dir, &["rev-parse", "HEAD"])
    }

    /// A bare origin with `main`, plus a clone of it. Returns (origin, clone).
    fn origin_and_clone(scratch: &Scratch) -> (PathBuf, PathBuf) {
        let seed = scratch.sub("seed");
        std::fs::create_dir_all(&seed).unwrap();
        g(&seed, &["init", "-q", "-b", "main"]);
        commit(&seed, "base");
        let origin = scratch.sub("origin.git");
        let clone = scratch.sub("clone");
        g(
            scratch.0.as_path(),
            &["clone", "-q", "--bare", "seed", "origin.git"],
        );
        g(scratch.0.as_path(), &["clone", "-q", "origin.git", "clone"]);
        (origin, clone)
    }

    /// Adds `message` on top of the seed and publishes it as `refs/pull/<n>/head`
    /// of the origin without any branch, like GitHub does for a PR.
    fn publish_pr(scratch: &Scratch, origin: &Path, number: u64, message: &str) -> String {
        let seed = scratch.sub("seed");
        let sha = commit(&seed, message);
        g(
            &seed,
            &["push", "-q", origin.to_str().unwrap(), "HEAD:refs/tmp/pr"],
        );
        g(
            origin,
            &["update-ref", &format!("refs/pull/{number}/head"), &sha],
        );
        g(origin, &["update-ref", "-d", "refs/tmp/pr"]);
        sha
    }

    fn refs(dir: &Path) -> String {
        g(dir, &["for-each-ref"])
    }

    #[test]
    fn ls_remote_lines_become_pr_heads_and_junk_is_skipped() {
        let text = format!(
            "{SHA_A}\trefs/pull/1/head\n\
             {SHA_B}\trefs/pull/22/head\n\
             {SHA_A}\trefs/pull/3/merge\n\
             {SHA_A}\trefs/pull/x/head\n\
             {SHA_A}\trefs/pull//head\n\
             {SHA_A}\trefs/pull/+4/head\n\
             nothex\trefs/pull/5/head\n\
             {SHA_A}\trefs/heads/main\n\
             {SHA_A} refs/pull/6/head\n\
             \n\
             garbage\n"
        );
        let heads = parse_pr_heads(&text);
        assert_eq!(heads.len(), 2);
        assert_eq!(heads[&1], SHA_A);
        assert_eq!(heads[&22], SHA_B);
    }

    #[test]
    fn remote_pr_heads_lists_the_origin_pull_refs() {
        let scratch = Scratch::new();
        let (origin, clone) = origin_and_clone(&scratch);
        let one = publish_pr(&scratch, &origin, 1, "pr one");
        let seven = publish_pr(&scratch, &origin, 7, "pr seven");
        let heads = remote_pr_heads(&clone, Duration::from_secs(30)).unwrap();
        assert_eq!(heads.len(), 2);
        assert_eq!(heads[&1], one);
        assert_eq!(heads[&7], seven);
    }

    #[test]
    fn remote_pr_heads_reports_a_missing_origin() {
        let scratch = Scratch::new();
        let dir = scratch.sub("lonely");
        std::fs::create_dir_all(&dir).unwrap();
        g(&dir, &["init", "-q"]);
        assert!(remote_pr_heads(&dir, Duration::from_secs(30)).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn a_slow_git_is_killed_at_the_deadline() {
        let scratch = Scratch::new();
        let started = Instant::now();
        let result = run_with_timeout(
            scratch.0.as_path(),
            &["-c", "alias.slow=!sleep 10", "slow"],
            Duration::from_millis(200),
        );
        assert_eq!(result, Err(TIMED_OUT.to_string()));
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn a_failing_git_reports_its_stderr() {
        let scratch = Scratch::new();
        let err = run_with_timeout(
            scratch.0.as_path(),
            &["rev-parse", "--verify", "nope"],
            Duration::from_secs(30),
        )
        .unwrap_err();
        assert!(!err.is_empty());
    }

    #[test]
    fn ensure_local_fetches_missing_heads_without_creating_refs() {
        let scratch = Scratch::new();
        let (origin, clone) = origin_and_clone(&scratch);
        let remote_only = publish_pr(&scratch, &origin, 1, "remote only");
        // PR 2 is already in the clone, under no ref at all.
        let local = commit(&clone, "already here");
        g(&clone, &["reset", "-q", "--hard", "HEAD~1"]);
        g(&origin, &["fetch", "-q", clone.to_str().unwrap(), &local]);
        g(&origin, &["update-ref", "refs/pull/2/head", &local]);
        assert!(git(&clone, ["cat-file", "-e", &remote_only]).is_err());
        // A user-configured pull refspec must not turn the fetch into a ref update.
        g(
            &clone,
            &[
                "config",
                "--add",
                "remote.origin.fetch",
                "+refs/pull/*/head:refs/remotes/origin/pr/*",
            ],
        );
        let refs_before = refs(&clone);

        let heads = vec![(1, remote_only.clone()), (2, local.clone())];
        let ready = ensure_local(&clone, &heads, Duration::from_secs(30));

        assert_eq!(ready, HashSet::from([1, 2]));
        assert!(git(&clone, ["cat-file", "-e", &remote_only]).is_ok());
        assert_eq!(refs(&clone), refs_before);
        assert!(!clone.join(".git").join("FETCH_HEAD").exists());
    }

    #[test]
    fn ensure_local_leaves_out_heads_that_cannot_be_fetched() {
        let scratch = Scratch::new();
        let (origin, clone) = origin_and_clone(&scratch);
        let good = publish_pr(&scratch, &origin, 1, "reachable");
        // A head the origin does not have under that number.
        let heads = vec![(1, good.clone()), (9, SHA_A.to_string())];
        let ready = ensure_local(&clone, &heads, Duration::from_secs(30));
        // git aborts a whole fetch for one unknown ref, so PR 1 may be lost
        // with it; only the bad head must never be reported as ready.
        assert!(!ready.contains(&9));
        let alone = ensure_local(&clone, &[(1, good)], Duration::from_secs(30));
        assert_eq!(alone, HashSet::from([1]));
    }

    #[test]
    fn ensure_local_survives_a_missing_origin() {
        let scratch = Scratch::new();
        let dir = scratch.sub("lonely");
        std::fs::create_dir_all(&dir).unwrap();
        g(&dir, &["init", "-q"]);
        let heads = vec![(1, SHA_A.to_string()), (2, "not a sha".to_string())];
        assert!(ensure_local(&dir, &heads, Duration::from_secs(30)).is_empty());
        assert!(ensure_local(&dir, &[], Duration::from_secs(30)).is_empty());
    }

    #[test]
    fn ensure_local_stops_starting_fetches_once_the_budget_is_spent() {
        let scratch = Scratch::new();
        let (origin, clone) = origin_and_clone(&scratch);
        let sha = publish_pr(&scratch, &origin, 1, "late");
        let ready = ensure_local(&clone, &[(1, sha)], Duration::ZERO);
        assert!(ready.is_empty());
    }

    fn branch_repo() -> (Scratch, PathBuf, String, Vec<String>) {
        let scratch = Scratch::new();
        let dir = scratch.sub("repo");
        std::fs::create_dir_all(&dir).unwrap();
        g(&dir, &["init", "-q", "-b", "main"]);
        let base = commit(&dir, "base");
        g(&dir, &["checkout", "-q", "-b", "feature"]);
        let mut shas = Vec::new();
        for (i, message) in ["first", "second", "third"].iter().enumerate() {
            let body = if i == 1 {
                format!("{message}\n\nbody\n\nCo-authored-by: Carol Dev <carol@example.com>\nCo-authored-by: Bot <12+octocat@users.noreply.github.com>")
            } else {
                message.to_string()
            };
            shas.push(commit(&dir, &body));
        }
        (scratch, dir, base, shas)
    }

    #[test]
    fn pr_commits_are_oldest_first_and_skip_what_is_excluded() {
        let (_scratch, dir, base, shas) = branch_repo();
        let (commits, cut) = pr_commits(&dir, &shas[2], &[base], 10).unwrap();
        assert!(!cut);
        let headlines: Vec<&str> = commits.iter().map(|c| c.headline.as_str()).collect();
        assert_eq!(headlines, ["first", "second", "third"]);
        assert_eq!(commits[0].hash, shas[0]);
        assert_eq!(commits[0].author.email, "alice@example.com");
        assert!(commits[0].authored_at.starts_with("20"));
        assert!(commits[0].committed_at.starts_with("20"));
        assert!(commits[0].co_authors.is_empty());

        let (rest, _) = pr_commits(&dir, &shas[2], &[shas[1].clone()], 10).unwrap();
        assert_eq!(rest.len(), 1);
        assert_eq!(rest[0].headline, "third");
    }

    #[test]
    fn pr_commits_parse_co_authors() {
        let (_scratch, dir, base, shas) = branch_repo();
        let (commits, _) = pr_commits(&dir, &shas[2], &[base], 10).unwrap();
        let second = &commits[1];
        assert_eq!(second.headline, "second");
        let emails: Vec<&str> = second.co_authors.iter().map(|c| c.email.as_str()).collect();
        assert_eq!(
            emails,
            ["carol@example.com", "12+octocat@users.noreply.github.com"]
        );
        assert_eq!(second.co_authors[1].login.as_deref(), Some("octocat"));
    }

    #[test]
    fn pr_commits_honour_max_and_report_the_cut() {
        let (_scratch, dir, base, shas) = branch_repo();
        let (commits, cut) = pr_commits(&dir, &shas[2], std::slice::from_ref(&base), 2).unwrap();
        assert!(cut);
        let headlines: Vec<&str> = commits.iter().map(|c| c.headline.as_str()).collect();
        assert_eq!(headlines, ["second", "third"]);

        let (exact, cut) = pr_commits(&dir, &shas[2], &[base], 3).unwrap();
        assert!(!cut);
        assert_eq!(exact.len(), 3);
    }

    #[test]
    fn pr_commits_skip_merges_and_fail_softly() {
        let (_scratch, dir, base, _) = branch_repo();
        g(&dir, &["checkout", "-q", "main"]);
        let side = commit(&dir, "side");
        g(&dir, &["merge", "-q", "--no-ff", "-m", "merge", "feature"]);
        let merged = g(&dir, &["rev-parse", "HEAD"]);
        let (commits, _) = pr_commits(&dir, &merged, &[base], 10).unwrap();
        let headlines: Vec<&str> = commits.iter().map(|c| c.headline.as_str()).collect();
        assert_eq!(headlines, ["first", "second", "third", "side"]);
        assert_eq!(commits[3].hash, side);

        assert!(pr_commits(&dir, SHA_A, &[], 10).is_none());
        assert!(pr_commits(&dir, "--all", &[], 10).is_none());
    }
}
