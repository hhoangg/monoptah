//! [`Connections`] side of the two-phase cloud Host upgrade: the [`UpgradeEnv`] that talks to a
//! real machine. The decision itself is in [`super::host_upgrade`].
use super::host_upgrade::{self, scrub, Outcome, SandboxAuth, UpgradeEnv};
use super::ssh::{self, HostPlatform, Job};
use super::{read, write, Connections, StoredMachine};
use serde_json::{json, Value};
use std::time::Duration;

/// How long a restarted Host gets to answer on the new tunnel.
const COME_BACK_ATTEMPTS: u32 = 20;
const COME_BACK_WAIT: Duration = Duration::from_millis(500);

impl Connections {
    /// Phase 2 of the cloud Host upgrade for `machine_id`: if its Host is older or foreign and
    /// nothing runs on it, replace it and reopen the tunnel; if something runs, mark the machine
    /// and leave it for the next idle connect. `auth` is the sandbox's Claude prelude (the same
    /// the connect step used); a codespace passes `None`. `progress` hears the status lines.
    ///
    /// Only for a user-initiated connect or start flow: it needs the live tunnel that flow just
    /// opened and may run ssh. Never fatal, and a user's own SSH machine is never touched (it
    /// keeps the manual Upgrade button).
    pub fn upgrade_host_if_needed(
        &self,
        machine_id: &str,
        auth: Option<SandboxAuth<'_>>,
        progress: &mut dyn FnMut(&str),
    ) -> Outcome {
        let Ok(Some(machine)) = self.stored_machine(machine_id) else {
            return Outcome::Current;
        };
        if !machine.ssh.as_ref().is_some_and(ssh::SshTarget::is_cloud) {
            return Outcome::Current;
        }
        let secret = auth.map_or("", |auth| auth.secret);
        let env = CloudUpgrade {
            connections: self,
            id: machine_id,
            sandbox: machine.ssh.as_ref().is_some_and(|t| t.sandbox.is_some()),
            auth,
        };
        host_upgrade::upgrade_if_needed(&env, &mut |line| progress(&scrub(line, secret)))
    }

    /// Whether the Host of this machine waits for a running turn to end before it is upgraded.
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
}

impl CloudUpgrade<'_> {
    fn machine(&self) -> Result<StoredMachine, String> {
        self.connections
            .stored_machine(self.id)?
            .ok_or_else(|| "Machine is no longer connected".into())
    }
    fn clean(&self, text: String) -> String {
        scrub(&text, self.auth.map_or("", |auth| auth.secret))
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

    fn install(&self) -> Result<(), String> {
        let machine = self.machine()?;
        let mut target = machine
            .ssh
            .clone()
            .ok_or("This connection does not use SSH")?;
        // A sandbox uses the SAME prelude as its connect step, so the restarted Host has the
        // Claude credential in its environment. It travels on stdin only.
        let script = if self.sandbox {
            ssh::sandbox_upgrade_script(self.auth.map(|auth| auth.prelude), target.remote_port)
        } else {
            ssh::codespace_upgrade_script(target.remote_port)
        };
        let job = Job::new();
        let askpass = job.askpass()?;
        let output = ssh::run_script(&target, HostPlatform::Unix, script, &job, &askpass)
            .map_err(|error| self.clean(error))?;
        let port = serde_json::from_str::<Value>(output.lines().last().unwrap_or(""))
            .ok()
            .and_then(|info| info.get("port").and_then(Value::as_u64))
            .and_then(|port| u16::try_from(port).ok())
            .filter(|port| *port > 0)
            .ok_or("Host update returned an invalid response")?;
        if port != target.remote_port {
            target.remote_port = port;
            self.connections.update_machine(self.id, |machine| {
                machine.ssh = Some(target);
                true
            })?;
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
        let job = Job::new();
        let askpass = job.askpass()?;
        let tunnel = ssh::Tunnel::start(&target, Some(&job), Some(&askpass))
            .map_err(|error| self.clean(error))?;
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
        Err(self.clean(format!("The updated Host did not answer: {last}")))
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
}
