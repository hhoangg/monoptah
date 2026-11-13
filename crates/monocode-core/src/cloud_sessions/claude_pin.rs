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
const DELIVERED_PREFIX: &str = "codespace-claude-delivered:";

/// The setting that pins the Claude account of codespace `name`.
pub fn codespace_key(name: &str) -> String {
    format!("{CODESPACE_PREFIX}{name}")
}

/// The setting that holds the id of the account the Host of codespace `name` was last
/// bootstrapped with (an id only; empty or missing: the Codespaces secret, or unknown).
pub fn delivered_key(name: &str) -> String {
    format!("{DELIVERED_PREFIX}{name}")
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
    parse(&store.setting(key).ok().flatten()?)
}

fn parse(raw: &str) -> Option<ClaudePin> {
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

/// Forgets the pin under `key` (the instance was deleted): the setting row is deleted.
pub fn clear(store: &Store, key: &str) -> Result<(), String> {
    store
        .delete_setting(key)
        .map_err(|_| "Could not forget which Claude account this session used.".to_string())
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

/// The account codespace `name` runs under now, see [`effective`], and whether it comes from
/// this session's own pinned override. An unreadable project setting counts as no project,
/// like a start does.
pub fn codespace_account_choice(store: &Store, name: &str) -> (Option<String>, bool) {
    let pin = read(store, &codespace_key(name));
    let project = project_of_codespace(store, name).and_then(|cwd| {
        super::project_cloud::load_codespace(store, &cwd)
            .ok()
            .map(|c| c.codespace.claude_account)
    });
    let pinned = pin.as_ref().is_some_and(|pin| pin.pinned);
    (effective(pin.as_ref(), project.as_ref()), pinned)
}

/// The account codespace `name` runs under now, see [`codespace_account_choice`].
pub fn codespace_account(store: &Store, name: &str) -> Option<String> {
    codespace_account_choice(store, name).0
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

/// Why an account cannot be used.
#[derive(Clone, Copy)]
enum Problem {
    Gone,
    NoToken,
    Unreadable,
}

/// The error for account `name`. A pinned account belongs to one cloud session (fixing the
/// project setting would not help), so that wording points to a new session instead of Cloud
/// settings.
fn problem_message(name: &str, pinned: bool, problem: Problem) -> String {
    if pinned {
        let what = match problem {
            Problem::Gone => "is no longer available",
            Problem::NoToken => "has no setup token or API key saved",
            Problem::Unreadable => "has a credential that could not be read",
        };
        return format!(
            "This cloud session uses Claude account \"{name}\", which {what}. Start a new cloud session or add the account back."
        );
    }
    match problem {
        Problem::Gone => "The Claude account chosen for this project is no longer available. Choose another account in Cloud settings.".to_string(),
        Problem::NoToken => format!("Claude account \"{name}\" has no setup token or API key saved. Choose another account in Cloud settings."),
        Problem::Unreadable => format!("The credential of Claude account \"{name}\" could not be read. Choose another account in Cloud settings."),
    }
}

/// Checks that `account` exists and has a setup token or API key saved, WITHOUT reading the
/// credential (metadata only, so it is cheap). `pinned`: the account belongs to one cloud
/// session, see [`require_auth_for`].
pub fn check_account(store: &Store, account: &str, pinned: bool) -> Result<(), String> {
    let exists = store.provider_accounts().is_ok_and(|book| {
        book.list(crate::provider::Kind::Claude)
            .iter()
            .any(|a| a.id == account)
    });
    let name = label(store, account);
    if !exists {
        return Err(problem_message(&name, pinned, Problem::Gone));
    }
    let has_token = store.data_dir().is_some_and(|root| {
        crate::accounts::Profile::new(account, root)
            .uses_claude_token()
            .unwrap_or(true)
    });
    if !has_token {
        return Err(problem_message(&name, pinned, Problem::NoToken));
    }
    Ok(())
}

/// The error for an `account` whose credential came back empty: the account is gone or has
/// no setup token or API key. Pairs with a [`SandboxSecrets`](super::docker_start::SandboxSecrets)
/// that answers "no credential" for a chosen account, which a cloud instance never accepts
/// silently.
pub fn missing_auth_error(store: &Store, account: &str, pinned: bool) -> String {
    match check_account(store, account, pinned) {
        Err(error) => error,
        Ok(()) => problem_message(&label(store, account), pinned, Problem::NoToken),
    }
}

/// The credential of `account`, or a clear error naming the account and where to fix it. A
/// deleted account and one without a token or key are both errors: a cloud instance never
/// falls back to another credential silently.
pub fn require_auth(store: &Store, account: &str) -> Result<ClaudeAuth, String> {
    require_auth_for(store, account, false)
}

/// [`require_auth`] for an account that is `pinned` to one cloud session or instance: the
/// error then says the account belongs to this session.
pub fn require_auth_for(store: &Store, account: &str, pinned: bool) -> Result<ClaudeAuth, String> {
    check_account(store, account, pinned)?;
    let name = label(store, account);
    match store_claude_auth(store, account) {
        Ok(Some(auth)) => Ok(auth),
        Ok(None) => Err(problem_message(&name, pinned, Problem::NoToken)),
        Err(_) => Err(problem_message(&name, pinned, Problem::Unreadable)),
    }
}

/// Resolves the Claude account of each codespace from the app's own store. Installed on
/// [`crate::remote::Connections`] by the app.
pub struct StoreCodespaceClaude {
    store: Arc<Mutex<Store>>,
    /// Where the state of a codespace is read (`gh codespace view`); without it no codespace
    /// is known to be stopped.
    gh: Option<Arc<dyn crate::codespaces::GhRunner>>,
}

impl StoreCodespaceClaude {
    pub fn new(store: Arc<Mutex<Store>>) -> Self {
        Self { store, gh: None }
    }

    /// Reads codespace states through `gh`, so a shut-down codespace is known to run no Host.
    pub fn with_gh(mut self, gh: Arc<dyn crate::codespaces::GhRunner>) -> Self {
        self.gh = Some(gh);
        self
    }
}

impl CodespaceClaude for StoreCodespaceClaude {
    fn is_stopped(&self, codespace: &str) -> bool {
        self.gh.as_ref().is_some_and(|gh| {
            crate::codespaces::state(&**gh, codespace)
                .is_ok_and(|state| matches!(state.as_str(), "Shutdown" | "ShuttingDown"))
        })
    }

    fn account_id(&self, codespace: &str) -> Result<Option<String>, String> {
        let store = self
            .store
            .lock()
            .map_err(|_| "Native session store unavailable".to_string())?;
        let (account, pinned) = codespace_account_choice(&store, codespace);
        let Some(account) = account else {
            return Ok(None);
        };
        check_account(&store, &account, pinned)?;
        Ok(Some(account))
    }

    fn resolve(&self, codespace: &str) -> Result<Option<ClaudeDelivery>, String> {
        let store = self
            .store
            .lock()
            .map_err(|_| "Native session store unavailable".to_string())?;
        let (account, pinned) = codespace_account_choice(&store, codespace);
        let Some(account) = account else {
            return Ok(None);
        };
        delivery_of(&store, account, pinned).map(Some)
    }

    fn resolve_account(
        &self,
        codespace: &str,
        account: &str,
    ) -> Result<Option<ClaudeDelivery>, String> {
        let store = self
            .store
            .lock()
            .map_err(|_| "Native session store unavailable".to_string())?;
        let pinned = read(&store, &codespace_key(codespace)).is_some_and(|pin| pin.pinned);
        delivery_of(&store, account.to_owned(), pinned).map(Some)
    }

    fn delivered_hint(&self, codespace: &str) -> Option<String> {
        let store = self.store.lock().ok()?;
        store
            .setting(&delivered_key(codespace))
            .ok()
            .flatten()
            .map(|id| id.trim().to_owned())
            .filter(|id| !id.is_empty())
    }

    fn note_delivered(&self, codespace: &str, account: Option<&str>) {
        let Ok(store) = self.store.lock() else {
            return;
        };
        let key = delivered_key(codespace);
        let _ = match account {
            Some(account) => store.save_setting(&key, account),
            None => clear(&store, &key),
        };
    }
}

fn delivery_of(store: &Store, account: String, pinned: bool) -> Result<ClaudeDelivery, String> {
    let auth = require_auth_for(store, &account, pinned)?;
    Ok(ClaudeDelivery {
        account,
        prelude: auth.prelude(),
        secret: zeroize::Zeroizing::new(auth.secret().to_owned()),
    })
}

/// What a codespace start records as the instance's Claude account.
#[derive(Default)]
pub(super) struct StartPin {
    /// The pin this start writes.
    pub pin: ClaudePin,
    /// An existing instance's own per-session override, which a start that did not choose an
    /// account keeps (nothing is written then, and this is the account to validate).
    pub keep: Option<ClaudePin>,
}

impl StartPin {
    /// An account that cannot be used fails the start before a codespace is created for it.
    /// A kept pin validates its own account, never the project's.
    pub(super) fn validate(&self, store: &Store) -> Result<(), String> {
        let pin = self.keep.as_ref().unwrap_or(&self.pin);
        match &pin.account {
            Some(account) => require_auth_for(store, account, pin.pinned).map(|_| ()),
            None => Ok(()),
        }
    }

    /// Records the pin on codespace `name`, unless an existing override is kept.
    pub(super) fn write(&self, store: &Store, name: &str) -> Result<(), String> {
        if self.keep.is_some() {
            return Ok(());
        }
        write(store, &codespace_key(name), &self.pin)
    }
}

impl super::CloudSessions {
    /// The pin a codespace start of `target` records: this session's own choice, else the
    /// project's. An existing codespace that already carries a per-session override keeps it
    /// when this start did not choose one.
    pub(super) fn codespace_start_pin(
        &self,
        local_cwd: &str,
        target: &super::Target,
    ) -> Result<StartPin, String> {
        let pin = if target.claude_account_pinned {
            ClaudePin {
                account: target.claude_account.clone(),
                pinned: true,
            }
        } else {
            ClaudePin {
                account: self.codespace_project_config(local_cwd).claude_account,
                pinned: false,
            }
        };
        let keep = match &target.codespace {
            super::CodespaceChoice::Named(name) if !pin.pinned => {
                read(&*self.store()?, &codespace_key(name)).filter(|existing| existing.pinned)
            }
            _ => None,
        };
        Ok(StartPin { pin, keep })
    }

    /// How to name app Claude account `id` to the user (its label, else the id).
    pub fn claude_account_label(&self, id: &str) -> String {
        self.store()
            .map(|store| label(&store, id))
            .unwrap_or_else(|_| id.to_owned())
    }
}
