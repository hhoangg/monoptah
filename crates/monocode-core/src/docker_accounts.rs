//! Docker accounts for Docker Sandbox projects: a Docker ID plus a personal access
//! token (PAT), stored AES-256-GCM encrypted in the native store database (see
//! [`crate::accounts::sealed`]; same key file as the Claude credentials and the Git
//! credentials). `sbx` is given a short-lived JWT exchanged from the PAT, never the PAT.
//!
//! # Public API
//!
//! * [`DockerAccounts`] `::for_store(&Store)` / `::at(path)`: [`add`](DockerAccounts::add),
//!   [`replace_token`](DockerAccounts::replace_token), [`list`](DockerAccounts::list)
//!   (never decrypts), [`get`](DockerAccounts::get), [`remove`](DockerAccounts::remove)
//!   (never decrypts), [`check`](DockerAccounts::check).
//! * [`DockerHub`]: injectable HTTP exchange (PAT to JWT); [`SystemDockerHub`] is the real one.
//! * [`DockerAccess`]: the process-wide in-memory JWT cache. [`DockerAccess::access_token`]
//!   refreshes when less than 60 s remain and coalesces concurrent refreshes of one
//!   account. [`install`] / [`shared`] hold the process-wide instance.
//! * [`DockerClaims`] / [`parse_claims`]: the claims of a JWT we received.
//!
//! The PAT and the JWT are [`Secret`]s: they never appear in an error, in `Debug` output
//! or in a log, and the PAT is never stored in plaintext.
use crate::accounts::sealed;
pub use crate::git_credentials::Secret;
use base64::Engine;
use rusqlite::{params, Connection, OpenFlags, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::Read;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, RwLock};

/// DDL for the Docker account table, shared with the native store schema.
pub(crate) const DOCKER_ACCOUNTS_TABLE: &str = "CREATE TABLE IF NOT EXISTS docker_accounts(
  id TEXT PRIMARY KEY, docker_id TEXT NOT NULL, nonce BLOB NOT NULL,
  ciphertext BLOB NOT NULL, status TEXT NOT NULL, checked_at INTEGER,
  expires_at INTEGER, created_at INTEGER NOT NULL);";

const UNAVAILABLE: &str = "Docker account storage is unavailable.";
const NOT_FOUND: &str = "That Docker account no longer exists.";
const REJECTED: &str = "Docker Hub rejected this Docker ID and token. Check both, and that the token is not revoked or expired.";
const NO_SCOPE: &str = "This token has no Cloud Sandboxes access. Create a token with \"Allow Cloud Sandboxes access\" ticked.";
const TOKEN_URL: &str = "https://hub.docker.com/v2/auth/token";
/// A cached JWT is refreshed when fewer seconds than this remain.
const REFRESH_MARGIN_SECS: u64 = 60;

// ---------------------------------------------------------------------------
// Hub exchange
// ---------------------------------------------------------------------------

/// Why a PAT to JWT exchange failed. Carries no secret and no response body.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExchangeError {
    /// Docker Hub answered 401 or 403: wrong Docker ID or PAT, revoked or expired.
    Rejected,
    /// Any other HTTP status.
    Status(u16),
    /// The request did not complete (short, redacted detail).
    Network(String),
    /// A success answer without a usable `access_token`.
    Malformed,
}
impl ExchangeError {
    /// Short text for "Could not reach Docker Hub (<detail>)".
    fn detail(&self) -> String {
        match self {
            Self::Rejected => "HTTP 401".into(),
            Self::Status(code) => format!("HTTP {code}"),
            Self::Network(detail) => {
                let detail = crate::sandboxes::redact(detail);
                detail.chars().take(80).collect()
            }
            Self::Malformed => "unexpected answer".into(),
        }
    }
}

/// Seam over the Docker Hub token endpoint so tests never reach the network.
pub trait DockerHub: Send + Sync {
    /// Exchange a Docker ID and PAT for a JWT access token.
    fn exchange(&self, docker_id: &str, pat: &str) -> Result<Secret, ExchangeError>;
}

