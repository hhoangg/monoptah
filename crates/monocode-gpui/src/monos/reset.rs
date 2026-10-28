//! Resetting a Mono's conversation (ConfirmReset.tsx): the dialog, deleting the
//! old conversation row, and the optional cleanup of the provider's session files.
use super::*;
use gpui::{FocusHandle, FontWeight};
use monocode_core::{
    mono::provider_files::{self, MonoProviderFiles, ResetRead},
    provider_cleanup::{self, CleanupRequest},
};
use std::{path::Path, sync::Arc};

pub(super) const BODY: &str = "All messages in this Mono's conversation will be deleted and any active reply will be stopped. This can't be undone.";
pub(super) const KEPT: &str = "Its soul, memory and habits will be kept.";

pub(super) fn title(name: &str) -> String {
    format!("Reset {name}'s conversation?")
}
pub(super) fn option_label(files: &MonoProviderFiles) -> String {
    format!(
        "Also delete the {} session files on this computer",
        files.label
    )
}
pub(super) fn option_note(files: &MonoProviderFiles) -> String {
    match (files.supported, &files.note) {
        (true, Some(note)) => format!("They can't be recovered. {note}"),
        (true, None) => "They can't be recovered.".into(),
        (false, note) => note.clone().unwrap_or_default(),
    }
}
pub(super) const STOP_TIMEOUT: &str = "The reply did not stop in time. Try again.";
const STOP_POLL: Duration = Duration::from_millis(50);
const STOP_WAIT: Duration = Duration::from_secs(10);
pub(super) fn failure(error: &str) -> String {
    format!("Could not reset the conversation. {error}")
}

/// Removes provider session files once the row is gone; `None` means the store
/// has no data directory (in memory), where there is nothing to find.
pub(super) type CleanupRunner =
    Arc<dyn Fn(Option<&Path>, &[CleanupRequest]) -> Vec<String> + Send + Sync>;
pub(super) fn default_cleanup() -> CleanupRunner {
    Arc::new(|data_dir, requests| match data_dir {
        // Tests inject their own runner; the real homes are never touched.
        _ if cfg!(test) => vec!["cleanup is off in tests".into()],
        Some(dir) => provider_cleanup::run_cleanup_report(dir, requests, true),
        None => vec!["no data directory, so nothing was removed".into()],
    })
}

pub(super) struct ResetDialog {
    pub(super) mono: String,
    name: String,
    /// What the box offers; `None` when the Mono has no saved conversation.
    pub(super) files: Option<MonoProviderFiles>,
    pub(super) delete_files: bool,
    pub(super) busy: bool,
    pub(super) error: Option<String>,
    focus: FocusHandle,
}

