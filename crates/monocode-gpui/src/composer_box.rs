//! The Mono chat composer's box, shared with other single-field composers
//! (the Today note box): painted frame, field row and round send button.
use crate::theme::*;
use gpui::{App, Div, Hsla, Role, Stateful, Window, canvas, div, prelude::*, px};

pub(crate) const COMPOSER_RADIUS: f32 = 12.;

/// The rounded frame, painted behind its children: a pill while the field is
/// one line, `COMPOSER_RADIUS` once it wraps. `border` is read at paint time
/// so it can follow focus.
pub(crate) fn frame(border: impl Fn(&mut Window, &App) -> Hsla + 'static) -> Div {
    div().relative().p(px(1.)).flex().flex_col().child(
        canvas(
            |_, _, _| {},
            move |bounds, _, window, cx| {
                // One line is 18px plus 8px field padding, 10px row
                // padding and 2px border. Decide after text wraps.
                let radius = if bounds.size.height <= px(38.) {
                    bounds.size.height / 2.
                } else {
                    px(COMPOSER_RADIUS)
                };
                let border = border(window, cx);
                window.paint_quad(
                    gpui::fill(bounds, content(0.03))
                        .corner_radii(radius)
                        .border_widths(px(1.))
                        .border_color(border),
                );
            },
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full(),
    )
}

/// The field beside the send button. Even padding centres send beside one
/// line, and keeps it at the bottom as the text grows.
pub(crate) fn field_row(field: impl IntoElement, send: impl IntoElement) -> Div {
    div()
        .flex()
        .items_end()
        .gap_1p5()
        .p(px(5.))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .self_center()
                .px_1p5()
                .py_1()
                .child(field),
        )
        .child(send)
}

type Shade = fn(f32) -> Hsla;

/// (surface, ink) colour makers for a button that is the inverse of the page:
/// white with black ink on dark, black with white ink on light.
pub(crate) fn inverse_ink() -> (Shade, Shade) {
    if is_light() {
        (black, white)
    } else {
        (white, black)
    }
}

/// The round send button. `id` is also its debug selector; the caller adds
/// the click handler.
pub(crate) fn send_button(id: &'static str, ready: bool) -> Stateful<Div> {
    let (solid, ink) = inverse_ink();
    div()
        .id(id)
        .debug_selector(|| id.into())
        .role(Role::Button)
        .aria_label("Send")
        .flex_shrink_0()
        .size(px(26.))
        .rounded_full()
        .flex()
        .items_center()
        .justify_center()
        .bg(solid(if ready { 1. } else { 0.3 }))
        .when(ready, |d| d.hover(|s| s.bg(solid(0.9))))
        .child(icon("mono-arrow-up", 14.).text_color(ink(if ready { 1. } else { 0.4 })))
        .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
}
