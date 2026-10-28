//! The Contributors page: a Ticket tab and a GitHub tab over the core's
//! `Contributors` handle. `mod jobs` is the app-wide event model that outlives
//! the page; `github` is the GitHub tab (one `GithubView` per project, kept
//! while the page lives so coming back is instant).
//!
//! Where the tab tasks plug in (see the 6.7 report):
//! - Stats, Leaderboard, Identities: `stats.rs`, `leaderboard.rs`, `identities.rs`
//!   (an `impl GithubView` method each, called with the built model).
//! - Ticket: `ticket.rs` (`ContributorsPage::ticket_body`).
//! - Ask Agent: `ask.rs`. The view's `GithubEvent::Ask(scope)` becomes `Event::Ask`;
//!   the shell answers with `open_ask` (the providers it offers) and, on
//!   "Start session", starts the chat and answers `ask_finished`.
use crate::{runtime::Database, theme::*};
use gpui::{
    App, Context, Entity, EventEmitter, FocusHandle, Focusable, Role, SharedString, Subscription,
    Window, div, prelude::*, px,
};
use monocode_core::contributors::{
    api::Contributors,
    model::{ask::AskAgentRequest, snapshot::SnapshotScope},
};
use std::sync::Arc;

mod ask;
#[cfg(test)]
mod ask_tests;
pub(crate) mod backend;
mod data;
#[cfg(test)]
mod data_tests;
mod formula;
#[cfg(test)]
mod formula_tests;
mod github;
mod identities;
#[cfg(test)]
mod identities_tests;
pub(crate) mod jobs;
mod leaderboard;
#[cfg(test)]
mod leaderboard_tests;
#[cfg(test)]
mod page_tests;
mod panels;
mod stats;
#[cfg(test)]
mod stats_tests;
mod ticket;
mod ui;

pub(crate) use ask::AskChoices;
use ask::{AskDialog, AskEvent};
use backend::{Backend, CoreBackend};
pub(crate) use github::{GithubEvent, GithubView};
pub(crate) use jobs::SyncJobs;
use ticket::{TicketEvent, TicketView};

/// Views kept so returning to a project is instant; the oldest goes first.
const VIEW_LIMIT: usize = 6;

/// The page's two views. Opening the page from the sidebar always starts on Ticket.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Tab {
    #[default]
    Ticket,
    Github,
}
impl Tab {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Ticket => "Ticket",
            Self::Github => "GitHub",
        }
    }
    fn id(self) -> &'static str {
        match self {
            Self::Ticket => "ticket",
            Self::Github => "github",
        }
    }
    fn from_id(id: &str) -> Option<Self> {
        [Self::Ticket, Self::Github]
            .into_iter()
            .find(|tab| tab.id() == id)
    }
}

pub(crate) enum Event {
    /// Escape, or the page's own close: the shell leaves the app view.
    Close,
    /// An Ask Agent button was pressed with the data on screen. The shell
    /// answers with [`ContributorsPage::open_ask`] and the providers on offer.
    Ask {
        project: String,
        scope: SnapshotScope,
        view: Entity<GithubView>,
    },
    /// "Start session" in the dialog. The shell starts the chat and answers with
    /// [`ContributorsPage::ask_finished`].
    StartSession {
        project: String,
        request: Box<AskAgentRequest>,
        /// Names this start; hand it back to [`ContributorsPage::ask_finished`]
        /// so the answer of an old start cannot land on a newer dialog.
        start: u64,
    },
    /// "Open Settings" on a ClickUp problem: the shell opens Settings > Inbox.
    OpenSettings,
}

use monocode_core::files::{looks_like_project, path_key};

fn is_remote(path: &str) -> bool {
    monocode_core::files::remote::is_remote(std::path::Path::new(path))
}

fn same_path(a: &str, b: &str) -> bool {
    path_key(a) == path_key(b)
}

/// The project the GitHub tab shows: the chosen one, else the active project,
/// else the first local project; `None` when the app knows no project at all.
pub(crate) fn resolve_project(
    chosen: Option<&str>,
    projects: &[String],
    active: &str,
) -> Option<String> {
    if let Some(chosen) = chosen.filter(|chosen| !chosen.is_empty()) {
        return Some(chosen.to_string());
    }
    if !active.is_empty() && looks_like_project(active) {
        return Some(active.to_string());
    }
    projects.iter().find(|path| !is_remote(path)).cloned()
}

