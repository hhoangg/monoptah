//! App API of the session moves: `sessions.moveToCloud`, `sessions.moveToLocal`,
//! `sessions.moveStatus` and `sessions.cancelMove` (rule D11).
//!
//! The API can only NARROW what the user could choose in the UI:
//!
//! * the fields are exactly `session`, `target {kind, instance}`, `kit` (`"none"` | `"default"`)
//!   and `project`. There is no `force`, `deleteBranch`, `deleteWorkspace`, `skipChecks` or
//!   list of kit items, and an unknown key is refused before anything is created;
//! * the target is matched against the targets the move dialog offers for the project
//!   ([`resolve_target`]), so a new sandbox always uses the project's saved sandbox setup and an
//!   existing instance must already run a session of the project;
//! * the move itself is the normal engine (`CloudSessions::begin_move` / `run_move`): the push
//!   rules (no force, never the default branch unless it is origin's tip, ignored files never
//!   uploaded) are the engine's, not this module's;
//! * a cancel never deletes a workspace.
//!
//! Only a Mono may call these actions. Nothing here touches git, the Host or a workspace.
use crate::cloud_sessions::move_session::{MoveRecord, MoveStep, MoveTarget, StartedBy};
use crate::cloud_sessions::{kit_prefs::ApiKit, CloudBackendKind};
use serde_json::{json, Value};

use super::cloud_api::{parse_session_ref, spec_of};

pub const MOVE_TO_CLOUD: &str = "sessions.moveToCloud";
pub const MOVE_TO_LOCAL: &str = "sessions.moveToLocal";
pub const MOVE_STATUS: &str = "sessions.moveStatus";
pub const CANCEL_MOVE: &str = "sessions.cancelMove";

/// Refusal of an Operator for any of these actions.
pub const MONO_ONLY: &str =
    "Moving a session between this computer and the cloud is only available to a Mono";

/// True for the four move actions.
pub fn is_move_action(action: &str) -> bool {
    matches!(
        action,
        MOVE_TO_CLOUD | MOVE_TO_LOCAL | MOVE_STATUS | CANCEL_MOVE
    )
}

/// Space separated field names of a move action; `None` for any other action.
pub fn fields(action: &str) -> Option<&'static str> {
    match action {
        MOVE_TO_CLOUD => Some("session target kit project"),
        MOVE_TO_LOCAL | MOVE_STATUS | CANCEL_MOVE => Some("session project"),
        _ => None,
    }
}

/// Which instance a move names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Instance {
    New,
    Named(String),
}

/// The checked input of `sessions.moveToCloud`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MoveToCloud {
    pub session: i64,
    pub kind: CloudBackendKind,
    pub instance: Instance,
    /// `kit: "none"`. The API can only turn the user's kit off.
    pub kit_off: bool,
}

/// `session`: a number or numeric text. A moved session keeps its (positive) id; a session
/// born in the cloud has a negative one, and moveToLocal brings it back as a new local session.
pub fn session_of(input: &Value) -> Result<i64, String> {
    let value = input.get("session").ok_or("Provide session")?;
    Ok(parse_session_ref(value).ok_or("session must be a session ID")?)
}

fn bad_target() -> String {
    "target must be {\"kind\":\"codespace\"|\"sandbox\",\"instance\":\"new\"|\"<name>\"}".into()
}

/// Checks the values of the move actions after the generic field check passed. Pure: nothing is
/// read or created.
pub fn validate(action: &str, input: &Value) -> Result<(), String> {
    let id = session_of(input)?;
    if action != MOVE_TO_CLOUD {
        return Ok(());
    }
    if id < 0 {
        return Err("That session already lives in the cloud".into());
    }
    parse_move_to_cloud(input).map(|_| ())
}

