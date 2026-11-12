//! What the project's Cloud settings dialog knows and decides: the form of both backends,
//! the option lists of its selects (a saved value that is not a preset stays selectable),
//! the warnings, the summary line and the validation that maps core errors to fields. Pure
//! functions of their inputs: `save` is the only one that touches the store, and it never
//! runs `gh`, `sbx` or `git`.
use monocode_core::{
    cloud_sessions::{
        docker_config::{self, DockerProjectConfig},
        project_cloud::{
            self, CodespaceProjectConfig, DefaultBackend, DevcontainerInfo, ProjectCloud,
        },
    },
    codespaces::{self, MachineType},
    docker_accounts::{DockerAccountStatus, DockerAccountSummary},
    git_credentials::{self, GitCredentialSummary},
    store::Store,
};
use std::path::Path;

pub(super) const CLAUDE_NOTE: &str = "Applies from the next start or reconnect. Running sessions keep the account they started with.";
pub(super) const DOCKER_ACCOUNT_NOTE: &str =
    "Creates and runs this project's sandboxes. Add accounts in Settings > Cloud.";
pub(super) const GIT_CODESPACE_LINE: &str = "GitHub token built into the codespace (git and gh)";
pub(super) const GIT_SANDBOX_NOTE: &str =
    "Also signs in gh for pull requests. The token stays in memory.";
pub(super) const NO_ACCOUNT_LINE: &str =
    "No Claude account with a setup-token or API key. Add one in Settings, Providers.";
pub(super) const REPO_HINT: &str = "HTTPS, GitHub or GitLab.com.";
pub(super) const CODESPACE_BASE_HINT: &str =
    "Empty means the current branch of the project folder.";
pub(super) const SANDBOX_BASE_HINT: &str = "Empty means the repository's default branch.";
pub(super) const CODESPACE_SECRET_LABEL: &str = "Codespaces secret (CLAUDE_CODE_OAUTH_TOKEN)";

/// A Claude account a cloud session can use: one that has a setup-token or an API key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AccountChoice {
    pub id: String,
    pub label: String,
    /// `setup-token` or `API key`.
    pub kind: &'static str,
}

/// The repository URL to prefill: the project's `origin` when it is an HTTPS URL on a host
/// the backend supports. An SSH origin is never converted.
pub(super) fn prefill_repo(origin: Option<&str>) -> String {
    origin
        .map(str::trim)
        .filter(|url| git_credentials::normalize_origin(url).is_ok())
        .map(str::to_owned)
        .unwrap_or_default()
}

/// The saved credentials that can read `repo_url`: the ones for its host, or all of them
/// while the URL is not valid yet.
pub(super) fn matching_credentials(
    all: &[GitCredentialSummary],
    repo_url: &str,
) -> Vec<GitCredentialSummary> {
    match git_credentials::normalize_origin(repo_url) {
        Ok((platform, _, _)) => all
            .iter()
            .filter(|c| c.platform == platform)
            .cloned()
            .collect(),
        Err(_) => all.to_vec(),
    }
}

/// `path` with the home folder written as `~`.
pub(super) fn display_path(path: &str, home: Option<&Path>) -> String {
    if let Some(home) = home
        && let Ok(rest) = Path::new(path).strip_prefix(home)
    {
        let rest = rest.to_string_lossy();
        return if rest.is_empty() {
            "~".to_owned()
        } else {
            format!("~/{rest}")
        };
    }
    path.to_owned()
}

// ------------------------------------------------------------------ form

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct CodespaceForm {
    pub base_ref: String,
    pub devcontainer: String,
    pub machine: Option<String>,
    pub idle: u32,
    pub claude: Option<String>,
}

impl CodespaceForm {
    pub(super) fn from_config(config: &CodespaceProjectConfig) -> Self {
        Self {
            base_ref: config.base_ref.clone(),
            devcontainer: config.devcontainer_path.clone(),
            machine: config.machine.clone(),
            idle: config.idle_timeout_minutes,
            claude: config.claude_account.clone(),
        }
    }

