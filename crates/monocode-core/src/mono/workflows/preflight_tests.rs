use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::model::{
    InputType, InputValue, Node, NodeKind, Requirement, RequirementKind, Workflow, WorkflowInput,
};
use super::ordered::Json;
use super::preflight::*;
use crate::mcp::McpConnection;

const SENTINEL: &str = "sentinel-secret-value-123";

#[derive(Default)]
struct Fake {
    clis: HashSet<&'static str>,
    process_env: HashSet<&'static str>,
    login_env: Option<Vec<&'static str>>,
    servers: Vec<McpConnection>,
    servers_error: Option<&'static str>,
    statuses: Option<Vec<(&'static str, &'static str)>>,
    sleep: Option<Duration>,
    servers_sleep: Option<Duration>,
    login_noise: Option<&'static str>,
    login_asked: Mutex<Vec<Vec<String>>>,
    auth_cwds: Mutex<Vec<String>>,
    server_cwds: Mutex<Vec<String>>,
}

impl PreflightProbe for Fake {
    fn cli_found(&self, name: &str) -> bool {
        if let Some(d) = self.sleep {
            std::thread::sleep(d);
        }
        assert!(
            super::validate::is_cli_name(name),
            "invalid cli name reached the probe"
        );
        self.clis.contains(name)
    }
    fn env_process(&self, name: &str) -> bool {
        assert!(
            super::validate::is_env_name(name),
            "invalid env name reached the probe"
        );
        self.process_env.contains(name)
    }
    fn env_login(&self, names: &[String]) -> Option<Vec<String>> {
        self.login_asked.lock().unwrap().push(names.to_vec());
        // Like the native probe: raw shell output goes through `names_in_output`.
        let mut stdout = String::new();
        if let Some(noise) = self.login_noise {
            stdout.push_str(noise);
            stdout.push('\n');
        }
        for n in self.login_env.as_ref()? {
            stdout.push_str(n);
            stdout.push('\n');
        }
        Some(names_in_output(&stdout, names))
    }
    fn mcp_servers(&self, cwds: &[String]) -> Result<Vec<McpConnection>, String> {
        if let Some(d) = self.servers_sleep {
            std::thread::sleep(d);
        }
        self.server_cwds
            .lock()
            .unwrap()
            .extend(cwds.iter().cloned());
        match self.servers_error {
            Some(e) => Err(e.into()),
            None => Ok(self.servers.clone()),
        }
    }
    fn mcp_statuses(&self, cwd: &str) -> Option<Vec<(String, String)>> {
        self.auth_cwds.lock().unwrap().push(cwd.into());
        self.statuses.as_ref().map(|l| {
            l.iter()
                .map(|(n, s)| (n.to_string(), s.to_string()))
                .collect()
        })
    }
}

fn req(kind: RequirementKind, name: &str) -> Requirement {
    Requirement {
        kind,
        name: name.into(),
        hint: None,
    }
}

fn hinted(kind: RequirementKind, name: &str, hint: &str) -> Requirement {
    Requirement {
        kind,
        name: name.into(),
        hint: Some(hint.into()),
    }
}

fn command_node(id: &str) -> Node {
    Node::new(
        id,
        NodeKind::Command {
            project: "{{inputs.p}}".into(),
            command: "true".into(),
            cwd: None,
            timeout_minutes: None,
        },
    )
}

fn session_node(id: &str) -> Node {
    Node::new(
        id,
        NodeKind::Session {
            project: "{{inputs.p}}".into(),
            prompt: "go".into(),
            checkout: super::model::Checkout::Root,
            output: Default::default(),
            harness: None,
            model: None,
            effort: None,
            reuse_session: None,
            nest_under: None,
            timeout_minutes: None,
        },
    )
}

fn workflow(requires: Vec<Requirement>, command: bool, session: bool) -> Workflow {
    let mut w = Workflow::new("w", "W");
    w.requires = requires;
    if command {
        w.nodes.push(command_node("c"));
    }
    if session {
        w.nodes.push(session_node("s"));
    }
    w
}

fn mcp(provider: &str, name: &str, enabled: bool) -> McpConnection {
    McpConnection {
        provider: provider.into(),
        name: name.into(),
        scope: "user".into(),
        config_path: "/x".into(),
        transport: "http".into(),
        enabled,
    }
}

