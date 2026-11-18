//! The one pending send a session keeps for a device that woke its instance (H-10).
//!
//! A web device that confirmed the wake of a stopped cloud instance sends its message at
//! once; the message waits in the history deferral (`defer_history_action`) until the reload
//! after the wake flushes it. Until then a second send of any device is refused (`busy`).
//! This is a busy flag, not a queue: it holds the message of nobody. It is cleared when the
//! deferred send runs, when the wake fails, or after [`monocode_core::mobile::cloud_policy::WAKE_SLOT_TIMEOUT`]
//! so a lost flush cannot refuse sends for good.
//!
//! The flags live in memory of the UI thread (every caller is on it), keyed by session id.
use super::*;
use monocode_core::mobile::cloud_policy::WAKE_SLOT_TIMEOUT;
use std::cell::RefCell;
use std::collections::HashMap;
use std::time::Instant;

thread_local! {
    static PENDING: RefCell<HashMap<i64, Instant>> = RefCell::new(HashMap::new());
}

/// Marks a wake-send of session `id` as pending. False when one already is.
fn begin(id: i64) -> bool {
    PENDING.with(|pending| {
        let mut pending = pending.borrow_mut();
        if is_live(&pending, id) {
            return false;
        }
        pending.insert(id, Instant::now());
        true
    })
}

/// Clears the pending wake-send of session `id`.
fn end(id: i64) {
    PENDING.with(|pending| {
        pending.borrow_mut().remove(&id);
    });
}

/// Whether a wake-send of session `id` waits for its flush.
#[cfg(test)]
fn is_pending(id: i64) -> bool {
    PENDING.with(|pending| is_live(&pending.borrow(), id))
}

fn is_live(pending: &HashMap<i64, Instant>, id: i64) -> bool {
    pending
        .get(&id)
        .is_some_and(|since| since.elapsed() < WAKE_SLOT_TIMEOUT)
}

/// Forgets every flag, for tests that share the thread.
#[cfg(test)]
fn reset() {
    PENDING.with(|pending| pending.borrow_mut().clear());
}

impl Conversation {
    /// Marks a wake-send of session `id` as pending. False when one already is.
    pub(crate) fn begin_wake_send(id: i64) -> bool {
        begin(id)
    }

    /// Clears the pending wake-send of session `id` (the wake failed or ended).
    pub(crate) fn end_wake_send(id: i64) {
        end(id)
    }

    /// Whether a wake-send of session `id` waits for its flush.
    #[cfg(test)]
    pub(crate) fn wake_send_pending(id: i64) -> bool {
        is_pending(id)
    }

    /// Forgets every flag, for tests that share the thread.
    #[cfg(test)]
    pub(crate) fn reset_wake_sends() {
        reset()
    }

    /// Called at the start of every remote-device send: a message from a remote device is the
    /// user's own act (it moves the idle stop of a cloud instance away, where merely watching
    /// never does), and once the history is ready the send is the flush of the deferral, so
    /// the pending flag ends.
    pub(super) fn mobile_send_begins(&mut self) {
        self.note_user_input();
        if self.history_ready() {
            end(self.summary.id);
        }
    }

    /// The instance of this cloud session is being woken now.
    pub(crate) fn is_waking(&self) -> bool {
        self.remote_state.waking
    }

    /// The wake ended and left a connection error.
    pub(crate) fn wake_failed(&self) -> bool {
        !self.remote_state.waking && self.remote_connection_failed()
    }

    /// A wake ended without a connection: the send it carried must not go out later, when
    /// somebody reconnects by hand and the deferral flushes. Everything that waited for the
    /// history of this cloud session is dropped with it (nothing could have run).
    pub(crate) fn drop_deferred_sends(&mut self) {
        self.history_actions.clear();
        end(self.summary.id);
    }
}
