//! Ports of the run-executing cases of `useMonoWorkflows.test.ts` (against a fake host),
//! `workflowSessionStart.test.ts` and the real host over a test `Monos`.
use super::text::*;
use super::*;
use async_channel::{Receiver, Sender, unbounded};
use gpui::TestAppContext;
use monocode_core::mono::workflows::{
    command::CommandResult,
    engine::start_run,
    model::{RunTrigger, Workflow},
    ordered::Json,
    parse::parse_workflow,
    validate::validate_workflow,
};
use serde_json::{Value, json};
use std::collections::VecDeque;

mod fake;
mod real;
use fake::*;

// --- pure helpers ---------------------------------------------------------------------------

#[test]
fn cap_output_cuts_long_strings_and_says_where() {
    let long = "x".repeat(20_000);
    let capped = cap_string(&long);
    assert!(capped.starts_with(&"x".repeat(16_000)));
    assert!(capped.ends_with("\n[truncated, 4000 more characters]"));
    assert_eq!(cap_string("short"), "short");
    assert_eq!(cap_string(&"y".repeat(16_000)).len(), 16_000);
}

#[test]
fn cap_output_walks_lists_and_objects_but_leaves_other_values() {
    let long = "z".repeat(16_001);
    let value =
        Json::parse(&json!({"a": [long, {"b": long}], "n": 3, "t": true, "u": null}).to_string())
            .unwrap();
    let capped = cap_output(&value).stringify(false);
    assert_eq!(capped.matches("[truncated, 1 more characters]").count(), 2);
    assert!(
        capped.contains("\"n\":3")
            && capped.contains("\"t\":true")
            && capped.contains("\"u\":null")
    );
    // Past 24 levels of nesting the value is kept as it is, like the TS walk.
    let mut deep = Json::String(long.clone());
    for _ in 0..30 {
        deep = Json::Array(vec![deep]);
    }
    assert!(cap_output(&deep).stringify(false).contains(&long));
}

#[test]
fn cap_output_counts_utf16_units_like_js() {
    // Each of these is two UTF-16 units: 8,001 of them are 16,002 units.
    let text = "😀".repeat(8_001);
    let capped = cap_string(&text);
    assert!(capped.starts_with(&"😀".repeat(8_000)));
    assert!(capped.ends_with("[truncated, 2 more characters]"));
    // A cut in the middle of a pair drops the pair instead of making a lone surrogate.
    let odd = format!("a{}", "😀".repeat(8_000));
    assert!(cap_string(&odd).starts_with(&format!("a{}", "😀".repeat(7_999))));
}

#[test]
fn errors_keep_their_end_behind_an_ellipsis() {
    assert_eq!(clip("short"), "short");
    let long = format!("{}END", "a".repeat(2_000));
    let clipped = clip(&long);
    assert!(clipped.starts_with('…') && clipped.ends_with("aaEND"));
    assert_eq!(clipped.encode_utf16().count(), 1_001);
}

#[test]
fn utf16_slices_never_split_a_character() {
    assert_eq!(utf16_head("a😀b", 2), "a");
    assert_eq!(utf16_head("a😀b", 3), "a😀");
    assert_eq!(utf16_tail("a😀b", 2), "b");
    assert_eq!(utf16_tail("a😀b", 3), "😀b");
    assert_eq!(utf16_head("abc", 10), "abc");
    assert_eq!(utf16_tail("abc", 10), "abc");
}

#[test]
fn the_run_folder_is_named_after_the_workflow_and_the_minute_it_started() {
    use chrono::{Local, TimeZone};
    let at = Local
        .with_ymd_and_hms(2026, 10, 22, 14, 5, 59)
        .single()
        .unwrap()
        .timestamp_millis() as u64;
    assert_eq!(run_folder_name("Ship it", at), "Ship it · 2026-10-22 14:05");
    // White space runs collapse and the ends are trimmed.
    assert_eq!(
        run_folder_name("  Ship \n\t it  ", at),
        "Ship it · 2026-10-22 14:05"
    );
    assert_eq!(run_folder_name("   ", at), "Workflow · 2026-10-22 14:05");
}

#[test]
fn the_run_folder_name_stays_within_100_characters() {
    let name = run_folder_name(&"w".repeat(300), 1_700_000_000_000);
    assert_eq!(name.encode_utf16().count(), 100);
    assert!(name.contains(" · 20"));
    let wide = run_folder_name(&"😀".repeat(300), 1_700_000_000_000);
    assert!(wide.encode_utf16().count() <= 100);
}

#[test]
fn a_turn_maps_to_the_outcome_a_run_reads() {
    use crate::conversation::{TurnEnd, WorkflowTurn};
    let turn = |end| WorkflowTurn {
        end,
        reply: "said".into(),
    };
    assert_eq!(
        host::outcome_of(turn(TurnEnd::Completed)),
        Outcome::completed("said")
    );
    let cancelled = host::outcome_of(turn(TurnEnd::Cancelled));
    assert_eq!(cancelled.status, Status::Cancelled);
    let failed = host::outcome_of(turn(TurnEnd::Failed("No network".into())));
    assert_eq!(failed.status, Status::Failed);
    assert_eq!(failed.error.as_deref(), Some("No network"));
}

#[test]
fn a_session_not_seen_working_is_given_up_on_unless_it_failed_to_start() {
    use crate::conversation::{TurnEnd, WorkflowTurn};
    use host::{Seen, look};
    let over = |end| {
        Seen::Over(WorkflowTurn {
            end,
            reply: String::new(),
        })
    };
    let (mut armed, mut unstarted) = (false, 0);
    // A prompt that failed to start says why at once, with the conversation's own status.
    let failed = look(
        over(TurnEnd::Failed("No network".into())),
        &mut armed,
        &mut unstarted,
    );
    assert_eq!(failed.unwrap().error.as_deref(), Some("No network"));
    // An idle session that was never working: patience, then a failure.
    for _ in 0..10 {
        assert_eq!(
            look(over(TurnEnd::Completed), &mut armed, &mut unstarted),
            None
        );
    }
    let late = look(over(TurnEnd::Completed), &mut armed, &mut unstarted).unwrap();
    assert_eq!(
        late.error.as_deref(),
        Some("The session did not start a turn.")
    );
    // Once seen working, idle means the turn is over.
    assert_eq!(look(Seen::Working, &mut armed, &mut unstarted), None);
    assert_eq!(
        look(over(TurnEnd::Completed), &mut armed, &mut unstarted)
            .unwrap()
            .status,
        Status::Completed
    );
}

// --- the primitives -------------------------------------------------------------------------

#[gpui::test]
fn the_gate_hands_out_turns_in_order(cx: &mut TestAppContext) {
    let gate = Gate::default();
    let log = Rc::new(RefCell::new(Vec::new()));
    let (release, wait) = unbounded::<()>();
    for name in ["a", "b", "c"] {
        let (gate, log, wait) = (gate.clone(), log.clone(), wait.clone());
        cx.spawn(async move |_| {
            let _turn = gate.turn().await;
            log.borrow_mut().push(format!("{name} in"));
            let _ = wait.recv().await;
            log.borrow_mut().push(format!("{name} out"));
        })
        .detach();
    }
    cx.run_until_parked();
    assert_eq!(*log.borrow(), ["a in"]);
    release.try_send(()).unwrap();
    cx.run_until_parked();
    assert_eq!(*log.borrow(), ["a in", "a out", "b in"]);
    release.try_send(()).unwrap();
    release.try_send(()).unwrap();
    cx.run_until_parked();
    assert_eq!(
        *log.borrow(),
        ["a in", "a out", "b in", "b out", "c in", "c out"]
    );
}

#[gpui::test]
fn a_signal_releases_every_waiter_now_and_later(cx: &mut TestAppContext) {
    let signal = Signal::default();
    let seen = Rc::new(Cell::new(0));
    for _ in 0..2 {
        let (signal, seen) = (signal.clone(), seen.clone());
        cx.spawn(async move |_| {
            signal.wait().await;
            seen.set(seen.get() + 1);
        })
        .detach();
    }
    cx.run_until_parked();
    assert_eq!(seen.get(), 0);
    signal.fire();
    cx.run_until_parked();
    assert_eq!(seen.get(), 2);
    let late = seen.clone();
    let again = signal.clone();
    cx.spawn(async move |_| {
        again.wait().await;
        late.set(late.get() + 1);
    })
    .detach();
    cx.run_until_parked();
    assert_eq!(seen.get(), 3);
}

