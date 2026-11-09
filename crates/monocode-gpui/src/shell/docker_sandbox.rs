//! The Docker Sandbox setup dialog opened from the project menu's Cloud setup submenu, and
//! the Git credential dialog it opens on top of itself.
use super::*;
use crate::docker_dialog::{self, DockerDialog, model::AccountChoice};
use crate::git_credential_dialog::{self, GitCredentialDialog};
use gpui::AnyView;

pub(super) struct OpenDocker {
    view: Entity<DockerDialog>,
    credential: Option<OpenCredential>,
    _events: Subscription,
}

struct OpenCredential {
    view: Entity<GitCredentialDialog>,
    _events: Subscription,
}

impl Shell {
    /// The Claude accounts that hold a setup-token or an API key.
    fn docker_accounts(&self, cx: &Context<Self>) -> Vec<AccountChoice> {
        docker_dialog::claude_accounts(cx)
    }

    pub(super) fn open_docker_dialog(
        &mut self,
        path: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let env = docker_dialog::Env::system(self.store.clone(), self.docker_accounts(cx));
        self.open_docker_dialog_with(path, env, window, cx);
    }

    pub(super) fn open_docker_dialog_with(
        &mut self,
        path: String,
        env: docker_dialog::Env,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.close_menu(cx);
        self.close_preview(cx);
        self.docker_sandbox = None;
        let name = self.project_label(&path);
        let (store, git) = (env.store.clone(), env.git.clone());
        let view = cx.new(|cx| DockerDialog::new(path, name, env, cx));
        let events = cx.subscribe_in(
            &view,
            window,
            move |this, _, event: &docker_dialog::Event, window, cx| match event {
                docker_dialog::Event::Close => this.close_docker_dialog(window, cx),
                docker_dialog::Event::ManageCredentials { repo_url, selected } => {
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
            },
        );
        window.focus(&view.focus_handle(cx), cx);
        self.docker_sandbox = Some(OpenDocker {
            view,
            credential: None,
            _events: events,
        });
        cx.notify();
    }

    pub(super) fn close_docker_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(open) = self.docker_sandbox.take() {
            if let Some(credential) = open.credential {
                credential.view.update(cx, |view, cx| view.shutdown(cx));
            }
            self.focus_workspace(window, cx);
            cx.notify();
        }
    }

    /// Escape closes the credential dialog first, then the Docker dialog.
    pub(super) fn docker_escape(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self
            .docker_sandbox
            .as_ref()
            .is_some_and(|open| open.credential.is_some())
        {
            self.close_git_credential_dialog(window, cx);
        } else {
            self.close_docker_dialog(window, cx);
        }
    }

