// Anchored popovers and menus, ported from src/shared/ui/Popover.tsx, its
// placement math in src/shared/lib/popover.ts, and the row/keyboard model of
// src/features/files/ui/ExplorerMenu.tsx.
use crate::{input::Input, theme::*};
use gpui::{
    AnyElement, AnyView, App, Bounds, BoxShadow, Context, Corners, DispatchPhase, Display, Element,
    ElementId, Entity, EventEmitter, FocusHandle, Focusable, GlobalElementId, Hsla,
    InspectorElementId, KeyBinding, LayoutId, MouseDownEvent, Pixels, Point,
    Position as CssPosition, Role, SharedString, Style, Subscription, Toggled, Window, actions,
    canvas, deferred, div, point, prelude::*, px, rgb, rgba, size,
};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    time::Instant,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Side {
    Top,
    Bottom,
    Left,
    Right,
}
impl Side {
    fn opposite(self) -> Self {
        match self {
            Self::Top => Self::Bottom,
            Self::Bottom => Self::Top,
            Self::Left => Self::Right,
            Self::Right => Self::Left,
        }
    }
    fn vertical(self) -> bool {
        matches!(self, Self::Top | Self::Bottom)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Align {
    Start,
    Center,
    End,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Options {
    /// Preferred side of the anchor; flips only when the space runs out.
    pub side: Side,
    pub align: Align,
    /// Space between anchor and popover. Negative overlaps, as flyouts like to.
    pub gap: f32,
    /// Breathing room kept against every viewport edge.
    pub padding: f32,
    pub width: Option<f32>,
    /// Height the popover wants before it is worth flipping to the other side.
    pub min_height: Option<f32>,
    pub max_height: Option<f32>,
}
impl Default for Options {
    fn default() -> Self {
        Self {
            side: Side::Bottom,
            align: Align::Start,
            gap: 6.,
            padding: 8.,
            width: None,
            min_height: None,
            max_height: None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Rect {
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
}
impl Rect {
    fn from_bounds(b: Bounds<Pixels>) -> Self {
        Self {
            left: f32::from(b.left()),
            top: f32::from(b.top()),
            right: f32::from(b.right()),
            bottom: f32::from(b.bottom()),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Placement {
    pub side: Side,
    pub left: f32,
    /// Set for every side except `Top`, which is pinned by its bottom edge.
    pub top: Option<f32>,
    /// Pinning a top-side popover here keeps it still while its content grows.
    pub bottom: Option<f32>,
    pub width: f32,
    pub max_height: f32,
}

fn clamp(value: f32, min: f32, max: f32) -> f32 {
    value.max(min).min(max.max(min))
}

fn room_on(side: Side, a: &Rect, viewport: (f32, f32), gap: f32, padding: f32) -> f32 {
    match side {
        Side::Top => a.top - gap - padding,
        Side::Bottom => viewport.1 - a.bottom - gap - padding,
        Side::Left => a.left - gap - padding,
        Side::Right => viewport.0 - a.right - gap - padding,
    }
}

fn cross_start(align: Align, start: f32, end: f32, length: f32) -> f32 {
    match align {
        Align::Start => start,
        Align::End => end - length,
        Align::Center => start + (end - start) / 2. - length / 2.,
    }
}

/// `placePopover`: prefer `options.side`, flip when it can't hold the
/// popover and the other side has more room, then clamp into the viewport.
pub(crate) fn place(
    anchor: Rect,
    popover: (f32, f32),
    viewport: (f32, f32),
    o: &Options,
) -> Placement {
    let (vw, vh) = viewport;
    let max = o.max_height.unwrap_or(f32::INFINITY);
    let width = o
        .width
        .unwrap_or(popover.0)
        .min((vw - o.padding * 2.).max(0.));
    let wanted = max.min(popover.1.max(o.min_height.unwrap_or(0.)));
    let needed = if o.side.vertical() { wanted } else { width };
    let room = room_on(o.side, &anchor, viewport, o.gap, o.padding);
    let flipped = room_on(o.side.opposite(), &anchor, viewport, o.gap, o.padding);
    let side = if room >= needed || flipped <= room {
        o.side
    } else {
        o.side.opposite()
    };

    if side.vertical() {
        let left = clamp(
            cross_start(o.align, anchor.left, anchor.right, width),
            o.padding,
            vw - width - o.padding,
        );
        // A minimum height can outgrow the room beside the anchor, but never
        // the viewport: past that the overflow would be unreachable.
        let available = room_on(side, &anchor, viewport, o.gap, o.padding)
            .max(o.min_height.unwrap_or(0.))
            .min((vh - o.padding * 2.).max(0.));
        let max_height = max.min(available);
        if side == Side::Bottom {
            let top = (anchor.bottom + o.gap).min(vh - o.padding - max_height);
            return Placement {
                side,
                left,
                top: Some(top.max(o.padding)),
                bottom: None,
                width,
                max_height,
            };
        }
        let bottom = (vh - anchor.top + o.gap).min(vh - o.padding - max_height);
        return Placement {
            side,
            left,
            top: None,
            bottom: Some(bottom.max(o.padding)),
            width,
            max_height,
        };
    }

    let capped = max.min((vh - o.padding * 2.).max(0.));
    let height = popover.1.min(capped);
    let top = clamp(
        cross_start(o.align, anchor.top, anchor.bottom, height),
        o.padding,
        vh - height - o.padding,
    );
    let raw_left = if side == Side::Right {
        anchor.right + o.gap
    } else {
        anchor.left - o.gap - width
    };
    Placement {
        side,
        left: clamp(raw_left, o.padding, vw - width - o.padding),
        top: Some(top),
        bottom: None,
        width,
        max_height: capped.min(vh - top - o.padding),
    }
}

/// Window bounds of a trigger, written while it is laid out so a popover can
/// open against it. Put [`AnchorSlot::probe`] inside the trigger.
#[derive(Clone, Default)]
pub(crate) struct AnchorSlot(Rc<Cell<Option<Bounds<Pixels>>>>);
impl AnchorSlot {
    pub(crate) fn get(&self) -> Option<Bounds<Pixels>> {
        self.0.get()
    }
    pub(crate) fn set(&self, bounds: Option<Bounds<Pixels>>) {
        self.0.set(bounds)
    }
    pub(crate) fn probe(&self) -> impl IntoElement {
        let slot = self.clone();
        // Pinned to the corner: an absolute child left at its static
        // position starts inside the trigger's padding, which would shift
        // every popover by it.
        canvas(move |bounds, _, _| slot.set(Some(bounds)), |_, _, _, _| {})
            .absolute()
            .top_0()
            .left_0()
            .size_full()
    }
    /// A probe for sizes the next render lays out from: a change of size
    /// asks for another frame, so nothing waits on unrelated input to catch up.
    pub(crate) fn layout_probe(&self) -> impl IntoElement {
        let slot = self.clone();
        canvas(
            move |bounds, window, _| {
                let changed = slot.get().is_none_or(|old| old.size != bounds.size);
                slot.set(Some(bounds));
                if changed {
                    window.refresh();
                }
            },
            |_, _, _, _| {},
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full()
    }
}

/// A trigger, or a bare point for context menus.
#[derive(Clone)]
pub(crate) enum Anchor {
    Slot(AnchorSlot),
    Point(Point<Pixels>),
    /// Centre the measured card in the window, for modals.
    Center,
    /// Follow the row vertically, but always measure the gap from the parent
    /// card's edge, regardless of its row padding or menu style.
    Submenu {
        row: AnchorSlot,
        surface: AnchorSlot,
    },
}
impl Anchor {
    fn bounds(&self) -> Bounds<Pixels> {
        match self {
            Self::Slot(slot) => slot.get().unwrap_or_default(),
            Self::Point(p) => Bounds::new(*p, size(px(0.), px(0.))),
            Self::Center => Bounds::default(),
            Self::Submenu { row, surface } => {
                let row = row.get().unwrap_or_default();
                let surface = surface.get().unwrap_or(row);
                Bounds::new(
                    point(surface.left(), row.top()),
                    size(surface.size.width, row.size.height),
                )
            }
        }
    }
}

type Dismiss = Rc<dyn Fn(&mut Window, &mut App)>;

// Popover.tsx / .popover-open: 170ms, cubic-bezier(.16, 1, .3, 1),
// fading and travelling 8px from the resolved anchor side. The whole card
// moves together so content cannot slide inside its frame. Dismissal is immediate.
const MENU_IN: f32 = 0.17;
const MENU_TRAVEL: f32 = 8.;
const SUBMENU_GAP: f32 = 4.;
const RADIUS: f32 = 12.;
// GlassBackdrop's backdrop-blur-xl.
#[cfg(any(target_os = "macos", target_os = "windows"))]
const BLUR: f32 = 24.;

fn entrance(elapsed: f32, reduced: bool) -> f32 {
    if reduced {
        1.
    } else {
        crate::reveal::bezier((elapsed / MENU_IN).clamp(0., 1.), 0.16, 1., 0.3, 1.)
    }
}
fn entrance_offset(side: Side, progress: f32) -> Point<Pixels> {
    let remaining = px(MENU_TRAVEL * (1. - progress));
    match side {
        Side::Top => point(px(0.), remaining),
        Side::Bottom => point(px(0.), -remaining),
        Side::Left => point(remaining, px(0.)),
        Side::Right => point(-remaining, px(0.)),
    }
}

/// A menu, dropdown or flyout drawn above everything else in the window,
/// placed against its anchor with viewport flipping (Popover.tsx's
/// placement). The glass frame, 10% border and shadow follow the React
/// component; the whole card fades and slides from the anchored edge.
/// Pressing outside it (and outside its anchor)
/// dismisses it, and that press reaches nothing else.
#[derive(IntoElement)]
pub(crate) struct Popover {
    id: ElementId,
    anchor: Anchor,
    options: Options,
    padding: f32,
    priority: usize,
    track: Vec<AnchorSlot>,
    ignore: Vec<AnchorSlot>,
    on_dismiss: Option<Dismiss>,
    consume_outside_press: bool,
    modal_classic: Option<bool>,
    animate: bool,
    /// Without a fixed width, the card hugs its content between these.
    fit: (Option<f32>, Option<f32>),
    children: Vec<AnyElement>,
}
pub(crate) fn popover(id: impl Into<ElementId>, anchor: Anchor) -> Popover {
    Popover {
        id: id.into(),
        anchor,
        options: Options::default(),
        padding: 0.,
        priority: DEFAULT_PRIORITY,
        track: Vec::new(),
        ignore: Vec::new(),
        on_dismiss: None,
        consume_outside_press: true,
        modal_classic: None,
        animate: true,
        fit: (None, None),
        children: Vec::new(),
    }
}
impl Popover {
    /// Dismiss without swallowing the next click. Text-selection toolbars and
    /// link menus let that press select text or open another link's menu.
    pub(crate) fn allow_outside_press(mut self) -> Self {
        self.consume_outside_press = false;
        self
    }
    pub(crate) fn without_animation(mut self) -> Self {
        self.animate = false;
        self
    }
    /// The standard modal card ([`crate::modal`]): 16px corners, the 10%
    /// rule and the dialog tint.
    pub(crate) fn modal_surface(mut self, classic: bool) -> Self {
        self.modal_classic = Some(classic);
        self
    }
    pub(crate) fn align(mut self, align: Align) -> Self {
        self.options.align = align;
        self
    }
    pub(crate) fn side(mut self, side: Side) -> Self {
        self.options.side = side;
        self
    }
    pub(crate) fn gap(mut self, gap: f32) -> Self {
        self.options.gap = gap;
        self
    }
    pub(crate) fn width(mut self, width: f32) -> Self {
        self.options.width = Some(width);
        self
    }
    /// Sizes the card to its content, wrapping text past `max` instead of
    /// leaving a fixed width's empty space beside short content.
    pub(crate) fn max_width(mut self, max: f32) -> Self {
        self.options.width = None;
        self.fit.1 = Some(max);
        self
    }
    pub(crate) fn min_width(mut self, min: f32) -> Self {
        self.fit.0 = Some(min);
        self
    }
    /// A fixed height, such as the model flyout's: `minHeight = maxHeight`.
    pub(crate) fn height(mut self, height: f32) -> Self {
        self.options.min_height = Some(height);
        self.options.max_height = Some(height);
        self
    }
    pub(crate) fn max_height(mut self, height: f32) -> Self {
        self.options.max_height = Some(height);
        self
    }
    /// The content's own padding, the caller's `className="p-…"`.
    pub(crate) fn padding(mut self, padding: f32) -> Self {
        self.padding = padding;
        self
    }
    pub(crate) fn priority(mut self, priority: usize) -> Self {
        self.priority = priority;
        self
    }
    /// Records the surface's bounds, so another popover can ignore them.
    pub(crate) fn track(mut self, slot: AnchorSlot) -> Self {
        self.track.push(slot);
        self
    }
    /// A press inside this region is not "outside", like a submenu's surface.
    pub(crate) fn ignore(mut self, slot: AnchorSlot) -> Self {
        self.ignore.push(slot);
        self
    }
    pub(crate) fn on_dismiss(mut self, f: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_dismiss = Some(Rc::new(f));
        self
    }
}
impl ParentElement for Popover {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements)
    }
}
impl RenderOnce for Popover {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        // Keyed like the surface, so it lives exactly as long as the popover
        // stays rendered and the entrance replays on every open.
        let opened = window.use_keyed_state(self.id.clone(), cx, |_, _| Instant::now());
        let enter = if self.animate {
            entrance(opened.read(cx).elapsed().as_secs_f32(), cx.reduce_motion())
        } else {
            1.
        };
        if enter < 1. {
            window.request_animation_frame();
        }
        self.render_at(enter, window.viewport_size())
    }
}
impl Popover {
    fn render_at(self, enter: f32, viewport: gpui::Size<Pixels>) -> impl IntoElement {
        // Built inside another popover's content: draw above it, whatever priority was asked.
        let priority = HOST_PRIORITY
            .get()
            .map_or(self.priority, |host| self.priority.max(host + 1));
        let (viewport_width, viewport_height) = (viewport.width, viewport.height);
        let radius = if self.modal_classic.is_some() {
            16.
        } else {
            RADIUS
        };
        let border = if self.modal_classic.is_some() {
            crate::modal::BORDER
        } else {
            0.1
        };
        let tint = if self.modal_classic.is_some() {
            let mut fill = crate::theme::pane_fill();
            fill.a = 0.55;
            fill
        } else {
            tint()
        };
        let viewport = f32::from(viewport_height) - self.options.padding * 2.;
        let max_height = self
            .options
            .max_height
            .unwrap_or(f32::INFINITY)
            .min(viewport);
        let frame = div()
            .id(self.id)
            .occlude()
            .relative()
            .flex()
            .flex_col()
            .overflow_hidden()
            .max_h(px(max_height))
            .when_some(self.options.min_height, |d, h| d.h(px(h.min(max_height))))
            .when_some(self.options.width, |d, w| d.w(px(w)))
            .when_some(self.fit.0, |d, w| d.min_w(px(w)))
            .when_some(self.fit.1, |d, w| {
                d.max_w(px(
                    w.min(f32::from(viewport_width) - self.options.padding * 2.)
                ))
            })
            .rounded(px(radius))
            .opacity(enter)
            .shadow(shadow())
            .child(glass(tint, radius))
            .child(
                div()
                    .relative()
                    .m(px(1.))
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .p(px(self.padding))
                    .children(self.children),
            )
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .rounded(px(radius))
                    .border_1()
                    .border_color(content(border)),
            );
        deferred(Placed {
            priority,
            anchor: self.anchor,
            options: self.options,
            progress: enter,
            frame: frame.into_any_element(),
            track: self.track,
            ignore: self.ignore,
            on_dismiss: self.on_dismiss,
            consume_outside_press: self.consume_outside_press,
        })
        .with_priority(priority)
    }
}
fn tint() -> Hsla {
    content(0.02)
}
// On macOS the renderer blurs what is already drawn behind this quad.
// Light themes, and platforms without the blur, paint an opaque card.
fn glass(tint: Hsla, radius: f32) -> impl IntoElement {
    canvas(
        |_, _, _| {},
        move |bounds, _, window, _| paint_glass(bounds, Corners::all(px(radius)), tint, window),
    )
    .absolute()
    .size_full()
}
/// The popover surface: `tint` over a backdrop blur, or the opaque card.
pub(crate) fn paint_glass(
    bounds: Bounds<Pixels>,
    corners: Corners<Pixels>,
    tint: Hsla,
    window: &mut Window,
) {
    let opaque = crate::theme::floating_fill();
    let light = crate::theme::is_light();
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
    window.paint_quad(quad.corner_radii(corners));
}
/// A whole popover surface painted directly, for cards drawn inside a
/// canvas such as chart readouts: shadow, glass (or the light theme's opaque
/// card) and the 10% border, at the popover's radius.
pub(crate) fn paint_surface(bounds: Bounds<Pixels>, window: &mut Window) {
    let corners = Corners::all(px(RADIUS));
    window.paint_drop_shadows(bounds, corners, &shadow());
    paint_glass(bounds, corners, tint(), window);
    window.paint_quad(
        gpui::fill(bounds, gpui::transparent_black())
            .corner_radii(corners)
            .border_widths(px(1.))
            .border_color(content(0.1)),
    );
}
/// The default popover glass tint.
pub(crate) fn glass_tint() -> Hsla {
    tint()
}
/// The popover's two-layer drop shadow.
pub(crate) fn shadow() -> Vec<BoxShadow> {
    vec![
        BoxShadow::new(px(0.), px(20.), rgba(0x0000001a).into())
            .blur_radius(px(25.))
            .spread_radius(px(-5.)),
        BoxShadow::new(px(0.), px(8.), rgba(0x0000001a).into())
            .blur_radius(px(10.))
            .spread_radius(px(-6.)),
    ]
}

/// A popover's card as last painted: which window it is in, how high it is drawn and where.
struct Layer {
    window: gpui::WindowId,
    priority: usize,
    surface: Bounds<Pixels>,
}
thread_local! {
    /// Every popover painted for the frames still alive. A layer is held strongly only by its
    /// popover's mouse listener, so it lives exactly as long as the frame that painted it: the
    /// window drops the listeners of a frame once the next one is drawn.
    static LAYERS: RefCell<Vec<std::rc::Weak<Layer>>> = const { RefCell::new(Vec::new()) };
}
/// Whether `position` is on the card of a popover drawn above `priority` in `window`. A press
/// there belongs to that popover, so the ones beneath it, a modal for one, must not take it for
/// an outside press and dismiss themselves before the item under the pointer is clicked.
fn on_higher_layer(window: gpui::WindowId, priority: usize, position: &Point<Pixels>) -> bool {
    LAYERS.with_borrow(|layers| {
        layers
            .iter()
            .filter_map(std::rc::Weak::upgrade)
            .any(|layer| {
                layer.window == window
                    && layer.priority > priority
                    && layer.surface.contains(position)
            })
    })
}

thread_local! {
    /// The priority of the popover whose content is being laid out, if any. A popover built
    /// inside it (a submenu, a flyout, a dropdown of a dialog's own widget) is raised above it,
    /// so it is never hidden beneath its host and a press on it is never an outside press for it.
    static HOST_PRIORITY: Cell<Option<usize>> = const { Cell::new(None) };
}
/// Where a popover draws unless its caller says otherwise.
const DEFAULT_PRIORITY: usize = 40;
/// Marks `priority` as the host layer until dropped, restoring the previous one.
struct HostLayer(Option<usize>);
impl HostLayer {
    fn enter(priority: usize) -> Self {
        Self(HOST_PRIORITY.replace(Some(priority)))
    }
}
impl Drop for HostLayer {
    fn drop(&mut self) {
        HOST_PRIORITY.set(self.0);
    }
}

struct Placed {
    priority: usize,
    anchor: Anchor,
    options: Options,
    progress: f32,
    frame: AnyElement,
    track: Vec<AnchorSlot>,
    ignore: Vec<AnchorSlot>,
    on_dismiss: Option<Dismiss>,
    consume_outside_press: bool,
}
impl IntoElement for Placed {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}
impl Element for Placed {
    type RequestLayoutState = LayoutId;
    type PrepaintState = Bounds<Pixels>;
    fn id(&self) -> Option<ElementId> {
        None
    }
    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }
    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, LayoutId) {
        // The content renders here, so a menu in it learns the layer it is drawn on.
        let child = {
            let _host = HostLayer::enter(self.priority);
            self.frame.request_layout(window, cx)
        };
        let style = Style {
            position: CssPosition::Absolute,
            display: Display::Flex,
            ..Style::default()
        };
        (window.request_layout(style, [child], cx), child)
    }
    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        child: &mut LayoutId,
        window: &mut Window,
        cx: &mut App,
    ) -> Bounds<Pixels> {
        let surface = window.layout_bounds(*child).size;
        let viewport = window.viewport_size();
        let viewport = (f32::from(viewport.width), f32::from(viewport.height));
        let height = f32::from(surface.height);
        let placed = place(
            Rect::from_bounds(self.anchor.bounds()),
            (f32::from(surface.width), height),
            viewport,
            &self.options,
        );
        let top = placed.top.unwrap_or_else(|| {
            viewport.1 - placed.bottom.unwrap_or(0.) - height.min(placed.max_height)
        });
        let target = if matches!(self.anchor, Anchor::Center) {
            point(
                px(((viewport.0 - f32::from(surface.width)) / 2.).max(0.)).round(),
                px(((viewport.1 - height) / 2.).max(0.)).round(),
            )
        } else {
            point(px(placed.left).round(), px(top).round())
        };
        let moving = target + entrance_offset(placed.side, self.progress);
        // One offset moves the glass, frame, content, clipping and hitboxes.
        window.with_element_offset(moving - bounds.origin, |window| {
            self.frame.prepaint(window, cx)
        });
        // Dismissal and child menus follow the visible card during its entrance.
        let placed = Bounds::new(moving, surface);
        for slot in &self.track {
            slot.set(Some(placed));
        }
        placed
    }
    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut LayoutId,
        surface: &mut Bounds<Pixels>,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.frame.paint(window, cx);
        let layer = Rc::new(Layer {
            window: window.window_handle().window_id(),
            priority: self.priority,
            surface: *surface,
        });
        LAYERS.with_borrow_mut(|layers| {
            layers.retain(|layer| layer.strong_count() > 0);
            layers.push(Rc::downgrade(&layer));
        });
        let dismiss = self.on_dismiss.clone();
        let surface = *surface;
        let anchor = self.anchor.bounds();
        let ignore = self.ignore.clone();
        let consume = self.consume_outside_press;
        let outside = move |position: &Point<Pixels>| {
            !surface.contains(position)
                && !anchor.contains(position)
                && !ignore
                    .iter()
                    .any(|slot| slot.get().is_some_and(|b| b.contains(position)))
                && !on_higher_layer(layer.window, layer.priority, position)
        };
        // Capture, so a trigger beneath that stops propagation still closes
        // the popover, and before that trigger reacts to the same press.
        // Then consume that press in bubble so the
        // content behind cannot act on it too; the next press reaches it.
        // Registered even without a dismissal: it keeps the layer alive.
        window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx| {
            let Some(dismiss) = &dismiss else {
                return;
            };
            if !outside(&event.position) {
                return;
            }
            match phase {
                DispatchPhase::Capture => dismiss(window, cx),
                DispatchPhase::Bubble if consume => cx.stop_propagation(),
                DispatchPhase::Bubble => {}
            }
        });
    }
}

actions!(
    popover_menu,
    [
        SelectPrevious,
        SelectNext,
        Confirm,
        Cancel,
        OpenSubmenu,
        CloseSubmenu
    ]
);
pub(crate) fn bind(cx: &mut App) {
    let context = Some("PopoverMenu");
    cx.bind_keys([
        KeyBinding::new("up", SelectPrevious, context),
        KeyBinding::new("down", SelectNext, context),
        KeyBinding::new("enter", Confirm, context),
        KeyBinding::new("escape", Cancel, context),
        KeyBinding::new("right", OpenSubmenu, context),
        KeyBinding::new("left", CloseSubmenu, context),
    ]);
}

/// Which React menu a [`Menu`] draws like. Each has its own row metrics.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum MenuStyle {
    /// ExplorerMenu: tab and session context menus, and every submenu.
    Explorer,
    /// TabGroupMenu: the project context menu. Icons at 55%, hover only.
    Group,
    /// SidebarWorktreeSwitcher: the sidebar project header's menu.
    Switcher,
    /// The composer's plus menu: an icon, title and one-line hint per row.
    Composer,
    /// AccessPicker: like Composer, with a medium title, wrapping hint and
    /// the selected row filled instead of checked.
    Access,
    /// ModelPicker's main menu: a setting per row, its value on the right.
    Settings,
    /// GitChangesPanel's branch and commit popovers: `text-[12px]` rows at
    /// a 10% hover, disabled rows at 40%.
    Git,
    /// SessionFiltersMenu's 228px card and compact checkbox rows.
    Filter,
    /// ModelPicker's recent models: 40px rows of harness icon, model name
    /// over its harness, and a muted check on the current one.
    Recent,
}
impl MenuStyle {
    fn padding(self) -> f32 {
        match self {
            Self::Group => 8.,
            Self::Composer => 6.,
            _ => 4.,
        }
    }
    /// Rows follow the keyboard highlight; the rest only react to hover.
    fn highlights(self) -> bool {
        matches!(
            self,
            Self::Explorer | Self::Access | Self::Settings | Self::Filter | Self::Recent
        )
    }
}

