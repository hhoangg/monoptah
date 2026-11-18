//! Pure tests of the start options and the target mapping (H-13, H-11).
use super::*;
use monocode_core::cloud_sessions::kit_prefs::ApiKit;

fn options() -> StartOptions {
    StartOptions {
        codespace_ready: true,
        sandbox_ready: false,
        branches: vec!["main".into(), "feat/x".into()],
        machines: vec![("basicLinux32gb".into(), "2 cores".into())],
        sizes: size_presets(),
    }
}

fn target(kind: StartKind) -> StartTarget {
    StartTarget {
        kind,
        instance: StartInstance::New,
        branch: None,
        machine: None,
        size: None,
    }
}

#[test]
fn the_result_has_the_shape_of_the_protocol_and_leaves_out_empty_lists() {
    let value = options().to_value();
    assert_eq!(
        value["backends"],
        json!([
            {"kind": "codespace", "ready": true},
            {"kind": "sandbox", "ready": false},
        ])
    );
    assert_eq!(value["branches"], json!(["main", "feat/x"]));
    assert_eq!(
        value["machines"],
        json!([{"id": "basicLinux32gb", "name": "2 cores"}])
    );
    assert_eq!(value["sizes"].as_array().unwrap().len(), SHAPES.len());
    let bare = StartOptions::default().to_value();
    for key in ["branches", "machines", "sizes"] {
        assert!(bare.get(key).is_none(), "{key} is left out when empty");
    }
}

#[test]
fn the_sandbox_presets_are_the_shapes_the_cloud_accepts() {
    for (id, name) in size_presets() {
        let (cpus, memory) = shape_of(&id).unwrap_or_else(|| panic!("{id} is not a shape"));
        assert_eq!(id, size_id(cpus, memory));
        assert!(!name.is_empty());
    }
    assert_eq!(shape_of("2cpu-4g"), Some((2, "4g")));
    assert_eq!(shape_of("3cpu-5g"), None);
    assert_eq!(shape_of("2cpu-4g; rm"), None);
}

#[test]
fn a_start_may_pick_only_what_was_offered() {
    let offered = options();
    let mut start = target(StartKind::Codespace);
    assert_eq!(offered.check(&start, &[]), Ok(()));
    start.branch = Some("feat/x".into());
    start.machine = Some("basicLinux32gb".into());
    assert_eq!(offered.check(&start, &[]), Ok(()));

    for (branch, machine) in [
        (Some("not-offered"), None),
        (None, Some("premiumLinux")),
        (Some("main"), Some("premiumLinux")),
    ] {
        let mut start = target(StartKind::Codespace);
        start.branch = branch.map(str::to_owned);
        start.machine = machine.map(str::to_owned);
        assert_eq!(
            offered.check(&start, &[]),
            Err(Code::Forbidden),
            "{branch:?}"
        );
    }

    let mut sandbox = target(StartKind::Sandbox);
    sandbox.size = Some("2cpu-4g".into());
    assert_eq!(offered.check(&sandbox, &[]), Ok(()));
    sandbox.size = Some("64cpu-256g".into());
    assert_eq!(offered.check(&sandbox, &[]), Err(Code::Forbidden));
}

#[test]
fn an_empty_list_offers_nothing_to_pick() {
    // No cached machine list: a client cannot name a machine.
    let mut start = target(StartKind::Codespace);
    start.machine = Some("basicLinux32gb".into());
    assert_eq!(
        StartOptions::default().check(&start, &[]),
        Err(Code::Forbidden)
    );
}

#[test]
fn an_existing_instance_must_be_one_the_project_uses_and_takes_no_options() {
    let used = vec!["octo-widgets-x7q9".to_owned()];
    let mut start = target(StartKind::Codespace);
    start.instance = StartInstance::Named("octo-widgets-x7q9".into());
    assert_eq!(options().check(&start, &used), Ok(()));
    assert_eq!(options().check(&start, &[]), Err(Code::UnsupportedTarget));
    start.instance = StartInstance::Named("someone-elses".into());
    assert_eq!(options().check(&start, &used), Err(Code::UnsupportedTarget));
    start.instance = StartInstance::Named("octo-widgets-x7q9".into());
    start.branch = Some("main".into());
    assert_eq!(options().check(&start, &used), Err(Code::Forbidden));
}

#[test]
fn the_cloud_target_carries_only_the_picked_options_and_the_desktop_kit_choice() {
    let mut start = target(StartKind::Codespace);
    start.branch = Some("main".into());
    start.machine = Some("basicLinux32gb".into());
    let target = CloudStartPlan { target: start }.cloud_target().unwrap();
    assert_eq!(target.kind, CloudBackendKind::Codespaces);
    assert_eq!(target.instance, InstanceChoice::New);
    assert_eq!(target.option("branch"), Some("main"));
    assert_eq!(target.option("machine"), Some("basicLinux32gb"));
    // The kit follows the saved preference: the API may only narrow it.
    assert_eq!(target.kit, ApiKit::Default);
    assert_eq!(target.account, None);

    let mut start = target_of(StartKind::Sandbox);
    start.size = Some("4cpu-8g".into());
    let target = CloudStartPlan { target: start }.cloud_target().unwrap();
    assert_eq!(target.kind, CloudBackendKind::DockerSandbox);
    assert_eq!(target.options["cpus"], json!(4));
    assert_eq!(target.options["memory"], json!("8g"));
    for key in ["kit", "template", "image", "ttl"] {
        assert!(!target.options.contains_key(key), "{key}");
    }

    let mut bad = target_of(StartKind::Sandbox);
    bad.size = Some("not-a-size".into());
    assert_eq!(
        CloudStartPlan { target: bad }.cloud_target(),
        Err(Code::Forbidden)
    );
}

fn target_of(kind: StartKind) -> StartTarget {
    target(kind)
}

#[test]
fn only_plain_branch_names_are_listed() {
    for good in ["main", "feat/x", "release-1.2"] {
        assert!(listed_branch(good), "{good}");
    }
    for bad in ["", "-flag", "has space", "tab\there", &"x".repeat(201)] {
        assert!(!listed_branch(bad), "{bad:?}");
    }
}