fn detail(p: &Preflight, i: usize) -> Option<&str> {
    p.statuses[i].detail.as_deref()
}

#[test]
fn cli_met_and_missing() {
    let probe = Fake {
        clis: ["gh"].into(),
        ..Default::default()
    };
    let w = workflow(
        vec![
            hinted(RequirementKind::Cli, "gh", "brew install gh"),
            req(RequirementKind::Cli, "jq"),
        ],
        true,
        false,
    );
    let p = evaluate(&w, &probe);
    assert!(p.statuses[0].met && p.statuses[0].detail.is_none());
    assert!(!p.statuses[1].met);
    assert_eq!(detail(&p, 1), Some("Not found on PATH"));
    assert!(!p.all_met());
    assert_eq!(p.missing().len(), 1);
}

#[test]
fn invalid_names_never_reach_the_probe() {
    // The fake panics on an invalid cli or env name.
    let probe = Fake::default();
    let w = workflow(
        vec![
            req(RequirementKind::Cli, "../bin/sh"),
            req(RequirementKind::Cli, "a b"),
            req(RequirementKind::Cli, ""),
            req(RequirementKind::Env, "1BAD"),
            req(RequirementKind::Env, "A;rm -rf"),
            req(RequirementKind::Env, "$(x)"),
        ],
        true,
        true,
    );
    let p = evaluate(&w, &probe);
    assert_eq!(p.statuses.len(), 6);
    for (i, s) in p.statuses.iter().enumerate() {
        assert!(!s.met);
        assert_eq!(detail(&p, i), Some("Invalid name"));
    }
    assert!(probe.login_asked.lock().unwrap().is_empty());
}

#[test]
fn env_in_process_is_met_for_every_kind_of_step() {
    let probe = Fake {
        process_env: ["TOKEN"].into(),
        ..Default::default()
    };
    for (c, s) in [(true, true), (true, false), (false, true), (false, false)] {
        let p = evaluate(
            &workflow(vec![req(RequirementKind::Env, "TOKEN")], c, s),
            &probe,
        );
        assert!(p.all_met(), "command={c} session={s}");
    }
    // Already in the process: the login shell is not asked.
    assert!(probe.login_asked.lock().unwrap().is_empty());
}

#[test]
fn env_matrix_login_only() {
    let probe = Fake {
        login_env: Some(vec!["TOKEN"]),
        ..Default::default()
    };
    let r = || vec![req(RequirementKind::Env, "TOKEN")];

    // Command only: the login shell is enough.
    assert!(evaluate(&workflow(r(), true, false), &probe).all_met());
    // Neither kind: either reach is enough.
    assert!(evaluate(&workflow(r(), false, false), &probe).all_met());
    // Both: session steps do not get it.
    let both = evaluate(&workflow(r(), true, true), &probe);
    assert!(!both.all_met());
    assert_eq!(
        detail(&both, 0),
        Some("Set for command steps only; session steps do not receive it")
    );
    // Session only: login is never asked and the variable is not set.
    let asked_before = probe.login_asked.lock().unwrap().len();
    let session = evaluate(&workflow(r(), false, true), &probe);
    assert!(!session.all_met());
    assert_eq!(detail(&session, 0), Some("Not set"));
    assert_eq!(probe.login_asked.lock().unwrap().len(), asked_before);
}

#[test]
fn env_unset_everywhere_and_login_unknown() {
    let nothing = Fake {
        login_env: Some(vec![]),
        ..Default::default()
    };
    let w = workflow(vec![req(RequirementKind::Env, "TOKEN")], true, true);
    let p = evaluate(&w, &nothing);
    assert!(!p.all_met());
    assert_eq!(detail(&p, 0), Some("Not set"));

    // None means could not tell, which counts as not reached.
    let unknown = Fake {
        login_env: None,
        ..Default::default()
    };
    let p = evaluate(
        &workflow(vec![req(RequirementKind::Env, "TOKEN")], true, false),
        &unknown,
    );
    assert!(!p.all_met());
    assert_eq!(detail(&p, 0), Some("Not set"));
}

