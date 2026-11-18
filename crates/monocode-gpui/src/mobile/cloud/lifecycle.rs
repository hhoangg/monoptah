//! Instance lifecycle for web devices (H-10, H-12): `cloud.stop`,
//! `cloud.resume`, `cloud.retryStart`, `cloud.keepRunning`, the archive and
//! delete hooks and the `kept` and `autoStop` parts of the `cloud` field.
//!
//! Every billable call needs `control` (the host checks it before the app
//! sees the request), a selected project, and, for the calls that cause
//! compute cost (resume, retry, a send that wakes), a slot of the per-device
//! rate limit (`cloud_policy::acquire_wake`). The slot is released when the
//! operation ends, whatever its outcome. `cloud.stop` saves money and is not
//! rate limited. Nothing here deletes or creates an instance.
use super::view::{AutoStop, Extras};
use crate::shell::Shell;
use gpui::{App, Context, Window};
use monocode_core::cloud_sessions::CloudRef;
use monocode_core::cloud_sessions::workspaces::WorkspaceTarget;
use monocode_core::mobile::cloud_policy::{self, WakeSlot};
use monocode_core::mobile::policy::{self, Code, Failure};
use monocode_core::mobile::{DeviceContext, DeviceKind};
use monocode_core::mono::cloud_api::CloudApi;
use serde_json::{Value, json};
use std::time::Duration;

/// What `session.archive` does after the hook looked at the session.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Archive {
    /// Not a cloud session of a web device: the caller archives it as before.
    Local,
    /// The cloud archive took it (the instance is kept or stopped as asked).
    Handled,
}

/// Which wake a worker follows.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Wake {
    /// A send: the message waits in the deferral, the pending flag guards it.
    Send,
    /// `cloud.resume`: only the instance is brought back.
    Resume,
}

/// How often a wake or a start is looked at.
const POLL: Duration = Duration::from_secs(2);

/// Why an operation on an instance is refused while the desktop works on it.
fn busy_failure(shell: &Shell, cloud: &CloudRef) -> Option<Failure> {
    if shell.mobile_auto_stopping(cloud) {
        return Some(Failure::coded(
            Code::AutoStopping.as_str(),
            crate::conversation::AUTO_STOPPING,
        ));
    }
    shell
        .mobile_instance_busy(cloud)
        .then(|| Code::InstanceBusy.failure())
}

/// An error text of a backend, made safe to leave the desktop (no path, URL,
/// command line or token), with a generic text when nothing of it is left.
fn safe_error(error: &str) -> Failure {
    let text = cloud_policy::sanitize_startup_error(error);
    if text == cloud_policy::sanitize_startup_error("") {
        Failure::from("The cloud request failed")
    } else {
        Failure::from(text)
    }
}

/// Starts the wake of session `id` on the UI thread's executor and follows it
/// until it ends, then releases `slot`. A wake that ends without a connection
/// also drops the send it carried (see `Conversation::drop_deferred_sends`).
/// Nothing waits for it.
pub(super) fn start_wake(
    shell: &mut Shell,
    id: i64,
    slot: WakeSlot,
    kind: Wake,
    cx: &mut Context<Shell>,
) {
    let Some(view) = shell.mobile_open_cloud_session(id, cx) else {
        cloud_policy::release_wake(&slot);
        crate::conversation::Conversation::end_wake_send(id);
        return;
    };
    let store = shell.mobile_database();
    cx.spawn(async move |this, cx| {
        let started = match kind {
            Wake::Send => crate::cloud_wake::wake_for_send(store, id, view, cx).await,
            Wake::Resume => view.update(cx, |view, cx| view.reconnect_cloud(cx)),
        };
        let mut failed = !started;
        if started {
            let polls = cloud_policy::WAKE_SLOT_TIMEOUT.as_secs() / POLL.as_secs();
            for _ in 0..polls {
                cx.background_executor().timer(POLL).await;
                match this.update(cx, |this, cx| this.mobile_wake_progress(id, cx)) {
                    Ok((true, _)) => {}
                    Ok((false, wake_failed)) => {
                        failed = wake_failed;
                        break;
                    }
                    Err(_) => break,
                }
            }
        }
        cloud_policy::release_wake(&slot);
        let _ = this.update(cx, |this, cx| {
            if failed && kind == Wake::Send {
                if let Some(view) = this.mobile_open_cloud_session(id, cx) {
                    view.update(cx, |view, _| view.drop_deferred_sends());
                }
                crate::conversation::Conversation::end_wake_send(id);
            }
            cx.notify();
        });
    })
    .detach();
}

