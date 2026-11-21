//! One long-lived `@playwright/mcp` server per Mono with a browser.
//!
//! Why: the extension makes one Chrome tab group per connection and greys the group when the
//! connection ends (`TabGroupManager.orphan`), never reusing it. A stdio server lives as long as
//! one agent process, so every Mono turn made a new group. Here the app runs the server in HTTP
//! mode and keeps it across turns; each run only connects an MCP client to it. With
//! `--shared-browser-context` every client shares the extension connection that the first client
//! opened, but the server closes that connection when its last client leaves. So the app keeps one
//! client of its own connected (the keeper, legacy SSE transport, which has no heartbeat): the
//! extension connection, and with it the Mono's tab group, outlives the turns.
//!
//! Facts and the residual risk: `docs/superpowers/specs/2026-11-22-browser-all-tabs-research.md`.
use super::{ServerSpec, TOKEN_ENV};
use crate::git_credentials::Secret;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

/// A server nobody has asked for in this long is stopped.
pub const IDLE_TIMEOUT: Duration = Duration::from_secs(30 * 60);
/// The first start may download the package with `npx`.
const READY_TIMEOUT: Duration = Duration::from_secs(120);
const LOOPBACK: &str = "127.0.0.1";

/// What starts a server: the command, its arguments (no port flags yet) and the token.
pub struct ServerLaunch<'a> {
    pub spec: &'a ServerSpec,
    pub token: &'a Secret,
    pub search_path: &'a str,
}
impl ServerLaunch<'_> {
    /// Changes when anything that shapes the server changes (profile, token, rules, name).
    /// A restart is needed exactly then. Only a hash of the token is kept.
    pub fn fingerprint(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        self.spec.command.hash(&mut hasher);
        self.spec.args.hash(&mut hasher);
        self.token.as_str().hash(&mut hasher);
        hasher.finish()
    }
}

/// A started server.
pub trait RunningServer: Send {
    /// The MCP endpoint clients connect to.
    fn url(&self) -> String;
    /// The process is still running.
    fn is_alive(&mut self) -> bool;
    /// Makes sure the app's own client is connected, so the browser connection stays open.
    fn keep_connected(&mut self);
    fn stop(&mut self);
}
/// Starts servers. Tests replace it so they need no node or Chrome.
pub trait ServerBackend: Send + Sync {
    fn start(&self, launch: &ServerLaunch) -> Result<Box<dyn RunningServer>, String>;
}

/// Counts the runs that use a server and when the last one ended.
struct Activity {
    active: AtomicUsize,
    last_release: Mutex<Instant>,
}
/// Held by a run for as long as it uses the server: the server is never stopped for being idle
/// while a lease exists, and idle time counts from the last lease dropped.
pub struct Lease {
    activity: Arc<Activity>,
}
impl Lease {
    fn new(activity: &Arc<Activity>) -> Self {
        activity.active.fetch_add(1, Ordering::SeqCst);
        Self {
            activity: Arc::clone(activity),
        }
    }
}
impl Drop for Lease {
    fn drop(&mut self) {
        *self
            .activity
            .last_release
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = Instant::now();
        self.activity.active.fetch_sub(1, Ordering::SeqCst);
    }
}
impl std::fmt::Debug for Lease {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Lease")
    }
}

struct Entry {
    fingerprint: u64,
    server: Box<dyn RunningServer>,
    activity: Arc<Activity>,
}
impl Entry {
    fn idle_for(&self, now: Instant) -> Option<Duration> {
        if self.activity.active.load(Ordering::SeqCst) > 0 {
            return None;
        }
        let last = *self
            .activity
            .last_release
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        Some(now.saturating_duration_since(last))
    }
}

