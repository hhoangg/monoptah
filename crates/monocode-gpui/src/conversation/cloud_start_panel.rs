//! The start panel above the composer while a cloud session starts: the same card as the cloud
//! setup panel and the background-tasks shelf. The header shows a spinner, the title ("Starting
//! sandbox"), the running phase, the elapsed time and a collapse chevron; the body is the newest
//! log line. It reads the snapshot the transcript row and the right panel already use. Hidden
//! once ready; on failure the header turns to the error tone and the actions stay in the chat.
use super::cloud_panel::Tip;
use super::*;
use crate::{
    cloud_start_view::StartPhase,
    right_panel::{spinner, tabular},
};

impl Conversation {
    /// Repaints once a second while the start runs, so the elapsed time ticks.
    pub(super) fn sync_cloud_start_clock(&mut self, cx: &mut Context<Self>) {
        let live = self
            .cloud_start
            .as_ref()
            .is_some_and(|start| start.phase == StartPhase::Starting);
        if !live {
            self.cloud_start_clock = None;
        } else if self.cloud_start_clock.is_none() && !cfg!(test) {
            self.cloud_start_clock = Some(cx.spawn(async move |this, cx| {
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

    /// The panel above the composer while a cloud start runs or failed; empty otherwise.
    pub(super) fn cloud_start_panel(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(start) = self.cloud_start.as_ref() else {
            return div().into_any_element();
        };
        let Some(title) = start.panel_title() else {
            return div().into_any_element();
        };
        let failed = start.phase == StartPhase::Failed;
        let collapsed = self.cloud_start_collapsed;
        let now = monocode_core::time::now_ms();
        let elapsed = crate::shell::live_elapsed(start.record.elapsed_ms(now));
        let tone = if failed {
            danger_color()
        } else {
            warning_color()
        };
        let mark: AnyElement = if failed {
            icon("x", 14.).text_color(tone).into_any_element()
        } else {
            div()
                .text_color(tone)
                .child(spinner("cloud-start-panel-spin", "loader", 14.))
                .into_any_element()
        };
        let subtitle = start.panel_subtitle();
        let header = div()
            .id("cloud-start-panel-header")
            .debug_selector(|| "cloud-start-panel-header".into())
            .role(Role::Button)
            .aria_label(title.clone())
            .aria_expanded(!collapsed)
            .h(px(30.))
            .flex()
            .items_center()
            .gap(px(8.))
            .cursor_pointer()
            .on_click(cx.listener(|this, _, _, cx| {
                this.cloud_start_collapsed = !this.cloud_start_collapsed;
                cx.notify();
            }))
            .child(div().size(px(16.)).flex_shrink_0().child(mark))
            .child(
                div()
                    .id("cloud-start-panel-title")
                    .debug_selector(|| "cloud-start-panel-title".into())
                    .flex_shrink_0()
                    .text_color(if failed { tone } else { content(0.8) })
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .child(title.clone()),
            )
            .child(
                div()
                    .id("cloud-start-panel-phase")
                    .debug_selector(|| "cloud-start-panel-phase".into())
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_color(content(0.35))
                    .children(subtitle),
            )
            .child(
                div()
                    .flex_shrink_0()
                    .font_features(tabular())
                    .text_color(content(0.35))
                    .child(elapsed),
            )
            .child(
                div()
                    .size(px(20.))
                    .flex_shrink_0()
                    .rounded_md()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(content(0.45))
                    .hover(|s| s.bg(content(0.08)).text_color(content(0.9)))
                    .child(icon(
                        if collapsed {
                            "chevron-up"
                        } else {
                            "chevron-down-2"
                        },
                        12.,
                    )),
            );
        let line = start.latest_log().map(str::to_owned);
        let card = composer_shelf::card("cloud-start-panel", self.classic_layout)
            .debug_selector(|| "cloud-start-panel".into())
            .role(Role::Group)
            .aria_label(title)
            .child(header)
            .when_some(line.filter(|_| !collapsed), |d, line| {
                let tip: SharedString = line.clone().into();
                d.child(
                    div()
                        .id("cloud-start-panel-log")
                        .debug_selector(|| "cloud-start-panel-log".into())
                        .min_h(px(28.))
                        .pb(px(6.))
                        .truncate()
                        .font_family(crate::fonts::mono())
                        .text_size(px(11.5))
                        .line_height(px(18.))
                        .text_color(content(0.4))
                        .tooltip(move |_, cx| cx.new(|_| Tip(tip.clone())).into())
                        .child(line),
                )
            });
        composer_shelf::shelf("cloud-start-shelf")
            .px(px(8.))
            .child(card)
            .into_any_element()
    }
}
