//! Port of `boardModel.test.ts` (same names, same assertions), plus checks of the Rust-side
//! contract (blank workflow, issues by node, constants).
use super::board_model::*;
use super::fixtures::{edge, ticket_workflow};
use super::model::*;
use super::ordered::OrderedMap;
use super::ui_fixtures::{manual, n, report, workflow};
use super::validate::{validate_workflow, WorkflowIssue};
use serde_json::{json, Value};

fn find<'a>(nodes: &'a [FlowNode], id: &str) -> &'a FlowNode {
    nodes
        .iter()
        .find(|n| n.id == id)
        .unwrap_or_else(|| panic!("no flow node {id}"))
}

fn moved(nodes: &[FlowNode], id: &str, x: f32, y: f32) -> Vec<FlowNode> {
    nodes
        .iter()
        .map(|n| {
            let mut n = n.clone();
            if n.id == id {
                n.position = Point { x, y };
            }
            n
        })
        .collect()
}

fn ids<T>(items: &[T], id: impl Fn(&T) -> &str) -> Vec<String> {
    items.iter().map(|i| id(i).to_string()).collect()
}

fn edge_ids(edges: &[FlowEdge]) -> Vec<String> {
    ids(edges, |e| &e.id)
}

fn without_loops(mut w: Workflow) -> Workflow {
    w.edges.retain(|e| e.loop_spec.is_none());
    w
}

fn set_node(w: &mut Workflow, node: Node) {
    w.nodes.push(node);
}

fn project_inputs(names: &[&str]) -> OrderedMap<WorkflowInput> {
    names
        .iter()
        .map(|k| (*k, WorkflowInput::new(InputType::Project)))
        .collect()
}

fn command_using(id: &str, input: &str) -> Node {
    n(
        json!({"id": id, "type": "command", "project": format!("{{{{inputs.{input}}}}}"), "command": "ls"}),
    )
}

mod fixture {
    use super::*;

    #[test]
    fn is_a_valid_workflow() {
        assert_eq!(validate_workflow(&ticket_workflow()), vec![]);
    }
}

mod to_flow_from_flow {
    use super::*;

    #[test]
    fn round_trip_keeps_nodes_edges_parents_and_positions() {
        let w = ticket_workflow();
        let flow = to_flow(&w);
        let back = from_flow(&w, &flow.nodes, &flow.edges);
        assert_eq!(back.nodes, w.nodes);
        assert_eq!(back.edges, w.edges);
        let dev = back.nodes.iter().find(|n| n.id == "dev").unwrap();
        assert_eq!(dev.parent.as_deref(), Some("each"));
        assert_eq!(dev.ui, Some(Ui { x: 16.0, y: 40.0 }));
    }

    #[test]
    fn makes_a_for_each_a_group_and_its_children_parent_bound_parents_first() {
        let flow = to_flow(&ticket_workflow());
        let group = find(&flow.nodes, "each");
        let dev = find(&flow.nodes, "dev");
        assert_eq!(group.kind, FlowKind::Group);
        assert_eq!(dev.parent_id.as_deref(), Some("each"));
        assert_eq!(dev.kind, FlowKind::Board);
        let pos = |id: &str| flow.nodes.iter().position(|n| n.id == id).unwrap();
        assert!(pos("each") < pos("dev"));
        assert!(group.width > NODE_W);
    }

    #[test]
    fn draws_a_loop_edge_on_the_side_handles_with_its_spec() {
        let flow = to_flow(&ticket_workflow());
        let looped = flow
            .edges
            .iter()
            .find(|e| e.kind == EdgeKind::Loop)
            .unwrap();
        assert_eq!(looped.source, "review");
        assert_eq!(looped.target, "dev");
        assert_eq!(looped.source_handle, Some("loop-out"));
        assert_eq!(looped.target_handle, Some("loop-in"));
        let spec = looped.loop_spec.as_ref().unwrap();
        assert_eq!(spec.max_rounds, 3.0);
        assert_eq!(spec.decide, "mono");
        let plain: Vec<&FlowEdge> = flow
            .edges
            .iter()
            .filter(|e| e.kind != EdgeKind::Loop)
            .collect();
        assert_eq!(plain.len(), 4);
        assert!(plain.iter().all(|e| e.source_handle.is_none()));
    }