/// The select's options: the rail's projects, with the shown one listed first
/// when the rail no longer has it. Two folders can share a name; the path
/// tells them apart.
pub(crate) fn project_options(
    projects: &[String],
    selected: Option<&str>,
) -> Vec<(String, String)> {
    let mut paths: Vec<&str> = projects.iter().map(String::as_str).collect();
    if let Some(selected) = selected
        && !projects.iter().any(|path| same_path(path, selected))
    {
        paths.insert(0, selected);
    }
    let names: Vec<String> = paths
        .iter()
        .map(|path| monocode_core::search::project_name(path))
        .collect();
    paths
        .iter()
        .enumerate()
        .map(|(index, path)| {
            let shared = names.iter().filter(|name| **name == names[index]).count() > 1;
            let label = if shared {
                format!(
                    "{} ({})",
                    names[index],
                    crate::worktree_switcher::pretty_path(path)
                )
            } else {
                names[index].clone()
            };
            ((*path).to_string(), label)
        })
        .collect()
}

pub(crate) struct ContributorsPage {
    tab: Tab,
    /// The project the GitHub tab shows; `None` leaves it to the page's default.
    project: Option<String>,
    /// The rail's projects and the active one, for the project selector.
    projects: Vec<String>,
    active_project: String,
    core: Option<Arc<Contributors>>,
    core_error: Option<String>,
    store: Option<Database>,
    backend: Option<Arc<dyn Backend>>,
    jobs: Entity<SyncJobs>,
    /// One GitHub view per project, least recently used first.
    views: Vec<(String, Entity<GithubView>)>,
    /// What each view's Ask buttons tell the page, by project.
    view_events: Vec<(String, Subscription)>,
    /// The Ask Agent dialog, a layer over the page while it is open.
    ask: Option<Entity<AskDialog>>,
    _ask_events: Vec<Subscription>,
    /// The Ticket tab, kept for the page's life; opening the tab mounts it.
    ticket: Entity<TicketView>,
    _ticket_events: Subscription,
    /// Starts handed out so far, and the one the open dialog waits for.
    starts: u64,
    ask_start: Option<u64>,
    /// The page takes the keyboard back on the next frame (the dialog just closed).
    refocus: bool,
    clock: data::Clock,
    project_dd: ui::Dropdown,
    focus: FocusHandle,
}
impl EventEmitter<Event> for ContributorsPage {}
impl Focusable for ContributorsPage {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl ContributorsPage {
    pub(crate) fn new(jobs: Entity<SyncJobs>, cx: &mut Context<Self>) -> Self {
        let clock = data::system_clock();
        let ticket = cx.new(|cx| TicketView::new(jobs.clone(), clock.clone(), cx));
        let ticket_events = cx.subscribe(&ticket, |_, _, event: &TicketEvent, cx| match event {
            TicketEvent::OpenSettings => cx.emit(Event::OpenSettings),
        });
        Self {
            tab: Tab::default(),
            project: None,
            projects: Vec::new(),
            active_project: String::new(),
            core: None,
            core_error: None,
            store: None,
            backend: None,
            jobs,
            views: Vec::new(),
            view_events: Vec::new(),
            ask: None,
            _ask_events: Vec::new(),
            ticket,
            _ticket_events: ticket_events,
            starts: 0,
            ask_start: None,
            refocus: false,
            clock,
            project_dd: ui::Dropdown::new(cx),
            focus: cx.focus_handle(),
        }
    }

    /// Opens on `tab` (Ticket when `None`) showing `project` (the page's default when `None`).
    pub(crate) fn show(
        &mut self,
        tab: Option<Tab>,
        project: Option<String>,
        cx: &mut Context<Self>,
    ) {
        self.tab = tab.unwrap_or_default();
        self.project = project;
        // A dialog left over goes with its events and any start it still waits for.
        self.close_ask(false, cx);
        self.sync_view(cx);
        self.mount_ticket(cx);
        cx.notify();
    }

    /// Opening the Ticket tab mounts its view: the rules start over, the
    /// windows fetched in this session stay fetched.
    fn mount_ticket(&mut self, cx: &mut Context<Self>) {
        if self.tab == Tab::Ticket {
            self.ticket.update(cx, |ticket, cx| ticket.mount(cx));
        } else {
            self.unmount_ticket(cx);
        }
    }

    /// The Ticket tab is not shown (the other tab, the page closed): React
    /// unmounts it, so it must not listen, reload or queue ClickUp fetches.
    fn unmount_ticket(&mut self, cx: &mut Context<Self>) {
        self.ticket.update(cx, |ticket, cx| ticket.unmount(cx));
    }

    /// The page is not on screen any more (another view, Settings): the Ticket
    /// tab goes with it. [`Self::reveal`] brings it back.
    pub(crate) fn hide(&mut self, cx: &mut Context<Self>) {
        self.unmount_ticket(cx);
    }

