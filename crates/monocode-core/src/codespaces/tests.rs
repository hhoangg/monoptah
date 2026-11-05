use super::*;
use std::cell::RefCell;
use std::collections::VecDeque;
use std::sync::Mutex;

type Call = (Vec<String>, Option<String>);

struct Fake {
    calls: Mutex<Vec<Call>>,
    replies: Mutex<VecDeque<Result<GhOutput, String>>>,
}

impl Fake {
    fn new(replies: Vec<Result<GhOutput, String>>) -> Self {
        Self {
            calls: Mutex::new(Vec::new()),
            replies: Mutex::new(replies.into()),
        }
    }
    fn calls(&self) -> Vec<Call> {
        self.calls.lock().unwrap().clone()
    }
    fn argv(&self, i: usize) -> Vec<String> {
        self.calls()[i].0.clone()
    }
}

impl GhRunner for Fake {
    fn run(&self, args: &[String], stdin: Option<&str>) -> Result<GhOutput, String> {
        self.calls
            .lock()
            .unwrap()
            .push((args.to_vec(), stdin.map(str::to_string)));
        self.replies
            .lock()
            .unwrap()
            .pop_front()
            .expect("unexpected gh call")
    }
}

fn ok(stdout: &str) -> Result<GhOutput, String> {
    Ok(GhOutput {
        success: true,
        stdout: stdout.into(),
        stderr: String::new(),
    })
}

fn fail(stderr: &str) -> Result<GhOutput, String> {
    Ok(GhOutput {
        success: false,
        stdout: String::new(),
        stderr: stderr.into(),
    })
}

fn v(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|p| p.to_string()).collect()
}

#[test]
fn repo_from_remote_forms() {
    for url in [
        "https://github.com/hhoangg/monoptah.git",
        "https://github.com/hhoangg/monoptah",
        "https://github.com/hhoangg/monoptah/",
        "https://user:tok@github.com/hhoangg/monoptah.git",
        "git@github.com:hhoangg/monoptah.git",
        "git@github.com:hhoangg/monoptah",
        "ssh://git@github.com/hhoangg/monoptah.git",
        "ssh://git@github.com:22/hhoangg/monoptah.git",
        "git://github.com/hhoangg/monoptah.git",
    ] {
        assert_eq!(
            repo_from_remote(url).as_deref(),
            Some("hhoangg/monoptah"),
            "{url}"
        );
    }
    assert_eq!(
        repo_from_remote("https://github.com/a/b.js.git").as_deref(),
        Some("a/b.js")
    );
    for url in [
        "",
        "https://github.com/onlyowner",
        "/local/path/repo",
        "file:///a/b",
        "https://github.com/a/b/c",
    ] {
        assert_eq!(repo_from_remote(url), None, "{url}");
    }
}

#[test]
fn auth_status_with_scope() {
    let text = "github.com\n  ✓ Logged in to github.com account hhoangg (keyring)\n  - Active account: true\n  - Git operations protocol: https\n  - Token: gho_************\n  - Token scopes: 'codespace', 'gist', 'read:org', 'repo'\n";
    let fake = Fake::new(vec![ok(text)]);
    let status = auth_status(&fake).unwrap();
    assert_eq!(fake.argv(0), v(&["auth", "status", "--active"]));
    assert!(status.logged_in && status.has_codespace_scope);
    assert_eq!(status.user.as_deref(), Some("hhoangg"));
    assert_eq!(status.scopes, ["codespace", "gist", "read:org", "repo"]);
    assert!(status.require_codespace_scope().is_ok());
}

#[test]
fn auth_status_without_scope_and_old_format() {
    let text = "github.com\n  ✓ Logged in to github.com as hhoangg (oauth_token)\n  ✓ Token scopes: gist, read:org, repo\n";
    let status = auth_status(&Fake::new(vec![ok(text)])).unwrap();
    assert!(status.logged_in && !status.has_codespace_scope);
    let err = status.require_codespace_scope().unwrap_err();
    assert!(err.contains("gh auth refresh -h github.com -s codespace"));
}

#[test]
fn auth_status_logged_out() {
    let fake = Fake::new(vec![fail(
        "You are not logged into any GitHub hosts. To log in, run: gh auth login",
    )]);
    let status = auth_status(&fake).unwrap();
    assert!(!status.logged_in && !status.has_codespace_scope);
    assert!(status
        .require_codespace_scope()
        .unwrap_err()
        .contains("gh auth login"));
}

