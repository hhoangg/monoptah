//! What the Today page needs from the Monos.
use super::*;
use crate::today::{HabitInfo, MonoInfo};
use monocode_core::{
    mono::{bridge::Call, heartbeat},
    time::now_ms,
    today::MonoCaller,
};
use std::sync::{Arc, Mutex};

/// How long a loaded copy of every Mono's habits is trusted before a read starts a reload.
const HABITS_TTL_MS: u64 = 5_000;

#[derive(Default)]
struct Shared {
    revision: u64,
    habits: HashMap<String, Vec<Habit>>,
    loaded_at: Option<u64>,
    loading: bool,
    /// Something changed while a load was reading; read again when it lands.
    dirty: bool,
}
/// The Today page's view of the Monos. Cloneable and lock-based so the render
/// path can read it and a background load can fill it, without the UI thread
/// ever reading a habits file.
#[derive(Clone, Default)]
pub(super) struct Bridge {
    shared: Arc<Mutex<Shared>>,
}
impl Bridge {
    fn lock(&self) -> std::sync::MutexGuard<'_, Shared> {
        self.shared.lock().unwrap_or_else(|e| e.into_inner())
    }
    /// Tells the page something it shows may have changed.
    pub(super) fn bump(&self) {
        let mut shared = self.lock();
        shared.revision += 1;
    }
    /// The loaded habits are out of date: the next read reloads them.
    fn stale(&self) {
        let mut shared = self.lock();
        shared.loaded_at = None;
        shared.dirty = shared.loading;
    }
    /// Stores a finished load. Returns whether the page's view changed, and whether
    /// the data went stale while it was being read.
    fn land(&self, loaded: HashMap<String, Vec<Habit>>) -> (bool, bool) {
        let mut shared = self.lock();
        shared.loading = false;
        let again = std::mem::take(&mut shared.dirty);
        shared.loaded_at = (!again).then(now_ms);
        let mut known: Vec<_> = shared.habits.keys().collect();
        let mut now_known: Vec<_> = loaded.keys().collect();
        known.sort();
        now_known.sort();
        let changed = known != now_known || digest(&shared.habits) != digest(&loaded);
        if changed {
            shared.revision += 1;
        }
        shared.habits = loaded;
        (changed, again)
    }
}

