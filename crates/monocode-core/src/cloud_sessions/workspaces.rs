//! The cloud workspaces overview: every codespace of the project's repository and every
//! Monoptah-made Docker sandbox, one [`WorkspaceRow`] each, so a failed or forgotten workspace
//! that still draws quota or credit can be found, stopped or deleted.
//!
//! * [`CloudSessions::workspaces_overview`] reads `gh codespace list` and `sbx ls` (nothing else:
//!   no `view`, no `ssh`, no `inspect`), so listing never wakes a stopped workspace. The result is
//!   cached for [`WORKSPACES_TTL`]; an explicit refresh passes `force`. BLOCKS on gh and sbx, so
//!   call it from a worker thread. A backend that cannot be read (CLI missing, signed out) is
//!   reported on the overview, never as an error: the other backend still lists.
//! * Which sessions use a workspace comes from the local store ([`CloudSessions::sessions_on`]).
//!   A workspace no live session uses is an orphan ([`WorkspaceRow::orphan`]). Sandbox names carry
//!   no repository, so a sandbox is listed when a session of the project uses it or when no session
//!   at all does; one that only other projects' sessions use is left out.
//! * Stopping and deleting reuse the existing paths ([`CloudSessions::stop_cloud`],
//!   [`CloudSessions::delete_cloud_outcome`]). A sandbox delete first stops the sandbox and waits
//!   until it is `stopped` ([`CloudSessions::delete_settled`]), because a sandbox that is stopping or
//!   hibernating may refuse `rm`.
//! * Costs are ESTIMATES: see the rate constants below.
use super::{
    start_record, CloudBackendKind, CloudRef, CloudSessions, CloudState, DeleteOutcome,
    InstanceSession,
};
use crate::codespaces::{self, AuthStatus};
use crate::sandboxes::{self, Prerequisites, SandboxInfo};
use std::time::{Duration, Instant};

/// How long a loaded overview is reused before the CLIs are asked again.
pub const WORKSPACES_TTL: Duration = Duration::from_secs(30);
/// How long a sandbox delete waits for the sandbox to reach `stopped` (a stop takes about two
/// minutes) before it gives up and leaves the sandbox alone.
pub const SETTLE_MAX_WAIT: Duration = Duration::from_secs(4 * 60);
/// How often a delete is retried when the sandbox still reports a transitional state.
const DELETE_RETRIES: u32 = 3;

// ESTIMATE ONLY. USD per hour of one Docker cloud sandbox while it runs, as listed on Docker's
// usage page; Docker bills per runtime second. Docker can change them and usage is reported late
// (up to 48 hours), so these numbers are for deciding what to stop, never for accounting.
/// Micro: 1 vCPU / 2g.
pub const DOCKER_RATE_MICRO: f64 = 0.07;
/// Small: 2 vCPU / 4g.
pub const DOCKER_RATE_SMALL: f64 = 0.14;
/// Medium: 4 vCPU / 8g.
pub const DOCKER_RATE_MEDIUM: f64 = 0.28;
/// Large: 8 vCPU / 16g.
pub const DOCKER_RATE_LARGE: f64 = 0.56;
/// XL: 16 vCPU / 32g.
pub const DOCKER_RATE_XL: f64 = 1.12;
// ESTIMATE ONLY. USD per core-hour of a running codespace (GitHub's published pay-as-you-go
// compute rate). The free monthly allowance, storage and prebuilds are not modelled.
pub const CODESPACES_RATE_PER_CORE_HOUR: f64 = 0.09;

/// One Docker cloud sandbox size.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DockerTier {
    pub name: &'static str,
    pub cpus: u32,
    pub memory_gib: u64,
    /// USD per hour while running (estimate, see [`DOCKER_RATE_MICRO`]).
    pub rate: f64,
}

/// The sizes `sbx create --cpus --memory` accepts, smallest first.
pub const DOCKER_TIERS: [DockerTier; 5] = [
    DockerTier {
        name: "Micro",
        cpus: 1,
        memory_gib: 2,
        rate: DOCKER_RATE_MICRO,
    },
    DockerTier {
        name: "Small",
        cpus: 2,
        memory_gib: 4,
        rate: DOCKER_RATE_SMALL,
    },
    DockerTier {
        name: "Medium",
        cpus: 4,
        memory_gib: 8,
        rate: DOCKER_RATE_MEDIUM,
    },
    DockerTier {
        name: "Large",
        cpus: 8,
        memory_gib: 16,
        rate: DOCKER_RATE_LARGE,
    },
    DockerTier {
        name: "XL",
        cpus: 16,
        memory_gib: 32,
        rate: DOCKER_RATE_XL,
    },
];

