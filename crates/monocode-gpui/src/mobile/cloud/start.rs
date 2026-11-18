//! Start options and the remote start's `target` check (H-13, H-11).
//! Owner: agent C fills this.
use crate::shell::Shell;
use gpui::{App, Context};
use monocode_core::mobile::DeviceContext;
use monocode_core::mobile::policy::{Code, Failure};
use serde_json::Value;

/// `cloud.startOptions {projectId}`. Stub.
pub(super) fn options(
    _shell: &mut Shell,
    _device: &DeviceContext,
    _params: &Value,
    _cx: &mut Context<Shell>,
) -> Result<Value, Failure> {
    Err(Code::Unsupported.failure())
}

/// Checks the `target` of a session start once its project (`cwd`) is
/// resolved. Today any start is a local one.
pub(super) fn start_target(
    _shell: &Shell,
    _device: &DeviceContext,
    _cwd: &str,
    _params: &Value,
    _cx: &App,
) -> Result<(), Failure> {
    Ok(())
}
