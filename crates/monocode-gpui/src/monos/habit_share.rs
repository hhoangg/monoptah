//! Sharing a habit as a file: the export on a habit's page, the import page, and the review an
//! imported habit needs before it can be enabled or run. The pages are built from the same
//! pieces as the workflow ones (`workflows_ui`): the file box and its debounced preview, the
//! picker, the save dialog, the review sections and the header buttons.
use super::workflows_ui::{
    actions::slug_file_name,
    apply_picked_file,
    page::{Saved, basename},
    pick_text_file, preview_later,
    review::{labelled, verbatim},
    view::{alert, choose_file_link, header_button, hint, property, section},
};
use super::*;
use monocode_core::mono::habit_yaml::{
    HabitPreview, export_habit, import_habit, preview_habit_import, warnings_for,
};
use monocode_core::mono::workflows::yaml::visible;
use monocode_core::time::now_ms;

const TEXT_KEY: &str = "habit-import-text";
const PLACEHOLDER: &str = "Or paste the text of a habit file here.";
const CHANGED: &str =
    "This habit changed while you were reading it. Read it again before approving.";

/// The file name and the text of a habit's export.
pub(super) fn export_parts(h: &Habit) -> (String, String) {
    (
        slug_file_name(&h.name, "habit", ".monoptah-habit.yaml"),
        export_habit(h),
    )
}

fn refusal(text: &str) -> String {
    preview_habit_import(text)
        .err()
        .unwrap_or_else(|| "Habit file is too large".into())
}

#[derive(Default)]
pub(super) struct HabitShare {
    /// What an export said, under the habit page's header.
    pub(super) notice: Option<String>,
    pub(super) import: HabitImport,
    pub(super) review: Option<HabitReview>,
}

#[derive(Default)]
pub(super) struct HabitImport {
    /// Counts the edits of the text; a preview belongs to one of them.
    pub(super) revision: u64,
    pub(super) shown: u64,
    pub(super) read: Option<Result<HabitPreview, String>>,
    pub(super) file_error: Option<String>,
    /// The next change of the text is the page emptying it after a refused file.
    pub(super) keep_file_error: bool,
    pub(super) saving: bool,
    pub(super) error: Option<String>,
    /// How long typing pauses before the preview; tests shorten it.
    pub(super) debounce: Option<Duration>,
}

/// What the review screen shows and what approving applies to.
pub(super) struct HabitReview {
    pub(super) id: String,
    /// What is on screen is what approving applies to.
    pub(super) shown: Habit,
    /// The fingerprint of `shown`, captured when it was put on screen.
    pub(super) fingerprint: String,
    pub(super) notice: Option<String>,
    pub(super) busy: bool,
}

impl Monos {
    /// Copies the habit as a file to the clipboard and offers to save it.
    pub(super) fn habit_export(&mut self, h: &Habit, cx: &mut Context<Self>) {
        let (file_name, text) = export_parts(h);
        cx.write_to_clipboard(gpui::ClipboardItem::new_string(text.clone()));
        self.habit_share.notice = Some("Copied to the clipboard.".into());
        cx.notify();
        self.save_text_file(
            file_name,
            text,
            "habit",
            |this, saved, cx| {
                this.habit_share.notice = Some(match saved {
                    Saved::To(path) => format!(
                        "Copied to the clipboard and saved as {}.",
                        basename(&path.to_string_lossy())
                    ),
                    Saved::Cancelled => "Copied to the clipboard.".into(),
                    Saved::Failed => {
                        "Copied to the clipboard. Could not save the habit file.".into()
                    }
                });
                cx.notify();
            },
            cx,
        );
    }

    /// Opens the import page with nothing in it.
    pub(super) fn open_habit_import(&mut self, cx: &mut Context<Self>) {
        self.clear_habit_fields();
        let debounce = self.habit_share.import.debounce;
        self.habit_share.import = HabitImport {
            debounce,
            ..Default::default()
        };
        self.route = Some(Route::ImportHabit);
        cx.notify();
    }

    fn habit_import_field(&mut self, cx: &mut Context<Self>) -> Entity<Input> {
        if let Some(field) = self.fields.get(TEXT_KEY) {
            return field.clone();
        }
        let field = cx.new(|cx| Input::multiline_field("Habit file text", PLACEHOLDER, cx));
        self.observations
            .push(cx.subscribe(&field, |this, _, _: &input::Changed, cx| {
                this.habit_import_changed(cx)
            }));
        self.fields.insert(TEXT_KEY.into(), field.clone());
        field
    }

