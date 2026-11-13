//! Cost control: stop (never delete) a cloud workspace that nothing needs any more.
//!
//! A Docker sandbox bills per runtime second until its runtime limit ends it, and people leave
//! workspaces behind after a session is done. This module is the app-side policy for that:
//!
//! * [`AutoStopPrefs`]: the user's choice, per backend. Defaults: a sandbox stops after
//!   [`DEFAULT_SANDBOX_IDLE_MINUTES`] idle minutes and when its last session is gone; a
//!   codespace is left alone because GitHub already idles it out after 30 minutes.
//! * [`AutoStop`]: the decision engine. It never reads a clock or touches `gh`/`sbx`: the app
//!   hands it the in-memory facts it already has ([`WorkspaceFacts`]) once a minute together
//!   with `now` (seconds on any monotonic clock), and it answers with the workspaces to stop.
//! * [`stop_if_still_idle`]: the one place that talks to the backend, and only for a
//!   [`Decision`]. It re-reads the state and the stored sessions first and stops through
//!   [`CloudSessions::stop_cloud`], the path every other Stop uses.
//!
//! The policy is conservative: an unknown state, a session that is working, waiting, starting,
//! restoring, being archived, holding a queued prompt or with files not yet copied locally
//! all count as "not idle". It only ever stops. A stopped workspace is resumed by the user's
//! next send, as before; nothing in the background wakes it.
use super::{CloudBackendKind, CloudRef, CloudSessions, CloudState};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

/// The idle times the settings offer.
pub const IDLE_CHOICES: [u32; 4] = [5, 10, 15, 30];
/// Sandboxes stop after this many idle minutes unless the user changes it.
pub const DEFAULT_SANDBOX_IDLE_MINUTES: u32 = 10;
/// How often the app should call [`AutoStop::check`]. The check reads memory only.
pub const CHECK_EVERY_SECS: u64 = 60;
/// Grace before a workspace with no session left is stopped when no idle time is set, and the
/// lower bound of the idle choices.
const MIN_GRACE_MINUTES: u32 = 5;
/// A stop that was asked for but never seen in the state is forgotten after this long.
const REQUEST_TTL_SECS: u64 = 5 * 60;
const MAX_IDLE_MINUTES: u32 = 24 * 60;

/// What the user chose for one backend.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct KindPolicy {
    /// Stop after this many idle minutes; `None` = never by idleness.
    pub idle_minutes: Option<u32>,
    /// Stop when the last session using the workspace was archived or deleted.
    pub stop_when_unused: bool,
}

impl KindPolicy {
    /// The same policy with an out-of-range idle time brought back into range.
    fn clamped(self) -> Self {
        Self {
            idle_minutes: self.idle_minutes.map(|m| m.clamp(1, MAX_IDLE_MINUTES)),
            ..self
        }
    }
    /// Minutes a workspace with no session left may keep running.
    fn grace_minutes(&self) -> u32 {
        self.clamped().idle_minutes.unwrap_or(MIN_GRACE_MINUTES)
    }
}

/// The auto-stop settings: global, one policy per backend.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AutoStopPrefs {
    pub sandbox: KindPolicy,
    /// Saved as `github`: a test of the workspace state forbids the word "codespace" in it.
    #[serde(rename = "github")]
    pub codespace: KindPolicy,
}

impl Default for AutoStopPrefs {
    fn default() -> Self {
        Self {
            sandbox: KindPolicy {
                idle_minutes: Some(DEFAULT_SANDBOX_IDLE_MINUTES),
                stop_when_unused: true,
            },
            // GitHub idles a codespace out after 30 minutes on its own: nothing to add.
            codespace: KindPolicy {
                idle_minutes: None,
                stop_when_unused: false,
            },
        }
    }
}

