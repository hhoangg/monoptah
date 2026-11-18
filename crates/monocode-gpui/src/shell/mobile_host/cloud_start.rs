//! The shell side of the remote cloud start and the moves (H-11, H-13, H-14):
//! the pieces of `mobile::cloud::{start, moves}` that need the shell's private
//! state, and the start itself.
//!
//! A cloud start never blocks the UI thread or the host's reply wait: the
//! values are checked against the start options on a worker, the rate limit
//! slot is claimed, `CloudApi::begin_start` (the call the app API makes) records
//! the session and requests the instance on a worker, and the reply carries the
//! new session id. The instance then comes up in the background and its
//! progress reaches the web through the `cloud` field of the watch frames.
use super::*;
use crate::cloud_start_view::StartPhase;
use crate::mobile::cloud::{CloudStartPlan, OptionsProbe};
use crate::shell::cloud::CloudMark;
use monocode_core::cloud_sessions::{CloudBackendKind, CloudSessions, move_session::MoveStep};
use monocode_core::mobile::cloud_policy::{self, WakeSlot, sanitize_startup_error};
use monocode_core::mono::cloud_api::CloudApi;

/// How often a held wake slot checks whether its operation ended.
const SLOT_POLL: Duration = Duration::from_secs(2);

/// How long a cloud start may take to record its session before the request is answered
/// with `{accepted, starting}` (the host stops waiting for a reply after 25 s).
pub(super) const START_REPLY_BOUND: Duration = Duration::from_secs(20);

/// Answers once: the first of the start's own reply and the time bound. A later one is dropped
/// (false).
pub(super) fn answer_once<F: FnOnce(&mut Shell, Result<Value, Failure>)>(
    done: &std::cell::RefCell<Option<F>>,
    shell: &mut Shell,
    reply: Result<Value, Failure>,
) -> bool {
    match done.borrow_mut().take() {
        Some(done) => {
            done(shell, reply);
            true
        }
        None => false,
    }
}

/// What the start needs from the request, owned.
pub(super) struct CloudStartRequest {
    pub device: DeviceContext,
    pub command_id: String,
    pub text: String,
    pub cwd: String,
    /// The `runtimeMode` the device chose (never full access), if any.
    pub access: Option<AccessMode>,
    pub params: Value,
    pub plan: CloudStartPlan,
}

impl Shell {
    /// The project a web device's call names, checked against its selection
    /// (`not_selected`) and refused for a remote machine (`unsupported_target`).
    pub(crate) fn mobile_start_project(
        &self,
        id: &str,
        device: &DeviceContext,
        cx: &App,
    ) -> Result<String, Failure> {
        self.mobile_project_path(id, device, cx)
    }

    /// The cloud service whose caches the start options read.
    pub(crate) fn mobile_cloud_service(&self) -> Arc<CloudSessions> {
        self.cloud.clone()
    }

    /// Names of the `kind` instances that sessions of `project` use. Cached
    /// marks only: nothing is called.
    pub(crate) fn mobile_cloud_instances(
        &self,
        project: &str,
        kind: CloudBackendKind,
    ) -> Vec<String> {
        let key = monocode_core::files::path_key(project);
        let mut names: Vec<String> = self
            .cloud_marks
            .values()
            .filter(|mark| {
                mark.kind == kind && monocode_core::files::path_key(&mark.local_project) == key
            })
            .map(|mark: &CloudMark| mark.codespace.clone())
            .collect();
        names.sort();
        names.dedup();
        names
    }

