//! Permission requests a paired device may answer (H-08). The whole path is
//! behind `policy::REMOTE_APPROVALS_ENABLED`, which stays false until the U-13
//! review gate passes: while it is off a device cannot answer one and sees no
//! approval fields.
use crate::shell::Shell;
use gpui::{App, Context};
use monocode_core::mobile::{DeviceContext, policy};
use serde_json::{Map, Value, json};

const INVALID: &str = "Invalid mobile parameters";
const NOT_PENDING: &str = "Approval is no longer pending";

/// Answers `session.answerApproval {conversationId, requestId, decision}`.
pub(crate) fn dispatch(
    shell: &mut Shell,
    device: &DeviceContext,
    _method: &str,
    params: &Value,
    cx: &mut Context<Shell>,
) -> Result<Value, String> {
    dispatch_with(shell, device, params, policy::REMOTE_APPROVALS_ENABLED, cx)
}

/// [`dispatch`] with the flag given, so tests can force it on. Checks in
/// order: flag, parameter shape, decision, selection (`not_selected`), full
/// access, then that the request is still pending.
pub(crate) fn dispatch_with(
    shell: &mut Shell,
    device: &DeviceContext,
    params: &Value,
    enabled: bool,
    cx: &mut Context<Shell>,
) -> Result<Value, String> {
    if !enabled {
        return Err(policy::Code::Disabled.message().to_owned());
    }
    let forbidden = || policy::Code::Forbidden.message().to_owned();
    let Some(map) = params.as_object() else {
        return Err(INVALID.into());
    };
    // No scope, always or remember: an answer is once, for one request.
    if map
        .keys()
        .any(|key| !matches!(key.as_str(), "conversationId" | "requestId" | "decision"))
    {
        return Err(forbidden());
    }
    let allow = match map.get("decision").and_then(Value::as_str) {
        Some("allow") => true,
        Some("deny") => false,
        _ => return Err(INVALID.into()),
    };
    let (Some(conversation), Some(request_id)) = (
        map.get("conversationId").and_then(Value::as_str),
        map.get("requestId").and_then(Value::as_str),
    ) else {
        return Err(INVALID.into());
    };
    let id = shell.mobile_approval_session(conversation, device, cx)?;
    let view = shell.mobile_session_view(id).ok_or(NOT_PENDING)?;
    if view.read(cx).mobile_is_full_access() {
        return Err(forbidden());
    }
    view.update(cx, |view, cx| {
        view.mobile_decide_approval(request_id, allow, cx)
    })?;
    Ok(json!({}))
}

/// The approvals a session waits on, as the phone's field.
fn approvals_field(shell: &Shell, session: i64, cx: &App) -> Value {
    let list = shell
        .mobile_session_view(session)
        .map(|view| view.read(cx).mobile_approvals(cx))
        .unwrap_or_default();
    Value::Array(
        list.into_iter()
            .map(|a| json!({"requestId": a.request_id, "label": a.label, "detail": a.detail}))
            .collect(),
    )
}

pub(crate) fn fields_with(
    shell: &Shell,
    session: i64,
    enabled: bool,
    cx: &App,
) -> Map<String, Value> {
    let mut fields = Map::new();
    if enabled {
        // Always present, so a cleared list is sent once as `[]`.
        fields.insert("approvals".into(), approvals_field(shell, session, cx));
    }
    fields
}

/// Fields added to a conversation read for a session: `approvals` when the
/// flag is on, nothing otherwise.
pub(crate) fn read_fields(
    shell: &Shell,
    session: i64,
    _device: &DeviceContext,
    cx: &App,
) -> Map<String, Value> {
    fields_with(shell, session, policy::REMOTE_APPROVALS_ENABLED, cx)
}

/// Fields added to a watch frame for a session; see [`read_fields`].
pub(crate) fn frame_fields(
    shell: &Shell,
    session: i64,
    _device: &DeviceContext,
    cx: &App,
) -> Map<String, Value> {
    fields_with(shell, session, policy::REMOTE_APPROVALS_ENABLED, cx)
}
