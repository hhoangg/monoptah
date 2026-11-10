//! Remote host connections. The renderer receives machine metadata, never the
//! saved bearer credential. Workspace requests never fall back to local calls.
pub mod attachments;
mod cloud_upgrade;
#[cfg(test)]
mod cloud_upgrade_tests;
pub mod host_info;
pub mod host_release;
pub mod host_upgrade;
pub mod protocol;
mod ssh;
pub mod ssh_askpass;
pub mod uploads;

pub use cloud_upgrade::HOST_UPDATING;
pub(crate) use ssh::{
    sandbox_bootstrap_script, shell_quote, validate_sandbox as validate_sandbox_target,
    validate_sandbox_workspace,
};
use ssh::{Job, Tunnel, Tunnels};
pub use ssh::{JobView, Prompt, SandboxMode, SandboxTarget, SshTarget};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock, RwLock};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

pub struct Connections {
    data_dir: PathBuf,
    store: Mutex<()>,
    tunnels: Tunnels,
    jobs: Mutex<HashMap<String, Arc<Job>>>,
    /// In-flight codespace wakes by machine id, so concurrent callers share one.
    wakes: Mutex<HashMap<String, WakeSlot>>,
    /// Flavor, version and capabilities of each machine's Host, from `environment.describe`.
    host_info: host_info::HostInfoCache,
    /// Per machine: the turn-starting requests in flight, and the hold during a Host swap.
    gates: Mutex<HashMap<String, Arc<cloud_upgrade::SendGate>>>,
    /// Ids of the jobs whose Host swap is past the point of no return (an app quit leaves them).
    swap_jobs: Mutex<std::collections::HashSet<String>>,
    #[cfg(test)]
    slot_wait_override: Mutex<Option<std::time::Duration>>,
    #[cfg(test)]
    drain_override: Mutex<Option<std::time::Duration>>,
    /// Machine id to the tunnel its last Host upgrade pass ran on (once per connect).
    upgrade_passes: Mutex<HashMap<String, u64>>,
    /// How the Host upgrade reaches a machine (replaced in tests).
    runner: Mutex<Arc<dyn cloud_upgrade::CloudRunner>>,
}

type WakeSlot = Arc<(Mutex<Option<Result<(), String>>>, Condvar)>;
const SSH_BUSY: &str = "Another SSH connection is being set up. Finish or cancel it first.";
/// How long a wake waits for the single SSH job slot before giving up.
const JOB_SLOT_WAIT: std::time::Duration = std::time::Duration::from_secs(15 * 60);
/// Error returned when a wake was cancelled by its caller.
pub const WAKE_CANCELLED: &str = "Wake cancelled";

impl Connections {
    /// Uses the same connection store as the Tauri desktop.
    pub fn new(data_dir: impl AsRef<Path>) -> Self {
        Self {
            data_dir: data_dir.as_ref().to_owned(),
            store: Mutex::new(()),
            tunnels: Tunnels::default(),
            jobs: Mutex::new(HashMap::new()),
            wakes: Mutex::new(HashMap::new()),
            host_info: host_info::HostInfoCache::default(),
            gates: Mutex::new(HashMap::new()),
            swap_jobs: Mutex::new(Default::default()),
            #[cfg(test)]
            slot_wait_override: Mutex::new(None),
            #[cfg(test)]
            drain_override: Mutex::new(None),
            upgrade_passes: Mutex::new(HashMap::new()),
            runner: Mutex::new(Arc::new(cloud_upgrade::SshRunner)),
        }
    }

    fn store_path(&self) -> PathBuf {
        self.data_dir.join("remote-machines.json")
    }

    pub fn machine_for_environment(&self, environment_id: &str) -> Result<Machine, String> {
        self.list()?
            .into_iter()
            .find(|machine| machine.environment_id == environment_id)
            .ok_or_else(|| "Machine is no longer connected".into())
    }

    pub fn shutdown(&self) {
        // A Host swap that is past the point of no return finishes on the machine by itself;
        // cancelling its job would only cut the app's view of it. Wait for it (bounded).
        let protected = self
            .swap_jobs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        if let Ok(jobs) = self.jobs.lock() {
            for (id, job) in jobs.iter() {
                if !protected.contains(id) {
                    job.cancel();
                }
            }
        }
        if !protected.is_empty() {
            self.wait_for_swaps(&protected);
        }
        self.tunnels.clear();
    }
    fn job(&self, id: &str) -> Result<Arc<Job>, String> {
        self.jobs
            .lock()
            .map_err(|_| "Connection setup is unavailable")?
            .get(id)
            .cloned()
            .ok_or_else(|| "Connection setup has expired".into())
    }
}

impl std::fmt::Debug for Connections {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Connections")
            .field("data_dir", &self.data_dir)
            .finish_non_exhaustive()
    }
}

impl Drop for Connections {
    fn drop(&mut self) {
        self.shutdown();
    }
}

static DEFAULT_CONNECTIONS: OnceLock<RwLock<Option<Arc<Connections>>>> = OnceLock::new();

/// Installs the shared transport used by path-based workspace services.
pub fn set_default_connections(connections: Arc<Connections>) {
    let slot = DEFAULT_CONNECTIONS.get_or_init(|| RwLock::new(None));
    *slot
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(connections);
}

pub fn default_connections() -> Result<Arc<Connections>, String> {
    DEFAULT_CONNECTIONS
        .get_or_init(|| RwLock::new(None))
        .read()
        .map_err(|_| "Remote connections are unavailable")?
        .clone()
        .ok_or_else(|| "Remote connections are unavailable".into())
}

pub const REMOTE_PATH_PREFIX: &str = "remote://";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemotePath {
    pub environment_id: String,
    pub host_path: String,
}

/// Qualifies a host path without losing Windows drive letters or UNC roots.
pub fn remote_path(environment_id: &str, host_path: &str) -> String {
    let path = host_path.replace('\\', "/");
    let path = if path.starts_with("//") {
        &path[1..]
    } else {
        path.trim_start_matches('/')
    };
    format!("{REMOTE_PATH_PREFIX}{environment_id}/{path}")
}

pub fn parse_remote_path(path: &str) -> Option<RemotePath> {
    let path = path.strip_prefix(REMOTE_PATH_PREFIX)?.replace('\\', "/");
    let (environment_id, host_path) = path.split_once('/')?;
    if environment_id.is_empty() {
        return None;
    }
    let bytes = host_path.as_bytes();
    let drive = bytes.len() >= 2
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && (bytes.len() == 2 || bytes[2] == b'/');
    Some(RemotePath {
        environment_id: environment_id.into(),
        host_path: if drive {
            host_path.into()
        } else {
            format!("/{host_path}")
        },
    })
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct StoredMachine {
    id: String,
    name: String,
    endpoint: String,
    environment_id: String,
    token: String,
    #[serde(default)]
    ssh: Option<SshTarget>,
    /// A cloud Host is older than the pin but a session was running, so the two-phase
    /// upgrade was deferred; the next connect that finds the Host idle upgrades it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pending_host_upgrade: bool,
    /// A Host swap was started on the machine and the app never saw it end (it quit, or the
    /// ssh dropped). The next upgrade pass asks the machine how it ended.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    swap_unconfirmed: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Machine {
    pub id: String,
    pub name: String,
    pub endpoint: String,
    pub environment_id: String,
    pub ssh: Option<SshTarget>,
    /// Codespace name when this machine is reached through `gh codespace ssh`.
    #[serde(default)]
    pub codespace: Option<String>,
    /// Docker Sandbox when this machine is reached through `<id>@sbx_cloud`.
    #[serde(default)]
    pub sandbox: Option<SandboxTarget>,
}

impl StoredMachine {
    fn public(&self) -> Machine {
        Machine {
            id: self.id.clone(),
            name: self.name.clone(),
            endpoint: self.endpoint.clone(),
            environment_id: self.environment_id.clone(),
            ssh: self.ssh.clone(),
            codespace: self.ssh.as_ref().and_then(|ssh| ssh.codespace.clone()),
            sandbox: self.ssh.as_ref().and_then(|ssh| ssh.sandbox.clone()),
        }
    }
}

fn endpoint(value: &str) -> Result<String, String> {
    let url = url::Url::parse(value.trim()).map_err(|_| "Enter a valid host URL")?;
    let host = url.host_str().ok_or("Host URL has no hostname")?;
    let loopback = host == "localhost"
        || host
            .trim_matches(['[', ']'])
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback());
    if !(url.scheme() == "https" || (url.scheme() == "http" && loopback)) {
        return Err("Use HTTPS, or an HTTP loopback address forwarded through SSH".into());
    }
    if !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path() != "/"
    {
        return Err("Host URL must contain only the scheme, hostname, and port".into());
    }
    Ok(url.as_str().trim_end_matches('/').to_string())
}

