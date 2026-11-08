//! "RUN ON CLOUD" in the new-session composer: a manual way to put a fresh session in a
//! Codespace or a Docker Sandbox. Choosing a target in the Workspace menu does not create
//! anything. "Create and open" asks the shell to (see `shell/cloud_create.rs`), which makes the
//! session at once, queues the composer text as its first prompt and follows the start. Send
//! never provisions on its own: while a target is chosen and nothing is created it is disabled.
//!
//! The project's saved Docker config is only the default. The panel (see `cloud_panel.rs`) lets
//! the user override the size, lifetime, base branch, feature branch and Claude account of a
//! sandbox, or the machine and branch of a codespace, for this session only: the overrides are
//! applied to a copy of the config when "Create and open" is pressed and nothing is saved back.
use super::cloud_branches::BranchList;
use super::*;
use crate::docker_dialog::model::AccountChoice;
use monocode_core::cloud_sessions::docker_config::{self, DockerProjectConfig};
use monocode_core::codespaces::MachineType;

/// Shown as the hint and the send button's label while a cloud target waits for "Create and open".
pub(crate) const CREATE_FIRST: &str = "Create and open first";

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
    /// The muted billing note under the panel title.
    pub(super) fn billing_note(self) -> &'static str {
        match self {
            Self::Codespace => "Billed to your GitHub account",
            Self::DockerSandbox => "Paid compute in your Docker account",
        }
    }
}

/// Machine types offered for a new codespace, loaded off the UI thread when the panel opens.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Machines {
    Loading,
    Loaded(Vec<MachineType>),
    /// The listing failed or is not offered: the default machine stays.
    Failed,
}

/// Lists the machine types of a local project's repository. Blocking: runs on a worker.
pub(crate) type MachineLoader =
    Arc<dyn Fn(String) -> Result<Vec<MachineType>, String> + Send + Sync>;

/// A setting of the cloud panel that opens a menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CloudField {
    Compute,
    Runtime,
    Account,
    Machine,
}
impl CloudField {
    pub(super) fn index(self) -> usize {
        self as usize
    }
}

/// Compute sizes a sandbox can be created with: `(cpus, memory)`.
pub(crate) const COMPUTE_CHOICES: [(u32, &str); 2] = [(2, "4g"), (4, "8g")];
/// Runtime limits, the sandbox stops when it runs out.
pub(crate) const RUNTIME_CHOICES: [&str; 3] = ["1h", "2h", "4h"];

/// `1h` -> `1 hour then stop`.
pub(crate) fn runtime_label(ttl: &str) -> String {
    match ttl.strip_suffix('h').and_then(|n| n.parse::<u32>().ok()) {
        Some(1) => "1 hour then stop".into(),
        Some(n) => format!("{n} hours then stop"),
        None => format!("{ttl} then stop"),
    }
}

/// `2 vCPU / 4 GiB`.
pub(crate) fn size_label(cpus: u32, memory: &str) -> String {
    format!("{cpus} vCPU / {}", memory_label(memory))
}

/// What "Create and open" asks the shell for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CloudCreate {
    pub target: CloudTarget,
    /// The local project the session twins.
    pub project: String,
    /// Docker: the new feature branch. Codespace: the branch the codespace is created on.
    pub branch: String,
    /// Docker only: the project config with this session's overrides applied, validated.
    pub config: Option<DockerProjectConfig>,
    /// Codespace only: the machine type picked for this session; `None` keeps the default.
    pub machine: Option<String>,
    /// The composer text, queued as the first prompt.
    pub prompt: Option<String>,
    /// Which press asked; the answer only applies to the draft that is still waiting for it.
    pub request_id: u64,
}

/// Emitted by "Create and open".
pub(crate) struct CreateCloudSession(pub CloudCreate);
/// Emitted when the Docker Sandbox row is picked and the project has no saved config.
pub(crate) struct OpenDockerSetup(pub String);
impl EventEmitter<CreateCloudSession> for Conversation {}
impl EventEmitter<OpenDockerSetup> for Conversation {}

