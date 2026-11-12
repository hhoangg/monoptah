//! Git credential profiles for Docker sandbox projects: a personal access token
//! for github.com or gitlab.com, stored AES-256-GCM encrypted in the native store
//! database (see [`crate::accounts::sealed`]; same key file as the Claude
//! credentials). Version 1 supports HTTPS on those two hosts only.
//!
//! # Public API
//!
//! * [`normalize_origin`] `(url) -> (GitPlatform, origin, repo_path)`: strict repo URL check.
//! * [`GitCredentials`] `::for_store(&Store)`: [`save`](GitCredentials::save),
//!   [`list`](GitCredentials::list) (never returns a secret),
//!   [`remove`](GitCredentials::remove), [`check_access`](GitCredentials::check_access)
//!   (read access only), [`credential_payload`](GitCredentials::credential_payload)
//!   (the `git credential approve` stdin payload, for delivery to a sandbox).
//!
//! The token never appears in argv or logs. A check hands it to git through a
//! throwaway `GIT_ASKPASS` script in a private temp directory plus environment
//! variables; the script is removed afterwards. Git runs through the injectable
//! [`GitRunner`] so tests never start a real process.
use crate::accounts::sealed;
use rusqlite::{params, Connection, OpenFlags, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use zeroize::Zeroizing;

/// A secret string that is wiped on drop and never printed by `{:?}`.
pub struct Secret(Zeroizing<String>);
impl Secret {
    pub fn new(value: String) -> Self {
        Self(Zeroizing::new(value))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Secret(***)")
    }
}

/// DDL for the credential table, shared with the native store schema.
pub(crate) const GIT_CREDENTIALS_TABLE: &str = "CREATE TABLE IF NOT EXISTS git_credentials(
  id TEXT PRIMARY KEY, label TEXT NOT NULL, platform TEXT NOT NULL, origin TEXT NOT NULL,
  username TEXT NOT NULL, nonce BLOB NOT NULL, ciphertext BLOB NOT NULL,
  created_at INTEGER NOT NULL);";

const UNAVAILABLE: &str = "Git credential storage is unavailable.";
const NOT_FOUND: &str = "That Git credential no longer exists.";
const USERNAME_ENV: &str = "MONOPTAH_GIT_USERNAME";
pub const TOKEN_ENV: &str = "MONOPTAH_GIT_TOKEN";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GitPlatform {
    GitHub,
    GitLab,
}
impl GitPlatform {
    pub fn host(self) -> &'static str {
        match self {
            Self::GitHub => "github.com",
            Self::GitLab => "gitlab.com",
        }
    }
    pub fn origin(self) -> String {
        format!("https://{}", self.host())
    }
    fn name(self) -> &'static str {
        match self {
            Self::GitHub => "github",
            Self::GitLab => "gitlab",
        }
    }
    fn parse(name: &str) -> Option<Self> {
        match name {
            "github" => Some(Self::GitHub),
            "gitlab" => Some(Self::GitLab),
            _ => None,
        }
    }
    fn from_host(host: &str) -> Option<Self> {
        match host {
            "github.com" => Some(Self::GitHub),
            "gitlab.com" => Some(Self::GitLab),
            _ => None,
        }
    }
}

/// Check a repository URL and split it into platform, origin and repo path.
/// HTTPS on github.com / gitlab.com only. The caller keeps the original URL for
/// cloning; the returned path has any trailing `.git` removed.
pub fn normalize_origin(url: &str) -> Result<(GitPlatform, String, String), String> {
    const BAD: &str =
        "Use an HTTPS repository URL on github.com or gitlab.com, such as https://github.com/owner/repo.";
    let url = url.trim();
    let dotted =
        url.contains("/./") || url.contains("/../") || url.ends_with("/.") || url.ends_with("/..");
    if url.is_empty()
        || !url.starts_with("https://")
        || dotted
        || url
            .chars()
            .any(|c| c.is_whitespace() || c.is_control() || c == '\\' || c == '%')
    {
        return Err(BAD.into());
    }
    let parsed = url::Url::parse(url).map_err(|_| BAD)?;
    if parsed.scheme() != "https" {
        return Err(BAD.into());
    }
    if !parsed.username().is_empty() || parsed.password().is_some() || url.contains('@') {
        return Err("Remove the credentials from the repository URL. Save the token as a Git credential instead.".into());
    }
    if parsed.query().is_some()
        || parsed.fragment().is_some()
        || url.contains('?')
        || url.contains('#')
    {
        return Err("The repository URL must not contain a query or fragment.".into());
    }
    if parsed.port().is_some() {
        return Err(BAD.into());
    }
    let platform = parsed
        .host_str()
        .and_then(GitPlatform::from_host)
        .ok_or(BAD)?;
    let path = parsed.path().trim_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);
    let segments: Vec<&str> = path.split('/').collect();
    let valid_depth = match platform {
        GitPlatform::GitHub => segments.len() == 2,
        GitPlatform::GitLab => segments.len() >= 2,
    };
    if !valid_depth
        || segments
            .iter()
            .any(|s| s.is_empty() || *s == "." || *s == ".." || s.starts_with('-'))
    {
        return Err(BAD.into());
    }
    Ok((platform, platform.origin(), segments.join("/")))
}

