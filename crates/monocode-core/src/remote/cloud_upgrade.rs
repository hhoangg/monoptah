//! [`Connections`] side of the two-phase cloud Host upgrade: the [`UpgradeEnv`] that talks to a
//! real machine. The decision itself is in [`super::host_upgrade`].
//!
//! Besides the Host itself, three things are guarded here:
//! * the machine's job slot is held for the whole upgrade, so a bootstrap from a concurrent
//!   wake cannot run on the same machine at the same time;
//! * while the Host is swapped, requests that could start a turn are refused with
//!   [`HOST_UPDATING`] (retryable);
//! * an upgrade pass runs once per tunnel (once per connect), however many callers of one
//!   connect ask.
use super::host_upgrade::{self, scrub, Outcome, SandboxAuth, SwapError, UpgradeEnv};
use super::ssh::{self, HostPlatform, Job, SshTarget, Tunnel, UpgradePhase, RESTORED_MARKER};
use super::ssh_askpass::Askpass;
use super::{read, write, Connections, StoredMachine};
use serde_json::{json, Value};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// How long a restarted Host gets to answer on the new tunnel.
const COME_BACK_ATTEMPTS: u32 = 20;
const COME_BACK_WAIT: Duration = Duration::from_millis(500);
/// How long an upgrade waits for the single SSH job slot before it gives up (a warning).
const SLOT_WAIT: Duration = Duration::from_secs(120);

/// Returned for a request that could start a turn while the Host is being swapped.
pub const HOST_UPDATING: &str = "Monoptah Host is being updated. Try again in a moment.";

/// Requests that start or feed a turn (a send, an import or move of a session, a workspace
/// command). Refused while the machine's Host is swapped.
pub(super) fn starts_turn(method: &str) -> bool {
    matches!(
        method,
        "commands.dispatch" | "sessions.sync" | "sessions.syncChunk" | "workspace.run"
    )
}

/// How the upgrade reaches the machine: the ssh runner and the tunnel. Replaced in tests.
pub(super) trait CloudRunner: Send + Sync {
    fn run_script(
        &self,
        target: &SshTarget,
        script: String,
        job: &Arc<Job>,
        askpass: &Askpass,
    ) -> Result<String, String>;
    fn start_tunnel(
        &self,
        target: &SshTarget,
        job: &Arc<Job>,
        askpass: &Askpass,
    ) -> Result<Tunnel, String>;
}

/// `gh codespace ssh` or the sandbox's container exec, and a real tunnel.
pub(super) struct SshRunner;
impl CloudRunner for SshRunner {
    fn run_script(
        &self,
        target: &SshTarget,
        script: String,
        job: &Arc<Job>,
        askpass: &Askpass,
    ) -> Result<String, String> {
        ssh::run_script(target, HostPlatform::Unix, script, job, askpass)
    }
    fn start_tunnel(
        &self,
        target: &SshTarget,
        job: &Arc<Job>,
        askpass: &Askpass,
    ) -> Result<Tunnel, String> {
        Tunnel::start(target, Some(job), Some(askpass))
    }
}

/// Holds the single SSH job slot; frees it when dropped.
struct SlotGuard(Arc<Job>);
impl Drop for SlotGuard {
    fn drop(&mut self) {
        self.0.complete(|| Err("Host update finished".into()));
    }
}

