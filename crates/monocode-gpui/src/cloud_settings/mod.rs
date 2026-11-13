//! The project's "Cloud settings" dialog: one frame for both cloud backends (Codespaces and
//! Docker Sandbox). A segment picks the backend shown, a Ready checklist says what is missing
//! (with a Fix where the app can help), the Workspace rows set the repository, branch,
//! devcontainer, size and lifetime, and the Credentials rows pick the Docker account, the
//! Claude account and the Git credential. Both backends' unsaved edits are kept while the
//! segment switches. Save writes both parts through `project_cloud::save`; Cancel writes
//! nothing.
//!
//! Every `gh`, `sbx`, `git` and Docker Hub call runs on the background executor, and an
//! answer is dropped when the state it was asked for has changed (a generation counter per
//! kind of check). `model` holds what the dialog decides, `secrets` the Codespaces secrets
//! block with the `claude setup-token` flow.
pub(crate) mod model;
pub(crate) mod secrets;
#[cfg(test)]
pub(crate) mod tests;

use crate::{
    cloud_checks::{Gh, gh_state},
    cloud_dialog_ui::{alert, field_box, heading, labelled, muted},
    cloud_kit_ui,
    fix_terminal::FixCommand,
    git_credential_panel::{self, GitCredentialPanel},
    input::{self, Input},
    modal,
    popover::{AnchorSlot, Choice, ChoiceEvent, Choices, Popover},
    runtime::{Database, locked},
    settings::{cloud_env::CloudServices, cloud_env::Resolver, widgets::secondary_button},
    theme::*,
};
use gpui::{
    AnyElement, App, Context, Div, ElementId, Entity, EventEmitter, FocusHandle, Focusable,
    FontWeight, IntoElement, KeyDownEvent, Render, Role, SharedString, Stateful, Subscription,
    Window, div, prelude::*, px, rgb,
};
use model::{AccountChoice, Field, FieldError, Form};
use monocode_core::{
    cloud_sessions::{
        project_cloud::{self, DefaultBackend, DevcontainerInfo, ProjectCloud},
        readiness::{self, Fix, Item, State},
    },
    codespaces::{self, GhRunner, MachineType},
    docker_accounts::{DockerAccountSummary, DockerAccounts, DockerAuthError, Secret},
    git_credentials::{GitCredentialSummary, GitCredentials, GitRunner, SystemGit},
    sandboxes::{SbxRunner, SystemSbx},
    store::Store,
};
use secrets::{SaveStart, SecretsEvent, SecretsView};
use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

/// What the Ready section says under its title.
const READY_NOTE: &str =
    "Checked when this tab opens. Every start checks sign-in and the Docker account again.";
/// Shown when an edit changed what a checklist was computed for.
const READY_CHANGED: &str = "Changed: Check again";
const REPLACE_CONFIRM: &str = "Discard changes and open Settings > Cloud?";

/// The Claude accounts that hold a setup-token or an API key: the ones a cloud session can
/// use.
pub(crate) fn claude_accounts(cx: &App) -> Vec<AccountChoice> {
    use monocode_core::{accounts::ClaudeCredential, provider::Kind};
    let Some(center) = crate::accounts::get(cx) else {
        return Vec::new();
    };
    let center = center.read(cx);
    center
        .book
        .list(Kind::Claude)
        .into_iter()
        .filter(|account| center.uses_claude_token(&account.id))
        .map(|account| AccountChoice {
            kind: match center.claude_credential(&account.id) {
                Some(ClaudeCredential::ApiKey) => "API key",
                _ => "setup-token",
            },
            id: account.id,
            label: account.label,
        })
        .collect()
}

/// The width of the dialog, and the extra width of the Git credential side panel.
pub(crate) const DIALOG_WIDTH: f32 = 540.;
pub(crate) const PANEL_WIDTH: f32 = 340.;

pub(crate) enum Event {
    Close,
    /// The settings were stored: conversations of the project forget their cached copy.
    Saved,
    /// The dialog got wider or narrower (the Git credential side panel opened or closed): the
    /// host resizes the modal.
    Resized,
    /// Run a fix in a terminal; call [`CloudSettings::fix_finished`] when it ends.
    Fix(FixSlot),
    /// The Docker account's token must be replaced: open Settings > Cloud.
    ReplaceDockerToken,
}

/// A fix command on its way to the host. An event is only borrowed and a command holds
/// secrets that are not `Clone`, so the host takes it out of the slot once.
pub(crate) struct FixSlot(Mutex<Option<FixCommand>>);

impl FixSlot {
    pub(crate) fn new(command: FixCommand) -> Self {
        Self(Mutex::new(Some(command)))
    }

    pub(crate) fn take(&self) -> Option<FixCommand> {
        self.0.lock().ok()?.take()
    }
}

type DockerAccountsFn = fn(&Store) -> Result<Vec<DockerAccountSummary>, String>;
type DockerEnvFn = fn(Option<&str>) -> Result<Option<(&'static str, Secret)>, DockerAuthError>;

fn list_docker_accounts(store: &Store) -> Result<Vec<DockerAccountSummary>, String> {
    DockerAccounts::for_store(store)?.list()
}

/// The outside world the dialog reaches.
#[derive(Clone)]
pub(crate) struct Env {
    pub gh: Arc<dyn GhRunner>,
    pub sbx: Arc<dyn SbxRunner>,
    pub git: Arc<dyn GitRunner>,
    pub store: Database,
    pub home: Option<PathBuf>,
    pub origin: fn(&Path) -> Option<String>,
    pub claude: fn() -> Option<PathBuf>,
    /// The saved Docker accounts (no token).
    pub docker_accounts: DockerAccountsFn,
    /// The environment that makes a command act as a Docker account (the JWT exchange: a
    /// network call, so only ever called off the UI thread).
    pub docker_env: DockerEnvFn,
    /// Finds a program the way a GUI app must.
    pub resolve: Resolver,
    pub claude_accounts: Vec<AccountChoice>,
}

impl Env {
    /// The real world, with the shell's `gh` runner. Tests install an [`EnvOverride`] so a
    /// dialog opened through the shell reaches nothing.
    pub(crate) fn for_shell(
        store: Database,
        gh: Arc<dyn GhRunner>,
        claude_accounts: Vec<AccountChoice>,
        cx: &App,
    ) -> Self {
        if let Some(over) = cx.try_global::<EnvOverride>() {
            return over.0.clone();
        }
        let services = CloudServices::get(cx);
        Self {
            gh,
            sbx: Arc::new(SystemSbx),
            git: Arc::new(SystemGit),
            store,
            home: services.home,
            origin: secrets::model::git_origin,
            claude: || monocode_core::scm::resolve_gui_binary("claude"),
            docker_accounts: list_docker_accounts,
            docker_env: monocode_core::docker_accounts::env_for,
            resolve: services.resolve,
            claude_accounts,
        }
    }
}

/// Replaces the environment of dialogs opened through the shell (tests only install it).
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) struct EnvOverride(pub Env);
impl gpui::Global for EnvOverride {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Select {
    Default,
    Machine,
    Size,
    Idle,
    Runtime,
    DockerAccount,
    Claude,
    Git,
}

impl Select {
    /// Every select, in the order of `CloudSettings::select_anchors`.
    const COUNT: usize = 8;

    fn index(self) -> usize {
        match self {
            Self::Default => 0,
            Self::Machine => 1,
            Self::Size => 2,
            Self::Idle => 3,
            Self::Runtime => 4,
            Self::DockerAccount => 5,
            Self::Claude => 6,
            Self::Git => 7,
        }
    }

    fn id(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::Machine => "machine",
            Self::Size => "size",
            Self::Idle => "idle",
            Self::Runtime => "runtime",
            Self::DockerAccount => "docker",
            Self::Claude => "claude",
            Self::Git => "git",
        }
    }
}

/// What picking an option does.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Pick {
    Default(DefaultBackend),
    Machine(Option<String>),
    Size(u32, String),
    Idle(u32),
    Runtime(String),
    DockerAccount(Option<String>),
    Claude(Option<String>),
    Git(Option<String>),
}

/// The floating list of the select that is open, and which opening it belongs to.
struct OpenSelect {
    which: Select,
    /// An event of an older list (one that was replaced by another select's) is not this one's.
    opening: u64,
    choices: Choices,
}

/// Where an open select's list draws: above the dialog's own layer (60 in the shell) and its
/// barrier, below the fix terminal that stacks on the dialog. A select is closed before a fix
/// terminal can open, so the two never share a layer.
pub(crate) const SELECT_PRIORITY: usize = 61;

struct Opt {
    label: String,
    selected: bool,
    pick: Pick,
}

enum Machines {
    NotLoaded,
    Loading,
    Loaded(Vec<MachineType>),
    /// The list could not be read: the machine is typed instead.
    Failed(String),
}

enum Ready {
    NotStarted,
    Checking,
    /// An edit made a running check stale: its answer was dropped.
    Changed,
    Done(Vec<Item>),
}

fn ix(backend: DefaultBackend) -> usize {
    match backend {
        DefaultBackend::Codespace => 0,
        DefaultBackend::Sandbox => 1,
    }
}