/// Talks to `https://hub.docker.com/v2/auth/token`.
pub struct SystemDockerHub;
impl DockerHub for SystemDockerHub {
    fn exchange(&self, docker_id: &str, pat: &str) -> Result<Secret, ExchangeError> {
        let agent = ureq::AgentBuilder::new()
            .redirects(0)
            .timeout_connect(std::time::Duration::from_secs(5))
            .timeout(std::time::Duration::from_secs(15))
            .build();
        let body = zeroize::Zeroizing::new(
            serde_json::json!({ "identifier": docker_id, "secret": pat }).to_string(),
        );
        let response = agent
            .post(TOKEN_URL)
            .set("Content-Type", "application/json")
            .send_string(&body);
        let response = match response {
            Ok(response) => response,
            Err(ureq::Error::Status(401 | 403, _)) => return Err(ExchangeError::Rejected),
            Err(ureq::Error::Status(code, _)) => return Err(ExchangeError::Status(code)),
            // The kind alone: the transport message may repeat the URL.
            Err(ureq::Error::Transport(error)) => {
                return Err(ExchangeError::Network(error.kind().to_string()))
            }
        };
        let mut text = zeroize::Zeroizing::new(String::new());
        response
            .into_reader()
            .take(1 << 20)
            .read_to_string(&mut text)
            .map_err(|_| ExchangeError::Malformed)?;
        let value: serde_json::Value =
            serde_json::from_str(&text).map_err(|_| ExchangeError::Malformed)?;
        match value.get("access_token").and_then(|v| v.as_str()) {
            Some(token) if !token.is_empty() => Ok(Secret::new(token.to_owned())),
            _ => Err(ExchangeError::Malformed),
        }
    }
}

// ---------------------------------------------------------------------------
// JWT claims
// ---------------------------------------------------------------------------

/// The claims of a Docker Hub JWT that this app reads.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DockerClaims {
    pub exp: Option<u64>,
    pub iat: Option<u64>,
    pub sub: Option<String>,
    pub scopes: Vec<String>,
}
impl DockerClaims {
    /// Sandbox access: any scope word equal to `sandbox:use` or starting with `sandbox`.
    pub fn has_sandbox_access(&self) -> bool {
        self.scopes.iter().any(|s| s.starts_with("sandbox"))
    }
}

/// Decode the payload of a JWT WITHOUT verifying its signature. This is safe here
/// because we only read the claims of the token Docker Hub just handed us over TLS;
/// it is never used to trust a token from anywhere else. `None` when the text is not
/// a JWT with a JSON object payload. `scope` may be a space-separated string or an
/// array of strings.
pub fn parse_claims(jwt: &str) -> Option<DockerClaims> {
    let mut parts = jwt.split('.');
    let (_, payload) = (parts.next()?, parts.next()?);
    parts.next()?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload.trim_end_matches('='))
        .ok()?;
    let value: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    let object = value.as_object()?;
    let number = |key: &str| object.get(key).and_then(|v| v.as_u64());
    let scopes = match object.get("scope") {
        Some(serde_json::Value::String(text)) => {
            text.split_whitespace().map(str::to_owned).collect()
        }
        Some(serde_json::Value::Array(items)) => items
            .iter()
            .filter_map(|v| v.as_str())
            .flat_map(str::split_whitespace)
            .map(str::to_owned)
            .collect(),
        _ => Vec::new(),
    };
    Some(DockerClaims {
        exp: number("exp"),
        iat: number("iat"),
        sub: object
            .get("sub")
            .and_then(|v| v.as_str())
            .map(str::to_owned),
        scopes,
    })
}

// ---------------------------------------------------------------------------
// Status and summary
// ---------------------------------------------------------------------------

/// Last known state of an account's token, persisted as text.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DockerAccountStatus {
    Valid,
    Revoked,
    MissingScope,
    Unchecked,
}
impl DockerAccountStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Valid => "valid",
            Self::Revoked => "revoked",
            Self::MissingScope => "missing_scope",
            Self::Unchecked => "unchecked",
        }
    }
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "valid" => Some(Self::Valid),
            "revoked" => Some(Self::Revoked),
            "missing_scope" => Some(Self::MissingScope),
            "unchecked" => Some(Self::Unchecked),
            _ => None,
        }
    }
    /// UI text.
    pub fn label(self) -> &'static str {
        match self {
            Self::Valid => "Valid",
            Self::Revoked => "Token revoked or expired",
            Self::MissingScope => "Missing Cloud Sandboxes access",
            Self::Unchecked => "Not checked yet",
        }
    }
}

