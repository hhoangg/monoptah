use super::tests_support::FakeBackend;
use super::*;
use std::io::{Read, Write};

fn spec(profile: &str) -> ServerSpec {
    ServerSpec {
        command: "npx".into(),
        args: vec!["--profile-dir-name".into(), profile.into()],
    }
}
fn launch<'a>(spec: &'a ServerSpec, token: &'a Secret) -> ServerLaunch<'a> {
    ServerLaunch {
        spec,
        token,
        search_path: "/usr/bin",
    }
}
fn token(value: &str) -> Secret {
    Secret::new(format!("fake-token-{value}-0123456789"))
}
fn manager(idle: Duration) -> (Arc<FakeBackend>, BrowserServers) {
    let backend = Arc::new(FakeBackend::default());
    (backend.clone(), BrowserServers::new(backend, idle))
}
fn starts(backend: &FakeBackend) -> usize {
    backend.starts.load(Ordering::SeqCst)
}
fn stops(backend: &FakeBackend) -> usize {
    backend.stops.load(Ordering::SeqCst)
}

#[test]
fn back_to_back_runs_share_one_server() {
    let (backend, servers) = manager(IDLE_TIMEOUT);
    let (s, t) = (spec("Default"), token("a"));
    let first = servers.ensure("mono-a", &launch(&s, &t)).unwrap().0;
    let second = servers.ensure("mono-a", &launch(&s, &t)).unwrap().0;
    assert_eq!(first, second);
    assert_eq!(starts(&backend), 1);
    assert_eq!(stops(&backend), 0);
    // Each run asks the server to keep its own browser client connected.
    assert_eq!(backend.connects.load(Ordering::SeqCst), 2);
    // Another Mono has its own server and its own group.
    let other = servers.ensure("mono-b", &launch(&s, &t)).unwrap().0;
    assert_ne!(first, other);
    assert_eq!(starts(&backend), 2);
    assert_eq!(servers.running(), 2);
}

#[test]
fn a_dead_server_is_started_again() {
    let (backend, servers) = manager(IDLE_TIMEOUT);
    let (s, t) = (spec("Default"), token("a"));
    let first = servers.ensure("mono-a", &launch(&s, &t)).unwrap().0;
    backend.alive.lock().unwrap()[0].store(false, Ordering::SeqCst);
    let second = servers.ensure("mono-a", &launch(&s, &t)).unwrap().0;
    assert_ne!(first, second);
    assert_eq!(starts(&backend), 2);
    assert_eq!(servers.running(), 1);
}

#[test]
fn a_changed_launch_replaces_the_server() {
    let (backend, servers) = manager(IDLE_TIMEOUT);
    let (s, t) = (spec("Default"), token("a"));
    servers.ensure("mono-a", &launch(&s, &t)).unwrap();
    // Another profile or other rules change the arguments; another token changes the hash.
    servers
        .ensure("mono-a", &launch(&spec("Profile 6"), &t))
        .unwrap();
    assert_eq!((starts(&backend), stops(&backend)), (2, 1));
    servers
        .ensure("mono-a", &launch(&spec("Profile 6"), &token("b")))
        .unwrap();
    assert_eq!((starts(&backend), stops(&backend)), (3, 2));
    assert_eq!(backend.tokens.lock().unwrap()[2], token("b").as_str());
    assert_eq!(servers.running(), 1);
}

#[test]
fn stopping_a_mono_or_the_app_ends_its_servers() {
    let (backend, servers) = manager(IDLE_TIMEOUT);
    let (s, t) = (spec("Default"), token("a"));
    servers.ensure("mono-a", &launch(&s, &t)).unwrap();
    servers.ensure("mono-b", &launch(&s, &t)).unwrap();
    servers.stop("mono-a");
    assert_eq!((stops(&backend), servers.running()), (1, 1));
    // Stopping a Mono that has no server is harmless.
    servers.stop("mono-a");
    assert_eq!(stops(&backend), 1);
    servers.stop_everything();
    assert_eq!((stops(&backend), servers.running()), (2, 0));
    // The next run starts a fresh one.
    servers.ensure("mono-a", &launch(&s, &t)).unwrap();
    assert_eq!(starts(&backend), 3);
}

#[test]
fn an_idle_server_is_stopped_when_another_mono_asks() {
    let (backend, servers) = manager(Duration::from_millis(30));
    let (s, t) = (spec("Default"), token("a"));
    servers.ensure("mono-a", &launch(&s, &t)).unwrap();
    std::thread::sleep(Duration::from_millis(80));
    servers.ensure("mono-b", &launch(&s, &t)).unwrap();
    assert_eq!((stops(&backend), servers.running()), (1, 1));
    // A Mono that asks again within the window keeps its server.
    let (backend, servers) = manager(Duration::from_secs(60));
    servers.ensure("mono-a", &launch(&s, &t)).unwrap();
    servers.ensure("mono-b", &launch(&s, &t)).unwrap();
    assert_eq!((starts(&backend), stops(&backend)), (2, 0));
}

