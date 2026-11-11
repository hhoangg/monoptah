//! The local, read-only half of "Continue in cloud": decide where the branch is pushed and
//! snapshot everything that travels with it (design 4.2.2 steps 1-2, rule D8).
//!
//! Nothing here pushes, uploads or contacts a Host. The user's real index, working tree,
//! local branches and stash are never written: the uncommitted state is built through a
//! temporary `GIT_INDEX_FILE` that lives in the staging folder.
use crate::scm::git_cmd;
use std::{
    io::Read,
    path::{Path, PathBuf},
    process::Stdio,
    time::{Duration, Instant},
};

/// The patch of uncommitted and untracked files may not exceed this size.
pub const MAX_PATCH_BYTES: u64 = 500 * 1024 * 1024;
/// The Claude transcript (`<id>.jsonl`) may not exceed this size.
pub const MAX_JSONL_BYTES: u64 = 50 * 1024 * 1024;
const FETCH_TIMEOUT: Duration = Duration::from_secs(60);
const LS_REMOTE_TIMEOUT: Duration = Duration::from_secs(10);
const REMOTE: &str = "origin";
/// Bound on the entries visited to size one ignored directory.
const SIZE_WALK_BUDGET: usize = 200_000;

/// Config that would otherwise change the shape of the patch or of what is staged.
const NEUTRAL: [&str; 12] = [
    "-c",
    "diff.noprefix=false",
    "-c",
    "diff.mnemonicPrefix=false",
    "-c",
    "core.fsmonitor=false",
    "-c",
    "core.quotepath=false",
    // A split index would write `sharedindex.*` into the real `.git`.
    "-c",
    "core.splitIndex=false",
    // Never fail (or warn) on mixed line endings while staging into the scratch index.
    "-c",
    "core.safecrlf=false",
];

/// Where the branch is pushed. `branch` is the name on origin.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PushTarget {
    pub branch: String,
    /// `HEAD` is not yet what origin has under `branch`.
    pub needs_push: bool,
    /// `branch` does not exist on origin yet (a cancel may offer to delete it).
    pub creates_branch: bool,
}

/// Facts that the dialog shows as warnings, never as blockers.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Warnings {
    /// A `.gitattributes` routes files through `filter=lfs`. A branch push may not carry
    /// the LFS objects.
    pub lfs: bool,
    /// Submodules whose checkout has changes. Their content does not travel.
    pub dirty_submodules: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Sizes {
    /// Bytes of the patch file, 0 when there is none.
    pub patch: u64,
    /// Total bytes of the ignored files (they stay local).
    pub ignored: u64,
    /// Tracked files with uncommitted changes.
    pub changed: usize,
    /// Untracked, non-ignored files.
    pub added: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Snapshot {
    pub head: String,
    /// `write-tree` of the temporary index after `add -A`. Content addressed, so it is
    /// comparable on the cloud side whatever the git version or diff configuration.
    pub tree: String,
    /// `None` when the working tree matches `HEAD`.
    pub patch_path: Option<PathBuf>,
    /// Ignored paths (directories end with `/`) and their size. Never in the patch.
    pub ignored: Vec<(String, u64)>,
    pub sizes: Sizes,
    pub warnings: Warnings,
}

/// The transcript files copied into staging.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Transcript {
    /// Root of the copy; it mirrors the layout under the provider home
    /// (`projects/<dir>/<id>.jsonl`, `projects/<dir>/<id>/`, `file-history/<id>/`).
    pub root: PathBuf,
    pub jsonl: PathBuf,
    pub jsonl_bytes: u64,
    /// Bytes of everything copied.
    pub total_bytes: u64,
}

// ---------------------------------------------------------------- git plumbing

/// Where a snapshot writes: its own index and its own object directory, so nothing lands
/// in the user's `.git`. Reads still see the real objects through the alternate.
struct Scratch {
    index: PathBuf,
    objects: PathBuf,
    real_objects: PathBuf,
}