/// A saved account without its token. Times are Unix milliseconds.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DockerAccountSummary {
    pub id: String,
    pub docker_id: String,
    pub status: DockerAccountStatus,
    pub checked_at: Option<u64>,
    /// Always `None` for now: Docker's exchange does not report the PAT expiry.
    pub expires_at: Option<u64>,
    pub created_at: u64,
}

// ---------------------------------------------------------------------------
// Validation
// ---------------------------------------------------------------------------

fn validate_docker_id(docker_id: &str) -> Result<String, String> {
    let docker_id = docker_id.trim();
    let len = docker_id.chars().count();
    if !(2..=64).contains(&len)
        || !docker_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
    {
        return Err(
            "The Docker ID must be 2 to 64 characters: letters, digits, \".\", \"_\" or \"-\"."
                .into(),
        );
    }
    Ok(docker_id.to_owned())
}
fn validate_pat(pat: &str) -> Result<String, String> {
    let pat = pat.trim();
    if pat.is_empty() {
        return Err("Enter the personal access token.".into());
    }
    if pat.len() > 512 || !pat.bytes().all(|b| b.is_ascii_graphic()) {
        return Err("The token must be one line without spaces.".into());
    }
    if !(pat.starts_with("dckr_pat_") || pat.starts_with("dckr_oat_")) {
        return Err("Use a personal access token (dckr_pat_...), not your Docker password.".into());
    }
    Ok(pat.to_owned())
}
fn associated_data(id: &str, docker_id: &str) -> Vec<u8> {
    format!("monoptah-docker-account-v1\0{id}\0{docker_id}").into_bytes()
}

// ---------------------------------------------------------------------------
// Exchange with the scope check
// ---------------------------------------------------------------------------

enum Failure {
    Rejected,
    MissingScope,
    Network(String),
}
/// Exchange and require sandbox scope. The JWT is returned with its claims.
fn exchange_checked(
    hub: &dyn DockerHub,
    docker_id: &str,
    pat: &str,
) -> Result<(Secret, DockerClaims), Failure> {
    let jwt = hub.exchange(docker_id, pat).map_err(|error| match error {
        ExchangeError::Rejected => Failure::Rejected,
        other => Failure::Network(other.detail()),
    })?;
    let claims = parse_claims(jwt.as_str())
        .ok_or_else(|| Failure::Network(ExchangeError::Malformed.detail()))?;
    if !claims.has_sandbox_access() {
        return Err(Failure::MissingScope);
    }
    Ok((jwt, claims))
}
fn failure_text(failure: Failure) -> String {
    match failure {
        Failure::Rejected => REJECTED.into(),
        Failure::MissingScope => NO_SCOPE.into(),
        Failure::Network(detail) => {
            format!("Could not reach Docker Hub ({detail}). Check your connection and try again.")
        }
    }
}

type RawRow = (String, String, String, Option<i64>, Option<i64>, i64);

// ---------------------------------------------------------------------------
// Store
// ---------------------------------------------------------------------------

/// Docker accounts stored in one native store database.
pub struct DockerAccounts {
    database: PathBuf,
}
impl DockerAccounts {
    /// The accounts of the store's data directory (the credentials database).
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

