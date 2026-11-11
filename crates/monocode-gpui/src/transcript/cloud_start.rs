//! The cloud startup rows of a cloud session: one compact status row (mark, headline, elapsed
//! time and "View log"; the steps and the log live in the right panel) in the style of the
//! "Worked for ..." row, with the failure box and its actions under it, and the bubble of the message queued while the codespace starts. Both
//! are presentation rows, outside the provider transcript, like the saved draft.
use super::*;
use crate::{
    cloud_start_view::{CloudStartAction, CloudStartSnapshot, StartAction, StartPhase},
    right_panel::{spinner, tabular},
    settings::widgets::secondary_button,
};

const GREEN: u32 = 0x4ade80;
pub(super) const CLOUD_START_ROW: (usize, usize) = (SAVED_DRAFT_START, 1);
pub(super) const CLOUD_QUEUED_ROW: (usize, usize) = (SAVED_DRAFT_START, 2);

/// What changes the height of the rows: the layout has to be measured again when it does.
type Signature = (Option<(StartPhase, Option<String>)>, Option<String>);

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
            self.cloud_start
                .as_ref()
                .map(|start| (start.phase, start.short_error())),
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
        self.push_moved_row();
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

    fn cloud_action(&self, action: StartAction, cx: &mut Context<Self>) {
        cx.emit(CloudStartAction { action });
    }

    pub(super) fn cloud_start_row(&mut self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let Some(start) = self.cloud_start.clone() else {
            return div().into_any_element();
        };
        let now = monocode_core::time::now_ms();
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
        let failed = start.phase == StartPhase::Failed;
        let header = div()
            .flex()
            .items_center()
            .gap(px(8.))
            .py_1()
            .child(
                div()
                    .id("cloud-start-headline")
                    .debug_selector(|| "cloud-start-headline".into())
                    .role(Role::Status)
                    .aria_label(headline.clone())
                    .min_w_0()
                    .flex_shrink(1.)
                    .flex()
                    .items_center()
                    .gap(px(8.))
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
                    }),
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
            let message = start.failure_message();
            let delete_label = start.delete_label();
            let button = |id: &'static str, label: String, action, danger| {
                secondary_button(id, label.clone(), danger, false)
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
                            "Retry".into(),
                            StartAction::Retry,
                            false,
                        ))
                        .when(start.offers_rebuild(), |d| {
                            d.child(button(
                                "cloud-start-rebuild",
                                "Rebuild container".into(),
                                StartAction::Rebuild,
                                false,
                            ))
                        })
                        .when(start.offers_browser(), |d| {
                            d.child(button(
                                "cloud-start-browser",
                                "Open in browser".into(),
                                StartAction::OpenInBrowser,
                                false,
                            ))
                        })
                        .child(button(
                            "cloud-start-delete",
                            delete_label,
                            StartAction::DeleteCodespace,
                            true,
                        ))
                        .when_some(start.help_url(), |d, url| {
                            d.child(
                                secondary_button("cloud-start-help", "How to fix", false, false)
                                    .debug_selector(|| "cloud-start-help".into())
                                    .child("How to fix")
                                    .on_click(move |_, _, cx| cx.open_url(url)),
                            )
                        }),
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