/// The tier of a sandbox with `cpus` and `memory_mib` (0 = not reported: the CPU count decides).
/// `None` for a shape that is not a tier, which has no rate.
pub fn docker_tier(cpus: u32, memory_mib: u64) -> Option<DockerTier> {
    DOCKER_TIERS
        .into_iter()
        .find(|t| t.cpus == cpus && (memory_mib == 0 || memory_mib == t.memory_gib * 1024))
}

/// Cores of the codespace machine types GitHub names `machineName`.
const CODESPACE_CORES: [(&str, u32); 6] = [
    ("basicLinux32gb", 2),
    ("standardLinux", 4),
    ("standardLinux32gb", 4),
    ("premiumLinux", 8),
    ("largePremiumLinux", 16),
    ("xLargePremiumLinux", 32),
];

/// Cores of a codespace machine, `None` for a machine name this table does not know.
pub fn codespace_cores(machine: &str) -> Option<u32> {
    CODESPACE_CORES
        .iter()
        .find(|(name, _)| *name == machine)
        .map(|(_, cores)| *cores)
}

/// Where a workspace stands, in the words the overview shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorkspaceState {
    Running,
    Starting,
    Stopping,
    /// A sandbox on its way to `stopped`; it may refuse a delete until it gets there.
    Hibernating,
    Stopped,
    Failed,
    Unknown,
}

impl WorkspaceState {
    /// From a `gh codespace list` state.
    pub fn of_codespace(raw: &str) -> Self {
        match raw {
            "Available" => Self::Running,
            "Shutdown" | "Archived" => Self::Stopped,
            "ShuttingDown" => Self::Stopping,
            "Failed" | "Unavailable" | "Deleted" => Self::Failed,
            other => match CloudState::from_gh(other) {
                CloudState::Starting => Self::Starting,
                _ => Self::Unknown,
            },
        }
    }
    /// From an `sbx ls` state.
    pub fn of_sandbox(raw: &str) -> Self {
        let lower = raw.trim().to_ascii_lowercase();
        if matches!(lower.as_str(), "hibernating" | "hibernated") {
            return Self::Hibernating;
        }
        match sandboxes::display_state(raw) {
            CloudState::Available => Self::Running,
            CloudState::Starting => Self::Starting,
            CloudState::Stopping => Self::Stopping,
            CloudState::Stopped => Self::Stopped,
            CloudState::Unavailable if lower == "deleting" => Self::Stopping,
            CloudState::Unavailable => Self::Failed,
            CloudState::Unknown => Self::Unknown,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Running => "Running",
            Self::Starting => "Starting",
            Self::Stopping => "Stopping",
            Self::Hibernating => "Hibernating",
            Self::Stopped => "Stopped",
            Self::Failed => "Failed",
            Self::Unknown => "Unknown",
        }
    }
    /// The workspace is, or may still be, drawing compute.
    pub fn accruing(self) -> bool {
        matches!(
            self,
            Self::Running | Self::Starting | Self::Stopping | Self::Hibernating
        )
    }
    /// Stop does something: the workspace is up or coming up.
    pub fn can_stop(self) -> bool {
        matches!(self, Self::Running | Self::Starting)
    }
    /// The state is changing by itself; a delete of a sandbox has to wait for it to settle.
    pub fn settling(self) -> bool {
        matches!(self, Self::Starting | Self::Stopping | Self::Hibernating)
    }
}

/// What a workspace has probably cost, from its size and how long it ran. Never exact.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CostEstimate {
    /// USD per hour while it runs; `None` when the size has no known rate.
    pub rate_per_hour: Option<f64>,
    /// The workspace is, or may be, drawing compute right now.
    pub accruing: bool,
    /// Seconds it has run, an UPPER BOUND: neither gh nor sbx reports uptime, so a workspace
    /// that is up is counted from its creation.
    pub runtime_secs: Option<u64>,
    /// `rate × runtime`, when both are known.
    pub total: Option<f64>,
}

