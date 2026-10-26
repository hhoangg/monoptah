//! Port of `workflowReport.test.ts`, plus exact-text checks of the report layout and prompt.
use super::engine::{AskPurpose, EngineAction};
use super::fixtures::{edge, node};
use super::model::*;
use super::ordered::{Json, OrderedMap};
use super::report::*;
use serde_json::{json, Value};
use std::collections::BTreeMap;

const PROJECT: &str = "{{inputs.project}}";

fn n(v: Value) -> Node {
    node(&v.to_string())
}

fn e(from: &str, to: &str) -> Edge {
    edge(&json!({"from": from, "to": to}).to_string())
}

fn eh(from: &str, to: &str, handle: &str) -> Edge {
    edge(&json!({"from": from, "to": to, "handle": handle}).to_string())
}

fn lp(from: &str, to: &str, max_rounds: u32, decide: &str) -> Edge {
    edge(
        &json!({"from": from, "to": to, "loop": {"maxRounds": max_rounds, "decide": decide}})
            .to_string(),
    )
}

fn def(nodes: Vec<Node>, edges: Vec<Edge>) -> Workflow {
    let mut w = Workflow::new("wf-aaaaaaaa", "Ship tickets");
    w.inputs
        .insert("project", WorkflowInput::new(InputType::Project));
    w.input_values
        .insert("project", InputValue::String("/work/repo".into()));
    w.nodes = nodes;
    w.edges = edges;
    w.enabled = true;
    w.created_at = 1;
    w
}

fn trigger() -> Node {
    n(json!({"id": "trigger", "type": "manual"}))
}

fn report() -> Node {
    n(json!({"id": "report", "type": "report"}))
}

fn session(id: &str, extra: Value) -> Node {
    let mut v = json!({
        "id": id, "type": "session", "project": PROJECT, "checkout": "root",
        "prompt": format!("Do {id}"), "output": {},
    });
    for (k, x) in extra.as_object().unwrap() {
        v[k] = x.clone();
    }
    n(v)
}

fn nr(node_id: &str) -> NodeRun {
    NodeRun {
        status: NodeStatus::Done,
        ..NodeRun::new(node_id, "")
    }
}

fn out(v: Value) -> Option<OrderedMap<Json>> {
    match Json::from(&v) {
        Json::Object(map) => Some(map),
        other => panic!("not an object: {other:?}"),
    }
}

fn items(v: Value) -> BTreeMap<String, Vec<Json>> {
    v.as_object()
        .unwrap()
        .iter()
        .map(|(k, list)| {
            let Json::Array(list) = Json::from(list) else {
                panic!("not a list")
            };
            (k.clone(), list)
        })
        .collect()
}

fn run_of(definition: Workflow, nodes: Vec<NodeRun>) -> WorkflowRun {
    WorkflowRun {
        id: "run-1".into(),
        workflow_id: definition.id.clone(),
        definition,
        status: RunStatus::Done,
        started_at: 1_000,
        ended_at: Some(2_000),
        trigger: RunTrigger::Manual,
        folder_id: None,
        items: BTreeMap::new(),
        nodes,
        report_posted_at: None,
        error: None,
    }
}

fn decided(value: &str, reason: &str) -> Option<Decision> {
    Some(Decision {
        value: value.into(),
        reason: Some(reason.into()),
    })
}

/// trigger -> each (forEach) -> dev (session with a PR link) -> report.
fn fan_out() -> Workflow {
    def(
        vec![
            trigger(),
            n(json!({"id": "each", "type": "forEach", "in": "{{items}}"})),
            session(
                "dev",
                json!({"parent": "each", "output": {"pr_url": {"type": "string"}, "files": {"type": "list"}}}),
            ),
            report(),
        ],
        vec![e("trigger", "each"), e("each", "report")],
    )
}

mod compose_report_tests {
    use super::*;

