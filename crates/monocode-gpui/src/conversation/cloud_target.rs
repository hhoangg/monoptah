//! "RUN ON CLOUD" in the new-session composer: a manual way to put a fresh session in a
//! Codespace or a Docker Sandbox. Choosing a target in the Workspace menu does not create
//! anything. Sending the first prompt (Enter or the Send button) asks the shell to (see
//! `shell/cloud_create.rs`), which makes the resource and the session at once, queues the composer
//! text as its first prompt and follows the start. With an empty prompt Send stays disabled.
//!
//! The project's Cloud settings are only the defaults. The panel (see `cloud_panel.rs`) has the
//! same rows for both backends and lets the user override them for this session only: size or
//! machine, runtime limit or idle stop, base branch, the working branch, the Docker account and
//! the Claude account. A row that differs from the project default is marked. The overrides are
//! applied to a copy of the settings when the prompt is sent and nothing is saved back.
use super::cloud_branches::BranchList;
use super::*;
use crate::cloud_presets::AccountChoice;
use monocode_core::cloud_sessions::CloudBackendKind;
use monocode_core::cloud_sessions::docker_config::{self, DockerProjectConfig};
use monocode_core::cloud_sessions::kit::Workspace;
use monocode_core::cloud_sessions::project_cloud::{self, CodespaceProjectConfig, DefaultBackend};
use monocode_core::codespaces::{self, MachineType};
use monocode_core::docker_accounts::{DockerAccountSummary, DockerAccounts};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CloudTarget {
    Codespace,
    DockerSandbox,
}
impl CloudTarget {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Codespace => "Codespace",
            Self::DockerSandbox => "Docker Sandbox",
        }
    }
    /// The resource this target creates, as used in button copy.
    pub(crate) fn noun(self) -> &'static str {
        match self {
            Self::Codespace => "codespace",
            Self::DockerSandbox => "Docker sandbox",
        }
    }
    /// The backend as the project's Cloud settings name it.
    pub(crate) fn backend(self) -> DefaultBackend {
        match self {
            Self::Codespace => DefaultBackend::Codespace,
            Self::DockerSandbox => DefaultBackend::Sandbox,
        }
    }
    /// The muted billing note under the panel title.
    pub(super) fn billing_note(self) -> &'static str {
        match self {
            Self::Codespace => "Billed to your GitHub account",
            Self::DockerSandbox => "Paid compute in your Docker account",
        }
    }
}

/// Machine types offered for a new codespace: the ones GitHub allows for the repository on the
/// branch (a devcontainer's `hostRequirements` hides the small ones), loaded off the UI thread
/// when the panel opens and when another branch is picked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Machines {
    Loading,
    Loaded(Vec<MachineType>),
    /// The listing failed or is not offered: the static default machine stays.
    Failed,
}

/// Lists the machine types a local project's repository allows on a branch (`None`: the
/// default branch). Blocking: runs on a worker.
pub(crate) type MachineLoader =
    Arc<dyn Fn(String, Option<String>) -> Result<Vec<MachineType>, String> + Send + Sync>;

/// Lists the saved Docker accounts (no token). Blocking: runs on a worker.
pub(crate) type DockerAccountsLoader =
    Arc<dyn Fn(&Database) -> Result<Vec<DockerAccountSummary>, String> + Send + Sync>;

/// The real loader: the credentials database of the store's data directory.
pub(crate) fn system_docker_accounts() -> DockerAccountsLoader {
    Arc::new(|store| {
        let accounts = DockerAccounts::for_store(&*locked(store)?)?;
        accounts.list()
    })
}

/// A setting of the cloud panel that opens a menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CloudField {
    Size,
    Runtime,
    Idle,
    Machine,
    DockerAccount,
    Claude,
}
impl CloudField {
    pub(super) fn index(self) -> usize {
        self as usize
    }
}

/// A row of the panel, in display order. Both backends show the same shape: place and size,
/// lifetime, base and working branch, accounts, Claude setup.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CloudRow {
    Machine,
    Size,
    Idle,
    Runtime,
    Base,
    Branch,
    DockerAccount,
    Claude,
    Kit,
}
impl CloudRow {
    /// The rows of `target`, top to bottom.
    pub(crate) fn of(target: CloudTarget) -> &'static [CloudRow] {
        match target {
            CloudTarget::Codespace => &[
                Self::Machine,
                Self::Idle,
                Self::Base,
                Self::Branch,
                Self::Claude,
                Self::Kit,
            ],
            CloudTarget::DockerSandbox => &[
                Self::Size,
                Self::Runtime,
                Self::Base,
                Self::Branch,
                Self::DockerAccount,
                Self::Claude,
                Self::Kit,
            ],
        }
    }
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Machine => "Machine",
            Self::Size => "Size",
            Self::Idle => "Stop when idle",
            Self::Runtime => "Runtime limit",
            Self::Base => "Base branch",
            Self::Branch => "Branch",
            Self::DockerAccount => "Docker account",
            Self::Claude => "Claude account",
            Self::Kit => "Claude setup",
        }
    }
    pub(super) fn id(self) -> &'static str {
        match self {
            Self::Machine => "cloud-row-machine",
            Self::Size => "cloud-row-size",
            Self::Idle => "cloud-row-idle",
            Self::Runtime => "cloud-row-runtime",
            Self::Base => "cloud-row-base",
            Self::Branch => "cloud-row-branch",
            Self::DockerAccount => "cloud-row-docker-account",
            Self::Claude => "cloud-row-claude",
            Self::Kit => "cloud-row-kit",
        }
    }
    pub(super) fn icon(self) -> &'static str {
        match self {
            Self::Machine => "terminal",
            Self::Size => "rocket",
            Self::Idle | Self::Runtime => "clock",
            Self::Base | Self::Branch => "git-branch",
            Self::DockerAccount | Self::Claude => "account-user",
            Self::Kit => "sparkles",
        }
    }
    /// The menu behind the row, when it has one.
    pub(super) fn field(self) -> Option<CloudField> {
        Some(match self {
            Self::Machine => CloudField::Machine,
            Self::Size => CloudField::Size,
            Self::Idle => CloudField::Idle,
            Self::Runtime => CloudField::Runtime,
            Self::DockerAccount => CloudField::DockerAccount,
            Self::Claude => CloudField::Claude,
            Self::Base | Self::Branch | Self::Kit => return None,
        })
    }
}

