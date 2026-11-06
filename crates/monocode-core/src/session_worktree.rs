//! What the app API says about a session's worktree, and removing that worktree
//! together with the session (`sessions.delete` with `removeWorktree`).
//!
//! A worktree counts as the session's own only when the app recorded it for the
//! session (its agent settings) and Git confirms a linked worktree inside the
//! `<project>-worktrees` folder the app creates them in. Remote, cloud and
//! codespace sessions are never touched.
use crate::{
    files::{is_equal_or_inside, path_key},
    git::git,
    store::Store,
    worktree_management::{default_root, status},
};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

/// The branch the app makes for a session's worktree. Only such a branch is
/// ever deleted with it: any other branch was chosen by the user.
const SESSION_BRANCH: &str = "mc/native-";

/// The worktree path to report for a session, or `None` when it works in the
/// main checkout. `dir` is where the session works (its recorded worktree, else
/// its folder). Cheap on purpose, no Git is run: a linked worktree has a `.git`
/// file where the main checkout has a directory.
pub fn recorded_worktree(cwd: &str, dir: &str) -> Option<String> {
    if dir.is_empty() || path_key(dir) == path_key(cwd) {
        return None;
    }
    let path = Path::new(dir);
    if crate::files::remote::is_remote(path) {
        return None;
    }
    (path.join(".git").is_file() || !path.exists()).then(|| dir.to_owned())
}

/// `(dirty, unpushed)` of a worktree, `None` where it could not be told (the
/// path is gone or Git failed). Dirty counts uncommitted changes and untracked
/// files that are not ignored; unpushed counts commits on HEAD that no
/// remote-tracking branch has.
pub fn inspect(path: &Path) -> (Option<bool>, Option<bool>) {
    if !path.is_dir() {
        return (None, None);
    }
    let dirty = git(path, &["status", "--porcelain", "--untracked-files=normal"])
        .ok()
        .map(|out| !out.trim().is_empty());
    let unpushed = git(path, &["rev-list", "--count", "HEAD", "--not", "--remotes"])
        .ok()
        .and_then(|out| out.trim().parse::<u64>().ok())
        .map(|count| count > 0);
    (dirty, unpushed)
}

/// A removal that passed every check.
#[derive(Clone, Debug)]
pub struct Plan {
    pub path: PathBuf,
    pub main: PathBuf,
    pub branch: Option<String>,
    pub base: Option<String>,
}

/// Checks that session `id`'s worktree can go with it. Nothing is changed.
pub fn plan(store: &Store, id: i64) -> Result<Plan, String> {
    if store.remote_session(id)?.is_some() {
        return Err(
            "A session on another machine or in the cloud has no local worktree to remove".into(),
        );
    }
    let settings = store.agent_settings(id)?.unwrap_or_default();
    let recorded = settings
        .worktree_cwd
        .as_deref()
        .filter(|path| !path.is_empty() && !settings.worktree_removed)
        .ok_or("This session has no worktree made by the app")?;
    let path = Path::new(recorded);
    if crate::files::remote::is_remote(path) {
        return Err("A worktree on another machine is never removed from here".into());
    }
    if !path.is_dir() {
        return Err("The session's worktree folder is already gone".into());
    }
    let canonical = path.canonicalize().map_err(|e| e.to_string())?;
    let trees = crate::git::snapshot(&canonical)
        .map_err(|e| format!("The worktree is not a Git checkout: {e}"))?
        .worktrees;
    let main = trees
        .first()
        .ok_or("No main working copy found")?
        .path
        .clone();
    let main_path = Path::new(&main)
        .canonicalize()
        .unwrap_or_else(|_| PathBuf::from(&main));
    let tree = trees
        .iter()
        .find(|t| {
            Path::new(&t.path).canonicalize().is_ok_and(|p| {
                path_key(&p.to_string_lossy()) == path_key(&canonical.to_string_lossy())
            })
        })
        .ok_or("This path is not a registered worktree of its repository")?;
    if tree.main || canonical == main_path || !canonical.join(".git").is_file() {
        return Err("This session works in the main checkout, which is never removed".into());
    }
    let root = default_root(&main_path);
    if !is_equal_or_inside(&canonical.to_string_lossy(), &root.to_string_lossy())
        || path_key(&canonical.to_string_lossy()) == path_key(&root.to_string_lossy())
    {
        return Err("This worktree was not made by the app".into());
    }
    if status(&main_path, tree).0 {
        return Err("This worktree is locked in Git".into());
    }
    if in_use(store, &canonical, Some(id))? {
        return Err("Another session still works in this worktree".into());
    }
    match inspect(&canonical) {
        (Some(false), Some(false)) => (),
        (Some(true), _) => return Err("The worktree has uncommitted or untracked changes".into()),
        (_, Some(true)) => {
            return Err("The worktree has commits that are on no remote branch".into())
        }
        _ => return Err("Could not tell whether the worktree holds unsaved work".into()),
    }
    Ok(Plan {
        path: canonical,
        main: main_path,
        branch: tree.branch.clone(),
        base: settings.worktree_base.clone(),
    })
}

fn in_use(store: &Store, path: &Path, ignoring: Option<i64>) -> Result<bool, String> {
    let key = path.to_string_lossy();
    Ok(store.working_dirs()?.iter().any(|(other, dir)| {
        let real = Path::new(dir).canonicalize();
        Some(*other) != ignoring
            && (is_equal_or_inside(dir, &key)
                || real.is_ok_and(|real| is_equal_or_inside(&real.to_string_lossy(), &key)))
    }))
}

