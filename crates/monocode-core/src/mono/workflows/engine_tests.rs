//! Port of `workflowEngine.test.ts`, plus checks of the Rust-side contract (run file shape,
//! rejected answers, list expressions).
use super::engine::*;
use super::fixtures::{edge, node};
use super::model::*;
use super::ordered::{Json, OrderedMap};
use serde_json::{json, Value};

const PROJECT: &str = "{{inputs.project}}";
const AT: u64 = 1_000;

fn n(v: Value) -> Node {
    node(&v.to_string())
}

fn def(nodes: Vec<Node>, edges: Vec<Edge>) -> Workflow {
    let mut w = Workflow::new("wf-aaaaaaaa", "Test");
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

/// A session node; `extra` overrides or adds fields.
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

fn plain(id: &str) -> Node {
    session(id, json!({}))
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

/// trigger -> fetch (session with a tickets list) -> each (forEach over it) -> report.
fn fan_out(max_parallel: u32, children: Option<Vec<Node>>, edges: Vec<Edge>) -> Workflow {
    let children = children.unwrap_or_else(|| {
        vec![session(
            "dev",
            json!({
                "parent": "each", "prompt": "Implement {{item.title}}",
                "output": {"pr_url": {"type": "string"}},
            }),
        )]
    });
    let mut nodes = vec![
        trigger(),
        session("fetch", json!({"output": {"tickets": {"type": "list"}}})),
        n(json!({
            "id": "each", "type": "forEach", "in": "{{fetch.tickets}}", "as": "item",
            "maxParallel": max_parallel,
        })),
    ];
    nodes.extend(children);
    nodes.push(report());
    let mut all = vec![
        e("trigger", "fetch"),
        e("fetch", "each"),
        e("each", "report"),
    ];
    all.extend(edges);
    def(nodes, all)
}

fn default_fan_out() -> Workflow {
    fan_out(2, None, vec![])
}

/// trigger -> dev -> review (loop back to dev) -> report.
fn dev_review(decide: &str, max_rounds: u32, reuse_session: Option<bool>) -> Workflow {
    let mut dev = json!({
        "prompt": "Implement round {{run.round}}.{{#if review}} Fix: {{review.issues}}{{/if}}",
        "output": {"pr_url": {"type": "string"}},
    });
    if let Some(r) = reuse_session {
        dev["reuseSession"] = json!(r);
    }
    def(
        vec![
            trigger(),
            session("dev", dev),
            session(
                "review",
                json!({"prompt": "Review {{dev.pr_url}}", "output": {"verdict": {"type": "string"}}}),
            ),
            report(),
        ],
        vec![
            e("trigger", "dev"),
            e("dev", "review"),
            e("review", "report"),
            lp("review", "dev", max_rounds, decide),
        ],
    )
}

fn mono_loop() -> Workflow {
    dev_review("mono", 3, None)
}

fn out(v: Value) -> OrderedMap<Json> {
    match Json::from(&v) {
        Json::Object(map) => map,
        _ => panic!("not an object"),
    }
}

fn jv(v: Value) -> Json {
    Json::from(&v)
}

fn started(run: &WorkflowRun, id: &str) -> WorkflowRun {
    started_on(run, id, "", None)
}

fn started_on(run: &WorkflowRun, id: &str, branch: &str, session_id: Option<&str>) -> WorkflowRun {
    started_at(run, id, branch, session_id, AT)
}

fn started_at(
    run: &WorkflowRun,
    id: &str,
    branch: &str,
    session_id: Option<&str>,
    at: u64,
) -> WorkflowRun {
    apply_event(
        run,
        &EngineEvent::NodeStarted {
            node_id: id.into(),
            branch: branch.into(),
            session_id: session_id.map(String::from),
            at,
        },
    )
}

fn finished(run: &WorkflowRun, id: &str, output: Value, branch: &str, at: u64) -> WorkflowRun {
    apply_event(
        run,
        &EngineEvent::NodeFinished {
            node_id: id.into(),
            branch: branch.into(),
            output: out(output),
            at,
        },
    )
}

fn failed(run: &WorkflowRun, id: &str, error: &str, branch: &str) -> WorkflowRun {
    apply_event(
        run,
        &EngineEvent::NodeFailed {
            node_id: id.into(),
            branch: branch.into(),
            error: error.into(),
            at: AT,
        },
    )
}

fn decided_on(
    run: &WorkflowRun,
    id: &str,
    value: &str,
    branch: &str,
    reason: Option<&str>,
) -> WorkflowRun {
    apply_event(
        run,
        &EngineEvent::Decided {
            node_id: id.into(),
            branch: branch.into(),
            value: value.into(),
            reason: reason.map(String::from),
            at: AT,
        },
    )
}

fn decided(run: &WorkflowRun, id: &str, value: &str) -> WorkflowRun {
    decided_on(run, id, value, "", None)
}

/// Starts and finishes a node in one go.
fn complete(run: &WorkflowRun, id: &str, output: Value) -> WorkflowRun {
    complete_on(run, id, output, "")
}

fn complete_on(run: &WorkflowRun, id: &str, output: Value, branch: &str) -> WorkflowRun {
    complete_at(run, id, output, branch, AT)
}

fn complete_at(run: &WorkflowRun, id: &str, output: Value, branch: &str, at: u64) -> WorkflowRun {
    finished(
        &started_at(run, id, branch, None, at),
        id,
        output,
        branch,
        at,
    )
}

fn status_of(run: &WorkflowRun, id: &str, branch: &str) -> Option<NodeStatus> {
    run.nodes
        .iter()
        .find(|x| x.node_id == id && x.branch == branch)
        .map(|x| x.status)
}

fn node_of<'a>(run: &'a WorkflowRun, id: &str, branch: &str) -> &'a NodeRun {
    run.nodes
        .iter()
        .find(|x| x.node_id == id && x.branch == branch)
        .unwrap_or_else(|| panic!("no node run {id}/{branch}"))
}

fn begin(w: &Workflow) -> WorkflowRun {
    start_run(w, RunTrigger::Manual, AT, "r")
}

fn start_session(
    id: &str,
    branch: &str,
    round: u32,
    prompt: &str,
    reuse: Option<&str>,
) -> EngineAction {
    EngineAction::StartSession {
        node_id: id.into(),
        branch: branch.into(),
        round,
        prompt: prompt.into(),
        reuse_session_id: reuse.map(String::from),
    }
}

fn run_command(id: &str, command: &str) -> EngineAction {
    EngineAction::RunCommand {
        node_id: id.into(),
        branch: "".into(),
        command: command.into(),
    }
}

/// `(nodeId, branch)` of every action, which must all be startSession (like the React tests'
/// `toEqual` over the whole list, where any other action would not match).
fn session_targets(actions: &[EngineAction]) -> Vec<(String, String)> {
    actions
        .iter()
        .map(|a| match a {
            EngineAction::StartSession {
                node_id, branch, ..
            } => (node_id.clone(), branch.clone()),
            other => panic!("expected only startSession actions, got {other:?}"),
        })
        .collect()
}

fn pairs(list: &[(&str, &str)]) -> Vec<(String, String)> {
    list.iter()
        .map(|(a, b)| (a.to_string(), b.to_string()))
        .collect()
}

fn session_node_ids(actions: &[EngineAction]) -> Vec<String> {
    session_targets(actions)
        .into_iter()
        .map(|(id, _)| id)
        .collect()
}

fn branch_of(a: &EngineAction) -> Option<&str> {
    match a {
        EngineAction::StartSession { branch, .. }
        | EngineAction::RunCommand { branch, .. }
        | EngineAction::AskMono { branch, .. } => Some(branch),
        EngineAction::PostReport => None,
    }
}

fn no_archived_branch(actions: &[EngineAction]) -> bool {
    actions
        .iter()
        .all(|a| branch_of(a).is_none_or(|b| !b.contains('@')))
}

fn prompt_of(a: &EngineAction) -> &str {
    match a {
        EngineAction::StartSession { prompt, .. } => prompt,
        other => panic!("not a startSession: {other:?}"),
    }
}

fn reported(run: &WorkflowRun, at: u64) -> WorkflowRun {
    apply_event(run, &EngineEvent::ReportPosted { at })
}

fn tickets() -> Vec<Json> {
    vec![
        jv(json!({"title": "A"})),
        jv(json!({"title": "B"})),
        jv(json!({"title": "C"})),
    ]
}

fn tickets_value() -> Value {
    json!([{"title": "A"}, {"title": "B"}, {"title": "C"}])
}

fn post_report() -> Vec<EngineAction> {
    vec![EngineAction::PostReport]
}

// ---------------------------------------------------------------------------------------------
// workflowEngine
// ---------------------------------------------------------------------------------------------

#[test]
fn linear_trigger_fetch_report_emits_start_session_then_post_report_then_done() {
    let mut run = start_run(
        &def(
            vec![
                trigger(),
                session("fetch", json!({"prompt": "Fetch for {{inputs.project}}"})),
                report(),
            ],
            vec![e("trigger", "fetch"), e("fetch", "report")],
        ),
        RunTrigger::Manual,
        AT,
        "run-1",
    );
    assert_eq!(status_of(&run, "trigger", ""), Some(NodeStatus::Done));
    assert_eq!(
        next_actions(&run),
        vec![start_session("fetch", "", 1, "Fetch for /work/repo", None)]
    );
    run = started_on(&run, "fetch", "", Some("s-1"));
    assert_eq!(next_actions(&run), vec![]);
    run = finished(&run, "fetch", json!({}), "", AT);
    assert_eq!(next_actions(&run), post_report());
    assert_eq!(run.status, RunStatus::Running);
    run = reported(&run, 2_000);
    assert_eq!(run.status, RunStatus::Done);
    assert_eq!(run.report_posted_at, Some(2_000));
    assert_eq!(run.ended_at, Some(2_000));
    assert_eq!(next_actions(&run), vec![]);
}

