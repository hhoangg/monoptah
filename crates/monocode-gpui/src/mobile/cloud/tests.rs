//! Pure tests of the cloud views (H-09) and the guards of the cloud module
//! (D7). Tests that need a shell are with the mobile host's.
use super::view::{Binding, CardView, Extras, Facts, Placement, StartupView, view_of};
use crate::cloud_start_view::fixtures::record;
use monocode_core::cloud_sessions::{CloudBackendKind, CloudState, StepKind, StepStatus};
use monocode_core::mobile::cloud_policy::{STARTUP_ERROR_MAX_CHARS, WEB_ONLY};
use serde_json::{Value, json};

fn binding(kind: CloudBackendKind, state: CloudState) -> Binding {
    Binding {
        local_project: "/work/widgets".into(),
        instance: "octo-widgets-x7q9".into(),
        kind,
        state,
    }
}

fn annotated(view: &CardView) -> Value {
    let mut card = json!({"id": "s", "projectId": "p"});
    view.annotate(&mut card);
    card
}

#[test]
fn a_codespace_card_has_target_state_and_no_startup_when_ready() {
    let view = CardView::cloud(
        &binding(CloudBackendKind::Codespaces, CloudState::Available),
        None,
        true,
    );
    assert_eq!(
        annotated(&view),
        json!({
            "id": "s", "projectId": "p",
            "target": {"kind": "codespace", "instance": "octo-widgets-x7q9", "state": "available"},
            "cloudState": "available",
            WEB_ONLY: true,
        })
    );
}

#[test]
fn a_sandbox_card_names_its_kind() {
    let view = CardView::cloud(
        &binding(CloudBackendKind::DockerSandbox, CloudState::Stopped),
        None,
        false,
    );
    let card = annotated(&view);
    assert_eq!(card["target"]["kind"], "sandbox");
    assert_eq!(card["cloudState"], "stopped");
    assert!(card.get(WEB_ONLY).is_none());
}

#[test]
fn a_local_card_says_local_and_nothing_else() {
    let view = CardView::local();
    assert_eq!(annotated(&view)["target"], json!({"kind": "local"}));
    assert!(annotated(&view).get("cloudState").is_none());
    assert_eq!(view.cloud_field(&Extras::default()), None);
    assert!(view.fields(&Extras::default()).is_empty());
}

#[test]
fn a_remote_machine_card_claims_no_state() {
    let view = CardView::remote("env-box".into(), true);
    let card = annotated(&view);
    assert_eq!(
        card["target"],
        json!({"kind": "remote", "instance": "env-box"})
    );
    assert!(card.get("cloudState").is_none());
    assert_eq!(card[WEB_ONLY], true);
    let field = view.cloud_field(&Extras::default()).unwrap();
    assert_eq!(field["kind"], "remote");
    assert_eq!(field["state"], "unknown");
}

#[test]
fn the_state_is_starting_while_a_start_runs_and_the_cached_state_after_it_fails() {
    let running = record(2);
    let view = CardView::cloud(
        &binding(CloudBackendKind::Codespaces, CloudState::Unknown),
        Some(&running),
        true,
    );
    assert_eq!(view.state, Some(CloudState::Starting));
    let card = annotated(&view);
    assert_eq!(card["cloudState"], "starting");
    assert_eq!(card["startup"], json!({"step": "buildContainer"}));

    let mut failed = record(2);
    failed.steps[2].status = StepStatus::Failed;
    failed.failed_at = Some(StepKind::BuildContainer);
    failed.error = Some("Build failed in /workspaces/widgets: see https://example.test/log".into());
    let view = CardView::cloud(
        &binding(CloudBackendKind::Codespaces, CloudState::Stopped),
        Some(&failed),
        true,
    );
    assert_eq!(view.state, Some(CloudState::Stopped));
    let card = annotated(&view);
    assert_eq!(card["cloudState"], "stopped");
    assert_eq!(
        card["startup"],
        json!({"step": "buildContainer", "error": "Build failed in see"})
    );
}