#[gpui::test]
fn a_nudge_is_remembered_until_someone_waits(cx: &mut TestAppContext) {
    let nudge = Nudge::default();
    nudge.nudge();
    let seen = Rc::new(Cell::new(0));
    let (n, s) = (nudge.clone(), seen.clone());
    cx.spawn(async move |_| {
        n.wait().await;
        s.set(1);
        n.wait().await;
        s.set(2);
    })
    .detach();
    cx.run_until_parked();
    assert_eq!(seen.get(), 1, "the early nudge counted once");
    nudge.nudge();
    cx.run_until_parked();
    assert_eq!(seen.get(), 2);
}

// --- workflows ------------------------------------------------------------------------------

const WF: &str = "wf-abcd1234";

fn trigger() -> Value {
    json!({"id": "trigger", "type": "manual"})
}

fn report() -> Value {
    json!({"id": "report", "type": "report"})
}

fn session(id: &str, output: Value, extra: Value) -> Value {
    let mut node = json!({
        "id": id, "type": "session", "project": "{{inputs.repo}}",
        "prompt": format!("Do {id}"), "checkout": "root", "output": output,
    });
    for (key, value) in extra.as_object().unwrap() {
        node[key] = value.clone();
    }
    node
}

fn edge(from: &str, to: &str) -> Value {
    json!({"from": from, "to": to})
}

fn workflow(nodes: Vec<Value>, edges: Vec<Value>) -> Workflow {
    workflow_with(
        nodes,
        edges,
        json!({"repo": "/code/app", "lib": "/code/lib"}),
    )
}

fn workflow_with(nodes: Vec<Value>, edges: Vec<Value>, values: Value) -> Workflow {
    let inputs = json!({"repo": {"type": "project"}, "lib": {"type": "project"}});
    let def = json!({
        "id": WF, "name": "Ship it", "inputs": inputs, "inputValues": values,
        "nodes": nodes, "edges": edges, "enabled": true, "createdAt": 0,
    });
    let built = parse_workflow(&Json::from(&def)).expect("a workflow");
    assert_eq!(validate_workflow(&built), vec![]);
    built
}

/// trigger -> dev -> report
fn simple(output: Value) -> Workflow {
    workflow(
        vec![trigger(), session("dev", output, json!({})), report()],
        vec![edge("trigger", "dev"), edge("dev", "report")],
    )
}

fn fenced(value: Value) -> String {
    format!("Done.\n```workflow-output\n{value}\n```")
}

fn command_workflow(extra: Value) -> Workflow {
    let mut cmd = json!({"id": "cmd", "type": "command", "project": "{{inputs.repo}}", "command": "./deploy.sh"});
    for (key, value) in extra.as_object().unwrap() {
        cmd[key] = value.clone();
    }
    workflow(
        vec![trigger(), cmd, report()],
        vec![edge("trigger", "cmd"), edge("cmd", "report")],
    )
}

fn ask_workflow() -> Workflow {
    workflow(
        vec![
            trigger(),
            json!({"id": "ask", "type": "askMono", "question": "Ship it?", "answers": ["yes", "no"]}),
            report(),
        ],
        vec![edge("trigger", "ask"), edge("ask", "report")],
    )
}

fn node<'a>(run: &'a WorkflowRun, id: &str) -> &'a NodeRun {
    run.nodes.iter().find(|n| n.node_id == id).unwrap()
}

fn output_of(run: &WorkflowRun, id: &str) -> String {
    node(run, id)
        .output
        .as_ref()
        .unwrap()
        .iter()
        .map(|(k, v)| format!("{k}={}", v.stringify(false)))
        .collect::<Vec<_>>()
        .join(",")
}

fn minutes(cx: &TestAppContext, seconds: u64) {
    // The poll is every 2 s; a step this size runs every timer in between.
    cx.executor().advance_clock(Duration::from_secs(seconds));
    cx.run_until_parked();
}

// --- the run: sessions ----------------------------------------------------------------------

#[gpui::test]
fn a_run_starts_a_session_per_node_and_posts_one_report(cx: &mut TestAppContext) {
    let rig = Rig::new(&[&fenced(json!({"summary": "all good"}))]);
    let def = simple(json!({"summary": {"type": "string"}}));
    assert!(rig.start(cx, &def));
    cx.run_until_parked();

    let run = rig.last_run();
    assert_eq!(run.status, RunStatus::Done);
    let dev = node(&run, "dev");
    assert_eq!(dev.status, NodeStatus::Done);
    assert_eq!(dev.session_id.as_deref(), Some("s1"));
    assert_eq!(output_of(&run, "dev"), "summary=\"all good\"");
    assert_eq!(run.folder_id.as_deref(), Some("folder:/code/app"));

    let w = rig.fake.world();
    let request = &w.starts[0];
    assert_eq!(request.project, "/code/app");
    assert!(!request.new_worktree);
    assert!(request.prompt.contains("Do dev") && request.prompt.contains("workflow-output"));
    assert_eq!(
        request.folder_name.as_deref(),
        Some(run_folder_name("Ship it", 1_000).as_str())
    );
    assert_eq!(request.folder_id, None);

    assert_eq!(w.posts.len(), 1);
    let post = &w.posts[0];
    assert_eq!(
        (post.mono_id.as_str(), post.run_id.as_str()),
        ("mono-1", "run-1")
    );
    assert_eq!(post.title, "Ship it · completed");
    assert!(post.text.contains("all good"));
    assert_eq!(
        post.launches,
        [SessionInfo {
            session_id: "s1".into(),
            cwd: "/code/app".into(),
            title: "Session s1".into(),
            harness: "claude".into(),
            model: Some("claude:x".into()),
        }]
    );
    drop(w);
    assert!(!rig.signals.is_running(WF), "the run is released");
    assert!(!rig.runner.is_running(WF));
}

#[gpui::test]
fn a_second_run_of_the_same_workflow_is_refused_while_one_goes(cx: &mut TestAppContext) {
    let rig = Rig::new(&["ok"]);
    let hold = rig.fake.hold();
    assert!(rig.start(cx, &simple(json!({}))));
    cx.run_until_parked();
    assert!(rig.signals.is_running(WF));
    assert!(rig.runner.is_running(WF));
    assert!(!rig.start(cx, &simple(json!({}))), "one run per workflow");
    hold.close();
    cx.run_until_parked();
    assert_eq!(rig.last_run().status, RunStatus::Done);
    assert!(!rig.runner.is_running(WF));
    assert!(
        rig.start(cx, &simple(json!({}))),
        "free again once it ended"
    );
}

#[gpui::test]
fn the_record_is_written_before_anything_runs(cx: &mut TestAppContext) {
    let rig = Rig::new(&["ok"]);
    let hold = rig.fake.hold();
    rig.start(cx, &simple(json!({})));
    cx.run_until_parked();
    let w = rig.fake.world();
    assert_eq!(w.saves.first().unwrap().status, RunStatus::Running);
    assert_eq!(
        node(w.saves.first().unwrap(), "dev").status,
        NodeStatus::Pending
    );
    drop(w);
    hold.close();
    cx.run_until_parked();
}

#[gpui::test]
fn it_re_asks_once_for_missing_output_then_fails_the_node(cx: &mut TestAppContext) {
    let rig = Rig::new(&["I did it.", "Still no block."]);
    rig.start(cx, &simple(json!({"summary": {"type": "string"}})));
    cx.run_until_parked();
    let w = rig.fake.world();
    assert_eq!(w.sends.len(), 1);
    assert_eq!(w.sends[0].0, "s1");
    assert!(w.sends[0].1.contains("did not satisfy the output contract"));
    assert_eq!(w.posts[0].title, "Ship it · failed");
    drop(w);
    let run = rig.last_run();
    assert_eq!(run.status, RunStatus::Failed);
    let dev = node(&run, "dev");
    assert_eq!(dev.status, NodeStatus::Failed);
    assert_eq!(dev.reasked, Some(true));
    assert!(dev.error.as_deref().unwrap().contains("workflow-output"));
}

