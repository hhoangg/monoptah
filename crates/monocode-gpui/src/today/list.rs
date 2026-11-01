//! The Tasks tab: filters, New task field, groups and rows.
//! Also home to the small pieces `detail` and `sources` share: due and priority
//! wording, and the menus.
use super::{Filters, Today, short_day};
use crate::{
    inbox::{caption, label_badge, message},
    popover::{Anchor, AnchorSlot, Menu, MenuAction, MenuEvent, MenuItem, MenuStyle, menu_popover},
    theme::*,
};
use gpui::{
    AnyElement, Context, Div, Entity, FontWeight, Role, SharedString, Stateful, Subscription,
    Window, div, prelude::*, px,
};
use monocode_core::{
    inbox::Label,
    today::{AGE_WARNING_DAYS, CheckReason, DayMode, Group, Priority, Row, Task},
};

/// `Nov 2` or `Nov 1, 17:00`; `None` without a due date.
pub(super) fn due_text(task: &Task) -> Option<String> {
    let day = short_day(task.due.as_deref()?);
    Some(match task.due_time.as_deref().filter(|t| !t.is_empty()) {
        Some(time) => format!("{day}, {time}"),
        None => day,
    })
}
pub(super) fn priority_label(priority: Priority) -> &'static str {
    match priority {
        Priority::Normal => "Normal",
        Priority::High => "High",
        Priority::Urgent => "Urgent",
    }
}

/// `3d open`; nothing for a task first seen today.
pub(super) fn age_label(age_days: u32) -> Option<String> {
    (age_days > 0).then(|| format!("{age_days}d open"))
}
/// Old and still not done reads in the warning color.
pub(super) fn age_warns(age_days: u32, group: Group) -> bool {
    age_days >= AGE_WARNING_DAYS && group != Group::Done
}
pub(super) fn check_label(reason: CheckReason) -> &'static str {
    match reason {
        CheckReason::Missing => "Left the list without a clear reason",
        CheckReason::DoneButSourceOpen => "You marked it done, source is still open",
    }
}
/// Why the task is here. A task that needs checking says why it does, which
/// matters more than how it first arrived; core keeps that only in `check_reason`.
pub(super) fn why_here(task: &Task) -> String {
    if let Some(reason) = task.check_reason {
        return check_label(reason).to_string();
    }
    let reason = task.reason.trim();
    if reason.is_empty() && task.source_id.is_none() {
        return "Added by you".to_string();
    }
    reason.to_string()
}
/// Source, why it is here and the source's own status, joined with a middle dot.
pub(super) fn source_line(task: &Task) -> String {
    [
        task.source_name.as_str(),
        why_here(task).as_str(),
        task.source_status.as_str(),
    ]
    .into_iter()
    .map(str::trim)
    .filter(|part| !part.is_empty())
    .collect::<Vec<_>>()
    .join(" · ")
}
/// The latest note on one line, cut so a long note cannot widen the row.
pub(super) fn note_preview(body: &str) -> String {
    const LIMIT: usize = 90;
    let text = body.split_whitespace().collect::<Vec<_>>().join(" ");
    if text.chars().count() <= LIMIT {
        return text;
    }
    let cut = text.chars().take(LIMIT).collect::<String>();
    format!("{}…", cut.trim_end())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Ring {
    Open,
    High,
    Urgent,
    NeedsCheck,
    Done,
}
/// How the round check reads: done and needs check win over priority.
pub(super) fn ring_state(group: Group, priority: Priority) -> Ring {
    match (group, priority) {
        (Group::Done, _) => Ring::Done,
        (Group::NeedsCheck, _) => Ring::NeedsCheck,
        (_, Priority::Urgent) => Ring::Urgent,
        (_, Priority::High) => Ring::High,
        _ => Ring::Open,
    }
}

fn group_title(group: Group) -> &'static str {
    match group {
        Group::CarriedOver => "CARRIED OVER",
        Group::NeedsCheck => "NEEDS CHECK",
        Group::Today => "TODAY",
        Group::Done => "DONE",
        Group::Snoozed => "SNOOZED",
        Group::Dismissed => "DISMISSED",
    }
}
const CURRENT_GROUPS: [Group; 4] = [
    Group::CarriedOver,
    Group::NeedsCheck,
    Group::Today,
    Group::Done,
];
const PAST_GROUPS: [Group; 6] = [
    Group::CarriedOver,
    Group::NeedsCheck,
    Group::Today,
    Group::Done,
    Group::Snoozed,
    Group::Dismissed,
];
/// The New task placeholder: a future day is named, so it is clear where the task lands.
pub(super) fn new_task_placeholder(mode: DayMode, day: &str) -> String {
    match mode {
        DayMode::Future => format!("New task for {}", short_day(day)),
        _ => "New task".to_string(),
    }
}

