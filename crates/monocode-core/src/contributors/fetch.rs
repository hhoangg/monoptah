//! Bringing one branch of `origin` up to date.
//!
//! Reading the Contributors page is network-free: it uses
//! `refs/remotes/origin/<branch>` as it is. This module is the only place that
//! moves that ref, and it is only called when the user asks (Refresh, Sync).
//! It fetches exactly one remote-tracking ref and never touches the working
//! tree, `HEAD` or `refs/heads`. The one other ref it writes is
//! `refs/remotes/origin/HEAD`, pointed at the default branch origin reports,
//! so passive reads follow a renamed or newly learned default.

use std::sync::atomic::AtomicBool;

use super::*;

/// Hard limit of each network call. A fetch without a pick makes two (the
/// default-branch lookup, then the fetch), so the ceiling is twice this.
const FETCH_TIMEOUT: Duration = Duration::from_secs(60);

/// What `fetch_origin` found out.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FetchOutcome {
    /// The tracking ref is up to date; `moved` says whether this fetch changed it.
    Fetched {
        moved: bool,
    },
    /// Nothing to ask origin: no origin remote, "HEAD", or origin has no such branch.
    Skipped,
    /// Another fetch of this repository is running.
    Busy,
    Cancelled,
    /// Origin no longer has the branch, which it had before (deleted or
    /// renamed, and for the default branch the new one could not be learned).
    /// The stale tracking ref is still what is read.
    Gone,
    /// Could not reach origin or timed out. The text is for logs and tests only,
    /// never the UI: git's stderr can contain URLs with credentials.
    Failed(String),
}

/// The fetches that are running, by repository (`store::git_common_dir`, so
/// linked worktrees share an entry). The flag cancels that fetch.
#[derive(Default)]
pub struct FetchRegistry {
    running: Mutex<HashMap<String, Arc<AtomicBool>>>,
}

/// Holds a registry entry and removes it when dropped, panic included.
struct Claim<'a> {
    registry: &'a FetchRegistry,
    key: String,
    cancel: Arc<AtomicBool>,
}

impl Drop for Claim<'_> {
    fn drop(&mut self) {
        lock(&self.registry.running).remove(&self.key);
    }
}

impl FetchRegistry {
    fn claim(&self, key: &str) -> Option<Claim<'_>> {
        let mut running = lock(&self.running);
        if running.contains_key(key) {
            return None;
        }
        let cancel = Arc::new(AtomicBool::new(false));
        running.insert(key.to_string(), cancel.clone());
        Some(Claim {
            registry: self,
            key: key.to_string(),
            cancel,
        })
    }

    /// Asks the fetch of the repository to stop; false when none runs.
    pub(super) fn cancel(&self, key: &str) -> bool {
        match lock(&self.running).get(key) {
            Some(flag) => {
                flag.store(true, Ordering::SeqCst);
                true
            }
            None => false,
        }
    }

    #[cfg(test)]
    pub(super) fn is_empty(&self) -> bool {
        lock(&self.running).is_empty()
    }

    #[cfg(test)]
    pub(super) fn register(&self, key: &str) -> Option<Arc<AtomicBool>> {
        self.claim(key).map(|claim| {
            let cancel = claim.cancel.clone();
            std::mem::forget(claim);
            cancel
        })
    }
}

/// Fetches `branch` (the default branch when `None`) from `origin` into its
/// remote-tracking ref. `Err` only when `cwd` is not a repository.
pub(super) fn fetch_origin(
    registry: &FetchRegistry,
    cwd: &Path,
    branch: Option<&str>,
) -> Result<FetchOutcome, String> {
    let root = repo_root(cwd)?;
    // A name git would take for an option is not a branch to ask origin for.
    let Ok(mut name) = target_branch(&root, branch) else {
        return Ok(FetchOutcome::Skipped);
    };
    if name == "HEAD" || git(&root, ["remote", "get-url", "origin"]).is_err() {
        return Ok(FetchOutcome::Skipped);
    }
    let key = store::git_common_dir(&root)?;
    let Some(claim) = registry.claim(&key) else {
        return Ok(FetchOutcome::Busy);
    };
    let cancelled = || claim.cancel.load(Ordering::SeqCst);
    let run = |args: &[&str]| {
        pr_heads::run_git(
            &root,
            args,
            None,
            FETCH_TIMEOUT,
            &[("LC_ALL", "C")],
            Some(&cancelled),
        )
    };
    let failed = |e: String| {
        if e == pr_heads::CANCELLED {
            FetchOutcome::Cancelled
        } else {
            FetchOutcome::Failed(e)
        }
    };

    let before = resolve_branch(&root, branch).ok();
    // Without a pick the default branch is whatever origin says it is now, so
    // a renamed default is followed instead of a stale tracking ref.
    let mut learned = false;
    if branch.map(str::trim).is_none_or(str::is_empty) {
        match run(&["ls-remote", "--symref", "origin", "HEAD"]) {
            Ok(out) => {
                if let Some(remote) = remote_default(&out) {
                    name = remote;
                    learned = true;
                }
            }
            Err(e) => return Ok(failed(e)),
        }
    }
    // The name goes into a refspec: `*` or `a:b` must not reach git there.
    let refname = format!("refs/heads/{name}");
    if git(&root, ["check-ref-format", refname.as_str()]).is_err() {
        return Ok(FetchOutcome::Skipped);
    }
    let tracked = pr_heads::remote_branch_tip(&root, &name);
    // `+` so a force-pushed branch is accepted; an explicit refspec works for
    // single-branch clones and custom `remote.origin.fetch`, and can only ever
    // write this one remote-tracking ref. No --depth/--unshallow: shallow and
    // partial clones keep their shape.
    let refspec = format!("+refs/heads/{name}:refs/remotes/origin/{name}");
    let result = run(&[
        "fetch",
        "--no-tags",
        "--no-write-fetch-head",
        "--no-recurse-submodules",
        // Do not start `git maintenance --auto` in the user's repository.
        "--no-auto-maintenance",
        "--quiet",
        // A configured refspec would otherwise update other tracking refs.
        "--refmap=",
        "origin",
        refspec.as_str(),
    ]);
    match result {
        Ok(_) => {
            if learned {
                point_origin_head(&root, &name);
            }
            let after = resolve_branch(&root, branch).ok();
            Ok(FetchOutcome::Fetched {
                moved: after != before,
            })
        }
        // A tracking ref that existed means origin dropped or renamed the
        // branch; one that never did is a local-only branch or a tag, no outage.
        Err(e) if e.contains("couldn't find remote ref") => Ok(if tracked.is_some() {
            FetchOutcome::Gone
        } else {
            FetchOutcome::Skipped
        }),
        Err(e) => Ok(failed(e)),
    }
}

