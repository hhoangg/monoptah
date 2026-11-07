//! The codespace startup rows of a cloud session: one compact status row in the style of the
//! "Worked for ..." row, and the bubble of the message queued while the codespace starts. Both
//! are presentation rows, outside the provider transcript, like the saved draft.
use super::*;
use crate::{
    cloud_start_view::{CloudStartAction, CloudStartSnapshot, StartAction, StartPhase, step_ms},
    right_panel::{spinner, tabular},
    settings::widgets::secondary_button,
};
use monocode_core::cloud_sessions::{Step, StepKind, StepStatus};

const GREEN: u32 = 0x4ade80;
pub(super) const CLOUD_START_ROW: (usize, usize) = (SAVED_DRAFT_START, 1);
pub(super) const CLOUD_QUEUED_ROW: (usize, usize) = (SAVED_DRAFT_START, 2);

/// The steps worth a line: a skipped one did not happen, and the background lifecycle commands
/// only show once they run.
pub(crate) fn visible_steps(steps: &[Step]) -> Vec<&Step> {
    steps
        .iter()
        .filter(|step| match step.kind {
            StepKind::LifecycleCommands => step.status != StepStatus::Pending,
            _ => step.status != StepStatus::Skipped,
        })
        .collect()
}

/// What changes the height of the rows: the layout has to be measured again when it does.
type Signature = (
    Option<(StartPhase, Option<String>, usize, bool)>,
    Option<String>,
);

impl TranscriptView {
    /// Shows the codespace start of this session (and the message queued behind it) as rows.
    /// `None` forgets them. Only a change in which rows exist rebuilds the list; a new log line
    /// or step just repaints.
    pub(crate) fn set_cloud_start(
        &mut self,
        start: Option<Arc<CloudStartSnapshot>>,
        queued: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let shape = (self.cloud_start.is_some(), self.cloud_queued.is_some());
        let before = self.cloud_signature();
        self.cloud_start = start;
        self.cloud_queued = queued;
        self.sync_cloud_clock(cx);
        if shape != (self.cloud_start.is_some(), self.cloud_queued.is_some()) {
            if self.source.is_empty() {
                self.rows.clear();
                self.push_saved_draft();
                self.push_cloud_rows(0);
                self.list.reset(self.rows.len() + 1);
                self.list.scroll_to_end();
            } else {
                self.rebuild_rows(0, cx);
            }
        } else if before != self.cloud_signature() {
            self.remeasure_cloud_rows();
        }
        cx.notify();
    }

    fn cloud_signature(&self) -> Signature {
        (
            self.cloud_start.as_ref().map(|start| {
                (
                    start.phase,
                    start.short_error(),
                    visible_steps(&start.record.steps).len(),
                    !start.log.is_empty(),
                )
            }),
            self.cloud_queued.clone(),
        )
    }

    fn remeasure_cloud_rows(&mut self) {
        for ix in 0..self.rows.len() {
            if matches!(self.rows[ix].body, Body::CloudStart | Body::CloudQueued) {
                self.list.remeasure_items(ix..ix + 1);
            }
        }
    }

    /// Running durations tick once a second while the start runs, even with the shimmer off.
    fn sync_cloud_clock(&mut self, cx: &mut Context<Self>) {
        let live = self
            .cloud_start
            .as_ref()
            .is_some_and(|start| start.phase == StartPhase::Starting);
        if !live {
            self.cloud_clock = None;
        } else if self.cloud_clock.is_none() && !cfg!(test) {
            self.cloud_clock = Some(cx.spawn(async move |this, cx| {
                loop {
                    cx.background_executor()
                        .timer(std::time::Duration::from_secs(1))
                        .await;
                    if this.update(cx, |_, cx| cx.notify()).is_err() {
                        break;
                    }
                }
            }));
        }
    }

