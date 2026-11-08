//! The seam between cloud sessions and the service that hosts them.
//!
//! A cloud session lives in a remote workspace owned by a backend: a GitHub codespace
//! today ([`codespaces::CodespacesBackend`]), a Docker sandbox later. This module holds the
//! pieces every backend shares:
//!
//! * [`CloudBackendKind`]: which backend, as persisted and as the registry key.
//! * [`CloudRef`]: the kind plus the backend's own name for one workspace. Its
//!   [`CloudRef::key`] is the cache and settings key, and is the bare name for a codespace
//!   so everything stored before backends existed keeps its key.
//! * [`CloudBackend`]: the lifecycle operations [`super::CloudSessions`] dispatches.
//!
//! Stored records name a sandbox with their own optional `sandbox` field (see
//! `store::RemoteProject` and [`super::StartRecord`]). A record without it is a codespace
//! record, so old data reads unchanged, and a reader that does not know the field sees a
//! record without a codespace.
use super::CloudState;
use serde::{Deserialize, Serialize};
use std::path::Path;

pub mod codespaces;
pub mod docker;
pub use codespaces::CodespacesBackend;
pub use docker::DockerBackend;

/// Which service hosts a cloud session.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CloudBackendKind {
    #[default]
    Codespaces,
    DockerSandbox,
}

/// One workspace of one backend.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct CloudRef {
    pub kind: CloudBackendKind,
    pub name: String,
}

impl CloudRef {
    pub fn codespace(name: impl Into<String>) -> Self {
        Self {
            kind: CloudBackendKind::Codespaces,
            name: name.into(),
        }
    }
    pub fn sandbox(name: impl Into<String>) -> Self {
        Self {
            kind: CloudBackendKind::DockerSandbox,
            name: name.into(),
        }
    }
    /// Cache and settings key: the bare name for a codespace (as before backends existed),
    /// `sbx:<name>` for a sandbox so the two namespaces cannot collide.
    pub fn key(&self) -> String {
        match self.kind {
            CloudBackendKind::Codespaces => self.name.clone(),
            CloudBackendKind::DockerSandbox => format!("sbx:{}", self.name),
        }
    }
    /// Lowercase word for user-facing sentences.
    pub fn noun(&self) -> &'static str {
        self.kind.noun()
    }
    /// Capitalised [`Self::noun`].
    pub fn title(&self) -> &'static str {
        self.kind.title()
    }
    /// Environment id of the provisional project of a session that is still starting.
    /// It always starts with `pending-`, see `store::is_pending_cloud_environment`.
    pub fn pending_env(&self) -> String {
        match self.kind {
            CloudBackendKind::Codespaces => format!("pending-{}", self.name),
            CloudBackendKind::DockerSandbox => format!("pending-sbx-{}", self.name),
        }
    }
}

impl CloudBackendKind {
    pub fn noun(self) -> &'static str {
        match self {
            Self::Codespaces => "codespace",
            Self::DockerSandbox => "sandbox",
        }
    }
    pub fn title(self) -> &'static str {
        match self {
            Self::Codespaces => "Codespace",
            Self::DockerSandbox => "Sandbox",
        }
    }
}

/// Lifecycle operations of a backend, addressed by the backend's own workspace name.
///
/// Every method may block (it shells out), so call from a worker thread.
pub trait CloudBackend: Send + Sync {
    fn kind(&self) -> CloudBackendKind;
    /// Maps a raw [`Self::state`] string to the displayable state.
    fn display_state(&self, raw: &str) -> CloudState;
    /// Whether a raw state means the workspace is stopped or stopping, so that reconnecting
    /// would start it.
    fn is_stopped(&self, raw: &str) -> bool;
    /// Whether a raw state means the workspace no longer exists.
    fn is_deleted_state(&self, raw: &str) -> bool;
    /// Whether an error from the backend proves the workspace does not exist. An error
    /// that only means "could not look" (offline, tool missing) must return false.
    fn is_missing_error(&self, error: &str) -> bool;
    /// Raw backend state string (for a codespace, gh's `Available`, `Shutdown`, ...).
    fn state(&self, name: &str) -> Result<String, String>;
    /// Fresh, read-only existence check.
    fn exists(&self, name: &str) -> Result<bool, String>;
    fn stop(&self, name: &str) -> Result<(), String>;
    fn delete(&self, name: &str) -> Result<(), String>;
    /// Rebuilds the workspace's container from its definition.
    fn rebuild(&self, name: &str) -> Result<(), String>;
    /// Browser URL of the workspace. `None` when the backend has none, which hides the
    /// button; `Some(Err)` when it has one but could not read it.
    fn web_url(&self, name: &str) -> Option<Result<String, String>>;
    /// Runs `script` with `sh -s` inside the workspace and returns stdout. `long` allows
    /// minutes instead of seconds.
    fn run_script(&self, name: &str, script: &str, long: bool) -> Result<String, String>;
    /// Copies `remote` (a file or folder in the workspace) into the existing absolute local
    /// folder `local`, like `cp -r remote local`.
    fn copy_from(&self, name: &str, remote: &str, local: &Path) -> Result<(), String>;
    /// Best-effort setup of the evidence folder and agent guidance in `workspace`.
    fn prepare_evidence(&self, name: &str, workspace: &str) -> Result<(), String>;
}

#[cfg(test)]
mod docker_tests;
#[cfg(test)]
mod tests;
