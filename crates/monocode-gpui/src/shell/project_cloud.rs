//! The project's Cloud settings dialog, the Git credential dialog it opens on top of itself
//! and the fix terminal it opens for a checklist line. Escape closes the topmost first.
use super::*;
use crate::cloud_settings::{self, CloudSettings, Env};
use crate::fix_terminal::{FixEvent, FixTerminal};
use crate::git_credential_dialog::{self, GitCredentialDialog};
use gpui::AnyView;
use monocode_core::cloud_sessions::project_cloud::DefaultBackend;
use std::path::PathBuf;

/// Above the Cloud settings dialog (60), the credential dialog (61) and its barrier.
const SETTINGS_PRIORITY: usize = 60;
const CREDENTIAL_PRIORITY: usize = 61;
const FIX_PRIORITY: usize = 62;

pub(super) struct OpenProjectCloud {
    view: Entity<CloudSettings>,
    path: String,
    credential: Option<Child<GitCredentialDialog>>,
    fix: Option<Child<FixTerminal>>,
    _events: Subscription,
}

struct Child<V: 'static> {
    view: Entity<V>,
    _events: Subscription,
}

impl Shell {
    /// Opens Cloud settings for the project at `path`, on `backend` or else the project's
    /// default backend.
    pub(super) fn open_cloud_settings(
        &mut self,
        path: String,
        backend: Option<DefaultBackend>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let env = Env::for_shell(
            self.store.clone(),
            self.gh.clone(),
            cloud_settings::claude_accounts(cx),
            cx,
        );
        self.open_cloud_settings_with(path, backend, env, window, cx);
    }

    pub(super) fn open_cloud_settings_with(
        &mut self,
        path: String,
        backend: Option<DefaultBackend>,
        env: Env,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.close_menu(cx);
        self.close_preview(cx);
        self.drop_cloud_settings(cx);
        let name = self.project_label(&path);
        let (store, git) = (env.store.clone(), env.git.clone());
        let project = path.clone();
        let view = cx.new(|cx| CloudSettings::new(path.clone(), name, backend, env, cx));
        let events = cx.subscribe_in(
            &view,
            window,
            move |this, _, event: &cloud_settings::Event, window, cx| match event {
                cloud_settings::Event::Close => this.close_cloud_settings(window, cx),
                cloud_settings::Event::Saved => this.cloud_settings_saved(&project, cx),
                cloud_settings::Event::ManageCredentials { repo_url, selected } => {
                    let env = git_credential_dialog::Env {
                        store: store.clone(),
                        git: git.clone(),
                    };
                    this.open_git_credential_dialog(
                        repo_url.clone(),
                        selected.clone(),
                        env,
                        window,
                        cx,
                    );
                }
                cloud_settings::Event::Fix(slot) => {
                    if let Some(command) = slot.take() {
                        this.open_fix_terminal(command, window, cx);
                    }
                }
                cloud_settings::Event::ReplaceDockerToken => {
                    this.close_cloud_settings(window, cx);
                    this.open_settings(crate::settings::Section::Cloud, window, cx);
                }
            },
        );
        window.focus(&view.focus_handle(cx), cx);
        self.cloud_settings = Some(OpenProjectCloud {
            view,
            path,
            credential: None,
            fix: None,
            _events: events,
        });
        cx.notify();
    }

    /// Ends everything the dialog and its children run, and forgets them.
    fn drop_cloud_settings(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(open) = self.cloud_settings.take() else {
            return false;
        };
        if let Some(credential) = open.credential {
            credential.view.update(cx, |view, cx| view.shutdown(cx));
        }
        if let Some(fix) = open.fix {
            fix.view.update(cx, |view, cx| view.shutdown(cx));
        }
        open.view.update(cx, |view, cx| view.shutdown(cx));
        true
    }

