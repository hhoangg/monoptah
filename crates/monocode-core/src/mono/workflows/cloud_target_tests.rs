//! Tests of the optional `target` of a session node (a step that runs in a codespace or a Docker
//! sandbox): reading, writing, the YAML file, validation and the model checks.
use super::fixtures::{json, node, workflow_from};
use super::model::*;
use super::models::{
    harnesses_used, session_node_problem, workflow_issues, CatalogHarness, CatalogModel,
    ModelCatalog,
};
use super::ordered::{Json, OrderedMap};
use super::validate::validate_workflow;
use super::yaml::{export_workflow, preview_import};
use crate::provider::Kind;
use serde_json::{json as value, Value};

fn target_of(n: &Node) -> Option<&SessionTarget> {
    match &n.kind {
        NodeKind::Session { target, .. } => target.as_ref(),
        _ => panic!("not a session"),
    }
}

fn session_json(id: &str, extra: Value) -> Value {
    let mut v = value!({
        "id": id,
        "type": "session",
        "project": "{{inputs.project}}",
        "prompt": "go",
        "checkout": "root",
        "output": {}
    });
    for (key, field) in extra.as_object().unwrap() {
        v[key] = field.clone();
    }
    v
}

/// trigger -> each node in `middle` -> report, as a stored workflow.
fn chain(middle: Vec<Value>) -> Workflow {
    let mut ids = vec!["trigger".to_string()];
    ids.extend(middle.iter().map(|n| n["id"].as_str().unwrap().to_string()));
    ids.push("report".into());
    let edges: Vec<Value> = ids
        .windows(2)
        .map(|pair| value!({"from": pair[0], "to": pair[1]}))
        .collect();
    let mut nodes = vec![value!({"id": "trigger", "type": "manual"})];
    nodes.extend(middle);
    nodes.push(value!({"id": "report", "type": "report"}));
    workflow_from(
        &value!({
            "id": "wf-abcd1234", "name": "Cloud",
            "inputs": {"project": {"type": "project"}},
            "inputValues": {"project": "/work/api"},
            "nodes": nodes, "edges": edges, "enabled": false, "createdAt": 0
        })
        .to_string(),
    )
}

fn cloud(id: &str, target: Value) -> Value {
    session_json(id, value!({"target": target}))
}

fn messages(w: &Workflow) -> Vec<String> {
    validate_workflow(w)
        .into_iter()
        .map(|i| i.message)
        .collect()
}

fn has(w: &Workflow, phrase: &str) -> bool {
    messages(w).iter().any(|m| m.contains(phrase))
}

fn no_issues(w: &Workflow) {
    assert_eq!(messages(w), Vec::<String>::new());
}

// --- parse and write ---------------------------------------------------------------------

#[test]
fn a_node_without_a_target_has_none_and_writes_none() {
    let n = node(&session_json("a", value!({})).to_string());
    assert_eq!(target_of(&n), None);
    assert!(serde_json::to_value(&n).unwrap().get("target").is_none());
}

#[test]
fn every_instance_form_is_read() {
    let read = |t: Value| {
        target_of(&node(&cloud("a", t).to_string()))
            .unwrap()
            .instance
            .clone()
    };
    assert_eq!(read(value!({"kind": "codespace"})), TargetInstance::New);
    assert_eq!(
        read(value!({"kind": "codespace", "instance": "new"})),
        TargetInstance::New
    );
    assert_eq!(
        read(value!({"kind": "sandbox", "instance": {"sameAs": "build"}})),
        TargetInstance::SameAs("build".into())
    );
    for raw in [
        r#"{"sameAs": ""}"#,
        r#"{"sameAs": 3}"#,
        r#"{"sameAs": "a", "x": 1}"#,
        r#"{}"#,
        r#""other""#,
        "null",
        "7",
        r#"["new"]"#,
    ] {
        let text = format!(r#"{{"kind":"codespace","instance":{raw}}}"#);
        let mut with = session_json("a", value!({}));
        with["target"] = serde_json::from_str(&text).unwrap();
        let n = node(&with.to_string());
        assert_eq!(
            target_of(&n).unwrap().instance,
            TargetInstance::Invalid(json(raw)),
            "{raw}"
        );
    }
}

#[test]
fn options_keep_their_written_order_and_other_keys() {
    let n = node(
        r#"{"id":"a","type":"session","project":"{{inputs.project}}","prompt":"p","checkout":"root",
            "target":{"machine":"basicLinux32gb","kind":"codespace","branch":"main","kit":"none"}}"#,
    );
    let t = target_of(&n).unwrap();
    assert_eq!(t.kind, "codespace");
    assert_eq!(
        t.options.keys().cloned().collect::<Vec<_>>(),
        ["machine", "branch", "kit"]
    );
    assert_eq!(t.options.get("branch"), Some(&Json::String("main".into())));
}

