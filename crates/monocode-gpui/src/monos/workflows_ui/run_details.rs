//! What a run did with the picked node (`RunNodeDetails.tsx`): its state, the rounds a loop
//! already finished, why it was decided the way it was, its output and, for a command, how it
//! exited. The text rules are in `run_model::details_of`; this draws them.
use super::super::scheduler::{duration, when};
use super::super::*;
use super::run_model::{CommandBlock, Details, OutputLine, RoundLine, Took};
use super::view::property;
use gpui::{Div, FontWeight};
use monocode_core::mono::workflows::run_graph_model::RunNodeStatus;
use monocode_core::time::now_ms;

fn label(text: &'static str) -> Div {
    div()
        .px_2()
        .pb_1()
        .text_size(px(11.))
        .font_weight(FontWeight::MEDIUM)
        .text_color(content(0.4))
        .child(text)
}

fn error_text(text: &str) -> Div {
    div()
        .text_size(px(12.))
        .line_height(px(20.))
        .text_color(gpui::Hsla::from(rgb(0xef4444)).opacity(0.8))
        .child(text.to_string())
}

/// The dot and words that say how a node stands.
fn status_label(status: RunNodeStatus, text: &str, color: u32, id: &str) -> Div {
    let dot: AnyElement = match status {
        RunNodeStatus::Running => super::view::running_dot(
            SharedString::from(format!("wf-details-pulse-{id}")),
            color,
            8.,
        ),
        RunNodeStatus::Pending | RunNodeStatus::Skipped => div()
            .size(px(8.))
            .flex_shrink_0()
            .rounded_full()
            .border_1()
            .border_color(content(0.3))
            .into_any_element(),
        other => div()
            .size(px(8.))
            .flex_shrink_0()
            .rounded_full()
            .bg(match other {
                RunNodeStatus::Done => gpui::Hsla::from(rgb(0x34d399)).opacity(0.8),
                RunNodeStatus::Waiting => gpui::Hsla::from(rgb(0xfbbf24)).opacity(0.8),
                RunNodeStatus::Failed => gpui::Hsla::from(rgb(0xef4444)).opacity(0.7),
                _ => content(0.4),
            })
            .into_any_element(),
    };
    div()
        .flex()
        .items_center()
        .gap(px(8.))
        .child(dot)
        .child(text.to_string())
}

impl Monos {
    fn session_button(
        &self,
        id: &str,
        selector: String,
        color: u32,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let session = id.to_string();
        div()
            .id(SharedString::from(selector.clone()))
            .debug_selector(move || selector.clone())
            .role(Role::Button)
            .aria_label("Open session")
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap_0p5()
            .rounded_md()
            .px_1p5()
            .py_0p5()
            .text_size(px(12.))
            .line_height(px(20.))
            .text_color(rgb(color))
            .cursor_pointer()
            .hover(|s| s.bg(content(0.05)))
            .child("Open session")
            .child(icon("chevron-right", 12.).text_color(rgb(color)))
            .on_click(cx.listener(move |this, _, _, cx| this.open_run_session(&session, cx)))
            .into_any_element()
    }

