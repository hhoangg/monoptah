//! The detail panel: actions, fields, notes timeline, note box.
use super::{
    Today,
    list::{Pick, Rows, Slot, due_text, priority_label, why_here},
    short_day,
};
use crate::{
    inbox::{caption, icon_action, message},
    popover::MenuAction,
    settings::widgets::primary_button,
    theme::*,
};
use chrono::{Datelike, Duration, NaiveDate, Weekday};
use gpui::{AnyElement, Context, Div, FontWeight, Role, div, prelude::*, px};
use monocode_core::{
    store::Store,
    today::{
        AGE_WARNING_DAYS, Actor, DayMode, Group, Kind, Note, Priority, Row, Status, TaskPatch,
    },
};

/// How a task reads in the Status field.
pub(super) fn status_label(row: &Row) -> String {
    match row.group {
        Group::CarriedOver => "Open, carried over".into(),
        Group::Today => "Open".into(),
        Group::NeedsCheck => "Needs check".into(),
        Group::Done => "Done".into(),
        Group::Snoozed => match row.task.snoozed_until.as_deref() {
            Some(day) => format!("Snoozed to {}", short_day(day)),
            None => "Snoozed".into(),
        },
        Group::Dismissed => "Dismissed".into(),
    }
}
fn kind_label(kind: Kind) -> &'static str {
    match kind {
        Kind::Ticket => "Ticket",
        Kind::PullRequest => "Pull request",
        Kind::Issue => "Issue",
        Kind::Manual => "Manual task",
        Kind::Other => "Other",
    }
}
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
/// An epoch-millisecond close time in local time.
fn closed_text(ms: u64) -> String {
    chrono::DateTime::from_timestamp_millis(ms as i64).map_or_else(String::new, |at| {
        at.with_timezone(&chrono::Local)
            .format("%b %-d, %H:%M")
            .to_string()
    })
}
fn only_http(url: &str) -> bool {
    let url = url.trim().to_ascii_lowercase();
    url.starts_with("http://") || url.starts_with("https://")
}

