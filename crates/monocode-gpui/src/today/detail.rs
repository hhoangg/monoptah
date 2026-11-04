//! The detail panel: breadcrumb and actions, title, pills, check callout, details,
//! context, one activity timeline and the note composer.
use super::{
    Today,
    list::{check_label, color_dot, due_text, priority_label, status_dot, why_here},
    menus::{Host, Pick, Rows, Slot},
    short_day,
};
use crate::{
    composer_box,
    inbox::{action, caption, message},
    input::{self, Input},
    popover::MenuAction,
    theme::*,
};
use chrono::{Datelike, Duration, NaiveDate, Weekday};
use gpui::{
    AnyElement, Context, Div, Entity, Focusable, FontWeight, Role, Stateful, Subscription, div,
    prelude::*, px,
};
use monocode_core::{
    store::Store,
    today::{
        AGE_WARNING_DAYS, Activity, Actor, CheckReason, DayMode, EventKind, Group, Kind, Note,
        Priority, Status, Tag, Task, TaskEvent, TaskPatch, statuses::TODO_ID,
    },
};
use std::cell::RefCell;

/// Tomorrow, then the next few weekdays, as (day, label) for the Snooze menu.
pub(super) fn snooze_options(current_day: &str) -> Vec<(String, String)> {
    let Ok(today) = NaiveDate::parse_from_str(current_day, "%Y-%m-%d") else {
        return Vec::new();
    };
    let mut options = vec![(
        (today + Duration::days(1)).format("%Y-%m-%d").to_string(),
        "Tomorrow".to_string(),
    )];
    let mut day = today + Duration::days(2);
    while options.len() < 5 {
        if !matches!(day.weekday(), Weekday::Sat | Weekday::Sun) {
            options.push((
                day.format("%Y-%m-%d").to_string(),
                day.format("%A, %b %-d").to_string(),
            ));
        }
        day += Duration::days(1);
    }
    options
}
/// `Snooze until tomorrow`, `Snooze until Wednesday, Nov 4`.
fn snooze_label(label: &str) -> String {
    if label == "Tomorrow" {
        "Snooze until tomorrow".to_string()
    } else {
        format!("Snooze until {label}")
    }
}
fn only_http(url: &str) -> bool {
    let url = url.trim().to_ascii_lowercase();
    url.starts_with("http://") || url.starts_with("https://")
}
/// The task's id at its source: the key after the provider prefix, else the
/// last path segment of its link. Manual tasks have none.
fn source_ref(task: &Task) -> Option<String> {
    if let Some((provider, rest)) = task.key.split_once(':')
        && provider != "manual"
        && !rest.trim().is_empty()
    {
        return Some(rest.trim().to_string());
    }
    if task.kind == Kind::Manual || task.key.starts_with("manual:") {
        return None;
    }
    task.link
        .as_deref()
        .and_then(|link| link.trim().trim_end_matches('/').rsplit('/').next())
        .filter(|segment| !segment.is_empty() && !segment.contains(':'))
        .map(str::to_string)
}
/// `project / Source ref`, the text of the top bar.
fn breadcrumb_text(project: Option<&str>, task: &Task) -> String {
    let source = match (task.source_name.trim(), source_ref(task)) {
        ("", Some(reference)) => reference,
        ("", None) => "Manual".to_string(),
        (name, Some(reference)) => format!("{name} {reference}"),
        (name, None) => name.to_string(),
    };
    match project {
        Some(project) => format!("{project} / {source}"),
        None => source,
    }
}
/// How long the task has been open.
fn open_for_text(age_days: u32, first_day: &str, current: bool) -> String {
    let since = short_day(first_day);
    match age_days {
        0 if current => "Today".to_string(),
        0 => format!("Since {since}"),
        1 => format!("1 day, since {since}"),
        n => format!("{n} days, since {since}"),
    }
}
/// One timeline line as text. `mono` is the display name of `event.mono_id`.
fn event_text(event: &TaskEvent, mono: Option<&str>) -> String {
    match event.kind {
        EventKind::Added => match (mono, event.source_id.is_some()) {
            (Some(name), _) => format!("First seen by {name}"),
            (None, true) => "First seen by a sync".to_string(),
            (None, false) => "Added by you".to_string(),
        },
        EventKind::StatusChanged => match (&event.from, &event.to) {
            (Some(from), Some(to)) => format!("Status: {} -> {}", from.name, to.name),
            (None, Some(to)) => format!("Status: {}", to.name),
            _ => "Status changed".to_string(),
        },
        EventKind::NeedsCheck => match event.check_reason {
            Some(reason) => format!("Needs check: {}", check_label(reason)),
            None => "Needs check".to_string(),
        },
        EventKind::CheckCleared => "Check cleared".to_string(),
        EventKind::ClosedBySource => match event.detail.as_deref().filter(|d| !d.is_empty()) {
            Some(reason) => format!("Closed at source: {reason}"),
            None => "Closed at source".to_string(),
        },
        EventKind::ReopenedBySource => "Reopened at source".to_string(),
        EventKind::Snoozed => match event.detail.as_deref().filter(|d| !d.is_empty()) {
            Some(day) => format!("Snoozed until {}", short_day(day)),
            None => "Snoozed".to_string(),
        },
        EventKind::Dismissed => "Dismissed".to_string(),
        EventKind::KeptDone => "Kept done".to_string(),
        EventKind::TagsChanged => tags_text(event),
    }
}
/// "Tag added: Bug", "Tags removed: Bug, UI", or "Tags: added Bug, removed UI"
/// when one event did both; "Tags changed" when it names none.
fn tags_text(event: &TaskEvent) -> String {
    let names = |tags: &[Tag]| {
        tags.iter()
            .map(|tag| tag.name.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    };
    let plural = |tags: &[Tag]| if tags.len() == 1 { "Tag" } else { "Tags" };
    match (event.added_tags.as_slice(), event.removed_tags.as_slice()) {
        ([], []) => "Tags changed".to_string(),
        (added, []) => format!("{} added: {}", plural(added), names(added)),
        ([], removed) => format!("{} removed: {}", plural(removed), names(removed)),
        (added, removed) => format!("Tags: added {}, removed {}", names(added), names(removed)),
    }
}
fn event_icon(kind: EventKind) -> &'static str {
    match kind {
        EventKind::Added => "plus",
        EventKind::StatusChanged => "circle-dot",
        EventKind::NeedsCheck => "alert-circle",
        EventKind::CheckCleared | EventKind::KeptDone => "check",
        EventKind::ClosedBySource => "check-circle-2",
        EventKind::ReopenedBySource => "rotate-ccw",
        EventKind::Snoozed => "clock",
        EventKind::Dismissed => "close",
        EventKind::TagsChanged => "tag",
    }
}

/// A rounded chip under the title; `live` ones open a menu.
fn pill(id: &'static str, live: bool) -> Stateful<Div> {
    div()
        .id(id)
        .relative()
        .h(px(24.))
        .px(px(8.))
        .rounded(px(12.))
        .border_1()
        .border_color(content(0.12))
        .flex()
        .items_center()
        .gap(px(6.))
        .text_size(px(12.))
        .text_color(content(0.8))
        .when(live, |d| {
            d.role(Role::Button)
                .cursor_pointer()
                .hover(|s| s.bg(content(0.07)))
        })
}
fn section(title: &'static str) -> Div {
    caption(title).px(px(0.)).pt(px(18.))
}
fn detail_row(glyph: &'static str, name: &'static str, value: impl IntoElement) -> Div {
    div()
        .flex()
        .items_start()
        .gap(px(8.))
        .text_size(px(12.))
        .line_height(px(18.))
        .child(icon(glyph, 14.).mt(px(2.)).text_color(content(0.4)))
        .child(
            div()
                .w(px(72.))
                .flex_shrink_0()
                .text_color(content(0.4))
                .child(name),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_color(content(0.8))
                .child(value),
        )
}
/// A detail value that opens a menu: quiet until hovered.
fn menu_value(id: &'static str, text: String, open: bool) -> Stateful<Div> {
    menu_trigger(id, text.clone(), open)
        .child(div().min_w_0().truncate().child(text))
        .child(icon("chevron-down", 12.).text_color(content(0.4)))
}
/// The frame of [`menu_value`], for a value that is not plain text.
fn menu_trigger(id: &'static str, label: String, open: bool) -> Stateful<Div> {
    div()
        .id(id)
        .debug_selector(|| id.to_string())
        .role(Role::Button)
        .aria_label(label)
        .aria_expanded(open)
        .relative()
        .ml(px(-4.))
        .px(px(4.))
        .rounded(px(4.))
        .flex()
        .items_center()
        .gap(px(4.))
        .cursor_pointer()
        .hover(|s| s.bg(content(0.07)))
}