    fn duplicate(conn: &Connection, docker_id: &str) -> Result<(), String> {
        let found: Option<String> = conn
            .query_row(
                "SELECT id FROM docker_accounts WHERE lower(docker_id)=lower(?1)",
                [docker_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(|_| UNAVAILABLE)?;
        if found.is_some() {
            return Err(format!(
                "Docker account {docker_id} is already added; use Replace token."
            ));
        }
        Ok(())
    }

    /// Validate, exchange, require sandbox scope and save a new account.
    pub fn add(
        &self,
        docker_id: &str,
        pat: &str,
        hub: &dyn DockerHub,
    ) -> Result<DockerAccountSummary, String> {
        let docker_id = validate_docker_id(docker_id)?;
        let pat = Secret::new(validate_pat(pat)?);
        // Never create the database or the key file just to fail later.
        let conn = self.open()?.ok_or(UNAVAILABLE)?;
        conn.execute_batch(DOCKER_ACCOUNTS_TABLE)
            .map_err(|_| UNAVAILABLE)?;
        Self::duplicate(&conn, &docker_id)?;
        exchange_checked(hub, &docker_id, pat.as_str()).map_err(failure_text)?;
        Self::duplicate(&conn, &docker_id)?;
        let id = crate::accounts::request_id();
        let (nonce, ciphertext) = sealed::seal(
            &self.database,
            &associated_data(&id, &docker_id),
            pat.as_str().as_bytes(),
        )?;
        let now = crate::time::now_ms() as i64;
        conn.execute(
            "INSERT INTO docker_accounts(id,docker_id,nonce,ciphertext,status,checked_at,expires_at,created_at)
             VALUES(?1,?2,?3,?4,?5,?6,NULL,?7)",
            params![
                id,
                docker_id,
                nonce.as_slice(),
                ciphertext,
                DockerAccountStatus::Valid.as_str(),
                now,
                now
            ],
        )
        .map_err(|_| "Could not save the Docker account.")?;
        self.get(&id)
    }

    /// Replace the token of an account, keeping its id. Same checks as [`add`](Self::add).
    /// Callers should also drop the account's cached JWT ([`DockerAccess::invalidate`]).
    pub fn replace_token(
        &self,
        id: &str,
        pat: &str,
        hub: &dyn DockerHub,
    ) -> Result<DockerAccountSummary, String> {
        let pat = Secret::new(validate_pat(pat)?);
        let summary = self.get(id)?;
        exchange_checked(hub, &summary.docker_id, pat.as_str()).map_err(failure_text)?;
        let (nonce, ciphertext) = sealed::seal(
            &self.database,
            &associated_data(id, &summary.docker_id),
            pat.as_str().as_bytes(),
        )?;
        let conn = self.open()?.ok_or(NOT_FOUND)?;
        let changed = conn
            .execute(
                "UPDATE docker_accounts SET nonce=?2, ciphertext=?3, status=?4, checked_at=?5
                 WHERE id=?1",
                params![
                    id,
                    nonce.as_slice(),
                    ciphertext,
                    DockerAccountStatus::Valid.as_str(),
                    crate::time::now_ms() as i64
                ],
            )
            .map_err(|_| "Could not save the Docker account.")?;
        if changed == 0 {
            return Err(NOT_FOUND.into());
        }
        self.get(id)
    }

    fn summary_of(row: &rusqlite::Row<'_>) -> rusqlite::Result<RawRow> {
        Ok((
            row.get(0)?,
            row.get(1)?,
            row.get(2)?,
            row.get(3)?,
            row.get(4)?,
            row.get(5)?,
        ))
    }
    fn build(raw: RawRow) -> DockerAccountSummary {
        let (id, docker_id, status, checked_at, expires_at, created_at) = raw;
        DockerAccountSummary {
            id,
            docker_id,
            status: DockerAccountStatus::parse(&status).unwrap_or(DockerAccountStatus::Unchecked),
            checked_at: checked_at.map(|v| v.max(0) as u64),
            expires_at: expires_at.map(|v| v.max(0) as u64),
            created_at: created_at.max(0) as u64,
        }
    }

    /// Every saved account, oldest first. Never reads or decrypts a token.
    pub fn list(&self) -> Result<Vec<DockerAccountSummary>, String> {
        let Some(conn) = self.open()? else {
            return Ok(Vec::new());
        };
        let mut query = match conn.prepare(
            "SELECT id,docker_id,status,checked_at,expires_at,created_at FROM docker_accounts
             ORDER BY created_at, rowid",
        ) {
            Ok(query) => query,
            Err(e) if e.to_string().contains("no such table") => return Ok(Vec::new()),
            Err(_) => return Err(UNAVAILABLE.into()),
        };
        let rows = query
            .query_map([], Self::summary_of)
            .map_err(|_| UNAVAILABLE)?;
        let mut out = Vec::new();
        for row in rows {
            out.push(Self::build(row.map_err(|_| UNAVAILABLE)?));
        }
        Ok(out)
    }

    /// One account without its token.
    pub fn get(&self, id: &str) -> Result<DockerAccountSummary, String> {
        let conn = self.open()?.ok_or(NOT_FOUND)?;
        match conn
            .query_row(
                "SELECT id,docker_id,status,checked_at,expires_at,created_at
                 FROM docker_accounts WHERE id=?1",
                [id],
                Self::summary_of,
            )
            .optional()
        {
            Ok(Some(raw)) => Ok(Self::build(raw)),
            Ok(None) => Err(NOT_FOUND.into()),
            Err(e) if e.to_string().contains("no such table") => Err(NOT_FOUND.into()),
            Err(_) => Err(UNAVAILABLE.into()),
        }
    }

    /// Delete an account. Never decrypts, so a lost key cannot block removal.
    pub fn remove(&self, id: &str) -> Result<(), String> {
        let Some(conn) = self.open()? else {
            return Ok(());
        };
        match conn.execute("DELETE FROM docker_accounts WHERE id=?1", [id]) {
            Ok(_) => Ok(()),
            Err(e) if e.to_string().contains("no such table") => Ok(()),
            Err(_) => Err("Could not remove the Docker account.".into()),
        }
    }

    /// The Docker ID and decrypted PAT, `None` when the account does not exist.
    fn secret(&self, id: &str) -> Result<Option<(String, Secret)>, String> {
        let Some(conn) = self.open()? else {
            return Ok(None);
        };
        let row = conn
            .query_row(
                "SELECT docker_id,nonce,ciphertext FROM docker_accounts WHERE id=?1",
                [id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, Vec<u8>>(1)?,
                        row.get::<_, Vec<u8>>(2)?,
                    ))
                },
            )
            .optional();
        let (docker_id, nonce, ciphertext) = match row {
            Ok(Some(row)) => row,
            Ok(None) => return Ok(None),
            Err(e) if e.to_string().contains("no such table") => return Ok(None),
            Err(_) => return Err(UNAVAILABLE.into()),
        };
        let plain = sealed::open(
            &self.database,
            &associated_data(id, &docker_id),
            &nonce,
            &ciphertext,
        )
        .map_err(|_| "Could not read the saved Docker token. Replace it in Settings > Cloud.")?;
        let pat = String::from_utf8(plain.to_vec()).map_err(|_| {
            "Could not read the saved Docker token. Replace it in Settings > Cloud."
        })?;
        Ok(Some((docker_id, Secret::new(pat))))
    }

