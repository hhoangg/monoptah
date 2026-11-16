//! The standard side drawer: a panel that slides in from the right edge of the window over
//! the same fading scrim the modals use, for content that wants a tall column instead of a
//! centred card (see [`crate::modal`] for the dialogs). The panel is a [`modal::card`] inset
//! from the window edges, so it shares the dialog surface, rule and shadow.
//!
//! Like a modal it is a barrier: the window behind gets no hover, click or scroll. A press on
//! the scrim calls `on_dismiss`; the panel swallows its own presses. Escape and a close button
//! belong to the content (it holds the focus, as a dialog does).
//!
//! # Opening and closing
//!
//! The caller owns a [`DrawerState`] next to whatever says the drawer is wanted, renders
//! [`drawer`] while it holds one and routes every way of closing (Escape, the
//! scrim, a Close button) to [`DrawerState::close`]. Closing does not drop the drawer at once:
//! it slides back out to the right edge (the reverse of the slide-in, the scrim fading with
//! it) and `close` schedules `done` on the executor timer for when it is gone, where the caller
//! drops it. Meanwhile the drawer is still a barrier and takes no input: a second dismiss is
//! ignored ([`Close::Ignored`]), the scrim swallows presses and an occluding sheet covers the
//! panel. Opening again while it slides out ([`DrawerState::reopen`]) cancels the removal and
//! slides back in from where the panel is, so nothing jumps. With reduce motion on there is
//! nothing to wait for: `close` returns [`Close::Now`] and the caller drops the drawer itself.
//!
//! The slide is computed in [`drawer`] from the state's [`Motion`] and the executor clock
//! (`BackgroundExecutor::now`, the real clock in the app, the test clock in tests), the same
//! clock the removal timer runs on, so the drawer is dropped exactly when the slide-out has
//! reached the edge and tests step both with `advance_clock`. No gpui animation element is used
//! on purpose: it would push a new id onto the element-id stack per phase and reset the state
//! (scroll offsets, focus-less hover) of everything inside the panel. Element ids stay the same
//! across phases.
use crate::modal;
use gpui::{
    App, Context, ElementId, IntoElement, MouseButton, SharedString, Task, Window, anchored,
    deferred, div, point, prelude::*, px,
};
use std::time::{Duration, Instant};

/// The gap between the panel and the top, right and bottom edges of the window, in points.
pub(crate) const INSET: f32 = 8.;
/// Extra distance the panel starts off-screen so its shadow does not peek in at the edge.
const SHADOW_ROOM: f32 = 56.;
const SLIDE: Duration = Duration::from_millis(240);
/// The slide-out from the resting place; a drawer that is part-way in leaves in proportion.
const SLIDE_OUT: Duration = Duration::from_millis(200);

fn ease_out(t: f32) -> f32 {
    1. - (1. - t).powi(3)
}

fn ease_in(t: f32) -> f32 {
    t.powi(3)
}

/// The panel width that fits a window `viewport` points wide: `wanted`, but never so wide that
/// the scrim to its left disappears.
pub(crate) fn fit_width(wanted: f32, viewport: f32) -> f32 {
    wanted.min((viewport - 2. * INSET - 16.).max(0.))
}

/// The panel's `right` offset at `p` (0 to 1) of the way in: off the edge at 0, resting `INSET`
/// from it at 1.
fn right_offset(width: f32, p: f32) -> f32 {
    INSET - (width + INSET + SHADOW_ROOM) * (1. - p)
}

/// One leg of the motion: `p` goes from `from` to `to` over `duration`, eased. `p` is how far in
/// the panel is, and how opaque the scrim is.
#[derive(Clone, Copy)]
struct Motion {
    from: f32,
    to: f32,
    duration: Duration,
    ease: fn(f32) -> f32,
    started: Instant,
}

impl Motion {
    fn at(&self, now: Instant) -> f32 {
        let t = if self.duration.is_zero() {
            1.
        } else {
            (now.saturating_duration_since(self.started).as_secs_f32()
                / self.duration.as_secs_f32())
            .clamp(0., 1.)
        };
        self.from + (self.to - self.from) * (self.ease)(t)
    }