    #[test]
    fn report_lists_each_branch_with_pr_link() {
        let mut run = run_of(
            fan_out(),
            vec![
                NodeRun {
                    branch: "each#0".into(),
                    output: out(
                        json!({"pr_url": "https://github.com/o/r/pull/1", "files": ["a.ts"]}),
                    ),
                    ended_at: Some(5),
                    ..nr("dev")
                },
                NodeRun {
                    branch: "each#1".into(),
                    status: NodeStatus::Failed,
                    error: Some("tests did not pass".into()),
                    ..nr("dev")
                },
                NodeRun {
                    branch: "each#2".into(),
                    output: out(json!({"pr_url": "https://github.com/o/r/pull/3"})),
                    ended_at: Some(7),
                    ..nr("dev")
                },
            ],
        );
        run.status = RunStatus::DoneWithFailures;
        run.items =
            items(json!({"each": [{"title": "Fix login"}, {"name": "Add dark mode"}, "plain"]}));
        let report = compose_report(&run);
        assert_eq!(report.title, "Ship tickets · completed with failures");
        let text = &report.text;
        assert!(text.contains("Fix login"));
        assert!(text.contains("https://github.com/o/r/pull/1"));
        assert!(text.contains("Add dark mode"));
        assert!(text.contains("tests did not pass"));
        assert!(text.contains("#3"));
        assert!(text.contains("https://github.com/o/r/pull/3"));
        // The list field is off by default, so it is not rendered.
        assert!(!text.contains("a.ts"));
        assert!(text.contains("failed"));
    }

    #[test]
    fn lists_a_failed_branch_of_an_earlier_loop_round_and_keeps_the_latest_round_as_the_main_line()
    {
        let mut run = run_of(
            fan_out(),
            vec![
                NodeRun {
                    branch: "each#0".into(),
                    output: out(json!({"pr_url": "https://x/new"})),
                    ended_at: Some(9),
                    ..nr("dev")
                },
                NodeRun {
                    branch: "each#0@r1".into(),
                    status: NodeStatus::Failed,
                    error: Some("tests did not pass".into()),
                    ..nr("dev")
                },
                NodeRun {
                    branch: "each#1@r1".into(),
                    output: out(json!({"pr_url": "https://x/old-ok"})),
                    ended_at: Some(4),
                    ..nr("dev")
                },
                NodeRun {
                    branch: "each#1".into(),
                    output: out(json!({"pr_url": "https://x/ok"})),
                    ended_at: Some(9),
                    ..nr("dev")
                },
            ],
        );
        run.status = RunStatus::DoneWithFailures;
        run.items = items(json!({
            "each": [{"title": "Fix login"}, {"title": "Billing"}],
            "each@r1": [{"title": "Fix login"}, {"title": "Billing"}],
        }));
        let text = compose_report(&run).text;
        assert!(text.contains("### Fix login\n- Status: done"));
        assert!(text.contains("https://x/new"));
        assert!(text.contains("### Fix login · round 1\n- Status: failed"));
        assert!(text.contains("Error in dev: tests did not pass"));
        // A round that passed is not repeated.
        assert!(!text.contains("Billing · round 1"));
        assert!(!text.contains("https://x/old-ok"));
    }

    #[test]
    fn lists_a_failed_archived_branch_at_an_index_the_current_list_no_longer_has() {
        let mut run = run_of(
            fan_out(),
            vec![
                NodeRun {
                    branch: "each#0".into(),
                    output: out(json!({"pr_url": "https://x/1"})),
                    ended_at: Some(9),
                    ..nr("dev")
                },
                NodeRun {
                    branch: "each#0@r1".into(),
                    output: out(json!({"pr_url": "https://x/old"})),
                    ended_at: Some(4),
                    ..nr("dev")
                },
                NodeRun {
                    branch: "each#1@r1".into(),
                    status: NodeStatus::Failed,
                    error: Some("lost in round 1".into()),
                    ..nr("dev")
                },
            ],
        );
        run.status = RunStatus::DoneWithFailures;
        run.items = items(json!({
            "each": [{"title": "A"}],
            "each@r1": [{"title": "A"}, {"title": "B"}],
        }));
        let text = compose_report(&run).text;
        assert!(text.contains("### B · round 1\n- Status: failed"));
        assert!(text.contains("Error in dev: lost in round 1"));
        assert!(!text.contains("### B\n"));
        assert!(!text.contains("A · round 1"));
    }

