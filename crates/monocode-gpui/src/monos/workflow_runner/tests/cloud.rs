//! Cloud steps (a session node with a `target`) against the fake host: the instance each step
//! asks for, what a stop leaves alone, and what the report says about the instances.
use super::*;
use monocode_core::mono::workflows::engine::{EngineEvent, apply_event};
use serde_json::Map;

fn cloud_request(kind: &str, instance: CloudInstance, options: Value) -> CloudRequest {
    CloudRequest {
        kind: kind.into(),
        instance,
        options: match options {
            Value::Object(map) => map,
            _ => Map::new(),
        },
    }
}

fn target_of(w: &fake::World, n: usize) -> Option<CloudRequest> {
    w.starts[n].target.clone()
}

/// trigger -> build (a new codespace on `trunk`) -> check (the same codespace) -> report
fn build_then_check() -> Workflow {
    workflow(
        vec![
            trigger(),
            session(
                "build",
                json!({}),
                json!({"target": {"kind": "codespace", "branch": "trunk"}}),
            ),
            session(
                "check",
                json!({}),
                json!({"target": {"kind": "codespace", "instance": {"sameAs": "build"}}}),
            ),
            report(),
        ],
        vec![
            edge("trigger", "build"),
            edge("build", "check"),
            edge("check", "report"),
        ],
    )
}

/// The same shape as `workflow`, without the validation, for a run that must cope with what a
/// definition should never hold.
fn unchecked(nodes: Vec<Value>, edges: Vec<Value>) -> Workflow {
    let def = json!({
        "id": WF, "name": "Ship it",
        "inputs": {"repo": {"type": "project"}},
        "inputValues": {"repo": "/code/app"},
        "nodes": nodes, "edges": edges, "enabled": true, "createdAt": 0,
    });
    parse_workflow(&Json::from(&def)).expect("a workflow")
}

/// dev (a cloud step, `reuse` as given) -> review, with a loop back from review to dev.
fn cloud_loop(target: Value, reuse: Option<bool>) -> Workflow {
    let mut extra = json!({ "target": target });
    if let Some(reuse) = reuse {
        extra["reuseSession"] = json!(reuse);
    }
    workflow(
        vec![
            trigger(),
            session("dev", json!({"summary": {"type": "string"}}), extra),
            session("review", json!({"verdict": {"type": "string"}}), json!({})),
            report(),
        ],
        vec![
            edge("trigger", "dev"),
            edge("dev", "review"),
            json!({"from": "review", "to": "dev", "loop": {"maxRounds": 3, "decide": "{{review.verdict}} != \"approve\""}}),
            edge("review", "report"),
        ],
    )
}

fn loop_replies() -> Rig {
    Rig::new(&[
        &fenced(json!({"summary": "v1"})),
        &fenced(json!({"verdict": "changes"})),
        &fenced(json!({"summary": "v2"})),
        &fenced(json!({"verdict": "approve"})),
    ])
}

#[gpui::test]
fn a_new_instance_is_asked_for_once_and_same_as_goes_on_in_it(cx: &mut TestAppContext) {
    let rig = Rig::new(&["one", "two"]);
    assert!(rig.start(cx, &build_then_check()));
    cx.run_until_parked();
    assert_eq!(rig.last_run().status, RunStatus::Done);
    let w = rig.fake.world();
    assert_eq!(w.starts.len(), 2);
    assert_eq!(
        target_of(&w, 0),
        Some(cloud_request(
            "codespace",
            CloudInstance::New,
            json!({"branch": "trunk"})
        ))
    );
    // The second step is a session of its own on the instance of the first, with no options:
    // they only apply when an instance is created.
    assert_eq!(
        target_of(&w, 1),
        Some(cloud_request(
            "codespace",
            CloudInstance::OfSession("s1".into()),
            json!({})
        ))
    );
    assert!(!w.starts[0].new_worktree && w.starts[0].worktree_cwd.is_none());
    assert!(
        w.sends.is_empty(),
        "no session is continued, two are started"
    );
}

#[gpui::test]
fn a_step_on_this_computer_asks_for_no_target(cx: &mut TestAppContext) {
    let rig = Rig::new(&["ok"]);
    rig.start(cx, &simple(json!({})));
    cx.run_until_parked();
    let w = rig.fake.world();
    assert_eq!(target_of(&w, 0), None);
    assert!(!w.posts[0].text.contains("Cloud instances"));
    assert!(!w.posts[0].text.contains("Cloud workspaces"));
}