    /// Keeps `slot` claimed until `ended` says the operation is over, then
    /// frees it. The limiter frees it by itself after `WAKE_SLOT_TIMEOUT` when
    /// this never learns the end.
    pub(crate) fn hold_wake_slot(
        &mut self,
        slot: WakeSlot,
        ended: impl Fn(&Shell) -> bool + 'static,
        cx: &mut Context<Self>,
    ) {
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(SLOT_POLL).await;
                if this.update(cx, |this, _| ended(this)).unwrap_or(true) {
                    break;
                }
            }
            cloud_policy::release_wake(&slot);
        })
        .detach();
    }

    /// Whether the start of session `id` is over (ready, failed or gone).
    fn cloud_start_over(&self, id: i64) -> bool {
        self.cloud_start_snapshot(id)
            .is_none_or(|snapshot| snapshot.phase != StartPhase::Starting)
    }

    /// Starts a cloud session for a web device. `done` gets the reply: the
    /// new session id as soon as `begin_start` has recorded the session, or the
    /// refusal. The start runs on; nothing here waits for the instance.
    pub(super) fn mobile_start_cloud(
        &mut self,
        start: CloudStartRequest,
        done: impl FnOnce(&mut Shell, Result<Value, Failure>) + 'static,
        cx: &mut Context<Self>,
    ) {
        let CloudStartRequest {
            device,
            command_id,
            text,
            cwd,
            access,
            params,
            plan,
        } = start;
        let prepared = (|| {
            let target = plan.cloud_target().map_err(policy::Code::failure)?;
            // A new session on an instance that is being stopped would be cut off.
            self.refuse_new_session_on(&target)
                .map_err(|_| policy::Code::AutoStopping.failure())?;
            let account = self.mobile_account(Kind::Claude, &cwd, &params, cx)?;
            Ok::<_, Failure>((target, account))
        })();
        let (target, account) = match prepared {
            Ok(prepared) => prepared,
            Err(error) => return done(self, Err(error)),
        };
        let model = start_model(
            &self.workspace.preferences,
            &cwd,
            Some(Kind::Claude),
            string(&params, "model"),
            None,
        );
        let access = access
            .unwrap_or_else(|| remote_default_access(crate::conversation::default_access(cx)));
        let effort = crate::conversation::preferred_effort(Kind::Claude.id(), cx);
        let chosen = model_settings(&params);
        let probe = OptionsProbe::new(cwd.clone(), self.mobile_cloud_service(), cx);
        let used = self.mobile_cloud_instances(&cwd, plan.kind());
        let store = self.store.clone();
        // The values are checked against the options first: a refusal costs no slot.
        let checking = cx.background_executor().spawn({
            let target = plan.target.clone();
            async move { probe.run().check(&target, &used) }
        });
        cx.spawn(async move |this, cx| {
            // The first answer wins: the reply of the start, or `starting` when the start
            // has not recorded the session within `START_REPLY_BOUND`, which is well inside
            // the host's reply wait. The start itself always runs to its end.
            let done = std::rc::Rc::new(std::cell::RefCell::new(Some(done)));
            if let Err(code) = checking.await {
                let _ = this.update(cx, |this, _| answer_once(&done, this, Err(code.failure())));
                return;
            }
            let claimed = this.update(cx, |_, _| cloud_policy::acquire_wake(&device.id));
            let slot = match claimed {
                Ok(Ok(slot)) => slot,
                Ok(Err(error)) => {
                    let _ = this.update(cx, |this, _| answer_once(&done, this, Err(error)));
                    return;
                }
                Err(_) => return,
            };
            let bound = done.clone();
            let waiting = this.clone();
            cx.spawn(async move |cx| {
                cx.background_executor().timer(START_REPLY_BOUND).await;
                let _ = waiting.update(cx, |this, _| {
                    answer_once(
                        &bound,
                        this,
                        Ok(json!({"accepted": true, "starting": true})),
                    )
                });
            })
            .detach();
            let work = cx.background_executor().spawn(async move {
                begin_cloud_session(
                    &store,
                    &cwd,
                    target,
                    &command_id,
                    &text,
                    model,
                    access,
                    effort,
                    chosen,
                    account,
                )
            });
            let result = work.await;
            let _ = this.update(cx, |this, cx| match result {
                Ok((summary, settings)) => {
                    let id = this.install_cloud_start(summary, settings, cx);
                    this.hold_wake_slot(slot, move |shell| shell.cloud_start_over(id), cx);
                    let reply = json!({"sessionId": this.mobile_session_id(id)});
                    answer_once(&done, this, Ok(reply));
                }
                Err(error) => {
                    cloud_policy::release_wake(&slot);
                    // The desktop's own text can hold a path or a command.
                    let text = sanitize_startup_error(&error);
                    // After the `starting` answer nobody is waiting for this failure: the
                    // desktop user sees it on the error line, as for any failed start.
                    if !answer_once(&done, this, Err(text.clone().into())) {
                        this.error =
                            Some(format!("A cloud start from a remote device failed: {text}"));
                        cx.notify();
                    }
                }
            });
        })
        .detach();
    }

    /// Lists the new cloud session, shows it as the app API does and follows
    /// its start. Returns its id.
    fn install_cloud_start(
        &mut self,
        summary: Session,
        settings: Settings,
        cx: &mut Context<Self>,
    ) -> i64 {
        let id = summary.id;
        self.mobile.started.insert(id);
        self.draft_sessions.insert(id);
        self.sidebar_hidden_sessions.remove(&id);
        self.session_worktrees.insert(id, None);
        // A cloud session is listed under its local project, not as a rail
        // project of its own.
        self.mark_cloud_session(id);
        self.register_cloud_start(id);
        self.add_loaded(summary, cx);
        if let Some(view) = self.sessions.get(&id) {
            view.update(cx, |v, cx| v.install_app_launch(settings, true, cx));
        }
        self.track_cloud_start(id, cx);
        self.order.retain(|s| *s != id);
        self.order.insert(0, id);
        self.invalidate_chrome(cx);
        self.persist(cx);
        self.publish_mobile_catalog(cx);
        cx.notify();
        id
    }

    // MARK: moves

    /// The session a move call names, checked against the device's selection.
    /// A Mono is not a session that moves.
    pub(crate) fn mobile_move_session(
        &self,
        id: Option<&str>,
        device: &DeviceContext,
        cx: &App,
    ) -> Result<i64, Failure> {
        let id = id.filter(|id| !id.is_empty()).ok_or("Choose a session")?;
        match self.mobile_target(id, device, cx)? {
            Target::Session(id) => Ok(id),
            Target::Mono(_) => Err(policy::Code::UnsupportedTarget.failure()),
        }
    }

    /// The project the app API's move actions check a session against: its
    /// local twin for a cloud session, else its own folder.
    pub(crate) fn mobile_move_project(&self, id: i64) -> Option<String> {
        match self.cloud_marks.get(&id) {
            Some(mark) => Some(mark.local_project.clone()),
            None => self.session_summaries.get(&id).map(|s| s.cwd.clone()),
        }
    }

    /// The target the move dialog offers first for session `id`, as `(kind, instance)` words
    /// (`"new"` or the instance name): what a move without a `target` uses.
    pub(crate) fn mobile_move_default(&self, id: i64) -> Option<(&'static str, String)> {
        use monocode_core::cloud_sessions::move_session::MoveTarget;
        let choice = self.move_targets(id).into_iter().next()?;
        Some(match choice.target {
            MoveTarget::NewCodespace { .. } => ("codespace", "new".to_owned()),
            MoveTarget::NewSandbox { .. } => ("sandbox", "new".to_owned()),
            MoveTarget::Existing { backend, name } => (backend.api_name(), name),
        })
    }

    /// Whether a move of the session is on (a worker runs or it waits).
    pub(crate) fn mobile_move_active(&self, id: i64) -> bool {
        self.cloud_moves.contains_key(&id)
    }

    /// Whether the desktop still shows the return-to-local confirmation or
    /// runs the return of the session.
    pub(crate) fn mobile_return_active(&self, id: i64) -> bool {
        self.cloud_continue.api_status(id).is_some()
    }

    /// The step of a move to the cloud in progress and its status: one of the
    /// 7 step ids and `pending`, `running`, `failed` or `done`. Nothing else
    /// of the move (paths, branch names, errors) is exposed.
    pub(crate) fn mobile_move_progress(&self, id: i64) -> Option<(&'static str, &'static str)> {
        use crate::mobile::cloud::move_step_id as step_id;
        let run = self.cloud_moves.get(&id)?;
        let Some(record) = &run.record else {
            return Some((step_id(MoveStep::Snapshot), "pending"));
        };
        if let Some(step) = record.failed_at {
            return Some((step_id(step), "failed"));
        }
        if let Some(step) = record.current {
            let status = if run.running { "running" } else { "pending" };
            return Some((step_id(step), status));
        }
        let next = MoveStep::ALL
            .into_iter()
            .find(|step| !record.done.contains(step));
        Some(match next {
            Some(step) => (step_id(step), "pending"),
            None => (step_id(MoveStep::Flip), "done"),
        })
    }
}

