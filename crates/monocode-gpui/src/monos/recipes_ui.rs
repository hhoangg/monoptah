//! The Recipes section of a Mono's Browser tab (Phase 4 of
//! `docs/superpowers/specs/2026-11-22-browser-recipes-design.md`): the list, the steps, approve,
//! dry run, rollback, delete, export and import, and sharing with the other Monos.
//!
//! Everything that decides something lives in `monocode_core::mono::browser::recipe` (`manage`,
//! `share`): this page only shows it and forwards the user's clicks. The clicks here are the
//! user's own approval, so a recipe that came from a file or the shared library becomes usable
//! only through them: Approve (bound to the steps shown), then a dry run in this Mono.
use super::browser_ui::{action, ago, note};
use super::panels::{card, heading};
use super::*;
use crate::cloud_dialog_ui::alert;
use gpui::{Div, Stateful};
use monocode_core::mono::browser::{
    self, MonoBrowserStore,
    recipe::{
        manage::{self, DryRunReport, Row, Status},
        share::{self, Library, RecipeFile, SOURCE_IMPORT, SOURCE_SHARED},
        store::RecipeStore,
    },
};

const RECIPES_SUBTITLE: &str = "Saved browser tasks this Mono can repeat without clicking through the page. Review the steps of a recipe before you approve it.";

/// The state of the Recipes section.
#[derive(Default)]
pub(super) struct RecipesUi {
    pub(super) rows: Vec<Row>,
    /// The shared library, every entry that passes the file checks.
    pub(super) shared: Vec<RecipeFile>,
    /// The recipe whose steps are open, with the steps and the hash of the content they show.
    /// Approve is bound to that hash.
    pub(super) open: Option<OpenRecipe>,
    pub(super) busy: Option<String>,
    /// A recipe that was asked to be deleted once; the next click deletes it.
    pub(super) confirm_delete: Option<String>,
    pub(super) notice: Option<String>,
    pub(super) error: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct OpenRecipe {
    pub(super) id: String,
    pub(super) steps: String,
    pub(super) hash: String,
}

/// The file name of an exported recipe.
pub(super) fn export_file_name(intent: &str) -> String {
    format!("{intent}.monoptah-recipe.json")
}

fn percent_text(row: &Row) -> String {
    match row.success_percent() {
        Some(pct) => format!("{pct}% ({}/{})", row.ok, row.runs),
        None => "not run yet".into(),
    }
}

fn row_button(id: String, label: &str) -> Stateful<Div> {
    let selector = id.clone();
    crate::modal::ghost_button(SharedString::from(id), label.to_owned())
        .debug_selector(move || selector.clone())
        .border_1()
        .border_color(content(0.1))
        .cursor_pointer()
}

impl Monos {
    /// The recipe files and the approvals of the open Mono, and the shared library.
    fn recipes_stores(&self) -> Result<(String, RecipeStore, MonoBrowserStore, Library), String> {
        let id = self.active.clone().ok_or("No Mono is open.")?;
        let approvals = self.browser_store()?;
        Ok((
            id.clone(),
            RecipeStore::new(browser::recipes_dir(&self.root, &id)),
            approvals,
            Library::new(browser::shared_recipes_dir(&self.root)),
        ))
    }

    /// Reads the list and the library from disk.
    pub(super) fn recipes_refresh(&mut self) {
        let loaded = self
            .recipes_stores()
            .map(|(id, store, approvals, library)| {
                (manage::rows(&store, &approvals, &id), library.list())
            });
        let ui = &mut self.browser_ui.recipes;
        match loaded {
            Ok((rows, shared)) => {
                ui.rows = rows;
                ui.shared = shared;
            }
            Err(_) => {
                ui.rows.clear();
                ui.shared.clear();
            }
        }
        // A recipe that is gone, or that changed since its steps were shown, closes its steps.
        let stale = ui.open.as_ref().is_some_and(|open| {
            !ui.rows
                .iter()
                .any(|r| r.id == open.id && r.hash == open.hash)
        });
        if stale {
            ui.open = None;
        }
    }