#[test]
fn a_run_in_use_keeps_its_server_and_idle_counts_from_the_last_release() {
    let (backend, servers) = manager(Duration::from_millis(60));
    let (s, t) = (spec("Default"), token("a"));
    let (_, lease) = servers.ensure("mono-a", &launch(&s, &t)).unwrap();
    std::thread::sleep(Duration::from_millis(150));
    // Long past the idle time, but a run still holds the lease: another Mono's run must not
    // stop it.
    servers.ensure("mono-b", &launch(&s, &t)).unwrap();
    assert_eq!(stops(&backend), 0);
    assert_eq!(servers.running(), 2);
    // The run ends: idle counts from now, not from the start.
    drop(lease);
    servers.ensure("mono-c", &launch(&s, &t)).unwrap();
    assert_eq!(stops(&backend), 0, "just released");
    std::thread::sleep(Duration::from_millis(150));
    servers.ensure("mono-c", &launch(&s, &t)).unwrap();
    assert_eq!(stops(&backend), 2, "mono-a and mono-b are idle now");
    assert_eq!(servers.running(), 1);
}

#[test]
fn two_runs_of_one_mono_that_start_together_share_one_server() {
    struct Slow(Arc<FakeBackend>);
    impl ServerBackend for Slow {
        fn start(&self, launch: &ServerLaunch) -> Result<Box<dyn RunningServer>, String> {
            std::thread::sleep(Duration::from_millis(150));
            self.0.start(launch)
        }
    }
    let backend = Arc::new(FakeBackend::default());
    let servers = Arc::new(BrowserServers::new(
        Arc::new(Slow(backend.clone())),
        IDLE_TIMEOUT,
    ));
    let urls: Vec<String> = (0..4)
        .map(|_| {
            let servers = servers.clone();
            std::thread::spawn(move || {
                let (s, t) = (spec("Default"), token("a"));
                let (url, lease) = servers.ensure("mono-a", &launch(&s, &t)).unwrap();
                std::mem::forget(lease);
                url
            })
        })
        .collect::<Vec<_>>()
        .into_iter()
        .map(|h| h.join().unwrap())
        .collect();
    assert_eq!(starts(&backend), 1);
    assert_eq!(stops(&backend), 0);
    assert!(urls.iter().all(|u| u == &urls[0]));
    assert!(backend.alive.lock().unwrap()[0].load(Ordering::SeqCst));
}

#[test]
fn a_failed_start_is_reported_and_leaves_nothing_behind() {
    struct Failing;
    impl ServerBackend for Failing {
        fn start(&self, _: &ServerLaunch) -> Result<Box<dyn RunningServer>, String> {
            Err("no node".into())
        }
    }
    let servers = BrowserServers::new(Arc::new(Failing), IDLE_TIMEOUT);
    let (s, t) = (spec("Default"), token("a"));
    assert_eq!(
        servers.ensure("mono-a", &launch(&s, &t)).err(),
        Some("no node".to_owned())
    );
    assert_eq!(servers.running(), 0);
}

#[test]
fn the_server_binds_loopback_only_and_accepts_only_its_own_host() {
    let base = vec![
        "--extension".to_owned(),
        "--session-name".into(),
        "Ops".into(),
    ];
    let args = http_args(&base, 5123);
    assert_eq!(&args[..3], &base[..]);
    let at = |flag: &str| args.iter().position(|a| a == flag).unwrap();
    assert_eq!(args[at("--port") + 1], "5123");
    assert_eq!(args[at("--host") + 1], "127.0.0.1");
    assert_eq!(args[at("--allowed-hosts") + 1], "127.0.0.1:5123");
    assert!(args.contains(&"--shared-browser-context".to_owned()));
    assert!(!args.iter().any(|a| a == "0.0.0.0" || a == "*"));
}

#[test]
fn the_fingerprint_follows_the_launch_and_hides_the_token() {
    let (s, t) = (spec("Default"), token("a"));
    let same = launch(&s, &t).fingerprint();
    assert_eq!(same, launch(&spec("Default"), &token("a")).fingerprint());
    assert_ne!(same, launch(&spec("Profile 6"), &t).fingerprint());
    assert_ne!(same, launch(&s, &token("b")).fingerprint());
}

#[test]
fn free_ports_are_on_loopback_and_differ() {
    let a = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let busy = a.local_addr().unwrap().port();
    let port = free_port().unwrap();
    assert_ne!(port, busy);
    assert!(port > 0);
}

