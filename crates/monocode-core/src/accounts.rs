//! Provider-owned sign-ins. Metadata never contains credentials.
pub(crate) mod sealed;
mod token;
pub(crate) use token::{database_for, register_database, CREDENTIALS_TABLE};
pub use token::{
    normalize_claude_api_key, normalize_claude_token, ClaudeCredential, CREDENTIAL_UNREADABLE,
};
mod desktop;
mod identity;
pub(crate) use desktop::DesktopOverrides;
pub(crate) mod login;
pub(crate) mod transfer;
pub use identity::Identity;
pub use login::arguments as login_arguments;
pub use login::{authenticate, AuthAction};
mod processes;
#[cfg(test)]
mod tests;
pub use processes::stop_processes;
pub(crate) use processes::{track, Tracked};

use crate::provider::Kind;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    ffi::OsString,
    path::{Path, PathBuf},
    process::Command,
};

pub const DEFAULT: &str = "default";
static CLAUDE_ENTRYPOINT: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// Opt-in: label spawned Claude processes as Monocode instead of the default
/// `sdk-cli`, which is metered at a higher subscription rate
/// (anthropics/claude-code#97074).
pub fn set_claude_entrypoint(on: bool) {
    CLAUDE_ENTRYPOINT.store(on, std::sync::atomic::Ordering::Relaxed);
}
pub fn claude_entrypoint() -> bool {
    CLAUDE_ENTRYPOINT.load(std::sync::atomic::Ordering::Relaxed)
}
pub const PROVIDERS: [Kind; 3] = [Kind::Claude, Kind::Codex, Kind::Devin];
fn default_id() -> String {
    DEFAULT.into()
}
pub(crate) fn request_id() -> String {
    let mut bytes: [u8; 16] = rand::random();
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex = format!("{:032x}", u128::from_be_bytes(bytes));
    format!(
        "{}-{}-{}-{}-{}",
        &hex[..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..]
    )
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(default)]
pub struct Profile {
    pub id: String,
    /// Supplied by the native store, never accepted from persisted settings.
    #[serde(skip)]
    pub root: Option<PathBuf>,
}
impl Default for Profile {
    fn default() -> Self {
        Self {
            id: default_id(),
            root: None,
        }
    }
}
impl Profile {
    pub fn new(id: &str, root: &Path) -> Self {
        if id == DEFAULT {
            return Self::default();
        }
        Self {
            id: id.into(),
            root: Some(root.into()),
        }
    }
    pub fn directory(&self, kind: Kind) -> Result<Option<PathBuf>, String> {
        if self.id == DEFAULT {
            return Ok(None);
        }
        if !supports_profiles(kind) {
            return Err("Provider account profiles are not supported for this provider".into());
        }
        validate_id(&self.id)?;
        let root = self
            .root
            .as_ref()
            .ok_or("Provider account storage is unavailable")?;
        Ok(Some(
            root.join("provider-accounts")
                .join(kind.id())
                .join(&self.id),
        ))
    }
    pub fn home(&self, kind: Kind) -> Result<PathBuf, String> {
        if let Some(dir) = self.directory(kind)? {
            return Ok(dir);
        }
        let (var, dir) = match kind {
            Kind::Codex => ("CODEX_HOME", ".codex"),
            Kind::Claude => ("CLAUDE_CONFIG_DIR", ".claude"),
            _ => {
                return Err("Native transcript transfer is not supported for this provider".into())
            }
        };
        std::env::var_os(var)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .or_else(|| home().map(|p| p.join(dir)))
            .ok_or_else(|| format!("Could not find the {} home directory", kind.label()))
    }
    /// The variables a provider process needs for this account: (set, remove).
    /// Creates the account directory. The default profile needs nothing.
    #[allow(clippy::type_complexity)]
    pub fn env(&self, kind: Kind) -> Result<(Vec<(String, OsString)>, Vec<&'static str>), String> {
        let Some(dir) = self.directory(kind)? else {
            return Ok((Vec::new(), Vec::new()));
        };
        std::fs::create_dir_all(&dir)
            .map_err(|e| format!("Could not create the account directory: {e}"))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))
                .map_err(|e| e.to_string())?;
        }
        let dir: OsString = dir.into();
        let mut environment = match kind {
            Kind::Claude => (
                vec![
                    ("CLAUDE_CONFIG_DIR".into(), dir.clone()),
                    ("CLAUDE_SECURESTORAGE_CONFIG_DIR".into(), dir),
                ],
                vec![
                    "ANTHROPIC_API_KEY",
                    "ANTHROPIC_AUTH_TOKEN",
                    "CLAUDE_CODE_OAUTH_TOKEN",
                ],
            ),
            Kind::Codex => (
                vec![("CODEX_HOME".into(), dir)],
                vec!["OPENAI_API_KEY", "CODEX_API_KEY", "CODEX_ACCESS_TOKEN"],
            ),
            _ => unreachable!(),
        };
        if kind == Kind::Claude {
            if let Some((credential, secret)) = self.claude_secret()? {
                // The PTY uses an env -u wrapper for removals; retaining this
                // key there would also strip the account-specific value.
                let variable = credential.variable();
                environment.1.retain(|key| *key != variable);
                environment.0.push((variable.into(), secret.into()));
            }
        }
        Ok(environment)
    }
    pub fn apply(&self, kind: Kind, command: &mut Command) -> Result<(), String> {
        if kind == Kind::Claude && claude_entrypoint() {
            command.env("CLAUDE_CODE_ENTRYPOINT", "monocode");
        }
        let (set, remove) = self.env(kind)?;
        for (key, value) in set {
            command.env(key, value);
        }
        for key in remove {
            command.env_remove(key);
        }
        Ok(())
    }
    pub fn identity(&self, kind: Kind) -> Option<Identity> {
        identity::read(self, kind)
    }
    pub fn transfer(&self, to: &Self, kind: Kind, thread: &str) -> Result<(), String> {
        transfer::transfer(self, to, kind, thread)
    }
    pub fn remove_credentials(&self, kind: Kind) -> Result<(), String> {
        let dir = self
            .directory(kind)?
            .ok_or("The default provider account cannot be removed")?;
        if kind == Kind::Claude {
            self.remove_claude_token()?;
        }
        #[cfg(target_os = "macos")]
        if kind == Kind::Claude {
            crate::usage::claude::delete_keychain(&dir)?;
        }
        match std::fs::symlink_metadata(&dir) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.to_string()),
            Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {
                std::fs::remove_dir_all(dir).map_err(|e| e.to_string())
            }
            Ok(_) => std::fs::remove_file(dir).map_err(|e| e.to_string()),
        }
    }
}

