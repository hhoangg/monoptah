//! The Claude setup start step, end to end against a scripted Host. Child of
//! `startup_tests`, which owns the scripted `gh` and the service fixture.
use super::*;
use crate::cloud_sessions::claude_setup::{
    self, local_changed, parse_health, HealthStatus, KitSeams, Scrub, ServerHealth, SetupPlan,
    APPROVAL_LABEL, SIGN_IN_LABEL,
};
use crate::cloud_sessions::kit::{build_inventory, KitRoots, Workspace};
use crate::cloud_sessions::kit_prefs::{ApiKit, CloudKitPrefs};

// Every value below is fake.
const TOKEN: &str = "fake-token-AAAA1111";
const HEADER: &str = "Bearer fake-header-BBBB2222";
const QUERY: &str = "fake-query-CCCC3333";
const SECRETS: [&str; 3] = [TOKEN, HEADER, QUERY];

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

/// A fake home with one skill and four MCP servers (two with secrets).
struct Home {
    _dir: tempfile::TempDir,
    roots: KitRoots,
}
fn home() -> Home {
    let dir = tempfile::tempdir().unwrap();
    let home = std::fs::canonicalize(dir.path()).unwrap();
    let project = home.join("work/app");
    std::fs::create_dir_all(&project).unwrap();
    write(&home.join(".claude/skills/review/SKILL.md"), "# review");
    write(
        &home.join(".claude.json"),
        &json!({"mcpServers": {
            "github": {"command": "npx", "args": ["gh"], "env": {"GITHUB_TOKEN": TOKEN}},
            "site": {"type": "http", "url": "https://site.example/mcp",
                     "headers": {"Authorization": HEADER}},
            "q": {"type": "http", "url": format!("https://q.example/mcp?key={QUERY}")},
            "linear": {"type": "http", "url": "https://mcp.linear.example/mcp"},
        }})
        .to_string(),
    );
    let roots = KitRoots {
        account_dir: None,
        claude_home: home.join(".claude"),
        project,
        key_dir: Some(home.join("app-data")),
        home,
    };
    Home { _dir: dir, roots }
}

fn on() -> CloudKitPrefs {
    CloudKitPrefs {
        enabled: true,
        notice_acknowledged: true,
        selection: None,
    }
}

fn seams(home: &Home, prefs: CloudKitPrefs) -> KitSeams {
    let roots = home.roots.clone();
    KitSeams {
        prefs: Arc::new(move || prefs.clone()),
        roots: Arc::new(move |_| Ok(roots.clone())),
    }
}

fn kit_fx(home: &Home, prefs: CloudKitPrefs) -> Fx {
    let mut f = fx(Gh::new(&["Available"], &[]));
    f.svc = f.svc.with_kit_seams(seams(home, prefs));
    *f.host.kit.lock().unwrap() = Some(KitHost::new());
    f
}

fn kit<R>(f: &Fx, read: impl FnOnce(&KitHost) -> R) -> R {
    read(f.host.kit.lock().unwrap().as_ref().unwrap())
}
fn script(f: &Fx, change: impl FnOnce(&mut KitHost)) {
    change(f.host.kit.lock().unwrap().as_mut().unwrap());
}

fn all_text(events: &[StartEvent]) -> String {
    format!("{events:?}")
}

fn tar_paths(bytes: &[u8]) -> Vec<String> {
    tar::Archive::new(bytes)
        .entries()
        .unwrap()
        .map(|e| e.unwrap().path().unwrap().to_string_lossy().into_owned())
        .collect()
}

fn state(f: &Fx) -> claude_setup::SetupState {
    f.svc
        .claude_setup_state(&CloudRef::codespace("octo-new1"))
        .expect("a stored setup state")
}