/// One line of a step list in the style of the Codespace panel: a mark for the status, the
/// label and an optional note on the right. The "Continue in cloud" dialog draws the steps of
/// a move with it.
pub(crate) fn step_row(
    id: SharedString,
    label: impl Into<SharedString>,
    status: monocode_core::cloud_sessions::StepStatus,
    note: Option<String>,
) -> gpui::AnyElement {
    use monocode_core::cloud_sessions::StepStatus;
    let ring = |alpha: f32| {
        div()
            .size(px(8.))
            .m(px(2.))
            .rounded_full()
            .border_1()
            .border_color(content(alpha))
            .into_any_element()
    };
    let (mark, color, label_alpha): (gpui::AnyElement, _, f32) = match status {
        StepStatus::Done => (
            icon("check", 12.).text_color(rgb(GREEN)).into_any_element(),
            content(0.8),
            0.8,
        ),
        StepStatus::Running => (
            spinner(SharedString::from(format!("{id}-spin")), "loader", 12.),
            content(0.45),
            1.,
        ),
        StepStatus::Failed => (
            icon("alert-circle", 12.)
                .text_color(danger_color())
                .into_any_element(),
            danger_color(),
            1.,
        ),
        StepStatus::Pending => (ring(0.3), content(0.45), 0.45),
        StepStatus::Skipped => (ring(0.15), content(0.3), 0.3),
    };
    let selector = id.clone();
    div()
        .id(id)
        .debug_selector(move || selector.to_string())
        .px_3()
        .py(px(3.))
        .flex()
        .items_center()
        .gap_2()
        .text_size(px(12.))
        .child(div().size(px(12.)).flex_shrink_0().child(mark))
        .child(
            div()
                .min_w_0()
                .flex_1()
                .truncate()
                .text_color(content(label_alpha))
                .child(label.into()),
        )
        .when_some(note, |d, note| {
            d.child(
                div()
                    .flex_shrink_0()
                    .font_features(tabular())
                    .text_size(px(11.))
                    .text_color(color)
                    .child(note),
            )
        })
        .into_any_element()
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
    use crate::cloud_start_view::fixtures::{as_sandbox, failed, ready, starting, with_mode};
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
    fn a_failed_plain_sandbox_row_has_no_rebuild_button(cx: &mut TestAppContext) {
        let window = window(cx);
        let plain = with_mode(
            failed(2, "git failed"),
            Some(monocode_core::remote::SandboxMode::Plain),
        );
        window
            .update(cx, |view, _, cx| {
                view.set_cloud_start(Some(plain), Some("fix the bug".into()), cx)
            })
            .unwrap();
        let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
        visual.run_until_parked();
        visual.refresh().unwrap();
        assert!(visual.debug_bounds("cloud-start-retry").is_some());
        assert!(visual.debug_bounds("cloud-start-rebuild").is_none());
    }

    #[gpui::test]
    fn a_failed_sandbox_row_has_no_browser_button_and_names_the_sandbox(cx: &mut TestAppContext) {
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
                    Some(as_sandbox(failed(2, "devcontainer up failed"))),
                    Some("fix the bug".into()),
                    cx,
                )
            })
            .unwrap();
        let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
        visual.run_until_parked();
        visual.refresh().unwrap();
        assert!(visual.debug_bounds("cloud-start-browser").is_none());
        for (id, action) in [
            ("cloud-start-retry", StartAction::Retry),
            ("cloud-start-rebuild", StartAction::Rebuild),
            ("cloud-start-delete", StartAction::DeleteCodespace),
            ("cloud-start-view-log", StartAction::ShowPanel),
        ] {
            let bounds = visual.debug_bounds(id).unwrap_or_else(|| panic!("{id}"));
            visual.simulate_click(bounds.center(), Default::default());
            assert_eq!(seen.borrow().last(), Some(&action), "{id}");
        }
        assert!(visual.debug_bounds("cloud-queued-note").is_some());
        assert!(
            visual.debug_bounds("cloud-start-help").is_none(),
            "an unrecognised failure has no fix guide"
        );
        window
            .update(visual, |view, _, _| {
                let start = view.cloud_start.clone().unwrap();
                assert_eq!(start.headline(), "Sandbox failed at Build container");
                assert_eq!(
                    start.failure_message(),
                    "devcontainer up failed The sandbox is kept, so a retry reuses it."
                );
                assert_eq!(start.delete_label(), "Delete sandbox");
            })
            .unwrap();
    }

    #[gpui::test]
    fn a_codespace_without_ssh_server_says_to_add_the_feature_and_keeps_retry(
        cx: &mut TestAppContext,
    ) {
        let window = window(cx);
        let error = "SSH setup failed: error getting ssh server details: failed to start SSH server: Please check if an SSH server is installed in the container.";
        window
            .update(cx, |view, _, cx| {
                view.set_cloud_start(Some(failed(5, error)), None, cx)
            })
            .unwrap();
        let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
        visual.run_until_parked();
        visual.refresh().unwrap();
        for id in [
            "cloud-start-help",
            "cloud-start-retry",
            "cloud-start-rebuild",
            "cloud-start-delete",
        ] {
            assert!(visual.debug_bounds(id).is_some(), "{id}");
        }
        window
            .update(visual, |view, _, _| {
                let message = view.cloud_start.clone().unwrap().failure_message();
                assert!(
                    message.contains("ghcr.io/devcontainers/features/sshd:1"),
                    "{message}"
                );
            })
            .unwrap();
    }

    #[gpui::test]
    fn the_status_row_is_one_compact_header_without_steps_or_a_log_tail(cx: &mut TestAppContext) {
        let window = window(cx);
        let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
        let draw = |visual: &mut gpui::VisualTestContext| {
            visual.run_until_parked();
            visual.refresh().unwrap();
        };
        for start in [
            starting(2),
            as_sandbox(starting(2)),
            failed(2, "boom"),
            ready(1000),
        ] {
            window
                .update(visual, |view, _, cx| {
                    view.set_cloud_start(Some(start), Some("fix it".into()), cx)
                })
                .unwrap();
            draw(visual);
            assert!(visual.debug_bounds("cloud-start-headline").is_some());
            assert!(visual.debug_bounds("cloud-start-view-log").is_some());
            assert!(visual.debug_bounds("cloud-queued-row").is_some());
            for gone in [
                "cloud-start-steps",
                "cloud-start-tail",
                "cloud-start-toggle",
            ] {
                assert!(visual.debug_bounds(gone).is_none(), "{gone}");
            }
        }
    }
}
