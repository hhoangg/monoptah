//! Per-project configuration of the Docker sandbox cloud backend: which repo to
//! clone, which Git credential reads it, and the sandbox size and lifetime.
//!
//! Stored in the native store setting [`SETTING`] as a JSON map keyed by the
//! local project's cwd (like the other per-project cloud settings). A config
//! never holds a secret: `credential_id` only names a profile saved through
//! [`crate::git_credentials`]. A public repo uses `credential_id: None`.
use crate::store::Store;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Native store setting holding the cwd -> config map.
pub const SETTING: &str = "docker-sandbox-projects.v1";
pub const DEFAULT_DEVCONTAINER: &str = ".devcontainer/devcontainer.json";
pub const DEFAULT_CPUS: u32 = 2;
pub const DEFAULT_MEMORY: &str = "4g";
pub const DEFAULT_TTL: &str = "2h";

const MAX_CPUS: u32 = 16;
const MIN_MEMORY_MIB: u64 = 512;
const MAX_MEMORY_MIB: u64 = 64 * 1024;
/// Docker caps a sandbox's remaining time at 24 hours.
const MAX_TTL_MINUTES: u64 = 24 * 60;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct DockerProjectConfig {
    pub repo_url: String,
    pub credential_id: Option<String>,
    /// Branch, tag or commit to check out; empty means the repo's default branch.
    pub base_ref: String,
    pub devcontainer_path: String,
    pub cpus: u32,
    pub memory: String,
    pub ttl: String,
    pub platform: Option<String>,
    pub claude_account: Option<String>,
}
impl Default for DockerProjectConfig {
    fn default() -> Self {
        Self {
            repo_url: String::new(),
            credential_id: None,
            base_ref: String::new(),
            devcontainer_path: DEFAULT_DEVCONTAINER.into(),
            cpus: DEFAULT_CPUS,
            memory: DEFAULT_MEMORY.into(),
            ttl: DEFAULT_TTL.into(),
            platform: None,
            claude_account: None,
        }
    }
}

/// `<digits><unit>`: ASCII digits only (no sign), exactly one trailing unit.
fn number_with_unit(value: &str, unit: char) -> Option<u64> {
    let digits = value.strip_suffix(unit)?;
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok()
}
/// `4g`, `512m`: whole mebibytes or gibibytes.
fn memory_mib(value: &str) -> Option<u64> {
    number_with_unit(value, 'm').or_else(|| number_with_unit(value, 'g')?.checked_mul(1024))
}
/// `30m`, `2h`: minutes or hours.
fn ttl_minutes(value: &str) -> Option<u64> {
    number_with_unit(value, 'm').or_else(|| number_with_unit(value, 'h')?.checked_mul(60))
}
/// Letters, digits and `. _ / -` only: safe to place in a shell command.
fn path_chars(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'/' | b'-'))
}
/// A branch, tag or commit name under `git check-ref-format` rules.
fn valid_ref(value: &str) -> bool {
    path_chars(value)
        && value.len() <= 200
        && !value.contains("..")
        && !value.contains("//")
        && !value.starts_with(['-', '/'])
        && !value.ends_with(['/', '.'])
        && !value.ends_with(".lock")
        && value
            .split('/')
            .all(|part| !part.starts_with('.') && !part.ends_with(".lock"))
}

impl DockerProjectConfig {
    /// Trim the text fields and check every value. Returns the cleaned config.
    pub fn validated(&self) -> Result<Self, String> {
        let mut cfg = self.clone();
        cfg.repo_url = cfg.repo_url.trim().to_owned();
        cfg.base_ref = cfg.base_ref.trim().to_owned();
        cfg.devcontainer_path = cfg.devcontainer_path.trim().to_owned();
        cfg.memory = cfg.memory.trim().to_lowercase();
        cfg.ttl = cfg.ttl.trim().to_lowercase();
        for field in [
            &mut cfg.credential_id,
            &mut cfg.platform,
            &mut cfg.claude_account,
        ] {
            *field = field
                .take()
                .map(|v| v.trim().to_owned())
                .filter(|v| !v.is_empty());
        }

        // Rejects embedded credentials, non-HTTPS and unsupported hosts.
        crate::git_credentials::normalize_origin(&cfg.repo_url)?;
        if !cfg.base_ref.is_empty() && !valid_ref(&cfg.base_ref) {
            return Err("The base branch contains invalid characters.".into());
        }
        let path = &cfg.devcontainer_path;
        if !path_chars(path)
            || path.len() > 200
            || path.starts_with('/')
            || path.split('/').any(|part| matches!(part, "" | "." | ".."))
        {
            return Err("The devcontainer path must be relative to the repository root.".into());
        }
        if !(1..=MAX_CPUS).contains(&cfg.cpus) {
            return Err(format!("CPUs must be between 1 and {MAX_CPUS}."));
        }
        match memory_mib(&cfg.memory) {
            Some(mib) if (MIN_MEMORY_MIB..=MAX_MEMORY_MIB).contains(&mib) => {}
            _ => return Err("Memory must look like 512m or 4g, between 512m and 64g.".into()),
        }
        match ttl_minutes(&cfg.ttl) {
            Some(minutes) if (1..=MAX_TTL_MINUTES).contains(&minutes) => {}
            _ => return Err("Lifetime must look like 30m or 2h, at most 24h.".into()),
        }
        if let Some(platform) = &cfg.platform {
            let parts: Vec<&str> = platform.split('/').collect();
            if !(2..=3).contains(&parts.len())
                || parts
                    .iter()
                    .any(|p| p.is_empty() || !p.bytes().all(|b| b.is_ascii_alphanumeric()))
            {
                return Err("The platform must look like linux/amd64.".into());
            }
        }
        if let Some(id) = &cfg.credential_id {
            if !id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') || id.len() > 64 {
                return Err("The Git credential reference is invalid.".into());
            }
        }
        Ok(cfg)
    }
}

/// The stored map. A missing setting is empty; an unreadable one is an error so
/// a save never overwrites it.
fn load_map(store: &Store) -> Result<BTreeMap<String, DockerProjectConfig>, String> {
    match store.setting(SETTING)? {
        None => Ok(BTreeMap::new()),
        Some(raw) => serde_json::from_str(&raw)
            .map_err(|_| "The saved Docker sandbox settings are unreadable.".to_owned()),
    }
}
fn save_map(store: &Store, map: &BTreeMap<String, DockerProjectConfig>) -> Result<(), String> {
    store.save_setting(
        SETTING,
        &serde_json::to_string(map).map_err(|e| e.to_string())?,
    )
}

/// The config of the project at `cwd`, if one was saved. A stored config that
/// no longer validates is an error.
pub fn load(store: &Store, cwd: &str) -> Result<Option<DockerProjectConfig>, String> {
    load_map(store)?
        .remove(cwd)
        .map(|config| config.validated())
        .transpose()
}

/// Validate and save the config of the project at `cwd`. Returns what was stored.
pub fn save(
    store: &Store,
    cwd: &str,
    config: &DockerProjectConfig,
) -> Result<DockerProjectConfig, String> {
    let config = config.validated()?;
    let mut map = load_map(store)?;
    map.insert(cwd.to_owned(), config.clone());
    save_map(store, &map)?;
    Ok(config)
}

/// Forget the config of the project at `cwd`.
pub fn clear(store: &Store, cwd: &str) -> Result<(), String> {
    let mut map = load_map(store)?;
    if map.remove(cwd).is_some() {
        save_map(store, &map)?;
    }
    Ok(())
}
