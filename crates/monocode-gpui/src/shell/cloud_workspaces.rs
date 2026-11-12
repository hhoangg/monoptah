//! The Cloud workspaces dialog opened from the project menu's Cloud setup submenu, and what the
//! shell does with its Stop and Delete: adds its live busy view, and on a delete forgets the
//! sessions of the deleted workspace in the window and the Sub-agents panel.
use super::*;
use crate::cloud_workspaces_dialog::{self, CloudWorkspacesDialog};

pub(super) struct OpenCloudWorkspaces {
    pub(super) view: Entity<CloudWorkspacesDialog>,
    _events: Subscription,
}

impl Shell {
    pub(super) fn open_cloud_workspaces(
        &mut self,
        path: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let env = cloud_workspaces_dialog::Env::system(self.store.clone());
        self.open_cloud_workspaces_with(path, env, window, cx);
    }

    pub(super) fn open_cloud_workspaces_with(
        &mut self,
        path: String,
        env: cloud_workspaces_dialog::Env,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.close_menu(cx);
        self.close_preview(cx);
        let name = self.project_label(&path);
        let view = cx.new(|cx| CloudWorkspacesDialog::new(path, name, env, cx));
        let dialog = view.clone();
        let events = cx.subscribe_in(
            &view,
            window,
            move |this, _, event: &cloud_workspaces_dialog::Event, window, cx| match event {
                cloud_workspaces_dialog::Event::Close => this.close_cloud_workspaces(window, cx),
                cloud_workspaces_dialog::Event::Act { cloud, action } => {
                    // The dialog runs the call; the live view of what is busy is the shell's.
                    let busy = this.cloud_busy_snapshot(cx);
                    let (cloud, action) = (cloud.clone(), *action);
                    dialog.update(cx, |dialog, cx| dialog.run(cloud, action, busy, cx));
                }
                cloud_workspaces_dialog::Event::Stopped(cloud) => {
                    this.cloud_states.remove(&cloud.key());
                    this.refresh_cloud(cx)
                }
                cloud_workspaces_dialog::Event::Deleted { cloud, removed } => {
                    this.forget_deleted_workspace(cloud.clone(), removed.clone(), window, cx)
                }
            },
        );
        window.focus(&view.focus_handle(cx), cx);
        self.cloud_workspaces = Some(OpenCloudWorkspaces {
            view,
            _events: events,
        });
        cx.notify();
    }

    pub(super) fn close_cloud_workspaces(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.cloud_workspaces.take().is_some() {
            self.focus_workspace(window, cx);
            cx.notify();
        }
    }

    /// Escape leaves the delete confirmation first, then closes the dialog.
    pub(super) fn cloud_workspaces_escape(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(open) = &self.cloud_workspaces else {
            return;
        };
        if !open.view.update(cx, |dialog, cx| dialog.cancel_confirm(cx)) {
            self.close_cloud_workspaces(window, cx);
        }
    }

    /// After the dialog deleted a workspace: the same cleanup as the app's `cloud.delete`. The
    /// Sub-agents panel dismisses the launches on it, and the window drops its sessions and marks
    /// the workspace gone ([`Shell::forget_deleted_cloud`]).
    fn forget_deleted_workspace(
        &mut self,
        cloud: monocode_core::cloud_sessions::CloudRef,
        removed: Vec<i64>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match self.monos.clone() {
            Some(monos) => monos.update(cx, |monos, cx| {
                monos.forget_deleted_cloud(cloud, removed, window, cx)
            }),
            None => self.forget_deleted_cloud(&cloud, &removed, window, cx),
        }
    }