/// The running servers, one per Mono id.
pub struct BrowserServers {
    backend: Arc<dyn ServerBackend>,
    idle: Duration,
    entries: Mutex<HashMap<String, Entry>>,
    /// One lock per Mono: while a server starts, a second run of that Mono waits for it and
    /// reuses it instead of starting (and later killing) another.
    starting: Mutex<HashMap<String, Arc<Mutex<()>>>>,
}
impl BrowserServers {
    pub fn new(backend: Arc<dyn ServerBackend>, idle: Duration) -> Self {
        Self {
            backend,
            idle,
            entries: Mutex::new(HashMap::new()),
            starting: Mutex::new(HashMap::new()),
        }
    }
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Entry>> {
        self.entries.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The URL of this Mono's server and a lease for the run: the running server, or a new one
    /// when there is none, it died or its launch changed. Keep the lease until the run ends.
    pub fn ensure(&self, mono_id: &str, launch: &ServerLaunch) -> Result<(String, Lease), String> {
        let gate = Arc::clone(
            self.starting
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .entry(mono_id.to_owned())
                .or_default(),
        );
        let _one_at_a_time = gate.lock().unwrap_or_else(|e| e.into_inner());
        let fingerprint = launch.fingerprint();
        let mut stale: Vec<Box<dyn RunningServer>> = Vec::new();
        let reused = {
            let mut map = self.lock();
            let now = Instant::now();
            // Servers of other Monos that no run uses and that were idle for too long.
            let expired: Vec<String> = map
                .iter()
                .filter(|(id, e)| {
                    id.as_str() != mono_id && e.idle_for(now).is_some_and(|d| d > self.idle)
                })
                .map(|(id, _)| id.clone())
                .collect();
            stale.extend(
                expired
                    .iter()
                    .filter_map(|id| map.remove(id))
                    .map(|e| e.server),
            );
            match map.get_mut(mono_id) {
                Some(entry) => {
                    if entry.fingerprint == fingerprint && entry.server.is_alive() {
                        entry.server.keep_connected();
                        Some((entry.server.url(), Lease::new(&entry.activity)))
                    } else {
                        stale.extend(map.remove(mono_id).map(|e| e.server));
                        None
                    }
                }
                None => None,
            }
        };
        // Stopping and starting are slow; neither holds the table lock.
        stop_all(stale);
        if let Some(reused) = reused {
            return Ok(reused);
        }
        let mut server = self.backend.start(launch)?;
        server.keep_connected();
        let url = server.url();
        let activity = Arc::new(Activity {
            active: AtomicUsize::new(0),
            last_release: Mutex::new(Instant::now()),
        });
        let lease = Lease::new(&activity);
        self.lock().insert(
            mono_id.to_owned(),
            Entry {
                fingerprint,
                server,
                activity,
            },
        );
        Ok((url, lease))
    }

    /// Stops this Mono's server (its browser was turned off or changed, or the Mono is gone).
    pub fn stop(&self, mono_id: &str) {
        let entry = self.lock().remove(mono_id);
        stop_all(entry.map(|e| e.server));
    }
    /// Stops every server (the app quits).
    pub fn stop_everything(&self) {
        let all: Vec<Entry> = self.lock().drain().map(|(_, e)| e).collect();
        stop_all(all.into_iter().map(|e| e.server));
    }
    pub fn running(&self) -> usize {
        self.lock().len()
    }
}
fn stop_all(servers: impl IntoIterator<Item = Box<dyn RunningServer>>) {
    for mut server in servers {
        server.stop();
    }
}

/// The manager the app uses: real node processes.
pub fn servers() -> &'static BrowserServers {
    static SERVERS: OnceLock<BrowserServers> = OnceLock::new();
    SERVERS.get_or_init(|| BrowserServers::new(Arc::new(NodeBackend), IDLE_TIMEOUT))
}

// ---- The real backend ----

pub struct NodeBackend;
impl ServerBackend for NodeBackend {
    fn start(&self, launch: &ServerLaunch) -> Result<Box<dyn RunningServer>, String> {
        let port = free_port()?;
        let mut command = std::process::Command::new(&launch.spec.command);
        command
            .args(http_args(&launch.spec.args, port))
            .env("PATH", launch.search_path)
            .env(TOKEN_ENV, launch.token.as_str())
            // The server exits when its stdin closes: if the app dies, so does the server.
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        crate::process::isolate(&mut command);
        let mut child = command
            .spawn()
            .map_err(|_| "Could not start npx. Check that Node.js is installed.".to_owned())?;
        // The token is in the environment only, so the argv record the guard writes is safe.
        let guard = crate::process::guard::arm(&child, &command);
        let stdin = child.stdin.take();
        let mut server = NodeServer {
            child,
            guard: Some(guard),
            _stdin: stdin,
            port,
            keeper: Keeper::new(port),
        };
        if let Err(error) = wait_until_listening(&mut server.child, port, READY_TIMEOUT) {
            server.stop();
            return Err(error);
        }
        Ok(Box::new(server))
    }
}

/// The base arguments plus the HTTP flags: loopback only, and only the loopback `Host` value of
/// this port accepted (the server's check against DNS rebinding).
pub fn http_args(base: &[String], port: u16) -> Vec<String> {
    let mut args = base.to_vec();
    args.extend([
        "--port".to_owned(),
        port.to_string(),
        "--host".into(),
        LOOPBACK.into(),
        "--allowed-hosts".into(),
        format!("{LOOPBACK}:{port}"),
        "--shared-browser-context".into(),
    ]);
    args
}

