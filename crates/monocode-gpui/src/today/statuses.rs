//! The Statuses dialog: the workflow statuses grouped by stage, renamed,
//! recolored, reordered, added and deleted.
//!
//! The dialog keeps only view state (one name input per status, the color
//! picker that is open, the focus); the list itself stays `Today::statuses` and
//! every edit goes through the store, which refreshes the page.
use super::{
    Today,
    list::{color_dot, drag_ghost},
};
use crate::{
    inbox::{action, icon_action, message},
    input::{self, Input},
    modal,
    monos::color_picker::{ColorPicker, Picked, spectrum},
    theme::*,
};
use gpui::{
    AnyElement, Context, CursorStyle, Entity, FocusHandle, Focusable, FontWeight, KeyDownEvent,
    Render, Role, SharedString, Subscription, Window, div, prelude::*, px, rgb,
};
use monocode_core::today::{
    WorkflowStatus,
    statuses::{MAX_NAME, Stage, StatusPatch, format_color, presets},
};

/// Width of every name input, so the swatches line up down the dialog.
const NAME_WIDTH: f32 = 200.;
/// The last cell of a row: "Built in" or the delete action, same width.
const ACTION_WIDTH: f32 = 84.;
const SWATCH: f32 = 18.;
const CARD_WIDTH: f32 = 640.;

/// The suggested name for a new status: "New status", "New status 2", ...
fn default_name(statuses: &[WorkflowStatus]) -> String {
    let taken = |name: &str| {
        statuses
            .iter()
            .any(|s| s.name.trim().to_lowercase() == name.to_lowercase())
    };
    let mut name = "New status".to_owned();
    let mut n = 2;
    while taken(&name) {
        name = format!("New status {n}");
        n += 1;
    }
    name
}

/// Where a dragged status lands: the stage and the index inside it that
/// `Store::today_move_status` takes. `before` is the row it was dropped on
/// (it takes that row's place); `None` is the end of the stage. A built-in
/// never leaves its stage, and a status dropped on itself does not move.
fn drop_target(
    statuses: &[WorkflowStatus],
    id: &str,
    stage: Stage,
    before: Option<&str>,
) -> Option<(Stage, usize)> {
    let moved = statuses.iter().find(|s| s.id == id)?;
    if moved.builtin && moved.stage != stage || before == Some(id) {
        return None;
    }
    let in_stage: Vec<&WorkflowStatus> = statuses.iter().filter(|s| s.stage == stage).collect();
    let index = match before {
        Some(target) => in_stage.iter().position(|s| s.id == target)?,
        None => in_stage.len(),
    };
    Some((stage, index))
}

fn stage_key(stage: Stage) -> &'static str {
    match stage {
        Stage::NotStarted => "not_started",
        Stage::Started => "started",
        Stage::Finished => "finished",
    }
}
fn stage_title(stage: Stage) -> &'static str {
    match stage {
        Stage::NotStarted => "NOT STARTED",
        Stage::Started => "STARTED",
        Stage::Finished => "FINISHED",
    }
}
fn stage_hint(stage: Stage) -> &'static str {
    match stage {
        Stage::NotStarted => "Waiting to begin",
        Stage::Started => "Being worked on",
        Stage::Finished => "Counts as done when syncing",
    }
}

/// What travels with the pointer while a status handle is dragged.
#[derive(Clone)]
struct DraggedStatus {
    id: String,
    name: SharedString,
    color: u32,
    /// The stage a built-in is tied to; it cannot be dropped anywhere else.
    builtin_stage: Option<Stage>,
}
impl Render for DraggedStatus {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        drag_ghost()
            .flex()
            .items_center()
            .gap(px(8.))
            .text_size(px(13.))
            .child(color_dot(self.color))
            .child(self.name.clone())
    }
}

struct RowInput {
    id: String,
    input: Entity<Input>,
    _submitted: Subscription,
    _edited: Subscription,
    /// The name last sent to the store, so Enter then blur saves once.
    committed: Option<String>,
    /// Committing on blur needs the window, so it is wired on first render.
    blur: Option<Subscription>,
}
struct OpenPicker {
    id: String,
    view: Entity<ColorPicker>,
    _picked: Subscription,
}