    /// The page is on screen again after [`Self::hide`] without being opened
    /// anew (Settings closed over it): the Ticket tab mounts afresh.
    pub(crate) fn reveal(&mut self, cx: &mut Context<Self>) {
        self.mount_ticket(cx);
    }

    pub(crate) fn set_context(
        &mut self,
        projects: Vec<String>,
        active_project: String,
        cx: &mut Context<Self>,
    ) {
        self.projects = projects;
        self.active_project = active_project;
        self.sync_view(cx);
        cx.notify();
    }

    /// The store the saved period and merge toggle live in.
    pub(crate) fn set_store(&mut self, store: Database) {
        self.store = Some(store);
    }

    pub(crate) fn set_core(
        &mut self,
        core: Result<Arc<Contributors>, String>,
        cx: &mut Context<Self>,
    ) {
        match core {
            Ok(core) => {
                let fresh = self
                    .core
                    .as_ref()
                    .is_none_or(|current| !Arc::ptr_eq(current, &core));
                self.core_error = None;
                if fresh {
                    self.core = Some(core.clone());
                    let tickets =
                        ticket::backend::CoreTicketBackend::new(core.clone(), self.store.clone());
                    self.set_ticket_backend(Arc::new(tickets), cx);
                    self.set_backend(Arc::new(CoreBackend::new(core, self.store.clone())), cx);
                }
            }
            Err(error) => self.core_error = Some(error),
        }
        cx.notify();
    }

    /// What the views read and write through. The tests give a fake one.
    pub(crate) fn set_backend(&mut self, backend: Arc<dyn Backend>, cx: &mut Context<Self>) {
        self.backend = Some(backend.clone());
        for (_, view) in &self.views {
            view.update(cx, |view, cx| view.set_backend(backend.clone(), cx));
        }
        self.sync_view(cx);
        cx.notify();
    }

    /// What the Ticket tab reads and writes through. The tests give a fake one.
    pub(crate) fn set_ticket_backend(
        &mut self,
        backend: Arc<dyn ticket::backend::TicketBackend>,
        cx: &mut Context<Self>,
    ) {
        self.ticket
            .update(cx, |ticket, cx| ticket.set_backend(backend, cx));
    }

    /// The window came back to the front: the open tab revalidates (the Ticket
    /// tab looks again at the ClickUp connection, which Settings may have changed).
    pub(crate) fn window_activated(&mut self, cx: &mut Context<Self>) {
        if self.tab == Tab::Ticket {
            self.ticket.update(cx, |ticket, cx| ticket.resume(cx));
            return;
        }
        if let Some(view) = self.current_view() {
            view.update(cx, |view, cx| view.resume(cx));
        }
    }

    /// The project the GitHub tab shows now.
    pub(crate) fn selected_project(&self) -> Option<String> {
        resolve_project(
            self.project.as_deref(),
            &self.projects,
            &self.active_project,
        )
    }

    /// The GitHub view of the selected project, once it exists.
    pub(crate) fn current_view(&self) -> Option<Entity<GithubView>> {
        let selected = self.selected_project()?;
        self.views
            .iter()
            .find(|(project, _)| same_path(project, &selected))
            .map(|(_, view)| view.clone())
    }

    /// Makes sure the selected project has a GitHub view (made when the tab is
    /// opened, like the React view mounts) and marks it the most recent.
    fn sync_view(&mut self, cx: &mut Context<Self>) {
        if self.tab != Tab::Github {
            return;
        }
        let Some(selected) = self.selected_project() else {
            return;
        };
        match self
            .views
            .iter()
            .position(|(project, _)| same_path(project, &selected))
        {
            Some(at) => {
                let entry = self.views.remove(at);
                self.views.push(entry);
            }
            None => {
                let (jobs, clock) = (self.jobs.clone(), self.clock.clone());
                let name = selected.clone();
                let view = cx.new(|cx| GithubView::new(name, jobs, clock, cx));
                if let Some(backend) = self.backend.clone() {
                    view.update(cx, |view, cx| view.set_backend(backend, cx));
                }
                let project = selected.clone();
                let events = cx.subscribe(&view, move |_, view, event: &GithubEvent, cx| {
                    let GithubEvent::Ask(scope) = event;
                    cx.emit(Event::Ask {
                        project: project.clone(),
                        scope: scope.clone(),
                        view: view.clone(),
                    });
                });
                self.view_events.push((selected.clone(), events));
                self.views.push((selected, view));
                if self.views.len() > VIEW_LIMIT {
                    let (gone, _) = self.views.remove(0);
                    self.view_events.retain(|(project, _)| *project != gone);
                }
            }
        }
    }