/// The `path=` of a credential payload: the clone URL without its origin and leading slash.
fn repo_path_of(clone_url: &str, origin: &str) -> String {
    clone_url[origin.len() + 1..].to_owned()
}

/// The platform and the `path=` the credential payload uses for `repo_url`, without needing
/// a stored credential (the sandbox's `gh` wrapper asks the cache for this exact entry).
pub fn payload_path(repo_url: &str) -> Result<(GitPlatform, String), String> {
    let (platform, origin, _) = normalize_origin(repo_url)?;
    Ok((platform, repo_path_of(&clone_url(repo_url)?, &origin)))
}

/// The URL to hand to git: the normalised form of `url`, keeping a trailing
/// `.git` when the input had one. Never the raw input.
pub fn clone_url(url: &str) -> Result<String, String> {
    let (_, origin, path) = normalize_origin(url)?;
    let git = url.trim().trim_end_matches('/').ends_with(".git");
    Ok(format!("{origin}/{path}{}", if git { ".git" } else { "" }))
}

/// A saved credential without its secret.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitCredentialSummary {
    pub id: String,
    pub label: String,
    pub platform: GitPlatform,
    pub origin: String,
    pub username: String,
    pub created_at: u64,
}

/// Remove every occurrence of `secret` from `text`.
pub fn redact(text: &str, secret: &str) -> String {
    if secret.is_empty() {
        text.to_owned()
    } else {
        text.replace(secret, "[redacted]")
    }
}

/// Outcome of a read-access check.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AccessStatus {
    ReadOk,
    AuthFailed,
    NotFound,
    Network,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AccessCheck {
    pub status: AccessStatus,
    /// User-facing text. For `ReadOk` it states that write access is not proven.
    pub message: String,
}

