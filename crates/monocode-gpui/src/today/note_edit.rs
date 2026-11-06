//! Editing and deleting a note in the detail panel. Edit swaps the bubble's
//! body for a field (the composer's keys: Enter or Cmd/Ctrl+Enter saves,
//! Escape cancels) and lists the attachments with a remove control; only one
//! note is open at a time. Delete asks first, in a modal, like moving a session.
use super::{Today, locked, now_ms};
use crate::{
    input::{self, Input},
    modal,
    theme::*,
};
use gpui::{
    AnyElement, Context, Div, Entity, Focusable, MouseButton, Role, Stateful, Subscription, Window,
    div, prelude::*, px,
};
use monocode_core::today::{Actor, Note, NoteAttachment};

/// The note being edited, and the one a delete is waiting on.
#[derive(Default)]
pub(crate) struct NoteEdit {
    open: Option<Editing>,
    /// The id of the note a delete confirmation is open for.
    delete: Option<String>,
}

struct Editing {
    note: String,
    body: String,
    attachments: Vec<NoteAttachment>,
    /// The `file` of each attachment the user took off.
    removed: Vec<String>,
    input: Entity<Input>,
    saving: bool,
    _subscriptions: Vec<Subscription>,
}

impl Editing {
    fn kept(&self) -> usize {
        self.attachments.len() - self.removed.len()
    }
    fn unchanged(&self, text: &str) -> bool {
        self.removed.is_empty() && text.trim() == self.body
    }
}

impl Today {
    /// Edit and Delete are offered wherever the composer is.
    pub(super) fn note_changeable(&self) -> bool {
        self.view
            .as_ref()
            .is_some_and(|v| v.mode != monocode_core::today::DayMode::Past)
    }

    /// Escape: closes the delete question, else the open editor. False when
    /// neither is open.
    pub(super) fn escape_note_edit(&mut self, cx: &mut Context<Self>) -> bool {
        if self.note_edit.delete.is_some() {
            self.cancel_delete_note(cx);
            true
        } else if self.note_edit.open.is_some() {
            self.cancel_note_edit(cx);
            true
        } else {
            false
        }
    }

    /// Forgets an edit or a delete question whose note is no longer in the
    /// timeline (deleted elsewhere), or that a read-only day cannot change.
    pub(super) fn prune_note_edit(&mut self) {
        let present = |id: &str| {
            self.activity
                .iter()
                .any(|a| matches!(a, monocode_core::today::Activity::Note(n) if n.id == id))
        };
        let keep_open = self
            .note_edit
            .open
            .as_ref()
            .is_none_or(|e| present(&e.note));
        let keep_delete = self.note_edit.delete.as_deref().is_none_or(present);
        if !self.note_changeable() || !keep_open {
            self.note_edit.open = None;
        }
        if !self.note_changeable() || !keep_delete {
            self.note_edit.delete = None;
        }
    }

    pub(super) fn start_note_edit(
        &mut self,
        note: &Note,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let input = cx.new(|cx| {
            let mut input = Input::new(cx);
            input.mono_composer("note", cx);
            input.set_label("Edit note");
            input.set_alternate_submit(true);
            input.set_text(note.body.clone(), cx);
            input
        });
        let subscriptions = vec![
            cx.subscribe(&input, |this, _, _: &input::Submitted, cx| {
                this.save_note_edit(cx)
            }),
            cx.subscribe(&input, |this, _, _: &input::AlternateSubmitted, cx| {
                this.save_note_edit(cx)
            }),
            // The save button's state follows the text.
            cx.subscribe(&input, |_, _, _: &input::Changed, cx| cx.notify()),
        ];
        window.focus(&input.focus_handle(cx), cx);
        self.note_edit.open = Some(Editing {
            note: note.id.clone(),
            body: note.body.trim().to_string(),
            attachments: note.attachments.clone(),
            removed: Vec::new(),
            input,
            saving: false,
            _subscriptions: subscriptions,
        });
        cx.notify();
    }

    pub(super) fn cancel_note_edit(&mut self, cx: &mut Context<Self>) {
        if self.note_edit.open.take().is_some() {
            cx.notify();
        }
    }

