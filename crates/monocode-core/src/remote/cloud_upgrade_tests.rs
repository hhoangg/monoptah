//! The real [`Connections`] layer of the cloud Host upgrade, against a fake Host (a local
//! HTTP server that answers the RPCs) and a fake ssh runner (records the scripts it is given
//! and plays back what a machine would print).
use super::cloud_upgrade::{CloudRunner, HOST_UPDATING};
use super::host_upgrade::{Outcome, SandboxAuth};
use super::ssh::{Job, SshTarget, Tunnel};
use super::ssh_askpass::Askpass;
use super::*;
use std::io::{BufRead, BufReader};
use std::sync::atomic::AtomicUsize;

const MACHINE: &str = "m";
const ENV: &str = "env-cs";

/// What the fake Host knows.
struct HostState {
    flavor: &'static str,
    version: &'static str,
    /// `status` of each session, as `sessions.list` reports it.
    sessions: Vec<&'static str>,
    /// `sessions.list` fails (the Host cannot be asked).
    sessions_fail: bool,
    /// A `commands.dispatch` starts a turn right away and answers only after this many ms.
    dispatch_delay_ms: u64,
}

struct FakeHost {
    port: u16,
    state: Arc<Mutex<HostState>>,
    calls: Arc<Mutex<Vec<String>>>,
    stop: Arc<AtomicBool>,
}
impl Drop for FakeHost {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}
impl FakeHost {
    fn start(flavor: &'static str, version: &'static str, sessions: Vec<&'static str>) -> Self {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let port = listener.local_addr().unwrap().port();
        let state = Arc::new(Mutex::new(HostState {
            flavor,
            version,
            sessions,
            sessions_fail: false,
            dispatch_delay_ms: 0,
        }));
        let calls = Arc::new(Mutex::new(vec![]));
        let stop = Arc::new(AtomicBool::new(false));
        let (state2, calls2, stop2) = (state.clone(), calls.clone(), stop.clone());
        std::thread::spawn(move || {
            while !stop2.load(Ordering::Relaxed) {
                let Ok((mut stream, _)) = listener.accept() else {
                    std::thread::sleep(std::time::Duration::from_millis(5));
                    continue;
                };
                stream.set_nonblocking(false).unwrap();
                stream
                    .set_read_timeout(Some(std::time::Duration::from_secs(3)))
                    .unwrap();
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut length = 0;
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                        break;
                    }
                    if let Some((key, value)) = line.split_once(':') {
                        if key.eq_ignore_ascii_case("content-length") {
                            length = value.trim().parse().unwrap();
                        }
                    }
                }
                let mut body = vec![0; length];
                if reader.read_exact(&mut body).is_err() {
                    continue;
                }
                let payload: Value = serde_json::from_slice(&body).unwrap();
                let method = payload["method"].as_str().unwrap_or("").to_owned();
                calls2.lock().unwrap().push(method.clone());
                let mut state = state2.lock().unwrap();
                let mut delay = 0;
                if method == "commands.dispatch" && state.dispatch_delay_ms > 0 {
                    // The turn starts on the Host at once; the answer comes later.
                    state.sessions = vec!["running"];
                    delay = state.dispatch_delay_ms;
                }
                let reply = match method.as_str() {
                    "environment.describe" => json!({"result": {
                        "protocolVersion": 1, "environmentId": ENV, "name": "Host",
                        "host": {"flavor": state.flavor, "version": state.version},
                    }}),
                    "projects.list" => json!({"result": [{"id": "p1"}]}),
                    "sessions.list" if state.sessions_fail => json!({"error": "Host is busy"}),
                    "sessions.list" => json!({"result": state.sessions.iter()
                        .map(|status| json!({"status": status, "needsInput": false}))
                        .collect::<Vec<_>>()}),
                    _ => json!({"result": {}}),
                };
                drop(state);
                std::thread::sleep(std::time::Duration::from_millis(delay));
                let body = reply.to_string();
                let _ = write!(stream, "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n{}", body.len(), body);
            }
        });
        Self {
            port,
            state,
            calls,
            stop,
        }
    }
    fn set(&self, change: impl FnOnce(&mut HostState)) {
        change(&mut self.state.lock().unwrap());
    }
    fn calls_of(&self, method: &str) -> usize {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .filter(|m| *m == method)
            .count()
    }
}