    fn recipes_said(
        &mut self,
        notice: Option<String>,
        error: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let ui = &mut self.browser_ui.recipes;
        ui.notice = notice;
        ui.error = error;
        ui.confirm_delete = None;
        self.recipes_refresh();
        cx.notify();
    }

    /// Opens or closes the steps of a recipe. What is shown is read from the file now, and the
    /// hash of exactly that content is what Approve is bound to.
    pub(super) fn recipes_toggle(&mut self, id: &str, cx: &mut Context<Self>) {
        let ui = &mut self.browser_ui.recipes;
        ui.confirm_delete = None;
        if ui.open.as_ref().is_some_and(|o| o.id == id) {
            ui.open = None;
            cx.notify();
            return;
        }
        let shown = self
            .recipes_stores()
            .and_then(|(_, store, _, _)| {
                store.get(id).ok_or("That recipe no longer exists.".into())
            })
            .map(|recipe| OpenRecipe {
                id: recipe.id.clone(),
                steps: manage::steps_text(&recipe),
                hash: recipe.content_hash(),
            });
        match shown {
            Ok(open) => {
                self.browser_ui.recipes.open = Some(open);
                self.browser_ui.recipes.error = None;
            }
            Err(error) => self.browser_ui.recipes.error = Some(error),
        }
        cx.notify();
    }

    /// The user approved the steps that are shown.
    pub(super) fn recipes_approve(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(open) = self
            .browser_ui
            .recipes
            .open
            .clone()
            .filter(|open| open.id == id)
        else {
            self.recipes_said(None, Some("Review the steps first.".into()), cx);
            return;
        };
        let result = self
            .recipes_stores()
            .and_then(|(mono, store, approvals, _)| {
                manage::approve(&store, &approvals, &mono, id, &open.hash)
            });
        match result {
            Ok(()) => {
                let needs_dry_run = self
                    .browser_ui
                    .recipes
                    .rows
                    .iter()
                    .any(|r| r.id == id && r.outside.is_some());
                self.recipes_said(
                    Some(if needs_dry_run {
                        "Approved. It came from outside this Mono, so it runs only after a dry run passes.".into()
                    } else {
                        "Approved.".into()
                    }),
                    None,
                    cx,
                );
            }
            Err(error) => {
                self.browser_ui.recipes.open = None;
                self.recipes_said(None, Some(error), cx);
            }
        }
    }

    /// The first click asks, the second deletes.
    pub(super) fn recipes_delete(&mut self, id: &str, cx: &mut Context<Self>) {
        if self.browser_ui.recipes.confirm_delete.as_deref() != Some(id) {
            self.browser_ui.recipes.confirm_delete = Some(id.to_owned());
            cx.notify();
            return;
        }
        let result = self
            .recipes_stores()
            .and_then(|(mono, store, approvals, _)| manage::delete(&store, &approvals, &mono, id));
        if self
            .browser_ui
            .recipes
            .open
            .as_ref()
            .is_some_and(|o| o.id == id)
        {
            self.browser_ui.recipes.open = None;
        }
        match result {
            Ok(()) => self.recipes_said(Some("Deleted.".into()), None, cx),
            Err(error) => self.recipes_said(None, Some(error), cx),
        }
    }

    pub(super) fn recipes_rollback(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(version) = self
            .browser_ui
            .recipes
            .rows
            .iter()
            .find(|r| r.id == id)
            .and_then(|r| r.previous)
        else {
            return;
        };
        let result = self
            .recipes_stores()
            .and_then(|(mono, store, approvals, _)| {
                manage::rollback(&store, &approvals, &mono, id, version)
            });
        self.browser_ui.recipes.open = None;
        match result {
            Ok(restored) => {
                let status = self
                    .recipes_stores()
                    .map(|(mono, _, approvals, _)| {
                        manage::status_of(&approvals, &mono, &restored)
                            .label()
                            .to_lowercase()
                    })
                    .unwrap_or_default();
                self.recipes_said(
                    Some(format!("Back to version {version} ({status}).")),
                    None,
                    cx,
                );
            }
            Err(error) => self.recipes_said(None, Some(error), cx),
        }
    }

