//! The standard side drawer: a panel that slides in from the right edge of the window over
//! the same fading scrim the modals use, for content that wants a tall column instead of a
//! centred card (see [`crate::modal`] for the dialogs). The panel is a [`modal::card`] inset
//! from the window edges, so it shares the dialog surface, rule and shadow.
//!
//! Like a modal it is a barrier: the window behind gets no hover, click or scroll. A press on
//! the scrim calls `on_dismiss`; the panel swallows its own presses. Escape and a close button
//! belong to the content (it holds the focus, as a dialog does), and the caller drops the
//! drawer from its render to close it.
use crate::modal;
use gpui::{
    Animation, AnimationExt, App, ElementId, IntoElement, MouseButton, SharedString, Window,
    anchored, deferred, div, point, prelude::*, px,
};
use std::time::Duration;

/// The gap between the panel and the top, right and bottom edges of the window, in points.
pub(crate) const INSET: f32 = 8.;
/// Extra distance the panel starts off-screen so its shadow does not peek in at the edge.
const SHADOW_ROOM: f32 = 56.;
const SLIDE: Duration = Duration::from_millis(240);

fn ease_out(t: f32) -> f32 {
    1. - (1. - t).powi(3)
}

/// The panel width that fits a window `viewport` points wide: `wanted`, but never so wide that
/// the scrim to its left disappears.
pub(crate) fn fit_width(wanted: f32, viewport: f32) -> f32 {
    wanted.min((viewport - 2. * INSET - 16.).max(0.))
}

/// The panel's `right` offset `t` of the way (0 to 1) through the slide-in: off the edge at
/// 0, resting `INSET` from it at 1.
fn right_offset(width: f32, t: f32) -> f32 {
    INSET - (width + INSET + SHADOW_ROOM) * (1. - t)
}

