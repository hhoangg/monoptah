//! Start flow of a Docker sandbox cloud session: the sandbox twin of
//! [`super::startup`]'s codespace flow, driving the same [`StartRecord`], [`StepKind`]s and
//! [`StartEvent`]s so the GUI only calls `begin_sandbox_start` / `run_start` /
//! `rebuild_start`.
//!
//! # Flow
//!
//! 1. [`CloudSessions::begin_sandbox_start`] `(local_cwd, &DockerProjectConfig, branch,
//!    harness)`: checks the prerequisites, derives a name from the branch and a fresh request
//!    id, creates the pending session and PERSISTS the start record (name, request id, config
//!    snapshot) BEFORE `sbx create`, then persists the sandbox id right after. A crash in
//!    between is reconciled by name ([`sandboxes::find_by_name`]); a name is never created
//!    twice.
//! 2. [`CloudSessions::run_start`] on a sandbox record (idempotent, restart-safe, Retry is the
//!    same call) walks the steps below. Each outer-sandbox script is an embedded file run as
//!    `sh -s` over ssh with the script on stdin; secrets travel on stdin only.
//!
//! | Step | What happens |
//! |---|---|
//! | Create | id known: `inspect` it (gone -> failure, never recreated); id unknown: create or adopt by name |
//! | Provision | `docker_provision.sh` phases `setup` (xz, Dev Container CLI), `approve` (Git credential into an in-memory `git-credential-cache`, payload on stdin) and `clone` (checkout + feature branch) |
//! | BuildContainer, BlockingCommands | `docker_up.sh` runs `devcontainer up --skip-non-blocking-commands --log-format json` detached into a log file; the log is followed over ssh and mapped to steps by [`parse_up_line`] |
//! | ConfigureCodespace ("Configure environment") | inside the container: git credential helper pointing at the mounted socket, for the exact repo URL |
//! | Connect | bootstrap the Host in the container (Claude prelude on stdin), start the relay, pair and tunnel through [`Host::connect_sandbox`](super::Host::connect_sandbox) |
//! | OpenProject | [`super::open_project`] with the container's `remoteWorkspaceFolder` |
//! | LifecycleCommands | `devcontainer run-user-commands` detached; never blocks `Ready` |
//!
//! # Where things live
//!
//! * Workspace in the OUTER sandbox: `$HOME/workspaces/<repo>`, the path the provision script
//!   reports (`MONOPTAH_WORKSPACE=`). It is persisted and is the `--workspace-folder` of every
//!   devcontainer call, so it is never deleted or recreated (a recreated folder breaks
//!   `run-user-commands`).
//! * Credentials: the Git payload goes to a `git-credential-cache` daemon whose socket is
//!   `$HOME/.monoptah/git/sock` in the outer sandbox, bind-mounted into the container at
//!   [`CONTAINER_GIT_DIR`]. The container's global git config names that socket for the exact
//!   repo URL only. The daemon keeps the token in memory (24 h), nothing is written to disk.
//! * Relay: the outer sandbox listens on `127.0.0.1:`[`DEFAULT_RELAY_PORT`] (the only place
//!   `ssh -L` may point to). Each connection runs `docker exec -i -u <remoteUser> <container>`
//!   with the Host's own packaged Node (`$(cat ~/.monocode-host/runtime-path)/bin/node -e`)
//!   piping stdin/stdout to the Host's loopback port, so it works in any image and needs no
//!   socat inside the container.
//! * Host port: the one the bootstrap reports (`connection-info`), 3774 by default.
//! * Host process: a devcontainer can have `/run/systemd/system` although systemd is not PID 1,
//!   and the released Host's `systemdBooted()` only checks that directory, so `service install`
//!   would fail with "needs systemd user services and lingering". The sandbox bootstrap
//!   ([`sandbox_bootstrap_script`]) therefore sets `MONOCODE_HOST_DETACHED=1`: it starts the Host
//!   itself like `startDetached` (same data dir, port and `host.log`) before `service install`,
//!   which then returns early because a Host answers. A restart (token or Claude path change) is
//!   the template's `service uninstall`, which stops a detached Host too; the same start follows.
//!   A real fix belongs in `host/service.ts` for a future Host release: `systemdBooted` should
//!   also require PID 1 to be systemd (for example `/proc/1/comm` equal to `systemd`); the
//!   bootstrap workaround can go once every supported Host has it.
//!
//! # Accepted trade-offs
//!
//! * Every process in the container, repository lifecycle commands included, can use the
//!   Git token through the mounted socket (scoped to the one repo URL) and the Claude token
//!   through the Host's environment.
//! * The Claude installer runs with the token exported.
//! * The socket is reachable only by the outer sandbox user's uid: with
//!   `updateRemoteUserUID` off and a different container uid, the container can not use it.
//! * The Git credential cache keeps a token for [`CREDENTIAL_CACHE_SECONDS`] (24 h);
//!   [`CloudSessions::deliver_git_credential`] puts it back (reconnect).
//!
//! Background code never runs ssh into a sandbox (that resumes and bills it): only
//! `run_start` and `rebuild_start` (user actions) do. `inspect` is the only read they use
//! before ssh.
use super::backend::docker::{DockerBackend, SandboxShell, ShellOutput, SHELL_ABORTED};
use super::backend::{CloudBackendKind, CloudRef};
use super::docker_config::{valid_ref, DockerProjectConfig};
use super::startup::{
    remove_start_record, save_start_record, start_record, StartEvent, StartRecord, Step, StepKind,
    StepStatus, PROGRESS_PREFIX,
};
use super::{require_claude, CloudBinding, CloudSessions, OpenRequest, START_ABORTED};
use crate::git_credentials::{self, GitCredentials};
use crate::remote::{
    sandbox_bootstrap_script, shell_quote, validate_sandbox_workspace, SandboxTarget,
};
use crate::sandboxes::{self, Compute, SbxRunner};
use crate::store::{pending_cloud_project, Session, Store};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use zeroize::Zeroizing;

/// Loopback port of the relay in the outer sandbox.
pub const DEFAULT_RELAY_PORT: u16 = 47200;
/// Where the credential socket directory is bind-mounted inside the container.
pub const CONTAINER_GIT_DIR: &str = "/run/monoptah-git";
/// Port the Host listens on unless its bootstrap says otherwise.
const DEFAULT_HOST_PORT: u16 = 3774;
const FOLLOW_ATTEMPTS: usize = 4;
const SANDBOX_GONE: &str = "Sandbox was deleted. Start a new session.";

/// Whether `error` says a reconnect can never succeed because the sandbox is deleted or its
/// container was recreated (the session's history lived in the old one). The UI then offers
/// archiving and a new session instead of Reconnect.
pub fn is_sandbox_unrecoverable(error: &str) -> bool {
    error.starts_with(SANDBOX_GONE) || error.starts_with(reconnect::RECREATED)
}

