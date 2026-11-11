#[cfg(unix)]
use super::move_out::fetch_with_timeout_for_test;
use super::move_out::snapshot_with_limit_for_test;
use super::move_out::*;
use std::{
    path::{Path, PathBuf},
    process::Command,
};

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_ALLOW_PROTOCOL", "file")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}

fn configure(dir: &Path) {
    git(dir, &["config", "user.email", "t@example.com"]);
    git(dir, &["config", "user.name", "Test"]);
    git(dir, &["config", "commit.gpgsign", "false"]);
    git(dir, &["config", "core.autocrlf", "false"]);
}

fn commit_file(dir: &Path, name: &str, text: &str) {
    std::fs::write(dir.join(name), text).unwrap();
    git(dir, &["add", name]);
    git(dir, &["commit", "-q", "-m", name]);
}

struct Fixture {
    _tmp: tempfile::TempDir,
    origin: PathBuf,
    work: PathBuf,
}

impl Fixture {
    /// A bare `origin` with `main`, and a clone `work` on `main` tracking it.
    fn new() -> Self {
        Self::in_dir(tempfile::tempdir().unwrap())
    }

    fn in_dir(tmp: tempfile::TempDir) -> Self {
        let root = tmp.path().canonicalize().unwrap();
        let origin = root.join("origin.git");
        let work = root.join("work");
        std::fs::create_dir_all(&origin).unwrap();
        git(&origin, &["init", "-q", "--bare", "-b", "main"]);
        std::fs::create_dir_all(&work).unwrap();
        git(&work, &["init", "-q", "-b", "main"]);
        configure(&work);
        git(
            &work,
            &["remote", "add", "origin", origin.to_str().unwrap()],
        );
        commit_file(&work, "README.md", "hello\n");
        git(&work, &["push", "-q", "-u", "origin", "main"]);
        git(&work, &["remote", "set-head", "origin", "main"]);
        Self {
            _tmp: tmp,
            origin,
            work,
        }
    }

    fn target(&self) -> PushTarget {
        fetch_origin(&self.work).unwrap();
        push_target(&self.work).unwrap()
    }

    /// Another clone that pushes `file` to `branch` on origin.
    fn push_from_elsewhere(&self, branch: &str, file: &str) {
        let other = self._tmp.path().canonicalize().unwrap().join("other");
        if !other.exists() {
            git(
                self._tmp.path(),
                &["clone", "-q", self.origin.to_str().unwrap(), "other"],
            );
            configure(&other);
        }
        git(&other, &["fetch", "-q", "origin"]);
        let exists = Command::new("git")
            .arg("-C")
            .arg(&other)
            .args(["rev-parse", "--verify", "-q", &format!("origin/{branch}")])
            .status()
            .unwrap()
            .success();
        if exists {
            git(
                &other,
                &["checkout", "-q", "-B", branch, &format!("origin/{branch}")],
            );
        } else {
            git(&other, &["checkout", "-q", "-B", branch, "origin/main"]);
        }
        commit_file(&other, file, "other\n");
        git(
            &other,
            &["push", "-q", "origin", &format!("{branch}:{branch}")],
        );
    }
}

fn pt(branch: &str, needs_push: bool, creates_branch: bool) -> PushTarget {
    PushTarget {
        branch: branch.into(),
        needs_push,
        creates_branch,
    }
}

// ------------------------------------------------------------ branch rules

#[test]
fn a_new_branch_with_a_free_name_keeps_its_name() {
    let f = Fixture::new();
    git(&f.work, &["checkout", "-q", "-b", "feature"]);
    commit_file(&f.work, "a.txt", "a\n");
    assert_eq!(f.target(), pt("feature", true, true));
}

#[test]
fn a_branch_ahead_of_its_upstream_pushes_to_the_upstream() {
    let f = Fixture::new();
    git(&f.work, &["checkout", "-q", "-b", "feature"]);
    commit_file(&f.work, "a.txt", "a\n");
    git(&f.work, &["push", "-q", "-u", "origin", "feature"]);
    commit_file(&f.work, "b.txt", "b\n");
    assert_eq!(f.target(), pt("feature", true, false));
}

#[test]
fn an_upstream_with_another_name_is_used() {
    let f = Fixture::new();
    git(&f.work, &["checkout", "-q", "-b", "local-name"]);
    commit_file(&f.work, "a.txt", "a\n");
    git(&f.work, &["push", "-q", "origin", "local-name:remote-name"]);
    git(
        &f.work,
        &["branch", "-u", "origin/remote-name", "local-name"],
    );
    commit_file(&f.work, "b.txt", "b\n");
    assert_eq!(f.target(), pt("remote-name", true, false));
}

#[test]
fn nothing_to_push_when_head_is_on_origin() {
    let f = Fixture::new();
    git(&f.work, &["checkout", "-q", "-b", "feature"]);
    commit_file(&f.work, "a.txt", "a\n");
    git(&f.work, &["push", "-q", "-u", "origin", "feature"]);
    assert_eq!(f.target(), pt("feature", false, false));
}

#[test]
fn a_same_name_branch_on_origin_at_head_needs_no_push_without_an_upstream() {
    let f = Fixture::new();
    git(&f.work, &["checkout", "-q", "-b", "feature"]);
    commit_file(&f.work, "a.txt", "a\n");
    git(&f.work, &["push", "-q", "origin", "feature"]);
    assert_eq!(f.target(), pt("feature", false, false));
}

