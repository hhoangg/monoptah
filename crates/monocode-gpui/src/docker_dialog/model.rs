//! What the Docker Sandbox setup dialog knows and decides. The prerequisite check takes its
//! runner as an argument so the dialog can run it on the background executor and tests can
//! use a fake. `save` only writes the project's config to the native store: it never starts
//! `sbx`, `git` or any cloud call.
use monocode_core::{
    cloud_sessions::docker_config::{self, DockerProjectConfig},
    git_credentials::{self, GitCredentialSummary},
    sandboxes::{self, Prerequisites, SbxRunner},
    store::Store,
};
use std::path::Path;

pub(super) const SAVE_NOTE: &str =
    "Saving configures this project only. A sandbox is created when you click Create and open.";
pub(super) const DEVCONTAINER_HINT: &str = "Relative to repository root. If the repository has none, the session runs directly in the sandbox.";
pub(super) const ACCOUNT_NOTE: &str = "Fixed per sandbox: a sandbox keeps the account it was created with. Accounts use a setup-token or an API key.";
pub(super) const NO_ACCOUNT_LINE: &str =
    "No Claude account with a setup-token or API key. Add one in Settings, Providers.";
pub(super) const CREDENTIAL_NOTE: &str = "Saved on this computer, reusable across projects.";
pub(super) const REPO_HINT: &str = "HTTPS, GitHub or GitLab.com.";

/// Whether this machine can create and open cloud sandboxes, as the dialog shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Access {
    Checking,
    Done(Prerequisites),
}

impl Access {
    pub(super) fn is_ready(&self) -> bool {
        matches!(self, Access::Done(p) if p.is_ready())
    }

    pub(super) fn line(&self) -> String {
        match self {
            Access::Checking => "Checking Docker cloud access…".into(),
            Access::Done(Prerequisites::Ready) => {
                "Ready: sbx installed, signed in, SSH configured".into()
            }
            Access::Done(other) => other.message(),
        }
    }

    /// The exact command the user should run themselves. The app never runs it.
    pub(super) fn fix(&self) -> Option<&'static str> {
        match self {
            Access::Done(p) => p.fix_command(),
            Access::Checking => None,
        }
    }
}

/// Runs the read-only prerequisite check (`sbx ls` plus a look at the SSH config).
pub(super) fn check_access(runner: &dyn SbxRunner, home: Option<&Path>) -> Prerequisites {
    match home {
        Some(home) => sandboxes::diagnose(runner, home),
        None => Prerequisites::Unreachable("The home directory is unknown.".into()),
    }
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct ComputeChoice {
    pub label: &'static str,
    pub cpus: u32,
    pub memory: &'static str,
}

pub(super) const COMPUTE: [ComputeChoice; 2] = [
    ComputeChoice {
        label: "2 vCPU / 4 GiB",
        cpus: 2,
        memory: "4g",
    },
    ComputeChoice {
        label: "4 vCPU / 8 GiB",
        cpus: 4,
        memory: "8g",
    },
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct RuntimeChoice {
    pub label: &'static str,
    pub ttl: &'static str,
}

pub(super) const RUNTIME: [RuntimeChoice; 3] = [
    RuntimeChoice {
        label: "2 hours then stop",
        ttl: "2h",
    },
    RuntimeChoice {
        label: "1 hour then stop",
        ttl: "1h",
    },
    RuntimeChoice {
        label: "4 hours then stop",
        ttl: "4h",
    },
];

/// A Claude account the sandbox can use: one that has a setup-token or an API key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AccountChoice {
    pub id: String,
    pub label: String,
    /// `setup-token` or `API key`.
    pub kind: &'static str,
}

/// The dialog's fields, as text and indexes. No secret is ever part of it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Form {
    pub repo_url: String,
    pub credential_id: Option<String>,
    pub devcontainer: String,
    pub base_ref: String,
    pub compute: usize,
    pub runtime: usize,
    pub account: Option<String>,
}