#[gpui::test]
fn without_reuse_a_later_round_goes_on_in_the_instance_of_the_round_before(
    cx: &mut TestAppContext,
) {
    let def = cloud_loop(json!({"kind": "sandbox", "instance": "new"}), Some(false));
    let rig = loop_replies();
    rig.start(cx, &def);
    cx.run_until_parked();
    assert_eq!(rig.last_run().status, RunStatus::Done);
    let w = rig.fake.world();
    // dev, review, then dev again in a session of its own.
    assert_eq!(w.starts.len(), 3);
    assert_eq!(
        target_of(&w, 0),
        Some(cloud_request("sandbox", CloudInstance::New, json!({})))
    );
    assert_eq!(target_of(&w, 1), None);
    assert_eq!(
        target_of(&w, 2),
        Some(cloud_request(
            "sandbox",
            CloudInstance::OfSession("s1".into()),
            json!({})
        ))
    );
    // One instance, made by round 1.
    let text = &w.posts[0].text;
    assert_eq!(text.matches("sandbox-1").count(), 1, "{text}");
    assert!(text.contains("created by this run; step dev"), "{text}");
    assert!(!text.contains("sandbox-2"), "{text}");
}

#[gpui::test]
fn with_reuse_a_later_round_continues_the_same_session(cx: &mut TestAppContext) {
    let def = cloud_loop(json!({"kind": "codespace", "instance": "new"}), None);
    let rig = loop_replies();
    rig.start(cx, &def);
    cx.run_until_parked();
    let w = rig.fake.world();
    assert_eq!(
        w.starts.len(),
        2,
        "round 2 of dev is a follow-up, not a start"
    );
    assert_eq!(w.sends[0].0, "s1");
    assert_eq!(w.posts[0].text.matches("codespace-1").count(), 1);
}

#[gpui::test]
fn each_item_of_a_for_each_gets_an_instance_of_its_own(cx: &mut TestAppContext) {
    let def = workflow(
        vec![
            trigger(),
            session("fetch", json!({"tickets": {"type": "list"}}), json!({})),
            json!({"id": "each", "type": "forEach", "in": "{{fetch.tickets}}", "as": "item", "maxParallel": 1}),
            session(
                "work",
                json!({}),
                json!({"parent": "each", "target": {"kind": "codespace", "instance": "new"}}),
            ),
            report(),
        ],
        vec![
            edge("trigger", "fetch"),
            edge("fetch", "each"),
            edge("each", "report"),
        ],
    );
    let rig = Rig::new(&[
        &fenced(
            json!({"tickets": [{"key": "A-1", "title": "One"}, {"key": "A-2", "title": "Two"}]}),
        ),
        "done",
        "done",
    ]);
    rig.start(cx, &def);
    cx.run_until_parked();
    assert_eq!(rig.last_run().status, RunStatus::Done);
    let w = rig.fake.world();
    assert_eq!(w.starts.len(), 3);
    assert_eq!(target_of(&w, 1).unwrap().instance, CloudInstance::New);
    assert_eq!(target_of(&w, 2).unwrap().instance, CloudInstance::New);
    let text = &w.posts[0].text;
    assert!(
        text.contains("codespace-1") && text.contains("codespace-2"),
        "{text}"
    );
}

#[gpui::test]
fn a_same_as_step_fails_when_its_source_has_no_session(cx: &mut TestAppContext) {
    let def = build_then_check();
    // `build` finished without ever having had a session.
    let mut run = start_run(&def, RunTrigger::Manual, 1_000, "run-1");
    for event in [
        EngineEvent::NodeStarted {
            node_id: "build".into(),
            branch: String::new(),
            session_id: None,
            at: 2_000,
        },
        EngineEvent::NodeFinished {
            node_id: "build".into(),
            branch: String::new(),
            output: out(json!({})),
            at: 2_000,
        },
    ] {
        run = apply_event(&run, &event);
    }
    let rig = Rig::new(&[]);
    assert!(rig.start_run(cx, run));
    cx.run_until_parked();
    let run = rig.last_run();
    let check = node(&run, "check");
    assert_eq!(check.status, NodeStatus::Failed);
    assert_eq!(
        check.error.as_deref(),
        Some("No cloud instance is available from \"build\"")
    );
    assert!(rig.fake.world().starts.is_empty(), "nothing was created");
    assert!(!rig.fake.world().posts[0].text.contains("Cloud instances"));
}