#[test]
fn login_shell_is_asked_once_and_only_for_unset_names() {
    let probe = Fake {
        process_env: ["A"].into(),
        login_env: Some(vec!["B"]),
        ..Default::default()
    };
    let w = workflow(
        vec![
            req(RequirementKind::Env, "A"),
            req(RequirementKind::Env, "B"),
            req(RequirementKind::Env, "C"),
        ],
        true,
        false,
    );
    let p = evaluate(&w, &probe);
    assert!(p.statuses[0].met && p.statuses[1].met && !p.statuses[2].met);
    assert_eq!(
        *probe.login_asked.lock().unwrap(),
        vec![vec!["B".to_string(), "C".to_string()]]
    );
}

#[test]
fn mcp_not_configured_disabled_and_other_provider() {
    let probe = Fake {
        servers: vec![
            mcp("cursor", "docs", true),
            mcp("cursor", "off", false),
            mcp("claude", "off", false),
        ],
        ..Default::default()
    };
    let w = workflow(
        vec![
            req(RequirementKind::Mcp, "docs"),
            req(RequirementKind::Mcp, "off"),
            req(RequirementKind::Mcp, "nope"),
            req(RequirementKind::Mcp, "Docs"),
        ],
        false,
        true,
    );
    let p = evaluate(&w, &probe);
    assert!(p.statuses[0].met && p.statuses[0].detail.is_none());
    assert_eq!(detail(&p, 1), Some("Disabled"));
    assert_eq!(detail(&p, 2), Some("Not configured"));
    // Exact match only.
    assert_eq!(detail(&p, 3), Some("Not configured"));
    // Nothing needed the claude health check.
    assert!(probe.auth_cwds.lock().unwrap().is_empty());
}

#[test]
fn mcp_claude_needs_auth_confirmed_and_unknown() {
    let servers = vec![mcp("claude", "linear", true), mcp("claude", "docs", true)];
    let w = workflow(
        vec![
            req(RequirementKind::Mcp, "linear"),
            req(RequirementKind::Mcp, "docs"),
        ],
        false,
        true,
    );

    let needs = Fake {
        servers: servers.clone(),
        statuses: Some(vec![
            ("linear", "Needs authentication"),
            ("docs", "Connected"),
        ]),
        ..Default::default()
    };
    let p = evaluate(&w, &needs);
    assert!(!p.statuses[0].met);
    assert_eq!(detail(&p, 0), Some("Configured but needs authentication"));
    assert!(p.statuses[1].met && p.statuses[1].detail.is_none());

    let unknown = Fake {
        servers,
        statuses: None,
        ..Default::default()
    };
    let p = evaluate(&w, &unknown);
    assert!(p.all_met());
    assert_eq!(detail(&p, 0), Some("Sign-in could not be confirmed"));
    assert_eq!(detail(&p, 1), Some("Sign-in could not be confirmed"));
}

#[test]
fn mcp_discover_error_marks_every_mcp_requirement() {
    let probe = Fake {
        servers_error: Some("Home directory not found"),
        clis: ["gh"].into(),
        ..Default::default()
    };
    let w = workflow(
        vec![
            req(RequirementKind::Mcp, "docs"),
            req(RequirementKind::Cli, "gh"),
        ],
        false,
        true,
    );
    let p = evaluate(&w, &probe);
    assert!(!p.statuses[0].met);
    assert_eq!(detail(&p, 0), Some("Could not list MCP servers"));
    assert!(p.statuses[1].met);
}

#[test]
fn mcp_looks_in_project_inputs_and_home_only() {
    // An absolute path on every platform: `/work/a` is not one on Windows.
    let absolute = |name: &str| std::env::temp_dir().join(name).display().to_string();
    let (stored, overridden, other) = (absolute("a"), absolute("b"), absolute("not-a-project"));
    let probe = Fake::default();
    let mut w = workflow(vec![req(RequirementKind::Mcp, "docs")], false, true);
    w.inputs.insert("p", WorkflowInput::new(InputType::Project));
    w.inputs.insert("q", WorkflowInput::new(InputType::Project));
    w.inputs
        .insert("note", WorkflowInput::new(InputType::String));
    w.input_values
        .insert("p", InputValue::String(stored.clone()));
    w.input_values
        .insert("q", InputValue::String("relative/dir".into()));
    w.input_values
        .insert("note", InputValue::String(other.clone()));
    let mut run = super::ordered::OrderedMap::new();
    run.insert("p", InputValue::String(overridden.clone()));
    w.run_values = Some(run);
    evaluate(&w, &probe);
    let cwds = probe.server_cwds.lock().unwrap().clone();
    assert_eq!(cwds.first(), Some(&overridden));
    assert!(!cwds
        .iter()
        .any(|c| c == "relative/dir" || *c == other || *c == stored));
}