impl Monos {
    /// The identity of a `today.*` call: the Mono behind the checked session capability, and
    /// the habit when the session is a habit run. Nothing here comes from the call's input.
    pub(super) fn today_caller(&self, call: &Call, mono: &Mono) -> MonoCaller {
        let run = self.running.get(&call.session);
        MonoCaller {
            id: mono.id.clone(),
            projects: mono.projects.clone(),
            // A heartbeat is a run but not a habit.
            habit: run
                .map(|r| r.habit.clone())
                .filter(|habit| habit != heartbeat::RUN_ID),
            in_run: run.is_some() || self.workflow_live.asked_by(call.session).is_some(),
        }
    }
    /// Every Mono with its projects and habits, for Today's badges and Sources tab.
    /// Runs in render paths, so habits come from a cached copy; a stale one starts a
    /// background reload that bumps the revision when it lands.
    /// `None` when the roster failed to load: an empty list then is not "every Mono removed".
    pub(crate) fn today_roster(&self, cx: &App) -> Option<Vec<MonoInfo>> {
        if self.roster_failed {
            return None;
        }
        self.load_today_habits(cx);
        let now = now_ms();
        let shared = self.today.lock();
        self.roster
            .monos
            .iter()
            .map(|mono| MonoInfo {
                id: mono.id.clone(),
                name: mono.label(),
                mascot: mono.mascot.clone(),
                color: mono.color,
                projects: mono.projects.clone(),
                habits_known: shared.habits.contains_key(&mono.id),
                habits: shared
                    .habits
                    .get(&mono.id)
                    .map(|habits| {
                        habits
                            .iter()
                            .map(|habit| HabitInfo {
                                id: habit.id.clone(),
                                name: habit.name.clone(),
                                schedule: habit.schedule.label(),
                                enabled: habit.enabled,
                                busy: self.today_habit_busy(&mono.id, habit, now),
                            })
                            .collect()
                    })
                    .unwrap_or_default(),
            })
            .collect::<Vec<_>>()
            .into()
    }
    /// Bumped after each `today.*` action, so the page can reload live.
    pub(crate) fn today_revision(&self) -> u64 {
        self.today.lock().revision
    }
    /// Queued, or running now.
    fn today_habit_busy(&self, mono: &str, habit: &Habit, now: u64) -> bool {
        habit.running_since.is_some()
            || habit.run_requested
            || habit.due(now)
            || self
                .running
                .values()
                .any(|r| r.mono == mono && r.habit == habit.id)
    }
    /// Claims a reload of every Mono's habits: `None` while one is running, or when the
    /// copy is still fresh and `force` is not set.
    fn begin_today_load(&self, force: bool) -> Option<(PathBuf, Vec<String>)> {
        let now = now_ms();
        let mut shared = self.today.lock();
        let fresh = shared
            .loaded_at
            .is_some_and(|at| now.saturating_sub(at) < HABITS_TTL_MS);
        if shared.loading || (fresh && !force) {
            return None;
        }
        shared.loading = true;
        let ids = self.roster.monos.iter().map(|m| m.id.clone()).collect();
        Some((self.root.clone(), ids))
    }
    /// Reloads when the copy is old. For render paths: nothing wakes the page when it lands,
    /// the revision tells its next refresh.
    fn load_today_habits(&self, cx: &App) {
        let Some((root, ids)) = self.begin_today_load(false) else {
            return;
        };
        let bridge = self.today.clone();
        cx.background_executor()
            .spawn(async move {
                bridge.land(read_habits(&root, ids));
            })
            .detach();
    }
    /// Reloads now and wakes the page when what it shows changed.
    pub(crate) fn reload_today_habits(&mut self, cx: &mut Context<Self>) {
        let Some((root, ids)) = self.begin_today_load(true) else {
            // One is in flight and may have read before this change.
            self.today.stale();
            return;
        };
        let work = cx
            .background_executor()
            .spawn(async move { read_habits(&root, ids) });
        cx.spawn(async move |this, cx| {
            let loaded = work.await;
            let again = this
                .update(cx, |this, cx| {
                    let (changed, again) = this.today.land(loaded);
                    if changed {
                        cx.notify();
                    }
                    again
                })
                .unwrap_or(false);
            if again {
                let _ = this.update(cx, |this, cx| this.reload_today_habits(cx));
            }
        })
        .detach();
    }
    /// Asks these (Mono id, habit id) to run through the normal habit path.
    pub(crate) fn request_habit_runs(
        &mut self,
        targets: Vec<(String, String)>,
        cx: &mut Context<Self>,
    ) {
        // A habit that is already queued or running is left alone.
        let now = now_ms();
        let mut wanted: Vec<(String, String)> = Vec::new();
        {
            let shared = self.today.lock();
            for target in targets
                .into_iter()
                .filter(|t| self.roster.get(&t.0).is_some())
            {
                let cached = shared
                    .habits
                    .get(&target.0)
                    .and_then(|all| all.iter().find(|h| h.id == target.1));
                let busy = match cached {
                    Some(h) => self.today_habit_busy(&target.0, h, now),
                    None => self
                        .running
                        .values()
                        .any(|r| r.mono == target.0 && r.habit == target.1),
                };
                if !busy && !wanted.contains(&target) {
                    wanted.push(target);
                }
            }
        }
        if wanted.is_empty() {
            return;
        }
        // Show it queued at once; the file write below makes it real.
        {
            let mut shared = self.today.lock();
            for (mono, id) in &wanted {
                if let Some(h) = shared
                    .habits
                    .get_mut(mono)
                    .and_then(|all| all.iter_mut().find(|h| &h.id == id))
                    && !h.needs_review
                {
                    h.run_requested = true;
                }
            }
            shared.revision += 1;
        }
        let root = self.root.clone();
        let work = cx.background_executor().spawn(async move {
            let monos: std::collections::BTreeSet<&String> =
                wanted.iter().map(|(m, _)| m).collect();
            let mut result = Ok(());
            for mono in monos {
                let ids: Vec<&String> = wanted
                    .iter()
                    .filter(|(m, _)| m == mono)
                    .map(|(_, h)| h)
                    .collect();
                let done = habits::update(&root, mono, |all| {
                    for h in all.iter_mut().filter(|h| ids.contains(&&h.id)) {
                        if h.running_since.is_none() {
                            // An imported habit waiting for review is left as it is.
                            let _ = h.request_run();
                        }
                    }
                    Ok(())
                });
                if let Err(error) = done {
                    result = Err(error);
                }
            }
            result.map(|_| wanted)
        });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let window = this
                .update(cx, |this, cx| {
                    match result {
                        Ok(wanted) => {
                            for (mono, _) in wanted {
                                this.reload_files(&mono, cx);
                            }
                        }
                        Err(error) => this.error = Some(error),
                    }
                    this.reload_today_habits(cx);
                    this.today.bump();
                    this.changed(cx);
                    this.parent_window
                })
                .ok()
                .flatten();
            // Claim the request now instead of at the next 10 second tick.
            if let Some(window) = window {
                let _ = window.update(cx, |_, window, app| {
                    let _ = this.update(app, |this, cx| this.tick(window, cx));
                });
            }
        })
        .detach();
        cx.notify();
    }
}

fn read_habits(root: &std::path::Path, ids: Vec<String>) -> HashMap<String, Vec<Habit>> {
    ids.into_iter()
        .filter_map(|id| Some((id.clone(), habits::load(root, &id).ok()?)))
        .collect()
}

/// The fields of a loaded set that the page shows, in a comparable form.
fn digest(all: &HashMap<String, Vec<Habit>>) -> Vec<(String, String, String, bool, bool, bool)> {
    let mut out: Vec<_> = all
        .iter()
        .flat_map(|(mono, habits)| {
            habits.iter().map(|h| {
                (
                    mono.clone(),
                    h.id.clone(),
                    format!("{}|{}", h.name, h.schedule.label()),
                    h.enabled,
                    h.run_requested,
                    h.running_since.is_some(),
                )
            })
        })
        .collect();
    out.sort();
    out
}

#[cfg(test)]
mod tests;
