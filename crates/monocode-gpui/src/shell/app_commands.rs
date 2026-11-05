//! Operator's project actions. Monos share the launch/request helpers but keep
//! their own project grants, visibility preference and completion bookkeeping.
use super::*;
use monocode_core::{
    agent::Settings,
    app_access as app,
    mono::{
        bridge::{Call, Server},
        cloud_api::{self, CloudApi},
    },
};
use serde_json::{Value, json};

#[cfg(test)]
mod tests;

type Placement = Option<(i64, Edge)>;
enum Effect {
    None,
    Start(Box<(Session, Settings, bool, bool, Placement)>),
    /// The sessions an archive covered, those the store saved, and why it stopped.
    Archive(Vec<i64>, Vec<i64>, Option<String>),
}
impl Shell {
    pub(super) fn init_app_bridge(&mut self, cx: &mut Context<Self>) {
        if cx.is_test() {
            return;
        }
        match Server::start() {
            Ok((server, calls)) => {
                self.app_bridge = Some(server);
                cx.spawn(async move |this, cx| {
                    while let Ok(call) = calls.recv().await {
                        if this
                            .update_in(cx, |this, window, cx| {
                                this.handle_app_call(call, window, cx)
                            })
                            .is_err()
                        {
                            break;
                        }
                    }
                })
                .detach();
            }
            Err(error) => self.error = Some(format!("Operator app access: {error}")),
        }
    }
    fn finish_app_call(
        &mut self,
        call: Call,
        result: Result<Value, String>,
        cx: &mut Context<Self>,
    ) {
        if !mutating(&call.action) {
            self.app_pending
                .remove(&(call.session, call.request_id.clone()));
            let _ = call.reply.send(result);
            return;
        }
        let fingerprint = app::fingerprint(&call.action, &call.input);
        if self.app_results.len() >= 256
            && let Some(key) = self.app_results.keys().next().cloned()
        {
            self.app_results.remove(&key);
        }
        self.app_results.insert(
            (call.session, call.request_id.clone()),
            (fingerprint.clone(), result.clone()),
        );
        let store = self.store.clone();
        let key = app::receipt_key(call.session, &call.request_id);
        let receipt = json!([fingerprint, result]).to_string();
        // A successful reply is sent only after its receipt reaches durable storage.
        let saved = cx
            .background_executor()
            .spawn(async move { locked(&store)?.save_setting(&key, &receipt) });
        cx.spawn(async move |this, cx| {
            let saved = saved.await;
            let _ = this.update(cx, |this, _| {
                this.app_pending
                    .remove(&(call.session, call.request_id.clone()));
            });
            let _ = call.reply.send(saved.and(result));
        })
        .detach();
    }
    fn app_background<F>(&mut self, call: Call, work: F, cx: &mut Context<Self>)
    where
        F: FnOnce(Database) -> Result<(Value, Effect), String> + Send + 'static,
    {
        let store = self.store.clone();
        let work = cx.background_executor().spawn(async move { work(store) });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update_in(cx, |this, window, cx| {
                let result = result.and_then(|(value, effect)| {
                    let mut failure = None;
                    match effect {
                        Effect::None => (),
                        Effect::Start(start) => {
                            let (summary, settings, draft, reveal, placement) = *start;
                            let id = summary.id;
                            this.draft_sessions.insert(id);
                            this.sidebar_hidden_sessions.remove(&id);
                            this.session_worktrees
                                .insert(id, settings.worktree_cwd.clone());
                            this.add(summary, cx);
                            this.started_by.insert(id, call.session);
                            if let Some(view) = this.sessions.get(&id) {
                                view.update(cx, |v, cx| v.install_app_launch(settings, !draft, cx));
                            }
                            this.app_place_launch(id, placement, reveal, window, cx);
                        }
                        Effect::Archive(family, written, error) => {
                            this.finish_archive(
                                &family,
                                &written,
                                error.is_none(),
                                true,
                                window,
                                cx,
                            );
                            failure = error;
                        }
                    }
                    this.invalidate_chrome(cx);
                    cx.notify();
                    failure.map_or(Ok(value), |error| {
                        Err(format!("Could not update archive: {error}"))
                    })
                });
                this.finish_app_call(call, result, cx);
            });
        })
        .detach();
    }
    fn authorize_app_call(&self, call: &Call, cx: &App) -> Result<(), String> {
        app::validate_operator(&call.action, &call.input)?;
        if call.request_id.is_empty()
            || call.request_id.len() > 128
            || !call
                .request_id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        {
            return Err("request ID must use letters, digits, underscores or hyphens".into());
        }
        let caller = self
            .sessions
            .get(&call.session)
            .map(|v| v.read(cx))
            .filter(|v| v.settings.operator_enabled)
            .ok_or("Operator is not enabled for this thread.")?;
        if caller.summary.cwd.starts_with("remote://") {
            return Err("Operator is not available on remote projects".into());
        }
        if call
            .input
            .get("project")
            .and_then(Value::as_str)
            .is_some_and(|p| {
                monocode_core::files::path_key(p)
                    != monocode_core::files::path_key(&caller.summary.cwd)
            })
        {
            return Err("Operator can only access this thread's project.".into());
        }
        if (caller.app_read_only
            || caller.settings.plan
            || caller.settings.access == monocode_core::agent::AccessMode::ReadOnly)
            && !matches!(
                call.action.as_str(),
                "models.list"
                    | "sessions.list"
                    | "sessions.read"
                    | "folders.list"
                    | "notes.list"
                    | "notes.read"
                    | "worktrees.list"
                    | "codespaces.list"
                    | "codespaces.secretsStatus"
            )
        {
            return Err("This thread's access policy allows read-only app actions.".into());
        }
        Ok(())
    }
    fn handle_app_call(&mut self, call: Call, window: &mut Window, cx: &mut Context<Self>) {
        if let Err(error) = self.authorize_app_call(&call, cx) {
            let _ = call.reply.send(Err(error));
            return;
        }
        let key = (call.session, call.request_id.clone());
        let fingerprint = app::fingerprint(&call.action, &call.input);
        if let Some((hash, result)) = self.app_results.get(&key) {
            let _ = call.reply.send(if *hash == fingerprint {
                result.clone()
            } else {
                Err("Request ID was already used with different input".into())
            });
            return;
        }
        if !self.app_pending.insert(key) {
            let _ = call.reply.send(Err(
                "Request is still running; retry with the same request ID.".into(),
            ));
            return;
        }
        if !mutating(&call.action) {
            self.dispatch_app_call(call, window, cx);
            return;
        }
        let receipt_key = app::receipt_key(call.session, &call.request_id);
        let store = self.store.clone();
        let work = cx.background_executor().spawn(async move {
            locked(&store)?
                .setting(&receipt_key)?
                .map(|raw| {
                    serde_json::from_str::<(String, Result<Value, String>)>(&raw)
                        .map_err(|e| e.to_string())
                })
                .transpose()
        });
        cx.spawn(async move |this, cx| {
            let saved = work.await;
            let _ = this.update_in(cx, |this, window, cx| match saved {
                Ok(Some((hash, result))) => {
                    this.app_pending
                        .remove(&(call.session, call.request_id.clone()));
                    let _ = call.reply.send(if hash == fingerprint {
                        result
                    } else {
                        Err("Request ID was already used with different input".into())
                    });
                }
                Ok(None) => this.dispatch_app_call(call, window, cx),
                Err(error) => this.finish_app_call(call, Err(error), cx),
            });
        })
        .detach();
        let _ = window;
    }
    /// What `sessions.archive` covers: the target and the sessions it started
    /// that this app call may manage. A call from inside that group is refused,
    /// since archiving would stop and close the caller mid-call.
    fn app_archive_family(
        &self,
        id: i64,
        caller: i64,
        project: &str,
        cx: &App,
    ) -> Result<Vec<i64>, String> {
        if self.archiving.contains(&id) {
            return Err("Session is already being archived".into());
        }
        let mut up = caller;
        for _ in 0..self.started_by.len() {
            let Some(&by) = self.started_by.get(&up) else {
                break;
            };
            if by == id {
                return Err(
                    "This session was started by the session being archived; archiving would stop it".into(),
                );
            }
            up = by;
        }
        let mut family = self.archive_family(id, true, cx);
        family.retain(|&member| {
            member == id
                || (self.session_summaries.get(&member).is_some_and(|s| {
                    monocode_core::files::path_key(&s.cwd)
                        == monocode_core::files::path_key(project)
                }) && !self.orchestration_worker(member)
                    && !self
                        .monos
                        .as_ref()
                        .is_some_and(|m| m.read(cx).is_mono_session(member)))
        });
        Ok(family)
    }
    fn app_target(
        &self,
        input: &Value,
        caller: i64,
        project: &str,
        manage: bool,
        cx: &App,
    ) -> Result<i64, String> {
        let id = session_id(input, "sessionId", &self.store)?;
        let summary = self
            .session_summaries
            .get(&id)
            .ok_or("Session was not found in this project")?;
        if monocode_core::files::path_key(&summary.cwd) != monocode_core::files::path_key(project) {
            // A cloud session lives on a codespace but belongs to its local project.
            let cloud = summary.cwd.starts_with("remote://")
                && locked(&self.store)
                    .is_ok_and(|store| cloud_api::is_cloud_session_of(&store, id, project));
            if !cloud {
                return Err("Session is outside this project.".into());
            }
        }
        if manage
            && (id == caller
                || self.orchestration_worker(id)
                || self
                    .monos
                    .as_ref()
                    .is_some_and(|m| m.read(cx).is_mono_session(id)))
        {
            return Err("Only other regular project sessions can be managed here".into());
        }
        Ok(id)
    }
    pub(crate) fn app_placement(
        &self,
        input: &Value,
        caller: i64,
        project: &str,
        cx: &App,
    ) -> Result<Placement, String> {
        let edge = match input["placement"].as_str().unwrap_or("tab") {
            "tab" => return Ok(None),
            "right" => Edge::Right,
            "down" => Edge::Bottom,
            _ => return Err("placement must be tab, right or down".into()),
        };
        let id = if input.get("besideSessionId").is_some() {
            session_id(input, "besideSessionId", &self.store)?
        } else {
            caller
        };
        if self.workspace.tab_of(id).is_none()
            || self.session_metadata(id, cx).summary.cwd != project
        {
            return Err("besideSessionId must be an open session pane in this project".into());
        }
        Ok(Some((id, edge)))
    }
    pub(crate) fn app_place_launch(
        &mut self,
        id: i64,
        placement: Placement,
        reveal: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some((target, edge)) =
            placement.filter(|(target, _)| self.workspace.tab_of(*target).is_some())
        {
            self.open(target, window, cx);
            let before = self.active_leaves();
            self.workspace.place(id, target, edge);
            self.begin_pane_motion(before, cx);
            self.fit_sidebar(cx);
            self.activate_session(id, window, cx);
        } else if reveal {
            self.open(id, window, cx);
        }
        self.persist(cx);
    }
    fn dispatch_app_call(&mut self, call: Call, window: &mut Window, cx: &mut Context<Self>) {
        if let Err(error) = self.authorize_app_call(&call, cx) {
            self.finish_app_call(call, Err(error), cx);
            return;
        }
        let caller = self.sessions[&call.session].read(cx);
        let project = caller.summary.cwd.clone();
        let provider = caller.summary.provider.clone();
        let inherited = caller.settings.clone();
        let input = call.input.clone();
        let action = call.action.clone();
        let result: Result<Value, String> = match action.as_str() {
            "sessions.list" => {
                // Rows are built here from live state. Cloud sessions are filtered and given their
                // target off the UI thread: the codespace state may need a `gh` call.
                let rows: Vec<_> = self.order.iter().filter_map(|id|{
                let metadata=self.session_metadata(*id,cx);
                let remote=metadata.summary.cwd.starts_with("remote://");
                if (metadata.summary.cwd!=project && !remote) || self.monos.as_ref().is_some_and(|m|m.read(cx).hidden_session(*id)) || self.orchestration_worker(*id){return None;}
                let live=self.sessions.get(id).map(|v|v.read(cx));
                Some((*id,metadata.summary.cwd==project,json!({"id":id,"sessionId":id,"title":metadata.summary.title,"harness":metadata.summary.provider,"model":live.and_then(|v|v.settings.model.clone()).or_else(||self.session_details.get(id).and_then(|m|m.model.clone())),"busy":metadata.busy,"hasDraft":live.is_some_and(|v|v.has_saved_draft() || !v.input.read(cx).content.is_empty()) || self.draft_sessions.contains(id),"archived":metadata.summary.archived})))
            }).collect();
                self.app_background(
                    call,
                    move |store| {
                        let targets = if rows.iter().any(|(_, local, _)| !local) {
                            CloudApi::shared(&store)
                                .map(|api| api.session_targets(&project))
                                .unwrap_or_default()
                        } else {
                            Default::default()
                        };
                        let sessions: Vec<_> = rows
                            .into_iter()
                            .filter(|(id, local, _)| *local || targets.contains_key(id))
                            .map(|(id, _, mut row)| {
                                cloud_api::annotate(&mut row, targets.get(&id));
                                row
                            })
                            .collect();
                        Ok((json!({"cwd":project,"sessions":sessions}), Effect::None))
                    },
                    cx,
                );
                return;
            }
            "models.list" => {
                let profiles: Vec<_> = Kind::ALL
                    .into_iter()
                    .map(|kind| {
                        let profile = if kind.id() == provider {
                            inherited.account.clone()
                        } else {
                            crate::accounts::get(cx)
                                .map(|center| center.read(cx).selected(kind, &project))
                                .unwrap_or_default()
                        };
                        (kind, profile)
                    })
                    .collect();
                self.app_background(
                    call,
                    move |_| Ok((app::models_catalog(profiles)?, Effect::None)),
                    cx,
                );
                return;
            }
            "sessions.start" => {
                let parsed = (|| {
                    let prompt = app::prompt(&input)?;
                    // A cloud session is Claude's: that is the default when a target is given.
                    let default = if input.get("target").is_some() {
                        "claude"
                    } else {
                        &provider
                    };
                    let kind = Kind::parse(input["harness"].as_str().unwrap_or(default))
                        .ok_or("Unknown harness")?;
                    let placement = self.app_placement(&input, call.session, &project, cx)?;
                    let mut settings = inherited;
                    if kind.id() != provider {
                        settings.account = crate::accounts::get(cx)
                            .map(|center| center.read(cx).selected(kind, &project))
                            .unwrap_or_default();
                    }
                    Ok::<_, String>((prompt, kind, placement, settings))
                })();
                let (prompt, kind, placement, settings) = match parsed {
                    Ok(p) => p,
                    Err(e) => {
                        self.finish_app_call(call, Err(e), cx);
                        return;
                    }
                };
                let caller = call.session;
                let request = call.request_id.clone();
                let fingerprint = app::fingerprint(&action, &input);
                let cloud = cloud_api::target_from_input(&input);
                self.app_background(call,move|store| {
                    // Creating or waking a codespace takes minutes; this runs off the UI thread and
                    // no session row exists until it is ready. A retry with the same request ID
                    // waits for it (the app answers "still running", then replays the result).
                    let models=if input.get("model").is_some() || input.get("modelSettings").is_some() || input.get("effort").is_some() {
                        monocode_core::catalog::models_for(kind,&settings.account)?
                    }else{monocode_core::catalog::cached_models_for(kind,&settings.account).unwrap_or_default()};
                    let mut settings=app::launch_settings(&input,&provider,settings,kind,&models)?;
                    settings.revision=0;
                    let title=app::optional_text(&input,"title",160)?.unwrap_or_else(||prompt.chars().take(160).collect());
                    // Everything cheap is validated above; only now is a codespace created (minutes).
                    let creating = matches!(&cloud, Some(t) if t.codespace == monocode_core::cloud_sessions::CodespaceChoice::New);
                    let binding = match cloud {
                        Some(target) => {
                            let api = CloudApi::shared(&store)?;
                            Some(api.prepare(&project, target, &mut |_| {})?)
                        }
                        None => None,
                    };
                    let built=(||{
                    // Always a chat: the app drives this session, whatever the surface preference.
                    let mut store=locked(&store)?;
                    let cwd=binding.as_ref().map_or(project.as_str(),|b|b.project_key.as_str());
                    let (mut summary,fresh)=store.create_app_session(caller,&request,&fingerprint,kind.id(),cwd)?;
                    if fresh {
                        // A cloud session has no local checkout to prepare.
                        if binding.is_none()
                            && let Err(error)=app::launch_checkout(&input,&project,summary.id,&mut settings) {let _=store.delete(summary.id);return Err(error);}
                        store.title(summary.id,&title)?;summary.title=title;
                        store.configure_agent(summary.id,kind.id(),&settings)?;
                        store.save_started_link(summary.id,caller)?;
                        store.prepare_app_prompt(summary.id,&request,&prompt)?;
                        summary.has_messages=true;
                    }
                    let draft=input["draft"].as_bool().unwrap_or(false);let reveal=input["reveal"].as_bool().unwrap_or(false);
                    let mut value=json!({"id":summary.id,"sessionId":summary.id,"cwd":project,"harness":kind.id(),"model":settings.model,"submitted":!draft,"draft":draft,"accepted":true});
                    if let Some(binding)=&binding {
                        value["target"]=json!({"kind":"codespace","codespace":binding.codespace,"state":"available"});
                        value["codespaceState"]=json!("available");
                    }
                    Ok((value,if fresh {Effect::Start(Box::new((summary,settings,draft,reveal,placement)))}else{Effect::None}))
                    })();
                    // A codespace created for this start is kept: say so, so a retry targets it.
                    built.map_err(|e| match &binding { Some(b) if creating => cloud_api::created_note(&b.codespace,&e), _ => e })
                },cx);
                return;
            }
            "sessions.read" => match self.app_target(&input, call.session, &project, false, cx) {
                Ok(id) => {
                    let busy = self.session_metadata(id, cx).busy;
                    let draft = self
                        .sessions
                        .get(&id)
                        .is_some_and(|v| v.read(cx).has_saved_draft())
                        || self.draft_sessions.contains(&id);
                    let remote = self
                        .session_metadata(id, cx)
                        .summary
                        .cwd
                        .starts_with("remote://");
                    self.app_background(
                        call,
                        move |store| {
                            let mut value =
                                locked(&store)?.app_conversation(id, &input, busy, draft)?;
                            let target = if remote {
                                CloudApi::shared(&store)?.target_of(id)
                            } else {
                                None
                            };
                            cloud_api::annotate(&mut value, target.as_ref());
                            Ok((value, Effect::None))
                        },
                        cx,
                    );
                    return;
                }
                Err(e) => Err(e),
            },
            "sessions.send" | "sessions.draft" | "sessions.stop" | "sessions.archive"
            | "sessions.delete" => {
                match self
                    .app_target(&input, call.session, &project, true, cx)
                    .and_then(|id| {
                        // A terminal session has no conversation to drive.
                        if self.is_terminal(id) {
                            return Err(
                                if matches!(
                                    action.as_str(),
                                    "sessions.send" | "sessions.draft" | "sessions.stop"
                                ) {
                                    "A terminal session takes no prompts.".into()
                                } else {
                                    "A terminal session is managed from its own tab.".into()
                                },
                            );
                        }
                        self.ensure_session(id, cx)
                            .map(|view| (id, view))
                            .ok_or("Session not found".into())
                    }) {
                    Err(e) => Err(e),
                    Ok((id, view)) => {
                        if action == "sessions.delete" {
                            self.delete_session(id, window, cx);
                            if !self.deleting.contains(&id) {
                                self.finish_app_call(
                                    call,
                                    Err(self
                                        .error
                                        .clone()
                                        .unwrap_or("Could not delete session".into())),
                                    cx,
                                );
                                return;
                            }
                            let deleting = self.group_writes.take();
                            let store = self.store.clone();
                            self.group_writes = Some(cx.spawn(async move |this, cx| {
                                if let Some(deleting) = deleting {
                                    deleting.await;
                                }
                                let result = cx
                                    .background_executor()
                                    .spawn(async move {
                                        if locked(&store)?.sessions()?.iter().any(|s| s.id == id) {
                                            Err("Could not delete session; its data was preserved"
                                                .into())
                                        } else {
                                            Ok(json!({"sessionId":id,"deleted":true}))
                                        }
                                    })
                                    .await;
                                let _ = this
                                    .update(cx, |this, cx| this.finish_app_call(call, result, cx));
                            }));
                            return;
                        } else if action == "sessions.archive" {
                            let ids = match self.app_archive_family(id, call.session, &project, cx)
                            {
                                Ok(ids) => ids,
                                Err(e) => {
                                    self.finish_app_call(call, Err(e), cx);
                                    return;
                                }
                            };
                            view.update(cx, |v, cx| v.stop_app_session(cx));
                            self.archiving.extend(ids.iter().copied());
                            self.app_background(
                                call,
                                move |store| {
                                    let (written, error) =
                                        archive_family::write_until_failure(&ids, |id| {
                                            locked(&store)?.set_archived(id, true)
                                        });
                                    Ok((
                                        json!({"sessionId":id,"archived":true}),
                                        Effect::Archive(ids, written, error),
                                    ))
                                },
                                cx,
                            );
                            return;
                        } else if action == "sessions.stop" {
                            view.update(cx, |v, cx| v.stop_app_session(cx));
                            Ok(json!({"sessionId":id,"stopped":true}))
                        } else {
                            view.update(cx, |v, cx| v.load(cx));
                            // A cloud session whose history cannot load is on a codespace that may be
                            // stopped: (re)connect it (a draft never does) and ask for a retry.
                            if action == "sessions.send"
                                && self
                                    .session_metadata(id, cx)
                                    .summary
                                    .cwd
                                    .starts_with("remote://")
                                && view.read(cx).needs_history_restore()
                                && locked(&self.store)
                                    .is_ok_and(|s| cloud_api::is_cloud_session_of(&s, id, &project))
                            {
                                let store = self.store.clone();
                                cx.spawn(async move |this, cx| {
                                    let woke = crate::cloud_wake::wake_for_send(store, id, view, cx).await;
                                    let _ = this.update(cx, |this, cx| {
                                        this.finish_app_call(
                                            call,
                                            Err(if woke {
                                                "The session's codespace is being woken or reconnected. Retry in a minute (with a new request ID)."
                                            } else {
                                                "Session is busy, loading, or has an unsent draft. Its draft was preserved."
                                            }
                                            .into()),
                                            cx,
                                        )
                                    });
                                })
                                .detach();
                                return;
                            }
                            if !view.read(cx).ready_for_external(cx)
                                || view.read(cx).has_saved_draft()
                            {
                                Err("Session is busy, loading, or has an unsent draft. Its draft was preserved.".into())
                            } else {
                                match app::prompt(&input) {
                                    Err(e) => Err(e),
                                    Ok(prompt) => {
                                        view.update(cx, |v, cx| {
                                            if action == "sessions.draft" {
                                                v.save_app_draft(prompt, cx)
                                            } else {
                                                v.install_transfer(
                                                    v.settings.clone(),
                                                    Some(prompt),
                                                    cx,
                                                )
                                            }
                                        });
                                        Ok(
                                            json!({"sessionId":id,"submitted":action=="sessions.send","saved":action=="sessions.draft"}),
                                        )
                                    }
                                }
                            }
                        }
                    }
                }
            }
            "codespaces.list"
            | "codespaces.create"
            | "codespaces.stop"
            | "codespaces.secretsStatus" => {
                // gh calls, and for create minutes of waiting: always off the UI thread.
                self.app_background(
                    call,
                    move |store| {
                        let api = CloudApi::shared(&store)?;
                        let name = || app::text(&input, "name", 100);
                        let value = match action.as_str() {
                            "codespaces.list" => api.list(&project)?,
                            "codespaces.create" => api.create(&project, &input, &mut |_| {})?,
                            "codespaces.stop" => api.stop(&project, &name()?)?,
                            _ => api.secrets_status(&project)?,
                        };
                        Ok((value, Effect::None))
                    },
                    cx,
                );
                return;
            }
            "notes.list" | "notes.read" | "notes.write" => {
                let caller = call.session;
                let request = call.request_id.clone();
                self.app_background(
                    call,
                    move |store| {
                        Ok((
                            app::notes_action(
                                &*locked(&store)?,
                                caller,
                                &request,
                                &action,
                                &input,
                                &[project],
                            )?,
                            Effect::None,
                        ))
                    },
                    cx,
                );
                return;
            }
            "folders.list" => Ok(
                json!({"cwd":project,"folders":self.groups.get(&project).cloned().unwrap_or_default().folders.iter().map(|f|json!({"id":f.id,"name":f.name,"sessionIds":f.session_ids})).collect::<Vec<_>>()}),
            ),
            "folders.move" => match self.app_target(&input, call.session, &project, false, cx) {
                Err(e) => Err(e),
                Ok(id) => {
                    let mut groups = self.groups.get(&project).cloned().unwrap_or_default();
                    let folder = app::optional_text(&input, "folderId", 256);
                    let name = app::optional_text(&input, "newFolderName", 100);
                    match folder.and_then(|f| name.map(|n| (f, n))) {
                        Err(e) => Err(e),
                        Ok((folder, name)) => {
                            if folder.is_some() == name.is_some() {
                                Err("Supply exactly one of folderId or newFolderName".into())
                            } else {
                                let folder = if let Some(name) = name {
                                    let folder = groups
                                        .create_folder(&[id], monocode_core::mono::new_id())
                                        .unwrap();
                                    groups.rename_folder(&folder, &name);
                                    folder
                                } else {
                                    folder.unwrap()
                                };
                                if groups.folder(&folder).is_none() {
                                    Err("Folder not found".into())
                                } else {
                                    groups.add_to_folder(&folder, id);
                                    let name = groups.folder(&folder).unwrap().name.clone();
                                    // Apply now, on this thread, and save through the shared
                                    // queue: a delete finishing while the save runs would
                                    // otherwise be overwritten by this snapshot.
                                    self.groups.insert(project.clone(), groups.clone());
                                    self.invalidate_chrome(cx);
                                    cx.notify();
                                    let saved = self.store_write_reporting(
                                        "save folders",
                                        cx,
                                        move |store| store.save_groups(&project, &groups),
                                    );
                                    let value =
                                        json!({"sessionId":id,"folderId":folder,"folderName":name});
                                    cx.spawn(async move |this, cx| {
                                        let result = saved
                                            .recv()
                                            .await
                                            .unwrap_or_else(|_| {
                                                Err("Could not confirm the folder save".into())
                                            })
                                            .map(|()| value);
                                        let _ = this.update(cx, |this, cx| {
                                            this.finish_app_call(call, result, cx)
                                        });
                                    })
                                    .detach();
                                    return;
                                }
                            }
                        }
                    }
                }
            },
            "worktrees.list" | "worktrees.create" => {
                self.app_background(
                    call,
                    move |_| {
                        let root = std::path::Path::new(&project);
                        let value = if action == "worktrees.list" {
                            let snapshot = monocode_core::git::snapshot(root)?;
                            json!({"worktrees":snapshot.worktrees.iter().map(worktree_json).collect::<Vec<_>>(),"branches":snapshot.branches.iter().map(|b|json!({"name":b.name,"remote":b.remote})).collect::<Vec<_>>()})
                        } else {
                            let branch = app::text(&input, "branch", 400)?;
                            let base = app::optional_text(&input, "base", 400)?;
                            let existing = input["existing"].as_bool().unwrap_or(false);
                            if existing && base.is_some() {
                                return Err("base cannot be set for an existing branch".into());
                            }
                            worktree_json(&monocode_core::git::create_project_worktree(
                                root,
                                &branch,
                                base.as_deref(),
                                existing
                            )?)
                        };
                        Ok((value, Effect::None))
                    },
                    cx,
                );
                return;
            }
            _ => Err("Unknown Operator action".into()),
        };
        self.finish_app_call(call, result, cx);
    }
}
fn mutating(action: &str) -> bool {
    !matches!(
        action,
        "models.list"
            | "sessions.list"
            | "sessions.read"
            | "folders.list"
            | "notes.list"
            | "notes.read"
            | "worktrees.list"
            | "codespaces.list"
            | "codespaces.secretsStatus"
    )
}
fn worktree_json(tree: &monocode_core::git::Worktree) -> Value {
    json!({"path":tree.path,"branch":tree.branch,"head":tree.head,"main":tree.main,"missing":tree.missing})
}
fn session_id(input: &Value, key: &str, store: &Database) -> Result<i64, String> {
    match input.get(key) {
        Some(Value::String(id)) => locked(store)?.resolve_session_id(id),
        Some(value) => value.as_i64().ok_or_else(|| format!("Provide {key}")),
        None => Err(format!("Provide {key}")),
    }
}
