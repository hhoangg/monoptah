//! Small pieces the workflow pages share: sections, properties, notices, the header.
use super::super::*;
use gpui::{Div, FontWeight};

pub(super) const ERROR: u32 = 0xf87171;

/// monoPanelParts' `Section`: a titled block, with an optional control at the end of the title.
pub(in crate::monos) fn section(title: impl Into<SharedString>, action: Option<AnyElement>) -> Div {
    let title: SharedString = title.into();
    div()
        .w_full()
        .min_w_0()
        .flex_shrink_0()
        .border_t_1()
        .border_color(content(0.1))
        .px_2()
        .pt_3()
        .pb_4()
        .flex()
        .flex_col()
        .gap_2()
        .child(
            div()
                .px_2()
                .flex()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .text_size(px(11.))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(content(0.4))
                        .child(title),
                )
                .children(action),
        )
}

/// monoPanelParts' `Property`: a label and its value on one row.
pub(in crate::monos) fn property(label: impl Into<SharedString>, value: impl IntoElement) -> Div {
    div()
        .w_full()
        .min_w_0()
        .flex_shrink_0()
        .flex()
        .items_start()
        .gap_2()
        .text_size(px(12.))
        .line_height(px(28.))
        .child(
            div()
                .w(px(88.))
                .flex_shrink_0()
                .truncate()
                .text_color(content(0.45))
                .child(label.into()),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_color(content(0.85))
                .child(value),
        )
}

/// monoPanelParts' `Hint`.
pub(in crate::monos) fn hint(text: impl Into<SharedString>) -> Div {
    div()
        .mt_2()
        .px_2()
        .text_size(px(11.))
        .line_height(px(16.))
        .text_color(content(0.35))
        .child(text.into())
}

/// What a refused action says, under the list or a page's header.
pub(in crate::monos) fn notice_line(
    id: &'static str,
    text: Option<&str>,
) -> Option<gpui::Stateful<Div>> {
    text.map(|text| {
        div()
            .id(id)
            .debug_selector(move || id.into())
            .role(gpui::Role::Status)
            .px_2()
            .pt_2()
            .child(hint(text.to_string()).mt_0())
    })
}

/// An error line (React's `role="alert"` paragraphs).
pub(in crate::monos) fn alert(text: impl Into<SharedString>) -> gpui::Stateful<Div> {
    div()
        .id("mono-workflow-alert")
        .role(gpui::Role::Alert)
        .px_2()
        .pt_2()
        .text_size(px(12.))
        .line_height(px(20.))
        .text_color(gpui::Hsla::from(rgb(ERROR)).opacity(0.8))
        .child(text.into())
}

/// The small label on a workflow whose commands and prompts nobody has read yet.
pub(in crate::monos) fn needs_review_badge() -> Div {
    div()
        .debug_selector(|| "mono-workflow-needs-review".into())
        .flex_shrink_0()
        .rounded_sm()
        .bg(content(0.1))
        .px_1p5()
        .text_size(px(10.))
        .line_height(px(16.))
        .font_weight(FontWeight::MEDIUM)
        .text_color(content(0.7))
        .child("Needs review")
}

/// A filled text button of the page header (Cancel, Approve, Import paused).
pub(in crate::monos) fn header_button(
    id: &'static str,
    label: &'static str,
    primary: bool,
    enabled: bool,
) -> gpui::Stateful<Div> {
    super::super::panels::button(id, label)
        .debug_selector(move || id.into())
        .flex_shrink_0()
        .h(px(24.))
        .py_0()
        .px_2()
        .when(primary, |d| d.bg(content(0.1)))
        .when(!primary, |d| d.text_color(content(0.7)))
        .when(!enabled, |d| d.opacity(0.4))
}

impl Monos {
    /// The header of a workflow page. `back` is where the back button goes; None is the overview.
    pub(in crate::monos) fn workflow_header(
        &self,
        title: &str,
        back: Option<Route>,
        cx: &mut Context<Self>,
    ) -> Div {
        div()
            .h(px(40.))
            .flex_shrink_0()
            .min_w_0()
            .overflow_hidden()
            .border_b_1()
            .border_color(content(0.1))
            .px_2()
            .flex()
            .items_center()
            .gap_2()
            .child(
                icon_button("mono-page-back", "Back", "chevron-left")
                    .debug_selector(|| "mono-page-back".into())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        // A closed review is not kept (nor probed again).
                        if matches!(this.route, Some(Route::WorkflowReview(..))) {
                            this.wf.review = None;
                        }
                        this.route = back.clone();
                        this.refresh_workflows(cx);
                        cx.notify();
                    })),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(px(13.))
                    .font_weight(FontWeight::MEDIUM)
                    .child(title.to_string()),
            )
    }
}

/// The dot of a run in progress: the Mono's colour, pulsing like React's `animate-pulse`.
pub(in crate::monos) fn running_dot(
    id: impl Into<gpui::ElementId>,
    color: u32,
    side: f32,
) -> AnyElement {
    crate::pulse::looping(
        id,
        Duration::from_secs(2),
        crate::pulse::Rate::Half,
        move |phase| {
            let wave = 0.5 + 0.5 * (phase * std::f32::consts::TAU).cos();
            div()
                .size(px(side))
                .flex_shrink_0()
                .rounded_full()
                .bg(gpui::Hsla::from(rgb(color)).opacity(0.5 + 0.5 * wave))
        },
    )
    .into_any_element()
}
