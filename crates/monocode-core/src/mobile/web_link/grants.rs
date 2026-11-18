//! Pending grants: devices the web server says the user approved or is about
//! to approve, held in memory only. The session directory consults them to
//! answer `Pending` for a fingerprint the desktop has not confirmed yet.
//!
//! Pending grants are never saved: they are refetched from
//! `GET /api/desktop/grants` on every relay connect and whenever the relay says
//! `{"t":"grants"}`, and the whole set is replaced each time. The desktop
//! computes every fingerprint itself from the grant's public key and never
//! trusts the server's own `fingerprint` field.

use super::desktop_api::ServerGrant;
use super::noise::fingerprint;
use super::session::clean_name;
use crate::mobile::host::Device;
use crate::mobile::policy::{DeviceKind, Permission};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// Runs at most one grant sync at a time; a request that arrives meanwhile
/// makes the running sync go round once more.
#[derive(Default)]
pub(crate) struct SyncGate {
    state: Mutex<(bool, bool)>,
}

impl SyncGate {
    /// `true` when the caller should run the sync; `false` when one is
    /// already running (it will repeat).
    pub(crate) fn enter(&self) -> bool {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.0 {
            state.1 = true;
            false
        } else {
            state.0 = true;
            true
        }
    }

    /// `true` when another round was requested; `false` when the gate is free.
    pub(crate) fn again_or_leave(&self) -> bool {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if std::mem::take(&mut state.1) {
            true
        } else {
            state.0 = false;
            false
        }
    }
}

/// A permission is higher when it allows more.
pub(crate) fn rank(permission: Permission) -> u8 {
    match permission {
        Permission::Read => 0,
        Permission::Control => 1,
    }
}

/// What a grant list means for this desktop, given its web rows.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct Plan {
    /// `(fingerprint, cleaned name, server device id)` of devices to confirm.
    pub pending: Vec<(String, String, String)>,
    /// Row ids of web devices the server revoked.
    pub revoked_rows: Vec<String>,
    /// `(row id, new permission)`: a lowering the desktop applies by itself.
    pub lowered: Vec<(String, Permission)>,
    /// `(row id, server device id, requested)`: a raise the user must approve.
    pub raised: Vec<(String, String, Permission)>,
    /// Server device ids confirmed on the server that this desktop has no
    /// row for: the desktop reports them revoked.
    pub orphaned: Vec<String>,
}

/// The fingerprint of a grant's public key, `None` for a key that is not
/// 32 bytes of unpadded base64url.
pub(crate) fn grant_fingerprint(public_key: &str) -> Option<String> {
    let bytes = URL_SAFE_NO_PAD.decode(public_key).ok()?;
    (bytes.len() == 32).then(|| fingerprint(&bytes))
}

