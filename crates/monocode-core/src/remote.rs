//! Remote host connections. The renderer receives machine metadata, never the
//! saved bearer credential. Workspace requests never fall back to local calls.
pub mod attachments;
pub mod protocol;
mod ssh;
pub mod ssh_askpass;

use ssh::{Job, Tunnel, Tunnels};
pub use ssh::{JobView, Prompt, SshTarget};
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
}

type WakeSlot = Arc<(Mutex<Option<Result<(), String>>>, Condvar)>;
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
        if let Ok(jobs) = self.jobs.lock() {
            for job in jobs.values() {
                job.cancel();
            }
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
    rpc_with(endpoint, token, environment_id, method, params, false)
}

/// `closed_is_dead`: a connection reset or closed without a reply also means
/// the route is gone. Behind a codespace tunnel that is what a stopped host
/// looks like: the local forward accepts, then the remote side hangs up.
fn rpc_with(
    endpoint: &str,
    token: &str,
    environment_id: Option<&str>,
    method: &str,
    params: Value,
    closed_is_dead: bool,
) -> Result<Value, String> {
    let agent = ureq::AgentBuilder::new()
        .redirects(0)
        .timeout_connect(std::time::Duration::from_secs(5))
        .timeout(std::time::Duration::from_secs(30))
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
        .map_err(|e| e.to_string())?;
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
        };
        machines.retain(|m| m.id != id);
        machines.push(machine.clone());
        write(&path, &machines)?;
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
        Ok(())
    }

    pub fn request(
        &self,
        machine_id: &str,
        method: &str,
        mut params: Value,
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
        let codespace = machine.ssh.as_ref().is_some_and(|t| t.codespace.is_some());
        let tunnel_lease = if let Some(target) = &machine.ssh {
            if codespace {
                // Background work must never wake a codespace (that is what
                // `gh codespace ssh` does), so only a live tunnel is used.
                Some(
                    self.tunnels
                        .existing(&machine.id)
                        .ok_or(CODESPACE_NOT_CONNECTED)?,
                )
            } else {
                Some(self.tunnels.endpoint(&machine.id, target)?)
            }
        } else {
            None
        };
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
            codespace,
        );
        if response
            .as_ref()
            .err()
            .is_some_and(|error| error.starts_with("Machine is unreachable"))
        {
            if let Some(lease) = &tunnel_lease {
                self.tunnels.invalidate(&machine.id, lease);
            }
            if codespace {
                return Err(CODESPACE_NOT_CONNECTED.into());
            }
        }
        let result = response?;
        if method == "environment.describe"
            && result.get("environmentId").and_then(Value::as_str)
                != Some(machine.environment_id.as_str())
        {
            return Err("Host identity changed. Add this machine again before continuing.".into());
        }
        Ok(result)
    }
}

/// Returned instead of waking a codespace. Callers that act on a user's
/// request answer it with `ensure_codespace`/`codespace_reconnect`.
pub const CODESPACE_NOT_CONNECTED: &str =
    "Codespace is not connected. Reconnect to start it and restore the connection.";

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
            | "devices.revokeSelf"
            | "git.diff"
            | "git.branches"
            | "git.switch"
            | "git.createBranch"
            | "git.worktrees"
            | "git.worktreeCreate"
            | "files.read"
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

