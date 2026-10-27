//! What the canvas draws, laid out in graph space: the cards and frames at their place on the
//! canvas, the edges already routed from handle to handle, and the strips (lane titles, item
//! tabs, round selector) that sit above a run's lanes. Built from the pure board and run models,
//! so the board editor draws from the same thing.
use super::geometry::{Rect, Seg, Side, arrow_head, bounds_of, rounded, step_points};
use monocode_core::mono::workflows::{
    board_model::{EdgeKind, FlowEdge, FlowKind, FlowNode, GROUP_PAD, NODE_W, Point, absolute},
    model::LoopSpec,
    ordered::js_number_text,
    run_graph_model::{RunEdgeState, RunNodeStatus},
    template::js_trim,
};

/// How far a plain edge runs clear of a card before it turns, and how round its corners are
/// (React Flow's smooth-step defaults).
const STEP_OFFSET: f32 = 20.;
const STEP_RADIUS: f32 = 5.;
/// The loop edge's run beside the cards (tighter in a compact lane) and its corner radius.
const LOOP_OFFSET: f32 = 26.;
const LOOP_OFFSET_COMPACT: f32 = 14.;
const LOOP_RADIUS: f32 = 12.;
/// A condition longer than this is cut in the loop label.
const CONDITION_MAX: usize = 28;
/// Arrowhead of the edges: the closed 12 x 12 marker at the edge's stroke width.
const ARROW_LENGTH: f32 = 5.;
const ARROW_HALF: f32 = 3.5;
/// Where the true and false outs of an If leave its bottom, and the loop handles on the right.
pub(super) const TRUE_AT: f32 = 0.28;
pub(super) const FALSE_AT: f32 = 0.72;
pub(super) const LOOP_OUT_AT: f32 = 0.7;
pub(super) const LOOP_IN_AT: f32 = 0.3;
/// The loop label is a pill this tall, and about this wide per character.
pub(in crate::monos) const LABEL_H: f32 = 16.;
/// Clear space between the loop line and the left edge of its label.
const LABEL_GAP: f32 = 8.;
const LABEL_CHAR: f32 = 6.2;
const LABEL_PAD: f32 = 12.;