impl AutoStopPrefs {
    pub fn policy(&self, kind: CloudBackendKind) -> KindPolicy {
        match kind {
            CloudBackendKind::DockerSandbox => self.sandbox,
            CloudBackendKind::Codespaces => self.codespace,
        }
        .clamped()
    }
    pub fn policy_mut(&mut self, kind: CloudBackendKind) -> &mut KindPolicy {
        match kind {
            CloudBackendKind::DockerSandbox => &mut self.sandbox,
            CloudBackendKind::Codespaces => &mut self.codespace,
        }
    }
    /// Whether any rule is on for any backend.
    pub fn any_enabled(&self) -> bool {
        CloudBackendKind::ALL.into_iter().any(|kind| {
            let policy = self.policy(kind);
            policy.idle_minutes.is_some() || policy.stop_when_unused
        })
    }
}

/// What the app knows about one session on a workspace, from memory.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SessionFacts {
    pub id: i64,
    /// A turn runs, it waits for input, or a terminal/tool is busy.
    pub busy: bool,
    /// Something that must not be cut off or left behind: the session is starting, restoring,
    /// moving or being archived, holds a queued prompt, or has files not copied locally yet.
    pub pending: bool,
    /// The app cannot tell (its state has not been read). Counts as not idle.
    pub unknown: bool,
    /// Last user input the app saw in this session, on the clock of `now`. `None` = none seen
    /// since the app started.
    pub last_input: Option<u64>,
}

/// What the app knows about one workspace that at least one session uses.
#[derive(Clone, Debug)]
pub struct WorkspaceFacts {
    pub cloud: CloudRef,
    /// The cached state. Only `Available` can be stopped.
    pub state: CloudState,
    pub sessions: Vec<SessionFacts>,
    /// The app is archiving, converting, moving or deleting something on this workspace.
    pub app_working: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reason {
    /// Nothing happened for this many minutes.
    Idle { minutes: u32 },
    /// The last session using it was archived or deleted.
    Unused,
}

/// A workspace to stop.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Decision {
    pub cloud: CloudRef,
    pub reason: Reason,
}

impl Decision {
    /// The notice shown after the stop worked.
    pub fn notice(&self) -> String {
        let what = match self.cloud.kind {
            CloudBackendKind::DockerSandbox => format!("Docker sandbox {}", self.cloud.name),
            CloudBackendKind::Codespaces => format!("codespace {}", self.cloud.name),
        };
        match self.reason {
            Reason::Idle { minutes } => {
                format!("Stopped {what} after {minutes} min idle; send a message to resume.")
            }
            Reason::Unused => format!(
                "Stopped {what}: its last session was archived or deleted. A new session can resume it."
            ),
        }
    }
}

/// How a [`Decision`] ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StopOutcome {
    Stopped,
    /// It was not running (or its state could not be read): nothing to do.
    NotRunning,
    /// A session works on it, or still uses it: left alone.
    InUse,
    Failed(String),
}

/// The decision engine. Pure: it keeps only the idle clocks.
#[derive(Debug, Default)]
pub struct AutoStop {
    /// By [`CloudRef::key`]: when the workspace was last seen doing something.
    since: HashMap<String, u64>,
    /// Workspaces seen with a session during this run.
    known: HashMap<String, CloudRef>,
    /// Known workspaces that no longer have a session, and since when.
    unused_since: HashMap<String, u64>,
    /// Stops asked for and not yet seen in the state, with the time of the request.
    requested: HashMap<String, u64>,
}

impl AutoStop {
    pub fn new() -> Self {
        Self::default()
    }