    fn round_row(&self, round: &RoundLine, color: u32, cx: &mut Context<Self>) -> AnyElement {
        let mut line = String::new();
        line.push_str(&format!("Round {}", round.round));
        if let Some(span) = &round.span {
            line.push_str(&format!(" \u{b7} {span}"));
        }
        if let Some(started) = round.started {
            line.push_str(&format!(" \u{b7} {}", when(started, now_ms())));
        }
        div()
            .debug_selector({
                let n = round.round;
                move || format!("wf-round-record-{n}")
            })
            .flex()
            .flex_col()
            .gap_0p5()
            .rounded_md()
            .px_2()
            .py_1p5()
            .text_size(px(12.))
            .line_height(px(20.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .text_color(content(0.85))
                    .child(status_label(
                        round.status,
                        super::run_model::status_label(round.status),
                        color,
                        &format!("round-{}", round.round),
                    ))
                    .child(div().ml_1().text_color(content(0.45)).child(line))
                    .children(round.session.as_ref().map(|session| {
                        div().ml_auto().child(self.session_button(
                            session,
                            format!("wf-open-session-round-{}", round.round),
                            color,
                            cx,
                        ))
                    })),
            )
            .children(
                round
                    .decision
                    .as_ref()
                    .map(|d| div().text_color(content(0.7)).child(d.clone())),
            )
            .children(round.error.as_ref().map(|e| error_text(e)))
            .into_any_element()
    }

    fn output_row(&self, key: &str, line: &OutputLine, color: u32, cx: &mut Context<Self>) -> Div {
        let slot = format!("{key}:{}", line.name);
        let all = self.wf.run.show_all.contains(&slot);
        let name = line.name.clone();
        property(
            line.name.clone(),
            div()
                .min_w_0()
                .py(px(6.))
                .line_height(px(16.))
                .child(line.shown(all))
                .when(line.long, |d| {
                    let selector = format!("wf-show-more-{name}");
                    d.child(
                        div()
                            .id(SharedString::from(selector.clone()))
                            .debug_selector(move || selector.clone())
                            .role(Role::Button)
                            .ml_1p5()
                            .text_size(px(11.))
                            .text_color(rgb(color))
                            .cursor_pointer()
                            .hover(|s| s.opacity(0.8))
                            .child(if all { "Show less" } else { "Show more" })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                let page = &mut this.wf.run;
                                if !page.show_all.remove(&slot) {
                                    page.show_all.insert(slot.clone());
                                }
                                cx.notify();
                            })),
                    )
                }),
        )
        .line_height(px(16.))
    }

    fn tail_box(&self, title: &'static str, text: &str, selector: &'static str) -> Div {
        div()
            .px_2()
            .pb_1p5()
            .child(
                div()
                    .pb_1()
                    .text_size(px(11.))
                    .line_height(px(16.))
                    .text_color(content(0.45))
                    .child(title),
            )
            .child(
                div()
                    .id(selector)
                    .debug_selector(move || selector.into())
                    .max_h(px(96.))
                    .overflow_y_scroll()
                    .rounded_md()
                    .bg(content(0.05))
                    .px_2()
                    .py(px(6.))
                    .font_family(crate::markdown::MONO)
                    .text_size(px(11.))
                    .line_height(px(16.))
                    .text_color(content(0.75))
                    .child(text.to_string()),
            )
    }

    fn command_block(&self, command: &CommandBlock) -> Div {
        div()
            .pt_1()
            .children(command.exit_code.as_ref().map(|code| {
                div().px_2().child(property(
                    "Exit code",
                    div()
                        .debug_selector(|| "wf-exit-code".into())
                        .child(code.clone()),
                ))
            }))
            .children(
                command
                    .stdout
                    .as_ref()
                    .map(|t| self.tail_box("Output", t, "wf-stdout")),
            )
            .children(
                command
                    .stderr
                    .as_ref()
                    .map(|t| self.tail_box("Errors", t, "wf-stderr")),
            )
    }

    fn details_body(&mut self, d: &Details, key: &str, color: u32, cx: &mut Context<Self>) -> Div {
        let now = now_ms();
        let mut facts = div().px_2().flex().flex_col();
        facts = facts.child(property("Type", d.kind));
        facts = facts.child(property(
            "Status",
            div()
                .flex()
                .items_center()
                .child(status_label(d.status, d.status_label, color, "node"))
                .children(d.round.map(|r| {
                    div()
                        .ml_1()
                        .text_color(content(0.45))
                        .child(format!("round {r}"))
                })),
        ));
        if let Some(started) = d.started {
            facts = facts.child(property("Started", when(started, now)));
        }
        match d.took {
            Took::None => {}
            Took::Spent(ms) => facts = facts.child(property("Took", duration(ms))),
            Took::Since(since) => {
                facts = facts.child(property("Took", duration(now.saturating_sub(since))))
            }
            Took::Unknown => facts = facts.child(property("Took", "-")),
        }
        if let Some(decision) = &d.decision {
            facts = facts.child(
                property(
                    "Decision",
                    div()
                        .py(px(6.))
                        .line_height(px(16.))
                        .child(decision.clone()),
                )
                .line_height(px(16.)),
            );
        }
        if let Some(session) = &d.session {
            facts = facts.child(property(
                "Session",
                self.session_button(session, "wf-open-session".into(), color, cx),
            ));
        }
        let rows: Vec<AnyElement> = d
            .rounds
            .iter()
            .map(|r| self.round_row(r, color, cx))
            .collect();
        div()
            .child(facts)
            .children(d.error.as_ref().map(|e| error_text(e).px_4().pt_1().pb_2()))
            .when(!rows.is_empty(), |el| {
                el.child(
                    div()
                        .pt_1()
                        .child(label("ROUNDS"))
                        .child(div().flex().flex_col().children(rows)),
                )
            })
            .children(d.command.as_ref().map(|c| self.command_block(c)))
            .when(!d.outputs.is_empty(), |el| {
                let rows: Vec<Div> = d
                    .outputs
                    .iter()
                    .map(|o| self.output_row(key, o, color, cx))
                    .collect();
                el.child(
                    div()
                        .pt_1()
                        .child(label("OUTPUT"))
                        .child(div().px_2().flex().flex_col().children(rows)),
                )
            })
    }

    /// The panel under the graph: collapsible, scrolling on its own.
    pub(super) fn run_details(
        &mut self,
        window: &Window,
        color: u32,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let open = !self.wf.run.details_closed;
        let details = self.wf.run.details.clone();
        let key = self.wf.run.selection.clone();
        let body = match (&details, &key) {
            (Some(d), Some(key)) if open => Some(self.details_body(d, key, color, cx)),
            _ => None,
        };
        div()
            .id("wf-details")
            .debug_selector(|| "wf-details".into())
            .flex_shrink_0()
            .border_t_1()
            .border_color(content(0.1))
            .child(
                div()
                    .id("wf-details-toggle")
                    .debug_selector(|| "wf-details-toggle".into())
                    .role(Role::Button)
                    .aria_expanded(open)
                    .w_full()
                    .flex()
                    .items_center()
                    .gap_1p5()
                    .px_4()
                    .py_2()
                    .cursor_pointer()
                    .child(
                        icon(
                            if open {
                                "chevron-down"
                            } else {
                                "chevron-right"
                            },
                            12.,
                        )
                        .text_color(content(0.4)),
                    )
                    .child(
                        div()
                            .text_size(px(11.))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(content(0.4))
                            .child("DETAILS"),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(px(12.))
                            .text_color(content(0.85))
                            .child(
                                details
                                    .as_ref()
                                    .map(|d| d.heading.clone())
                                    .unwrap_or_default(),
                            ),
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.wf.run.details_closed = !this.wf.run.details_closed;
                        cx.notify();
                    })),
            )
            .when(open, |d| {
                d.child(
                    div()
                        .id("wf-details-body")
                        .debug_selector(|| "wf-details-body".into())
                        .max_h(window.viewport_size().height * 0.3)
                        .overflow_y_scroll()
                        .px_2()
                        .pb_3()
                        .map(|el| match body {
                            Some(body) => el.child(body),
                            None => el.child(
                                div()
                                    .px_4()
                                    .py_2()
                                    .text_size(px(12.))
                                    .text_color(content(0.45))
                                    .child("Pick a node to see what it did."),
                            ),
                        }),
                )
            })
            .into_any_element()
    }
}
