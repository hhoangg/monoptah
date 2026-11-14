//! What the project's Cloud settings dialog knows and decides: the form of both backends,
//! the option lists of its selects (a saved value that is not a preset stays selectable),
//! the warnings and the validation that maps core errors to fields. Pure
//! functions of their inputs: `save` is the only one that touches the store, and it never
//! runs `gh`, `sbx` or `git`.
pub(crate) use crate::cloud_presets::AccountChoice;
pub(super) use crate::cloud_presets::{
    CODESPACE_SECRET_LABEL, docker_account_label, idle_label, idle_options, machine_label,
    memory_mib, runtime_label, runtime_options, size_label, size_options,
};
use monocode_core::{
    cloud_sessions::{
        docker_config::{self, DockerProjectConfig},
        project_cloud::{
            self, CodespaceProjectConfig, DefaultBackend, DevcontainerInfo, ProjectCloud,
        },
    },
    codespaces::{self, MachineType},
    docker_accounts::DockerAccountSummary,
    git_credentials::{self, GitCredentialSummary},
    sandboxes::SandboxSource,
    store::Store,
};
use std::path::Path;

/// The single line under the Credentials group: the account pickers apply later.
pub(super) const ACCOUNT_CHANGE_NOTE: &str =
    "Changing an account applies from the next start or reconnect; running sessions keep theirs.";
pub(super) const ENV_TEMPLATE_HINT: &str = "The template must already exist in the Docker cloud registry (sbx --cloud template ls). Monoptah does not upload it, and its size is fixed.";
pub(super) const ENV_IMAGE_HINT: &str = "A public OCI image, for example docker.io/library/rust:slim. It needs sh, git and curl; Monoptah does not build or publish it.";
pub(super) const ENV_DEVCONTAINER_LABEL: &str = "Also build the repository's dev container";
pub(super) const ENV_DEVCONTAINER_NOTE: &str = "Off by default: the toolchain comes from the template or image, so the session runs directly in the sandbox.";
pub(super) const GIT_CODESPACE_LINE: &str = "GitHub token built into the codespace (git and gh)";

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

/// Where the sandbox starts from, as the Environment row offers it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum EnvKind {
    #[default]
    Default,
    Template,
    Image,
}