#[test]
fn a_target_that_is_not_readable_makes_the_node_unreadable() {
    use super::parse::parse_node;
    for raw in [
        r#""codespace""#,
        "7",
        "null",
        "[]",
        r#"{"instance":"new"}"#,
        r#"{"kind":3}"#,
        r#"{"kind":null}"#,
    ] {
        let text = format!(
            r#"{{"id":"a","type":"session","project":"{{{{inputs.project}}}}","prompt":"p","checkout":"root","target":{raw}}}"#
        );
        assert_eq!(parse_node(&json(&text)), None, "{raw}");
    }
}

#[test]
fn the_target_is_written_in_order_and_new_is_left_out() {
    let n = node(
        r#"{"id":"a","type":"session","project":"{{inputs.project}}","prompt":"go","checkout":"root",
            "target":{"kind":"sandbox","instance":"new","cpus":2,"branch":"main"}}"#,
    );
    let text = serde_json::to_string(target_of(&n).unwrap()).unwrap();
    assert_eq!(text, r#"{"kind":"sandbox","cpus":2,"branch":"main"}"#);
    let same = node(
        &cloud(
            "b",
            value!({"kind": "codespace", "instance": {"sameAs": "a"}}),
        )
        .to_string(),
    );
    assert_eq!(
        serde_json::to_string(target_of(&same).unwrap()).unwrap(),
        r#"{"kind":"codespace","instance":{"sameAs":"a"}}"#
    );
    let bad = node(&cloud("c", value!({"kind": "codespace", "instance": 7})).to_string());
    assert_eq!(
        serde_json::to_string(target_of(&bad).unwrap()).unwrap(),
        r#"{"kind":"codespace","instance":7}"#
    );
    // The node carries it under `target`, and a stored workflow reads back the same.
    let written = serde_json::to_value(&n).unwrap();
    assert_eq!(
        written["target"],
        value!({"kind": "sandbox", "cpus": 2, "branch": "main"})
    );
    assert_eq!(super::parse::parse_node(&Json::from(&written)).unwrap(), n);
}

#[test]
fn the_fingerprint_changes_with_the_target() {
    let local = chain(vec![session_json("a", value!({}))]);
    let cloud_a = chain(vec![cloud("a", value!({"kind": "codespace"}))]);
    let cloud_b = chain(vec![cloud("a", value!({"kind": "sandbox"}))]);
    let branch = chain(vec![cloud(
        "a",
        value!({"kind": "codespace", "branch": "main"}),
    )]);
    let prints: Vec<String> = [&local, &cloud_a, &cloud_b, &branch]
        .iter()
        .map(|w| definition_fingerprint(w))
        .collect();
    for (i, a) in prints.iter().enumerate() {
        for b in &prints[i + 1..] {
            assert_ne!(a, b);
        }
    }
}

// --- the YAML file -----------------------------------------------------------------------