fn run(
    root: &Path,
    args: &[&str],
    scratch: Option<&Scratch>,
    stdout: Option<Stdio>,
) -> Result<Vec<u8>, String> {
    let mut cmd = git_cmd();
    cmd.args(NEUTRAL)
        .arg("--no-pager")
        .arg("-C")
        .arg(root)
        .args(args)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_NO_LAZY_FETCH", "1")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .env_remove("GIT_OBJECT_DIRECTORY")
        .env_remove("GIT_ALTERNATE_OBJECT_DIRECTORIES");
    if let Some(scratch) = scratch {
        cmd.env("GIT_INDEX_FILE", &scratch.index)
            .env("GIT_OBJECT_DIRECTORY", &scratch.objects)
            .env(
                "GIT_ALTERNATE_OBJECT_DIRECTORIES",
                c_quote(&scratch.real_objects),
            );
    }
    let to_file = stdout.is_some();
    if let Some(stdout) = stdout {
        cmd.stdout(stdout);
    }
    let out = cmd
        .output()
        .map_err(|e| format!("Could not run Git: {e}"))?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_owned());
    }
    Ok(if to_file { Vec::new() } else { out.stdout })
}

/// The alternates list is separated by `:` (`;` on Windows); git reads an entry that starts
/// with a double quote as C-style quoted, so a path containing the separator still works.
fn c_quote(path: &Path) -> String {
    let text = path.to_string_lossy();
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        if c == '"' || c == '\\' {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('"');
    out
}

fn git_text(root: &Path, args: &[&str], scratch: Option<&Scratch>) -> Result<String, String> {
    let out = run(root, args, scratch, None)?;
    Ok(String::from_utf8_lossy(&out).trim().to_owned())
}

fn git_ok(root: &Path, args: &[&str]) -> Result<bool, String> {
    let mut cmd = git_cmd();
    let out = cmd
        .args(NEUTRAL)
        .arg("-C")
        .arg(root)
        .args(args)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_NO_LAZY_FETCH", "1")
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .map_err(|e| format!("Could not run Git: {e}"))?;
    match out.status.code() {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        _ => Err(String::from_utf8_lossy(&out.stderr).trim().to_owned()),
    }
}

fn toplevel(cwd: &Path) -> Result<PathBuf, String> {
    git_text(cwd, &["rev-parse", "--show-toplevel"], None).map(PathBuf::from)
}

fn is_ancestor(root: &Path, ancestor: &str, descendant: &str) -> Result<bool, String> {
    git_ok(root, &["merge-base", "--is-ancestor", ancestor, descendant])
}

fn remote_tip(root: &Path, branch: &str) -> Option<String> {
    let name = format!("refs/remotes/{REMOTE}/{branch}");
    git_text(root, &["rev-parse", "--verify", "--quiet", &name], None)
        .ok()
        .filter(|s| !s.is_empty())
}

// ---------------------------------------------------------------- step 1: fetch

enum Timed {
    Expired,
    Stopped,
    Failed(String),
}

/// Runs git with a deadline. On timeout the whole process group is killed, so helpers such
/// as `ssh` or a credential program do not outlive the call.
fn run_timed(root: &Path, args: &[&str], timeout: Duration) -> Result<String, Timed> {
    run_timed_with(root, args, timeout, None, None)
}

/// [`run_timed`] that also stops when `cancel` is set, and may run with another `PATH` (a
/// push needs the hooks and credential helpers of the login shell).
fn run_timed_with(
    root: &Path,
    args: &[&str],
    timeout: Duration,
    cancel: Option<&std::sync::atomic::AtomicBool>,
    path: Option<&str>,
) -> Result<String, Timed> {
    let failed = |e: String| Timed::Failed(e);
    let mut out = tempfile::tempfile().map_err(|e| failed(e.to_string()))?;
    let mut err = tempfile::tempfile().map_err(|e| failed(e.to_string()))?;
    let mut cmd = git_cmd();
    cmd.args(["-c", "gc.auto=0", "-c", "maintenance.auto=false"])
        .arg("-C")
        .arg(root)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env_remove("GIT_DIR")
        .env_remove("GIT_INDEX_FILE")
        .stdin(Stdio::null())
        .stdout(out.try_clone().map_err(|e| failed(e.to_string()))?)
        .stderr(err.try_clone().map_err(|e| failed(e.to_string()))?);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    if let Some(path) = path {
        cmd.env("PATH", path);
    }
    let mut child = cmd
        .spawn()
        .map_err(|e| failed(format!("Could not run Git: {e}")))?;
    let started = Instant::now();
    let status = loop {
        match child.try_wait().map_err(|e| failed(e.to_string()))? {
            Some(status) => break status,
            None if started.elapsed() >= timeout => {
                crate::process::terminate(&mut child);
                return Err(Timed::Expired);
            }
            None if cancel.is_some_and(|c| c.load(std::sync::atomic::Ordering::Relaxed)) => {
                crate::process::terminate(&mut child);
                return Err(Timed::Stopped);
            }
            None => std::thread::sleep(Duration::from_millis(25)),
        }
    };
    let read = |file: &mut std::fs::File| {
        let mut text = String::new();
        let _ = std::io::Seek::rewind(file);
        let _ = file.read_to_string(&mut text);
        text
    };
    if status.success() {
        Ok(read(&mut out))
    } else {
        Err(Timed::Failed(read(&mut err).trim().to_owned()))
    }
}

/// A plain `git fetch origin`, killed (with its children) after 60 s and never prompting
/// for credentials. It updates only the remote-tracking refs, as any fetch does.
pub fn fetch_origin(cwd: &Path) -> Result<(), String> {
    fetch_with_timeout(cwd, FETCH_TIMEOUT)
}

fn fetch_with_timeout(cwd: &Path, timeout: Duration) -> Result<(), String> {
    let root = toplevel(cwd)?;
    match run_timed(&root, &["fetch", "--no-tags", REMOTE], timeout) {
        Ok(_) => Ok(()),
        Err(Timed::Expired) => Err(format!(
            "Fetching {REMOTE} took longer than {} seconds",
            timeout.as_secs()
        )),
        Err(Timed::Stopped) => Err("Cancelled".into()),
        Err(Timed::Failed(e)) => Err(format!("Could not fetch {REMOTE}: {e}")),
    }
}

// ---------------------------------------------------------------- step 2: push target

/// Chooses the branch to push, from the remote-tracking refs as they are now (call
/// [`fetch_origin`] first). Never a force push and never a push that changes origin's
/// default branch.
pub fn push_target(cwd: &Path) -> Result<PushTarget, String> {
    let root = toplevel(cwd)?;
    let branch = git_text(&root, &["symbolic-ref", "-q", "HEAD"], None)
        .ok()
        .and_then(|r| r.strip_prefix("refs/heads/").map(str::to_owned))
        .filter(|b| !b.is_empty())
        .ok_or("HEAD is detached. Switch to a branch before continuing in the cloud.")?;
    let head = git_text(&root, &["rev-parse", "--verify", "HEAD"], None)
        .map_err(|_| "This branch has no commits yet".to_string())?;
    if git_text(&root, &["remote", "get-url", REMOTE], None).is_err() {
        return Err(format!("This project has no {REMOTE} remote to push to"));
    }
    let defaults = default_branches(&root);

    // An upstream that is itself the default branch counts as no upstream: the branch
    // keeps its own name instead.
    let plain = if defaults.may_be_default(&branch) {
        branch.clone()
    } else {
        upstream_on_origin(&root, &branch)
            .filter(|up| !defaults.may_be_default(up))
            .unwrap_or_else(|| branch.clone())
    };
    let tip = remote_tip(&root, &plain);
    let at_tip = tip.as_deref() == Some(head.as_str());
    if at_tip {
        // Nothing to push, so even the default branch is left untouched.
        return Ok(PushTarget {
            branch: plain,
            needs_push: false,
            creates_branch: false,
        });
    }
    if !defaults.may_be_default(&plain) {
        match tip {
            None => {
                return Ok(PushTarget {
                    branch: plain,
                    needs_push: true,
                    creates_branch: true,
                })
            }
            Some(tip) if is_ancestor(&root, &tip, &head)? => {
                return Ok(PushTarget {
                    branch: plain,
                    needs_push: true,
                    creates_branch: false,
                })
            }
            // Diverged, or HEAD is behind origin: a fast-forward push is impossible.
            Some(_) => {}
        }
    }
    cloud_branch(&root, &branch, &head)
}

/// `<branch>-cloud`, `-cloud-2`, ... The first name that is free on origin, or that
/// already holds HEAD or an ancestor of it (an earlier move), where a push fast-forwards.
fn cloud_branch(root: &Path, branch: &str, head: &str) -> Result<PushTarget, String> {
    for n in 1..=1000u32 {
        let name = if n == 1 {
            format!("{branch}-cloud")
        } else {
            format!("{branch}-cloud-{n}")
        };
        match remote_tip(root, &name) {
            None => {
                return Ok(PushTarget {
                    branch: name,
                    needs_push: true,
                    creates_branch: true,
                })
            }
            Some(tip) if tip == head => {
                return Ok(PushTarget {
                    branch: name,
                    needs_push: false,
                    creates_branch: false,
                })
            }
            Some(tip) if is_ancestor(root, &tip, head)? => {
                return Ok(PushTarget {
                    branch: name,
                    needs_push: true,
                    creates_branch: false,
                })
            }
            Some(_) => {}
        }
    }
    Err(format!(
        "Could not find a free {branch}-cloud branch name on {REMOTE}"
    ))
}

/// The branch name on origin that `branch` tracks, when it tracks one that still exists.
fn upstream_on_origin(root: &Path, branch: &str) -> Option<String> {
    let remote = git_text(root, &["config", &format!("branch.{branch}.remote")], None).ok()?;
    if remote != REMOTE {
        return None;
    }
    let merge = git_text(root, &["config", &format!("branch.{branch}.merge")], None).ok()?;
    let name = merge.strip_prefix("refs/heads/")?.to_owned();
    remote_tip(root, &name).map(|_| name)
}

/// What is known about origin's default branch.
struct Defaults {
    names: Vec<String>,
    /// origin itself named its default branch just now (`ls-remote --symref`).
    confirmed: bool,
}

impl Defaults {
    /// Whether pushing to `name` might change origin's default branch.
    ///
    /// Fail closed: when origin did not confirm its default (it was unreachable, timed out,
    /// or reported no symref), any name could be the default, because `origin/HEAD` may be
    /// missing or stale and a default such as `develop` has no telltale name. Only a
    /// confirmed answer that does not list `name` clears it.
    fn may_be_default(&self, name: &str) -> bool {
        !self.confirmed || self.names.iter().any(|n| n == name)
    }
}

/// Every branch that may be origin's default: what a read-only `ls-remote --symref`
/// reports, the target of `refs/remotes/origin/HEAD` while it still exists, and `main` and
/// `master` when they exist on origin. See [`Defaults::may_be_default`] for the case where
/// origin cannot be asked.
fn default_branches(root: &Path) -> Defaults {
    let mut names: Vec<String> = Vec::new();
    let head = format!("refs/remotes/{REMOTE}/HEAD");
    let prefix = format!("refs/remotes/{REMOTE}/");
    if let Some(name) = git_text(root, &["symbolic-ref", "-q", &head], None)
        .ok()
        .and_then(|t| t.strip_prefix(&prefix).map(str::to_owned))
        .filter(|name| remote_tip(root, name).is_some())
    {
        names.push(name);
    }
    for name in ["main", "master"] {
        if remote_tip(root, name).is_some() {
            names.push(name.to_owned());
        }
    }
    let mut confirmed = false;
    if let Ok(text) = run_timed(
        root,
        &["ls-remote", "--symref", REMOTE, "HEAD"],
        LS_REMOTE_TIMEOUT,
    ) {
        for name in text.lines().filter_map(|line| {
            line.strip_prefix("ref: refs/heads/")
                .and_then(|rest| rest.split_whitespace().next())
                .map(str::to_owned)
        }) {
            confirmed = true;
            names.push(name);
        }
    }
    names.sort();
    names.dedup();
    Defaults { names, confirmed }
}

/// Whether pushing to `branch` might change origin's default branch. Fails closed when
/// origin cannot be asked (see [`Defaults::may_be_default`]). The move refuses such a push.
pub fn may_be_default_branch(cwd: &Path, branch: &str) -> Result<bool, String> {
    let root = toplevel(cwd)?;
    Ok(default_branches(&root).may_be_default(branch))
}

/// Why [`push_timed`] stopped because of `cancel`.
pub const PUSH_CANCELLED: &str = "The push was cancelled.";
/// A push may take this long (large repositories over a slow link), like the Host's push.
pub const PUSH_TIMEOUT: Duration = Duration::from_secs(300);

/// Runs `git push` with `args` (built and validated by `scm::push_commit_args`), ended after
/// `timeout` or when `cancel` is set; the whole process group is stopped then. Never prompts.
pub fn push_timed(
    cwd: &Path,
    args: &[String],
    timeout: Duration,
    cancel: &std::sync::atomic::AtomicBool,
) -> Result<(), String> {
    let root = toplevel(cwd)?;
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    // Hooks (LFS) and credential helpers may only be on the login-shell PATH.
    let path = crate::scm::shell::gui_search_path();
    match run_timed_with(&root, &args, timeout, Some(cancel), Some(&path)) {
        Ok(_) => Ok(()),
        Err(Timed::Expired) => Err(format!(
            "Pushing took longer than {} seconds",
            timeout.as_secs()
        )),
        Err(Timed::Stopped) => Err(PUSH_CANCELLED.into()),
        Err(Timed::Failed(e)) => Err(e),
    }
}

/// The commit `HEAD` points at, `None` when it cannot be read (no repository, no commits).
pub fn head_commit(cwd: &Path) -> Option<String> {
    let root = toplevel(cwd).ok()?;
    git_text(&root, &["rev-parse", "--verify", "HEAD"], None)
        .ok()
        .filter(|head| !head.is_empty())
}

/// The tip of `refs/heads/<branch>` on origin, asked with a read-only `ls-remote`
/// (10 s cap). `None` when the branch does not exist there. Used to see that a push already
/// went through, so a retried push step does not push again.
pub fn origin_branch_tip(cwd: &Path, branch: &str) -> Result<Option<String>, String> {
    let root = toplevel(cwd)?;
    let name = format!("refs/heads/{branch}");
    match run_timed(&root, &["ls-remote", REMOTE, &name], LS_REMOTE_TIMEOUT) {
        Ok(text) => Ok(text
            .lines()
            .filter_map(|line| line.split_once('\t'))
            .find(|(_, name_found)| *name_found == name)
            .map(|(sha, _)| sha.trim().to_owned())),
        Err(Timed::Expired) => Err(format!("Asking {REMOTE} took longer than 10 seconds")),
        Err(Timed::Stopped) => Err("Cancelled".into()),
        Err(Timed::Failed(e)) => Err(format!("Could not ask {REMOTE}: {e}")),
    }
}

// ---------------------------------------------------------------- step 2: snapshot

/// Reads the working copy at `cwd` and writes the patch into `staging`. The real index,
/// working tree, branches, stash and object store are only read.
pub fn snapshot(cwd: &Path, staging: &Path) -> Result<Snapshot, String> {
    snapshot_with_limit(cwd, staging, MAX_PATCH_BYTES)
}

fn snapshot_with_limit(cwd: &Path, staging: &Path, max_patch: u64) -> Result<Snapshot, String> {
    let root = toplevel(cwd)?;
    let head = git_text(&root, &["rev-parse", "--verify", "HEAD"], None)
        .map_err(|_| "This branch has no commits yet".to_string())?;
    std::fs::create_dir_all(staging).map_err(|e| e.to_string())?;
    // The scratch paths go into environment variables of git runs in another directory.
    let staging = staging.canonicalize().map_err(|e| e.to_string())?;
    let objects = git_text(&root, &["rev-parse", "--git-path", "objects"], None)?;
    let scratch = Scratch {
        index: staging.join("index.tmp"),
        objects: staging.join("objects.tmp"),
        real_objects: root.join(objects),
    };
    let result = precheck(&root, max_patch)
        .and_then(|()| snapshot_in(&root, &staging, &scratch, head, max_patch));
    let _ = std::fs::remove_file(&scratch.index);
    let _ = std::fs::remove_dir_all(&scratch.objects);
    result
}

fn too_big(bytes: u64, max: u64) -> String {
    format!(
        "The uncommitted changes are {} MB, over the {} MB limit",
        bytes / (1024 * 1024),
        max / (1024 * 1024)
    )
}

/// Refuses early when the untracked files alone already exceed the limit, so nothing huge
/// is hashed first. Tracked files are not counted: a one-line edit of a big file is a small
/// patch, and the size of the finished patch is checked after it is written (into the
/// scratch object store, never the real one).
fn precheck(root: &Path, max: u64) -> Result<(), String> {
    let mut total = 0u64;
    for path in run(
        root,
        &["ls-files", "-z", "--others", "--exclude-standard"],
        None,
        None,
    )?
    .split(|b| *b == 0)
    .filter(|p| !p.is_empty())
    {
        let path = root.join(String::from_utf8_lossy(path).as_ref());
        total += std::fs::symlink_metadata(path).map_or(0, |m| m.len());
        if total > max {
            return Err(too_big(total, max));
        }
    }
    Ok(())
}

fn snapshot_in(
    root: &Path,
    staging: &Path,
    scratch: &Scratch,
    head: String,
    max_patch: u64,
) -> Result<Snapshot, String> {
    let _ = std::fs::remove_dir_all(&scratch.objects);
    std::fs::create_dir_all(&scratch.objects).map_err(|e| e.to_string())?;
    seed_index(root, scratch, true)?;
    if run(root, &["add", "-A"], Some(scratch), None).is_err() {
        // A split or unreadable index cannot be borrowed; start from HEAD instead.
        seed_index(root, scratch, false)?;
        run(root, &["add", "-A"], Some(scratch), None)?;
    }
    let tree = git_text(root, &["write-tree"], Some(scratch))?;

    let status = run(
        root,
        &[
            "diff",
            "--cached",
            "--name-status",
            "--no-renames",
            "--ignore-submodules=none",
            "-z",
            "HEAD",
        ],
        Some(scratch),
        None,
    )?;
    let mut sizes = Sizes::default();
    let mut parts = status.split(|b| *b == 0).filter(|p| !p.is_empty());
    while let (Some(code), Some(_path)) = (parts.next(), parts.next()) {
        if code.first() == Some(&b'A') {
            sizes.added += 1;
        } else {
            sizes.changed += 1;
        }
    }

    let patch = staging.join("uncommitted.patch");
    let patch_path = if sizes.added + sizes.changed == 0 {
        let _ = std::fs::remove_file(&patch);
        None
    } else {
        let file = std::fs::File::create(&patch).map_err(|e| e.to_string())?;
        let made = run(
            root,
            &[
                "diff",
                "--cached",
                "--binary",
                "--full-index",
                "--no-renames",
                "--no-ext-diff",
                "--no-textconv",
                "--no-color",
                "--ignore-submodules=none",
                "-U3",
                "--src-prefix=a/",
                "--dst-prefix=b/",
                "HEAD",
            ],
            Some(scratch),
            Some(Stdio::from(file)),
        );
        if let Err(e) = made {
            let _ = std::fs::remove_file(&patch);
            return Err(e);
        }
        sizes.patch = std::fs::metadata(&patch).map_err(|e| e.to_string())?.len();
        if sizes.patch > max_patch {
            let _ = std::fs::remove_file(&patch);
            return Err(too_big(sizes.patch, max_patch));
        }
        Some(patch)
    };

    let ignored = ignored_files(root, scratch)?;
    sizes.ignored = ignored.iter().map(|(_, size)| size).sum();
    let warnings = Warnings {
        lfs: uses_lfs(root),
        dirty_submodules: dirty_submodules(root),
    };
    Ok(Snapshot {
        head,
        tree,
        patch_path,
        ignored,
        sizes,
        warnings,
    })
}

/// Fills the scratch index: a copy of the real one, or `HEAD`'s tree.
fn seed_index(root: &Path, scratch: &Scratch, copy: bool) -> Result<(), String> {
    let _ = std::fs::remove_file(&scratch.index);
    if copy {
        let real = git_text(root, &["rev-parse", "--git-path", "index"], None)?;
        let real = root.join(real);
        if std::fs::copy(&real, &scratch.index).is_ok() {
            return Ok(());
        }
    }
    run(root, &["read-tree", "HEAD"], Some(scratch), None).map(|_| ())
}

/// Ignored paths with sizes. Fully ignored directories are one entry ending in `/`.
fn ignored_files(root: &Path, scratch: &Scratch) -> Result<Vec<(String, u64)>, String> {
    let out = run(
        root,
        &[
            "ls-files",
            "-z",
            "--others",
            "--ignored",
            "--exclude-standard",
            "--directory",
        ],
        Some(scratch),
        None,
    )?;
    let mut files: Vec<(String, u64)> = out
        .split(|b| *b == 0)
        .filter(|p| !p.is_empty())
        .map(|p| {
            let rel = String::from_utf8_lossy(p).into_owned();
            let mut budget = SIZE_WALK_BUDGET;
            let size = path_size(&root.join(rel.trim_end_matches('/')), &mut budget);
            (rel, size)
        })
        .collect();
    files.sort();
    Ok(files)
}

fn path_size(path: &Path, budget: &mut usize) -> u64 {
    let Ok(meta) = std::fs::symlink_metadata(path) else {
        return 0;
    };
    if meta.is_file() {
        return meta.len();
    }
    if !meta.is_dir() || *budget == 0 {
        return 0;
    }
    let Ok(entries) = std::fs::read_dir(path) else {
        return 0;
    };
    let mut total = 0;
    for entry in entries.flatten() {
        *budget = budget.saturating_sub(1);
        total += path_size(&entry.path(), budget);
    }
    total
}

fn uses_lfs(root: &Path) -> bool {
    let listed = run(
        root,
        &[
            "ls-files",
            "-z",
            "--cached",
            "--others",
            "--exclude-standard",
            "--",
            ":(glob,top).gitattributes",
            ":(glob)**/.gitattributes",
        ],
        None,
        None,
    )
    .unwrap_or_default();
    let mut candidates: Vec<PathBuf> = listed
        .split(|b| *b == 0)
        .filter(|p| !p.is_empty())
        .map(|p| root.join(String::from_utf8_lossy(p).as_ref()))
        .collect();
    if let Ok(info) = git_text(root, &["rev-parse", "--git-path", "info/attributes"], None) {
        candidates.push(root.join(info));
    }
    candidates.iter().any(|path| {
        std::fs::read_to_string(path)
            .is_ok_and(|text| text.lines().any(|line| line.contains("filter=lfs")))
    })
}

fn dirty_submodules(root: &Path) -> Vec<String> {
    let Ok(staged) = run(root, &["ls-files", "-z", "--stage"], None, None) else {
        return Vec::new();
    };
    let specs: Vec<String> = staged
        .split(|b| *b == 0)
        .filter(|e| e.starts_with(b"160000 "))
        .filter_map(|e| e.iter().position(|b| *b == b'\t').map(|i| &e[i + 1..]))
        .map(|p| format!(":(literal){}", String::from_utf8_lossy(p)))
        .collect();
    if specs.is_empty() {
        return Vec::new();
    }
    let mut args = vec![
        "status",
        "--porcelain",
        "-z",
        "--ignore-submodules=none",
        "--",
    ];
    args.extend(specs.iter().map(String::as_str));
    let Ok(out) = run(root, &args, None, None) else {
        return Vec::new();
    };
    let mut dirty: Vec<String> = out
        .split(|b| *b == 0)
        .filter(|e| e.len() > 3)
        .map(|e| String::from_utf8_lossy(&e[3..]).into_owned())
        .collect();
    dirty.sort();
    dirty.dedup();
    dirty
}

// ---------------------------------------------------------------- step 2: transcript

/// Copies the Claude transcript files of `provider_id` from `account_home` into
/// `staging/transcript`: `projects/<dir>/<id>.jsonl`, `projects/<dir>/<id>/` and
/// `file-history/<id>/`, the set `accounts/transfer.rs` carries between accounts. Only
/// regular files are copied.
pub fn collect_transcript(
    account_home: &Path,
    cwd: &Path,
    provider_id: &str,
    staging: &Path,
) -> Result<Transcript, String> {
    if provider_id.is_empty()
        || provider_id.len() > 128
        || !provider_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return Err("Invalid Claude session id".into());
    }
    let name = format!("{provider_id}.jsonl");
    let projects = account_home.join("projects");
    let preferred = projects.join(encode_cwd(cwd));
    let holds = |dir: &Path| is_real_dir(dir) && is_regular(&dir.join(&name));
    let project = if holds(&preferred) {
        preferred
    } else {
        std::fs::read_dir(&projects)
            .ok()
            .and_then(|dirs| {
                let mut dirs: Vec<PathBuf> = dirs.flatten().map(|d| d.path()).collect();
                dirs.sort();
                dirs.into_iter().find(|d| holds(d))
            })
            .ok_or("Could not find the Claude conversation to move")?
    };
    let source = project.join(&name);
    let jsonl_bytes = std::fs::metadata(&source).map_err(|e| e.to_string())?.len();
    if jsonl_bytes > MAX_JSONL_BYTES {
        return Err(format!(
            "The conversation is {} MB, over the {} MB limit",
            jsonl_bytes / (1024 * 1024),
            MAX_JSONL_BYTES / (1024 * 1024)
        ));
    }
    let dir_name = project
        .file_name()
        .ok_or("Invalid Claude project directory")?;
    let root = staging.join("transcript");
    let _ = std::fs::remove_dir_all(&root);
    let dest_project = root.join("projects").join(dir_name);
    std::fs::create_dir_all(&dest_project).map_err(|e| e.to_string())?;
    let mut total = copy_tree(&project.join(provider_id), &dest_project.join(provider_id))?;
    total += copy_tree(
        &account_home.join("file-history").join(provider_id),
        &root.join("file-history").join(provider_id),
    )?;
    // The transcript last, as in `transfer_claude`: it is what `--resume` looks for.
    let jsonl = dest_project.join(&name);
    total += std::fs::copy(&source, &jsonl).map_err(|e| e.to_string())?;
    Ok(Transcript {
        root,
        jsonl,
        jsonl_bytes,
        total_bytes: total,
    })
}

