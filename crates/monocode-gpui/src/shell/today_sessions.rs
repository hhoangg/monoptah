//! Sessions of internal tasks (Today items): feeds the Today page what only the
//! shell knows about them, and carries out "continue in an existing session".
use super::*;
use crate::{
    session_mark::State as MarkState,
    today::{self, SessionRow},
};
use monocode_core::today::{Task, mention};

/// How many of a project's recent sessions the Continue menu can offer.
const CONTINUE_LIMIT: usize = 40;

impl Shell {
    /// One session as the Today page shows it, or `None` when the shell does not
    /// have it (deleted, or on its way out).
    fn today_session_row(&self, id: i64, cx: &App) -> Option<SessionRow> {
        if !self.session_summaries.contains_key(&id) || self.deleting.contains(&id) {
            return None;
        }
        let meta = self.session_metadata(id, cx);
        // The tab's mark also knows a session waiting for approval.
        let state = match self.tab_marks.get(&id) {
            Some(mark) => mark.read(cx).state(),
            None if meta.busy => MarkState::Working,
            None => MarkState::Idle,
        };
        let branch = self
            .sessions
            .get(&id)
            .and_then(|view| view.read(cx).sidebar_details())
            .and_then(|(_, branch)| branch)
            .or_else(|| self.session_details.get(&id).and_then(|d| d.branch.clone()));
        Some(SessionRow {
            id,
            title: meta.display_title(),
            provider: meta.summary.provider.clone(),
            state,
            archived: meta.summary.archived,
            updated_at: meta.summary.updated_at,
            branch,
        })
    }

    /// Pushes the selected task's sessions to the Today page while it is on
    /// screen: its linked ones, and the project's recent chats to continue in.
    /// Cheap when nothing changed; `Today` ignores an equal snapshot.
    pub(super) fn sync_today_sessions(&self, cx: &mut Context<Self>) {
        let Some(today) = self.today.clone() else {
            return;
        };
        if self.active_app_view != Some(app_views::AppView::Today) {
            return;
        }
        let (ids, project) = today.read(cx).session_needs();
        let linked: Vec<SessionRow> = ids
            .into_iter()
            .filter_map(|id| self.today_session_row(id, cx))
            .collect();
        let mut recent: Vec<&Session> = project
            .iter()
            .flat_map(|project| {
                self.session_summaries
                    .values()
                    .filter(|s| s.cwd == *project)
            })
            // A terminal session has no composer to leave a mention in.
            .filter(|s| !s.archived && !runs_in_terminal(s))
            .collect();
        recent.sort_by_key(|s| std::cmp::Reverse(s.updated_at));
        let candidates: Vec<SessionRow> = recent
            .into_iter()
            .take(CONTINUE_LIMIT)
            .filter_map(|s| self.today_session_row(s.id, cx))
            .collect();
        today.update(cx, |today, cx| {
            today.set_session_rows(linked, candidates, cx)
        });
    }

