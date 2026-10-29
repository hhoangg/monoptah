//! Archiving and deleting projects from their menu, and opening them in an
//! external editor (useProjectMenu.tsx and App.tsx's onRemoveProject).
use super::*;
use monocode_core::{
    external_editor::{self, ExternalEditor},
    files::{is_equal_or_inside, path_key},
    provider_cleanup::{self, CleanupRequest},
    store::Store,
};
use std::path::Path;

/// Removes a deleted session's provider files; `None` means the store has no
/// data directory (in memory), where there is nothing to find.
pub(super) type CleanupRunner =
    Arc<dyn Fn(Option<&Path>, &CleanupRequest) -> Vec<String> + Send + Sync>;
pub(super) fn default_cleanup() -> CleanupRunner {
    Arc::new(|data_dir, request| match data_dir {
        Some(dir) => provider_cleanup::run_cleanup(dir, request),
        None => vec!["no data directory, so nothing was removed".into()],
    })
}

/// What a confirmed delete also removes, besides the session itself.
#[derive(Default)]
pub(super) struct DeleteOptions {
    pub provider_files: bool,
    /// An unused worktree to remove once the session is gone.
    pub worktree: Option<String>,
}

/// Which CLI owns the session files a delete can also remove.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ProviderFiles {
    Claude,
    Codex,
    Mixed,
}
impl ProviderFiles {
    pub(super) fn of(kinds: impl IntoIterator<Item = Kind>) -> Option<Self> {
        kinds.into_iter().fold(None, |found, kind| {
            let kind = if kind == Kind::Claude {
                Self::Claude
            } else {
                Self::Codex
            };
            match found {
                None => Some(kind),
                Some(found) if found == kind => Some(kind),
                Some(_) => Some(Self::Mixed),
            }
        })
    }
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Claude => "the Claude Code session files",
            Self::Codex => "the Codex session files",
            Self::Mixed => "the harness session files",
        }
    }
}

/// DeleteSessionDialog.tsx: one confirmation for a single or a bulk delete.
pub(super) struct DeleteDialog {
    pub(super) ids: Vec<i64>,
    title: String,
    pub(super) provider_files: Option<ProviderFiles>,
    /// Each session's worktree that nothing else uses, so deleting leaves it unused.
    worktrees: Vec<(i64, String)>,
    pub(super) delete_provider_files: bool,
    delete_worktree: bool,
    focus: FocusHandle,
}

pub(super) fn delete_title(count: usize) -> String {
    match count {
        1 => "Delete session?".into(),
        n => format!("Delete {n} sessions?"),
    }
}
pub(super) fn delete_summary(title: &str, count: usize) -> String {
    match count {
        1 => format!("“{title}” will be permanently deleted."),
        n => format!("{n} sessions will be permanently deleted."),
    }
}
pub(super) fn delete_button(count: usize) -> String {
    match count {
        1 => "Delete session".into(),
        n => format!("Delete {n} sessions"),
    }
}

/// What to remove for the session, read before its row is deleted.
pub(super) fn provider_cleanup_request(store: &Store, id: i64) -> Option<CleanupRequest> {
    let session = store.sessions().ok()?.into_iter().find(|s| s.id == id)?;
    let side = store
        .codex_side_thread_ids(id, &session.provider)
        .unwrap_or_default();
    provider_cleanup::cleanup_request(&session, &side)
}

/// Whether any stored session row except `ignoring` works inside `path`. Every
/// row counts, hidden, worker and inbox sessions included.
fn worktree_in_use(store: &Database, path: &str, ignoring: &[i64]) -> Result<bool, String> {
    Ok(locked(store)?
        .working_dirs()?
        .iter()
        .any(|(id, dir)| !ignoring.contains(id) && is_equal_or_inside(dir, path)))
}

/// Deletes the worktree once no stored session works in it, and only if it holds
/// no uncommitted work. The branch stays.
pub(super) fn remove_unused_worktree(store: &Database, path: &str) -> Result<(), String> {
    if worktree_in_use(store, path, &[])? {
        return Err("Another session still uses it.".into());
    }
    monocode_core::worktree_management::remove_clean(Path::new(path))
}

