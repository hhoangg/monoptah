//! Bringing the local default branch (`main`) up to date after a cloud backup, without ever
//! asking the user and without ever touching work.
//!
//! The backup itself does not depend on the local default branch: the saved commits travel in
//! a bundle and origin is fetched into a private ref namespace (see `convert::fetch_origin`).
//! Still, a local `main` that fell behind `origin/main` makes the restored code look odd, and
//! the cure is a fetch and a fast-forward. That is all this does:
//!
//! Every git call goes through `convert::run_git`: `GIT_TERMINAL_PROMPT=0`, ssh in batch mode
//! (so a credential or host-key prompt fails instead of waiting) and a 120 second limit after
//! which the process is killed. Offline or unauthenticated, the refresh is silently skipped.
//!
//! * fetch the default branch from origin (the same fetch the user would run);
//! * local branch behind origin and a fast-forward is possible: fast-forward it, but only
//!   through git's own refusals (`merge --ff-only` in a clean checkout, or a `fetch` without
//!   `+` for a branch that is not checked out). Nothing is reset, rebased or forced;
//! * anything else (ahead, diverged, a dirty checkout, the branch checked out elsewhere) is
//!   left exactly as it is, and the returned note says so. It is a notice, never a dialog.
use super::convert::run_git;
use std::path::Path;

fn git(project: &Path, args: &[&str]) -> Option<String> {
    run_git(project, args)
        .ok()
        .map(|out| out.trim().to_string())
}

/// The default branch name of `origin`, from `origin/HEAD` or else `main` / `master`.
fn default_branch(project: &Path) -> Option<String> {
    if let Some(head) = git(
        project,
        &["symbolic-ref", "-q", "--short", "refs/remotes/origin/HEAD"],
    ) {
        if let Some(name) = head.strip_prefix("origin/") {
            return Some(name.to_string());
        }
    }
    ["main", "master"].into_iter().find_map(|name| {
        git(
            project,
            &[
                "rev-parse",
                "--verify",
                "--quiet",
                &format!("refs/remotes/origin/{name}"),
            ],
        )
        .map(|_| name.to_string())
    })
}

/// Fetches and, when that is safe, fast-forwards the local default branch of `project`.
/// Returns a note for the user when something is worth saying, `None` when the branch was
/// already current or there is nothing to do (no git, no origin, no local branch).
/// BLOCKS (network); call from a worker.
pub fn refresh_default_branch(project: &Path) -> Option<String> {
    git(project, &["remote", "get-url", "origin"])?;
    let name = default_branch(project)?;
    let remote_ref = format!("refs/remotes/origin/{name}");
    // A plain fetch of one branch, as the user would run it. Offline or a failure: leave
    // everything as it is and say nothing, the backup does not need it.
    run_git(project, &["fetch", "-q", "--no-tags", "origin", &name]).ok()?;
    let local_ref = format!("refs/heads/{name}");
    let local = git(project, &["rev-parse", "--verify", "--quiet", &local_ref])?;
    let remote = git(project, &["rev-parse", "--verify", "--quiet", &remote_ref])?;
    if local == remote {
        return None;
    }
    let behind = run_git(project, &["merge-base", "--is-ancestor", &local, &remote]).is_ok();
    if !behind {
        let ahead = run_git(project, &["merge-base", "--is-ancestor", &remote, &local]).is_ok();
        return Some(if ahead {
            format!(
                "Local {name} has commits that origin/{name} does not have; it was left as it is."
            )
        } else {
            format!(
                "Local {name} and origin/{name} have diverged; local {name} was left untouched. \
                 The backup does not depend on it."
            )
        });
    }
    let checked_out_here =
        git(project, &["symbolic-ref", "-q", "--short", "HEAD"]).as_deref() == Some(name.as_str());
    let advanced = if checked_out_here {
        // Tracked changes would be in the way of the merge: leave a dirty checkout alone.
        let clean = git(project, &["status", "--porcelain", "--untracked-files=no"])
            .is_some_and(|status| status.is_empty());
        clean && run_git(project, &["merge", "--ff-only", "--quiet", &remote_ref]).is_ok()
    } else {
        // Without a leading `+`, git refuses anything but a fast-forward and refuses a
        // branch that is checked out in another worktree.
        run_git(
            project,
            &[
                "fetch",
                "-q",
                "--no-tags",
                "origin",
                &format!("{name}:{name}"),
            ],
        )
        .is_ok()
    };
    Some(if advanced {
        format!("Local {name} was fast-forwarded to origin/{name}.")
    } else {
        format!(
            "Local {name} is behind origin/{name} but was not updated (uncommitted changes or a \
             checkout elsewhere). Pull it when convenient; the backup does not depend on it."
        )
    })
}

#[cfg(test)]
mod tests;
