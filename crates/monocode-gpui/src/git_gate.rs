//! One switch for every git write while a cloud session is in view, and the policy for the
//! writes that go to the cloud workspace.
//!
//! While a cloud session is in view the File and Git panels show its cloud workspace by default
//! (the panels are bound to its `remote://` root, see `panel_root`), and its git writes are
//! confirmed user actions under [`policy`]. When the user switches the panels to Local, they show
//! the local checkout, which is not the session's: the Changes panel, the review view and the file
//! editor's diff all ask here and refuse to stage, discard or commit there.
//! The shell sets it from the active session on the UI thread, where every caller also runs.
use crate::panel_root::PanelRoot;
use std::cell::RefCell;

/// What the shell knows about the panels, as far as git writes care.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Gate {
    /// Cache key of the cloud session's workspace in view; `None` for a local session.
    cloud: Option<String>,
    /// Writes are refused: the panels show the local checkout, or no workspace yet.
    refuses: bool,
    /// The panels show the cloud workspace, so writes go to the remote under [`policy`].
    remote: bool,
    /// A turn is running in the workspace (see [`turn_running`]).
    running: bool,
}

impl Gate {
    pub(crate) fn of(root: &PanelRoot, running: bool) -> Self {
        Self {
            cloud: root
                .cloud()
                .map(monocode_core::cloud_sessions::CloudRef::key),
            refuses: root.refuses_writes(),
            remote: root.is_cloud_workspace(),
            running,
        }
    }
    /// The panels show the local checkout of a cloud session (tests).
    #[cfg(test)]
    pub(crate) fn local_of(cloud_key: &str) -> Self {
        Self {
            cloud: Some(cloud_key.into()),
            refuses: true,
            ..Self::default()
        }
    }
    /// The panels show the cloud workspace `cloud_key` (tests).
    #[cfg(test)]
    pub(crate) fn remote_of(cloud_key: &str, running: bool) -> Self {
        Self {
            cloud: Some(cloud_key.into()),
            remote: true,
            running,
            ..Self::default()
        }
    }
}

thread_local! {
    static STATE: RefCell<Gate> = RefCell::new(Gate::default());
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

/// Whether stage, unstage, discard and commit may run at all. False only when the panels show
/// the local checkout of a cloud session (or its workspace is not created yet).
pub(crate) fn writes_allowed() -> bool {
    STATE.with(|state| !state.borrow().refuses)
}

/// Whether a turn runs in the workspace the panels show. This is the app-side check for the
/// policy below.
///
/// It is racy and best effort until the Host refuses by itself (F2-5h, capability
/// `git.idleGuard`): the answer is the app's last view of the sessions, so a turn can start
/// between this check and the request reaching the Host.
#[cfg(test)]
pub(crate) fn turn_running() -> bool {
    STATE.with(|state| state.borrow().running)
}

/// A git write the panels can ask for. The table is complete on purpose: the operations that are
/// always allowed (stage, unstage, push, pull, sync, PR) and `ChangeBranch` (no panel offers it
/// yet) are named so the policy is tested whole, and the first panel that offers one asks here.
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

/// [`policy`] for the panels in view.
pub(crate) fn decide(op: Op) -> Decision {
    STATE.with(|state| {
        let state = state.borrow();
        policy(op, state.remote, state.running)
    })
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

    #[test]
    fn the_gate_decides_from_what_the_shell_set() {
        set(Gate::remote_of("octo-abc", true));
        assert!(writes_allowed());
        assert!(turn_running());
        assert!(matches!(decide(Op::Discard), Decision::Refuse(_)));
        set(Gate::remote_of("octo-abc", false));
        assert_eq!(decide(Op::Discard), Decision::Allow);
        set(Gate::local_of("octo-abc"));
        assert!(!writes_allowed());
        assert_eq!(decide(Op::Discard), Decision::Allow);
        set(Gate::default());
        assert!(writes_allowed());
    }
}
