//! Cloud sessions for a paired device: the hooks the mobile host calls where
//! cloud behaviour must differ from a local session. Every stub keeps today's
//! behaviour, so a phone sees no change until the cloud methods land.
use crate::shell::Shell;
use gpui::{App, Context};
use monocode_core::store::Session;
use serde_json::{Map, Value};

const UNSUPPORTED: &str = "Unsupported mobile operation";

/// Answers a `cloud.*` method. None is supported yet, so each is answered as
/// an unknown method was.
pub(crate) fn dispatch(
    _shell: &mut Shell,
    _method: &str,
    _params: &Value,
    _cx: &mut Context<Shell>,
) -> Result<Value, String> {
    Err(UNSUPPORTED.into())
}

/// The project id a cloud session is filed under in the catalog, when the
/// session is not in a local project. Today none is.
pub(crate) fn catalog_project_of(_summary: &Session) -> Option<String> {
    None
}

/// Runs before a session is archived. Today it never refuses.
pub(crate) fn before_archive(
    _shell: &mut Shell,
    _session: i64,
    _params: &Value,
    _cx: &mut Context<Shell>,
) -> Result<(), String> {
    Ok(())
}

/// Runs before a session is deleted. Today it never refuses.
pub(crate) fn before_delete(
    _shell: &mut Shell,
    _session: i64,
    _cx: &mut Context<Shell>,
) -> Result<(), String> {
    Ok(())
}

/// Runs at the head of a send. Today it never refuses.
pub(crate) fn before_send(
    _shell: &mut Shell,
    _params: &Value,
    _cx: &mut Context<Shell>,
) -> Result<(), String> {
    Ok(())
}

/// Checks the `target` of a session start. Today any start is a local one.
pub(crate) fn start_target(_shell: &Shell, _params: &Value, _cx: &App) -> Result<(), String> {
    Ok(())
}

/// Fields added to a conversation read for a session. None yet.
pub(crate) fn read_fields(_session: i64, _cx: &App) -> Map<String, Value> {
    Map::new()
}

/// Fields added to a watch frame for a session. None yet.
pub(crate) fn frame_fields(_session: i64, _cx: &App) -> Map<String, Value> {
    Map::new()
}