impl Shell {
    /// Finds installed editors once, off the main thread; the first lookup
    /// may read the login shell's PATH.
    pub(super) fn scan_external_editors(&mut self, cx: &mut Context<Self>) {
        let work = cx
            .background_executor()
            .spawn(async { external_editor::installed() });
        cx.spawn(async move |this, cx| {
            let editors = work.await;
            let _ = this.update(cx, |this, _| this.external_editors = Some(editors));
        })
        .detach();
    }
    pub(super) fn external_editor_menu(&self, path: &str) -> MenuAction {
        let item = MenuAction::new("external-editor", "Open in editor").icon("app-window");
        let none =
            |label: &str| vec![MenuAction::new("external-editor:none", label).disabled(true)];
        if path.starts_with("remote://") {
            return item
                .disabled(true)
                .submenu(none("Remote folders stay on their host"));
        }
        match &self.external_editors {
            None => item.disabled(true).submenu(none("Looking for editors…")),
            Some(editors) if editors.is_empty() => item.submenu(none("No supported editors found")),
            Some(editors) => item.submenu(
                editors
                    .iter()
                    .map(|ExternalEditor { id, name }| {
                        MenuAction::new(format!("external-editor:{id}"), *name)
                    })
                    .collect(),
            ),
        }
    }
    pub(super) fn open_in_external_editor(
        &mut self,
        editor: &str,
        path: &str,
        cx: &mut Context<Self>,
    ) {
        let (editor, path) = (editor.to_owned(), path.to_owned());
        let work = cx
            .background_executor()
            .spawn(async move { external_editor::open(&editor, &path) });
        cx.spawn(async move |this, cx| {
            if let Err(error) = work.await {
                let _ = this.update(cx, |this, cx| {
                    this.error = Some(error);
                    cx.notify();
                });
            }
        })
        .detach();
    }

