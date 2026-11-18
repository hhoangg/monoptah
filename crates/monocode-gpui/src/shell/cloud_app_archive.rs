//! `sessions.archive` with `cloud`, for the Operator app API and for Monos: the same flow as
//! archiving a cloud session in the window (save a local copy, then keep, stop or delete the
//! workspace), driven by an app call instead of the dialog.
use super::cloud::{
    ArchiveChoice, CANCELLED, ConvertFn, GroupOutcome, NEEDS_FILES_ONLY, NEEDS_REVIEW, Then,
    WORKSPACE_BUSY, archive_groups, group_by_codespace,
};
use super::*;
use monocode_core::cloud_sessions::{CloudBackendKind, CloudRef, CloudState};
use serde_json::{Value, json};
use std::sync::atomic::{AtomicBool, Ordering};

/// Why an app archive did not finish, and whether the session got archived anyway (its local
/// copy was saved), so a caller undoes its own bookkeeping only when nothing was archived.
#[derive(Debug)]
pub(crate) struct ArchiveFailure {
    pub message: String,
    pub archived: bool,
}
impl ArchiveFailure {
    fn untouched(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            archived: false,
        }
    }
}

impl ArchiveChoice {
    fn parse(text: &str) -> Result<Self, String> {
        match text {
            "keep" => Ok(Self::Keep),
            "stop" => Ok(Self::Stop),
            "delete" => Ok(Self::Delete),
            _ => Err("cloud must be \"keep\", \"stop\" or \"delete\"".into()),
        }
    }
    fn word(self) -> &'static str {
        match self {
            Self::Keep => "keep",
            Self::Stop => "stop",
            Self::Delete => "delete",
        }
    }
}

impl Shell {
    /// The instance action of `sessions.delete` with `cloud: "stop" | "delete"` for a cloud
    /// session; `None` otherwise (the instance is left alone).
    pub(super) fn app_delete_instance_task(
        &self,
        id: i64,
        input: &Value,
    ) -> Option<super::project_removal::InstanceTask> {
        let action = match input["cloud"].as_str()? {
            "stop" => monocode_core::cloud_sessions::InstanceAction::Stop,
            "delete" => monocode_core::cloud_sessions::InstanceAction::Delete,
            _ => return None,
        };
        // The window's mark, else the stored binding (a session the sidebar has not marked yet).
        let target = match self.cloud_marks.get(&id) {
            Some(mark) => mark.cloud(),
            None => locked(&self.store)
                .ok()?
                .remote_session(id)
                .ok()??
                .project
                .cloud()?,
        };
        Some(super::project_removal::InstanceTask {
            target,
            action,
            selected: vec![id],
        })
    }