impl CostEstimate {
    pub fn compute(
        rate_per_hour: Option<f64>,
        state: WorkspaceState,
        created_ms: Option<u64>,
        now_ms: u64,
    ) -> Self {
        let accruing = state.accruing();
        let runtime_secs = created_ms
            .filter(|_| accruing)
            .map(|created| now_ms.saturating_sub(created) / 1000);
        let total = rate_per_hour
            .zip(runtime_secs)
            .map(|(rate, secs)| rate * secs as f64 / 3600.0);
        Self {
            rate_per_hour,
            accruing,
            runtime_secs,
            total,
        }
    }
}

/// A live session that runs in a workspace.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UsingSession {
    pub id: i64,
    pub title: String,
    /// A turn is running, waits for input, or the start is in progress.
    pub busy: bool,
}

/// One cloud workspace.
#[derive(Clone, Debug, PartialEq)]
pub struct WorkspaceRow {
    pub cloud: CloudRef,
    /// The sandbox id (`sbx_...`).
    pub id: Option<String>,
    pub display_name: String,
    pub branch: Option<String>,
    /// Machine type or size, e.g. `basicLinux32gb · 2 cores`, `Medium · 4 vCPU / 8g`.
    pub size: String,
    pub raw_state: String,
    pub state: WorkspaceState,
    pub created_ms: Option<u64>,
    pub last_used_ms: Option<u64>,
    pub sessions: Vec<UsingSession>,
    /// What deleting would lose, as recorded when its sessions were saved.
    pub unsaved: Vec<String>,
    pub cost: CostEstimate,
    /// The backend has a web page for it (a codespace).
    pub has_web: bool,
}

impl WorkspaceRow {
    /// No live session uses it: nothing in Monoptah will ever stop it.
    pub fn orphan(&self) -> bool {
        self.sessions.is_empty()
    }
    pub fn busy(&self) -> bool {
        self.sessions.iter().any(|s| s.busy)
    }
    /// Seconds since creation at `now_ms`.
    pub fn age_secs(&self, now_ms: u64) -> Option<u64> {
        self.created_ms.map(|c| now_ms.saturating_sub(c) / 1000)
    }
}

/// Everything one load found.
#[derive(Clone, Debug, PartialEq)]
pub struct WorkspaceOverview {
    pub project: String,
    /// `owner/repo` of the project's origin, when it is on GitHub.
    pub repo: Option<String>,
    pub loaded_ms: u64,
    pub rows: Vec<WorkspaceRow>,
    /// The `gh auth status` check; `None` when the project is not on GitHub (nothing to ask).
    pub gh: Option<Result<AuthStatus, String>>,
    /// `gh codespace list` failed although gh is signed in.
    pub codespaces_error: Option<String>,
    /// Why sandboxes could not be listed; `None` when they could.
    pub sandbox_problem: Option<Prerequisites>,
}

/// Rows that could be drawing compute first, orphans before used ones, then oldest first.
fn order(rows: &mut [WorkspaceRow]) {
    rows.sort_by_key(|r| {
        (
            !r.state.accruing(),
            !r.orphan(),
            r.created_ms.unwrap_or(u64::MAX),
            r.cloud.name.clone(),
        )
    });
}

/// Epoch milliseconds of an RFC 3339 time, or of a bare number (seconds or milliseconds).
pub fn parse_time_ms(text: &str) -> Option<u64> {
    let text = text.trim();
    if let Ok(time) = chrono::DateTime::parse_from_rfc3339(text) {
        return u64::try_from(time.timestamp_millis()).ok();
    }
    let n: u64 = text.parse().ok()?;
    Some(if n >= 100_000_000_000 { n } else { n * 1000 })
}

fn memory_text(mib: u64) -> String {
    if mib >= 1024 && mib.is_multiple_of(1024) {
        format!("{}g", mib / 1024)
    } else {
        format!("{mib}m")
    }
}

/// `"Medium · 4 vCPU / 8g"`, or just the shape when it is not a tier.
fn sandbox_size(info: &SandboxInfo) -> String {
    let shape = match (info.cpus, info.memory_mib) {
        (0, 0) => return "Size unknown".into(),
        (cpus, 0) => format!("{cpus} vCPU"),
        (0, mib) => memory_text(mib),
        (cpus, mib) => format!("{cpus} vCPU / {}", memory_text(mib)),
    };
    match docker_tier(info.cpus, info.memory_mib) {
        Some(tier) => format!("{} \u{b7} {shape}", tier.name),
        None => shape,
    }
}

