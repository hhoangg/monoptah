//! The Codespace panel: the right-panel tab that follows a cloud session while its codespace
//! starts. A meta block, the phase list with durations and the creation log, in the style of
//! the Changes pane.
use crate::{
    cloud_start_view::{CloudStartSnapshot, StartAction, StartPhase, step_ms},
    right_panel::{icon_action, pane_header, spinner, tabular},
    settings::widgets::secondary_button,
    theme::*,
};
use gpui::{
    AnyElement, Context, EventEmitter, FontWeight, Role, ScrollHandle, SharedString, Window, div,
    point, prelude::*, px, rgb,
};
use monocode_core::cloud_sessions::{StepKind, StepStatus};
use std::sync::Arc;

const GREEN: u32 = 0x4ade80;

pub(crate) enum PaneEvent {
    Action(StartAction),
    Close,
}

pub(crate) struct CodespacePane {
    start: Option<Arc<CloudStartSnapshot>>,
    scroll: ScrollHandle,
    body: ScrollHandle,
    follow: bool,
    shown_lines: usize,
    log_text: SharedString,
    clock: Option<gpui::Task<()>>,
}
impl EventEmitter<PaneEvent> for CodespacePane {}

pub(crate) fn state_word(phase: StartPhase) -> &'static str {
    match phase {
        StartPhase::Starting => "Starting",
        StartPhase::Failed => "Failed",
        StartPhase::Ready => "Ready",
    }
}

impl CodespacePane {
    pub(crate) fn new() -> Self {
        Self {
            start: None,
            scroll: ScrollHandle::new(),
            body: ScrollHandle::new(),
            follow: true,
            shown_lines: 0,
            log_text: "Waiting for the creation log...".into(),
            clock: None,
        }
    }

    #[cfg(test)]
    pub(crate) fn has_start(&self) -> bool {
        self.start.is_some()
    }

    /// Shows the start of the active session, or the empty state for `None`.
    pub(crate) fn set_start(
        &mut self,
        start: Option<Arc<CloudStartSnapshot>>,
        cx: &mut Context<Self>,
    ) {
        let same = match (&self.start, &start) {
            (None, None) => true,
            (Some(a), Some(b)) => Arc::ptr_eq(a, b),
            _ => false,
        };
        if same {
            return;
        }
        let other_session = self.start.as_ref().map(|s| s.record.session_id)
            != start.as_ref().map(|s| s.record.session_id);
        if other_session {
            self.follow = true;
            self.shown_lines = 0;
        }
        if let Some(next) = &start
            && self
                .start
                .as_ref()
                .is_none_or(|old| !Arc::ptr_eq(&old.log, &next.log))
        {
            self.log_text = if next.log.is_empty() {
                "Waiting for the creation log...".into()
            } else {
                next.log.join("\n").into()
            };
        }
        self.start = start;
        self.sync_clock(cx);
        cx.notify();
    }