impl Form {
    /// A saved config when there is one, otherwise the defaults and the prefilled URL.
    pub(super) fn initial(saved: Option<&DockerProjectConfig>, origin: Option<&str>) -> Self {
        let Some(config) = saved else {
            return Self {
                repo_url: prefill_repo(origin),
                credential_id: None,
                devcontainer: docker_config::DEFAULT_DEVCONTAINER.into(),
                base_ref: String::new(),
                compute: 0,
                runtime: 0,
                account: None,
            };
        };
        Self {
            repo_url: config.repo_url.clone(),
            credential_id: config.credential_id.clone(),
            devcontainer: config.devcontainer_path.clone(),
            base_ref: config.base_ref.clone(),
            compute: COMPUTE
                .iter()
                .position(|c| c.cpus == config.cpus && c.memory == config.memory)
                .unwrap_or(0),
            runtime: RUNTIME
                .iter()
                .position(|r| r.ttl == config.ttl)
                .unwrap_or(0),
            account: config.claude_account.clone(),
        }
    }

    /// The config to validate and store. The platform is left to the server.
    pub(super) fn to_config(&self, saved: Option<&DockerProjectConfig>) -> DockerProjectConfig {
        let compute = COMPUTE[self.compute.min(COMPUTE.len() - 1)];
        let runtime = RUNTIME[self.runtime.min(RUNTIME.len() - 1)];
        DockerProjectConfig {
            repo_url: self.repo_url.clone(),
            credential_id: self.credential_id.clone(),
            base_ref: self.base_ref.clone(),
            devcontainer_path: self.devcontainer.clone(),
            cpus: compute.cpus,
            memory: compute.memory.into(),
            ttl: runtime.ttl.into(),
            platform: saved.and_then(|c| c.platform.clone()),
            claude_account: self.account.clone(),
            // This dialog does not edit the Docker account; keep the saved one.
            docker_account: saved.and_then(|c| c.docker_account.clone()),
        }
    }
}

/// The field an error belongs under.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Field {
    Repo,
    Credential,
    Devcontainer,
    Base,
    General,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct FieldError {
    pub field: Field,
    pub message: String,
}

fn error(field: Field, message: impl Into<String>) -> FieldError {
    FieldError {
        field,
        message: message.into(),
    }
}

/// Which field a validation message from `DockerProjectConfig::validated` is about.
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
    } else if lower.contains("repository") || lower.contains("https") || lower.contains("url") {
        Field::Repo
    } else {
        Field::General
    }
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

/// Everything wrong with the form that can be told without the store, then the config
/// validation itself.
pub(super) fn validate(
    form: &Form,
    credentials: &[GitCredentialSummary],
    accounts: &[AccountChoice],
    saved: Option<&DockerProjectConfig>,
) -> Result<DockerProjectConfig, FieldError> {
    let config = form.to_config(saved);
    let config = config
        .validated()
        .map_err(|message| error(field_of(&message), message))?;
    if let Some(id) = &config.credential_id {
        let Some(credential) = credentials.iter().find(|c| &c.id == id) else {
            return Err(error(
                Field::Credential,
                "That Git credential no longer exists. Choose another one.",
            ));
        };
        let platform = git_credentials::normalize_origin(&config.repo_url).map(|(p, _, _)| p);
        if platform.as_ref() != Ok(&credential.platform) {
            return Err(error(
                Field::Credential,
                format!(
                    "This credential is for {}, not the repository's host.",
                    credential.platform.host()
                ),
            ));
        }
    }
    if let Some(id) = &config.claude_account
        && !accounts.iter().any(|a| &a.id == id)
    {
        return Err(error(
            Field::General,
            "That Claude account is no longer available. Choose another one.",
        ));
    }
    Ok(config)
}

/// Validates and stores the project's config. Nothing else happens.
pub(super) fn save(
    store: &Store,
    project: &str,
    form: &Form,
    credentials: &[GitCredentialSummary],
    accounts: &[AccountChoice],
    saved: Option<&DockerProjectConfig>,
) -> Result<DockerProjectConfig, FieldError> {
    let config = validate(form, credentials, accounts, saved)?;
    docker_config::save(store, project, &config).map_err(|message| error(Field::General, message))
}
