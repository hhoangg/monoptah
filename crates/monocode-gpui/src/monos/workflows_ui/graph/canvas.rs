//! The canvas: a [`Scene`] drawn through a pan and zoom view. Cards, frames and strips are
//! positioned divs, the dots behind and the edges are painted, and the view answers the wheel,
//! a drag on the background, a pinch and the zoom buttons. It only reports what was picked
//! ([`GraphEvent`]); the page that owns it decides what a pick means.
use super::geometry::{
    Seg, Side, Viewport, fit_scene, rounded, step_points, zoom_about, zoom_in, zoom_out,
};
use super::hit::{EDGE_SLOP, HandleKind, dragged_to, drop_target, edge_near, past_threshold};
use super::scene::{RoutedEdge, Scene};
use crate::monos::*;
use crate::popover::AnchorSlot;
use gpui::{
    Bounds, ClickEvent, DispatchPhase, FocusHandle, KeyDownEvent, MouseMoveEvent, MouseUpEvent,
    PathBuilder, PinchEvent, Pixels, ScrollHandle, ScrollWheelEvent, canvas, fill, point,
};
use monocode_core::mono::workflows::{board_model::Point, run_graph_model::RunEdgeState};
use std::rc::Rc;

/// What the canvas tells its owner.
pub(in crate::monos) enum GraphEvent {
    /// A card or frame was picked, by its run key.
    Select(String),
    /// An item tab was picked.
    PickItem { group: String, index: usize },
    /// A round tab was picked.
    PickRound { group: String, round: u32 },
    /// Editing: a card was pressed and let go without being dragged.
    Click(String),
    /// Editing: a card is being dragged; `to` is where its top left corner is now, on the
    /// canvas.
    Drag { id: String, to: Point },
    /// Editing: the drag of a card ended.
    Drop(String),
    /// Editing: a connection from `source` (leaving by the If handle `handle`, if any) was
    /// dropped on `target`, at `at` in the window.
    Connect {
        source: String,
        handle: Option<String>,
        target: String,
        at: gpui::Point<Pixels>,
    },
    /// Editing: an edge was clicked.
    EdgeClick(String),
    /// Editing: the empty canvas was clicked.
    PaneClick,
    /// Editing: Delete or Backspace with the canvas focused.
    Delete,
}

/// Whether a connection from a node (by the If handle, if any) to another may be made.
pub(in crate::monos) type Connector = Rc<dyn Fn(&str, Option<&str>, &str) -> bool>;

/// A card being dragged: which, where the pointer went down, and where the card was.
struct NodeDrag {
    id: String,
    start: gpui::Point<Pixels>,
    origin: Point,
    moved: bool,
}

/// A connection being drawn from a handle to the pointer.
#[derive(Clone)]
pub(super) struct Connection {
    pub source: String,
    pub kind: HandleKind,
    pub from: Point,
    pub to: Point,
    /// The card it would connect to if let go now.
    pub hover: Option<String>,
    pub at: gpui::Point<Pixels>,
}

/// A drag on the background: where it began and the view then.
#[derive(Clone, Copy)]
struct Drag {
    start: gpui::Point<Pixels>,
    view: Viewport,
}

/// A strip of tabs that scrolls on its own.
#[derive(Default)]
pub(super) struct Strip {
    pub handle: ScrollHandle,
    /// The tab last scrolled into view.
    pub shown: Option<usize>,
}

pub(in crate::monos) struct GraphCanvas {
    pub(super) scene: Rc<Scene>,
    /// The Mono's colour, the accent of the drawing.
    pub(super) color: u32,
    /// The run key of the picked card.
    pub(super) selected: Option<String>,
    pub(super) view: Viewport,
    /// The `fit_key` the view was last fitted for.
    fitted: Option<String>,
    /// Where the canvas is on screen, written while it is laid out.
    pub(super) size: AnchorSlot,
    drag: Option<Drag>,
    pub(super) strips: HashMap<String, Strip>,
    /// The focus of each card, so Tab reaches it and Enter or Space picks it.
    pub(super) focus: HashMap<String, gpui::FocusHandle>,
    /// The board editor: cards can be dragged, wired and deleted.
    pub(super) editable: bool,
    connector: Option<Connector>,
    node_drag: Option<NodeDrag>,
    pub(super) connection: Option<Connection>,
    /// Focused when the canvas is pressed, so the Delete key reaches it.
    root_focus: Option<FocusHandle>,
}

