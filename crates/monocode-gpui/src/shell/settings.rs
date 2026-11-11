//! The settings surface replaces the rail body and the workspace, preserving
//! open conversations. Its breadcrumb/actions occupy the inset title bar.
use super::*;
use crate::settings::{ArchivedSession, Section, SettingsContext, SettingsEvent, SettingsView};
use std::path::Path;
impl Shell {
    pub(super) fn settings_context(&self, cx: &App) -> SettingsContext {
        let mut archived: Vec<_> = self
            .order
            .iter()
            .map(|&id| self.session_metadata(id, cx).summary)
            .filter(|s| s.archived && self.project_of(s.id, &s.cwd) == self.workspace.project)
            .map(|s| ArchivedSession {
                id: s.id,
                title: s.title.clone(),
                provider: s.provider.clone(),
                updated_at: s.updated_at,
            })
            .collect();
        archived.sort_by_key(|s| std::cmp::Reverse(s.updated_at));
        let mut catalogs = HashMap::new();
        let active = self
            .monos
            .as_ref()
            .filter(|_| self.mono_active(cx))
            .and_then(|m| m.read(cx).chat())
            .or_else(|| {
                self.workspace.active.and_then(|id| {
                    self.sessions
                        .get(&self.workspace.session_for_pane(id))
                        .cloned()
                })
            });
        let accounts = self.settings_profiles(cx);
        let mut profiles: HashMap<_, _> = accounts
            .iter()
            .map(|(kind, profile)| (*kind, profile.id.clone()))
            .collect();
        if let Some(active) = &active {
            let v = active.read(cx);
            profiles.insert(
                Kind::from_id(&v.summary.provider),
                v.settings.account.id.clone(),
            );
        }
        for session in active.iter().chain(self.sessions.values()) {
            let session = session.read(cx);
            if profiles.get(&Kind::from_id(&session.summary.provider))
                != Some(&session.settings.account.id)
            {
                continue;
            }
            if catalogs.contains_key(&session.summary.provider) {
                continue;
            }
            let models = session.settings_models();
            if !models.is_empty() {
                catalogs.insert(session.summary.provider.clone(), models);
            }
        }
        // Without an open session, use the catalog discovered for the account
        // new sessions will use (see `prefetch_settings_catalogs`).
        for (kind, profile) in &accounts {
            if catalogs.contains_key(kind.id()) {
                continue;
            }
            let models =
                monocode_core::catalog::cached_models_for(*kind, profile).unwrap_or_default();
            if !models.is_empty() {
                catalogs.insert(kind.id().into(), settings_rows(*kind, &models));
            }
        }
        SettingsContext {
            today_rollover_minutes: self
                .today
                .as_ref()
                .and_then(|t| t.read(cx).rollover_minutes()),
            data_dir: self.data_dir.clone(),
            remote: Some(self.remote.clone()),
            mobile: self.mobile_status(),
            mobile_invite: self.mobile.invite.clone(),
            quick_error: self.quick.error.clone(),
            inbox_connections: self
                .inbox
                .as_ref()
                .map(|inbox| inbox.read(cx).connection_statuses())
                .unwrap_or_default(),
            sessions: self
                .order
                .iter()
                .map(|&id| {
                    (
                        id,
                        self.session_working_copy(id, cx).to_owned(),
                        self.session_metadata(id, cx).busy,
                    )
                })
                .collect(),
            monos: self
                .monos
                .as_ref()
                .map(|m| {
                    m.read(cx)
                        .roster
                        .monos
                        .iter()
                        .map(|m| crate::settings::MonoSetting {
                            id: m.id.clone(),
                            name: m.label(),
                            mascot: m.mascot.clone(),
                            color: m.color,
                            projects: m.projects.iter().map(|p| self.project_label(p)).collect(),
                            show: m.show_started_sessions_in_sidebar,
                            folders: m.use_sidebar_folders,
                        })
                        .collect()
                })
                .unwrap_or_default(),
            catalogs,
            classic: self.workspace.classic_layout,
            new_layout_boxy: self.workspace.new_layout_boxy,
            no_terminal_sessions: !self.surface_supported,
            cwd: self.workspace.project.clone(),
            project_label: self.project_label(&self.workspace.project),
            archived,
            archived_projects: self
                .workspace
                .archived_projects
                .iter()
                .map(|path| (path.clone(), self.project_label(path)))
                .collect(),
            projects: self
                .workspace
                .projects
                .iter()
                .map(|path| (path.clone(), self.project_label(path)))
                .collect(),
        }
    }
    pub(super) fn open_settings(
        &mut self,
        section: Section,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.leave_app_search(cx);
        self.close_preview(cx);
        self.menu = None;
        if self.settings.is_some() {
            self.select_settings(section, cx);
            return;
        }
        self.navigation_revision += 1;
        self.hide_contributors(cx);
        let context = self.settings_context(cx);
        let prefs = self.workspace.preferences.clone();
        let view = cx.new(|cx| SettingsView::new(section, prefs, context, cx));
        self.prefetch_settings_catalogs(cx);
        self.settings_events = Some(cx.subscribe_in(
            &view,
            window,
            |this, _, event: &SettingsEvent, window, cx| match event {
                SettingsEvent::Section(section) => this.select_settings(*section, cx),
                SettingsEvent::ManageInboxProvider(provider) => {
                    this.open_inbox(window, cx);
                    if let Some(inbox) = &this.inbox {
                        inbox.update(cx, |inbox, cx| inbox.open_connection(provider.clone(), cx));
                    }
                }
                SettingsEvent::TodayRollover(minutes) => {
                    this.ensure_background_today(window, cx);
                    if let Some(today) = &this.today {
                        today.update(cx, |today, cx| today.set_rollover(*minutes, cx));
                    }
                    this.sync_settings(cx);
                }
                SettingsEvent::WhatsNew => {
                    this.open_whats_new(crate::notifications::updater::version().into(), cx)
                }
                SettingsEvent::CheckAppUpdates => {
                    this.notification_state
                        .updater
                        .update(cx, |updater, cx| updater.check(true, cx));
                }
                SettingsEvent::CheckHarnessUpdates => {
                    let hidden = this.workspace.preferences.hidden_providers.clone();
                    this.notification_state
                        .harness
                        .update(cx, |notice, cx| notice.check_now(hidden, cx));
                }
                SettingsEvent::Changed(prefs) => {
                    #[cfg(target_os = "linux")]
                    if this.workspace.preferences.wayland_window_decorations
                        != prefs.wayland_window_decorations
                        && let Some(decorations) = prefs.wayland_decorations(cx.compositor_name())
                    {
                        window.request_decorations(decorations);
                    }
                    this.workspace.preferences = *prefs.clone();
                    this.apply_preferences(cx);
                    this.persist(cx);
                    this.invalidate_chrome(cx);
                    cx.notify();
                }
                SettingsEvent::NewLayout(on) => {
                    if this.workspace.classic_layout == *on {
                        this.toggle_layout(cx);
                    }
                    this.sync_settings(cx);
                }
                SettingsEvent::NewLayoutBoxy(boxy) => {
                    this.workspace.new_layout_boxy = *boxy;
                    this.layout_changed(cx);
                    this.sync_settings(cx);
                }
                SettingsEvent::OpenSession(id) => {
                    this.close_settings(window, cx);
                    this.open(*id, window, cx);
                }
                SettingsEvent::Archive(id, archived) => {
                    if this.session_summaries.contains_key(id)
                        && this.session_metadata(*id, cx).summary.archived != *archived
                    {
                        this.toggle_archive(*id, cx);
                    }
                }
                SettingsEvent::Delete(id) => this.delete_from_settings(*id, window, cx),
                SettingsEvent::UnarchiveProject(path) => this.unarchive_project(path, cx),
                SettingsEvent::DeleteProject(path) => {
                    this.confirm_delete_project(path.clone(), window, cx)
                }
                SettingsEvent::RemoveWorktree {
                    cwd,
                    path,
                    delete_sessions,
                } => this.remove_settings_worktree(
                    cwd.clone(),
                    path.clone(),
                    *delete_sessions,
                    window,
                    cx,
                ),
                SettingsEvent::ChooseBackground => this.choose_chat_background(cx),
                SettingsEvent::ResetMono(id) => {
                    if let Some(m) = this.monos.clone() {
                        let reset = m.update(cx, |m, cx| m.reset_defaults(id, cx));
                        cx.spawn(async move |this, cx| {
                            let result = reset.await;
                            let _ = this.update(cx, |this, cx| {
                                if let Some(settings) = &this.settings {
                                    settings.update(cx, |s, cx| {
                                        s.management_finished(result, None, cx)
                                    });
                                }
                                this.sync_settings(cx);
                            });
                        })
                        .detach();
                    }
                }
                SettingsEvent::MonoSidebar(id, show, folders) => {
                    if let Some(m) = this.monos.clone() {
                        m.update(cx, |m, cx| m.sidebar_preferences(id, *show, *folders, cx));
                    }
                    this.sync_settings(cx);
                }
                SettingsEvent::MobileRefresh => this.sync_settings(cx),
                SettingsEvent::MobileConfigure {
                    enabled,
                    open_router,
                    remote_address,
                } => this.mobile_configure(*enabled, *open_router, remote_address.clone(), cx),
                SettingsEvent::MobilePair => this.pair_mobile(cx),
                SettingsEvent::MobileCancelInvite => this.mobile_cancel_invite(cx),
                SettingsEvent::MobileRevoke(id) => this.mobile_revoke(id, cx),
            },
        ));
        // This dialog is painted at the window root, like React's portal.
        // Redraw it for countdown/copy/close changes even when only the
        // Settings entity was notified, without rebuilding the shell for
        // ordinary settings edits.
        let mut overlay_was_open = false;
        // The search lives in the cached rail, so redraw it as the query moves.
        let mut search_was = view.read(cx).search_state();
        self.settings_overlay_watch = Some(cx.observe(&view, move |this, view, cx| {
            let open = view.read(cx).mobile_overlay_open();
            if open || overlay_was_open {
                cx.notify();
            }
            overlay_was_open = open;
            let search = view.read(cx).search_state();
            if search != search_was {
                search_was = search;
                this.invalidate_chrome(cx);
                cx.notify();
            }
        }));
        self.settings = Some(view.clone());
        // Settings has its own complete rail, even when the project rail was hidden.
        self.settings_nav_visible = true;
        self.sidebar_slide = None;
        self.sidebar_fraction = 1.;
        window.focus(&view.focus_handle(cx), cx);
        this_update(self, cx);
    }
    /// The account each provider's new sessions use in the current project.
    fn settings_profiles(&self, cx: &App) -> Vec<(Kind, monocode_core::accounts::Profile)> {
        Kind::ALL
            .into_iter()
            .map(|kind| {
                let profile = crate::accounts::get(cx)
                    .map(|center| center.read(cx).selected(kind, &self.workspace.project))
                    .unwrap_or_default();
                (kind, profile)
            })
            .collect()
    }
    /// Settings lists models per Agent CLI, but only open sessions carried a
    /// catalog. Discover the rest for the account new sessions use (which may
    /// be a setup-token account), so a provider without a session is not
    /// shown with no models and cannot be made the default.
    pub(super) fn prefetch_settings_catalogs(&mut self, cx: &mut Context<Self>) {
        // Tests must never start an installed provider CLI.
        if cfg!(test) {
            return;
        }
        let shown = self.settings_context(cx).catalogs;
        let missing: Vec<_> = self
            .settings_profiles(cx)
            .into_iter()
            .filter(|(kind, _)| !shown.contains_key(kind.id()))
            .collect();
        if missing.is_empty() {
            return;
        }
        let work = cx.background_executor().spawn(async move {
            for (kind, profile) in missing {
                if monocode_core::provider::resolve(kind).is_some() {
                    // Fills the account-scoped cache that settings_context reads;
                    // a failure leaves the provider as before.
                    let _ = monocode_core::catalog::models_for(kind, &profile);
                }
            }
        });
        cx.spawn(async move |this, cx| {
            work.await;
            let _ = this.update(cx, |this, cx| this.sync_settings(cx));
        })
        .detach();
    }
    pub(super) fn select_settings(&mut self, section: Section, cx: &mut Context<Self>) {
        if let Some(view) = &self.settings {
            view.update(cx, |v, cx| v.select_section(section, cx));
        }
        this_update(self, cx);
    }
    pub(super) fn sync_settings(&mut self, cx: &mut Context<Self>) {
        if let Some(view) = self.settings.clone() {
            let context = self.settings_context(cx);
            let prefs = self.workspace.preferences.clone();
            view.update(cx, |v, cx| {
                v.set_context(context, cx);
                v.set_preferences(&prefs, cx);
            });
        }
    }
    pub(super) fn close_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // A running worktree delete reports back to this view, so keep it alive.
        if let Some(view) = &self.settings
            && view.read(cx).delete_in_flight()
        {
            return;
        }
        self.settings = None;
        self.settings_events = None;
        self.settings_overlay_watch = None;
        // A code nobody can see should not stay usable.
        if self.mobile.invite.is_some() {
            self.mobile_cancel_invite(cx);
        }
        self.sidebar_slide = None;
        self.sidebar_fraction = if self.workspace.sidebar_visible {
            1.
        } else {
            0.
        };
        self.reveal_contributors(cx);
        self.focus_active(window, cx);
        this_update(self, cx);
    }
    /// MobileSettings.tsx's pair: the desktop listens only while a phone is
    /// paired or being paired, so start listening before making the code.
    fn pair_mobile(&mut self, cx: &mut Context<Self>) {
        let Some(status) = self.mobile_status() else {
            return;
        };
        if status.enabled && status.listening {
            self.mobile_invite(cx);
            return;
        }
        self.mobile_configure(true, status.open_router, status.remote_address, cx);
        cx.spawn(async move |this, cx| {
            // Binding runs in the background; a failure lands in `error`.
            for _ in 0..100 {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(100))
                    .await;
                let Ok(ready) = this.update(cx, |this, _| {
                    this.mobile_status()
                        .is_some_and(|status| status.enabled && status.listening)
                }) else {
                    return;
                };
                if ready {
                    let _ = this.update(cx, |this, cx| this.mobile_invite(cx));
                    return;
                }
            }
        })
        .detach();
    }
    /// With "auto resume at usage limit reset" on, every loaded local session
    /// still waiting on a limit is armed. Sessions that exist only on disk are
    /// armed when they load. Turning the switch off arms and cancels nothing.
    fn sweep_auto_resume(&mut self, cx: &mut Context<Self>) {
        if !self.workspace.preferences.auto_resume_usage_limit {
            return;
        }
        for view in self.sessions.values() {
            view.update(cx, |view, cx| view.sweep_auto_resume(cx));
        }
        if let Some(monos) = &self.monos {
            monos.update(cx, |monos, cx| monos.sweep_auto_resume(cx));
        }
    }
    /// Starts that run on a worker, and the app API, read the Claude setup choice from the
    /// published copy. It must be there BEFORE a resumed start runs, or that start would read
    /// the default (off) and skip the step for good. Tests do not publish: the slot is
    /// process-wide, and one test's choice must not reach another's service.
    pub(super) fn publish_kit_prefs(&self) {
        #[cfg(not(test))]
        monocode_core::cloud_sessions::kit_prefs::publish(
            self.workspace.preferences.cloud_kit.clone(),
        );
    }

    pub(super) fn apply_preferences(&mut self, cx: &mut Context<Self>) {
        self.refresh_notification_permission();
        self.configure_quick_composer(cx);
        // Switching search styles closes an open search of the other style.
        if self
            .app_search
            .as_ref()
            .is_some_and(|s| s.panel != self.workspace.preferences.search_panel)
        {
            self.leave_app_search(cx);
            self.update_chat_visibility(cx);
            self.invalidate_chrome(cx);
        }
        monocode_core::cli::initialize(&self.workspace.preferences.binary_paths);
        self.keep_visible_idle_agents(cx);
        monocode_core::provider::configure_idle_agents(
            self.workspace.preferences.idle_agents as usize,
        );
        monocode_core::accounts::set_claude_entrypoint(
            self.workspace.preferences.claude_entrypoint,
        );
        if let Some(monos) = &self.monos {
            let prefs = self.workspace.preferences.clone();
            monos.update(cx, |m, cx| m.set_preferences(prefs, cx));
        }
        crate::settings::apply_keybindings(&self.workspace.preferences, cx);
        // After the keymap, so the menu shows rebound shortcuts.
        crate::app_menu::install(&self.workspace.preferences, cx);
        cx.set_global(self.workspace.preferences.clone());
        self.publish_kit_prefs();
        self.sweep_auto_resume(cx);
        self.sync_right_panel(cx);
        if let Some(center) = crate::accounts::get(cx) {
            center.update(cx, |center, cx| {
                let changed = center.mask_emails != self.workspace.preferences.mask_emails
                    || center.show_remaining != self.workspace.preferences.show_remaining_usage;
                center.mask_emails = self.workspace.preferences.mask_emails;
                center.show_remaining = self.workspace.preferences.show_remaining_usage;
                if changed {
                    cx.notify();
                }
            });
        }
        for session in self.sessions.values() {
            session.read(cx).transcript.clone().update(cx, |view, cx| {
                view.apply_preferences(&self.workspace.preferences, cx)
            });
        }
        self.usage_footer.update(cx, |footer, cx| {
            footer.set_remaining(self.workspace.preferences.show_remaining_usage, cx)
        });
    }
    fn choose_chat_background(&mut self, cx: &mut Context<Self>) {
        let prompt = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: None,
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = prompt.await
                && let Some(path) = paths.first()
            {
                let source = path.clone();
                let directory = crate::data_dir().join("backgrounds");
                let work = cx.background_executor().spawn(async move {
                    monocode_core::project_appearance::import_image(
                        &source, &directory, "global", false,
                    )
                });
                let result = work.await;
                let _ = this.update_in(cx, |this, window, cx| match result {
                    Ok(path) => {
                        this.workspace.preferences.chat_background = Some(path);
                        this.persist(cx);
                        this.sync_settings(cx);
                        cx.notify();
                    }
                    Err(error) => {
                        let prompt = window.prompt(
                            gpui::PromptLevel::Critical,
                            "Cannot use this image",
                            Some(&error),
                            &["OK"],
                            cx,
                        );
                        cx.spawn(async move |_, _| {
                            let _ = prompt.await;
                        })
                        .detach();
                    }
                });
            }
        })
        .detach();
    }
    fn finish_worktree_action(
        &mut self,
        path: &str,
        result: Result<(), String>,
        cx: &mut Context<Self>,
    ) {
        if let Some(view) = &self.settings {
            view.update(cx, |v, cx| v.management_finished(result, Some(path), cx));
        }
        self.sync_git(cx);
        self.sync_settings(cx);
        self.invalidate_chrome(cx);
        cx.notify();
    }
    fn remove_settings_worktree(
        &mut self,
        cwd: String,
        path: String,
        delete_sessions: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use monocode_core::files::is_equal_or_inside;
        if self
            .terminals
            .values()
            .flatten()
            .any(|t| is_equal_or_inside(&t.read(cx).cwd, &path))
        {
            self.finish_worktree_action(
                &path,
                Err("Close the terminals using this worktree first.".into()),
                cx,
            );
            return;
        }
        let ids = self
            .session_summaries
            .keys()
            .filter(|id| is_equal_or_inside(self.session_working_copy(**id, cx), &path))
            .copied()
            .collect::<Vec<_>>();
        if ids
            .iter()
            .any(|id| self.sessions.get(id).is_some_and(|v| v.read(cx).busy))
        {
            self.finish_worktree_action(
                &path,
                Err("Stop the running sessions using this worktree first.".into()),
                cx,
            );
            return;
        }
        let check_cwd = cwd.clone();
        let check_path = path.clone();
        let work = cx.background_executor().spawn(async move {
            monocode_core::worktree_management::check(Path::new(&check_cwd), Path::new(&check_path))
        });
        cx.spawn(async move|this,cx|{
            let result=work.await;
            if let Err(e)=result {let _=this.update(cx,|this,cx|this.finish_worktree_action(&path,Err(e),cx));return;}
            let pending=match this.update_in(cx,|this,window,cx| {
                // Recheck before deleting any conversations after the preflight worker.
                if this.terminals.values().flatten().any(|t|is_equal_or_inside(&t.read(cx).cwd,&path))||ids.iter().any(|id|this.sessions.get(id).is_some_and(|s|s.read(cx).busy)) {return Err("This worktree is now in use. Close its terminals and stop its sessions first.".to_owned());}
                if delete_sessions {for id in &ids{this.delete_session(*id,window,cx);}}
                Ok((this.group_writes.take(),this.store.clone()))
            }) {Ok(Ok(v))=>v,Ok(Err(e))=>{let _=this.update(cx,|this,cx|this.finish_worktree_action(&path,Err(e),cx));return;},Err(_)=>return};
            if let Some(task)=pending.0 {task.await;}
            let worker_cwd=cwd.clone();let worker_path=path.clone();let store=pending.1;
            let work=cx.background_executor().spawn(async move{
                if delete_sessions {let store=locked(&store)?;let paths=store.session_worktrees()?;if store.sessions()?.iter().any(|s|is_equal_or_inside(paths.get(&s.id).and_then(|p|p.as_deref()).unwrap_or(&s.cwd),&worker_path)){return Err("Some sessions could not be deleted, so the worktree was kept.".into());}}
                let snapshot=monocode_core::git::snapshot(Path::new(&worker_cwd))?;
                let main=snapshot.worktrees.first().ok_or("No main working copy found")?;
                monocode_core::worktree_management::remove(Path::new(&main.path),Path::new(&worker_path))?;
                locked(&store)?.detach_worktree(&worker_path,&main.path)
            });let result=work.await;
            let _=this.update(cx,|this,cx|{let result=result.map(|detached|{for(summary,settings)in detached{this.session_summaries.insert(summary.id, summary.clone());this.session_worktrees.insert(summary.id,settings.worktree_cwd.clone());if let Some(view)=this.sessions.get(&summary.id){view.update(cx,|v,cx|v.worktree_deleted(summary,settings,cx));}}this.worktree_focus.retain(|_,focus|!is_equal_or_inside(&focus.path,&path));});this.finish_worktree_action(&path,result,cx);});
        }).detach();
        let _ = window;
    }
    fn delete_from_settings(&mut self, id: i64, window: &mut Window, cx: &mut Context<Self>) {
        let prompt = window.prompt(
            gpui::PromptLevel::Warning,
            "Delete this conversation permanently?",
            Some("Its messages and saved settings will be removed."),
            &["Delete", "Cancel"],
            cx,
        );
        cx.spawn(async move |this, cx| {
            if prompt.await != Ok(0) {
                return;
            }
            let _ = this.update_in(cx, |this, window, cx| {
                this.delete_session(id, window, cx);
            });
        })
        .detach();
    }
}
fn this_update(this: &mut Shell, cx: &mut Context<Shell>) {
    this.invalidate_chrome(cx);
    this.update_chat_visibility(cx);
    cx.notify();
}