/// Result of one git invocation.
pub struct GitOutput {
    pub success: bool,
    pub stderr: String,
}
/// Seam over the local `git` binary. `env` is added to the process environment;
/// the runner must also export `token` as `MONOPTAH_GIT_TOKEN` (see
/// [`TOKEN_ENV`]) so the secret never sits in a plain `String` list.
pub trait GitRunner: Send + Sync {
    fn run(
        &self,
        args: &[String],
        env: &[(String, String)],
        token: &Secret,
    ) -> Result<GitOutput, String>;
}
/// Runs the real `git`.
pub struct SystemGit;
impl GitRunner for SystemGit {
    fn run(
        &self,
        args: &[String],
        env: &[(String, String)],
        token: &Secret,
    ) -> Result<GitOutput, String> {
        // Run from an empty directory under a ceiling, with no inherited GIT_*
        // variable (GIT_DIR, GIT_CONFIG_PARAMETERS, GIT_CONFIG_COUNT, ...).
        let cwd = tempfile::tempdir().map_err(|e| format!("Could not run git: {e}"))?;
        let mut command = std::process::Command::new("git");
        command.args(args).current_dir(cwd.path());
        for (name, _) in std::env::vars_os() {
            if name.to_string_lossy().to_uppercase().starts_with("GIT_") {
                command.env_remove(name);
            }
        }
        if let Some(parent) = cwd.path().parent() {
            command.env("GIT_CEILING_DIRECTORIES", parent);
        }
        let output = command
            .envs(env.iter().map(|(k, v)| (k, v)))
            .env(TOKEN_ENV, token.as_str())
            .stdin(std::process::Stdio::null())
            .output()
            .map_err(|e| format!("Could not run git: {e}"))?;
        Ok(GitOutput {
            success: output.status.success(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        })
    }
}

fn classify(stderr: &str) -> AccessStatus {
    let text = stderr.to_lowercase();
    if text.contains("authentication failed")
        || text.contains("could not read username")
        || text.contains("could not read password")
        || text.contains("invalid username or password")
        || text.contains("error: 401")
        || text.contains("error: 403")
        || text.contains("permission denied")
    {
        AccessStatus::AuthFailed
    } else if text.contains("not found") || text.contains("error: 404") {
        AccessStatus::NotFound
    } else {
        AccessStatus::Network
    }
}
fn associated_data(id: &str, origin: &str, username: &str) -> Vec<u8> {
    format!("monoptah-git-credential-v1\0{id}\0{origin}\0{username}").into_bytes()
}

fn validate_label(label: &str) -> Result<String, String> {
    let label = label.trim();
    if label.is_empty() || label.chars().count() > 80 || label.chars().any(char::is_control) {
        return Err("Give the Git credential a short name.".into());
    }
    Ok(label.to_owned())
}
fn validate_username(platform: GitPlatform, username: &str) -> Result<String, String> {
    let username = username.trim();
    if username.is_empty() {
        return match platform {
            GitPlatform::GitHub => Err("Enter the GitHub username for this token.".into()),
            GitPlatform::GitLab => Ok("oauth2".into()),
        };
    }
    // Also embedded in the askpass script, so keep it to a safe alphabet.
    if username.len() > 128
        || !username
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
    {
        return Err("The username contains invalid characters.".into());
    }
    Ok(username.to_owned())
}
fn validate_token(token: &str) -> Result<(), String> {
    if token.is_empty() || token.len() > 4096 || !token.bytes().all(|b| b.is_ascii_graphic()) {
        return Err("The token must be one line without spaces.".into());
    }
    Ok(())
}

/// Git credential profiles stored in one native store database.
pub struct GitCredentials {
    database: PathBuf,
}
impl GitCredentials {
    /// The profiles of the store's data directory. Uses the database the store
    /// registered for credentials, which is where the Claude tokens live too.
    pub fn for_store(store: &crate::store::Store) -> Result<Self, String> {
        let dir = store.data_dir().ok_or(UNAVAILABLE)?;
        let database = crate::accounts::database_for(dir).ok_or(UNAVAILABLE)?;
        Ok(Self::at(database))
    }
    pub fn at(database: PathBuf) -> Self {
        Self { database }
    }

    /// Open an existing database without creating it.
    fn open(&self) -> Result<Option<Connection>, String> {
        if !self.database.exists() {
            return Ok(None);
        }
        let conn = Connection::open_with_flags(&self.database, OpenFlags::SQLITE_OPEN_READ_WRITE)
            .map_err(|_| UNAVAILABLE)?;
        conn.busy_timeout(std::time::Duration::from_secs(5))
            .map_err(|_| UNAVAILABLE)?;
        Ok(Some(conn))
    }

    /// Validate and store a token. Returns the new credential id.
    pub fn save(
        &self,
        label: &str,
        platform: GitPlatform,
        origin: &str,
        username: &str,
        token: &str,
    ) -> Result<String, String> {
        let label = validate_label(label)?;
        if origin.trim_end_matches('/') != platform.origin() {
            return Err(format!("This token must be for {}.", platform.host()));
        }
        let origin = platform.origin();
        let username = validate_username(platform, username)?;
        validate_token(token)?;
        // Never create the database or the key file just to fail later.
        let conn = self.open()?.ok_or("Could not save the Git credential.")?;
        conn.execute_batch(GIT_CREDENTIALS_TABLE)
            .map_err(|_| "Could not save the Git credential.")?;
        let id = crate::accounts::request_id();
        let (nonce, ciphertext) = sealed::seal(
            &self.database,
            &associated_data(&id, &origin, &username),
            token.as_bytes(),
        )?;
        conn.execute(
            "INSERT INTO git_credentials(id,label,platform,origin,username,nonce,ciphertext,created_at)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
            params![
                id,
                label,
                platform.name(),
                origin,
                username,
                nonce.as_slice(),
                ciphertext,
                crate::time::now_ms() as i64
            ],
        )
        .map_err(|_| "Could not save the Git credential.")?;
        Ok(id)
    }