/// Where a row's flyout comes from: a whole view, like the model list.
#[derive(Clone)]
pub(crate) struct Flyout {
    pub view: AnyView,
    pub width: f32,
    /// Card inset for content that does not provide its own padding.
    pub padding: f32,
    pub height: Option<f32>,
    pub max_height: Option<f32>,
    /// Focused when the flyout opens, such as its search field.
    pub focus: Option<FocusHandle>,
    /// A nested Menu uses its host card as the parent of further submenus.
    pub surface: Option<AnchorSlot>,
}

#[derive(Clone, Default)]
pub(crate) struct MenuAction {
    pub id: SharedString,
    pub label: SharedString,
    pub description: Option<SharedString>,
    pub shortcut: Option<SharedString>,
    pub icon: Option<&'static str>,
    pub icon_artwork: Option<&'static str>,
    pub image: Option<&'static str>,
    pub icon_color: Option<Hsla>,
    /// A provider logo shown with `value`, as the model row's harness icon.
    pub value_image: Option<&'static str>,
    pub value: Option<SharedString>,
    pub badge: Option<SharedString>,
    pub disabled: bool,
    pub danger: bool,
    pub checked: bool,
    /// A toggle row uses a switch instead of the selected-item checkmark.
    pub switch: bool,
    /// Checkbox-style actions can update their owner without dismissing the menu.
    pub keep_open: bool,
    pub submenu: Vec<MenuAction>,
    pub flyout: Option<Flyout>,
}
impl MenuAction {
    pub(crate) fn new(id: impl Into<SharedString>, label: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            ..Self::default()
        }
    }
    pub(crate) fn description(mut self, text: impl Into<SharedString>) -> Self {
        self.description = Some(text.into());
        self
    }
    pub(crate) fn shortcut(mut self, text: impl Into<SharedString>) -> Self {
        self.shortcut = Some(text.into());
        self
    }
    pub(crate) fn icon(mut self, path: &'static str) -> Self {
        self.icon = Some(path);
        self
    }
    pub(crate) fn icon_artwork(mut self, path: &'static str) -> Self {
        self.icon = None;
        self.icon_artwork = Some(path);
        self
    }
    pub(crate) fn image(mut self, provider: &'static str) -> Self {
        self.image = Some(provider);
        self
    }
    pub(crate) fn icon_color(mut self, color: Hsla) -> Self {
        self.icon_color = Some(color);
        self
    }
    pub(crate) fn value(mut self, text: impl Into<SharedString>) -> Self {
        self.value = Some(text.into());
        self
    }
    pub(crate) fn value_image(mut self, provider: &'static str) -> Self {
        self.value_image = Some(provider);
        self
    }
    pub(crate) fn badge(mut self, text: impl Into<SharedString>) -> Self {
        self.badge = Some(text.into());
        self
    }
    pub(crate) fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }
    pub(crate) fn danger(mut self) -> Self {
        self.danger = true;
        self
    }
    pub(crate) fn checked(mut self, checked: bool) -> Self {
        self.checked = checked;
        self
    }
    pub(crate) fn toggle(mut self, on: bool) -> Self {
        self.checked = on;
        self.switch = true;
        self.keep_open = true;
        self
    }
    pub(crate) fn keep_open(mut self) -> Self {
        self.keep_open = true;
        self
    }
    pub(crate) fn submenu(mut self, items: Vec<MenuAction>) -> Self {
        self.submenu = items;
        self
    }
    pub(crate) fn flyout(mut self, flyout: Flyout) -> Self {
        self.flyout = Some(flyout);
        self
    }
    fn opens(&self) -> bool {
        !self.disabled && (!self.submenu.is_empty() || self.flyout.is_some())
    }
}