fn read(path: &Path) -> Result<Vec<StoredMachine>, String> {
    match std::fs::read(path) {
        Ok(bytes) => {
            serde_json::from_slice(&bytes).map_err(|_| "Remote connection store is invalid".into())
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(error.to_string()),
    }
}

fn write(path: &Path, machines: &[StoredMachine]) -> Result<(), String> {
    let parent = path.parent().ok_or("Missing connection directory")?;
    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let temporary = parent.join(format!("remote-machines-{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| -> Result<(), String> {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temporary).map_err(|e| e.to_string())?;
        file.write_all(&serde_json::to_vec(machines).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
        file.sync_all().map_err(|e| e.to_string())?;
        std::fs::rename(&temporary, path).map_err(|e| e.to_string())?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(temporary);
    }
    result
}

/// Hosts split large session syncs into pieces below this cap
/// (`host/sync-transfer.ts`), so it bounds memory without limiting transcripts.
const MAX_RESPONSE_BYTES: u64 = 16 * 1024 * 1024;

fn rpc(
    endpoint: &str,
    token: &str,
    environment_id: Option<&str>,
    method: &str,
    params: Value,
) -> Result<Value, String> {
    rpc_with(endpoint, token, environment_id, method, params, None, false)
}

/// Total time any ordinary RPC may take.
const DEFAULT_RPC_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// Total time a push or pull may take. The Host stops git after 90 s, so this leaves
/// room for its own error reply to arrive first.
pub const GIT_REMOTE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);

/// Total time a sync may take. The Host runs a pull and then a push, each stopped at
/// 90 s, so it can legitimately take about 180 s; this adds a margin.
pub const GIT_SYNC_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(200);

/// The result of a long operation that timed out on our side. The host may still be
/// running it, so callers must re-read state instead of reporting a failure.
pub const MAY_STILL_RUN: &str = "The operation may still be running on the host. Checking...";

/// `closed_is_dead`: a connection reset or closed without a reply also means
/// the route is gone. Behind a codespace tunnel that is what a stopped host
/// looks like: the local forward accepts, then the remote side hangs up.
///
/// `timeout`: `None` is the ordinary 30 s limit. `Some(limit)` is for operations that may
/// outlive the client (push, pull, sync): hitting it returns `MAY_STILL_RUN`, and the
/// connection is dropped, so a reply that arrives later is never read.
fn rpc_with(
    endpoint: &str,
    token: &str,
    environment_id: Option<&str>,
    method: &str,
    params: Value,
    timeout: Option<std::time::Duration>,
    closed_is_dead: bool,
) -> Result<Value, String> {
    rpc_with_default(
        endpoint,
        token,
        environment_id,
        method,
        params,
        timeout,
        closed_is_dead,
        DEFAULT_RPC_TIMEOUT,
    )
}

/// `rpc_with` with the ordinary limit as a parameter, so tests need not wait 30 s.
#[allow(clippy::too_many_arguments)]
fn rpc_with_default(
    endpoint: &str,
    token: &str,
    environment_id: Option<&str>,
    method: &str,
    params: Value,
    timeout: Option<std::time::Duration>,
    closed_is_dead: bool,
    default_timeout: std::time::Duration,
) -> Result<Value, String> {
    let agent = ureq::AgentBuilder::new()
        .redirects(0)
        .timeout_connect(std::time::Duration::from_secs(5))
        .timeout(timeout.unwrap_or(default_timeout))
        .build();
    let payload = json!({ "version": 1, "environmentId": environment_id, "method": method, "params": params });
    let response = agent
        .post(&format!("{endpoint}/rpc"))
        .set("Authorization", &format!("Bearer {token}"))
        .set("Content-Type", "application/json")
        .send_string(&payload.to_string());
    let response = match response {
        Ok(response) => response,
        Err(ureq::Error::Status(_, response)) => response,
        Err(ureq::Error::Transport(error))
            if connection_refused(&error) || (closed_is_dead && connection_closed(&error)) =>
        {
            return Err(
                "Machine is unreachable. Check the host and SSH tunnel, then reconnect.".into(),
            )
        }
        Err(ureq::Error::Transport(error)) if timeout.is_some() && timed_out(&error) => {
            return Err(MAY_STILL_RUN.into())
        }
        Err(_) => {
            return Err("The host request did not complete. Retry to confirm its result.".into())
        }
    };
    let status = response.status();
    let mut bytes = Vec::new();
    response
        .into_reader()
        .take(MAX_RESPONSE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| {
            if timeout.is_some() && e.kind() == std::io::ErrorKind::TimedOut {
                MAY_STILL_RUN.to_string()
            } else {
                e.to_string()
            }
        })?;
    if bytes.len() as u64 > MAX_RESPONSE_BYTES {
        return Err("Host response is too large".into());
    }
    let value: Value = serde_json::from_slice(&bytes).map_err(|_| "Invalid host response")?;
    if let Some(error) = value.get("error").and_then(Value::as_str) {
        return Err(format!("Host rejected request: {error}"));
    }
    if status != 200 {
        return Err(format!("Host returned HTTP {status}"));
    }
    value
        .get("result")
        .cloned()
        .ok_or_else(|| "Invalid host response".into())
}

// Only a refused connection proves the local forwarding listener is gone.
// A slow request or an interrupted response says nothing about SSH health.
fn connection_closed(error: &ureq::Transport) -> bool {
    use std::io::ErrorKind::*;
    let mut source: Option<&(dyn std::error::Error + 'static)> = Some(error);
    while let Some(error) = source {
        if error.downcast_ref::<std::io::Error>().is_some_and(|e| {
            matches!(
                e.kind(),
                ConnectionReset | ConnectionAborted | BrokenPipe | UnexpectedEof
            )
        }) {
            return true;
        }
        source = error.source();
    }
    false
}

/// Whether the request may have reached the host. A connect failure (including a connect
/// timeout, whose source is also an `io::ErrorKind::TimedOut`) means nothing was sent.
fn may_have_been_sent(kind: ureq::ErrorKind) -> bool {
    kind == ureq::ErrorKind::Io
}

fn timed_out(error: &ureq::Transport) -> bool {
    if !may_have_been_sent(error.kind()) {
        return false;
    }
    let mut source: Option<&(dyn std::error::Error + 'static)> = Some(error);
    while let Some(error) = source {
        if error
            .downcast_ref::<std::io::Error>()
            .is_some_and(|e| e.kind() == std::io::ErrorKind::TimedOut)
        {
            return true;
        }
        source = error.source();
    }
    false
}

fn connection_refused(error: &ureq::Transport) -> bool {
    let mut source: Option<&(dyn std::error::Error + 'static)> = Some(error);
    while let Some(error) = source {
        if error
            .downcast_ref::<std::io::Error>()
            .is_some_and(|e| e.kind() == std::io::ErrorKind::ConnectionRefused)
        {
            return true;
        }
        source = error.source();
    }
    false
}

impl Connections {
    pub fn list(&self) -> Result<Vec<Machine>, String> {
        let _guard = self
            .store
            .lock()
            .map_err(|_| "Connection store is locked")?;
        Ok(read(&self.store_path())?
            .iter()
            .map(StoredMachine::public)
            .collect())
    }

    pub fn connect(&self, name: String, url: String, token: String) -> Result<Machine, String> {
        let endpoint = endpoint(&url)?;
        let token = token.trim().to_string();
        if token.len() != 43
            || !token
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        {
            return Err("Enter the device token issued by monocode-host pair".into());
        }
        let descriptor = rpc(
            &endpoint,
            &token,
            None,
            "environment.describe",
            json!({"supportedProviders":protocol::supported_providers()}),
        )?;
        if descriptor.get("protocolVersion").and_then(Value::as_u64) != Some(1) {
            return Err("This host uses an incompatible protocol version".into());
        }
        let environment_id = descriptor
            .get("environmentId")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .ok_or("Host identity is missing")?
            .to_string();
        let name = if name.trim().is_empty() {
            descriptor
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("Remote machine")
                .to_string()
        } else {
            name.trim().chars().take(100).collect()
        };
        let _guard = self
            .store
            .lock()
            .map_err(|_| "Connection store is locked")?;
        let path = self.store_path();
        let mut machines = read(&path)?;
        let existing = machines.iter().find(|m| m.environment_id == environment_id);
        let id = existing
            .map(|m| m.id.clone())
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        let machine = StoredMachine {
            id: id.clone(),
            name,
            endpoint,
            environment_id,
            token,
            ssh: existing.and_then(|m| m.ssh.clone()),
            pending_host_upgrade: false,
            swap_unconfirmed: false,
        };
        machines.retain(|m| m.id != id);
        machines.push(machine.clone());
        write(&path, &machines)?;
        self.host_info.record(&id, &descriptor);
        Ok(machine.public())
    }

    pub fn disconnect(&self, machine_id: &str) -> Result<(), String> {
        let _guard = self
            .store
            .lock()
            .map_err(|_| "Connection store is locked")?;
        let path = self.store_path();
        let mut machines = read(&path)?;
        machines.retain(|m| m.id != machine_id);
        write(&path, &machines)?;
        self.tunnels.remove(machine_id);
        self.host_info.drop_machine(machine_id);
        Ok(())
    }

    /// What the last `environment.describe` of this machine reported, if any.
    /// A machine reached through a tunnel with no live tunnel has no answer: the cached
    /// entry may describe a Host that is gone, so it is dropped.
    pub fn host_info(&self, machine_id: &str) -> Option<host_info::HostInfo> {
        let through_tunnel = self
            .list()
            .ok()?
            .into_iter()
            .any(|m| m.id == machine_id && m.ssh.is_some());
        if through_tunnel && self.tunnels.existing(machine_id).is_none() {
            self.host_info.drop_machine(machine_id);
            return None;
        }
        self.host_info.get(machine_id)
    }

    /// Forgets what is cached about this machine's Host.
    pub(crate) fn forget_host(&self, machine_id: &str) {
        self.host_info.drop_machine(machine_id);
    }

    /// Describes the Host now (refreshing the cache) and returns what it reports.
    pub fn describe_host(&self, machine_id: &str) -> Result<host_info::HostInfo, String> {
        self.request(machine_id, "environment.describe", json!({}))?;
        self.host_info
            .get(machine_id)
            .ok_or_else(|| "Machine is no longer connected".into())
    }

    pub fn request(&self, machine_id: &str, method: &str, params: Value) -> Result<Value, String> {
        self.request_timeout(machine_id, method, params, None)
    }

    /// Like `request` with a per-request limit for long operations; see `rpc_with`.
    /// Never wakes a workspace: cloud machines still need a live tunnel.
    pub fn request_timeout(
        &self,
        machine_id: &str,
        method: &str,
        mut params: Value,
        timeout: Option<std::time::Duration>,
    ) -> Result<Value, String> {
        if method == "environment.describe" {
            let values = params
                .as_object_mut()
                .ok_or("Invalid host descriptor parameters")?;
            values
                .entry("supportedProviders")
                .or_insert_with(|| json!(protocol::supported_providers()));
        }
        if !supported_remote_method(method) {
            return Err("Unsupported remote operation".into());
        }
        // A request that starts a turn holds its place for the WHOLE call, so a Host swap that
        // waits for the places to empty knows that none of them can still reach the Host.
        let _in_flight = if cloud_upgrade::starts_turn(method) {
            Some(self.gate(machine_id).enter()?)
        } else {
            None
        };
        let machine = {
            let _guard = self
                .store
                .lock()
                .map_err(|_| "Connection store is locked")?;
            read(&self.store_path())?
                .into_iter()
                .find(|m| m.id == machine_id)
                .ok_or("Machine is no longer connected")?
        };
        let cloud = machine.ssh.as_ref().is_some_and(SshTarget::is_cloud);
        let not_connected = if machine.ssh.as_ref().is_some_and(|t| t.sandbox.is_some()) {
            SANDBOX_NOT_CONNECTED
        } else {
            CODESPACE_NOT_CONNECTED
        };
        let tunnel_lease = if let Some(target) = &machine.ssh {
            if cloud {
                // Background work must never wake a codespace (that is what
                // `gh codespace ssh` does) or resume a sandbox (any ssh does),
                // so only a live tunnel is used.
                Some(self.tunnels.existing(&machine.id).ok_or(not_connected)?)
            } else {
                Some(self.tunnels.endpoint(&machine.id, target)?)
            }
        } else {
            None
        };
        // A tunnel that was just (re)started may reach a different Host than the one that
        // was described; learn it again.
        if tunnel_lease.as_ref().is_some_and(|lease| lease.fresh) {
            self.host_info.drop_machine(&machine.id);
        }
        let endpoint = tunnel_lease
            .as_ref()
            .map(|lease| lease.endpoint.as_str())
            .unwrap_or(&machine.endpoint);
        let response = rpc_with(
            endpoint,
            &machine.token,
            Some(&machine.environment_id),
            method,
            params,
            timeout,
            cloud,
        );
        if response
            .as_ref()
            .err()
            .is_some_and(|error| error.starts_with("Machine is unreachable"))
        {
            if let Some(lease) = &tunnel_lease {
                self.tunnels.invalidate(&machine.id, lease);
            }
            // The Host behind an unreachable tunnel may be replaced; learn it again.
            self.host_info.drop_machine(&machine.id);
            if cloud {
                return Err(not_connected.into());
            }
        }
        let result = response?;
        if method == "environment.describe"
            && result.get("environmentId").and_then(Value::as_str)
                != Some(machine.environment_id.as_str())
        {
            return Err("Host identity changed. Add this machine again before continuing.".into());
        }
        if method == "environment.describe" {
            self.host_info.record(&machine.id, &result);
        }
        Ok(result)
    }
}

/// Returned instead of waking a codespace. Callers that act on a user's
/// request answer it with `ensure_codespace`/`codespace_reconnect`.
pub const CODESPACE_NOT_CONNECTED: &str =
    "Codespace is not connected. Reconnect to start it and restore the connection.";

/// Same as `CODESPACE_NOT_CONNECTED` for a Docker Sandbox, which any ssh would
/// resume (and bill).
pub const SANDBOX_NOT_CONNECTED: &str =
    "Sandbox is not connected. Reconnect to start it and restore the connection.";

fn supported_remote_method(method: &str) -> bool {
    matches!(
        method,
        "environment.describe"
            | "projects.list"
            | "projects.browse"
            | "projects.open"
            | "models.list"
            | "sessions.list"
            | "sessions.update"
            | "sessions.delete"
            | "sessions.sync"
            | "sessions.syncChunk"
            | "commands.dispatch"
            | "attachments.upload"
            | "attachments.read"
            | "uploads.begin"
            | "uploads.chunk"
            | "uploads.commit"
            | "uploads.discard"
            | "devices.revokeSelf"
            | "git.diff"
            | "git.branches"
            | "git.switch"
            | "git.createBranch"
            | "git.worktrees"
            | "git.worktreeCreate"
            | "files.read"
            | "files.readBytes"
            | "files.list"
            | "files.index"
            | "workspace.run"
            | "files.search"
            | "files.searchContent"
            | "files.create"
            | "files.write"
            | "git.index"
            | "git.fileDiff"
            | "git.action"
    )
}

/// Bootstrap script for a target: cloud targets track the token and install
/// Claude; a sandbox has no per-start secret prelude yet.
fn cloud_or_plain_bootstrap(target: &SshTarget, platform: ssh::HostPlatform) -> String {
    match (&target.codespace, &target.sandbox) {
        (Some(_), _) => ssh::codespace_bootstrap_script(),
        (None, Some(_)) => ssh::sandbox_bootstrap_script(None),
        (None, None) => ssh::bootstrap_script(platform),
    }
}

impl Connections {
    fn start_ssh_job(
        self: &Arc<Self>,
        target: SshTarget,
        name: String,
        existing: Option<StoredMachine>,
        upgrade: bool,
    ) -> Result<String, String> {
        ssh::check_target(&target)?;
        let job = Job::new();
        let id = job.view().id;
        {
            let mut jobs = self
                .jobs
                .lock()
                .map_err(|_| "Connection setup is unavailable")?;
            if jobs.values().any(|job| !job.view().done) {
                return Err(SSH_BUSY.into());
            }
            jobs.retain(|_, job| !job.view().done);
            jobs.insert(id.clone(), job.clone());
        }
        let state = self.clone();
        std::thread::spawn(move || {
            let prepared = (|| -> Result<(StoredMachine, Tunnel, Value), String> {
                let askpass = job.askpass()?;
                let mut target = target;
                let cloud = target.is_cloud();
                let mut machine = if let Some(mut existing) = existing {
                    if upgrade || cloud {
                        // A codespace or sandbox may have stopped, which also stopped its
                        // host. gh starts it on connect, and the bootstrap's
                        // service install brings the host back with its saved
                        // state, so a codespace reconnect always re-runs it.
                        let platform = ssh::detect_platform(&target, &job, &askpass)?;
                        job.message(if upgrade {
                            "Updating Monoptah Host on the machine…"
                        } else if target.sandbox.is_some() {
                            "Starting Monoptah Host in the sandbox…"
                        } else {
                            "Starting Monoptah Host on the codespace…"
                        });
                        let script = if upgrade {
                            ssh::upgrade_script(platform, target.remote_port)
                        } else {
                            cloud_or_plain_bootstrap(&target, platform)
                        };
                        let output = ssh::run_script(&target, platform, script, &job, &askpass)?;
                        let info: Value = serde_json::from_str(output.lines().last().unwrap_or(""))
                            .map_err(|_| "Host update returned an invalid response")?;
                        target.remote_port = info
                            .get("port")
                            .and_then(Value::as_u64)
                            .and_then(|v| u16::try_from(v).ok())
                            .filter(|p| *p > 0)
                            .ok_or("Host did not report a valid port")?;
                        existing.ssh = Some(target.clone());
                    } else {
                        job.message("Reconnecting to the machine…");
                    }
                    existing
                } else {
                    let platform = ssh::detect_platform(&target, &job, &askpass)?;
                    job.message("Installing or starting Monoptah Host…");
                    let output = ssh::run_script(
                        &target,
                        platform,
                        cloud_or_plain_bootstrap(&target, platform),
                        &job,
                        &askpass,
                    )?;
                    let info: Value = serde_json::from_str(output.lines().last().unwrap_or(""))
                        .map_err(|_| "Host setup returned an invalid response")?;
                    target.remote_port = info
                        .get("port")
                        .and_then(Value::as_u64)
                        .and_then(|v| u16::try_from(v).ok())
                        .filter(|p| *p > 0)
                        .ok_or("Host did not report a valid port")?;
                    job.message("Pairing this desktop with the host…");
                    let pair = ssh::run_script(
                        &target,
                        platform,
                        ssh::pairing_script(platform, &ssh::device_name()),
                        &job,
                        &askpass,
                    )?;
                    let pair: Value = serde_json::from_str(pair.lines().last().unwrap_or(""))
                        .map_err(|_| "Host pairing returned an invalid response")?;
                    let token = pair
                        .get("token")
                        .and_then(Value::as_str)
                        .filter(|s| {
                            s.len() == 43
                                && s.bytes()
                                    .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
                        })
                        .ok_or("Host returned an invalid device credential")?
                        .to_string();
                    let environment_id = pair
                        .get("environmentId")
                        .and_then(Value::as_str)
                        .filter(|s| !s.is_empty())
                        .ok_or("Host identity is missing")?
                        .to_string();
                    StoredMachine {
                        id: uuid::Uuid::new_v4().to_string(),
                        name,
                        endpoint: match (&target.codespace, &target.sandbox) {
                            (Some(name), _) => format!("codespace://{name}"),
                            (None, Some(sandbox)) => format!("sbx://{}", sandbox.name),
                            (None, None) => format!("ssh://{}", target.target),
                        },
                        environment_id,
                        token,
                        ssh: Some(target.clone()),
                        pending_host_upgrade: false,
                        swap_unconfirmed: false,
                    }
                };
                job.message("Opening the secure connection…");
                let tunnel = Tunnel::start(&target, Some(&job), Some(&askpass))?;
                let descriptor = rpc(
                    &format!("http://127.0.0.1:{}", tunnel.port),
                    &machine.token,
                    Some(&machine.environment_id),
                    "environment.describe",
                    json!({"supportedProviders":protocol::supported_providers()}),
                )?;
                if descriptor.get("protocolVersion").and_then(Value::as_u64) != Some(1) {
                    return Err("This host uses an incompatible protocol version".into());
                }
                if descriptor.get("environmentId").and_then(Value::as_str)
                    != Some(&machine.environment_id)
                {
                    return Err(
                        "Host identity changed. Remove this connection and add the machine again."
                            .into(),
                    );
                }
                if upgrade {
                    let info = host_info::HostInfo::from_descriptor(&descriptor);
                    let supports = |name: &str| info.supports(name);
                    if !supports("git.index")
                        || !supports("files.list")
                        || !supports("files.create")
                        || !supports("files.searchContent")
                    {
                        return Err("The installed host package still lacks Explorer and Changes. Install a newer Monoptah release with updated host packages.".into());
                    }
                }
                if machine.name.trim().is_empty() {
                    machine.name = descriptor
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or(&target.target)
                        .chars()
                        .take(100)
                        .collect();
                }
                Ok((machine, tunnel, descriptor))
            })();
            job.complete(|| {
                let (machine, tunnel, descriptor) = prepared?;
                let machine = state.commit_machine(machine, &descriptor)?;
                state.tunnels.insert(machine.id.clone(), tunnel);
                Ok(machine.public())
            });
        });
        Ok(id)
    }

    /// Saves a machine set up over SSH. An already saved machine with the same environment
    /// keeps its id, and the Host info is recorded under that final id only: the id the job
    /// started with is provisional and must leave nothing behind.
    fn commit_machine(
        &self,
        mut machine: StoredMachine,
        descriptor: &Value,
    ) -> Result<StoredMachine, String> {
        let _guard = self
            .store
            .lock()
            .map_err(|_| "Connection store is locked")?;
        let path = self.store_path();
        let mut machines = read(&path)?;
        let provisional = machine.id.clone();
        if let Some(old) = machines
            .iter()
            .find(|m| m.environment_id == machine.environment_id)
        {
            machine.id = old.id.clone();
            machine.pending_host_upgrade |= old.pending_host_upgrade;
        }
        machines.retain(|m| m.id != machine.id);
        machines.push(machine.clone());
        write(&path, &machines)?;
        self.host_info.drop_machine(&provisional);
        self.host_info.record(&machine.id, descriptor);
        Ok(machine)
    }

    pub fn ssh_begin(
        self: &Arc<Self>,
        target: String,
        name: String,
        port: Option<u16>,
    ) -> Result<String, String> {
        let target = ssh::validate_target(&target, port)?;
        self.start_ssh_job(
            SshTarget {
                target,
                port,
                remote_port: 3774,
                codespace: None,
                sandbox: None,
            },
            name.trim().chars().take(100).collect(),
            None,
            false,
        )
    }

    /// Pair with the monocode-host inside a GitHub Codespace over
    /// `gh codespace ssh`. Poll the returned job with `ssh_poll`.
    pub fn codespace_begin(
        self: &Arc<Self>,
        name: &str,
        display_name: Option<&str>,
    ) -> Result<String, String> {
        let name = ssh::validate_codespace(name)?;
        let display = display_name
            .map(|d| d.trim().chars().take(100).collect::<String>())
            .filter(|d| !d.is_empty())
            .unwrap_or_else(|| name.clone());
        self.start_ssh_job(
            SshTarget {
                target: name.clone(),
                port: None,
                remote_port: 3774,
                codespace: Some(name),
                sandbox: None,
            },
            display,
            None,
            false,
        )
    }

    /// Pair with the monocode-host inside a Docker Sandbox's devcontainer over
    /// `ssh <id>@sbx_cloud`. The relay on `sandbox.relay_port` must be running
    /// by the time the tunnel opens (the sandbox start flow sets it up). Poll
    /// the returned job with `ssh_poll`.
    pub fn sandbox_begin(
        self: &Arc<Self>,
        sandbox: SandboxTarget,
        display_name: Option<&str>,
    ) -> Result<String, String> {
        ssh::validate_sandbox(&sandbox)?;
        let display = display_name
            .map(|d| d.trim().chars().take(100).collect::<String>())
            .filter(|d| !d.is_empty())
            .unwrap_or_else(|| sandbox.name.clone());
        self.start_ssh_job(
            SshTarget {
                target: sandbox.destination(),
                port: None,
                remote_port: 3774,
                codespace: None,
                sandbox: Some(sandbox),
            },
            display,
            None,
            false,
        )
    }

    /// Bring a codespace machine back: gh starts a stopped codespace, the
    /// bootstrap restarts a dead host, then a new tunnel is opened.
    pub fn codespace_reconnect(self: &Arc<Self>, machine_id: &str) -> Result<String, String> {
        let machine = self
            .list()?
            .into_iter()
            .find(|m| m.id == machine_id)
            .ok_or("Machine is no longer connected")?;
        if machine.sandbox.is_some() {
            // Waking a sandbox means restoring its relay, credential cache and Claude
            // environment; only a user-initiated sandbox reconnect does that.
            return Err(SANDBOX_NOT_CONNECTED.into());
        }
        if machine.codespace.is_none() {
            return Err("This connection is not a codespace".into());
        }
        self.ssh_reconnect(machine_id, false)
    }

    pub fn ssh_reconnect(
        self: &Arc<Self>,
        machine_id: &str,
        upgrade: bool,
    ) -> Result<String, String> {
        let machine = {
            let _guard = self
                .store
                .lock()
                .map_err(|_| "Connection store is locked")?;
            read(&self.store_path())?
                .into_iter()
                .find(|m| m.id == machine_id)
                .ok_or("Machine is no longer connected")?
        };
        let target = machine
            .ssh
            .clone()
            .ok_or("This connection does not use SSH")?;
        self.start_ssh_job(target, machine.name.clone(), Some(machine), upgrade)
    }

    /// Make sure a codespace machine is connected, waking the codespace if
    /// needed. See `ensure_codespace_with`; no cancel, no progress.
    pub fn ensure_codespace(self: &Arc<Self>, machine_id: &str) -> Result<(), String> {
        self.ensure_codespace_with(machine_id, &AtomicBool::new(false), &mut |_| {})
    }

    /// No-op when the tunnel already answers `environment.describe`; otherwise
    /// runs `codespace_reconnect` and waits for it. BLOCKS for as long as the
    /// codespace takes to start (minutes): call it from a background thread,
    /// never the UI thread. Only user-initiated actions should call it.
    ///
    /// * `progress` fires (once) only when a wake actually starts or is joined.
    /// * Concurrent callers for the same machine share one reconnect job.
    /// * Setting `cancel` aborts this caller's wait with `WAKE_CANCELLED` and,
    ///   if it owns the job, cancels the job.
    pub fn ensure_codespace_with(
        self: &Arc<Self>,
        machine_id: &str,
        cancel: &AtomicBool,
        progress: &mut dyn FnMut(&str),
    ) -> Result<(), String> {
        self.run_coalesced(machine_id, cancel, progress, |progress| {
            let noun = match self.request(machine_id, "environment.describe", json!({})) {
                Ok(_) => return Ok(()),
                Err(error) if error.starts_with("Codespace is not connected") => "codespace",
                // Never wake a sandbox here (any ssh resumes and bills it, and the relay and
                // credentials would not be restored): the caller shows the state.
                Err(error) => return Err(error),
            };
            progress(&format!("Waking {noun}…"));
            let job = self.start_when_free(cancel, JOB_SLOT_WAIT, || {
                self.codespace_reconnect(machine_id)
            })?;
            let woke = loop {
                if cancel.load(Ordering::Relaxed) {
                    let _ = self.ssh_cancel(&job);
                    // The job thread stops within moments; let it finish so an
                    // immediate re-send does not find the slot busy.
                    for _ in 0..20 {
                        if self.ssh_poll(&job).map_or(true, |view| view.done) {
                            break;
                        }
                        std::thread::sleep(std::time::Duration::from_millis(100));
                    }
                    return Err(WAKE_CANCELLED.into());
                }
                let view = self.ssh_poll(&job)?;
                if view.done {
                    break view.error.map_or(Ok(()), Err);
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            };
            woke?;
            // Phase 2 of the Host upgrade. This is a user-initiated connect (a send, a
            // retry), so it may run; a failure is a warning line, never an error.
            self.upgrade_host_if_needed(machine_id, None, cancel, progress);
            Ok(())
        })
    }

    /// SSH setup runs one job at a time. Instead of failing while another
    /// job (a pairing, a wake of another machine, a just-cancelled wake) holds
    /// the slot, wait for it, bounded and cancellable.
    fn start_when_free(
        &self,
        cancel: &AtomicBool,
        limit: std::time::Duration,
        mut start: impl FnMut() -> Result<String, String>,
    ) -> Result<String, String> {
        let deadline = std::time::Instant::now() + limit;
        loop {
            match start() {
                Err(error) if error == SSH_BUSY => {}
                other => return other,
            }
            if cancel.load(Ordering::Relaxed) {
                return Err(WAKE_CANCELLED.into());
            }
            if std::time::Instant::now() >= deadline {
                return Err(SSH_BUSY.into());
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
    }

    fn run_coalesced(
        &self,
        key: &str,
        cancel: &AtomicBool,
        progress: &mut dyn FnMut(&str),
        work: impl FnOnce(&mut dyn FnMut(&str)) -> Result<(), String>,
    ) -> Result<(), String> {
        let mut work = Some(work);
        loop {
            if cancel.load(Ordering::Relaxed) {
                return Err(WAKE_CANCELLED.into());
            }
            let (slot, leader) = {
                let mut wakes = self.wakes.lock().map_err(|_| "Wake state is unavailable")?;
                match wakes.get(key) {
                    Some(slot) => (slot.clone(), false),
                    None => {
                        let slot: WakeSlot = Arc::new((Mutex::new(None), Condvar::new()));
                        wakes.insert(key.into(), slot.clone());
                        (slot, true)
                    }
                }
            };
            if leader {
                // Fills and clears the slot even if `work` panics.
                struct Finish<'a> {
                    wakes: &'a Mutex<HashMap<String, WakeSlot>>,
                    key: &'a str,
                    slot: WakeSlot,
                    result: Option<Result<(), String>>,
                }
                impl Drop for Finish<'_> {
                    fn drop(&mut self) {
                        if let Ok(mut wakes) = self.wakes.lock() {
                            wakes.remove(self.key);
                        }
                        let result = self.result.take().unwrap_or_else(|| {
                            Err("Waking the codespace failed unexpectedly".into())
                        });
                        *self.slot.0.lock().unwrap_or_else(|e| e.into_inner()) = Some(result);
                        self.slot.1.notify_all();
                    }
                }
                let mut finish = Finish {
                    wakes: &self.wakes,
                    key,
                    slot,
                    result: None,
                };
                let result = work.take().expect("leader runs once")(progress);
                finish.result = Some(result.clone());
                return result;
            }
            progress("Waking codespace…");
            let mut guard = slot.0.lock().unwrap();
            let result = loop {
                if let Some(result) = guard.clone() {
                    break result;
                }
                if cancel.load(Ordering::Relaxed) {
                    return Err(WAKE_CANCELLED.into());
                }
                guard = slot
                    .1
                    .wait_timeout(guard, std::time::Duration::from_millis(100))
                    .unwrap()
                    .0;
            };
            drop(guard);
            // The leader's caller gave up; this caller did not.
            if result.as_ref().err().is_some_and(|e| e == WAKE_CANCELLED) {
                continue;
            }
            return result;
        }
    }

    pub fn ssh_poll(&self, job_id: &str) -> Result<JobView, String> {
        Ok(self.job(job_id)?.view())
    }

    pub fn ssh_answer(&self, job_id: &str, prompt_id: &str, answer: String) -> Result<(), String> {
        self.job(job_id)?.answer(prompt_id, answer)
    }

    pub fn ssh_cancel(&self, job_id: &str) -> Result<(), String> {
        self.job(job_id)?.cancel();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    struct ReceivedRequest {
        headers: String,
        payload: Value,
    }

    fn host_fixture(
        responses: Vec<Value>,
    ) -> (
        String,
        std::sync::mpsc::Receiver<ReceivedRequest>,
        std::thread::JoinHandle<()>,
    ) {
        use std::io::{BufRead, BufReader};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let (sender, receiver) = std::sync::mpsc::channel();
        let server = std::thread::spawn(move || {
            for response in responses {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(std::time::Duration::from_secs(3)))
                    .unwrap();
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut headers = String::new();
                let mut length = 0;
                loop {
                    let mut line = String::new();
                    reader.read_line(&mut line).unwrap();
                    if line == "\r\n" {
                        break;
                    }
                    if let Some((key, value)) = line.split_once(':') {
                        if key.eq_ignore_ascii_case("content-length") {
                            length = value.trim().parse().unwrap();
                        }
                    }
                    headers.push_str(&line);
                }
                let mut body = vec![0; length];
                reader.read_exact(&mut body).unwrap();
                let _ = sender.send(ReceivedRequest {
                    headers,
                    payload: serde_json::from_slice(&body).unwrap(),
                });
                let body = response.to_string();
                write!(stream, "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n{}", body.len(), body).unwrap();
            }
        });
        (endpoint, receiver, server)
    }

    #[test]
    fn existing_store_remains_compatible_and_credentials_are_private() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("remote-machines.json");
        std::fs::write(&path, r#"[{"id":"old","name":"server","endpoint":"https://host.example","environmentId":"env","token":"private"}]"#).unwrap();
        let connections = Connections::new(dir.path());
        let machine = connections.machine_for_environment("env").unwrap();
        assert_eq!(machine.id, "old");
        assert_eq!(machine.ssh, None);
        assert!(!serde_json::to_string(&machine).unwrap().contains("private"));
        assert!(!format!("{connections:?}").contains("private"));
        connections.disconnect("old").unwrap();
        assert!(connections.list().unwrap().is_empty());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "[]");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }

    #[test]
    fn host_info_follows_connect_describe_and_disconnect() {
        let upstream = json!({ "result": { "protocolVersion": 1, "environmentId": "env", "capabilities": ["files.list"] }});
        let fork = json!({ "result": {
            "protocolVersion": 1,
            "environmentId": "env",
            "host": { "flavor": "monoptah", "version": "1.1.0" },
            "capabilities": ["files.list", "files.readBytes"],
        }});
        let (url, _requests, server) = host_fixture(vec![upstream, fork]);
        let dir = tempfile::tempdir().unwrap();
        let connections = Connections::new(dir.path());
        assert_eq!(connections.host_info("nobody"), None);
        let machine = connections.connect("".into(), url, "a".repeat(43)).unwrap();
        // An upstream describe has no `host` field.
        let info = connections.host_info(&machine.id).unwrap();
        assert_eq!(info.flavor, "upstream");
        assert_eq!(info.version, None);
        assert!(!info.supports("files.readBytes"));
        // A fork describe replaces the entry.
        let info = connections.describe_host(&machine.id).unwrap();
        assert!(info.is_monoptah());
        assert_eq!(info.version.as_deref(), Some("1.1.0"));
        assert!(info.supports("files.readBytes"));
        assert_eq!(connections.host_info(&machine.id), Some(info));
        // Disconnecting drops it.
        connections.disconnect(&machine.id).unwrap();
        assert_eq!(connections.host_info(&machine.id), None);
        server.join().unwrap();
    }

    fn stored(id: &str, environment: &str, ssh: Option<SshTarget>) -> StoredMachine {
        StoredMachine {
            id: id.into(),
            name: id.into(),
            endpoint: "https://host.example".into(),
            environment_id: environment.into(),
            token: "t".repeat(43),
            ssh,
            pending_host_upgrade: false,
            swap_unconfirmed: false,
        }
    }
    fn codespace_target() -> SshTarget {
        SshTarget {
            target: "cs".into(),
            port: None,
            remote_port: 3774,
            codespace: Some("cs".into()),
            sandbox: None,
        }
    }

    #[test]
    fn a_re_added_machine_keeps_only_the_host_info_under_its_final_id() {
        let dir = tempfile::tempdir().unwrap();
        let connections = Connections::new(dir.path());
        write(
            &connections.store_path(),
            &[stored("old", "env", Some(codespace_target()))],
        )
        .unwrap();
        connections
            .host_info
            .record("old", &json!({"capabilities": ["stale"]}));
        // The job started under a provisional id and recorded nothing yet.
        let fork = json!({"host": {"flavor": "monoptah", "version": "1.1.0"}, "capabilities": ["uploads"]});
        connections.host_info.record("provisional", &fork);
        let machine = connections
            .commit_machine(
                stored("provisional", "env", Some(codespace_target())),
                &fork,
            )
            .unwrap();
        assert_eq!(machine.id, "old");
        assert_eq!(connections.host_info.get("provisional"), None);
        let info = connections.host_info.get("old").unwrap();
        assert!(info.is_monoptah());
        assert!(info.supports("uploads") && !info.supports("stale"));
        assert_eq!(connections.list().unwrap().len(), 1);
    }

    #[test]
    fn host_info_of_a_tunnelled_machine_without_a_live_tunnel_is_not_served() {
        let dir = tempfile::tempdir().unwrap();
        let connections = Arc::new(Connections::new(dir.path()));
        write(
            &connections.store_path(),
            &[
                stored("cloud", "env-1", Some(codespace_target())),
                stored("direct", "env-2", None),
            ],
        )
        .unwrap();
        let info = json!({"capabilities": ["files.readBytes"]});
        connections.host_info.record("cloud", &info);
        connections.host_info.record("direct", &info);
        // A direct machine has no tunnel to lose.
        assert!(connections.host_info("direct").is_some());
        // No live tunnel: no answer, and the stale entry is gone.
        assert_eq!(connections.host_info("cloud"), None);
        assert_eq!(connections.host_info.get("cloud"), None);
        // The seam used by file viewing then reports the not-connected answer.
        let error = crate::cloud_sessions::Host::host_info(&connections, "cloud").unwrap_err();
        assert_eq!(error, CODESPACE_NOT_CONNECTED);
    }

    #[test]
    fn connecting_same_host_keeps_identity_and_scopes_authenticated_requests() {
        let descriptor =
            json!({ "result": { "protocolVersion": 1, "environmentId": "env", "name": "Server" }});
        let (url, requests, server) = host_fixture(vec![
            descriptor.clone(),
            descriptor,
            json!({ "result": { "projects": [] } }),
            json!({ "result": { "protocolVersion": 1, "environmentId": "replacement" } }),
        ]);
        let dir = tempfile::tempdir().unwrap();
        let connections = Connections::new(dir.path());
        let token = "a".repeat(43);
        let first = connections
            .connect("".into(), url.clone(), token.clone())
            .unwrap();
        assert_eq!(first.name, "Server");
        let second = connections
            .connect(" New name ".into(), url, token.clone())
            .unwrap();
        assert_eq!(first.id, second.id);
        assert_eq!(second.name, "New name");
        assert_eq!(connections.list().unwrap().len(), 1);
        assert_eq!(
            connections
                .request(&first.id, "projects.list", json!({}))
                .unwrap(),
            json!({ "projects": [] })
        );
        let error = connections
            .request(&first.id, "environment.describe", json!({}))
            .unwrap_err();
        assert!(error.starts_with("Host identity changed"));
        server.join().unwrap();
        let requests: Vec<_> = requests.iter().collect();
        assert_eq!(requests.len(), 4);
        for request in &requests {
            assert!(request.headers.starts_with("POST /rpc HTTP/1.1"));
            assert!(request
                .headers
                .contains(&format!("Authorization: Bearer {token}")));
            assert_eq!(request.payload["version"], 1);
        }
        assert_eq!(requests[0].payload["environmentId"], Value::Null);
        assert_eq!(requests[2].payload["environmentId"], "env");
        assert_eq!(requests[2].payload["method"], "projects.list");
        assert!(requests[3].payload["params"]["supportedProviders"]
            .as_array()
            .unwrap()
            .contains(&json!("devin")));
    }

    #[test]
    fn stored_machines_without_a_codespace_still_load() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("remote-machines.json"),
            r#"[{"id":"a","name":"box","endpoint":"ssh://me@box","environmentId":"env","token":"t","ssh":{"target":"me@box","port":null,"remotePort":3774}}]"#,
        )
        .unwrap();
        let machine = Connections::new(dir.path()).list().unwrap().remove(0);
        assert_eq!(machine.codespace, None);
        let old: Machine = serde_json::from_str(
            r#"{"id":"a","name":"box","endpoint":"e","environmentId":"env","ssh":null}"#,
        )
        .unwrap();
        assert_eq!(old.codespace, None);
    }

    #[test]
    fn codespace_begin_rejects_bad_names_and_reconnect_rejects_plain_machines() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("remote-machines.json"),
            r#"[{"id":"a","name":"box","endpoint":"ssh://me@box","environmentId":"env","token":"t"}]"#,
        )
        .unwrap();
        let connections = Arc::new(Connections::new(dir.path()));
        assert!(connections
            .codespace_begin("-oProxyCommand=x", None)
            .is_err());
        assert!(connections.codespace_begin("a b", None).is_err());
        assert_eq!(
            connections.codespace_reconnect("a").unwrap_err(),
            "This connection is not a codespace"
        );
        assert!(connections.codespace_reconnect("missing").is_err());
    }

    /// Drives the whole codespace lifecycle against a fake `gh`: bootstrap,
    /// pairing and tunnel on begin, then a reconnect that must re-run the
    /// bootstrap before tunnelling, and a lazily restarted tunnel on request.
    #[cfg(unix)]
    #[test]
    fn codespace_begin_reconnect_and_tunnel_restart_use_gh() {
        use std::os::unix::fs::PermissionsExt;
        let Some(python) = [
            "/usr/bin/python3",
            "/opt/homebrew/bin/python3",
            "/usr/local/bin/python3",
        ]
        .into_iter()
        .find(|p| Path::new(p).exists()) else {
            eprintln!(
                "SKIPPED codespace_begin_reconnect_and_tunnel_restart_use_gh: python3 not found"
            );
            return;
        };
        // A current fork Host, so the connect-time upgrade check leaves it alone.
        let describe = json!({ "result": { "protocolVersion": 1, "environmentId": "env-cs", "name": "Host", "host": {"flavor": "monoptah", "version": "1.1.0"} }});
        let (url, _requests, server) = host_fixture(vec![
            describe.clone(),
            describe.clone(),
            describe.clone(),
            // The upgrade check after the explicit wake reads the Host once more.
            describe.clone(),
            json!({ "result": { "projects": [] } }),
            describe,
        ]);
        let fixture_port = url.rsplit(':').next().unwrap().to_string();
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("gh.log");
        let token = "b".repeat(43);
        let gh = dir.path().join("gh");
        std::fs::write(
            &gh,
            format!(
                r#"#!{python}
import os, socket, sys, threading
args = sys.argv[1:]
with open({log:?}, "a") as f:
    f.write(" ".join(args) + "\n")
if "-N" in args:
    port = int(args[args.index("-L") + 1].split(":")[1])
    srv = socket.socket(); srv.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    srv.bind(("127.0.0.1", port)); srv.listen(8)
    def pump(a, b):
        try:
            while True:
                d = a.recv(65536)
                if not d: break
                b.sendall(d)
        except OSError: pass
        try: b.shutdown(socket.SHUT_WR)
        except OSError: pass
    def serve(c):
        # Readiness probes connect and hang up without data: never wake the host.
        first = c.recv(65536)
        if not first:
            c.close(); return
        u = socket.create_connection(("127.0.0.1", {fixture_port}))
        u.sendall(first)
        threading.Thread(target=pump, args=(u, c), daemon=True).start()
        pump(c, u)
    while True:
        c, _ = srv.accept()
        threading.Thread(target=serve, args=(c,), daemon=True).start()
else:
    script = sys.stdin.read()
    if "pair --name" in script:
        print('{{"token":"{token}","environmentId":"env-cs"}}')
    else:
        print('noise')
        print('{{"port":3774}}')
"#,
                log = log.display().to_string(),
            ),
        )
        .unwrap();
        std::fs::set_permissions(&gh, std::fs::Permissions::from_mode(0o755)).unwrap();
        let _gh_guard = ssh::GhOverride::set(gh);

        let connections = Arc::new(Connections::new(dir.path()));
        let wait = |job: String| {
            for _ in 0..600 {
                let view = connections.ssh_poll(&job).unwrap();
                if view.done {
                    return view;
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            panic!(
                "job did not finish: {:?}",
                connections.ssh_poll(&job).unwrap()
            );
        };
        let done = wait(
            connections
                .codespace_begin("octo-abc123", Some("Cloud"))
                .unwrap(),
        );
        assert_eq!(done.error, None);
        let machine = done.machine.unwrap();
        assert_eq!(machine.name, "Cloud");
        assert_eq!(machine.endpoint, "codespace://octo-abc123");
        assert_eq!(machine.codespace.as_deref(), Some("octo-abc123"));
        assert_eq!(machine.ssh.as_ref().unwrap().remote_port, 3774);

        let lines = || -> Vec<String> {
            std::fs::read_to_string(&log)
                .unwrap()
                .lines()
                .map(String::from)
                .collect()
        };
        let before = lines();
        assert_eq!(before.len(), 3, "{before:?}");
        for line in &before[..2] {
            assert!(
                line.starts_with("codespace ssh -c octo-abc123 -- -T "),
                "{line}"
            );
            assert!(line.ends_with("sh -l -s"), "{line}");
        }
        assert!(before[2].contains("-N -L 127.0.0.1:"), "{}", before[2]);
        assert!(before[2].ends_with(":127.0.0.1:3774"), "{}", before[2]);

        // Reconnect: bootstrap again (no pairing), then a fresh tunnel.
        let done = wait(connections.codespace_reconnect(&machine.id).unwrap());
        assert_eq!(done.error, None);
        assert_eq!(done.machine.unwrap().id, machine.id);
        let after = lines();
        assert_eq!(after.len(), before.len() + 2, "{after:?}");
        assert!(after[3].ends_with("sh -l -s"), "{}", after[3]);
        assert!(after[4].contains("-N -L 127.0.0.1:"), "{}", after[4]);

        // With no live tunnel a request must not wake the codespace.
        connections.tunnels.clear();
        let error = connections
            .request(&machine.id, "projects.list", json!({}))
            .unwrap_err();
        assert!(error.starts_with("Codespace is not connected"), "{error}");
        assert_eq!(lines().len(), after.len(), "request spawned gh");

        // Waking is explicit: bootstrap, then tunnel.
        connections.ensure_codespace(&machine.id).unwrap();
        let woke = lines();
        assert_eq!(woke.len(), after.len() + 2, "{woke:?}");
        assert!(woke[5].ends_with("sh -l -s"), "{}", woke[5]);
        assert!(woke[6].contains("-N -L 127.0.0.1:"), "{}", woke[6]);
        assert_eq!(
            connections
                .request(&machine.id, "projects.list", json!({}))
                .unwrap(),
            json!({ "projects": [] })
        );
        // Already connected: a no-op that does not run gh again.
        connections.ensure_codespace(&machine.id).unwrap();
        assert_eq!(lines().len(), woke.len());
        connections.tunnels.clear();
        server.join().unwrap();
    }

    #[test]
    fn closed_connections_count_as_a_dead_route_only_when_asked() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            for _ in 0..2 {
                let (mut stream, _) = listener.accept().unwrap();
                let mut buffer = [0; 4096];
                let _ = stream.read(&mut buffer);
            }
        });
        let call = |dead| rpc_with(&endpoint, "t", None, "projects.list", json!({}), None, dead);
        assert!(call(true)
            .unwrap_err()
            .starts_with("Machine is unreachable"));
        assert!(call(false)
            .unwrap_err()
            .starts_with("The host request did not complete"));
        server.join().unwrap();
    }

    /// A server that accepts, reads the request, and answers only after `delay`. It returns
    /// whether the client had already hung up when it woke (so a late reply goes nowhere).
    fn slow_server(delay: std::time::Duration) -> (String, std::thread::JoinHandle<bool>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut buffer = [0; 4096];
            let _ = stream.read(&mut buffer);
            std::thread::sleep(delay);
            // The request body may arrive in a second segment: drain it, then expect EOF.
            let hung_up = loop {
                match stream.read(&mut buffer) {
                    Ok(0) | Err(_) => break true,
                    Ok(_) => {}
                }
            };
            let body = r#"{"result":{"late":true}}"#;
            let _ = stream.write_all(
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
                .as_bytes(),
            );
            hung_up
        });
        (endpoint, server)
    }

    #[test]
    fn a_timeout_on_a_long_operation_says_it_may_still_be_running() {
        let (endpoint, server) = slow_server(std::time::Duration::from_millis(600));
        let started = std::time::Instant::now();
        let error = rpc_with(
            &endpoint,
            "t",
            None,
            "workspace.run",
            json!({}),
            Some(std::time::Duration::from_millis(150)),
            false,
        )
        .unwrap_err();
        // The passed limit applied (not the 30 s default).
        assert!(started.elapsed() < std::time::Duration::from_secs(5));
        assert_eq!(error, MAY_STILL_RUN);
        // The client dropped the connection, so the late reply is never read.
        assert!(server.join().unwrap(), "the client should have hung up");
    }

    #[test]
    fn the_limits_are_120_for_push_and_pull_200_for_sync_and_30_otherwise() {
        assert_eq!(GIT_REMOTE_TIMEOUT, std::time::Duration::from_secs(120));
        assert_eq!(GIT_SYNC_TIMEOUT, std::time::Duration::from_secs(200));
        assert_eq!(DEFAULT_RPC_TIMEOUT, std::time::Duration::from_secs(30));
    }

    #[test]
    fn an_ordinary_request_that_times_out_is_a_plain_incomplete_request() {
        let (endpoint, server) = slow_server(std::time::Duration::from_millis(600));
        let error = rpc_with_default(
            &endpoint,
            "t",
            None,
            "projects.list",
            json!({}),
            None,
            false,
            std::time::Duration::from_millis(150),
        )
        .unwrap_err();
        assert!(
            error.starts_with("The host request did not complete"),
            "{error}"
        );
        assert_ne!(error, MAY_STILL_RUN);
        assert!(server.join().unwrap());
    }

    #[test]
    fn only_errors_after_connecting_can_mean_the_request_was_sent() {
        assert!(may_have_been_sent(ureq::ErrorKind::Io));
        assert!(!may_have_been_sent(ureq::ErrorKind::ConnectionFailed));
        assert!(!may_have_been_sent(ureq::ErrorKind::Dns));
        assert!(!may_have_been_sent(ureq::ErrorKind::InvalidUrl));
    }

    #[test]
    fn tunnels_existing_never_starts_anything() {
        let tunnels = Tunnels::default();
        assert!(tunnels.existing("none").is_none());
    }

    #[test]
    fn a_tampered_codespace_name_in_the_store_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("remote-machines.json"),
            r#"[{"id":"a","name":"x","endpoint":"codespace://x","environmentId":"env","token":"t","ssh":{"target":"x","port":null,"remotePort":3774,"codespace":"-oProxyCommand=evil"}}]"#,
        )
        .unwrap();
        let connections = Arc::new(Connections::new(dir.path()));
        assert!(connections
            .codespace_reconnect("a")
            .unwrap_err()
            .contains("valid codespace name"));
    }
    #[test]
    fn incompatible_pairing_does_not_create_a_machine() {
        let (url, _, server) = host_fixture(vec![
            json!({ "result": { "protocolVersion": 2, "environmentId": "env" } }),
        ]);
        let dir = tempfile::tempdir().unwrap();
        let connections = Connections::new(dir.path());
        assert!(connections
            .connect("".into(), url, "a".repeat(43))
            .unwrap_err()
            .contains("incompatible"));
        server.join().unwrap();
        assert!(connections.list().unwrap().is_empty());
        assert!(!dir.path().join("remote-machines.json").exists());
    }

    #[test]
    fn invalid_store_and_unknown_remote_operations_never_fall_back() {
        let dir = tempfile::tempdir().unwrap();
        let connections = Connections::new(dir.path());
        assert_eq!(
            connections
                .request("missing", "git.arbitrary", json!({}))
                .unwrap_err(),
            "Unsupported remote operation"
        );
        assert_eq!(
            connections
                .request("missing", "files.read", json!({ "path": "/tmp/private" }))
                .unwrap_err(),
            "Machine is no longer connected"
        );
        std::fs::write(connections.store_path(), "invalid").unwrap();
        assert!(connections.list().is_err());
        assert!(connections.disconnect("missing").is_err());
        assert_eq!(
            std::fs::read_to_string(connections.store_path()).unwrap(),
            "invalid"
        );
    }

    #[test]
    fn remote_paths_round_trip_unix_drives_and_unc_roots() {
        for (host, normalized, qualified) in [
            (
                "/home/user/repo",
                "/home/user/repo",
                "remote://env/home/user/repo",
            ),
            (
                r"C:\Users\Nick\repo",
                "C:/Users/Nick/repo",
                "remote://env/C:/Users/Nick/repo",
            ),
            (
                r"\\server\share\repo",
                "//server/share/repo",
                "remote://env//server/share/repo",
            ),
            ("/", "/", "remote://env/"),
            ("C:", "C:", "remote://env/C:"),
        ] {
            assert_eq!(remote_path("env", host), qualified);
            let parsed = parse_remote_path(qualified).unwrap();
            assert_eq!(parsed.environment_id, "env");
            assert_eq!(parsed.host_path, normalized);
        }
        assert!(parse_remote_path("/local/repo").is_none());
        assert!(parse_remote_path("remote://env").is_none());
        assert!(parse_remote_path("remote:///repo").is_none());
    }

    #[test]
    fn a_slow_host_does_not_look_like_a_dead_tunnel() {
        use std::{net::TcpListener, thread, time::Duration};
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let server = thread::spawn(move || {
            let (_stream, _) = listener.accept().unwrap();
            thread::sleep(Duration::from_millis(200));
        });
        let error = ureq::get(&url)
            .timeout(Duration::from_millis(50))
            .call()
            .unwrap_err();
        let ureq::Error::Transport(error) = error else {
            panic!("Expected a timeout")
        };
        assert!(!connection_refused(&error));
        server.join().unwrap();
        // A freed ephemeral port can be claimed by another parallel fixture.
        // Exercise ureq's actual wrapped I/O source without that port race.
        let error = ureq::Error::from(std::io::Error::from(std::io::ErrorKind::ConnectionRefused));
        let ureq::Error::Transport(error) = error else {
            panic!("Expected a refused connection")
        };
        assert!(connection_refused(&error));
    }

    #[test]
    fn desktop_forwards_supported_host_operations() {
        for method in [
            "git.branches",
            "git.switch",
            "git.createBranch",
            "git.worktrees",
            "git.worktreeCreate",
            "attachments.upload",
            "attachments.read",
            "uploads.begin",
            "uploads.chunk",
            "uploads.commit",
            "uploads.discard",
            "files.readBytes",
        ] {
            assert!(supported_remote_method(method), "{method}");
        }
        assert!(!supported_remote_method("git.arbitrary"));
    }
    #[test]
    fn endpoints_require_an_encrypted_route_and_no_embedded_secrets() {
        assert_eq!(
            endpoint("http://127.0.0.1:3774/").unwrap(),
            "http://127.0.0.1:3774"
        );
        assert!(endpoint("http://[::1]:3774").is_ok());
        assert!(endpoint("https://host.example").is_ok());
        for url in [
            "http://192.168.1.5:3774",
            "file:///tmp/socket",
            "https://user:secret@host.example",
            "https://host.example/rpc",
            "https://host.example?token=secret",
            "https://host.example#secret",
        ] {
            assert!(endpoint(url).is_err(), "{url}");
        }
    }
    #[test]
    fn public_machine_metadata_never_contains_the_token() {
        let stored = StoredMachine {
            id: "id".into(),
            name: "server".into(),
            endpoint: "https://host.example".into(),
            environment_id: "env".into(),
            token: "secret".into(),
            ssh: None,
            pending_host_upgrade: false,
            swap_unconfirmed: false,
        };
        let value = serde_json::to_string(&stored.public()).unwrap();
        assert!(!value.contains("secret"));
        assert!(!value.contains("token"));
    }

    #[test]
    fn concurrent_wakes_share_one_job_and_cancel_is_honoured() {
        let dir = tempfile::tempdir().unwrap();
        let connections = Arc::new(Connections::new(dir.path()));
        let runs = std::sync::atomic::AtomicUsize::new(0);
        let waking = std::sync::atomic::AtomicUsize::new(0);
        std::thread::scope(|scope| {
            for _ in 0..4 {
                scope.spawn(|| {
                    let cancel = AtomicBool::new(false);
                    connections
                        .run_coalesced(
                            "m",
                            &cancel,
                            &mut |_| {
                                waking.fetch_add(1, Ordering::SeqCst);
                            },
                            |_| {
                                runs.fetch_add(1, Ordering::SeqCst);
                                std::thread::sleep(std::time::Duration::from_millis(300));
                                Ok(())
                            },
                        )
                        .unwrap();
                });
            }
        });
        // Callers that arrived during the job joined it; late ones ran again.
        assert!(
            runs.load(Ordering::SeqCst) < 4,
            "{}",
            runs.load(Ordering::SeqCst)
        );
        assert!(waking.load(Ordering::SeqCst) >= 1);
        assert!(connections.wakes.lock().unwrap().is_empty());

        // Pre-cancelled: no work at all.
        let cancel = AtomicBool::new(true);
        let ran = std::sync::atomic::AtomicBool::new(false);
        let error = connections
            .run_coalesced("m", &cancel, &mut |_| {}, |_| {
                ran.store(true, Ordering::SeqCst);
                Ok(())
            })
            .unwrap_err();
        assert_eq!(error, WAKE_CANCELLED);
        assert!(!ran.load(Ordering::SeqCst));

        // A follower whose leader was cancelled retries instead of failing.
        let leader_cancel = Arc::new(AtomicBool::new(false));
        let attempts = std::sync::atomic::AtomicUsize::new(0);
        std::thread::scope(|scope| {
            let c = leader_cancel.clone();
            let conn = &connections;
            let leader = scope.spawn(move || {
                conn.run_coalesced("m", &c, &mut |_| {}, |_| {
                    while !c.load(Ordering::SeqCst) {
                        std::thread::sleep(std::time::Duration::from_millis(10));
                    }
                    Err(WAKE_CANCELLED.into())
                })
            });
            std::thread::sleep(std::time::Duration::from_millis(100));
            let follower = scope.spawn(|| {
                let cancel = AtomicBool::new(false);
                connections.run_coalesced("m", &cancel, &mut |_| {}, |_| {
                    attempts.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                })
            });
            std::thread::sleep(std::time::Duration::from_millis(100));
            leader_cancel.store(true, Ordering::SeqCst);
            assert_eq!(leader.join().unwrap().unwrap_err(), WAKE_CANCELLED);
            follower.join().unwrap().unwrap();
        });
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn a_panicking_leader_clears_the_wake_slot() {
        let dir = tempfile::tempdir().unwrap();
        let connections = Connections::new(dir.path());
        let cancel = AtomicBool::new(false);
        let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _ = connections.run_coalesced("m", &cancel, &mut |_| {}, |_| panic!("boom"));
        }));
        assert!(panicked.is_err());
        assert!(connections.wakes.lock().unwrap().is_empty());
        connections
            .run_coalesced("m", &cancel, &mut |_| {}, |_| Ok(()))
            .unwrap();
    }

    #[test]
    fn wakes_wait_for_the_job_slot_instead_of_failing_busy() {
        let dir = tempfile::tempdir().unwrap();
        let connections = Arc::new(Connections::new(dir.path()));
        // A pairing job (or a just-cancelled wake) holds the single slot.
        let busy = Job::new();
        connections
            .jobs
            .lock()
            .unwrap()
            .insert("pairing".into(), busy.clone());
        // Same busy rule as start_ssh_job, without spawning a real gh job.
        let try_start = |c: &Connections| {
            if c.jobs.lock().unwrap().values().any(|j| !j.view().done) {
                Err(SSH_BUSY.to_string())
            } else {
                Ok("job".to_string())
            }
        };
        let attempts = std::sync::atomic::AtomicUsize::new(0);
        let cancel = AtomicBool::new(false);
        let releaser = {
            let busy = busy.clone();
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_millis(350));
                busy.complete(|| Err("done".into()));
            })
        };
        let id = connections
            .start_when_free(&cancel, std::time::Duration::from_secs(10), || {
                attempts.fetch_add(1, Ordering::SeqCst);
                try_start(&connections)
            })
            .unwrap();
        releaser.join().unwrap();
        assert!(attempts.load(Ordering::SeqCst) > 1);
        assert_eq!(id, "job");
        // Cancel while waiting.
        let hold = Job::new();
        connections.jobs.lock().unwrap().insert("hold".into(), hold);
        let cancelled = AtomicBool::new(true);
        let error = connections
            .start_when_free(&cancelled, std::time::Duration::from_secs(10), || {
                try_start(&connections)
            })
            .unwrap_err();
        assert_eq!(error, WAKE_CANCELLED);
        // The wait is bounded.
        let error = connections
            .start_when_free(&cancel, std::time::Duration::from_millis(250), || {
                try_start(&connections)
            })
            .unwrap_err();
        assert_eq!(error, SSH_BUSY);
        // Other errors are not retried.
        let calls = std::sync::atomic::AtomicUsize::new(0);
        let error = connections
            .start_when_free(&cancel, std::time::Duration::from_secs(10), || {
                calls.fetch_add(1, Ordering::SeqCst);
                Err("nope".into())
            })
            .unwrap_err();
        assert_eq!((error.as_str(), calls.load(Ordering::SeqCst)), ("nope", 1));
    }

    #[test]
    fn sandbox_requests_without_a_live_tunnel_never_connect() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("remote-machines.json"),
            r#"[{"id":"a","name":"x","endpoint":"sbx://box","environmentId":"env","token":"t","ssh":{"target":"sbx_001abc@sbx_cloud","port":null,"remotePort":3774,"sandbox":{"id":"sbx_001abc","name":"box","workspace":"/home/agent/ws","relayPort":47200}}}]"#,
        )
        .unwrap();
        let connections = Arc::new(Connections::new(dir.path()));
        let machine = connections.list().unwrap().remove(0);
        assert_eq!(
            machine.sandbox.as_ref().map(|s| s.id.as_str()),
            Some("sbx_001abc")
        );
        assert_eq!(machine.codespace, None);
        // No tunnel exists, and a request must not create one (that would ssh
        // into, and so resume, the sandbox).
        let error = connections
            .request("a", "projects.list", json!({}))
            .unwrap_err();
        assert_eq!(error, SANDBOX_NOT_CONNECTED);
        assert!(crate::cloud_sessions::is_not_connected(&error));
        assert!(connections.tunnels.existing("a").is_none());
    }

    #[test]
    fn waking_never_reconnects_a_sandbox_machine() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("remote-machines.json"),
            r#"[{"id":"a","name":"x","endpoint":"sbx://box","environmentId":"env","token":"t","ssh":{"target":"sbx_001abc@sbx_cloud","port":null,"remotePort":3774,"sandbox":{"id":"sbx_001abc","name":"box","workspace":"/home/agent/ws","relayPort":47200}}}]"#,
        )
        .unwrap();
        let connections = Arc::new(Connections::new(dir.path()));
        let mut progress = vec![];
        let error = connections
            .ensure_codespace_with("a", &AtomicBool::new(false), &mut |l| {
                progress.push(l.to_string())
            })
            .unwrap_err();
        assert_eq!(error, SANDBOX_NOT_CONNECTED);
        assert!(progress.is_empty(), "no wake was announced: {progress:?}");
        assert_eq!(
            connections.codespace_reconnect("a").unwrap_err(),
            SANDBOX_NOT_CONNECTED
        );
        // No ssh job was started for either call.
        assert!(connections.jobs.lock().unwrap().is_empty());
        assert!(connections.tunnels.existing("a").is_none());
    }

    #[test]
    fn sandbox_begin_rejects_bad_targets() {
        let dir = tempfile::tempdir().unwrap();
        let connections = Arc::new(Connections::new(dir.path()));
        let target = |id: &str, workspace: &str, relay_port| SandboxTarget {
            id: id.into(),
            name: "box".into(),
            workspace: workspace.into(),
            relay_port,
            config: None,
            mode: Default::default(),
        };
        for bad in [
            target("-oProxyCommand=x", "/w", 1),
            target("sbx_1", "w", 1),
            target("sbx_1", "/w;x", 1),
            target("sbx_1", "/w", 0),
        ] {
            assert!(connections.sandbox_begin(bad, None).is_err());
        }
    }
}