/// What sending the first prompt asks the shell for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CloudCreate {
    pub target: CloudTarget,
    /// The local project the session twins.
    pub project: String,
    /// Docker: the new feature branch. Codespace: the base branch the codespace is created on.
    pub branch: String,
    /// Docker only: the project config with this session's overrides applied, validated.
    pub config: Option<DockerProjectConfig>,
    /// Codespace only: the machine type picked for this session; `None` keeps the project's.
    pub machine: Option<String>,
    /// Codespace only: minutes before the idle codespace stops, only when changed for this
    /// session; `None` keeps the project's.
    pub idle_timeout_minutes: Option<u32>,
    /// Codespace only: the working branch, created in the codespace with `git switch -c` when
    /// it differs from `branch` (the base the codespace is created on).
    pub work_branch: Option<String>,
    /// The app Claude account of this session when `account_pinned` (a Docker Sandbox carries
    /// it in `config` as well). Ignored otherwise: the instance follows the project.
    pub account: Option<String>,
    /// `account` was changed for this session and is kept by the instance across reconnects.
    pub account_pinned: bool,
    /// The composer text, queued as the first prompt.
    pub prompt: Option<String>,
    /// Copy the global Claude setup to the new workspace. `false` is recorded on the start
    /// (`StartRecord::kit_off`) before it runs, so the "Claude setup" step sends nothing.
    pub kit: bool,
    /// Which press asked; the answer only applies to the draft that is still waiting for it.
    pub request_id: u64,
}

/// Emitted when the first prompt is sent with a cloud target chosen.
pub(crate) struct CreateCloudSession(pub CloudCreate);
/// Emitted when the Docker Sandbox row is picked and the project has no saved config.
pub(crate) struct OpenDockerSetup(pub String);
/// Asks the shell to open the project's Cloud settings, on `backend` or else the project's
/// default backend.
pub(crate) struct OpenProjectCloudSettings {
    pub cwd: String,
    pub backend: Option<DefaultBackend>,
}
impl EventEmitter<OpenProjectCloudSettings> for Conversation {}
/// Asks the shell to open the picker of the existing instances of `backend` of the project
/// `cwd` (the project of this conversation, which is not always the one open in the shell).
pub(crate) struct OpenExistingInstance {
    pub cwd: String,
    pub backend: DefaultBackend,
}
impl EventEmitter<OpenExistingInstance> for Conversation {}
/// Emitted by the "Claude setup" row to open Settings > Cloud.
pub(crate) struct OpenCloudSettings;
impl EventEmitter<OpenCloudSettings> for Conversation {}
impl EventEmitter<CreateCloudSession> for Conversation {}
impl EventEmitter<OpenDockerSetup> for Conversation {}

/// The project's values the panel starts from, and what a changed row is compared with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct CloudDefaults {
    /// Docker: the size and the runtime limit.
    pub cpus: u32,
    pub memory: String,
    pub ttl: String,
    /// Codespace: the machine (`None` is the default machine) and the idle stop.
    pub machine: Option<String>,
    pub idle: u32,
    /// The base branch text: Docker's `base_ref`; a codespace's base branch, else HEAD.
    pub base: String,
    /// The app Claude account of the project's backend.
    pub claude: Option<String>,
    pub docker_account: Option<String>,
}

/// What the Workspace menu shows about the project, see [`Conversation::cloud_menu_facts`].
#[derive(Clone, Debug)]
pub(super) struct CloudMenuFacts {
    default_backend: DefaultBackend,
    /// The codespace / sandbox of the project's most recently updated live cloud session.
    recent_codespace: Option<String>,
    recent_sandbox: Option<String>,
}
impl Default for CloudMenuFacts {
    fn default() -> Self {
        Self {
            default_backend: DefaultBackend::Codespace,
            recent_codespace: None,
            recent_sandbox: None,
        }
    }
}

pub(super) struct CloudDraft {
    pub target: CloudTarget,
    /// Docker: the new feature branch. Codespace: the working branch, which follows the base
    /// branch until the user types another name.
    pub branch: Entity<input::Input>,
    /// The base branch, editable. Starts as the project's.
    pub base: Entity<input::Input>,
    /// What `base` holds, mirrored so labels need no entity read.
    pub base_text: String,
    /// What `branch` holds, mirrored so labels need no entity read.
    pub branch_text: String,
    /// Codespace: the working branch is still the base branch (typing another name ends it).
    pub branch_follows_base: bool,
    /// The project's saved Docker config: only the default of the overrides below (Docker).
    pub config: Option<DockerProjectConfig>,
    /// The project's values, see [`CloudDefaults`].
    pub defaults: CloudDefaults,
    /// Overrides for this session, seeded from `defaults`.
    pub cpus: u32,
    pub memory: String,
    pub ttl: String,
    pub idle: u32,
    pub account: Option<String>,
    pub docker_account: Option<String>,
    /// Accounts with a setup-token or an API key, read when the panel opened.
    pub accounts: Vec<AccountChoice>,
    /// The saved Docker accounts, `None` while they are read (Docker only).
    pub docker_accounts: Option<Vec<DockerAccountSummary>>,
    /// Which listing the answer belongs to.
    pub docker_accounts_request: u64,
    /// Codespace: the machine picked for this session or saved for the project; `None` while
    /// the default stays (the smallest machine the repository allows).
    pub machine: Option<String>,
    pub machines: Machines,
    /// Which listing the answer belongs to.
    pub machines_request: u64,
    /// The base branch the machine listing was asked for, so picking it again asks nothing.
    pub machines_ref: String,
    /// A muted note when the machine (picked, or saved for the project) is not allowed on the
    /// base branch and was replaced.
    pub machine_note: Option<String>,
    /// The repository's branches for the Base branch row.
    pub branches: BranchList,
    /// Which listing the answer belongs to.
    pub branches_request: u64,
    pub collapsed: bool,
    pub creating: bool,
    pub error: Option<String>,
    /// The request in flight (or the last one).
    pub request: u64,
    /// A codespace this draft already created: creating again would bill a second one.
    pub created: Option<String>,
    /// The id of the queue message holding the prompt while the create runs.
    pub queued: Option<String>,
    /// This start copies the global Claude setup (the per-start switch). Starts on only when
    /// the user turned the kit on in Settings.
    pub kit: bool,
    /// What the global selection takes from this project, read off the UI thread.
    pub kit_summary: crate::cloud_kit_ui::Summary,
    pub kit_task: Option<gpui::Task<()>>,
    _events: Vec<Subscription>,
}

impl CloudDraft {
    /// The project starts its sandbox from a saved template, which fixes the size.
    pub(super) fn size_from_template(&self) -> bool {
        self.config
            .as_ref()
            .is_some_and(|config| config.size_from_template())
    }