/// The open dialog's state (name inputs, color being picked).
pub(crate) struct Dialog {
    rows: Vec<RowInput>,
    picker: Option<OpenPicker>,
    focus: FocusHandle,
    focused: bool,
    /// The newest color picked and not yet written, with its status.
    pending: Option<(String, u32)>,
    /// A color write is in flight; the next one waits for it.
    writing: bool,
}
impl Dialog {
    pub(super) fn new(statuses: &[WorkflowStatus], cx: &mut Context<Today>) -> Self {
        Self {
            rows: statuses.iter().map(|s| Self::row(s, cx)).collect(),
            picker: None,
            focus: cx.focus_handle(),
            focused: false,
            pending: None,
            writing: false,
        }
    }
    fn row(status: &WorkflowStatus, cx: &mut Context<Today>) -> RowInput {
        let input = cx.new(|cx| {
            let mut field = Input::field("Status name", "Name", content(0.35), cx);
            field.set_text(status.name.clone(), cx);
            field
        });
        let id = status.id.clone();
        let submitted = cx.subscribe(
            &input,
            move |this: &mut Today, _, _: &input::Submitted, cx| this.commit_status_name(&id, cx),
        );
        let id = status.id.clone();
        let edited = cx.subscribe(&input, move |this: &mut Today, _, _: &input::Changed, _| {
            if let Some(row) = this
                .statuses_dialog
                .as_mut()
                .and_then(|d| d.rows.iter_mut().find(|row| row.id == id))
            {
                row.committed = None;
            }
        });
        RowInput {
            id: status.id.clone(),
            input,
            _submitted: submitted,
            _edited: edited,
            committed: None,
            blur: None,
        }
    }
    pub(super) fn input(&self, id: &str) -> Option<Entity<Input>> {
        self.rows
            .iter()
            .find(|row| row.id == id)
            .map(|row| row.input.clone())
    }
    #[cfg(test)]
    pub(super) fn picker_view(&self) -> Option<Entity<ColorPicker>> {
        self.picker.as_ref().map(|p| p.view.clone())
    }
    /// Brings the inputs in step with the list: new rows get one, removed rows
    /// drop theirs, and a name that is not being edited follows a rename made
    /// elsewhere. Also takes focus on the first call so Escape reaches the dialog.
    fn sync(&mut self, statuses: &[WorkflowStatus], window: &mut Window, cx: &mut Context<Today>) {
        self.rows
            .retain(|row| statuses.iter().any(|s| s.id == row.id));
        if self
            .picker
            .as_ref()
            .is_some_and(|p| !statuses.iter().any(|s| s.id == p.id))
        {
            self.picker = None;
        }
        for status in statuses {
            if !self.rows.iter().any(|row| row.id == status.id) {
                self.rows.push(Self::row(status, cx));
            }
        }
        for row in &mut self.rows {
            let Some(status) = statuses.iter().find(|s| s.id == row.id) else {
                continue;
            };
            let focus = row.input.read(cx).focus_handle(cx);
            if !focus.is_focused(window) && row.input.read(cx).content != status.name {
                let name = status.name.clone();
                row.input.update(cx, |field, cx| field.set_text(name, cx));
            }
            if row.blur.is_none() {
                let id = row.id.clone();
                row.blur = Some(cx.on_focus_out(&focus, window, move |this, _, _, cx| {
                    this.commit_status_name(&id, cx)
                }));
            }
        }
        if !self.focused {
            self.focused = true;
            window.focus(&self.focus, cx);
        }
    }
}