pub(super) const PROVISION_SCRIPT: &str = include_str!("backend/docker_provision.sh");
pub(super) const UP_SCRIPT: &str = include_str!("backend/docker_up.sh");
pub(super) const RELAY_SCRIPT: &str = include_str!("backend/docker_relay.sh");
pub(super) const GIT_CONFIG_SCRIPT: &str = include_str!("backend/docker_git_config.sh");

const SHORT: Duration = Duration::from_secs(120);
const LONG: Duration = Duration::from_secs(30 * 60);

mod reconnect;
pub(crate) use reconnect::ReconnectError;

// ------------------------------------------------------------------------ data

/// The container `devcontainer up` reported.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContainerInfo {
    pub id: String,
    pub remote_user: String,
    /// The project folder inside the container.
    pub remote_workspace: String,
}

/// What a sandbox start needs beyond a codespace start's fields. Persisted inside the
/// [`StartRecord`]. Holds no secret: the config only names a credential and a Claude account.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SandboxStart {
    /// Seeds the unique suffix of the sandbox name.
    pub request_id: String,
    /// Snapshot of the project config this start was created from.
    pub config: DockerProjectConfig,
    /// Known right after `create`; `None` until then.
    pub sandbox_id: Option<String>,
    /// Folder name of the checkout under `$HOME/workspaces`.
    pub repo_dir: String,
    /// Absolute checkout path in the OUTER sandbox, known once provisioned.
    pub workspace: Option<String>,
    pub container: Option<ContainerInfo>,
    pub relay_port: u16,
    /// The next `devcontainer up` removes the existing container (rebuild).
    #[serde(default)]
    pub rebuild: bool,
}

/// The ssh/exec seam, the secret source and the `sbx` runner of the sandbox flow.
#[derive(Clone)]
pub(super) struct SandboxDeps {
    pub runner: Arc<dyn SbxRunner>,
    pub shell: Arc<dyn SandboxShell>,
    pub secrets: Arc<dyn SandboxSecrets>,
    pub backend: Arc<DockerBackend>,
    /// Home directory whose `.ssh` config must carry the `sbx_cloud` alias.
    pub home: PathBuf,
}

// --------------------------------------------------------------------- secrets

/// A Claude credential that is ready to hand to a sandbox.
pub struct ClaudeAuth {
    variable: &'static str,
    secret: Zeroizing<String>,
}

impl ClaudeAuth {
    pub fn new(variable: &'static str, secret: String) -> Self {
        Self {
            variable,
            secret: Zeroizing::new(secret),
        }
    }
    pub fn secret(&self) -> &str {
        &self.secret
    }
    /// Shell text for the top of the Host bootstrap: it drops every other Claude credential
    /// variable and exports exactly the pinned one. Travels on stdin only.
    pub fn prelude(&self) -> Zeroizing<String> {
        let mut text = Zeroizing::new(String::from(
            "unset ANTHROPIC_AUTH_TOKEN ANTHROPIC_API_KEY CLAUDE_CODE_OAUTH_TOKEN\n",
        ));
        if self.variable != "CLAUDE_CODE_OAUTH_TOKEN" {
            // The bootstrap hashes this variable to know when to restart the Host.
            text.push_str(&format!("MONOCODE_HOST_TOKEN_VAR={}\n", self.variable));
        }
        text.push_str(&format!(
            "export {}={}\n",
            self.variable,
            shell_quote(&self.secret)
        ));
        text
    }
}

/// Where a sandbox gets its secrets. Injectable; also used by reconnect.
pub trait SandboxSecrets: Send + Sync {
    /// The Claude credential of the pinned account; `None` when there is no account or it
    /// has no saved token or key.
    fn claude_auth(&self, account: Option<&str>) -> Result<Option<ClaudeAuth>, String>;
    /// The `git credential approve` payload for `repo_url`.
    fn git_payload(&self, credential_id: &str, repo_url: &str)
        -> Result<Zeroizing<String>, String>;
    /// [`ClaudeAuth::prelude`] of the pinned account.
    fn claude_prelude(&self, account: Option<&str>) -> Result<Option<Zeroizing<String>>, String> {
        Ok(self.claude_auth(account)?.map(|auth| auth.prelude()))
    }
}

/// Secrets from the app's own store.
pub struct StoreSecrets {
    store: Arc<Mutex<Store>>,
}

impl StoreSecrets {
    pub fn new(store: Arc<Mutex<Store>>) -> Self {
        Self { store }
    }
}

impl SandboxSecrets for StoreSecrets {
    fn claude_auth(&self, account: Option<&str>) -> Result<Option<ClaudeAuth>, String> {
        let Some(account) = account else {
            return Ok(None);
        };
        let store = self
            .store
            .lock()
            .map_err(|_| "Native session store unavailable".to_string())?;
        let root = store
            .data_dir()
            .ok_or("Provider account storage is unavailable")?;
        let profile = crate::accounts::Profile::new(account, root);
        Ok(profile
            .claude_secret()?
            .map(|(credential, secret)| ClaudeAuth::new(credential.variable(), secret)))
    }
    fn git_payload(
        &self,
        credential_id: &str,
        repo_url: &str,
    ) -> Result<Zeroizing<String>, String> {
        let store = self
            .store
            .lock()
            .map_err(|_| "Native session store unavailable".to_string())?;
        GitCredentials::for_store(&store)?.credential_payload(credential_id, repo_url)
    }
}

/// Masks every secret this run holds, plus anything that looks like one, in text that may be
/// shown or stored.
struct Scrub(Vec<Zeroizing<String>>);

impl Scrub {
    fn apply(&self, text: &str) -> String {
        let mut out = sandboxes::redact(text);
        for secret in self.0.iter().filter(|s| s.len() >= 4) {
            out = git_credentials::redact(&out, secret.as_str());
        }
        out
    }
}

// ------------------------------------------------------------- pure helpers

/// `git credential` payload line `password=` value.
fn payload_password(payload: &str) -> Option<String> {
    payload
        .lines()
        .find_map(|line| line.strip_prefix("password="))
        .map(str::to_string)
}

/// Checkout folder name for the repo path `owner/name`: its last segment, plain characters.
pub(super) fn repo_dir_of(path: &str) -> String {
    let last = path.rsplit('/').next().unwrap_or("");
    let last = last.strip_suffix(".git").unwrap_or(last);
    let clean: String = last
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
        .collect();
    let clean = clean.trim_start_matches(['.', '-']).to_string();
    if clean.is_empty() {
        "repo".into()
    } else {
        clean
    }
}

fn compute_of(config: &DockerProjectConfig) -> Result<Compute, String> {
    let compute = Compute {
        cpus: u8::try_from(config.cpus).map_err(|_| "CPUs are out of range".to_string())?,
        memory: config.memory.clone(),
        ttl: config.ttl.clone(),
        platform: config.platform.clone(),
    };
    compute.validate()?;
    Ok(compute)
}

