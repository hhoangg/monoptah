use super::*;
use crate::cloud_sessions::docker_config::DockerProjectConfig;

fn record(extra: Value) -> MoveRecord {
    let mut base = json!({
        "sessionId": 7,
        "target": {"kind": "newCodespace", "machine": null},
        "kind": {"kind": "resume"},
        "accountHome": "/Users/me/.claude-work",
        "projectCwd": "/Users/me/secret-project",
        "workCwd": "/Users/me/secret-project-wt",
        "title": "t",
        "provider": "claude",
        "startedAt": 1,
        "done": ["snapshot"],
        "current": "push",
        "failedAt": null,
        "error": null,
        "hostCwd": "/workspaces/x",
        "hostSessionId": "host-1",
        "patchUpload": "upload-1",
        "owner": "owner-secret",
        "createCommand": "move-7-abc",
        "branch": "feat",
    });
    base.as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
    serde_json::from_value(base).unwrap()
}

fn ok(action: &str, input: Value) {
    crate::mono::api::validate(action, &input).unwrap_or_else(|e| panic!("{input}: {e}"));
}
fn refused(action: &str, input: Value) -> String {
    crate::mono::api::validate(action, &input)
        .err()
        .unwrap_or_else(|| panic!("{action} {input} was accepted"))
}

#[test]
fn a_valid_move_is_accepted_in_every_spelling() {
    for input in [
        json!({"session": 5, "target": {"kind": "codespace", "instance": "new"}}),
        json!({"session": "5", "target": {"kind": "sandbox", "instance": "new"}, "kit": "none"}),
        json!({"session": 5, "target": {"kind": "codespace", "instance": "octo-abc"}, "kit": "default", "project": "/p"}),
    ] {
        ok(MOVE_TO_CLOUD, input);
    }
    for action in [MOVE_TO_LOCAL, MOVE_STATUS, CANCEL_MOVE] {
        ok(action, json!({"session": 5}));
        ok(action, json!({"session": -5, "project": "/p"}));
    }
}

#[test]
fn unknown_keys_and_ways_to_widen_are_refused() {
    let target = json!({"kind": "codespace", "instance": "new"});
    for key in [
        "force",
        "deleteBranch",
        "deleteWorkspace",
        "skipChecks",
        "kitItems",
        "items",
        "branch",
        "sessionId",
    ] {
        let mut input = json!({"session": 5, "target": target});
        input[key] = json!(true);
        let error = refused(MOVE_TO_CLOUD, input);
        assert!(error.contains("does not take"), "{key}: {error}");
    }
    for key in [
        "force",
        "branch",
        "machine",
        "cpus",
        "deleteWorkspace",
        "kit",
    ] {
        let mut t = target.clone();
        t[key] = json!("x");
        assert!(
            refused(MOVE_TO_CLOUD, json!({"session": 5, "target": t})).contains("does not take"),
            "{key}"
        );
    }
    for action in [MOVE_TO_LOCAL, MOVE_STATUS, CANCEL_MOVE] {
        for key in [
            "force",
            "deleteWorkspace",
            "deleteBranch",
            "skipChecks",
            "sessionId",
        ] {
            let error = refused(action, json!({"session": 5, key: true}));
            assert!(error.contains("does not take"), "{action} {key}: {error}");
        }
    }
}