type OnScript = Box<dyn Fn(&str, &str) -> Result<String, String> + Send + Sync>;

/// The fake ssh runner: records each script's phase and body, answers through `on_script`
/// (phase word, script), and hands out tunnels that reach the fake Host.
struct FakeRunner {
    host_port: u16,
    scripts: Mutex<Vec<(String, String)>>,
    tunnels: AtomicUsize,
    on_script: Mutex<OnScript>,
    /// A `stage` runs until its job is cancelled (a download that is still going).
    block_stage: AtomicBool,
}
impl FakeRunner {
    fn phases(&self) -> Vec<String> {
        self.scripts
            .lock()
            .unwrap()
            .iter()
            .map(|(phase, _)| phase.clone())
            .collect()
    }
}
impl CloudRunner for FakeRunner {
    fn run_script(
        &self,
        _: &SshTarget,
        script: String,
        job: &Arc<Job>,
        _: &Askpass,
    ) -> Result<String, String> {
        // Like the real ssh runner: a cancelled job runs nothing.
        if job.cancelled.load(Ordering::Relaxed) {
            return Err("Connection cancelled".into());
        }
        let phase = ["stage", "swapnow", "discard", "status"]
            .into_iter()
            .find(|p| script.contains(&format!("MONOCODE_HOST_UPGRADE_PHASE={p}\n")))
            .expect("an upgrade script")
            .to_owned();
        self.scripts
            .lock()
            .unwrap()
            .push((phase.clone(), script.clone()));
        if phase == "stage" && self.block_stage.load(Ordering::Relaxed) {
            for _ in 0..500 {
                if job.cancelled.load(Ordering::Relaxed) {
                    return Err("Connection cancelled".into());
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        }
        (self.on_script.lock().unwrap())(&phase, &script)
    }
    fn poll_interval(&self) -> std::time::Duration {
        std::time::Duration::from_millis(10)
    }
    fn start_tunnel(&self, _: &SshTarget, _: &Arc<Job>, _: &Askpass) -> Result<Tunnel, String> {
        self.tunnels.fetch_add(1, Ordering::SeqCst);
        Ok(Tunnel::for_test(self.host_port))
    }
}

struct Fx {
    _dir: tempfile::TempDir,
    connections: Arc<Connections>,
    host: FakeHost,
    runner: Arc<FakeRunner>,
}

fn target(sandbox: bool) -> SshTarget {
    if sandbox {
        SshTarget {
            target: "sbx_1@sbx_cloud".into(),
            port: None,
            remote_port: 3774,
            codespace: None,
            sandbox: Some(SandboxTarget {
                id: "sbx_1".into(),
                name: "box".into(),
                workspace: "/work/repo".into(),
                relay_port: 3900,
                config: None,
                mode: SandboxMode::Container,
            }),
        }
    } else {
        SshTarget {
            target: "cs".into(),
            port: None,
            remote_port: 3774,
            codespace: Some("cs".into()),
            sandbox: None,
        }
    }
}

fn setup(
    sandbox: bool,
    flavor: &'static str,
    version: &'static str,
    sessions: Vec<&'static str>,
    on_script: OnScript,
) -> Fx {
    let dir = tempfile::tempdir().unwrap();
    let host = FakeHost::start(flavor, version, sessions);
    let connections = Arc::new(Connections::new(dir.path()));
    write(
        &connections.store_path(),
        &[StoredMachine {
            id: MACHINE.into(),
            name: "cloud".into(),
            endpoint: "codespace://cs".into(),
            environment_id: ENV.into(),
            token: "t".repeat(43),
            ssh: Some(target(sandbox)),
            pending_host_upgrade: false,
            swap_unconfirmed: false,
        }],
    )
    .unwrap();
    connections
        .tunnels
        .insert(MACHINE.into(), Tunnel::for_test(host.port));
    let runner = Arc::new(FakeRunner {
        host_port: host.port,
        scripts: Mutex::new(vec![]),
        tunnels: AtomicUsize::new(0),
        on_script: Mutex::new(on_script),
        block_stage: AtomicBool::new(false),
    });
    connections.set_runner(runner.clone());
    Fx {
        _dir: dir,
        connections,
        host,
        runner,
    }
}

fn upgrade(fx: &Fx, auth: Option<SandboxAuth<'_>>) -> (Outcome, Vec<String>) {
    upgrade_with(fx, auth, &AtomicBool::new(false))
}
fn upgrade_with(
    fx: &Fx,
    auth: Option<SandboxAuth<'_>>,
    cancel: &AtomicBool,
) -> (Outcome, Vec<String>) {
    let mut lines = vec![];
    let outcome = fx
        .connections
        .upgrade_host_if_needed(MACHINE, auth, cancel, &mut |line| {
            lines.push(line.to_owned())
        });
    (outcome, lines)
}

/// A machine that stages, then installs: the swap makes the fake Host the pinned one.
fn happy(host_state: &Arc<Mutex<HostState>>) -> OnScript {
    let state = host_state.clone();
    Box::new(move |phase, _| match phase {
        "stage" => Ok("{\"staged\":true}".into()),
        // The launcher starts the worker; the machine swaps the Host on its own.
        "swapnow" => {
            let mut state = state.lock().unwrap();
            state.flavor = "monoptah";
            state.version = "1.1.0";
            Ok("{\"swap\":\"started\"}".into())
        }
        "status" => Ok("{\"swap\":\"done\",\"port\":3774}".into()),
        _ => Ok("{\"staged\":false}".into()),
    })
}

/// `setup` for a script that needs the Host state: the closure is wired after the Host exists.
fn setup_happy(sessions: Vec<&'static str>) -> Fx {
    let fx = setup(
        false,
        "upstream",
        "",
        sessions,
        Box::new(|_, _| Ok(String::new())),
    );
    *fx.runner.on_script.lock().unwrap() = happy(&fx.host.state);
    fx
}

#[test]
fn an_older_idle_host_is_staged_swapped_and_the_tunnel_reopened_with_sends_held_only_during_the_swap(
) {
    let fx = setup_happy(vec!["idle", "interrupted"]);
    let connections = fx.connections.clone();
    let seen = Arc::new(Mutex::new(vec![]));
    {
        let (state, seen) = (fx.host.state.clone(), seen.clone());
        *fx.runner.on_script.lock().unwrap() = Box::new(move |phase, _| {
            if phase == "swapnow" {
                // A send during the swap is refused, retryably; reads still work.
                seen.lock().unwrap().push((
                    connections.request(MACHINE, "commands.dispatch", json!({})),
                    connections.request(MACHINE, "sessions.list", json!({"projectId": "p1"})),
                ));
                let mut state = state.lock().unwrap();
                state.flavor = "monoptah";
                state.version = "1.1.0";
                return Ok("{\"swap\":\"started\"}".into());
            }
            if phase == "status" {
                return Ok("{\"swap\":\"done\",\"port\":3774}".into());
            }
            Ok("{\"staged\":true}".into())
        });
    }
    let first_tunnel = fx.connections.tunnels.existing(MACHINE).unwrap().tunnel_id;
    let (outcome, lines) = upgrade(&fx, None);
    assert!(matches!(outcome, Outcome::Upgraded(_)), "{outcome:?}");
    assert_eq!(fx.runner.phases(), ["stage", "swapnow", "status"]);
    assert_eq!(lines, ["Updating Monoptah Host…"]);
    let seen = seen.lock().unwrap();
    assert_eq!(seen[0].0.as_ref().unwrap_err(), HOST_UPDATING);
    assert!(seen[0].1.is_ok());
    // Reconnected: a new tunnel replaced the old one, and the flag is clear.
    assert_eq!(fx.runner.tunnels.load(Ordering::SeqCst), 1);
    let second_tunnel = fx.connections.tunnels.existing(MACHINE).unwrap().tunnel_id;
    assert_ne!(first_tunnel, second_tunnel);
    assert!(!fx.connections.host_upgrade_pending(MACHINE));
    // The hold is released: a send goes through again.
    assert!(fx
        .connections
        .request(MACHINE, "commands.dispatch", json!({}))
        .is_ok());
}

#[test]
fn a_turn_that_starts_between_the_download_and_the_swap_defers_and_sets_the_flag() {
    let fx = setup_happy(vec!["idle"]);
    {
        let (state, inner) = (fx.host.state.clone(), happy(&fx.host.state));
        *fx.runner.on_script.lock().unwrap() = Box::new(move |phase, script| {
            if phase == "stage" {
                // The user sends from another window while the package downloads.
                state.lock().unwrap().sessions = vec!["running"];
            }
            inner(phase, script)
        });
    }
    let (outcome, lines) = upgrade(&fx, None);
    assert_eq!(outcome, Outcome::Deferred);
    // The swap never ran, the staged package was dropped, nothing was restarted.
    assert_eq!(fx.runner.phases(), ["stage", "discard"]);
    assert_eq!(fx.runner.tunnels.load(Ordering::SeqCst), 0);
    assert!(fx.connections.host_upgrade_pending(MACHINE));
    assert_eq!(
        lines.last().unwrap(),
        "Monoptah Host 1.1.0 will be installed the next time this workspace connects"
    );
    // Sends are not left blocked.
    assert!(fx
        .connections
        .request(MACHINE, "commands.dispatch", json!({}))
        .is_ok());
    // The next connect (a new tunnel) finds it idle: upgrades and clears the flag.
    fx.host.set(|state| state.sessions = vec!["idle"]);
    *fx.runner.on_script.lock().unwrap() = happy(&fx.host.state);
    fx.connections
        .tunnels
        .insert(MACHINE.into(), Tunnel::for_test(fx.host.port));
    let (outcome, _) = upgrade(&fx, None);
    assert!(matches!(outcome, Outcome::Upgraded(_)), "{outcome:?}");
    assert!(!fx.connections.host_upgrade_pending(MACHINE));
}

#[test]
fn an_older_host_with_a_running_turn_is_deferred_before_any_script_runs() {
    let fx = setup_happy(vec!["running"]);
    let (outcome, lines) = upgrade(&fx, None);
    assert_eq!(outcome, Outcome::Deferred);
    assert!(fx.runner.phases().is_empty());
    assert!(fx.connections.host_upgrade_pending(MACHINE));
    assert_eq!(lines.len(), 1);
}

#[test]
fn a_host_whose_sessions_cannot_be_listed_is_deferred_not_touched() {
    let fx = setup_happy(vec!["idle"]);
    fx.host.set(|state| state.sessions_fail = true);
    let (outcome, _) = upgrade(&fx, None);
    assert_eq!(outcome, Outcome::Deferred);
    assert!(fx.runner.phases().is_empty());
    assert!(fx.connections.host_upgrade_pending(MACHINE));
    assert!(fx.host.calls_of("projects.list") >= 1);
}

#[test]
fn a_current_fork_host_runs_nothing() {
    let fx = setup(
        false,
        "monoptah",
        "1.2.0",
        vec![],
        Box::new(|_, _| unreachable!()),
    );
    let (outcome, lines) = upgrade(&fx, None);
    assert_eq!(outcome, Outcome::Current);
    assert!(lines.is_empty());
    assert!(fx.runner.phases().is_empty());
}

#[test]
fn a_new_host_that_fails_to_start_is_rolled_back_and_the_connect_goes_on() {
    let fx = setup(
        false,
        "upstream",
        "",
        vec![],
        Box::new(|_, _| Ok(String::new())),
    );
    *fx.runner.on_script.lock().unwrap() = Box::new(|phase, _| match phase {
        "stage" => Ok("{\"staged\":true}".into()),
        // The worker started, and the machine says it put the old Host back.
        "swapnow" => Ok("{\"swap\":\"started\"}".into()),
        _ => Ok("{\"swap\":\"restored\"}".into()),
    });
    let (outcome, lines) = upgrade(&fx, None);
    let Outcome::Failed(warning) = outcome else {
        panic!("expected a warning, got {outcome:?}");
    };
    assert!(warning.contains("the current Host is kept"), "{warning}");
    assert_eq!(lines.last(), Some(&warning));
    // The tunnel was reopened to the old Host, which answers.
    assert_eq!(fx.runner.tunnels.load(Ordering::SeqCst), 1);
    assert!(fx
        .connections
        .request(MACHINE, "environment.describe", json!({}))
        .is_ok());
}

#[test]
fn a_failed_download_in_a_sandbox_keeps_the_old_host_and_shows_the_policy_hint() {
    let fx = setup(
        true,
        "upstream",
        "",
        vec![],
        Box::new(|_, _| Ok(String::new())),
    );
    *fx.runner.on_script.lock().unwrap() = Box::new(|_, _| {
        Err("SSH setup failed: curl: (6) Could not resolve host: github.com".into())
    });
    let (outcome, lines) = upgrade(&fx, None);
    let Outcome::Failed(warning) = outcome else {
        panic!("expected a warning");
    };
    assert!(
        warning.contains("sbx policy allow network github.com"),
        "{warning}"
    );
    assert_eq!(fx.runner.phases(), ["stage"]);
    assert_eq!(
        fx.runner.tunnels.load(Ordering::SeqCst),
        0,
        "nothing was restarted"
    );
    assert_eq!(lines.last(), Some(&warning));
}

#[test]
fn a_sandbox_upgrade_uses_the_connect_prelude_and_no_secret_reaches_what_is_shown() {
    let secret = "plainsecretvalue99";
    let token = "sk-ant-oat01-ABCDEFGHIJKLMN";
    let prelude = format!(
        "unset ANTHROPIC_AUTH_TOKEN ANTHROPIC_API_KEY CLAUDE_CODE_OAUTH_TOKEN\nexport CLAUDE_CODE_OAUTH_TOKEN='{secret}'\n"
    );
    let fx = setup(
        true,
        "upstream",
        "",
        vec![],
        Box::new(|_, _| Ok(String::new())),
    );
    let (leak, leak_prelude) = (secret.to_owned(), prelude.clone());
    let device = "t".repeat(43);
    // The script fails and its stderr echoes the credential, the whole prelude and a token.
    *fx.runner.on_script.lock().unwrap() = Box::new(move |_, _| {
        Err(format!(
            "SSH setup failed: line 3: export CLAUDE_CODE_OAUTH_TOKEN='{leak}' denied; {leak_prelude}; Authorization: Bearer abc; {token}; device {device}"
        ))
    });
    let auth = SandboxAuth {
        prelude: &prelude,
        secret,
    };
    let (outcome, lines) = upgrade(&fx, Some(auth));
    // The script that ran starts with exactly the prelude the connect step uses.
    let scripts = fx.runner.scripts.lock().unwrap().clone();
    assert!(scripts[0].1.starts_with(&prelude), "{}", scripts[0].1);
    assert!(scripts[0].1.contains("ensure_detached_host"));
    // Nothing shown or returned carries the secret, the prelude or the token.
    let Outcome::Failed(warning) = outcome else {
        panic!("expected a warning");
    };
    for text in lines.iter().chain([&warning]) {
        assert!(!text.contains(secret), "{text}");
        assert!(!text.contains("sk-ant-oat01-ABCDEFGHIJKLMN"), "{text}");
        assert!(!text.contains("unset ANTHROPIC_AUTH_TOKEN"), "{text}");
        assert!(!text.contains("Bearer abc"), "{text}");
        assert!(!text.contains(&"t".repeat(43)), "{text}");
    }
    // The stored machine record holds none of it either.
    let stored = std::fs::read_to_string(fx.connections.store_path()).unwrap();
    assert!(!stored.contains(secret));
}

#[test]
fn one_connect_upgrades_once_and_a_new_tunnel_is_a_new_connect() {
    let fx = setup_happy(vec!["running"]);
    let (first, lines) = upgrade(&fx, None);
    assert_eq!(first, Outcome::Deferred);
    assert_eq!(lines.len(), 1);
    // The same connect asks again (the wake, then the start): silent no-op.
    let (second, lines) = upgrade(&fx, None);
    assert_eq!(second, Outcome::Current);
    assert!(lines.is_empty(), "{lines:?}");
    let listed = fx.host.calls_of("sessions.list");
    upgrade(&fx, None);
    assert_eq!(
        fx.host.calls_of("sessions.list"),
        listed,
        "nothing was asked again"
    );
    // A new tunnel is a new connect.
    fx.connections
        .tunnels
        .insert(MACHINE.into(), Tunnel::for_test(fx.host.port));
    let (third, lines) = upgrade(&fx, None);
    assert_eq!(third, Outcome::Deferred);
    assert_eq!(lines.len(), 1);
}

#[test]
fn a_cancelled_start_runs_nothing_and_drops_what_it_staged() {
    // Cancelled before it started.
    let fx = setup_happy(vec!["idle"]);
    let (outcome, _) = upgrade_with(&fx, None, &AtomicBool::new(true));
    assert_eq!(outcome, Outcome::Cancelled);
    assert!(fx.runner.phases().is_empty());
    // Cancelled while the package downloads: the swap does not run.
    let fx = setup_happy(vec!["idle"]);
    let cancel = Arc::new(AtomicBool::new(false));
    {
        let (cancel, inner) = (cancel.clone(), happy(&fx.host.state));
        *fx.runner.on_script.lock().unwrap() = Box::new(move |phase, script| {
            if phase == "stage" {
                cancel.store(true, Ordering::Relaxed);
            }
            inner(phase, script)
        });
    }
    let (outcome, _) = upgrade_with(&fx, None, &cancel);
    assert_eq!(outcome, Outcome::Cancelled);
    assert_eq!(fx.runner.phases(), ["stage", "discard"]);
    // A cancelled pass is not recorded as done: the next connect asks again.
    let (outcome, _) = upgrade(&fx, None);
    assert!(matches!(outcome, Outcome::Upgraded(_)), "{outcome:?}");
    assert_eq!(
        fx.runner.phases(),
        ["stage", "discard", "stage", "swapnow", "status"]
    );
}

#[test]
fn the_upgrade_holds_the_job_slot_so_a_wake_cannot_bootstrap_beside_it() {
    let fx = setup_happy(vec!["idle"]);
    let busy = Arc::new(Mutex::new(vec![]));
    {
        let (connections, busy, inner) =
            (fx.connections.clone(), busy.clone(), happy(&fx.host.state));
        *fx.runner.on_script.lock().unwrap() = Box::new(move |phase, script| {
            // A wake of the same codespace tries to start its phase-1 bootstrap now.
            busy.lock()
                .unwrap()
                .push(connections.codespace_reconnect(MACHINE));
            inner(phase, script)
        });
    }
    let (outcome, _) = upgrade(&fx, None);
    assert!(matches!(outcome, Outcome::Upgraded(_)), "{outcome:?}");
    let busy = busy.lock().unwrap();
    assert!(!busy.is_empty());
    for attempt in busy.iter() {
        assert_eq!(attempt.as_ref().unwrap_err(), SSH_BUSY);
    }
    // The slot is free again afterwards.
    assert!(fx
        .connections
        .jobs
        .lock()
        .unwrap()
        .values()
        .all(|job| job.view().done));
}

#[test]
fn an_upgrade_waits_for_a_running_job_and_gives_up_when_cancelled() {
    let fx = setup_happy(vec!["idle"]);
    // Another job (a wake of some machine) holds the slot.
    let job = Job::new();
    fx.connections
        .jobs
        .lock()
        .unwrap()
        .insert(job.view().id, job.clone());
    let cancel = Arc::new(AtomicBool::new(false));
    let flip = cancel.clone();
    let canceller = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(300));
        flip.store(true, Ordering::Relaxed);
    });
    let (outcome, _) = upgrade_with(&fx, None, &cancel);
    canceller.join().unwrap();
    assert_eq!(outcome, Outcome::Cancelled);
    assert!(
        fx.runner.phases().is_empty(),
        "nothing ran beside the other job"
    );
}