#[test]
fn summary_and_missing_json() {
    let probe = Fake {
        clis: ["ok"].into(),
        login_env: Some(vec![]),
        ..Default::default()
    };
    let w = workflow(
        vec![
            req(RequirementKind::Cli, "ok"),
            hinted(RequirementKind::Cli, "gh", "brew install gh"),
            hinted(RequirementKind::Env, "TOKEN", "create one in settings"),
            req(RequirementKind::Mcp, "docs"),
        ],
        true,
        false,
    );
    let p = evaluate(&w, &probe);
    assert_eq!(
        p.summary(),
        "cli gh: not found on PATH (brew install gh); env TOKEN: not set (create one in settings); mcp docs: not configured"
    );
    let json = p.missing_json();
    let items = json.as_array().unwrap();
    assert_eq!(items.len(), 3);
    assert_eq!(items[0].get("kind").unwrap().as_str(), Some("cli"));
    assert_eq!(items[0].get("name").unwrap().as_str(), Some("gh"));
    assert_eq!(
        items[0].get("detail").unwrap().as_str(),
        Some("Not found on PATH")
    );
    assert_eq!(
        items[0].get("hint").unwrap().as_str(),
        Some("brew install gh")
    );
    assert!(items[2].get("hint").is_none());

    let met = evaluate(
        &workflow(vec![req(RequirementKind::Cli, "ok")], true, false),
        &probe,
    );
    assert_eq!(met.summary(), "");
    assert_eq!(met.missing_json(), Json::Array(vec![]));
}

#[test]
fn no_requirements_is_trivially_met() {
    let p = evaluate(&workflow(vec![], true, true), &Fake::default());
    assert!(p.all_met() && p.statuses.is_empty());
    assert!(native_preflight(&workflow(vec![], true, true)).all_met());
}

#[test]
fn timeout_marks_everything_missing_and_returns_promptly() {
    let probe = Arc::new(Fake {
        clis: ["gh"].into(),
        sleep: Some(Duration::from_secs(3)),
        ..Default::default()
    });
    let w = workflow(
        vec![
            req(RequirementKind::Cli, "gh"),
            req(RequirementKind::Env, "X"),
        ],
        true,
        false,
    );
    let start = Instant::now();
    let p = super::preflight::evaluate_bounded(&w, probe, Duration::from_millis(150));
    assert!(start.elapsed() < Duration::from_secs(1));
    assert_eq!(p.statuses.len(), 2);
    for (i, s) in p.statuses.iter().enumerate() {
        assert!(!s.met);
        assert_eq!(detail(&p, i), Some("Could not check in time; try again"));
    }
}

#[test]
fn bounded_returns_the_real_result_when_in_time() {
    let probe = Arc::new(Fake {
        clis: ["gh"].into(),
        ..Default::default()
    });
    let w = workflow(vec![req(RequirementKind::Cli, "gh")], true, false);
    let p = super::preflight::evaluate_bounded(&w, probe, Duration::from_secs(5));
    assert!(p.all_met());
}

#[test]
fn union_dedupes_by_kind_and_name_and_keeps_a_hint() {
    let a = vec![
        req(RequirementKind::Cli, "gh"),
        hinted(RequirementKind::Env, "T", "first"),
    ];
    let b = vec![
        hinted(RequirementKind::Cli, "gh", "brew install gh"),
        hinted(RequirementKind::Env, "T", "second"),
        req(RequirementKind::Mcp, "gh"),
    ];
    let u = union_requires(&a, &b);
    assert_eq!(u.len(), 3);
    assert_eq!(u[0].hint.as_deref(), Some("brew install gh"));
    assert_eq!(u[1].hint.as_deref(), Some("first"));
    assert_eq!(u[2].kind, RequirementKind::Mcp);
}

