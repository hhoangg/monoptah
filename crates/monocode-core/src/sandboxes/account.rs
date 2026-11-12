//! Which Docker account a sandbox lives in.
//!
//! A sandbox belongs to the Docker account that created it, so every later `sbx` or ssh call
//! must run as that account, whatever the project names today. The creating account is
//! pinned per sandbox NAME in a native-store setting (`sandbox-docker-account:<name>`) when
//! `create` succeeds or an existing sandbox is adopted, and cleared when the sandbox is
//! deleted. A sandbox without a pin (created before accounts existed, or with the machine
//! login) uses the machine login: `None`.
//!
//! The setting holds only the account id, never a token.
use crate::store::Store;

/// Prefix of the per-sandbox setting.
const PIN_PREFIX: &str = "sandbox-docker-account:";

/// The setting that pins the Docker account of sandbox `name`.
pub fn pin_key(name: &str) -> String {
    format!("{PIN_PREFIX}{name}")
}

/// The pinned account of sandbox `name`; `None` for a sandbox without a pin or when the
/// setting cannot be read.
pub fn pinned(store: &Store, name: &str) -> Option<String> {
    store
        .setting(&pin_key(name))
        .ok()
        .flatten()
        .map(|id| id.trim().to_owned())
        .filter(|id| !id.is_empty())
}

/// Pins sandbox `name` to `account`. A `None` account writes nothing: such a sandbox is
/// reached with the machine login, which is also what no pin means.
pub fn pin(store: &Store, name: &str, account: Option<&str>) {
    if let Some(account) = account {
        let _ = store.save_setting(&pin_key(name), account);
    }
}

/// Forgets the pin of sandbox `name` (the sandbox was deleted).
pub fn clear_pin(store: &Store, name: &str) {
    if store.setting(&pin_key(name)).ok().flatten().is_some() {
        let _ = store.save_setting(&pin_key(name), "");
    }
}

/// How to name the account of a sandbox to the user: its Docker ID when the account is
/// still saved, else a generic phrase. Never a token.
pub fn describe(store: &Store, account: &str) -> String {
    crate::docker_accounts::DockerAccounts::for_store(store)
        .and_then(|accounts| accounts.get(account))
        .map(|a| format!("Docker account {}", a.docker_id))
        .unwrap_or_else(|_| "its Docker account".to_string())
}

#[cfg(test)]
mod tests;
