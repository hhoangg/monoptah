//! Read-only helpers for the "Continue in cloud" dialog (design 4.2.2 dialog, 4.5).
//!
//! * [`preview`] answers "what will move" BEFORE the user confirms: the branch that would be
//!   pushed, the counts, and the ignored files that stay local. It fetches origin (a read) and
//!   builds the same snapshot the move builds, in a temporary folder that is removed again. It
//!   never pushes, uploads or contacts a workspace, so it is safe to run when the dialog opens.
//! * [`MoveMark`] is the small record the flip leaves behind (when and where the session
//!   moved) so the transcript can draw its divider after an app restart.
use super::backend::CloudBackendKind;
use super::move_out;
use crate::store::Store;
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Store setting prefix of the divider record: `cloud-move-mark:<session id>`.
pub const MARK_PREFIX: &str = "cloud-move-mark:";

/// What the dialog lists as "what will move". Holds no file content.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MovePreview {
    /// The branch on origin that would receive the commits.
    pub branch: String,
    /// HEAD is not on origin yet. `false` means "nothing to push".
    pub needs_push: bool,
    /// The push would create `branch` (a `-cloud` branch when the plain name cannot be used).
    pub creates_branch: bool,
    /// The local branch name, to say when `branch` differs from it.
    pub local_branch: String,
    /// Commits the push sends.
    pub commits: usize,
    /// Tracked files with uncommitted changes.
    pub changed: usize,
    /// New files that are not ignored.
    pub added: usize,
    /// Size of the patch that carries the uncommitted state.
    pub patch_bytes: u64,
    /// Ignored files and folders (folders end with `/`) with their size. They stay local.
    pub ignored: Vec<(String, u64)>,
    pub ignored_bytes: u64,
    /// A `.gitattributes` routes files through `filter=lfs`.
    pub lfs: bool,
    /// Submodules with changes in their checkout; their content does not travel.
    pub dirty_submodules: Vec<String>,
}

/// The message `begin_move` gives for a session whose worktree is gone: the preview refuses
/// with the same words.
pub const NO_WORKTREE: &str = "Select a branch or worktree to continue this conversation.";

use std::sync::{Condvar, LazyLock, Mutex};

/// The project folders being inspected right now, with a condvar to wait for one to free up.
static INSPECTING: LazyLock<(Mutex<std::collections::HashSet<String>>, Condvar)> =
    LazyLock::new(Default::default);

/// Holds the per-project lock until dropped.
pub(super) struct ProjectGuard(String);

impl Drop for ProjectGuard {
    fn drop(&mut self) {
        let (set, freed) = &*INSPECTING;
        set.lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&self.0);
        freed.notify_all();
    }
}

/// One git inspection of a project folder at a time (the preview and the move's own snapshot
/// both fetch origin and write staging). Blocks until the folder is free.
pub(super) fn project_guard(cwd: &Path) -> ProjectGuard {
    let key = std::fs::canonicalize(cwd)
        .unwrap_or_else(|_| cwd.to_path_buf())
        .to_string_lossy()
        .into_owned();
    let (set, freed) = &*INSPECTING;
    let mut held = set.lock().unwrap_or_else(|e| e.into_inner());
    while held.contains(&key) {
        held = freed.wait(held).unwrap_or_else(|e| e.into_inner());
    }
    held.insert(key.clone());
    ProjectGuard(key)
}

/// Inspects the working copy at `cwd`. Blocking (a `git fetch`, up to 60 s): run it on a
/// dedicated thread. Nothing in the repository is written and nothing is pushed. Setting
/// `cancel` stops the fetch (killing git and its children) and returns `Err("Cancelled")`.
pub fn preview(cwd: &Path, cancel: &std::sync::atomic::AtomicBool) -> Result<MovePreview, String> {
    use std::sync::atomic::Ordering;
    let stopped = || cancel.load(Ordering::Relaxed);
    let _inspect = project_guard(cwd);
    if stopped() {
        return Err("Cancelled".into());
    }
    move_out::fetch_origin_cancellable(cwd, cancel)?;
    let target = move_out::push_target(cwd)?;
    if stopped() {
        return Err("Cancelled".into());
    }
    let staging = tempfile::tempdir().map_err(|e| e.to_string())?;
    let snapshot = move_out::snapshot(cwd, staging.path())?;
    let commits = if target.needs_push {
        commits_to_push(cwd, &target.branch)
    } else {
        0
    };
    Ok(MovePreview {
        local_branch: current_branch(cwd).unwrap_or_default(),
        branch: target.branch,
        needs_push: target.needs_push,
        creates_branch: target.creates_branch,
        commits,
        changed: snapshot.sizes.changed,
        added: snapshot.sizes.added,
        patch_bytes: snapshot.sizes.patch,
        ignored_bytes: snapshot.sizes.ignored,
        ignored: snapshot.ignored,
        lfs: snapshot.warnings.lfs,
        dirty_submodules: snapshot.warnings.dirty_submodules,
    })
}

fn git_out(cwd: &Path, args: &[&str]) -> Option<String> {
    let out = crate::scm::git_cmd()
        .arg("--no-pager")
        .arg("-C")
        .arg(cwd)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env_remove("GIT_DIR")
        .env_remove("GIT_INDEX_FILE")
        .stdin(std::process::Stdio::null())
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

fn current_branch(cwd: &Path) -> Option<String> {
    git_out(cwd, &["symbolic-ref", "-q", "--short", "HEAD"]).filter(|b| !b.is_empty())
}

/// Commits that `HEAD` has and `origin/<branch>` has not; when the branch is new on origin,
/// the commits no remote-tracking branch of origin has.
fn commits_to_push(cwd: &Path, branch: &str) -> usize {
    let tracking = format!("refs/remotes/origin/{branch}");
    let count = if git_out(cwd, &["rev-parse", "--verify", "--quiet", &tracking]).is_some() {
        git_out(
            cwd,
            &["rev-list", "--count", &format!("origin/{branch}..HEAD")],
        )
    } else {
        git_out(
            cwd,
            &["rev-list", "--count", "HEAD", "--not", "--remotes=origin"],
        )
    };
    count.and_then(|c| c.parse().ok()).unwrap_or(0)
}

// --------------------------------------------------------------------- the mark

/// When and where a session moved to the cloud. Written by the flip, read by the transcript
/// divider ("Continued in the cloud"). Holds no secret.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MoveMark {
    /// Unix milliseconds of the flip.
    pub at_ms: u64,
    pub backend: CloudBackendKind,
    /// The codespace or sandbox name.
    pub target: String,
}

impl MoveMark {
    /// `codespace foo`, `sandbox bar`.
    pub fn target_label(&self) -> String {
        if self.target.is_empty() {
            self.backend.noun().to_owned()
        } else {
            format!("{} {}", self.backend.noun(), self.target)
        }
    }
}

pub(super) fn save_mark(store: &Store, session: i64, mark: &MoveMark) -> Result<(), String> {
    store.save_setting(
        &format!("{MARK_PREFIX}{session}"),
        &serde_json::to_string(mark).map_err(|e| e.to_string())?,
    )
}

/// The divider record of a session that moved, `None` when there is none (an older move, or a
/// session that never moved).
pub fn move_mark(store: &Store, session: i64) -> Option<MoveMark> {
    serde_json::from_str(&store.setting(&format!("{MARK_PREFIX}{session}")).ok()??).ok()
}

#[cfg(test)]
pub(super) fn save_mark_for_test(store: &Store, session: i64, mark: &MoveMark) {
    save_mark(store, session, mark).unwrap();
}
