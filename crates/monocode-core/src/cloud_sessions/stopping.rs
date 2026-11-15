//! What became of a Stop, told from read-only listings and never from the stop call alone.
//!
//! `sbx --cloud stop` only says the request was accepted ("Stop returns once the request is
//! accepted"). A sandbox with Auto-resume can start again afterwards, so "the call succeeded"
//! is not "it is stopped". This module remembers each stop this process asked for, and every
//! workspaces overview load (`sbx ls`, which never wakes anything) is read against that
//! memory:
//!
//! * the listing says stopping: [`StopReport::Stopping`], and [`StopReport::TakingLong`] once it
//!   has been stopping for [`STOP_EXPECTED`] (the CLI says a large sandbox can take 7 to 9
//!   minutes);
//! * the listing says stopped: [`StopReport::Stopped`];
//! * the listing still says running after [`STOP_GRACE`], or says running again after it had
//!   been seen stopped (another computer's tunnel, the TTL, Auto-resume traffic):
//!   [`StopReport::StillRunning`] with the reason. It stays that way until the user stops it
//!   again, brings it back with Reconnect, or the workspace is seen stopped, so a workspace is
//!   never quietly "Running" again after a Stop;
//! * the listing cannot be read, or no longer lists the workspace: [`UnreadStop`] says so, so a
//!   followed Stop never just disappears.
//!
//! Only a **Docker sandbox** is followed. A codespace has no tunnel to close and no Auto-resume;
//! its Stop is left as it was (only the first-seen time of a stopping codespace is kept, for the
//! footer chip).
//!
//! The memory is in this process only. After an app restart the Stop line is gone and the 15
//! minute timer of the chip restarts at the first listing that shows the workspace stopping (the
//! row's state itself is always read from the listing).
//!
//! Since when a workspace has been stopping ([`super::workspaces::WorkspaceRow::stopping_since_ms`])
//! is kept here too: from the request when this app made it, else from the first listing that
//! showed it. The footer chip warns when that passes [`STOPPING_WARN`].
use super::workspaces::{WorkspaceOverview, WorkspaceRow, WorkspaceState};
use super::{CloudBackendKind, CloudRef, CloudSessions};
use std::collections::HashMap;
use std::time::Duration;

/// A listing that still says running this soon after the request has probably not caught up.
pub const STOP_GRACE: Duration = Duration::from_secs(60);
/// A stop that takes longer than this is reported as taking longer than expected.
pub const STOP_EXPECTED: Duration = Duration::from_secs(10 * 60);
/// The footer chip turns to a warning after a workspace has been stopping this long.
pub const STOPPING_WARN: Duration = Duration::from_secs(15 * 60);
/// A followed Stop whose workspace is missing from a readable listing is dropped after this many
/// listings (it was deleted, or only another project uses it).
const MISSING_READS: u32 = 3;
/// A followed Stop whose listing stays unreadable is dropped this long after the request.
const UNREAD_LIMIT: Duration = Duration::from_secs(30 * 60);
/// A listing-based close of a tunnel waits for this many consecutive readings that show the
/// sandbox down, so one stale listing never closes a connection somebody just opened.
const DOWN_READS: u32 = 2;

/// What the dialog says about a workspace after a Stop.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StopReport {
    /// The stop was accepted and the workspace is on its way down.
    Stopping,
    /// Still on its way down after [`STOP_EXPECTED`].
    TakingLong,
    Stopped,
    /// It is running although a stop was accepted; the text says why that can be.
    StillRunning(String),
}

impl StopReport {
    /// Whether the answer can still change by itself (the dialog keeps reading).
    pub fn unsettled(&self) -> bool {
        matches!(self, Self::Stopping | Self::TakingLong)
    }
}

/// A followed Stop whose state could not be read in the last listing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnreadStop {
    pub cloud: CloudRef,
    /// `Could not read the state of …: <problem>`.
    pub text: String,
}

/// How this app's own connection to the workspace fared before the stop was sent.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) enum Connection {
    /// Nothing of this app was connected to it.
    #[default]
    NoneOpen,
    /// This many connections were closed first.
    Closed(usize),
    /// Closing failed; the stop was sent anyway.
    CloseFailed(String),
    /// A request or a start held the connection's lock: it closes when that ends.
    Deferred,
}