/// `NAME='value'` lines in front of an embedded script.
pub(super) fn with_vars(vars: &[(&str, &str)], script: &str) -> String {
    let mut out = String::new();
    for (name, value) in vars {
        out.push_str(&format!("{name}={}\n", shell_quote(value)));
    }
    out.push_str(script);
    out
}

/// How long the in-memory Git credential cache keeps a token (the sandbox's own 24 h cap):
/// after that, git inside the container asks again and fails, so a long-lived session needs
/// [`CloudSessions::deliver_git_credential`] again (a reconnect does that).
pub const CREDENTIAL_CACHE_SECONDS: u32 = 24 * 60 * 60;

/// The remote command that feeds a `git credential approve` payload (on stdin) to the
/// cache daemon. The script is non-secret text in argv; the payload never is.
fn approve_remote() -> String {
    let script = with_vars(
        &[
            ("PHASE", "approve"),
            ("CACHE_TIMEOUT", &CREDENTIAL_CACHE_SECONDS.to_string()),
        ],
        PROVISION_SCRIPT,
    );
    format!("sh -c {}", shell_quote(&script))
}

/// Remote command word that runs `sh -s` in the outer sandbox.
const SH_S: &str = "sh -s";

/// What one line of `devcontainer up --log-format json` means.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UpLine {
    /// `{"type":"progress","name":"Running onCreateCommand...","status":"running"}`.
    Progress { name: String, status: String },
    /// The text of a `start` / `stop` line.
    Text(String),
    /// The final stdout JSON.
    Outcome {
        success: bool,
        container_id: Option<String>,
        remote_user: Option<String>,
        remote_workspace: Option<String>,
        /// The CLI's own error text on failure.
        message: Option<String>,
    },
    /// Anything else (non-JSON output, other JSON types).
    Plain(String),
}

/// Parses one log line of `devcontainer up --log-format json`.
pub fn parse_up_line(line: &str) -> UpLine {
    let line = line.trim();
    let Ok(serde_json::Value::Object(map)) = serde_json::from_str::<serde_json::Value>(line) else {
        return UpLine::Plain(line.to_string());
    };
    let text = |key: &str| {
        map.get(key)
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };
    if let Some(outcome) = text("outcome") {
        return UpLine::Outcome {
            success: outcome == "success",
            container_id: text("containerId"),
            remote_user: text("remoteUser"),
            remote_workspace: text("remoteWorkspaceFolder"),
            message: text("description").or_else(|| text("message")),
        };
    }
    match map.get("type").and_then(|v| v.as_str()) {
        Some("progress") => match (text("name"), text("status")) {
            (Some(name), Some(status)) => UpLine::Progress { name, status },
            _ => UpLine::Plain(line.to_string()),
        },
        Some("start" | "stop") => text("text").map_or(UpLine::Plain(String::new()), UpLine::Text),
        _ => UpLine::Plain(String::new()),
    }
}

/// True for the progress line that starts a blocking lifecycle command
/// (`Running onCreateCommand...`, `Running postCreateCommand...`).
pub(super) fn starts_blocking_commands(name: &str, status: &str) -> bool {
    status == "running" && name.starts_with("Running ") && name.contains("Command")
}

fn valid_container_id(id: &str) -> bool {
    (12..=64).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_hexdigit())
}
fn valid_user(user: &str) -> bool {
    !user.is_empty()
        && user.len() <= 64
        && !user.starts_with('-')
        && user
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

/// Value of `KEY=` in the marker line `line`.
fn marker<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    line.trim()
        .strip_prefix(key)
        .and_then(|rest| rest.strip_prefix('='))
}

/// Last few lines of a failed script's output, redacted, as one error sentence.
fn failure_text(out: &ShellOutput, scrub: &Scrub, what: &str) -> String {
    let text = if out.stderr.trim().is_empty() {
        &out.stdout
    } else {
        &out.stderr
    };
    let lines: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with("MONOPTAH_"))
        .collect();
    let tail = lines[lines.len().saturating_sub(4)..].join(" | ");
    let tail: String = scrub.apply(&tail).chars().take(600).collect();
    if tail.is_empty() {
        format!("{what} failed")
    } else {
        format!("{what} failed: {tail}")
    }
}

/// Why a run ended without `Ready`.
enum Stop {
    Aborted,
    Failed(String),
}
impl From<&str> for Stop {
    fn from(error: &str) -> Self {
        Stop::from(error.to_string())
    }
}
impl From<String> for Stop {
    fn from(error: String) -> Self {
        if error == START_ABORTED || error == SHELL_ABORTED {
            Stop::Aborted
        } else {
            Stop::Failed(error)
        }
    }
}

/// Everything one run shares.
struct Run<'a> {
    record: &'a mut StartRecord,
    events: &'a mut dyn FnMut(StartEvent),
    cancel: &'a AtomicBool,
    deps: SandboxDeps,
    scrub: Scrub,
    /// A reconnect: the record is transient, so "aborted" means `cancel` only (a start
    /// record that is gone or failed is how a start is aborted).
    detached: bool,
}

// ------------------------------------------------------------------- sessions

impl CloudSessions {
    /// Registers the Docker sandbox backend with the given `sbx` runner, ssh seam and secret
    /// source. Without it every sandbox start is refused.
    pub fn with_sandboxes(
        mut self,
        runner: Arc<dyn SbxRunner>,
        shell: Arc<dyn SandboxShell>,
        secrets: Arc<dyn SandboxSecrets>,
    ) -> Self {
        let backend = Arc::new(DockerBackend::new(runner.clone()).with_shell(shell.clone()));
        self.backends
            .insert(CloudBackendKind::DockerSandbox, backend.clone());
        self.sandbox = Some(SandboxDeps {
            runner,
            shell,
            secrets,
            backend,
            home: crate::accounts::home().unwrap_or_default(),
        });
        self
    }
    /// Uses `home` to look for the `sbx_cloud` ssh alias.
    #[cfg(test)]
    pub(super) fn with_sandbox_home(mut self, home: PathBuf) -> Self {
        if let Some(deps) = self.sandbox.as_mut() {
            deps.home = home;
        }
        self
    }
    fn sandbox_deps(&self) -> Result<SandboxDeps, String> {
        self.sandbox
            .clone()
            .ok_or_else(|| "Docker sandboxes are not available.".to_string())
    }

