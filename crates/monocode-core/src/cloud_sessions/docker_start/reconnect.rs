//! Explicit reconnect of a Docker sandbox session.
//!
//! A stopped sandbox keeps its disk and memory, but background code must never wake it (any
//! ssh resumes and bills it), so `Connections::request` and `ensure_codespace_with` answer
//! `SANDBOX_NOT_CONNECTED` and stay that way. [`CloudSessions::reconnect_sandbox`] is the one
//! door for an explicit wake: a click on Reconnect, a send typed in the session's own view, or
//! a click on a file link in its transcript (the app reconnects, then reads the file), or a send /
//! `cloud.resume` through the app API (a Mono included, via `CloudApi::wake`). It
//! BLOCKS for minutes, so callers use a worker thread, never the UI thread. Background polling
//! and listings never reach it.
//!
//! It never creates a sandbox. It inspects the state first (read-only), then re-runs the
//! idempotent parts of the start flow that a stop or resume can lose, in this order:
//!
//! 1. `devcontainer up` (mode `start`): the container may be stopped or recreated. A plain
//!    sandbox (no devcontainer, [`SandboxMode::Plain`]) has no container: steps 1 and 3 to
//!    the relay are skipped, the Host runs in the outer sandbox and the tunnel goes straight
//!    to it.
//! 2. The Git credential back into the in-memory cache (it expires and dies with a daemon
//!    restart), when the project config names one.
//! 3. The container's git config for the exact repo URL.
//! 4. The Host bootstrap WITH the Claude prelude (stdin only), then the relay (Container
//!    mode only).
//! 5. [`open_project`](super::super::open_project): drops the stale machine, pairs afresh
//!    (a new tunnel), opens the project again.
//!
//! # Crash race
//!
//! The Host bootstrap runs twice: here with the Claude prelude, then again without it when
//! [`open_project`](super::super::open_project) pairs through `Connections` (the same order
//! as a start). The Host tracks the Claude token by hash. If the app dies between the two,
//! the Host keeps the environment of the first run; the next reconnect repeats both, so the
//! state converges. Until then a Host restarted by the second bootstrap may lack the token
//! (Claude asks to sign in). Real-sandbox behaviour of that restart is unverified.
//!
//! A new container has a new Host identity, so the session's history is gone with the old
//! one: that is reported as an error instead of silently rebinding the session.
use super::*;
use crate::cloud_sessions::startup::STILL_STARTING;
use crate::cloud_sessions::CloudBackend;
use crate::cloud_sessions::{CloudState, OpenRequest};

/// How old the last Git credential delivery may be before an already-connected reconnect
/// repeats it (the cache itself keeps a token for 24 hours).
const CREDENTIAL_REFRESH_MS: u64 = 12 * 60 * 60 * 1000;

fn credential_key(name: &str) -> String {
    format!("sandbox-credential-at:{name}")
}

pub(super) const RECREATED: &str =
    "The sandbox container was recreated, so this session's history is gone. Start a new session.";

/// Why [`CloudSessions::reconnect_sandbox_typed`] failed. The archive flow tells the first two
/// apart from the rest: they do not stop it from saving the files (see
/// `CloudSessions::convert_session`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ReconnectError {
    /// The devcontainer was recreated: its Host is new and the session's history stayed in
    /// the old container. The workspace (a bind mount) survived.
    Recreated,
    /// The sandbox state was fine but bringing the container and Host back failed.
    HostUnreachable(String),
    /// Anything else (missing session, deleted or stopping sandbox, cancel, ...).
    Failed(String),
}
impl std::fmt::Display for ReconnectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Recreated => f.write_str(RECREATED),
            Self::HostUnreachable(text) | Self::Failed(text) => f.write_str(text),
        }
    }
}
impl From<String> for ReconnectError {
    fn from(text: String) -> Self {
        Self::Failed(text)
    }
}
impl From<&str> for ReconnectError {
    fn from(text: &str) -> Self {
        Self::Failed(text.to_string())
    }
}

/// Removes the in-flight mark of a reconnect, however the call ends.
struct InFlight<'a> {
    set: &'a Mutex<std::collections::HashSet<String>>,
    name: String,
}
impl Drop for InFlight<'_> {
    fn drop(&mut self) {
        if let Ok(mut set) = self.set.lock() {
            set.remove(&self.name);
        }
    }
}

