//! Opening a file a chat mentions: the shell's side of `crate::file_preview`.
use super::*;
use crate::{
    document_lightbox::{CloudDoc, Kind},
    file_preview::{self, CannotPreview, Route},
    image_lightbox::Source,
};
use std::path::{Path, PathBuf};

impl Shell {
    /// The folder, absolute path and `:line:col` of a file link clicked in a chat, and the
    /// folders it was looked for in. A Mono's link is bound to its first project; the file may be
    /// in another one, in a worktree, or in any project open in the app (see
    /// `files::links::search_roots`, which fixes the order).
    pub(super) fn link_target(
        &self,
        raw: &str,
        cx: &App,
    ) -> (String, String, Option<(usize, usize)>, Vec<String>) {
        let cwd = self.git.read(cx).project().to_owned();
        let (mono_roots, session_worktree) = if self.mono_active(cx) {
            let projects = self
                .monos
                .as_ref()
                .and_then(|m| m.read(cx).current().map(|m| m.projects.clone()))
                .unwrap_or_default();
            (
                crate::file_links::candidate_roots(Some(&self.workspace.project), &projects),
                None,
            )
        } else {
            let worktree = self
                .workspace
                .active
                .and_then(|id| self.sessions.get(&id))
                .map(|view| view.read(cx))
                .map(|view| (view.execution_cwd().to_owned(), view.summary.cwd.clone()))
                .filter(|(run, own)| run != own)
                .map(|(run, _)| run);
            (Vec::new(), worktree)
        };
        let project = mono_roots.first().cloned().unwrap_or_else(|| cwd.clone());
        let search = |worktrees: fn(&str) -> Vec<String>| {
            monocode_core::files::links::search_roots(
                &project,
                session_worktree.as_deref(),
                &mono_roots,
                &self.workspace.projects,
                worktrees,
            )
        };
        // The git worktrees are listed only when the file is not found without them.
        let cheap = search(|_| Vec::new());
        let (path, location) = crate::file_links::resolve_link(raw, &cwd, &cheap);
        if Path::new(&path).exists() || monocode_core::files::remote::is_remote(Path::new(&path)) {
            return (cwd, path, location, cheap);
        }
        let roots = search(monocode_core::files::links::project_worktrees);
        let (path, location) = crate::file_links::resolve_link(raw, &cwd, &roots);
        (cwd, path, location, roots)
    }

    /// Makes a session pane the active one for a file to open beside, when none is: a Mono tab
    /// and a project with nothing open have no active pane. The project's remembered tab is
    /// preferred, else the first plain session tab. False when there is no session to attach to.
    fn borrow_file_owner(
        &mut self,
        apart: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let workspace = &self.workspace;
        let plain = |id: i64| {
            workspace.tab_of(id).is_some()
                && !workspace.review_owners.contains_key(&id)
                && !workspace.mono_panes.contains_key(&id)
                && !workspace.terminal_panes.contains_key(&id)
                && self
                    .session_summaries
                    .contains_key(&workspace.session_for_pane(id))
        };
        let pick = workspace
            .project_focus
            .get(&workspace.project)
            .copied()
            .filter(|id| plain(*id))
            .or_else(|| workspace.tabs.iter().copied().find(|id| plain(*id)));
        let Some(id) = pick else {
            return false;
        };
        if apart {
            // The file is moved into a pane of its own right after, which takes the view over.
            self.workspace.active = Some(id);
        } else {
            self.activate_session(id, window, cx);
        }
        true
    }

    /// Opens `path` in the code editor at `location`. The file sits beside the active session's
    /// chat; from a Mono or a page such as Today it gets a pane of its own, which takes the
    /// window over without closing the Mono's chat.
    pub(super) fn open_file_at(
        &mut self,
        cwd: String,
        path: String,
        location: Option<(usize, usize)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let apart = self.mono_active(cx) || self.active_app_view.is_some();
        if self.workspace.active.is_none() && !self.borrow_file_owner(apart, window, cx) {
            self.error = Some(format!(
                "Open a chat in this project first to view {}",
                file_preview::file_name(Path::new(&path))
            ));
            cx.notify();
            return;
        }
        self.open_review(crate::review::ReviewTab::File { cwd, path }, window, cx);
        if apart
            && let Some(owner) = self.workspace.active
            && let Some(pane) = self.independent_file_pane(owner, cx)
        {
            self.open_file_pane(pane, window, cx);
        }
        if let Some((line, column)) = location
            && let Some(pane) = self.active_review(cx)
        {
            pane.update(cx, |pane, cx| pane.navigate_file(line, column, cx));
        }
    }

