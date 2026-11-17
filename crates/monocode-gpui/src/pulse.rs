//! One shared clock for decorative loops: spinners, mascots, avatars and the
//! composer runner.
//!
//! A view that paints a loop leases the clock for itself. Each tick notifies
//! every leased view together, so all loops land in one frame instead of one
//! frame per spinner, and at 30Hz or less instead of the display's 120Hz. A
//! lease ends when its view was notified but did not paint a loop again, so
//! a closed, scrolled-away or hidden spinner stops costing anything, and the
//! clock parks once no lease is left. While no window is visible the clock
//! slows every loop down further; a visible window behind another app's keeps
//! the full pace, since its loops are still on screen.
//!
//! Every lease records where it was requested, and launching with
//! `MONOPTAH_PULSE_STATS=1` logs the leases, their owners and the tick and draw
//! rates every ten seconds (see [`start_stats`]).
use gpui::{
    AnyElement, App, Element, ElementId, EntityId, Global, GlobalElementId, InspectorElementId,
    IntoElement, LayoutId, Pixels, Window,
};
use std::{
    collections::HashMap,
    panic::Location,
    time::{Duration, Instant},
};

const TICK: Duration = Duration::from_millis(33);
/// A hidden app's loops tick this many times slower.
const BACKGROUND_STRIDE: u64 = 8;
/// How often [`start_stats`] writes a line.
const STATS_PERIOD: Duration = Duration::from_secs(10);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Rate {
    /// About 30fps: rotations and fades that must look continuous.
    Smooth,
    /// About 15fps: small loaders.
    Half,
    /// About 7.5fps: pixel art that steps between a few poses.
    Low,
}
impl Rate {
    fn stride(self) -> u64 {
        match self {
            Rate::Smooth => 1,
            Rate::Half => 2,
            Rate::Low => 4,
        }
    }
}

type FrameCallback = Box<dyn FnOnce(&mut App)>;
struct Lease {
    rate: Rate,
    /// Call sites that requested this view's loops, for the stats line.
    owners: Vec<&'static Location<'static>>,
    /// Painted a loop since the clock last notified it.
    painted: bool,
    on_tick: Option<FrameCallback>,
}

#[derive(Default)]
struct Clock {
    epoch: Option<Instant>,
    leases: HashMap<EntityId, Lease>,
    /// Waiters for the next tick, see [`beat`].
    beats: Vec<async_channel::Sender<()>>,
    tick: u64,
    running: bool,
}
impl Global for Clock {}

/// Keeps the view being drawn repainting at `rate`. Call it while rendering
/// or painting a loop, every time the loop is drawn. Returns false, and
/// leases nothing, under reduced motion: draw the loop's resting frame.
#[track_caller]
pub(crate) fn request(rate: Rate, window: &Window, cx: &mut App) -> bool {
    request_inner(rate, Location::caller(), window, cx, None)
}

/// Stream presentation uses the same coalesced 30Hz wakeup as decorative
/// loops, advancing its state before notifying rather than spawning a timer.
#[track_caller]
pub(crate) fn request_frame(
    rate: Rate,
    window: &Window,
    cx: &mut App,
    on_tick: impl FnOnce(&mut App) + 'static,
) -> bool {
    request_inner(
        rate,
        Location::caller(),
        window,
        cx,
        Some(Box::new(on_tick)),
    )
}

fn request_inner(
    rate: Rate,
    owner: &'static Location<'static>,
    window: &Window,
    cx: &mut App,
    mut on_tick: Option<FrameCallback>,
) -> bool {
    if cx.reduce_motion() {
        return false;
    }
    let view = window.current_view();
    let clock = cx.default_global::<Clock>();
    clock
        .leases
        .entry(view)
        .and_modify(|lease| {
            lease.painted = true;
            if !lease.owners.contains(&owner) {
                lease.owners.push(owner);
            }
            if on_tick.is_some() {
                lease.on_tick = on_tick.take();
            }
            if lease.rate.stride() > rate.stride() {
                lease.rate = rate;
            }
        })
        .or_insert_with(|| Lease {
            rate,
            owners: vec![owner],
            painted: true,
            on_tick,
        });
    if !clock.running {
        clock.running = true;
        cx.spawn(async move |cx| {
            loop {
                cx.background_executor().timer(TICK).await;
                if !cx.update(tick) {
                    break;
                }
            }
        })
        .detach();
    }
    true
}