#[test]
fn auth_status_prefers_github_com_block() {
    let text = "ghe.example.com\n  ✓ Logged in to ghe.example.com account corp (keyring)\n  - Token scopes: 'repo'\ngithub.com\n  ✓ Logged in to github.com account me (keyring)\n  - Token scopes: 'codespace'\n";
    let status = parse_auth_status(text);
    assert_eq!(status.user.as_deref(), Some("me"));
    assert!(status.has_codespace_scope);
}

#[test]
fn list_argv_and_parse() {
    let json = r#"[{"name":"cs-abc","displayName":"monoptah-main","state":"Available",
        "gitStatus":{"ahead":1,"behind":0,"hasUncommittedChanges":true,"hasUnpushedChanges":true,"ref":"main"},
        "machineName":"basicLinux32gb","repository":"hhoangg/monoptah","lastUsedAt":"2026-11-01T10:00:00Z"}]"#;
    let fake = Fake::new(vec![ok(json)]);
    let items = list(&fake, "hhoangg/monoptah").unwrap();
    assert_eq!(
        fake.argv(0),
        v(&[
            "codespace",
            "list",
            "--repo",
            "hhoangg/monoptah",
            "--json",
            "name,displayName,state,gitStatus,machineName,repository,lastUsedAt"
        ])
    );
    assert_eq!(items[0].name, "cs-abc");
    assert_eq!(items[0].git_status.as_ref().unwrap().git_ref, "main");
    let out = serde_json::to_value(&items[0]).unwrap();
    assert_eq!(out["displayName"], "monoptah-main");
    assert_eq!(out["gitStatus"]["hasUncommittedChanges"], true);
}

#[test]
fn list_surfaces_scope_error() {
    let fake = Fake::new(vec![fail(
        "error getting codespaces: HTTP 403: needs the codespace scope",
    )]);
    let err = list(&fake, "a/b").unwrap_err();
    assert!(err.contains("gh auth refresh -h github.com -s codespace"));
}

#[test]
fn plain_403_is_not_mapped_to_scope_hint() {
    let fake = Fake::new(vec![fail(
        "HTTP 403: Codespaces are disabled by your organization",
    )]);
    let err = list(&fake, "a/b").unwrap_err();
    assert!(!err.contains("auth refresh"));
    assert!(err.contains("disabled by your organization"));
}

#[test]
fn auth_status_multi_account_keeps_first_account_scopes() {
    let text = "github.com\n  ✓ Logged in to github.com account one (keyring)\n  - Token scopes: 'repo'\n  ✓ Logged in to github.com account two (keyring)\n  - Token scopes: 'codespace'\n";
    let status = parse_auth_status(text);
    assert_eq!(status.user.as_deref(), Some("one"));
    assert!(!status.has_codespace_scope);
}