    /// The row's value differs from the project default (the row is marked for this session).
    pub(super) fn row_changed(&self, row: CloudRow) -> bool {
        let defaults = &self.defaults;
        match row {
            // Compared as what each one starts on: a pick of the machine the project's default
            // resolves to is no change.
            CloudRow::Machine => {
                resolved_machine(&self.machines, self.machine.as_deref())
                    != resolved_machine(&self.machines, defaults.machine.as_deref())
            }
            // A template fixes the size: there is nothing to change for this session.
            CloudRow::Size => {
                !self.size_from_template()
                    && (self.cpus, &self.memory) != (defaults.cpus, &defaults.memory)
            }
            CloudRow::Idle => self.idle != defaults.idle,
            CloudRow::Runtime => self.ttl != defaults.ttl,
            CloudRow::Base => self.base_text.trim() != defaults.base,
            CloudRow::DockerAccount => self.docker_account != defaults.docker_account,
            CloudRow::Claude => self.account != defaults.claude,
            // A codespace's working branch is changed for this session when it names another
            // branch than the base it is created on (an empty field means the base).
            CloudRow::Branch => {
                self.target == CloudTarget::Codespace && {
                    let branch = self.branch_text.trim();
                    !branch.is_empty() && branch != self.base_text.trim()
                }
            }
            // The switch has no project default.
            CloudRow::Kit => false,
        }
    }

    /// How many rows differ from the project defaults.
    pub(super) fn changed_count(&self) -> usize {
        CloudRow::of(self.target)
            .iter()
            .filter(|row| self.row_changed(**row))
            .count()
    }

    /// The project's settings changed under an open panel: every row the user did not change
    /// follows the new defaults, a row they changed keeps its override. Returns whether the
    /// base branch row followed (the caller then updates its field).
    fn follow_defaults(&mut self, new: CloudDefaults) -> bool {
        let changed = |row| self.row_changed(row);
        let (machine, size, idle, runtime) = (
            changed(CloudRow::Machine),
            changed(CloudRow::Size),
            changed(CloudRow::Idle),
            changed(CloudRow::Runtime),
        );
        let (base, docker, claude) = (
            changed(CloudRow::Base),
            changed(CloudRow::DockerAccount),
            changed(CloudRow::Claude),
        );
        if !machine {
            self.machine = new.machine.clone();
        }
        if !size {
            self.cpus = new.cpus;
            self.memory = new.memory.clone();
        }
        if !idle {
            self.idle = new.idle;
        }
        if !runtime {
            self.ttl = new.ttl.clone();
        }
        if !docker {
            self.docker_account = new.docker_account.clone();
        }
        if !claude {
            self.account = new.claude.clone();
        }
        if !base {
            self.base_text = new.base.clone();
        }
        self.defaults = new;
        !base
    }

    /// Starts the overrides over from `defaults`.
    fn use_defaults(&mut self, defaults: CloudDefaults) {
        self.cpus = defaults.cpus;
        self.memory = defaults.memory.clone();
        self.ttl = defaults.ttl.clone();
        self.idle = defaults.idle;
        self.account = defaults.claude.clone();
        self.docker_account = defaults.docker_account.clone();
        self.machine = defaults.machine.clone();
        self.base_text = defaults.base.clone();
        self.defaults = defaults;
    }
}

/// The codespace or sandbox named by a start error that happened after it was created
/// (`created_note_for`), if any.
pub(crate) fn created_codespace(error: &str) -> Option<&str> {
    monocode_core::cloud_sessions::split_created_note(error).map(|(_, _, name)| name)
}

fn short_id() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos() ^ (d.as_secs() as u32));
    format!("{:06x}", nanos & 0xff_ffff)
}

/// The project's values for `target`: the saved Docker config (or the built-in defaults) and
/// the codespace settings, with `base` as the base branch text.
fn defaults_of(
    target: CloudTarget,
    config: Option<&DockerProjectConfig>,
    codespace: &CodespaceProjectConfig,
    codespace_base: String,
) -> CloudDefaults {
    let sandbox = config.cloned().unwrap_or_default();
    let docker = target == CloudTarget::DockerSandbox;
    CloudDefaults {
        cpus: sandbox.cpus,
        memory: sandbox.memory,
        ttl: sandbox.ttl,
        machine: codespace.machine.clone(),
        idle: codespace.idle_timeout_minutes,
        base: if docker {
            sandbox.base_ref.trim().to_owned()
        } else {
            codespace_base
        },
        claude: if docker {
            sandbox.claude_account
        } else {
            codespace.claude_account.clone()
        },
        docker_account: sandbox.docker_account,
    }
}

/// The machine a codespace starts on for `preferred` (a pick or the project's saved machine)
/// given what the listing says: `preferred` while the repository allows it, else the smallest
/// allowed machine; while the listing runs or when it failed, `preferred` or the static default.
pub(super) fn resolved_machine(machines: &Machines, preferred: Option<&str>) -> String {
    match machines {
        Machines::Loaded(list) => codespaces::pick_machine(list, preferred).name,
        _ => preferred.unwrap_or(codespaces::DEFAULT_MACHINE).to_owned(),
    }
}

/// The machine the panel shows for a new codespace: [`resolved_machine`] of the pick. `None`
/// while the listing still runs and nothing is picked.
pub(super) fn effective_machine(machines: &Machines, picked: Option<&str>) -> Option<String> {
    match (machines, picked) {
        (Machines::Loading, None) => None,
        _ => Some(resolved_machine(machines, picked)),
    }
}

/// The label of the machine type `name`, from the loaded list when it knows it.
pub(super) fn machine_label(machines: &Machines, name: &str) -> String {
    match machines {
        Machines::Loaded(list) => crate::cloud_presets::machine_name_label(list, name),
        _ => name.to_owned(),
    }
}

impl Conversation {
    /// The shell tells whether the project is a GitHub repository (Codespace needs one).
    pub(crate) fn set_offers_codespace(&mut self, offers: bool, cx: &mut Context<Self>) {
        if self.cloud_offers_codespace != offers {
            self.cloud_offers_codespace = offers;
            cx.notify();
        }
    }

    /// The shell tells how to list the machine types of this session's repository.
    pub(crate) fn set_machine_loader(&mut self, loader: MachineLoader) {
        self.machine_loader = Some(loader);
    }

    pub(crate) fn cloud_target(&self) -> Option<CloudTarget> {
        self.cloud_draft.as_ref().map(|draft| draft.target)
    }

    /// A create is running: the target and the workspace cannot change under it.
    pub(crate) fn cloud_creating(&self) -> bool {
        self.cloud_draft
            .as_ref()
            .is_some_and(|draft| draft.creating)
    }

    /// A cloud target is chosen: sending creates the resource instead of messaging locally.
    pub(crate) fn cloud_create_pending(&self) -> bool {
        self.cloud_draft.is_some()
    }

    /// The Send button's tooltip while a cloud target is chosen, and whether it is usable.
    pub(super) fn cloud_send_tip(&self, has_value: bool) -> Option<(SharedString, bool)> {
        let draft = self.cloud_draft.as_ref()?;
        let noun = draft.target.noun();
        Some(if draft.creating {
            (format!("Creating {noun}\u{2026}").into(), false)
        } else if let Some(name) = &draft.created {
            (format!("Already created: {name}").into(), false)
        } else if !has_value {
            (
                format!("Type a prompt to create the {noun} and send it").into(),
                false,
            )
        } else {
            (format!("Create {noun} and send").into(), true)
        })
    }