pub(super) struct CloudDraft {
    pub target: CloudTarget,
    /// Docker: the feature branch, editable. Codespace: the branch the codespace is created on.
    pub branch: Entity<input::Input>,
    /// Docker: the base branch, editable. Starts as the project's `base_ref`.
    pub base: Entity<input::Input>,
    /// What `base` holds, mirrored so labels need no entity read.
    pub base_text: String,
    /// The project's saved config: only the default of the overrides below (Docker).
    pub config: Option<DockerProjectConfig>,
    /// Docker overrides for this session, seeded from `config`.
    pub cpus: u32,
    pub memory: String,
    pub ttl: String,
    pub account: Option<String>,
    /// Accounts with a setup-token or an API key, read when the panel opened.
    pub accounts: Vec<AccountChoice>,
    /// Codespace: the machine picked, `None` while the default stays.
    pub machine: Option<String>,
    pub machines: Machines,
    /// Which listing the answer belongs to.
    pub machines_request: u64,
    /// The repository's branches for the Base branch / Branch row.
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
    _events: Vec<Subscription>,
}

/// The codespace named by a start error that happened after it was created
/// (`created_note`), if any.
pub(crate) fn created_codespace(error: &str) -> Option<&str> {
    let rest = error.split_once("(codespace ")?.1;
    let (name, tail) = rest.split_once(" was created and kept")?;
    (!name.is_empty() && !tail.is_empty() && !name.contains(' ')).then_some(name)
}

/// `4g` -> `4 GiB`, `512m` -> `512 MiB`; anything else as saved.
pub(crate) fn memory_label(memory: &str) -> String {
    for (suffix, unit) in [('g', "GiB"), ('m', "MiB")] {
        if let Some(digits) = memory.strip_suffix(suffix)
            && !digits.is_empty()
            && digits.bytes().all(|b| b.is_ascii_digit())
        {
            return format!("{digits} {unit}");
        }
    }
    memory.to_owned()
}

fn short_id() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos() ^ (d.as_secs() as u32));
    format!("{:06x}", nanos & 0xff_ffff)
}

/// What the project's config (or the built-in defaults) gives the overrides to start from:
/// `(cpus, memory, ttl, claude account, base branch)`.
fn config_defaults(
    config: Option<&DockerProjectConfig>,
) -> (u32, String, String, Option<String>, String) {
    let config = config.cloned().unwrap_or_default();
    (
        config.cpus,
        config.memory,
        config.ttl,
        config.claude_account,
        config.base_ref.trim().to_owned(),
    )
}

