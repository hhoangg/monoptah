//! Small building blocks the Cloud settings dialog and the Git credential panel share: hints,
//! alerts, labelled fields and one-of-many rows. They carry no state of their own.
use crate::{cloud_mark::CloudTip, input::Input, settings::widgets::secondary_button, theme::*};
use gpui::{
    Div, Entity, FocusHandle, FontWeight, Role, SharedString, Stateful, div, prelude::*, px, rgb,
};

/// Where Docker shows the credit and compute usage of Sandboxes. `sbx` has no command that
/// reads the balance, so the Console page is the only place; the app opens it in the browser.
pub(crate) const DOCKER_USAGE_URL: &str = "https://agentic-platform.docker.com/usage";

/// The hover text of [`docker_usage_link`]. The Console page shows the Docker account that is
/// signed in in the browser, which is not necessarily the account a project runs `sbx` as.
pub(crate) const DOCKER_USAGE_TIP: &str = "Opens the Docker Console usage page for the account \
     signed in in your browser, which may not be this project's Docker account. Credits and \
     compute usage may lag by up to 48 hours.";

/// The visible text of [`docker_usage_link`].
pub(crate) const DOCKER_USAGE_TEXT: &str = "View usage & credits \u{2197}";

/// A link that opens `url` in the default browser. It is reachable with the keyboard: it takes
/// part in the tab order through `focus` and opens on Enter or Space as well as on a click. The
/// caller adds the content and any size. `id` is also its debug selector.
pub(crate) fn external_link(
    id: &'static str,
    aria_label: &'static str,
    url: &'static str,
    focus: &FocusHandle,
) -> Stateful<Div> {
    div()
        .id(id)
        .debug_selector(move || id.into())
        .role(Role::Link)
        .aria_label(aria_label)
        .track_focus(focus)
        .tab_index(0)
        .text_color(content(0.7))
        .underline()
        .cursor_pointer()
        .hover(|s| s.text_color(content(1.)))
        .on_click(move |_, _, cx| cx.open_url(url))
        .on_key_down(move |event, _, cx| {
            if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                cx.open_url(url);
                cx.stop_propagation();
            }
        })
}

/// The link to the Docker usage page. Only Docker Sandbox surfaces use it: Codespaces are billed
/// by GitHub.
pub(crate) fn docker_usage_link(id: &'static str, focus: &FocusHandle) -> Stateful<Div> {
    external_link(id, "View Docker usage and credits", DOCKER_USAGE_URL, focus)
        .text_size(px(12.))
        .tooltip(|_, cx| cx.new(|_| CloudTip(DOCKER_USAGE_TIP.into())).into())
        .child(DOCKER_USAGE_TEXT)
}

pub(crate) fn muted(text: impl Into<SharedString>) -> Div {
    div()
        .text_size(px(11.))
        .line_height(px(17.))
        .text_color(content(0.45))
        .child(text.into())
}

pub(crate) fn alert(id: impl Into<SharedString>, text: impl Into<SharedString>) -> Stateful<Div> {
    let id = id.into();
    div()
        .id(id.clone())
        .debug_selector(move || id.to_string())
        .role(Role::Alert)
        .text_size(px(12.))
        .line_height(px(18.))
        .text_color(rgb(0xf87171))
        .child(text.into())
}

pub(crate) fn code_box(text: &str) -> Div {
    div()
        .min_w_0()
        .rounded_md()
        .border_1()
        .border_color(content(0.1))
        .px(px(8.))
        .py(px(4.))
        .text_size(px(12.))
        .line_height(px(18.))
        .child(text.to_owned())
}

/// A bordered secondary button whose label is visible and whose id is a debug selector.
pub(crate) fn button(
    id: &'static str,
    label: impl Into<SharedString>,
    disabled: bool,
) -> Stateful<Div> {
    let label = label.into();
    secondary_button(id, label.clone(), false, disabled)
        .child(label)
        .debug_selector(move || id.into())
}

/// A label above its control.
pub(crate) fn labelled(label: &'static str) -> Div {
    div().flex().flex_col().gap(px(6.)).child(
        div()
            .text_size(px(12.))
            .font_weight(FontWeight::MEDIUM)
            .child(label),
    )
}

/// The framed one-line box around an [`Input`].
pub(crate) fn field_box(id: impl Into<SharedString>, input: &Entity<Input>) -> Stateful<Div> {
    let id = id.into();
    div()
        .id(id.clone())
        .debug_selector(move || id.to_string())
        .h(px(36.))
        .min_w_0()
        .rounded_lg()
        .border_1()
        .border_color(content(0.15))
        .bg(content(0.03))
        .px_3()
        .flex()
        .items_center()
        .text_color(content(1.))
        .child(input.clone())
}

/// One row of a pick-one list. The caller adds `on_click`.
pub(crate) fn choice(
    id: impl Into<SharedString>,
    label: impl Into<SharedString>,
    selected: bool,
) -> Stateful<Div> {
    let id = id.into();
    let label = label.into();
    div()
        .id(id.clone())
        .debug_selector(move || id.to_string())
        .role(Role::RadioButton)
        .aria_label(label.clone())
        .aria_toggled(if selected {
            gpui::Toggled::True
        } else {
            gpui::Toggled::False
        })
        .flex()
        .items_center()
        .gap_2()
        .h(px(30.))
        .px_2()
        .rounded_md()
        .cursor_pointer()
        .text_size(px(12.))
        .when(selected, |d| d.bg(content(0.08)))
        .hover(|s| s.bg(content(0.06)))
        .child(
            div()
                .w(px(14.))
                .flex_shrink_0()
                .text_color(accent_color())
                .child(if selected { "✓" } else { "" }),
        )
        .child(div().min_w_0().truncate().child(label))
}

/// The look of a dialog title and its muted subtitle line.
pub(crate) fn heading(title: &'static str, subtitle: String) -> Div {
    div()
        .min_w_0()
        .flex_1()
        .pt(px(2.))
        .flex()
        .flex_col()
        .child(
            div()
                .text_size(px(16.))
                .line_height(px(20.))
                .font_weight(FontWeight::SEMIBOLD)
                .child(title),
        )
        .child(
            div()
                .mt(px(2.))
                .min_w_0()
                .truncate()
                .text_size(px(12.))
                .line_height(px(16.5))
                .text_color(content(0.5))
                .child(subtitle),
        )
}