    /// Every saved credential, oldest first. Never reads or decrypts a secret.
    pub fn list(&self) -> Result<Vec<GitCredentialSummary>, String> {
        let Some(conn) = self.open()? else {
            return Ok(Vec::new());
        };
        let mut query = match conn.prepare(
            "SELECT id,label,platform,origin,username,created_at FROM git_credentials
             ORDER BY created_at, rowid",
        ) {
            Ok(query) => query,
            Err(e) if e.to_string().contains("no such table") => return Ok(Vec::new()),
            Err(_) => return Err(UNAVAILABLE.into()),
        };
        let rows = query
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, i64>(5)?,
                ))
            })
            .map_err(|_| UNAVAILABLE)?;
        let mut out = Vec::new();
        for row in rows {
            let (id, label, platform, origin, username, created_at) =
                row.map_err(|_| UNAVAILABLE)?;
            if let Some(platform) = GitPlatform::parse(&platform) {
                out.push(GitCredentialSummary {
                    id,
                    label,
                    platform,
                    origin,
                    username,
                    created_at: created_at.max(0) as u64,
                });
            }
        }
        Ok(out)
    }

    /// Delete a credential. Never decrypts, so a lost key cannot block removal.
    pub fn remove(&self, id: &str) -> Result<(), String> {
        let Some(conn) = self.open()? else {
            return Ok(());
        };
        match conn.execute("DELETE FROM git_credentials WHERE id=?1", [id]) {
            Ok(_) => Ok(()),
            Err(e) if e.to_string().contains("no such table") => Ok(()),
            Err(_) => Err("Could not remove the Git credential.".into()),
        }
    }

    fn row(&self, id: &str) -> Result<(GitCredentialSummary, Vec<u8>, Vec<u8>), String> {
        let conn = self.open()?.ok_or(NOT_FOUND)?;
        let row = conn
            .query_row(
                "SELECT label,platform,origin,username,nonce,ciphertext,created_at
                 FROM git_credentials WHERE id=?1",
                [id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, Vec<u8>>(4)?,
                        row.get::<_, Vec<u8>>(5)?,
                        row.get::<_, i64>(6)?,
                    ))
                },
            )
            .optional()
            .map_err(|_| UNAVAILABLE)?
            .ok_or(NOT_FOUND)?;
        let (label, platform, origin, username, nonce, ciphertext, created_at) = row;
        let platform = GitPlatform::parse(&platform).ok_or(UNAVAILABLE)?;
        Ok((
            GitCredentialSummary {
                id: id.to_owned(),
                label,
                platform,
                origin,
                username,
                created_at: created_at.max(0) as u64,
            },
            nonce,
            ciphertext,
        ))
    }

    /// The decrypted token. Callers must not log it or put it in argv.
    pub(crate) fn secret(&self, id: &str) -> Result<Secret, String> {
        let (summary, nonce, ciphertext) = self.row(id)?;
        let plain = sealed::open(
            &self.database,
            &associated_data(&summary.id, &summary.origin, &summary.username),
            &nonce,
            &ciphertext,
        )
        .map_err(|_| "Could not read the saved Git credential. Save the token again.")?;
        String::from_utf8(plain.to_vec())
            .map(Secret::new)
            .map_err(|_| "Could not read the saved Git credential. Save the token again.".into())
    }

    /// The summary of a credential, ensuring it can be used for `repo_url`.
    /// The summary, repo path and normalised clone URL, checking the
    /// credential belongs to the URL's host.
    fn summary_for(
        &self,
        id: &str,
        repo_url: &str,
    ) -> Result<(GitCredentialSummary, String, String), String> {
        let (platform, origin, _) = normalize_origin(repo_url)?;
        let url = clone_url(repo_url)?;
        let (summary, _, _) = self.row(id)?;
        if summary.platform != platform || summary.origin != origin {
            return Err(format!(
                "This Git credential is for {}, not {}.",
                summary.platform.host(),
                platform.host()
            ));
        }
        let path = repo_path_of(&url, &origin);
        Ok((summary, path, url))
    }

    /// Check read access to `repo_url` with `git ls-remote --heads`. Write
    /// access is not proven. Fails only for local problems (missing credential,
    /// wrong host, git not runnable); remote failures are an [`AccessCheck`].
    ///
    /// Git is isolated from the user's configuration (no global/system config,
    /// HTTPS only, no redirects) so an `insteadOf` rewrite cannot send the token
    /// to another host, and the askpass helper answers only for the exact origin.
    pub fn check_access(
        &self,
        runner: &dyn GitRunner,
        id: &str,
        repo_url: &str,
    ) -> Result<AccessCheck, String> {
        let (summary, _, _) = self.summary_for(id, repo_url)?;
        let token = self.secret(id)?;
        check_with(
            runner,
            summary.platform,
            &summary.username,
            &token,
            repo_url,
        )
    }

    /// The `git credential approve` stdin payload for `repo_url`, for delivery
    /// to a sandbox. Holds the token: keep it out of argv and logs.
    ///
    /// `path` is the path of the URL actually used for cloning (with `.git` when
    /// the clone URL has it). Git only matches it against the remote URL when
    /// `credential.useHttpPath` is true, so the sandbox must not rely on it
    /// otherwise; the host and username always match.
    pub fn credential_payload(
        &self,
        id: &str,
        repo_url: &str,
    ) -> Result<Zeroizing<String>, String> {
        let (summary, path, _) = self.summary_for(id, repo_url)?;
        let token = self.secret(id)?;
        let mut payload = Zeroizing::new(String::with_capacity(160));
        for part in [
            "protocol=https\nhost=",
            summary.platform.host(),
            "\npath=",
            &path,
            "\nusername=",
            &summary.username,
            "\npassword=",
            token.as_str(),
            "\n\n",
        ] {
            payload.push_str(part);
        }
        Ok(payload)
    }
}