    pub(super) fn open_git_credential_dialog(
        &mut self,
        repo_url: String,
        selected: Option<String>,
        env: git_credential_dialog::Env,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(open) = &self.docker_sandbox else {
            return;
        };
        let docker = open.view.clone();
        let name = docker.read(cx).project_name().to_owned();
        let view = cx.new(|cx| GitCredentialDialog::new(name, repo_url, selected, env, cx));
        let events = cx.subscribe_in(
            &view,
            window,
            move |this, _, event: &git_credential_dialog::Event, window, cx| match event {
                git_credential_dialog::Event::Close => this.close_git_credential_dialog(window, cx),
                git_credential_dialog::Event::Chosen(id) => {
                    let id = id.clone();
                    docker.update(cx, |dialog, cx| {
                        dialog.reload_credentials(Some(id), cx);
                    });
                    this.close_git_credential_dialog(window, cx);
                }
            },
        );
        window.focus(&view.focus_handle(cx), cx);
        if let Some(open) = &mut self.docker_sandbox {
            open.credential = Some(OpenCredential {
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
        let Some(open) = &mut self.docker_sandbox else {
            return;
        };
        if let Some(credential) = open.credential.take() {
            credential.view.update(cx, |view, cx| view.shutdown(cx));
            let docker = open.view.clone();
            window.focus(&docker.focus_handle(cx), cx);
            cx.notify();
        }
    }

    fn cloud_modal(
        &self,
        id: &'static str,
        view: impl Into<AnyView>,
        priority: usize,
        close: fn(&mut Shell, &mut Window, &mut Context<Shell>),
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let viewport = window.viewport_size();
        let width = 460f32.min(viewport.width.as_f32() - 24.);
        let weak = cx.weak_entity();
        div()
            .absolute()
            .size_full()
            .child(crate::modal::scrim(format!("{id}-scrim")))
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

    pub(super) fn docker_modal(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let view = self.docker_sandbox.as_ref()?.view.clone();
        Some(self.cloud_modal(
            "docker-modal",
            view,
            60,
            |this, window, cx| this.close_docker_dialog(window, cx),
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
            .docker_sandbox
            .as_ref()?
            .credential
            .as_ref()?
            .view
            .clone();
        Some(self.cloud_modal(
            "git-credential-modal",
            view,
            61,
            |this, window, cx| this.close_git_credential_dialog(window, cx),
            window,
            cx,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::popover::MenuItem;
    use crate::shell::tests::window;
    use gpui::{TestAppContext, VisualTestContext};
    use monocode_core::{
        git_credentials::{GitOutput, GitRunner, Secret},
        sandboxes::{SbxOutput, SbxRunner},
        store::Store,
    };
    use std::sync::Mutex;

    struct Sbx(Mutex<usize>);
    impl SbxRunner for Sbx {
        fn run(&self, _: &[String], _: Option<&str>) -> Result<SbxOutput, String> {
            *self.0.lock().unwrap() += 1;
            Ok(SbxOutput {
                success: true,
                stdout: "[]".into(),
                stderr: String::new(),
            })
        }
    }
    struct Git;
    impl GitRunner for Git {
        fn run(
            &self,
            _: &[String],
            _: &[(String, String)],
            _: &Secret,
        ) -> Result<GitOutput, String> {
            Ok(GitOutput {
                success: true,
                stderr: String::new(),
            })
        }
    }

    fn env() -> (docker_dialog::Env, std::path::PathBuf, Arc<Sbx>) {
        let dir = std::env::temp_dir().join(format!(
            "monoptah-docker-shell-{}",
            monocode_core::mono::new_id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let store = Store::open(&dir.join("monocode.db")).unwrap();
        let sbx = Arc::new(Sbx(Mutex::new(0)));
        let env = docker_dialog::Env {
            sbx: sbx.clone(),
            git: Arc::new(Git),
            store: Arc::new(Mutex::new(store)),
            home: Some(dir.clone()),
            origin: |_| Some("https://github.com/acme/alpha".into()),
            accounts: Vec::new(),
        };
        (env, dir, sbx)
    }

    fn project_items(
        shell: &mut Shell,
        path: &str,
        w: &mut Window,
        cx: &mut Context<Shell>,
    ) -> Vec<MenuItem> {
        let target = MenuTarget::Project {
            path: path.into(),
            rail: false,
        };
        shell.menu_items(&target, &mut Vec::new(), w, cx).1
    }

    #[gpui::test]
    fn cloud_setup_is_a_submenu_for_local_projects_only(cx: &mut TestAppContext) {
        let (window, _, _) = window(cx);
        window
            .update(cx, |shell, w, cx| {
                let items = project_items(shell, "/work/monocode", w, cx);
                let labels: Vec<_> = items
                    .iter()
                    .filter_map(|i| match i {
                        MenuItem::Action(a) => Some(a.label.to_string()),
                        _ => None,
                    })
                    .collect();
                assert!(
                    !labels.iter().any(|l| l.starts_with("Codespaces")),
                    "{labels:?}"
                );
                let at = labels
                    .iter()
                    .position(|l| l == "Cloud setup")
                    .expect("Cloud setup");
                assert_eq!(labels[at - 1], "Telegram…");
                let Some(MenuItem::Action(cloud)) = items
                    .iter()
                    .find(|i| matches!(i, MenuItem::Action(a) if a.id == "cloud-setup"))
                else {
                    panic!("no cloud-setup item");
                };
                assert!(
                    !cloud.submenu.is_empty(),
                    "it opens a submenu instead of running something"
                );
                let sub: Vec<_> = cloud
                    .submenu
                    .iter()
                    .map(|a| (a.id.to_string(), a.label.to_string()))
                    .collect();
                assert_eq!(
                    sub,
                    [
                        ("codespaces".to_owned(), "Codespaces".to_owned()),
                        ("docker-sandbox".to_owned(), "Docker Sandbox".to_owned()),
                    ]
                );
                let remote = project_items(shell, "remote://box/work/monocode", w, cx);
                assert!(
                    !remote
                        .iter()
                        .any(|i| matches!(i, MenuItem::Action(a) if a.id == "cloud-setup"))
                );
            })
            .unwrap();
    }

    #[gpui::test]
    fn choosing_the_parent_row_opens_no_dialog(cx: &mut TestAppContext) {
        let (window, _, _) = window(cx);
        window
            .update(cx, |shell, w, cx| {
                shell.run_menu(
                    MenuTarget::Project {
                        path: "/work/monocode".into(),
                        rail: false,
                    },
                    "cloud-setup",
                    None,
                    w,
                    cx,
                );
                assert!(shell.codespaces.is_none() && shell.docker_sandbox.is_none());
            })
            .unwrap();
    }

    #[gpui::test]
    fn the_codespaces_entry_still_opens_its_dialog(cx: &mut TestAppContext) {
        let (window, _, _) = window(cx);
        window
            .update(cx, |shell, w, cx| {
                shell.run_menu(
                    MenuTarget::Project {
                        path: "/work/monocode".into(),
                        rail: false,
                    },
                    "codespaces",
                    None,
                    w,
                    cx,
                );
                assert!(shell.codespaces.is_some() && shell.docker_sandbox.is_none());
            })
            .unwrap();
    }

    #[gpui::test]
    fn the_docker_dialog_opens_draws_and_stacks_the_credential_dialog(cx: &mut TestAppContext) {
        let (window, _, _) = window(cx);
        cx.update(crate::input::bind);
        let (env, dir, sbx) = env();
        window
            .update(cx, |shell, w, cx| {
                shell.open_docker_dialog_with("/work/monocode".into(), env, w, cx)
            })
            .unwrap();
        cx.run_until_parked();
        cx.refresh().unwrap();
        assert_eq!(*sbx.0.lock().unwrap(), 1, "one read-only access check");
        let visual = &mut VisualTestContext::from_window(window.into(), cx);
        for selector in [
            "docker-dialog",
            "docker-field-repo",
            "docker-save",
            "docker-save-note",
        ] {
            assert!(visual.debug_bounds(selector).is_some(), "{selector}");
        }
        // Managing credentials opens a child modal on top; Escape closes it first.
        let docker = window
            .read_with(visual, |shell, _| {
                shell.docker_sandbox.as_ref().unwrap().view.clone()
            })
            .unwrap();
        docker.update(visual, |_, cx| {
            cx.emit(docker_dialog::Event::ManageCredentials {
                repo_url: "https://github.com/acme/alpha".into(),
                selected: None,
            })
        });
        visual.run_until_parked();
        visual.refresh().unwrap();
        assert!(visual.debug_bounds("git-credential-dialog").is_some());
        window
            .update(visual, |shell, w, cx| shell.docker_escape(w, cx))
            .unwrap();
        window
            .read_with(visual, |shell, _| {
                let open = shell.docker_sandbox.as_ref().expect("docker dialog stays");
                assert!(open.credential.is_none());
            })
            .unwrap();
        window
            .update(visual, |shell, w, cx| shell.docker_escape(w, cx))
            .unwrap();
        window
            .read_with(visual, |shell, _| assert!(shell.docker_sandbox.is_none()))
            .unwrap();
        assert_eq!(*sbx.0.lock().unwrap(), 1, "closing runs nothing");
        std::fs::remove_dir_all(dir).ok();
    }
}