    /// Persist a status and the check time. Failures are ignored by callers that
    /// already have an answer for the user.
    fn set_status(&self, id: &str, status: DockerAccountStatus) -> Result<(), String> {
        let Some(conn) = self.open()? else {
            return Ok(());
        };
        conn.execute(
            "UPDATE docker_accounts SET status=?2, checked_at=?3 WHERE id=?1",
            params![id, status.as_str(), crate::time::now_ms() as i64],
        )
        .map(|_| ())
        .map_err(|_| UNAVAILABLE.into())
    }

    /// Re-exchange the stored token and update the status. A rejection or missing scope
    /// is a result (the returned summary carries the new status). A network failure keeps
    /// the stored status and returns the error.
    pub fn check(&self, id: &str, hub: &dyn DockerHub) -> Result<DockerAccountSummary, String> {
        let (docker_id, pat) = self.secret(id)?.ok_or(NOT_FOUND)?;
        let status = match exchange_checked(hub, &docker_id, pat.as_str()) {
            Ok(_) => DockerAccountStatus::Valid,
            Err(Failure::Rejected) => DockerAccountStatus::Revoked,
            Err(Failure::MissingScope) => DockerAccountStatus::MissingScope,
            Err(network) => return Err(failure_text(network)),
        };
        self.set_status(id, status)?;
        self.get(id)
    }
}

// ---------------------------------------------------------------------------
// JWT cache
// ---------------------------------------------------------------------------

/// Why no JWT could be produced for an account. Display texts are user-facing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DockerAuthError {
    UnknownAccount,
    Revoked { docker_id: String },
    MissingScope { docker_id: String },
    Network { docker_id: String, detail: String },
    Storage(String),
}
impl std::fmt::Display for DockerAuthError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownAccount => {
                f.write_str("That Docker account no longer exists. Choose another in Cloud settings.")
            }
            Self::Revoked { docker_id } => write!(
                f,
                "Docker account {docker_id}: token revoked or expired. Replace it in Settings > Cloud."
            ),
            Self::MissingScope { docker_id } => write!(
                f,
                "Docker account {docker_id}: missing Cloud Sandboxes access. Replace the token in Settings > Cloud."
            ),
            Self::Network { docker_id, detail } => write!(
                f,
                "Docker account {docker_id}: could not reach Docker Hub ({detail}). Check your connection and try again."
            ),
            Self::Storage(text) => f.write_str(text),
        }
    }
}
impl std::error::Error for DockerAuthError {}