#[cfg(unix)]
#[test]
fn login_script_prints_names_never_values() {
    let names = vec![
        "PREFLIGHT_TEST_SET".to_string(),
        "PREFLIGHT_TEST_UNSET".to_string(),
    ];
    let script = login_env_script(&names);
    assert!(script.contains("PREFLIGHT_TEST_SET"));
    assert!(!script.contains(SENTINEL));
    // A name that is not a variable name is dropped, so it can never be shell text.
    assert!(!login_env_script(&["A; echo hi".to_string()]).contains("echo"));

    let out = std::process::Command::new("/bin/sh")
        .arg("-c")
        .arg(&script)
        .env("PREFLIGHT_TEST_SET", SENTINEL)
        .env_remove("PREFLIGHT_TEST_UNSET")
        .output()
        .unwrap();
    let stdout = String::from_utf8(out.stdout).unwrap();
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(stdout.contains("PREFLIGHT_TEST_SET"));
    assert!(!stdout.contains("PREFLIGHT_TEST_UNSET"));
    assert!(!stdout.contains(SENTINEL) && !stderr.contains(SENTINEL));
    assert_eq!(names_in_output(&stdout, &names), vec!["PREFLIGHT_TEST_SET"]);
}

#[test]
fn names_in_output_drops_noise() {
    let names = vec!["A".to_string(), "B".to_string()];
    let got = names_in_output("welcome to zsh\nA\r\nAB\nnot B\nB\n", &names);
    assert_eq!(got, vec!["A", "B"]);
}

#[test]
fn login_noise_never_reaches_summary_or_json() {
    // The fake's login output carries a sentinel line; only exact requested names survive.
    let probe = Fake {
        login_env: Some(vec!["TOKEN"]),
        login_noise: Some(SENTINEL),
        ..Default::default()
    };
    let w = workflow(
        vec![
            req(RequirementKind::Env, "TOKEN"),
            req(RequirementKind::Env, "OTHER"),
        ],
        true,
        true,
    );
    let p = evaluate(&w, &probe);
    assert!(!p.all_met());
    assert_eq!(detail(&p, 0), Some(COMMAND_ONLY_TEXT));
    let all = format!(
        "{:?}{}{}",
        p,
        p.summary(),
        p.missing_json().stringify(false)
    );
    assert!(!all.contains(SENTINEL));
}

const COMMAND_ONLY_TEXT: &str = "Set for command steps only; session steps do not receive it";

#[test]
fn deadline_keeps_statuses_already_decided() {
    let probe = Arc::new(Fake {
        clis: ["gh"].into(),
        servers_sleep: Some(Duration::from_secs(3)),
        ..Default::default()
    });
    let w = workflow(
        vec![
            req(RequirementKind::Mcp, "docs"),
            req(RequirementKind::Cli, "gh"),
            req(RequirementKind::Cli, "jq"),
        ],
        false,
        true,
    );
    let start = Instant::now();
    let p = super::preflight::evaluate_bounded(&w, probe, Duration::from_millis(200));
    assert!(start.elapsed() < Duration::from_secs(1));
    assert!(p.statuses[1].met && p.statuses[1].detail.is_none());
    assert_eq!(detail(&p, 2), Some("Not found on PATH"));
    assert!(!p.statuses[0].met);
    assert_eq!(detail(&p, 0), Some("Could not check in time; try again"));
}

#[test]
fn mcp_claude_connected_absent_and_partial_none() {
    let w = workflow(vec![req(RequirementKind::Mcp, "docs")], false, true);
    let servers = vec![mcp("claude", "docs", true)];
    let connected = Fake {
        servers: servers.clone(),
        statuses: Some(vec![("docs", "Connected")]),
        ..Default::default()
    };
    assert_eq!(detail(&evaluate(&w, &connected), 0), None);
    // Absent from the parsed output: met, sign-in unconfirmed.
    let absent = Fake {
        servers,
        statuses: Some(vec![("other", "Connected")]),
        ..Default::default()
    };
    let p = evaluate(&w, &absent);
    assert!(p.all_met());
    assert_eq!(detail(&p, 0), Some("Sign-in could not be confirmed"));
}