    /// The x on an attachment while editing: it is dropped when the edit is saved.
    fn remove_edit_attachment(&mut self, file: &str, cx: &mut Context<Self>) {
        if let Some(edit) = self.note_edit.open.as_mut()
            && !edit.saving
            && !edit.removed.iter().any(|f| f == file)
        {
            edit.removed.push(file.to_string());
            cx.notify();
        }
    }

    /// Whether Save is ready: something would be left, and no save runs.
    fn edit_saveable(&self, edit: &Editing, cx: &Context<Self>) -> bool {
        let text = edit.input.read(cx).content.trim().to_string();
        !edit.saving && (!text.is_empty() || edit.kept() > 0)
    }

    pub(super) fn save_note_edit(&mut self, cx: &mut Context<Self>) {
        if !self.note_changeable() {
            return;
        }
        let Some(task) = self.selected.clone() else {
            return;
        };
        let Some(edit) = self.note_edit.open.as_ref() else {
            return;
        };
        let (text, composing) = {
            let input = edit.input.read(cx);
            (input.content.to_string(), input.is_composing())
        };
        if composing || !self.edit_saveable(edit, cx) {
            return;
        }
        // Nothing changed: just close.
        if edit.unchanged(&text) {
            self.note_edit.open = None;
            cx.notify();
            return;
        }
        let note = edit.note.clone();
        let removed = edit.removed.clone();
        if let Some(edit) = self.note_edit.open.as_mut() {
            edit.saving = true;
        }
        let store = self.store.clone();
        let work = cx.background_executor().spawn(async move {
            locked(&store)?
                .today_edit_note(&task, &note, &text, &removed, &Actor::User, now_ms())
                .map(|_| ())
        });
        let edited = edit_id(&self.note_edit);
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |this, cx| {
                match &result {
                    Ok(()) => {
                        this.action_error = None;
                        // Only the edit that was saved closes.
                        if edit_id(&this.note_edit) == edited {
                            this.note_edit.open = None;
                        }
                    }
                    // The text stays for another try.
                    Err(error) => {
                        this.action_error = Some(error.clone());
                        if let Some(edit) = this.note_edit.open.as_mut() {
                            edit.saving = false;
                        }
                    }
                }
                this.refresh(cx);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn ask_delete_note(&mut self, id: &str, cx: &mut Context<Self>) {
        self.note_edit.delete = Some(id.to_string());
        cx.notify();
    }
    pub(super) fn cancel_delete_note(&mut self, cx: &mut Context<Self>) {
        if self.note_edit.delete.take().is_some() {
            cx.notify();
        }
    }
    fn confirm_delete_note(&mut self, cx: &mut Context<Self>) {
        if !self.note_changeable() {
            self.note_edit.delete = None;
            cx.notify();
            return;
        }
        let Some(note) = self.note_edit.delete.take() else {
            return;
        };
        let Some(task) = self.selected.clone() else {
            return;
        };
        // A note deleted while it is open for editing closes too.
        if edit_id(&self.note_edit).as_deref() == Some(note.as_str()) {
            self.note_edit.open = None;
        }
        self.act(
            move |store, now| store.today_delete_note(&task, &note, &Actor::User, now),
            cx,
        );
        cx.notify();
    }

    /// Edit and Delete in a note's header.
    pub(super) fn note_actions(
        &self,
        ix: usize,
        note: &Note,
        cx: &mut Context<Self>,
    ) -> Option<Div> {
        if !self.note_changeable() {
            return None;
        }
        let (edit_note, delete_id) = (note.clone(), note.id.clone());
        Some(
            div()
                .flex_shrink_0()
                .flex()
                .items_center()
                .child(
                    note_icon(format!("today-note-edit-{ix}"), "Edit note", "pencil-edit")
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.start_note_edit(&edit_note, window, cx)
                        })),
                )
                .child(
                    note_icon(format!("today-note-delete-{ix}"), "Delete note", "trash").on_click(
                        cx.listener(move |this, _, _, cx| this.ask_delete_note(&delete_id, cx)),
                    ),
                ),
        )
    }

    /// Whether `note` is the one open for editing.
    pub(super) fn editing_note(&self, note: &Note) -> bool {
        self.note_changeable() && edit_id(&self.note_edit).as_deref() == Some(note.id.as_str())
    }

    /// The editor that takes the place of a note's body and attachments.
    pub(super) fn note_edit_view(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let edit = self.note_edit.open.as_ref()?;
        let saveable = self.edit_saveable(edit, cx);
        let chips = edit
            .attachments
            .iter()
            .enumerate()
            .filter(|(_, att)| !edit.removed.contains(&att.file))
            .map(|(ix, att)| {
                let file = att.file.clone();
                div()
                    .debug_selector(move || format!("today-note-edit-attachment-{ix}"))
                    .h(px(24.))
                    .pl(px(8.))
                    .pr(px(4.))
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .rounded(px(6.))
                    .bg(content(0.08))
                    .text_color(content(0.8))
                    .child(div().max_w(px(200.)).truncate().child(att.name.clone()))
                    .child(
                        div()
                            .id(("today-note-edit-remove", ix))
                            .debug_selector(move || format!("today-note-edit-remove-{ix}"))
                            .role(Role::Button)
                            .aria_label(format!("Remove {}", att.name))
                            .size(px(18.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_full()
                            .cursor_pointer()
                            .hover(|s| s.bg(content(0.15)))
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .child(crate::theme::icon("close", 10.))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.remove_edit_attachment(&file, cx)
                            })),
                    )
            })
            .collect::<Vec<_>>();
        Some(
            div()
                .debug_selector(|| "today-note-edit".into())
                .mt(px(4.))
                .flex()
                .flex_col()
                .gap(px(6.))
                .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, _, cx| {
                    if event.keystroke.key == "escape" {
                        this.cancel_note_edit(cx);
                        cx.stop_propagation();
                    }
                }))
                .child(
                    div()
                        .debug_selector(|| "today-note-edit-field".into())
                        .px(px(8.))
                        .py(px(6.))
                        .rounded(px(6.))
                        .border_1()
                        .border_color(content(0.2))
                        .bg(content(0.04))
                        .child(edit.input.clone()),
                )
                .when(!chips.is_empty(), |d| {
                    d.child(
                        div()
                            .debug_selector(|| "today-note-edit-attachments".into())
                            .flex()
                            .flex_wrap()
                            .gap(px(4.))
                            .children(chips),
                    )
                })
                .child(
                    div()
                        .flex()
                        .justify_end()
                        .gap(px(6.))
                        .child(
                            modal::ghost_button("today-note-edit-cancel", "Cancel")
                                .debug_selector(|| "today-note-edit-cancel".into())
                                .cursor_pointer()
                                .on_click(cx.listener(|this, _, _, cx| this.cancel_note_edit(cx))),
                        )
                        .child(
                            modal::disabled(
                                modal::primary_button("today-note-edit-save", "Save")
                                    .debug_selector(|| "today-note-edit-save".into()),
                                !saveable,
                            )
                            .on_click(cx.listener(|this, _, _, cx| this.save_note_edit(cx))),
                        ),
                )
                .into_any_element(),
        )
    }

    /// "Delete this note?" over the page, while the answer is open.
    pub(super) fn note_delete_confirmation(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        self.note_edit
            .delete
            .as_ref()
            .filter(|_| self.note_changeable())?;
        let this = cx.entity();
        let card = modal::card().child(
            modal::card_body()
                .child(modal::title("Delete this note?"))
                .child(
                    modal::body("The note and its attachments will be removed for good.")
                        .mt(px(8.))
                        .debug_selector(|| "today-note-delete-text".into()),
                )
                .child(
                    modal::actions()
                        .child(
                            modal::ghost_button("today-note-delete-cancel", "Cancel")
                                .debug_selector(|| "today-note-delete-cancel".into())
                                .cursor_pointer()
                                .on_click(
                                    cx.listener(|this, _, _, cx| this.cancel_delete_note(cx)),
                                ),
                        )
                        .child(
                            modal::danger_button("today-note-delete-confirm", "Delete")
                                .debug_selector(|| "today-note-delete-confirm".into())
                                .cursor_pointer()
                                .on_click(
                                    cx.listener(|this, _, _, cx| this.confirm_delete_note(cx)),
                                ),
                        ),
                ),
        );
        Some(
            modal::overlay("today-note-delete", card, move |_, cx| {
                this.update(cx, |today, cx| today.cancel_delete_note(cx))
            })
            .into_any_element(),
        )
    }
}