#[test]
fn the_cloud_field_lists_steps_in_milliseconds_without_detail() {
    let mut running = record(1);
    running.steps[0].detail = Some("octo-widgets-x7q9".into());
    let view = CardView::cloud(
        &binding(CloudBackendKind::Codespaces, CloudState::Unknown),
        Some(&running),
        false,
    );
    let field = view.cloud_field(&Extras::default()).unwrap();
    assert_eq!(field["kind"], "codespace");
    assert_eq!(field["instance"], "octo-widgets-x7q9");
    assert_eq!(field["state"], "starting");
    let startup = &field["startup"];
    assert_eq!(startup["current"], "provision");
    assert_eq!(startup["steps"].as_array().unwrap().len(), 9);
    assert_eq!(
        startup["steps"][0],
        json!({"id": "create", "status": "done", "startedAt": 1000, "finishedAt": 6000})
    );
    assert_eq!(
        startup["steps"][1],
        json!({"id": "provision", "status": "running", "startedAt": 11000, "finishedAt": null})
    );
    assert!(startup.get("error").is_none());
    assert!(!field.to_string().contains("detail"));
}

#[test]
fn extras_plug_into_the_cloud_field() {
    let view = CardView::cloud(
        &binding(CloudBackendKind::Codespaces, CloudState::Available),
        None,
        false,
    );
    let extras = Extras {
        kept: true,
        auto_stop: Some(super::AutoStop {
            at: 1_700_000_000_000,
            reason: "idle".into(),
        }),
        moving: Some(super::MoveStep {
            step: "push",
            status: "running",
        }),
    };
    let field = view.cloud_field(&extras).unwrap();
    assert_eq!(field["kept"], true);
    assert_eq!(
        field["autoStop"],
        json!({"at": 1_700_000_000_000u64, "reason": "idle"})
    );
    assert_eq!(field["move"], json!({"step": "push", "status": "running"}));
    let plain = view.cloud_field(&Extras::default()).unwrap();
    for key in ["kept", "autoStop", "move", "startup"] {
        assert!(plain.get(key).is_none(), "{key}");
    }
}

#[test]
fn view_of_chooses_cloud_then_remote_then_local() {
    let facts = |binding: Option<Binding>, remote_machine: bool| Facts {
        cwd: "remote://env-box/home/me/app".into(),
        binding,
        start: None,
        remote_machine,
    };
    let cloud = view_of(
        &facts(
            Some(binding(CloudBackendKind::Codespaces, CloudState::Available)),
            true,
        ),
        false,
    );
    assert_eq!(cloud.placement, Placement::Codespace);
    assert!(cloud.is_cloud());
    let remote = view_of(&facts(None, true), true);
    assert_eq!(remote.placement, Placement::Remote);
    assert_eq!(remote.instance.as_deref(), Some("env-box"));
    assert!(!remote.is_cloud());
    assert_eq!(view_of(&facts(None, false), false), CardView::local());
}