#[test]
fn schedule_trigger_exposes_started_at() {
    let run = start_run(
        &def(
            vec![
                n(json!({"id": "trigger", "type": "schedule",
                    "schedule": {"scheduleKind": "hourly", "time": "00:00", "minute": 0, "dayOfWeek": 1}})),
                report(),
            ],
            vec![e("trigger", "report")],
        ),
        RunTrigger::Schedule,
        5,
        "r",
    );
    assert_eq!(
        node_of(&run, "trigger", "").output,
        Some(out(json!({"startedAt": 5})))
    );
    assert_eq!(next_actions(&run), post_report());
}

#[test]
fn appends_the_output_instruction_to_the_prompt_only_when_fields_are_declared() {
    let run = begin(&def(
        vec![
            trigger(),
            session("a", json!({"output": {"x": {"type": "string"}}})),
            report(),
        ],
        vec![e("trigger", "a"), e("a", "report")],
    ));
    let actions = next_actions(&run);
    assert!(prompt_of(&actions[0]).starts_with("Do a\n\nWhen you are finished"));
    let bare = begin(&def(
        vec![trigger(), plain("a"), report()],
        vec![e("trigger", "a"), e("a", "report")],
    ));
    assert_eq!(prompt_of(&next_actions(&bare)[0]), "Do a");
}

fn cmd_flow(command: &str) -> Workflow {
    def(
        vec![
            trigger(),
            session("a", json!({"output": {"name": {"type": "string"}}})),
            n(json!({"id": "cmd", "type": "command", "project": PROJECT, "command": command})),
            report(),
        ],
        vec![e("trigger", "a"), e("a", "cmd"), e("cmd", "report")],
    )
}

#[test]
fn renders_commands_with_shell_quoting_and_fails_unsafe_command_templates() {
    let ok = begin(&cmd_flow("echo {{a.name}}"));
    let next = complete(&ok, "a", json!({"name": "it's"}));
    assert_eq!(
        next_actions(&next),
        vec![run_command("cmd", "echo 'it'\\''s'")]
    );

    let unsafe_run = begin(&cmd_flow("echo \"{{a.name}}\""));
    let after = complete(&unsafe_run, "a", json!({"name": "x"}));
    assert_eq!(status_of(&after, "cmd", ""), Some(NodeStatus::Failed));
    assert!(node_of(&after, "cmd", "")
        .error
        .as_deref()
        .unwrap()
        .contains("inside double quotes"));
    assert_eq!(next_actions(&after), post_report());
}

#[test]
fn fails_a_command_whose_rendered_value_would_be_read_as_an_option() {
    let cmd = |command: &str| {
        begin(&def(
            vec![
                trigger(),
                session("a", json!({"output": {"host": {"type": "string"}}})),
                n(json!({"id": "cmd", "type": "command", "project": PROJECT, "command": command})),
                report(),
            ],
            vec![e("trigger", "a"), e("a", "cmd"), e("cmd", "report")],
        ))
    };
    let bad = complete(
        &cmd("ssh {{a.host}}"),
        "a",
        json!({"host": "-oProxyCommand=x"}),
    );
    assert_eq!(status_of(&bad, "cmd", ""), Some(NodeStatus::Failed));
    assert!(node_of(&bad, "cmd", "")
        .error
        .as_deref()
        .unwrap()
        .contains("{{a.host}}"));
    assert_eq!(next_actions(&bad), post_report());

    let guarded = complete(&cmd("git log -- {{a.host}}"), "a", json!({"host": "-x"}));
    assert_eq!(
        next_actions(&guarded),
        vec![run_command("cmd", "git log -- '-x'")]
    );
    let plain_run = complete(&cmd("ssh {{a.host}}"), "a", json!({"host": "example.com"}));
    assert_eq!(
        next_actions(&plain_run),
        vec![run_command("cmd", "ssh 'example.com'")]
    );
}

#[test]
fn for_each_fans_out_3_items_with_max_parallel_2_2_then_1() {
    let mut run = begin(&default_fan_out());
    run = complete(&run, "fetch", json!({"tickets": tickets_value()}));
    assert_eq!(run.items.get("each"), Some(&tickets()));
    assert_eq!(status_of(&run, "each", ""), Some(NodeStatus::Running));

    let first = next_actions(&run);
    assert_eq!(
        session_targets(&first),
        pairs(&[("dev", "each#0"), ("dev", "each#1")])
    );
    assert!(prompt_of(&first[0]).contains("Implement A"));
    assert!(prompt_of(&first[1]).contains("Implement B"));

    run = started_on(&run, "dev", "each#0", None);
    run = started_on(&run, "dev", "each#1", None);
    assert_eq!(next_actions(&run), vec![]);

    run = finished(&run, "dev", json!({"pr_url": "u0"}), "each#0", AT);
    let second = next_actions(&run);
    assert_eq!(second.len(), 1);
    assert_eq!(session_targets(&second), pairs(&[("dev", "each#2")]));

    run = started_on(&run, "dev", "each#2", None);
    run = finished(&run, "dev", json!({"pr_url": "u1"}), "each#1", AT);
    run = finished(&run, "dev", json!({"pr_url": "u2"}), "each#2", AT);
    assert_eq!(status_of(&run, "each", ""), Some(NodeStatus::Done));
    assert_eq!(
        node_of(&run, "each", "").output,
        Some(out(json!({
            "results": [{"pr_url": "u0"}, {"pr_url": "u1"}, {"pr_url": "u2"}]
        })))
    );
    assert_eq!(next_actions(&run), post_report());
}

#[test]
fn for_each_empty_list_completes_with_no_branches() {
    let mut run = begin(&default_fan_out());
    run = complete(&run, "fetch", json!({"tickets": []}));
    assert_eq!(status_of(&run, "each", ""), Some(NodeStatus::Done));
    assert_eq!(
        node_of(&run, "each", "").output,
        Some(out(json!({"results": []})))
    );
    assert!(run.nodes.iter().all(|x| x.branch.is_empty()));
    assert_eq!(next_actions(&run), post_report());
    run = reported(&run, 3);
    assert_eq!(run.status, RunStatus::Done);
}

#[test]
fn for_each_on_non_list_fails_with_expected_a_list() {
    let mut run = begin(&default_fan_out());
    run = complete(&run, "fetch", json!({"tickets": "nope"}));
    assert_eq!(status_of(&run, "each", ""), Some(NodeStatus::Failed));
    assert_eq!(
        node_of(&run, "each", "").error.as_deref(),
        Some("Expected a list for {{fetch.tickets}}")
    );
    assert_eq!(next_actions(&run), post_report());
    run = reported(&run, 3);
    assert_eq!(run.status, RunStatus::DoneWithFailures);
}

#[test]
fn fails_a_nested_for_each_and_ignores_edges_that_cross_a_for_each_boundary() {
    let nested = fan_out(
        2,
        Some(vec![
            session("dev", json!({"parent": "each"})),
            n(json!({"id": "inner", "type": "forEach", "parent": "each", "in": "{{item.sub}}"})),
        ]),
        vec![
            e("dev", "inner"),
            // Crossing edge: ignored, so dev still starts without waiting for fetch's edge.
            e("fetch", "dev"),
        ],
    );
    let mut run = begin(&nested);
    run = complete(&run, "fetch", json!({"tickets": [{"sub": [1]}]}));
    run = complete_on(&run, "dev", json!({}), "each#0");
    assert_eq!(status_of(&run, "inner", "each#0"), Some(NodeStatus::Failed));
    assert_eq!(
        node_of(&run, "inner", "each#0").error.as_deref(),
        Some("Nested For each is not supported")
    );
    assert_eq!(status_of(&run, "each", ""), Some(NodeStatus::Done));
}

#[test]
fn loop_asks_mono_and_resets_dev_review_to_round_2_with_reuse_session_id() {
    let mut run = begin(&mono_loop());
    run = started_on(&run, "dev", "", Some("s-dev"));
    run = finished(&run, "dev", json!({"pr_url": "pr1"}), "", AT);
    run = started(&run, "review");
    run = finished(
        &run,
        "review",
        json!({"verdict": "changes", "issues": ["x"]}),
        "",
        AT,
    );
    // review has finished but is not settled until the Mono decides.
    assert_eq!(status_of(&run, "review", ""), Some(NodeStatus::Done));
    let asks = next_actions(&run);
    assert_eq!(asks.len(), 1);
    match &asks[0] {
        EngineAction::AskMono {
            node_id,
            purpose,
            answers,
            context,
            ..
        } => {
            assert_eq!(node_id, "review");
            assert_eq!(*purpose, AskPurpose::Loop);
            assert_eq!(answers, &vec!["loop".to_string(), "exit".to_string()]);
            assert_eq!(
                context.get("review"),
                Some(&jv(json!({"verdict": "changes", "issues": ["x"]})))
            );
            assert_eq!(context.get("round"), Some(&Json::Number(1.0)));
        }
        other => panic!("expected askMono, got {other:?}"),
    }
    // The ask is repeated until it is acknowledged, then not again.
    run = started(&run, "review");
    assert_eq!(next_actions(&run), vec![]);

    run = decided_on(&run, "review", "loop", "", Some("issues remain"));
    assert_eq!(status_of(&run, "dev", ""), Some(NodeStatus::Pending));
    assert_eq!(status_of(&run, "review", ""), Some(NodeStatus::Pending));
    assert_eq!(node_of(&run, "dev", "").round, 2);
    assert_eq!(node_of(&run, "review", "").round, 2);
    // kept until it reruns
    assert_eq!(
        node_of(&run, "dev", "").output,
        Some(out(json!({"pr_url": "pr1"})))
    );
    let again = next_actions(&run);
    match &again[0] {
        EngineAction::StartSession {
            node_id,
            round,
            prompt,
            reuse_session_id,
            ..
        } => {
            assert_eq!(node_id, "dev");
            assert_eq!(*round, 2);
            assert_eq!(reuse_session_id.as_deref(), Some("s-dev"));
            assert!(prompt.contains("Implement round 2. Fix: [\n  \"x\"\n]"));
        }
        other => panic!("expected startSession, got {other:?}"),
    }
}

