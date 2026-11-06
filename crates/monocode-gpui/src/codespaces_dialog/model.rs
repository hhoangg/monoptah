//! What the Codespaces setup dialog knows and decides. Every `gh` call here takes the runner
//! as an argument, so the dialog runs them on the background executor and tests use a fake.
//! Secret values only ever pass through `save`, which hands them to `set_secret` (stdin).
use monocode_core::codespaces::{
    self, AuthStatus, DevcontainerSecret, GhRunner, OAUTH_SECRET_KEY, SecretState,
};
use std::path::Path;

pub(super) const CLAUDE_TOKEN_NOTE: &str = "GitHub only injects a changed secret into a codespace when it starts. After changing CLAUDE_CODE_OAUTH_TOKEN, stop and restart the codespace (or create a new one), then reconnect. Whitespace from a wrapped `claude setup-token` paste is removed.";
pub(super) const PRIVACY_NOTE: &str = "Monoptah keeps no copy of these values. They go straight to GitHub as user Codespaces secrets for this repository.";
pub(super) const REFRESH_COMMAND: &str = "gh auth refresh -h github.com -s codespace";
pub(super) const LOGIN_COMMAND: &str = "gh auth login";
pub(super) const NO_REPO_LINE: &str =
    "No GitHub repository found: the project needs a git origin on github.com.";
pub(super) const INSTALL_COMMAND: &str = "brew install gh";

/// How far the GitHub CLI is set up.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Gh {
    Missing,
    NotLoggedIn,
    MissingScope(Option<String>),
    Ready(Option<String>),
    /// gh ran but the check failed.
    Failed(String),
}

impl Gh {
    pub(super) fn ready(&self) -> bool {
        matches!(self, Gh::Ready(_))
    }

    pub(super) fn line(&self) -> String {
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
    pub(super) fn fix(&self) -> Option<&'static str> {
        match self {
            Gh::Missing => Some(INSTALL_COMMAND),
            Gh::NotLoggedIn => Some(LOGIN_COMMAND),
            Gh::MissingScope(_) => Some(REFRESH_COMMAND),
            Gh::Ready(_) | Gh::Failed(_) => None,
        }
    }
}

pub(super) fn gh_state(result: Result<AuthStatus, String>) -> Gh {
    match result {
        Err(error) if error.contains("not installed") => Gh::Missing,
        Err(error) => Gh::Failed(error),
        Ok(status) if !status.logged_in => Gh::NotLoggedIn,
        Ok(status) if !status.has_codespace_scope => Gh::MissingScope(status.user),
        Ok(status) => Gh::Ready(status.user),
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Row {
    pub key: String,
    pub description: String,
    /// None while the status could not be read.
    pub state: Option<SecretState>,
}

impl Row {
    pub(super) fn is_claude(&self) -> bool {
        self.key == OAUTH_SECRET_KEY
    }
}

pub(super) fn state_label(state: Option<SecretState>) -> &'static str {
    match state {
        Some(SecretState::Set) => "Set",
        Some(SecretState::Missing) => "Missing",
        Some(SecretState::NotForRepo) => "Not enabled for this repository",
        None => "Unknown",
    }
}

const CLAUDE_DESCRIPTION: &str =
    "Lets Claude Code sign in inside the codespace. Generate it with `claude setup-token`.";

/// Everything one check found out.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Snapshot {
    pub gh: Gh,
    pub repo: Option<String>,
    pub rows: Vec<Row>,
    /// A secrets read that failed after gh itself was ready.
    pub error: Option<String>,
}

/// The secrets a project asks for, with CLAUDE_CODE_OAUTH_TOKEN always first.
fn wanted(secrets: &[DevcontainerSecret]) -> Vec<(String, String)> {
    let mut out = vec![(OAUTH_SECRET_KEY.to_owned(), CLAUDE_DESCRIPTION.to_owned())];
    for secret in secrets {
        if !out.iter().any(|(key, _)| *key == secret.key) {
            out.push((secret.key.clone(), secret.description.clone()));
        }
    }
    out
}

impl Snapshot {
    /// Why the inputs cannot be used, or None when they can.
    pub(super) fn disabled_reason(&self) -> Option<String> {
        if !self.gh.ready() {
            return Some(self.gh.line());
        }
        if self.repo.is_none() {
            return Some(NO_REPO_LINE.to_owned());
        }
        None
    }

    /// Why a row's status is unknown, or None when it is known.
    pub(super) fn unknown_reason(&self, row: &Row) -> Option<String> {
        if row.state.is_some() {
            return None;
        }
        self.disabled_reason()
            .or_else(|| self.error.clone())
            .or_else(|| Some("The status was not returned by GitHub.".to_owned()))
    }
}