/// A stand-in server for the legacy SSE transport: sends the endpoint event, answers
/// `initialize` and then every tools/call on the event stream, and counts the calls.
fn fake_sse_server(call_ok: bool) -> (u16, Arc<AtomicU64>, Arc<AtomicBool>) {
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    let calls = Arc::new(AtomicU64::new(0));
    let streams = Arc::new(Mutex::new(Vec::<std::net::TcpStream>::new()));
    let initialized = Arc::new(AtomicBool::new(false));
    let (c, i) = (calls.clone(), initialized.clone());
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let (calls, streams, initialized) = (c.clone(), streams.clone(), i.clone());
            std::thread::spawn(move || {
                let mut stream = stream;
                let mut buffer = Vec::new();
                let mut chunk = [0u8; 4096];
                // Read the head and the body (Content-Length).
                let (head, body) = loop {
                    let n = stream.read(&mut chunk).unwrap_or(0);
                    if n == 0 {
                        return;
                    }
                    buffer.extend_from_slice(&chunk[..n]);
                    let text = String::from_utf8_lossy(&buffer).into_owned();
                    if let Some(end) = text.find("\r\n\r\n") {
                        let head = text[..end].to_owned();
                        let length = head
                            .lines()
                            .find_map(|l| {
                                l.to_lowercase()
                                    .strip_prefix("content-length:")
                                    .map(|v| v.trim().parse::<usize>().unwrap())
                            })
                            .unwrap_or(0);
                        if text.len() >= end + 4 + length {
                            break (head, text[end + 4..end + 4 + length].to_owned());
                        }
                    }
                };
                if head.starts_with("GET /sse") {
                    let _ = write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\nevent: endpoint\r\ndata: /sse?sessionId=s1\r\n\r\n");
                    let _ = stream.flush();
                    streams.lock().unwrap().push(stream);
                    return;
                }
                let _ = write!(stream, "HTTP/1.1 202 Accepted\r\nContent-Length: 8\r\nConnection: close\r\n\r\nAccepted");
                let message: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
                let reply = match message["method"].as_str() {
                    Some("initialize") => {
                        Some(json!({"jsonrpc":"2.0","id":message["id"],"result":{}}))
                    }
                    Some("notifications/initialized") => {
                        initialized.store(true, Ordering::SeqCst);
                        None
                    }
                    Some("tools/call") => {
                        calls.fetch_add(1, Ordering::SeqCst);
                        Some(if call_ok {
                            json!({"jsonrpc":"2.0","id":message["id"],"result":{"content":[]}})
                        } else {
                            json!({"jsonrpc":"2.0","id":message["id"],"result":{"isError":true}})
                        })
                    }
                    _ => None,
                };
                if let Some(reply) = reply {
                    for mut open in streams
                        .lock()
                        .unwrap()
                        .iter()
                        .filter_map(|s| s.try_clone().ok())
                    {
                        let _ = write!(open, "event: message\r\ndata: {reply}\r\n\r\n");
                        let _ = open.flush();
                    }
                }
            });
        }
    });
    (port, calls, initialized)
}
fn wait_for(what: &str, condition: impl Fn() -> bool) {
    for _ in 0..200 {
        if condition() {
            return;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    panic!("timed out waiting for {what}");
}

#[test]
fn the_keeper_connects_once_opens_the_browser_and_stays_connected() {
    let (port, calls, initialized) = fake_sse_server(true);
    let keeper = Keeper::new(port);
    Keeper::ensure(&keeper);
    wait_for("the opening call", || keeper.is_warm());
    assert!(initialized.load(Ordering::SeqCst));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    // Later runs find it connected and warm: no new session, no new call.
    for _ in 0..3 {
        Keeper::ensure(&keeper);
    }
    std::thread::sleep(Duration::from_millis(150));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert!(keeper.is_connected());
}

#[test]
fn the_keeper_repeats_the_opening_call_after_a_failed_one() {
    let (port, calls, _) = fake_sse_server(false);
    let keeper = Keeper::new(port);
    Keeper::ensure(&keeper);
    wait_for("the failed call", || {
        calls.load(Ordering::SeqCst) == 1 && !keeper.calling.load(Ordering::SeqCst)
    });
    assert!(!keeper.is_warm());
    Keeper::ensure(&keeper);
    wait_for("the repeated call", || calls.load(Ordering::SeqCst) == 2);
}

#[test]
fn a_stopped_or_unreachable_keeper_does_not_connect() {
    let keeper = Keeper::new(free_port().unwrap());
    keeper.stopped.store(true, Ordering::SeqCst);
    Keeper::ensure(&keeper);
    assert!(!keeper.is_connected());
    // Nothing listens: the thread ends and the keeper can try again later.
    let keeper = Keeper::new(free_port().unwrap());
    Keeper::ensure(&keeper);
    wait_for("the failed connection", || !keeper.is_connected());
    assert!(!keeper.is_warm());
}
