//! What the app footer's cloud chip says about a project's cloud workspaces: how many run, and
//! whether any of them deserves a second look. It is a pure reading of a loaded
//! [`WorkspaceOverview`] (the same one the Cloud workspaces dialog shows, loaded by
//! [`CloudSessions::workspaces_overview`], which only lists and never wakes anything), so asking
//! for it costs no CLI call beyond that load.
//!
//! * The chip is visible only while something is [`WorkspaceState::Running`] or
//!   [`WorkspaceState::Starting`] ([`FooterStatus::visible`]).
//! * It turns to a warning when a workspace carries a [`Concern`] ([`FooterStatus::warns`]).
//! * [`CloudSessions::project_uses_cloud`] says whether a project is worth asking about at all.
use super::workspaces::{WorkspaceOverview, WorkspaceRow, WorkspaceState};
use super::{docker_config, project_cloud, CloudBackendKind, CloudSessions};
use std::time::Duration;

/// A workspace that is up and that no busy session uses for this long is flagged
/// ([`Concern::LongRunning`]). Six hours is well past a working session (a codespace's own idle
/// timeout is at most four hours and a sandbox's default TTL is two), so a workspace that is still up
/// then is most likely forgotten. The age is counted from creation, an UPPER BOUND, because
/// neither `gh` nor `sbx` reports uptime (see [`super::workspaces::CostEstimate`]).
pub const LONG_RUNNING: Duration = Duration::from_secs(6 * 60 * 60);

/// Why a workspace turns the chip to a warning.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Concern {
    /// The workspace failed: it may still draw compute and cannot be used.
    Failed,
    /// This app's store knows it, no live session uses it and it is up or coming up or down:
    /// nothing in Monoptah will ever stop it ([`WorkspaceRow::orphan`]). A stopped orphan costs
    /// nothing and is not flagged; a workspace this app did not create is never an orphan.
    Orphan,
    /// It has been up longer than [`LONG_RUNNING`] and no session of it is working.
    LongRunning,
}

impl Concern {
    pub fn label(self) -> &'static str {
        match self {
            Self::Failed => "failed",
            Self::Orphan => "no session uses it",
            Self::LongRunning => "running for hours with no session working",
        }
    }
}

/// One workspace the chip names: every running or starting one, and any other that has a
/// [`Concern`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FooterEntry {
    pub name: String,
    pub backend: CloudBackendKind,
    pub state: WorkspaceState,
    pub concern: Option<Concern>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FooterStatus {
    /// Workspaces that are running or starting.
    pub running: usize,
    /// What the chip names, running ones first (the overview's order).
    pub entries: Vec<FooterEntry>,
}

/// The concern of a row at `now_ms`; the most serious one when several apply.
pub fn concern_of(row: &WorkspaceRow, now_ms: u64) -> Option<Concern> {
    let up = matches!(
        row.state,
        WorkspaceState::Running | WorkspaceState::Starting
    );
    if row.state == WorkspaceState::Failed {
        Some(Concern::Failed)
    } else if row.orphan() && row.state.accruing() {
        Some(Concern::Orphan)
    } else if up
        && !row.busy()
        && row
            .age_secs(now_ms)
            .is_some_and(|age| age >= LONG_RUNNING.as_secs())
    {
        Some(Concern::LongRunning)
    } else {
        None
    }
}

impl FooterStatus {
    /// Reads `view` as of `now_ms` (the age of a workspace is measured against it, not against
    /// when the view was loaded: a cached view keeps aging).
    pub fn of(view: &WorkspaceOverview, now_ms: u64) -> Self {
        let mut status = Self::default();
        for row in &view.rows {
            let up = matches!(
                row.state,
                WorkspaceState::Running | WorkspaceState::Starting
            );
            let concern = concern_of(row, now_ms);
            status.running += usize::from(up);
            if up || concern.is_some() {
                status.entries.push(FooterEntry {
                    name: row.display_name.clone(),
                    backend: row.cloud.kind,
                    state: row.state,
                    concern,
                });
            }
        }
        status
    }

    /// The chip shows: something runs or starts.
    pub fn visible(&self) -> bool {
        self.running > 0
    }

    /// The chip takes the warning colour.
    pub fn warns(&self) -> bool {
        self.entries.iter().any(|e| e.concern.is_some())
    }
}

impl CloudSessions {
    /// Whether the footer chip has any reason to ask about `project`: it has saved cloud
    /// settings (Docker Sandbox or codespace) or a cloud session, live or moved. Reads the local
    /// store only: no `gh`, no `sbx`, no `git`. A project folder on a remote host (`remote://`)
    /// is never asked, and neither is one whose store cannot be read.
    pub fn project_uses_cloud(&self, project: &str) -> bool {
        if project.is_empty() || project.starts_with(crate::remote::REMOTE_PATH_PREFIX) {
            return false;
        }
        let saved = match self.store() {
            Ok(store) => {
                docker_config::load(&store, project).is_ok_and(|config| config.is_some())
                    || project_cloud::has_saved_settings(&store, project)
            }
            Err(_) => return false,
        };
        // `cloud_sessions_for` takes the store lock itself: the guard above is already gone.
        saved
            || self
                .cloud_sessions_for(project)
                .is_ok_and(|sessions| !sessions.is_empty())
    }
}
