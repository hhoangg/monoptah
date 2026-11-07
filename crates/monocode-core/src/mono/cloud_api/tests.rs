use super::*;
use crate::codespaces::GhOutput;
use crate::remote::Machine;
use std::sync::atomic::AtomicBool;

const AUTH_OK: &str = "github.com\n  ✓ Logged in to github.com account me (keyring)\n  - Token scopes: 'codespace', 'repo'\n";
const AUTH_NO_SCOPE: &str =
    "github.com\n  ✓ Logged in to github.com account me (keyring)\n  - Token scopes: 'repo'\n";
const LIST: &str = r#"[{"name":"octo-abc","displayName":"my cs","state":"Shutdown","machineName":"basicLinux32gb","repository":"octo/widgets","gitStatus":{"ref":"main"}},{"name":"octo-run","displayName":"run","state":"Available","repository":"octo/widgets"}]"#;

/// Replies by the first rule whose text is in the joined argv; records every call.
struct FakeGh {
    rules: Vec<(String, Result<GhOutput, String>)>,
    calls: Mutex<Vec<String>>,
    stdin: Mutex<Vec<String>>,
}
impl FakeGh {
    fn new(rules: Vec<(&str, Result<GhOutput, String>)>) -> Arc<Self> {
        Arc::new(Self {
            rules: rules.into_iter().map(|(k, v)| (k.to_string(), v)).collect(),
            calls: Mutex::new(vec![]),
            stdin: Mutex::new(vec![]),
        })
    }
    fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }
}
impl GhRunner for FakeGh {
    fn run(&self, args: &[String], stdin: Option<&str>) -> Result<GhOutput, String> {
        let line = args.join(" ");
        self.calls.lock().unwrap().push(line.clone());
        // Evidence folder setup runs after every prepare; it is best effort.
        if line.starts_with("codespace ssh") {
            return ok("");
        }
        if let Some(text) = stdin {
            self.stdin.lock().unwrap().push(text.into());
        }
        self.rules
            .iter()
            .find(|(key, _)| line.contains(key.as_str()))
            .map(|(_, reply)| reply.clone())
            .unwrap_or_else(|| panic!("unexpected gh call: {line}"))
    }
}
fn ok(stdout: &str) -> Result<GhOutput, String> {
    Ok(GhOutput {
        success: true,
        stdout: stdout.into(),
        stderr: String::new(),
    })
}

#[derive(Default)]
struct FakeHost {
    machines: Mutex<Vec<Machine>>,
    log: Mutex<Vec<String>>,
}
fn machine(codespace: &str) -> Machine {
    Machine {
        id: format!("m-{codespace}"),
        name: codespace.into(),
        endpoint: format!("codespace://{codespace}"),
        environment_id: format!("env-{codespace}"),
        ssh: None,
        codespace: Some(codespace.into()),
    }
}
impl Host for FakeHost {
    fn machines(&self) -> Result<Vec<Machine>, String> {
        Ok(self.machines.lock().unwrap().clone())
    }
    fn connect(
        &self,
        codespace: &str,
        display: &str,
        _progress: &mut dyn FnMut(&str),
    ) -> Result<Machine, String> {
        self.log
            .lock()
            .unwrap()
            .push(format!("connect {codespace} {display}"));
        let m = machine(codespace);
        self.machines.lock().unwrap().push(m.clone());
        Ok(m)
    }
    fn ensure(&self, id: &str, _: &AtomicBool, _: &mut dyn FnMut(&str)) -> Result<(), String> {
        self.log.lock().unwrap().push(format!("ensure {id}"));
        Ok(())
    }
    fn request(&self, id: &str, method: &str, params: Value) -> Result<Value, String> {
        self.log
            .lock()
            .unwrap()
            .push(format!("request {id} {method}"));
        Ok(json!({"id":"project-1","cwd":params["cwd"]}))
    }
    fn disconnect(&self, id: &str) -> Result<(), String> {
        self.log.lock().unwrap().push(format!("disconnect {id}"));
        self.machines.lock().unwrap().retain(|m| m.id != id);
        Ok(())
    }
}