fn free_port() -> Result<u16, String> {
    std::net::TcpListener::bind((LOOPBACK, 0))
        .and_then(|listener| listener.local_addr())
        .map(|addr| addr.port())
        .map_err(|_| "Could not find a free port for the browser server.".to_owned())
}

fn wait_until_listening(
    child: &mut std::process::Child,
    port: u16,
    timeout: Duration,
) -> Result<(), String> {
    let deadline = Instant::now() + timeout;
    let address = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    loop {
        if std::net::TcpStream::connect_timeout(&address, Duration::from_millis(300)).is_ok() {
            // Something answers on the port; make sure it is our child and not another process
            // that took the port between choosing it and starting the server.
            return if matches!(child.try_wait(), Ok(None)) {
                Ok(())
            } else {
                Err("The browser server stopped while starting.".into())
            };
        }
        if !matches!(child.try_wait(), Ok(None)) {
            return Err("The browser server stopped while starting.".into());
        }
        if Instant::now() >= deadline {
            return Err("The browser server did not start in time.".into());
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

struct NodeServer {
    child: std::process::Child,
    guard: Option<crate::process::guard::ChildGuard>,
    _stdin: Option<std::process::ChildStdin>,
    port: u16,
    keeper: Arc<Keeper>,
}
impl RunningServer for NodeServer {
    fn url(&self) -> String {
        format!("http://{LOOPBACK}:{}/mcp", self.port)
    }
    fn is_alive(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }
    fn keep_connected(&mut self) {
        Keeper::ensure(&self.keeper);
    }
    fn stop(&mut self) {
        self.keeper.stopped.store(true, Ordering::SeqCst);
        match self.guard.take() {
            Some(guard) => guard.finish_with(&mut self.child),
            None => crate::process::terminate(&mut self.child),
        }
    }
}
impl Drop for NodeServer {
    fn drop(&mut self) {
        self.stop();
    }
}

// ---- The keeper: the app's own client of the server ----

/// A legacy SSE client that stays connected. It opens the browser connection (a first
/// `browser_tabs` call) and holds it open between turns. If the browser connection is gone (Chrome
/// quit), the next [`Keeper::ensure`] repeats the call, which opens a new one.
pub struct Keeper {
    base: String,
    /// The POST URL of the live stream, or None while there is none.
    endpoint: Mutex<Option<String>>,
    connected: AtomicBool,
    /// The last call that opened the browser connection worked.
    warm: AtomicBool,
    calling: AtomicBool,
    next_id: AtomicU64,
    pub stopped: AtomicBool,
}
impl Keeper {
    pub fn new(port: u16) -> Arc<Self> {
        Self::at(format!("http://{LOOPBACK}:{port}"))
    }
    pub fn at(base: String) -> Arc<Self> {
        Arc::new(Self {
            base,
            endpoint: Mutex::new(None),
            connected: AtomicBool::new(false),
            warm: AtomicBool::new(false),
            calling: AtomicBool::new(false),
            next_id: AtomicU64::new(10),
            stopped: AtomicBool::new(false),
        })
    }
    pub fn is_warm(&self) -> bool {
        self.warm.load(Ordering::SeqCst)
    }
    pub fn is_connected(&self) -> bool {
        self.connected.load(Ordering::SeqCst)
    }
    /// Connects when not connected, and repeats the opening call when the last one failed.
    /// Never blocks: the calls wait for Chrome on their own thread.
    pub fn ensure(this: &Arc<Self>) {
        if this.stopped.load(Ordering::SeqCst) {
            return;
        }
        if !this.connected.swap(true, Ordering::SeqCst) {
            let keeper = Arc::clone(this);
            std::thread::spawn(move || {
                let _ = keeper.run();
                keeper.connected.store(false, Ordering::SeqCst);
                keeper.warm.store(false, Ordering::SeqCst);
                keeper.calling.store(false, Ordering::SeqCst);
                *keeper.endpoint.lock().unwrap_or_else(|e| e.into_inner()) = None;
            });
        } else if !this.is_warm() && !this.calling.swap(true, Ordering::SeqCst) {
            let keeper = Arc::clone(this);
            std::thread::spawn(move || {
                let id = keeper.next_id.fetch_add(1, Ordering::SeqCst);
                if keeper.post(&warm_call(id)).is_err() {
                    keeper.calling.store(false, Ordering::SeqCst);
                }
            });
        }
    }

    fn post(&self, message: &Value) -> Result<(), String> {
        let url = self
            .endpoint
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
            .ok_or("not connected")?;
        ureq::post(&url)
            .set("Content-Type", "application/json")
            .send_string(&message.to_string())
            .map(|_| ())
            .map_err(|e| e.to_string())
    }

    /// Reads the event stream until it ends.
    fn run(&self) -> Result<(), String> {
        let response = ureq::get(&format!("{}/sse", self.base))
            .set("Accept", "text/event-stream")
            .call()
            .map_err(|e| e.to_string())?;
        let mut event = String::new();
        let mut data = String::new();
        for line in BufReader::new(response.into_reader()).lines() {
            let line = line.map_err(|e| e.to_string())?;
            if let Some(value) = line.strip_prefix("event:") {
                event = value.trim().to_owned();
            } else if let Some(value) = line.strip_prefix("data:") {
                data = value.trim().to_owned();
            } else if line.is_empty() {
                let (kind, payload) = (std::mem::take(&mut event), std::mem::take(&mut data));
                self.on_event(&kind, &payload)?;
            }
        }
        Ok(())
    }
    fn on_event(&self, kind: &str, data: &str) -> Result<(), String> {
        match kind {
            "endpoint" => {
                *self.endpoint.lock().unwrap_or_else(|e| e.into_inner()) =
                    Some(format!("{}{data}", self.base));
                self.post(
                    &json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{
                    "protocolVersion":"2024-11-05","capabilities":{},
                    "clientInfo":{"name":"monoptah-keeper","version":env!("CARGO_PKG_VERSION")}}}),
                )
            }
            "message" => {
                let message: Value = serde_json::from_str(data).unwrap_or(Value::Null);
                match message["id"].as_u64() {
                    Some(1) => {
                        self.post(&json!({"jsonrpc":"2.0","method":"notifications/initialized"}))?;
                        self.calling.store(true, Ordering::SeqCst);
                        let sent = self.post(&warm_call(2));
                        if sent.is_err() {
                            self.calling.store(false, Ordering::SeqCst);
                        }
                        sent
                    }
                    Some(_) => {
                        let ok =
                            message.get("error").is_none() && message["result"]["isError"] != true;
                        self.warm.store(ok, Ordering::SeqCst);
                        self.calling.store(false, Ordering::SeqCst);
                        Ok(())
                    }
                    None => Ok(()),
                }
            }
            _ => Ok(()),
        }
    }
}
/// A cheap call that makes the server open (or reuse) the browser connection.
fn warm_call(id: u64) -> Value {
    json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{
        "name":"browser_tabs","arguments":{"action":"list"}}})
}