impl CloudSessions {
    /// Brings the sandbox of cloud session `session_id` back after a stop, a resume or an app
    /// restart. BLOCKS for minutes: call it from a worker thread, never the UI thread. Explicit
    /// callers only (the user, or a Mono / the app API sending a message), see the module docs. `progress` hears human-readable lines (already scrubbed of secrets); setting
    /// `cancel` aborts at the next step.
    pub fn reconnect_sandbox(
        &self,
        session_id: i64,
        progress: &mut dyn FnMut(&str),
        cancel: &AtomicBool,
    ) -> Result<(), String> {
        self.reconnect_sandbox_typed(session_id, progress, cancel)
            .map_err(|e| e.to_string())
    }

    /// [`Self::reconnect_sandbox`] with a typed error. Same rules: explicit callers only.
    pub(crate) fn reconnect_sandbox_typed(
        &self,
        session_id: i64,
        progress: &mut dyn FnMut(&str),
        cancel: &AtomicBool,
    ) -> Result<(), ReconnectError> {
        let deps = self.sandbox_deps()?;
        let (session, config) = {
            let store = self.store()?;
            let session = store
                .remote_session(session_id)?
                .ok_or("Session not found")?;
            let config = match &session.project.local_project {
                Some(local) => super::super::docker_config::load(&store, local)?,
                None => None,
            };
            (session, config)
        };
        let cloud = session
            .project
            .cloud()
            .filter(|c| c.kind == CloudBackendKind::DockerSandbox)
            .ok_or("This session does not run in a sandbox.")?;
        if crate::store::is_pending_cloud_environment(&session.project.environment_id) {
            return Err(STILL_STARTING.into());
        }
        require_claude(&session.summary.provider)?;
        let name = cloud.name.clone();
        let _flight = {
            let mut set = self
                .reconnecting
                .lock()
                .map_err(|_| "Reconnect state is unavailable".to_string())?;
            if !set.insert(name.clone()) {
                return Err("This sandbox is already reconnecting.".into());
            }
            InFlight {
                set: &self.reconnecting,
                name: name.clone(),
            }
        };

        // Read-only: decides whether an ssh is worth it.
        let raw = deps.backend.state(&name).map_err(|e| {
            if deps.backend.is_missing_error(&e) {
                SANDBOX_GONE.to_string()
            } else {
                Scrub(vec![]).apply(&e)
            }
        })?;
        if deps.backend.is_deleted_state(&raw) {
            return Err(SANDBOX_GONE.into());
        }
        match deps.backend.display_state(&raw) {
            CloudState::Stopping => {
                return Err("Sandbox is stopping. Try again in a minute.".into());
            }
            CloudState::Unavailable | CloudState::Unknown => {
                return Err(format!(
                    "The sandbox is in state {}. Delete this session and start a new one.",
                    raw.trim()
                )
                .into());
            }
            CloudState::Available | CloudState::Starting | CloudState::Stopped => {}
        }
        self.invalidate_ref(&cloud);

        let machine = self
            .host
            .machines()?
            .into_iter()
            .find(|m| {
                m.environment_id == session.project.environment_id
                    && m.sandbox.as_ref().is_some_and(|s| s.name == name)
            })
            .ok_or("The sandbox connection was removed. Start a new session.")?;
        let target = machine
            .sandbox
            .clone()
            .ok_or("The sandbox connection was removed. Start a new session.")?;
        crate::remote::validate_sandbox_target(&target)?;
        // The project config only supplies the credential and the Claude account; without it
        // the container is still brought back, just without those two.
        let have_config = config.is_some();
        let mut config = config.unwrap_or_default();
        if let Some(path) = &target.config {
            config.devcontainer_path = path.clone();
        }
        // A live tunnel needs the container and the Host left alone (this uses the tunnel
        // only, never ssh). The Git credential cache is the one thing that expires on its
        // own, so a delivery older than [`CREDENTIAL_REFRESH_MS`] is repeated: it is cheap,
        // stdin-only, and the sandbox is running anyway.
        if self
            .host
            .request(&machine.id, "environment.describe", serde_json::json!({}))
            .is_ok()
        {
            progress("The sandbox is already connected");
            let key = credential_key(&name);
            let last = self
                .store()
                .ok()
                .and_then(|store| store.setting(&key).ok().flatten())
                .and_then(|text| text.parse::<u64>().ok())
                .unwrap_or(0);
            if config.credential_id.is_some()
                && self.now().saturating_sub(last) >= CREDENTIAL_REFRESH_MS
            {
                let halted = || cancel.load(Ordering::Relaxed);
                match self.deliver_git_credential(&target.id, &config, &halted) {
                    Ok(()) => {
                        if let Ok(store) = self.store() {
                            let _ = store.save_setting(&key, &self.now().to_string());
                        }
                    }
                    Err(error) => {
                        progress(&format!("Could not refresh the Git credential: {error}"))
                    }
                }
            }
            return Ok(());
        }
        let auth = deps.secrets.claude_auth(config.claude_account.as_deref())?;
        let mut scrub = Scrub(vec![]);
        if let Some(auth) = &auth {
            scrub.0.push(Zeroizing::new(auth.secret().to_string()));
        }
        let repo = git_credentials::normalize_origin(&config.repo_url)
            .map(|(_, _, path)| path)
            .unwrap_or_default();
        let repo_dir = target
            .workspace
            .rsplit('/')
            .next()
            .unwrap_or("repo")
            .to_string();
        let start = SandboxStart {
            request_id: String::new(),
            config: config.clone(),
            sandbox_id: Some(target.id.clone()),
            repo_dir,
            workspace: Some(target.workspace.clone()),
            container: None,
            mode: Some(target.mode),
            relay_port: target.relay_port,
            rebuild: false,
        };
        let mut record = fresh_sandbox_record(
            &name,
            session.project.local_project.as_deref().unwrap_or(""),
            &repo,
            "",
            &session.project.cwd,
            start,
            self.now(),
        );
        record.session_id = session_id;
        deps.backend.remember(&name, &target.id);

        let mut tell = |event: StartEvent| {
            if let StartEvent::Log(text) = event {
                progress(text.strip_prefix(PROGRESS_PREFIX).unwrap_or(&text));
            }
        };
        let mut run = Run {
            record: &mut record,
            events: &mut tell,
            cancel,
            deps: deps.clone(),
            scrub,
            detached: true,
        };
        let recreated = std::cell::Cell::new(false);
        let outcome = (|| -> Result<(), Stop> {
            // A plain sandbox has no container: nothing to start or compare.
            if target.mode == SandboxMode::Container {
                self.progress_line(&mut run, "Starting the container");
            }
            self.sandbox_build_step(&mut run)?;
            // A recreated container has a new Host that does not know this session. Say so
            // now, before the paired machine or the project row is touched.
            let (_, container) = self.container_target(&run)?;
            if let Some(container) = &container {
                let remembered = self
                    .store()
                    .ok()
                    .and_then(|store| store.setting(&container_key(&name)).ok().flatten());
                if remembered.is_some_and(|old| old != container.id) {
                    recreated.set(true);
                    return Err(Stop::Failed(RECREATED.into()));
                }
            }
            if have_config && config.credential_id.is_some() {
                self.progress_line(&mut run, "Delivering the Git credential");
                let halted = || cancel.load(Ordering::Relaxed);
                self.deliver_git_credential(&target.id, &config, &halted)?;
                if let Ok(store) = self.store() {
                    let _ = store.save_setting(&credential_key(&name), &self.now().to_string());
                }
            }
            if have_config {
                self.sandbox_configure_step(&mut run)?;
            }
            self.sandbox_connect_step(&mut run, &name, auth.as_ref())?;
            let (_, container) = self.container_target(&run)?;
            let remote_cwd = Self::project_folder(&target, container.as_ref());
            self.progress_line(&mut run, "Reconnecting to the sandbox");
            let display = machine.name.clone();
            let request = OpenRequest {
                local_cwd: session.project.local_project.as_deref().unwrap_or(""),
                cloud: &cloud,
                sandbox: Some(&target),
                display: &display,
                repo: &repo,
                remote_cwd: &remote_cwd,
            };
            deps.backend.remember_target(&target);
            let backend = self.backend(CloudBackendKind::DockerSandbox)?;
            let mut lines: Vec<String> = vec![];
            let result = super::super::open_project(
                &*backend,
                &*self.host,
                &self.store,
                &request,
                cancel,
                &mut |_, _| {},
                &mut |line| lines.push(line.to_string()),
            );
            for line in lines {
                self.progress_line(&mut run, &line);
            }
            let (binding, _) = result.map_err(|e| Stop::from(run.scrub.apply(&e)))?;
            if binding.environment_id != session.project.environment_id {
                // The container was recreated: its Host is a new one and this session's
                // history stayed in the old one.
                recreated.set(true);
                return Err(Stop::Failed(RECREATED.into()));
            }
            Ok(())
        })();
        if outcome.is_ok() {
            if let (Ok(store), Some(container)) = (
                self.store(),
                record
                    .sandbox_start
                    .as_ref()
                    .and_then(|s| s.container.as_ref()),
            ) {
                let _ = store.save_setting(&container_key(&name), &container.id);
            }
        }
        self.invalidate_ref(&cloud);
        match outcome {
            Ok(()) => Ok(()),
            Err(Stop::Aborted) => Err(START_ABORTED.into()),
            Err(Stop::Failed(_)) if recreated.get() => Err(ReconnectError::Recreated),
            Err(Stop::Failed(error)) => Err(ReconnectError::HostUnreachable(error)),
        }
    }
}