/// The input of `sessions.moveToCloud`.
pub fn parse_move_to_cloud(input: &Value) -> Result<MoveToCloud, String> {
    let session = session_of(input)?;
    if session < 0 {
        return Err("That session already lives in the cloud".into());
    }
    let target = input
        .get("target")
        .and_then(Value::as_object)
        .ok_or_else(bad_target)?;
    if let Some(key) = target
        .keys()
        .find(|k| !matches!(k.as_str(), "kind" | "instance"))
    {
        return Err(format!(
            "target does not take {key}; a move takes only kind and instance"
        ));
    }
    let kind = target
        .get("kind")
        .and_then(Value::as_str)
        .and_then(CloudBackendKind::from_api_name)
        .ok_or_else(bad_target)?;
    let instance = match target.get("instance") {
        Some(Value::String(name)) if name == "new" => Instance::New,
        Some(Value::String(name)) => {
            spec_of(kind).instance_name(name)?;
            Instance::Named(name.clone())
        }
        _ => return Err(bad_target()),
    };
    let kit_off = input
        .get("kit")
        .map(ApiKit::parse)
        .transpose()
        .map_err(|_| "kit must be \"none\" or \"default\"".to_string())?
        == Some(ApiKit::None);
    Ok(MoveToCloud {
        session,
        kind,
        instance,
        kit_off,
    })
}

/// The target the dialog offers that matches `request`. `offered` is the dialog's own list for
/// the session's project, so the API reaches nothing the user could not choose.
pub fn resolve_target(request: &MoveToCloud, offered: &[MoveTarget]) -> Result<MoveTarget, String> {
    let noun = request.kind.noun();
    let found = offered
        .iter()
        .find(|target| match (target, &request.instance) {
            (MoveTarget::NewCodespace { .. }, Instance::New) => {
                request.kind == CloudBackendKind::Codespaces
            }
            (MoveTarget::NewSandbox { .. }, Instance::New) => {
                request.kind == CloudBackendKind::DockerSandbox
            }
            (MoveTarget::Existing { backend, name }, Instance::Named(wanted)) => {
                *backend == request.kind && name == wanted
            }
            _ => false,
        });
    found.cloned().ok_or_else(|| match &request.instance {
        Instance::New if request.kind == CloudBackendKind::Codespaces => {
            "A new codespace is not available for this project (it needs a GitHub repository)."
                .to_string()
        }
        Instance::New => {
            "A new sandbox is not available for this project: set up its Docker sandbox first."
                .to_string()
        }
        Instance::Named(name) => format!(
            "The {noun} {name} does not run a session of this project, so a move cannot use it."
        ),
    })
}

fn step_id(step: MoveStep) -> String {
    serde_json::to_value(step)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_default()
}

/// What `sessions.moveStatus` reports for a move: what the move dialog shows, and nothing
/// else. No path, account, upload id or Host id.
///
/// `state` is `running` (a worker is on it), `failed` (a step failed; Retry or Cancel in the
/// app) or `paused` (the record is kept but nothing runs, e.g. after a restart).
pub fn status_json(session: i64, record: &MoveRecord, running: bool) -> Value {
    let state = if record.is_failed() {
        "failed"
    } else if running {
        "running"
    } else {
        "paused"
    };
    let steps: Vec<Value> = MoveStep::ALL
        .into_iter()
        .map(|step| {
            let step_state = if record.done.contains(&step) {
                "done"
            } else if record.failed_at == Some(step) {
                "failed"
            } else if record.current == Some(step) {
                "current"
            } else {
                "pending"
            };
            json!({"id": step_id(step), "label": step.label(), "state": step_state})
        })
        .collect();
    let mut value = json!({
        "session": session,
        "direction": "toCloud",
        "state": state,
        "startedBy": match record.started_by {
            StartedBy::Api => "api",
            StartedBy::User => "user",
        },
        "steps": steps,
        "stale": record.stale,
    });
    if let Some(step) = record.failed_at.or(record.current) {
        value["step"] = json!(step_id(step));
    }
    if let Some(error) = &record.error {
        value["error"] = json!(error);
    }
    if let Some(cloud) = record.cloud() {
        value["target"] = json!({"kind": cloud.kind.api_name(), "instance": cloud.name});
    }
    if !record.branch.is_empty() {
        value["branch"] = json!(record.branch);
    }
    if !record.notices.is_empty() || !record.warnings.is_empty() {
        let mut notes: Vec<&String> = record.warnings.iter().collect();
        notes.extend(record.notices.iter());
        value["notices"] = json!(notes);
    }
    value
}

/// The status of a session that has no move record.
pub fn idle_json(session: i64, in_cloud: bool) -> Value {
    json!({
        "session": session,
        "state": "none",
        "location": if in_cloud { "cloud" } else { "local" },
    })
}

#[cfg(test)]
mod tests;