#[test]
fn a_target_survives_export_and_import() {
    let mut w = chain(vec![
        cloud(
            "a",
            value!({"kind": "codespace", "machine": "basicLinux32gb", "branch": "main"}),
        ),
        cloud(
            "b",
            value!({"kind": "codespace", "instance": {"sameAs": "a"}}),
        ),
        cloud(
            "c",
            value!({"kind": "sandbox", "cpus": 2, "memory": "4g", "kit": "none"}),
        ),
        cloud(
            "d",
            value!({"kind": "weird", "instance": null, "x": [1, "two", {"k": true}]}),
        ),
        cloud(
            "e",
            value!({"kind": "codespace", "instance": {"sameAs": "a", "more": 1}}),
        ),
        cloud("f", value!({"kind": "codespace", "instance": "reuse-me"})),
        session_json("g", value!({})),
    ]);
    // serde_json sorts the keys the builder wrote: put the written order back for node "a".
    let a = w.nodes.iter_mut().find(|n| n.id == "a").unwrap();
    if let NodeKind::Session {
        target: Some(target),
        ..
    } = &mut a.kind
    {
        target.options = OrderedMap::new();
        target
            .options
            .insert("machine", Json::String("basicLinux32gb".into()));
        target.options.insert("branch", Json::String("main".into()));
    }
    let text = export_workflow(&w);
    let preview = preview_import(&text, &ModelCatalog::unprobed()).unwrap();
    assert_eq!(preview.workflow.nodes, w.nodes);
    let read = |id: &str| {
        target_of(preview.workflow.nodes.iter().find(|n| n.id == id).unwrap())
            .cloned()
            .unwrap()
    };
    assert_eq!(
        read("a").options.keys().cloned().collect::<Vec<_>>(),
        ["machine", "branch"]
    );
    assert_eq!(read("d").instance, TargetInstance::Invalid(Json::Null));
    assert!(text.contains("sameAs: a"), "{text}");
    // Exporting again gives the same file.
    let mut again = w.clone();
    again.nodes = preview.workflow.nodes.clone();
    assert_eq!(export_workflow(&again), text);
}

// --- validation --------------------------------------------------------------------------

#[test]
fn valid_cloud_steps_pass() {
    no_issues(&chain(vec![cloud(
        "a",
        value!({"kind": "codespace", "branch": "main", "machine": "basicLinux32gb"}),
    )]));
    no_issues(&chain(vec![cloud(
        "a",
        value!({"kind": "sandbox", "instance": "new", "cpus": 2, "memory": "4g", "kit": "none"}),
    )]));
    no_issues(&chain(vec![
        cloud("a", value!({"kind": "sandbox"})),
        cloud(
            "b",
            value!({"kind": "sandbox", "instance": {"sameAs": "a"}}),
        ),
    ]));
    no_issues(&chain(vec![session_json(
        "a",
        value!({"target": {"kind": "codespace"}, "harness": "claude", "model": "claude:sonnet-5"}),
    )]));
}

#[test]
fn an_unknown_kind_is_reported() {
    let w = chain(vec![cloud("a", value!({"kind": "vm"}))]);
    assert!(has(&w, "target: target.kind must be codespace or sandbox"));
    let at: Vec<_> = validate_workflow(&w)
        .into_iter()
        .filter_map(|i| i.node_id)
        .collect();
    assert_eq!(at, ["a"]);
}

#[test]
fn a_cloud_step_runs_claude_only() {
    let w = chain(vec![session_json(
        "a",
        value!({"target": {"kind": "codespace"}, "harness": "codex"}),
    )]);
    assert!(has(&w, "runs Claude only"));
    let w = chain(vec![session_json(
        "a",
        value!({"target": {"kind": "codespace"}, "model": "grok:grok-4.6"}),
    )]);
    assert!(has(&w, "runs Claude only"));
    // A local step may use any harness.
    no_issues(&chain(vec![session_json(
        "a",
        value!({"harness": "codex"}),
    )]));
}

#[test]
fn a_cloud_step_needs_the_root_checkout() {
    let w = chain(vec![session_json(
        "a",
        value!({"target": {"kind": "codespace"}, "checkout": "newWorktree"}),
    )]);
    assert!(has(&w, "newWorktree"));
    let w = chain(vec![
        cloud("a", value!({"kind": "codespace"})),
        session_json(
            "b",
            value!({"target": {"kind": "codespace"}, "checkout": {"worktreeOf": "a"}}),
        ),
    ]);
    assert!(has(&w, "cannot use checkout worktreeOf"));
    // A local step may still use a new worktree.
    no_issues(&chain(vec![session_json(
        "a",
        value!({"checkout": "newWorktree"}),
    )]));
}