    /// Archives cloud session `id` and applies `choice` ("keep", "stop" or "delete") to its
    /// workspace. Nothing is stopped or deleted unless the local copy is complete. `reply` gets
    /// the result, or why the workspace was left as it was.
    pub(crate) fn app_archive_cloud(
        &mut self,
        id: i64,
        choice: &str,
        backup: bool,
        reply: async_channel::Sender<Result<Value, ArchiveFailure>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let choice = match ArchiveChoice::parse(choice) {
            Ok(choice) => choice,
            Err(error) => {
                let _ = reply.try_send(Err(ArchiveFailure::untouched(error)));
                return;
            }
        };
        let Some(mark) = self.cloud_marks.get(&id).cloned() else {
            let _ = reply.try_send(Err(ArchiveFailure::untouched(
                "cloud only applies to a GitHub Codespaces or Docker Sandbox session",
            )));
            return;
        };
        let target = mark.cloud();
        let noun = mark.kind.noun();
        if self.cloud_busy.contains(&id)
            || self.archiving.contains(&id)
            || self.cloud_archive.is_some()
        {
            let _ = reply.try_send(Err(ArchiveFailure::untouched(
                "This session is being archived or another cloud archive is open. Nothing was changed",
            )));
            return;
        }
        if self.cloud_workspaces_busy.contains(&target.key()) {
            let _ = reply.try_send(Err(ArchiveFailure::untouched(format!(
                "{} Nothing was changed.",
                WORKSPACE_BUSY
            ))));
            return;
        }
        let groups = group_by_codespace(&[id], &self.cloud_marks, |target, selected| {
            self.other_users_of(target, selected)
        });
        let Some(group) = groups.into_iter().next() else {
            let _ = reply.try_send(Err(ArchiveFailure::untouched("Session not found")));
            return;
        };
        if choice == ArchiveChoice::Delete && group.others > 0 {
            let _ = reply.try_send(Err(ArchiveFailure::untouched(format!(
                "{} other session(s) that are not archived still use the {noun} {}; it was not deleted and nothing was changed. Archive them first, or use cloud \"stop\" or \"keep\".",
                group.others, target.name
            ))));
            return;
        }
        if choice != ArchiveChoice::Keep
            && self.cloud_marks.iter().any(|(other, m)| {
                *other != id
                    && m.cloud() == target
                    && self
                        .remote_marks
                        .get(other)
                        .is_some_and(|(running, _)| *running)
            })
        {
            let _ = reply.try_send(Err(ArchiveFailure::untouched(format!(
                "Another session is running in the {noun} {}; it was left as it was and nothing was changed.",
                target.name
            ))));
            return;
        }
        if let Some(view) = self.sessions.get(&id).cloned() {
            view.update(cx, |v, cx| v.stop_app_session(cx));
        }
        // Delete saves every session of the workspace, so all of them stop taking messages.
        let mut locked_ids = vec![id];
        if choice == ArchiveChoice::Delete {
            locked_ids.extend(
                self.cloud_marks
                    .iter()
                    .filter(|(other, m)| **other != id && m.cloud() == target)
                    .map(|(other, _)| *other),
            );
        }
        self.cloud_busy.extend(locked_ids.iter().copied());
        let key = target.key();
        self.cloud_workspaces_busy.insert(key.clone());
        self.lock_cloud_composer(&locked_ids, true, cx);
        // A copy without the conversation never reaches the stop or delete: the conversion
        // trips `cancel`, which `archive_groups` checks before the workspace call.
        let cancel = Arc::new(AtomicBool::new(false));
        let lost_history = Arc::new(AtomicBool::new(false));
        let inner = self.cloud_convert.clone();
        let convert: ConvertFn = {
            let (cancel, lost) = (cancel.clone(), lost_history.clone());
            Arc::new(move |cloud, session, files_only, progress| {
                let converted = inner(cloud, session, files_only, progress)?;
                if !converted.history_saved && choice != ArchiveChoice::Keep {
                    lost.store(true, Ordering::Relaxed);
                    cancel.store(true, Ordering::Relaxed);
                }
                Ok(converted)
            })
        };
        let cloud = self.cloud.clone();
        let work_group = group.clone();
        let work = cx.background_executor().spawn(async move {
            archive_groups(
                &cloud,
                vec![work_group],
                choice,
                backup,
                &convert,
                &cancel,
                &[],
                &[],
                &mut |_| {},
            )
        });
        cx.spawn_in(window, async move |this, cx| {
            let outcomes = work.await;
            let lost = lost_history.load(Ordering::Relaxed);
            let _ = this.update_in(cx, |this, window, cx| {
                this.lock_cloud_composer(&locked_ids, false, cx);
                this.cloud_workspaces_busy.remove(&key);
                let report = outcomes
                    .first()
                    .map(|outcome| archive_report(id, choice, lost, outcome))
                    .unwrap_or_else(|| Err(ArchiveFailure::untouched("Nothing was archived")));
                // Never through the dialog: the user may have one of their own open.
                this.settle_cloud_archive(choice, locked_ids, outcomes, window, cx);
                this.invalidate_chrome(cx);
                cx.notify();
                let _ = reply.try_send(report);
            });
        })
        .detach();
    }
}

