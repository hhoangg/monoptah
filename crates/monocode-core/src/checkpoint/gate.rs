//! Which rewind-like operations a session may run, given where it lives (design 4.5, 4.9 R2
//! and R6). Pure: callers pass the stored location and the first row the operation touches.
//!
//! Reading of the design, for a session whose location is Cloud with move point `m`
//! (`moved_at_row`):
//!
//! * Rows before `m` were written on this computer. Claude's `file-history` and the app's
//!   checkpoints for those turns describe files as they were before the move, so restoring or
//!   rewinding to them would touch the wrong paths, or nothing (R6). Every
//!   operation is disabled for them.
//! * Rows from `m` on belong to the cloud workspace, which is the only writer of the local
//!   rows and of the JSONL (R2). An operation that would rewrite local rows or the local
//!   JSONL (edit the last turn, fork) is disabled there too. An operation that only acts on
//!   files (restore) rewrites neither, so it stays allowed after the move point.
//! * A Local session is gated here only by [`gate_returned`]: when it came back through a new
//!   worktree or a new Claude id, the turns up to that return are disabled.
use crate::store::SessionLocation;

/// A rewind-like operation of the app.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Operation {
    /// Replace the last user message and re-run from it.
    EditLastTurn,
    /// Start a new conversation from an earlier point.
    Fork,
    /// Put the files of a turn back from the app's checkpoints.
    RestoreFiles,
}

impl Operation {
    /// Whether it would rewrite local rows or the local provider JSONL.
    pub fn rewrites_local_history(self) -> bool {
        matches!(self, Self::EditLastTurn | Self::Fork)
    }
}

/// Why an operation on a turn before the move point is disabled.
pub const BEFORE_MOVE: &str = "This turn ran on this computer before the conversation moved to the cloud. Its file history belongs to the other side of the move, so it cannot be rewound here.";

/// Why an operation that rewrites the history is disabled while the session is in the cloud.
pub const CLOUD_OWNED: &str = "This conversation is in the cloud. Its messages are written from the cloud workspace, so they cannot be edited or forked from this computer.";

/// Why an operation on a turn from before a return through a new worktree or a new Claude id
/// is disabled.
pub const BEFORE_RETURN: &str = "This turn ran in another folder or under another Claude session than this conversation uses now. Its file history describes that other place, so it cannot be rewound here.";

/// [`gate_at`] for a session that came back from the cloud through a new worktree or a new
/// Claude id (`Store::returned_gate`): turns before `before` are disabled for every
/// operation, later ones are ordinary local turns. `None` (including an in-place return,
/// where the folder is the same) gates nothing.
pub fn gate_returned(before: Option<usize>, row: usize) -> Gate {
    match before {
        Some(before) if row < before => Gate::Disabled(BEFORE_RETURN),
        _ => Gate::Allowed,
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Gate {
    Allowed,
    /// Disabled, with the sentence the UI shows next to the control.
    Disabled(&'static str),
}

impl Gate {
    pub fn is_allowed(&self) -> bool {
        matches!(self, Self::Allowed)
    }
    pub fn reason(&self) -> Option<&'static str> {
        match self {
            Self::Allowed => None,
            Self::Disabled(reason) => Some(reason),
        }
    }
}

/// Whether `operation` may start at local row `row` of a session at `location`.
pub fn rewind_gate(location: &SessionLocation, operation: Operation, row: usize) -> Gate {
    gate_at(location.cloud().map(|c| c.moved_at_row), operation, row)
}

/// [`rewind_gate`] for a caller that only knows the move point: `Some(moved_at_row)` while
/// the session is in the cloud, `None` while it is local.
pub fn gate_at(moved_at_row: Option<usize>, operation: Operation, row: usize) -> Gate {
    let Some(moved_at_row) = moved_at_row else {
        return Gate::Allowed;
    };
    if row < moved_at_row {
        Gate::Disabled(BEFORE_MOVE)
    } else if operation.rewrites_local_history() {
        Gate::Disabled(CLOUD_OWNED)
    } else {
        Gate::Allowed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cloud_sessions::CloudRef;
    use crate::store::CloudLocation;

    fn cloud_at(moved_at_row: usize) -> SessionLocation {
        SessionLocation::Cloud(CloudLocation {
            cloud: CloudRef::codespace("octo-abc"),
            environment_id: "env".into(),
            project_id: "project".into(),
            host_session_id: "host-1".into(),
            remote_cwd: "/workspaces/repo".into(),
            local_cwd: "/Users/me/repo".into(),
            moved_at_row,
            pushed_branch: None,
        })
    }

    #[test]
    fn turns_before_a_return_through_a_new_place_are_disabled() {
        assert!(gate_returned(None, 0).is_allowed());
        assert_eq!(gate_returned(Some(5), 4).reason(), Some(BEFORE_RETURN));
        assert!(gate_returned(Some(5), 5).is_allowed());
    }

    #[test]
    fn a_local_session_is_never_gated() {
        for op in [
            Operation::EditLastTurn,
            Operation::Fork,
            Operation::RestoreFiles,
        ] {
            assert_eq!(rewind_gate(&SessionLocation::Local, op, 0), Gate::Allowed);
        }
    }

    #[test]
    fn rows_before_the_move_point_are_disabled_for_every_operation() {
        // 10 rows were written locally; the move happened at row 10.
        let location = cloud_at(10);
        for op in [
            Operation::EditLastTurn,
            Operation::Fork,
            Operation::RestoreFiles,
        ] {
            for row in [0, 4, 9] {
                assert_eq!(
                    rewind_gate(&location, op, row),
                    Gate::Disabled(BEFORE_MOVE),
                    "{op:?} at row {row}"
                );
            }
        }
    }

    #[test]
    fn after_the_move_point_only_operations_that_keep_the_history_are_allowed() {
        let location = cloud_at(10);
        assert_eq!(
            rewind_gate(&location, Operation::RestoreFiles, 10),
            Gate::Allowed
        );
        assert_eq!(
            rewind_gate(&location, Operation::RestoreFiles, 14),
            Gate::Allowed
        );
        for op in [Operation::EditLastTurn, Operation::Fork] {
            assert_eq!(
                rewind_gate(&location, op, 10),
                Gate::Disabled(CLOUD_OWNED),
                "{op:?}"
            );
        }
    }

    #[test]
    fn the_move_point_form_agrees_with_the_location_form() {
        for row in [0, 9, 10, 11] {
            for op in [
                Operation::EditLastTurn,
                Operation::Fork,
                Operation::RestoreFiles,
            ] {
                assert_eq!(
                    gate_at(Some(10), op, row),
                    rewind_gate(&cloud_at(10), op, row)
                );
                assert_eq!(gate_at(None, op, row), Gate::Allowed);
            }
        }
    }

    #[test]
    fn the_reasons_explain_themselves_without_dashes() {
        for reason in [BEFORE_MOVE, CLOUD_OWNED] {
            assert!(reason.contains("cloud") || reason.contains("move"));
            assert!(!reason.contains('\u{2014}') && !reason.contains('\u{2013}'));
        }
    }
}