/// The branch `ls-remote --symref origin HEAD` reports as origin's default.
fn remote_default(out: &str) -> Option<String> {
    out.lines()
        .find_map(|line| line.strip_prefix("ref: refs/heads/"))
        .and_then(|rest| rest.split_once('\t'))
        .map(|(name, _)| name.to_string())
        .filter(|name| !name.is_empty())
}

/// Makes `refs/remotes/origin/HEAD` name `name`, so reads that never ask
/// origin follow its current default. Local only; a failure changes nothing.
fn point_origin_head(root: &Path, name: &str) {
    const HEAD: &str = "refs/remotes/origin/HEAD";
    let target = format!("refs/remotes/origin/{name}");
    let current = git(root, ["symbolic-ref", "-q", HEAD]).ok();
    if current.as_deref().map(str::trim) != Some(target.as_str()) {
        let _ = git(root, ["symbolic-ref", HEAD, target.as_str()]);
    }
}

#[cfg(test)]
mod tests {
    use super::super::db::ContributorsDb;
    use super::super::jobs::{self, estimate_response, load_response, prepare_job, run_job};
    use super::super::sync::{Reporter, SyncStep};
    use super::super::tests::{TestRepo, ALICE, BOB};
    use super::*;

    const WHEN: &str = "2026-01-01T00:00:00+0000";
    const SINCE: &str = "2026-01-01T00:00:00Z";
    const UNTIL: &str = "2026-01-31T23:59:59Z";

    fn plain_git(args: &[&str]) {
        let output = Command::new("git")
            .args(args)
            .env(
                "GIT_CONFIG_GLOBAL",
                std::env::temp_dir().join("monoptah-contributors-no-gitconfig"),
            )
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_TERMINAL_PROMPT", "0")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn fresh_path(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!("monoptah-fetch-{label}-{}", uuid::Uuid::new_v4()))
    }

    /// Removes the directory when dropped.
    struct Dir(PathBuf);

    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    struct NoReport;

    impl Reporter for NoReport {
        fn progress(&self, _: SyncStep, _: u64, _: u64) {}
        fn cancelled(&self) -> bool {
            false
        }
    }

    /// A bare `origin`, a checkout that pushes to it (the other people), and a
    /// clone of it (the user's repository).
    struct World {
        bare: Dir,
        pusher: TestRepo,
        clone: TestRepo,
        registry: FetchRegistry,
    }

    impl World {
        fn new() -> Self {
            let bare = Dir(fresh_path("origin"));
            plain_git(&[
                "init",
                "-q",
                "--bare",
                "-b",
                "main",
                bare.0.to_str().unwrap(),
            ]);
            let pusher = TestRepo::new();
            pusher.run(
                &["remote", "add", "origin", bare.0.to_str().unwrap()],
                ALICE,
                WHEN,
            );
            let world_pusher = |name: &str, date: &str| {
                pusher.commit(&[(name, "x\n")], name, ALICE, date);
            };
            world_pusher("one", "2026-01-05T10:00:00+0000");
            world_pusher("two", "2026-01-06T10:00:00+0000");
            pusher.run(&["push", "-q", "origin", "main"], ALICE, WHEN);
            let clone = Self::clone_of(&bare.0, &[]);
            Self {
                bare,
                pusher,
                clone,
                registry: FetchRegistry::default(),
            }
        }

