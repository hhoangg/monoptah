//! The cloud workspaces overview: every codespace of the project's repository and every
//! Monoptah-made Docker sandbox, one [`WorkspaceRow`] each, so a failed or forgotten workspace
//! that still draws quota or credit can be found, stopped or deleted.
//!
//! * [`CloudSessions::workspaces_overview`] reads `gh codespace list` and `sbx ls` (nothing else:
//!   no `view`, no `ssh`, no `inspect`), so listing never wakes a stopped workspace. Sandboxes are
//!   listed once per Docker account the project can reach, as that account, and each row keeps the
//!   account it was listed under ([`WorkspaceRow::account`]). The result is cached for
//!   [`WORKSPACES_TTL`]; an explicit refresh passes `force`. BLOCKS on gh and sbx, so call it from a
//!   worker thread. A backend that cannot be read (CLI missing, signed out, an account that cannot
//!   sign in) is reported on the overview, never as an error: the rest still lists.
//! * Whose workspace it is. Only a workspace this app's own store knows ([`WorkspaceRow::ours`]: a
//!   session, a start record, a move, a saved copy or a Docker account pin names it) can be called
//!   an orphan. Every other one is `Not created by this app`: another app, another computer or the
//!   user by hand made it, and Monoptah cannot see what is in it. Deleting such a workspace takes a
//!   stronger confirmation ([`DeletePreview::needs_acknowledgement`]).
//! * Which sessions use a workspace comes from the local store: the live sessions bound to it and
//!   the local sessions moved there. Sandbox names carry no repository, so a sandbox is listed when
//!   a session of the project uses it or when no session at all does; one that only other
//!   projects' sessions use is left out.
//! * Stopping and deleting reuse the existing paths ([`CloudSessions::stop_cloud`],
//!   [`CloudSessions::delete_cloud_outcome`]) after [`CloudSessions::use_listed_account`] made them
//!   run as the account the row was listed under. A sandbox delete first stops the sandbox and
//!   waits until it is `stopped` ([`CloudSessions::delete_settled`]), because a sandbox that is
//!   stopping or hibernating may refuse `rm`, and runs the caller's guard once more right before
//!   the delete. What a delete would lose is read when the user is asked
//!   ([`CloudSessions::delete_preview`]) and again before the delete, which refuses what the user
//!   did not see.
//! * Costs are ESTIMATES: see the rate constants below.
use super::move_session::{pending_moves, MoveTarget};
use super::{
    convert, docker_config, start_record, CloudBackendKind, CloudRef, CloudSessions, CloudState,
    DeleteOutcome, InstanceSession,
};
use crate::codespaces::{self, AuthStatus};
use crate::sandboxes::{self, Prerequisites, SandboxInfo};
use std::collections::BTreeSet;
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
    /// A local conversation that was moved into the workspace and is written there now. Deleting
    /// the workspace would lose the turns that were not synced back.
    pub moved: bool,
}

/// What a stop or delete acts on: the workspace and, for a sandbox, the Docker account it was
/// listed under (`None`: the machine's `sbx login`). A codespace has no account.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkspaceTarget {
    pub cloud: CloudRef,
    pub account: Option<String>,
}

/// What GitHub last reported about the git state of a codespace.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GitState {
    pub uncommitted: bool,
    pub unpushed: bool,
}

/// One cloud workspace.
#[derive(Clone, Debug, PartialEq)]
pub struct WorkspaceRow {
    pub cloud: CloudRef,
    /// The sandbox id (`sbx_...`).
    pub id: Option<String>,
    /// The Docker account a sandbox was listed under; `None` for the machine login and for a
    /// codespace. Stop and Delete run as this account.
    pub account: Option<String>,
    /// How to name [`Self::account`] to the user (its Docker ID).
    pub account_label: Option<String>,
    pub display_name: String,
    pub branch: Option<String>,
    /// Machine type or size, e.g. `basicLinux32gb · 2 cores`, `Medium · 4 vCPU / 8g`.
    pub size: String,
    pub raw_state: String,
    pub state: WorkspaceState,
    pub created_ms: Option<u64>,
    pub last_used_ms: Option<u64>,
    /// This app's own store knows the workspace: a session, a start record, a move, a saved copy
    /// or a Docker account pin names it. `false` means it was made elsewhere (another app or
    /// computer, or by hand), and Monoptah cannot tell what is in it or who uses it.
    pub ours: bool,
    pub sessions: Vec<UsingSession>,
    /// What deleting would lose, as recorded when its sessions were saved.
    pub unsaved: Vec<String>,
    /// The git state GitHub last reported (a codespace only).
    pub git: Option<GitState>,
    pub cost: CostEstimate,
    /// The backend has a web page for it (a codespace).
    pub has_web: bool,
}