/// A discovered catalog as the Settings model menus list it: id and name.
pub(super) fn settings_rows(
    kind: Kind,
    models: &[monocode_core::catalog::Model],
) -> Vec<(String, String)> {
    models
        .iter()
        .map(|m| {
            (
                m.id.clone(),
                crate::pickers::model_display_name(kind, &m.id, &m.name).to_string(),
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shell::tests::window;
    use gpui::TestAppContext;

    #[gpui::test]
    fn the_global_auto_resume_switch_arms_waiting_sessions_and_off_leaves_them_armed(
        cx: &mut TestAppContext,
    ) {
        use monocode_core::usage::{AutoResume, Limit, now_ms};
        let (window, a, b) = window(cx);
        let reset = Some(now_ms() + 3_600_000);
        window
            .update(cx, |shell, _, cx| {
                for (id, opt_out) in [(a, false), (b, true)] {
                    let view = shell.ensure_session(id, cx).unwrap();
                    view.update(cx, |v, _| {
                        v.settings.usage_limit = Some(Limit {
                            resets_at: reset,
                            resume_at_reset: false,
                        });
                        v.settings.auto_resume_opt_out = opt_out;
                    });
                    assert_eq!(shell.session_metadata(id, cx).auto_resume(now_ms()), None);
                }
                shell.workspace.preferences.auto_resume_usage_limit = true;
                shell.apply_preferences(cx);
                let armed = |shell: &Shell, id: i64, cx: &gpui::App| {
                    shell.sessions[&id]
                        .read(cx)
                        .settings
                        .usage_limit
                        .as_ref()
                        .is_some_and(|l| l.resume_at_reset)
                };
                assert!(armed(shell, a, cx));
                assert!(!armed(shell, b, cx));
                // The sidebar indicator follows the armed state only.
                assert_eq!(
                    shell.session_metadata(a, cx).auto_resume(now_ms()),
                    reset.map(AutoResume::At)
                );
                assert_eq!(shell.session_metadata(b, cx).auto_resume(now_ms()), None);
                shell.workspace.preferences.auto_resume_usage_limit = false;
                shell.apply_preferences(cx);
                assert!(armed(shell, a, cx));
                assert!(!armed(shell, b, cx));
            })
            .unwrap();
    }

    #[gpui::test]
    fn pairing_dialog_is_a_window_overlay_and_updates_from_settings_notifications(
        cx: &mut TestAppContext,
    ) {
        cx.update(super::super::bind_keys);
        let (window, _, _) = window(cx);
        window
            .update(cx, |shell, w, cx| {
                shell.open_settings(Section::Connections, w, cx)
            })
            .unwrap();
        let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
        visual.run_until_parked();
        assert!(visual.debug_bounds("mobile-pair-overlay").is_none());
        window
            .update(visual, |shell, _, cx| {
                shell.mobile.invite = Some(monocode_core::mobile::Invite {
                    url: format!("monocode://pair?invite={}", "q".repeat(500)),
                    expires: monocode_core::time::now_ms() / 1000 + 900,
                });
                // This notifies Settings alone. The root overlay must observe it.
                shell.sync_settings(cx);
            })
            .unwrap();
        visual.run_until_parked();
        let viewport = window.update(visual, |_, w, _| w.viewport_size()).unwrap();
        let scrim = visual
            .debug_bounds("mobile-pair-overlay")
            .expect("Settings changes show the portal at the shell root");
        assert_eq!(scrim.origin, gpui::point(px(0.), px(0.)));
        assert_eq!(scrim.size, viewport);
        let dialog = visual.debug_bounds("mobile-pair-dialog").unwrap();
        assert_eq!(dialog.size.width, px(560.));
        assert!((dialog.center().x - viewport.width / 2.).abs() < px(1.));
        assert!((dialog.center().y - viewport.height / 2.).abs() < px(1.));
        let qr = visual.debug_bounds("mobile-pair-code").unwrap();
        visual.simulate_click(qr.center(), Default::default());
        visual.run_until_parked();
        assert!(
            visual.debug_bounds("mobile-pair-overlay").is_some(),
            "clicking inside does not dismiss or reach Settings"
        );
        visual.simulate_keystrokes("escape");
        visual.run_until_parked();
        assert!(visual.debug_bounds("mobile-pair-overlay").is_none());
        window
            .update(visual, |shell, _, _| {
                assert!(
                    shell.settings.is_some(),
                    "Escape only dismisses the pairing dialog"
                );
                assert!(shell.mobile.invite.is_none());
            })
            .unwrap();
    }

    #[gpui::test]
    fn a_running_worktree_delete_keeps_settings_open_until_it_reports(cx: &mut TestAppContext) {
        let (window, _, _) = window(cx);
        window
            .update(cx, |shell, w, cx| {
                shell.open_settings(Section::Worktrees, w, cx)
            })
            .unwrap();
        let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
        visual.run_until_parked();
        window
            .update(visual, |shell, w, cx| {
                let view = shell.settings.clone().unwrap();
                view.update(cx, |v, cx| v.start_delete_for_test("/wt/a", w, cx));
                shell.close_settings(w, cx);
                assert!(shell.settings.is_some(), "blocked while deleting");
                shell.select_settings(Section::Archive, cx);
                assert_eq!(view.read(cx).section, Section::Worktrees);
                shell.finish_worktree_action("/wt/a", Ok(()), cx);
                shell.close_settings(w, cx);
                assert!(shell.settings.is_none(), "closes once the delete is done");
            })
            .unwrap();
    }

    #[gpui::test]
    fn settings_skill_form_opens_and_cancels_without_leaving_settings(cx: &mut TestAppContext) {
        cx.update(super::super::bind_keys);
        let (window, _, _) = window(cx);
        window
            .update(cx, |shell, w, cx| {
                shell.open_settings(Section::Skills, w, cx)
            })
            .unwrap();
        let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
        visual.run_until_parked();
        let add = visual.debug_bounds("settings-add-skill").unwrap();
        visual.simulate_click(add.center(), Default::default());
        visual.run_until_parked();
        assert!(visual.debug_bounds("settings-skill-form").is_some());
        assert!(visual.debug_bounds("settings-management-dialog").is_none());
        visual.simulate_keystrokes("escape");
        visual.run_until_parked();
        assert!(visual.debug_bounds("settings-skill-form").is_none());
        window
            .update(visual, |shell, _, _| assert!(shell.settings.is_some()))
            .unwrap();
    }

    #[gpui::test]
    fn escape_closes_the_inner_settings_dialog_first(cx: &mut TestAppContext) {
        cx.update(super::super::bind_keys);
        let (window, _, _) = window(cx);
        window
            .update(cx, |shell, w, cx| shell.open_settings(Section::Mcp, w, cx))
            .unwrap();
        let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
        visual.run_until_parked();
        let add = visual.debug_bounds("settings-add-mcp").unwrap();
        visual.simulate_click(add.center(), Default::default());
        visual.run_until_parked();
        assert!(visual.debug_bounds("settings-management-dialog").is_some());
        visual.simulate_keystrokes("escape");
        visual.run_until_parked();
        window
            .update(visual, |shell, _, _| assert!(shell.settings.is_some()))
            .unwrap();
        assert!(visual.debug_bounds("settings-management-dialog").is_none());
        visual.simulate_keystrokes("escape");
        visual.run_until_parked();
        window
            .update(visual, |shell, _, _| assert!(shell.settings.is_none()))
            .unwrap();
    }
    #[gpui::test]
    fn the_cloud_page_is_always_in_the_rail(cx: &mut TestAppContext) {
        cx.update(|cx| {
            cx.set_reduce_motion(true);
            cx.set_global(crate::cloud_kit_ui::KitScanner(
                crate::cloud_kit_ui::fixture::scan,
            ));
        });
        let (window, _, _) = window(cx);
        window
            .update(cx, |shell, w, cx| {
                shell.open_settings(Section::General, w, cx)
            })
            .unwrap();
        let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
        visual.run_until_parked();
        assert!(visual.debug_bounds("settings-nav-mcp").is_some());
        visual.refresh().unwrap();
        let row = visual.debug_bounds("settings-nav-cloud").unwrap();
        visual.simulate_click(row.center(), Default::default());
        visual.run_until_parked();
        window
            .update(visual, |shell, _, cx| {
                assert_eq!(
                    shell.settings.as_ref().unwrap().read(cx).section,
                    Section::Cloud
                )
            })
            .unwrap();
    }
    #[gpui::test]
    fn settings_nav_search_steps_through_results_with_arrows(cx: &mut TestAppContext) {
        cx.update(|cx| {
            super::super::bind_keys(cx);
            crate::input::bind(cx);
            crate::popover::bind(cx);
        });
        cx.update(|cx| cx.set_reduce_motion(true));
        let (window, _, _) = window(cx);
        window
            .update(cx, |shell, w, cx| {
                shell.open_settings(Section::General, w, cx)
            })
            .unwrap();
        let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
        visual.run_until_parked();
        let back = visual.debug_bounds("settings-nav-back").unwrap();
        let search = visual.debug_bounds("settings-search").unwrap();
        let first = visual.debug_bounds("settings-nav-general").unwrap();
        assert!(search.bottom() < first.top() && first.bottom() < back.top());
        visual.simulate_click(search.center(), Default::default());
        visual.simulate_input("theme");
        visual.run_until_parked();
        assert!(visual.debug_bounds("settings-result-1").is_some());
        visual.simulate_keystrokes("down");
        visual.run_until_parked();
        window
            .update(visual, |shell, _, cx| {
                assert_eq!(
                    shell.settings.as_ref().unwrap().read(cx).search_state().1,
                    1
                )
            })
            .unwrap();
    }
    #[gpui::test]
    fn settings_replace_native_rail_and_tabs_and_return_to_the_same_session(
        cx: &mut TestAppContext,
    ) {
        cx.update(super::super::bind_keys);
        let (window, _, active) = window(cx);
        window
            .update(cx, |shell, w, cx| shell.focus_active(w, cx))
            .unwrap();
        let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
        visual.simulate_keystrokes("cmd-,");
        visual.run_until_parked();
        let card = visual.debug_bounds("main-pane-frame").unwrap();
        let header = visual.debug_bounds("settings-header").unwrap();
        let page = visual.debug_bounds("settings-page-header").unwrap();
        assert!(
            header.bottom() <= px(40.),
            "breadcrumb and search live in the title bar"
        );
        assert!(
            (page.left()
                - card.left()
                - (card.size.width - px(8.) - px(1024.)).max(px(0.)) / 2.
                - px(33.))
            .abs()
                < px(1.),
            "React's 32px page padding plus the card border: page={page:?}, frame={card:?}"
        );
        assert!(visual.debug_bounds("settings-toggle-sounds").is_some());
        assert!(visual.debug_bounds("sidebar-content").is_none());
        for (section, selector) in Section::ALL.into_iter().zip([
            "settings-nav-general",
            "settings-nav-connections",
            "settings-nav-appearance",
            "settings-nav-keybindings",
            "settings-nav-chat",
            "settings-nav-providers",
            "settings-nav-mcp",
            "settings-nav-skills",
            "settings-nav-monos",
            "settings-nav-inbox",
            "settings-nav-archive",
            "settings-nav-worktrees",
        ]) {
            let bounds = visual.debug_bounds(selector).unwrap();
            visual.simulate_click(bounds.center(), Default::default());
            visual.run_until_parked();
            window
                .update(visual, |shell, _, cx| {
                    assert_eq!(shell.settings.as_ref().unwrap().read(cx).section, section)
                })
                .unwrap();
            assert!(
                visual.debug_bounds("settings-page-header").is_some(),
                "page {} renders",
                section.label()
            );
        }
        let back = visual.debug_bounds("settings-nav-back").unwrap();
        visual.simulate_click(back.center(), Default::default());
        visual.run_until_parked();
        window
            .update(visual, |shell, _, _| {
                assert!(shell.settings.is_none());
                assert_eq!(shell.workspace.active, Some(active));
                assert_eq!(shell.workspace.tabs.len(), 2);
            })
            .unwrap();
        assert!(visual.debug_bounds("sidebar-content").is_some());
    }

    #[gpui::test]
    fn settings_nav_collapses_with_search_on_top_and_back_at_the_bottom(cx: &mut TestAppContext) {
        cx.update(super::super::bind_keys);
        cx.update(|cx| cx.set_reduce_motion(true));
        let (window, _, _) = window(cx);
        window
            .update(cx, |shell, w, cx| shell.focus_active(w, cx))
            .unwrap();
        let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
        visual.simulate_keystrokes("cmd-,");
        visual.run_until_parked();
        let back = visual.debug_bounds("settings-nav-back").unwrap();
        let search = visual.debug_bounds("settings-search").unwrap();
        let general = visual.debug_bounds("settings-nav-general").unwrap();
        let header = visual.debug_bounds("settings-header").unwrap();
        assert!(
            search.bottom() <= general.top(),
            "search heads the nav, above the pages"
        );
        assert!(general.bottom() <= back.top(), "Back sits below the pages");
        assert!(search.right() <= header.left(), "search lives in the nav");

        let toggle = visual.debug_bounds("toggle-sidebar").unwrap();
        visual.simulate_click(toggle.center(), Default::default());
        visual.run_until_parked();
        window
            .update(visual, |shell, _, _| {
                assert!(shell.settings.is_some());
                assert_eq!(shell.sidebar_fraction, 0.);
                assert!(
                    shell.workspace.sidebar_visible,
                    "the project sidebar is untouched"
                );
            })
            .unwrap();
        let search = visual.debug_bounds("settings-search").unwrap();
        let header = visual.debug_bounds("settings-header").unwrap();
        assert!(
            search.left() >= header.left() && search.bottom() <= header.bottom(),
            "collapsed, search moves to the title bar"
        );

        let toggle = visual.debug_bounds("toggle-sidebar").unwrap();
        visual.simulate_click(toggle.center(), Default::default());
        visual.run_until_parked();
        window
            .update(visual, |shell, _, _| assert_eq!(shell.sidebar_fraction, 1.))
            .unwrap();
    }

    #[gpui::test]
    fn native_setting_edits_persist_and_theme_switches_live(cx: &mut TestAppContext) {
        let (window, _, _) = window(cx);
        window
            .update(cx, |shell, w, cx| {
                shell.open_settings(Section::Appearance, w, cx)
            })
            .unwrap();
        let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
        visual.run_until_parked();
        let light = visual.debug_bounds("settings-theme-light").unwrap();
        visual.simulate_click(light.center(), Default::default());
        visual.run_until_parked();
        window
            .update(visual, |shell, _, _| {
                assert_eq!(shell.workspace.preferences.theme, "light");
                let saved = shell.latest_state.lock().unwrap().clone();
                let restored: Workspace = serde_json::from_str(&saved).unwrap();
                assert_eq!(restored.preferences.theme, "light");
                assert!(
                    content(1.).l < 0.3,
                    "the native text ink switched to the light palette"
                );
            })
            .unwrap();
        let glass = visual
            .debug_bounds("settings-toggle-main-pane-glass")
            .unwrap();
        assert!(glass.size.width > px(0.));
        let dark = visual.debug_bounds("settings-theme-dark").unwrap();
        visual.simulate_click(dark.center(), Default::default());
        visual.run_until_parked();
    }

    #[gpui::test]
    fn shortcut_recording_rebinds_native_settings_and_reset_restores_it(cx: &mut TestAppContext) {
        cx.update(crate::input::bind);
        cx.update(super::super::bind_keys);
        let (window, _, _) = window(cx);
        window
            .update(cx, |shell, w, cx| {
                shell.open_settings(Section::Keybindings, w, cx)
            })
            .unwrap();
        let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
        visual.run_until_parked();
        let recorder = visual.debug_bounds("shortcut-App: Settings").unwrap();
        visual.simulate_click(recorder.center(), Default::default());
        visual.simulate_keystrokes("cmd-k");
        visual.run_until_parked();
        window
            .update(visual, |shell, _, _| {
                assert!(
                    !shell
                        .workspace
                        .preferences
                        .keybindings
                        .contains_key("App: Settings"),
                    "a default shortcut cannot be claimed twice"
                )
            })
            .unwrap();
        visual.simulate_keystrokes("cmd-alt-y");
        visual.run_until_parked();
        window
            .update(visual, |shell, _, _| {
                assert!(
                    shell.workspace.preferences.keybindings["App: Settings"]
                        .as_ref()
                        .unwrap()
                        .ends_with('y')
                )
            })
            .unwrap();
        visual.simulate_keystrokes("escape");
        visual.run_until_parked();
        window
            .update(visual, |shell, _, _| assert!(shell.settings.is_none()))
            .unwrap();
        visual.simulate_keystrokes("cmd-,");
        visual.run_until_parked();
        window
            .update(visual, |shell, _, _| {
                assert!(shell.settings.is_none(), "old chord is removed")
            })
            .unwrap();
        visual.simulate_keystrokes("cmd-alt-y");
        visual.run_until_parked();
        window
            .update(visual, |shell, _, _| {
                assert!(
                    shell.settings.is_some(),
                    "the newly recorded chord dispatches the native action"
                )
            })
            .unwrap();
        window
            .update(visual, |shell, _, cx| {
                shell.select_settings(Section::Keybindings, cx)
            })
            .unwrap();
        visual.run_until_parked();
        let reset = visual.debug_bounds("reset-shortcut-App: Settings").unwrap();
        visual.simulate_click(reset.center(), Default::default());
        visual.run_until_parked();
        visual.simulate_keystrokes("escape");
        visual.run_until_parked();
        visual.simulate_keystrokes("cmd-,");
        visual.run_until_parked();
        window
            .update(visual, |shell, _, _| {
                assert!(
                    shell.settings.is_some(),
                    "reset restores the native default shortcut"
                )
            })
            .unwrap();
    }
}