        fn clone_of(bare: &Path, flags: &[&str]) -> TestRepo {
            let dir = fresh_path("clone");
            let mut args = vec!["clone", "-q"];
            args.extend(flags);
            args.push(bare.to_str().unwrap());
            args.push(dir.to_str().unwrap());
            plain_git(&args);
            TestRepo::adopt(dir)
        }

        /// A commit by someone else on the origin's `main`; returns its sha.
        fn land(&self, name: &str, date: &str) -> String {
            let sha = self.pusher.commit(&[(name, "x\n")], name, BOB, date);
            self.pusher
                .run(&["push", "-q", "origin", "main"], BOB, WHEN);
            sha
        }

        fn fetch(&self, repo: &TestRepo, branch: Option<&str>) -> FetchOutcome {
            fetch_origin(&self.registry, repo.path(), branch).unwrap()
        }
    }

    fn rev(repo: &TestRepo, name: &str) -> Option<String> {
        git(repo.path(), ["rev-parse", "--verify", "-q", name])
            .ok()
            .map(|s| s.trim().to_string())
    }

    fn head_of(repo: &TestRepo, branch: Option<&str>) -> String {
        resolve_branch(repo.path(), branch).unwrap().1
    }

    fn name_of(repo: &TestRepo, branch: Option<&str>) -> String {
        resolve_branch(repo.path(), branch).unwrap().0
    }

    fn status(repo: &TestRepo) -> String {
        git(repo.path(), ["status", "--porcelain"]).unwrap()
    }

    fn now() -> i64 {
        now_epoch()
    }

    fn sync(store: &ContributorsDb, repo: &TestRepo, mode: &str, old_head: Option<&str>) {
        let spec = prepare_job(repo.path(), None, SINCE, UNTIL, mode, old_head).unwrap();
        let outcome = run_job(
            store,
            &spec,
            &gh::Unavailable,
            &gh::CommitCache::new(),
            &NoReport,
        )
        .unwrap();
        assert!(!outcome.cancelled);
    }

    fn load(store: &ContributorsDb, repo: &TestRepo) -> jobs::LoadResponse {
        load_response(store, repo.path(), None, SINCE, UNTIL, now()).unwrap()
    }

    fn subjects(loaded: &jobs::LoadResponse) -> Vec<String> {
        let mut all: Vec<String> = loaded
            .raw
            .commits
            .iter()
            .map(|c| c.subject.clone())
            .collect();
        all.sort();
        all
    }

    // ---- reading ----

    #[test]
    fn reads_origin_when_the_clone_is_behind_and_fetch_leaves_the_checkout_alone() {
        let w = World::new();
        let old = head_of(&w.clone, None);
        let new = w.land("three", "2026-01-10T10:00:00+0000");
        // Reading is network-free: the new commit is not known yet.
        assert_eq!(head_of(&w.clone, None), old);

        let checkout = (
            rev(&w.clone, "HEAD"),
            rev(&w.clone, "refs/heads/main"),
            status(&w.clone),
        );
        assert_eq!(
            w.fetch(&w.clone, None),
            FetchOutcome::Fetched { moved: true }
        );
        assert_eq!(head_of(&w.clone, None), new);
        assert_eq!(head_of(&w.clone, Some("main")), new);
        assert_eq!(
            (
                rev(&w.clone, "HEAD"),
                rev(&w.clone, "refs/heads/main"),
                status(&w.clone)
            ),
            checkout,
            "the checkout, the local branch and the working tree stay as they were"
        );
        assert_eq!(rev(&w.clone, "refs/heads/main"), Some(old));
        assert_eq!(
            w.fetch(&w.clone, None),
            FetchOutcome::Fetched { moved: false }
        );
        assert!(w.registry.is_empty());
    }

    #[test]
    fn fetch_then_a_top_up_brings_the_new_commit_in() {
        let w = World::new();
        let store = ContributorsDb::open_in_memory().unwrap();
        sync(&store, &w.clone, "full", None);
        let synced = load(&store, &w.clone);
        assert!(synced.covered);
        let old = synced.head.clone();

        w.land("three", "2026-01-10T10:00:00+0000");
        w.fetch(&w.clone, None);
        let moved = load(&store, &w.clone);
        assert_ne!(moved.head, old);
        assert_eq!(moved.coverage_head.as_deref(), Some(old.as_str()));
        assert_eq!(moved.raw.commits.len(), 2);

        sync(&store, &w.clone, "topup", Some(&old));
        let done = load(&store, &w.clone);
        assert!(done.covered);
        assert_eq!(done.coverage_head.as_deref(), Some(done.head.as_str()));
        assert_eq!(subjects(&done), ["one", "three", "two"]);
        assert_eq!(
            w.fetch(&w.clone, None),
            FetchOutcome::Fetched { moved: false }
        );
    }

