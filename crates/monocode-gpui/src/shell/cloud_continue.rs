//! "Continue locally": the session menu action that brings a cloud session back to this
//! computer as ONE local session with ONE Claude id (design 4.2.3). It is a user-confirmed
//! action: syncing from the Host wakes a stopped workspace, so nothing here runs without the
//! confirmation, and nothing is done by a poll.
//!
//! The work itself is `CloudSessions::continue_locally` on a worker. It cannot be interrupted
//! half way, but any failure before its last step leaves the session in the cloud, so the
//! dialog only offers Cancel before it starts and Retry after a failure.
use super::cloud::{CloudMark, WORKSPACE_BUSY};
use super::*;
use monocode_core::cloud_sessions::{CloudSessions, ConvertError, Converted};
use monocode_core::store::WORKSPACE_LOST;

/// Brings one cloud session back (see `CloudSessions::continue_locally`). A seam for tests.
pub(super) type ContinueFn = Arc<
    dyn Fn(&CloudSessions, i64, &mut dyn FnMut(&str)) -> Result<Converted, ConvertError>
        + Send
        + Sync,
>;

/// What the user can do with the backup of an interrupted return.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Op {
    /// Put the files back as they were before the attempt.
    Restore,
    /// Delete the backup and forget the attempt.
    Discard,
    /// The cloud state is in place: only make the index follow it.
    Keep,
}

/// Does `Op` on the backup of an interrupted return and says what happened. A seam for tests.
pub(super) type BackupFn =
    Arc<dyn Fn(&CloudSessions, i64, Op) -> Result<String, String> + Send + Sync>;

pub(super) fn default_backup() -> BackupFn {
    Arc::new(|cloud, id, op| {
        match op {
        Op::Restore => cloud.restore_continue_backup(id).map(|(fresh, original)| {
            format!(
                "Your files are back as they were before the interrupted return. The state they were in just before is saved in {} and the earlier backup is still in {}. Delete them when you no longer need them.",
                fresh.display(),
                original.display()
            )
        }),
        Op::Discard => cloud
            .discard_continue_marker(id)
            .map(|()| "The backup was deleted.".to_string()),
        Op::Keep => cloud
            .keep_cloud_state(id)
            .map(|backup| {
                format!(
                    "The cloud state is kept. Your earlier uncommitted work is still saved in {}; delete that folder when you no longer need it.",
                    backup.display()
                )
            }),
    }
    })
}

pub(super) fn default_continue() -> ContinueFn {
    Arc::new(|cloud, id, progress| cloud.continue_locally(id, progress))
}

/// What the dialog is doing.
pub(super) enum Phase {
    Confirm,
    Working {
        progress: Arc<std::sync::Mutex<String>>,
    },
    Failed(String),
    /// Asking before the backup of an interrupted return is restored or deleted.
    ConfirmBackup {
        op: Op,
    },
}

/// What the dialog is for, so Retry runs the right thing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Action {
    Continue,
    Backup { op: Op },
}

pub(super) struct ContinueDialog {
    id: i64,
    title: String,
    kind: Kind,
    noun: &'static str,
    action: Action,
    /// The backup of an interrupted return of this session, when there is one.
    backup: Option<String>,
    /// "Keep the cloud state" applies: the branch is at the cloud commit.
    keepable: bool,
    phase: Phase,
    focus: FocusHandle,
}

/// Which of the three cases the dialog describes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Kind {
    /// A local session that moved to the cloud.
    Moved { unsynced: bool },
    /// A moved session whose workspace is gone.
    Lost,
    /// A session that was started in the cloud.
    Born,
}

pub(super) struct State {
    pub dialog: Option<ContinueDialog>,
    pub run: ContinueFn,
    pub backup: BackupFn,
}

#[cfg(test)]
impl State {
    /// For tests: which step the dialog is on.
    pub fn phase_name(&self) -> &'static str {
        match self.dialog.as_ref().map(|d| &d.phase) {
            None => "closed",
            Some(Phase::Confirm) => "confirm",
            Some(Phase::Working { .. }) => "working",
            Some(Phase::Failed(_)) => "failed",
            Some(Phase::ConfirmBackup { .. }) => "confirm-backup",
        }
    }
}

