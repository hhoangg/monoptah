//! Sessions of an internal task (a Today item) in its detail panel: the
//! SESSIONS section, the Start session split button and the "continue in an
//! existing session" menu.
//!
//! Which sessions belong to a task is stored (`today_session_links`), and
//! `Today` reads those links itself. What a session looks like (title, state,
//! branch) only the shell knows, so the shell pushes a [`SessionRow`] snapshot
//! with [`Today::set_session_rows`]. A linked session the snapshot lacks was
//! deleted and is not listed.
use super::{
    Event, Today,
    detail::section,
    menus::{Pick, Rows, Slot},
};
use crate::{
    inbox::message,
    modal,
    popover::{MenuAction, MenuSearch},
    session_mark::{State, mark_element},
    theme::*,
};
use gpui::{
    AnyElement, Context, Entity, MouseButton, Role, SharedString, Subscription, Window, div,
    prelude::*, px,
};
use monocode_core::{provider::Kind, time, today::Task};

/// One session as the detail panel shows it. Plain data, so `Today` never holds
/// a shell entity.
#[derive(Clone, PartialEq)]
pub(crate) struct SessionRow {
    pub id: i64,
    pub title: String,
    pub provider: String,
    pub state: State,
    pub archived: bool,
    /// Milliseconds since the epoch of the last activity.
    pub updated_at: u64,
    /// The branch it works on, else nothing.
    pub branch: Option<String>,
}

/// Everything the sessions UI keeps in `Today`.
#[derive(Default)]
pub(crate) struct TaskSessions {
    /// Every link as (session id, task id), newest first.
    links: Vec<(i64, String)>,
    /// The linked sessions of the selected task, as the shell sees them.
    linked: Vec<SessionRow>,
    /// Recent chat sessions of the selected task's project, for Continue.
    candidates: Vec<SessionRow>,
    generation: u64,
    /// The row whose menu was opened last, so only its trigger writes the anchor.
    menu: Option<i64>,
    /// A session linked to another task that the user wants to move here.
    ask: Option<MoveAsk>,
}
struct MoveAsk {
    task: Task,
    session: i64,
}

/// The state behind the Start session menu: the task and its search field.
pub(super) struct ContinueMenu {
    pub task: Task,
    pub search: Entity<MenuSearch>,
    pub _changed: Subscription,
}

fn state_label(state: State) -> &'static str {
    match state {
        State::Idle => "Idle",
        State::Working => "Running",
        State::Done => "Done",
        State::Attention => "Waiting",
    }
}