pub(crate) struct CloudSettings {
    project: String,
    name: String,
    env: Env,
    tab: DefaultBackend,
    form: Form,
    /// The sandbox form as it was offered (with the origin prefill), to tell an untouched
    /// section from an edited one.
    sandbox_baseline: model::SandboxForm,
    sandbox_saved: bool,
    sandbox_wanted: bool,
    /// The project's origin, once read.
    origin: Option<String>,
    /// Whether the origin was read at all: "not read yet" is not "no origin".
    origin_read: bool,
    /// The form as it was offered, to tell whether there is anything to lose.
    initial: Form,
    load_error: Option<String>,
    cs_base: Entity<Input>,
    cs_devcontainer: Entity<Input>,
    cs_machine: Entity<Input>,
    repo: Entity<Input>,
    sb_base: Entity<Input>,
    sb_devcontainer: Entity<Input>,
    credentials: Vec<GitCredentialSummary>,
    credentials_loaded: bool,
    credentials_error: Option<String>,
    credentials_gen: u64,
    /// A credential to select once the newest read ends (an older read was superseded).
    pending_select: Option<String>,
    docker_accounts: Vec<DockerAccountSummary>,
    docker_accounts_loaded: bool,
    docker_accounts_error: Option<String>,
    machines: Machines,
    machines_gen: u64,
    ready: [Ready; 2],
    ready_gen: [u64; 2],
    /// An edit changed what a finished checklist was computed for.
    ready_stale: [bool; 2],
    /// The newest `gh` state a Codespaces check found; the secrets block reuses it.
    gh: Option<Gh>,
    infos: [DevcontainerInfo; 2],
    infos_gen: u64,
    open_select: Option<OpenSelect>,
    /// Counts the lists opened, to tell a stale list's event from the current one's.
    select_openings: u64,
    /// Each select trigger's bounds, which its floating list is placed against.
    select_anchors: [AnchorSlot; Select::COUNT],
    error: Option<FieldError>,
    /// Save is waiting for the Codespaces secrets to reach GitHub.
    saving: bool,
    /// "Replace token" was pressed with unsaved edits: asking before discarding them.
    confirm_replace: bool,
    /// A fix waits for the Docker account's token (off the UI thread).
    fix_pending: bool,
    fix_gen: u64,
    /// The backend whose fix is running in the terminal.
    fix_backend: Option<DefaultBackend>,
    secrets: Option<Entity<SecretsView>>,
    /// The Git credential side panel (Docker Sandbox tab), inside this dialog.
    panel: Option<(Entity<GitCredentialPanel>, Subscription)>,
    /// Focus returns to the dialog on the next render (the panel that had it is gone).
    refocus: bool,
    focus: FocusHandle,
    _events: Vec<Subscription>,
}

impl EventEmitter<Event> for CloudSettings {}

impl Focusable for CloudSettings {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

fn text_input(
    label: &'static str,
    placeholder: &str,
    value: &str,
    cx: &mut Context<CloudSettings>,
) -> Entity<Input> {
    let (placeholder, value) = (placeholder.to_owned(), value.to_owned());
    let input = cx.new(|cx| {
        let mut field = Input::field(label, placeholder, content(0.35), cx);
        field.set_font_size(13.);
        field
    });
    input.update(cx, |input, cx| input.set_text(value, cx));
    input
}

impl CloudSettings {
    /// Opens on `backend`, else on the project's default backend.
    pub(crate) fn new(
        project: String,
        name: String,
        backend: Option<DefaultBackend>,
        env: Env,
        cx: &mut Context<Self>,
    ) -> Self {
        let (saved, load_error) = match locked(&env.store)
            .and_then(|store| project_cloud::load(&store, &project).map_err(|e| e.to_string()))
        {
            Ok(saved) => (saved, None),
            Err(message) => {
                // An unreadable part must not hide the other one.
                let codespace = locked(&env.store)
                    .and_then(|store| project_cloud::load_codespace(&store, &project))
                    .unwrap_or_default();
                (
                    ProjectCloud {
                        default_backend: codespace.default_backend,
                        codespace: codespace.codespace,
                        sandbox: None,
                    },
                    Some(message),
                )
            }
        };
        let form = Form::initial(&saved, None);
        let tab = backend.unwrap_or_else(|| saved.effective_default_backend());
        let sandbox_saved = saved.sandbox.is_some();
        let cs_base = text_input(
            "Base branch",
            "The current branch of the project folder",
            &form.codespace.base_ref,
            cx,
        );
        let cs_devcontainer = text_input(
            "Devcontainer configuration path",
            "Repository default",
            &form.codespace.devcontainer,
            cx,
        );
        let cs_machine = text_input(
            "Machine type",
            codespaces::DEFAULT_MACHINE,
            form.codespace.machine.as_deref().unwrap_or(""),
            cx,
        );
        let repo = text_input(
            "Repository URL",
            "https://github.com/owner/repo",
            &form.sandbox.repo_url,
            cx,
        );
        let sb_base = text_input(
            "Base branch",
            "Default branch of the repository",
            &form.sandbox.base_ref,
            cx,
        );
        let sb_devcontainer = text_input(
            "Devcontainer configuration path",
            monocode_core::cloud_sessions::docker_config::DEFAULT_DEVCONTAINER,
            &form.sandbox.devcontainer,
            cx,
        );
        let mut events = Vec::new();
        events.push(cx.subscribe(&repo, |this, _, _: &input::Changed, cx| {
            this.repo_changed(cx)
        }));
        // Only a path that a check or a hint reads is read again, and off the UI thread.
        events.push(
            cx.subscribe(&cs_devcontainer, |this, _, _: &input::Changed, cx| {
                this.mark_stale(DefaultBackend::Codespace);
                this.refresh_infos(cx);
                cx.notify();
            }),
        );
        events.push(
            cx.subscribe(&sb_devcontainer, |this, _, _: &input::Changed, cx| {
                this.mark_stale(DefaultBackend::Sandbox);
                this.refresh_infos(cx);
                cx.notify();
            }),
        );
        for input in [&cs_base, &cs_machine, &sb_base] {
            events.push(cx.subscribe(input, |_, _, _: &input::Changed, cx| cx.notify()));
        }
        for input in [
            &repo,
            &cs_base,
            &cs_devcontainer,
            &cs_machine,
            &sb_base,
            &sb_devcontainer,
        ] {
            events.push(cx.subscribe(input, |this, _, _: &input::Submitted, cx| this.save(cx)));
        }
        let sandbox_baseline = form.sandbox.clone();
        let initial = form.clone();
        let mut this = Self {
            project,
            name,
            env,
            tab,
            form,
            sandbox_baseline,
            sandbox_saved,
            sandbox_wanted: backend == Some(DefaultBackend::Sandbox) && !sandbox_saved,
            origin: None,
            origin_read: false,
            initial,
            load_error,
            cs_base,
            cs_devcontainer,
            cs_machine,
            repo,
            sb_base,
            sb_devcontainer,
            credentials: Vec::new(),
            credentials_loaded: false,
            credentials_error: None,
            credentials_gen: 0,
            pending_select: None,
            docker_accounts: Vec::new(),
            docker_accounts_loaded: false,
            docker_accounts_error: None,
            machines: Machines::NotLoaded,
            machines_gen: 0,
            ready: [Ready::NotStarted, Ready::NotStarted],
            ready_gen: [0, 0],
            ready_stale: [false, false],
            gh: None,
            infos: [DevcontainerInfo::default(), DevcontainerInfo::default()],
            infos_gen: 0,
            open_select: None,
            select_openings: 0,
            select_anchors: Default::default(),
            error: None,
            saving: false,
            confirm_replace: false,
            fix_pending: false,
            fix_gen: 0,
            fix_backend: None,
            secrets: None,
            panel: None,
            refocus: false,
            focus: cx.focus_handle(),
            _events: events,
        };
        this.refresh_infos(cx);
        this.reload_credentials(None, cx);
        this.load_docker_accounts(cx);
        this.read_origin(cx);
        this.tab_shown(cx);
        this
    }

    /// Whether leaving now would lose something typed or picked.
    fn dirty(&self, cx: &App) -> bool {
        self.typed(cx) != self.initial
            || self
                .secrets
                .as_ref()
                .is_some_and(|secrets| secrets.read(cx).has_typed(cx))
    }