#[test]
fn same_as_must_name_an_earlier_cloud_step_of_the_same_kind() {
    let same = |id: &str| {
        cloud(
            "b",
            value!({"kind": "codespace", "instance": {"sameAs": id}}),
        )
    };
    let a = cloud("a", value!({"kind": "codespace"}));
    assert!(has(
        &chain(vec![a.clone(), same("nope")]),
        "sameAs \"nope\" does not exist"
    ));
    assert!(has(
        &chain(vec![a.clone(), same("trigger")]),
        "sameAs \"trigger\" must be a session node"
    ));
    assert!(has(
        &chain(vec![a.clone(), same("b")]),
        "cannot be the node itself"
    ));
    assert!(has(
        &chain(vec![session_json("a", value!({})), same("a")]),
        "must be a cloud session node"
    ));
    assert!(has(
        &chain(vec![cloud("a", value!({"kind": "sandbox"})), same("a")]),
        "must be the same kind"
    ));
    // Downstream: `b` runs before `a`.
    assert!(has(&chain(vec![same("a"), a]), "must run before this node"));
}

#[test]
fn options_with_same_as_are_refused() {
    let w = chain(vec![
        cloud("a", value!({"kind": "codespace"})),
        cloud(
            "b",
            value!({"kind": "codespace", "instance": {"sameAs": "a"}, "branch": "main"}),
        ),
    ]);
    assert!(has(&w, "only applies when creating a codespace"));
}

#[test]
fn a_bad_instance_is_reported_once() {
    for instance in [
        value!("other"),
        value!({"sameAs": ""}),
        value!(null),
        value!(4),
    ] {
        let w = chain(vec![cloud(
            "a",
            value!({"kind": "codespace", "instance": instance}),
        )]);
        let found = messages(&w);
        assert_eq!(
            found,
            ["target.instance must be \"new\" or {\"sameAs\": \"<session node id>\"}"],
            "{instance}"
        );
    }
    // The kind and the options are still judged.
    let w = chain(vec![cloud(
        "a",
        value!({"kind": "vm", "instance": "other", "branch": "main"}),
    )]);
    assert!(has(&w, "target.kind must be"));
    assert!(has(&w, "target.instance must be \"new\""));
}

#[test]
fn string_options_may_hold_templates_that_are_checked_like_a_prompt() {
    // Changed on purpose: templates in these options used to be refused ("templates are not
    // filled in"); a run now fills them in when the step starts.
    for (kind, key, value) in [
        // (`{{inputs.project}}` used to be accepted here; a project input fills in a local path,
        // so it is refused now, see `a_project_input_cannot_fill_a_target_option`.)
        ("codespace", "branch", "{{run.id}}"),
        ("codespace", "machine", "m-{{run.id}}"),
        ("sandbox", "branch", "x-{{run.id}}"),
        ("sandbox", "image", "img:{{run.id}}"),
        ("sandbox", "ttl", "{{run.id}}"),
        ("sandbox", "memory", "{{run.id}}"),
        ("sandbox", "template", "{{run.id}}"),
    ] {
        no_issues(&chain(vec![cloud("a", value!({"kind": kind, key: value}))]));
    }
    // Upstream outputs and loop variables are in scope as in a prompt.
    no_issues(&chain(vec![
        session_json("a", value!({"output": {"branch": {"type": "string"}}})),
        cloud("b", value!({"kind": "codespace", "branch": "{{a.branch}}"})),
    ]));
}

#[test]
fn a_project_input_cannot_fill_a_target_option() {
    for (kind, key) in [
        ("codespace", "branch"),
        ("codespace", "machine"),
        ("sandbox", "image"),
        ("sandbox", "template"),
    ] {
        let w = chain(vec![cloud(
            "a",
            value!({"kind": kind, key: "x-{{inputs.project}}"}),
        )]);
        assert!(
            has(&w, "{{inputs.project}} is a project input"),
            "{kind} {key}: {:?}",
            messages(&w)
        );
    }
    // Other inputs and upstream outputs are fine.
    let mut w = chain(vec![cloud(
        "a",
        value!({"kind": "codespace", "branch": "{{inputs.branch}}"}),
    )]);
    w.inputs
        .insert("branch", WorkflowInput::new(InputType::String));
    no_issues(&w);
}

