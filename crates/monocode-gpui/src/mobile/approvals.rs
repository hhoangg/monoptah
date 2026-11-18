//! Permission requests a paired device may answer (H-08). The whole path is
//! behind `policy::REMOTE_APPROVALS_ENABLED`, which stays false until the U-13
//! review gate passes, and the desktop's own toggle
//! (`PolicyFlags::remote_approvals`): while either is off a device cannot
//! answer one and sees no approval fields. A request is answered once: the
//! `(session, requestId)` pair is recorded before the decision is applied.
use crate::shell::Shell;
use gpui::{App, Context};
use monocode_core::mobile::{DeviceContext, audit, policy, policy::Failure};
use serde_json::{Map, Value, json};

const INVALID: &str = "Invalid mobile parameters";

/// What the audit line of one answer knows, filled as the call is checked.
#[derive(Default)]
struct Seen {
    conversation: Option<String>,
    request: Option<String>,
    decision: Option<&'static str>,
    project: Option<String>,
}

/// Answers `session.answerApproval {conversationId, requestId, decision}`.
pub(crate) fn dispatch(
    shell: &mut Shell,
    device: &DeviceContext,
    command_id: &str,
    params: &Value,
    cx: &mut Context<Shell>,
) -> Result<Value, Failure> {
    dispatch_with(
        shell,
        device,
        command_id,
        params,
        policy::approvals_enabled(),
        cx,
    )
}

/// [`dispatch`] with the effective flag given (gate and toggle), so tests can
/// force it on. Checks in order: flag, parameter shape, decision, selection
/// (`not_selected`), full access, that the request was not answered before,
/// then that it is still pending and may be allowed. Writes one audit line,
/// with the decision, the ids and the refusal code, never the request's text;
/// an uncoded failure is left to the host's own line.
pub(crate) fn dispatch_with(
    shell: &mut Shell,
    device: &DeviceContext,
    command_id: &str,
    params: &Value,
    enabled: bool,
    cx: &mut Context<Shell>,
) -> Result<Value, Failure> {
    let mut seen = Seen::default();
    let result = answer(shell, device, params, enabled, &mut seen, cx);
    let outcome = match &result {
        Ok(_) => Some(audit::Outcome::Ok),
        Err(failure) => policy::code_of_failure(failure).map(audit::Outcome::Denied),
    };
    if let Some(outcome) = outcome {
        let fingerprint = shell.mobile_fingerprint(&device.id);
        audit::record_approval_answer_with(
            device,
            &audit::ApprovalAnswer {
                fingerprint: &fingerprint,
                project_id: seen.project.as_deref(),
                conversation_id: seen.conversation.as_deref(),
                request_id: seen.request.as_deref(),
                decision: seen.decision,
                command_id: Some(command_id),
                transport: device.kind.into(),
                outcome,
            },
        );
    }
    result
}

fn answer(
    shell: &mut Shell,
    device: &DeviceContext,
    params: &Value,
    enabled: bool,
    seen: &mut Seen,
    cx: &mut Context<Shell>,
) -> Result<Value, Failure> {
    let not_pending = || policy::Code::NotPending.failure();
    let forbidden = || policy::Code::Forbidden.failure();
    let Some(map) = params.as_object() else {
        return Err(if enabled {
            INVALID.into()
        } else {
            policy::Code::Disabled.failure()
        });
    };
    seen.conversation = map
        .get("conversationId")
        .and_then(Value::as_str)
        .map(str::to_owned);
    seen.request = map
        .get("requestId")
        .and_then(Value::as_str)
        .map(str::to_owned);
    seen.decision = match map.get("decision").and_then(Value::as_str) {
        Some("allow") => Some("allow"),
        Some("deny") => Some("deny"),
        _ => None,
    };
    if !enabled {
        return Err(policy::Code::Disabled.failure());
    }
    // No scope, always or remember: an answer is once, for one request.
    if map
        .keys()
        .any(|key| !matches!(key.as_str(), "conversationId" | "requestId" | "decision"))
    {
        return Err(forbidden());
    }
    let allow = match seen.decision {
        Some("allow") => true,
        Some(_) => false,
        None => return Err(INVALID.into()),
    };
    // The id is the desktop's own opaque string: a number or anything else is
    // refused as malformed, never converted.
    let (Some(conversation), Some(request_id)) = (seen.conversation.clone(), seen.request.clone())
    else {
        return Err(INVALID.into());
    };
    let id = shell.mobile_approval_session(&conversation, device, cx)?;
    seen.project = shell.mobile_session_project(id, cx);
    let view = shell.mobile_session_view(id).ok_or_else(not_pending)?;
    if view.read(cx).mobile_is_full_access() {
        return Err(forbidden());
    }
    // One answer per request, also while the desktop still shows it.
    if shell.mobile_was_answered(id, &request_id) {
        return Err(not_pending());
    }
    view.update(cx, |view, cx| {
        view.mobile_decide_approval(&request_id, allow, cx)
    })?;
    // Remembered only once applied: a refused allow leaves a deny possible.
    shell.mobile_claim_answer(id, &request_id);
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
            .map(|a| {
                json!({
                    "requestId": a.request_id,
                    "label": a.label,
                    "detail": a.detail,
                    "truncated": a.truncated,
                })
            })
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
    fields_with(shell, session, policy::approvals_enabled(), cx)
}

/// Fields added to a watch frame for a session; see [`read_fields`].
pub(crate) fn frame_fields(
    shell: &Shell,
    session: i64,
    _device: &DeviceContext,
    cx: &App,
) -> Map<String, Value> {
    fields_with(shell, session, policy::approvals_enabled(), cx)
}