fn edit_id(state: &NoteEdit) -> Option<String> {
    state.open.as_ref().map(|edit| edit.note.clone())
}

/// A small quiet icon button for a note's header.
fn note_icon(id: String, label: &'static str, path: &'static str) -> Stateful<Div> {
    let selector = id.clone();
    div()
        .id(gpui::SharedString::from(id))
        .debug_selector(move || selector.clone())
        .role(Role::Button)
        .aria_label(label)
        .size(px(20.))
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(4.))
        .cursor_pointer()
        .text_color(content(0.4))
        .hover(|s| s.bg(content(0.1)).text_color(content(1.)))
        .child(crate::theme::icon(path, 12.).text_color(content(0.5)))
}

#[cfg(test)]
mod tests {
    use super::super::list::testkit::has;
    use super::*;
    use crate::runtime::Database;
    use gpui::{TestAppContext, VisualTestContext};
    use monocode_core::today::NewTask;
    use std::{
        path::PathBuf,
        sync::{Arc, Mutex},
    };

    struct Rig {
        today: Entity<Today>,
        store: Database,
        task: String,
        dir: PathBuf,
    }
    impl Drop for Rig {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }
    fn rig(cx: &mut TestAppContext) -> (Rig, &mut VisualTestContext) {
        cx.update(crate::input::bind);
        cx.update(crate::popover::bind);
        let dir = std::env::temp_dir().join(format!("note-edit-{}", monocode_core::mono::new_id()));
        std::fs::create_dir_all(&dir).unwrap();
        let store = Arc::new(Mutex::new(
            monocode_core::store::Store::open(&dir.join("monocode.db")).unwrap(),
        ));
        let task = locked(&store)
            .unwrap()
            .today_add(
                &NewTask {
                    title: "Notes".into(),
                    ..Default::default()
                },
                &Actor::User,
                now_ms(),
            )
            .unwrap()
            .id;
        let (today, visual) = cx.add_window_view({
            let store = store.clone();
            move |_, cx| Today::new_quiet(store, cx)
        });
        today.update(visual, |today, cx| today.refresh(cx));
        visual.run_until_parked();
        today.update(visual, |today, cx| today.select(Some(task.clone()), cx));
        visual.run_until_parked();
        (
            Rig {
                today,
                store,
                task,
                dir,
            },
            visual,
        )
    }
    fn add_note(rig: &Rig, body: &str, files: &[&str]) -> Note {
        let paths: Vec<PathBuf> = files
            .iter()
            .map(|name| {
                let path = rig.dir.join(name);
                std::fs::write(&path, b"data").unwrap();
                path
            })
            .collect();
        locked(&rig.store)
            .unwrap()
            .today_note_with_files(&rig.task, body, &paths, &Actor::User, now_ms())
            .unwrap()
            .note
    }
    fn reload(rig: &Rig, visual: &mut VisualTestContext) {
        rig.today.update(visual, |today, cx| today.refresh(cx));
        visual.run_until_parked();
        visual.refresh().unwrap();
    }
    fn notes(rig: &Rig) -> Vec<Note> {
        locked(&rig.store).unwrap().today_notes(&rig.task).unwrap()
    }
    fn click(visual: &mut VisualTestContext, selector: &str) {
        visual.refresh().unwrap();
        let selector: &'static str = Box::leak(selector.to_string().into_boxed_str());
        let bounds = visual
            .debug_bounds(selector)
            .unwrap_or_else(|| panic!("{selector} is drawn"));
        visual.simulate_click(bounds.center(), Default::default());
        visual.run_until_parked();
        visual.refresh().unwrap();
    }
    fn drawn(visual: &mut VisualTestContext, selector: &str) -> bool {
        let selector: &'static str = Box::leak(selector.to_string().into_boxed_str());
        has(visual, selector)
    }
    /// The timeline index of the note: events come first.
    fn ix_of(rig: &Rig, visual: &VisualTestContext, id: &str) -> usize {
        rig.today.read_with(visual, |today, _| {
            today
                .activity
                .iter()
                .position(|a| matches!(a, monocode_core::today::Activity::Note(n) if n.id == id))
                .unwrap()
        })
    }
    fn type_into_edit(rig: &Rig, visual: &mut VisualTestContext, text: &str) {
        let input = rig.today.read_with(visual, |today, _| {
            today.note_edit.open.as_ref().unwrap().input.clone()
        });
        input.update(visual, |input, cx| input.set_text(text.into(), cx));
        visual.run_until_parked();
        visual.refresh().unwrap();
    }
    fn open_edit(rig: &Rig, visual: &mut VisualTestContext, note: &Note) {
        reload(rig, visual);
        let ix = ix_of(rig, visual, &note.id);
        click(visual, &format!("today-note-edit-{ix}"));
        assert!(drawn(visual, "today-note-edit"));
    }