#[test]
fn a_bad_target_or_session_is_refused() {
    for (input, needle) in [
        (
            json!({"target": {"kind": "codespace", "instance": "new"}}),
            "Provide session",
        ),
        (
            json!({"session": 0, "target": {"kind": "codespace", "instance": "new"}}),
            "session ID",
        ),
        (
            json!({"session": "abc", "target": {"kind": "codespace", "instance": "new"}}),
            "session ID",
        ),
        (
            json!({"session": -3, "target": {"kind": "codespace", "instance": "new"}}),
            "already lives in the cloud",
        ),
        (json!({"session": 5}), "target must be"),
        (
            json!({"session": 5, "target": "codespace"}),
            "target must be",
        ),
        (
            json!({"session": 5, "target": {"instance": "new"}}),
            "target must be",
        ),
        (
            json!({"session": 5, "target": {"kind": "vm", "instance": "new"}}),
            "target must be",
        ),
        (
            json!({"session": 5, "target": {"kind": "codespace"}}),
            "target must be",
        ),
        (
            json!({"session": 5, "target": {"kind": "codespace", "instance": {"session": 4}}}),
            "target must be",
        ),
        (
            json!({"session": 5, "target": {"kind": "codespace", "instance": 3}}),
            "target must be",
        ),
        (
            json!({"session": 5, "target": {"kind": "codespace", "instance": "a b; rm"}}),
            "",
        ),
        (
            json!({"session": 5, "target": {"kind": "codespace", "instance": "new"}, "kit": "all"}),
            "kit must be",
        ),
        (
            json!({"session": 5, "target": {"kind": "codespace", "instance": "new"}, "kit": {"skills": ["x"]}}),
            "must be text",
        ),
        (
            json!({"session": 5, "target": {"kind": "codespace", "instance": "new"}, "kit": ["x"]}),
            "must be text",
        ),
    ] {
        let error = refused(MOVE_TO_CLOUD, input.clone());
        assert!(error.contains(needle), "{input}: {error}");
    }
}

#[test]
fn only_a_mono_may_call_the_move_actions() {
    for action in [MOVE_TO_CLOUD, MOVE_TO_LOCAL, MOVE_STATUS, CANCEL_MOVE] {
        let input = json!({"session": 5, "target": {"kind": "codespace", "instance": "new"}});
        let error = crate::app_access::validate_operator(action, &input).unwrap_err();
        assert_eq!(error, MONO_ONLY, "{action}");
    }
}

#[test]
fn kit_none_turns_the_kit_off_and_nothing_else_changes_it() {
    let parse = |kit: Option<&str>| {
        let mut input = json!({"session": 5, "target": {"kind": "codespace", "instance": "new"}});
        if let Some(kit) = kit {
            input["kit"] = json!(kit);
        }
        parse_move_to_cloud(&input).unwrap().kit_off
    };
    assert!(parse(Some("none")));
    // "default" is the user's global kit as set in Settings: it adds nothing.
    assert!(!parse(Some("default")));
    assert!(!parse(None));
}

fn offered() -> Vec<MoveTarget> {
    vec![
        MoveTarget::NewCodespace { machine: None },
        MoveTarget::Existing {
            backend: CloudBackendKind::Codespaces,
            name: "octo-abc".into(),
        },
    ]
}

fn request(kind: CloudBackendKind, instance: Instance) -> MoveToCloud {
    MoveToCloud {
        session: 5,
        kind,
        instance,
        kit_off: false,
    }
}

#[test]
fn a_target_must_be_one_the_dialog_offers() {
    let kind = CloudBackendKind::Codespaces;
    assert_eq!(
        resolve_target(&request(kind, Instance::New), &offered()).unwrap(),
        MoveTarget::NewCodespace { machine: None }
    );
    assert_eq!(
        resolve_target(
            &request(kind, Instance::Named("octo-abc".into())),
            &offered()
        )
        .unwrap(),
        MoveTarget::Existing {
            backend: kind,
            name: "octo-abc".into()
        }
    );
    // An instance that runs no session of the project, a kind that is not offered.
    let error =
        resolve_target(&request(kind, Instance::Named("other".into())), &offered()).unwrap_err();
    assert!(
        error.contains("does not run a session of this project"),
        "{error}"
    );
    let sandbox = CloudBackendKind::DockerSandbox;
    let error = resolve_target(&request(sandbox, Instance::New), &offered()).unwrap_err();
    assert!(error.contains("set up its Docker sandbox"), "{error}");
    let error = resolve_target(&request(kind, Instance::New), &[]).unwrap_err();
    assert!(error.contains("needs a GitHub repository"), "{error}");
    // A codespace name never matches a sandbox of the same name.
    let named = request(sandbox, Instance::Named("octo-abc".into()));
    assert!(resolve_target(&named, &offered()).is_err());
}

#[test]
fn a_new_sandbox_uses_the_saved_setup_the_dialog_offers() {
    let config = DockerProjectConfig::default();
    let offered = vec![MoveTarget::NewSandbox {
        config: config.clone(),
    }];
    let got = resolve_target(
        &request(CloudBackendKind::DockerSandbox, Instance::New),
        &offered,
    )
    .unwrap();
    assert_eq!(got, MoveTarget::NewSandbox { config });
}

