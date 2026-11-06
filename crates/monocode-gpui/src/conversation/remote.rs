//! RemoteSession.tsx's host observation and composer restrictions.
use super::*;
use serde_json::{Value, json};

#[derive(Default)]
pub(super) struct State {
    pub(super) client: Option<Arc<monocode_core::remote::Connections>>,
    clock: Option<gpui::Task<()>>,
    pub(super) refreshing: bool,
    pub(super) pending: bool,
    pub(super) online: bool,
    app_prompt_waiting: bool,
    app_draft_launched: bool,
    waking: bool,
    error: Option<String>,
    rejected: Option<Value>,
    snapshot: Option<Value>,
    pub(super) capabilities: std::collections::HashSet<String>,
}
impl State {
    fn legacy_recovery_error(&self) -> Option<&str> {
        self.rejected.as_ref().filter(|rejected| rejected["legacyRecovery"] == true)
            .map(|rejected| rejected["error"].as_str().unwrap_or("Older pending requests could not be linked to one conversation. Open the original desktop to recover them."))
    }
}
impl Conversation {
    /// Everything that decides whether a remote view can send, for live-debugging tests.
    #[cfg(test)]
    pub(crate) fn remote_debug(&self) -> String {
        format!(
            "status={:?} loaded={} loading={} unloaded={} busy={} online={} refreshing={} pending={} waiting={} waking={} error={:?} actions={} has_messages={}",
            self.status,
            self.loaded,
            self.loading,
            self.history_unloaded,
            self.busy,
            self.remote_state.online,
            self.remote_state.refreshing,
            self.remote_state.pending,
            self.remote_state.app_prompt_waiting,
            self.remote_state.waking,
            self.remote_state.error,
            self.history_actions.len(),
            self.summary.has_messages
        )
    }
    pub(super) fn is_remote(&self) -> bool {
        self.summary.cwd.starts_with("remote://")
    }
    /// A remote session that lives in a codespace: Claude only, so the model
    /// picker offers no other harness.
    pub(super) fn is_cloud_session(&self) -> bool {
        self.is_remote()
            && locked(&self.store)
                .and_then(|store| store.remote_session(self.summary.id))
                .is_ok_and(|session| session.is_some_and(|s| s.project.codespace.is_some()))
    }
    pub(crate) fn remote_connection_error(&mut self, error: &str, cx: &mut Context<Self>) {
        if self.is_remote() && !self.busy {
            self.remote_state.online = false;
            self.remote_state.error = Some(error.into());
            self.status = error.into();
            cx.notify();
        }
    }
    /// Reconnects the codespace of a cloud session off the UI thread, then reloads. Background
    /// loads never open a tunnel, so after an app restart the history stays unloaded until a
    /// user action (send or "Reconnect") comes through here. Returns whether this is a cloud
    /// session (a wake is running or was started).
    pub(super) fn reconnect_cloud(&mut self, cx: &mut Context<Self>) -> bool {
        if !self.is_cloud_session() {
            return false;
        }
        if self.remote_state.waking {
            return true;
        }
        self.remote_state.waking = true;
        self.status = "Reconnecting the codespace...".into();
        cx.notify();
        let store = self.store.clone();
        let id = self.summary.id;
        cx.spawn(async move |this, cx| {
            let result = crate::cloud_wake::reconnect(store, id, cx).await;
            let _ = this.update(cx, |this, cx| {
                this.remote_state.waking = false;
                match result {
                    Ok(true) => {
                        this.remote_state.error = None;
                        this.load_remote(cx);
                    }
                    Ok(false) => this.remote_connection_error(
                        "This codespace is unavailable. It may have been deleted.",
                        cx,
                    ),
                    Err(error) => this.remote_connection_error(&error, cx),
                }
                cx.notify();
            });
        })
        .detach();
        true
    }
    pub(super) fn load_remote(&mut self, cx: &mut Context<Self>) {
        if self.remote_state.clock.is_none() && !cfg!(test) {
            self.remote_state.clock = Some(cx.spawn(async move |this, cx| {
                loop {
                    cx.background_executor()
                        .timer(std::time::Duration::from_secs(3))
                        .await;
                    if this
                        .update(cx, |this, cx| {
                            // Live turns have their own 750 ms sync and control loop.
                            if !this.busy && !this.stopping && !this.history_unloaded {
                                this.refresh_remote_snapshot(cx);
                            }
                        })
                        .is_err()
                    {
                        break;
                    }
                }
            }));
        }
        if !self.loaded || self.history_unloaded {
            self.refresh_remote_snapshot(cx);
        }
    }
    pub(super) fn refresh_remote_snapshot(&mut self, cx: &mut Context<Self>) {
        if self.remote_state.refreshing || self.busy || self.stopping {
            return;
        }
        self.remote_state.refreshing = true;
        let initial = !self.loaded || self.history_unloaded;
        self.loading = initial;
        let store = self.store.clone();
        let id = self.summary.id;
        let known = self.remote_state.snapshot.clone();
        if self.remote_state.client.is_none() {
            self.remote_state.client = monocode_core::remote::default_connections().ok();
        }
        let client = self.remote_state.client.clone();
        let work = cx.background_executor().spawn(async move {
            let (mut remote, mut settings) = {
                let store = locked(&store)?;
                (
                    store
                        .remote_session(id)?
                        .ok_or("Remote session no longer exists")?,
                    store.agent_settings(id)?.unwrap_or_default(),
                )
            };
            let client = client.ok_or("Remote connections are unavailable")?;
            let machine = remote.machine(&client)?;
            let descriptor = if initial {
                Some(client.request(&machine.id, "environment.describe", json!({}))?)
            } else {
                None
            };
            let snapshot = remote
                .session_id
                .as_deref()
                .map(|session| client.load_session(&machine.id, session, known.as_ref()))
                .transpose()?;
            if let Some(snapshot) = snapshot.as_ref() {
                let store = locked(&store)?;
                let summary = store.register_remote_summary(&remote.project, snapshot)?;
                remote = store
                    .remote_session(summary.id)?
                    .ok_or("Remote session identity is missing")?;
                settings = store.agent_settings(summary.id)?.unwrap_or_default();
            }
            let changed = initial
                || known.as_ref().map(|s| &s["revision"])
                    != snapshot.as_ref().map(|s| &s["revision"]);
            let history = if changed {
                let qualified = snapshot.as_ref().map(|snapshot| {
                    let mut snapshot = snapshot.clone();
                    if let Some(id) = remote.session_id.as_deref() {
                        monocode_core::remote::attachments::qualify_snapshot(
                            &mut snapshot,
                            &remote.project.environment_id,
                            id,
                        );
                    }
                    snapshot
                });
                let (history, turns) = qualified
                    .as_ref()
                    .map(monocode_core::store::Store::project_remote_history)
                    .transpose()?
                    .unwrap_or_default();
                Some((
                    crate::transcript::PreparedHistory::new(history.rows, false),
                    turns,
                ))
            } else {
                None
            };
            Ok::<_, String>((remote, settings, snapshot, history, descriptor))
        });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |this, cx| {
                this.remote_state.refreshing = false;
                this.loading = false;
                match result {
                    Ok((remote, settings, snapshot, history, descriptor)) => {
                        this.remote_state.online = true;
                        this.remote_state.error = remote
                            .rejected
                            .as_ref()
                            .and_then(|rejected| rejected["error"].as_str().map(str::to_owned));
                        this.remote_state.rejected = remote.rejected;
                        this.remote_state.pending = !remote.outbox.is_empty()
                            || this.remote_state.legacy_recovery_error().is_some();
                        this.summary = remote.summary;
                        if let Some(descriptor) = descriptor {
                            this.remote_state.capabilities = descriptor["capabilities"]
                                .as_array()
                                .into_iter()
                                .flatten()
                                .filter_map(|v| v.as_str().map(str::to_owned))
                                .collect();
                        }
                        if settings.revision > this.settings.revision
                            || (initial && settings.revision >= this.settings.revision)
                        {
                            this.settings = settings;
                            this.model = this.settings.model.clone();
                        }
                        if let Some((history, turns)) = history {
                            this.settings.turns = turns.clone();
                            this.transcript.update(cx, |view, cx| {
                                view.set_turns(turns);
                                view.install_prepared_history(history, cx);
                            });
                        }
                        this.loaded = true;
                        this.history_unloaded = false;
                        this.status = "Ready".into();
                        let running = snapshot.as_ref().is_some_and(|s| s["status"] == "running");
                        if let Some(snapshot) = &snapshot {
                            this.context = snapshot["session"]
                                .get("context")
                                .cloned()
                                .and_then(|context| serde_json::from_value(context).ok());
                            if let Some(title) = snapshot["session"]["title"].as_str() {
                                this.summary.title = title.into();
                            }
                            this.summary.provider_id = snapshot["session"]["providerSessionId"]
                                .as_str()
                                .map(str::to_owned);
                            this.summary.has_messages = snapshot["session"]["blocks"]
                                .as_array()
                                .is_some_and(|b| !b.is_empty());
                            this.install_remote_draft(snapshot, cx);
                            this.install_remote_plans(snapshot, cx);
                        }
                        this.remote_state.snapshot = snapshot;
                        if initial {
                            this.refresh_catalog(cx);
                            this.refresh_git(cx);
                        }
                        if let Some((row, query)) = this.pending_search.take() {
                            this.reveal_search(row, query, cx);
                        }
                        if running && !this.busy && !this.stopping {
                            this.settings.remote_action = Some(json!({"type":"attach"}));
                            this.launch_prompt(String::new(), cx);
                        } else {
                            this.run_history_actions(cx);
                        }
                        this.finish_app_prompt(cx);
                        if initial {
                            let known = this.remote_state.snapshot.clone();
                            this.recover_unsent_app_prompt(known.as_ref(), cx);
                        }
                    }
                    Err(error) => this.remote_connection_error(&error, cx),
                }
                cx.emit(SessionChanged);
                cx.notify();
            });
        })
        .detach();
    }
    fn install_remote_draft(&mut self, snapshot: &Value, cx: &mut Context<Self>) {
        let draft = snapshot["session"]["blocks"]
            .as_array()
            .and_then(|blocks| blocks.iter().find(|b| b["draft"] == true))
            .map(|block| monocode_core::follow_up::Message {
                id: block["id"].as_str().unwrap_or("").into(),
                text: block["text"].as_str().unwrap_or("").into(),
                at: self.transcript.read(cx).source_rows().len(),
                ..Default::default()
            });
        self.follow_ups.state.draft = draft.clone();
        self.transcript
            .update(cx, |view, cx| view.set_saved_draft(draft, cx));
    }
    pub(super) fn remote_send_guard(&mut self, cx: &mut Context<Self>) -> bool {
        let error = if !self.loaded {
            Some("Wait for this remote conversation to load.".to_owned())
        } else if !self.remote_state.online {
            Some(
                "The machine is offline. Reconnect in Settings → Connections, then Retry."
                    .to_owned(),
            )
        } else if self.remote_state.pending {
            Some(
                self.remote_state
                    .legacy_recovery_error()
                    .unwrap_or(
                        "A remote command needs confirmation. Choose Retry before sending another message.",
                    )
                    .to_owned(),
            )
        } else if self.busy || self.stopping {
            Some("Wait for this remote turn to finish before sending another message.".to_owned())
        } else {
            None
        };
        if let Some(error) = error {
            let unreachable = !self.loaded || !self.remote_state.online;
            if unreachable && self.reconnect_cloud(cx) {
                self.status = "Reconnecting the codespace. Send again when it is ready.".into();
                cx.notify();
                return true;
            }
            self.status = error;
            cx.notify();
            return true;
        }
        if self.settings.new_worktree && self.settings.worktree_cwd.is_none() {
            if !self.preparing_worktree {
                self.prepare_worktree(cx);
            }
            return true;
        }
        if !self.attachments.is_empty()
            && !self
                .remote_state
                .capabilities
                .contains("attachments.upload")
        {
            self.status =
                "Update Monoptah Host in Connections settings to send attachments.".into();
            cx.notify();
            return true;
        }
        if self.settings.plan && !self.remote_state.capabilities.contains("sessions.plan") {
            self.status = "Update Monoptah Host in Connections settings to use Plan mode.".into();
            cx.notify();
            return true;
        }
        if self.draft_active(cx) && !self.remote_state.capabilities.contains("sessions.draft") {
            self.status = "Update Monoptah Host in Connections settings to save drafts.".into();
            cx.notify();
            return true;
        }
        let text = self.input.read(cx).content.trim();
        if text == "/compact" {
            self.input
                .update(cx, |input, cx| input.set_text(String::new(), cx));
            self.launch_prompt("/compact".into(), cx);
            return true;
        }
        if text.starts_with('/')
            && !["/plan", "/draft"]
                .iter()
                .any(|command| text == *command || text.starts_with(&format!("{command} ")))
        {
            self.status =
                "Only /plan, /compact and saved drafts are supported by this remote host.".into();
            cx.notify();
            return true;
        }
        if self.settings.operator || self.settings.orchestrator {
            self.status = "Operator and orchestration are not available on remote projects.".into();
            cx.notify();
            return true;
        }
        false
    }
    pub(super) fn remote_draft_action(&mut self, send: bool, cx: &mut Context<Self>) {
        if self.busy || self.stopping || self.remote_state.pending || !self.remote_state.online {
            return;
        }
        let Some(draft) = self.follow_ups.state.draft.as_ref() else {
            return;
        };
        let prompt = if send {
            draft.text.clone()
        } else {
            String::new()
        };
        self.settings.remote_action = Some(if send {
            json!({"type":"send","draftBlockId":draft.id,"intent":"default"})
        } else {
            json!({"type":"removeDraft","draftBlockId":draft.id})
        });
        self.launch_prompt(prompt, cx);
    }
    /// The first prompt of a session started through the app API (a cloud session). It is saved
    /// as a local draft, but a remote conversation never reads local follow-ups and the host
    /// knows no such draft block, so it goes out as an ordinary prompt. When the machine is not
    /// ready the draft is kept and the send is retried after the next successful load; a cloud
    /// session also reconnects its codespace, as any user-initiated send does.
    pub(super) fn send_remote_app_prompt(&mut self, cx: &mut Context<Self>) {
        if self.busy || self.stopping || self.remote_state.pending || !self.remote_state.online {
            self.remote_state.app_prompt_waiting = true;
            if self.reconnect_cloud(cx) {
                self.status = "Reconnecting the codespace to send the first prompt…".into();
            } else {
                self.status = "The first prompt will be sent when the machine is ready.".into();
            }
            cx.notify();
            return;
        }
        self.remote_state.app_prompt_waiting = false;
        match self.stored_app_draft() {
            Ok(Some(draft)) => {
                self.settings.remote_action = None;
                self.launch_prompt(draft.text, cx);
                // The local copy is dropped once the turn has run (see `finish_app_prompt`).
                self.remote_state.app_draft_launched = true;
            }
            Ok(None) => (),
            Err(error) => {
                self.status = error;
                cx.notify();
            }
        }
    }
    /// An API draft (`draft: true`) on a remote session: the remote composer never shows local
    /// follow-ups, so the text goes into the composer where the user can see and send it.
    pub(super) fn show_remote_app_draft(&mut self, cx: &mut Context<Self>) {
        match self.stored_app_draft() {
            Ok(Some(draft)) => {
                self.input
                    .update(cx, |input, cx| input.set_text(draft.text, cx));
                self.drop_stored_app_draft();
                cx.notify();
            }
            Ok(None) => (),
            Err(error) => {
                self.status = error;
                cx.notify();
            }
        }
    }
    fn stored_app_draft(&self) -> Result<Option<monocode_core::follow_up::Message>, String> {
        locked(&self.store).and_then(|store| Ok(store.follow_ups(self.summary.id)?.draft))
    }
    fn drop_stored_app_draft(&self) {
        let _ = locked(&self.store).and_then(|store| {
            let mut state = store.follow_ups(self.summary.id)?;
            state.draft = None;
            state.revision += 1;
            store.save_follow_ups(self.summary.id, &state)
        });
    }
    /// After the first load of a remote conversation: an API launch whose prompt never went out
    /// (the app quit while offline or waking) is still in the local draft, which a remote view
    /// never shows. Put it in the composer with a notice; it is not sent on its own after a
    /// restart. A prompt the host already has is just dropped.
    fn recover_unsent_app_prompt(&mut self, snapshot: Option<&Value>, cx: &mut Context<Self>) {
        if self.remote_state.app_prompt_waiting
            || self.remote_state.app_draft_launched
            || !self.history_actions.is_empty()
        {
            return;
        }
        let Ok(Some(draft)) = self.stored_app_draft() else {
            return;
        };
        if !draft.id.starts_with("app-prompt-") {
            return;
        }
        let sent = snapshot.is_some_and(|snapshot| {
            snapshot["session"]["blocks"]
                .as_array()
                .is_some_and(|blocks| {
                    blocks
                        .iter()
                        .any(|b| b["role"] == "user" && b["text"] == draft.text.as_str())
                })
        });
        if sent {
            self.drop_stored_app_draft();
        } else if self.input.read(cx).content.is_empty() {
            self.input
                .update(cx, |input, cx| input.set_text(draft.text, cx));
            self.drop_stored_app_draft();
            self.status = "This prompt was not sent yet".into();
        }
    }
    /// After a refresh (which only runs between turns): drops the local copy of a launched first
    /// prompt once its turn has run and the command is journaled, and retries one that was
    /// waiting for the machine.
    fn finish_app_prompt(&mut self, cx: &mut Context<Self>) {
        if self.remote_state.app_draft_launched && !self.busy {
            self.remote_state.app_draft_launched = false;
            self.drop_stored_app_draft();
        }
        if self.remote_state.app_prompt_waiting
            && !self.busy
            && !self.stopping
            && !self.remote_state.pending
            && self.remote_state.online
        {
            self.send_remote_app_prompt(cx);
        }
    }
    pub(super) fn remote_save_draft(&mut self, cx: &mut Context<Self>) {
        if self.remote_send_guard(cx) {
            return;
        }
        let raw = self.input.read(cx).content.to_string();
        let prompt = monocode_core::composer::consume_command(&raw, "draft")
            .unwrap_or(&raw)
            .to_owned();
        if prompt.trim().is_empty() && self.attachments.is_empty() {
            return;
        }
        self.input
            .update(cx, |input, cx| input.set_text(String::new(), cx));
        self.follow_ups.draft_selected = false;
        self.settings.remote_action = Some(json!({"type":"draft"}));
        self.launch_prompt(prompt, cx);
    }
    pub(super) fn remote_notice(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.is_remote()
            || self.busy
            || (self.remote_state.error.is_none() && !self.remote_state.pending)
        {
            return None;
        }
        let legacy = self.remote_state.legacy_recovery_error();
        Some(div().mx_2().my_2().p_3().rounded_lg().bg(content(0.04)).flex().items_center().gap_2()
            .child(div().flex_1().text_size(px(12.)).child(if let Some(error) = legacy {error.to_owned()} else if self.remote_state.pending {"A remote command needs confirmation. Retry uses its original command ID.".to_owned()} else {self.remote_state.error.clone().unwrap_or_default()}))
            .when(legacy.is_none() && !self.remote_state.pending && self.is_cloud_session(), |d| d
            .child(crate::settings::widgets::secondary_button("remote-reconnect","Reconnect",!self.remote_state.waking,false).debug_selector(|| "remote-reconnect".into()).child("Reconnect").on_click(cx.listener(|this,_,_,cx| { this.reconnect_cloud(cx); }))))
            .when(legacy.is_none(), |d| d
            .child(crate::settings::widgets::secondary_button("remote-retry",if self.remote_state.rejected.is_some() {"Restore message"} else {"Retry"},true,false).debug_selector(|| "remote-retry".into()).child(if self.remote_state.rejected.is_some() {"Restore message"} else {"Retry"}).on_click(cx.listener(|this,_,_,cx| {
                if this.remote_state.rejected.is_some() { this.restore_remote_message(cx); } else {this.retry_remote(cx);}
            })))).into_any_element())
    }
    fn retry_remote(&mut self, cx: &mut Context<Self>) {
        if let Some(error) = self.remote_state.legacy_recovery_error() {
            self.status = error.to_owned();
            cx.notify();
            return;
        }
        if self.busy || self.stopping {
            return;
        }
        if self.remote_state.pending {
            self.settings.remote_action = Some(json!({"type": "retry"}));
            self.launch_prompt(String::new(), cx);
        } else {
            self.refresh_remote_snapshot(cx);
        }
    }
    pub(super) fn release_remote_snapshot(&mut self) {
        self.remote_state.snapshot = None;
    }
    fn restore_remote_message(&mut self, cx: &mut Context<Self>) {
        if let Some(error) = self.remote_state.legacy_recovery_error() {
            self.status = error.to_owned();
            cx.notify();
            return;
        }
        if !self.input.read(cx).content.trim().is_empty() || !self.attachments.is_empty() {
            self.status = "Clear the composer before restoring the rejected message.".into();
            cx.notify();
            return;
        }
        if let Some(rejected) = self.remote_state.rejected.as_ref() {
            let message = rejected["commands"]
                .as_array()
                .into_iter()
                .flatten()
                .rev()
                .find(|command| {
                    matches!(command["type"].as_str(), Some("send" | "draft" | "compact"))
                });
            if let Some(message) = message {
                let text = if message["type"] == "compact" {
                    "/compact"
                } else {
                    message["text"].as_str().unwrap_or("")
                };
                self.input
                    .update(cx, |input, cx| input.set_text(text.to_owned(), cx));
                self.attachments = rejected["uploads"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|upload| upload["path"].as_str().map(std::path::PathBuf::from))
                    .collect();
            }
        }
        cx.notify();
    }
}

#[cfg(test)]
#[path = "remote_tests.rs"]
mod tests;