    /// Running durations tick once a second while the start runs.
    fn sync_clock(&mut self, cx: &mut Context<Self>) {
        let live = self
            .start
            .as_ref()
            .is_some_and(|s| s.phase == StartPhase::Starting);
        if !live {
            self.clock = None;
        } else if self.clock.is_none() && !cfg!(test) {
            self.clock = Some(cx.spawn(async move |this, cx| {
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

    fn section(title: &'static str) -> gpui::Div {
        div()
            .pt_3()
            .pb_1()
            .px_3()
            .text_size(px(11.))
            .font_weight(FontWeight::MEDIUM)
            .text_color(content(0.45))
            .child(title)
    }

    fn meta_row(label: &'static str, value: String) -> AnyElement {
        div()
            .px_3()
            .py(px(2.))
            .flex()
            .gap_2()
            .text_size(px(12.))
            .child(
                div()
                    .w(px(64.))
                    .flex_shrink_0()
                    .text_color(content(0.45))
                    .child(label),
            )
            .child(
                div()
                    .min_w_0()
                    .flex_1()
                    .truncate()
                    .text_color(content(0.8))
                    .child(value),
            )
            .into_any_element()
    }

    fn step_row(snapshot: &CloudStartSnapshot, kind: StepKind, now: u64) -> AnyElement {
        let step = snapshot.record.step(kind);
        let duration = step_ms(step, now).map(crate::shell::live_elapsed);
        let (mark, color, label_alpha, note): (AnyElement, _, _, Option<String>) = match step.status
        {
            StepStatus::Done => (
                icon("check", 12.).text_color(rgb(GREEN)).into_any_element(),
                content(0.8),
                0.8,
                duration,
            ),
            StepStatus::Running => (
                spinner(("codespace-step-spin", kind as usize), "loader", 12.),
                content(0.45),
                1.,
                duration,
            ),
            StepStatus::Failed => (
                icon("alert-circle", 12.)
                    .text_color(danger_color())
                    .into_any_element(),
                danger_color(),
                1.,
                duration,
            ),
            StepStatus::Pending => (
                div()
                    .size(px(8.))
                    .m(px(2.))
                    .rounded_full()
                    .border_1()
                    .border_color(content(0.3))
                    .into_any_element(),
                content(0.45),
                0.45,
                None,
            ),
            StepStatus::Skipped => (
                div()
                    .size(px(8.))
                    .m(px(2.))
                    .rounded_full()
                    .border_1()
                    .border_color(content(0.15))
                    .into_any_element(),
                content(0.3),
                0.3,
                Some("skipped".into()),
            ),
        };
        let id: SharedString = format!("codespace-step-{}", kind.id()).into();
        div()
            .id(id.clone())
            .debug_selector(move || id.to_string())
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
                    .child(kind.label_for(snapshot.backend())),
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

    fn action_button(
        &self,
        id: &'static str,
        label: impl Into<SharedString>,
        action: StartAction,
        danger: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let label: SharedString = label.into();
        secondary_button(id, label.clone(), danger, false)
            .debug_selector(move || id.into())
            .child(label)
            .on_click(cx.listener(move |_, _, _, cx| cx.emit(PaneEvent::Action(action))))
            .into_any_element()
    }

    /// The pane's name: `Codespace` or `Sandbox` by the backend of the start shown.
    pub(crate) fn title(&self) -> &'static str {
        self.start.as_ref().map_or("Codespace", |s| s.title())
    }

    fn header(
        title: &'static str,
        phase: Option<StartPhase>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        pane_header("codespace-header", title)
            .child(div().child(title))
            .when_some(phase, |d, phase| {
                let color = match phase {
                    StartPhase::Starting => warning_color(),
                    StartPhase::Failed => danger_color(),
                    StartPhase::Ready => rgb(GREEN).into(),
                };
                d.child(
                    div()
                        .debug_selector(|| "codespace-state".into())
                        .text_size(px(11.))
                        .font_weight(FontWeight::NORMAL)
                        .text_color(color)
                        .child(state_word(phase)),
                )
            })
            .child(div().flex_1())
            .child(
                icon_action("codespace-close", "Close", "x", true)
                    .cursor_pointer()
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(PaneEvent::Close))),
            )
    }
}

impl Render for CodespacePane {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let header = Self::header(self.title(), self.start.as_ref().map(|s| s.phase), cx);
        let Some(start) = self.start.clone() else {
            return div()
                .id("codespace-pane")
                .size_full()
                .flex()
                .flex_col()
                .child(header)
                .child(
                    div()
                        .px_3()
                        .py_2()
                        .text_size(px(12.))
                        .text_color(content(0.5))
                        .child("This session has no codespace startup to show."),
                )
                .into_any_element();
        };
        let now = monocode_core::time::now_ms();
        let record = &start.record;
        let log_len = start.log.len();
        // Follow the tail until the user scrolls up; follow again once back at the bottom.
        let max = self.scroll.max_offset().y;
        let at_bottom = max + self.scroll.offset().y < px(4.);
        if log_len == self.shown_lines {
            self.follow = at_bottom;
        } else if self.follow {
            self.scroll.set_offset(point(px(0.), -max));
        }
        self.shown_lines = log_len;
        let meta = div()
            .flex()
            .flex_col()
            .child(Self::meta_row(
                "Name",
                record
                    .sandbox
                    .clone()
                    .unwrap_or_else(|| record.codespace.clone()),
            ))
            .when_some(record.branch.clone(), |d, branch| {
                d.child(Self::meta_row("Branch", branch))
            })
            .when_some(record.machine.clone(), |d, machine| {
                d.child(Self::meta_row("Machine", machine))
            })
            .child(Self::meta_row(
                if start.offers_browser() {
                    "GitHub"
                } else {
                    "State"
                },
                record.gh_state.clone().unwrap_or_else(|| "Unknown".into()),
            ));
        let phases = div().flex().flex_col().children(
            StepKind::ALL
                .iter()
                .map(|kind| Self::step_row(&start, *kind, now)),
        );
        let failed = start.phase == StartPhase::Failed;
        let log_text = self.log_text.clone();
        div()
            .id("codespace-pane")
            .debug_selector(|| "codespace-pane".into())
            .role(Role::Group)
            .aria_label(start.title())
            .size_full()
            .min_h_0()
            .flex()
            .flex_col()
            .child(header)
            .child(
                div()
                    .id("codespace-body")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&self.body)
                    .flex()
                    .flex_col()
                    .pb_3()
                    .child(meta)
                    .when_some(start.error().map(str::to_owned), |d, error| {
                        d.child(
                            div()
                                .id("codespace-error")
                                .debug_selector(|| "codespace-error".into())
                                .role(Role::Alert)
                                .px_3()
                                .pt_2()
                                .text_size(px(12.))
                                .line_height(px(17.))
                                .text_color(danger_color())
                                .child(error),
                        )
                    })
                    .when(failed, |d| {
                        d.child(
                            div()
                                .px_3()
                                .pt_2()
                                .flex()
                                .flex_wrap()
                                .gap_2()
                                .child(self.action_button(
                                    "codespace-retry",
                                    "Retry",
                                    StartAction::Retry,
                                    false,
                                    cx,
                                ))
                                .when(start.offers_rebuild(), |d| {
                                    d.child(self.action_button(
                                        "codespace-rebuild",
                                        "Rebuild",
                                        StartAction::Rebuild,
                                        false,
                                        cx,
                                    ))
                                })
                                .child(self.action_button(
                                    "codespace-delete",
                                    start.delete_label(),
                                    StartAction::DeleteCodespace,
                                    true,
                                    cx,
                                )),
                        )
                    })
                    .child(Self::section("Phases"))
                    .child(phases)
                    .child(
                        div()
                            .pt_3()
                            .pb_1()
                            .px_3()
                            .flex()
                            .items_center()
                            .gap_2()
                            .text_size(px(11.))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(content(0.45))
                            .child(div().flex_1().child("Creation log"))
                            .child(
                                secondary_button("codespace-copy-log", "Copy", false, false)
                                    .debug_selector(|| "codespace-copy-log".into())
                                    .child("Copy")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        if let Some(start) = &this.start {
                                            cx.write_to_clipboard(gpui::ClipboardItem::new_string(
                                                start.log.join("\n"),
                                            ));
                                        }
                                    })),
                            )
                            .when(start.offers_browser(), |d| {
                                d.child(
                                    secondary_button(
                                        "codespace-open-browser",
                                        "Browser",
                                        false,
                                        false,
                                    )
                                    .debug_selector(|| "codespace-open-browser".into())
                                    .child("Browser")
                                    .on_click(cx.listener(
                                        |_, _, _, cx| {
                                            cx.emit(PaneEvent::Action(StartAction::OpenInBrowser))
                                        },
                                    )),
                                )
                            }),
                    )
                    .child(
                        div()
                            .id("codespace-log")
                            .debug_selector(|| "codespace-log".into())
                            .mx_3()
                            .h(px(220.))
                            .flex_shrink_0()
                            .overflow_y_scroll()
                            .track_scroll(&self.scroll)
                            .rounded_md()
                            .bg(content(0.05))
                            .px_2()
                            .py(px(6.))
                            .font_family(crate::fonts::mono())
                            .text_size(px(11.))
                            .line_height(px(16.))
                            .text_color(content(0.75))
                            .child(log_text),
                    ),
            )
            .into_any_element()
    }
}
