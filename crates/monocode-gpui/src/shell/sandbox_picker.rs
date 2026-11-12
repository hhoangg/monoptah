//! "Existing instance..." for Docker Sandbox: pick one of the sandboxes the project's cloud
//! sessions already run in and start another session on it. The list comes from the store (the
//! sessions bound to each sandbox, like `SandboxProvider::bound_names`), so nothing talks to
//! `sbx` while the picker opens and a sandbox's state is not shown; the first send wakes a
//! stopped one. Starting is `CloudSessions::attach_session`, the path Mono uses for an existing
//! instance: the project must already have a ready session on it.
use super::cloud::CloudMark;
use super::*;
use gpui::EventEmitter;
use monocode_core::cloud_sessions::{
    CloudBackendKind, CloudRef, CloudSessions, CloudState, HARNESS, ProjectInstance,
};

pub(super) struct Open {
    picker: Entity<SandboxPicker>,
    restore: Option<FocusHandle>,
    _events: Subscription,
}

#[allow(clippy::large_enum_variant)]
enum Picked {
    /// A new session on the sandbox `cloud` of the local project.
    Started {
        summary: Session,
        cloud: CloudRef,
    },
    Cancel,
}

struct SandboxPicker {
    cloud: Arc<CloudSessions>,
    local_project: String,
    /// `None` while the list is read.
    instances: Option<Vec<ProjectInstance>>,
    busy: bool,
    error: String,
    focus: FocusHandle,
}
impl EventEmitter<Picked> for SandboxPicker {}
impl Focusable for SandboxPicker {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl SandboxPicker {
    fn new(cloud: Arc<CloudSessions>, local_project: String, cx: &mut Context<Self>) -> Self {
        let mut this = Self {
            cloud,
            local_project,
            instances: None,
            busy: false,
            error: String::new(),
            focus: cx.focus_handle(),
        };
        this.load(cx);
        this
    }

    /// Reads the sandboxes of the project's sessions on a worker.
    fn load(&mut self, cx: &mut Context<Self>) {
        let cloud = self.cloud.clone();
        let project = self.local_project.clone();
        let work = cx.background_executor().spawn(async move {
            cloud.project_instances(&project, CloudBackendKind::DockerSandbox)
        });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(instances) => this.instances = Some(instances),
                    Err(error) => this.error = error,
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Starts a new session on the sandbox `name`, on a worker.
    fn start(&mut self, name: String, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        self.busy = true;
        self.error.clear();
        let cloud = self.cloud.clone();
        let project = self.local_project.clone();
        let target = CloudRef::sandbox(&name);
        let work = {
            let target = target.clone();
            cx.background_executor()
                .spawn(async move { cloud.attach_session(&project, &target, HARNESS) })
        };
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |this, cx| {
                this.busy = false;
                match result {
                    Ok(summary) => cx.emit(Picked::Started {
                        summary,
                        cloud: target,
                    }),
                    Err(error) => this.error = error,
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn row(&self, ix: usize, instance: &ProjectInstance, cx: &mut Context<Self>) -> AnyElement {
        let enabled = instance.ready && !self.busy;
        let name = instance.cloud.name.clone();
        let detail = if instance.ready {
            format!(
                "{} in this project",
                crate::cloud_presets::plural(instance.sessions as u32, "session")
            )
        } else {
            "Still starting".to_owned()
        };
        div()
            .id(("sandbox-row", ix))
            .debug_selector(move || format!("sandbox-row-{ix}"))
            .role(Role::Button)
            .aria_label(format!("Use sandbox {name}"))
            .w_full()
            .rounded_md()
            .px_2()
            .py(px(6.))
            .flex()
            .items_center()
            .gap_2()
            .text_size(px(12.))
            .text_color(content(if enabled { 0.8 } else { 0.4 }))
            .when(enabled, |d| {
                let name = name.clone();
                d.cursor_pointer()
                    .hover(|s| s.bg(content(0.08)).text_color(content(1.)))
                    .on_click(cx.listener(move |this, _, _, cx| this.start(name.clone(), cx)))
            })
            .child(icon("cloud-upload", 14.).text_color(content(0.5)))
            .child(
                div()
                    .min_w_0()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .child(div().truncate().child(instance.cloud.name.clone()))
                    .child(
                        div()
                            .truncate()
                            .text_size(px(11.))
                            .text_color(content(0.45))
                            .child(detail),
                    ),
            )
            .into_any_element()
    }
}

impl Render for SandboxPicker {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut view = div()
            .id("sandbox-picker")
            .debug_selector(|| "sandbox-picker".into())
            .role(Role::Dialog)
            .aria_label("Existing Docker sandbox")
            .track_focus(&self.focus)
            .p_4()
            .flex()
            .flex_col()
            .gap_3()
            .min_h_0()
            .text_color(content(1.))
            .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, _, cx| {
                if event.keystroke.key == "escape" && !this.busy {
                    cx.stop_propagation();
                    cx.emit(Picked::Cancel);
                }
            }))
            .child(
                div()
                    .text_size(px(13.))
                    .font_weight(FontWeight::MEDIUM)
                    .child("New session in an existing sandbox"),
            )
            .child(
                div()
                    .text_size(px(12.))
                    .line_height(px(17.))
                    .text_color(content(0.55))
                    .child(
                        "The session runs Claude Code in a Docker sandbox this project already \
                         uses. A stopped sandbox starts when you send the first message.",
                    ),
            );
        match &self.instances {
            Some(instances) => {
                let mut rows = div()
                    .id("sandbox-list")
                    .role(Role::List)
                    .aria_label("Existing sandboxes")
                    .max_h(px(220.))
                    .overflow_y_scroll()
                    .rounded_md()
                    .border_1()
                    .border_color(content(0.1))
                    .p_1()
                    .flex()
                    .flex_col();
                if instances.is_empty() {
                    rows = rows.child(
                        div()
                            .px_2()
                            .py(px(6.))
                            .text_size(px(12.))
                            .text_color(content(0.45))
                            .child("No sandboxes for this project yet."),
                    );
                }
                for (ix, instance) in instances.iter().enumerate() {
                    rows = rows.child(self.row(ix, instance, cx));
                }
                view = view.child(rows);
            }
            None if self.error.is_empty() => {
                view = view.child(
                    div()
                        .text_size(px(12.))
                        .text_color(content(0.45))
                        .child("Loading sandboxes\u{2026}"),
                );
            }
            None => {}
        }
        if !self.error.is_empty() {
            view = view.child(
                div()
                    .id("sandbox-error")
                    .role(Role::Alert)
                    .text_size(px(12.))
                    .line_height(px(17.))
                    .text_color(rgb(0xf87171))
                    .child(self.error.clone()),
            );
        }
        view.child(
            div().flex().justify_end().gap_2().child(
                crate::modal::disabled(
                    crate::modal::ghost_button("sandbox-cancel", "Cancel"),
                    self.busy,
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    if !this.busy {
                        cx.emit(Picked::Cancel)
                    }
                })),
            ),
        )
    }
}