/// Checks gh, resolves the repo from `origin` and reads the status of each secret.
pub(super) fn load(runner: &dyn GhRunner, project: &Path, origin: Option<&str>) -> Snapshot {
    let repo = origin.and_then(codespaces::repo_from_remote);
    let secrets = codespaces::devcontainer_secrets(project);
    let mut rows: Vec<Row> = wanted(&secrets)
        .into_iter()
        .map(|(key, description)| Row {
            key,
            description,
            state: None,
        })
        .collect();
    let gh = gh_state(codespaces::auth_status(runner));
    let mut error = None;
    if gh.ready()
        && let Some(repo) = &repo
    {
        let keys: Vec<String> = rows.iter().map(|row| row.key.clone()).collect();
        match codespaces::secrets_status(runner, repo, &keys) {
            Ok(statuses) => {
                for row in &mut rows {
                    row.state = statuses
                        .iter()
                        .find(|status| status.key == row.key)
                        .map(|status| status.status);
                }
            }
            Err(message) => error = Some(message),
        }
    }
    Snapshot {
        gh,
        repo,
        rows,
        error,
    }
}

/// The fields to save: non-empty after trimming, in form order. Empty fields keep
/// their secret untouched.
pub(super) fn to_save(fields: &[(String, String)]) -> Vec<(String, String)> {
    fields
        .iter()
        .filter_map(|(key, value)| {
            let value = value.trim();
            (!value.is_empty()).then(|| (key.clone(), value.to_owned()))
        })
        .collect()
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct Outcome {
    pub saved: Vec<String>,
    pub failed: Vec<(String, String)>,
}

impl Outcome {
    pub(super) fn message(&self) -> Option<String> {
        if self.failed.is_empty() {
            return None;
        }
        Some(
            self.failed
                .iter()
                .map(|(key, error)| format!("{key}: {error}"))
                .collect::<Vec<_>>()
                .join("\n"),
        )
    }
}

/// Stores each entry; one failure does not stop the others.
pub(super) fn save(runner: &dyn GhRunner, repo: &str, entries: &[(String, String)]) -> Outcome {
    let mut outcome = Outcome::default();
    for (key, value) in entries {
        match codespaces::set_secret(runner, key, value, repo) {
            Ok(()) => outcome.saved.push(key.clone()),
            Err(error) => outcome.failed.push((key.clone(), error)),
        }
    }
    outcome
}

const TOKEN_PREFIX: &str = "sk-ant-oat";
/// Real tokens are about 108 characters; this still rejects a half-printed one.
const MIN_TOKEN_LEN: usize = 80;

fn is_token_line(line: &str) -> bool {
    !line.is_empty()
        && line
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// The token `claude setup-token` printed, found in the terminal's tail text. It is the
/// last line that is only token characters and starts with `sk-ant-oat`. When the terminal
/// wrapped it, the lines after it are token-only and as long as the first, the last one
/// possibly shorter. The block must end at a blank line or at the end of the text: a line
/// with anything else on it, a longer or shorter-in-the-middle line, or text right after
/// it means this is not a clean token and nothing is returned.
pub(super) fn captured_token(tail: &str) -> Option<String> {
    let lines: Vec<&str> = tail.lines().map(str::trim).collect();
    let start = lines
        .iter()
        .rposition(|line| line.starts_with(TOKEN_PREFIX) && is_token_line(line))?;
    let width = lines[start].len();
    let mut token = String::from(lines[start]);
    let mut shorter = false;
    for line in &lines[start + 1..] {
        if line.is_empty() {
            break;
        }
        if shorter || !is_token_line(line) || line.len() > width {
            return None;
        }
        shorter = line.len() < width;
        token.push_str(line);
    }
    let token = codespaces::normalize_secret_value(OAUTH_SECRET_KEY, &token).ok()?;
    (token.len() >= MIN_TOKEN_LEN).then_some(token)
}

/// `git remote get-url origin` for the project, if it has one.
pub(super) fn git_origin(project: &Path) -> Option<String> {
    let program = monocode_core::scm::resolve_gui_binary("git")?;
    let mut command = std::process::Command::new(program);
    command
        .args(["remote", "get-url", "origin"])
        .current_dir(project)
        .stdin(std::process::Stdio::null());
    monocode_core::process::configure_child(&mut command);
    let out = command.output().ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_owned())
        .filter(|url| !url.is_empty())
}