/// Records the session on the cloud service (`begin_start`, the call the app
/// API makes) and saves what the session starts with: its title, settings
/// and first prompt, which the start sends when the instance is ready.
/// Blocking: runs on a worker.
#[allow(clippy::too_many_arguments)]
fn begin_cloud_session(
    store: &Database,
    cwd: &str,
    target: monocode_core::mono::cloud_api::CloudTarget,
    command_id: &str,
    prompt: &str,
    model: Option<String>,
    access: AccessMode,
    effort: Option<String>,
    mut chosen: HashMap<String, String>,
    account: monocode_core::accounts::Profile,
) -> Result<(Session, Settings), String> {
    let kind = Kind::Claude;
    let startup = CloudApi::shared(store)?.begin_start(cwd, target, kind.id())?;
    let built = (|| {
        let store = locked(store)?;
        let mut summary = startup.session().clone();
        let mut settings = session_loading::new_session_settings(
            store
                .desktop_preferences()
                .and_then(|preferences| preferences.session_defaults(kind.id(), cwd)),
            model,
        );
        settings.access = access;
        settings.account = account;
        if let Some(effort) = chosen.remove("effort").or(effort) {
            settings.effort = Some(effort);
        }
        if let Some(fast) = chosen.remove("fast") {
            settings.fast = fast == "true";
        }
        if let Some(thinking) = chosen.remove("thinking") {
            settings.thinking = Some(thinking == "true");
        }
        settings.model_settings.extend(chosen);
        settings.revision = 0;
        let title: String = prompt.chars().take(160).collect();
        store.title(summary.id, &title)?;
        summary.title = title;
        store.configure_agent(summary.id, kind.id(), &settings)?;
        store.prepare_app_prompt(summary.id, command_id, prompt)?;
        summary.has_messages = true;
        Ok::<_, String>((summary, settings))
    })();
    // An instance created for this start is kept: a failed sandbox row is the
    // only handle on it, so it stays (failed) and a retry targets it.
    built.map_err(|error| {
        if let Ok(db) = locked(store) {
            crate::cloud_actions::keep_failed_sandbox_start(&db, Some(&startup), &error);
        }
        crate::cloud_actions::failure_note(Some(&startup), error)
    })
}
