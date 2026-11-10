//! [`Connections`] side of the two-phase cloud Host upgrade: the [`UpgradeEnv`] that talks to a
//! real machine. The decision itself is in [`super::host_upgrade`].
//!
//! Besides the Host itself, four things are guarded here:
//! * the machine's job slot is held for the whole upgrade, so a bootstrap from a concurrent
//!   wake cannot run on the same machine at the same time;
//! * while the Host is swapped, requests that start a turn are refused with [`HOST_UPDATING`]
//!   (retryable), and the ones already on their way are waited for before the last look;
//! * an upgrade pass runs once per tunnel (once per connect), however many callers of one
//!   connect ask;
//! * the swap itself runs detached on the machine (an ssh that drops or an app that quits
//!   cannot leave the machine without a Host); the app only polls its state.
use super::host_upgrade::{self, scrub, Outcome, SandboxAuth, SwapError, UpgradeEnv};
use super::ssh::{self, HostPlatform, Job, SshTarget, SwapCheck, Tunnel, UpgradePhase};
use super::ssh_askpass::Askpass;
use super::{read, write, Connections, StoredMachine};
use serde_json::{json, Value};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

/// How long a restarted Host gets to answer on the new tunnel.
const COME_BACK_ATTEMPTS: u32 = 20;
const COME_BACK_WAIT: Duration = Duration::from_millis(500);
/// How long an upgrade waits for the single SSH job slot before it defers (a warning).
const SLOT_WAIT: Duration = Duration::from_secs(120);
/// How long a hold waits for the requests that are already on their way to a Host.
const DRAIN_LIMIT: Duration = Duration::from_secs(30);
/// How long the app follows a swap, and how many failed status polls in a row it tolerates.
const SWAP_WAIT: Duration = Duration::from_secs(600);
const POLL_FAILURES: u32 = 5;
/// How long an app quit waits for a swap that is past the point of no return.
const SHUTDOWN_WAIT: Duration = Duration::from_secs(60);

/// Returned for a request that starts a turn while the Host is being swapped.
pub const HOST_UPDATING: &str = "Monoptah Host is being updated. Try again in a moment.";

/// Requests that start a turn (a send, a workspace command). Held back while the machine's
/// Host is swapped. Reads and session imports are not: they start nothing.
pub(super) fn starts_turn(method: &str) -> bool {
    matches!(method, "commands.dispatch" | "workspace.run")
}

/// Counts the turn-starting requests that are on their way to one machine, and can hold new
/// ones back. An entry is taken for the whole request, so a hold that has drained them knows
/// that none of them can still reach the Host.
#[derive(Default)]
pub(super) struct SendGate {
    state: Mutex<GateState>,
    drained: Condvar,
}
#[derive(Default)]
struct GateState {
    in_flight: usize,
    held: bool,
}
/// One turn-starting request in flight; frees its place when dropped.
pub(super) struct Entry(Arc<SendGate>);
impl Drop for Entry {
    fn drop(&mut self) {
        let mut state = self
            .0
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.in_flight -= 1;
        self.0.drained.notify_all();
    }
}
impl SendGate {
    /// Takes a place, or says the machine is being updated.
    pub(super) fn enter(self: &Arc<Self>) -> Result<Entry, String> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.held {
            return Err(HOST_UPDATING.into());
        }
        state.in_flight += 1;
        Ok(Entry(self.clone()))
    }
    /// Refuses new requests, then waits for the ones in flight. On timeout the hold is undone.
    fn hold(&self, limit: Duration) -> Result<(), String> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.held = true;
        let (mut state, timeout) = self
            .drained
            .wait_timeout_while(state, limit, |state| state.in_flight > 0)
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if timeout.timed_out() {
            state.held = false;
            return Err("Requests to the Host did not finish in time.".into());
        }
        Ok(())
    }
    #[cfg(test)]
    pub(super) fn hold_for_test(&self) {
        self.state.lock().unwrap().held = true;
    }
    fn release(&self) {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .held = false;
    }
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
    /// How often the state of a running swap is asked for.
    fn poll_interval(&self) -> Duration {
        Duration::from_secs(2)
    }
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