    /// An edit changed what a checklist was computed for: its answer is dropped (or, when
    /// already shown, marked "Changed: Check again"). No command runs on a keystroke.
    fn mark_stale(&mut self, backend: DefaultBackend) {
        let slot = ix(backend);
        match self.ready[slot] {
            Ready::NotStarted | Ready::Changed => {}
            Ready::Checking => {
                self.ready_gen[slot] += 1;
                self.ready[slot] = Ready::Changed;
            }
            Ready::Done(_) => {
                self.ready_gen[slot] += 1;
                self.ready_stale[slot] = true;
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn tab(&self) -> DefaultBackend {
        self.tab
    }

    pub(crate) fn repo_url(&self, cx: &App) -> String {
        self.repo.read(cx).content.trim().to_owned()
    }

    // -------------------------------------------------------------- background loads

    /// Reads the project's `origin` off the UI thread; prefills the repository field when
    /// no sandbox is configured and the user typed nothing.
    fn read_origin(&mut self, cx: &mut Context<Self>) {
        let (origin, project) = (self.env.origin, self.project.clone());
        cx.spawn(async move |this, cx| {
            let found = cx
                .background_executor()
                .spawn(async move { origin(Path::new(&project)) })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.origin = found.clone();
                this.origin_read = true;
                if !this.sandbox_saved && this.repo.read(cx).content.trim().is_empty() {
                    let url = model::prefill_repo(found.as_deref());
                    if !url.is_empty() {
                        this.repo
                            .update(cx, |input, cx| input.set_text(url.clone(), cx));
                        // The prefill is not an edit of the user's.
                        this.sandbox_baseline.repo_url = url.clone();
                        this.initial.sandbox.repo_url = url;
                    }
                }
                this.refresh_infos(cx);
                cx.notify();
            });
        })
        .detach();
    }

    /// Reads the saved Git credentials again, off the UI thread, and selects `select` if given.
    pub(crate) fn reload_credentials(&mut self, select: Option<String>, cx: &mut Context<Self>) {
        // Only the newest read counts; a pick asked for by an older one is kept for it.
        self.credentials_gen += 1;
        let generation = self.credentials_gen;
        if select.is_some() {
            self.pending_select = select;
        }
        let store = self.env.store.clone();
        cx.spawn(async move |this, cx| {
            let listed = cx
                .background_executor()
                .spawn(async move {
                    let credentials =
                        locked(&store).and_then(|store| GitCredentials::for_store(&store))?;
                    credentials.list()
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.credentials_gen != generation {
                    return;
                }
                let select = this.pending_select.take();
                match listed {
                    Ok(list) => {
                        this.credentials = list;
                        this.credentials_error = None;
                    }
                    Err(message) => {
                        this.credentials.clear();
                        this.credentials_error = Some(message);
                    }
                }
                this.credentials_loaded = true;
                if let Some(id) = select {
                    this.form.sandbox.credential_id = Some(id);
                    this.check_ready(DefaultBackend::Sandbox, cx);
                } else if let Some(id) = &this.form.sandbox.credential_id
                    && this.credentials_error.is_none()
                    && !this.credentials.iter().any(|c| c.id == *id)
                {
                    this.form.sandbox.credential_id = None;
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn load_docker_accounts(&mut self, cx: &mut Context<Self>) {
        let (store, list) = (self.env.store.clone(), self.env.docker_accounts);
        cx.spawn(async move |this, cx| {
            let listed = cx
                .background_executor()
                .spawn(async move { locked(&store).and_then(|store| list(&store)) })
                .await;
            let _ = this.update(cx, |this, cx| {
                match listed {
                    Ok(list) => {
                        this.docker_accounts = list;
                        this.docker_accounts_error = None;
                    }
                    Err(message) => {
                        this.docker_accounts.clear();
                        this.docker_accounts_error = Some(message);
                    }
                }
                this.docker_accounts_loaded = true;
                cx.notify();
            });
        })
        .detach();
    }

    /// Lists the machine types of the project's repository, off the UI thread.
    fn load_machines(&mut self, cx: &mut Context<Self>) {
        // What was typed in the text fallback is the machine now.
        self.form.codespace.machine = self.typed(cx).codespace.machine;
        self.machines_gen += 1;
        let generation = self.machines_gen;
        self.machines = Machines::Loading;
        cx.notify();
        let (gh, origin, project) = (self.env.gh.clone(), self.env.origin, self.project.clone());
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let origin = origin(Path::new(&project));
                    let repo = origin
                        .as_deref()
                        .and_then(codespaces::repo_from_remote)
                        .ok_or_else(|| "The project has no GitHub origin.".to_owned())?;
                    codespaces::list_machines(gh.as_ref(), &repo)
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.machines_gen != generation {
                    return;
                }
                this.machines = match result {
                    Ok(list) if !list.is_empty() => Machines::Loaded(list),
                    Ok(_) => Machines::Failed("GitHub returned no machine types.".into()),
                    Err(message) => Machines::Failed(message),
                };
                if matches!(this.machines, Machines::Failed(_)) {
                    // The text fallback starts from the machine picked so far.
                    let text = this.form.codespace.machine.clone().unwrap_or_default();
                    this.cs_machine
                        .update(cx, |input, cx| input.set_text(text, cx));
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Computes the Ready checklist of `backend` off the UI thread. An answer for a state
    /// that has changed since (another check started) is dropped.
    pub(crate) fn check_ready(&mut self, backend: DefaultBackend, cx: &mut Context<Self>) {
        let slot = ix(backend);
        self.ready_gen[slot] += 1;
        let generation = self.ready_gen[slot];
        self.ready[slot] = Ready::Checking;
        self.ready_stale[slot] = false;
        cx.notify();
        let typed = self.typed(cx);
        let (env, project) = (self.env.clone(), self.project.clone());
        cx.spawn(async move |this, cx| {
            let (items, gh) = cx
                .background_executor()
                .spawn(async move {
                    let root = Path::new(&project);
                    let origin = (env.origin)(root);
                    match backend {
                        DefaultBackend::Codespace => {
                            // One `gh auth status`: the checklist and the secrets block
                            // both use its answer.
                            let auth = codespaces::auth_status(env.gh.as_ref());
                            let gh = gh_state(auth.clone());
                            let items = readiness::codespace_with_auth(
                                auth,
                                root,
                                origin.as_deref(),
                                &typed.codespace.devcontainer,
                            );
                            (items, Some(gh))
                        }
                        DefaultBackend::Sandbox => {
                            let credentials = locked(&env.store)
                                .ok()
                                .and_then(|store| GitCredentials::for_store(&store).ok());
                            let items = readiness::sandbox(
                                env.sbx.as_ref(),
                                env.git.as_ref(),
                                credentials.as_ref(),
                                &readiness::SandboxQuery {
                                    project_root: root,
                                    origin: origin.as_deref(),
                                    home: env.home.as_deref(),
                                    docker_account: typed.sandbox.docker_account.as_deref(),
                                    repo_url: &typed.sandbox.repo_url,
                                    credential_id: typed.sandbox.credential_id.as_deref(),
                                    devcontainer_path: &typed.sandbox.devcontainer,
                                },
                            );
                            (items, None)
                        }
                    }
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.ready_gen[slot] != generation {
                    return;
                }
                this.ready[slot] = Ready::Done(items);
                if let Some(gh) = gh {
                    this.gh = Some(gh.clone());
                    if let Some(secrets) = this.secrets.clone() {
                        secrets.update(cx, |view, cx| view.refresh(gh, cx));
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// A tab became visible: run its first checks.
    fn tab_shown(&mut self, cx: &mut Context<Self>) {
        let tab = self.tab;
        if matches!(self.ready[ix(tab)], Ready::NotStarted) {
            self.check_ready(tab, cx);
            if tab == DefaultBackend::Codespace && matches!(self.machines, Machines::NotLoaded) {
                self.load_machines(cx);
            }
        }
        self.ensure_secrets(cx);
        cx.notify();
    }

    /// Builds the Codespaces secrets block the first time it is needed.
    fn ensure_secrets(&mut self, cx: &mut Context<Self>) {
        if self.secrets.is_some()
            || self.tab != DefaultBackend::Codespace
            || self.form.codespace.claude.is_some()
        {
            return;
        }
        let env = secrets::Env {
            runner: self.env.gh.clone(),
            origin: self.env.origin,
            claude: self.env.claude,
        };
        let project = self.project.clone();
        let secrets = cx.new(|cx| SecretsView::new(project, env, cx));
        self._events.push(cx.subscribe(
            &secrets,
            |this, _, event: &SecretsEvent, cx| match event {
                SecretsEvent::Submitted => this.save(cx),
                SecretsEvent::SaveFinished { ok } => this.secrets_saved(*ok, cx),
            },
        ));
        // The Ready check already ran `gh auth status`: reuse its answer, or wait for it.
        if let Some(gh) = self.gh.clone() {
            secrets.update(cx, |view, cx| view.refresh(gh, cx));
        }
        self.secrets = Some(secrets);
    }

    /// Re-checks what a fix was fixing. Called by the host when the fix terminal's command
    /// ends.
    pub(crate) fn fix_finished(&mut self, cx: &mut Context<Self>) {
        if let Some(backend) = self.fix_backend.take() {
            self.check_ready(backend, cx);
            if backend == DefaultBackend::Codespace {
                // The secrets block follows the new check.
                self.load_machines(cx);
            }
        }
    }

    // -------------------------------------------------------------- form

    /// The form with what the text fields hold now.
    fn typed(&self, cx: &App) -> Form {
        let mut form = self.form.clone();
        let text = |input: &Entity<Input>| input.read(cx).content.to_string();
        form.codespace.base_ref = text(&self.cs_base);
        form.codespace.devcontainer = text(&self.cs_devcontainer);
        if let Machines::Failed(_) | Machines::NotLoaded = self.machines {
            let typed = text(&self.cs_machine);
            let typed = typed.trim();
            form.codespace.machine = (!typed.is_empty()).then(|| typed.to_owned());
        }
        form.sandbox.repo_url = text(&self.repo);
        form.sandbox.base_ref = text(&self.sb_base);
        form.sandbox.devcontainer = text(&self.sb_devcontainer);
        form
    }

    /// Reads the devcontainer files again, off the UI thread; only the newest answer counts.
    fn refresh_infos(&mut self, cx: &mut Context<Self>) {
        self.infos_gen += 1;
        let generation = self.infos_gen;
        let typed = self.typed(cx);
        let local = self
            .origin
            .as_deref()
            .is_some_and(|origin| readiness::same_repository(origin, &typed.sandbox.repo_url));
        let root = PathBuf::from(&self.project);
        let (codespace_path, sandbox_path) =
            (typed.codespace.devcontainer, typed.sandbox.devcontainer);
        cx.spawn(async move |this, cx| {
            let infos = cx
                .background_executor()
                .spawn(async move {
                    let codespace = project_cloud::devcontainer_info(&root, &codespace_path);
                    let sandbox = if local {
                        project_cloud::devcontainer_info(&root, &sandbox_path)
                    } else {
                        DevcontainerInfo::default()
                    };
                    [codespace, sandbox]
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.infos_gen == generation {
                    this.infos = infos;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// A different host than the picked credential's drops the pick.
    fn repo_changed(&mut self, cx: &mut Context<Self>) {
        let url = self.repo.read(cx).content.to_string();
        if let Some(id) = &self.form.sandbox.credential_id
            && let Ok((platform, _, _)) = monocode_core::git_credentials::normalize_origin(&url)
            && self
                .credentials
                .iter()
                .any(|c| &c.id == id && c.platform != platform)
        {
            self.form.sandbox.credential_id = None;
        }
        self.mark_stale(DefaultBackend::Sandbox);
        self.refresh_infos(cx);
        if let Some((panel, _)) = &self.panel {
            panel.update(cx, |panel, cx| {
                panel.set_repo_url(url.trim().to_owned(), cx)
            });
        }
        cx.notify();
    }

    fn effective_default(&self) -> DefaultBackend {
        self.form.default_backend.unwrap_or(if self.sandbox_saved {
            DefaultBackend::Sandbox
        } else {
            DefaultBackend::Codespace
        })
    }

    fn switch_tab(
        &mut self,
        backend: DefaultBackend,
        window: Option<&Window>,
        cx: &mut Context<Self>,
    ) {
        if self.tab == backend {
            return;
        }
        self.tab = backend;
        self.open_select = None;
        // The Git credential panel belongs to the Docker Sandbox tab.
        self.close_credentials(window, cx);
        self.tab_shown(cx);
    }

    fn validation_context<R>(&self, f: impl FnOnce(&model::Context<'_>) -> R) -> R {
        let ctx = model::Context {
            credentials: &self.credentials,
            credentials_loaded: self.credentials_loaded,
            claude_accounts: &self.env.claude_accounts,
            docker_accounts: self
                .docker_accounts_loaded
                .then_some(self.docker_accounts.as_slice())
                .filter(|_| self.docker_accounts_error.is_none()),
            sandbox_saved: self.sandbox_saved,
            sandbox_baseline: &self.sandbox_baseline,
            sandbox_wanted: self.sandbox_wanted,
        };
        f(&ctx)
    }

    /// Shows a save error. An error that belongs to a field brings its tab forward; a general
    /// one (the store, the secrets) leaves the tab the user is on.
    fn show_error(&mut self, error: FieldError, cx: &mut Context<Self>) {
        if error.field != Field::General {
            self.tab = error.backend;
        }
        self.error = Some(error);
        self.tab_shown(cx);
        cx.notify();
    }

    /// Validates both backends, saves the Codespaces secrets typed in the block, then stores
    /// the settings. Field errors stay in the dialog; success closes it. When GitHub refuses
    /// a secret nothing is stored and the dialog stays open with the error.
    pub(crate) fn save(&mut self, cx: &mut Context<Self>) {
        if self.saving {
            return;
        }
        let form = self.typed(cx);
        // Nothing is sent to GitHub for a form that would not be stored.
        if let Err(error) = self.validation_context(|ctx| model::validate(&form, ctx)) {
            self.show_error(error, cx);
            return;
        }
        let start = match &self.secrets {
            Some(secrets) if form.codespace.claude.is_none() => {
                secrets.update(cx, |view, cx| view.begin_save(cx))
            }
            _ => SaveStart::Nothing,
        };
        match start {
            SaveStart::Nothing => self.store_form(form, cx),
            SaveStart::Started => {
                self.saving = true;
                self.error = None;
                cx.notify();
            }
            SaveStart::Blocked(message) => self.show_error(
                FieldError {
                    backend: DefaultBackend::Codespace,
                    field: Field::General,
                    message,
                },
                cx,
            ),
        }
    }

    /// The Codespaces secrets save the dialog asked for ended.
    fn secrets_saved(&mut self, ok: bool, cx: &mut Context<Self>) {
        if !std::mem::take(&mut self.saving) {
            return;
        }
        if ok {
            let form = self.typed(cx);
            self.store_form(form, cx);
        } else {
            let message = self
                .secrets
                .as_ref()
                .and_then(|secrets| secrets.read(cx).error_text())
                .unwrap_or_else(|| "The Codespaces secrets could not be saved.".into());
            self.show_error(
                FieldError {
                    backend: DefaultBackend::Codespace,
                    field: Field::General,
                    message: format!("Nothing was saved. {message}"),
                },
                cx,
            );
        }
    }

    /// Writes the validated settings and closes the dialog. A form still exactly as it was
    /// offered writes nothing: Save without an edit must not invent a stored default.
    fn store_form(&mut self, form: Form, cx: &mut Context<Self>) {
        if form == self.initial && !self.sandbox_wanted {
            if let Err(error) = self.validation_context(|ctx| model::validate(&form, ctx)) {
                self.show_error(error, cx);
                return;
            }
            self.error = None;
            cx.emit(Event::Saved);
            cx.emit(Event::Close);
            return;
        }
        // The settings are small and live in the native store like the app's other
        // settings, so they are written on the UI thread.
        let result = locked(&self.env.store)
            .map_err(|message| FieldError {
                backend: self.tab,
                field: Field::General,
                message,
            })
            .and_then(|store| {
                self.validation_context(|ctx| model::save(&store, &self.project, &form, ctx))
            });
        match result {
            Ok(saved) => {
                self.sandbox_saved = saved.sandbox.is_some();
                self.form = form;
                self.error = None;
                cx.emit(Event::Saved);
                cx.emit(Event::Close);
            }
            Err(error) => {
                self.form = form;
                self.show_error(error, cx);
            }
        }
    }

    // -------------------------------------------------------------- selects

    fn claude_options(&self, backend: DefaultBackend) -> Vec<Opt> {
        let (current, none_label) = match backend {
            DefaultBackend::Codespace => {
                (&self.form.codespace.claude, model::CODESPACE_SECRET_LABEL)
            }
            DefaultBackend::Sandbox => (
                &self.form.sandbox.claude,
                crate::cloud_presets::SANDBOX_NO_CLAUDE_LABEL,
            ),
        };
        crate::cloud_presets::claude_choices(
            none_label,
            &self.env.claude_accounts,
            current.as_deref(),
            false,
        )
        .into_iter()
        .map(|choice| Opt {
            label: choice.label,
            selected: choice.selected,
            pick: Pick::Claude(choice.id),
        })
        .collect()
    }

    fn options(&self, which: Select, cx: &App) -> Vec<Opt> {
        let typed = self.typed(cx);
        match which {
            Select::Default => {
                let current = self.effective_default();
                [
                    (DefaultBackend::Codespace, "Codespaces"),
                    (DefaultBackend::Sandbox, "Docker Sandbox"),
                ]
                .into_iter()
                .map(|(backend, label)| Opt {
                    label: label.into(),
                    selected: backend == current,
                    pick: Pick::Default(backend),
                })
                .collect()
            }
            Select::Machine => {
                let Machines::Loaded(machines) = &self.machines else {
                    return Vec::new();
                };
                let current = typed.codespace.machine.as_deref();
                let selected = model::selected_machine(machines, current);
                model::machine_options(machines, current, &self.infos[0])
                    .into_iter()
                    .map(|option| Opt {
                        selected: selected.as_deref() == Some(option.name.as_str()),
                        pick: Pick::Machine(Some(option.name)),
                        label: option.label,
                    })
                    .collect()
            }
            Select::Size => model::size_options(typed.sandbox.cpus, &typed.sandbox.memory)
                .into_iter()
                .map(|(cpus, memory)| Opt {
                    label: model::size_label(cpus, &memory),
                    selected: cpus == typed.sandbox.cpus && memory == typed.sandbox.memory,
                    pick: Pick::Size(cpus, memory),
                })
                .collect(),
            Select::Idle => model::idle_options(typed.codespace.idle)
                .into_iter()
                .map(|minutes| Opt {
                    label: model::idle_label(minutes),
                    selected: minutes == typed.codespace.idle,
                    pick: Pick::Idle(minutes),
                })
                .collect(),
            Select::Runtime => model::runtime_options(&typed.sandbox.ttl)
                .into_iter()
                .map(|ttl| Opt {
                    label: model::runtime_label(&ttl),
                    selected: ttl == typed.sandbox.ttl,
                    pick: Pick::Runtime(ttl),
                })
                .collect(),
            Select::DockerAccount => {
                let current = &typed.sandbox.docker_account;
                let mut options = vec![Opt {
                    label: crate::cloud_presets::MACHINE_SIGN_IN_LABEL.into(),
                    selected: current.is_none(),
                    pick: Pick::DockerAccount(None),
                }];
                for account in &self.docker_accounts {
                    options.push(Opt {
                        label: model::docker_account_label(account),
                        selected: current.as_deref() == Some(account.id.as_str()),
                        pick: Pick::DockerAccount(Some(account.id.clone())),
                    });
                }
                if let Some(id) = current
                    && self.docker_accounts_loaded
                    && !self.docker_accounts.iter().any(|a| &a.id == id)
                {
                    options.push(Opt {
                        label: format!("{id} (no longer exists)"),
                        selected: true,
                        pick: Pick::DockerAccount(Some(id.clone())),
                    });
                }
                options
            }
            Select::Claude => self.claude_options(self.tab),
            Select::Git => {
                let current = &typed.sandbox.credential_id;
                let mut options: Vec<Opt> =
                    model::matching_credentials(&self.credentials, &typed.sandbox.repo_url)
                        .into_iter()
                        .map(|credential| Opt {
                            label: format!(
                                "{} ({}, saved credential)",
                                credential.label,
                                credential.platform.host()
                            ),
                            selected: current.as_deref() == Some(credential.id.as_str()),
                            pick: Pick::Git(Some(credential.id)),
                        })
                        .collect();
                options.push(Opt {
                    label: "None (read-only clone)".into(),
                    selected: current.is_none(),
                    pick: Pick::Git(None),
                });
                options
            }
        }
    }

    fn pick(&mut self, pick: Pick, cx: &mut Context<Self>) {
        self.open_select = None;
        self.error = None;
        match pick {
            Pick::Default(backend) => self.form.default_backend = Some(backend),
            Pick::Machine(machine) => {
                // Also in the text fallback, so a failed reload cannot bring the old one back.
                let text = machine.clone().unwrap_or_default();
                self.cs_machine
                    .update(cx, |input, cx| input.set_text(text, cx));
                self.form.codespace.machine = machine;
            }
            Pick::Size(cpus, memory) => {
                self.form.sandbox.cpus = cpus;
                self.form.sandbox.memory = memory;
            }
            Pick::Idle(minutes) => self.form.codespace.idle = minutes,
            Pick::Runtime(ttl) => self.form.sandbox.ttl = ttl,
            Pick::DockerAccount(account) => {
                if self.form.sandbox.docker_account != account {
                    self.form.sandbox.docker_account = account;
                    self.check_ready(DefaultBackend::Sandbox, cx);
                }
            }
            Pick::Claude(account) => {
                match self.tab {
                    DefaultBackend::Codespace => {
                        // Leaving "Codespaces secret" ends a running `claude setup-token`:
                        // a token it prints later must not be saved as that secret.
                        if account.is_some()
                            && let Some(secrets) = self.secrets.clone()
                        {
                            secrets.update(cx, |view, cx| view.stop_generating(cx));
                        }
                        self.form.codespace.claude = account;
                    }
                    DefaultBackend::Sandbox => self.form.sandbox.claude = account,
                }
                self.ensure_secrets(cx);
            }
            Pick::Git(credential) => {
                if self.form.sandbox.credential_id != credential {
                    self.form.sandbox.credential_id = credential;
                    self.check_ready(DefaultBackend::Sandbox, cx);
                }
            }
        }
        cx.notify();
    }

    // -------------------------------------------------------------- fixes

    fn fix_title(line: &str) -> &'static str {
        if line.starts_with("gh auth") {
            "Sign in to GitHub"
        } else if line.starts_with("sbx --cloud setup ssh") {
            "Set up ssh for Docker Sandbox"
        } else if line.starts_with("sbx login") {
            "Sign in to Docker"
        } else {
            "Install the GitHub CLI"
        }
    }

    /// Runs the fix of a checklist line. The Docker account's token (a Hub exchange, a
    /// network call) is fetched off the UI thread and goes only into the terminal's
    /// environment.
    pub(crate) fn run_fix(&mut self, backend: DefaultBackend, fix: Fix, cx: &mut Context<Self>) {
        let checking = matches!(self.ready[ix(backend)], Ready::Checking);
        if self.fix_pending || checking || self.fix_backend.is_some() {
            return;
        }
        let line = match fix {
            Fix::ReplaceToken => {
                // Settings opens over nothing of this dialog: unsaved edits would be lost, so
                // ask first.
                if self.dirty(cx) && !self.confirm_replace {
                    self.confirm_replace = true;
                    cx.notify();
                    return;
                }
                self.confirm_replace = false;
                cx.emit(Event::ReplaceDockerToken);
                return;
            }
            Fix::Command(line) => line,
        };
        let resolve = self.env.resolve.clone();
        let Some(command) =
            FixCommand::from_line(Self::fix_title(line), line, |name| resolve(name))
        else {
            return;
        };
        let account = (backend == DefaultBackend::Sandbox && line == readiness::SBX_SSH_COMMAND)
            .then(|| self.form.sandbox.docker_account.clone())
            .flatten();
        let Some(account) = account else {
            self.fix_backend = Some(backend);
            self.open_select = None;
            cx.emit(Event::Fix(FixSlot::new(command)));
            return;
        };
        self.fix_pending = true;
        self.fix_gen += 1;
        let generation = self.fix_gen;
        self.error = None;
        cx.notify();
        let docker_env = self.env.docker_env;
        cx.spawn(async move |this, cx| {
            let env = cx
                .background_executor()
                .spawn(async move { docker_env(Some(&account)) })
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.fix_gen != generation || !this.fix_pending {
                    return;
                }
                this.fix_pending = false;
                match env {
                    Ok(pair) => {
                        let command = match pair {
                            Some((name, token)) => command.with_env(name, token),
                            None => command,
                        };
                        this.fix_backend = Some(backend);
                        this.open_select = None;
                        cx.emit(Event::Fix(FixSlot::new(command)));
                    }
                    Err(error) => {
                        this.error = Some(FieldError {
                            backend,
                            field: Field::DockerAccount,
                            message: error.to_string(),
                        });
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Closes the topmost transient layer, one per call; true when something was closed.
    /// Escape does this before it closes the dialog. The order is: an open select, the Git
    /// credential panel (its "Confirm remove" question first, then the panel itself), the
    /// replace-token question.
    pub(crate) fn dismiss_transient(
        &mut self,
        window: Option<&Window>,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.open_select.take().is_some() {
            // The list had the keyboard; the dialog takes it back on the next render.
            self.refocus = true;
            cx.notify();
            return true;
        }
        // Then the Git credential panel: its own question first, then the panel itself.
        if let Some((panel, _)) = &self.panel {
            if !panel.update(cx, |panel, cx| panel.dismiss_transient(cx)) {
                self.close_credentials(window, cx);
            }
            return true;
        }
        let asked = std::mem::take(&mut self.confirm_replace);
        if asked {
            cx.notify();
        }
        asked
    }

    /// Opens the Git credential side panel for the repository typed in the form, with the
    /// picked profile shown, and gives it the keyboard.
    fn open_credentials(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.tab != DefaultBackend::Sandbox {
            return;
        }
        self.open_select = None;
        if self.panel.is_none() {
            let env = git_credential_panel::Env {
                store: self.env.store.clone(),
                git: self.env.git.clone(),
            };
            let (name, repo_url, selected) = (
                self.name.clone(),
                self.repo_url(cx),
                self.form.sandbox.credential_id.clone(),
            );
            let panel = cx.new(|cx| GitCredentialPanel::new(name, repo_url, selected, env, cx));
            let events = cx.subscribe_in(
                &panel,
                window,
                |this, _, event: &git_credential_panel::Event, window, cx| match event {
                    git_credential_panel::Event::Close => this.close_credentials(Some(window), cx),
                    // A saved or picked profile becomes the form's selection and the panel
                    // closes: the choice is made.
                    git_credential_panel::Event::Chosen(id) => {
                        this.close_credentials(Some(window), cx);
                        this.reload_credentials(Some(id.clone()), cx);
                    }
                    git_credential_panel::Event::Removed => this.reload_credentials(None, cx),
                },
            );
            self.panel = Some((panel, events));
            cx.emit(Event::Resized);
        }
        if let Some((panel, _)) = &self.panel {
            let panel = panel.clone();
            panel.update(cx, |panel, cx| panel.focus_form(window, cx));
        }
        cx.notify();
    }

    /// Closes the Git credential panel and forgets anything typed in it. The dialog takes the
    /// keyboard back on the next render, but only when the panel had it: a field in the main
    /// column keeps its caret. The saved list is read again, because a save or remove that
    /// ends after the panel is gone can no longer report itself.
    fn close_credentials(&mut self, window: Option<&Window>, cx: &mut Context<Self>) {
        if let Some((panel, _)) = self.panel.take() {
            let had_focus =
                window.is_some_and(|window| panel.focus_handle(cx).contains_focused(window, cx));
            panel.update(cx, |panel, cx| panel.shutdown(cx));
            self.refocus = had_focus;
            self.reload_credentials(None, cx);
            cx.emit(Event::Resized);
            cx.notify();
        }
    }

    /// The width the modal needs now.
    pub(crate) fn dialog_width(&self) -> f32 {
        DIALOG_WIDTH
            + if self.panel.is_some() {
                PANEL_WIDTH
            } else {
                0.
            }
    }

    #[cfg(test)]
    pub(crate) fn credentials_panel(&self) -> Option<Entity<GitCredentialPanel>> {
        self.panel.as_ref().map(|(panel, _)| panel.clone())
    }

    /// The embedded `claude setup-token` terminal has the keyboard: Escape belongs to it.
    pub(crate) fn terminal_focused(&self, window: &Window, cx: &App) -> bool {
        self.secrets
            .as_ref()
            .is_some_and(|secrets| secrets.read(cx).terminal_focused(window, cx))
    }

    /// Ends the secrets terminal and drops anything typed; called when the dialog closes.
    pub(crate) fn shutdown(&mut self, cx: &mut Context<Self>) {
        // Answers still in flight are dropped.
        self.ready_gen = [self.ready_gen[0] + 1, self.ready_gen[1] + 1];
        self.machines_gen += 1;
        self.fix_gen += 1;
        self.infos_gen += 1;
        self.credentials_gen += 1;
        self.fix_pending = false;
        self.saving = false;
        if let Some(secrets) = self.secrets.take() {
            secrets.update(cx, |view, cx| view.shutdown(cx));
        }
        if let Some((panel, _)) = self.panel.take() {
            panel.update(cx, |panel, cx| panel.shutdown(cx));
        }
    }

    // -------------------------------------------------------------- tests

    #[cfg(test)]
    fn set_text(&mut self, which: &str, text: &str, cx: &mut Context<Self>) {
        let input = match which {
            "repo" => &self.repo,
            "sb_base" => &self.sb_base,
            "sb_dev" => &self.sb_devcontainer,
            "cs_base" => &self.cs_base,
            "cs_dev" => &self.cs_devcontainer,
            _ => &self.cs_machine,
        };
        input.update(cx, |input, cx| input.set_text(text.to_owned(), cx));
    }

    #[cfg(test)]
    pub(crate) fn selected_credential(&self) -> Option<String> {
        self.form.sandbox.credential_id.clone()
    }

    /// A select's list is open.
    pub(crate) fn select_open(&self) -> bool {
        self.open_select.is_some()
    }

    #[cfg(test)]
    pub(crate) fn select_label(&self, id: &str, cx: &App) -> String {
        let which = [
            Select::Default,
            Select::Machine,
            Select::Size,
            Select::Idle,
            Select::Runtime,
            Select::DockerAccount,
            Select::Claude,
            Select::Git,
        ]
        .into_iter()
        .find(|s| s.id() == id)
        .expect("a select id");
        self.current_label(which, cx)
    }

    #[cfg(test)]
    pub(crate) fn open_select_id(&self) -> Option<&'static str> {
        self.open_select.as_ref().map(|open| open.which.id())
    }

    #[cfg(test)]
    pub(crate) fn secret_text(&self, cx: &App) -> String {
        self.secrets
            .as_ref()
            .map(|secrets| secrets.read(cx).first_field_text(cx))
            .unwrap_or_default()
    }

    /// Pretends the project's devcontainer asks for `info` on both backends.
    #[cfg(test)]
    pub(crate) fn set_requirements(&mut self, info: DevcontainerInfo, cx: &mut Context<Self>) {
        self.infos_gen += 1;
        self.infos = [info.clone(), info];
        cx.notify();
    }

    #[cfg(test)]
    pub(crate) fn saved_credential_ids(&self) -> Vec<String> {
        self.credentials.iter().map(|c| c.id.clone()).collect()
    }

    #[cfg(test)]
    pub(crate) fn text(&self, which: &str, cx: &App) -> String {
        let input = match which {
            "repo" => &self.repo,
            "sb_base" => &self.sb_base,
            "sb_dev" => &self.sb_devcontainer,
            "cs_base" => &self.cs_base,
            "cs_dev" => &self.cs_devcontainer,
            _ => &self.cs_machine,
        };
        input.read(cx).content.to_string()
    }
}

// ---------------------------------------------------------------------- render

const GREEN: u32 = 0x34d399;
const AMBER: u32 = 0xfbbf24;

fn section_title(text: &'static str, note: Option<&'static str>) -> Div {
    div()
        .flex()
        .items_baseline()
        .gap_2()
        .child(
            div()
                .text_size(px(13.))
                .font_weight(FontWeight::SEMIBOLD)
                .child(text),
        )
        .children(note.map(muted))
}

/// Two fields side by side, equal halves, tops aligned.
fn pair(left: Div, right: Div) -> Div {
    div()
        .flex()
        .items_start()
        .gap_3()
        .child(left.flex_1().min_w_0())
        .child(right.flex_1().min_w_0())
}

fn rule() -> Div {
    div().h(px(1.)).bg(content(0.08))
}

impl CloudSettings {
    fn error_for(&self, backend: DefaultBackend, field: Field) -> Option<AnyElement> {
        let error = self
            .error
            .as_ref()
            .filter(|e| e.backend == backend && e.field == field)?;
        let id = format!("cloud-error-{field:?}").to_lowercase();
        Some(alert(id, error.message.clone()).into_any_element())
    }

    fn segment(
        &self,
        backend: DefaultBackend,
        label: &'static str,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let on = self.tab == backend;
        let id: &'static str = match backend {
            DefaultBackend::Codespace => "cloud-seg-codespace",
            DefaultBackend::Sandbox => "cloud-seg-sandbox",
        };
        div()
            .id(id)
            .debug_selector(move || id.into())
            .role(Role::Tab)
            .aria_label(label)
            .aria_selected(on)
            .flex_1()
            .h(px(30.))
            .flex()
            .items_center()
            .justify_center()
            .rounded_md()
            .cursor_pointer()
            .text_size(px(12.))
            .font_weight(FontWeight::MEDIUM)
            .when(on, |d| d.bg(content(0.12)).text_color(content(1.)))
            .when(!on, |d| {
                d.text_color(content(0.6)).hover(|s| s.bg(content(0.06)))
            })
            .on_click(
                cx.listener(move |this, _, window, cx| this.switch_tab(backend, Some(window), cx)),
            )
            .child(label)
    }

    /// Opens the floating list of `which` under its trigger (or closes it when it is the open
    /// one). The list is the app's dropdown [`Menu`](crate::popover::Menu), so opening it moves
    /// nothing in the form.
    fn toggle_select(&mut self, which: Select, window: &mut Window, cx: &mut Context<Self>) {
        if self.open_select.as_ref().is_some_and(|o| o.which == which) {
            self.open_select = None;
            self.refocus = true;
            cx.notify();
            return;
        }
        let options = self.options(which, cx);
        if options.is_empty() {
            self.open_select = None;
            cx.notify();
            return;
        }
        let width = self.select_anchors[which.index()]
            .get()
            .map_or(220., |bounds| bounds.size.width.as_f32());
        self.select_openings += 1;
        let opening = self.select_openings;
        let choices = options
            .into_iter()
            .map(|option| Choice {
                value: option.pick,
                label: option.label.into(),
                checked: option.selected,
            })
            .collect();
        let choices = Choices::open(
            which.id(),
            choices,
            width,
            window,
            cx,
            move |this, event, _, cx| {
                // A newer list replaced this one: its close is not the new list's.
                if this
                    .open_select
                    .as_ref()
                    .is_none_or(|o| o.opening != opening)
                {
                    return;
                }
                match event {
                    ChoiceEvent::Picked(pick) => this.pick(pick, cx),
                    ChoiceEvent::Dismissed => {
                        this.open_select = None;
                        cx.notify();
                    }
                }
            },
        );
        self.open_select = Some(OpenSelect {
            which,
            opening,
            choices,
        });
        cx.notify();
    }

    /// The open select's list, drawn above the dialog (see [`SELECT_PRIORITY`]).
    fn select_popover(&self, cx: &App) -> Option<Popover> {
        let open = self.open_select.as_ref()?;
        Some(open.choices.popover(
            &self.select_anchors[open.which.index()],
            SELECT_PRIORITY,
            cx,
        ))
    }

    /// Where the open select's list is drawn, so the dialog's own outside-press dismissal
    /// treats a press on it as inside (it can reach past the dialog's card).
    pub(crate) fn select_surface(&self, cx: &App) -> Option<AnchorSlot> {
        self.open_select
            .as_ref()
            .map(|open| open.choices.surface(cx))
    }

    /// A select: the current value in a box; clicking opens its list floating below it.
    fn select(&mut self, which: Select, current: String, cx: &mut Context<Self>) -> Div {
        let open = self.open_select.as_ref().is_some_and(|o| o.which == which);
        let id = which.id();
        let trigger_id: SharedString = format!("cloud-select-{id}").into();
        div().flex().flex_col().child(
            div()
                .id(ElementId::from(trigger_id.clone()))
                .debug_selector({
                    let selector = trigger_id.to_string();
                    move || selector.clone()
                })
                .role(Role::ComboBox)
                .aria_label(current.clone())
                .aria_expanded(open)
                .relative()
                .h(px(34.))
                .min_w_0()
                .px_3()
                .flex()
                .items_center()
                .justify_between()
                .gap_2()
                .rounded_lg()
                .border_1()
                .border_color(content(if open { 0.3 } else { 0.15 }))
                .bg(content(0.03))
                .cursor_pointer()
                .text_size(px(12.5))
                .hover(|s| s.bg(content(0.06)))
                .on_click(
                    cx.listener(move |this, _, window, cx| this.toggle_select(which, window, cx)),
                )
                .child(self.select_anchors[which.index()].probe())
                .child(div().min_w_0().truncate().child(current))
                .child(
                    div()
                        .flex_shrink_0()
                        .text_color(content(0.5))
                        .child(if open { "\u{25b4}" } else { "\u{25be}" }),
                ),
        )
    }

    fn current_label(&self, which: Select, cx: &App) -> String {
        self.options(which, cx)
            .into_iter()
            .find(|o| o.selected)
            .map(|o| o.label)
            .unwrap_or_default()
    }

    fn select_row(&mut self, label: &'static str, which: Select, cx: &mut Context<Self>) -> Div {
        let current = self.current_label(which, cx);
        labelled(label).child(self.select(which, current, cx))
    }

    fn ready_section(&self, cx: &mut Context<Self>) -> Div {
        let backend = self.tab;
        let slot = ix(backend);
        let state = &self.ready[slot];
        let checking = matches!(state, Ready::Checking | Ready::NotStarted);
        let busy = checking || self.fix_pending || self.fix_backend.is_some();
        let mut column = div().flex().flex_col().gap(px(6.));
        column = column.child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap_3()
                .child(section_title("Ready", None))
                .child(
                    div()
                        .id("cloud-recheck")
                        .debug_selector(|| "cloud-recheck".into())
                        .role(Role::Button)
                        .aria_label("Check again")
                        .text_size(px(12.))
                        .text_color(content(0.55))
                        .when(!checking, |d| {
                            d.cursor_pointer()
                                .hover(|s| s.text_color(content(1.)))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.check_ready(backend, cx);
                                    if backend == DefaultBackend::Codespace {
                                        this.load_machines(cx);
                                    }
                                }))
                        })
                        .when(checking, |d| d.opacity(0.4))
                        .child("Check again"),
                ),
        );
        column = column.child(muted(READY_NOTE));
        let changed = matches!(state, Ready::Changed) || self.ready_stale[slot];
        match state {
            Ready::NotStarted | Ready::Checking => {
                column = column.child(
                    div()
                        .id("cloud-ready-checking")
                        .debug_selector(|| "cloud-ready-checking".into())
                        .text_size(px(12.))
                        .text_color(content(0.55))
                        .child("Checking..."),
                );
            }
            Ready::Changed => {}
            Ready::Done(items) => {
                column = column.child(self.ready_row(items, busy, cx));
            }
        }
        if changed {
            column = column.child(
                div()
                    .id("cloud-ready-changed")
                    .debug_selector(|| "cloud-ready-changed".into())
                    .text_size(px(11.5))
                    .text_color(rgb(AMBER))
                    .child(READY_CHANGED),
            );
        }
        if self.confirm_replace {
            column = column.child(
                div()
                    .id("cloud-replace-confirm")
                    .debug_selector(|| "cloud-replace-confirm".into())
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_size(px(12.))
                            .text_color(rgb(AMBER))
                            .child(REPLACE_CONFIRM),
                    )
                    .child(
                        secondary_button("cloud-replace-discard", "Discard and open", true, false)
                            .child("Discard and open")
                            .debug_selector(|| "cloud-replace-discard".into())
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.confirm_replace = false;
                                cx.emit(Event::ReplaceDockerToken);
                            })),
                    )
                    .child(
                        secondary_button("cloud-replace-keep", "Keep editing", false, false)
                            .child("Keep editing")
                            .debug_selector(|| "cloud-replace-keep".into())
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.confirm_replace = false;
                                cx.notify();
                            })),
                    ),
            );
        }
        column
    }

    /// The text of a checklist chip. A working Docker account is named by its Docker ID.
    fn chip_text(&self, item: &Item) -> String {
        if item.id == readiness::Id::DockerAccount
            && item.state == State::Ok
            && let Some(id) = &self.form.sandbox.docker_account
            && let Some(account) = self.docker_accounts.iter().find(|a| &a.id == id)
        {
            return format!("Docker account {}", account.docker_id);
        }
        item.text.clone()
    }

    /// The checklist as chips that wrap, with the fixes at the end of the row.
    fn ready_row(&self, items: &[Item], busy: bool, cx: &mut Context<Self>) -> Div {
        let backend = self.tab;
        let mut chips = div().min_w_0().flex_1().flex().flex_wrap().gap(px(6.));
        let mut fixes = div().flex_shrink_0().flex().flex_col().gap(px(4.));
        for item in items {
            let (mark, colour) = match item.state {
                State::Ok => ("\u{2713}", rgb(GREEN)),
                State::Problem => ("!", rgb(AMBER)),
                State::Unknown => ("\u{2026}", rgb(0x9ca3af)),
            };
            let key = item.id.key();
            let row_id: SharedString = format!("cloud-ready-{key}").into();
            chips = chips.child(
                div()
                    .flex()
                    .items_start()
                    .gap(px(5.))
                    .px(px(8.))
                    .py(px(3.))
                    .rounded_md()
                    .bg(content(0.05))
                    .child(
                        div()
                            .flex_shrink_0()
                            .text_size(px(12.))
                            .text_color(colour)
                            .child(mark),
                    )
                    .child(
                        div()
                            .id(ElementId::from(row_id.clone()))
                            .debug_selector({
                                let selector = row_id.to_string();
                                move || selector.clone()
                            })
                            .min_w_0()
                            .text_size(px(12.))
                            .line_height(px(18.))
                            .text_color(if item.state == State::Ok {
                                content(0.8)
                            } else {
                                content(0.95)
                            })
                            .child(self.chip_text(item)),
                    ),
            );
            if let Some(fix) = item.fix {
                let label = match fix {
                    Fix::ReplaceToken => "Replace token",
                    Fix::Command(_) => "Fix",
                };
                let id: SharedString = format!("cloud-fix-{key}").into();
                fixes = fixes.child(
                    secondary_button(ElementId::from(id.clone()), label, false, busy)
                        .child(label)
                        .debug_selector({
                            let selector = id.to_string();
                            move || selector.clone()
                        })
                        .when(!busy, |b| {
                            b.on_click(
                                cx.listener(move |this, _, _, cx| this.run_fix(backend, fix, cx)),
                            )
                        }),
                );
            }
        }
        div()
            .flex()
            .items_start()
            .justify_between()
            .gap_3()
            .child(chips)
            .child(fixes)
    }

    fn input_row(
        &self,
        label: &'static str,
        id: &'static str,
        input: &Entity<Input>,
        backend: DefaultBackend,
        field: Field,
    ) -> Div {
        labelled(label)
            .child(field_box(id, input))
            .children(self.error_for(backend, field))
    }

    /// What the Codespaces Repository row says: the GitHub repository, or why there is none.
    fn codespace_repo_text(&self) -> String {
        match (&self.origin, self.origin_read) {
            (Some(origin), _) => codespaces::repo_from_remote(origin).map_or_else(
                || "The origin is not a GitHub repository".into(),
                |repo| format!("github.com/{repo}"),
            ),
            (None, true) => "The project has no git origin.".into(),
            (None, false) => "Reading the project's origin...".into(),
        }
    }

    fn codespace_workspace(&mut self, cx: &mut Context<Self>) -> Div {
        let cs = DefaultBackend::Codespace;
        let repo_text = self.codespace_repo_text();
        let mut column = div().flex().flex_col().gap_3().child(
            labelled("Repository").child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .id("cloud-repo-readonly")
                            .debug_selector(|| "cloud-repo-readonly".into())
                            .min_w_0()
                            .truncate()
                            .text_size(px(12.5))
                            .child(repo_text),
                    )
                    .child(
                        div()
                            .flex_shrink_0()
                            .px(px(6.))
                            .rounded_md()
                            .bg(content(0.08))
                            .text_size(px(10.))
                            .text_color(content(0.6))
                            .child("from origin"),
                    ),
            ),
        );
        column = column.child(self.input_row(
            "Base branch",
            "cloud-field-cs-base",
            &self.cs_base.clone(),
            cs,
            Field::Base,
        ));
        column = column.child(self.input_row(
            "Devcontainer",
            "cloud-field-cs-devcontainer",
            &self.cs_devcontainer.clone(),
            cs,
            Field::Devcontainer,
        ));
        // Machine and Stop when idle share a row; the warning and the errors sit under it.
        let machine = match &self.machines {
            Machines::Loaded(_) => {
                let current = self.current_label(Select::Machine, cx);
                labelled("Machine").child(self.select(Select::Machine, current, cx))
            }
            Machines::NotLoaded | Machines::Loading => labelled("Machine").child(
                div()
                    .id("cloud-machines-loading")
                    .debug_selector(|| "cloud-machines-loading".into())
                    .h(px(34.))
                    .flex()
                    .items_center()
                    .text_size(px(12.))
                    .text_color(content(0.55))
                    .child("Reading the machine types..."),
            ),
            Machines::Failed(_) => {
                labelled("Machine").child(field_box("cloud-field-cs-machine", &self.cs_machine))
            }
        };
        column = column.child(pair(
            machine,
            self.select_row("Stop when idle", Select::Idle, cx),
        ));
        if let Machines::Failed(message) = &self.machines {
            column = column.child(muted(format!(
                "Could not list the machine types ({}).",
                message.trim_end_matches('.')
            )));
        }
        column = column.children(self.machine_warning(cx));
        column = column.children(self.error_for(cs, Field::Machine));
        column.children(self.error_for(cs, Field::Idle))
    }

    fn machine_warning(&self, cx: &App) -> Option<Stateful<Div>> {
        let Machines::Loaded(machines) = &self.machines else {
            return None;
        };
        let typed = self.typed(cx);
        let name = model::selected_machine(machines, typed.codespace.machine.as_deref())?;
        let machine = machines.iter().find(|m| m.name == name)?;
        let warning = model::size_warning(
            machine.cpus,
            Some(machine.memory_in_bytes / (1024 * 1024)),
            &self.infos[0],
        )?;
        Some(
            div()
                .id("cloud-machine-warning")
                .debug_selector(|| "cloud-machine-warning".into())
                .text_size(px(11.))
                .line_height(px(17.))
                .text_color(rgb(AMBER))
                .child(warning),
        )
    }

    fn size_warning(&self) -> Option<Stateful<Div>> {
        let form = &self.form.sandbox;
        let warning =
            model::size_warning(form.cpus, model::memory_mib(&form.memory), &self.infos[1])?;
        Some(
            div()
                .id("cloud-size-warning")
                .debug_selector(|| "cloud-size-warning".into())
                .text_size(px(11.))
                .line_height(px(17.))
                .text_color(rgb(AMBER))
                .child(warning),
        )
    }

    fn sandbox_workspace(&mut self, cx: &mut Context<Self>) -> Div {
        let sb = DefaultBackend::Sandbox;
        let mut column = div().flex().flex_col().gap_3();
        column = column.child(self.input_row(
            "Repository",
            "cloud-field-repo",
            &self.repo.clone(),
            sb,
            Field::Repo,
        ));
        column = column.child(self.input_row(
            "Base branch",
            "cloud-field-sb-base",
            &self.sb_base.clone(),
            sb,
            Field::Base,
        ));
        column = column.child(self.input_row(
            "Devcontainer",
            "cloud-field-sb-devcontainer",
            &self.sb_devcontainer.clone(),
            sb,
            Field::Devcontainer,
        ));
        // Size and Runtime limit share a row; the warning and the errors sit under it.
        column = column.child(pair(
            self.select_row("Size", Select::Size, cx),
            self.select_row("Runtime limit", Select::Runtime, cx),
        ));
        column = column.children(self.size_warning());
        column = column.children(self.error_for(sb, Field::Size));
        column.children(self.error_for(sb, Field::Runtime))
    }

    fn credentials_section(&mut self, cx: &mut Context<Self>) -> Div {
        let backend = self.tab;
        let mut column = div().flex().flex_col().gap_3();
        if backend == DefaultBackend::Sandbox {
            column = column.child(
                self.select_row("Docker account", Select::DockerAccount, cx)
                    .children(
                        self.docker_accounts_error
                            .clone()
                            .map(|message| alert("cloud-docker-accounts-error", message)),
                    )
                    .children(self.error_for(backend, Field::DockerAccount)),
            );
        }
        let claude = if self.env.claude_accounts.is_empty() {
            labelled("Claude").child(self.select(
                Select::Claude,
                self.current_label(Select::Claude, cx),
                cx,
            ))
        } else {
            self.select_row("Claude", Select::Claude, cx)
        };
        column = column.child(claude.children(self.error_for(backend, Field::Claude)));
        if backend == DefaultBackend::Codespace && self.form.codespace.claude.is_none() {
            column = column.children(self.secrets.clone());
        }
        column = column.child(self.git_row(cx));
        // The one line the spec asks for, for the whole group.
        column.child(
            muted(model::ACCOUNT_CHANGE_NOTE)
                .id("cloud-credentials-note")
                .debug_selector(|| "cloud-credentials-note".into()),
        )
    }

    fn git_row(&mut self, cx: &mut Context<Self>) -> Div {
        match self.tab {
            DefaultBackend::Codespace => labelled("Git").child(
                div()
                    .id("cloud-git-codespace")
                    .debug_selector(|| "cloud-git-codespace".into())
                    .text_size(px(12.5))
                    .child(model::GIT_CODESPACE_LINE),
            ),
            DefaultBackend::Sandbox => {
                let current = self.current_label(Select::Git, cx);
                let select = self.select(Select::Git, current, cx);
                labelled("Git")
                    .child(
                        div()
                            .flex()
                            .items_start()
                            .gap_2()
                            .child(div().min_w_0().flex_1().child(select))
                            .child(
                                secondary_button("cloud-git-manage", "Manage", false, false)
                                    .child("Manage")
                                    .debug_selector(|| "cloud-git-manage".into())
                                    .h(px(34.))
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.open_credentials(window, cx)
                                    })),
                            ),
                    )
                    .children(
                        self.credentials_error
                            .clone()
                            .map(|message| alert("cloud-credentials-error", message)),
                    )
                    .children(self.error_for(DefaultBackend::Sandbox, Field::Credential))
            }
        }
    }

    /// `New cloud sessions use ...` for the backend shown.
    pub(crate) fn summary(&self, cx: &App) -> String {
        let typed = self.typed(cx);
        let kit_on = cloud_kit_ui::default_on(&cloud_kit_ui::kit_prefs(cx));
        let account_label = |id: &Option<String>, none: &str| match id {
            None => none.to_owned(),
            Some(id) => self
                .env
                .claude_accounts
                .iter()
                .find(|a| &a.id == id)
                .map_or_else(
                    || crate::cloud_presets::unavailable_account_label(id),
                    |a| a.label.clone(),
                ),
        };
        match self.tab {
            DefaultBackend::Codespace => {
                let machine = match &self.machines {
                    Machines::Loaded(list) => {
                        model::selected_machine(list, typed.codespace.machine.as_deref())
                            .map(|name| crate::cloud_presets::machine_name_label(list, &name))
                    }
                    _ => Some(
                        typed
                            .codespace
                            .machine
                            .clone()
                            .unwrap_or_else(|| codespaces::DEFAULT_MACHINE.to_owned()),
                    ),
                }
                .unwrap_or_else(|| codespaces::DEFAULT_MACHINE.to_owned());
                model::summary_line(
                    self.tab,
                    &machine,
                    &account_label(&typed.codespace.claude, "Codespaces secret"),
                    None,
                    kit_on,
                )
            }
            DefaultBackend::Sandbox => {
                let docker = typed.sandbox.docker_account.as_ref().map(|id| {
                    self.docker_accounts
                        .iter()
                        .find(|a| &a.id == id)
                        .map_or_else(|| id.clone(), |a| a.docker_id.clone())
                });
                model::summary_line(
                    self.tab,
                    &model::size_label(typed.sandbox.cpus, &typed.sandbox.memory),
                    &account_label(
                        &typed.sandbox.claude,
                        crate::cloud_presets::SANDBOX_NO_CLAUDE_LABEL,
                    ),
                    docker.as_deref(),
                    kit_on,
                )
            }
        }
    }
}

impl Render for CloudSettings {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let height = window.viewport_size().height.as_f32() - 34.;
        let backend = self.tab;
        if std::mem::take(&mut self.refocus) {
            window.focus(&self.focus, cx);
        }
        let default_select = {
            let current = self.current_label(Select::Default, cx);
            div()
                .id("cloud-default-row")
                .debug_selector(|| "cloud-default-row".into())
                .flex()
                .items_start()
                .justify_between()
                .gap_2()
                .child(
                    div()
                        .min_w_0()
                        .pt(px(8.))
                        .text_size(px(11.5))
                        .text_color(content(0.6))
                        .child("Default for new cloud sessions"),
                )
                .child(div().w(px(150.)).flex_shrink_0().child(self.select(
                    Select::Default,
                    current,
                    cx,
                )))
        };
        let subtitle = format!(
            "{} \u{b7} {}",
            self.name,
            model::display_path(&self.project, self.env.home.as_deref())
        );
        let workspace = match backend {
            DefaultBackend::Codespace => self.codespace_workspace(cx),
            DefaultBackend::Sandbox => self.sandbox_workspace(cx),
        };
        let body = div()
            .id("cloud-settings-body")
            .min_h_0()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap_4()
            .p_4()
            .child(
                div()
                    .id("cloud-segments")
                    .debug_selector(|| "cloud-segments".into())
                    .w_full()
                    .flex()
                    .p(px(2.))
                    .gap(px(2.))
                    .rounded_lg()
                    .bg(content(0.05))
                    .child(self.segment(DefaultBackend::Codespace, "Codespaces", cx))
                    .child(self.segment(DefaultBackend::Sandbox, "Docker Sandbox", cx)),
            )
            .child(default_select)
            .child(rule())
            .child(self.ready_section(cx))
            .child(rule())
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(section_title("Workspace", None))
                    .child(workspace),
            )
            .child(rule())
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(section_title("Credentials", None))
                    .child(self.credentials_section(cx)),
            )
            .children(
                self.load_error
                    .clone()
                    .map(|message| alert("cloud-load-error", message)),
            )
            .children(
                self.error
                    .as_ref()
                    .filter(|e| e.field == Field::General)
                    .map(|e| alert("cloud-error-general", e.message.clone())),
            );
        let summary = self.summary(cx);
        let footer = div()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .gap_2()
            .px_4()
            .pb_4()
            .pt_3()
            .border_t_1()
            .border_color(content(0.08))
            .child(
                div()
                    .id("cloud-summary")
                    .debug_selector(|| "cloud-summary".into())
                    .role(Role::Status)
                    .aria_label(summary.clone())
                    .text_size(px(11.5))
                    .line_height(px(17.))
                    .text_color(content(0.65))
                    .child(summary),
            )
            .child(
                modal::actions()
                    .mt(px(0.))
                    .child(
                        modal::ghost_button("cloud-cancel", "Cancel")
                            .debug_selector(|| "cloud-cancel".into())
                            .cursor_pointer()
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(Event::Close))),
                    )
                    .child(
                        modal::primary_button(
                            "cloud-save",
                            if self.saving { "Saving..." } else { "Save" },
                        )
                        .debug_selector(|| "cloud-save".into())
                        .cursor_pointer()
                        .on_click(cx.listener(|this, _, _, cx| this.save(cx))),
                    ),
            );
        let main = div()
            .id("cloud-settings-main")
            .debug_selector(|| "cloud-settings-main".into())
            .flex_1()
            .min_w_0()
            .min_h_0()
            .max_h(px(height))
            .flex()
            .flex_col()
            .child(
                div()
                    .flex_shrink_0()
                    .flex()
                    .items_start()
                    .gap_2()
                    .px_4()
                    .pt_3()
                    .child(heading("Cloud settings", subtitle))
                    .child(
                        icon_button("cloud-close", "Close Cloud settings", "close")
                            .size(px(28.))
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(Event::Close))),
                    ),
            )
            .child(body)
            .child(footer);
        div()
            .id("cloud-settings")
            .debug_selector(|| "cloud-settings".into())
            .role(Role::Dialog)
            .aria_label("Cloud settings")
            .track_focus(&self.focus)
            .relative()
            .min_h_0()
            .max_h(px(height))
            .flex()
            .flex_row()
            .text_color(content(1.))
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                if event.keystroke.key == "escape" && !this.terminal_focused(window, cx) {
                    // An open select or the Git credential panel closes first; the dialog
                    // stays.
                    if !this.dismiss_transient(Some(window), cx) {
                        cx.emit(Event::Close);
                    }
                    window.prevent_default();
                    cx.stop_propagation();
                }
            }))
            .child(main)
            .children(self.select_popover(cx))
            .children(self.panel.as_ref().map(|(panel, _)| {
                div()
                    .id("cloud-credentials-side")
                    .debug_selector(|| "cloud-credentials-side".into())
                    .w(px(PANEL_WIDTH))
                    .flex_shrink_0()
                    .min_h_0()
                    .max_h(px(height))
                    .border_l_1()
                    .border_color(content(0.08))
                    .child(panel.clone())
            }))
    }
}