    pub(super) fn to_config(&self) -> CodespaceProjectConfig {
        CodespaceProjectConfig {
            base_ref: self.base_ref.clone(),
            devcontainer_path: self.devcontainer.clone(),
            machine: self.machine.clone(),
            idle_timeout_minutes: self.idle,
            claude_account: self.claude.clone(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct SandboxForm {
    pub repo_url: String,
    pub credential_id: Option<String>,
    pub devcontainer: String,
    pub base_ref: String,
    pub cpus: u32,
    pub memory: String,
    pub ttl: String,
    pub claude: Option<String>,
    pub docker_account: Option<String>,
    /// Not edited here; kept so a save writes back what was stored.
    pub platform: Option<String>,
}

impl SandboxForm {
    /// A saved config when there is one, otherwise the defaults and the prefilled URL.
    pub(super) fn initial(saved: Option<&DockerProjectConfig>, origin: Option<&str>) -> Self {
        match saved {
            Some(config) => Self {
                repo_url: config.repo_url.clone(),
                credential_id: config.credential_id.clone(),
                devcontainer: config.devcontainer_path.clone(),
                base_ref: config.base_ref.clone(),
                cpus: config.cpus,
                memory: config.memory.clone(),
                ttl: config.ttl.clone(),
                claude: config.claude_account.clone(),
                docker_account: config.docker_account.clone(),
                platform: config.platform.clone(),
            },
            None => Self {
                repo_url: prefill_repo(origin),
                credential_id: None,
                devcontainer: docker_config::DEFAULT_DEVCONTAINER.into(),
                base_ref: String::new(),
                cpus: docker_config::DEFAULT_CPUS,
                memory: docker_config::DEFAULT_MEMORY.into(),
                ttl: docker_config::DEFAULT_TTL.into(),
                claude: None,
                docker_account: None,
                platform: None,
            },
        }
    }

    pub(super) fn to_config(&self) -> DockerProjectConfig {
        DockerProjectConfig {
            repo_url: self.repo_url.clone(),
            credential_id: self.credential_id.clone(),
            base_ref: self.base_ref.clone(),
            devcontainer_path: self.devcontainer.clone(),
            cpus: self.cpus,
            memory: self.memory.clone(),
            ttl: self.ttl.clone(),
            platform: self.platform.clone(),
            claude_account: self.claude.clone(),
            docker_account: self.docker_account.clone(),
        }
    }
}

/// Both backends' fields. No secret is ever part of it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Form {
    /// The user's choice; `None` while never chosen.
    pub default_backend: Option<DefaultBackend>,
    pub codespace: CodespaceForm,
    pub sandbox: SandboxForm,
}

impl Form {
    pub(super) fn initial(saved: &ProjectCloud, origin: Option<&str>) -> Self {
        Self {
            default_backend: saved.default_backend,
            codespace: CodespaceForm::from_config(&saved.codespace),
            sandbox: SandboxForm::initial(saved.sandbox.as_ref(), origin),
        }
    }
}

// ------------------------------------------------------------------ errors

/// The field an error belongs under.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Field {
    Repo,
    Credential,
    Devcontainer,
    Base,
    Machine,
    Idle,
    Runtime,
    Size,
    DockerAccount,
    Claude,
    General,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct FieldError {
    pub backend: DefaultBackend,
    pub field: Field,
    pub message: String,
}

fn error(backend: DefaultBackend, field: Field, message: impl Into<String>) -> FieldError {
    FieldError {
        backend,
        field,
        message: message.into(),
    }
}

/// Which field a validation message from the core is about.
fn field_of(message: &str) -> Field {
    let lower = message.to_lowercase();
    if lower.contains("repository url") {
        // Includes "Remove the credentials from the repository URL".
        Field::Repo
    } else if lower.contains("devcontainer") {
        Field::Devcontainer
    } else if lower.contains("base branch") {
        Field::Base
    } else if lower.contains("git credential") {
        Field::Credential
    } else if lower.contains("machine") {
        Field::Machine
    } else if lower.contains("stop when idle") {
        Field::Idle
    } else if lower.contains("lifetime") {
        Field::Runtime
    } else if lower.contains("cpus") || lower.contains("memory") {
        Field::Size
    } else if lower.contains("docker account") {
        Field::DockerAccount
    } else if lower.contains("claude account") {
        Field::Claude
    } else if lower.contains("repository") || lower.contains("https") || lower.contains("url") {
        Field::Repo
    } else {
        Field::General
    }
}

/// What a save checks the form against, as far as it is known.
pub(super) struct Context<'a> {
    pub credentials: &'a [GitCredentialSummary],
    /// Whether the credential list was read: until then a saved id cannot be checked.
    pub credentials_loaded: bool,
    pub claude_accounts: &'a [AccountChoice],
    /// The Docker accounts, `None` while they were not read.
    pub docker_accounts: Option<&'a [DockerAccountSummary]>,
    /// A sandbox config is stored for the project.
    pub sandbox_saved: bool,
    /// The sandbox form as it was offered; a form still equal to it is untouched.
    pub sandbox_baseline: &'a SandboxForm,
    /// The caller asked for the sandbox setup (the composer's Docker Sandbox row).
    pub sandbox_wanted: bool,
}

impl Context<'_> {
    /// A sandbox section that was never configured and is left as offered writes nothing,
    /// so a project stays "not configured" until the user does something there.
    fn include_sandbox(&self, form: &Form) -> bool {
        self.sandbox_saved
            || self.sandbox_wanted
            || form.default_backend == Some(DefaultBackend::Sandbox)
            || form.sandbox != *self.sandbox_baseline
    }
}

/// Everything wrong with the form, then the core validation itself. The result is what
/// [`project_cloud::save`] stores.
pub(super) fn validate(form: &Form, ctx: &Context<'_>) -> Result<ProjectCloud, FieldError> {
    let cs = DefaultBackend::Codespace;
    let codespace = form
        .codespace
        .to_config()
        .validated()
        .map_err(|message| error(cs, field_of(&message), message))?;
    if let Some(id) = &codespace.claude_account
        && !ctx.claude_accounts.iter().any(|a| &a.id == id)
    {
        return Err(error(
            cs,
            Field::Claude,
            "That Claude account is no longer available. Choose another one.",
        ));
    }
    let sandbox = if ctx.include_sandbox(form) {
        Some(validate_sandbox(&form.sandbox, ctx)?)
    } else {
        None
    };
    Ok(ProjectCloud {
        default_backend: form.default_backend,
        codespace,
        sandbox,
    })
}

fn validate_sandbox(
    form: &SandboxForm,
    ctx: &Context<'_>,
) -> Result<DockerProjectConfig, FieldError> {
    let sb = DefaultBackend::Sandbox;
    let config = form
        .to_config()
        .validated()
        .map_err(|message| error(sb, field_of(&message), message))?;
    if let Some(id) = &config.credential_id
        && ctx.credentials_loaded
    {
        let Some(credential) = ctx.credentials.iter().find(|c| &c.id == id) else {
            return Err(error(
                sb,
                Field::Credential,
                "That Git credential no longer exists. Choose another one.",
            ));
        };
        let platform = git_credentials::normalize_origin(&config.repo_url).map(|(p, _, _)| p);
        if platform.as_ref() != Ok(&credential.platform) {
            return Err(error(
                sb,
                Field::Credential,
                format!(
                    "This credential is for {}, not the repository's host.",
                    credential.platform.host()
                ),
            ));
        }
    }
    if let Some(id) = &config.claude_account
        && !ctx.claude_accounts.iter().any(|a| &a.id == id)
    {
        return Err(error(
            sb,
            Field::Claude,
            "That Claude account is no longer available. Choose another one.",
        ));
    }
    if let (Some(id), Some(accounts)) = (&config.docker_account, ctx.docker_accounts)
        && !accounts.iter().any(|a| &a.id == id)
    {
        return Err(error(
            sb,
            Field::DockerAccount,
            "That Docker account no longer exists. Choose another one.",
        ));
    }
    Ok(config)
}

/// Validates and stores the project's cloud settings. Nothing else happens.
pub(super) fn save(
    store: &Store,
    project: &str,
    form: &Form,
    ctx: &Context<'_>,
) -> Result<ProjectCloud, FieldError> {
    let validated = validate(form, ctx)?;
    project_cloud::save(store, project, &validated)
        .map_err(|message| error(DefaultBackend::Codespace, Field::General, message))
}

// ------------------------------------------------------------------ options

/// The sizes the cloud offers, `(cpus, memory)`.
pub(super) const SIZES: [(u32, &str); 5] =
    [(1, "2g"), (2, "4g"), (4, "8g"), (8, "16g"), (16, "32g")];
pub(super) const IDLE_MINUTES: [u32; 4] = [30, 60, 120, 240];
pub(super) const RUNTIMES: [&str; 4] = ["1h", "2h", "4h", "8h"];

/// `4g` or `512m` as MiB.
pub(super) fn memory_mib(memory: &str) -> Option<u64> {
    let memory = memory.trim().to_ascii_lowercase();
    if let Some(g) = memory.strip_suffix('g') {
        return g.parse::<u64>().ok().map(|g| g * 1024);
    }
    memory.strip_suffix('m')?.parse().ok()
}

/// `16384` MiB as `16 GB`.
fn gb_text(mib: u64) -> String {
    if mib >= 1024 && mib.is_multiple_of(1024) {
        format!("{} GB", mib / 1024)
    } else if mib >= 1024 {
        format!("{:.1} GB", mib as f64 / 1024.0)
    } else {
        format!("{mib} MB")
    }
}

fn plural(count: u32, unit: &str) -> String {
    if count == 1 {
        format!("{count} {unit}")
    } else {
        format!("{count} {unit}s")
    }
}

pub(super) fn size_label(cpus: u32, memory: &str) -> String {
    let memory = memory_mib(memory).map_or_else(|| memory.to_owned(), gb_text);
    format!("{}, {memory}", plural(cpus, "CPU"))
}

pub(super) fn idle_label(minutes: u32) -> String {
    match minutes {
        m if m >= 60 && m % 60 == 0 => plural(m / 60, "hour"),
        m => plural(m, "minute"),
    }
}

pub(super) fn runtime_label(ttl: &str) -> String {
    let long = match ttl.strip_suffix('h').and_then(|n| n.parse::<u32>().ok()) {
        Some(hours) => plural(hours, "hour"),
        None => match ttl.strip_suffix('m').and_then(|n| n.parse::<u32>().ok()) {
            Some(minutes) => plural(minutes, "minute"),
            None => ttl.to_owned(),
        },
    };
    format!("{long}, then stop")
}

/// The sizes to offer: the presets, plus the current value when it is none of them.
pub(super) fn size_options(cpus: u32, memory: &str) -> Vec<(u32, String)> {
    let mut options: Vec<(u32, String)> =
        SIZES.iter().map(|(c, m)| (*c, (*m).to_owned())).collect();
    if !options.iter().any(|(c, m)| *c == cpus && m == memory) {
        options.push((cpus, memory.to_owned()));
    }
    options
}

pub(super) fn idle_options(current: u32) -> Vec<u32> {
    let mut options = IDLE_MINUTES.to_vec();
    if !options.contains(&current) {
        options.push(current);
    }
    options
}

pub(super) fn runtime_options(current: &str) -> Vec<String> {
    let mut options: Vec<String> = RUNTIMES.iter().map(|r| (*r).to_owned()).collect();
    if !options.iter().any(|r| r == current) {
        options.push(current.to_owned());
    }
    options
}

/// "Asks for 4 CPUs and 16 GB" from the devcontainer's `hostRequirements`.
pub(super) fn requirement_hint(info: &DevcontainerInfo) -> Option<String> {
    let memory = info.host_memory_mib.map(gb_text);
    match (info.host_cpus, memory) {
        (Some(cpus), Some(memory)) => {
            Some(format!("Asks for {} and {memory}", plural(cpus, "CPU")))
        }
        (Some(cpus), None) => Some(format!("Asks for {}", plural(cpus, "CPU"))),
        (None, Some(memory)) => Some(format!("Asks for {memory}")),
        (None, None) => None,
    }
}

/// A warning when a size has fewer CPUs or less memory than the devcontainer asks for. The
/// size stays selectable.
pub(super) fn size_warning(
    cpus: u32,
    memory_mib: Option<u64>,
    info: &DevcontainerInfo,
) -> Option<String> {
    let low_cpus = info.host_cpus.is_some_and(|need| cpus < need);
    let low_memory =
        matches!((info.host_memory_mib, memory_mib), (Some(need), Some(have)) if have < need);
    if !(low_cpus || low_memory) {
        return None;
    }
    let ask = requirement_hint(info)?.replacen("Asks for", "asks for", 1);
    Some(format!(
        "Below what the devcontainer {ask}. The environment may not build or run."
    ))
}

/// One machine type as the select shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct MachineOption {
    pub name: String,
    pub label: String,
}