#[test]
fn a_new_start_runs_the_claude_setup_before_ready() {
    let home = home();
    let f = kit_fx(&home, on());
    script(&f, |k| {
        k.health = json!({"kitServers": true, "timedOut": false, "exitCode": 0, "servers": [
            {"name": "github", "status": "connected", "signInNeeded": false, "transport": "stdio", "detail": "Connected"},
            {"name": "linear", "status": "signInNeeded", "signInNeeded": true, "transport": "http", "detail": "Needs authentication"},
        ]});
    });
    let (session, _) = f.svc.begin_start("/p", new_target(), "claude").unwrap();
    let (result, events) = f.run(session.id);
    result.unwrap();
    // Order: probe, upload, apply, secrets, health, all before the project's evidence/Ready.
    let methods = kit(&f, |k| k.methods.clone());
    let at = |m: &str| methods.iter().position(|x| x == m).unwrap();
    assert!(at("kit.probe") < at("uploads.begin"));
    assert!(at("uploads.commit") < at("kit.apply"));
    assert!(at("kit.apply") < at("kit.secrets"));
    assert!(at("kit.secrets") < at("kit.health"));
    assert_eq!(
        methods.iter().filter(|m| *m == "kit.apply").count(),
        1,
        "{methods:?}"
    );
    // The tar holds the skill and the config; the hash sent is the tar's own.
    let tar = kit(&f, |k| k.applied[0].clone());
    let paths = tar_paths(&tar);
    assert!(
        paths.contains(&"skills/review/SKILL.md".to_string()),
        "{paths:?}"
    );
    assert!(paths.contains(&"mcp.json".to_string()));
    assert_eq!(kit(&f, |k| k.hash_matched.clone()), [true]);
    for secret in SECRETS {
        assert!(!tar.windows(secret.len()).any(|w| w == secret.as_bytes()));
    }
    // The secrets went through kit.secrets, once.
    let secrets = kit(&f, |k| k.secrets.clone());
    assert_eq!(secrets.len(), 1);
    assert_eq!(secrets[0]["MONOPTAH_KIT_GITHUB_GITHUB_TOKEN"], TOKEN);
    // Health asked for the project that was just opened.
    assert_eq!(
        kit(&f, |k| k.health_params.clone())[0]["projectId"],
        "project-1"
    );
    // The step is Done in the last list shown before Ready.
    let Some(StartEvent::Updated(last)) = events.iter().rev().nth(1) else {
        panic!("{events:?}");
    };
    assert_eq!(status(last, StepKind::ClaudeSetup), StepStatus::Done);
    assert!(matches!(events.last(), Some(StartEvent::Ready { .. })));
    // Health is recorded for the panel; the OAuth-style server says to sign in.
    let stored = state(&f);
    assert!(stored.applied);
    assert_eq!(stored.summary, "1 skill, 4 MCP servers");
    let health = |name: &str| {
        stored
            .health
            .iter()
            .find(|h| h.name == name)
            .unwrap()
            .clone()
    };
    assert_eq!(health("github").label(), "connected");
    assert_eq!(health("linear").label(), SIGN_IN_LABEL);
    assert!(health("linear").oauth_style && health("linear").sign_in_needed);
    // Servers the health output did not mention are listed as not checked.
    assert_eq!(health("site").status, HealthStatus::Configured);
}

#[test]
fn an_attached_codespace_gets_no_kit() {
    let home = home();
    let f = kit_fx(&home, on());
    let (session, record) = f.svc.begin_start("/p", named_target(), "claude").unwrap();
    assert!(!record.created);
    let (result, events) = f.run(session.id);
    result.unwrap();
    assert!(kit(&f, |k| k.methods.is_empty()));
    let Some(StartEvent::Updated(last)) = events.iter().rev().nth(1) else {
        panic!("{events:?}");
    };
    assert_eq!(status(last, StepKind::ClaudeSetup), StepStatus::Skipped);
    assert!(f
        .svc
        .claude_setup_state(&CloudRef::codespace("octo-abc"))
        .is_none());
}

#[test]
fn the_kit_is_off_unless_the_user_turned_it_on_and_accepted_the_notice() {
    let home = home();
    for prefs in [
        CloudKitPrefs::default(),
        CloudKitPrefs {
            enabled: true,
            notice_acknowledged: false,
            selection: None,
        },
        CloudKitPrefs {
            enabled: false,
            notice_acknowledged: true,
            selection: None,
        },
    ] {
        let f = kit_fx(&home, prefs.clone());
        let (session, _) = f.svc.begin_start("/p", new_target(), "claude").unwrap();
        f.run(session.id).0.unwrap();
        assert!(kit(&f, |k| k.methods.is_empty()), "{prefs:?}");
    }
}