fn codespace_size(machine: &str) -> String {
    match (machine.is_empty(), codespace_cores(machine)) {
        (true, _) => "Machine unknown".into(),
        (false, Some(cores)) => format!("{machine} \u{b7} {cores} cores"),
        (false, None) => machine.to_owned(),
    }
}

fn using(sessions: &[InstanceSession]) -> Vec<UsingSession> {
    sessions
        .iter()
        .map(|s| UsingSession {
            id: s.id,
            title: s.title.clone(),
            busy: s.busy(),
        })
        .collect()
}

impl CloudSessions {
    /// A loaded overview of `project` that is still fresh, with no CLI call.
    pub fn cached_workspaces(&self, project: &str) -> Option<WorkspaceOverview> {
        let views = self.workspace_views.lock().ok()?;
        let (at, view) = views.get(project)?;
        (at.elapsed() < WORKSPACES_TTL).then(|| view.clone())
    }

    /// Forgets every loaded overview, after a stop or delete changed what they show.
    pub fn forget_workspace_views(&self) {
        if let Ok(mut views) = self.workspace_views.lock() {
            views.clear();
        }
    }

    /// The workspaces of `project`. Fresh cached data is reused unless `force`. BLOCKS on gh and
    /// sbx; read-only (see the module docs).
    pub fn workspaces_overview(&self, project: &str, force: bool) -> WorkspaceOverview {
        if !force {
            if let Some(view) = self.cached_workspaces(project) {
                return view;
            }
        }
        let view = self.load_workspaces(project);
        if let Ok(mut views) = self.workspace_views.lock() {
            views.insert(project.to_owned(), (Instant::now(), view.clone()));
        }
        view
    }

    fn load_workspaces(&self, project: &str) -> WorkspaceOverview {
        let now = (self.clock)();
        let mut view = WorkspaceOverview {
            project: project.to_owned(),
            repo: None,
            loaded_ms: now,
            rows: Vec::new(),
            gh: None,
            codespaces_error: None,
            sandbox_problem: None,
        };
        if let Ok(repo) = self.repo_for(project) {
            let auth = codespaces::auth_status(&*self.gh);
            let ready = auth
                .as_ref()
                .is_ok_and(|status| status.require_codespace_scope().is_ok());
            view.gh = Some(auth);
            if ready {
                match codespaces::list(&*self.gh, &repo) {
                    Ok(list) => view
                        .rows
                        .extend(list.into_iter().map(|c| self.codespace_row(c, now))),
                    Err(error) => view.codespaces_error = Some(error),
                }
            }
            view.repo = Some(repo);
        }
        match self.sandbox_tools() {
            Err(error) => view.sandbox_problem = Some(Prerequisites::Unreachable(error)),
            Ok((runner, _)) => match sandboxes::list(&*runner) {
                Ok(list) => {
                    let key = crate::files::path_key(project);
                    view.rows.extend(
                        list.into_iter()
                            .filter(|info| info.agent == "shell" && is_ours(&info.name))
                            .filter_map(|info| self.sandbox_row(info, &key, now)),
                    );
                }
                Err(error) => view.sandbox_problem = Some(sandboxes::access_problem(&error)),
            },
        }
        order(&mut view.rows);
        view
    }

    fn codespace_row(&self, c: codespaces::Codespace, now: u64) -> WorkspaceRow {
        let cloud = CloudRef::codespace(&c.name);
        let state = WorkspaceState::of_codespace(&c.state);
        let created_ms = parse_time_ms(&c.created_at);
        let rate = codespace_cores(&c.machine_name)
            .map(|cores| f64::from(cores) * CODESPACES_RATE_PER_CORE_HOUR);
        WorkspaceRow {
            sessions: using(&self.sessions_on(&cloud)),
            unsaved: self.unsaved_for_cloud(&cloud),
            display_name: if c.display_name.is_empty() {
                c.name.clone()
            } else {
                c.display_name
            },
            branch: c.git_status.map(|g| g.git_ref).filter(|b| !b.is_empty()),
            size: codespace_size(&c.machine_name),
            raw_state: c.state,
            state,
            created_ms,
            last_used_ms: parse_time_ms(&c.last_used_at),
            cost: CostEstimate::compute(rate, state, created_ms, now),
            has_web: true,
            id: None,
            cloud,
        }
    }

