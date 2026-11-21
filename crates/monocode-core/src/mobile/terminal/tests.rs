use super::*;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

struct ManualClock(AtomicU64);

impl ManualClock {
    fn new() -> Arc<Self> {
        Arc::new(Self(AtomicU64::new(1_000)))
    }
    fn advance(&self, ms: u64) {
        self.0.fetch_add(ms, Ordering::SeqCst);
    }
}

impl Clock for ManualClock {
    fn now_ms(&self) -> u64 {
        self.0.load(Ordering::SeqCst)
    }
}

/// A shell that records what it is told and can echo input as output.
pub(crate) struct FakeShell {
    pub(crate) out: Arc<Output>,
    pub(crate) echo: bool,
    pub(crate) written: Mutex<Vec<u8>>,
    pub(crate) resized: Mutex<Vec<(u16, u16)>>,
    pub(crate) kills: AtomicUsize,
}

impl ShellHandle for FakeShell {
    fn write(&self, data: &[u8]) -> Result<(), String> {
        self.written.lock().unwrap().extend_from_slice(data);
        if self.echo {
            self.out.push(data);
        }
        Ok(())
    }
    fn resize(&self, cols: u16, rows: u16) {
        self.resized.lock().unwrap().push((cols, rows));
    }
    fn kill(&self) {
        self.kills.fetch_add(1, Ordering::SeqCst);
    }
}

#[derive(Default)]
pub(crate) struct FakeSpawner {
    pub(crate) shells: Mutex<Vec<Arc<FakeShell>>>,
    pub(crate) fail: std::sync::atomic::AtomicBool,
    pub(crate) cwds: Mutex<Vec<PathBuf>>,
}

impl ShellSpawner for FakeSpawner {
    fn spawn(&self, spec: SpawnSpec, out: Arc<Output>) -> Result<Arc<dyn ShellHandle>, String> {
        if self.fail.load(Ordering::SeqCst) {
            return Err("no shell".into());
        }
        self.cwds.lock().unwrap().push(spec.cwd);
        let shell = Arc::new(FakeShell {
            out,
            echo: true,
            written: Mutex::default(),
            resized: Mutex::default(),
            kills: AtomicUsize::new(0),
        });
        self.shells.lock().unwrap().push(shell.clone());
        Ok(shell)
    }
}

struct Fixture {
    hub: Arc<Hub>,
    clock: Arc<ManualClock>,
    spawner: Arc<FakeSpawner>,
}

fn fixture_with(limits: Limits) -> Fixture {
    let clock = ManualClock::new();
    let hub = Hub::new(clock.clone(), limits);
    let spawner = Arc::new(FakeSpawner::default());
    hub.set_spawner(spawner.clone());
    Fixture {
        hub,
        clock,
        spawner,
    }
}

fn fixture() -> Fixture {
    fixture_with(Limits::default())
}

fn owner(device: &str) -> Owner {
    Owner {
        device_id: device.into(),
        fingerprint: format!("fp-{device}"),
        name: format!("Browser {device}"),
        relay_id: Some(format!("relay-{device}")),
    }
}

fn request() -> OpenRequest {
    OpenRequest {
        project: "p1".into(),
        label: "Project".into(),
        cwd: PathBuf::from("/tmp"),
        cols: 80,
        rows: 24,
    }
}

/// Opens a shell whose ticket the server accepts.
fn open_shell(f: &Fixture, who: &Owner) -> Opened {
    f.hub
        .open(who, request(), || TicketCheck::Accepted)
        .unwrap()
}

fn code_of(failure: &Failure) -> Option<&str> {
    failure.code.as_deref()
}

fn kills(f: &Fixture) -> Vec<usize> {
    f.spawner
        .shells
        .lock()
        .unwrap()
        .iter()
        .map(|s| s.kills.load(Ordering::SeqCst))
        .collect()
}

#[test]
fn constant_time_eq_compares_content_and_length() {
    assert!(constant_time_eq(b"ABC234", b"ABC234"));
    assert!(!constant_time_eq(b"ABC234", b"ABC235"));
    assert!(!constant_time_eq(b"ABC234", b"ABC23"));
    assert!(!constant_time_eq(b"", b"A"));
}

