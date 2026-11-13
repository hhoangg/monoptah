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

/// The pin of sandbox `name`. `Ok(None)` is "no pin" (never written or cleared); an `Err` is
/// a pin that could not be read (store or content): never to be taken for "no account".
fn read_pin(store: &Store, name: &str) -> Result<Option<Pin>, String> {
    let unreadable = || "Could not read which Docker account this sandbox belongs to.".to_string();
    let Some(raw) = store.setting(&pin_key(name)).map_err(|_| unreadable())? else {
        return Ok(None);
    };
    let raw = raw.trim();
    if raw.is_empty() {
        return Ok(None);
    }
    if raw.starts_with('{') {
        let pin: Pin = serde_json::from_str(raw).map_err(|_| unreadable())?;
        let account = pin.account.trim().to_owned();
        return Ok((!account.is_empty()).then_some(Pin { account, ..pin }));
    }
    Ok(Some(Pin {
        account: raw.to_owned(),
        docker_id: None,
    }))
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
                .find(|a| a.docker_id.eq_ignore_ascii_case(docker_id))
                .map(|a| a.id)
        })
}

/// The pinned account of sandbox `name`; `Ok(None)` for a sandbox without a pin. When the
/// pinned account no longer exists but an account with the same Docker ID does, that one
/// (the owner re-added the token). `Err` when the pin cannot be read: callers must not fall
/// back to the machine login on it.
pub fn pinned(store: &Store, name: &str) -> Result<Option<String>, String> {
    let Some(pin) = read_pin(store, name)? else {
        return Ok(None);
    };
    if docker_id_of(store, &pin.account).is_some() {
        return Ok(Some(pin.account));
    }
    Ok(pin
        .docker_id
        .as_deref()
        .and_then(|docker_id| account_of_docker_id(store, docker_id))
        .or(Some(pin.account)))
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

/// Forgets the pin of sandbox `name` (the sandbox was deleted): the setting row is deleted.
pub fn clear_pin(store: &Store, name: &str) -> Result<(), String> {
    store
        .delete_setting(&pin_key(name))
        .map_err(|_| "Could not forget which Docker account this sandbox belonged to.".to_string())
}

/// Names of the live sandboxes (ones the app still has a cloud project for) pinned to
/// Docker account `account_id`, sorted. They live in that account, so removing it would
/// strand them even when no project names it.
pub fn sandboxes_pinned_to(store: &Store, account_id: &str) -> Result<Vec<String>, String> {
    let live: std::collections::BTreeSet<String> = store
        .remote_projects()?
        .into_values()
        .filter_map(|p| p.sandbox)
        .collect();
    let mut found = Vec::new();
    // The keys come back without the prefix: they are the sandbox names.
    for (name, _) in store.settings_with_key_prefix(PIN_PREFIX)? {
        if !live.contains(&name) {
            continue;
        }
        if read_pin(store, &name)?.is_some_and(|pin| pin.account == account_id) {
            found.push(name);
        }
    }
    found.sort();
    Ok(found)
}

/// The user-facing error for sandbox `name` when the Docker account it was created with was
/// removed (and no saved account has its Docker ID). `None` when the sandbox has no pin, the
/// account still exists, or the pin predates the recorded Docker ID (then the generic account
/// error applies).
pub fn removed_account_error(store: &Store, name: &str) -> Option<String> {
    let pin = read_pin(store, name).ok().flatten()?;
    let docker_id = pin.docker_id?;
    if docker_id_of(store, &pin.account).is_some()
        || account_of_docker_id(store, &docker_id).is_some()
    {
        return None;
    }
    Some(format!(
        "Sandbox {name} was created with Docker account {docker_id}, which was removed. Add a token for {docker_id} again in Settings > Cloud."
    ))
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