#[test]
fn records_the_finished_round_before_a_loop_resets_a_node() {
    let mut run = begin(&mono_loop());
    run = started_on(&run, "dev", "", Some("s-dev-1"));
    run = finished(&run, "dev", json!({"pr_url": "u1"}), "", AT);
    run = started_on(&run, "review", "", Some("s-rev-1"));
    run = finished(&run, "review", json!({"verdict": "changes"}), "", AT);
    run = decided_on(&run, "review", "loop", "", Some("needs tests"));
    let dev_history = node_of(&run, "dev", "").history.clone().unwrap();
    assert_eq!(dev_history.len(), 1);
    assert_eq!(dev_history[0].round, 1);
    assert_eq!(dev_history[0].status, NodeStatus::Done);
    assert_eq!(dev_history[0].session_id.as_deref(), Some("s-dev-1"));
    // Keeping each round's output would multiply the size of a run, and nothing shows it.
    assert!(serde_json::to_value(&dev_history[0])
        .unwrap()
        .get("output")
        .is_none());
    let review_history = node_of(&run, "review", "").history.clone().unwrap();
    assert_eq!(review_history.len(), 1);
    assert_eq!(review_history[0].session_id.as_deref(), Some("s-rev-1"));
    assert_eq!(
        review_history[0].decision,
        Some(Decision {
            value: "loop".into(),
            reason: Some("needs tests".into())
        })
    );
    assert_eq!(node_of(&run, "review", "").round, 2);
    // The current round is never part of the history.
    assert_eq!(node_of(&run, "review", "").decision, None);
}

#[test]
fn exit_decision_stays_on_the_node_not_in_history() {
    let mut run = begin(&mono_loop());
    run = complete(&run, "dev", json!({"pr_url": "p1"}));
    run = complete(&run, "review", json!({"verdict": "no"}));
    run = decided(&run, "review", "loop");
    run = complete(&run, "dev", json!({"pr_url": "p2"}));
    run = complete(&run, "review", json!({"verdict": "ok"}));
    run = decided(&run, "review", "exit");
    assert_eq!(
        node_of(&run, "review", "")
            .decision
            .as_ref()
            .map(|d| d.value.as_str()),
        Some("exit")
    );
    let history = node_of(&run, "review", "").history.clone().unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].round, 1);
}

#[test]
fn loop_does_not_reuse_the_session_when_reuse_session_is_false() {
    let mut run = begin(&dev_review("mono", 3, Some(false)));
    run = complete(&run, "dev", json!({"pr_url": "p"}));
    run = complete(&run, "review", json!({"verdict": "no"}));
    run = decided(&run, "review", "loop");
    match &next_actions(&run)[0] {
        EngineAction::StartSession {
            node_id,
            round,
            reuse_session_id,
            ..
        } => {
            assert_eq!(node_id, "dev");
            assert_eq!(*round, 2);
            assert_eq!(*reuse_session_id, None);
        }
        other => panic!("expected startSession, got {other:?}"),
    }
}

#[test]
fn loop_stops_at_max_rounds_with_reason_max_rounds_reached() {
    let mut run = begin(&dev_review("mono", 2, None));
    run = complete(&run, "dev", json!({"pr_url": "p1"}));
    run = complete(&run, "review", json!({"verdict": "no"}));
    run = decided(&run, "review", "loop");
    run = complete(&run, "dev", json!({"pr_url": "p2"}));
    run = complete(&run, "review", json!({"verdict": "no"}));
    assert_eq!(
        node_of(&run, "review", "").decision,
        Some(Decision {
            value: "exit".into(),
            reason: Some("max rounds reached".into())
        })
    );
    // No question is put to the Mono; the run moves on to the report.
    assert_eq!(next_actions(&run), post_report());
}

#[test]
fn loop_exits_when_the_mono_answers_exit() {
    let mut run = begin(&mono_loop());
    run = complete(&run, "dev", json!({"pr_url": "p1"}));
    run = complete(&run, "review", json!({"verdict": "ok"}));
    run = started(&run, "review");
    run = decided_on(&run, "review", "exit", "", Some("looks good"));
    assert_eq!(status_of(&run, "review", ""), Some(NodeStatus::Done));
    assert_eq!(
        node_of(&run, "review", "").decision,
        Some(Decision {
            value: "exit".into(),
            reason: Some("looks good".into())
        })
    );
    assert_eq!(next_actions(&run), post_report());
}

#[test]
fn condition_loops_evaluate_without_asking_the_mono() {
    let mut run = begin(&dev_review("{{review.verdict}} != \"approve\"", 3, None));
    run = complete(&run, "dev", json!({"pr_url": "p1"}));
    run = complete(&run, "review", json!({"verdict": "changes"}));
    assert_eq!(node_of(&run, "dev", "").round, 2);
    let actions = next_actions(&run);
    assert_eq!(actions.len(), 1);
    assert_eq!(session_targets(&actions), pairs(&[("dev", "")]));
    run = complete(&run, "dev", json!({"pr_url": "p2"}));
    run = complete(&run, "review", json!({"verdict": "approve"}));
    assert_eq!(
        node_of(&run, "review", "").decision,
        Some(Decision {
            value: "exit".into(),
            reason: None
        })
    );
    assert_eq!(next_actions(&run), post_report());
}

#[test]
fn condition_loops_record_the_loop_decision_in_history() {
    let mut run = begin(&dev_review("{{review.verdict}} != \"approve\"", 3, None));
    run = complete(&run, "dev", json!({"pr_url": "p1"}));
    run = complete(&run, "review", json!({"verdict": "changes"}));
    let history = node_of(&run, "review", "").history.clone().unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].round, 1);
    assert_eq!(history[0].status, NodeStatus::Done);
    assert_eq!(
        history[0].decision,
        Some(Decision {
            value: "loop".into(),
            reason: None
        })
    );
    assert_eq!(node_of(&run, "review", "").decision, None);
}

#[test]
fn a_round_record_leaves_out_what_the_node_did_in_an_earlier_round() {
    let w = def(
        vec![
            trigger(),
            plain("dev"),
            n(json!({"id": "gate", "type": "if", "condition": "{{dev.ok}} == true"})),
            plain("fixA"),
            plain("fixB"),
            plain("review"),
            report(),
        ],
        vec![
            e("trigger", "dev"),
            e("dev", "gate"),
            eh("gate", "fixA", "true"),
            eh("gate", "fixB", "false"),
            e("fixA", "review"),
            e("fixB", "review"),
            e("review", "report"),
            lp("review", "dev", 3, "mono"),
        ],
    );
    let mut run = complete(&begin(&w), "dev", json!({"ok": true}));
    run = started_on(&run, "fixA", "", Some("s-a"));
    run = finished(&run, "fixA", json!({"fixed": "a"}), "", AT);
    run = complete(&run, "review", json!({}));
    run = decided(&run, "review", "loop");
    run = complete(&run, "dev", json!({"ok": false}));
    run = complete(&run, "fixB", json!({"fixed": "b"}));
    run = complete(&run, "review", json!({}));
    run = decided(&run, "review", "loop");
    let a = node_of(&run, "fixA", "").history.clone().unwrap();
    assert_eq!((a[0].round, a[0].status), (1, NodeStatus::Done));
    assert_eq!(a[0].session_id.as_deref(), Some("s-a"));
    assert_eq!((a[1].round, a[1].status), (2, NodeStatus::Skipped));
    assert_eq!(a[1].session_id, None);
    let b = node_of(&run, "fixB", "").history.clone().unwrap();
    assert_eq!((b[0].round, b[0].status), (1, NodeStatus::Skipped));
    assert_eq!((b[1].round, b[1].status), (2, NodeStatus::Done));
    for record in a.iter().chain(b.iter()) {
        assert!(serde_json::to_value(record)
            .unwrap()
            .get("output")
            .is_none());
    }
}

#[test]
fn loops_inside_a_for_each_keep_their_rounds_per_branch() {
    let w = fan_out(
        2,
        Some(vec![
            session(
                "dev",
                json!({
                    "parent": "each", "prompt": "Round {{run.round}} of {{item.title}}",
                    "output": {"pr_url": {"type": "string"}},
                }),
            ),
            session(
                "review",
                json!({"parent": "each", "output": {"verdict": {"type": "string"}}}),
            ),
        ]),
        vec![
            e("dev", "review"),
            lp("review", "dev", 2, "{{review.verdict}} != \"approve\""),
        ],
    );
    let mut run = complete(
        &begin(&w),
        "fetch",
        json!({"tickets": [{"title": "A"}, {"title": "B"}]}),
    );
    run = complete_on(&run, "dev", json!({"pr_url": "a1"}), "each#0");
    run = complete_on(&run, "review", json!({"verdict": "changes"}), "each#0");
    run = complete_on(&run, "dev", json!({"pr_url": "b1"}), "each#1");
    run = complete_on(&run, "review", json!({"verdict": "approve"}), "each#1");
    assert_eq!(node_of(&run, "dev", "each#0").round, 2);
    assert_eq!(node_of(&run, "dev", "each#1").round, 1);
    let actions = next_actions(&run);
    assert_eq!(actions.len(), 1);
    match &actions[0] {
        EngineAction::StartSession {
            node_id,
            branch,
            round,
            prompt,
            ..
        } => {
            assert_eq!(
                (node_id.as_str(), branch.as_str(), *round),
                ("dev", "each#0", 2)
            );
            assert!(prompt.starts_with("Round 2 of A"));
        }
        other => panic!("expected startSession, got {other:?}"),
    }
    run = complete_on(&run, "dev", json!({"pr_url": "a2"}), "each#0");
    run = complete_on(&run, "review", json!({"verdict": "changes"}), "each#0");
    assert_eq!(
        node_of(&run, "review", "each#0")
            .decision
            .as_ref()
            .and_then(|d| d.reason.as_deref()),
        Some("max rounds reached")
    );
    assert_eq!(status_of(&run, "each", ""), Some(NodeStatus::Done));
}