fn meets(machine: &MachineType, info: &DevcontainerInfo) -> bool {
    let cpus = info.host_cpus.is_none_or(|need| machine.cpus >= need);
    let memory = info
        .host_memory_mib
        .is_none_or(|need| machine.memory_in_bytes / (1024 * 1024) >= need);
    cpus && memory
}

/// The machine GitHub would pick for the devcontainer's `hostRequirements`: the smallest
/// one that meets them. `None` when the file asks for nothing or no machine fits.
pub(super) fn devcontainer_machine<'a>(
    machines: &'a [MachineType],
    info: &DevcontainerInfo,
) -> Option<&'a MachineType> {
    if info.host_cpus.is_none() && info.host_memory_mib.is_none() {
        return None;
    }
    machines
        .iter()
        .filter(|m| meets(m, info))
        .min_by_key(|m| (m.cpus, m.memory_in_bytes))
}

pub(super) fn machine_label(machine: &MachineType) -> String {
    if machine.display_name.is_empty() {
        machine.name.clone()
    } else {
        machine.display_name.clone()
    }
}

/// The machine select's options: every machine the repository offers (the one the
/// devcontainer asks for marked), plus the current value when the list lacks it.
pub(super) fn machine_options(
    machines: &[MachineType],
    current: Option<&str>,
    info: &DevcontainerInfo,
) -> Vec<MachineOption> {
    let marked = devcontainer_machine(machines, info).map(|m| m.name.clone());
    let mut options: Vec<MachineOption> = machines
        .iter()
        .map(|machine| MachineOption {
            name: machine.name.clone(),
            label: if marked.as_deref() == Some(machine.name.as_str()) {
                format!("{} (devcontainer)", machine_label(machine))
            } else {
                machine_label(machine)
            },
        })
        .collect();
    if let Some(current) = current
        && !options.iter().any(|o| o.name == current)
    {
        options.push(MachineOption {
            name: current.to_owned(),
            label: current.to_owned(),
        });
    }
    options
}

