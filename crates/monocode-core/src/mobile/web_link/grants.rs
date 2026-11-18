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
    /// Row ids of web devices the server revoked, or no longer lists at all.
    pub revoked_rows: Vec<String>,
    /// `(row id, new permission)`: a lowering the desktop applies by itself.
    pub lowered: Vec<(String, Permission)>,
    /// `(row id, server device id, requested)`: a raise the user must approve.
    pub raised: Vec<(String, String, Permission)>,
    /// Server device ids confirmed on the server that this desktop has no
    /// row for: the desktop reports them `pending` again (not `revoked`, which
    /// is final), so they show up for local confirmation.
    pub orphaned: Vec<String>,
    /// `(server device id, permission)`: the server's view of a confirmed
    /// local row is stale (a report failed), so report `confirmed` again.
    pub reconfirm: Vec<(String, Permission)>,
    /// Ids the user revoked or denied that the server still lists as pending
    /// or confirmed: report `revoked` again, and never list them as pending.
    pub rerevoke: Vec<String>,
    /// Ids the user revoked that the server now lists `revoked`: forget them.
    pub settled: Vec<String>,
}

/// The fingerprint of a grant's public key, `None` for a key that is not
/// 32 bytes of unpadded base64url.
pub(crate) fn grant_fingerprint(public_key: &str) -> Option<String> {
    let bytes = URL_SAFE_NO_PAD.decode(public_key).ok()?;
    (bytes.len() == 32).then(|| fingerprint(&bytes))
}

/// Sorts the server's grants into what to do. `rows` are the desktop's
/// devices; only web rows count.
pub(crate) fn plan(grants: &[ServerGrant], rows: &[Device], user_revoked: &[String]) -> Plan {
    let web = |row: &&Device| row.kind == DeviceKind::Web;
    let by_server_id = |id: &str| {
        rows.iter().filter(web).find(|row| {
            row.server_device_id
                .as_deref()
                .is_some_and(|row_id| row_id.eq_ignore_ascii_case(id))
        })
    };
    // A pending key listed under several device ids is ambiguous: list none
    // of them. Revoked and confirmed grants are keyed by server id instead.
    let mut ids_by_key: std::collections::HashMap<String, Vec<&str>> = Default::default();
    for grant in grants.iter().filter(|g| g.state == "pending") {
        if let Some(own) = grant_fingerprint(&grant.public_key) {
            let ids = ids_by_key.entry(own).or_default();
            if !ids.contains(&grant.device_id.as_str()) {
                ids.push(grant.device_id.as_str());
            }
        }
    }
    let mut plan = Plan::default();
    for grant in grants {
        let listed = user_revoked
            .iter()
            .any(|id| id.eq_ignore_ascii_case(&grant.device_id))
            && by_server_id(&grant.device_id).is_none();
        if listed {
            match grant.state.as_str() {
                "revoked" => plan.settled.push(grant.device_id.clone()),
                "pending" | "confirmed" => plan.rerevoke.push(grant.device_id.clone()),
                _ => {}
            }
            continue;
        }
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
                if ids_by_key.get(&own).is_some_and(|ids| ids.len() > 1) {
                    continue;
                }
                let row = rows.iter().filter(web).find(|row| row.fingerprint == own);
                if let Some(row) = row {
                    let same_id = row
                        .server_device_id
                        .as_deref()
                        .is_some_and(|id| id.eq_ignore_ascii_case(&grant.device_id));
                    if same_id {
                        plan.reconfirm
                            .push((grant.device_id.clone(), row.permission));
                    }
                } else {
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
                    if grant.permission != Some(row.permission) {
                        plan.reconfirm
                            .push((grant.device_id.clone(), row.permission));
                    }
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
    // The list carries every grant of this desktop, a revoked one included.
    // A web row whose server id is not in it at all is a grant the server
    // deleted: the server says it is gone, so the row goes too and a stale
    // row can never re-create it by reporting `confirmed`.
    for row in rows.iter().filter(web) {
        let Some(id) = row.server_device_id.as_deref() else {
            continue;
        };
        let listed = grants
            .iter()
            .any(|grant| grant.device_id.eq_ignore_ascii_case(id));
        if !listed && !plan.revoked_rows.contains(&row.id) {
            plan.revoked_rows.push(row.id.clone());
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
            &[],
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
    fn the_plan_heals_stale_server_state_and_skips_ambiguous_keys() {
        let stale_pending = server_grant([1; 32], "s1", "pending");
        let mut wrong_permission = server_grant([2; 32], "s2", "confirmed");
        wrong_permission.permission = Some(Permission::Read);
        let mut right_permission = server_grant([3; 32], "s3", "confirmed");
        right_permission.permission = Some(Permission::Control);
        let twin_a = server_grant([4; 32], "s4", "pending");
        let twin_b = server_grant([4; 32], "s5", "pending");
        let rows = [
            web_row([1; 32], "s1", Permission::Control),
            web_row([2; 32], "s2", Permission::Control),
            web_row([3; 32], "s3", Permission::Control),
        ];
        let plan = plan(
            &[
                stale_pending,
                wrong_permission,
                right_permission,
                twin_a,
                twin_b,
            ],
            &rows,
            &[],
        );
        assert_eq!(
            plan.reconfirm,
            [
                ("s1".to_string(), Permission::Control),
                ("s2".to_string(), Permission::Control)
            ]
        );
        assert!(plan.pending.is_empty());
    }

    #[test]
    fn the_plan_rerevokes_what_the_user_revoked_and_settles_listed_ids() {
        let pending = server_grant([1; 32], "s1", "pending");
        let confirmed = server_grant([2; 32], "s2", "confirmed");
        let revoked = server_grant([3; 32], "s3", "revoked");
        let other = server_grant([4; 32], "s4", "confirmed");
        let ids = ["s1".to_string(), "s2".to_string(), "S3".to_string()];
        let plan = plan(&[pending, confirmed, revoked, other], &[], &ids);
        assert_eq!(plan.rerevoke, ["s1", "s2"]);
        assert_eq!(plan.settled, ["s3"]);
        assert!(plan.pending.is_empty());
        // Another confirmed grant without a row is still an orphan.
        assert_eq!(plan.orphaned, ["s4"]);
    }

    #[test]
    fn a_row_whose_grant_the_server_no_longer_lists_is_dropped() {
        let listed = server_grant([1; 32], "s1", "confirmed");
        let rows = [
            web_row([1; 32], "s1", Permission::Control),
            web_row([2; 32], "S2", Permission::Control),
            web_row([3; 32], "s3", Permission::Read),
        ];
        // `s3` differs from the list only by being absent; a row with no
        // server id at all is not the server's to drop.
        let mut unbound = web_row([4; 32], "s4", Permission::Read);
        unbound.server_device_id = None;
        let mut rows = rows.to_vec();
        rows.push(unbound);
        let mut twin = server_grant([2; 32], "s2", "confirmed");
        twin.permission = Some(Permission::Control);
        let plan = plan(&[listed, twin], &rows, &[]);
        assert_eq!(plan.revoked_rows, ["row-s3"]);
    }

    #[test]
    fn only_pending_twins_are_ambiguous() {
        let old = server_grant([5; 32], "s1", "revoked");
        let new = server_grant([5; 32], "s2", "pending");
        let plan = plan(&[old, new], &[], &[]);
        assert_eq!(plan.pending.len(), 1);
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
