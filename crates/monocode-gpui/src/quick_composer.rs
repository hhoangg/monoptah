//! The floating prompt window opened by the Settings global shortcut.
use crate::{
    input::{self, Input},
    pickers::{
        ModelList, ModelPicked, ProjectPicked, ProjectPicker, model_display_name, model_name,
    },
    popover::{Anchor, AnchorSlot, Side, popover},
    theme::*,
};
use gpui::{
    App, Context, Entity, EventEmitter, FocusHandle, Focusable, Render, Role, Subscription, Task,
    Window, WindowHandle, div, prelude::*, px,
};
use monocode_core::{agent::AccessMode, provider::Kind};
#[cfg(all(target_os = "macos", not(test)))]
mod hotkey;
#[derive(Clone)]
pub(crate) struct Launch {
    pub cwd: String,
    pub kind: Kind,
    pub model: Option<String>,
    pub access: AccessMode,
    pub prompt: String,
    pub foreground: bool,
}
#[derive(Default)]
pub(crate) struct Controller {
    pub window: Option<WindowHandle<QuickComposer>>,
    pub last: Option<Launch>,
    pub error: Option<String>,
    pub task: Option<Task<()>>,
    pub events: Option<Subscription>,
    config: String,
    #[cfg(all(target_os = "macos", not(test)))]
    registration: Option<hotkey::Registration>,
}
impl Controller {
    pub fn configure(
        &mut self,
        prefs: &crate::settings::Preferences,
    ) -> Option<async_channel::Receiver<()>> {
        let shortcut = match prefs.keybindings.get("App: Quick Composer") {
            Some(Some(v)) => v.clone(),
            Some(None) => String::new(),
            None => "cmd-shift-space".into(),
        };
        let config = format!("{}:{shortcut}", prefs.quick_composer);
        if self.config == config {
            return None;
        }
        self.config = config;
        self.error = None;
        self.task = None;
        #[cfg(all(target_os = "macos", not(test)))]
        {
            self.registration = None;
            if prefs.quick_composer && !shortcut.is_empty() {
                match hotkey::Registration::new(&shortcut) {
                    Ok((registration, rx)) => {
                        self.registration = Some(registration);
                        return Some(rx);
                    }
                    Err(e) => self.error = Some(e),
                }
            }
        }
        None
    }
}
pub(crate) struct QuickComposer {
    input: Entity<Input>,
    completion: Entity<crate::composer_completion::Completion>,
    projects: Vec<String>,
    cwd: String,
    kind: Kind,
    model: Option<String>,
    access: AccessMode,
    project_picker: Option<Entity<ProjectPicker>>,
    models: Option<Entity<ModelList>>,
    access_open: bool,
    access_anchor: AnchorSlot,
    project_anchor: AnchorSlot,
    model_anchor: AnchorSlot,
    _subscriptions: Vec<Subscription>,
    picker_events: Option<Subscription>,
}
impl EventEmitter<Launch> for QuickComposer {}
impl QuickComposer {
    pub fn new(
        projects: Vec<String>,
        choice: Launch,
        store: crate::runtime::Database,
        cx: &mut Context<Self>,
    ) -> Self {
        let input = cx.new(|cx| {
            let mut input = Input::new(cx);
            input.set_font_size(16.);
            input.set_placeholder("Ask, build, or describe a change…", cx);
            input
        });
        let completion = cx.new(|cx| {
            crate::composer_completion::Completion::new(input.clone(), choice.cwd.clone(), cx)
                .with_store(store)
        });
        completion.update(cx, |completion, cx| {
            completion.set_provider(choice.kind.id(), cx)
        });
        let subscriptions = vec![
            cx.subscribe(&input, |this, _, _: &input::Submitted, cx| {
                this.launch(false, cx)
            }),
            cx.observe(&input, |_, _, cx| cx.notify()),
            cx.subscribe(
                &completion,
                |this, _, picked: &crate::composer_completion::ModePicked, cx| {
                    let text = this.input.read(cx).content.trim_start().to_owned();
                    this.input.update(cx, |input, cx| {
                        input.set_text(format!("/{} {text}", picked.0), cx)
                    });
                    cx.notify();
                },
            ),
        ];
        Self {
            input,
            completion,
            projects,
            cwd: choice.cwd,
            kind: choice.kind,
            model: choice.model,
            access: choice.access,
            project_picker: None,
            models: None,
            access_open: false,
            access_anchor: Default::default(),
            project_anchor: Default::default(),
            model_anchor: Default::default(),
            _subscriptions: subscriptions,
            picker_events: None,
        }
    }
    fn launch(&mut self, foreground: bool, cx: &mut Context<Self>) {
        if self.input.read(cx).is_composing() {
            return;
        }
        let prompt = self.input.read(cx).content.trim().to_owned();
        if prompt.is_empty() {
            return;
        }
        cx.emit(Launch {
            cwd: self.cwd.clone(),
            kind: self.kind,
            model: self.model.clone(),
            access: self.access,
            prompt,
            foreground,
        });
    }
    pub fn focus(&self, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.input.focus_handle(cx), cx);
        window.activate_window();
    }
    fn projects(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.models = None;
        if self.project_picker.take().is_some() {
            cx.notify();
            return;
        }
        let picker = cx.new(|cx| ProjectPicker::new(&self.projects, &self.cwd, cx));
        self.picker_events = Some(cx.subscribe_in(
            &picker,
            window,
            |this, _, picked: &ProjectPicked, window, cx| {
                let ProjectPicked::Project(path) = picked else {
                    this.choose_project(cx);
                    return;
                };
                this.cwd = path.clone();
                let p = cx.global::<crate::settings::Preferences>();
                this.kind = p
                    .project_providers
                    .get(&this.cwd)
                    .and_then(|v| v.provider.as_deref())
                    .or(p.default_provider.as_deref())
                    .map(Kind::from_id)
                    .unwrap_or(Kind::Codex);
                this.model = p
                    .project_providers
                    .get(&this.cwd)
                    .and_then(|v| v.models.get(this.kind.id()))
                    .or(p.default_models.get(this.kind.id()))
                    .cloned();
                this.completion.update(cx, |c, cx| {
                    c.set_cwd(&this.cwd, cx);
                    c.set_provider(this.kind.id(), cx);
                });
                this.project_picker = None;
                window.focus(&this.input.focus_handle(cx), cx);
                cx.notify();
            },
        ));
        window.focus(&picker.read(cx).search_focus(cx), cx);
        self.project_picker = Some(picker);
        cx.notify();
    }
    fn choose_project(&mut self, cx: &mut Context<Self>) {
        self.project_picker = None;
        let prompt = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Choose project".into()),
        });
        cx.spawn(async move |this, cx| {
            let result = prompt.await;
            let _ = this.update_in(cx, |this, window, cx| {
                if let Ok(Ok(Some(paths))) = result
                    && let Some(path) = paths.first()
                {
                    this.cwd = path.to_string_lossy().into_owned();
                    if !this.projects.contains(&this.cwd) {
                        this.projects.push(this.cwd.clone());
                    }
                    let p = cx.global::<crate::settings::Preferences>();
                    this.kind = p
                        .default_provider
                        .as_deref()
                        .map(Kind::from_id)
                        .unwrap_or(Kind::Codex);
                    this.model = p.default_models.get(this.kind.id()).cloned();
                    this.completion.update(cx, |c, cx| {
                        c.set_cwd(&this.cwd, cx);
                        c.set_provider(this.kind.id(), cx);
                    });
                }
                window.focus(&this.input.focus_handle(cx), cx);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
    fn models(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.project_picker = None;
        if self.models.take().is_some() {
            cx.notify();
            return;
        }
        let profile = crate::accounts::get(cx)
            .map(|a| a.read(cx).selected(self.kind, &self.cwd))
            .unwrap_or_default();
        let list = cx.new(|cx| {
            ModelList::new_for(self.kind, self.model.clone(), &self.cwd, profile, cx)
                .hide_providers(
                    cx.global::<crate::settings::Preferences>()
                        .hidden_for(&self.cwd),
                )
        });
        self.picker_events = Some(cx.subscribe_in(
            &list,
            window,
            |this, _, picked: &ModelPicked, window, cx| {
                this.kind = picked.0;
                this.model = picked.1.clone();
                this.completion.update(cx, |completion, cx| {
                    completion.set_provider(this.kind.id(), cx)
                });
                this.models = None;
                window.focus(&this.input.focus_handle(cx), cx);
                cx.notify();
            },
        ));
        window.focus(&list.read(cx).search_focus(cx), cx);
        self.models = Some(list);
        cx.notify();
    }
}
impl Focusable for QuickComposer {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.input.focus_handle(cx)
    }
}
impl Render for QuickComposer {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut root = div()
            .id("quick-composer")
            .size_full()
            .p_4()
            .rounded_xl()
            .bg(shell_fill())
            .text_color(content(1.))
            .font_family(crate::fonts::UI)
            .border_1()
            .border_color(content(0.15))
            .flex()
            .flex_col()
            .gap_3()
            .capture_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, window, cx| {
                if event.keystroke.key == "escape" {
                    if this.models.take().is_some() || this.project_picker.take().is_some() {
                        window.focus(&this.input.focus_handle(cx), cx);
                        cx.notify();
                    } else {
                        window.remove_window();
                    }
                    cx.stop_propagation();
                } else if event.keystroke.key == "enter" && event.keystroke.modifiers.platform {
                    this.launch(true, cx);
                    cx.stop_propagation();
                }
            }))
            .child(
                crate::settings::widgets::secondary_button(
                    "quick-project",
                    "Choose project",
                    false,
                    false,
                )
                .relative()
                .child(self.project_anchor.probe())
                .child(icon("folder", 14.))
                .child(
                    std::path::Path::new(&self.cwd)
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned(),
                )
                .child(icon("chevron-down", 12.))
                .on_click(cx.listener(|this, _, window, cx| this.projects(window, cx))),
            )
            .child(div().flex_1().min_h_0().child(self.completion.clone()))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        crate::settings::widgets::secondary_button(
                            "quick-model",
                            "Choose model",
                            false,
                            false,
                        )
                        .relative()
                        .child(self.model_anchor.probe())
                        .child(provider_icon(self.kind.id(), 16.))
                        .child(model_display_name(
                            self.kind,
                            self.model.as_deref().unwrap_or(""),
                            &model_name(self.kind, self.model.as_deref()),
                        ))
                        .child(icon("chevron-down", 12.))
                        .on_click(cx.listener(|this, _, window, cx| this.models(window, cx))),
                    )
                    .child(
                        crate::settings::widgets::secondary_button(
                            "quick-access",
                            "Change access",
                            false,
                            false,
                        )
                        .relative()
                        .child(self.access_anchor.probe())
                        .child(icon("square-lock", 14.))
                        .child(self.access.label())
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.access_open = !this.access_open;
                            cx.notify();
                        })),
                    )
                    .child(div().flex_1())
                    .child(
                        div()
                            .text_size(px(10.))
                            .text_color(content(0.35))
                            .child("↵ start · ⌘↵ open"),
                    )
                    .child(
                        crate::settings::widgets::primary_button(
                            "quick-send",
                            "Start session",
                            self.input.read(cx).content.trim().is_empty(),
                        )
                        .child(icon("arrow-up", 14.))
                        .on_click(cx.listener(|this, _, _, cx| this.launch(false, cx))),
                    ),
            );
        if self.access_open {
            let weak = cx.weak_entity();
            root = root.child(
                popover(
                    "quick-access-options",
                    Anchor::Slot(self.access_anchor.clone()),
                )
                .side(Side::Top)
                .width(280.)
                .on_dismiss(move |_, cx| {
                    let _ = weak.update(cx, |this, cx| {
                        this.access_open = false;
                        cx.notify();
                    });
                })
                .child(
                    div().p_1().children(
                        [
                            AccessMode::ReadOnly,
                            AccessMode::Supervised,
                            AccessMode::AutoAcceptEdits,
                            AccessMode::Auto,
                            AccessMode::FullAccess,
                        ]
                        .map(|mode| {
                            div()
                                .id(gpui::SharedString::from(mode.id()))
                                .role(Role::Button)
                                .aria_label(mode.label())
                                .px_3()
                                .py_2()
                                .rounded_md()
                                .text_size(px(12.))
                                .hover(|s| s.bg(content(0.1)))
                                .child(mode.label())
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.access = mode;
                                    this.access_open = false;
                                    window.focus(&this.input.focus_handle(cx), cx);
                                    cx.notify();
                                }))
                        }),
                    ),
                ),
            );
        }
        if let Some(picker) = &self.project_picker {
            let weak = cx.weak_entity();
            root = root.child(
                popover("quick-projects", Anchor::Slot(self.project_anchor.clone()))
                    .side(Side::Bottom)
                    .width(320.)
                    .on_dismiss(move |window, cx| {
                        let _ = weak.update(cx, |this, cx| {
                            this.project_picker = None;
                            window.focus(&this.input.focus_handle(cx), cx);
                            cx.notify();
                        });
                    })
                    .child(picker.clone()),
            );
        }
        if let Some(models) = &self.models {
            let weak = cx.weak_entity();
            root = root.child(
                popover("quick-models", Anchor::Slot(self.model_anchor.clone()))
                    .side(Side::Top)
                    .width(310.)
                    .on_dismiss(move |window, cx| {
                        let _ = weak.update(cx, |this, cx| {
                            this.models = None;
                            window.focus(&this.input.focus_handle(cx), cx);
                            cx.notify();
                        });
                    })
                    .child(div().h(px(442.)).child(models.clone())),
            );
        }
        root
    }
}