impl EventEmitter<GraphEvent> for GraphCanvas {}

/// Pixels a click may move and still be a click, not the end of a pan.
const CLICK_SLOP: f64 = 4.;
/// Dots of the background: one every this many graph pixels.
const DOT_GAP: f32 = 18.;

impl GraphCanvas {
    pub(in crate::monos) fn new(color: u32) -> Self {
        Self {
            scene: Rc::new(Scene::default()),
            color,
            selected: None,
            view: Viewport::default(),
            fitted: None,
            size: AnchorSlot::default(),
            drag: None,
            strips: HashMap::new(),
            focus: HashMap::new(),
            editable: false,
            connector: None,
            node_drag: None,
            connection: None,
            root_focus: None,
        }
    }

    pub(in crate::monos) fn set_scene(&mut self, scene: Scene, cx: &mut Context<Self>) {
        // What a node that is gone leaves behind goes with it.
        self.focus
            .retain(|id, _| scene.nodes.iter().any(|n| n.flow.id == *id));
        self.strips
            .retain(|id, _| scene.bars.iter().any(|b| b.id == *id));
        self.scene = Rc::new(scene);
        cx.notify();
    }

    /// The board editor's canvas: cards can be dragged, wired and deleted. `connector` says
    /// which connections may be made.
    pub(in crate::monos) fn set_editable(&mut self, connector: Connector) {
        self.editable = true;
        self.connector = Some(connector);
    }

    /// Replaces what says which connections may be made (it follows the workflow).
    pub(in crate::monos) fn set_connector(&mut self, connector: Connector) {
        self.connector = Some(connector);
    }

    /// Whether a connection from `source` by `handle` to `target` may be made.
    pub(in crate::monos) fn may_connect(
        &self,
        source: &str,
        handle: Option<&str>,
        target: &str,
    ) -> bool {
        self.connector
            .as_ref()
            .is_some_and(|connector| connector(source, handle, target))
    }

    /// A drag of a card began.
    pub(super) fn begin_drag(&mut self, id: &str, origin: Point, at: gpui::Point<Pixels>) {
        self.node_drag = Some(NodeDrag {
            id: id.to_string(),
            start: at,
            origin,
            moved: false,
        });
    }

    /// A connection is drawn from the handle of a card.
    pub(super) fn begin_connection(
        &mut self,
        source: &str,
        kind: HandleKind,
        from: Point,
        at: gpui::Point<Pixels>,
    ) {
        self.connection = Some(Connection {
            source: source.to_string(),
            kind,
            from,
            to: from,
            hover: None,
            at,
        });
    }

    /// The focus of the canvas itself.
    pub(super) fn root_focus(&mut self, cx: &mut Context<Self>) -> FocusHandle {
        self.root_focus
            .get_or_insert_with(|| cx.focus_handle())
            .clone()
    }

    /// The cards that have a focus handle.
    #[cfg(test)]
    pub(in crate::monos) fn focus_ids(&self) -> Vec<String> {
        let mut ids: Vec<String> = self.focus.keys().cloned().collect();
        ids.sort();
        ids
    }

    #[cfg(test)]
    pub(in crate::monos) fn card_focus(&self, id: &str) -> Option<FocusHandle> {
        self.focus.get(id).cloned()
    }

    #[cfg(test)]
    pub(in crate::monos) fn dragging_node(&self) -> bool {
        self.node_drag.is_some()
    }

    #[cfg(test)]
    pub(in crate::monos) fn connecting(&self) -> bool {
        self.connection.is_some()
    }

    pub(in crate::monos) fn set_selected(&mut self, key: Option<String>, cx: &mut Context<Self>) {
        if self.selected != key {
            self.selected = key;
            cx.notify();
        }
    }