/// `cloud.stop {sessionId}` or `{kind, instance}`: asks the instance to stop
/// the way the app API does (`CloudApi::stop_instance`). The instance is
/// checked against what the app knows (a session's, or one in the cached
/// listing), the request is answered at once with the sessions that work on it
/// and a `warning` naming those that are busy, and the stop runs on a worker
/// (it can take a `gh` call per project, longer than a request may wait).
/// `stopped` means requested; progress is the `stopReport` of
/// `cloud.instances`, and a stop that fails afterwards is shown to the desktop
/// user. Allowed on a `kept` instance. Refused with `instance_busy` while the
/// desktop archives or converts on it, `auto_stopping` while it stops it
/// itself, `not_selected` for an instance no selected project uses,
/// `unsupported_target` for a name the app does not know, `busy` beyond
/// `STOP_LIMIT_PER_MINUTE` stops per device (a stop saves money, so only a
/// loop is limited).
pub(super) fn stop(
    shell: &mut Shell,
    device: &DeviceContext,
    params: &Value,
    finish: impl FnOnce(&mut Shell, Result<Value, Failure>) + 'static,
    cx: &mut Context<Shell>,
) {
    let planned = (|| {
        let (cloud, project) = shell.mobile_cloud_subject(params, device, cx)?;
        if let Some(failure) = busy_failure(shell, &cloud) {
            return Err(failure);
        }
        cloud_policy::allow_stop(&device.id)?;
        Ok((cloud, project))
    })();
    let (cloud, project) = match planned {
        Ok(planned) => planned,
        Err(failure) => return finish(shell, Err(failure)),
    };
    let busy = shell.cloud_busy_snapshot(cx);
    let lookup: Vec<String> = match project {
        Some(project) => vec![project],
        None => shell.mobile_all_cloud_projects(cx),
    };
    let store = shell.mobile_database();
    let target_cloud = cloud.clone();
    let work = cx.background_executor().spawn(async move {
        let api = CloudApi::shared(&store)?;
        // The account a sandbox was listed under; the machine's login when it is not listed.
        let target = lookup
            .iter()
            .find_map(|project| {
                api.cloud()
                    .workspaces_overview(project, false)
                    .rows
                    .into_iter()
                    .find(|row| row.cloud == target_cloud)
                    .map(|row| row.target())
            })
            .unwrap_or(WorkspaceTarget {
                cloud: target_cloud,
                account: None,
            });
        api.stop_instance(&target, &busy)
    });
    let preview = shell.mobile_stop_preview(&cloud, cx);
    let reply = stop_reply(shell, device, &preview, cx);
    cx.spawn(async move |this, cx| {
        let result = work.await;
        let _ = this.update(cx, move |this, cx| match result {
            Ok(_) => this.mobile_cloud_changed(&cloud, cx),
            Err(error) => this.mobile_stop_failed(safe_error(&error).error, cx),
        });
    })
    .detach();
    finish(shell, Ok(reply));
}

