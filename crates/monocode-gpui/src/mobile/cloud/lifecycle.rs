//! Instance lifecycle for web devices (H-10, H-12): `cloud.stop`,
//! `cloud.resume`, `cloud.retryStart`, `cloud.keepRunning`, the archive and
//! delete hooks and the `kept` and `autoStop` parts of the `cloud` field.
//! Owner: agent B fills this.
use super::view::Extras;
use crate::shell::Shell;
use gpui::Context;
use monocode_core::mobile::DeviceContext;
use monocode_core::mobile::policy::{Code, Failure};
use serde_json::Value;

pub(super) fn stop(
    _shell: &mut Shell,
    _device: &DeviceContext,
    _params: &Value,
    _cx: &mut Context<Shell>,
) -> Result<Value, Failure> {
    Err(Code::Unsupported.failure())
}

pub(super) fn resume(
    _shell: &mut Shell,
    _device: &DeviceContext,
    _params: &Value,
    _cx: &mut Context<Shell>,
) -> Result<Value, Failure> {
    Err(Code::Unsupported.failure())
}

pub(super) fn retry_start(
    _shell: &mut Shell,
    _device: &DeviceContext,
    _params: &Value,
    _cx: &mut Context<Shell>,
) -> Result<Value, Failure> {
    Err(Code::Unsupported.failure())
}

pub(super) fn keep_running(
    _shell: &mut Shell,
    _device: &DeviceContext,
    _params: &Value,
    _cx: &mut Context<Shell>,
) -> Result<Value, Failure> {
    Err(Code::Unsupported.failure())
}

/// Before a session is archived: a cloud session needs `cloud: "keep"` or
/// `"stop"` (D10). Today it never refuses.
pub(super) fn before_archive(
    _shell: &mut Shell,
    _device: &DeviceContext,
    _session: i64,
    _params: &Value,
    _cx: &mut Context<Shell>,
) -> Result<(), Failure> {
    Ok(())
}

/// Before a session is deleted: a cloud or remote session answers
/// `unsupported_target` (D7). Today it never refuses.
pub(super) fn before_delete(
    _shell: &mut Shell,
    _device: &DeviceContext,
    _session: i64,
    _cx: &mut Context<Shell>,
) -> Result<(), Failure> {
    Ok(())
}

/// Sets `kept` and `auto_stop` of the `cloud` field for a session. Stub.
pub(super) fn extras(_shell: &Shell, _session: i64, _extras: &mut Extras) {}
