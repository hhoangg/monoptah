//! The Board layout: one column per workflow status, cards dragged between
//! columns to change status.
use super::{
    Today,
    list::{drag_ghost, source_line, status_groups, status_title},
    menus::{Host, Pick, Rows, Slot},
};
use crate::{
    inbox::{icon_action, message},
    input::{self, Input},
    popover::MenuAction,
    theme::*,
};
use gpui::{
    AnyElement, Context, Entity, Focusable, FontWeight, Render, Role, SharedString, Subscription,
    Window, div, prelude::*, px,
};
use monocode_core::today::{
    Actor, DayMode, NewTask, Row, Stage, WorkflowStatus, statuses::TODO_ID,
};

const COLUMN_WIDTH: f32 = 264.;

/// A card in flight: which task, and its title for the ghost.
#[derive(Clone)]
pub(super) struct DraggedCard {
    pub id: String,
    pub title: String,
}

/// The card that follows the pointer.
impl Render for DraggedCard {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        drag_ghost()
            .debug_selector(|| "today-card-ghost".into())
            .max_w(px(COLUMN_WIDTH))
            .truncate()
            .text_size(px(12.))
            .line_height(px(16.))
            .child(self.title.clone())
    }
}

/// The open "Add task" field of one column.
pub(crate) struct Adding {
    pub status_id: String,
    pub input: Entity<Input>,
    _submitted: Subscription,
}

/// Board-only page state.
#[derive(Default)]
pub(crate) struct Board {
    /// The column whose `...` menu was opened last. Only its trigger writes the
    /// `Slot::Column` anchor, so the popover hangs from the right header.
    pub menu: Option<String>,
    pub adding: Option<Adding>,
}

impl Today {
    /// The board below the page header and warnings.
    pub(super) fn board_view(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(view) = &self.view else {
            return message("Loading Today…").into_any_element();
        };
        let readonly = view.mode == DayMode::Past;
        let rows = self.visible_rows();
        let columns = status_groups(&self.statuses, &rows, readonly)
            .into_iter()
            .map(|(status, mine)| self.column(&status, &mine, readonly, cx))
            .collect::<Vec<_>>();
        div()
            .size_full()
            .min_h_0()
            .flex()
            .flex_col()
            .child(self.filters_row(cx))
            .child(
                div()
                    .id("today-board-scroll")
                    .debug_selector(|| "today-board-scroll".into())
                    .role(Role::List)
                    .aria_label("Board")
                    .flex_1()
                    .min_h_0()
                    .overflow_x_scroll()
                    .px(px(12.))
                    .pb(px(16.))
                    // The columns are direct children: gpui sizes the scrollable
                    // content from them, so a wrapper without a width would never scroll.
                    .flex()
                    .items_start()
                    .gap(px(10.))
                    .children(columns)
                    .child(
                        icon_action("today-add-status", "Add status", "plus")
                            .debug_selector(|| "today-add-status".into())
                            .flex_shrink_0()
                            .on_click(cx.listener(|this, _, _, cx| this.open_statuses(cx))),
                    ),
            )
            .children(self.menu_view(Host::Board, cx))
            .into_any_element()
    }