#[gpui::test]
fn it_accepts_the_re_asked_reply_when_it_has_the_block(cx: &mut TestAppContext) {
    let rig = Rig::new(&["no block", &fenced(json!({"summary": "second try"}))]);
    rig.start(cx, &simple(json!({"summary": {"type": "string"}})));
    cx.run_until_parked();
    let run = rig.last_run();
    assert_eq!(run.status, RunStatus::Done);
    assert_eq!(rig.fake.world().sends.len(), 1);
    assert_eq!(output_of(&run, "dev"), "summary=\"second try\"");
    assert_eq!(node(&run, "dev").reasked, Some(true));
}

#[gpui::test]
fn it_never_re_asks_a_node_that_declares_no_output(cx: &mut TestAppContext) {
    let rig = Rig::new(&["just prose, no block"]);
    rig.start(cx, &simple(json!({})));
    cx.run_until_parked();
    assert_eq!(rig.last_run().status, RunStatus::Done);
    assert!(rig.fake.world().sends.is_empty());
}

#[gpui::test]
fn a_run_lets_go_of_every_session_it_used_once_it_is_over(cx: &mut TestAppContext) {
    let rig = Rig::new(&["ok"]);
    let hold = rig.fake.hold();
    rig.start(cx, &simple(json!({})));
    cx.run_until_parked();
    assert!(rig.fake.world().released.is_empty(), "not while it runs");
    hold.close();
    cx.run_until_parked();
    assert_eq!(rig.fake.world().released, ["s1"]);
}

#[gpui::test]
fn a_loop_continues_the_dev_session_through_send(cx: &mut TestAppContext) {
    let def = workflow(
        vec![
            trigger(),
            session("dev", json!({"summary": {"type": "string"}}), json!({})),
            session("review", json!({"verdict": {"type": "string"}}), json!({})),
            report(),
        ],
        vec![
            edge("trigger", "dev"),
            edge("dev", "review"),
            json!({"from": "review", "to": "dev", "loop": {"maxRounds": 3, "decide": "{{review.verdict}} != \"approve\""}}),
            edge("review", "report"),
        ],
    );
    let rig = Rig::new(&[
        &fenced(json!({"summary": "v1"})),
        &fenced(json!({"verdict": "changes"})),
        &fenced(json!({"summary": "v2"})),
        &fenced(json!({"verdict": "approve"})),
    ]);
    rig.start(cx, &def);
    cx.run_until_parked();
    assert_eq!(rig.last_run().status, RunStatus::Done);
    let w = rig.fake.world();
    assert_eq!(w.starts.len(), 2);
    // Round 2 continues the dev session, then the review session.
    assert_eq!(
        w.sends.iter().map(|s| s.0.as_str()).collect::<Vec<_>>(),
        ["s1", "s2"]
    );
    assert!(w.sends[0].1.contains("Do dev"));
    assert!(w.posts[0].text.contains("Loop rounds used: 2"));
}

#[gpui::test]
fn a_mono_decided_loop_asks_the_mono_and_goes_round_again(cx: &mut TestAppContext) {
    let def = workflow(
        vec![
            trigger(),
            session("dev", json!({"summary": {"type": "string"}}), json!({})),
            session("review", json!({"notes": {"type": "string"}}), json!({})),
            report(),
        ],
        vec![
            edge("trigger", "dev"),
            edge("dev", "review"),
            edge("review", "report"),
            json!({"from": "review", "to": "dev", "loop": {"maxRounds": 3, "decide": "mono"}}),
        ],
    );
    let rig = Rig::new(&[
        &fenced(json!({"summary": "a"})),
        &fenced(json!({"notes": "meh"})),
        &fenced(json!({"summary": "b"})),
        &fenced(json!({"notes": "good"})),
    ]);
    rig.fake.answer(Ok(fenced(
        json!({"answer": "loop", "reason": "not enough"}),
    )));
    rig.fake
        .answer(Ok(fenced(json!({"answer": "exit", "reason": "good"}))));
    rig.start(cx, &def);
    cx.run_until_parked();
    let run = rig.last_run();
    assert_eq!(run.status, RunStatus::Done);
    let w = rig.fake.world();
    assert_eq!(w.asks.len(), 2);
    assert_eq!(w.asks[0].0, "mono-1");
    assert!(w.asks[0].1.contains("Allowed answers"));
    // The second round continues both sessions.
    assert_eq!(w.sends.len(), 2);
    drop(w);
    let decision = node(&run, "review").decision.clone().unwrap();
    assert_eq!(
        (decision.value.as_str(), decision.reason.as_deref()),
        ("exit", Some("good"))
    );
}

#[gpui::test]
fn the_node_fails_when_the_monos_answer_cannot_be_read(cx: &mut TestAppContext) {
    let rig = Rig::new(&[]);
    rig.fake.answer(Ok("I think so, not sure.".into()));
    rig.start(cx, &ask_workflow());
    cx.run_until_parked();
    let run = rig.last_run();
    assert_eq!(run.status, RunStatus::Failed);
    let ask = node(&run, "ask");
    assert_eq!(ask.status, NodeStatus::Failed);
    assert!(ask.error.as_deref().unwrap().contains("workflow-output"));
}

#[gpui::test]
fn the_monos_answer_is_a_decision_and_its_reason_is_capped(cx: &mut TestAppContext) {
    let rig = Rig::new(&[]);
    rig.fake.answer(Ok(fenced(
        json!({"answer": "yes", "reason": "r".repeat(20_000)}),
    )));
    rig.start(cx, &ask_workflow());
    cx.run_until_parked();
    let run = rig.last_run();
    assert_eq!(run.status, RunStatus::Done);
    let output = node(&run, "ask").output.clone().unwrap();
    assert_eq!(output.get("answer"), Some(&Json::String("yes".into())));
    let Some(Json::String(reason)) = output.get("reason") else {
        panic!("a reason")
    };
    assert!(reason.len() < 16_200 && reason.contains("truncated"));
}

#[gpui::test]
fn a_host_that_cannot_ask_fails_the_node_with_its_message(cx: &mut TestAppContext) {
    let rig = Rig::new(&[]);
    rig.fake
        .answer(Err("The Mono's chat is not available".into()));
    rig.start(cx, &ask_workflow());
    cx.run_until_parked();
    let ask = rig.last_run();
    assert_eq!(
        node(&ask, "ask").error.as_deref(),
        Some("The Mono's chat is not available")
    );
}

#[gpui::test]
fn an_unanswered_question_fails_after_ten_minutes(cx: &mut TestAppContext) {
    let rig = Rig::new(&[]);
    rig.fake.hang_next_ask();
    rig.start(cx, &ask_workflow());
    cx.run_until_parked();
    assert_eq!(rig.last_run().status, RunStatus::Running);
    minutes(cx, 9 * 60);
    assert_eq!(rig.last_run().status, RunStatus::Running);
    minutes(cx, 61);
    let run = rig.last_run();
    assert_eq!(run.status, RunStatus::Failed);
    assert_eq!(
        node(&run, "ask").error.as_deref(),
        Some("The Mono did not answer in time")
    );
}

#[gpui::test]
fn a_long_output_string_is_cut_before_it_is_stored(cx: &mut TestAppContext) {
    let rig = Rig::new(&[&fenced(json!({"summary": "x".repeat(20_000)}))]);
    rig.start(cx, &simple(json!({"summary": {"type": "string"}})));
    cx.run_until_parked();
    let run = rig.last_run();
    let output = output_of(&run, "dev");
    assert!(output.len() < 16_200 && output.contains("truncated"));
}

#[gpui::test]
fn a_project_that_is_not_the_monos_fails_the_node(cx: &mut TestAppContext) {
    let def = workflow_with(
        vec![
            trigger(),
            session("dev", json!({}), json!({"project": "{{inputs.lib}}"})),
            report(),
        ],
        vec![edge("trigger", "dev"), edge("dev", "report")],
        json!({"repo": "/code/app", "lib": "/elsewhere"}),
    );
    let rig = Rig::new(&[]);
    rig.start(cx, &def);
    cx.run_until_parked();
    let run = rig.last_run();
    assert_eq!(
        node(&run, "dev").error.as_deref(),
        Some("\"{{inputs.lib}}\" is not one of the Mono's projects")
    );
    assert!(rig.fake.world().starts.is_empty());
}