    /// One check. `workspaces` holds every workspace that has a session in the window.
    /// Returns the workspaces to stop now; each is returned once until [`Self::finished`].
    pub fn check(
        &mut self,
        now: u64,
        prefs: &AutoStopPrefs,
        workspaces: &[WorkspaceFacts],
    ) -> Vec<Decision> {
        let mut decisions = Vec::new();
        let live: HashSet<String> = workspaces.iter().map(|w| w.cloud.key()).collect();
        for workspace in workspaces {
            let key = workspace.cloud.key();
            self.known.insert(key.clone(), workspace.cloud.clone());
            self.unused_since.remove(&key);
            // A stop is over once the state shows anything but running; a request the state
            // never confirmed (an old cache, a lost call) expires.
            if let Some(&at) = self.requested.get(&key) {
                let running = workspace.state == CloudState::Available;
                let unknown = workspace.state == CloudState::Unknown;
                if (!running && !unknown) || now.saturating_sub(at) >= REQUEST_TTL_SECS {
                    self.requested.remove(&key);
                }
            }
            let active = workspace.app_working
                || workspace.sessions.is_empty()
                || workspace
                    .sessions
                    .iter()
                    .any(|s| s.busy || s.pending || s.unknown);
            let clock = self.since.entry(key.clone()).or_insert(now);
            if active || workspace.state != CloudState::Available {
                *clock = now;
            }
            let last_input = workspace
                .sessions
                .iter()
                .filter_map(|s| s.last_input)
                .max()
                .unwrap_or(0)
                .min(now);
            let last = (*clock).max(last_input);
            if active
                || workspace.state != CloudState::Available
                || self.requested.contains_key(&key)
            {
                continue;
            }
            let policy = prefs.policy(workspace.cloud.kind);
            let Some(minutes) = policy.idle_minutes else {
                continue;
            };
            if now.saturating_sub(last) >= u64::from(minutes) * 60 {
                self.requested.insert(key, now);
                decisions.push(Decision {
                    cloud: workspace.cloud.clone(),
                    reason: Reason::Idle { minutes },
                });
            }
        }
        // Workspaces that had a session and have none now: archived, deleted, or the project
        // was closed. The worker looks at the store again before it stops anything.
        let gone: Vec<(String, CloudRef)> = self
            .known
            .iter()
            .filter(|(key, _)| !live.contains(*key))
            .map(|(key, cloud)| (key.clone(), cloud.clone()))
            .collect();
        for (key, cloud) in gone {
            let since = *self.unused_since.entry(key.clone()).or_insert(now);
            let policy = prefs.policy(cloud.kind);
            if !policy.stop_when_unused || self.requested.contains_key(&key) {
                continue;
            }
            if now.saturating_sub(since) >= u64::from(policy.grace_minutes()) * 60 {
                self.requested.insert(key, now);
                decisions.push(Decision {
                    cloud,
                    reason: Reason::Unused,
                });
            }
        }
        decisions
    }

    /// The worker finished a [`Decision`].
    pub fn finished(&mut self, decision: &Decision, outcome: &StopOutcome, now: u64) {
        let key = decision.cloud.key();
        self.since.insert(key.clone(), now);
        if *outcome != StopOutcome::Stopped {
            // Ask again only after a whole new window, never in a loop.
            self.requested.remove(&key);
        }
        // A workspace without a session is watched until it is dealt with: stopped, not
        // running, or used after all. Only a failed stop is tried again.
        if decision.reason == Reason::Unused {
            if matches!(outcome, StopOutcome::Failed(_)) {
                self.unused_since.insert(key, now);
            } else {
                self.known.remove(&key);
                self.unused_since.remove(&key);
            }
        }
    }

    /// The user resumed the workspace: it starts a fresh idle window.
    pub fn resumed(&mut self, cloud: &CloudRef, now: u64) {
        let key = cloud.key();
        self.requested.remove(&key);
        self.since.insert(key, now);
    }
}

/// Stops the workspace of `decision` if it still qualifies. BLOCKS (state read, backend stop):
/// call it on a worker, and only for a decision, never on a timer of its own.
pub fn stop_if_still_idle(cloud: &CloudSessions, decision: &Decision) -> StopOutcome {
    let target = &decision.cloud;
    // The cached state can lag a stop done elsewhere; one fresh read before the real call.
    cloud.invalidate_ref(target);
    match cloud.cloud_state_of(target) {
        CloudState::Available => {}
        _ => return StopOutcome::NotRunning,
    }
    let sessions = cloud.sessions_on(target);
    let in_use = match decision.reason {
        Reason::Idle { .. } => sessions.iter().any(|s| s.busy()),
        Reason::Unused => !sessions.is_empty(),
    };
    if in_use {
        return StopOutcome::InUse;
    }
    match cloud.stop_cloud(target) {
        Ok(()) => StopOutcome::Stopped,
        Err(error) => StopOutcome::Failed(error),
    }
}
