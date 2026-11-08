//! What the Git credential dialog decides. The access token only ever passes through the
//! arguments of these functions: nothing here keeps it, and every message that could echo it
//! is redacted first.
use monocode_core::git_credentials::{
    self, AccessCheck, AccessStatus, GitCredentialSummary, GitCredentials, GitPlatform, GitRunner,
    Secret,
};

pub(super) const TOKEN_HINT: &str = "Encrypted on this computer. No account password.";
pub(super) const SCOPE_HINT: &str = "Read access for clone/fetch; write permission for push. Choose only the required repositories.";
pub(super) const USERNAME_HINT_GITHUB: &str = "GitHub username. The token supplies authentication.";
pub(super) const USERNAME_HINT_GITLAB: &str =
    "Optional. GitLab accepts oauth2 with an access token.";

pub(super) fn username_hint(platform: GitPlatform) -> &'static str {
    match platform {
        GitPlatform::GitHub => USERNAME_HINT_GITHUB,
        GitPlatform::GitLab => USERNAME_HINT_GITLAB,
    }
}

/// The username to show when the platform changes and the field was not edited.
pub(super) fn default_username(platform: GitPlatform) -> &'static str {
    match platform {
        GitPlatform::GitHub => "",
        GitPlatform::GitLab => "oauth2",
    }
}

/// The result line under Check access.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct CheckLine {
    pub ok: bool,
    pub text: String,
}

/// Maps a read-access check to a line. Read access is the most it can prove.
pub(super) fn check_line(result: Result<AccessCheck, String>, token: &str) -> CheckLine {
    match result {
        Ok(check) => CheckLine {
            ok: check.status == AccessStatus::ReadOk,
            text: git_credentials::redact(&check.message, token),
        },
        Err(message) => CheckLine {
            ok: false,
            text: git_credentials::redact(&message, token),
        },
    }
}

/// Stores a new credential. The token is not kept.
pub(super) fn save_credential(
    credentials: &GitCredentials,
    label: &str,
    platform: GitPlatform,
    username: &str,
    token: &str,
) -> Result<String, String> {
    credentials
        .save(label, platform, &platform.origin(), username, token)
        .map_err(|e| git_credentials::redact(&e, token))
}

/// Checks read access of a saved credential against the repository.
pub(super) fn check_saved(
    credentials: &GitCredentials,
    runner: &dyn GitRunner,
    id: &str,
    repo_url: &str,
) -> CheckLine {
    check_line(credentials.check_access(runner, id, repo_url), "")
}

/// Checks a token that is not saved. It is held in memory only: nothing is stored.
pub(super) fn check_unsaved(
    runner: &dyn GitRunner,
    platform: GitPlatform,
    username: &str,
    token: Secret,
    repo_url: &str,
) -> CheckLine {
    // A wiped-on-drop copy, only to redact the token from any message.
    let redact = Secret::new(token.as_str().to_owned());
    check_line(
        git_credentials::check_token(runner, platform, username, token, repo_url),
        redact.as_str(),
    )
}

/// Why the repository URL cannot be checked against `platform`, if it cannot.
pub(super) fn repo_problem(repo_url: &str, platform: GitPlatform) -> Option<String> {
    match git_credentials::normalize_origin(repo_url) {
        Ok((found, _, _)) if found == platform => None,
        Ok((found, _, _)) => Some(format!(
            "The repository is on {}, not {}.",
            found.host(),
            platform.host()
        )),
        Err(_) => Some("Enter a valid repository URL in the Docker Sandbox dialog first.".into()),
    }
}

/// The picker line of a saved profile.
pub(super) fn label_of(summary: &GitCredentialSummary) -> String {
    format!("{} · {}", summary.label, summary.platform.host())
}