/// What a picked menu row does.
#[derive(Clone)]
pub(super) enum Pick {
    Filters(Filters),
    Snooze { id: String, day: String },
    Priority { id: String, priority: Priority },
    Project { id: String, project: Option<String> },
}
/// Which trigger a menu hangs from; also where its popover is drawn.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Slot {
    ProjectFilter,
    SourceFilter,
    MonoFilter,
    Snooze,
    Priority,
    Project,
}
impl Slot {
    fn in_filters(self) -> bool {
        matches!(
            self,
            Self::ProjectFilter | Self::SourceFilter | Self::MonoFilter
        )
    }
}
pub(super) struct OpenMenu {
    menu: Entity<Menu>,
    slot: Slot,
    picks: Vec<(SharedString, Pick)>,
    _events: Subscription,
}
/// The one open dropdown, and the anchors the triggers write while they lay out.
#[derive(Default)]
pub(crate) struct Menus {
    open: Option<OpenMenu>,
    anchors: [AnchorSlot; 6],
}
impl Menus {
    #[cfg(test)]
    pub(super) fn is_open(&self) -> bool {
        self.open.is_some()
    }
}
/// Menu rows with what each one does.
#[derive(Default)]
pub(super) struct Rows {
    items: Vec<MenuItem>,
    picks: Vec<(SharedString, Pick)>,
}
impl Rows {
    pub(super) fn add(&mut self, action: MenuAction, pick: Pick) {
        self.picks.push((action.id.clone(), pick));
        self.items.push(action.into());
    }
}

fn empty(text: &'static str) -> gpui::Div {
    message(text).debug_selector(|| "today-empty".into())
}
/// The compact select-like trigger of the filters row.
fn filter_trigger(id: &'static str, label: String, active: bool, open: bool) -> Stateful<Div> {
    div()
        .id(id)
        .debug_selector(|| id.to_string())
        .role(Role::Button)
        .aria_label(label.clone())
        .aria_expanded(open)
        .relative()
        .max_w(px(200.))
        .h(px(24.))
        .px(px(8.))
        .flex()
        .items_center()
        .gap(px(4.))
        .rounded(px(6.))
        .text_size(px(12.))
        .cursor_pointer()
        .bg(content(if active { 0.1 } else { 0.05 }))
        .text_color(content(if active { 1. } else { 0.7 }))
        .hover(|s| s.bg(content(0.1)))
        .child(div().min_w_0().truncate().child(label))
        .child(icon("chevron-down", 12.))
}

impl Today {
    /// Opens `rows` under the trigger of `slot`, or closes it when it already is.
    /// Uses the app's `Menu`, so it takes focus, answers the arrow keys, Enter
    /// and Escape, and scrolls past the popover's height cap.
    pub(super) fn open_menu(
        &mut self,
        slot: Slot,
        label: &'static str,
        rows: Rows,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self
            .menus
            .open
            .as_ref()
            .is_some_and(|open| open.slot == slot)
        {
            self.menus.open = None;
            cx.notify();
            return;
        }
        let items = rows.items;
        let menu = cx.new(|cx| {
            Menu::new(label, items, window, cx)
                .style(MenuStyle::Filter)
                .width(240.)
        });
        let events = cx.subscribe(&menu, |this, _, event: &MenuEvent, cx| {
            let picked = match event {
                MenuEvent::Picked(id, _) => this.menus.open.as_ref().and_then(|open| {
                    open.picks
                        .iter()
                        .find(|(row, _)| row == id)
                        .map(|(_, pick)| pick.clone())
                }),
                MenuEvent::Dismissed => None,
            };
            this.menus.open = None;
            if let Some(pick) = picked {
                this.apply_pick(pick, cx);
            }
            cx.notify();
        });
        self.menus.open = Some(OpenMenu {
            menu,
            slot,
            picks: rows.picks,
            _events: events,
        });
        cx.notify();
    }
    fn apply_pick(&mut self, pick: Pick, cx: &mut Context<Self>) {
        match pick {
            Pick::Filters(filters) => self.set_filters(filters, cx),
            Pick::Snooze { id, day } => self.snooze(id, day, cx),
            Pick::Priority { id, priority } => self.patch(
                id,
                monocode_core::today::TaskPatch {
                    priority: Some(priority),
                    ..Default::default()
                },
                cx,
            ),
            Pick::Project { id, project } => self.patch(
                id,
                monocode_core::today::TaskPatch {
                    project: Some(project),
                    ..Default::default()
                },
                cx,
            ),
        }
    }
    /// The open menu's popover, drawn by whichever of the list or the panel owns its trigger.
    pub(super) fn menu_view(&self, in_filters: bool, cx: &gpui::App) -> Option<AnyElement> {
        let open = self
            .menus
            .open
            .as_ref()
            .filter(|open| open.slot.in_filters() == in_filters)?;
        let anchor = self.menus.anchors[open.slot as usize].clone();
        Some(menu_popover(&open.menu, Anchor::Slot(anchor), cx).into_any_element())
    }
    pub(super) fn menus_open(&self, slot: Slot) -> bool {
        self.menus
            .open
            .as_ref()
            .is_some_and(|open| open.slot == slot)
    }
    pub(super) fn anchor(&self, slot: Slot) -> AnchorSlot {
        self.menus.anchors[slot as usize].clone()
    }
    /// A project's folder row in a menu, tinted with the project's own color.
    pub(super) fn project_row(
        &self,
        id: String,
        label: String,
        path: &str,
        checked: bool,
    ) -> MenuAction {
        let color = self
            .appearances
            .get(path)
            .cloned()
            .unwrap_or_default()
            .color(path);
        MenuAction::new(id, label)
            .icon("folder")
            .icon_color(gpui::rgb(color).into())
            .checked(checked)
    }
    /// The project badge of a row or a source: the project's own color, or a stable one.
    pub(super) fn project_badge(&self, path: &str, compact: bool) -> Div {
        let color = self
            .appearances
            .get(path)
            .cloned()
            .unwrap_or_default()
            .color(path);
        label_badge(
            &Label {
                name: self.project_label(path),
                color: format!("#{color:06x}"),
            },
            compact,
        )
    }
    /// Projects a task can belong to: the Monos' projects, the ones with an
    /// appearance and the ones on screen, by display name.
    pub(super) fn project_options(&self) -> Vec<(String, String)> {
        let mut paths = std::collections::BTreeSet::new();
        paths.extend(self.roster.iter().flat_map(|mono| mono.projects.clone()));
        paths.extend(self.appearances.keys().cloned());
        if let Some(view) = &self.view {
            paths.extend(view.rows.iter().filter_map(|row| row.task.project.clone()));
        }
        let mut options = paths
            .into_iter()
            .map(|path| (self.project_label(&path), path))
            .collect::<Vec<_>>();
        options.sort_by_key(|(label, _)| label.to_lowercase());
        options
    }
    pub(super) fn mono_mark(&self, id: &str, side: f32) -> Option<AnyElement> {
        let mono = self.mono(id)?;
        Some(crate::monos::avatar::avatar(
            &mono.mascot,
            mono.color,
            "idle",
            side,
            true,
        ))
    }

