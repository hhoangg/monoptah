//! What the app knows about the Host on the other end of a connection: its flavor
//! (`monoptah` or `upstream`), its version and its capability strings, read from
//! `environment.describe`.
//!
//! Features gate on capability strings, never on version numbers (rule D3). The version
//! only decides upgrades. The cache is keyed by machine id, filled on every successful
//! connect or describe and dropped when the machine is disconnected.

use serde_json::Value;
use std::collections::{BTreeSet, HashMap};
use std::sync::Mutex;

/// Flavor reported by a Host that predates `describe.host`.
pub const FLAVOR_UPSTREAM: &str = "upstream";
/// Flavor of the Host this fork publishes.
pub const FLAVOR_MONOPTAH: &str = "monoptah";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostInfo {
    pub flavor: String,
    /// Bare `X.Y.Z` as reported by the Host; `None` for an upstream Host.
    pub version: Option<String>,
    pub capabilities: BTreeSet<String>,
}

impl Default for HostInfo {
    /// Nothing is known yet: an upstream Host with no capabilities.
    fn default() -> Self {
        Self {
            flavor: FLAVOR_UPSTREAM.into(),
            version: None,
            capabilities: BTreeSet::new(),
        }
    }
}

impl HostInfo {
    /// Reads an `environment.describe` result. A missing or malformed `host` field means
    /// an upstream Host.
    pub fn from_descriptor(descriptor: &Value) -> Self {
        let host = descriptor.get("host").filter(|host| host.is_object());
        let text = |key: &str| {
            host.and_then(|host| host.get(key))
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_owned)
        };
        let flavor = text("flavor");
        Self {
            version: flavor.as_ref().and_then(|_| text("version")),
            flavor: flavor.unwrap_or_else(|| FLAVOR_UPSTREAM.into()),
            capabilities: descriptor
                .get("capabilities")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|entry| entry.as_str().map(str::to_owned))
                .collect(),
        }
    }

    pub fn supports(&self, capability: &str) -> bool {
        self.capabilities.contains(capability)
    }

    pub fn is_monoptah(&self) -> bool {
        self.flavor == FLAVOR_MONOPTAH
    }

    /// One line for Settings, e.g. `Monoptah 1.1.0` or `Upstream (version unknown)`.
    pub fn summary(&self) -> String {
        let flavor = if self.is_monoptah() {
            "Monoptah"
        } else if self.flavor == FLAVOR_UPSTREAM {
            "Upstream"
        } else {
            self.flavor.as_str()
        };
        match &self.version {
            Some(version) => format!("{flavor} {version}"),
            None => format!("{flavor} (version unknown)"),
        }
    }
}

/// Host info by machine id.
#[derive(Default)]
pub struct HostInfoCache {
    entries: Mutex<HashMap<String, HostInfo>>,
}

impl HostInfoCache {
    /// Stores what `descriptor` says about the machine and returns it.
    pub fn record(&self, machine_id: &str, descriptor: &Value) -> HostInfo {
        let info = HostInfo::from_descriptor(descriptor);
        if let Ok(mut entries) = self.entries.lock() {
            entries.insert(machine_id.to_owned(), info.clone());
        }
        info
    }

    pub fn get(&self, machine_id: &str) -> Option<HostInfo> {
        self.entries.lock().ok()?.get(machine_id).cloned()
    }

    pub fn drop_machine(&self, machine_id: &str) {
        if let Ok(mut entries) = self.entries.lock() {
            entries.remove(machine_id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn an_upstream_describe_has_no_host_field() {
        let info = HostInfo::from_descriptor(&json!({
            "protocolVersion": 1,
            "capabilities": ["files.list", "files.readBytes"],
        }));
        assert_eq!(info.flavor, "upstream");
        assert_eq!(info.version, None);
        assert!(!info.is_monoptah());
        assert!(info.supports("files.readBytes"));
        assert!(!info.supports("uploads"));
        assert_eq!(info.summary(), "Upstream (version unknown)");
        // A malformed host field is not trusted either.
        for host in [
            json!("monoptah"),
            json!({"flavor": ""}),
            json!({"version": "1.1.0"}),
        ] {
            let info = HostInfo::from_descriptor(&json!({"host": host}));
            assert_eq!((info.flavor.as_str(), info.version), ("upstream", None));
        }
    }

    #[test]
    fn a_fork_describe_reports_flavor_version_and_capabilities() {
        let info = HostInfo::from_descriptor(&json!({
            "host": {"flavor": "monoptah", "version": "1.1.0"},
            "capabilities": ["files.readBytes", "uploads", "workspace.watch", 7],
        }));
        assert!(info.is_monoptah());
        assert_eq!(info.version.as_deref(), Some("1.1.0"));
        assert_eq!(info.capabilities.len(), 3);
        assert!(info.supports("workspace.watch"));
        assert_eq!(info.summary(), "Monoptah 1.1.0");
    }

    #[test]
    fn the_cache_is_filled_by_a_describe_and_dropped_on_disconnect() {
        let cache = HostInfoCache::default();
        assert_eq!(cache.get("m1"), None);
        cache.record("m1", &json!({"capabilities": ["a"]}));
        cache.record(
            "m2",
            &json!({"host": {"flavor": "monoptah", "version": "1.1.0"}, "capabilities": ["b"]}),
        );
        assert!(!cache.get("m1").unwrap().is_monoptah());
        assert!(cache.get("m2").unwrap().supports("b"));
        // A newer describe replaces the entry.
        cache.record(
            "m1",
            &json!({"host": {"flavor": "monoptah", "version": "1.2.0"}}),
        );
        assert_eq!(cache.get("m1").unwrap().version.as_deref(), Some("1.2.0"));
        cache.drop_machine("m1");
        assert_eq!(cache.get("m1"), None);
        assert!(cache.get("m2").is_some());
    }
}
