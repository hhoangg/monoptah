//! The scene the canvas draws: nodes at their place on the canvas and edges routed from handle
//! to handle, built from the pure board and run models.
use super::geometry::{Rect, Seg};
use super::scene::*;
use monocode_core::mono::workflows::{
    board_model::{GROUP_PAD, NODE_H, NODE_W, Point, to_flow},
    model::{LoopSpec, Workflow},
    ordered::Json,
    parse::parse_workflow,
    run_graph_model::RunEdgeState,
};
use serde_json::{Value, json};

fn workflow(nodes: Value, edges: Value) -> Workflow {
    parse_workflow(&Json::from(&json!({
        "id": "wf-aaaaaaaa", "name": "Nightly checks", "inputs": {}, "inputValues": {},
        "nodes": nodes, "edges": edges, "enabled": true, "createdAt": 1,
    })))
    .expect("a workflow")
}

fn session(id: &str) -> Value {
    json!({"id": id, "type": "session", "project": "{{inputs.project}}", "prompt": "Do it",
           "checkout": "root", "output": {}})
}

fn scene_of(w: &Workflow) -> Scene {
    let flow = to_flow(w);
    Scene::build(&flow.nodes, &flow.edges, Vec::new(), "fit".into())
}

fn node<'a>(scene: &'a Scene, id: &str) -> &'a SceneNode {
    scene.nodes.iter().find(|n| n.flow.id == id).expect(id)
}

fn first_and_last(edge: &RoutedEdge) -> (Point, Point) {
    let start = match edge.segs.first() {
        Some(Seg::Move(p)) => *p,
        other => panic!("{other:?}"),
    };
    let end = match edge.segs.last() {
        Some(Seg::Line(p)) => *p,
        other => panic!("{other:?}"),
    };
    (start, end)
}

#[test]
fn an_edge_runs_from_the_bottom_of_a_card_to_the_top_of_the_next() {
    let w = workflow(
        json!([{"id": "trigger", "type": "manual"}, session("dev"), {"id": "report", "type": "report"}]),
        json!([{"from": "trigger", "to": "dev"}, {"from": "dev", "to": "report"}]),
    );
    let scene = scene_of(&w);
    assert_eq!(scene.edges.len(), 2);
    let (a, b) = (&node(&scene, "trigger").rect, &node(&scene, "dev").rect);
    let (start, end) = first_and_last(&scene.edges[0]);
    assert_eq!(
        start,
        Point {
            x: a.x + NODE_W / 2.,
            y: a.y + NODE_H
        }
    );
    assert_eq!(
        end,
        Point {
            x: b.x + NODE_W / 2.,
            y: b.y
        }
    );
    // The arrowhead's tip is where the line ends.
    assert_eq!(scene.edges[0].arrow.unwrap()[0], end);
    assert!(scene.edges[0].label.is_none());
}

#[test]
fn the_two_outs_of_an_if_leave_from_28_and_72_percent_of_its_width() {
    let w = workflow(
        json!([
            {"id": "trigger", "type": "manual"},
            {"id": "check", "type": "if", "condition": "{{trigger.x}} == 1"},
            session("yes"), session("no"),
        ]),
        json!([
            {"from": "trigger", "to": "check"},
            {"from": "check", "to": "yes", "handle": "true"},
            {"from": "check", "to": "no", "handle": "false"},
        ]),
    );
    let scene = scene_of(&w);
    let rect = node(&scene, "check").rect;
    let starts: Vec<f32> = scene
        .edges
        .iter()
        .filter(|e| !e.id.starts_with("trigger"))
        .map(|e| first_and_last(e).0.x - rect.x)
        .collect();
    assert_eq!(starts.len(), 2);
    assert!(
        starts.iter().any(|x| (x - NODE_W * 0.28).abs() < 0.01),
        "{starts:?}"
    );
    assert!(
        starts.iter().any(|x| (x - NODE_W * 0.72).abs() < 0.01),
        "{starts:?}"
    );
}

