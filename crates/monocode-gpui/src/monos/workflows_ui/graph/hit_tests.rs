//! The maths of editing on the canvas: where the handles of a card are, how far a drag moved a
//! node, which card a connection was dropped on and which edge a click was on.
use super::geometry::{Rect, Seg};
use super::hit::*;
use super::scene::{RoutedEdge, Scene, SceneNode};
use monocode_core::mono::workflows::{
    board_model::{GROUP_HEADER, GROUP_PAD, NODE_H, NODE_W, Point, to_flow},
    model::Workflow,
    ordered::Json,
    parse::parse_workflow,
};
use serde_json::{Value, json};

fn workflow(nodes: Value, edges: Value) -> Workflow {
    parse_workflow(&Json::from(&json!({
        "id": "wf-aaaaaaaa", "name": "Nightly checks", "inputs": {}, "inputValues": {},
        "nodes": nodes, "edges": edges, "enabled": true, "createdAt": 1,
    })))
    .expect("a workflow")
}

fn session(id: &str, y: f64) -> Value {
    json!({"id": id, "type": "session", "project": "{{inputs.project}}", "prompt": "Do it",
           "checkout": "root", "output": {}, "ui": {"x": 0, "y": y}})
}

fn scene() -> Scene {
    let w = workflow(
        json!([
            {"id": "trigger", "type": "manual", "ui": {"x": 0, "y": 0}},
            session("a", 100.),
            {"id": "check", "type": "if", "decide": "mono", "ui": {"x": 0, "y": 200}},
            {"id": "each", "type": "forEach", "in": "{{a.list}}", "ui": {"x": 400, "y": 0}},
            {"id": "report", "type": "report", "ui": {"x": 0, "y": 300}},
        ]),
        json!([{"from": "trigger", "to": "a"}, {"from": "a", "to": "check"}]),
    );
    let flow = to_flow(&w);
    Scene::build(&flow.nodes, &flow.edges, Vec::new(), "k".into())
}

fn node<'a>(scene: &'a Scene, id: &str) -> &'a SceneNode {
    scene.nodes.iter().find(|n| n.flow.id == id).expect(id)
}

fn kinds(scene: &Scene, id: &str) -> Vec<HandleKind> {
    handles_of(node(scene, id)).iter().map(|h| h.kind).collect()
}

#[test]
fn a_trigger_has_only_an_out_handle() {
    assert_eq!(kinds(&scene(), "trigger"), vec![HandleKind::Out(None)]);
}

#[test]
fn a_session_has_in_out_and_both_loop_handles() {
    let kinds = kinds(&scene(), "a");
    assert!(kinds.contains(&HandleKind::In));
    assert!(kinds.contains(&HandleKind::Out(None)));
    assert!(kinds.contains(&HandleKind::LoopIn));
    assert!(kinds.contains(&HandleKind::LoopOut));
}

#[test]
fn an_if_leaves_by_true_and_false_and_cannot_start_a_loop() {
    let kinds = kinds(&scene(), "check");
    assert!(kinds.contains(&HandleKind::Out(Some("true"))));
    assert!(kinds.contains(&HandleKind::Out(Some("false"))));
    assert!(!kinds.contains(&HandleKind::Out(None)));
    assert!(!kinds.contains(&HandleKind::LoopOut));
    assert!(kinds.contains(&HandleKind::LoopIn));
}

#[test]
fn a_report_only_takes_lines_in() {
    assert_eq!(kinds(&scene(), "report"), vec![HandleKind::In]);
}

#[test]
fn a_for_each_takes_and_gives_over_the_column_of_its_cards() {
    let scene = scene();
    let group = node(&scene, "each");
    let handles = handles_of(group);
    let x = group.rect.x + GROUP_PAD + NODE_W / 2.;
    let top = handles.iter().find(|h| h.kind == HandleKind::In).unwrap();
    let bottom = handles
        .iter()
        .find(|h| h.kind == HandleKind::Out(None))
        .unwrap();
    assert_eq!(top.at, Point { x, y: group.rect.y });
    assert_eq!(
        bottom.at,
        Point {
            x,
            y: group.rect.bottom()
        }
    );
    assert!(!handles.iter().any(|h| h.kind == HandleKind::LoopOut));
}

