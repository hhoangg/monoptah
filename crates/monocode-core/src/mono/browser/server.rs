//! One long-lived `@playwright/mcp` server per Mono with a browser.
//!
//! Why: the extension makes one Chrome tab group per connection and greys the group when the
//! connection ends (`TabGroupManager.orphan`), never reusing it. A server that lived as long as one
//! agent process made a new group every Mono turn. Here the app starts the server once per Mono
//! and keeps it across turns, speaking MCP to it over its stdin and stdout. The only client is the
//! Mono's gate (see [`super::gate`]), which holds one MCP session for the server's life, so the
//! extension connection, and with it the Mono's tab group, outlives the turns. Because the server
//! has no port, no other process (the agent's shell included) can reach it and skip the gate.
//!
//! Facts and the residual risk: `docs/superpowers/specs/2026-11-22-browser-all-tabs-research.md`.
use super::gate::{self, Gate};
use super::{output, ServerSpec, TOKEN_ENV};
use crate::git_credentials::Secret;
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

/// A server nobody has asked for in this long is stopped (checked when another Mono starts its
/// own). Stopping it ends the extension connection, and the tabs of its group are left behind
/// (see [`super::gate::tabs`]), so this is long: a Mono that is used again after hours still
/// finds its Teams or Slack tab open and owned.
pub const IDLE_TIMEOUT: Duration = Duration::from_secs(12 * 60 * 60);

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
    /// Names this server instance. A new instance has a new name; the gate starts a new MCP
    /// session when it changes.
    fn url(&self) -> String;
    /// How the gate reaches this server, when it is not the default HTTP client.
    fn upstream(&self) -> Option<Arc<dyn gate::Upstream>> {
        None
    }
    /// The process is still running.
    fn is_alive(&mut self) -> bool;
    /// Nothing to do for a stdio server: the gate's own session keeps the browser connection open.
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
    /// One gate per Mono (see [`gate`]): the agent talks to it, never to the server directly.
    gates: Mutex<HashMap<String, Arc<Gate>>>,
    /// How a gate reaches the server; tests replace it.
    upstream: Arc<dyn gate::Upstream>,
    /// Why the last launch of a Mono could not attach its browser (no secrets). The Browser tab
    /// shows it; a launch that attached clears it.
    attach_errors: Mutex<HashMap<String, String>>,
}
impl BrowserServers {
    pub fn new(backend: Arc<dyn ServerBackend>, idle: Duration) -> Self {
        Self {
            backend,
            idle,
            entries: Mutex::new(HashMap::new()),
            starting: Mutex::new(HashMap::new()),
            gates: Mutex::new(HashMap::new()),
            upstream: Arc::new(gate::HttpUpstream::new()),
            attach_errors: Mutex::new(HashMap::new()),
        }
    }
    pub fn set_attach_error(&self, mono_id: &str, reason: Option<String>) {
        let mut errors = self.attach_errors.lock().unwrap_or_else(|e| e.into_inner());
        match reason {
            Some(reason) => errors.insert(mono_id.to_owned(), reason),
            None => errors.remove(mono_id),
        };
    }
    /// Why the Mono's last launch could not attach its browser, if it could not.
    pub fn attach_error(&self, mono_id: &str) -> Option<String> {
        self.attach_errors
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(mono_id)
            .cloned()
    }
    /// Uses `upstream` instead of real HTTP to reach the servers (tests).
    pub fn with_upstream(mut self, upstream: Arc<dyn gate::Upstream>) -> Self {
        self.upstream = upstream;
        self
    }
    /// This Mono's gate in front of the server at `upstream_url`: the one it has, updated to the
    /// current policy and server address, or a new one. Its address stays the same while the
    /// Mono's browser stays on, so a parked agent process keeps a working URL.
    pub fn gate_for(
        &self,
        mono_id: &str,
        upstream_url: &str,
        policy: gate::Policy,
    ) -> Result<Arc<Gate>, String> {
        let upstream = self
            .lock()
            .get(mono_id)
            .and_then(|entry| entry.server.upstream())
            .unwrap_or_else(|| self.upstream.clone());
        let mut gates = self.gates.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(gate) = gates.get(mono_id) {
            gate.configure(policy, upstream_url, upstream);
            return Ok(gate.clone());
        }
        let gate = Gate::start(upstream, policy, upstream_url)?;
        gates.insert(mono_id.to_owned(), gate.clone());
        Ok(gate)
    }
    /// The Mono's gate, if its browser has been started in this app session.
    pub fn gate(&self, mono_id: &str) -> Option<Arc<Gate>> {
        self.gates
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(mono_id)
            .cloned()
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

    /// Gives this Mono's gate, if it has one, a new policy at once (the user changed the trust
    /// level or the blocked origins while a run may be using it).
    pub fn update_policy(&self, mono_id: &str, policy: gate::Policy) {
        if let Some(gate) = self
            .gates
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(mono_id)
        {
            gate.set_policy(policy);
        }
    }

    /// Stops this Mono's server (its browser was turned off or changed, or the Mono is gone).
    pub fn stop(&self, mono_id: &str) {
        let gate = self
            .gates
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(mono_id);
        if let Some(gate) = gate {
            gate.clear_output();
        }
        let entry = self.lock().remove(mono_id);
        stop_all(entry.map(|e| e.server));
    }
    /// Stops every server (the app quits).
    pub fn stop_everything(&self) {
        let gates: Vec<Arc<Gate>> = self
            .gates
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .drain()
            .map(|(_, gate)| gate)
            .collect();
        for gate in gates {
            gate.clear_output();
        }
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

/// Starts `@playwright/mcp` as a child process that speaks MCP over its stdin and stdout. There
/// is no port at all, so nothing but the gate can reach the server: no process on the machine,
/// the agent's shell included, can connect to it.
pub struct NodeBackend;
impl ServerBackend for NodeBackend {
    fn start(&self, launch: &ServerLaunch) -> Result<Box<dyn RunningServer>, String> {
        // The server runs in its private output folder; without one it does not start (it would
        // write page files into whatever directory the app has).
        let output = launch
            .spec
            .output_dir()
            .ok_or("The browser server has no output folder.")?;
        output::prepare(&output).map_err(|_| "Could not prepare the browser output folder.")?;
        let mut command = std::process::Command::new(&launch.spec.command);
        command
            .args(&launch.spec.args)
            .env("PATH", launch.search_path)
            .env(TOKEN_ENV, launch.token.as_str())
            // The server's working directory is its (private) output folder: a relative file name
            // it is given lands there, never in the app's own working directory.
            .current_dir(&output)
            // The server exits when its stdin closes: if the app dies, so does the server.
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null());
        crate::process::isolate(&mut command);
        let mut child = command
            .spawn()
            .map_err(|_| "Could not start npx. Check that Node.js is installed.".to_owned())?;
        // The token is in the environment only, so the argv record the guard writes is safe.
        let guard = crate::process::guard::arm(&child, &command);
        let (Some(stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else {
            crate::process::terminate(&mut child);
            return Err("Could not talk to the browser server.".into());
        };
        Ok(Box::new(NodeServer {
            key: format!("stdio:{}", child.id()),
            upstream: gate::StdioUpstream::new(stdin, stdout),
            child,
            guard: Some(guard),
        }))
    }
}

struct NodeServer {
    child: std::process::Child,
    guard: Option<crate::process::guard::ChildGuard>,
    upstream: Arc<gate::StdioUpstream>,
    key: String,
}
impl RunningServer for NodeServer {
    fn url(&self) -> String {
        self.key.clone()
    }
    fn is_alive(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None)) && self.upstream.is_open()
    }
    fn upstream(&self) -> Option<Arc<dyn gate::Upstream>> {
        Some(self.upstream.clone())
    }
    fn keep_connected(&mut self) {}
    fn stop(&mut self) {
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

/// A backend that starts no process: tests count its starts and kill its servers.
#[cfg(test)]
pub(crate) mod tests_support {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicUsize};

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