#[test]
fn if_inside_a_loop_body_runs_the_other_side_in_round_2() {
    let w = def(
        vec![
            trigger(),
            plain("dev"),
            n(json!({"id": "gate", "type": "if", "condition": "{{dev.ok}} == true"})),
            plain("review"),
            plain("escalate"),
            report(),
        ],
        vec![
            e("trigger", "dev"),
            e("dev", "gate"),
            eh("gate", "review", "true"),
            eh("gate", "escalate", "false"),
            e("review", "report"),
            e("escalate", "report"),
            lp("review", "dev", 3, "mono"),
        ],
    );
    let mut run = complete(&begin(&w), "dev", json!({"ok": true}));
    assert_eq!(status_of(&run, "escalate", ""), Some(NodeStatus::Skipped));
    run = complete(&run, "review", json!({}));
    run = decided(&run, "review", "loop");
    assert_eq!(status_of(&run, "escalate", ""), Some(NodeStatus::Pending));
    run = complete(&run, "dev", json!({"ok": false}));
    assert_eq!(status_of(&run, "review", ""), Some(NodeStatus::Skipped));
    assert_eq!(
        session_node_ids(&next_actions(&run)),
        vec!["escalate".to_string()]
    );
    run = complete(&run, "escalate", json!({}));
    assert_eq!(next_actions(&run), post_report());
    assert_eq!(status_of(&run, "escalate", ""), Some(NodeStatus::Done));
}

#[test]
fn does_not_mutate_the_input_run_after_a_loop_reset_or_a_for_each_expansion() {
    let mut run = begin(&mono_loop());
    run = complete(&run, "dev", json!({"pr_url": "p"}));
    run = complete(&run, "review", json!({"verdict": "no"}));
    let before = serde_json::to_string(&run).unwrap();
    let after = decided(&run, "review", "loop");
    assert_eq!(serde_json::to_string(&run).unwrap(), before);
    assert_ne!(after, run);

    let mut fan = begin(&default_fan_out());
    fan = started(&fan, "fetch");
    let snap = serde_json::to_string(&fan).unwrap();
    let expanded = finished(&fan, "fetch", json!({"tickets": tickets_value()}), "", AT);
    assert_eq!(serde_json::to_string(&fan).unwrap(), snap);
    assert!(expanded.nodes.len() > fan.nodes.len());
}

#[test]
fn snapshots_the_definition_at_start_run() {
    let mut w = mono_loop();
    let run = begin(&w);
    match &mut w.nodes[1].kind {
        NodeKind::Session { prompt, .. } => *prompt = "changed".into(),
        _ => unreachable!(),
    }
    w.nodes.pop();
    assert_ne!(run.definition, w);
    assert_eq!(run.definition.nodes.len(), 4);
    match &run.definition.nodes[1].kind {
        NodeKind::Session { prompt, .. } => assert!(prompt.contains("Implement round")),
        _ => unreachable!(),
    }
    assert_eq!(session_node_ids(&next_actions(&run))[0], "dev");
}

mod failed_branches_do_not_loop {
    use super::*;

    /// The probe: lint fails while review runs; merge was skipped by the failure.
    fn probe() -> Workflow {
        def(
            vec![
                trigger(),
                plain("dev"),
                plain("lint"),
                plain("review"),
                plain("merge"),
                report(),
            ],
            vec![
                e("trigger", "dev"),
                e("trigger", "lint"),
                e("dev", "review"),
                e("review", "merge"),
                e("lint", "merge"),
                e("merge", "report"),
                lp("review", "dev", 3, "mono"),
            ],
        )
    }

    #[test]
    fn exits_instead_of_asking_when_the_branch_already_failed() {
        let mut run = begin(&probe());
        run = complete(&run, "dev", json!({}));
        run = started(&run, "lint");
        run = started(&run, "review");
        run = failed(&run, "lint", "boom", "");
        assert_eq!(status_of(&run, "merge", ""), Some(NodeStatus::Skipped));
        run = finished(&run, "review", json!({}), "", AT);
        assert_eq!(
            node_of(&run, "review", "").decision,
            Some(Decision {
                value: "exit".into(),
                reason: Some("branch failed".into())
            })
        );
        assert_eq!(status_of(&run, "merge", ""), Some(NodeStatus::Skipped));
        assert_eq!(node_of(&run, "dev", "").round, 1);
        assert_eq!(next_actions(&run), post_report());
    }

    #[test]
    fn turns_a_loop_answer_that_arrives_after_a_failure_into_an_exit() {
        let mut run = begin(&probe());
        run = complete(&run, "dev", json!({}));
        run = started(&run, "lint");
        run = complete(&run, "review", json!({}));
        let asks = next_actions(&run);
        assert!(matches!(
            asks.as_slice(),
            [EngineAction::AskMono {
                purpose: AskPurpose::Loop,
                ..
            }]
        ));
        run = started(&run, "review"); // question in flight
        run = failed(&run, "lint", "boom", "");
        run = decided(&run, "review", "loop");
        assert_eq!(status_of(&run, "review", ""), Some(NodeStatus::Done));
        assert_eq!(
            node_of(&run, "review", "").decision,
            Some(Decision {
                value: "exit".into(),
                reason: Some("branch failed".into())
            })
        );
        assert_eq!(status_of(&run, "dev", ""), Some(NodeStatus::Done));
        assert_eq!(status_of(&run, "merge", ""), Some(NodeStatus::Skipped));
        assert_eq!(next_actions(&run), post_report());
    }

    #[test]
    fn a_failed_for_each_branch_does_not_stop_another_branch_from_looping() {
        let w = fan_out(
            2,
            Some(vec![
                session("dev", json!({"parent": "each"})),
                session("review", json!({"parent": "each"})),
            ]),
            vec![e("dev", "review"), lp("review", "dev", 3, "mono")],
        );
        let mut run = complete(&begin(&w), "fetch", json!({"tickets": [1, 2]}));
        run = started_on(&run, "dev", "each#0", None);
        run = failed(&run, "dev", "boom", "each#0");
        run = complete_on(&run, "dev", json!({}), "each#1");
        run = complete_on(&run, "review", json!({}), "each#1");
        let asks = next_actions(&run);
        assert!(matches!(
            asks.as_slice(),
            [EngineAction::AskMono { purpose: AskPurpose::Loop, branch, .. }] if branch == "each#1"
        ));
        run = started_on(&run, "review", "each#1", None);
        run = decided_on(&run, "review", "loop", "each#1", None);
        assert_eq!(node_of(&run, "dev", "each#1").round, 2);
        assert_eq!(status_of(&run, "dev", "each#1"), Some(NodeStatus::Pending));
        assert_eq!(
            status_of(&run, "review", "each#0"),
            Some(NodeStatus::Skipped)
        );
    }
}

mod a_loop_around_a_for_each_keeps_the_earlier_rounds {
    use super::*;

    /// trigger -> fetch -> each(dev) -> review, review loops back to each, review -> report.
    fn each_loop(max_rounds: u32, max_parallel: u32) -> Workflow {
        def(
            vec![
                trigger(),
                session("fetch", json!({"output": {"tickets": {"type": "list"}}})),
                n(
                    json!({"id": "each", "type": "forEach", "in": "{{fetch.tickets}}",
                    "as": "item", "maxParallel": max_parallel}),
                ),
                session(
                    "dev",
                    json!({"parent": "each", "prompt": "Implement {{item.title}}",
                        "output": {"pr_url": {"type": "string"}}}),
                ),
                session("review", json!({"output": {"verdict": {"type": "string"}}})),
                report(),
            ],
            vec![
                e("trigger", "fetch"),
                e("fetch", "each"),
                e("each", "review"),
                e("review", "report"),
                lp("review", "each", max_rounds, "mono"),
            ],
        )
    }

    /// Round 1 where `each#0` fails and `each#1` passes, then Mono sends the loop back.
    fn after_round_one() -> WorkflowRun {
        let mut run = complete(
            &begin(&each_loop(3, 2)),
            "fetch",
            json!({"tickets": [{"title": "A"}, {"title": "B"}]}),
        );
        run = started_on(&run, "dev", "each#0", Some("s-0"));
        run = failed(&run, "dev", "boom", "each#0");
        run = started_on(&run, "dev", "each#1", Some("s-1"));
        run = finished(&run, "dev", json!({"pr_url": "u1"}), "each#1", AT);
        run = complete(&run, "review", json!({"verdict": "changes"}));
        run = started(&run, "review"); // the loop question went out
        decided(&run, "review", "loop")
    }

    #[test]
    fn keeps_no_output_on_the_for_each_s_own_round_record() {
        let run = after_round_one();
        let history = node_of(&run, "each", "").history.clone().unwrap();
        assert_eq!((history[0].round, history[0].status), (1, NodeStatus::Done));
        assert!(serde_json::to_value(&history[0])
            .unwrap()
            .get("output")
            .is_none());
    }

    #[test]
    fn archives_the_branches_of_the_finished_round_instead_of_deleting_them() {
        let run = after_round_one();
        assert_eq!(
            status_of(&run, "dev", "each#0@r1"),
            Some(NodeStatus::Failed)
        );
        assert_eq!(
            node_of(&run, "dev", "each#0@r1").session_id.as_deref(),
            Some("s-0")
        );
        assert_eq!(
            node_of(&run, "dev", "each#0@r1").error.as_deref(),
            Some("boom")
        );
        assert_eq!(status_of(&run, "dev", "each#1@r1"), Some(NodeStatus::Done));
        assert_eq!(
            node_of(&run, "dev", "each#1@r1").output,
            Some(out(json!({"pr_url": "u1"})))
        );
        // The live branches start over, and the item list of round 1 is still known.
        assert_eq!(status_of(&run, "dev", "each#0"), Some(NodeStatus::Pending));
        assert_eq!(status_of(&run, "dev", "each#1"), Some(NodeStatus::Pending));
        assert_eq!(
            run.items.get("each@r1"),
            Some(&vec![jv(json!({"title": "A"})), jv(json!({"title": "B"}))])
        );
    }

    #[test]
    fn never_emits_or_admits_an_archived_branch() {
        let run = after_round_one();
        let actions = next_actions(&run);
        assert!(!actions.is_empty());
        assert!(no_archived_branch(&actions));
        let targets: Vec<(String, String, u32)> = actions
            .iter()
            .filter_map(|a| match a {
                EngineAction::StartSession {
                    node_id,
                    branch,
                    round,
                    ..
                } => Some((node_id.clone(), branch.clone(), *round)),
                _ => None,
            })
            .collect();
        assert_eq!(
            targets,
            vec![
                ("dev".to_string(), "each#0".to_string(), 1),
                ("dev".to_string(), "each#1".to_string(), 1)
            ]
        );
        // Events aimed at an archived branch change nothing.
        let event = EngineEvent::NodeStarted {
            node_id: "dev".into(),
            branch: "each#0@r1".into(),
            session_id: None,
            at: AT,
        };
        assert_eq!(apply_event(&run, &event), run);
    }

