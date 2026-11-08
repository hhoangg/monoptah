//! "RUN ON CLOUD" from the new-session composer, shell side: the conversation asks for
//! "Create and open" (`CreateCloudSession`), this creates the Codespace or Docker sandbox on a
//! worker (the same `begin_start` / `begin_sandbox_start` the pickers use), queues the composer
//! text as the first prompt of the new session and hands it to `open_cloud_start`, so the step
//! list, the queue and Retry/Delete are shared with the header-menu picker.
use super::*;
use crate::conversation::cloud_target::{CloudCreate, CloudTarget};
use monocode_core::cloud_sessions::{
    CloudSessions, CodespaceChoice, HARNESS, StartRecord, queue_start_message,
};

/// Creates the resource and the session for `request`. Blocking: call it on a worker.
pub(super) fn begin_cloud_create(
    cloud: &CloudSessions,
    request: &CloudCreate,
) -> Result<(Session, StartRecord), String> {
    match request.target {
        CloudTarget::Codespace => {
            let target = cloud_picker::target_for(CodespaceChoice::New, &request.branch, None)?;
            cloud_picker::begin_cloud_start(cloud, &request.project, target)
        }
        CloudTarget::DockerSandbox => {
            let config = request
                .config
                .as_ref()
                .ok_or("Set up Docker Sandbox for this project first.")?;
            cloud.begin_sandbox_start(&request.project, config, &request.branch, HARNESS)
        }
    }
}

/// Replaces the real creation in tests, which never call a cloud.
#[cfg(test)]
pub(super) type CreateHook =
    Arc<dyn Fn(&CloudCreate) -> Result<(Session, StartRecord), String> + Send + Sync>;

impl Shell {
    /// Tells every conversation whether its project can host a codespace: a local GitHub repository.
    pub(super) fn sync_codespace_offers(&mut self, cx: &mut Context<Self>) {
        for view in self.sessions.values() {
            let cwd = view.read(cx).summary.cwd.clone();
            let offers = !cwd.starts_with("remote://")
                && self.github_projects.get(&cwd).copied().unwrap_or(false);
            view.update(cx, |view, cx| view.set_offers_codespace(offers, cx));
        }
    }

    pub(super) fn create_cloud_session(
        &mut self,
        draft: i64,
        request: CloudCreate,
        cx: &mut Context<Self>,
    ) {
        let cloud = self.cloud.clone();
        let store = self.store.clone();
        #[cfg(test)]
        let hook = self.cloud_create_hook.clone();
        let started = request.clone();
        let work = cx.background_executor().spawn(async move {
            #[cfg(test)]
            let created = match hook {
                Some(hook) => hook(&started),
                None => begin_cloud_create(&cloud, &started),
            };
            #[cfg(not(test))]
            let created = begin_cloud_create(&cloud, &started);
            // Queued here, right after the resource exists, so closing the window while the
            // create runs does not lose the prompt.
            let consumed = match (&created, &started.prompt) {
                (Ok((summary, _)), Some(text)) => locked(&store)
                    .and_then(|store| queue_start_message(&store, summary.id, text))
                    .is_ok(),
                _ => true,
            };
            (created, consumed)
        });
        cx.spawn(async move |this, cx| {
            let (result, consumed) = work.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.finish_cloud_create(draft, request.request_id, result, consumed, window, cx)
            });
        })
        .detach();
    }

    fn finish_cloud_create(
        &mut self,
        draft: i64,
        request_id: u64,
        result: Result<(Session, StartRecord), String>,
        consumed: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let outcome = match result {
            Ok((summary, record)) => {
                self.open_cloud_start(summary, record, window, cx);
                Ok(consumed)
            }
            Err(error) => Err(error),
        };
        if let Some(view) = self.sessions.get(&draft) {
            view.update(cx, |view, cx| {
                view.cloud_create_finished(request_id, outcome, cx)
            });
        }
    }
}
