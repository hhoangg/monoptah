//! Pending grants: devices the web server says the user approved or is about
//! to approve, held in memory only. The session directory consults them to
//! answer `Pending` for a fingerprint the desktop has not confirmed yet.
//!
//! Filling this set from `GET /api/desktop/grants` (grant sync) is ticket
//! R-15 and is not part of this wave; for now callers insert and remove
//! entries themselves.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// What is known about one pending grant. `name` is untrusted text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Grant {
    pub name: String,
    pub server_device_id: String,
}

/// A shared, cloneable set of pending grants keyed by device fingerprint.
#[derive(Clone, Debug, Default)]
pub struct PendingGrants {
    inner: Arc<Mutex<HashMap<String, Grant>>>,
}

impl PendingGrants {
    pub fn new() -> Self {
        Self::default()
    }

    fn map(&self) -> std::sync::MutexGuard<'_, HashMap<String, Grant>> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Adds or replaces the grant for `fingerprint`.
    pub fn insert(&self, fingerprint: &str, name: &str, server_device_id: &str) {
        self.map().insert(
            fingerprint.to_string(),
            Grant {
                name: name.to_string(),
                server_device_id: server_device_id.to_string(),
            },
        );
    }

    /// Removes the grant; returns it when there was one.
    pub fn remove(&self, fingerprint: &str) -> Option<Grant> {
        self.map().remove(fingerprint)
    }

    pub fn contains(&self, fingerprint: &str) -> bool {
        self.map().contains_key(fingerprint)
    }

    pub fn get(&self, fingerprint: &str) -> Option<Grant> {
        self.map().get(fingerprint).cloned()
    }

    pub fn len(&self) -> usize {
        self.map().len()
    }

    pub fn is_empty(&self) -> bool {
        self.map().is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insert_contains_remove() {
        let g = PendingGrants::new();
        assert!(g.is_empty());
        g.insert("ab12", "Laptop", "11111111-1111-1111-1111-111111111111");
        assert!(g.contains("ab12"));
        assert_eq!(g.get("ab12").unwrap().name, "Laptop");
        let shared = g.clone();
        assert!(shared.contains("ab12"));
        assert_eq!(g.len(), 1);
        assert!(g.remove("ab12").is_some());
        assert!(!shared.contains("ab12"));
        assert!(g.remove("ab12").is_none());
    }
}