fn field(name: &'static str, value: impl IntoElement) -> Div {
    div()
        .flex()
        .items_start()
        .gap(px(8.))
        .text_size(px(12.))
        .line_height(px(18.))
        .child(
            div()
                .w(px(96.))
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
/// A field value that opens a menu: quiet until hovered.
fn menu_value(id: &'static str, text: String, open: bool) -> gpui::Stateful<Div> {
    div()
        .id(id)
        .debug_selector(|| id.to_string())
        .role(Role::Button)
        .aria_label(text.clone())
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
        .child(div().min_w_0().truncate().child(text))
        .child(icon("chevron-down", 12.).text_color(content(0.4)))
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

        self.note.update(cx, |input, cx| {
            input.set_placeholder("Why is this not done yet?", cx)
        });

        // Past-day notes stop at the viewed day, so a record never reads as newer.
        let notes = self
            .notes
            .iter()
            .filter(|note| note.day.as_str() <= view.day.as_str())
            .collect::<Vec<_>>();

        let mut actions = div().flex().flex_wrap().gap(px(2.)).mx(px(-6.)).mt(px(12.));
        if let Some(link) = task.link.clone().filter(|link| only_http(link)) {
            actions = actions.child(
                icon_action("today-open-link", "Open link", "external-link")
                    .debug_selector(|| "today-open-link".into())
                    .on_click(move |_, _, cx| cx.open_url(link.trim())),
            );
        }
        if editable {
            let started = task.clone();
            actions = actions.child(
                icon_action("today-start-session", "Start session", "play")
                    .debug_selector(|| "today-start-session".into())
                    .on_click(
                        cx.listener(move |this, _, _, cx| this.start_session(started.clone(), cx)),
                    ),
            );
        }
        let open = |slot: Slot| self.menus_open(slot);
        let not_done = task.status != Status::Done;
        // Only an open task can be snoozed or dismissed; the store refuses a done one.
        if current && not_done {
            let current_day = view.current_day.clone();
            let id = id.clone();
            actions = actions.child(
                icon_action("today-snooze", "Snooze", "clock")
                    .debug_selector(|| "today-snooze".into())
                    .aria_expanded(open(Slot::Snooze))
                    .relative()
                    .child(self.anchor(Slot::Snooze).probe())
                    .on_click(cx.listener(move |this, _, window, cx| {
                        let mut rows = Rows::default();
                        for (ix, (day, label)) in
                            snooze_options(&current_day).into_iter().enumerate()
                        {
                            rows.add(
                                MenuAction::new(format!("snooze-{ix}"), label),
                                Pick::Snooze {
                                    id: id.clone(),
                                    day,
                                },
                            );
                        }
                        this.open_menu(Slot::Snooze, "Snooze until", rows, window, cx);
                    })),
            );
        }
        if editable && not_done {
            let id = id.clone();
            actions = actions.child(
                icon_action("today-dismiss", "Dismiss", "close")
                    .debug_selector(|| "today-dismiss".into())
                    .on_click(cx.listener(move |this, _, _, cx| this.dismiss(id.clone(), cx))),
            );
        }

        let priority_value: AnyElement = if editable {
            let id = id.clone();
            let current_priority = task.priority;
            menu_value(
                "today-priority",
                priority_label(task.priority).into(),
                open(Slot::Priority),
            )
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
            .into_any_element()
        } else {
            priority_label(task.priority).into_any_element()
        };

        let project_text = task
            .project
            .as_deref()
            .map_or("Not set".to_string(), |path| self.project_label(path));
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

        let mono_name = task.mono_id.as_deref().map_or("None".to_string(), |id| {
            self.mono(id)
                .map_or_else(|| id.to_string(), |m| m.name.clone())
        });
        let first_seen = match row.age_days {
            // "today" is only true on the current day.
            0 if !current => short_day(&task.first_day),
            0 => format!("{}, today", short_day(&task.first_day)),
            1 => format!("{}, 1 day open", short_day(&task.first_day)),
            n => format!("{}, {n} days open", short_day(&task.first_day)),
        };
        let source = [task.source_name.as_str(), task.source_status.as_str()]
            .into_iter()
            .filter(|part| !part.trim().is_empty())
            .collect::<Vec<_>>()
            .join(" · ");

        let mut fields = div()
            .mt(px(14.))
            .flex()
            .flex_col()
            .gap(px(6.))
            .child(field("Status", status_label(row)))
            .child(field(
                "Source",
                if source.is_empty() {
                    "Manual".into()
                } else {
                    source
                },
            ))
            .child(field("Why here", {
                let why = why_here(task);
                if why.is_empty() {
                    "Not set".to_string()
                } else {
                    why
                }
            }))
            .child(field("Type", kind_label(task.kind)))
            .child(field("Project", project_value))
            .child(field("Mono", mono_name))
            .child(field("Due", due_text(task).unwrap_or("Not set".into())))
            .child(field("Priority", priority_value));
        if let Some(link) = task.link.as_deref().filter(|l| !l.trim().is_empty()) {
            let shown = link.to_string();
            let target = link.trim().to_string();
            fields = fields.child(field(
                "Link",
                if only_http(link) {
                    div()
                        .id("today-link-value")
                        .debug_selector(|| "today-link-value".into())
                        .role(Role::Link)
                        .aria_label("Open link")
                        .truncate()
                        .cursor_pointer()
                        .underline()
                        .child(shown)
                        .on_click(move |_, _, cx| cx.open_url(&target))
                        .into_any_element()
                } else {
                    div().truncate().child(shown).into_any_element()
                },
            ));
        }
        if !task.tags.is_empty() {
            fields = fields.child(field("Tags", task.tags.join(", ")));
        }
        fields = fields.child(field("First seen", first_seen));
        if task.closed_at.is_some() || task.close_reason.is_some() {
            if let Some(reason) = task.close_reason.as_deref().filter(|r| !r.is_empty()) {
                fields = fields.child(field("Closed", reason.to_string()));
            }
            if let Some(at) = task.closed_at {
                fields = fields.child(field("Closed at", closed_text(at)));
            }
        }
        let timeline = if notes.is_empty() {
            div()
                .pt(px(8.))
                .text_size(px(12.))
                .text_color(content(0.4))
                .child("No notes yet.")
        } else {
            div()
                .flex()
                .flex_col()
                .children(notes.iter().map(|note| self.note_view(note)))
        };
        let needs_hint = editable
            && notes.is_empty()
            && row.age_days >= AGE_WARNING_DAYS
            && !matches!(row.group, Group::Done | Group::Dismissed);

        div()
            .id("today-detail")
            .debug_selector(|| "today-detail".into())
            .size_full()
            .min_h_0()
            .overflow_y_scroll()
            .p(px(16.))
            .flex()
            .flex_col()
            .child(
                div()
                    .text_size(px(15.))
                    .line_height(px(20.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(content(1.))
                    .child(task.title.clone()),
            )
            .child(actions)
            .child(fields)
            .child(caption("NOTES").px(px(0.)).pt(px(16.)))
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
            })
            .when(editable, |d| {
                d.child(
                    div()
                        .debug_selector(|| "today-note-box".into())
                        .mt(px(10.))
                        .rounded(px(6.))
                        .border_1()
                        .border_color(content(0.08))
                        .bg(content(0.05))
                        .px(px(8.))
                        .py(px(6.))
                        .child(self.note.clone()),
                )
                .child(
                    div().mt(px(8.)).flex().justify_end().child(
                        primary_button("today-add-note", "Add note", false)
                            .child("Add note")
                            .debug_selector(|| "today-add-note".into())
                            .on_click(cx.listener(|this, _, _, cx| this.add_note(cx))),
                    ),
                )
            })
            .children(self.menu_view(false, cx))
            .into_any_element()
    }

    fn note_view(&self, note: &Note) -> Div {
        let author = note.mono_id.as_deref().map(|id| {
            self.mono(id)
                .map_or_else(|| id.to_string(), |m| m.name.clone())
        });
        div()
            .flex()
            .gap(px(10.))
            .pt(px(8.))
            .text_size(px(12.))
            .child(
                div()
                    .w(px(46.))
                    .flex_shrink_0()
                    .text_color(content(0.4))
                    .child(short_day(&note.day)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_color(content(0.8))
                    .child(note.body.clone())
                    .when_some(author, |d, author| {
                        d.child(
                            div()
                                .mt(px(2.))
                                .text_size(px(11.))
                                .text_color(content(0.4))
                                .child(author),
                        )
                    }),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::super::list::testkit::*;
    use super::*;
    use gpui::TestAppContext;
    use monocode_core::today::{Note, Status, Task};

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
    fn status_reads_with_the_carried_over_wording() {
        let mut t = task("a", "A");
        let row = |group, t: &Task| row(group, t.clone(), 0);
        assert_eq!(
            status_label(&row(Group::CarriedOver, &t)),
            "Open, carried over"
        );
        assert_eq!(status_label(&row(Group::NeedsCheck, &t)), "Needs check");
        t.snoozed_until = Some("2026-11-02".into());
        assert_eq!(status_label(&row(Group::Snoozed, &t)), "Snoozed to Nov 2");
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
            today.notes = notes;
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

    #[gpui::test]
    fn nothing_selected_shows_the_hint(cx: &mut TestAppContext) {
        let (_, mut visual) = page(cx, Show::Detail, |today| {
            today.view = Some(view(DayMode::Current, "2026-11-01", Vec::new()));
        });
        assert!(has(&mut visual, "today-detail-empty"));
        assert!(!has(&mut visual, "today-detail"));
    }
    #[gpui::test]
    fn a_current_task_has_every_action_and_the_note_box(cx: &mut TestAppContext) {
        let (_, mut visual) = page(
            cx,
            Show::Detail,
            selected(
                DayMode::Current,
                0,
                Vec::new(),
                Some("https://example.com/1"),
            ),
        );
        for selector in [
            "today-open-link",
            "today-start-session",
            "today-snooze",
            "today-dismiss",
            "today-priority",
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
    fn the_link_field_opens_an_http_link(cx: &mut TestAppContext) {
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
        visual.refresh().unwrap();
        let value = visual
            .debug_bounds("today-link-value")
            .expect("the link field is clickable");
        visual.simulate_click(value.center(), Default::default());
        assert_eq!(cx.opened_url().as_deref(), Some("https://example.com/1"));
    }
    #[gpui::test]
    fn the_link_field_stays_plain_text_for_anything_but_http(cx: &mut TestAppContext) {
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
    fn a_future_task_cannot_be_snoozed_again_from_the_panel(cx: &mut TestAppContext) {
        let (_, mut visual) = page(
            cx,
            Show::Detail,
            selected(DayMode::Future, 0, Vec::new(), None),
        );
        assert!(!has(&mut visual, "today-snooze"));
        assert!(has(&mut visual, "today-start-session"));
        assert!(has(&mut visual, "today-add-note"));
    }
    #[gpui::test]
    fn a_done_task_cannot_be_snoozed_or_dismissed(cx: &mut TestAppContext) {
        let (_, mut visual) = page(cx, Show::Detail, |today| {
            let mut t = task("a", "Done one");
            t.status = Status::Done;
            today.view = Some(view(
                DayMode::Current,
                "2026-11-01",
                vec![row(Group::Done, t, 0)],
            ));
            today.selected = Some("id-a".into());
        });
        assert!(has(&mut visual, "today-start-session"));
        assert!(!has(&mut visual, "today-snooze"));
        assert!(!has(&mut visual, "today-dismiss"));
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
    fn the_snooze_and_priority_menus_open_from_the_panel(cx: &mut TestAppContext) {
        let (window, mut visual) = page(
            cx,
            Show::Detail,
            selected(DayMode::Current, 0, Vec::new(), None),
        );
        for trigger in ["today-snooze", "today-priority", "today-project"] {
            visual.refresh().unwrap();
            let bounds = visual.debug_bounds(trigger).unwrap();
            visual.simulate_click(bounds.center(), Default::default());
            visual.refresh().unwrap();
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
    fn a_past_task_is_read_only(cx: &mut TestAppContext) {
        let (_, mut visual) = page(
            cx,
            Show::Detail,
            selected(DayMode::Past, 0, Vec::new(), Some("https://example.com/1")),
        );
        assert!(has(&mut visual, "today-detail"));
        assert!(has(&mut visual, "today-open-link"));
        for selector in [
            "today-start-session",
            "today-snooze",
            "today-dismiss",
            "today-priority",
            "today-project",
            "today-note-box",
            "today-add-note",
        ] {
            assert!(!has(&mut visual, selector), "{selector} on a past day");
        }
    }
}
