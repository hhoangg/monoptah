//! The cloud setup panel: once a cloud target is chosen in the Workspace menu it sits attached
//! above the composer box, like the background-tasks panel. A header (cloud icon, title, billing
//! note, collapse chevron) and one row per setting. A row shows its label and current value and
//! opens a small menu to change it (or holds an inline field for a branch). Creating happens when
//! the first prompt is sent, see `cloud_target.rs`. The values are this session's overrides of the project's saved defaults,
//! see `cloud_target.rs`.
use super::cloud_branches::CloudRefField;
use super::cloud_target::*;
use super::*;
use gpui::{Div, Stateful};
use monocode_core::codespaces;

const ROW_HEIGHT: f32 = 38.;

/// A plain text hover card.
pub(super) struct Tip(pub SharedString);
impl Render for Tip {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .max_w(px(280.))
            .px_2()
            .py_1()
            .rounded_md()
            .bg(shell_fill())
            .border_1()
            .border_color(content(0.12))
            .text_size(px(12.))
            .text_color(content(0.85))
            .child(self.0.clone())
    }
}

/// The value and chevron on the right of a row. A menu opens against this, not the whole row,
/// so it sits under the value it changes.
fn row_value(id: &'static str, anchor: &AnchorSlot, value: String, locked: bool) -> Stateful<Div> {
    div()
        .id((id, 1usize))
        .debug_selector(move || format!("{id}-anchor"))
        .relative()
        .flex_shrink_0()
        .flex()
        .items_center()
        .gap(px(6.))
        .child(anchor.probe())
        .child(
            div()
                .max_w(px(220.))
                .truncate()
                .text_size(px(12.))
                .text_color(content(if locked { 0.35 } else { 0.55 }))
                .child(value),
        )
        .child(icon("chevron-down", 12.).text_color(content(0.35)))
}

/// One row of the panel: a leading icon tile, the label, and whatever the caller puts on the
/// right.
fn row(id: &'static str, icon_name: &'static str, label: impl Into<SharedString>) -> Stateful<Div> {
    let label: SharedString = label.into();
    div()
        .id(id)
        .debug_selector(move || id.into())
        .role(Role::Group)
        .aria_label(label.clone())
        .relative()
        .min_h(px(ROW_HEIGHT))
        .flex()
        .items_center()
        .gap(px(10.))
        .border_t_1()
        .border_color(content(0.05))
        .child(
            div()
                .size(px(24.))
                .flex_shrink_0()
                .rounded(px(7.))
                .bg(content(0.07))
                .text_color(content(0.6))
                .flex()
                .items_center()
                .justify_center()
                .child(icon(icon_name, 13.)),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_color(content(0.8))
                .child(label),
        )
}

impl Conversation {
    /// The menu behind a settings row: its title and choices, the current one checked.
    pub(super) fn cloud_field_menu(&self, field: CloudField) -> (&'static str, Vec<MenuItem>) {
        let Some(draft) = self.cloud_draft.as_ref() else {
            return ("", Vec::new());
        };
        match field {
            CloudField::Compute => {
                let mut sizes: Vec<(u32, String)> = COMPUTE_CHOICES
                    .iter()
                    .map(|(cpus, memory)| (*cpus, (*memory).to_owned()))
                    .collect();
                // A project default outside the presets stays selectable.
                if !sizes.contains(&(draft.cpus, draft.memory.clone())) {
                    sizes.insert(0, (draft.cpus, draft.memory.clone()));
                }
                let items = sizes
                    .into_iter()
                    .map(|(cpus, memory)| {
                        MenuAction::new(
                            format!("cloud-set:compute:{cpus}:{memory}"),
                            size_label(cpus, &memory),
                        )
                        .checked(cpus == draft.cpus && memory == draft.memory)
                        .into()
                    })
                    .collect();
                ("Compute", items)
            }
            CloudField::Runtime => {
                let mut limits: Vec<String> = RUNTIME_CHOICES
                    .iter()
                    .map(|ttl| (*ttl).to_owned())
                    .collect();
                if !limits.contains(&draft.ttl) {
                    limits.insert(0, draft.ttl.clone());
                }
                let items = limits
                    .into_iter()
                    .map(|ttl| {
                        MenuAction::new(format!("cloud-set:runtime:{ttl}"), runtime_label(&ttl))
                            .checked(ttl == draft.ttl)
                            .into()
                    })
                    .collect();
                ("Runtime limit", items)
            }
            CloudField::Account => {
                let mut items = vec![
                    MenuAction::new("cloud-set:account:", "Default account")
                        .checked(draft.account.is_none())
                        .into(),
                ];
                items.extend(draft.accounts.iter().map(|account| {
                    MenuAction::new(
                        format!("cloud-set:account:{}", account.id),
                        format!("{} \u{b7} {}", account.label, account.kind),
                    )
                    .checked(draft.account.as_deref() == Some(account.id.as_str()))
                    .into()
                }));
                ("Claude account", items)
            }
            CloudField::Machine => {
                let selected = draft
                    .machine
                    .as_deref()
                    .unwrap_or(codespaces::DEFAULT_MACHINE);
                let items = match &draft.machines {
                    Machines::Loaded(machines) => machines
                        .iter()
                        .map(|machine| {
                            let label = if machine.display_name.is_empty() {
                                machine.name.clone()
                            } else {
                                machine.display_name.clone()
                            };
                            MenuAction::new(format!("cloud-set:machine:{}", machine.name), label)
                                .checked(machine.name == selected)
                                .into()
                        })
                        .collect(),
                    Machines::Loading => {
                        vec![
                            MenuAction::new("cloud-set:none", "Loading machines...")
                                .disabled(true)
                                .into(),
                        ]
                    }
                    Machines::Failed => vec![
                        MenuAction::new("cloud-set:machine:", codespaces::DEFAULT_MACHINE)
                            .checked(true)
                            .into(),
                    ],
                };
                ("Machine type", items)
            }
        }
    }

