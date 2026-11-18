//! `cloud.instances` (H-10, H-12): the cloud instances of the device's
//! selected projects, from the cached workspaces overview (30 s TTL, one
//! `workspaces_overview(project, false)` per project) with the cost estimates
//! of its rows. A request never forces a fresh listing and never wakes an
//! instance; the load runs on a worker, so the UI thread never waits for
//! `gh` or `sbx` either, and a device that asks again within the TTL causes no
//! outside call at all.
//!
//! Who sees what (security.md T16): a device whose selection is all projects
//! gets every instance by name, failed and orphan billing ones included. Any
//! other device gets the instances its selected projects' sessions use, with
//! only those sessions, and the failed and orphan billing instances it may not
//! name only as `otherCount`.
use crate::shell::Shell;
use gpui::Context;
use monocode_core::cloud_sessions::footer_status::{Concern, concern_of};
use monocode_core::cloud_sessions::stopping::StopReport;
use monocode_core::cloud_sessions::workspaces::{WorkspaceOverview, WorkspaceRow, WorkspaceState};
use monocode_core::mobile::DeviceContext;
use monocode_core::mobile::cloud_policy;
use monocode_core::mobile::policy::Failure;
use monocode_core::mono::cloud_api::CloudApi;
use serde_json::{Value, json};
use std::collections::BTreeMap;

/// The instances of the device's selected projects, answered through
/// `finish` when the worker that reads the cached overviews is done.
pub(super) fn list(
    shell: &mut Shell,
    device: &DeviceContext,
    finish: impl FnOnce(&mut Shell, Result<Value, Failure>) + 'static,
    cx: &mut Context<Shell>,
) {
    if let Err(failure) = cloud_policy::allow_read(&device.id) {
        return finish(shell, Err(failure));
    }
    let projects = shell.mobile_cloud_projects(device, cx);
    let store = shell.mobile_database();
    let work = cx.background_executor().spawn(async move {
        let api = CloudApi::shared(&store).ok()?;
        Some(
            projects
                .iter()
                .map(|project| api.cloud().workspaces_overview(project, false))
                .collect::<Vec<_>>(),
        )
    });
    let device = device.clone();
    cx.spawn(async move |this, cx| {
        let loads = work.await;
        let _ = this.update(cx, move |this, cx| {
            let result = match loads {
                Some(loads) => {
                    let all = device.projects.is_none();
                    let now = monocode_core::time::now_ms();
                    Ok(assemble(all, &loads, now, &mut |id| {
                        this.mobile_session_use(id, &device, cx)
                    }))
                }
                None => Err(Failure::from("Cloud is not available")),
            };
            finish(this, result);
        });
    })
    .detach();
}

/// `state` on the wire for a workspace state (`cloud_api::state_id` words).
fn state_word(state: WorkspaceState) -> &'static str {
    match state {
        WorkspaceState::Running => "available",
        WorkspaceState::Starting => "starting",
        WorkspaceState::Stopping | WorkspaceState::Hibernating => "stopping",
        WorkspaceState::Stopped => "stopped",
        WorkspaceState::Failed => "unavailable",
        WorkspaceState::Unknown => "unknown",
    }
}

/// `stopReport` on the wire; a settled stop reports nothing. The reason text of
/// `StillRunning` never leaves the desktop.
fn stop_word(report: &StopReport) -> Option<&'static str> {
    match report {
        StopReport::Stopping => Some("stopping"),
        StopReport::TakingLong => Some("takingLong"),
        StopReport::StillRunning(_) => Some("stillRunning"),
        StopReport::Stopped => None,
    }
}

/// The reply from loaded overviews. `all`: the device's selection is every
/// project. `uses` says what one session is (None: unknown, never shown to a
/// device with a selection).
pub(super) fn assemble(
    all: bool,
    loads: &[WorkspaceOverview],
    now_ms: u64,
    uses: &mut dyn FnMut(i64) -> Option<(String, bool, bool)>,
) -> Value {
    // One row per instance: projects of one repository list the same ones.
    let mut rows: BTreeMap<String, WorkspaceRow> = BTreeMap::new();
    for load in loads {
        for row in &load.rows {
            match rows.get_mut(&row.key()) {
                None => {
                    rows.insert(row.key(), row.clone());
                }
                Some(known) => {
                    for session in &row.sessions {
                        if !known.sessions.iter().any(|s| s.id == session.id) {
                            known.sessions.push(session.clone());
                        }
                    }
                }
            }
        }
    }
    let mut instances = Vec::new();
    let mut other = 0usize;
    for row in rows.values() {
        let mut sessions = Vec::new();
        for session in &row.sessions {
            let Some((persisted, selected, busy)) = uses(session.id) else {
                continue;
            };
            if all || selected {
                sessions.push(json!({
                    "sessionId": persisted,
                    "title": session.title,
                    "busy": session.busy || busy,
                }));
            }
        }
        if !all && sessions.is_empty() {
            if matches!(
                concern_of(row, now_ms),
                Some(Concern::Failed | Concern::Orphan)
            ) {
                other += 1;
            }
            continue;
        }
        let mut item = json!({
            "kind": row.cloud.kind.api_name(),
            "instance": row.cloud.name,
            "displayName": row.display_name,
            "state": state_word(row.state),
            "rawState": row.raw_state,
            "sessions": sessions,
        });
        if let Some(ms) = row.last_used_ms {
            item["lastUsedAt"] = json!(ms);
        }
        if let Some(rate) = row.cost.rate_per_hour {
            item["costEstimatePerHour"] = json!(rate);
        }
        if let Some(word) = row.stop.as_ref().and_then(stop_word) {
            item["stopReport"] = json!(word);
        }
        instances.push(item);
    }
    let codespaces_ready = loads.iter().any(|load| {
        load.codespaces_error.is_none()
            && load.gh.as_ref().is_some_and(|gh| {
                gh.as_ref()
                    .is_ok_and(|auth| auth.require_codespace_scope().is_ok())
            })
    });
    let sandboxes_ready =
        !loads.is_empty() && loads.iter().all(|load| load.sandbox_problems.is_empty());
    let mut reply = json!({
        "instances": instances,
        "backends": [
            {"kind": "codespace", "ready": codespaces_ready},
            {"kind": "sandbox", "ready": sandboxes_ready},
        ],
    });
    if other > 0 {
        reply["otherCount"] = json!(other);
    }
    reply
}