    /// Links an existing chat session to the task, opens it and leaves the
    /// task's `@today/<key>` mention in its composer without sending it. Text
    /// already in the composer stays: the mention goes after it on a new line.
    pub(super) fn continue_in_session(
        &mut self,
        task: Task,
        id: i64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(view) = self.ensure_session(id, cx) else {
            self.error =
                Some("This session is a terminal: it has no composer to continue in".into());
            cx.notify();
            return;
        };
        let existing = view.read(cx).input.read(cx).content.to_string();
        let mentioned = mention::keys_in_text(&existing).contains(&task.key);
        let text = if mentioned {
            existing
        } else if existing.trim().is_empty() {
            format!("{} ", mention::token(&task.key))
        } else {
            let gap = if existing.ends_with('\n') { "" } else { "\n" };
            format!("{existing}{gap}{} ", mention::token(&task.key))
        };
        view.update(cx, |view, cx| view.set_draft(text, Some(&task.key), cx));
        self.activate_session(id, window, cx);
        self.persist(cx);

        let store = self.store.clone();
        let task_id = task.id.clone();
        let work = cx.background_executor().spawn(async move {
            locked(&store)?.today_link_session(id, &task_id, monocode_core::time::now_ms())
        });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(_) => {}
                    Err(error) => {
                        this.error = Some(format!("Could not link the session: {error}"));
                        cx.notify();
                    }
                }
                this.refresh_today_sessions(cx);
            });
        })
        .detach();
    }

    /// The links changed: reload them on the Today page.
    pub(super) fn refresh_today_sessions(&self, cx: &mut Context<Self>) {
        if let Some(today) = self.today.clone() {
            today.update(cx, |today: &mut today::Today, cx| today.refresh(cx));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shell::surface_tests::{all_terminal, known, since};
    use gpui::TestAppContext;
    use monocode_core::today::{Actor, NewTask};

    const PROJECT: &str = "/work/monocode";

    fn add_task(shell: &Shell, title: &str) -> Task {
        locked(&shell.store)
            .unwrap()
            .today_add(
                &NewTask {
                    title: title.into(),
                    project: Some(PROJECT.into()),
                    ..Default::default()
                },
                &Actor::User,
                monocode_core::time::now_ms(),
            )
            .unwrap()
    }
    fn linked_task(shell: &Shell, session: i64) -> Option<String> {
        locked(&shell.store)
            .unwrap()
            .today_session_task(session)
            .unwrap()
    }
    fn composer(shell: &Shell, session: i64, cx: &App) -> String {
        shell.sessions[&session]
            .read(cx)
            .input
            .read(cx)
            .content
            .to_string()
    }

    #[gpui::test]
    fn starting_a_session_from_a_task_links_it(cx: &mut TestAppContext) {
        let (window, _, _) = crate::shell::tests::window(cx);
        let project = std::env::temp_dir().join(format!(
            "monocode-today-start-{}",
            monocode_core::mono::new_id()
        ));
        std::fs::create_dir_all(&project).unwrap();
        let (before, task) = window
            .update(cx, |shell, _, cx| {
                shell.workspace.preferences.default_provider = Some("claude".into());
                all_terminal(shell);
                let before = known(shell);
                let mut task = add_task(shell, "Ship it");
                task.project = Some(project.to_string_lossy().into_owned());
                shell.start_today_session(task.clone(), cx);
                (before, task)
            })
            .unwrap();
        cx.run_until_parked();
        window
            .update(cx, |shell, _, _| {
                let ids = since(shell, &before);
                assert_eq!(ids.len(), 1);
                assert_eq!(linked_task(shell, ids[0]), Some(task.id.clone()));
            })
            .unwrap();
    }

    #[gpui::test]
    fn continuing_leaves_an_unsent_mention_in_an_empty_composer_and_links(cx: &mut TestAppContext) {
        let (window, _, session) = crate::shell::tests::window(cx);
        window
            .update(cx, |shell, window, cx| {
                let task = add_task(shell, "Ship it");
                shell.continue_in_session(task.clone(), session, window, cx);
                assert_eq!(
                    composer(shell, session, cx),
                    format!("@today/{} ", task.key)
                );
                assert_eq!(shell.workspace.active, Some(session));
            })
            .unwrap();
        cx.run_until_parked();
        window
            .update(cx, |shell, _, _| {
                assert!(linked_task(shell, session).is_some());
                // Nothing was sent.
                assert!(!shell.session_summaries[&session].has_messages);
            })
            .unwrap();
    }

    #[gpui::test]
    fn continuing_keeps_a_draft_and_adds_the_mention_on_a_new_line(cx: &mut TestAppContext) {
        let (window, _, session) = crate::shell::tests::window(cx);
        window
            .update(cx, |shell, window, cx| {
                let task = add_task(shell, "Ship it");
                let view = shell.ensure_session(session, cx).unwrap();
                view.update(cx, |view, cx| view.set_draft("half typed".into(), None, cx));
                shell.continue_in_session(task.clone(), session, window, cx);
                assert_eq!(
                    composer(shell, session, cx),
                    format!("half typed\n@today/{} ", task.key)
                );
                // Continuing again does not stack a second mention.
                shell.continue_in_session(task.clone(), session, window, cx);
                assert_eq!(
                    composer(shell, session, cx),
                    format!("half typed\n@today/{} ", task.key)
                );
            })
            .unwrap();
    }

    #[gpui::test]
    fn a_task_has_many_sessions_and_a_session_has_one_task(cx: &mut TestAppContext) {
        let (window, first, second) = crate::shell::tests::window(cx);
        let (one, two) = window
            .update(cx, |shell, window, cx| {
                let one = add_task(shell, "One");
                let two = add_task(shell, "Two");
                shell.continue_in_session(one.clone(), first, window, cx);
                shell.continue_in_session(one.clone(), second, window, cx);
                (one, two)
            })
            .unwrap();
        cx.run_until_parked();
        window
            .update(cx, |shell, window, cx| {
                let ids = locked(&shell.store)
                    .unwrap()
                    .today_task_session_ids(&one.id)
                    .unwrap();
                assert_eq!(ids.len(), 2);
                shell.continue_in_session(two.clone(), first, window, cx);
            })
            .unwrap();
        cx.run_until_parked();
        window
            .update(cx, |shell, _, _| {
                assert_eq!(linked_task(shell, first), Some(two.id.clone()));
                assert_eq!(linked_task(shell, second), Some(one.id.clone()));
            })
            .unwrap();
    }

    #[gpui::test]
    fn the_page_gets_linked_and_recent_sessions_and_follows_changes(cx: &mut TestAppContext) {
        let (window, first, second) = crate::shell::tests::window(cx);
        let task = window
            .update(cx, |shell, window, cx| {
                // `second` works in PROJECT; `first` in another project.
                let task = add_task(shell, "Ship it");
                shell.continue_in_session(task.clone(), second, window, cx);
                task
            })
            .unwrap();
        cx.run_until_parked();
        window
            .update(cx, |shell, window, cx| {
                shell.open_today(window, cx);
            })
            .unwrap();
        cx.run_until_parked();
        window
            .update(cx, |shell, _, cx| {
                let today = shell.today.clone().unwrap();
                today.update(cx, |today, cx| today.select_for_test(&task.id, cx));
            })
            .unwrap();
        cx.run_until_parked();
        let rows = |cx: &mut TestAppContext| {
            window
                .update(cx, |shell, _, cx| {
                    shell.sync_today_sessions(cx);
                    let today = shell.today.clone().unwrap();
                    let (linked, candidates) = today.read(cx).session_rows();
                    (
                        linked
                            .iter()
                            .map(|r| (r.id, r.archived))
                            .collect::<Vec<_>>(),
                        candidates.iter().map(|r| r.id).collect::<Vec<_>>(),
                    )
                })
                .unwrap()
        };
        let (linked, candidates) = rows(cx);
        assert_eq!(linked, vec![(second, false)]);
        assert_eq!(candidates, vec![second], "only the task's project");
        let _ = first;
        // Archived: still listed, flagged; no longer offered to continue in.
        window
            .update(cx, |shell, _, cx| {
                shell.session_summaries.get_mut(&second).unwrap().archived = true;
                if let Some(view) = shell.sessions.get(&second) {
                    view.update(cx, |view, _| view.summary.archived = true);
                }
            })
            .unwrap();
        let (linked, candidates) = rows(cx);
        assert_eq!(linked, vec![(second, true)]);
        assert!(candidates.is_empty());
        // Deleted: gone from the shell, gone from the page.
        window
            .update(cx, |shell, _, _| {
                shell.session_summaries.remove(&second);
                shell.sessions.remove(&second);
            })
            .unwrap();
        let (linked, _) = rows(cx);
        assert!(linked.is_empty());
    }
}