impl Monos {
    /// Reads what a reset could also delete, off the main thread, then asks.
    pub(super) fn request_reset(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.reset.is_some() {
            return;
        }
        let Some(mono) = self.roster.get(id) else {
            return;
        };
        let (name, session) = (mono.label(), mono.session_id);
        let store = self.store.clone();
        let read = cx.background_executor().spawn(async move {
            match session {
                Some(session) => {
                    let db = locked(&store)?;
                    provider_files::read_reset(&db, session).map(|r| r.files)
                }
                None => Ok(None),
            }
        });
        let mono = id.to_string();
        cx.spawn_in(window, async move |this, cx| {
            let files = read.await;
            let _ = this.update_in(cx, |this, window, cx| {
                if this.reset.is_some() {
                    return;
                }
                let files = match files {
                    Ok(files) => files,
                    Err(error) => {
                        this.error = Some(error);
                        cx.notify();
                        return;
                    }
                };
                let focus = cx.focus_handle();
                window.focus(&focus, cx);
                this.reset = Some(ResetDialog {
                    mono,
                    name,
                    delete_files: files.as_ref().is_some_and(|f| f.supported),
                    files,
                    busy: false,
                    error: None,
                    focus,
                });
                cx.notify();
            });
        })
        .detach();
    }
    /// True while a reset of this Mono's conversation is deleting it.
    pub(super) fn resetting(&self, id: &str) -> bool {
        self.reset.as_ref().is_some_and(|d| d.busy && d.mono == id)
    }
    /// Dismisses the dialog; nothing can be abandoned while the reset runs.
    pub(super) fn close_reset(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.reset.as_ref().is_some_and(|d| d.busy) {
            return;
        }
        if self.reset.take().is_some() {
            self.focus_active(window, cx);
            cx.notify();
        }
    }
    /// Stops the chat, deletes the old conversation row (reading what to clean
    /// up first), and only then starts a fresh conversation. A failed deletion
    /// keeps the original conversation and reports in the dialog.
    pub(super) fn confirm_reset(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(dialog) = self.reset.as_mut().filter(|d| !d.busy) else {
            return;
        };
        dialog.busy = true;
        dialog.error = None;
        let id = dialog.mono.clone();
        let delete_files =
            dialog.delete_files && dialog.files.as_ref().is_some_and(|f| f.supported);
        let label = dialog.files.as_ref().map(|f| f.label.clone());
        if let Some(view) = self.chats.get(&id) {
            view.update(cx, |v, cx| v.mono_stop_turn(cx));
        }
        let old = self.roster.get(&id).and_then(|m| m.session_id);
        let view = self.chats.get(&id).cloned();
        let store = self.store.clone();
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            // A stopped turn may still be saving; its rows must not land in a
            // conversation that reuses this id.
            let mut waited = Duration::ZERO;
            while view
                .as_ref()
                .is_some_and(|v| !v.read_with(cx, |v, _| v.mono_settled()))
            {
                if waited >= STOP_WAIT {
                    let _ = this.update(cx, |this, cx| this.reset_failed(&id, STOP_TIMEOUT, cx));
                    return;
                }
                cx.background_executor().timer(STOP_POLL).await;
                waited += STOP_POLL;
            }
            let result = cx
                .background_executor()
                .spawn(async move {
                    let Some(old) = old else {
                        return Ok((None, None));
                    };
                    let mut db = locked(&store)?;
                    let read = delete_files
                        .then(|| provider_files::read_reset(&db, old))
                        .transpose()?;
                    db.delete(old)?;
                    // Drop the conversation's recorded turns and their Git refs.
                    if let Some(checkpoints) = monocode_core::checkpoint::store() {
                        let _ = checkpoints.exclusive(|c| c.forget(&old.to_string()));
                    }
                    Ok::<_, String>((read, db.data_dir().map(Path::to_path_buf)))
                })
                .await;
            let _ = this.update_in(cx, |this, window, cx| match result {
                Ok(deleted) => this.finish_reset(&id, old, deleted, label, window, cx),
                Err(error) => this.reset_failed(&id, &error, cx),
            });
        })
        .detach();
    }
    /// Keeps the original conversation and says why in the dialog. What the
    /// reset held back is delivered to that conversation now.
    fn reset_failed(&mut self, id: &str, error: &str, cx: &mut Context<Self>) {
        if let Some(dialog) = &mut self.reset {
            dialog.busy = false;
            dialog.error = Some(failure(error));
        }
        self.dispatch(id, cx);
        cx.notify();
    }
    fn finish_reset(
        &mut self,
        id: &str,
        old: Option<i64>,
        (read, data_dir): (Option<ResetRead>, Option<std::path::PathBuf>),
        label: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.reset = None;
        // Messages the old conversation already took are not sent again; the
        // rest, and any reports, wait for the fresh conversation.
        let delivered = self
            .chats
            .get(id)
            .map(|v| v.read(cx).mono_delivered().clone())
            .unwrap_or_default();
        self.chats.remove(id);
        if let Some(old) = old {
            self.roster.hidden_sessions.retain(|&s| s != old);
        }
        if let Some(mono) = self.roster.get_mut(id) {
            mono.session_id = None;
            mono.unavailable_session = None;
            mono.seen_rows = 0;
            mono.outbox.retain(|m| !delivered.contains(&m.id));
        }
        self.save(cx);
        self.ensure_chat(id, cx);
        self.focus_active(window, cx);
        let Some(read) = read else {
            return;
        };
        let label = read.files.map(|f| f.label).or(label).unwrap_or_default();
        let mut problems = read.problems;
        if read.requests.is_empty() {
            self.reset_notice(&label, problems.len(), cx);
            return;
        }
        // The row is gone; slow disk work must not hold the window.
        let cleanup = self.cleanup.clone();
        let work = cx.background_executor().spawn(async move {
            problems.extend(cleanup(data_dir.as_deref(), &read.requests));
            problems
        });
        cx.spawn(async move |this, cx| {
            let problems = work.await;
            let _ = this.update(cx, |this, cx| this.reset_notice(&label, problems.len(), cx));
        })
        .detach();
    }
    /// Trouble deleting files never undoes the reset; it only says so.
    fn reset_notice(&mut self, label: &str, problems: usize, cx: &mut Context<Self>) {
        if problems > 0 {
            self.error = Some(provider_files::problem_message(label, problems));
            self.changed(cx);
        }
    }

    pub(super) fn reset_modal(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let dialog = self.reset.as_ref()?;
        let size = window.viewport_size();
        let width = 400f32.min(size.width.as_f32() - 24.);
        let weak = cx.weak_entity();
        let busy = dialog.busy;
        let option = dialog.files.as_ref().map(|files| {
            let (label, note) = (option_label(files), option_note(files));
            let (checked, disabled) = (dialog.delete_files, !files.supported || busy);
            div()
                .id("mono-reset-files")
                .debug_selector(|| "mono-reset-files".into())
                .role(Role::CheckBox)
                .aria_label(label.clone())
                .aria_toggled(if checked {
                    gpui::Toggled::True
                } else {
                    gpui::Toggled::False
                })
                .when(!disabled, |d| {
                    d.cursor_pointer().on_click(cx.listener(|this, _, _, cx| {
                        if let Some(d) = &mut this.reset {
                            d.delete_files = !d.delete_files;
                            cx.notify();
                        }
                    }))
                })
                .flex()
                .items_start()
                .gap_2()
                .child(
                    div()
                        .mt(px(2.))
                        .size(px(14.))
                        .flex_shrink_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(4.))
                        .border_1()
                        .border_color(if checked {
                            accent_color()
                        } else {
                            content(0.3)
                        })
                        .when(checked, |d| {
                            d.bg(accent_color())
                                .child(icon("check", 10.).text_color(gpui::white()))
                        })
                        .when(disabled, |d| d.opacity(0.5)),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_size(px(12.))
                        .line_height(px(17.))
                        .text_color(content(if files.supported { 0.85 } else { 0.5 }))
                        .child(label)
                        .when(!note.is_empty(), |d| {
                            d.child(
                                div()
                                    .mt(px(4.))
                                    .text_size(px(11.))
                                    .line_height(px(15.))
                                    .text_color(content(0.45))
                                    .child(note),
                            )
                        }),
                )
        });
        Some(
            div()
                .absolute()
                .size_full()
                .child(
                    gpui::deferred(
                        gpui::anchored()
                            .position(gpui::point(px(0.), px(0.)))
                            .child(
                                div()
                                    .w(size.width)
                                    .h(size.height)
                                    .bg(crate::modal::scrim_color())
                                    .occlude(),
                            ),
                    )
                    .with_priority(79),
                )
                .child(
                    popover("mono-reset-modal", Anchor::Center)
                        .modal_surface(true)
                        .width(width)
                        .gap(0.)
                        .priority(80)
                        .on_dismiss(move |window, cx| {
                            let _ = weak.update(cx, |this, cx| this.close_reset(window, cx));
                        })
                        .child(
                            div()
                                .id("mono-reset-dialog")
                                .debug_selector(|| "mono-reset-dialog".into())
                                .role(Role::Dialog)
                                .aria_label(title(&dialog.name))
                                .track_focus(&dialog.focus)
                                .key_context("MonoReset")
                                .on_key_down(cx.listener(
                                    |this, event: &gpui::KeyDownEvent, window, cx| {
                                        if event.keystroke.key == "enter" {
                                            this.confirm_reset(window, cx);
                                            cx.stop_propagation();
                                        }
                                    },
                                ))
                                .p_4()
                                .flex()
                                .flex_col()
                                .gap_3()
                                .child(
                                    div()
                                        .text_size(px(14.))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(content(1.))
                                        .child(title(&dialog.name)),
                                )
                                .child(
                                    div()
                                        .text_size(px(13.))
                                        .line_height(px(19.))
                                        .text_color(content(0.7))
                                        .child(BODY),
                                )
                                .child(
                                    div()
                                        .text_size(px(12.))
                                        .line_height(px(19.))
                                        .text_color(content(0.5))
                                        .child(KEPT),
                                )
                                .children(option)
                                .children(dialog.error.clone().map(|error| {
                                    div()
                                        .id("mono-reset-error")
                                        .role(Role::Alert)
                                        .text_size(px(12.))
                                        .line_height(px(19.))
                                        .text_color(rgb(0xff6467))
                                        .child(error)
                                }))
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .justify_end()
                                        .gap_2()
                                        .child(
                                            crate::modal::disabled(
                                                crate::modal::ghost_button(
                                                    "mono-reset-cancel",
                                                    "Cancel",
                                                )
                                                .debug_selector(|| "mono-reset-cancel".into()),
                                                busy,
                                            )
                                            .when(
                                                !busy,
                                                |d| {
                                                    d.on_click(cx.listener(
                                                        |this, _, window, cx| {
                                                            this.close_reset(window, cx)
                                                        },
                                                    ))
                                                },
                                            ),
                                        )
                                        .child(
                                            crate::modal::disabled(
                                                crate::modal::danger_button(
                                                    "mono-reset-confirm",
                                                    if busy {
                                                        "Resetting…"
                                                    } else {
                                                        "Reset conversation"
                                                    },
                                                )
                                                .debug_selector(|| "mono-reset-confirm".into()),
                                                busy,
                                            )
                                            .on_click(
                                                cx.listener(|this, _, window, cx| {
                                                    this.confirm_reset(window, cx)
                                                }),
                                            ),
                                        ),
                                ),
                        ),
                )
                .into_any_element(),
        )
    }
}
