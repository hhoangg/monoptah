//! The three application destinations share one navigation and focus boundary.
use super::*;
use crate::{automations, inbox, notes};

pub(super) use crate::workspace::AppView;

impl Shell {
    pub(super) fn app_view_open(&self) -> bool {
        self.active_app_view.is_some()
    }

    pub(super) fn app_view_label(&self) -> Option<&'static str> {
        self.active_app_view.map(|view| match view {
            AppView::Inbox => "Inbox",
            AppView::Notes => "Notes",
            AppView::Automations => "Automations",
        })
    }

    pub(super) fn app_title_tab(
        &self,
        view: AppView,
        width: f32,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let active = self.active_app_view == Some(view);
        let close = active || width >= TAB_CLOSE_MIN_WIDTH;
        div()
            .id(SharedString::from(format!(
                "app-title-tab-{}",
                view.label()
            )))
            .debug_selector(move || format!("app-title-tab-{}", view.label()))
            .group("app-title-tab")
            .role(Role::Tab)
            .aria_label(view.label())
            .aria_selected(active)
            .cursor_pointer()
            .w(px(width))
            .h(px(TITLE_BAR_HEIGHT))
            .flex_shrink_0()
            .flex()
            .items_center()
            .relative()
            .child(
                title_tab_plate(active, false)
                    .text_color(content(if active { 1. } else { 0.5 }))
                    .w_full()
                    .min_w_0()
                    .when(close && width >= TAB_CLOSE_MIN_WIDTH, |d| d.pr_7())
                    .when(width < 30., |d| d.px(px(((width - 14.) / 2.).max(0.))))
                    .gap(px(6.))
                    .group_hover("app-title-tab", |s| {
                        s.text_color(content(1.)).bg(title_tab_fill(active, true))
                    })
                    .child(icon(view.icon(), 14.))
                    .child(
                        div()
                            .min_w_0()
                            .flex_1()
                            .truncate()
                            .text_size(px(TITLE_TAB_TEXT_SIZE))
                            .child(view.label()),
                    ),
            )
            .when(close, |d| {
                d.child(
                    div()
                        .id(SharedString::from(format!(
                            "close-app-tab-{}",
                            view.label()
                        )))
                        .debug_selector(move || format!("close-app-tab-{}", view.label()))
                        .role(Role::Button)
                        .aria_label(format!("Close {}", view.label()))
                        .absolute()
                        .right_1()
                        .top(px(10.))
                        .size(px(20.))
                        .rounded_sm()
                        .flex()
                        .items_center()
                        .justify_center()
                        .opacity(0.)
                        .group_hover("app-title-tab", |s| s.opacity(1.))
                        .hover(|s| s.bg(content(0.1)))
                        .child(icon("close", 12.))
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_click(cx.listener(move |this, _, window, cx| {
                            cx.stop_propagation();
                            this.close_title_tabs(&[TitleTab::App(view)], None, window, cx);
                        })),
                )
            })
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event, _, cx| {
                    this.start_tab_drag(TitleTab::App(view), event, cx)
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                    this.open_menu(
                        MenuTarget::AppTab(view),
                        Anchor::Point(event.position),
                        window,
                        cx,
                    )
                }),
            )
            .on_mouse_up(
                MouseButton::Middle,
                cx.listener(move |this, _, window, cx| {
                    cx.stop_propagation();
                    this.close_title_tabs(&[TitleTab::App(view)], None, window, cx);
                }),
            )
            .on_click(cx.listener(move |this, _, window, cx| {
                if std::mem::take(&mut this.skip_tab_click) {
                    return;
                }
                this.open_title_tab(&TitleTab::App(view), window, cx);
            }))
    }
    pub(super) fn leave_app_views(&mut self, cx: &mut Context<Self>) {
        if self.active_app_view == Some(AppView::Notes)
            && let Some(notes) = &self.notes
        {
            notes.update(cx, |notes, cx| notes.flush(cx));
        }
        self.active_app_view = None;
        self.invalidate_chrome(cx);
    }

    pub(super) fn close_app_view(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.leave_app_views(cx);
        if self.workspace.active.is_none() {
            let previous = self
                .workspace
                .project_focus
                .get(&self.workspace.project)
                .copied()
                .filter(|id| self.workspace.tab_of(*id).is_some());
            if let Some(id) = previous {
                self.activate_session(id, window, cx);
            }
        }
        self.ensure_project_session(window, cx);
        self.update_chat_visibility(cx);
        self.sync_right_panel(cx);
        self.focus_active(window, cx);
        if let Some(tab) = self.active_title_tab(cx) {
            self.remember_title_tab(tab);
        }
        self.persist(cx);
        cx.notify();
    }

    fn prepare_app_view(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.leave_app_search(cx);
        self.close_preview(cx);
        self.close_menu(cx);
        if self.settings.is_some() {
            self.close_settings(window, cx);
        }
        if let Some(monos) = self.monos.clone() {
            monos.update(cx, |monos, cx| monos.close(cx));
        }
        self.navigation_revision += 1;
        self.review_focused = false;
        self.pending_composer_focus = None;
    }

    fn show_app_view(&mut self, view: AppView, window: &mut Window, cx: &mut Context<Self>) {
        if self.active_app_view != Some(view) {
            self.leave_app_views(cx);
        }
        self.active_app_view = Some(view);
        if !self.workspace.classic_layout {
            self.remember_title_tab(TitleTab::App(view));
            self.reveal_active_tab(cx);
        }
        self.update_chat_visibility(cx);
        self.sync_right_panel(cx);
        self.invalidate_chrome(cx);
        self.focus_app_view(window, cx);
        self.persist(cx);
        cx.notify();
    }

    pub(super) fn app_view_body(&self) -> Option<AnyElement> {
        match self.active_app_view? {
            AppView::Notes => self.notes.clone().map(IntoElement::into_any_element),
            AppView::Inbox => self.inbox.clone().map(IntoElement::into_any_element),
            AppView::Automations => self.automations.clone().map(IntoElement::into_any_element),
        }
    }

    pub(super) fn focus_app_view(&self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let focus = match self.active_app_view {
            Some(AppView::Notes) => self.notes.as_ref().map(|view| view.focus_handle(cx)),
            Some(AppView::Inbox) => self.inbox.as_ref().map(|view| view.focus_handle(cx)),
            Some(AppView::Automations) => {
                self.automations.as_ref().map(|view| view.focus_handle(cx))
            }
            None => return false,
        };
        if let Some(focus) = focus {
            window.focus(&focus, cx);
        }
        true
    }

    pub(super) fn open_notes(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.prepare_app_view(window, cx);
        if self.notes.is_none() {
            let view = cx.new(|cx| notes::Notes::new(self.store.clone(), cx));
            self.app_view_events.push(cx.subscribe_in(
                &view,
                window,
                |this, _, event: &notes::Event, window, cx| match event {
                    notes::Event::Close => this.close_app_view(window, cx),
                    notes::Event::OpenSource(id) => this.open(*id, window, cx),
                    notes::Event::AddToChat(note) => this.note_to_chat(note.clone(), window, cx),
                },
            ));
            self.notes = Some(view);
        }
        if let Some(notes) = &self.notes {
            let cwd = self.workspace.project.clone();
            let projects = self.workspace.projects.clone();
            notes.update(cx, |notes, cx| {
                notes.set_appearances(self.workspace.project_appearance.clone(), cx);
                notes.set_context(cwd, projects, cx);
                notes.refresh(cx);
            });
        }
        self.show_app_view(AppView::Notes, window, cx);
    }

    pub(super) fn open_inbox(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.prepare_app_view(window, cx);
        self.ensure_background_inbox(window, cx);
        if let Some(inbox) = &self.inbox {
            inbox.update(cx, |inbox, cx| inbox.refresh(cx));
        }
        self.show_app_view(AppView::Inbox, window, cx);
    }

    pub(super) fn ensure_background_inbox(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.inbox.is_none() {
            let projects = self.workspace.projects.iter().map(Into::into).collect();
            let cwd = self.workspace.project.clone().into();
            let view = cx.new(|cx| inbox::Inbox::new(self.store.clone(), projects, cwd, cx));
            self.app_view_events.push(cx.subscribe_in(
                &view,
                window,
                |this, _, event: &inbox::Event, window, cx| match event {
                    inbox::Event::Close => this.close_app_view(window, cx),
                    inbox::Event::OpenSession(id) => this.open(*id, window, cx),
                    inbox::Event::StartSession { item, prompt } => {
                        this.start_inbox_session(item.as_ref().clone(), prompt.clone(), cx);
                    }
                    inbox::Event::ItemsChanged(items) => {
                        this.inbox_automation_events(items.clone(), cx)
                    }
                },
            ));
            self.app_view_events
                .push(cx.observe(&view, |this, inbox, cx| {
                    if let Some(automations) = &this.automations {
                        let connections = inbox
                            .read(cx)
                            .connection_statuses()
                            .into_iter()
                            .map(|(provider, (connected, _))| (provider, connected))
                            .collect();
                        automations
                            .update(cx, |view, cx| view.set_trigger_connections(connections, cx));
                    }
                    this.invalidate_chrome(cx);
                    this.sync_settings(cx);
                    cx.notify();
                }));
            self.inbox = Some(view);
        }
        if let Some(inbox) = &self.inbox {
            let projects = self.workspace.projects.iter().map(Into::into).collect();
            let cwd = self.workspace.project.clone().into();
            let busy = self.busy_sessions.clone();
            inbox.update(cx, |inbox, cx| {
                inbox.set_appearances(self.workspace.project_appearance.clone(), cx);
                inbox.set_context(projects, cwd, cx);
                inbox.set_busy(busy, cx);
            });
        }
    }

    pub(super) fn open_automations(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(service) = self.automation_state.service.clone() else {
            self.automation_state.open_requested = Some(self.navigation_revision);
            self.initialize_automations(cx);
            return;
        };
        self.prepare_app_view(window, cx);
        if self.automations.is_none() {
            let cwd = self.workspace.project.clone();
            let projects = self.workspace.projects.clone();
            let view = cx.new(|cx| automations::Automations::new(service, cwd, projects, cx));
            view.update(cx, |view, cx| view.set_database(self.store.clone(), cx));
            self.app_view_events.push(cx.subscribe_in(
                &view,
                window,
                |this, _, event: &automations::Event, window, cx| match event {
                    automations::Event::Close => this.close_app_view(window, cx),
                    automations::Event::Launch(run) => {
                        this.launch_automation(run.as_ref().clone(), true, cx)
                    }
                    automations::Event::OpenSession(id) => {
                        let native = locked(&this.store)
                            .ok()
                            .and_then(|store| store.resolve_existing_session_id(id));
                        if let Some(id) = native {
                            this.open(id, window, cx);
                        }
                    }
                },
            ));
            self.automations = Some(view);
        }
        if let Some(view) = &self.automations {
            let cwd = self.workspace.project.clone();
            let projects = self.workspace.projects.clone();
            let surface_supported = self.surface_supported;
            view.update(cx, |view, cx| {
                view.set_appearances(self.workspace.project_appearance.clone(), cx);
                let connections = self
                    .inbox
                    .as_ref()
                    .map(|inbox| inbox.read(cx).connection_statuses())
                    .unwrap_or_default()
                    .into_iter()
                    .map(|(provider, (connected, _))| (provider, connected))
                    .collect();
                view.set_trigger_connections(connections, cx);
                view.set_terminal_sessions(surface_supported, cx);
                view.set_context(cwd, projects, cx);
                view.refresh(cx);
            });
        }
        self.show_app_view(AppView::Automations, window, cx);
    }

    fn note_to_chat(
        &mut self,
        note: monocode_core::notes::Note,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let cwd = note
            .source_cwd
            .clone()
            .filter(|path| !path.is_empty())
            .unwrap_or_else(|| self.workspace.project.clone());
        let kind = self.default_kind();
        let store = self.store.clone();
        let settings = monocode_core::agent::Settings {
            model: self
                .workspace
                .preferences
                .default_models
                .get(kind.id())
                .cloned(),
            effort: crate::conversation::preferred_effort(kind.id(), cx),
            access: self
                .workspace
                .preferences
                .default_access
                .as_deref()
                .map(monocode_core::agent::AccessMode::from_id)
                .unwrap_or_default(),
            account: crate::accounts::get(cx)
                .map(|accounts| accounts.read(cx).selected(kind, &cwd))
                .unwrap_or_default(),
            ..Default::default()
        };
        self.leave_app_views(cx);
        self.navigation_revision += 1;
        let revision = self.navigation_revision;
        let work = cx.background_executor().spawn(async move {
            let store = locked(&store)?;
            // Always a chat: the app drives this session, whatever the surface preference.
            let mut summary = store.create(kind.id(), &cwd)?;
            if !note.title.trim().is_empty() {
                store.title(summary.id, &note.title)?;
                summary.title = note.title.clone();
            }
            store.save_agent_settings(summary.id, &settings)?;
            Ok::<_, String>((summary, settings, note))
        });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update_in(cx, |this, window, cx| match result {
                Ok((summary, settings, note)) => {
                    let id = summary.id;
                    this.add(summary, cx);
                    this.sessions[&id].update(cx, |view, cx| {
                        view.seed_new_settings(settings, cx);
                        view.input.update(cx, |input, cx| {
                            input.set_text(format!("@note/{} ", note.slug), cx)
                        });
                    });
                    if this.navigation_revision == revision {
                        this.activate_session(id, window, cx);
                    }
                    this.persist(cx);
                }
                Err(error) => {
                    this.error = Some(error);
                    cx.notify();
                }
            });
        })
        .detach();
        self.update_chat_visibility(cx);
        self.focus_active(window, cx);
    }

    fn start_inbox_session(
        &mut self,
        item: monocode_core::inbox::Item,
        prompt: String,
        cx: &mut Context<Self>,
    ) {
        let cwd = if item.project_path.is_empty() {
            self.workspace.project.clone()
        } else {
            item.project_path.clone()
        };
        let kind = self.default_kind();
        let settings = monocode_core::agent::Settings {
            model: self
                .workspace
                .preferences
                .project_providers
                .get(&cwd)
                .and_then(|preferences| preferences.models.get(kind.id()))
                .or(self.workspace.preferences.default_models.get(kind.id()))
                .cloned(),
            effort: crate::conversation::preferred_effort(kind.id(), cx),
            access: self
                .workspace
                .preferences
                .default_access
                .as_deref()
                .map(monocode_core::agent::AccessMode::from_id)
                .unwrap_or_default(),
            account: crate::accounts::get(cx)
                .map(|accounts| accounts.read(cx).selected(kind, &cwd))
                .unwrap_or_default(),
            ..Default::default()
        };
        // applyInboxStart: the provider's surface, kept as a terminal only when its
        // CLI takes a first prompt (the start prompt becomes it); else a chat.
        let preferred =
            monocode_core::surface::automation_surface(kind, &cwd, self.provider_surface(kind));
        let store = self.store.clone();
        self.navigation_revision += 1;
        let revision = self.navigation_revision;
        let work = cx.background_executor().spawn(async move {
            if !std::path::Path::new(&cwd).is_dir() {
                return Err("Choose a project folder before starting work on this item".into());
            }
            let store = locked(&store)?;
            let mut summary = store.create_with_surface(kind.id(), &cwd, preferred)?;
            summary.title = format!("{} · {}", item.reference(), item.title);
            summary.work_item = Some(item.work_item());
            store.title(summary.id, &summary.title)?;
            store.set_work_item(summary.id, summary.work_item.as_ref())?;
            store.save_agent_settings(summary.id, &settings)?;
            Ok::<_, String>((summary, settings, prompt))
        });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update_in(cx, |this, window, cx| match result {
                Ok((summary, settings, prompt)) => {
                    let id = summary.id;
                    let terminal = summary.surface.is_tui();
                    this.add(summary, cx);
                    if terminal {
                        if let Some(tui) = this.tui_sessions.get(&id) {
                            tui.update(cx, |tui, _| tui.set_initial_prompt(prompt));
                        }
                    } else {
                        this.sessions[&id]
                            .update(cx, |view, cx| view.quick_submit(prompt, settings, cx));
                    }
                    // A terminal only runs while shown, so it is always opened; a chat
                    // runs in the background if the user has moved on.
                    if terminal || this.navigation_revision == revision {
                        this.activate_session(id, window, cx);
                    }
                    this.persist(cx);
                }
                Err(error) => {
                    this.error = Some(error);
                    cx.notify();
                }
            });
        })
        .detach();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shell::surface_tests::{NoClis, all_terminal, assert_chat, known, since};
    use gpui::TestAppContext;
    use monocode_core::surface::Surface;

    fn item(dir: &std::path::Path) -> monocode_core::inbox::Item {
        monocode_core::inbox::Item {
            provider: "github".into(),
            kind: "pr".into(),
            repo: "acme/web".into(),
            number: 7,
            title: "Fix retries".into(),
            url: "https://github.com/acme/web/pull/7".into(),
            project_path: dir.to_string_lossy().into_owned(),
            ..Default::default()
        }
    }
    fn dir() -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "monocode-inbox-start-{}",
            monocode_core::mono::new_id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }
    fn start(
        cx: &mut TestAppContext,
        provider: &str,
        surface: Surface,
        prompt: &str,
    ) -> (
        gpui::WindowHandle<Shell>,
        std::collections::HashSet<i64>,
        i64,
    ) {
        let (window, _, _) = crate::shell::tests::window(cx);
        let project = dir();
        let before = window
            .update(cx, |shell, _, cx| {
                shell.workspace.preferences.default_provider = Some(provider.into());
                shell
                    .workspace
                    .preferences
                    .provider_surfaces
                    .insert(provider.into(), surface);
                let before = known(shell);
                shell.start_inbox_session(item(&project), prompt.into(), cx);
                before
            })
            .unwrap();
        cx.run_until_parked();
        let id = window
            .update(cx, |shell, _, _| {
                let ids = since(shell, &before);
                assert_eq!(ids.len(), 1);
                ids[0]
            })
            .unwrap();
        (window, before, id)
    }

    #[gpui::test]
    fn start_work_keeps_a_terminal_when_the_cli_takes_a_first_prompt(cx: &mut TestAppContext) {
        let (window, _, id) = start(cx, "claude", Surface::Tui, "Work on this pull request");
        window
            .update(cx, |shell, _, cx| {
                assert_eq!(shell.session_summaries[&id].surface, Surface::Tui);
                assert!(shell.tui_sessions.contains_key(&id));
                assert!(!shell.sessions.contains_key(&id));
                assert_eq!(shell.workspace.active, Some(id));
                let args: Vec<_> = shell.tui_sessions[&id]
                    .read(cx)
                    .terminal()
                    .unwrap()
                    .read(cx)
                    .launched()
                    .unwrap()
                    .args
                    .iter()
                    .map(|arg| arg.to_string_lossy().into_owned())
                    .collect();
                assert_eq!(
                    args.last().map(String::as_str),
                    Some("Work on this pull request"),
                    "the start prompt is the CLI's first prompt"
                );
                // The session still names and links the work item.
                assert_eq!(shell.session_summaries[&id].title, "#7 · Fix retries");
                assert_eq!(
                    shell.session_summaries[&id]
                        .work_item
                        .as_ref()
                        .map(|item| item.number),
                    Some(7)
                );
            })
            .unwrap();
    }

    #[gpui::test]
    fn start_work_is_a_chat_when_the_cli_cannot_take_a_first_prompt(cx: &mut TestAppContext) {
        let _clis = NoClis::new();
        let (window, _, id) = start(cx, "codex", Surface::Tui, "Work on this pull request");
        window
            .update(cx, |shell, _, _| {
                assert_chat(shell, id);
                assert!(shell.tui_sessions.is_empty());
            })
            .unwrap();
    }

    #[gpui::test]
    fn start_work_follows_a_chat_preference(cx: &mut TestAppContext) {
        let _clis = NoClis::new();
        let (window, _, id) = start(cx, "claude", Surface::Chat, "Work on this pull request");
        window
            .update(cx, |shell, _, _| assert_chat(shell, id))
            .unwrap();
    }

    #[gpui::test]
    fn start_work_is_a_chat_on_a_database_without_terminal_support(cx: &mut TestAppContext) {
        let _clis = NoClis::new();
        let (window, _, _) = crate::shell::tests::window(cx);
        let project = dir();
        let before = window
            .update(cx, |shell, _, cx| {
                shell.workspace.preferences.default_provider = Some("claude".into());
                all_terminal(shell);
                shell.surface_supported = false;
                let before = known(shell);
                shell.start_inbox_session(item(&project), "Work on it".into(), cx);
                before
            })
            .unwrap();
        cx.run_until_parked();
        window
            .update(cx, |shell, _, _| {
                let ids = since(shell, &before);
                assert_eq!(ids.len(), 1);
                assert_chat(shell, ids[0]);
            })
            .unwrap();
    }

    #[gpui::test]
    fn a_note_added_to_a_chat_is_a_chat_whatever_the_preference(cx: &mut TestAppContext) {
        let (window, _, _) = crate::shell::tests::window(cx);
        let before = window
            .update(cx, |shell, window, cx| {
                all_terminal(shell);
                let before = known(shell);
                shell.note_to_chat(
                    monocode_core::notes::Note {
                        id: "note-1".into(),
                        slug: "plan".into(),
                        title: "Plan".into(),
                        body: "Body".into(),
                        tags: Vec::new(),
                        source_session_id: None,
                        source_cwd: Some("/work/monocode".into()),
                        slug_pending: false,
                        created_at: 0,
                        updated_at: 0,
                    },
                    window,
                    cx,
                );
                before
            })
            .unwrap();
        cx.run_until_parked();
        window
            .update(cx, |shell, _, cx| {
                let ids = since(shell, &before);
                assert_eq!(ids.len(), 1);
                assert_chat(shell, ids[0]);
                // The note rides on the chat composer, which a terminal would drop.
                assert_eq!(
                    shell.sessions[&ids[0]].read(cx).input.read(cx).content,
                    "@note/plan "
                );
            })
            .unwrap();
    }
}