#[gpui::test]
fn a_session_that_fails_to_start_fails_its_node_with_the_hosts_message(cx: &mut TestAppContext) {
    let rig = Rig::new(&[]);
    rig.fake
        .fail_starts("Model \"codex:gone\" is not available. Pick one in the step's model list.");
    rig.start(cx, &simple(json!({})));
    cx.run_until_parked();
    let run = rig.last_run();
    assert_eq!(run.status, RunStatus::Failed);
    assert!(
        node(&run, "dev")
            .error
            .as_deref()
            .unwrap()
            .starts_with("Model \"codex:gone\"")
    );
}

#[gpui::test]
fn a_session_that_ends_in_error_fails_its_node_with_the_error(cx: &mut TestAppContext) {
    let rig = Rig::new(&[]);
    rig.fake
        .end_turns_with(Outcome::failed("Claude Code reached its usage limit."));
    rig.start(cx, &simple(json!({})));
    cx.run_until_parked();
    let run = rig.last_run();
    assert_eq!(
        node(&run, "dev").error.as_deref(),
        Some("Claude Code reached its usage limit.")
    );
}

#[gpui::test]
fn it_nests_under_another_nodes_session_and_uses_its_worktree(cx: &mut TestAppContext) {
    let def = workflow(
        vec![
            trigger(),
            session("dev", json!({}), json!({})),
            session(
                "review",
                json!({}),
                json!({"nestUnder": "dev", "checkout": {"worktreeOf": "dev"}}),
            ),
            report(),
        ],
        vec![
            edge("trigger", "dev"),
            edge("dev", "review"),
            edge("review", "report"),
        ],
    );
    let rig = Rig::new(&["ok", "ok"]);
    rig.start(cx, &def);
    cx.run_until_parked();
    assert_eq!(rig.last_run().status, RunStatus::Done);
    let w = rig.fake.world();
    let second = &w.starts[1];
    assert!(!second.new_worktree);
    assert_eq!(second.parent_session_id.as_deref(), Some("s1"));
    assert_eq!(second.worktree_cwd.as_deref(), Some("/code/app-wt-s1"));
    assert_eq!(second.folder_id.as_deref(), Some("folder:/code/app"));
    assert_eq!(second.folder_name, w.starts[0].folder_name);
}

#[gpui::test]
fn a_new_worktree_checkout_asks_for_one(cx: &mut TestAppContext) {
    let def = workflow(
        vec![
            trigger(),
            session(
                "dev",
                json!({}),
                json!({"checkout": "newWorktree", "harness": "codex", "model": "codex:test", "effort": "high"}),
            ),
            report(),
        ],
        vec![edge("trigger", "dev"), edge("dev", "report")],
    );
    let rig = Rig::new(&["ok"]);
    rig.start(cx, &def);
    cx.run_until_parked();
    let w = rig.fake.world();
    let request = &w.starts[0];
    assert!(request.new_worktree);
    assert_eq!(
        (
            request.harness.as_deref(),
            request.model.as_deref(),
            request.effort.as_deref()
        ),
        (Some("codex"), Some("codex:test"), Some("high"))
    );
}

#[gpui::test]
fn it_fails_the_node_when_the_worktree_it_wants_is_not_there(cx: &mut TestAppContext) {
    let def = workflow(
        vec![
            trigger(),
            session("dev", json!({}), json!({})),
            session(
                "review",
                json!({}),
                json!({"checkout": {"worktreeOf": "dev"}}),
            ),
            report(),
        ],
        vec![
            edge("trigger", "dev"),
            edge("dev", "review"),
            edge("review", "report"),
        ],
    );
    let rig = Rig::new(&["ok"]);
    rig.fake.no_worktrees();
    rig.start(cx, &def);
    cx.run_until_parked();
    let run = rig.last_run();
    assert_eq!(
        node(&run, "review").error.as_deref(),
        Some("No worktree is available from \"dev\"")
    );
    assert_eq!(rig.fake.world().starts.len(), 1);
}

#[gpui::test]
fn it_keeps_one_folder_per_project_for_a_run(cx: &mut TestAppContext) {
    let def = workflow(
        vec![
            trigger(),
            session("dev", json!({}), json!({})),
            session("docs", json!({}), json!({"project": "{{inputs.lib}}"})),
            session("more", json!({}), json!({"project": "{{inputs.lib}}"})),
            report(),
        ],
        vec![
            edge("trigger", "dev"),
            edge("dev", "docs"),
            edge("docs", "more"),
            edge("more", "report"),
        ],
    );
    let rig = Rig::new(&["ok", "ok", "ok"]);
    rig.start(cx, &def);
    cx.run_until_parked();
    assert_eq!(rig.last_run().status, RunStatus::Done);
    let w = rig.fake.world();
    // First session: no folder yet, so it names one.
    assert_eq!(w.starts[0].project, "/code/app");
    assert_eq!(w.starts[0].folder_id, None);
    // First session in another project: the name makes a folder there.
    assert_eq!(w.starts[1].project, "/code/lib");
    assert_eq!(w.starts[1].folder_name, w.starts[0].folder_name);
    // Later sessions reuse the folder of their own project, not the first project's.
    assert_eq!(w.starts[2].project, "/code/lib");
    assert_eq!(w.starts[2].folder_id.as_deref(), Some("folder:/code/lib"));
    drop(w);
    assert_eq!(
        rig.last_run().folder_id.as_deref(),
        Some("folder:/code/app")
    );
}

// --- the run: timeouts and approvals --------------------------------------------------------

fn one_minute() -> Workflow {
    workflow(
        vec![
            trigger(),
            session("dev", json!({}), json!({"timeoutMinutes": 1})),
            report(),
        ],
        vec![edge("trigger", "dev"), edge("dev", "report")],
    )
}

#[gpui::test]
fn a_session_that_works_too_long_is_stopped_and_fails(cx: &mut TestAppContext) {
    let rig = Rig::new(&["late"]);
    rig.fake.hold();
    rig.start(cx, &one_minute());
    cx.run_until_parked();
    minutes(cx, 58);
    assert_eq!(rig.last_run().status, RunStatus::Running);
    assert!(rig.fake.world().stops.is_empty());
    minutes(cx, 4);
    let run = rig.last_run();
    assert_eq!(run.status, RunStatus::Failed);
    assert_eq!(
        node(&run, "dev").error.as_deref(),
        Some("It took longer than 1 minutes and was stopped.")
    );
    assert_eq!(rig.fake.world().stops, ["s1"]);
}

#[gpui::test]
fn time_spent_waiting_for_an_approval_does_not_count_toward_the_timeout(cx: &mut TestAppContext) {
    let rig = Rig::new(&["done"]);
    let hold = rig.fake.hold();
    rig.start(cx, &one_minute());
    cx.run_until_parked();
    minutes(cx, 20);
    // Five minutes on the user: far past the node's minute.
    rig.fake.set_waiting("s1", true);
    minutes(cx, 5 * 60);
    assert_eq!(rig.last_run().status, RunStatus::Running);
    assert_eq!(
        node(&rig.last_run(), "dev").status,
        NodeStatus::WaitingApproval
    );
    rig.fake.set_waiting("s1", false);
    minutes(cx, 30);
    assert_eq!(node(&rig.last_run(), "dev").status, NodeStatus::Running);
    hold.close();
    cx.run_until_parked();
    assert_eq!(rig.last_run().status, RunStatus::Done);
    assert!(rig.fake.world().stops.is_empty());
}

#[gpui::test]
fn working_time_before_and_after_an_approval_adds_up(cx: &mut TestAppContext) {
    let rig = Rig::new(&["done"]);
    rig.fake.hold();
    rig.start(cx, &one_minute());
    cx.run_until_parked();
    minutes(cx, 40);
    rig.fake.set_waiting("s1", true);
    minutes(cx, 600);
    rig.fake.set_waiting("s1", false);
    minutes(cx, 16);
    assert_eq!(rig.last_run().status, RunStatus::Running);
    minutes(cx, 6);
    assert_eq!(rig.last_run().status, RunStatus::Failed);
}