#[test]
fn a_dispatch_already_on_its_way_is_waited_for_and_then_seen_by_the_busy_check() {
    let fx = setup_happy(vec!["idle"]);
    fx.host.set(|state| state.dispatch_delay_ms = 400);
    let send = Arc::new(Mutex::new(None));
    {
        let (connections, host_calls, send, inner) = (
            fx.connections.clone(),
            fx.host.calls.clone(),
            send.clone(),
            happy(&fx.host.state),
        );
        *fx.runner.on_script.lock().unwrap() = Box::new(move |phase, script| {
            if phase == "stage" {
                // The user sends from this app while the package downloads.
                let connections = connections.clone();
                *send.lock().unwrap() = Some(std::thread::spawn(move || {
                    connections.request(MACHINE, "commands.dispatch", json!({}))
                }));
                // The call is on its way to the Host before the download ends.
                while !host_calls
                    .lock()
                    .unwrap()
                    .iter()
                    .any(|m| m == "commands.dispatch")
                {
                    std::thread::sleep(std::time::Duration::from_millis(5));
                }
            }
            inner(phase, script)
        });
    }
    let started = std::time::Instant::now();
    let (outcome, _) = upgrade(&fx, None);
    // The hold waited for the dispatch, the busy check then saw the turn, nothing was swapped.
    assert!(started.elapsed() >= std::time::Duration::from_millis(300));
    assert_eq!(outcome, Outcome::Deferred);
    assert_eq!(fx.runner.phases(), ["stage", "discard"]);
    assert!(fx.connections.host_upgrade_pending(MACHINE));
    assert_eq!(fx.runner.tunnels.load(Ordering::SeqCst), 0);
    // The user's send was not refused and not cut off.
    let result = send.lock().unwrap().take().unwrap().join().unwrap();
    assert!(result.is_ok(), "{result:?}");
}

