//! Tests of the board's editing rules: `WorkflowBoard.test.ts` (save, flagged, changed
//! elsewhere, the picker), `NodeInspector.test.ts` (the effects of the inspector's fields) and
//! the rules of `WorkflowBoard.tsx` that have no DOM (connect, loop, reparent, delete, add).
use super::board_edit::*;
use super::board_model::*;
use super::fixtures::{edge, ticket_workflow};
use super::model::*;
use super::models::ModelCatalog;
use super::ordered::OrderedMap;
use super::ui_fixtures::{n, workflow};
use serde_json::json;

fn flow_of(w: &Workflow) -> Flow {
    to_flow(w)
}

fn node<'a>(flow: &'a Flow, id: &str) -> &'a FlowNode {
    flow.nodes
        .iter()
        .find(|n| n.id == id)
        .unwrap_or_else(|| panic!("no node {id}"))
}

fn has_edge(flow: &Flow, from: &str, to: &str) -> bool {
    flow.edges
        .iter()
        .any(|e| e.source == from && e.target == to && e.loop_spec.is_none())
}

fn board(w: &Workflow) -> (Flow, Workflow) {
    let flow = flow_of(w);
    let current = current(w, &flow, &Meta::of(w));
    (flow, current)
}

fn selected(flow: &Flow, id: &str) -> Flow {
    select(flow, Some(id), None)
}

mod add {
    use super::*;