    /// The scrolling list below the page header and warnings.
    pub(super) fn list_view(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(view) = &self.view else {
            return message("Loading Today…").into_any_element();
        };
        let (mode, day) = (view.mode, view.day.clone());
        let readonly = mode == DayMode::Past;
        let placeholder = new_task_placeholder(mode, &day);
        // Idempotent: only a changed placeholder repaints the field.
        self.new_task
            .update(cx, |input, cx| input.set_placeholder(placeholder, cx));
        let rows = self.visible_rows();
        let filtered = self.filters != Filters::default();

        let mut body = div().flex().flex_col().gap(px(1.));
        if !readonly {
            body = body.child(
                div()
                    .debug_selector(|| "today-new-task".into())
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .h(px(34.))
                    .px(px(8.))
                    .rounded(px(6.))
                    .child(
                        div()
                            .size(px(16.))
                            .flex_shrink_0()
                            .rounded_full()
                            .border_1()
                            .border_color(content(0.3)),
                    )
                    .child(div().flex_1().min_w_0().child(self.new_task.clone())),
            );
        }

        if mode == DayMode::Future {
            if rows.is_empty() {
                body = body.child(empty(if filtered {
                    "No tasks match these filters."
                } else {
                    "Nothing planned yet. Snooze a task or add one here."
                }));
            } else {
                body = body.child(self.group(&rows, None, "PLANNED", readonly, cx));
            }
        } else {
            let groups: &[Group] = if readonly {
                &PAST_GROUPS
            } else {
                &CURRENT_GROUPS
            };
            for group in groups {
                body =
                    body.child(self.group(&rows, Some(*group), group_title(*group), readonly, cx));
            }
            if rows.is_empty() && readonly {
                if filtered {
                    body = body.child(empty("No tasks match these filters."));
                }
            } else if rows.is_empty() {
                body = body.child(empty(if filtered {
                    "No tasks match these filters."
                } else {
                    "Nothing here yet. Add a task above or ask a Mono to sync a source."
                }));
            }
        }

        div()
            .size_full()
            .min_h_0()
            .flex()
            .flex_col()
            .child(self.filters_row(cx))
            .child(
                div()
                    .id("today-list-scroll")
                    .role(Role::List)
                    .aria_label("Tasks")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px(px(12.))
                    .pb(px(16.))
                    .child(body),
            )
            .children(self.menu_view(true, cx))
            .into_any_element()
    }

    /// A caption with its count, then the rows. `group` `None` takes every row.
    fn group(
        &self,
        rows: &[&Row],
        group: Option<Group>,
        title: &'static str,
        readonly: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let rows = rows
            .iter()
            .filter(|row| group.is_none_or(|g| row.group == g))
            .collect::<Vec<_>>();
        if rows.is_empty() {
            return div().into_any_element();
        }
        div()
            .flex()
            .flex_col()
            .gap(px(1.))
            .child(
                caption(title)
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .child(rows.len().to_string()),
            )
            .children(rows.iter().map(|row| self.row_view(row, readonly, cx)))
            .into_any_element()
    }

