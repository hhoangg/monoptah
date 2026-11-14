//! The standard modal: a light scrim that fades in, a frosted 16px card that
//! rises into place, and the dialog buttons every modal shares. Light themes
//! paint the card opaque. `window.prompt` renders through [`prompt_builder`],
//! so confirmations use the same card as every other modal.
use crate::theme::*;
use gpui::{
    Animation, AnimationElement, AnimationExt, App, Context, Div, ElementId, EventEmitter,
    FocusHandle, Focusable, FontWeight, Hsla, KeyDownEvent, MouseButton, PromptButton,
    PromptHandle, PromptLevel, PromptResponse, RenderablePromptHandle, Role, SharedString,
    Stateful, Window, canvas, div, prelude::*, px,
};
use std::time::Duration;

pub(crate) const RADIUS: f32 = 16.;
/// Backdrop blur behind the card, in points.
#[cfg(any(target_os = "macos", target_os = "windows"))]
const BLUR: f32 = 24.;
const ENTER: Duration = Duration::from_millis(200);
const RISE: f32 = 6.;
/// The card's rule, the same 10% hairline as popovers.
pub(crate) const BORDER: f32 = 0.1;

fn ease_out(t: f32) -> f32 {
    1. - (1. - t).powi(3)
}

/// Dims the window just enough to separate the card while the content
/// behind keeps its colour through the blur.
pub(crate) fn scrim_color() -> Hsla {
    black(if is_light() { 0.18 } else { 0.35 })
}

/// Fades a full-window scrim in. The caller sizes and positions `scrim`
/// (usually `absolute().inset_0()`), and wires dismissal.
pub(crate) fn backdrop<E: Styled + IntoElement + 'static>(
    id: impl Into<ElementId>,
    scrim: E,
) -> AnimationElement<E> {
    scrim.with_animation(id, Animation::new(ENTER).with_easing(ease_out), |el, t| {
        let mut color = scrim_color();
        color.a *= t;
        el.bg(color)
    })
}

/// A full-window scrim layer, for a modal parent that is already
/// `absolute().inset_0()`. Add it before the card so the card paints above.
pub(crate) fn scrim(id: impl Into<ElementId>) -> AnimationElement<Div> {
    backdrop(id, div().absolute().inset_0())
}

/// Where the blocking progress dialogs (continue locally, archive a cloud session) draw:
/// above quick open, search, prompts and the lightboxes (80 to 100), below toasts (110) and
/// the quit notice (120).
pub(crate) const BLOCKING_PRIORITY: usize = 105;

/// A full-window scrim that blocks the window behind a dialog drawn at
/// `priority`. It occludes every hitbox behind it, so the window gets no
/// hover, tooltip, cursor, click, scroll, drag/drop or context menu. Presses on
/// it are left to the dialog's own dismissal (a popover's `on_dismiss` is a
/// window-level capture listener, so it still sees them). It is deferred just
/// below `priority`, so the dialog card paints and hit-tests above it.
pub(crate) fn barrier(
    id: impl Into<ElementId>,
    priority: usize,
    window: &Window,
) -> impl IntoElement {
    let size = window.viewport_size();
    gpui::deferred(
        gpui::anchored()
            .position(gpui::point(px(0.), px(0.)))
            .child(backdrop(id, div().w(size.width).h(size.height).occlude())),
    )
    .with_priority(priority.saturating_sub(1))
}

/// Lifts a modal overlay out of the pane that renders it, so its scrim covers
/// the whole window and a flex-centred card centres on the window rather than
/// the pane. Popovers opened inside it (priority 40 and up) still draw above.
pub(crate) fn window_overlay(window: &Window, overlay: impl IntoElement) -> impl IntoElement {
    let size = window.viewport_size();
    gpui::deferred(
        gpui::anchored()
            .position(gpui::point(px(0.), px(0.)))
            .child(div().relative().w(size.width).h(size.height).child(overlay)),
    )
    .with_priority(30)
}

/// Fades a card in while it rises a few points into place.
pub(crate) fn enter<E: Styled + IntoElement + 'static>(
    id: impl Into<ElementId>,
    card: E,
) -> AnimationElement<E> {
    card.with_animation(id, Animation::new(ENTER).with_easing(ease_out), |el, t| {
        el.relative().opacity(t).top(px(RISE * (1. - t)))
    })
}