    /// Opens the Ask Agent dialog over the page for `view`'s data. Ignored while
    /// one is open or when the data is not on screen.
    pub(crate) fn open_ask(
        &mut self,
        project: String,
        scope: SnapshotScope,
        view: Entity<GithubView>,
        choices: AskChoices,
        cx: &mut Context<Self>,
    ) {
        if self.ask.is_some() || !view.read(cx).ask_ready(cx) {
            return;
        }
        let dialog = cx.new(|cx| AskDialog::new(view, project.clone(), scope, choices, cx));
        self._ask_events =
            vec![
                cx.subscribe(&dialog, move |this, _, event: &AskEvent, cx| match event {
                    AskEvent::Close => this.close_ask(true, cx),
                    AskEvent::Start(request) => {
                        this.starts += 1;
                        this.ask_start = Some(this.starts);
                        cx.emit(Event::StartSession {
                            project: project.clone(),
                            request: request.clone(),
                            start: this.starts,
                        })
                    }
                }),
            ];
        self.ask = Some(dialog);
        cx.notify();
    }

    /// The outcome of [`Event::StartSession`]: the dialog closes, or shows why
    /// the session could not start and lets the user try again.
    /// `start` is the one [`Event::StartSession`] carried: the answer of a start
    /// whose dialog is gone (closed, or replaced by a newer one) is dropped.
    pub(crate) fn ask_finished(
        &mut self,
        start: u64,
        result: Result<(), String>,
        cx: &mut Context<Self>,
    ) {
        let (Some(dialog), true) = (self.ask.clone(), self.ask_start == Some(start)) else {
            return;
        };
        self.ask_start = None;
        match result {
            Ok(()) => self.close_ask(false, cx),
            Err(error) => dialog.update(cx, |dialog, cx| dialog.failed(&error, cx)),
        }
    }

    fn close_ask(&mut self, refocus: bool, cx: &mut Context<Self>) {
        self.ask_start = None;
        self._ask_events.clear();
        if self.ask.take().is_some() {
            self.refocus = refocus;
            cx.notify();
        }
    }

    /// The open Ask Agent dialog.
    #[cfg(test)]
    pub(crate) fn ask_dialog(&self) -> Option<Entity<AskDialog>> {
        self.ask.clone()
    }

    /// Whether the Ticket tab is mounted (listening, reloading, fetching).
    #[cfg(test)]
    pub(crate) fn ticket_mounted(&self, cx: &App) -> bool {
        self.ticket.read(cx).is_mounted()
    }

    #[cfg(test)]
    pub(crate) fn with_clock(mut self, clock: data::Clock, cx: &mut Context<Self>) -> Self {
        self.ticket
            .update(cx, |ticket, _| ticket.set_clock(clock.clone()));
        self.clock = clock;
        self
    }
}

/// What the page tasks read and set (the shell tests read them too).
#[allow(dead_code)]
impl ContributorsPage {
    pub(crate) fn tab(&self) -> Tab {
        self.tab
    }

    pub(crate) fn project(&self) -> Option<&str> {
        self.project.as_deref()
    }

    pub(crate) fn projects(&self) -> &[String] {
        &self.projects
    }

    pub(crate) fn active_project(&self) -> &str {
        &self.active_project
    }

    /// The core handle once it is open (it opens off the UI thread).
    pub(crate) fn core(&self) -> Option<&Arc<Contributors>> {
        self.core.as_ref()
    }

    /// Why the core could not open, if it could not.
    pub(crate) fn core_error(&self) -> Option<&str> {
        self.core_error.as_deref()
    }

    /// The app-wide job model; follow its `JobsEvent`s.
    pub(crate) fn jobs(&self) -> &Entity<SyncJobs> {
        &self.jobs
    }

    /// The segmented control calls this, so "remember the last tab" reads it.
    pub(crate) fn set_tab(&mut self, tab: Tab, cx: &mut Context<Self>) {
        if self.tab != tab {
            self.tab = tab;
            self.sync_view(cx);
            self.mount_ticket(cx);
            cx.notify();
        }
    }