    fn filters_row(&self, cx: &mut Context<Self>) -> AnyElement {
        let filters = self.filters.clone();
        let open = |slot: Slot| self.menus.open.as_ref().is_some_and(|o| o.slot == slot);
        let project_label = match &filters.project {
            None => "All projects".to_string(),
            Some(None) => "No project".to_string(),
            Some(Some(path)) => self.project_label(path),
        };
        let source_label = match &filters.source {
            None => "All sources".to_string(),
            Some(None) => "Manual".to_string(),
            Some(Some(id)) => self
                .sources
                .iter()
                .find(|s| &s.id == id)
                .map_or_else(|| id.clone(), |s| s.name.clone()),
        };
        let mono_label = filters.mono.as_ref().map_or("All Monos".to_string(), |id| {
            self.mono(id).map_or_else(|| id.clone(), |m| m.name.clone())
        });
        let trigger = |slot: Slot, id: &'static str, label: String, active: bool| {
            filter_trigger(id, label, active, open(slot))
                .child(self.anchor(slot).probe())
                .on_click(cx.listener(move |this, _, window, cx| {
                    let rows = this.filter_rows(slot);
                    this.open_menu(slot, "Filter tasks", rows, window, cx);
                }))
        };
        div()
            .flex_shrink_0()
            .min_h(px(36.))
            .px(px(12.))
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(6.))
            .child(trigger(
                Slot::ProjectFilter,
                "today-filter-project",
                project_label,
                filters.project.is_some(),
            ))
            .child(trigger(
                Slot::SourceFilter,
                "today-filter-source",
                source_label,
                filters.source.is_some(),
            ))
            .child(trigger(
                Slot::MonoFilter,
                "today-filter-mono",
                mono_label,
                filters.mono.is_some(),
            ))
            .into_any_element()
    }

    /// The rows of one filter menu, built when it opens so they are never stale.
    pub(super) fn filter_rows(&self, slot: Slot) -> Rows {
        let f = &self.filters;
        let with = |change: &dyn Fn(&mut Filters)| {
            let mut next = f.clone();
            change(&mut next);
            Pick::Filters(next)
        };
        let mut rows = Rows::default();
        match slot {
            Slot::ProjectFilter => {
                rows.add(
                    MenuAction::new("project-all", "All projects").checked(f.project.is_none()),
                    with(&|n| n.project = None),
                );
                rows.add(
                    MenuAction::new("project-none", "No project").checked(f.project == Some(None)),
                    with(&|n| n.project = Some(None)),
                );
                for (ix, (label, path)) in self.project_options().into_iter().enumerate() {
                    let checked = f.project == Some(Some(path.clone()));
                    rows.add(
                        self.project_row(format!("project-{ix}"), label, &path, checked),
                        with(&|n| n.project = Some(Some(path.clone()))),
                    );
                }
            }
            Slot::SourceFilter => {
                rows.add(
                    MenuAction::new("source-all", "All sources").checked(f.source.is_none()),
                    with(&|n| n.source = None),
                );
                rows.add(
                    MenuAction::new("source-manual", "Manual").checked(f.source == Some(None)),
                    with(&|n| n.source = Some(None)),
                );
                for (ix, source) in self.sources.iter().enumerate() {
                    rows.add(
                        MenuAction::new(format!("source-{ix}"), source.name.clone())
                            .checked(f.source == Some(Some(source.id.clone()))),
                        with(&|n| n.source = Some(Some(source.id.clone()))),
                    );
                }
            }
            _ => {
                rows.add(
                    MenuAction::new("mono-all", "All Monos").checked(f.mono.is_none()),
                    with(&|n| n.mono = None),
                );
                for (ix, mono) in self.roster.iter().enumerate() {
                    rows.add(
                        MenuAction::new(format!("mono-{ix}"), mono.name.clone())
                            .checked(f.mono.as_deref() == Some(mono.id.as_str())),
                        with(&|n| n.mono = Some(mono.id.clone())),
                    );
                }
            }
        }
        rows
    }

    /// The round check. A past day's is static: nothing there can be changed.
    fn check_view(&self, row: &Row, readonly: bool, cx: &mut Context<Self>) -> AnyElement {
        let ring = ring_state(row.group, row.task.priority);
        let key = row.task.key.clone();
        let done = ring == Ring::Done;
        let glyph = match ring {
            Ring::NeedsCheck => icon("circle-dashed-hi", 16.)
                .text_color(warning_color())
                .into_any_element(),
            _ => div()
                .size_full()
                .rounded_full()
                .border_1()
                .flex()
                .items_center()
                .justify_center()
                .border_color(match ring {
                    Ring::High => warning_color(),
                    Ring::Urgent => danger_color(),
                    Ring::Done => content(0.),
                    _ => content(0.3),
                })
                .when(done, |d| {
                    d.bg(content(0.6))
                        .child(icon("check", 10.).text_color(background_base()))
                })
                .into_any_element(),
        };
        let frame = div()
            .size(px(16.))
            .mt(px(2.))
            .flex_shrink_0()
            .flex()
            .items_center()
            .justify_center()
            .child(glyph);
        if readonly {
            return frame.into_any_element();
        }
        let id = row.task.id.clone();
        let label = if done {
            format!("Mark {} open", row.task.title)
        } else {
            format!("Mark {} done", row.task.title)
        };
        frame
            .id(SharedString::from(format!("today-check-{key}")))
            .debug_selector(move || format!("today-check-{key}"))
            .role(Role::Button)
            .aria_label(label)
            .cursor_pointer()
            .on_click(cx.listener(move |this, _, _, cx| {
                // The check is its own target: it must not also select the row.
                cx.stop_propagation();
                this.toggle_done(id.clone(), cx);
            }))
            .into_any_element()
    }

    fn row_view(&self, row: &Row, readonly: bool, cx: &mut Context<Self>) -> AnyElement {
        let task = &row.task;
        let key = task.key.clone();
        let done = row.group == Group::Done;
        let selected = self.selected.as_deref() == Some(task.id.as_str());
        let id = task.id.clone();
        let warns = age_warns(row.age_days, row.group);

        let project = match task.project.as_deref() {
            Some(path) => self.project_badge(path, false),
            None => label_badge(
                &Label {
                    name: "No project".into(),
                    color: String::new(),
                },
                false,
            )
            .debug_selector({
                let key = key.clone();
                move || format!("today-no-project-{key}")
            })
            .bg(warning_color().opacity(0.1))
            .text_color(warning_color()),
        };
        let meta = |text: String| {
            div()
                .flex_shrink_0()
                .text_size(px(11.))
                .text_color(content(0.5))
                .child(text)
        };
        let badges = div()
            .mt(px(3.))
            .min_h(px(18.))
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(6.))
            .child(project)
            .when_some(
                task.mono_id
                    .as_deref()
                    .and_then(|id| self.mono_mark(id, 16.)),
                |d, mark| d.child(mark),
            )
            .when_some(due_text(task), |d, due| d.child(meta(format!("Due {due}"))))
            .when(task.priority != Priority::Normal, |d| {
                d.child(
                    div()
                        .flex_shrink_0()
                        .text_size(px(11.))
                        .text_color(if task.priority == Priority::Urgent {
                            danger_color()
                        } else {
                            warning_color()
                        })
                        .child(priority_label(task.priority)),
                )
            })
            .when_some(row.latest_note.as_ref(), |d, note| {
                d.child(
                    div()
                        .min_w_0()
                        .max_w(px(320.))
                        .truncate()
                        .text_size(px(11.))
                        .text_color(content(0.4))
                        .child(format!("Note: {}", note_preview(&note.body))),
                )
            });

        div()
            .id(SharedString::from(format!("today-row-{key}")))
            .debug_selector(move || format!("today-row-{key}"))
            .role(Role::Button)
            .aria_label(task.title.clone())
            .px(px(8.))
            .py(px(7.))
            .rounded(px(6.))
            .border_1()
            .border_color(content(0.))
            .flex()
            .gap(px(10.))
            .cursor_pointer()
            .when(selected, |d| d.bg(content(0.1)))
            .when(!selected, |d| d.hover(|s| s.bg(content(0.05))))
            .child(self.check_view(row, readonly, cx))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .flex()
                            .items_baseline()
                            .gap(px(8.))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .text_size(px(13.))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(content(if done { 0.5 } else { 1. }))
                                    .when(done, |d| d.line_through())
                                    .child(task.title.clone()),
                            )
                            .when_some(age_label(row.age_days).filter(|_| !done), |d, age| {
                                d.child(
                                    div()
                                        .debug_selector({
                                            let key = task.key.clone();
                                            move || format!("today-age-{key}")
                                        })
                                        .flex_shrink_0()
                                        .text_size(px(11.))
                                        .text_color(if warns {
                                            warning_color()
                                        } else {
                                            content(0.4)
                                        })
                                        .child(age),
                                )
                            }),
                    )
                    .child(
                        div()
                            .truncate()
                            .text_size(px(12.))
                            .text_color(content(if done { 0.35 } else { 0.5 }))
                            .child(source_line(task)),
                    )
                    .child(badges),
            )
            .on_click(cx.listener(move |this, _, _, cx| this.select(Some(id.clone()), cx)))
            .into_any_element()
    }
}