    #[test]
    fn still_lists_archived_failures_when_the_for_each_never_expanded_again() {
        let nodes = vec![
            NodeRun {
                branch: "each#0@r1".into(),
                status: NodeStatus::Failed,
                error: Some("boom".into()),
                ..nr("dev")
            },
            NodeRun {
                branch: "each#1@r1".into(),
                ..nr("dev")
            },
        ];
        let mut run = run_of(fan_out(), nodes);
        run.status = RunStatus::DoneWithFailures;
        run.items = items(json!({"each@r1": [{"title": "A"}, {"title": "B"}]}));
        let text = compose_report(&run).text;
        assert!(text.contains("### A · round 1\n- Status: failed"));
        assert!(text.contains("Error in dev: boom"));
        assert!(!text.contains("B · round 1"));
        // Without the old item list the title falls back to the place.
        let mut bare = run_of(fan_out(), vec![run.nodes[0].clone()]);
        bare.status = RunStatus::DoneWithFailures;
        assert!(compose_report(&bare).text.contains("### #1 · round 1"));
    }

    #[test]
    fn lists_an_archived_branch_that_hit_max_rounds() {
        let mut run = run_of(
            fan_out(),
            vec![
                NodeRun {
                    branch: "each#0".into(),
                    output: out(json!({"pr_url": "https://x/1"})),
                    ended_at: Some(9),
                    ..nr("dev")
                },
                NodeRun {
                    branch: "each#0@r1".into(),
                    decision: decided("exit", "max rounds reached"),
                    ..nr("dev")
                },
            ],
        );
        run.items = items(json!({"each": [{"title": "A"}], "each@r1": [{"title": "A"}]}));
        assert!(compose_report(&run).text.contains("### A · round 1"));
    }

    #[test]
    fn shows_top_level_session_outputs_next_to_a_for_each() {
        let d = def(
            vec![
                trigger(),
                n(json!({"id": "each", "type": "forEach", "in": "{{items}}"})),
                session(
                    "dev",
                    json!({"parent": "each", "output": {"pr_url": {"type": "string"}}}),
                ),
                session(
                    "summary",
                    json!({"output": {"digest": {"type": "string"}, "raw": {"type": "list"}}}),
                ),
                report(),
            ],
            vec![
                e("trigger", "each"),
                e("each", "summary"),
                e("summary", "report"),
            ],
        );
        let nodes = vec![
            NodeRun {
                branch: "each#0".into(),
                output: out(json!({"pr_url": "https://x/1"})),
                ended_at: Some(2),
                ..nr("dev")
            },
            NodeRun {
                output: out(json!({"digest": "One PR opened", "raw": ["x"]})),
                ended_at: Some(3),
                ..nr("summary")
            },
        ];
        let mut run = run_of(d.clone(), nodes);
        run.items = items(json!({"each": [{"title": "T"}]}));
        let text = compose_report(&run).text;
        assert!(text.contains("summary.digest: One PR opened"));
        assert!(!text.contains("raw"));
        assert!(text.contains("https://x/1"));
        // Without any reported top-level output there is no extra section.
        let mut quiet = run_of(d, vec![run.nodes[0].clone()]);
        quiet.items = run.items.clone();
        assert!(!compose_report(&quiet).text.contains("### Run"));
    }

    fn dev_review() -> Workflow {
        def(
            vec![
                trigger(),
                session("dev", json!({"output": {"pr_url": {"type": "string"}}})),
                session("review", json!({"output": {"verdict": {"type": "string"}}})),
                report(),
            ],
            vec![
                e("trigger", "dev"),
                e("dev", "review"),
                e("review", "report"),
                lp("review", "dev", 3, "mono"),
            ],
        )
    }

    #[test]
    fn report_flags_max_rounds() {
        let run = run_of(
            dev_review(),
            vec![
                NodeRun {
                    round: 3,
                    output: out(json!({"pr_url": "https://x/1"})),
                    ended_at: Some(3),
                    ..nr("dev")
                },
                NodeRun {
                    round: 3,
                    output: out(json!({"verdict": "changes"})),
                    decision: decided("exit", "max rounds reached"),
                    ended_at: Some(4),
                    ..nr("review")
                },
            ],
        );
        let text = compose_report(&run).text;
        assert!(text.contains("Max rounds reached"));
        assert!(text.contains("Loop rounds used: 3"));
        // The last session node is review, so its verdict is shown.
        assert!(text.contains("changes"));
    }