    /// Whether the Workspace menu can offer a cloud target for this session.
    fn cloud_menu_applies(&self) -> bool {
        !self.summary.has_messages && !self.is_remote()
    }

    /// The "RUN ON CLOUD" section of the Workspace menu, empty when nothing applies.
    pub(super) fn cloud_menu_items(&self) -> Vec<MenuItem> {
        if !self.cloud_menu_applies() {
            return Vec::new();
        }
        let default_backend = self.project_default_backend();
        // Each row says who pays (the panel's own note), then whether it is the project's
        // default, or that it must be set up first.
        let described = |target: CloudTarget, extra: Option<&str>| match extra {
            Some(extra) => format!("{} \u{b7} {extra}", target.billing_note()),
            None => target.billing_note().to_owned(),
        };
        let default_note =
            |backend: DefaultBackend| (default_backend == backend).then_some("default");
        let mut rows = Vec::new();
        if self.cloud_offers_codespace {
            let row = MenuAction::new("workspace:cloud-codespace", "Codespace")
                .icon("cloud-upload")
                .checked(self.cloud_target() == Some(CloudTarget::Codespace))
                .description(described(
                    CloudTarget::Codespace,
                    default_note(DefaultBackend::Codespace),
                ));
            rows.push(row.into());
        }
        let configured = self.docker_config().is_some();
        let docker = MenuAction::new("workspace:cloud-docker", "Docker Sandbox")
            .icon("cloud-upload")
            .checked(self.cloud_target() == Some(CloudTarget::DockerSandbox))
            .description(described(
                CloudTarget::DockerSandbox,
                if configured {
                    default_note(DefaultBackend::Sandbox)
                } else {
                    Some("Set up first")
                },
            ));
        rows.push(docker.into());
        // The instances of the backend that is chosen, else the project's default.
        let backend = self
            .cloud_target()
            .map_or(default_backend, CloudTarget::backend);
        let facts = self.cloud_menu_facts();
        let mut existing =
            MenuAction::new("workspace:cloud-existing", "Existing instance\u{2026}").icon("plus");
        match backend {
            DefaultBackend::Codespace if self.cloud_offers_codespace => {
                if let Some(name) = facts.recent_codespace {
                    existing = existing.description(name);
                }
            }
            DefaultBackend::Codespace => existing = existing.disabled(true),
            // A sandbox is reached through the sessions of the project that run on it.
            DefaultBackend::Sandbox => {
                existing = match facts.recent_sandbox {
                    Some(name) => existing.description(name),
                    None => existing.description("No sandbox yet").disabled(true),
                }
            }
        }
        rows.push(existing.into());
        let mut items = vec![MenuItem::Separator, MenuItem::Header("Run on cloud".into())];
        items.extend(rows);
        items.push(MenuItem::Separator);
        items.push(
            MenuAction::new(
                "workspace:cloud-settings",
                "Cloud settings for this project\u{2026}",
            )
            .icon("sliders")
            .into(),
        );
        items
    }

