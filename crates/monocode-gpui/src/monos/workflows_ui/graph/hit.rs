//! The maths of editing on the canvas, with no window in it: where the handles of a card are, how
//! far a drag moved a node, which card a connection was dropped on and which edge a click was on.
use super::geometry::{Rect, Seg};
use super::scene::{LOOP_IN_AT, LOOP_OUT_AT, RoutedEdge, SceneNode, group_anchor, out_at};
use monocode_core::mono::workflows::{
    board_model::{FlowKind, GROUP_HEADER, Point, out_handles},
    model::NodeKind,
};

/// A drag starts once the pointer is more than this many screen pixels from where it went down
/// (xyflow's `nodeDragThreshold`).
pub(super) const DRAG_THRESHOLD: f32 = 2.;
/// A connection dropped this close to a card (screen pixels) lands on it (xyflow's
/// `connectionRadius`).
pub(super) const DROP_RADIUS: f32 = 20.;
/// A click this close to an edge (graph pixels) picks it.
pub(super) const EDGE_SLOP: f32 = 6.;

/// Which of a card's handles.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::monos) enum HandleKind {
    /// Where lines arrive, at the top.
    In,
    /// Where lines leave, at the bottom; an If has one for true and one for false, a Switch one
    /// for each case and one for the default.
    Out(Option<String>),
    /// Where a loop arrives and leaves, on the right.
    LoopIn,
    LoopOut,
}

#[derive(Clone, Debug, PartialEq)]
pub(in crate::monos) struct Handle {
    pub kind: HandleKind,
    /// On the canvas.
    pub at: Point,
}

/// The handles a card has, at the places its edges leave and arrive.
pub(in crate::monos) fn handles_of(node: &SceneNode) -> Vec<Handle> {
    let r = node.rect;
    let handle = |kind, x, y| Handle {
        kind,
        at: Point { x, y },
    };
    if node.flow.kind == FlowKind::Group {
        let x = group_anchor(&r);
        return vec![
            handle(HandleKind::In, x, r.y),
            handle(HandleKind::Out(None), x, r.bottom()),
        ];
    }
    let kind = &node.flow.data.node.kind;
    let trigger = matches!(kind, NodeKind::Schedule { .. } | NodeKind::Manual);
    let report = matches!(kind, NodeKind::Report { .. });
    let mut out = Vec::new();
    if !trigger {
        out.push(handle(HandleKind::In, r.x + r.w / 2., r.y));
        if !report {
            out.push(handle(
                HandleKind::LoopIn,
                r.right(),
                r.y + r.h * LOOP_IN_AT,
            ));
        }
    }
    match kind {
        NodeKind::If { .. } | NodeKind::Switch { .. } => {
            let outs = out_handles(&node.flow.data.node);
            for (index, name) in outs.iter().enumerate() {
                out.push(handle(
                    HandleKind::Out(Some(name.clone())),
                    r.x + r.w * out_at(index, outs.len()),
                    r.bottom(),
                ));
            }
        }
        _ if !report => out.push(handle(HandleKind::Out(None), r.x + r.w / 2., r.bottom())),
        _ => {}
    }
    if matches!(
        kind,
        NodeKind::Session { .. } | NodeKind::Command { .. } | NodeKind::AskMono { .. }
    ) {
        out.push(handle(
            HandleKind::LoopOut,
            r.right(),
            r.y + r.h * LOOP_OUT_AT,
        ));
    }
    out
}

/// `r` grown by `by` on every side.
pub(in crate::monos) fn inflate(r: Rect, by: f32) -> Rect {
    Rect::new(r.x - by, r.y - by, r.w + by * 2., r.h + by * 2.)
}

fn contains(r: Rect, p: Point) -> bool {
    p.x >= r.x && p.x <= r.right() && p.y >= r.y && p.y <= r.bottom()
}

