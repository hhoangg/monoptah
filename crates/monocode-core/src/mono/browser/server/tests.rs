use super::tests_support::FakeBackend;
use super::*;

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
fn the_fingerprint_follows_the_launch_and_hides_the_token() {
    let (s, t) = (spec("Default"), token("a"));
    let same = launch(&s, &t).fingerprint();
    assert_eq!(same, launch(&spec("Default"), &token("a")).fingerprint());
    assert_ne!(same, launch(&spec("Profile 6"), &t).fingerprint());
    assert_ne!(same, launch(&s, &token("b")).fingerprint());
}

#[test]
fn a_server_outlives_an_idle_gap_of_hours() {
    // The tabs of a Mono's group belong to its connection: stopping the server for a gap of
    // minutes made the next run open duplicates. The idle limit is hours.
    assert!(IDLE_TIMEOUT >= Duration::from_secs(8 * 60 * 60));
    let (backend, servers) = manager(IDLE_TIMEOUT);
    let (s, t) = (spec("Default"), token("a"));
    servers.ensure("mono-a", &launch(&s, &t)).unwrap();
    servers.ensure("mono-b", &launch(&s, &t)).unwrap();
    servers.ensure("mono-a", &launch(&s, &t)).unwrap();
    assert_eq!((starts(&backend), stops(&backend)), (2, 0));
}