impl EnvKind {
    pub(super) const ALL: [EnvKind; 3] = [Self::Default, Self::Template, Self::Image];

    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Default => "Default",
            Self::Template => "Template",
            Self::Image => "Public image",
        }
    }

    pub(super) fn hint(self) -> Option<&'static str> {
        match self {
            Self::Default => None,
            Self::Template => Some(ENV_TEMPLATE_HINT),
            Self::Image => Some(ENV_IMAGE_HINT),
        }
    }

    pub(super) fn placeholder(self) -> &'static str {
        match self {
            Self::Default => "",
            Self::Template => "Template name",
            Self::Image => "docker.io/library/rust:slim",
        }
    }

    fn of(source: &SandboxSource) -> Self {
        match source {
            SandboxSource::Default => Self::Default,
            SandboxSource::Template { .. } => Self::Template,
            SandboxSource::Image { .. } => Self::Image,
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
    pub env_kind: EnvKind,
    /// The template name or image reference, as typed; unused for the default sandbox.
    pub env_text: String,
    /// Whether the repository's dev container is built in a template or image sandbox.
    /// Kept as the stored `Option` so a save writes back what was stored.
    pub use_devcontainer: Option<bool>,
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
                env_kind: EnvKind::of(&config.environment),
                env_text: config.environment.value().unwrap_or_default().to_owned(),
                use_devcontainer: config.use_devcontainer,
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
                env_kind: EnvKind::Default,
                env_text: String::new(),
                use_devcontainer: None,
            },
        }
    }

    /// The environment the form describes.
    pub(super) fn environment(&self) -> SandboxSource {
        match self.env_kind {
            EnvKind::Default => SandboxSource::Default,
            EnvKind::Template => SandboxSource::Template {
                name: self.env_text.clone(),
            },
            EnvKind::Image => SandboxSource::Image {
                reference: self.env_text.clone(),
            },
        }
    }

    /// True while the template fixes the size, so the Size row has nothing to choose.
    pub(super) fn size_from_template(&self) -> bool {
        self.env_kind == EnvKind::Template
    }

    /// The size to store. A template's size is never checked by the core, so a stored one
    /// that is out of range (an old or hand-edited file) is replaced by the defaults. A size
    /// that is in range but not a shape (3 CPUs / 5g) is kept: a Mono API start rounds it up
    /// to a shape when an image override uses it; a composer start refuses it at Send.
    fn stored_size(&self) -> (u32, String) {
        let memory = self.memory.trim().to_lowercase();
        if self.size_from_template()
            && (docker_config::check_cpus(self.cpus).is_err()
                || docker_config::check_memory(&memory).is_err())
        {
            return (
                docker_config::DEFAULT_CPUS,
                docker_config::DEFAULT_MEMORY.into(),
            );
        }
        (self.cpus, self.memory.clone())
    }

    pub(super) fn to_config(&self) -> DockerProjectConfig {
        let (cpus, memory) = self.stored_size();
        DockerProjectConfig {
            repo_url: self.repo_url.clone(),
            credential_id: self.credential_id.clone(),
            base_ref: self.base_ref.clone(),
            devcontainer_path: self.devcontainer.clone(),
            cpus,
            memory,
            ttl: self.ttl.clone(),
            platform: self.platform.clone(),
            claude_account: self.claude.clone(),
            docker_account: self.docker_account.clone(),
            environment: self.environment(),
            use_devcontainer: (self.env_kind != EnvKind::Default)
                .then_some(self.use_devcontainer)
                .flatten(),
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
    Environment,
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
    if lower.contains("template name") || lower.contains("image reference") {
        Field::Environment
    } else if lower.contains("repository url") {
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
    // The default shown in the form is Codespaces until a sandbox is stored. A save that
    // newly stores the sandbox would flip the stored fallback to Docker Sandbox behind the
    // user's back, so the shown value is stored explicitly instead.
    let default_backend = form
        .default_backend
        .or_else(|| (sandbox.is_some() && !ctx.sandbox_saved).then_some(DefaultBackend::Codespace));
    Ok(ProjectCloud {
        default_backend,
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
    if config.credential_id.is_some() && !ctx.credentials_loaded {
        // A saved id cannot be checked yet, and a save would keep or drop it blindly.
        return Err(error(
            sb,
            Field::Credential,
            "Still reading the saved Git credentials. Try again in a moment.",
        ));
    }
    if let Some(id) = &config.credential_id {
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

#[cfg(test)]
pub(super) use crate::cloud_presets::requirement_hint;
pub(super) use crate::cloud_presets::size_warning;

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

/// The machine select's options: every machine the repository allows on the base branch (the
/// one the devcontainer asks for marked). A saved machine the list lacks is not offered:
/// GitHub would refuse it, see [`unavailable_machine`].
pub(super) fn machine_options(
    machines: &[MachineType],
    info: &DevcontainerInfo,
) -> Vec<MachineOption> {
    let marked = devcontainer_machine(machines, info).map(|m| m.name.clone());
    machines
        .iter()
        .map(|machine| MachineOption {
            name: machine.name.clone(),
            label: if marked.as_deref() == Some(machine.name.as_str()) {
                format!("{} (devcontainer)", machine_label(machine))
            } else {
                machine_label(machine)
            },
        })
        .collect()
}

/// The name that is selected: the project's machine while the repository allows it, else the
/// smallest allowed one (the machine a codespace then starts on).
pub(super) fn selected_machine(machines: &[MachineType], current: Option<&str>) -> Option<String> {
    codespaces::default_machine(machines, current)
}

/// The saved machine when the listed machines do not include it (the repository's devcontainer
/// or policy rules it out now): the start then uses [`selected_machine`] instead.
pub(super) fn unavailable_machine<'a>(
    machines: &[MachineType],
    current: Option<&'a str>,
) -> Option<&'a str> {
    current.filter(|name| !machines.is_empty() && !machines.iter().any(|m| m.name == *name))
}