impl Today {
    /// Saves the name typed in a status's input, or puts the old one back when
    /// it is empty or already taken. A name that did not change does nothing.
    fn commit_status_name(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(input) = self.statuses_dialog.as_ref().and_then(|d| d.input(id)) else {
            return;
        };
        let Some(current) = self.status_of(id).map(|s| s.name.clone()) else {
            return;
        };
        let typed = input.read(cx).content.trim().to_owned();
        let already = self
            .statuses_dialog
            .as_ref()
            .and_then(|d| d.rows.iter().find(|row| row.id == id))
            .is_some_and(|row| row.committed.as_deref() == Some(typed.as_str()));
        if typed == current || already {
            return;
        }
        let taken = self
            .statuses
            .iter()
            .any(|s| s.id != id && s.name.to_lowercase() == typed.to_lowercase());
        if typed.is_empty() || taken {
            if taken {
                self.action_error = Some("A status with that name already exists".into());
            }
            input.update(cx, |field, cx| field.set_text(current, cx));
            cx.notify();
            return;
        }
        let name: String = typed.chars().take(MAX_NAME).collect();
        if let Some(row) = self
            .statuses_dialog
            .as_mut()
            .and_then(|d| d.rows.iter_mut().find(|row| row.id == id))
        {
            row.committed = Some(typed);
        }
        self.update_status(
            id.to_owned(),
            StatusPatch {
                name: Some(name),
                color: None,
            },
            cx,
        );
    }
    fn commit_all_status_names(&mut self, cx: &mut Context<Self>) {
        let ids: Vec<String> = self.statuses.iter().map(|s| s.id.clone()).collect();
        for id in ids {
            self.commit_status_name(&id, cx);
        }
    }
    /// Recolors a status from the picker. The dot changes at once; the writes
    /// are serialized so the stored color is always the last one picked.
    fn set_status_color(&mut self, id: &str, color: u32, cx: &mut Context<Self>) {
        if let Some(status) = self.statuses.iter_mut().find(|s| s.id == id) {
            status.color = format_color(color);
        }
        if let Some(dialog) = self.statuses_dialog.as_mut() {
            dialog.pending = Some((id.to_owned(), color));
        }
        cx.notify();
        self.flush_status_color(cx);
    }
    fn flush_status_color(&mut self, cx: &mut Context<Self>) {
        let Some(dialog) = self.statuses_dialog.as_mut() else {
            return;
        };
        if dialog.writing {
            return;
        }
        let Some((id, color)) = dialog.pending.take() else {
            return;
        };
        dialog.writing = true;
        let store = self.store.clone();
        let patch = StatusPatch {
            name: None,
            color: Some(format_color(color)),
        };
        let write = cx.background_executor().spawn(async move {
            super::locked(&store)?
                .today_update_status(&id, &patch, super::now_ms())
                .map(|_| ())
        });
        cx.spawn(async move |this, cx| {
            let result = write.await;
            let _ = this.update(cx, |this, cx| {
                if let Err(error) = result {
                    this.action_error = Some(error);
                }
                let more = this.statuses_dialog.as_mut().is_some_and(|dialog| {
                    dialog.writing = false;
                    dialog.pending.is_some()
                });
                if more {
                    this.flush_status_color(cx);
                } else {
                    this.refresh(cx);
                }
            });
        })
        .detach();
    }
    /// Saves every name still being typed, closes the dialog and hands the
    /// focus back to the page.
    pub(super) fn finish_statuses(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.commit_all_status_names(cx);
        self.close_statuses(cx);
        window.focus(&self.focus, cx);
    }
    /// A status handle was dropped on a row (`before`) or on the end of a stage.
    /// A built-in dropped on another stage is refused without a word.
    pub(super) fn drop_status(
        &mut self,
        id: &str,
        stage: Stage,
        before: Option<&str>,
        cx: &mut Context<Self>,
    ) {
        if let Some((stage, index)) = drop_target(&self.statuses, id, stage, before) {
            self.move_status(id.to_owned(), stage, index, cx);
        }
    }
    /// Opens the color picker under a row, or closes it when it is that row's.
    fn toggle_status_picker(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(color) = self.status_of(id).map(|s| s.rgb()) else {
            return;
        };
        let Some(dialog) = self.statuses_dialog.as_mut() else {
            return;
        };
        if dialog.picker.as_ref().is_some_and(|p| p.id == id) {
            dialog.picker = None;
        } else {
            let view = cx.new(|cx| ColorPicker::new(color, cx));
            let target = id.to_owned();
            let picked = cx.subscribe(&view, move |this: &mut Today, _, event: &Picked, cx| {
                this.set_status_color(&target, event.0, cx)
            });
            dialog.picker = Some(OpenPicker {
                id: id.to_owned(),
                view,
                _picked: picked,
            });
        }
        cx.notify();
    }