#[test]
fn requests_that_do_not_finish_in_time_defer_instead_of_swapping_under_them() {
    let fx = setup_happy(vec!["idle"]);
    fx.host.set(|state| state.dispatch_delay_ms = 1500);
    *fx.connections.drain_override.lock().unwrap() = Some(std::time::Duration::from_millis(100));
    let send = Arc::new(Mutex::new(None));
    {
        let (connections, host_calls, send, inner) = (
            fx.connections.clone(),
            fx.host.calls.clone(),
            send.clone(),
            happy(&fx.host.state),
        );
        *fx.runner.on_script.lock().unwrap() = Box::new(move |phase, script| {
            if phase == "stage" {
                let connections = connections.clone();
                *send.lock().unwrap() = Some(std::thread::spawn(move || {
                    connections.request(MACHINE, "commands.dispatch", json!({}))
                }));
                while !host_calls
                    .lock()
                    .unwrap()
                    .iter()
                    .any(|m| m == "commands.dispatch")
                {
                    std::thread::sleep(std::time::Duration::from_millis(5));
                }
            }
            inner(phase, script)
        });
    }
    let (outcome, _) = upgrade(&fx, None);
    assert_eq!(outcome, Outcome::Deferred);
    assert_eq!(fx.runner.phases(), ["stage", "discard"]);
    // The hold was undone: sends are not left blocked once the slow one is done.
    let result = send.lock().unwrap().take().unwrap().join().unwrap();
    assert!(result.is_ok());
    fx.host.set(|state| state.dispatch_delay_ms = 0);
    assert!(fx
        .connections
        .request(MACHINE, "commands.dispatch", json!({}))
        .is_ok());
}

