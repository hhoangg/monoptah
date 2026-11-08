use super::startup::{log_step, LogMark};
use super::*;
use crate::codespaces::{GhOutput, GhRunner};
use crate::remote::Machine;
use std::collections::VecDeque;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

const AUTH_OK: &str = "github.com\n  ✓ Logged in to github.com account me (keyring)\n  - Token scopes: 'codespace', 'repo'\n";
const CREATED: &str =
    r#"{"name":"octo-new1","state":"Provisioning","web_url":"https://octo-new1.github.dev"}"#;
const LIST: &str = r#"[{"name":"octo-abc","displayName":"my cs","state":"Shutdown","machineName":"basicLinux32gb","repository":"octo/widgets"}]"#;
/// A creation log as measured on a real codespace.
const LOG: &[&str] = &[
    "2026-11-07 04:01:54.456Z: Configuration starting...",
    "2026-11-07 04:01:54.500Z: Cloning...",
    "2026-11-07 04:01:58.000Z: Using image: mcr.microsoft.com/devcontainers/universal",
    "2026-11-07 04:01:58.100Z: Creating container...",
    "2026-11-07 04:01:59.000Z: Running blocking commands...",
    "$ devcontainer up --workspace-folder /workspaces/widgets --skip-non-blocking-commands",
    "Outcome: success User: codespace WorkspaceFolder: /workspaces/widgets",
    "==================================================",
    "2026-11-07 04:02:00.000Z: Configuring codespace...",
    "2026-11-07 04:02:00.500Z: Running oryx...",
    "2026-11-07 04:04:46.000Z: Running commands...",
    "2026-11-07 04:04:46.100Z: Running the postCreateCommand from Feature 'ghcr.io/devcontainers/features/git-lfs:1'...",
    "2026-11-07 04:05:04.000Z: Finished configuring codespace.",
];