impl Connections {
    /// Phase 2 of the cloud Host upgrade for `machine_id`: if its Host is older or foreign and
    /// nothing runs on it, stage the pinned Host, check again, swap it and reopen the tunnel; if
    /// something runs, mark the machine and leave it for the next connect. `auth` is the
    /// sandbox's Claude prelude (the same the connect step used); a codespace passes `None`.
    /// `cancel` aborts between steps, never in the middle of the swap. `progress` hears the
    /// status lines.
    ///
    /// Only for a user-initiated connect or start flow: it needs the live tunnel that flow just
    /// opened and may run ssh. Never fatal, runs once per connect, and a user's own SSH machine
    /// is never touched (it keeps the manual Upgrade button).
    pub fn upgrade_host_if_needed(
        &self,
        machine_id: &str,
        auth: Option<SandboxAuth<'_>>,
        cancel: &AtomicBool,
        progress: &mut dyn FnMut(&str),
    ) -> Outcome {
        let Ok(Some(machine)) = self.stored_machine(machine_id) else {
            return Outcome::Current;
        };
        if !machine.ssh.as_ref().is_some_and(SshTarget::is_cloud) {
            return Outcome::Current;
        }
        // No tunnel, no connect to upgrade in. One pass per tunnel: a start that woke a
        // codespace asks twice (the wake, then the start), and the second ask is a no-op.
        let Some(lease) = self.tunnels.existing(machine_id) else {
            return Outcome::Current;
        };
        if self.passes().get(machine_id) == Some(&lease.tunnel_id) {
            return Outcome::Current;
        }
        if cancel.load(Ordering::Relaxed) {
            return Outcome::Cancelled;
        }
        let Ok((job, _slot)) = self.hold_job_slot(cancel) else {
            return Outcome::Cancelled;
        };
        let askpass = match job.askpass() {
            Ok(askpass) => askpass,
            Err(error) => {
                let text = host_upgrade::warning(&error, false);
                progress(&text);
                return Outcome::Failed(text);
            }
        };
        let runner = self.runner();
        let secret = auth.map_or("", |auth| auth.secret);
        let env = CloudUpgrade {
            connections: self,
            id: machine_id,
            sandbox: machine.ssh.as_ref().is_some_and(|t| t.sandbox.is_some()),
            auth,
            cancel,
            job: &job,
            askpass: &askpass,
            runner: &*runner,
        };
        let outcome = host_upgrade::upgrade_if_needed(&env, &mut |line| {
            progress(&env.clean(line.to_owned(), secret))
        });
        if outcome != Outcome::Cancelled {
            // The tunnel after the pass (a swap reopens it) is the connect's tunnel.
            if let Some(lease) = self.tunnels.existing(machine_id) {
                self.passes().insert(machine_id.to_owned(), lease.tunnel_id);
            }
        }
        outcome
    }

    /// Whether the Host of this machine waits for the next connect to be upgraded (it was
    /// busy at the last one).
    pub fn host_upgrade_pending(&self, machine_id: &str) -> bool {
        self.stored_machine(machine_id)
            .ok()
            .flatten()
            .is_some_and(|machine| machine.pending_host_upgrade)
    }

    /// Like [`Self::host_upgrade_pending`], for the machine of a workspace.
    pub fn host_upgrade_pending_for_environment(&self, environment_id: &str) -> bool {
        self.machine_for_environment(environment_id)
            .is_ok_and(|machine| self.host_upgrade_pending(&machine.id))
    }