impl WorkspaceRow {
    /// This app made it and no live session uses it: nothing in Monoptah will ever stop it. A
    /// workspace that is not [`Self::ours`] is never an orphan: whoever made it may be using it.
    pub fn orphan(&self) -> bool {
        self.ours && self.sessions.is_empty()
    }
    pub fn busy(&self) -> bool {
        self.sessions.iter().any(|s| s.busy)
    }
    /// What Stop and Delete act on.
    pub fn target(&self) -> WorkspaceTarget {
        WorkspaceTarget {
            cloud: self.cloud.clone(),
            account: self.account.clone(),
        }
    }
    /// Names this row among the others: the workspace and the account it was listed under.
    pub fn key(&self) -> String {
        target_key(&self.cloud, self.account.as_deref())
    }
    /// Seconds since creation at `now_ms`.
    pub fn age_secs(&self, now_ms: u64) -> Option<u64> {
        self.created_ms.map(|c| now_ms.saturating_sub(c) / 1000)
    }
}

/// [`WorkspaceRow::key`] of a workspace and account.
pub fn target_key(cloud: &CloudRef, account: Option<&str>) -> String {
    format!("{}\u{1f}{}", cloud.key(), account.unwrap_or_default())
}

/// Why the sandboxes of one Docker account could not be listed.
#[derive(Clone, Debug, PartialEq)]
pub struct SandboxProblem {
    /// How to name the account to the user; `None` for the machine login.
    pub account: Option<String>,
    pub problem: Prerequisites,
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
    /// The Docker accounts whose sandboxes could not be listed, and why. Empty when every
    /// account could be read.
    pub sandbox_problems: Vec<SandboxProblem>,
}

/// What the user must be told, and has to confirm, before a workspace is deleted. Read when the
/// confirmation opens ([`CloudSessions::delete_preview`]); the delete reads it again and refuses
/// what [`Self::lines`] did not name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeletePreview {
    /// What would be lost, as far as it can be read. The delete is refused when it finds a line
    /// that is not in the list the user confirmed.
    pub lines: Vec<String>,
    /// What cannot be read, and so may be lost too.
    pub notes: Vec<String>,
    /// Why the delete will be refused whatever the user answers.
    pub blockers: Vec<String>,
    /// This app's store knows the workspace, see [`WorkspaceRow::ours`].
    pub ours: bool,
    /// Monoptah cannot look inside (a sandbox is not started to read it).
    pub contents_unknown: bool,
}

impl DeletePreview {
    /// A plain Delete button is not enough: the workspace is not this app's, something would be
    /// lost, or what is in it cannot be read. The user has to say they understood.
    pub fn needs_acknowledgement(&self) -> bool {
        !self.ours || !self.lines.is_empty() || self.contents_unknown
    }
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
            moved: false,
        })
        .collect()
}

/// The line a delete shows when the codespace has changes that were not committed.
pub const LINE_UNCOMMITTED: &str = "It has changes that were not committed.";
/// The line a delete shows when the codespace has commits that were not pushed.
pub const LINE_UNPUSHED: &str = "It has commits that were not pushed.";
/// The line a delete shows when GitHub does not say what the codespace holds.
pub const LINE_GIT_UNKNOWN: &str =
    "GitHub did not report whether it has uncommitted or unpushed changes.";
