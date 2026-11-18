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
use gpui::{App, Context, Window};
use monocode_core::mobile::cloud_policy::{self, WEB_ONLY};
use monocode_core::mobile::policy::{Code, Failure};
use monocode_core::mobile::{DeviceContext, DeviceKind, Request};
use monocode_core::store::Session;
use serde_json::{Map, Value};

mod instances;
mod lifecycle;
#[cfg(test)]
mod lifecycle_tests;
mod moves;
mod send;
mod start;
#[cfg(test)]
mod tests;
mod view;

pub(crate) use instances::instance_listed_project;
pub(crate) use lifecycle::Archive;
#[cfg(test)]
pub(crate) use moves::STEP_IDS as MOVE_STEP_IDS;
pub(crate) use moves::{move_to_local, step_id as move_step_id};
pub(crate) use send::SendPlan;
#[cfg(test)]
pub(crate) use start::prepare_options;
pub(crate) use start::{CloudStartPlan, OptionsProbe};
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

fn precheck(device: &DeviceContext, method: &str) -> Result<(), Failure> {
    if NEVER_EXPOSED.contains(&method) {
        return Err(Code::Forbidden.failure());
    }
    if device.kind != DeviceKind::Web {
        return Err(Code::Unsupported.failure());
    }
    Ok(())
}

/// Answers a `cloud.*` request. `cloud.instances` and `cloud.stop` reach the
/// backends (cached listings, `gh`, `sbx`), so they answer when a worker is
/// done; every other method answers at once. `finish` gets the answer.
pub(crate) fn dispatch_request(
    shell: &mut Shell,
    request: Request,
    finish: impl FnOnce(&mut Shell, Request, Result<Value, Failure>) + 'static,
    cx: &mut Context<Shell>,
) {
    let device = request.device.clone();
    let method = request.method.clone();
    let params = request.params.clone();
    dispatch_deferred(
        shell,
        &device,
        &method,
        &params,
        move |shell, result| finish(shell, request, result),
        cx,
    );
}

/// [`dispatch_request`] without the request: `finish` gets the answer, now or
/// from a worker.
pub(crate) fn dispatch_deferred(
    shell: &mut Shell,
    device: &DeviceContext,
    method: &str,
    params: &Value,
    finish: impl FnOnce(&mut Shell, Result<Value, Failure>) + 'static,
    cx: &mut Context<Shell>,
) {
    if let Err(failure) = precheck(device, method) {
        return finish(shell, Err(failure));
    }
    match method {
        "cloud.instances" => instances::list(shell, device, finish, cx),
        "cloud.stop" => lifecycle::stop(shell, device, params, finish, cx),
        "cloud.startOptions" => start::options_deferred(shell, device, params, finish, cx),
        _ => {
            let result = dispatch(shell, device, method, params, cx);
            finish(shell, result);
        }
    }
}