#[test]
fn wait_ready_fails_fast_when_shut_down() {
    for state in ["Shutdown", "ShuttingDown"] {
        let fake = Fake::new(vec![ok(&format!(r#"{{"state":"{state}"}}"#))]);
        let err = wait_ready_with(
            &fake,
            "cs-1",
            Duration::from_secs(10),
            Duration::from_secs(5),
            &mut |_| panic!("must not sleep"),
        )
        .unwrap_err();
        assert!(err.contains(state) && err.contains("start it"));
        assert_eq!(fake.calls().len(), 1);
    }
}

#[test]
fn secrets_status_reads_all_pages() {
    let fake = Fake::new(vec![
        ok(r#"[{"secrets":[{"name":"A"}]},{"secrets":[{"name":"CLAUDE_CODE_OAUTH_TOKEN"}]}]"#),
        ok(r#"[{"repositories":[{"full_name":"x/1"}]},{"repositories":[{"full_name":"a/b"}]}]"#),
        ok(r#"[{"repositories":[]}]"#),
    ]);
    let status = secrets_status(&fake, "a/b", &v(&["A"])).unwrap();
    assert_eq!(status[0].status, SecretState::Set);
    assert_eq!(status[1].status, SecretState::NotForRepo);
}

#[test]
fn api_pages_accepts_concatenated_objects() {
    let fake = Fake::new(vec![ok(
        r#"{"secrets":[{"name":"A"}]}{"secrets":[{"name":"B"}]}"#,
    )]);
    let pages = api_pages(&fake, "/x").unwrap();
    assert_eq!(collect_field(&pages, "secrets", "name"), ["A", "B"]);
}

#[test]
fn set_secret_new_secret_and_already_selected() {
    let fake = Fake::new(vec![fail("gh: Not Found (HTTP 404)"), ok("")]);
    set_secret(&fake, "K", "v", "a/b").unwrap();
    assert_eq!(fake.argv(1).last().unwrap(), "a/b");
    let fake = Fake::new(vec![
        ok(r#"[{"repositories":[{"full_name":"A/B"}]}]"#),
        ok(""),
    ]);
    set_secret(&fake, "K", "v", "a/b").unwrap();
    assert_eq!(fake.argv(1).last().unwrap(), "A/B");
    let fake = Fake::new(vec![fail("HTTP 500 boom")]);
    assert!(set_secret(&fake, "K", "v", "a/b").is_err());
    assert_eq!(fake.calls().len(), 1);
}

#[test]
fn create_argv_defaults_and_progress() {
    let fake = Fake::new(vec![Ok(GhOutput {
        success: true,
        stdout: "\nfuzzy-space-123\n".into(),
        stderr: "Creating codespace...\nStill working\n".into(),
    })]);
    let lines = RefCell::new(Vec::new());
    let name = create(&fake, "hhoangg/monoptah", "main", None, None, &mut |l| {
        lines.borrow_mut().push(l.to_string())
    })
    .unwrap();
    assert_eq!(name, "fuzzy-space-123");
    assert_eq!(
        fake.argv(0),
        v(&[
            "codespace",
            "create",
            "--repo",
            "hhoangg/monoptah",
            "--branch",
            "main",
            "--machine",
            "basicLinux32gb",
            "--display-name",
            "monoptah-main",
            "--idle-timeout",
            "30m",
            "--retention-period",
            "72h",
            "--default-permissions"
        ])
    );
    assert_eq!(
        lines.into_inner(),
        ["Creating codespace...", "Still working", "fuzzy-space-123"]
    );
}

#[test]
fn create_custom_machine_and_truncated_display() {
    let fake = Fake::new(vec![ok("cs-1\n")]);
    let branch = "feature/".to_string() + &"x".repeat(80);
    create(
        &fake,
        "a/b",
        &branch,
        Some("standardLinux32gb"),
        None,
        &mut |_| {},
    )
    .unwrap();
    let args = fake.argv(0);
    let at = |flag: &str| args[args.iter().position(|a| a == flag).unwrap() + 1].clone();
    assert_eq!(at("--machine"), "standardLinux32gb");
    assert_eq!(at("--display-name").chars().count(), 48);
    assert!(at("--display-name").starts_with("monoptah-feature/x"));
}

#[test]
fn create_failure_and_bad_output() {
    let fake = Fake::new(vec![fail("no such branch")]);
    assert_eq!(
        create(&fake, "a/b", "x", None, None, &mut |_| {}).unwrap_err(),
        "no such branch"
    );
    let fake = Fake::new(vec![ok("")]);
    assert!(create(&fake, "a/b", "x", None, None, &mut |_| {}).is_err());
    let fake = Fake::new(vec![]);
    assert!(create(&fake, "bad repo", "x", None, None, &mut |_| {}).is_err());
}

#[test]
fn state_argv() {
    let fake = Fake::new(vec![ok(r#"{"state":"Shutdown"}"#)]);
    assert_eq!(state(&fake, "cs-1").unwrap(), "Shutdown");
    assert_eq!(
        fake.argv(0),
        v(&["codespace", "view", "-c", "cs-1", "--json", "state"])
    );
    assert!(state(&fake, "cs 1; rm").is_err());
}

#[test]
fn wait_ready_polls_until_available() {
    let fake = Fake::new(vec![
        ok(r#"{"state":"Queued"}"#),
        ok(r#"{"state":"Provisioning"}"#),
        ok(r#"{"state":"Available"}"#),
    ]);
    let mut slept = Vec::new();
    wait_ready_with(
        &fake,
        "cs-1",
        Duration::from_secs(60),
        Duration::from_secs(5),
        &mut |d| slept.push(d),
    )
    .unwrap();
    assert_eq!(fake.calls().len(), 3);
    assert_eq!(slept, [Duration::from_secs(5); 2]);
}

#[test]
fn wait_ready_times_out_and_fails_fast() {
    let replies = (0..10).map(|_| ok(r#"{"state":"Starting"}"#)).collect();
    let fake = Fake::new(replies);
    let err = wait_ready_with(
        &fake,
        "cs-1",
        Duration::from_secs(10),
        Duration::from_secs(5),
        &mut |_| {},
    )
    .unwrap_err();
    assert!(err.contains("Timed out") && err.contains("Starting"));
    assert_eq!(fake.calls().len(), 3);
    let fake = Fake::new(vec![ok(r#"{"state":"Failed"}"#)]);
    assert!(wait_ready_with(
        &fake,
        "cs-1",
        Duration::from_secs(10),
        Duration::from_secs(5),
        &mut |_| {}
    )
    .is_err());
}

#[test]
fn stop_and_delete_argv() {
    let fake = Fake::new(vec![ok(""), ok("")]);
    stop(&fake, "cs-1").unwrap();
    delete(&fake, "cs-1").unwrap();
    assert_eq!(fake.argv(0), v(&["codespace", "stop", "-c", "cs-1"]));
    assert_eq!(
        fake.argv(1),
        v(&["codespace", "delete", "-c", "cs-1", "--force"])
    );
}

#[test]
fn list_machines_argv_and_parse() {
    let json = r#"{"total_count":2,"machines":[
      {"name":"basicLinux32gb","display_name":"2 cores, 8 GB RAM, 32 GB storage","operating_system":"linux","storage_in_bytes":34359738368,"memory_in_bytes":8589934592,"cpus":2,"prebuild_availability":null},
      {"name":"standardLinux32gb","display_name":"4 cores","operating_system":"linux","storage_in_bytes":34359738368,"memory_in_bytes":17179869184,"cpus":4}]}"#;
    let fake = Fake::new(vec![ok(json)]);
    let machines = list_machines(&fake, "hhoangg/monoptah").unwrap();
    assert_eq!(
        fake.argv(0),
        v(&["api", "/repos/hhoangg/monoptah/codespaces/machines"])
    );
    assert_eq!(machines.len(), 2);
    assert_eq!(machines[0].cpus, 2);
    assert_eq!(
        serde_json::to_value(&machines[1]).unwrap()["displayName"],
        "4 cores"
    );
}

#[test]
fn jsonc_stripping() {
    let text = r#"{
      // line comment
      "a": "http://x.y/*not*/", /* block */
      "b": [1, 2, /* c */ ],
      "c": {"d": "e\"//f",},
    }"#;
    let value: serde_json::Value = serde_json::from_str(&strip_jsonc(text)).unwrap();
    assert_eq!(value["a"], "http://x.y/*not*/");
    assert_eq!(value["b"], serde_json::json!([1, 2]));
    assert_eq!(value["c"]["d"], "e\"//f");
}

#[test]
fn devcontainer_secrets_from_all_locations() {
    let dir = tempfile::tempdir().unwrap();
    let dc = dir.path().join(".devcontainer");
    std::fs::create_dir_all(dc.join("python")).unwrap();
    std::fs::write(
        dc.join("devcontainer.json"),
        "{\n // main\n \"name\": \"x\",\n \"secrets\": {\n  \"API_KEY\": {\"description\": \"The key\", \"documentationUrl\": \"https://x\"},\n  \"BARE\": {},\n },\n}\n",
    )
    .unwrap();
    std::fs::write(
        dc.join("python").join("devcontainer.json"),
        "/* nested */ {\"secrets\": {\"API_KEY\": {\"description\": \"dup\"}, \"PY_TOKEN\": {\"description\": \"py\"}}}",
    )
    .unwrap();
    std::fs::write(
        dc.join("broken").tap_dir().join("devcontainer.json"),
        "{ not json",
    )
    .unwrap();
    let found = devcontainer_secrets(dir.path());
    let pairs: Vec<_> = found
        .iter()
        .map(|s| (s.key.as_str(), s.description.as_str()))
        .collect();
    assert_eq!(
        pairs,
        [("API_KEY", "The key"), ("BARE", ""), ("PY_TOKEN", "py")]
    );
}

trait TapDir {
    fn tap_dir(self) -> std::path::PathBuf;
}
impl TapDir for std::path::PathBuf {
    fn tap_dir(self) -> std::path::PathBuf {
        std::fs::create_dir_all(&self).unwrap();
        self
    }
}

#[test]
fn devcontainer_secrets_none() {
    let dir = tempfile::tempdir().unwrap();
    assert!(devcontainer_secrets(dir.path()).is_empty());
    std::fs::write(dir.path().join(".devcontainer.json"), "{\"name\":\"x\"}").unwrap();
    assert!(devcontainer_secrets(dir.path()).is_empty());
}

#[test]
fn secrets_status_set_missing_not_for_repo() {
    let fake = Fake::new(vec![
        ok(r#"{"total_count":2,"secrets":[{"name":"CLAUDE_CODE_OAUTH_TOKEN"},{"name":"OTHER"}]}"#),
        ok(r#"{"total_count":1,"repositories":[{"full_name":"HHoangg/Monoptah"}]}"#),
        ok(r#"{"total_count":1,"repositories":[{"full_name":"x/y"}]}"#),
    ]);
    let keys = v(&["OTHER", "ABSENT", "CLAUDE_CODE_OAUTH_TOKEN"]);
    let status = secrets_status(&fake, "hhoangg/monoptah", &keys).unwrap();
    assert_eq!(
        fake.argv(0),
        v(&[
            "api",
            "--paginate",
            "--slurp",
            "/user/codespaces/secrets?per_page=100"
        ])
    );
    assert_eq!(
        fake.argv(1),
        v(&[
            "api",
            "--paginate",
            "--slurp",
            "/user/codespaces/secrets/CLAUDE_CODE_OAUTH_TOKEN/repositories?per_page=100"
        ])
    );
    assert_eq!(
        fake.argv(2),
        v(&[
            "api",
            "--paginate",
            "--slurp",
            "/user/codespaces/secrets/OTHER/repositories?per_page=100"
        ])
    );
    assert_eq!(fake.calls().len(), 3);
    let got: Vec<_> = status.iter().map(|s| (s.key.as_str(), s.status)).collect();
    assert_eq!(
        got,
        [
            ("CLAUDE_CODE_OAUTH_TOKEN", SecretState::Set),
            ("OTHER", SecretState::NotForRepo),
            ("ABSENT", SecretState::Missing)
        ]
    );
    let json = serde_json::to_value(&status).unwrap();
    assert_eq!(json[1]["status"], "notForRepo");
    assert_eq!(json[2]["status"], "missing");
}

#[test]
fn secrets_status_always_includes_oauth_token() {
    let fake = Fake::new(vec![ok(r#"{"total_count":0,"secrets":[]}"#)]);
    let status = secrets_status(&fake, "a/b", &[]).unwrap();
    assert_eq!(status.len(), 1);
    assert_eq!(status[0].key, OAUTH_SECRET_KEY);
    assert_eq!(status[0].status, SecretState::Missing);
}

#[test]
fn set_secret_uses_stdin_only_and_keeps_existing_repos() {
    let fake = Fake::new(vec![
        ok(r#"[{"repositories":[{"full_name":"o/A"},{"full_name":"o/B"}]}]"#),
        ok(""),
    ]);
    set_secret(
        &fake,
        "CLAUDE_CODE_OAUTH_TOKEN",
        "sk-ant-oat01-SECRET",
        "hhoangg/monoptah",
    )
    .unwrap();
    assert_eq!(
        fake.argv(0),
        v(&[
            "api",
            "--paginate",
            "--slurp",
            "/user/codespaces/secrets/CLAUDE_CODE_OAUTH_TOKEN/repositories?per_page=100"
        ])
    );
    assert_eq!(fake.calls()[0].1, None);
    let (args, stdin) = fake.calls().remove(1);
    assert_eq!(
        args,
        v(&[
            "secret",
            "set",
            "CLAUDE_CODE_OAUTH_TOKEN",
            "--app",
            "codespaces",
            "--user",
            "--repos",
            "o/A,o/B,hhoangg/monoptah"
        ])
    );
    assert!(args.iter().all(|a| !a.contains("SECRET")));
    assert_eq!(stdin.as_deref(), Some("sk-ant-oat01-SECRET"));
}

#[test]
fn set_secret_validation_and_error_redaction() {
    let fake = Fake::new(vec![
        ok(r#"[{"repositories":[]}]"#),
        fail("bad value hunter2 rejected"),
    ]);
    let err = set_secret(&fake, "K", "hunter2", "a/b").unwrap_err();
    assert!(!err.contains("hunter2"));
    let fake = Fake::new(vec![]);
    assert!(set_secret(&fake, "bad key", "v", "a/b").is_err());
    assert!(set_secret(&fake, "K", "", "a/b").is_err());
    assert!(set_secret(&fake, "K", "v", "nope").is_err());
    assert!(fake.calls().is_empty());
}

#[test]
fn workspace_path_uses_repo_name() {
    assert_eq!(
        workspace_path("hhoangg/monoptah").as_deref(),
        Some("/workspaces/monoptah")
    );
}
