//! The Tasks tab: filters, New task field, groups and rows.
//! Also home to the small pieces `detail` and `sources` share: due and priority
//! wording, and the menus.
use super::{
    Filters, Today, filter,
    menus::{Host, Pick, Rows, Slot},
    short_day,
};
use crate::{
    inbox::{label_badge, message, plain_action},
    popover::MenuAction,
    theme::*,
};
use gpui::{
    AnyElement, Context, Div, FontWeight, Hsla, Role, SharedString, Stateful, div, prelude::*, px,
};
use monocode_core::{
    inbox::Label,
    today::{
        AGE_WARNING_DAYS, CheckReason, DayMode, Group, Priority, Row, Stage, Task, WorkflowStatus,
    },
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

/// How much of the round check is filled.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Fill {
    Empty,
    Half,
    Full,
}
/// The ring of a status: empty before work starts, half while it is going, full when finished.
pub(super) fn ring_fill(stage: Stage) -> Fill {
    match stage {
        Stage::NotStarted => Fill::Empty,
        Stage::Started => Fill::Half,
        Stage::Finished => Fill::Full,
    }
}
/// A small round color dot.
pub(super) fn color_dot(color: u32) -> Div {
    div()
        .size(px(8.))
        .flex_shrink_0()
        .rounded_full()
        .bg(gpui::rgb(color))
}
/// A status's dot, in its own color.
pub(super) fn status_dot(status: &WorkflowStatus) -> Div {
    color_dot(status.rgb())
}
/// The frame of the card that follows the pointer while something is dragged.
pub(super) fn drag_ghost() -> Div {
    div()
        .rounded_md()
        .border_1()
        .border_color(accent_color().opacity(0.6))
        .bg(floating_fill())
        .px_2()
        .py_1()
        .text_color(content(1.))
}
/// The statuses to group by and their rows, in the current status order. Rows
/// whose status no longer exists (a past day, or one deleted since) follow, in
/// stage order. On a `frozen` day the group takes the name and color the day
/// recorded, not today's.
pub(super) fn status_groups<'a>(
    statuses: &[WorkflowStatus],
    rows: &[&'a Row],
    frozen: bool,
) -> Vec<(WorkflowStatus, Vec<&'a Row>)> {
    let mut groups: Vec<(WorkflowStatus, Vec<&'a Row>)> = statuses
        .iter()
        .map(|status| {
            let mine = rows
                .iter()
                .copied()
                .filter(|row| row.status.id == status.id)
                .collect::<Vec<_>>();
            let shown = match mine.first() {
                Some(row) if frozen => row.status.clone(),
                _ => status.clone(),
            };
            (shown, mine)
        })
        .collect();
    let mut gone: Vec<(WorkflowStatus, Vec<&'a Row>)> = Vec::new();
    for row in rows {
        if statuses.iter().any(|s| s.id == row.status.id) {
            continue;
        }
        match gone.iter_mut().find(|(s, _)| s.id == row.status.id) {
            Some((_, mine)) => mine.push(row),
            None => gone.push((row.status.clone(), vec![row])),
        }
    }
    gone.sort_by_key(|(status, _)| status.stage);
    groups.extend(gone);
    groups
}
/// A status's dot, name and count: the title of a list group and a board column.
pub(super) fn status_title(status: &WorkflowStatus, count: usize) -> Div {
    div()
        .min_w_0()
        .flex()
        .items_center()
        .gap(px(6.))
        .child(status_dot(status))
        .child(
            div()
                .min_w_0()
                .truncate()
                .text_size(px(11.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(content(0.7))
                .child(status.name.clone()),
        )
        .child(
            div()
                .flex_shrink_0()
                .text_size(px(11.))
                .text_color(content(0.4))
                .child(count.to_string()),
        )
}
/// A small flag next to a row's project: needs check, snoozed, dismissed.
fn flag(text: &'static str, kind: &'static str, key: &str, warn: bool) -> Div {
    label_badge(
        &Label {
            name: text.into(),
            color: String::new(),
        },
        false,
    )
    .debug_selector({
        let key = key.to_string();
        move || format!("today-badge-{kind}-{key}")
    })
    .when(warn, |d| {
        d.bg(warning_color().opacity(0.1))
            .text_color(warning_color())
    })
}

/// The New task placeholder: a future day is named, so it is clear where the task lands.
pub(super) fn new_task_placeholder(mode: DayMode, day: &str) -> String {
    match mode {
        DayMode::Future => format!("New task for {}", short_day(day)),
        _ => "New task".to_string(),
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

/// Tag chips on a row or card before the rest fold into "+N".
const TAG_CHIPS_SHOWN: usize = 3;

impl Today {
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
        let filtered = filter::active_count(&self.filters) > 0;

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

        let message_shown = rows.is_empty() && (filtered || mode == DayMode::Future);
        if rows.is_empty() && filtered {
            body = body.child(empty("No tasks match these filters."));
        } else if rows.is_empty() && mode == DayMode::Future {
            body = body.child(empty("Nothing planned yet. Snooze a task or add one here."));
        }
        for (status, mine) in self.shown_groups(&rows, readonly) {
            body = body.child(self.group(&status, &mine, readonly, !message_shown, cx));
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
            .children(self.menu_view(Host::Filters, cx))
            .into_any_element()
    }

    /// The groups or columns to draw: all statuses, or only the ones the status filter selects.
    pub(super) fn shown_groups<'a>(
        &self,
        rows: &[&'a Row],
        frozen: bool,
    ) -> Vec<(WorkflowStatus, Vec<&'a Row>)> {
        status_groups(&self.statuses, rows, frozen)
            .into_iter()
            .filter(|(status, _)| filter::status_passes(&self.filters, status, &self.statuses))
            .collect()
    }

    /// One status: its title, then its rows, or a quiet line when it has none.
    fn group(
        &self,
        status: &WorkflowStatus,
        rows: &[&Row],
        readonly: bool,
        say_empty: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let id = status.id.clone();
        let empty_id = status.id.clone();
        div()
            .debug_selector(move || format!("today-group-{id}"))
            .flex()
            .flex_col()
            .gap(px(1.))
            .child(
                status_title(status, rows.len())
                    .px(px(8.))
                    .pt(px(10.))
                    .pb(px(4.)),
            )
            .children(rows.iter().map(|row| self.row_view(row, readonly, cx)))
            .when(rows.is_empty() && say_empty, |d| {
                d.child(
                    div()
                        .debug_selector(move || format!("today-group-empty-{empty_id}"))
                        .px(px(8.))
                        .py(px(4.))
                        .text_size(px(12.))
                        .text_color(content(0.35))
                        .child("Nothing here"),
                )
            })
            .into_any_element()
    }

    pub(super) fn filters_row(&self, cx: &mut Context<Self>) -> AnyElement {
        let filters = self.filters.clone();
        let open = |slot: Slot| self.menus_open(slot);
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
        let status_label = filter::status_summary(&filters, &self.statuses);
        let tag_label = filter::tag_summary(&filters, &self.tags);
        let active = filter::active_count(&filters);
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
                Slot::StatusFilter,
                "today-filter-status",
                status_label,
                !filters.statuses.is_empty(),
            ))
            .child(trigger(
                Slot::TagFilter,
                "today-filter-tag",
                tag_label,
                !filters.tags.is_empty() || filters.no_tag,
            ))
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
            .when(active > 0, |d| {
                d.child(
                    div()
                        .debug_selector(|| "today-filter-count".into())
                        .text_size(px(12.))
                        .text_color(content(0.5))
                        .child(filter::count_label(active)),
                )
                .child(
                    plain_action("today-filters-clear", "Clear filters")
                        .debug_selector(|| "today-filters-clear".into())
                        .text_color(content(0.7))
                        .hover(|s| s.text_color(content(1.)))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.menus.close();
                            this.set_filters(Filters::default(), cx);
                        })),
                )
            })
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
            Slot::StatusFilter => {
                let toggle = |id: &str| {
                    let mut next = f.clone();
                    if !next.statuses.remove(id) {
                        next.statuses.insert(id.to_string());
                    }
                    Pick::FilterToggle(next)
                };
                for stage in Stage::ALL {
                    let mine = self
                        .statuses
                        .iter()
                        .filter(|s| s.stage == stage)
                        .collect::<Vec<_>>();
                    if mine.is_empty() {
                        continue;
                    }
                    rows.header(match stage {
                        Stage::NotStarted => "Not started",
                        Stage::Started => "Started",
                        Stage::Finished => "Finished",
                    });
                    for status in mine {
                        rows.add(
                            MenuAction::new(format!("status-{}", status.id), &status.name)
                                .icon("circle-dot")
                                .icon_color(gpui::rgb(status.rgb()).into())
                                .checked(f.statuses.contains(&status.id))
                                .keep_open(),
                            toggle(&status.id),
                        );
                    }
                }
                if !f.statuses.is_empty() {
                    rows.separator();
                    let mut cleared = f.clone();
                    cleared.statuses.clear();
                    rows.add(
                        MenuAction::new("status-clear", "Clear").keep_open(),
                        Pick::FilterToggle(cleared),
                    );
                }
            }
            Slot::TagFilter => {
                let with_toggle = |change: &dyn Fn(&mut Filters)| {
                    let mut next = f.clone();
                    change(&mut next);
                    Pick::FilterToggle(next)
                };
                rows.add(
                    MenuAction::new("tag-none", "No tag")
                        .checked(f.no_tag)
                        .keep_open(),
                    with_toggle(&|n| n.no_tag = !n.no_tag),
                );
                for tag in &self.tags {
                    rows.add(
                        MenuAction::new(format!("tag-{}", tag.id), &tag.name)
                            .icon("circle-dot")
                            .icon_color(gpui::rgb(tag.rgb()).into())
                            .checked(f.tags.contains(&tag.id))
                            .keep_open(),
                        with_toggle(&|n| {
                            if !n.tags.remove(&tag.id) {
                                n.tags.insert(tag.id.clone());
                            }
                        }),
                    );
                }
                rows.separator();
                rows.add(
                    MenuAction::new("tag-match-any", "Match any")
                        .checked(!f.match_all)
                        .keep_open(),
                    with_toggle(&|n| n.match_all = false),
                );
                rows.add(
                    MenuAction::new("tag-match-all", "Match all")
                        .checked(f.match_all)
                        .keep_open(),
                    with_toggle(&|n| n.match_all = true),
                );
                if !f.tags.is_empty() || f.no_tag {
                    rows.separator();
                    rows.add(
                        MenuAction::new("tag-clear", "Clear").keep_open(),
                        with_toggle(&|n| {
                            n.tags.clear();
                            n.no_tag = false;
                        }),
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

    /// The round check, drawn in the status color. A past day's is static: nothing there can be changed.
    fn check_view(&self, row: &Row, readonly: bool, cx: &mut Context<Self>) -> AnyElement {
        let fill = ring_fill(row.status.stage);
        let color: Hsla = gpui::rgb(row.status.rgb()).into();
        let key = row.task.key.clone();
        let done = fill == Fill::Full;
        let mark = match fill {
            Fill::Empty => "empty",
            Fill::Half => "half",
            Fill::Full => "full",
        };
        let glyph = div()
            .debug_selector({
                let key = key.clone();
                move || format!("today-ring-{key}-{mark}")
            })
            .size_full()
            .rounded_full()
            .border_1()
            .border_color(color)
            .flex()
            .items_center()
            .when(fill == Fill::Half, |d| {
                d.child(div().w(px(7.)).h_full().rounded_l(px(7.)).bg(color))
            })
            .when(done, |d| {
                d.bg(color)
                    .justify_center()
                    .child(icon("check", 10.).text_color(background_base()))
            });
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

    /// The chips under a row's or card's title: project, tags, Mono, flags, age,
    /// due, priority and the latest note. Rows and board cards both draw these.
    pub(super) fn chips(&self, row: &Row) -> Div {
        let task = &row.task;
        let key = task.key.clone();
        let done = row.group == Group::Done || row.status.stage == Stage::Finished;
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
        div()
            .mt(px(3.))
            .min_h(px(18.))
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(6.))
            .child(project)
            .children(self.tag_chips(
                "today-tag-chip",
                &task.id,
                &row.tags,
                true,
                Some(TAG_CHIPS_SHOWN),
            ))
            .when_some(
                task.mono_id
                    .as_deref()
                    .and_then(|id| self.mono_mark(id, 16.)),
                |d, mark| d.child(mark),
            )
            .when(row.group == Group::NeedsCheck, |d| {
                d.child(flag("Needs check", "needs-check", &key, true))
            })
            .when(row.group == Group::Snoozed, |d| {
                d.child(flag("Snoozed", "snoozed", &key, false))
            })
            .when(row.group == Group::Dismissed, |d| {
                d.child(flag("Dismissed", "dismissed", &key, false))
            })
            .when_some(age_label(row.age_days).filter(|_| !done), |d, age| {
                d.child(
                    div()
                        .debug_selector({
                            let key = key.clone();
                            move || format!("today-age-{key}")
                        })
                        .flex_shrink_0()
                        .text_size(px(11.))
                        .text_color(if warns { warning_color() } else { content(0.4) })
                        .child(age),
                )
            })
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
            })
    }

    fn row_view(&self, row: &Row, readonly: bool, cx: &mut Context<Self>) -> AnyElement {
        let task = &row.task;
        let key = task.key.clone();
        let done = row.group == Group::Done || row.status.stage == Stage::Finished;
        let selected = self.selected.as_deref() == Some(task.id.as_str());
        let id = task.id.clone();

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
                            .truncate()
                            .text_size(px(13.))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(content(if done { 0.5 } else { 1. }))
                            .when(done, |d| d.line_through())
                            .child(task.title.clone()),
                    )
                    .child(
                        div()
                            .truncate()
                            .text_size(px(12.))
                            .text_color(content(if done { 0.35 } else { 0.5 }))
                            .child(source_line(task)),
                    )
                    .child(self.chips(row)),
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
        Bounds, Entity, Render, TestAppContext, VisualTestContext, Window, WindowBounds,
        WindowHandle, WindowOptions, point, size,
    };
    use monocode_core::today::{DayView, Summary};
    use std::sync::{Arc, Mutex};

    #[derive(Clone, Copy)]
    pub(in crate::today) enum Show {
        List,
        Board,
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
                    Show::Board => today.board_view(cx),
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
        let status = monocode_core::today::statuses::default_statuses().remove(match group {
            Group::Done => 2,
            _ => 0,
        });
        Row {
            task,
            group,
            status,
            tags: Vec::new(),
            age_days,
            latest_note: None,
        }
    }
    /// A row in the default status `id` (`todo`, `in_progress` or `done`).
    pub(in crate::today) fn row_in(id: &str, task: Task, age_days: u32) -> Row {
        let status = monocode_core::today::statuses::default_statuses()
            .into_iter()
            .find(|s| s.id == id)
            .unwrap();
        let group = if status.stage == Stage::Finished {
            Group::Done
        } else {
            Group::Today
        };
        Row {
            task,
            group,
            status,
            tags: Vec::new(),
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
    fn the_ring_follows_the_stage() {
        assert_eq!(ring_fill(Stage::NotStarted), Fill::Empty);
        assert_eq!(ring_fill(Stage::Started), Fill::Half);
        assert_eq!(ring_fill(Stage::Finished), Fill::Full);
    }
    #[test]
    fn groups_follow_the_status_order_and_a_deleted_status_comes_last() {
        let statuses = monocode_core::today::statuses::default_statuses();
        let mut gone = statuses[1].clone();
        gone.id = "gone".into();
        gone.name = "Gone".into();
        let mut gone_early = statuses[0].clone();
        gone_early.id = "gone-early".into();
        let rows = [
            Row {
                status: gone.clone(),
                ..row_in("todo", task("a", "A"), 0)
            },
            row_in("done", task("b", "B"), 0),
            Row {
                status: gone_early.clone(),
                ..row_in("todo", task("c", "C"), 0)
            },
            row_in("todo", task("d", "D"), 0),
        ];
        let refs = rows.iter().collect::<Vec<_>>();
        let groups = status_groups(&statuses, &refs, true);
        let ids = groups
            .iter()
            .map(|(s, _)| s.id.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            ids,
            ["todo", "in_progress", "done", "gone-early", "gone"],
            "current order first, then the vanished ones in stage order"
        );
        let counts = groups.iter().map(|(_, r)| r.len()).collect::<Vec<_>>();
        assert_eq!(counts, [1, 0, 1, 1, 1]);
    }
    #[test]
    fn a_past_group_keeps_the_name_the_day_froze() {
        let statuses = monocode_core::today::statuses::default_statuses();
        let mut old = statuses[0].clone();
        old.name = "Old name".into();
        let rows = [Row {
            status: old,
            ..row_in("todo", task("a", "A"), 0)
        }];
        let refs = rows.iter().collect::<Vec<_>>();
        assert_eq!(status_groups(&statuses, &refs, true)[0].0.name, "Old name");
        assert_eq!(status_groups(&statuses, &refs, false)[0].0.name, "To do");
    }
    #[gpui::test]
    fn the_list_groups_by_status_and_an_empty_group_says_nothing_here(cx: &mut TestAppContext) {
        let (_, mut visual) = page(cx, Show::List, |today| {
            today.view = Some(view(
                DayMode::Current,
                "2026-11-01",
                vec![
                    row_in("done", task("fin", "Finished"), 1),
                    row_in("todo", task("first", "First"), 0),
                ],
            ));
        });
        let mut last = f32::MIN;
        for id in ["todo", "in_progress", "done"] {
            let selector: &'static str = Box::leak(format!("today-group-{id}").into_boxed_str());
            let bounds = visual.debug_bounds(selector).expect(selector);
            assert!(
                f32::from(bounds.origin.y) > last,
                "{id} follows the group before it"
            );
            last = f32::from(bounds.origin.y);
        }
        assert!(!has(&mut visual, "today-group-empty-todo"));
        assert!(has(&mut visual, "today-group-empty-in_progress"));
        assert!(!has(&mut visual, "today-group-empty-done"));
        // The row sits under its own status, not under the lifecycle group.
        let todo = visual.debug_bounds("today-group-todo").unwrap();
        let row = visual.debug_bounds("today-row-first").unwrap();
        assert!(todo.contains(&row.center()));
    }
    #[gpui::test]
    fn a_past_day_lists_every_status_too(cx: &mut TestAppContext) {
        let (_, mut visual) = page(cx, Show::List, |today| {
            today.view = Some(view(DayMode::Past, "2026-10-31", Vec::new()));
            today.day = Some("2026-10-31".into());
        });
        for id in ["todo", "in_progress", "done"] {
            let selector: &'static str =
                Box::leak(format!("today-group-empty-{id}").into_boxed_str());
            assert!(has(&mut visual, selector), "{selector}");
        }
    }
    #[gpui::test]
    fn the_ring_is_empty_half_or_full_by_stage_and_shows_the_flags_as_badges(
        cx: &mut TestAppContext,
    ) {
        let (_, mut visual) = page(cx, Show::List, |today| {
            let mut check = row_in("todo", task("chk", "Check"), 0);
            check.group = Group::NeedsCheck;
            let mut snoozed = row_in("todo", task("snz", "Snoozed"), 0);
            snoozed.group = Group::Snoozed;
            today.view = Some(view(
                DayMode::Past,
                "2026-10-31",
                vec![
                    row_in("todo", task("a", "A"), 0),
                    row_in("in_progress", task("b", "B"), 0),
                    row_in("done", task("c", "C"), 0),
                    check,
                    snoozed,
                ],
            ));
            today.day = Some("2026-10-31".into());
        });
        assert!(has(&mut visual, "today-ring-a-empty"));
        assert!(has(&mut visual, "today-ring-b-half"));
        assert!(has(&mut visual, "today-ring-c-full"));
        assert!(has(&mut visual, "today-badge-needs-check-chk"));
        assert!(has(&mut visual, "today-badge-snoozed-snz"));
        assert!(!has(&mut visual, "today-badge-needs-check-a"));
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
        assert!(
            !has(&mut visual, "today-group-empty-todo"),
            "one message, not one per group"
        );
    }
    #[gpui::test]
    fn an_empty_current_day_has_an_empty_state(cx: &mut TestAppContext) {
        let (_, mut visual) = page(cx, Show::List, |today| {
            today.view = Some(view(DayMode::Current, "2026-11-01", Vec::new()));
        });
        assert!(has(&mut visual, "today-group-empty-todo"));
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
    fn set_of(ids: &[&str]) -> std::collections::BTreeSet<String> {
        ids.iter().map(|s| s.to_string()).collect()
    }
    fn status_rows() -> Vec<Row> {
        vec![
            row_in("todo", task("a", "A"), 0),
            row_in("in_progress", task("b", "B"), 0),
            row_in("done", task("c", "C"), 0),
        ]
    }
    #[gpui::test]
    fn a_status_filter_shows_only_the_selected_groups(cx: &mut TestAppContext) {
        let (_, mut visual) = page(cx, Show::List, |today| {
            today.view = Some(view(DayMode::Current, "2026-11-01", status_rows()));
            today.filters.statuses = set_of(&["todo", "done"]);
        });
        assert!(has(&mut visual, "today-group-todo"));
        assert!(has(&mut visual, "today-group-done"));
        assert!(!has(&mut visual, "today-group-in_progress"));
        assert!(has(&mut visual, "today-row-a"));
        assert!(!has(&mut visual, "today-row-b"));
        assert!(has(&mut visual, "today-filter-count"));
    }
    #[gpui::test]
    fn a_status_filter_with_no_rows_says_so(cx: &mut TestAppContext) {
        let (_, mut visual) = page(cx, Show::List, |today| {
            today.view = Some(view(DayMode::Current, "2026-11-01", status_rows()));
            today.filters.statuses = set_of(&["todo"]);
            today.filters.mono = Some("nobody".into());
        });
        assert!(has(&mut visual, "today-empty"));
        assert!(!has(&mut visual, "today-group-in_progress"));
    }
    #[gpui::test]
    fn clear_filters_resets_every_kind(cx: &mut TestAppContext) {
        let (window, mut visual) = page(cx, Show::List, |today| {
            today.view = Some(view(DayMode::Current, "2026-11-01", status_rows()));
            today.filters = Filters {
                project: Some(None),
                mono: Some("m".into()),
                statuses: set_of(&["todo"]),
                tags: set_of(&["x"]),
                no_tag: true,
                match_all: true,
                ..Default::default()
            };
        });
        let bounds = visual.debug_bounds("today-filters-clear").unwrap();
        visual.simulate_click(bounds.center(), Default::default());
        visual.refresh().unwrap();
        let filters = window
            .update(&mut visual, |page, _, cx| {
                page.today.read(cx).filters.clone()
            })
            .unwrap();
        assert_eq!(filters, Filters::default());
        assert!(!has(&mut visual, "today-filters-clear"));
    }
    #[gpui::test]
    fn the_status_menu_stays_open_while_options_are_toggled(cx: &mut TestAppContext) {
        let (window, mut visual) = page(cx, Show::List, |today| {
            today.view = Some(view(DayMode::Current, "2026-11-01", status_rows()));
        });
        let read = |visual: &mut gpui::VisualTestContext| {
            window
                .update(visual, |page, _, cx| {
                    let today = page.today.read(cx);
                    (today.menus.is_open(), today.filters.statuses.clone())
                })
                .unwrap()
        };
        visual.refresh().unwrap();
        let trigger = visual.debug_bounds("today-filter-status").unwrap();
        visual.simulate_click(trigger.center(), Default::default());
        visual.refresh().unwrap();
        // The menu opens on its first action ("To do"; headers are skipped).
        visual.simulate_keystrokes("enter");
        visual.refresh().unwrap();
        assert_eq!(read(&mut visual), (true, set_of(&["todo"])));
        visual.simulate_keystrokes("down enter");
        visual.refresh().unwrap();
        let (open, statuses) = read(&mut visual);
        assert!(open, "a keep-open pick leaves the menu open");
        assert_eq!(statuses, set_of(&["todo", "in_progress"]));
        // Picking the same row again turns it off.
        visual.simulate_keystrokes("enter");
        visual.refresh().unwrap();
        assert_eq!(read(&mut visual), (true, set_of(&["todo"])));
    }
    #[gpui::test]
    fn clear_in_a_filter_menu_keeps_a_row_highlighted(cx: &mut TestAppContext) {
        let (window, mut visual) = page(cx, Show::List, |today| {
            today.view = Some(view(DayMode::Current, "2026-11-01", status_rows()));
        });
        let trigger = visual.debug_bounds("today-filter-status").unwrap();
        visual.simulate_click(trigger.center(), Default::default());
        visual.refresh().unwrap();
        // To do on, then down to Clear (after In progress and Done) and pick it.
        visual.simulate_keystrokes("enter down down down enter");
        visual.refresh().unwrap();
        // The menu was rebuilt without Clear: Enter still acts on a highlighted row.
        visual.simulate_keystrokes("enter");
        visual.refresh().unwrap();
        let statuses = window
            .update(&mut visual, |page, _, cx| {
                page.today.read(cx).filters.statuses.clone()
            })
            .unwrap();
        assert_eq!(statuses, set_of(&["todo"]));
    }
    #[gpui::test]
    fn the_tag_menu_toggles_in_place_and_switches_to_match_all(cx: &mut TestAppContext) {
        let (window, mut visual) = page(cx, Show::List, |today| {
            today.view = Some(view(DayMode::Current, "2026-11-01", status_rows()));
            today.tags = vec![
                monocode_core::today::Tag {
                    id: "tag-1".into(),
                    name: "One".into(),
                    color: "#4da3f5".into(),
                },
                monocode_core::today::Tag {
                    id: "tag-2".into(),
                    name: "Two".into(),
                    color: "#10b981".into(),
                },
            ];
        });
        let trigger = visual.debug_bounds("today-filter-tag").unwrap();
        visual.simulate_click(trigger.center(), Default::default());
        visual.refresh().unwrap();
        // No tag, One, then Two, Match any and Match all.
        visual.simulate_keystrokes("enter down enter down down down enter");
        visual.refresh().unwrap();
        let (open, filters) = window
            .update(&mut visual, |page, _, cx| {
                let today = page.today.read(cx);
                (today.menus.is_open(), today.filters.clone())
            })
            .unwrap();
        assert!(open);
        assert!(filters.no_tag);
        assert_eq!(filters.tags, set_of(&["tag-1"]));
        assert!(filters.match_all, "the third pick lands on Match all");
    }

    #[test]
    fn today_files_hold_no_theme_color_literals() {
        for (name, source) in [
            ("list.rs", include_str!("list.rs")),
            ("board.rs", include_str!("board.rs")),
            ("detail.rs", include_str!("detail.rs")),
            ("sources.rs", include_str!("sources.rs")),
            ("statuses.rs", include_str!("statuses.rs")),
            ("menus.rs", include_str!("menus.rs")),
            ("filter.rs", include_str!("filter.rs")),
            ("../today.rs", include_str!("../today.rs")),
        ] {
            let code = source.split("#[cfg(test)]").next().unwrap();
            for banned in ["rgb(0x", "rgba(", "hsla(", "Hsla { h", "0xf8", "0xfb"] {
                assert!(!code.contains(banned), "{name} hardcodes a color: {banned}");
            }
        }
    }

    fn tagged_row(count: usize) -> Row {
        let mut r = row(Group::Today, task("a", "Tagged"), 0);
        r.tags = (1..=count)
            .map(|n| monocode_core::today::Tag {
                id: format!("t{n}"),
                name: format!("Tag {n}"),
                color: "#4da3f5".into(),
            })
            .collect();
        r
    }
    #[gpui::test]
    fn a_row_shows_its_tags_in_order_and_folds_the_rest(cx: &mut TestAppContext) {
        let (_, mut visual) = page(cx, Show::List, |today| {
            today.view = Some(view(DayMode::Current, "2026-11-01", vec![tagged_row(5)]));
        });
        let x = |visual: &mut gpui::VisualTestContext, s: &'static str| {
            visual
                .debug_bounds(s)
                .unwrap_or_else(|| panic!("{s}"))
                .origin
                .x
        };
        assert!(has(&mut visual, "today-tag-chip-id-a-t1"));
        let (a, b, c) = (
            x(&mut visual, "today-tag-chip-id-a-t1"),
            x(&mut visual, "today-tag-chip-id-a-t2"),
            x(&mut visual, "today-tag-chip-id-a-t3"),
        );
        assert!(a < b && b < c);
        assert!(!has(&mut visual, "today-tag-chip-id-a-t4"));
        assert!(has(&mut visual, "today-tag-chip-id-a-more"));
    }
    #[gpui::test]
    fn a_row_with_three_tags_shows_them_all(cx: &mut TestAppContext) {
        let (_, mut visual) = page(cx, Show::List, |today| {
            today.view = Some(view(DayMode::Current, "2026-11-01", vec![tagged_row(3)]));
        });
        assert!(has(&mut visual, "today-tag-chip-id-a-t3"));
        assert!(!has(&mut visual, "today-tag-chip-id-a-more"));
    }
}