#[test]
fn the_per_start_switch_and_the_api_can_only_turn_the_kit_off() {
    let home = home();
    let f = kit_fx(&home, on());
    let (session, record) = f.svc.begin_start("/p", new_target(), "claude").unwrap();
    assert!(!record.kit_off);
    f.svc.set_start_kit_off(session.id).unwrap();
    assert!(f.record(session.id).unwrap().kit_off);
    f.run(session.id).0.unwrap();
    assert!(kit(&f, |k| k.methods.is_empty()));
    // A session that is not starting cannot be switched.
    assert!(f.svc.set_start_kit_off(session.id).is_err());
    // The default API value changes nothing: the user's choice decides.
    assert!(on().applies(ApiKit::Default) && !on().applies(ApiKit::None));
}

#[test]
fn only_a_start_that_created_its_workspace_and_has_not_finished_the_step_sends() {
    let home = home();
    let f = kit_fx(&home, on());
    let (_, mut record) = f.svc.begin_start("/p", new_target(), "claude").unwrap();
    assert!(f.svc.setup_plan(&record).is_some());
    // A retry after the step finished never sends the credentials again (rule D7).
    record.step_mut(StepKind::ClaudeSetup).status = StepStatus::Done;
    assert!(f.svc.setup_plan(&record).is_none());
    record.step_mut(StepKind::ClaudeSetup).status = StepStatus::Pending;
    // An interrupted run starts over: the workspace never reached Ready.
    record.step_mut(StepKind::ClaudeSetup).status = StepStatus::Running;
    assert!(f.svc.setup_plan(&record).is_some());
    record.created = false;
    assert!(f.svc.setup_plan(&record).is_none());
    record.created = true;
    record.kit_off = true;
    assert!(f.svc.setup_plan(&record).is_none());
}

#[test]
fn a_missing_runtime_drops_the_item_with_a_warning() {
    let home = home();
    let f = kit_fx(&home, on());
    script(&f, |k| k.present = vec!["python3"]);
    let (session, _) = f.svc.begin_start("/p", new_target(), "claude").unwrap();
    f.run(session.id).0.unwrap();
    let stored = state(&f);
    assert!(
        stored
            .warnings
            .iter()
            .any(|w| w.contains("github") && w.contains("npx")),
        "{:?}",
        stored.warnings
    );
    let mcp = tar::Archive::new(kit(&f, |k| k.applied[0].clone()).as_slice())
        .entries()
        .unwrap()
        .map(|e| e.unwrap())
        .find(|e| e.path().unwrap().to_str() == Some("mcp.json"))
        .map(|mut e| {
            let mut text = String::new();
            std::io::Read::read_to_string(&mut e, &mut text).unwrap();
            serde_json::from_str::<Value>(&text).unwrap()
        })
        .unwrap();
    assert!(mcp["mcpServers"].get("github").is_none());
    assert!(mcp["mcpServers"].get("linear").is_some());
}

#[test]
fn a_failure_stays_a_warning_and_the_start_continues() {
    let home = home();
    for (method, expect) in [
        ("environment.describe", None),
        ("kit.probe", Some("check the workspace")),
        ("uploads.begin", Some("upload")),
        ("kit.apply", Some("install")),
        ("kit.secrets", Some("secrets")),
        ("kit.health", Some("MCP servers")),
    ] {
        let f = kit_fx(&home, on());
        script(&f, |k| k.errors.push((method, "boom".into())));
        let (session, _) = f.svc.begin_start("/p", new_target(), "claude").unwrap();
        let (result, events) = f.run(session.id);
        result.unwrap_or_else(|e| panic!("{method}: the start failed: {e}"));
        assert!(
            matches!(events.last(), Some(StartEvent::Ready { .. })),
            "{method}: {events:?}"
        );
        let Some(StartEvent::Updated(last)) = events.iter().rev().nth(1) else {
            panic!("{events:?}");
        };
        assert_eq!(
            status(last, StepKind::ClaudeSetup),
            StepStatus::Done,
            "{method}"
        );
        assert!(last.error.is_none() && last.failed_at.is_none());
        let stored = state(&f);
        if let Some(expect) = expect {
            assert!(
                stored.warnings.iter().any(|w| w.contains(expect)),
                "{method}: {:?}",
                stored.warnings
            );
        }
    }
}