    #[test]
    fn ends_done_with_failures_when_round_1_failed_and_round_2_passed() {
        let mut run = after_round_one();
        run = started_on(&run, "dev", "each#0", Some("s-0b"));
        run = finished(&run, "dev", json!({"pr_url": "u0"}), "each#0", AT);
        run = complete_on(&run, "dev", json!({"pr_url": "u1b"}), "each#1");
        assert_eq!(status_of(&run, "each", ""), Some(NodeStatus::Done));
        run = complete(&run, "review", json!({"verdict": "approve"}));
        run = started(&run, "review");
        run = decided(&run, "review", "exit");
        assert_eq!(next_actions(&run), post_report());
        run = reported(&run, AT);
        assert_eq!(run.status, RunStatus::DoneWithFailures);
        assert_eq!(
            status_of(&run, "dev", "each#0@r1"),
            Some(NodeStatus::Failed)
        );
    }

    #[test]
    fn keeps_every_earlier_round_when_the_loop_runs_again() {
        let mut run = after_round_one();
        run = complete_on(&run, "dev", json!({"pr_url": "a"}), "each#0");
        run = complete_on(&run, "dev", json!({"pr_url": "b"}), "each#1");
        run = complete(&run, "review", json!({"verdict": "changes"}));
        run = started(&run, "review");
        run = decided(&run, "review", "loop");
        assert_eq!(node_of(&run, "dev", "each#0").round, 1);
        assert_eq!(node_of(&run, "review", "").round, 3);
        for key in ["each#0@r1", "each#1@r1", "each#0@r2", "each#1@r2"] {
            assert!(status_of(&run, "dev", key).is_some(), "{key}");
        }
        assert_eq!(
            status_of(&run, "dev", "each#0@r1"),
            Some(NodeStatus::Failed)
        );
        assert_eq!(status_of(&run, "dev", "each#0@r2"), Some(NodeStatus::Done));
        assert_eq!(run.items.get("each@r2").map(Vec::len), Some(2));
        assert!(no_archived_branch(&next_actions(&run)));
    }

    #[test]
    fn does_not_count_archived_branches_against_max_parallel() {
        let mut run = complete(
            &begin(&each_loop(3, 1)),
            "fetch",
            json!({"tickets": [1, 2]}),
        );
        run = complete_on(&run, "dev", json!({}), "each#0");
        run = complete_on(&run, "dev", json!({}), "each#1");
        run = complete(&run, "review", json!({}));
        run = started(&run, "review");
        run = decided(&run, "review", "loop");
        assert_eq!(
            session_targets(&next_actions(&run)),
            pairs(&[("dev", "each#0")])
        );
    }

    #[test]
    fn an_archived_failure_does_not_stop_the_outer_loop_from_running_another_round() {
        let run = after_round_one();
        assert_eq!(node_of(&run, "review", "").round, 2);
        assert_eq!(node_of(&run, "review", "").decision, None);
    }
}

#[test]
fn if_true_false_skips_the_other_side() {
    let w = def(
        vec![
            trigger(),
            session("a", json!({"output": {"ok": {"type": "boolean"}}})),
            n(json!({"id": "gate", "type": "if", "condition": "{{a.ok}} == true"})),
            plain("yes"),
            plain("no"),
            plain("after"),
            report(),
        ],
        vec![
            e("trigger", "a"),
            e("a", "gate"),
            eh("gate", "yes", "true"),
            eh("gate", "no", "false"),
            e("no", "after"),
            e("yes", "report"),
            e("after", "report"),
        ],
    );
    let mut t = complete(&begin(&w), "a", json!({"ok": true}));
    assert_eq!(status_of(&t, "gate", ""), Some(NodeStatus::Done));
    assert_eq!(
        node_of(&t, "gate", "").decision,
        Some(Decision {
            value: "true".into(),
            reason: None
        })
    );
    assert_eq!(status_of(&t, "no", ""), Some(NodeStatus::Skipped));
    assert_eq!(status_of(&t, "after", ""), Some(NodeStatus::Skipped)); // transitive
    assert_eq!(session_node_ids(&next_actions(&t)), vec!["yes".to_string()]);
    t = complete(&t, "yes", json!({}));
    assert_eq!(next_actions(&t), post_report());

    let f = complete(&begin(&w), "a", json!({"ok": false}));
    assert_eq!(
        node_of(&f, "gate", "").decision,
        Some(Decision {
            value: "false".into(),
            reason: None
        })
    );
    assert_eq!(status_of(&f, "yes", ""), Some(NodeStatus::Skipped));
    assert_eq!(session_node_ids(&next_actions(&f)), vec!["no".to_string()]);
}

#[test]
fn if_decided_by_the_mono_asks_true_false() {
    let w = def(
        vec![
            trigger(),
            n(json!({"id": "gate", "type": "if", "decide": "mono",
                "question": "Is {{inputs.project}} fine?"})),
            plain("yes"),
            report(),
        ],
        vec![
            e("trigger", "gate"),
            eh("gate", "yes", "true"),
            e("yes", "report"),
            eh("gate", "report", "false"),
        ],
    );
    let mut run = begin(&w);
    let asks = next_actions(&run);
    match asks.as_slice() {
        [EngineAction::AskMono {
            purpose,
            question,
            answers,
            ..
        }] => {
            assert_eq!(*purpose, AskPurpose::If);
            assert_eq!(question, "Is /work/repo fine?");
            assert_eq!(answers, &vec!["true".to_string(), "false".to_string()]);
        }
        other => panic!("expected one askMono, got {other:?}"),
    }
    run = started(&run, "gate");
    assert_eq!(next_actions(&run), vec![]);
    run = decided(&run, "gate", "false");
    assert_eq!(status_of(&run, "yes", ""), Some(NodeStatus::Skipped));
    assert_eq!(next_actions(&run), post_report());
}

#[test]
fn ask_mono_node_records_answer_and_reason() {
    let w = def(
        vec![
            trigger(),
            n(
                json!({"id": "pick", "type": "askMono", "question": "Which?", "answers": ["a", "b"]}),
            ),
            session("s", json!({"prompt": "Got {{pick.answer}}"})),
            report(),
        ],
        vec![e("trigger", "pick"), e("pick", "s"), e("s", "report")],
    );
    let mut run = begin(&w);
    match next_actions(&run).as_slice() {
        [EngineAction::AskMono {
            purpose,
            question,
            answers,
            ..
        }] => {
            assert_eq!(*purpose, AskPurpose::AskMono);
            assert_eq!(question, "Which?");
            assert_eq!(answers, &vec!["a".to_string(), "b".to_string()]);
        }
        other => panic!("expected one askMono, got {other:?}"),
    }
    run = started(&run, "pick");
    run = decided_on(&run, "pick", "b", "", Some("because"));
    assert_eq!(
        node_of(&run, "pick", "").output,
        Some(out(json!({"answer": "b", "reason": "because"})))
    );
    let actions = next_actions(&run);
    assert_eq!(actions.len(), 1);
    assert_eq!(prompt_of(&actions[0]), "Got b");
}

#[test]
fn failure_in_one_branch_does_not_stop_another_status_done_with_failures() {
    let w = fan_out(
        3,
        Some(vec![
            session(
                "dev",
                json!({"parent": "each", "output": {"pr_url": {"type": "string"}}}),
            ),
            session("review", json!({"parent": "each"})),
        ]),
        vec![e("dev", "review")],
    );
    let mut run = complete(&begin(&w), "fetch", json!({"tickets": [1, 2]}));
    run = started_on(&run, "dev", "each#0", None);
    run = started_on(&run, "dev", "each#1", None);
    run = failed(&run, "dev", "boom", "each#0");
    assert_eq!(
        status_of(&run, "review", "each#0"),
        Some(NodeStatus::Skipped)
    );
    assert_eq!(next_actions(&run), vec![]);
    run = finished(&run, "dev", json!({"pr_url": "u"}), "each#1", AT);
    assert_eq!(
        session_targets(&next_actions(&run)),
        pairs(&[("review", "each#1")])
    );
    run = complete_on(&run, "review", json!({}), "each#1");
    // A failed branch has no output: `null` in its slot.
    let mut expected = OrderedMap::new();
    expected.insert("results", Json::Array(vec![Json::Null, jv(json!({}))]));
    assert_eq!(node_of(&run, "each", "").output, Some(expected));
    run = reported(&run, 9);
    assert_eq!(run.status, RunStatus::DoneWithFailures);
}

#[test]
fn run_is_failed_when_no_session_or_command_succeeded() {
    let mut run = begin(&def(
        vec![trigger(), plain("a"), report()],
        vec![e("trigger", "a"), e("a", "report")],
    ));
    run = failed(&started(&run, "a"), "a", "nope", "");
    assert_eq!(next_actions(&run), post_report());
    run = reported(&run, 5);
    assert_eq!(run.status, RunStatus::Failed);
}

#[test]
fn a_run_without_a_report_node_ends_as_soon_as_nothing_is_left() {
    let mut run = begin(&def(vec![trigger(), plain("a")], vec![e("trigger", "a")]));
    run = complete_at(&run, "a", json!({}), "", 7);
    assert_eq!(run.status, RunStatus::Done);
    assert_eq!(run.ended_at, Some(7));
    assert_eq!(next_actions(&run), vec![]);
}

