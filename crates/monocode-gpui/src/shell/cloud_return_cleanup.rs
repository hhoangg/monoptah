//! Deleting the cloud instance after "Continue locally" succeeded (design 4.2.3).
//!
//! The core arms a record when a return finished (see `cloud_sessions::return_cleanup`); this
//! module runs it on a worker and reports the result in the status line. A failure never
//! retries by itself: the session menu offers Retry and Keep. Nothing here runs after a failed
//! or cancelled continue, because no record exists then.
use super::*;
use monocode_core::cloud_sessions::{
    CleanupOutcome, CloudSessions, return_cleanup::ALREADY_DELETING,
};

/// Runs the delete of the instance recorded under a cloud key. A seam for tests.
pub(super) type CleanupFn =
    Arc<dyn Fn(&CloudSessions, &str) -> Result<CleanupOutcome, String> + Send + Sync>;

pub(super) fn default_cleanup() -> CleanupFn {
    Arc::new(|cloud, key| cloud.run_return_cleanup(key))
}

pub(super) struct State {
    /// Cloud keys whose delete is running now.
    pub running: HashSet<String>,
    /// The status text of the return a delete follows, by cloud key: the delete's result is
    /// appended to it, not shown instead of it.
    pub base: HashMap<String, String>,
    pub run: CleanupFn,
}

impl Default for State {
    fn default() -> Self {
        Self {
            running: HashSet::new(),
            base: HashMap::new(),
            run: default_cleanup(),
        }
    }
}

/// The status line for a finished delete of `what` (for example "codespace octo-abc").
pub(super) fn outcome_text(
    what: &str,
    result: &Result<CleanupOutcome, String>,
    after_error: &str,
) -> Option<String> {
    match result {
        Ok(CleanupOutcome::Nothing) => None,
        Ok(CleanupOutcome::AlreadyGone) => Some(format!("The {what} was already deleted.")),
        Ok(CleanupOutcome::Deleted { warning: None }) => Some(format!("The {what} was deleted.")),
        Ok(CleanupOutcome::Deleted { warning: Some(w) }) => {
            Some(format!("The {what} was deleted, but {w}."))
        }
        Ok(CleanupOutcome::Kept { reason }) => Some(reason.clone()),
        Err(e) if e == ALREADY_DELETING => None,
        Err(e) => Some(format!("Could not delete the {what}: {e}. {after_error}")),
    }
}