/// What the app call answers for one group's outcome.
fn archive_report(
    id: i64,
    choice: ArchiveChoice,
    lost_history: bool,
    outcome: &GroupOutcome,
) -> Result<Value, ArchiveFailure> {
    let group = &outcome.group;
    let noun = group.kind.noun();
    let kept = format!("The {noun} {} was kept.", group.codespace);
    let saved = outcome.converted.iter().find(|c| c.cloud_id == id);
    let copy = saved.map_or(String::new(), |c| {
        format!(" The local copy is archived as session {}.", c.local_id)
    });
    let report = |action: &str, done: bool, warning: Option<String>, archived: bool| {
        let mut value = json!({
            "sessionId": id,
            "archived": archived,
            "cloud": {"kind": noun, "name": group.codespace, "action": action, "done": done},
        });
        if let Some(c) = saved {
            value["archivedSessionId"] = json!(c.local_id);
        }
        let mut warnings: Vec<String> = outcome
            .converted
            .iter()
            .flat_map(|c| c.warnings.iter().cloned())
            .collect();
        warnings.extend(warning);
        if saved.is_some_and(|c| !c.history_saved) {
            warnings.push("The conversation could not be saved; only the code was.".into());
        }
        if !warnings.is_empty() {
            value["warning"] = json!(warnings.join(" "));
        }
        value
    };
    match &outcome.result {
        Ok((Then::Archive, warning)) => Ok(report(choice.word(), true, warning.clone(), true)),
        Ok((Then::Forget, warning)) => {
            let note = saved.is_none().then(|| {
                format!(
                    "The {noun} was deleted and its sessions were dropped from the app.{}",
                    if outcome.converted.is_empty() {
                        " No local copy was needed or made."
                    } else {
                        ""
                    }
                )
            });
            let warning = [warning.clone(), note]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>()
                .join(" ");
            Ok(report(
                choice.word(),
                true,
                (!warning.is_empty()).then_some(warning),
                saved.is_some(),
            ))
        }
        Err(error) => {
            // What was done decides the wording and the undo: the target counts as archived
            // only when its local copy was saved, whatever stopped the rest.
            let failed = |message: String| {
                Err(ArchiveFailure {
                    message,
                    archived: saved.is_some(),
                })
            };
            let nothing = if saved.is_some() {
                String::new()
            } else {
                " Nothing was archived.".to_string()
            };
            if error == CANCELLED && lost_history {
                failed(format!(
                    "The conversation of the session could not be saved (only its code was), so the {noun} {} was not stopped or deleted.{copy}",
                    group.codespace
                ))
            } else if error == CANCELLED {
                failed(format!("Cancelled.{nothing} {kept}{copy}"))
            } else if error == NEEDS_FILES_ONLY {
                failed(format!(
                    "The conversation of a session in the {noun} could not be reached.{nothing} {kept}{copy} Retry later."
                ))
            } else if error == NEEDS_REVIEW {
                failed(format!(
                    "The {noun} holds files the local copy lacks ({}), so it was not deleted. The user must review them in the app (the session menu offers Delete {noun}), then delete it there. {kept}{copy}",
                    outcome.risky.join("; ")
                ))
            } else if saved.is_some() {
                Ok(report(
                    choice.word(),
                    false,
                    Some(format!(
                        "{error}. The session is archived; the {noun} {} is still there.",
                        group.codespace
                    )),
                    true,
                ))
            } else {
                failed(format!("{error}.{nothing} {kept}"))
            }
        }
    }
}

// MARK: remote devices (WS-D4, H-10, H-12)
//
// What `mobile/cloud/*` needs from the shell, so those modules read no private field: plain
// answers from cached state and the one place each worker is started. Nothing here calls out
// on the UI thread.

/// How many seconds before an idle stop the `cloud` field of a session announces it.
pub(crate) const AUTO_STOP_NOTICE_SECS: u64 = 5 * 60;

impl Shell {
    /// The local database, for a worker.
    pub(crate) fn mobile_database(&self) -> crate::runtime::Database {
        self.store.clone()
    }

    /// The cloud session a request names with `sessionId`, and its instance. The selection
    /// applies as for every other request; a session that is not on a codespace or sandbox
    /// answers `unsupported_target`.
    pub(crate) fn mobile_cloud_session_of(
        &self,
        params: &Value,
        device: &monocode_core::mobile::DeviceContext,
        cx: &App,
    ) -> Result<(i64, CloudRef), monocode_core::mobile::policy::Failure> {
        use monocode_core::mobile::policy::{Code, Failure};
        let text = params["sessionId"]
            .as_str()
            .filter(|text| !text.is_empty())
            .ok_or_else(|| Failure::from("Name a session"))?;
        let id = self.mobile_cloud_session(text, device, cx)?;
        let mark = self
            .cloud_marks
            .get(&id)
            .ok_or_else(|| Code::UnsupportedTarget.failure())?;
        Ok((id, mark.cloud()))
    }

