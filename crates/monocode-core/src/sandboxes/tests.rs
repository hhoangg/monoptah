use super::*;
use std::collections::VecDeque;
use std::sync::Mutex;

type Call = (Vec<String>, Option<String>);

struct Fake {
    calls: Mutex<Vec<Call>>,
    replies: Mutex<VecDeque<Result<SbxOutput, String>>>,
}

impl Fake {
    fn new(replies: Vec<Result<SbxOutput, String>>) -> Self {
        Self {
            calls: Mutex::new(Vec::new()),
            replies: Mutex::new(replies.into()),
        }
    }
    fn argv(&self, i: usize) -> Vec<String> {
        self.calls.lock().unwrap()[i].0.clone()
    }
    fn count(&self) -> usize {
        self.calls.lock().unwrap().len()
    }
}

impl SbxRunner for Fake {
    fn run(&self, args: &[String], stdin: Option<&str>) -> Result<SbxOutput, String> {
        self.calls
            .lock()
            .unwrap()
            .push((args.to_vec(), stdin.map(str::to_string)));
        self.replies
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_else(|| Err("no reply queued".into()))
    }
}

fn ok(stdout: &str) -> Result<SbxOutput, String> {
    Ok(SbxOutput {
        success: true,
        stdout: stdout.into(),
        stderr: String::new(),
    })
}

fn fail(stderr: &str) -> Result<SbxOutput, String> {
    Ok(SbxOutput {
        success: false,
        stdout: String::new(),
        stderr: stderr.into(),
    })
}

const ID: &str = "sbx_001mabc123";
const PROGRESS: &str = "Pulling kit oci://docker.io/sbx/shell:latest\n  layer 1/3 done\nCreating sandbox monoptah-fix-ab12cd\nsbx_001mabc123\nSandbox is running\n";

#[test]
fn create_argv_is_explicit_and_cloud_only() {
    let fake = Fake::new(vec![ok(PROGRESS)]);
    let id = create(&fake, "monoptah-fix-ab12cd", &Compute::default()).unwrap();
    assert_eq!(id, ID);
    let argv = fake.argv(0);
    assert_eq!(&argv[..3], ["--cloud", "--no-input", "create"]);
    assert!(argv.contains(&"--no-progress".to_string()));
    let joined = argv.join(" ");
    assert!(joined.contains("--name monoptah-fix-ab12cd"));
    assert!(joined.contains("--cpus 2 --memory 4g"));
    assert!(joined.contains("--ttl 2h"));
    assert!(joined.contains("--on-timeout stop"));
    assert!(!joined.contains("--platform"));
    assert_eq!(argv.last().unwrap(), "shell");
}

#[test]
fn create_passes_platform_before_the_kit() {
    let fake = Fake::new(vec![ok(PROGRESS)]);
    let compute = Compute {
        platform: Some("linux/arm64".into()),
        ..Compute::default()
    };
    create(&fake, "monoptah-x-000000", &compute).unwrap();
    let argv = fake.argv(0);
    let n = argv.len();
    assert_eq!(&argv[n - 3..], ["--platform", "linux/arm64", "shell"]);
}

#[test]
fn create_sends_no_stdin_and_no_secret_shaped_argv() {
    let fake = Fake::new(vec![ok(PROGRESS)]);
    create(&fake, "monoptah-x-000000", &Compute::default()).unwrap();
    let (argv, stdin) = fake.calls.lock().unwrap()[0].clone();
    assert!(stdin.is_none());
    for a in &argv {
        assert!(!a.contains("TOKEN") && !a.contains("ghp_") && !a.contains("glpat"));
        assert!(a != "--env" && a != "-e" && a != "--env-file");
    }
}

#[test]
fn create_rejects_bad_input_without_calling_sbx() {
    let fake = Fake::new(vec![]);
    assert!(create(&fake, "a.b", &Compute::default()).is_err());
    let bad = Compute {
        cpus: 3,
        ..Compute::default()
    };
    assert!(create(&fake, "monoptah-x-000000", &bad).is_err());
    assert_eq!(fake.count(), 0);
}