#[gpui::test]
fn an_approval_nobody_answers_ends_the_session(cx: &mut TestAppContext) {
    let rig = Rig::with_limits(
        &[],
        Limits {
            approval_wait_ms: 10_000,
            ..Limits::default()
        },
    );
    rig.fake.hold();
    rig.start(cx, &simple(json!({})));
    cx.run_until_parked();
    rig.fake.set_waiting("s1", true);
    minutes(cx, 12);
    let run = rig.last_run();
    assert_eq!(
        node(&run, "dev").error.as_deref(),
        Some("Nobody answered its approval request.")
    );
    assert_eq!(rig.fake.world().stops, ["s1"]);
}

#[gpui::test]
fn an_approval_is_shown_and_the_node_is_marked_waiting(cx: &mut TestAppContext) {
    let rig = Rig::new(&["ok"]);
    let hold = rig.fake.hold();
    rig.start(cx, &simple(json!({})));
    cx.run_until_parked();
    rig.fake.set_waiting("s1", true);
    minutes(cx, 5);
    {
        let w = rig.fake.world();
        assert!(!w.approvals.is_empty());
        let ask = &w.approvals[0];
        assert_eq!(
            (
                ask.mono_id.as_str(),
                ask.session_id.as_str(),
                ask.run_id.as_str()
            ),
            ("mono-1", "s1", "run-1")
        );
        assert_eq!(
            (ask.workflow_id.as_str(), ask.workflow_name.as_str()),
            (WF, "Ship it")
        );
    }
    assert_eq!(
        node(&rig.last_run(), "dev").status,
        NodeStatus::WaitingApproval
    );

    rig.fake.set_waiting("s1", false);
    hold.close();
    cx.run_until_parked();
    let run = rig.last_run();
    assert_eq!(run.status, RunStatus::Done);
    assert!(rig.fake.world().ended.contains(&"s1".to_string()));
}

// --- the run: commands ----------------------------------------------------------------------

#[gpui::test]
fn a_command_runs_in_the_project_and_its_output_is_kept(cx: &mut TestAppContext) {
    let rig = Rig::new(&[]);
    rig.fake
        .command_returns(command_result(Some(0), "fine\n", "warn\n"));
    rig.start(
        cx,
        &command_workflow(json!({"command": "echo hi", "timeoutMinutes": 2})),
    );
    cx.run_until_parked();
    let run = rig.last_run();
    assert_eq!(run.status, RunStatus::Done);
    assert_eq!(
        output_of(&run, "cmd"),
        "exitCode=0,stdout=\"fine\\n\",stderr=\"warn\\n\""
    );
    let w = rig.fake.world();
    let (cwd, command, timeout, id) = w.commands[0].clone();
    assert_eq!(
        (cwd.as_str(), command.as_str(), timeout),
        ("/code/app", "echo hi", 2 * 60_000)
    );
    let parts: Vec<&str> = id.split(':').collect();
    assert_eq!(
        (parts[0], parts[2], parts[3], parts[4]),
        ("run-1", "cmd", "", "1")
    );
}

#[gpui::test]
fn a_command_runs_in_the_worktree_of_an_earlier_session(cx: &mut TestAppContext) {
    let def = workflow(
        vec![
            trigger(),
            session("dev", json!({}), json!({})),
            json!({"id": "cmd", "type": "command", "project": "{{inputs.repo}}", "command": "make", "cwd": {"worktreeOf": "dev"}}),
            report(),
        ],
        vec![
            edge("trigger", "dev"),
            edge("dev", "cmd"),
            edge("cmd", "report"),
        ],
    );
    let rig = Rig::new(&["ok"]);
    rig.start(cx, &def);
    cx.run_until_parked();
    assert_eq!(rig.fake.world().commands[0].0, "/code/app-wt-s1");
    assert_eq!(rig.last_run().status, RunStatus::Done);
}

#[gpui::test]
fn a_failing_command_fails_only_its_branch(cx: &mut TestAppContext) {
    let def = workflow(
        vec![
            trigger(),
            session("fetch", json!({"tickets": {"type": "list"}}), json!({})),
            json!({"id": "each", "type": "forEach", "in": "{{fetch.tickets}}", "as": "item"}),
            json!({"id": "cmd", "type": "command", "project": "{{inputs.repo}}", "command": "echo {{item}}", "parent": "each"}),
            report(),
        ],
        vec![
            edge("trigger", "fetch"),
            edge("fetch", "each"),
            edge("each", "report"),
        ],
    );
    let rig = Rig::new(&[&fenced(json!({"tickets": ["ok", "bad"]}))]);
    rig.fake
        .command_returns(command_result(Some(0), "fine", ""));
    rig.fake
        .command_returns(command_result(Some(1), "", "boom"));
    rig.start(cx, &def);
    cx.run_until_parked();
    let run = rig.last_run();
    assert_eq!(run.status, RunStatus::DoneWithFailures);
    let cmds: Vec<_> = run.nodes.iter().filter(|n| n.node_id == "cmd").collect();
    assert_eq!(
        cmds.iter()
            .map(|n| (n.branch.as_str(), n.status))
            .collect::<Vec<_>>(),
        [("each#0", NodeStatus::Done), ("each#1", NodeStatus::Failed)]
    );
    assert_eq!(
        cmds[1].error.as_deref(),
        Some("The command exited with code 1: boom")
    );
    let w = rig.fake.world();
    assert_eq!(w.commands[0].1, "echo 'ok'");
    assert_eq!(w.commands[0].2, 10 * 60_000);
    let ids: std::collections::HashSet<_> = w.commands.iter().map(|c| c.3.clone()).collect();
    assert_eq!(ids.len(), w.commands.len(), "every command has its own id");
    assert_eq!(w.posts[0].title, "Ship it · completed with failures");
}

#[gpui::test]
fn a_command_that_exits_without_a_code_or_output_says_so(cx: &mut TestAppContext) {
    let rig = Rig::new(&[]);
    rig.fake.command_returns(command_result(None, "", ""));
    rig.start(cx, &command_workflow(json!({})));
    cx.run_until_parked();
    assert_eq!(
        node(&rig.last_run(), "cmd").error.as_deref(),
        Some("The command exited with code unknown")
    );
}

#[gpui::test]
fn a_failed_command_names_the_end_of_its_output(cx: &mut TestAppContext) {
    let rig = Rig::new(&[]);
    let stdout = format!("{}TAIL", "o".repeat(900));
    rig.fake
        .command_returns(command_result(Some(2), &stdout, "  \n"));
    rig.start(cx, &command_workflow(json!({})));
    cx.run_until_parked();
    let error = node(&rig.last_run(), "cmd").error.clone().unwrap();
    // stderr is blank, so stdout speaks, cut to its last 500 characters.
    assert!(
        error.starts_with("The command exited with code 2: ooo") && error.ends_with("oTAIL"),
        "{error}"
    );
    assert_eq!(error.len(), "The command exited with code 2: ".len() + 500);
}

#[gpui::test]
fn a_command_that_times_out_fails_its_node(cx: &mut TestAppContext) {
    let rig = Rig::new(&[]);
    let mut result = command_result(None, "", "");
    result.timed_out = true;
    rig.fake.command_returns(result);
    rig.start(cx, &command_workflow(json!({"timeoutMinutes": 2})));
    cx.run_until_parked();
    assert_eq!(rig.fake.world().commands[0].2, 2 * 60_000);
    assert_eq!(
        node(&rig.last_run(), "cmd").error.as_deref(),
        Some("The command timed out after 2 minutes")
    );
}

#[gpui::test]
fn a_command_cancelled_from_outside_fails_its_node(cx: &mut TestAppContext) {
    let rig = Rig::new(&[]);
    let mut result = command_result(None, "", "");
    result.cancelled = true;
    rig.fake.command_returns(result);
    rig.start(cx, &command_workflow(json!({})));
    cx.run_until_parked();
    let run = rig.last_run();
    assert_eq!(run.status, RunStatus::Failed);
    assert!(
        node(&run, "cmd")
            .error
            .as_deref()
            .unwrap()
            .contains("cancelled")
    );
}