/// Run `git ls-remote --heads` against `repo_url` with a token that is held only in
/// memory. Shared by a saved profile ([`GitCredentials::check_access`]) and an unsaved
/// token ([`check_token`]); it never touches the database.
fn check_with(
    runner: &dyn GitRunner,
    platform: GitPlatform,
    username: &str,
    token: &Secret,
    repo_url: &str,
) -> Result<AccessCheck, String> {
    let (found, origin, _) = normalize_origin(repo_url)?;
    if found != platform {
        return Err(format!("This token must be for {}.", platform.host()));
    }
    let url = clone_url(repo_url)?;
    let dir = tempfile::Builder::new()
        .prefix("monoptah-askpass-")
        .tempdir()
        .map_err(|_| "Could not prepare the Git check.")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700))
            .map_err(|_| "Could not prepare the Git check.")?;
    }
    let script = write_askpass(dir.path(), &origin, username)?;
    let args: Vec<String> = [
        "-c",
        "credential.helper=",
        "-c",
        "http.followRedirects=false",
        "-c",
        "http.lowSpeedLimit=1000",
        "-c",
        "http.lowSpeedTime=30",
        "ls-remote",
        "--heads",
        "--",
        &url,
    ]
    .map(String::from)
    .into();
    let pair = |k: &str, v: &str| (k.to_owned(), v.to_owned());
    let env = vec![
        pair("GIT_TERMINAL_PROMPT", "0"),
        pair("GCM_INTERACTIVE", "never"),
        pair("GIT_CONFIG_GLOBAL", "/dev/null"),
        pair("GIT_CONFIG_NOSYSTEM", "1"),
        pair("GIT_ALLOW_PROTOCOL", "https"),
        pair("GIT_ASKPASS", &script.to_string_lossy()),
        pair(USERNAME_ENV, username),
    ];
    let result = runner.run(&args, &env, token);
    // Drop the helper before reporting, whatever happened.
    drop(dir);
    let output = result.map_err(|e| redact(&e, token.as_str()))?;
    Ok(if output.success {
        AccessCheck {
                status: AccessStatus::ReadOk,
                message: "Read access confirmed. Write access is not verified; pushing needs a token with write permission.".into(),
            }
    } else {
        let detail = redact(output.stderr.trim(), token.as_str());
        match classify(&detail) {
                AccessStatus::AuthFailed => AccessCheck {
                    status: AccessStatus::AuthFailed,
                    message: "The token was rejected. Check that it is valid, not expired, and can read this repository.".into(),
                },
                AccessStatus::NotFound => AccessCheck {
                    status: AccessStatus::NotFound,
                    message: "Repository not found. Check the URL, and that the token can see it.".into(),
                },
                _ => AccessCheck {
                    status: AccessStatus::Network,
                    message: format!(
                        "Could not reach {}. {}",
                        platform.host(),
                        detail.lines().last().unwrap_or("")
                    )
                    .trim()
                    .to_owned(),
                },
            }
    })
}