    #[cfg(test)]
    pub(in crate::monos) fn view(&self) -> Viewport {
        self.view
    }

    /// The scene being drawn.
    #[cfg(test)]
    pub(in crate::monos) fn scene(&self) -> &Scene {
        &self.scene
    }

    fn area(&self) -> Option<Bounds<Pixels>> {
        self.size.get()
    }

    /// Fits the view to the scene now, or when the size is known.
    pub(in crate::monos) fn fit(&mut self, cx: &mut Context<Self>) {
        self.fitted = None;
        self.fit_if_needed();
        cx.notify();
    }

    fn fit_if_needed(&mut self) {
        let Some(area) = self.area() else {
            return;
        };
        if self.fitted.as_deref() == Some(self.scene.fit_key.as_str()) {
            return;
        }
        let size = monocode_core::mono::workflows::board_model::Size {
            width: area.size.width.as_f32(),
            height: area.size.height.as_f32(),
        };
        let Some(view) = fit_scene(self.scene.bounds(), size) else {
            return;
        };
        self.view = view;
        self.fitted = Some(self.scene.fit_key.clone());
    }

    /// A point of the window as a point of the canvas.
    fn local(&self, at: gpui::Point<Pixels>) -> Point {
        let origin = self.area().map(|a| a.origin).unwrap_or_default();
        Point {
            x: (at.x - origin.x).as_f32(),
            y: (at.y - origin.y).as_f32(),
        }
    }

    fn zoom_to(&mut self, zoom: f32, anchor: Point, cx: &mut Context<Self>) {
        self.view = zoom_about(self.view, zoom, anchor);
        cx.notify();
    }

    /// A zoom button: about the middle of the canvas.
    pub(in crate::monos) fn zoom_button(&mut self, zoom_in_not_out: bool, cx: &mut Context<Self>) {
        let middle = self.area().map_or(Point::default(), |a| Point {
            x: a.size.width.as_f32() / 2.,
            y: a.size.height.as_f32() / 2.,
        });
        let zoom = if zoom_in_not_out {
            zoom_in(self.view.zoom)
        } else {
            zoom_out(self.view.zoom)
        };
        self.zoom_to(zoom, middle, cx);
    }

    fn wheel(&mut self, event: &ScrollWheelEvent, cx: &mut Context<Self>) {
        let delta = event.delta.pixel_delta(px(16.));
        if event.modifiers.control || event.modifiers.platform {
            // GPUI's wheel deltas have the opposite sign to DOM wheel deltas.
            let zoom = self.view.zoom * (delta.y.as_f32() * 0.01).exp();
            self.zoom_to(zoom, self.local(event.position), cx);
        } else {
            self.view.x += delta.x.as_f32();
            self.view.y += delta.y.as_f32();
            cx.notify();
        }
        cx.stop_propagation();
    }

    fn pinch(&mut self, event: &PinchEvent, cx: &mut Context<Self>) {
        let zoom = self.view.zoom * (1. + event.delta);
        self.zoom_to(zoom, self.local(event.position), cx);
        cx.stop_propagation();
    }

    fn screen(at: gpui::Point<Pixels>) -> Point {
        Point {
            x: at.x.as_f32(),
            y: at.y.as_f32(),
        }
    }

    fn moved(&mut self, event: &MouseMoveEvent, cx: &mut Context<Self>) {
        if self.node_drag.is_some() || self.connection.is_some() {
            if event.pressed_button != Some(MouseButton::Left) {
                // The button was let go where this window did not see it.
                self.finish(event.position, cx);
                return;
            }
            self.edit_moved(event, cx);
            return;
        }
        let Some(drag) = self.drag else {
            return;
        };
        if event.pressed_button != Some(MouseButton::Left) {
            self.drag = None;
            return;
        }
        self.view = Viewport {
            x: drag.view.x + (event.position.x - drag.start.x).as_f32(),
            y: drag.view.y + (event.position.y - drag.start.y).as_f32(),
            ..drag.view
        };
        cx.notify();
    }

