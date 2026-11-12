//! Per-project cloud settings of both backends behind one facade.
//!
//! The sandbox part stays in [`docker_config`] (setting `docker-sandbox-projects.v1`). The
//! codespace part and the project's default backend live in the native store setting
//! [`SETTING`], a JSON map keyed by the local project's cwd. Nothing here holds a secret:
//! account fields only name accounts saved elsewhere.
use super::docker_config::{self, DockerProjectConfig};
use crate::store::Store;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

/// Native store setting holding the cwd -> [`StoredProject`] map.
pub const SETTING: &str = "cloud-projects.v1";
/// Minutes a codespace may sit idle before it stops, when the project sets nothing.
pub const DEFAULT_IDLE_TIMEOUT_MINUTES: u32 = crate::codespaces::DEFAULT_IDLE_TIMEOUT_MINUTES;

/// Which backend new cloud sessions of a project use by default.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DefaultBackend {
    Codespace,
    Sandbox,
}

impl From<DefaultBackend> for super::backend::CloudBackendKind {
    fn from(kind: DefaultBackend) -> Self {
        match kind {
            DefaultBackend::Codespace => Self::Codespaces,
            DefaultBackend::Sandbox => Self::DockerSandbox,
        }
    }
}

impl From<super::backend::CloudBackendKind> for DefaultBackend {
    fn from(kind: super::backend::CloudBackendKind) -> Self {
        match kind {
            super::backend::CloudBackendKind::Codespaces => Self::Codespace,
            super::backend::CloudBackendKind::DockerSandbox => Self::Sandbox,
        }
    }
}

/// How a project's new codespaces are created.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CodespaceProjectConfig {
    /// Branch to create the codespace from; empty means the HEAD branch of the project folder.
    pub base_ref: String,
    /// Devcontainer config path; empty means the repository default.
    pub devcontainer_path: String,
    /// Machine type; `None` means [`crate::codespaces::DEFAULT_MACHINE`].
    pub machine: Option<String>,
    pub idle_timeout_minutes: u32,
    /// App Claude account; `None` means the Codespaces secret `CLAUDE_CODE_OAUTH_TOKEN`.
    pub claude_account: Option<String>,
}
impl Default for CodespaceProjectConfig {
    fn default() -> Self {
        Self {
            base_ref: String::new(),
            devcontainer_path: String::new(),
            machine: None,
            idle_timeout_minutes: DEFAULT_IDLE_TIMEOUT_MINUTES,
            claude_account: None,
        }
    }
}

fn machine_chars(machine: &str) -> bool {
    machine.len() <= 64
        && machine
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
}

impl CodespaceProjectConfig {
    /// Trim the text fields and check every value. Returns the cleaned config.
    pub fn validated(&self) -> Result<Self, String> {
        let mut cfg = self.clone();
        cfg.base_ref = cfg.base_ref.trim().to_owned();
        cfg.devcontainer_path = cfg.devcontainer_path.trim().to_owned();
        for field in [&mut cfg.machine, &mut cfg.claude_account] {
            *field = field
                .take()
                .map(|v| v.trim().to_owned())
                .filter(|v| !v.is_empty());
        }
        if !cfg.base_ref.is_empty() && !docker_config::valid_ref(&cfg.base_ref) {
            return Err("The base branch contains invalid characters.".into());
        }
        if !cfg.devcontainer_path.is_empty()
            && !docker_config::valid_devcontainer_path(&cfg.devcontainer_path)
        {
            return Err("The devcontainer path must be relative to the repository root.".into());
        }
        if let Some(machine) = &cfg.machine {
            if !machine_chars(machine) {
                return Err("The machine type contains invalid characters.".into());
            }
        }
        if !crate::codespaces::IDLE_TIMEOUT_MINUTES.contains(&cfg.idle_timeout_minutes) {
            return Err(crate::codespaces::IDLE_TIMEOUT_ERROR.into());
        }
        if let Some(id) = &cfg.claude_account {
            if !docker_config::valid_account_id(id) {
                return Err("The Claude account reference is invalid.".into());
            }
        }
        Ok(cfg)
    }
}

/// What the `cloud-projects.v1` setting stores per project.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct StoredProject {
    #[serde(skip_serializing_if = "Option::is_none")]
    default_backend: Option<DefaultBackend>,
    codespace: CodespaceProjectConfig,
}