    #[test]
    fn an_old_commit_landing_later_is_repaired() {
        let w = World::new();
        let store = ContributorsDb::open_in_memory().unwrap();
        sync(&store, &w.clone, "full", None);

        // Dated before the sync, as a merge of old work brings in.
        w.land("dated earlier", "2026-01-02T10:00:00+0000");
        w.fetch(&w.clone, None);
        let moved = load(&store, &w.clone);
        assert!(!moved.covered);
        assert!(
            !moved.has_gaps,
            "the coverage is intact: a repair, not a gap"
        );
        assert_eq!(moved.missing_commits, 1);

        sync(&store, &w.clone, "full", None);
        let done = load(&store, &w.clone);
        assert!(done.covered);
        assert_eq!(done.missing_commits, 0);
        // A repair adds the missing commits but leaves the row at the head it
        // was synced at; the page's top-up (a cheap no-op here) moves it.
        sync(
            &store,
            &w.clone,
            "topup",
            Some(done.coverage_head.as_deref().unwrap()),
        );
        let done = load(&store, &w.clone);
        assert_eq!(done.coverage_head.as_deref(), Some(done.head.as_str()));
        assert_eq!(subjects(&done), ["dated earlier", "one", "two"]);
    }

    #[test]
    fn a_force_pushed_origin_converges_and_dropped_commits_leave_the_numbers() {
        let w = World::new();
        let store = ContributorsDb::open_in_memory().unwrap();
        sync(&store, &w.clone, "full", None);
        let old = load(&store, &w.clone).head;
        assert_eq!(subjects(&load(&store, &w.clone)), ["one", "two"]);

        // Origin drops "two" and puts "rewrite" in its place.
        w.pusher
            .run(&["reset", "-q", "--hard", "HEAD~1"], ALICE, WHEN);
        w.pusher.commit(
            &[("rewrite", "x\n")],
            "rewrite",
            ALICE,
            "2026-01-07T10:00:00+0000",
        );
        w.pusher
            .run(&["push", "-q", "--force", "origin", "main"], ALICE, WHEN);
        assert_eq!(
            w.fetch(&w.clone, None),
            FetchOutcome::Fetched { moved: true }
        );

        sync(&store, &w.clone, "topup", Some(&old));
        let done = load(&store, &w.clone);
        assert!(done.covered);
        assert_eq!(subjects(&done), ["one", "rewrite"]);
    }

    #[test]
    fn a_local_branch_ahead_of_origin_reads_origin_and_converges() {
        let w = World::new();
        let store = ContributorsDb::open_in_memory().unwrap();
        let origin_tip = head_of(&w.clone, None);
        let local = w.clone.commit(
            &[("local", "x\n")],
            "local only",
            ALICE,
            "2026-01-08T10:00:00+0000",
        );
        // Coverage recorded at the local head, as before the origin was read.
        w.clone.run(
            &["update-ref", "-d", "refs/remotes/origin/main"],
            ALICE,
            WHEN,
        );
        assert_eq!(head_of(&w.clone, None), local);
        sync(&store, &w.clone, "full", None);
        assert_eq!(
            subjects(&load(&store, &w.clone)),
            ["local only", "one", "two"]
        );

        w.fetch(&w.clone, None);
        assert_eq!(
            head_of(&w.clone, None),
            origin_tip,
            "unpushed work is not counted"
        );
        assert_eq!(rev(&w.clone, "refs/heads/main"), Some(local.clone()));
        sync(&store, &w.clone, "topup", Some(&local));
        let done = load(&store, &w.clone);
        assert!(done.covered);
        assert_eq!(subjects(&done), ["one", "two"]);
    }

    #[test]
    fn without_an_origin_the_fetch_is_skipped_and_the_local_branch_is_read() {
        let repo = TestRepo::new();
        let tip = repo.commit(&[("a", "x\n")], "one", ALICE, "2026-01-05T10:00:00+0000");
        let registry = FetchRegistry::default();
        assert_eq!(
            fetch_origin(&registry, repo.path(), None).unwrap(),
            FetchOutcome::Skipped
        );
        assert_eq!(head_of(&repo, None), tip);
        assert_eq!(origin_default_branch(repo.path()), None);
    }

    #[test]
    fn a_fetch_outside_a_repository_is_an_error() {
        let dir = Dir(fresh_path("plain"));
        std::fs::create_dir_all(&dir.0).unwrap();
        let err = fetch_origin(&FetchRegistry::default(), &dir.0, None).unwrap_err();
        assert_eq!(err, "Not a git repository");
    }

    #[test]
    fn an_unreachable_origin_fails_and_changes_nothing() {
        let w = World::new();
        let tip = head_of(&w.clone, None);
        w.clone.run(
            &[
                "remote",
                "set-url",
                "origin",
                fresh_path("missing").to_str().unwrap(),
            ],
            ALICE,
            WHEN,
        );
        let refs = git(w.clone.path(), ["for-each-ref"]).unwrap();
        assert!(matches!(w.fetch(&w.clone, None), FetchOutcome::Failed(_)));
        assert_eq!(git(w.clone.path(), ["for-each-ref"]).unwrap(), refs);
        assert!(w.registry.is_empty());

        let store = ContributorsDb::open_in_memory().unwrap();
        sync(&store, &w.clone, "full", None);
        assert_eq!(load(&store, &w.clone).head, tip);
    }