    #[test]
    fn keeps_the_true_and_false_handles_of_an_if_node() {
        let mut w = workflow();
        w.nodes = vec![
            n(json!({"id": "trigger", "type": "manual", "ui": {"x": 0, "y": 0}})),
            n(
                json!({"id": "check", "type": "if", "condition": "{{inputs.x}} == 1", "ui": {"x": 0, "y": 80}}),
            ),
            n(json!({"id": "report", "type": "report", "ui": {"x": 0, "y": 160}})),
        ];
        w.edges = vec![
            edge(r#"{"from": "trigger", "to": "check"}"#),
            edge(r#"{"from": "check", "to": "report", "handle": "true"}"#),
        ];
        let flow = to_flow(&w);
        assert_eq!(flow.edges[1].source_handle, Some("true"));
        assert_eq!(from_flow(&w, &flow.nodes, &flow.edges).edges, w.edges);
    }

    #[test]
    fn lays_out_nodes_that_have_no_position_top_to_bottom_without_overlap() {
        let mut w = ticket_workflow();
        for node in &mut w.nodes {
            node.ui = None;
        }
        let flow = to_flow(&w);
        let at = |id: &str| find(&flow.nodes, id).position;
        assert!(at("fetch").y >= at("trigger").y + NODE_H);
        assert!(at("each").y >= at("fetch").y + NODE_H);
        assert!(at("review").y >= at("dev").y + NODE_H);
        // The report sits below the whole group.
        let group = find(&flow.nodes, "each");
        assert!(at("report").y >= at("each").y + group.height);
        // Children are inside the group.
        assert!(at("dev").x >= 0.0);
        assert!(at("dev").y >= 0.0);
    }

    #[test]
    fn saves_moved_positions_to_node_ui_and_a_new_parent_to_node_parent() {
        let w = ticket_workflow();
        let flow = to_flow(&w);
        let shifted = moved(&flow.nodes, "fetch", 40.0, 120.0);
        let out = from_flow(&w, &shifted, &flow.edges);
        let fetch = out.nodes.iter().find(|n| n.id == "fetch").unwrap();
        assert_eq!(fetch.ui, Some(Ui { x: 40.0, y: 120.0 }));
        let detached: Vec<FlowNode> = flow
            .nodes
            .iter()
            .map(|n| {
                let mut n = n.clone();
                if n.id == "dev" {
                    n.parent_id = None;
                }
                n
            })
            .collect();
        let out = from_flow(&w, &detached, &flow.edges);
        let dev = out.nodes.iter().find(|n| n.id == "dev").unwrap();
        assert_eq!(dev.parent, None);
    }

    #[test]
    fn keeps_the_base_order_and_appends_new_nodes() {
        let w = ticket_workflow();
        let flow = to_flow(&w);
        let added = new_node(NodeType::Command, &w);
        let extra = FlowNode::new_board(added.clone(), Point { x: 5.0, y: 6.0 });
        let mut listed = vec![extra];
        listed.extend(flow.nodes.clone());
        let out = from_flow(&w, &listed, &flow.edges);
        let mut want: Vec<String> = w.nodes.iter().map(|n| n.id.clone()).collect();
        want.push(added.id.clone());
        assert_eq!(ids(&out.nodes, |n| &n.id), want);
        assert_eq!(out.nodes.last().unwrap().ui, Some(Ui { x: 5.0, y: 6.0 }));
    }
}

mod connection_kind_rules {
    use super::*;

    #[test]
    fn returns_loop_for_review_dev() {
        let open = without_loops(ticket_workflow());
        assert_eq!(
            connection_kind(&open, "review", "dev", None),
            ConnectionKind::Loop
        );
    }

    #[test]
    fn is_invalid_for_a_second_loop_out_of_the_same_node() {
        assert_eq!(
            connection_kind(&ticket_workflow(), "review", "dev", None),
            ConnectionKind::Invalid
        );
    }

    #[test]
    fn is_invalid_for_an_edge_into_a_trigger() {
        assert_eq!(
            connection_kind(&ticket_workflow(), "fetch", "trigger", None),
            ConnectionKind::Invalid
        );
    }

    #[test]
    fn is_invalid_out_of_a_report_node_and_into_itself() {
        let w = ticket_workflow();
        assert_eq!(
            connection_kind(&w, "report", "fetch", None),
            ConnectionKind::Invalid
        );
        assert_eq!(
            connection_kind(&w, "fetch", "fetch", None),
            ConnectionKind::Invalid
        );
    }

    #[test]
    fn is_invalid_for_an_edge_that_already_exists() {
        assert_eq!(
            connection_kind(&ticket_workflow(), "trigger", "fetch", None),
            ConnectionKind::Invalid
        );
    }

    #[test]
    fn is_invalid_across_a_for_each_boundary() {
        let w = ticket_workflow();
        assert_eq!(
            connection_kind(&w, "fetch", "dev", None),
            ConnectionKind::Invalid
        );
        assert_eq!(
            connection_kind(&w, "review", "report", None),
            ConnectionKind::Invalid
        );
    }

    #[test]
    fn connects_forward_in_a_branch_as_a_plain_edge() {
        let mut wide = ticket_workflow();
        let extra = new_node(NodeType::Command, &wide);
        let extra_id = extra.id.clone();
        set_node(&mut wide, extra);
        assert_eq!(
            connection_kind(&wide, "fetch", &extra_id, None),
            ConnectionKind::Edge
        );
    }

    #[test]
    fn only_lets_a_session_command_or_ask_mono_node_start_a_loop() {
        let mut wide = without_loops(ticket_workflow());
        set_node(
            &mut wide,
            n(
                json!({"id": "gate", "type": "if", "condition": "{{dev.summary}} == 'x'", "parent": "each", "ui": {"x": 16, "y": 200}}),
            ),
        );
        wide.edges.push(Edge::new("review", "gate"));
        assert_eq!(
            connection_kind(&wide, "gate", "dev", None),
            ConnectionKind::Invalid
        );
        assert_eq!(
            connection_kind(&wide, "gate", "review", None),
            ConnectionKind::Invalid
        );
    }

    #[test]
    fn treats_a_loop_target_that_is_not_an_ancestor_as_invalid() {
        let mut wide = ticket_workflow();
        set_node(
            &mut wide,
            n(
                json!({"id": "other", "type": "command", "project": "{{inputs.project}}", "command": "ls", "parent": "each", "ui": {"x": 16, "y": 220}}),
            ),
        );
        // Nothing links "other" to "dev", so going back to it is not a loop.
        assert_eq!(
            connection_kind(&wide, "dev", "other", None),
            ConnectionKind::Edge
        );
        assert_eq!(
            connection_kind(&wide, "other", "dev", None),
            ConnectionKind::Edge
        );
    }
}

mod new_node_defaults {
    use super::*;

    #[test]
    fn ids_are_unique() {
        let mut w = ticket_workflow();
        let mut seen: Vec<String> = w.nodes.iter().map(|n| n.id.clone()).collect();
        for ty in [
            NodeType::Session,
            NodeType::Session,
            NodeType::Command,
            NodeType::AskMono,
            NodeType::If,
            NodeType::ForEach,
            NodeType::Session,
        ] {
            let node = new_node(ty, &w);
            assert!(!seen.contains(&node.id), "{} is taken", node.id);
            assert!(is_node_id(&node.id));
            seen.push(node.id.clone());
            w.nodes.push(node);
        }
    }

    #[test]
    fn gives_each_type_sensible_defaults() {
        let mut w = workflow();
        w.inputs = project_inputs(&["repo"]);
        match new_node(NodeType::Session, &w).kind {
            NodeKind::Session {
                project,
                checkout,
                prompt,
                output,
                ..
            } => {
                assert_eq!(project, "{{inputs.repo}}");
                assert_eq!(checkout, Checkout::Root);
                assert_eq!(prompt, "");
                assert!(output.is_empty());
            }
            other => panic!("not a session: {other:?}"),
        }
        match new_node(NodeType::Command, &w).kind {
            NodeKind::Command {
                project, command, ..
            } => {
                assert_eq!(project, "{{inputs.repo}}");
                assert_eq!(command, "");
            }
            other => panic!("not a command: {other:?}"),
        }
        match new_node(NodeType::AskMono, &w).kind {
            NodeKind::AskMono { answers, .. } => assert_eq!(answers, ["yes", "no"]),
            other => panic!("not an askMono: {other:?}"),
        }
        assert!(matches!(
            new_node(NodeType::If, &w).kind,
            NodeKind::If {
                decide: Some(Decide::Mono),
                condition: None,
                ..
            }
        ));
        match new_node(NodeType::ForEach, &w).kind {
            NodeKind::ForEach {
                item, max_parallel, ..
            } => {
                assert_eq!(item.as_deref(), Some("item"));
                assert_eq!(max_parallel, Some(3.0));
            }
            other => panic!("not a forEach: {other:?}"),
        }
        match new_node(NodeType::Schedule, &w).kind {
            NodeKind::Schedule { schedule } => assert_eq!(schedule.schedule_kind, "weekdays"),
            other => panic!("not a schedule: {other:?}"),
        }
    }

    #[test]
    fn leaves_the_project_empty_when_no_project_input_exists_yet() {
        match new_node(NodeType::Session, &workflow()).kind {
            NodeKind::Session { project, .. } => assert_eq!(project, ""),
            other => panic!("not a session: {other:?}"),
        }
    }

    #[test]
    fn every_type_has_a_node_the_parser_accepts_and_a_matching_type_name() {
        let w = workflow();
        for ty in NodeType::ALL {
            let node = new_node(ty, &w);
            assert_eq!(node.kind.type_name(), ty.name());
            assert_eq!(NodeType::of(&node.kind), ty);
            let back: Node = serde_json::from_value(serde_json::to_value(&node).unwrap()).unwrap();
            assert_eq!(back, node);
        }
    }
}

mod auto_place_rules {
    use super::*;

    #[test]
    fn puts_a_node_below_the_lowest_one_at_the_same_x() {
        let place = auto_place(&ticket_workflow(), None);
        // Top-level nodes: trigger 0, fetch 90, each 190 (a group), report 480.
        assert_eq!(place.x, 0.0);
        assert!(place.y >= 480.0 + NODE_H);
    }

    #[test]
    fn works_inside_a_group_relative_to_it() {
        let place = auto_place(&ticket_workflow(), Some("each"));
        assert_eq!(place.x, 16.0);
        assert!(place.y >= 130.0 + NODE_H);
    }

    #[test]
    fn starts_at_the_origin_on_an_empty_board() {
        let mut w = workflow();
        w.nodes.clear();
        w.edges.clear();
        assert_eq!(auto_place(&w, None), Point { x: 0.0, y: 0.0 });
    }

    #[test]
    fn starts_at_the_top_left_of_an_empty_group() {
        let mut w = workflow();
        w.nodes
            .push(n(json!({"id": "each", "type": "forEach", "in": ""})));
        assert_eq!(
            auto_place(&w, Some("each")),
            Point {
                x: GROUP_PAD,
                y: GROUP_HEADER
            }
        );
    }
}

mod groups {
    use super::*;

    #[test]
    fn fit_groups_wraps_the_children_with_room_to_add_another() {
        let flow = to_flow(&ticket_workflow());
        let fitted = fit_groups(&flow.nodes);
        let group = find(&fitted, "each");
        let review = find(&fitted, "review");
        assert!(group.width >= review.position.x + NODE_W);
        assert!(group.height > review.position.y + NODE_H);
    }

    #[test]
    fn settle_drop_attaches_a_node_dropped_inside_a_group_and_converts_its_position() {
        let flow = to_flow(&ticket_workflow());
        let group = find(&flow.nodes, "each").position;
        let dropped = moved(&flow.nodes, "fetch", group.x + 30.0, group.y + 60.0);
        let after = settle_drop(&dropped, "fetch");
        let fetch = find(&after, "fetch");
        assert_eq!(fetch.parent_id.as_deref(), Some("each"));
        assert_eq!(fetch.position, Point { x: 30.0, y: 60.0 });
    }

    #[test]
    fn settle_drop_detaches_a_child_dragged_out_of_its_group() {
        let flow = to_flow(&ticket_workflow());
        let dropped = moved(&flow.nodes, "dev", 600.0, 40.0);
        let after = settle_drop(&dropped, "dev");
        let dev = find(&after, "dev");
        assert_eq!(dev.parent_id, None);
        let group = find(&flow.nodes, "each").position;
        assert_eq!(
            dev.position,
            Point {
                x: group.x + 600.0,
                y: group.y + 40.0
            }
        );
    }

    #[test]
    fn settle_drop_keeps_a_child_inside_its_group_and_clamps_it() {
        let flow = to_flow(&ticket_workflow());
        let dropped = moved(&flow.nodes, "dev", -5.0, 12.0);
        let after = settle_drop(&dropped, "dev");
        let dev = find(&after, "dev");
        assert_eq!(dev.parent_id.as_deref(), Some("each"));
        assert!(dev.position.x >= 0.0);
        assert!(dev.position.y > 0.0);
    }

    #[test]
    fn settle_drop_never_nests_a_group_and_never_puts_the_report_in_one() {
        let flow = to_flow(&ticket_workflow());
        let group = find(&flow.nodes, "each").position;
        let inside = Point {
            x: group.x + 20.0,
            y: group.y + 60.0,
        };
        let report = settle_drop(&moved(&flow.nodes, "report", inside.x, inside.y), "report");
        assert_eq!(find(&report, "report").parent_id, None);
        let mut second = find(&flow.nodes, "each").clone();
        second.id = "each2".into();
        second.position = inside;
        let mut with_second = flow.nodes.clone();
        with_second.push(second);
        let nested = settle_drop(&with_second, "each2");
        assert_eq!(find(&nested, "each2").parent_id, None);
    }

    #[test]
    fn push_below_moves_what_is_under_a_group_that_grew() {
        let flow = to_flow(&ticket_workflow());
        let taller: Vec<FlowNode> = flow
            .nodes
            .iter()
            .map(|n| {
                let mut n = n.clone();
                if n.id == "each" {
                    n.height += 60.0;
                }
                n
            })
            .collect();
        let out = push_below(&flow.nodes, &taller);
        assert_eq!(find(&out, "report").position.y, 480.0 + 60.0);
        // Children, and nodes above the group, stay put; a group that did not grow moves nothing.
        assert_eq!(
            find(&out, "dev").position,
            find(&flow.nodes, "dev").position
        );
        assert_eq!(find(&out, "fetch").position.y, 90.0);
        assert_eq!(push_below(&flow.nodes, &flow.nodes), flow.nodes);
    }

    #[test]
    fn drop_crossing_edges_removes_the_wires_a_node_can_no_longer_keep_after_changing_groups() {
        let flow = to_flow(&ticket_workflow());
        let group = find(&flow.nodes, "each").position;
        let after = settle_drop(
            &moved(&flow.nodes, "fetch", group.x + 30.0, group.y + 60.0),
            "fetch",
        );
        let kept = drop_crossing_edges(&after, &flow.edges, &["fetch".to_string()]);
        // trigger -> fetch and fetch -> each now cross the frame; the rest stay.
        let want: Vec<String> = flow
            .edges
            .iter()
            .filter(|e| e.source != "fetch" && e.target != "fetch")
            .map(|e| e.id.clone())
            .collect();
        assert_eq!(edge_ids(&kept), want);
        // A node that did not change scope keeps all of its wires.
        assert_eq!(
            drop_crossing_edges(&flow.nodes, &flow.edges, &["dev".to_string()]),
            flow.edges
        );
    }

    #[test]
    fn drop_nodes_removes_edges_and_lifts_the_children_of_a_removed_group() {
        let flow = to_flow(&ticket_workflow());
        let out = drop_nodes(&flow.nodes, &flow.edges, &["each".to_string()]);
        assert!(out.nodes.iter().all(|n| n.id != "each"));
        let dev = find(&out.nodes, "dev");
        assert_eq!(dev.parent_id, None);
        let group = find(&flow.nodes, "each").position;
        assert_eq!(
            dev.position,
            Point {
                x: group.x + 16.0,
                y: group.y + 40.0
            }
        );
        assert!(!out
            .edges
            .iter()
            .any(|e| e.source == "each" || e.target == "each"));
        assert!(out
            .edges
            .iter()
            .any(|e| e.source == "dev" && e.target == "review"));
    }

    #[test]
    fn drop_nodes_leaves_nothing_pointing_at_a_removed_session() {
        let flow = to_flow(&ticket_workflow());
        let out = drop_nodes(&flow.nodes, &flow.edges, &["dev".to_string()]);
        match &find(&out.nodes, "review").data.node.kind {
            NodeKind::Session {
                checkout,
                nest_under,
                ..
            } => {
                assert_eq!(*checkout, Checkout::Root);
                assert_eq!(*nest_under, None);
            }
            other => panic!("not a session: {other:?}"),
        }
        let mut w = ticket_workflow();
        w.nodes.push(n(json!({"id": "build", "type": "command", "project": "{{inputs.project}}", "command": "ls", "cwd": {"worktreeOf": "dev"}})));
        let flow = to_flow(&w);
        let out = drop_nodes(&flow.nodes, &flow.edges, &["dev".to_string()]);
        match &find(&out.nodes, "build").data.node.kind {
            NodeKind::Command { cwd, .. } => assert_eq!(*cwd, None),
            other => panic!("not a command: {other:?}"),
        }
    }
}

mod project_input {
    use super::*;

    #[test]
    fn creates_a_project_input_named_project_for_the_first_project() {
        let out = project_input_for(&workflow(), "/work/api", "api", None);
        assert_eq!(out.reference, "{{inputs.project}}");
        assert_eq!(
            out.inputs.get("project"),
            Some(&WorkflowInput::new(InputType::Project))
        );
        assert_eq!(
            out.input_values.get("project"),
            Some(&InputValue::String("/work/api".into()))
        );
    }

    #[test]
    fn reuses_the_input_that_already_holds_that_project() {
        let w = ticket_workflow();
        let out = project_input_for(&w, "/work/api", "api", None);
        assert_eq!(out.reference, "{{inputs.project}}");
        assert_eq!(
            out.inputs.keys().collect::<Vec<_>>(),
            w.inputs.keys().collect::<Vec<_>>()
        );
    }

    #[test]
    fn names_another_project_after_it_and_keeps_the_first() {
        let out = project_input_for(&ticket_workflow(), "/work/web app", "web app", None);
        assert_eq!(out.reference, "{{inputs.web_app}}");
        assert_eq!(
            out.inputs.get("web_app"),
            Some(&WorkflowInput::new(InputType::Project))
        );
        assert_eq!(
            out.input_values.get("project"),
            Some(&InputValue::String("/work/api".into()))
        );
        assert_eq!(
            out.input_values.get("web_app"),
            Some(&InputValue::String("/work/web app".into()))
        );
    }

    #[test]
    fn never_reuses_a_name_that_is_taken_by_another_kind_of_input() {
        let mut w = workflow();
        w.inputs
            .insert("project", WorkflowInput::new(InputType::String));
        w.input_values
            .insert("project", InputValue::String("x".into()));
        let out = project_input_for(&w, "/work/api", "api", None);
        assert_eq!(out.reference, "{{inputs.api}}");
    }

    #[test]
    fn names_a_label_that_cannot_be_a_name_after_a_prefix_or_falls_back_to_project() {
        let mut w = workflow();
        w.inputs
            .insert("project", WorkflowInput::new(InputType::String));
        let digits = project_input_for(&w, "/p", "2fast", None);
        assert_eq!(digits.reference, "{{inputs.p_2fast}}");
        let symbols = project_input_for(&w, "/p", "***", None);
        assert_eq!(symbols.reference, "{{inputs.project_2}}");
        let upper = project_input_for(&w, "/p", "  My App! ", None);
        assert_eq!(upper.reference, "{{inputs.my_app}}");
    }
}

mod keyboard_delete {
    use super::*;

    #[test]
    fn removes_a_for_each_and_its_wires_but_keeps_its_children_with_theirs() {
        let flow = to_flow(&ticket_workflow());
        // What the canvas hands over for a selected group: the group, its children, and every
        // edge touching any of them.
        let mut group = find(&flow.nodes, "each").clone();
        group.selected = true;
        let mut listed = vec![group];
        listed.extend(
            flow.nodes
                .iter()
                .filter(|n| n.parent_id.as_deref() == Some("each"))
                .cloned(),
        );
        let out = deletion(&listed, &flow.edges);
        assert_eq!(ids(&out.nodes, |n| &n.id), ["each"]);
        assert_eq!(edge_ids(&out.edges), ["fetch->each", "each->report"]);
    }

    #[test]
    fn removes_children_that_were_selected_themselves_and_edges_that_were_selected() {
        let flow = to_flow(&ticket_workflow());
        let mut dev = find(&flow.nodes, "dev").clone();
        dev.selected = true;
        let mut edges: Vec<FlowEdge> = flow
            .edges
            .iter()
            .filter(|e| e.id != "trigger->fetch")
            .cloned()
            .collect();
        let mut wire = flow
            .edges
            .iter()
            .find(|e| e.id == "trigger->fetch")
            .unwrap()
            .clone();
        wire.selected = true;
        edges.push(wire);
        let out = deletion(&[dev], &edges);
        assert_eq!(ids(&out.nodes, |n| &n.id), ["dev"]);
        let mut got = edge_ids(&out.edges);
        got.sort();
        assert_eq!(got, ["dev->review", "review->dev~loop", "trigger->fetch"]);
    }
}

mod if_handles {
    use super::*;

    #[test]
    fn lets_true_and_false_lead_to_the_same_node_but_not_the_same_handle_twice() {
        let mut w = workflow();
        w.nodes = vec![
            manual(),
            n(json!({"id": "check", "type": "if", "decide": "mono"})),
            report(),
        ];
        w.edges = vec![
            edge(r#"{"from": "trigger", "to": "check"}"#),
            edge(r#"{"from": "check", "to": "report", "handle": "true"}"#),
        ];
        assert_eq!(
            connection_kind(&w, "check", "report", Some(Handle::False)),
            ConnectionKind::Edge
        );
        assert_eq!(
            connection_kind(&w, "check", "report", Some(Handle::True)),
            ConnectionKind::Invalid
        );
    }
}

mod definition_key_rules {
    use super::*;

    #[test]
    fn treats_a_whitespace_only_description_as_none() {
        let w = ticket_workflow();
        let mut blank = w.clone();
        blank.description = Some("   ".into());
        let mut bare = w;
        bare.description = None;
        assert_eq!(definition_key(&blank), definition_key(&bare));
    }

    #[test]
    fn a_workflow_with_no_saved_layout_equals_the_same_one_after_being_laid_out() {
        let mut bare = ticket_workflow();
        for node in &mut bare.nodes {
            node.ui = None;
        }
        let flow = to_flow(&bare);
        let laid_out = from_flow(&bare, &flow.nodes, &flow.edges);
        assert_eq!(definition_key(&bare), definition_key(&laid_out));
    }

    #[test]
    fn leaves_out_app_managed_fields_and_input_values_without_an_input() {
        let w = ticket_workflow();
        let mut other = w.clone();
        other.next_run_at = Some(5);
        other.last_run_id = Some("run-1".into());
        other
            .input_values
            .insert("stray", InputValue::String("x".into()));
        assert_eq!(definition_key(&w), definition_key(&other));
        let mut renamed = w.clone();
        renamed.name = "Other".into();
        assert_ne!(definition_key(&w), definition_key(&renamed));
        let mut changed = w.clone();
        changed
            .input_values
            .insert("project", InputValue::String("/elsewhere".into()));
        assert_ne!(definition_key(&w), definition_key(&changed));
    }
}

mod project_input_for_and_inputs_with_no_value {
    use super::*;

    #[test]
    fn fills_a_declared_project_input_that_has_no_value_yet() {
        let mut w = workflow();
        w.inputs = project_inputs(&["repo"]);
        let out = project_input_for(&w, "/work/api", "api", None);
        assert_eq!(out.reference, "{{inputs.repo}}");
        assert_eq!(out.inputs.keys().collect::<Vec<_>>(), ["repo"]);
        assert_eq!(
            out.input_values.get("repo"),
            Some(&InputValue::String("/work/api".into()))
        );
    }

    #[test]
    fn does_not_bind_a_step_to_a_valueless_input_another_step_uses() {
        let mut w = workflow();
        w.inputs = project_inputs(&["api"]);
        w.nodes = vec![manual(), command_using("a", "api"), report()];
        let out = project_input_for(&w, "/work/web", "web", None);
        assert_eq!(out.reference, "{{inputs.project}}");
        assert_eq!(
            out.inputs.get("api"),
            Some(&WorkflowInput::new(InputType::Project))
        );
        let want: OrderedMap<InputValue> = [("project", InputValue::String("/work/web".into()))]
            .into_iter()
            .collect();
        assert_eq!(out.input_values, want);
        // A loop condition that mentions the input counts as using it too.
        let mut looped = workflow();
        looped.inputs = project_inputs(&["api"]);
        looped.edges = vec![edge(
            r#"{"from": "trigger", "to": "report", "loop": {"maxRounds": 2, "decide": "{{inputs.api}} == 1"}}"#,
        )];
        assert_eq!(
            project_input_for(&looped, "/work/web", "web", None).reference,
            "{{inputs.project}}"
        );
    }

    #[test]
    fn reuses_the_steps_own_valueless_input() {
        let mut w = workflow();
        w.inputs = project_inputs(&["api", "other"]);
        w.nodes = vec![manual(), command_using("a", "api"), report()];
        let out = project_input_for(&w, "/work/web", "web", Some("api"));
        assert_eq!(out.reference, "{{inputs.api}}");
        let want: OrderedMap<InputValue> = [("api", InputValue::String("/work/web".into()))]
            .into_iter()
            .collect();
        assert_eq!(out.input_values, want);
    }

    #[test]
    fn does_not_mistake_api_2_for_api_when_looking_for_use() {
        let mut w = workflow();
        w.inputs = project_inputs(&["api", "api_2"]);
        w.nodes = vec![manual(), command_using("a", "api_2"), report()];
        assert_eq!(
            project_input_for(&w, "/work/web", "web", None).reference,
            "{{inputs.api}}"
        );
    }

    #[test]
    fn prefers_the_input_the_step_already_uses() {
        let mut w = workflow();
        w.inputs = project_inputs(&["a", "b"]);
        assert_eq!(
            project_input_for(&w, "/work/api", "api", Some("b")).reference,
            "{{inputs.b}}"
        );
    }
}

mod whole_workflows {
    use super::*;

    #[test]
    fn a_blank_workflow_is_a_paused_manual_trigger_into_a_report() {
        let w = blank_workflow(1234);
        assert!(w.id.starts_with("wf-"));
        assert_eq!(w.name, "New workflow");
        assert!(!w.enabled);
        assert_eq!(w.created_at, 1234);
        assert_eq!(ids(&w.nodes, |n| &n.id), ["trigger", "report"]);
        assert_eq!(w.nodes[0].title.as_deref(), Some("Manual trigger"));
        assert_eq!(w.nodes[1].title.as_deref(), Some("Report to Mono"));
        assert_eq!(w.nodes[1].ui, Some(Ui { x: 0.0, y: 84.0 }));
        assert_eq!(w.edges, vec![Edge::new("trigger", "report")]);
        assert_eq!(validate_workflow(&w), vec![]);
    }

    #[test]
    fn issues_by_node_groups_messages_and_skips_issues_without_a_node() {
        let issue = |node: Option<&str>, message: &str| WorkflowIssue {
            node_id: node.map(str::to_string),
            message: message.to_string(),
        };
        let out = issues_by_node(&[
            issue(Some("a"), "one"),
            issue(None, "global"),
            issue(Some("b"), "two"),
            issue(Some("a"), "three"),
        ]);
        assert_eq!(out.len(), 2);
        assert_eq!(out["a"], ["one", "three"]);
        assert_eq!(out["b"], ["two"]);
    }

    #[test]
    fn the_constants_are_the_ones_layout_counts_on() {
        assert_eq!(
            (NODE_W, NODE_H, GAP_Y, GAP_X, GROUP_PAD, GROUP_HEADER),
            (232.0, 48.0, 36.0, 28.0, 16.0, 32.0)
        );
        assert_eq!(
            (GROUP_RIGHT, GROUP_SLACK, EMPTY_GROUP_H),
            (52.0, 44.0, 140.0)
        );
    }

    #[test]
    fn an_empty_group_has_its_own_size_and_a_group_with_children_wraps_them() {
        let mut w = workflow();
        w.nodes
            .insert(1, n(json!({"id": "each", "type": "forEach", "in": ""})));
        let flow = to_flow(&w);
        let group = find(&flow.nodes, "each");
        assert_eq!(group.width, NODE_W + GROUP_PAD + GROUP_RIGHT);
        assert_eq!(group.height, EMPTY_GROUP_H);
    }

    #[test]
    fn duplicate_edges_get_numbered_ids() {
        let mut w = workflow();
        w.edges.push(Edge::new("trigger", "report"));
        w.edges.push(Edge::new("trigger", "report"));
        let flow = to_flow(&w);
        assert_eq!(
            edge_ids(&flow.edges),
            ["trigger->report", "trigger->report#2", "trigger->report#3"]
        );
    }

    #[test]
    fn make_edge_ids_name_the_handle_and_the_loop() {
        assert_eq!(make_edge("a", "b", None, None).id, "a->b");
        assert_eq!(
            make_edge("a", "b", Some(Handle::False), None).id,
            "a.false->b"
        );
        let spec = LoopSpec {
            max_rounds: 2.0,
            decide: "mono".into(),
        };
        let looped = make_edge("a", "b", None, Some(spec));
        assert_eq!(looped.id, "a->b~loop");
        assert_eq!(
            (looped.source_handle, looped.target_handle),
            (Some(LOOP_OUT), Some(LOOP_IN))
        );
    }

    #[test]
    fn serialized_values_in_json_are_unchanged_by_a_flow_round_trip() {
        let w = ticket_workflow();
        let flow = to_flow(&w);
        let back = from_flow(&w, &flow.nodes, &flow.edges);
        let a: Value = serde_json::to_value(&w.nodes).unwrap();
        let b: Value = serde_json::to_value(&back.nodes).unwrap();
        assert_eq!(a, b);
    }
}
