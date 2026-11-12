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
    let name = create(
        &fake,
        "hhoangg/monoptah",
        "main",
        &CreateOptions::default(),
        None,
        &mut |l| lines.borrow_mut().push(l.to_string()),
    )
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
        &CreateOptions {
            machine: Some("standardLinux32gb"),
            ..Default::default()
        },
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
        create(
            &fake,
            "a/b",
            "x",
            &CreateOptions::default(),
            None,
            &mut |_| {}
        )
        .unwrap_err(),
        "no such branch"
    );
    let fake = Fake::new(vec![ok("")]);
    assert!(create(
        &fake,
        "a/b",
        "x",
        &CreateOptions::default(),
        None,
        &mut |_| {}
    )
    .is_err());
    let fake = Fake::new(vec![]);
    assert!(create(
        &fake,
        "bad repo",
        "x",
        &CreateOptions::default(),
        None,
        &mut |_| {}
    )
    .is_err());
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

fn sized(name: &str, cpus: u32, memory_gb: u64) -> MachineType {
    MachineType {
        name: name.into(),
        display_name: format!("{cpus} cores"),
        operating_system: "linux".into(),
        storage_in_bytes: 32 << 30,
        memory_in_bytes: memory_gb << 30,
        cpus,
    }
}

/// What GitHub answers for a repository whose devcontainer asks for 4 cores and 8 GB.
const ONLY_STANDARD: &str = r#"{"total_count":1,"machines":[
  {"name":"standardLinux32gb","display_name":"4 cores, 16 GB RAM, 32 GB storage","operating_system":"linux","storage_in_bytes":34359738368,"memory_in_bytes":17179869184,"cpus":4}]}"#;

#[test]
fn list_machines_at_asks_for_the_branch() {
    let fake = Fake::new(vec![ok(ONLY_STANDARD), ok(ONLY_STANDARD)]);
    let machines = list_machines_at(&fake, "hhoangg/monoptah", Some("feat/x y")).unwrap();
    assert_eq!(
        fake.argv(0),
        v(&[
            "api",
            "/repos/hhoangg/monoptah/codespaces/machines?ref=feat%2Fx+y"
        ])
    );
    assert_eq!(machines.len(), 1);
    assert_eq!(machines[0].name, "standardLinux32gb");
    // A blank branch is the default branch: no ref.
    list_machines_at(&fake, "hhoangg/monoptah", Some("  ")).unwrap();
    assert_eq!(
        fake.argv(1),
        v(&["api", "/repos/hhoangg/monoptah/codespaces/machines"])
    );
}

#[test]
fn the_default_machine_is_the_smallest_allowed() {
    // Fewest cores first, then least memory, whatever GitHub's order.
    let allowed = vec![
        sized("premiumLinux", 8, 32),
        sized("standardLinux32gb", 4, 16),
        sized("standardLinuxBigMem", 4, 32),
    ];
    assert_eq!(
        pick_machine(&allowed, None),
        MachinePick {
            name: "standardLinux32gb".into(),
            replaced: None
        }
    );
    // Unrestricted repositories keep today's default, the 2-core machine.
    let open = vec![
        sized("standardLinux32gb", 4, 16),
        sized("basicLinux32gb", 2, 8),
    ];
    assert_eq!(pick_machine(&open, None).name, DEFAULT_MACHINE);
}

#[test]
fn a_saved_machine_is_kept_while_allowed_and_replaced_when_not() {
    let allowed = vec![
        sized("standardLinux32gb", 4, 16),
        sized("premiumLinux", 8, 32),
    ];
    assert_eq!(
        pick_machine(&allowed, Some("premiumLinux")),
        MachinePick {
            name: "premiumLinux".into(),
            replaced: None
        }
    );
    assert_eq!(
        pick_machine(&allowed, Some("basicLinux32gb")),
        MachinePick {
            name: "standardLinux32gb".into(),
            replaced: Some("basicLinux32gb".into())
        }
    );
    // Nothing known (the lookup failed): the saved machine, else the static default.
    assert_eq!(pick_machine(&[], Some("premiumLinux")).name, "premiumLinux");
    assert_eq!(pick_machine(&[], None).name, DEFAULT_MACHINE);
    assert_eq!(pick_machine(&[], Some(" ")).name, DEFAULT_MACHINE);
}

#[test]
fn the_machine_cache_asks_once_per_repo_and_branch() {
    let fake = Fake::new(vec![ok(ONLY_STANDARD), ok(ONLY_STANDARD)]);
    let cache = MachineCache::default();
    for _ in 0..3 {
        assert_eq!(
            cache.pick(&fake, "octo/widgets", Some("main"), None).name,
            "standardLinux32gb"
        );
    }
    // Another branch is another question; the Fake panics on a third call.
    cache.allowed(&fake, "octo/widgets", Some("feat")).unwrap();
    cache.allowed(&fake, "octo/widgets", Some("feat")).unwrap();
    assert_eq!(fake.calls().len(), 2);
}