    /// What the Workspace menu shows about the project besides the saved Docker config: read
    /// from the store once and kept until the project's cloud settings or cloud sessions
    /// change ([`Self::forget_project_cloud`]), not on every opening of the menu.
    fn cloud_menu_facts(&self) -> CloudMenuFacts {
        self.cloud_menu_cache
            .borrow_mut()
            .get_or_insert_with(|| {
                let Ok(store) = locked(&self.store) else {
                    return CloudMenuFacts::default();
                };
                let default_backend = project_cloud::load(&store, &self.summary.cwd)
                    .map_or(DefaultBackend::Codespace, |project| {
                        project.effective_default_backend()
                    });
                // Newest first.
                let sessions = store
                    .cloud_sessions(&self.summary.cwd)
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|s| !s.summary.archived)
                    .filter_map(|s| s.project.cloud())
                    .collect::<Vec<_>>();
                let recent = |kind| {
                    sessions
                        .iter()
                        .find(|cloud| cloud.kind == kind)
                        .map(|cloud| cloud.name.clone())
                };
                CloudMenuFacts {
                    default_backend,
                    recent_codespace: recent(CloudBackendKind::Codespaces),
                    recent_sandbox: recent(CloudBackendKind::DockerSandbox),
                }
            })
            .clone()
    }

    /// The project's default backend (Docker Sandbox when only that is set up, else Codespaces).
    fn project_default_backend(&self) -> DefaultBackend {
        self.cloud_menu_facts().default_backend
    }

    /// The saved config, read from the store once per menu opening, not per render.
    pub(super) fn docker_config(&self) -> Option<DockerProjectConfig> {
        self.docker_config_cache
            .borrow_mut()
            .get_or_insert_with(|| {
                locked(&self.store).ok().and_then(|store| {
                    docker_config::load(&store, &self.summary.cwd)
                        .ok()
                        .flatten()
                })
            })
            .clone()
    }

    /// The codespace part of the project's cloud settings (the defaults when none is saved).
    /// Read once per action and passed around.
    fn codespace_project_config(&self) -> CodespaceProjectConfig {
        project_cloud::codespace_config_or_default(&self.store, &self.summary.cwd)
    }

    /// The branch a new codespace starts on: the project's base branch when set, else HEAD of
    /// the project folder (empty while HEAD is detached).
    fn codespace_default_branch(&self, config: &CodespaceProjectConfig) -> String {
        if !config.base_ref.is_empty() {
            return config.base_ref.clone();
        }
        git_head(Path::new(&self.summary.cwd))
            .and_then(|(branch, detached)| (!detached).then_some(branch))
            .unwrap_or_default()
    }

    /// Reads the saved config into the cache, like opening the menu does.
    #[cfg(test)]
    pub(crate) fn cache_docker_config(&self) {
        let _ = self.docker_config();
    }

    #[cfg(test)]
    pub(crate) fn docker_config_is_cached(&self) -> bool {
        self.docker_config_cache.borrow().is_some()
    }

    pub(crate) fn forget_docker_config(&self) {
        self.docker_config_cache.take();
    }

    /// Forgets everything read about the project's cloud setup: the Docker config and what the
    /// Workspace menu shows (default backend, most recent instances).
    pub(crate) fn forget_project_cloud(&self) {
        self.forget_docker_config();
        self.cloud_menu_cache.take();
    }

    /// A cloud session of the project came or went: the menu reads its most recent instance
    /// again the next time it opens.
    pub(crate) fn forget_cloud_menu(&self) {
        self.cloud_menu_cache.take();
    }

    /// Whether the Workspace menu facts are cached (tests).
    #[cfg(test)]
    pub(crate) fn cloud_menu_is_cached(&self) -> bool {
        self.cloud_menu_cache.borrow().is_some()
    }

    /// The project's cloud settings were saved: forget what was read, and let an open panel
    /// follow the new defaults on every row the user did not change.
    pub(crate) fn project_cloud_saved(&mut self, cx: &mut Context<Self>) {
        self.forget_project_cloud();
        self.sync_cloud_defaults(cx);
    }

    /// Reads the project's settings again for the open panel. A row that still holds the old
    /// default takes the new one; a row the user changed keeps its override. Nothing changes
    /// under a running create.
    fn sync_cloud_defaults(&mut self, cx: &mut Context<Self>) {
        let Some(target) = self.cloud_target().filter(|_| !self.cloud_creating()) else {
            return;
        };
        self.forget_project_cloud();
        let config = if target == CloudTarget::DockerSandbox {
            self.docker_config()
        } else {
            None
        };
        let fresh = self.cloud_defaults(target, config.as_ref());
        // The save may have added, replaced or removed accounts: read them again, so a row
        // names its account by label (Docker ID) and not by a raw id.
        let accounts = crate::cloud_settings::claude_accounts(cx);
        self.cloud_request_seq += 1;
        let request = self.cloud_request_seq;
        let docker = target == CloudTarget::DockerSandbox;
        let Some(draft) = &mut self.cloud_draft else {
            return;
        };
        draft.accounts = accounts;
        if docker {
            draft.docker_accounts_request = request;
            self.load_docker_accounts(request, cx);
        }
        let Some(draft) = &mut self.cloud_draft else {
            return;
        };
        if fresh == draft.defaults && config == draft.config {
            cx.notify();
            return;
        }
        draft.config = config;
        if draft.follow_defaults(fresh) {
            let (field, text) = (draft.base.clone(), draft.base_text.clone());
            field.update(cx, |field, cx| field.set_text(text, cx));
        }
        cx.notify();
    }

    /// Handles the cloud rows of the Workspace menu; false for any other id.
    pub(crate) fn cloud_menu_choice(&mut self, id: &str, cx: &mut Context<Self>) -> bool {
        if let Some(setting) = id.strip_prefix("cloud-set:") {
            self.cloud_set(setting, cx);
            return true;
        }
        match id {
            "workspace:cloud-move" => {
                cx.emit(ContinueInCloud);
                true
            }
            "workspace:cloud-settings" => {
                // On the backend that is chosen; else the dialog opens on the project's default.
                cx.emit(OpenProjectCloudSettings {
                    cwd: self.summary.cwd.clone(),
                    backend: self.cloud_target().map(CloudTarget::backend),
                });
                true
            }
            "workspace:cloud-existing" => {
                let backend = self
                    .cloud_target()
                    .map_or_else(|| self.project_default_backend(), CloudTarget::backend);
                let offered = match backend {
                    DefaultBackend::Codespace => self.cloud_offers_codespace,
                    DefaultBackend::Sandbox => self.cloud_menu_facts().recent_sandbox.is_some(),
                };
                if self.cloud_menu_applies() && !self.cloud_creating() && offered {
                    cx.emit(OpenExistingInstance {
                        cwd: self.summary.cwd.clone(),
                        backend,
                    });
                }
                true
            }
            "workspace:cloud-codespace" => {
                if self.cloud_menu_applies()
                    && !self.cloud_creating()
                    && self.cloud_offers_codespace
                {
                    self.choose_cloud_target(CloudTarget::Codespace, None, cx);
                }
                true
            }
            "workspace:cloud-docker" => {
                if self.cloud_menu_applies() && !self.cloud_creating() {
                    self.forget_docker_config();
                    match self.docker_config() {
                        Some(config) => {
                            self.choose_cloud_target(CloudTarget::DockerSandbox, Some(config), cx)
                        }
                        None => cx.emit(OpenDockerSetup(self.summary.cwd.clone())),
                    }
                }
                true
            }
            // Local checkouts mean Local.
            "workspace:current" | "workspace:new" => {
                // Not while a create runs: it would send locally and queue the same prompt.
                if self.cloud_creating() {
                    return true;
                }
                self.clear_cloud_target(cx);
                false
            }
            _ => false,
        }
    }

    pub(super) fn clear_cloud_target(&mut self, cx: &mut Context<Self>) {
        if self.cloud_creating() {
            return;
        }
        self.close_cloud_picker();
        if self.cloud_draft.take().is_some() {
            cx.notify();
        }
    }

    /// The project's defaults for `target`, read once per action.
    fn cloud_defaults(
        &self,
        target: CloudTarget,
        config: Option<&DockerProjectConfig>,
    ) -> CloudDefaults {
        let codespace = self.codespace_project_config();
        let base = self.codespace_default_branch(&codespace);
        defaults_of(target, config, &codespace, base)
    }

    fn choose_cloud_target(
        &mut self,
        target: CloudTarget,
        config: Option<DockerProjectConfig>,
        cx: &mut Context<Self>,
    ) {
        let docker = target == CloudTarget::DockerSandbox;
        if let Some(draft) = &self.cloud_draft
            && draft.target == target
            && !draft.creating
        {
            // Picked again (after Cloud settings, say): the project's settings are the
            // defaults again, so this session's overrides start over from them.
            let defaults = self.cloud_defaults(target, config.as_ref());
            let base = defaults.base.clone();
            let accounts = crate::cloud_settings::claude_accounts(cx);
            let Some(draft) = &mut self.cloud_draft else {
                return;
            };
            draft.config = config;
            draft.use_defaults(defaults);
            draft.accounts = accounts;
            let field = draft.base.clone();
            field.update(cx, |field, cx| field.set_text(base.clone(), cx));
            if !docker {
                // The same reset for a codespace: the working branch is the base again and
                // the machines are listed afresh.
                draft.machine_note = None;
                draft.machines = Machines::Loading;
                draft.branch_follows_base = true;
                let branch = draft.branch.clone();
                branch.update(cx, |field, cx| field.set_text(base, cx));
                self.cloud_request_seq += 1;
                let request = self.cloud_request_seq;
                if let Some(draft) = &mut self.cloud_draft {
                    draft.machines_request = request;
                }
                self.load_cloud_machines(request, cx);
            } else {
                self.cloud_request_seq += 1;
                let request = self.cloud_request_seq;
                if let Some(draft) = &mut self.cloud_draft {
                    draft.docker_accounts_request = request;
                }
                self.load_docker_accounts(request, cx);
            }
            // The repository's branches again, as a fresh listing.
            self.cloud_request_seq += 1;
            let branches_request = self.cloud_request_seq;
            if let Some(draft) = &mut self.cloud_draft {
                draft.branches = BranchList::Loading;
                draft.branches_request = branches_request;
            }
            self.load_cloud_branches(branches_request, cx);
            cx.notify();
            return;
        }
        if self
            .cloud_draft
            .as_ref()
            .is_some_and(|draft| draft.creating)
        {
            return;
        }
        let defaults = self.cloud_defaults(target, config.as_ref());
        // Docker names a new feature branch; a codespace works on its base branch (HEAD of the
        // project folder, as the header picker does, empty while HEAD is detached, unless the
        // project sets a base branch) until another name is typed.
        let branch_text = if docker {
            format!("monoptah/{}", short_id())
        } else {
            defaults.base.clone()
        };
        let branch_value = branch_text.clone();
        let branch = cx.new(|cx| {
            let mut field = input::Input::field(
                "Branch",
                if docker { "monoptah/feature" } else { "main" },
                content(0.35),
                cx,
            );
            field.set_font_size(12.);
            field.set_text(branch_value, cx);
            field
        });
        let base_text = defaults.base.clone();
        let base = cx.new(|cx| {
            let mut field = input::Input::field("Base branch", "default branch", content(0.35), cx);
            field.set_font_size(12.);
            field.set_text(base_text.clone(), cx);
            field
        });
        let events = vec![
            cx.subscribe(&branch, |this, branch, _: &input::Changed, cx| {
                let text = branch.read(cx).content.to_string();
                if let Some(draft) = &mut this.cloud_draft {
                    draft.branch_follows_base = text.trim() == draft.base_text.trim();
                    draft.branch_text = text;
                }
                cx.notify();
            }),
            // Settings changed while the panel is open: count again with the new selection
            // (this also picks up the flag being turned on).
            cx.observe_global::<crate::settings::Preferences>(|this, cx| this.load_kit_summary(cx)),
            cx.subscribe(&base, |this, base, _: &input::Changed, cx| {
                let text = base.read(cx).content.to_string();
                let mut follow = None;
                if let Some(draft) = &mut this.cloud_draft {
                    draft.base_text = text.clone();
                    // A codespace works on its base branch until another name is typed.
                    if draft.target == CloudTarget::Codespace && draft.branch_follows_base {
                        follow = Some(draft.branch.clone());
                    }
                }
                if let Some(branch) = follow {
                    branch.update(cx, |field, cx| field.set_text(text, cx));
                }
                cx.notify();
            }),
        ];
        // The cloud session starts from the project's own checkout, not a worktree.
        self.settings.worktree_cwd = None;
        self.settings.new_worktree = false;
        self.settings.worktree_removed = false;
        self.save_settings(cx);
        self.refresh_git(cx);
        let accounts = crate::cloud_settings::claude_accounts(cx);
        let kit_prefs = crate::cloud_kit_ui::kit_prefs(cx);
        self.cloud_request_seq += 1;
        let machines_request = self.cloud_request_seq;
        self.cloud_request_seq += 1;
        let branches_request = self.cloud_request_seq;
        self.cloud_request_seq += 1;
        let docker_accounts_request = self.cloud_request_seq;
        self.cloud_draft = Some(CloudDraft {
            target,
            branch,
            base,
            base_text: defaults.base.clone(),
            branch_text,
            branch_follows_base: !docker,
            config,
            cpus: defaults.cpus,
            memory: defaults.memory.clone(),
            ttl: defaults.ttl.clone(),
            idle: defaults.idle,
            account: defaults.claude.clone(),
            docker_account: defaults.docker_account.clone(),
            accounts,
            docker_accounts: if docker { None } else { Some(Vec::new()) },
            docker_accounts_request,
            machine: (!docker).then(|| defaults.machine.clone()).flatten(),
            defaults,
            machines: if docker {
                Machines::Failed
            } else {
                Machines::Loading
            },
            machines_request,
            machines_ref: String::new(),
            machine_note: None,
            branches: BranchList::Loading,
            branches_request,
            collapsed: false,
            creating: false,
            error: None,
            request: 0,
            created: None,
            queued: None,
            kit: crate::cloud_kit_ui::default_on(&kit_prefs),
            kit_summary: crate::cloud_kit_ui::Summary::Loading,
            kit_task: None,
            _events: events,
        });
        self.load_kit_summary(cx);
        if docker {
            self.load_docker_accounts(docker_accounts_request, cx);
        } else {
            self.load_cloud_machines(machines_request, cx);
            // The usage line under the machine row: asked once per opening (a cached answer
            // when fresh), never on render.
            self.load_codespace_usage(false, cx);
        }
        self.load_cloud_branches(branches_request, cx);
        cx.notify();
    }

    /// Reads the saved Docker accounts on a worker; the answer only applies to the draft that
    /// asked. A failed read leaves the machine sign-in as the only choice.
    fn load_docker_accounts(&mut self, request: u64, cx: &mut Context<Self>) {
        let loader = self.docker_accounts_loader.clone();
        let store = self.store.clone();
        let work = cx
            .background_executor()
            .spawn(async move { loader(&store) });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |this, cx| {
                if let Some(draft) = this
                    .cloud_draft
                    .as_mut()
                    .filter(|draft| draft.docker_accounts_request == request)
                {
                    draft.docker_accounts = Some(result.unwrap_or_default());
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Replaces how the saved Docker accounts are listed (tests hand in fakes).
    #[cfg(test)]
    pub(crate) fn set_docker_accounts_loader(&mut self, loader: DockerAccountsLoader) {
        self.docker_accounts_loader = loader;
    }

    /// Counts the Claude setup this project would take, on a worker.
    fn load_kit_summary(&mut self, cx: &mut Context<Self>) {
        if self.cloud_draft.is_none() {
            return;
        }
        let workspace = match self.cloud_draft.as_ref().map(|draft| draft.target) {
            Some(CloudTarget::DockerSandbox) => Workspace::Sandbox,
            _ => Workspace::Codespace,
        };
        let task = crate::cloud_kit_ui::load_summary(
            self.summary.cwd.clone(),
            workspace,
            crate::cloud_kit_ui::kit_prefs(cx),
            cx,
        );
        let task = cx.spawn(async move |this, cx| {
            let summary = task.await;
            let _ = this.update(cx, |this, cx| {
                if let Some(draft) = this.cloud_draft.as_mut() {
                    draft.kit_summary = summary;
                    cx.notify();
                }
            });
        });
        if let Some(draft) = &mut self.cloud_draft {
            draft.kit_task = Some(task);
        }
    }

    /// The per-start switch of the Claude setup row.
    pub(super) fn toggle_cloud_kit(&mut self, cx: &mut Context<Self>) {
        let allowed = crate::cloud_kit_ui::default_on(&crate::cloud_kit_ui::kit_prefs(cx));
        if let Some(draft) = self.cloud_draft.as_mut().filter(|d| !d.creating && allowed) {
            draft.kit = !draft.kit;
            cx.notify();
        }
    }

    /// Lists the machine types the repository allows on the draft's branch, on a worker; the
    /// answer only applies to the draft that asked. The static default machine stays when there
    /// is no loader or the listing fails (offline, no `codespace` scope): nothing blocks Send,
    /// which leaves the choice to the start when nothing is picked. A pick the new listing does
    /// not allow falls back to the smallest allowed machine, with a note.
    fn load_cloud_machines(&mut self, request: u64, cx: &mut Context<Self>) {
        let Some(loader) = self.machine_loader.clone() else {
            if let Some(draft) = &mut self.cloud_draft {
                draft.machines = Machines::Failed;
            }
            return;
        };
        let Some(draft) = self.cloud_draft.as_mut() else {
            return;
        };
        // The codespace is created on the base branch (typed, picked or the project's); with
        // none, on HEAD of the project folder, as the start does.
        let base = draft.base.read(cx).content.trim().to_owned();
        draft.machines_ref = base.clone();
        let project = self.summary.cwd.clone();
        let work = cx.background_executor().spawn(async move {
            let branch = if base.is_empty() {
                git_head(Path::new(&project))
                    .and_then(|(branch, detached)| (!detached).then_some(branch))
            } else {
                Some(base)
            };
            loader(project, branch)
        });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |this, cx| {
                if let Some(draft) = this
                    .cloud_draft
                    .as_mut()
                    .filter(|draft| draft.machines_request == request)
                {
                    draft.machines = match result {
                        Ok(machines) if !machines.is_empty() => Machines::Loaded(machines),
                        _ => Machines::Failed,
                    };
                    // A machine the repository does not allow on this branch, whether the
                    // project saved it or the user picked it on another branch, falls back to
                    // the smallest allowed one, with a note.
                    if let Machines::Loaded(list) = &draft.machines
                        && let Some(machine) = draft.machine.clone()
                    {
                        let pick = codespaces::pick_machine(list, Some(&machine));
                        if let Some(replaced) = pick.replaced {
                            draft.machine = None;
                            draft.machine_note = Some(format!(
                                "{replaced} is not available for this repository; using {}",
                                machine_label(&draft.machines, &pick.name)
                            ));
                        }
                    }
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// The base branch of the codespace changed through the picker: list the machines that
    /// branch allows, unless they were listed for it already. Typing in the field asks
    /// nothing; the start itself checks the branch it creates on.
    pub(super) fn reload_cloud_machines_for_base(&mut self, cx: &mut Context<Self>) {
        let Some(draft) = self
            .cloud_draft
            .as_ref()
            .filter(|d| d.target == CloudTarget::Codespace && !d.creating)
        else {
            return;
        };
        if draft.base.read(cx).content.trim() == draft.machines_ref {
            return;
        }
        self.cloud_request_seq += 1;
        let request = self.cloud_request_seq;
        if let Some(draft) = &mut self.cloud_draft {
            draft.machines = Machines::Loading;
            draft.machines_request = request;
            draft.machine_note = None;
        }
        self.load_cloud_machines(request, cx);
    }

    /// Applies one choice of a panel menu (`size:2:4g`, `runtime:1h`, `idle:60`, `machine:<name>`,
    /// `docker:<id>`, `claude:<id>`) to the draft. An empty id of `docker` or `claude` is "no
    /// account". Nothing is saved: the choice lives for this session only.
    fn cloud_set(&mut self, setting: &str, cx: &mut Context<Self>) {
        let Some(draft) = self.cloud_draft.as_mut().filter(|draft| !draft.creating) else {
            return;
        };
        let (field, value) = setting.split_once(':').unwrap_or((setting, ""));
        let some = |value: &str| (!value.is_empty()).then(|| value.to_owned());
        match field {
            "size" => {
                if let Some((cpus, memory)) = value.split_once(':')
                    && let Ok(cpus) = cpus.parse()
                {
                    draft.cpus = cpus;
                    draft.memory = memory.to_owned();
                }
            }
            "runtime" => draft.ttl = value.to_owned(),
            "idle" => {
                if let Ok(minutes) = value.parse() {
                    draft.idle = minutes;
                }
            }
            "claude" => draft.account = some(value),
            "docker" => draft.docker_account = some(value),
            "machine" => {
                draft.machine = some(value);
                draft.machine_note = None;
            }
            _ => return,
        }
        cx.notify();
    }

    /// The base branch the cloud session starts from.
    fn cloud_base(&self) -> String {
        match self.cloud_draft.as_ref() {
            Some(draft) => {
                let base = draft.base_text.trim();
                if base.is_empty() {
                    "default branch".into()
                } else {
                    base.to_owned()
                }
            }
            None => self
                .branch
                .clone()
                .unwrap_or_else(|| "default branch".into()),
        }
    }

    /// The workspace trigger's text while a cloud target is chosen.
    pub(super) fn cloud_workspace_label(&self) -> Option<String> {
        self.cloud_target()
            .map(|target| format!("\u{2601} {}", target.label()))
    }

    /// The branch trigger's text while a cloud target is chosen.
    pub(super) fn cloud_base_label(&self) -> Option<String> {
        self.cloud_draft
            .is_some()
            .then(|| format!("base {}", self.cloud_base()))
    }

    /// Sending with a cloud target chosen: asks the shell to create the resource and the session.
    /// One request per press: a running or finished create ignores further presses.
    pub(crate) fn create_cloud(&mut self, cx: &mut Context<Self>) {
        let Some(draft) = &self.cloud_draft else {
            return;
        };
        if draft.creating || draft.created.is_some() {
            return;
        }
        let target = draft.target;
        let fail = |this: &mut Self, error: &str, cx: &mut Context<Self>| {
            if let Some(draft) = &mut this.cloud_draft {
                draft.error = Some(error.to_owned());
            }
            cx.notify();
        };
        if !self.attachments.is_empty() {
            fail(
                self,
                "Attachments cannot be queued while the session starts. Remove them first.",
                cx,
            );
            return;
        }
        // What the fields hold now (their change events may not have run yet), then the
        // project's settings, which may have been saved since the panel opened: rows the user
        // did not touch follow them, so only a real override is sent.
        if let Some(draft) = &mut self.cloud_draft {
            draft.base_text = draft.base.read(cx).content.to_string();
            draft.branch_text = draft.branch.read(cx).content.to_string();
        }
        self.sync_cloud_defaults(cx);
        let Some(draft) = &self.cloud_draft else {
            return;
        };
        let typed = draft.branch.read(cx).content.trim().to_owned();
        let typed_base = draft.base.read(cx).content.trim().to_owned();
        let codespace_base = self.codespace_project_config().base_ref;
        let mut default_account: Option<String> = None;
        let mut work_branch = None;
        let (branch, config) = match target {
            // The base branch is the one typed or picked when the user changed it. Else the
            // project's base branch, else HEAD of the project folder (as the header picker
            // does; a worktree's branch would not exist on the remote).
            CloudTarget::Codespace => {
                let base = if draft.row_changed(CloudRow::Base) && !typed_base.is_empty() {
                    typed_base
                } else if !codespace_base.is_empty() {
                    codespace_base
                } else {
                    match git_head(Path::new(&self.summary.cwd)) {
                        Some((_, true)) => {
                            fail(
                                self,
                                "HEAD is detached. Check out a branch to start a codespace on it.",
                                cx,
                            );
                            return;
                        }
                        Some((branch, false)) => branch,
                        None => String::new(),
                    }
                };
                // The working branch is created in the codespace when it is another name.
                if !typed.is_empty() && typed != base {
                    if !docker_config::valid_ref(&typed) {
                        fail(self, "The branch name contains invalid characters.", cx);
                        return;
                    }
                    work_branch = Some(typed);
                }
                (base, None)
            }
            CloudTarget::DockerSandbox => {
                if typed.is_empty() {
                    fail(self, "Enter a branch name for the new session.", cx);
                    return;
                }
                match self.docker_config() {
                    Some(project) => {
                        // The project's config is the default; this copy carries only the rows
                        // changed for this session and is checked by the same validation a
                        // saved one gets.
                        let mut config = project;
                        default_account = config.claude_account.clone();
                        // Never for a template: `row_changed` is false then, and the saved
                        // size of such a project is not a size anyone checked.
                        if draft.row_changed(CloudRow::Size) {
                            config.cpus = draft.cpus;
                            config.memory = draft.memory.clone();
                        }
                        if draft.row_changed(CloudRow::Runtime) {
                            config.ttl = draft.ttl.clone();
                        }
                        if draft.row_changed(CloudRow::Claude) {
                            config.claude_account = draft.account.clone();
                        }
                        if draft.row_changed(CloudRow::DockerAccount) {
                            config.docker_account = draft.docker_account.clone();
                        }
                        if draft.row_changed(CloudRow::Base) {
                            config.base_ref = typed_base;
                        }
                        match config.validated() {
                            Ok(config) => (typed, Some(config)),
                            Err(error) => {
                                fail(self, &error, cx);
                                return;
                            }
                        }
                    }
                    None => {
                        fail(
                            self,
                            "Set up Docker Sandbox in the project's Cloud settings first.",
                            cx,
                        );
                        return;
                    }
                }
            }
        };
        if branch.is_empty() {
            fail(self, "There is no branch to start the codespace on.", cx);
            return;
        }
        // Only a row changed for this session is an override; the rest follows the project.
        let changed = |row| {
            self.cloud_draft
                .as_ref()
                .is_some_and(|d| d.row_changed(row))
        };
        let (machine, idle_timeout_minutes) = match target {
            CloudTarget::Codespace => (
                // The pick, only when it differs from the project's. With none the start takes
                // the project's machine (or the smallest allowed one) for the branch it
                // creates on.
                self.cloud_draft
                    .as_ref()
                    .filter(|_| changed(CloudRow::Machine))
                    .and_then(|d| d.machine.clone()),
                self.cloud_draft
                    .as_ref()
                    .filter(|_| changed(CloudRow::Idle))
                    .map(|d| d.idle),
            ),
            CloudTarget::DockerSandbox => (None, None),
        };
        // A Claude account that differs from the project's is this session's own: the
        // instance keeps it across reconnects.
        let (account, account_pinned) = match (&config, target) {
            (Some(config), CloudTarget::DockerSandbox) => (
                config.claude_account.clone(),
                config.claude_account != default_account,
            ),
            (None, CloudTarget::Codespace) if changed(CloudRow::Claude) => (
                self.cloud_draft.as_ref().and_then(|d| d.account.clone()),
                true,
            ),
            _ => (None, false),
        };
        let prompt = self.input.read(cx).content.trim().to_owned();
        // The prompt leaves the composer for the send queue at once. The queue copy is not
        // saved: this draft is a local session, the new cloud session gets its own stored copy.
        let queued = (!prompt.is_empty())
            .then(|| self.queue_composer_prompt(false, cx))
            .flatten();
        self.cloud_request_seq += 1;
        let request_id = self.cloud_request_seq;
        let request = CloudCreate {
            request_id,
            target,
            project: self.summary.cwd.clone(),
            branch,
            config,
            machine,
            idle_timeout_minutes,
            work_branch,
            account,
            account_pinned,
            prompt: (!prompt.is_empty()).then_some(prompt),
            // The switch is this start's own choice, but it can only narrow Settings: if the
            // kit was turned off while the panel was open, nothing is sent.
            kit: self.cloud_draft.as_ref().is_some_and(|draft| draft.kit)
                && crate::cloud_kit_ui::default_on(&crate::cloud_kit_ui::kit_prefs(cx)),
        };
        if let Some(draft) = &mut self.cloud_draft {
            draft.creating = true;
            draft.queued = queued;
            draft.request = request_id;
            draft.error = None;
            let fields = [draft.branch.clone(), draft.base.clone()];
            for field in fields {
                field.update(cx, |field, _| field.set_read_only(true));
            }
        }
        cx.emit(CreateCloudSession(request));
        cx.notify();
    }

    /// The shell finished (or refused) the creation requested by [`Self::create_cloud`]. `Ok`
    /// says whether the prompt moved into the new session's queue; if not, or on an error, the
    /// prompt returns to the composer.
    pub(crate) fn cloud_create_finished(
        &mut self,
        request_id: u64,
        result: Result<bool, String>,
        cx: &mut Context<Self>,
    ) {
        // A stale answer (the draft was replaced) must not touch the composer.
        if !self
            .cloud_draft
            .as_ref()
            .is_some_and(|draft| draft.creating && draft.request == request_id)
        {
            return;
        }
        // The queued copy is gone either way: on success the new session holds its own, and
        // otherwise the prompt goes back to the composer so it is never lost.
        let keep = matches!(result, Ok(true));
        if let Some(id) = self
            .cloud_draft
            .as_mut()
            .and_then(|draft| draft.queued.take())
        {
            self.unqueue_composer_prompt(&id, !keep, cx);
        }
        match result {
            Ok(_) => {
                // The resource exists: never create a second one for it.
                self.cloud_draft = None;
            }
            Err(error) => {
                if let Some(draft) = &mut self.cloud_draft {
                    let fields = [draft.branch.clone(), draft.base.clone()];
                    for field in fields {
                        field.update(cx, |field, _| field.set_read_only(false));
                    }
                    draft.creating = false;
                    draft.created = created_codespace(&error).map(str::to_owned);
                    draft.error = Some(error);
                }
            }
        }
        cx.notify();
    }
}
