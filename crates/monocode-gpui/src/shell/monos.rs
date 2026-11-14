use super::*;
use monocode_core::agent::Settings;
pub(crate) struct MonoSession {
    pub id: i64,
    pub title: String,
    pub cwd: String,
    pub busy: bool,
    pub has_messages: bool,
    pub archived: bool,
    pub updated_at: u64,
    pub needs_input: bool,
    /// Its codespace or sandbox failed to start; it waits for Retry and is not working.
    pub start_failed: bool,
    pub branch: Option<String>,
    pub model: SharedString,
    /// Runs on a codespace or a sandbox rather than on this machine.
    pub cloud: bool,
}
impl Shell {
    pub(crate) fn mono_install_app(
        &mut self,
        summary: Session,
        settings: Settings,
        draft: bool,
        cx: &mut Context<Self>,
    ) {
        let id = summary.id;
        self.draft_sessions.insert(id);
        self.session_worktrees
            .insert(id, settings.worktree_cwd.clone());
        self.mark_cloud_session(id);
        self.register_cloud_start(id);
        self.add_loaded(summary, cx);
        self.sessions[&id].update(cx, |view, cx| view.install_app_launch(settings, !draft, cx));
        self.track_cloud_start(id, cx);
        self.invalidate_chrome(cx);
    }
    pub(crate) fn close_mono_overview(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.set_right_panel_open(false, window, cx);
    }
    /// As a restart would: the store's session list omits hidden launches.
    #[cfg(test)]
    pub(crate) fn mono_forget_session(&mut self, id: i64) {
        self.order.retain(|s| *s != id);
        self.session_summaries.remove(&id);
        self.sessions.remove(&id);
    }
    /// Reads the cloud marks from the store, as the shell's periodic refresh does.
    #[cfg(test)]
    pub(crate) fn mono_refresh_cloud(&mut self, cx: &mut Context<Self>) {
        self.refresh_cloud(cx);
    }
    #[cfg(test)]
    pub(crate) fn mono_manager(&self) -> Entity<crate::monos::Monos> {
        self.monos.as_ref().unwrap().clone()
    }
    #[cfg(all(target_os = "macos", not(test)))]
    pub(crate) fn mono_quick_compose(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.show_quick_composer(window, cx);
    }
    pub(super) fn mono_active(&self, cx: &App) -> bool {
        self.monos
            .as_ref()
            .is_some_and(|m| m.read(cx).active.is_some())
    }
    /// The compact cloud mark of a Mono launch row, from the cached state (never wakes the instance).
    pub(crate) fn mono_cloud_badge(&self, id: i64) -> Option<AnyElement> {
        self.cloud_badge(id, true)
    }
    /// "Docker Sandbox box-1": the backend and instance of a cloud launch, from the cached marks.
    pub(crate) fn mono_cloud_label(&self, id: i64) -> Option<String> {
        let mark = self.cloud_marks.get(&id)?;
        Some(format!("{} {}", mark.kind.title(), mark.codespace))
    }
    pub(crate) fn mono_session_views(&self, cx: &App) -> Vec<MonoSession> {
        // A Mono's launches are usually hidden from the sidebar, so they are
        // known by summary only, outside the sidebar's order.
        let listed: HashSet<_> = self.order.iter().collect();
        let hidden = self
            .session_summaries
            .keys()
            .filter(|id| !listed.contains(id));
        self.order
            .iter()
            .chain(hidden)
            .map(|&id| {
                let metadata = self.session_metadata(id, cx);
                let live = self.sessions.get(&id).map(|v| v.read(cx));
                MonoSession {
                    id,
                    title: metadata.summary.title.clone(),
                    cwd: metadata.summary.cwd.clone(),
                    busy: metadata.busy
                        || live
                            .is_some_and(|v| v.mono_external_pending() || v.cloud_launch_pending()),
                    has_messages: metadata.summary.has_messages,
                    archived: metadata.summary.archived,
                    updated_at: metadata.summary.updated_at,
                    needs_input: metadata.needs_approval(),
                    start_failed: self.cloud_start_failed(id)
                        || live.is_some_and(|v| v.cloud_start_failed()),
                    branch: live.and_then(|v| v.branch_name()).map(str::to_owned),
                    model: live.map(|v| v.mono_model_label()).unwrap_or_default(),
                    cloud: self.is_cloud(id),
                }
            })
            .collect()
    }
    /// Load the sessions a Mono launched that the sidebar hides, so its panel
    /// can list and open them. They stay out of the sidebar.
    pub(crate) fn mono_track_sessions(&mut self, ids: Vec<i64>, cx: &mut Context<Self>) {
        let missing: Vec<_> = ids
            .into_iter()
            .filter(|id| !self.session_summaries.contains_key(id) && !self.deleting.contains(id))
            .collect();
        if missing.is_empty() {
            return;
        }
        let store = self.store.clone();
        let work = cx.background_executor().spawn(async move {
            let store = locked(&store)?;
            Ok::<_, String>(
                missing
                    .into_iter()
                    .filter_map(|id| store.mono_session(id).ok().flatten())
                    .collect::<Vec<_>>(),
            )
        });
        cx.spawn(async move |this, cx| {
            let Ok(found) = work.await else {
                return;
            };
            let _ = this.update(cx, |this, cx| {
                for summary in found {
                    if this.session_summaries.contains_key(&summary.id) {
                        continue;
                    }
                    this.sidebar_hidden_sessions.insert(summary.id);
                    this.session_summaries.insert(summary.id, summary);
                }
                if let Some(monos) = &this.monos {
                    monos.update(cx, |_, cx| cx.notify());
                }
            });
        })
        .detach();
    }
    pub(crate) fn mono_session_entity(&self, id: i64) -> Option<Entity<Conversation>> {
        self.sessions.get(&id).cloned()
    }
    pub(crate) fn mono_ensure_session(
        &mut self,
        id: i64,
        cx: &mut Context<Self>,
    ) -> Option<Entity<Conversation>> {
        self.ensure_session(id, cx)
    }
    /// Hangs a Mono's launch under the session it was started beneath; the link is saved by the caller.
    pub(crate) fn mono_link_started(&mut self, child: i64, parent: i64, cx: &mut Context<Self>) {
        self.started_by.insert(child, parent);
        self.invalidate_chrome(cx);
    }
    /// Puts a session made in the store into the shell, as the sidebar does for a cloud session.
    #[cfg(test)]
    pub(crate) fn mono_add_session(
        &mut self,
        summary: monocode_core::store::Session,
        cx: &mut Context<Self>,
    ) {
        self.add(summary, cx);
    }
    /// The project the sidebar lists a session under.
    #[cfg(test)]
    pub(crate) fn mono_listed_project(&self, id: i64) -> Option<String> {
        self.session_summaries
            .get(&id)
            .map(|s| self.project_of(id, &s.cwd).to_owned())
    }
    #[cfg(test)]
    pub(crate) fn mono_started_by(&self, id: i64) -> Option<i64> {
        self.started_by.get(&id).copied()
    }
    pub(crate) fn mono_groups(&self, project: &str) -> monocode_core::session_groups::Groups {
        self.groups.get(project).cloned().unwrap_or_default()
    }
    /// Shows the groups now and saves them through the same queue as deletes,
    /// so an older snapshot can never land after a newer one. The receiver says
    /// how the save went; a failure also shows in the error line.
    pub(crate) fn mono_apply_groups(
        &mut self,
        project: String,
        groups: monocode_core::session_groups::Groups,
        cx: &mut Context<Self>,
    ) -> async_channel::Receiver<Result<(), String>> {
        self.groups.insert(project.clone(), groups.clone());
        self.invalidate_chrome(cx);
        self.store_write_reporting("save folders", cx, move |store| {
            store.save_groups(&project, &groups)
        })
    }
    /// Deletes a session for a Mono and answers on `report` once the provider
    /// files are dealt with. Files are only requested for a session this
    /// computer holds files for; anything else reports `Nothing`.
    pub(crate) fn mono_delete_with_files(
        &mut self,
        id: i64,
        report: async_channel::Sender<Result<super::project_removal::FilesOutcome, String>>,
        input: &serde_json::Value,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // `cloud: "stop" | "delete"` of the call; anything else leaves the instance alone.
        let instance = self.app_delete_instance_task(id, input);
        let provider_files = self.session_summaries.contains_key(&id)
            && input["providerFiles"] == true
            && self.local_files_kind(id, cx).is_some();
        self.delete_session_with(
            id,
            super::project_removal::DeleteOptions {
                provider_files,
                worktree: None,
                report: Some(report),
                instance,
            },
            window,
            cx,
        );
    }
    /// Points the shell at the cloud service a test installed (fake gh and host).
    #[cfg(test)]
    pub(crate) fn mono_set_cloud(
        &mut self,
        cloud: std::sync::Arc<monocode_core::cloud_sessions::CloudSessions>,
    ) {
        self.cloud = cloud;
    }
    /// Whether an orchestration lead is running this session as a worker.
    pub(crate) fn mono_is_worker(&self, id: i64) -> bool {
        self.orchestration_worker(id)
    }
    #[cfg(test)]
    pub(crate) fn mono_set_provider_cleanup(
        &mut self,
        cleanup: impl Fn(
            Option<&std::path::Path>,
            &monocode_core::provider_cleanup::CleanupRequest,
        ) -> monocode_core::provider_cleanup::CleanupReport
        + Send
        + Sync
        + 'static,
    ) {
        self.provider_cleanup = Arc::new(cleanup);
    }
    /// Test support: makes a stored session known to the shell, as a restart would.
    #[cfg(test)]
    pub(crate) fn mono_test_adopt(&mut self, summary: Session) {
        self.session_summaries.insert(summary.id, summary);
    }
    #[cfg(test)]
    pub(crate) fn mono_test_has_session(&self, id: i64) -> bool {
        self.session_summaries.contains_key(&id)
    }
    /// Test support: `worker` runs as an orchestration worker of `lead`.
    #[cfg(test)]
    pub(crate) fn mono_test_make_worker(&mut self, lead: i64, worker: i64) {
        self.orchestration_summaries.insert(
            lead,
            monocode_core::orchestration::Summary {
                live: false,
                tasks: vec![monocode_core::orchestration::SummaryTask {
                    session: Some(worker),
                    title: "Write the tests".into(),
                    provider: "codex".into(),
                    model: None,
                    status: "completed".into(),
                }],
            },
        );
    }
    /// Test support: an archive is working on the workspace `cloud`.
    #[cfg(test)]
    pub(crate) fn mono_test_busy_cloud(&mut self, cloud: &monocode_core::cloud_sessions::CloudRef) {
        self.cloud_workspaces_busy.insert(cloud.key());
    }
    /// Test support: a detached file pane of the session sits in a tab.
    #[cfg(test)]
    pub(crate) fn mono_test_pin_pane(&mut self, session: i64) {
        let pane = 9_000_001;
        self.workspace.review_owners.insert(pane, session);
        self.workspace.tabs.push(pane);
        self.workspace
            .title_order
            .push(crate::workspace::TitleTab::Session(pane));
    }
    pub(crate) fn mono_remove(
        &mut self,
        id: i64,
        delete: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if delete {
            self.delete_session(id, window, cx);
        } else if self.session_summaries.contains_key(&id)
            && !self.session_metadata(id, cx).summary.archived
        {
            self.toggle_archive_now(id, cx);
        }
    }
}