/// Time on the shared clock, so loops of one period stay in step.
pub(crate) fn elapsed(cx: &mut App) -> Duration {
    let now = cx.background_executor().now();
    let clock = cx.default_global::<Clock>();
    now - *clock.epoch.get_or_insert(now)
}

/// The fraction of `period` the shared clock is into, in `0..1`.
pub(crate) fn phase(period: Duration, cx: &mut App) -> f32 {
    if period.is_zero() {
        return 0.;
    }
    let into = elapsed(cx).as_nanos() % period.as_nanos();
    into as f32 / period.as_nanos() as f32
}

#[cfg(test)]
pub(crate) fn running(cx: &App) -> bool {
    cx.try_global::<Clock>().is_some_and(|clock| clock.running)
}

/// Waits for the clock's next tick, or returns at once while no loop runs.
/// Work that would otherwise draw a frame of its own, such as a streamed
/// reply's updates, lands in the loops' frame instead.
pub(crate) async fn beat(cx: &mut gpui::AsyncApp) {
    // Tests park the executor without advancing the clock, so a tick would
    // never come; they apply updates as soon as they arrive.
    if cfg!(test) {
        return;
    }
    let beat = cx.update(|cx| {
        let clock = cx.default_global::<Clock>();
        clock.running.then(|| {
            let (tx, rx) = async_channel::bounded(1);
            clock.beats.push(tx);
            rx
        })
    });
    if let Some(beat) = beat {
        beat.recv().await.ok();
    }
}

fn tick(cx: &mut App) -> bool {
    let background = !app_visible(cx);
    let slowdown = if background { BACKGROUND_STRIDE } else { 1 };
    let clock = cx.global_mut::<Clock>();
    // Before the loops notify, so whatever the waiters change draws with them.
    for beat in clock.beats.drain(..) {
        beat.try_send(()).ok();
    }
    clock.tick = clock.tick.wrapping_add(1);
    let tick = clock.tick;
    let mut due = Vec::new();
    clock.leases.retain(|view, lease| {
        let stride = lease.rate.stride() * slowdown;
        if !tick.is_multiple_of(stride) {
            return true;
        }
        if !lease.painted {
            return false;
        }
        lease.painted = false;
        due.push((*view, lease.on_tick.take()));
        true
    });
    if clock.leases.is_empty() {
        clock.running = false;
        return false;
    }
    for (view, callback) in due {
        if let Some(callback) = callback {
            callback(cx);
        } else {
            // A loop's frames keep its size; views can keep their measurements.
            cx.notify_paint(view);
        }
    }
    true
}

/// Whether one of the app's windows has focus.
pub(crate) fn app_active(cx: &mut App) -> bool {
    cx.windows().into_iter().any(|window| {
        window
            .update(cx, |_, window, _| window.is_window_active())
            .unwrap_or(false)
    })
}

/// Whether any window is on screen: an inactive window can still be in view.
fn app_visible(cx: &mut App) -> bool {
    cx.windows().into_iter().any(|window| {
        window
            .update(cx, |_, window, _| window.is_visible())
            .unwrap_or(false)
    })
}

/// What the clock holds right now: how many views lease it and which call
/// sites asked, most frequent first.
#[derive(Debug, PartialEq, Eq)]
struct Snapshot {
    leases: usize,
    owners: Vec<(String, usize)>,
}