#[test]
fn create_maps_already_exists() {
    let fake = Fake::new(vec![fail(
        "error: create sandbox: rpc error: code = already_exists desc = name taken",
    )]);
    let err = create(&fake, "monoptah-x-000000", &Compute::default()).unwrap_err();
    assert!(err.already_exists);
    assert!(is_already_exists_error(&err.message));
    assert!(err.message.contains("name taken"));
    assert_eq!(err.id, None);
}

#[test]
fn create_other_failures_are_redacted_and_not_already_exists() {
    let fake = Fake::new(vec![fail("denied for GITHUB_TOKEN=ghp_abc123 on retry")]);
    let err = create(&fake, "monoptah-x-000000", &Compute::default()).unwrap_err();
    assert!(!err.message.contains("ghp_abc123"));
    assert!(!err.already_exists);
}

#[test]
fn create_without_an_id_is_an_error() {
    let fake = Fake::new(vec![ok("Creating sandbox...\nDone\n")]);
    assert!(create(&fake, "monoptah-x-000000", &Compute::default()).is_err());
}

#[test]
fn id_must_be_on_its_own_line() {
    assert_eq!(parse_created_id(PROGRESS).as_deref(), Some(ID));
    assert_eq!(
        parse_created_id("  sbx_abc  \r\n").as_deref(),
        Some("sbx_abc")
    );
    assert_eq!(parse_created_id("created sbx_abc ok"), None);
    assert_eq!(parse_created_id("sbx_ABC"), None);
}

const INSPECT: &str = r#"{
  "id": "sbx_001mabc123", "name": "monoptah-x-000000", "agent": "shell",
  "state": "running", "image": "docker.io/sbx/shell:latest", "cpus": 2,
  "memory_mib": 4096, "on_timeout": "stop", "auto_resume": true,
  "age": "5m", "created_at": "2026-11-08T10:00:00Z",
  "expires_at": "2026-11-08T12:00:00Z", "something_new": {"a": 1}
}"#;

#[test]
fn inspect_parses_and_ignores_unknown_fields() {
    let fake = Fake::new(vec![ok(INSPECT)]);
    let info = inspect(&fake, ID).unwrap();
    assert_eq!(
        fake.argv(0),
        ["--cloud", "--no-input", "inspect", ID, "--format", "json"]
    );
    assert_eq!(info.id, ID);
    assert_eq!(info.cpus, 2);
    assert_eq!(info.memory_mib, 4096);
    assert_eq!(info.on_timeout, "stop");
    assert!(info.auto_resume);
    assert!(!info.ttl_paused);
    assert_eq!(info.expires_at.as_deref(), Some("2026-11-08T12:00:00Z"));
}