    /// The motion has reached its end (a zero duration is done at once).
    fn done(&self, now: Instant) -> bool {
        now.saturating_duration_since(self.started) >= self.duration
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Phase {
    Open,
    Closing,
}

/// What [`DrawerState::close`] did.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Close {
    /// It was not open (already sliding out): nothing happened.
    Ignored,
    /// It slides out; `done` runs when it is gone.
    Started,
    /// Motion is off: there is nothing to wait for, drop the drawer now. `done` does not run.
    Now,
}

/// Whether a drawer is in, or sliding out. The caller drops it from its render once it is gone.
pub(crate) struct DrawerState {
    phase: Phase,
    motion: Motion,
    /// The pending removal while closing; dropping it cancels the removal.
    removal: Option<Task<()>>,
}

impl DrawerState {
    /// A drawer that slides in now.
    pub(crate) fn open<V: 'static>(cx: &mut Context<V>) -> Self {
        Self {
            phase: Phase::Open,
            motion: Motion {
                from: 0.,
                to: 1.,
                duration: SLIDE,
                ease: ease_out,
                started: cx.background_executor().now(),
            },
            removal: None,
        }
    }

    /// Open, not sliding out: the one that takes Escape and dismissals.
    pub(crate) fn is_open(&self) -> bool {
        self.phase == Phase::Open
    }

    pub(crate) fn is_closing(&self) -> bool {
        self.phase == Phase::Closing
    }

    fn position<V: 'static>(&self, cx: &Context<V>) -> f32 {
        self.motion.at(cx.background_executor().now())
    }

    /// Slides the drawer out and runs `done` on the owner when it is gone (then the caller drops
    /// it, if [`DrawerState::is_closing`] still holds). See [`Close`].
    pub(crate) fn close<V: 'static>(
        &mut self,
        cx: &mut Context<V>,
        done: impl FnOnce(&mut V, &mut Context<V>) + 'static,
    ) -> Close {
        if self.phase != Phase::Open {
            return Close::Ignored;
        }
        if cx.reduce_motion() {
            return Close::Now;
        }
        let now = cx.background_executor().now();
        let from = self.motion.at(now);
        let duration = SLIDE_OUT.mul_f32(from);
        self.phase = Phase::Closing;
        self.motion = Motion {
            from,
            to: 0.,
            duration,
            ease: ease_in,
            started: now,
        };
        self.removal = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(duration).await;
            let _ = this.update(cx, |this, cx| done(this, cx));
        }));
        Close::Started
    }

    /// Opens a drawer that is sliding out again: the removal is cancelled and the panel slides
    /// back in from where it is. Does nothing when it is open.
    pub(crate) fn reopen<V: 'static>(&mut self, cx: &mut Context<V>) {
        if self.phase != Phase::Closing {
            return;
        }
        let from = self.position(cx);
        self.phase = Phase::Open;
        self.removal = None;
        self.motion = Motion {
            from,
            to: 1.,
            duration: SLIDE,
            ease: ease_out,
            started: cx.background_executor().now(),
        };
    }
}

