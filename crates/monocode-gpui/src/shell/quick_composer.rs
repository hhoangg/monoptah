use super::*;
impl Shell {
    pub(super) fn configure_quick_composer(&mut self, cx: &mut Context<Self>) {
        if let Some(receiver) = self.quick.configure(&self.workspace.preferences) {
            self.quick.task = Some(cx.spawn(async move |this, cx| {
                while receiver.recv().await.is_ok() {
                    let _ = this.update_in(cx, |this, window, cx| {
                        this.toggle_quick_composer(window, cx)
                    });
                }
            }));
        }
    }
    fn toggle_quick_composer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(handle) = self.quick.window.take()
            && handle
                .update(cx, |_, window, _| window.remove_window())
                .is_ok()
        {
            return;
        }
        let cwd = self.workspace.project.clone();
        let kind = self.default_kind();
        let p = &self.workspace.preferences;
        let initial = crate::quick_composer::Launch {
            cwd: cwd.clone(),
            kind,
            model: p
                .project_providers
                .get(&cwd)
                .and_then(|p| p.models.get(kind.id()))
                .or(p.default_models.get(kind.id()))
                .cloned(),
            access: p
                .default_access
                .as_deref()
                .map(monocode_core::agent::AccessMode::from_id)
                .unwrap_or_default(),
            prompt: String::new(),
            foreground: false,
        };
        let mut choice = self
            .quick
            .last
            .clone()
            .filter(|v| self.workspace.projects.contains(&v.cwd))
            .unwrap_or(initial);
        choice.access = crate::conversation::default_access(cx);
        let projects = self.workspace.projects.clone();
        let store = self.store.clone();
        let bounds = gpui::Bounds::centered(None, gpui::size(px(680.), px(230.)), cx);
        let handle = cx.open_window(
            gpui::WindowOptions {
                window_bounds: Some(gpui::WindowBounds::Windowed(bounds)),
                kind: gpui::WindowKind::PopUp,
                titlebar: None,
                is_resizable: false,
                app_id: Some("dev.usemono.quick-composer".into()),
                ..Default::default()
            },
            move |window, cx| {
                let view = cx.new(|cx| {
                    crate::quick_composer::QuickComposer::new(projects, choice, store, cx)
                });
                view.update(cx, |v, cx| v.focus(window, cx));
                view
            },
        );
        match handle {
            Ok(handle) => {
                let view = handle.update(cx, |_, _, cx| cx.entity()).unwrap();
                self.quick.events = Some(cx.subscribe_in(
                    &view,
                    window,
                    move |this, _, launch: &crate::quick_composer::Launch, window, cx| {
                        this.quick.last = Some(launch.clone());
                        let _ = handle.update(cx, |_, window, _| window.remove_window());
                        this.quick.window = None;
                        this.launch_quick_composer(launch.clone(), window, cx);
                    },
                ));
                self.quick.window = Some(handle);
            }
            Err(e) => self.quick.error = Some(e.to_string()),
        }
    }
    fn launch_quick_composer(
        &mut self,
        launch: crate::quick_composer::Launch,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let store = self.store.clone();
        let choice = launch.clone();
        crate::conversation::remember_access(choice.access, cx);
        let effort = crate::conversation::preferred_effort(choice.kind.id(), cx);
        let account = crate::accounts::get(cx)
            .map(|a| a.read(cx).selected(launch.kind, &launch.cwd))
            .unwrap_or_default();
        let work = cx.background_executor().spawn(async move {
            let store = locked(&store)?;
            let summary = store.create(choice.kind.id(), &choice.cwd)?;
            let settings = monocode_core::agent::Settings {
                account,
                model: choice.model,
                access: choice.access,
                effort,
                ..Default::default()
            };
            store.save_agent_settings(summary.id, &settings)?;
            Ok::<_, String>((summary, settings))
        });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update_in(cx, |this, window, cx| match result {
                Ok((summary, settings)) => {
                    let id = summary.id;
                    if !this.workspace.projects.contains(&launch.cwd) {
                        this.workspace.projects.push(launch.cwd.clone());
                    }
                    this.add(summary, cx);
                    this.sessions[&id]
                        .update(cx, |v, cx| v.quick_submit(launch.prompt, settings, cx));
                    this.order.retain(|i| *i != id);
                    this.order.insert(0, id);
                    if launch.foreground {
                        this.workspace.project = launch.cwd;
                        this.close_settings(window, cx);
                        this.activate_session(id, window, cx);
                        window.activate_window();
                    }
                    this.invalidate_chrome(cx);
                    this.persist(cx);
                    cx.notify();
                }
                Err(e) => {
                    this.error = Some(e);
                    cx.notify();
                }
            });
        })
        .detach();
    }
}