/// Seconds since the Unix epoch.
pub type Clock = Arc<dyn Fn() -> u64 + Send + Sync>;
/// The real clock.
pub fn system_clock() -> Clock {
    Arc::new(|| crate::time::now_ms() / 1000)
}

struct Cached {
    token: Secret,
    /// Seconds. A token without `exp` is stored as already expired (never reused).
    exp: u64,
}
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

/// Process-wide cache of Docker JWTs, one entry per account.
pub struct DockerAccess {
    accounts: DockerAccounts,
    hub: Arc<dyn DockerHub>,
    clock: Clock,
    /// Per-account slot; its lock is held across the exchange so concurrent refreshes of
    /// one account coalesce while other accounts proceed.
    cache: Mutex<HashMap<String, Arc<Mutex<Option<Cached>>>>>,
}
impl DockerAccess {
    pub fn new(database: PathBuf, hub: Arc<dyn DockerHub>, clock: Clock) -> Self {
        Self {
            accounts: DockerAccounts::at(database),
            hub,
            clock,
            cache: Mutex::new(HashMap::new()),
        }
    }

    /// A JWT for `account_id`: cached while more than 60 s remain, otherwise exchanged
    /// again. Updates the stored status (`valid`, `revoked`, `missing_scope`).
    pub fn access_token(&self, account_id: &str) -> Result<Secret, DockerAuthError> {
        let slot = lock(&self.cache)
            .entry(account_id.to_owned())
            .or_default()
            .clone();
        let mut cached = lock(&slot);
        let now = (self.clock)();
        if let Some(entry) = cached.as_ref() {
            if entry.exp > now.saturating_add(REFRESH_MARGIN_SECS) {
                return Ok(Secret::new(entry.token.as_str().to_owned()));
            }
        }
        *cached = None;
        let (docker_id, pat) = self
            .accounts
            .secret(account_id)
            .map_err(DockerAuthError::Storage)?
            .ok_or(DockerAuthError::UnknownAccount)?;
        match exchange_checked(self.hub.as_ref(), &docker_id, pat.as_str()) {
            Ok((jwt, claims)) => {
                let _ = self
                    .accounts
                    .set_status(account_id, DockerAccountStatus::Valid);
                *cached = Some(Cached {
                    token: Secret::new(jwt.as_str().to_owned()),
                    exp: claims.exp.unwrap_or(0),
                });
                Ok(jwt)
            }
            Err(Failure::Rejected) => {
                let _ = self
                    .accounts
                    .set_status(account_id, DockerAccountStatus::Revoked);
                Err(DockerAuthError::Revoked { docker_id })
            }
            Err(Failure::MissingScope) => {
                let _ = self
                    .accounts
                    .set_status(account_id, DockerAccountStatus::MissingScope);
                Err(DockerAuthError::MissingScope { docker_id })
            }
            Err(Failure::Network(detail)) => Err(DockerAuthError::Network { docker_id, detail }),
        }
    }

    /// Drop the cached JWT of an account (after a rejection, token replacement or removal).
    pub fn invalidate(&self, account_id: &str) {
        let slot = lock(&self.cache).get(account_id).cloned();
        if let Some(slot) = slot {
            *lock(&slot) = None;
        }
    }
}

static SHARED: RwLock<Option<Arc<DockerAccess>>> = RwLock::new(None);

/// Install the process-wide instance, replacing any earlier one (tests install fakes).
pub fn install(access: Arc<DockerAccess>) {
    *SHARED.write().unwrap_or_else(|e| e.into_inner()) = Some(access);
}
/// The process-wide instance, if one was installed.
pub fn shared() -> Option<Arc<DockerAccess>> {
    SHARED.read().unwrap_or_else(|e| e.into_inner()).clone()
}

#[cfg(test)]
mod tests;