impl Shell {
    /// Deletes the instance recorded under `cloud_key` on a worker. `base` is the status text
    /// of the return it follows; the result is appended to it. `false` when it already runs,
    /// when nothing is recorded, or when another action works on the same workspace: that
    /// last case turns the record into a Retry so it never runs silently later.
    pub(super) fn start_return_cleanup(
        &mut self,
        cloud_key: &str,
        base: Option<String>,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.cloud_return_cleanup.running.contains(cloud_key) {
            return false;
        }
        let Some(record) = self
            .cloud
            .return_cleanups()
            .into_iter()
            .find(|r| r.key() == cloud_key)
        else {
            return false;
        };
        let what = format!("{} {}", record.cloud.noun(), record.cloud.name);
        if self.cloud_workspaces_busy.contains(cloud_key) {
            let reason =
                format!("another action was working on the {what}; choose Retry to delete it");
            let saved = self.cloud.defer_return_cleanup(cloud_key, &reason);
            let text = if saved {
                format!("The {what} was kept: {reason}.")
            } else {
                format!(
                    "The {what} was not deleted: another action was working on it. The cleanup will be offered again at the next start."
                )
            };
            self.error = Some(match base {
                Some(base) => format!("{base} {text}"),
                None => text,
            });
            cx.notify();
            return false;
        }
        let key = cloud_key.to_string();
        self.cloud_return_cleanup.running.insert(key.clone());
        self.cloud_workspaces_busy.insert(key.clone());
        match base {
            Some(base) => self.cloud_return_cleanup.base.insert(key.clone(), base),
            None => self.cloud_return_cleanup.base.remove(&key),
        };
        let cloud = self.cloud.clone();
        let run = self.cloud_return_cleanup.run.clone();
        let worker_key = key.clone();
        let work = cx
            .background_executor()
            .spawn(async move { run(&cloud, &worker_key) });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |this, cx| {
                this.cloud_return_cleanup.running.remove(&key);
                this.cloud_workspaces_busy.remove(&key);
                let base = this.cloud_return_cleanup.base.remove(&key);
                // What the user can do next must match what is stored.
                let after_error = match this
                    .cloud
                    .return_cleanups()
                    .into_iter()
                    .find(|r| r.key() == key)
                {
                    Some(r) if r.needs_retry() => "Retry from the session menu.",
                    Some(_) => "It will be offered again at the next start.",
                    None => "",
                };
                if let Some(text) = outcome_text(&what, &result, after_error) {
                    this.error = Some(match base {
                        Some(base) => format!("{base} {text}"),
                        None => text,
                    });
                }
                this.refresh_cloud(cx);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
        true
    }

    /// App start: a delete that was running when the app closed becomes a Retry, then every
    /// delete that was armed but never tried runs.
    pub(super) fn resume_return_cleanups(&mut self, cx: &mut Context<Self>) {
        self.cloud.mark_interrupted_return_cleanups();
        for record in self.cloud.return_cleanups() {
            if record.auto_run() {
                let _ = self.start_return_cleanup(&record.key(), None, cx);
            }
        }
    }

    /// Retry and Keep for a delete that failed or was interrupted, in the menu of the session
    /// that came back.
    pub(super) fn return_cleanup_items(&self, id: i64) -> Vec<MenuItem> {
        let Some(record) = self.cloud.return_cleanup_for_session(id) else {
            return vec![];
        };
        if !record.needs_retry() || self.cloud_return_cleanup.running.contains(&record.key()) {
            return vec![];
        }
        let what = format!("{} {}", record.cloud.noun(), record.cloud.name);
        if record.gone {
            // The instance is already gone: only bookkeeping is left.
            return vec![
                MenuAction::new("retry-return-cleanup", format!("Finish cleanup of {what}")).into(),
                MenuAction::new("keep-return-cleanup", format!("Dismiss cleanup of {what}")).into(),
            ];
        }
        vec![
            MenuAction::new("retry-return-cleanup", format!("Retry deleting {what}"))
                .danger()
                .into(),
            MenuAction::new("keep-return-cleanup", format!("Keep {what}")).into(),
        ]
    }

    /// Menu action: try the delete again.
    pub(super) fn retry_return_cleanup(&mut self, id: i64, cx: &mut Context<Self>) {
        if let Some(record) = self
            .cloud
            .return_cleanup_for_session(id)
            .filter(|r| r.needs_retry())
        {
            let _ = self.start_return_cleanup(&record.key(), None, cx);
        }
    }

    /// Menu action: leave the instance running and forget the pending delete.
    pub(super) fn keep_return_cleanup(&mut self, id: i64, cx: &mut Context<Self>) {
        let Some(record) = self
            .cloud
            .return_cleanup_for_session(id)
            .filter(|r| r.needs_retry())
        else {
            return;
        };
        if self.cloud_return_cleanup.running.contains(&record.key()) {
            return;
        }
        let what = format!("{} {}", record.cloud.noun(), record.cloud.name);
        self.error = Some(match self.cloud.dismiss_return_cleanup(&record.key()) {
            Ok(()) if record.gone => {
                format!("The {what} is already deleted; only the cleanup notice was dismissed.")
            }
            Ok(()) => format!(
                "The {what} was kept. {}",
                self.cloud
                    .kept_instance_hint(&record.cloud, Some(record.local_id))
            ),
            Err(e) => format!(
                "Could not keep the {what}: {e}. The cleanup is still pending and will be offered again at the next start."
            ),
        });
        cx.notify();
    }
}