#[derive(Clone)]
pub(crate) enum MenuItem {
    /// Interactive appearance controls above a project's action rows.
    Custom(AnyView, FocusHandle),
    /// A quiet uppercase caption, like the composer's "Add to message".
    Header(SharedString),
    Separator,
    Action(Box<MenuAction>),
    /// A footnote paragraph, like AccessPicker's busy note.
    Note(SharedString),
}
impl From<MenuAction> for MenuItem {
    fn from(action: MenuAction) -> Self {
        Self::Action(Box::new(action))
    }
}

/// The search field a searchable menu shows as its first row: render it as
/// `MenuItem::Custom` (see [`MenuSearch::item`]), build the menu with
/// `Menu::search_navigation()` and `replace_items` on its `input::Changed`.
pub(crate) struct MenuSearch {
    pub(crate) input: Entity<Input>,
}
impl MenuSearch {
    pub(crate) fn new(placeholder: &'static str, cx: &mut App) -> Entity<Self> {
        let input = cx.new(|cx| Input::field(placeholder, placeholder, content(0.35), cx));
        cx.new(|_| Self { input })
    }
    /// What is typed, as typed.
    pub(crate) fn query(&self, cx: &App) -> String {
        self.input.read(cx).content.to_string()
    }
    /// The menu row holding the field.
    pub(crate) fn item(this: &Entity<Self>, cx: &App) -> MenuItem {
        MenuItem::Custom(
            this.clone().into(),
            this.read(cx).input.read(cx).focus_handle(cx),
        )
    }
}
impl Render for MenuSearch {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .h(px(44.))
            .mx(px(-4.))
            .mt(px(-4.))
            .mb(px(4.))
            .px(px(12.))
            .border_b_1()
            .border_color(content(0.1))
            .flex()
            .items_center()
            .gap(px(10.))
            .child(icon("search", 14.).text_color(content(0.45)))
            .child(div().flex_1().min_w_0().child(self.input.clone()))
    }
}

pub(crate) enum MenuEvent {
    /// The picked action, and where its row was, for menus opened from it.
    Picked(SharedString, Option<Bounds<Pixels>>),
    Dismissed,
}
/// A separate event so lazy flyouts can load without dismissing menu owners.
pub(crate) struct MenuOpened(pub SharedString);

/// Vertical padding of a row that shows a description under its title.
const TWO_LINE_PAD: f32 = 6.;

/// A row's height: fixed at `height` for one line, but a row with a description grows to fit
/// its second line (never below `height`), so the hint cannot draw over the next row.
fn row_height<E: gpui::Styled>(row: E, height: f32, two_line: bool) -> E {
    if two_line {
        row.min_h(px(height)).py(px(TWO_LINE_PAD))
    } else {
        row.h(px(height))
    }
}

const MENU_WIDTH: f32 = 228.;
/// Content-sized menus never shrink below this.
const MENU_MIN_WIDTH: f32 = 160.;
// Tailwind v4 reds.
const RED_300: u32 = 0xffa2a2;
const RED_500: u32 = 0xfb2c36;