    /// The instance a `cloud.stop` names: a session's, or `kind` and `instance`. A device
    /// that does not see every project may stop only an instance one of its selected
    /// projects' sessions uses. The project is where the instance is listed from, when a
    /// session is known.
    pub(crate) fn mobile_cloud_subject(
        &self,
        params: &Value,
        device: &monocode_core::mobile::DeviceContext,
        cx: &App,
    ) -> Result<(CloudRef, Option<String>), monocode_core::mobile::policy::Failure> {
        use monocode_core::mobile::policy::{Code, Failure};
        if params.get("sessionId").is_some() {
            let (id, cloud) = self.mobile_cloud_session_of(params, device, cx)?;
            let project = self.cloud_marks.get(&id).map(|m| m.local_project.clone());
            return Ok((cloud, project));
        }
        let kind = params["kind"]
            .as_str()
            .and_then(CloudBackendKind::from_api_name)
            .ok_or_else(|| Failure::from("Name a session, or a kind and an instance"))?;
        let name = params["instance"]
            .as_str()
            .filter(|name| !name.is_empty() && name.len() <= 200)
            .ok_or_else(|| Failure::from("Name a session, or a kind and an instance"))?;
        let cloud = CloudRef {
            kind,
            name: name.to_owned(),
        };
        let users: Vec<&super::cloud::CloudMark> = self
            .cloud_marks
            .values()
            .filter(|mark| mark.cloud() == cloud)
            .collect();
        // Only an instance this app knows: one a session uses, or one a recent
        // `cloud.instances` reply listed (under a project the device may see). Any other
        // name could be a codespace or sandbox the app does not own.
        let listed = crate::mobile::cloud::instance_listed_project(&cloud);
        if users.is_empty() && listed.is_none() {
            return Err(Code::UnsupportedTarget.failure());
        }
        let allowed = device.projects.is_none()
            || users.iter().any(|mark| {
                super::mobile_host::in_selection(
                    &device.projects,
                    &super::mobile_host::project_id(&mark.local_project),
                )
            })
            || listed.as_ref().is_some_and(|project| {
                super::mobile_host::in_selection(
                    &device.projects,
                    &super::mobile_host::project_id(project),
                )
            });
        if !allowed {
            return Err(Code::NotSelected.failure());
        }
        Ok((
            cloud,
            users
                .first()
                .map(|mark| mark.local_project.clone())
                .or(listed),
        ))
    }

    /// The cloud instance of session `id`, if it runs on one.
    pub(crate) fn mobile_cloud_ref_of(&self, id: i64) -> Option<CloudRef> {
        self.cloud_marks.get(&id).map(|mark| mark.cloud())
    }

    /// The desktop is stopping this instance on its own now (the auto-stop worker runs).
    pub(crate) fn mobile_auto_stopping(&self, cloud: &CloudRef) -> bool {
        self.auto_stop_running.contains(&cloud.key())
    }

    /// The desktop archives, converts or stops on this instance now.
    pub(crate) fn mobile_instance_busy(&self, cloud: &CloudRef) -> bool {
        self.cloud_workspaces_busy.contains(&cloud.key())
            || self.auto_stop_running.contains(&cloud.key())
            || self.cloud_marks.iter().any(|(id, mark)| {
                mark.cloud() == *cloud
                    && (self.cloud_busy.contains(id) || self.archiving.contains(id))
            })
    }

    /// An archive of session `id` runs, or another cloud archive is open, so a new one is
    /// refused ([`Self::app_archive_cloud`] checks the same).
    pub(crate) fn mobile_archive_busy(&self, id: i64) -> bool {
        self.cloud_busy.contains(&id)
            || self.archiving.contains(&id)
            || self.cloud_archive.is_some()
            || self.workspace_busy_of(id)
    }