/// The frosted (or, in light themes, opaque) fill painted behind a card's
/// content. Place it first inside a `relative()` card with matching corners.
pub(crate) fn surface(radius: f32) -> impl IntoElement {
    let light = is_light();
    let mut tint = pane_fill();
    tint.a = 0.55;
    let opaque = floating_fill();
    canvas(
        |_, _, _| {},
        move |bounds, _, window, _| {
            #[cfg(any(target_os = "macos", target_os = "windows"))]
            let quad = if light {
                gpui::fill(bounds, opaque)
            } else {
                gpui::fill(bounds, tint)
                    .border_color(gpui::backdrop_blur_marker(BLUR * window.scale_factor()))
            };
            #[cfg(not(any(target_os = "macos", target_os = "windows")))]
            let quad = {
                let _ = (light, tint);
                gpui::fill(bounds, opaque)
            };
            window.paint_quad(quad.corner_radii(px(radius)));
        },
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full()
}

/// A dialog card: frosted surface, hairline rule, soft shadow and `p-5`.
pub(crate) fn card() -> Div {
    div()
        .relative()
        .w(px(360.))
        .rounded(px(RADIUS))
        .border_1()
        .border_color(content(BORDER))
        .shadow(vec![
            gpui::BoxShadow::new(px(0.), px(24.), black(if is_light() { 0.12 } else { 0.3 }))
                .blur_radius(px(48.))
                .spread_radius(px(-12.)),
        ])
        .child(surface(RADIUS - 1.))
        .text_color(content(1.))
}
/// The padded column inside a [`card`].
pub(crate) fn card_body() -> Div {
    div().relative().p(px(20.)).flex().flex_col()
}
pub(crate) fn title(text: impl Into<SharedString>) -> Div {
    div()
        .text_size(px(15.))
        .line_height(px(20.))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(content(1.))
        .child(text.into())
}
pub(crate) fn body(text: impl Into<SharedString>) -> Div {
    div()
        .text_size(px(13.))
        .line_height(px(19.))
        .text_color(content(0.6))
        .child(text.into())
}
/// The right-aligned button row closing a dialog.
pub(crate) fn actions() -> Div {
    div()
        .mt(px(20.))
        .flex()
        .justify_end()
        .items_center()
        .gap(px(8.))
}

fn button(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Stateful<Div> {
    let label = label.into();
    div()
        .id(id)
        .role(Role::Button)
        .aria_label(label.clone())
        .flex()
        .flex_shrink_0()
        .items_center()
        .justify_center()
        .h(px(32.))
        .px(px(14.))
        .rounded(px(9.))
        .text_size(px(13.))
        .whitespace_nowrap()
        .child(label)
}
/// Quiet text that gains a soft wash on hover, for Cancel and the like.
pub(crate) fn ghost_button(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
) -> Stateful<Div> {
    button(id, label)
        .text_color(content(0.6))
        .hover(|s| s.bg(content(0.07)).text_color(content(1.)))
}
/// A quiet destructive action, like Delete beside Cancel.
pub(crate) fn danger_ghost_button(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
) -> Stateful<Div> {
    button(id, label)
        .text_color(danger(1.))
        .hover(|s| s.bg(danger(0.1)))
}
/// The filled confirming action: content ink with the inverse text.
pub(crate) fn primary_button(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
) -> Stateful<Div> {
    let mut ink = floating_fill();
    ink.a = 1.;
    button(id, label)
        .bg(content(0.92))
        .text_color(ink)
        .font_weight(FontWeight::MEDIUM)
        .hover(|s| s.bg(content(0.8)))
}
/// A destructive confirmation.
pub(crate) fn danger_button(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
) -> Stateful<Div> {
    button(id, label)
        .bg(danger(1.))
        .text_color(gpui::white())
        .font_weight(FontWeight::MEDIUM)
        .hover(|s| s.bg(danger(0.85)))
}
/// What the marker of an [`option_row`] looks like.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Marker {
    /// A square box with a tick: any number of rows can be on.
    Check,
    /// A round dot: one row of a group is on.
    Radio,
}

/// The shared row of a checkbox or a radio choice in a dialog: the marker, the label and muted
/// notes under it. The caller adds `on_click` when the row is enabled; a disabled row is dimmed
/// and shows no pointer.
fn option_row(
    id: &'static str,
    marker: Marker,
    on: bool,
    disabled: bool,
    label: SharedString,
    notes: Vec<String>,
) -> Stateful<Div> {
    let note = |text: String| {
        div()
            .mt(px(4.))
            .text_size(px(11.))
            .line_height(px(15.))
            .text_color(content(0.45))
            .child(text)
    };
    let mark = div()
        .mt(px(2.))
        .size(px(14.))
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center()
        .border_1()
        .border_color(if on { accent_color() } else { content(0.3) });
    let mark = match marker {
        Marker::Check => mark.rounded(px(4.)).when(on, |d| {
            d.bg(accent_color())
                .child(icon("check", 10.).text_color(gpui::white()))
        }),
        Marker::Radio => mark.rounded_full().when(on, |d| {
            d.child(div().size(px(6.)).rounded_full().bg(accent_color()))
        }),
    };
    div()
        .id(id)
        .debug_selector(move || id.into())
        .role(match marker {
            Marker::Check => Role::CheckBox,
            Marker::Radio => Role::RadioButton,
        })
        .aria_label(label.clone())
        .aria_toggled(if on {
            gpui::Toggled::True
        } else {
            gpui::Toggled::False
        })
        .when(disabled, |d| d.opacity(0.5))
        .when(!disabled, |d| d.cursor_pointer())
        .flex()
        .items_start()
        .gap_2()
        .child(mark)
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_size(px(12.))
                .line_height(px(17.))
                .child(label)
                .children(notes.into_iter().map(note)),
        )
}