    /// The pointer moved with a card or a connection in hand.
    fn edit_moved(&mut self, event: &MouseMoveEvent, cx: &mut Context<Self>) {
        let zoom = self.view.zoom;
        if let Some(drag) = &mut self.node_drag {
            let (start, now) = (Self::screen(drag.start), Self::screen(event.position));
            if !drag.moved && !past_threshold(start, now) {
                return;
            }
            drag.moved = true;
            let to = dragged_to(drag.origin, start, now, zoom);
            let id = drag.id.clone();
            cx.emit(GraphEvent::Drag { id, to });
            return;
        }
        let to = self.view.to_graph(self.local(event.position));
        let scene = self.scene.clone();
        let hover = self.connection.as_ref().and_then(|c| {
            let handle = match &c.kind {
                HandleKind::Out(handle) => handle.as_deref(),
                _ => None,
            };
            drop_target(to, &scene.nodes, &c.source, self.view.zoom)
                .filter(|target| self.may_connect(&c.source, handle, target))
        });
        if let Some(c) = &mut self.connection {
            c.to = to;
            c.hover = hover;
            c.at = event.position;
        }
        cx.notify();
    }

    /// The button went up at `at`: a drag ends (a click when it never moved), a connection
    /// lands on the card under it, and a press on the empty canvas that did not pan is a click.
    fn finish(&mut self, at: gpui::Point<Pixels>, cx: &mut Context<Self>) {
        if let Some(drag) = self.node_drag.take() {
            cx.emit(if drag.moved {
                GraphEvent::Drop(drag.id)
            } else {
                GraphEvent::Click(drag.id)
            });
            cx.notify();
            return;
        }
        if let Some(c) = self.connection.take() {
            if let Some(target) = c.hover {
                let handle = match c.kind {
                    HandleKind::Out(handle) => handle,
                    _ => None,
                };
                cx.emit(GraphEvent::Connect {
                    source: c.source,
                    handle,
                    target,
                    at: c.at,
                });
            }
            cx.notify();
            return;
        }
        let Some(drag) = self.drag.take() else {
            return;
        };
        if !self.editable || (at - drag.start).magnitude() > CLICK_SLOP {
            return;
        }
        let p = self.view.to_graph(self.local(at));
        match edge_near(p, &self.scene.edges, EDGE_SLOP / self.view.zoom) {
            Some(edge) => cx.emit(GraphEvent::EdgeClick(edge)),
            None => cx.emit(GraphEvent::PaneClick),
        }
    }

    fn key_down(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        if self.editable
            && !event.keystroke.modifiers.modified()
            && matches!(event.keystroke.key.as_str(), "delete" | "backspace")
        {
            cx.emit(GraphEvent::Delete);
            cx.stop_propagation();
        }
    }

    /// Whether a click is a click and not the end of a drag across the card.
    pub(super) fn is_click(event: &ClickEvent) -> bool {
        match event {
            ClickEvent::Mouse(m) => (m.up.position - m.down.position).magnitude() <= CLICK_SLOP,
            _ => true,
        }
    }

    /// The window position of a graph point, as a position inside the canvas element.
    pub(super) fn at(&self, p: Point) -> (Pixels, Pixels) {
        let s = self.view.to_screen(p);
        (px(s.x), px(s.y))
    }

    pub(super) fn px(&self, graph: f32) -> Pixels {
        px(graph * self.view.zoom)
    }

    pub(super) fn scaled(&self, graph: f32) -> f32 {
        graph * self.view.zoom
    }

    fn dots(&self) -> impl IntoElement {
        let view = self.view;
        canvas(
            |_, _, _| (),
            move |bounds, _, window, _| {
                let gap = DOT_GAP * view.zoom;
                // Closer than this the dots are a fog, and not worth the quads.
                if gap < 10. {
                    return;
                }
                let side = px((view.zoom).max(1.));
                let color = content(0.12);
                let first = |offset: f32| offset.rem_euclid(gap);
                let mut x = first(view.x);
                while x < bounds.size.width.as_f32() {
                    let mut y = first(view.y);
                    while y < bounds.size.height.as_f32() {
                        window.paint_quad(fill(
                            Bounds::new(
                                bounds.origin + point(px(x), px(y)),
                                gpui::size(side, side),
                            ),
                            color,
                        ));
                        y += gap;
                    }
                    x += gap;
                }
            },
        )
        .absolute()
        .size_full()
    }