    #[test]
    fn shows_when_a_loop_ended_because_the_branch_failed() {
        let d = def(
            vec![
                trigger(),
                session("dev", json!({})),
                session("review", json!({})),
                report(),
            ],
            vec![
                e("trigger", "dev"),
                e("dev", "review"),
                e("review", "report"),
                lp("review", "dev", 3, "mono"),
            ],
        );
        let mut run = run_of(
            d,
            vec![
                NodeRun {
                    status: NodeStatus::Failed,
                    error: Some("boom".into()),
                    round: 2,
                    ..nr("dev")
                },
                NodeRun {
                    decision: decided("exit", "branch failed"),
                    round: 1,
                    ..nr("review")
                },
            ],
        );
        run.status = RunStatus::Failed;
        let report = compose_report(&run);
        assert_eq!(report.title, "Ship tickets · failed");
        assert!(report.text.contains("branch failed"));
        assert!(report.text.contains("boom"));
        assert!(!report.text.contains("Max rounds reached"));
    }

    #[test]
    fn report_for_empty_for_each_says_no_items() {
        let mut run = run_of(fan_out(), vec![]);
        run.items = items(json!({"each": []}));
        assert!(compose_report(&run).text.contains("No items"));
    }

    #[test]
    fn template_can_read_top_level_node_outputs_and_inputs() {
        let d = def(
            vec![
                trigger(),
                session(
                    "session",
                    json!({"output": {"weather": {"type": "string"}, "summary": {"type": "string"}}}),
                ),
                n(
                    json!({"id": "gate", "type": "if", "condition": "{{session.weather}} == 'rain'"}),
                ),
                session("session_2", json!({})),
                session("session_3", json!({})),
                n(json!({
                    "id": "report", "type": "report",
                    "template": "Đà Nẵng hôm nay: {{session.weather}}. {{session.summary}} ({{inputs.project}}, {{run.id}}, {{run.status}})",
                })),
            ],
            vec![
                e("trigger", "session"),
                e("session", "gate"),
                eh("gate", "session_2", "true"),
                eh("gate", "session_3", "false"),
                e("session_2", "report"),
                e("session_3", "report"),
            ],
        );
        let run = run_of(
            d,
            vec![
                NodeRun {
                    output: out(json!({"weather": "nắng", "summary": "Trời đẹp."})),
                    ended_at: Some(2),
                    ..nr("session")
                },
                NodeRun {
                    status: NodeStatus::Skipped,
                    ..nr("session_2")
                },
                NodeRun {
                    status: NodeStatus::Skipped,
                    ..nr("session_3")
                },
            ],
        );
        assert!(compose_report(&run)
            .text
            .contains("Đà Nẵng hôm nay: nắng. Trời đẹp. (/work/repo, run-1, done)"));
    }

    #[test]
    fn template_override_renders() {
        let d = def(
            vec![
                trigger(),
                n(json!({"id": "each", "type": "forEach", "in": "{{items}}"})),
                session(
                    "dev",
                    json!({"parent": "each", "output": {"pr_url": {"type": "string"}}}),
                ),
                n(
                    json!({"id": "report", "type": "report", "template": "RUN {{run.id}} is {{run.status}}\n{{results}}"}),
                ),
            ],
            vec![e("trigger", "each"), e("each", "report")],
        );
        let mut run = run_of(
            d,
            vec![NodeRun {
                branch: "each#0".into(),
                output: out(json!({"pr_url": "https://x/9"})),
                ended_at: Some(2),
                ..nr("dev")
            }],
        );
        run.items = items(json!({"each": [{"title": "T"}]}));
        let report = compose_report(&run);
        assert_eq!(report.title, "Ship tickets · completed");
        assert!(report.text.contains("RUN run-1 is done"));
        assert!(report.text.contains("https://x/9"));
        // The default per-branch layout is replaced.
        assert!(!report.text.contains("Loop rounds"));
        assert!(!report.text.contains("### "));
    }

    #[test]
    fn honours_in_report_overrides() {
        let d = def(
            vec![
                trigger(),
                session(
                    "dev",
                    json!({"output": {
                        "summary": {"type": "string", "inReport": false},
                        "files": {"type": "list", "inReport": true},
                        "n": {"type": "number"},
                        "ok": {"type": "boolean", "inReport": true},
                    }}),
                ),
                report(),
            ],
            vec![e("trigger", "dev"), e("dev", "report")],
        );
        let run = run_of(
            d,
            vec![NodeRun {
                output: out(
                    json!({"summary": "hidden text", "files": ["a.ts", "b.ts"], "n": 42, "ok": true}),
                ),
                ended_at: Some(1),
                ..nr("dev")
            }],
        );
        let text = compose_report(&run).text;
        assert!(!text.contains("hidden text"));
        assert!(text.contains("a.ts"));
        assert!(text.contains("b.ts"));
        assert!(!text.contains("42"));
        assert!(text.contains("true"));
    }