#[test]
fn an_old_host_without_the_kit_capability_gets_a_warning_and_nothing() {
    let home = home();
    let f = kit_fx(&home, on());
    script(&f, |k| k.capabilities = vec!["uploads"]);
    let (session, _) = f.svc.begin_start("/p", new_target(), "claude").unwrap();
    f.run(session.id).0.unwrap();
    assert_eq!(kit(&f, |k| k.methods.clone()), ["environment.describe"]);
    assert!(state(&f).warnings[0].contains("too old"));
}

#[test]
fn no_secret_appears_in_the_start_log_the_events_or_the_stored_state() {
    let home = home();
    let f = kit_fx(&home, on());
    // The Host echoes secret values in every place it could: errors, warnings, details.
    script(&f, |k| {
        k.apply_warnings = vec![format!("note {QUERY} {TOKEN}")];
        k.errors
            .push(("kit.secrets", format!("cannot write {TOKEN} or {HEADER}")));
        k.health = json!({"kitServers": true, "timedOut": true, "exitCode": 1, "servers": [
            {"name": "github", "status": "failed", "signInNeeded": false, "transport": "stdio",
             "detail": format!("Failed: token {TOKEN} rejected")},
            {"name": "site", "status": "failed", "signInNeeded": false, "transport": "http",
             "detail": format!("401 for {HEADER}")},
            {"name": "q", "status": "failed", "signInNeeded": false, "transport": "http",
             "detail": format!("https://q.example/mcp?key={QUERY} refused")},
        ], "stdout": format!("github: {TOKEN} - Failed")});
    });
    let (session, _) = f.svc.begin_start("/p", new_target(), "claude").unwrap();
    let (result, events) = f.run(session.id);
    result.unwrap();
    let mut seen = all_text(&events);
    seen.push_str(&serde_json::to_string(&state(&f)).unwrap());
    let store = f.store.lock().unwrap();
    for key in ["cloud-claude-setup.v1", "cloud-starts.v1"] {
        seen.push_str(&store.setting(key).unwrap().unwrap_or_default());
    }
    drop(store);
    for secret in SECRETS {
        assert!(!seen.contains(secret), "{secret} leaked");
    }
    // The warnings still say that something went wrong.
    let stored = state(&f);
    assert!(stored.warnings.iter().any(|w| w.contains("[redacted]")));
}

#[test]
fn update_claude_setup_resends_one_way_and_replaces_the_secrets() {
    let home = home();
    let f = kit_fx(&home, on());
    let (session, _) = f.svc.begin_start("/p", new_target(), "claude").unwrap();
    f.run(session.id).0.unwrap();
    let first = state(&f);
    // The local setup changed: the panel can tell, and an update sends it again.
    write(
        &home.roots.claude_home.join("skills/deploy/SKILL.md"),
        "# deploy",
    );
    let inventory = build_inventory(&home.roots, Workspace::Codespace);
    assert!(local_changed(&first, &inventory, &on()));
    let before = kit(&f, |k| k.applied.len());
    // A reconnect or any other call never sends: only this action does.
    let updated = f
        .svc
        .update_claude_setup(session.id, &AtomicBool::new(false), &mut |_| {})
        .unwrap();
    assert_eq!(kit(&f, |k| k.applied.len()), before + 1);
    assert!(updated.summary.starts_with("2 skills"));
    assert!(!local_changed(&updated, &inventory, &on()));
    // The secrets file is replaced as a whole.
    assert_eq!(kit(&f, |k| k.secrets.len()), 2);
    // Turned off in Settings, the action refuses.
    let off = f.svc.with_kit_seams(seams(&home, CloudKitPrefs::default()));
    assert!(off
        .update_claude_setup(session.id, &AtomicBool::new(false), &mut |_| {})
        .unwrap_err()
        .contains("off"));
}