#[gpui::test]
fn a_command_the_host_refuses_fails_with_the_hosts_message(cx: &mut TestAppContext) {
    let rig = Rig::new(&[]);
    rig.fake
        .command_fails("Command nodes are not supported on Windows yet");
    rig.start(cx, &command_workflow(json!({})));
    cx.run_until_parked();
    assert!(
        node(&rig.last_run(), "cmd")
            .error
            .as_deref()
            .unwrap()
            .contains("Command nodes are not supported on Windows yet")
    );
}

// --- stop -----------------------------------------------------------------------------------

#[gpui::test]
fn stop_stops_running_sessions_ends_stopped_and_reports_once(cx: &mut TestAppContext) {
    let rig = Rig::new(&["ok"]);
    rig.fake.hold();
    rig.start(cx, &simple(json!({})));
    cx.run_until_parked();
    assert_eq!(
        node(&rig.last_run(), "dev").session_id.as_deref(),
        Some("s1")
    );

    assert!(rig.runner.stop(WF));
    assert!(!rig.runner.stop(WF), "a second stop changes nothing");
    cx.run_until_parked();
    let run = rig.last_run();
    assert_eq!(run.status, RunStatus::Stopped);
    assert_eq!(node(&run, "dev").status, NodeStatus::Stopped);
    let w = rig.fake.world();
    assert_eq!(w.stops, ["s1"]);
    assert!(w.ended.contains(&"s1".to_string()));
    assert_eq!(w.posts.len(), 1, "a stopped run reports once");
    assert_eq!(w.posts[0].title, "Ship it · stopped");
    assert!(w.posts[0].text.contains("stopped"));
    drop(w);
    assert!(!rig.signals.is_running(WF));
    assert!(run.report_posted_at.is_some());
}

#[gpui::test]
fn stop_cancels_a_running_command_and_the_run_is_released_after_it_settles(
    cx: &mut TestAppContext,
) {
    let rig = Rig::new(&[]);
    let gate = rig.fake.command_gate();
    let mut result = command_result(None, "partial", "");
    result.cancelled = true;
    rig.fake.command_returns(result);
    rig.start(cx, &command_workflow(json!({})));
    cx.run_until_parked();
    let id = rig.fake.world().commands[0].3.clone();

    rig.runner.stop(WF);
    cx.run_until_parked();
    assert!(rig.fake.world().cancelled.contains(&id));
    let run = rig.last_run();
    assert_eq!(run.status, RunStatus::Stopped);
    assert_eq!(node(&run, "cmd").status, NodeStatus::Stopped);
    // The command is still alive: another run of the workflow must not start yet.
    minutes(cx, 1);
    assert!(rig.runner.is_running(WF));
    assert!(rig.signals.is_running(WF));

    gate.close();
    cx.run_until_parked();
    assert!(!rig.runner.is_running(WF));
    assert_eq!(rig.fake.world().posts.len(), 1);
    assert_eq!(rig.last_run().status, RunStatus::Stopped);
}

#[gpui::test]
fn stop_retries_a_cancel_that_beat_the_command_starting(cx: &mut TestAppContext) {
    let rig = Rig::new(&[]);
    let gate = rig.fake.command_gate();
    rig.fake.command_returns(command_result(None, "", ""));
    rig.fake.cancel_refuses(3);
    rig.start(cx, &command_workflow(json!({})));
    cx.run_until_parked();
    rig.runner.stop(WF);
    cx.run_until_parked();
    minutes(cx, 1);
    // Three misses, then it took (the run asks once more when it is over).
    assert!(rig.fake.world().cancelled.len() >= 4);
    gate.close();
    cx.run_until_parked();
}

#[gpui::test]
fn stop_does_not_wait_for_a_pending_ask_mono_question(cx: &mut TestAppContext) {
    let rig = Rig::new(&[]);
    rig.fake.hang_next_ask();
    rig.start(cx, &ask_workflow());
    cx.run_until_parked();
    assert_eq!(rig.fake.world().asks.len(), 1);
    rig.runner.stop(WF);
    cx.run_until_parked();
    assert_eq!(rig.last_run().status, RunStatus::Stopped);
    assert!(!rig.runner.is_running(WF));
}

#[gpui::test]
fn stop_does_not_wait_for_a_session_that_never_finishes_starting(cx: &mut TestAppContext) {
    let rig = Rig::new(&["ok"]);
    let gate = rig.fake.start_gate();
    rig.start(cx, &simple(json!({})));
    cx.run_until_parked();
    assert_eq!(rig.fake.world().starts.len(), 1);
    rig.runner.stop(WF);
    cx.run_until_parked();
    assert_eq!(rig.last_run().status, RunStatus::Stopped);
    assert!(!rig.runner.is_running(WF));
    assert!(rig.fake.world().stops.is_empty());
    // A start that finally comes through after the stop is still ended.
    gate.close();
    cx.run_until_parked();
    assert_eq!(rig.fake.world().stops, ["s1"]);
    // And the host lets go of it, so it keeps no watch, wake-up or id for a run that is over.
    assert_eq!(rig.fake.world().released, ["s1"]);
    assert_eq!(rig.fake.world().ended, ["s1"]);
}

#[gpui::test]
fn starts_queued_behind_another_are_not_launched_once_the_run_is_stopped(cx: &mut TestAppContext) {
    let def = workflow(
        vec![
            trigger(),
            session("fetch", json!({"tickets": {"type": "list"}}), json!({})),
            json!({"id": "each", "type": "forEach", "in": "{{fetch.tickets}}", "as": "item"}),
            session("work", json!({}), json!({"parent": "each"})),
            report(),
        ],
        vec![
            edge("trigger", "fetch"),
            edge("fetch", "each"),
            edge("each", "report"),
        ],
    );
    let rig = Rig::new(&[&fenced(json!({"tickets": ["a", "b"]}))]);
    // The first branch's start is slow, so the second one waits in the queue behind it.
    let gate = rig.fake.start_gate_for(2);
    rig.start(cx, &def);
    cx.run_until_parked();
    assert_eq!(rig.fake.world().starts.len(), 2);

    rig.runner.stop(WF);
    cx.run_until_parked();
    assert_eq!(rig.last_run().status, RunStatus::Stopped);
    gate.close();
    cx.run_until_parked();
    minutes(cx, 1);
    let w = rig.fake.world();
    // fetch and the first branch only; the first branch's session is stopped as it appears.
    assert_eq!(w.starts.len(), 2);
    assert_eq!(w.stops, ["s2"]);
    drop(w);
    assert!(!rig.runner.is_running(WF));
}

#[gpui::test]
fn stop_goes_on_when_a_session_cannot_be_stopped(cx: &mut TestAppContext) {
    let rig = Rig::new(&["ok"]);
    rig.fake.hold();
    rig.fake.stop_fails();
    rig.start(cx, &simple(json!({})));
    cx.run_until_parked();
    rig.runner.stop(WF);
    cx.run_until_parked();
    assert_eq!(rig.last_run().status, RunStatus::Stopped);
    let w = rig.fake.world();
    assert_eq!(w.posts.len(), 1);
    assert!(
        w.ended.contains(&"s1".to_string()),
        "its approvals are withdrawn all the same"
    );
}

#[gpui::test]
fn stopping_a_run_that_is_over_does_nothing(cx: &mut TestAppContext) {
    let rig = Rig::new(&["ok"]);
    rig.start(cx, &simple(json!({})));
    cx.run_until_parked();
    assert!(!rig.runner.stop(WF));
    assert!(!rig.runner.stop("wf-unknown1"));
    assert_eq!(rig.last_run().status, RunStatus::Done);
}

#[gpui::test]
fn stopping_a_run_that_is_already_over_does_not_hold_it_open(cx: &mut TestAppContext) {
    let def = simple(json!({}));
    let mut run = start_run(&def, RunTrigger::Manual, 1_000, "run-1");
    run.status = RunStatus::Failed;
    run.ended_at = Some(2_000);
    let rig = Rig::new(&[]);
    rig.start_run(cx, run);
    // Before the first save has even run.
    assert!(!rig.runner.stop(WF));
    cx.run_until_parked();
    assert!(!rig.runner.is_running(WF));
    assert!(!rig.signals.is_running(WF));
}

// --- persistence ----------------------------------------------------------------------------