    /// Step 1 of a sandbox start, see the module docs. `Err` means no sandbox was created by
    /// this call. A create that timed out without the sandbox showing up is `Ok` with a
    /// failed record, so the session offers Retry (which reconciles by name).
    pub fn begin_sandbox_start(
        &self,
        local_cwd: &str,
        config: &DockerProjectConfig,
        branch: &str,
        harness: &str,
    ) -> Result<(Session, StartRecord), String> {
        require_claude(harness)?;
        let deps = self.sandbox_deps()?;
        let config = config.validated()?;
        let branch = branch.trim().to_string();
        if !valid_ref(&branch) {
            return Err("The branch name contains invalid characters.".into());
        }
        compute_of(&config)?;
        let (_, _, path) = git_credentials::normalize_origin(&config.repo_url)?;
        let prerequisites = sandboxes::diagnose(&*deps.runner, &deps.home);
        if !prerequisites.is_ready() {
            return Err(prerequisites.message());
        }
        let began = self.now();
        let request_id = uuid::Uuid::new_v4().to_string();
        let name = sandboxes::sandbox_name_for(&branch, &request_id);
        let repo_dir = repo_dir_of(&path);
        let start = SandboxStart {
            request_id,
            config,
            sandbox_id: None,
            repo_dir: repo_dir.clone(),
            workspace: None,
            container: None,
            relay_port: DEFAULT_RELAY_PORT,
            rebuild: false,
        };
        let remote_cwd = format!("/workspaces/{repo_dir}");
        let mut record =
            fresh_sandbox_record(&name, local_cwd, &path, &branch, &remote_cwd, start, began);
        let session = {
            let store = self.store()?;
            let project = pending_cloud_project(local_cwd, &CloudRef::sandbox(&name), &remote_cwd);
            let session = store.create_pending_cloud_session(harness, project)?;
            record.session_id = session.id;
            // Persisted BEFORE `sbx create`: a crash from here on finds the name again.
            if let Err(error) = save_start_record(&store, &record) {
                let _ = store.forget_cloud_session(session.id);
                return Err(error);
            }
            session
        };
        match self.ensure_created(&deps, &mut record, false) {
            Ok(()) => {
                let now = self.now();
                record.finish_step(StepKind::Create, now);
                record.step_mut(StepKind::Create).detail = Some(name.clone());
                record.start_step(StepKind::Provision, now);
                record.step_mut(StepKind::Provision).detail = Some("Waiting to start".into());
                self.persist_start(&record);
                Ok((session, record))
            }
            Err(failure) => {
                if failure.ambiguous {
                    record.fail(failure.message, self.now());
                    self.persist_start(&record);
                    return Ok((session, record));
                }
                let store = self.store()?;
                let _ = remove_start_record(&store, session.id);
                let _ = store.forget_cloud_session(session.id);
                Err(failure.message)
            }
        }
    }

    /// Creates the sandbox of `record`, or adopts the one already called that, and persists
    /// its id. Never creates a second sandbox for one name: `lookup_first` (a retry, where an
    /// earlier create may have gone through) looks the name up before creating, and `create`
    /// itself fails with `already_exists` when a call did get through.
    ///
    /// A failure without an id is ambiguous (the sandbox may exist and bill) unless the text
    /// is a clear pre-flight refusal, see [`is_clear_refusal`].
    fn ensure_created(
        &self,
        deps: &SandboxDeps,
        record: &mut StartRecord,
        lookup_first: bool,
    ) -> Result<(), CreateFailure> {
        let name = record.sandbox.clone().unwrap_or_default();
        let start = record
            .sandbox_start
            .as_ref()
            .ok_or_else(|| CreateFailure::definite("This start has no sandbox data.".into()))?;
        let compute = compute_of(&start.config).map_err(CreateFailure::definite)?;
        let mut found = None;
        if lookup_first {
            found = sandboxes::find_by_name(&*deps.runner, &name)
                .map_err(|e| CreateFailure {
                    message: e,
                    ambiguous: true,
                })?
                .map(|info| info.id);
        }
        let id = match found {
            Some(id) => id,
            None => match sandboxes::create(&*deps.runner, &name, &compute) {
                Ok(id) => id,
                Err(error) => match error.id {
                    Some(id) => id,
                    None => match sandboxes::find_by_name(&*deps.runner, &name) {
                        Ok(Some(found)) => found.id,
                        Ok(None) => {
                            return Err(CreateFailure {
                                ambiguous: !is_clear_refusal(&error.message),
                                message: error.message,
                            })
                        }
                        Err(lookup) => {
                            return Err(CreateFailure {
                                ambiguous: true,
                                message: format!("{} ({lookup})", error.message),
                            })
                        }
                    },
                },
            },
        };
        deps.backend.remember(&name, &id);
        if let Some(start) = record.sandbox_start.as_mut() {
            start.sandbox_id = Some(id);
        }
        self.persist_start(record);
        Ok(())
    }

    /// `run_start` for a sandbox record; see the module docs.
    pub(super) fn run_sandbox_start(
        &self,
        mut record: StartRecord,
        events: &mut dyn FnMut(StartEvent),
        cancel: &AtomicBool,
    ) -> Result<CloudBinding, String> {
        // Without the sandbox seams nothing is touched, the record included.
        self.sandbox_deps()?;
        let session_id = record.session_id;
        record.reset_for_run();
        if cancel.load(Ordering::Relaxed) {
            return Err(START_ABORTED.into());
        }
        {
            let store = self.store()?;
            if start_record(&store, session_id).is_none() {
                return Err(START_ABORTED.into());
            }
            save_start_record(&store, &record)?;
        }
        record.log_tail.clear();
        self.publish(&record, events);
        match self.drive_sandbox_start(&mut record, events, cancel) {
            Ok((binding, session)) => {
                events(StartEvent::Ready {
                    binding: binding.clone(),
                    session,
                });
                Ok(binding)
            }
            Err(Stop::Aborted) => Err(START_ABORTED.into()),
            Err(Stop::Failed(error)) => {
                let kept = record
                    .sandbox_start
                    .as_ref()
                    .is_some_and(|s| s.sandbox_id.is_some());
                let error = match (&record.sandbox, kept && !error.starts_with(SANDBOX_GONE)) {
                    (Some(name), true) => format!(
                        "{error} (sandbox {name} was kept; Retry resumes this start, or delete the session to remove it)"
                    ),
                    _ => error,
                };
                record.fail(error.clone(), self.now());
                self.persist_start(&record);
                events(StartEvent::Failed(record));
                Err(error)
            }
        }
    }

