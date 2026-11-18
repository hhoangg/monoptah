//! Moves between local and cloud (H-14): `cloud.moveToCloud`,
//! `cloud.moveToLocal`, `cloud.cancelMove` and the `move` part of the `cloud`
//! field.
//!
//! They are the app API's move actions (`shell/cloud_move_api.rs`), so the
//! engine's rules are the user's: no force push, ignored files never uploaded,
//! and a cancel never deletes a workspace. The host has already checked that
//! both desktop toggles ("Remote cloud start" and "Remote push in Git") are on
//! and that the device may control; a move or a return that starts counts in
//! the wake rate limit until it ends. `cloud.moveToLocal` only opens the
//! desktop's Continue locally confirmation (the app API's rule: it changes the
//! user's working tree), so the person at the desktop presses Continue.
use super::view::{Extras, MoveStep};
use crate::shell::Shell;
use gpui::{Context, Window};
use monocode_core::mobile::cloud_policy::{self, sanitize_startup_error};
use monocode_core::mobile::policy::{Code, Failure};
use monocode_core::mobile::{DeviceContext, DeviceKind};
use monocode_core::mono::move_api;
use serde_json::{Value, json};

/// The wire id of each of the 7 steps (`MoveStep::ALL` order).
pub(crate) const STEP_IDS: [&str; 7] = [
    "snapshot",
    "push",
    "workspace",
    "upload",
    "restoreCode",
    "restoreConversation",
    "flip",
];

/// The wire id of a move step.
pub(crate) fn step_id(step: monocode_core::cloud_sessions::move_session::MoveStep) -> &'static str {
    use monocode_core::cloud_sessions::move_session::MoveStep as S;
    match step {
        S::Snapshot => STEP_IDS[0],
        S::Push => STEP_IDS[1],
        S::Workspace => STEP_IDS[2],
        S::Upload => STEP_IDS[3],
        S::RestoreCode => STEP_IDS[4],
        S::RestoreConversation => STEP_IDS[5],
        S::Flip => STEP_IDS[6],
    }
}

/// The desktop's refusal as a failure for the web: coded, and the text sanitized (no path,
/// URL or command line).
fn refusal(error: &str) -> Failure {
    Failure::coded(
        Code::UnsupportedTarget.as_str(),
        sanitize_startup_error(error),
    )
}

/// `sessionId` of the params, the only thing a cancel or a return takes.
fn only_session_id(params: &Value) -> Result<&str, Failure> {
    let map = params.as_object().ok_or(Code::Forbidden.failure())?;
    if map.keys().any(|key| key != "sessionId") {
        return Err(Code::Forbidden.failure());
    }
    session_id(params)
}

fn session_id(params: &Value) -> Result<&str, Failure> {
    params["sessionId"]
        .as_str()
        .filter(|id| !id.is_empty())
        .ok_or_else(|| Failure::coded(Code::Forbidden.as_str(), "Choose a session"))
}

/// `{sessionId, target?}` as the app API's input. The `target` takes the keys of a
/// remote start (`kind`, `instance`, `branch`, `machine`, `size`, see
/// `cloud_policy::validate_start_target`); anything else is `forbidden`. The move itself is
/// the app API's, which takes only the kind and the instance and runs against the targets
/// the desktop's own dialog offers, so `branch`, `machine` and `size` are refused
/// (`unsupported_target`) after the shape check. No `target` is the desktop's default: the first
/// target its dialog offers for the session.
fn move_to_cloud_input(
    shell: &Shell,
    session: i64,
    params: &Value,
) -> Result<move_api::MoveToCloud, Failure> {
    let map = params.as_object().ok_or(Code::Forbidden.failure())?;
    if map.keys().any(|key| key != "sessionId" && key != "target") {
        return Err(Code::Forbidden.failure());
    }
    let (kind, instance) = match map.get("target") {
        None | Some(Value::Null) => {
            let (kind, instance) = shell
                .mobile_move_default(session)
                .ok_or_else(|| Code::UnsupportedTarget.failure())?;
            (kind, instance)
        }
        Some(target) => {
            let target = cloud_policy::validate_start_target(target).map_err(Code::failure)?;
            // The move engine takes only the kind and the instance: a choice it cannot honour
            // is refused, not silently ignored.
            if target.branch.is_some() || target.machine.is_some() || target.size.is_some() {
                return Err(Failure::coded(
                    Code::UnsupportedTarget.as_str(),
                    "This choice is not available for a move",
                ));
            }
            let kind = match target.kind {
                cloud_policy::StartKind::Codespace => "codespace",
                cloud_policy::StartKind::Sandbox => "sandbox",
            };
            let instance = match target.instance {
                cloud_policy::StartInstance::New => "new".to_owned(),
                cloud_policy::StartInstance::Named(name) => name,
            };
            (kind, instance)
        }
    };
    move_api::parse_move_to_cloud(
        &json!({"session": session, "target": {"kind": kind, "instance": instance}}),
    )
    .map_err(|error| refusal(&error))
}

