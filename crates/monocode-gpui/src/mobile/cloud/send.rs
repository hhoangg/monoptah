//! The `before_send` body: wake on send, `starting` and `auto_stopping`
//! refusals (H-10, H-12).
//!
//! Web devices only (D1): a phone's send is never looked at, so it keeps today's
//! path and reply, and it can never wake billable compute (its message goes
//! through `Conversation::mobile_submit`, which never reconnects a workspace).
//! For a codespace or sandbox session of a web device, in this order:
//!
//! 1. the instance is starting (or its start failed): `starting` carrying the
//!    desktop's own `starting_refusal` text;
//! 2. the desktop is stopping the instance on its own: `auto_stopping`;
//! 3. an archive works on the instance: `instance_busy`;
//! 4. the instance is `available` and the session is connected (or its view
//!    is not loaded and says nothing else): a plain send, as today;
//! 5. `unavailable`: `unsupported_target`, nothing can wake it;
//! 6. `stopped`, `stopping` or `unknown`, or `available` by the cache but not
//!    connected in the session's loaded view (the cache can be stale after an
//!    idle stop or a sandbox TTL): without `wake: true` it is `wake_required`,
//!    which costs nothing. With it, one pending wake-send per session (`busy`)
//!    is checked here; the plan says `wake`, and the host calls
//!    [`start_wake_send`] only once the message is validated, its files are
//!    staged and the submit is about to be queued, so a refused message never
//!    wakes anything or holds a slot. Nothing waits for the wake.
use super::lifecycle::{self, Wake};
use crate::shell::Shell;
use gpui::Context;
use monocode_core::cloud_sessions::CloudState;
use monocode_core::mobile::cloud_policy;
use monocode_core::mobile::policy::{Code, Failure};
use monocode_core::mobile::{DeviceContext, DeviceKind};
use serde_json::Value;

/// What a checked send does besides sending.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct SendPlan {
    /// The send wakes a stopped instance: the host answers at once with
    /// `{sessionId, wake: true, queued: true}` and the message waits in the
    /// existing deferral until the reload after the wake flushes it.
    pub wake: bool,
}

/// Runs once a send's conversation is resolved and checked.
pub(super) fn before_send(
    shell: &mut Shell,
    device: &DeviceContext,
    session: Option<i64>,
    params: &Value,
    cx: &mut Context<Shell>,
) -> Result<SendPlan, Failure> {
    let Some(id) = session else {
        return Ok(SendPlan::default());
    };
    // The conversation of a cloud session (also before the marks are loaded) is sent to
    // only while its link is live, for every device: a reply that says ok means the
    // message is delivered.
    if !shell.mobile_is_cloud_conversation(id) {
        return Ok(SendPlan::default());
    }
    let card = super::card_view(shell, id, false).filter(|view| view.is_cloud());
    let cloud = shell.mobile_cloud_ref_of(id);
    let (true, Some(view), Some(cloud)) = (device.kind == DeviceKind::Web, card, cloud) else {
        return still_connected(shell, id, cx).map(|()| SendPlan::default());
    };
    if let Some(text) = shell.mobile_start_refusal(id) {
        return Err(starting_failure(&text));
    }
    let state = view.state.unwrap_or(CloudState::Unknown);
    if state == CloudState::Starting {
        return Err(Code::Starting.failure());
    }
    if shell.mobile_auto_stopping(&cloud) {
        return Err(Failure::coded(
            Code::AutoStopping.as_str(),
            crate::conversation::AUTO_STOPPING,
        ));
    }
    if shell.mobile_instance_busy(&cloud) {
        return Err(Code::InstanceBusy.failure());
    }
    // The cache can be stale (an idle stop, a sandbox TTL, a restart of the desktop): only a
    // live, connected view is available. A view that is not loaded or not connected is not.
    let live = shell
        .mobile_session_view(id)
        .and_then(|view| view.read(cx).mobile_cloud_connected());
    let state = match (state, live) {
        (CloudState::Available, Some(true)) => CloudState::Available,
        (CloudState::Available, _) => CloudState::Stopped,
        (state, _) => state,
    };
    match state {
        CloudState::Available => Ok(SendPlan::default()),
        CloudState::Unavailable => Err(Code::UnsupportedTarget.failure()),
        CloudState::Starting => unreachable!("refused above"),
        CloudState::Stopped | CloudState::Stopping | CloudState::Unknown => {
            if params["wake"] != true {
                return Err(Code::WakeRequired.failure());
            }
            if crate::conversation::Conversation::wake_send_pending(id) {
                return Err(Code::Busy.failure());
            }
            Ok(SendPlan { wake: true })
        }
    }
}

/// Whether a cloud conversation is connected now. A phone, or any device when the web
/// path does not apply, gets a coded failure instead of a message that cannot be sent.
pub(crate) fn still_connected(shell: &Shell, id: i64, cx: &gpui::App) -> Result<(), Failure> {
    let live = shell
        .mobile_session_view(id)
        .and_then(|view| view.read(cx).mobile_cloud_connected());
    if live == Some(true) {
        return Ok(());
    }
    Err(Failure::coded(
        Code::UnsupportedTarget.as_str(),
        "The cloud instance is not connected; open it on the desktop",
    ))
}

/// Starts the wake of a send that was consented to, right before its message
/// is queued: one pending wake-send per session (`busy`), then a slot of the
/// per-device rate limit (`busy`), then the wake on a worker. On any failure
/// nothing stays claimed.
pub(crate) fn start_wake_send(
    shell: &mut Shell,
    device: &DeviceContext,
    id: i64,
    cx: &mut Context<Shell>,
) -> Result<(), Failure> {
    if !crate::conversation::Conversation::begin_wake_send(id) {
        return Err(Code::Busy.failure());
    }
    let slot = match cloud_policy::acquire_wake(&device.id) {
        Ok(slot) => slot,
        Err(failure) => {
            crate::conversation::Conversation::end_wake_send(id);
            return Err(failure);
        }
    };
    if let Err(failure) = lifecycle::start_wake(shell, id, slot, Wake::Send, cx) {
        crate::conversation::Conversation::end_wake_send(id);
        return Err(failure);
    }
    Ok(())
}

/// The `starting` refusal with the desktop's own text, which can quote the
/// error of a failed start: it leaves through the sanitizer of `startup.error`.
pub(super) fn starting_failure(text: &str) -> Failure {
    Failure::coded(
        Code::Starting.as_str(),
        cloud_policy::sanitize_startup_error(text),
    )
}