/// ExplorerMenu and its siblings: takes focus on open, highlights under the
/// pointer and with the arrow keys, and opens one level of submenu or flyout.
/// Picking or dismissing returns focus to wherever it was before.
pub(crate) struct Menu {
    label: SharedString,
    items: Vec<MenuItem>,
    style: MenuStyle,
    width: f32,
    row_metrics: Option<(f32, f32)>,
    search_navigation: bool,
    submenu_width: Option<f32>,
    active: Option<usize>,
    submenu: Option<usize>,
    sub_active: Option<usize>,
    rows: Vec<AnchorSlot>,
    surface: AnchorSlot,
    sub_region: AnchorSlot,
    focus: FocusHandle,
    restore: Option<FocusHandle>,
}
impl EventEmitter<MenuEvent> for Menu {}
impl EventEmitter<MenuOpened> for Menu {}
impl Focusable for Menu {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}
impl Menu {
    pub(crate) fn submenu_region(&self) -> AnchorSlot {
        self.sub_region.clone()
    }
    pub(crate) fn surface_region(&self) -> AnchorSlot {
        self.surface.clone()
    }
    pub(crate) fn new(
        label: impl Into<SharedString>,
        items: Vec<MenuItem>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut menu = Self::embedded(label, items, cx);
        menu.restore = window.focused(cx);
        window.focus(&menu.content_focus(), cx);
        menu
    }
    /// A flyout is constructed before it opens; it must not steal focus then.
    pub(crate) fn embedded(
        label: impl Into<SharedString>,
        items: Vec<MenuItem>,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus = cx.focus_handle();
        let active = items
            .iter()
            .position(|item| matches!(item, MenuItem::Action(_)));
        Self {
            label: label.into(),
            rows: items.iter().map(|_| AnchorSlot::default()).collect(),
            surface: AnchorSlot::default(),
            items,
            style: MenuStyle::Explorer,
            width: MENU_WIDTH,
            row_metrics: None,
            search_navigation: false,
            submenu_width: None,
            active,
            submenu: None,
            sub_active: None,
            sub_region: AnchorSlot::default(),
            focus,
            restore: None,
        }
    }
    pub(crate) fn width(mut self, width: f32) -> Self {
        self.width = width;
        self
    }
    /// Override row and icon sizes for menus whose source uses taller rows.
    pub(crate) fn row_metrics(mut self, height: f32, icon_size: f32) -> Self {
        self.row_metrics = Some((height, icon_size));
        self
    }
    /// A custom search field yields arrow and submit actions to this menu.
    pub(crate) fn search_navigation(mut self) -> Self {
        self.search_navigation = true;
        self
    }
    pub(crate) fn submenu_width(mut self, width: f32) -> Self {
        self.submenu_width = Some(width);
        self
    }
    pub(crate) fn content_focus(&self) -> FocusHandle {
        self.items
            .iter()
            .find_map(|item| match item {
                MenuItem::Custom(_, focus) => Some(focus.clone()),
                _ => None,
            })
            .unwrap_or_else(|| self.focus.clone())
    }
    pub(crate) fn select_value(mut self, value: &str) -> Self {
        self.highlight_value(value);
        self
    }
    /// Highlights the enabled action with id `value` (none when there is no such row).
    pub(crate) fn highlight_value(&mut self, value: &str) {
        self.active = self.items.iter().position(|item| {
            matches!(item, MenuItem::Action(action) if action.id.as_ref() == value && !action.disabled)
        });
    }
    pub(crate) fn style(mut self, style: MenuStyle) -> Self {
        self.style = style;
        self
    }
    /// Replace a loading placeholder in place, retaining the flyout and focus.
    pub(crate) fn set_items(&mut self, items: Vec<MenuItem>, cx: &mut Context<Self>) {
        self.active = items
            .iter()
            .position(|item| matches!(item, MenuItem::Action(action) if !action.disabled));
        self.rows = items.iter().map(|_| AnchorSlot::default()).collect();
        self.items = items;
        self.submenu = None;
        self.sub_active = None;
        self.sub_region.set(None);
        cx.notify();
    }
    /// Rows of the open submenu or flyout's anchor, for anchoring follow-ups.
    pub(crate) fn row_bounds(&self, ix: usize) -> Option<Bounds<Pixels>> {
        self.rows.get(ix).and_then(AnchorSlot::get)
    }
    fn action(&self, ix: usize) -> Option<&MenuAction> {
        match self.items.get(ix) {
            Some(MenuItem::Action(action)) => Some(action),
            _ => None,
        }
    }
    fn submenu_items(&self) -> &[MenuAction] {
        self.submenu
            .and_then(|ix| self.action(ix))
            .map_or(&[], |action| &action.submenu)
    }
    fn flyout_open(&self) -> bool {
        self.submenu
            .and_then(|ix| self.action(ix))
            .is_some_and(|a| a.flyout.is_some())
    }
    pub(crate) fn finish(&mut self, event: MenuEvent, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(handle) = self.restore.take() {
            window.focus(&handle, cx);
        }
        cx.emit(event);
    }
    pub(crate) fn replace_items(&mut self, items: Vec<MenuItem>, cx: &mut Context<Self>) {
        self.rows.resize_with(items.len(), AnchorSlot::default);
        self.items = items;
        self.active = self
            .active
            .filter(|&ix| self.action(ix).is_some_and(|a| !a.disabled));
        self.submenu = None;
        self.sub_active = None;
        cx.notify();
    }
    /// Highlights the first enabled action, wherever the highlight was.
    pub(crate) fn highlight_first(&mut self, cx: &mut Context<Self>) {
        self.active = None;
        self.ensure_active(cx);
    }
    /// Highlights the first enabled action when no row is highlighted.
    pub(crate) fn ensure_active(&mut self, cx: &mut Context<Self>) {
        if self.active.is_none() {
            self.active =
                (0..self.items.len()).find(|&ix| self.action(ix).is_some_and(|a| !a.disabled));
            cx.notify();
        }
    }
    /// Refresh a live toggle without resetting keyboard focus or its flyout.
    pub(crate) fn set_toggle(&mut self, id: &str, on: bool, cx: &mut Context<Self>) {
        if let Some(action) = self.items.iter_mut().find_map(|item| match item {
            MenuItem::Action(action) if action.id.as_ref() == id && action.switch => Some(action),
            _ => None,
        }) {
            action.checked = on;
            cx.notify();
        }
    }
    pub(crate) fn toggle_state(&self, id: &str) -> Option<bool> {
        self.items.iter().find_map(|item| match item {
            MenuItem::Action(action) if action.id.as_ref() == id && action.switch => {
                Some(action.checked)
            }
            _ => None,
        })
    }
    fn open_submenu(
        &mut self,
        ix: usize,
        active: Option<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let changed = self.submenu != Some(ix);
        self.submenu = Some(ix);
        self.sub_active = active;
        if let Some(focus) = self
            .action(ix)
            .and_then(|a| a.flyout.as_ref())
            .and_then(|f| f.focus.clone())
        {
            window.focus(&focus, cx);
        }
        if changed && let Some(action) = self.action(ix) {
            cx.emit(MenuOpened(action.id.clone()));
        }
        cx.notify();
    }
    fn close_submenu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.flyout_open() {
            window.focus(&self.focus, cx);
        }
        self.submenu = None;
        self.sub_active = None;
        self.sub_region.set(None);
        cx.notify();
    }
    fn pick(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(action) = self.action(ix) else {
            return;
        };
        if action.disabled {
            return;
        }
        if action.opens() {
            let first = action.flyout.is_none().then(|| {
                action
                    .submenu
                    .iter()
                    .position(|item| item.checked)
                    .unwrap_or(0)
            });
            self.open_submenu(ix, first, window, cx);
        } else {
            let id = action.id.clone();
            let keep_open = action.keep_open;
            let row = self.row_bounds(ix);
            if keep_open {
                if self.submenu.is_some() {
                    self.close_submenu(window, cx);
                }
                cx.emit(MenuEvent::Picked(id, row));
            } else {
                self.finish(MenuEvent::Picked(id, row), window, cx);
            }
        }
    }
    fn pick_sub(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(action) = self.submenu_items().get(ix) else {
            return;
        };
        if !action.disabled {
            let id = action.id.clone();
            self.finish(MenuEvent::Picked(id, None), window, cx);
        }
    }
    fn step(&mut self, direction: isize, cx: &mut Context<Self>) {
        let count = self.submenu_items().len();
        if count > 0 {
            let next = match self.sub_active {
                None if direction > 0 => 0,
                None => count - 1,
                Some(at) => (at as isize + direction).rem_euclid(count as isize) as usize,
            };
            self.sub_active = Some(next);
        } else {
            let ids: Vec<_> = (0..self.items.len())
                .filter(|ix| self.action(*ix).is_some())
                .collect();
            if ids.is_empty() {
                return;
            }
            let from = self
                .active
                .and_then(|a| ids.iter().position(|ix| *ix == a))
                .unwrap_or(0);
            self.active =
                Some(ids[(from as isize + direction).rem_euclid(ids.len() as isize) as usize]);
        }
        cx.notify();
    }
    fn row(
        &self,
        ix: usize,
        action: &MenuAction,
        style: MenuStyle,
        highlighted: bool,
        in_submenu: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let red: Hsla = rgb(RED_300).into();
        let rich = matches!(style, MenuStyle::Composer | MenuStyle::Access);
        // The plus menu dims a disabled row whole; the others grey its ink.
        let dim = action.disabled && (rich || style == MenuStyle::Git);
        let ink = if action.disabled && !dim {
            content(0.3)
        } else if action.danger {
            red.opacity(if highlighted { 1. } else { 0.9 })
        } else {
            content(1.)
        };
        let open = self.submenu == Some(ix) && !in_submenu;
        let filled = (style.highlights() && highlighted)
            || (style == MenuStyle::Access && action.checked)
            || (style == MenuStyle::Switcher && open);
        let fill = match style {
            MenuStyle::Switcher => content(0.08),
            _ if action.danger => rgb(RED_500).into(),
            _ => content(0.1),
        };
        let fill = if action.danger && style != MenuStyle::Switcher {
            fill.opacity(0.2)
        } else {
            fill
        };
        let hover = match style {
            _ if action.danger => {
                let mut c: Hsla = rgb(RED_500).into();
                c.a = 0.15;
                c
            }
            MenuStyle::Switcher => content(0.08),
            MenuStyle::Composer | MenuStyle::Git => content(0.1),
            _ => content(0.05),
        };
        let icon_size = self
            .row_metrics
            .map(|(_, size)| size)
            .unwrap_or_else(|| match style {
                MenuStyle::Group | MenuStyle::Access | MenuStyle::Git | MenuStyle::Filter => 14.,
                _ => 16.,
            });
        let icon_ink = action.icon_color.unwrap_or(match style {
            MenuStyle::Group | MenuStyle::Switcher => content(0.55),
            MenuStyle::Access => content(0.7),
            _ => content(1.),
        });
        let has_submenu = !action.submenu.is_empty() || action.flyout.is_some();
        let selector = action.id.clone();
        let two_line = action.description.is_some();
        div()
            .id((
                if in_submenu {
                    "submenu-item"
                } else {
                    "menu-item"
                },
                ix,
            ))
            .debug_selector(move || format!("menu-action-{selector}"))
            .role(if action.switch {
                Role::MenuItemCheckBox
            } else {
                Role::MenuItem
            })
            .aria_label(action.label.clone())
            .when_some(action.description.clone(), |d, s| d.aria_description(s))
            .when_some(action.shortcut.clone(), |d, s| d.aria_keyshortcuts(s))
            .when(action.checked || action.switch, |d| {
                d.aria_toggled(if action.checked {
                    Toggled::True
                } else {
                    Toggled::False
                })
            })
            .when(has_submenu, |d| d.aria_expanded(open))
            .aria_selected(highlighted)
            .cursor_pointer()
            .relative()
            .flex_shrink_0()
            .w_full()
            .flex()
            .px_2()
            .text_size(px(if style == MenuStyle::Git { 12. } else { 13. }))
            .text_color(ink)
            .when(dim, |d| d.opacity(0.4))
            .map(|d| match style {
                MenuStyle::Explorer => d
                    .items_center()
                    .gap_3()
                    .rounded(px(8.))
                    // Not 13px: `truncate` clips to the line box, and Windows'
                    // Segoe UI lost the bottom of g/y/p at the font's own size.
                    .line_height(px(16.))
                    .map(|d| {
                        if action.description.is_some() {
                            // The 16px line (above) eats 1.5px a side.
                            d.py(px(4.5))
                        } else {
                            d.h(px(if in_submenu && self.style == MenuStyle::Settings {
                                32.
                            } else {
                                28.
                            }))
                        }
                    }),
                MenuStyle::Group => d
                    .items_center()
                    .gap(px(10.))
                    .rounded(px(8.))
                    .min_h(px(32.))
                    .when(action.description.is_some(), |d| d.py_2()),
                MenuStyle::Switcher => {
                    row_height(d.items_center().gap(px(10.)).rounded(px(6.)), 32., two_line)
                }
                MenuStyle::Git | MenuStyle::Filter => {
                    row_height(d.items_center().gap_2().rounded(px(8.)), 28., two_line)
                }
                MenuStyle::Settings => {
                    row_height(d.items_center().gap_2().rounded(px(8.)), 36., two_line)
                }
                // Sized for its title (16px) over a one-line hint (16px): already two lines.
                MenuStyle::Recent => d.items_center().gap_2().rounded(px(8.)).h(px(40.)),
                MenuStyle::Composer | MenuStyle::Access => {
                    d.items_start().gap(px(10.)).rounded(px(8.)).py_2()
                }
            })
            .when_some(self.row_metrics, |d, (height, _)| {
                row_height(d, height, two_line).gap(px(8.))
            })
            .when(filled && !action.disabled, |d| d.bg(fill))
            .when(!filled && !action.disabled, |d| d.hover(|s| s.bg(hover)))
            .when(!in_submenu, |d| d.child(self.rows[ix].probe()))
            .when_some(action.icon, |d, path| {
                d.child(
                    icon(path, icon_size)
                        .text_color(icon_ink)
                        .when(rich, |i| i.mt(px(2.))),
                )
            })
            .when_some(action.icon_artwork, |d, path| {
                d.child(
                    gpui::img(path)
                        .size(px(icon_size))
                        .flex_shrink_0()
                        .object_fit(gpui::ObjectFit::Contain),
                )
            })
            .when_some(action.image, |d, provider| {
                d.child(provider_icon(provider, icon_size))
            })
            .child(self.row_text(action, style))
            .when_some(action.value.clone(), |d, value| {
                d.child(match style {
                    MenuStyle::Settings => div()
                        .flex_shrink_0()
                        .max_w(px(if action.value_image.is_some() {
                            144.
                        } else {
                            112.
                        }))
                        .flex()
                        .items_center()
                        .gap_1()
                        .text_color(content(0.55))
                        .when_some(action.value_image, |d, provider| {
                            d.child(provider_icon(provider, 14.))
                        })
                        .child(div().min_w_0().truncate().child(value)),
                    _ => div()
                        .flex_shrink_0()
                        .max_w(px(96.))
                        .truncate()
                        .text_size(px(11.))
                        .text_color(content(0.4))
                        .child(value),
                })
            })
            .map(|d| {
                if has_submenu {
                    let chevron = match style {
                        MenuStyle::Switcher => content(0.4),
                        MenuStyle::Settings => content(0.45),
                        _ => content(0.5),
                    };
                    d.child(icon("chevron-right", 14.).text_color(chevron))
                } else if action.switch {
                    d.child(
                        div()
                            .relative()
                            .w(px(36.))
                            .h(px(20.))
                            .flex_shrink_0()
                            .rounded_full()
                            .bg(content(if action.checked { 0.35 } else { 0.15 }))
                            .child(
                                div()
                                    .absolute()
                                    .top(px(2.))
                                    .left(px(if action.checked { 18. } else { 2. }))
                                    .size(px(16.))
                                    .rounded_full()
                                    .bg(content(1.)),
                            ),
                    )
                } else if action.checked && style == MenuStyle::Recent {
                    d.child(icon("check", 14.).text_color(content(0.55)))
                } else if action.checked && style != MenuStyle::Access {
                    d.child(
                        icon(
                            if in_submenu || style == MenuStyle::Explorer {
                                "check"
                            } else {
                                "check-2"
                            },
                            14.,
                        )
                        .text_color(action.icon_color.unwrap_or(content(1.)))
                        .when(rich, |i| i.mt(px(2.))),
                    )
                } else if let Some(shortcut) = action.shortcut.clone() {
                    d.child(
                        div()
                            .flex_shrink_0()
                            .text_size(px(11.))
                            .text_color(content(0.4))
                            .child(shortcut),
                    )
                } else {
                    d
                }
            })
            .on_hover(cx.listener(move |this, hovered: &bool, window, cx| {
                if !*hovered {
                    return;
                }
                if in_submenu {
                    this.sub_active = Some(ix);
                } else {
                    this.active = Some(ix);
                    let opens = this.action(ix).is_some_and(MenuAction::opens);
                    if opens {
                        if this.submenu != Some(ix) {
                            this.open_submenu(ix, None, window, cx);
                        }
                    } else if this.submenu.is_some() {
                        this.close_submenu(window, cx);
                    }
                }
                cx.notify();
            }))
            .on_click(cx.listener(move |this, _, window, cx| {
                if in_submenu {
                    this.pick_sub(ix, window, cx);
                } else {
                    this.pick(ix, window, cx);
                }
            }))
            .into_any()
    }
    fn row_text(&self, action: &MenuAction, style: MenuStyle) -> impl IntoElement {
        let title = div()
            .min_w_0()
            .truncate()
            .when(style == MenuStyle::Access, |d| {
                d.font_weight(gpui::FontWeight::MEDIUM).line_height(px(20.))
            })
            .when(style == MenuStyle::Composer, |d| d.line_height(px(19.5)))
            .when(style == MenuStyle::Recent, |d| d.line_height(px(16.)))
            .child(action.label.clone());
        let hint = action.description.clone().map(|text| {
            let (alpha, gap) = match style {
                MenuStyle::Composer | MenuStyle::Recent => (0.45, 0.),
                MenuStyle::Access => (0.5, 2.),
                MenuStyle::Group => (0.6, 4.),
                _ => (0.5, 4.),
            };
            div()
                .mt(px(gap))
                .text_size(px(11.))
                .line_height(px(
                    if matches!(
                        style,
                        MenuStyle::Composer | MenuStyle::Access | MenuStyle::Recent
                    ) {
                        16.
                    } else {
                        15.
                    },
                ))
                .text_color(content(alpha))
                .when(
                    matches!(style, MenuStyle::Composer | MenuStyle::Recent),
                    |d| d.truncate(),
                )
                .child(text)
        });
        div()
            .flex_1()
            .min_w_0()
            .child(match &action.badge {
                // The Orchestrator row's "v1" pill beside its title.
                Some(badge) => div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .child(title)
                    .child(
                        div()
                            .flex_shrink_0()
                            .mb(px(1.))
                            .px(px(6.))
                            .py(px(2.))
                            .rounded_full()
                            .bg(gpui::hsla(0.82, 1., 0.83, 0.1))
                            .text_size(px(9.))
                            .line_height(px(9.))
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .text_color(gpui::hsla(0.81, 1., 0.9, 0.55))
                            .child(badge.clone()),
                    )
                    .into_any(),
                None => title.into_any(),
            })
            .children(hint)
    }
}
impl Render for Menu {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let style = self.style;
        let rows: Vec<AnyElement> = self
            .items
            .iter()
            .enumerate()
            .map(|(ix, item)| match item {
                MenuItem::Custom(view, _) => {
                    div().flex_shrink_0().child(view.clone()).into_any_element()
                }
                MenuItem::Header(text) => div()
                    .flex_shrink_0()
                    .px_2()
                    .pt(px(2.))
                    .pb_1()
                    .text_size(px(10.))
                    .line_height(px(15.))
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .text_color(content(0.4))
                    .child(text.to_uppercase())
                    .into_any(),
                MenuItem::Note(text) => div()
                    .flex_shrink_0()
                    .px_2()
                    .py(px(6.))
                    .text_size(px(11.))
                    .line_height(px(16.))
                    .text_color(content(0.5))
                    .child(text.clone())
                    .into_any(),
                MenuItem::Separator => separator(ix, style),
                MenuItem::Action(action) => {
                    self.row(ix, action, style, self.active == Some(ix), false, cx)
                }
            })
            .collect();
        let submenu = self.submenu.and_then(|ix| {
            let action = self.action(ix)?;
            let submenu = submenu_popover(
                ("submenu", ix),
                self.rows[ix].clone(),
                self.surface.clone(),
                self.sub_region.clone(),
            );
            if let Some(flyout) = &action.flyout {
                let p = submenu
                    .width(flyout.width)
                    .padding(flyout.padding)
                    .when_some(flyout.surface.clone(), |p, slot| p.track(slot));
                let p = match (flyout.height, flyout.max_height) {
                    (Some(h), _) => p.height(h),
                    (None, Some(h)) => p.max_height(h),
                    _ => p,
                };
                return Some(p.child(flyout.view.clone()));
            }
            let rows: Vec<_> = action
                .submenu
                .iter()
                .enumerate()
                .map(|(sub, item)| {
                    self.row(
                        sub,
                        item,
                        MenuStyle::Explorer,
                        self.sub_active == Some(sub),
                        true,
                        cx,
                    )
                })
                .collect();
            Some(
                submenu
                    .map(|p| match self.submenu_width {
                        Some(width) => p.width(width),
                        None => p.max_width(MENU_WIDTH).min_width(MENU_MIN_WIDTH),
                    })
                    .padding(4.)
                    .child(
                        div()
                            .id("popover-submenu")
                            .role(Role::Menu)
                            .aria_label(action.label.clone())
                            .flex()
                            .flex_col()
                            .children(rows),
                    ),
            )
        });
        div()
            .id("popover-menu")
            .role(Role::Menu)
            .aria_label(self.label.clone())
            .key_context("PopoverMenu")
            .track_focus(&self.focus)
            .capture_action(cx.listener(|this, _: &crate::input::Up, window, cx| {
                if !this.search_navigation {
                    cx.propagate();
                    return;
                }
                this.step(-1, cx);
                window.focus(&this.focus, cx);
                cx.stop_propagation();
            }))
            .capture_action(cx.listener(|this, _: &crate::input::Down, window, cx| {
                if !this.search_navigation {
                    cx.propagate();
                    return;
                }
                this.step(1, cx);
                window.focus(&this.focus, cx);
                cx.stop_propagation();
            }))
            .capture_action(cx.listener(|this, _: &crate::input::Submit, window, cx| {
                if !this.search_navigation {
                    cx.propagate();
                    return;
                }
                window.focus(&this.focus, cx);
                match (this.submenu, this.sub_active) {
                    (Some(_), Some(sub)) => this.pick_sub(sub, window, cx),
                    _ => {
                        if let Some(ix) = this.active {
                            this.pick(ix, window, cx);
                        }
                    }
                }
                cx.stop_propagation();
            }))
            // Space activates a focused menu, but remains text in its fields.
            .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, window, cx| {
                if event.keystroke.key != "space" || !this.focus.is_focused(window) {
                    return;
                }
                match (this.submenu, this.sub_active) {
                    (Some(_), Some(sub)) => this.pick_sub(sub, window, cx),
                    _ => {
                        if let Some(ix) = this.active {
                            this.pick(ix, window, cx);
                        }
                    }
                }
                window.prevent_default();
                cx.stop_propagation();
            }))
            .on_action(cx.listener(|this, _: &SelectNext, _, cx| this.step(1, cx)))
            .on_action(cx.listener(|this, _: &SelectPrevious, _, cx| this.step(-1, cx)))
            .on_action(cx.listener(|this, _: &Confirm, window, cx| {
                match (this.submenu, this.sub_active) {
                    (Some(_), Some(sub)) => this.pick_sub(sub, window, cx),
                    _ => {
                        if let Some(ix) = this.active {
                            this.pick(ix, window, cx)
                        }
                    }
                }
            }))
            .on_action(cx.listener(|this, _: &OpenSubmenu, window, cx| {
                if let Some(ix) = this.active
                    && this.action(ix).is_some_and(MenuAction::opens)
                {
                    let first = this
                        .action(ix)
                        .filter(|action| action.flyout.is_none())
                        .map(|action| {
                            action
                                .submenu
                                .iter()
                                .position(|item| item.checked)
                                .unwrap_or(0)
                        });
                    this.open_submenu(ix, first, window, cx);
                }
            }))
            .on_action(
                cx.listener(|this, _: &CloseSubmenu, window, cx| this.close_submenu(window, cx)),
            )
            .on_action(cx.listener(|this, _: &Cancel, window, cx| {
                // A flyout's Escape closes everything, as the pickers' does.
                if this.submenu.is_some() && !this.flyout_open() {
                    this.close_submenu(window, cx);
                } else {
                    this.finish(MenuEvent::Dismissed, window, cx);
                }
            }))
            .min_h_0()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .children(rows)
            .children(submenu)
    }
}
fn separator(ix: usize, style: MenuStyle) -> AnyElement {
    div()
        .id(("menu-separator", ix))
        .role(Role::Splitter)
        .flex_shrink_0()
        .my_1()
        .h(px(1.))
        .map(|d| {
            if style == MenuStyle::Switcher {
                // `mx-2 bg-stroke`.
                d.mx_2().bg(content(0.07))
            } else if style == MenuStyle::Git {
                // `mx-1 border-t border-content/10`.
                d.mx_1().bg(content(0.1))
            } else {
                d.bg(content(0.1))
            }
        })
        .into_any()
}