pub(super) fn move_to_cloud(
    shell: &mut Shell,
    device: &DeviceContext,
    params: &Value,
    cx: &mut Context<Shell>,
) -> Result<Value, Failure> {
    if device.kind != DeviceKind::Web {
        return Err(Code::Unsupported.failure());
    }
    let session = shell.mobile_move_session(Some(session_id(params)?), device, cx)?;
    let request = move_to_cloud_input(shell, session, params)?;
    let project = shell
        .mobile_move_project(session)
        .ok_or_else(|| Code::UnsupportedTarget.failure())?;
    let slot = cloud_policy::acquire_wake(&device.id)?;
    match shell.api_move_to_cloud(&project, request, cx) {
        Ok(_) => {
            shell.hold_wake_slot(slot, move |shell| !shell.mobile_move_active(session), cx);
            Ok(json!({"accepted": true, "direction": "toCloud"}))
        }
        Err(error) => {
            cloud_policy::release_wake(&slot);
            Err(refusal(&error))
        }
    }
}

/// `cloud.moveToLocal`: opens the Continue locally confirmation on the
/// desktop. Needs the window, so the host routes it here directly.
pub(crate) fn move_to_local(
    shell: &mut Shell,
    device: &DeviceContext,
    params: &Value,
    window: &mut Window,
    cx: &mut Context<Shell>,
) -> Result<Value, Failure> {
    if device.kind != DeviceKind::Web {
        return Err(Code::Unsupported.failure());
    }
    let session = shell.mobile_move_session(Some(only_session_id(params)?), device, cx)?;
    let project = shell
        .mobile_move_project(session)
        .ok_or_else(|| Code::UnsupportedTarget.failure())?;
    let slot = cloud_policy::acquire_wake(&device.id)?;
    match shell.api_move_to_local(&project, session, window, cx) {
        Ok(_) => {
            shell.hold_wake_slot(slot, move |shell| !shell.mobile_return_active(session), cx);
            Ok(json!({"accepted": true, "direction": "toLocal", "awaitingConfirmation": true}))
        }
        Err(error) => {
            cloud_policy::release_wake(&slot);
            Err(refusal(&error))
        }
    }
}

/// `cloud.cancelMove`: the app API's cancel. It deletes no workspace: the
/// engine is asked with `delete_workspace: false` and nothing here can say
/// otherwise (the params take only `sessionId`).
pub(super) fn cancel_move(
    shell: &mut Shell,
    device: &DeviceContext,
    params: &Value,
    cx: &mut Context<Shell>,
) -> Result<Value, Failure> {
    if device.kind != DeviceKind::Web {
        return Err(Code::Unsupported.failure());
    }
    let session = shell.mobile_move_session(Some(only_session_id(params)?), device, cx)?;
    let project = shell
        .mobile_move_project(session)
        .ok_or_else(|| Code::UnsupportedTarget.failure())?;
    shell
        .api_cancel_move(&project, session, cx)
        .map(|_| json!({"accepted": true}))
        .map_err(|error| refusal(&error))
}

/// Sets `moving` of the `cloud` field for a session: the step in progress and
/// its status, from the shell's own move. Only the 7 step ids and a status
/// reach the wire.
pub(super) fn extras(shell: &Shell, session: i64, extras: &mut Extras) {
    extras.moving = shell
        .mobile_move_progress(session)
        .map(|(step, status)| MoveStep { step, status });
}