/// The label of the machine type `name`, from the loaded list when it knows it.
pub(super) fn machine_label(machines: &Machines, name: &str) -> String {
    match machines {
        Machines::Loaded(list) => list
            .iter()
            .find(|m| m.name == name)
            .filter(|m| !m.display_name.is_empty())
            .map_or_else(|| name.to_owned(), |m| m.display_name.clone()),
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

    /// A cloud target is chosen and nothing is created yet: the send button stays disabled.
    pub(crate) fn cloud_create_pending(&self) -> bool {
        self.cloud_draft.is_some()
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
        let mut rows = Vec::new();
        if self.cloud_offers_codespace {
            rows.push(
                MenuAction::new("workspace:cloud-codespace", "Codespace")
                    .icon("cloud-upload")
                    .checked(self.cloud_target() == Some(CloudTarget::Codespace))
                    .into(),
            );
        }
        let configured = self.docker_config().is_some();
        let mut docker = MenuAction::new("workspace:cloud-docker", "Docker Sandbox")
            .icon("cloud-upload")
            .checked(self.cloud_target() == Some(CloudTarget::DockerSandbox));
        if !configured {
            docker = docker.description("Set up first");
        }
        rows.push(docker.into());
        let mut items = vec![MenuItem::Separator, MenuItem::Header("Run on cloud".into())];
        items.extend(rows);
        items
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

    pub(super) fn forget_docker_config(&self) {
        self.docker_config_cache.take();
    }

    /// Handles the cloud rows of the Workspace menu; false for any other id.
    pub(crate) fn cloud_menu_choice(&mut self, id: &str, cx: &mut Context<Self>) -> bool {
        if let Some(setting) = id.strip_prefix("cloud-set:") {
            self.cloud_set(setting, cx);
            return true;
        }
        match id {
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

    fn choose_cloud_target(
        &mut self,
        target: CloudTarget,
        config: Option<DockerProjectConfig>,
        cx: &mut Context<Self>,
    ) {
        if let Some(draft) = &mut self.cloud_draft
            && draft.target == target
            && !draft.creating
        {
            // Picked again (after the setup dialog, say): the project's config is the default
            // again, so this session's overrides start over from it.
            let (cpus, memory, ttl, account, base) = config_defaults(config.as_ref());
            draft.config = config;
            draft.cpus = cpus;
            draft.memory = memory;
            draft.ttl = ttl;
            draft.account = account;
            draft.base_text = base.clone();
            let field = draft.base.clone();
            field.update(cx, |field, cx| field.set_text(base, cx));
            if target == CloudTarget::Codespace {
                // The same reset for a codespace: no machine pick, the branch from HEAD again
                // and a fresh listing.
                draft.machine = None;
                draft.machines = Machines::Loading;
                let branch = draft.branch.clone();
                let head = git_head(Path::new(&self.summary.cwd))
                    .and_then(|(name, detached)| (!detached).then_some(name))
                    .unwrap_or_default();
                branch.update(cx, |field, cx| field.set_text(head, cx));
                self.cloud_request_seq += 1;
                let request = self.cloud_request_seq;
                if let Some(draft) = &mut self.cloud_draft {
                    draft.machines_request = request;
                }
                self.load_cloud_machines(request, cx);
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
        let docker = target == CloudTarget::DockerSandbox;
        let (cpus, memory, ttl, account, base_text) = config_defaults(config.as_ref());
        // Docker names a new feature branch; a codespace starts on the current branch of the
        // project folder (HEAD, as the header picker does), empty while HEAD is detached.
        let default = if docker {
            format!("monoptah/{}", short_id())
        } else {
            git_head(Path::new(&self.summary.cwd))
                .and_then(|(branch, detached)| (!detached).then_some(branch))
                .unwrap_or_default()
        };
        let branch = cx.new(|cx| {
            let mut field = input::Input::field(
                "Branch",
                if docker { "monoptah/feature" } else { "main" },
                content(0.35),
                cx,
            );
            field.set_font_size(12.);
            field.set_text(default, cx);
            field
        });
        let base = cx.new(|cx| {
            let mut field = input::Input::field("Base branch", "default branch", content(0.35), cx);
            field.set_font_size(12.);
            field.set_text(base_text.clone(), cx);
            field
        });
        let events = vec![
            cx.subscribe(&branch, |_, _, _: &input::Changed, cx| cx.notify()),
            cx.subscribe(&base, |this, base, _: &input::Changed, cx| {
                let text = base.read(cx).content.to_string();
                if let Some(draft) = &mut this.cloud_draft {
                    draft.base_text = text;
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
        let accounts = if docker {
            crate::docker_dialog::claude_accounts(cx)
        } else {
            Vec::new()
        };
        self.cloud_request_seq += 1;
        let machines_request = self.cloud_request_seq;
        self.cloud_request_seq += 1;
        let branches_request = self.cloud_request_seq;
        self.cloud_draft = Some(CloudDraft {
            target,
            branch,
            base,
            base_text,
            config,
            cpus,
            memory,
            ttl,
            account,
            accounts,
            machine: None,
            machines: if docker {
                Machines::Failed
            } else {
                Machines::Loading
            },
            machines_request,
            branches: BranchList::Loading,
            branches_request,
            collapsed: false,
            creating: false,
            error: None,
            request: 0,
            created: None,
            _events: events,
        });
        if !docker {
            self.load_cloud_machines(machines_request, cx);
        }
        self.load_cloud_branches(branches_request, cx);
        cx.notify();
    }

    /// Lists the machine types on a worker; the answer only applies to the draft that asked.
    /// The default machine stays when there is no loader or the listing fails.
    fn load_cloud_machines(&mut self, request: u64, cx: &mut Context<Self>) {
        let Some(loader) = self.machine_loader.clone() else {
            if let Some(draft) = &mut self.cloud_draft {
                draft.machines = Machines::Failed;
            }
            return;
        };
        let project = self.summary.cwd.clone();
        let work = cx
            .background_executor()
            .spawn(async move { loader(project) });
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
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Applies one choice of a panel menu (`compute:2:4g`, `runtime:1h`, `account:<id>`,
    /// `machine:<name>`) to the draft. Nothing is saved: the choice lives for this session only.
    fn cloud_set(&mut self, setting: &str, cx: &mut Context<Self>) {
        let Some(draft) = self.cloud_draft.as_mut().filter(|draft| !draft.creating) else {
            return;
        };
        let (field, value) = setting.split_once(':').unwrap_or((setting, ""));
        match field {
            "compute" => {
                if let Some((cpus, memory)) = value.split_once(':')
                    && let Ok(cpus) = cpus.parse()
                {
                    draft.cpus = cpus;
                    draft.memory = memory.to_owned();
                }
            }
            "runtime" => draft.ttl = value.to_owned(),
            "account" => draft.account = (!value.is_empty()).then(|| value.to_owned()),
            "machine" => draft.machine = (!value.is_empty()).then(|| value.to_owned()),
            _ => return,
        }
        cx.notify();
    }

    /// The base branch the cloud session starts from.
    fn cloud_base(&self) -> String {
        match self.cloud_draft.as_ref() {
            Some(draft) if draft.target == CloudTarget::DockerSandbox => {
                let base = draft.base_text.trim();
                if base.is_empty() {
                    "default branch".into()
                } else {
                    base.to_owned()
                }
            }
            _ => self
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

    /// "Create and open": asks the shell to create the resource and the session.
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
        let typed = draft.branch.read(cx).content.trim().to_owned();
        let (branch, config) = match target {
            // Same source as the header picker unless the user typed another branch: HEAD of
            // the project folder (a worktree's branch would not exist on the remote).
            CloudTarget::Codespace if !typed.is_empty() => (typed, None),
            CloudTarget::Codespace => match git_head(Path::new(&self.summary.cwd)) {
                Some((_, true)) => {
                    fail(
                        self,
                        "HEAD is detached. Check out a branch to start a codespace on it.",
                        cx,
                    );
                    return;
                }
                Some((branch, false)) => (branch, None),
                None => (String::new(), None),
            },
            CloudTarget::DockerSandbox => {
                if typed.is_empty() {
                    fail(self, "Enter a branch name for the new session.", cx);
                    return;
                }
                self.forget_docker_config();
                match self.docker_config() {
                    Some(project) => {
                        // The project's config is the default; this copy carries the session's
                        // overrides and is checked by the same validation a saved one gets.
                        let mut config = project;
                        if let Some(draft) = &self.cloud_draft {
                            config.cpus = draft.cpus;
                            config.memory = draft.memory.clone();
                            config.ttl = draft.ttl.clone();
                            config.claude_account = draft.account.clone();
                            config.base_ref = draft.base.read(cx).content.trim().to_owned();
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
                        fail(self, "Set up Docker Sandbox for this project first.", cx);
                        return;
                    }
                }
            }
        };
        if branch.is_empty() {
            fail(self, "There is no branch to start the codespace on.", cx);
            return;
        }
        let machine = match target {
            CloudTarget::Codespace => self.cloud_draft.as_ref().and_then(|d| d.machine.clone()),
            CloudTarget::DockerSandbox => None,
        };
        let prompt = self.input.read(cx).content.trim().to_owned();
        self.cloud_request_seq += 1;
        let request_id = self.cloud_request_seq;
        let request = CloudCreate {
            request_id,
            target,
            project: self.summary.cwd.clone(),
            branch,
            config,
            machine,
            prompt: (!prompt.is_empty()).then_some(prompt),
        };
        if let Some(draft) = &mut self.cloud_draft {
            draft.creating = true;
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
    /// says whether the composer text moved into the new session's queue.
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
        match result {
            Ok(consumed) => {
                if consumed {
                    self.input
                        .update(cx, |input, cx| input.set_text(String::new(), cx));
                }
                // The resource exists: never offer a second "Create and open" for it.
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