/// A card or a For each frame at its place on the canvas.
#[derive(Clone, Debug)]
pub(in crate::monos) struct SceneNode {
    /// The flow node; its `position` stays relative to its frame, `rect` is the canvas place.
    pub flow: FlowNode,
    pub rect: Rect,
    /// The key that picks it in a run (`<nodeId>|<branch>`).
    pub key: Option<String>,
    /// A card of a lane shown alone while other items wait behind it: drawn with cards stacked
    /// behind it.
    pub stacked: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub(in crate::monos) struct EdgeLabel {
    /// The left edge of the pill, at its vertical middle: right of the loop line, so it never
    /// covers a card the line passes.
    pub at: Point,
    pub text: String,
    /// The text with the whole condition.
    pub full: String,
    /// An estimate of the width, for the bounds only: the pill itself follows its text.
    pub width: f32,
}

/// An edge as drawn: its path on the canvas, its arrowhead and the state of the run.
#[derive(Clone, Debug)]
pub(in crate::monos) struct RoutedEdge {
    pub id: String,
    pub is_loop: bool,
    pub state: Option<RunEdgeState>,
    pub segs: Vec<Seg>,
    pub arrow: Option<[Point; 3]>,
    pub label: Option<EdgeLabel>,
    /// The edge picked on the board.
    pub selected: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub(in crate::monos) struct TabItem {
    pub index: usize,
    pub title: String,
    pub status: Option<RunNodeStatus>,
}

#[derive(Clone, Debug, PartialEq)]
pub(in crate::monos) enum BarKind {
    /// The title above one lane.
    Lane {
        title: String,
        status: RunNodeStatus,
    },
    /// The item tabs of a For each shown one lane at a time.
    Tabs {
        group: String,
        items: Vec<TabItem>,
        selected: usize,
    },
    /// The round selector of a For each a loop ran again.
    Rounds {
        group: String,
        rounds: Vec<u32>,
        selected: u32,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub(in crate::monos) struct Bar {
    pub id: String,
    pub rect: Rect,
    pub kind: BarKind,
}

#[derive(Clone, Debug, Default)]
pub(in crate::monos) struct Scene {
    /// Frames first, then cards, in the order they are drawn.
    pub nodes: Vec<SceneNode>,
    pub edges: Vec<RoutedEdge>,
    pub bars: Vec<Bar>,
    /// A new key means a new picture: the canvas fits its view again.
    pub fit_key: String,
}

/// "Mono decides · max 3", or "If <condition> · max 3".
pub(in crate::monos) fn loop_label(spec: &LoopSpec, full: bool) -> String {
    let condition = js_trim(&spec.decide);
    let shown: String = if condition.chars().count() > CONDITION_MAX && !full {
        let mut cut: String = condition.chars().take(CONDITION_MAX - 1).collect();
        cut.push('\u{2026}');
        cut
    } else {
        condition.to_string()
    };
    let who = if spec.decide == "mono" {
        "Mono decides".to_string()
    } else {
        format!("If {shown}")
    };
    format!("{who} \u{b7} max {}", js_number_text(spec.max_rounds))
}

/// Where a For each takes and gives its lines: over the column of the cards inside it.
pub(super) fn group_anchor(rect: &Rect) -> f32 {
    rect.x + GROUP_PAD + NODE_W / 2.
}

fn route(edge: &FlowEdge, source: &SceneNode, target: &SceneNode) -> RoutedEdge {
    let (s, t) = (source.rect, target.rect);
    let is_loop = edge.kind == EdgeKind::Loop;
    let (from, to, offset, radius, a, b) = if is_loop {
        (
            Side::Right,
            Side::Right,
            if edge.compact {
                LOOP_OFFSET_COMPACT
            } else {
                LOOP_OFFSET
            },
            LOOP_RADIUS,
            Point {
                x: s.right(),
                y: s.y + s.h * LOOP_OUT_AT,
            },
            Point {
                x: t.right(),
                y: t.y + t.h * LOOP_IN_AT,
            },
        )
    } else {
        let x = |n: &SceneNode, at: f32| {
            if n.flow.kind == FlowKind::Group {
                group_anchor(&n.rect)
            } else {
                n.rect.x + n.rect.w * at
            }
        };
        let out = match edge.source_handle {
            Some("true") => TRUE_AT,
            Some("false") => FALSE_AT,
            _ => 0.5,
        };
        (
            Side::Bottom,
            Side::Top,
            STEP_OFFSET,
            STEP_RADIUS,
            Point {
                x: x(source, out),
                y: s.bottom(),
            },
            Point {
                x: x(target, 0.5),
                y: t.y,
            },
        )
    };
    let path = step_points(a, from, b, to, offset);
    let label = (is_loop && !edge.compact)
        .then_some(edge.loop_spec.as_ref())
        .flatten()
        .map(|spec| {
            let text = loop_label(spec, false);
            let width = text.chars().count() as f32 * LABEL_CHAR + LABEL_PAD;
            EdgeLabel {
                at: Point {
                    x: path.line + LABEL_GAP,
                    y: path.label.y,
                },
                text,
                full: loop_label(spec, true),
                width,
            }
        });
    RoutedEdge {
        id: edge.id.clone(),
        is_loop,
        state: edge.run,
        segs: rounded(&path.points, radius),
        arrow: arrow_head(&path.points, ARROW_LENGTH, ARROW_HALF),
        label,
        selected: edge.selected,
    }
}

impl Scene {
    /// Lays out `nodes` (a child's position is relative to its frame) and routes `edges`.
    pub(in crate::monos) fn build(
        nodes: &[FlowNode],
        edges: &[FlowEdge],
        bars: Vec<Bar>,
        fit_key: String,
    ) -> Scene {
        let mut placed: Vec<SceneNode> = nodes
            .iter()
            .map(|n| {
                let at = absolute(nodes, n);
                SceneNode {
                    rect: Rect::new(at.x, at.y, n.width, n.height),
                    key: n.data.run.as_ref().map(|r| r.node_run_key.clone()),
                    stacked: false,
                    flow: n.clone(),
                }
            })
            .collect();
        // Frames under the cards, so a line over a frame is still seen above its fill.
        placed.sort_by_key(|n| n.flow.kind != FlowKind::Group);
        let routed = edges
            .iter()
            .filter_map(|e| {
                let source = placed.iter().find(|n| n.flow.id == e.source)?;
                let target = placed.iter().find(|n| n.flow.id == e.target)?;
                Some(route(e, source, target))
            })
            .collect();
        Scene {
            nodes: placed,
            edges: routed,
            bars,
            fit_key,
        }
    }

    /// Everything the view fits to: the cards, the bars and the loop labels.
    pub(in crate::monos) fn bounds(&self) -> Option<Rect> {
        let labels = self
            .edges
            .iter()
            .filter_map(|e| e.label.as_ref())
            .map(|l| Rect::new(l.at.x, l.at.y - LABEL_H / 2., l.width, LABEL_H));
        bounds_of(
            self.nodes
                .iter()
                .map(|n| n.rect)
                .chain(self.bars.iter().map(|b| b.rect))
                .chain(labels),
        )
    }
}