#[test]
fn only_requests_that_start_a_turn_are_held_back() {
    let fx = setup_happy(vec!["idle"]);
    let gate = fx.connections.gate(MACHINE);
    gate.hold_for_test();
    assert_eq!(
        fx.connections
            .request(MACHINE, "workspace.run", json!({}))
            .unwrap_err(),
        HOST_UPDATING
    );
    assert_eq!(
        fx.connections
            .request(MACHINE, "commands.dispatch", json!({}))
            .unwrap_err(),
        HOST_UPDATING
    );
    for method in [
        "sessions.sync",
        "sessions.syncChunk",
        "sessions.list",
        "files.read",
    ] {
        assert!(
            fx.connections.request(MACHINE, method, json!({})).is_ok(),
            "{method}"
        );
    }
}

#[test]
fn a_cancel_during_the_download_is_a_cancel_and_leaves_no_staged_package() {
    let fx = setup_happy(vec!["idle"]);
    fx.runner.block_stage.store(true, Ordering::Relaxed);
    let cancel = Arc::new(AtomicBool::new(false));
    let flip = cancel.clone();
    let canceller = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(150));
        flip.store(true, Ordering::Relaxed);
    });
    let (outcome, lines) = upgrade_with(&fx, None, &cancel);
    canceller.join().unwrap();
    assert_eq!(outcome, Outcome::Cancelled);
    // No warning for a cancel; the discard ran on a job that is not cancelled.
    assert_eq!(lines, ["Updating Monoptah Host…"]);
    assert_eq!(fx.runner.phases(), ["stage", "discard"]);
    // The pass is not recorded for this tunnel: the next ask runs again.
    fx.runner.block_stage.store(false, Ordering::Relaxed);
    let (outcome, _) = upgrade(&fx, None);
    assert!(matches!(outcome, Outcome::Upgraded(_)), "{outcome:?}");
}