/// A backend that starts no process: tests count its starts and kill its servers.
#[cfg(test)]
pub(crate) mod tests_support {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    #[derive(Default)]
    pub struct FakeBackend {
        pub starts: AtomicUsize,
        pub stops: Arc<AtomicUsize>,
        pub connects: Arc<AtomicUsize>,
        /// One flag per started server: false means the process died.
        pub alive: Mutex<Vec<Arc<AtomicBool>>>,
        /// The token each server was started with.
        pub tokens: Mutex<Vec<String>>,
    }
    struct FakeServer {
        url: String,
        alive: Arc<AtomicBool>,
        stops: Arc<AtomicUsize>,
        connects: Arc<AtomicUsize>,
    }
    impl RunningServer for FakeServer {
        fn url(&self) -> String {
            self.url.clone()
        }
        fn is_alive(&mut self) -> bool {
            self.alive.load(Ordering::SeqCst)
        }
        fn keep_connected(&mut self) {
            self.connects.fetch_add(1, Ordering::SeqCst);
        }
        fn stop(&mut self) {
            self.alive.store(false, Ordering::SeqCst);
            self.stops.fetch_add(1, Ordering::SeqCst);
        }
    }
    impl ServerBackend for FakeBackend {
        fn start(&self, launch: &ServerLaunch) -> Result<Box<dyn RunningServer>, String> {
            let n = self.starts.fetch_add(1, Ordering::SeqCst);
            let alive = Arc::new(AtomicBool::new(true));
            self.alive.lock().unwrap().push(alive.clone());
            self.tokens
                .lock()
                .unwrap()
                .push(launch.token.as_str().to_owned());
            Ok(Box::new(FakeServer {
                url: format!("http://127.0.0.1:{}/mcp", 41000 + n),
                alive,
                stops: self.stops.clone(),
                connects: self.connects.clone(),
            }))
        }
    }
}

#[cfg(test)]
mod tests;