    #[test]
    fn a_remote_that_cannot_be_reached_before_any_fetch_leaves_the_local_read() {
        let repo = TestRepo::new();
        let tip = repo.commit(&[("a", "x\n")], "one", ALICE, "2026-01-05T10:00:00+0000");
        repo.run(
            &[
                "remote",
                "add",
                "origin",
                fresh_path("missing").to_str().unwrap(),
            ],
            ALICE,
            WHEN,
        );
        let registry = FetchRegistry::default();
        assert!(matches!(
            fetch_origin(&registry, repo.path(), None).unwrap(),
            FetchOutcome::Failed(_)
        ));
        assert_eq!(head_of(&repo, None), tip);
    }

    #[test]
    fn a_local_only_branch_is_skipped_and_read_locally() {
        let w = World::new();
        w.clone
            .run(&["checkout", "-q", "-b", "feature"], ALICE, WHEN);
        let tip = w.clone.commit(
            &[("f", "x\n")],
            "feature work",
            ALICE,
            "2026-01-08T10:00:00+0000",
        );
        assert_eq!(w.fetch(&w.clone, Some("feature")), FetchOutcome::Skipped);
        assert_eq!(head_of(&w.clone, Some("feature")), tip);
        // A tag is no outage either.
        w.clone.run(&["tag", "v1"], ALICE, WHEN);
        assert_eq!(w.fetch(&w.clone, Some("v1")), FetchOutcome::Skipped);
        assert_eq!(head_of(&w.clone, Some("v1")), tip);
        assert_eq!(w.fetch(&w.clone, Some("-x")), FetchOutcome::Skipped);
    }

    #[test]
    fn head_is_never_fetched_or_looked_up_on_the_remote() {
        let w = World::new();
        w.land("three", "2026-01-10T10:00:00+0000");
        assert_eq!(w.fetch(&w.clone, Some("HEAD")), FetchOutcome::Skipped);

        // Detached with an origin default: the default is read.
        w.clone.run(&["checkout", "-q", "--detach"], ALICE, WHEN);
        assert_eq!(name_of(&w.clone, None), "main");
        // An explicit HEAD is the checkout, not refs/remotes/origin/HEAD.
        let detached = rev(&w.clone, "HEAD").unwrap();
        assert_eq!(
            resolve_branch(w.clone.path(), Some("HEAD")).unwrap(),
            ("HEAD".into(), detached)
        );

        // Detached without an origin: HEAD, and nothing to fetch.
        let lone = TestRepo::new();
        let tip = lone.commit(&[("a", "x\n")], "one", ALICE, "2026-01-05T10:00:00+0000");
        lone.run(&["checkout", "-q", "--detach"], ALICE, WHEN);
        assert_eq!(
            resolve_branch(lone.path(), None).unwrap(),
            ("HEAD".into(), tip)
        );
        assert_eq!(
            fetch_origin(&w.registry, lone.path(), None).unwrap(),
            FetchOutcome::Skipped
        );
    }

    #[test]
    fn the_default_branch_falls_back_from_origin_head_to_main_to_master_to_the_checkout() {
        let w = World::new();
        let c = &w.clone;
        let tip = head_of(c, None);
        c.run(&["checkout", "-q", "-b", "feature"], ALICE, WHEN);
        // origin/HEAD names it.
        assert_eq!(origin_default_branch(c.path()).as_deref(), Some("main"));
        assert_eq!(name_of(c, None), "main");
        // An explicit pick wins.
        assert_eq!(name_of(c, Some("feature")), "feature");

        // A stale origin/HEAD falls back to main.
        c.run(
            &[
                "symbolic-ref",
                "refs/remotes/origin/HEAD",
                "refs/remotes/origin/gone",
            ],
            ALICE,
            WHEN,
        );
        assert_eq!(origin_default_branch(c.path()).as_deref(), Some("main"));
        // No origin/HEAD at all: main still.
        c.run(&["remote", "set-head", "origin", "-d"], ALICE, WHEN);
        assert_eq!(origin_default_branch(c.path()).as_deref(), Some("main"));
        // origin/HEAD naming another existing branch.
        c.run(
            &["update-ref", "refs/remotes/origin/trunk", &tip],
            ALICE,
            WHEN,
        );
        c.run(
            &[
                "symbolic-ref",
                "refs/remotes/origin/HEAD",
                "refs/remotes/origin/trunk",
            ],
            ALICE,
            WHEN,
        );
        assert_eq!(origin_default_branch(c.path()).as_deref(), Some("trunk"));
        c.run(&["remote", "set-head", "origin", "-d"], ALICE, WHEN);
        // master when there is no main.
        c.run(
            &["update-ref", "refs/remotes/origin/master", &tip],
            ALICE,
            WHEN,
        );
        c.run(
            &["update-ref", "-d", "refs/remotes/origin/main"],
            ALICE,
            WHEN,
        );
        assert_eq!(origin_default_branch(c.path()).as_deref(), Some("master"));
        // Neither: the checkout.
        c.run(
            &["update-ref", "-d", "refs/remotes/origin/master"],
            ALICE,
            WHEN,
        );
        assert_eq!(origin_default_branch(c.path()), None);
        assert_eq!(name_of(c, None), "feature");
    }