struct Fixture {
    api: CloudApi,
    host: Arc<FakeHost>,
    store: Database,
    project: tempfile::TempDir,
}
/// A real git project on branch `trunk` whose origin is a GitHub repository.
fn fixture(gh: &Arc<FakeGh>) -> Fixture {
    let project = tempfile::tempdir().unwrap();
    let path = project.path().to_str().unwrap().to_owned();
    for args in [
        vec!["init", "-q", "-b", "trunk"],
        vec!["remote", "add", "origin", "git@github.com:octo/widgets.git"],
        vec![
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "x",
        ],
    ] {
        assert!(std::process::Command::new("git")
            .args(["-C", &path])
            .args(args)
            .status()
            .unwrap()
            .success());
    }
    let host = Arc::new(FakeHost::default());
    let store = Arc::new(Mutex::new(Store::open(Path::new(":memory:")).unwrap()));
    Fixture {
        api: CloudApi::new(gh.clone(), host.clone(), store.clone()),
        host,
        store,
        project,
    }
}
impl Fixture {
    fn path(&self) -> String {
        self.project.path().to_str().unwrap().to_owned()
    }
    fn bind(&self, codespace: &str) -> i64 {
        self.host.machines.lock().unwrap().push(machine(codespace));
        let binding = self
            .api
            .prepare(
                &self.path(),
                Target {
                    codespace: CodespaceChoice::Named(codespace.into()),
                    branch: None,
                    machine: None,
                },
                &mut |_| {},
            )
            .unwrap();
        self.api.create_session(&binding, "claude").unwrap().id
    }
}

#[test]
fn parses_the_target_of_sessions_start() {
    assert_eq!(target_from_input(&json!({"prompt":"x"})), None);
    let new = target_from_input(&json!({"target":{"codespace":"new","branch":"b","machine":"m"}}))
        .unwrap();
    assert_eq!(new.codespace, CodespaceChoice::New);
    assert_eq!(
        (new.branch.as_deref(), new.machine.as_deref()),
        (Some("b"), Some("m"))
    );
    let named = target_from_input(&json!({"target":{"codespace":"octo-abc"}})).unwrap();
    assert_eq!(named.codespace, CodespaceChoice::Named("octo-abc".into()));
}

#[test]
fn list_reports_the_repository_codespaces_with_readable_states() {
    let gh = FakeGh::new(vec![
        ("auth status", ok(AUTH_OK)),
        ("codespace list", ok(LIST)),
    ]);
    let fx = fixture(&gh);
    let listed = fx.api.list(&fx.path()).unwrap();
    assert_eq!(listed["repo"], "octo/widgets");
    let rows = listed["codespaces"].as_array().unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["name"], "octo-abc");
    assert_eq!(rows[0]["state"], "stopped");
    assert_eq!(rows[0]["branch"], "main");
    assert_eq!(rows[1]["state"], "available");
}

#[test]
fn listing_needs_the_codespace_scope_and_says_how_to_get_it() {
    let gh = FakeGh::new(vec![("auth status", ok(AUTH_NO_SCOPE))]);
    let fx = fixture(&gh);
    let error = fx.api.list(&fx.path()).unwrap_err();
    assert!(
        error.contains("gh auth refresh -h github.com -s codespace"),
        "{error}"
    );
}