/// All child menus and picker flyouts share the same card-edge gap, flipping
/// and layer. Their content only controls size and padding.
fn submenu_popover(
    id: impl Into<ElementId>,
    row: AnchorSlot,
    surface: AnchorSlot,
    region: AnchorSlot,
) -> Popover {
    popover(id, Anchor::Submenu { row, surface })
        .side(Side::Right)
        .gap(SUBMENU_GAP)
        .priority(41)
        .track(region)
}

/// Wraps `menu` in its popover. Pressing outside it, or outside its open
/// submenu or flyout, dismisses it like Escape does.
pub(crate) fn menu_popover(menu: &Entity<Menu>, anchor: Anchor, cx: &App) -> Popover {
    let m = menu.read(cx);
    let weak = menu.downgrade();
    // Plain menus hug their rows up to their width; embedded views keep it.
    let custom = m
        .items
        .iter()
        .any(|item| matches!(item, MenuItem::Custom(..)));
    popover(("menu", menu.entity_id().as_u64() as usize), anchor)
        .map(|p| {
            if custom || m.style == MenuStyle::Filter {
                p.width(m.width)
            } else {
                p.max_width(m.width).min_width(MENU_MIN_WIDTH.min(m.width))
            }
        })
        .when(m.style == MenuStyle::Filter, |p| p.max_height(480.))
        .padding(m.style.padding())
        .track(m.surface.clone())
        .ignore(m.sub_region.clone())
        .on_dismiss(move |window, cx| {
            let _ = weak.update(cx, |m, cx| m.finish(MenuEvent::Dismissed, window, cx));
        })
        .child(menu.clone())
}

/// One entry of a [`Choices`] list: the value it stands for, its label and whether it is the
/// current one (drawn with a check).
pub(crate) struct Choice<T> {
    pub value: T,
    pub label: SharedString,
    pub checked: bool,
}