#[test]
fn probe_deadline_ends_before_the_collector() {
    let start = Instant::now();
    let probe = super::preflight::probe_deadline(start);
    assert!(probe < start + super::preflight::PREFLIGHT_TIMEOUT);
    assert_eq!(
        start + super::preflight::PREFLIGHT_TIMEOUT - probe,
        Duration::from_millis(500)
    );
}

// ---- likely_clis ------------------------------------------------------------------------

fn clis(command: &str) -> Vec<String> {
    likely_clis(command)
}

#[test]
fn likely_clis_takes_the_first_word_of_each_segment() {
    assert_eq!(clis("gh pr list | jq ."), ["gh", "jq"]);
    assert_eq!(clis("npm ci && npm test; cargo build"), ["npm", "cargo"]);
    assert_eq!(
        clis("make build || docker compose up\nkubectl get pods"),
        ["make", "docker", "kubectl"]
    );
    assert_eq!(clis("  gh   pr view"), ["gh"]);
}

#[test]
fn likely_clis_skips_leading_assignments_and_quoted_assignment_values() {
    assert_eq!(clis("CI=1 FOO=bar npm test"), ["npm"]);
    assert_eq!(clis("MSG=\"two words\" gh pr list"), ["gh"]);
    assert!(clis("FOO=bar").is_empty());
}

#[test]
fn likely_clis_ignores_separators_inside_quotes() {
    assert_eq!(clis("echo \"a; rm b | c\" && gh pr list"), ["gh"]);
    assert_eq!(clis("echo 'x && y' | jq ."), ["jq"]);
}

#[test]
fn likely_clis_skips_a_segment_with_a_template_substitution_or_backtick() {
    assert!(clis("{{inputs.tool}} --version").is_empty());
    assert!(clis("gh pr view {{inputs.pr}}").is_empty());
    assert!(clis("$(which gh) pr list").is_empty());
    assert!(clis("`which gh` pr list").is_empty());
    // Only that segment is dropped.
    assert_eq!(clis("gh pr view {{inputs.pr}} && jq ."), ["jq"]);
}

#[test]
fn likely_clis_ignores_paths_shell_words_and_base_utilities() {
    for command in [
        "./run.sh",
        "/usr/bin/gh pr list",
        "../bin/tool",
        "~/bin/tool",
        "if true; then echo hi; fi",
        "for f in a b; do echo $f; done",
        "while read l; do echo $l; done",
        "cd sub && export A=1",
        "source env.sh",
        "ls -la | sort | head -n 1 | wc -l",
        "cat a | grep b | sed s/a/b/ | awk 1 | cut -d, -f1 | tr a b | tail -1",
        "mkdir -p x; cp a b; mv a b; rm -rf x; touch f; test -f f; printf hi; find . | xargs echo",
        "date; env; sleep 1; basename a; dirname a; pwd; sh -c x; bash x; true; false",
        "{ echo hi; }",
        "[ -f x ]",
        "! gh pr list",
    ] {
        assert!(clis(command).is_empty(), "{command}: {:?}", clis(command));
    }
}

#[test]
fn likely_clis_lists_each_cli_once_in_order_of_first_use() {
    assert_eq!(clis("gh a; jq .; gh b"), ["gh", "jq"]);
    assert!(clis("").is_empty());
}

#[test]
fn likely_clis_ignores_heredoc_bodies() {
    assert!(clis("cat > f <<EOF\nhello world\nEOF").is_empty());
    assert_eq!(clis("gh pr list && cat <<'EOF'\nit's jq here\nEOF"), ["gh"]);
    assert!(clis("cat <<-EOF\n\tkubectl get\n\tEOF").is_empty());
}

#[test]
fn likely_clis_ignores_comments_but_not_a_hash_inside_a_word() {
    assert!(clis("# first; run stuff").is_empty());
    assert_eq!(
        clis("gh pr list # then jq; docker ps\nkubectl get"),
        ["gh", "kubectl"]
    );
    assert_eq!(clis("echo $# && gh x"), ["gh"]);
    assert_eq!(clis("echo \"# not a comment\"; gh x"), ["gh"]);
}