    /// Follows a drag that began on the canvas wherever the pointer goes while the button is
    /// held, as the other drags of the app do (window-level mouse events).
    fn drag_tracking(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let weak = cx.entity().downgrade();
        canvas(
            |_, _, _| (),
            move |_, _, window, _| {
                let moving = weak.clone();
                window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
                    if phase == DispatchPhase::Bubble {
                        let _ = moving.update(cx, |this, cx| this.moved(event, cx));
                    }
                });
                let released = weak.clone();
                window.on_mouse_event(move |event: &MouseUpEvent, phase, _, cx| {
                    if phase == DispatchPhase::Bubble {
                        let _ = released.update(cx, |this, cx| this.finish(event.position, cx));
                    }
                });
            },
        )
        .absolute()
        .size_full()
    }

    fn edges(&self) -> impl IntoElement {
        let (scene, view, accent) = (self.scene.clone(), self.view, self.color);
        let connection = self.connection.clone();
        canvas(
            |_, _, _| (),
            move |bounds, _, window, _| {
                for edge in &scene.edges {
                    paint_edge(edge, view, accent, bounds.origin, window);
                }
                if let Some(c) = &connection {
                    paint_connection(c, view, accent, bounds.origin, window);
                }
            },
        )
        .absolute()
        .size_full()
    }

    fn controls(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .absolute()
            .bottom(px(8.))
            .left(px(8.))
            .flex()
            .flex_col()
            .p(px(2.))
            .rounded_lg()
            .border_1()
            .border_color(content(0.1))
            .bg(floating_fill())
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                icon_button("wf-zoom-in", "Zoom in", "plus")
                    .debug_selector(|| "wf-zoom-in".into())
                    .on_click(cx.listener(|this, _, _, cx| this.zoom_button(true, cx))),
            )
            .child(
                icon_button("wf-zoom-out", "Zoom out", "minus")
                    .debug_selector(|| "wf-zoom-out".into())
                    .on_click(cx.listener(|this, _, _, cx| this.zoom_button(false, cx))),
            )
            .child(
                icon_button("wf-fit", "Fit to screen", "maximize")
                    .debug_selector(|| "wf-fit".into())
                    .on_click(cx.listener(|this, _, _, cx| this.fit(cx))),
            )
    }
}

impl Render for GraphCanvas {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.fit_if_needed();
        let ready =
            self.area().is_some() && self.fitted.as_deref() == Some(self.scene.fit_key.as_str());
        let scene = self.scene.clone();
        let focus = self.editable.then(|| self.root_focus(cx));
        let mut root = div()
            .id("wf-graph")
            .debug_selector(|| "wf-graph".into())
            .relative()
            .size_full()
            .overflow_hidden()
            .when_some(focus, |d, focus| {
                d.track_focus(&focus).on_key_down(
                    cx.listener(|this, event: &KeyDownEvent, _, cx| this.key_down(event, cx)),
                )
            })
            .child(self.size.layout_probe())
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &gpui::MouseDownEvent, window, cx| {
                    if this.editable {
                        let focus = this.root_focus(cx);
                        window.focus(&focus, cx);
                    }
                    this.drag = Some(Drag {
                        start: event.position,
                        view: this.view,
                    });
                }),
            )
            .child(self.drag_tracking(cx))
            .on_scroll_wheel(
                cx.listener(|this, event: &ScrollWheelEvent, _, cx| this.wheel(event, cx)),
            )
            .on_pinch(cx.listener(|this, event: &PinchEvent, _, cx| this.pinch(event, cx)));
        if ready {
            use monocode_core::mono::workflows::board_model::FlowKind;
            root = root.child(self.dots());
            for node in scene
                .nodes
                .iter()
                .filter(|n| n.flow.kind == FlowKind::Group)
            {
                root = root.children(self.node(node, cx));
            }
            // Lines over the frames, under the cards.
            root = root.child(self.edges());
            for node in scene
                .nodes
                .iter()
                .filter(|n| n.flow.kind != FlowKind::Group)
            {
                root = root.children(self.node(node, cx));
            }
            for bar in &scene.bars {
                root = root.child(self.bar(bar, cx));
            }
            for edge in &scene.edges {
                if let Some(label) = &edge.label {
                    root = root.child(self.edge_label(&edge.id, label));
                }
            }
        }
        root.child(self.controls(cx))
    }
}

