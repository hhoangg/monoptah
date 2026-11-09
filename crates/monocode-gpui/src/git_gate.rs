//! One switch for every git write while a cloud session is in view.
//!
//! The File and Git panels show the cloud workspace while a cloud session is
//! in view (the panels are bound to its `remote://` root), so the changes they
//! list are the agent's. Until remote git writes are confirmed user actions,
//! the Changes panel, the review view and the file editor's diff all ask here
//! and refuse to stage, discard or commit.
//! The shell sets it from the active session on the UI thread, where every
//! caller also runs.
use std::cell::RefCell;

thread_local! {
    static CODESPACE: RefCell<Option<String>> = const { RefCell::new(None) };
}

/// Why a write was refused.
pub(crate) const REFUSED: &str = "Local git changes are disabled while a cloud session is in view: these files are not the session's.";

/// Records the codespace of the session in view (`None` when it is a local session).
/// Returns whether the value changed.
pub(crate) fn set(codespace: Option<String>) -> bool {
    CODESPACE.with(|c| {
        let mut current = c.borrow_mut();
        let changed = *current != codespace;
        *current = codespace;
        changed
    })
}

/// The codespace whose session is in view, if any.
#[cfg(test)]
pub(crate) fn codespace() -> Option<String> {
    CODESPACE.with(|c| c.borrow().clone())
}

/// Whether stage, unstage, discard and commit may touch the local checkout.
pub(crate) fn writes_allowed() -> bool {
    CODESPACE.with(|c| c.borrow().is_none())
}