/// The cloud settings of one project, both backends.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ProjectCloud {
    /// The user's choice; `None` while never chosen (see [`Self::effective_default_backend`]).
    pub default_backend: Option<DefaultBackend>,
    pub codespace: CodespaceProjectConfig,
    /// `None` means no sandbox config is saved.
    pub sandbox: Option<DockerProjectConfig>,
}

impl ProjectCloud {
    /// The stored choice, else Sandbox when a sandbox config exists, else Codespace.
    pub fn effective_default_backend(&self) -> DefaultBackend {
        self.default_backend.unwrap_or(if self.sandbox.is_some() {
            DefaultBackend::Sandbox
        } else {
            DefaultBackend::Codespace
        })
    }
}

/// The stored map. A missing setting is empty; an unreadable one is an error so a save never
/// overwrites it.
fn load_map(store: &Store) -> Result<BTreeMap<String, StoredProject>, String> {
    match store.setting(SETTING)? {
        None => Ok(BTreeMap::new()),
        Some(raw) => serde_json::from_str(&raw)
            .map_err(|_| "The saved cloud settings are unreadable.".to_owned()),
    }
}

/// Both parts of the project at `cwd`. A stored part that no longer validates is an error.
pub fn load(store: &Store, cwd: &str) -> Result<ProjectCloud, String> {
    let CodespaceCloud {
        default_backend,
        codespace,
    } = load_codespace(store, cwd)?;
    Ok(ProjectCloud {
        default_backend,
        codespace,
        sandbox: docker_config::load(store, cwd)?,
    })
}

/// What `cloud-projects.v1` holds for one project: no sandbox part.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CodespaceCloud {
    pub default_backend: Option<DefaultBackend>,
    pub codespace: CodespaceProjectConfig,
}

/// The default backend and codespace part of the project at `cwd`. Reads only
/// `cloud-projects.v1`, so an invalid sandbox config never affects a codespace.
pub fn load_codespace(store: &Store, cwd: &str) -> Result<CodespaceCloud, String> {
    let stored = load_map(store)?.remove(cwd).unwrap_or_default();
    Ok(CodespaceCloud {
        default_backend: stored.default_backend,
        codespace: stored.codespace.validated()?,
    })
}

/// The codespace settings of the project at `cwd`, or the defaults when none are saved, the
/// saved ones cannot be read, or the store is unavailable (a start then behaves as before the
/// settings existed). The one function every codespace entry point reads settings through.
pub fn codespace_config_or_default(
    store: &std::sync::Mutex<Store>,
    cwd: &str,
) -> CodespaceProjectConfig {
    store
        .lock()
        .ok()
        .and_then(|store| load_codespace(&store, cwd).ok())
        .map(|p| p.codespace)
        .unwrap_or_default()
}

/// Validate both parts first, then write. Nothing is written when either is invalid, and an
/// unreadable stored setting is never overwritten.
///
/// `sandbox: None` means "not configured here": the existing sandbox entry is left alone,
/// never cleared. Forget it explicitly with [`docker_config::clear`]. The returned value
/// shows what is stored afterwards, so a left-alone sandbox entry appears in it.
pub fn save(store: &Store, cwd: &str, project: &ProjectCloud) -> Result<ProjectCloud, String> {
    let codespace = project.codespace.validated()?;
    let sandbox = project
        .sandbox
        .as_ref()
        .map(DockerProjectConfig::validated)
        .transpose()?;
    let mut map = load_map(store)?;
    // Surfaces an unreadable sandbox setting before anything is written.
    docker_config::load_all(store)?;
    map.insert(
        cwd.to_owned(),
        StoredProject {
            default_backend: project.default_backend,
            codespace: codespace.clone(),
        },
    );
    let json = serde_json::to_string(&map).map_err(|e| e.to_string())?;
    // `cloud-projects.v1` goes first: when the second write fails, only the new, additive
    // setting changed and the caller is told Err; the existing sandbox config (the critical
    // one) is still the old, working value. The store has no multi-setting transaction.
    store.save_setting(SETTING, &json)?;
    let sandbox = match sandbox {
        Some(config) => Some(docker_config::save(store, cwd, &config)?),
        None => docker_config::load(store, cwd).ok().flatten(),
    };
    Ok(ProjectCloud {
        default_backend: project.default_backend,
        codespace,
        sandbox,
    })
}