/// Sorts the server's grants into what to do. `rows` are the desktop's
/// devices; only web rows count.
pub(crate) fn plan(grants: &[ServerGrant], rows: &[Device]) -> Plan {
    let web = |row: &&Device| row.kind == DeviceKind::Web;
    let by_server_id = |id: &str| {
        rows.iter().filter(web).find(|row| {
            row.server_device_id
                .as_deref()
                .is_some_and(|row_id| row_id.eq_ignore_ascii_case(id))
        })
    };
    let mut plan = Plan::default();
    for grant in grants {
        let Some(own) = grant_fingerprint(&grant.public_key) else {
            continue;
        };
        // A grant whose key does not match the server's own claim is not
        // trusted at all.
        if !grant.fingerprint.eq_ignore_ascii_case(&own) {
            continue;
        }
        match grant.state.as_str() {
            "pending" => {
                let confirmed = rows.iter().filter(web).any(|row| row.fingerprint == own);
                if !confirmed {
                    plan.pending.push((
                        own,
                        clean_name(&grant.device_name),
                        grant.device_id.clone(),
                    ));
                }
            }
            "revoked" => {
                if let Some(row) = by_server_id(&grant.device_id) {
                    plan.revoked_rows.push(row.id.clone());
                }
            }
            "confirmed" => match by_server_id(&grant.device_id) {
                None => plan.orphaned.push(grant.device_id.clone()),
                Some(row) if row.fingerprint != own => {}
                Some(row) => {
                    if let Some(requested) = grant.requested.filter(|r| *r != row.permission) {
                        if rank(requested) < rank(row.permission) {
                            plan.lowered.push((row.id.clone(), requested));
                        } else {
                            plan.raised
                                .push((row.id.clone(), grant.device_id.clone(), requested));
                        }
                    }
                }
            },
            _ => {}
        }
    }
    plan
}

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

    /// Replaces the whole set at once.
    pub fn replace(&self, entries: Vec<(String, String, String)>) {
        let mut map = self.map();
        map.clear();
        for (fingerprint, name, server_device_id) in entries {
            map.insert(
                fingerprint,
                Grant {
                    name,
                    server_device_id,
                },
            );
        }
    }

    /// Every pending grant as `(fingerprint, grant)`, in a stable order.
    pub fn list(&self) -> Vec<(String, Grant)> {
        let mut list: Vec<(String, Grant)> = self
            .map()
            .iter()
            .map(|(fingerprint, grant)| (fingerprint.clone(), grant.clone()))
            .collect();
        list.sort_by(|a, b| (&a.1.name, &a.0).cmp(&(&b.1.name, &b.0)));
        list
    }

    pub fn clear(&self) {
        self.map().clear();
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

    fn server_grant(public: [u8; 32], id: &str, state: &str) -> ServerGrant {
        ServerGrant {
            device_id: id.into(),
            device_name: "Laptop".into(),
            public_key: URL_SAFE_NO_PAD.encode(public),
            fingerprint: fingerprint(&public),
            state: state.into(),
            requested: None,
            permission: None,
        }
    }

    fn web_row(public: [u8; 32], id: &str, permission: Permission) -> Device {
        Device {
            id: format!("row-{id}"),
            name: "Laptop".into(),
            fingerprint: fingerprint(&public),
            push: None,
            kind: DeviceKind::Web,
            permission,
            projects: None,
            monos: None,
            server_device_id: Some(id.into()),
            created_at: None,
            last_seen: None,
        }
    }

    #[test]
    fn the_plan_trusts_only_its_own_fingerprint_and_known_rows() {
        let mut forged = server_grant([1; 32], "s1", "pending");
        forged.fingerprint = "0".repeat(64);
        let mut short_key = server_grant([2; 32], "s2", "pending");
        short_key.public_key = URL_SAFE_NO_PAD.encode([2u8; 31]);
        let known = server_grant([3; 32], "s3", "pending");
        let fresh = server_grant([4; 32], "s4", "pending");
        let orphan = server_grant([5; 32], "s5", "confirmed");
        let revoked = server_grant([6; 32], "s6", "revoked");
        let mut raise = server_grant([7; 32], "s7", "confirmed");
        raise.requested = Some(Permission::Control);
        let mut lower = server_grant([8; 32], "s8", "confirmed");
        lower.requested = Some(Permission::Read);
        let mut same = server_grant([9; 32], "s9", "confirmed");
        same.requested = Some(Permission::Control);
        let rows = [
            web_row([3; 32], "s3", Permission::Control),
            web_row([6; 32], "s6", Permission::Control),
            web_row([7; 32], "s7", Permission::Read),
            web_row([8; 32], "s8", Permission::Control),
            web_row([9; 32], "s9", Permission::Control),
        ];
        let plan = plan(
            &[
                forged, short_key, known, fresh, orphan, revoked, raise, lower, same,
            ],
            &rows,
        );
        assert_eq!(
            plan.pending,
            [(
                fingerprint(&[4; 32]),
                "Laptop".to_string(),
                "s4".to_string()
            )]
        );
        assert_eq!(plan.revoked_rows, ["row-s6"]);
        assert_eq!(plan.orphaned, ["s5"]);
        assert_eq!(plan.lowered, [("row-s8".to_string(), Permission::Read)]);
        assert_eq!(
            plan.raised,
            [("row-s7".to_string(), "s7".to_string(), Permission::Control)]
        );
    }

    #[test]
    fn the_sync_gate_runs_one_sync_and_repeats_once_when_asked_meanwhile() {
        let gate = SyncGate::default();
        assert!(gate.enter());
        assert!(!gate.enter());
        assert!(!gate.enter());
        assert!(gate.again_or_leave());
        assert!(!gate.again_or_leave());
        assert!(gate.enter());
    }

    #[test]
    fn replace_swaps_the_whole_set() {
        let g = PendingGrants::new();
        g.insert("a", "A", "1");
        g.replace(vec![("b".into(), "B".into(), "2".into())]);
        assert!(!g.contains("a") && g.contains("b"));
        assert_eq!(g.list().len(), 1);
        g.clear();
        assert!(g.is_empty());
    }

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