#[test]
fn mark_interrupted_keeps_done_nodes_and_resets_running_to_pending() {
    let mut run = begin(&default_fan_out());
    run = complete(&run, "fetch", json!({"tickets": tickets_value()}));
    run = started_on(&run, "dev", "each#0", Some("s-0"));
    run = started_on(&run, "dev", "each#1", Some("s-1"));
    run = finished(&run, "dev", json!({"pr_url": "u1"}), "each#1", AT);
    let hurt = mark_interrupted(&run);
    assert_eq!(hurt.status, RunStatus::Interrupted);
    assert_eq!(status_of(&hurt, "fetch", ""), Some(NodeStatus::Done));
    assert_eq!(status_of(&hurt, "dev", "each#1"), Some(NodeStatus::Done));
    assert_eq!(status_of(&hurt, "dev", "each#0"), Some(NodeStatus::Pending));
    assert_eq!(node_of(&hurt, "dev", "each#0").session_id, None);
    // the group keeps its expansion
    assert_eq!(status_of(&hurt, "each", ""), Some(NodeStatus::Running));
    assert_eq!(hurt.items.get("each"), Some(&tickets()));
    assert_eq!(next_actions(&hurt), vec![]);
    let event = EngineEvent::NodeFinished {
        node_id: "dev".into(),
        branch: "each#0".into(),
        output: OrderedMap::new(),
        at: AT,
    };
    assert_eq!(apply_event(&hurt, &event), hurt);
}

mod resuming_a_node_that_was_mid_session {
    use super::*;

    fn start_action(run: &WorkflowRun) -> (u32, Option<String>, String) {
        match next_actions(run)
            .into_iter()
            .find(|a| matches!(a, EngineAction::StartSession { .. }))
        {
            Some(EngineAction::StartSession {
                round,
                reuse_session_id,
                node_id,
                ..
            }) => (round, reuse_session_id, node_id),
            _ => panic!("no startSession"),
        }
    }

    #[test]
    fn reuses_the_loop_session_of_round_2_or_later() {
        let mut run = begin(&mono_loop());
        run = complete(
            &started_on(&run, "dev", "", Some("s-dev")),
            "dev",
            json!({"pr_url": "p1"}),
        );
        run = complete(&run, "review", json!({"verdict": "no"}));
        run = started(&run, "review");
        run = decided(&run, "review", "loop");
        run = started_on(&run, "dev", "", Some("s-dev-2")); // dev is running round 2
        let resumed = resume_run(&mark_interrupted(&run));
        assert_eq!(node_of(&resumed, "dev", "").session_id, None);
        assert_eq!(
            start_action(&resumed),
            (2, Some("s-dev-2".to_string()), "dev".to_string())
        );
        // Interrupted again before it restarted: the id is not lost.
        assert_eq!(
            start_action(&resume_run(&mark_interrupted(&resumed))).1,
            Some("s-dev-2".to_string())
        );
    }

    #[test]
    fn does_not_reuse_when_the_node_opted_out_of_reuse() {
        let mut run = begin(&dev_review("mono", 3, Some(false)));
        run = complete(
            &started_on(&run, "dev", "", Some("s-dev")),
            "dev",
            json!({}),
        );
        run = complete(&run, "review", json!({}));
        run = started(&run, "review");
        run = decided(&run, "review", "loop");
        run = started_on(&run, "dev", "", Some("s-dev-2"));
        assert_eq!(start_action(&resume_run(&mark_interrupted(&run))).1, None);
    }

    #[test]
    fn re_runs_a_round_1_session_fresh() {
        let mut run = begin(&mono_loop());
        run = started_on(&run, "dev", "", Some("s-dev"));
        let action = start_action(&resume_run(&mark_interrupted(&run)));
        assert_eq!(action.0, 1);
        assert_eq!(action.1, None);
    }
}

#[test]
fn resume_run_never_re_emits_a_done_node() {
    let mut run = begin(&default_fan_out());
    run = complete(&run, "fetch", json!({"tickets": tickets_value()}));
    run = started_on(&run, "dev", "each#0", None);
    run = started_on(&run, "dev", "each#1", None);
    run = finished(&run, "dev", json!({"pr_url": "u1"}), "each#1", AT);
    let resumed = resume_run(&mark_interrupted(&run));
    assert_eq!(resumed.status, RunStatus::Running);
    let actions = next_actions(&resumed);
    assert_eq!(
        session_targets(&actions),
        pairs(&[("dev", "each#0"), ("dev", "each#2")])
    );
    assert!(!session_node_ids(&actions).contains(&"fetch".to_string()));
}

#[test]
fn resume_run_after_a_loop_question_was_lost_asks_again_without_rerunning_the_node() {
    let mut run = begin(&mono_loop());
    run = complete(&run, "dev", json!({"pr_url": "p"}));
    run = complete(&run, "review", json!({"verdict": "no"}));
    run = started(&run, "review"); // question sent
    let resumed = resume_run(&mark_interrupted(&run));
    assert_eq!(status_of(&resumed, "review", ""), Some(NodeStatus::Done));
    assert!(matches!(
        next_actions(&resumed).as_slice(),
        [EngineAction::AskMono {
            purpose: AskPurpose::Loop,
            ..
        }]
    ));
}

#[test]
fn stopped_event_ends_run_as_stopped_and_emits_nothing() {
    let mut run = begin(&default_fan_out());
    run = started(&run, "fetch");
    run = apply_event(&run, &EngineEvent::Stopped { at: 50 });
    assert_eq!(run.status, RunStatus::Stopped);
    assert_eq!(run.ended_at, Some(50));
    assert_eq!(status_of(&run, "fetch", ""), Some(NodeStatus::Stopped));
    assert_eq!(status_of(&run, "each", ""), Some(NodeStatus::Skipped));
    assert_eq!(next_actions(&run), vec![]);
    let event = EngineEvent::NodeFinished {
        node_id: "fetch".into(),
        branch: "".into(),
        output: OrderedMap::new(),
        at: AT,
    };
    assert_eq!(apply_event(&run, &event), run);
}

fn single_session() -> Workflow {
    def(
        vec![trigger(), plain("a"), report()],
        vec![e("trigger", "a"), e("a", "report")],
    )
}

#[test]
fn waiting_approval_toggles_a_running_node() {
    let mut run = begin(&single_session());
    run = started(&run, "a");
    let wait = |run: &WorkflowRun, waiting: bool| {
        apply_event(
            run,
            &EngineEvent::WaitingApproval {
                node_id: "a".into(),
                branch: "".into(),
                waiting,
            },
        )
    };
    run = wait(&run, true);
    assert_eq!(status_of(&run, "a", ""), Some(NodeStatus::WaitingApproval));
    assert_eq!(next_actions(&run), vec![]);
    run = wait(&run, false);
    assert_eq!(status_of(&run, "a", ""), Some(NodeStatus::Running));
}

#[test]
fn events_are_immutable_updates() {
    let run = begin(&single_session());
    let snapshot = serde_json::to_string(&run).unwrap();
    let next = started_on(&run, "a", "", Some("s"));
    assert_eq!(serde_json::to_string(&run).unwrap(), snapshot);
    assert_ne!(next, run);
}

#[test]
fn run_scope_exposes_inputs_run_the_loop_variable_and_branch_outputs() {
    let mut run = start_run(&default_fan_out(), RunTrigger::Manual, 77, "run-x");
    run = complete(&run, "fetch", json!({"tickets": tickets_value()}));
    run = complete_on(&run, "dev", json!({"pr_url": "u0"}), "each#0");
    let scope = run_scope(&run, "each#0", Some("dev"));
    assert_eq!(
        scope.get("inputs"),
        Some(&jv(json!({"project": "/work/repo"})))
    );
    assert_eq!(
        scope.get("run"),
        Some(&jv(json!({"id": "run-x", "startedAt": 77, "round": 1})))
    );
    assert_eq!(scope.get("item"), Some(&jv(json!({"title": "A"}))));
    assert_eq!(
        scope.get("fetch"),
        Some(&jv(json!({"tickets": tickets_value()})))
    );
    assert_eq!(scope.get("dev"), Some(&jv(json!({"pr_url": "u0"}))));
    let other = run_scope(&run, "each#1", None);
    assert_eq!(other.get("item"), Some(&jv(json!({"title": "B"}))));
    assert_eq!(other.get("dev"), None);
    assert_eq!(run_scope(&run, "", None).get("item"), None);
}

#[test]
fn run_round_is_the_current_round_of_the_node_inside_a_loop() {
    let mut run = begin(&mono_loop());
    run = complete(&run, "dev", json!({"pr_url": "p"}));
    run = complete(&run, "review", json!({"verdict": "no"}));
    run = decided(&run, "review", "loop");
    let scope = run_scope(&run, "", Some("dev"));
    assert_eq!(
        scope.get("run").and_then(|r| r.get("round")),
        Some(&Json::Number(2.0))
    );
}

#[test]
fn does_not_run_nodes_the_trigger_cannot_reach() {
    let run = begin(&def(
        vec![trigger(), plain("a"), plain("orphan"), report()],
        vec![e("trigger", "a"), e("a", "report")],
    ));
    assert_eq!(status_of(&run, "orphan", ""), Some(NodeStatus::Skipped));
    assert_eq!(session_node_ids(&next_actions(&run)), vec!["a".to_string()]);
}

#[test]
fn ignores_events_for_unknown_nodes() {
    let run = begin(&def(
        vec![trigger(), report()],
        vec![e("trigger", "report")],
    ));
    let bogus = EngineEvent::NodeFinished {
        node_id: "ghost".into(),
        branch: "".into(),
        output: OrderedMap::new(),
        at: AT,
    };
    assert_eq!(apply_event(&run, &bogus), run);
}

// ---------------------------------------------------------------------------------------------
// loopedNodeIds
// ---------------------------------------------------------------------------------------------

fn sorted(ids: std::collections::HashSet<String>) -> Vec<String> {
    let mut v: Vec<String> = ids.into_iter().collect();
    v.sort();
    v
}

#[test]
fn looped_node_ids_names_every_node_from_the_loop_s_target_to_its_source_and_no_other() {
    let w = def(
        vec![
            trigger(),
            plain("prep"),
            plain("dev"),
            plain("lint"),
            plain("review"),
            plain("ship"),
            report(),
        ],
        vec![
            e("trigger", "prep"),
            e("prep", "dev"),
            e("dev", "lint"),
            e("lint", "review"),
            e("review", "ship"),
            e("ship", "report"),
            lp("review", "dev", 3, "mono"),
        ],
    );
    assert_eq!(sorted(looped_node_ids(&w)), vec!["dev", "lint", "review"]);
}