#[test]
fn the_handles_sit_where_the_edges_leave_and_arrive() {
    let scene = scene();
    let a = node(&scene, "a");
    let at = |kind| {
        handles_of(a)
            .iter()
            .find(|h| h.kind == kind)
            .map(|h| h.at)
            .unwrap()
    };
    assert_eq!(
        at(HandleKind::In),
        Point {
            x: a.rect.x + NODE_W / 2.,
            y: a.rect.y
        }
    );
    assert_eq!(
        at(HandleKind::Out(None)),
        Point {
            x: a.rect.x + NODE_W / 2.,
            y: a.rect.bottom()
        }
    );
    assert_eq!(
        at(HandleKind::LoopOut),
        Point {
            x: a.rect.right(),
            y: a.rect.y + NODE_H * 0.7
        }
    );
    assert_eq!(
        at(HandleKind::LoopIn),
        Point {
            x: a.rect.right(),
            y: a.rect.y + NODE_H * 0.3
        }
    );
    // The edge from a to the If starts exactly on the out handle.
    let edge = &scene
        .edges
        .iter()
        .find(|e| e.id.starts_with("a->"))
        .unwrap();
    assert_eq!(edge.segs[0], Seg::Move(at(HandleKind::Out(None))));
}

#[test]
fn a_drag_moves_a_node_by_the_pointer_travel_in_graph_pixels() {
    let origin = Point { x: 100., y: 50. };
    let start = Point { x: 300., y: 200. };
    let now = Point { x: 360., y: 170. };
    assert_eq!(
        dragged_to(origin, start, now, 1.),
        Point { x: 160., y: 20. }
    );
    assert_eq!(
        dragged_to(origin, start, now, 0.5),
        Point { x: 220., y: -10. }
    );
}

#[test]
fn a_drag_starts_past_two_pixels() {
    let start = Point { x: 10., y: 10. };
    assert!(!past_threshold(start, Point { x: 11., y: 11. }));
    assert!(!past_threshold(start, Point { x: 12., y: 10. }));
    assert!(past_threshold(start, Point { x: 12.1, y: 10. }));
    assert!(past_threshold(start, Point { x: 10., y: 14. }));
}

#[test]
fn a_connection_dropped_on_a_card_connects_to_it() {
    let scene = scene();
    let a = node(&scene, "a").rect;
    let inside = Point {
        x: a.x + 20.,
        y: a.y + 20.,
    };
    assert_eq!(
        drop_target(inside, &scene.nodes, "trigger", 1.).as_deref(),
        Some("a")
    );
    // Not to the card it started from.
    assert_eq!(drop_target(inside, &scene.nodes, "a", 1.), None);
}

#[test]
fn a_connection_dropped_a_little_outside_a_card_still_finds_it() {
    let scene = scene();
    let a = node(&scene, "a").rect;
    let near = Point {
        x: a.x - 10.,
        y: a.y - 10.,
    };
    assert_eq!(
        drop_target(near, &scene.nodes, "trigger", 1.).as_deref(),
        Some("a")
    );
    let far = Point {
        x: a.x - 60.,
        y: a.y - 60.,
    };
    assert_eq!(drop_target(far, &scene.nodes, "trigger", 1.), None);
}

#[test]
fn the_drop_radius_is_in_screen_pixels() {
    let scene = scene();
    let a = node(&scene, "a").rect;
    let near = Point {
        x: a.x - 15.,
        y: a.y + 10.,
    };
    // 15 graph pixels is 30 on screen at zoom 2: too far; at zoom 0.5 it is 7.5: close enough.
    assert_eq!(drop_target(near, &scene.nodes, "trigger", 2.), None);
    assert_eq!(
        drop_target(near, &scene.nodes, "trigger", 0.5).as_deref(),
        Some("a")
    );
}