#[test]
fn the_state_goes_away_with_its_workspace_and_never_prints_the_token() {
    let home = home();
    let f = kit_fx(&home, on());
    let (session, _) = f.svc.begin_start("/p", new_target(), "claude").unwrap();
    f.run(session.id).0.unwrap();
    let stored = state(&f);
    assert!(stored.token.is_some());
    let debug = format!("{stored:?}");
    assert!(debug.contains("redacted"));
    let cloud = CloudRef::codespace("octo-new1");
    f.store.lock().unwrap().forget_cloud(&cloud).unwrap();
    assert!(f.svc.claude_setup_state(&cloud).is_none());
}

// ---- sandbox versus codespace, at the step itself

fn run_step(f: &Fx, home: &Home, workspace: Workspace) -> claude_setup::SetupState {
    f.host.machines.lock().unwrap().push(machine("octo-new1"));
    let plan = SetupPlan {
        seams: seams(home, on()),
        prefs: on(),
        workspace,
        replace_secrets: false,
    };
    let cloud = CloudRef::codespace("octo-new1");
    claude_setup::run(
        &claude_setup::Env {
            host: &*f.host,
            store: &f.store,
            halt: &AtomicBool::new(false),
            now: 1,
        },
        &claude_setup::Target {
            local_project: "/p",
            cloud: &cloud,
            machine_id: "m-octo-new1",
            project_id: Some("project-1"),
        },
        &plan,
        &mut |_| {},
    )
}

fn failing_site() -> Value {
    json!({"kitServers": true, "timedOut": false, "exitCode": 0, "servers": [
        {"name": "site", "status": "failed", "signInNeeded": false, "transport": "http", "detail": "connection refused"},
    ]})
}

#[test]
fn a_sandbox_adds_network_policy_hints_and_a_codespace_does_not() {
    let home = home();
    let sandbox = {
        let f = kit_fx(&home, on());
        script(&f, |k| k.health = failing_site());
        run_step(&f, &home, Workspace::Sandbox)
    };
    let codespace = {
        let f = kit_fx(&home, on());
        script(&f, |k| k.health = failing_site());
        run_step(&f, &home, Workspace::Codespace)
    };
    let hint = |s: &claude_setup::SetupState| {
        s.warnings
            .iter()
            .find(|w| w.contains("sbx policy allow"))
            .cloned()
    };
    let hint_text = hint(&sandbox).expect("a hint on a sandbox");
    assert!(
        hint_text.contains("sbx policy allow network site.example"),
        "{hint_text}"
    );
    assert!(hint_text.contains("--allow-network"));
    assert_eq!(hint(&codespace), None);
}

#[test]
fn a_docker_server_is_sent_to_a_codespace_but_not_to_a_sandbox() {
    let home = home();
    write(
        &home.roots.home.join(".claude.json"),
        &json!({"mcpServers": {"dock": {"command": "docker", "args": ["run", "x"]}}}).to_string(),
    );
    std::fs::remove_dir_all(home.roots.claude_home.join("skills")).unwrap();
    let servers_of = |workspace| {
        let f = kit_fx(&home, on());
        script(&f, |k| k.present = vec!["docker"]);
        let state = run_step(&f, &home, workspace);
        (state.applied, kit(&f, |k| k.applied.len()))
    };
    // Only the Docker server was selected: a sandbox has nothing to send.
    assert_eq!(servers_of(Workspace::Codespace), (true, 1));
    assert_eq!(servers_of(Workspace::Sandbox), (false, 0));
}

// ---- health parsing

