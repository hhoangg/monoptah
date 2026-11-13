//! Cost control: stops a cloud workspace nothing needs any more (see
//! `monocode_core::cloud_sessions::auto_stop` for the policy and its rules).
//!
//! Once a minute the shell copies what it already knows about each cloud workspace into
//! [`WorkspaceFacts`] and asks the policy. That read is memory only: no `gh`, no `sbx`. Only a
//! decision starts a worker, which re-reads the state once and stops through
//! `CloudSessions::stop_cloud`, the path every other Stop uses. The workspace then reads
//! Stopped, so the conversation offers Resume, and the user's next send wakes it as before.
//! Nothing here ever wakes a workspace, and nothing here deletes one.
use super::*;
use monocode_core::cloud_sessions::{
    CloudState,
    auto_stop::{
        CHECK_EVERY_SECS, Decision, SessionFacts, StopOutcome, WorkspaceFacts, stop_if_still_idle,
    },
};
use std::time::{Duration, Instant};

pub(super) fn clock(cx: &mut Context<Shell>) -> gpui::Task<()> {
    cx.spawn(async move |this, cx| {
        loop {
            cx.background_executor()
                .timer(Duration::from_secs(CHECK_EVERY_SECS))
                .await;
            if this.update(cx, |this, cx| this.check_auto_stop(cx)).is_err() {
                break;
            }
        }
    })
}

impl Shell {
    /// Seconds on the auto-stop clock (monotonic, from app start).
    pub(super) fn auto_stop_now(&self) -> u64 {
        self.auto_stop_epoch.elapsed().as_secs()
    }

    /// What the window knows about every workspace that has a live session in it. Memory only.
    pub(super) fn auto_stop_facts(&self, cx: &App) -> Vec<WorkspaceFacts> {
        let epoch: Instant = self.auto_stop_epoch;
        // Unsaved edits could be written to a workspace that is gone: hold everything.
        let unsaved = self.unsaved_files(cx);
        let dialog: Vec<String> = self
            .cloud_archive
            .as_ref()
            .map(|dialog| dialog.groups.iter().map(|g| g.cloud().key()).collect())
            .unwrap_or_default();
        let mut by_key: HashMap<String, WorkspaceFacts> = HashMap::new();
        let mut ids: Vec<i64> = self.cloud_marks.keys().copied().collect();
        ids.sort_unstable();
        for id in ids {
            let mark = &self.cloud_marks[&id];
            let key = mark.key();
            let workspace = by_key.entry(key.clone()).or_insert_with(|| WorkspaceFacts {
                cloud: mark.cloud(),
                state: self
                    .cloud_states
                    .get(&key)
                    .copied()
                    .unwrap_or(CloudState::Unknown),
                sessions: Vec::new(),
                app_working: unsaved
                    || self.cloud_workspaces_busy.contains(&key)
                    || self.cloud_return_cleanup.running.contains(&key)
                    || self.cloud_continue.dialog.is_some()
                    || dialog.contains(&key),
            });
            let Some(summary) = self.session_summaries.get(&id) else {
                workspace.sessions.push(SessionFacts {
                    id,
                    unknown: true,
                    ..Default::default()
                });
                continue;
            };
            // An archived session no longer needs its workspace.
            if summary.archived {
                continue;
            }
            let mut facts = match self.sessions.get(&id) {
                Some(view) => view.read(cx).auto_stop_facts(epoch),
                None => SessionFacts {
                    id,
                    ..Default::default()
                },
            };
            let stored = self.remote_marks.get(&id).copied().unwrap_or((false, false));
            facts.busy |= stored.0 || stored.1 || self.terminal_busy(id, cx);
            facts.pending |= self.cloud_busy.contains(&id)
                || self.cloud_moves.contains_key(&id)
                || self.queued_cloud_opens.has(id)
                || self.cloud_open_due == Some(id)
                || self.cloud_starts.get(&id).is_some_and(|start| {
                    start.phase() != crate::cloud_start_view::StartPhase::Failed
                })
                || self
                    .cloud_located
                    .get(&id)
                    .is_some_and(|l| l.sync_error.is_some() || l.unsynced || l.host_active);
            workspace.sessions.push(facts);
        }
        // A workspace whose sessions are all archived is not used any more: leave it out, so
        // the policy sees it vanish.
        let mut found: Vec<WorkspaceFacts> = by_key
            .into_values()
            .filter(|w| !w.sessions.is_empty())
            .collect();
        found.sort_by_key(|w| w.cloud.key());
        found
    }

    /// The once-a-minute check. A refresh in flight means the marks and states are about to
    /// change, so it waits for the next minute.
    pub(super) fn check_auto_stop(&mut self, cx: &mut Context<Self>) {
        if self.cloud_refreshing {
            return;
        }
        let prefs = self.workspace.preferences.cloud_auto_stop;
        let facts = self.auto_stop_facts(cx);
        let now = self.auto_stop_now();
        for decision in self.auto_stop.check(now, &prefs, &facts) {
            self.run_auto_stop(decision, cx);
        }
    }

    fn run_auto_stop(&mut self, decision: Decision, cx: &mut Context<Self>) {
        let key = decision.cloud.key();
        if !self.auto_stop_running.insert(key) {
            return;
        }
        let cloud = self.cloud.clone();
        let asked = decision.clone();
        let work = cx
            .background_executor()
            .spawn(async move { stop_if_still_idle(&cloud, &asked) });
        cx.spawn(async move |this, cx| {
            let outcome = work.await;
            let _ = this.update(cx, |this, cx| this.finish_auto_stop(decision, outcome, cx));
        })
        .detach();
    }

    pub(super) fn finish_auto_stop(
        &mut self,
        decision: Decision,
        outcome: StopOutcome,
        cx: &mut Context<Self>,
    ) {
        let key = decision.cloud.key();
        self.auto_stop_running.remove(&key);
        let now = self.auto_stop_now();
        self.auto_stop.finished(&decision, &outcome, now);
        match outcome {
            StopOutcome::Stopped => {
                self.cloud_states.insert(key, CloudState::Stopped);
                self.sync_cloud_stopped(cx);
                self.invalidate_chrome(cx);
                self.notice = Some(decision.notice());
                cx.notify();
            }
            // The cached state was behind: read it again.
            StopOutcome::NotRunning => self.refresh_cloud(cx),
            StopOutcome::InUse => {}
            StopOutcome::Failed(error) => crate::native_log::warn(&format!(
                "Auto-stop of {} {} failed: {error}",
                decision.cloud.noun(),
                decision.cloud.name
            )),
        }
    }
}