/// A checkbox row of a dialog (see [`option_row`]).
pub(crate) fn check_row(
    id: &'static str,
    checked: bool,
    disabled: bool,
    label: impl Into<SharedString>,
    notes: Vec<String>,
) -> Stateful<Div> {
    option_row(id, Marker::Check, checked, disabled, label.into(), notes)
}

/// One choice of a pick-one group in a dialog (see [`option_row`]).
pub(crate) fn radio_row(
    id: &'static str,
    selected: bool,
    disabled: bool,
    label: impl Into<SharedString>,
    notes: Vec<String>,
) -> Stateful<Div> {
    option_row(id, Marker::Radio, selected, disabled, label.into(), notes)
}
/// Disables a dialog button without changing its size.
pub(crate) fn disabled(button: Stateful<Div>, disabled: bool) -> Stateful<Div> {
    if disabled {
        button.opacity(0.4)
    } else {
        button.cursor_pointer()
    }
}
fn danger(alpha: f32) -> Hsla {
    gpui::hsla(0., 0.66, if is_light() { 0.5 } else { 0.47 }, alpha)
}

/// Lays `card` over the window behind a fading scrim. A press on the scrim
/// calls `on_dismiss`; the card itself swallows presses.
pub(crate) fn overlay(
    id: &'static str,
    card: impl IntoElement,
    on_dismiss: impl Fn(&mut Window, &mut App) + 'static,
) -> impl IntoElement {
    backdrop(
        SharedString::from(format!("{id}-scrim")),
        div()
            .id(SharedString::from(format!("{id}-overlay")))
            .absolute()
            .inset_0()
            .occlude()
            .flex()
            .items_center()
            .justify_center()
            .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                on_dismiss(window, cx)
            })
            .child(enter(
                SharedString::from(format!("{id}-card")),
                div()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(card),
            )),
    )
}

/// `cx.set_prompt_builder` target: renders `window.prompt` as a standard
/// modal rather than a native sheet.
pub(crate) fn prompt_builder(
    level: PromptLevel,
    message: &str,
    detail: Option<&str>,
    actions: &[PromptButton],
    handle: PromptHandle,
    window: &mut Window,
    cx: &mut App,
) -> RenderablePromptHandle {
    let view = cx.new(|cx| Prompt {
        level,
        message: message.to_owned().into(),
        detail: detail.map(|d| d.to_owned().into()),
        actions: actions.to_vec(),
        focus: cx.focus_handle(),
    });
    handle.with_view(view, window, cx)
}

