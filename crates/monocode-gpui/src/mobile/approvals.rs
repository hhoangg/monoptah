//! Permission requests a paired device may answer. Stubs until remote approval
//! ships: a phone cannot answer one and sees no approval fields.
use crate::shell::Shell;
use gpui::{App, Context};
use serde_json::{Map, Value};

/// Answers `session.answerApproval`. Not supported yet, so it is answered as
/// an unknown method was.
pub(crate) fn dispatch(
    _shell: &mut Shell,
    _method: &str,
    _params: &Value,
    _cx: &mut Context<Shell>,
) -> Result<Value, String> {
    Err("Unsupported mobile operation".into())
}

/// Fields added to a conversation read for a session. None yet.
pub(crate) fn read_fields(_session: i64, _cx: &App) -> Map<String, Value> {
    Map::new()
}

/// Fields added to a watch frame for a session. None yet.
pub(crate) fn frame_fields(_session: i64, _cx: &App) -> Map<String, Value> {
    Map::new()
}