    fn column(
        &self,
        status: &WorkflowStatus,
        rows: &[&Row],
        readonly: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let id = status.id.clone();
        let drop_id = status.id.clone();
        let no_finish = self.refuses_finished(status);
        let selector = format!("today-column-{id}");
        let menu_open = self.board.menu.as_deref() == Some(id.as_str());
        let menu_id = id.clone();
        let header = div()
            .flex_shrink_0()
            .px(px(10.))
            .pt(px(8.))
            .pb(px(6.))
            .flex()
            .items_center()
            .gap(px(6.))
            .child(status_title(status, rows.len()).flex_1().debug_selector({
                let id = id.clone();
                move || format!("today-column-title-{id}")
            }))
            .child(
                div()
                    .id(SharedString::from(format!("today-column-menu-{id}")))
                    .debug_selector({
                        let id = id.clone();
                        move || format!("today-column-menu-{id}")
                    })
                    .role(Role::Button)
                    .aria_label(format!("{} column menu", status.name))
                    .aria_expanded(menu_open && self.menus_open(Slot::Column))
                    .relative()
                    .size(px(24.))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(6.))
                    .cursor_pointer()
                    .hover(|s| s.bg(content(0.1)))
                    .child(icon("more-horizontal", 16.))
                    .when(menu_open, |d| d.child(self.anchor(Slot::Column).probe()))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.open_column_menu(menu_id.clone(), window, cx)
                    })),
            );
        let cards = div()
            .id(SharedString::from(format!("today-column-cards-{id}")))
            .min_h_0()
            .overflow_y_scroll()
            .px(px(6.))
            .pb(px(6.))
            .flex()
            .flex_col()
            .gap(px(6.))
            .children(rows.iter().map(|row| self.card(row, readonly, cx)));
        div()
            .id(SharedString::from(selector.clone()))
            .debug_selector(move || selector.clone())
            .w(px(COLUMN_WIDTH))
            .max_h_full()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .rounded(px(8.))
            .border_1()
            .border_color(content(0.07))
            .bg(content(0.03))
            .when(!readonly && !no_finish, |d| {
                d.drag_over::<DraggedCard>(|style, _, _, _| {
                    style
                        .border_color(accent_color().opacity(0.6))
                        .bg(accent_color().opacity(0.06))
                })
                .on_drop(cx.listener(move |this, card: &DraggedCard, _, cx| {
                    this.drop_card(card.id.clone(), drop_id.clone(), cx)
                }))
            })
            .child(header)
            .child(cards)
            .when(!readonly && !no_finish, |d| {
                d.child(self.add_task_area(&id, cx))
            })
            .into_any_element()
    }

    /// The `+ Add task` action of a column, or its field once opened.
    fn add_task_area(&self, status_id: &str, cx: &mut Context<Self>) -> AnyElement {
        if let Some(adding) = self
            .board
            .adding
            .as_ref()
            .filter(|adding| adding.status_id == status_id)
        {
            return div()
                .debug_selector({
                    let id = status_id.to_string();
                    move || format!("today-add-field-{id}")
                })
                .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, _, cx| {
                    if event.keystroke.key == "escape" {
                        this.board.adding = None;
                        cx.stop_propagation();
                        cx.notify();
                    }
                }))
                .flex_shrink_0()
                .mx(px(6.))
                .mb(px(6.))
                .h(px(32.))
                .px(px(8.))
                .flex()
                .items_center()
                .gap(px(6.))
                .rounded(px(6.))
                .border_1()
                .border_color(content(0.15))
                .child(div().flex_1().min_w_0().child(adding.input.clone()))
                .child(
                    div()
                        .id("today-add-cancel")
                        .role(Role::Button)
                        .aria_label("Cancel")
                        .flex_shrink_0()
                        .cursor_pointer()
                        .child(icon("close", 12.))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.board.adding = None;
                            cx.notify();
                        })),
                )
                .into_any_element();
        }
        let id = status_id.to_string();
        let selector = id.clone();
        icon_action(format!("today-add-task-{id}"), "Add task", "plus")
            .debug_selector(move || format!("today-add-task-{selector}"))
            .flex_shrink_0()
            .mx(px(6.))
            .mb(px(6.))
            .justify_start()
            .text_color(content(0.5))
            .on_click(
                cx.listener(move |this, _, window, cx| this.start_add(id.clone(), window, cx)),
            )
            .into_any_element()
    }

    /// A card: the source line, the title and the same chips as a row.
    fn card(&self, row: &Row, readonly: bool, cx: &mut Context<Self>) -> AnyElement {
        let task = &row.task;
        let key = task.key.clone();
        let id = task.id.clone();
        let selected = self.selected.as_deref() == Some(id.as_str());
        let done = row.status.stage == monocode_core::today::Stage::Finished;
        let ghost = DraggedCard {
            id: id.clone(),
            title: task.title.clone(),
        };
        div()
            .id(SharedString::from(format!("today-card-{key}")))
            .debug_selector(move || format!("today-card-{key}"))
            .role(Role::Button)
            .aria_label(task.title.clone())
            .p(px(10.))
            .rounded(px(8.))
            .border_1()
            .border_color(if selected {
                accent_color().opacity(0.7)
            } else {
                content(0.1)
            })
            .bg(content(0.05))
            .cursor_pointer()
            .hover(|s| s.bg(content(0.08)))
            .when(!readonly, |d| {
                d.on_drag(ghost, |ghost, _, _, cx| cx.new(|_| ghost.clone()))
            })
            .child(
                div()
                    .truncate()
                    .text_size(px(11.))
                    .text_color(content(0.45))
                    .child(source_line(task)),
            )
            .child(
                div()
                    .mt(px(2.))
                    .text_size(px(13.))
                    .line_height(px(18.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(content(if done { 0.5 } else { 1. }))
                    .when(done, |d| d.line_through())
                    .child(task.title.clone()),
            )
            .child(self.chips(row))
            .on_click(cx.listener(move |this, _, _, cx| this.select(Some(id.clone()), cx)))
            .into_any_element()
    }

    /// Opens the `...` menu of a column, or closes it when it is already open.
    pub(super) fn open_column_menu(
        &mut self,
        status_id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let toggling =
            self.menus_open(Slot::Column) && self.board.menu.as_deref() == Some(status_id.as_str());
        if !toggling {
            // Another column's menu is open: close it so this one opens, not toggles.
            self.menus.close();
        }
        self.board.menu = Some(status_id);
        let mut rows = Rows::default();
        rows.add(
            MenuAction::new("edit-statuses", "Edit statuses").icon("sliders"),
            Pick::EditStatuses,
        );
        self.open_menu(Slot::Column, "Column menu", rows, window, cx);
    }

    /// A planned (future) day cannot hold done work: its Finished columns take
    /// neither a dropped card nor a new task.
    fn refuses_finished(&self, status: &WorkflowStatus) -> bool {
        status.stage == Stage::Finished
            && self
                .view
                .as_ref()
                .is_some_and(|v| v.mode == DayMode::Future)
    }

    /// A card dropped on a column: the task takes that column's status. A
    /// past day is a record, so nothing moves there.
    pub(super) fn drop_card(&mut self, id: String, status_id: String, cx: &mut Context<Self>) {
        let Some(view) = &self.view else { return };
        if view.mode == DayMode::Past {
            return;
        }
        if self
            .status_of(&status_id)
            .is_some_and(|s| self.refuses_finished(s))
        {
            return;
        }
        match view.rows.iter().find(|row| row.task.id == id) {
            Some(row) if row.status.id != status_id => self.set_status(id, status_id, cx),
            _ => {}
        }
    }

    /// Opens the new task field of a column and focuses it.
    pub(super) fn start_add(
        &mut self,
        status_id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.view.as_ref().is_none_or(|v| v.mode == DayMode::Past) {
            return;
        }
        if self
            .status_of(&status_id)
            .is_some_and(|s| self.refuses_finished(s))
        {
            return;
        }
        let input = cx.new(|cx| Input::field("New task", "Add task", content(0.4), cx));
        let submitted = cx.subscribe(&input, |this, _, _: &input::Submitted, cx| {
            this.submit_column_task(cx)
        });
        window.focus(&input.read(cx).focus_handle(cx), cx);
        self.board.adding = Some(Adding {
            status_id,
            input,
            _submitted: submitted,
        });
        cx.notify();
    }

    /// Enter in a column's field: adds the task to the day on screen, then moves
    /// it to the column. An empty field just closes. A column of the built-in
    /// To do needs no move: new tasks start there.
    pub(super) fn submit_column_task(&mut self, cx: &mut Context<Self>) {
        let Some(adding) = self.board.adding.take() else {
            return;
        };
        cx.notify();
        let Some(view) = &self.view else { return };
        if view.mode == DayMode::Past {
            return;
        }
        let day = view.day.clone();
        let Some(title) = adding.input.update(cx, |input, cx| input.take(cx)) else {
            return;
        };
        let status_id = adding.status_id.clone();
        self.act(
            move |store, now| {
                let new = NewTask {
                    title: title.trim().to_string(),
                    day: Some(day),
                    ..Default::default()
                };
                let task = store.today_add(&new, &Actor::User, now)?;
                if status_id != TODO_ID {
                    store.today_set_status(&task.id, &status_id, &Actor::User, now)?;
                }
                Ok(())
            },
            cx,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::super::list::testkit::*;
    use super::*;
    use crate::runtime::locked;
    use gpui::TestAppContext;
    use monocode_core::{time::now_ms, today::Task};

    fn cards_by_column() -> Vec<Row> {
        vec![
            row_in("todo", task("a", "A"), 0),
            row_in("todo", task("b", "B"), 0),
            row_in("in_progress", task("c", "C"), 0),
        ]
    }
    fn centre(
        visual: &mut gpui::VisualTestContext,
        selector: &'static str,
    ) -> gpui::Bounds<gpui::Pixels> {
        visual.refresh().unwrap();
        visual.debug_bounds(selector).expect(selector)
    }
    fn leak(text: String) -> &'static str {
        Box::leak(text.into_boxed_str())
    }

    #[gpui::test]
    fn the_board_has_a_column_per_status_in_order_with_cards_inside(cx: &mut TestAppContext) {
        let (_, mut visual) = page(cx, Show::Board, |today| {
            today.view = Some(view(DayMode::Current, "2026-11-01", cards_by_column()));
        });
        let todo = centre(&mut visual, "today-column-todo");
        let progress = centre(&mut visual, "today-column-in_progress");
        let done = centre(&mut visual, "today-column-done");
        assert!(todo.origin.x < progress.origin.x && progress.origin.x < done.origin.x);
        for (card, column) in [("a", todo), ("b", todo), ("c", progress)] {
            let bounds = centre(&mut visual, leak(format!("today-card-{card}")));
            assert!(
                column.contains(&bounds.center()),
                "card {card} sits in its column"
            );
        }
        assert!(visual.debug_bounds("today-card-c").unwrap().origin.x >= progress.origin.x);
        assert!(has(&mut visual, "today-add-status"));
        assert!(has(&mut visual, "today-add-task-todo"));
        assert!(has(&mut visual, "today-filter-project"));
    }
    #[gpui::test]
    fn a_card_draws_the_same_chips_as_a_row(cx: &mut TestAppContext) {
        let (_, mut visual) = page(cx, Show::Board, |today| {
            let mut check = row_in("todo", task("chk", "Check"), 3);
            check.group = monocode_core::today::Group::NeedsCheck;
            today.view = Some(view(DayMode::Current, "2026-11-01", vec![check]));
        });
        assert!(has(&mut visual, "today-no-project-chk"));
        assert!(has(&mut visual, "today-badge-needs-check-chk"));
        assert!(has(&mut visual, "today-age-chk"));
    }
    #[gpui::test]
    fn dropping_a_card_on_another_column_changes_the_status_in_the_store(cx: &mut TestAppContext) {
        let (window, mut visual) = page(cx, Show::Board, |_| {});
        let (today, store) = window
            .update(&mut visual, |page, _, cx| {
                (page.today.clone(), page.today.read(cx).store.clone())
            })
            .unwrap();
        let added = locked(&store)
            .unwrap()
            .today_add(
                &NewTask {
                    title: "Move me".into(),
                    ..Default::default()
                },
                &Actor::User,
                now_ms(),
            )
            .unwrap();
        today.update(&mut visual, |today, cx| today.refresh(cx));
        visual.run_until_parked();
        let status_of = |visual: &mut gpui::VisualTestContext| {
            today.read_with(visual, |today, _| today.visible_rows()[0].status.id.clone())
        };
        assert_eq!(status_of(&mut visual), "todo");
        // Dropping on its own column changes nothing.
        today.update(&mut visual, |today, cx| {
            today.drop_card(added.id.clone(), "todo".into(), cx)
        });
        visual.run_until_parked();
        assert_eq!(status_of(&mut visual), "todo");
        today.update(&mut visual, |today, cx| {
            today.drop_card(added.id.clone(), "in_progress".into(), cx)
        });
        visual.run_until_parked();
        assert_eq!(status_of(&mut visual), "in_progress");
        let stored = locked(&store).unwrap().today_view(
            &today.read_with(&visual, |t, _| t.view.as_ref().unwrap().day.clone()),
            now_ms(),
        );
        assert_eq!(stored.unwrap().rows[0].status.id, "in_progress");
        // The card now sits in the new column.
        visual.refresh().unwrap();
        let column = centre(&mut visual, "today-column-in_progress");
        let card = centre(&mut visual, leak(format!("today-card-{}", added.key)));
        assert!(column.contains(&card.center()));
    }
    #[gpui::test]
    fn a_past_day_board_is_read_only(cx: &mut TestAppContext) {
        let (window, mut visual) = page(cx, Show::Board, |_| {});
        let (today, store) = window
            .update(&mut visual, |page, _, cx| {
                (page.today.clone(), page.today.read(cx).store.clone())
            })
            .unwrap();
        let added = locked(&store)
            .unwrap()
            .today_add(
                &NewTask {
                    title: "Frozen".into(),
                    ..Default::default()
                },
                &Actor::User,
                now_ms(),
            )
            .unwrap();
        let day = locked(&store).unwrap().today_roll(now_ms()).unwrap();
        let mut past = view(
            DayMode::Past,
            "2026-10-31",
            vec![Row {
                task: Task {
                    id: added.id.clone(),
                    ..task("frozen", "Frozen")
                },
                ..row_in("todo", task("frozen", "Frozen"), 0)
            }],
        );
        past.current_day = day.clone();
        today.update(&mut visual, |today, cx| {
            today.view = Some(past);
            today.day = Some("2026-10-31".into());
            cx.notify();
        });
        visual.refresh().unwrap();
        assert!(has(&mut visual, "today-card-frozen"));
        assert!(!has(&mut visual, "today-add-task-todo"));
        assert!(has(&mut visual, "today-add-status"));
        today.update(&mut visual, |today, cx| {
            today.drop_card(added.id.clone(), "done".into(), cx)
        });
        window
            .update(&mut visual, |page, window, cx| {
                page.today
                    .update(cx, |today, cx| today.start_add("todo".into(), window, cx))
            })
            .unwrap();
        visual.run_until_parked();
        today.read_with(&visual, |today, _| assert!(today.board.adding.is_none()));
        let stored = locked(&store).unwrap().today_view(&day, now_ms()).unwrap();
        assert_eq!(stored.rows.len(), 1, "nothing was added");
        assert_eq!(stored.rows[0].status.id, "todo", "nothing moved");
    }
    #[gpui::test]
    fn the_column_menu_opens_from_its_own_trigger_and_edits_statuses(cx: &mut TestAppContext) {
        let (window, mut visual) = page(cx, Show::Board, |today| {
            today.view = Some(view(DayMode::Current, "2026-11-01", cards_by_column()));
        });
        let trigger = centre(&mut visual, "today-column-menu-in_progress");
        visual.simulate_click(trigger.center(), Default::default());
        visual.refresh().unwrap();
        let (open, column) = window
            .update(&mut visual, |page, _, cx| {
                let today = page.today.read(cx);
                (today.menus_open(Slot::Column), today.board.menu.clone())
            })
            .unwrap();
        assert!(open);
        assert_eq!(column.as_deref(), Some("in_progress"));
        visual.simulate_keystrokes("enter");
        visual.refresh().unwrap();
        let dialog = window
            .update(&mut visual, |page, _, cx| {
                let today = page.today.read(cx);
                (
                    today.statuses_dialog.is_some(),
                    today.menus_open(Slot::Column),
                )
            })
            .unwrap();
        assert_eq!(
            dialog,
            (true, false),
            "Edit statuses opens the dialog and closes the menu"
        );
    }
    #[gpui::test]
    fn adding_a_task_in_a_column_puts_it_in_that_status(cx: &mut TestAppContext) {
        let (window, mut visual) = page(cx, Show::Board, |_| {});
        let today = window
            .update(&mut visual, |page, _, _| page.today.clone())
            .unwrap();
        today.update(&mut visual, |today, cx| today.refresh(cx));
        visual.run_until_parked();
        window
            .update(&mut visual, |page, window, cx| {
                page.today.update(cx, |today, cx| {
                    today.start_add("in_progress".into(), window, cx)
                })
            })
            .unwrap();
        visual.refresh().unwrap();
        assert!(has(&mut visual, "today-add-field-in_progress"));
        today.update(&mut visual, |today, cx| {
            let input = today.board.adding.as_ref().unwrap().input.clone();
            input.update(cx, |input, cx| input.set_text("Plan the week".into(), cx));
            today.submit_column_task(cx);
        });
        visual.run_until_parked();
        today.read_with(&visual, |today, _| {
            let rows = today.visible_rows();
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0].task.title, "Plan the week");
            assert_eq!(rows[0].status.id, "in_progress");
            assert!(today.board.adding.is_none());
        });
    }

    #[gpui::test]
    fn many_columns_scroll_sideways_to_reach_the_last_one(cx: &mut TestAppContext) {
        let (_, mut visual) = page(cx, Show::Board, |today| {
            let base = today.statuses[0].clone();
            for ix in 0..6 {
                let mut extra = base.clone();
                extra.id = format!("extra-{ix}");
                extra.name = format!("Extra {ix}");
                today.statuses.push(extra);
            }
            today.view = Some(view(DayMode::Current, "2026-11-01", Vec::new()));
        });
        let viewport = centre(&mut visual, "today-board-scroll");
        let last = centre(&mut visual, "today-add-status");
        assert!(
            last.right() > viewport.right(),
            "the last item starts out of view"
        );
        visual.simulate_event(gpui::ScrollWheelEvent {
            position: viewport.center(),
            delta: gpui::ScrollDelta::Pixels(gpui::point(px(-2000.), px(0.))),
            ..Default::default()
        });
        visual.refresh().unwrap();
        let after = centre(&mut visual, "today-add-status");
        assert!(
            after.origin.x < last.origin.x,
            "scrolling moved the columns"
        );
        assert!(
            after.right() <= viewport.right() + px(1.),
            "the last item is reachable"
        );
    }
    #[gpui::test]
    fn escape_closes_the_add_field_and_stops_there(cx: &mut TestAppContext) {
        let (window, mut visual) = page(cx, Show::Board, |today| {
            today.view = Some(view(DayMode::Current, "2026-11-01", Vec::new()));
        });
        window
            .update(&mut visual, |page, window, cx| {
                page.today
                    .update(cx, |today, cx| today.start_add("todo".into(), window, cx))
            })
            .unwrap();
        visual.refresh().unwrap();
        assert!(has(&mut visual, "today-add-field-todo"));
        visual.simulate_keystrokes("escape");
        visual.refresh().unwrap();
        assert!(!has(&mut visual, "today-add-field-todo"));
    }
    #[gpui::test]
    fn a_future_day_refuses_cards_and_new_tasks_in_a_finished_column(cx: &mut TestAppContext) {
        let (window, mut visual) = page(cx, Show::Board, |_| {});
        let (today, store) = window
            .update(&mut visual, |page, _, cx| {
                (page.today.clone(), page.today.read(cx).store.clone())
            })
            .unwrap();
        let added = locked(&store)
            .unwrap()
            .today_add(
                &NewTask {
                    title: "Planned".into(),
                    ..Default::default()
                },
                &Actor::User,
                now_ms(),
            )
            .unwrap();
        let day = locked(&store).unwrap().today_roll(now_ms()).unwrap();
        let planned = Row {
            task: Task {
                id: added.id.clone(),
                ..task("planned", "Planned")
            },
            ..row_in("todo", task("planned", "Planned"), 0)
        };
        today.update(&mut visual, |today, cx| {
            today.view = Some(view(DayMode::Future, "2026-11-02", vec![planned]));
            cx.notify();
        });
        visual.refresh().unwrap();
        assert!(has(&mut visual, "today-add-task-todo"));
        assert!(has(&mut visual, "today-add-task-in_progress"));
        assert!(!has(&mut visual, "today-add-task-done"));
        today.update(&mut visual, |today, cx| {
            today.drop_card(added.id.clone(), "done".into(), cx)
        });
        window
            .update(&mut visual, |page, window, cx| {
                page.today
                    .update(cx, |today, cx| today.start_add("done".into(), window, cx))
            })
            .unwrap();
        visual.run_until_parked();
        today.read_with(&visual, |today, _| assert!(today.board.adding.is_none()));
        let stored = locked(&store).unwrap().today_view(&day, now_ms()).unwrap();
        assert_eq!(stored.rows[0].status.id, "todo", "nothing moved");
    }
}
