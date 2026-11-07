//! "New session in Codespace...": pick an existing codespace of the project's
//! repository or create one (branch and machine), then start a cloud session on
//! it. Listing, creating and connecting run on background threads; the
//! progress lines of `prepare_target` are shown while they run.
use super::*;
use crate::settings::widgets::secondary_button;
use gpui::EventEmitter;
use monocode_core::{
    cloud_sessions::{CloudSessions, CloudState, CodespaceChoice, HARNESS, StartRecord, Target},
    codespaces::{self, Codespace, GhRunner, MachineType},
};

pub(super) struct Open {
    picker: Entity<CodespacePicker>,
    restore: Option<FocusHandle>,
    _events: Subscription,
}

#[allow(clippy::large_enum_variant)]
enum Picked {
    Started {
        summary: Session,
        record: StartRecord,
    },
    Cancel,
}

/// Machine selected for a new codespace when the user does not choose.
fn default_machine(machines: &[MachineType]) -> Option<String> {
    machines
        .iter()
        .find(|m| m.name == codespaces::DEFAULT_MACHINE)
        .or_else(|| machines.first())
        .map(|m| m.name.clone())
}

/// The target a picker choice turns into. A new codespace needs a branch.
pub(super) fn target_for(
    choice: CodespaceChoice,
    branch: &str,
    machine: Option<&str>,
) -> Result<Target, String> {
    let branch = branch.trim();
    if choice == CodespaceChoice::New && branch.is_empty() {
        return Err("Enter a branch for the new codespace.".into());
    }
    Ok(Target {
        branch: (!branch.is_empty()).then(|| branch.to_owned()),
        machine: machine.map(str::to_owned),
        codespace: choice,
    })
}

/// Requests the codespace (creating it if asked) and creates the session that will run in it.
/// Takes seconds; the codespace itself comes up in the background (see `cloud_start`). Blocking:
/// call it on a worker.
pub(super) fn begin_cloud_start(
    cloud: &CloudSessions,
    local_project: &str,
    target: Target,
) -> Result<(Session, StartRecord), String> {
    cloud.begin_start(local_project, target, HARNESS)
}

/// Existing codespaces and machine types of the project's repository.
struct Listing {
    repo: String,
    codespaces: Vec<Codespace>,
    machines: Vec<MachineType>,
}

fn load_listing(
    cloud: &CloudSessions,
    gh: &dyn GhRunner,
    local_project: &str,
) -> Result<Listing, String> {
    let repo = cloud.repo_for(local_project)?;
    let codespaces = codespaces::list(gh, &repo)?;
    // The machine list is only a convenience: a repository that does not offer
    // it still creates codespaces with the default machine.
    let machines = codespaces::list_machines(gh, &repo).unwrap_or_default();
    Ok(Listing {
        repo,
        codespaces,
        machines,
    })
}

fn selectable(state: CloudState) -> bool {
    !matches!(state, CloudState::Stopping | CloudState::Unavailable)
}

struct CodespacePicker {
    cloud: Arc<CloudSessions>,
    gh: Arc<dyn GhRunner>,
    local_project: String,
    listing: Option<Listing>,
    machine: Option<String>,
    branch: Entity<input::Input>,
    progress: String,
    busy: bool,
    error: String,
    focus: FocusHandle,
    _events: Vec<Subscription>,
}
impl EventEmitter<Picked> for CodespacePicker {}
impl Focusable for CodespacePicker {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl CodespacePicker {
    fn new(
        cloud: Arc<CloudSessions>,
        gh: Arc<dyn GhRunner>,
        local_project: String,
        cx: &mut Context<Self>,
    ) -> Self {
        let current = crate::conversation::git_branch(std::path::Path::new(&local_project))
            .unwrap_or_default();
        let branch = cx.new(|cx| {
            let mut input = input::Input::field("Branch", "main", content(0.35), cx);
            input.set_font_size(12.);
            input.set_text(current, cx);
            input
        });
        let events = vec![
            cx.subscribe(&branch, |_, _, _: &input::Changed, cx| cx.notify()),
            cx.subscribe(&branch, |this, _, _: &input::Submitted, cx| {
                this.start(CodespaceChoice::New, cx)
            }),
        ];
        let mut this = Self {
            cloud,
            gh,
            local_project,
            listing: None,
            machine: None,
            branch,
            progress: String::new(),
            busy: false,
            error: String::new(),
            focus: cx.focus_handle(),
            _events: events,
        };
        this.load(cx);
        this
    }