fn snapshot(cx: &App) -> Snapshot {
    let mut counts: HashMap<String, usize> = HashMap::new();
    let mut leases = 0;
    if let Some(clock) = cx.try_global::<Clock>() {
        leases = clock.leases.len();
        for lease in clock.leases.values() {
            for owner in &lease.owners {
                *counts.entry(owner_label(owner)).or_default() += 1;
            }
        }
    }
    let mut owners: Vec<_> = counts.into_iter().collect();
    owners.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    Snapshot { leases, owners }
}

fn owner_label(owner: &Location<'_>) -> String {
    let file = owner.file().replace('\\', "/");
    let file = file.rsplit("/src/").next().unwrap_or(&file);
    let file = file.strip_prefix("src/").unwrap_or(file);
    format!("{file}:{}", owner.line())
}

/// One stats line: `draws` and `vsync` are `None` when nothing counts them.
fn stats_line(
    snapshot: &Snapshot,
    ticks: u64,
    draws: Option<u32>,
    vsync: Option<u64>,
    elapsed: Duration,
) -> String {
    let secs = elapsed.as_secs_f64().max(f64::EPSILON);
    let rate = |count: f64| format!("{:.1}", count / secs);
    let owners = snapshot
        .owners
        .iter()
        .map(|(owner, count)| format!("{owner} x{count}"))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "leases={} owners=[{owners}] ticks/s={} draws/s={}{}",
        snapshot.leases,
        rate(ticks as f64),
        draws.map_or_else(|| "n/a".into(), |draws| rate(f64::from(draws))),
        vsync.map_or_else(String::new, |vsync| format!(
            " vsync/s={}",
            rate(vsync as f64)
        )),
    )
}

/// With `MONOPTAH_PULSE_STATS` set (and not `0`), logs a stats line to the
/// native log every ten seconds: the active leases, the call sites that hold
/// them, and the clock's ticks, the window's draws and (macOS) display link
/// ticks per second. Draws are counted with GPUI's frame stats, which
/// `MONOCODE_FRAME_LOG` reads too, so this line leaves them out when that is set.
/// Nothing runs when the variable is unset.
pub(crate) fn start_stats(cx: &mut App) {
    if !std::env::var_os("MONOPTAH_PULSE_STATS").is_some_and(|value| value != "0") {
        return;
    }
    let count_draws = std::env::var_os("MONOCODE_FRAME_LOG").is_none();
    if count_draws {
        gpui::frame_stats::enable();
    }
    cx.spawn(async move |cx| {
        let mut last = Instant::now();
        let mut last_tick = cx.update(|cx| cx.default_global::<Clock>().tick);
        #[cfg(target_os = "macos")]
        let mut last_vsync = gpui_macos::frame_diagnostics().vsync_ticks;
        loop {
            cx.background_executor().timer(STATS_PERIOD).await;
            let now = Instant::now();
            let (snapshot, tick) =
                cx.update(|cx| (snapshot(cx), cx.default_global::<Clock>().tick));
            let draws = count_draws.then(|| gpui::frame_stats::take().frames);
            #[cfg(target_os = "macos")]
            let vsync = {
                let total = gpui_macos::frame_diagnostics().vsync_ticks;
                let delta = total.wrapping_sub(last_vsync);
                last_vsync = total;
                Some(delta)
            };
            #[cfg(not(target_os = "macos"))]
            let vsync = None;
            let line = stats_line(
                &snapshot,
                tick.wrapping_sub(std::mem::replace(&mut last_tick, tick)),
                draws,
                vsync,
                now.duration_since(std::mem::replace(&mut last, now)),
            );
            log::info!(target: "pulse_stats", "{line}");
        }
    })
    .detach();
}

/// A repeating animation on the shared clock: `build` gets the fraction of
/// `period` elapsed and returns the frame. Reduced motion draws phase zero.
#[track_caller]
pub(crate) fn looping<E: IntoElement>(
    id: impl Into<ElementId>,
    period: Duration,
    rate: Rate,
    build: impl FnOnce(f32) -> E + 'static,
) -> Looping {
    Looping {
        id: id.into(),
        period,
        rate,
        owner: Location::caller(),
        build: Some(Box::new(move |phase| build(phase).into_any_element())),
    }
}