    fn passes(&self) -> std::sync::MutexGuard<'_, std::collections::HashMap<String, u64>> {
        self.upgrade_passes
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn runner(&self) -> Arc<dyn CloudRunner> {
        self.runner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    #[cfg(test)]
    pub(super) fn set_runner(&self, runner: Arc<dyn CloudRunner>) {
        *self
            .runner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = runner;
    }

    /// Takes the single SSH job slot (waiting for a running job, bounded and cancellable), so a
    /// bootstrap from a concurrent wake cannot run beside the upgrade.
    fn hold_job_slot(&self, cancel: &AtomicBool) -> Result<(Arc<Job>, SlotGuard), String> {
        let deadline = Instant::now() + SLOT_WAIT;
        loop {
            {
                let mut jobs = self
                    .jobs
                    .lock()
                    .map_err(|_| "Connection setup is unavailable")?;
                if !jobs.values().any(|job| !job.view().done) {
                    jobs.retain(|_, job| !job.view().done);
                    let job = Job::new();
                    jobs.insert(job.view().id, job.clone());
                    return Ok((job.clone(), SlotGuard(job)));
                }
            }
            if cancel.load(Ordering::Relaxed) {
                return Err(super::WAKE_CANCELLED.into());
            }
            if Instant::now() >= deadline {
                return Err(super::SSH_BUSY.into());
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    fn stored_machine(&self, machine_id: &str) -> Result<Option<StoredMachine>, String> {
        let _guard = self
            .store
            .lock()
            .map_err(|_| "Connection store is locked")?;
        Ok(read(&self.store_path())?
            .into_iter()
            .find(|m| m.id == machine_id))
    }

    /// Changes one stored machine under the store lock; a missing machine is not an error.
    fn update_machine(
        &self,
        machine_id: &str,
        change: impl FnOnce(&mut StoredMachine) -> bool,
    ) -> Result<(), String> {
        let _guard = self
            .store
            .lock()
            .map_err(|_| "Connection store is locked")?;
        let path = self.store_path();
        let mut machines = read(&path)?;
        let Some(machine) = machines.iter_mut().find(|m| m.id == machine_id) else {
            return Ok(());
        };
        if change(machine) {
            write(&path, &machines)?;
        }
        Ok(())
    }
}

struct CloudUpgrade<'a> {
    connections: &'a Connections,
    id: &'a str,
    sandbox: bool,
    auth: Option<SandboxAuth<'a>>,
    cancel: &'a AtomicBool,
    job: &'a Arc<Job>,
    askpass: &'a Askpass,
    runner: &'a dyn CloudRunner,
}

impl CloudUpgrade<'_> {
    fn machine(&self) -> Result<StoredMachine, String> {
        self.connections
            .stored_machine(self.id)?
            .ok_or_else(|| "Machine is no longer connected".into())
    }

    /// Masks the credential, and the whole prelude it came in, in text that may be shown.
    fn clean(&self, text: String, secret: &str) -> String {
        let text = match self.auth {
            Some(auth) if !auth.prelude.is_empty() => text.replace(auth.prelude, "[prelude]"),
            _ => text,
        };
        scrub(&text, secret)
    }
    fn clean_error(&self, text: String) -> String {
        let secret = self.auth.map_or("", |auth| auth.secret);
        self.clean(text, secret)
    }

    /// Runs one step of the upgrade script on the machine and returns its last stdout line.
    fn run(&self, phase: UpgradePhase) -> Result<String, String> {
        let target = self
            .machine()?
            .ssh
            .ok_or("This connection does not use SSH")?;
        // A sandbox uses the SAME prelude as its connect step, so a restarted Host has the
        // Claude credential in its environment. It travels on stdin only.
        let script = if self.sandbox {
            ssh::sandbox_upgrade_script(
                self.auth.map(|auth| auth.prelude),
                target.remote_port,
                phase,
            )
        } else {
            ssh::codespace_upgrade_script(target.remote_port, phase)
        };
        let output = self
            .runner
            .run_script(&target, script, self.job, self.askpass)
            .map_err(|error| self.clean_error(error))?;
        Ok(self.clean_error(output.lines().last().unwrap_or("").to_owned()))
    }

    /// Runs `work`, and stops the ssh child of the job when the start is cancelled meanwhile.
    fn cancellable<T>(&self, work: impl FnOnce() -> T) -> T {
        let finished = AtomicBool::new(false);
        std::thread::scope(|scope| {
            scope.spawn(|| {
                while !finished.load(Ordering::Relaxed) {
                    if self.cancel.load(Ordering::Relaxed) {
                        self.job.cancel();
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(50));
                }
            });
            let result = work();
            finished.store(true, Ordering::Relaxed);
            result
        })
    }
}

impl UpgradeEnv for CloudUpgrade<'_> {
    fn host_info(&self) -> Result<super::host_info::HostInfo, String> {
        self.connections.describe_host(self.id)
    }

    fn busy(&self) -> Result<bool, String> {
        let projects = self
            .connections
            .request(self.id, "projects.list", json!({}))?;
        let projects = projects
            .as_array()
            .ok_or("Invalid remote projects response")?;
        for project in projects {
            let sessions = self.connections.request(
                self.id,
                "sessions.list",
                json!({"projectId": project["id"]}),
            )?;
            let sessions = sessions
                .as_array()
                .ok_or("Invalid remote sessions response")?;
            if sessions.iter().any(session_is_active) {
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn pending(&self) -> bool {
        self.connections.host_upgrade_pending(self.id)
    }

    fn set_pending(&self, pending: bool) {
        let _ = self.connections.update_machine(self.id, |machine| {
            let changed = machine.pending_host_upgrade != pending;
            machine.pending_host_upgrade = pending;
            changed
        });
    }

    fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }

    fn stage(&self) -> Result<bool, String> {
        let last = self.cancellable(|| self.run(UpgradePhase::Stage))?;
        serde_json::from_str::<Value>(&last)
            .ok()
            .and_then(|info| info.get("staged").and_then(Value::as_bool))
            .ok_or_else(|| "Host update returned an invalid response".into())
    }

    fn discard(&self) {
        let _ = self.run(UpgradePhase::Discard);
    }

    fn hold_sends(&self, held: bool) {
        let mut upgrading = self
            .connections
            .upgrading
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if held {
            upgrading.insert(self.id.to_owned());
        } else {
            upgrading.remove(self.id);
        }
    }

    fn swap(&self) -> Result<(), SwapError> {
        let last = self.run(UpgradePhase::Swap).map_err(|error| {
            if error.contains(RESTORED_MARKER) {
                SwapError::Restored(error)
            } else {
                SwapError::Failed(error)
            }
        })?;
        let port = serde_json::from_str::<Value>(&last)
            .ok()
            .and_then(|info| info.get("port").and_then(Value::as_u64))
            .and_then(|port| u16::try_from(port).ok())
            .filter(|port| *port > 0)
            .ok_or_else(|| SwapError::Failed("Host update returned an invalid response".into()))?;
        let mut target = self
            .machine()
            .and_then(|m| {
                m.ssh
                    .ok_or_else(|| "This connection does not use SSH".into())
            })
            .map_err(SwapError::Failed)?;
        if port != target.remote_port {
            target.remote_port = port;
            self.connections
                .update_machine(self.id, |machine| {
                    machine.ssh = Some(target);
                    true
                })
                .map_err(SwapError::Failed)?;
        }
        Ok(())
    }

    fn reconnect(&self) -> Result<super::host_info::HostInfo, String> {
        let connections = self.connections;
        // The Host restarted under the old tunnel: drop it the way `request` does when a
        // tunnel turns unreachable, and learn the new Host from scratch.
        if let Some(lease) = connections.tunnels.existing(self.id) {
            connections.tunnels.invalidate(self.id, &lease);
        }
        connections.host_info.drop_machine(self.id);
        let target = self
            .machine()?
            .ssh
            .ok_or("This connection does not use SSH")?;
        let tunnel = self
            .runner
            .start_tunnel(&target, self.job, self.askpass)
            .map_err(|error| self.clean_error(error))?;
        connections.tunnels.insert(self.id.to_owned(), tunnel);
        let mut last = String::new();
        for attempt in 0..COME_BACK_ATTEMPTS {
            if attempt > 0 {
                std::thread::sleep(COME_BACK_WAIT);
            }
            match connections.describe_host(self.id) {
                Ok(info) => return Ok(info),
                Err(error) => last = error,
            }
        }
        Err(self.clean_error(format!("The updated Host did not answer: {last}")))
    }

    fn is_sandbox(&self) -> bool {
        self.sandbox
    }
}

/// A session that runs, waits for input, or is in a state this code does not know: the Host
/// is not idle. Only `idle` and `interrupted` sessions without a pending question are quiet.
fn session_is_active(session: &Value) -> bool {
    let quiet = matches!(
        session.get("status").and_then(Value::as_str),
        Some("idle" | "interrupted")
    );
    !quiet || session.get("needsInput").and_then(Value::as_bool) == Some(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn machine(id: &str, ssh: Option<SshTarget>, pending: bool) -> StoredMachine {
        StoredMachine {
            id: id.into(),
            name: id.into(),
            endpoint: "http://127.0.0.1:9".into(),
            environment_id: format!("env-{id}"),
            token: "t".repeat(43),
            ssh,
            pending_host_upgrade: pending,
        }
    }
    fn target(codespace: Option<&str>) -> SshTarget {
        SshTarget {
            target: "box".into(),
            port: None,
            remote_port: 3774,
            codespace: codespace.map(Into::into),
            sandbox: None,
        }
    }

    #[test]
    fn only_idle_or_interrupted_sessions_without_a_question_are_quiet() {
        assert!(!session_is_active(
            &json!({"status": "idle", "needsInput": false})
        ));
        assert!(!session_is_active(&json!({"status": "interrupted"})));
        assert!(session_is_active(&json!({"status": "running"})));
        assert!(session_is_active(
            &json!({"status": "idle", "needsInput": true})
        ));
        assert!(session_is_active(&json!({"status": "mystery"})));
        assert!(session_is_active(&json!({})));
    }

    #[test]
    fn a_users_own_ssh_machine_and_a_plain_host_are_never_upgraded() {
        let dir = tempfile::tempdir().unwrap();
        let connections = Connections::new(dir.path());
        write(
            &connections.store_path(),
            &[
                machine("ssh", Some(target(None)), false),
                machine("direct", None, false),
            ],
        )
        .unwrap();
        for id in ["ssh", "direct", "missing"] {
            let mut lines = vec![];
            let outcome = connections.upgrade_host_if_needed(
                id,
                None,
                &AtomicBool::new(false),
                &mut |line| lines.push(line.to_owned()),
            );
            // Nothing was asked of the machine (its endpoint is a dead port) and nothing said.
            assert_eq!(outcome, Outcome::Current, "{id}");
            assert!(lines.is_empty(), "{id}: {lines:?}");
            assert!(!connections.host_upgrade_pending(id));
        }
    }

    #[test]
    fn the_pending_flag_is_stored_per_machine_and_old_stores_load_without_it() {
        let dir = tempfile::tempdir().unwrap();
        let connections = Connections::new(dir.path());
        write(
            &connections.store_path(),
            &[
                machine("a", Some(target(Some("cs"))), false),
                machine("b", Some(target(Some("cs2"))), false),
            ],
        )
        .unwrap();
        connections
            .update_machine("a", |m| {
                m.pending_host_upgrade = true;
                true
            })
            .unwrap();
        assert!(connections.host_upgrade_pending("a"));
        assert!(!connections.host_upgrade_pending("b"));
        assert!(connections.host_upgrade_pending_for_environment("env-a"));
        connections
            .update_machine("a", |m| {
                m.pending_host_upgrade = false;
                true
            })
            .unwrap();
        assert!(!connections.host_upgrade_pending("a"));
        // The flag is only written when set, and a store from before it still loads.
        let text = std::fs::read_to_string(connections.store_path()).unwrap();
        assert!(!text.contains("pendingHostUpgrade"), "{text}");
        std::fs::write(
            connections.store_path(),
            r#"[{"id":"o","name":"o","endpoint":"e","environmentId":"x","token":"t"}]"#,
        )
        .unwrap();
        assert!(!connections.host_upgrade_pending("o"));
    }

    #[test]
    fn requests_that_start_a_turn_are_the_ones_held_during_a_swap() {
        for method in [
            "commands.dispatch",
            "sessions.sync",
            "sessions.syncChunk",
            "workspace.run",
        ] {
            assert!(starts_turn(method), "{method}");
        }
        for method in [
            "environment.describe",
            "projects.list",
            "sessions.list",
            "files.read",
        ] {
            assert!(!starts_turn(method), "{method}");
        }
    }
}