    fn load(&mut self, cx: &mut Context<Self>) {
        let cloud = self.cloud.clone();
        let gh = self.gh.clone();
        let project = self.local_project.clone();
        let work = cx
            .background_executor()
            .spawn(async move { load_listing(&cloud, &*gh, &project) });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(listing) => {
                        this.machine = default_machine(&listing.machines);
                        this.listing = Some(listing);
                    }
                    Err(error) => this.error = error,
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn start(&mut self, choice: CodespaceChoice, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let branch = self.branch.read(cx).content.to_string();
        let target = match target_for(choice, &branch, self.machine.as_deref()) {
            Ok(target) => target,
            Err(error) => {
                self.error = error;
                cx.notify();
                return;
            }
        };
        self.busy = true;
        self.error.clear();
        self.progress = "Creating codespace...".into();
        let cloud = self.cloud.clone();
        let project = self.local_project.clone();
        let work = cx
            .background_executor()
            .spawn(async move { begin_cloud_start(&cloud, &project, target) });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |this, cx| {
                this.busy = false;
                match result {
                    Ok((summary, record)) => cx.emit(Picked::Started { summary, record }),
                    Err(error) => this.error = error,
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn codespace_row(
        &self,
        ix: usize,
        codespace: &Codespace,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let state = CloudState::from_gh(&codespace.state);
        let enabled = selectable(state) && !self.busy;
        let name = codespace.name.clone();
        let title = if codespace.display_name.is_empty() {
            codespace.name.clone()
        } else {
            codespace.display_name.clone()
        };
        let detail = [
            codespace.git_status.as_ref().map(|g| g.git_ref.clone()),
            Some(codespace.machine_name.clone()),
        ]
        .into_iter()
        .flatten()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" · ");
        div()
            .id(("codespace-row", ix))
            .debug_selector(move || format!("codespace-row-{ix}"))
            .role(Role::Button)
            .aria_label(format!("Use codespace {title}"))
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
                d.cursor_pointer()
                    .hover(|s| s.bg(content(0.08)).text_color(content(1.)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.start(CodespaceChoice::Named(name.clone()), cx)
                    }))
            })
            .child(icon("cloud-upload", 14.).text_color(content(0.5)))
            .child(
                div()
                    .min_w_0()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .child(div().truncate().child(title))
                    .when(!detail.is_empty(), |d| {
                        d.child(
                            div()
                                .truncate()
                                .text_size(px(11.))
                                .text_color(content(0.45))
                                .child(detail),
                        )
                    }),
            )
            .child(
                div()
                    .flex_shrink_0()
                    .text_size(px(11.))
                    .text_color(content(0.5))
                    .child(super::cloud::indicator(Some(state)).label),
            )
            .into_any_element()
    }
}

impl Render for CodespacePicker {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut view = div()
            .id("codespace-picker")
            .debug_selector(|| "codespace-picker".into())
            .role(Role::Dialog)
            .aria_label("New session in Codespace")
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
                    .child("New session in Codespace"),
            )
            .child(
                div()
                    .text_size(px(12.))
                    .line_height(px(17.))
                    .text_color(content(0.55))
                    .child(match &self.listing {
                        Some(listing) => format!(
                            "The session runs Claude Code in a GitHub codespace of {}. It keeps running when you close Monoptah.",
                            listing.repo
                        ),
                        None => "The session runs Claude Code in a GitHub codespace.".into(),
                    }),
            );
        if self.busy {
            view = view.child(
                div()
                    .id("codespace-progress")
                    .role(Role::Status)
                    .flex()
                    .items_center()
                    .gap_2()
                    .text_size(px(12.))
                    .text_color(content(0.7))
                    .child(icon("cloud-upload", 14.))
                    .child(div().min_w_0().truncate().child(self.progress.clone())),
            );
        } else if let Some(listing) = &self.listing {
            let mut rows = div()
                .id("codespace-list")
                .role(Role::List)
                .aria_label("Existing codespaces")
                .max_h(px(220.))
                .overflow_y_scroll()
                .rounded_md()
                .border_1()
                .border_color(content(0.1))
                .p_1()
                .flex()
                .flex_col();
            if listing.codespaces.is_empty() {
                rows = rows.child(
                    div()
                        .px_2()
                        .py(px(6.))
                        .text_size(px(12.))
                        .text_color(content(0.45))
                        .child("No codespaces for this repository yet."),
                );
            }
            for (ix, codespace) in listing.codespaces.iter().enumerate() {
                rows = rows.child(self.codespace_row(ix, codespace, cx));
            }
            view = view
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(content(0.55))
                        .child("Use an existing codespace"),
                )
                .child(rows)
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(content(0.55))
                        .child("Or create a new one"),
                )
                .child(
                    div()
                        .h_8()
                        .rounded_md()
                        .border_1()
                        .border_color(content(0.1))
                        .bg(content(0.03))
                        .px(px(10.))
                        .flex()
                        .items_center()
                        .font_family("Menlo")
                        .child(self.branch.clone()),
                );
            if !listing.machines.is_empty() {
                view = view.child(div().flex().flex_wrap().gap_2().children(
                    listing.machines.iter().map(|machine| {
                        let name = machine.name.clone();
                        let label = if machine.display_name.is_empty() {
                            machine.name.clone()
                        } else {
                            machine.display_name.clone()
                        };
                        secondary_button(
                            SharedString::from(format!("codespace-machine-{name}")),
                            label.clone(),
                            false,
                            false,
                        )
                        .when(self.machine.as_deref() == Some(name.as_str()), |d| {
                            d.bg(content(0.12))
                        })
                        .child(label)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.machine = Some(name.clone());
                            cx.notify();
                        }))
                    }),
                ));
            }
        } else if self.error.is_empty() {
            view = view.child(
                div()
                    .text_size(px(12.))
                    .text_color(content(0.45))
                    .child("Loading codespaces…"),
            );
        }
        if !self.error.is_empty() {
            view = view.child(
                div()
                    .id("codespace-error")
                    .role(Role::Alert)
                    .text_size(px(12.))
                    .line_height(px(17.))
                    .text_color(rgb(0xf87171))
                    .child(self.error.clone()),
            );
        }
        view.child(
            div()
                .flex()
                .justify_end()
                .gap_2()
                .child(
                    crate::modal::disabled(
                        crate::modal::ghost_button("codespace-cancel", "Cancel"),
                        self.busy,
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        if !this.busy {
                            cx.emit(Picked::Cancel)
                        }
                    })),
                )
                .when(self.listing.is_some(), |d| {
                    d.child(
                        crate::modal::disabled(
                            crate::modal::primary_button(
                                "codespace-create",
                                if self.busy {
                                    "Working…"
                                } else {
                                    "Create and open"
                                },
                            ),
                            self.busy,
                        )
                        .on_click(
                            cx.listener(|this, _, _, cx| this.start(CodespaceChoice::New, cx)),
                        ),
                    )
                }),
        )
    }
}