    /// Why a send to a session whose start has not finished is refused: the start in progress
    /// or its failure. `None` when the session is not starting.
    pub(crate) fn mobile_start_refusal(&self, id: i64) -> Option<String> {
        let store = locked(&self.store).ok()?;
        monocode_core::cloud_sessions::starting_refusal(&store, id)
    }

    /// The start of session `id` failed and waits for a retry.
    pub(crate) fn mobile_start_failed(&self, id: i64) -> bool {
        self.cloud_start_failed(id)
            || locked(&self.store)
                .ok()
                .and_then(|store| monocode_core::cloud_sessions::start_record(&store, id))
                .is_some_and(|record| record.is_failed())
    }

    /// Follows the start of session `id` again, as the Retry button does.
    pub(crate) fn mobile_retry_start(&mut self, id: i64, cx: &mut Context<Self>) {
        self.register_cloud_start(id);
        self.track_cloud_start(id, cx);
    }

    /// Whether the start worker of session `id` is still running.
    pub(crate) fn mobile_start_running(&self, id: i64) -> bool {
        self.cloud_start_snapshot(id)
            .is_some_and(|snapshot| snapshot.running)
    }

    /// What `cloud.instances` says of session `id`: its catalog id, whether its project is in
    /// the device's selection and whether it is busy; `None` when the session is unknown.
    pub(crate) fn mobile_session_use(
        &mut self,
        id: i64,
        device: &monocode_core::mobile::DeviceContext,
        cx: &App,
    ) -> Option<(String, bool, bool)> {
        let cwd = self.session_summaries.get(&id)?.cwd.clone();
        let project = self.project_of(id, &cwd).to_owned();
        let busy = self.session_metadata(id, cx).busy;
        Some((
            self.mobile_session_id(id),
            self.mobile_project_selected(device, &project),
            busy,
        ))
    }

    /// The project path is in the device's selection (all when it has none).
    pub(crate) fn mobile_project_selected(
        &self,
        device: &monocode_core::mobile::DeviceContext,
        project: &str,
    ) -> bool {
        super::mobile_host::in_selection(&device.projects, &super::mobile_host::project_id(project))
    }

    /// The local projects whose cloud instances a device may list: selected, and with a cloud
    /// setting or session. A store read, no network.
    pub(crate) fn mobile_cloud_projects(
        &self,
        device: &monocode_core::mobile::DeviceContext,
        cx: &App,
    ) -> Vec<String> {
        self.mobile_projects(cx)
            .into_iter()
            .filter(|path| !path.starts_with(monocode_core::remote::REMOTE_PATH_PREFIX))
            .filter(|path| self.mobile_project_selected(device, path))
            .filter(|path| self.cloud.project_uses_cloud(path))
            .collect()
    }

    /// Every local project that uses a cloud, selected or not (where an instance a device
    /// named by kind and name is listed from).
    pub(crate) fn mobile_all_cloud_projects(&self, cx: &App) -> Vec<String> {
        self.mobile_projects(cx)
            .into_iter()
            .filter(|path| !path.starts_with(monocode_core::remote::REMOTE_PATH_PREFIX))
            .filter(|path| self.cloud.project_uses_cloud(path))
            .collect()
    }

    /// The cached state of `cloud` is stale after a stop: read it again.
    pub(crate) fn mobile_cloud_changed(&mut self, cloud: &CloudRef, cx: &mut Context<Self>) {
        self.cloud_states.remove(&cloud.key());
        self.refresh_cloud(cx);
        cx.notify();
    }

    /// The conversation of a cloud session, opened if need be (it does not load a history
    /// and never wakes anything by itself).
    pub(crate) fn mobile_open_cloud_session(
        &mut self,
        id: i64,
        cx: &mut Context<Self>,
    ) -> Option<Entity<crate::conversation::Conversation>> {
        self.ensure_session(id, cx)
    }

    /// How the wake of session `id` stands: `(waking, failed)`. A session that is gone has
    /// failed. See `Conversation::is_waking`.
    pub(crate) fn mobile_wake_progress(&self, id: i64, cx: &App) -> (bool, bool) {
        self.sessions.get(&id).map_or((false, true), |view| {
            let view = view.read(cx);
            (view.is_waking(), view.wake_failed())
        })
    }

