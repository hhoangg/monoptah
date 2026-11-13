//! Cost control: stops a cloud workspace nothing needs any more (see
//! `monocode_core::cloud_sessions::auto_stop` for the policy and its rules).
//!
//! Once a minute the shell copies what it already knows about each cloud workspace into
//! [`WorkspaceFacts`] and asks the policy. That read is memory only: no `gh`, no `sbx`. Only a
//! decision starts a worker, which re-reads the state once and stops through
//! `CloudSessions::stop_cloud`, the path every other Stop uses. The workspace then reads
//! Stopped, so the conversation offers Resume, and the user's next send wakes it as before.
//! Nothing here ever wakes a workspace, and nothing here deletes one.
//!
//! The worker takes seconds, so for that long the shell holds the workspace the way an
//! archive does: its key is in `cloud_workspaces_busy` (archive, delete, continue and the
//! `cloud.*` actions refuse), the composers of its sessions are locked (`auto_stopping`: a send
//! or an app/Mono turn is refused with a clear message and the typed text stays), and the worker
//! gets the same busy snapshot `cloud.stop` gets. Open cloud terminals count as activity.
//!
//! The hold also covers what is opened *during* the stop: a conversation registered or marked
//! as cloud while the worker runs is locked at once (`sync_auto_stopping`), and a NEW session on
//! the workspace (pickers, `sessions.start` of an app or a Mono) is refused while its key is in
//! [`StoppingWorkspaces`].
//! A failed stop is shown in the window's error line, once per workspace until one works.
use super::*;
use monocode_core::cloud_sessions::{
    CloudRef, CloudState,
    auto_stop::{
        CHECK_EVERY_SECS, Decision, SessionFacts, StopOutcome, WorkspaceFacts, stop_if_still_idle,
    },
};
use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Why a NEW session on a workspace is refused while it is being stopped. Nothing was created.
pub(crate) const STOPPING_NEW_SESSION: &str = "Monoptah is stopping this workspace after it sat idle. \
     Nothing was started; try again in a moment and it will resume.";

/// The workspaces (`CloudRef::key`) whose automatic stop runs now. A shared handle: the shell
/// changes it on the UI thread and the pickers read it when the user clicks, so a click never
/// works from a copy taken earlier.
#[derive(Clone, Default)]
pub(crate) struct StoppingWorkspaces(Arc<Mutex<HashSet<String>>>);

impl StoppingWorkspaces {
    fn set(&self) -> std::sync::MutexGuard<'_, HashSet<String>> {
        // A poisoned set is still a valid set of keys.
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }
    pub(crate) fn contains(&self, key: &str) -> bool {
        self.set().contains(key)
    }
    pub(crate) fn insert(&self, key: String) -> bool {
        self.set().insert(key)
    }
    pub(crate) fn remove(&self, key: &str) -> bool {
        self.set().remove(key)
    }
    /// `Err` with the reason when `cloud` is being stopped.
    pub(crate) fn refuse_new_session(&self, cloud: &CloudRef) -> Result<(), String> {
        if self.contains(&cloud.key()) {
            Err(STOPPING_NEW_SESSION.into())
        } else {
            Ok(())
        }
    }
}