    /// The file the document lightbox shows, for tests in other modules.
    #[cfg(test)]
    pub(crate) fn document_preview_path(&self, cx: &App) -> Option<PathBuf> {
        self.document_lightbox
            .as_ref()
            .map(|(view, _)| view.read(cx).path().to_path_buf())
    }

    pub(super) fn show_image_lightbox(
        &mut self,
        open: crate::image_lightbox::Open,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let view = cx.new(|cx| crate::image_lightbox::ImageLightbox::new(open, window, cx));
        let closed = cx.subscribe(&view, |this, _, _: &crate::image_lightbox::Closed, cx| {
            this.image_lightbox = None;
            cx.notify();
        });
        self.image_lightbox = Some((view, closed));
        cx.notify();
    }

    pub(super) fn show_document_lightbox(
        &mut self,
        open: crate::document_lightbox::Open,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let view = cx.new(|cx| crate::document_lightbox::DocumentLightbox::new(open, window, cx));
        let closed = cx.subscribe(
            &view,
            |this, _, _: &crate::document_lightbox::Closed, cx| {
                this.document_lightbox = None;
                cx.notify();
            },
        );
        self.document_lightbox = Some((view, closed));
        cx.notify();
    }

    fn show_cannot_preview(
        &mut self,
        path: PathBuf,
        size: u64,
        folder: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let view = cx.new(|cx| CannotPreview::new(path, size, folder, window, cx));
        let closed = cx.subscribe(&view, |this, _, _: &file_preview::Closed, cx| {
            this.cannot_preview = None;
            cx.notify();
        });
        self.cannot_preview = Some((view, closed));
        cx.notify();
    }

    /// A click on a file chip or link in a chat: shows the file as what it is (see
    /// `file_preview::classify`). Paths outside the projects open like any other: this is the
    /// local machine.
    pub(crate) fn preview_path(&mut self, raw: &str, window: &mut Window, cx: &mut Context<Self>) {
        // A path in the cloud workspace of the active cloud session.
        if self.open_cloud_path(raw, window, cx) {
            return;
        }
        self.preview_local_path(raw, window, cx);
    }

    /// [`Shell::preview_path`] for a link that is known to name a file of this machine: one a
    /// floating Mono forwarded, already resolved against its own projects. The active cloud
    /// session has nothing to say about it, so it never goes through `open_cloud_path`.
    pub(crate) fn preview_local_path(
        &mut self,
        raw: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (cwd, path, location, searched) = self.link_target(raw, cx);
        // A file on another machine is not on this disk to look at: the editor reads it.
        if monocode_core::files::remote::is_remote(Path::new(&path)) {
            self.open_file_at(cwd, path, location, window, cx);
            return;
        }
        let route = file_preview::destination(file_preview::classify(Path::new(&path)), location);
        // The file shown before gives way to this one.
        self.image_lightbox = None;
        self.document_lightbox = None;
        self.cannot_preview = None;
        let file = PathBuf::from(&path);
        let title = file_preview::file_name(&file);
        match route {
            Route::Editor => self.open_file_at(cwd, path, location, window, cx),
            Route::Html => {
                // The page may read the files beside it (its images and styles), and nothing else.
                let root = file.parent().map(Path::to_path_buf).unwrap_or_default();
                let cloud = CloudDoc {
                    kind: Kind::Html,
                    text: None,
                    web_root: Some(root),
                    scripts: false,
                };
                self.show_document_lightbox(
                    crate::document_lightbox::Open {
                        path: file,
                        title,
                        cloud: Some(cloud),
                    },
                    window,
                    cx,
                );
            }
            Route::Markdown { mdx } => {
                // The lightbox only knows `md` and `markdown` by extension.
                let cloud = mdx.then_some(CloudDoc {
                    kind: Kind::Markdown,
                    text: None,
                    web_root: None,
                    scripts: true,
                });
                self.show_document_lightbox(
                    crate::document_lightbox::Open {
                        path: file,
                        title,
                        cloud,
                    },
                    window,
                    cx,
                );
            }
            Route::Image => self.show_image_lightbox(
                crate::image_lightbox::Open {
                    source: Source::Path(file),
                    alt: title,
                },
                window,
                cx,
            ),
            Route::Unsupported { size, folder } => {
                self.show_cannot_preview(file, size, folder, window, cx)
            }
            Route::Missing => {
                self.error = Some(format!(
                    "File not found: {path} (looked in: {})",
                    monocode_core::files::links::looked_in(&searched)
                ));
                cx.notify();
            }
            Route::Unreadable(reason) => {
                self.error = Some(format!("Could not read {path}: {reason}"));
                cx.notify();
            }
        }
    }
}