    pub(super) fn cloud_workspaces_modal(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let view = self.cloud_workspaces.as_ref()?.view.clone();
        let viewport = window.viewport_size();
        let width = 640f32.min(viewport.width.as_f32() - 24.);
        let weak = cx.weak_entity();
        Some(
            div()
                .absolute()
                .size_full()
                .child(crate::modal::scrim("cloud-workspaces-scrim"))
                .child(
                    popover("cloud-workspaces-modal", Anchor::Center)
                        .modal_surface(self.workspace.boxy_layout())
                        .width(width)
                        .gap(0.)
                        .max_height(viewport.height.as_f32() - 32.)
                        .priority(60)
                        .on_dismiss(move |window, cx| {
                            let _ = weak
                                .update(cx, |this, cx| this.cloud_workspaces_escape(window, cx));
                        })
                        .child(view),
                )
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cloud_workspaces_dialog::{Action, Env, Event, tests as fixtures};
    use gpui::TestAppContext;
    use monocode_core::cloud_sessions::{CloudBackendKind, CloudRef, CloudSessions};

    #[gpui::test]
    fn the_cloud_setup_entry_opens_the_dialog_and_escape_closes_it(cx: &mut TestAppContext) {
        let (window, _, _) = crate::shell::tests::window(cx);
        let w = fixtures::world_in(window.read_with(cx, |s, _| s.store.clone()).unwrap());
        let env = Env {
            api: Some(w.api.clone()),
            store: w.store.clone(),
        };
        window
            .update(cx, |shell, window, cx| {
                shell.open_cloud_workspaces_with(w.checkout.path(), env, window, cx);
                assert!(shell.cloud_workspaces.is_some());
            })
            .unwrap();
        cx.run_until_parked();
        window
            .update(cx, |shell, window, cx| {
                shell.cloud_workspaces_escape(window, cx);
                assert!(shell.cloud_workspaces.is_none());
            })
            .unwrap();
    }

    #[gpui::test]
    fn deleting_from_the_overview_cleans_the_window_like_the_other_delete_paths(
        cx: &mut TestAppContext,
    ) {
        let (window, _, _) = crate::shell::tests::window(cx);
        let store = window.read_with(cx, |s, _| s.store.clone()).unwrap();
        let w = fixtures::world_in(store.clone());
        let id = w.used_session;
        let used = CloudRef::codespace("octo-used");
        window
            .update(cx, |shell, window, cx| {
                let summary = crate::runtime::locked(&shell.store)
                    .unwrap()
                    .sessions()
                    .unwrap()
                    .into_iter()
                    .find(|s| s.id == id)
                    .unwrap();
                // The window's own cloud service must not reach the real gh.
                shell.cloud = Arc::new(CloudSessions::new(
                    w.gh.clone(),
                    Arc::new(fixtures::FakeHost::default()),
                    shell.store.clone(),
                ));
                shell.session_summaries.insert(id, summary);
                shell.order.push(id);
                shell.cloud_marks.insert(
                    id,
                    cloud::CloudMark {
                        local_project: w.checkout.path(),
                        codespace: "octo-used".into(),
                        kind: CloudBackendKind::Codespaces,
                    },
                );
                let env = Env {
                    api: Some(w.api.clone()),
                    store: w.store.clone(),
                };
                shell.open_cloud_workspaces_with(w.checkout.path(), env, window, cx);
            })
            .unwrap();
        cx.run_until_parked();
        // The user confirms Delete: the dialog asks, the shell adds its busy view and runs it.
        let dialog = window
            .read_with(cx, |shell, _| {
                shell.cloud_workspaces.as_ref().unwrap().view.clone()
            })
            .unwrap();
        dialog.update(cx, |_, cx| {
            cx.emit(Event::Act {
                cloud: used.clone(),
                action: Action::Delete,
            })
        });
        cx.run_until_parked();
        cx.run_until_parked();
        assert_eq!(w.gh.count("codespace delete -c octo-used --force"), 1);
        window
            .read_with(cx, |shell, _| {
                assert!(
                    !shell.cloud_marks.contains_key(&id),
                    "the session is dropped"
                );
                assert!(!shell.session_summaries.contains_key(&id));
                assert!(shell.cloud_gone.contains(&used.key()), "marked gone");
            })
            .unwrap();
        // The other workspaces and their sessions stay.
        assert!(
            store
                .lock()
                .unwrap()
                .remote_session(w.sandbox_session)
                .unwrap()
                .is_some()
        );
    }
}
