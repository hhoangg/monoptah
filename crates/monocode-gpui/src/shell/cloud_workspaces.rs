//! The Cloud workspaces dialog opened from the project menu's "Cloud workspaces…" item (next to
//! "Cloud settings…"), and what the shell does with its Stop and Delete: adds its busy view (a copy),
//! and on a delete forgets the sessions of the deleted workspace in the window and the Sub-agents
//! panel. The dialog stays open while a stop or delete runs, so the result always gets here.
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
                cloud_workspaces_dialog::Event::Act {
                    target,
                    action,
                    confirmed,
                    acknowledged,
                } => {
                    // The dialog runs the call; what is busy is the shell's, copied now (it does
                    // not follow what starts during a wait, see `CloudWorkspacesDialog::run`).
                    let busy = this.cloud_busy_snapshot(cx);
                    let (target, action, confirmed) = (target.clone(), *action, confirmed.clone());
                    let acknowledged = *acknowledged;
                    dialog.update(cx, |dialog, cx| {
                        dialog.run(target, action, confirmed, acknowledged, busy, cx)
                    });
                }
                cloud_workspaces_dialog::Event::Stopped(cloud) => {
                    this.cloud_states.remove(&cloud.key());
                    this.refresh_cloud(cx);
                    this.cloud_chip_changed(cx);
                }
                cloud_workspaces_dialog::Event::Deleted { cloud, removed } => {
                    this.forget_deleted_workspace(cloud.clone(), removed.clone(), window, cx);
                    this.cloud_chip_changed(cx);
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

    /// A stop or delete changed the workspaces: the footer's cloud chip reads them again.
    fn cloud_chip_changed(&self, cx: &mut Context<Self>) {
        let chip = self.usage_footer.read(cx).cloud_chip().clone();
        chip.update(cx, |chip, cx| chip.changed(cx));
    }

    /// Closes the dialog, unless a stop or delete is running: closing would drop the dialog and
    /// the subscription that carries the result back here, and the window would keep sessions of
    /// a workspace that is gone.
    pub(super) fn close_cloud_workspaces(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(open) = &self.cloud_workspaces {
            if open.view.read(cx).working() {
                return;
            }
            self.cloud_workspaces = None;
            self.focus_workspace(window, cx);
            cx.notify();
        }
    }

    /// Escape leaves the delete confirmation first, then closes the dialog.
    pub(super) fn cloud_workspaces_escape(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(open) = &self.cloud_workspaces else {
            return;
        };
        // A running stop or delete keeps the dialog open; `close_cloud_workspaces` checks that.
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
        // The shared cloud dialog frame: a barrier (the window behind gets no hover, click or
        // scroll) and the modal surface.
        Some(self.cloud_modal_sized(
            "cloud-workspaces-modal",
            view,
            60,
            640.,
            |this, window, cx| this.cloud_workspaces_escape(window, cx),
            window,
            cx,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cloud_workspaces_dialog::{Action, Env, Event, tests as fixtures};
    use gpui::{TestAppContext, WindowHandle};
    use monocode_core::cloud_sessions::{CloudBackendKind, CloudRef, CloudSessions};

    #[gpui::test]
    fn the_menu_entry_opens_the_dialog_and_escape_closes_it(cx: &mut TestAppContext) {
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
    fn the_footer_chip_follows_the_local_project_of_a_cloud_session_and_opens_the_dialog(
        cx: &mut TestAppContext,
    ) {
        let (window, _, _) = crate::shell::tests::window(cx);
        let store = window.read_with(cx, |s, _| s.store.clone()).unwrap();
        let w = fixtures::world_in(store);
        let env = Env {
            api: Some(w.api.clone()),
            store: w.store.clone(),
        };
        let chip = window
            .read_with(cx, |s, cx| s.usage_footer.read(cx).cloud_chip().clone())
            .unwrap();
        // The open session runs in a codespace: its folder is `remote://...`, the chip reports on
        // the local project it belongs to.
        window
            .update(cx, |shell, _, cx| {
                let pane = shell.workspace.active.unwrap();
                let id = shell.workspace.session_for_pane(pane);
                shell.cloud_marks.insert(
                    id,
                    cloud::CloudMark {
                        local_project: w.checkout.path(),
                        codespace: "octo-used".into(),
                        kind: CloudBackendKind::Codespaces,
                    },
                );
                shell.update_usage_provider(cx);
            })
            .unwrap();
        chip.update(cx, |chip, cx| {
            assert_eq!(chip.project(), w.checkout.path());
            chip.set_env(env, cx);
            chip.set_window_active(true, cx);
        });
        cx.run_until_parked();
        assert!(chip.read_with(cx, |chip, _| chip.visible()));

        // Clicking it opens the same dialog the menu item opens.
        let mut visual = gpui::VisualTestContext::from_window(window.into(), cx);
        visual.refresh().unwrap();
        visual.run_until_parked();
        let at = visual
            .debug_bounds("cloud-status-chip")
            .expect("the chip is in the footer")
            .center();
        window
            .read_with(&visual, |shell, _| {
                assert!(shell.cloud_workspaces.is_none())
            })
            .unwrap();
        visual.simulate_click(at, Default::default());
        visual.run_until_parked();
        window
            .read_with(&visual, |shell, cx| {
                let open = shell.cloud_workspaces.as_ref().expect("the dialog opened");
                assert!(open.view.read(cx).view().is_some(), "and it loaded");
            })
            .unwrap();
        // Escape closes it, as for the menu entry.
        window
            .update(&mut visual, |shell, window, cx| {
                shell.cloud_workspaces_escape(window, cx);
                assert!(shell.cloud_workspaces.is_none());
            })
            .unwrap();
    }

    /// A window whose footer chip reads the fixtures' fake `gh` and `sbx` (never the real ones).
    fn window_with_chip(
        cx: &mut TestAppContext,
    ) -> (
        WindowHandle<Shell>,
        fixtures::World,
        Entity<crate::usage_footer::cloud_chip::CloudChip>,
    ) {
        let (window, _, _) = crate::shell::tests::window(cx);
        let store = window.read_with(cx, |s, _| s.store.clone()).unwrap();
        let w = fixtures::world_in(store);
        let env = Env {
            api: Some(w.api.clone()),
            store: w.store.clone(),
        };
        let chip = window
            .read_with(cx, |s, cx| s.usage_footer.read(cx).cloud_chip().clone())
            .unwrap();
        chip.update(cx, |chip, cx| {
            chip.set_env(env, cx);
            chip.set_window_active(true, cx);
        });
        (window, w, chip)
    }

    fn chip_reports(
        chip: &Entity<crate::usage_footer::cloud_chip::CloudChip>,
        project: &str,
        cx: &mut TestAppContext,
    ) {
        cx.run_until_parked();
        chip.read_with(cx, |chip, _| {
            assert_eq!(chip.project(), project);
            assert!(chip.visible(), "{:?}", chip.status());
        });
    }

    #[gpui::test]
    fn the_footer_chip_stays_on_the_mono_tab_and_the_app_views(cx: &mut TestAppContext) {
        let (window, w, chip) = window_with_chip(cx);
        let project = w.checkout.path();
        // The window is on a project that uses cloud, and shows a session of it.
        window
            .update(cx, |shell, window, cx| {
                shell.workspace.add_project(&project);
                shell.select_project(project.clone(), window, cx);
            })
            .unwrap();
        chip_reports(&chip, &project, cx);

        // The MonoCrab tab: a Mono chat has its own cwd, the chip still reports on the project.
        window
            .update(cx, |shell, window, cx| {
                let project = shell.workspace.project.clone();
                shell.mono_manager().update(cx, |monos, cx| {
                    let id = monos.roster.create(vec![project]).id.clone();
                    monos.open(&id, window, cx);
                });
                assert!(shell.mono_active(cx));
            })
            .unwrap();
        chip_reports(&chip, &project, cx);

        // The app views, one after the other.
        window
            .update(cx, |shell, window, cx| shell.open_today(window, cx))
            .unwrap();
        chip_reports(&chip, &project, cx);
        window
            .update(cx, |shell, window, cx| shell.open_inbox(window, cx))
            .unwrap();
        chip_reports(&chip, &project, cx);
        window
            .update(cx, |shell, window, cx| shell.open_notes(window, cx))
            .unwrap();
        window
            .read_with(cx, |shell, _| assert!(shell.app_view_open()))
            .unwrap();
        chip_reports(&chip, &project, cx);
    }

    #[gpui::test]
    fn the_footer_chip_keeps_the_local_project_of_a_cloud_session_across_tabs(
        cx: &mut TestAppContext,
    ) {
        let (window, w, chip) = window_with_chip(cx);
        let project = w.checkout.path();
        window
            .update(cx, |shell, _, cx| {
                let pane = shell.workspace.active.unwrap();
                let id = shell.workspace.session_for_pane(pane);
                shell.cloud_marks.insert(
                    id,
                    cloud::CloudMark {
                        local_project: project.clone(),
                        codespace: "octo-used".into(),
                        kind: CloudBackendKind::Codespaces,
                    },
                );
                // The window is on a folder that is not the session's project (a restored
                // window, a remote folder): the session's mark decides.
                shell.workspace.project = "remote://host/elsewhere".into();
                shell.invalidate_chrome(cx);
            })
            .unwrap();
        chip_reports(&chip, &project, cx);

        // Leaving the session for the Mono tab: the window's project, no longer a remote one.
        window
            .update(cx, |shell, window, cx| {
                shell.workspace.project = project.clone();
                let id = shell.workspace.project.clone();
                shell.mono_manager().update(cx, |monos, cx| {
                    let mono = monos.roster.create(vec![id]).id.clone();
                    monos.open(&mono, window, cx);
                });
            })
            .unwrap();
        chip_reports(&chip, &project, cx);
    }

    #[gpui::test]
    fn the_footer_chip_follows_the_project_and_hides_for_one_that_does_not_use_cloud(
        cx: &mut TestAppContext,
    ) {
        let (window, w, chip) = window_with_chip(cx);
        let project = w.checkout.path();
        // A project that never used cloud: no session, no Cloud settings.
        let plain = fixtures::Checkout::new();
        let plain_project = plain.path();
        window
            .update(cx, |shell, window, cx| {
                shell.workspace.add_project(&project);
                shell.workspace.add_project(&plain_project);
                shell.select_project(plain_project.clone(), window, cx);
            })
            .unwrap();
        cx.run_until_parked();
        chip.read_with(cx, |chip, _| {
            assert_eq!(chip.project(), plain_project);
            assert!(!chip.visible());
        });
        let asked = w.gh.count("codespace list");

        // Its Mono tab and app views do not show it either, and never start `gh`.
        window
            .update(cx, |shell, window, cx| {
                let project = shell.workspace.project.clone();
                shell.mono_manager().update(cx, |monos, cx| {
                    let id = monos.roster.create(vec![project]).id.clone();
                    monos.open(&id, window, cx);
                });
            })
            .unwrap();
        cx.run_until_parked();
        window
            .update(cx, |shell, window, cx| shell.open_today(window, cx))
            .unwrap();
        cx.run_until_parked();
        chip.read_with(cx, |chip, _| {
            assert_eq!(chip.project(), plain_project);
            assert!(!chip.visible());
        });
        assert_eq!(w.gh.count("codespace list"), asked);

        // Back to the project that uses cloud: the chip is there, on every tab of it.
        window
            .update(cx, |shell, window, cx| {
                shell.select_project(project.clone(), window, cx)
            })
            .unwrap();
        chip_reports(&chip, &project, cx);
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
                target: fixtures::target(used.clone()),
                action: Action::Delete,
                confirmed: vec![],
                acknowledged: false,
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

    #[gpui::test]
    fn closing_during_a_delete_is_refused_and_the_result_still_cleans_the_window(
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
        let dialog = window
            .read_with(cx, |shell, _| {
                shell.cloud_workspaces.as_ref().unwrap().view.clone()
            })
            .unwrap();
        // The user confirmed Delete and presses Escape (or Close) while it runs.
        dialog.update(cx, |_, cx| {
            cx.emit(Event::Act {
                target: fixtures::target(used.clone()),
                action: Action::Delete,
                confirmed: vec![],
                acknowledged: false,
            })
        });
        window
            .update(cx, |shell, window, cx| {
                // The event ran the delete on a worker; it has not finished.
                assert!(dialog.read(cx).working());
                shell.cloud_workspaces_escape(window, cx);
                assert!(shell.cloud_workspaces.is_some(), "Escape is ignored");
                shell.close_cloud_workspaces(window, cx);
                assert!(shell.cloud_workspaces.is_some(), "Close is ignored");
            })
            .unwrap();
        // The result reaches the shell, which forgets the deleted workspace's sessions.
        cx.run_until_parked();
        cx.run_until_parked();
        assert_eq!(w.gh.count("codespace delete -c octo-used --force"), 1);
        window
            .read_with(cx, |shell, _| {
                assert!(!shell.cloud_marks.contains_key(&id));
                assert!(shell.cloud_gone.contains(&used.key()), "marked gone");
            })
            .unwrap();
        // Now it can be closed.
        window
            .update(cx, |shell, window, cx| {
                shell.cloud_workspaces_escape(window, cx);
                assert!(shell.cloud_workspaces.is_none());
            })
            .unwrap();
    }
}