    /// The text changed: the old preview and errors no longer describe it. The new preview is made
    /// once typing pauses, off the UI thread.
    fn habit_import_changed(&mut self, cx: &mut Context<Self>) {
        let text = self.value(TEXT_KEY, cx);
        let imp = &mut self.habit_share.import;
        imp.revision += 1;
        if !std::mem::take(&mut imp.keep_file_error) {
            imp.file_error = None;
        }
        imp.error = None;
        let revision = imp.revision;
        if text.trim().is_empty() {
            imp.read = None;
            imp.shown = revision;
            cx.notify();
            return;
        }
        let pause = imp.debounce;
        cx.notify();
        preview_later(
            pause,
            revision,
            |this| this.habit_share.import.revision,
            move || preview_habit_import(&text),
            move |this, read, cx| {
                this.habit_share.import.read = Some(read);
                this.habit_share.import.shown = revision;
                cx.notify();
            },
            cx,
        );
    }

    fn habit_import_choose_file(&mut self, cx: &mut Context<Self>) {
        pick_text_file(
            "Import habit",
            refusal,
            |this, read, cx| {
                let field = this.habit_import_field(cx);
                let imp = &mut this.habit_share.import;
                apply_picked_file(
                    &field,
                    read,
                    &mut imp.file_error,
                    &mut imp.keep_file_error,
                    cx,
                );
                cx.notify();
            },
            cx,
        );
    }

    /// What a user reads before a habit may run: its name, when it runs, what it tells the Mono
    /// to do (all of it, hidden characters made visible) and what is worth a second look. The
    /// import page and the review both render this, so they cannot drift.
    fn habit_sections(
        name: &str,
        schedule: String,
        instructions: &str,
        warnings: &[String],
    ) -> Vec<AnyElement> {
        let mut out = vec![
            div()
                .w_full()
                .min_w_0()
                .flex_shrink_0()
                .px_4()
                .py_3()
                .flex()
                .flex_col()
                .gap_0p5()
                .child(property("Name", visible(name)))
                .child(property("Runs", schedule))
                .into_any_element(),
        ];
        if !warnings.is_empty() {
            out.push(
                section("Worth a second look", None)
                    .id("mono-habit-warnings")
                    .debug_selector(|| "mono-habit-warnings".into())
                    .child(
                        div()
                            .px_2()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .children(warnings.iter().map(|warning| {
                                div()
                                    .text_size(px(12.))
                                    .line_height(px(20.))
                                    .text_color(content(0.75))
                                    .child(visible(warning))
                            })),
                    )
                    .into_any_element(),
            );
        }
        out.push(
            section("What it does", None)
                .child(labelled(
                    "The Mono is told this each time the habit runs",
                    verbatim(instructions)
                        .id("mono-habit-shown-instructions")
                        .debug_selector(|| "mono-habit-shown-instructions".into()),
                ))
                .into_any_element(),
        );
        out
    }