#[test]
fn a_same_name_branch_behind_head_is_fast_forwarded() {
    let f = Fixture::new();
    git(&f.work, &["checkout", "-q", "-b", "feature"]);
    commit_file(&f.work, "a.txt", "a\n");
    git(&f.work, &["push", "-q", "origin", "feature"]);
    commit_file(&f.work, "b.txt", "b\n");
    assert_eq!(f.target(), pt("feature", true, false));
}

#[test]
fn a_diverged_branch_goes_to_the_cloud_name() {
    let f = Fixture::new();
    git(&f.work, &["checkout", "-q", "-b", "feature"]);
    commit_file(&f.work, "a.txt", "a\n");
    git(&f.work, &["push", "-q", "-u", "origin", "feature"]);
    f.push_from_elsewhere("feature", "theirs.txt");
    commit_file(&f.work, "mine.txt", "mine\n");
    assert_eq!(f.target(), pt("feature-cloud", true, true));
}

#[test]
fn a_taken_cloud_name_gets_a_number() {
    let f = Fixture::new();
    git(&f.work, &["checkout", "-q", "-b", "feature"]);
    commit_file(&f.work, "a.txt", "a\n");
    git(&f.work, &["push", "-q", "-u", "origin", "feature"]);
    f.push_from_elsewhere("feature", "theirs.txt");
    f.push_from_elsewhere("feature-cloud", "someone.txt");
    commit_file(&f.work, "mine.txt", "mine\n");
    assert_eq!(f.target(), pt("feature-cloud-2", true, true));
}

#[test]
fn an_earlier_move_is_reused_when_head_is_ahead_of_it() {
    let f = Fixture::new();
    git(&f.work, &["checkout", "-q", "-b", "feature"]);
    commit_file(&f.work, "a.txt", "a\n");
    git(&f.work, &["push", "-q", "-u", "origin", "feature"]);
    f.push_from_elsewhere("feature", "theirs.txt");
    commit_file(&f.work, "mine.txt", "mine\n");
    git(
        &f.work,
        &["push", "-q", "origin", "HEAD:refs/heads/feature-cloud"],
    );
    assert_eq!(f.target(), pt("feature-cloud", false, false));
    commit_file(&f.work, "more.txt", "more\n");
    assert_eq!(f.target(), pt("feature-cloud", true, false));
}

#[test]
fn a_branch_behind_its_upstream_cannot_fast_forward_so_it_uses_the_cloud_name() {
    let f = Fixture::new();
    git(&f.work, &["checkout", "-q", "-b", "feature"]);
    commit_file(&f.work, "a.txt", "a\n");
    git(&f.work, &["push", "-q", "-u", "origin", "feature"]);
    f.push_from_elsewhere("feature", "theirs.txt");
    assert_eq!(f.target(), pt("feature-cloud", true, true));
}

#[test]
fn the_default_branch_is_never_pushed_to() {
    let f = Fixture::new();
    commit_file(&f.work, "a.txt", "a\n");
    assert_eq!(f.target(), pt("main-cloud", true, true));
}

#[test]
fn the_default_branch_at_its_tip_needs_no_push() {
    let f = Fixture::new();
    assert_eq!(f.target(), pt("main", false, false));
}

#[test]
fn a_diverged_default_branch_uses_the_cloud_name() {
    let f = Fixture::new();
    f.push_from_elsewhere("main", "theirs.txt");
    commit_file(&f.work, "mine.txt", "mine\n");
    assert_eq!(f.target(), pt("main-cloud", true, true));
}

#[test]
fn a_branch_tracking_the_default_branch_keeps_its_own_name() {
    let f = Fixture::new();
    git(
        &f.work,
        &["checkout", "-q", "-b", "feature", "--track", "origin/main"],
    );
    commit_file(&f.work, "a.txt", "a\n");
    assert_eq!(f.target(), pt("feature", true, true));
    // Once that name exists on origin and has diverged, it moves to the cloud name.
    f.push_from_elsewhere("feature", "theirs.txt");
    assert_eq!(f.target(), pt("feature-cloud", true, true));
}

#[test]
fn the_default_branch_is_found_without_origin_head() {
    let f = Fixture::new();
    git(&f.work, &["remote", "set-head", "origin", "-d"]);
    commit_file(&f.work, "a.txt", "a\n");
    assert_eq!(f.target(), pt("main-cloud", true, true));
}

#[test]
fn the_default_branch_follows_origin_head() {
    let f = Fixture::new();
    git(&f.work, &["checkout", "-q", "-b", "trunk"]);
    git(&f.work, &["push", "-q", "-u", "origin", "trunk"]);
    git(&f.work, &["remote", "set-head", "origin", "trunk"]);
    commit_file(&f.work, "a.txt", "a\n");
    assert_eq!(f.target(), pt("trunk-cloud", true, true));
    git(&f.work, &["checkout", "-q", "main"]);
    commit_file(&f.work, "b.txt", "b\n");
    // main is still protected: it also holds the conventional default name.
    assert_eq!(f.target(), pt("main-cloud", true, true));
}

/// Origin's default is `develop` and `main` is gone from origin.
fn develop_is_the_default(f: &Fixture) {
    git(&f.work, &["checkout", "-q", "-b", "develop"]);
    git(&f.work, &["push", "-q", "-u", "origin", "develop"]);
    git(&f.origin, &["symbolic-ref", "HEAD", "refs/heads/develop"]);
    git(&f.work, &["push", "-q", "origin", "--delete", "main"]);
    git(&f.work, &["fetch", "-q", "--prune", "origin"]);
}