/// What an open [`Choices`] list reports to its owner.
pub(crate) enum ChoiceEvent<T> {
    Picked(T),
    Dismissed,
}

/// An open single-choice dropdown: the app's [`Menu`] (the style of the Providers "sign-in
/// options" menu) listing `Choice`s, floating under the trigger it is anchored to instead of
/// taking room in the layout. Hold it while the list is open and drop it to close; render
/// [`Choices::popover`] next to the trigger. The row ids are the choices' indexes, so the rows
/// draw as `menu-action-0`, `menu-action-1`, ...
pub(crate) struct Choices<T> {
    menu: Entity<Menu>,
    values: Rc<RefCell<Vec<Option<T>>>>,
    _events: Subscription,
}

/// The menu rows of `choices` and the values they stand for.
fn choice_items<T>(choices: Vec<Choice<T>>) -> (Vec<MenuItem>, Vec<Option<T>>) {
    let mut values = Vec::with_capacity(choices.len());
    let mut items = Vec::with_capacity(choices.len());
    for (ix, choice) in choices.into_iter().enumerate() {
        items.push(
            MenuAction::new(ix.to_string(), choice.label)
                .checked(choice.checked)
                .into(),
        );
        values.push(Some(choice.value));
    }
    (items, values)
}

/// The id of the row that is checked, which the keyboard starts on.
fn checked_id(items: &[MenuItem]) -> Option<String> {
    items.iter().find_map(|item| match item {
        MenuItem::Action(a) if a.checked => Some(a.id.to_string()),
        _ => None,
    })
}

