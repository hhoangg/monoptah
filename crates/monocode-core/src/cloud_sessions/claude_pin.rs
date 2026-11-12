//! Which app Claude account a cloud instance runs under.
//!
//! The account is chosen per project (Cloud settings) and can be overridden for one session
//! (the composer, or a Mono's `account`). Each instance records its choice by NAME in a
//! native-store setting, `codespace-claude-account:<codespace name>` or
//! `sandbox-claude-account:<sandbox name>`, as `{"account": <id|null>, "pinned": <bool>}`:
//!
//! * `pinned: true`: this session overrode the project; the instance keeps that account on
//!   every reconnect, wake and Host upgrade.
//! * `pinned: false`: the instance followed the project default when it was created; a later
//!   reconnect follows the project's CURRENT choice ("applies from the next start or
//!   reconnect"; a running Host keeps its account until then).
//!
//! The pin is written when a start creates or adopts the instance and cleared when the
//! instance is forgotten. It holds an account id only, never a credential. [`effective`] is
//! the one resolution rule both backends use.
use super::docker_start::{store_claude_auth, ClaudeAuth};
use crate::remote::codespace_claude::{ClaudeDelivery, CodespaceClaude};
use crate::store::Store;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};

const CODESPACE_PREFIX: &str = "codespace-claude-account:";
const SANDBOX_PREFIX: &str = "sandbox-claude-account:";

/// The setting that pins the Claude account of codespace `name`.
pub fn codespace_key(name: &str) -> String {
    format!("{CODESPACE_PREFIX}{name}")
}

/// The setting that pins the Claude account of sandbox `name`.
pub fn sandbox_key(name: &str) -> String {
    format!("{SANDBOX_PREFIX}{name}")
}

/// What an instance's pin setting holds.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClaudePin {
    /// App Claude account id; `None` is no account (a codespace then uses the Codespaces
    /// secret, a sandbox gets no Claude credential).
    #[serde(default)]
    pub account: Option<String>,
    /// A per-session override rather than the project default at creation time.
    #[serde(default)]
    pub pinned: bool,
}

/// The pin saved under `key`; `None` when there is none or it cannot be read (an instance
/// from before pins existed).
pub fn read(store: &Store, key: &str) -> Option<ClaudePin> {
    let raw = store.setting(key).ok().flatten()?;
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    serde_json::from_str::<ClaudePin>(raw)
        .ok()
        .map(|pin| ClaudePin {
            account: pin
                .account
                .map(|a| a.trim().to_owned())
                .filter(|a| !a.is_empty()),
            ..pin
        })
}

/// Saves `pin` under `key`. An unwritable setting is an error: an instance whose account
/// could not be recorded would later run under whatever the project says then.
pub fn write(store: &Store, key: &str, pin: &ClaudePin) -> Result<(), String> {
    let value = serde_json::to_string(pin).map_err(|e| e.to_string())?;
    store
        .save_setting(key, &value)
        .map_err(|_| "Could not save which Claude account this session uses.".to_string())
}

/// Forgets the pin under `key` (the instance was deleted).
pub fn clear(store: &Store, key: &str) -> Result<(), String> {
    if store
        .setting(key)
        .ok()
        .flatten()
        .is_some_and(|v| !v.is_empty())
    {
        store
            .save_setting(key, "")
            .map_err(|_| "Could not forget which Claude account this session used.".to_string())?;
    }
    Ok(())
}

/// The account an instance runs under. `pin` is its saved pin and `project` is the project's
/// CURRENT choice (`Some(None)`: the project chooses no account; `None`: the instance has no
/// known project).
///
/// * a pin with `pinned: true` wins, whatever the project says now;
/// * else the project's current choice;
/// * else (no project) what the pin recorded, and with no pin either (a legacy instance) no
///   account at all: today's behavior.
pub fn effective(pin: Option<&ClaudePin>, project: Option<&Option<String>>) -> Option<String> {
    match (pin, project) {
        (Some(pin), _) if pin.pinned => pin.account.clone(),
        (_, Some(choice)) => choice.clone(),
        (Some(pin), None) => pin.account.clone(),
        (None, None) => None,
    }
}

/// The local project a codespace belongs to, from the remote project bound to it.
fn project_of_codespace(store: &Store, name: &str) -> Option<String> {
    store
        .remote_projects()
        .ok()?
        .into_values()
        .find(|p| p.sandbox.is_none() && p.codespace.as_deref() == Some(name))
        .and_then(|p| p.local_project)
}

/// The account codespace `name` runs under now, see [`effective`]. An unreadable project
/// setting counts as no project, like a start does.
pub fn codespace_account(store: &Store, name: &str) -> Option<String> {
    let pin = read(store, &codespace_key(name));
    let project = project_of_codespace(store, name).and_then(|cwd| {
        super::project_cloud::load_codespace(store, &cwd)
            .ok()
            .map(|c| c.codespace.claude_account)
    });
    effective(pin.as_ref(), project.as_ref())
}

/// How to name app Claude account `id` to the user: its label, else the id.
pub fn label(store: &Store, id: &str) -> String {
    store
        .provider_accounts()
        .ok()
        .and_then(|book| {
            book.list(crate::provider::Kind::Claude)
                .into_iter()
                .find(|a| a.id == id)
                .map(|a| a.label)
        })
        .unwrap_or_else(|| id.to_owned())
}

/// The credential of `account`, or a clear error naming the account and where to fix it. A
/// deleted account and one without a token or key are both errors: a cloud instance never
/// falls back to another credential silently.
pub fn require_auth(store: &Store, account: &str) -> Result<ClaudeAuth, String> {
    let exists = store.provider_accounts().is_ok_and(|book| {
        book.list(crate::provider::Kind::Claude)
            .iter()
            .any(|a| a.id == account)
    });
    let name = label(store, account);
    if !exists {
        return Err(
            "The Claude account chosen for this project is no longer available. Choose another account in Cloud settings."
                .to_string(),
        );
    }
    match store_claude_auth(store, account) {
        Ok(Some(auth)) => Ok(auth),
        Ok(None) => Err(format!(
            "Claude account \"{name}\" has no setup token or API key saved. Choose another account in Cloud settings."
        )),
        Err(_) => Err(format!(
            "The credential of Claude account \"{name}\" could not be read. Choose another account in Cloud settings."
        )),
    }
}

/// Resolves the Claude account of each codespace from the app's own store. Installed on
/// [`crate::remote::Connections`] by the app.
pub struct StoreCodespaceClaude {
    store: Arc<Mutex<Store>>,
}

impl StoreCodespaceClaude {
    pub fn new(store: Arc<Mutex<Store>>) -> Self {
        Self { store }
    }
}

impl CodespaceClaude for StoreCodespaceClaude {
    fn resolve(&self, codespace: &str) -> Result<Option<ClaudeDelivery>, String> {
        let store = self
            .store
            .lock()
            .map_err(|_| "Native session store unavailable".to_string())?;
        let Some(account) = codespace_account(&store, codespace) else {
            return Ok(None);
        };
        let auth = require_auth(&store, &account)?;
        Ok(Some(ClaudeDelivery {
            account,
            prelude: auth.prelude(),
            secret: zeroize::Zeroizing::new(auth.secret().to_owned()),
        }))
    }
}

impl super::CloudSessions {
    /// How to name app Claude account `id` to the user (its label, else the id).
    pub fn claude_account_label(&self, id: &str) -> String {
        self.store()
            .map(|store| label(&store, id))
            .unwrap_or_else(|_| id.to_owned())
    }
}