#[test]
fn an_unusual_default_branch_is_found_without_origin_head() {
    let f = Fixture::new();
    develop_is_the_default(&f);
    git(&f.work, &["remote", "set-head", "origin", "-d"]);
    commit_file(&f.work, "a.txt", "a\n");
    assert_eq!(f.target(), pt("develop-cloud", true, true));
}

/// origin cannot be asked (`ls-remote` fails), so its default cannot be confirmed.
fn origin_unreachable(f: &Fixture) {
    git(
        &f.work,
        &["remote", "set-url", "origin", "/nonexistent/path.git"],
    );
}

#[test]
fn an_unconfirmed_default_branch_is_never_pushed_to() {
    let f = Fixture::new();
    develop_is_the_default(&f);
    git(&f.work, &["remote", "set-head", "origin", "-d"]);
    commit_file(&f.work, "a.txt", "a\n");
    origin_unreachable(&f);
    // develop is ahead of origin/develop, which would be a plain fast-forward push to the
    // default branch if the name were trusted.
    assert_eq!(
        push_target(&f.work).unwrap(),
        pt("develop-cloud", true, true)
    );
}

#[test]
fn an_unconfirmed_default_makes_every_branch_use_the_cloud_name_unless_at_tip() {
    let f = Fixture::new();
    git(&f.work, &["checkout", "-q", "-b", "feature"]);
    commit_file(&f.work, "a.txt", "a\n");
    git(&f.work, &["push", "-q", "-u", "origin", "feature"]);
    commit_file(&f.work, "b.txt", "b\n");
    assert_eq!(f.target(), pt("feature", true, false));
    origin_unreachable(&f);
    assert_eq!(
        push_target(&f.work).unwrap(),
        pt("feature-cloud", true, true)
    );
    git(&f.work, &["reset", "-q", "--hard", "origin/feature"]);
    assert_eq!(push_target(&f.work).unwrap(), pt("feature", false, false));
}

#[test]
fn a_stale_origin_head_does_not_hide_the_default_branch() {
    let f = Fixture::new();
    develop_is_the_default(&f);
    git(
        &f.work,
        &[
            "symbolic-ref",
            "refs/remotes/origin/HEAD",
            "refs/remotes/origin/deleted",
        ],
    );
    commit_file(&f.work, "a.txt", "a\n");
    assert_eq!(f.target(), pt("develop-cloud", true, true));
}

#[test]
fn a_branch_named_like_a_tag_still_has_its_plain_name() {
    let f = Fixture::new();
    git(&f.work, &["checkout", "-q", "-b", "feature"]);
    git(&f.work, &["tag", "feature"]);
    commit_file(&f.work, "a.txt", "a\n");
    assert_eq!(f.target(), pt("feature", true, true));
}

#[test]
fn a_gone_upstream_falls_back_to_the_branch_name() {
    let f = Fixture::new();
    git(&f.work, &["checkout", "-q", "-b", "feature"]);
    commit_file(&f.work, "a.txt", "a\n");
    git(&f.work, &["push", "-q", "-u", "origin", "feature"]);
    git(&f.work, &["push", "-q", "origin", "--delete", "feature"]);
    assert_eq!(f.target(), pt("feature", true, true));
}

#[test]
fn a_detached_head_is_refused() {
    let f = Fixture::new();
    git(&f.work, &["checkout", "-q", "--detach"]);
    let err = push_target(&f.work).unwrap_err();
    assert!(err.contains("detached"), "{err}");
}

#[test]
fn a_repository_without_origin_is_refused() {
    let f = Fixture::new();
    git(&f.work, &["remote", "remove", "origin"]);
    let err = push_target(&f.work).unwrap_err();
    assert!(err.contains("origin"), "{err}");
}

#[test]
fn push_target_reads_a_session_worktree() {
    let f = Fixture::new();
    let wt = f.work.parent().unwrap().join("wt");
    git(
        &f.work,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "session-x",
            wt.to_str().unwrap(),
        ],
    );
    configure(&wt);
    commit_file(&wt, "w.txt", "w\n");
    fetch_origin(&wt).unwrap();
    assert_eq!(push_target(&wt).unwrap(), pt("session-x", true, true));
}

// ------------------------------------------------------------ fetch

#[test]
fn fetch_brings_in_new_remote_branches() {
    let f = Fixture::new();
    f.push_from_elsewhere("fresh", "x.txt");
    fetch_origin(&f.work).unwrap();
    git(&f.work, &["rev-parse", "--verify", "origin/fresh"]);
}