#[gpui::test]
fn a_cloud_step_that_slipped_past_validation_fails_and_creates_nothing(cx: &mut TestAppContext) {
    let cases = [
        (
            json!({"kind": "vm"}),
            "root",
            "\"vm\" is not a cloud kind a run can start",
        ),
        (
            json!({"kind": "codespace", "instance": "my-codespace"}),
            "root",
            "The cloud instance of this step is not valid",
        ),
        (
            json!({"kind": "codespace"}),
            "newWorktree",
            "its checkout must be root",
        ),
    ];
    for (target, checkout, message) in cases {
        let def = unchecked(
            vec![
                trigger(),
                session(
                    "dev",
                    json!({}),
                    json!({"target": target, "checkout": checkout}),
                ),
                report(),
            ],
            vec![edge("trigger", "dev"), edge("dev", "report")],
        );
        let rig = Rig::new(&[]);
        assert!(rig.start(cx, &def));
        cx.run_until_parked();
        let run = rig.last_run();
        let dev = node(&run, "dev");
        assert_eq!(dev.status, NodeStatus::Failed, "{message}");
        assert!(
            dev.error.as_deref().unwrap().contains(message),
            "{:?}",
            dev.error
        );
        assert!(rig.fake.world().starts.is_empty(), "{message}");
    }
}

#[gpui::test]
fn stopping_while_the_instance_starts_launches_nothing_later(cx: &mut TestAppContext) {
    let rig = Rig::new(&["ok", "ok"]);
    let gate = rig.fake.start_gate();
    rig.start(cx, &build_then_check());
    cx.run_until_parked();
    assert_eq!(rig.fake.world().starts.len(), 1);
    assert!(rig.runner.stop(WF));
    cx.run_until_parked();
    assert_eq!(rig.last_run().status, RunStatus::Stopped);
    // The start returns after the stop: its session is ended, nothing is sent, and the second
    // step never starts.
    gate.close();
    cx.run_until_parked();
    let w = rig.fake.world();
    assert_eq!(w.starts.len(), 1);
    assert!(w.sends.is_empty());
    // Only the session is stopped, as for any step: the host has no call that stops or deletes
    // an instance.
    assert_eq!(w.stops, ["s1"]);
    assert_eq!(w.released, ["s1"]);
    // The report was out before the instance existed, so a follow-up names it.
    assert_eq!(w.posts.len(), 2);
    assert!(!w.posts[0].text.contains("codespace-1"));
    assert!(w.posts[1].title.ends_with("cloud instance left running"));
    // The chat shows only the text, so it names the workflow itself.
    let name = &w.posts[1].workflow_name;
    assert!(
        w.posts[1].text.starts_with(&format!("**{name}**\n\n")),
        "{}",
        w.posts[1].text
    );
    assert!(
        w.posts[1]
            .text
            .contains("Codespace codespace-1: created by this run; step build"),
        "{}",
        w.posts[1].text
    );
    drop(w);
    // And the run record keeps the session, so the instance is not lost to the history.
    assert_eq!(
        node(&rig.last_run(), "build")
            .interrupted_sessions
            .as_deref(),
        Some(&["s1".to_string()][..])
    );
}

#[gpui::test]
fn a_stopped_run_lists_the_instances_it_used_and_leaves_them_alone(cx: &mut TestAppContext) {
    let rig = Rig::new(&["one"]);
    rig.fake.hold();
    rig.start(cx, &build_then_check());
    cx.run_until_parked();
    assert_eq!(
        node(&rig.last_run(), "build").session_id.as_deref(),
        Some("s1")
    );
    assert!(rig.runner.stop(WF));
    cx.run_until_parked();
    let w = rig.fake.world();
    assert_eq!(w.stops, ["s1"]);
    assert_eq!(w.posts.len(), 1);
    let text = &w.posts[0].text;
    assert!(
        text.contains("Codespace codespace-1: created by this run; step build"),
        "{text}"
    );
    assert!(text.contains("never stops or deletes"), "{text}");
    assert!(
        text.contains("Stop or delete them from Cloud workspaces"),
        "{text}"
    );
}

#[gpui::test]
fn every_ending_lists_each_instance_once_with_its_steps(cx: &mut TestAppContext) {
    let rig = Rig::new(&["one", "two"]);
    rig.start(cx, &build_then_check());
    cx.run_until_parked();
    assert_eq!(rig.last_run().status, RunStatus::Done);
    let w = rig.fake.world();
    let text = &w.posts[0].text;
    assert_eq!(text.matches("Codespace codespace-1").count(), 1, "{text}");
    assert!(
        text.contains("Codespace codespace-1: created by this run; steps build, check"),
        "{text}"
    );
    assert!(!text.contains("codespace-2"));
}