/// Project cwds whose sandbox config uses Docker account `account_id`, sorted.
pub fn projects_using_docker_account(
    store: &Store,
    account_id: &str,
) -> Result<Vec<String>, String> {
    Ok(docker_config::load_all(store)?
        .into_iter()
        .filter(|(_, c)| c.docker_account.as_deref() == Some(account_id))
        .map(|(cwd, _)| cwd)
        .collect())
}

/// Project cwds that use Claude account `account_id` on either backend, sorted.
pub fn projects_using_claude_account(
    store: &Store,
    account_id: &str,
) -> Result<Vec<String>, String> {
    let mut cwds = BTreeSet::new();
    for (cwd, c) in docker_config::load_all(store)? {
        if c.claude_account.as_deref() == Some(account_id) {
            cwds.insert(cwd);
        }
    }
    for (cwd, p) in load_map(store)? {
        if p.codespace.claude_account.as_deref() == Some(account_id) {
            cwds.insert(cwd);
        }
    }
    Ok(cwds.into_iter().collect())
}

/// What the project's devcontainer file declares that matters to cloud setup.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DevcontainerInfo {
    /// Path relative to the project root, when a config file exists.
    pub found_path: Option<String>,
    /// `hostRequirements.cpus`.
    pub host_cpus: Option<u32>,
    /// `hostRequirements.memory` in MiB.
    pub host_memory_mib: Option<u64>,
    /// A feature whose id mentions `sshd` is installed (the app reaches a codespace over ssh).
    pub has_sshd: bool,
}

/// `16gb`, `8GB`, `512mb`, `1tb` (whole or decimal) as MiB.
fn memory_mib(text: &str) -> Option<u64> {
    let text = text.trim().to_ascii_lowercase();
    let split = text.find(|c: char| !(c.is_ascii_digit() || c == '.'))?;
    let amount: f64 = text[..split].parse().ok()?;
    let factor = match text[split..].trim() {
        "tb" => 1024.0 * 1024.0,
        "gb" => 1024.0,
        "mb" => 1.0,
        "kb" => 1.0 / 1024.0,
        _ => return None,
    };
    let mib = (amount * factor).ceil();
    (mib.is_finite() && mib >= 0.0).then_some(mib as u64)
}

/// Whether the feature id `id` names the sshd feature: its last path segment, without a
/// `:version` or `@digest` tag, is exactly `sshd`.
fn is_sshd_feature(id: &str) -> bool {
    let last = id.rsplit('/').next().unwrap_or(id);
    let name = last.split([':', '@']).next().unwrap_or(last);
    name == "sshd"
}

/// Reads the devcontainer config of `project_root`. `path` is relative to the root; empty
/// looks for `.devcontainer/devcontainer.json`, then `.devcontainer.json`. A file that cannot
/// be parsed is reported as found, with nothing declared. Reads files only.
pub fn devcontainer_info(project_root: &Path, path: &str) -> DevcontainerInfo {
    let path = path.trim();
    let candidates: Vec<&str> = if path.is_empty() {
        vec![".devcontainer/devcontainer.json", ".devcontainer.json"]
    } else if docker_config::valid_devcontainer_path(path) {
        vec![path]
    } else {
        Vec::new()
    };
    let Some((found, file)) = candidates
        .into_iter()
        .map(|rel| (rel, project_root.join(rel)))
        .find(|(_, file)| file.is_file())
    else {
        return DevcontainerInfo::default();
    };
    let mut info = DevcontainerInfo {
        found_path: Some(found.to_owned()),
        ..Default::default()
    };
    let Some(value) = std::fs::read_to_string(&file).ok().and_then(|text| {
        serde_json::from_str::<serde_json::Value>(&crate::codespaces::strip_jsonc(&text)).ok()
    }) else {
        return info;
    };
    let host = value.get("hostRequirements");
    info.host_cpus = host
        .and_then(|h| h.get("cpus"))
        .and_then(|c| c.as_u64())
        .and_then(|c| u32::try_from(c).ok());
    info.host_memory_mib = host
        .and_then(|h| h.get("memory"))
        .and_then(|m| m.as_str())
        .and_then(memory_mib);
    info.has_sshd = value
        .get("features")
        .and_then(|f| f.as_object())
        .is_some_and(|f| f.keys().any(|k| is_sshd_feature(k)));
    info
}
