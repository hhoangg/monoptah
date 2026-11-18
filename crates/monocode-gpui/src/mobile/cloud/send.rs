//! The `before_send` body: wake on send, `starting` and `auto_stopping`
//! refusals (H-10, H-12). Owner: agent B fills this.
use crate::shell::Shell;
use gpui::Context;
use monocode_core::mobile::DeviceContext;
use monocode_core::mobile::policy::Failure;
use serde_json::Value;

/// What a checked send does besides sending.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct SendPlan {
    /// The send wakes a stopped instance: the host answers at once with
    /// `{sessionId, wake: true, queued: true}` and the message waits in the
    /// existing deferral until the reload after the wake flushes it.
    pub wake: bool,
}

/// Runs once a send's conversation is resolved and checked. Today it never
/// refuses and never wakes.
pub(super) fn before_send(
    _shell: &mut Shell,
    _device: &DeviceContext,
    _session: Option<i64>,
    _params: &Value,
    _cx: &mut Context<Shell>,
) -> Result<SendPlan, Failure> {
    Ok(SendPlan::default())
}