/// Every value a web device can be sent for a session, for several kinds of
/// session, with secrets planted in everything the views must not copy.
#[test]
fn no_cloud_value_leaks_a_secret_a_path_or_a_command() {
    let token = concat!("gh", "p_", "0123456789abcdefghij0123456789");
    let mut started = record(3);
    started.web_url = Some("https://leak-web-url.example.test/codespace".into());
    started.gh_state = Some("LeakGhState".into());
    started.remote_cwd = "/leak/remote/cwd".into();
    started.log_tail = vec![format!("leak-log-line {token}")];
    started.repo = "leak-owner/leak-repo".into();
    started.branch = Some("leak-branch".into());
    for step in &mut started.steps {
        step.detail = Some("leak-step-detail".into());
    }
    let mut failed = started.clone();
    failed.steps[3].status = StepStatus::Failed;
    failed.failed_at = Some(StepKind::BlockingCommands);
    failed.error = Some(format!(
        "gh codespace ssh --codespace leak-cs failed with {token} at /leak/path/file ssh://leak-ssh-host"
    ));
    let extras = Extras::default();
    let mut values: Vec<Value> = Vec::new();
    for (kind, state, start) in [
        (CloudBackendKind::Codespaces, CloudState::Available, None),
        (CloudBackendKind::DockerSandbox, CloudState::Available, None),
        (CloudBackendKind::Codespaces, CloudState::Stopped, None),
        (
            CloudBackendKind::Codespaces,
            CloudState::Unknown,
            Some(&started),
        ),
        (
            CloudBackendKind::DockerSandbox,
            CloudState::Stopped,
            Some(&failed),
        ),
    ] {
        let view = CardView::cloud(&binding(kind, state), start, true);
        values.push(annotated(&view));
        values.push(view.cloud_field(&extras).unwrap());
        values.push(Value::Object(view.fields(&extras)));
    }
    for view in [CardView::remote("env-box".into(), true), CardView::local()] {
        values.push(annotated(&view));
        values.push(view.cloud_field(&extras).unwrap_or(Value::Null));
    }
    // The startup view of a record is itself a value that must be clean.
    let startup = StartupView::of(&failed);
    values.push(json!(format!("{startup:?}")));
    let text = serde_json::to_string(&values).unwrap();
    for needle in [
        "leak-",
        "Leak",
        "ghp_",
        "https://",
        "ssh://",
        "/leak",
        "log_tail",
        "logTail",
        "web_url",
        "webUrl",
        "remote_cwd",
        "remoteCwd",
        "gh_state",
        "ghState",
        "detail",
        "token",
        "ssh",
    ] {
        assert!(!text.contains(needle), "leaked {needle:?} in {text}");
    }
    // The error that does leave is sanitized and capped.
    let error = values[13]["startup"]["error"].as_str().unwrap().to_owned();
    assert!(error.chars().count() <= STARTUP_ERROR_MAX_CHARS);
}

/// D7: nothing under `mobile/cloud*` may call the functions that delete an
/// instance or pull evidence.
#[test]
fn the_cloud_module_never_names_a_destructive_call() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/mobile");
    let needles = [
        concat!("delete", "_instance("),
        concat!(".", "delete("),
        concat!("pull", "_evidence("),
    ];
    let mut files = vec![root.join("cloud.rs")];
    for entry in std::fs::read_dir(root.join("cloud")).unwrap() {
        files.push(entry.unwrap().path());
    }
    for path in std::fs::read_dir(&root).unwrap().map(|e| e.unwrap().path()) {
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        if name.starts_with("cloud") && name.ends_with(".rs") && !files.contains(&path) {
            files.push(path);
        }
    }
    assert!(files.len() >= 7, "the scan found {files:?}");
    for path in files
        .iter()
        .filter(|p| p.extension().is_some_and(|e| e == "rs"))
    {
        let source = std::fs::read_to_string(path).unwrap();
        for needle in needles {
            assert!(!source.contains(needle), "{path:?} names {needle}");
        }
    }
    // The shell halves of the same methods: the start and move helpers, and the part of the
    // cloud archive file that serves remote devices (the rest is the desktop's own archive).
    let shell = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/shell");
    let start = std::fs::read_to_string(shell.join("mobile_host/cloud_start.rs")).unwrap();
    let archive = std::fs::read_to_string(shell.join("cloud_app_archive.rs")).unwrap();
    let marker = "// MARK: remote devices (WS-D4";
    let remote_part = archive
        .split(marker)
        .nth(1)
        .expect("the remote-device part");
    for (name, source) in [
        ("cloud_start.rs", start.as_str()),
        ("cloud_app_archive.rs", remote_part),
    ] {
        for needle in needles {
            assert!(!source.contains(needle), "{name} names {needle}");
        }
    }
}
