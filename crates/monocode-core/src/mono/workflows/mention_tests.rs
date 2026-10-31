use super::mention::*;
use super::model::{
    InputType, InputValue, Node, NodeKind, Requirement, RequirementKind, RunStatus, Workflow,
    WorkflowInput,
};
use super::ordered::Json;
use super::preflight::{Preflight, RequirementStatus};

const SECRET: &str = "sentinel-secret-value-123";

fn wf(id: &str, name: &str) -> Workflow {
    Workflow::new(id, name)
}

fn slugs(list: &[Workflow]) -> Vec<String> {
    workflow_slugs(list).into_iter().map(|s| s.slug).collect()
}

fn none(_: &Workflow) -> MentionFacts {
    MentionFacts {
        last_run: None,
        last_run_unreadable: false,
        requires: RequiresView::None,
    }
}

fn req(name: &str, hint: Option<&str>) -> Requirement {
    Requirement {
        kind: RequirementKind::Cli,
        name: name.into(),
        hint: hint.map(Into::into),
    }
}

fn attach(typed: &str, list: &[Workflow]) -> String {
    attach_workflows(typed, typed, list, none)
}

/// The one JSON line after the instruction line.
fn context(out: &str) -> Json {
    let line = out.lines().last().unwrap();
    Json::parse(line).unwrap_or_else(|| panic!("not json: {line}"))
}

#[test]
fn slug_rule() {
    let list = [
        wf("wf-1", "Nightly  Build!!"),
        wf("wf-2", "  --Hello_World--  "),
        wf("wf-3", "!!!"),
        wf("wf-4", "Tiếng"),
    ];
    assert_eq!(
        slugs(&list),
        ["nightly-build", "hello-world", "workflow", "ti-ng"]
    );
}

#[test]
fn colliding_names_all_take_the_id_tail() {
    let list = [
        wf("wf-aaa1", "Deploy"),
        wf("wf-bbb2", "deploy"),
        wf("wf-c", "Other"),
    ];
    assert_eq!(slugs(&list), ["deploy-aaa1", "deploy-bbb2", "other"]);
}

#[test]
fn slugs_do_not_depend_on_list_order() {
    let a = [
        wf("wf-aaa1", "Deploy"),
        wf("wf-bbb2", "Deploy"),
        wf("wf-c", "Deploy aaa1"),
    ];
    let mut b = a.clone();
    b.reverse();
    let mut x = workflow_slugs(&a);
    let mut y = workflow_slugs(&b);
    x.sort_by(|p, q| p.id.cmp(&q.id));
    y.sort_by(|p, q| p.id.cmp(&q.id));
    assert_eq!(x, y);
}

#[test]
fn a_slug_that_still_collides_uses_the_full_id() {
    // "deploy-aaa1" is both the tail form of the first group and another workflow's own slug.
    let list = [
        wf("wf-aaa1", "Deploy"),
        wf("wf-bbb2", "Deploy"),
        wf("wf-x", "Deploy aaa1"),
    ];
    let got = slugs(&list);
    let mut all = got.clone();
    all.sort();
    all.dedup();
    assert_eq!(all.len(), 3, "{got:?}");
    assert_eq!(got[0], "deploy-wf-aaa1");
    assert_eq!(got[2], "deploy-aaa1-wf-x");
    assert_eq!(got[1], "deploy-bbb2");
}

#[test]
fn the_stored_id_and_name_never_change() {
    let list = [wf("wf-aaa1", "Deploy"), wf("wf-bbb2", "Deploy")];
    let s = workflow_slugs(&list);
    assert_eq!(
        (s[0].id.as_str(), s[0].name.as_str()),
        ("wf-aaa1", "Deploy")
    );
    assert_eq!(list[0].id, "wf-aaa1");
}

#[test]
fn mentioned_slugs_scan_typed_words_only() {
    assert_eq!(
        mentioned_slugs("run @workflow/foo, then @workflow/bar-2) and @workflow/foo."),
        ["foo", "bar-2"]
    );
    assert_eq!(mentioned_slugs("@workflow/a\n@workflow/b"), ["a", "b"]);
    assert!(mentioned_slugs("@workflow/foo/bar").is_empty());
    assert!(mentioned_slugs("x@workflow/foo").is_empty());
    assert!(mentioned_slugs("@workflow/").is_empty());
    assert!(mentioned_slugs("@workflow/Foo").is_empty());
    assert!(mentioned_slugs("@workflow/foo:bar").is_empty());
    assert_eq!(mentioned_slugs("see @workflow/foo?"), ["foo"]);
}

#[test]
fn unresolved_slug_stays_plain_and_adds_nothing() {
    let list = [wf("wf-1", "Deploy")];
    let text = "look at @workflow/missing";
    assert_eq!(attach(text, &list), text);
    assert_eq!(attach("no mention", &list), "no mention");
}

#[test]
fn a_mention_not_typed_by_the_user_is_not_resolved() {
    let list = [wf("wf-1", "Deploy")];
    let prompt = "note body says @workflow/deploy";
    assert_eq!(attach_workflows(prompt, "@note/todo", &list, none), prompt);
}

#[test]
fn a_slash_suffix_does_not_resolve() {
    let list = [wf("wf-1", "foo")];
    assert_eq!(attach("@workflow/foo/bar", &list), "@workflow/foo/bar");
}