#[gpui::test]
fn a_failed_cloud_step_still_lists_its_instance(cx: &mut TestAppContext) {
    let rig = Rig::new(&[]);
    rig.fake
        .end_turns_with(Outcome::failed("The agent crashed"));
    rig.start(cx, &build_then_check());
    cx.run_until_parked();
    let run = rig.last_run();
    assert_eq!(run.status, RunStatus::Failed);
    assert_eq!(node(&run, "build").status, NodeStatus::Failed);
    // Its first prompt is dropped (a later Retry must not run the step outside the run); the
    // instance is not touched.
    assert_eq!(rig.fake.world().stops, ["s1"]);
    let text = rig.fake.world().posts[0].text.clone();
    assert!(
        text.contains("Codespace codespace-1: created by this run; step build"),
        "{text}"
    );
}

#[gpui::test]
fn a_timeout_stops_the_session_and_leaves_the_instance(cx: &mut TestAppContext) {
    let mut def = build_then_check();
    if let NodeKind::Session {
        timeout_minutes, ..
    } = &mut def.nodes.iter_mut().find(|n| n.id == "build").unwrap().kind
    {
        *timeout_minutes = Some(1.0);
    }
    let rig = Rig::new(&["one"]);
    rig.fake.hold();
    rig.start(cx, &def);
    cx.run_until_parked();
    minutes(cx, 61);
    let run = rig.last_run();
    assert_eq!(node(&run, "build").status, NodeStatus::Failed);
    assert!(
        node(&run, "build")
            .error
            .as_deref()
            .unwrap()
            .contains("took longer than 1 minutes")
    );
    // The timeout stops the turn, then the failed cloud node drops its prompt: two stops of
    // this session and nothing else.
    assert_eq!(rig.fake.world().stops, ["s1", "s1"]);
    assert!(rig.fake.world().posts[0].text.contains("codespace-1"));
}

#[gpui::test]
fn a_failed_step_on_this_computer_is_not_stopped(cx: &mut TestAppContext) {
    let rig = Rig::new(&[]);
    rig.fake
        .end_turns_with(Outcome::failed("The agent crashed"));
    rig.start(cx, &simple(json!({})));
    cx.run_until_parked();
    assert!(rig.fake.world().stops.is_empty());
}

#[gpui::test]
fn a_resumed_run_goes_on_in_the_instance_of_the_session_it_was_interrupted_in(
    cx: &mut TestAppContext,
) {
    use monocode_core::mono::workflows::engine::mark_interrupted;
    let def = build_then_check();
    // Round 1 of `build` was running in cloud session s9 when the app closed.
    let mut run = start_run(&def, RunTrigger::Manual, 1_000, "run-old");
    run = apply_event(
        &run,
        &EngineEvent::NodeStarted {
            node_id: "build".into(),
            branch: String::new(),
            session_id: Some("s9".into()),
            at: 2_000,
        },
    );
    run.nodes
        .iter_mut()
        .find(|n| n.node_id == "build")
        .unwrap()
        .session_id = Some("s9".into());
    let interrupted = mark_interrupted(&run);
    assert_eq!(
        node(&interrupted, "build").interrupted_sessions.as_deref(),
        Some(&["s9".to_string()][..])
    );

    let rig = Rig::new(&["one", "two"]);
    rig.fake.knows_cloud("s9", "codespace", "codespace-9");
    let host: Rc<dyn WorkflowHost> = Rc::new(rig.fake.clone());
    assert!(
        rig.runner
            .resume(host, &cx.to_async(), "mono-1", &interrupted)
    );
    cx.run_until_parked();
    assert_eq!(rig.last_run().status, RunStatus::Done);
    let w = rig.fake.world();
    // The step starts a session of its own on the earlier instance: no new instance. The old
    // session's first prompt is dropped, so it never runs next to the new one.
    assert_eq!(
        target_of(&w, 0),
        Some(cloud_request(
            "codespace",
            CloudInstance::OfSession("s9".into()),
            json!({})
        ))
    );
    assert_eq!(w.stops.first().map(String::as_str), Some("s9"));
    let text = &w.posts[0].text;
    assert_eq!(text.matches("codespace-9").count(), 1, "{text}");
    assert!(!text.contains("codespace-1"), "{text}");
    assert!(
        text.contains("Codespace codespace-9: created by this run; steps build, check"),
        "{text}"
    );
}