impl Shell {
    /// Whether the project can host a codespace: a local folder.
    pub(super) fn can_open_codespace(&self) -> bool {
        !self.workspace.project.starts_with("remote://")
    }

    pub(super) fn open_codespace_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.can_open_codespace() || self.codespace_picker.is_some() {
            return;
        }
        let restore = window.focused(cx);
        self.close_menu(cx);
        self.close_preview(cx);
        let project = self.workspace.project.clone();
        let picker = cx.new(|cx| {
            CodespacePicker::new(self.cloud.clone(), self.gh.clone(), project.clone(), cx)
        });
        let events = cx.subscribe_in(
            &picker,
            window,
            move |this, _, event: &Picked, window, cx| {
                this.close_codespace_picker(window, cx);
                match event {
                    Picked::Started { summary, record } => {
                        this.open_cloud_start(summary.clone(), record.clone(), window, cx)
                    }
                    Picked::Cancel => {}
                }
                cx.notify();
            },
        );
        window.focus(&picker.focus_handle(cx), cx);
        self.codespace_picker = Some(Open {
            picker,
            restore,
            _events: events,
        });
        cx.notify();
    }

    pub(super) fn close_codespace_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(open) = self.codespace_picker.take() {
            if let Some(restore) = open.restore {
                window.focus(&restore, cx);
            } else {
                self.focus_active(window, cx);
            }
        }
        cx.notify();
    }

    pub(super) fn codespace_picker_modal(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let open = self.codespace_picker.as_ref()?;
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
                .child(crate::modal::scrim("codespace-picker-scrim"))
                .child(
                    popover("codespace-picker-modal", anchor)
                        .modal_surface(self.workspace.boxy_layout())
                        .width(width)
                        .max_height(viewport.height.as_f32() * 0.8)
                        .gap(0.)
                        .padding(0.)
                        .priority(80)
                        .on_dismiss(move |window, cx| {
                            let _ = weak.update(cx, |this, cx| {
                                let busy = this
                                    .codespace_picker
                                    .as_ref()
                                    .is_some_and(|o| o.picker.read(cx).busy);
                                if !busy {
                                    this.close_codespace_picker(window, cx);
                                }
                            });
                        })
                        .child(open.picker.clone()),
                )
                .into_any_element(),
        )
    }
}