    pub(super) fn import_habit_page(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let text = self.habit_import_field(cx);
        let imp = &self.habit_share.import;
        let read = imp.read.clone();
        let preview = read.as_ref().and_then(|r| r.as_ref().ok()).cloned();
        let empty = text.read(cx).content.trim().is_empty();
        // The preview must be of the text in the box, not of what it said a moment ago.
        let ready = preview.is_some() && imp.shown == imp.revision && !imp.saving;
        let (file_error, save_error) = (imp.file_error.clone(), imp.error.clone());
        let body = match &preview {
            Some(p) => {
                let mut parts =
                    Self::habit_sections(&p.name, p.schedule.label(), &p.instructions, &p.warnings);
                parts.push(
                    div()
                        .id("mono-habit-import-status")
                        .debug_selector(|| "mono-habit-import-status".into())
                        .w_full()
                        .min_w_0()
                        .flex_shrink_0()
                        .border_t_1()
                        .border_color(content(0.1))
                        .px_2()
                        .pb_4()
                        .children(save_error.map(alert))
                        .child(hint(
                            "It is saved off. You read it again and approve it before it can run.",
                        ))
                        .into_any_element(),
                );
                parts
            }
            None => Vec::new(),
        };
        let choose = choose_file_link("mono-habit-import-choose-file")
            .on_click(cx.listener(|this, _, _, cx| this.habit_import_choose_file(cx)))
            .into_any_element();
        div()
            .size_full()
            .min_w_0()
            .min_h_0()
            .overflow_hidden()
            .flex()
            .flex_col()
            .child(
                self.page_header("Import habit", cx).child(
                    header_button("mono-habit-import-save", "Import", true, ready).on_click(
                        cx.listener(move |this, _, _, cx| {
                            if ready {
                                this.import_habit_save(cx);
                            }
                        }),
                    ),
                ),
            )
            .child(
                div()
                    .id("mono-habit-import-page")
                    .debug_selector(|| "mono-habit-import-page".into())
                    .w_full()
                    .min_w_0()
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .flex()
                    .flex_col()
                    .child(
                        section("Habit file", Some(choose))
                            .child(
                                div()
                                    .id("mono-habit-import-text")
                                    .debug_selector(|| "mono-habit-import-text".into())
                                    .child(super::scheduler::habit_field(text)),
                            )
                            .children(file_error.clone().map(alert))
                            .children(match &read {
                                Some(Err(error)) => Some(alert(error.clone())),
                                _ => None,
                            })
                            .when(empty && file_error.is_none(), |d| {
                                d.child(hint(
                                    "Nothing is saved until you import, and an imported habit starts off.",
                                ))
                            }),
                    )
                    .children(preview.is_some().then(|| {
                        div()
                            .id("mono-habit-import-preview")
                            .debug_selector(|| "mono-habit-import-preview".into())
                            .w_full()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .children(body)
                    })),
            )
            .into_any_element()
    }