    /// The value shown on the right of a settings row.
    pub(super) fn cloud_field_value(&self, field: CloudField) -> String {
        let Some(draft) = self.cloud_draft.as_ref() else {
            return String::new();
        };
        match field {
            CloudField::Compute => size_label(draft.cpus, &draft.memory),
            CloudField::Runtime => runtime_label(&draft.ttl),
            CloudField::Account => match draft.account.as_deref() {
                None => "Default account".into(),
                Some(id) => draft
                    .accounts
                    .iter()
                    .find(|account| account.id == id)
                    .map_or_else(|| id.to_owned(), |account| account.label.clone()),
            },
            CloudField::Machine => match (&draft.machines, draft.machine.as_deref()) {
                (Machines::Loading, None) => "Loading machines...".into(),
                (machines, picked) => {
                    machine_label(machines, picked.unwrap_or(codespaces::DEFAULT_MACHINE))
                }
            },
        }
    }

    /// A row that opens a menu to change `field`.
    fn cloud_choice_row(
        &self,
        field: CloudField,
        id: &'static str,
        icon_name: &'static str,
        label: &'static str,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let locked = self.cloud_creating();
        let open = self.menu_is(ComposerMenu::CloudField(field));
        row(id, icon_name, label)
            .aria_expanded(open)
            .when(!locked, |d| d.cursor_pointer())
            .child(row_value(
                id,
                &self.cloud_anchors[field.index()],
                self.cloud_field_value(field),
                locked,
            ))
            .on_click(cx.listener(move |this, _, window, cx| {
                if !this.cloud_creating() {
                    this.toggle_menu(ComposerMenu::CloudField(field), window, cx);
                }
            }))
    }

    /// A branch row that opens the branch picker over the repository's branches.
    fn cloud_ref_row(
        &self,
        field: CloudRefField,
        id: &'static str,
        label: &'static str,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let locked = self.cloud_creating();
        row(id, "git-branch", label)
            .aria_expanded(self.cloud_ref_menu_open(field))
            .when(!locked, |d| d.cursor_pointer())
            .child(row_value(
                id,
                &self.cloud_ref_anchors[field.index()],
                self.cloud_ref_value(field, cx),
                locked,
            ))
            .on_click(cx.listener(move |this, _, window, cx| {
                this.toggle_cloud_ref_picker(field, window, cx);
            }))
    }

    /// A row with an inline text field for a branch.
    fn cloud_field_row(
        &self,
        id: &'static str,
        label: &'static str,
        field: &Entity<input::Input>,
    ) -> Stateful<Div> {
        let locked = self.cloud_creating();
        row(id, "git-branch", label).child(
            div()
                .when(locked, |d| d.opacity(0.55))
                .w(px(190.))
                .h(px(26.))
                .flex_shrink_0()
                .rounded_md()
                .border_1()
                .border_color(content(0.1))
                .bg(content(0.03))
                .px(px(8.))
                .flex()
                .items_center()
                .font_family("Menlo")
                .child(field.clone()),
        )
    }

