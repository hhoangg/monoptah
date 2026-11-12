//! Which Docker account a sandbox lives in.
//!
//! A sandbox belongs to the Docker account that created it, so every later `sbx` or ssh call
//! must run as that account, whatever the project names today. The creating account is
//! pinned per sandbox NAME in a native-store setting (`sandbox-docker-account:<name>`) when
//! `create` succeeds or an existing sandbox is adopted, and cleared when the sandbox is
//! deleted. A sandbox without a pin (created before accounts existed, or with the machine
//! login) uses the machine login: `None`.
//!
//! The setting holds only the account id and its Docker ID, never a token.
use crate::store::Store;
use serde::{Deserialize, Serialize};

/// Prefix of the per-sandbox setting.
const PIN_PREFIX: &str = "sandbox-docker-account:";

/// The setting that pins the Docker account of sandbox `name`.
pub fn pin_key(name: &str) -> String {
    format!("{PIN_PREFIX}{name}")
}

/// What the pin setting holds: the account id and the Docker ID it had when pinned. The
/// Docker ID lets a sandbox follow its owner when the account was removed and added again
/// (a new id for the same Docker ID). An older setting is the bare account id.
#[derive(Serialize, Deserialize)]
struct Pin {
    account: String,
    #[serde(default, rename = "dockerId", skip_serializing_if = "Option::is_none")]
    docker_id: Option<String>,
}

fn read_pin(store: &Store, name: &str) -> Option<Pin> {
    let raw = store.setting(&pin_key(name)).ok().flatten()?;
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    if raw.starts_with('{') {
        let pin: Pin = serde_json::from_str(raw).ok()?;
        let account = pin.account.trim().to_owned();
        return (!account.is_empty()).then_some(Pin { account, ..pin });
    }
    Some(Pin {
        account: raw.to_owned(),
        docker_id: None,
    })
}

/// The Docker ID of a saved account: from the process-wide accounts, else from the store's
/// own credentials database.
fn docker_id_of(store: &Store, account: &str) -> Option<String> {
    crate::docker_accounts::shared()
        .and_then(|access| access.docker_id(account))
        .or_else(|| {
            crate::docker_accounts::DockerAccounts::for_store(store)
                .ok()?
                .get(account)
                .ok()
                .map(|a| a.docker_id)
        })
}

/// The id of the saved account with this Docker ID, if there is one.
fn account_of_docker_id(store: &Store, docker_id: &str) -> Option<String> {
    crate::docker_accounts::shared()
        .and_then(|access| access.account_for_docker_id(docker_id))
        .or_else(|| {
            crate::docker_accounts::DockerAccounts::for_store(store)
                .ok()?
                .list()
                .ok()?
                .into_iter()
                .find(|a| a.docker_id == docker_id)
                .map(|a| a.id)
        })
}

/// The pinned account of sandbox `name`; `None` for a sandbox without a pin or when the
/// setting cannot be read. When the pinned account no longer exists but an account with the
/// same Docker ID does, that one (the owner re-added the token).
pub fn pinned(store: &Store, name: &str) -> Option<String> {
    let pin = read_pin(store, name)?;
    if docker_id_of(store, &pin.account).is_some() {
        return Some(pin.account);
    }
    pin.docker_id
        .as_deref()
        .and_then(|docker_id| account_of_docker_id(store, docker_id))
        .or(Some(pin.account))
}

/// Pins sandbox `name` to `account`, together with the account's Docker ID. A `None` account
/// clears an existing pin: such a sandbox is reached with the machine login, which is also
/// what no pin means. An unwritable setting is an error: the caller must not go on to create
/// a sandbox it could not find again.
pub fn pin(store: &Store, name: &str, account: Option<&str>) -> Result<(), String> {
    let Some(account) = account else {
        return clear_pin(store, name);
    };
    let value = serde_json::to_string(&Pin {
        account: account.to_owned(),
        docker_id: docker_id_of(store, account),
    })
    .map_err(|e| e.to_string())?;
    store
        .save_setting(&pin_key(name), &value)
        .map_err(|_| "Could not save which Docker account this sandbox belongs to.".to_string())
}

/// Forgets the pin of sandbox `name` (the sandbox was deleted).
pub fn clear_pin(store: &Store, name: &str) -> Result<(), String> {
    if store.setting(&pin_key(name)).ok().flatten().is_some() {
        store.save_setting(&pin_key(name), "").map_err(|_| {
            "Could not forget which Docker account this sandbox belonged to.".to_string()
        })?;
    }
    Ok(())
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