    pub(super) fn close_cloud_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.drop_cloud_settings(cx) {
            self.focus_workspace(window, cx);
            cx.notify();
        }
    }

    /// The settings were stored: open conversations of the project forget what they read
    /// (config, default backend) and an open cloud panel follows the new defaults on the rows
    /// the user did not change.
    pub(super) fn cloud_settings_saved(&mut self, project: &str, cx: &mut Context<Self>) {
        for session in self.sessions.values() {
            if session.read(cx).summary.cwd == project {
                session.update(cx, |session, cx| session.project_cloud_saved(cx));
            }
        }
    }

    /// The project's cloud sessions changed (one started, archived, moved): the Workspace
    /// menus re-read the most recent instance the next time they open.
    pub(super) fn forget_cloud_menus(&mut self, cx: &mut Context<Self>) {
        for session in self.sessions.values() {
            session.update(cx, |session, _| session.forget_cloud_menu());
        }
    }

    /// Escape closes the fix terminal first, then the credential dialog, then Cloud
    /// settings. A terminal with the keyboard keeps Escape for its program.
    pub(super) fn cloud_settings_escape(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(open) = &self.cloud_settings else {
            return;
        };
        if let Some(fix) = &open.fix {
            if !fix.view.read(cx).terminal_focused(window, cx) {
                self.close_fix_terminal(window, cx);
            }
            return;
        }
        if open.credential.is_some() {
            self.close_git_credential_dialog(window, cx);
            return;
        }
        if open.view.read(cx).terminal_focused(window, cx) {
            return;
        }
        // An open select (or the discard question) closes first; the dialog stays.
        let view = open.view.clone();
        if view.update(cx, |dialog, cx| dialog.dismiss_transient(cx)) {
            return;
        }
        self.close_cloud_settings(window, cx);
    }

    pub(super) fn open_git_credential_dialog(
        &mut self,
        repo_url: String,
        selected: Option<String>,
        env: git_credential_dialog::Env,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(open) = &self.cloud_settings else {
            return;
        };
        if open.credential.is_some() || open.fix.is_some() {
            return;
        }
        let settings = open.view.clone();
        let name = settings.read(cx).project_name().to_owned();
        let view = cx.new(|cx| GitCredentialDialog::new(name, repo_url, selected, env, cx));
        let events = cx.subscribe_in(
            &view,
            window,
            move |this, _, event: &git_credential_dialog::Event, window, cx| match event {
                git_credential_dialog::Event::Close => this.close_git_credential_dialog(window, cx),
                git_credential_dialog::Event::Chosen(id) => {
                    let id = id.clone();
                    settings.update(cx, |dialog, cx| {
                        dialog.reload_credentials(Some(id), cx);
                    });
                    this.close_git_credential_dialog(window, cx);
                }
            },
        );
        window.focus(&view.focus_handle(cx), cx);
        if let Some(open) = &mut self.cloud_settings {
            open.credential = Some(Child {
                view,
                _events: events,
            });
        }
        cx.notify();
    }

    pub(super) fn close_git_credential_dialog(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(open) = &mut self.cloud_settings else {
            return;
        };
        if let Some(credential) = open.credential.take() {
            credential.view.update(cx, |view, cx| view.shutdown(cx));
            let settings = open.view.clone();
            window.focus(&settings.focus_handle(cx), cx);
            cx.notify();
        }
    }

    fn open_fix_terminal(
        &mut self,
        command: crate::fix_terminal::FixCommand,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(open) = &self.cloud_settings else {
            return;
        };
        if open.fix.is_some() {
            return;
        }
        let settings = open.view.clone();
        let cwd = if std::path::Path::new(&open.path).is_dir() {
            PathBuf::from(&open.path)
        } else {
            std::env::temp_dir()
        };
        let view = cx.new(|cx| FixTerminal::new(command, &cwd, cx));
        let events = cx.subscribe_in(
            &view,
            window,
            move |this, _, event: &FixEvent, window, cx| match event {
                FixEvent::Exited { .. } => {
                    settings.update(cx, |dialog, cx| dialog.fix_finished(cx))
                }
                FixEvent::Close => this.close_fix_terminal(window, cx),
            },
        );
        window.focus(&view.focus_handle(cx), cx);
        if let Some(open) = &mut self.cloud_settings {
            open.fix = Some(Child {
                view,
                _events: events,
            });
        }
        cx.notify();
    }

    pub(super) fn close_fix_terminal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(open) = &mut self.cloud_settings else {
            return;
        };
        if let Some(fix) = open.fix.take() {
            fix.view.update(cx, |view, cx| view.shutdown(cx));
            // The command may have ended without the dialog hearing of it (Close first).
            let settings = open.view.clone();
            settings.update(cx, |dialog, cx| dialog.fix_finished(cx));
            window.focus(&settings.focus_handle(cx), cx);
            cx.notify();
        }
    }

    pub(super) fn cloud_modal(
        &self,
        id: &'static str,
        view: impl Into<AnyView>,
        priority: usize,
        close: fn(&mut Shell, &mut Window, &mut Context<Shell>),
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.cloud_modal_sized(id, view, priority, 460., close, window, cx)
    }

    #[allow(clippy::too_many_arguments)]
    fn cloud_modal_sized(
        &self,
        id: &'static str,
        view: impl Into<AnyView>,
        priority: usize,
        width: f32,
        close: fn(&mut Shell, &mut Window, &mut Context<Shell>),
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let viewport = window.viewport_size();
        let width = width.min(viewport.width.as_f32() - 24.);
        let weak = cx.weak_entity();
        div()
            .absolute()
            .size_full()
            // A barrier, not a plain scrim: the window behind gets no hover, click or scroll
            // while the dialog is open.
            .child(crate::modal::barrier(
                format!("{id}-scrim"),
                priority,
                window,
            ))
            .child(
                popover(id, Anchor::Center)
                    .modal_surface(self.workspace.boxy_layout())
                    .width(width)
                    .gap(0.)
                    .max_height(viewport.height.as_f32() - 32.)
                    .priority(priority)
                    .on_dismiss(move |window, cx| {
                        let _ = weak.update(cx, |this, cx| close(this, window, cx));
                    })
                    .child(view.into()),
            )
            .into_any_element()
    }

    pub(super) fn cloud_settings_modal(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let view = self.cloud_settings.as_ref()?.view.clone();
        Some(self.cloud_modal_sized(
            "cloud-settings-modal",
            view,
            SETTINGS_PRIORITY,
            540.,
            |this, window, cx| this.close_cloud_settings(window, cx),
            window,
            cx,
        ))
    }

    pub(super) fn git_credential_modal(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let view = self
            .cloud_settings
            .as_ref()?
            .credential
            .as_ref()?
            .view
            .clone();
        Some(self.cloud_modal(
            "git-credential-modal",
            view,
            CREDENTIAL_PRIORITY,
            |this, window, cx| this.close_git_credential_dialog(window, cx),
            window,
            cx,
        ))
    }

    pub(super) fn fix_terminal_modal(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let view = self.cloud_settings.as_ref()?.fix.as_ref()?.view.clone();
        Some(self.cloud_modal(
            "cloud-fix-modal",
            view,
            FIX_PRIORITY,
            |this, window, cx| this.close_fix_terminal(window, cx),
            window,
            cx,
        ))
    }
}

#[cfg(test)]
mod tests;