    #[test]
    fn keeps_short_string_values_on_one_line() {
        let d = def(
            vec![
                trigger(),
                session("dev", json!({"output": {"note": {"type": "string"}}})),
                report(),
            ],
            vec![e("trigger", "dev"), e("dev", "report")],
        );
        let run = run_of(
            d,
            vec![NodeRun {
                output: out(json!({"note": "line one\nline two"})),
                ended_at: Some(1),
                ..nr("dev")
            }],
        );
        assert!(compose_report(&run)
            .text
            .contains("note: line one line two"));
    }

    #[test]
    fn labels_each_run_status() {
        for (status, label) in [
            (RunStatus::Done, "completed"),
            (RunStatus::DoneWithFailures, "completed with failures"),
            (RunStatus::Failed, "failed"),
            (RunStatus::Stopped, "stopped"),
            (RunStatus::Interrupted, "interrupted"),
        ] {
            let mut run = run_of(
                def(vec![trigger(), report()], vec![e("trigger", "report")]),
                vec![],
            );
            run.status = status;
            assert_eq!(
                compose_report(&run).title,
                format!("Ship tickets · {label}")
            );
        }
    }

    #[test]
    fn labels_a_running_run() {
        let mut run = run_of(
            def(vec![trigger(), report()], vec![e("trigger", "report")]),
            vec![],
        );
        run.status = RunStatus::Running;
        assert_eq!(compose_report(&run).title, "Ship tickets · running");
    }
}

mod layout {
    use super::*;

    fn one_session(output: Value) -> Workflow {
        def(
            vec![
                trigger(),
                session("dev", json!({ "output": output })),
                report(),
            ],
            vec![e("trigger", "dev"), e("dev", "report")],
        )
    }

    #[test]
    fn a_plain_run_is_one_run_section() {
        let run = run_of(
            one_session(json!({"summary": {"type": "string"}})),
            vec![NodeRun {
                output: out(json!({"summary": "All good"})),
                ended_at: Some(1),
                ..nr("dev")
            }],
        );
        assert_eq!(
            compose_report(&run).text,
            "## Ship tickets · completed\n\n### Run\n- Status: done\n- summary: All good"
        );
    }

    #[test]
    fn a_run_error_comes_first_and_is_one_line() {
        let mut run = run_of(one_session(json!({})), vec![]);
        run.status = RunStatus::Failed;
        run.error = Some("could not\n   start".into());
        let text = compose_report(&run).text;
        assert!(text.starts_with(
            "## Ship tickets · failed\n\nRun error: could not start\n\n### Run\n- Status: pending"
        ));
    }

    #[test]
    fn long_strings_become_a_quote_block() {
        let long = format!("{}\r\nsecond", "a".repeat(201));
        let run = run_of(
            one_session(json!({"summary": {"type": "string"}})),
            vec![NodeRun {
                output: out(json!({ "summary": long })),
                ended_at: Some(1),
                ..nr("dev")
            }],
        );
        let text = compose_report(&run).text;
        assert!(text.contains(&format!("- summary:\n  > {}\n  > second", "a".repeat(201))));
        // 200 characters stay inline.
        let run = run_of(
            one_session(json!({"summary": {"type": "string"}})),
            vec![NodeRun {
                output: out(json!({"summary": "b".repeat(200)})),
                ended_at: Some(1),
                ..nr("dev")
            }],
        );
        assert!(compose_report(&run)
            .text
            .contains(&format!("- summary: {}", "b".repeat(200))));
    }

    #[test]
    fn the_length_limit_counts_utf16_units_like_js() {
        // 101 astral characters are 202 UTF-16 units, so they are quoted, not inline.
        let run = run_of(
            one_session(json!({"summary": {"type": "string"}})),
            vec![NodeRun {
                output: out(json!({"summary": "😀".repeat(101)})),
                ended_at: Some(1),
                ..nr("dev")
            }],
        );
        assert!(compose_report(&run).text.contains("- summary:\n  > 😀"));
    }