#[test]
fn health_comes_from_the_host_list_or_the_raw_output() {
    let scrub = Scrub::from_values_for_test(&[]);
    let (servers, notes) = parse_health(
        &json!({"servers": [
            {"name": "a", "status": "connected", "signInNeeded": false, "transport": "stdio", "detail": "ok"},
            {"name": "b", "status": "signInNeeded", "signInNeeded": true, "transport": "http", "detail": "Needs authentication"},
            {"name": "c", "status": "signInNeeded", "signInNeeded": true, "transport": "stdio", "detail": "pending approval"},
            {"name": "d", "status": "failed", "signInNeeded": false, "transport": "stdio", "detail": "npx: not found"},
        ], "timedOut": true}),
        &scrub,
    );
    let labels: Vec<_> = servers.iter().map(ServerHealth::label).collect();
    assert_eq!(
        labels,
        [
            "connected".to_string(),
            SIGN_IN_LABEL.to_string(),
            APPROVAL_LABEL.to_string(),
            "failed - npx: not found".to_string(),
        ]
    );
    assert!(servers[3].is_problem() && !servers[1].is_problem());
    assert!(notes.iter().any(|n| n.contains("timed out")));
    // Without the Host's list, the raw `claude mcp list` text is parsed.
    let (servers, _) = parse_health(
        &json!({"servers": [], "stdout": "Checking MCP server health...\ndocs: https://x (HTTP) - \u{2713} Connected\nlinear: https://y (HTTP) - Needs authentication\nbroken: npx broken - \u{2717} Failed to connect\n"}),
        &scrub,
    );
    let got: Vec<_> = servers
        .iter()
        .map(|s| (s.name.as_str(), s.status))
        .collect();
    assert_eq!(
        got,
        [
            ("docs", HealthStatus::Connected),
            ("linear", HealthStatus::SignInNeeded),
            ("broken", HealthStatus::Failed),
        ]
    );
}

#[test]
fn an_old_start_record_without_the_step_still_loads_and_ends_with_it_skipped() {
    let record: StartRecord = serde_json::from_str(OLD_START_RECORD).unwrap();
    assert!(!record.kit_off);
    let f = fx(Gh::new(&["Available"], &[]));
    f.svc.persist_start(&record);
    f.store
        .lock()
        .unwrap()
        .save_setting("cloud-starts.v1", &format!("{{\"4\":{OLD_START_RECORD}}}"))
        .unwrap();
    let loaded = start_record(&f.store.lock().unwrap(), 4).unwrap();
    assert_eq!(loaded.steps.len(), StepKind::ALL.len());
    // It predates the step: it never sends, whatever the preferences say.
    assert_eq!(status(&loaded, StepKind::ClaudeSetup), StepStatus::Skipped);
    assert!(f.svc.setup_plan(&loaded).is_none());
    let mut ended = loaded.clone();
    ended.skip_unrun_claude_setup(2000);
    assert_eq!(status(&ended, StepKind::ClaudeSetup), StepStatus::Skipped);
}

#[test]
fn the_panel_view_reports_a_local_change_and_whether_update_is_on_offer() {
    let home = home();
    let f = kit_fx(&home, on());
    let (session, _) = f.svc.begin_start("/p", new_target(), "claude").unwrap();
    f.run(session.id).0.unwrap();
    let view = f.svc.claude_setup_view(session.id).unwrap();
    assert_eq!(view.cloud, CloudRef::codespace("octo-new1"));
    assert!(view.can_update && !view.changed);
    // A skill added locally after the start: "changed since start".
    write(
        &home.roots.claude_home.join("skills/deploy/SKILL.md"),
        "# deploy",
    );
    assert!(f.svc.claude_setup_view(session.id).unwrap().changed);
    // With the kit off in Settings the view stays but Update is not offered.
    let off = f.svc.with_kit_seams(seams(&home, CloudKitPrefs::default()));
    let view = off.claude_setup_view(session.id).unwrap();
    assert!(!view.can_update);
    // A session that never had a setup has no view.
    assert!(off.claude_setup_view(session.id + 100).is_none());
}

