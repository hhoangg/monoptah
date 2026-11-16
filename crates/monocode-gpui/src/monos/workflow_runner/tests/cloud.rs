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
        account: None,
    }
}

fn with_account(mut request: CloudRequest, account: &str) -> CloudRequest {
    request.account = Some(account.into());
    request
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

// --- templates in target options ------------------------------------------------------------

/// fetch (a local step that reports a branch) -> build (a cloud step with `target`, and the
/// node fields in `extra`) -> report.
fn branch_then_build(target: Value, extra: Value) -> Workflow {
    let mut build = json!({ "target": target });
    for (key, value) in extra.as_object().unwrap() {
        build[key] = value.clone();
    }
    workflow(
        vec![
            trigger(),
            session("fetch", json!({"branch": {"type": "string"}}), json!({})),
            session("build", json!({}), build),
            report(),
        ],
        vec![
            edge("trigger", "fetch"),
            edge("fetch", "build"),
            edge("build", "report"),
        ],
    )
}

#[gpui::test]
fn a_template_in_an_option_is_filled_in_from_the_run_before_the_start(cx: &mut TestAppContext) {
    let def = branch_then_build(
        json!({"kind": "codespace", "branch": "{{fetch.branch}}", "machine": "m-{{run.id}}"}),
        json!({}),
    );
    let rig = Rig::new(&[&fenced(json!({"branch": "feature-x"})), "built"]);
    assert!(rig.start(cx, &def));
    cx.run_until_parked();
    assert_eq!(rig.last_run().status, RunStatus::Done);
    let w = rig.fake.world();
    assert_eq!(w.starts.len(), 2, "fetch, then build");
    // The start request carries the rendered text, never the template.
    assert_eq!(
        target_of(&w, 1),
        Some(cloud_request(
            "codespace",
            CloudInstance::New,
            json!({"branch": "feature-x", "machine": "m-run-1"})
        ))
    );
    // The definition in the run record still holds the template.
    let run = rig.last_run();
    let NodeKind::Session {
        target: Some(target),
        ..
    } = &run
        .definition
        .nodes
        .iter()
        .find(|n| n.id == "build")
        .unwrap()
        .kind
    else {
        panic!("a cloud step")
    };
    assert_eq!(
        target.options.get("branch"),
        Some(&Json::String("{{fetch.branch}}".into()))
    );
}

#[gpui::test]
fn a_rendered_option_that_is_not_valid_fails_the_step_before_anything_is_started(
    cx: &mut TestAppContext,
) {
    for (target, branch, message) in [
        (
            json!({"kind": "codespace", "branch": "{{fetch.branch}}"}),
            "two words",
            "target.branch is not a valid branch name",
        ),
        (
            json!({"kind": "codespace", "machine": "{{fetch.branch}}"}),
            "not a machine",
            "target.machine is not a valid machine name",
        ),
        (
            json!({"kind": "sandbox", "memory": "{{fetch.branch}}"}),
            "lots",
            "target.memory",
        ),
    ] {
        let def = branch_then_build(target, json!({}));
        let rig = Rig::new(&[&fenced(json!({"branch": branch})), "never sent"]);
        assert!(rig.start(cx, &def));
        cx.run_until_parked();
        let run = rig.last_run();
        let build = node(&run, "build");
        assert_eq!(build.status, NodeStatus::Failed, "{message}");
        let error = build.error.as_deref().unwrap();
        assert!(
            error.starts_with("The cloud target is not valid: ") && error.contains(message),
            "{error}"
        );
        let w = rig.fake.world();
        // Only the local step started anything: no cloud start, no session, no stop.
        assert_eq!(w.starts.len(), 1, "{message}");
        assert!(w.starts.iter().all(|s| s.target.is_none()), "{message}");
        assert!(w.stops.is_empty() && w.sends.is_empty(), "{message}");
        assert!(
            !w.posts[0].text.contains("Cloud instances"),
            "nothing was created"
        );
    }
}

#[gpui::test]
fn a_template_in_an_option_that_cannot_hold_one_fails_the_step_and_creates_nothing(
    cx: &mut TestAppContext,
) {
    // Validation never lets these through: this is what a run does if one slips past.
    for target in [
        json!({"kind": "sandbox", "cpus": "{{run.id}}"}),
        json!({"kind": "sandbox", "kit": "{{run.id}}"}),
        json!({"kind": "codespace", "branch": ["{{run.id}}"]}),
    ] {
        let def = unchecked(
            vec![
                trigger(),
                session("dev", json!({}), json!({"target": target})),
                report(),
            ],
            vec![edge("trigger", "dev"), edge("dev", "report")],
        );
        let rig = Rig::new(&[]);
        assert!(rig.start(cx, &def));
        cx.run_until_parked();
        let run = rig.last_run();
        assert_eq!(node(&run, "dev").status, NodeStatus::Failed, "{target}");
        assert!(
            node(&run, "dev")
                .error
                .as_deref()
                .unwrap()
                .contains("cannot hold a template"),
            "{:?}",
            node(&run, "dev").error
        );
        assert!(rig.fake.world().starts.is_empty(), "{target}");
    }
}

#[gpui::test]
fn a_later_round_on_the_same_instance_does_not_fill_in_the_options_again(cx: &mut TestAppContext) {
    // Round 2 goes on in the instance round 1 made, so there is nothing to create and no option
    // to fill in or judge.
    let def = cloud_loop(
        json!({"kind": "codespace", "branch": "{{run.id}}"}),
        Some(false),
    );
    let rig = loop_replies();
    rig.start(cx, &def);
    cx.run_until_parked();
    assert_eq!(rig.last_run().status, RunStatus::Done);
    let w = rig.fake.world();
    assert_eq!(
        target_of(&w, 0),
        Some(cloud_request(
            "codespace",
            CloudInstance::New,
            json!({"branch": "run-1"})
        ))
    );
    assert_eq!(
        target_of(&w, 2),
        Some(cloud_request(
            "codespace",
            CloudInstance::OfSession("s1".into()),
            json!({})
        ))
    );
}

// --- the account of a cloud step ------------------------------------------------------------

#[gpui::test]
fn the_account_goes_to_the_start_of_a_new_instance_and_not_to_a_step_that_shares_one(
    cx: &mut TestAppContext,
) {
    let mut def = build_then_check();
    if let NodeKind::Session { account, .. } =
        &mut def.nodes.iter_mut().find(|n| n.id == "build").unwrap().kind
    {
        *account = Some("Work".into());
    }
    let rig = Rig::new(&["one", "two"]);
    rig.start(cx, &def);
    cx.run_until_parked();
    assert_eq!(rig.last_run().status, RunStatus::Done);
    let w = rig.fake.world();
    assert_eq!(
        target_of(&w, 0),
        Some(with_account(
            cloud_request("codespace", CloudInstance::New, json!({"branch": "trunk"})),
            "Work"
        ))
    );
    // The account belongs to the instance: a step that goes on in it names none.
    assert_eq!(target_of(&w, 1).unwrap().account, None);
}

#[gpui::test]
fn a_later_round_on_the_same_instance_takes_no_account(cx: &mut TestAppContext) {
    let mut def = cloud_loop(json!({"kind": "sandbox", "instance": "new"}), Some(false));
    if let NodeKind::Session { account, .. } =
        &mut def.nodes.iter_mut().find(|n| n.id == "dev").unwrap().kind
    {
        *account = Some("Work".into());
    }
    let rig = loop_replies();
    rig.start(cx, &def);
    cx.run_until_parked();
    assert_eq!(rig.last_run().status, RunStatus::Done);
    let w = rig.fake.world();
    assert_eq!(target_of(&w, 0).unwrap().account.as_deref(), Some("Work"));
    assert_eq!(target_of(&w, 2).unwrap().account, None);
}

#[gpui::test]
fn an_account_that_slipped_past_validation_fails_the_step_and_creates_nothing(
    cx: &mut TestAppContext,
) {
    let cases = [
        (
            json!({"kind": "codespace", "instance": {"sameAs": "build"}}),
            "Work",
            monocode_core::mono::api::ACCOUNT_EXISTING_INSTANCE,
        ),
        (
            json!({"kind": "codespace", "instance": "my-codespace"}),
            "Work",
            monocode_core::mono::api::ACCOUNT_EXISTING_INSTANCE,
        ),
        (
            json!({"kind": "codespace"}),
            "",
            "must be a non-empty text of at most 128 characters",
        ),
    ];
    for (target, account, message) in cases {
        let def = unchecked(
            vec![
                trigger(),
                session(
                    "dev",
                    json!({}),
                    json!({"target": target, "account": account}),
                ),
                report(),
            ],
            vec![edge("trigger", "dev"), edge("dev", "report")],
        );
        let rig = Rig::new(&[]);
        assert!(rig.start(cx, &def));
        cx.run_until_parked();
        let run = rig.last_run();
        assert_eq!(node(&run, "dev").status, NodeStatus::Failed, "{message}");
        assert!(
            node(&run, "dev")
                .error
                .as_deref()
                .unwrap()
                .contains(message),
            "{:?}",
            node(&run, "dev").error
        );
        assert!(rig.fake.world().starts.is_empty(), "{message}");
    }
}

// --- the instances of a run that is interrupted ---------------------------------------------

fn octo_run(sessions: &[&str]) -> RunInstance {
    RunInstance {
        kind: "codespace".into(),
        name: "octo-run".into(),
        created: true,
        nodes: vec!["build".into()],
        sessions: sessions.iter().map(|s| s.to_string()).collect(),
    }
}

#[gpui::test]
fn an_interrupted_run_keeps_the_instances_it_used_in_its_record(cx: &mut TestAppContext) {
    use monocode_core::mono::workflows::engine::mark_interrupted;
    let rig = Rig::new(&["one"]);
    let hold = rig.fake.hold();
    rig.start(cx, &build_then_check());
    cx.run_until_parked();
    // The app closes while `build` is working in its new codespace.
    rig.runner.quit();
    hold.close();
    cx.run_until_parked();
    let run = rig.last_run();
    assert_eq!(run.status, RunStatus::Running);
    assert_eq!(
        run.cloud_instances,
        vec![RunInstance {
            kind: "codespace".into(),
            name: "codespace-1".into(),
            created: true,
            nodes: vec!["build".into()],
            sessions: vec!["s1".into()],
        }]
    );
    // Next start: the run is interrupted and the record still names the instance, though no
    // report will ever be written for it.
    let interrupted = mark_interrupted(&run);
    assert_eq!(interrupted.status, RunStatus::Interrupted);
    assert_eq!(interrupted.cloud_instances, run.cloud_instances);
    let warning = interrupted.left_instances_warning().unwrap();
    assert!(
        warning.starts_with("This run left 1 cloud instance running: Codespace codespace-1."),
        "{warning}"
    );
    // Nothing was touched: not even the session was stopped.
    assert!(rig.fake.world().stops.is_empty());
}

#[gpui::test]
fn a_run_with_no_cloud_step_records_no_instance(cx: &mut TestAppContext) {
    let rig = Rig::new(&["ok"]);
    rig.start(cx, &simple(json!({})));
    cx.run_until_parked();
    assert!(rig.last_run().cloud_instances.is_empty());
}

#[gpui::test]
fn a_resumed_run_that_is_stopped_names_the_instances_of_the_interrupted_one_and_stops_none(
    cx: &mut TestAppContext,
) {
    use monocode_core::mono::workflows::engine::mark_interrupted;
    let def = build_then_check();
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
    run.cloud_instances = vec![octo_run(&["s9"])];
    let interrupted = mark_interrupted(&run);
    assert!(interrupted.left_instances_warning().is_some());

    let rig = Rig::new(&["one"]);
    rig.fake.knows_cloud("s9", "codespace", "octo-run");
    rig.fake.hold();
    let host: Rc<dyn WorkflowHost> = Rc::new(rig.fake.clone());
    assert!(
        rig.runner
            .resume(host, &cx.to_async(), "mono-1", &interrupted)
    );
    cx.run_until_parked();
    assert!(rig.runner.stop(WF));
    cx.run_until_parked();
    let w = rig.fake.world();
    // Only sessions were stopped (the old one's queued prompt, then the running turn): the fake
    // host has no call that stops or deletes an instance, and none was asked.
    assert!(
        w.stops.iter().all(|id| id == "s9" || id == "s1"),
        "{:?}",
        w.stops
    );
    let text = &w.posts.last().unwrap().text;
    assert_eq!(text.matches("octo-run").count(), 1, "{text}");
    assert!(
        text.contains("Codespace octo-run: created by this run; step build"),
        "{text}"
    );
    assert!(text.contains("never stops or deletes"), "{text}");
}

#[test]
fn the_report_names_an_instance_its_session_no_longer_tells_about() {
    use monocode_core::mono::workflows::engine::mark_interrupted;
    let def = build_then_check();
    let mut run = start_run(&def, RunTrigger::Manual, 1_000, "run-1");
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
    let mut interrupted = mark_interrupted(&run);
    // Without the record, a session the host does not know has no name to report.
    let unnamed = instances::report_section(&interrupted, |_| None).unwrap();
    assert!(unnamed.contains("name not known"), "{unnamed}");
    interrupted.cloud_instances = vec![octo_run(&["s9"])];
    let named = instances::report_section(&interrupted, |_| None).unwrap();
    assert!(
        named.contains("- Codespace octo-run: created by this run; step build"),
        "{named}"
    );
    assert!(!named.contains("name not known"), "{named}");
    assert_eq!(named.matches("octo-run").count(), 1, "{named}");
    // An instance the nodes no longer point at is still listed from the record.
    let mut orphan = interrupted.clone();
    orphan.nodes.iter_mut().for_each(|n| {
        n.interrupted_sessions = None;
    });
    let listed = instances::report_section(&orphan, |_| None).unwrap();
    assert!(
        listed.contains("- Codespace octo-run: created by this run; step build"),
        "{listed}"
    );
}

#[gpui::test]
fn a_start_that_fails_after_creating_its_instance_still_names_the_instance(
    cx: &mut TestAppContext,
) {
    // The error of such a start says the instance was created and kept.
    let rig = Rig::new(&[]);
    rig.fake
        .fail_starts(&monocode_core::cloud_sessions::created_note(
            "octo-new",
            "The setup failed",
        ));
    rig.start(cx, &build_then_check());
    cx.run_until_parked();
    let run = rig.last_run();
    assert_eq!(run.status, RunStatus::Failed);
    assert_eq!(node(&run, "build").status, NodeStatus::Failed);
    assert_eq!(
        run.cloud_instances,
        vec![RunInstance {
            kind: "codespace".into(),
            name: "octo-new".into(),
            created: true,
            nodes: vec!["build".into()],
            sessions: Vec::new(),
        }]
    );
    {
        let w = rig.fake.world();
        let text = &w.posts[0].text;
        assert!(
            text.contains("Codespace octo-new: created by this run; step build"),
            "{text}"
        );
        // Named, not touched.
        assert!(w.stops.is_empty());
    }
    // A start that failed before creating anything leaves no instance behind.
    let rig = Rig::new(&[]);
    rig.fake.fail_starts("No capacity");
    rig.start(cx, &build_then_check());
    cx.run_until_parked();
    let run = rig.last_run();
    assert_eq!(node(&run, "build").status, NodeStatus::Failed);
    assert!(run.cloud_instances.is_empty());
}