#[test]
fn waiting_for_the_job_slot_is_said_and_a_slot_that_stays_busy_defers_with_a_warning() {
    let fx = setup_happy(vec!["idle"]);
    *fx.connections.slot_wait_override.lock().unwrap() =
        Some(std::time::Duration::from_millis(300));
    let job = Job::new();
    fx.connections
        .jobs
        .lock()
        .unwrap()
        .insert(job.view().id, job.clone());
    let (outcome, lines) = upgrade(&fx, None);
    assert_eq!(outcome, Outcome::Deferred);
    assert_eq!(lines.len(), 2, "{lines:?}");
    assert_eq!(lines[0], "Waiting for another operation on this workspace…");
    assert!(lines[1].contains("another operation on this workspace is still running"));
    assert!(fx.runner.phases().is_empty());
    assert!(fx.connections.host_upgrade_pending(MACHINE));
}

#[test]
fn a_swap_the_app_never_saw_end_is_settled_at_the_next_connect() {
    let fx = setup_happy(vec!["idle"]);
    fx.connections
        .update_machine(MACHINE, |m| {
            m.swap_unconfirmed = true;
            true
        })
        .unwrap();
    let statuses = Arc::new(AtomicUsize::new(0));
    {
        let (statuses, inner) = (statuses.clone(), happy(&fx.host.state));
        *fx.runner.on_script.lock().unwrap() = Box::new(move |phase, script| {
            if phase == "status" && statuses.fetch_add(1, Ordering::SeqCst) == 0 {
                // The machine finished the old swap by putting the old Host back.
                return Ok("{\"swap\":\"restored\"}".into());
            }
            inner(phase, script)
        });
    }
    let (outcome, _) = upgrade(&fx, None);
    assert!(matches!(outcome, Outcome::Upgraded(_)), "{outcome:?}");
    // It asked first, reopened the tunnel to whatever runs now, and then upgraded normally.
    assert_eq!(fx.runner.phases(), ["status", "stage", "swapnow", "status"]);
    assert_eq!(fx.runner.tunnels.load(Ordering::SeqCst), 2);
    assert!(
        !fx.connections
            .stored_machine(MACHINE)
            .unwrap()
            .unwrap()
            .swap_unconfirmed
    );
}