    #[test]
    fn objects_and_lists_are_fenced_json_longer_than_any_backtick_run() {
        let run = run_of(
            one_session(json!({
                "o": {"type": "object", "inReport": true},
                "l": {"type": "list", "inReport": true},
            })),
            vec![NodeRun {
                output: out(json!({"o": {"k": "a ```` b"}, "l": [1, 2.5]})),
                ended_at: Some(1),
                ..nr("dev")
            }],
        );
        let text = compose_report(&run).text;
        assert!(text.contains("- o:\n  `````json\n  {\n    \"k\": \"a ```` b\"\n  }\n  `````"));
        assert!(text.contains("- l:\n  ```json\n  [\n    1,\n    2.5\n  ]\n  ```"));
    }

    #[test]
    fn numbers_and_booleans_print_the_js_way() {
        let run = run_of(
            one_session(json!({
                "n": {"type": "number", "inReport": true},
                "b": {"type": "boolean", "inReport": true},
            })),
            vec![NodeRun {
                output: out(json!({"n": 1e21, "b": false})),
                ended_at: Some(1),
                ..nr("dev")
            }],
        );
        let text = compose_report(&run).text;
        // (Field order follows the declaration, which is sorted in this build's json! maps.)
        assert!(text.contains("- n: 1e+21"));
        assert!(text.contains("- b: false"));
    }

    #[test]
    fn two_for_eaches_name_the_empty_one() {
        let d = def(
            vec![
                trigger(),
                n(json!({"id": "a", "type": "forEach", "title": "Alpha", "in": "{{x}}"})),
                n(json!({"id": "b", "type": "forEach", "in": "{{y}}"})),
                report(),
            ],
            vec![e("trigger", "a"), e("a", "b"), e("b", "report")],
        );
        let mut run = run_of(d, vec![]);
        run.items = items(json!({"a": [], "b": []}));
        let text = compose_report(&run).text;
        assert!(text.contains("No items in Alpha\n\nNo items in b"));
    }

    #[test]
    fn an_empty_error_text_stays_empty_and_a_missing_one_says_failed() {
        let nodes = |error: Option<&str>| {
            vec![NodeRun {
                status: NodeStatus::Failed,
                error: error.map(str::to_owned),
                ..nr("dev")
            }]
        };
        let text = |error| compose_report(&run_of(one_session(json!({})), nodes(error))).text;
        // The report is trimmed at the end, so the empty text leaves the label alone on its line.
        assert!(text(Some("")).ends_with("- Error in dev:"));
        assert!(text(None).contains("- Error in dev: failed"));
    }

    #[test]
    fn a_blank_template_falls_back_to_the_default_layout() {
        let d = def(
            vec![
                trigger(),
                n(json!({"id": "report", "type": "report", "template": " \n "})),
            ],
            vec![e("trigger", "report")],
        );
        let run = run_of(d, vec![]);
        assert!(compose_report(&run).text.contains("### Run"));
    }

    #[test]
    fn the_report_title_and_run_fields_reach_a_template() {
        let d = def(
            vec![
                trigger(),
                n(json!({
                    "id": "report", "type": "report",
                    "template": "{{run.name}}|{{run.statusLabel}}|{{run.endedAt}}|{{run.startedAt}}|{{run.round}}",
                })),
            ],
            vec![e("trigger", "report")],
        );
        let mut run = run_of(d, vec![]);
        run.status = RunStatus::DoneWithFailures;
        assert_eq!(
            compose_report(&run).text,
            "## Ship tickets · completed with failures\n\nShip tickets|completed with failures|2000|1000|1"
        );
        run.ended_at = None;
        assert!(compose_report(&run)
            .text
            .ends_with("|completed with failures||1000|1"));
    }

