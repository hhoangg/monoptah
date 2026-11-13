//! The cloud setup panel: once a cloud target is chosen in the Workspace menu it sits attached
//! above the composer box, like the background-tasks panel. A header (cloud icon, backend name,
//! billing note, a pill that says whether anything differs from the project defaults, collapse
//! chevron) and one row per setting, the same rows for both backends (`CloudRow`). A row shows
//! its label and current value and opens a small menu to change it (or holds an inline field for
//! a branch). A row marks the project default as "default"; one the user changed is highlighted
//! and marked "changed for this session". Creating happens when the first prompt is sent, see
//! `cloud_target.rs`. The values are this session's overrides of the project's saved defaults.
use super::cloud_branches::CloudRefField;
use super::cloud_target::*;
use super::*;
use crate::cloud_presets as presets;
use gpui::{Div, Stateful};
use monocode_core::codespaces;

/// What a row says about itself on the right.
const DEFAULT_MARK: &str = "default";
const CHANGED_MARK: &str = "changed for this session";
const MARK_WIDTH: f32 = 150.;

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
fn row(
    id: &'static str,
    icon_name: &'static str,
    label: impl Into<SharedString>,
    changed: bool,
) -> Stateful<Div> {
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
        .when(changed, |d| d.bg(warning_color().opacity(0.08)))
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

/// The header pill: whether anything differs from the project defaults.
pub(super) fn pill_text(changed: usize) -> String {
    if changed > 0 {
        format!("{changed} changed")
    } else {
        "project defaults".to_owned()
    }
}

/// The right end of a row: "default", or "changed for this session" when it differs.
fn mark(id: &'static str, changed: bool) -> Stateful<Div> {
    div()
        .id((id, 2usize))
        .debug_selector(move || format!("{id}-mark"))
        .w(px(MARK_WIDTH))
        .flex_shrink_0()
        .truncate()
        .text_right()
        .text_size(px(11.))
        .text_color(if changed {
            warning_color()
        } else {
            content(0.3)
        })
        .child(if changed { CHANGED_MARK } else { DEFAULT_MARK })
}

/// `options` plus `extra` when it is not among them, so the project's own value and the
/// current one stay selectable when they are not presets.
fn with_value<T: PartialEq + Clone>(mut options: Vec<T>, extra: &[&T]) -> Vec<T> {
    for value in extra {
        if !options.contains(value) {
            options.push((*value).clone());
        }
    }
    options
}

impl Conversation {
    /// The menu behind a settings row: its title and choices, the current one checked and the
    /// project's own marked "default".
    pub(super) fn cloud_field_menu(&self, field: CloudField) -> (&'static str, Vec<MenuItem>) {
        let Some(draft) = self.cloud_draft.as_ref() else {
            return ("", Vec::new());
        };
        let defaults = &draft.defaults;
        let item = |id: String, label: String, checked: bool, is_default: bool| -> MenuItem {
            let mut action = MenuAction::new(id, label).checked(checked);
            if is_default {
                action = action.description(DEFAULT_MARK);
            }
            action.into()
        };
        match field {
            CloudField::Size => {
                let current = (draft.cpus, draft.memory.clone());
                let project = (defaults.cpus, defaults.memory.clone());
                let items = with_value(
                    presets::size_options(draft.cpus, &draft.memory),
                    &[&project, &current],
                )
                .into_iter()
                .map(|(cpus, memory)| {
                    item(
                        format!("cloud-set:size:{cpus}:{memory}"),
                        presets::size_label(cpus, &memory),
                        (cpus, &memory) == (draft.cpus, &draft.memory),
                        (cpus, &memory) == (defaults.cpus, &defaults.memory),
                    )
                })
                .collect();
                ("Size", items)
            }
            CloudField::Runtime => {
                let items = with_value(
                    presets::runtime_options(&draft.ttl),
                    &[&defaults.ttl, &draft.ttl],
                )
                .into_iter()
                .map(|ttl| {
                    item(
                        format!("cloud-set:runtime:{ttl}"),
                        presets::runtime_label(&ttl),
                        ttl == draft.ttl,
                        ttl == defaults.ttl,
                    )
                })
                .collect();
                ("Runtime limit", items)
            }
            CloudField::Idle => {
                let items = with_value(
                    presets::idle_options(draft.idle),
                    &[&defaults.idle, &draft.idle],
                )
                .into_iter()
                .map(|minutes| {
                    item(
                        format!("cloud-set:idle:{minutes}"),
                        presets::idle_label(minutes),
                        minutes == draft.idle,
                        minutes == defaults.idle,
                    )
                })
                .collect();
                ("Stop when idle", items)
            }
            CloudField::Claude => {
                let items = presets::claude_choices(
                    self.cloud_claude_none_label(),
                    &draft.accounts,
                    draft.account.as_deref(),
                    true,
                )
                .into_iter()
                .map(|choice| {
                    let is_default = choice.id == defaults.claude;
                    item(
                        format!("cloud-set:claude:{}", choice.id.as_deref().unwrap_or("")),
                        choice.label,
                        choice.selected,
                        is_default,
                    )
                })
                .collect();
                ("Claude account", items)
            }
            CloudField::DockerAccount => {
                let mut items = vec![item(
                    "cloud-set:docker:".into(),
                    presets::MACHINE_SIGN_IN_LABEL.into(),
                    draft.docker_account.is_none(),
                    defaults.docker_account.is_none(),
                )];
                let accounts = draft.docker_accounts.as_deref().unwrap_or_default();
                items.extend(accounts.iter().map(|account| {
                    item(
                        format!("cloud-set:docker:{}", account.id),
                        presets::docker_account_label(account),
                        draft.docker_account.as_deref() == Some(account.id.as_str()),
                        defaults.docker_account.as_deref() == Some(account.id.as_str()),
                    )
                }));
                if let Some(id) = &draft.docker_account
                    && draft.docker_accounts.is_some()
                    && !accounts.iter().any(|account| &account.id == id)
                {
                    items.push(item(
                        format!("cloud-set:docker:{id}"),
                        format!("{id} (no longer exists)"),
                        true,
                        defaults.docker_account.as_ref() == Some(id),
                    ));
                }
                ("Docker account", items)
            }
            CloudField::Machine => {
                let selected = draft
                    .machine
                    .as_deref()
                    .unwrap_or(codespaces::DEFAULT_MACHINE);
                let project = defaults
                    .machine
                    .as_deref()
                    .unwrap_or(codespaces::DEFAULT_MACHINE);
                let items = match &draft.machines {
                    Machines::Loaded(machines) => machines
                        .iter()
                        .map(|machine| {
                            item(
                                format!("cloud-set:machine:{}", machine.name),
                                presets::machine_label(machine),
                                machine.name == selected,
                                machine.name == project,
                            )
                        })
                        .collect(),
                    Machines::Loading => {
                        vec![
                            MenuAction::new("cloud-set:none", "Loading machines...")
                                .disabled(true)
                                .into(),
                        ]
                    }
                    // The listing failed: offer the machine the row shows (the project's, else
                    // the default), so it stays selectable.
                    Machines::Failed => vec![item(
                        format!(
                            "cloud-set:machine:{}",
                            draft.machine.as_deref().unwrap_or("")
                        ),
                        selected.to_owned(),
                        true,
                        selected == project,
                    )],
                };
                ("Machine", items)
            }
        }
    }

    /// What "no account" is called in the Claude account row of the chosen backend.
    fn cloud_claude_none_label(&self) -> &'static str {
        match self.cloud_target() {
            Some(CloudTarget::Codespace) => presets::CODESPACE_SECRET_LABEL,
            _ => presets::SANDBOX_NO_CLAUDE_LABEL,
        }
    }

    /// The value shown on the right of a settings row.
    pub(super) fn cloud_field_value(&self, field: CloudField) -> String {
        let Some(draft) = self.cloud_draft.as_ref() else {
            return String::new();
        };
        match field {
            CloudField::Size => presets::size_label(draft.cpus, &draft.memory),
            CloudField::Runtime => presets::runtime_label(&draft.ttl),
            CloudField::Idle => presets::idle_label(draft.idle),
            CloudField::Claude => match draft.account.as_deref() {
                None => self.cloud_claude_none_label().into(),
                Some(id) => draft
                    .accounts
                    .iter()
                    .find(|account| account.id == id)
                    .map_or_else(
                        || presets::unavailable_account_label(id),
                        |account| account.label.clone(),
                    ),
            },
            CloudField::DockerAccount => match draft.docker_account.as_deref() {
                None => presets::MACHINE_SIGN_IN_LABEL.into(),
                Some(id) => draft
                    .docker_accounts
                    .as_deref()
                    .unwrap_or_default()
                    .iter()
                    .find(|account| account.id == id)
                    .map_or_else(
                        || {
                            // Until the list is read the id is all there is; after it, an
                            // account that is not in it reads like the menu's entry.
                            if draft.docker_accounts.is_some() {
                                presets::unavailable_account_label(id)
                            } else {
                                id.to_owned()
                            }
                        },
                        |account| account.docker_id.clone(),
                    ),
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
        row_kind: CloudRow,
        field: CloudField,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let locked = self.cloud_creating();
        let open = self.menu_is(ComposerMenu::CloudField(field));
        let (id, changed) = (row_kind.id(), self.cloud_row_changed(row_kind));
        row(id, row_kind.icon(), row_kind.label(), changed)
            .aria_expanded(open)
            .when(!locked, |d| d.cursor_pointer())
            .child(row_value(
                id,
                &self.cloud_anchors[field.index()],
                self.cloud_field_value(field),
                locked,
            ))
            .child(mark(id, changed))
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
        row_kind: CloudRow,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let locked = self.cloud_creating();
        let (id, changed) = (row_kind.id(), self.cloud_row_changed(row_kind));
        row(id, row_kind.icon(), row_kind.label(), changed)
            .aria_expanded(self.cloud_ref_menu_open(field))
            .when(!locked, |d| d.cursor_pointer())
            .child(row_value(
                id,
                &self.cloud_ref_anchors[field.index()],
                self.cloud_ref_value(field, cx),
                locked,
            ))
            .child(mark(id, changed))
            .on_click(cx.listener(move |this, _, window, cx| {
                this.toggle_cloud_ref_picker(field, window, cx);
            }))
    }

    /// "Claude setup: 6 skills, 4 MCP servers, 3 plugins - Settings..." and the switch for this
    /// start.
    fn cloud_kit_row(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        use crate::cloud_kit_ui as kit;
        let prefs = kit::kit_prefs(cx);
        let globally = kit::default_on(&prefs);
        let locked = self.cloud_creating();
        let draft = self.cloud_draft.as_ref();
        let on = globally && draft.is_some_and(|d| d.kit);
        let summary = draft.map_or(kit::Summary::Loading, |d| d.kit_summary);
        let label = format!("Claude setup: {}", kit::summary_text(&summary, globally));
        row("cloud-row-kit", "sparkles", label, false)
            .child(
                kit::settings_link("cloud-kit-settings")
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(OpenCloudSettings))),
            )
            .child(
                kit::switch(
                    "cloud-kit-switch",
                    "cloud-kit-switch",
                    "Copy my Claude setup to this workspace",
                    on,
                    locked || !globally,
                )
                .when(!locked && globally, |d| {
                    d.on_click(cx.listener(|this, _, _, cx| this.toggle_cloud_kit(cx)))
                }),
            )
    }

    /// A row with an inline text field for a branch.
    fn cloud_field_row(&self, row_kind: CloudRow, field: &Entity<input::Input>) -> Stateful<Div> {
        let locked = self.cloud_creating();
        let (id, changed) = (row_kind.id(), self.cloud_row_changed(row_kind));
        // The working branch has no project default to mark, but it says so when it names
        // another branch than the base.
        let marked = row_kind != CloudRow::Branch || changed;
        row(id, row_kind.icon(), row_kind.label(), changed)
            .child(
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
            .child(if marked {
                mark(id, changed)
            } else {
                div().id((id, 2usize)).w(px(MARK_WIDTH)).flex_shrink_0()
            })
    }

    /// The row's value differs from the project default.
    fn cloud_row_changed(&self, row: CloudRow) -> bool {
        self.cloud_draft
            .as_ref()
            .is_some_and(|draft| draft.row_changed(row))
    }

    /// The row `row` of the panel.
    fn cloud_row(&self, row: CloudRow, draft: &CloudDraft, cx: &mut Context<Self>) -> AnyElement {
        match row {
            CloudRow::Base if self.cloud_picks(CloudRefField::Base) => self
                .cloud_ref_row(CloudRefField::Base, row, cx)
                .into_any_element(),
            CloudRow::Base => self.cloud_field_row(row, &draft.base).into_any_element(),
            CloudRow::Branch => self.cloud_field_row(row, &draft.branch).into_any_element(),
            CloudRow::Kit => self.cloud_kit_row(cx).into_any_element(),
            _ => match row.field() {
                Some(field) => self.cloud_choice_row(row, field, cx).into_any_element(),
                None => div().into_any_element(),
            },
        }
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
        let collapsed = draft.collapsed;
        let title = draft.target.label();
        let changed = draft.changed_count();
        let header = div()
            .id("cloud-target-header")
            .debug_selector(|| "cloud-target-header".into())
            .role(Role::Button)
            .aria_label(format!("Run on {title}"))
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
                    .id("cloud-target-pill")
                    .debug_selector(|| "cloud-target-pill".into())
                    .flex_shrink_0()
                    .px(px(8.))
                    .rounded_full()
                    .bg(if changed > 0 {
                        warning_color().opacity(0.14)
                    } else {
                        content(0.07)
                    })
                    .text_size(px(11.))
                    .text_color(if changed > 0 {
                        warning_color()
                    } else {
                        content(0.5)
                    })
                    .child(pill_text(changed)),
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
        let body = div().id("cloud-target-rows").children(
            CloudRow::of(draft.target)
                .iter()
                .map(|row| self.cloud_row(*row, draft, cx))
                .collect::<Vec<_>>(),
        );
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