#[test]
fn a_swap_is_marked_unconfirmed_while_it_runs_and_cleared_when_it_ends() {
    let fx = setup_happy(vec!["idle"]);
    let seen = Arc::new(Mutex::new(vec![]));
    {
        let (connections, seen, inner) =
            (fx.connections.clone(), seen.clone(), happy(&fx.host.state));
        *fx.runner.on_script.lock().unwrap() = Box::new(move |phase, script| {
            if phase == "swapnow" {
                let machine = connections.stored_machine(MACHINE).unwrap().unwrap();
                seen.lock().unwrap().push(machine.swap_unconfirmed);
                // Past the point of no return an app quit must not cancel it.
                seen.lock()
                    .unwrap()
                    .push(!connections.swap_jobs.lock().unwrap().is_empty());
            }
            inner(phase, script)
        });
    }
    let (outcome, _) = upgrade(&fx, None);
    assert!(matches!(outcome, Outcome::Upgraded(_)));
    assert_eq!(*seen.lock().unwrap(), [true, true]);
    assert!(
        !fx.connections
            .stored_machine(MACHINE)
            .unwrap()
            .unwrap()
            .swap_unconfirmed
    );
    assert!(fx.connections.swap_jobs.lock().unwrap().is_empty());
    // The launcher carries the device credential for the worker's last look, on stdin only.
    let scripts = fx.runner.scripts.lock().unwrap().clone();
    let launcher = scripts
        .iter()
        .find(|(phase, _)| phase == "swapnow")
        .unwrap();
    assert!(launcher
        .1
        .contains(&format!("export MONOCODE_CHECK_TOKEN='{}'", "t".repeat(43))));
}

#[test]
fn an_app_quit_leaves_a_swap_past_the_point_of_no_return_alone_and_waits_for_it() {
    let dir = tempfile::tempdir().unwrap();
    let connections = Connections::new(dir.path());
    let (swap, other) = (Job::new(), Job::new());
    {
        let mut jobs = connections.jobs.lock().unwrap();
        jobs.insert(swap.view().id, swap.clone());
        jobs.insert(other.view().id, other.clone());
    }
    connections.swap_jobs.lock().unwrap().insert(swap.view().id);
    let finish = swap.clone();
    let finisher = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(300));
        finish.complete(|| Err("done".into()));
    });
    let started = std::time::Instant::now();
    connections.shutdown();
    finisher.join().unwrap();
    assert!(
        !swap.cancelled.load(Ordering::Relaxed),
        "the swap is not cancelled"
    );
    assert!(other.cancelled.load(Ordering::Relaxed), "any other job is");
    assert!(
        started.elapsed() >= std::time::Duration::from_millis(250),
        "it waited for the swap"
    );
}