    fn drive_sandbox_start(
        &self,
        record: &mut StartRecord,
        events: &mut dyn FnMut(StartEvent),
        cancel: &AtomicBool,
    ) -> Result<(CloudBinding, Session), Stop> {
        let deps = self.sandbox_deps()?;
        let session_id = record.session_id;
        let name = record.sandbox.clone().ok_or("This start has no sandbox.")?;
        let config = record
            .sandbox_start
            .as_ref()
            .ok_or("This start has no sandbox data.")?
            .config
            .clone();
        // Secrets are read once, up front, and only kept to be masked in what we show.
        let auth = deps.secrets.claude_auth(config.claude_account.as_deref())?;
        let payload = match &config.credential_id {
            Some(credential) => Some(deps.secrets.git_payload(credential, &config.repo_url)?),
            None => None,
        };
        let mut scrub = Scrub(vec![]);
        if let Some(auth) = &auth {
            scrub.0.push(Zeroizing::new(auth.secret().to_string()));
        }
        if let Some(password) = payload.as_ref().and_then(|p| payload_password(p)) {
            scrub.0.push(Zeroizing::new(password));
        }
        let mut run = Run {
            record,
            events,
            cancel,
            deps,
            scrub,
            detached: false,
        };

        self.sandbox_create_step(&mut run, &name)?;
        self.sandbox_provision_step(&mut run, payload.as_deref().map(String::as_str))?;
        self.sandbox_build_step(&mut run)?;
        self.sandbox_configure_step(&mut run)?;
        let target = self.sandbox_connect_step(&mut run, &name, auth.as_ref())?;
        let (binding, project) = self.sandbox_open_step(&mut run, &name, &target)?;
        self.sandbox_lifecycle_step(&mut run);

        if self.start_aborted(session_id, cancel) {
            return Err(Stop::Aborted);
        }
        let session = {
            let store = self.store()?;
            match store.rebind_cloud_session(session_id, project) {
                Ok(session) => session,
                Err(_) if start_record(&store, session_id).is_none() => return Err(Stop::Aborted),
                Err(error) => return Err(Stop::Failed(error)),
            }
        };
        let at = self.now();
        let record = run.record;
        record.finish_step(StepKind::OpenProject, at);
        if record.step(StepKind::BlockingCommands).status == StepStatus::Pending {
            record.step_mut(StepKind::BlockingCommands).status = StepStatus::Skipped;
        }
        record.finished_at = Some(at);
        (run.events)(StartEvent::Updated(record.clone()));
        remove_start_record(&*self.store()?, session_id)?;
        self.invalidate_ref(&CloudRef::sandbox(&name));
        Ok((binding, session))
    }

    /// Drops the paired machines of sandbox `name` (local record and tunnel only, no ssh).
    fn forget_sandbox_machines(&self, name: &str) {
        if let Ok(machines) = self.host.machines() {
            for machine in machines {
                if machine.sandbox.as_ref().is_some_and(|s| s.name == name) {
                    let _ = self.host.disconnect(&machine.id);
                }
            }
        }
    }

    fn aborted(&self, run: &Run) -> bool {
        self.run_halted(run.record.session_id, run.cancel, run.detached)
    }

    fn run_halted(&self, session_id: i64, cancel: &AtomicBool, detached: bool) -> bool {
        if detached {
            cancel.load(Ordering::Relaxed)
        } else {
            self.start_aborted(session_id, cancel)
        }
    }

    fn log_line(&self, run: &mut Run, text: &str) {
        let text = run.scrub.apply(text);
        if text.is_empty() {
            return;
        }
        run.record.push_log(&text);
        (run.events)(StartEvent::Log(text));
    }
    fn progress_line(&self, run: &mut Run, text: &str) {
        self.log_line(run, &format!("{PROGRESS_PREFIX}{text}"));
    }
    fn set_detail(&self, run: &mut Run, kind: StepKind, detail: &str) {
        run.record.step_mut(kind).detail = Some(detail.to_string());
        self.publish(run.record, run.events);
    }

    /// Runs `remote` in the outer sandbox `id`. Lines reach the log (markers excepted).
    fn sandbox_exec(
        &self,
        run: &mut Run,
        id: &str,
        remote: &str,
        stdin: &str,
        timeout: Duration,
        what: &str,
    ) -> Result<ShellOutput, Stop> {
        let shell = run.deps.shell.clone();
        let cancel = run.cancel;
        let session_id = run.record.session_id;
        let detached = run.detached;
        let abort = || self.run_halted(session_id, cancel, detached);
        let result = {
            let mut on_line = |line: &str| {
                if !line.trim_start().starts_with("MONOPTAH_") {
                    self.log_line(run, line);
                }
            };
            shell.run(id, remote, stdin, &mut on_line, &abort, timeout)
        };
        let out = result.map_err(|e| Stop::from(run.scrub.apply(&e)))?;
        if !out.success {
            return Err(Stop::Failed(failure_text(&out, &run.scrub, what)));
        }
        Ok(out)
    }

    /// Puts the Git credential of `config` into the sandbox's in-memory cache again (it
    /// expires after [`CREDENTIAL_CACHE_SECONDS`] and does not survive a daemon restart).
    /// A no-op for a public repo. Any ssh resumes a stopped sandbox, so only call this for
    /// a user action such as a reconnect; B2b's reconnect uses it.
    pub fn deliver_git_credential(
        &self,
        sandbox_id: &str,
        config: &DockerProjectConfig,
        abort: &dyn Fn() -> bool,
    ) -> Result<(), String> {
        let deps = self.sandbox_deps()?;
        sandboxes::validate_sandbox_id(sandbox_id)?;
        let config = config.validated()?;
        let Some(credential) = &config.credential_id else {
            return Ok(());
        };
        let payload = deps.secrets.git_payload(credential, &config.repo_url)?;
        let mut scrub = Scrub(vec![]);
        if let Some(password) = payload_password(&payload) {
            scrub.0.push(Zeroizing::new(password));
        }
        let out = deps
            .shell
            .run(
                sandbox_id,
                &approve_remote(),
                &payload,
                &mut |_| {},
                abort,
                SHORT,
            )
            .map_err(|e| scrub.apply(&e))?;
        if out.success {
            Ok(())
        } else {
            Err(failure_text(&out, &scrub, "Delivering the Git credential"))
        }
    }

    fn sandbox_id(run: &Run) -> Result<String, Stop> {
        run.record
            .sandbox_start
            .as_ref()
            .and_then(|s| s.sandbox_id.clone())
            .ok_or_else(|| Stop::Failed("The sandbox id is not known yet.".into()))
    }

    // ---- Create

    fn sandbox_create_step(&self, run: &mut Run, name: &str) -> Result<(), Stop> {
        let at = self.now();
        if run
            .record
            .sandbox_start
            .as_ref()
            .is_some_and(|s| s.sandbox_id.is_none())
        {
            run.record.start_step(StepKind::Create, at);
            self.publish(run.record, run.events);
            self.progress_line(run, "Creating the sandbox");
            let deps = run.deps.clone();
            self.ensure_created(&deps, run.record, true)
                .map_err(|failure| Stop::Failed(run.scrub.apply(&failure.message)))?;
        }
        let id = Self::sandbox_id(run)?;
        run.deps.backend.remember(name, &id);
        // Reconcile: the sandbox may be stopped, or gone. Never recreated.
        match sandboxes::inspect(&*run.deps.runner, &id) {
            Ok(info) => {
                let state = info.state.trim().to_ascii_lowercase();
                match state.as_str() {
                    "deleted" | "terminated" | "deleting" => {
                        return Err(Stop::Failed(SANDBOX_GONE.into()));
                    }
                    // ssh into a broken sandbox can not fix it and may bill.
                    "failed" | "error" => {
                        return Err(Stop::Failed(format!(
                            "The sandbox is in state {state}. Delete this session and start a new one."
                        )));
                    }
                    _ => {}
                }
            }
            Err(error) if sandboxes::is_deleted(&error, &id) => {
                return Err(Stop::Failed(SANDBOX_GONE.into()));
            }
            Err(error) => return Err(Stop::Failed(run.scrub.apply(&error))),
        }
        run.record.finish_step(StepKind::Create, self.now());
        run.record.step_mut(StepKind::Create).detail = Some(name.to_string());
        self.publish(run.record, run.events);
        if self.aborted(run) {
            return Err(Stop::Aborted);
        }
        Ok(())
    }