impl Connection {
    /// What to tell right when the stop is accepted, when the connection could not be closed.
    pub(super) fn warning(&self) -> Option<String> {
        match self {
            Self::CloseFailed(error) => Some(format!(
                "Stop sent, but the app could not close its connection to it ({error}); that connection may wake the sandbox again."
            )),
            Self::Deferred => Some(
                "Stop sent, but the app's connection to it is busy and will close when its request ends; it may wake the sandbox again."
                    .to_owned(),
            ),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub(super) struct StopWatch {
    cloud: Option<CloudRef>,
    /// The projects (path keys) whose listing shows this sandbox: the projects of its sessions
    /// and of the local sessions moved into it. Empty: a sandbox no session uses, which every
    /// project lists. A load of another project says nothing about this Stop.
    projects: Vec<String>,
    /// When this process had the stop accepted; cleared once the listing settled.
    requested_ms: Option<u64>,
    /// Since when the listing has said stopping.
    stopping_since_ms: Option<u64>,
    /// A verdict that stays until the state changes again.
    settled: Option<StopReport>,
    connection: Connection,
    /// Consecutive readable listings that did not list the workspace.
    missing_reads: u32,
}

fn reason(connection: &Connection, when: &str) -> String {
    let ours = match connection {
        Connection::NoneOpen => "Monoptah had no connection to it open.".to_owned(),
        Connection::Closed(_) => {
            "Monoptah closed its own connection to it before stopping.".to_owned()
        }
        Connection::CloseFailed(error) => {
            format!("Monoptah could not close its own connection to it before stopping ({error}).")
        }
        Connection::Deferred => {
            "Monoptah's own connection to it was busy and could only be closed after its request ended."
                .to_owned()
        }
    };
    format!(
        "it was running again {when}. A sandbox with Auto-resume starts again when anything sends it traffic: a command, or a request through an ssh tunnel that is still open (a tunnel survives a stop and the sandbox's gateway then resumes it). {ours}"
    )
}

/// The report of one row and the since-when of its stopping, from what the listing says now.
fn read(
    watch: &mut StopWatch,
    state: WorkspaceState,
    now: u64,
) -> (Option<StopReport>, Option<u64>) {
    match state {
        WorkspaceState::Stopping | WorkspaceState::Hibernating => {
            watch.settled = None;
            let since = *watch
                .stopping_since_ms
                .get_or_insert(watch.requested_ms.unwrap_or(now));
            let slow = now.saturating_sub(since) >= STOP_EXPECTED.as_millis() as u64;
            // Only a Stop this app asked for is reported on (and followed by the dialog): one
            // somebody else started, or the TTL, shows in the row's state and in the chip.
            let report = watch.requested_ms.map(|_| {
                if slow {
                    StopReport::TakingLong
                } else {
                    StopReport::Stopping
                }
            });
            (report, Some(since))
        }
        WorkspaceState::Stopped => {
            watch.stopping_since_ms = None;
            // Seen stopped: a Stop that was followed has ended well, and so has one that had
            // come back (it went down again).
            if watch.requested_ms.take().is_some() || watch.settled.is_some() {
                watch.settled = Some(StopReport::Stopped);
            }
            (watch.settled.clone(), None)
        }
        WorkspaceState::Running | WorkspaceState::Starting => {
            watch.stopping_since_ms = None;
            if let Some(requested) = watch.requested_ms {
                if now.saturating_sub(requested) < STOP_GRACE.as_millis() as u64 {
                    return (Some(StopReport::Stopping), None);
                }
                watch.requested_ms = None;
                watch.settled = Some(StopReport::StillRunning(reason(
                    &watch.connection,
                    "after the stop was accepted",
                )));
            } else if watch.settled == Some(StopReport::Stopped) {
                // It had stopped and something woke it: another computer's tunnel, the TTL, a
                // command. Never a quiet "Running" again; the user's own Reconnect forgets this.
                watch.settled = Some(StopReport::StillRunning(reason(
                    &watch.connection,
                    "after it had stopped",
                )));
            }
            (watch.settled.clone(), None)
        }
        WorkspaceState::Failed | WorkspaceState::Unknown => (watch.settled.clone(), None),
    }
}

#[derive(Default)]
pub(super) struct StopWatches {
    watches: HashMap<String, StopWatch>,
    /// Consecutive listings that showed the sandbox down, by cloud key.
    down: HashMap<String, u32>,
}

#[cfg(test)]
impl StopWatches {
    /// How many workspaces are being followed.
    pub(super) fn followed(&self) -> usize {
        self.watches.len()
    }
    /// How many sandboxes have a count of consecutive "down" listings.
    pub(super) fn down_counted(&self) -> usize {
        self.down.len()
    }
}

impl CloudSessions {
    /// Remembers that the stop of the sandbox `cloud` was accepted at the current time.
    /// `connection` is how this app's own connection fared before it. A codespace is not
    /// followed. A second Stop of a sandbox that the listing already shows stopping keeps the
    /// first request time, so the timers do not restart.
    pub(super) fn note_stop_accepted(&self, cloud: &CloudRef, connection: Connection) {
        if cloud.kind != CloudBackendKind::DockerSandbox {
            return;
        }
        let now = (self.clock)();
        // Read before the lock: these ask the store.
        let mut projects: Vec<String> = self
            .sessions_on(cloud)
            .into_iter()
            .filter_map(|s| s.local_project)
            .chain(self.moved_projects(cloud))
            .map(|p| crate::files::path_key(&p))
            .collect();
        projects.sort();
        projects.dedup();
        if let Ok(mut watches) = self.stops.lock() {
            let key = cloud.key();
            let kept = watches
                .watches
                .get(&key)
                .filter(|w| w.stopping_since_ms.is_some())
                .and_then(|w| w.requested_ms);
            watches.watches.insert(
                key,
                StopWatch {
                    cloud: Some(cloud.clone()),
                    projects,
                    requested_ms: Some(kept.unwrap_or(now)),
                    connection,
                    ..StopWatch::default()
                },
            );
        }
    }

    /// Forgets what was noted about `cloud` (it is gone, or the user resumed it).
    pub(super) fn forget_stop(&self, cloud: &CloudRef) {
        if let Ok(mut watches) = self.stops.lock() {
            watches.watches.remove(&cloud.key());
            watches.down.remove(&cloud.key());
        }
    }

    /// Dismisses the `Still running: …` line of `cloud`: the user knows (they resumed it by other
    /// means, such as `sbx run` or another computer) and does not want it kept until the next
    /// Stop. Only a verdict that is settled is dismissed; a Stop still on its way is not.
    /// Returns whether something was dismissed. Cached overviews are forgotten so the chip and the
    /// dialog read again.
    pub fn dismiss_stop(&self, cloud: &CloudRef) -> bool {
        let dismissed = match self.stops.lock() {
            Ok(mut watches) => {
                let settled = matches!(
                    watches.watches.get(&cloud.key()),
                    Some(watch) if matches!(watch.settled, Some(StopReport::StillRunning(_)))
                        && watch.requested_ms.is_none()
                );
                if settled {
                    watches.watches.remove(&cloud.key());
                }
                settled
            }
            Err(_) => false,
        };
        if dismissed {
            self.forget_workspace_views();
        }
        dismissed
    }

    /// Closes the connections this app still holds to a sandbox the listing says is stopping or
    /// stopped. A tunnel survives a stop (its keepalives are answered by the gateway), and the
    /// first request a session sends through it resumes the sandbox again: that is what a Stop
    /// from this app, another computer or the TTL would otherwise lose to.
    ///
    /// It closes only after [`DOWN_READS`] consecutive listings showed the sandbox down, and
    /// never while the user's own Reconnect is bringing it back or a start of it is pending
    /// (another flow may just have opened a connection a stale listing knows nothing about).
    ///
    /// `listed_at_ms` is when the listing was READ: a connection opened after it (a Reconnect
    /// that finished while this listing was being processed) is left alone.
    pub(super) fn close_connections_of_stopped(&self, rows: &[WorkspaceRow], listed_at_ms: u64) {
        if let Ok(mut watches) = self.stops.lock() {
            // A sandbox that is no longer listed has nothing to count.
            let listed: Vec<String> = rows.iter().map(|row| row.cloud.key()).collect();
            watches.down.retain(|key, _| listed.contains(key));
        }
        for row in rows {
            if row.cloud.kind != CloudBackendKind::DockerSandbox {
                continue;
            }
            let down = matches!(
                row.state,
                WorkspaceState::Stopping | WorkspaceState::Hibernating | WorkspaceState::Stopped
            );
            let reads = match self.stops.lock() {
                Ok(mut watches) => {
                    if down {
                        let reads = watches.down.entry(row.cloud.key()).or_insert(0);
                        *reads += 1;
                        *reads
                    } else {
                        watches.down.remove(&row.cloud.key());
                        0
                    }
                }
                Err(_) => 0,
            };
            if reads < DOWN_READS {
                continue;
            }
            let bringing_back = self
                .reconnecting
                .lock()
                .map_or(true, |set| set.contains(&row.cloud.name));
            let starting = self.store().map_or(true, |store| {
                super::startup::pending_starts(&store)
                    .iter()
                    .any(|r| r.cloud().as_ref() == Some(&row.cloud))
            });
            if !bringing_back && !starting {
                let _ = self.close_connections_before_stop(&row.cloud, Some(listed_at_ms));
            }
        }
    }

    /// Reads every row of a freshly listed overview against the stops noted so far, and adds an
    /// [`UnreadStop`] for a followed Stop that has no row.
    pub(super) fn apply_stop_reports(&self, view: &mut WorkspaceOverview, now: u64) {
        let Ok(mut watches) = self.stops.lock() else {
            return;
        };
        let mut listed: Vec<String> = Vec::new();
        for row in &mut view.rows {
            let key = row.cloud.key();
            listed.push(key.clone());
            let tracked = watches.watches.contains_key(&key);
            if !tracked && !row.going_down() {
                continue;
            }
            let watch = watches.watches.entry(key.clone()).or_default();
            watch.missing_reads = 0;
            // A sandbox that is being deleted is neither stopping nor up for these rules.
            let state = if row.is_deleting() {
                WorkspaceState::Unknown
            } else {
                row.state
            };
            let (report, since) = read(watch, state, now);
            row.stop = report;
            row.stopping_since_ms = since;
            if watch.requested_ms.is_none()
                && watch.stopping_since_ms.is_none()
                && watch.settled.is_none()
            {
                watches.watches.remove(&key);
            }
        }
        let readable = view.sandbox_problems.is_empty();
        let this_project = crate::files::path_key(&view.project);
        let mut dropped = Vec::new();
        for (key, watch) in watches.watches.iter_mut() {
            let Some(cloud) = &watch.cloud else {
                continue;
            };
            if listed.contains(key) {
                continue;
            }
            // Another project's load does not list this sandbox when sessions of other projects
            // use it: it says nothing about this Stop, so it is not a missing read.
            if !watch.projects.is_empty() && !watch.projects.contains(&this_project) {
                continue;
            }
            watch.missing_reads += 1;
            let followed = watch.requested_ms;
            let too_old = followed.is_some_and(|requested| {
                now.saturating_sub(requested) >= UNREAD_LIMIT.as_millis() as u64
            });
            if (readable && watch.missing_reads >= MISSING_READS) || too_old {
                dropped.push(key.clone());
                continue;
            }
            // A settled verdict (Stopped, or Still running) of a sandbox that is gone is dropped
            // quietly above; only a Stop still on its way is worth a line.
            if followed.is_none() {
                continue;
            }
            let why = if readable {
                "it is not in the listing (deleted, or used only by another project)".to_owned()
            } else {
                view.sandbox_problems
                    .iter()
                    .map(|p| p.problem.message())
                    .collect::<Vec<_>>()
                    .join("; ")
            };
            view.stop_unread.push(UnreadStop {
                cloud: cloud.clone(),
                text: format!("Could not read the state of {}: {why}", cloud.name),
            });
        }
        for key in dropped {
            watches.watches.remove(&key);
        }
    }
}
