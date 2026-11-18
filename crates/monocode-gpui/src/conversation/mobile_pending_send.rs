//! The one pending send a session keeps for a device that woke its instance (H-10).
//!
//! A web device that confirmed the wake of a stopped cloud instance sends its message at
//! once; the message waits in the history deferral (`defer_history_action`) until the reload
//! after the wake flushes it. Until then a second wake-send is refused (`busy`).
//! This is a busy flag, not a queue: it holds the message of nobody. It is cleared by the
//! flush of the deferred wake-send itself, when the wake fails, or after
//! [`monocode_core::mobile::cloud_policy::WAKE_SLOT_TIMEOUT`] so a lost flush cannot refuse
//! sends for good. Merely submitting the message does not clear it.
//!
//! Each flag carries a token. The deferred send holds the token it was queued with and runs
//! only while that token is still the live one, so a wake that failed or timed out cancels
//! exactly its own message and nothing else that waits for the history (S7).
//!
//! The flags live in memory of the UI thread (every caller is on it), keyed by session id.
use super::*;
use monocode_core::mobile::cloud_policy::WAKE_SLOT_TIMEOUT;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::time::{Duration, Instant};

/// How long a send that may not wake a cloud workspace waits for the history of a session
/// that was not loaded (it only waits for the load, never for a wake). A send that is still
/// waiting after this is dropped, so it cannot go out hours later when somebody reconnects.
pub(super) const NO_WAKE_DEFER: Duration = Duration::from_secs(2 * 60);

thread_local! {
    static PENDING: RefCell<HashMap<i64, (Instant, u64)>> = RefCell::new(HashMap::new());
    static NEXT_TOKEN: Cell<u64> = const { Cell::new(1) };
}

/// How a remote device's send may proceed.
#[derive(Clone, Copy)]
pub(super) struct SendGate {
    /// The device confirmed waking the instance.
    pub wake: bool,
    /// The pending wake-send this message is, when it is one.
    pub token: Option<u64>,
    /// A message that may not wake anything is dropped after this.
    pub deadline: Option<Instant>,
}

impl SendGate {
    pub(super) fn no_wake() -> Self {
        Self {
            wake: false,
            token: None,
            deadline: None,
        }
    }

    pub(super) fn wake(id: i64) -> Self {
        Self {
            wake: true,
            token: live_token(id),
            deadline: None,
        }
    }
}

/// Marks a wake-send of session `id` as pending. False when one already is.
fn begin(id: i64) -> bool {
    PENDING.with(|pending| {
        let mut pending = pending.borrow_mut();
        if pending.get(&id).is_some_and(|(since, _)| is_live(*since)) {
            return false;
        }
        let token = NEXT_TOKEN.with(|next| {
            let token = next.get();
            next.set(token + 1);
            token
        });
        pending.insert(id, (Instant::now(), token));
        true
    })
}

/// Clears the pending wake-send of session `id`.
fn end(id: i64) {
    PENDING.with(|pending| {
        pending.borrow_mut().remove(&id);
    });
}

fn is_live(since: Instant) -> bool {
    since.elapsed() < WAKE_SLOT_TIMEOUT
}

fn live_token(id: i64) -> Option<u64> {
    PENDING.with(|pending| {
        pending
            .borrow()
            .get(&id)
            .filter(|(since, _)| is_live(*since))
            .map(|(_, token)| *token)
    })
}

/// Whether the wake-send `token` of session `id` is still the pending one.
pub(super) fn wake_send_current(id: i64, token: u64) -> bool {
    live_token(id) == Some(token)
}

/// The deferred wake-send `token` runs now: clears its flag. False when it was dropped,
/// replaced or timed out, and so must not go out.
pub(super) fn finish_wake_send(id: i64, token: u64) -> bool {
    let current = wake_send_current(id, token);
    if current {
        end(id);
    }
    current
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
    pub(crate) fn wake_send_pending(id: i64) -> bool {
        live_token(id).is_some()
    }

    /// Forgets every flag, for tests that share the thread.
    #[cfg(test)]
    pub(crate) fn reset_wake_sends() {
        reset()
    }

    /// Called at the start of every remote-device send: a message from a remote device is the
    /// user's own act (it moves the idle stop of a cloud instance away, where merely watching
    /// never does).
    pub(super) fn mobile_send_begins(&mut self) {
        self.note_user_input();
    }

    /// Whether the live view is connected to its cloud workspace: `None` while its history is
    /// not loaded (it knows nothing yet). A loaded view that has no live link is not connected,
    /// whatever the cache says (it is stale after an idle stop or a sandbox TTL). Reads cached
    /// view state; it never wakes or reconnects.
    pub(crate) fn mobile_cloud_connected(&self) -> Option<bool> {
        self.history_loaded()
            .then_some(self.remote_state.online && !self.remote_state.waking)
    }

    /// Test support: the prompts this view sent to its workspace.
    #[cfg(test)]
    pub(crate) fn launched_for_test(&self) -> Vec<String> {
        self.launched.clone()
    }

    /// Test support: runs what waited for the history, as the reload after a wake does.
    #[cfg(test)]
    pub(crate) fn flush_history_for_test(&mut self, cx: &mut Context<Self>) {
        self.run_history_actions(cx);
    }

    /// Test support: the desktop composer holds text.
    #[cfg(test)]
    pub(crate) fn input_has_text_for_test(&self, cx: &gpui::App) -> bool {
        !self.input.read(cx).content.is_empty()
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
    /// somebody reconnects by hand and the deferral flushes. Only that message is dropped:
    /// its flag goes, and the deferred send runs only while its flag lives. Whatever else
    /// waits for the history (the desktop's own or a Mono's action) is left alone.
    pub(crate) fn drop_deferred_sends(&mut self) {
        end(self.summary.id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// S6/S7: the flag belongs to one wake-send. Its flush ends it once; a dropped,
    /// replaced or finished send cannot run later.
    #[test]
    fn a_wake_send_runs_only_while_its_own_flag_lives() {
        reset();
        assert!(begin(7));
        assert!(!begin(7), "one pending send per session");
        let first = live_token(7).unwrap();
        assert!(wake_send_current(7, first));
        assert!(finish_wake_send(7, first), "the flush runs it");
        assert!(!finish_wake_send(7, first), "and only once");
        assert!(begin(7));
        let second = live_token(7).unwrap();
        assert_ne!(first, second);
        assert!(
            !wake_send_current(7, first),
            "an older send is not the pending one"
        );
        // A failed wake drops its message: the flag goes, the deferred send must not run.
        end(7);
        assert!(!finish_wake_send(7, second));
        assert!(begin(7), "the session is free again");
        // Other sessions are unaffected.
        assert!(begin(8));
        end(7);
        assert!(live_token(8).is_some());
        reset();
    }

    #[test]
    fn the_gate_of_a_wake_send_carries_its_token_and_a_plain_one_none() {
        reset();
        assert!(begin(3));
        let gate = SendGate::wake(3);
        assert!(gate.wake && gate.token == live_token(3) && gate.deadline.is_none());
        let plain = SendGate::no_wake();
        assert!(!plain.wake && plain.token.is_none());
        reset();
    }
}