impl State {
    /// For `sessions.moveStatus`: how a return of session `id` stands, `None` when no return of
    /// it is running or failed (a dialog still waiting for the user's confirmation is none).
    pub(super) fn api_status(&self, id: i64) -> Option<(&'static str, Option<String>)> {
        let dialog = self.dialog.as_ref().filter(|d| d.id == id)?;
        if dialog.action != Action::Continue {
            return None;
        }
        match &dialog.phase {
            Phase::Working { .. } => Some(("running", None)),
            Phase::Failed(error) => Some(("failed", Some(error.clone()))),
            Phase::Confirm => Some(("awaitingConfirmation", None)),
            Phase::ConfirmBackup { .. } => None,
        }
    }
}

impl Default for State {
    fn default() -> Self {
        Self {
            dialog: None,
            run: default_continue(),
            backup: default_backup(),
        }
    }
}

/// What Restore and Discard tell the user before anything happens.
pub(super) fn backup_text(op: Op, path: &str) -> String {
    if op == Op::Keep {
        return format!(
            "The branch is at the cloud commit, left there by an interrupted return. Keep makes the index follow that commit and forgets the attempt. Your files are not touched. Your earlier uncommitted work stays in {path}; delete that folder when you no longer need it."
        );
    }
    if op == Op::Restore {
        format!(
            "An earlier return from the cloud was interrupted. Your uncommitted work from before it is saved in {path}. Restore takes a fresh backup of your files as they are now (kept next to it), then puts your files back as they were before that attempt. It refuses, and changes nothing, if the branch or commit changed or a file is in the way."
        )
    } else {
        format!(
            "This deletes the backup of your uncommitted work in {path} and forgets the interrupted return. It cannot be undone. Only do this when your files are as you want them."
        )
    }
}

/// What the confirmation says will happen. It is the only thing the user reads before a
/// workspace may be woken and a Claude transcript replaced.
pub(super) fn confirm_text(kind: Kind, title: &str, noun: &str) -> String {
    let who = super::cloud::quoted_title(title);
    match kind {
        Kind::Lost => format!(
            "The {noun} of {who} was deleted, so the turns written there after the last sync are lost. The conversation continues on this computer. Its next message starts a new Claude session from a summary of the messages kept here; the old local Claude file is not resumed."
        ),
        Kind::Moved { unsynced } => format!(
            "{who} comes back to this computer as the same conversation. Monoptah will wake the {noun} if it is stopped, {sync}apply the cloud code here (in place if your files are unchanged since the move, otherwise in a new worktree; your uncommitted changes are never overwritten), bring the Claude transcript back and then close the cloud copy. Nothing changes until all of that succeeded; if it fails, the conversation stays in the cloud. After it succeeded, the {noun} is deleted when no other session uses it and nothing on it is missing here; otherwise it is kept and you are told why.",
            sync = if unsynced {
                "copy the cloud turns that are not here yet, "
            } else {
                "check for newer cloud turns, "
            }
        ),
        Kind::Born => format!(
            "{who} gets a copy on this computer that replaces it in the list (it is not archived). Monoptah will wake the {noun} if it is stopped, copy the conversation, put the cloud code in a new worktree and bring the Claude transcript back. After that, the {noun} is deleted when no other session uses it and nothing on it is missing here; otherwise it is kept and you are told why."
        ),
    }
}

/// The status line after a return: the warnings follow the lead.
fn back_text(message: &str) -> String {
    if message.is_empty() {
        "Back on this computer.".to_string()
    } else {
        format!("Back on this computer. {message}")
    }
}

impl Shell {
    /// Whether the session menu offers Continue locally for `id`.
    pub(super) fn can_continue_locally(&self, id: i64) -> bool {
        self.cloud_marks.contains_key(&id)
            && !self.cloud_busy.contains(&id)
            && self.cloud_continue.dialog.is_none()
            && !self.workspace_busy_of(id)
    }

    fn continue_kind(&self, id: i64) -> Kind {
        match self.cloud_located.get(&id) {
            Some(located) if located.sync_error.as_deref() == Some(WORKSPACE_LOST) => Kind::Lost,
            Some(located) => Kind::Moved {
                unsynced: located.unsynced,
            },
            None => Kind::Born,
        }
    }