/// What a sandbox delete cannot read.
pub const NOTE_SANDBOX_CONTENTS: &str = "Monoptah cannot look inside a sandbox without starting it, which would bill it again. Anything in it that was not pushed or copied to this computer is lost.";
/// What a delete of a workspace that is not this app's cannot read.
pub const NOTE_NOT_OURS: &str = "Monoptah did not create this workspace and cannot see who uses it or what is in it. Another app, another computer or a person may be working in it right now.";

impl CloudSessions {
    /// A loaded overview of `project` that is still fresh, with no CLI call.
    pub fn cached_workspaces(&self, project: &str) -> Option<WorkspaceOverview> {
        let views = self.workspace_views.lock().ok()?;
        let (at, view) = views.get(project)?;
        (at.elapsed() < WORKSPACES_TTL).then(|| view.clone())
    }

    /// Forgets every loaded overview, after a stop or delete changed what they show. Stopping and
    /// deleting do this themselves ([`Self::stop_cloud`], [`Self::delete_cloud_outcome`]), whoever
    /// called them.
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
            sandbox_problems: Vec::new(),
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
        self.load_sandboxes(project, now, &mut view);
        order(&mut view.rows);
        view
    }

    /// The sandboxes of `project`, once per Docker account it can reach.
    fn load_sandboxes(&self, project: &str, now: u64, view: &mut WorkspaceOverview) {
        let (runner, _) = match self.sandbox_tools() {
            Ok(tools) => tools,
            Err(error) => {
                view.sandbox_problems.push(SandboxProblem {
                    account: None,
                    problem: Prerequisites::Unreachable(error),
                });
                return;
            }
        };
        let accounts = match self.accounts_to_list(project) {
            Ok(accounts) => accounts,
            Err(error) => {
                view.sandbox_problems.push(SandboxProblem {
                    account: None,
                    problem: Prerequisites::DockerAccount(error),
                });
                return;
            }
        };
        let key = crate::files::path_key(project);
        let mut seen: Vec<(String, String)> = Vec::new();
        for account in accounts {
            let label = account.as_deref().and_then(|id| self.account_label(id));
            match sandboxes::list(&sandboxes::as_account(&*runner, account.as_deref())) {
                Ok(list) => {
                    for info in list
                        .into_iter()
                        .filter(|info| info.agent == "shell" && is_ours_by_name(&info.name))
                    {
                        // A sandbox lives in one account; it is never shown twice.
                        let identity = (info.id.clone(), info.name.clone());
                        if seen.contains(&identity) {
                            continue;
                        }
                        seen.push(identity);
                        if let Some(row) =
                            self.sandbox_row(info, &key, account.as_deref(), label.clone(), now)
                        {
                            view.rows.push(row);
                        }
                    }
                }
                Err(error) => view.sandbox_problems.push(SandboxProblem {
                    account: label,
                    problem: sandboxes::access_problem(&error),
                }),
            }
        }
    }

    /// The Docker accounts to list `project`'s sandboxes as: the account of every sandbox its
    /// sessions are bound to (a sandbox lives in the account that created it, which may differ
    /// from the project's choice today) and the project's own account, or the machine login when
    /// it names none. `Err` when the account of a bound sandbox cannot be read: it is never
    /// taken for the machine login, which would list the wrong account.
    fn accounts_to_list(&self, project: &str) -> Result<Vec<Option<String>>, String> {
        let bound: BTreeSet<String> = self
            .cloud_sessions_for(project)
            .unwrap_or_default()
            .into_iter()
            .filter(|s| s.cloud.kind == CloudBackendKind::DockerSandbox)
            .map(|s| s.cloud.name)
            .collect();
        let mut accounts: BTreeSet<Option<String>> = BTreeSet::new();
        for name in &bound {
            accounts.insert(self.try_sandbox_account(name)?);
        }
        let configured = {
            let store = self.store()?;
            docker_config::load(&store, project)?.and_then(|config| config.docker_account)
        };
        accounts.insert(configured);
        Ok(accounts.into_iter().collect())
    }

    /// How to name Docker account `id` to the user.
    fn account_label(&self, id: &str) -> Option<String> {
        let store = self.store().ok()?;
        Some(sandboxes::account::describe(&store, id))
    }

    fn codespace_row(&self, c: codespaces::Codespace, now: u64) -> WorkspaceRow {
        let cloud = CloudRef::codespace(&c.name);
        let state = WorkspaceState::of_codespace(&c.state);
        let created_ms = parse_time_ms(&c.created_at);
        let rate = codespace_cores(&c.machine_name)
            .map(|cores| f64::from(cores) * CODESPACES_RATE_PER_CORE_HOUR);
        let git = c.git_status.as_ref().map(|g| GitState {
            uncommitted: g.has_uncommitted_changes,
            unpushed: g.has_unpushed_changes,
        });
        WorkspaceRow {
            sessions: self.sessions_of_row(&cloud),
            unsaved: self.unsaved_for_cloud(&cloud),
            ours: self.known_to_this_app(&cloud),
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
            git,
            id: None,
            account: None,
            account_label: None,
            cloud,
        }
    }

    /// `None` when only other projects' sessions use the sandbox.
    fn sandbox_row(
        &self,
        info: SandboxInfo,
        project_key: &str,
        account: Option<&str>,
        account_label: Option<String>,
        now: u64,
    ) -> Option<WorkspaceRow> {
        let cloud = CloudRef::sandbox(&info.name);
        let sessions = self.sessions_of_row(&cloud);
        let live = self.sessions_on(&cloud);
        let mine = live
            .iter()
            .filter_map(|s| s.local_project.as_deref())
            .chain(self.moved_projects(&cloud).iter().map(String::as_str))
            .any(|p| crate::files::path_key(p) == project_key);
        if !sessions.is_empty() && !mine {
            return None;
        }
        let state = WorkspaceState::of_sandbox(&info.state);
        let created_ms = info.created_at.as_deref().and_then(parse_time_ms);
        let rate = docker_tier(info.cpus, info.memory_mib).map(|t| t.rate);
        let branch = self.store().ok().and_then(|store| {
            live.iter()
                .find_map(|s| start_record(&store, s.id).and_then(|r| r.branch))
        });
        Some(WorkspaceRow {
            size: sandbox_size(&info),
            sessions,
            unsaved: self.unsaved_for_cloud(&cloud),
            ours: self.known_to_this_app(&cloud),
            display_name: info.name,
            id: Some(info.id),
            account: account.map(str::to_owned),
            account_label,
            branch,
            raw_state: info.state,
            state,
            created_ms,
            last_used_ms: None,
            cost: CostEstimate::compute(rate, state, created_ms, now),
            has_web: false,
            git: None,
            cloud,
        })
    }

    /// The sessions a row names: the live sessions bound to the workspace (the Host record that
    /// only backs a moved local session is the local session's plumbing, not a session of its
    /// own) and the local sessions moved into it.
    fn sessions_of_row(&self, cloud: &CloudRef) -> Vec<UsingSession> {
        let hidden: Vec<i64> = match self.store() {
            Ok(store) => store
                .cloud_session_ids(cloud)
                .unwrap_or_default()
                .into_iter()
                .filter(|&id| {
                    store
                        .remote_session(id)
                        .ok()
                        .flatten()
                        .is_some_and(|record| store.host_record_is_hidden(&record))
                })
                .collect(),
            Err(_) => Vec::new(),
        };
        let mut sessions = using(
            &self
                .sessions_on(cloud)
                .into_iter()
                .filter(|s| !hidden.contains(&s.id))
                .collect::<Vec<_>>(),
        );
        sessions.extend(self.moved_sessions(cloud).0);
        sessions
    }

    /// The local sessions whose location is `cloud`, and whether a stored location could not be
    /// read at all (such a session fails closed everywhere: it may live anywhere).
    fn moved_sessions(&self, cloud: &CloudRef) -> (Vec<UsingSession>, bool) {
        let Ok(store) = self.store() else {
            return (Vec::new(), true);
        };
        let Ok(located) = store.cloud_located_raw() else {
            return (Vec::new(), true);
        };
        let titles: std::collections::HashMap<i64, String> = store
            .sessions()
            .unwrap_or_default()
            .into_iter()
            .map(|s| (s.id, s.title))
            .collect();
        let mut unreadable = false;
        let mut sessions = Vec::new();
        for (id, location) in located {
            match location {
                None => unreadable = true,
                Some(location) if location.cloud == *cloud => {
                    // The Host record behind a moved session carries its activity flags.
                    let busy = store
                        .linked_remote_session(id)
                        .ok()
                        .flatten()
                        .is_some_and(|(_, record)| record.running || record.needs_input);
                    sessions.push(UsingSession {
                        id,
                        title: titles.get(&id).cloned().unwrap_or_default(),
                        busy,
                        moved: true,
                    });
                }
                Some(_) => {}
            }
        }
        (sessions, unreadable)
    }

    /// The project folders of the local sessions moved into `cloud`.
    fn moved_projects(&self, cloud: &CloudRef) -> Vec<String> {
        let Ok(store) = self.store() else {
            return Vec::new();
        };
        store
            .cloud_located()
            .unwrap_or_default()
            .into_iter()
            .filter(|(_, location)| location.cloud == *cloud)
            .map(|(_, location)| location.local_cwd)
            .collect()
    }

    /// Whether this app's own store knows `cloud`: a session (archived ones too), a start record,
    /// a move, a local session moved there or converted from it, a saved list of what a delete
    /// would lose, or a Docker account pin. Anything else was made elsewhere (see
    /// [`WorkspaceRow::ours`]); a store that cannot be read knows nothing.
    pub fn known_to_this_app(&self, cloud: &CloudRef) -> bool {
        let Ok(store) = self.store() else {
            return false;
        };
        if store
            .cloud_session_ids(cloud)
            .is_ok_and(|ids| !ids.is_empty())
        {
            return true;
        }
        if super::pending_starts(&store)
            .iter()
            .any(|record| record.cloud().as_ref() == Some(cloud))
        {
            return true;
        }
        if store
            .cloud_located()
            .is_ok_and(|located| located.iter().any(|(_, l)| l.cloud == *cloud))
        {
            return true;
        }
        if cloud.kind == CloudBackendKind::Codespaces
            && store
                .codespace_origins()
                .is_ok_and(|origins| origins.iter().any(|(_, name)| *name == cloud.name))
        {
            return true;
        }
        if pending_moves(&store).iter().any(|m| {
            m.workspace
                .as_ref()
                .is_some_and(|w| w.backend == cloud.kind && w.name == cloud.name)
                || matches!(
                    &m.target,
                    MoveTarget::Existing { backend, name }
                        if *backend == cloud.kind && *name == cloud.name
                )
        }) {
            return true;
        }
        let saved_list = store
            .setting(&convert::unsaved_key(cloud))
            .ok()
            .flatten()
            .and_then(|raw| serde_json::from_str::<Vec<String>>(&raw).ok())
            .is_some_and(|lines| !lines.is_empty());
        saved_list
            || (cloud.kind == CloudBackendKind::DockerSandbox
                && sandboxes::account::pinned(&store, &cloud.name)
                    .ok()
                    .flatten()
                    .is_some())
    }

    /// Makes the calls that follow ([`Self::stop_cloud`], [`Self::delete_cloud_outcome`], state
    /// reads) run as the Docker account the sandbox was LISTED under. A sandbox lives in one
    /// account; asked as another, `sbx` says "not found", which a delete takes for "already gone".
    ///
    /// A sandbox the app has a pin for keeps it: `Err` when the listing and the pin disagree
    /// (refresh and look again), never a silent switch. Only a sandbox without a pin is taken from
    /// the listing. A codespace has no account.
    pub fn use_listed_account(&self, target: &WorkspaceTarget) -> Result<(), String> {
        if target.cloud.kind != CloudBackendKind::DockerSandbox {
            return Ok(());
        }
        let known = self.try_sandbox_account(&target.cloud.name)?;
        if known == target.account {
            return Ok(());
        }
        match (&known, &self.sandbox) {
            (None, Some(deps)) => {
                deps.backend
                    .remember_account(&target.cloud.name, target.account.as_deref());
                Ok(())
            }
            _ => Err(format!(
                "Sandbox {} was listed under a different Docker account than the one this app has for it, so it was left alone. Refresh the list and try again.",
                target.cloud.name
            )),
        }
    }

    /// What deleting `target` would lose and who it belongs to, read now. BLOCKS (a codespace is
    /// asked for its git state; nothing is started or connected to).
    pub fn delete_preview(&self, target: &WorkspaceTarget) -> Result<DeletePreview, String> {
        self.use_listed_account(target)?;
        let cloud = &target.cloud;
        let sandbox = cloud.kind == CloudBackendKind::DockerSandbox;
        let ours = self.known_to_this_app(cloud);
        let mut notes = Vec::new();
        if !ours {
            notes.push(NOTE_NOT_OURS.to_owned());
        }
        if sandbox {
            notes.push(NOTE_SANDBOX_CONTENTS.to_owned());
        }
        Ok(DeletePreview {
            lines: self.loss_lines(cloud)?,
            notes,
            blockers: self.delete_blockers(cloud),
            ours,
            contents_unknown: sandbox,
        })
    }

    /// What a delete of `cloud` would lose that can be read: the list recorded when its sessions
    /// were saved, and for a codespace what GitHub says about its git state now. `Err` when that
    /// cannot be read: a delete does not go on without knowing.
    pub fn loss_lines(&self, cloud: &CloudRef) -> Result<Vec<String>, String> {
        let mut lines = self.unsaved_for_cloud(cloud);
        if cloud.kind == CloudBackendKind::Codespaces {
            match codespaces::git_status(&*self.gh, &cloud.name)? {
                Some(git) => {
                    if git.has_uncommitted_changes {
                        lines.push(LINE_UNCOMMITTED.to_owned());
                    }
                    if git.has_unpushed_changes {
                        lines.push(LINE_UNPUSHED.to_owned());
                    }
                }
                None => lines.push(LINE_GIT_UNKNOWN.to_owned()),
            }
        }
        Ok(lines)
    }

    /// Why `cloud` must not be deleted whatever the user says: local sessions that were moved
    /// into it (deleting it loses the turns that were not synced back, and `return_cleanup`
    /// refuses the same), or a stored location that cannot be read. Empty when nothing is in
    /// the way.
    pub fn delete_blockers(&self, cloud: &CloudRef) -> Vec<String> {
        let (moved, unreadable) = self.moved_sessions(cloud);
        let mut reasons = Vec::new();
        if !moved.is_empty() {
            let titles: Vec<String> = moved
                .iter()
                .map(|s| {
                    let title = s.title.trim();
                    format!(
                        "\u{201c}{}\u{201d}",
                        if title.is_empty() { "Untitled" } else { title }
                    )
                })
                .collect();
            reasons.push(format!(
                "{} {} moved into it and written there. Bring {} back to this computer first (Reset to local), or the turns that were not synced back are lost.",
                titles.join(", "),
                if titles.len() == 1 { "is" } else { "are" },
                if titles.len() == 1 { "it" } else { "them" },
            ));
        }
        if unreadable {
            reasons.push(
                "A session's cloud location cannot be read, so Monoptah cannot tell whether it lives in this workspace."
                    .to_owned(),
            );
        }
        reasons
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
    ///
    /// `guard` runs right before every delete call, after the wait: what was true when the user
    /// confirmed (nothing busy, nothing new to lose, the sandbox still stopped) may not be true
    /// minutes later. An `Err` from it ends the delete with that error and nothing is removed.
    pub fn delete_settled(
        &self,
        cloud: &CloudRef,
        guard: &dyn Fn() -> Result<(), String>,
    ) -> Result<DeleteOutcome, String> {
        self.settle_for_delete(cloud)?;
        let mut retries = 0;
        loop {
            guard()?;
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

/// A sandbox this app makes is named `monoptah-<branch>-<id>` (`sandboxes::NAME_PREFIX`); others
/// are not listed at all. The name says only that SOME Monoptah made it: [`WorkspaceRow::ours`] is
/// what says this store did.
fn is_ours_by_name(name: &str) -> bool {
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