#[test]
fn the_status_has_the_dialogs_facts_and_no_secret_or_path() {
    let value = status_json(
        7,
        &record(json!({"startedBy": "api", "warnings": ["LFS files are not uploaded"]})),
        true,
    );
    assert_eq!(value["state"], "running");
    assert_eq!(value["startedBy"], "api");
    assert_eq!(value["direction"], "toCloud");
    assert_eq!(value["step"], "push");
    assert_eq!(value["branch"], "feat");
    let steps = value["steps"].as_array().unwrap();
    assert_eq!(steps.len(), 7);
    assert_eq!(
        steps[0],
        json!({"id": "snapshot", "label": "Snapshot", "state": "done"})
    );
    assert_eq!(steps[1]["state"], "current");
    assert_eq!(steps[2]["state"], "pending");
    let text = value.to_string();
    for secret in [
        "/Users/me",
        "secret-project",
        "claude-work",
        "/workspaces/x",
        "host-1",
        "upload-1",
        "owner-secret",
        "move-7-abc",
    ] {
        assert!(!text.contains(secret), "{secret} leaked: {text}");
    }
    let mut keys: Vec<_> = value.as_object().unwrap().keys().cloned().collect();
    keys.sort();
    assert_eq!(
        keys,
        [
            "branch",
            "direction",
            "notices",
            "session",
            "stale",
            "startedBy",
            "state",
            "step",
            "steps"
        ]
    );
}

#[test]
fn the_status_names_a_failure_and_a_paused_move() {
    let failed = status_json(
        7,
        &record(json!({"failedAt": "push", "current": null, "error": "Push rejected"})),
        false,
    );
    assert_eq!(failed["state"], "failed");
    assert_eq!(failed["error"], "Push rejected");
    assert_eq!(failed["step"], "push");
    assert_eq!(failed["steps"][1]["state"], "failed");
    assert_eq!(status_json(7, &record(json!({})), false)["state"], "paused");
    let named = status_json(
        7,
        &record(
            json!({"workspace": {"backend": "codespaces", "name": "octo-new1", "created": true}}),
        ),
        true,
    );
    assert_eq!(
        named["target"],
        json!({"kind": "codespace", "instance": "octo-new1"})
    );
    assert_eq!(
        idle_json(7, false, None),
        json!({"session": 7, "state": "none", "location": "local"})
    );
}

#[test]
fn paths_are_scrubbed_from_every_error_the_status_can_carry() {
    for (raw, expected) in [
        (
            "Could not read /Users/me/secret-project/.git/index: permission denied",
            "Could not read <path> permission denied",
        ),
        (
            "failed: '/Users/me/Library/Application Support/com.monoptah.desktop/x.json' is bad",
            "failed: '<path>' is bad",
        ),
        ("cannot open ~/work/repo", "cannot open <path>"),
        ("bad (C:\\Users\\me\\repo\\file)", "bad (<path>)"),
        ("path=/tmp/x/y", "path=<path>"),
        // Not paths: a URL, a branch and a remote ref stay.
        (
            "Push to https://github.com/octo/widgets.git of origin/feat-cloud failed",
            "Push to https://github.com/octo/widgets.git of origin/feat-cloud failed",
        ),
    ] {
        assert_eq!(scrub_paths(raw), expected, "{raw}");
    }
    let failed = status_json(
        7,
        &record(json!({
            "failedAt": "push",
            "current": null,
            "error": "cannot write /Users/me/Library/Application Support/com.monoptah.desktop/cloud-move/x",
            "notices": ["kept /Users/me/secret-project-wt"],
        })),
        false,
    );
    let text = failed.to_string();
    assert!(!text.contains("/Users"), "{text}");
    assert!(!text.contains("Application Support"), "{text}");
    assert!(
        !return_json(5, "failed", Some("no /Users/me/x"))
            .to_string()
            .contains("/Users"),
        "return errors are scrubbed"
    );
    assert!(!idle_json(5, true, Some("see /Users/me/x"))
        .to_string()
        .contains("/Users"));
}