/// Marks a job as past the point of no return: an app quit leaves it running (and waits for it).
struct SwapMark<'a> {
    connections: &'a Connections,
    job: String,
}
impl<'a> SwapMark<'a> {
    fn new(connections: &'a Connections, job: &Job) -> Self {
        let job = job.view().id;
        connections
            .swap_jobs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(job.clone());
        Self { connections, job }
    }
}
impl Drop for SwapMark<'_> {
    fn drop(&mut self) {
        self.connections
            .swap_jobs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&self.job);
    }
}

impl Connections {
    /// Phase 2 of the cloud Host upgrade for `machine_id`: if its Host is older or foreign and
    /// nothing runs on it, stage the pinned Host, hold sends and look again, have the machine
    /// swap it (detached, with a last look on the Host itself) and reopen the tunnel; if
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
        let (job, _slot) = match self.hold_job_slot(cancel, &mut || {
            progress("Waiting for another operation on this workspace…")
        }) {
            Ok(slot) => slot,
            Err(error) if error == super::SSH_BUSY => {
                // Still busy after the wait: not an error, the next connect tries again.
                let _ = self.update_machine(machine_id, |m| {
                    let changed = !m.pending_host_upgrade;
                    m.pending_host_upgrade = true;
                    changed
                });
                progress(
                    "Monoptah Host could not be updated now: another operation on this workspace is still running. It will be retried at the next connect",
                );
                return Outcome::Deferred;
            }
            Err(_) => return Outcome::Cancelled,
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
        let env = CloudUpgrade {
            connections: self,
            id: machine_id,
            sandbox: machine.ssh.as_ref().is_some_and(|t| t.sandbox.is_some()),
            auth,
            device_token: machine.token.clone(),
            environment_id: machine.environment_id.clone(),
            cancel,
            job: &job,
            askpass: &askpass,
            runner: &*runner,
        };
        let outcome = host_upgrade::upgrade_if_needed(&env, &mut |line| {
            progress(&env.clean_error(line.to_owned()))
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

    pub(super) fn gate(&self, machine_id: &str) -> Arc<SendGate> {
        self.gates
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .entry(machine_id.to_owned())
            .or_default()
            .clone()
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
    /// bootstrap from a concurrent wake cannot run beside the upgrade. `waiting` is called once
    /// if the slot is not free at first.
    fn hold_job_slot(
        &self,
        cancel: &AtomicBool,
        waiting: &mut dyn FnMut(),
    ) -> Result<(Arc<Job>, SlotGuard), String> {
        let deadline = Instant::now() + self.slot_wait();
        let mut told = false;
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
            if !told {
                told = true;
                waiting();
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

    fn drain_limit(&self) -> Duration {
        #[cfg(test)]
        if let Some(limit) = *self
            .drain_override
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
        {
            return limit;
        }
        DRAIN_LIMIT
    }

    fn slot_wait(&self) -> Duration {
        #[cfg(test)]
        if let Some(wait) = *self
            .slot_wait_override
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
        {
            return wait;
        }
        SLOT_WAIT
    }

    /// Waits (bounded) for the swaps that are past the point of no return; used by `shutdown`.
    pub(super) fn wait_for_swaps(&self, protected: &std::collections::HashSet<String>) {
        let deadline = Instant::now() + SHUTDOWN_WAIT;
        while Instant::now() < deadline {
            let running = self.jobs.lock().is_ok_and(|jobs| {
                jobs.iter()
                    .any(|(id, job)| protected.contains(id) && !job.view().done)
            });
            if !running {
                return;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    pub(super) fn stored_machine(&self, machine_id: &str) -> Result<Option<StoredMachine>, String> {
        let _guard = self
            .store
            .lock()
            .map_err(|_| "Connection store is locked")?;
        Ok(read(&self.store_path())?
            .into_iter()
            .find(|m| m.id == machine_id))
    }

    /// Changes one stored machine under the store lock; a missing machine is not an error.
    pub(super) fn update_machine(
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
    /// The machine's device credential: handed to the swap worker for its last look, and
    /// never shown.
    device_token: String,
    environment_id: String,
    cancel: &'a AtomicBool,
    job: &'a Arc<Job>,
    askpass: &'a Askpass,
    runner: &'a dyn CloudRunner,
}

/// How a swap worker ended, as the machine reports it.
#[derive(Debug, PartialEq, Eq)]
enum Finished {
    Done(u16),
    Restored,
    Unrestored,
    Busy,
    Refused(u32),
    Failed,
    /// No worker was ever started.
    NotStarted,
}

impl CloudUpgrade<'_> {
    fn machine(&self) -> Result<StoredMachine, String> {
        self.connections
            .stored_machine(self.id)?
            .ok_or_else(|| "Machine is no longer connected".into())
    }

    /// Masks the credentials, and the whole prelude they came in, in text that may be shown.
    fn clean_error(&self, text: String) -> String {
        let secret = self.auth.map_or("", |auth| auth.secret);
        let text = match self.auth {
            Some(auth) if !auth.prelude.is_empty() => text.replace(auth.prelude, "[prelude]"),
            _ => text,
        };
        scrub(&scrub(&text, secret), &self.device_token)
    }

    /// Runs one step of the upgrade script on the machine and returns its last stdout line.
    fn run(&self, phase: UpgradePhase) -> Result<String, String> {
        self.run_with(self.job, self.askpass, phase)
    }
    fn run_with(
        &self,
        job: &Arc<Job>,
        askpass: &Askpass,
        phase: UpgradePhase,
    ) -> Result<String, String> {
        let target = self
            .machine()?
            .ssh
            .ok_or("This connection does not use SSH")?;
        let check = SwapCheck {
            device_token: &self.device_token,
            environment_id: &self.environment_id,
        };
        // A sandbox uses the SAME prelude as its connect step, so a restarted Host has the
        // Claude credential in its environment. It travels on stdin only.
        let script = if self.sandbox {
            ssh::sandbox_upgrade_script(
                self.auth.map(|auth| auth.prelude),
                target.remote_port,
                phase,
                Some(check),
            )
        } else {
            ssh::codespace_upgrade_script(target.remote_port, phase, Some(check))
        };
        let output = self
            .runner
            .run_script(&target, script, job, askpass)
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

    fn set_unconfirmed(&self, on: bool) {
        let _ = self.connections.update_machine(self.id, |machine| {
            let changed = machine.swap_unconfirmed != on;
            machine.swap_unconfirmed = on;
            changed
        });
    }

    /// Asks the machine how the swap worker is doing until it is done. `Err`: contact was lost
    /// or the swap takes too long; the machine keeps going on its own.
    fn wait_for_worker(&self) -> Result<Finished, String> {
        let deadline = Instant::now() + SWAP_WAIT;
        let mut failures = 0;
        loop {
            match self.run(UpgradePhase::Status) {
                Ok(line) => {
                    failures = 0;
                    let status: Value = serde_json::from_str(&line).unwrap_or(Value::Null);
                    match status.get("swap").and_then(Value::as_str) {
                        Some("done") => {
                            let port = status["port"]
                                .as_u64()
                                .and_then(|port| u16::try_from(port).ok())
                                .filter(|port| *port > 0)
                                .ok_or("Host update returned an invalid response")?;
                            return Ok(Finished::Done(port));
                        }
                        Some("restored") => return Ok(Finished::Restored),
                        Some("unrestored") => return Ok(Finished::Unrestored),
                        Some("busy") => return Ok(Finished::Busy),
                        Some("refused") => {
                            let code = status["code"].as_u64().unwrap_or(0);
                            return Ok(Finished::Refused(u32::try_from(code).unwrap_or(0)));
                        }
                        Some("failed") => return Ok(Finished::Failed),
                        Some("none") => return Ok(Finished::NotStarted),
                        Some("checking" | "swapping") => {}
                        _ => return Err("Host update returned an invalid response".into()),
                    }
                }
                Err(error) => {
                    failures += 1;
                    if failures >= POLL_FAILURES {
                        return Err(error);
                    }
                }
            }
            if Instant::now() >= deadline {
                return Err("The Host update is still running on the machine.".into());
            }
            std::thread::sleep(self.runner.poll_interval());
        }
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

    fn recover(&self) {
        if !self.machine().is_ok_and(|m| m.swap_unconfirmed) {
            return;
        }
        // An earlier connect started a swap and never saw it end. The machine finishes it or
        // puts the old Host back; the tunnel then has to reach whichever runs now.
        match self.wait_for_worker() {
            Err(_) => {}
            Ok(_) => {
                self.set_unconfirmed(false);
                let _ = self.reconnect();
            }
        }
    }

    fn stage(&self) -> Result<bool, String> {
        let last = self.cancellable(|| self.run(UpgradePhase::Stage))?;
        serde_json::from_str::<Value>(&last)
            .ok()
            .and_then(|info| info.get("staged").and_then(Value::as_bool))
            .ok_or_else(|| "Host update returned an invalid response".into())
    }

    fn discard(&self) {
        // The job of this start may be cancelled, which is why we are here: use a fresh one.
        let job = Job::new();
        if let Ok(askpass) = job.askpass() {
            let _ = self.run_with(&job, &askpass, UpgradePhase::Discard);
        }
    }

    fn hold_sends(&self, held: bool) -> Result<(), String> {
        let gate = self.connections.gate(self.id);
        if held {
            gate.hold(self.connections.drain_limit())
        } else {
            gate.release();
            Ok(())
        }
    }

    fn swap(&self) -> Result<(), SwapError> {
        // From the moment the worker starts, an app quit leaves it alone.
        let _mark = SwapMark::new(self.connections, self.job);
        self.set_unconfirmed(true);
        // If this fails the worker may still have started (ssh can drop after it): ask anyway.
        let started = self.run(UpgradePhase::Swap);
        let finished = self.wait_for_worker();
        let ended = finished.is_ok();
        let result = match finished {
            Ok(Finished::Done(port)) => {
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
            Ok(Finished::Restored) => Err(SwapError::Restored(
                "The new Host did not start, so the previous Host was put back".into(),
            )),
            Ok(Finished::Unrestored) => Err(SwapError::Failed(
                "The new Host did not start and the previous Host could not be put back".into(),
            )),
            Ok(Finished::Busy) => Err(SwapError::Busy),
            Ok(Finished::Refused(3)) => Err(SwapError::NotStaged(
                "The downloaded package was not there when the update started".into(),
            )),
            Ok(Finished::Refused(78)) => Err(SwapError::NotTouched(
                "There is no previous Host to roll back to, so the Host was not replaced".into(),
            )),
            Ok(Finished::Refused(_) | Finished::Failed) => Err(SwapError::NotTouched(
                "The update was refused before it changed anything".into(),
            )),
            Ok(Finished::NotStarted) => Err(SwapError::NotTouched(
                started
                    .err()
                    .unwrap_or_else(|| "The update did not start".into()),
            )),
            Err(error) => Err(SwapError::Failed(error)),
        };
        // Only a swap that is still running somewhere stays unconfirmed.
        if ended {
            self.set_unconfirmed(false);
        }
        result
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
            swap_unconfirmed: false,
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
        for method in ["commands.dispatch", "workspace.run"] {
            assert!(starts_turn(method), "{method}");
        }
        for method in [
            "environment.describe",
            "projects.list",
            "sessions.list",
            "sessions.sync",
            "sessions.syncChunk",
            "files.read",
        ] {
            assert!(!starts_turn(method), "{method}");
        }
    }
}
