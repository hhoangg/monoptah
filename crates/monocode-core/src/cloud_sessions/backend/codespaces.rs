//! [`CloudBackend`] over the GitHub CLI: thin wrappers around the free functions of
//! [`crate::codespaces`], which stay where they are.
use super::{CloudBackend, CloudBackendKind};
use crate::cloud_sessions::CloudState;
use crate::codespaces::{self, GhRunner};
use std::path::Path;
use std::sync::Arc;

pub struct CodespacesBackend {
    gh: Arc<dyn GhRunner>,
}

impl CodespacesBackend {
    pub fn new(gh: Arc<dyn GhRunner>) -> Self {
        Self { gh }
    }
}

impl CloudBackend for CodespacesBackend {
    fn kind(&self) -> CloudBackendKind {
        CloudBackendKind::Codespaces
    }
    fn display_state(&self, raw: &str) -> CloudState {
        CloudState::from_gh(raw)
    }
    fn is_stopped(&self, raw: &str) -> bool {
        matches!(raw, "Shutdown" | "ShuttingDown")
    }
    fn is_deleted_state(&self, raw: &str) -> bool {
        raw == "Deleted"
    }
    fn is_missing_error(&self, error: &str) -> bool {
        // gh prints "HTTP 404: Not Found" for a missing codespace; a missing `gh` binary
        // says "command not found" and must not count. The codespace-specific phrasings
        // ("codespace not found", "no such codespace") are the same fact in other words, but
        // only as the START of a line (after an optional "error:"): the same words inside a
        // longer message (an auth or network failure that quotes them) prove nothing.
        let lower = error.to_lowercase();
        lower.contains("http 404")
            || lower.lines().any(|line| {
                let line = line.trim();
                let line = line.strip_prefix("error:").map_or(line, str::trim);
                line.starts_with("codespace not found") || line.starts_with("no such codespace")
            })
    }
    fn state(&self, name: &str) -> Result<String, String> {
        codespaces::state(&*self.gh, name)
    }
    fn exists(&self, name: &str) -> Result<bool, String> {
        codespaces::exists(&*self.gh, name)
    }
    fn stop(&self, name: &str) -> Result<(), String> {
        codespaces::stop(&*self.gh, name)
    }
    fn delete(&self, name: &str) -> Result<(), String> {
        codespaces::delete(&*self.gh, name)
    }
    fn rebuild(&self, name: &str) -> Result<(), String> {
        codespaces::rebuild(&*self.gh, name)
    }
    fn web_url(&self, name: &str) -> Option<Result<String, String>> {
        Some(codespaces::web_url(&*self.gh, name))
    }
    fn run_script(&self, name: &str, script: &str, long: bool) -> Result<String, String> {
        codespaces::run_script(&*self.gh, name, script, long)
    }
    fn copy_from(&self, name: &str, remote: &str, local: &Path) -> Result<(), String> {
        codespaces::copy_from(&*self.gh, name, remote, local)
    }
    fn prepare_evidence(&self, name: &str, workspace: &str) -> Result<(), String> {
        codespaces::prepare_evidence(&*self.gh, name, workspace)
    }
}