#[test]
fn nothing_is_opened_or_asked_without_a_shell_backend() {
    let hub = Hub::new(ManualClock::new(), Limits::default());
    let asked = AtomicUsize::new(0);
    let failure = hub
        .open(&owner("a"), request(), || {
            asked.fetch_add(1, Ordering::SeqCst);
            TicketCheck::Accepted
        })
        .unwrap_err();
    assert_eq!(code_of(&failure), Some("unsupported"));
    assert_eq!(asked.load(Ordering::SeqCst), 0, "no ticket is spent");
}

#[test]
fn an_accepted_ticket_opens_a_shell_in_the_asked_folder() {
    let f = fixture();
    let who = owner("a");
    let opened = open_shell(&f, &who);
    assert_eq!((opened.cols, opened.rows), (80, 24));
    assert_eq!(opened.idle_ms, 15 * 60_000);
    assert_eq!(opened.max_ms, 8 * 60 * 60_000);
    assert_eq!(f.spawner.cwds.lock().unwrap()[0], PathBuf::from("/tmp"));
    assert_eq!(f.hub.status().open[0].device_name, who.name);
}

#[test]
fn a_refused_ticket_spawns_nothing_and_gives_its_slot_back() {
    let f = fixture();
    let who = owner("a");
    for _ in 0..3 {
        let failure = f
            .hub
            .open(&who, request(), || TicketCheck::Rejected)
            .unwrap_err();
        assert_eq!(code_of(&failure), Some("fresh_required"));
    }
    assert!(f.spawner.shells.lock().unwrap().is_empty());
    assert!(f.hub.status().open.is_empty());
    // The slots are free again.
    open_shell(&f, &who);
    open_shell(&f, &who);
}

#[test]
fn a_ticket_that_the_server_consumed_cannot_open_a_second_shell() {
    let f = fixture();
    let who = owner("a");
    // Like the server: a ticket is accepted once, then it is gone.
    let used = Mutex::new(std::collections::HashSet::new());
    let server = |ticket: &str| {
        if used.lock().unwrap().insert(ticket.to_owned()) {
            TicketCheck::Accepted
        } else {
            TicketCheck::Rejected
        }
    };
    f.hub.open(&who, request(), || server("t1")).unwrap();
    let replay = f.hub.open(&who, request(), || server("t1")).unwrap_err();
    assert_eq!(code_of(&replay), Some("fresh_required"));
    assert_eq!(f.spawner.shells.lock().unwrap().len(), 1);
    f.hub.open(&who, request(), || server("t2")).unwrap();
    assert_eq!(f.spawner.shells.lock().unwrap().len(), 2);
}

#[test]
fn a_locally_refused_open_never_asks_the_server() {
    let f = fixture();
    let who = owner("a");
    open_shell(&f, &who);
    open_shell(&f, &who);
    let asked = AtomicUsize::new(0);
    let failure = f
        .hub
        .open(&who, request(), || {
            asked.fetch_add(1, Ordering::SeqCst);
            TicketCheck::Accepted
        })
        .unwrap_err();
    assert_eq!(code_of(&failure), Some("busy"));
    assert_eq!(asked.load(Ordering::SeqCst), 0, "the ticket is not burned");
    // The same holds overall.
    let (b, c) = (owner("b"), owner("c"));
    open_shell(&f, &b);
    open_shell(&f, &b);
    let failure = f
        .hub
        .open(&c, request(), || {
            asked.fetch_add(1, Ordering::SeqCst);
            TicketCheck::Accepted
        })
        .unwrap_err();
    assert_eq!(code_of(&failure), Some("busy"));
    assert_eq!(asked.load(Ordering::SeqCst), 0);
}

#[test]
fn a_device_holds_a_limited_number_of_shells() {
    let f = fixture();
    let who = owner("a");
    open_shell(&f, &who);
    open_shell(&f, &who);
    let failure = f
        .hub
        .open(&who, request(), || TicketCheck::Accepted)
        .unwrap_err();
    assert_eq!(code_of(&failure), Some("busy"));
}