#[test]
fn a_failed_machine_lookup_keeps_the_static_default_and_is_asked_again() {
    let fake = Fake::new(vec![
        fail("HTTP 403: Must have admin rights to Repository."),
        ok(r#"{"total_count":0,"machines":[]}"#),
        Err("GitHub CLI (gh) is not installed".into()),
        ok(ONLY_STANDARD),
    ]);
    let cache = MachineCache::default();
    // 403, an empty answer and no gh: today's default, and nothing is cached.
    for _ in 0..3 {
        assert_eq!(
            cache.pick(&fake, "octo/widgets", None, None),
            MachinePick {
                name: DEFAULT_MACHINE.into(),
                replaced: None
            }
        );
    }
    assert_eq!(
        cache.pick(&fake, "octo/widgets", None, None).name,
        "standardLinux32gb"
    );
    assert_eq!(fake.calls().len(), 4);
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

#[cfg(unix)]
#[test]
fn a_hung_gh_is_killed_and_reported_as_an_error() {
    let mut cmd = std::process::Command::new("sleep");
    cmd.arg("30");
    let started = std::time::Instant::now();
    let result = super::run_command(
        cmd,
        &["auth".to_string(), "status".to_string()],
        None,
        Duration::from_millis(200),
    );
    let error = result.unwrap_err();
    assert!(error.contains("did not answer within"), "{error}");
    assert!(started.elapsed() < Duration::from_secs(5));
}

#[test]
fn only_codespace_lifecycle_calls_get_the_long_timeout() {
    let args = |parts: &[&str]| parts.iter().map(|p| p.to_string()).collect::<Vec<_>>();
    assert_eq!(
        super::call_timeout(&args(&["auth", "status"])),
        Duration::from_secs(20)
    );
    assert_eq!(
        super::call_timeout(&args(&["api", "x"])),
        Duration::from_secs(20)
    );
    assert_eq!(
        super::call_timeout(&args(&["codespace", "create"])),
        Duration::from_secs(120)
    );
}

#[test]
fn oauth_token_loses_all_whitespace_and_is_validated() {
    let fake = Fake::new(vec![ok(r#"[{"repositories":[]}]"#), ok("")]);
    set_secret(
        &fake,
        OAUTH_SECRET_KEY,
        "  sk-ant-oat01-AAAA\nBBBB CC_dd\r\n\tEE-ff \n",
        "a/b",
    )
    .unwrap();
    let (args, stdin) = fake.calls().remove(1);
    assert_eq!(stdin.as_deref(), Some("sk-ant-oat01-AAAABBBBCC_ddEE-ff"));
    assert!(args.iter().all(|a| !a.contains("AAAA")));

    for bad in [
        "",
        "  \n ",
        "abc-def",
        "sk-ant-oat01-ab$cd",
        "sk-ant-oat01-a\u{e9}",
    ] {
        let fake = Fake::new(vec![]);
        let err = set_secret(&fake, OAUTH_SECRET_KEY, bad, "a/b").unwrap_err();
        assert!(err.contains(OAUTH_SECRET_KEY));
        assert!(bad.trim().is_empty() || !err.contains(bad.trim()));
        assert!(fake.calls().is_empty());
    }
}

#[test]
fn other_secrets_are_only_trimmed() {
    assert_eq!(
        normalize_secret_value("K", " \n a b\nc \r\n").unwrap(),
        "a b\nc"
    );
    let fake = Fake::new(vec![ok(r#"[{"repositories":[]}]"#), ok("")]);
    set_secret(&fake, "K", "\n  v w \n", "a/b").unwrap();
    assert_eq!(fake.calls().remove(1).1.as_deref(), Some("v w"));
    let fake = Fake::new(vec![]);
    assert!(set_secret(&fake, "K", " \n ", "a/b").is_err());
}

#[test]
fn copy_from_argv_expands_only_a_safe_literal_path() {
    let destination = tempfile::tempdir().unwrap();
    let fake = Fake::new(vec![ok("")]);
    copy_from(
        &fake,
        "cs-1",
        "/workspaces/widgets/.evidence",
        destination.path(),
    )
    .unwrap();
    assert_eq!(
        fake.argv(0),
        v(&[
            "codespace",
            "cp",
            "-e",
            "-r",
            "-c",
            "cs-1",
            "remote:/workspaces/widgets/.evidence",
            destination.path().to_str().unwrap()
        ])
    );
}

#[test]
fn copy_from_rejects_unsafe_input_without_calling_gh() {
    let destination = tempfile::tempdir().unwrap();
    let fake = Fake::new(vec![]);
    for path in [
        "",
        "-r",
        "--help",
        "a\nb",
        "a\0b",
        "/x/../etc",
        "/x y",
        "/x;rm",
        "~/x",
        "/x/*",
        "$HOME",
        "/x`id`",
        "/x{a,b}",
    ] {
        assert!(
            copy_from(&fake, "cs-1", path, destination.path()).is_err(),
            "{path:?}"
        );
    }
    assert!(copy_from(&fake, "bad name", "/x", destination.path()).is_err());
    assert!(copy_from(&fake, "cs-1", "/x", Path::new("relative")).is_err());
    assert!(copy_from(&fake, "cs-1", "/x", Path::new("-r")).is_err());
    assert!(fake.calls().is_empty());
}

#[test]
fn copy_from_reports_gh_failure_and_missing_path() {
    let destination = tempfile::tempdir().unwrap();
    let fake = Fake::new(vec![fail("scp: '/x': No such file or directory")]);
    let error = copy_from(&fake, "cs-1", "/x", destination.path()).unwrap_err();
    assert!(is_missing_path(&error, "/x"), "{error}");
    assert!(is_missing_path("scp: /x: No such file or directory", "/x"));
    assert!(!is_missing_path("permission denied", "/x"));
    // A nested failure names a longer path: the folder itself exists.
    assert!(!is_missing_path(
        "scp: '/x/latest.png': No such file or directory",
        "/x"
    ));
    assert!(!is_missing_path(
        "scp: /xy: No such file or directory",
        "/x"
    ));
}

#[test]
fn copy_gets_a_long_timeout() {
    assert!(
        super::call_timeout(&v(&["codespace", "cp", "-r"]))
            > super::call_timeout(&v(&["codespace", "create"]))
    );
}

#[test]
fn prepare_evidence_sends_an_idempotent_script_over_stdin() {
    let fake = Fake::new(vec![ok("")]);
    prepare_evidence(&fake, "cs-1", "/workspaces/widgets").unwrap();
    assert_eq!(
        fake.argv(0),
        v(&["codespace", "ssh", "-c", "cs-1", "--", "sh", "-s"])
    );
    let script = fake.calls()[0].1.clone().unwrap();
    assert!(script.contains("d='/workspaces/widgets'"));
    assert!(script.contains("info/exclude"));
    assert!(!script.contains(".gitignore"));
    assert!(script.contains("CLAUDE.local.md"));
    assert!(script.contains("monoptah-evidence:begin"));
}

#[test]
fn prepare_evidence_rejects_unsafe_workspaces_and_reports_failures() {
    let fake = Fake::new(vec![fail("ssh: connect failed")]);
    for workspace in [
        "",
        "/etc",
        "/workspaces/",
        "/workspaces/a'b",
        "/workspaces/a b",
        "/workspaces/a/b",
    ] {
        assert!(
            prepare_evidence(&fake, "cs-1", workspace).is_err(),
            "{workspace}"
        );
    }
    assert!(fake.calls().is_empty());
    let error = prepare_evidence(&fake, "cs-1", "/workspaces/widgets").unwrap_err();
    assert!(error.contains("connect failed"), "{error}");
}

/// Runs the script with `HOME` set; returns whether it exited successfully.
#[cfg(unix)]
fn run_script(script: &str, home: &Path) -> bool {
    let mut child = std::process::Command::new("sh")
        .arg("-s")
        .env("HOME", home)
        .stdin(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(script.as_bytes())
        .unwrap();
    child.wait().unwrap().success()
}

#[cfg(unix)]
fn git_init(path: &Path) {
    assert!(std::process::Command::new("git")
        .args(["init", "-q"])
        .arg(path)
        .status()
        .unwrap()
        .success());
}

/// Names in `dir`, to prove no temp file is left behind.
#[cfg(unix)]
fn names(dir: &Path) -> Vec<String> {
    let mut all: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    all.sort();
    all
}

#[cfg(unix)]
fn guide_env() -> (tempfile::TempDir, tempfile::TempDir, String) {
    let home = tempfile::tempdir().unwrap();
    let ws = tempfile::tempdir().unwrap();
    git_init(ws.path());
    let script = evidence_script(ws.path().to_str().unwrap(), CloudBackendKind::Codespaces);
    (home, ws, script)
}

#[test]
#[cfg(unix)]
fn the_evidence_script_writes_a_local_guide_keeps_other_content_and_is_idempotent() {
    let (home, ws, script) = guide_env();
    // An exclude file without a trailing newline must not have its last pattern corrupted.
    std::fs::write(ws.path().join(".git/info/exclude"), "keep-me").unwrap();
    let guide_path = ws.path().join("CLAUDE.local.md");
    std::fs::write(&guide_path, "my own notes").unwrap();
    for _ in 0..2 {
        assert!(run_script(&script, home.path()));
    }
    assert!(ws.path().join(".evidence").is_dir());
    let exclude = std::fs::read_to_string(ws.path().join(".git/info/exclude")).unwrap();
    let lines: Vec<_> = exclude.lines().collect();
    assert_eq!(lines.iter().filter(|l| **l == ".evidence/").count(), 1);
    assert_eq!(lines.iter().filter(|l| **l == "CLAUDE.local.md").count(), 1);
    assert!(lines.contains(&"keep-me"));
    let guide = std::fs::read_to_string(&guide_path).unwrap();
    assert!(guide.starts_with("my own notes\n"));
    assert_eq!(guide.matches("monoptah-evidence:begin").count(), 1);
    assert!(guide.contains(&format!("{}/.evidence/", ws.path().display())));
    // Nothing in the user's home is created or touched, and no temp file is left.
    assert!(names(home.path()).is_empty());
    assert!(!names(ws.path())
        .iter()
        .any(|n| n.contains("monoptah.") || n.starts_with(".CLAUDE")));
    let status = std::process::Command::new("git")
        .arg("-C")
        .arg(ws.path())
        .args(["status", "--porcelain"])
        .output()
        .unwrap();
    assert!(String::from_utf8_lossy(&status.stdout).trim().is_empty());
}

/// The guide block of an evidence script, between its markers.
fn guide_block(script: &str) -> &str {
    let start = script.rfind("<!-- monoptah-evidence:begin -->").unwrap();
    let end = script.rfind("<!-- monoptah-evidence:end -->").unwrap();
    &script[start..end + "<!-- monoptah-evidence:end -->".len()]
}

#[test]
fn the_codespace_guide_is_golden_and_the_sandbox_guide_differs_only_in_the_place() {
    let codespace = guide_block(&evidence_script(
        "/workspaces/w",
        CloudBackendKind::Codespaces,
    ))
    .to_string();
    assert_eq!(
        codespace,
        "<!-- monoptah-evidence:begin -->\n\
         ## Evidence for the local user\n\
         You run in a GitHub codespace. Save screenshots, test output and written reports that prove\n\
         your work under `/workspaces/w/.evidence/` (create subfolders freely). That folder is excluded\n\
         from git and is never committed. The user's Monoptah app copies it to their machine on request,\n\
         so do not put secrets there. Mention the file names in your final message.\n\
         The user can click a file path in the chat to view it.\n\
         <!-- monoptah-evidence:end -->"
    );
    let sandbox = guide_block(&evidence_script(
        "/workspaces/w",
        CloudBackendKind::DockerSandbox,
    ))
    .to_string();
    assert!(sandbox.contains("You run in a Docker sandbox. Save"));
    assert_eq!(
        sandbox.replace("a Docker sandbox", "a GitHub codespace"),
        codespace
    );
    // Markers are shared, and the rest of the script is identical.
    let a = evidence_script("/w", CloudBackendKind::Codespaces);
    let b = evidence_script("/w", CloudBackendKind::DockerSandbox);
    assert_eq!(a.replace("a GitHub codespace", "a Docker sandbox"), b);
    assert!(
        evidence_script_at("/home/u/w", CloudBackendKind::DockerSandbox)
            .unwrap()
            .contains("You run in a Docker sandbox.")
    );
}

#[test]
#[cfg(unix)]
fn a_new_guide_file_is_created_world_readable() {
    use std::os::unix::fs::PermissionsExt;
    let (home, ws, script) = guide_env();
    assert!(run_script(&script, home.path()));
    let mode = std::fs::metadata(ws.path().join("CLAUDE.local.md"))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o644);
}

#[test]
#[cfg(unix)]
fn the_evidence_script_replaces_only_its_own_block() {
    let (home, ws, script) = guide_env();
    let guide_path = ws.path().join("CLAUDE.local.md");
    std::fs::write(
        &guide_path,
        "before\n<!-- monoptah-evidence:begin -->\nstale\n<!-- monoptah-evidence:end -->\nafter\n",
    )
    .unwrap();
    assert!(run_script(&script, home.path()));
    let guide = std::fs::read_to_string(&guide_path).unwrap();
    assert!(guide.contains("before\n") && guide.contains("after\n"));
    assert!(!guide.contains("stale"));
    assert_eq!(guide.matches("monoptah-evidence:begin").count(), 1);
}

#[test]
#[cfg(unix)]
fn unbalanced_or_misordered_markers_leave_the_file_untouched_and_fail() {
    for content in [
        "keep\n<!-- monoptah-evidence:begin -->\nthe rest of my notes\n",
        "keep\n<!-- monoptah-evidence:end -->\nnotes\n",
        "<!-- monoptah-evidence:end -->\nx\n<!-- monoptah-evidence:begin -->\ny\n",
        "<!-- monoptah-evidence:begin -->\na\n<!-- monoptah-evidence:end -->\n<!-- monoptah-evidence:begin -->\nb\n<!-- monoptah-evidence:end -->\n",
    ] {
        let (home, ws, script) = guide_env();
        let path = ws.path().join("CLAUDE.local.md");
        std::fs::write(&path, content).unwrap();
        assert!(!run_script(&script, home.path()), "{content}");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), content);
        assert!(!names(ws.path()).iter().any(|n| n.starts_with(".CLAUDE")));
        // The folder and exclude are still prepared.
        assert!(ws.path().join(".evidence").is_dir());
    }
}

#[test]
#[cfg(unix)]
fn an_unreadable_guide_is_not_replaced() {
    use std::os::unix::fs::PermissionsExt;
    let (home, ws, script) = guide_env();
    let path = ws.path().join("CLAUDE.local.md");
    std::fs::write(&path, "private\n").unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).unwrap();
    let ran = run_script(&script, home.path());
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
    // Root can read anything; the property only holds for an ordinary user.
    if !ran {
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "private\n");
    }
    assert!(!names(ws.path()).iter().any(|n| n.starts_with(".CLAUDE")));
}

#[test]
#[cfg(unix)]
fn a_directory_or_a_symlink_named_like_the_guide_is_left_alone() {
    let (home, ws, script) = guide_env();
    std::fs::create_dir(ws.path().join("CLAUDE.local.md")).unwrap();
    assert!(!run_script(&script, home.path()));
    assert!(ws.path().join("CLAUDE.local.md").is_dir());

    let (home, ws, script) = guide_env();
    let outside = tempfile::tempdir().unwrap();
    let target = outside.path().join("dotfile.md");
    std::fs::write(&target, "precious\n").unwrap();
    std::os::unix::fs::symlink(&target, ws.path().join("CLAUDE.local.md")).unwrap();
    assert!(!run_script(&script, home.path()));
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "precious\n");
    assert!(ws.path().join("CLAUDE.local.md").is_symlink());

    // A dangling symlink is still a symlink: nothing is created behind it.
    let (home, ws, script) = guide_env();
    let missing = outside.path().join("missing.md");
    std::os::unix::fs::symlink(&missing, ws.path().join("CLAUDE.local.md")).unwrap();
    assert!(!run_script(&script, home.path()));
    assert!(!missing.exists());
}

#[test]
#[cfg(unix)]
fn a_git_tracked_guide_is_the_projects_own_file() {
    let (home, ws, script) = guide_env();
    std::fs::write(ws.path().join("CLAUDE.local.md"), "tracked\n").unwrap();
    let git = |args: &[&str]| {
        assert!(std::process::Command::new("git")
            .arg("-C")
            .arg(ws.path())
            .args(["-c", "user.name=t", "-c", "user.email=t@t"])
            .args(args)
            .status()
            .unwrap()
            .success())
    };
    git(&["add", "CLAUDE.local.md"]);
    git(&["commit", "-q", "-m", "x"]);
    assert!(!run_script(&script, home.path()));
    assert_eq!(
        std::fs::read_to_string(ws.path().join("CLAUDE.local.md")).unwrap(),
        "tracked\n"
    );
}

#[test]
#[cfg(unix)]
fn the_script_never_touches_the_home_guide() {
    let (home, _ws, script) = guide_env();
    std::fs::create_dir_all(home.path().join(".claude")).unwrap();
    let old = home.path().join(".claude/CLAUDE.md");
    let text = "mine\n<!-- monoptah-evidence -->\nold block\n";
    std::fs::write(&old, text).unwrap();
    assert!(run_script(&script, home.path()));
    assert_eq!(std::fs::read_to_string(&old).unwrap(), text);
}

#[test]
fn remote_paths_may_use_plus_and_at_but_not_dot_segments() {
    assert!(valid_remote_path("/workspaces/a+b/@x/.evidence/shot-1.png").is_ok());
    for bad in [".", "./", "/workspaces/w/.", "/workspaces/w/./x"] {
        assert!(valid_remote_path(bad).is_err(), "{bad}");
    }
}

#[test]
fn gh_ssh_has_a_short_timeout_and_cp_a_long_one() {
    let ssh = super::call_timeout(&v(&["codespace", "ssh", "-c", "x"]));
    assert_eq!(ssh, Duration::from_secs(30));
    assert!(super::call_timeout(&v(&["codespace", "cp"])) > Duration::from_secs(300));
}

#[test]
fn a_marked_ssh_script_may_run_for_minutes_and_run_script_adds_the_marker() {
    let long = super::call_timeout(&v(&[
        "codespace",
        "ssh",
        "-c",
        "x",
        "--",
        "sh",
        "-s",
        "--",
        super::LONG_SCRIPT_MARKER,
    ]));
    assert!(long > Duration::from_secs(120));
    struct Echo(std::sync::Mutex<Vec<String>>);
    impl GhRunner for Echo {
        fn run(&self, args: &[String], stdin: Option<&str>) -> Result<GhOutput, String> {
            self.0
                .lock()
                .unwrap()
                .push(format!("{} <{}>", args.join(" "), stdin.unwrap()));
            Ok(GhOutput {
                success: true,
                stdout: "out".into(),
                stderr: String::new(),
            })
        }
    }
    let echo = Echo(Default::default());
    assert_eq!(
        super::run_script(&echo, "cs-1", "echo hi", true).unwrap(),
        "out"
    );
    super::run_script(&echo, "cs-1", "echo hi", false).unwrap();
    let calls = echo.0.lock().unwrap().clone();
    assert_eq!(
        calls[0],
        "codespace ssh -c cs-1 -- sh -s -- monoptah-long <echo hi>"
    );
    assert_eq!(calls[1], "codespace ssh -c cs-1 -- sh -s <echo hi>");
    assert!(super::run_script(&echo, "bad name", "x", false).is_err());
}

const STARTED: &str = r#"{"name":"fuzzy-space-123","state":"Provisioning","web_url":"https://fuzzy-space-123.github.dev"}"#;

#[test]
fn create_started_uses_the_rest_api_and_returns_the_name() {
    let fake = Fake::new(vec![ok(STARTED)]);
    let started = create_started(
        &fake,
        "hhoangg/monoptah",
        "main",
        &CreateOptions::default(),
        None,
    )
    .unwrap();
    assert_eq!(
        started,
        StartedCodespace {
            name: "fuzzy-space-123".into(),
            state: "Provisioning".into(),
            web_url: Some("https://fuzzy-space-123.github.dev".into()),
        }
    );
    assert_eq!(
        fake.argv(0),
        v(&[
            "api",
            "--method",
            "POST",
            "repos/hhoangg/monoptah/codespaces",
            "-f",
            "ref=main",
            "-f",
            "machine=basicLinux32gb",
            "-f",
            "display_name=monoptah-main",
            "-F",
            "idle_timeout_minutes=30",
            "-F",
            "retention_period_minutes=4320",
            "-F",
            "multi_repo_permissions_opt_out=true",
        ])
    );
}

#[test]
fn create_started_custom_values_and_failures() {
    let fake = Fake::new(vec![ok(r#"{"name":"cs-1"}"#)]);
    let branch = "feature/".to_string() + &"x".repeat(80);
    let started = create_started(
        &fake,
        "a/b",
        &branch,
        &CreateOptions {
            machine: Some("standardLinux32gb"),
            ..Default::default()
        },
        None,
    )
    .unwrap();
    assert_eq!(started.state, "Provisioning");
    assert_eq!(started.web_url, None);
    let args = fake.argv(0);
    assert!(args.contains(&"machine=standardLinux32gb".to_string()));
    let display = args
        .iter()
        .find(|a| a.starts_with("display_name="))
        .unwrap();
    assert_eq!(display.chars().count(), "display_name=".len() + 48);
    // gh errors, bad JSON, a missing or unsafe name and bad input never reach the network.
    let fake = Fake::new(vec![fail("HTTP 422: no such branch")]);
    assert!(
        create_started(&fake, "a/b", "x", &CreateOptions::default(), None)
            .unwrap_err()
            .contains("no such branch")
    );
    for body in ["not json", "{}", r#"{"name":"a b; rm"}"#] {
        let fake = Fake::new(vec![ok(body)]);
        assert!(
            create_started(&fake, "a/b", "x", &CreateOptions::default(), None).is_err(),
            "{body}"
        );
    }
    let fake = Fake::new(vec![]);
    assert!(create_started(&fake, "bad repo", "x", &CreateOptions::default(), None).is_err());
    assert!(create_started(&fake, "a/b", "  ", &CreateOptions::default(), None).is_err());
}

#[test]
fn the_rest_create_gets_the_slow_timeout_but_other_api_reads_do_not() {
    let create = v(&[
        "api",
        "--method",
        "POST",
        "repos/a/b/codespaces",
        "-f",
        "ref=x",
    ]);
    assert_eq!(call_timeout(&create), SLOW_CALL_TIMEOUT);
    assert_eq!(
        call_timeout(&v(&["api", "user/codespaces/cs-1", "--jq", ".web_url"])),
        CALL_TIMEOUT
    );
}

#[test]
fn web_url_and_rebuild_argv() {
    let fake = Fake::new(vec![ok("https://cs-1.github.dev\n")]);
    assert_eq!(web_url(&fake, "cs-1").unwrap(), "https://cs-1.github.dev");
    assert_eq!(
        fake.argv(0),
        v(&["api", "user/codespaces/cs-1", "--jq", ".web_url"])
    );
    assert!(web_url(&Fake::new(vec![ok("\n")]), "cs-1").is_err());
    assert!(web_url(&Fake::new(vec![]), "cs 1").is_err());
    let fake = Fake::new(vec![ok("")]);
    rebuild(&fake, "cs-1").unwrap();
    assert_eq!(fake.argv(0), v(&["codespace", "rebuild", "-c", "cs-1"]));
    assert!(rebuild(&Fake::new(vec![fail("nope")]), "cs-1").is_err());
    assert!(rebuild(&Fake::new(vec![]), "../x").is_err());
}

/// Streams canned lines and records whether `stop` was passed through.
struct Streamer {
    lines: Vec<&'static str>,
    result: GhOutput,
    argv: Mutex<Vec<String>>,
}
impl GhRunner for Streamer {
    fn run(&self, _: &[String], _: Option<&str>) -> Result<GhOutput, String> {
        unreachable!("the log is followed through run_streaming_until")
    }
    fn run_streaming_until(
        &self,
        args: &[String],
        on_line: &mut dyn FnMut(&str),
        _stop: &AtomicBool,
    ) -> Result<GhOutput, String> {
        *self.argv.lock().unwrap() = args.to_vec();
        self.lines.iter().for_each(|l| on_line(l));
        Ok(self.result.clone())
    }
}

#[test]
fn follow_creation_log_tails_the_persisted_log_over_gh_ssh() {
    let runner = Streamer {
        lines: vec![
            "2026-11-07 04:01:54.456Z: Configuration starting...",
            "=====",
            "x",
        ],
        result: GhOutput {
            success: true,
            ..Default::default()
        },
        argv: Mutex::new(vec![]),
    };
    let mut seen = vec![];
    follow_creation_log(&runner, "cs-1", &AtomicBool::new(false), &mut |l| {
        seen.push(l.to_string())
    })
    .unwrap();
    assert_eq!(seen.len(), 3);
    assert_eq!(
        *runner.argv.lock().unwrap(),
        v(&[
            "codespace",
            "ssh",
            "-c",
            "cs-1",
            "--",
            "tail",
            "-n",
            "+1",
            "-F",
            CREATION_LOG
        ])
    );
    // A failed ssh is an error, unless it was stopped on purpose.
    let failing = Streamer {
        lines: vec![],
        result: GhOutput {
            success: false,
            stderr: "Permission denied".into(),
            ..Default::default()
        },
        argv: Mutex::new(vec![]),
    };
    let stop = AtomicBool::new(false);
    assert!(follow_creation_log(&failing, "cs-1", &stop, &mut |_| {})
        .unwrap_err()
        .contains("Permission denied"));
    stop.store(true, Ordering::Relaxed);
    follow_creation_log(&failing, "cs-1", &stop, &mut |_| {}).unwrap();
    assert!(follow_creation_log(&failing, "bad name", &stop, &mut |_| {}).is_err());
}

#[test]
fn the_default_run_streaming_until_falls_back_to_run_streaming() {
    let fake = Fake::new(vec![ok("a\nb\n")]);
    let mut seen = vec![];
    fake.run_streaming_until(
        &v(&["x"]),
        &mut |l| seen.push(l.to_string()),
        &AtomicBool::new(true),
    )
    .unwrap();
    assert_eq!(seen, ["a", "b"]);
}

#[cfg(unix)]
#[test]
fn stream_command_kills_the_process_group_when_stopped() {
    let stop = std::sync::Arc::new(AtomicBool::new(false));
    let flag = stop.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(400));
        flag.store(true, Ordering::Relaxed);
    });
    let mut cmd = Command::new("sh");
    // The grandchild keeps the pipes open: only a group kill ends the read.
    cmd.args(["-c", "echo first; sleep 30 & sleep 30"]);
    let started = std::time::Instant::now();
    let mut seen = vec![];
    let out = stream_command(cmd, &mut |l| seen.push(l.to_string()), Some(&stop)).unwrap();
    assert!(started.elapsed() < Duration::from_secs(10));
    assert_eq!(seen, ["first"]);
    assert!(!out.success);
    assert_eq!(out.stdout, "first\n");
}

#[test]
fn parse_log_line_reads_real_creation_log_lines() {
    let line = parse_log_line("2026-11-07 04:01:54.456Z: Configuration starting...").unwrap();
    assert_eq!(line.text, "Configuration starting...");
    assert_eq!(line.at_ms, Some(1_794_024_114_456));
    let line = parse_log_line(
        "2026-11-07 04:01:58.000Z: Using image: mcr.microsoft.com/devcontainers/universal",
    )
    .unwrap();
    assert_eq!(line.at_ms, Some(1_794_024_118_000));
    assert_eq!(
        line.text,
        "Using image: mcr.microsoft.com/devcontainers/universal"
    );
    // Continuation lines have no prefix and keep their text (indentation trimmed).
    let line =
        parse_log_line("Outcome: success User: codespace WorkspaceFolder: /workspaces/monoptah\r")
            .unwrap();
    assert_eq!(line.at_ms, None);
    assert_eq!(
        line.text,
        "Outcome: success User: codespace WorkspaceFolder: /workspaces/monoptah"
    );
    let line = parse_log_line("$ devcontainer up --skip-non-blocking-commands").unwrap();
    assert_eq!(line.at_ms, None);
    // Blank lines and separators are dropped, with or without a prefix.
    for skipped in [
        "",
        "   ",
        "==========",
        "2026-11-07 04:01:54.456Z: =====",
        "2026-11-07 04:01:54.456Z: ",
    ] {
        assert_eq!(parse_log_line(skipped), None, "{skipped:?}");
    }
    // A malformed stamp is plain text, never a panic.
    let line = parse_log_line("2026-13-45 99:99:99.999Z: odd").unwrap();
    assert_eq!(line.at_ms, None);
    assert_eq!(
        parse_log_line("héllo wörld ünïcode 😀 line that is long")
            .unwrap()
            .at_ms,
        None
    );
}

fn opts(idle: Option<u32>, path: Option<&'static str>) -> CreateOptions<'static> {
    CreateOptions {
        machine: None,
        idle_timeout_minutes: idle,
        devcontainer_path: path,
    }
}

#[test]
fn create_cli_passes_idle_timeout_and_devcontainer_path_only_when_set() {
    let fake = Fake::new(vec![ok("cs-1\n")]);
    let options = opts(Some(120), Some(".devcontainer/web/devcontainer.json"));
    create(&fake, "a/b", "x", &options, None, &mut |_| {}).unwrap();
    let args = fake.argv(0);
    let at = |flag: &str| args[args.iter().position(|a| a == flag).unwrap() + 1].clone();
    assert_eq!(at("--idle-timeout"), "120m");
    assert_eq!(
        at("--devcontainer-path"),
        ".devcontainer/web/devcontainer.json"
    );
    assert_eq!(at("--retention-period"), "72h");
    let fake = Fake::new(vec![ok("cs-1\n")]);
    create(
        &fake,
        "a/b",
        "x",
        &opts(None, Some("  ")),
        None,
        &mut |_| {},
    )
    .unwrap();
    let args = fake.argv(0);
    assert!(!args.contains(&"--devcontainer-path".to_string()));
    assert_eq!(
        args[args.iter().position(|a| a == "--idle-timeout").unwrap() + 1],
        "30m"
    );
}

#[test]
fn create_started_passes_idle_timeout_and_devcontainer_path_only_when_set() {
    let fake = Fake::new(vec![ok(r#"{"name":"cs-1"}"#)]);
    let options = opts(Some(45), Some(".devcontainer.json"));
    create_started(&fake, "a/b", "x", &options, None).unwrap();
    let args = fake.argv(0);
    assert!(args.contains(&"idle_timeout_minutes=45".to_string()));
    assert!(!args.contains(&"idle_timeout_minutes=30".to_string()));
    assert!(args.contains(&"devcontainer_path=.devcontainer.json".to_string()));
    let fake = Fake::new(vec![ok(r#"{"name":"cs-1"}"#)]);
    create_started(&fake, "a/b", "x", &CreateOptions::default(), None).unwrap();
    let args = fake.argv(0);
    assert!(args.contains(&"idle_timeout_minutes=30".to_string()));
    assert!(!args.iter().any(|a| a.starts_with("devcontainer_path")));
}

#[test]
fn create_rejects_out_of_range_idle_timeout_and_unsafe_devcontainer_path() {
    let fake = Fake::new(vec![]);
    for bad in [
        opts(Some(4), None),
        opts(Some(241), None),
        opts(None, Some("../x")),
        opts(None, Some("/etc/x")),
        opts(None, Some("a b")),
    ] {
        assert!(
            create(&fake, "a/b", "x", &bad, None, &mut |_| {}).is_err(),
            "{bad:?}"
        );
        assert!(
            create_started(&fake, "a/b", "x", &bad, None).is_err(),
            "{bad:?}"
        );
    }
}

#[test]
fn idle_timeout_error_copy_is_identical_in_create_options_and_project_settings() {
    use crate::cloud_sessions::project_cloud::CodespaceProjectConfig;
    let fake = Fake::new(vec![]);
    let from_create =
        create(&fake, "a/b", "x", &opts(Some(4), None), None, &mut |_| {}).unwrap_err();
    let from_started = create_started(&fake, "a/b", "x", &opts(Some(4), None), None).unwrap_err();
    let from_settings = CodespaceProjectConfig {
        idle_timeout_minutes: 4,
        ..Default::default()
    }
    .validated()
    .unwrap_err();
    let expected = "Stop when idle must be between 5 and 240 minutes.";
    assert_eq!(from_create, expected);
    assert_eq!(from_started, expected);
    assert_eq!(from_settings, expected);
}

fn machine(name: &str) -> MachineType {
    MachineType {
        name: name.into(),
        ..Default::default()
    }
}

#[test]
fn default_machine_prefers_the_project_machine_and_falls_back() {
    let sized = |name: &str, cpus| MachineType {
        cpus,
        ..machine(name)
    };
    let offered = [
        sized("premiumLinux", 16),
        sized("standardLinux32gb", 4),
        sized(DEFAULT_MACHINE, 2),
    ];
    let pick = |m: &[MachineType], preferred| default_machine(m, preferred);
    assert_eq!(
        pick(&offered, Some("standardLinux32gb")).as_deref(),
        Some("standardLinux32gb")
    );
    // Not offered: the smallest one the repository offers.
    assert_eq!(
        pick(&offered, Some("gone")).as_deref(),
        Some(DEFAULT_MACHINE)
    );
    assert_eq!(pick(&offered, None).as_deref(), Some(DEFAULT_MACHINE));
    // The 2-core machine is not offered (a devcontainer asks for 4): the smallest one is.
    assert_eq!(
        pick(&offered[..2], Some("gone")).as_deref(),
        Some("standardLinux32gb")
    );
    assert_eq!(
        pick(&offered[..2], None).as_deref(),
        Some("standardLinux32gb")
    );
    assert_eq!(pick(&[], Some("x")), None);
}