#[test]
fn looped_node_ids_is_empty_without_a_loop() {
    let w = def(
        vec![trigger(), plain("a"), report()],
        vec![e("trigger", "a"), e("a", "report")],
    );
    assert!(looped_node_ids(&w).is_empty());
}

#[test]
fn looped_node_ids_stays_within_one_for_each() {
    let w = def(
        vec![
            trigger(),
            session("fetch", json!({"output": {"tickets": {"type": "list"}}})),
            n(
                json!({"id": "each", "type": "forEach", "in": "{{fetch.tickets}}",
                "as": "item", "maxParallel": 2}),
            ),
            session("dev", json!({"parent": "each"})),
            session("review", json!({"parent": "each"})),
            report(),
        ],
        vec![
            e("trigger", "fetch"),
            e("fetch", "each"),
            e("dev", "review"),
            e("each", "report"),
            lp("review", "dev", 3, "mono"),
        ],
    );
    assert_eq!(sorted(looped_node_ids(&w)), vec!["dev", "review"]);
}

// ---------------------------------------------------------------------------------------------
// Rust-side checks beyond the React tests
// ---------------------------------------------------------------------------------------------

#[test]
fn is_asking_is_a_running_node_with_an_end_time() {
    let mut r = NodeRun::new("review", "");
    r.status = NodeStatus::Running;
    assert!(!is_asking(&r));
    r.ended_at = Some(5);
    assert!(is_asking(&r));
    r.status = NodeStatus::Done;
    assert!(!is_asking(&r));
}

