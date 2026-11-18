//! Moves between local and cloud (H-14): `cloud.moveToCloud`,
//! `cloud.moveToLocal`, `cloud.cancelMove` and the `move` part of the `cloud`
//! field. Owner: agent C fills this.
use super::view::Extras;
use crate::shell::Shell;
use gpui::Context;
use monocode_core::mobile::DeviceContext;
use monocode_core::mobile::policy::{Code, Failure};
use serde_json::Value;

pub(super) fn move_to_cloud(
    _shell: &mut Shell,
    _device: &DeviceContext,
    _params: &Value,
    _cx: &mut Context<Shell>,
) -> Result<Value, Failure> {
    Err(Code::Unsupported.failure())
}

pub(super) fn move_to_local(
    _shell: &mut Shell,
    _device: &DeviceContext,
    _params: &Value,
    _cx: &mut Context<Shell>,
) -> Result<Value, Failure> {
    Err(Code::Unsupported.failure())
}

pub(super) fn cancel_move(
    _shell: &mut Shell,
    _device: &DeviceContext,
    _params: &Value,
    _cx: &mut Context<Shell>,
) -> Result<Value, Failure> {
    Err(Code::Unsupported.failure())
}

/// Sets `moving` of the `cloud` field for a session. Stub.
pub(super) fn extras(_shell: &Shell, _session: i64, _extras: &mut Extras) {}