/// The reply of a stop for `device`: only the sessions of its selected
/// projects are named, and the `warning` names only those, so a stop of a
/// shared instance does not tell what else ran there.
fn stop_reply(shell: &mut Shell, device: &DeviceContext, value: &Value, cx: &App) -> Value {
    let noun = value["kind"].as_str().unwrap_or("instance");
    let listed = value["sessions"].as_array().cloned().unwrap_or_default();
    let mut sessions = Vec::new();
    let (mut shown, mut hidden) = (Vec::new(), 0usize);
    let mut core_names = Vec::new();
    for item in &listed {
        let (Some(id), title) = (
            item["sessionId"].as_i64(),
            item["title"].as_str().unwrap_or(""),
        ) else {
            continue;
        };
        let busy = item["busy"].as_bool().unwrap_or(false);
        if busy {
            core_names.push(format!("\"{title}\" ({id})"));
        }
        match shell.mobile_session_use(id, device, cx) {
            Some((persisted, true, _)) => {
                if busy {
                    shown.push(format!("\"{title}\""));
                }
                sessions.push(json!({"sessionId": persisted, "title": title, "busy": busy}));
            }
            _ => hidden += busy as usize,
        }
    }
    let mut reply = json!({
        "stopped": true,
        "kind": value["kind"],
        "instance": value["instance"],
        "sessions": sessions,
    });
    let mut parts: Vec<String> = Vec::new();
    if !shown.is_empty() || hidden > 0 {
        let who = match (shown.is_empty(), hidden) {
            (false, 0) => shown.join(", "),
            (false, n) => format!("{} and {n} more", shown.join(", ")),
            (true, n) => format!("{n} from other projects"),
        };
        parts.push(format!(
            "Sessions that were working on this {noun} were interrupted: {who}."
        ));
    }
    if let Some(core) = value["warning"].as_str() {
        let sentence = format!(
            "Sessions that were working on this {noun} were interrupted: {}.",
            core_names.join(", ")
        );
        let notes = if core_names.is_empty() {
            core
        } else {
            core.strip_prefix(&sentence).unwrap_or("")
        }
        .trim();
        if !notes.is_empty() {
            parts.push(cloud_policy::sanitize_startup_error(notes));
        }
    }
    if !parts.is_empty() {
        reply["warning"] = json!(parts.join(" "));
    }
    reply
}

/// `cloud.resume {sessionId}`: wakes the instance of a session on a worker
/// and answers `{accepted: true}` at once; progress is the `cloud` field of
/// the session's watch frames. A wake that is already running is joined
/// without costing a second slot.
pub(super) fn resume(
    shell: &mut Shell,
    device: &DeviceContext,
    params: &Value,
    cx: &mut Context<Shell>,
) -> Result<Value, Failure> {
    let (id, cloud) = shell.mobile_cloud_session_of(params, device, cx)?;
    let view =
        super::card_view(shell, id, false).ok_or_else(|| Code::UnsupportedTarget.failure())?;
    if let Some(text) = shell.mobile_start_refusal(id) {
        return Err(super::send::starting_failure(&text));
    }
    match view.state {
        Some(monocode_core::cloud_sessions::CloudState::Starting) => {
            return Err(Code::Starting.failure());
        }
        Some(monocode_core::cloud_sessions::CloudState::Unavailable) => {
            return Err(Code::UnsupportedTarget.failure());
        }
        _ => {}
    }
    if let Some(failure) = busy_failure(shell, &cloud) {
        return Err(failure);
    }
    if shell.mobile_wake_progress(id, cx).0 {
        return Ok(json!({"accepted": true}));
    }
    let slot = cloud_policy::acquire_wake(&device.id)?;
    start_wake(shell, id, slot, Wake::Resume, cx);
    Ok(json!({"accepted": true}))
}

/// `cloud.retryStart {sessionId}`: runs a failed start again the way the
/// desktop's own Retry does, for a failed start (a `kept` instance is one).
/// Answers `{accepted: true}` at once; progress is the `startup` of the
/// `cloud` field. Rate limited like any call that can bill. A retry that would
/// create the instance (the failed start never did) needs "Remote cloud start"
/// (`disabled`); one on a kept instance needs only `control`.
pub(super) fn retry_start(
    shell: &mut Shell,
    device: &DeviceContext,
    params: &Value,
    cx: &mut Context<Shell>,
) -> Result<Value, Failure> {
    let (id, cloud) = shell.mobile_cloud_session_of(params, device, cx)?;
    if shell.mobile_start_running(id) {
        return Err(Code::Starting.failure());
    }
    if !shell.mobile_start_failed(id) {
        return Err(Code::UnsupportedTarget.failure());
    }
    if let Some(failure) = busy_failure(shell, &cloud) {
        return Err(failure);
    }
    // A retry on a kept instance (the failed start created it) only runs again what the
    // user already has; one that has to create the instance is a cloud start.
    if shell.mobile_retry_creates(id) && !policy::flags().remote_cloud_start {
        return Err(Code::Disabled.failure());
    }
    let slot = cloud_policy::acquire_wake(&device.id)?;
    shell.mobile_retry_start(id, cx);
    cx.spawn(async move |this, cx| {
        let polls = cloud_policy::WAKE_SLOT_TIMEOUT.as_secs() / POLL.as_secs();
        for _ in 0..polls {
            cx.background_executor().timer(POLL).await;
            match this.update(cx, |this, _| this.mobile_start_running(id)) {
                Ok(true) => {}
                _ => break,
            }
        }
        cloud_policy::release_wake(&slot);
    })
    .detach();
    Ok(json!({"accepted": true}))
}