#[test]
fn a_loop_edge_leaves_the_right_of_the_later_card_and_comes_back_into_the_earlier() {
    let w = workflow(
        json!([{"id": "trigger", "type": "manual"}, session("dev"), session("review")]),
        json!([
            {"from": "trigger", "to": "dev"},
            {"from": "dev", "to": "review"},
            {"from": "review", "to": "dev", "loop": {"maxRounds": 3, "decide": "mono"}},
        ]),
    );
    let scene = scene_of(&w);
    let edge = scene
        .edges
        .iter()
        .find(|e| e.is_loop)
        .expect("the loop edge");
    let (dev, review) = (node(&scene, "dev").rect, node(&scene, "review").rect);
    let (start, end) = first_and_last(edge);
    assert_eq!(
        start,
        Point {
            x: review.right(),
            y: review.y + NODE_H * 0.7
        }
    );
    assert_eq!(
        end,
        Point {
            x: dev.right(),
            y: dev.y + NODE_H * 0.3
        }
    );
    let label = edge.label.as_ref().expect("a label");
    assert_eq!(label.text, "Mono decides \u{b7} max 3");
    // Right of the run beside the cards (never over a card it passes), half way between the
    // two ends.
    assert!(label.at.x > review.right() + 26., "{:?}", label.at);
    assert!(label.at.x - (review.right() + 26.) <= 12.);
    assert!((label.at.y - (start.y + end.y) / 2.).abs() < 0.01);
}

#[test]
fn a_compact_loop_edge_hugs_the_cards_and_has_no_label() {
    let w = workflow(
        json!([{"id": "trigger", "type": "manual"}, session("dev"), session("review")]),
        json!([
            {"from": "trigger", "to": "dev"},
            {"from": "dev", "to": "review"},
            {"from": "review", "to": "dev", "loop": {"maxRounds": 3, "decide": "mono"}},
        ]),
    );
    let flow = to_flow(&w);
    let mut edges = flow.edges.clone();
    for e in &mut edges {
        e.compact = true;
    }
    let scene = Scene::build(&flow.nodes, &edges, Vec::new(), "fit".into());
    let edge = scene.edges.iter().find(|e| e.is_loop).unwrap();
    let review = node(&scene, "review").rect;
    let (start, _) = first_and_last(edge);
    assert!(edge.label.is_none());
    let Some(Seg::Line(p)) = edge.segs.get(1) else {
        panic!("{:?}", edge.segs)
    };
    // The corner is 14 pixels out and rounds by half of that, the most the short run allows.
    assert!(
        (p.x - (review.right() + 7.)).abs() < 0.01,
        "{p:?} {start:?}"
    );
}

#[test]
fn the_loop_label_names_the_decider_and_cuts_a_long_condition() {
    let mono = LoopSpec {
        max_rounds: 3.,
        decide: "mono".into(),
    };
    assert_eq!(loop_label(&mono, false), "Mono decides \u{b7} max 3");
    let short = LoopSpec {
        max_rounds: 2.,
        decide: "  {{r.v}} != \"ok\" ".into(),
    };
    assert_eq!(
        loop_label(&short, false),
        "If {{r.v}} != \"ok\" \u{b7} max 2"
    );
    let long = LoopSpec {
        max_rounds: 10.,
        decide: "{{review.verdict}} != \"approve\" and more".into(),
    };
    let cut = loop_label(&long, false);
    assert_eq!(cut, "If {{review.verdict}} != \"appr\u{2026} \u{b7} max 10");
    assert!(loop_label(&long, true).contains("and more"));
}