#[gpui::test]
fn it_keeps_going_in_memory_when_the_run_cannot_be_saved(cx: &mut TestAppContext) {
    let rig = Rig::new(&["ok"]);
    rig.fake.fail_saves("file too large");
    rig.start(cx, &simple(json!({})));
    cx.run_until_parked();
    let w = rig.fake.world();
    assert_eq!(w.posts.len(), 1);
    // The report mentions that the history could not be written.
    assert!(w.posts[0].text.contains("Run error"), "{}", w.posts[0].text);
    assert!(
        w.posts[0]
            .text
            .contains("Could not save the run history: file too large")
    );
    drop(w);
    assert!(!rig.runner.is_running(WF));
}

#[gpui::test]
fn saves_fold_into_the_newest_state_and_never_overlap(cx: &mut TestAppContext) {
    let rig = Rig::new(&["ok"]);
    rig.fake.slow_saves(20);
    rig.start(cx, &simple(json!({})));
    for _ in 0..40 {
        cx.run_until_parked();
        cx.executor().advance_clock(Duration::from_millis(10));
    }
    assert!(!rig.runner.is_running(WF));
    let w = rig.fake.world();
    assert_eq!(w.max_inflight, 1);
    // Start, node started, then everything that happened while that write was going.
    assert!(w.saves.len() > 1 && w.saves.len() <= 4, "{}", w.saves.len());
    let last = w.saves.last().unwrap();
    assert_eq!(last.status, RunStatus::Done);
    assert_eq!(node(last, "dev").status, NodeStatus::Done);
    assert_eq!(node(last, "dev").session_id.as_deref(), Some("s1"));
}

#[gpui::test]
fn a_run_that_cannot_make_progress_fails_saying_so(cx: &mut TestAppContext) {
    use monocode_core::mono::workflows::engine::{EngineEvent, apply_event};
    let def = simple(json!({}));
    let started = start_run(&def, RunTrigger::Manual, 1_000, "run-1");
    // A run left `running` with its node running and no task behind it.
    let stuck = apply_event(
        &started,
        &EngineEvent::NodeStarted {
            node_id: "dev".into(),
            branch: String::new(),
            session_id: None,
            at: 2_000,
        },
    );
    assert_eq!(node(&stuck, "dev").status, NodeStatus::Running);
    let rig = Rig::new(&[]);
    assert!(rig.start_run(cx, stuck));
    cx.run_until_parked();
    let run = rig.last_run();
    assert_eq!(run.status, RunStatus::Failed);
    assert_eq!(
        run.error.as_deref(),
        Some("The run could not make progress.")
    );
    assert!(run.ended_at.is_some());
    assert!(!rig.runner.is_running(WF));
}

#[gpui::test]
fn a_run_that_is_already_over_is_only_written(cx: &mut TestAppContext) {
    let def = simple(json!({}));
    let mut run = start_run(&def, RunTrigger::Manual, 1_000, "run-1");
    run.status = RunStatus::Failed;
    run.ended_at = Some(2_000);
    run.error = Some("The workflow is not valid: nope".into());
    let rig = Rig::new(&[]);
    rig.start_run(cx, run);
    cx.run_until_parked();
    assert_eq!(
        rig.last_run().error.as_deref(),
        Some("The workflow is not valid: nope")
    );
    let w = rig.fake.world();
    assert!(w.starts.is_empty() && w.posts.is_empty());
    drop(w);
    assert!(!rig.runner.is_running(WF));
}

// --- resume ---------------------------------------------------------------------------------

#[gpui::test]
fn it_resumes_an_interrupted_run_from_where_it_stopped(cx: &mut TestAppContext) {
    use monocode_core::mono::workflows::engine::mark_interrupted;
    let def = simple(json!({}));
    let interrupted = mark_interrupted(&start_run(&def, RunTrigger::Manual, 1_000, "run-old"));
    assert_eq!(interrupted.status, RunStatus::Interrupted);
    let rig = Rig::new(&["ok"]);
    let host: Rc<dyn WorkflowHost> = Rc::new(rig.fake.clone());
    assert!(
        rig.runner
            .resume(host, &cx.to_async(), "mono-1", &interrupted)
    );
    cx.run_until_parked();
    let run = rig.last_run();
    assert_eq!((run.id.as_str(), run.status), ("run-old", RunStatus::Done));
    let w = rig.fake.world();
    assert_eq!(w.starts.len(), 1);
    assert_eq!(w.posts.len(), 1);
}

#[gpui::test]
fn only_an_interrupted_run_can_be_resumed(cx: &mut TestAppContext) {
    let def = simple(json!({}));
    let running = start_run(&def, RunTrigger::Manual, 1_000, "run-1");
    let rig = Rig::new(&[]);
    let host: Rc<dyn WorkflowHost> = Rc::new(rig.fake.clone());
    assert!(!rig.runner.resume(host, &cx.to_async(), "mono-1", &running));
    cx.run_until_parked();
    assert!(rig.fake.world().saves.is_empty());
}

#[gpui::test]
fn a_resumed_loop_node_continues_its_session_with_a_follow_up(cx: &mut TestAppContext) {
    use monocode_core::mono::workflows::engine::{EngineEvent, apply_event, mark_interrupted};
    let def = workflow(
        vec![
            trigger(),
            session("dev", json!({"summary": {"type": "string"}}), json!({})),
            session("review", json!({"verdict": {"type": "string"}}), json!({})),
            report(),
        ],
        vec![
            edge("trigger", "dev"),
            edge("dev", "review"),
            json!({"from": "review", "to": "dev", "loop": {"maxRounds": 3, "decide": "{{review.verdict}} != \"approve\""}}),
            edge("review", "report"),
        ],
    );
    // Round 1 happened; round 2's dev was running in session s9 when the app closed.
    let mut run = start_run(&def, RunTrigger::Manual, 1_000, "run-old");
    let at = 2_000;
    let step = |run: &WorkflowRun, event| apply_event(run, &event);
    run = step(
        &run,
        EngineEvent::NodeStarted {
            node_id: "dev".into(),
            branch: String::new(),
            session_id: None,
            at,
        },
    );
    run = step(
        &run,
        EngineEvent::NodeFinished {
            node_id: "dev".into(),
            branch: String::new(),
            output: out(json!({"summary": "v1"})),
            at,
        },
    );
    run = step(
        &run,
        EngineEvent::NodeStarted {
            node_id: "review".into(),
            branch: String::new(),
            session_id: None,
            at,
        },
    );
    run = step(
        &run,
        EngineEvent::NodeFinished {
            node_id: "review".into(),
            branch: String::new(),
            output: out(json!({"verdict": "changes"})),
            at,
        },
    );
    run = step(
        &run,
        EngineEvent::NodeStarted {
            node_id: "dev".into(),
            branch: String::new(),
            session_id: Some("s9".into()),
            at,
        },
    );
    run.nodes
        .iter_mut()
        .find(|n| n.node_id == "dev")
        .unwrap()
        .session_id = Some("s9".into());
    let interrupted = mark_interrupted(&run);
    assert_eq!(
        node(&interrupted, "dev").reuse_session_id.as_deref(),
        Some("s9")
    );

    let rig = Rig::new(&[
        &fenced(json!({"summary": "v2"})),
        &fenced(json!({"verdict": "approve"})),
    ]);
    let host: Rc<dyn WorkflowHost> = Rc::new(rig.fake.clone());
    assert!(
        rig.runner
            .resume(host, &cx.to_async(), "mono-1", &interrupted)
    );
    cx.run_until_parked();
    let w = rig.fake.world();
    assert_eq!(
        w.sends[0].0, "s9",
        "the loop session is continued, not started again"
    );
    assert_eq!(w.starts.len(), 1, "only review starts a session of its own");
    drop(w);
    assert_eq!(rig.last_run().status, RunStatus::Done);
}

fn out(value: Value) -> OrderedMap<Json> {
    match Json::from(&value) {
        Json::Object(map) => map,
        _ => panic!("not an object"),
    }
}

fn command_result(code: Option<i32>, stdout: &str, stderr: &str) -> CommandResult {
    CommandResult {
        exit_code: code,
        stdout: stdout.into(),
        stderr: stderr.into(),
        timed_out: false,
        cancelled: false,
    }
}