impl Today {
    /// The session ids the selected task is linked to and its project, which the
    /// shell needs to build [`SessionRow`]s.
    pub(crate) fn session_needs(&self) -> (Vec<i64>, Option<String>) {
        let Some(row) = self.selected_row() else {
            return (Vec::new(), None);
        };
        let ids = self
            .sessions
            .links
            .iter()
            .filter(|(_, task)| *task == row.task.id)
            .map(|(session, _)| *session)
            .collect();
        (ids, row.task.project.clone().filter(|p| !p.is_empty()))
    }
    /// Replaces the shell's view of the selected task's sessions. Nothing
    /// happens when it did not change.
    pub(crate) fn set_session_rows(
        &mut self,
        linked: Vec<SessionRow>,
        candidates: Vec<SessionRow>,
        cx: &mut Context<Self>,
    ) {
        if self.sessions.linked == linked && self.sessions.candidates == candidates {
            return;
        }
        self.sessions.linked = linked;
        self.sessions.candidates = candidates;
        // An open Start menu keeps the rows it was built with: the menu keeps its
        // highlight by index, so a reorder under it would move the pick to
        // another session. A search change rebuilds it from the new order.
        cx.notify();
    }
    /// Reads the session links off the UI thread. Runs with every refresh, so an
    /// unlink, a new session or a backfill shows without its own plumbing.
    pub(super) fn load_session_links(&mut self, cx: &mut Context<Self>) {
        self.sessions.generation += 1;
        let generation = self.sessions.generation;
        let store = self.store.clone();
        let work = cx
            .background_executor()
            .spawn(async move { crate::runtime::locked(&store)?.today_session_links() });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |this, cx| {
                if this.sessions.generation != generation {
                    return;
                }
                if let Ok(links) = result
                    && this.sessions.links != links
                {
                    this.sessions.links = links;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// The rows the shell last pushed: (linked, candidates).
    #[cfg(test)]
    pub(crate) fn session_rows(&self) -> (&[SessionRow], &[SessionRow]) {
        (&self.sessions.linked, &self.sessions.candidates)
    }
    /// Selects a task, as a click on its row does.
    #[cfg(test)]
    pub(crate) fn select_for_test(&mut self, id: &str, cx: &mut Context<Self>) {
        self.select(Some(id.to_owned()), cx);
    }

    /// The SESSIONS section: the task's linked sessions, or a hint.
    pub(super) fn sessions_section(&self, task_id: &str, cx: &mut Context<Self>) -> AnyElement {
        let rows: Vec<&SessionRow> = self
            .sessions
            .links
            .iter()
            .filter(|(_, task)| task == task_id)
            .filter_map(|(session, _)| self.sessions.linked.iter().find(|r| r.id == *session))
            .collect();
        let body = if rows.is_empty() {
            message("No sessions yet. Start one to work on this task.")
                .debug_selector(|| "today-sessions-empty".into())
                .px(px(0.))
                .py(px(6.))
                .into_any_element()
        } else {
            div()
                .mt(px(4.))
                .flex()
                .flex_col()
                .children(rows.into_iter().map(|row| self.session_row(row, cx)))
                .into_any_element()
        };
        div()
            .debug_selector(|| "today-sessions".into())
            .flex()
            .flex_col()
            .child(section("SESSIONS"))
            .child(body)
            .into_any_element()
    }

    fn session_row(&self, row: &SessionRow, cx: &mut Context<Self>) -> AnyElement {
        let id = row.id;
        let menu_open = self.sessions.menu == Some(id) && self.menus_open(Slot::SessionMore);
        let mut meta = div()
            .flex()
            .items_center()
            .gap(px(4.))
            .text_size(px(11.))
            .text_color(content(0.45))
            .child(if row.archived {
                div()
                    .debug_selector(move || format!("today-session-archived-{id}"))
                    .child("Archived")
            } else {
                div()
                    .debug_selector(move || format!("today-session-state-{id}"))
                    .child(state_label(row.state))
            })
            .child("·")
            .child(time::relative(row.updated_at, time::now_ms()));
        if let Some(branch) = &row.branch {
            meta = meta
                .child(icon("git-branch", 11.).text_color(content(0.4)))
                .child(div().min_w_0().truncate().child(branch.clone()));
        }
        div()
            .id(("today-session", id as usize))
            .debug_selector(move || format!("today-session-{id}"))
            .role(Role::Button)
            .aria_label(SharedString::from(row.title.clone()))
            .flex()
            .items_center()
            .gap(px(8.))
            .mx(px(-6.))
            .px(px(6.))
            .py(px(4.))
            .rounded(px(6.))
            .cursor_pointer()
            .hover(|s| s.bg(content(0.07)))
            .when(row.archived, |d| d.opacity(0.5))
            .child(mark_element(&row.provider, row.state, 14.))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(1.))
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .text_size(px(12.))
                            .line_height(px(17.))
                            .text_color(content(0.85))
                            .child(row.title.clone()),
                    )
                    .child(meta),
            )
            .child(
                icon_button("today-session-more", "Session actions", "more-horizontal")
                    .debug_selector(move || format!("today-session-more-{id}"))
                    .aria_expanded(menu_open)
                    .relative()
                    .when(menu_open, |d| {
                        d.child(self.anchor(Slot::SessionMore).probe())
                    })
                    // The row below opens the session: keep this click off it.
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(move |this, _, window, cx| {
                        cx.stop_propagation();
                        this.open_session_menu(id, window, cx);
                    })),
            )
            .on_click(cx.listener(move |_, _, _, cx| cx.emit(Event::OpenSession(id))))
            .into_any_element()
    }

    fn open_session_menu(&mut self, id: i64, window: &mut Window, cx: &mut Context<Self>) {
        // Another row's menu is open: close it so this one opens, not toggles.
        if self.sessions.menu != Some(id) {
            self.menus.close();
        }
        self.sessions.menu = Some(id);
        let mut rows = Rows::default();
        rows.add(
            MenuAction::new("session-unlink", "Unlink").icon("close"),
            Pick::Unlink { session: id },
        );
        self.open_menu(Slot::SessionMore, "Session actions", rows, window, cx);
    }
    /// Takes the session off the task. The row goes at once; the store answers
    /// and the reload that follows makes it final.
    pub(super) fn unlink_session(&mut self, session: i64, cx: &mut Context<Self>) {
        self.sessions.links.retain(|(id, _)| *id != session);
        self.act(move |store, _| store.today_unlink_session(session), cx);
        cx.notify();
    }

    // --- Start session ---------------------------------------------------------------

    /// Start session as a split button: the main part starts a new session (see
    /// [`Today::start_session`]), the chevron opens the Start session menu.
    pub(super) fn start_split_button(&self, task: &Task, cx: &mut Context<Self>) -> AnyElement {
        let outer = 6.;
        let project_open = self.menus_open(Slot::StartProject);
        let menu_open = self.menus_open(Slot::StartMenu);
        let started = task.clone();
        let for_menu = task.clone();
        let main = split_segment("today-start-session", "today-start", outer)
            .debug_selector(|| "today-start-session".into())
            .aria_label("Start session")
            .aria_expanded(project_open)
            .relative()
            .w(px(22.))
            .justify_center()
            .child(icon("play", 14.).group_hover("today-start", |s| s.text_color(content(1.))))
            .child(self.anchor(Slot::StartProject).probe())
            .on_click(cx.listener(move |this, _, window, cx| {
                this.start_session(started.clone(), window, cx)
            }));
        let chevron = split_segment("today-start-menu", "today-start", outer)
            .debug_selector(|| "today-start-menu".into())
            .aria_label("Start session options")
            .aria_expanded(menu_open)
            .relative()
            .w(px(14.))
            .justify_center()
            .when(menu_open, |d| d.bg(content(0.08)).text_color(content(1.)))
            .child(self.anchor(Slot::StartMenu).probe())
            .child(
                icon("chevron-down", 12.).group_hover("today-start", |s| s.text_color(content(1.))),
            )
            .on_click(cx.listener(move |this, _, window, cx| {
                this.open_start_menu(for_menu.clone(), window, cx)
            }));
        split_frame("today-start", "Start session", outer, menu_open)
            .h(px(24.))
            .child(main)
            .child(chevron)
            .into_any_element()
    }

    fn open_start_menu(&mut self, task: Task, window: &mut Window, cx: &mut Context<Self>) {
        let search = MenuSearch::new("Search sessions", cx);
        let input = search.read(cx).input.clone();
        let changed = cx.subscribe(&input, |this, _, _: &crate::input::Changed, cx| {
            this.menu_search_changed(cx)
        });
        let rows = self.start_rows(&task, &search, cx);
        let menu = ContinueMenu {
            task,
            search,
            _changed: changed,
        };
        self.open_menu_for(Slot::StartMenu, "Start session", rows, menu, window, cx);
    }
    /// The Start session menu's rows: a new session, then the project's recent
    /// ones that match the search field.
    pub(super) fn start_rows(
        &self,
        task: &Task,
        search: &Entity<MenuSearch>,
        cx: &gpui::App,
    ) -> Rows {
        let typed = search.read(cx).query(cx);
        let query = typed.trim().to_lowercase();
        let mut rows = Rows::default();
        rows.add(
            MenuAction::new("start-new", "Start new session").icon("plus"),
            Pick::StartNew(Box::new(task.clone())),
        );
        rows.separator();
        rows.header("Continue in an existing session");
        rows.search(search, cx);
        if task.project.as_deref().is_none_or(str::is_empty) {
            rows.note("Choose a project for this task to continue in one of its sessions.");
            return rows;
        }
        let mut shown = 0;
        for session in self
            .sessions
            .candidates
            .iter()
            .filter(|s| s.title.to_lowercase().contains(&query))
        {
            shown += 1;
            let elsewhere = self
                .sessions
                .links
                .iter()
                .any(|(id, linked)| *id == session.id && *linked != task.id);
            let mut action = MenuAction::new(format!("continue-{}", session.id), &session.title)
                .image(Kind::from_id(&session.provider).id())
                .value(time::relative(session.updated_at, time::now_ms()));
            if elsewhere {
                action = action.badge("Linked");
            }
            rows.add(
                action,
                Pick::ContinueIn {
                    task: Box::new(task.clone()),
                    session: session.id,
                },
            );
        }
        if shown == 0 {
            rows.note(if self.sessions.candidates.is_empty() {
                "No recent sessions in this project."
            } else {
                "No session matches."
            });
        }
        rows
    }
    /// Continue in an existing session. A session linked to another task asks
    /// before it moves; any other is handed to the shell at once.
    pub(super) fn continue_in(&mut self, task: Task, session: i64, cx: &mut Context<Self>) {
        let elsewhere = self
            .sessions
            .links
            .iter()
            .any(|(id, linked)| *id == session && *linked != task.id);
        if elsewhere {
            self.sessions.ask = Some(MoveAsk { task, session });
            cx.notify();
        } else {
            self.emit_continue(task, session, cx);
        }
    }
    fn emit_continue(&mut self, task: Task, session: i64, cx: &mut Context<Self>) {
        // Link here at once, so the section is right before the store answers.
        if !self.sessions.links.iter().any(|(id, _)| *id == session) {
            self.sessions.links.insert(0, (session, task.id.clone()));
        }
        cx.emit(Event::ContinueInSession {
            task: Box::new(task),
            session,
        });
        cx.notify();
    }
    fn confirm_move(&mut self, cx: &mut Context<Self>) {
        if let Some(MoveAsk { task, session }) = self.sessions.ask.take() {
            self.emit_continue(task, session, cx);
        }
    }
    pub(super) fn cancel_move(&mut self, cx: &mut Context<Self>) {
        if self.sessions.ask.take().is_some() {
            cx.notify();
        }
    }
    pub(super) fn asking_to_move(&self) -> bool {
        self.sessions.ask.is_some()
    }
    /// "Move this session to <task>?" over the page, while a pick waits on it.
    pub(super) fn move_confirmation(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let ask = self.sessions.ask.as_ref()?;
        let session = self
            .sessions
            .candidates
            .iter()
            .chain(&self.sessions.linked)
            .find(|s| s.id == ask.session)
            .map(|s| s.title.clone())
            .unwrap_or_else(|| "This session".into());
        let this = cx.entity();
        let card = modal::card().child(
            modal::card_body()
                .child(modal::title("Move this session?"))
                .child(
                    modal::body(format!(
                        "\"{session}\" is linked to another task. Move it to \"{}\"?",
                        ask.task.title
                    ))
                    .mt(px(8.))
                    .debug_selector(|| "today-move-text".into()),
                )
                .child(
                    modal::actions()
                        .child(
                            modal::ghost_button("today-move-cancel", "Cancel")
                                .debug_selector(|| "today-move-cancel".into())
                                .cursor_pointer()
                                .on_click(cx.listener(|this, _, _, cx| this.cancel_move(cx))),
                        )
                        .child(
                            modal::primary_button("today-move-confirm", "Move")
                                .debug_selector(|| "today-move-confirm".into())
                                .cursor_pointer()
                                .on_click(cx.listener(|this, _, _, cx| this.confirm_move(cx))),
                        ),
                ),
        );
        Some(
            modal::overlay("today-move", card, move |_, cx| {
                this.update(cx, |today, cx| today.cancel_move(cx))
            })
            .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::super::list::testkit::*;
    use super::*;
    use gpui::TestAppContext;
    use monocode_core::today::{Actor, NewTask};
    use std::sync::{Arc, Mutex};

    fn session(id: i64, title: &str) -> SessionRow {
        SessionRow {
            id,
            title: title.into(),
            provider: "claude".into(),
            state: State::Idle,
            archived: false,
            updated_at: time::now_ms(),
            branch: None,
        }
    }
    type Rig = (
        gpui::WindowHandle<Page>,
        gpui::VisualTestContext,
        Entity<Today>,
        Arc<Mutex<monocode_core::store::Store>>,
        Task,
    );
    /// A store backed page with one project task selected.
    fn rig(cx: &mut TestAppContext) -> Rig {
        let (window, mut visual) = page(cx, Show::Detail, |_| {});
        let (today, store) = window
            .update(&mut visual, |page, _, cx| {
                (page.today.clone(), page.today.read(cx).store.clone())
            })
            .unwrap();
        let task = crate::runtime::locked(&store)
            .unwrap()
            .today_add(
                &NewTask {
                    title: "Ship it".into(),
                    project: Some("/work/app".into()),
                    ..Default::default()
                },
                &Actor::User,
                time::now_ms(),
            )
            .unwrap();
        today.update(&mut visual, |today, cx| {
            today.selected = Some(task.id.clone());
            today.refresh(cx);
        });
        visual.run_until_parked();
        (window, visual, today, store, task)
    }
    /// Stores a second task, for sessions that belong elsewhere.
    fn other_task(store: &Arc<Mutex<monocode_core::store::Store>>) -> Task {
        crate::runtime::locked(store)
            .unwrap()
            .today_add(
                &NewTask {
                    title: "Other".into(),
                    project: Some("/work/app".into()),
                    ..Default::default()
                },
                &Actor::User,
                time::now_ms(),
            )
            .unwrap()
    }
    fn link(store: &Arc<Mutex<monocode_core::store::Store>>, session: i64, task: &Task) {
        crate::runtime::locked(store)
            .unwrap()
            .today_link_session(session, &task.id, time::now_ms())
            .unwrap();
    }
    fn settle(today: &Entity<Today>, rows: Vec<SessionRow>, visual: &mut gpui::VisualTestContext) {
        today.update(visual, |today, cx| {
            today.refresh(cx);
            today.set_session_rows(rows.clone(), rows, cx);
        });
        visual.run_until_parked();
        visual.refresh().unwrap();
    }
    fn click(visual: &mut gpui::VisualTestContext, selector: String) {
        visual.refresh().unwrap();
        let selector: &'static str = Box::leak(selector.into_boxed_str());
        let bounds = visual
            .debug_bounds(selector)
            .unwrap_or_else(|| panic!("{selector} is drawn"));
        visual.simulate_click(bounds.center(), Default::default());
        visual.run_until_parked();
        visual.refresh().unwrap();
    }
    fn drawn(visual: &mut gpui::VisualTestContext, selector: String) -> bool {
        visual.refresh().unwrap();
        let selector: &'static str = Box::leak(selector.into_boxed_str());
        visual.debug_bounds(selector).is_some()
    }
    /// What `Today` emits about sessions, as short strings.
    fn events(
        window: &gpui::WindowHandle<Page>,
        visual: &mut gpui::VisualTestContext,
    ) -> Arc<Mutex<Vec<String>>> {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink = seen.clone();
        window
            .update(visual, |page, _, cx| {
                let today = page.today.clone();
                cx.subscribe(&today, move |_, _, event: &Event, _| match event {
                    Event::OpenSession(id) => sink.lock().unwrap().push(format!("open {id}")),
                    Event::ContinueInSession { task, session } => sink
                        .lock()
                        .unwrap()
                        .push(format!("continue {} {session}", task.title)),
                    _ => {}
                })
                .detach();
            })
            .unwrap();
        seen
    }

    #[gpui::test]
    fn no_sessions_shows_the_hint(cx: &mut TestAppContext) {
        let (_, mut visual, _, _, _) = rig(cx);
        assert!(drawn(&mut visual, "today-sessions".into()));
        assert!(drawn(&mut visual, "today-sessions-empty".into()));
    }

    #[gpui::test]
    fn linked_sessions_are_listed_with_their_state_and_archived_ones_are_dimmed(
        cx: &mut TestAppContext,
    ) {
        let (_, mut visual, today, store, task) = rig(cx);
        link(&store, 11, &task);
        link(&store, 12, &task);
        let mut running = session(11, "Fix the bug");
        running.state = State::Working;
        running.branch = Some("feat/bug".into());
        let mut old = session(12, "Old attempt");
        old.archived = true;
        settle(&today, vec![running, old], &mut visual);
        assert!(!drawn(&mut visual, "today-sessions-empty".into()));
        assert!(drawn(&mut visual, "today-session-11".into()));
        assert!(drawn(&mut visual, "today-session-state-11".into()));
        assert!(!drawn(&mut visual, "today-session-archived-11".into()));
        assert!(drawn(&mut visual, "today-session-12".into()));
        assert!(drawn(&mut visual, "today-session-archived-12".into()));
        assert!(!drawn(&mut visual, "today-session-state-12".into()));
    }

    #[gpui::test]
    fn a_session_the_shell_no_longer_has_disappears(cx: &mut TestAppContext) {
        let (_, mut visual, today, store, task) = rig(cx);
        link(&store, 11, &task);
        link(&store, 12, &task);
        settle(
            &today,
            vec![session(11, "Kept"), session(12, "Gone")],
            &mut visual,
        );
        assert!(drawn(&mut visual, "today-session-12".into()));
        settle(&today, vec![session(11, "Kept")], &mut visual);
        assert!(drawn(&mut visual, "today-session-11".into()));
        assert!(!drawn(&mut visual, "today-session-12".into()));
    }

    #[gpui::test]
    fn another_tasks_sessions_are_not_listed(cx: &mut TestAppContext) {
        let (_, mut visual, today, store, task) = rig(cx);
        let other = other_task(&store);
        link(&store, 11, &task);
        link(&store, 12, &other);
        settle(
            &today,
            vec![session(11, "Mine"), session(12, "Theirs")],
            &mut visual,
        );
        assert!(drawn(&mut visual, "today-session-11".into()));
        assert!(!drawn(&mut visual, "today-session-12".into()));
    }

    #[gpui::test]
    fn clicking_a_row_opens_the_session(cx: &mut TestAppContext) {
        let (window, mut visual, today, store, task) = rig(cx);
        link(&store, 11, &task);
        settle(&today, vec![session(11, "Fix the bug")], &mut visual);
        let seen = events(&window, &mut visual);
        click(&mut visual, "today-session-11".into());
        assert_eq!(*seen.lock().unwrap(), vec!["open 11".to_string()]);
    }

    #[gpui::test]
    fn unlink_from_the_row_menu_removes_the_link_and_the_row(cx: &mut TestAppContext) {
        let (window, mut visual, today, store, task) = rig(cx);
        link(&store, 11, &task);
        link(&store, 12, &task);
        settle(
            &today,
            vec![session(11, "One"), session(12, "Two")],
            &mut visual,
        );
        let seen = events(&window, &mut visual);
        click(&mut visual, "today-session-more-11".into());
        assert!(today.read_with(&visual, |t, _| t.menus.is_open()));
        assert_eq!(
            today.read_with(&visual, |t, _| t.menus.pick_ids()),
            ["session-unlink"]
        );
        // The click on the button did not open the session.
        assert!(seen.lock().unwrap().is_empty());
        visual.simulate_keystrokes("enter");
        visual.run_until_parked();
        visual.refresh().unwrap();
        assert!(!drawn(&mut visual, "today-session-11".into()));
        assert!(drawn(&mut visual, "today-session-12".into()));
        let links = crate::runtime::locked(&store)
            .unwrap()
            .today_session_links()
            .unwrap();
        assert_eq!(links, vec![(12, task.id.clone())]);
    }

    /// Shows the detail panel of a `Today` made on an existing store.
    struct Probe(Entity<Today>);
    impl gpui::Render for Probe {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            self.0.update(cx, |today, cx| {
                div().size_full().child(today.detail_view(cx))
            })
        }
    }

    #[gpui::test]
    fn sessions_started_before_links_existed_appear(cx: &mut TestAppContext) {
        cx.update(|cx| {
            cx.set_reduce_motion(true);
            crate::popover::bind(cx);
        });
        // The store has a task and a session whose first message mentions it,
        // but no link row: the way Start session worked before links.
        let mut db = monocode_core::store::Store::open(std::path::Path::new(":memory:")).unwrap();
        let task = db
            .today_add(
                &NewTask {
                    title: "Ship it".into(),
                    project: Some("/work/app".into()),
                    ..Default::default()
                },
                &Actor::User,
                time::now_ms(),
            )
            .unwrap();
        let id = db.create("codex", "/work/app").unwrap().id;
        db.write_rows(
            id,
            0,
            &[monocode_core::transcript::Row {
                role: "user".into(),
                text: format!("@today/{} fix it", task.key),
                first: true,
                sent_at: None,
            }],
        )
        .unwrap();
        let store = Arc::new(Mutex::new(db));
        let (probe, visual) =
            cx.add_window_view(|_, cx| Probe(cx.new(|cx| Today::new_quiet(store.clone(), cx))));
        let today = probe.read_with(visual, |probe, _| probe.0.clone());
        today.update(visual, |today, cx| {
            today.refresh(cx);
        });
        visual.run_until_parked();
        today.update(visual, |today, cx| {
            today.select_for_test(&task.id, cx);
            today.set_session_rows(vec![session(id, "Backfilled")], Vec::new(), cx);
        });
        visual.run_until_parked();
        assert!(drawn(visual, format!("today-session-{id}")));
    }

    fn open_chevron(visual: &mut gpui::VisualTestContext) {
        click(visual, "today-start-menu".into());
    }

    #[gpui::test]
    fn the_chevron_lists_a_new_session_then_the_projects_sessions(cx: &mut TestAppContext) {
        let (_, mut visual, today, _, _) = rig(cx);
        today.update(&mut visual, |today, cx| {
            today.set_session_rows(
                Vec::new(),
                vec![session(21, "Alpha"), session(22, "Beta")],
                cx,
            )
        });
        open_chevron(&mut visual);
        assert_eq!(
            today.read_with(&visual, |t, _| t.menus.pick_ids()),
            ["start-new", "continue-21", "continue-22"]
        );
    }

    #[gpui::test]
    fn typing_filters_the_sessions_in_the_start_menu(cx: &mut TestAppContext) {
        let (_, mut visual, today, _, _) = rig(cx);
        today.update(&mut visual, |today, cx| {
            today.set_session_rows(
                Vec::new(),
                vec![session(21, "Alpha"), session(22, "Beta")],
                cx,
            )
        });
        open_chevron(&mut visual);
        let input = today.read_with(&visual, |t, cx| t.menus.search_input(cx).unwrap());
        input.update(&mut visual, |input, cx| input.set_text("bet".into(), cx));
        visual.run_until_parked();
        visual.refresh().unwrap();
        assert_eq!(
            today.read_with(&visual, |t, _| t.menus.pick_ids()),
            ["start-new", "continue-22"]
        );
    }

    #[gpui::test]
    fn picking_a_session_continues_in_it(cx: &mut TestAppContext) {
        let (window, mut visual, today, _, _) = rig(cx);
        today.update(&mut visual, |today, cx| {
            today.set_session_rows(Vec::new(), vec![session(21, "Alpha")], cx)
        });
        let seen = events(&window, &mut visual);
        open_chevron(&mut visual);
        // Arrow down past "Start new session", then Enter.
        visual.simulate_keystrokes("down enter");
        visual.run_until_parked();
        assert_eq!(
            *seen.lock().unwrap(),
            vec!["continue Ship it 21".to_string()]
        );
    }

    #[gpui::test]
    fn a_reordered_snapshot_does_not_move_the_highlight_to_another_session(
        cx: &mut TestAppContext,
    ) {
        let (window, mut visual, today, _, _) = rig(cx);
        today.update(&mut visual, |today, cx| {
            today.set_session_rows(
                Vec::new(),
                vec![session(21, "Alpha"), session(22, "Beta")],
                cx,
            )
        });
        let seen = events(&window, &mut visual);
        open_chevron(&mut visual);
        // Highlight Alpha, then the shell reorders the list under the open menu.
        visual.simulate_keystrokes("down");
        today.update(&mut visual, |today, cx| {
            today.set_session_rows(
                Vec::new(),
                vec![session(22, "Beta"), session(21, "Alpha")],
                cx,
            )
        });
        visual.run_until_parked();
        visual.refresh().unwrap();
        visual.simulate_keystrokes("enter");
        visual.run_until_parked();
        assert_eq!(
            *seen.lock().unwrap(),
            vec!["continue Ship it 21".to_string()]
        );
    }

    #[gpui::test]
    fn a_session_linked_elsewhere_asks_before_it_moves(cx: &mut TestAppContext) {
        let (window, mut visual, today, store, task) = rig(cx);
        let other = other_task(&store);
        link(&store, 21, &other);
        today.update(&mut visual, |today, cx| {
            today.refresh(cx);
            today.set_session_rows(Vec::new(), vec![session(21, "Alpha")], cx)
        });
        visual.run_until_parked();
        let seen = events(&window, &mut visual);
        open_chevron(&mut visual);
        visual.simulate_keystrokes("down enter");
        visual.run_until_parked();
        visual.refresh().unwrap();
        assert!(seen.lock().unwrap().is_empty(), "nothing moved yet");
        assert!(drawn(&mut visual, "today-move-text".into()));
        // Cancel: still nothing.
        click(&mut visual, "today-move-cancel".into());
        assert!(!drawn(&mut visual, "today-move-text".into()));
        assert!(seen.lock().unwrap().is_empty());
        // Ask again and confirm.
        open_chevron(&mut visual);
        visual.simulate_keystrokes("down enter");
        visual.run_until_parked();
        click(&mut visual, "today-move-confirm".into());
        assert_eq!(
            *seen.lock().unwrap(),
            vec!["continue Ship it 21".to_string()]
        );
        let _ = task;
    }

    #[gpui::test]
    fn a_task_without_a_project_lists_no_sessions_and_says_why(cx: &mut TestAppContext) {
        let (_, mut visual, today, _, _) = rig(cx);
        today.update(&mut visual, |today, cx| {
            if let Some(view) = today.view.as_mut() {
                for row in &mut view.rows {
                    row.task.project = None;
                }
            }
            today.set_session_rows(Vec::new(), vec![session(21, "Alpha")], cx)
        });
        open_chevron(&mut visual);
        assert_eq!(
            today.read_with(&visual, |t, _| t.menus.pick_ids()),
            ["start-new"]
        );
    }

    #[gpui::test]
    fn start_new_session_in_the_menu_starts_like_the_main_part(cx: &mut TestAppContext) {
        let (window, mut visual, _, _, _) = rig(cx);
        let started = Arc::new(Mutex::new(0));
        let sink = started.clone();
        window
            .update(&mut visual, |page, _, cx| {
                let today = page.today.clone();
                cx.subscribe(&today, move |_, _, event: &Event, _| {
                    if matches!(event, Event::StartSession(_)) {
                        *sink.lock().unwrap() += 1;
                    }
                })
                .detach();
            })
            .unwrap();
        open_chevron(&mut visual);
        visual.simulate_keystrokes("enter");
        visual.run_until_parked();
        assert_eq!(*started.lock().unwrap(), 1);
    }

    #[gpui::test]
    fn the_main_part_still_starts_a_new_session(cx: &mut TestAppContext) {
        let (window, mut visual, _, _, _) = rig(cx);
        let started = Arc::new(Mutex::new(0));
        let sink = started.clone();
        window
            .update(&mut visual, |page, _, cx| {
                let today = page.today.clone();
                cx.subscribe(&today, move |_, _, event: &Event, _| {
                    if matches!(event, Event::StartSession(_)) {
                        *sink.lock().unwrap() += 1;
                    }
                })
                .detach();
            })
            .unwrap();
        click(&mut visual, "today-start-session".into());
        assert_eq!(*started.lock().unwrap(), 1);
    }
}