    /// Adds the status row (after the first user row, else at the end) and the queued bubble
    /// (at the end) when they are not in `rows` yet. Rows before `display_start` are kept.
    pub(super) fn push_cloud_rows(&mut self, display_start: usize) {
        let row = |key, body| UiRow {
            key,
            body,
            message: usize::MAX,
            first: false,
            last: false,
            work: false,
            work_tail: false,
            thought_phase: None,
        };
        // Once the message is in the conversation it shows as a user row, not as a queued one.
        let delivered = self.source.iter().any(|row| row.is_prompt());
        if self.cloud_queued.is_some()
            && !delivered
            && !self
                .rows
                .iter()
                .any(|row| matches!(row.body, Body::CloudQueued))
        {
            self.rows.push(row(CLOUD_QUEUED_ROW, Body::CloudQueued));
        }
        if self.cloud_start.is_some()
            && !self
                .rows
                .iter()
                .any(|row| matches!(row.body, Body::CloudStart))
        {
            let at = self
                .first_prompt_end()
                .unwrap_or(self.rows.len())
                .max(display_start);
            self.rows.insert(at, row(CLOUD_START_ROW, Body::CloudStart));
        }
    }

    /// The index just after the rows of the first user message.
    fn first_prompt_end(&self) -> Option<usize> {
        let role = |row: &UiRow| {
            self.messages
                .get(row.message)
                .map(|m| (m.role.as_str(), m.turn))
        };
        let first = self
            .rows
            .iter()
            .position(|row| role(row).is_some_and(|(role, _)| role == "user"))?;
        let turn = role(&self.rows[first])?.1;
        let mut end = first + 1;
        while self.rows.get(end).is_some_and(|row| {
            role(row).is_some_and(|(role, t)| {
                t == turn
                    && (role == "user" || role == monocode_core::transcript::attachments::ROLE)
            })
        }) {
            end += 1;
        }
        Some(end)
    }

    fn toggle_cloud_steps(&mut self, open: bool, cx: &mut Context<Self>) {
        if let Some(start) = &self.cloud_start {
            self.cloud_steps_open = Some((start.phase, open));
        }
        self.remeasure_cloud_rows();
        cx.notify();
    }

    /// Whether the status row shows its steps: open while starting, closed otherwise, unless the
    /// user chose for this phase.
    pub(crate) fn cloud_steps_open(&self) -> bool {
        let Some(start) = &self.cloud_start else {
            return false;
        };
        match self.cloud_steps_open {
            Some((phase, open)) if phase == start.phase => open,
            _ => start.phase == StartPhase::Starting,
        }
    }

    fn cloud_action(&self, action: StartAction, cx: &mut Context<Self>) {
        cx.emit(CloudStartAction { action });
    }

    fn cloud_step_line(&self, step: &Step, now: u64) -> gpui::AnyElement {
        let duration = step_ms(step, now).map(crate::shell::live_elapsed);
        let (mark, label_alpha): (gpui::AnyElement, f32) = match step.status {
            StepStatus::Done => (
                icon("check", 12.).text_color(rgb(GREEN)).into_any_element(),
                0.7,
            ),
            StepStatus::Running => (
                div()
                    .text_color(warning_color())
                    .child(spinner(
                        ("cloud-step-spin", step.kind as usize),
                        "loader",
                        12.,
                    ))
                    .into_any_element(),
                1.,
            ),
            StepStatus::Failed => (
                icon("x", 12.).text_color(danger_color()).into_any_element(),
                1.,
            ),
            StepStatus::Pending | StepStatus::Skipped => (
                div()
                    .size(px(8.))
                    .m(px(2.))
                    .rounded_full()
                    .border_1()
                    .border_color(content(0.3))
                    .into_any_element(),
                0.4,
            ),
        };
        let failed = step.status == StepStatus::Failed;
        let detail = matches!(step.status, StepStatus::Running | StepStatus::Done)
            .then(|| step.detail.clone())
            .flatten()
            .filter(|detail| !detail.is_empty());
        div()
            .id(("cloud-step", step.kind as usize))
            .flex()
            .items_center()
            .gap(px(8.))
            .text_size(px(12.))
            .line_height(px(18.))
            .child(div().size(px(12.)).flex_shrink_0().child(mark))
            .child(
                div()
                    .min_w_0()
                    .flex_1()
                    .flex()
                    .gap(px(6.))
                    .child(
                        div()
                            .flex_shrink_0()
                            .text_color(if failed {
                                danger_color()
                            } else {
                                content(label_alpha)
                            })
                            .child(step.kind.label()),
                    )
                    .when_some(detail, |d, detail| {
                        d.child(
                            div()
                                .min_w_0()
                                .truncate()
                                .text_color(content(0.35))
                                .child(detail),
                        )
                    }),
            )
            .when_some(duration, |d, duration| {
                d.child(
                    div()
                        .flex_shrink_0()
                        .font_features(tabular())
                        .text_color(content(0.4))
                        .child(duration),
                )
            })
            .into_any_element()
    }

