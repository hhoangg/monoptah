//! One switch for every git write while a cloud session is in view, and the policy for the
//! writes that go to the cloud workspace.
//!
//! While a cloud session is in view the File and Git panels show its cloud workspace by default
//! (the panels are bound to its `remote://` root, see `panel_root`), and its git writes are
//! confirmed user actions under [`policy`]. When the user switches the panels to Local, they show
//! the local checkout, which is not the session's: the Changes panel, the review view and the file
//! editor's diff all ask here and refuse to stage, discard or commit there.
//! Every question carries the cwd the caller works on, so the answer follows the path, not the
//! panel in view. The shell sets the state each frame on the UI thread, where every caller runs.
use crate::panel_root::PanelRoot;
use monocode_core::{remote::parse_remote_path, store::is_pending_cloud_environment};
use std::cell::RefCell;

/// What the shell knows about the sessions, as far as git writes care.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Gate {
    /// Cache key of the cloud session's workspace in view; `None` for a local session.
    cloud: Option<String>,
    /// A cloud session is in view, so writes to a local checkout are refused: those files are
    /// not the session's, whichever side the panels show right now.
    local_refused: bool,
    /// Host environments where some session has a turn running (see [`turn_running`]).
    running: Vec<String>,
}

impl Gate {
    pub(crate) fn of(root: &PanelRoot, running: Vec<String>) -> Self {
        Self {
            cloud: root
                .cloud()
                .map(monocode_core::cloud_sessions::CloudRef::key),
            local_refused: root.cloud().is_some(),
            running,
        }
    }
    /// A cloud session is in view (tests).
    #[cfg(test)]
    pub(crate) fn local_of(cloud_key: &str) -> Self {
        Self {
            cloud: Some(cloud_key.into()),
            local_refused: true,
            ..Self::default()
        }
    }
    /// A turn runs in each of the Host environments `envs` (tests).
    #[cfg(test)]
    pub(crate) fn running_in(envs: &[&str]) -> Self {
        Self {
            running: envs.iter().map(|env| (*env).to_owned()).collect(),
            ..Self::default()
        }
    }
}

thread_local! {
    static STATE: RefCell<Gate> = RefCell::new(Gate::default());
}

/// The Host environment of a `remote://` path.
fn environment(cwd: &str) -> Option<String> {
    parse_remote_path(cwd).map(|path| path.environment_id)
}

/// Why a write was refused.
pub(crate) const REFUSED: &str = "Git changes are off while the panels show the local checkout of a cloud session: those files are not the session's. Switch the panels to Cloud to change its workspace.";

/// Records the panels in view. Returns whether the value changed.
pub(crate) fn set(gate: Gate) -> bool {
    STATE.with(|state| {
        let mut current = state.borrow_mut();
        let changed = *current != gate;
        *current = gate;
        changed
    })
}

/// The workspace key of the cloud session in view, if any.
#[cfg(test)]
pub(crate) fn codespace() -> Option<String> {
    STATE.with(|state| state.borrow().cloud.clone())
}

/// Whether stage, unstage, discard and commit may run at all in `cwd`. A cloud workspace is
/// allowed under [`policy`], unless it is still being created. A local checkout is refused while
/// a cloud session is in view: those files are not the session's. The answer depends on the cwd
/// the caller works on, not on which panel is visible, so a view opened earlier stays right when
/// the panels switch.
pub(crate) fn writes_allowed(cwd: &str) -> bool {
    match environment(cwd) {
        Some(env) => !is_pending_cloud_environment(&env),
        None => STATE.with(|state| !state.borrow().local_refused),
    }
}

/// Whether a turn runs in the workspace of `cwd`. This is the app-side check for the policy
/// below.
///
/// It is racy and best effort until the Host refuses by itself (F2-5h, capability
/// `git.idleGuard`). The answer is the app's last view of the sessions, refreshed every frame.
/// A turn can start while a confirmation dialog is open, or between the check and the request
/// reaching the Host, so callers ask again when the dialog is answered.
pub(crate) fn turn_running(cwd: &str) -> bool {
    let Some(env) = environment(cwd) else {
        return false;
    };
    STATE.with(|state| state.borrow().running.contains(&env))
}

/// A git write the panels can ask for. The table is complete on purpose: the operations that are
/// always allowed (unstage, push, pull, sync, PR) are named so the policy is tested whole, though
/// no panel asks about them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(dead_code)]
pub(crate) enum Op {
    Stage,
    Unstage,
    Discard,
    Commit,
    /// Checkout, create branch, stash and worktrees: they change the agent's working directory.
    ChangeBranch,
    Push,
    Pull,
    Sync,
    CreatePr,
}

/// What to do before a git write.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Decision {
    /// Go ahead. Discard, push and the other writes keep the confirmations they already have.
    Allow,
    /// Ask first; `ok` labels the button.
    Confirm {
        message: &'static str,
        ok: &'static str,
    },
    /// Do not run; say why.
    Refuse(&'static str),
}

