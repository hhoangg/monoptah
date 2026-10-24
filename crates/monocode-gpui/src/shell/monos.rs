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
    pub branch: Option<String>,
    pub model: SharedString,
}
impl Shell {
    pub(crate) fn close_mono_overview(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.set_right_panel_open(false, window, cx);
    }
    #[cfg(test)]
    pub(crate) fn mono_manager(&self) -> Entity<crate::monos::Monos> {
        self.monos.as_ref().unwrap().clone()
    }
    #[cfg(all(target_os = "macos", not(test)))]
    pub(crate) fn mono_quick_compose(&mut self, cx: &mut Context<Self>) {
        self.create_chat(Kind::Codex, cx);
    }
    pub(super) fn mono_active(&self, cx: &App) -> bool {
        self.monos
            .as_ref()
            .is_some_and(|m| m.read(cx).active.is_some())
    }
    pub(crate) fn mono_session_views(&self, cx: &App) -> Vec<MonoSession> {
        self.order
            .iter()
            .map(|&id| {
                let metadata = self.session_metadata(id, cx);
                let live = self.sessions.get(&id).map(|v| v.read(cx));
                MonoSession {
                    id,
                    title: metadata.summary.title.clone(),
                    cwd: metadata.summary.cwd.clone(),
                    busy: metadata.busy || live.is_some_and(|v| v.mono_external_pending()),
                    has_messages: metadata.summary.has_messages,
                    archived: metadata.summary.archived,
                    updated_at: metadata.summary.updated_at,
                    needs_input: metadata.needs_approval(),
                    branch: live.and_then(|v| v.branch_name()).map(str::to_owned),
                    model: live.map(|v| v.mono_model_label()).unwrap_or_default(),
                }
            })
            .collect()
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
    pub(crate) fn mono_install(
        &mut self,
        summary: Session,
        settings: Settings,
        prompt: String,
        draft: bool,
        cx: &mut Context<Self>,
    ) {
        let id = summary.id;
        self.add_loaded(summary, cx);
        let view = self.sessions[&id].clone();
        view.update(cx, |v, cx| {
            v.install_transfer(settings, (!draft).then(|| prompt.clone()), cx);
            if draft {
                v.input.update(cx, |i, cx| i.set_text(prompt, cx));
            }
        });
        self.invalidate_chrome(cx);
    }
    /// Hangs a Mono's launch under the session it was started beneath; the link is saved by the caller.
    pub(crate) fn mono_link_started(&mut self, child: i64, parent: i64, cx: &mut Context<Self>) {
        self.started_by.insert(child, parent);
        self.invalidate_chrome(cx);
    }
    #[cfg(test)]
    pub(crate) fn mono_started_by(&self, id: i64) -> Option<i64> {
        self.started_by.get(&id).copied()
    }
    pub(crate) fn mono_groups(&self, project: &str) -> monocode_core::session_groups::Groups {
        self.groups.get(project).cloned().unwrap_or_default()
    }
    pub(crate) fn mono_apply_groups(
        &mut self,
        project: String,
        groups: monocode_core::session_groups::Groups,
        cx: &mut Context<Self>,
    ) {
        self.groups.insert(project, groups);
        self.invalidate_chrome(cx);
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
            self.toggle_archive(id, cx);
        }
    }
}
