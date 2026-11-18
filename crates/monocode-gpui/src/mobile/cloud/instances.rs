//! `cloud.instances` (H-10, H-12). Owner: agent B fills this.
use crate::shell::Shell;
use gpui::Context;
use monocode_core::mobile::DeviceContext;
use monocode_core::mobile::policy::{Code, Failure};
use serde_json::Value;

/// The cached instances of the device's selected projects. Stub.
pub(super) fn list(
    _shell: &mut Shell,
    _device: &DeviceContext,
    _params: &Value,
    _cx: &mut Context<Shell>,
) -> Result<Value, Failure> {
    Err(Code::Unsupported.failure())
}