    /// The dialog over the page, while open.
    pub(super) fn statuses_dialog_view(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let statuses = self.statuses.clone();
        self.statuses_dialog.as_mut()?.sync(&statuses, window, cx);
        let dialog = self.statuses_dialog.as_ref()?;
        let focus = dialog.focus.clone();
        let sections = Stage::ALL
            .into_iter()
            .map(|stage| self.stage_section(stage, &statuses, cx))
            .collect::<Vec<_>>();
        let height = f32::from(window.viewport_size().height);
        let error = self.action_error.clone();
        let card = modal::card()
            .w(px(CARD_WIDTH))
            .track_focus(&focus)
            .key_context("StatusesDialog")
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                if event.keystroke.key == "escape" {
                    this.finish_statuses(window, cx);
                    cx.stop_propagation();
                }
            }))
            .child(
                modal::card_body()
                    .child(modal::title("Statuses"))
                    .child(
                        modal::body(
                            "Board columns and list groups follow this order. The stage decides \
                             what syncing treats as finished. Drag a status to reorder it or \
                             move it to another stage. Built in statuses can be renamed and \
                             recolored.",
                        )
                        .mt(px(6.))
                        .text_size(px(12.))
                        .line_height(px(18.)),
                    )
                    .child(
                        div()
                            .id("statuses-sections")
                            .mt(px(8.))
                            .max_h(px((height - 260.).max(160.)))
                            .overflow_y_scroll()
                            .flex()
                            .flex_col()
                            .children(sections),
                    )
                    .children(error.map(|text| {
                        message(text)
                            .debug_selector(|| "statuses-error".into())
                            .px_0()
                            .text_color(danger_color())
                    }))
                    .child(
                        modal::actions().child(
                            modal::primary_button("statuses-done", "Done")
                                .debug_selector(|| "statuses-done".into())
                                .cursor_pointer()
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.finish_statuses(window, cx)
                                })),
                        ),
                    ),
            );
        let this = cx.entity();
        Some(
            modal::window_overlay(
                window,
                modal::overlay("statuses", card, move |window, cx| {
                    this.update(cx, |today, cx| today.finish_statuses(window, cx))
                }),
            )
            .into_any_element(),
        )
    }

    fn stage_section(
        &self,
        stage: Stage,
        statuses: &[WorkflowStatus],
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let key = stage_key(stage);
        let rows: Vec<&WorkflowStatus> = statuses.iter().filter(|s| s.stage == stage).collect();
        let open_picker = self
            .statuses_dialog
            .as_ref()
            .and_then(|d| d.picker.as_ref())
            .map(|p| (p.id.clone(), p.view.clone()));
        let new_name = default_name(statuses);
        let first = presets(stage)[0];
        let body = rows
            .iter()
            .enumerate()
            .map(|(ix, status)| {
                let picker = open_picker
                    .as_ref()
                    .filter(|(id, _)| *id == status.id)
                    .map(|(_, view)| view.clone());
                self.status_row(status, ix > 0, picker, cx)
            })
            .collect::<Vec<_>>();
        div()
            .id(SharedString::from(format!("statuses-section-{key}")))
            .debug_selector(move || format!("statuses-section-{key}"))
            .flex()
            .flex_col()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .mt(px(10.))
                    .mb(px(4.))
                    .child(
                        div()
                            .text_size(px(10.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(content(0.4))
                            .child(stage_title(stage)),
                    )
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(content(0.4))
                            .child(stage_hint(stage)),
                    )
                    .child(div().flex_1())
                    .child(
                        action(format!("status-add-{key}"), "+ Add")
                            .debug_selector(move || format!("status-add-{key}"))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.add_status(new_name.clone(), stage, format_color(first), cx)
                            })),
                    ),
            )
            .child(
                div()
                    .id(SharedString::from(format!("statuses-drop-{key}")))
                    .rounded(px(10.))
                    .border_1()
                    .border_color(content(0.1))
                    .bg(content(0.03))
                    .flex()
                    .flex_col()
                    .overflow_hidden()
                    .drag_over::<DraggedStatus>(move |style, dragged, _, _| {
                        if dragged.builtin_stage.is_some_and(|own| own != stage) {
                            style
                        } else {
                            style.bg(accent_color().opacity(0.08))
                        }
                    })
                    .on_drop(cx.listener(move |this, dragged: &DraggedStatus, _, cx| {
                        this.drop_status(&dragged.id, stage, None, cx)
                    }))
                    .children(body),
            )
            .into_any_element()
    }

    fn status_row(
        &self,
        status: &WorkflowStatus,
        divided: bool,
        picker: Option<Entity<ColorPicker>>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let id = status.id.clone();
        let stage = status.stage;
        let color = status.rgb();
        let input = self.statuses_dialog.as_ref().and_then(|d| d.input(&id));
        let payload = DraggedStatus {
            id: id.clone(),
            name: status.name.clone().into(),
            color,
            builtin_stage: status.builtin.then_some(status.stage),
        };
        let handle = div()
            .id(SharedString::from(format!("status-grab-{id}")))
            .debug_selector({
                let id = id.clone();
                move || format!("status-grab-{id}")
            })
            .w(px(20.))
            .flex_shrink_0()
            .flex()
            .items_center()
            .cursor(CursorStyle::OpenHand)
            .child(icon("grip-vertical", 14.).text_color(content(0.3)))
            .on_drag(payload, |drag, _, _, cx| cx.new(|_| drag.clone()));
        let swatches = presets(stage).into_iter().enumerate().map(|(n, preset)| {
            let target = id.clone();
            let chosen = color == preset;
            swatch(format!("status-swatch-{id}-{n}"), chosen)
                .bg(rgb(preset))
                .on_click(
                    cx.listener(move |this, _, _, cx| this.set_status_color(&target, preset, cx)),
                )
        });
        let custom = {
            let target = id.clone();
            let chosen = !presets(stage).contains(&color);
            swatch(format!("status-custom-{id}"), chosen)
                .relative()
                .overflow_hidden()
                .child(div().absolute().inset_0().child(spectrum()))
                .on_click(cx.listener(move |this, _, _, cx| this.toggle_status_picker(&target, cx)))
        };
        let end = if status.builtin {
            div()
                .debug_selector({
                    let id = id.clone();
                    move || format!("status-builtin-{id}")
                })
                .text_size(px(11.))
                .text_color(content(0.4))
                .child("Built in")
                .into_any_element()
        } else {
            let target = id.clone();
            icon_action(format!("status-delete-{id}"), "Delete", "trash")
                .debug_selector({
                    let id = id.clone();
                    move || format!("status-delete-{id}")
                })
                .on_click(cx.listener(move |this, _, _, cx| this.delete_status(target.clone(), cx)))
                .into_any_element()
        };
        let drop_id = id.clone();
        div()
            .id(SharedString::from(format!("status-row-{id}")))
            .debug_selector({
                let id = id.clone();
                move || format!("status-row-{id}")
            })
            .flex()
            .flex_col()
            .when(divided, |row| row.border_t_1().border_color(content(0.08)))
            .drag_over::<DraggedStatus>(move |style, dragged, _, _| {
                if dragged.builtin_stage.is_some_and(|own| own != stage) {
                    style
                } else {
                    style.bg(accent_color().opacity(0.12))
                }
            })
            .on_drop(cx.listener(move |this, dragged: &DraggedStatus, _, cx| {
                cx.stop_propagation();
                this.drop_status(&dragged.id, stage, Some(&drop_id), cx)
            }))
            .child(
                div()
                    .h(px(44.))
                    .px(px(10.))
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .child(handle)
                    .child(color_dot(color))
                    .child(
                        div()
                            .debug_selector({
                                let id = id.clone();
                                move || format!("status-name-{id}")
                            })
                            .w(px(NAME_WIDTH))
                            .flex_shrink_0()
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .border_1()
                            .border_color(content(0.1))
                            .bg(content(0.05))
                            .text_size(px(13.))
                            .children(input),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .children(swatches)
                            .child(custom),
                    )
                    .child(div().flex_1())
                    .child(
                        div()
                            .w(px(ACTION_WIDTH))
                            .flex_shrink_0()
                            .flex()
                            .justify_end()
                            .items_center()
                            .child(end),
                    ),
            )
            .children(picker.map(|view| div().px(px(10.)).pb(px(10.)).child(view)))
            .into_any_element()
    }
}