pub fn home() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}
pub fn supports_profiles(kind: Kind) -> bool {
    matches!(kind, Kind::Codex | Kind::Claude)
}
pub fn supports_accounts(kind: Kind) -> bool {
    PROVIDERS.contains(&kind)
}
pub fn validate_id(id: &str) -> Result<(), String> {
    if id.is_empty()
        || id.len() > 80
        || !id
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
    {
        Err("Invalid provider account id".into())
    } else {
        Ok(())
    }
}
fn clean_label(label: &str) -> String {
    label
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(48)
        .collect()
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Account {
    pub id: String,
    pub provider: String,
    pub label: String,
}
impl Account {
    pub fn is_default(&self) -> bool {
        self.id == DEFAULT
    }
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Book {
    pub accounts: Vec<Account>,
    pub selections: HashMap<String, HashMap<String, String>>,
    /// Per provider id, the account new sessions use when a project has no
    /// choice of its own. Missing or stale entries fall back to [`DEFAULT`].
    pub defaults: HashMap<String, String>,
}
impl Book {
    pub fn list(&self, kind: Kind) -> Vec<Account> {
        if !supports_accounts(kind) {
            return Vec::new();
        }
        let label = self
            .accounts
            .iter()
            .find(|a| a.provider == kind.id() && a.is_default())
            .map(|a| clean_label(&a.label))
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "CLI login".into());
        let mut rows = vec![Account {
            id: DEFAULT.into(),
            provider: kind.id().into(),
            label,
        }];
        if supports_profiles(kind) {
            for a in self
                .accounts
                .iter()
                .filter(|a| a.provider == kind.id() && !a.is_default())
            {
                let label = clean_label(&a.label);
                if validate_id(&a.id).is_ok()
                    && !label.is_empty()
                    && !rows.iter().any(|row| row.id == a.id)
                {
                    rows.push(Account { label, ..a.clone() });
                }
            }
        }
        rows
    }
    pub fn add(&mut self, kind: Kind, label: &str) -> Result<Account, String> {
        if !supports_profiles(kind) {
            return Err("This provider supports only its default account".into());
        }
        let label = clean_label(label);
        let account = Account {
            id: format!("account-{}", request_id()),
            provider: kind.id().into(),
            label: if label.is_empty() {
                format!("Account {}", self.list(kind).len() + 1)
            } else {
                label
            },
        };
        self.accounts.push(account.clone());
        Ok(account)
    }
    pub fn rename(&mut self, kind: Kind, id: &str, label: &str) -> Result<(), String> {
        let label = clean_label(label);
        if label.is_empty() || !self.exists(kind, id) {
            return Err("Account name is empty or account no longer exists".into());
        }
        if let Some(account) = self
            .accounts
            .iter_mut()
            .find(|a| a.provider == kind.id() && a.id == id)
        {
            account.label = label;
        } else {
            self.accounts.push(Account {
                id: id.into(),
                provider: kind.id().into(),
                label,
            });
        }
        Ok(())
    }
    pub fn exists(&self, kind: Kind, id: &str) -> bool {
        self.list(kind).iter().any(|a| a.id == id)
    }
    pub fn remove(&mut self, kind: Kind, id: &str) -> Result<(), String> {
        if id == DEFAULT || !self.exists(kind, id) {
            return Err("This account cannot be removed".into());
        }
        self.accounts
            .retain(|a| a.provider != kind.id() || a.id != id);
        for selections in self.selections.values_mut() {
            if selections.get(kind.id()).is_some_and(|v| v == id) {
                selections.remove(kind.id());
            }
        }
        if self.defaults.get(kind.id()).is_some_and(|v| v == id) {
            self.defaults.remove(kind.id());
        }
        Ok(())
    }
    /// The account for a new session in `project`: the project's own choice,
    /// else the provider's default account.
    pub fn selected(&self, kind: Kind, project: &str) -> String {
        self.chosen(kind, project)
            .unwrap_or_else(|| self.preferred(kind))
    }
    /// Only the project's explicit, still-existing choice.
    pub(crate) fn chosen(&self, kind: Kind, project: &str) -> Option<String> {
        self.selections
            .get(&project_key(project))
            .and_then(|s| s.get(kind.id()))
            .filter(|id| self.exists(kind, id))
            .cloned()
    }
    /// The account marked as default for `kind`, or the provider CLI profile.
    ///
    /// Only existence is checked, not whether the account can still sign in:
    /// an expired or revoked setup token stays the default, so new sessions
    /// fail authentication until the user saves a new token or picks another
    /// default. That is deliberate; silently moving sessions to a different
    /// account (and its config dir and billing) would be worse.
    pub fn preferred(&self, kind: Kind) -> String {
        self.defaults
            .get(kind.id())
            .filter(|id| self.exists(kind, id))
            .cloned()
            .unwrap_or_else(default_id)
    }
    pub fn set_preferred(&mut self, kind: Kind, id: &str) -> Result<(), String> {
        if !self.exists(kind, id) {
            return Err("Provider account no longer exists".into());
        }
        if id == DEFAULT {
            self.defaults.remove(kind.id());
        } else {
            self.defaults.insert(kind.id().into(), id.into());
        }
        Ok(())
    }
    pub fn select(&mut self, kind: Kind, project: &str, id: &str) -> Result<(), String> {
        if !self.exists(kind, id) {
            return Err("Provider account no longer exists".into());
        }
        self.selections
            .entry(project_key(project))
            .or_default()
            .insert(kind.id().into(), id.into());
        Ok(())
    }
}
/// Why a textual account selector did not name exactly one account.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResolveError {
    /// The provider has no account support at all.
    Unsupported(String),
    /// Two accounts of the provider share the label; the ids are listed.
    Ambiguous {
        provider: String,
        label: String,
        ids: Vec<String>,
    },
    /// The name exists only for another provider.
    OtherProvider {
        provider: String,
        owner: String,
        valid: String,
    },
    /// Nothing matches; `valid` lists this provider's labels and ids.
    NotFound {
        provider: String,
        selector: String,
        valid: String,
    },
}
impl std::fmt::Display for ResolveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unsupported(provider) => {
                write!(f, "{provider} does not support provider accounts")
            }
            Self::Ambiguous {
                provider,
                label,
                ids,
            } => write!(
                f,
                "Several {provider} accounts are named \"{label}\"; use one of these ids: {}",
                ids.join(", ")
            ),
            Self::OtherProvider {
                provider,
                owner,
                valid,
            } => write!(
                f,
                "That account belongs to {owner}, not {provider}. Valid {provider} accounts: {valid}"
            ),
            Self::NotFound {
                provider,
                selector,
                valid,
            } => write!(
                f,
                "No {provider} account matches \"{selector}\". Valid {provider} accounts: {valid}"
            ),
        }
    }
}
fn fold_label(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}
fn describe_accounts(rows: &[Account]) -> String {
    rows.iter()
        .map(|a| format!("{} ({})", a.label, a.id))
        .collect::<Vec<_>>()
        .join(", ")
}
/// Resolve a user-written selector to an account id of `kind`. Accepts, in
/// order: the literal `default`, an exact id, then a case-insensitive exact
/// label (inner whitespace collapsed). Never a prefix or fuzzy match.
pub fn resolve(book: &Book, kind: Kind, selector: &str) -> Result<String, ResolveError> {
    if !supports_accounts(kind) {
        return Err(ResolveError::Unsupported(kind.id().into()));
    }
    let selector = selector.trim();
    let rows = book.list(kind);
    if selector == DEFAULT {
        return Ok(DEFAULT.into());
    }
    if let Some(row) = rows.iter().find(|a| a.id == selector) {
        return Ok(row.id.clone());
    }
    let wanted = fold_label(selector);
    if !wanted.is_empty() {
        let hits: Vec<&Account> = rows
            .iter()
            .filter(|a| fold_label(&a.label) == wanted)
            .collect();
        match hits.as_slice() {
            [one] => return Ok(one.id.clone()),
            [] => (),
            many => {
                return Err(ResolveError::Ambiguous {
                    provider: kind.id().into(),
                    label: selector.into(),
                    ids: many.iter().map(|a| a.id.clone()).collect(),
                })
            }
        }
    }
    let valid = describe_accounts(&rows);
    for other in PROVIDERS.into_iter().filter(|other| *other != kind) {
        let theirs = book.list(other);
        if theirs
            .iter()
            .any(|a| a.id == selector || (!wanted.is_empty() && fold_label(&a.label) == wanted))
        {
            return Err(ResolveError::OtherProvider {
                provider: kind.id().into(),
                owner: other.id().into(),
                valid,
            });
        }
    }
    Err(ResolveError::NotFound {
        provider: kind.id().into(),
        selector: selector.into(),
        valid,
    })
}
/// Parse the `provider` filter of `accounts.list`.
pub fn parse_provider(text: &str) -> Result<Kind, String> {
    Kind::parse(text.trim())
        .filter(|kind| supports_accounts(*kind))
        .ok_or_else(|| {
            let ids: Vec<&str> = PROVIDERS.iter().map(|kind| kind.id()).collect();
            format!(
                "provider must be one of: {} (the providers with accounts)",
                ids.join(", ")
            )
        })
}
/// How an account signs in: "cli" (the provider CLI's own login), "setupToken" or "apiKey".
pub fn auth_kind(kind: Kind, credential: Option<ClaudeCredential>) -> &'static str {
    match (kind, credential) {
        (Kind::Claude, Some(ClaudeCredential::SetupToken)) => "setupToken",
        (Kind::Claude, Some(ClaudeCredential::ApiKey)) => "apiKey",
        _ => "cli",
    }
}
/// The `accounts.list` result: ids, labels and credential kinds only, never a
/// token, an email or a path. `claude_credential` reports the kind of a named
/// Claude account; anything without one (and every other provider) is "cli".
pub fn list_value(
    book: &Book,
    provider: Option<Kind>,
    claude_credential: &dyn Fn(&str) -> Option<ClaudeCredential>,
) -> serde_json::Value {
    let kinds: Vec<Kind> = match provider {
        Some(kind) => vec![kind],
        None => PROVIDERS.to_vec(),
    };
    let mut rows = Vec::new();
    for kind in kinds {
        let preferred = book.preferred(kind);
        for account in book.list(kind) {
            let auth = auth_kind(kind, claude_credential(&account.id));
            rows.push(serde_json::json!({
                "provider": kind.id(),
                "id": account.id,
                "label": account.label,
                "isDefault": account.id == preferred,
                "authKind": auth,
            }));
        }
    }
    serde_json::json!({ "accounts": rows })
}
fn project_key(project: &str) -> String {
    let key = project.trim().replace('\\', "/");
    let key = key.trim_end_matches('/');
    let key = if key.is_empty() { "~" } else { key };
    if cfg!(windows) {
        key.to_lowercase()
    } else {
        key.into()
    }
}
