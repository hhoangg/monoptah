//! Cloud sessions for a paired device: the hooks the mobile host calls where
//! cloud behaviour must differ from a local session (WS-D4).
//!
//! Everything cloud is for web devices only (D1): `DeviceKind::Phone` (QUIC)
//! gets today's lists and send semantics byte for byte. The shared catalog the
//! app publishes carries the web-only sessions, projects and fields under a
//! marker that `cloud_policy::scope_catalog` removes for phones before the
//! host pages the lists.
//!
//! Layout: [`view`] builds the catalog and `cloud` fields (H-09); every other
//! child module is the home of one ticket and starts as a stub that answers
//! `Code::Unsupported`, so a ticket fills its own file and touches no other:
//!
//! * `instances` (`cloud.instances`) and `lifecycle` (`cloud.stop`,
//!   `cloud.resume`, `cloud.retryStart`, `cloud.keepRunning`) and `send` (the
//!   `before_send` body, wake on send): H-10 and H-12.
//! * `start` (`cloud.startOptions` and the `session.start` target check):
//!   H-11 and H-13.
//! * `moves` (`cloud.moveToCloud`, `cloud.moveToLocal`, `cloud.cancelMove`):
//!   H-14.
use crate::shell::Shell;
use gpui::{App, Context};
use monocode_core::mobile::cloud_policy::{self, WEB_ONLY};
use monocode_core::mobile::policy::{Code, Failure};
use monocode_core::mobile::{DeviceContext, DeviceKind};
use monocode_core::store::Session;
use serde_json::{Map, Value};

mod instances;
mod lifecycle;
mod moves;
mod send;
mod start;
#[cfg(test)]
mod tests;
mod view;

pub(crate) use send::SendPlan;
pub(crate) use view::{Binding, CardView, Extras, Facts};
// For the lifecycle and move tickets to fill `Extras` with.
#[allow(unused_imports)]
pub(crate) use view::{AutoStop, MoveStep};

/// Method names that must never be reachable (D7): the host answers
/// `unsupported` for them before the app, and this answers `forbidden` if one
/// ever arrives.
const NEVER_EXPOSED: [&str; 7] = [
    "cloud.delete",
    "cloud.create",
    "cloud.rebuild",
    "cloud.pullEvidence",
    "cloud.openInBrowser",
    "cloud.deleteCodespace",
    "cloud.openBrowser",
];

/// Answers a `cloud.*` method. Phones have none (they never had); the web
/// methods route to the module of their ticket.
pub(crate) fn dispatch(
    shell: &mut Shell,
    device: &DeviceContext,
    method: &str,
    params: &Value,
    cx: &mut Context<Shell>,
) -> Result<Value, Failure> {
    if NEVER_EXPOSED.contains(&method) {
        return Err(Code::Forbidden.failure());
    }
    if device.kind != DeviceKind::Web {
        return Err(Code::Unsupported.failure());
    }
    match method {
        "cloud.instances" => instances::list(shell, device, params, cx),
        "cloud.stop" => lifecycle::stop(shell, device, params, cx),
        "cloud.resume" => lifecycle::resume(shell, device, params, cx),
        "cloud.retryStart" => lifecycle::retry_start(shell, device, params, cx),
        "cloud.keepRunning" => lifecycle::keep_running(shell, device, params, cx),
        "cloud.startOptions" => start::options(shell, device, params, cx),
        "cloud.moveToCloud" => moves::move_to_cloud(shell, device, params, cx),
        "cloud.moveToLocal" => moves::move_to_local(shell, device, params, cx),
        "cloud.cancelMove" => moves::cancel_move(shell, device, params, cx),
        _ => Err(Code::Unsupported.failure()),
    }
}

/// D9: a web device may not use `project.*`, `project.gitAction`,
/// `session.start` or `addProject` on a remote-machine project. A phone keeps
/// today's behaviour.
pub(crate) fn reject_non_local_project(
    device: &DeviceContext,
    project_id: &str,
) -> Result<(), Failure> {
    if device.kind == DeviceKind::Web && cloud_policy::is_non_local_project(project_id) {
        return Err(Code::UnsupportedTarget.failure());
    }
    Ok(())
}

fn on_rail<'a>(paths: &'a [String], project: &str) -> Option<&'a String> {
    let key = monocode_core::files::path_key(project);
    paths
        .iter()
        .find(|path| monocode_core::files::path_key(path) == key)
}

/// The project path a session is filed under in the catalog when its own
/// `cwd` is not a rail project. The catalog is built once for every device, so
/// this has no device: it answers for the web, and `cloud_policy::scope_catalog`
/// removes what a phone must not see.
///
/// * A codespace or sandbox session is filed under its local twin
///   (`Shell::project_of`), when that project is on the rail (`paths`).
/// * A session on a remote machine is filed under its own remote project.
pub(crate) fn catalog_project_of(
    shell: &Shell,
    id: i64,
    summary: &Session,
    paths: &[String],
) -> Option<String> {
    let facts = shell.mobile_cloud_facts(id)?;
    match &facts.binding {
        Some(binding) => on_rail(paths, &binding.local_project).cloned(),
        None => facts
            .remote_machine
            .then(|| monocode_core::files::path_key(&summary.cwd)),
    }
}