/// A round color swatch button; a ring marks the chosen one.
fn swatch(id: String, chosen: bool) -> gpui::Stateful<gpui::Div> {
    div()
        .id(SharedString::from(id.clone()))
        .debug_selector(move || id.clone())
        .role(Role::Button)
        .size(px(SWATCH))
        .flex_shrink_0()
        .rounded_full()
        .cursor_pointer()
        .border_2()
        .border_color(if chosen {
            content(0.9)
        } else {
            gpui::transparent_black()
        })
        .font_weight(FontWeight::NORMAL)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input;
    use gpui::{
        Bounds, Entity, Render, TestAppContext, VisualTestContext, WindowBounds, WindowHandle,
        WindowOptions, point, size,
    };
    use monocode_core::today::statuses::{format_color, presets};
    use std::sync::{Arc, Mutex};

    struct Host {
        today: Entity<Today>,
    }
    impl Render for Host {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            self.today.update(cx, |today, cx| {
                div()
                    .size_full()
                    .children(today.statuses_dialog_view(window, cx))
            })
        }
    }

    fn open(cx: &mut TestAppContext) -> (WindowHandle<Host>, VisualTestContext) {
        cx.update(|cx| {
            cx.set_reduce_motion(true);
            crate::popover::bind(cx);
            crate::input::bind(cx);
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
                        Host {
                            today: cx.new(|cx| Today::new(store, cx)),
                        }
                    })
                },
            )
            .unwrap()
        });
        let mut visual = VisualTestContext::from_window(window.into(), cx);
        visual.run_until_parked();
        window
            .update(&mut visual, |host, _, cx| {
                host.today.update(cx, |today, cx| today.open_statuses(cx))
            })
            .unwrap();
        visual.run_until_parked();
        visual.refresh().unwrap();
        (window, visual)
    }
    fn today(window: &WindowHandle<Host>, visual: &mut VisualTestContext) -> Entity<Today> {
        window
            .read_with(visual, |host, _| host.today.clone())
            .unwrap()
    }
    fn list(window: &WindowHandle<Host>, visual: &mut VisualTestContext) -> Vec<WorkflowStatus> {
        let today = today(window, visual);
        today.read_with(visual, |today, _| today.statuses.clone())
    }
    fn settle(visual: &mut VisualTestContext) {
        visual.run_until_parked();
        visual.refresh().unwrap();
    }
    fn click(visual: &mut VisualTestContext, selector: &str) {
        visual.refresh().unwrap();
        let at = bounds(visual, selector).unwrap_or_else(|| panic!("{selector} is drawn"));
        visual.simulate_click(at.center(), Default::default());
        settle(visual);
    }
    fn has(visual: &mut VisualTestContext, selector: &str) -> bool {
        visual.refresh().unwrap();
        bounds(visual, selector).is_some()
    }
    fn bounds(visual: &mut VisualTestContext, selector: &str) -> Option<Bounds<gpui::Pixels>> {
        // `debug_bounds` wants a 'static name; a test may leak a few bytes.
        visual.debug_bounds(Box::leak(selector.to_owned().into_boxed_str()))
    }
    /// Types `text` into a name input the way a user does: focused, then Enter.
    fn rename(
        window: &WindowHandle<Host>,
        visual: &mut VisualTestContext,
        field: &Entity<Input>,
        text: &str,
    ) {
        window
            .update(visual, |_, window, cx| {
                window.focus(&field.read(cx).focus_handle(cx), cx)
            })
            .unwrap();
        field.update(visual, |field, cx| field.set_text(text.into(), cx));
        field.update(visual, |_, cx| cx.emit(input::Submitted));
        settle(visual);
    }
    fn add(
        window: &WindowHandle<Host>,
        visual: &mut VisualTestContext,
        name: &str,
        stage: Stage,
    ) -> WorkflowStatus {
        let today = today(window, visual);
        let name = name.to_owned();
        today.update(visual, |today, cx| {
            today.add_status(name, stage, format_color(presets(stage)[1]), cx)
        });
        settle(visual);
        list(window, visual)
            .into_iter()
            .rfind(|s| s.stage == stage)
            .unwrap()
    }

    fn status(id: &str, stage: Stage, builtin: bool) -> WorkflowStatus {
        WorkflowStatus {
            id: id.into(),
            name: id.into(),
            stage,
            color: format_color(presets(stage)[0]),
            builtin,
        }
    }

    #[test]
    fn new_statuses_get_the_first_free_default_name() {
        let mut list = vec![status("todo", Stage::NotStarted, true)];
        assert_eq!(default_name(&list), "New status");
        list.push(WorkflowStatus {
            name: "new STATUS".into(),
            ..status("a", Stage::Started, false)
        });
        assert_eq!(default_name(&list), "New status 2");
        list.push(WorkflowStatus {
            name: "New status 2".into(),
            ..status("b", Stage::Started, false)
        });
        assert_eq!(default_name(&list), "New status 3");
    }
    #[test]
    fn built_ins_only_drop_inside_their_own_stage() {
        let list = vec![
            status("todo", Stage::NotStarted, true),
            status("c1", Stage::NotStarted, false),
            status("in_progress", Stage::Started, true),
            status("c2", Stage::Started, false),
            status("done", Stage::Finished, true),
        ];
        assert_eq!(drop_target(&list, "todo", Stage::Started, None), None);
        assert_eq!(
            drop_target(&list, "todo", Stage::NotStarted, Some("c1")),
            Some((Stage::NotStarted, 1))
        );
        assert_eq!(
            drop_target(&list, "c2", Stage::NotStarted, None),
            Some((Stage::NotStarted, 2))
        );
        assert_eq!(
            drop_target(&list, "c2", Stage::Finished, Some("done")),
            Some((Stage::Finished, 0))
        );
        assert_eq!(drop_target(&list, "c2", Stage::Started, Some("c2")), None);
        assert_eq!(drop_target(&list, "missing", Stage::Started, None), None);
    }

    #[gpui::test]
    fn the_dialog_shows_three_sections_with_rows_in_order(cx: &mut TestAppContext) {
        let (window, mut visual) = open(cx);
        add(&window, &mut visual, "In review", Stage::Started);
        for section in ["not_started", "started", "finished"] {
            assert!(
                has(&mut visual, &format!("statuses-section-{section}")),
                "{section}"
            );
        }
        let top = |visual: &mut VisualTestContext, id: &str| {
            bounds(visual, &format!("status-row-{id}"))
                .unwrap()
                .origin
                .y
        };
        let ids: Vec<String> = list(&window, &mut visual)
            .iter()
            .map(|s| s.id.clone())
            .collect();
        assert_eq!(ids.len(), 4);
        let ys: Vec<_> = ids.iter().map(|id| top(&mut visual, id)).collect();
        assert!(
            ys.windows(2).all(|w| w[0] < w[1]),
            "rows follow the list order"
        );
    }
    #[gpui::test]
    fn built_ins_say_built_in_and_cannot_be_deleted(cx: &mut TestAppContext) {
        let (window, mut visual) = open(cx);
        let custom = add(&window, &mut visual, "In review", Stage::Started);
        for id in ["todo", "in_progress", "done"] {
            assert!(has(&mut visual, &format!("status-builtin-{id}")), "{id}");
            assert!(!has(&mut visual, &format!("status-delete-{id}")), "{id}");
        }
        assert!(has(&mut visual, &format!("status-delete-{}", custom.id)));
        assert!(!has(&mut visual, &format!("status-builtin-{}", custom.id)));
    }
    #[gpui::test]
    fn add_puts_a_custom_status_at_the_end_of_that_stage(cx: &mut TestAppContext) {
        let (window, mut visual) = open(cx);
        click(&mut visual, "status-add-started");
        click(&mut visual, "status-add-started");
        let started: Vec<_> = list(&window, &mut visual)
            .into_iter()
            .filter(|s| s.stage == Stage::Started)
            .collect();
        let names: Vec<_> = started.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["In progress", "New status", "New status 2"]);
        assert_eq!(started[1].color, format_color(presets(Stage::Started)[0]));
        assert!(!started[1].builtin);
    }
    #[gpui::test]
    fn delete_removes_a_custom_status(cx: &mut TestAppContext) {
        let (window, mut visual) = open(cx);
        let custom = add(&window, &mut visual, "In review", Stage::Started);
        click(&mut visual, &format!("status-delete-{}", custom.id));
        assert!(list(&window, &mut visual).iter().all(|s| s.id != custom.id));
        assert!(!has(&mut visual, &format!("status-row-{}", custom.id)));
    }
    #[gpui::test]
    fn a_swatch_recolors_the_status(cx: &mut TestAppContext) {
        let (window, mut visual) = open(cx);
        click(&mut visual, "status-swatch-todo-1");
        let todo = list(&window, &mut visual).remove(0);
        assert_eq!(todo.color, format_color(presets(Stage::NotStarted)[1]));
    }
    #[gpui::test]
    fn the_gradient_swatch_opens_the_color_picker_and_a_pick_recolors(cx: &mut TestAppContext) {
        let (window, mut visual) = open(cx);
        assert!(!has(&mut visual, "mono-custom-color-picker"));
        click(&mut visual, "status-custom-todo");
        assert!(has(&mut visual, "mono-custom-color-picker"));
        let today = today(&window, &mut visual);
        let picker = today.read_with(&visual, |today, _| {
            today
                .statuses_dialog
                .as_ref()
                .unwrap()
                .picker_view()
                .unwrap()
        });
        picker.update(&mut visual, |_, cx| cx.emit(Picked(0x654321)));
        settle(&mut visual);
        assert_eq!(list(&window, &mut visual)[0].color, "#654321");
        click(&mut visual, "status-custom-todo");
        assert!(!has(&mut visual, "mono-custom-color-picker"));
    }
    #[gpui::test]
    fn several_picks_in_a_row_store_the_last_color(cx: &mut TestAppContext) {
        let (window, mut visual) = open(cx);
        click(&mut visual, "status-custom-todo");
        let today = today(&window, &mut visual);
        let picker = today.read_with(&visual, |today, _| {
            today
                .statuses_dialog
                .as_ref()
                .unwrap()
                .picker_view()
                .unwrap()
        });
        for color in [0x111111, 0x222222, 0x333333, 0x444444] {
            picker.update(&mut visual, |_, cx| cx.emit(Picked(color)));
        }
        // The dot follows at once, before any write finishes.
        assert_eq!(list(&window, &mut visual)[0].color, "#444444");
        settle(&mut visual);
        settle(&mut visual);
        let stored = today.read_with(&visual, |today, _| {
            monocode_core::today::statuses::parse_color(&today.statuses[0].color)
        });
        assert_eq!(stored, Some(0x444444));
        let db = today.read_with(&visual, |today, _| today.store.clone());
        let saved = db.lock().unwrap().today_statuses().unwrap();
        assert_eq!(saved[0].color, "#444444");
    }
    #[gpui::test]
    fn escape_commits_a_name_being_typed(cx: &mut TestAppContext) {
        let (window, mut visual) = open(cx);
        let today = today(&window, &mut visual);
        let field = today.read_with(&visual, |today, _| {
            today
                .statuses_dialog
                .as_ref()
                .unwrap()
                .input("todo")
                .unwrap()
        });
        window
            .update(&mut visual, |_, window, cx| {
                window.focus(&field.read(cx).focus_handle(cx), cx)
            })
            .unwrap();
        field.update(&mut visual, |field, cx| field.set_text("Queued".into(), cx));
        visual.simulate_keystrokes("escape");
        settle(&mut visual);
        settle(&mut visual);
        let db = today.read_with(&visual, |today, _| today.store.clone());
        assert_eq!(
            db.lock().unwrap().today_statuses().unwrap()[0].name,
            "Queued"
        );
    }
    #[gpui::test]
    fn a_taken_name_shows_its_error_inside_the_card(cx: &mut TestAppContext) {
        let (window, mut visual) = open(cx);
        let today = today(&window, &mut visual);
        let field = today.read_with(&visual, |today, _| {
            today
                .statuses_dialog
                .as_ref()
                .unwrap()
                .input("todo")
                .unwrap()
        });
        rename(&window, &mut visual, &field, "done");
        assert!(has(&mut visual, "statuses-error"));
    }
    #[gpui::test]
    fn a_rename_commits_on_enter(cx: &mut TestAppContext) {
        let (window, mut visual) = open(cx);
        let today = today(&window, &mut visual);
        let field = today.read_with(&visual, |today, _| {
            today
                .statuses_dialog
                .as_ref()
                .unwrap()
                .input("todo")
                .unwrap()
        });
        rename(&window, &mut visual, &field, "Backlog");
        assert_eq!(list(&window, &mut visual)[0].name, "Backlog");
    }
    #[gpui::test]
    fn an_empty_or_taken_name_is_put_back(cx: &mut TestAppContext) {
        let (window, mut visual) = open(cx);
        let today = today(&window, &mut visual);
        let field = today.read_with(&visual, |today, _| {
            today
                .statuses_dialog
                .as_ref()
                .unwrap()
                .input("todo")
                .unwrap()
        });
        for text in ["   ", "done"] {
            rename(&window, &mut visual, &field, text);
            assert_eq!(list(&window, &mut visual)[0].name, "To do");
            assert_eq!(
                field.read_with(&visual, |f, _| f.content.to_string()),
                "To do"
            );
        }
    }
    #[gpui::test]
    fn dropping_respects_the_built_in_rule(cx: &mut TestAppContext) {
        let (window, mut visual) = open(cx);
        let custom = add(&window, &mut visual, "In review", Stage::Started);
        let today = today(&window, &mut visual);
        today.update(&mut visual, |today, cx| {
            today.drop_status("todo", Stage::Started, None, cx)
        });
        settle(&mut visual);
        assert_eq!(list(&window, &mut visual)[0].stage, Stage::NotStarted);
        let id = custom.id.clone();
        today.update(&mut visual, |today, cx| {
            today.drop_status(&id, Stage::Finished, Some("done"), cx)
        });
        settle(&mut visual);
        let moved = list(&window, &mut visual)
            .into_iter()
            .find(|s| s.id == custom.id)
            .unwrap();
        assert_eq!(moved.stage, Stage::Finished);
        let order: Vec<_> = list(&window, &mut visual)
            .into_iter()
            .filter(|s| s.stage == Stage::Finished)
            .map(|s| s.id)
            .collect();
        assert_eq!(order, [custom.id, "done".to_owned()]);
    }
    #[gpui::test]
    fn every_name_input_has_the_same_width(cx: &mut TestAppContext) {
        let (window, mut visual) = open(cx);
        let custom = add(
            &window,
            &mut visual,
            "A much longer status name here",
            Stage::Finished,
        );
        let width = |visual: &mut VisualTestContext, id: &str| {
            visual.refresh().unwrap();
            bounds(visual, &format!("status-name-{id}"))
                .unwrap()
                .size
                .width
        };
        let first = width(&mut visual, "todo");
        assert!(first > px(100.));
        for id in ["in_progress", "done", custom.id.as_str()] {
            assert_eq!(width(&mut visual, id), first, "{id}");
        }
    }
    #[gpui::test]
    fn escape_and_done_close_the_dialog(cx: &mut TestAppContext) {
        let (window, mut visual) = open(cx);
        let today = today(&window, &mut visual);
        let closed = std::rc::Rc::new(std::cell::Cell::new(false));
        let seen = closed.clone();
        let _sub = visual.update(|_, cx| {
            cx.subscribe(&today, move |_, event: &crate::today::Event, _| {
                if matches!(event, crate::today::Event::Close) {
                    seen.set(true);
                }
            })
        });
        let open = |visual: &mut VisualTestContext| {
            today.read_with(visual, |today, _| today.statuses_dialog.is_some())
        };
        assert!(open(&mut visual));
        visual.simulate_keystrokes("escape");
        settle(&mut visual);
        assert!(!open(&mut visual));
        assert!(!closed.get(), "Escape in the dialog must not close Today");
        today.update(&mut visual, |today, cx| today.open_statuses(cx));
        settle(&mut visual);
        assert!(open(&mut visual));
        click(&mut visual, "statuses-done");
        assert!(!open(&mut visual));
    }
    #[gpui::test]
    fn a_name_being_edited_survives_a_refresh_and_new_rows_get_inputs(cx: &mut TestAppContext) {
        let (window, mut visual) = open(cx);
        let today = today(&window, &mut visual);
        let field = today.read_with(&visual, |today, _| {
            today
                .statuses_dialog
                .as_ref()
                .unwrap()
                .input("todo")
                .unwrap()
        });
        window
            .update(&mut visual, |_, window, cx| {
                window.focus(&field.read(cx).focus_handle(cx), cx)
            })
            .unwrap();
        field.update(&mut visual, |field, cx| field.set_text("Typing".into(), cx));
        today.update(&mut visual, |today, cx| {
            today.statuses[0].name = "Renamed elsewhere".into();
            cx.notify();
        });
        settle(&mut visual);
        assert_eq!(
            field.read_with(&visual, |f, _| f.content.to_string()),
            "Typing"
        );
        let custom = add(&window, &mut visual, "In review", Stage::Started);
        assert!(today.read_with(&visual, |t, _| {
            t.statuses_dialog
                .as_ref()
                .unwrap()
                .input(&custom.id)
                .is_some()
        }));
    }
}