/// `cloud.keepRunning {sessionId}`: cancels the announced auto-stop of the
/// session's instance (it starts a fresh idle window). Refused with
/// `auto_stopping` once the stop is running: it cannot be taken back.
pub(super) fn keep_running(
    shell: &mut Shell,
    device: &DeviceContext,
    params: &Value,
    cx: &mut Context<Shell>,
) -> Result<Value, Failure> {
    let (_, cloud) = shell.mobile_cloud_session_of(params, device, cx)?;
    if shell.mobile_auto_stopping(&cloud) {
        return Err(Failure::coded(
            Code::AutoStopping.as_str(),
            crate::conversation::AUTO_STOPPING,
        ));
    }
    shell.mobile_keep_running(&cloud);
    cx.notify();
    Ok(json!({"keepRunning": true}))
}

/// Before a session is archived: a cloud session of a web device needs
/// `cloud: "keep"` or `"stop"` (D10, like the app API) and goes through the
/// cloud archive, which saves a local copy first. `delete` is never offered.
/// Refused with `instance_busy` while the desktop archives or converts.
pub(super) fn before_archive(
    shell: &mut Shell,
    device: &DeviceContext,
    session: i64,
    params: &Value,
    window: &mut Window,
    cx: &mut Context<Shell>,
) -> Result<Archive, Failure> {
    if device.kind != DeviceKind::Web {
        // A phone has no keep/stop choice to make: its archive of a session bound to an
        // instance would skip it (the instance would keep billing), so it is refused (H2).
        return if shell.mobile_cloud_bound(session) {
            Err(Code::UnsupportedTarget.failure())
        } else {
            Ok(Archive::Local)
        };
    }
    let Some(view) = super::card_view(shell, session, false).filter(|view| view.is_cloud()) else {
        return Ok(Archive::Local);
    };
    let choice = match params["cloud"].as_str() {
        Some(choice @ ("keep" | "stop")) => choice,
        _ => {
            return Err(Failure::coded(
                Code::UnsupportedTarget.as_str(),
                "Choose whether the cloud instance keeps running or stops",
            ));
        }
    };
    drop(view);
    if shell.mobile_archive_busy(session) {
        return Err(Code::InstanceBusy.failure());
    }
    shell.mobile_archive_cloud(session, choice, window, cx);
    Ok(Archive::Handled)
}

/// Before a session is deleted: nobody remote deletes a session that is bound
/// to a cloud instance (it would orphan a billable instance) or that lives on
/// a remote machine (D7, `unsupported_target`). A session counts as bound when
/// the app has a mark, a binding in the store or a start record for it, not
/// only what the catalog shows (L2). A phone's delete of any other session
/// only removes the local row, as it always did, and is left as it is.
pub(super) fn before_delete(
    shell: &mut Shell,
    device: &DeviceContext,
    session: i64,
    _cx: &mut Context<Shell>,
) -> Result<(), Failure> {
    if shell.mobile_cloud_bound(session) {
        return Err(Code::UnsupportedTarget.failure());
    }
    if device.kind != DeviceKind::Web {
        return Ok(());
    }
    let remote = shell.mobile_remote_cwd(session)
        || super::card_view(shell, session, false)
            .is_some_and(|view| view.placement != super::view::Placement::Local);
    if remote {
        return Err(Code::UnsupportedTarget.failure());
    }
    Ok(())
}

/// Sets `kept` and `auto_stop` of the `cloud` field for a session.
pub(super) fn extras(shell: &Shell, session: i64, cx: &App, extras: &mut Extras) {
    if let Some(facts) = shell.mobile_cloud_facts(session) {
        extras.kept = facts.binding.is_some()
            && facts
                .start
                .as_ref()
                .is_some_and(|record| record.is_failed() && record.created);
    }
    extras.auto_stop = shell
        .mobile_auto_stop(session, cx)
        .map(|(at, reason)| AutoStop {
            at,
            reason: reason.into(),
        });
}
