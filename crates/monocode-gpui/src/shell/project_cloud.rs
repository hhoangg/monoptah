//! The project's Cloud settings dialog and the fix terminal it opens for a checklist line.
//! The Git credential panel is not a modal: it is a side panel inside the dialog, which owns
//! its Escape. Escape closes the fix terminal first, then the dialog (after its transient
//! parts).
use super::*;
use crate::cloud_settings::{self, CloudSettings, Env};
use crate::fix_terminal::{FixEvent, FixTerminal};
use gpui::AnyView;
use monocode_core::cloud_sessions::project_cloud::DefaultBackend;
use std::path::PathBuf;

/// The Cloud settings dialog; its barrier is one below. An open select's list draws above it
/// (`cloud_settings::SELECT_PRIORITY`, 61).
const SETTINGS_PRIORITY: usize = 60;
/// The fix terminal stacks on the dialog and over a select's list.
const FIX_PRIORITY: usize = 62;

pub(super) struct OpenProjectCloud {
    view: Entity<CloudSettings>,
    path: String,
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
        let project = path.clone();
        let view = cx.new(|cx| CloudSettings::new(path.clone(), name, backend, env, cx));
        let events = cx.subscribe_in(
            &view,
            window,
            move |this, _, event: &cloud_settings::Event, window, cx| match event {
                cloud_settings::Event::Close => this.close_cloud_settings(window, cx),
                cloud_settings::Event::Saved => this.cloud_settings_saved(&project, cx),
                cloud_settings::Event::Resized => cx.notify(),
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

    /// Escape closes the fix terminal first, then what the dialog has open (a select, the Git
    /// credential panel), then Cloud settings. A terminal with the keyboard keeps Escape for its program.
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
        if open.view.read(cx).terminal_focused(window, cx) {
            return;
        }
        // An open select (or the discard question) closes first; the dialog stays.
        let view = open.view.clone();
        if view.update(cx, |dialog, cx| dialog.dismiss_transient(Some(window), cx)) {
            return;
        }
        self.close_cloud_settings(window, cx);
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
        self.cloud_modal_sized(id, view, priority, 460., None, close, window, cx)
    }

    #[allow(clippy::too_many_arguments)]
    fn cloud_modal_sized(
        &self,
        id: &'static str,
        view: impl Into<AnyView>,
        priority: usize,
        width: f32,
        // A layer drawn over the card (the open select's list): a press on it is inside.
        inside: Option<crate::popover::AnchorSlot>,
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
                    .when_some(inside, |p, slot| p.ignore(slot))
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
        // Wider while the Git credential side panel is open.
        let width = view.read(cx).dialog_width();
        let select = view.read(cx).select_surface(cx);
        Some(self.cloud_modal_sized(
            "cloud-settings-modal",
            view,
            SETTINGS_PRIORITY,
            width,
            select,
            |this, window, cx| {
                // A press outside the dialog that also dismisses an open select's list (the
                // list's own listener handles it in the same press) closes only the list.
                let list_open = this
                    .cloud_settings
                    .as_ref()
                    .is_some_and(|open| open.view.read(cx).select_open());
                if !list_open {
                    this.close_cloud_settings(window, cx);
                }
            },
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