#[test]
fn a_for_each_takes_and_gives_its_lines_over_the_column_of_the_cards_inside() {
    let w = workflow(
        json!([
            {"id": "trigger", "type": "manual"},
            {"id": "each", "type": "forEach", "in": "{{trigger.items}}"},
            {"id": "dev", "type": "session", "parent": "each", "project": "{{inputs.project}}",
             "prompt": "x", "checkout": "root", "output": {}},
            {"id": "report", "type": "report"},
        ]),
        json!([{"from": "trigger", "to": "each"}, {"from": "each", "to": "report"}]),
    );
    let scene = scene_of(&w);
    let frame = node(&scene, "each").rect;
    let into = scene
        .edges
        .iter()
        .find(|e| e.id.contains("each") && e.id.contains("trigger"))
        .unwrap();
    assert_eq!(first_and_last(into).1.x, frame.x + GROUP_PAD + NODE_W / 2.);
    let out = scene
        .edges
        .iter()
        .find(|e| e.id.starts_with("each"))
        .unwrap();
    let (start, _) = first_and_last(out);
    assert_eq!(
        start,
        Point {
            x: frame.x + GROUP_PAD + NODE_W / 2.,
            y: frame.bottom()
        }
    );
    // A card inside the frame is placed by the frame's corner, and the frames are drawn first.
    let dev = node(&scene, "dev");
    assert_eq!(dev.rect.x, frame.x + dev.flow.position.x);
    let order: Vec<&str> = scene.nodes.iter().map(|n| n.flow.id.as_str()).collect();
    assert!(order.iter().position(|id| *id == "each") < order.iter().position(|id| *id == "dev"));
}

#[test]
fn the_bounds_cover_the_cards_the_bars_and_the_loop_label() {
    let w = workflow(
        json!([{"id": "trigger", "type": "manual"}, session("dev"), session("review")]),
        json!([
            {"from": "trigger", "to": "dev"},
            {"from": "dev", "to": "review"},
            {"from": "review", "to": "dev", "loop": {"maxRounds": 3, "decide": "mono"}},
        ]),
    );
    let flow = to_flow(&w);
    let bar = Bar {
        id: "bar".into(),
        rect: Rect::new(-40., -60., 50., 20.),
        kind: BarKind::Lane {
            title: "x".into(),
            status: monocode_core::mono::workflows::run_graph_model::RunNodeStatus::Done,
        },
    };
    let scene = Scene::build(&flow.nodes, &flow.edges, vec![bar], "fit".into());
    let all = scene.bounds().unwrap();
    assert!(all.x <= -40. && all.y <= -60.);
    let label = scene.edges.iter().find_map(|e| e.label.as_ref()).unwrap();
    assert!(all.right() >= label.at.x + label.width);
    assert!(
        Scene::build(&[], &[], Vec::new(), String::new())
            .bounds()
            .is_none()
    );
}

#[test]
fn run_states_travel_with_the_edges() {
    let w = workflow(
        json!([{"id": "trigger", "type": "manual"}, {"id": "report", "type": "report"}]),
        json!([{"from": "trigger", "to": "report"}]),
    );
    let flow = to_flow(&w);
    let mut edges = flow.edges.clone();
    edges[0].run = Some(RunEdgeState::Taken);
    let scene = Scene::build(&flow.nodes, &edges, Vec::new(), "fit".into());
    assert_eq!(scene.edges[0].state, Some(RunEdgeState::Taken));
}

#[test]
fn an_edge_to_a_node_that_is_not_drawn_is_left_out() {
    let w = workflow(
        json!([{"id": "trigger", "type": "manual"}, {"id": "report", "type": "report"}]),
        json!([{"from": "trigger", "to": "report"}]),
    );
    let flow = to_flow(&w);
    let only_trigger: Vec<_> = flow
        .nodes
        .iter()
        .filter(|n| n.id == "trigger")
        .cloned()
        .collect();
    let scene = Scene::build(&only_trigger, &flow.edges, Vec::new(), "fit".into());
    assert!(scene.edges.is_empty());
}

#[test]
fn the_loop_label_never_covers_a_card_the_loop_passes() {
    // A loop from the third card back to the first runs past the second.
    let w = workflow(
        json!([{"id": "trigger", "type": "manual"}, session("a"), session("b"), session("c")]),
        json!([
            {"from": "trigger", "to": "a"},
            {"from": "a", "to": "b"},
            {"from": "b", "to": "c"},
            {"from": "c", "to": "a", "loop": {"maxRounds": 3, "decide": "{{c.verdict}} != \"approve\" and a long reason"}},
        ]),
    );
    let scene = scene_of(&w);
    let label = scene.edges.iter().find_map(|e| e.label.as_ref()).unwrap();
    let widest = scene
        .nodes
        .iter()
        .map(|n| n.rect.right())
        .fold(f32::MIN, f32::max);
    assert!(label.at.x > widest, "{} vs {widest}", label.at.x);
}