    /// The panel above the composer while a cloud target is chosen, empty otherwise.
    pub(super) fn cloud_target_panel(
        &self,
        _window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(draft) = self.cloud_draft.as_ref() else {
            return div().into_any_element();
        };
        let inset = px(8.);
        let docker = draft.target == CloudTarget::DockerSandbox;
        let collapsed = draft.collapsed;
        let title = format!("Run on {}", draft.target.label());
        let header = div()
            .id("cloud-target-header")
            .debug_selector(|| "cloud-target-header".into())
            .role(Role::Button)
            .aria_label(title.clone())
            .aria_expanded(!collapsed)
            .h(px(30.))
            .flex()
            .items_center()
            .gap(px(8.))
            .cursor_pointer()
            .on_click(cx.listener(|this, _, _, cx| {
                if let Some(draft) = &mut this.cloud_draft {
                    draft.collapsed = !draft.collapsed;
                    cx.notify();
                }
            }))
            .child(
                div()
                    .size(px(16.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(icon("cloud-upload", 14.).text_color(content(0.6))),
            )
            .child(
                div()
                    .flex_shrink_0()
                    .text_color(content(0.8))
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .child(title),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_color(content(0.35))
                    .child(draft.target.billing_note()),
            )
            .child(
                div()
                    .size(px(20.))
                    .flex_shrink_0()
                    .rounded_md()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(content(0.45))
                    .hover(|s| s.bg(content(0.08)).text_color(content(0.9)))
                    .child(icon(
                        if collapsed {
                            "chevron-up"
                        } else {
                            "chevron-down-2"
                        },
                        12.,
                    )),
            );
        let body = div()
            .id("cloud-target-rows")
            .when(docker, |d| {
                d.child(self.cloud_choice_row(
                    CloudField::Compute,
                    "cloud-row-compute",
                    "rocket",
                    "Compute",
                    cx,
                ))
                .child(self.cloud_choice_row(
                    CloudField::Runtime,
                    "cloud-row-runtime",
                    "clock",
                    "Runtime limit",
                    cx,
                ))
                .child(if self.cloud_picks(CloudRefField::Base) {
                    self.cloud_ref_row(CloudRefField::Base, "cloud-row-base", "Base branch", cx)
                } else {
                    self.cloud_field_row("cloud-row-base", "Base branch", &draft.base)
                })
                .child(self.cloud_field_row("cloud-row-branch", "Feature branch", &draft.branch))
                .when(
                    !draft.accounts.is_empty() || draft.account.is_some(),
                    |d| {
                        d.child(self.cloud_choice_row(
                            CloudField::Account,
                            "cloud-row-account",
                            "account-user",
                            "Claude account",
                            cx,
                        ))
                    },
                )
            })
            .when(!docker, |d| {
                d.child(self.cloud_choice_row(
                    CloudField::Machine,
                    "cloud-row-machine",
                    "terminal",
                    "Machine type",
                    cx,
                ))
                .child(if self.cloud_picks(CloudRefField::Branch) {
                    self.cloud_ref_row(CloudRefField::Branch, "cloud-row-branch", "Branch", cx)
                } else {
                    self.cloud_field_row("cloud-row-branch", "Branch", &draft.branch)
                })
            });
        let tail = div()
            .id("cloud-target-tail")
            .children(draft.error.clone().map(|error| {
                div()
                    .id("cloud-target-error")
                    .debug_selector(|| "cloud-target-error".into())
                    .role(Role::Alert)
                    .min_h(px(ROW_HEIGHT))
                    .py(px(8.))
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .border_t_1()
                    .border_color(content(0.05))
                    .text_color(danger_color())
                    .line_height(px(17.))
                    .child(
                        div()
                            .size(px(24.))
                            .flex_shrink_0()
                            .rounded(px(7.))
                            .bg(danger_color().opacity(0.12))
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(icon("alert-circle", 13.).text_color(danger_color())),
                    )
                    .child(div().flex_1().min_w_0().child(error))
            }));
        let card = composer_shelf::card("cloud-target-panel", self.classic_layout)
            .debug_selector(|| "cloud-target-panel".into())
            .role(Role::Group)
            .aria_label(format!("Run on {}", draft.target.label()))
            .child(header)
            .when(!collapsed, |d| d.child(body))
            // The error stays in view while the settings are folded away.
            .child(tail);
        composer_shelf::shelf("cloud-target-shelf")
            .px(inset)
            .child(card)
            .into_any_element()
    }
}