pub(crate) struct Looping {
    id: ElementId,
    period: Duration,
    rate: Rate,
    owner: &'static Location<'static>,
    build: Option<Box<dyn FnOnce(f32) -> AnyElement>>,
}
impl IntoElement for Looping {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}
impl Element for Looping {
    type RequestLayoutState = AnyElement;
    type PrepaintState = ();
    fn id(&self) -> Option<ElementId> {
        Some(self.id.clone())
    }
    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }
    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, AnyElement) {
        let phase = if request_inner(self.rate, self.owner, window, cx, None) {
            phase(self.period, cx)
        } else {
            0.
        };
        let build = self.build.take().expect("a loop lays out once");
        let mut element = build(phase);
        (element.request_layout(window, cx), element)
    }
    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: gpui::Bounds<Pixels>,
        element: &mut AnyElement,
        window: &mut Window,
        cx: &mut App,
    ) {
        element.prepaint(window, cx);
    }
    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: gpui::Bounds<Pixels>,
        element: &mut AnyElement,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        element.paint(window, cx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{Context, Render, TestAppContext, div, prelude::*};
    use std::{cell::Cell, rc::Rc};

    struct Spinner {
        spinning: bool,
        renders: Rc<Cell<usize>>,
    }
    impl Render for Spinner {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            self.renders.set(self.renders.get() + 1);
            let spinning = self.spinning;
            div().size_4().when(spinning, |d| {
                d.child(looping(
                    "spin",
                    Duration::from_secs(1),
                    Rate::Smooth,
                    |_| div(),
                ))
            })
        }
    }

    #[gpui::test]
    fn a_loop_repaints_on_the_clock_and_parks_once_it_is_gone(cx: &mut TestAppContext) {
        cx.update(|cx| cx.set_reduce_motion(false));
        let renders = Rc::new(Cell::new(0));
        let (view, cx) = cx.add_window_view(|_, _| Spinner {
            spinning: true,
            renders: renders.clone(),
        });
        cx.run_until_parked();
        let first = renders.get();
        cx.executor().advance_clock(TICK * 2);
        cx.run_until_parked();
        assert!(renders.get() > first, "a leased view repaints on ticks");

        view.update(cx, |view, cx| {
            view.spinning = false;
            cx.notify();
        });
        cx.run_until_parked();
        cx.executor()
            .advance_clock(TICK * BACKGROUND_STRIDE as u32 * 3);
        cx.run_until_parked();
        let stopped = renders.get();
        cx.executor()
            .advance_clock(TICK * BACKGROUND_STRIDE as u32 * 3);
        cx.run_until_parked();
        assert_eq!(
            renders.get(),
            stopped,
            "an unpainted loop releases the clock"
        );
        cx.update(|_, cx| assert!(!cx.global::<Clock>().running));
    }

    #[gpui::test]
    fn a_visible_window_keeps_full_pace_and_a_hidden_one_slows(cx: &mut TestAppContext) {
        cx.update(|cx| cx.set_reduce_motion(false));
        let renders = Rc::new(Cell::new(0));
        let (_view, cx) = cx.add_window_view(|_, _| Spinner {
            spinning: true,
            renders: renders.clone(),
        });
        cx.run_until_parked();
        // A test window is visible but never active, like one behind another app.
        let start = renders.get();
        cx.executor().advance_clock(TICK * BACKGROUND_STRIDE as u32);
        cx.run_until_parked();
        assert_eq!(renders.get() - start, BACKGROUND_STRIDE as usize);

        cx.simulate_visibility_change(gpui::WindowVisibility::Hidden);
        cx.run_until_parked();
        let hidden = renders.get();
        cx.executor()
            .advance_clock(TICK * BACKGROUND_STRIDE as u32 * 2);
        cx.run_until_parked();
        assert!(
            renders.get() - hidden <= 2,
            "a hidden app's loops slow down"
        );
    }

    #[gpui::test]
    fn leases_are_counted_by_owner_and_released_with_the_view(cx: &mut TestAppContext) {
        cx.update(|cx| cx.set_reduce_motion(false));
        let renders = Rc::new(Cell::new(0));
        let (view, cx) = cx.add_window_view(|_, _| Spinner {
            spinning: true,
            renders: renders.clone(),
        });
        cx.run_until_parked();
        let held = cx.update(|_, cx| snapshot(cx));
        assert_eq!(held.leases, 1);
        assert_eq!(held.owners.len(), 1);
        let (owner, count) = &held.owners[0];
        assert!(
            owner.starts_with("pulse.rs:"),
            "the owner is the `looping` call site, got {owner}"
        );
        assert_eq!(*count, 1);

        view.update(cx, |view, cx| {
            view.spinning = false;
            cx.notify();
        });
        cx.run_until_parked();
        cx.executor().advance_clock(TICK * 3);
        cx.run_until_parked();
        let released = cx.update(|_, cx| snapshot(cx));
        assert_eq!(
            released,
            Snapshot {
                leases: 0,
                owners: Vec::new()
            }
        );
    }

    #[test]
    fn the_stats_line_names_the_leases_and_rates() {
        let snapshot = Snapshot {
            leases: 3,
            owners: vec![("activity.rs:57".into(), 2), ("avatar.rs:58".into(), 1)],
        };
        assert_eq!(
            stats_line(&snapshot, 150, Some(100), None, Duration::from_secs(10)),
            "leases=3 owners=[activity.rs:57 x2, avatar.rs:58 x1] ticks/s=15.0 draws/s=10.0"
        );
        assert_eq!(
            stats_line(
                &Snapshot {
                    leases: 0,
                    owners: Vec::new()
                },
                0,
                None,
                Some(1200),
                Duration::from_secs(10)
            ),
            "leases=0 owners=[] ticks/s=0.0 draws/s=n/a vsync/s=120.0"
        );
    }

    #[gpui::test]
    fn reduced_motion_leases_nothing(cx: &mut TestAppContext) {
        cx.update(|cx| cx.set_reduce_motion(true));
        let renders = Rc::new(Cell::new(0));
        let (_view, cx) = cx.add_window_view(|_, _| Spinner {
            spinning: true,
            renders: renders.clone(),
        });
        cx.run_until_parked();
        let first = renders.get();
        cx.executor()
            .advance_clock(TICK * BACKGROUND_STRIDE as u32 * 3);
        cx.run_until_parked();
        assert_eq!(renders.get(), first);
    }

    #[gpui::test]
    fn stream_callbacks_share_the_clock_and_release_their_lease(cx: &mut TestAppContext) {
        struct Stream {
            active: bool,
            ticks: Rc<Cell<usize>>,
        }
        impl Render for Stream {
            fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
                let active = self.active;
                let view = cx.entity().downgrade();
                div().size_4().child(gpui::canvas(
                    move |_, window, cx| {
                        if active {
                            request_frame(Rate::Smooth, window, cx, move |cx| {
                                let _ = view.update(cx, |stream, cx| {
                                    if stream.active {
                                        stream.ticks.set(stream.ticks.get() + 1);
                                        cx.notify();
                                    }
                                });
                            });
                        }
                    },
                    |_, _, _, _| {},
                ))
            }
        }
        cx.update(|cx| cx.set_reduce_motion(false));
        let ticks = Rc::new(Cell::new(0));
        let (view, cx) = cx.add_window_view(|_, _| Stream {
            active: true,
            ticks: ticks.clone(),
        });
        cx.run_until_parked();
        cx.executor().advance_clock(TICK);
        cx.run_until_parked();
        assert_eq!(ticks.get(), 1);
        view.update(cx, |stream, cx| {
            stream.active = false;
            cx.notify();
        });
        cx.run_until_parked();
        cx.executor()
            .advance_clock(TICK * BACKGROUND_STRIDE as u32 * 3);
        cx.run_until_parked();
        assert_eq!(ticks.get(), 1);
        cx.update(|_, cx| assert!(!running(cx)));
    }
}
