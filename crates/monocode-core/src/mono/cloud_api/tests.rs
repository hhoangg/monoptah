use super::*;
use crate::cloud_sessions::backend::docker::{SandboxShell, ShellOutput};
use crate::cloud_sessions::docker_start::{ClaudeAuth, SandboxSecrets};
use crate::codespaces::GhOutput;
use crate::remote::{Machine, SandboxTarget};
use crate::sandboxes::{SbxOutput, SbxRunner};
use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::time::Duration;
use zeroize::Zeroizing;

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
        sandbox: None,
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

/// A fake `sbx`: a list of `(id, name, state)` sandboxes, recorded calls and an optional
/// runner-level failure ("not installed").
struct Sbx {
    sandboxes: Mutex<Vec<(String, String, String)>>,
    calls: Mutex<Vec<String>>,
    broken: Mutex<Option<String>>,
}
impl Sbx {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            sandboxes: Mutex::new(vec![]),
            calls: Mutex::new(vec![]),
            broken: Mutex::new(None),
        })
    }
    fn add(&self, id: &str, name: &str, state: &str) {
        self.sandboxes
            .lock()
            .unwrap()
            .push((id.into(), name.into(), state.into()));
    }
    fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }
    fn count(&self, verb: &str) -> usize {
        self.calls()
            .iter()
            .filter(|c| c.split_whitespace().nth(2) == Some(verb))
            .count()
    }
}
fn sbx_out(success: bool, stdout: &str, stderr: &str) -> Result<SbxOutput, String> {
    Ok(SbxOutput {
        success,
        stdout: stdout.into(),
        stderr: stderr.into(),
    })
}
impl SbxRunner for Sbx {
    fn run(&self, args: &[String], _stdin: Option<&str>) -> Result<SbxOutput, String> {
        self.calls.lock().unwrap().push(args.join(" "));
        if let Some(error) = self.broken.lock().unwrap().clone() {
            return Err(error);
        }
        let item = |(id, name, state): &(String, String, String)| json!({"id":id,"name":name,"agent":"shell","state":state,"cpus":4,"memory_mib":8192,"expires_at":"2030-01-01T00:00:00Z"});
        match args.get(2).map(String::as_str).unwrap_or("") {
            "ls" => {
                let items: Vec<Value> = self.sandboxes.lock().unwrap().iter().map(item).collect();
                sbx_out(true, &Value::Array(items).to_string(), "")
            }
            "inspect" => {
                let found = self
                    .sandboxes
                    .lock()
                    .unwrap()
                    .iter()
                    .find(|s| s.0 == args[3])
                    .map(item);
                match found {
                    Some(value) => sbx_out(true, &value.to_string(), ""),
                    None => sbx_out(
                        false,
                        "",
                        &format!("error: sandbox '{}' not found", args[3]),
                    ),
                }
            }
            "create" => {
                let name = args[args.iter().position(|a| a == "--name").unwrap() + 1].clone();
                self.add("sbx_new0001", &name, "running");
                sbx_out(true, "Creating\nsbx_new0001\n", "")
            }
            "stop" | "rm" => sbx_out(true, "", ""),
            other => panic!("unexpected sbx call: {other}"),
        }
    }
}

/// The ssh seam: replies with a canned stream, or refuses.
#[derive(Default)]
struct Shell {
    canned: Mutex<Option<String>>,
    calls: Mutex<usize>,
}
impl SandboxShell for Shell {
    fn run(
        &self,
        _id: &str,
        _remote: &str,
        _stdin: &str,
        on_line: &mut dyn FnMut(&str),
        _abort: &dyn Fn() -> bool,
        _timeout: Duration,
    ) -> Result<ShellOutput, String> {
        *self.calls.lock().unwrap() += 1;
        let stdout = self
            .canned
            .lock()
            .unwrap()
            .clone()
            .ok_or("unexpected ssh")?;
        for line in stdout.lines() {
            on_line(line);
        }
        Ok(ShellOutput {
            success: true,
            stdout,
            stderr: String::new(),
        })
    }
}

struct NoSecrets;
impl SandboxSecrets for NoSecrets {
    fn claude_auth(&self, _account: Option<&str>) -> Result<Option<ClaudeAuth>, String> {
        Ok(None)
    }
    fn git_payload(&self, _credential: &str, _repo: &str) -> Result<Zeroizing<String>, String> {
        Err("no credentials in tests".into())
    }
}

const REPO_URL: &str = "https://github.com/octo/widgets.git";

fn sandbox_machine(name: &str) -> Machine {
    Machine {
        id: format!("m-sbx-{name}"),
        name: name.into(),
        endpoint: format!("sandbox://{name}"),
        environment_id: format!("env-{name}"),
        ssh: None,
        codespace: None,
        sandbox: Some(SandboxTarget {
            id: "sbx_001abc".into(),
            name: name.into(),
            workspace: "/home/agent/workspaces/widgets".into(),
            relay_port: 47200,
            config: None,
            mode: Default::default(),
        }),
    }
}

struct Fixture {
    api: CloudApi,
    host: Arc<FakeHost>,
    sbx: Arc<Sbx>,
    shell: Arc<Shell>,
    store: Database,
    project: tempfile::TempDir,
    /// A home with the `sbx_cloud` ssh alias, so sandbox prerequisites pass.
    _home: tempfile::TempDir,
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
    let home = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(home.path().join(".ssh")).unwrap();
    std::fs::write(
        home.path().join(".ssh/config"),
        "Host sbx_cloud\n  User x\n",
    )
    .unwrap();
    let host = Arc::new(FakeHost::default());
    let store = Arc::new(Mutex::new(Store::open(Path::new(":memory:")).unwrap()));
    let sbx = Sbx::new();
    let shell = Arc::new(Shell::default());
    let cloud = CloudSessions::new(gh.clone(), host.clone(), store.clone())
        .with_sandboxes(sbx.clone(), shell.clone(), Arc::new(NoSecrets))
        .with_sandbox_home(home.path().to_path_buf());
    Fixture {
        api: CloudApi::assemble(cloud, gh.clone(), host.clone(), store.clone()),
        host,
        sbx,
        shell,
        store,
        project,
        _home: home,
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
                CloudTarget::new(
                    CloudBackendKind::Codespaces,
                    InstanceChoice::Named(codespace.into()),
                ),
                &mut |_| {},
            )
            .unwrap();
        self.api.create_session(&binding, "claude").unwrap().id
    }
    /// A session bound to the Docker sandbox `name`, as the sandbox start flow leaves it. The
    /// sandbox exists in the fake `sbx` and its machine is connected.
    fn bind_sandbox(&self, name: &str) -> i64 {
        self.sbx.add("sbx_001abc", name, "running");
        self.host
            .machines
            .lock()
            .unwrap()
            .push(sandbox_machine(name));
        let store = self.store.lock().unwrap();
        let project = store
            .remember_cloud_ref_project(
                &format!("env-{name}"),
                &json!({"id":"project-1","cwd":"/workspaces/widgets"}),
                &self.path(),
                &CloudRef::sandbox(name),
            )
            .unwrap();
        store.create("claude", &project.key).unwrap().id
    }
    /// Marks the (remote) session as running a turn.
    fn set_running(&self, id: i64) {
        let store = self.store.lock().unwrap();
        let mut remote = store.remote_session(id).unwrap().unwrap();
        remote.running = true;
        store.save_remote_session(&remote).unwrap();
    }
    fn save_docker_config(&self) {
        let config = crate::cloud_sessions::docker_config::DockerProjectConfig {
            repo_url: REPO_URL.into(),
            ..Default::default()
        };
        crate::cloud_sessions::docker_config::save(
            &self.store.lock().unwrap(),
            &self.path(),
            &config,
        )
        .unwrap();
    }
}

fn start_target(kind: CloudBackendKind, instance: InstanceChoice) -> CloudTarget {
    CloudTarget::new(kind, instance)
}
fn cs(name: &str) -> CloudTarget {
    start_target(
        CloudBackendKind::Codespaces,
        InstanceChoice::Named(name.into()),
    )
}