    /// Tells the desktop user that a stop a remote device asked for did not go through (the
    /// device was already answered: the stop runs after the answer).
    pub(crate) fn mobile_stop_failed(&mut self, error: String, cx: &mut Context<Self>) {
        self.error = Some(error);
        cx.notify();
    }

    /// What a `cloud.stop` of `cloud` reports at once, from the store and the live views (the
    /// same reading the stop itself makes, `CloudApi::busy_sessions_on`): the sessions of
    /// this app on it with their titles and whether they work now. The stop runs on a
    /// worker afterwards.
    pub(crate) fn mobile_stop_preview(&self, cloud: &CloudRef, cx: &App) -> Value {
        use monocode_core::mono::cloud_api::AppBusy;
        let busy = self.cloud_busy_snapshot(cx);
        let idle = matches!(
            self.cloud_states.get(&cloud.key()),
            Some(CloudState::Stopped | CloudState::Unavailable)
        );
        let sessions: Vec<Value> = self
            .cloud
            .sessions_on(cloud)
            .into_iter()
            .map(|mut session| {
                if idle {
                    session.running = false;
                    session.needs_input = false;
                } else if busy.session_busy(session.id) {
                    session.running = true;
                }
                json!({"sessionId": session.id, "title": session.title, "busy": session.busy()})
            })
            .collect();
        json!({"kind": cloud.kind.api_name(), "instance": cloud.name, "sessions": sessions})
    }

    /// The session is bound to a cloud instance as far as the app knows: a mark, a binding
    /// in the store or a start record (also a failed one, which holds a billable instance).
    pub(crate) fn mobile_cloud_bound(&self, id: i64) -> bool {
        if self.cloud_marks.contains_key(&id) {
            return true;
        }
        let Ok(store) = locked(&self.store) else {
            return false;
        };
        store
            .remote_session(id)
            .ok()
            .flatten()
            .is_some_and(|remote| remote.project.cloud().is_some())
            || monocode_core::cloud_sessions::start_record(&store, id).is_some()
    }

    /// The session is the conversation of a cloud workspace: its folder is remote and it is
    /// bound to an instance, as far as the app knows (also before the marks are loaded).
    pub(crate) fn mobile_is_cloud_conversation(&self, id: i64) -> bool {
        (self.mobile_remote_cwd(id) && self.mobile_cloud_bound(id)) || self.mobile_cloud_located(id)
    }

    /// A moved session: a local row whose store location is Cloud, so its turns run on the
    /// linked Host session and a send wakes the instance (`ensure_before_send`). An unreadable
    /// location counts as Cloud (fail closed, like the store's own guards).
    pub(crate) fn mobile_cloud_located(&self, id: i64) -> bool {
        locked(&self.store)
            .map(|store| store.cloud_location(id).map_or(true, |l| l.is_cloud()))
            .unwrap_or(false)
    }

    /// The session's own folder is on a remote machine (`remote://`).
    pub(crate) fn mobile_remote_cwd(&self, id: i64) -> bool {
        self.session_summaries.get(&id).is_some_and(|summary| {
            summary
                .cwd
                .starts_with(monocode_core::remote::REMOTE_PATH_PREFIX)
        })
    }

    /// Retrying the failed start of session `id` would create a new instance: the failed
    /// start never created one (a kept one is retried in place).
    pub(crate) fn mobile_retry_creates(&self, id: i64) -> bool {
        let Ok(store) = locked(&self.store) else {
            return true;
        };
        !monocode_core::cloud_sessions::start_record(&store, id)
            .is_some_and(|record| record.created)
    }