    #[test]
    fn results_in_a_template_carry_the_branch_fields() {
        let d = def(
            vec![
                trigger(),
                n(json!({"id": "each", "type": "forEach", "in": "{{items}}"})),
                session(
                    "dev",
                    json!({"parent": "each", "output": {"pr": {"type": "string"}}}),
                ),
                n(json!({"id": "report", "type": "report", "template": "{{results}}"})),
            ],
            vec![e("trigger", "each"), e("each", "report")],
        );
        let mut run = run_of(
            d,
            vec![NodeRun {
                branch: "each#0".into(),
                output: out(json!({"pr": "p1"})),
                ended_at: Some(2),
                ..nr("dev")
            }],
        );
        run.items = items(json!({"each": [{"title": "T"}]}));
        let text = compose_report(&run).text;
        let body = text.split_once("\n\n").unwrap().1;
        let parsed = Json::parse(body).unwrap();
        let first = &parsed.as_array().unwrap()[0];
        assert_eq!(first.get("branch"), Some(&Json::String("each#0".into())));
        assert_eq!(first.get("name"), Some(&Json::String("T".into())));
        assert_eq!(first.get("status"), Some(&Json::String("done".into())));
        assert_eq!(first.get("rounds"), Some(&Json::Number(0.0)));
        assert_eq!(first.get("maxRoundsReached"), Some(&Json::Bool(false)));
        assert_eq!(first.get("errors"), Some(&Json::Array(vec![])));
        assert_eq!(
            first.get("output").and_then(|o| o.get("pr")),
            Some(&Json::String("p1".into()))
        );
    }

    #[test]
    fn branch_status_follows_the_most_urgent_node() {
        let d = def(
            vec![
                trigger(),
                session("a", json!({})),
                session("b", json!({})),
                report(),
            ],
            vec![e("trigger", "a"), e("a", "b"), e("b", "report")],
        );
        let status = |nodes: Vec<NodeRun>| {
            let text = compose_report(&run_of(d.clone(), nodes)).text;
            text.lines()
                .find_map(|l| l.strip_prefix("- Status: "))
                .unwrap()
                .to_string()
        };
        let with = |a: NodeStatus, b: NodeStatus| {
            vec![
                NodeRun {
                    status: a,
                    ..nr("a")
                },
                NodeRun {
                    status: b,
                    ..nr("b")
                },
            ]
        };
        assert_eq!(status(vec![]), "pending");
        assert_eq!(status(with(NodeStatus::Done, NodeStatus::Failed)), "failed");
        assert_eq!(
            status(with(NodeStatus::Stopped, NodeStatus::Running)),
            "stopped"
        );
        assert_eq!(
            status(with(NodeStatus::Done, NodeStatus::WaitingApproval)),
            "waiting for approval"
        );
        assert_eq!(
            status(with(NodeStatus::Done, NodeStatus::Running)),
            "running"
        );
        assert_eq!(
            status(with(NodeStatus::Done, NodeStatus::Pending)),
            "pending"
        );
        assert_eq!(status(with(NodeStatus::Done, NodeStatus::Skipped)), "done");
        assert_eq!(
            status(with(NodeStatus::Skipped, NodeStatus::Skipped)),
            "skipped"
        );
    }
}

mod ask_mono_prompt_tests {
    use super::*;

    fn action() -> EngineAction {
        let Some(context) = out(json!({"dev": {"pr_url": "https://x/1"}, "round": 2})) else {
            unreachable!()
        };
        EngineAction::AskMono {
            node_id: "gate".into(),
            branch: "each#0".into(),
            purpose: AskPurpose::AskMono,
            question: "Is the PR safe to merge?".into(),
            answers: vec!["yes".into(), "no".into()],
            context,
        }
    }

    #[test]
    fn tells_the_mono_it_is_consulted_by_a_workflow_and_asks_for_a_workflow_output_block() {
        let action = action();
        let text = ask_mono_prompt(&action, "Ship tickets").unwrap();
        assert!(text.contains("Ship tickets"));
        assert!(text.contains("running workflow"));
        assert!(text.contains("Is the PR safe to merge?"));
        let EngineAction::AskMono { context, .. } = &action else {
            unreachable!()
        };
        assert!(text.contains(&Json::Object(context.clone()).stringify(true)));
        assert!(text.contains("\"yes\""));
        assert!(text.contains("\"no\""));
        assert!(text.trim_end().ends_with("```"));
        assert!(text.contains("```workflow-output"));
        assert!(text.contains("{\"answer\":"));
    }