    #[test]
    fn the_branch_list_starts_with_what_a_request_without_a_pick_reads() {
        let w = World::new();
        let c = &w.clone;
        c.run(&["checkout", "-q", "-b", "feature"], ALICE, WHEN);
        let listed = list_branches(c.path()).unwrap();
        assert_eq!(listed.current, "main");
        assert_eq!(listed.branches, ["main", "feature"]);

        // The origin default has no local branch: it is listed anyway.
        c.run(&["branch", "-q", "-D", "main"], ALICE, WHEN);
        let listed = list_branches(c.path()).unwrap();
        assert_eq!(listed.current, "main");
        assert_eq!(listed.branches, ["main", "feature"]);

        // Detached without an origin default keeps today's behaviour.
        let lone = TestRepo::new();
        lone.commit(&[("a", "x\n")], "one", ALICE, "2026-01-05T10:00:00+0000");
        lone.run(&["checkout", "-q", "--detach"], ALICE, WHEN);
        let listed = list_branches(lone.path()).unwrap();
        assert_eq!(listed.current, "HEAD");
        assert_eq!(listed.branches, ["main"]);
    }

    // ---- clone shapes ----

    #[test]
    fn a_linked_worktree_updates_the_shared_tracking_ref() {
        let w = World::new();
        let tree = Dir(fresh_path("worktree"));
        w.clone.run(
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "side",
                tree.0.to_str().unwrap(),
            ],
            ALICE,
            WHEN,
        );
        let new = w.land("three", "2026-01-10T10:00:00+0000");
        let outcome = fetch_origin(&w.registry, &tree.0, None).unwrap();
        assert_eq!(outcome, FetchOutcome::Fetched { moved: true });
        assert_eq!(head_of(&w.clone, None), new);
        assert!(w.registry.is_empty());
    }

    #[test]
    fn a_shallow_clone_stays_shallow() {
        let w = World::new();
        let url = format!("file://{}", w.bare.0.display());
        let shallow = TestRepo::adopt({
            let dir = fresh_path("shallow");
            plain_git(&["clone", "-q", "--depth", "1", &url, dir.to_str().unwrap()]);
            dir
        });
        let shallow_file = shallow.path().join(".git/shallow");
        assert!(shallow_file.exists());
        let new = w.land("three", "2026-01-10T10:00:00+0000");
        assert_eq!(
            w.fetch(&shallow, None),
            FetchOutcome::Fetched { moved: true }
        );
        assert!(shallow_file.exists(), "no --unshallow");
        assert_eq!(head_of(&shallow, None), new);
    }

    #[test]
    fn a_single_branch_clone_with_a_custom_refspec_fetches_the_asked_branch_only() {
        let w = World::new();
        w.pusher.run(&["checkout", "-q", "-b", "dev"], ALICE, WHEN);
        let dev = w.pusher.commit(
            &[("d", "x\n")],
            "dev work",
            ALICE,
            "2026-01-09T10:00:00+0000",
        );
        w.pusher.run(&["push", "-q", "origin", "dev"], ALICE, WHEN);
        let single = World::clone_of(&w.bare.0, &["--single-branch", "-b", "main"]);
        single.run(
            &[
                "config",
                "remote.origin.fetch",
                "+refs/heads/zzz:refs/remotes/upstream/zzz",
            ],
            ALICE,
            WHEN,
        );
        assert_eq!(rev(&single, "refs/remotes/origin/dev"), None);
        assert_eq!(
            w.fetch(&single, Some("dev")),
            FetchOutcome::Fetched { moved: true }
        );
        assert_eq!(rev(&single, "refs/remotes/origin/dev"), Some(dev));
        let refs = git(single.path(), ["for-each-ref", "--format=%(refname)"]).unwrap();
        assert!(!refs.contains("upstream"), "{refs}");
        assert!(!single.path().join(".git/FETCH_HEAD").exists());
    }

    #[test]
    fn a_partial_clone_fetches_without_losing_its_filter() {
        let w = World::new();
        plain_git(&[
            "-C",
            w.bare.0.to_str().unwrap(),
            "config",
            "uploadpack.allowFilter",
            "true",
        ]);
        let url = format!("file://{}", w.bare.0.display());
        let partial = TestRepo::adopt({
            let dir = fresh_path("partial");
            plain_git(&[
                "clone",
                "-q",
                "--filter=blob:none",
                &url,
                dir.to_str().unwrap(),
            ]);
            dir
        });
        let new = w.land("three", "2026-01-10T10:00:00+0000");
        assert_eq!(
            w.fetch(&partial, None),
            FetchOutcome::Fetched { moved: true }
        );
        assert_eq!(head_of(&partial, None), new);
        let filter = git(
            partial.path(),
            ["config", "remote.origin.partialclonefilter"],
        )
        .unwrap();
        assert_eq!(filter.trim(), "blob:none");
    }

    // ---- origin changes its mind ----

    fn bare_git(w: &World, args: &[&str]) {
        let mut all = vec!["-C", w.bare.0.to_str().unwrap()];
        all.extend(args);
        plain_git(&all);
    }

    #[test]
    fn a_renamed_default_branch_is_learned_from_origin() {
        let w = World::new();
        w.pusher
            .run(&["checkout", "-q", "-b", "trunk"], ALICE, WHEN);
        let new = w.pusher.commit(
            &[("t", "x\n")],
            "on trunk",
            ALICE,
            "2026-01-10T10:00:00+0000",
        );
        w.pusher
            .run(&["push", "-q", "origin", "trunk"], ALICE, WHEN);
        bare_git(&w, &["symbolic-ref", "HEAD", "refs/heads/trunk"]);
        bare_git(&w, &["branch", "-q", "-D", "main"]);

        assert_eq!(name_of(&w.clone, None), "main");
        assert_eq!(
            w.fetch(&w.clone, None),
            FetchOutcome::Fetched { moved: true }
        );
        assert_eq!(name_of(&w.clone, None), "trunk");
        assert_eq!(head_of(&w.clone, None), new);
        assert_eq!(
            origin_default_branch(w.clone.path()).as_deref(),
            Some("trunk")
        );
        assert_eq!(
            w.fetch(&w.clone, None),
            FetchOutcome::Fetched { moved: false }
        );
    }

    #[test]
    fn a_repository_without_origin_head_learns_the_default() {
        let w = World::new();
        w.pusher
            .run(&["checkout", "-q", "-b", "develop"], ALICE, WHEN);
        let tip = w
            .pusher
            .commit(&[("d", "x\n")], "dev", ALICE, "2026-01-10T10:00:00+0000");
        w.pusher
            .run(&["push", "-q", "origin", "develop"], ALICE, WHEN);
        bare_git(&w, &["symbolic-ref", "HEAD", "refs/heads/develop"]);
        bare_git(&w, &["branch", "-q", "-D", "main"]);
        let added = TestRepo::new();
        added.commit(&[("a", "x\n")], "local", ALICE, "2026-01-05T10:00:00+0000");
        added.run(
            &["remote", "add", "origin", w.bare.0.to_str().unwrap()],
            ALICE,
            WHEN,
        );
        assert_eq!(origin_default_branch(added.path()), None);
        assert_eq!(w.fetch(&added, None), FetchOutcome::Fetched { moved: true });
        assert_eq!(
            origin_default_branch(added.path()).as_deref(),
            Some("develop")
        );
        assert_eq!(head_of(&added, None), tip);
    }

    #[test]
    fn a_branch_origin_dropped_is_gone_and_the_stale_ref_is_still_read() {
        let w = World::new();
        w.pusher
            .run(&["checkout", "-q", "-b", "feature"], ALICE, WHEN);
        let tip = w.pusher.commit(
            &[("f", "x\n")],
            "feature",
            ALICE,
            "2026-01-10T10:00:00+0000",
        );
        w.pusher
            .run(&["push", "-q", "origin", "feature"], ALICE, WHEN);
        assert_eq!(
            w.fetch(&w.clone, Some("feature")),
            FetchOutcome::Fetched { moved: true }
        );
        bare_git(&w, &["branch", "-q", "-D", "feature"]);
        assert_eq!(w.fetch(&w.clone, Some("feature")), FetchOutcome::Gone);
        assert_eq!(head_of(&w.clone, Some("feature")), tip);
        // One that never existed there is only skipped.
        assert_eq!(w.fetch(&w.clone, Some("never")), FetchOutcome::Skipped);
    }

    #[test]
    fn names_that_are_not_branch_names_never_reach_the_refspec() {
        let w = World::new();
        let refs = git(w.clone.path(), ["for-each-ref"]).unwrap();
        for name in ["*", "a:b", "a..b", "x^y"] {
            assert_eq!(
                w.fetch(&w.clone, Some(name)),
                FetchOutcome::Skipped,
                "{name}"
            );
        }
        assert_eq!(git(w.clone.path(), ["for-each-ref"]).unwrap(), refs);
    }

    #[test]
    fn a_branch_wins_over_a_tag_of_the_same_name_without_origin() {
        let repo = TestRepo::new();
        let tag = repo.commit(&[("a", "x\n")], "one", ALICE, "2026-01-05T10:00:00+0000");
        repo.run(&["tag", "foo"], ALICE, WHEN);
        repo.run(&["checkout", "-q", "-b", "foo"], ALICE, WHEN);
        let tip = repo.commit(&[("b", "x\n")], "two", ALICE, "2026-01-06T10:00:00+0000");
        assert_eq!(head_of(&repo, Some("foo")), tip);
        // A tag with no branch still resolves, to the tag.
        repo.run(&["tag", "only-tag", &tag], ALICE, WHEN);
        assert_eq!(head_of(&repo, Some("only-tag")), tag);
    }

    #[test]
    fn a_tag_named_like_the_checked_out_branch_lists_it_once() {
        let repo = TestRepo::new();
        repo.commit(&[("a", "x\n")], "one", ALICE, "2026-01-05T10:00:00+0000");
        repo.run(&["tag", "main"], ALICE, WHEN);
        let listed = list_branches(repo.path()).unwrap();
        assert_eq!(listed.current, "main");
        assert_eq!(listed.branches, ["main"]);
    }

    #[test]
    fn a_tag_named_like_a_branch_does_not_rename_it_in_the_list() {
        let w = World::new();
        w.clone.run(&["branch", "dev"], ALICE, WHEN);
        w.clone.run(&["tag", "dev"], ALICE, WHEN);
        let listed = list_branches(w.clone.path()).unwrap();
        assert_eq!(listed.branches, ["main", "dev"]);
    }

    // ---- concurrency and cancel ----

    #[test]
    fn a_second_fetch_of_the_repository_is_busy_and_the_entry_goes_away() {
        let w = World::new();
        let key = store::git_common_dir(w.clone.path()).unwrap();
        let held = w.registry.claim(&key).unwrap();
        // A worktree shares the key, so it is busy too.
        assert_eq!(w.fetch(&w.clone, None), FetchOutcome::Busy);
        assert!(w.registry.claim(&key).is_none());
        drop(held);
        assert!(w.registry.is_empty());
        assert_eq!(
            w.fetch(&w.clone, None),
            FetchOutcome::Fetched { moved: false }
        );
        assert!(w.registry.is_empty());
    }

    #[test]
    fn the_entry_is_removed_when_the_fetch_panics() {
        let registry = FetchRegistry::default();
        let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _claim = registry.claim("repo").unwrap();
            panic!("boom");
        }));
        assert!(caught.is_err());
        assert!(registry.is_empty());
    }

    #[test]
    fn cancel_sets_the_flag_of_that_repository_only() {
        let registry = FetchRegistry::default();
        assert!(!registry.cancel("repo"));
        let flag = registry.register("repo").unwrap();
        assert!(!registry.cancel("other"));
        assert!(!flag.load(Ordering::SeqCst));
        assert!(registry.cancel("repo"));
        assert!(flag.load(Ordering::SeqCst));
    }

    /// Makes git run `command` instead of `git-upload-pack` for the remote, so
    /// a test can see when the network would be used, or hold a fetch open.
    #[cfg(unix)]
    fn set_uploadpack(repo: &TestRepo, command: &str) {
        repo.run(
            &["config", "remote.origin.uploadpack", command],
            ALICE,
            WHEN,
        );
    }

    #[cfg(unix)]
    #[test]
    fn cancel_stops_a_fetch_that_is_waiting_on_the_remote() {
        let w = std::sync::Arc::new(World::new());
        set_uploadpack(&w.clone, "sleep 5; :");
        let key = store::git_common_dir(w.clone.path()).unwrap();
        let started = Instant::now();
        let worker = {
            let w = w.clone();
            std::thread::spawn(move || w.fetch(&w.clone, None))
        };
        while w.registry.is_empty() {
            std::thread::sleep(Duration::from_millis(10));
        }
        std::thread::sleep(Duration::from_millis(300));
        assert!(w.registry.cancel(&key));
        assert_eq!(worker.join().unwrap(), FetchOutcome::Cancelled);
        assert!(
            started.elapsed() < Duration::from_secs(4),
            "{:?}",
            started.elapsed()
        );
        assert!(w.registry.is_empty());
    }

    // ---- reading never touches the network ----

    /// Proves no branch fetch or ls-remote happens (the probe is the remote's
    /// upload-pack); it passes `gh::Unavailable`, so PR-head fetching, which
    /// needs GitHub data, is not exercised here.
    #[cfg(unix)]
    #[test]
    fn loading_estimating_and_syncing_never_fetch_a_branch() {
        let w = World::new();
        let marker = fresh_path("marker");
        let probe = format!("touch {}; false", marker.display());
        set_uploadpack(&w.clone, &probe);
        w.land("three", "2026-01-10T10:00:00+0000");

        let store = ContributorsDb::open_in_memory().unwrap();
        let loaded = load(&store, &w.clone);
        assert_eq!(loaded.raw.commits.len(), 0);
        let estimate = estimate_response(
            &store,
            &gh::Unavailable,
            w.clone.path(),
            None,
            SINCE,
            UNTIL,
            now(),
        )
        .unwrap();
        assert_eq!(estimate.commits, 2);
        sync(&store, &w.clone, "full", None);
        sync(&store, &w.clone, "topup", Some(&loaded.head));
        assert!(load(&store, &w.clone).covered);
        assert!(list_branches(w.clone.path()).is_ok());
        assert!(first_commit(w.clone.path(), None).unwrap().is_some());
        assert!(!marker.exists(), "something asked the remote");

        // The probe does fire for a fetch, so the assertion above means something.
        assert!(matches!(w.fetch(&w.clone, None), FetchOutcome::Failed(_)));
        assert!(marker.exists());
        let _ = std::fs::remove_file(&marker);
    }
}