impl Connections {
    fn start_ssh_job(
        self: &Arc<Self>,
        target: SshTarget,
        name: String,
        existing: Option<StoredMachine>,
        upgrade: bool,
    ) -> Result<String, String> {
        ssh::check_codespace(&target)?;
        let job = Job::new();
        let id = job.view().id;
        {
            let mut jobs = self
                .jobs
                .lock()
                .map_err(|_| "Connection setup is unavailable")?;
            if jobs.values().any(|job| !job.view().done) {
                return Err(
                    "Another SSH connection is being set up. Finish or cancel it first.".into(),
                );
            }
            jobs.retain(|_, job| !job.view().done);
            jobs.insert(id.clone(), job.clone());
        }
        let state = self.clone();
        std::thread::spawn(move || {
            let prepared = (|| -> Result<(StoredMachine, Tunnel), String> {
                let askpass = job.askpass()?;
                let mut target = target;
                let codespace = target.codespace.is_some();
                let mut machine = if let Some(mut existing) = existing {
                    if upgrade || codespace {
                        // A codespace may have stopped, which also stopped its
                        // host. gh starts it on connect, and the bootstrap's
                        // service install brings the host back with its saved
                        // state, so a codespace reconnect always re-runs it.
                        let platform = ssh::detect_platform(&target, &job, &askpass)?;
                        job.message(if upgrade {
                            "Updating Monoptah Host on the machine…"
                        } else {
                            "Starting Monoptah Host on the codespace…"
                        });
                        let script = if upgrade {
                            ssh::upgrade_script(platform, target.remote_port)
                        } else if codespace {
                            ssh::codespace_bootstrap_script()
                        } else {
                            ssh::bootstrap_script(platform)
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
                        if codespace {
                            ssh::codespace_bootstrap_script()
                        } else {
                            ssh::bootstrap_script(platform)
                        },
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
                        endpoint: match &target.codespace {
                            Some(name) => format!("codespace://{name}"),
                            None => format!("ssh://{}", target.target),
                        },
                        environment_id,
                        token,
                        ssh: Some(target.clone()),
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
                    let capabilities = descriptor.get("capabilities").and_then(Value::as_array);
                    let supports = |name: &str| {
                        capabilities.is_some_and(|entries| {
                            entries.iter().any(|entry| entry.as_str() == Some(name))
                        })
                    };
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
                Ok((machine, tunnel))
            })();
            job.complete(|| {
                let (mut machine, tunnel) = prepared?;
                let _guard = state
                    .store
                    .lock()
                    .map_err(|_| "Connection store is locked")?;
                let path = state.store_path();
                let mut machines = read(&path)?;
                if let Some(old) = machines
                    .iter()
                    .find(|m| m.environment_id == machine.environment_id)
                {
                    machine.id = old.id.clone();
                }
                machines.retain(|m| m.id != machine.id);
                machines.push(machine.clone());
                write(&path, &machines)?;
                state.tunnels.insert(machine.id.clone(), tunnel);
                Ok(machine.public())
            });
        });
        Ok(id)
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
            match self.request(machine_id, "environment.describe", json!({})) {
                Ok(_) => return Ok(()),
                Err(error) if error.starts_with("Codespace is not connected") => {}
                Err(error) => return Err(error),
            }
            progress("Waking codespace…");
            let job = self.codespace_reconnect(machine_id)?;
            loop {
                if cancel.load(Ordering::Relaxed) {
                    let _ = self.ssh_cancel(&job);
                    return Err(WAKE_CANCELLED.into());
                }
                let view = self.ssh_poll(&job)?;
                if view.done {
                    return view.error.map_or(Ok(()), Err);
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
        })
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
                let result = work.take().expect("leader runs once")(progress);
                if let Ok(mut wakes) = self.wakes.lock() {
                    wakes.remove(key);
                }
                *slot.0.lock().unwrap() = Some(result.clone());
                slot.1.notify_all();
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
        let describe =
            json!({ "result": { "protocolVersion": 1, "environmentId": "env-cs", "name": "Host" }});
        let (url, _requests, server) = host_fixture(vec![
            describe.clone(),
            describe.clone(),
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
        let call = |dead| rpc_with(&endpoint, "t", None, "projects.list", json!({}), dead);
        assert!(call(true)
            .unwrap_err()
            .starts_with("Machine is unreachable"));
        assert!(call(false)
            .unwrap_err()
            .starts_with("The host request did not complete"));
        server.join().unwrap();
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
}