    /// Runs the dry run through the Mono's gate (its trust level and blocked sites apply) off the
    /// UI thread.
    pub(super) fn recipes_dry_run(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(mono) = self.active.clone() else {
            return;
        };
        if self.browser_ui.recipes.busy.is_some() {
            return;
        }
        let ops = self.browser_ui.ops.clone();
        self.browser_ui.recipes.busy = Some(id.to_owned());
        self.browser_ui.recipes.notice = Some("Running the dry run...".into());
        self.browser_ui.recipes.error = None;
        self.browser_ui.recipes.confirm_delete = None;
        cx.notify();
        let recipe = id.to_owned();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move { ops.dry_run_recipe(&mono, &recipe) })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.browser_ui.recipes.busy = None;
                match result {
                    Ok(report) => this.recipes_dry_run_done(report, cx),
                    Err(error) => this.recipes_said(None, Some(error), cx),
                }
            });
        })
        .detach();
    }
    fn recipes_dry_run_done(&mut self, report: DryRunReport, cx: &mut Context<Self>) {
        if report.ok {
            let tail = match report.stopped_before {
                Some(step) => format!(
                    " It stopped before step {step}, which changes things: a dry run never types or sends."
                ),
                None => String::new(),
            };
            self.recipes_said(
                Some(format!(
                    "The dry run passed ({} steps).{tail}",
                    report.steps_run
                )),
                None,
                cx,
            );
        } else {
            self.recipes_said(
                None,
                Some(format!(
                    "The dry run failed. {}",
                    report.failure.unwrap_or_default()
                )),
                cx,
            );
        }
    }

    /// Saves the recipe as a file the user chooses.
    pub(super) fn recipes_export(&mut self, id: &str, cx: &mut Context<Self>) {
        let exported = self
            .recipes_stores()
            .and_then(|(_, store, _, _)| {
                store.get(id).ok_or("That recipe no longer exists.".into())
            })
            .and_then(|recipe| share::export(&recipe).map(|text| (recipe.intent, text)));
        match exported {
            Ok((intent, text)) => self.save_text_file(
                export_file_name(&intent),
                text,
                "recipe",
                |this, saved, cx| {
                    let (notice, error) = match saved {
                        super::workflows_ui::page::Saved::To(path) => {
                            (Some(format!("Exported to {}.", path.display())), None)
                        }
                        super::workflows_ui::page::Saved::Cancelled => (None, None),
                        super::workflows_ui::page::Saved::Failed => {
                            (None, Some("Could not save the recipe file.".to_owned()))
                        }
                    };
                    this.recipes_said(notice, error, cx);
                },
                cx,
            ),
            Err(error) => self.recipes_said(None, Some(error), cx),
        }
    }

    /// Asks for a recipe file and imports it as a draft.
    pub(super) fn recipes_import(&mut self, cx: &mut Context<Self>) {
        let picked = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Import recipe".into()),
        });
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = picked.await else {
                return;
            };
            let Some(path) = paths.into_iter().next() else {
                return;
            };
            // A recipe file is small: a bigger file is refused without being read.
            let text = cx
                .background_spawn(async move {
                    let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
                    if size > share::MAX_FILE_BYTES as u64 {
                        return Err("This file is too big to be a recipe.".to_owned());
                    }
                    std::fs::read_to_string(&path)
                        .map_err(|_| "That file could not be read.".to_owned())
                })
                .await;
            let _ = this.update(cx, |this, cx| match text {
                Ok(text) => this.recipes_import_text(&text, cx),
                Err(error) => this.recipes_said(None, Some(error), cx),
            });
        })
        .detach();
    }

    /// Imports the text of a recipe file: checked, saved as an unapproved draft, and its steps
    /// opened for review.
    pub(super) fn recipes_import_text(&mut self, text: &str, cx: &mut Context<Self>) {
        let blocked = self.browser_setting().blocked_origins;
        let result = self
            .recipes_stores()
            .and_then(|(mono, store, approvals, _)| {
                share::import(&store, &approvals, &mono, text, &blocked, SOURCE_IMPORT)
            });
        self.recipes_imported(result, "Imported", cx);
    }

    fn recipes_imported(
        &mut self,
        result: Result<monocode_core::mono::browser::recipe::model::Recipe, String>,
        verb: &str,
        cx: &mut Context<Self>,
    ) {
        match result {
            Ok(recipe) => {
                self.recipes_said(
                    Some(format!(
                        "{verb} `{}` as a draft. Review its steps, approve it, then run its dry run.",
                        recipe.intent
                    )),
                    None,
                    cx,
                );
                self.recipes_toggle(&recipe.id, cx);
            }
            Err(error) => self.recipes_said(None, Some(error), cx),
        }
    }

    pub(super) fn recipes_promote(&mut self, id: &str, cx: &mut Context<Self>) {
        let result = self
            .recipes_stores()
            .and_then(|(mono, store, approvals, library)| {
                manage::promote(&store, &approvals, &library, &mono, id)
            });
        match result {
            Ok(()) => self.recipes_said(
                Some("Shared. Other Monos can add a copy; each keeps its own level and blocked sites.".into()),
                None,
                cx,
            ),
            Err(error) => self.recipes_said(None, Some(error), cx),
        }
    }

    pub(super) fn recipes_unshare(&mut self, origin: &str, intent: &str, cx: &mut Context<Self>) {
        let result = self
            .recipes_stores()
            .and_then(|(_, _, _, library)| library.unshare(origin, intent));
        match result {
            Ok(()) => self.recipes_said(
                Some("Unshared. Copies other Monos already made stay with them.".into()),
                None,
                cx,
            ),
            Err(error) => self.recipes_said(None, Some(error), cx),
        }
    }

    /// Copies a shared recipe into this Mono as a draft.
    pub(super) fn recipes_adopt(&mut self, origin: &str, intent: &str, cx: &mut Context<Self>) {
        let blocked = self.browser_setting().blocked_origins;
        let result = self
            .recipes_stores()
            .and_then(|(mono, store, approvals, library)| {
                library.adopt(origin, intent, &store, &approvals, &mono, &blocked)
            });
        self.recipes_imported(result, "Added", cx);
    }

    // ---- Drawing ----

    pub(super) fn recipes_card(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let ui = &self.browser_ui.recipes;
        let (rows, shared) = (ui.rows.clone(), ui.shared.clone());
        let (open, busy, confirm) = (ui.open.clone(), ui.busy.clone(), ui.confirm_delete.clone());
        let (notice, error) = (ui.notice.clone(), ui.error.clone());
        let mut list = card();
        if rows.is_empty() {
            list = list.child(
                div().px_3().py_3().child(note(
                    "mono-recipes-empty",
                    "No recipes yet. A Mono saves one after a browsing task you are likely to repeat, or you can import a file.",
                    0.45,
                )),
            );
        }
        for (ix, row) in rows.iter().enumerate() {
            let in_library = shared
                .iter()
                .any(|f| f.origin == row.origin && f.recipe.intent == row.intent);
            list = list.child(self.recipe_row(
                ix,
                row,
                open.as_ref().filter(|o| o.id == row.id),
                busy.as_deref() == Some(row.id.as_str()),
                confirm.as_deref() == Some(row.id.as_str()),
                in_library,
                cx,
            ));
        }
        list = list.child(
            div().flex().items_center().gap_2().px_3().py_2().child(
                action("mono-recipes-import", "Import a recipe file...")
                    .on_click(cx.listener(|this, _, _, cx| this.recipes_import(cx))),
            ),
        );
        let have: Vec<(String, String)> = rows
            .iter()
            .map(|r| (r.origin.clone(), r.intent.clone()))
            .collect();
        let library = self.recipes_shared_card(&shared, &have, cx);
        let mut column = div()
            .flex()
            .flex_col()
            .gap_2()
            .child(heading("Recipes", Some(RECIPES_SUBTITLE)));
        if let Some(error) = error {
            column = column.child(alert("mono-recipes-error", error));
        }
        if let Some(notice) = notice {
            column = column.child(note("mono-recipes-notice", notice, 0.7).px_1());
        }
        column.child(list).child(library).into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    fn recipe_row(
        &mut self,
        ix: usize,
        row: &Row,
        open: Option<&OpenRecipe>,
        busy: bool,
        confirm: bool,
        in_library: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let id = row.id.clone();
        let origin_note = match row.outside.as_deref() {
            Some(SOURCE_SHARED) => "  (copied from the shared library)",
            Some(_) => "  (imported)",
            None => "",
        };
        let meta = format!(
            "v{}  -  last run {}  -  {}{}{}",
            row.version,
            if row.last_run_ms == 0 {
                "never".to_owned()
            } else {
                ago(row.last_run_ms)
            },
            percent_text(row),
            if row.repaired { "  -  repaired" } else { "" },
            if row.min_level > 1 {
                format!("  -  needs Level {}", row.min_level)
            } else {
                String::new()
            },
        );
        let mut head = div()
            .flex()
            .items_center()
            .gap_2()
            .child(
                note(
                    format!("mono-recipe-title-{ix}"),
                    format!("{}  {}{origin_note}", row.intent, row.origin),
                    0.9,
                )
                .flex_1()
                .min_w_0()
                .truncate(),
            )
            .child(note(
                format!("mono-recipe-status-{ix}"),
                row.status.label(),
                match row.status {
                    Status::Approved => 0.7,
                    _ => 0.5,
                },
            ));
        if row.flaky {
            head = head.child(note(format!("mono-recipe-flaky-{ix}"), "Flaky", 0.9));
        }
        let button =
            |name: &str, label: &str| row_button(format!("mono-recipe-{name}-{ix}"), label);
        let mut buttons = div().flex().flex_wrap().gap_2().child(
            button(
                "view",
                if open.is_some() {
                    "Hide steps"
                } else {
                    "View steps"
                },
            )
            .on_click({
                let id = id.clone();
                cx.listener(move |this, _, _, cx| this.recipes_toggle(&id, cx))
            }),
        );
        if row.status != Status::Draft {
            buttons = buttons.child(
                button("dryrun", if busy { "Running..." } else { "Dry run" }).on_click({
                    let id = id.clone();
                    cx.listener(move |this, _, _, cx| this.recipes_dry_run(&id, cx))
                }),
            );
        }
        if let Some(previous) = row.previous {
            buttons = buttons.child(
                button("rollback", &format!("Roll back to v{previous}")).on_click({
                    let id = id.clone();
                    cx.listener(move |this, _, _, cx| this.recipes_rollback(&id, cx))
                }),
            );
        }
        if row.status == Status::Approved && !in_library {
            buttons = buttons.child(button("share", "Share with other Monos").on_click({
                let id = id.clone();
                cx.listener(move |this, _, _, cx| this.recipes_promote(&id, cx))
            }));
        }
        if in_library {
            let (origin, intent) = (row.origin.clone(), row.intent.clone());
            buttons = buttons.child(button("unshare", "Unshare").on_click(
                cx.listener(move |this, _, _, cx| this.recipes_unshare(&origin, &intent, cx)),
            ));
        }
        buttons = buttons.child(button("export", "Export...").on_click({
            let id = id.clone();
            cx.listener(move |this, _, _, cx| this.recipes_export(&id, cx))
        }));
        buttons = buttons.child(
            crate::modal::danger_ghost_button(
                SharedString::from(format!("mono-recipe-delete-{ix}")),
                if confirm {
                    "Click again to delete"
                } else {
                    "Delete"
                },
            )
            .debug_selector(move || format!("mono-recipe-delete-{ix}"))
            .cursor_pointer()
            .on_click({
                let id = id.clone();
                cx.listener(move |this, _, _, cx| this.recipes_delete(&id, cx))
            }),
        );
        let mut body = div()
            .flex()
            .flex_col()
            .gap_1()
            .px_3()
            .py_2()
            .child(head)
            .child(note(format!("mono-recipe-meta-{ix}"), meta, 0.5))
            .child(note(
                format!("mono-recipe-description-{ix}"),
                row.description.clone(),
                0.6,
            ))
            .child(buttons);
        if let Some(open) = open {
            body = body.child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .p_2()
                    .rounded_md()
                    .border_1()
                    .border_color(content(0.1))
                    .child(note(
                        format!("mono-recipe-steps-{ix}"),
                        open.steps.clone(),
                        0.8,
                    ))
                    .when(row.status == Status::Draft, |d| {
                        d.child(note(
                            format!("mono-recipe-approve-note-{ix}"),
                            "Approving lets this Mono run exactly these steps in your browser, under its level and blocked sites. Values are never part of a recipe.",
                            0.55,
                        ))
                        .child(
                            row_button(format!("mono-recipe-approve-{ix}"), "Approve these steps")
                                .on_click({
                                    let id = id.clone();
                                    cx.listener(move |this, _, _, cx| this.recipes_approve(&id, cx))
                                }),
                        )
                    }),
            );
        }
        div()
            .id(SharedString::from(format!("mono-recipe-{ix}")))
            .debug_selector(move || format!("mono-recipe-{ix}"))
            .child(body)
            .into_any_element()
    }

    fn recipes_shared_card(
        &mut self,
        shared: &[RecipeFile],
        have: &[(String, String)],
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut library = card();
        if shared.is_empty() {
            library = library.child(div().px_3().py_3().child(note(
                "mono-recipes-shared-empty",
                "Nothing is shared yet.",
                0.45,
            )));
        }
        for (ix, file) in shared.iter().enumerate() {
            let (origin, intent) = (file.origin.clone(), file.recipe.intent.clone());
            let owned = have.iter().any(|(o, i)| o == &origin && i == &intent);
            let mut line = div().flex().items_center().gap_2().px_3().py_2().child(
                note(
                    format!("mono-recipe-shared-{ix}"),
                    format!(
                        "{}  {}  -  {} risk, v{}",
                        intent,
                        origin,
                        file.recipe.computed_risk().as_str(),
                        file.version
                    ),
                    0.8,
                )
                .flex_1()
                .min_w_0()
                .truncate(),
            );
            if !owned {
                let (o, i) = (origin.clone(), intent.clone());
                line = line.child(
                    row_button(format!("mono-recipe-adopt-{ix}"), "Add to this Mono").on_click(
                        cx.listener(move |this, _, _, cx| this.recipes_adopt(&o, &i, cx)),
                    ),
                );
            }
            line = line.child(
                row_button(format!("mono-recipe-shared-remove-{ix}"), "Unshare").on_click(
                    cx.listener(move |this, _, _, cx| this.recipes_unshare(&origin, &intent, cx)),
                ),
            );
            library = library.child(line);
        }
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(heading(
                "Shared recipes",
                Some("Recipes users shared from any Mono. Adding one makes a copy here: it starts unapproved, and this Mono's level and blocked sites apply."),
            ))
            .child(library)
            .into_any_element()
    }
}