#[test]
fn a_for_each_is_a_target_by_its_header_not_by_its_empty_inside() {
    let scene = scene();
    let g = node(&scene, "each").rect;
    let header = Point {
        x: g.x + 40.,
        y: g.y + GROUP_HEADER / 2.,
    };
    assert_eq!(
        drop_target(header, &scene.nodes, "trigger", 1.).as_deref(),
        Some("each")
    );
    let inside = Point {
        x: g.x + 40.,
        y: g.y + g.h - 10.,
    };
    assert_eq!(drop_target(inside, &scene.nodes, "trigger", 1.), None);
}

#[test]
fn a_card_wins_over_the_frame_it_sits_in() {
    let w = workflow(
        json!([
            {"id": "trigger", "type": "manual", "ui": {"x": -400, "y": 0}},
            {"id": "each", "type": "forEach", "in": "x", "ui": {"x": 0, "y": 0}},
            {"id": "dev", "type": "command", "project": "p", "command": "ls", "parent": "each",
             "ui": {"x": 16, "y": 40}},
        ]),
        json!([]),
    );
    let flow = to_flow(&w);
    let scene = Scene::build(&flow.nodes, &flow.edges, Vec::new(), "k".into());
    let dev = node(&scene, "dev").rect;
    let over = Point {
        x: dev.x + 5.,
        y: dev.y + 5.,
    };
    assert_eq!(
        drop_target(over, &scene.nodes, "trigger", 1.).as_deref(),
        Some("dev")
    );
}

fn straight(id: &str, a: Point, b: Point) -> RoutedEdge {
    RoutedEdge {
        id: id.into(),
        is_loop: false,
        state: None,
        segs: vec![Seg::Move(a), Seg::Line(b)],
        arrow: None,
        label: None,
        selected: false,
    }
}

#[test]
fn a_click_near_a_line_picks_the_edge_and_far_from_it_picks_nothing() {
    let edges = vec![
        straight("x", Point { x: 0., y: 0. }, Point { x: 100., y: 0. }),
        straight("y", Point { x: 0., y: 50. }, Point { x: 0., y: 150. }),
    ];
    assert_eq!(
        edge_at(Point { x: 50., y: 4. }, &edges).as_deref(),
        Some("x")
    );
    assert_eq!(
        edge_at(Point { x: 3., y: 100. }, &edges).as_deref(),
        Some("y")
    );
    assert_eq!(edge_at(Point { x: 50., y: 30. }, &edges), None);
    // Beyond the end of a line is not on it.
    assert_eq!(edge_at(Point { x: 130., y: 0. }, &edges), None);
}

#[test]
fn a_rounded_corner_is_on_the_edge_too() {
    let corner = RoutedEdge {
        segs: vec![
            Seg::Move(Point { x: 0., y: 0. }),
            Seg::Line(Point { x: 50., y: 0. }),
            Seg::Quad {
                ctrl: Point { x: 60., y: 0. },
                to: Point { x: 60., y: 10. },
            },
            Seg::Line(Point { x: 60., y: 80. }),
        ],
        ..straight("c", Point::default(), Point::default())
    };
    // The middle of the curve is nearer the corner point than the chord.
    assert_eq!(
        edge_at(Point { x: 57.5, y: 2.5 }, std::slice::from_ref(&corner)).as_deref(),
        Some("c")
    );
    assert_eq!(
        edge_at(Point { x: 60., y: 40. }, &[corner]).as_deref(),
        Some("c")
    );
}

#[test]
fn the_closest_edge_wins() {
    let edges = vec![
        straight("far", Point { x: 0., y: 5. }, Point { x: 100., y: 5. }),
        straight("near", Point { x: 0., y: 1. }, Point { x: 100., y: 1. }),
    ];
    assert_eq!(
        edge_at(Point { x: 50., y: 0. }, &edges).as_deref(),
        Some("near")
    );
}

#[test]
fn a_rect_is_inflated_on_all_sides() {
    let r = Rect::new(10., 10., 20., 20.);
    assert_eq!(inflate(r, 5.), Rect::new(5., 5., 30., 30.));
}
