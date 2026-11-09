//! The `cloud.*` app actions and the cloud half of `sessions.start`, shared by the Operator
//! dispatcher (`shell/app_commands.rs`) and the Mono dispatcher (`monos/host.rs`) so both
//! behave the same. Everything here BLOCKS on gh, sbx or ssh: call it from a worker thread.
use crate::runtime::Database;
use monocode_core::{
    cloud_sessions::CloudRef,
    mono::cloud_api::{self, AppBusy, CloudApi, CloudStart},
};
use serde_json::{Value, json};
use std::collections::HashSet;

/// What the UI thread knows about busy work, copied before a worker starts (the worker cannot
/// read the shell): the workspaces (`CloudRef::key`) an archive or convert is working on, and the
/// cloud sessions the live views show as working or waiting for input.
#[derive(Clone, Default)]
pub(crate) struct LiveBusy {
    pub workspaces: HashSet<String>,
    pub sessions: HashSet<i64>,
}

impl AppBusy for LiveBusy {
    fn working_on(&self, cloud: &CloudRef) -> bool {
        self.workspaces.contains(&cloud.key())
    }
    fn session_busy(&self, id: i64) -> bool {
        self.sessions.contains(&id)
    }
}

/// Runs one `cloud.*` action. `busy` is the UI snapshot: `cloud.stop` and `cloud.delete` refuse
/// while an archive works on the instance, and both count the live busy sessions.
pub(crate) fn run(
    store: &Database,
    action: &str,
    project: &str,
    input: &Value,
    busy: &LiveBusy,
) -> Result<Value, String> {
    let api = CloudApi::shared(store)?;
    match action {
        "cloud.list" => api.list(project, input),
        "cloud.status" => api.status(project, input),
        "cloud.create" => api.create(project, input, &mut |_| {}),
        "cloud.stop" => api.stop(project, input, busy),
        "cloud.resume" => api.resume(project, input),
        "cloud.delete" => api.delete(project, input, busy),
        _ => Err("Unknown app action. Run app help".into()),
    }
}

/// Adds `target`, `cloudState` and (while provisioning) `startup` to a `sessions.start` result.
pub(crate) fn describe_start(result: &mut Value, start: &CloudStart) {
    let state = cloud_api::state_id(start.initial_state());
    let cloud = start.cloud_ref();
    result["target"] = json!({
        "kind":cloud.kind.api_name(),
        "instance":cloud.name,
        "state":state,
    });
    result["cloudState"] = json!(state);
    if let Some(record) = start.record() {
        result["startup"] = json!({"step":record.current().map(|step| step.id())});
    }
}

/// The error of a failed start. An instance created for this start is kept: say so (worded for
/// its backend), so a retry targets it instead of creating a second one.
pub(crate) fn failure_note(start: Option<&CloudStart>, error: String) -> String {
    match start.and_then(CloudStart::record) {
        Some(record) if record.created => match record.cloud() {
            Some(cloud) => monocode_core::cloud_sessions::created_note_for(&cloud, &error),
            None => error,
        },
        _ => error,
    }
}

/// Cleans up the pending session of a start that failed before the app began driving it.
/// A codespace stays reachable through `gh`, so its row is dropped. A sandbox is found only
/// through the sessions bound to it, so its row is kept and marked failed: it shows the error
/// with Retry, and `cloud.list` / `cloud.delete` still reach the sandbox.
pub(crate) fn discard_failed_start(
    db: &mut monocode_core::store::Store,
    start: Option<&CloudStart>,
    session_id: i64,
    error: &str,
) {
    if !keep_failed_sandbox_start(db, start, error) {
        let _ = db.delete(session_id);
    }
}

/// For a start that created a sandbox, marks its pending session failed (Retry stays available)
/// and returns true; false for anything else, which is left untouched.
pub(crate) fn keep_failed_sandbox_start(
    db: &monocode_core::store::Store,
    start: Option<&CloudStart>,
    error: &str,
) -> bool {
    let Some(start) = start else { return false };
    let Some(record) = start.record().filter(|r| r.created && r.sandbox.is_some()) else {
        return false;
    };
    let _ = monocode_core::cloud_sessions::pause_start(db, record.session_id, error);
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use monocode_core::{cloud_sessions::StartRecord, store::Session};

    fn record(sandbox: Option<&str>, codespace: &str) -> StartRecord {
        let mut json = json!({
            "sessionId":-5,"localProject":"/p","codespace":codespace,"repo":"o/r","branch":null,
            "machine":null,"display":"d","remoteCwd":"/workspaces/r","webUrl":null,"created":true,
            "ghState":null,"steps":[],"error":null,"failedAt":null,"logTail":[],"startedAt":1,
            "finishedAt":null,
        });
        if let Some(name) = sandbox {
            json["sandbox"] = json!(name);
        }
        serde_json::from_value(json).unwrap()
    }
    fn session() -> Session {
        serde_json::from_value(json!({
            "id":-5,"title":"t","provider":"claude","cwd":"remote://x","provider_id":null,
            "has_messages":false,"updated_at":1,"archived":false,"pinned":false,"work_item":null,
        }))
        .unwrap()
    }

    #[test]
    fn a_failed_sandbox_start_is_worded_for_the_sandbox() {
        let start = CloudStart::Pending(session(), record(Some("box-9"), ""));
        let note = failure_note(Some(&start), "store failed".into());
        assert!(
            note.contains("sandbox box-9 was created and kept"),
            "{note}"
        );
        assert!(!note.contains("codespace"), "{note}");
        let start = CloudStart::Pending(session(), record(None, "octo-1"));
        let note = failure_note(Some(&start), "store failed".into());
        assert!(
            note.contains("codespace octo-1 was created and kept"),
            "{note}"
        );
        assert_eq!(failure_note(None, "plain".into()), "plain");
    }

    #[test]
    fn the_live_busy_snapshot_answers_for_workspaces_and_sessions() {
        let busy = LiveBusy {
            workspaces: HashSet::from([CloudRef::sandbox("box-1").key()]),
            sessions: HashSet::from([-7]),
        };
        assert!(busy.working_on(&CloudRef::sandbox("box-1")));
        assert!(!busy.working_on(&CloudRef::codespace("box-1")));
        assert!(busy.session_busy(-7) && !busy.session_busy(-8));
    }
}