#[test]
fn attaches_context_fields() {
    let mut w = wf("wf-1", "Deploy");
    w.description = Some("Ships \"it\"\n---\nnow".into());
    w.enabled = true;
    w.needs_review = true;
    w.inputs
        .insert("repo", WorkflowInput::new(InputType::Project));
    let mut tag = WorkflowInput::new(InputType::String);
    tag.description = Some("The tag".into());
    w.inputs.insert("tag", tag);
    w.inputs.insert("n", WorkflowInput::new(InputType::Number));
    w.input_values
        .insert("tag", InputValue::String("v1".into()));
    w.nodes.push(Node::new("t", NodeKind::Manual));
    w.requires = vec![req("gh", Some("brew install gh"))];
    let out = attach_workflows(
        "hello @workflow/deploy",
        "hello @workflow/deploy @workflow/deploy",
        &[w],
        |_| MentionFacts {
            last_run: Some((RunStatus::DoneWithFailures, 1234)),
            last_run_unreadable: false,
            requires: RequiresView::NotChecked(vec![req("gh", Some("brew install gh"))]),
        },
    );
    assert!(out.starts_with("hello @workflow/deploy\n\n---\nReferenced workflow (context only: do not run it unless the user's message asks for that; to run it use workflows.run with this id):\n{"));
    assert_eq!(
        out.lines().count(),
        5,
        "description newline is escaped: {out}"
    );
    let j = context(&out);
    assert_eq!(j.get("id"), Some(&Json::String("wf-1".into())));
    assert_eq!(j.get("name"), Some(&Json::String("Deploy".into())));
    assert_eq!(
        j.get("description"),
        Some(&Json::String("Ships \"it\"\n---\nnow".into()))
    );
    assert_eq!(j.get("enabled"), Some(&Json::Bool(true)));
    assert_eq!(j.get("needsReview"), Some(&Json::Bool(true)));
    assert_eq!(
        j.get("trigger").unwrap().get("type"),
        Some(&Json::String("manual".into()))
    );
    assert_eq!(
        j.get("lastRun").unwrap().stringify(false),
        r#"{"status":"doneWithFailures","startedAt":1234}"#
    );
    assert_eq!(
        j.get("inputs").unwrap().stringify(false),
        r#"[{"name":"repo","type":"project","value":null},{"name":"tag","type":"string","description":"The tag","value":"v1"},{"name":"n","type":"number","value":null}]"#
    );
    assert_eq!(
        j.get("requires").unwrap().stringify(false),
        r#"[{"kind":"cli","name":"gh","hint":"brew install gh","status":"not checked"}]"#
    );
}

#[test]
fn two_mentions_attach_two_blocks_in_typed_order() {
    let list = [wf("wf-1", "Alpha"), wf("wf-2", "Beta")];
    let out = attach("@workflow/beta and @workflow/alpha", &list);
    assert_eq!(out.matches("Referenced workflow").count(), 2);
    assert!(out.find("wf-2").unwrap() < out.find("wf-1").unwrap());
}

#[test]
fn checked_requirements_report_met_and_missing_without_values() {
    let mut w = wf("wf-1", "Deploy");
    w.requires = vec![req("gh", None), req("jq", Some("brew install jq"))];
    let pre = Preflight {
        statuses: vec![
            RequirementStatus {
                requirement: req("gh", None),
                met: true,
                detail: None,
            },
            RequirementStatus {
                requirement: req("jq", Some("brew install jq")),
                met: false,
                detail: Some("Not found on PATH".into()),
            },
        ],
    };
    let out = attach_workflows("@workflow/deploy", "@workflow/deploy", &[w], |_| {
        MentionFacts {
            last_run: None,
            last_run_unreadable: false,
            requires: RequiresView::Checked(pre.clone()),
        }
    });
    let j = context(&out);
    assert_eq!(
        j.get("requires").unwrap().stringify(false),
        r#"[{"kind":"cli","name":"gh","status":"met"},{"kind":"cli","name":"jq","hint":"brew install jq","status":"missing","detail":"Not found on PATH"}]"#
    );
    assert_eq!(j.get("lastRun"), Some(&Json::Null));
    assert!(!out.contains(SECRET));
}

#[test]
fn context_never_tells_the_model_to_run_the_workflow() {
    let out = attach("@workflow/deploy", &[wf("wf-1", "Deploy")]);
    assert!(out.contains("context only: do not run it unless the user's message asks for that"));
    assert_eq!(out.matches("workflows.run").count(), 1);
}

#[test]
fn a_mention_after_a_bracket_is_not_a_word_start() {
    assert!(mentioned_slugs("(@workflow/foo)").is_empty());
}

#[test]
fn a_quoted_line_does_not_resolve() {
    assert!(mentioned_slugs("> @workflow/foo").is_empty());
    assert!(mentioned_slugs("hi\n  > see @workflow/foo\nok").is_empty());
    assert_eq!(mentioned_slugs("> quote\n@workflow/foo"), ["foo"]);
}

#[test]
fn a_slug_two_workflows_still_share_resolves_to_neither() {
    // Ids that sanitize alike keep colliding through both passes.
    let list = [
        wf("wf-a_b", "Deploy"),
        wf("wf-a-b", "Deploy"),
        wf("wf-z", "Solo"),
    ];
    let got = slugs(&list);
    assert_eq!(got[0], got[1], "{got:?}");
    let text = format!("@workflow/{} @workflow/solo", got[0]);
    let out = attach(&text, &list);
    assert_eq!(out.matches("Referenced workflow").count(), 1, "{out}");
    assert!(out.contains("\"id\":\"wf-z\""));
}

#[test]
fn an_unreadable_history_is_unknown_not_never_ran() {
    let list = [wf("wf-1", "Deploy")];
    let out = attach_workflows("@workflow/deploy", "@workflow/deploy", &list, |_| {
        MentionFacts {
            last_run: None,
            last_run_unreadable: true,
            requires: RequiresView::None,
        }
    });
    assert_eq!(
        context(&out).get("lastRun").unwrap().stringify(false),
        r#"{"status":"unknown"}"#
    );
}