/// [`catalog_project_of`] for a request of `device` (the selection check of
/// `mobile_target`): phones never saw these sessions, so they have no project.
pub(crate) fn target_project_of(
    shell: &Shell,
    device: &DeviceContext,
    id: i64,
    summary: &Session,
    paths: &[String],
) -> Option<String> {
    (device.kind == DeviceKind::Web)
        .then(|| catalog_project_of(shell, id, summary, paths))
        .flatten()
}

/// How a session is shown on a web device's card; `web_only` is true when a
/// phone's catalog would not list it. None when the session is unknown.
pub(crate) fn card_view(shell: &Shell, id: i64, web_only: bool) -> Option<CardView> {
    shell
        .mobile_cloud_facts(id)
        .map(|facts| view::view_of(&facts, web_only))
}

/// Adds `target`, `cloudState` and `startup` to a catalog session card.
pub(crate) fn annotate_card(card: &mut Value, view: Option<&CardView>) {
    if let Some(view) = view {
        view.annotate(card);
    }
}

/// The remote-machine projects the catalog must add to `projects` for web
/// devices: the own project of every listed session that no rail project
/// holds, once each. `sessions` is each card's view and its `cwd`.
pub(crate) fn web_only_projects<'a>(
    sessions: impl Iterator<Item = (&'a CardView, &'a str)>,
) -> Vec<String> {
    let mut paths: Vec<String> = Vec::new();
    for (view, cwd) in sessions {
        let path = monocode_core::files::path_key(cwd);
        if view.web_only && view.placement == view::Placement::Remote && !paths.contains(&path) {
            paths.push(path);
        }
    }
    paths
}

/// Marks a catalog project card as one only web devices see.
pub(crate) fn mark_web_only(project: &mut Value) {
    project[WEB_ONLY] = Value::Bool(true);
}

/// Runs before a session is archived (H-12 fills `lifecycle::before_archive`).
pub(crate) fn before_archive(
    shell: &mut Shell,
    device: &DeviceContext,
    session: i64,
    params: &Value,
    cx: &mut Context<Shell>,
) -> Result<(), Failure> {
    lifecycle::before_archive(shell, device, session, params, cx)
}

/// Runs before a session is deleted (H-12 fills `lifecycle::before_delete`).
pub(crate) fn before_delete(
    shell: &mut Shell,
    device: &DeviceContext,
    session: i64,
    cx: &mut Context<Shell>,
) -> Result<(), Failure> {
    lifecycle::before_delete(shell, device, session, cx)
}

/// Runs once a send's conversation is resolved and checked: `session` is its
/// session, None for a Mono's chat. The plan says whether the send wakes the
/// instance (H-10).
pub(crate) fn before_send(
    shell: &mut Shell,
    device: &DeviceContext,
    session: Option<i64>,
    params: &Value,
    cx: &mut Context<Shell>,
) -> Result<SendPlan, Failure> {
    send::before_send(shell, device, session, params, cx)
}

/// Checks the `target` of a session start once its project (`cwd`) is
/// resolved (H-11 fills `start::start_target`).
pub(crate) fn start_target(
    shell: &Shell,
    device: &DeviceContext,
    cwd: &str,
    params: &Value,
    cx: &App,
) -> Result<(), Failure> {
    start::start_target(shell, device, cwd, params, cx)
}

/// The `cloud` field of a conversation read for a session. Web devices only;
/// reads use cached state and never wake an instance.
pub(crate) fn read_fields(
    shell: &Shell,
    session: i64,
    device: &DeviceContext,
    cx: &App,
) -> Map<String, Value> {
    session_fields(shell, session, device, cx)
}

/// The `cloud` field of a watch frame for a session; the watch sends it only
/// when it changed (`watch::update`'s `extra`).
pub(crate) fn frame_fields(
    shell: &Shell,
    session: i64,
    device: &DeviceContext,
    cx: &App,
) -> Map<String, Value> {
    session_fields(shell, session, device, cx)
}

fn session_fields(
    shell: &Shell,
    session: i64,
    device: &DeviceContext,
    _cx: &App,
) -> Map<String, Value> {
    if device.kind != DeviceKind::Web {
        return Map::new();
    }
    let Some(view) = card_view(shell, session, false) else {
        return Map::new();
    };
    let mut extras = Extras::default();
    lifecycle::extras(shell, session, &mut extras);
    moves::extras(shell, session, &mut extras);
    view.fields(&extras)
}