#[test]
fn the_kit_choice_is_part_of_the_first_persisted_record() {
    let home = home();
    let f = kit_fx(&home, on());
    // No second write: the record read back right after `begin` already says off.
    let (session, record) = f
        .svc
        .begin_start_with_kit("/p", new_target(), "claude", false)
        .unwrap();
    assert!(record.kit_off);
    assert!(f.record(session.id).unwrap().kit_off);
    f.run(session.id).0.unwrap();
    assert!(kit(&f, |k| k.methods.is_empty()));
    // `true` leaves the decision to Settings.
    let (session, record) = f
        .svc
        .begin_start_with_kit("/p", new_target(), "claude", true)
        .unwrap();
    assert!(!record.kit_off && !f.record(session.id).unwrap().kit_off);
}

#[test]
fn a_halted_setup_is_not_recorded_as_done_so_a_retry_runs_it() {
    use std::sync::atomic::Ordering;
    let home = home();
    let f = kit_fx(&home, on());
    let halt = Arc::new(AtomicBool::new(false));
    let flag = halt.clone();
    script(&f, |k| {
        k.on_call.push((
            "kit.probe",
            Arc::new(move || flag.store(true, Ordering::SeqCst)),
        ))
    });
    f.host.machines.lock().unwrap().push(machine("octo-new1"));
    let (backend, cloud) = (
        f.svc.backend(CloudBackendKind::Codespaces).unwrap(),
        CloudRef::codespace("octo-new1"),
    );
    let request = OpenRequest {
        local_cwd: "/p",
        cloud: &cloud,
        sandbox: None,
        display: "d",
        repo: "octo/widgets",
        remote_cwd: "/workspaces/widgets",
        auth: None,
        setup: Some(SetupPlan {
            seams: seams(&home, on()),
            prefs: on(),
            workspace: Workspace::Codespace,
            replace_secrets: false,
        }),
    };
    let mut steps = vec![];
    let result = super::super::open_project(
        &*backend,
        &*f.host,
        &f.store,
        &request,
        &halt,
        &mut |kind, status| steps.push((kind, status)),
        &mut |_| {},
    );
    assert_eq!(result.unwrap_err(), START_ABORTED);
    assert!(steps.contains(&(StepKind::ClaudeSetup, StepStatus::Running)));
    assert!(!steps.contains(&(StepKind::ClaudeSetup, StepStatus::Done)));
}

#[test]
fn a_secret_changed_after_the_scan_is_still_scrubbed_from_host_errors() {
    let home = home();
    let f = kit_fx(&home, on());
    const LATE: &str = "late-token-DDDD4444";
    // Between the scan and the pack the config gets a new secret value.
    let path = home.roots.home.join(".claude.json");
    let late_config = std::fs::read_to_string(&path).unwrap().replace(TOKEN, LATE);
    script(&f, |k| {
        k.on_call.push((
            "kit.probe",
            Arc::new(move || std::fs::write(&path, &late_config).unwrap()),
        ));
        k.errors
            .push(("kit.secrets", format!("cannot write {LATE}")));
    });
    let (session, _) = f.svc.begin_start("/p", new_target(), "claude").unwrap();
    f.run(session.id).0.unwrap();
    let stored = state(&f);
    // The late value was the one sent, and it never shows in what we keep.
    assert_eq!(kit(&f, |k| k.secrets.len()), 0);
    let text = serde_json::to_string(&stored).unwrap();
    assert!(!text.contains(LATE), "{text}");
    assert!(stored.warnings.iter().any(|w| w.contains("secrets")));
}

#[test]
fn a_secret_too_short_to_mask_hides_host_text_instead_of_trusting_the_scrub() {
    let home = home();
    write(
        &home.roots.home.join(".claude.json"),
        &json!({"mcpServers": {"short": {"command": "npx", "env": {"K": "xy1"}}}}).to_string(),
    );
    let f = kit_fx(&home, on());
    script(&f, |k| {
        k.errors.push(("kit.secrets", "rejected xy1 here".into()));
        k.apply_warnings = vec!["note xy1".into()];
    });
    let (session, _) = f.svc.begin_start("/p", new_target(), "claude").unwrap();
    let (result, events) = f.run(session.id);
    result.unwrap();
    let mut seen = all_text(&events);
    seen.push_str(&serde_json::to_string(&state(&f)).unwrap());
    assert!(!seen.contains("xy1"), "{seen}");
    assert!(seen.contains("too short to mask"));
}
