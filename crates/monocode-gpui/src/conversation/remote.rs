//! RemoteSession.tsx's host observation and composer restrictions.
use super::*;
use serde_json::{Value, json};

#[derive(Default)]
pub(super) struct State {
    pub(super) client: Option<Arc<monocode_core::remote::Connections>>,
    clock: Option<gpui::Task<()>>,
    pub(super) refreshing: bool,
    /// Bumped when a reconnect starts and when it succeeds: a refresh that began under an older
    /// value belongs to a host that may have been stopped, so its answer is ignored.
    epoch: u64,
    /// The epoch the refresh that holds `refreshing` began under.
    refresh_epoch: u64,
    /// Test seam: stands in for the host round trip of a refresh.
    #[cfg(test)]
    pub(super) refresh_stub: Option<Result<(), String>>,
    pub(super) pending: bool,
    pub(super) online: bool,
    /// The link was live at least once, so a later `!online` is a known drop and not a refresh
    /// that has not finished yet.
    was_online: bool,
    app_prompt_waiting: bool,
    app_draft_launched: bool,
    waking: bool,
    /// Stops the reconnect that is running (set by the Cancel button).
    wake_cancel: Option<Arc<std::sync::atomic::AtomicBool>>,
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
/// Emitted when a user-initiated reconnect (Resume or Reconnect) succeeded, so the shell
/// refreshes the cached cloud state.
pub(crate) struct CloudResumed;
impl EventEmitter<CloudResumed> for Conversation {}

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
    /// The snapshot of this session's codespace start, while one is shown (running, failed or
    /// just ready). `None` for every other session.
    #[cfg(test)]
    pub(crate) fn cloud_start(&self) -> Option<&Arc<crate::cloud_start_view::CloudStartSnapshot>> {
        self.cloud_start.as_ref()
    }
    /// The message typed while the codespace started, not sent yet.
    #[cfg(test)]
    pub(crate) fn cloud_queued(&self) -> Option<&str> {
        self.cloud_queued.as_deref()
    }
    /// `codespace` or `sandbox`, by the backend of the start shown.
    pub(super) fn cloud_start_noun(&self) -> &'static str {
        self.cloud_start.as_ref().map_or("codespace", |s| s.noun())
    }
    /// The codespace is not ready: the host is not loaded and sends are queued.
    pub(crate) fn cloud_start_blocks(&self) -> bool {
        self.cloud_start
            .as_ref()
            .is_some_and(|s| s.blocks_session())
    }
    pub(crate) fn cloud_launch_pending(&self) -> bool {
        self.cloud_start.is_some()
            && (self.cloud_start_blocks()
                || self.remote_state.app_prompt_waiting
                || self.remote_state.app_draft_launched)
    }
    /// Publishes the shell's view of the start. `None` forgets it.
    pub(crate) fn set_cloud_start(
        &mut self,
        start: Option<Arc<crate::cloud_start_view::CloudStartSnapshot>>,
        cx: &mut Context<Self>,
    ) {
        if self.cloud_start.is_none() && start.is_some() {
            let id = self.summary.id;
            self.cloud_queued = locked(&self.store)
                .ok()
                .and_then(|store| monocode_core::cloud_sessions::queued_start_message(&store, id));
        }
        if self.cloud_start.is_none() && start.is_some() {
            self.cloud_start_collapsed = false;
        }
        self.cloud_start = start;
        self.sync_cloud_chrome(cx);
        self.sync_cloud_start_clock(cx);
        cx.notify();
    }
    /// Shows the start in the transcript row and the composer (placeholder), which draw it.
    pub(super) fn sync_cloud_chrome(&mut self, cx: &mut Context<Self>) {
        let (start, queued) = (self.cloud_start.clone(), self.cloud_queued.clone());
        self.transcript
            .update(cx, |view, cx| view.set_cloud_start(start, queued, cx));
        let wanted = self.cloud_start.as_ref().and_then(|start| {
            let base = self
                .cloud_placeholder
                .clone()
                .unwrap_or_else(|| self.input.read(cx).placeholder_text().clone());
            start.placeholder(&base)
        });
        match wanted {
            Some(text) => {
                if self.cloud_placeholder.is_none() {
                    self.cloud_placeholder = Some(self.input.read(cx).placeholder_text().clone());
                }
                self.input
                    .update(cx, |input, cx| input.set_placeholder(text, cx));
            }
            None => {
                if let Some(base) = self.cloud_placeholder.take() {
                    self.input
                        .update(cx, |input, cx| input.set_placeholder(base, cx));
                }
            }
        }
    }
    /// Asks the shell to Retry, Rebuild, open the browser, delete the codespace or sandbox,
    /// show the Codespace panel, or (for a dead sandbox) archive the session or start a new one.
    pub(crate) fn request_cloud_start_action(
        &mut self,
        action: crate::cloud_start_view::StartAction,
        cx: &mut Context<Self>,
    ) {
        cx.emit(crate::cloud_start_view::CloudStartAction { action });
    }
    /// The codespace is open: the session now lives on `session`'s project. A message queued
    /// meanwhile goes out once the host history has loaded.
    pub(crate) fn cloud_start_ready(
        &mut self,
        session: Session,
        ready: Arc<crate::cloud_start_view::CloudStartSnapshot>,
        cx: &mut Context<Self>,
    ) {
        // The sidebar lists a session only once it has messages; this one is launching.
        let launching = self.summary.has_messages;
        self.summary = session;
        self.summary.has_messages |= launching;
        let cwd = self.summary.cwd.clone();
        self.review.update(cx, |review, cx| review.set_cwd(cwd, cx));
        self.cloud_start = Some(ready);
        self.sync_cloud_chrome(cx);
        self.sync_cloud_start_clock(cx);
        if self.cloud_queued.is_some() {
            self.remote_state.app_prompt_waiting = true;
        }
        self.status = format!(
            "{} ready",
            self.cloud_start.as_ref().map_or("Codespace", |s| s.title())
        );
        self.load(cx);
        cx.emit(SessionChanged);
        cx.notify();
    }
    /// A send while the workspace starts joins the session's send queue, like a message sent
    /// behind a running turn. The queue drains once the workspace is ready and loaded.
    pub(super) fn queue_start_send(&mut self, cx: &mut Context<Self>) {
        if self.attaching || self.input.read(cx).is_composing() {
            return;
        }
        if !self.attachments.is_empty() {
            self.status = format!(
                "Attachments cannot be sent while the {} starts. Remove them or wait.",
                self.cloud_start_noun()
            );
            cx.notify();
            return;
        }
        if !self.input.read(cx).has_message_text() {
            return;
        }
        if self.queue_composer_prompt(true, cx).is_some() {
            self.status = format!(
                "Queued. It is sent when the {} is ready.",
                self.cloud_start_noun()
            );
        }
        cx.notify();
    }
    /// Moves the composer text and attachments into the send queue as a plain message (never a
    /// steer: a cloud workspace takes no steering) and clears the composer. Returns the id of
    /// the queued message. `save` persists the queue; the new-session draft keeps it in memory
    /// because that local session never drains it.
    pub(super) fn queue_composer_prompt(
        &mut self,
        save: bool,
        cx: &mut Context<Self>,
    ) -> Option<String> {
        let text = self
            .input
            .update(cx, |input, cx| input.take(cx))
            .unwrap_or_default();
        let text = text.trim().to_owned();
        if text.is_empty() && self.attachments.is_empty() {
            return None;
        }
        let mode = self.settings.plan.then(|| "plan".to_owned());
        let attachments = std::mem::take(&mut self.attachments);
        let mut message = self.message(text, attachments, mode.clone(), cx);
        message.steer = false;
        let id = message.id.clone();
        self.follow_ups.motion.insert(
            &id,
            self.follow_ups.state.messages.is_empty(),
            !cx.reduce_motion(),
        );
        self.follow_ups.state.messages.push(message);
        self.completion
            .update(cx, |completion, cx| completion.dismiss(cx));
        if mode.is_some() {
            self.clear_submitted_mode(cx);
        }
        if save {
            self.save_follow_ups(cx);
            cx.emit(SessionChanged);
        } else {
            // Held while a create runs: the queue must not move on its own, and the entry is
            // dimmed and inert like a message that is being sent.
            self.follow_ups.sending = Some(id.clone());
        }
        cx.notify();
        Some(id)
    }
    /// Sends the head of the send queue of a remote (cloud) session, once. The queue never
    /// wakes or creates anything: it waits while the workspace is starting, stopped, offline,
    /// reconnecting or has a command to confirm, and while a turn runs (the normal queue
    /// rule, `pump_follow_ups` does not call this when busy). The message leaves the queue only
    /// when the runtime reports it delivered; a launch that does not start fails it in place.
    pub(super) fn drain_remote_follow_up(
        &mut self,
        message: monocode_core::follow_up::Message,
        cx: &mut Context<Self>,
    ) {
        if self.cloud_start_blocks()
            || !self.remote_state.online
            || self.remote_state.pending
            || self.remote_state.waking
            || self.cloud_stopped
            || message.error.is_some()
        {
            return;
        }
        let capabilities = &self.remote_state.capabilities;
        let blocker = if !message.attachments.is_empty()
            && !capabilities.contains("attachments.upload")
        {
            Some("Update Monoptah Host in Connections settings to send attachments.")
        } else if message.mode.as_deref() == Some("plan") && !capabilities.contains("sessions.plan")
        {
            Some("Update Monoptah Host in Connections settings to use Plan mode.")
        } else {
            None
        };
        if let Some(blocker) = blocker {
            // Pauses the queue and shows the error with a Retry, like any failed message.
            self.fail_follow_up(&message.id, blocker.into(), cx);
            cx.notify();
            return;
        }
        // The message stays in the queue, marked as being sent, until the runtime reports the
        // command journaled (`Update::delivered`). A launch that fails earlier (waking the
        // host, models, attachment upload) therefore never loses the prompt.
        let id = message.id.clone();
        self.follow_ups.sending = Some(id.clone());
        self.settings.mono_message_id = Some(id.clone());
        self.launch_message(message, cx);
        self.settings.mono_message_id = None;
        if !self.busy {
            self.fail_follow_up(&id, self.status.clone(), cx);
        }
        cx.notify();
    }
    /// Takes back a message put in the queue by [`Self::queue_composer_prompt`] with `save`
    /// false. Its attachments always return to the composer; the text does when `restore` is set.
    pub(super) fn unqueue_composer_prompt(
        &mut self,
        id: &str,
        restore: bool,
        cx: &mut Context<Self>,
    ) {
        if self.follow_ups.sending.as_deref() == Some(id) {
            self.follow_ups.sending = None;
        }
        let Some(position) = self
            .follow_ups
            .state
            .messages
            .iter()
            .position(|message| message.id == id)
        else {
            return;
        };
        let message = self.follow_ups.state.messages.remove(position);
        // The new session is created from the text alone: attachments and Plan mode stay here.
        let (kept_attachments, kept_plan) = (
            !message.attachments.is_empty(),
            message.mode.as_deref() == Some("plan"),
        );
        self.attachments.extend(message.attachments);
        if kept_plan {
            self.settings.plan = true;
        }
        if !restore && (kept_attachments || kept_plan) {
            self.status = "Attachments and Plan mode are not sent to a new session. They stay in the composer.".into();
        }
        if restore && self.input.read(cx).content.trim().is_empty() {
            self.input
                .update(cx, |input, cx| input.set_text(message.text, cx));
        }
        cx.notify();
    }
    pub(super) fn is_remote(&self) -> bool {
        self.summary.cwd.starts_with("remote://")
    }
    /// A remote session that lives in a codespace or a Docker sandbox: Claude only, so the
    /// model picker offers no other harness.
    pub(super) fn is_cloud_session(&self) -> bool {
        self.cloud_noun().is_some()
    }
    /// `codespace` or `sandbox` for a cloud session, `None` for any other.
    pub(super) fn cloud_noun(&self) -> Option<&'static str> {
        if !self.is_remote() {
            return None;
        }
        locked(&self.store)
            .and_then(|store| store.remote_session(self.summary.id))
            .ok()
            .flatten()
            .and_then(|s| s.project.cloud())
            .map(|cloud| cloud.noun())
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
    ///
    /// Wakes a codespace and a Docker sandbox alike: sending is an explicit act whoever performs
    /// it (the user, a Mono, a workflow or the app API). Drafts and background loads never come
    /// here.
    pub(crate) fn reconnect_cloud(&mut self, cx: &mut Context<Self>) -> bool {
        if !self.is_cloud_session() {
            return false;
        }
        if self.remote_state.waking {
            return true;
        }
        self.remote_state.waking = true;
        self.remote_state.epoch += 1;
        let noun = self.cloud_noun().unwrap_or("codespace");
        self.status = format!("Reconnecting the {noun}...");
        cx.notify();
        let store = self.store.clone();
        let id = self.summary.id;
        let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
        self.remote_state.wake_cancel = Some(cancel.clone());
        // Progress lines from the worker reach the status line as they arrive.
        let (lines, heard) = async_channel::unbounded::<String>();
        cx.spawn(async move |this, cx| {
            while let Ok(line) = heard.recv().await {
                if this
                    .update(cx, |this, cx| {
                        if this.remote_state.waking {
                            this.status = line;
                            cx.notify();
                        }
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
        cx.spawn(async move |this, cx| {
            let result = crate::cloud_wake::reconnect(store, id, cancel, lines, cx).await;
            let _ = this.update(cx, |this, cx| {
                this.remote_state.waking = false;
                this.remote_state.wake_cancel = None;
                match result {
                    Ok(true) => this.cloud_reconnected(cx),
                    Ok(false) => this.remote_connection_error(
                        &format!("This {noun} is unavailable. It may have been deleted."),
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
    /// The workspace of this session was brought back by a user action (the Reconnect button, a
    /// send or a click on a file link): clears the error, tells the shell and reloads.
    pub(crate) fn cloud_reconnected(&mut self, cx: &mut Context<Self>) {
        self.remote_state.error = None;
        self.cloud_stopped = false;
        cx.emit(CloudResumed);
        // Whatever refresh was in flight talked to the stopped host. Start
        // over: the queue drains only when a fresh refresh has reached the
        // host (its success path marks the view online and pumps the queue).
        self.remote_state.epoch += 1;
        self.remote_state.refreshing = false;
        if !self.busy {
            self.remote_state.online = false;
        }
        self.load_remote(cx);
        self.refresh_remote_snapshot(cx);
        cx.notify();
    }
    /// Whether the 3 second clock may refresh now. A stopped or waking workspace answers
    /// nothing (its old tunnel can still accept), so polling it only produces timeouts.
    pub(super) fn background_refresh_due(&self) -> bool {
        !self.busy
            && !self.stopping
            && !self.history_unloaded
            && !self.remote_state.waking
            && !self.cloud_stopped
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
                            if this.background_refresh_due() {
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
        if self.cloud_start_blocks() || self.remote_state.refreshing || self.busy || self.stopping {
            return;
        }
        self.remote_state.refreshing = true;
        let epoch = self.remote_state.epoch;
        self.remote_state.refresh_epoch = epoch;
        #[cfg(test)]
        let stub = self.remote_state.refresh_stub.clone();
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
            #[cfg(test)]
            if let Some(stub) = stub {
                stub?;
                return Ok::<_, String>((remote, settings, None, None, None));
            }
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
                if this.remote_state.epoch != epoch {
                    // A reconnect began or ended since this started: the answer (or the
                    // timeout) is about a host that is gone. Its result is dropped, but if it
                    // still holds the flags (no newer refresh took over, e.g. the reconnect
                    // failed or was cancelled) they are released so the view can refresh again.
                    if this.remote_state.refresh_epoch == epoch {
                        this.remote_state.refreshing = false;
                        this.loading = false;
                    }
                    return;
                }
                this.remote_state.refreshing = false;
                this.loading = false;
                match result {
                    Ok((remote, settings, snapshot, history, descriptor)) => {
                        this.remote_state.online = true;
                        this.remote_state.was_online = true;
                        this.remote_state.error = remote
                            .rejected
                            .as_ref()
                            .and_then(|rejected| rejected["error"].as_str().map(str::to_owned));
                        this.remote_state.rejected = remote.rejected;
                        this.remote_state.pending = !remote.outbox.is_empty()
                            || this.remote_state.legacy_recovery_error().is_some();
                        // A launch whose first prompt is still on its way has no stored messages
                        // yet; the sidebar lists a session only once it has some.
                        let launching = this.summary.has_messages && snapshot.is_none();
                        this.summary = remote.summary;
                        this.summary.has_messages |= launching;
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
                        this.pump_follow_ups(cx);
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
    /// Stops a reconnect that is running; the worker ends at its next step.
    pub(super) fn cancel_cloud_reconnect(&mut self, cx: &mut Context<Self>) {
        if let Some(cancel) = &self.remote_state.wake_cancel {
            cancel.store(true, std::sync::atomic::Ordering::Relaxed);
            self.status = "Cancelling the reconnect...".into();
            cx.notify();
        }
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
            let typed = self.input.read(cx).has_message_text();
            // Only a typed prompt from the user's own hand wakes a workspace from here, and it
            // goes through the send queue: it is sent once, when the reconnect has loaded.
            if unreachable && self.user_send && typed && self.is_cloud_session() {
                if self.input.read(cx).content.trim_start().starts_with('/') {
                    self.status = "Commands are available once the workspace is ready".into();
                    cx.notify();
                    return true;
                }
                if self.queue_composer_prompt(true, cx).is_some() {
                    self.reconnect_cloud(cx);
                    let noun = self.cloud_noun().unwrap_or("codespace");
                    self.status =
                        format!("Reconnecting the {noun}. Your message is sent when it is ready.");
                    cx.notify();
                    return true;
                }
            } else if unreachable && !self.user_send && self.reconnect_cloud(cx) {
                let noun = self.cloud_noun().unwrap_or("codespace");
                self.status = format!("Reconnecting the {noun}...");
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
                let noun = self.cloud_noun().unwrap_or("codespace");
                self.status = format!("Reconnecting the {noun} to send the first prompt…");
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
            self.cloud_queued = None;
            self.sync_cloud_chrome(cx);
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
    /// A sandbox reconnect is running without having failed yet: the banner then carries the
    /// progress line and the Cancel button (it used to appear only after an error).
    fn sandbox_reconnecting(&self) -> bool {
        self.remote_state.waking
            && self.remote_state.error.is_none()
            && !self.remote_state.pending
            && self.cloud_noun() == Some("sandbox")
    }
    /// A send while the workspace is known to be stopped. The user's own submit with a prompt
    /// queues it, resumes the workspace and sends the queue once connected; clicking the Resume pill resumes without
    /// one. Enter on an empty composer never resumes. A Mono or the app API does not come through
    /// here: its send is queued and wakes the instance through `reconnect_cloud`.
    pub(super) fn resume_for_user_send(&mut self, cx: &mut Context<Self>) -> bool {
        if !(self.cloud_stopped && self.is_cloud_session() && self.is_remote()) {
            return false;
        }
        if !self.user_send {
            return false;
        }
        let typed = self.input.read(cx).has_message_text();
        if !typed && !self.resume_click {
            if !self.attachments.is_empty() {
                self.status = "Attachments need a prompt to be sent. Write a message first.".into();
                cx.notify();
            }
            return true;
        }
        if typed {
            if self.input.read(cx).content.trim_start().starts_with('/') {
                self.status = "Commands are available once the workspace is ready".into();
                cx.notify();
                return true;
            }
            // The prompt leaves the composer for the send queue and goes out once, when the
            // resume has connected and the history is loaded.
            self.queue_composer_prompt(true, cx);
        }
        self.reconnect_cloud(cx);
        true
    }
    /// The Send button while the workspace is stopped: its tooltip and whether it can be used.
    pub(super) fn cloud_resume_tip(&self) -> Option<(SharedString, bool)> {
        if !(self.cloud_stopped && self.is_remote() && self.is_cloud_session()) {
            return None;
        }
        let noun = self.cloud_noun().unwrap_or("codespace");
        Some(if self.remote_state.waking {
            (format!("Resuming {noun}\u{2026}").into(), false)
        } else {
            (format!("Resume {noun} and send").into(), true)
        })
    }
    /// The shell tells whether its cached state of this session's workspace is Stopped.
    pub(crate) fn set_cloud_stopped(&mut self, stopped: bool, cx: &mut Context<Self>) {
        if self.cloud_stopped != stopped {
            self.cloud_stopped = stopped;
            cx.notify();
        }
    }
    #[cfg(test)]
    pub(crate) fn cloud_stopped_for_test(&self) -> bool {
        self.cloud_stopped
    }
    /// The live link of the workspace, as a successful refresh would leave it.
    #[cfg(test)]
    pub(crate) fn set_online_for_test(&mut self, online: bool) {
        self.remote_state.online = online;
        self.remote_state.was_online |= online;
    }
    /// Whether the app has a live connection to this session's workspace, for the connection
    /// strip of the Files and Changes panels. Reads cached view state only; it never wakes or
    /// reconnects (the strip's Connect action is `reconnect_cloud`, a user action).
    pub(crate) fn panel_link(&self) -> crate::panel_root::Link {
        use crate::panel_root::Link;
        if self.remote_state.waking {
            Link::Starting
        } else if self.remote_state.online && !self.cloud_stopped {
            Link::Connected
        } else {
            Link::NotConnected
        }
    }
    /// Whether `panel_link` is a fact: the workspace is known to be down, waking or live. A
    /// conversation that has not finished its first refresh knows nothing yet, and the panels
    /// then keep reading as they did (no dimming, no read-only) instead of flashing offline.
    pub(crate) fn panel_link_known(&self) -> bool {
        let state = &self.remote_state;
        state.online
            || state.waking
            || self.cloud_stopped
            || state.error.is_some()
            || state.was_online
    }
    /// The slim "Codespace stopped" / "Sandbox stopped" banner with a Resume button applies:
    /// the workspace is stopped and nothing else (an error, a pending command) needs the notice.
    fn cloud_stopped_banner(&self) -> bool {
        self.cloud_stopped
            && self.remote_state.error.is_none()
            && !self.remote_state.pending
            && self.is_cloud_session()
    }
    /// The sandbox is deleted or its container was recreated: Reconnect can not help.
    fn sandbox_unrecoverable(&self) -> bool {
        self.remote_state
            .error
            .as_deref()
            .is_some_and(monocode_core::cloud_sessions::docker_start::is_sandbox_unrecoverable)
            && self.cloud_noun() == Some("sandbox")
    }
    /// The notice of a remote session's connection state: an error, a command that needs
    /// confirmation, a stopped or resuming workspace. It sits on the ledge above the composer in
    /// the card style of the cloud start panel, with the actions on the right. While the start
    /// panel is showing (a workspace that is starting or whose start failed) this stays hidden:
    /// that panel already tells the state and nothing of the host is reachable yet.
    pub(super) fn remote_notice(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let stopped = self.is_remote() && self.cloud_stopped_banner();
        let reconnecting = self.sandbox_reconnecting() || (stopped && self.remote_state.waking);
        if !self.is_remote()
            || self.busy
            || self
                .cloud_start
                .as_ref()
                .is_some_and(|start| start.panel_title().is_some())
            || (self.remote_state.error.is_none()
                && !self.remote_state.pending
                && !reconnecting
                && !stopped)
        {
            return None;
        }
        let legacy = self.remote_state.legacy_recovery_error();
        let pending = self.remote_state.pending;
        let dead = legacy.is_none() && !pending && self.sandbox_unrecoverable();
        let rejected = self.remote_state.rejected.is_some();
        let waking = self.remote_state.waking;
        let noun = match self.cloud_noun() {
            Some("sandbox") => "Sandbox",
            _ => "Codespace",
        };
        // (title, message, tone, mark): danger for errors, warning for work in progress or a
        // command to confirm, neutral for a stopped workspace.
        let (title, message, tone, mark) = if let Some(error) = legacy {
            (
                "Recovery needed".to_owned(),
                error.to_owned(),
                danger_color(),
                "x",
            )
        } else if pending {
            (
                "Confirm command".to_owned(),
                "A remote command needs confirmation. Retry uses its original command ID."
                    .to_owned(),
                warning_color(),
                "circle-alert",
            )
        } else if stopped {
            (
                format!("{noun} stopped"),
                if waking {
                    self.status.clone()
                } else {
                    String::new()
                },
                if waking {
                    warning_color()
                } else {
                    content(0.45)
                },
                if waking { "loader" } else { "queue-pause" },
            )
        } else if reconnecting {
            (
                format!("Reconnecting {}", noun.to_lowercase()),
                self.status.clone(),
                warning_color(),
                "loader",
            )
        } else if dead {
            (
                format!("{noun} unavailable"),
                self.remote_state.error.clone().unwrap_or_default(),
                danger_color(),
                "x",
            )
        } else {
            (
                if rejected {
                    "Message rejected"
                } else {
                    "Connection problem"
                }
                .to_owned(),
                self.remote_state.error.clone().unwrap_or_default(),
                danger_color(),
                "x",
            )
        };
        let spinning = mark == "loader";
        let mark: AnyElement = if spinning {
            div()
                .text_color(tone)
                .child(crate::right_panel::spinner(
                    "remote-notice-spin",
                    "loader",
                    14.,
                ))
                .into_any_element()
        } else {
            icon(mark, 14.).text_color(tone).into_any_element()
        };
        let action = |id: &'static str, label: &'static str, action, cx: &mut Context<Self>| {
            crate::settings::widgets::secondary_button(id, label, false, false)
                .debug_selector(move || id.into())
                .child(label)
                .on_click(
                    cx.listener(move |this, _, _, cx| this.request_cloud_start_action(action, cx)),
                )
        };
        let retry_label = if rejected { "Restore message" } else { "Retry" };
        let header = div()
            .id("remote-notice-header")
            .debug_selector(|| "remote-notice-header".into())
            .h(px(30.))
            .flex()
            .items_center()
            .gap(px(8.))
            .child(div().size(px(16.)).flex_shrink_0().child(mark))
            .child(
                div()
                    .id("remote-notice-title")
                    .debug_selector(|| "remote-notice-title".into())
                    .flex_shrink_0()
                    .text_color(if tone == content(0.45) {
                        content(0.8)
                    } else {
                        tone
                    })
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .child(title.clone()),
            )
            .child(
                div()
                    .id("remote-notice-message")
                    .debug_selector(|| "remote-notice-message".into())
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_color(content(0.35))
                    .child(message),
            )
            .when(stopped, |d| {
                let label = if waking { "Resuming\u{2026}" } else { "Resume" };
                d.child(
                    crate::settings::widgets::secondary_button(
                        "cloud-resume",
                        label,
                        false,
                        waking,
                    )
                    .debug_selector(|| "cloud-resume".into())
                    .child(label)
                    .on_click(cx.listener(|this, _, _, cx| {
                        if !this.remote_state.waking {
                            this.reconnect_cloud(cx);
                        }
                    })),
                )
            })
            .when(dead, |d| {
                d.child(action(
                    "remote-archive-session",
                    "Archive session",
                    crate::cloud_start_view::StartAction::ArchiveSession,
                    cx,
                ))
                .child(action(
                    "remote-new-session",
                    "New session",
                    crate::cloud_start_view::StartAction::NewSession,
                    cx,
                ))
            })
            .when(
                legacy.is_none()
                    && !pending
                    && !dead
                    && !reconnecting
                    && !stopped
                    && self.is_cloud_session(),
                |d| {
                    d.child(
                        crate::settings::widgets::secondary_button(
                            "remote-reconnect",
                            "Reconnect",
                            !waking,
                            false,
                        )
                        .debug_selector(|| "remote-reconnect".into())
                        .child("Reconnect")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.reconnect_cloud(cx);
                        })),
                    )
                },
            )
            .when(waking, |d| {
                d.child(
                    crate::settings::widgets::secondary_button(
                        "remote-reconnect-cancel",
                        "Cancel",
                        true,
                        false,
                    )
                    .debug_selector(|| "remote-reconnect-cancel".into())
                    .child("Cancel")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.cancel_cloud_reconnect(cx);
                    })),
                )
            })
            .when(
                legacy.is_none() && !dead && !reconnecting && !stopped,
                |d| {
                    d.child(
                        crate::settings::widgets::secondary_button(
                            "remote-retry",
                            retry_label,
                            true,
                            false,
                        )
                        .debug_selector(|| "remote-retry".into())
                        .child(retry_label)
                        .on_click(cx.listener(|this, _, _, cx| {
                            if this.remote_state.rejected.is_some() {
                                this.restore_remote_message(cx);
                            } else {
                                this.retry_remote(cx);
                            }
                        })),
                    )
                },
            );
        let card = composer_shelf::card("remote-notice", self.classic_layout)
            .debug_selector(|| "remote-notice".into())
            .role(Role::Group)
            .aria_label(title)
            .child(header);
        Some(
            composer_shelf::shelf("remote-notice-shelf")
                .px(px(8.))
                .child(card)
                .into_any_element(),
        )
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