    pub(crate) fn set_project(&mut self, project: Option<String>, cx: &mut Context<Self>) {
        if self.project != project {
            self.project = project;
            self.sync_view(cx);
            cx.notify();
        }
    }
}

impl ContributorsPage {
    fn tab_strip(&mut self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let options = project_options(&self.projects, self.selected_project().as_deref());
        let selected = self.selected_project();
        let value = options
            .iter()
            .find(|(path, _)| selected.as_deref().is_some_and(|s| same_path(path, s)))
            .map(|(path, _)| path.clone())
            .unwrap_or_default();
        let at = options.iter().position(|(path, _)| *path == value);
        let label = at.map(|at| options[at].1.clone()).unwrap_or_default();
        div()
            .flex_shrink_0()
            .border_b_1()
            .border_color(content(0.1))
            .child(
                div()
                    .mx_auto()
                    .w_full()
                    .max_w(px(1024.))
                    .px(px(32.))
                    .py(px(10.))
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(10.))
                    .child(ui::segmented(
                        "page-tab",
                        "Contributors sources",
                        self.tab.id(),
                        &[
                            (Tab::Ticket.id(), Tab::Ticket.label()),
                            (Tab::Github.id(), Tab::Github.label()),
                        ],
                        cx,
                        |this: &mut Self, id, cx| {
                            if let Some(tab) = Tab::from_id(id) {
                                this.set_tab(tab, cx);
                            }
                        },
                    ))
                    .when(self.tab == Tab::Github && !options.is_empty(), |d| {
                        d.child(div().ml_auto().child(ui::select_trigger(
                            "project",
                            "Project",
                            SharedString::from(label),
                            &self.project_dd,
                            cx,
                            |this: &mut Self| &mut this.project_dd,
                            at.unwrap_or(0),
                        )))
                    }),
            )
            .into_any_element()
    }

    fn no_projects(&self) -> gpui::AnyElement {
        div()
            .mx_auto()
            .w_full()
            .max_w(px(1024.))
            .px(px(32.))
            .py(px(24.))
            .child(
                div()
                    .debug_selector(|| "contributors-no-projects".into())
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap_2()
                    .rounded_xl()
                    .border_1()
                    .border_color(content(0.1))
                    .px_4()
                    .py_8()
                    .child(icon("users", 20.).text_color(content(0.35)))
                    .child(
                        div()
                            .text_size(px(13.))
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .child("No projects yet"),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(content(0.5))
                            .child("Open a project on this computer to see its contributors."),
                    ),
            )
            .into_any_element()
    }
}

impl Render for ContributorsPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if std::mem::take(&mut self.refocus) {
            window.focus(&self.focus, cx);
        }
        let body = match (self.tab, &self.core_error) {
            (_, Some(error)) => div()
                .debug_selector(|| "contributors-unavailable".into())
                .p_4()
                .text_size(px(12.))
                .text_color(content(0.5))
                .child(format!("Contributors is unavailable: {error}"))
                .into_any_element(),
            (Tab::Ticket, None) => self.ticket_body(cx),
            (Tab::Github, None) => match self.current_view() {
                Some(view) => view.into_any_element(),
                None => self.no_projects(),
            },
        };
        let project_list = (self.project_dd.open && self.tab == Tab::Github).then(|| {
            let options = project_options(&self.projects, self.selected_project().as_deref());
            let value = self.selected_project().and_then(|selected| {
                options
                    .iter()
                    .find(|(path, _)| same_path(path, &selected))
                    .map(|(path, _)| path.clone())
            });
            ui::select_list(
                "project",
                "Project",
                value.as_deref().unwrap_or(""),
                options
                    .into_iter()
                    .map(|(path, label)| (path, SharedString::from(label)))
                    .collect(),
                &self.project_dd,
                cx,
                |this: &mut Self| &mut this.project_dd,
                self.focus.clone(),
                |this, path, _, cx| this.set_project(Some(path), cx),
            )
        });
        div()
            .id("contributors-page")
            .debug_selector(|| "contributors-page".into())
            .role(Role::Region)
            .aria_label("Contributors")
            .track_focus(&self.focus)
            .size_full()
            .min_h_0()
            .flex()
            .flex_col()
            .text_color(content(1.))
            .on_key_down(cx.listener(|_, event: &gpui::KeyDownEvent, _, cx| {
                if event.keystroke.key == "escape" {
                    cx.emit(Event::Close);
                    cx.stop_propagation();
                }
            }))
            .child(
                div()
                    .h(px(44.))
                    .flex_shrink_0()
                    .px_4()
                    .flex()
                    .items_center()
                    .gap_2()
                    .border_b_1()
                    .border_color(content(0.07))
                    .child(icon("chart-square", 16.).text_color(content(0.6)))
                    .child(
                        div()
                            .text_size(px(14.))
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .child("Contributors"),
                    ),
            )
            .child(self.tab_strip(cx))
            .child(
                div()
                    .id("contributors-panel-host")
                    .role(Role::TabPanel)
                    .flex_1()
                    .min_h_0()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(body),
            )
            .children(project_list)
            .children(self.ask.clone())
    }
}