    /// Saves the previewed file as a new habit: off, and flagged for review. The limit of habits
    /// is checked over what is on disk.
    fn import_habit_save(&mut self, cx: &mut Context<Self>) {
        let Some(Ok(preview)) = self.habit_share.import.read.clone() else {
            return;
        };
        let Some(mono) = self.active.clone() else {
            return;
        };
        let imp = &mut self.habit_share.import;
        if imp.saving {
            return;
        }
        imp.saving = true;
        imp.error = None;
        let root = self.root.clone();
        let target = mono.clone();
        let work = cx.background_executor().spawn(async move {
            let habit = import_habit(&preview, now_ms())?;
            habits::update(&root, &target, |all| {
                all.push(habit.clone());
                Ok(())
            })?;
            Ok::<_, String>(habit)
        });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |this, cx| {
                this.habit_share.import.saving = false;
                match result {
                    Ok(habit) => {
                        if this.active.as_deref() == Some(&mono) {
                            if !this.habits.iter().any(|h| h.id == habit.id) {
                                this.habits.push(habit.clone());
                            }
                            // The import page is replaced by the new habit's page.
                            if this.route == Some(Route::ImportHabit) {
                                this.clear_habit_fields();
                                this.route = Some(Route::Habit(habit.id.clone()));
                            }
                        }
                        this.reload_files(&mono, cx);
                        this.reload_today_habits(cx);
                    }
                    Err(error) => {
                        eprintln!("Could not import the habit: {error}");
                        this.habit_share.import.error = Some(if error.is_empty() {
                            "Could not save the habit.".into()
                        } else {
                            error
                        });
                    }
                }
                this.changed(cx);
            });
        })
        .detach();
        cx.notify();
    }

    /// The row on an imported habit's page that leads to its review.
    pub(super) fn habit_review_prompt(&self, id: &str, cx: &mut Context<Self>) -> AnyElement {
        let id = id.to_string();
        div()
            .w_full()
            .min_w_0()
            .flex_shrink_0()
            .px_4()
            .pt_3()
            .flex()
            .items_center()
            .gap_2()
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_size(px(12.))
                    .line_height(px(18.))
                    .text_color(content(0.6))
                    .child("Imported from a file. It stays off until you read and approve it."),
            )
            .child(
                header_button("mono-habit-review", "Review", true, true)
                    .on_click(cx.listener(move |this, _, _, cx| this.open_habit_review(&id, cx))),
            )
            .into_any_element()
    }

    /// Opens the review an imported habit needs before it can be enabled or run.
    pub(super) fn open_habit_review(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(habit) = self.habits.iter().find(|h| h.id == id).cloned() else {
            return;
        };
        self.habit_share.review = Some(HabitReview {
            id: id.to_string(),
            fingerprint: habit.review_fingerprint(),
            shown: habit,
            notice: None,
            busy: false,
        });
        self.route = Some(Route::HabitReview(id.to_string()));
        cx.notify();
    }

    /// The stored habit changed (the Mono edits them too): the screen follows it and says so, and
    /// what it now shows is what approving applies to.
    fn habit_review_sync(&mut self) {
        let Some(review) = &mut self.habit_share.review else {
            return;
        };
        let Some(stored) = self.habits.iter().find(|h| h.id == review.id) else {
            return;
        };
        let incoming = stored.review_fingerprint();
        if incoming != review.fingerprint {
            review.fingerprint = incoming;
            review.shown = stored.clone();
            review.notice = Some(CHANGED.into());
        }
    }

    /// Approves what was shown. The fingerprint is the one captured when it was put on screen: the
    /// stored habit is changed only if it still matches, else nothing is approved.
    fn approve_habit_review(&mut self, cx: &mut Context<Self>) {
        let Some(mono) = self.active.clone() else {
            return;
        };
        let Some(review) = &mut self.habit_share.review else {
            return;
        };
        if review.busy {
            return;
        }
        review.busy = true;
        review.notice = None;
        let (id, fingerprint) = (review.id.clone(), review.fingerprint.clone());
        let root = self.root.clone();
        let target = mono.clone();
        let task_id = id.clone();
        let work = cx.background_executor().spawn(async move {
            habits::update(&root, &target, |all| {
                all.iter_mut()
                    .find(|h| h.id == task_id)
                    .ok_or("Habit not found")?
                    .approve(&fingerprint, now_ms())
            })
        });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |this, cx| {
                this.reload_files(&mono, cx);
                this.reload_today_habits(cx);
                if let Some(review) = this.habit_share.review.as_mut().filter(|r| r.id == id) {
                    review.busy = false;
                    match result {
                        Ok(()) => {
                            this.habit_share.review = None;
                            if this.route == Some(Route::HabitReview(id.clone())) {
                                this.habit_share.notice = None;
                                this.route = Some(Route::Habit(id.clone()));
                            }
                        }
                        Err(error) => review.notice = Some(error),
                    }
                }
                this.changed(cx);
            });
        })
        .detach();
        cx.notify();
    }

    pub(super) fn habit_review_page(&mut self, id: &str, cx: &mut Context<Self>) -> AnyElement {
        self.habit_review_sync();
        let stored = self.habits.iter().any(|h| h.id == id);
        let Some(review) = self.habit_share.review.as_ref().filter(|r| r.id == id) else {
            // The page is sliding away; nothing to review any more.
            return div().size_full().into_any_element();
        };
        let shown = review.shown.clone();
        let can_approve = stored && !review.busy;
        let notice = review.notice.clone();
        let sections = Self::habit_sections(
            &shown.name,
            shown.schedule.label(),
            &shown.instructions,
            &warnings_for(&shown.instructions),
        );
        let leave = id.to_string();
        div()
            .size_full()
            .min_w_0()
            .min_h_0()
            .overflow_hidden()
            .flex()
            .flex_col()
            .child(
                self.page_header("Review habit", cx)
                    .child(
                        header_button("mono-habit-review-cancel", "Cancel", false, true).on_click(
                            cx.listener(move |this, _, _, cx| {
                                this.habit_share.review = None;
                                this.route = Some(Route::Habit(leave.clone()));
                                cx.notify();
                            }),
                        ),
                    )
                    .child(
                        header_button(
                            "mono-habit-approve",
                            "Approve and enable",
                            true,
                            can_approve,
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if can_approve {
                                this.approve_habit_review(cx);
                            }
                        })),
                    ),
            )
            .child(
                div()
                    .id("mono-habit-review-page")
                    .debug_selector(|| "mono-habit-review-page".into())
                    .w_full()
                    .min_w_0()
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .flex()
                    .flex_col()
                    .children(super::workflows_ui::view::notice_line(
                        "mono-habit-review-notice",
                        notice.as_deref(),
                    ))
                    .children(sections),
            )
            .into_any_element()
    }
}