#[test]
fn create_defaults_to_the_current_branch_and_waits_until_available() {
    let gh = FakeGh::new(vec![
        ("auth status", ok(AUTH_OK)),
        ("codespace create", ok("octo-new1\n")),
        ("codespace view", ok(r#"{"state":"Available"}"#)),
    ]);
    let fx = fixture(&gh);
    let made = fx.api.create(&fx.path(), &json!({}), &mut |_| {}).unwrap();
    assert_eq!(made["codespace"], "octo-new1");
    assert_eq!(made["branch"], "trunk");
    assert_eq!(made["state"], "available");
    let calls = gh.calls();
    let create = calls
        .iter()
        .find(|c| c.contains("codespace create"))
        .unwrap();
    assert!(
        create.contains("--branch trunk") && create.contains("basicLinux32gb"),
        "{create}"
    );
    // Explicit options win.
    let gh = FakeGh::new(vec![
        ("auth status", ok(AUTH_OK)),
        ("codespace create", ok("octo-new2\n")),
        ("codespace view", ok(r#"{"state":"Available"}"#)),
    ]);
    let fx = fixture(&gh);
    fx.api
        .create(
            &fx.path(),
            &json!({"branch":"feat","machine":"standardLinux32gb","displayName":"mine"}),
            &mut |_| {},
        )
        .unwrap();
    let calls = gh.calls();
    let create = calls
        .iter()
        .find(|c| c.contains("codespace create"))
        .unwrap();
    assert!(
        create.contains("--branch feat")
            && create.contains("standardLinux32gb")
            && create.contains("mine"),
        "{create}"
    );
}

#[test]
fn stop_and_delete_only_touch_codespaces_of_the_projects_repository() {
    let gh = FakeGh::new(vec![
        ("auth status", ok(AUTH_OK)),
        ("codespace list", ok(LIST)),
        ("codespace stop", ok("")),
        ("codespace delete", ok("")),
    ]);
    let fx = fixture(&gh);
    let error = fx.api.stop(&fx.path(), "someone-elses").unwrap_err();
    assert!(error.contains("not a codespace of octo/widgets"), "{error}");
    let error = fx.api.delete(&fx.path(), "someone-elses").unwrap_err();
    assert!(error.contains("not a codespace of octo/widgets"), "{error}");
    assert!(!gh
        .calls()
        .iter()
        .any(|c| c.contains("codespace stop") || c.contains("codespace delete")));
    let stopped = fx.api.stop(&fx.path(), "octo-run").unwrap();
    assert_eq!(stopped, json!({"codespace":"octo-run","stopped":true}));
    assert!(gh
        .calls()
        .iter()
        .any(|c| c.contains("codespace stop -c octo-run")));
}

#[test]
fn delete_is_explicit_and_forgets_the_bindings() {
    let gh = FakeGh::new(vec![
        ("auth status", ok(AUTH_OK)),
        ("codespace list", ok(LIST)),
        ("codespace delete", ok("")),
        ("codespace view", ok(r#"{"state":"Available"}"#)),
    ]);
    let fx = fixture(&gh);
    let id = fx.bind("octo-run");
    assert!(is_cloud_session_of(
        &fx.store.lock().unwrap(),
        id,
        &fx.path()
    ));
    let deleted = fx.api.delete(&fx.path(), "octo-run").unwrap();
    assert_eq!(deleted, json!({"codespace":"octo-run","deleted":true}));
    assert!(gh
        .calls()
        .iter()
        .any(|c| c.contains("codespace delete -c octo-run --force")));
    assert!(fx
        .host
        .log
        .lock()
        .unwrap()
        .iter()
        .any(|l| l == "disconnect m-octo-run"));
    assert!(!is_cloud_session_of(
        &fx.store.lock().unwrap(),
        id,
        &fx.path()
    ));
    assert!(fx.api.session_targets(&fx.path()).is_empty());
}

#[test]
fn secrets_status_reports_states_never_values() {
    let gh = FakeGh::new(vec![
        ("auth status", ok(AUTH_OK)),
        (
            "user/codespaces/secrets/NPM_TOKEN/repositories",
            ok(r#"{"repositories":[]}"#),
        ),
        (
            "user/codespaces/secrets/CLAUDE_CODE_OAUTH_TOKEN/repositories",
            ok(r#"{"repositories":[{"full_name":"octo/widgets"}]}"#),
        ),
        (
            "user/codespaces/secrets",
            ok(r#"{"secrets":[{"name":"CLAUDE_CODE_OAUTH_TOKEN","value":"leak-me"}]}"#),
        ),
    ]);
    let fx = fixture(&gh);
    std::fs::create_dir_all(fx.project.path().join(".devcontainer")).unwrap();
    std::fs::write(
        fx.project.path().join(".devcontainer/devcontainer.json"),
        r#"{"secrets":{"NPM_TOKEN":{"description":"npm"}}}"#,
    )
    .unwrap();
    let status = fx.api.secrets_status(&fx.path()).unwrap();
    assert_eq!(status["auth"]["hasCodespaceScope"], true);
    assert_eq!(status["repo"], "octo/widgets");
    assert_eq!(status["devcontainerSecrets"][0]["key"], "NPM_TOKEN");
    let secrets = status["secrets"].as_array().unwrap();
    assert_eq!(
        secrets[0],
        json!({"key":"CLAUDE_CODE_OAUTH_TOKEN","status":"set"})
    );
    assert_eq!(secrets[1], json!({"key":"NPM_TOKEN","status":"missing"}));
    assert!(!status.to_string().contains("leak-me"));
}

#[test]
fn secrets_status_without_the_scope_explains_instead_of_failing() {
    let gh = FakeGh::new(vec![("auth status", ok(AUTH_NO_SCOPE))]);
    let fx = fixture(&gh);
    let status = fx.api.secrets_status(&fx.path()).unwrap();
    assert_eq!(status["auth"]["hasCodespaceScope"], false);
    assert!(status["note"].as_str().unwrap().contains("-s codespace"));
    assert_eq!(status["secrets"], json!([]));
}

#[test]
fn preparing_a_new_target_uses_the_current_branch_by_default() {
    let gh = FakeGh::new(vec![
        ("auth status", ok(AUTH_OK)),
        ("codespace list", ok("[]")),
        (
            "--method POST",
            ok(r#"{"name":"octo-new1","state":"Provisioning"}"#),
        ),
        ("codespace view", ok(r#"{"state":"Available"}"#)),
    ]);
    let fx = fixture(&gh);
    let binding = fx
        .api
        .prepare(
            &fx.path(),
            Target {
                codespace: CodespaceChoice::New,
                branch: None,
                machine: None,
            },
            &mut |_| {},
        )
        .unwrap();
    assert_eq!(binding.codespace, "octo-new1");
    assert_eq!(
        binding.project_key,
        "remote://env-octo-new1/workspaces/widgets"
    );
    assert!(gh.calls().iter().any(|c| c.contains("ref=trunk")));
    // A non-Claude harness never gets that far.
    assert_eq!(
        fx.api.create_session(&binding, "codex").unwrap_err(),
        "Cloud sessions support Claude only for now."
    );
}

#[test]
fn targets_are_local_or_codespace_and_never_wake_anything() {
    let gh = FakeGh::new(vec![
        ("auth status", ok(AUTH_OK)),
        ("codespace list", ok(LIST)),
        ("codespace view", ok(r#"{"state":"Shutdown"}"#)),
    ]);
    let fx = fixture(&gh);
    let cloud = fx.bind("octo-abc");
    let local = fx
        .store
        .lock()
        .unwrap()
        .create("claude", &fx.path())
        .unwrap()
        .id;
    fx.host.log.lock().unwrap().clear();

    let targets = fx.api.session_targets(&fx.path());
    assert_eq!(targets.len(), 1);
    assert_eq!(targets[&cloud].state, CloudState::Stopped);
    assert!(fx.api.target_of(local).is_none());
    assert_eq!(fx.api.target_of(cloud).unwrap().codespace, "octo-abc");
    assert!(
        fx.host.log.lock().unwrap().is_empty(),
        "a listing must not wake or connect"
    );

    let mut row = json!({"id":cloud});
    annotate(&mut row, targets.get(&cloud));
    assert_eq!(
        row,
        json!({"id":cloud,"target":{"kind":"codespace","codespace":"octo-abc","state":"stopped"},"codespaceState":"stopped"})
    );
    let mut row = json!({"id":local});
    annotate(&mut row, targets.get(&local));
    assert_eq!(row, json!({"id":local,"target":{"kind":"local"}}));
}

#[test]
fn a_starting_session_is_listed_as_starting_and_refuses_sends() {
    let gh = FakeGh::new(vec![
        ("auth status", ok(AUTH_OK)),
        ("codespace list", ok(LIST)),
        ("codespace view", ok(r#"{"state":"Available"}"#)),
    ]);
    let fx = fixture(&gh);
    let (session, record) = fx
        .api
        .cloud
        .begin_start(
            &fx.path(),
            Target {
                codespace: CodespaceChoice::Named("octo-abc".into()),
                branch: None,
                machine: None,
            },
            "claude",
        )
        .unwrap();
    let id = session.id;
    fx.host.log.lock().unwrap().clear();
    let calls = gh.calls().len();
    let targets = fx.api.session_targets(&fx.path());
    assert_eq!(targets[&id].state, CloudState::Starting);
    assert_eq!(
        gh.calls().len(),
        calls,
        "a starting session needs no gh read"
    );
    let mut row = json!({"id":id});
    annotate(&mut row, targets.get(&id));
    assert_eq!(
        row,
        json!({"id":id,"target":{"kind":"codespace","codespace":"octo-abc","state":"starting"},
            "codespaceState":"starting","startup":{"step":"provision"}})
    );
    let target = fx.api.target_of(id).unwrap();
    assert_eq!(target.state, CloudState::Starting);
    // Sends are refused with the phase and never touch the host.
    let error = fx.api.wake(id, &fx.store).unwrap_err();
    assert_eq!(
        error,
        "This cloud session is still starting its codespace (Provisioning). Send again when it is ready."
    );
    assert!(fx.host.log.lock().unwrap().is_empty());
    // A failed start reports its error and the real codespace state.
    let mut failed = record;
    failed.error = Some("ssh refused".into());
    failed.failed_at = Some(crate::cloud_sessions::StepKind::Provision);
    crate::cloud_sessions::startup::save_start_record(&fx.store.lock().unwrap(), &failed).unwrap();
    let targets = fx.api.session_targets(&fx.path());
    assert_eq!(targets[&id].state, CloudState::Available);
    let mut row = json!({"id":id});
    annotate(&mut row, targets.get(&id));
    assert_eq!(row["codespaceState"], "available");
    assert_eq!(
        row["startup"],
        json!({"step":"provision","error":"ssh refused"})
    );
    let error = fx.api.wake(id, &fx.store).unwrap_err();
    assert_eq!(
        error,
        "This cloud session could not start: ssh refused. Choose Retry in the session."
    );
}

#[test]
fn a_cloud_session_belongs_only_to_its_local_project() {
    let gh = FakeGh::new(vec![
        ("auth status", ok(AUTH_OK)),
        ("codespace list", ok(LIST)),
    ]);
    let fx = fixture(&gh);
    let cloud = fx.bind("octo-abc");
    let store = fx.store.lock().unwrap();
    assert!(is_cloud_session_of(&store, cloud, &fx.path()));
    assert!(!is_cloud_session_of(&store, cloud, "/somewhere/else"));
    let local = store.create("claude", &fx.path()).unwrap().id;
    assert!(!is_cloud_session_of(&store, local, &fx.path()));
}

#[test]
fn waking_a_cloud_session_ensures_its_machine_and_ignores_local_sessions() {
    let gh = FakeGh::new(vec![
        ("auth status", ok(AUTH_OK)),
        ("codespace list", ok(LIST)),
    ]);
    let fx = fixture(&gh);
    let cloud = fx.bind("octo-abc");
    let local = fx
        .store
        .lock()
        .unwrap()
        .create("claude", &fx.path())
        .unwrap()
        .id;
    fx.host.log.lock().unwrap().clear();
    fx.api.wake(local, &fx.store).unwrap();
    assert!(fx.host.log.lock().unwrap().is_empty());
    fx.api.wake(cloud, &fx.store).unwrap();
    assert_eq!(
        *fx.host.log.lock().unwrap(),
        vec!["ensure m-octo-abc".to_string()]
    );
}

#[test]
fn a_failed_new_start_names_the_codespace_it_created() {
    // gh creates the codespace, then it never becomes available. Another codespace appearing
    // meanwhile (a parallel start) must not be mistaken for it: the name comes from create.
    let gh = FakeGh::new(vec![
        ("auth status", ok(AUTH_OK)),
        (
            "--method POST",
            ok(r#"{"name":"octo-new1","state":"Provisioning"}"#),
        ),
        ("codespace view", ok(r#"{"state":"Failed"}"#)),
        (
            "codespace list",
            ok(r#"[{"name":"octo-other","state":"Available"}]"#),
        ),
    ]);
    let fx = fixture(&gh);
    let error = fx
        .api
        .prepare(
            &fx.path(),
            Target {
                codespace: CodespaceChoice::New,
                branch: None,
                machine: None,
            },
            &mut |_| {},
        )
        .unwrap_err();
    assert!(
        error.contains("codespace octo-new1 was created and kept"),
        "{error}"
    );
    assert!(
        error.contains(r#"target {"codespace":"octo-new1"}"#),
        "{error}"
    );
    assert!(!error.contains("octo-other"), "{error}");
    assert!(
        !gh.calls().iter().any(|c| c.contains("codespace list")),
        "no listing is needed"
    );
}

#[test]
fn only_an_unavailable_codespace_is_not_woken_for_a_send() {
    for (state, wake) in [
        (CloudState::Available, true),
        (CloudState::Stopped, true),
        (CloudState::Starting, true),
        (CloudState::Stopping, true),
        (CloudState::Unknown, true),
        (CloudState::Unavailable, false),
    ] {
        assert_eq!(should_wake(state), wake, "{state:?}");
    }
}

#[test]
fn a_named_start_error_is_not_decorated() {
    let gh = FakeGh::new(vec![("auth status", ok(AUTH_NO_SCOPE))]);
    let fx = fixture(&gh);
    let error = fx
        .api
        .prepare(
            &fx.path(),
            Target {
                codespace: CodespaceChoice::Named("octo-abc".into()),
                branch: None,
                machine: None,
            },
            &mut |_| {},
        )
        .unwrap_err();
    assert!(!error.contains("was created"), "{error}");
}

struct CpGh {
    files: Vec<&'static str>,
}
impl GhRunner for CpGh {
    fn run(&self, args: &[String], _stdin: Option<&str>) -> Result<GhOutput, String> {
        assert_eq!(&args[..2], ["codespace", "cp"], "{args:?}");
        let remote = args[args.len() - 2].strip_prefix("remote:").unwrap();
        let root = Path::new(args.last().unwrap()).join(remote.rsplit('/').next().unwrap());
        for file in &self.files {
            let path = root.join(file);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, "x").unwrap();
        }
        ok("")
    }
}

struct Evidence {
    api: CloudApi,
    store: Database,
    session: i64,
    project: &'static str,
    _dir: tempfile::TempDir,
}
fn evidence(files: Vec<&'static str>) -> Evidence {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(Mutex::new(Store::open(&dir.path().join("s.db")).unwrap()));
    let api = CloudApi::new(
        Arc::new(CpGh { files }),
        Arc::new(FakeHost::default()),
        store.clone(),
    );
    let project = "/Users/me/widgets";
    let session = {
        let db = store.lock().unwrap();
        let remote = db
            .remember_cloud_project(
                "env-1",
                &json!({"id":"p1","cwd":"/workspaces/widgets"}),
                project,
                "octo-abc",
            )
            .unwrap();
        db.create("claude", &remote.key).unwrap().id
    };
    Evidence {
        api,
        store,
        session,
        project,
        _dir: dir,
    }
}
fn add_task(store: &Database, title: &str, project: Option<&str>) -> String {
    store
        .lock()
        .unwrap()
        .today_add(
            &crate::today::NewTask {
                title: title.into(),
                project: project.map(str::to_owned),
                ..Default::default()
            },
            &Actor::User,
            1_700_000_000_000,
        )
        .unwrap()
        .id
}

#[test]
fn pull_evidence_appends_a_note_with_file_urls_to_the_task() {
    let e = evidence(vec!["a b.png", "r.md"]);
    let task = add_task(&e.store, "Check login", Some(e.project));
    let value = e
        .api
        .pull_evidence(
            &e.store,
            e.project,
            e.session,
            &json!({"taskId":task}),
            &EvidenceScope::Operator,
            1_700_000_100_000,
        )
        .unwrap();
    assert_eq!(value["files"].as_array().unwrap().len(), 2);
    let urls = value["fileUrls"].as_array().unwrap();
    assert!(urls[0].as_str().unwrap().starts_with("file:///"));
    assert!(urls[0].as_str().unwrap().ends_with("a%20b.png"));
    let notes = e.store.lock().unwrap().today_notes(&task).unwrap();
    assert_eq!(notes.len(), 1);
    assert!(
        notes[0].body.contains("codespace octo-abc"),
        "{}",
        notes[0].body
    );
    assert!(notes[0].body.contains(urls[1].as_str().unwrap()));
    assert_eq!(value["note"]["id"], json!(notes[0].id));
}

#[test]
fn pull_evidence_without_a_task_adds_no_note_and_an_empty_pull_adds_none() {
    let e = evidence(vec![]);
    let task = add_task(&e.store, "t", Some(e.project));
    let value = e
        .api
        .pull_evidence(
            &e.store,
            e.project,
            e.session,
            &json!({"taskId":task}),
            &EvidenceScope::Operator,
            1_700_000_100_000,
        )
        .unwrap();
    assert!(value["message"].as_str().unwrap().contains("No evidence"));
    assert!(value.get("note").is_none());
    assert!(e
        .store
        .lock()
        .unwrap()
        .today_notes(&task)
        .unwrap()
        .is_empty());
}

#[test]
fn pull_evidence_checks_the_session_and_the_task_before_pulling() {
    let e = evidence(vec!["a.png"]);
    let other = add_task(&e.store, "Other project", Some("/Users/me/other"));
    let run = |project: &str, session: i64, input: Value, scope: &EvidenceScope| {
        e.api
            .pull_evidence(&e.store, project, session, &input, scope, 1)
    };
    let op = EvidenceScope::Operator;
    assert!(run("/Users/me/other", e.session, json!({}), &op).is_err());
    assert!(run(e.project, e.session, json!({"taskId":other}), &op)
        .unwrap_err()
        .contains("Task not found"));
    assert!(run(e.project, e.session, json!({"taskId":"nope"}), &op).is_err());
    assert!(run(e.project, e.session, json!({"taskId":5}), &op).is_err());
    // A Mono that does not own the project cannot annotate its tasks either.
    let mine = add_task(&e.store, "Mine", Some(e.project));
    let mono = EvidenceScope::Mono(MonoCaller {
        id: "mono-1".into(),
        projects: vec!["/Users/me/other".into()],
        ..Default::default()
    });
    assert!(run(e.project, e.session, json!({"taskId":mine}), &mono).is_err());
    let owner = EvidenceScope::Mono(MonoCaller {
        id: "mono-2".into(),
        projects: vec![e.project.into()],
        ..Default::default()
    });
    let ok = run(e.project, e.session, json!({"taskId":mine}), &owner).unwrap();
    assert!(ok["note"]["id"].is_string());
    let notes = e.store.lock().unwrap().today_notes(&mine).unwrap();
    assert_eq!(notes[0].mono_id.as_deref(), Some("mono-2"));
}

#[test]
fn a_long_file_list_stays_under_the_note_cap() {
    let urls: Vec<String> = (0..200)
        .map(|i| format!("file:///a/very/long/{}/{i}.png", "x".repeat(60)))
        .collect();
    let body = evidence_note("t", "cs", &urls, 0);
    assert!(body.chars().count() <= 5000);
    assert!(body.contains("... and "));
    assert!(body
        .lines()
        .all(|l| !l.starts_with("file:") || l.ends_with(".png")));
}