    /// `None` when only other projects' sessions use the sandbox.
    fn sandbox_row(&self, info: SandboxInfo, project_key: &str, now: u64) -> Option<WorkspaceRow> {
        let cloud = CloudRef::sandbox(&info.name);
        let sessions = self.sessions_on(&cloud);
        let mine = sessions.iter().any(|s| {
            s.local_project
                .as_deref()
                .is_some_and(|p| crate::files::path_key(p) == project_key)
        });
        if !sessions.is_empty() && !mine {
            return None;
        }
        let state = WorkspaceState::of_sandbox(&info.state);
        let created_ms = info.created_at.as_deref().and_then(parse_time_ms);
        let rate = docker_tier(info.cpus, info.memory_mib).map(|t| t.rate);
        let branch = self.store().ok().and_then(|store| {
            sessions
                .iter()
                .find_map(|s| start_record(&store, s.id).and_then(|r| r.branch))
        });
        Some(WorkspaceRow {
            size: sandbox_size(&info),
            sessions: using(&sessions),
            unsaved: self.unsaved_for_cloud(&cloud),
            display_name: info.name,
            id: Some(info.id),
            branch,
            raw_state: info.state,
            state,
            created_ms,
            last_used_ms: None,
            cost: CostEstimate::compute(rate, state, created_ms, now),
            has_web: false,
            cloud,
        })
    }

    /// Waits until a sandbox can be deleted: stops it when it is up, then polls until it reports
    /// `stopped`. A codespace needs no wait. `Err` when it is still changing state after
    /// [`SETTLE_MAX_WAIT`]; nothing is deleted then. A sandbox that is already gone is `Ok`.
    pub fn settle_for_delete(&self, cloud: &CloudRef) -> Result<(), String> {
        if cloud.kind != CloudBackendKind::DockerSandbox {
            return Ok(());
        }
        let backend = self.backend(cloud.kind)?;
        let mut waited = Duration::ZERO;
        let mut asked = false;
        loop {
            let raw = match backend.state(&cloud.name) {
                Ok(raw) => raw,
                Err(error) if backend.is_missing_error(&error) => return Ok(()),
                Err(error) => return Err(error),
            };
            let state = WorkspaceState::of_sandbox(&raw);
            match state {
                WorkspaceState::Stopped | WorkspaceState::Failed | WorkspaceState::Unknown => {
                    return Ok(());
                }
                WorkspaceState::Running | WorkspaceState::Starting if !asked => {
                    self.stop_cloud(cloud)?;
                    asked = true;
                }
                _ => {}
            }
            if waited >= SETTLE_MAX_WAIT {
                return Err(format!(
                    "{} is still {} after {} minutes, so it was not deleted. Try Delete again in a minute.",
                    cloud.name,
                    state.label().to_ascii_lowercase(),
                    SETTLE_MAX_WAIT.as_secs() / 60
                ));
            }
            (self.sleep)(self.ready_poll);
            waited += self.ready_poll;
        }
    }

    /// [`Self::delete_cloud_outcome`] after [`Self::settle_for_delete`]; a sandbox whose delete is
    /// refused because it is still stopping is waited for and deleted again (a few times).
    pub fn delete_settled(&self, cloud: &CloudRef) -> Result<DeleteOutcome, String> {
        self.settle_for_delete(cloud)?;
        let mut retries = 0;
        loop {
            match self.delete_cloud_outcome(cloud) {
                Err(error)
                    if cloud.kind == CloudBackendKind::DockerSandbox
                        && retries < DELETE_RETRIES
                        && looks_transitional(&error) =>
                {
                    retries += 1;
                    (self.sleep)(self.ready_poll);
                    self.settle_for_delete(cloud)?;
                }
                other => return other,
            }
        }
    }
}

/// A sandbox this app makes is named `monoptah-<branch>-<id>` (`sandboxes::NAME_PREFIX`); others are not ours to list.
fn is_ours(name: &str) -> bool {
    name.starts_with(sandboxes::NAME_PREFIX)
}

/// An sbx refusal that means "not yet", as opposed to a real failure.
fn looks_transitional(error: &str) -> bool {
    let lower = error.to_ascii_lowercase();
    [
        "hibernat",
        "stopping",
        "not stopped",
        "must be stopped",
        "still running",
        "in transition",
        "try again",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}