    pub(super) fn cloud_start_row(&mut self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let Some(start) = self.cloud_start.clone() else {
            return div().into_any_element();
        };
        let now = monocode_core::time::now_ms();
        let open = self.cloud_steps_open();
        let headline = start.headline();
        let record = &start.record;
        let mark: gpui::AnyElement = match start.phase {
            StartPhase::Starting => div()
                .text_color(warning_color())
                .child(spinner("cloud-start-spin", "loader", 14.))
                .into_any_element(),
            StartPhase::Failed => icon("x", 14.).text_color(danger_color()).into_any_element(),
            StartPhase::Ready => icon("check", 14.).text_color(rgb(GREEN)).into_any_element(),
        };
        let title: gpui::AnyElement = match start.phase {
            StartPhase::Starting if !cx.reduce_motion() => crate::activity::shimmer(
                headline.clone(),
                now.saturating_sub(record.started_at) as f32 / 1000.,
            ),
            StartPhase::Failed => div()
                .text_color(danger_color())
                .child(headline.clone())
                .into_any_element(),
            _ => div().child(headline.clone()).into_any_element(),
        };
        let elapsed = (start.phase != StartPhase::Ready)
            .then(|| crate::shell::live_elapsed(record.elapsed_ms(now)));
        let all: &[Step] = start
            .ready
            .as_ref()
            .map_or(record.steps.as_slice(), |ready| ready.steps.as_slice());
        let steps: Vec<_> = visible_steps(all)
            .into_iter()
            .map(|step| self.cloud_step_line(step, now))
            .collect();
        let tail = (start.phase == StartPhase::Starting)
            .then(|| start.log.iter().rev().find(|line| !line.trim().is_empty()))
            .flatten()
            .cloned();
        let failed = start.phase == StartPhase::Failed;
        let header = div()
            .flex()
            .items_center()
            .gap(px(8.))
            .py_1()
            .child(
                div()
                    .id("cloud-start-toggle")
                    .debug_selector(|| "cloud-start-toggle".into())
                    .role(Role::Button)
                    .aria_label(headline.clone())
                    .aria_expanded(open)
                    .cursor_pointer()
                    .min_w_0()
                    .flex_shrink(1.)
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(
                        icon(
                            if open {
                                "chevron-down"
                            } else {
                                "chevron-right"
                            },
                            14.,
                        )
                        .text_color(content(0.4)),
                    )
                    .child(div().size(px(14.)).flex_shrink_0().child(mark))
                    .child(div().min_w_0().truncate().child(title))
                    .when_some(elapsed, |d, elapsed| {
                        d.child(
                            div()
                                .flex_shrink_0()
                                .font_features(tabular())
                                .text_size(px(12.))
                                .text_color(content(0.35))
                                .child(elapsed),
                        )
                    })
                    .hover(|s| s.text_color(content(0.8)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.toggle_cloud_steps(!open, cx);
                    })),
            )
            .child(
                div()
                    .id("cloud-start-view-log")
                    .debug_selector(|| "cloud-start-view-log".into())
                    .role(Role::Button)
                    .cursor_pointer()
                    .ml_auto()
                    .flex_shrink_0()
                    .rounded(px(4.))
                    .px_1()
                    .text_size(px(12.))
                    .text_color(content(0.4))
                    .hover(|s| s.text_color(content(0.8)).bg(content(0.06)))
                    .child("View log")
                    .on_click(
                        cx.listener(|this, _, _, cx| this.cloud_action(StartAction::ShowPanel, cx)),
                    ),
            );
        let error = failed.then(|| {
            let reason = start
                .short_error()
                .filter(|reason| !reason.is_empty())
                .unwrap_or_else(|| "The codespace failed to start.".into());
            let message = format!("{} The codespace is kept, so a retry reuses it.", reason);
            let button = |id: &'static str, label: &'static str, action, danger| {
                secondary_button(id, label, danger, false)
                    .debug_selector(move || id.into())
                    .child(label)
                    .on_click(cx.listener(move |this, _, _, cx| this.cloud_action(action, cx)))
            };
            div()
                .id("cloud-start-error")
                .debug_selector(|| "cloud-start-error".into())
                .mt_2()
                .flex()
                .flex_col()
                .gap_2()
                .rounded(px(10.))
                .border_1()
                .border_color(danger_color().opacity(0.25))
                .bg(danger_color().opacity(0.08))
                .px(px(12.))
                .py(px(10.))
                .child(
                    div()
                        .id("cloud-start-reason")
                        .role(Role::Alert)
                        .text_size(px(13.))
                        .line_height(px(20.))
                        .text_color(danger_color().opacity(0.9))
                        .child(message),
                )
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap(px(6.))
                        .child(button(
                            "cloud-start-retry",
                            "Retry",
                            StartAction::Retry,
                            false,
                        ))
                        .child(button(
                            "cloud-start-rebuild",
                            "Rebuild container",
                            StartAction::Rebuild,
                            false,
                        ))
                        .child(button(
                            "cloud-start-browser",
                            "Open in browser",
                            StartAction::OpenInBrowser,
                            false,
                        ))
                        .child(button(
                            "cloud-start-delete",
                            "Delete codespace",
                            StartAction::DeleteCodespace,
                            true,
                        )),
                )
        });
        div()
            .id("cloud-start-row")
            .debug_selector(|| "cloud-start-row".into())
            .w_full()
            .flex()
            .justify_center()
            .child(
                div()
                    .w_full()
                    .max_w(px(COLUMN))
                    .min_w_0()
                    .px(px(COLUMN_PAD))
                    .pt_1()
                    .pb(px(12.))
                    .text_size(px(14.))
                    .line_height(px(20.))
                    .text_color(content(0.5))
                    .child(header)
                    .when(open && !steps.is_empty(), |d| {
                        d.child(
                            div()
                                .id("cloud-start-steps")
                                .ml(px(22.))
                                .mt_1()
                                .flex()
                                .flex_col()
                                .gap(px(3.))
                                .children(steps),
                        )
                    })
                    .when_some(tail.filter(|_| open), |d, tail| {
                        d.child(
                            div()
                                .id("cloud-start-tail")
                                .ml(px(22.))
                                .mt_1()
                                .truncate()
                                .font_family(crate::fonts::mono())
                                .text_size(px(11.5))
                                .line_height(px(18.))
                                .text_color(content(0.35))
                                .child(tail),
                        )
                    })
                    .children(error),
            )
            .into_any_element()
    }

    /// The bubble of the message typed while the codespace starts, with its delivery note.
    pub(super) fn cloud_queued_row(&mut self, _cx: &mut Context<Self>) -> gpui::AnyElement {
        let Some(text) = self.cloud_queued.clone() else {
            return div().into_any_element();
        };
        let note = self
            .cloud_start
            .as_ref()
            .and_then(|start| start.queued_note().map(|note| (note, start.phase)));
        div()
            .id("cloud-queued-row")
            .debug_selector(|| "cloud-queued-row".into())
            .w_full()
            .flex()
            .justify_center()
            .child(
                div()
                    .w_full()
                    .max_w(px(COLUMN))
                    .min_w_0()
                    .pl(px(56.))
                    .pr(px(COLUMN_PAD))
                    .pt_1()
                    .pb(px(12.))
                    .flex()
                    .flex_col()
                    .items_end()
                    .text_size(px(14.))
                    .line_height(px(20.))
                    .text_color(content(1.))
                    .child(
                        div()
                            .relative()
                            .max_w(px(576.))
                            .min_w_0()
                            .px_3()
                            .py_2()
                            .child(
                                canvas(
                                    |_, _, _| {},
                                    move |bounds, _, window, _| {
                                        let radius = if bounds.size.height <= px(37.) {
                                            f32::from(bounds.size.height) * 0.5
                                        } else {
                                            12.
                                        };
                                        window.paint_quad(
                                            gpui::fill(bounds, user_bubble_fill())
                                                .corner_radii(px(radius)),
                                        );
                                    },
                                )
                                .absolute()
                                .top_0()
                                .left_0()
                                .size_full(),
                            )
                            .child(div().child(text)),
                    )
                    .when_some(note, |d, (note, phase)| {
                        let failed = phase == StartPhase::Failed;
                        d.child(
                            div()
                                .id("cloud-queued-note")
                                .debug_selector(|| "cloud-queued-note".into())
                                .role(Role::Status)
                                .flex()
                                .items_center()
                                .gap(px(6.))
                                .px_3()
                                .pt_1()
                                .text_size(px(12.))
                                .line_height(px(16.))
                                .text_color(content(0.5))
                                .child(cloud_dot(failed))
                                .child(note),
                        )
                    }),
            )
            .into_any_element()
    }
}