    #[test]
    fn a_command_goes_before_the_report_which_moves_down_with_the_edge_into_it() {
        let mut w = ticket_workflow();
        w.nodes
            .retain(|n| !matches!(n.id.as_str(), "each" | "dev" | "review"));
        w.edges = vec![
            edge(r#"{"from": "trigger", "to": "fetch"}"#),
            edge(r#"{"from": "fetch", "to": "report"}"#),
        ];
        let (flow, current) = board(&w);
        let report_y = node(&flow, "report").position.y;
        let (out, id) = add_node(&flow, &current, NodeType::Command);
        assert_eq!(id, "command");
        assert!(has_edge(&out, "fetch", "command"));
        assert!(has_edge(&out, "command", "report"));
        assert!(!has_edge(&out, "fetch", "report"));
        assert!(node(&out, "report").position.y > report_y);
        assert!(node(&out, "command").position.y < node(&out, "report").position.y);
        assert!(node(&out, "command").selected);
    }

    #[test]
    fn a_for_each_before_the_report_leaves_room_for_its_frame() {
        let (flow, current) = board(&workflow());
        let (out, id) = add_node(&flow, &current, NodeType::ForEach);
        let each = node(&out, &id);
        assert_eq!(each.kind, FlowKind::Group);
        assert_eq!(each.height, EMPTY_GROUP_H);
        assert!(each.position.y + each.height <= node(&out, "report").position.y);
        assert!(has_edge(&out, "trigger", &id));
        assert!(has_edge(&out, &id, "report"));
    }

    #[test]
    fn a_trigger_goes_above_everything() {
        let mut w = workflow();
        w.nodes.retain(|n| n.id != "trigger");
        w.edges.clear();
        let (flow, current) = board(&w);
        let top = flow.nodes.iter().map(|n| n.position.y).fold(0., f32::min);
        let (out, id) = add_node(&flow, &current, NodeType::Schedule);
        assert!(node(&out, &id).position.y < top);
    }

    #[test]
    fn a_node_added_with_a_for_each_selected_goes_inside_it() {
        let (flow, current) = board(&ticket_workflow());
        let (out, id) = add_node(&selected(&flow, "each"), &current, NodeType::Command);
        assert_eq!(node(&out, &id).parent_id.as_deref(), Some("each"));
        // The new node hangs under the lowest one in the frame.
        assert!(has_edge(&out, "review", &id));
        assert!(node(&out, "each").height >= node(&flow, "each").height);
    }

    #[test]
    fn a_node_added_with_a_child_selected_joins_its_frame() {
        let (flow, current) = board(&ticket_workflow());
        let (out, id) = add_node(&selected(&flow, "dev"), &current, NodeType::AskMono);
        assert_eq!(node(&out, &id).parent_id.as_deref(), Some("each"));
    }

    #[test]
    fn a_for_each_or_report_never_goes_inside_a_frame() {
        let (flow, current) = board(&ticket_workflow());
        let (out, id) = add_node(&selected(&flow, "each"), &current, NodeType::ForEach);
        assert_eq!(node(&out, &id).parent_id, None);
    }

    #[test]
    fn only_the_new_node_ends_up_selected() {
        let (flow, current) = board(&ticket_workflow());
        let (out, id) = add_node(&selected(&flow, "fetch"), &current, NodeType::Command);
        let picked: Vec<&str> = out
            .nodes
            .iter()
            .filter(|n| n.selected)
            .map(|n| n.id.as_str())
            .collect();
        assert_eq!(picked, vec![id.as_str()]);
    }

    #[test]
    fn an_if_or_a_report_is_not_wired_after() {
        let mut w = ticket_workflow();
        w.nodes
            .retain(|n| !matches!(n.id.as_str(), "each" | "dev" | "review" | "report"));
        w.edges = vec![edge(r#"{"from": "trigger", "to": "fetch"}"#)];
        let (flow, current) = board(&w);
        let (out, id) = add_node(&flow, &current, NodeType::If);
        // fetch leads nowhere, so it feeds the If.
        assert!(has_edge(&out, "fetch", &id));
        let (flow, current) = board(&current_without(&out, &id));
        let (out2, other) = add_node(&flow, &current, NodeType::Command);
        assert!(has_edge(&out2, "fetch", &other));
    }

    /// The workflow of `flow` with the node `id` taken out and the edges of it too.
    fn current_without(flow: &Flow, id: &str) -> Workflow {
        let base = ticket_workflow();
        let kept = drop_nodes(&flow.nodes, &flow.edges, &[id.to_string()]);
        from_flow(&base, &kept.nodes, &kept.edges)
    }

    #[test]
    fn a_node_is_not_wired_after_a_node_that_already_leads_somewhere() {
        let (flow, current) = board(&workflow());
        // trigger -> report: the trigger leads on already.
        let (out, id) = add_node(&selected(&flow, "trigger"), &current, NodeType::Schedule);
        assert!(!out.edges.iter().any(|e| e.target == id));
    }
}

mod connecting {
    use super::*;

    fn wide() -> (Flow, Workflow) {
        let mut w = ticket_workflow();
        w.edges.retain(|e| e.loop_spec.is_none());
        let (flow, current) = board(&w);
        let (flow, _) = add_node(&flow, &current, NodeType::Command);
        let current = current_of(&w, &flow);
        (flow, current)
    }

    fn current_of(base: &Workflow, flow: &Flow) -> Workflow {
        from_flow(base, &flow.nodes, &flow.edges)
    }

    #[test]
    fn an_invalid_connection_changes_nothing() {
        let (flow, current) = board(&ticket_workflow());
        assert!(matches!(
            connect(&flow, &current, "fetch", "trigger", None),
            Connected::Invalid
        ));
        assert!(matches!(
            connect(&flow, &current, "fetch", "dev", None),
            Connected::Invalid
        ));
    }

    #[test]
    fn a_connection_back_up_a_branch_asks_for_a_loop() {
        let mut w = ticket_workflow();
        w.edges.retain(|e| e.loop_spec.is_none());
        let (flow, current) = board(&w);
        assert!(matches!(
            connect(&flow, &current, "review", "dev", None),
            Connected::Loop
        ));
    }

    #[test]
    fn a_plain_connection_adds_an_edge() {
        let (flow, current) = wide();
        let command = flow
            .nodes
            .iter()
            .find(|n| n.id.starts_with("command"))
            .unwrap()
            .id
            .clone();
        let Connected::Edge(out) = connect(&flow, &current, "fetch", &command, None) else {
            panic!("an edge")
        };
        assert!(has_edge(&out, "fetch", &command));
        assert_eq!(out.edges.len(), flow.edges.len() + 1);
    }

    #[test]
    fn an_if_sends_true_and_false_to_one_place_each() {
        let mut w = workflow();
        w.nodes = vec![
            n(json!({"id": "trigger", "type": "manual", "ui": {"x": 0, "y": 0}})),
            n(json!({"id": "check", "type": "if", "decide": "mono", "ui": {"x": 0, "y": 80}})),
            n(json!({"id": "a", "type": "report", "ui": {"x": 0, "y": 160}})),
            n(
                json!({"id": "b", "type": "command", "project": "x", "command": "ls", "ui": {"x": 300, "y": 160}}),
            ),
        ];
        w.edges = vec![
            edge(r#"{"from": "trigger", "to": "check"}"#),
            edge(r#"{"from": "check", "to": "a", "handle": "true"}"#),
        ];
        let (flow, current) = board(&w);
        let Connected::Edge(out) = connect(&flow, &current, "check", "b", Some("true")) else {
            panic!("an edge")
        };
        let from_check: Vec<(&str, Option<&str>)> = out
            .edges
            .iter()
            .filter(|e| e.source == "check")
            .map(|e| (e.target.as_str(), e.source_handle))
            .collect();
        assert_eq!(from_check, vec![("b", Some("true"))]);
        // The other handle may go to the same place.
        let Connected::Edge(out) =
            connect(&out, &current_of(&w, &out), "check", "b", Some("false"))
        else {
            panic!("an edge")
        };
        assert_eq!(out.edges.iter().filter(|e| e.source == "check").count(), 2);
    }

    #[test]
    fn confirming_a_loop_adds_the_dashed_edge() {
        let mut w = ticket_workflow();
        w.edges.retain(|e| e.loop_spec.is_none());
        let (flow, _) = board(&w);
        let out = add_loop(
            &flow,
            "review",
            "dev",
            LoopSpec {
                max_rounds: 2.0,
                decide: "{{review.verdict}} != \"approve\"".into(),
            },
        );
        let looped = out.edges.iter().find(|e| e.loop_spec.is_some()).unwrap();
        assert_eq!(
            (looped.source.as_str(), looped.target.as_str()),
            ("review", "dev")
        );
        assert_eq!(looped.loop_spec.as_ref().unwrap().max_rounds, 2.0);
        assert_eq!(looped.kind, EdgeKind::Loop);
    }

    #[test]
    fn a_loop_is_allowed_only_while_it_still_makes_one() {
        let mut w = ticket_workflow();
        w.edges.retain(|e| e.loop_spec.is_none());
        assert!(loop_allowed(&w, "review", "dev"));
        // A plain forward connection is not a loop, and a node that is gone cannot be one.
        assert!(!loop_allowed(&w, "dev", "review"));
        w.nodes.retain(|n| n.id != "dev");
        assert!(!loop_allowed(&w, "review", "dev"));
        let looped = ticket_workflow();
        assert!(!loop_allowed(&looped, "review", "dev"));
    }

    #[test]
    fn the_default_loop_is_three_rounds_decided_by_the_mono() {
        assert_eq!(
            default_loop(),
            LoopSpec {
                max_rounds: 3.0,
                decide: "mono".into()
            }
        );
    }

    #[test]
    fn editing_a_loop_changes_it_and_removing_it_takes_the_edge_away() {
        let (flow, _) = board(&ticket_workflow());
        let spec = LoopSpec {
            max_rounds: 7.0,
            decide: "mono".into(),
        };
        let out = set_loop(&flow, "review", Some(spec.clone()));
        let looped = out.edges.iter().find(|e| e.loop_spec.is_some()).unwrap();
        assert_eq!(looped.loop_spec.as_ref(), Some(&spec));
        assert_eq!(out.edges.len(), flow.edges.len());
        let out = set_loop(&out, "review", None);
        assert!(!out.edges.iter().any(|e| e.loop_spec.is_some()));
        // A node with no loop has nothing to change.
        assert_eq!(set_loop(&flow, "fetch", None), flow);
    }
}

mod dragging {
    use super::*;

    fn moved(flow: &Flow, id: &str, x: f32, y: f32) -> Flow {
        let mut out = flow.clone();
        out.nodes.iter_mut().find(|n| n.id == id).unwrap().position = Point { x, y };
        out
    }

    #[test]
    fn dropping_a_node_over_a_for_each_reparents_it_and_drops_the_wires_across_the_frame() {
        let (flow, _) = board(&ticket_workflow());
        let group = node(&flow, "each");
        // fetch is dragged into the frame.
        let over = moved(
            &flow,
            "fetch",
            group.position.x + 40.,
            group.position.y + 40.,
        );
        let out = drag_stop(&over, "fetch");
        assert_eq!(node(&out, "fetch").parent_id.as_deref(), Some("each"));
        // trigger -> fetch is across the frame now; fetch -> each is a wire into its own frame.
        assert!(!has_edge(&out, "trigger", "fetch"));
        assert!(!has_edge(&out, "fetch", "each"));
    }

    #[test]
    fn dropping_a_child_outside_its_frame_takes_it_out() {
        let (flow, _) = board(&ticket_workflow());
        let out = drag_stop(&moved(&flow, "dev", 900., 900.), "dev");
        assert_eq!(node(&out, "dev").parent_id, None);
        assert!(!out
            .edges
            .iter()
            .any(|e| e.source == "dev" || e.target == "dev"));
    }

    #[test]
    fn a_node_dropped_where_it_was_keeps_its_wires() {
        let (flow, _) = board(&ticket_workflow());
        let out = drag_stop(&flow, "fetch");
        assert_eq!(out.edges, flow.edges);
        assert_eq!(node(&out, "fetch").parent_id, None);
    }

    #[test]
    fn a_trigger_or_report_dropped_over_a_frame_stays_out() {
        let (flow, _) = board(&ticket_workflow());
        let group = node(&flow, "each");
        let over = moved(
            &flow,
            "report",
            group.position.x + 40.,
            group.position.y + 40.,
        );
        assert_eq!(node(&drag_stop(&over, "report"), "report").parent_id, None);
    }
}

mod deleting {
    use super::*;

    #[test]
    fn deleting_a_node_takes_its_wires_and_the_references_to_it() {
        let (flow, _) = board(&ticket_workflow());
        let out = delete_selected(&selected(&flow, "dev"));
        assert!(out.nodes.iter().all(|n| n.id != "dev"));
        assert!(!out
            .edges
            .iter()
            .any(|e| e.source == "dev" || e.target == "dev"));
        // The review checked out a worktree of dev and nested under it.
        let review = &node(&out, "review").data.node;
        let NodeKind::Session {
            checkout,
            nest_under,
            ..
        } = &review.kind
        else {
            panic!("a session")
        };
        assert_eq!(*checkout, Checkout::Root);
        assert_eq!(*nest_under, None);
    }

    #[test]
    fn deleting_a_for_each_takes_only_its_frame() {
        let (flow, _) = board(&ticket_workflow());
        let out = delete_selected(&selected(&flow, "each"));
        assert!(out.nodes.iter().all(|n| n.id != "each"));
        let dev = node(&out, "dev");
        assert_eq!(dev.parent_id, None);
        // The wires of the children stay.
        assert!(has_edge(&out, "dev", "review"));
        assert!(!out
            .edges
            .iter()
            .any(|e| e.source == "each" || e.target == "each"));
    }

    #[test]
    fn deleting_a_selected_edge_leaves_the_nodes() {
        let (flow, _) = board(&ticket_workflow());
        let id = flow
            .edges
            .iter()
            .find(|e| e.source == "trigger")
            .unwrap()
            .id
            .clone();
        let out = delete_selected(&select(&flow, None, Some(&id)));
        assert_eq!(out.nodes.len(), flow.nodes.len());
        assert_eq!(out.edges.len(), flow.edges.len() - 1);
        assert!(!has_edge(&out, "trigger", "fetch"));
    }

    #[test]
    fn nothing_selected_deletes_nothing() {
        let (flow, _) = board(&ticket_workflow());
        assert_eq!(delete_selected(&flow), flow);
    }

    #[test]
    fn removing_a_node_from_the_inspector_is_the_same_as_dropping_it() {
        let (flow, _) = board(&ticket_workflow());
        let out = remove_node(&flow, "fetch");
        assert!(out.nodes.iter().all(|n| n.id != "fetch"));
        assert!(!out
            .edges
            .iter()
            .any(|e| e.source == "fetch" || e.target == "fetch"));
    }
}

mod selecting {
    use super::*;

    #[test]
    fn selecting_a_node_deselects_the_rest_and_the_edges() {
        let (flow, _) = board(&ticket_workflow());
        let edge_id = flow.edges[0].id.clone();
        let flow = select(&flow, None, Some(&edge_id));
        assert!(flow.edges[0].selected);
        let flow = select(&flow, Some("fetch"), None);
        assert!(node(&flow, "fetch").selected);
        assert!(flow.edges.iter().all(|e| !e.selected));
        let flow = select(&flow, Some("report"), None);
        assert_eq!(flow.nodes.iter().filter(|n| n.selected).count(), 1);
        let flow = select(&flow, None, None);
        assert!(flow.nodes.iter().all(|n| !n.selected));
    }
}

mod save {
    use super::*;

    fn stored() -> Workflow {
        let mut w = ticket_workflow();
        w.run_requested = true;
        w.stop_requested = true;
        w.resume_run_id = Some("run-9".into());
        w.last_run_id = Some("run-8".into());
        w.imported_from = Some("a.yaml".into());
        w.next_run_at = Some(123);
        w
    }

    fn renamed(w: &Workflow, name: &str) -> Workflow {
        let mut w = w.clone();
        w.name = name.into();
        w
    }

    #[test]
    fn a_valid_workflow_saves_with_the_fields_the_app_manages_kept() {
        let w = stored();
        let (list, saved) = save_into(
            vec![w.clone()],
            &renamed(&w, "Renamed"),
            false,
            &definition_key(&w),
            1_000,
        )
        .unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(saved.name, "Renamed");
        assert!(saved.run_requested && saved.stop_requested);
        assert_eq!(saved.resume_run_id.as_deref(), Some("run-9"));
        assert_eq!(saved.last_run_id.as_deref(), Some("run-8"));
        assert_eq!(saved.imported_from.as_deref(), Some("a.yaml"));
        assert_eq!(saved.created_at, 1);
        assert_eq!(
            saved.nodes.iter().map(|n| n.id.clone()).collect::<Vec<_>>(),
            w.nodes.iter().map(|n| n.id.clone()).collect::<Vec<_>>()
        );
        assert_eq!(list[0], saved);
    }

    #[test]
    fn it_will_not_save_a_flagged_workflow_enabled_but_saves_it_paused_keeping_the_mark() {
        let mut w = stored();
        w.needs_review = true;
        let seen = definition_key(&w);
        let refused = save_into(
            vec![w.clone()],
            &renamed(&w, "Renamed"),
            false,
            &seen,
            1_000,
        );
        assert!(matches!(refused, Err(SaveRefusal::Message(m)) if m == REVIEW_FIRST));
        let mut paused = renamed(&w, "Renamed");
        paused.enabled = false;
        let (_, saved) = save_into(vec![w], &paused, false, &seen, 1_000).unwrap();
        assert!(saved.needs_review);
        assert!(!saved.enabled);
        assert_eq!(saved.name, "Renamed");
    }

    #[test]
    fn it_refuses_a_workflow_flagged_after_the_board_loaded_it() {
        let w = ticket_workflow();
        let seen = definition_key(&w);
        let mut flagged = w.clone();
        flagged.needs_review = true;
        let refused = save_into(vec![flagged], &renamed(&w, "Renamed"), false, &seen, 1_000);
        assert!(matches!(refused, Err(SaveRefusal::Message(m)) if m == REVIEW_FIRST));
    }

    #[test]
    fn it_does_not_save_over_a_definition_that_changed_since_the_board_loaded_it() {
        let w = ticket_workflow();
        let seen = definition_key(&w);
        let elsewhere = renamed(&w, "By the Mono");
        let refused = save_into(
            vec![elsewhere.clone()],
            &renamed(&w, "Mine"),
            false,
            &seen,
            1_000,
        );
        let Err(SaveRefusal::Changed(stored)) = refused else {
            panic!("changed elsewhere")
        };
        assert_eq!(stored.name, "By the Mono");
        // Overwrite is the explicit way past.
        let (list, saved) =
            save_into(vec![elsewhere], &renamed(&w, "Mine"), true, &seen, 1_000).unwrap();
        assert_eq!(saved.name, "Mine");
        assert_eq!(list[0].name, "Mine");
    }

    #[test]
    fn it_saves_normally_when_only_fields_the_app_manages_changed() {
        let w = ticket_workflow();
        let seen = definition_key(&w);
        let mut ran = w.clone();
        ran.last_run_id = Some("run-3".into());
        ran.run_requested = true;
        let (_, saved) = save_into(vec![ran], &renamed(&w, "Mine"), false, &seen, 1_000).unwrap();
        assert_eq!(saved.name, "Mine");
        assert_eq!(saved.last_run_id.as_deref(), Some("run-3"));
        assert!(saved.run_requested);
    }

    #[test]
    fn a_new_workflow_is_appended_and_the_twenty_first_is_refused() {
        let w = blank_workflow(5);
        let (list, saved) = save_into(vec![], &w, false, "", 1_000).unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(saved.created_at, 5);
        assert!(!saved.enabled);
        let full: Vec<Workflow> = (0..WORKFLOWS_MAX)
            .map(|i| {
                let mut x = ticket_workflow();
                x.id = format!("wf-{i:08}");
                x
            })
            .collect();
        let refused = save_into(full, &w, false, "", 1_000);
        assert!(matches!(refused, Err(SaveRefusal::Message(m)) if m == "At most 20 workflows."));
    }

    #[test]
    fn saving_anchors_an_interval_schedule_that_has_no_start_date() {
        let mut w = ticket_workflow();
        for node in &mut w.nodes {
            if let NodeKind::Schedule { schedule } = &mut node.kind {
                *schedule = crate::mono::habits::Schedule {
                    schedule_kind: "interval".into(),
                    time: "09:00".into(),
                    minute: 0,
                    day_of_week: 1,
                    interval_hours: Some(4),
                    interval_from: None,
                };
            }
        }
        let (_, saved) = save_into(
            vec![w.clone()],
            &renamed(&w, "Renamed"),
            false,
            &definition_key(&w),
            1_700_000_000_000,
        )
        .unwrap();
        let Some(NodeKind::Schedule { schedule }) = trigger_of(&saved).map(|n| &n.kind) else {
            panic!("a schedule")
        };
        assert!(schedule.interval_from.is_some());
        assert!(saved.next_run_at.is_some());
    }

    #[test]
    fn an_unchanged_schedule_keeps_its_place_and_a_resumed_one_starts_afresh() {
        let mut w = ticket_workflow();
        w.next_run_at = Some(4_242);
        let seen = definition_key(&w);
        let (_, same) =
            save_into(vec![w.clone()], &renamed(&w, "Mine"), false, &seen, 1_000).unwrap();
        assert_eq!(same.next_run_at, Some(4_242));
        // Paused, then resumed: the stored copy was not enabled, so the time is counted again.
        let mut paused = w.clone();
        paused.enabled = false;
        let seen = definition_key(&paused);
        let (_, fresh) = save_into(vec![paused], &w, false, &seen, 1_000).unwrap();
        assert_ne!(fresh.next_run_at, Some(4_242));
        assert!(fresh.next_run_at.is_some());
    }

    #[test]
    fn a_paused_workflow_has_no_next_run() {
        let mut w = ticket_workflow();
        w.next_run_at = Some(1);
        w.enabled = false;
        let seen = definition_key(&w);
        let (_, saved) =
            save_into(vec![w.clone()], &renamed(&w, "X"), false, &seen, 1_000).unwrap();
        assert_eq!(saved.next_run_at, None);
    }

    #[test]
    fn input_values_of_inputs_that_are_gone_are_dropped() {
        let mut w = ticket_workflow();
        w.input_values
            .insert("ghost", InputValue::String("x".into()));
        let (_, saved) = save_into(vec![ticket_workflow()], &w, true, "", 1_000).unwrap();
        assert!(!saved.input_values.contains_key("ghost"));
    }

    #[test]
    fn a_blank_description_is_not_saved() {
        let mut w = ticket_workflow();
        w.description = Some("   ".into());
        let (_, saved) = save_into(vec![ticket_workflow()], &w, true, "", 1_000).unwrap();
        assert_eq!(saved.description, None);
    }
}

mod problems {
    use super::*;

    #[test]
    fn a_step_with_no_project_says_choose_a_project() {
        let mut w = ticket_workflow();
        for node in &mut w.nodes {
            if node.id == "fetch" {
                if let NodeKind::Session { project, .. } = &mut node.kind {
                    project.clear();
                }
            }
        }
        let issues = problems_of(&w, &ModelCatalog::unprobed());
        assert!(issues
            .iter()
            .any(|i| i.node_id.as_deref() == Some("fetch") && i.message == "Choose a project"));
    }

    #[test]
    fn a_missing_input_value_is_a_problem_without_a_node() {
        let mut w = ticket_workflow();
        w.input_values = OrderedMap::new();
        let issues = problems_of(&w, &ModelCatalog::unprobed());
        assert!(issues
            .iter()
            .any(|i| i.node_id.is_none() && i.message.contains("Missing a value for input")));
    }

    #[test]
    fn a_good_workflow_has_none() {
        let mut w = ticket_workflow();
        w.input_values
            .insert("project", InputValue::String("/work/api".into()));
        w.input_values
            .insert("clickupList", InputValue::String("list".into()));
        assert_eq!(problems_of(&w, &ModelCatalog::unprobed()), vec![]);
    }
}

mod picker {
    use super::*;

    #[test]
    fn groups_are_trigger_action_flow_end() {
        let titles: Vec<&str> = picker_groups().iter().map(|g| g.title).collect();
        assert_eq!(titles, vec!["Trigger", "Action", "Flow", "End"]);
        let types: Vec<NodeType> = picker_groups()
            .iter()
            .flat_map(|g| g.items.iter().map(|(ty, _)| *ty))
            .collect();
        assert_eq!(types.len(), 8);
        for ty in NodeType::ALL {
            assert!(types.contains(&ty));
        }
    }

    #[test]
    fn a_second_trigger_or_report_says_why_not() {
        assert_eq!(
            picker_unavailable(NodeType::Schedule, true, false),
            Some("A workflow has one trigger")
        );
        assert_eq!(
            picker_unavailable(NodeType::Manual, true, false),
            Some("A workflow has one trigger")
        );
        assert_eq!(
            picker_unavailable(NodeType::Report, false, true),
            Some("A workflow has one Report node")
        );
        assert_eq!(picker_unavailable(NodeType::Command, true, true), None);
        assert_eq!(picker_unavailable(NodeType::Schedule, false, true), None);
    }

    #[test]
    fn every_type_has_its_label() {
        assert_eq!(type_label(NodeType::Report), "Report to Mono");
        assert_eq!(type_label(NodeType::AskMono), "Ask Mono");
        assert_eq!(type_label(NodeType::ForEach), "For each");
    }
}

mod fields {
    use super::*;

    fn with_project(path: &str) -> Workflow {
        let mut w = ticket_workflow();
        w.input_values
            .insert("project", InputValue::String(path.into()));
        w
    }

    #[test]
    fn choosing_a_project_fills_the_input_the_step_names_when_it_has_no_value() {
        let mut w = with_project("/work/api");
        w.input_values.remove("project");
        let fetch = w.node("fetch").unwrap().clone();
        let out = project_choice(&w, &fetch, "/work/web", "web");
        let NodeKind::Session { project, .. } = &out.node.kind else {
            panic!("a session")
        };
        assert_eq!(project, "{{inputs.project}}");
        assert_eq!(
            out.input_values.get("project"),
            Some(&InputValue::String("/work/web".into()))
        );
        assert_eq!(out.inputs.len(), w.inputs.len());
    }

    #[test]
    fn choosing_the_project_an_input_already_holds_names_that_input() {
        let w = with_project("/work/api");
        let review = w.node("review").unwrap().clone();
        let out = project_choice(&w, &review, "/work/api", "api");
        let NodeKind::Session { project, .. } = &out.node.kind else {
            panic!("a session")
        };
        assert_eq!(project, "{{inputs.project}}");
        assert_eq!(out.inputs.len(), w.inputs.len());
    }

    #[test]
    fn choosing_a_second_project_for_another_step_adds_an_input() {
        let w = with_project("/work/api");
        let review = w.node("review").unwrap().clone();
        let out = project_choice(&w, &review, "/work/web", "web");
        let NodeKind::Session { project, .. } = &out.node.kind else {
            panic!("a session")
        };
        assert_eq!(project, "{{inputs.web}}");
        assert!(out.inputs.contains_key("web"));
        assert_eq!(
            out.input_values.get("web"),
            Some(&InputValue::String("/work/web".into()))
        );
        // The first input is still used by the other steps.
        assert!(out.inputs.contains_key("project"));
    }

    #[test]
    fn an_input_a_step_leaves_goes_when_nothing_else_mentions_it() {
        let mut w = workflow();
        w.nodes = vec![
            n(json!({"id": "trigger", "type": "manual"})),
            n(
                json!({"id": "dev", "type": "command", "project": "{{inputs.old}}", "command": "ls"}),
            ),
            n(json!({"id": "report", "type": "report"})),
        ];
        w.inputs
            .insert("old", WorkflowInput::new(InputType::Project));
        w.input_values
            .insert("old", InputValue::String("/old".into()));
        let dev = w.node("dev").unwrap().clone();
        let out = project_choice(&w, &dev, "/new", "new");
        let NodeKind::Command { project, .. } = &out.node.kind else {
            panic!("a command")
        };
        assert_eq!(project, "{{inputs.project}}");
        assert!(!out.inputs.contains_key("old"));
        assert!(!out.input_values.contains_key("old"));
        assert!(out.inputs.contains_key("project"));
    }

    #[test]
    fn the_path_a_step_stands_for_is_the_value_of_its_input() {
        let w = with_project("/work/api");
        assert_eq!(
            project_path(&w, w.node("fetch").unwrap()).as_deref(),
            Some("/work/api")
        );
        assert_eq!(project_path(&w, w.node("report").unwrap()), None);
    }

    #[test]
    fn a_new_output_field_gets_a_free_name() {
        let mut fields = OrderedMap::new();
        assert_eq!(next_field_name(&fields), "field");
        fields.insert("field", OutputField::new(OutputFieldType::String));
        assert_eq!(next_field_name(&fields), "field_2");
        fields.insert("field_2", OutputField::new(OutputFieldType::String));
        assert_eq!(next_field_name(&fields), "field_3");
    }

    #[test]
    fn a_name_is_usable_when_it_is_a_plain_identifier() {
        assert!(usable_name("pr_url"));
        assert!(usable_name("_x1"));
        assert!(!usable_name("1x"));
        assert!(!usable_name("a-b"));
        assert!(!usable_name(""));
        assert!(!usable_name("__proto__"));
    }

    #[test]
    fn an_input_name_says_what_is_wrong() {
        let taken = |n: &str| n == "project";
        assert_eq!(input_name_problem("", &taken), None);
        assert_eq!(
            input_name_problem("a b", &taken).as_deref(),
            Some("Use letters, digits and underscores")
        );
        assert_eq!(
            input_name_problem("project", &taken).as_deref(),
            Some("Already declared")
        );
        assert_eq!(input_name_problem("fresh", &taken), None);
    }

    #[test]
    fn the_checkout_choices_read_and_write_the_same_values() {
        assert_eq!(checkout_value(&Checkout::Root), "root");
        assert_eq!(checkout_value(&Checkout::NewWorktree), "newWorktree");
        assert_eq!(
            checkout_value(&Checkout::WorktreeOf("dev".into())),
            "worktree:dev"
        );
        assert_eq!(parse_checkout("root"), Checkout::Root);
        assert_eq!(parse_checkout("newWorktree"), Checkout::NewWorktree);
        assert_eq!(
            parse_checkout("worktree:dev"),
            Checkout::WorktreeOf("dev".into())
        );
        assert_eq!(cwd_value(None), "root");
        assert_eq!(
            cwd_value(Some(&Cwd::WorktreeOf("dev".into()))),
            "worktree:dev"
        );
        assert_eq!(parse_cwd("root"), None);
        assert_eq!(
            parse_cwd("worktree:dev"),
            Some(Cwd::WorktreeOf("dev".into()))
        );
    }

    #[test]
    fn the_worktrees_on_offer_are_the_other_sessions() {
        let w = ticket_workflow();
        let ids: Vec<String> = sessions_except(&w, "dev")
            .into_iter()
            .map(|n| n.id.clone())
            .collect();
        assert_eq!(ids, vec!["fetch", "review"]);
    }

    #[test]
    fn the_reuse_switch_is_for_sessions_in_a_loop_body_only() {
        let w = ticket_workflow();
        assert!(in_loop(&w, "dev"));
        assert!(in_loop(&w, "review"));
        assert!(!in_loop(&w, "fetch"));
    }

    #[test]
    fn an_if_decided_by_the_mono_drops_its_condition_and_back() {
        let mut check = n(json!({"id": "check", "type": "if", "condition": "{{a.b}} == 1"}));
        check = if_decided_by(&check, true);
        let NodeKind::If {
            condition, decide, ..
        } = &check.kind
        else {
            panic!("an if")
        };
        assert_eq!((condition.as_deref(), *decide), (None, Some(Decide::Mono)));
        check = if_decided_by(&check, false);
        let NodeKind::If {
            condition, decide, ..
        } = &check.kind
        else {
            panic!("an if")
        };
        assert_eq!((condition.as_deref(), *decide), (Some(""), None));
    }

    #[test]
    fn a_loop_decided_by_a_condition_remembers_nothing_of_mono() {
        let spec = LoopSpec {
            max_rounds: 3.0,
            decide: "mono".into(),
        };
        assert_eq!(loop_decided_by(&spec, false).decide, "");
        let cond = LoopSpec {
            max_rounds: 3.0,
            decide: "x == 1".into(),
        };
        assert_eq!(loop_decided_by(&cond, false).decide, "x == 1");
        assert_eq!(loop_decided_by(&cond, true).decide, "mono");
    }

    #[test]
    fn toggling_in_report_stores_the_opposite_of_the_effective_value() {
        let mut fields = OrderedMap::new();
        fields.insert("summary", OutputField::new(OutputFieldType::String));
        fields.insert("count", OutputField::new(OutputFieldType::Number));
        let out = toggle_in_report(&fields, "summary");
        assert_eq!(out.get("summary").unwrap().in_report, Some(false));
        let out = toggle_in_report(&out, "count");
        assert_eq!(out.get("count").unwrap().in_report, Some(true));
    }

    #[test]
    fn renaming_an_output_field_keeps_its_place() {
        let mut fields = OrderedMap::new();
        fields.insert("a", OutputField::new(OutputFieldType::String));
        fields.insert("b", OutputField::new(OutputFieldType::Number));
        let out = rename_field(&fields, "a", "z");
        assert_eq!(out.keys().cloned().collect::<Vec<_>>(), vec!["z", "b"]);
    }
}