/// Strokes the path of `segs` (on the canvas) through the view.
fn stroke_segs(
    segs: &[Seg],
    view: Viewport,
    origin: gpui::Point<Pixels>,
    width: f32,
    dash: Option<[f32; 2]>,
    color: gpui::Hsla,
    window: &mut Window,
) {
    let to = |p: Point| {
        let s = view.to_screen(p);
        origin + point(px(s.x), px(s.y))
    };
    let width = (width * view.zoom).max(0.75);
    let mut line = PathBuilder::stroke(px(width));
    if let Some([on, off]) = dash {
        line = line.dash_array(&[px(on * view.zoom), px(off * view.zoom)]);
    }
    for seg in segs {
        match *seg {
            Seg::Move(p) => line.move_to(to(p)),
            Seg::Line(p) => line.line_to(to(p)),
            Seg::Quad { ctrl, to: end } => line.curve_to(to(end), to(ctrl)),
        }
    }
    if let Ok(path) = line.build() {
        window.paint_path(path, color);
    }
}

fn paint_edge(
    edge: &RoutedEdge,
    view: Viewport,
    accent: u32,
    origin: gpui::Point<Pixels>,
    window: &mut Window,
) {
    let accent = gpui::Hsla::from(rgb(accent));
    let done = gpui::Hsla::from(rgb(0x34d399)).opacity(0.75);
    let (color, width, dash): (gpui::Hsla, f32, Option<[f32; 2]>) = if edge.is_loop {
        let color = if edge.state == Some(RunEdgeState::Idle) {
            accent.opacity(0.35)
        } else {
            accent
        };
        // An idle loop is dashed like any idle line (the later CSS rule wins in React).
        let dash = if edge.state == Some(RunEdgeState::Idle) {
            [3., 4.]
        } else {
            [5., 4.]
        };
        (color, 1.5, Some(dash))
    } else {
        match edge.state {
            Some(RunEdgeState::Taken) => (done, 1.5, None),
            Some(RunEdgeState::Active) => (accent, 1.5, None),
            Some(RunEdgeState::Idle) => (content(0.2), 1.25, Some([3., 4.])),
            None => (content(0.26), 1.25, None),
        }
    };
    // The picked edge is drawn stronger.
    let (color, width) = if edge.selected && !edge.is_loop {
        (content(0.55), width + 0.25)
    } else {
        (color, width)
    };
    stroke_segs(&edge.segs, view, origin, width, dash, color, window);
    if let Some([tip, a, b]) = edge.arrow {
        let to = |p: Point| {
            let s = view.to_screen(p);
            origin + point(px(s.x), px(s.y))
        };
        let mut head = PathBuilder::fill();
        head.move_to(to(tip));
        head.line_to(to(a));
        head.line_to(to(b));
        head.close();
        if let Ok(path) = head.build() {
            window.paint_path(path, if edge.is_loop { accent } else { color });
        }
    }
}

/// The line from a handle to the pointer while a connection is drawn.
fn paint_connection(
    c: &Connection,
    view: Viewport,
    accent: u32,
    origin: gpui::Point<Pixels>,
    window: &mut Window,
) {
    let (from, to) = match c.kind {
        HandleKind::LoopOut | HandleKind::LoopIn => (Side::Right, Side::Right),
        _ => (Side::Bottom, Side::Top),
    };
    let path = step_points(c.from, from, c.to, to, 20.);
    let segs = rounded(&path.points, 5.);
    stroke_segs(
        &segs,
        view,
        origin,
        1.25,
        None,
        gpui::Hsla::from(rgb(accent)),
        window,
    );
}