#[test]
fn an_account_that_is_not_text_makes_the_node_unreadable_instead_of_being_dropped() {
    for account in [
        value!(5),
        value!(true),
        value!(["Work"]),
        value!({"id": "x"}),
    ] {
        let raw = cloud_with_account("a", value!({"kind": "codespace"}), account).to_string();
        assert!(
            super::parse::parse_node(&Json::parse(&raw).unwrap()).is_none(),
            "{raw}"
        );
    }
    // Absent and null both mean no account.
    let raw = cloud_with_account("a", value!({"kind": "codespace"}), value!(null)).to_string();
    assert_eq!(account_of(&node(&raw)), None);
}

#[test]
fn an_unknown_template_path_in_an_option_is_an_issue() {
    let w = chain(vec![cloud(
        "a",
        value!({"kind": "codespace", "branch": "{{inputs.nope}}"}),
    )]);
    assert!(has(&w, "{{inputs.nope}} does not name a declared input"));
    let w = chain(vec![cloud(
        "a",
        value!({"kind": "sandbox", "image": "{{ghost.out}}"}),
    )]);
    assert!(has(&w, "{{ghost.out}} is unknown"));
    // A node that is not upstream has no output to read yet.
    let w = chain(vec![
        cloud("a", value!({"kind": "codespace", "branch": "{{b.branch}}"})),
        session_json("b", value!({"output": {"branch": {"type": "string"}}})),
    ]);
    assert!(has(&w, "not upstream of this node"));
}

#[test]
fn only_the_string_options_can_hold_a_template() {
    for (kind, key, value) in [
        ("sandbox", "cpus", value!("{{run.id}}")),
        ("sandbox", "kit", value!("{{run.id}}")),
        ("codespace", "branch", value!(["{{run.id}}"])),
        ("codespace", "machine", value!({"k": "{{run.id}}"})),
    ] {
        let w = chain(vec![cloud("a", value!({"kind": kind, key: value}))]);
        assert!(
            has(&w, "templates are filled in only in target.branch"),
            "{kind} {key}: {:?}",
            messages(&w)
        );
    }
    // `kind` and `instance` stay literal: they decide whether an instance is made at all.
    assert!(has(
        &chain(vec![cloud("a", value!({"kind": "{{run.id}}"}))]),
        "target.kind must be"
    ));
    assert!(has(
        &chain(vec![cloud(
            "a",
            value!({"kind": "codespace", "instance": "{{run.id}}"})
        )]),
        "target.instance must be \"new\""
    ));
    // Literal values of these keys are still checked as before.
    assert!(has(
        &chain(vec![cloud("a", value!({"kind": "codespace", "cpus": 2}))]),
        "target does not take cpus for a codespace"
    ));
    assert!(has(
        &chain(vec![cloud(
            "a",
            value!({"kind": "sandbox", "template": "t", "image": "i"})
        )]),
        "either template or image"
    ));
    assert!(has(
        &chain(vec![cloud("a", value!({"kind": "sandbox", "cpus": 0}))]),
        "target.cpus"
    ));
}

#[test]
fn a_templated_option_still_needs_a_new_instance_and_keeps_the_combination_rules() {
    let w = chain(vec![
        cloud("a", value!({"kind": "codespace"})),
        cloud(
            "b",
            value!({"kind": "codespace", "instance": {"sameAs": "a"}, "branch": "{{inputs.project}}"}),
        ),
    ]);
    assert!(has(&w, "only applies when creating a codespace"));
    let w = chain(vec![cloud(
        "a",
        value!({"kind": "sandbox", "template": "{{run.id}}", "image": "i"}),
    )]);
    assert!(has(&w, "either template or image"));
    let w = chain(vec![cloud(
        "a",
        value!({"kind": "sandbox", "template": "{{run.id}}", "cpus": 2}),
    )]);
    assert!(has(&w, "cannot be combined with target.template"));
    // A literal option next to a templated one is still judged.
    let w = chain(vec![cloud(
        "a",
        value!({"kind": "sandbox", "branch": "{{run.id}}", "cpus": 0}),
    )]);
    assert!(has(&w, "target.cpus"));
}

