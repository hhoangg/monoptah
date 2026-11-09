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
const REMOTE: &str = "origin";
/// Bound on the entries visited to size one ignored directory.
const SIZE_WALK_BUDGET: usize = 200_000;

/// Config that would otherwise change the shape of the patch or of what is staged.
const NEUTRAL: [&str; 8] = [
    "-c",
    "diff.noprefix=false",
    "-c",
    "diff.mnemonicPrefix=false",
    "-c",
    "core.fsmonitor=false",
    "-c",
    "core.quotepath=false",
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

fn run(
    root: &Path,
    args: &[&str],
    index: Option<&Path>,
    stdout: Option<Stdio>,
) -> Result<Vec<u8>, String> {
    let mut cmd = git_cmd();
    cmd.args(NEUTRAL)
        .arg("--no-pager")
        .arg("-C")
        .arg(root)
        .args(args)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE");
    if let Some(index) = index {
        cmd.env("GIT_INDEX_FILE", index);
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

fn git_text(root: &Path, args: &[&str], index: Option<&Path>) -> Result<String, String> {
    let out = run(root, args, index, None)?;
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

/// `git fetch origin`, killed after 60 s, never prompting for credentials. This updates
/// only the remote-tracking refs.
pub fn fetch_origin(cwd: &Path) -> Result<(), String> {
    fetch_with_timeout(cwd, FETCH_TIMEOUT)
}

fn fetch_with_timeout(cwd: &Path, timeout: Duration) -> Result<(), String> {
    let root = toplevel(cwd)?;
    let mut stderr = tempfile::tempfile().map_err(|e| e.to_string())?;
    let mut cmd = git_cmd();
    cmd.args(["-c", "gc.auto=0", "-c", "maintenance.auto=false"])
        .arg("-C")
        .arg(&root)
        .args(["fetch", "--prune", "--no-tags", REMOTE])
        .env("GIT_TERMINAL_PROMPT", "0")
        .env_remove("GIT_DIR")
        .env_remove("GIT_INDEX_FILE")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(stderr.try_clone().map_err(|e| e.to_string())?);
    let mut child = cmd.spawn().map_err(|e| format!("Could not run Git: {e}"))?;
    let started = Instant::now();
    let status = loop {
        match child.try_wait().map_err(|e| e.to_string())? {
            Some(status) => break status,
            None if started.elapsed() >= timeout => {
                crate::process::terminate(&mut child);
                let _ = child.wait();
                return Err(format!(
                    "Fetching {REMOTE} took longer than {} seconds",
                    timeout.as_secs()
                ));
            }
            None => std::thread::sleep(Duration::from_millis(25)),
        }
    };
    if status.success() {
        return Ok(());
    }
    let mut text = String::new();
    let _ = std::io::Seek::rewind(&mut stderr);
    let _ = stderr.read_to_string(&mut text);
    Err(format!("Could not fetch {REMOTE}: {}", text.trim()))
}

// ---------------------------------------------------------------- step 2: push target

/// Chooses the branch to push, from the remote-tracking refs as they are now (call
/// [`fetch_origin`] first). Never a force push and never origin's default branch.
pub fn push_target(cwd: &Path) -> Result<PushTarget, String> {
    let root = toplevel(cwd)?;
    let branch = git_text(&root, &["symbolic-ref", "--short", "-q", "HEAD"], None)
        .ok()
        .filter(|b| !b.is_empty())
        .ok_or("HEAD is detached. Switch to a branch before continuing in the cloud.")?;
    let head = git_text(&root, &["rev-parse", "--verify", "HEAD"], None)
        .map_err(|_| "This branch has no commits yet".to_string())?;
    if git_text(&root, &["remote", "get-url", REMOTE], None).is_err() {
        return Err(format!("This project has no {REMOTE} remote to push to"));
    }
    let defaults = default_branches(&root);
    let upstream = upstream_on_origin(&root, &branch);

    // The branch an ordinary push would use, if the rules allow it.
    let plain = upstream.clone().unwrap_or_else(|| branch.clone());
    if !defaults.contains(&branch) && !defaults.contains(&plain) {
        match remote_tip(&root, &plain) {
            None => {
                return Ok(PushTarget {
                    branch: plain,
                    needs_push: true,
                    creates_branch: true,
                })
            }
            Some(tip) if tip == head => {
                return Ok(PushTarget {
                    branch: plain,
                    needs_push: false,
                    creates_branch: false,
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

/// Origin's default branch from `refs/remotes/origin/HEAD`. When the clone has no such
/// ref, `main` and `master` count as default if they exist on origin.
fn default_branches(root: &Path) -> Vec<String> {
    let head = format!("refs/remotes/{REMOTE}/HEAD");
    if let Ok(target) = git_text(root, &["symbolic-ref", "-q", &head], None) {
        let prefix = format!("refs/remotes/{REMOTE}/");
        if let Some(name) = target.strip_prefix(&prefix) {
            return vec![name.to_owned()];
        }
    }
    ["main", "master"]
        .into_iter()
        .filter(|name| remote_tip(root, name).is_some())
        .map(str::to_owned)
        .collect()
}

// ---------------------------------------------------------------- step 2: snapshot

/// Reads the working copy at `cwd` and writes the patch into `staging`. The real index,
/// working tree, branches and stash are only read.
pub fn snapshot(cwd: &Path, staging: &Path) -> Result<Snapshot, String> {
    let root = toplevel(cwd)?;
    let head = git_text(&root, &["rev-parse", "--verify", "HEAD"], None)
        .map_err(|_| "This branch has no commits yet".to_string())?;
    std::fs::create_dir_all(staging).map_err(|e| e.to_string())?;
    let index = staging.join("index.tmp");
    let result = snapshot_with_index(&root, staging, &index, head);
    let _ = std::fs::remove_file(&index);
    result
}

fn snapshot_with_index(
    root: &Path,
    staging: &Path,
    index: &Path,
    head: String,
) -> Result<Snapshot, String> {
    seed_index(root, index, true)?;
    if run(root, &["add", "-A"], Some(index), None).is_err() {
        // A split or unreadable index cannot be borrowed; start from HEAD instead.
        seed_index(root, index, false)?;
        run(root, &["add", "-A"], Some(index), None)?;
    }
    let tree = git_text(root, &["write-tree"], Some(index))?;

    let status = run(
        root,
        &[
            "diff",
            "--cached",
            "--name-status",
            "--no-renames",
            "-z",
            "HEAD",
        ],
        Some(index),
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
                "--src-prefix=a/",
                "--dst-prefix=b/",
                "HEAD",
            ],
            Some(index),
            Some(Stdio::from(file)),
        );
        if let Err(e) = made {
            let _ = std::fs::remove_file(&patch);
            return Err(e);
        }
        sizes.patch = std::fs::metadata(&patch).map_err(|e| e.to_string())?.len();
        if sizes.patch > MAX_PATCH_BYTES {
            let _ = std::fs::remove_file(&patch);
            return Err(format!(
                "The uncommitted changes are {} MB, over the {} MB limit",
                sizes.patch / (1024 * 1024),
                MAX_PATCH_BYTES / (1024 * 1024)
            ));
        }
        Some(patch)
    };

    let ignored = ignored_files(root, index)?;
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

/// Fills the temporary index: a copy of the real one, or `HEAD`'s tree.
fn seed_index(root: &Path, index: &Path, copy: bool) -> Result<(), String> {
    let _ = std::fs::remove_file(index);
    if copy {
        let real = git_text(root, &["rev-parse", "--git-path", "index"], None)?;
        let real = root.join(real);
        if std::fs::copy(&real, index).is_ok() {
            return Ok(());
        }
    }
    run(root, &["read-tree", "HEAD"], Some(index), None).map(|_| ())
}

/// Ignored paths with sizes. Fully ignored directories are one entry ending in `/`.
fn ignored_files(root: &Path, index: &Path) -> Result<Vec<(String, u64)>, String> {
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
        Some(index),
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
    let project = if preferred.join(&name).is_file() {
        preferred
    } else {
        std::fs::read_dir(&projects)
            .ok()
            .and_then(|dirs| {
                let mut dirs: Vec<PathBuf> = dirs.flatten().map(|d| d.path()).collect();
                dirs.sort();
                dirs.into_iter().find(|d| d.join(&name).is_file())
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

fn copy_tree(source: &Path, dest: &Path) -> Result<u64, String> {
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