/// Shared by the tests of `list`, `detail` and `sources`.
#[cfg(test)]
pub(super) mod testkit {
    use super::*;
    use gpui::{
        Bounds, Entity, Render, TestAppContext, VisualTestContext, WindowBounds, WindowHandle,
        WindowOptions, point, size,
    };
    use monocode_core::today::{DayView, Summary};
    use std::sync::{Arc, Mutex};

    #[derive(Clone, Copy)]
    pub(in crate::today) enum Show {
        List,
        Detail,
        Sources,
    }
    pub(in crate::today) struct Page {
        pub today: Entity<Today>,
        show: Show,
    }
    impl Render for Page {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let show = self.show;
            self.today.update(cx, |today, cx| {
                div().size_full().child(match show {
                    Show::List => today.list_view(cx),
                    Show::Detail => today.detail_view(cx),
                    Show::Sources => today.sources_view(),
                })
            })
        }
    }
    pub(in crate::today) fn page(
        cx: &mut TestAppContext,
        show: Show,
        setup: impl FnOnce(&mut Today) + 'static,
    ) -> (WindowHandle<Page>, VisualTestContext) {
        cx.update(|cx| {
            cx.set_reduce_motion(true);
            crate::popover::bind(cx);
        });
        let window = cx.update(|cx| {
            cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                        point(px(0.), px(0.)),
                        size(px(1000.), px(900.)),
                    ))),
                    ..Default::default()
                },
                |_, cx| {
                    cx.new(|cx| {
                        let store = Arc::new(Mutex::new(
                            monocode_core::store::Store::open(std::path::Path::new(":memory:"))
                                .unwrap(),
                        ));
                        let today = cx.new(|cx| Today::new(store, cx));
                        Page { today, show }
                    })
                },
            )
            .unwrap()
        });
        let mut visual = VisualTestContext::from_window(window.into(), cx);
        // Let the page's own first load settle, so it cannot overwrite the fixture.
        visual.run_until_parked();
        window
            .update(&mut visual, |page, _, cx| {
                page.today.update(cx, |today, cx| {
                    setup(today);
                    cx.notify();
                })
            })
            .unwrap();
        visual.refresh().unwrap();
        (window, visual)
    }
    pub(in crate::today) fn task(key: &str, title: &str) -> Task {
        Task {
            id: format!("id-{key}"),
            key: key.into(),
            title: title.into(),
            source_name: "GitHub".into(),
            reason: "Review requested from you".into(),
            source_status: "Open".into(),
            first_day: "2026-11-01".into(),
            ..Default::default()
        }
    }
    pub(in crate::today) fn row(group: Group, task: Task, age_days: u32) -> Row {
        Row {
            task,
            group,
            age_days,
            latest_note: None,
        }
    }
    pub(in crate::today) fn view(mode: DayMode, day: &str, rows: Vec<Row>) -> DayView {
        DayView {
            day: day.into(),
            current_day: "2026-11-01".into(),
            mode,
            rows,
            summary: Summary::default(),
            recorded: true,
        }
    }
    pub(in crate::today) fn has(visual: &mut VisualTestContext, selector: &'static str) -> bool {
        visual.refresh().unwrap();
        visual.debug_bounds(selector).is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::testkit::*;
    use super::*;
    use gpui::TestAppContext;

    #[test]
    fn the_ring_follows_state_then_priority() {
        use Group::*;
        assert_eq!(ring_state(Done, Priority::Urgent), Ring::Done);
        assert_eq!(ring_state(NeedsCheck, Priority::High), Ring::NeedsCheck);
        assert_eq!(ring_state(Today, Priority::Urgent), Ring::Urgent);
        assert_eq!(ring_state(CarriedOver, Priority::High), Ring::High);
        assert_eq!(ring_state(Today, Priority::Normal), Ring::Open);
        assert_eq!(ring_state(Snoozed, Priority::Normal), Ring::Open);
    }
    #[test]
    fn age_turns_to_a_warning_at_two_days_and_never_for_done() {
        assert_eq!(AGE_WARNING_DAYS, 2);
        assert!(!age_warns(1, Group::CarriedOver));
        assert!(age_warns(2, Group::CarriedOver));
        assert!(age_warns(5, Group::NeedsCheck));
        assert!(!age_warns(5, Group::Done));
        assert_eq!(age_label(0), None);
        assert_eq!(age_label(3).as_deref(), Some("3d open"));
    }
    #[test]
    fn row_text_helpers_read_like_the_mockup() {
        let mut t = task("a", "A");
        assert_eq!(source_line(&t), "GitHub · Review requested from you · Open");
        t.source_status.clear();
        t.reason.clear();
        t.source_id = Some("src".into());
        assert_eq!(source_line(&t), "GitHub");
        assert_eq!(due_text(&t), None);
        t.due = Some("2026-11-02".into());
        assert_eq!(due_text(&t).as_deref(), Some("Nov 2"));
        t.due_time = Some("17:00".into());
        assert_eq!(due_text(&t).as_deref(), Some("Nov 2, 17:00"));
        assert_eq!(note_preview("a\n  b"), "a b");
        assert!(note_preview(&"x".repeat(200)).chars().count() <= 91);
        assert_eq!(
            new_task_placeholder(DayMode::Future, "2026-11-03"),
            "New task for Nov 3"
        );
        assert_eq!(
            new_task_placeholder(DayMode::Current, "2026-11-01"),
            "New task"
        );
    }

    fn mixed_rows() -> Vec<Row> {
        let mut urgent = task("urgent", "Urgent one");
        urgent.priority = Priority::Urgent;
        vec![
            row(Group::CarriedOver, task("carried", "Carried"), 3),
            row(Group::NeedsCheck, task("check", "Check me"), 1),
            row(Group::Today, urgent, 0),
            row(Group::Done, task("done", "Finished"), 1),
        ]
    }

    #[gpui::test]
    fn rows_render_in_group_order_with_a_check_each(cx: &mut TestAppContext) {
        let (_, mut visual) = page(cx, Show::List, |today| {
            today.view = Some(view(DayMode::Current, "2026-11-01", mixed_rows()));
        });
        let mut last = f32::MIN;
        for key in ["carried", "check", "urgent", "done"] {
            let selector: &'static str = Box::leak(format!("today-row-{key}").into_boxed_str());
            let bounds = visual
                .debug_bounds(selector)
                .unwrap_or_else(|| panic!("{selector}"));
            assert!(
                f32::from(bounds.origin.y) > last,
                "{key} is below the group before it"
            );
            last = f32::from(bounds.origin.y);
            let check: &'static str = Box::leak(format!("today-check-{key}").into_boxed_str());
            assert!(has(&mut visual, check), "{check}");
        }
        assert!(has(&mut visual, "today-new-task"));
        assert!(has(&mut visual, "today-filter-project"));
        assert!(has(&mut visual, "today-filter-source"));
        assert!(has(&mut visual, "today-filter-mono"));
    }
    #[gpui::test]
    fn a_task_without_a_project_gets_the_warning_badge(cx: &mut TestAppContext) {
        let (_, mut visual) = page(cx, Show::List, |today| {
            let mut with = task("with", "With project");
            with.project = Some("/work/app".into());
            today.view = Some(view(
                DayMode::Current,
                "2026-11-01",
                vec![
                    row(Group::Today, with, 0),
                    row(Group::Today, task("without", "Without"), 0),
                ],
            ));
        });
        assert!(has(&mut visual, "today-no-project-without"));
        assert!(!has(&mut visual, "today-no-project-with"));
    }
    #[gpui::test]
    fn a_past_day_is_a_static_record(cx: &mut TestAppContext) {
        let (_, mut visual) = page(cx, Show::List, |today| {
            today.view = Some(view(DayMode::Past, "2026-10-31", mixed_rows()));
            today.day = Some("2026-10-31".into());
        });
        assert!(has(&mut visual, "today-row-carried"));
        assert!(!has(&mut visual, "today-check-carried"));
        assert!(!has(&mut visual, "today-check-done"));
        assert!(!has(&mut visual, "today-new-task"));
        // The filters stay visible on a record, or a stale one would hide rows unseen.
        assert!(has(&mut visual, "today-filter-project"));
    }
    #[gpui::test]
    fn an_empty_future_day_says_nothing_is_planned(cx: &mut TestAppContext) {
        let (_, mut visual) = page(cx, Show::List, |today| {
            today.view = Some(view(DayMode::Future, "2026-11-02", Vec::new()));
        });
        assert!(has(&mut visual, "today-empty"));
        assert!(has(&mut visual, "today-new-task"));
    }
    #[gpui::test]
    fn an_empty_current_day_has_an_empty_state(cx: &mut TestAppContext) {
        let (_, mut visual) = page(cx, Show::List, |today| {
            today.view = Some(view(DayMode::Current, "2026-11-01", Vec::new()));
        });
        assert!(has(&mut visual, "today-empty"));
        assert!(!has(&mut visual, "today-row-carried"));
    }
    #[gpui::test]
    fn clicking_the_check_does_not_select_the_row(cx: &mut TestAppContext) {
        let (window, mut visual) = page(cx, Show::List, |today| {
            today.view = Some(view(DayMode::Current, "2026-11-01", mixed_rows()));
        });
        visual.refresh().unwrap();
        let check = visual.debug_bounds("today-check-carried").unwrap();
        visual.simulate_click(check.center(), Default::default());
        visual.refresh().unwrap();
        let selected = window
            .update(&mut visual, |page, _, cx| {
                page.today.read(cx).selected.clone()
            })
            .unwrap();
        assert_eq!(selected, None);
        // The check acts on the store, which reloads the page: put the rows back.
        window
            .update(&mut visual, |page, _, cx| {
                page.today.update(cx, |today, cx| {
                    today.view = Some(view(DayMode::Current, "2026-11-01", mixed_rows()));
                    cx.notify();
                })
            })
            .unwrap();
        visual.refresh().unwrap();
        let row = visual.debug_bounds("today-row-carried").unwrap();
        let body = gpui::point(row.right() - px(40.), row.center().y);
        visual.simulate_click(body, Default::default());
        visual.refresh().unwrap();
        let selected = window
            .update(&mut visual, |page, _, cx| {
                page.today.read(cx).selected.clone()
            })
            .unwrap();
        assert_eq!(selected.as_deref(), Some("id-carried"));
    }
    #[test]
    fn a_needs_check_row_says_why_for_both_causes() {
        let mut t = task("a", "A");
        t.check_reason = Some(CheckReason::Missing);
        assert_eq!(
            source_line(&t),
            "GitHub · Left the list without a clear reason · Open"
        );
        t.check_reason = Some(CheckReason::DoneButSourceOpen);
        assert_eq!(
            source_line(&t),
            "GitHub · You marked it done, source is still open · Open"
        );
        assert_eq!(why_here(&t), "You marked it done, source is still open");
        t.check_reason = None;
        assert_eq!(why_here(&t), "Review requested from you");
        t.reason.clear();
        t.source_id = None;
        assert_eq!(why_here(&t), "Added by you");
    }
    #[test]
    fn warning_and_danger_darken_on_light_themes() {
        let dark = (warning_color(), danger_color());
        crate::theme::configure(
            &crate::settings::Preferences {
                theme: "light".into(),
                ..Default::default()
            },
            false,
        );
        assert!(is_light());
        assert!(warning_color().l < dark.0.l && danger_color().l < dark.1.l);
        assert!(warning_color().l < 0.5, "readable on a white pane");
    }
    #[gpui::test]
    fn done_rows_show_no_age_and_open_ones_do(cx: &mut TestAppContext) {
        let (_, mut visual) = page(cx, Show::List, |today| {
            today.view = Some(view(DayMode::Current, "2026-11-01", mixed_rows()));
        });
        assert!(has(&mut visual, "today-age-carried"));
        assert!(!has(&mut visual, "today-age-done"));
    }
    #[gpui::test]
    fn an_active_filter_on_a_past_day_is_visible_and_explains_an_empty_list(
        cx: &mut TestAppContext,
    ) {
        let (_, mut visual) = page(cx, Show::List, |today| {
            today.view = Some(view(DayMode::Past, "2026-10-31", Vec::new()));
            today.filters.mono = Some("nobody".into());
        });
        assert!(has(&mut visual, "today-filter-mono"));
        assert!(has(&mut visual, "today-empty"));
    }
    #[gpui::test]
    fn a_filter_menu_opens_answers_the_keyboard_and_escape_closes_only_it(cx: &mut TestAppContext) {
        let (window, mut visual) = page(cx, Show::List, |today| {
            today.view = Some(view(DayMode::Current, "2026-11-01", mixed_rows()));
        });
        let closed = std::rc::Rc::new(std::cell::Cell::new(false));
        let seen = closed.clone();
        let _sub = window
            .update(&mut visual, |page, _, cx| {
                cx.subscribe(&page.today, move |_, _, event: &crate::today::Event, _| {
                    if matches!(event, crate::today::Event::Close) {
                        seen.set(true);
                    }
                })
            })
            .unwrap();
        let open = |visual: &mut gpui::VisualTestContext| {
            window
                .update(visual, |page, _, cx| page.today.read(cx).menus.is_open())
                .unwrap()
        };
        visual.refresh().unwrap();
        let trigger = visual.debug_bounds("today-filter-project").unwrap();
        visual.simulate_click(trigger.center(), Default::default());
        visual.refresh().unwrap();
        assert!(open(&mut visual), "the click opens the menu");
        // Down to "No project", Enter picks it.
        visual.simulate_keystrokes("down enter");
        visual.refresh().unwrap();
        assert!(!open(&mut visual));
        let filters = window
            .update(&mut visual, |page, _, cx| {
                page.today.read(cx).filters.clone()
            })
            .unwrap();
        assert_eq!(filters.project, Some(None));
        // Escape closes the menu and leaves the page alone.
        let trigger = visual.debug_bounds("today-filter-source").unwrap();
        visual.simulate_click(trigger.center(), Default::default());
        visual.refresh().unwrap();
        assert!(open(&mut visual));
        visual.simulate_keystrokes("escape");
        visual.refresh().unwrap();
        assert!(!open(&mut visual));
        assert!(
            !closed.get(),
            "Escape must not close Today while a menu is open"
        );
    }
    #[test]
    fn today_files_hold_no_theme_color_literals() {
        for (name, source) in [
            ("list.rs", include_str!("list.rs")),
            ("detail.rs", include_str!("detail.rs")),
            ("sources.rs", include_str!("sources.rs")),
        ] {
            let code = source.split("#[cfg(test)]").next().unwrap();
            for banned in ["rgb(0x", "rgba(", "hsla(", "Hsla { h", "0xf8", "0xfb"] {
                assert!(!code.contains(banned), "{name} hardcodes a color: {banned}");
            }
        }
    }
}