    // ---- Provision

    fn sandbox_provision_step(&self, run: &mut Run, payload: Option<&str>) -> Result<(), Stop> {
        let id = Self::sandbox_id(run)?;
        let start = run
            .record
            .sandbox_start
            .clone()
            .ok_or("Missing sandbox data.")?;
        let branch = run.record.branch.clone().unwrap_or_default();
        let clone_url = git_credentials::clone_url(&start.config.repo_url)?;
        let provisioned = run.record.step(StepKind::Provision).status == StepStatus::Done
            && start.workspace.is_some();
        let vars = |phase: &'static str| {
            with_vars(
                &[
                    ("PHASE", phase),
                    ("REPO_URL", &clone_url),
                    ("REPO_DIR", &start.repo_dir),
                    ("BASE_REF", &start.config.base_ref),
                    ("BRANCH", &branch),
                ],
                PROVISION_SCRIPT,
            )
        };
        if !provisioned {
            self.set_detail(run, StepKind::Provision, "Installing tools");
            self.sandbox_exec(
                run,
                &id,
                SH_S,
                &vars("setup"),
                LONG,
                "Preparing the sandbox",
            )?;
        }
        if let Some(payload) = payload {
            // The script is non-secret text in argv; the credential is stdin only.
            self.set_detail(run, StepKind::Provision, "Delivering the Git credential");
            let remote = approve_remote();
            self.sandbox_exec(
                run,
                &id,
                &remote,
                payload,
                SHORT,
                "Delivering the Git credential",
            )?;
        }
        if provisioned {
            return Ok(());
        }
        if self.aborted(run) {
            return Err(Stop::Aborted);
        }
        self.set_detail(run, StepKind::Provision, "Cloning the repository");
        let out = self.sandbox_exec(
            run,
            &id,
            SH_S,
            &vars("clone"),
            LONG,
            "Cloning the repository",
        )?;
        let workspace = out
            .stdout
            .lines()
            .rev()
            .find_map(|l| marker(l, "MONOPTAH_WORKSPACE"))
            .map(str::to_string)
            .ok_or("The clone did not report its folder.")?;
        validate_sandbox_workspace(&workspace)?;
        if let Some(start) = run.record.sandbox_start.as_mut() {
            start.workspace = Some(workspace);
        }
        run.record.finish_step(StepKind::Provision, self.now());
        self.publish(run.record, run.events);
        if self.aborted(run) {
            return Err(Stop::Aborted);
        }
        Ok(())
    }

    // ---- Build container

    fn up_vars(run: &Run, mode: &str) -> Result<String, Stop> {
        let start = run
            .record
            .sandbox_start
            .as_ref()
            .ok_or("Missing sandbox data.")?;
        let workspace = start
            .workspace
            .as_deref()
            .ok_or("The workspace is not known yet.")?;
        let config = config_arg(&start.config);
        Ok(with_vars(
            &[
                ("MODE", mode),
                ("WS", workspace),
                ("CONFIG", config.as_deref().unwrap_or("")),
                (
                    "MOUNT",
                    if start.config.credential_id.is_some() {
                        "1"
                    } else {
                        "0"
                    },
                ),
                ("MOUNT_DST", CONTAINER_GIT_DIR),
            ],
            UP_SCRIPT,
        ))
    }

    fn sandbox_build_step(&self, run: &mut Run) -> Result<(), Stop> {
        let id = Self::sandbox_id(run)?;
        let rebuild = run.record.sandbox_start.as_ref().is_some_and(|s| s.rebuild);
        let name = run.record.sandbox.clone().unwrap_or_default();
        let at = self.now();
        run.record.start_step(StepKind::BuildContainer, at);
        self.publish(run.record, run.events);
        // `devcontainer up` is idempotent and quick for an existing container, so every run
        // does it: a resumed sandbox may have a stopped container.
        let mode = if rebuild { "rebuild" } else { "start" };
        let vars = Self::up_vars(run, mode)?;
        self.sandbox_exec(run, &id, SH_S, &vars, SHORT, "Starting the container build")?;
        let follow_vars = Self::up_vars(run, "follow")?;
        let shell = run.deps.shell.clone();
        let mut delivered = 0usize;
        let mut exit: Option<String> = None;
        let mut outcome: Option<UpLine> = None;
        let mut last_error = String::from("Lost the connection to the sandbox.");
        for attempt in 0..FOLLOW_ATTEMPTS {
            if attempt > 0 {
                (self.sleep)(self.follower_retry);
            }
            // Only `L `-prefixed lines are log lines (ssh noise is not), so the count is exact
            // and a re-follow resumes at the right line.
            let follow = format!("OFFSET='{delivered}'\n{follow_vars}");
            let session_id = run.record.session_id;
            let cancel = run.cancel;
            let detached = run.detached;
            let abort = || self.run_halted(session_id, cancel, detached);
            let result = {
                let mut on_line = |line: &str| {
                    if let Some(log) = line.strip_prefix("L ") {
                        delivered += 1;
                        self.handle_up_line(run, log, &mut outcome);
                    } else if let Some(code) = marker(line, "MONOPTAH_UP_EXIT") {
                        exit = Some(code.trim().to_string());
                    }
                };
                shell.run(&id, SH_S, &follow, &mut on_line, &abort, LONG)
            };
            match result {
                Err(error) => {
                    let error = run.scrub.apply(&error);
                    if Stop::from(error.clone()).is_aborted() {
                        return Err(Stop::Aborted);
                    }
                    last_error = error;
                }
                Ok(out) if !out.success && exit.is_none() => {
                    last_error = failure_text(&out, &run.scrub, "Following the build");
                }
                Ok(_) => {}
            }
            if exit.is_some() {
                break;
            }
        }
        let exit = exit.ok_or(Stop::Failed(last_error))?;
        let (container, message) = match &outcome {
            Some(UpLine::Outcome {
                success: true,
                container_id: Some(id),
                remote_user,
                remote_workspace: Some(workspace),
                ..
            }) if exit == "0" => (
                Some(ContainerInfo {
                    id: id.clone(),
                    remote_user: remote_user.clone().unwrap_or_else(|| "root".into()),
                    remote_workspace: workspace.clone(),
                }),
                None,
            ),
            Some(UpLine::Outcome {
                message: Some(message),
                ..
            }) => (None, Some(message.clone())),
            _ => (None, None),
        };
        let Some(container) = container else {
            let detail = match (message, exit.as_str()) {
                (Some(message), _) => run.scrub.apply(&message),
                (None, "gone") => "The container build stopped unexpectedly. Retry.".into(),
                (None, code) => format!("`devcontainer up` exited with {code}"),
            };
            return Err(Stop::Failed(format!(
                "Building the container failed: {detail}"
            )));
        };
        if !valid_container_id(&container.id)
            || !valid_user(&container.remote_user)
            || validate_sandbox_workspace(&container.remote_workspace).is_err()
        {
            return Err(Stop::Failed(
                "The Dev Container CLI reported an unusable container.".into(),
            ));
        }
        let at = self.now();
        run.record.finish_step(StepKind::BuildContainer, at);
        if run.record.step(StepKind::BlockingCommands).status == StepStatus::Pending {
            run.record.step_mut(StepKind::BlockingCommands).status = StepStatus::Skipped;
        } else {
            run.record.finish_step(StepKind::BlockingCommands, at);
        }
        // A new container has a fresh Host identity: the paired machine would fail with
        // "Host identity changed" forever.
        let changed = run
            .record
            .sandbox_start
            .as_ref()
            .and_then(|s| s.container.as_ref())
            .is_some_and(|old| old.id != container.id);
        if changed || rebuild {
            self.forget_sandbox_machines(&name);
        }
        if let Some(start) = run.record.sandbox_start.as_mut() {
            start.container = Some(container);
            start.rebuild = false;
        }
        self.publish(run.record, run.events);
        if self.aborted(run) {
            return Err(Stop::Aborted);
        }
        Ok(())
    }

    /// One log line of `devcontainer up`: shown, and mapped to steps.
    fn handle_up_line(&self, run: &mut Run, line: &str, outcome: &mut Option<UpLine>) {
        let parsed = parse_up_line(line);
        match &parsed {
            UpLine::Progress { name, status } => {
                self.log_line(run, &format!("{name} ({status})"));
                if starts_blocking_commands(name, status) {
                    let at = self.now();
                    if run.record.step(StepKind::BlockingCommands).status == StepStatus::Pending {
                        run.record.finish_step(StepKind::BuildContainer, at);
                        run.record.start_step(StepKind::BlockingCommands, at);
                        self.publish(run.record, run.events);
                    }
                }
            }
            UpLine::Text(text) => self.log_line(run, text),
            UpLine::Plain(text) => self.log_line(run, text),
            UpLine::Outcome {
                success, message, ..
            } => {
                if *success {
                    self.log_line(run, "Container ready");
                } else {
                    let message = message.clone().unwrap_or_else(|| "failed".into());
                    self.log_line(run, &format!("Container build failed: {message}"));
                }
                *outcome = Some(parsed.clone());
            }
        }
    }

    // ---- Configure environment

    fn container_target(&self, run: &Run) -> Result<(SandboxTarget, ContainerInfo), Stop> {
        let start = run
            .record
            .sandbox_start
            .as_ref()
            .ok_or("Missing sandbox data.")?;
        let container = start
            .container
            .clone()
            .ok_or("The container is not known yet.")?;
        let target = SandboxTarget {
            id: Self::sandbox_id(run)?,
            name: run.record.sandbox.clone().unwrap_or_default(),
            workspace: start
                .workspace
                .clone()
                .ok_or("The workspace is not known yet.")?,
            relay_port: start.relay_port,
            config: config_arg(&start.config),
        };
        crate::remote::validate_sandbox_target(&target)?;
        Ok((target, container))
    }

    fn sandbox_configure_step(&self, run: &mut Run) -> Result<(), Stop> {
        let (target, _) = self.container_target(run)?;
        run.record
            .start_step(StepKind::ConfigureCodespace, self.now());
        self.publish(run.record, run.events);
        let start = run
            .record
            .sandbox_start
            .clone()
            .ok_or("Missing sandbox data.")?;
        let clone_url = git_credentials::clone_url(&start.config.repo_url)?;
        let socket = format!("{CONTAINER_GIT_DIR}/sock");
        let script = with_vars(
            &[
                (
                    "WANT_CRED",
                    if start.config.credential_id.is_some() {
                        "1"
                    } else {
                        "0"
                    },
                ),
                ("CLONE_URL", &clone_url),
                ("SOCKET", &socket),
            ],
            GIT_CONFIG_SCRIPT,
        );
        let remote = target.exec_in_container(&["sh", "-l", "-s"]);
        self.sandbox_exec(
            run,
            &target.id,
            &remote,
            &script,
            SHORT,
            "Configuring the environment",
        )?;
        run.record
            .finish_step(StepKind::ConfigureCodespace, self.now());
        self.publish(run.record, run.events);
        if self.aborted(run) {
            return Err(Stop::Aborted);
        }
        Ok(())
    }

    // ---- Connect

    fn sandbox_connect_step(
        &self,
        run: &mut Run,
        name: &str,
        auth: Option<&ClaudeAuth>,
    ) -> Result<SandboxTarget, Stop> {
        let _ = name;
        let (target, container) = self.container_target(run)?;
        run.record.start_step(StepKind::Connect, self.now());
        self.publish(run.record, run.events);

        // 1. The Host, with the pinned Claude credential in its environment (stdin only).
        self.progress_line(run, "Starting Monoptah Host in the sandbox");
        let prelude = auth.map(ClaudeAuth::prelude);
        if prelude.is_none() {
            self.progress_line(
                run,
                "No Claude credential is saved for this account; Claude will need to sign in",
            );
        }
        let script = Zeroizing::new(sandbox_bootstrap_script(
            prelude.as_deref().map(String::as_str),
        ));
        let remote = target.exec_in_container(&["sh", "-l", "-s"]);
        let out =
            self.sandbox_exec(run, &target.id, &remote, &script, LONG, "Starting the Host")?;
        let host_port = out
            .stdout
            .lines()
            .last()
            .and_then(|l| serde_json::from_str::<serde_json::Value>(l).ok())
            .and_then(|v| v.get("port").and_then(serde_json::Value::as_u64))
            .and_then(|p| u16::try_from(p).ok())
            .filter(|p| *p > 0)
            .unwrap_or(DEFAULT_HOST_PORT);
        if self.aborted(run) {
            return Err(Stop::Aborted);
        }

        // 2. The relay in the outer sandbox.
        self.progress_line(run, "Starting the relay");
        let port = target.relay_port.to_string();
        let host_port = host_port.to_string();
        let script = with_vars(
            &[
                ("CID", &container.id),
                ("RUN_USER", &container.remote_user),
                ("PORT", &port),
                ("HOST_PORT", &host_port),
            ],
            RELAY_SCRIPT,
        );
        self.sandbox_exec(run, &target.id, SH_S, &script, SHORT, "Starting the relay")?;
        if self.aborted(run) {
            return Err(Stop::Aborted);
        }
        Ok(target)
    }

    // ---- Open project

    fn sandbox_open_step(
        &self,
        run: &mut Run,
        name: &str,
        target: &SandboxTarget,
    ) -> Result<(CloudBinding, crate::store::RemoteProject), Stop> {
        let (_, container) = self.container_target(run)?;
        let cloud = CloudRef::sandbox(name);
        // The evidence setup and the later archive copy exec into this container.
        run.deps.backend.remember_target(target);
        let backend = self.backend(CloudBackendKind::DockerSandbox)?;
        let request = OpenRequest {
            local_cwd: &run.record.local_project.clone(),
            cloud: &cloud,
            sandbox: Some(target),
            display: &run.record.display.clone(),
            repo: &run.record.repo.clone(),
            remote_cwd: &container.remote_workspace,
        };
        let cell = std::cell::RefCell::new((&mut *run.record, &mut *run.events));
        let halt = AtomicBool::new(false);
        let cancel = run.cancel;
        let session_id = cell.borrow().0.session_id;
        // `open_project` polls `halt` between its sub-steps; fold cancel and deletion in.
        if self.start_aborted(session_id, cancel) {
            return Err(Stop::Aborted);
        }
        let result = super::open_project(
            &*backend,
            &*self.host,
            &self.store,
            &request,
            &halt,
            &mut |kind, status| {
                let mut guard = cell.borrow_mut();
                let (record, events) = &mut *guard;
                let at = self.now();
                if status == StepStatus::Running {
                    record.start_step(kind, at);
                } else {
                    record.finish_step(kind, at);
                }
                self.publish(record, &mut **events);
            },
            &mut |line| {
                let mut guard = cell.borrow_mut();
                let (record, events) = &mut *guard;
                let text = format!("{PROGRESS_PREFIX}{}", run.scrub.apply(line));
                record.push_log(&text);
                (**events)(StartEvent::Log(text));
            },
        );
        let opened = result.map_err(|e| Stop::from(run.scrub.apply(&e)))?;
        // Remembered so a later reconnect can tell a recreated container (a new Host) from
        // the same one before it touches the paired machine.
        if let Ok(store) = self.store() {
            let _ = store.save_setting(&container_key(name), &container.id);
        }
        Ok(opened)
    }

    // ---- Lifecycle

    /// Starts `devcontainer run-user-commands` detached. Best effort: a failure is only
    /// logged and never blocks `Ready`.
    fn sandbox_lifecycle_step(&self, run: &mut Run) {
        let Ok(id) = Self::sandbox_id(run) else {
            return;
        };
        run.record
            .start_step(StepKind::LifecycleCommands, self.now());
        let Ok(vars) = Self::up_vars(run, "lifecycle") else {
            return;
        };
        match self.sandbox_exec(
            run,
            &id,
            SH_S,
            &vars,
            SHORT,
            "Starting the background commands",
        ) {
            Ok(_) => {
                run.record
                    .finish_step(StepKind::LifecycleCommands, self.now());
                run.record.step_mut(StepKind::LifecycleCommands).detail =
                    Some("Running in the background".into());
            }
            Err(Stop::Failed(error)) => {
                self.progress_line(run, &error);
                run.record.step_mut(StepKind::LifecycleCommands).status = StepStatus::Skipped;
            }
            Err(Stop::Aborted) => {}
        }
        self.publish(run.record, run.events);
    }

    // ---- Rebuild

    /// `rebuild_start` for a sandbox: the next `run_start` runs `devcontainer up
    /// --remove-existing-container` in the same sandbox. The workspace is kept.
    pub(super) fn rebuild_sandbox_start(
        &self,
        mut record: StartRecord,
    ) -> Result<StartRecord, String> {
        let session_id = record.session_id;
        let now = self.now();
        let provisioned = record
            .sandbox_start
            .as_ref()
            .is_some_and(|s| s.workspace.is_some());
        if !provisioned {
            return Err("This sandbox is not provisioned yet. Retry the start instead.".into());
        }
        for step in &mut record.steps {
            if !matches!(step.kind, StepKind::Create | StepKind::Provision) {
                *step = Step::pending(step.kind);
            }
        }
        if let Some(start) = record.sandbox_start.as_mut() {
            start.rebuild = true;
        }
        if let Some(name) = record.sandbox.clone() {
            self.forget_sandbox_machines(&name);
        }
        record.error = None;
        record.failed_at = None;
        record.finished_at = None;
        record.log_tail.clear();
        record.started_at = now;
        record.step_mut(StepKind::Provision).detail = Some("Rebuilding".into());
        let store = self.store()?;
        if start_record(&store, session_id).is_none() {
            return Err(START_ABORTED.into());
        }
        save_start_record(&store, &record)?;
        Ok(record)
    }
}