/// The remote git policy (design 2.2.5). It only applies to a cloud workspace (`remote`); a
/// local checkout keeps today's behaviour. `running` says a turn runs in the workspace; the
/// check is the app's best effort, see [`turn_running`].
pub(crate) fn policy(op: Op, remote: bool, running: bool) -> Decision {
    if !remote {
        return Decision::Allow;
    }
    match (op, running) {
        (Op::Discard, true) => Decision::Refuse(
            "The agent is working in this workspace and may be editing these files. Wait for it to finish before discarding.",
        ),
        (Op::Commit, true) => Decision::Confirm {
            message: "The agent may commit or change the index at the same time. Commit anyway?",
            ok: "Commit",
        },
        (Op::ChangeBranch, true) => Decision::Refuse(
            "The agent is working in this workspace. Wait for it to finish before changing branches, stashing or managing worktrees.",
        ),
        (Op::ChangeBranch, false) => Decision::Confirm {
            message: "This changes the working directory the agent works in. Continue?",
            ok: "Continue",
        },
        // Reads, staging, and push, pull, sync and PR (explicit user actions that use the
        // remote's own credentials) are allowed, with the confirmations they have today.
        _ => Decision::Allow,
    }
}

/// [`policy`] for a write in `cwd`: remote when it is a `remote://` path.
pub(crate) fn decide(op: Op, cwd: &str) -> Decision {
    policy(op, environment(cwd).is_some(), turn_running(cwd))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discard_is_refused_while_a_turn_runs_and_otherwise_keeps_its_own_confirmation() {
        assert!(matches!(
            policy(Op::Discard, true, true),
            Decision::Refuse(_)
        ));
        assert_eq!(policy(Op::Discard, true, false), Decision::Allow);
        // A local checkout is never subject to the remote policy.
        assert_eq!(policy(Op::Discard, false, true), Decision::Allow);
    }

    #[test]
    fn commit_asks_only_while_a_turn_runs() {
        assert!(matches!(
            policy(Op::Commit, true, true),
            Decision::Confirm { ok: "Commit", .. }
        ));
        assert_eq!(policy(Op::Commit, true, false), Decision::Allow);
    }

    #[test]
    fn branch_changes_need_an_idle_workspace_and_a_confirmation() {
        assert!(matches!(
            policy(Op::ChangeBranch, true, true),
            Decision::Refuse(_)
        ));
        assert!(matches!(
            policy(Op::ChangeBranch, true, false),
            Decision::Confirm { ok: "Continue", .. }
        ));
        assert_eq!(policy(Op::ChangeBranch, false, false), Decision::Allow);
    }

    #[test]
    fn staging_and_the_network_operations_are_allowed_even_while_a_turn_runs() {
        for op in [
            Op::Stage,
            Op::Unstage,
            Op::Push,
            Op::Pull,
            Op::Sync,
            Op::CreatePr,
        ] {
            assert_eq!(policy(op, true, true), Decision::Allow, "{op:?}");
            assert_eq!(policy(op, true, false), Decision::Allow, "{op:?}");
        }
    }

    const REMOTE: &str = "remote://env-octo-abc/workspaces/widgets";

    #[test]
    fn the_gate_decides_per_cwd_from_what_the_shell_set() {
        set(Gate::running_in(&["env-octo-abc"]));
        assert!(writes_allowed(REMOTE));
        assert!(turn_running(REMOTE));
        assert!(matches!(decide(Op::Discard, REMOTE), Decision::Refuse(_)));
        // Another workspace, and a local checkout, are not affected by that turn.
        assert!(!turn_running("remote://env-other/workspaces/w"));
        assert_eq!(decide(Op::Discard, "/work/local"), Decision::Allow);
        set(Gate::running_in(&[]));
        assert_eq!(decide(Op::Discard, REMOTE), Decision::Allow);
        set(Gate::default());
        assert!(writes_allowed("/work/local"));
    }

    #[test]
    fn a_local_cwd_is_refused_while_a_cloud_session_is_in_view_whatever_the_panel_shows() {
        set(Gate::local_of("octo-abc"));
        assert!(!writes_allowed("/work/local"));
        // A remote cwd gets the remote policy even then.
        assert!(writes_allowed(REMOTE));
        set(Gate::default());
        assert!(writes_allowed("/work/local"));
    }

    #[test]
    fn a_remote_cwd_gets_the_remote_policy_while_a_local_session_is_active() {
        set(Gate::running_in(&["env-octo-abc"]));
        assert!(writes_allowed(REMOTE));
        assert!(matches!(decide(Op::Discard, REMOTE), Decision::Refuse(_)));
        assert!(matches!(
            decide(Op::ChangeBranch, REMOTE),
            Decision::Refuse(_)
        ));
        set(Gate::default());
    }

    #[test]
    fn a_pending_workspace_is_never_written() {
        assert!(!writes_allowed(
            "remote://pending-octo-abc/workspaces/widgets"
        ));
    }
}