/// Where a node dragged from `origin` (its place when the drag began, on the canvas) goes when
/// the pointer went from `start` to `now` (screen pixels) at `zoom`.
pub(in crate::monos) fn dragged_to(origin: Point, start: Point, now: Point, zoom: f32) -> Point {
    Point {
        x: origin.x + (now.x - start.x) / zoom,
        y: origin.y + (now.y - start.y) / zoom,
    }
}

/// Whether the pointer moved far enough from where it went down to be a drag, not a click.
pub(in crate::monos) fn past_threshold(start: Point, now: Point) -> bool {
    ((now.x - start.x).powi(2) + (now.y - start.y).powi(2)).sqrt() > DRAG_THRESHOLD
}

/// How far `p` is from the rectangle (0 inside it).
fn gap(r: Rect, p: Point) -> f32 {
    let dx = (r.x - p.x).max(0.).max(p.x - r.right());
    let dy = (r.y - p.y).max(0.).max(p.y - r.bottom());
    (dx * dx + dy * dy).sqrt()
}

/// The node a connection from `source` dropped at `at` (on the canvas) lands on: the nearest card
/// within the drop radius, else a For each by its header (the place of its top handle).
pub(in crate::monos) fn drop_target(
    at: Point,
    nodes: &[SceneNode],
    source: &str,
    zoom: f32,
) -> Option<String> {
    // xyflow's `connectionRadius` is in screen pixels.
    let radius = DROP_RADIUS / zoom;
    let nearest = |group: bool| {
        nodes
            .iter()
            .filter(|n| n.flow.id != source && (n.flow.kind == FlowKind::Group) == group)
            .filter_map(|n| {
                let area = if group {
                    Rect::new(n.rect.x, n.rect.y, n.rect.w, GROUP_HEADER)
                } else {
                    n.rect
                };
                (contains(inflate(area, radius), at)).then(|| (gap(area, at), n))
            })
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .map(|(_, n)| n.flow.id.clone())
    };
    nearest(false).or_else(|| nearest(true))
}

fn near_segment(p: Point, a: Point, b: Point) -> f32 {
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    let span = dx * dx + dy * dy;
    let t = if span == 0. {
        0.
    } else {
        (((p.x - a.x) * dx + (p.y - a.y) * dy) / span).clamp(0., 1.)
    };
    let (x, y) = (a.x + t * dx, a.y + t * dy);
    ((p.x - x).powi(2) + (p.y - y).powi(2)).sqrt()
}

/// How far `p` is from the drawn path.
fn path_gap(p: Point, segs: &[Seg]) -> f32 {
    let mut best = f32::INFINITY;
    let mut at = Point::default();
    for seg in segs {
        match *seg {
            Seg::Move(to) => at = to,
            Seg::Line(to) => {
                best = best.min(near_segment(p, at, to));
                at = to;
            }
            Seg::Quad { ctrl, to } => {
                let mut last = at;
                for step in 1..=8 {
                    let t = step as f32 / 8.;
                    let u = 1. - t;
                    let next = Point {
                        x: u * u * at.x + 2. * u * t * ctrl.x + t * t * to.x,
                        y: u * u * at.y + 2. * u * t * ctrl.y + t * t * to.y,
                    };
                    best = best.min(near_segment(p, last, next));
                    last = next;
                }
                at = to;
            }
        }
    }
    best
}

/// The edge whose line passes within `slop` of `p`, the closest if several do.
pub(in crate::monos) fn edge_near(p: Point, edges: &[RoutedEdge], slop: f32) -> Option<String> {
    edges
        .iter()
        .map(|e| (path_gap(p, &e.segs), e))
        .filter(|(gap, _)| *gap <= slop)
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, e)| e.id.clone())
}

#[cfg(test)]
pub(in crate::monos) fn edge_at(p: Point, edges: &[RoutedEdge]) -> Option<String> {
    edge_near(p, edges, EDGE_SLOP)
}