#[test]
fn parses_the_target_of_sessions_start() {
    assert_eq!(target_from_input(&json!({"prompt":"x"})), Ok(None));
    let new = target_from_input(
        &json!({"target":{"kind":"codespace","instance":"new","branch":"b","machine":"m"}}),
    )
    .unwrap()
    .unwrap();
    assert_eq!(new.kind, CloudBackendKind::Codespaces);
    assert_eq!(new.instance, InstanceChoice::New);
    assert_eq!(
        (new.option("branch"), new.option("machine")),
        (Some("b"), Some("m"))
    );
    // The instance defaults to "new".
    let omitted = target_from_input(&json!({"target":{"kind":"sandbox","cpus":4}}))
        .unwrap()
        .unwrap();
    assert_eq!(omitted.kind, CloudBackendKind::DockerSandbox);
    assert_eq!(omitted.instance, InstanceChoice::New);
    assert_eq!(omitted.options["cpus"], 4);
    let named = target_from_input(&json!({"target":{"kind":"codespace","instance":"octo-abc"}}))
        .unwrap()
        .unwrap();
    assert_eq!(named.instance, InstanceChoice::Named("octo-abc".into()));
    for (session, id) in [
        (json!(-5), -5),
        (json!("-5"), -5),
        (json!(7), 7),
        (json!("12"), 12),
    ] {
        let of =
            target_from_input(&json!({"target":{"kind":"sandbox","instance":{"session":session}}}))
                .unwrap()
                .unwrap();
        assert_eq!(of.instance, InstanceChoice::OfSession(id));
    }
    assert!(target_from_input(&json!({"target":{"kind":"vm"}})).is_err());
}

#[test]
fn backend_kinds_have_api_names_and_specs() {
    for kind in CloudBackendKind::ALL {
        assert_eq!(CloudBackendKind::from_api_name(kind.api_name()), Some(kind));
        assert_eq!(spec_of(kind).kind(), kind);
    }
    assert_eq!(CloudBackendKind::from_api_name("codespaces"), None);
    assert_eq!(specs().len(), CloudBackendKind::ALL.len());
    assert!(spec_of(CloudBackendKind::Codespaces).supports_standalone_create());
    assert!(!spec_of(CloudBackendKind::DockerSandbox).supports_standalone_create());
    assert_eq!(
        unsupported(CloudBackendKind::DockerSandbox, "rebuilding"),
        "Sandbox does not support rebuilding."
    );
}