#[test]
fn inspect_tolerates_missing_fields_numbers_and_arrays() {
    let info = parse_inspect(r#"[{"id":"sbx_1","state":"stopped","ttl_paused":true,"expires_at":1762600000,"created_at":null}]"#).unwrap();
    assert_eq!(info.state, "stopped");
    assert!(info.ttl_paused);
    assert_eq!(info.expires_at.as_deref(), Some("1762600000"));
    assert_eq!(info.created_at, None);
    assert_eq!(info.cpus, 0);
}

#[test]
fn state_reads_the_inspect_state() {
    let fake = Fake::new(vec![ok(INSPECT)]);
    assert_eq!(state(&fake, ID).unwrap(), "running");
}

#[test]
fn inspect_not_found_is_recognised() {
    let fake = Fake::new(vec![fail(
        "error: sandbox 'sbx_doesnotexist' not found\n  try: sbx --cloud ls\n",
    )]);
    let err = inspect(&fake, "sbx_doesnotexist").unwrap_err();
    assert!(is_not_found_error(&err, "sbx_doesnotexist"));
    assert!(is_deleted(&err, "sbx_doesnotexist"));
    assert!(!is_deleted(&err, "sbx_other"));
    assert!(!is_deleted(
        "error: network unreachable",
        "sbx_doesnotexist"
    ));
}

#[test]
fn other_not_found_messages_are_not_a_deleted_sandbox() {
    for msg in [
        "error: container for runtime not found",
        "error: session not found",
        "error: kit 'shell' not found",
        "error: template 'x' not found",
        "error: sandbox 'sbx_other' not found",
        "error: sandbox 'sbx_abc' not found in registry",
        "error: no sandbox 'sbx_abc' not found",
    ] {
        assert!(!is_deleted(msg, "sbx_abc"), "{msg}");
    }
    assert!(is_deleted(
        "error: sandbox 'sbx_abc' not found\n  try: sbx --cloud ls",
        "sbx_abc"
    ));
}

#[test]
fn list_parses_array_and_empty() {
    let fake = Fake::new(vec![ok(&format!("[{INSPECT},{{\"id\":\"sbx_2\"}}]"))]);
    let all = list(&fake).unwrap();
    assert_eq!(
        fake.argv(0),
        ["--cloud", "--no-input", "ls", "--format", "json"]
    );
    assert_eq!(all.len(), 2);
    assert_eq!(all[1].id, "sbx_2");
    assert!(parse_list("[]").unwrap().is_empty());
    assert!(parse_list("").unwrap().is_empty());
    assert!(parse_list("not json").is_err());
    assert!(parse_list("null").unwrap().is_empty());
    let wrapped = parse_list(r#"{"sandboxes":[{"id":"sbx_9","name":"n"}]}"#).unwrap();
    assert_eq!(wrapped[0].id, "sbx_9");
    assert!(parse_list("{}").unwrap().is_empty());
    assert!(parse_list("42").is_err());
}

#[test]
fn ttl_parses_and_extends() {
    let fake = Fake::new(vec![
        ok(r#"{"expires_in_seconds":7000,"max_remaining_seconds":80000,"ttl_paused":false,"x":1}"#),
        ok(""),
    ]);
    let info = ttl(&fake, ID).unwrap();
    assert_eq!(fake.argv(0), ["--cloud", "--no-input", "ttl", ID, "--json"]);
    assert_eq!(info.expires_in_seconds, Some(7000));
    assert_eq!(info.max_remaining_seconds, Some(80000));
    extend_ttl(&fake, ID, "1h30m").unwrap();
    assert_eq!(fake.argv(1), ["--cloud", "--no-input", "ttl", "+1h30m", ID]);
    assert!(extend_ttl(&fake, ID, "soon").is_err());
    assert_eq!(fake.count(), 2);
}

#[test]
fn stop_and_delete_argv() {
    let fake = Fake::new(vec![ok(""), ok("")]);
    stop(&fake, ID).unwrap();
    delete(&fake, ID).unwrap();
    assert_eq!(fake.argv(0), ["--cloud", "--no-input", "stop", ID]);
    assert_eq!(fake.argv(1), ["--cloud", "--no-input", "rm", "--force", ID]);
    assert!(delete(&fake, "--all").is_err());
    assert!(delete(&fake, "my-name").is_err());
    assert!(stop(&fake, "my-name").is_err());
    assert!(inspect(&fake, "my-name").is_err());
    assert!(ttl(&fake, "my-name").is_err());
    assert_eq!(fake.count(), 2);
}

#[test]
fn system_runner_always_prepends_base_flags_once() {
    let bare = vec!["ls".to_string()];
    assert_eq!(with_base_flags(&bare), ["--cloud", "--no-input", "ls"]);
    let full = args(&["ls"]);
    assert_eq!(with_base_flags(&full), full);
}

#[test]
fn call_timeouts_by_verb() {
    assert_eq!(call_timeout(&args(&["create", "x"])), SLOW_CALL_TIMEOUT);
    assert_eq!(call_timeout(&args(&["rm", ID])), SLOW_CALL_TIMEOUT);
    assert_eq!(call_timeout(&args(&["inspect", ID])), CALL_TIMEOUT);
}

#[test]
fn name_validation_table() {
    for good in ["ab", "a1", "1a", "monoptah-fix-ab12cd", "A-b"] {
        assert!(validate_sandbox_name(good).is_ok(), "{good}");
    }
    let long = "a".repeat(NAME_MAX + 1);
    for bad in [
        "",
        "a",
        "-ab",
        "a.b",
        "a_b",
        "a b",
        "ab/",
        "é1",
        long.as_str(),
    ] {
        assert!(validate_sandbox_name(bad).is_err(), "{bad}");
    }
    assert!(validate_sandbox_name(&"a".repeat(NAME_MAX)).is_ok());
}

#[test]
fn id_validation_table() {
    for good in ["sbx_001mabc123", "sbx_a", "sbx_0"] {
        assert!(validate_sandbox_id(good).is_ok(), "{good}");
    }
    for bad in [
        "", "sbx_", "sbx", "sbx_ABC", "sbx_a-b", "sbx_a b", "xsbx_a", "--all", "sbx_a\n",
    ] {
        assert!(validate_sandbox_id(bad).is_err(), "{bad:?}");
    }
}

#[test]
fn compute_validation_table() {
    for (cpus, mem) in [
        (1, "2g"),
        (2, "4g"),
        (4, "8g"),
        (8, "16g"),
        (16, "32g"),
        (2, "4G"),
    ] {
        let c = Compute {
            cpus,
            memory: mem.into(),
            ..Compute::default()
        };
        assert!(c.validate().is_ok(), "{cpus} {mem}");
    }
    for (cpus, mem) in [(2, "8g"), (1, "4g"), (3, "6g"), (0, "0g"), (2, "4096m")] {
        let c = Compute {
            cpus,
            memory: mem.into(),
            ..Compute::default()
        };
        assert!(c.validate().is_err(), "{cpus} {mem}");
    }
    let bad_platform = Compute {
        platform: Some("windows/amd64".into()),
        ..Compute::default()
    };
    assert!(bad_platform.validate().is_err());
}

#[test]
fn ttl_validation_table() {
    for good in ["30m", "2h", "1h30m", "2H", "90s", "24h"] {
        assert!(validate_ttl(good).is_ok(), "{good}");
    }
    for bad in [
        "", "0m", "h", "2", "2d", "-1h", "1.5h", "25h", "30m5", "+2h", "2 h",
    ] {
        assert!(validate_ttl(bad).is_err(), "{bad}");
    }
}

#[test]
fn sandbox_name_slugging() {
    let cases = [
        (
            "feat/Add Login",
            "ab12cd34",
            "monoptah-feat-add-login-ab12cd",
        ),
        ("main", "ABCDEF", "monoptah-main-abcdef"),
        ("--//--", "x", "monoptah-session-x00000"),
        ("fix: é bug!!", "r-1_2.3", "monoptah-fix-bug-r12300"),
    ];
    for (branch, req, want) in cases {
        let got = sandbox_name_for(branch, req);
        assert!(validate_sandbox_name(&got).is_ok(), "{got}");
        assert_eq!(got, want);
    }
    let long = sandbox_name_for(&"x".repeat(200), "abcdef");
    assert!(long.len() <= NAME_MAX);
    assert!(long.ends_with("-abcdef"));
    let trimmed = sandbox_name_for(&format!("{}/y", "a".repeat(29)), "abcdef");
    assert!(!trimmed.contains("--"));
}

#[test]
fn state_mapping() {
    assert_eq!(display_state("running"), CloudState::Available);
    assert_eq!(display_state("stopping"), CloudState::Stopping);
    assert_eq!(display_state("stopped"), CloudState::Stopped);
    assert_eq!(display_state("Stopped "), CloudState::Stopped);
    assert_eq!(display_state("creating"), CloudState::Starting);
    assert_eq!(display_state("failed"), CloudState::Unavailable);
    assert_eq!(display_state("weird"), CloudState::Unknown);
    assert!(is_stopped("stopped"));
    assert!(!is_stopped("stopping"));
    assert!(!is_stopped("running"));
}

#[test]
fn ssh_destination_format() {
    assert_eq!(ssh_destination(ID), "sbx_001mabc123@sbx_cloud");
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

#[test]
fn ssh_alias_detection_from_files() {
    let none = tempfile::tempdir().unwrap();
    assert!(!ssh_alias_configured(none.path()));

    let direct = tempfile::tempdir().unwrap();
    write(
        &direct.path().join(".ssh/config"),
        "Host example\n  User me\nHost sbx_cloud\n  HostName x\n",
    );
    assert!(ssh_alias_configured(direct.path()));

    let included = tempfile::tempdir().unwrap();
    write(
        &included.path().join(".ssh/config"),
        "Include ~/.ssh/sandboxes/config\nHost other\n",
    );
    write(
        &included.path().join(".ssh/sandboxes/config"),
        "# >>> docker sandboxes ssh: sbx_cloud >>>\nHost sbx_cloud\n    HostName sbx_gateway\n",
    );
    assert!(ssh_alias_configured(included.path()));

    let relative = tempfile::tempdir().unwrap();
    write(&relative.path().join(".ssh/config"), "Include extra.conf\n");
    write(&relative.path().join(".ssh/extra.conf"), "host sbx_cloud\n");
    assert!(ssh_alias_configured(relative.path()));

    let lookalike = tempfile::tempdir().unwrap();
    write(
        &lookalike.path().join(".ssh/config"),
        "# Host sbx_cloud\nHost sbx_cloud2 *.sbx\nHostName sbx_cloud\n",
    );
    assert!(!ssh_alias_configured(lookalike.path()));

    let looped = tempfile::tempdir().unwrap();
    write(
        &looped.path().join(".ssh/config"),
        "Include ~/.ssh/config\n",
    );
    assert!(!ssh_alias_configured(looped.path()));
}

#[test]
fn diagnose_mapping() {
    let home = tempfile::tempdir().unwrap();
    let missing = Fake::new(vec![Err(SBX_MISSING.into())]);
    assert_eq!(diagnose(&missing, home.path()), Prerequisites::SbxMissing);

    let logged_out = Fake::new(vec![fail("error: not logged in\n  try: sbx login")]);
    let status = diagnose(&logged_out, home.path());
    assert!(matches!(&status, Prerequisites::NotLoggedIn(m) if m.contains("not logged in")));
    assert!(status.message().contains("not logged in"));
    assert_eq!(status.fix_command(), Some("sbx login"));

    let down = Fake::new(vec![fail("error: connection refused")]);
    assert!(matches!(
        diagnose(&down, home.path()),
        Prerequisites::Unreachable(_)
    ));

    let no_alias = Fake::new(vec![ok("[]")]);
    let status = diagnose(&no_alias, home.path());
    assert_eq!(status, Prerequisites::SshAliasMissing);
    assert_eq!(status.fix_command(), Some("sbx --cloud setup ssh"));
    assert!(status.message().contains("sbx --cloud setup ssh"));

    write(&home.path().join(".ssh/config"), "Host sbx_cloud\n");
    let ready = Fake::new(vec![ok("[]")]);
    let status = diagnose(&ready, home.path());
    assert!(status.is_ready());
    assert_eq!(status.fix_command(), None);
    // The probe is a read-only listing, never a create/rm/stop/exec.
    assert_eq!(
        ready.argv(0),
        ["--cloud", "--no-input", "ls", "--format", "json"]
    );
}

#[test]
fn redaction() {
    assert_eq!(
        redact("bad token ghp_abc and more"),
        "bad token [redacted] and more"
    );
    assert_eq!(redact("API_KEY=sk-ant-123 x"), "[redacted] x");
    assert_eq!(redact("plain text stays"), "plain text stays");
    assert_eq!(redact("(glpat-xyz)"), "([redacted])");
}

#[test]
fn diagnose_recognises_more_login_texts() {
    let home = tempfile::tempdir().unwrap();
    for msg in ["please sign-in again", "Docker session expired"] {
        let fake = Fake::new(vec![fail(msg)]);
        assert!(
            matches!(diagnose(&fake, home.path()), Prerequisites::NotLoggedIn(_)),
            "{msg}"
        );
    }
}

#[test]
fn create_returns_the_id_printed_before_a_failure() {
    let fake = Fake::new(vec![Ok(SbxOutput {
        success: false,
        stdout: "Creating\nsbx_001morphan\n".into(),
        stderr: "error: start failed".into(),
    })]);
    let err = create(&fake, "monoptah-x-000000", &Compute::default()).unwrap_err();
    assert_eq!(err.id.as_deref(), Some("sbx_001morphan"));
    assert!(err.message.contains("start failed"));
}

#[test]
fn create_timeout_has_no_id_and_find_by_name_recovers_it() {
    let fake = Fake::new(vec![
        Err("Docker Sandboxes CLI did not answer within 120 seconds".into()),
        ok(
            r#"[{"id":"sbx_a","name":"other","agent":"shell"},{"id":"sbx_b","name":"monoptah-x-000000","agent":"shell"},{"id":"sbx_c","name":"monoptah-x-000000","agent":"claude"}]"#,
        ),
    ]);
    let err = create(&fake, "monoptah-x-000000", &Compute::default()).unwrap_err();
    assert_eq!(err.id, None);
    assert!(!err.already_exists);
    let found = find_by_name(&fake, "monoptah-x-000000").unwrap().unwrap();
    assert_eq!(found.id, "sbx_b");
    assert_eq!(
        fake.argv(1),
        ["--cloud", "--no-input", "ls", "--format", "json"]
    );
}

#[test]
fn find_by_name_handles_none_and_ambiguity() {
    let fake = Fake::new(vec![ok("[]")]);
    assert_eq!(find_by_name(&fake, "monoptah-x-000000").unwrap(), None);
    let fake = Fake::new(vec![ok(
        r#"[{"id":"sbx_a","name":"dup-1","agent":"shell"},{"id":"sbx_b","name":"dup-1","agent":"shell"}]"#,
    )]);
    let err = find_by_name(&fake, "dup-1").unwrap_err();
    assert!(err.contains("sbx_a") && err.contains("sbx_b"));
    assert!(find_by_name(&fake, "a.b").is_err());
}

#[test]
fn already_exists_is_resolvable_through_find_by_name() {
    let fake = Fake::new(vec![
        fail("error: code = already_exists"),
        ok(r#"[{"id":"sbx_b","name":"monoptah-x-000000","agent":"shell"}]"#),
    ]);
    let err = create(&fake, "monoptah-x-000000", &Compute::default()).unwrap_err();
    assert!(err.already_exists);
    assert_eq!(
        find_by_name(&fake, "monoptah-x-000000")
            .unwrap()
            .unwrap()
            .id,
        "sbx_b"
    );
}

#[test]
fn id_parsing_survives_spinners_and_ansi() {
    let out = "\u{1b}[2K\r- Pulling\r\u{1b}[2K\r\\ Starting\r\n\u{1b}[32msbx_001mabc123\u{1b}[0m\r\nready\n";
    assert_eq!(parse_created_id(out).as_deref(), Some("sbx_001mabc123"));
    assert_eq!(
        parse_created_id("\r\u{1b}[1Asbx_x1\r").as_deref(),
        Some("sbx_x1")
    );
    assert_eq!(parse_created_id("spin sbx_x1 spin\r"), None);
}

#[test]
fn redaction_catches_secrets_anywhere() {
    for (input, secret) in [
        ("clone https://user:s3cr3t@github.com/o/r failed", "s3cr3t"),
        (
            "clone https://ghp_abcdef@github.com/o/r failed",
            "ghp_abcdef",
        ),
        ("Authorization: Bearer abc.def-123 rejected", "abc.def-123"),
        (r#"{"token":"s3cr3t","x":1}"#, "s3cr3t"),
        (r#"{"access_token": "s3cr3t"}"#, "s3cr3t"),
        (r#"{"Password":"p\"w"}"#, "p\\\"w"),
        ("url=x?k=ghp_abc123def&b=1", "ghp_abc123def"),
        ("prefix:github_pat_11AAA_bbb suffix", "github_pat_11AAA_bbb"),
        ("(sk-ant-api03-zzz)", "sk-ant-api03-zzz"),
        ("--header=glpat-xyz123", "glpat-xyz123"),
        ("DB_PASSWORD=hunter2", "hunter2"),
    ] {
        let out = redact(input);
        assert!(!out.contains(secret), "{input} -> {out}");
        assert!(out.contains("[redacted]"), "{input} -> {out}");
    }
    assert_eq!(
        redact("no secrets here: sandbox ready"),
        "no secrets here: sandbox ready"
    );
    assert!(redact("https://github.com/o/r").contains("https://github.com/o/r"));
}

#[test]
fn run_survives_large_stdin_and_stdout_without_deadlock() {
    // Exercises the real runner's pipe handling with `cat` standing in for sbx.
    let mut cmd = Command::new("cat");
    cmd.stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(Stdio::piped());
    let text = "x".repeat(4 * 1024 * 1024);
    let out = run_piped(cmd, Some(&text), Duration::from_secs(20), "cat").unwrap();
    assert!(out.success);
    assert_eq!(out.stdout.len(), text.len());
}