    /// Closes the project's tabs and drops it from the rail. Its chats stay in
    /// history, and opening the folder again (or a new chat there) restores it.
    pub(super) fn archive_project(
        &mut self,
        path: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.leave_project(path, window, cx);
        self.workspace.archived_projects.retain(|p| p != path);
        self.workspace.archived_projects.insert(0, path.to_owned());
        self.persist(cx);
        self.sync_settings(cx);
        cx.notify();
    }
    pub(super) fn unarchive_project(&mut self, path: &str, cx: &mut Context<Self>) {
        self.workspace.add_project(path);
        self.persist(cx);
        self.sync_settings(cx);
        cx.notify();
    }
    /// Asks before deleting every conversation in the project. The folder on
    /// disk is left alone; opening it again brings the project back empty.
    pub(super) fn confirm_delete_project(
        &mut self,
        path: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let count = self.project_sessions(&path).len();
        let detail = match count {
            0 => String::new(),
            1 => "Its 1 saved conversation will be deleted. ".into(),
            n => format!("Its {n} saved conversations will be deleted. "),
        } + "The folder on disk stays put, and opening it again brings the project back empty.";
        let prompt = window.prompt(
            gpui::PromptLevel::Warning,
            &format!("Delete “{}”?", self.project_label(&path)),
            Some(&detail),
            &["Delete", "Cancel"],
            cx,
        );
        cx.spawn(async move |this, cx| {
            if prompt.await != Ok(0) {
                return;
            }
            let _ = this.update_in(cx, |this, window, cx| {
                this.delete_project(&path, window, cx)
            });
        })
        .detach();
    }
    fn delete_project(&mut self, path: &str, window: &mut Window, cx: &mut Context<Self>) {
        let sessions = self.project_sessions(path);
        self.leave_project(path, window, cx);
        self.workspace.archived_projects.retain(|p| p != path);
        self.workspace.project_rail_assignments.remove(path);
        self.workspace.project_appearance.remove(path);
        self.workspace.project_focus.remove(path);
        self.workspace.collapsed.remove(path);
        self.persist(cx);
        self.sync_settings(cx);
        for id in sessions {
            self.delete_session(id, window, cx);
        }
        cx.notify();
    }
    /// Asks before permanently deleting the conversation in a tab.
    pub(super) fn confirm_delete_session(
        &mut self,
        id: i64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.request_delete_sessions(vec![id], window, cx);
    }
    /// Opens the delete dialog for the sessions, once nothing else is asking.
    pub(super) fn request_delete_sessions(
        &mut self,
        mut ids: Vec<i64>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        ids.retain(|id| self.session_summaries.contains_key(id));
        ids.dedup();
        if ids.is_empty() || self.delete_dialog.is_some() || self.delete_checking {
            return;
        }
        let candidates = self.unused_worktrees(&ids, cx);
        if candidates.is_empty() {
            self.open_delete_dialog(ids, Vec::new(), window, cx);
            return;
        }
        // Whether a worktree may be removed is a Git question; ask off the UI thread.
        self.delete_checking = true;
        let store = self.store.clone();
        let selected = ids.clone();
        let work = cx.background_executor().spawn(async move {
            candidates
                .into_iter()
                .filter(|(_, path)| {
                    // A failed lookup must never offer filesystem cleanup.
                    worktree_in_use(&store, path, &selected) == Ok(false) && {
                        let path = Path::new(path);
                        monocode_core::worktree_management::check(path, path).is_ok()
                    }
                })
                .collect::<Vec<_>>()
        });
        cx.spawn_in(window, async move |this, cx| {
            let worktrees = work.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.delete_checking = false;
                this.open_delete_dialog(ids, worktrees, window, cx);
            });
        })
        .detach();
    }
    /// Each selected session's own worktree when no other session or terminal the
    /// window knows is working in it. A cheap first cut: the store's rows (hidden
    /// and worker sessions too) decide in `request_delete_sessions`.
    pub(super) fn unused_worktrees(&self, ids: &[i64], cx: &App) -> Vec<(i64, String)> {
        let mut found = Vec::new();
        for id in ids {
            let Some(path) = self
                .session_worktrees
                .get(id)
                .and_then(|p| p.as_deref())
                .or_else(|| {
                    self.sessions
                        .get(id)
                        .and_then(|v| v.read(cx).settings.worktree_cwd.as_deref())
                })
            else {
                continue;
            };
            let used = self
                .session_summaries
                .keys()
                .filter(|other| !ids.contains(other))
                .any(|other| is_equal_or_inside(self.session_working_copy(*other, cx), path))
                || self
                    .terminals
                    .values()
                    .flatten()
                    .any(|t| is_equal_or_inside(&t.read(cx).cwd, path));
            if !used {
                found.push((*id, path.to_owned()));
            }
        }
        found
    }
    /// The CLI whose files this computer holds for the session. A session on a
    /// remote machine keeps them there, out of reach.
    fn local_files_kind(&self, id: i64, cx: &App) -> Option<Kind> {
        let summary = self.session_metadata(id, cx).summary;
        provider_cleanup::files_kind(summary).filter(|_| !summary.cwd.starts_with("remote://"))
    }
    pub(super) fn open_delete_dialog(
        &mut self,
        ids: Vec<i64>,
        worktrees: Vec<(i64, String)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let ids: Vec<i64> = ids
            .into_iter()
            .filter(|id| self.session_summaries.contains_key(id))
            .collect();
        if ids.is_empty() || self.delete_dialog.is_some() {
            return;
        }
        self.close_preview(cx);
        let title = match ids.as_slice() {
            [id] => self.session_metadata(*id, cx).display_title(),
            _ => String::new(),
        };
        let provider_files =
            ProviderFiles::of(ids.iter().filter_map(|id| self.local_files_kind(*id, cx)));
        let dialog = DeleteDialog {
            ids,
            title,
            provider_files,
            worktrees,
            delete_provider_files: true,
            delete_worktree: true,
            focus: cx.focus_handle(),
        };
        window.focus(&dialog.focus, cx);
        self.delete_dialog = Some(dialog);
        cx.notify();
    }
    pub(super) fn close_delete_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.delete_dialog.take().is_some() {
            self.focus_active(window, cx);
            cx.notify();
        }
    }
    pub(super) fn confirm_delete_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(dialog) = self.delete_dialog.take() else {
            return;
        };
        self.focus_active(window, cx);
        let provider_files = dialog.provider_files.is_some() && dialog.delete_provider_files;
        // Sessions sharing a worktree are deleted in order, so only the last one
        // removes it, once nothing else is using it.
        let mut last: HashMap<String, i64> = HashMap::new();
        for (id, path) in &dialog.worktrees {
            last.insert(path_key(path), *id);
        }
        for id in dialog.ids {
            let worktree = dialog
                .worktrees
                .iter()
                .find(|(owner, _)| *owner == id)
                .filter(|(_, path)| {
                    dialog.delete_worktree && last.get(&path_key(path)) == Some(&id)
                })
                .map(|(_, path)| path.clone());
            // Only sessions that really have files get a request, so a hidden
            // box can never turn into a cleanup.
            let has_files = self.local_files_kind(id, cx).is_some();
            self.delete_session_with(
                id,
                DeleteOptions {
                    provider_files: provider_files && has_files,
                    worktree,
                },
                window,
                cx,
            );
        }
        cx.notify();
    }
    /// DeleteSessionDialog.tsx, centred over the window.
    pub(super) fn delete_modal(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let dialog = self.delete_dialog.as_ref()?;
        let count = dialog.ids.len();
        let width = 400f32.min(window.viewport_size().width.as_f32() - 24.);
        let weak = cx.weak_entity();
        let note = |text: String| {
            div()
                .mt(px(4.))
                .text_size(px(11.))
                .line_height(px(15.))
                .text_color(content(0.45))
                .child(text)
        };
        let option = |id: &'static str, checked: bool, label: String, notes: Vec<String>| {
            div()
                .id(id)
                .debug_selector(move || id.into())
                .role(Role::CheckBox)
                .aria_label(label.clone())
                .aria_toggled(if checked {
                    gpui::Toggled::True
                } else {
                    gpui::Toggled::False
                })
                .cursor_pointer()
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
                        }),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_size(px(12.))
                        .line_height(px(17.))
                        .child(label)
                        .children(notes.into_iter().map(&note)),
                )
        };
        let paths: Vec<String> = {
            let mut seen = HashSet::new();
            dialog
                .worktrees
                .iter()
                .filter(|(_, path)| seen.insert(path_key(path)))
                .map(|(_, path)| path.clone())
                .collect()
        };
        Some(
            div()
                .absolute()
                .size_full()
                .child(crate::modal::scrim("delete-sessions-scrim"))
                .child(
                    popover("delete-sessions-modal", Anchor::Center)
                        .modal_surface(self.workspace.boxy_layout())
                        .width(width)
                        .gap(0.)
                        .priority(60)
                        .on_dismiss(move |window, cx| {
                            let _ = weak.update(cx, |this, cx| this.close_delete_dialog(window, cx));
                        })
                        .child(
                            div()
                                .id("delete-sessions-dialog")
                                .role(Role::Dialog)
                                .aria_label(delete_title(count))
                                .track_focus(&dialog.focus)
                                .key_context("DeleteSessions")
                                .on_key_down(cx.listener(
                                    |this, event: &gpui::KeyDownEvent, window, cx| {
                                        if event.keystroke.key == "enter" {
                                            this.confirm_delete_dialog(window, cx);
                                            cx.stop_propagation();
                                        }
                                    },
                                ))
                                .p_4()
                                .flex()
                                .flex_col()
                                .gap_4()
                                .child(
                                    div()
                                        .text_size(px(14.))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(content(1.))
                                        .child(delete_title(count)),
                                )
                                .child(
                                    div()
                                        .text_size(px(12.))
                                        .child(delete_summary(&dialog.title, count)),
                                )
                                .when_some(dialog.provider_files, |d, files| {
                                    d.child(
                                        option(
                                            "delete-provider-files",
                                            dialog.delete_provider_files,
                                            format!("Also delete {} on this computer", files.label()),
                                            vec![
                                                "Frees disk space. The conversation can no longer be resumed from the CLI."
                                                    .into(),
                                            ],
                                        )
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            if let Some(d) = &mut this.delete_dialog {
                                                d.delete_provider_files = !d.delete_provider_files;
                                                cx.notify();
                                            }
                                        })),
                                    )
                                })
                                .when(!paths.is_empty(), |d| {
                                    let mut notes: Vec<String> =
                                        paths
                                        .iter()
                                        .map(|path| crate::worktree_switcher::pretty_path(path))
                                        .collect();
                                    notes.push(
                                        "The branch is kept. If files have uncommitted changes, the worktree stays."
                                            .into(),
                                    );
                                    d.child(
                                        option(
                                            "delete-worktree",
                                            dialog.delete_worktree,
                                            if paths.len() > 1 {
                                                "Also delete unused worktrees"
                                            } else {
                                                "Also delete the unused worktree"
                                            }
                                            .into(),
                                            notes,
                                        )
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            if let Some(d) = &mut this.delete_dialog {
                                                d.delete_worktree = !d.delete_worktree;
                                                cx.notify();
                                            }
                                        })),
                                    )
                                })
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .justify_end()
                                        .gap_2()
                                        .child(
                                            crate::modal::ghost_button("delete-cancel", "Cancel")
                                                .debug_selector(|| "delete-cancel".into())
                                                .cursor_pointer()
                                                .on_click(cx.listener(|this, _, window, cx| {
                                                    this.close_delete_dialog(window, cx)
                                                })),
                                        )
                                        .child(
                                            crate::modal::danger_button(
                                                "delete-confirm",
                                                delete_button(count),
                                            )
                                            .debug_selector(|| "delete-confirm".into())
                                            .cursor_pointer()
                                            .on_click(cx.listener(|this, _, window, cx| {
                                                this.confirm_delete_dialog(window, cx)
                                            })),
                                        ),
                                ),
                        ),
                )
                .into_any_element(),
        )
    }

    fn project_sessions(&self, path: &str) -> Vec<i64> {
        self.session_summaries
            .values()
            .filter(|summary| summary.cwd == path)
            .map(|summary| summary.id)
            .collect()
    }
    /// Closes the project's panes and moves to another project if it was current.
    fn leave_project(&mut self, path: &str, window: &mut Window, cx: &mut Context<Self>) {
        let sessions: HashSet<i64> = self.project_sessions(path).into_iter().collect();
        let panes: Vec<i64> = self
            .workspace
            .open_sessions()
            .into_iter()
            .filter(|pane| sessions.contains(&self.workspace.session_for_pane(*pane)))
            .collect();
        for pane in panes {
            self.close_pane(pane, window, cx);
        }
        self.workspace.projects.retain(|p| p != path);
        self.workspace.project_rail_pinned.remove(path);
        if self.workspace.project == path {
            let next = self
                .workspace
                .projects
                .first()
                .cloned()
                .unwrap_or_else(|| std::env::var("HOME").unwrap_or_else(|_| "/".into()));
            if !self.workspace.projects.contains(&next) {
                self.workspace.add_project(&next);
            }
            self.select_project(next, window, cx);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shell::tests::window;
    use gpui::{MouseButton, TestAppContext, VisualTestContext, WindowHandle};
    use monocode_core::provider_cleanup::CleanupRequest;
    use std::path::PathBuf;

    const CODEX_ID: &str = "00000000-0000-4000-8000-00000000000a";
    const CLAUDE_ID: &str = "00000000-0000-4000-8000-00000000000b";

    type Calls = Arc<Mutex<Vec<(Option<PathBuf>, Kind, Vec<String>, bool)>>>;

    /// The shared fixture with provider ids on both sessions and a cleanup
    /// seam that records its calls instead of touching a real provider home.
    fn setup(cx: &mut TestAppContext) -> (WindowHandle<Shell>, i64, i64, Calls) {
        cx.update(|cx| cx.set_prompt_builder(crate::modal::prompt_builder));
        let (window, a, b) = window(cx);
        let calls = Calls::default();
        window
            .update(cx, |shell, _, cx| {
                for (id, provider_id) in [(a, CODEX_ID), (b, CLAUDE_ID)] {
                    locked(&shell.store)
                        .unwrap()
                        .provider_id(id, provider_id)
                        .unwrap();
                    shell.session_summaries.get_mut(&id).unwrap().provider_id =
                        Some(provider_id.into());
                    if let Some(view) = shell.sessions.get(&id) {
                        view.update(cx, |v, _| v.summary.provider_id = Some(provider_id.into()));
                    }
                }
                let (store, seen) = (shell.store.clone(), calls.clone());
                shell.provider_cleanup = Arc::new(move |dir, request: &CleanupRequest| {
                    let present = locked(&store).unwrap().sessions().unwrap().iter().any(|s| {
                        s.provider_id.as_deref() == request.ids().first().map(String::as_str)
                    });
                    seen.lock().unwrap().push((
                        dir.map(Path::to_path_buf),
                        request.provider(),
                        request.ids().to_vec(),
                        present,
                    ));
                    Vec::new()
                });
            })
            .unwrap();
        (window, a, b, calls)
    }

    #[gpui::test]
    fn sidebar_delete_opens_the_dialog_instead_of_deleting(cx: &mut TestAppContext) {
        let (window, a, _, calls) = setup(cx);
        window
            .update(cx, |shell, w, cx| {
                shell.run_menu(MenuTarget::Session(a), "delete", None, w, cx);
                let dialog = shell.delete_dialog.as_ref().expect("a confirmation");
                assert_eq!(dialog.ids, [a]);
                assert_eq!(dialog.provider_files, Some(ProviderFiles::Codex));
                assert!(dialog.delete_provider_files, "on by default");
                assert!(shell.deleting.is_empty());
                assert!(shell.session_summaries.contains_key(&a));
                shell.close_delete_dialog(w, cx);
                assert!(shell.delete_dialog.is_none());
            })
            .unwrap();
        cx.run_until_parked();
        window
            .update(cx, |shell, _, _| {
                assert!(shell.session_summaries.contains_key(&a));
                assert!(
                    locked(&shell.store)
                        .unwrap()
                        .sessions()
                        .unwrap()
                        .iter()
                        .any(|s| s.id == a)
                );
            })
            .unwrap();
        assert!(calls.lock().unwrap().is_empty());
    }

    #[gpui::test]
    fn the_tab_menu_asks_through_the_same_dialog(cx: &mut TestAppContext) {
        let (window, a, _, _) = setup(cx);
        window
            .update(cx, |shell, w, cx| {
                shell.run_menu(MenuTarget::Tab(a), "delete", None, w, cx);
                assert_eq!(shell.delete_dialog.as_ref().unwrap().ids, [a]);
            })
            .unwrap();
    }

    #[gpui::test]
    fn confirming_with_the_provider_box_unchecked_does_not_clean_up(cx: &mut TestAppContext) {
        let (window, a, b, calls) = setup(cx);
        window
            .update(cx, |shell, w, cx| {
                shell.run_menu(MenuTarget::Session(a), "delete", None, w, cx);
                shell.delete_dialog.as_mut().unwrap().delete_provider_files = false;
                shell.confirm_delete_dialog(w, cx);
                assert!(shell.delete_dialog.is_none());
            })
            .unwrap();
        cx.run_until_parked();
        window
            .update(cx, |shell, _, _| {
                assert!(!shell.session_summaries.contains_key(&a), "still deleted");
                assert!(shell.session_summaries.contains_key(&b));
            })
            .unwrap();
        assert!(calls.lock().unwrap().is_empty());
    }

    #[gpui::test]
    fn confirming_by_default_cleans_up_after_the_row_is_gone(cx: &mut TestAppContext) {
        let (window, a, _, calls) = setup(cx);
        window
            .update(cx, |shell, w, cx| {
                shell.run_menu(MenuTarget::Session(a), "delete", None, w, cx);
                shell.confirm_delete_dialog(w, cx);
            })
            .unwrap();
        cx.run_until_parked();
        let calls = calls.lock().unwrap();
        // The in-memory test store has no data directory.
        assert_eq!(
            *calls,
            [(None, Kind::Codex, vec![CODEX_ID.to_string()], false)],
            "one cleanup, and the record was already deleted"
        );
    }

    #[gpui::test]
    fn the_row_is_forgotten_before_provider_cleanup_runs(cx: &mut TestAppContext) {
        let (window, a, _, _) = setup(cx);
        let ran = Arc::new(std::sync::atomic::AtomicBool::new(false));
        window
            .update(cx, |shell, w, cx| {
                let seen = ran.clone();
                shell.provider_cleanup = Arc::new(move |_, _| {
                    seen.store(true, std::sync::atomic::Ordering::SeqCst);
                    Vec::new()
                });
                shell.run_menu(MenuTarget::Session(a), "delete", None, w, cx);
                shell.confirm_delete_dialog(w, cx);
            })
            .unwrap();
        // Step the executor until the cleanup has just run, then look at the window.
        while !ran.load(std::sync::atomic::Ordering::SeqCst) && cx.executor().tick() {}
        assert!(ran.load(std::sync::atomic::Ordering::SeqCst), "cleanup ran");
        window
            .update(cx, |shell, _, _| {
                assert!(
                    !shell.session_summaries.contains_key(&a),
                    "the sidebar row is gone before the slow file cleanup finishes"
                );
                assert!(!shell.deleting.contains(&a));
            })
            .unwrap();
        cx.run_until_parked();
    }

    #[gpui::test]
    fn backspace_and_bulk_delete_confirm_once_for_the_whole_selection(cx: &mut TestAppContext) {
        let (window, a, b, calls) = setup(cx);
        // The last open tab cannot close, so the second session is a history row.
        let history = window
            .update(cx, |shell, _, _| {
                let store = locked(&shell.store).unwrap();
                let mut session = store.create("claude", "/work/monocode").unwrap();
                store.provider_id(session.id, CLAUDE_ID).unwrap();
                session.provider_id = Some(CLAUDE_ID.into());
                drop(store);
                shell.session_summaries.insert(session.id, session.clone());
                session.id
            })
            .unwrap();
        window
            .update(cx, |shell, w, cx| {
                shell.selected_sessions = [a, history].into_iter().collect();
                shell.delete_sidebar_selection(w, cx);
                let dialog = shell.delete_dialog.as_ref().unwrap();
                assert_eq!(dialog.ids.len(), 2);
                assert_eq!(dialog.provider_files, Some(ProviderFiles::Mixed));
                assert!(shell.session_summaries.contains_key(&a));
                // A second request while one is open must not stack another.
                shell.delete_sidebar_selection(w, cx);
                shell.confirm_delete_dialog(w, cx);
            })
            .unwrap();
        cx.run_until_parked();
        window
            .update(cx, |shell, _, _| {
                assert!(
                    !shell.session_summaries.contains_key(&a),
                    "{:?}",
                    shell.error
                );
                assert!(!shell.session_summaries.contains_key(&history));
                assert!(shell.session_summaries.contains_key(&b));
            })
            .unwrap();
        let mut kinds: Vec<_> = calls.lock().unwrap().iter().map(|c| c.1).collect();
        kinds.sort_by_key(|kind| kind.id());
        assert_eq!(kinds, [Kind::Claude, Kind::Codex]);
    }

    #[gpui::test]
    fn no_provider_checkbox_for_a_session_that_never_reached_the_cli(cx: &mut TestAppContext) {
        let (window, a, _, calls) = setup(cx);
        window
            .update(cx, |shell, w, cx| {
                shell.session_summaries.get_mut(&a).unwrap().provider_id = None;
                if let Some(view) = shell.sessions.get(&a) {
                    view.update(cx, |v, _| v.summary.provider_id = None);
                }
                shell.run_menu(MenuTarget::Session(a), "delete", None, w, cx);
                let dialog = shell.delete_dialog.as_mut().unwrap();
                assert_eq!(dialog.provider_files, None);
                // A stale "on" must not request cleanup for a hidden box.
                dialog.delete_provider_files = true;
                shell.confirm_delete_dialog(w, cx);
            })
            .unwrap();
        cx.run_until_parked();
        assert!(calls.lock().unwrap().is_empty());
    }

    #[gpui::test]
    fn a_remote_session_keeps_its_files_on_the_remote_machine(cx: &mut TestAppContext) {
        let (window, a, _, calls) = setup(cx);
        window
            .update(cx, |shell, w, cx| {
                shell.session_summaries.get_mut(&a).unwrap().cwd = "remote://box/srv/app".into();
                if let Some(view) = shell.sessions.get(&a) {
                    view.update(cx, |v, _| v.summary.cwd = "remote://box/srv/app".into());
                }
                shell.run_menu(MenuTarget::Session(a), "delete", None, w, cx);
                assert_eq!(shell.delete_dialog.as_ref().unwrap().provider_files, None);
                shell.confirm_delete_dialog(w, cx);
            })
            .unwrap();
        cx.run_until_parked();
        assert!(calls.lock().unwrap().is_empty());
    }

    #[gpui::test]
    fn the_checkbox_is_drawn_and_a_click_toggles_it(cx: &mut TestAppContext) {
        let (window, a, _, _) = setup(cx);
        window
            .update(cx, |shell, w, cx| {
                shell.run_menu(MenuTarget::Session(a), "delete", None, w, cx);
            })
            .unwrap();
        let visual = &mut VisualTestContext::from_window(window.into(), cx);
        visual.refresh().unwrap();
        visual.run_until_parked();
        let row = visual.debug_bounds("delete-provider-files").expect("drawn");
        assert!(visual.debug_bounds("delete-worktree").is_none());
        visual.simulate_click(row.center(), Default::default());
        window
            .update(visual, |shell, _, _| {
                assert!(!shell.delete_dialog.as_ref().unwrap().delete_provider_files);
            })
            .unwrap();
        let confirm = visual.debug_bounds("delete-confirm").expect("drawn");
        visual.simulate_mouse_down(confirm.center(), MouseButton::Left, Default::default());
        visual.simulate_mouse_up(confirm.center(), MouseButton::Left, Default::default());
        visual.run_until_parked();
        window
            .update(visual, |shell, _, _| {
                assert!(shell.delete_dialog.is_none());
                assert!(!shell.session_summaries.contains_key(&a));
            })
            .unwrap();
    }

    #[gpui::test]
    fn only_worktrees_nothing_else_uses_are_offered(cx: &mut TestAppContext) {
        let (window, a, b, _) = setup(cx);
        window
            .update(cx, |shell, _, cx| {
                shell.session_worktrees.insert(a, Some("/work/wt".into()));
                shell.session_worktrees.insert(b, Some("/work/wt".into()));
                // Another session still working in the worktree keeps it alive.
                assert!(shell.unused_worktrees(&[a], cx).is_empty());
                assert_eq!(
                    shell.unused_worktrees(&[a, b], cx),
                    [(a, "/work/wt".to_string()), (b, "/work/wt".to_string())]
                );
            })
            .unwrap();
    }

    /// A temp repository with one linked worktree on its own branch.
    struct Repo {
        path: PathBuf,
        tree: String,
    }
    impl Repo {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "monoptah-delete-worktree-{}",
                monocode_core::mono::new_id()
            ));
            std::fs::create_dir_all(&path).unwrap();
            let mut repo = Self {
                path,
                tree: String::new(),
            };
            repo.git(&["init", "-b", "main"]);
            repo.git(&["config", "user.email", "fixture@example.invalid"]);
            repo.git(&["config", "user.name", "Fixture"]);
            std::fs::write(repo.path.join("file"), "x").unwrap();
            repo.git(&["add", "file"]);
            repo.git(&["commit", "-m", "Initial"]);
            repo.tree = monocode_core::git::create_named_worktree(&repo.path, "session-wt")
                .unwrap()
                .path;
            repo
        }
        fn git(&self, args: &[&str]) {
            let output = std::process::Command::new("git")
                .arg("-C")
                .arg(&self.path)
                .args(args)
                .output()
                .unwrap();
            assert!(output.status.success(), "{args:?}");
        }
        fn exists(&self) -> bool {
            Path::new(&self.tree).exists()
        }
    }
    impl Drop for Repo {
        fn drop(&mut self) {
            let _ =
                std::fs::remove_dir(monocode_core::worktree_management::default_root(&self.path));
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }
    /// A stored row the window never loaded, like a hidden Mono session or an
    /// orchestration worker, working in `dir`.
    fn stored_only_session(shell: &Shell, dir: &str) {
        let store = locked(&shell.store).unwrap();
        let session = store.create("codex", "/work/monocode").unwrap();
        let settings = monocode_core::agent::Settings {
            worktree_cwd: Some(dir.into()),
            ..Default::default()
        };
        store.save_agent_settings(session.id, &settings).unwrap();
    }

    #[gpui::test]
    fn the_unused_worktree_is_offered_and_removed_with_the_session(cx: &mut TestAppContext) {
        cx.executor().allow_parking();
        let (window, a, b, _) = setup(cx);
        let repo = Repo::new();
        window
            .update(cx, |shell, w, cx| {
                shell.session_worktrees.insert(a, Some(repo.tree.clone()));
                shell.request_delete_sessions(vec![a], w, cx);
                assert!(shell.delete_dialog.is_none(), "waits for the Git check");
            })
            .unwrap();
        cx.run_until_parked();
        window
            .update(cx, |shell, w, cx| {
                let dialog = shell
                    .delete_dialog
                    .as_ref()
                    .expect("opened after the check");
                assert_eq!(dialog.worktrees, [(a, repo.tree.clone())]);
                shell.confirm_delete_dialog(w, cx);
            })
            .unwrap();
        cx.run_until_parked();
        window
            .update(cx, |shell, _, _| {
                assert!(!shell.session_summaries.contains_key(&a));
                assert!(shell.session_summaries.contains_key(&b));
                assert_eq!(shell.error, None);
            })
            .unwrap();
        assert!(!repo.exists());
        repo.git(&["show-ref", "--verify", "refs/heads/session-wt"]);
    }

    #[gpui::test]
    fn unchecking_the_worktree_box_keeps_the_worktree(cx: &mut TestAppContext) {
        cx.executor().allow_parking();
        let (window, a, _, _) = setup(cx);
        let repo = Repo::new();
        window
            .update(cx, |shell, w, cx| {
                shell.session_worktrees.insert(a, Some(repo.tree.clone()));
                shell.request_delete_sessions(vec![a], w, cx);
            })
            .unwrap();
        cx.run_until_parked();
        window
            .update(cx, |shell, w, cx| {
                let dialog = shell
                    .delete_dialog
                    .as_mut()
                    .expect("opened after the check");
                assert!(dialog.delete_worktree, "on by default");
                dialog.delete_worktree = false;
                shell.confirm_delete_dialog(w, cx);
            })
            .unwrap();
        cx.run_until_parked();
        window
            .update(cx, |shell, _, _| {
                assert!(
                    !shell.session_summaries.contains_key(&a),
                    "session still deleted"
                );
                assert_eq!(shell.error, None);
            })
            .unwrap();
        assert!(repo.exists());
    }

    #[gpui::test]
    fn a_stored_session_the_window_never_loaded_keeps_the_worktree(cx: &mut TestAppContext) {
        cx.executor().allow_parking();
        let (window, a, _, _) = setup(cx);
        let repo = Repo::new();
        // Known when the dialog opens: it must not even offer the worktree.
        window
            .update(cx, |shell, w, cx| {
                shell.session_worktrees.insert(a, Some(repo.tree.clone()));
                stored_only_session(shell, &repo.tree);
                shell.request_delete_sessions(vec![a], w, cx);
            })
            .unwrap();
        cx.run_until_parked();
        window
            .update(cx, |shell, w, cx| {
                let dialog = shell.delete_dialog.as_ref().expect("still asks to delete");
                assert!(dialog.worktrees.is_empty(), "not offered");
                shell.close_delete_dialog(w, cx);
            })
            .unwrap();
        assert!(repo.exists());
    }

    #[gpui::test]
    fn the_final_recheck_sees_a_session_that_appeared_after_the_dialog(cx: &mut TestAppContext) {
        cx.executor().allow_parking();
        let (window, a, _, _) = setup(cx);
        let repo = Repo::new();
        window
            .update(cx, |shell, w, cx| {
                shell.session_worktrees.insert(a, Some(repo.tree.clone()));
                shell.request_delete_sessions(vec![a], w, cx);
            })
            .unwrap();
        cx.run_until_parked();
        window
            .update(cx, |shell, w, cx| {
                assert_eq!(shell.delete_dialog.as_ref().unwrap().worktrees.len(), 1);
                stored_only_session(shell, &repo.tree);
                shell.confirm_delete_dialog(w, cx);
            })
            .unwrap();
        cx.run_until_parked();
        window
            .update(cx, |shell, _, _| {
                assert!(!shell.session_summaries.contains_key(&a));
                let error = shell.error.as_deref().unwrap();
                assert!(error.contains("Its worktree was kept"), "{error}");
            })
            .unwrap();
        assert!(repo.exists());
    }

    #[test]
    fn dialog_copy_follows_the_react_dialog() {
        assert_eq!(delete_title(1), "Delete session?");
        assert_eq!(delete_title(3), "Delete 3 sessions?");
        assert_eq!(
            delete_summary("Fix it", 1),
            "“Fix it” will be permanently deleted."
        );
        assert_eq!(
            delete_summary("", 3),
            "3 sessions will be permanently deleted."
        );
        assert_eq!(delete_button(1), "Delete session");
        assert_eq!(delete_button(3), "Delete 3 sessions");
        assert_eq!(
            ProviderFiles::Claude.label(),
            "the Claude Code session files"
        );
        assert_eq!(ProviderFiles::Codex.label(), "the Codex session files");
        assert_eq!(ProviderFiles::Mixed.label(), "the harness session files");
        assert_eq!(
            ProviderFiles::of([Kind::Claude, Kind::Claude]),
            Some(ProviderFiles::Claude)
        );
        assert_eq!(
            ProviderFiles::of([Kind::Claude, Kind::Codex]),
            Some(ProviderFiles::Mixed)
        );
        assert_eq!(ProviderFiles::of([]), None);
    }
}