/// Scripted gh: `states` answer `codespace view` in order (the last one repeats), `log` is what
/// the creation-log follower streams.
struct Gh {
    states: Mutex<VecDeque<String>>,
    log: Vec<&'static str>,
    follow_error: bool,
    calls: Mutex<Vec<String>>,
    /// Set once the follower delivered its lines; the host waits for it to stay deterministic.
    delivered: Arc<AtomicBool>,
    on_view: Mutex<Option<Box<dyn Fn() + Send>>>,
}
impl Gh {
    fn new(states: &[&str], log: &[&'static str]) -> Arc<Self> {
        Arc::new(Self {
            states: Mutex::new(states.iter().map(|s| s.to_string()).collect()),
            log: log.to_vec(),
            follow_error: false,
            calls: Mutex::new(vec![]),
            delivered: Arc::new(AtomicBool::new(log.is_empty())),
            on_view: Mutex::new(None),
        })
    }
    fn failing_follower(states: &[&str]) -> Arc<Self> {
        let mut gh = Arc::try_unwrap(Self::new(states, &[])).ok().unwrap();
        gh.follow_error = true;
        Arc::new(gh)
    }
    fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }
    fn count(&self, needle: &str) -> usize {
        self.calls().iter().filter(|c| c.contains(needle)).count()
    }
    fn position(&self, needle: &str) -> Option<usize> {
        self.calls().iter().position(|c| c.contains(needle))
    }
}
fn ok(stdout: &str) -> Result<GhOutput, String> {
    Ok(GhOutput {
        success: true,
        stdout: stdout.into(),
        stderr: String::new(),
    })
}
impl GhRunner for Gh {
    fn run(&self, args: &[String], _stdin: Option<&str>) -> Result<GhOutput, String> {
        let line = args.join(" ");
        self.calls.lock().unwrap().push(line.clone());
        if line.starts_with("auth status") {
            ok(AUTH_OK)
        } else if line.starts_with("api --method POST") {
            ok(CREATED)
        } else if line.starts_with("api user/codespaces") {
            ok("https://asked.github.dev\n")
        } else if line.starts_with("codespace list") {
            ok(LIST)
        } else if line.starts_with("codespace rebuild") || line.starts_with("codespace ssh") {
            ok("")
        } else if line.starts_with("codespace view") {
            if let Some(hook) = self.on_view.lock().unwrap().as_ref() {
                hook();
            }
            let mut states = self.states.lock().unwrap();
            let state = if states.len() > 1 {
                states.pop_front().unwrap()
            } else {
                states
                    .front()
                    .cloned()
                    .unwrap_or_else(|| "Available".into())
            };
            ok(&format!(r#"{{"state":"{state}"}}"#))
        } else {
            panic!("unexpected gh call: {line}")
        }
    }
    fn run_streaming_until(
        &self,
        args: &[String],
        on_line: &mut dyn FnMut(&str),
        _stop: &AtomicBool,
    ) -> Result<GhOutput, String> {
        self.calls
            .lock()
            .unwrap()
            .push(format!("TAIL {}", args.join(" ")));
        if self.follow_error {
            return Err("Permission denied (publickey)".into());
        }
        self.log.iter().for_each(|l| on_line(l));
        self.delivered.store(true, Ordering::SeqCst);
        ok("")
    }
}

#[derive(Default)]
struct Host {
    machines: Mutex<Vec<Machine>>,
    log: Mutex<Vec<String>>,
    connect_error: Mutex<Option<String>>,
    wait_for: Mutex<Option<Arc<AtomicBool>>>,
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
impl super::Host for Host {
    fn machines(&self) -> Result<Vec<Machine>, String> {
        Ok(self.machines.lock().unwrap().clone())
    }
    fn connect(
        &self,
        codespace: &str,
        display: &str,
        progress: &mut dyn FnMut(&str),
    ) -> Result<Machine, String> {
        let gate = self.wait_for.lock().unwrap().clone();
        if let Some(gate) = gate {
            let until = std::time::Instant::now() + Duration::from_secs(10);
            while !gate.load(Ordering::SeqCst) && std::time::Instant::now() < until {
                std::thread::sleep(Duration::from_millis(5));
            }
        }
        progress("pairing");
        self.log
            .lock()
            .unwrap()
            .push(format!("connect {codespace} {display}"));
        if let Some(error) = self.connect_error.lock().unwrap().clone() {
            return Err(error);
        }
        let m = machine(codespace);
        self.machines.lock().unwrap().push(m.clone());
        Ok(m)
    }
    fn ensure(&self, _: &str, _: &AtomicBool, _: &mut dyn FnMut(&str)) -> Result<(), String> {
        Ok(())
    }
    fn request(&self, machine_id: &str, method: &str, params: Value) -> Result<Value, String> {
        self.log
            .lock()
            .unwrap()
            .push(format!("request {machine_id} {method}"));
        Ok(json!({"id":"project-1","cwd":params["cwd"]}))
    }
    fn disconnect(&self, machine_id: &str) -> Result<(), String> {
        self.machines.lock().unwrap().retain(|m| m.id != machine_id);
        Ok(())
    }
}

struct Fx {
    svc: CloudSessions,
    gh: Arc<Gh>,
    host: Arc<Host>,
    store: Arc<Mutex<Store>>,
}
fn fx(gh: Arc<Gh>) -> Fx {
    let host = Arc::new(Host::default());
    *host.wait_for.lock().unwrap() = Some(gh.delivered.clone());
    let store = Arc::new(Mutex::new(Store::open(Path::new(":memory:")).unwrap()));
    let tick = Arc::new(AtomicU64::new(1_794_024_100_000));
    let svc = CloudSessions::new(gh.clone(), host.clone(), store.clone())
        .with_origin(|_| Ok("git@github.com:octo/widgets.git".into()))
        .with_timing(
            Duration::from_secs(5),
            |_| {},
            move || tick.fetch_add(1000, Ordering::SeqCst),
        );
    Fx {
        svc,
        gh,
        host,
        store,
    }
}
fn new_target() -> Target {
    Target {
        codespace: CodespaceChoice::New,
        branch: Some("main".into()),
        machine: None,
    }
}
fn named_target() -> Target {
    Target {
        codespace: CodespaceChoice::Named("octo-abc".into()),
        branch: None,
        machine: None,
    }
}
impl Fx {
    fn record(&self, id: i64) -> Option<StartRecord> {
        start_record(&self.store.lock().unwrap(), id)
    }
    fn run(&self, id: i64) -> (Result<CloudBinding, String>, Vec<StartEvent>) {
        let mut events = vec![];
        let result = self
            .svc
            .run_start(id, &mut |e| events.push(e), &AtomicBool::new(false));
        (result, events)
    }
}
fn status(record: &StartRecord, kind: StepKind) -> StepStatus {
    record.step(kind).status
}

// ---------------------------------------------------------------- pure model

#[test]
fn steps_have_a_fixed_order_labels_and_ids() {
    assert_eq!(StepKind::ALL.len(), 8);
    assert_eq!(StepKind::ALL[0], StepKind::Create);
    assert_eq!(StepKind::ALL[7], StepKind::LifecycleCommands);
    let ids: Vec<_> = StepKind::ALL.iter().map(|k| k.id()).collect();
    assert_eq!(
        ids,
        [
            "create",
            "provision",
            "buildContainer",
            "blockingCommands",
            "configureCodespace",
            "connect",
            "openProject",
            "lifecycleCommands"
        ]
    );
    for kind in StepKind::ALL {
        // The serde name is the id of the app API.
        assert_eq!(serde_json::to_value(kind).unwrap(), json!(kind.id()));
    }
    assert_eq!(
        StepKind::BlockingCommands.label(),
        "Lifecycle commands (waitFor)"
    );
    assert_eq!(
        StepKind::BlockingCommands.running_label(),
        "Running lifecycle commands"
    );
    assert_eq!(StepKind::Connect.running_label(), "Connecting");
    assert_eq!(
        StepKind::LifecycleCommands.label(),
        "Background lifecycle commands"
    );
}

#[test]
fn log_markers_map_to_steps() {
    let text = |raw: &str| {
        codespaces::parse_log_line(raw)
            .map(|l| l.text)
            .unwrap_or_default()
    };
    let mark = |raw: &str| log_step(&text(raw));
    assert_eq!(mark(LOG[0]), Some(LogMark::Provision));
    assert_eq!(mark(LOG[1]), Some(LogMark::Provision));
    assert_eq!(mark(LOG[2]), None);
    assert_eq!(mark(LOG[3]), Some(LogMark::Start(StepKind::BuildContainer)));
    assert_eq!(
        mark(LOG[4]),
        Some(LogMark::Start(StepKind::BlockingCommands))
    );
    assert_eq!(mark(LOG[5]), None);
    assert_eq!(
        mark(LOG[8]),
        Some(LogMark::Start(StepKind::ConfigureCodespace))
    );
    assert_eq!(mark(LOG[9]), None);
    assert_eq!(
        mark(LOG[10]),
        Some(LogMark::Start(StepKind::LifecycleCommands))
    );
    assert_eq!(
        mark(LOG[11]),
        Some(LogMark::Start(StepKind::LifecycleCommands))
    );
    assert_eq!(mark(LOG[12]), Some(LogMark::Finished));
    assert_eq!(
        log_step("Running the postStartCommand from Feature 'x'..."),
        Some(LogMark::Start(StepKind::LifecycleCommands))
    );
}

fn fresh_record() -> StartRecord {
    let resolved = ResolvedCodespace {
        name: "cs-1".into(),
        display: "monoptah-main".into(),
        repo: "octo/widgets".into(),
        remote_cwd: "/workspaces/widgets".into(),
        created: true,
        web_url: None,
        state: "Provisioning".into(),
        branch: Some("main".into()),
        machine: None,
    };
    StartRecord::fresh(&resolved, -7, "/p", 1000)
}

fn replay(record: &mut StartRecord, lines: &[&str]) {
    for raw in lines {
        if let Some(line) = codespaces::parse_log_line(raw) {
            record.apply_log(&line, 0);
        }
    }
}

#[test]
fn replaying_the_log_gives_real_durations_and_the_chip_step() {
    let mut record = fresh_record();
    assert_eq!(record.steps.len(), 8);
    // Up to "Running oryx": configuring is running and is what the chip shows.
    replay(&mut record, &LOG[..10]);
    let build = record.step(StepKind::BuildContainer);
    assert_eq!(build.status, StepStatus::Done);
    assert_eq!(build.finished_at.unwrap() - build.started_at.unwrap(), 900);
    let blocking = record.step(StepKind::BlockingCommands);
    assert_eq!(
        blocking.finished_at.unwrap() - blocking.started_at.unwrap(),
        1000
    );
    assert_eq!(
        status(&record, StepKind::ConfigureCodespace),
        StepStatus::Running
    );
    assert_eq!(record.current(), Some(StepKind::ConfigureCodespace));
    // Lifecycle commands start: configuring ends at the same instant, the detail is the line.
    replay(&mut record, &LOG[10..12]);
    let configure = record.step(StepKind::ConfigureCodespace);
    assert_eq!(configure.status, StepStatus::Done);
    assert_eq!(
        configure.finished_at.unwrap() - configure.started_at.unwrap(),
        166_000
    );
    let lifecycle = record.step(StepKind::LifecycleCommands);
    assert_eq!(lifecycle.status, StepStatus::Running);
    assert!(lifecycle
        .detail
        .as_deref()
        .unwrap()
        .starts_with("Running the postCreateCommand"));
    assert_eq!(record.current(), Some(StepKind::LifecycleCommands));
    replay(&mut record, &LOG[12..]);
    let lifecycle = record.step(StepKind::LifecycleCommands);
    assert_eq!(lifecycle.status, StepStatus::Done);
    assert_eq!(
        lifecycle.finished_at.unwrap() - lifecycle.started_at.unwrap(),
        18_000
    );
    // A replay changes nothing.
    let before = record.clone();
    replay(&mut record, LOG);
    assert_eq!(record, before);
}

#[test]
fn the_chip_prefers_main_flow_steps_and_falls_back_to_provision() {
    let mut record = fresh_record();
    assert_eq!(record.current(), None);
    record.start_step(StepKind::Provision, 1);
    assert_eq!(record.current(), Some(StepKind::Provision));
    replay(&mut record, &LOG[8..9]);
    assert_eq!(record.current(), Some(StepKind::ConfigureCodespace));
    record.start_step(StepKind::Connect, 2);
    assert_eq!(record.current(), Some(StepKind::Connect));
    assert_eq!(record.elapsed_ms(5000), 4000);
    record.finished_at = Some(3000);
    assert_eq!(record.elapsed_ms(5000), 2000);
    assert!(!record.is_failed());
}

#[test]
fn a_main_flow_failure_wins_over_a_running_log_step() {
    let mut record = fresh_record();
    replay(&mut record, &LOG[3..9]);
    record.start_step(StepKind::Connect, 1);
    record.fail("boom".into(), 9);
    assert!(record.is_failed());
    assert_eq!(record.failed_at, Some(StepKind::Connect));
    assert_eq!(status(&record, StepKind::Connect), StepStatus::Failed);
    assert_eq!(
        status(&record, StepKind::ConfigureCodespace),
        StepStatus::Skipped
    );
    // A retry redoes what failed or was interrupted and keeps finished steps.
    record.reset_for_run();
    assert_eq!(record.error, None);
    assert_eq!(record.failed_at, None);
    assert_eq!(status(&record, StepKind::Connect), StepStatus::Pending);
    assert_eq!(
        status(&record, StepKind::ConfigureCodespace),
        StepStatus::Pending
    );
    assert_eq!(status(&record, StepKind::BuildContainer), StepStatus::Done);
}

#[test]
fn the_log_tail_keeps_the_last_forty_lines() {
    let mut record = fresh_record();
    for i in 0..55 {
        record.push_log(&format!("line {i}"));
    }
    assert_eq!(record.log_tail.len(), 40);
    assert_eq!(record.log_tail[0], "line 15");
    assert_eq!(record.log_tail[39], "line 54");
}

// ------------------------------------------------------------------- begin

#[test]
fn begin_start_creates_the_pending_session_and_its_record_in_seconds() {
    let f = fx(Gh::new(&["Provisioning"], &[]));
    let (session, record) = f
        .svc
        .begin_start("/Users/me/widgets", new_target(), "claude")
        .unwrap();
    assert_eq!(record.codespace, "octo-new1");
    assert!(record.created && record.track_ready);
    assert_eq!(record.branch.as_deref(), Some("main"));
    assert_eq!(record.machine.as_deref(), Some("basicLinux32gb"));
    assert_eq!(record.display, "monoptah-main");
    assert_eq!(
        record.web_url.as_deref(),
        Some("https://octo-new1.github.dev")
    );
    assert_eq!(record.remote_cwd, "/workspaces/widgets");
    assert_eq!(record.gh_state.as_deref(), Some("Provisioning"));
    assert_eq!(
        record.steps.iter().map(|s| s.kind).collect::<Vec<_>>(),
        StepKind::ALL
    );
    assert_eq!(status(&record, StepKind::Create), StepStatus::Done);
    assert_eq!(
        record.step(StepKind::Create).detail.as_deref(),
        Some("octo-new1")
    );
    assert_eq!(status(&record, StepKind::Provision), StepStatus::Running);
    assert_eq!(record.current(), Some(StepKind::Provision));
    // Nothing waited and nothing connected.
    assert_eq!(f.gh.count("codespace view"), 0);
    assert!(f.host.log.lock().unwrap().is_empty());
    // The session is a cloud session of the local project on the provisional project.
    let store = f.store.lock().unwrap();
    let stored = store.remote_session(session.id).unwrap().unwrap();
    assert_eq!(stored.project.environment_id, "pending-octo-new1");
    assert_eq!(stored.project.codespace.as_deref(), Some("octo-new1"));
    assert_eq!(
        stored.project.local_project.as_deref(),
        Some("/Users/me/widgets")
    );
    assert_eq!(session.cwd, "remote://pending-octo-new1/workspaces/widgets");
    assert_eq!(store.cloud_sessions("/Users/me/widgets").unwrap().len(), 1);
    assert!(store.remote_projects().unwrap().is_empty());
    assert_eq!(start_record(&store, session.id), Some(record.clone()));
    assert_eq!(pending_starts(&store), vec![record]);
}

#[test]
fn begin_start_for_a_named_codespace_skips_create() {
    let f = fx(Gh::new(&["Shutdown"], &[]));
    let (_, record) = f.svc.begin_start("/p", named_target(), "claude").unwrap();
    assert!(!record.created && !record.track_ready);
    assert_eq!(status(&record, StepKind::Create), StepStatus::Skipped);
    assert_eq!(status(&record, StepKind::Provision), StepStatus::Running);
    assert_eq!(record.gh_state.as_deref(), Some("Shutdown"));
    assert_eq!(record.display, "my cs");
    assert_eq!(record.machine.as_deref(), Some("basicLinux32gb"));
}

#[test]
fn begin_start_failing_before_a_codespace_exists_creates_nothing() {
    let f = fx(Gh::new(&[], &[]));
    let error = f.svc.begin_start("/p", new_target(), "codex").unwrap_err();
    assert!(error.contains("Claude only"), "{error}");
    let no_branch = Target {
        branch: Some(" ".into()),
        ..new_target()
    };
    assert!(f
        .svc
        .begin_start("/p", no_branch, "claude")
        .unwrap_err()
        .contains("branch"));
    let unknown = Target {
        codespace: CodespaceChoice::Named("octo-zzz".into()),
        ..named_target()
    };
    assert!(f
        .svc
        .begin_start("/p", unknown, "claude")
        .unwrap_err()
        .contains("does not belong"));
    assert_eq!(f.gh.count("--method POST"), 0);
    let store = f.store.lock().unwrap();
    assert!(store.sessions().unwrap().is_empty());
    assert!(pending_starts(&store).is_empty());
}

// --------------------------------------------------------------------- run

#[test]
fn run_start_tracks_the_codespace_follows_the_log_and_rebinds_the_session() {
    let f = fx(Gh::new(&["Provisioning", "Starting", "Available"], LOG));
    let (session, _) = f.svc.begin_start("/p", new_target(), "claude").unwrap();
    let (result, events) = f.run(session.id);
    let binding = result.unwrap();
    assert_eq!(binding.codespace, "octo-new1");
    assert_eq!(
        binding.project_key,
        "remote://env-octo-new1/workspaces/widgets"
    );
    // gh states were announced as they changed.
    let states: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            StartEvent::Updated(r) => r.gh_state.clone(),
            _ => None,
        })
        .collect();
    let mut seen: Vec<String> = vec![];
    for s in states {
        if seen.last() != Some(&s) {
            seen.push(s);
        }
    }
    assert_eq!(seen, ["Provisioning", "Starting", "Available"]);
    // Log lines arrive once each, without timestamps, separators or blanks.
    let logs: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            StartEvent::Log(l) => Some(l.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(
        logs.iter()
            .filter(|l| *l == "Creating container...")
            .count(),
        1
    );
    assert!(!logs
        .iter()
        .any(|l| l.starts_with("====") || l.contains("04:01")));
    assert!(logs.contains(&"[monoptah] pairing".to_string()));
    // The follower started only once the codespace was Available.
    let available =
        f.gh.calls()
            .iter()
            .rposition(|c| c.contains("codespace view"))
            .unwrap();
    let tail = f.gh.position("TAIL").unwrap();
    assert!(tail > available, "{:?}", f.gh.calls());
    assert_eq!(
        f.gh.count("TAIL codespace ssh -c octo-new1 -- tail -n +1 -F"),
        f.gh.count("TAIL")
    );
    // The last event is Ready, after a final step list.
    let Some(StartEvent::Ready {
        binding: ready,
        session: moved,
    }) = events.last()
    else {
        panic!("{events:?}");
    };
    assert_eq!(ready, &binding);
    assert_eq!(moved.id, session.id);
    assert_eq!(moved.cwd, binding.project_key);
    let Some(StartEvent::Updated(last)) = events.iter().rev().nth(1) else {
        panic!("{events:?}");
    };
    for kind in StepKind::ALL {
        assert_eq!(status(last, kind), StepStatus::Done, "{kind:?}");
    }
    assert_eq!(
        f.host.log.lock().unwrap()[0],
        "connect octo-new1 monoptah-main"
    );
    // The record is gone and the session now lives on the real project, with the binding.
    assert_eq!(f.record(session.id), None);
    let store = f.store.lock().unwrap();
    let stored = store.remote_session(session.id).unwrap().unwrap();
    assert_eq!(stored.project.environment_id, "env-octo-new1");
    assert_eq!(stored.project.codespace.as_deref(), Some("octo-new1"));
    assert_eq!(stored.summary.cwd, binding.project_key);
    assert_eq!(store.cloud_sessions("/p").unwrap().len(), 1);
}

#[test]
fn steps_that_never_appeared_in_the_log_are_skipped_at_ready() {
    let f = fx(Gh::new(&["Available"], &LOG[8..10]));
    let (session, _) = f.svc.begin_start("/p", new_target(), "claude").unwrap();
    let (result, events) = f.run(session.id);
    result.unwrap();
    let Some(StartEvent::Updated(last)) = events.iter().rev().nth(1) else {
        panic!("{events:?}");
    };
    assert_eq!(status(last, StepKind::BuildContainer), StepStatus::Skipped);
    assert_eq!(
        status(last, StepKind::BlockingCommands),
        StepStatus::Skipped
    );
    // Still running in the codespace: it goes on in the background.
    assert_eq!(
        status(last, StepKind::ConfigureCodespace),
        StepStatus::Running
    );
    assert_eq!(
        status(last, StepKind::LifecycleCommands),
        StepStatus::Pending
    );
    assert_eq!(status(last, StepKind::Connect), StepStatus::Done);
    assert_eq!(status(last, StepKind::OpenProject), StepStatus::Done);
}

#[test]
fn a_named_codespace_can_wake_but_a_stopped_tracked_start_fails() {
    let f = fx(Gh::new(&["Shutdown"], LOG));
    let (session, record) = f.svc.begin_start("/p", named_target(), "claude").unwrap();
    assert!(!record.track_ready);
    let (result, _) = f.run(session.id);
    let binding = result.unwrap();
    assert_eq!(binding.codespace, "octo-abc");
    // A named codespace is never polled; the log is replayed from the top at once.
    assert_eq!(f.gh.count("codespace view"), 0);
    assert!(f.gh.count("TAIL") >= 1);
    assert_eq!(f.host.log.lock().unwrap()[0], "connect octo-abc my cs");
    // An explicit stop while tracking must never be undone by the worker.
    let f = fx(Gh::new(&["Provisioning", "Shutdown"], &[]));
    let (session, _) = f.svc.begin_start("/p", new_target(), "claude").unwrap();
    assert!(f.run(session.id).0.unwrap_err().contains("Shutdown"));
    assert!(f.host.log.lock().unwrap().is_empty());
}

#[test]
fn a_connect_failure_is_persisted_and_retry_resumes_without_polling_again() {
    let f = fx(Gh::new(&["Available"], LOG));
    let (session, _) = f.svc.begin_start("/p", new_target(), "claude").unwrap();
    *f.host.connect_error.lock().unwrap() = Some("ssh refused".into());
    let (result, events) = f.run(session.id);
    let error = result.unwrap_err();
    assert!(
        error.starts_with("ssh refused (codespace octo-new1 was created and kept"),
        "{error}"
    );
    let Some(StartEvent::Failed(failed)) = events.last() else {
        panic!("{events:?}");
    };
    assert!(!events.iter().any(|e| matches!(e, StartEvent::Ready { .. })));
    assert_eq!(failed.error.as_deref(), Some(error.as_str()));
    assert_eq!(failed.failed_at, Some(StepKind::Connect));
    assert_eq!(status(failed, StepKind::Connect), StepStatus::Failed);
    assert!(failed.log_tail.iter().any(|l| l == "Creating container..."));
    // Persisted: this is what survives an app restart.
    assert_eq!(f.record(session.id), Some(failed.clone()));
    assert!(starting_refusal(&f.store.lock().unwrap(), session.id)
        .unwrap()
        .starts_with("This cloud session could not start: ssh refused"));
    let views = f.gh.count("codespace view");
    // Retry.
    *f.host.connect_error.lock().unwrap() = None;
    let (result, events) = f.run(session.id);
    result.unwrap();
    assert!(matches!(events.last(), Some(StartEvent::Ready { .. })));
    assert_eq!(
        f.gh.count("codespace view"),
        views,
        "Provision was already done"
    );
    assert_eq!(f.record(session.id), None);
}

#[test]
fn a_failed_codespace_fails_the_provision_step() {
    let f = fx(Gh::new(&["Provisioning", "Failed"], &[]));
    let (session, _) = f.svc.begin_start("/p", new_target(), "claude").unwrap();
    let (result, events) = f.run(session.id);
    assert!(result
        .unwrap_err()
        .contains("Codespace octo-new1 is Failed"));
    let Some(StartEvent::Failed(failed)) = events.last() else {
        panic!("{events:?}");
    };
    assert_eq!(failed.failed_at, Some(StepKind::Provision));
    assert_eq!(failed.gh_state.as_deref(), Some("Failed"));
    assert!(f.host.log.lock().unwrap().is_empty());
    assert_eq!(f.gh.count("TAIL"), 0, "no follower before Available");
}

#[test]
fn a_provisioning_timeout_is_reported() {
    let f = fx(Gh::new(&["Provisioning"], &[]));
    let (session, _) = f.svc.begin_start("/p", new_target(), "claude").unwrap();
    let (result, _) = f.run(session.id);
    let error = result.unwrap_err();
    assert!(
        error.contains("Timed out waiting for codespace octo-new1"),
        "{error}"
    );
}

#[test]
fn a_cancelled_or_deleted_start_stops_without_a_failure_event() {
    // Cancel flag.
    let f = fx(Gh::new(&["Provisioning"], &[]));
    let (session, _) = f.svc.begin_start("/p", new_target(), "claude").unwrap();
    let mut events = vec![];
    let error = f
        .svc
        .run_start(session.id, &mut |e| events.push(e), &AtomicBool::new(true))
        .unwrap_err();
    assert_eq!(error, START_ABORTED);
    assert!(!events
        .iter()
        .any(|e| matches!(e, StartEvent::Failed(_) | StartEvent::Ready { .. })));
    assert!(
        f.record(session.id).is_some(),
        "a cancel keeps the record for a later resume"
    );
    // The codespace is deleted while the start waits for it.
    let f = fx(Gh::new(&["Provisioning"], &[]));
    let (session, _) = f.svc.begin_start("/p", new_target(), "claude").unwrap();
    let store = f.store.clone();
    *f.gh.on_view.lock().unwrap() = Some(Box::new(move || {
        store.lock().unwrap().forget_codespace("octo-new1").unwrap();
    }));
    let (result, events) = f.run(session.id);
    assert_eq!(result.unwrap_err(), START_ABORTED);
    assert!(!events
        .iter()
        .any(|e| matches!(e, StartEvent::Failed(_) | StartEvent::Ready { .. })));
    // No record, nothing to run.
    assert!(f.run(session.id).0.unwrap_err().contains("not starting"));
}

#[test]
fn a_failing_log_follower_never_fails_the_start() {
    let f = fx(Gh::failing_follower(&["Available"]));
    let (session, _) = f.svc.begin_start("/p", new_target(), "claude").unwrap();
    let (result, events) = f.run(session.id);
    result.unwrap();
    assert!(matches!(events.last(), Some(StartEvent::Ready { .. })));
}

#[test]
fn a_late_log_still_gives_every_step_its_real_duration() {
    // Connecting is slower than the log: lines arrive while the host connects.
    let gh = Gh::new(&["Available"], LOG);
    let f = fx(gh);
    let (session, _) = f.svc.begin_start("/p", new_target(), "claude").unwrap();
    let (_, events) = f.run(session.id);
    let Some(StartEvent::Updated(last)) = events.iter().rev().nth(1) else {
        panic!("{events:?}");
    };
    let lifecycle = last.step(StepKind::LifecycleCommands);
    assert_eq!(
        lifecycle.finished_at.unwrap() - lifecycle.started_at.unwrap(),
        18_000
    );
}

#[test]
fn deleting_the_codespace_drops_its_start_records() {
    let gh = Gh::new(&["Provisioning"], &[]);
    let f = fx(gh);
    let (session, _) = f.svc.begin_start("/p", new_target(), "claude").unwrap();
    // gh delete is not scripted: forget directly, as delete_codespace_outcome does after gh.
    f.store
        .lock()
        .unwrap()
        .forget_codespace("octo-new1")
        .unwrap();
    assert_eq!(f.record(session.id), None);
    assert!(pending_starts(&f.store.lock().unwrap()).is_empty());
    assert!(f
        .store
        .lock()
        .unwrap()
        .remote_session(session.id)
        .unwrap()
        .is_none());
}

#[test]
fn rebuild_resets_the_steps_and_the_browser_url_is_known() {
    let f = fx(Gh::new(&["Available"], LOG));
    let (session, _) = f.svc.begin_start("/p", named_target(), "claude").unwrap();
    assert_eq!(
        f.svc.start_web_url(session.id).unwrap(),
        "https://asked.github.dev"
    );
    let (_, created) = f.svc.begin_start("/p", new_target(), "claude").unwrap();
    assert_eq!(
        f.svc.start_web_url(created.session_id).unwrap(),
        "https://octo-new1.github.dev"
    );
    let rebuilt = f.svc.rebuild_start(session.id).unwrap();
    assert!(f
        .gh
        .calls()
        .contains(&"codespace rebuild -c octo-abc".to_string()));
    assert!(rebuilt.track_ready);
    assert_eq!(status(&rebuilt, StepKind::Provision), StepStatus::Running);
    assert_eq!(status(&rebuilt, StepKind::Create), StepStatus::Skipped);
    assert_eq!(f.record(session.id), Some(rebuilt));
    assert!(f.svc.rebuild_start(-1).is_err());
    assert!(f.svc.start_web_url(-1).is_err());
}

// ------------------------------------------------------------ queue + refusal

#[test]
fn a_message_typed_while_starting_joins_the_send_queue_in_order() {
    let f = fx(Gh::new(&[], &[]));
    let (session, _) = f.svc.begin_start("/p", named_target(), "claude").unwrap();
    let store = f.store.lock().unwrap();
    assert!(queued_start_messages(&store, session.id).is_empty());
    queue_start_message(&store, session.id, "Fix the bug").unwrap();
    queue_start_message(&store, session.id, "Then add a test").unwrap();
    assert_eq!(
        queued_start_messages(&store, session.id),
        vec!["Fix the bug".to_owned(), "Then add a test".to_owned()]
    );
    let state = store.follow_ups(session.id).unwrap();
    assert!(state
        .messages
        .iter()
        .all(|m| m.id.starts_with("cloud-start-")));
    assert!(!state.messages.iter().any(|m| m.steer));
    // The app-launched draft is a separate mechanism.
    assert_eq!(queued_start_message(&store, session.id), None);
}

#[test]
fn queuing_a_start_message_never_touches_the_users_draft() {
    let f = fx(Gh::new(&[], &[]));
    let (session, _) = f.svc.begin_start("/p", named_target(), "claude").unwrap();
    let store = f.store.lock().unwrap();
    let mut state = store.follow_ups(session.id).unwrap();
    state.draft = Some(crate::follow_up::Message {
        id: "typed-by-user".into(),
        text: "mine".into(),
        ..Default::default()
    });
    store.save_follow_ups(session.id, &state).unwrap();
    queue_start_message(&store, session.id, "x").unwrap();
    assert_eq!(queued_start_messages(&store, session.id), vec!["x"]);
    assert_eq!(
        store.follow_ups(session.id).unwrap().draft.unwrap().text,
        "mine"
    );
}

#[test]
fn sends_to_a_starting_session_are_refused_with_the_phase() {
    let f = fx(Gh::new(&[], &[]));
    let (session, _) = f.svc.begin_start("/p", new_target(), "claude").unwrap();
    let store = f.store.lock().unwrap();
    assert_eq!(
        starting_refusal(&store, session.id).unwrap(),
        "This cloud session is still starting its codespace (Provisioning). Send again when it is ready."
    );
    assert_eq!(starting_refusal(&store, -1), None);
    // ensure_before_send recognises the provisional environment without a store.
    let stored = store.remote_session(session.id).unwrap().unwrap();
    drop(store);
    let error =
        ensure_before_send(&*f.host, &stored, &AtomicBool::new(false), &mut |_| {}).unwrap_err();
    assert_eq!(error, startup::STILL_STARTING);
}

#[test]
fn an_old_creation_log_of_a_named_codespace_never_moves_the_steps() {
    // The fake clock starts at 2026-11-07 04:01:40; the log is from before that by hours.
    let old: Vec<&'static str> = LOG
        .iter()
        .map(|l| l.replace("2026-11-07", "2026-11-05"))
        .map(|l| &*Box::leak(l.into_boxed_str()))
        .collect();
    let f = fx(Gh::new(&["Available"], &old));
    let (session, _) = f.svc.begin_start("/p", named_target(), "claude").unwrap();
    let (result, events) = f.run(session.id);
    result.unwrap();
    // Still forwarded to the panel.
    assert!(events
        .iter()
        .any(|e| matches!(e, StartEvent::Log(l) if l == "Creating container...")));
    let Some(StartEvent::Updated(last)) = events.iter().rev().nth(1) else {
        panic!("{events:?}");
    };
    for kind in [
        StepKind::BuildContainer,
        StepKind::BlockingCommands,
        StepKind::ConfigureCodespace,
    ] {
        assert_eq!(status(last, kind), StepStatus::Skipped, "{kind:?}");
    }
    assert_eq!(
        status(last, StepKind::LifecycleCommands),
        StepStatus::Pending
    );
}

#[test]
fn a_fresh_log_after_a_rebuild_updates_the_steps() {
    let f = fx(Gh::new(&["Available"], LOG));
    let (session, _) = f.svc.begin_start("/p", named_target(), "claude").unwrap();
    // The rebuild restarts the timeline at the (fake) clock; the log is a little older than
    // the margin allows only if it predates the rebuild by over a minute.
    let rebuilt = f.svc.rebuild_start(session.id).unwrap();
    assert!(rebuilt.started_at >= 1_794_024_100_000);
    let (result, events) = f.run(session.id);
    result.unwrap();
    let Some(StartEvent::Updated(last)) = events.iter().rev().nth(1) else {
        panic!("{events:?}");
    };
    assert_eq!(status(last, StepKind::BuildContainer), StepStatus::Done);
    assert_eq!(status(last, StepKind::LifecycleCommands), StepStatus::Done);
}

#[test]
fn resume_never_wakes_a_stopped_codespace() {
    let f = fx(Gh::new(&["Shutdown"], &[]));
    let (session, _) = f.svc.begin_start("/p", named_target(), "claude").unwrap();
    let mut events = vec![];
    assert!(f
        .svc
        .resume_start(session.id, &mut |e| events.push(e), &AtomicBool::new(false))
        .unwrap_err()
        .contains("Codespace stopped"));
    assert!(f.host.log.lock().unwrap().is_empty());
    assert!(matches!(events.last(), Some(StartEvent::Failed(_))));
}

#[test]
fn deleting_a_pending_session_removes_its_start_record() {
    let f = fx(Gh::new(&["Provisioning"], &[]));
    let (session, _) = f.svc.begin_start("/p", new_target(), "claude").unwrap();
    f.store
        .lock()
        .unwrap()
        .delete_remote_session(session.id)
        .unwrap();
    assert!(f.record(session.id).is_none());
    assert!(f.run(session.id).0.is_err());
    assert!(f.host.log.lock().unwrap().is_empty());
}

#[test]
fn archive_pauses_startup_and_a_late_publish_cannot_overwrite_it() {
    let f = fx(Gh::new(&["Provisioning"], &[]));
    let (session, original) = f.svc.begin_start("/p", new_target(), "claude").unwrap();
    f.store
        .lock()
        .unwrap()
        .set_archived(session.id, true)
        .unwrap();
    f.svc.persist_start(&original);
    assert!(f
        .record(session.id)
        .unwrap()
        .error
        .unwrap()
        .contains("archived"));
    let mut events = vec![];
    assert!(f
        .svc
        .resume_start(session.id, &mut |e| events.push(e), &AtomicBool::new(false))
        .is_err());
    assert!(f.host.log.lock().unwrap().is_empty());
}

#[test]
fn a_deleted_record_cannot_be_resurrected_by_a_late_publish() {
    let f = fx(Gh::new(&["Provisioning"], &[]));
    let (session, original) = f.svc.begin_start("/p", new_target(), "claude").unwrap();
    f.store
        .lock()
        .unwrap()
        .forget_codespace("octo-new1")
        .unwrap();
    f.svc.persist_start(&original);
    assert!(f.record(session.id).is_none());
}

/// A `cloud-starts.v1` record as written before sandboxes existed.
const OLD_START_RECORD: &str = r#"{"sessionId":4,"localProject":"/p","codespace":"cs-1","repo":"octo/widgets","branch":"main","machine":null,"display":"monoptah-main","remoteCwd":"/workspaces/widgets","webUrl":null,"created":true,"ghState":"Provisioning","steps":[],"error":null,"failedAt":null,"logTail":[],"startedAt":1000,"finishedAt":null,"trackReady":true}"#;

#[test]
fn an_old_start_record_reads_as_a_codespace_and_writes_back_unchanged() {
    let record: StartRecord = serde_json::from_str(OLD_START_RECORD).unwrap();
    assert_eq!(record.sandbox, None);
    assert_eq!(record.backend(), CloudBackendKind::Codespaces);
    assert_eq!(record.cloud(), Some(CloudRef::codespace("cs-1")));
    assert_eq!(serde_json::to_string(&record).unwrap(), OLD_START_RECORD);
    assert!(!serde_json::to_string(&fresh_record())
        .unwrap()
        .contains("sandbox"));
}

#[test]
fn a_sandbox_start_record_round_trips_and_reports_its_backend() {
    let mut record = fresh_record();
    record.codespace = String::new();
    record.sandbox = Some("sbx-1".into());
    let json = serde_json::to_string(&record).unwrap();
    let back: StartRecord = serde_json::from_str(&json).unwrap();
    assert_eq!(back, record);
    assert_eq!(back.backend(), CloudBackendKind::DockerSandbox);
    assert_eq!(back.cloud(), Some(CloudRef::sandbox("sbx-1")));
}

#[test]
fn a_start_record_without_a_codespace_names_no_workspace() {
    let mut value: serde_json::Value = serde_json::from_str(OLD_START_RECORD).unwrap();
    value["codespace"] = "".into();
    let record: StartRecord = serde_json::from_value(value).unwrap();
    assert_eq!(record.cloud(), None);
}