/// The 7px dot of the queued note and the composer chip: amber and pulsing while the codespace
/// starts, red once it failed.
pub(crate) fn cloud_dot(failed: bool) -> gpui::AnyElement {
    let dot = move |opacity: f32| {
        div()
            .size(px(7.))
            .flex_shrink_0()
            .rounded_full()
            .bg(if failed {
                danger_color()
            } else {
                warning_color()
            })
            .opacity(opacity)
    };
    if failed {
        return dot(1.).into_any_element();
    }
    crate::pulse::looping(
        "cloud-dot",
        std::time::Duration::from_millis(1200),
        crate::pulse::Rate::Half,
        move |phase| {
            // Fade between full and 35% opacity once per period.
            let wave = (phase * std::f32::consts::TAU).cos() * 0.5 + 0.5;
            dot(0.35 + 0.65 * wave)
        },
    )
    .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cloud_start_view::fixtures::{failed, ready, starting};
    use gpui::{TestAppContext, WindowHandle};

    fn row(role: &str, text: &str) -> Row {
        Row {
            role: role.into(),
            text: text.into(),
            first: true,
            sent_at: None,
        }
    }
    fn window(cx: &mut TestAppContext) -> WindowHandle<TranscriptView> {
        cx.update(|cx| cx.set_reduce_motion(true));
        cx.update(|cx| {
            cx.open_window(Default::default(), |_, cx| {
                cx.new(|_| TranscriptView::new())
            })
            .unwrap()
        })
    }
    fn bodies(view: &TranscriptView) -> Vec<&'static str> {
        view.rows
            .iter()
            .map(|row| match row.body {
                Body::CloudStart => "status",
                Body::CloudQueued => "queued",
                _ => match view.messages.get(row.message).map(|m| m.role.as_str()) {
                    Some("user") => "user",
                    Some("assistant") => "assistant",
                    _ => "other",
                },
            })
            .collect()
    }

    #[gpui::test]
    fn an_empty_transcript_shows_the_queued_bubble_then_the_status_row(cx: &mut TestAppContext) {
        let window = window(cx);
        window
            .update(cx, |view, _, cx| {
                view.set_cloud_start(Some(starting(2)), Some("fix the bug".into()), cx);
                assert_eq!(bodies(view), ["queued", "status"]);
                // A new log line or step repaints; it does not change the rows.
                view.set_cloud_start(Some(starting(3)), Some("fix the bug".into()), cx);
                assert_eq!(bodies(view), ["queued", "status"]);
                view.set_cloud_start(None, None, cx);
                assert!(view.rows.is_empty());
            })
            .unwrap();
    }

    #[gpui::test]
    fn the_status_row_follows_the_first_user_row_and_later_rows_do_not_panic(
        cx: &mut TestAppContext,
    ) {
        let window = window(cx);
        window
            .update(cx, |view, _, cx| {
                view.set_cloud_start(Some(starting(2)), None, cx);
                assert_eq!(bodies(view), ["status"]);
                view.patch(
                    0,
                    vec![
                        row("user", "Review the file"),
                        row("assistant", "Reading it."),
                        row("user", "And the tests"),
                        row("assistant", "Done."),
                    ],
                    cx,
                );
                let shown = bodies(view);
                assert_eq!(shown.iter().filter(|b| **b == "status").count(), 1);
                let at = shown.iter().position(|b| *b == "status").unwrap();
                assert_eq!(shown[at - 1], "user");
                assert_ne!(shown[at + 1], "user");
                // A rebuild from a later row keeps it where it is, once.
                view.patch(2, vec![row("user", "And the tests (edited)")], cx);
                assert_eq!(bodies(view).iter().filter(|b| **b == "status").count(), 1);
                // The message was delivered: the queued bubble gives way to the user row.
                view.set_cloud_start(Some(ready(554_000)), Some("Review the file".into()), cx);
                assert!(!bodies(view).contains(&"queued"));
            })
            .unwrap();
    }

    #[gpui::test]
    fn a_failed_row_offers_the_four_actions_and_each_emits_its_own(cx: &mut TestAppContext) {
        let window = window(cx);
        let seen = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let log = seen.clone();
        let view = window.root(cx).unwrap();
        let _subscription = cx.update(|cx| {
            cx.subscribe(&view, move |_, event: &CloudStartAction, _| {
                log.borrow_mut().push(event.action)
            })
        });
        window
            .update(cx, |view, _, cx| {
                view.set_cloud_start(
                    Some(failed(2, "cargo fetch failed")),
                    Some("fix the bug".into()),
                    cx,
                )
            })
            .unwrap();
        let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
        visual.run_until_parked();
        visual.refresh().unwrap();
        for (id, action) in [
            ("cloud-start-retry", StartAction::Retry),
            ("cloud-start-rebuild", StartAction::Rebuild),
            ("cloud-start-browser", StartAction::OpenInBrowser),
            ("cloud-start-delete", StartAction::DeleteCodespace),
            ("cloud-start-view-log", StartAction::ShowPanel),
        ] {
            let bounds = visual.debug_bounds(id).unwrap_or_else(|| panic!("{id}"));
            visual.simulate_click(bounds.center(), Default::default());
            assert_eq!(seen.borrow().last(), Some(&action), "{id}");
        }
        assert!(visual.debug_bounds("cloud-queued-note").is_some());
    }

    #[gpui::test]
    fn steps_are_open_while_starting_and_closed_otherwise_until_the_user_chooses(
        cx: &mut TestAppContext,
    ) {
        let window = window(cx);
        window
            .update(cx, |view, _, cx| {
                view.set_cloud_start(Some(starting(2)), None, cx);
                assert!(view.cloud_steps_open());
                view.toggle_cloud_steps(false, cx);
                assert!(!view.cloud_steps_open());
                view.set_cloud_start(Some(ready(1000)), None, cx);
                assert!(!view.cloud_steps_open());
                view.toggle_cloud_steps(true, cx);
                assert!(view.cloud_steps_open());
            })
            .unwrap();
    }
}