    /// Archives cloud session `id` with `choice` ("keep" or "stop") like the app API, and
    /// tells the desktop user how it ended: a failure on the error line, a warning as a
    /// notice. The remote device is answered at once; the archive takes minutes.
    pub(crate) fn mobile_archive_cloud(
        &mut self,
        id: i64,
        choice: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (reply, answer) = async_channel::bounded(1);
        self.app_archive_cloud(id, choice, true, reply, window, cx);
        cx.spawn(async move |this, cx| {
            let outcome = answer.recv().await;
            let _ = this.update(cx, |this, cx| {
                match outcome {
                    Ok(Err(failure)) => this.error = Some(failure.message),
                    Ok(Ok(report)) => {
                        if let Some(warning) = report["warning"].as_str() {
                            this.notice = Some(warning.to_owned());
                        }
                    }
                    Err(_) => {}
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// The announced auto-stop of the instance of session `id` as `(at in ms, reason)`: the
    /// stop is running now, or the idle stop is at most [`AUTO_STOP_NOTICE_SECS`] away and
    /// nothing is keeping the instance busy. Memory only. Typing in a session counts as
    /// activity (it moves the idle stop away); watching never does.
    ///
    /// `at` is a fixed deadline: the idle window's end for an idle stop, the moment the stop
    /// was first seen running for a stop in progress. It never moves between two frames, so
    /// the watch (which sends the field when it changed) stays quiet.
    pub(crate) fn mobile_auto_stop(&self, id: i64, cx: &App) -> Option<(u64, &'static str)> {
        let mark = self.cloud_marks.get(&id)?;
        let cloud = mark.cloud();
        let base = epoch_wall_ms(self.auto_stop_epoch);
        if self.auto_stop_running.contains(&cloud.key()) {
            let started = stop_first_seen(&cloud.key(), Some(base + self.auto_stop_now() * 1000));
            return Some((started.unwrap_or(0), "idle"));
        }
        stop_first_seen(&cloud.key(), None);
        if self.cloud_states.get(&cloud.key()) != Some(&CloudState::Available) {
            return None;
        }
        let minutes = self
            .workspace
            .preferences
            .cloud_auto_stop
            .policy(cloud.kind)
            .idle_minutes?;
        let facts = self.auto_stop_facts(cx);
        let workspace = facts.iter().find(|w| w.cloud == cloud)?;
        let active = workspace.app_working
            || workspace.sessions.is_empty()
            || workspace
                .sessions
                .iter()
                .any(|s| s.busy || s.pending || s.unknown);
        if active {
            return None;
        }
        let now = self.auto_stop_now();
        let last_input = workspace
            .sessions
            .iter()
            .filter_map(|s| s.last_input)
            .max()
            .unwrap_or(0);
        // Not checked yet: nothing is announced until the idle window has a start.
        let since = self.auto_stop.idle_since(&cloud)?.max(last_input);
        let deadline = since + u64::from(minutes) * 60;
        (deadline.saturating_sub(now) <= AUTO_STOP_NOTICE_SECS)
            .then_some((base + deadline * 1000, "idle"))
    }

    /// `cloud.keepRunning`: starts a fresh idle window for the instance and exempts it from
    /// being stopped for having no session left, so the announced stop does not happen.
    /// There is no standing "never auto-stop"; the next idle window can stop it again.
    pub(crate) fn mobile_keep_running(&mut self, cloud: &CloudRef) {
        let now = self.auto_stop_now();
        self.auto_stop.resumed(cloud, now);
        self.auto_stop.keep_running(cloud);
    }
}

/// The wall clock (ms since the Unix epoch) at the auto-stop clock's start, fixed the first
/// time it is asked for: deadlines derived from it do not move with the clock's jitter.
fn epoch_wall_ms(epoch: std::time::Instant) -> u64 {
    thread_local! {
        static BASES: std::cell::RefCell<std::collections::HashMap<std::time::Instant, u64>> =
            std::cell::RefCell::new(std::collections::HashMap::new());
    }
    BASES.with(|bases| {
        *bases.borrow_mut().entry(epoch).or_insert_with(|| {
            monocode_core::time::now_ms().saturating_sub(epoch.elapsed().as_millis() as u64)
        })
    })
}

/// When an auto-stop of the instance `key` was first seen running (ms): remembered while it
/// runs (`seen` is the value to record the first time), forgotten when asked with `None`.
fn stop_first_seen(key: &str, seen: Option<u64>) -> Option<u64> {
    thread_local! {
        static SEEN: std::cell::RefCell<std::collections::HashMap<String, u64>> =
            std::cell::RefCell::new(std::collections::HashMap::new());
    }
    SEEN.with(|map| {
        let mut map = map.borrow_mut();
        match seen {
            Some(at) => Some(*map.entry(key.to_owned()).or_insert(at)),
            None => {
                map.remove(key);
                None
            }
        }
    })
}