#[test]
fn a_local_step_cannot_use_the_worktree_of_a_cloud_step() {
    let w = chain(vec![
        cloud("a", value!({"kind": "codespace"})),
        session_json("b", value!({"checkout": {"worktreeOf": "a"}})),
    ]);
    assert!(has(
        &w,
        "that node runs in the cloud and has no local worktree"
    ));
    let w = chain(vec![
        cloud("a", value!({"kind": "codespace"})),
        value!({"id": "c", "type": "command", "project": "{{inputs.project}}",
                "command": "ls", "cwd": {"worktreeOf": "a"}}),
    ]);
    assert!(has(&w, "cwd worktreeOf \"a\": that node runs in the cloud"));
    // A local session is fine.
    no_issues(&chain(vec![
        session_json("a", value!({"checkout": "newWorktree"})),
        session_json("b", value!({"checkout": {"worktreeOf": "a"}})),
    ]));
}

// --- models ------------------------------------------------------------------------------

fn catalog(claude_installed: bool) -> ModelCatalog {
    ModelCatalog {
        probed: true,
        harnesses: Kind::ALL
            .into_iter()
            .map(|kind| CatalogHarness {
                id: kind.id().into(),
                installed: kind == Kind::Claude && claude_installed,
                models: (kind == Kind::Claude).then(|| {
                    vec![CatalogModel {
                        id: "claude-sonnet-5".into(),
                        alias_target: None,
                        efforts: vec![],
                    }]
                }),
            })
            .collect(),
    }
}

#[test]
fn a_cloud_step_without_a_harness_is_judged_as_claude() {
    let w = chain(vec![session_json(
        "a",
        value!({"target": {"kind": "codespace"}, "model": "claude:nope"}),
    )]);
    assert_eq!(harnesses_used(&w), [Kind::Claude]);
    let a = w.nodes.iter().find(|n| n.id == "a").unwrap();
    assert!(session_node_problem(a, &catalog(true))
        .unwrap()
        .contains("claude:nope"));
    // Claude need not be installed here: it runs in the instance.
    assert_eq!(session_node_problem(a, &catalog(false)), None);
    // A good model passes.
    let ok = chain(vec![session_json(
        "a",
        value!({"target": {"kind": "codespace"}, "model": "claude:sonnet-5"}),
    )]);
    assert!(workflow_issues(&ok, &catalog(true)).is_empty());
    // The same local step with a bare claude harness needs the CLI.
    let local = chain(vec![session_json("a", value!({"harness": "claude"}))]);
    let a = local.nodes.iter().find(|n| n.id == "a").unwrap();
    assert!(session_node_problem(a, &catalog(false)).is_some());
}

#[test]
fn another_harness_on_a_cloud_step_is_reported_once() {
    let w = chain(vec![session_json(
        "a",
        value!({"target": {"kind": "codespace"}, "harness": "codex", "model": "codex:gpt"}),
    )]);
    let found = workflow_issues(&w, &catalog(true));
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(found[0].message.contains("runs Claude only"));
}

#[test]
fn the_summary_reads_for_the_board() {
    let summary = |t: Value| {
        target_of(&node(&cloud("a", t).to_string()))
            .unwrap()
            .summary()
    };
    assert_eq!(summary(value!({"kind": "codespace"})), "Codespace (new)");
    assert_eq!(
        summary(value!({"kind": "sandbox", "instance": {"sameAs": "build"}})),
        "Sandbox (same as build)"
    );
    assert_eq!(
        summary(value!({"kind": "sandbox", "instance": 3})),
        "Sandbox (invalid instance)"
    );
    assert_eq!(summary(value!({"kind": "vm"})), "vm (new)");
}