    /// Opens the confirmation. Nothing is woken or changed until it is confirmed.
    pub(super) fn open_continue_locally(
        &mut self,
        id: i64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.can_continue_locally(id) {
            if self.workspace_busy_of(id) {
                self.error = Some(WORKSPACE_BUSY.into());
                cx.notify();
            }
            return;
        }
        // A turn running here would be a second writer next to the one that comes back.
        if self.session_metadata(id, cx).busy {
            self.error =
                Some("A turn is running. Wait for it to finish, then continue locally.".into());
            cx.notify();
            return;
        }
        let noun = self
            .cloud_marks
            .get(&id)
            .map_or("workspace", |mark: &CloudMark| mark.kind.noun());
        let dialog = ContinueDialog {
            id,
            title: self.session_metadata(id, cx).display_title(),
            kind: self.continue_kind(id),
            noun,
            action: Action::Continue,
            backup: self.continue_backup_of(id),
            keepable: false,
            phase: Phase::Confirm,
            focus: cx.focus_handle(),
        };
        window.focus(&dialog.focus, cx);
        self.cloud_continue.dialog = Some(dialog);
        cx.notify();
    }

    pub(super) fn close_continue_locally(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // A running continue cannot be interrupted; its result closes the dialog.
        if self
            .cloud_continue
            .dialog
            .as_ref()
            .is_some_and(|d| matches!(d.phase, Phase::Working { .. }))
        {
            return;
        }
        if self.cloud_continue.dialog.take().is_some() {
            self.focus_active(window, cx);
            cx.notify();
        }
    }