#[test]
fn opens_waiting_on_the_server_count_against_the_limits_and_each_needs_its_own_ticket() {
    let f = fixture();
    let who = owner("a");
    let entered = Arc::new(AtomicUsize::new(0));
    let release = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let seen = Arc::new(Mutex::new(Vec::new()));
    let mut waiting = Vec::new();
    for (want, ticket) in [(1, "t1"), (2, "t2")] {
        let (hub, who) = (f.hub.clone(), who.clone());
        let (inside, release, seen) = (entered.clone(), release.clone(), seen.clone());
        waiting.push(std::thread::spawn(move || {
            hub.open(&who, request(), || {
                seen.lock().unwrap().push(ticket);
                inside.fetch_add(1, Ordering::SeqCst);
                while !release.load(Ordering::SeqCst) {
                    std::thread::sleep(std::time::Duration::from_millis(2));
                }
                TicketCheck::Accepted
            })
        }));
        while entered.load(Ordering::SeqCst) < want {
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
    }
    // Two tickets are being checked and no shell exists yet: a third open is
    // already refused and its ticket is not asked about.
    let asked = AtomicUsize::new(0);
    let third = f
        .hub
        .open(&who, request(), || {
            asked.fetch_add(1, Ordering::SeqCst);
            TicketCheck::Accepted
        })
        .unwrap_err();
    assert_eq!(code_of(&third), Some("busy"));
    assert_eq!(asked.load(Ordering::SeqCst), 0);
    release.store(true, Ordering::SeqCst);
    for thread in waiting {
        thread.join().unwrap().unwrap();
    }
    let mut seen = seen.lock().unwrap().clone();
    seen.sort();
    assert_eq!(seen, ["t1", "t2"]);
    assert_eq!(f.hub.status().open.len(), 2);
}

/// A spawner that waits inside `spawn` until released, like a slow pty start.
struct GatedSpawner {
    inner: FakeSpawner,
    entered: std::sync::atomic::AtomicUsize,
    release: std::sync::atomic::AtomicBool,
}

impl ShellSpawner for GatedSpawner {
    fn spawn(&self, spec: SpawnSpec, out: Arc<Output>) -> Result<Arc<dyn ShellHandle>, String> {
        self.entered.fetch_add(1, Ordering::SeqCst);
        while !self.release.load(Ordering::SeqCst) {
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        self.inner.spawn(spec, out)
    }
}

#[test]
fn shells_still_starting_count_against_the_per_device_limit() {
    let f = fixture();
    let gate = Arc::new(GatedSpawner {
        inner: FakeSpawner::default(),
        entered: Default::default(),
        release: Default::default(),
    });
    f.hub.set_spawner(gate.clone());
    let who = owner("a");
    let mut starting = Vec::new();
    for want in 1..=2 {
        let (hub, who) = (f.hub.clone(), who.clone());
        starting.push(std::thread::spawn(move || {
            hub.open(&who, request(), || TicketCheck::Accepted)
        }));
        while gate.entered.load(Ordering::SeqCst) < want {
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
    }
    // Two shells are starting and none is registered yet: a third open must
    // already be refused.
    let failure = f
        .hub
        .open(&who, request(), || TicketCheck::Accepted)
        .unwrap_err();
    assert_eq!(code_of(&failure), Some("busy"));
    gate.release.store(true, Ordering::SeqCst);
    for thread in starting {
        thread.join().unwrap().unwrap();
    }
    assert_eq!(f.hub.status().open.len(), 2);
}

struct PanickingSpawner;

impl ShellSpawner for PanickingSpawner {
    fn spawn(&self, _: SpawnSpec, _: Arc<Output>) -> Result<Arc<dyn ShellHandle>, String> {
        panic!("spawner panic (expected by the test)");
    }
}

#[test]
fn a_panicking_spawner_gives_its_slot_back() {
    let f = fixture();
    let who = owner("a");
    f.hub.set_spawner(Arc::new(PanickingSpawner));
    for _ in 0..3 {
        let hub = f.hub.clone();
        let who = who.clone();
        let joined =
            std::thread::spawn(move || hub.open(&who, request(), || TicketCheck::Accepted)).join();
        assert!(joined.is_err(), "the spawner panicked");
    }
    f.hub.set_spawner(f.spawner.clone());
    open_shell(&f, &who);
    open_shell(&f, &who);
}

#[test]
fn a_failed_spawn_gives_its_slot_back() {
    let f = fixture();
    let who = owner("a");
    f.spawner.fail.store(true, Ordering::SeqCst);
    for _ in 0..3 {
        f.hub
            .open(&who, request(), || TicketCheck::Accepted)
            .unwrap_err();
    }
    f.spawner.fail.store(false, Ordering::SeqCst);
    open_shell(&f, &who);
    open_shell(&f, &who);
}

#[test]
fn a_failed_spawn_opens_nothing() {
    let f = fixture();
    let who = owner("a");
    f.spawner.fail.store(true, Ordering::SeqCst);
    let failure = f
        .hub
        .open(&who, request(), || TicketCheck::Accepted)
        .unwrap_err();
    assert_eq!(failure.error, "no shell");
    assert!(f.hub.status().open.is_empty());
}

#[tokio::test]
async fn input_reaches_the_shell_and_its_output_comes_back_in_order() {
    let f = fixture();
    let who = owner("a");
    let opened = open_shell(&f, &who);
    f.hub.write(&who, &opened.id, b"ls\r").unwrap();
    f.hub.resize(&who, &opened.id, 100, 30).unwrap();
    let shell = f.spawner.shells.lock().unwrap()[0].clone();
    assert_eq!(shell.written.lock().unwrap().as_slice(), b"ls\r");
    assert_eq!(shell.resized.lock().unwrap().as_slice(), [(100, 30)]);
    let first = f.hub.read(&who, &opened.id, 0, 0, 1024).await.unwrap();
    assert_eq!((first.from, first.next), (0, 3));
    assert_eq!(first.data, b"ls\r");
    assert!(first.exited.is_none());
    // Reading from `next` acknowledges, and with nothing new returns empty.
    let idle = f
        .hub
        .read(&who, &opened.id, first.next, 0, 1024)
        .await
        .unwrap();
    assert!(idle.data.is_empty());
    assert_eq!(idle.next, 3);
}

#[tokio::test]
async fn a_long_poll_wakes_when_output_arrives() {
    let f = fixture();
    let who = owner("a");
    let opened = open_shell(&f, &who);
    let hub = f.hub.clone();
    let (reader, id) = (who.clone(), opened.id.clone());
    let waiting = tokio::spawn(async move { hub.read(&reader, &id, 0, 10_000, 1024).await });
    tokio::time::sleep(Duration::from_millis(50)).await;
    f.hub.write(&who, &opened.id, b"x").unwrap();
    let answer = tokio::time::timeout(Duration::from_secs(5), waiting)
        .await
        .expect("the poll woke up")
        .unwrap()
        .unwrap();
    assert_eq!(answer.data, b"x");
}

#[test]
fn output_flow_is_windowed_and_chunked() {
    let out = Output::new(8);
    assert!(out.push(b"12345678"));
    // A producer waits while a full window is unacknowledged.
    let producer = {
        let out = out.clone();
        std::thread::spawn(move || out.push(b"abcd"))
    };
    std::thread::sleep(Duration::from_millis(150));
    assert_eq!(out.buffered(), 8, "the producer is blocked, nothing grew");
    let first = out.take(0, 3);
    assert_eq!((first.next, first.data.as_slice()), (3, &b"123"[..]));
    assert!(!first.drained);
    // Taking without moving `from` acknowledges nothing: still blocked.
    std::thread::sleep(Duration::from_millis(150));
    assert_eq!(out.buffered(), 8);
    // Acknowledging by asking from offset 8 frees the window.
    let second = out.take(8, 100);
    assert_eq!(second.from, 8);
    assert!(producer.join().unwrap(), "the producer went on");
    assert_eq!(out.take(8, 100).data, b"abcd");
}

#[test]
fn a_closed_output_frees_a_blocked_producer_and_refuses_more() {
    let out = Output::new(4);
    assert!(out.push(b"1234"));
    let producer = {
        let out = out.clone();
        std::thread::spawn(move || out.push(b"more"))
    };
    std::thread::sleep(Duration::from_millis(100));
    out.close();
    assert!(!producer.join().unwrap());
    assert!(!out.push(b"x"));
    // What was there stays readable after the close.
    assert_eq!(out.take(0, 10).data, b"1234");
}

#[tokio::test]
async fn a_shell_that_exits_reports_its_status_after_its_output() {
    let f = fixture();
    let who = owner("a");
    let opened = open_shell(&f, &who);
    let shell = f.spawner.shells.lock().unwrap()[0].clone();
    shell.out.push(b"bye\n");
    shell.out.finish(Some(3));
    let first = f.hub.read(&who, &opened.id, 0, 0, 1024).await.unwrap();
    assert_eq!(first.data, b"bye\n");
    assert!(first.exited.is_none());
    let last = f
        .hub
        .read(&who, &opened.id, first.next, 0, 1024)
        .await
        .unwrap();
    assert_eq!(last.exited, Some((Reason::Exit, Some(3))));
    assert_eq!(kills(&f), [1], "the exited shell's group is reaped");
}

#[tokio::test]
async fn a_shell_that_exits_without_being_read_is_noticed_by_the_tick() {
    let f = fixture();
    let who = owner("a");
    open_shell(&f, &who);
    f.spawner.shells.lock().unwrap()[0].out.finish(Some(0));
    f.hub.tick();
    assert!(f.hub.status().open.is_empty());
    assert_eq!(kills(&f), [1]);
}

#[test]
fn another_device_cannot_touch_a_shell_it_does_not_own() {
    let f = fixture();
    let (a, b) = (owner("a"), owner("b"));
    let opened = open_shell(&f, &a);
    for result in [
        f.hub.write(&b, &opened.id, b"x"),
        f.hub.resize(&b, &opened.id, 80, 24),
        f.hub.close(&b, &opened.id),
    ] {
        assert_eq!(code_of(&result.unwrap_err()), Some("not_found"));
    }
    // The same fingerprint under another row id is another device too.
    let mut imposter = a.clone();
    imposter.fingerprint = "fp-other".into();
    assert!(f.hub.write(&imposter, &opened.id, b"x").is_err());
    assert_eq!(kills(&f), [0]);
}

#[test]
fn input_is_bounded() {
    let f = fixture();
    let who = owner("a");
    let opened = open_shell(&f, &who);
    assert!(f
        .hub
        .write(&who, &opened.id, &vec![0; MAX_INPUT_BYTES])
        .is_ok());
    assert!(f
        .hub
        .write(&who, &opened.id, &vec![0; MAX_INPUT_BYTES + 1])
        .is_err());
}

#[test]
fn the_browser_can_close_its_shell_and_it_is_killed_once() {
    let f = fixture();
    let who = owner("a");
    let opened = open_shell(&f, &who);
    f.hub.close(&who, &opened.id).unwrap();
    f.hub.close(&who, &opened.id).unwrap();
    f.hub.tick();
    assert_eq!(kills(&f), [1]);
    assert!(f.hub.write(&who, &opened.id, b"x").is_err());
}

#[tokio::test]
async fn an_ended_shell_stays_readable_with_its_reason_and_then_is_forgotten() {
    let f = fixture();
    let who = owner("a");
    let opened = open_shell(&f, &who);
    f.hub.write(&who, &opened.id, b"last").unwrap();
    assert!(f.hub.kill(&opened.id));
    let ended = f.hub.read(&who, &opened.id, 0, 0, 1024).await.unwrap();
    assert_eq!(ended.data, b"last");
    assert_eq!(ended.exited, Some((Reason::Killed, None)));
    f.clock.advance(Limits::default().tombstone_ms);
    let gone = f.hub.read(&who, &opened.id, 0, 0, 1024).await.unwrap_err();
    assert_eq!(code_of(&gone), Some("not_found"));
}

#[test]
fn the_kill_button_ends_one_shell_and_only_that_one() {
    let f = fixture();
    let (a, b) = (owner("a"), owner("b"));
    let first = open_shell(&f, &a);
    open_shell(&f, &b);
    assert!(f.hub.kill(&first.id));
    assert!(!f.hub.kill(&first.id), "already ended");
    assert!(!f.hub.kill("nope"));
    assert_eq!(kills(&f), [1, 0]);
    assert_eq!(f.hub.status().open.len(), 1);
}

#[test]
fn killing_a_device_ends_its_shells() {
    let f = fixture();
    let (a, b) = (owner("a"), owner("b"));
    open_shell(&f, &a);
    open_shell(&f, &b);
    assert_eq!(f.hub.kill_device("a", Reason::Revoked), 1);
    assert_eq!(kills(&f), [1, 0]);
    assert_eq!(f.hub.open_count("a"), 0);
    assert_eq!(f.hub.open_count("b"), 1);
}

#[test]
fn killing_by_key_and_killing_all_reach_every_shell() {
    let f = fixture();
    let (a, b) = (owner("a"), owner("b"));
    open_shell(&f, &a);
    open_shell(&f, &b);
    assert_eq!(f.hub.kill_fingerprint("fp-a", Reason::Revoked), 1);
    assert_eq!(kills(&f), [1, 0]);
    assert_eq!(f.hub.kill_all(Reason::Desktop), 1);
    assert_eq!(kills(&f), [1, 1]);
    assert_eq!(f.hub.kill_all(Reason::Desktop), 0);
    assert_eq!(kills(&f), [1, 1], "an ended shell is not killed again");
}

#[test]
fn a_shell_whose_project_is_no_longer_shared_is_killed() {
    let f = fixture();
    let who = owner("a");
    open_shell(&f, &who);
    assert_eq!(f.hub.kill_unselected("a", &["p1".to_owned()]), 0);
    assert_eq!(f.hub.kill_unselected("a", &["p2".to_owned()]), 1);
    assert_eq!(kills(&f), [1]);
}

#[test]
fn the_idle_limit_counts_input_not_output() {
    let f = fixture();
    let who = owner("a");
    let opened = open_shell(&f, &who);
    f.clock.advance(Limits::default().idle_ms - 1);
    // Output does not keep a shell alive.
    f.spawner.shells.lock().unwrap()[0].out.push(b"noise");
    f.hub.tick();
    assert_eq!(f.hub.status().open.len(), 1);
    // A keystroke does.
    f.hub.write(&who, &opened.id, b"a").unwrap();
    f.clock.advance(Limits::default().idle_ms - 1);
    f.hub.tick();
    assert_eq!(f.hub.status().open.len(), 1);
    f.clock.advance(1);
    f.hub.tick();
    assert!(f.hub.status().open.is_empty());
    assert_eq!(kills(&f), [1]);
}

#[tokio::test]
async fn the_idle_end_reaches_the_browser_with_its_reason() {
    let f = fixture();
    let who = owner("a");
    let opened = open_shell(&f, &who);
    f.clock.advance(Limits::default().idle_ms);
    f.hub.tick();
    let ended = f.hub.read(&who, &opened.id, 0, 0, 1024).await.unwrap();
    assert_eq!(
        ended.exited.map(|(reason, _)| reason.as_str()),
        Some("idle")
    );
}

#[test]
fn the_maximum_duration_ends_a_shell_that_is_still_typed_into() {
    let f = fixture();
    let who = owner("a");
    let opened = open_shell(&f, &who);
    let step = Limits::default().idle_ms / 2;
    let mut elapsed = 0;
    while elapsed + step < Limits::default().max_ms {
        f.clock.advance(step);
        elapsed += step;
        f.hub.write(&who, &opened.id, b"k").unwrap();
        f.hub.tick();
        assert_eq!(f.hub.status().open.len(), 1, "alive at {elapsed} ms");
    }
    f.clock.advance(Limits::default().max_ms - elapsed);
    f.hub.tick();
    assert!(f.hub.status().open.is_empty());
    assert_eq!(kills(&f), [1]);
}

#[tokio::test]
async fn a_dropped_link_keeps_the_shell_for_the_grace_window_for_the_same_device_only() {
    let f = fixture();
    let who = owner("a");
    let opened = open_shell(&f, &who);
    f.hub.link_closed("relay-a");
    assert!(f.hub.status().open[0].detached);
    f.clock.advance(Limits::default().grace_ms - 1);
    f.hub.tick();
    assert_eq!(kills(&f), [0]);
    // Another device, even one that learned the id, cannot attach.
    let stranger = owner("b");
    assert!(f.hub.read(&stranger, &opened.id, 0, 0, 10).await.is_err());
    assert!(f.hub.status().open[0].detached);
    // The owner reads again: attached, and the grace clock is cancelled.
    f.hub.read(&who, &opened.id, 0, 0, 10).await.unwrap();
    assert!(!f.hub.status().open[0].detached);
    f.clock.advance(Limits::default().grace_ms * 3);
    f.hub.tick();
    assert_eq!(kills(&f), [0]);
}

#[tokio::test]
async fn a_shell_nobody_reattaches_is_killed_at_the_end_of_the_grace_window() {
    let f = fixture();
    let who = owner("a");
    let opened = open_shell(&f, &who);
    f.hub.link_closed("RELAY-A");
    f.clock.advance(Limits::default().grace_ms);
    f.hub.tick();
    assert!(f.hub.status().open.is_empty());
    assert_eq!(kills(&f), [1]);
    let ended = f.hub.read(&who, &opened.id, 0, 0, 10).await.unwrap();
    assert_eq!(ended.exited.map(|(reason, _)| reason), Some(Reason::Grace));
}

#[test]
fn a_link_that_closes_for_another_device_leaves_the_shell_attached() {
    let f = fixture();
    open_shell(&f, &owner("a"));
    f.hub.link_closed("relay-b");
    assert!(!f.hub.status().open[0].detached);
}

#[test]
fn the_tick_ends_shells_of_a_device_that_lost_its_standing() {
    let f = fixture();
    let standing = Arc::new(Mutex::new(Standing::Allowed));
    let seen = standing.clone();
    f.hub.set_rights(Arc::new(move |_| *seen.lock().unwrap()));
    let who = owner("a");
    open_shell(&f, &who);
    f.hub.tick();
    assert_eq!(kills(&f), [0]);
    *standing.lock().unwrap() = Standing::ToggleOff;
    f.hub.tick();
    assert_eq!(kills(&f), [1]);
    open_shell_again(&f, &who, &standing);
    *standing.lock().unwrap() = Standing::Revoked;
    f.hub.tick();
    assert_eq!(kills(&f), [1, 1]);
}

fn open_shell_again(f: &Fixture, who: &Owner, standing: &Arc<Mutex<Standing>>) {
    *standing.lock().unwrap() = Standing::Allowed;
    open_shell(f, who);
}

#[test]
fn a_device_revoked_while_its_shell_starts_gets_no_shell() {
    let f = fixture();
    f.hub.set_rights(Arc::new(|_| Standing::Revoked));
    let who = owner("a");
    let failure = f
        .hub
        .open(&who, request(), || TicketCheck::Accepted)
        .unwrap_err();
    assert_eq!(code_of(&failure), Some("revoked"));
    assert_eq!(kills(&f), [1], "the shell that was started is killed");
    assert!(f.hub.status().open.is_empty());
}

#[test]
fn killing_never_waits_for_the_shell() {
    // A handle whose kill returns at once is all the hub needs: it calls it
    // with no lock held, so a slow audit write or reader cannot block it.
    let f = fixture();
    let who = owner("a");
    let opened = open_shell(&f, &who);
    let shell = f.spawner.shells.lock().unwrap()[0].clone();
    // A producer blocked on a full window is freed by the kill.
    for _ in 0..(Limits::default().window / 1024) {
        assert!(shell.out.push(&[0; 1024]));
    }
    let blocked = {
        let out = shell.out.clone();
        std::thread::spawn(move || out.push(b"x"))
    };
    std::thread::sleep(Duration::from_millis(100));
    let started = Instant::now();
    f.hub.kill(&opened.id);
    assert!(started.elapsed() < Duration::from_secs(1));
    assert!(!blocked.join().unwrap());
}