    #[test]
    fn the_whole_prompt_is_the_react_text() {
        let text = ask_mono_prompt(&action(), "Ship\n  tickets").unwrap();
        let expected = [
            "You are being consulted by a running workflow, \"Ship tickets\", at step \"gate\".",
            "This is a single decision request. Do not start any work or use any tools; just decide.",
            "",
            "Question:",
            "Is the PR safe to merge?",
            "",
            "Relevant outputs so far:",
            "```json",
            "{\n  \"dev\": {\n    \"pr_url\": \"https://x/1\"\n  },\n  \"round\": 2\n}",
            "```",
            "",
            "Allowed answers: \"yes\", \"no\"",
            "",
            "Reply with a short explanation if you like, and end with exactly one fenced code block whose info string is `workflow-output`:",
            "```workflow-output",
            "{\"answer\": <one of the allowed answers>, \"reason\": \"...\"}",
            "```",
        ]
        .join("\n");
        assert_eq!(text, expected);
    }

    #[test]
    fn the_context_fence_outgrows_backticks_inside_it() {
        let mut action = action();
        let EngineAction::AskMono { context, .. } = &mut action else {
            unreachable!()
        };
        context.insert("note", Json::String("x ````` y".into()));
        let text = ask_mono_prompt(&action, "W").unwrap();
        assert!(text.contains("``````json\n"));
        assert!(text.contains("\n``````\n\nAllowed answers"));
    }

    #[test]
    fn only_an_ask_mono_action_has_a_prompt() {
        assert_eq!(ask_mono_prompt(&EngineAction::PostReport, "W"), None);
    }
}

mod parse_mono_decision_tests {
    use super::*;

    fn reply(body: &str) -> String {
        format!("Thinking out loud.\n\n```workflow-output\n{body}\n```")
    }

    fn answers() -> Vec<String> {
        vec!["yes".into(), "no".into()]
    }

    #[test]
    fn accepts_an_allowed_answer_with_a_reason() {
        assert_eq!(
            parse_mono_decision(
                &reply(r#"{"answer":"yes","reason":"all green"}"#),
                &answers()
            ),
            Ok(MonoDecision {
                value: "yes".into(),
                reason: Some("all green".into())
            })
        );
    }

    #[test]
    fn treats_the_reason_as_optional() {
        assert_eq!(
            parse_mono_decision(&reply(r#"{"answer":"no"}"#), &answers()),
            Ok(MonoDecision {
                value: "no".into(),
                reason: None
            })
        );
        // An empty or non-text reason is dropped.
        assert_eq!(
            parse_mono_decision(&reply(r#"{"answer":"no","reason":""}"#), &answers())
                .unwrap()
                .reason,
            None
        );
        assert_eq!(
            parse_mono_decision(&reply(r#"{"answer":"no","reason":3}"#), &answers())
                .unwrap()
                .reason,
            None
        );
    }

    #[test]
    fn parse_mono_decision_rejects_answers_outside_the_list() {
        let error = parse_mono_decision(&reply(r#"{"answer":"maybe"}"#), &answers()).unwrap_err();
        assert!(error.contains("maybe"));
        assert!(error.contains("yes"));
        assert_eq!(
            error,
            "Answer \"maybe\" is not allowed; choose one of: \"yes\", \"no\"."
        );
    }

    #[test]
    fn rejects_a_reply_without_the_block_and_a_non_string_answer() {
        assert!(parse_mono_decision("yes", &["yes".to_string()]).is_err());
        assert!(parse_mono_decision(&reply(r#"{"answer":1}"#), &["1".to_string()]).is_err());
        assert!(parse_mono_decision(&reply(r#"{"reason":"x"}"#), &["yes".to_string()]).is_err());
    }

    #[test]
    fn uses_the_output_contract_error_text() {
        assert_eq!(
            parse_mono_decision("yes", &["yes".to_string()]).unwrap_err(),
            "No workflow-output block found in the reply."
        );
    }
}

mod field_in_report_tests {
    use super::*;

    #[test]
    fn defaults_to_on_for_strings_and_off_for_the_rest() {
        assert!(field_in_report(&OutputField::new(OutputFieldType::String)));
        assert!(!field_in_report(&OutputField::new(OutputFieldType::Number)));
        assert!(!field_in_report(&OutputField::new(
            OutputFieldType::Boolean
        )));
    }

    #[test]
    fn follows_an_explicit_in_report() {
        let field = |ty, in_report| OutputField {
            in_report: Some(in_report),
            ..OutputField::new(ty)
        };
        assert!(!field_in_report(&field(OutputFieldType::String, false)));
        assert!(field_in_report(&field(OutputFieldType::Number, true)));
    }
}