pub(crate) struct Prompt {
    level: PromptLevel,
    message: SharedString,
    detail: Option<SharedString>,
    actions: Vec<PromptButton>,
    focus: FocusHandle,
}
impl EventEmitter<PromptResponse> for Prompt {}
impl Focusable for Prompt {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}
impl Prompt {
    /// Escape and the scrim choose Cancel, or the last answer like a sheet.
    fn cancel(&self) -> usize {
        self.actions
            .iter()
            .position(PromptButton::is_cancel)
            .unwrap_or(self.actions.len().saturating_sub(1))
    }
    /// Return chooses the first non-cancel answer, as AppKit's default button.
    fn default(&self) -> Option<usize> {
        self.actions.iter().position(|a| !a.is_cancel())
    }
    fn destructive(&self, label: &str) -> bool {
        self.level != PromptLevel::Info
            && [
                "delete", "remove", "discard", "revoke", "reset", "stop", "quit", "close",
            ]
            .iter()
            .any(|word| label.to_lowercase().starts_with(word))
    }
    fn key(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let choice = match event.keystroke.key.as_str() {
            "escape" => Some(self.cancel()),
            "enter" => self.default(),
            _ => None,
        };
        if let Some(ix) = choice {
            cx.emit(PromptResponse(ix));
            cx.stop_propagation();
        }
    }
}
impl Render for Prompt {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let default = self.default();
        // Cancel sits on the left; the default answer is the rightmost.
        let mut order: Vec<usize> = (0..self.actions.len()).collect();
        order.sort_by_key(|ix| (!self.actions[*ix].is_cancel(), std::cmp::Reverse(*ix)));
        let buttons = order.into_iter().map(|ix| {
            let action = &self.actions[ix];
            let label = action.label().clone();
            let id = ("prompt-action", ix);
            let button = if action.is_cancel() || Some(ix) != default {
                ghost_button(id, label)
            } else if self.destructive(&label) {
                danger_button(id, label)
            } else {
                primary_button(id, label)
            };
            button
                .cursor_pointer()
                .on_click(cx.listener(move |_, _, _, cx| cx.emit(PromptResponse(ix))))
        });
        let cancel = self.cancel();
        let card = card().child(
            card_body()
                .child(title(self.message.clone()))
                .when_some(self.detail.clone(), |d, detail| {
                    d.child(body(detail).mt(px(6.)))
                })
                .child(actions().children(buttons)),
        );
        div()
            .size_full()
            .track_focus(&self.focus)
            .key_context("Prompt")
            .on_key_down(cx.listener(Self::key))
            .child(overlay("prompt", card, {
                let entity = cx.weak_entity();
                move |_, cx| {
                    let _ = entity.update(cx, |_, cx| cx.emit(PromptResponse(cancel)));
                }
            }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::TestAppContext;

    #[gpui::test]
    fn prompts_answer_with_keys_and_buttons_in_sheet_order(cx: &mut TestAppContext) {
        cx.update(|cx| cx.set_prompt_builder(prompt_builder));
        let window = cx.add_window(|_, _| gpui::Empty);
        let mut answer = window
            .update(cx, |_, window, cx| {
                window.prompt(
                    PromptLevel::Warning,
                    "Delete?",
                    None,
                    &["Delete", "Cancel"],
                    cx,
                )
            })
            .unwrap();
        cx.run_until_parked();
        let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
        visual.simulate_keystrokes("escape");
        visual.run_until_parked();
        assert_eq!(answer.try_recv(), Ok(Some(1)));
        let mut answer = window
            .update(visual, |_, window, cx| {
                window.prompt(
                    PromptLevel::Info,
                    "Continue?",
                    Some("Detail"),
                    &["OK", "Cancel"],
                    cx,
                )
            })
            .unwrap();
        visual.run_until_parked();
        visual.simulate_keystrokes("enter");
        visual.run_until_parked();
        assert_eq!(answer.try_recv(), Ok(Some(0)));
    }

    /// A background that records every pointer event it receives, a barrier over it, and a
    /// card button above the barrier.
    #[derive(Default)]
    struct Probe {
        barrier: bool,
        hovered: bool,
        downs: usize,
        right_downs: usize,
        scrolls: usize,
        card_clicks: usize,
    }

    impl Render for Probe {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let view = cx.entity();
            let background = div()
                .id("probe-background")
                .absolute()
                .inset_0()
                .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                    this.hovered = *hovered;
                    cx.notify();
                }))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, _| this.downs += 1),
                )
                .on_mouse_down(
                    MouseButton::Right,
                    cx.listener(|this, _, _, _| this.right_downs += 1),
                )
                .on_scroll_wheel(cx.listener(|this, _, _, _| this.scrolls += 1));
            let overlay = self.barrier.then(|| {
                let card = div()
                    .id("probe-card")
                    .debug_selector(|| "probe-card".into())
                    .w(px(120.))
                    .h(px(40.))
                    .on_click(move |_, _, cx| view.update(cx, |this, _| this.card_clicks += 1));
                div().child(barrier("probe-barrier", 60, window)).child(
                    gpui::deferred(
                        gpui::anchored()
                            .position(gpui::point(px(400.), px(300.)))
                            .child(card),
                    )
                    .with_priority(60),
                )
            });
            div().size_full().child(background).children(overlay)
        }
    }

    fn scroll(position: gpui::Point<gpui::Pixels>) -> gpui::ScrollWheelEvent {
        gpui::ScrollWheelEvent {
            position,
            delta: gpui::ScrollDelta::Pixels(gpui::point(px(0.), px(-20.))),
            modifiers: Default::default(),
            touch_phase: gpui::TouchPhase::Moved,
        }
    }

    /// Moves, left-clicks, right-clicks and scrolls over the background at `at`.
    fn poke(visual: &mut gpui::VisualTestContext, at: gpui::Point<gpui::Pixels>) {
        visual.simulate_mouse_move(at, None, Default::default());
        visual.run_until_parked();
        visual.refresh().unwrap();
        visual.simulate_click(at, Default::default());
        visual.simulate_mouse_down(at, MouseButton::Right, Default::default());
        visual.simulate_mouse_up(at, MouseButton::Right, Default::default());
        visual.simulate_event(scroll(at));
        visual.run_until_parked();
    }

    #[gpui::test]
    fn the_barrier_blocks_the_window_behind_it_and_leaves_the_card_working(
        cx: &mut TestAppContext,
    ) {
        cx.update(|cx| cx.set_reduce_motion(true));
        let window = cx.add_window(|_, _| Probe::default());
        let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
        visual.run_until_parked();
        let at = gpui::point(px(100.), px(100.));
        let counts = |visual: &mut gpui::VisualTestContext| {
            window
                .update(visual, |p, _, _| {
                    (p.hovered, p.downs, p.right_downs, p.scrolls, p.card_clicks)
                })
                .unwrap()
        };

        // The probe sees every kind of event before the barrier exists.
        poke(visual, at);
        assert_eq!(counts(visual), (true, 1, 1, 1, 0), "the probe must work");
        visual.simulate_mouse_move(gpui::point(px(900.), px(700.)), None, Default::default());
        visual.run_until_parked();
        window
            .update(visual, |p, _, _| {
                *p = Probe {
                    barrier: true,
                    ..Default::default()
                }
            })
            .unwrap();
        visual.run_until_parked();
        visual.refresh().unwrap();

        // With the barrier up the background gets nothing.
        poke(visual, at);
        assert_eq!(counts(visual), (false, 0, 0, 0, 0), "the barrier occludes");

        // The card above the barrier still takes clicks.
        let card = visual.debug_bounds("probe-card").expect("the card");
        visual.simulate_click(card.center(), Default::default());
        visual.run_until_parked();
        assert_eq!(counts(visual), (false, 0, 0, 0, 1));

        // Hiding the barrier gives the background its events back.
        window.update(visual, |p, _, _| p.barrier = false).unwrap();
        visual.run_until_parked();
        visual.refresh().unwrap();
        visual.simulate_mouse_move(gpui::point(px(900.), px(700.)), None, Default::default());
        visual.run_until_parked();
        poke(visual, at);
        assert_eq!(counts(visual), (true, 1, 1, 1, 1), "no stuck barrier");
    }
}