/// Answers a `cloud.*` method that needs no worker. Phones have none (they
/// never had); the web methods route to the module of their ticket.
/// `cloud.instances` and `cloud.stop` answer through [`dispatch_deferred`]
/// only.
pub(crate) fn dispatch(
    shell: &mut Shell,
    device: &DeviceContext,
    method: &str,
    params: &Value,
    cx: &mut Context<Shell>,
) -> Result<Value, Failure> {
    precheck(device, method)?;
    match method {
        "cloud.resume" => lifecycle::resume(shell, device, params, cx),
        "cloud.retryStart" => lifecycle::retry_start(shell, device, params, cx),
        "cloud.keepRunning" => lifecycle::keep_running(shell, device, params, cx),
        "cloud.startOptions" => start::options(shell, device, params, cx),
        "cloud.moveToCloud" => moves::move_to_cloud(shell, device, params, cx),
        // Opening the desktop's confirmation needs the window: the host
        // routes this method to `move_to_local` below.
        "cloud.moveToLocal" => Err(Code::Unsupported.failure()),
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

/// The project id a session is filed under in the catalog when its own `cwd`
/// is not a rail project. The catalog is built once for every device, so this
/// has no device: it answers for the web, and `cloud_policy::scope_catalog`
/// removes what a phone must not see.
///
/// * A codespace or sandbox session is filed under its local twin
///   (`Shell::project_of`), when that project is on the rail (`paths`): the
///   rail project's id.
/// * A session on a remote machine is filed under an opaque id of its own
///   remote project ([`cloud_policy::remote_project_id`]): nothing of the
///   machine's path reaches the wire (M2).
pub(crate) fn catalog_project_of(
    shell: &Shell,
    id: i64,
    summary: &Session,
    paths: &[String],
) -> Option<String> {
    let facts = shell.mobile_cloud_facts(id)?;
    match &facts.binding {
        Some(binding) => {
            on_rail(paths, &binding.local_project).map(|path| Shell::mobile_project_id(path))
        }
        None => facts.remote_machine.then(|| {
            cloud_policy::remote_project_id(&monocode_core::files::path_key(&summary.cwd))
        }),
    }
}

/// The id a project path has on the wire when no project is known for it:
/// opaque for a remote path, the app's usual id for a local one.
pub(crate) fn opaque_or_local_id(cwd: &str) -> String {
    if cwd.starts_with(monocode_core::remote::REMOTE_PATH_PREFIX) {
        cloud_policy::remote_project_id(&monocode_core::files::path_key(cwd))
    } else {
        Shell::mobile_project_id(cwd)
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

/// Whether only web devices see session `id`: it runs in a codespace, a
/// sandbox or on a remote machine, and its own folder is not a rail project,
/// so the phone catalog never listed it (D1). A phone can reach none of them.
pub(crate) fn web_only_session(shell: &Shell, id: i64, paths: &[String]) -> bool {
    shell.mobile_cloud_facts(id).is_some_and(|facts| {
        (facts.binding.is_some() || facts.remote_machine) && on_rail(paths, &facts.cwd).is_none()
    })
}

/// H2: a phone is refused a session that only web devices see
/// (`unsupported_target`), before anything else about it is looked at.
pub(crate) fn refuse_web_only_for_phone(
    shell: &Shell,
    device: &DeviceContext,
    id: i64,
    paths: &[String],
) -> Result<(), Failure> {
    if device.kind == DeviceKind::Phone && web_only_session(shell, id, paths) {
        return Err(Code::UnsupportedTarget.failure());
    }
    Ok(())
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

/// Makes a catalog project card the one only web devices see, for the remote
/// project at `path`: it gets the opaque id sessions are filed under and loses
/// its path, so only a label of the machine's folder is left (M2).
pub(crate) fn mark_web_only(project: &mut Value, path: &str) {
    project[WEB_ONLY] = Value::Bool(true);
    project["id"] = Value::String(cloud_policy::remote_project_id(path));
    if let Some(card) = project.as_object_mut() {
        card.remove("path");
    }
}

/// Runs before a session is archived. A cloud session of a web device is
/// archived here, through the app's cloud archive ([`Archive::Handled`]);
/// every other session is archived by the caller ([`Archive::Local`]).
pub(crate) fn before_archive(
    shell: &mut Shell,
    device: &DeviceContext,
    session: i64,
    params: &Value,
    window: &mut Window,
    cx: &mut Context<Shell>,
) -> Result<Archive, Failure> {
    lifecycle::before_archive(shell, device, session, params, window, cx)
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

/// `session.resumeUsage` and `session.resumeUsageAtReset` start a turn, which wakes a stopped
/// cloud instance like a send: the same check (`wake_required` without `wake: true` for web,
/// a coded "not connected" for a phone unless live). With consent the call counts in the
/// per-device wake limit and the run may wake (`true`); otherwise the run never wakes.
pub(crate) fn before_resume_usage(
    shell: &mut Shell,
    device: &DeviceContext,
    session: i64,
    params: &Value,
    cx: &mut Context<Shell>,
) -> Result<bool, Failure> {
    let plan = send::before_send(shell, device, Some(session), params, cx)?;
    if plan.wake {
        let slot = cloud_policy::acquire_wake(&device.id)?;
        // The run is the wake; the call stays counted for the hour, nothing is held.
        cloud_policy::release_wake(&slot);
    }
    Ok(plan.wake)
}

/// Just before a plain message is submitted: a cloud conversation must still be connected
/// (a coded failure otherwise), so an ok reply always means the message is delivered.
pub(crate) fn before_send_again(shell: &Shell, session: i64, cx: &App) -> Result<(), Failure> {
    if shell.mobile_is_cloud_conversation(session) {
        send::still_connected(shell, session, cx)?;
    }
    Ok(())
}

/// Starts the wake of a consented send (see `send::start_wake_send`).
pub(crate) fn start_wake_send(
    shell: &mut Shell,
    device: &DeviceContext,
    session: i64,
    cx: &mut Context<Shell>,
) -> Result<(), Failure> {
    send::start_wake_send(shell, device, session, cx)
}

/// Checks the `target` of a session start once its project (`cwd`) is
/// resolved. `None` is a local start; a plan is a cloud start the shell runs
/// (`Shell::mobile_start_cloud`).
pub(crate) fn start_target(
    shell: &Shell,
    device: &DeviceContext,
    cwd: &str,
    params: &Value,
    cx: &App,
) -> Result<Option<CloudStartPlan>, Failure> {
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
    cx: &App,
) -> Map<String, Value> {
    if device.kind != DeviceKind::Web {
        return Map::new();
    }
    let Some(view) = card_view(shell, session, false) else {
        return Map::new();
    };
    let mut extras = Extras::default();
    lifecycle::extras(shell, session, cx, &mut extras);
    moves::extras(shell, session, &mut extras);
    view.fields(&extras)
}