#[test]
fn list_reports_instances_of_both_backends_with_their_sessions() {
    let gh = FakeGh::new(vec![
        ("auth status", ok(AUTH_OK)),
        ("codespace list", ok(LIST)),
        ("codespace view", ok(r#"{"state":"Shutdown"}"#)),
    ]);
    let fx = fixture(&gh);
    let cs_session = fx.bind("octo-abc");
    let sbx_session = fx.bind_sandbox("box-1");
    // A sandbox of the account that no session of this project uses is not listed.
    fx.sbx.add("sbx_other0", "someone-elses", "running");
    fx.set_running(sbx_session);
    let listed = fx.api.list(&fx.path(), &json!({})).unwrap();
    assert_eq!(
        listed["backends"],
        json!({"codespace":{"ok":true},"sandbox":{"ok":true}})
    );
    let rows = listed["instances"].as_array().unwrap();
    assert_eq!(rows.len(), 3, "{rows:?}");
    let by = |name: &str| rows.iter().find(|r| r["instance"] == name).unwrap();
    let stopped = by("octo-abc");
    assert_eq!(stopped["kind"], "codespace");
    assert_eq!(stopped["displayName"], "my cs");
    assert_eq!(stopped["state"], "stopped");
    assert_eq!(stopped["rawState"], "Shutdown");
    assert_eq!(stopped["branch"], "main");
    assert_eq!(stopped["machine"], "basicLinux32gb");
    assert_eq!(stopped["sessions"].as_array().unwrap().len(), 1);
    assert_eq!(stopped["sessions"][0]["sessionId"], cs_session);
    assert_eq!(stopped["sessions"][0]["busy"], false);
    assert_eq!(by("octo-run")["state"], "available");
    assert_eq!(by("octo-run")["sessions"], json!([]));
    let sandbox = by("box-1");
    assert_eq!(sandbox["kind"], "sandbox");
    assert_eq!(sandbox["state"], "available");
    assert_eq!(sandbox["rawState"], "running");
    assert_eq!(sandbox["cpus"], 4);
    assert_eq!(sandbox["memory"], "8g");
    assert_eq!(sandbox["expiresAt"], "2030-01-01T00:00:00Z");
    assert_eq!(sandbox["sessions"][0]["sessionId"], sbx_session);
    assert_eq!(sandbox["sessions"][0]["busy"], true);
    assert!(rows.iter().all(|r| r["instance"] != "someone-elses"));
    // `kind` narrows the listing.
    let only = fx.api.list(&fx.path(), &json!({"kind":"sandbox"})).unwrap();
    assert_eq!(only["instances"].as_array().unwrap().len(), 1);
    assert_eq!(only["backends"], json!({"sandbox":{"ok":true}}));
    assert!(fx.api.list(&fx.path(), &json!({"kind":"vm"})).is_err());
}

#[test]
fn one_failing_backend_does_not_hide_the_others() {
    let gh = FakeGh::new(vec![("auth status", ok(AUTH_NO_SCOPE))]);
    let fx = fixture(&gh);
    let session = fx.bind_sandbox("box-1");
    let listed = fx.api.list(&fx.path(), &json!({})).unwrap();
    assert_eq!(listed["backends"]["codespace"]["ok"], false);
    assert!(listed["backends"]["codespace"]["error"]
        .as_str()
        .unwrap()
        .contains("gh auth refresh -h github.com -s codespace"));
    assert_eq!(listed["backends"]["sandbox"], json!({"ok":true}));
    assert_eq!(listed["instances"][0]["instance"], "box-1");
    assert_eq!(listed["instances"][0]["sessions"][0]["sessionId"], session);
    // Asking for the failing kind returns its error.
    let error = fx
        .api
        .list(&fx.path(), &json!({"kind":"codespace"}))
        .unwrap_err();
    assert!(
        error.contains("gh auth refresh -h github.com -s codespace"),
        "{error}"
    );
    // The other way round: sbx missing leaves the codespaces.
    let gh = FakeGh::new(vec![
        ("auth status", ok(AUTH_OK)),
        ("codespace list", ok(LIST)),
    ]);
    let fx = fixture(&gh);
    fx.save_docker_config();
    *fx.sbx.broken.lock().unwrap() = Some("Docker Sandboxes CLI (sbx) is not installed.".into());
    let listed = fx.api.list(&fx.path(), &json!({})).unwrap();
    assert_eq!(listed["backends"]["codespace"], json!({"ok":true}));
    assert!(listed["backends"]["sandbox"]["error"]
        .as_str()
        .unwrap()
        .contains("not installed"));
    assert_eq!(listed["instances"].as_array().unwrap().len(), 2);
    let error = fx
        .api
        .list(&fx.path(), &json!({"kind":"sandbox"}))
        .unwrap_err();
    assert!(error.contains("not installed"), "{error}");
}

/// A project with a GitHub origin and no sandbox session or config.
#[test]
fn an_unfiltered_call_skips_backends_the_project_does_not_use() {
    let gh = FakeGh::new(vec![
        ("auth status", ok(AUTH_OK)),
        ("codespace list", ok(LIST)),
        ("codespace view", ok(r#"{"state":"Shutdown"}"#)),
        ("user/codespaces/secrets", ok(r#"{"secrets":[]}"#)),
    ]);
    let fx = fixture(&gh);
    let listed = fx.api.list(&fx.path(), &json!({})).unwrap();
    assert_eq!(listed["backends"], json!({"codespace":{"ok":true}}));
    assert!(listed.get("note").is_none());
    let status = fx.api.status(&fx.path(), &json!({})).unwrap();
    assert_eq!(status["backends"].as_object().unwrap().len(), 1);
    assert!(fx.sbx.calls().is_empty(), "{:?}", fx.sbx.calls());
    // A session bound to a sandbox makes the sandbox relevant.
    fx.bind_sandbox("box-1");
    let listed = fx.api.list(&fx.path(), &json!({})).unwrap();
    assert!(listed["backends"].get("sandbox").is_some());
    // Asking for the kind always queries it.
    let fresh = fixture(&gh);
    fresh
        .api
        .list(&fresh.path(), &json!({"kind":"sandbox"}))
        .unwrap();
    assert!(!fresh.sbx.calls().is_empty());
}

#[test]
fn a_project_without_a_github_origin_never_runs_gh_unfiltered() {
    let gh = FakeGh::new(vec![("auth status", ok(AUTH_OK))]);
    let fx = fixture(&gh);
    assert!(std::process::Command::new("git")
        .args(["-C", &fx.path(), "remote", "remove", "origin"])
        .status()
        .unwrap()
        .success());
    fx.save_docker_config();
    let listed = fx.api.list(&fx.path(), &json!({})).unwrap();
    assert_eq!(listed["backends"], json!({"sandbox":{"ok":true}}));
    fx.api.status(&fx.path(), &json!({})).unwrap();
    assert!(gh.calls().is_empty(), "{:?}", gh.calls());
}

#[test]
fn no_relevant_backend_gives_an_empty_answer_with_a_note() {
    let gh = FakeGh::new(vec![]);
    let fx = fixture(&gh);
    assert!(std::process::Command::new("git")
        .args(["-C", &fx.path(), "remote", "remove", "origin"])
        .status()
        .unwrap()
        .success());
    let listed = fx.api.list(&fx.path(), &json!({})).unwrap();
    assert_eq!(listed["instances"], json!([]));
    assert_eq!(listed["backends"], json!({}));
    let note = listed["note"].as_str().unwrap();
    assert!(
        note.contains("GitHub origin") && note.contains("Docker Sandbox"),
        "{note}"
    );
    let status = fx.api.status(&fx.path(), &json!({})).unwrap();
    assert_eq!(status["backends"], json!({}));
    assert!(status["note"].as_str().is_some());
    assert!(gh.calls().is_empty());
    assert!(fx.sbx.calls().is_empty());
}

#[test]
fn status_merges_both_backends_and_never_returns_secret_values() {
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
    fx.save_docker_config();
    let status = fx.api.status(&fx.path(), &json!({})).unwrap();
    let codespace = &status["backends"]["codespace"];
    assert_eq!(codespace["ready"], true);
    assert_eq!(codespace["auth"]["hasCodespaceScope"], true);
    assert_eq!(codespace["repo"], "octo/widgets");
    assert_eq!(codespace["devcontainerSecrets"][0]["key"], "NPM_TOKEN");
    let secrets = codespace["secrets"].as_array().unwrap();
    assert_eq!(
        secrets[0],
        json!({"key":"CLAUDE_CODE_OAUTH_TOKEN","status":"set"})
    );
    assert_eq!(secrets[1], json!({"key":"NPM_TOKEN","status":"missing"}));
    let sandbox = &status["backends"]["sandbox"];
    assert_eq!(
        sandbox,
        &json!({
            "ready":true,"installed":true,"loggedIn":true,"sshConfigured":true,
            "configured":true,"repoUrl":REPO_URL
        })
    );
    assert!(!status.to_string().contains("leak-me"));
    // `kind` narrows it.
    let only = fx
        .api
        .status(&fx.path(), &json!({"kind":"sandbox"}))
        .unwrap();
    assert_eq!(only["backends"].as_object().unwrap().len(), 1);
}

#[test]
fn status_explains_what_is_missing_instead_of_failing() {
    let gh = FakeGh::new(vec![("auth status", ok(AUTH_NO_SCOPE))]);
    let fx = fixture(&gh);
    // A session bound to a sandbox makes the backend relevant without configuring it.
    fx.bind_sandbox("box-1");
    *fx.sbx.broken.lock().unwrap() = Some("Docker Sandboxes CLI (sbx) is not installed.".into());
    let status = fx.api.status(&fx.path(), &json!({})).unwrap();
    let codespace = &status["backends"]["codespace"];
    assert_eq!(codespace["ready"], false);
    assert_eq!(codespace["auth"]["hasCodespaceScope"], false);
    assert!(codespace["note"].as_str().unwrap().contains("-s codespace"));
    assert_eq!(codespace["secrets"], json!([]));
    let sandbox = &status["backends"]["sandbox"];
    assert_eq!(sandbox["ready"], false);
    assert_eq!(sandbox["installed"], false);
    assert_eq!(sandbox["loggedIn"], false);
    assert_eq!(sandbox["configured"], false);
    assert!(sandbox["message"]
        .as_str()
        .unwrap()
        .contains("not installed"));
    assert!(sandbox.get("fixCommand").is_none());
}

#[test]
fn create_defaults_to_the_current_branch_and_waits_until_available() {
    let gh = FakeGh::new(vec![
        ("auth status", ok(AUTH_OK)),
        ("codespace create", ok("octo-new1\n")),
        ("codespace view", ok(r#"{"state":"Available"}"#)),
    ]);
    let fx = fixture(&gh);
    let made = fx
        .api
        .create(&fx.path(), &json!({"kind":"codespace"}), &mut |_| {})
        .unwrap();
    assert_eq!(made["kind"], "codespace");
    assert_eq!(made["instance"], "octo-new1");
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
            &json!({"kind":"codespace","branch":"feat","machine":"standardLinux32gb","displayName":"mine"}),
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
fn a_sandbox_cannot_be_created_alone_and_the_error_says_what_to_do() {
    let gh = FakeGh::new(vec![]);
    let fx = fixture(&gh);
    let error = fx
        .api
        .create(&fx.path(), &json!({"kind":"sandbox"}), &mut |_| {})
        .unwrap_err();
    assert!(
        error.starts_with("Sandbox cannot be created on its own"),
        "{error}"
    );
    assert!(
        error.contains(r#"sessions.start with target {"kind":"sandbox","instance":"new"}"#),
        "{error}"
    );
    assert!(fx.sbx.calls().is_empty() && gh.calls().is_empty());
    assert!(fx.api.create(&fx.path(), &json!({}), &mut |_| {}).is_err());
}

#[test]
fn stop_and_delete_by_instance_only_touch_instances_of_the_project() {
    let gh = FakeGh::new(vec![
        ("auth status", ok(AUTH_OK)),
        ("codespace list", ok(LIST)),
        ("codespace view", ok(r#"{"state":"Available"}"#)),
        ("codespace stop", ok("")),
        ("codespace delete", ok("")),
    ]);
    let fx = fixture(&gh);
    let other = json!({"kind":"codespace","instance":"someone-elses"});
    let error = fx.api.stop(&fx.path(), &other, &never_busy).unwrap_err();
    assert!(error.contains("not a codespace of octo/widgets"), "{error}");
    let error = fx.api.delete(&fx.path(), &other, &never_busy).unwrap_err();
    assert!(error.contains("not a codespace of octo/widgets"), "{error}");
    let error = fx.api.resume(&fx.path(), &other).unwrap_err();
    assert!(error.contains("not a codespace of octo/widgets"), "{error}");
    // A sandbox is the project's only when one of its sessions is bound to it.
    fx.sbx.add("sbx_other0", "someone-elses", "running");
    let foreign = json!({"kind":"sandbox","instance":"someone-elses"});
    let error = fx.api.stop(&fx.path(), &foreign, &never_busy).unwrap_err();
    assert!(
        error.contains("not used by a session of this project"),
        "{error}"
    );
    let error = fx
        .api
        .delete(&fx.path(), &foreign, &never_busy)
        .unwrap_err();
    assert!(
        error.contains("not used by a session of this project"),
        "{error}"
    );
    assert!(!gh
        .calls()
        .iter()
        .any(|c| c.contains("codespace stop") || c.contains("codespace delete")));
    assert_eq!(fx.sbx.count("stop") + fx.sbx.count("rm"), 0);
    // A session of another project does not authorise either.
    let id = fx.bind("octo-run");
    let error = fx
        .api
        .stop("/somewhere/else", &json!({"sessionId":id}), &never_busy)
        .unwrap_err();
    assert!(
        error.contains("not a cloud session of this project"),
        "{error}"
    );
    let stopped = fx
        .api
        .stop(
            &fx.path(),
            &json!({"kind":"codespace","instance":"octo-run"}),
            &never_busy,
        )
        .unwrap();
    assert_eq!(stopped["kind"], "codespace");
    assert_eq!(stopped["instance"], "octo-run");
    assert_eq!(stopped["stopped"], true);
    assert_eq!(stopped["sessions"][0]["sessionId"], id);
    assert!(stopped.get("warning").is_none());
    assert!(gh
        .calls()
        .iter()
        .any(|c| c.contains("codespace stop -c octo-run")));
}

#[test]
fn stop_by_session_works_for_both_backends_and_warns_about_busy_sessions() {
    let gh = FakeGh::new(vec![
        ("auth status", ok(AUTH_OK)),
        ("codespace list", ok(LIST)),
        ("codespace stop", ok("")),
    ]);
    let fx = fixture(&gh);
    let sandbox_a = fx.bind_sandbox("box-1");
    // A second session on the same sandbox, in a turn.
    let sandbox_b = fx.api.begin_start(
        &fx.path(),
        start_target(
            CloudBackendKind::DockerSandbox,
            InstanceChoice::OfSession(sandbox_a),
        ),
        "claude",
    );
    let sandbox_b = sandbox_b.unwrap().into_session().id;
    fx.set_running(sandbox_b);
    let stopped = fx
        .api
        .stop(
            &fx.path(),
            &json!({"sessionId":sandbox_a.to_string()}),
            &never_busy,
        )
        .unwrap();
    assert_eq!(stopped["kind"], "sandbox");
    assert_eq!(stopped["instance"], "box-1");
    assert_eq!(stopped["stopped"], true);
    let sessions = stopped["sessions"].as_array().unwrap();
    assert_eq!(sessions.len(), 2);
    assert!(sessions
        .iter()
        .any(|s| s["sessionId"] == sandbox_b && s["busy"] == true));
    assert!(sessions
        .iter()
        .any(|s| s["sessionId"] == sandbox_a && s["busy"] == false));
    let warning = stopped["warning"].as_str().unwrap();
    assert!(warning.contains(&format!("({sandbox_b})")), "{warning}");
    assert!(!warning.contains(&format!("({sandbox_a})")), "{warning}");
    assert!(fx.sbx.calls().iter().any(|c| c.contains("stop sbx_001abc")));
}

#[test]
fn delete_is_refused_while_a_session_is_busy_and_forgets_bindings_otherwise() {
    let gh = FakeGh::new(vec![
        ("auth status", ok(AUTH_OK)),
        ("codespace list", ok(LIST)),
        ("codespace delete", ok("")),
        ("codespace view", ok(r#"{"state":"Available"}"#)),
    ]);
    let fx = fixture(&gh);
    let id = fx.bind("octo-run");
    let by_instance = json!({"kind":"codespace","instance":"octo-run"});
    // A busy session blocks it, whichever way it is addressed.
    fx.set_running(id);
    for input in [json!({"sessionId":id}), by_instance.clone()] {
        let error = fx.api.delete(&fx.path(), &input, &never_busy).unwrap_err();
        assert!(error.contains("sessions are working on it"), "{error}");
        assert!(error.contains(&format!("({id})")), "{error}");
    }
    assert!(!gh.calls().iter().any(|c| c.contains("codespace delete")));
    {
        let store = fx.store.lock().unwrap();
        let mut remote = store.remote_session(id).unwrap().unwrap();
        remote.running = false;
        remote.needs_input = true;
        store.save_remote_session(&remote).unwrap();
    }
    assert!(fx
        .api
        .delete(&fx.path(), &by_instance, &never_busy)
        .is_err());
    {
        let store = fx.store.lock().unwrap();
        let mut remote = store.remote_session(id).unwrap().unwrap();
        remote.needs_input = false;
        store.save_remote_session(&remote).unwrap();
    }
    // So does the caller's own busy state.
    let error = fx
        .api
        .delete(&fx.path(), &by_instance, &|_: &CloudRef| true)
        .unwrap_err();
    assert!(error.contains("archiving or converting"), "{error}");
    // Files that only exist there are never deleted from the API.
    fx.api
        .cloud
        .record_unsaved("octo-run", &["3 ignored files".to_string()])
        .unwrap();
    let error = fx
        .api
        .delete(&fx.path(), &by_instance, &never_busy)
        .unwrap_err();
    assert!(error.contains("would lose files"), "{error}");
    assert!(!gh.calls().iter().any(|c| c.contains("codespace delete")));
    fx.store
        .lock()
        .unwrap()
        .save_setting(
            &crate::cloud_sessions::unsaved_key(&CloudRef::codespace("octo-run")),
            "[]",
        )
        .unwrap();
    // Idle and saved: gone, with the machine disconnected and the bindings forgotten.
    let deleted = fx
        .api
        .delete(&fx.path(), &by_instance, &never_busy)
        .unwrap();
    assert_eq!(deleted["deleted"], true);
    assert_eq!(deleted["kind"], "codespace");
    assert_eq!(deleted["instance"], "octo-run");
    assert_eq!(deleted["sessions"][0]["sessionId"], id);
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
fn a_sandbox_is_deleted_by_session_or_by_name() {
    let gh = FakeGh::new(vec![]);
    let fx = fixture(&gh);
    let id = fx.bind_sandbox("box-1");
    let deleted = fx
        .api
        .delete(
            &fx.path(),
            &json!({"kind":"sandbox","instance":"box-1"}),
            &never_busy,
        )
        .unwrap();
    assert_eq!(deleted["kind"], "sandbox");
    assert_eq!(deleted["instance"], "box-1");
    assert_eq!(deleted["deleted"], true);
    assert_eq!(deleted["sessions"][0]["sessionId"], id);
    assert!(fx
        .sbx
        .calls()
        .iter()
        .any(|c| c.contains("rm --force sbx_001abc")));
    assert!(fx
        .host
        .log
        .lock()
        .unwrap()
        .iter()
        .any(|l| l == "disconnect m-sbx-box-1"));
    // By id of a bound sandbox: the id maps to the name.
    let fx = fixture(&gh);
    fx.bind_sandbox("box-2");
    let deleted = fx
        .api
        .delete(
            &fx.path(),
            &json!({"kind":"sandbox","instance":"sbx_001abc"}),
            &never_busy,
        )
        .unwrap();
    assert_eq!(deleted["instance"], "box-2");
}

#[test]
fn resume_reconnects_through_a_session_of_the_project() {
    let gh = FakeGh::new(vec![
        ("auth status", ok(AUTH_OK)),
        ("codespace list", ok(LIST)),
    ]);
    let fx = fixture(&gh);
    let cs_id = fx.bind("octo-abc");
    let sbx_id = fx.bind_sandbox("box-1");
    fx.host.log.lock().unwrap().clear();
    let resumed = fx
        .api
        .resume(
            &fx.path(),
            &json!({"kind":"codespace","instance":"octo-abc"}),
        )
        .unwrap();
    assert_eq!(resumed["resumed"], true);
    assert_eq!(resumed["kind"], "codespace");
    assert_eq!(resumed["sessions"][0]["sessionId"], cs_id);
    assert_eq!(
        *fx.host.log.lock().unwrap(),
        vec!["ensure m-octo-abc".to_string()]
    );
    // A sandbox is reconnected: its state is read and the live tunnel is probed.
    let resumed = fx
        .api
        .resume(&fx.path(), &json!({"sessionId":sbx_id}))
        .unwrap();
    assert_eq!(resumed["kind"], "sandbox");
    assert_eq!(resumed["instance"], "box-1");
    assert!(fx.sbx.count("inspect") >= 1, "{:?}", fx.sbx.calls());
    assert!(fx
        .host
        .log
        .lock()
        .unwrap()
        .iter()
        .any(|l| l == "request m-sbx-box-1 environment.describe"));
    // An instance with no session of the project cannot be resumed from here.
    let error = fx
        .api
        .resume(
            &fx.path(),
            &json!({"kind":"codespace","instance":"octo-run"}),
        )
        .unwrap_err();
    assert!(error.contains("sessions.start with target"), "{error}");
    assert!(error.contains(r#""instance":"octo-run""#), "{error}");
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
            start_target(CloudBackendKind::Codespaces, InstanceChoice::New),
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
    // A sandbox has no blocking prepare.
    let error = fx
        .api
        .prepare(
            &fx.path(),
            start_target(CloudBackendKind::DockerSandbox, InstanceChoice::New),
            &mut |_| {},
        )
        .unwrap_err();
    assert!(error.starts_with("Sandbox does not support"), "{error}");
}

#[test]
fn targets_are_local_or_cloud_and_never_wake_anything() {
    let gh = FakeGh::new(vec![
        ("auth status", ok(AUTH_OK)),
        ("codespace list", ok(LIST)),
        ("codespace view", ok(r#"{"state":"Shutdown"}"#)),
    ]);
    let fx = fixture(&gh);
    let cloud = fx.bind("octo-abc");
    let sandbox = fx.bind_sandbox("box-1");
    let local = fx
        .store
        .lock()
        .unwrap()
        .create("claude", &fx.path())
        .unwrap()
        .id;
    fx.host.log.lock().unwrap().clear();

    let targets = fx.api.session_targets(&fx.path());
    assert_eq!(targets.len(), 2);
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
        json!({"id":cloud,"target":{"kind":"codespace","instance":"octo-abc","state":"stopped"},"cloudState":"stopped"})
    );
    // A sandbox session has the same shape, with `sandbox` as its kind.
    let mut row = json!({"id":sandbox});
    annotate(&mut row, targets.get(&sandbox));
    assert_eq!(
        row,
        json!({"id":sandbox,"target":{"kind":"sandbox","instance":"box-1","state":"available"},"cloudState":"available"})
    );
    assert!(row.get("codespaceState").is_none());
    assert!(row["target"].get("sandbox").is_none() && row["target"].get("codespace").is_none());
    let mut row = json!({"id":local});
    annotate(&mut row, targets.get(&local));
    assert_eq!(row, json!({"id":local,"target":{"kind":"local"}}));
    assert_eq!(target_json(None), json!({"kind":"local"}));
}

#[test]
fn a_starting_session_is_listed_as_starting_and_refuses_sends() {
    let gh = FakeGh::new(vec![
        ("auth status", ok(AUTH_OK)),
        ("codespace list", ok(LIST)),
        ("codespace view", ok(r#"{"state":"Available"}"#)),
    ]);
    let fx = fixture(&gh);
    let start = fx
        .api
        .begin_start(&fx.path(), cs("octo-abc"), "claude")
        .unwrap();
    let CloudStart::Pending(session, record) = start.clone() else {
        panic!("a codespace start is pending")
    };
    assert_eq!(start.cloud_ref(), CloudRef::codespace("octo-abc"));
    assert_eq!(start.instance(), "octo-abc");
    assert_eq!(start.initial_state(), CloudState::Starting);
    assert_eq!(
        start.startup_step(),
        Some(crate::cloud_sessions::StepKind::Provision)
    );
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
        json!({"id":id,"target":{"kind":"codespace","instance":"octo-abc","state":"starting"},
            "cloudState":"starting","startup":{"step":"provision"}})
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
    // A pending start counts as a busy session on its instance.
    let on = fx.api.sessions_on(&CloudRef::codespace("octo-abc"));
    assert_eq!(on.len(), 1);
    assert!(on[0].pending && on[0].starting && on[0].busy());
    // A failed start reports its error and the real state.
    let mut failed = record;
    failed.error = Some("ssh refused".into());
    failed.failed_at = Some(crate::cloud_sessions::StepKind::Provision);
    crate::cloud_sessions::startup::save_start_record(&fx.store.lock().unwrap(), &failed).unwrap();
    let targets = fx.api.session_targets(&fx.path());
    assert_eq!(targets[&id].state, CloudState::Available);
    let mut row = json!({"id":id});
    annotate(&mut row, targets.get(&id));
    assert_eq!(row["cloudState"], "available");
    assert_eq!(
        row["startup"],
        json!({"step":"provision","error":"ssh refused"})
    );
    assert!(!fx.api.sessions_on(&CloudRef::codespace("octo-abc"))[0].busy());
    let error = fx.api.wake(id, &fx.store).unwrap_err();
    assert_eq!(
        error,
        "This cloud session could not start: ssh refused. Choose Retry in the session."
    );
}

#[test]
fn sessions_on_lists_live_sessions_of_any_project_and_skips_archived() {
    let gh = FakeGh::new(vec![]);
    let fx = fixture(&gh);
    let first = fx.bind_sandbox("box-1");
    let second = fx
        .api
        .begin_start(
            &fx.path(),
            start_target(
                CloudBackendKind::DockerSandbox,
                InstanceChoice::Named("box-1".into()),
            ),
            "claude",
        )
        .unwrap()
        .into_session()
        .id;
    // The same sandbox used from another local project.
    let other_project = {
        let store = fx.store.lock().unwrap();
        let project = store
            .remember_cloud_ref_project(
                "env-box-1-b",
                &json!({"id":"project-2","cwd":"/workspaces/other"}),
                "/another/project",
                &CloudRef::sandbox("box-1"),
            )
            .unwrap();
        store.create("claude", &project.key).unwrap().id
    };
    fx.set_running(other_project);
    let on = fx.api.sessions_on(&CloudRef::sandbox("box-1"));
    let ids: Vec<i64> = on.iter().map(|s| s.id).collect();
    assert_eq!(on.len(), 3, "{ids:?}");
    for id in [first, second, other_project] {
        assert!(ids.contains(&id));
    }
    assert!(on.iter().any(|s| s.id == other_project && s.busy()));
    assert!(fx.api.sessions_on(&CloudRef::sandbox("nope")).is_empty());
    assert!(fx.api.sessions_on(&CloudRef::codespace("box-1")).is_empty());
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
fn waking_a_codespace_session_ensures_its_machine_and_ignores_local_sessions() {
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
fn waking_a_sandbox_session_reconnects_it() {
    let gh = FakeGh::new(vec![]);
    let fx = fixture(&gh);
    let id = fx.bind_sandbox("box-1");
    fx.host.log.lock().unwrap().clear();
    fx.api.wake(id, &fx.store).unwrap();
    // The state was read read-only, then the live tunnel probed; nothing was spawned over ssh.
    assert!(fx.sbx.count("inspect") >= 1, "{:?}", fx.sbx.calls());
    assert_eq!(
        *fx.host.log.lock().unwrap(),
        vec!["request m-sbx-box-1 environment.describe".to_string()]
    );
    assert_eq!(*fx.shell.calls.lock().unwrap(), 0);
    // A deleted sandbox cannot be woken and says so.
    fx.sbx.sandboxes.lock().unwrap().clear();
    let error = fx.api.wake(id, &fx.store).unwrap_err();
    assert!(error.starts_with("Sandbox was deleted"), "{error}");
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
            start_target(CloudBackendKind::Codespaces, InstanceChoice::New),
            &mut |_| {},
        )
        .unwrap_err();
    assert!(
        error.contains("codespace octo-new1 was created and kept"),
        "{error}"
    );
    assert!(
        error.contains(r#"target {"kind":"codespace","instance":"octo-new1"}"#),
        "{error}"
    );
    assert!(error.contains("cloud.delete"), "{error}");
    assert!(!error.contains("octo-other"), "{error}");
    assert!(
        !gh.calls().iter().any(|c| c.contains("codespace list")),
        "no listing is needed"
    );
}

#[test]
fn a_failed_sandbox_start_names_the_sandbox_and_keeps_it_reachable() {
    let gh = FakeGh::new(vec![]);
    let fx = fixture(&gh);
    fx.save_docker_config();
    let start = fx
        .api
        .begin_start(
            &fx.path(),
            start_target(CloudBackendKind::DockerSandbox, InstanceChoice::New),
            "claude",
        )
        .unwrap();
    let CloudStart::Pending(session, record) = &start else {
        panic!("a new sandbox start is pending")
    };
    // The record is a sandbox one: no codespace name, created.
    assert!(record.created && record.codespace.is_empty());
    let cloud = start.cloud_ref();
    assert_eq!(cloud.kind, CloudBackendKind::DockerSandbox);
    // The note is worded for a sandbox, never for a codespace.
    let note = crate::cloud_sessions::created_note_for(&cloud, "store failed");
    assert!(
        note.contains(&format!("sandbox {} was created and kept", cloud.name)),
        "{note}"
    );
    assert!(note.contains("Retry"), "{note}");
    assert!(note.contains("cloud.delete"), "{note}");
    assert!(!note.contains("codespace"), "{note}");
    assert!(!note.contains(r#""kind":"codespace""#), "{note}");
    // The caller marks the pending session failed instead of deleting it: the sandbox stays
    // listed under the project, with the error, and cloud.delete still reaches it.
    crate::cloud_sessions::pause_start(&fx.store.lock().unwrap(), session.id, "store failed")
        .unwrap();
    let on = fx.api.sessions_on(&cloud);
    assert_eq!(on.len(), 1);
    assert!(on[0].pending && !on[0].starting && !on[0].busy());
    let listed = fx.api.list(&fx.path(), &json!({"kind":"sandbox"})).unwrap();
    let row = listed["instances"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["instance"] == cloud.name.as_str())
        .unwrap_or_else(|| panic!("the sandbox is listed: {listed}"));
    assert_eq!(row["sessions"][0]["sessionId"], session.id);
    let refusal =
        crate::cloud_sessions::starting_refusal(&fx.store.lock().unwrap(), session.id).unwrap();
    assert!(refusal.contains("store failed"), "{refusal}");
    let deleted = fx
        .api
        .delete(
            &fx.path(),
            &json!({"kind":"sandbox","instance":cloud.name}),
            &never_busy,
        )
        .unwrap();
    assert_eq!(deleted["deleted"], true, "{deleted}");
    // A codespace keeps its wording and its retry target.
    let codespace = crate::cloud_sessions::created_note_for(&CloudRef::codespace("octo-1"), "x");
    assert!(codespace.contains("codespace octo-1 was created and kept"));
    assert!(codespace.contains(r#"target {"kind":"codespace","instance":"octo-1"}"#));
}

#[test]
fn stored_busy_flags_are_ignored_on_a_stopped_instance() {
    // After cloud.stop the flag of an interrupted turn stays set in the store; a stopped
    // instance runs nothing, so it must not block delete forever.
    let gh = FakeGh::new(vec![
        ("auth status", ok(AUTH_OK)),
        (
            "codespace list",
            ok(
                r#"[{"name":"octo-run","displayName":"run","state":"Shutdown","repository":"octo/widgets"}]"#,
            ),
        ),
        ("codespace view", ok(r#"{"state":"Shutdown"}"#)),
        ("codespace delete", ok("")),
    ]);
    let fx = fixture(&gh);
    let id = fx.bind("octo-run");
    fx.set_running(id);
    let by_instance = json!({"kind":"codespace","instance":"octo-run"});
    let listed = fx.api.list(&fx.path(), &by_instance).unwrap();
    let row = listed["instances"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["instance"] == "octo-run")
        .unwrap();
    assert_eq!(row["sessions"][0]["sessionId"], id, "{listed}");
    assert_eq!(row["sessions"][0]["busy"], false, "{listed}");
    let deleted = fx
        .api
        .delete(&fx.path(), &by_instance, &never_busy)
        .unwrap();
    assert_eq!(deleted["deleted"], true, "{deleted}");
}

struct LiveSessions(Vec<i64>);
impl AppBusy for LiveSessions {
    fn working_on(&self, _: &CloudRef) -> bool {
        false
    }
    fn session_busy(&self, id: i64) -> bool {
        self.0.contains(&id)
    }
}

#[test]
fn a_session_the_live_view_calls_busy_blocks_delete_and_is_named_in_the_stop_warning() {
    let gh = FakeGh::new(vec![
        ("auth status", ok(AUTH_OK)),
        ("codespace list", ok(LIST)),
        ("codespace view", ok(r#"{"state":"Available"}"#)),
        ("codespace delete", ok("")),
        ("codespace stop", ok("")),
    ]);
    let fx = fixture(&gh);
    // Nothing is stored as running yet (a send was just made): only the live view knows.
    let id = fx.bind("octo-run");
    let by_instance = json!({"kind":"codespace","instance":"octo-run"});
    let error = fx
        .api
        .delete(&fx.path(), &by_instance, &LiveSessions(vec![id]))
        .unwrap_err();
    assert!(error.contains("sessions are working on it"), "{error}");
    assert!(error.contains(&format!("({id})")), "{error}");
    assert!(!gh.calls().iter().any(|c| c.contains("codespace delete")));
    let stopped = fx
        .api
        .stop(&fx.path(), &by_instance, &LiveSessions(vec![id]))
        .unwrap();
    assert!(
        stopped["warning"]
            .as_str()
            .unwrap()
            .contains(&format!("({id})")),
        "{stopped}"
    );
    assert_eq!(stopped["sessions"][0]["busy"], true);
}

#[test]
fn stop_and_delete_are_refused_while_an_archive_works_on_the_instance_for_both_backends() {
    let gh = FakeGh::new(vec![
        ("auth status", ok(AUTH_OK)),
        ("codespace list", ok(LIST)),
        ("codespace view", ok(r#"{"state":"Available"}"#)),
        ("codespace delete", ok("")),
        ("codespace stop", ok("")),
    ]);
    let fx = fixture(&gh);
    let cs_id = fx.bind("octo-run");
    let sbx_id = fx.bind_sandbox("box-1");
    for (id, kind, name) in [
        (cs_id, "codespace", "octo-run"),
        (sbx_id, "sandbox", "box-1"),
    ] {
        let archiving = |cloud: &CloudRef| cloud.name == name;
        for input in [
            json!({"sessionId":id}),
            json!({"kind":kind,"instance":name}),
        ] {
            let error = fx.api.stop(&fx.path(), &input, &archiving).unwrap_err();
            assert!(
                error.starts_with(&format!("Cannot stop {kind} {name}")),
                "{error}"
            );
            assert!(error.contains("archiving or converting"), "{error}");
            let error = fx.api.delete(&fx.path(), &input, &archiving).unwrap_err();
            assert!(
                error.starts_with(&format!("Cannot delete {kind} {name}")),
                "{error}"
            );
        }
    }
    assert!(!gh.calls().iter().any(|c| c.contains("codespace stop")));
    assert!(!gh.calls().iter().any(|c| c.contains("codespace delete")));
    assert_eq!(fx.sbx.count("stop"), 0, "{:?}", fx.sbx.calls());
    assert_eq!(fx.sbx.count("rm"), 0, "{:?}", fx.sbx.calls());
    // Not archiving: it stops.
    fx.api
        .stop(&fx.path(), &json!({"sessionId":cs_id}), &never_busy)
        .unwrap();
}

#[test]
fn resume_without_a_ready_session_gives_backend_specific_advice() {
    let gh = FakeGh::new(vec![
        ("auth status", ok(AUTH_OK)),
        ("codespace list", ok(LIST)),
        ("codespace view", ok(r#"{"state":"Shutdown"}"#)),
    ]);
    let fx = fixture(&gh);
    let error = fx
        .api
        .resume(
            &fx.path(),
            &json!({"kind":"codespace","instance":"octo-run"}),
        )
        .unwrap_err();
    assert!(
        error.contains(r#"sessions.start with target {"kind":"codespace","instance":"octo-run"}"#),
        "{error}"
    );
    fx.save_docker_config();
    let start = fx
        .api
        .begin_start(
            &fx.path(),
            start_target(CloudBackendKind::DockerSandbox, InstanceChoice::New),
            "claude",
        )
        .unwrap();
    let error = fx
        .api
        .resume(
            &fx.path(),
            &json!({"kind":"sandbox","instance":start.instance()}),
        )
        .unwrap_err();
    assert!(error.contains("No ready session"), "{error}");
    assert!(error.contains("Retry"), "{error}");
    assert!(!error.contains("sessions.start"), "{error}");
}

#[test]
fn only_an_unavailable_instance_is_not_woken_for_a_send() {
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
        .prepare(&fx.path(), cs("octo-abc"), &mut |_| {})
        .unwrap_err();
    assert!(!error.contains("was created"), "{error}");
}

#[test]
fn starting_a_codespace_session_resolves_every_instance_form() {
    let gh = FakeGh::new(vec![
        ("auth status", ok(AUTH_OK)),
        ("codespace list", ok(LIST)),
        ("codespace view", ok(r#"{"state":"Shutdown"}"#)),
        (
            "--method POST",
            ok(r#"{"name":"octo-new1","state":"Provisioning"}"#),
        ),
    ]);
    let fx = fixture(&gh);
    // New: branch defaults to the current branch, options pass through.
    let mut target = start_target(CloudBackendKind::Codespaces, InstanceChoice::New);
    target
        .options
        .insert("machine".into(), json!("standardLinux32gb"));
    let start = fx.api.begin_start(&fx.path(), target, "claude").unwrap();
    assert_eq!(start.instance(), "octo-new1");
    assert!(matches!(start, CloudStart::Pending(..)));
    assert!(gh.calls().iter().any(|c| c.contains("ref=trunk")));
    // Named: works while stopped, no options allowed.
    let start = fx
        .api
        .begin_start(&fx.path(), cs("octo-abc"), "claude")
        .unwrap();
    assert_eq!(start.instance(), "octo-abc");
    let mut with_option = cs("octo-abc");
    with_option.options.insert("branch".into(), json!("main"));
    assert!(fx
        .api
        .begin_start(&fx.path(), with_option, "claude")
        .is_err());
    // Of a session: the same instance as that cloud session of the project.
    let id = fx.bind("octo-run");
    let start = fx
        .api
        .begin_start(
            &fx.path(),
            start_target(CloudBackendKind::Codespaces, InstanceChoice::OfSession(id)),
            "claude",
        )
        .unwrap();
    assert_eq!(start.cloud_ref(), CloudRef::codespace("octo-run"));
    // A session of another project, a local session or the wrong kind is refused.
    let error = fx
        .api
        .begin_start(
            "/somewhere/else",
            start_target(CloudBackendKind::Codespaces, InstanceChoice::OfSession(id)),
            "claude",
        )
        .unwrap_err();
    assert!(
        error.contains("not a cloud session of this project"),
        "{error}"
    );
    let error = fx
        .api
        .begin_start(
            &fx.path(),
            start_target(
                CloudBackendKind::DockerSandbox,
                InstanceChoice::OfSession(id),
            ),
            "claude",
        )
        .unwrap_err();
    assert!(
        error.contains("runs in a codespace, not a sandbox"),
        "{error}"
    );
    // Claude only.
    assert!(fx
        .api
        .begin_start(&fx.path(), cs("octo-abc"), "codex")
        .is_err());
}

#[test]
fn starting_a_new_sandbox_uses_the_project_config_and_overrides() {
    let gh = FakeGh::new(vec![]);
    let fx = fixture(&gh);
    let new = || start_target(CloudBackendKind::DockerSandbox, InstanceChoice::New);
    // Without a Docker config the user is asked to set one up.
    let error = fx.api.begin_start(&fx.path(), new(), "claude").unwrap_err();
    assert!(error.contains("not set up for this project"), "{error}");
    fx.save_docker_config();
    let start = fx.api.begin_start(&fx.path(), new(), "claude").unwrap();
    let CloudStart::Pending(session, record) = &start else {
        panic!("a new sandbox start is pending")
    };
    assert_eq!(start.cloud_ref().kind, CloudBackendKind::DockerSandbox);
    assert!(fx
        .sbx
        .sandboxes
        .lock()
        .unwrap()
        .iter()
        .any(|s| s.1 == start.instance()));
    assert_eq!(start.initial_state(), CloudState::Starting);
    assert!(start.startup_step().is_some());
    let sandbox_start = record.sandbox_start.as_ref().unwrap();
    assert_eq!(sandbox_start.config.cpus, 2);
    assert_eq!(sandbox_start.config.repo_url, REPO_URL);
    assert!(record.branch.as_deref().is_some_and(|b| b == "trunk"));
    assert!(is_cloud_session_of(
        &fx.store.lock().unwrap(),
        session.id,
        &fx.path()
    ));
    // Options override the config for this start only.
    let mut target = new();
    target.options.insert("branch".into(), json!("feat/x"));
    target.options.insert("cpus".into(), json!(4));
    target.options.insert("memory".into(), json!("8G"));
    target.options.insert("ttl".into(), json!("1h"));
    let start = fx.api.begin_start(&fx.path(), target, "claude").unwrap();
    let sandbox_start = start.record().unwrap().sandbox_start.as_ref().unwrap();
    assert_eq!(sandbox_start.config.cpus, 4);
    assert_eq!(sandbox_start.config.memory, "8g");
    assert_eq!(sandbox_start.config.ttl, "1h");
    assert_eq!(start.record().unwrap().branch.as_deref(), Some("feat/x"));
    let stored = crate::cloud_sessions::docker_config::load(&fx.store.lock().unwrap(), &fx.path())
        .unwrap()
        .unwrap();
    assert_eq!(stored.cpus, 2, "the project config is untouched");
    // Bad overrides are refused before anything is created.
    let creates = fx.sbx.count("create");
    for (key, value) in [
        ("cpus", json!(99)),
        ("memory", json!("lots")),
        ("ttl", json!("48h")),
    ] {
        let mut target = new();
        target.options.insert(key.into(), value);
        assert!(
            fx.api.begin_start(&fx.path(), target, "claude").is_err(),
            "{key}"
        );
    }
    assert_eq!(fx.sbx.count("create"), creates);
    // Claude only.
    assert!(fx.api.begin_start(&fx.path(), new(), "codex").is_err());
}

#[test]
fn starting_on_an_existing_sandbox_attaches_a_ready_session() {
    let gh = FakeGh::new(vec![]);
    let fx = fixture(&gh);
    let first = fx.bind_sandbox("box-1");
    fx.host.log.lock().unwrap().clear();
    let calls = fx.sbx.calls().len();
    let key = fx
        .store
        .lock()
        .unwrap()
        .remote_session(first)
        .unwrap()
        .unwrap()
        .project
        .key;
    let sandbox = |instance| start_target(CloudBackendKind::DockerSandbox, instance);
    for instance in [
        InstanceChoice::Named("box-1".into()),
        InstanceChoice::OfSession(first),
    ] {
        let start = fx
            .api
            .begin_start(&fx.path(), sandbox(instance), "claude")
            .unwrap();
        let CloudStart::Attached(session, cloud, state) = &start else {
            panic!("an existing sandbox is attached, not provisioned: {start:?}")
        };
        assert_ne!(session.id, first);
        assert_eq!(session.cwd, key, "same remote project as the first session");
        assert_eq!(cloud, &CloudRef::sandbox("box-1"));
        assert_eq!(*state, CloudState::Available);
        assert_eq!(start.initial_state(), CloudState::Available);
        assert_eq!(start.startup_step(), None);
        assert!(start.record().is_none());
        assert_eq!(start.session().id, session.id);
        // No start record: the session is ready.
        assert!(
            crate::cloud_sessions::start_record(&fx.store.lock().unwrap(), session.id).is_none()
        );
    }
    assert_eq!(fx.api.sessions_on(&CloudRef::sandbox("box-1")).len(), 3);
    // Attaching reads the state only: nothing is created, woken or connected.
    assert_eq!(fx.sbx.count("create"), 0);
    assert!(fx.host.log.lock().unwrap().is_empty());
    assert!(fx.sbx.calls().len() >= calls);
    // By id of the sandbox.
    let start = fx
        .api
        .begin_start(
            &fx.path(),
            sandbox(InstanceChoice::Named("sbx_001abc".into())),
            "claude",
        )
        .unwrap();
    assert_eq!(start.instance(), "box-1");
    // Refusals: unknown sandbox, a sandbox of another session's project, options, harness.
    let error = fx
        .api
        .begin_start(
            &fx.path(),
            sandbox(InstanceChoice::Named("nope-1".into())),
            "claude",
        )
        .unwrap_err();
    assert!(
        error.contains("not used by a session of this project"),
        "{error}"
    );
    let error = fx
        .api
        .begin_start(
            "/somewhere/else",
            sandbox(InstanceChoice::Named("box-1".into())),
            "claude",
        )
        .unwrap_err();
    assert!(
        error.contains("not used by a session of this project"),
        "{error}"
    );
    let mut with_option = sandbox(InstanceChoice::Named("box-1".into()));
    with_option.options.insert("cpus".into(), json!(4));
    assert!(fx
        .api
        .begin_start(&fx.path(), with_option, "claude")
        .is_err());
    assert!(fx
        .api
        .begin_start(
            &fx.path(),
            sandbox(InstanceChoice::Named("box-1".into())),
            "codex"
        )
        .is_err());
}

#[test]
fn attaching_needs_a_ready_session_on_the_instance() {
    let gh = FakeGh::new(vec![]);
    let fx = fixture(&gh);
    let cloud = CloudRef::sandbox("box-1");
    let error = fx
        .api
        .cloud
        .attach_session(&fx.path(), &cloud, "claude")
        .unwrap_err();
    assert!(
        error.contains("No session of this project runs in sandbox box-1"),
        "{error}"
    );
    assert!(
        error.contains(r#"{"kind":"sandbox","instance":"new"}"#),
        "{error}"
    );
    // A session that is still on its provisional project is not enough.
    fx.sbx.add("sbx_001abc", "box-1", "running");
    {
        let store = fx.store.lock().unwrap();
        let project =
            crate::store::pending_cloud_project(&fx.path(), &cloud, "/workspaces/widgets");
        store
            .create_pending_cloud_session("claude", project)
            .unwrap();
    }
    let error = fx
        .api
        .cloud
        .attach_session(&fx.path(), &cloud, "claude")
        .unwrap_err();
    assert!(error.contains("still starting"), "{error}");
    // The session of another project does not count.
    let other = fx.bind_sandbox("box-2");
    let _ = other;
    assert!(fx
        .api
        .cloud
        .attach_session("/somewhere/else", &CloudRef::sandbox("box-2"), "claude")
        .is_err());
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
    assert_eq!(value["kind"], "codespace");
    assert_eq!(value["instance"], "octo-abc");
    assert!(value.get("codespace").is_none());
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
    let body = evidence_note("t", &CloudRef::codespace("cs"), &urls, 0);
    assert!(body.chars().count() <= 5000);
    assert!(body.contains("... and "));
    assert!(body
        .lines()
        .all(|l| !l.starts_with("file:") || l.ends_with(".png")));
}

/// base64 lines of a tar of `entries`, then the end marker, as the copy script prints them.
fn tar_stream(entries: &[(&str, &[u8])]) -> String {
    use base64::Engine as _;
    let mut raw = vec![];
    {
        let mut builder = tar::Builder::new(&mut raw);
        for (name, data) in entries {
            let mut header = tar::Header::new_gnu();
            header.set_size(data.len() as u64);
            header.set_mode(0o644);
            header.set_entry_type(tar::EntryType::Regular);
            builder.append_data(&mut header, name, *data).unwrap();
        }
        builder.finish().unwrap();
    }
    let text = base64::engine::general_purpose::STANDARD.encode(&raw);
    let mut out = String::new();
    for chunk in text.as_bytes().chunks(76) {
        out.push_str(&format!(
            "MONOPTAH_B:{}\n",
            std::str::from_utf8(chunk).unwrap()
        ));
    }
    out.push_str(&format!("MONOPTAH_TAR_END={}\n", raw.len()));
    out
}

#[test]
fn pull_evidence_works_for_a_sandbox_session() {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(Mutex::new(Store::open(&dir.path().join("s.db")).unwrap()));
    let host = Arc::new(FakeHost::default());
    host.machines.lock().unwrap().push(sandbox_machine("box-1"));
    let shell = Arc::new(Shell::default());
    *shell.canned.lock().unwrap() = Some(tar_stream(&[
        (".evidence/shot one.png", b"png"),
        (".evidence/notes/r.md", b"md"),
    ]));
    let sbx = Sbx::new();
    let gh = FakeGh::new(vec![]);
    let api = CloudApi::with_sandboxes(
        gh.clone(),
        host.clone(),
        store.clone(),
        sbx.clone(),
        shell.clone(),
        Arc::new(NoSecrets),
    );
    let project = "/Users/me/widgets";
    let session = {
        let db = store.lock().unwrap();
        let remote = db
            .remember_cloud_ref_project(
                "env-box-1",
                &json!({"id":"p1","cwd":"/workspaces/widgets"}),
                project,
                &CloudRef::sandbox("box-1"),
            )
            .unwrap();
        db.create("claude", &remote.key).unwrap().id
    };
    let task = add_task(&store, "Check login", Some(project));
    let value = api
        .pull_evidence(
            &store,
            project,
            session,
            &json!({"taskId":task}),
            &EvidenceScope::Operator,
            1_700_000_100_000,
        )
        .unwrap();
    assert_eq!(value["kind"], "sandbox");
    assert_eq!(value["instance"], "box-1");
    assert!(value.get("codespace").is_none());
    assert_eq!(value["remotePath"], "/workspaces/widgets/.evidence");
    let files = value["files"].as_array().unwrap();
    assert_eq!(files.len(), 2, "{value}");
    let local = value["localDir"].as_str().unwrap();
    assert_eq!(
        std::fs::read(Path::new(local).join("shot one.png")).unwrap(),
        b"png"
    );
    assert_eq!(
        std::fs::read(Path::new(local).join("notes/r.md")).unwrap(),
        b"md"
    );
    assert!(value["fileUrls"][0]
        .as_str()
        .unwrap()
        .starts_with("file:///"));
    // The note names the sandbox, and nothing but the ssh seam was used (no gh, no sbx).
    let notes = store.lock().unwrap().today_notes(&task).unwrap();
    assert_eq!(notes.len(), 1);
    assert!(notes[0].body.contains("sandbox box-1"), "{}", notes[0].body);
    assert_eq!(*shell.calls.lock().unwrap(), 1);
    assert!(gh.calls().is_empty() && sbx.calls().is_empty());
    // A custom path stays inside the workspace.
    assert!(api
        .pull_evidence(
            &store,
            project,
            session,
            &json!({"path":"../x"}),
            &EvidenceScope::Operator,
            1
        )
        .is_err());
}
