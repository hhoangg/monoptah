//! The compare dialog of a remote save the Host refused because the file changed on the
//! cloud workspace after the editor loaded it ("File changed on the host").
//!
//! The user's text stays in the editor the whole time. Nothing is overwritten or reloaded
//! until one of the buttons is pressed: Keep mine saves over the cloud version (with the
//! cloud version's hash), Take cloud replaces the editor text, Cancel closes the dialog.
use super::*;
use crate::modal;
use monocode_core::files::document::Document;

/// How the file looks on the cloud workspace now.
pub(super) enum Cloud {
    Text(Box<TextDocument>),
    /// The file is gone or cannot be shown as text; only Keep mine applies.
    Unavailable(String),
}

pub(super) struct Conflict {
    /// The editor text being saved.
    pub(super) mine: String,
    pub(super) cloud: Cloud,
    /// Keep mine is in flight.
    pub(super) working: bool,
}

type SaveFn =
    dyn Fn(&std::path::Path, &TextDocument, &str) -> Result<Vec<u8>, String> + Send + Sync;
type ReadFn = dyn Fn(&std::path::Path) -> Result<Document, String> + Send + Sync;

/// What the editor does to the file. Tests stand in a fake Host here.
#[derive(Clone)]
pub(super) struct FileIo {
    pub(super) save: Arc<SaveFn>,
    pub(super) overwrite: Arc<SaveFn>,
    pub(super) read: Arc<ReadFn>,
}
impl Default for FileIo {
    fn default() -> Self {
        Self {
            save: Arc::new(document::save_document),
            overwrite: Arc::new(document::overwrite_document),
            read: Arc::new(document::read_document),
        }
    }
}

/// First lines of `text`, so a big file does not build a huge dialog.
fn excerpt(text: &str) -> String {
    const LINES: usize = 40;
    let mut out: Vec<&str> = text.lines().take(LINES + 1).collect();
    let more = out.len() > LINES;
    out.truncate(LINES);
    let mut out = out.join("\n");
    if more {
        out.push_str("\n...");
    }
    out
}