#[cfg(unix)]
#[test]
fn fetch_gives_up_after_the_timeout_and_leaves_no_orphan() {
    use std::os::unix::fs::PermissionsExt;
    let f = Fixture::new();
    let dir = f.work.parent().unwrap();
    let pidfile = dir.join("grandchild.pid");
    let script = dir.join("slow.sh");
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\nsleep 60 &\necho $! > '{}'\nwait\n",
            pidfile.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    git(
        &f.work,
        &["config", "core.sshCommand", script.to_str().unwrap()],
    );
    git(
        &f.work,
        &[
            "remote",
            "set-url",
            "origin",
            "ssh://example.invalid/repo.git",
        ],
    );
    let started = std::time::Instant::now();
    let err = fetch_with_timeout_for_test(&f.work, 1500).unwrap_err();
    assert!(err.contains("longer than"), "{err}");
    assert!(started.elapsed().as_secs() < 10);
    let pid: i32 = std::fs::read_to_string(&pidfile)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    // The grandchild (`sleep 60`) must be dead; give the kernel a moment to reap it.
    let mut alive = true;
    for _ in 0..40 {
        alive = unsafe { libc::kill(pid, 0) } == 0;
        if !alive {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    assert!(!alive, "the grandchild process {pid} outlived the fetch");
}

#[test]
fn fetch_reports_a_failure() {
    let f = Fixture::new();
    git(
        &f.work,
        &["remote", "set-url", "origin", "/nonexistent/path.git"],
    );
    assert!(fetch_origin(&f.work)
        .unwrap_err()
        .contains("Could not fetch"));
}

// ------------------------------------------------------------ snapshot

fn tree_of_head(dir: &Path) -> String {
    git(dir, &["rev-parse", "HEAD^{tree}"])
}

#[test]
fn a_clean_tree_has_no_patch_and_head_tree() {
    let f = Fixture::new();
    let staging = f._tmp.path().join("stage");
    let snap = snapshot(&f.work, &staging).unwrap();
    assert_eq!(snap.head, git(&f.work, &["rev-parse", "HEAD"]));
    assert_eq!(snap.tree, tree_of_head(&f.work));
    assert_eq!(snap.patch_path, None);
    assert_eq!(snap.sizes, Sizes::default());
    assert!(snap.ignored.is_empty());
    assert_eq!(snap.warnings, Warnings::default());
    assert!(!staging.join("index.tmp").exists());
}

fn dirty(f: &Fixture) {
    std::fs::write(f.work.join("README.md"), "hello\nchanged\n").unwrap();
    std::fs::write(f.work.join("new.txt"), "brand new\n").unwrap();
    std::fs::write(f.work.join("blob.bin"), [0u8, 1, 2, 255, 254, 0, 7]).unwrap();
    std::fs::write(f.work.join(".gitignore"), ".env.local\nbuild/\n").unwrap();
    std::fs::write(f.work.join(".env.local"), "SECRET_TOKEN=abc123\n").unwrap();
    std::fs::create_dir_all(f.work.join("build")).unwrap();
    std::fs::write(f.work.join("build/out.o"), vec![1u8; 100]).unwrap();
}

#[test]
fn dirty_untracked_binary_and_ignored_files_are_handled() {
    let f = Fixture::new();
    dirty(&f);
    let staging = f._tmp.path().join("stage");
    let snap = snapshot(&f.work, &staging).unwrap();

    let patch_path = snap.patch_path.clone().unwrap();
    let patch = std::fs::read(&patch_path).unwrap();
    let text = String::from_utf8_lossy(&patch);
    assert!(text.starts_with("diff --git a/"), "{text}");
    assert!(text.contains("+changed"));
    assert!(text.contains("new.txt"));
    assert!(text.contains("blob.bin"));
    assert!(text.contains("GIT binary patch"));
    assert!(!text.contains("b/.env.local"));
    assert!(!text.contains("SECRET_TOKEN"));
    assert!(!text.contains("out.o"));
    assert_eq!(snap.sizes.patch, patch.len() as u64);
    assert_eq!((snap.sizes.changed, snap.sizes.added), (1, 3));

    assert_eq!(
        snap.ignored,
        vec![(".env.local".to_owned(), 20), ("build/".to_owned(), 100)]
    );
    assert_eq!(snap.sizes.ignored, 120);
}

#[test]
fn the_patch_rebuilds_the_tree_on_a_fresh_clone() {
    let f = Fixture::new();
    dirty(&f);
    let staging = f._tmp.path().join("stage");
    let snap = snapshot(&f.work, &staging).unwrap();

    let remote = f._tmp.path().join("remote");
    git(
        f._tmp.path(),
        &["clone", "-q", f.origin.to_str().unwrap(), "remote"],
    );
    configure(&remote);
    assert_eq!(git(&remote, &["rev-parse", "HEAD"]), snap.head);
    git(
        &remote,
        &[
            "apply",
            "--binary",
            snap.patch_path.unwrap().to_str().unwrap(),
        ],
    );
    git(&remote, &["add", "-A"]);
    assert_eq!(git(&remote, &["write-tree"]), snap.tree);
    assert!(!remote.join(".env.local").exists());
}

#[test]
fn snapshot_leaves_index_stash_branches_and_files_untouched() {
    let f = Fixture::new();
    git(&f.work, &["checkout", "-q", "-b", "feature"]);
    commit_file(&f.work, "a.txt", "a\n");
    // A stash, a staged edit, an unstaged edit, an untracked and an ignored file.
    std::fs::write(f.work.join("a.txt"), "stashed\n").unwrap();
    git(&f.work, &["stash", "push", "-q"]);
    std::fs::write(f.work.join("staged.txt"), "staged\n").unwrap();
    git(&f.work, &["add", "staged.txt"]);
    std::fs::write(f.work.join("a.txt"), "unstaged\n").unwrap();
    std::fs::write(f.work.join("untracked.txt"), "u\n").unwrap();
    std::fs::write(f.work.join(".gitignore"), "*.log\n").unwrap();
    std::fs::write(f.work.join("x.log"), "log\n").unwrap();

    let observe = |dir: &Path| {
        (
            std::fs::read(dir.join(".git/index")).unwrap(),
            git(dir, &["stash", "list"]),
            git(dir, &["rev-parse", "refs/stash"]),
            git(dir, &["for-each-ref", "refs/heads", "refs/stash"]),
            git(dir, &["branch", "--list", "-vv"]),
            git(dir, &["status", "--porcelain=v2", "--ignored"]),
            git(dir, &["config", "--list", "--local"]),
            std::fs::read(dir.join("a.txt")).unwrap(),
            std::fs::read(dir.join("untracked.txt")).unwrap(),
        )
    };
    let before = observe(&f.work);
    fetch_origin(&f.work).unwrap();
    let _ = push_target(&f.work).unwrap();
    let snap = snapshot(&f.work, &f._tmp.path().join("stage")).unwrap();
    assert!(snap.patch_path.is_some());
    assert_eq!(observe(&f.work), before);
    // The real index stays usable and unchanged: staged.txt still staged alone.
    assert_eq!(
        git(&f.work, &["diff", "--cached", "--name-only"]),
        "staged.txt"
    );
}

fn rebuilt_tree(f: &Fixture, patch: &Path, name: &str) -> String {
    let clone = f._tmp.path().join(name);
    git(
        f._tmp.path(),
        &["clone", "-q", f.origin.to_str().unwrap(), name],
    );
    configure(&clone);
    git(&clone, &["apply", "--binary", patch.to_str().unwrap()]);
    git(&clone, &["add", "-A"]);
    git(&clone, &["write-tree"])
}

#[test]
fn the_tree_is_deterministic_and_ignores_diff_config() {
    let f = Fixture::new();
    dirty(&f);
    let stage = |name: &str| f._tmp.path().join(name);
    let first = snapshot(&f.work, &stage("s1")).unwrap();
    let second = snapshot(&f.work, &stage("s2")).unwrap();
    assert_eq!(first.tree, second.tree);
    assert_eq!(
        std::fs::read(first.patch_path.as_ref().unwrap()).unwrap(),
        std::fs::read(second.patch_path.as_ref().unwrap()).unwrap()
    );

    git(&f.work, &["config", "core.autocrlf", "input"]);
    git(&f.work, &["config", "diff.noprefix", "true"]);
    git(&f.work, &["config", "diff.mnemonicPrefix", "true"]);
    git(&f.work, &["config", "diff.renames", "copies"]);
    let third = snapshot(&f.work, &stage("s3")).unwrap();
    assert_eq!(third.tree, first.tree);
    let patch = std::fs::read(third.patch_path.unwrap()).unwrap();
    assert!(patch.starts_with(b"diff --git a/"));
    assert_eq!(patch, std::fs::read(first.patch_path.unwrap()).unwrap());
}

/// `core.autocrlf` is the user's own choice and is honoured on purpose: it decides what a
/// commit would store, so the snapshot stores the same. Because the patch is made from the
/// same staged content, the cloud (which has no autocrlf) rebuilds the same tree id.
#[test]
fn autocrlf_changes_the_stored_line_endings_and_the_patch_still_rebuilds_the_tree() {
    let f = Fixture::new();
    std::fs::write(f.work.join("crlf.txt"), "a\r\nb\r\n").unwrap();
    let keep = snapshot(&f.work, &f._tmp.path().join("s1")).unwrap();
    git(&f.work, &["config", "core.autocrlf", "input"]);
    let normalised = snapshot(&f.work, &f._tmp.path().join("s2")).unwrap();
    assert_ne!(keep.tree, normalised.tree);
    let patch = std::fs::read(normalised.patch_path.as_ref().unwrap()).unwrap();
    assert!(!String::from_utf8_lossy(&patch).contains('\r'));
    assert_eq!(
        rebuilt_tree(&f, normalised.patch_path.as_ref().unwrap(), "c1"),
        normalised.tree
    );
    assert_eq!(
        rebuilt_tree(&f, keep.patch_path.as_ref().unwrap(), "c2"),
        keep.tree
    );
    // The same setting twice is stable.
    let again = snapshot(&f.work, &f._tmp.path().join("s3")).unwrap();
    assert_eq!(again.tree, normalised.tree);
}

#[test]
fn a_user_diff_context_does_not_break_the_patch() {
    let f = Fixture::new();
    let lines: String = (1..=30).map(|n| format!("line {n}\n")).collect();
    commit_file(&f.work, "long.txt", &lines);
    git(&f.work, &["push", "-q", "origin", "main"]);
    git(&f.work, &["config", "diff.context", "0"]);
    let edited = lines
        .replace("line 10\n", "ten\n")
        .replace("line 20\n", "twenty\n");
    std::fs::write(f.work.join("long.txt"), edited).unwrap();
    let snap = snapshot(&f.work, &f._tmp.path().join("s")).unwrap();
    let patch = snap.patch_path.unwrap();
    assert!(std::fs::read_to_string(&patch)
        .unwrap()
        .contains(" line 9\n"));
    assert_eq!(rebuilt_tree(&f, &patch, "c"), snap.tree);
}

#[test]
fn submodule_ignore_settings_do_not_hide_a_moved_submodule() {
    let f = Fixture::new();
    let sub_src = f._tmp.path().join("sub-src");
    std::fs::create_dir_all(&sub_src).unwrap();
    git(&sub_src, &["init", "-q", "-b", "main"]);
    configure(&sub_src);
    commit_file(&sub_src, "lib.txt", "lib\n");
    git(
        &f.work,
        &[
            "-c",
            "protocol.file.allow=always",
            "submodule",
            "add",
            "-q",
            sub_src.to_str().unwrap(),
            "vendor",
        ],
    );
    git(&f.work, &["commit", "-q", "-m", "add submodule"]);
    let vendor = f.work.join("vendor");
    configure(&vendor);
    commit_file(&vendor, "more.txt", "more\n");
    git(&f.work, &["config", "diff.ignoreSubmodules", "all"]);
    git(&f.work, &["config", "submodule.vendor.ignore", "all"]);
    let snap = snapshot(&f.work, &f._tmp.path().join("s")).unwrap();
    assert_ne!(snap.tree, tree_of_head(&f.work));
    assert_eq!(snap.sizes.changed, 1);
    let text = std::fs::read_to_string(snap.patch_path.unwrap()).unwrap();
    assert!(text.contains("Subproject commit"), "{text}");
}

fn listing(dir: &Path) -> Vec<String> {
    fn walk(base: &Path, dir: &Path, out: &mut Vec<String>) {
        for entry in std::fs::read_dir(dir).unwrap().flatten() {
            let path = entry.path();
            out.push(path.strip_prefix(base).unwrap().display().to_string());
            if entry.file_type().unwrap().is_dir() {
                walk(base, &path, out);
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, dir, &mut out);
    out.sort();
    out
}

#[test]
fn a_split_index_leaks_no_shared_index_into_the_repository() {
    let f = Fixture::new();
    git(&f.work, &["config", "core.splitIndex", "true"]);
    git(&f.work, &["update-index", "--split-index"]);
    std::fs::write(f.work.join("README.md"), "hello\nmore\n").unwrap();
    std::fs::write(f.work.join("new.txt"), "new\n").unwrap();
    let before = listing(&f.work.join(".git"));
    assert!(before.iter().any(|n| n.starts_with("sharedindex.")));
    let snap = snapshot(&f.work, &f._tmp.path().join("s")).unwrap();
    assert!(snap.patch_path.is_some());
    assert_eq!(listing(&f.work.join(".git")), before);
}

#[test]
fn no_object_reaches_the_real_object_store() {
    let f = Fixture::new();
    dirty(&f);
    let before = listing(&f.work.join(".git/objects"));
    let snap = snapshot(&f.work, &f._tmp.path().join("s")).unwrap();
    assert!(snap.patch_path.is_some());
    assert_eq!(listing(&f.work.join(".git/objects")), before);
    assert!(!f._tmp.path().join("s/objects.tmp").exists());
    assert!(!f._tmp.path().join("s/index.tmp").exists());
}

#[test]
fn files_over_the_limit_are_refused_before_anything_is_written() {
    let f = Fixture::new();
    std::fs::write(f.work.join("big.bin"), vec![7u8; 4096]).unwrap();
    let before = listing(&f.work.join(".git/objects"));
    let err = snapshot_with_limit_for_test(&f.work, &f._tmp.path().join("s"), 1000).unwrap_err();
    assert!(err.contains("limit"), "{err}");
    assert_eq!(listing(&f.work.join(".git/objects")), before);
    assert!(!f._tmp.path().join("s/uncommitted.patch").exists());
    // Under the limit it works.
    assert!(snapshot_with_limit_for_test(&f.work, &f._tmp.path().join("s"), 1 << 20).is_ok());
}

/// Git re-reads an index entry as new as the index file itself. The scratch copy of the index
/// must keep the real index's time, or a file rewritten in place with the same size in the
/// same tick as the last index refresh would be taken as unchanged and its old blob kept.
#[test]
fn the_snapshot_sees_a_same_size_rewrite_made_in_the_tick_of_the_last_index_refresh() {
    use std::time::{Duration, SystemTime};
    let f = Fixture::new();
    // Compare size and mtime only, as a filesystem with coarse ctime would.
    git(&f.work, &["config", "core.trustctime", "false"]);
    commit_file(&f.work, "same.txt", "aaaa\n");
    let tick = SystemTime::now() - Duration::from_secs(30);
    let set_time = |path: &Path| {
        std::fs::OpenOptions::new()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(tick)
            .unwrap();
    };
    let file = f.work.join("same.txt");
    set_time(&file);
    git(&f.work, &["update-index", "--really-refresh"]);
    // The index was refreshed in the same tick that the file was last written.
    set_time(&f.work.join(".git/index"));
    // Same length, same inode, same mtime: only the content differs.
    std::fs::write(&file, "bbbb\n").unwrap();
    set_time(&file);
    let snap = snapshot_with_limit_for_test(&f.work, &f._tmp.path().join("s"), 1 << 20).unwrap();
    assert_ne!(
        snap.tree,
        tree_of_head(&f.work),
        "the snapshot kept the old content"
    );
}

#[test]
fn a_relative_staging_path_works() {
    let f = Fixture::new();
    std::fs::write(f.work.join("new.txt"), "new\n").unwrap();
    // Reach the temp dir from the current directory without a single absolute component.
    let cwd = std::env::current_dir().unwrap();
    let mut rel = PathBuf::new();
    for _ in cwd.components().skip(1) {
        rel.push("..");
    }
    rel.push(f._tmp.path().strip_prefix("/").unwrap());
    rel.push("relative-stage");
    assert!(rel.is_relative());
    let snap = snapshot(&f.work, &rel).unwrap();
    assert!(snap.patch_path.unwrap().is_absolute());
    assert_ne!(snap.tree, tree_of_head(&f.work));
}

#[test]
fn snapshot_reads_a_session_worktree() {
    let f = Fixture::new();
    let wt = f.work.parent().unwrap().join("wt");
    git(
        &f.work,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "session-x",
            wt.to_str().unwrap(),
        ],
    );
    configure(&wt);
    std::fs::write(wt.join("only-here.txt"), "w\n").unwrap();
    std::fs::write(wt.join("README.md"), "hello\nwt edit\n").unwrap();
    let main_index = std::fs::read(f.work.join(".git/index")).unwrap();

    let snap = snapshot(&wt, &f._tmp.path().join("stage")).unwrap();
    let text = std::fs::read_to_string(snap.patch_path.unwrap()).unwrap();
    assert!(text.contains("only-here.txt") && text.contains("wt edit"));
    assert_eq!((snap.sizes.changed, snap.sizes.added), (1, 1));
    assert_eq!(snap.head, git(&wt, &["rev-parse", "HEAD"]));
    // The main checkout is not part of it, and nothing in it moved.
    assert_eq!(
        std::fs::read(f.work.join(".git/index")).unwrap(),
        main_index
    );
    assert!(!f.work.join("only-here.txt").exists());
    assert_eq!(
        git(&wt, &["status", "--porcelain"]),
        "M README.md\n?? only-here.txt"
    );
}

#[test]
fn a_subdirectory_cwd_snapshots_the_whole_repository() {
    let f = Fixture::new();
    std::fs::create_dir_all(f.work.join("sub")).unwrap();
    std::fs::write(f.work.join("sub/in.txt"), "in\n").unwrap();
    std::fs::write(f.work.join("top.txt"), "top\n").unwrap();
    let snap = snapshot(&f.work.join("sub"), &f._tmp.path().join("stage")).unwrap();
    assert_eq!(snap.sizes.added, 2);
}

#[test]
fn a_repository_without_commits_is_refused() {
    let tmp = tempfile::tempdir().unwrap();
    git(tmp.path(), &["init", "-q", "-b", "main"]);
    let err = snapshot(tmp.path(), &tmp.path().join("stage")).unwrap_err();
    assert!(err.contains("no commits"), "{err}");
}

#[test]
fn lfs_in_gitattributes_is_a_warning() {
    let f = Fixture::new();
    let staging = f._tmp.path().join("stage");
    assert!(!snapshot(&f.work, &staging).unwrap().warnings.lfs);
    std::fs::write(
        f.work.join(".gitattributes"),
        "*.psd filter=lfs diff=lfs merge=lfs -text\n",
    )
    .unwrap();
    assert!(snapshot(&f.work, &staging).unwrap().warnings.lfs);
    git(&f.work, &["add", ".gitattributes"]);
    git(&f.work, &["commit", "-q", "-m", "lfs"]);
    assert!(snapshot(&f.work, &staging).unwrap().warnings.lfs);
}

#[test]
fn a_nested_gitattributes_with_lfs_is_a_warning() {
    let f = Fixture::new();
    std::fs::create_dir_all(f.work.join("assets")).unwrap();
    std::fs::write(f.work.join("assets/.gitattributes"), "*.png filter=lfs\n").unwrap();
    assert!(
        snapshot(&f.work, &f._tmp.path().join("s"))
            .unwrap()
            .warnings
            .lfs
    );
}

#[test]
fn a_dirty_submodule_is_a_warning() {
    let f = Fixture::new();
    let sub_src = f._tmp.path().join("sub-src");
    std::fs::create_dir_all(&sub_src).unwrap();
    git(&sub_src, &["init", "-q", "-b", "main"]);
    configure(&sub_src);
    commit_file(&sub_src, "lib.txt", "lib\n");
    git(
        &f.work,
        &[
            "-c",
            "protocol.file.allow=always",
            "submodule",
            "add",
            "-q",
            sub_src.to_str().unwrap(),
            "vendor",
        ],
    );
    git(&f.work, &["commit", "-q", "-m", "add submodule"]);
    let staging = f._tmp.path().join("stage");
    assert!(snapshot(&f.work, &staging)
        .unwrap()
        .warnings
        .dirty_submodules
        .is_empty());
    std::fs::write(f.work.join("vendor/lib.txt"), "edited\n").unwrap();
    assert_eq!(
        snapshot(&f.work, &staging)
            .unwrap()
            .warnings
            .dirty_submodules,
        vec!["vendor".to_owned()]
    );
}

// ------------------------------------------------------------ transcript

fn claude_home(dir: &Path, id: &str, cwd: &str) -> PathBuf {
    let home = dir.join("claude");
    let project = home.join("projects").join(cwd);
    std::fs::create_dir_all(project.join(id).join("subagents")).unwrap();
    std::fs::write(project.join(format!("{id}.jsonl")), "{\"a\":1}\n").unwrap();
    std::fs::write(project.join(id).join("subagents/agent.jsonl"), "sub").unwrap();
    std::fs::create_dir_all(home.join("file-history").join(id)).unwrap();
    std::fs::write(home.join("file-history").join(id).join("abc@v1"), "hist").unwrap();
    // Another session that must not be copied.
    std::fs::write(project.join("other.jsonl"), "other").unwrap();
    home
}

#[test]
fn the_transcript_files_of_one_session_are_collected() {
    let tmp = tempfile::tempdir().unwrap();
    let home = claude_home(tmp.path(), "sess-1", "-Users-me-app");
    let staging = tmp.path().join("stage");
    let t = collect_transcript(&home, Path::new("/Users/me/app"), "sess-1", &staging).unwrap();
    assert_eq!(t.root, staging.join("transcript"));
    assert_eq!(std::fs::read_to_string(&t.jsonl).unwrap(), "{\"a\":1}\n");
    assert_eq!(t.jsonl_bytes, 8);
    assert_eq!(t.total_bytes, 8 + 3 + 4);
    let root = &t.root;
    assert!(root
        .join("projects/-Users-me-app/sess-1/subagents/agent.jsonl")
        .is_file());
    assert!(root.join("file-history/sess-1/abc@v1").is_file());
    assert!(!root.join("projects/-Users-me-app/other.jsonl").exists());
}

#[test]
fn the_project_folder_is_found_even_when_the_cwd_does_not_match() {
    let tmp = tempfile::tempdir().unwrap();
    let home = claude_home(tmp.path(), "sess-2", "-some-other-dir");
    let t = collect_transcript(
        &home,
        Path::new("/elsewhere"),
        "sess-2",
        &tmp.path().join("s"),
    )
    .unwrap();
    assert!(t.jsonl.ends_with("projects/-some-other-dir/sess-2.jsonl"));
}

#[test]
fn a_missing_conversation_or_bad_id_is_refused() {
    let tmp = tempfile::tempdir().unwrap();
    let home = claude_home(tmp.path(), "sess-3", "-p");
    let staging = tmp.path().join("s");
    let cwd = Path::new("/p");
    assert!(collect_transcript(&home, cwd, "nope", &staging).is_err());
    assert!(collect_transcript(&home, cwd, "../sess-3", &staging).is_err());
    assert!(collect_transcript(&home, cwd, "", &staging).is_err());
}

#[test]
fn a_jsonl_over_the_limit_is_refused() {
    let tmp = tempfile::tempdir().unwrap();
    let home = claude_home(tmp.path(), "big", "-p");
    let file = std::fs::OpenOptions::new()
        .write(true)
        .open(home.join("projects/-p/big.jsonl"))
        .unwrap();
    file.set_len(MAX_JSONL_BYTES + 1).unwrap();
    let err = collect_transcript(&home, Path::new("/p"), "big", &tmp.path().join("s")).unwrap_err();
    assert!(err.contains("50 MB"), "{err}");
    let file = std::fs::OpenOptions::new()
        .write(true)
        .open(home.join("projects/-p/big.jsonl"))
        .unwrap();
    file.set_len(MAX_JSONL_BYTES).unwrap();
    collect_transcript(&home, Path::new("/p"), "big", &tmp.path().join("s")).unwrap();
}

#[cfg(unix)]
#[test]
fn symlinks_in_the_transcript_folders_are_not_followed() {
    let tmp = tempfile::tempdir().unwrap();
    let home = claude_home(tmp.path(), "sess-4", "-p");
    let secret = tmp.path().join("secret.txt");
    std::fs::write(&secret, "secret").unwrap();
    std::os::unix::fs::symlink(&secret, home.join("projects/-p/sess-4/link.txt")).unwrap();
    let t = collect_transcript(&home, Path::new("/p"), "sess-4", &tmp.path().join("s")).unwrap();
    assert!(!t.root.join("projects/-p/sess-4/link.txt").exists());
}

#[cfg(unix)]
#[test]
fn symlinked_roots_of_the_transcript_are_not_followed() {
    let tmp = tempfile::tempdir().unwrap();
    let home = claude_home(tmp.path(), "sess-5", "-p");
    let outside = tmp.path().join("outside");
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::write(outside.join("secret.txt"), "secret").unwrap();
    let project = home.join("projects/-p");
    // The id folder and the file-history folder are links to somewhere else.
    std::fs::remove_dir_all(project.join("sess-5")).unwrap();
    std::os::unix::fs::symlink(&outside, project.join("sess-5")).unwrap();
    std::fs::remove_dir_all(home.join("file-history/sess-5")).unwrap();
    std::os::unix::fs::symlink(&outside, home.join("file-history/sess-5")).unwrap();
    let t = collect_transcript(&home, Path::new("/p"), "sess-5", &tmp.path().join("s")).unwrap();
    assert!(!t.root.join("projects/-p/sess-5").exists());
    assert!(!t.root.join("file-history/sess-5").exists());

    // A transcript that is itself a link is not a transcript.
    let real = project.join("sess-6.jsonl");
    std::fs::write(&real, "x").unwrap();
    std::os::unix::fs::symlink(&real, project.join("sess-7.jsonl")).unwrap();
    let err =
        collect_transcript(&home, Path::new("/p"), "sess-7", &tmp.path().join("s2")).unwrap_err();
    assert!(err.contains("Could not find"), "{err}");
}

#[test]
fn a_repository_path_with_a_colon_works() {
    let tmp = tempfile::Builder::new().prefix("a:b;c-").tempdir().unwrap();
    let f = Fixture::in_dir(tmp);
    assert!(f.work.to_string_lossy().contains(':'));
    dirty(&f);
    let snap = snapshot(&f.work, &f._tmp.path().join("s")).unwrap();
    assert_eq!(
        rebuilt_tree(&f, snap.patch_path.as_ref().unwrap(), "c"),
        snap.tree
    );
}

#[test]
fn a_big_tracked_file_with_a_small_edit_is_not_refused() {
    let f = Fixture::new();
    let lines: String = (1..=500).map(|n| format!("line {n}\n")).collect();
    commit_file(&f.work, "big.txt", &lines);
    std::fs::write(f.work.join("big.txt"), lines.replace("line 5\n", "five\n")).unwrap();
    assert!(std::fs::metadata(f.work.join("big.txt")).unwrap().len() > 3000);
    let snap = snapshot_with_limit_for_test(&f.work, &f._tmp.path().join("s"), 2000).unwrap();
    assert_eq!(snap.sizes.changed, 1);
    // The finished patch is still checked against the limit.
    let err = snapshot_with_limit_for_test(&f.work, &f._tmp.path().join("s2"), 100).unwrap_err();
    assert!(err.contains("limit"), "{err}");
}
