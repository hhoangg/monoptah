//! Cloud and remote session policy for remote devices. Stubs until the cloud
//! methods land: today's catalog holds no cloud or remote sessions.
use super::policy::DeviceContext;
use serde_json::Value;

/// Removes the cloud and remote sessions and the new fields from a catalog
/// reply for a device that must not see them. No-op while the catalog holds
/// none.
pub fn scope_catalog(_device: &DeviceContext, _catalog: &mut Value) {}

/// Whether `project_id` names a cloud or remote project rather than a local
/// one. No such projects exist yet.
#[allow(dead_code)]
pub fn is_non_local_project(_project_id: &str) -> bool {
    false
}