impl FileEditor {
    /// The Host refused the save: look at the cloud version and ask what to do.
    pub(super) fn open_conflict(&mut self, mine: String, cx: &mut Context<Self>) {
        let path = PathBuf::from(&self.path);
        let read = self.io.read.clone();
        let work = cx.background_executor().spawn(async move { read(&path) });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |this, cx| {
                let cloud = match result {
                    Ok(Document::Text(doc)) => Cloud::Text(Box::new(doc)),
                    Ok(Document::Unsupported(reason)) => Cloud::Unavailable(reason),
                    Ok(_) => Cloud::Unavailable("The cloud version is not text.".into()),
                    Err(error) => Cloud::Unavailable(error),
                };
                this.show_conflict(mine, cloud, cx);
            });
        })
        .detach();
    }

    pub(super) fn show_conflict(&mut self, mine: String, cloud: Cloud, cx: &mut Context<Self>) {
        self.autosave = None;
        self.conflict = Some(Conflict {
            mine,
            cloud,
            working: false,
        });
        // The text being compared must not change under the dialog.
        self.input.update(cx, |input, _| input.set_read_only(true));
        cx.emit(FileEvent::SaveFailed);
        cx.notify();
    }

    pub(super) fn close_conflict(&mut self, cx: &mut Context<Self>) {
        if self.conflict.take().is_some() {
            self.sync_read_only(cx);
            self.schedule_autosave(cx);
            cx.notify();
        }
    }

    /// Cancel: the editor keeps the user's text and stays dirty.
    pub(super) fn cancel_conflict(&mut self, cx: &mut Context<Self>) {
        if self.conflict.as_ref().is_some_and(|c| !c.working) {
            self.close_conflict(cx);
        }
    }

    /// Keep mine: save over the cloud version with its current hash.
    pub(super) fn keep_mine(&mut self, cx: &mut Context<Self>) {
        let Some(conflict) = &mut self.conflict else {
            return;
        };
        if conflict.working {
            return;
        }
        let Some(doc) = self.document.clone() else {
            return;
        };
        conflict.working = true;
        let mine = conflict.mine.clone();
        let path = PathBuf::from(&self.path);
        let overwrite = self.io.overwrite.clone();
        self.saving = true;
        let text = mine.clone();
        let work = cx
            .background_executor()
            .spawn(async move { overwrite(&path, &doc, &text) });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |this, cx| {
                this.conflict = None;
                this.sync_read_only(cx);
                this.finish_save(result, mine, cx);
            });
        })
        .detach();
        cx.notify();
    }

    /// Take cloud: the editor shows the cloud text and the local edits are dropped.
    pub(super) fn take_cloud(&mut self, cx: &mut Context<Self>) {
        let Some(Conflict {
            cloud: Cloud::Text(_),
            working: false,
            ..
        }) = &self.conflict
        else {
            return;
        };
        let Some(Conflict {
            cloud: Cloud::Text(doc),
            ..
        }) = self.conflict.take()
        else {
            return;
        };
        let text = doc.text.clone();
        self.error = None;
        self.disk_changed = false;
        self.document = Some(*doc);
        self.input
            .update(cx, |input, cx| input.reload_file(text, false, cx));
        self.dirty = false;
        self.sync_read_only(cx);
        self.sync_blame_options(cx);
        self.request_blame(cx);
        self.update_preview(cx);
        self.schedule_analysis(cx);
        self.git.update(cx, |git, cx| git.changed(cx));
        cx.emit(FileEvent::Dirty);
        cx.notify();
    }

    /// The dialog, when a save is in conflict.
    pub(super) fn conflict_dialog(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let conflict = self.conflict.as_ref()?;
        let can_take = matches!(conflict.cloud, Cloud::Text(_)) && !conflict.working;
        let cloud_text = match &conflict.cloud {
            Cloud::Text(doc) => excerpt(&doc.text),
            Cloud::Unavailable(reason) => reason.clone(),
        };
        let column = |id: &'static str, label: &'static str, text: String| {
            div()
                .id(id)
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap_1()
                .child(
                    div()
                        .text_size(px(11.))
                        .text_color(content(0.5))
                        .child(label),
                )
                .child(
                    div()
                        .id((id, 1usize))
                        .h(px(220.))
                        .overflow_y_scroll()
                        .p_2()
                        .rounded_md()
                        .bg(content(0.06))
                        .text_size(px(11.))
                        .line_height(px(15.))
                        .font_family("Menlo")
                        .child(text),
                )
        };
        let this = cx.entity();
        let card = modal::card()
            .w(px(720.))
            .child(
                modal::card_body()
                    .gap_3()
                    .child(modal::title("This file changed on the cloud workspace"))
                    .child(modal::body(
                        "Someone changed it after you opened it. Nothing was saved. Your text is still in the editor.",
                    ))
                    .when(self.agent_working(), |d| {
                        d.child(
                            div()
                                .id("conflict-agent-working")
                                .debug_selector(|| "conflict-agent-working".into())
                                .text_size(px(12.))
                                .text_color(rgb(0xfbbf24))
                                .child(
                                    "The agent is working in this workspace and may still be editing this file.",
                                ),
                        )
                    })
                    .child(
                        div()
                            .flex()
                            .gap_3()
                            .child(column("conflict-mine", "Mine", excerpt(&conflict.mine)))
                            .child(column("conflict-cloud", "Cloud", cloud_text)),
                    )
                    .child(
                        modal::actions()
                            .child(
                                modal::ghost_button("conflict-cancel", "Cancel")
                                    .cursor_pointer()
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.cancel_conflict(cx)
                                    })),
                            )
                            .child(
                                modal::disabled(
                                    modal::ghost_button("conflict-take-cloud", "Take cloud"),
                                    !can_take,
                                )
                                .on_click(cx.listener(|this, _, _, cx| this.take_cloud(cx))),
                            )
                            .child(
                                modal::disabled(
                                    modal::danger_button("conflict-keep-mine", "Keep mine")
                                        .debug_selector(|| "conflict-keep-mine".into()),
                                    conflict.working,
                                )
                                .on_click(cx.listener(|this, _, _, cx| this.keep_mine(cx))),
                            ),
                    ),
            );
        Some(
            modal::overlay("file-conflict", card, move |_, cx| {
                this.update(cx, |editor, cx| editor.cancel_conflict(cx))
            })
            .into_any_element(),
        )
    }
}