/// The title field of a manual task. Enter commits it through [`Today::patch`].
pub(crate) struct TitleEdit {
    input: Entity<Input>,
    /// The (task id, title) the field was last filled from, so a refresh never
    /// overwrites what is being typed.
    filled: RefCell<Option<(String, String)>>,
    _submit: Subscription,
}
impl TitleEdit {
    pub(super) fn new(cx: &mut Context<Today>) -> Self {
        let input = cx.new(|cx| {
            let mut input = Input::field("Title", "Title", content(0.4), cx);
            input.set_font_size(15.);
            input
        });
        let submit = cx.subscribe(&input, |this: &mut Today, _, _: &input::Submitted, cx| {
            this.commit_title(cx)
        });
        Self {
            input,
            filled: RefCell::new(None),
            _submit: submit,
        }
    }
    fn fill(&self, task: &Task, cx: &mut Context<Today>) {
        let now = (task.id.clone(), task.title.clone());
        if self.filled.borrow().as_ref() == Some(&now) {
            return;
        }
        *self.filled.borrow_mut() = Some(now);
        self.input
            .update(cx, |input, cx| input.set_text(task.title.clone(), cx));
    }
}

impl Today {
    pub(super) fn patch(&mut self, id: String, patch: TaskPatch, cx: &mut Context<Self>) {
        self.act(
            move |store: &Store, now| {
                store
                    .today_update(&id, &patch, &Actor::User, now)
                    .map(|_| ())
            },
            cx,
        );
    }
    /// Enter in the title field of a manual task.
    fn commit_title(&mut self, cx: &mut Context<Self>) {
        let Some((id, title)) = self
            .selected_row()
            .filter(|row| row.task.kind == Kind::Manual)
            .filter(|_| self.view.as_ref().is_some_and(|v| v.mode != DayMode::Past))
            .map(|row| (row.task.id.clone(), row.task.title.clone()))
        else {
            return;
        };
        let typed = self.title.input.read(cx).content.trim().to_string();
        if typed.is_empty() {
            self.title
                .input
                .update(cx, |input, cx| input.set_text(title, cx));
        } else if typed != title {
            self.patch(
                id,
                TaskPatch {
                    title: Some(typed),
                    ..Default::default()
                },
                cx,
            );
        }
    }