pub(super) fn clock(cx: &mut Context<Shell>) -> gpui::Task<()> {
    cx.spawn(async move |this, cx| {
        loop {
            cx.background_executor()
                .timer(Duration::from_secs(CHECK_EVERY_SECS))
                .await;
            if this
                .update(cx, |this, cx| this.check_auto_stop(cx))
                .is_err()
            {
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
        let terminals = self.open_cloud_terminals(cx);
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
                    || terminals.contains_key(&key)
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
            let stored = self
                .remote_marks
                .get(&id)
                .copied()
                .unwrap_or((false, false));
            facts.busy |= stored.0 || stored.1 || self.terminal_busy(id, cx);
            facts.pending |= self.cloud_busy.contains(&id)
                || self.cloud_moves.contains_key(&id)
                || self.queued_cloud_opens.has(id)
                || self.cloud_open_due == Some(id)
                || (self.cloud_starts.contains_key(&id) && !self.cloud_start_failed(id))
                || self
                    .cloud_located
                    .get(&id)
                    .is_some_and(|l| l.sync_error.is_some() || l.unsynced || l.host_active);
            workspace.sessions.push(facts);
        }
        // A terminal open on a workspace holds it even when every session on it is archived or
        // gone (a dev server, a build): it stays in the facts, as working, until it is closed.
        for (key, cloud) in &terminals {
            by_key
                .entry(key.clone())
                .or_insert_with(|| WorkspaceFacts {
                    cloud: cloud.clone(),
                    state: self
                        .cloud_states
                        .get(key)
                        .copied()
                        .unwrap_or(CloudState::Unknown),
                    sessions: Vec::new(),
                    app_working: true,
                })
                .app_working = true;
        }
        // A workspace whose sessions are all archived is not used any more: leave it out, so
        // the policy sees it vanish.
        let mut found: Vec<WorkspaceFacts> = by_key
            .into_values()
            .filter(|w| !w.sessions.is_empty() || terminals.contains_key(&w.cloud.key()))
            .collect();
        found.sort_by_key(|w| w.cloud.key());
        found
    }

    /// Cloud terminals that hold their workspace, by workspace key: still open (the process has
    /// not exited) and either opened by the user or typed in. The app opens a codespace's panel
    /// shell by itself when its dock is shown; one nobody touched is not use, or no codespace
    /// would ever be stopped. A dropped terminal, closed from the dock, no longer upgrades.
    /// A quiet process in a shell (a dev server) cannot be told from a forgotten prompt through
    /// ssh, so an open shell holds the workspace until it is closed.
    pub(super) fn open_cloud_terminals(&self, cx: &App) -> HashMap<String, CloudRef> {
        self.cloud_terminals
            .iter()
            .filter(|(_, terminal, explicit)| {
                terminal.upgrade().is_some_and(|terminal| {
                    let terminal = terminal.read(cx);
                    !terminal.exited && (*explicit || terminal.user_typed)
                })
            })
            .map(|(cloud, _, _)| (cloud.key(), cloud.clone()))
            .collect()
    }

    /// Remembers `terminal` as a shell on `cloud`, so the auto-stop counts it as activity.
    /// `explicit`: the user asked for it (session menu).
    pub(super) fn track_cloud_terminal(
        &mut self,
        cloud: CloudRef,
        terminal: &Entity<crate::terminal::Terminal>,
        explicit: bool,
    ) {
        self.cloud_terminals
            .retain(|(_, t, _)| t.upgrade().is_some());
        self.cloud_terminals
            .push((cloud, terminal.downgrade(), explicit));
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

    pub(super) fn run_auto_stop(&mut self, decision: Decision, cx: &mut Context<Self>) {
        let key = decision.cloud.key();
        // An archive, delete or continue holds the workspace: leave it, try again in a window.
        if self.auto_stop_running.contains(&key) || self.cloud_workspaces_busy.contains(&key) {
            let now = self.auto_stop_now();
            self.auto_stop.finished(&decision, &StopOutcome::InUse, now);
            return;
        }
        // What the worker may not see on its own: live turns and open terminals, read now, on
        // this thread, before the workspace is held.
        let mut busy = self.cloud_busy_snapshot(cx);
        busy.workspaces
            .extend(self.open_cloud_terminals(cx).into_keys());
        // From here until `finish_auto_stop` nothing may start on the workspace: other actions
        // on it refuse, and its conversations take no message (the typed text stays).
        self.auto_stop_running.insert(key.clone());
        self.cloud_workspaces_busy.insert(key);
        self.sync_auto_stopping(cx);
        let cloud = self.cloud.clone();
        let asked = decision.clone();
        let work = cx
            .background_executor()
            .spawn(async move { stop_if_still_idle(&cloud, &asked, &busy) });
        cx.spawn(async move |this, cx| {
            let outcome = work.await;
            let _ = this.update(cx, |this, cx| this.finish_auto_stop(decision, outcome, cx));
        })
        .detach();
    }

    /// Makes every open conversation agree with the stopping set: a conversation whose workspace
    /// is being stopped takes no message, every other one does. Called when a stop starts and
    /// ends, and whenever a conversation is registered or the cloud marks change, so a
    /// composer opened while the worker runs is locked like the ones that were open before.
    pub(super) fn sync_auto_stopping(&mut self, cx: &mut Context<Self>) {
        for (id, view) in &self.sessions {
            let locked = self
                .cloud_marks
                .get(id)
                .is_some_and(|mark| self.auto_stop_running.contains(&mark.key()));
            if view.read(cx).auto_stopping != locked {
                view.update(cx, |view, _| view.auto_stopping = locked);
            }
        }
    }

    /// Refuses a NEW session that would run on the workspace `target` names, while it is being
    /// stopped. `New` names nothing yet, so it is never refused.
    pub(crate) fn refuse_new_session_on(
        &self,
        target: &monocode_core::mono::cloud_api::CloudTarget,
    ) -> Result<(), String> {
        use monocode_core::mono::cloud_api::InstanceChoice;
        let cloud = match &target.instance {
            InstanceChoice::New => return Ok(()),
            InstanceChoice::Named(name) => CloudRef {
                kind: target.kind,
                name: name.clone(),
            },
            InstanceChoice::OfSession(id) => match self.cloud_marks.get(id) {
                Some(mark) => mark.cloud(),
                None => return Ok(()),
            },
        };
        self.auto_stop_running.refuse_new_session(&cloud)
    }

    pub(super) fn finish_auto_stop(
        &mut self,
        decision: Decision,
        outcome: StopOutcome,
        cx: &mut Context<Self>,
    ) {
        let key = decision.cloud.key();
        self.auto_stop_running.remove(&key);
        self.cloud_workspaces_busy.remove(&key);
        self.sync_auto_stopping(cx);
        let now = self.auto_stop_now();
        self.auto_stop.finished(&decision, &outcome, now);
        match outcome {
            StopOutcome::Stopped => {
                self.auto_stop_failed.remove(&key);
                self.cloud_states.insert(key, CloudState::Stopped);
                self.sync_cloud_stopped(cx);
                self.invalidate_chrome(cx);
                self.notice = Some(decision.notice());
                cx.notify();
            }
            // The cached state was behind: read it again.
            StopOutcome::NotRunning => self.refresh_cloud(cx),
            StopOutcome::InUse => {}
            StopOutcome::Failed(error) => {
                let message = failure_message(&decision, &error);
                // The window says so once per workspace (a retry a window later would repeat
                // the same line); the log keeps every failure.
                eprintln!("{message}");
                if self.auto_stop_failed.insert(key) {
                    self.error = Some(message);
                    cx.notify();
                }
            }
        }
    }
}

/// What the error line says when an automatic stop failed: the workspace keeps running and
/// billing, and the user can stop it by hand.
pub(super) fn failure_message(decision: &Decision, error: &str) -> String {
    format!(
        "Could not stop {} {} automatically ({error}). It is still running and may keep costing \
         money: stop it from the session menu.",
        decision.cloud.noun(),
        decision.cloud.name
    )
}