impl Shell {
    /// The picker of the sandboxes the sessions of `project` run in.
    pub(super) fn open_sandbox_picker(
        &mut self,
        project: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if project.starts_with("remote://") || self.sandbox_picker.is_some() {
            return;
        }
        let restore = window.focused(cx);
        self.close_menu(cx);
        self.close_preview(cx);
        let picked_project = project.clone();
        let picker = cx.new(|cx| SandboxPicker::new(self.cloud.clone(), project, cx));
        let events = cx.subscribe_in(
            &picker,
            window,
            move |this, _, event: &Picked, window, cx| {
                this.close_sandbox_picker(window, cx);
                if let Picked::Started { summary, cloud } = event {
                    this.open_attached_cloud_session(
                        summary.clone(),
                        cloud.clone(),
                        &picked_project,
                        window,
                        cx,
                    );
                }
                cx.notify();
            },
        );
        window.focus(&picker.focus_handle(cx), cx);
        self.sandbox_picker = Some(Open {
            picker,
            restore,
            _events: events,
        });
        cx.notify();
    }

    pub(super) fn close_sandbox_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(open) = self.sandbox_picker.take() {
            if let Some(restore) = open.restore {
                window.focus(&restore, cx);
            } else {
                self.focus_active(window, cx);
            }
        }
        cx.notify();
    }

    /// Lists a session just attached to an existing instance of `local_project` and opens it.
    /// It is ready at once (no start to follow); the first send wakes a stopped instance.
    fn open_attached_cloud_session(
        &mut self,
        summary: Session,
        cloud: CloudRef,
        local_project: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let id = summary.id;
        self.workspace.add_project(local_project);
        self.cloud_marks.insert(
            id,
            CloudMark {
                local_project: local_project.to_owned(),
                codespace: cloud.name.clone(),
                kind: cloud.kind,
            },
        );
        self.cloud_states
            .entry(cloud.key())
            .or_insert(CloudState::Unknown);
        self.add_loaded(summary, cx);
        self.order.retain(|s| *s != id);
        self.order.insert(0, id);
        self.activate_session(id, window, cx);
        self.forget_cloud_menus(cx);
        self.invalidate_chrome(cx);
        self.refresh_cloud(cx);
        cx.notify();
    }

    pub(super) fn sandbox_picker_modal(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let open = self.sandbox_picker.as_ref()?;
        let viewport = window.viewport_size();
        let width = 480f32.min(viewport.width.as_f32() - 24.);
        let anchor = Anchor::Point(gpui::point(
            px((viewport.width.as_f32() - width) / 2.),
            px((viewport.height.as_f32() * 0.16).max(16.)),
        ));
        let weak = cx.weak_entity();
        Some(
            div()
                .absolute()
                .size_full()
                .child(crate::modal::scrim("sandbox-picker-scrim"))
                .child(
                    popover("sandbox-picker-modal", anchor)
                        .modal_surface(self.workspace.boxy_layout())
                        .width(width)
                        .max_height(viewport.height.as_f32() * 0.8)
                        .gap(0.)
                        .padding(0.)
                        .priority(80)
                        .on_dismiss(move |window, cx| {
                            let _ = weak.update(cx, |this, cx| {
                                let busy = this
                                    .sandbox_picker
                                    .as_ref()
                                    .is_some_and(|o| o.picker.read(cx).busy);
                                if !busy {
                                    this.close_sandbox_picker(window, cx);
                                }
                            });
                        })
                        .child(open.picker.clone()),
                )
                .into_any_element(),
        )
    }

    /// The sandboxes the open picker lists, by name (tests).
    #[cfg(test)]
    pub(super) fn sandbox_picker_names(&self, cx: &App) -> Option<Vec<String>> {
        let open = self.sandbox_picker.as_ref()?;
        let instances = open.picker.read(cx).instances.clone()?;
        Some(instances.into_iter().map(|i| i.cloud.name).collect())
    }
}
