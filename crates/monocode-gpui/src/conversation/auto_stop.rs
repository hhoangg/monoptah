//! What a conversation tells the cloud auto-stop policy (`shell/cloud_auto_stop.rs`): is it
//! doing anything, does it hold something that stopping its workspace would cut off, and when
//! did the user last touch it. In-memory reads only.
use super::*;
use monocode_core::cloud_sessions::auto_stop::SessionFacts;
use std::time::Instant;

/// Why a send is refused while the shell stops this session's workspace (cost control). Nothing
/// is lost: the typed text stays in the composer and the next send resumes the workspace.
pub(crate) const AUTO_STOPPING: &str = "Monoptah is stopping this workspace after it sat idle. \
     Nothing was sent; send again in a moment and it will resume.";

impl Conversation {
    /// The user typed (an edit of the composer, not any notify of it), sent or clicked Resume in
    /// this conversation just now.
    pub(super) fn note_user_input(&mut self) {
        self.last_user_input = Some(Instant::now());
    }

    /// Test support: the conversation finished loading, so it is not "pending" for the policy.
    #[cfg(test)]
    pub(crate) fn loaded_for_test(&mut self) {
        self.loaded = true;
        self.loading = false;
    }

    /// The facts the policy needs, on the clock of `epoch` (seconds since it).
    pub(crate) fn auto_stop_facts(&self, epoch: Instant) -> SessionFacts {
        let busy = self.busy
            || self.stopping
            || !self.background.tasks.is_empty()
            || self.orchestration_progress.is_some();
        let pending = self.loading
            || !self.loaded
            || !self.history_actions.is_empty()
            || self.cloud_locked
            || self.cloud_launch_pending()
            || self.cloud_start_blocks()
            || self.cloud_queued.is_some()
            || self.remote_state.pending
            || self.remote_state.waking
            || self.remote_state.app_prompt_waiting
            || !self.follow_ups.state.messages.is_empty();
        SessionFacts {
            id: self.summary.id,
            busy,
            pending,
            unknown: false,
            last_input: self
                .last_user_input
                .map(|at| at.saturating_duration_since(epoch).as_secs()),
        }
    }
}