#[test]
fn a_same_as_step_must_use_the_project_input_of_its_source() {
    let build = cloud("build", value!({"kind": "codespace"}));
    let mut check = cloud(
        "check",
        value!({"kind": "codespace", "instance": {"sameAs": "build"}}),
    );
    check["project"] = value!("{{inputs.other}}");
    let w = workflow_from(
        &value!({
            "id": "wf-abcd1234", "name": "Cloud",
            "inputs": {"project": {"type": "project"}, "other": {"type": "project"}},
            "inputValues": {"project": "/work/api", "other": "/work/web"},
            "nodes": [
                {"id": "trigger", "type": "manual"}, build, check,
                {"id": "report", "type": "report"}
            ],
            "edges": [
                {"from": "trigger", "to": "build"},
                {"from": "build", "to": "check"},
                {"from": "check", "to": "report"}
            ],
            "enabled": false, "createdAt": 0
        })
        .to_string(),
    );
    assert!(
        has(&w, "must use the same project input"),
        "{:?}",
        messages(&w)
    );
}

// --- account ------------------------------------------------------------------------------

fn cloud_with_account(id: &str, target: Value, account: Value) -> Value {
    session_json(id, value!({"target": target, "account": account}))
}

fn account_of(n: &Node) -> Option<&str> {
    match &n.kind {
        NodeKind::Session { account, .. } => account.as_deref(),
        _ => panic!("not a session"),
    }
}

#[test]
fn an_account_is_read_written_and_part_of_the_fingerprint() {
    let n =
        node(&cloud_with_account("a", value!({"kind": "codespace"}), value!("Work")).to_string());
    assert_eq!(account_of(&n), Some("Work"));
    let written = serde_json::to_value(&n).unwrap();
    assert_eq!(written["account"], "Work");
    assert_eq!(super::parse::parse_node(&Json::from(&written)).unwrap(), n);
    let none = node(&cloud("a", value!({"kind": "codespace"})).to_string());
    assert_eq!(account_of(&none), None);
    assert!(serde_json::to_value(&none)
        .unwrap()
        .get("account")
        .is_none());
    // Approving a workflow must not carry over to a different account.
    let a = chain(vec![cloud("a", value!({"kind": "codespace"}))]);
    let b = chain(vec![cloud_with_account(
        "a",
        value!({"kind": "codespace"}),
        value!("Work"),
    )]);
    let c = chain(vec![cloud_with_account(
        "a",
        value!({"kind": "codespace"}),
        value!("Home"),
    )]);
    assert_ne!(definition_fingerprint(&a), definition_fingerprint(&b));
    assert_ne!(definition_fingerprint(&b), definition_fingerprint(&c));
}

#[test]
fn an_account_on_a_new_instance_is_valid() {
    for account in ["default", "Work", "account-1234"] {
        no_issues(&chain(vec![cloud_with_account(
            "a",
            value!({"kind": "codespace", "branch": "main"}),
            value!(account),
        )]));
        no_issues(&chain(vec![cloud_with_account(
            "a",
            value!({"kind": "sandbox", "instance": "new", "cpus": 2}),
            value!(account),
        )]));
    }
}

#[test]
fn an_account_with_same_as_or_a_local_step_is_refused() {
    let w = chain(vec![
        cloud("a", value!({"kind": "codespace"})),
        cloud_with_account(
            "b",
            value!({"kind": "codespace", "instance": {"sameAs": "a"}}),
            value!("Work"),
        ),
    ]);
    assert!(has(&w, "The account belongs to the instance"));
    let w = chain(vec![session_json("a", value!({"account": "Work"}))]);
    assert!(has(&w, "account only applies to a cloud step"));
}

#[test]
fn an_account_must_be_a_plain_short_text() {
    for (account, phrase) in [
        ("".to_string(), "non-empty text of at most 128"),
        ("   ".to_string(), "non-empty text of at most 128"),
        ("a".repeat(129), "non-empty text of at most 128"),
        (
            "{{inputs.project}}".to_string(),
            "account is used as written",
        ),
    ] {
        let w = chain(vec![cloud_with_account(
            "a",
            value!({"kind": "codespace"}),
            value!(account),
        )]);
        assert!(has(&w, phrase), "{account:?}: {:?}", messages(&w));
    }
}