/// What a removal did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Outcome {
    pub worktree_removed: bool,
    /// Why the worktree stayed, when it did.
    pub worktree_error: Option<String>,
    pub branch: Option<String>,
    pub branch_deleted: bool,
    pub branch_reason: String,
}

impl Outcome {
    fn kept(plan: &Plan, reason: String) -> Self {
        Self {
            worktree_removed: false,
            worktree_error: Some(reason),
            branch: plan.branch.clone(),
            branch_deleted: false,
            branch_reason: "The worktree was kept, so its branch stays with it".into(),
        }
    }
    pub fn json(&self, path: &Path) -> Value {
        let mut value = json!({
            "worktreeRemoved": self.worktree_removed,
            "worktreePath": path.to_string_lossy(),
            "branch": self.branch,
            "branchDeleted": self.branch_deleted,
            "branchReason": self.branch_reason,
        });
        if let Some(error) = &self.worktree_error {
            value["worktreeError"] = json!(error);
        }
        value
    }
}

/// Rechecks sharing after the session row has gone. A session created while
/// deletion was queued must also keep the checkout alive.
pub fn execute_if_unused(store: &Store, plan: &Plan) -> Outcome {
    match in_use(store, &plan.path, None) {
        Ok(false) => execute(plan),
        Ok(true) => Outcome::kept(plan, "Another session still works in this worktree".into()),
        Err(error) => Outcome::kept(plan, error),
    }
}

/// Removes the planned worktree, then its branch when nothing would be lost.
/// The work is checked again first: it ran some time before the session went.
pub fn execute(plan: &Plan) -> Outcome {
    let kept = |reason: String| Outcome::kept(plan, reason);
    if plan.path.exists() {
        // The checkout may have been locked, moved, or switched to another
        // branch while the session deletion was queued. Never bypass Git's
        // refusal by wiping such a checkout ourselves.
        let current = crate::git::snapshot(&plan.main).ok().and_then(|snapshot| {
            snapshot
                .worktrees
                .into_iter()
                .find(|tree| Path::new(&tree.path).canonicalize().ok().as_ref() == Some(&plan.path))
        });
        if !wipeable(plan)
            || plan.path.canonicalize().ok().as_ref() != Some(&plan.path)
            || current.as_ref().is_none_or(|tree| {
                tree.main || tree.branch != plan.branch || status(&plan.main, tree).0
            })
        {
            return kept(
                "The worktree registration, branch or lock changed, so it was kept".into(),
            );
        }
        match inspect(&plan.path) {
            (Some(false), Some(false)) => (),
            _ => {
                return kept("The worktree changed or could not be checked, so it was kept".into())
            }
        }
        crate::provider::discard_worktree(&plan.path);
        let removal = git(
            &plan.main,
            &[
                "worktree",
                "remove",
                "--force",
                "--",
                &plan.path.to_string_lossy(),
            ],
        );
        // Git leaves ignored and untracked folders (target, node_modules) it
        // could not delete; the folder is the app's own, so clear it.
        if plan.path.exists() && wipeable(plan) {
            let _ = std::fs::remove_dir_all(&plan.path);
        }
        if plan.path.exists() {
            return kept(match removal {
                Err(error) if !error.is_empty() => error,
                _ => "The worktree folder could not be deleted".into(),
            });
        }
    }
    let _ = git(&plan.main, &["worktree", "prune"]);
    let (branch_deleted, branch_reason) = drop_branch(plan);
    Outcome {
        worktree_removed: true,
        worktree_error: None,
        branch: plan.branch.clone(),
        branch_deleted,
        branch_reason,
    }
}

/// The folder may be wiped by hand only when it is plainly a worktree of the
/// app: inside the worktrees folder and neither the main checkout nor above it.
fn wipeable(plan: &Plan) -> bool {
    let path = plan.path.to_string_lossy();
    let main = plan.main.to_string_lossy();
    is_equal_or_inside(&path, &default_root(&plan.main).to_string_lossy())
        && path_key(&path) != path_key(&default_root(&plan.main).to_string_lossy())
        && !is_equal_or_inside(&main, &path)
}

fn drop_branch(plan: &Plan) -> (bool, String) {
    let Some(branch) = plan.branch.as_deref() else {
        return (false, "The worktree had no branch".into());
    };
    if !branch.starts_with(SESSION_BRANCH) {
        return (
            false,
            "The branch was not made for this session, so it stays".into(),
        );
    }
    let reference = format!("refs/heads/{branch}");
    let exclude = format!("--exclude={branch}");
    let mut args = vec![
        "rev-list",
        "--count",
        &reference,
        "--not",
        &exclude,
        "--branches",
        "--remotes",
    ];
    // Work already in the branch the worktree started from is not unique either.
    let base = plan.base.as_deref().and_then(|base| {
        git(
            &plan.main,
            &[
                "rev-parse",
                "--verify",
                "--end-of-options",
                &format!("{base}^{{commit}}"),
            ],
        )
        .ok()
        .map(|sha| sha.trim().to_owned())
    });
    if let Some(base) = &base {
        args.push(base);
    }
    let unique = match git(&plan.main, &args)
        .ok()
        .and_then(|out| out.trim().parse::<u64>().ok())
    {
        Some(count) => count,
        None => {
            return (
                false,
                "Could not tell whether the branch holds unique commits".into(),
            )
        }
    };
    if unique > 0 {
        return (
            false,
            format!("The branch has {unique} commit(s) that no other branch has, so it stays"),
        );
    }
    match git(&plan.main, &["branch", "-D", "--", branch]) {
        Ok(_) => (true, "The branch had no commits of its own".into()),
        Err(error) => (false, format!("Could not delete the branch: {error}")),
    }
}

#[cfg(test)]
mod tests;