impl<T: 'static> Choices<T> {
    /// Opens the list, `width` wide, and gives it the keyboard (arrows, Enter and Escape work as
    /// in any [`Menu`]). `on_event` runs once per pick or dismissal; the owner drops the
    /// `Choices` in it.
    pub(crate) fn open<V: 'static>(
        label: impl Into<SharedString>,
        choices: Vec<Choice<T>>,
        width: f32,
        window: &mut Window,
        cx: &mut Context<V>,
        on_event: impl Fn(&mut V, ChoiceEvent<T>, &mut Window, &mut Context<V>) + 'static,
    ) -> Self {
        let (items, values) = choice_items(choices);
        let checked = checked_id(&items);
        let menu = cx.new(|cx| {
            let menu = Menu::new(label, items, window, cx)
                .width(width)
                .style(MenuStyle::Git);
            // The keyboard starts on the current value.
            match checked {
                Some(id) => menu.select_value(&id),
                None => menu,
            }
        });
        let values = Rc::new(RefCell::new(values));
        let picked = values.clone();
        let events = cx.subscribe_in(
            &menu,
            window,
            move |this, _, event: &MenuEvent, window, cx| {
                let event = match event {
                    MenuEvent::Picked(id, _) => id
                        .parse::<usize>()
                        .ok()
                        .and_then(|ix| picked.borrow_mut().get_mut(ix)?.take())
                        .map_or(ChoiceEvent::Dismissed, ChoiceEvent::Picked),
                    MenuEvent::Dismissed => ChoiceEvent::Dismissed,
                };
                on_event(this, event, window, cx);
            },
        );
        Self {
            menu,
            values,
            _events: events,
        }
    }

    /// Replaces the rows of the open list (its options changed under it). The highlight follows
    /// its VALUE to the new rows. It starts on the checked row (the first row when none is
    /// checked), so when it is still there, or its value is gone, the new checked row gets it.
    pub(crate) fn refresh(&self, choices: Vec<Choice<T>>, cx: &mut App)
    where
        T: PartialEq,
    {
        let (items, values) = choice_items(choices);
        let checked = checked_id(&items);
        let old = std::mem::replace(&mut *self.values.borrow_mut(), values);
        let new = self.values.clone();
        self.menu.update(cx, |menu, cx| {
            let first = menu
                .items
                .iter()
                .position(|item| matches!(item, MenuItem::Action(a) if !a.disabled));
            let old_default = menu
                .items
                .iter()
                .position(|item| matches!(item, MenuItem::Action(a) if a.checked))
                .or(first);
            let moved = menu
                .active
                .filter(|&ix| Some(ix) != old_default)
                .and_then(|ix| old.get(ix)?.as_ref())
                .and_then(|value| new.borrow().iter().position(|v| v.as_ref() == Some(value)));
            menu.replace_items(items, cx);
            match (moved, checked) {
                (Some(ix), _) => menu.highlight_value(&ix.to_string()),
                (None, Some(id)) => menu.highlight_value(&id),
                (None, None) => menu.highlight_first(cx),
            }
            cx.notify();
        });
    }

    /// Closes the list without emitting and gives the keyboard back to where it was when the
    /// list opened. Dropping a `Choices` alone leaves focus on the closed list.
    pub(crate) fn close(self, window: &mut Window, cx: &mut App) {
        if let Some(handle) = self.release(cx) {
            window.focus(&handle, cx);
        }
    }

    /// Closes the list without emitting and hands back the focus it should return to, for
    /// callers that have no window at hand.
    pub(crate) fn release(self, cx: &mut App) -> Option<FocusHandle> {
        self.menu.update(cx, |menu, _| menu.restore.take())
    }

    /// The highlighted row and the checked row of the open list (tests only).
    #[cfg(test)]
    pub(crate) fn highlight_and_checked(&self, cx: &App) -> (Option<usize>, Option<usize>) {
        let menu = self.menu.read(cx);
        let checked = menu
            .items
            .iter()
            .position(|item| matches!(item, MenuItem::Action(a) if a.checked));
        (menu.active, checked)
    }

    /// The list drawn under `anchor`, as wide as it was opened with. `priority` is where it
    /// draws: above the modal dialog that holds the trigger.
    pub(crate) fn popover(&self, anchor: &AnchorSlot, priority: usize, cx: &App) -> Popover {
        let width = self.menu.read(cx).width;
        menu_popover(&self.menu, Anchor::Slot(anchor.clone()), cx)
            .width(width)
            .gap(4.)
            .max_height(320.)
            .priority(priority)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entrance_uses_react_timing_and_the_resolved_side() {
        assert_eq!(entrance(0., false), 0.);
        assert_eq!(entrance(MENU_IN, false), 1.);
        assert_eq!(entrance(0., true), 1.);
        // CSS cubic-bezier(.16, 1, .3, 1) is already about 97% done halfway.
        assert!((entrance(MENU_IN / 2., false) - 0.972).abs() < 0.002);
        for (side, from) in [
            (Side::Bottom, point(px(0.), px(-8.))),
            (Side::Top, point(px(0.), px(8.))),
            (Side::Right, point(px(-8.), px(0.))),
            (Side::Left, point(px(8.), px(0.))),
        ] {
            assert_eq!(entrance_offset(side, 0.), from);
            assert_eq!(entrance_offset(side, 1.), Point::default());
        }
    }

    struct MotionHost {
        side: Side,
        anchor: Point<Pixels>,
        progress: f32,
        card: AnchorSlot,
    }
    impl Render for MotionHost {
        fn render(&mut self, window: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().size_full().child(
                popover("motion-test", Anchor::Point(self.anchor))
                    .side(self.side)
                    .width(180.)
                    .height(100.)
                    .padding(8.)
                    .track(self.card.clone())
                    .child(div().size(px(20.)).bg(rgb(0x19734b)))
                    .render_at(self.progress, window.viewport_size()),
            )
        }
    }

    struct FitHost {
        content: f32,
        card: AnchorSlot,
    }
    impl Render for FitHost {
        fn render(&mut self, window: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().size_full().child(
                popover("fit-test", Anchor::Point(point(px(100.), px(100.))))
                    .max_width(300.)
                    .padding(8.)
                    .track(self.card.clone())
                    .child(div().w(px(self.content)).h(px(20.)))
                    .render_at(1., window.viewport_size()),
            )
        }
    }

    struct LayersHost {
        above: bool,
        dismissed: Rc<Cell<usize>>,
        below_card: AnchorSlot,
        above_card: AnchorSlot,
    }
    impl Render for LayersHost {
        fn render(&mut self, window: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let dismissed = self.dismissed.clone();
            div()
                .size_full()
                .child(
                    popover("layer-below", Anchor::Point(point(px(100.), px(100.))))
                        .priority(70)
                        .width(300.)
                        .height(200.)
                        .track(self.below_card.clone())
                        .on_dismiss(move |_, _| dismissed.set(dismissed.get() + 1))
                        .render_at(1., window.viewport_size()),
                )
                .when(self.above, |d| {
                    d.child(
                        popover("layer-above", Anchor::Point(point(px(350.), px(150.))))
                            .priority(90)
                            .width(200.)
                            .height(100.)
                            .track(self.above_card.clone())
                            .render_at(1., window.viewport_size()),
                    )
                })
        }
    }

    #[gpui::test]
    fn a_press_on_a_popover_drawn_above_does_not_dismiss_the_one_beneath(
        cx: &mut gpui::TestAppContext,
    ) {
        let (below_card, above_card) = (AnchorSlot::default(), AnchorSlot::default());
        let dismissed = Rc::new(Cell::new(0));
        let window = cx.update(|cx| {
            cx.open_window(Default::default(), |_, cx| {
                cx.new(|_| LayersHost {
                    above: true,
                    dismissed: dismissed.clone(),
                    below_card: below_card.clone(),
                    above_card: above_card.clone(),
                })
            })
            .unwrap()
        });
        let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
        visual.run_until_parked();
        let (below, above) = (below_card.get().unwrap(), above_card.get().unwrap());
        // The card above hangs over the edge of the one beneath: its right half is outside it.
        let beyond = point(above.right() - px(10.), above.center().y);
        assert!(above.contains(&beyond) && !below.contains(&beyond));
        visual.simulate_click(beyond, Default::default());
        assert_eq!(
            dismissed.get(),
            0,
            "a press on the card above is not outside"
        );
        // A press on neither card is outside the one beneath.
        visual.simulate_click(
            point(below.right() + px(300.), below.bottom() + px(300.)),
            Default::default(),
        );
        assert_eq!(dismissed.get(), 1);
        // The card above is gone: the point it covered is outside again, not still claimed.
        window
            .update(visual, |host, _, cx| {
                host.above = false;
                cx.notify();
            })
            .unwrap();
        visual.run_until_parked();
        visual.simulate_click(beyond, Default::default());
        assert_eq!(
            dismissed.get(),
            2,
            "a closed popover no longer claims presses"
        );
    }

    #[gpui::test]
    fn a_max_width_card_hugs_short_content_and_caps_long_content(cx: &mut gpui::TestAppContext) {
        let card = AnchorSlot::default();
        let window = cx.update(|cx| {
            cx.open_window(Default::default(), |_, cx| {
                cx.new(|_| FitHost {
                    content: 50.,
                    card: card.clone(),
                })
            })
            .unwrap()
        });
        cx.run_until_parked();
        // The content, its 8px padding and the 1px frame inset each side.
        assert_eq!(card.get().unwrap().size.width, px(50. + 16. + 2.));
        window
            .update(cx, |host, _, cx| {
                host.content = 500.;
                cx.notify();
            })
            .unwrap();
        cx.run_until_parked();
        assert_eq!(card.get().unwrap().size.width, px(300.));
    }

    #[gpui::test]
    fn painted_card_and_content_move_and_fade_together(cx: &mut gpui::TestAppContext) {
        let window = cx.update(|cx| {
            cx.open_window(Default::default(), |_, cx| {
                cx.new(|_| MotionHost {
                    side: Side::Bottom,
                    anchor: point(px(300.), px(300.)),
                    progress: 1.,
                    card: AnchorSlot::default(),
                })
            })
            .unwrap()
        });
        for side in [Side::Top, Side::Bottom, Side::Left, Side::Right] {
            for flip in [false, true] {
                for progress in [0.25, 0.75, 1.] {
                    window
                        .update(cx, |host, window, cx| {
                            host.side = side;
                            host.progress = progress;
                            let viewport = window.viewport_size();
                            host.anchor = if flip {
                                match side {
                                    Side::Top => point(px(300.), px(8.)),
                                    Side::Bottom => point(px(300.), viewport.height - px(8.)),
                                    Side::Left => point(px(8.), px(300.)),
                                    Side::Right => point(viewport.width - px(8.), px(300.)),
                                }
                            } else {
                                point(px(300.), px(300.))
                            };
                            cx.notify();
                        })
                        .unwrap();
                    cx.refresh().unwrap();
                    window
                        .update(cx, |host, window, _| {
                            let quads = window.painted_quads();
                            let border = quads
                                .iter()
                                .find(|q| q.border_color == content(0.1).opacity(progress))
                                .expect("the card border fades with the content");
                            let marker_color =
                                gpui::Background::from(rgb(0x19734b)).opacity(progress);
                            let marker = quads
                                .iter()
                                .find(|q| q.background == marker_color)
                                .expect("the content uses the card's opacity");
                            let origin = |q: &gpui::Quad| {
                                point(
                                    px(q.bounds.left().0 / window.scale_factor()),
                                    px(q.bounds.top().0 / window.scale_factor()),
                                )
                            };
                            assert_eq!(
                                origin(marker) - origin(border),
                                point(px(9.), px(9.)),
                                "content must keep its inset throughout the entrance"
                            );
                            assert_eq!(origin(border), host.card.get().unwrap().origin);
                            let viewport = window.viewport_size();
                            let p = place(
                                Rect::from_bounds(Bounds::new(host.anchor, size(px(0.), px(0.)))),
                                (180., 100.),
                                (f32::from(viewport.width), f32::from(viewport.height)),
                                &Options {
                                    side,
                                    ..Options::default()
                                },
                            );
                            assert_eq!(p.side, if flip { side.opposite() } else { side });
                            let top = p.top.unwrap_or_else(|| {
                                f32::from(viewport.height) - p.bottom.unwrap() - 100.
                            });
                            assert_eq!(
                                origin(border),
                                point(px(p.left).round(), px(top).round())
                                    + entrance_offset(p.side, progress),
                                "the visible frame and its hit region must move together"
                            );
                        })
                        .unwrap();
                }
            }
        }
    }

    struct MenuHost {
        menu: Entity<Menu>,
        anchor: Point<Pixels>,
    }
    impl Render for MenuHost {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            div()
                .size_full()
                .child(menu_popover(&self.menu, Anchor::Point(self.anchor), cx))
        }
    }

    #[gpui::test]
    fn rows_with_a_description_grow_instead_of_overlapping_the_next_row(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(|cx| cx.set_reduce_motion(true));
        // The style's own fixed height, and the `row_metrics` override that replaces it.
        for (style, metrics) in [
            (MenuStyle::Settings, None),
            (MenuStyle::Switcher, None),
            (MenuStyle::Git, None),
            (MenuStyle::Filter, None),
            (MenuStyle::Settings, Some(36.)),
        ] {
            let window = cx.update(|cx| {
                cx.open_window(Default::default(), |window, cx| {
                    cx.new(|cx| {
                        let menu = cx.new(|cx| {
                            let menu = Menu::new(
                                "Rows",
                                vec![
                                    MenuAction::new("plain", "Plain").into(),
                                    MenuAction::new("first", "Codespace")
                                        .description("Billed to your GitHub account")
                                        .into(),
                                    MenuAction::new("second", "Docker Sandbox")
                                        .description("Paid compute in your Docker account")
                                        .into(),
                                ],
                                window,
                                cx,
                            )
                            .style(style);
                            match metrics {
                                Some(height) => menu.row_metrics(height, 14.),
                                None => menu,
                            }
                        });
                        MenuHost {
                            menu,
                            anchor: point(px(100.), px(100.)),
                        }
                    })
                })
                .unwrap()
            });
            cx.run_until_parked();
            window
                .update(cx, |host, _, cx| {
                    let menu = host.menu.read(cx);
                    let rect = |ix: usize| menu.rows[ix].get().unwrap();
                    let (plain, first, second) = (rect(0), rect(1), rect(2));
                    // A one-line row keeps the style's fixed height.
                    let one_line = plain.size.height;
                    // A title plus a 15px hint cannot fit the one-line height.
                    assert!(
                        first.size.height > one_line,
                        "{style:?}: a described row must be taller than a plain one"
                    );
                    assert!(
                        first.size.height >= px(13. + 15.),
                        "{style:?}: the row must hold both lines"
                    );
                    assert!(
                        first.bottom() <= second.top(),
                        "{style:?}: the first row overlaps the second"
                    );
                })
                .unwrap();
        }
    }

    #[gpui::test]
    fn menus_and_picker_flyouts_keep_the_same_card_edge_gap(cx: &mut gpui::TestAppContext) {
        // Compare the resting gap; entrance motion deliberately starts child
        // cards a little closer to the parent before settling into this gap.
        cx.update(|cx| cx.set_reduce_motion(true));
        let window = cx.update(|cx| {
            cx.open_window(Default::default(), |window, cx| {
                cx.new(|cx| {
                    let nested = cx.new(|cx| {
                        Menu::new(
                            "Models",
                            vec![
                                MenuAction::new("model", "A model")
                                    .submenu(vec![MenuAction::new("effort", "High")])
                                    .into(),
                            ],
                            window,
                            cx,
                        )
                    });
                    let surface = nested.read(cx).surface_region();
                    let menu = cx.new(|cx| {
                        Menu::new(
                            "Settings",
                            vec![
                                MenuAction::new("models", "Models")
                                    .flyout(Flyout {
                                        view: nested.into(),
                                        width: MENU_WIDTH,
                                        padding: 0.,
                                        height: None,
                                        max_height: None,
                                        focus: None,
                                        surface: Some(surface),
                                    })
                                    .into(),
                                MenuAction::new("reasoning", "Reasoning")
                                    .submenu(vec![MenuAction::new("high", "High")])
                                    .into(),
                            ],
                            window,
                            cx,
                        )
                    });
                    MenuHost {
                        menu,
                        anchor: point(px(100.), px(100.)),
                    }
                })
            })
            .unwrap()
        });
        for style in [
            MenuStyle::Explorer,
            MenuStyle::Group,
            MenuStyle::Composer,
            MenuStyle::Settings,
            MenuStyle::Switcher,
            MenuStyle::Access,
        ] {
            for left in [false, true] {
                window
                    .update(cx, |host, window, cx| {
                        host.anchor.x = if left {
                            window.viewport_size().width - px(MENU_WIDTH + 40.)
                        } else {
                            px(100.)
                        };
                        host.menu.update(cx, |menu, cx| {
                            menu.style = style;
                            menu.close_submenu(window, cx);
                        });
                        cx.notify();
                    })
                    .unwrap();
                cx.refresh().unwrap();
                for ix in [0, 1] {
                    window
                        .update(cx, |host, window, cx| {
                            host.menu.update(cx, |menu, cx| {
                                menu.open_submenu(ix, None, window, cx);
                            });
                        })
                        .unwrap();
                    cx.refresh().unwrap();
                    window
                        .update(cx, |host, _, cx| {
                            let menu = host.menu.read(cx);
                            let parent = menu.surface.get().unwrap();
                            let child = menu.sub_region.get().unwrap();
                            let gap = if left {
                                parent.left() - child.right()
                            } else {
                                child.left() - parent.right()
                            };
                            assert_eq!(gap, px(SUBMENU_GAP), "{style:?}, row {ix}, left={left}");
                            assert_eq!(child.top(), menu.rows[ix].get().unwrap().top().round());
                            if ix == 0 {
                                let surface = menu
                                    .action(0)
                                    .unwrap()
                                    .flyout
                                    .as_ref()
                                    .unwrap()
                                    .surface
                                    .as_ref()
                                    .unwrap()
                                    .get()
                                    .unwrap();
                                assert_eq!(surface, child, "nested menus track their host card");
                            }
                        })
                        .unwrap();
                }
            }
        }
    }

    fn anchor(left: f32, top: f32, width: f32, height: f32) -> Rect {
        Rect {
            left,
            top,
            right: left + width,
            bottom: top + height,
        }
    }
    const MENU: (f32, f32) = (200., 150.);
    const VIEWPORT: (f32, f32) = (800., 600.);
    fn with(f: impl FnOnce(&mut Options)) -> Options {
        let mut o = Options::default();
        f(&mut o);
        o
    }

    #[test]
    fn opens_below_aligned_to_the_leading_edge_and_flips_above() {
        assert_eq!(
            place(
                anchor(100., 100., 80., 20.),
                MENU,
                VIEWPORT,
                &Options::default()
            ),
            Placement {
                side: Side::Bottom,
                left: 100.,
                top: Some(126.),
                bottom: None,
                width: 200.,
                max_height: 466.,
            }
        );
        assert_eq!(
            place(
                anchor(100., 560., 80., 20.),
                MENU,
                VIEWPORT,
                &Options::default()
            ),
            Placement {
                side: Side::Top,
                left: 100.,
                top: None,
                bottom: Some(46.),
                width: 200.,
                max_height: 546.,
            }
        );
        let top = place(
            anchor(100., 400., 80., 20.),
            MENU,
            VIEWPORT,
            &with(|o| o.side = Side::Top),
        );
        assert_eq!((top.top, top.bottom), (None, Some(206.)));
    }

    #[test]
    fn clamps_aligns_caps_and_narrows() {
        let o = Options::default();
        assert_eq!(
            place(anchor(700., 100., 80., 20.), MENU, VIEWPORT, &o).left,
            592.
        );
        let center = with(|o| o.align = Align::Center);
        assert_eq!(
            place(anchor(300., 100., 80., 20.), MENU, VIEWPORT, &center).left,
            240.
        );
        let end = with(|o| o.align = Align::End);
        assert_eq!(
            place(anchor(400., 100., 80., 20.), MENU, VIEWPORT, &end).left,
            280.
        );
        let capped = with(|o| o.max_height = Some(200.));
        assert_eq!(
            place(anchor(100., 100., 80., 20.), MENU, VIEWPORT, &capped).max_height,
            200.
        );
        let narrow = place(anchor(0., 100., 80., 20.), MENU, (180., 600.), &o);
        assert_eq!((narrow.left, narrow.width), (8., 164.));
    }

    #[test]
    fn keeps_minimum_heights_on_screen() {
        let min = with(|o| {
            o.side = Side::Top;
            o.min_height = Some(180.);
        });
        assert_eq!(
            place(anchor(100., 100., 80., 20.), MENU, (800., 200.), &min),
            Placement {
                side: Side::Top,
                left: 100.,
                top: None,
                bottom: Some(12.),
                width: 200.,
                max_height: 180.,
            }
        );
        let tall = with(|o| {
            o.side = Side::Top;
            o.min_height = Some(442.);
            o.max_height = Some(442.);
        });
        let placed = place(anchor(100., 300., 80., 20.), MENU, (800., 400.), &tall);
        assert_eq!(placed.max_height, 384.);
        assert!(400. - placed.bottom.unwrap() - placed.max_height >= 8.);
    }

    #[test]
    fn flyouts_hang_off_the_right_flip_left_and_clamp() {
        let flyout = with(|o| {
            o.side = Side::Right;
            o.gap = SUBMENU_GAP;
        });
        assert_eq!(
            place(
                anchor(100., 100., 200., 24.),
                (240., 300.),
                VIEWPORT,
                &flyout
            ),
            Placement {
                side: Side::Right,
                left: 304.,
                top: Some(100.),
                bottom: None,
                width: 240.,
                max_height: 492.,
            }
        );
        let flipped = place(
            anchor(200., 100., 200., 24.),
            (240., 300.),
            (420., 600.),
            &flyout,
        );
        assert_eq!((flipped.side, flipped.left), (Side::Left, 8.));
        let low = place(
            anchor(100., 560., 200., 24.),
            (240., 300.),
            VIEWPORT,
            &with(|o| o.side = Side::Right),
        );
        assert_eq!((low.top, low.max_height), (Some(292.), 300.));
    }

    /// How the dialog under test is built.
    #[derive(Clone, Copy)]
    enum DialogKind {
        /// `modal::window_overlay` around `modal::overlay`: the scrim's press dismisses.
        Overlay,
        /// A modal popover at this priority over an occluding scrim just beneath it, as the
        /// Mono settings and the Cloud settings are built.
        Popover(usize),
    }
    type Opened = (
        gpui::WindowHandle<DialogHost>,
        Rc<Cell<usize>>,
        Rc<RefCell<Vec<String>>>,
    );
    struct DialogHost {
        kind: DialogKind,
        /// A busy dialog has no dismissal.
        busy: bool,
        /// Where the dropdown opens, once it is open.
        menu_at: Option<Point<Pixels>>,
        /// Overrides the dropdown's priority; `None` keeps what `menu_popover` gives it.
        menu_priority: Option<usize>,
        menu: Entity<Menu>,
        dismissed: Rc<Cell<usize>>,
        card: AnchorSlot,
        _picked: gpui::Subscription,
    }
    impl DialogHost {
        fn open(
            kind: DialogKind,
            busy: bool,
            menu_priority: Option<usize>,
            cx: &mut gpui::TestAppContext,
        ) -> Opened {
            let dismissed = Rc::new(Cell::new(0));
            let picked = Rc::new(RefCell::new(Vec::new()));
            let (d, p) = (dismissed.clone(), picked.clone());
            let window = cx.update(|cx| {
                cx.set_reduce_motion(true);
                cx.open_window(Default::default(), move |window, cx| {
                    cx.new(|cx| {
                        let menu = cx.new(|cx| {
                            Menu::new(
                                "Dropdown",
                                vec![
                                    MenuAction::new("plain", "Plain").into(),
                                    MenuAction::new("nested", "Nested")
                                        .submenu(vec![MenuAction::new("deep", "Deep")])
                                        .into(),
                                ],
                                window,
                                cx,
                            )
                            .width(300.)
                        });
                        let sink = p.clone();
                        let _picked =
                            cx.subscribe(&menu, move |host: &mut Self, _, e: &MenuEvent, cx| {
                                if let MenuEvent::Picked(id, _) = e {
                                    sink.borrow_mut().push(id.to_string());
                                }
                                // Picked or dismissed, the dropdown closes.
                                host.menu_at = None;
                                cx.notify();
                            });
                        Self {
                            kind,
                            busy,
                            menu_at: None,
                            menu_priority,
                            menu,
                            dismissed: d,
                            card: AnchorSlot::default(),
                            _picked,
                        }
                    })
                })
                .unwrap()
            });
            let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
            visual.run_until_parked();
            (window, dismissed, picked)
        }
        /// Opens the dropdown with its left edge `inset` inside the card's right edge, so most of
        /// it hangs outside the dialog, and returns the dialog card's bounds.
        fn open_menu(
            window: gpui::WindowHandle<Self>,
            inset: f32,
            cx: &mut gpui::VisualTestContext,
        ) -> Bounds<Pixels> {
            let card = cx.debug_bounds("dialog-card").expect("the dialog card");
            window
                .update(cx, |host, _, cx| {
                    host.menu_at = Some(point(card.right() - px(inset), card.top() + px(40.)));
                    cx.notify();
                })
                .unwrap();
            cx.run_until_parked();
            card
        }
    }
    impl Render for DialogHost {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let dismissed = self.dismissed.clone();
            let busy = self.busy;
            let body = div()
                .id("dialog-card")
                .debug_selector(|| "dialog-card".into())
                .w(px(300.))
                .h(px(200.))
                .bg(rgb(0x202020))
                .child("Dialog");
            let dialog = match self.kind {
                DialogKind::Overlay => crate::modal::window_overlay(
                    window,
                    crate::modal::overlay("dialog-test", body, move |_, _| {
                        if !busy {
                            dismissed.set(dismissed.get() + 1);
                        }
                    }),
                )
                .into_any_element(),
                DialogKind::Popover(priority) => div()
                    .absolute()
                    .size_full()
                    .child(
                        deferred(
                            gpui::anchored().position(point(px(0.), px(0.))).child(
                                div()
                                    .w(window.viewport_size().width)
                                    .h(window.viewport_size().height)
                                    .occlude(),
                            ),
                        )
                        .with_priority(priority - 1),
                    )
                    .child(
                        popover("dialog-test", Anchor::Point(point(px(300.), px(200.))))
                            .priority(priority)
                            .width(300.)
                            .height(200.)
                            .track(self.card.clone())
                            .when(!busy, |p| {
                                p.on_dismiss(move |_, _| dismissed.set(dismissed.get() + 1))
                            })
                            .child(body),
                    )
                    .into_any_element(),
            };
            let menu = self.menu_at.map(|at| {
                menu_popover(&self.menu, Anchor::Point(at), cx)
                    .map(|p| match self.menu_priority {
                        Some(priority) => p.priority(priority),
                        None => p,
                    })
                    .into_any_element()
            });
            div().size_full().child(dialog).children(menu)
        }
    }
    fn dropdown_item_outside_the_dialog(
        kind: DialogKind,
        menu_priority: Option<usize>,
        item: &'static str,
        cx: &mut gpui::TestAppContext,
    ) {
        let (window, dismissed, picked) = DialogHost::open(kind, false, menu_priority, cx);
        let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
        let card = DialogHost::open_menu(window, 20., visual);
        if item == "deep" {
            window
                .update(visual, |host, window, cx| {
                    host.menu
                        .update(cx, |menu, cx| menu.open_submenu(1, None, window, cx));
                })
                .unwrap();
            visual.run_until_parked();
        }
        let row = visual
            .debug_bounds(Box::leak(format!("menu-action-{item}").into_boxed_str()))
            .expect("the dropdown row");
        let at = row.center();
        assert!(
            !card.contains(&at),
            "the press must land outside the dialog: {at:?} {card:?}"
        );
        visual.simulate_click(at, Default::default());
        assert_eq!(dismissed.get(), 0, "the dialog stays open");
        assert_eq!(*picked.borrow(), [item], "the option is applied once");
        assert!(
            window
                .read_with(visual, |h, _| h.menu_at.is_none())
                .unwrap()
        );
    }

    #[gpui::test]
    fn a_dropdown_over_an_overlay_dialog_takes_a_press_outside_the_dialog(
        cx: &mut gpui::TestAppContext,
    ) {
        dropdown_item_outside_the_dialog(DialogKind::Overlay, None, "plain", cx);
    }
    #[gpui::test]
    fn a_dropdown_over_a_popover_dialog_takes_a_press_outside_the_dialog(
        cx: &mut gpui::TestAppContext,
    ) {
        dropdown_item_outside_the_dialog(DialogKind::Popover(70), Some(90), "plain", cx);
    }
    #[gpui::test]
    fn a_submenu_of_a_dropdown_takes_a_press_outside_the_dialog(cx: &mut gpui::TestAppContext) {
        dropdown_item_outside_the_dialog(DialogKind::Popover(70), Some(90), "deep", cx);
    }
    #[gpui::test]
    fn a_submenu_of_a_dropdown_over_an_overlay_dialog_takes_a_press_outside_it(
        cx: &mut gpui::TestAppContext,
    ) {
        dropdown_item_outside_the_dialog(DialogKind::Overlay, None, "deep", cx);
    }
    #[gpui::test]
    fn a_press_outside_both_the_dialog_and_its_dropdown_still_dismisses_the_dialog(
        cx: &mut gpui::TestAppContext,
    ) {
        for kind in [DialogKind::Overlay, DialogKind::Popover(70)] {
            let (window, dismissed, picked) = DialogHost::open(kind, false, Some(90), cx);
            let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
            DialogHost::open_menu(window, 20., visual);
            visual.simulate_click(point(px(5.), px(5.)), Default::default());
            assert!(picked.borrow().is_empty());
            assert!(
                window
                    .read_with(visual, |h, _| h.menu_at.is_none())
                    .unwrap()
            );
            // A popover dialog sees the same press as the dropdown. An overlay's scrim is behind
            // the dropdown's consumed press, so only the next press reaches it.
            if matches!(kind, DialogKind::Overlay) {
                assert_eq!(
                    dismissed.get(),
                    0,
                    "the first press only closes the dropdown"
                );
                visual.simulate_click(point(px(5.), px(5.)), Default::default());
            }
            assert_eq!(dismissed.get(), 1, "the dialog's own policy still applies");
        }
    }
    #[gpui::test]
    fn a_dropdown_over_a_dialog_without_dismissal_takes_its_press_and_closes_alone(
        cx: &mut gpui::TestAppContext,
    ) {
        // `busy` builds the dialog with no `on_dismiss`, so only the dropdown reacts to presses.
        for kind in [DialogKind::Overlay, DialogKind::Popover(70)] {
            let (window, _, picked) = DialogHost::open(kind, true, Some(90), cx);
            let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
            let card = DialogHost::open_menu(window, 20., visual);
            let row = visual.debug_bounds("menu-action-plain").unwrap();
            assert!(!card.contains(&row.center()));
            visual.simulate_click(row.center(), Default::default());
            assert_eq!(*picked.borrow(), ["plain"]);
            assert!(
                window
                    .read_with(visual, |h, _| h.menu_at.is_none())
                    .unwrap()
            );
        }
    }

    /// A dialog at 100 whose content builds a popover that asks for the lower priority 70, as
    /// the Mono background dialog's Effect list does.
    struct NestedList {
        picked: Rc<Cell<usize>>,
    }
    impl Render for NestedList {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let picked = self.picked.clone();
            let _ = cx;
            popover("nested-list", Anchor::Point(point(px(560.), px(240.))))
                .priority(70)
                .width(200.)
                .height(60.)
                .track(AnchorSlot::default())
                .child(
                    div()
                        .id("nested-item")
                        .debug_selector(|| "nested-item".into())
                        .size_full()
                        .on_click(move |_, _, _| picked.set(picked.get() + 1)),
                )
        }
    }
    struct NestedHost {
        list: Entity<NestedList>,
        dismissed: Rc<Cell<usize>>,
        card: AnchorSlot,
    }
    impl Render for NestedHost {
        fn render(&mut self, window: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let dismissed = self.dismissed.clone();
            div()
                .size_full()
                .child(
                    deferred(
                        gpui::anchored().position(point(px(0.), px(0.))).child(
                            div()
                                .w(window.viewport_size().width)
                                .h(window.viewport_size().height)
                                .occlude(),
                        ),
                    )
                    .with_priority(99),
                )
                .child(
                    popover("nested-host", Anchor::Point(point(px(300.), px(200.))))
                        .priority(100)
                        .width(300.)
                        .height(200.)
                        .track(self.card.clone())
                        .on_dismiss(move |_, _| dismissed.set(dismissed.get() + 1))
                        .child(self.list.clone()),
                )
        }
    }

    #[gpui::test]
    fn a_popover_built_inside_a_high_priority_dialog_draws_above_it_and_takes_presses(
        cx: &mut gpui::TestAppContext,
    ) {
        let (picked, dismissed, card) = (
            Rc::new(Cell::new(0)),
            Rc::new(Cell::new(0)),
            AnchorSlot::default(),
        );
        let window = cx.update(|cx| {
            let (p, d, c) = (picked.clone(), dismissed.clone(), card.clone());
            cx.open_window(Default::default(), move |_, cx| {
                cx.new(|cx| NestedHost {
                    list: cx.new(|_| NestedList { picked: p }),
                    dismissed: d,
                    card: c,
                })
            })
            .unwrap()
        });
        let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
        visual.run_until_parked();
        let (card, item) = (
            card.get().unwrap(),
            visual.debug_bounds("nested-item").expect("the nested list"),
        );
        let at = point(item.right() - px(10.), item.center().y);
        assert!(
            !card.contains(&at),
            "the press must be outside the host card"
        );
        visual.simulate_click(at, Default::default());
        assert_eq!(picked.get(), 1, "the nested list takes the press");
        assert_eq!(dismissed.get(), 0, "and the host dialog stays open");
    }
}