/// True for a `create` failure that is certainly a refusal before anything was made
/// (validation, sign-in, plan or quota); every other failure may have created a sandbox.
fn is_clear_refusal(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    [
        "quota",
        "plan limit",
        "limit exceeded",
        "sbx login",
        "not logged in",
        "not signed in",
        "unauthenticated",
        "invalid",
        "billable shape",
        "unsupported platform",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

impl Stop {
    fn is_aborted(&self) -> bool {
        matches!(self, Stop::Aborted)
    }
}

/// Why [`CloudSessions::ensure_created`] failed.
struct CreateFailure {
    message: String,
    /// The create may still have gone through (no answer in time, lookup failed).
    ambiguous: bool,
}
impl CreateFailure {
    fn definite(message: String) -> Self {
        Self {
            message,
            ambiguous: false,
        }
    }
}

/// Setting that remembers the id of the container a sandbox's Host lives in.
pub(super) fn container_key(name: &str) -> String {
    format!("sandbox-container:{name}")
}

/// `--config` argument: the devcontainer path, unless it is the default one.
fn config_arg(config: &DockerProjectConfig) -> Option<String> {
    (config.devcontainer_path != super::docker_config::DEFAULT_DEVCONTAINER)
        .then(|| config.devcontainer_path.clone())
}

fn fresh_sandbox_record(
    name: &str,
    local_cwd: &str,
    repo: &str,
    branch: &str,
    remote_cwd: &str,
    start: SandboxStart,
    now: u64,
) -> StartRecord {
    let machine = format!("{} CPU / {}", start.config.cpus, start.config.memory);
    let mut steps: Vec<Step> = StepKind::ALL.iter().map(|k| Step::pending(*k)).collect();
    steps[0].status = StepStatus::Running;
    steps[0].started_at = Some(now);
    StartRecord {
        session_id: 0,
        local_project: local_cwd.into(),
        codespace: String::new(),
        sandbox: Some(name.into()),
        repo: repo.into(),
        branch: Some(branch.into()),
        machine: Some(machine),
        display: crate::codespaces::display_name_for(branch),
        remote_cwd: remote_cwd.into(),
        web_url: None,
        created: true,
        gh_state: None,
        steps,
        error: None,
        failed_at: None,
        log_tail: vec![],
        started_at: now,
        finished_at: None,
        track_ready: false,
        sandbox_start: Some(start),
    }
}