/// Check that `repo_url` can be read without any credential (a public repository), with
/// `git ls-remote --heads`. Git is isolated like in [`GitCredentials::check_access`] and
/// never prompts. Nothing is stored and no secret is involved.
pub fn check_public(runner: &dyn GitRunner, repo_url: &str) -> Result<AccessCheck, String> {
    let (platform, _, _) = normalize_origin(repo_url)?;
    let url = clone_url(repo_url)?;
    let args: Vec<String> = [
        "-c",
        "credential.helper=",
        "-c",
        "http.followRedirects=false",
        "-c",
        "http.lowSpeedLimit=1000",
        "-c",
        "http.lowSpeedTime=30",
        "ls-remote",
        "--heads",
        "--",
        &url,
    ]
    .map(String::from)
    .into();
    let pair = |k: &str, v: &str| (k.to_owned(), v.to_owned());
    let env = vec![
        pair("GIT_TERMINAL_PROMPT", "0"),
        pair("GCM_INTERACTIVE", "never"),
        pair("GIT_CONFIG_GLOBAL", "/dev/null"),
        pair("GIT_CONFIG_NOSYSTEM", "1"),
        pair("GIT_ALLOW_PROTOCOL", "https"),
        pair("GIT_ASKPASS", "/bin/false"),
    ];
    let output = runner.run(&args, &env, &Secret::new(String::new()))?;
    Ok(if output.success {
        AccessCheck {
            status: AccessStatus::ReadOk,
            message: "The repository can be read without a credential.".into(),
        }
    } else {
        let detail = output.stderr.trim();
        match classify(detail) {
            AccessStatus::AuthFailed => AccessCheck {
                status: AccessStatus::AuthFailed,
                message: "The repository needs a credential. Choose a Git credential.".into(),
            },
            AccessStatus::NotFound => AccessCheck {
                status: AccessStatus::NotFound,
                message: "Repository not found. Check the URL, or choose a Git credential if it is private.".into(),
            },
            _ => AccessCheck {
                status: AccessStatus::Network,
                message: format!(
                    "Could not reach {}. {}",
                    platform.host(),
                    detail.lines().last().unwrap_or("")
                )
                .trim()
                .to_owned(),
            },
        }
    })
}

/// Check read access to `repo_url` with a token that is not saved. Validates the username
/// and token like [`GitCredentials::save`] and stores nothing.
pub fn check_token(
    runner: &dyn GitRunner,
    platform: GitPlatform,
    username: &str,
    token: Secret,
    repo_url: &str,
) -> Result<AccessCheck, String> {
    let username = validate_username(platform, username)?;
    validate_token(token.as_str())?;
    check_with(runner, platform, &username, &token, repo_url)
}

/// Write the askpass helper into `dir` (a private directory). It answers only
/// the exact prompts git shows for `origin` (with or without `user@`) and exits
/// 1 for anything else, so a redirect or rewrite to another host gets nothing.
/// The secret is read from the environment, never written to the file.
/// `username` is validated to a shell-safe alphabet by [`GitCredentials::save`].
fn write_askpass(dir: &Path, origin: &str, username: &str) -> Result<PathBuf, String> {
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let safe = |value: &str| {
            value
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-' | b':' | b'/'))
        };
        if !safe(origin) || !safe(username) {
            return Err("Could not prepare the Git check.".into());
        }
        let host = origin.trim_start_matches("https://");
        let path = dir.join("askpass.sh");
        let script = format!(
            "#!/bin/sh\ncase \"$1\" in\n  \"Username for '{origin}'\"*) printf '%s\\n' \"${USERNAME_ENV}\" ;;\n  \"Password for '{origin}'\"*|\"Password for 'https://{username}@{host}'\"*) printf '%s\\n' \"${TOKEN_ENV}\" ;;\n  *) exit 1 ;;\nesac\n"
        );
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o700)
            .open(&path)
            .and_then(|mut file| file.write_all(script.as_bytes()))
            .map_err(|_| "Could not prepare the Git check.")?;
        Ok(path)
    }
    #[cfg(not(unix))]
    {
        let _ = (dir, origin, username);
        Err("Checking Git access is not supported on this platform yet.".into())
    }
}

#[cfg(test)]
mod tests;
