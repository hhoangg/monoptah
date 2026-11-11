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
}

pub(super) struct ContinueDialog {
    id: i64,
    title: String,
    kind: Kind,
    noun: &'static str,
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
}

impl Default for State {
    fn default() -> Self {
        Self {
            dialog: None,
            run: default_continue(),
        }
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
            "{who} comes back to this computer as the same conversation. Monoptah will wake the {noun} if it is stopped, {sync}apply the cloud code here (in place if your files are unchanged since the move, otherwise in a new worktree; your uncommitted changes are never overwritten), bring the Claude transcript back and then close the cloud copy. Nothing changes until all of that succeeded; if it fails, the conversation stays in the cloud.",
            sync = if unsynced {
                "copy the cloud turns that are not here yet, "
            } else {
                "check for newer cloud turns, "
            }
        ),
        Kind::Born => format!(
            "{who} gets a copy on this computer that replaces it in the list (it is not archived). Monoptah will wake the {noun} if it is stopped, copy the conversation, put the cloud code in a new worktree and bring the Claude transcript back. The {noun} itself keeps running."
        ),
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
                if let Some(dialog) = self.cloud_continue.dialog.as_mut() {
                    dialog.phase = Phase::Failed(error.to_string());
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
        let message = done.warnings.join(" ");
        if done.local_id == id {
            self.session_summaries.insert(id, done.summary);
            self.cloud_located.remove(&id);
            self.sync_cloud_location(cx);
            self.refresh_cloud(cx);
        } else {
            self.adopt_converted(done, window, cx);
        }
        self.error = Some(if message.is_empty() {
            "Back on this computer.".to_string()
        } else {
            format!("Back on this computer. {message}")
        });
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
                footer.child(cancel).child(go("Retry", cx)),
            ),
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