// --- quitting, deleting a Mono, Telegram context ----------------------------------------------

#[gpui::test]
fn quitting_leaves_a_run_running_in_the_file_for_the_next_start_to_interrupt(
    cx: &mut TestAppContext,
) {
    let rig = Rig::new(&["ok"]);
    let hold = rig.fake.hold();
    rig.start(cx, &simple(json!({})));
    cx.run_until_parked();
    let saved = rig.fake.world().saves.len();
    rig.runner.quit();
    hold.close();
    cx.run_until_parked();
    // What the sessions report while the app goes down changes nothing: the file still says
    // `running`, no stop report was posted and nothing was stopped on the Mono's behalf.
    assert_eq!(rig.last_run().status, RunStatus::Running);
    assert_eq!(rig.fake.world().saves.len(), saved);
    assert!(rig.fake.world().posts.is_empty());
    assert!(rig.fake.world().stops.is_empty());
    assert!(!rig.runner.is_running(WF));
    assert!(!rig.signals.is_running(WF));
    // A second quit, and a stop after it, do nothing.
    rig.runner.quit();
    assert!(!rig.runner.stop(WF));
}

#[gpui::test]
fn a_command_that_dies_with_the_app_does_not_fail_its_node(cx: &mut TestAppContext) {
    let rig = Rig::new(&[]);
    let gate = rig.fake.command_gate();
    rig.fake.command_returns(command_result(None, "", "killed"));
    rig.start(cx, &command_workflow(json!({})));
    cx.run_until_parked();
    rig.runner.quit();
    gate.close();
    cx.run_until_parked();
    let run = rig.last_run();
    assert_eq!(run.status, RunStatus::Running);
    assert_ne!(node(&run, "cmd").status, NodeStatus::Failed);
    assert!(rig.fake.world().posts.is_empty());
}

#[gpui::test]
fn a_session_that_finishes_starting_after_the_quit_writes_nothing(cx: &mut TestAppContext) {
    let rig = Rig::new(&["ok"]);
    let gate = rig.fake.start_gate();
    rig.start(cx, &simple(json!({})));
    cx.run_until_parked();
    let saved = rig.fake.world().saves.len();
    rig.runner.quit();
    gate.close();
    cx.run_until_parked();
    // The start came through (with its folder) after the app began to go down: not recorded.
    assert_eq!(rig.fake.world().saves.len(), saved);
    assert_eq!(rig.last_run().folder_id, None);
    assert_eq!(rig.last_run().status, RunStatus::Running);
}

#[gpui::test]
fn a_session_that_fails_to_start_during_the_quit_does_not_fail_the_node(cx: &mut TestAppContext) {
    let rig = Rig::new(&[]);
    let gate = rig.fake.start_gate();
    rig.fake.fail_starts("The app is closing");
    rig.start(cx, &simple(json!({})));
    cx.run_until_parked();
    rig.runner.quit();
    gate.close();
    cx.run_until_parked();
    let run = rig.last_run();
    assert_eq!(run.status, RunStatus::Running);
    assert_ne!(node(&run, "dev").status, NodeStatus::Failed);
}

#[gpui::test]
fn nothing_starts_or_resumes_once_the_app_is_quitting(cx: &mut TestAppContext) {
    let rig = Rig::new(&["ok"]);
    rig.runner.quit();
    assert!(!rig.start(cx, &simple(json!({}))));
    let interrupted = monocode_core::mono::workflows::engine::mark_interrupted(&start_run(
        &simple(json!({})),
        RunTrigger::Manual,
        1_000,
        "run-old",
    ));
    assert!(!rig.runner.resume(
        Rc::new(rig.fake.clone()),
        &cx.to_async(),
        "mono-1",
        &interrupted
    ));
    cx.run_until_parked();
    assert!(rig.fake.world().saves.is_empty(), "not even a record");
    assert!(!rig.signals.is_running(WF));
}

#[gpui::test]
fn quitting_stops_waiting_on_a_question_to_the_mono(cx: &mut TestAppContext) {
    let rig = Rig::new(&[]);
    rig.fake.hang_next_ask();
    rig.start(cx, &ask_workflow());
    cx.run_until_parked();
    rig.runner.quit();
    cx.run_until_parked();
    assert!(!rig.runner.is_running(WF));
    assert_eq!(rig.last_run().status, RunStatus::Running);
}

fn start_for(rig: &Rig, cx: &TestAppContext, mono: &str, workflow_id: &str) {
    let mut def = simple(json!({}));
    def.id = workflow_id.into();
    let run = start_run(
        &def,
        RunTrigger::Manual,
        1_000,
        &format!("run-{workflow_id}"),
    );
    assert!(
        rig.runner
            .start(Rc::new(rig.fake.clone()), &cx.to_async(), mono, run)
    );
}

#[gpui::test]
fn stopping_a_monos_runs_leaves_the_runs_of_other_monos_going(cx: &mut TestAppContext) {
    let rig = Rig::new(&["ok", "ok"]);
    let hold = rig.fake.hold();
    start_for(&rig, cx, "mono-1", "wf-first000");
    start_for(&rig, cx, "mono-2", "wf-second00");
    cx.run_until_parked();
    assert_eq!(rig.runner.running().len(), 2);
    rig.runner.stop_mono("mono-2");
    cx.run_until_parked();
    assert!(rig.runner.is_running("wf-first000"));
    assert!(!rig.runner.is_running("wf-second00"));
    let stopped = rig
        .fake
        .world()
        .saves
        .iter()
        .filter(|r| r.status == RunStatus::Stopped)
        .map(|r| r.workflow_id.clone())
        .collect::<Vec<_>>();
    assert!(stopped.iter().all(|id| id == "wf-second00") && !stopped.is_empty());
    assert_eq!(rig.runner.stop_mono("mono-none"), 0);
    hold.close();
    cx.run_until_parked();
    assert!(!rig.runner.is_running("wf-first000"));
}

#[gpui::test]
fn a_session_knows_its_workflow_and_node_while_the_run_lasts(cx: &mut TestAppContext) {
    let rig = Rig::new(&["ok"]);
    let hold = rig.fake.hold();
    let mut def = simple(json!({}));
    def.nodes.iter_mut().find(|n| n.id == "dev").unwrap().title = Some("Build it".into());
    rig.start(cx, &def);
    cx.run_until_parked();
    let context = rig
        .runner
        .context("s1")
        .expect("known once the session id is");
    assert_eq!(context.workflow_name, "Ship it");
    assert_eq!(context.node_title, "Build it");
    assert_eq!((context.item_id, context.round), (None, None));
    assert!(rig.runner.context("s2").is_none());
    hold.close();
    cx.run_until_parked();
    assert_eq!(rig.last_run().status, RunStatus::Done);
    assert!(rig.runner.context("s1").is_none(), "gone with the run");
}

#[gpui::test]
fn a_loop_item_and_round_reach_the_context_of_its_session(cx: &mut TestAppContext) {
    let def = workflow(
        vec![
            trigger(),
            session("fetch", json!({"tickets": {"type": "list"}}), json!({})),
            json!({"id": "each", "type": "forEach", "in": "{{fetch.tickets}}", "as": "item"}),
            session("work", json!({}), json!({"parent": "each"})),
            report(),
        ],
        vec![
            edge("trigger", "fetch"),
            edge("fetch", "each"),
            edge("each", "report"),
        ],
    );
    let rig = Rig::new(&[
        &fenced(json!({"tickets": [{"key": "A-1", "title": "One"}]})),
        "done",
    ]);
    // Hold the session that works the item, not the one that fetched it.
    let gate = rig.fake.start_gate_for(2);
    rig.start(cx, &def);
    cx.run_until_parked();
    assert!(rig.runner.context("s1").is_some());
    gate.close();
    let hold = rig.fake.hold();
    cx.run_until_parked();
    let context = rig.runner.context("s2").expect("the item session");
    assert_eq!(context.node_title, "work");
    assert_eq!(context.item_id.as_deref(), Some("A-1"));
    assert_eq!(context.item_label.as_deref(), Some("One"));
    hold.close();
    cx.run_until_parked();
    assert!(rig.runner.context("s2").is_none());
}