#[test]
fn templated_options_and_the_account_survive_export_and_import() {
    let w = chain(vec![
        cloud_with_account(
            "a",
            value!({"kind": "codespace", "branch": "{{inputs.project}}", "machine": "m-{{run.id}}"}),
            value!("Work"),
        ),
        cloud(
            "b",
            value!({"kind": "codespace", "instance": {"sameAs": "a"}}),
        ),
    ]);
    let text = export_workflow(&w);
    assert!(text.contains("account: Work"), "{text}");
    assert!(text.contains("{{inputs.project}}"), "{text}");
    let preview = preview_import(&text, &ModelCatalog::unprobed()).unwrap();
    assert_eq!(preview.workflow.nodes, w.nodes);
    let read = preview.workflow.nodes.iter().find(|n| n.id == "a").unwrap();
    assert_eq!(account_of(read), Some("Work"));
    assert_eq!(
        target_of(read).unwrap().options.get("branch"),
        Some(&Json::String("{{inputs.project}}".into()))
    );
    let mut again = w.clone();
    again.nodes = preview.workflow.nodes.clone();
    assert_eq!(export_workflow(&again), text);
}

// --- the instances a run used --------------------------------------------------------------

fn instance(kind: &str, name: &str, created: bool) -> RunInstance {
    RunInstance {
        kind: kind.into(),
        name: name.into(),
        created,
        nodes: vec!["build".into()],
        sessions: vec!["remote:1".into()],
    }
}

#[test]
fn an_old_run_record_without_instances_still_reads() {
    let mut run = super::ui_fixtures::run();
    assert!(run.cloud_instances.is_empty());
    let written = serde_json::to_value(&run).unwrap();
    assert!(
        written.get("cloudInstances").is_none(),
        "left out when empty"
    );
    // A record written before the field existed.
    let back: WorkflowRun = serde_json::from_value(written).unwrap();
    assert_eq!((back.id.as_str(), back.status), ("run-1", RunStatus::Done));
    assert!(back.cloud_instances.is_empty());
    run.cloud_instances = vec![instance("codespace", "octo-run", true)];
    let written = serde_json::to_value(&run).unwrap();
    assert_eq!(written["cloudInstances"][0]["name"], "octo-run");
    assert_eq!(written["cloudInstances"][0]["created"], true);
    let back: WorkflowRun = serde_json::from_value(written).unwrap();
    assert_eq!(back.cloud_instances, run.cloud_instances);
    // An entry that lacks the optional parts reads too.
    let sparse: RunInstance =
        serde_json::from_value(value!({"kind": "sandbox", "name": "box"})).unwrap();
    assert_eq!(
        (sparse.created, sparse.nodes.len(), sparse.sessions.len()),
        (false, 0, 0)
    );
}

#[test]
fn only_an_interrupted_run_warns_about_what_it_left_running() {
    let mut run = super::ui_fixtures::run();
    run.cloud_instances = vec![
        instance("codespace", "octo-run", true),
        instance("sandbox", "box", false),
    ];
    for status in [RunStatus::Running, RunStatus::Done, RunStatus::Stopped] {
        run.status = status;
        assert_eq!(run.left_instances_warning(), None);
    }
    run.status = RunStatus::Interrupted;
    let warning = run.left_instances_warning().unwrap();
    assert!(
        warning.starts_with(
            "This run left 2 cloud instances running: Codespace octo-run, Sandbox box."
        ),
        "{warning}"
    );
    assert!(warning.contains("never stops or deletes"), "{warning}");
    run.cloud_instances.truncate(1);
    assert!(run
        .left_instances_warning()
        .unwrap()
        .starts_with("This run left 1 cloud instance running: Codespace octo-run."));
    run.cloud_instances.clear();
    assert_eq!(run.left_instances_warning(), None);
}

#[test]
fn the_interrupted_runs_of_a_workflow_name_each_instance_once() {
    let mut a = super::ui_fixtures::run();
    a.status = RunStatus::Interrupted;
    a.cloud_instances = vec![instance("codespace", "octo-run", true)];
    let mut b = a.clone();
    b.cloud_instances = vec![
        instance("codespace", "octo-run", false),
        instance("sandbox", "box", true),
    ];
    let mut done = a.clone();
    done.status = RunStatus::Done;
    done.cloud_instances = vec![instance("codespace", "finished-one", true)];
    let found = interrupted_instances(&[a, b, done]);
    let names: Vec<&str> = found.iter().map(|i| i.name.as_str()).collect();
    assert_eq!(names, ["octo-run", "box"]);
}
