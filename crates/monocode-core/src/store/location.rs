//! Where a local session lives: on this machine, or in a cloud workspace (design 4.2.1).
//!
//! The location is a per-session setting, `cloud-location:{id}`. While it is `Cloud`, the
//! session's runtime is the remote one and the only writer of its rows is the remote
//! row append in `cloud_rows.rs` (single writer, design 4.9 R2).
use super::Store;
use crate::cloud_sessions::{CloudBackendKind, CloudRef};
use serde::{Deserialize, Serialize};

/// A cloud workspace as stored: `CloudRef` has no serde support of its own.
mod cloud_ref {
    use super::{CloudBackendKind, CloudRef};
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    #[derive(Serialize, Deserialize)]
    struct Wire {
        backend: CloudBackendKind,
        name: String,
    }
    pub fn serialize<S: Serializer>(cloud: &CloudRef, s: S) -> Result<S::Ok, S::Error> {
        Wire {
            backend: cloud.kind,
            name: cloud.name.clone(),
        }
        .serialize(s)
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<CloudRef, D::Error> {
        let wire = Wire::deserialize(d)?;
        Ok(CloudRef {
            kind: wire.backend,
            name: wire.name,
        })
    }
}

/// The cloud side of a moved session.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudLocation {
    #[serde(with = "cloud_ref")]
    pub cloud: CloudRef,
    pub environment_id: String,
    pub project_id: String,
    pub host_session_id: String,
    pub remote_cwd: String,
    pub local_cwd: String,
    /// Rows before this index were written locally; the row append never rewrites them.
    pub moved_at_row: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pushed_branch: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum SessionLocation {
    #[default]
    Local,
    Cloud(CloudLocation),
}

impl SessionLocation {
    pub fn is_cloud(&self) -> bool {
        matches!(self, Self::Cloud(_))
    }
    pub fn cloud(&self) -> Option<&CloudLocation> {
        match self {
            Self::Cloud(location) => Some(location),
            Self::Local => None,
        }
    }
}

impl Store {
    fn location_key(session: i64) -> String {
        format!("cloud-location:{session}")
    }
    /// The location of a local session. A missing, empty or unreadable value is `Local`, so a
    /// session never becomes unusable because of a bad setting.
    pub fn cloud_location(&self, session: i64) -> Result<SessionLocation, String> {
        Ok(self
            .setting(&Self::location_key(session))?
            .filter(|raw| !raw.is_empty())
            .and_then(|raw| serde_json::from_str::<CloudLocation>(&raw).ok())
            .map_or(SessionLocation::Local, SessionLocation::Cloud))
    }
    pub fn set_cloud_location(
        &self,
        session: i64,
        location: &SessionLocation,
    ) -> Result<(), String> {
        let raw = match location {
            SessionLocation::Local => String::new(),
            SessionLocation::Cloud(cloud) => {
                serde_json::to_string(cloud).map_err(|e| e.to_string())?
            }
        };
        self.save_setting(&Self::location_key(session), &raw)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::path::Path;

    pub(crate) fn cloud_location() -> CloudLocation {
        CloudLocation {
            cloud: CloudRef::codespace("octo-abc"),
            environment_id: "env".into(),
            project_id: "project".into(),
            host_session_id: "host-1".into(),
            remote_cwd: "/workspaces/repo".into(),
            local_cwd: "/Users/me/repo".into(),
            moved_at_row: 2,
            pushed_branch: Some("feature".into()),
        }
    }

    #[test]
    fn location_defaults_to_local_and_flips_both_ways() {
        let store = Store::open(Path::new(":memory:")).unwrap();
        let session = store.create("claude", "/Users/me/repo").unwrap();
        assert_eq!(
            store.cloud_location(session.id).unwrap(),
            SessionLocation::Local
        );
        let cloud = SessionLocation::Cloud(cloud_location());
        store.set_cloud_location(session.id, &cloud).unwrap();
        assert_eq!(store.cloud_location(session.id).unwrap(), cloud);
        assert!(store.cloud_location(session.id).unwrap().is_cloud());
        store
            .set_cloud_location(session.id, &SessionLocation::Local)
            .unwrap();
        assert_eq!(
            store.cloud_location(session.id).unwrap(),
            SessionLocation::Local
        );
    }

    #[test]
    fn location_is_per_session_and_sandbox_refs_round_trip() {
        let store = Store::open(Path::new(":memory:")).unwrap();
        let a = store.create("claude", "/Users/me/repo").unwrap();
        let b = store.create("claude", "/Users/me/repo").unwrap();
        let mut location = cloud_location();
        location.cloud = CloudRef::sandbox("box-1");
        store
            .set_cloud_location(a.id, &SessionLocation::Cloud(location.clone()))
            .unwrap();
        assert_eq!(store.cloud_location(a.id).unwrap().cloud(), Some(&location));
        assert_eq!(store.cloud_location(b.id).unwrap(), SessionLocation::Local);
    }

    #[test]
    fn unreadable_location_reads_as_local_and_delete_forgets_it() {
        let mut store = Store::open(Path::new(":memory:")).unwrap();
        let session = store.create("claude", "/Users/me/repo").unwrap();
        store
            .save_setting(&format!("cloud-location:{}", session.id), "not json")
            .unwrap();
        assert_eq!(
            store.cloud_location(session.id).unwrap(),
            SessionLocation::Local
        );
        store
            .set_cloud_location(session.id, &SessionLocation::Cloud(cloud_location()))
            .unwrap();
        store.delete(session.id).unwrap();
        assert_eq!(
            store
                .setting(&format!("cloud-location:{}", session.id))
                .unwrap(),
            None
        );
    }
}
