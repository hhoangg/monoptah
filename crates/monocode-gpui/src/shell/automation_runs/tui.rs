//! A terminal automation run: the CLI is the run, so it settles from the pane's
//! launch events instead of a chat turn.
use super::*;
use crate::tui_session::TuiEvent;
use monocode_core::tui_launch::{EARLY_EXIT, run_error};

/// A terminal run in flight.
pub(super) struct Run {
    pub(super) run_id: String,
    /// The check made once the CLI has stayed up; a new start replaces it.
    settle: Option<gpui::Task<()>>,
}

const GONE: &str = "The terminal session closed before the run could start.";

impl Shell {
    /// Opens the run's terminal session and starts the CLI with the prompt. The
    /// run is `running` first, then the pane's launch events settle it.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn begin_tui_run(
        &mut self,
        summary: Session,
        settings: &monocode_core::agent::Settings,
        persisted: String,
        due: &DueAutomationRun,
        reveal: bool,
        revision: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let id = summary.id;
        // The pane reads where it runs from here and from the saved settings.
        if let Some(tree) = &settings.worktree_cwd {
            self.session_worktrees.insert(id, Some(tree.clone()));
        }
        self.add(summary, cx);
        self.automation_state
            .session_automations
            .insert(id, due.automation.id.clone());
        // Registered before the write: the pane can report while it is in flight.
        self.automation_state.tui_runs.insert(
            id,
            Run {
                run_id: due.run.id.clone(),
                settle: None,
            },
        );
        self.update_automation_run(due.run.id.clone(), "running", Some(persisted), None, cx);
        self.place_automation_session(id, &due.automation, cx);
        let prompt = due
            .run
            .prompt
            .clone()
            .unwrap_or_else(|| due.automation.prompt.clone());
        match self.ensure_tui(id, cx) {
            Some(tui) => {
                // Not waiting to be shown: a background run has no one looking at it.
                tui.update(cx, |tui, cx| {
                    tui.set_initial_prompt(prompt.trim().to_owned());
                    tui.start(cx);
                });
            }
            None => self.settle_tui_run(
                id,
                "failed",
                Some("The terminal session could not be opened.".into()),
                cx,
            ),
        }
        if reveal && self.navigation_revision == revision {
            self.activate_session(id, window, cx);
        }
        self.persist(cx);
    }

    /// What the pane reports decides the run: the CLI taking the prompt and
    /// staying up is success, dying first is failure.
    pub(in crate::shell) fn automation_tui_event(
        &mut self,
        id: i64,
        event: &TuiEvent,
        cx: &mut Context<Self>,
    ) {
        let Some(run) = self.automation_state.tui_runs.get_mut(&id) else {
            return;
        };
        match event {
            TuiEvent::Started => {
                // A terminal turn has no end the app can see, so the CLI staying
                // up is the success. A relaunch starts the wait over.
                run.settle = Some(cx.spawn(async move |this, cx| {
                    cx.background_executor().timer(EARLY_EXIT).await;
                    let _ = this.update(cx, |this, cx| this.tui_run_stayed_up(id, cx));
                }));
            }
            TuiEvent::Exited(exit) => {
                let kind = self
                    .session_summaries
                    .get(&id)
                    .map_or(Kind::Claude, |summary| Kind::from_id(&summary.provider));
                self.settle_tui_run(id, "failed", Some(run_error(kind, exit)), cx);
            }
            _ => {}
        }
    }

    fn tui_run_stayed_up(&mut self, id: i64, cx: &mut Context<Self>) {
        if self.tui_sessions.contains_key(&id) && self.session_summaries.contains_key(&id) {
            self.settle_tui_run(id, "succeeded", None, cx);
        } else {
            self.automation_tui_gone(id, cx);
        }
    }

    /// The pane is gone (tab closed, session removed): it can never report.
    pub(in crate::shell) fn automation_tui_gone(&mut self, id: i64, cx: &mut Context<Self>) {
        self.settle_tui_run(id, "cancelled", Some(GONE.into()), cx);
    }

    /// Records the outcome once; the entry goes first, so a run settles at most once.
    fn settle_tui_run(
        &mut self,
        id: i64,
        status: &str,
        error: Option<String>,
        cx: &mut Context<Self>,
    ) {
        if let Some(run) = self.automation_state.tui_runs.remove(&id) {
            self.update_automation_run(run.run_id, status, None, error, cx);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shell::surface_tests::{NoClis, assert_chat, known, since};
    use gpui::{TestAppContext, WindowHandle};
    use monocode_core::automations::{Automation, AutomationRun, AutomationUpsert};
    use std::{path::PathBuf, time::Duration};

    const PROMPT: &str = "Check the build";

    /// A folder the runs work in; a repository when a worktree must be made.
    struct Project {
        path: PathBuf,
    }
    impl Project {
        fn new(repo: bool) -> Self {
            let path = std::env::temp_dir().join(format!(
                "monoptah-automation-tui-{}",
                monocode_core::mono::new_id()
            ));
            std::fs::create_dir_all(&path).unwrap();
            let project = Self { path };
            if repo {
                project.git(&["init", "-b", "main"]);
                project.git(&["config", "user.email", "fixture@example.invalid"]);
                project.git(&["config", "user.name", "Fixture"]);
                std::fs::write(project.path.join("file"), "x").unwrap();
                project.git(&["add", "file"]);
                project.git(&["-c", "commit.gpgsign=false", "commit", "-m", "Initial"]);
            }
            project
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
        fn cwd(&self) -> String {
            self.path.to_string_lossy().into_owned()
        }
    }
    impl Drop for Project {
        fn drop(&mut self) {
            let mut trees = self.path.clone().into_os_string();
            trees.push("-worktrees");
            let _ = std::fs::remove_dir_all(PathBuf::from(trees));
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

    /// The shell with an in-memory automation service and `surfaces` as the
    /// providers' session surface preference. The guard keeps every test here
    /// from starting a real provider CLI, whatever path a regression takes.
    fn shell(
        cx: &mut TestAppContext,
        surfaces: &[(&str, Surface)],
    ) -> (WindowHandle<Shell>, NoClis) {
        let clis = NoClis::new();
        let (window, _, _) = crate::shell::tests::window(cx);
        window
            .update(cx, |shell, _, _| {
                shell.automation_state.service = Some(Arc::new(Mutex::new(
                    Service::open(std::path::Path::new(":memory:")).unwrap(),
                )));
                for (provider, surface) in surfaces {
                    shell
                        .workspace
                        .preferences
                        .provider_surfaces
                        .insert((*provider).into(), *surface);
                }
            })
            .unwrap();
        (window, clis)
    }

    /// An automation in `project` and a run of it that is due.
    fn due(
        shell: &Shell,
        project: &Project,
        harness: &str,
        edit: impl FnOnce(&mut AutomationUpsert),
    ) -> DueAutomationRun {
        let mut upsert = AutomationUpsert::new(project.cwd(), harness.into(), String::new());
        upsert.name = "Nightly".into();
        upsert.prompt = PROMPT.into();
        upsert.workspace_mode = "current".into();
        edit(&mut upsert);
        let service = shell.automation_state.service.clone().unwrap();
        let mut service = service.lock().unwrap();
        let automation = service.upsert(upsert).unwrap();
        let run = service.run_now(&automation.id, now()).unwrap();
        DueAutomationRun { automation, run }
    }

    fn launch(window: &WindowHandle<Shell>, cx: &mut TestAppContext, due: &DueAutomationRun) {
        let due = due.clone();
        window
            .update(cx, |shell, _, cx| shell.launch_automation(due, false, cx))
            .unwrap();
        cx.run_until_parked();
    }

    fn runs(
        window: &WindowHandle<Shell>,
        cx: &mut TestAppContext,
        automation: &Automation,
    ) -> Vec<AutomationRun> {
        cx.run_until_parked();
        window
            .update(cx, |shell, _, _| {
                let service = shell.automation_state.service.clone().unwrap();
                service.lock().unwrap().runs(&automation.id).unwrap()
            })
            .unwrap()
    }

    /// The one run's status and error.
    fn outcome(
        window: &WindowHandle<Shell>,
        cx: &mut TestAppContext,
        automation: &Automation,
    ) -> (String, Option<String>) {
        let mut runs = runs(window, cx, automation);
        assert_eq!(runs.len(), 1);
        let run = runs.remove(0);
        (run.status, run.error)
    }

    /// Runs `due` and returns the terminal session it opened.
    fn start(window: &WindowHandle<Shell>, cx: &mut TestAppContext, due: &DueAutomationRun) -> i64 {
        let before = window.update(cx, |shell, _, _| known(shell)).unwrap();
        launch(window, cx, due);
        window
            .update(cx, |shell, _, _| {
                let ids = since(shell, &before);
                assert_eq!(ids.len(), 1, "one session per run");
                ids[0]
            })
            .unwrap()
    }

    fn fake(window: &WindowHandle<Shell>, cx: &mut TestAppContext, id: i64, started: bool) {
        window
            .update(cx, |shell, _, cx| {
                let terminal = shell.tui_sessions[&id]
                    .read(cx)
                    .terminal()
                    .cloned()
                    .expect("the CLI is launched");
                terminal.update(cx, |terminal, cx| {
                    if started {
                        terminal.fake_started(cx)
                    } else {
                        terminal.fake_exit(Some(1), cx)
                    }
                });
            })
            .unwrap();
        cx.run_until_parked();
    }

    // automationSession.test.ts: automationRunSurface
    #[gpui::test]
    fn a_run_follows_a_terminal_preference_when_the_cli_takes_a_first_prompt(
        cx: &mut TestAppContext,
    ) {
        let project = Project::new(false);
        let (window, _clis) = shell(cx, &[("claude", Surface::Tui)]);
        let due = window
            .update(cx, |shell, _, _| due(shell, &project, "claude", |_| {}))
            .unwrap();
        let id = start(&window, cx, &due);
        window
            .update(cx, |shell, _, cx| {
                assert_eq!(shell.session_summaries[&id].surface, Surface::Tui);
                assert!(!shell.sessions.contains_key(&id), "a terminal has no chat");
                let tui = shell.tui_sessions[&id].read(cx).terminal().unwrap();
                let args = tui.read(cx).launched().unwrap().args.clone();
                assert_eq!(args.last().unwrap().to_string_lossy(), PROMPT);
                assert_eq!(shell.session_summaries[&id].title, "Nightly");
                assert_eq!(
                    shell.automation_state.session_automations.get(&id),
                    Some(&due.automation.id)
                );
            })
            .unwrap();
    }

    #[gpui::test]
    fn a_run_is_a_chat_when_the_provider_prefers_one(cx: &mut TestAppContext) {
        let project = Project::new(false);
        let (window, _clis) = shell(cx, &[("claude", Surface::Chat)]);
        let due = window
            .update(cx, |shell, _, _| due(shell, &project, "claude", |_| {}))
            .unwrap();
        let before = window.update(cx, |shell, _, _| known(shell)).unwrap();
        launch(&window, cx, &due);
        window
            .update(cx, |shell, _, _| {
                let ids = since(shell, &before);
                assert_eq!(ids.len(), 1);
                assert_chat(shell, ids[0]);
                assert!(shell.automation_state.tui_runs.is_empty());
            })
            .unwrap();
    }

    #[gpui::test]
    fn a_run_stays_a_chat_when_the_cli_cannot_take_a_first_prompt(cx: &mut TestAppContext) {
        let project = Project::new(false);
        let (window, _clis) = shell(cx, &[("codex", Surface::Tui)]);
        let due = window
            .update(cx, |shell, _, _| due(shell, &project, "codex", |_| {}))
            .unwrap();
        let before = window.update(cx, |shell, _, _| known(shell)).unwrap();
        launch(&window, cx, &due);
        window
            .update(cx, |shell, _, _| {
                let ids = since(shell, &before);
                assert_eq!(ids.len(), 1);
                assert_chat(shell, ids[0]);
            })
            .unwrap();
    }

    #[gpui::test]
    fn a_terminal_run_never_continues_an_earlier_session(cx: &mut TestAppContext) {
        let project = Project::new(false);
        let (window, _clis) = shell(cx, &[("claude", Surface::Tui)]);
        // A chat of the same provider in the same folder, which a chat run would continue.
        let sibling = window
            .update(cx, |shell, _, cx| {
                let mut summary = locked(&shell.store)
                    .unwrap()
                    .create("claude", &project.cwd())
                    .unwrap();
                summary.has_messages = true;
                let id = summary.id;
                shell.add(summary, cx);
                id
            })
            .unwrap();
        let due = window
            .update(cx, |shell, _, _| {
                due(shell, &project, "claude", |upsert| {
                    upsert.reuse_session = true;
                })
            })
            .unwrap();
        let due = DueAutomationRun {
            automation: Automation {
                last_session_id: Some(sibling.to_string()),
                ..due.automation
            },
            run: due.run,
        };
        let id = start(&window, cx, &due);
        window
            .update(cx, |shell, _, _| {
                assert_ne!(id, sibling);
                assert!(shell.is_terminal(id));
                assert!(
                    !shell
                        .automation_state
                        .session_automations
                        .contains_key(&sibling)
                );
            })
            .unwrap();
    }

    #[gpui::test]
    fn the_run_is_recorded_as_running_with_its_session(cx: &mut TestAppContext) {
        let project = Project::new(false);
        let (window, _clis) = shell(cx, &[("claude", Surface::Tui)]);
        let due = window
            .update(cx, |shell, _, _| due(shell, &project, "claude", |_| {}))
            .unwrap();
        let id = start(&window, cx, &due);
        let runs = runs(&window, cx, &due.automation);
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].status, "running");
        assert_eq!(runs[0].session_id.as_deref(), Some(id.to_string().as_str()));
        assert_eq!(runs[0].error, None);
    }

    #[gpui::test]
    fn a_new_worktree_run_makes_the_worktree_before_the_cli_starts(cx: &mut TestAppContext) {
        let project = Project::new(true);
        let (window, _clis) = shell(cx, &[("claude", Surface::Tui)]);
        let due = window
            .update(cx, |shell, _, _| {
                due(shell, &project, "claude", |upsert| {
                    upsert.workspace_mode = "worktree".into()
                })
            })
            .unwrap();
        let id = start(&window, cx, &due);
        window
            .update(cx, |shell, _, cx| {
                let settings = locked(&shell.store)
                    .unwrap()
                    .agent_settings(id)
                    .unwrap()
                    .unwrap();
                let tree = settings.worktree_cwd.clone().expect("a worktree");
                assert!(std::path::Path::new(&tree).is_dir());
                assert!(!settings.new_worktree, "nothing is left to create later");
                assert_eq!(settings.worktree_base, None);
                assert_eq!(shell.session_worktrees.get(&id), Some(&Some(tree.clone())));
                // The CLI starts inside it, not in the project folder.
                let terminal = shell.tui_sessions[&id].read(cx).terminal().unwrap();
                assert_eq!(terminal.read(cx).cwd, tree);
                assert_ne!(tree, project.cwd());
            })
            .unwrap();
    }

    #[gpui::test]
    fn a_worktree_that_cannot_be_made_fails_the_run_and_leaves_no_session(cx: &mut TestAppContext) {
        // Not a repository, so there is nothing to branch from.
        let project = Project::new(false);
        let (window, _clis) = shell(cx, &[("claude", Surface::Tui)]);
        let due = window
            .update(cx, |shell, _, _| {
                due(shell, &project, "claude", |upsert| {
                    upsert.workspace_mode = "worktree".into()
                })
            })
            .unwrap();
        let (before, rows) = window
            .update(cx, |shell, _, _| {
                (
                    known(shell),
                    locked(&shell.store).unwrap().sessions().unwrap().len(),
                )
            })
            .unwrap();
        launch(&window, cx, &due);
        let (status, error) = outcome(&window, cx, &due.automation);
        assert_eq!(status, "failed");
        assert!(error.is_some_and(|error| !error.is_empty()));
        window
            .update(cx, |shell, _, _| {
                assert_eq!(known(shell), before);
                assert!(shell.tui_sessions.is_empty());
                assert_eq!(
                    locked(&shell.store).unwrap().sessions().unwrap().len(),
                    rows
                );
            })
            .unwrap();
    }

    #[gpui::test]
    fn the_run_succeeds_once_the_cli_has_stayed_up_five_seconds(cx: &mut TestAppContext) {
        let project = Project::new(false);
        let (window, _clis) = shell(cx, &[("claude", Surface::Tui)]);
        let due = window
            .update(cx, |shell, _, _| due(shell, &project, "claude", |_| {}))
            .unwrap();
        let id = start(&window, cx, &due);
        // Launching is not starting: the clock runs from the process.
        cx.executor().advance_clock(Duration::from_secs(30));
        assert_eq!(outcome(&window, cx, &due.automation).0, "running");
        fake(&window, cx, id, true);
        cx.executor().advance_clock(Duration::from_millis(4900));
        assert_eq!(outcome(&window, cx, &due.automation).0, "running");
        cx.executor().advance_clock(Duration::from_millis(200));
        assert_eq!(
            outcome(&window, cx, &due.automation),
            ("succeeded".into(), None)
        );
        window
            .update(cx, |shell, _, _| {
                assert!(shell.automation_state.tui_runs.is_empty())
            })
            .unwrap();
    }

    #[gpui::test]
    fn an_exit_before_five_seconds_fails_the_run_with_the_exit_notice(cx: &mut TestAppContext) {
        let project = Project::new(false);
        let (window, _clis) = shell(cx, &[("claude", Surface::Tui)]);
        let due = window
            .update(cx, |shell, _, _| due(shell, &project, "claude", |_| {}))
            .unwrap();
        let id = start(&window, cx, &due);
        fake(&window, cx, id, true);
        fake(&window, cx, id, false);
        let (status, error) = outcome(&window, cx, &due.automation);
        assert_eq!(status, "failed");
        let error = error.unwrap();
        assert!(error.starts_with("Process exited (code 1). "), "{error}");
        assert!(error.contains("stopped right after it started"), "{error}");
        // The pending timer must not turn it into a success.
        cx.executor().advance_clock(Duration::from_secs(10));
        assert_eq!(outcome(&window, cx, &due.automation).0, "failed");
    }

    #[gpui::test]
    fn a_cli_that_cannot_start_fails_the_run_with_the_reason(cx: &mut TestAppContext) {
        let project = Project::new(false);
        let (window, _clis) = shell(cx, &[("claude", Surface::Tui)]);
        let due = window
            .update(cx, |shell, _, _| due(shell, &project, "claude", |_| {}))
            .unwrap();
        let id = start(&window, cx, &due);
        window
            .update(cx, |shell, _, cx| {
                let terminal = shell.tui_sessions[&id]
                    .read(cx)
                    .terminal()
                    .cloned()
                    .unwrap();
                terminal.update(cx, |t, cx| t.fake_spawn_error("binary not found", cx));
            })
            .unwrap();
        cx.run_until_parked();
        assert_eq!(
            outcome(&window, cx, &due.automation),
            (
                "failed".into(),
                Some("Could not start Claude Code. binary not found".into())
            )
        );
    }

    #[gpui::test]
    fn closing_the_session_before_the_run_settles_cancels_it(cx: &mut TestAppContext) {
        let project = Project::new(false);
        let (window, _clis) = shell(cx, &[("claude", Surface::Tui)]);
        let due = window
            .update(cx, |shell, _, _| due(shell, &project, "claude", |_| {}))
            .unwrap();
        let id = start(&window, cx, &due);
        fake(&window, cx, id, true);
        window
            .update(cx, |shell, window, cx| shell.close_pane(id, window, cx))
            .unwrap();
        cx.run_until_parked();
        let (status, error) = outcome(&window, cx, &due.automation);
        assert_eq!(status, "cancelled");
        assert_eq!(
            error.as_deref(),
            Some("The terminal session closed before the run could start.")
        );
        cx.executor().advance_clock(Duration::from_secs(10));
        assert_eq!(outcome(&window, cx, &due.automation).0, "cancelled");
    }

    #[gpui::test]
    fn a_pane_that_is_gone_when_the_timer_fires_cancels_the_run(cx: &mut TestAppContext) {
        let project = Project::new(false);
        let (window, _clis) = shell(cx, &[("claude", Surface::Tui)]);
        let due = window
            .update(cx, |shell, _, _| due(shell, &project, "claude", |_| {}))
            .unwrap();
        let id = start(&window, cx, &due);
        fake(&window, cx, id, true);
        // Gone without a stop reaching the run: the timer is the last check.
        window
            .update(cx, |shell, _, _| {
                shell.tui_sessions.remove(&id);
            })
            .unwrap();
        cx.executor().advance_clock(Duration::from_secs(6));
        assert_eq!(outcome(&window, cx, &due.automation).0, "cancelled");
    }

    #[gpui::test]
    fn a_terminal_run_is_not_started_twice(cx: &mut TestAppContext) {
        let project = Project::new(false);
        let (window, _clis) = shell(cx, &[("claude", Surface::Tui)]);
        let due = window
            .update(cx, |shell, _, _| due(shell, &project, "claude", |_| {}))
            .unwrap();
        start(&window, cx, &due);
        let before = window.update(cx, |shell, _, _| known(shell)).unwrap();
        launch(&window, cx, &due);
        window
            .update(cx, |shell, _, _| assert_eq!(known(shell), before))
            .unwrap();
    }
}
