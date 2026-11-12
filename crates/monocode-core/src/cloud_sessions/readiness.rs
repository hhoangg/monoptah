//! "Ready" checklists of the project's Cloud settings, one per backend. Each item says what
//! was found and, where the app can help, the fix. The functions take their runners as
//! arguments (the caller runs them off the UI thread; tests pass fakes) and hold no secret:
//! a Docker account is only named, its token is read by `docker_accounts` where it is needed.
//!
//! They only read: `gh auth status`, one `sbx ls`, `git ls-remote` and local files. They are
//! meant for the settings dialog and are not called by the start path, which does its own
//! checks.
use crate::{
    codespaces::{self, AuthStatus, GhRunner},
    git_credentials::{self, AccessStatus, GitCredentials, GitRunner},
    sandboxes::{self, Prerequisites, SbxRunner},
};
use std::path::Path;

pub const GH_INSTALL_COMMAND: &str = "brew install gh";
pub const GH_LOGIN_COMMAND: &str = "gh auth login";
pub const GH_REFRESH_COMMAND: &str = "gh auth refresh -h github.com -s codespace";
pub const SBX_SSH_COMMAND: &str = "sbx --cloud setup ssh";
pub const SBX_LOGIN_COMMAND: &str = "sbx login";

/// What one checklist line found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Ok,
    Problem,
    /// Could not be decided now (an earlier item failed, or the answer comes later).
    Unknown,
}

/// What the user can do about an item.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fix {
    /// Run this command in a terminal. For [`SBX_SSH_COMMAND`] the caller runs it as the
    /// project's Docker account.
    Command(&'static str),
    /// The Docker account's token must be replaced in Settings > Cloud.
    ReplaceToken,
}

/// Which line of the checklist an item is. Stable, for tests and the UI's element ids.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Id {
    Gh,
    Origin,
    Devcontainer,
    Sshd,
    DockerAccount,
    Repository,
    Ssh,
}

impl Id {
    pub fn key(self) -> &'static str {
        match self {
            Self::Gh => "gh",
            Self::Origin => "origin",
            Self::Devcontainer => "devcontainer",
            Self::Sshd => "sshd",
            Self::DockerAccount => "docker-account",
            Self::Repository => "repository",
            Self::Ssh => "ssh",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    pub id: Id,
    pub state: State,
    pub text: String,
    pub fix: Option<Fix>,
}

impl Item {
    fn new(id: Id, state: State, text: impl Into<String>, fix: Option<Fix>) -> Self {
        Self {
            id,
            state,
            text: text.into(),
            fix,
        }
    }
    fn ok(id: Id, text: impl Into<String>) -> Self {
        Self::new(id, State::Ok, text, None)
    }
    fn problem(id: Id, text: impl Into<String>, fix: Option<Fix>) -> Self {
        Self::new(id, State::Problem, text, fix)
    }
    fn unknown(id: Id, text: impl Into<String>) -> Self {
        Self::new(id, State::Unknown, text, None)
    }
}

/// True when every item is [`State::Ok`].
pub fn all_ok(items: &[Item]) -> bool {
    items.iter().all(|item| item.state == State::Ok)
}

fn host_of(url: &str) -> Option<String> {
    let url = url.trim();
    let host = match url.split_once("://") {
        Some((_, rest)) => rest.split('/').next()?.rsplit('@').next()?,
        None => url.split_once(':')?.0.rsplit('@').next()?,
    };
    let host = host.split(':').next()?.to_ascii_lowercase();
    (!host.is_empty()).then_some(host)
}

/// Whether two remote URLs name the same repository (same host, owner and name), whatever
/// their form (https, ssh, scp-like, with or without `.git`).
pub fn same_repository(a: &str, b: &str) -> bool {
    match (
        host_of(a),
        codespaces::repo_from_remote(a),
        host_of(b),
        codespaces::repo_from_remote(b),
    ) {
        (Some(ha), Some(ra), Some(hb), Some(rb)) => ha == hb && ra.eq_ignore_ascii_case(&rb),
        _ => false,
    }
}

fn gh_item(status: Result<AuthStatus, String>) -> Item {
    let (state, text, fix) = match status {
        Err(error) if error.contains("not installed") => (
            State::Problem,
            "GitHub CLI (gh) is not installed".to_owned(),
            Some(Fix::Command(GH_INSTALL_COMMAND)),
        ),
        Err(error) => (
            State::Problem,
            format!("GitHub CLI check failed: {error}"),
            None,
        ),
        Ok(status) if !status.logged_in => (
            State::Problem,
            "gh is not signed in".to_owned(),
            Some(Fix::Command(GH_LOGIN_COMMAND)),
        ),
        Ok(status) if !status.has_codespace_scope => (
            State::Problem,
            "gh token has no codespace scope".to_owned(),
            Some(Fix::Command(GH_REFRESH_COMMAND)),
        ),
        Ok(status) => (
            State::Ok,
            match status.user {
                Some(user) => format!("gh signed in as {user}"),
                None => "gh signed in".to_owned(),
            },
            None,
        ),
    };
    Item::new(Id::Gh, state, text, fix)
}

/// The devcontainer lines (found, and optionally sshd) from the project's checkout.
fn devcontainer_items(project_root: &Path, path: &str, with_sshd: bool) -> Vec<Item> {
    let info = super::project_cloud::devcontainer_info(project_root, path);
    let mut items = Vec::new();
    match &info.found_path {
        Some(found) => items.push(Item::ok(
            Id::Devcontainer,
            format!("devcontainer found ({found})"),
        )),
        None => items.push(Item::problem(
            Id::Devcontainer,
            "no devcontainer file in the project folder",
            None,
        )),
    }
    if with_sshd {
        items.push(if info.found_path.is_none() {
            Item::unknown(Id::Sshd, "sshd feature: needs a devcontainer")
        } else if info.has_sshd {
            Item::ok(Id::Sshd, "sshd feature present")
        } else {
            Item::problem(
                Id::Sshd,
                "sshd feature missing from the devcontainer (the app reaches a codespace over ssh)",
                None,
            )
        });
    }
    items
}

/// The Codespaces checklist: gh signed in with the codespace scope, origin on GitHub, the
/// devcontainer found, and the sshd feature in it. `origin` is the project's `origin` URL
/// when it has one; `devcontainer_path` is empty for the repository default.
pub fn codespace(
    gh: &dyn GhRunner,
    project_root: &Path,
    origin: Option<&str>,
    devcontainer_path: &str,
) -> Vec<Item> {
    let mut items = vec![gh_item(codespaces::auth_status(gh))];
    items.push(match origin {
        Some(url) if host_of(url).as_deref() == Some("github.com") => {
            match codespaces::repo_from_remote(url) {
                Some(repo) => Item::ok(Id::Origin, format!("origin on GitHub ({repo})")),
                None => Item::problem(Id::Origin, "origin is not a GitHub repository URL", None),
            }
        }
        Some(_) => Item::problem(Id::Origin, "origin is not on github.com", None),
        None => Item::problem(Id::Origin, "the project has no git origin", None),
    });
    items.extend(devcontainer_items(project_root, devcontainer_path, true));
    items
}

/// What the Docker Sandbox checklist asks about.
pub struct SandboxQuery<'a> {
    /// The project folder on this computer.
    pub project_root: &'a Path,
    pub origin: Option<&'a str>,
    pub home: Option<&'a Path>,
    /// The Docker account the project will use; `None` is the machine's `sbx login`.
    pub docker_account: Option<&'a str>,
    pub repo_url: &'a str,
    /// The saved Git credential the sandbox clones with; `None` for a public repository.
    pub credential_id: Option<&'a str>,
    pub devcontainer_path: &'a str,
}

