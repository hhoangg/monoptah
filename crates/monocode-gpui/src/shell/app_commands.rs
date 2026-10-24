//! Operator uses the existing authenticated, per-session local app bridge.
use super::*;
use monocode_core::{
    agent::Settings,
    mono::bridge::{Call, Server},
};
use serde_json::{Value, json};

enum Effect {
    None,
    Start(Box<(Session, Settings, String, bool, bool)>),
    Groups(String, monocode_core::session_groups::Groups),
}
impl Shell {
    pub(super) fn init_app_bridge(&mut self, cx: &mut Context<Self>) {
        // GPUI's deterministic executor cannot be woken by an OS socket thread.
        // Command tests exercise the same dispatcher directly without a listener.
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
    fn finish_app_call(&mut self, call: Call, result: Result<Value, String>) {
        let key = (call.session, call.request_id.clone());
        self.app_pending.remove(&key);
        if self.app_results.len() >= 256
            && let Some(key) = self.app_results.keys().next().cloned()
        {
            self.app_results.remove(&key);
        }
        self.app_results.insert(key, result.clone());
        let _ = call.reply.send(result);
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
                let caller = call.session;
                let result = result.map(|(result, effect)| {
                    match effect {
                        Effect::None => (),
                        Effect::Start(start) => {
                            let (summary, settings, prompt, draft, reveal) = *start;
                            let id = summary.id;
                            this.add(summary, cx);
                            // The sidebar hangs it under the thread that started it.
                            this.started_by.insert(id, caller);
                            let store = this.store.clone();
                            cx.background_executor()
                                .spawn(async move {
                                    let _ = locked(&store)
                                        .and_then(|s| s.save_started_link(id, caller));
                                })
                                .detach();
                            if let Some(view) = this.sessions.get(&id) {
                                view.update(cx, |v, cx| {
                                    v.install_transfer(
                                        settings,
                                        (!draft).then(|| prompt.clone()),
                                        cx,
                                    );
                                    if draft {
                                        v.input.update(cx, |i, cx| i.set_text(prompt, cx));
                                    }
                                });
                            }
                            if reveal {
                                this.open(id, window, cx);
                            }
                            this.invalidate_chrome(cx);
                        }
                        Effect::Groups(project, groups) => {
                            this.groups.insert(project, groups);
                            this.invalidate_chrome(cx);
                        }
                    }
                    cx.notify();
                    result
                });
                this.finish_app_call(call, result);
            });
        })
        .detach();
    }
    fn handle_app_call(&mut self, call: Call, window: &mut Window, cx: &mut Context<Self>) {
        let key = (call.session, call.request_id.clone());
        let caller = self.sessions.get(&call.session).map(|v| v.read(cx));
        let Some(caller) = caller.filter(|v| v.settings.operator) else {
            let _ = call
                .reply
                .send(Err("Operator is not enabled for this thread.".into()));
            return;
        };
        let project = caller.summary.cwd.clone();
        if (caller.settings.plan
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
            )
        {
            let _ = call.reply.send(Err(
                "This thread's access policy allows read-only app actions.".into(),
            ));
            return;
        }
        if call
            .input
            .get("project")
            .and_then(Value::as_str)
            .is_some_and(|p| {
                monocode_core::files::path_key(p) != monocode_core::files::path_key(&project)
            })
        {
            let _ = call
                .reply
                .send(Err("Operator can only access this thread's project.".into()));
            return;
        }
        let mut inherited = caller.settings.clone();
        inherited.operator = false;
        inherited.orchestrator = false;
        inherited.environment.clear();
        inherited.attachments.clear();
        inherited.handoff = None;
        inherited.turns.clear();
        let provider = caller.summary.provider.clone();
        if let Some(result) = self.app_results.get(&key) {
            let _ = call.reply.send(result.clone());
            return;
        }
        if !self.app_pending.insert(key) {
            let _ = call.reply.send(Err(
                "Request is still running; retry with the same request ID.".into(),
            ));
            return;
        }
        let input = call.input.clone();
        let action = call.action.clone();
        let result = match action.as_str() {
            "sessions.list" => Ok(json!(self.order.iter().map(|&id| self.session_metadata(id, cx).summary).filter(|s| s.cwd == project).map(|s| json!({"sessionId":s.id,"title":s.title,"harness":s.provider,"project":s.cwd,"archived":s.archived})).collect::<Vec<_>>())),
            "models.list" => Ok(json!(Kind::ALL.into_iter().map(|kind| json!({"harness":kind.id(),"name":kind.label(),"models":self.sessions.values().find(|v| { let v=v.read(cx); let profile=if kind.id()==provider { inherited.account.clone() } else { crate::accounts::get(cx).map(|center|center.read(cx).selected(kind,&project)).unwrap_or_default() }; v.summary.provider == kind.id() && v.settings.account.id==profile.id }).map(|v| v.read(cx).settings_models()).unwrap_or_default()})).collect::<Vec<_>>())),
            "folders.list" => Ok(json!(self.groups.get(&project).cloned().unwrap_or_default().folders)),
            "sessions.send" | "sessions.draft" | "sessions.stop" => {
                let id = session_id(&input, &self.store);
                match id.and_then(|id| {
                    if !self.session_summaries.contains_key(&id) || self.session_metadata(id, cx).summary.cwd != project { return Err("Session is outside this project.".into()); }
                    if self.is_terminal(id) { return Err("A terminal session takes no prompts.".into()); }
                    self.ensure_session(id, cx).ok_or("Session is outside this project.".into())
                }) {
                    Ok(view) => {
                        let ready = if action == "sessions.stop" { true } else {
                            view.update(cx, |v, cx| v.load(cx));
                            view.read(cx).ready_for_external(cx)
                        };
                        if action == "sessions.stop" { view.update(cx, |v, cx| v.stop_if_busy(cx)); Ok(json!({"stopping":true})) }
                        else if !ready { Err("Session is busy, loading, or has an unsent draft. Its draft was preserved.".into()) }
                        else { match text(&input, "prompt", 64 * 1024) { Ok(prompt) => { view.update(cx, |v, cx| {
                            if action == "sessions.draft" { v.input.update(cx, |i, cx| i.set_text(prompt, cx)); }
                            else { v.install_transfer(v.settings.clone(), Some(prompt), cx); }
                        }); Ok(json!({"accepted":true})) }, Err(error) => Err(error) } }
                    }
                    Err(error) => Err(error),
                }
            }
            "sessions.start" => {
                let prompt = text(&input, "prompt", 64 * 1024);
                let kind = input.get("harness").and_then(Value::as_str).unwrap_or(&provider).to_owned();
                if Kind::parse(&kind).is_none() { self.finish_app_call(call, Err("Unknown harness.".into())); return; }
                let prompt = match prompt { Ok(p) => p, Err(e) => { self.finish_app_call(call, Err(e)); return; } };
                inherited.usage_limit = None;
                if kind != provider {
                    inherited.account = crate::accounts::get(cx).map(|center| center.read(cx).selected(Kind::from_id(&kind), &project)).unwrap_or_default();
                    inherited.model = None; inherited.effort = None; inherited.model_settings.clear();
                    inherited.fast = false; inherited.service_tier = None; inherited.thinking = None;
                }
                if let Some(model) = input.get("model").and_then(Value::as_str) { inherited.model = Some(model.into()); }
                let title = input.get("title").and_then(Value::as_str).map(|s| s.chars().take(160).collect::<String>());
                let draft = input.get("draft").and_then(Value::as_bool).unwrap_or(false);
                let reveal = input.get("reveal").and_then(Value::as_bool).unwrap_or(true);
                self.app_background(call, move |store| {
                    // Always a chat: the app drives this session, whatever the surface preference.
                    let store = locked(&store)?; let mut summary = store.create(&kind, &project)?;
                    if let Some(title) = title { store.title(summary.id, &title)?; summary.title = title; }
                    store.save_agent_settings(summary.id, &inherited)?;
                    Ok((json!({"sessionId":summary.id,"accepted":true}), Effect::Start(Box::new((summary, inherited, prompt, draft, reveal)))))
                }, cx); return;
            }
            "sessions.read" => {
                let id = match session_id(&input, &self.store) { Ok(id) => id, Err(e) => { self.finish_app_call(call, Err(e)); return; } };
                if !self.session_summaries.contains_key(&id) || self.session_metadata(id, cx).summary.cwd != project { self.finish_app_call(call, Err("Session is outside this project.".into())); return; }
                let limit = input.get("limit").and_then(Value::as_u64).unwrap_or(40).min(200) as usize;
                self.app_background(call, move |store| {
                    let transcript = locked(&store)?.load(id)?;
                    let rows: Vec<_> = transcript.rows.iter().rev().take(limit).rev().map(|r| json!({"role":r.role,"text":monocode_core::response::limit(&r.text, 16_000)})).collect();
                    Ok((json!({"sessionId":id,"messages":rows}), Effect::None))
                }, cx); return;
            }
            "notes.list" | "notes.read" | "notes.write" => {
                let caller = call.session;
                self.app_background(call, move |store| {
                    let store = locked(&store)?;
                    let notes = store.notes()?;
                    let result = match action.as_str() {
                        "notes.list" => json!(notes.iter().filter(|n| n.source_cwd == project).map(|n| json!({"id":n.id,"title":n.title})).collect::<Vec<_>>()),
                        "notes.read" => { let id = number(&input, "id")?; json!(notes.iter().find(|n| n.id == id && n.source_cwd == project).ok_or("Note not found in this project.")?) },
                        _ => {
                            let id = input.get("id").and_then(Value::as_u64).unwrap_or_else(monocode_core::time::now_ms);
                            if notes.iter().any(|n| n.id == id && n.source_cwd != project) { return Err("Note is outside this project.".into()); }
                            let note = monocode_core::response::Note { id, title: text(&input,"title",512)?, body: text(&input,"body",256*1024)?, tags:input.get("tags").and_then(Value::as_array).map(|tags|tags.iter().filter_map(Value::as_str).map(str::to_owned).collect()).or_else(||notes.iter().find(|n|n.id==id).map(|n|n.tags.clone())).unwrap_or_default(),source_session: caller, source_cwd: project.clone() };
                            store.save_note(&note)?; json!(note)
                        }
                    };
                    Ok((result, Effect::None))
                }, cx); return;
            }
            "folders.move" => {
                let id = match session_id(&input, &self.store) { Ok(id) => id, Err(e) => { self.finish_app_call(call, Err(e)); return; } };
                if !self.session_summaries.contains_key(&id) || self.session_metadata(id, cx).summary.cwd != project { self.finish_app_call(call, Err("Session is outside this project.".into())); return; }
                let mut groups = self.groups.get(&project).cloned().unwrap_or_default();
                if let Some(name) = input.get("newFolderName").and_then(Value::as_str) {
                    if let Some(folder) = groups.create_folder(&[id], monocode_core::mono::new_id()) { groups.rename_folder(&folder, name); }
                } else if let Some(folder) = input.get("folderId").and_then(Value::as_str) {
                    if groups.folder(folder).is_none() { self.finish_app_call(call, Err("Folder not found.".into())); return; }
                    groups.add_to_folder(folder,id);
                } else { groups.remove_sessions(&[id]); }
                self.app_background(call, move |store| { locked(&store)?.save_groups(&project, &groups)?; Ok((json!({"moved":true}), Effect::Groups(project, groups))) }, cx); return;
            }
            "worktrees.list" => {
                self.app_background(call, move |_| { let snapshot = monocode_core::git::snapshot(std::path::Path::new(&project))?; Ok((json!(snapshot.worktrees.iter().map(|w| json!({"path":w.path,"branch":w.branch,"head":w.head,"main":w.main,"missing":w.missing})).collect::<Vec<_>>()), Effect::None)) }, cx); return;
            }
            _ => Err("Unknown Operator action. Supported: models.list, sessions.list/read/start/send/draft/stop, folders.list/move, notes.list/read/write, worktrees.list.".into()),
        };
        self.finish_app_call(call, result);
        let _ = window;
    }
}
fn text(input: &Value, key: &str, max: usize) -> Result<String, String> {
    input
        .get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty() && s.len() <= max)
        .map(str::to_owned)
        .ok_or_else(|| format!("Provide {key} (up to {max} bytes)."))
}
fn number(input: &Value, key: &str) -> Result<u64, String> {
    input
        .get(key)
        .and_then(|v| v.as_u64().or_else(|| v.as_str()?.parse().ok()))
        .ok_or_else(|| format!("Provide {key}."))
}
fn session_id(input: &Value, store: &Database) -> Result<i64, String> {
    if let Some(id) = input.get("sessionId").and_then(Value::as_str) {
        return locked(store)?.resolve_session_id(id);
    }
    i64::try_from(number(input, "sessionId")?).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::TestAppContext;
    #[gpui::test]
    fn operator_scopes_sessions_and_preserves_an_unsent_draft(cx: &mut TestAppContext) {
        let (window, other, active) = crate::shell::tests::window(cx);
        window
            .update(cx, |shell, window, cx| {
                let view = shell.sessions[&active].clone();
                view.update(cx, |v, cx| {
                    v.install_transfer(Settings::default(), None, cx);
                    v.enable_mode("operator", cx);
                    v.input
                        .update(cx, |i, cx| i.set_text("keep my draft".into(), cx));
                });
                for (action, input, should_succeed) in [
                    ("sessions.list", json!({}), true),
                    ("sessions.read", json!({"sessionId":other}), false),
                    (
                        "sessions.draft",
                        json!({"sessionId":active,"prompt":"replace draft"}),
                        false,
                    ),
                ] {
                    let (reply, result) = std::sync::mpsc::channel();
                    shell.handle_app_call(
                        Call {
                            session: active,
                            request_id: action.into(),
                            action: action.into(),
                            input,
                            reply,
                        },
                        window,
                        cx,
                    );
                    let result = result.try_recv().unwrap();
                    assert_eq!(result.is_ok(), should_succeed);
                    if action == "sessions.list" {
                        let list = result.unwrap();
                        assert!(
                            list.as_array()
                                .unwrap()
                                .iter()
                                .any(|s| s["sessionId"] == active)
                        );
                        assert!(
                            !list
                                .as_array()
                                .unwrap()
                                .iter()
                                .any(|s| s["sessionId"] == other)
                        );
                    }
                }
                assert_eq!(view.read(cx).input.read(cx).content, "keep my draft");
                view.update(cx, |v, _| {
                    v.settings.access = monocode_core::agent::AccessMode::ReadOnly
                });
                let (reply, result) = std::sync::mpsc::channel();
                shell.handle_app_call(
                    Call {
                        session: active,
                        request_id: "read-only-write".into(),
                        action: "notes.write".into(),
                        input: json!({"title":"Should not save","body":"Read-only"}),
                        reply,
                    },
                    window,
                    cx,
                );
                assert!(
                    result
                        .try_recv()
                        .unwrap()
                        .unwrap_err()
                        .contains("read-only")
                );
            })
            .unwrap();
    }

    #[gpui::test]
    fn a_session_an_agent_starts_is_a_chat_whatever_the_preference(cx: &mut TestAppContext) {
        use crate::shell::surface_tests::{all_terminal, assert_chat, known, since};
        let (window, _, active) = crate::shell::tests::window(cx);
        let (result, before) = window
            .update(cx, |shell, window, cx| {
                all_terminal(shell);
                shell.sessions[&active].update(cx, |v, cx| {
                    v.install_transfer(Settings::default(), None, cx);
                    v.enable_mode("operator", cx);
                });
                let before = known(shell);
                let (reply, result) = std::sync::mpsc::channel();
                shell.handle_app_call(
                    Call {
                        session: active,
                        request_id: "start".into(),
                        action: "sessions.start".into(),
                        input: json!({"prompt":"Review the project","harness":"claude","draft":true}),
                        reply,
                    },
                    window,
                    cx,
                );
                (result, before)
            })
            .unwrap();
        cx.run_until_parked();
        let started = result.try_recv().unwrap().unwrap();
        let id = started["sessionId"].as_i64().unwrap();
        window
            .update(cx, |shell, _, cx| {
                assert_eq!(since(shell, &before), vec![id]);
                assert_chat(shell, id);
                assert_eq!(
                    shell.sessions[&id].read(cx).input.read(cx).content,
                    "Review the project"
                );
            })
            .unwrap();
    }
}