    #[gpui::test]
    fn saving_an_edit_stores_the_body_and_marks_the_note_edited(cx: &mut TestAppContext) {
        let (rig, visual) = rig(cx);
        let note = add_note(&rig, "first", &[]);
        reload(&rig, visual);
        assert!(!drawn(visual, "today-note-edited"));
        open_edit(&rig, visual, &note);
        let prefilled = rig.today.read_with(visual, |today, cx| {
            today
                .note_edit
                .open
                .as_ref()
                .unwrap()
                .input
                .read(cx)
                .content
                .to_string()
        });
        assert_eq!(prefilled, "first");
        type_into_edit(&rig, visual, "second");
        click(visual, "today-note-edit-save");
        let stored = notes(&rig);
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].body, "second");
        assert!(stored[0].edited_at.is_some());
        assert!(!drawn(visual, "today-note-edit"));
        assert!(drawn(visual, "today-note-edited"));
    }

    #[gpui::test]
    fn cancelling_an_edit_leaves_the_note_alone(cx: &mut TestAppContext) {
        let (rig, visual) = rig(cx);
        let note = add_note(&rig, "first", &[]);
        open_edit(&rig, visual, &note);
        type_into_edit(&rig, visual, "changed");
        click(visual, "today-note-edit-cancel");
        assert!(!drawn(visual, "today-note-edit"));
        assert_eq!(notes(&rig)[0].body, "first");
        assert_eq!(notes(&rig)[0].edited_at, None);
        // Escape cancels too.
        click(
            visual,
            &format!("today-note-edit-{}", ix_of(&rig, visual, &note.id)),
        );
        type_into_edit(&rig, visual, "changed again");
        visual.simulate_keystrokes("escape");
        visual.refresh().unwrap();
        assert!(!drawn(visual, "today-note-edit"));
        assert_eq!(notes(&rig)[0].body, "first");
    }

    #[gpui::test]
    fn removing_an_attachment_while_editing_drops_it_on_save(cx: &mut TestAppContext) {
        let (rig, visual) = rig(cx);
        let note = add_note(&rig, "with files", &["a.txt", "b.txt"]);
        open_edit(&rig, visual, &note);
        assert!(drawn(visual, "today-note-edit-attachment-0"));
        assert!(drawn(visual, "today-note-edit-attachment-1"));
        click(visual, "today-note-edit-remove-0");
        assert!(!drawn(visual, "today-note-edit-attachment-0"));
        assert!(drawn(visual, "today-note-edit-attachment-1"));
        // Nothing is stored until Save.
        assert_eq!(notes(&rig)[0].attachments.len(), 2);
        click(visual, "today-note-edit-save");
        let stored = notes(&rig);
        let names: Vec<_> = stored[0]
            .attachments
            .iter()
            .map(|a| a.name.as_str())
            .collect();
        assert_eq!(names, ["b.txt"]);
        assert_eq!(stored[0].body, "with files");
        assert!(stored[0].edited_at.is_some());
    }

    #[gpui::test]
    fn save_is_disabled_when_nothing_would_be_left(cx: &mut TestAppContext) {
        let (rig, visual) = rig(cx);
        let note = add_note(&rig, "only text", &[]);
        open_edit(&rig, visual, &note);
        type_into_edit(&rig, visual, "   ");
        click(visual, "today-note-edit-save");
        assert!(drawn(visual, "today-note-edit"));
        assert_eq!(notes(&rig)[0].body, "only text");
        // Saving without a change just closes.
        type_into_edit(&rig, visual, "only text");
        click(visual, "today-note-edit-save");
        assert!(!drawn(visual, "today-note-edit"));
        assert_eq!(notes(&rig)[0].edited_at, None);
    }

    #[gpui::test]
    fn only_one_note_is_edited_at_a_time(cx: &mut TestAppContext) {
        let (rig, visual) = rig(cx);
        let first = add_note(&rig, "one", &[]);
        let second = add_note(&rig, "two", &[]);
        open_edit(&rig, visual, &first);
        let ix = ix_of(&rig, visual, &second.id);
        click(visual, &format!("today-note-edit-{ix}"));
        let open = rig.today.read_with(visual, |today, _| {
            today.note_edit.open.as_ref().map(|e| e.note.clone())
        });
        assert_eq!(open, Some(second.id));
    }

    #[gpui::test]
    fn deleting_asks_first_and_cancel_keeps_the_note(cx: &mut TestAppContext) {
        let (rig, visual) = rig(cx);
        let note = add_note(&rig, "keep me", &[]);
        reload(&rig, visual);
        let ix = ix_of(&rig, visual, &note.id);
        click(visual, &format!("today-note-delete-{ix}"));
        assert!(drawn(visual, "today-note-delete-text"));
        assert_eq!(notes(&rig).len(), 1);
        click(visual, "today-note-delete-cancel");
        assert!(!drawn(visual, "today-note-delete-text"));
        assert_eq!(notes(&rig).len(), 1);
        assert!(drawn(visual, "today-note-body"));
    }

    #[gpui::test]
    fn confirming_a_delete_removes_the_note_from_the_store_and_the_view(cx: &mut TestAppContext) {
        let (rig, visual) = rig(cx);
        let note = add_note(&rig, "bye", &["a.txt"]);
        reload(&rig, visual);
        let ix = ix_of(&rig, visual, &note.id);
        assert!(drawn(visual, "today-note-body"));
        click(visual, &format!("today-note-delete-{ix}"));
        click(visual, "today-note-delete-confirm");
        reload(&rig, visual);
        assert!(notes(&rig).is_empty());
        assert!(!drawn(visual, "today-note-body"));
        assert!(!drawn(visual, "today-note-delete-text"));
        assert_eq!(
            rig.today.read_with(visual, |t, _| t.action_error.clone()),
            None
        );
    }

    fn focus_page(rig: &Rig, visual: &mut VisualTestContext) {
        let focus = rig.today.read_with(visual, |today, _| today.focus.clone());
        visual.update(|window, cx| window.focus(&focus, cx));
    }
    fn closed_flag(rig: &Rig, visual: &mut VisualTestContext) -> Arc<Mutex<bool>> {
        let flag = Arc::new(Mutex::new(false));
        let seen = flag.clone();
        visual.update(|_, cx| {
            cx.subscribe(&rig.today, move |_, event: &crate::today::Event, _| {
                if matches!(event, crate::today::Event::Close) {
                    *seen.lock().unwrap() = true;
                }
            })
            .detach()
        });
        flag
    }

    #[gpui::test]
    fn escape_closes_the_delete_question_and_not_the_page(cx: &mut TestAppContext) {
        let (rig, visual) = rig(cx);
        let note = add_note(&rig, "stay", &[]);
        reload(&rig, visual);
        let closed = closed_flag(&rig, visual);
        let ix = ix_of(&rig, visual, &note.id);
        click(visual, &format!("today-note-delete-{ix}"));
        focus_page(&rig, visual);
        assert!(drawn(visual, "today-note-delete-text"));
        visual.simulate_keystrokes("escape");
        visual.refresh().unwrap();
        assert!(!drawn(visual, "today-note-delete-text"));
        assert!(!*closed.lock().unwrap());
        assert_eq!(notes(&rig).len(), 1);
    }

    #[gpui::test]
    fn escape_in_the_editor_cancels_only_the_edit(cx: &mut TestAppContext) {
        let (rig, visual) = rig(cx);
        let note = add_note(&rig, "stay", &[]);
        let closed = closed_flag(&rig, visual);
        open_edit(&rig, visual, &note);
        visual.simulate_keystrokes("escape");
        visual.refresh().unwrap();
        assert!(!drawn(visual, "today-note-edit"));
        assert!(!*closed.lock().unwrap());
        // Focus elsewhere on the page: the page chain cancels the edit first.
        open_edit(&rig, visual, &note);
        focus_page(&rig, visual);
        visual.simulate_keystrokes("escape");
        visual.refresh().unwrap();
        assert!(!drawn(visual, "today-note-edit"));
        assert!(!*closed.lock().unwrap());
    }

    #[gpui::test]
    fn a_past_day_drops_an_open_edit_and_cannot_save_or_delete(cx: &mut TestAppContext) {
        let (rig, visual) = rig(cx);
        let note = add_note(&rig, "now", &[]);
        open_edit(&rig, visual, &note);
        type_into_edit(&rig, visual, "sneaky");
        // A read-only view refuses even if the state lingers.
        rig.today.update(visual, |today, cx| {
            today.view.as_mut().unwrap().mode = monocode_core::today::DayMode::Past;
            today.save_note_edit(cx);
            today.note_edit.delete = Some(note.id.clone());
            today.confirm_delete_note(cx);
        });
        visual.run_until_parked();
        assert_eq!(notes(&rig).len(), 1);
        assert_eq!(notes(&rig)[0].body, "now");
        assert!(!drawn(visual, "today-note-edit"));
        assert!(!drawn(visual, "today-note-delete-text"));
    }

    #[gpui::test]
    fn switching_to_a_past_day_resets_an_open_edit_and_delete_question(cx: &mut TestAppContext) {
        let (rig, visual) = rig(cx);
        let note = add_note(&rig, "now", &[]);
        open_edit(&rig, visual, &note);
        rig.today.update(visual, |today, cx| {
            today.note_edit.delete = Some(note.id.clone());
            today.show_day(Some("2000-01-01".into()), cx);
        });
        visual.run_until_parked();
        rig.today.read_with(visual, |today, _| {
            assert!(today.note_edit.open.is_none() && today.note_edit.delete.is_none());
        });
    }

    #[gpui::test]
    fn a_note_that_vanished_drops_its_edit_and_delete_state(cx: &mut TestAppContext) {
        let (rig, visual) = rig(cx);
        let note = add_note(&rig, "soon gone", &[]);
        open_edit(&rig, visual, &note);
        rig.today.update(visual, |today, _| {
            today.note_edit.delete = Some(note.id.clone())
        });
        locked(&rig.store)
            .unwrap()
            .today_delete_note(&rig.task, &note.id, &Actor::User, now_ms())
            .unwrap();
        reload(&rig, visual);
        rig.today.read_with(visual, |today, _| {
            assert!(today.note_edit.open.is_none() && today.note_edit.delete.is_none());
        });
        assert!(!drawn(visual, "today-note-edit"));
    }

    #[gpui::test]
    fn a_failed_save_keeps_the_editor_and_its_text_and_reports(cx: &mut TestAppContext) {
        let (rig, visual) = rig(cx);
        let note = add_note(&rig, "short", &[]);
        open_edit(&rig, visual, &note);
        let long = "x".repeat(6000);
        type_into_edit(&rig, visual, &long);
        click(visual, "today-note-edit-save");
        assert!(drawn(visual, "today-note-edit"));
        let (text, error, saving) = rig.today.read_with(visual, |today, cx| {
            let edit = today.note_edit.open.as_ref().unwrap();
            (
                edit.input.read(cx).content.to_string(),
                today.action_error.clone(),
                edit.saving,
            )
        });
        assert_eq!(text, long);
        assert!(error.is_some());
        assert!(!saving);
        assert_eq!(notes(&rig)[0].body, "short");
    }

    #[gpui::test]
    fn switching_task_resets_an_open_edit_and_delete_question(cx: &mut TestAppContext) {
        let (rig, visual) = rig(cx);
        let note = add_note(&rig, "mine", &[]);
        open_edit(&rig, visual, &note);
        rig.today.update(visual, |today, _| {
            today.note_edit.delete = Some(note.id.clone())
        });
        rig.today.update(visual, |today, cx| today.select(None, cx));
        visual.run_until_parked();
        rig.today.read_with(visual, |today, _| {
            assert!(today.note_edit.open.is_none() && today.note_edit.delete.is_none());
        });
    }
}