/// The Docker Sandbox checklist: Docker account valid, repository reachable with the chosen
/// credential, devcontainer found, ssh set up. The Docker account line is the machine
/// sign-in when no account is named. The repository is remote, so the devcontainer is read
/// from the local checkout only when its origin is the same repository; otherwise it is
/// checked when the sandbox starts.
pub fn sandbox(
    sbx: &dyn SbxRunner,
    git: &dyn GitRunner,
    credentials: Option<&GitCredentials>,
    query: &SandboxQuery<'_>,
) -> Vec<Item> {
    let prerequisites = match query.home {
        Some(home) => sandboxes::diagnose_as(sbx, home, query.docker_account),
        None => Prerequisites::Unreachable("The home directory is unknown.".into()),
    };
    let mut items = vec![account_item(&prerequisites, query.docker_account)];
    items.push(repository_item(git, credentials, query));
    items.push(match query.origin {
        Some(origin) if same_repository(origin, query.repo_url) => {
            devcontainer_items(query.project_root, query.devcontainer_path, false).remove(0)
        }
        _ => Item::unknown(
            Id::Devcontainer,
            "devcontainer: checked when the sandbox starts",
        ),
    });
    items.push(ssh_item(&prerequisites));
    items
}

fn account_item(prerequisites: &Prerequisites, account: Option<&str>) -> Item {
    let name = if account.is_some() {
        "Docker account"
    } else {
        "Docker sign-in (sbx login)"
    };
    match prerequisites {
        Prerequisites::SbxMissing => {
            Item::problem(Id::DockerAccount, prerequisites.message(), None)
        }
        Prerequisites::NotLoggedIn(_) => Item::problem(
            Id::DockerAccount,
            "Docker is not signed in on this computer",
            Some(Fix::Command(SBX_LOGIN_COMMAND)),
        ),
        Prerequisites::DockerAccount(why) => {
            Item::problem(Id::DockerAccount, why.clone(), Some(Fix::ReplaceToken))
        }
        Prerequisites::Unreachable(why) => Item::problem(
            Id::DockerAccount,
            format!("{name}: Docker Sandboxes is not reachable ({why})"),
            None,
        ),
        Prerequisites::SshAliasMissing | Prerequisites::Ready => {
            Item::ok(Id::DockerAccount, format!("{name} works"))
        }
    }
}

fn ssh_item(prerequisites: &Prerequisites) -> Item {
    match prerequisites {
        Prerequisites::Ready => Item::ok(Id::Ssh, "ssh set up"),
        Prerequisites::SshAliasMissing => Item::problem(
            Id::Ssh,
            "ssh not set up",
            Some(Fix::Command(SBX_SSH_COMMAND)),
        ),
        _ => Item::unknown(Id::Ssh, "ssh: checked once the Docker account works"),
    }
}

fn repository_item(
    git: &dyn GitRunner,
    credentials: Option<&GitCredentials>,
    query: &SandboxQuery<'_>,
) -> Item {
    if query.repo_url.trim().is_empty() {
        return Item::problem(Id::Repository, "enter the repository URL", None);
    }
    let check = match (query.credential_id, credentials) {
        (Some(id), Some(credentials)) => credentials.check_access(git, id, query.repo_url),
        (Some(_), None) => Err("The saved Git credentials are not available.".to_owned()),
        (None, _) => git_credentials::check_public(git, query.repo_url),
    };
    match check {
        Ok(check) if check.status == AccessStatus::ReadOk => {
            Item::ok(Id::Repository, "repository reachable")
        }
        Ok(check) => Item::problem(Id::Repository, check.message, None),
        Err(error) => Item::problem(Id::Repository, error, None),
    }
}

#[cfg(test)]
mod tests;