/// Claude names a project folder after the cwd with every non-alphanumeric as `-`.
fn encode_cwd(cwd: &Path) -> String {
    cwd.to_string_lossy()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

/// A regular file, not a symlink to one.
fn is_regular(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|m| m.is_file())
}

/// A directory, not a symlink to one.
fn is_real_dir(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|m| m.is_dir())
}

fn copy_tree(source: &Path, dest: &Path) -> Result<u64, String> {
    if !is_real_dir(source) {
        return Ok(0);
    }
    let entries = match std::fs::read_dir(source) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(e) => return Err(e.to_string()),
    };
    std::fs::create_dir_all(dest).map_err(|e| e.to_string())?;
    let mut total = 0;
    for entry in entries {
        let entry = entry.map_err(|e| e.to_string())?;
        let kind = entry.file_type().map_err(|e| e.to_string())?;
        let target = dest.join(entry.file_name());
        if kind.is_dir() {
            total += copy_tree(&entry.path(), &target)?;
        } else if kind.is_file() {
            total += std::fs::copy(entry.path(), &target).map_err(|e| e.to_string())?;
        }
    }
    Ok(total)
}

#[cfg(test)]
pub(super) fn fetch_with_timeout_for_test(cwd: &Path, millis: u64) -> Result<(), String> {
    fetch_with_timeout(cwd, Duration::from_millis(millis))
}

#[cfg(test)]
pub(super) fn snapshot_with_limit_for_test(
    cwd: &Path,
    staging: &Path,
    max_patch: u64,
) -> Result<Snapshot, String> {
    snapshot_with_limit(cwd, staging, max_patch)
}