    /// Runs it, after the user confirmed.
    pub(super) fn confirm_continue_locally(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(dialog) = self.cloud_continue.dialog.as_mut() else {
            return;
        };
        if matches!(dialog.phase, Phase::Working { .. }) {
            return;
        }
        let id = dialog.id;
        let progress = Arc::new(std::sync::Mutex::new(String::from("Starting")));
        dialog.phase = Phase::Working {
            progress: progress.clone(),
        };
        window.focus(&dialog.focus, cx);
        self.cloud_busy.insert(id);
        // Stop or Delete of the same workspace from another session must not interleave.
        let workspace = self.cloud_marks.get(&id).map(|mark| mark.key());
        if let Some(key) = &workspace {
            self.cloud_workspaces_busy.insert(key.clone());
        }
        self.lock_cloud_composer(&[id], true, cx);
        let cloud = self.cloud.clone();
        let run = self.cloud_continue.run.clone();
        let work = cx.background_executor().spawn(async move {
            run(&cloud, id, &mut |line| {
                if let Ok(mut current) = progress.lock() {
                    *current = line.to_string();
                }
            })
        });
        // The worker cannot touch the window, so the dialog polls its progress line.
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(250))
                    .await;
                let live = this
                    .update(cx, |this, cx| {
                        let working = this
                            .cloud_continue
                            .dialog
                            .as_ref()
                            .is_some_and(|d| matches!(d.phase, Phase::Working { .. }));
                        if working {
                            cx.notify();
                        }
                        working
                    })
                    .unwrap_or(false);
                if !live {
                    break;
                }
            }
        })
        .detach();
        cx.spawn_in(window, async move |this, cx| {
            let result = work.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.cloud_busy.remove(&id);
                if let Some(key) = &workspace {
                    this.cloud_workspaces_busy.remove(key);
                }
                this.lock_cloud_composer(&[id], false, cx);
                this.finish_continue_locally(id, result, window, cx)
            });
        })
        .detach();
        cx.notify();
    }

    fn finish_continue_locally(
        &mut self,
        id: i64,
        result: Result<Converted, ConvertError>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match result {
            Ok(done) => {
                self.cloud_continue.dialog = None;
                self.adopt_continued(id, done, window, cx);
            }
            Err(error) => {
                // The session is still where it was: the dialog stays for a retry.
                let backup = self.continue_backup_of(id);
                let keepable = backup.is_some() && self.continue_keepable(id);
                if let Some(dialog) = self.cloud_continue.dialog.as_mut() {
                    dialog.phase = Phase::Failed(error.to_string());
                    dialog.backup = backup;
                    dialog.keepable = keepable;
                }
            }
        }
        cx.notify();
    }

    /// Applies a finished continue to the window. A session that moved keeps its entry (it is
    /// the same session); one born in the cloud is replaced by its new local session.
    fn adopt_continued(
        &mut self,
        id: i64,
        done: Converted,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut message = done.warnings.join(" ");
        let done_local = done.local_id;
        // The record of THIS return: the same session and the instance it came back from. An
        // older record of the session for another instance is none of this return's business.
        let came_from = self.cloud_marks.get(&id).map(|mark| mark.kind);
        let armed = self
            .cloud
            .return_cleanups_for_session(done.local_id)
            .into_iter()
            .find(|r| {
                r.cloud.name == done.codespace && came_from.is_none_or(|kind| r.cloud.kind == kind)
            })
            .filter(|record| record.auto_run());
        if done.local_id != id {
            self.remember_returned(id, done.local_id);
        }
        if done.local_id == id {
            self.session_summaries.insert(id, done.summary);
            self.cloud_located.remove(&id);
            self.sync_cloud_location(cx);
            self.refresh_cloud(cx);
        } else {
            self.adopt_converted(done, window, cx);
        }
        // Only after the adoption above: the instance is deleted once the return is complete
        // and nothing else uses it (the core checks again; it may keep it and say why).
        if let Some(record) = armed {
            let started = self.start_return_cleanup(&record.key(), Some(back_text(&message)), cx);
            let tail = if started {
                Some(format!(
                    "The {} {} is being deleted.",
                    record.cloud.noun(),
                    record.cloud.name
                ))
            } else {
                // Could not start: the record is now a Retry and says why, or (when even
                // that could not be saved) is offered again at the next start.
                let what = format!("{} {}", record.cloud.noun(), record.cloud.name);
                match self
                    .cloud
                    .return_cleanup_for(done_local, &record.cloud)
                    .map(|r| r.last_error)
                {
                    Some(Some(why)) => Some(format!("The {what} was kept: {why}.")),
                    Some(None) => Some(format!(
                        "The {what} was not deleted; the cleanup will be offered again at the next start."
                    )),
                    None => None,
                }
            };
            if let Some(tail) = tail {
                if !message.is_empty() {
                    message.push(' ');
                }
                message.push_str(&tail);
            }
        }
        self.error = Some(back_text(&message));
        cx.notify();
    }

    /// The backup path of an interrupted return of session `id`, when there is one.
    fn continue_backup_of(&self, id: i64) -> Option<String> {
        locked(&self.store)
            .ok()
            .and_then(|s| s.continue_backup_path(id))
    }

    /// Whether the branch of the interrupted return is at the cloud commit, the only state
    /// "Keep the cloud state" is for.
    fn continue_keepable(&self, id: i64) -> bool {
        let Some(raw) = locked(&self.store)
            .ok()
            .and_then(|s| s.cloud_continue_broken(id))
        else {
            return false;
        };
        let Ok(marker) = serde_json::from_str::<serde_json::Value>(&raw) else {
            return false;
        };
        let (root, cloud) = (
            marker["root"].as_str().unwrap_or_default(),
            marker["cloudHead"].as_str().unwrap_or_default(),
        );
        !cloud.is_empty()
            && monocode_core::cloud_sessions::move_out::head_commit(std::path::Path::new(root))
                .as_deref()
                == Some(cloud)
    }

    /// The session menu entries for an interrupted return (Delete is blocked until one of
    /// them is done).
    pub(super) fn continue_backup_items(&self, id: i64) -> Vec<MenuItem> {
        if self.continue_backup_of(id).is_none() {
            return vec![];
        }
        let mut items: Vec<MenuItem> =
            vec![MenuAction::new("restore-continue-backup", "Restore my backup…").into()];
        if self.continue_keepable(id) {
            items.push(MenuAction::new("keep-continue-backup", "Keep the cloud state…").into());
        }
        items.push(
            MenuAction::new("discard-continue-backup", "Discard backup…")
                .danger()
                .into(),
        );
        items
    }

    /// Opens the confirmation for Restore (`true`) or Discard. Nothing runs before it.
    pub(super) fn open_backup_action(
        &mut self,
        id: i64,
        op: Op,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(backup) = self.continue_backup_of(id) else {
            return;
        };
        if op == Op::Keep && !self.continue_keepable(id) {
            return;
        }
        if self.cloud_continue.dialog.is_some() || self.cloud_busy.contains(&id) {
            return;
        }
        let dialog = ContinueDialog {
            id,
            title: self.session_metadata(id, cx).display_title(),
            kind: Kind::Born,
            noun: "workspace",
            action: Action::Backup { op },
            backup: Some(backup),
            keepable: false,
            phase: Phase::ConfirmBackup { op },
            focus: cx.focus_handle(),
        };
        window.focus(&dialog.focus, cx);
        self.cloud_continue.dialog = Some(dialog);
        cx.notify();
    }

    /// From the failed dialog: switch to the confirmation of Restore or Discard.
    pub(super) fn ask_backup_action(&mut self, op: Op, cx: &mut Context<Self>) {
        if let Some(dialog) = self.cloud_continue.dialog.as_mut()
            && matches!(dialog.phase, Phase::Failed(_))
            && dialog.backup.is_some()
        {
            dialog.action = Action::Backup { op };
            dialog.phase = Phase::ConfirmBackup { op };
            cx.notify();
        }
    }

    /// Runs Restore or Discard on a worker, after the confirmation.
    pub(super) fn confirm_backup_action(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(dialog) = self.cloud_continue.dialog.as_mut() else {
            return;
        };
        let op = match (&dialog.phase, dialog.action) {
            (Phase::ConfirmBackup { op }, _) => *op,
            (Phase::Failed(_), Action::Backup { op }) => op,
            _ => return,
        };
        let id = dialog.id;
        let progress = Arc::new(std::sync::Mutex::new(String::from(match op {
            Op::Restore => "Restoring your files",
            Op::Discard => "Deleting the backup",
            Op::Keep => "Updating the index",
        })));
        dialog.phase = Phase::Working { progress };
        window.focus(&dialog.focus, cx);
        self.cloud_busy.insert(id);
        let cloud = self.cloud.clone();
        let run = self.cloud_continue.backup.clone();
        let work = cx
            .background_executor()
            .spawn(async move { run(&cloud, id, op) });
        cx.spawn_in(window, async move |this, cx| {
            let result = work.await;
            let _ = this.update_in(cx, |this, _window, cx| {
                this.cloud_busy.remove(&id);
                match result {
                    Ok(message) => {
                        this.cloud_continue.dialog = None;
                        this.error = Some(message);
                    }
                    // The marker stays: the dialog keeps the choice for a retry.
                    Err(error) => {
                        if let Some(dialog) = this.cloud_continue.dialog.as_mut() {
                            dialog.phase = Phase::Failed(error);
                        }
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub(super) fn cloud_continue_modal(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let dialog = self.cloud_continue.dialog.as_ref()?;
        let width = 520f32.min(window.viewport_size().width.as_f32() - 24.);
        let weak = cx.weak_entity();
        let working = matches!(dialog.phase, Phase::Working { .. });
        let text = |id: &'static str, text: String, dim: bool| {
            div()
                .id(id)
                .text_size(px(12.))
                .line_height(px(17.))
                .when(dim, |d| d.text_color(content(0.6)))
                .child(text)
        };
        let cancel = crate::modal::ghost_button("cloud-continue-cancel", "Cancel")
            .debug_selector(|| "cloud-continue-cancel".into())
            .cursor_pointer()
            .on_click(cx.listener(|this, _, window, cx| this.close_continue_locally(window, cx)));
        let action = dialog.action;
        let retry = crate::modal::primary_button("cloud-continue-go", "Retry")
            .debug_selector(|| "cloud-continue-go".into())
            .cursor_pointer()
            .on_click(cx.listener(move |this, _, window, cx| match action {
                Action::Continue => this.confirm_continue_locally(window, cx),
                Action::Backup { .. } => this.confirm_backup_action(window, cx),
            }));
        let go = |label: &'static str, cx: &mut Context<Self>| {
            crate::modal::primary_button("cloud-continue-go", label)
                .debug_selector(|| "cloud-continue-go".into())
                .cursor_pointer()
                .on_click(
                    cx.listener(|this, _, window, cx| this.confirm_continue_locally(window, cx)),
                )
        };
        let footer = div()
            .flex()
            .flex_nowrap()
            .items_center()
            .justify_end()
            .gap_2();
        let (heading, body, footer) = match &dialog.phase {
            Phase::Confirm => (
                "Continue locally?",
                div().child(text(
                    "cloud-continue-body",
                    confirm_text(dialog.kind, &dialog.title, dialog.noun),
                    false,
                )),
                footer.child(cancel).child(go("Continue locally", cx)),
            ),
            Phase::Working { progress } => (
                "Continuing locally",
                div().child(text(
                    "cloud-continue-progress",
                    format!(
                        "{}...",
                        progress
                            .lock()
                            .map(|line| line.clone())
                            .unwrap_or_default()
                            .trim_end_matches(['.', '\u{2026}'])
                    ),
                    false,
                )),
                footer,
            ),
            Phase::Failed(error) => (
                "Could not continue locally",
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(text("cloud-continue-error", error.clone(), false))
                    .child(text(
                        "cloud-continue-still",
                        "The conversation is still where it was; nothing was lost.".into(),
                        true,
                    )),
                {
                    let mut footer = footer.child(cancel);
                    // An interrupted return blocks Continue and Delete until its backup is
                    // restored or discarded; each is its own confirmation.
                    if dialog.backup.is_some() && dialog.action == Action::Continue {
                        footer = footer
                            .child(
                                crate::modal::ghost_button(
                                    "cloud-continue-restore",
                                    "Restore my backup…",
                                )
                                .debug_selector(|| "cloud-continue-restore".into())
                                .cursor_pointer()
                                .on_click(cx.listener(
                                    |this, _, _, cx| this.ask_backup_action(Op::Restore, cx),
                                )),
                            )
                            .children(dialog.keepable.then(|| {
                                crate::modal::ghost_button(
                                    "cloud-continue-keep",
                                    "Keep the cloud state…",
                                )
                                .debug_selector(|| "cloud-continue-keep".into())
                                .cursor_pointer()
                                .on_click(cx.listener(
                                    |this, _, _, cx| this.ask_backup_action(Op::Keep, cx),
                                ))
                            }))
                            .child(
                                crate::modal::danger_button(
                                    "cloud-continue-discard",
                                    "Discard backup…",
                                )
                                .debug_selector(|| "cloud-continue-discard".into())
                                .cursor_pointer()
                                .on_click(cx.listener(
                                    |this, _, _, cx| this.ask_backup_action(Op::Discard, cx),
                                )),
                            );
                    }
                    footer.child(retry)
                },
            ),
            Phase::ConfirmBackup { op } => {
                let op = *op;
                let path = dialog.backup.clone().unwrap_or_default();
                (
                    match op {
                        Op::Restore => "Restore my backup?",
                        Op::Discard => "Discard the backup?",
                        Op::Keep => "Keep the cloud state?",
                    },
                    div().child(text(
                        "cloud-continue-backup-body",
                        backup_text(op, &path),
                        false,
                    )),
                    footer.child(cancel).child(
                        match op {
                            Op::Restore => {
                                crate::modal::primary_button("cloud-continue-backup-go", "Restore")
                            }
                            Op::Keep => {
                                crate::modal::primary_button("cloud-continue-backup-go", "Keep")
                            }
                            Op::Discard => {
                                crate::modal::danger_button("cloud-continue-backup-go", "Discard")
                            }
                        }
                        .debug_selector(|| "cloud-continue-backup-go".into())
                        .cursor_pointer()
                        .on_click(
                            cx.listener(|this, _, window, cx| {
                                this.confirm_backup_action(window, cx)
                            }),
                        ),
                    ),
                )
            }
        };
        Some(
            div()
                .absolute()
                .size_full()
                .child(crate::modal::scrim("cloud-continue-scrim"))
                .child(
                    popover("cloud-continue-modal", Anchor::Center)
                        .modal_surface(self.workspace.boxy_layout())
                        .width(width)
                        .gap(0.)
                        .priority(60)
                        .on_dismiss(move |window, cx| {
                            let _ =
                                weak.update(cx, |this, cx| this.close_continue_locally(window, cx));
                        })
                        .child(
                            div()
                                .id("cloud-continue-dialog")
                                .role(Role::Dialog)
                                .aria_label("Continue locally")
                                .track_focus(&dialog.focus)
                                .key_context("CloudContinue")
                                .p_4()
                                .flex()
                                .flex_col()
                                .gap_3()
                                .child(
                                    div()
                                        .text_size(px(14.))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(content(1.))
                                        .child(heading),
                                )
                                .child(body)
                                .when(!working, |d| d.child(footer)),
                        ),
                )
                .into_any_element(),
        )
    }
}