/// The name that is selected: the project's machine, else the one a codespace starts on.
pub(super) fn selected_machine(machines: &[MachineType], current: Option<&str>) -> Option<String> {
    match current {
        Some(name) => Some(name.to_owned()),
        None => codespaces::default_machine(machines, None),
    }
}

/// A Docker account as the select shows it.
pub(super) fn docker_account_label(account: &DockerAccountSummary) -> String {
    match account.status {
        DockerAccountStatus::Revoked => format!("{} (token needs replacing)", account.docker_id),
        DockerAccountStatus::MissingScope => {
            format!("{} (needs Cloud Sandboxes access)", account.docker_id)
        }
        DockerAccountStatus::Valid | DockerAccountStatus::Unchecked => account.docker_id.clone(),
    }
}

/// `New cloud sessions use <backend> . <size or machine> . Claude <x> [. Docker <id>] .
/// Claude setup on|off`, the dots as middle dots.
pub(super) fn summary_line(
    backend: DefaultBackend,
    place: &str,
    claude: &str,
    docker: Option<&str>,
    kit_on: bool,
) -> String {
    let name = match backend {
        DefaultBackend::Codespace => "Codespaces",
        DefaultBackend::Sandbox => "Docker Sandbox",
    };
    let mut parts = vec![format!("New cloud sessions use {name}"), place.to_owned()];
    parts.push(format!("Claude {claude}"));
    if let Some(docker) = docker {
        parts.push(format!("Docker {docker}"));
    }
    parts.push(format!(
        "Claude setup {}",
        if kit_on { "on" } else { "off" }
    ));
    parts.join(" \u{b7} ")
}