#[test]
fn an_engine_run_is_written_in_the_shape_the_react_app_reads() {
    let mut run = begin(&mono_loop());
    run = started_on(&run, "dev", "", Some("s-dev"));
    run = finished(&run, "dev", json!({"pr_url": "p"}), "", 1_500);
    let value = serde_json::to_value(&run).unwrap();
    assert_eq!(value["status"], "running");
    assert_eq!(value["trigger"], "manual");
    assert_eq!(value["startedAt"], 1_000);
    assert_eq!(value["items"], json!({}));
    let dev = value["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|x| x["nodeId"] == "dev")
        .unwrap();
    assert_eq!(dev["sessionId"], "s-dev");
    assert_eq!(dev["output"], json!({"pr_url": "p"}));
    assert_eq!(dev["endedAt"], 1_500);
    let back: WorkflowRun = serde_json::from_value(value).unwrap();
    assert_eq!(back, run);
    // A run read from disk can go on where the engine left it.
    assert_eq!(next_actions(&back), next_actions(&run));
}

#[test]
fn rejects_answers_that_are_not_allowed() {
    // If node: only true or false.
    let gate = def(
        vec![
            trigger(),
            n(json!({"id": "gate", "type": "if", "decide": "mono"})),
            report(),
        ],
        vec![e("trigger", "gate"), e("gate", "report")],
    );
    let run = decided(&started(&begin(&gate), "gate"), "gate", "maybe");
    assert_eq!(
        node_of(&run, "gate", "").error.as_deref(),
        Some("Invalid decision \"maybe\" (expected true or false)")
    );
    // The default question names the node.
    let asks = next_actions(&begin(&gate));
    match asks.as_slice() {
        [EngineAction::AskMono { question, .. }] => assert_eq!(
            question,
            "Should the \"true\" side of \"gate\" be taken? Answer true or false."
        ),
        other => panic!("expected one askMono, got {other:?}"),
    }
    // Ask Mono node: only a listed answer.
    let pick = def(
        vec![
            trigger(),
            n(json!({"id": "pick", "type": "askMono", "question": "Which?", "answers": ["a"]})),
            report(),
        ],
        vec![e("trigger", "pick"), e("pick", "report")],
    );
    let run = decided(&begin(&pick), "pick", "z");
    assert_eq!(
        node_of(&run, "pick", "").error.as_deref(),
        Some("Mono answered \"z\", which is not an allowed answer")
    );
    // A loop answer: loop or exit.
    let mut run = begin(&mono_loop());
    run = complete(&run, "dev", json!({}));
    run = complete(&run, "review", json!({}));
    run = decided(&started(&run, "review"), "review", "again");
    assert_eq!(status_of(&run, "review", ""), Some(NodeStatus::Failed));
    assert_eq!(
        node_of(&run, "review", "").error.as_deref(),
        Some("Invalid loop decision \"again\" (expected loop or exit)")
    );
}

#[test]
fn a_condition_error_fails_the_node() {
    let w = def(
        vec![
            trigger(),
            n(json!({"id": "gate", "type": "if", "condition": "{{a}} >> 1"})),
            report(),
        ],
        vec![e("trigger", "gate"), e("gate", "report")],
    );
    let run = begin(&w);
    assert_eq!(status_of(&run, "gate", ""), Some(NodeStatus::Failed));
    assert!(node_of(&run, "gate", "")
        .error
        .as_deref()
        .unwrap()
        .starts_with("Cannot parse condition"));
}

#[test]
fn a_for_each_list_expression_may_have_white_space_but_nothing_else() {
    let list_for = |expression: &str| {
        let w = def(
            vec![
                trigger(),
                session("fetch", json!({"output": {"tickets": {"type": "list"}}})),
                n(json!({"id": "each", "type": "forEach", "in": expression})),
                session("dev", json!({"parent": "each"})),
                report(),
            ],
            vec![
                e("trigger", "fetch"),
                e("fetch", "each"),
                e("each", "report"),
            ],
        );
        let run = complete(&begin(&w), "fetch", json!({"tickets": [1]}));
        status_of(&run, "each", "")
    };
    assert_eq!(list_for("{{fetch.tickets}}"), Some(NodeStatus::Running));
    assert_eq!(
        list_for("  {{ fetch.tickets }}\n"),
        Some(NodeStatus::Running)
    );
    assert_eq!(list_for("x {{fetch.tickets}}"), Some(NodeStatus::Failed));
    assert_eq!(
        list_for("{{fetch.tickets}} {{fetch.tickets}}"),
        Some(NodeStatus::Failed)
    );
    assert_eq!(list_for("{{fetch tickets}}"), Some(NodeStatus::Failed));
    assert_eq!(list_for("{{}}"), Some(NodeStatus::Failed));
}

#[test]
fn events_that_do_not_fit_the_node_state_change_nothing() {
    let mut run = begin(&single_session());
    // Not started yet: waiting does nothing.
    let wait = EngineEvent::WaitingApproval {
        node_id: "a".into(),
        branch: "".into(),
        waiting: true,
    };
    assert_eq!(apply_event(&run, &wait), run);
    // A decision on a session node is ignored.
    assert_eq!(
        decided(&run, "a", "true"),
        run,
        "session nodes take no decision"
    );
    run = complete(&run, "a", json!({}));
    // A done node cannot fail or finish again.
    assert_eq!(failed(&run, "a", "late", ""), run);
    assert_eq!(finished(&run, "a", json!({"x": 1}), "", AT), run);
    // A finished run takes no event.
    let done = reported(&run, 9);
    assert_eq!(done.status, RunStatus::Done);
    assert_eq!(reported(&done, 10), done);
    assert_eq!(apply_event(&done, &EngineEvent::Stopped { at: 11 }), done);
}

#[test]
fn start_run_without_a_trigger_starts_from_the_nodes_nothing_leads_to() {
    let run = begin(&def(
        vec![plain("a"), plain("b"), report()],
        vec![e("a", "b"), e("b", "report")],
    ));
    assert_eq!(session_node_ids(&next_actions(&run)), vec!["a".to_string()]);
    assert_eq!(status_of(&run, "report", ""), Some(NodeStatus::Pending));
}

#[test]
fn an_interrupted_run_can_be_stopped_and_the_stop_marks_pending_nodes_skipped() {
    let mut run = begin(&single_session());
    run = started(&run, "a");
    let hurt = mark_interrupted(&run);
    let stopped = apply_event(&hurt, &EngineEvent::Stopped { at: 70 });
    assert_eq!(stopped.status, RunStatus::Stopped);
    assert_eq!(stopped.ended_at, Some(70));
    assert_eq!(status_of(&stopped, "a", ""), Some(NodeStatus::Skipped));
    assert_eq!(status_of(&stopped, "report", ""), Some(NodeStatus::Skipped));
}

#[test]
fn mark_interrupted_and_resume_leave_other_states_alone() {
    let run = begin(&single_session());
    assert_eq!(resume_run(&run), run);
    let stopped = apply_event(&run, &EngineEvent::Stopped { at: 3 });
    assert_eq!(mark_interrupted(&stopped), stopped);
    let hurt = mark_interrupted(&run);
    assert_eq!(hurt.status, RunStatus::Interrupted);
    let resumed = resume_run(&hurt);
    assert_eq!(resumed.status, RunStatus::Running);
    assert_eq!(resumed.ended_at, None);
}

#[test]
fn a_waiting_approval_node_goes_back_to_pending_when_interrupted() {
    let mut run = begin(&single_session());
    run = started_on(&run, "a", "", Some("s"));
    run = apply_event(
        &run,
        &EngineEvent::WaitingApproval {
            node_id: "a".into(),
            branch: "".into(),
            waiting: true,
        },
    );
    let hurt = mark_interrupted(&run);
    assert_eq!(status_of(&hurt, "a", ""), Some(NodeStatus::Pending));
    assert_eq!(node_of(&hurt, "a", "").session_id, None);
    assert_eq!(node_of(&hurt, "a", "").reuse_session_id, None);
}

#[test]
fn an_unknown_loop_edge_target_or_cross_parent_loop_is_ignored() {
    // A loop edge into a node that does not exist, and one that crosses a For each boundary.
    let w = def(
        vec![
            trigger(),
            plain("a"),
            n(json!({"id": "each", "type": "forEach", "in": "{{a.list}}"})),
            session("b", json!({"parent": "each"})),
            report(),
        ],
        vec![
            e("trigger", "a"),
            e("a", "each"),
            e("each", "report"),
            lp("a", "ghost", 3, "mono"),
            lp("b", "a", 3, "mono"),
        ],
    );
    assert!(looped_node_ids(&w).is_empty());
    let run = begin(&w);
    assert_eq!(session_node_ids(&next_actions(&run)), vec!["a".to_string()]);
}

// ---------------------------------------------------------------------------------------------
// switch
// ---------------------------------------------------------------------------------------------

/// trigger -> triage -> route (switch on `triage.kind`) -> fix | plan | other -> report.
fn switch_workflow(route: Value) -> Workflow {
    let mut route_node = json!({"id": "route", "type": "switch", "cases": ["bug", "feature"]});
    for (k, x) in route.as_object().unwrap() {
        route_node[k] = x.clone();
    }
    def(
        vec![
            trigger(),
            session("triage", json!({"output": {"kind": {"type": "string"}}})),
            n(route_node),
            plain("fix"),
            plain("plan"),
            plain("other"),
            report(),
        ],
        vec![
            e("trigger", "triage"),
            e("triage", "route"),
            eh("route", "fix", "bug"),
            eh("route", "plan", "feature"),
            eh("route", "other", "default"),
            e("fix", "report"),
            e("plan", "report"),
            e("other", "report"),
        ],
    )
}

fn decision_of(run: &WorkflowRun, id: &str) -> Option<String> {
    node_of(run, id, "")
        .decision
        .as_ref()
        .map(|d| d.value.clone())
}

#[test]
fn switch_value_that_matches_a_case_takes_only_that_branch() {
    let w = switch_workflow(json!({"value": "{{triage.kind}}"}));
    let run = complete(&begin(&w), "triage", json!({"kind": "feature"}));
    assert_eq!(status_of(&run, "route", ""), Some(NodeStatus::Done));
    assert_eq!(decision_of(&run, "route").as_deref(), Some("feature"));
    assert_eq!(status_of(&run, "fix", ""), Some(NodeStatus::Skipped));
    assert_eq!(status_of(&run, "other", ""), Some(NodeStatus::Skipped));
    assert_eq!(
        session_node_ids(&next_actions(&run)),
        vec!["plan".to_string()]
    );
}

#[test]
fn switch_value_with_no_matching_case_takes_default() {
    let w = switch_workflow(json!({"value": "{{triage.kind}}"}));
    for kind in ["chore", "", "Bug"] {
        let run = complete(&begin(&w), "triage", json!({ "kind": kind }));
        assert_eq!(
            decision_of(&run, "route").as_deref(),
            Some("default"),
            "{kind:?}"
        );
        assert_eq!(status_of(&run, "fix", ""), Some(NodeStatus::Skipped));
        assert_eq!(status_of(&run, "plan", ""), Some(NodeStatus::Skipped));
        assert_eq!(
            session_node_ids(&next_actions(&run)),
            vec!["other".to_string()]
        );
    }
}

#[test]
fn switch_decided_by_the_mono_asks_cases_and_default() {
    let w = switch_workflow(json!({"decide": "mono", "question": "What is {{inputs.project}}?"}));
    let mut run = complete(&begin(&w), "triage", json!({"kind": "x"}));
    match next_actions(&run).as_slice() {
        [EngineAction::AskMono {
            purpose,
            question,
            answers,
            ..
        }] => {
            assert_eq!(*purpose, AskPurpose::Switch);
            assert_eq!(question, "What is /work/repo?");
            assert_eq!(
                answers,
                &vec![
                    "bug".to_string(),
                    "feature".to_string(),
                    "default".to_string()
                ]
            );
        }
        other => panic!("expected one askMono, got {other:?}"),
    }
    run = started(&run, "route");
    assert_eq!(next_actions(&run), vec![]);
    run = decided_on(&run, "route", "bug", "", Some("looks like one"));
    assert_eq!(
        node_of(&run, "route", "").decision,
        Some(Decision {
            value: "bug".into(),
            reason: Some("looks like one".into())
        })
    );
    assert_eq!(status_of(&run, "plan", ""), Some(NodeStatus::Skipped));
    assert_eq!(
        session_node_ids(&next_actions(&run)),
        vec!["fix".to_string()]
    );
    // `default` is a valid answer too.
    let run = complete(&begin(&w), "triage", json!({}));
    let run = decided(&started(&run, "route"), "route", "default");
    assert_eq!(
        session_node_ids(&next_actions(&run)),
        vec!["other".to_string()]
    );
}

#[test]
fn switch_asks_a_default_question_that_names_the_node() {
    let w = switch_workflow(json!({"decide": "mono"}));
    let run = complete(&begin(&w), "triage", json!({}));
    match next_actions(&run).as_slice() {
        [EngineAction::AskMono { question, .. }] => assert_eq!(
            question,
            "Which case of \"route\" applies? Answer with one of: bug, feature, default."
        ),
        other => panic!("expected one askMono, got {other:?}"),
    }
}

#[test]
fn switch_rejects_an_answer_that_is_not_a_case() {
    let w = switch_workflow(json!({"decide": "mono"}));
    let run = complete(&begin(&w), "triage", json!({}));
    let run = decided(&started(&run, "route"), "route", "maybe");
    assert_eq!(status_of(&run, "route", ""), Some(NodeStatus::Failed));
    assert_eq!(
        node_of(&run, "route", "").error.as_deref(),
        Some("Invalid decision \"maybe\" (expected one of the cases or default)")
    );
    assert_eq!(decision_of(&run, "route"), None);
}

#[test]
fn switch_in_a_loop_is_decided_again_each_round() {
    let w = def(
        vec![
            trigger(),
            session("dev", json!({"output": {"kind": {"type": "string"}}})),
            n(
                json!({"id": "route", "type": "switch", "value": "{{dev.kind}}",
                "cases": ["bug", "feature"]}),
            ),
            plain("fix"),
            plain("plan"),
            plain("other"),
            plain("review"),
            report(),
        ],
        vec![
            e("trigger", "dev"),
            e("dev", "route"),
            eh("route", "fix", "bug"),
            eh("route", "plan", "feature"),
            eh("route", "other", "default"),
            e("fix", "review"),
            e("plan", "review"),
            e("other", "review"),
            e("review", "report"),
            lp("review", "dev", 3, "mono"),
        ],
    );
    let mut run = complete(&begin(&w), "dev", json!({"kind": "bug"}));
    assert_eq!(
        session_node_ids(&next_actions(&run)),
        vec!["fix".to_string()]
    );
    run = complete(&run, "fix", json!({}));
    run = complete(&run, "review", json!({}));
    run = decided(&run, "review", "loop");
    // The decision is cleared for the new round and taken again from the new output.
    assert_eq!(decision_of(&run, "route"), None);
    run = complete(&run, "dev", json!({"kind": "feature"}));
    assert_eq!(decision_of(&run, "route").as_deref(), Some("feature"));
    assert_eq!(status_of(&run, "fix", ""), Some(NodeStatus::Skipped));
    assert_eq!(
        session_node_ids(&next_actions(&run)),
        vec!["plan".to_string()]
    );
    run = complete(&run, "plan", json!({}));
    run = complete(&run, "review", json!({}));
    run = decided(&run, "review", "loop");
    run = complete(&run, "dev", json!({"kind": "something else"}));
    assert_eq!(decision_of(&run, "route").as_deref(), Some("default"));
    assert_eq!(
        session_node_ids(&next_actions(&run)),
        vec!["other".to_string()]
    );
}

#[test]
fn a_join_after_a_switch_runs_once_when_only_one_of_its_case_branches_is_live() {
    let w = def(
        vec![
            trigger(),
            session("triage", json!({"output": {"kind": {"type": "string"}}})),
            n(
                json!({"id": "route", "type": "switch", "value": "{{triage.kind}}",
                "cases": ["bug", "feature"]}),
            ),
            plain("fix"),
            plain("plan"),
            plain("join"),
            report(),
        ],
        vec![
            e("trigger", "triage"),
            e("triage", "route"),
            eh("route", "fix", "bug"),
            eh("route", "plan", "feature"),
            e("fix", "join"),
            e("plan", "join"),
            e("join", "report"),
        ],
    );
    let mut run = complete(&begin(&w), "triage", json!({"kind": "feature"}));
    assert_eq!(status_of(&run, "fix", ""), Some(NodeStatus::Skipped));
    assert_eq!(
        session_node_ids(&next_actions(&run)),
        vec!["plan".to_string()]
    );
    // The join waits for the live branch, not for the dead one.
    run = started(&run, "plan");
    assert_eq!(next_actions(&run), vec![]);
    run = finished(&run, "plan", json!({}), "", AT);
    assert_eq!(
        session_node_ids(&next_actions(&run)),
        vec!["join".to_string()]
    );
    run = complete(&run, "join", json!({}));
    assert_eq!(status_of(&run, "join", ""), Some(NodeStatus::Done));
    assert_eq!(next_actions(&run), post_report());
}

#[test]
fn a_switch_inside_a_for_each_picks_a_branch_per_item() {
    let kinds = json!([{"kind": "bug"}, {"kind": "feature"}, {"kind": "chore"}]);
    let w = fan_out(
        3,
        Some(vec![
            n(json!({"id": "route", "type": "switch", "parent": "each",
                "value": "{{item.kind}}", "cases": ["bug", "feature"]})),
            session("fix", json!({"parent": "each"})),
            session("plan", json!({"parent": "each"})),
            session("other", json!({"parent": "each"})),
        ]),
        vec![
            eh("route", "fix", "bug"),
            eh("route", "plan", "feature"),
            eh("route", "other", "default"),
        ],
    );
    let run = complete(&begin(&w), "fetch", json!({"tickets": kinds}));
    let decisions: Vec<Option<String>> = (0..3)
        .map(|i| {
            node_of(&run, "route", &format!("each#{i}"))
                .decision
                .as_ref()
                .map(|d| d.value.clone())
        })
        .collect();
    assert_eq!(
        decisions,
        vec![
            Some("bug".to_string()),
            Some("feature".to_string()),
            Some("default".to_string())
        ]
    );
    assert_eq!(
        session_targets(&next_actions(&run)),
        pairs(&[("fix", "each#0"), ("plan", "each#1"), ("other", "each#2")])
    );
    assert_eq!(status_of(&run, "plan", "each#0"), Some(NodeStatus::Skipped));
    assert_eq!(status_of(&run, "fix", "each#1"), Some(NodeStatus::Skipped));
    assert_eq!(status_of(&run, "fix", "each#2"), Some(NodeStatus::Skipped));
}