/// Lays `content` in a panel `width` points wide that slides in from the right, over a fading
/// scrim that blocks the window behind it. `priority` is where it draws (see
/// [`modal::BLOCKING_PRIORITY`] for the scale); popovers opened from the content draw above it.
/// A press on the scrim calls `on_dismiss` (not while it slides out). `content` fills the panel's
/// height and should scroll its own overflow. See the module docs for `state`;
/// `cx` gives the clock.
///
/// Debug selectors: `id` (the panel) and `{id}-scrim`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn drawer(
    id: &'static str,
    width: f32,
    priority: usize,
    state: &DrawerState,
    content: impl IntoElement,
    on_dismiss: impl Fn(&mut Window, &mut App) + 'static,
    window: &Window,
    cx: &App,
) -> impl IntoElement {
    let size = window.viewport_size();
    let width = fit_width(width, size.width.as_f32());
    let closing = state.is_closing();
    let now = cx.background_executor().now();
    // With motion off a drawer is not eased: it rests at the end of its leg.
    let p = if cx.reduce_motion() {
        state.motion.to
    } else {
        // Keep frames coming while it moves.
        if !state.motion.done(now) {
            window.request_animation_frame();
        }
        state.motion.at(now)
    };
    let panel = modal::card()
        .id(ElementId::from(id))
        .debug_selector(move || id.into())
        .absolute()
        .top(px(INSET))
        .bottom(px(INSET))
        .w(px(width))
        .right(px(right_offset(width, p)))
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .child(
            div()
                .relative()
                .size_full()
                .flex()
                .flex_col()
                .overflow_hidden()
                .rounded(px(modal::RADIUS - 1.))
                .child(content)
                // Sliding out, the panel takes no more input: this sheet occludes it.
                .when(closing, |d| {
                    d.child(
                        div()
                            .id(SharedString::from(format!("{id}-closing")))
                            .absolute()
                            .inset_0()
                            .occlude(),
                    )
                }),
        );
    let mut scrim_bg = modal::scrim_color();
    scrim_bg.a *= p;
    let scrim = div()
        .id(SharedString::from(format!("{id}-scrim")))
        .debug_selector(move || format!("{id}-scrim"))
        .absolute()
        .inset_0()
        .occlude()
        .bg(scrim_bg)
        .on_mouse_down(MouseButton::Left, move |_, window, cx| {
            if !closing {
                on_dismiss(window, cx)
            }
        })
        .child(panel);
    deferred(
        anchored()
            .position(point(px(0.), px(0.)))
            .child(div().relative().w(size.width).h(size.height).child(scrim)),
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
        drawer: Option<DrawerState>,
        background_downs: usize,
        dismissals: Rc<Cell<usize>>,
        content_clicks: usize,
        removed: usize,
    }

    impl Host {
        fn close(&mut self, cx: &mut Context<Self>) -> Close {
            let Some(drawer) = self.drawer.as_mut() else {
                return Close::Ignored;
            };
            let result = drawer.close(cx, |this, cx| {
                if this.drawer.as_ref().is_some_and(DrawerState::is_closing) {
                    this.drawer = None;
                    this.removed += 1;
                    cx.notify();
                }
            });
            if result == Close::Now {
                self.drawer = None;
                self.removed += 1;
            }
            cx.notify();
            result
        }
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
            let weak = cx.weak_entity();
            let drawer = self.drawer.as_ref().map(|state| {
                drawer(
                    "host-drawer",
                    400.,
                    60,
                    state,
                    div()
                        .id("host-content")
                        .debug_selector(|| "host-content".into())
                        .size_full()
                        .overflow_y_scroll()
                        .child(
                            div()
                                .debug_selector(|| "host-tall".into())
                                .w_full()
                                .h(px(3000.)),
                        )
                        .on_click(cx.listener(|this, _, _, _| this.content_clicks += 1)),
                    move |_, cx| {
                        dismissals.set(dismissals.get() + 1);
                        let _ = weak.update(cx, |this, cx| {
                            this.close(cx);
                        });
                    },
                    window,
                    cx,
                )
            });
            div().size_full().child(background).children(drawer)
        }
    }

    fn open_host_with(
        cx: &mut TestAppContext,
        reduce_motion: bool,
    ) -> (gpui::WindowHandle<Host>, VisualTestContext) {
        cx.update(|cx| cx.set_reduce_motion(reduce_motion));
        let window = cx.add_window(|_, _| Host::default());
        let mut visual = VisualTestContext::from_window(window.into(), cx);
        visual.run_until_parked();
        window
            .update(&mut visual, |h, _, cx| {
                h.drawer = Some(DrawerState::open(cx));
                cx.notify();
            })
            .unwrap();
        visual.run_until_parked();
        visual.refresh().unwrap();
        (window, visual)
    }

    fn open_host(cx: &mut TestAppContext) -> (gpui::WindowHandle<Host>, VisualTestContext) {
        open_host_with(cx, true)
    }

    fn close_host(window: &gpui::WindowHandle<Host>, visual: &mut VisualTestContext) -> Close {
        window.update(visual, |h, _, cx| h.close(cx)).unwrap()
    }

    fn has_drawer(window: &gpui::WindowHandle<Host>, visual: &VisualTestContext) -> bool {
        window.read_with(visual, |h, _| h.drawer.is_some()).unwrap()
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
        let (_, mut visual) = open_host_with(cx, false);
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
    fn with_motion_off_closing_drops_the_drawer_at_once(cx: &mut TestAppContext) {
        let (window, mut visual) = open_host(cx);
        assert_eq!(close_host(&window, &mut visual), Close::Now);
        visual.run_until_parked();
        visual.refresh().unwrap();
        assert!(!has_drawer(&window, &visual));
        assert!(visual.debug_bounds("host-drawer").is_none());
        visual.simulate_click(point(px(20.), px(20.)), Default::default());
        visual.run_until_parked();
        window
            .read_with(&visual, |h, _| assert_eq!(h.background_downs, 1))
            .unwrap();
    }

    #[gpui::test]
    fn the_drawer_stays_and_blocks_input_while_it_slides_out_then_goes(cx: &mut TestAppContext) {
        let (window, mut visual) = open_host_with(cx, false);
        // Let the slide-in settle on the executor clock too.
        visual.executor().advance_clock(SLIDE);
        visual.run_until_parked();
        assert_eq!(close_host(&window, &mut visual), Close::Started);
        visual.run_until_parked();
        visual.refresh().unwrap();
        assert!(
            has_drawer(&window, &visual),
            "still rendered while it leaves"
        );
        assert!(visual.debug_bounds("host-drawer").is_some());

        // A second dismiss is ignored, and a press anywhere reaches neither the window behind,
        // the content nor the dismissal.
        assert_eq!(close_host(&window, &mut visual), Close::Ignored);
        visual.simulate_click(point(px(20.), px(20.)), Default::default());
        let content = visual.debug_bounds("host-content").expect("the content");
        visual.simulate_click(content.center(), Default::default());
        visual.run_until_parked();
        window
            .read_with(&visual, |h, _| {
                assert_eq!(h.dismissals.get(), 0, "the scrim no longer dismisses");
                assert_eq!(h.background_downs, 0);
                assert_eq!(h.content_clicks, 0, "the panel takes no input");
                assert_eq!(h.removed, 0);
            })
            .unwrap();

        visual.executor().advance_clock(SLIDE_OUT);
        visual.run_until_parked();
        visual.refresh().unwrap();
        assert!(!has_drawer(&window, &visual), "dropped after the slide-out");
        assert!(visual.debug_bounds("host-drawer").is_none());
        window
            .read_with(&visual, |h, _| assert_eq!(h.removed, 1))
            .unwrap();
        visual.simulate_click(point(px(20.), px(20.)), Default::default());
        visual.run_until_parked();
        window
            .read_with(&visual, |h, _| assert_eq!(h.background_downs, 1))
            .unwrap();
    }

    #[gpui::test]
    fn a_scrim_press_starts_the_slide_out(cx: &mut TestAppContext) {
        let (window, mut visual) = open_host_with(cx, false);
        visual.executor().advance_clock(SLIDE);
        visual.run_until_parked();
        visual.simulate_click(point(px(20.), px(20.)), Default::default());
        visual.run_until_parked();
        visual.refresh().unwrap();
        assert!(has_drawer(&window, &visual));
        window
            .read_with(&visual, |h, _| {
                assert_eq!(h.dismissals.get(), 1);
                assert!(h.drawer.as_ref().unwrap().is_closing());
            })
            .unwrap();
        visual.executor().advance_clock(SLIDE_OUT);
        visual.run_until_parked();
        assert!(!has_drawer(&window, &visual));
    }

    #[gpui::test]
    fn reopening_during_the_slide_out_cancels_the_removal(cx: &mut TestAppContext) {
        let (window, mut visual) = open_host_with(cx, false);
        visual.executor().advance_clock(SLIDE);
        visual.run_until_parked();
        assert_eq!(close_host(&window, &mut visual), Close::Started);
        visual.executor().advance_clock(SLIDE_OUT / 2);
        visual.run_until_parked();
        window
            .update(&mut visual, |h, _, cx| {
                h.drawer.as_mut().unwrap().reopen(cx);
                cx.notify();
            })
            .unwrap();
        visual.run_until_parked();
        visual.refresh().unwrap();
        // The pending removal is gone: well past the slide-out, it is still there and open.
        visual.executor().advance_clock(SLIDE_OUT * 3);
        visual.run_until_parked();
        visual.refresh().unwrap();
        assert!(has_drawer(&window, &visual));
        window
            .read_with(&visual, |h, _| {
                assert_eq!(h.removed, 0);
                assert!(h.drawer.as_ref().unwrap().is_open());
            })
            .unwrap();
        // It takes input again and can close again.
        let content = visual.debug_bounds("host-content").expect("the content");
        visual.simulate_click(content.center(), Default::default());
        visual.run_until_parked();
        window
            .read_with(&visual, |h, _| assert_eq!(h.content_clicks, 1))
            .unwrap();
        assert_eq!(close_host(&window, &mut visual), Close::Started);
        visual.executor().advance_clock(SLIDE_OUT);
        visual.run_until_parked();
        assert!(!has_drawer(&window, &visual));
    }

    #[gpui::test]
    fn closing_part_way_through_the_slide_in_leaves_from_there(cx: &mut TestAppContext) {
        let (window, mut visual) = open_host_with(cx, false);
        // Nothing has advanced: the drawer is at (or very near) the start of the slide-in, so
        // its slide-out is nearly instant but still goes through the removal.
        assert_eq!(close_host(&window, &mut visual), Close::Started);
        visual.executor().advance_clock(SLIDE_OUT);
        visual.run_until_parked();
        assert!(!has_drawer(&window, &visual));
    }

    /// Re-renders the host (its drawer is computed from the clock at render time) and draws.
    fn redraw(window: &gpui::WindowHandle<Host>, visual: &mut VisualTestContext) {
        window.update(visual, |_, _, cx| cx.notify()).unwrap();
        visual.run_until_parked();
        visual.refresh().unwrap();
    }

    fn panel_left(visual: &mut VisualTestContext) -> gpui::Pixels {
        visual
            .debug_bounds("host-drawer")
            .expect("the panel")
            .left()
    }

    #[gpui::test]
    fn the_panel_visibly_moves_right_and_off_screen_before_it_is_dropped(cx: &mut TestAppContext) {
        let (window, mut visual) = open_host_with(cx, false);
        let viewport = visual.update(|w, _| w.viewport_size().width);
        // Slide-in: part-way, then at rest.
        visual.executor().advance_clock(SLIDE / 2);
        redraw(&window, &mut visual);
        let halfway_in = panel_left(&mut visual);
        visual.executor().advance_clock(SLIDE);
        redraw(&window, &mut visual);
        let rest = panel_left(&mut visual);
        assert_eq!(rest, viewport - px(INSET) - px(400.));
        assert!(halfway_in > rest, "{halfway_in:?} still on its way in");

        // Slide-out, stepped on the same clock that removes it.
        assert_eq!(close_host(&window, &mut visual), Close::Started);
        redraw(&window, &mut visual);
        assert_eq!(
            panel_left(&mut visual),
            rest,
            "it starts from where it rests"
        );
        visual.executor().advance_clock(SLIDE_OUT / 2);
        redraw(&window, &mut visual);
        let halfway_out = panel_left(&mut visual);
        assert!(halfway_out > rest, "{halfway_out:?} moved right");
        visual
            .executor()
            .advance_clock(SLIDE_OUT / 2 - Duration::from_millis(1));
        redraw(&window, &mut visual);
        assert!(has_drawer(&window, &visual), "not dropped before the end");
        let nearly = panel_left(&mut visual);
        assert!(nearly > halfway_out, "{nearly:?} keeps moving");
        assert!(
            nearly >= viewport,
            "{nearly:?} is off the right edge when it is dropped"
        );
        visual.executor().advance_clock(Duration::from_millis(1));
        redraw(&window, &mut visual);
        assert!(!has_drawer(&window, &visual));
    }

    #[gpui::test]
    fn a_scrolled_body_keeps_its_scroll_through_close_and_reopen(cx: &mut TestAppContext) {
        let (window, mut visual) = open_host_with(cx, false);
        visual.executor().advance_clock(SLIDE);
        redraw(&window, &mut visual);
        let panel = visual.debug_bounds("host-drawer").unwrap();
        visual.simulate_event(gpui::ScrollWheelEvent {
            position: panel.center(),
            delta: gpui::ScrollDelta::Pixels(point(px(0.), px(-300.))),
            ..Default::default()
        });
        redraw(&window, &mut visual);
        let scrolled = visual.debug_bounds("host-tall").unwrap().origin.y;
        assert!(
            scrolled < panel.origin.y - px(100.),
            "{scrolled:?} scrolled up"
        );

        assert_eq!(close_host(&window, &mut visual), Close::Started);
        visual.executor().advance_clock(SLIDE_OUT / 2);
        redraw(&window, &mut visual);
        assert_eq!(
            visual.debug_bounds("host-tall").unwrap().origin.y,
            scrolled,
            "the slide-out does not reset the scroll"
        );
        window
            .update(&mut visual, |h, _, cx| {
                h.drawer.as_mut().unwrap().reopen(cx);
                cx.notify();
            })
            .unwrap();
        visual.executor().advance_clock(SLIDE / 2);
        redraw(&window, &mut visual);
        assert_eq!(
            visual.debug_bounds("host-tall").unwrap().origin.y,
            scrolled,
            "nor does reopening"
        );
    }

    #[test]
    fn the_slide_out_mirrors_the_slide_in() {
        let now = Instant::now();
        let slide_in = Motion {
            from: 0.,
            to: 1.,
            duration: SLIDE,
            ease: ease_out,
            started: now,
        };
        assert_eq!(slide_in.at(now), 0.);
        assert_eq!(slide_in.at(now + SLIDE), 1.);
        let slide_out = Motion {
            from: 1.,
            to: 0.,
            duration: SLIDE_OUT,
            ease: ease_in,
            started: now,
        };
        assert_eq!(slide_out.at(now), 1.);
        assert_eq!(slide_out.at(now + SLIDE_OUT * 2), 0.);
        assert!(
            slide_out.at(now + SLIDE_OUT / 2) > 0.5,
            "leaves slowly first"
        );
    }
}