/// Lays `content` in a panel `width` points wide that slides in from the right, over a fading
/// scrim that blocks the window behind it. `priority` is where it draws (see
/// [`modal::BLOCKING_PRIORITY`] for the scale); popovers opened from the content draw above it.
/// A press on the scrim calls `on_dismiss`. `content` fills the panel's height and should scroll
/// its own overflow.
///
/// Debug selectors: `id` (the panel) and `{id}-scrim`.
pub(crate) fn drawer(
    id: &'static str,
    width: f32,
    priority: usize,
    content: impl IntoElement,
    on_dismiss: impl Fn(&mut Window, &mut App) + 'static,
    window: &Window,
) -> impl IntoElement {
    let size = window.viewport_size();
    let width = fit_width(width, size.width.as_f32());
    let panel = modal::card()
        .id(ElementId::from(id))
        .debug_selector(move || id.into())
        .absolute()
        .top(px(INSET))
        .bottom(px(INSET))
        .w(px(width))
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .child(
            div()
                .relative()
                .size_full()
                .flex()
                .flex_col()
                .overflow_hidden()
                .rounded(px(modal::RADIUS - 1.))
                .child(content),
        )
        .with_animation(
            SharedString::from(format!("{id}-slide")),
            Animation::new(SLIDE).with_easing(ease_out),
            move |el, t| el.right(px(right_offset(width, t))),
        );
    deferred(
        anchored().position(point(px(0.), px(0.))).child(
            div()
                .relative()
                .w(size.width)
                .h(size.height)
                .child(modal::backdrop(
                    SharedString::from(format!("{id}-scrim")),
                    div()
                        .id(SharedString::from(format!("{id}-scrim")))
                        .debug_selector(move || format!("{id}-scrim"))
                        .absolute()
                        .inset_0()
                        .occlude()
                        .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                            on_dismiss(window, cx)
                        })
                        .child(panel),
                )),
        ),
    )
    .with_priority(priority)
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{Context, Render, TestAppContext, VisualTestContext};
    use std::{cell::Cell, rc::Rc};

    #[test]
    fn the_panel_starts_off_the_edge_and_rests_inset_from_it() {
        assert!(
            right_offset(400., 0.) < -400.,
            "fully off screen at the start"
        );
        assert_eq!(right_offset(400., 1.), INSET);
        assert!(right_offset(400., 0.5) > right_offset(400., 0.));
        assert!(right_offset(400., 0.5) < right_offset(400., 1.));
    }

    #[test]
    fn the_panel_leaves_room_for_the_scrim_in_a_narrow_window() {
        assert_eq!(fit_width(520., 1200.), 520.);
        assert!(fit_width(520., 400.) < 400. - 2. * INSET);
        assert_eq!(fit_width(520., 10.), 0.);
    }

    /// A background that counts the presses it receives, and optionally a drawer over it.
    #[derive(Default)]
    struct Host {
        open: bool,
        background_downs: usize,
        dismissals: Rc<Cell<usize>>,
        content_clicks: usize,
    }

    impl Render for Host {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let background = div()
                .id("host-background")
                .absolute()
                .inset_0()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, _| this.background_downs += 1),
                );
            let dismissals = self.dismissals.clone();
            let drawer = self.open.then(|| {
                drawer(
                    "host-drawer",
                    400.,
                    60,
                    div()
                        .id("host-content")
                        .debug_selector(|| "host-content".into())
                        .size_full()
                        .on_click(cx.listener(|this, _, _, _| this.content_clicks += 1)),
                    move |_, _| dismissals.set(dismissals.get() + 1),
                    window,
                )
            });
            div().size_full().child(background).children(drawer)
        }
    }

    fn open_host(cx: &mut TestAppContext) -> (gpui::WindowHandle<Host>, VisualTestContext) {
        cx.update(|cx| cx.set_reduce_motion(true));
        let window = cx.add_window(|_, _| Host::default());
        let mut visual = VisualTestContext::from_window(window.into(), cx);
        visual.run_until_parked();
        window
            .update(&mut visual, |h, _, cx| {
                h.open = true;
                cx.notify();
            })
            .unwrap();
        visual.run_until_parked();
        visual.refresh().unwrap();
        (window, visual)
    }

    #[gpui::test]
    fn the_drawer_rests_against_the_right_edge_and_fills_the_height(cx: &mut TestAppContext) {
        let (_, mut visual) = open_host(cx);
        let window_size = visual.update(|window, _| window.viewport_size());
        let panel = visual.debug_bounds("host-drawer").expect("the panel");
        assert_eq!(panel.size.width, px(400.));
        assert_eq!(panel.right(), window_size.width - px(INSET));
        assert_eq!(panel.origin.y, px(INSET));
        assert_eq!(panel.bottom(), window_size.height - px(INSET));
        let content = visual.debug_bounds("host-content").expect("the content");
        assert_eq!(content.size.height, panel.size.height - px(2.));
    }

    #[gpui::test]
    fn the_panel_starts_off_screen_when_motion_is_on(cx: &mut TestAppContext) {
        let window = cx.add_window(|_, _| Host::default());
        let mut visual = VisualTestContext::from_window(window.into(), cx);
        visual.run_until_parked();
        window
            .update(&mut visual, |h, _, cx| {
                h.open = true;
                cx.notify();
            })
            .unwrap();
        visual.run_until_parked();
        visual.refresh().unwrap();
        let width = visual.update(|window, _| window.viewport_size().width);
        let panel = visual.debug_bounds("host-drawer").expect("the panel");
        // Not yet at rest, whatever the clock does in the first frames: the exact curve is the
        // `right_offset` test's business.
        assert!(
            panel.right() > width - px(INSET),
            "{panel:?} slides in from the right"
        );
    }

    #[gpui::test]
    fn a_press_on_the_scrim_dismisses_and_never_reaches_the_window_behind(cx: &mut TestAppContext) {
        let (window, mut visual) = open_host(cx);
        let scrim = visual.debug_bounds("host-drawer-scrim").expect("the scrim");
        assert_eq!(scrim.size, visual.update(|w, _| w.viewport_size()));
        visual.simulate_click(point(px(20.), px(20.)), Default::default());
        visual.run_until_parked();
        window
            .read_with(&visual, |h, _| {
                assert_eq!(h.dismissals.get(), 1);
                assert_eq!(h.background_downs, 0, "the barrier occludes the window");
            })
            .unwrap();
    }

    #[gpui::test]
    fn a_press_on_the_panel_stays_on_the_panel(cx: &mut TestAppContext) {
        let (window, mut visual) = open_host(cx);
        let content = visual.debug_bounds("host-content").expect("the content");
        visual.simulate_click(content.center(), Default::default());
        visual.run_until_parked();
        window
            .read_with(&visual, |h, _| {
                assert_eq!(h.dismissals.get(), 0, "the panel swallows its presses");
                assert_eq!(h.background_downs, 0);
                assert_eq!(h.content_clicks, 1, "and the content still works");
            })
            .unwrap();
    }

    #[gpui::test]
    fn dropping_the_drawer_gives_the_window_its_presses_back(cx: &mut TestAppContext) {
        let (window, mut visual) = open_host(cx);
        window
            .update(&mut visual, |h, _, cx| {
                h.open = false;
                cx.notify();
            })
            .unwrap();
        visual.run_until_parked();
        visual.refresh().unwrap();
        assert!(visual.debug_bounds("host-drawer").is_none());
        visual.simulate_click(point(px(20.), px(20.)), Default::default());
        visual.run_until_parked();
        window
            .read_with(&visual, |h, _| assert_eq!(h.background_downs, 1))
            .unwrap();
    }
}
