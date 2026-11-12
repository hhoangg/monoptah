//! Sign-in and tool checks for the cloud backends, shared by Settings > Cloud and the
//! Codespaces setup dialog: how far the GitHub CLI is set up, whether `sbx` is installed and
//! whether its `sbx_cloud` ssh alias exists. Pure functions of their inputs; callers run
//! them on the background executor.
use monocode_core::{codespaces::AuthStatus, sandboxes};
use std::path::{Path, PathBuf};

pub(crate) const REFRESH_COMMAND: &str = "gh auth refresh -h github.com -s codespace";
pub(crate) const LOGIN_COMMAND: &str = "gh auth login";
pub(crate) const INSTALL_COMMAND: &str = "brew install gh";
/// What fixes a missing ssh alias for Docker Sandbox.
pub(crate) const SBX_SSH_COMMAND: &str = "sbx --cloud setup ssh";

/// How far the GitHub CLI is set up.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Gh {
    Missing,
    NotLoggedIn,
    MissingScope(Option<String>),
    Ready(Option<String>),
    /// gh ran but the check failed.
    Failed(String),
}

impl Gh {
    pub(crate) fn ready(&self) -> bool {
        matches!(self, Gh::Ready(_))
    }

    pub(crate) fn line(&self) -> String {
        match self {
            Gh::Missing => "GitHub CLI (gh) is not installed".into(),
            Gh::NotLoggedIn => "GitHub CLI is not logged in".into(),
            Gh::MissingScope(_) => "GitHub CLI token has no codespace scope".into(),
            Gh::Ready(Some(user)) => format!("GitHub CLI is ready (logged in as {user})"),
            Gh::Ready(None) => "GitHub CLI is ready".into(),
            Gh::Failed(error) => format!("GitHub CLI check failed: {error}"),
        }
    }

    /// The exact command that fixes what is missing.
    pub(crate) fn fix(&self) -> Option<&'static str> {
        match self {
            Gh::Missing => Some(INSTALL_COMMAND),
            Gh::NotLoggedIn => Some(LOGIN_COMMAND),
            Gh::MissingScope(_) => Some(REFRESH_COMMAND),
            Gh::Ready(_) | Gh::Failed(_) => None,
        }
    }
}

pub(crate) fn gh_state(result: Result<AuthStatus, String>) -> Gh {
    match result {
        Err(error) if error.contains("not installed") => Gh::Missing,
        Err(error) => Gh::Failed(error),
        Ok(status) if !status.logged_in => Gh::NotLoggedIn,
        Ok(status) if !status.has_codespace_scope => Gh::MissingScope(status.user),
        Ok(status) => Gh::Ready(status.user),
    }
}

/// What the Docker Sandbox backend needs on this computer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Sbx {
    pub installed: bool,
    pub ssh_set_up: bool,
}

impl Sbx {
    /// `sbx` is on the path and the ssh alias exists. Neither check signs in to Docker.
    pub(crate) fn check(resolve: fn(&str) -> Option<PathBuf>, home: Option<&Path>) -> Self {
        Self {
            installed: resolve("sbx").is_some(),
            ssh_set_up: home.is_some_and(sandboxes::ssh_alias_configured),
        }
    }

    /// The command that fixes what is missing, if the app can run one.
    pub(crate) fn fix(&self) -> Option<&'static str> {
        (self.installed && !self.ssh_set_up).then_some(SBX_SSH_COMMAND)
    }
}