    /// The selected task; the empty hint when nothing is selected.
    pub(super) fn detail_view(&self, cx: &mut Context<Self>) -> AnyElement {
        let (Some(row), Some(view)) = (self.selected_row(), self.view.as_ref()) else {
            return div()
                .debug_selector(|| "today-detail-empty".into())
                .size_full()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .child(
                    message("Select a task to see its details and notes.")
                        .text_center()
                        .text_color(content(0.4)),
                )
                .into_any_element();
        };
        let task = &row.task;
        let mode = view.mode;
        let editable = mode != DayMode::Past;
        let current = mode == DayMode::Current;
        let id = task.id.clone();
        let open = |slot: Slot| self.menus_open(slot);
        let not_done = task.status != Status::Done;

        self.note.update(cx, |input, cx| {
            input.set_placeholder("Why is this not done yet?", cx)
        });

        // --- top bar ---------------------------------------------------------------
        let project_name = task.project.as_deref().map(|path| self.project_label(path));
        let project_color = task.project.as_deref().map(|path| {
            self.appearances
                .get(path)
                .cloned()
                .unwrap_or_default()
                .color(path)
        });
        let mut bar = div().flex().items_center().gap(px(2.));
        if let Some(link) = task.link.clone().filter(|link| only_http(link)) {
            bar = bar.child(
                icon_button("today-open-link", "Open in source", "external-link")
                    .debug_selector(|| "today-open-link".into())
                    .on_click(move |_, _, cx| cx.open_url(link.trim())),
            );
        }
        if editable {
            let started = task.clone();
            bar = bar.child(
                icon_button("today-start-session", "Start session", "play")
                    .debug_selector(|| "today-start-session".into())
                    .aria_expanded(open(Slot::StartProject))
                    .relative()
                    .child(self.anchor(Slot::StartProject).probe())
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.start_session(started.clone(), window, cx)
                    })),
            );
        }
        // Only an open task can be snoozed or dismissed; the store refuses a done one.
        let snoozable = current && not_done;
        let dismissable = editable && not_done;
        if snoozable || dismissable {
            let current_day = view.current_day.clone();
            let id = id.clone();
            bar = bar.child(
                icon_button("today-more", "More", "more-horizontal")
                    .debug_selector(|| "today-more".into())
                    .aria_expanded(open(Slot::More))
                    .relative()
                    .child(self.anchor(Slot::More).probe())
                    .on_click(cx.listener(move |this, _, window, cx| {
                        let mut rows = Rows::default();
                        if snoozable {
                            for (ix, (day, label)) in
                                snooze_options(&current_day).into_iter().enumerate()
                            {
                                rows.add(
                                    MenuAction::new(format!("snooze-{ix}"), snooze_label(&label))
                                        .icon("clock"),
                                    Pick::Snooze {
                                        id: id.clone(),
                                        day,
                                    },
                                );
                            }
                        }
                        if snoozable && dismissable {
                            rows.separator();
                        }
                        if dismissable {
                            rows.add(
                                MenuAction::new("dismiss", "Dismiss").icon("close"),
                                Pick::Dismiss { id: id.clone() },
                            );
                        }
                        this.open_menu(Slot::More, "More", rows, window, cx);
                    })),
            );
        }
        let top_bar = div()
            .flex_shrink_0()
            .h(px(40.))
            .px(px(12.))
            .flex()
            .items_center()
            .gap(px(8.))
            .border_b_1()
            .border_color(content(0.07))
            .child(
                div()
                    .debug_selector(|| "today-breadcrumb".into())
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .text_size(px(12.))
                    .text_color(content(0.5))
                    .when_some(project_color, |d, color| d.child(color_dot(color)))
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .child(breadcrumb_text(project_name.as_deref(), task)),
                    ),
            )
            .child(bar);

        // --- title -----------------------------------------------------------------
        let title: AnyElement = if editable && task.kind == Kind::Manual {
            self.title.fill(task, cx);
            div()
                .debug_selector(|| "today-title-input".into())
                .text_size(px(15.))
                .line_height(px(20.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(content(1.))
                .child(self.title.input.clone())
                .into_any_element()
        } else {
            div()
                .debug_selector(|| "today-title".into())
                .text_size(px(15.))
                .line_height(px(20.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(content(1.))
                .child(task.title.clone())
                .into_any_element()
        };

        // --- pills -----------------------------------------------------------------
        let status_pill = {
            let pill = pill("today-status", editable)
                .debug_selector(|| "today-status".into())
                .child(status_dot(&row.status))
                .child(row.status.name.clone());
            if editable {
                let id = id.clone();
                let current_status = row.status.id.clone();
                pill.aria_expanded(open(Slot::Status))
                    .aria_label(format!("Status: {}", row.status.name))
                    .child(icon("chevron-down", 12.).text_color(content(0.4)))
                    .child(self.anchor(Slot::Status).probe())
                    .on_click(cx.listener(move |this, _, window, cx| {
                        let mut rows = Rows::default();
                        for status in &this.statuses {
                            rows.add(
                                MenuAction::new(format!("status-{}", status.id), &status.name)
                                    .icon("circle-dot")
                                    .icon_color(gpui::rgb(status.rgb()).into())
                                    .checked(status.id == current_status),
                                Pick::Status {
                                    id: id.clone(),
                                    status_id: status.id.clone(),
                                },
                            );
                        }
                        rows.separator();
                        rows.add(
                            MenuAction::new("status-edit", "Edit statuses").icon("sliders"),
                            Pick::EditStatuses,
                        );
                        this.open_menu(Slot::Status, "Status", rows, window, cx);
                    }))
            } else {
                pill
            }
        };
        let priority_pill = if editable {
            let id = id.clone();
            let current_priority = task.priority;
            pill("today-priority", true)
                .debug_selector(|| "today-priority".into())
                .aria_expanded(open(Slot::Priority))
                .aria_label(format!("Priority: {}", priority_label(task.priority)))
                .child(priority_label(task.priority))
                .child(icon("chevron-down", 12.).text_color(content(0.4)))
                .child(self.anchor(Slot::Priority).probe())
                .on_click(cx.listener(move |this, _, window, cx| {
                    let mut rows = Rows::default();
                    for priority in [Priority::Normal, Priority::High, Priority::Urgent] {
                        rows.add(
                            MenuAction::new(
                                format!("priority-{}", priority_label(priority).to_lowercase()),
                                priority_label(priority),
                            )
                            .checked(current_priority == priority),
                            Pick::Priority {
                                id: id.clone(),
                                priority,
                            },
                        );
                    }
                    this.open_menu(Slot::Priority, "Priority", rows, window, cx);
                }))
        } else {
            pill("today-priority-record", false).child(priority_label(task.priority))
        };
        let pills = div()
            .mt(px(10.))
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(6.))
            .child(status_pill)
            .child(priority_pill)
            .when_some(due_text(task), |d, due| {
                d.child(
                    pill("today-due", false)
                        .debug_selector(|| "today-due".into())
                        .child(icon("clock", 12.).text_color(content(0.4)))
                        .child(format!("Due {due}")),
                )
            });

        // --- needs check callout ---------------------------------------------------
        let source_name = match task.source_name.trim() {
            "" => "the source".to_string(),
            name => name.to_string(),
        };
        let callout = task
            .check_reason
            .filter(|_| row.group == Group::NeedsCheck)
            .map(|reason| {
                let text = match reason {
                    CheckReason::DoneButSourceOpen => {
                        format!("You marked this done, but {source_name} still lists it open.")
                    }
                    CheckReason::Missing => {
                        let owner = if task.source_name.trim().is_empty() {
                            "the source's".to_string()
                        } else {
                            format!("{source_name}'s")
                        };
                        format!("It left {owner} list without a clear reason.")
                    }
                };
                let mut actions = div().mt(px(8.)).flex().flex_wrap().gap(px(6.));
                if editable {
                    match reason {
                        CheckReason::DoneButSourceOpen => {
                            let keep = id.clone();
                            let back = id.clone();
                            actions = actions
                                .child(
                                    action("today-keep-done", "Keep done")
                                        .debug_selector(|| "today-keep-done".into())
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.keep_done(keep.clone(), cx)
                                        })),
                                )
                                .child(
                                    action("today-move-todo", "Move back to To do")
                                        .debug_selector(|| "today-move-todo".into())
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.set_status(back.clone(), TODO_ID.into(), cx)
                                        })),
                                );
                        }
                        CheckReason::Missing => {
                            let done = id.clone();
                            let dismiss = id.clone();
                            actions = actions
                                .child(
                                    action("today-mark-done", "Mark done")
                                        .debug_selector(|| "today-mark-done".into())
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.toggle_done(done.clone(), cx)
                                        })),
                                )
                                .child(
                                    action("today-callout-dismiss", "Dismiss")
                                        .debug_selector(|| "today-callout-dismiss".into())
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.dismiss(dismiss.clone(), cx)
                                        })),
                                );
                        }
                    }
                }
                div()
                    .debug_selector(|| "today-check-callout".into())
                    .mt(px(12.))
                    .p(px(10.))
                    .rounded(px(6.))
                    .border_1()
                    .border_color(warning_color().opacity(0.3))
                    .bg(warning_color().opacity(0.08))
                    .child(
                        div()
                            .flex()
                            .items_start()
                            .gap(px(8.))
                            .text_size(px(12.))
                            .line_height(px(18.))
                            .text_color(warning_color())
                            .child(
                                icon("alert-circle", 14.)
                                    .mt(px(2.))
                                    .text_color(warning_color()),
                            )
                            .child(div().flex_1().min_w_0().child(text)),
                    )
                    .child(actions)
            });

        // --- details ---------------------------------------------------------------
        let project_text = project_name.clone().unwrap_or_else(|| "Not set".into());
        let project_value: AnyElement = if editable {
            let id = id.clone();
            menu_value("today-project", project_text, open(Slot::Project))
                .child(self.anchor(Slot::Project).probe())
                .on_click(cx.listener(move |this, _, window, cx| {
                    let current = this.selected_row().and_then(|row| row.task.project.clone());
                    let mut rows = Rows::default();
                    rows.add(
                        MenuAction::new("project-none", "Not set").checked(current.is_none()),
                        Pick::Project {
                            id: id.clone(),
                            project: None,
                        },
                    );
                    for (ix, (label, path)) in this.project_options().into_iter().enumerate() {
                        let checked = current.as_deref() == Some(path.as_str());
                        rows.add(
                            this.project_row(format!("project-{ix}"), label, &path, checked),
                            Pick::Project {
                                id: id.clone(),
                                project: Some(path),
                            },
                        );
                    }
                    this.open_menu(Slot::Project, "Project", rows, window, cx);
                }))
                .into_any_element()
        } else {
            project_text.into_any_element()
        };
        let tag_chips = self.tag_chips("today-detail-tag", &task.id, &row.tags, false, None);
        let tags_value: AnyElement = if editable {
            let id = id.clone();
            let names = row
                .tags
                .iter()
                .map(|tag| tag.name.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            menu_trigger(
                "today-tags-value",
                format!("Tags: {}", if names.is_empty() { "None" } else { &names }),
                open(Slot::Tags),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(4.))
                    .when(tag_chips.is_empty(), |d| d.child("None"))
                    .children(tag_chips),
            )
            .child(icon("chevron-down", 12.).text_color(content(0.4)))
            .child(self.anchor(Slot::Tags).probe())
            .on_click(
                cx.listener(move |this, _, window, cx| this.open_tags_menu(id.clone(), window, cx)),
            )
            .into_any_element()
        } else if tag_chips.is_empty() {
            div()
                .debug_selector(|| "today-tags-record".into())
                .child("None")
                .into_any_element()
        } else {
            div()
                .debug_selector(|| "today-tags-record".into())
                .flex()
                .flex_wrap()
                .items_center()
                .gap(px(4.))
                .children(tag_chips)
                .into_any_element()
        };
        let mono_value: AnyElement = match task.mono_id.as_deref() {
            Some(mono_id) => div()
                .flex()
                .items_center()
                .gap(px(6.))
                .children(self.mono_mark(mono_id, 16.))
                .child(
                    div().min_w_0().truncate().child(
                        self.mono(mono_id)
                            .map_or_else(|| mono_id.to_string(), |m| m.name.clone()),
                    ),
                )
                .into_any_element(),
            None => "None".into_any_element(),
        };
        let source_value: AnyElement = match task.link.as_deref().filter(|l| !l.trim().is_empty()) {
            Some(link) if only_http(link) => {
                let target = link.trim().to_string();
                div()
                    .id("today-link-value")
                    .debug_selector(|| "today-link-value".into())
                    .role(Role::Link)
                    .aria_label("Open link")
                    .truncate()
                    .cursor_pointer()
                    .underline()
                    .child(target.clone())
                    .on_click(move |_, _, cx| cx.open_url(&target))
                    .into_any_element()
            }
            Some(link) => div().truncate().child(link.to_string()).into_any_element(),
            None if task.source_name.trim().is_empty() => "Manual".into_any_element(),
            None => task.source_name.clone().into_any_element(),
        };
        let why = match why_here(task) {
            why if why.is_empty() => "Not set".to_string(),
            why => why,
        };
        let details = div()
            .mt(px(8.))
            .flex()
            .flex_col()
            .gap(px(6.))
            .child(detail_row("folder", "Project", project_value))
            .child(detail_row("bot", "Mono", mono_value))
            .child(detail_row("external-link", "Source", source_value))
            .when(!task.source_status.trim().is_empty(), |d| {
                d.child(detail_row(
                    "circle-dot",
                    "At source",
                    task.source_status.clone(),
                ))
            })
            .child(detail_row("file", "Why here", why))
            .child(detail_row(
                "clock",
                "Open for",
                open_for_text(row.age_days, &task.first_day, current),
            ))
            .child(detail_row("tag", "Tags", tags_value));

        // --- activity --------------------------------------------------------------
        // `load_activity` already cut a past day's timeline at that day.
        let entries = self.activity.iter().collect::<Vec<_>>();
        let note_count = entries
            .iter()
            .filter(|entry| matches!(entry, Activity::Note(_)))
            .count();
        let timeline =
            if entries.is_empty() {
                div()
                    .pt(px(8.))
                    .text_size(px(12.))
                    .text_color(content(0.4))
                    .child("No activity yet.")
            } else {
                div().mt(px(4.)).flex().flex_col().gap(px(4.)).children(
                    entries.iter().enumerate().map(|(ix, entry)| match entry {
                        Activity::Event(event) => self.event_view(ix, event),
                        Activity::Note(note) => self.note_view(ix, note, cx),
                    }),
                )
            };
        let ready = !self.note.read(cx).content.trim().is_empty();
        let needs_hint = editable
            && note_count == 0
            && row.age_days >= AGE_WARNING_DAYS
            && !matches!(row.group, Group::Done | Group::Dismissed);

        let body = div()
            .id("today-detail-scroll")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .p(px(16.))
            .flex()
            .flex_col()
            .child(title)
            .child(pills)
            .children(callout)
            .child(section("DETAILS"))
            .child(details)
            .when(!task.summary.trim().is_empty(), |d| {
                d.child(section("CONTEXT")).child(
                    div()
                        .debug_selector(|| "today-context".into())
                        .mt(px(4.))
                        .text_size(px(12.))
                        .line_height(px(18.))
                        .text_color(content(0.7))
                        .child(self.texts.view(
                            &format!("context:{}", task.id),
                            "Context",
                            task.summary.trim(),
                            cx,
                        )),
                )
            })
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(section("ACTIVITY"))
                    .child(
                        div()
                            .debug_selector(|| "today-activity-count".into())
                            .pt(px(18.))
                            .text_size(px(10.))
                            .text_color(content(0.4))
                            .child(match note_count {
                                1 => "1 note".to_string(),
                                n => format!("{n} notes"),
                            }),
                    ),
            )
            .child(timeline)
            .when(needs_hint, |d| {
                d.child(
                    div()
                        .debug_selector(|| "today-note-hint".into())
                        .pt(px(8.))
                        .text_size(px(12.))
                        .line_height(px(18.))
                        .text_color(warning_color().opacity(0.9))
                        .child(format!(
                            "Open {} days with no note. Add one if something is blocking it.",
                            row.age_days
                        )),
                )
            });

        div()
            .id("today-detail")
            .debug_selector(|| "today-detail".into())
            .size_full()
            .min_h_0()
            .flex()
            .flex_col()
            .child(top_bar)
            .child(body)
            .when(editable, |d| {
                d.child(
                    div()
                        .flex_shrink_0()
                        .px(px(16.))
                        .py(px(10.))
                        .border_t_1()
                        .border_color(content(0.07))
                        .child({
                            let focus = self.note.focus_handle(cx);
                            composer_box::frame(move |window, cx| {
                                content(if focus.contains_focused(window, cx) {
                                    0.2
                                } else {
                                    0.1
                                })
                            })
                            .debug_selector(|| "today-note-box".into())
                            .on_mouse_down(gpui::MouseButton::Left, {
                                let note = self.note.clone();
                                move |_, window, cx| window.focus(&note.focus_handle(cx), cx)
                            })
                            .child(composer_box::field_row(
                                self.note.clone(),
                                composer_box::send_button("today-add-note", ready)
                                    .on_click(cx.listener(|this, _, _, cx| this.add_note(cx))),
                            ))
                        }),
                )
            })
            .children(self.menu_view(Host::Panel, cx))
            .into_any_element()
    }

    fn mono_name(&self, id: &str) -> String {
        self.mono(id)
            .map_or_else(|| id.to_string(), |m| m.name.clone())
    }

    /// One event line: icon, text (a status change shows both statuses' dots) and day.
    fn event_view(&self, ix: usize, event: &TaskEvent) -> Div {
        let mono = event.mono_id.as_deref().map(|id| self.mono_name(id));
        let text = event_text(event, mono.as_deref());
        let line = match (event.kind, &event.from, &event.to) {
            (EventKind::StatusChanged, Some(from), Some(to)) => div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap(px(5.))
                .child("Status:")
                .child(status_dot(from))
                .child(from.name.clone())
                .child("->")
                .child(status_dot(to))
                .child(to.name.clone()),
            _ => div().child(text.clone()),
        };
        div()
            .debug_selector(move || format!("today-activity-{ix}"))
            .flex()
            .items_start()
            .gap(px(8.))
            .py(px(3.))
            .text_size(px(12.))
            .line_height(px(18.))
            .text_color(content(0.6))
            .child(
                icon(event_icon(event.kind), 14.)
                    .mt(px(2.))
                    .text_color(content(0.4)),
            )
            .child(div().flex_1().min_w_0().child(line))
            .child(
                div()
                    .flex_shrink_0()
                    .text_size(px(11.))
                    .text_color(content(0.4))
                    .child(short_day(&event.day)),
            )
    }

    /// A note bubble: author, day, body.
    fn note_view(&self, ix: usize, note: &Note, cx: &mut Context<Self>) -> Div {
        let author = note
            .mono_id
            .as_deref()
            .map_or_else(|| "You".to_string(), |id| self.mono_name(id));
        div()
            .debug_selector(move || format!("today-activity-{ix}"))
            .my(px(3.))
            .px(px(10.))
            .py(px(8.))
            .rounded(px(8.))
            .bg(content(0.05))
            .text_size(px(12.))
            .line_height(px(18.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(8.))
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(content(0.8))
                            .child(author),
                    )
                    .child(
                        div()
                            .flex_shrink_0()
                            .text_size(px(11.))
                            .text_color(content(0.4))
                            .child(short_day(&note.day)),
                    ),
            )
            .child(
                div().text_color(content(0.8)).child(self.texts.view(
                    &format!("note:{}", note.id),
                    "Note",
                    &note.body,
                    cx,
                )),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::super::list::testkit::*;
    use super::*;
    use crate::today::Event;
    use gpui::TestAppContext;
    use monocode_core::today::{
        CheckReason, EventKind, MonoCaller, Note, Status, SyncItem, SyncRequest, Task, TaskEvent,
        statuses::default_statuses,
    };
    use std::sync::{Arc, Mutex};

    #[test]
    fn snooze_offers_tomorrow_then_weekdays() {
        // 2026-11-01 is a Sunday.
        let options = snooze_options("2026-11-01");
        assert_eq!(
            options[0],
            ("2026-11-02".to_string(), "Tomorrow".to_string())
        );
        assert_eq!(options.len(), 5);
        assert_eq!(options[1].0, "2026-11-03");
        assert!(options.iter().all(|(day, _)| {
            let d = NaiveDate::parse_from_str(day, "%Y-%m-%d").unwrap();
            d.weekday() != Weekday::Sat && d.weekday() != Weekday::Sun || day == "2026-11-02"
        }));
        assert!(snooze_options("not a day").is_empty());
    }
    #[test]
    fn links_open_only_over_http() {
        assert!(only_http("https://github.com/a/b/pull/1"));
        assert!(only_http(" HTTP://example.com"));
        assert!(!only_http("file:///etc/passwd"));
        assert!(!only_http("javascript:alert(1)"));
        assert!(!only_http("github.com/a/b"));
    }
    #[test]
    fn the_ref_is_the_key_after_the_provider_or_the_links_last_segment() {
        let mut t = task("clickup:86abc", "A");
        assert_eq!(source_ref(&t).as_deref(), Some("86abc"));
        t.key = "manual:xyz".into();
        assert_eq!(source_ref(&t), None);
        t.key = "plain".into();
        t.link = Some("https://github.com/a/b/pull/12/".into());
        assert_eq!(source_ref(&t).as_deref(), Some("12"));
        t.link = None;
        assert_eq!(source_ref(&t), None);
    }
    #[test]
    fn the_breadcrumb_names_the_project_then_the_source_and_ref() {
        let mut t = task("clickup:86abc", "A");
        t.source_name = "ClickUp".into();
        assert_eq!(
            breadcrumb_text(Some("app"), &t),
            "app / ClickUp 86abc".to_string()
        );
        assert_eq!(breadcrumb_text(None, &t), "ClickUp 86abc".to_string());
        t.key = "manual:x".into();
        t.source_name = "Manual".into();
        assert_eq!(breadcrumb_text(Some("app"), &t), "app / Manual".to_string());
        t.source_name.clear();
        assert_eq!(breadcrumb_text(None, &t), "Manual".to_string());
    }
    #[test]
    fn open_for_reads_days_and_the_first_day() {
        assert_eq!(open_for_text(3, "2026-10-31", true), "3 days, since Oct 31");
        assert_eq!(open_for_text(1, "2026-10-31", true), "1 day, since Oct 31");
        assert_eq!(open_for_text(0, "2026-11-01", true), "Today");
        assert_eq!(open_for_text(0, "2026-11-01", false), "Since Nov 1");
    }
    fn event(kind: EventKind) -> TaskEvent {
        TaskEvent {
            id: "e".into(),
            task_id: "id-a".into(),
            day: "2026-11-01".into(),
            at: 1,
            kind,
            mono_id: None,
            source_id: None,
            from: None,
            to: None,
            check_reason: None,
            detail: None,
            added_tags: Vec::new(),
            removed_tags: Vec::new(),
        }
    }
    #[test]
    fn every_event_kind_reads_as_a_line() {
        let statuses = default_statuses();
        let mut e = event(EventKind::Added);
        assert_eq!(event_text(&e, None), "Added by you");
        e.mono_id = Some("m".into());
        assert_eq!(event_text(&e, Some("Mo")), "First seen by Mo");
        e.mono_id = None;
        e.source_id = Some("cu".into());
        assert_eq!(event_text(&e, None), "First seen by a sync");
        let mut e = event(EventKind::StatusChanged);
        e.from = Some(statuses[0].clone());
        e.to = Some(statuses[1].clone());
        assert_eq!(event_text(&e, None), "Status: To do -> In progress");
        let mut e = event(EventKind::NeedsCheck);
        e.check_reason = Some(CheckReason::Missing);
        assert_eq!(
            event_text(&e, None),
            "Needs check: Left the list without a clear reason"
        );
        assert_eq!(
            event_text(&event(EventKind::CheckCleared), None),
            "Check cleared"
        );
        let mut e = event(EventKind::ClosedBySource);
        assert_eq!(event_text(&e, None), "Closed at source");
        e.detail = Some("merged".into());
        assert_eq!(event_text(&e, None), "Closed at source: merged");
        assert_eq!(
            event_text(&event(EventKind::ReopenedBySource), None),
            "Reopened at source"
        );
        let mut e = event(EventKind::Snoozed);
        e.detail = Some("2026-11-03".into());
        assert_eq!(event_text(&e, None), "Snoozed until Nov 3");
        assert_eq!(event_text(&event(EventKind::Dismissed), None), "Dismissed");
        assert_eq!(event_text(&event(EventKind::KeptDone), None), "Kept done");
    }
    fn tag(id: &str, name: &str) -> Tag {
        Tag {
            id: id.into(),
            name: name.into(),
            color: "#4da3f5".into(),
        }
    }
    #[test]
    fn a_tags_event_reads_as_a_short_line() {
        let mut e = event(EventKind::TagsChanged);
        assert_eq!(event_text(&e, None), "Tags changed");
        e.added_tags = vec![tag("a", "Bug")];
        assert_eq!(event_text(&e, None), "Tag added: Bug");
        e.added_tags = vec![tag("a", "Bug"), tag("b", "UI")];
        assert_eq!(event_text(&e, None), "Tags added: Bug, UI");
        e.added_tags.clear();
        e.removed_tags = vec![tag("a", "Bug")];
        assert_eq!(event_text(&e, None), "Tag removed: Bug");
        e.removed_tags = vec![tag("a", "Bug"), tag("b", "UI")];
        assert_eq!(event_text(&e, None), "Tags removed: Bug, UI");
        e.added_tags = vec![tag("c", "Docs")];
        e.removed_tags = vec![tag("b", "UI")];
        assert_eq!(event_text(&e, None), "Tags: added Docs, removed UI");
    }

    fn selected(
        mode: DayMode,
        age: u32,
        notes: Vec<Note>,
        link: Option<&str>,
    ) -> impl FnOnce(&mut Today) + 'static {
        let link = link.map(str::to_string);
        move |today| {
            let mut t = task("a", "Review it");
            t.link = link;
            t.project = Some("/work/app".into());
            today.view = Some(view(
                mode,
                if mode == DayMode::Past {
                    "2026-10-31"
                } else {
                    "2026-11-01"
                },
                vec![row(Group::CarriedOver, t, age)],
            ));
            today.selected = Some("id-a".into());
            today.activity = notes.into_iter().map(Activity::Note).collect();
        }
    }
    /// A single selected row built by `build`, on the current day.
    fn with_task(
        group: Group,
        build: impl FnOnce(&mut Task) + 'static,
    ) -> impl FnOnce(&mut Today) + 'static {
        move |today| {
            let mut t = task("a", "Review it");
            build(&mut t);
            today.view = Some(view(DayMode::Current, "2026-11-01", vec![row(group, t, 0)]));
            today.selected = Some("id-a".into());
        }
    }
    fn note(day: &str, body: &str) -> Note {
        Note {
            id: format!("n-{day}"),
            task_id: "id-a".into(),
            day: day.into(),
            at: 0,
            mono_id: None,
            body: body.into(),
        }
    }
    fn click(visual: &mut gpui::VisualTestContext, selector: &'static str) {
        visual.refresh().unwrap();
        let bounds = visual
            .debug_bounds(selector)
            .unwrap_or_else(|| panic!("{selector} is drawn"));
        visual.simulate_click(bounds.center(), Default::default());
        visual.refresh().unwrap();
    }

    #[gpui::test]
    fn nothing_selected_shows_the_hint(cx: &mut TestAppContext) {
        let (_, mut visual) = page(cx, Show::Detail, |today| {
            today.view = Some(view(DayMode::Current, "2026-11-01", Vec::new()));
        });
        assert!(has(&mut visual, "today-detail-empty"));
        assert!(!has(&mut visual, "today-detail"));
    }
    #[gpui::test]
    fn a_current_task_has_every_action_pill_and_the_composer(cx: &mut TestAppContext) {
        let (_, mut visual) = page(cx, Show::Detail, |today| {
            selected(
                DayMode::Current,
                0,
                Vec::new(),
                Some("https://example.com/1"),
            )(today);
            today.view.as_mut().unwrap().rows[0].task.due = Some("2026-11-02".into());
        });
        for selector in [
            "today-breadcrumb",
            "today-open-link",
            "today-start-session",
            "today-more",
            "today-status",
            "today-priority",
            "today-due",
            "today-project",
            "today-note-box",
            "today-add-note",
        ] {
            assert!(has(&mut visual, selector), "{selector}");
        }
        assert!(
            !has(&mut visual, "today-note-hint"),
            "a new task needs no nudge"
        );
    }
    #[gpui::test]
    fn the_due_pill_is_hidden_without_a_due(cx: &mut TestAppContext) {
        let (_, mut visual) = page(
            cx,
            Show::Detail,
            selected(DayMode::Current, 0, Vec::new(), None),
        );
        assert!(has(&mut visual, "today-status"));
        assert!(!has(&mut visual, "today-due"));
    }
    #[gpui::test]
    fn open_link_needs_an_http_link(cx: &mut TestAppContext) {
        let (_, mut visual) = page(
            cx,
            Show::Detail,
            selected(DayMode::Current, 0, Vec::new(), Some("file:///etc/passwd")),
        );
        assert!(!has(&mut visual, "today-open-link"));
        let (_, mut visual) = page(
            cx,
            Show::Detail,
            selected(DayMode::Current, 0, Vec::new(), None),
        );
        assert!(!has(&mut visual, "today-open-link"));
    }
    #[gpui::test]
    fn the_source_field_opens_an_http_link(cx: &mut TestAppContext) {
        let (_, mut visual) = page(
            cx,
            Show::Detail,
            selected(
                DayMode::Past,
                0,
                Vec::new(),
                Some(" https://example.com/1 "),
            ),
        );
        click(&mut visual, "today-link-value");
        assert_eq!(cx.opened_url().as_deref(), Some("https://example.com/1"));
    }
    #[gpui::test]
    fn the_source_field_stays_plain_text_for_anything_but_http(cx: &mut TestAppContext) {
        let (_, mut visual) = page(
            cx,
            Show::Detail,
            selected(DayMode::Current, 0, Vec::new(), Some("file:///etc/passwd")),
        );
        assert!(has(&mut visual, "today-detail"));
        assert!(!has(&mut visual, "today-link-value"));
    }
    #[gpui::test]
    fn an_old_task_with_no_note_gets_a_hint_a_noted_one_does_not(cx: &mut TestAppContext) {
        let (_, mut visual) = page(
            cx,
            Show::Detail,
            selected(DayMode::Current, 2, Vec::new(), None),
        );
        assert!(has(&mut visual, "today-note-hint"));
        let (_, mut visual) = page(
            cx,
            Show::Detail,
            selected(
                DayMode::Current,
                2,
                vec![note("2026-11-01", "Waiting on CI")],
                None,
            ),
        );
        assert!(!has(&mut visual, "today-note-hint"));
    }
    #[gpui::test]
    fn a_future_task_has_no_snooze_but_can_start_and_take_notes(cx: &mut TestAppContext) {
        let (_, mut visual) = page(
            cx,
            Show::Detail,
            selected(DayMode::Future, 0, Vec::new(), None),
        );
        // Dismiss is still there, so the More menu is; Snooze is not offered.
        click(&mut visual, "today-more");
        assert!(has(&mut visual, "menu-action-dismiss"));
        assert!(!has(&mut visual, "menu-action-snooze-0"));
        assert!(has(&mut visual, "today-start-session"));
        assert!(has(&mut visual, "today-add-note"));
    }
    #[gpui::test]
    fn a_done_task_has_no_more_menu(cx: &mut TestAppContext) {
        let (_, mut visual) = page(
            cx,
            Show::Detail,
            with_task(Group::Done, |t| t.status = Status::Done),
        );
        assert!(has(&mut visual, "today-start-session"));
        assert!(!has(&mut visual, "today-more"));
    }
    #[gpui::test]
    fn a_past_task_gets_no_note_hint(cx: &mut TestAppContext) {
        let (_, mut visual) = page(
            cx,
            Show::Detail,
            selected(DayMode::Past, 3, Vec::new(), None),
        );
        assert!(!has(&mut visual, "today-note-hint"));
    }
    #[gpui::test]
    fn the_priority_project_and_status_menus_open_from_the_panel(cx: &mut TestAppContext) {
        let (window, mut visual) = page(
            cx,
            Show::Detail,
            selected(DayMode::Current, 0, Vec::new(), None),
        );
        for trigger in [
            "today-more",
            "today-priority",
            "today-project",
            "today-status",
        ] {
            click(&mut visual, trigger);
            let open = window
                .update(&mut visual, |page, _, cx| {
                    page.today.read(cx).menus.is_open()
                })
                .unwrap();
            assert!(open, "{trigger} opens its menu");
            visual.simulate_keystrokes("escape");
            visual.refresh().unwrap();
        }
    }
    #[gpui::test]
    fn the_status_pill_lists_every_status_then_edit_statuses(cx: &mut TestAppContext) {
        let (_, mut visual) = page(
            cx,
            Show::Detail,
            selected(DayMode::Current, 0, Vec::new(), None),
        );
        click(&mut visual, "today-status");
        for selector in [
            "menu-action-status-todo",
            "menu-action-status-in_progress",
            "menu-action-status-done",
            "menu-action-status-edit",
        ] {
            assert!(has(&mut visual, selector), "{selector}");
        }
    }
    #[gpui::test]
    fn the_more_menu_holds_snooze_and_dismiss(cx: &mut TestAppContext) {
        let (_, mut visual) = page(
            cx,
            Show::Detail,
            selected(DayMode::Current, 0, Vec::new(), None),
        );
        click(&mut visual, "today-more");
        assert!(has(&mut visual, "menu-action-snooze-0"));
        assert!(has(&mut visual, "menu-action-dismiss"));
    }
    #[gpui::test]
    fn a_done_but_source_open_check_offers_keep_done_and_move_back(cx: &mut TestAppContext) {
        let (_, mut visual) = page(
            cx,
            Show::Detail,
            with_task(Group::NeedsCheck, |t| {
                t.status = Status::NeedsCheck;
                t.check_reason = Some(CheckReason::DoneButSourceOpen);
            }),
        );
        assert!(has(&mut visual, "today-check-callout"));
        assert!(has(&mut visual, "today-keep-done"));
        assert!(has(&mut visual, "today-move-todo"));
        assert!(!has(&mut visual, "today-mark-done"));
    }
    #[gpui::test]
    fn a_missing_check_offers_mark_done_and_dismiss(cx: &mut TestAppContext) {
        let (_, mut visual) = page(
            cx,
            Show::Detail,
            with_task(Group::NeedsCheck, |t| {
                t.status = Status::NeedsCheck;
                t.check_reason = Some(CheckReason::Missing);
            }),
        );
        assert!(has(&mut visual, "today-check-callout"));
        assert!(has(&mut visual, "today-mark-done"));
        assert!(has(&mut visual, "today-callout-dismiss"));
        assert!(!has(&mut visual, "today-keep-done"));
    }
    #[gpui::test]
    fn no_check_means_no_callout_and_a_past_one_has_no_actions(cx: &mut TestAppContext) {
        let (_, mut visual) = page(
            cx,
            Show::Detail,
            selected(DayMode::Current, 0, Vec::new(), None),
        );
        assert!(!has(&mut visual, "today-check-callout"));
        let (_, mut visual) = page(cx, Show::Detail, |today| {
            with_task(Group::NeedsCheck, |t| {
                t.status = Status::NeedsCheck;
                t.check_reason = Some(CheckReason::Missing);
            })(today);
            today.view.as_mut().unwrap().mode = DayMode::Past;
        });
        assert!(has(&mut visual, "today-check-callout"));
        assert!(!has(&mut visual, "today-mark-done"));
    }
    #[gpui::test]
    fn keep_done_reaches_the_store_and_clears_the_check(cx: &mut TestAppContext) {
        let (window, mut visual) = page(cx, Show::Detail, |today| {
            let caller = MonoCaller {
                id: "m1".into(),
                ..Default::default()
            };
            let request = SyncRequest {
                source: "cu".into(),
                name: Some("ClickUp".into()),
                items: Some(vec![SyncItem {
                    key: "a".into(),
                    title: "Review it".into(),
                    ..Default::default()
                }]),
                ..Default::default()
            };
            let now = monocode_core::time::now_ms();
            let store = crate::runtime::locked(&today.store).unwrap();
            store.today_sync(&request, &caller, now).unwrap();
            store
                .today_complete("a", &monocode_core::today::Actor::User, now)
                .unwrap();
            store.today_sync(&request, &caller, now).unwrap();
            let task = store.today_task("a").unwrap().unwrap();
            assert_eq!(task.check_reason, Some(CheckReason::DoneButSourceOpen));
            let day = store.today_roll(now).unwrap();
            today.view = Some(store.today_view(&day, now).unwrap());
            today.selected = Some(task.id);
        });
        click(&mut visual, "today-keep-done");
        visual.run_until_parked();
        let task = window
            .update(&mut visual, |page, _, cx| {
                let today = page.today.read(cx);
                let store = crate::runtime::locked(&today.store).unwrap();
                store.today_task("a").unwrap().unwrap()
            })
            .unwrap();
        assert_eq!(task.check_reason, None);
        assert!(task.kept_done);
    }
    #[gpui::test]
    fn only_a_manual_task_has_an_editable_title(cx: &mut TestAppContext) {
        let (_, mut visual) = page(
            cx,
            Show::Detail,
            with_task(Group::Today, |t| {
                t.kind = monocode_core::today::Kind::Manual
            }),
        );
        assert!(has(&mut visual, "today-title-input"));
        assert!(!has(&mut visual, "today-title"));
        let (_, mut visual) = page(
            cx,
            Show::Detail,
            with_task(Group::Today, |t| {
                t.kind = monocode_core::today::Kind::Ticket
            }),
        );
        assert!(has(&mut visual, "today-title"));
        assert!(!has(&mut visual, "today-title-input"));
        let (_, mut visual) = page(cx, Show::Detail, |today| {
            with_task(Group::Today, |t| {
                t.kind = monocode_core::today::Kind::Manual
            })(today);
            today.view.as_mut().unwrap().mode = DayMode::Past;
        });
        assert!(has(&mut visual, "today-title"), "a past title is a record");
        assert!(!has(&mut visual, "today-title-input"));
    }
    /// A manual task in the store, selected, with its title field filled.
    fn manual_in_store(today: &mut Today) {
        let now = monocode_core::time::now_ms();
        let store = crate::runtime::locked(&today.store).unwrap();
        let added = store
            .today_add(
                &monocode_core::today::NewTask {
                    title: "Old title".into(),
                    ..Default::default()
                },
                &monocode_core::today::Actor::User,
                now,
            )
            .unwrap();
        let day = store.today_roll(now).unwrap();
        today.view = Some(store.today_view(&day, now).unwrap());
        today.selected = Some(added.id);
    }
    fn submit_title(
        window: &gpui::WindowHandle<Page>,
        visual: &mut gpui::VisualTestContext,
        typed: &str,
    ) {
        let typed = typed.to_string();
        window
            .update(visual, |page, _, cx| {
                page.today.update(cx, |today, cx| {
                    today.title.input.update(cx, |input, cx| {
                        input.set_text(typed, cx);
                        cx.emit(input::Submitted);
                    });
                })
            })
            .unwrap();
        visual.run_until_parked();
    }
    fn stored_title(
        window: &gpui::WindowHandle<Page>,
        visual: &mut gpui::VisualTestContext,
    ) -> (String, String) {
        window
            .update(visual, |page, _, cx| {
                let today = page.today.read(cx);
                let id = today.selected.clone().unwrap();
                let stored = crate::runtime::locked(&today.store)
                    .unwrap()
                    .today_task(&id)
                    .unwrap()
                    .unwrap()
                    .title;
                (stored, today.title.input.read(cx).content.to_string())
            })
            .unwrap()
    }
    #[gpui::test]
    fn submitting_the_title_renames_a_manual_task(cx: &mut TestAppContext) {
        let (window, mut visual) = page(cx, Show::Detail, manual_in_store);
        visual.refresh().unwrap();
        submit_title(&window, &mut visual, "  New title ");
        assert_eq!(stored_title(&window, &mut visual).0, "New title");
    }
    #[gpui::test]
    fn submitting_an_empty_title_resets_the_field_and_writes_nothing(cx: &mut TestAppContext) {
        let (window, mut visual) = page(cx, Show::Detail, manual_in_store);
        visual.refresh().unwrap();
        submit_title(&window, &mut visual, "   ");
        let (stored, field) = stored_title(&window, &mut visual);
        assert_eq!(stored, "Old title");
        assert_eq!(field, "Old title");
    }
    #[test]
    fn relative_snooze_words_read_lowercase_after_until() {
        assert_eq!(snooze_label("Tomorrow"), "Snooze until tomorrow");
        assert_eq!(
            snooze_label("Wednesday, Nov 4"),
            "Snooze until Wednesday, Nov 4"
        );
    }

    #[gpui::test]
    fn context_shows_only_with_a_summary(cx: &mut TestAppContext) {
        let (_, mut visual) = page(
            cx,
            Show::Detail,
            with_task(Group::Today, |t| {
                t.summary = "Fix the login redirect".into()
            }),
        );
        assert!(has(&mut visual, "today-context"));
        let (_, mut visual) = page(
            cx,
            Show::Detail,
            with_task(Group::Today, |t| t.summary = "  \n ".into()),
        );
        assert!(!has(&mut visual, "today-context"));
    }
    #[gpui::test]
    fn the_timeline_merges_events_and_notes_oldest_first(cx: &mut TestAppContext) {
        let (_, mut visual) = page(cx, Show::Detail, |today| {
            selected(DayMode::Current, 0, Vec::new(), None)(today);
            let mut added = event(EventKind::Added);
            added.at = 1;
            let mut n = note("2026-11-01", "Waiting on CI");
            n.at = 2;
            let mut dismissed = event(EventKind::KeptDone);
            dismissed.id = "e2".into();
            dismissed.at = 3;
            today.activity = vec![
                Activity::Event(added),
                Activity::Note(n),
                Activity::Event(dismissed),
            ];
        });
        visual.refresh().unwrap();
        let mut y = |selector: &'static str| {
            f32::from(
                visual
                    .debug_bounds(selector)
                    .unwrap_or_else(|| panic!("{selector} is drawn"))
                    .origin
                    .y,
            )
        };
        assert!(y("today-activity-0") < y("today-activity-1"));
        assert!(y("today-activity-1") < y("today-activity-2"));
        assert!(!has(&mut visual, "today-activity-3"));
        assert!(has(&mut visual, "today-activity-count"));
    }

    fn recorded(
        window: &gpui::WindowHandle<Page>,
        visual: &mut gpui::VisualTestContext,
    ) -> Arc<Mutex<Vec<String>>> {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink = seen.clone();
        window
            .update(visual, |page, _, cx| {
                let today = page.today.clone();
                cx.subscribe(&today, move |_, _, event: &Event, _| {
                    if let Event::StartSession(task) = event {
                        sink.lock().unwrap().push(task.id.clone());
                    }
                })
                .detach();
            })
            .unwrap();
        seen
    }
    #[gpui::test]
    fn start_session_without_a_project_asks_for_one_first(cx: &mut TestAppContext) {
        let (window, mut visual) = page(
            cx,
            Show::Detail,
            with_task(Group::Today, |t| {
                t.project = None;
            }),
        );
        window
            .update(&mut visual, |page, _, cx| {
                page.today.update(cx, |today, _| {
                    today
                        .appearances
                        .insert("/work/app".into(), Default::default());
                })
            })
            .unwrap();
        let seen = recorded(&window, &mut visual);
        click(&mut visual, "today-start-session");
        let open = window
            .update(&mut visual, |page, _, cx| {
                page.today.read(cx).menus.is_open()
            })
            .unwrap();
        assert!(open, "the project menu opens");
        assert!(seen.lock().unwrap().is_empty());
    }
    #[gpui::test]
    fn start_session_with_no_known_project_says_so_and_opens_no_menu(cx: &mut TestAppContext) {
        let (window, mut visual) = page(
            cx,
            Show::Detail,
            with_task(Group::Today, |t| t.project = None),
        );
        let seen = recorded(&window, &mut visual);
        click(&mut visual, "today-start-session");
        let (open, error) = window
            .update(&mut visual, |page, _, cx| {
                let today = page.today.read(cx);
                (today.menus.is_open(), today.action_error.clone())
            })
            .unwrap();
        assert!(!open);
        assert!(error.unwrap().contains("Add a project first"));
        assert!(seen.lock().unwrap().is_empty());
    }
    #[gpui::test]
    fn start_session_with_a_project_starts_at_once(cx: &mut TestAppContext) {
        let (window, mut visual) = page(
            cx,
            Show::Detail,
            with_task(Group::Today, |t| t.project = Some("/work/app".into())),
        );
        let seen = recorded(&window, &mut visual);
        click(&mut visual, "today-start-session");
        assert_eq!(*seen.lock().unwrap(), vec!["id-a".to_string()]);
    }
    #[gpui::test]
    fn a_past_task_is_read_only(cx: &mut TestAppContext) {
        let (_, mut visual) = page(
            cx,
            Show::Detail,
            selected(DayMode::Past, 0, Vec::new(), Some("https://example.com/1")),
        );
        assert!(has(&mut visual, "today-detail"));
        assert!(has(&mut visual, "today-open-link"));
        assert!(has(&mut visual, "today-status"));
        for selector in [
            "today-start-session",
            "today-more",
            "today-priority",
            "today-project",
            "today-note-box",
            "today-add-note",
        ] {
            assert!(!has(&mut visual, selector), "{selector} on a past day");
        }
    }

    /// A store backed page with tags Bug and UI and one manual task, selected.
    fn tagged(
        cx: &mut TestAppContext,
    ) -> (
        gpui::WindowHandle<super::super::list::testkit::Page>,
        gpui::VisualTestContext,
        Entity<Today>,
        Arc<Mutex<Store>>,
        String,
    ) {
        let (window, mut visual) = page(cx, Show::Detail, |_| {});
        let (today, store) = window
            .update(&mut visual, |page, _, cx| {
                (page.today.clone(), page.today.read(cx).store.clone())
            })
            .unwrap();
        let id = {
            let db = crate::runtime::locked(&store).unwrap();
            db.today_add_tag("Bug", "#4da3f5").unwrap();
            db.today_add_tag("UI", "#10b981").unwrap();
            db.today_add(
                &monocode_core::today::NewTask {
                    title: "Tag me".into(),
                    ..Default::default()
                },
                &Actor::User,
                monocode_core::time::now_ms(),
            )
            .unwrap()
            .id
        };
        today.update(&mut visual, |today, cx| {
            today.selected = Some(id.clone());
            today.refresh(cx);
        });
        visual.run_until_parked();
        (window, visual, today, store, id)
    }
    fn tags_of(
        store: &Arc<Mutex<Store>>,
        today: &Entity<Today>,
        visual: &gpui::VisualTestContext,
    ) -> Vec<String> {
        let day = today.read_with(visual, |t, _| t.view.as_ref().unwrap().day.clone());
        let view = crate::runtime::locked(store)
            .unwrap()
            .today_view(&day, monocode_core::time::now_ms())
            .unwrap();
        view.rows[0].tags.iter().map(|t| t.name.clone()).collect()
    }
    fn pick_ids(today: &Entity<Today>, visual: &mut gpui::VisualTestContext) -> Vec<String> {
        today.read_with(visual, |t, _| t.menus.pick_ids())
    }
    fn type_in_search(today: &Entity<Today>, text: &str, visual: &mut gpui::VisualTestContext) {
        let input = today.read_with(visual, |t, cx| t.menus.search_input(cx).unwrap());
        input.update(visual, |input, cx| input.set_text(text.into(), cx));
        visual.run_until_parked();
        visual.refresh().unwrap();
    }

    #[gpui::test]
    fn the_tags_row_opens_a_menu_that_toggles_in_place(cx: &mut TestAppContext) {
        let (_, mut visual, today, store, task_id) = tagged(cx);
        assert!(has(&mut visual, "today-tags-value"));
        click(&mut visual, "today-tags-value");
        assert!(today.read_with(&visual, |t, _| t.menus.is_open()));
        let ids = pick_ids(&today, &mut visual);
        assert_eq!(ids.len(), 3, "{ids:?}");
        assert_eq!(ids.last().map(String::as_str), Some("tag-edit"));
        // Toggle the first tag with the keyboard: Enter on the first action.
        visual.simulate_keystrokes("enter");
        visual.run_until_parked();
        visual.refresh().unwrap();
        assert_eq!(tags_of(&store, &today, &visual), ["Bug"]);
        assert!(
            today.read_with(&visual, |t, _| t.menus.is_open()),
            "stays open"
        );
        let bug = crate::runtime::locked(&store)
            .unwrap()
            .today_tags()
            .unwrap()[0]
            .id
            .clone();
        let chip: &'static str =
            Box::leak(format!("today-detail-tag-{task_id}-{bug}").into_boxed_str());
        assert!(
            has(&mut visual, chip),
            "the chip shows behind the open menu"
        );
    }
    #[gpui::test]
    fn typing_filters_the_tag_rows_and_offers_create_for_a_new_name(cx: &mut TestAppContext) {
        let (_, mut visual, today, _, _) = tagged(cx);
        click(&mut visual, "today-tags-value");
        type_in_search(&today, "ui", &mut visual);
        let ids = pick_ids(&today, &mut visual);
        assert_eq!(ids.len(), 2, "UI and Edit tags only: {ids:?}");
        assert!(ids[0].starts_with("tag-tag-"));
        // An existing name ignoring case offers no Create row.
        type_in_search(&today, "  bUg ", &mut visual);
        assert!(!pick_ids(&today, &mut visual).contains(&"tag-create".to_string()));
        type_in_search(&today, "Docs", &mut visual);
        assert_eq!(pick_ids(&today, &mut visual), ["tag-create", "tag-edit"]);
    }
    #[gpui::test]
    fn create_tag_adds_the_definition_and_puts_it_on_the_task(cx: &mut TestAppContext) {
        let (_, mut visual, today, store, _) = tagged(cx);
        click(&mut visual, "today-tags-value");
        type_in_search(&today, "Docs", &mut visual);
        visual.simulate_keystrokes("enter");
        visual.run_until_parked();
        visual.refresh().unwrap();
        let names: Vec<String> = crate::runtime::locked(&store)
            .unwrap()
            .today_tags()
            .unwrap()
            .into_iter()
            .map(|t| t.name)
            .collect();
        assert_eq!(names, ["Bug", "UI", "Docs"]);
        assert_eq!(tags_of(&store, &today, &visual), ["Docs"]);
        assert!(today.read_with(&visual, |t, _| t.menus.is_open()));
    }
    #[gpui::test]
    fn quick_toggles_keep_each_others_changes(cx: &mut TestAppContext) {
        let (_, mut visual, today, store, _) = tagged(cx);
        click(&mut visual, "today-tags-value");
        // Bug, then UI, before the store answers anything.
        visual.simulate_keystrokes("enter down enter");
        visual.run_until_parked();
        visual.refresh().unwrap();
        assert_eq!(tags_of(&store, &today, &visual), ["Bug", "UI"]);
    }
    #[gpui::test]
    fn create_reuses_a_tag_made_meanwhile(cx: &mut TestAppContext) {
        let (_, mut visual, today, store, _) = tagged(cx);
        click(&mut visual, "today-tags-value");
        // Made elsewhere; the page has not loaded it yet.
        crate::runtime::locked(&store)
            .unwrap()
            .today_add_tag("Docs", "#e5a23c")
            .unwrap();
        type_in_search(&today, "docs", &mut visual);
        assert!(pick_ids(&today, &mut visual).contains(&"tag-create".to_string()));
        visual.simulate_keystrokes("enter");
        visual.run_until_parked();
        visual.refresh().unwrap();
        let count = crate::runtime::locked(&store)
            .unwrap()
            .today_tags()
            .unwrap()
            .len();
        assert_eq!(count, 3);
        assert_eq!(tags_of(&store, &today, &visual), ["Docs"]);
        assert!(today.read_with(&visual, |t, _| t.action_error.is_none()));
    }
    #[gpui::test]
    fn edit_tags_closes_the_menu_and_opens_the_dialog(cx: &mut TestAppContext) {
        let (_, mut visual, today, _, _) = tagged(cx);
        click(&mut visual, "today-tags-value");
        visual.simulate_keystrokes("up enter");
        visual.run_until_parked();
        visual.refresh().unwrap();
        assert!(!today.read_with(&visual, |t, _| t.menus.is_open()));
        assert!(today.read_with(&visual, |t, _| t.tags_dialog.is_some()));
    }
    #[gpui::test]
    fn a_past_day_shows_the_tags_without_a_menu(cx: &mut TestAppContext) {
        let (_, mut visual) = page(cx, Show::Detail, |today| {
            selected(DayMode::Past, 0, Vec::new(), None)(today);
            let tags = vec![tag("tag-1", "Renamed")];
            today.view.as_mut().unwrap().rows[0].tags = tags;
        });
        assert!(has(&mut visual, "today-tags-record"));
        assert!(has(&mut visual, "today-detail-tag-id-a-tag-1"));
        assert!(!has(&mut visual, "today-tags-value"));
    }
}
