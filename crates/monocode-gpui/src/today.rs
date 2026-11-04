//! Today: the global daily work ledger page. `list`, `board`, `detail`,
//! `statuses` and `sources` render from the state kept here and call its methods;
//! `menus` holds the one open dropdown they share.
mod board;
mod detail;
mod detail_sessions;
mod filter;
mod list;
mod menus;
mod sources;
mod statuses;

pub(crate) use detail_sessions::SessionRow;

use crate::{
    inbox::{action, icon_action, message, plain_action},
    input::{self, Input},
    runtime::{Database, locked},
    theme::*,
};
use gpui::{
    AnyElement, App, Context, CursorStyle, Entity, EventEmitter, FocusHandle, Focusable,
    FontWeight, MouseButton, Render, Role, Subscription, Window, canvas, div, prelude::*, px,
};
use monocode_core::{
    project_appearance::ProjectAppearance,
    store::Store,
    time::now_ms,
    today::{
        Activity, Actor, Day, DayMode, DayView, NewTask, Source, Stage, StatusPatch, SyncStatus,
        Tag, TagPatch, Task, WorkflowStatus, workday::add_days,
    },
};
use std::{cell::Cell, collections::HashMap, rc::Rc, time::Duration};

/// Under this page width the detail replaces the list instead of sitting beside it.
const NARROW_WIDTH: f32 = 720.;
const PANEL_MIN: f32 = 300.;
const PANEL_MAX: f32 = 560.;
const PANEL_WIDTH_KEY: &str = "today.panelWidth";
/// `list` or `board`.
const LAYOUT_KEY: &str = "today.layout";
/// The filters (status, tag, project, source and Mono), as JSON (see `filter::Saved`).
const FILTERS_KEY: &str = "today.filters";

pub(crate) enum Event {
    Close,
    /// Open a new chat session in the task's project with an unsent
    /// `@today/<key>` mention in its composer. The task always has a project.
    StartSession(Box<Task>),
    /// Open a session listed in a task's SESSIONS section.
    OpenSession(i64),
    /// Link `session` to the task, open it and leave an unsent `@today/<key>`
    /// mention in its composer (after any text already there).
    ContinueInSession {
        task: Box<Task>,
        session: i64,
    },
    /// Run these (Mono id, habit id) through the normal habit path.
    SyncNow(Vec<(String, String)>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct HabitInfo {
    pub id: String,
    pub name: String,
    /// `Schedule::label()`.
    pub schedule: String,
    pub enabled: bool,
    /// Queued or running now.
    pub busy: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct MonoInfo {
    pub id: String,
    pub name: String,
    pub mascot: String,
    pub color: u32,
    pub projects: Vec<String>,
    pub habits: Vec<HabitInfo>,
    /// The Mono's habits were read. False before the first load and when its file is
    /// unreadable, so an empty list then does not mean the habits were removed.
    pub habits_known: bool,
}

/// How the Tasks tab lays out the day: grouped rows or status columns.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum Layout {
    #[default]
    List,
    Board,
}
impl Layout {
    fn key(self) -> &'static str {
        match self {
            Self::List => "list",
            Self::Board => "board",
        }
    }
    fn parse(text: &str) -> Option<Self> {
        match text {
            "list" => Some(Self::List),
            "board" => Some(Self::Board),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Tab {
    Tasks,
    Sources,
}

/// `None` shows everything. `project: Some(None)` is the No project rows and
/// `source: Some(None)` the manual ones. `statuses` and `tags` are multi-select:
/// empty shows everything. See `filter` for how they match.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct Filters {
    pub project: Option<Option<String>>,
    pub source: Option<Option<String>>,
    pub mono: Option<String>,
    /// Workflow status ids.
    pub statuses: std::collections::BTreeSet<String>,
    /// Tag ids.
    pub tags: std::collections::BTreeSet<String>,
    /// Also match tasks that have no tag.
    pub no_tag: bool,
    /// Tags: a task needs all the selected ones, not any of them.
    pub match_all: bool,
}

pub(crate) struct Today {
    pub(super) store: Database,
    pub(super) focus: FocusHandle,
    pub(super) tab: Tab,
    /// The day on screen; `None` follows the current work day.
    pub(super) day: Option<Day>,
    pub(super) view: Option<DayView>,
    pub(super) sources: Vec<Source>,
    pub(super) roster: Vec<MonoInfo>,
    /// `set_roster` ran at least once; an empty roster before that is not "no Monos".
    pub(super) roster_loaded: bool,
    pub(super) appearances: HashMap<String, ProjectAppearance>,
    pub(super) filters: Filters,
    pub(super) selected: Option<String>,
    /// The selected task's events and notes, oldest first; a past day stops at that day.
    pub(super) activity: Vec<Activity>,
    /// The workflow statuses in order: list groups and board columns.
    pub(super) statuses: Vec<WorkflowStatus>,
    /// The app wide tag definitions in order: chips, the Tags menu and filter.
    pub(super) tags: Vec<Tag>,
    pub(super) layout: Layout,
    /// The Statuses dialog, while open.
    pub(super) statuses_dialog: Option<statuses::Dialog>,
    /// The Tags dialog, while open. Never open together with the Statuses one.
    pub(super) tags_dialog: Option<statuses::Dialog>,
    /// Board-only state (drag, column menu, per-column new task).
    pub(super) board: board::Board,
    pub(super) new_task: Entity<Input>,
    pub(super) note: Entity<Input>,
    /// The detail's title field, for manual tasks.
    pub(super) title: detail::TitleEdit,
    /// The selected task's sessions (see `detail_sessions`).
    pub(super) sessions: detail_sessions::TaskSessions,
    pub(super) panel_width: f32,
    pub(super) resize_start: Option<(f32, f32)>,
    pub(super) error: Option<String>,
    /// A failed action. A reload does not clear it, so the message stays up.
    pub(super) action_error: Option<String>,
    /// The open filter, Snooze, Priority or Project menu (see `menus::Menus`).
    pub(super) menus: menus::Menus,
    pub(super) open_count: usize,
    /// Stale loads are dropped: only the latest `refresh` applies.
    generation: u64,
    notes_generation: u64,
    rollover: Option<u32>,
    width_loaded: bool,
    /// Set once the layout was picked here, so the stored one no longer applies.
    layout_chosen: bool,
    /// Set once the filters were changed here, so the stored ones no longer apply.
    filters_chosen: bool,
    /// A filters save is running; a newer value waits in `filters_pending`.
    filters_saving: bool,
    filters_pending: Option<String>,
    /// A task was just added that the filters hide: say so, with a way out.
    added_hidden: bool,
    /// Measured by the page itself, read by the next render.
    page_width: Rc<Cell<f32>>,
    _subscriptions: Vec<Subscription>,
}

/// What one background load brings back.
struct Loaded {
    view: DayView,
    sources: Vec<Source>,
    statuses: Vec<WorkflowStatus>,
    tags: Vec<Tag>,
    open_count: usize,
    rollover: u32,
    panel_width: Option<f32>,
    layout: Option<Layout>,
    filters: Option<filter::Saved>,
}

impl EventEmitter<Event> for Today {}
impl Focusable for Today {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Today {
    pub(crate) fn new(store: Database, cx: &mut Context<Self>) -> Self {
        let this = Self::new_inner(store, cx);
        cx.spawn(async move |this, cx| {
            if this.update(cx, |this, cx| this.refresh(cx)).is_err() {
                return;
            }
            // Cheap enough to run always: it picks up a rollover and agent syncs.
            loop {
                cx.background_executor()
                    .timer(Duration::from_secs(30))
                    .await;
                if this.update(cx, |this, cx| this.refresh(cx)).is_err() {
                    break;
                }
            }
        })
        .detach();
        this
    }
    fn new_inner(store: Database, cx: &mut Context<Self>) -> Self {
        let new_task = cx.new(|cx| Input::field("New task", "New task", content(0.4), cx));
        let note = cx.new(Input::inbox_comment);
        let title = detail::TitleEdit::new(cx);
        let subscriptions = vec![
            cx.subscribe(&new_task, |this, _, _: &input::Submitted, cx| {
                this.add_task(cx)
            }),
            cx.subscribe(&note, |this, _, _: &input::Submitted, cx| this.add_note(cx)),
        ];
        Self {
            store,
            focus: cx.focus_handle(),
            tab: Tab::Tasks,
            day: None,
            view: None,
            sources: Vec::new(),
            roster: Vec::new(),
            roster_loaded: false,
            appearances: HashMap::new(),
            filters: Filters::default(),
            selected: None,
            activity: Vec::new(),
            statuses: monocode_core::today::statuses::default_statuses(),
            tags: Vec::new(),
            layout: Layout::default(),
            statuses_dialog: None,
            tags_dialog: None,
            board: board::Board::default(),
            new_task,
            note,
            title,
            sessions: Default::default(),
            panel_width: 360.,
            resize_start: None,
            error: None,
            action_error: None,
            menus: menus::Menus::default(),
            open_count: 0,
            generation: 0,
            notes_generation: 0,
            rollover: None,
            width_loaded: false,
            layout_chosen: false,
            filters_chosen: false,
            filters_saving: false,
            filters_pending: None,
            added_hidden: false,
            page_width: Rc::new(Cell::new(f32::MAX)),
            _subscriptions: subscriptions,
        }
    }
    #[cfg(test)]
    pub(crate) fn new_quiet(store: Database, cx: &mut Context<Self>) -> Self {
        Self::new_inner(store, cx)
    }
    /// Reloads off the UI thread; also what picks up a rollover and agent syncs.
    pub(crate) fn refresh(&mut self, cx: &mut Context<Self>) {
        self.load_session_links(cx);
        self.generation += 1;
        let generation = self.generation;
        let store = self.store.clone();
        let viewed = self.day.clone();
        let load_width = !self.width_loaded;
        let work = cx.background_executor().spawn(async move {
            let db = locked(&store)?;
            let now = now_ms();
            let current = db.today_roll(now)?;
            let day = viewed.unwrap_or(current);
            Ok::<_, String>(Loaded {
                view: db.today_view(&day, now)?,
                sources: db.today_sources()?,
                statuses: db.today_statuses()?,
                tags: db.today_tags()?,
                open_count: db.today_open_count(now)?,
                rollover: db.today_rollover_minutes()?,
                panel_width: if load_width {
                    db.setting(PANEL_WIDTH_KEY)?.and_then(|v| v.parse().ok())
                } else {
                    None
                },
                // Loaded with the width: once, and skipped when a pick was made meanwhile.
                layout: if load_width {
                    db.setting(LAYOUT_KEY)?.and_then(|v| Layout::parse(&v))
                } else {
                    None
                },
                filters: if load_width {
                    db.setting(FILTERS_KEY)?
                        .and_then(|v| filter::Saved::parse(&v))
                } else {
                    None
                },
            })
        });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |this, cx| {
                if this.generation != generation {
                    return;
                }
                match result {
                    Ok(loaded) => this.apply(loaded, cx),
                    Err(error) => {
                        this.error = Some(error);
                        cx.notify();
                    }
                }
            });
        })
        .detach();
    }
    fn apply(&mut self, loaded: Loaded, cx: &mut Context<Self>) {
        if let Some(width) = loaded.panel_width {
            self.panel_width = width.clamp(PANEL_MIN, PANEL_MAX);
        }
        if let Some(layout) = loaded.layout.filter(|_| !self.layout_chosen) {
            self.layout = layout;
        }
        self.width_loaded = true;
        self.rollover = Some(loaded.rollover);
        self.statuses = loaded.statuses;
        self.tags = loaded.tags;
        if let Some(saved) = loaded.filters.filter(|_| !self.filters_chosen) {
            self.filters = saved.apply(std::mem::take(&mut self.filters));
        }
        let before_prune = self.filters.clone();
        // A status or tag deleted since is no longer a filter.
        self.filters = filter::pruned(
            std::mem::take(&mut self.filters),
            &self.statuses,
            &self.tags,
        );
        self.sources = loaded.sources;
        self.filters = filter::pruned_source(std::mem::take(&mut self.filters), &self.sources);
        if self.roster_loaded {
            self.filters = filter::pruned_mono(std::mem::take(&mut self.filters), &self.roster);
        }
        // A pruned filter may have been the one hiding a task just added.
        if self.filters != before_prune {
            self.added_hidden = false;
        }
        self.open_count = loaded.open_count;
        self.error = None;
        // Viewing "today" by date once it is the current day goes back to following it.
        if self.day.as_deref() == Some(loaded.view.current_day.as_str()) {
            self.day = None;
        }
        let vanished = self
            .selected
            .as_ref()
            .is_some_and(|id| !loaded.view.rows.iter().any(|row| &row.task.id == id));
        self.view = Some(loaded.view);
        if vanished {
            self.selected = None;
            self.activity.clear();
        } else if self.selected.is_some() {
            self.load_activity(cx);
        }
        self.refresh_tags_menu(cx);
        self.refresh_filter_menu(cx);
        cx.notify();
    }
    fn load_activity(&mut self, cx: &mut Context<Self>) {
        self.notes_generation += 1;
        let generation = self.notes_generation;
        let Some(id) = self.selected.clone() else {
            self.activity.clear();
            return;
        };
        let store = self.store.clone();
        let work = {
            let id = id.clone();
            cx.background_executor()
                .spawn(async move { locked(&store)?.today_activity(&id) })
        };
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |this, cx| {
                if this.notes_generation != generation || this.selected.as_ref() != Some(&id) {
                    return;
                }
                match result {
                    Ok(mut activity) => {
                        // A past day shows the timeline as it stood that day.
                        if let Some(view) = this.view.as_ref().filter(|v| v.mode == DayMode::Past) {
                            activity.retain(|entry| entry.day() <= view.day.as_str());
                        }
                        this.activity = activity;
                    }
                    Err(error) => this.error = Some(error),
                }
                cx.notify();
            });
        })
        .detach();
    }
    pub(crate) fn open_count(&self) -> usize {
        self.open_count
    }
    /// Minutes after midnight when the work day starts, once loaded.
    pub(crate) fn rollover_minutes(&self) -> Option<u32> {
        self.rollover
    }
    /// Saves the setting off the UI thread, then reloads on the new rollover.
    pub(crate) fn set_rollover(&mut self, minutes: u32, cx: &mut Context<Self>) {
        let minutes = minutes.min(720);
        self.rollover = Some(minutes);
        self.act(
            move |store, _| store.save_today_rollover_minutes(minutes),
            cx,
        );
    }
    pub(crate) fn set_roster(&mut self, roster: Vec<MonoInfo>, cx: &mut Context<Self>) {
        let first = !self.roster_loaded;
        self.roster_loaded = true;
        if first || self.roster != roster {
            self.roster = roster;
            let before = self.filters.clone();
            self.filters = filter::pruned_mono(std::mem::take(&mut self.filters), &self.roster);
            if self.filters != before {
                self.added_hidden = false;
            }
            cx.notify();
        }
    }
    pub(crate) fn set_appearances(
        &mut self,
        appearances: HashMap<String, ProjectAppearance>,
        cx: &mut Context<Self>,
    ) {
        self.appearances = appearances;
        cx.notify();
    }

    pub(super) fn mono(&self, id: &str) -> Option<&MonoInfo> {
        self.roster.iter().find(|mono| mono.id == id)
    }
    pub(super) fn habit(&self, mono: &str, habit: &str) -> Option<&HabitInfo> {
        self.mono(mono)?.habits.iter().find(|h| h.id == habit)
    }
    /// The project's display name, from its appearance as in Inbox.
    pub(super) fn project_label(&self, path: &str) -> String {
        self.appearances
            .get(path)
            .cloned()
            .unwrap_or_default()
            .label(path)
    }
    /// The rows that pass the filters, in the view's order.
    pub(super) fn visible_rows(&self) -> Vec<&monocode_core::today::Row> {
        let Some(view) = &self.view else {
            return Vec::new();
        };
        view.rows
            .iter()
            .filter(|row| filter::matches(&self.filters, row, &self.statuses))
            .collect()
    }
    pub(super) fn selected_row(&self) -> Option<&monocode_core::today::Row> {
        let id = self.selected.as_ref()?;
        self.view
            .as_ref()?
            .rows
            .iter()
            .find(|row| &row.task.id == id)
    }
    /// Runs store work off the UI thread, then reloads; errors land in `error`.
    pub(super) fn act(
        &mut self,
        work: impl FnOnce(&Store, u64) -> Result<(), String> + Send + 'static,
        cx: &mut Context<Self>,
    ) {
        self.act_inner(None, work, cx);
    }
    fn act_inner(
        &mut self,
        added_in: Option<String>,
        work: impl FnOnce(&Store, u64) -> Result<(), String> + Send + 'static,
        cx: &mut Context<Self>,
    ) {
        let store = self.store.clone();
        let task = cx
            .background_executor()
            .spawn(async move { work(&*locked(&store)?, now_ms()) });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                this.action_error = result.as_ref().err().cloned();
                if let (Ok(()), Some(status_id)) = (&result, &added_in) {
                    this.added_hidden = filter::hides_new_task(&this.filters, status_id);
                }
                this.refresh(cx);
            });
        })
        .detach();
    }
    pub(super) fn select(&mut self, id: Option<String>, cx: &mut Context<Self>) {
        if self.selected == id {
            return;
        }
        self.selected = id;
        self.activity.clear();
        // A draft belongs to the task it was typed on.
        self.note
            .update(cx, |input, cx| input.set_text(String::new(), cx));
        self.load_activity(cx);
        cx.notify();
    }
    pub(super) fn show_tab(&mut self, tab: Tab, cx: &mut Context<Self>) {
        if self.tab != tab {
            self.tab = tab;
            self.board.adding = None;
            self.board.menu = None;
            self.menus.close();
            cx.notify();
        }
    }
    /// The work day on screen.
    fn viewed_day(&self) -> Option<&str> {
        self.view.as_ref().map(|view| view.day.as_str())
    }
    fn current_day(&self) -> Option<&str> {
        self.view.as_ref().map(|view| view.current_day.as_str())
    }
    /// Moves the date strip by `delta` days.
    pub(super) fn step_day(&mut self, delta: i64, cx: &mut Context<Self>) {
        // From the day asked for, so two quick clicks move two days.
        let from = self.day.as_deref().or(self.viewed_day());
        let Some(next) = from.and_then(|day| add_days(day, delta)) else {
            return;
        };
        self.show_day(Some(next), cx);
    }
    pub(super) fn show_day(&mut self, day: Option<Day>, cx: &mut Context<Self>) {
        let day = day.filter(|day| Some(day.as_str()) != self.current_day());
        if self.day == day {
            return;
        }
        self.day = day;
        self.added_hidden = false;
        self.refresh(cx);
        cx.notify();
    }
    pub(super) fn set_filters(&mut self, filters: Filters, cx: &mut Context<Self>) {
        let saved = filter::Saved::of(&filters);
        let changed = saved != filter::Saved::of(&self.filters);
        self.filters = filters;
        self.added_hidden = false;
        if changed {
            self.filters_chosen = true;
            self.save_filters(saved.to_json(), cx);
        }
        cx.notify();
    }
    /// Writes the filters setting off the UI thread, one write at a time: a value
    /// that arrives meanwhile waits and replaces any older waiting one, so the
    /// newest is always the last stored.
    fn save_filters(&mut self, json: String, cx: &mut Context<Self>) {
        if self.filters_saving {
            self.filters_pending = Some(json);
            return;
        }
        self.filters_saving = true;
        let store = self.store.clone();
        let work = cx.background_executor().spawn(async move {
            let _ = locked(&store).and_then(|db| db.save_setting(FILTERS_KEY, &json));
        });
        cx.spawn(async move |this, cx| {
            work.await;
            let _ = this.update(cx, |this, cx| {
                this.filters_saving = false;
                if let Some(next) = this.filters_pending.take() {
                    this.save_filters(next, cx);
                }
            });
        })
        .detach();
    }
    /// Like [`Self::act`] for work that adds a task in `status_id`: once the add
    /// succeeded, remembers whether the filters hide the new task. A failed add
    /// leaves nothing to explain.
    pub(super) fn act_add(
        &mut self,
        status_id: &str,
        work: impl FnOnce(&Store, u64) -> Result<(), String> + Send + 'static,
        cx: &mut Context<Self>,
    ) {
        let status_id = status_id.to_string();
        self.act_inner(Some(status_id), work, cx);
    }
    /// Enter in the New task field: adds to the day on screen.
    pub(super) fn add_task(&mut self, cx: &mut Context<Self>) {
        let Some(view) = &self.view else { return };
        // A past day is a record: nothing is added to it.
        if view.mode == DayMode::Past {
            return;
        }
        let day = view.day.clone();
        let Some(title) = self.new_task.update(cx, |input, cx| input.take(cx)) else {
            return;
        };
        let new = NewTask {
            title: title.trim().to_string(),
            day: Some(day),
            ..Default::default()
        };
        self.act_add(
            monocode_core::today::statuses::TODO_ID,
            move |store, now| store.today_add(&new, &Actor::User, now).map(|_| ()),
            cx,
        );
    }
    /// The round check: done, or back to open when it already is.
    pub(super) fn toggle_done(&mut self, id: String, cx: &mut Context<Self>) {
        let done = self
            .view
            .as_ref()
            .and_then(|view| view.rows.iter().find(|row| row.task.id == id))
            .is_some_and(|row| row.status.stage == Stage::Finished);
        self.act(
            move |store, now| {
                if done {
                    store.today_reopen(&id, now)
                } else {
                    store.today_complete(&id, &Actor::User, now)
                }
                .map(|_| ())
            },
            cx,
        );
    }
    pub(super) fn snooze(&mut self, id: String, until: Day, cx: &mut Context<Self>) {
        self.act(
            move |store, now| {
                store
                    .today_snooze(&id, &until, &Actor::User, now)
                    .map(|_| ())
            },
            cx,
        );
    }
    pub(super) fn dismiss(&mut self, id: String, cx: &mut Context<Self>) {
        self.act(
            move |store, now| store.today_dismiss(&id, &Actor::User, now).map(|_| ()),
            cx,
        );
    }
    pub(super) fn add_note(&mut self, cx: &mut Context<Self>) {
        if self.view.as_ref().is_none_or(|v| v.mode == DayMode::Past) {
            return;
        }
        let Some(id) = self.selected.clone() else {
            return;
        };
        let Some(body) = self.note.update(cx, |input, cx| input.take(cx)) else {
            return;
        };
        self.act(
            move |store, now| {
                store
                    .today_note(&id, body.trim(), &Actor::User, now)
                    .map(|_| ())
            },
            cx,
        );
    }
    /// A source whose Mono is gone, or whose habit is absent from habits that were really
    /// read: nothing can sync it any more, so it raises no band. The Sources tab still
    /// lists it. Until a roster arrived, or while a Mono's habits are unknown or
    /// unreadable, nothing is judged: a failed or stale source must keep its band.
    pub(super) fn retired(&self, source: &Source) -> bool {
        if !self.roster_loaded {
            return false;
        }
        match self.mono(&source.mono_id) {
            None => true,
            Some(mono) => {
                mono.habits_known
                    && source
                        .habit_id
                        .as_deref()
                        .is_some_and(|habit| self.habit(&source.mono_id, habit).is_none())
            }
        }
    }
    /// The (Mono id, habit id) pairs a sync would ask for. A habit already
    /// queued or running is left alone. Asking for every source also leaves paused
    /// habits alone; asking for one source runs it even when paused.
    fn sync_targets(&self, source: Option<&str>) -> Vec<(String, String)> {
        self.sources
            .iter()
            .filter(|s| source.is_none_or(|id| s.id == id))
            .filter_map(|s| Some((s.mono_id.clone(), s.habit_id.clone()?)))
            // A Mono or habit that is gone cannot run, and a busy one is already running.
            .filter(|(mono, habit)| {
                self.habit(mono, habit)
                    .is_some_and(|h| !h.busy && (source.is_some() || h.enabled))
            })
            .collect()
    }
    /// At least one source has a habit that could run.
    fn can_sync(&self) -> bool {
        !self.sync_targets(None).is_empty()
    }
    /// A source's habit is queued or running now.
    fn syncing(&self) -> bool {
        self.sources.iter().any(|s| {
            s.habit_id
                .as_deref()
                .is_some_and(|habit| self.habit(&s.mono_id, habit).is_some_and(|h| h.busy))
        })
    }
    /// Asks one source's habit, or every source's when `None`.
    pub(super) fn sync(&mut self, source: Option<String>, cx: &mut Context<Self>) {
        let targets = self.sync_targets(source.as_deref());
        if !targets.is_empty() {
            cx.emit(Event::SyncNow(targets));
        }
    }
    /// Start session: a task with a project starts at once; one without asks
    /// for a project first (the `StartProject` menu, drawn by the detail panel),
    /// then [`Self::start_with_project`] sets it and starts.
    pub(super) fn start_session(
        &mut self,
        task: Task,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if task.project.as_deref().is_some_and(|p| !p.is_empty()) {
            cx.emit(Event::StartSession(Box::new(task)));
            return;
        }
        let options = self.project_options();
        if options.is_empty() {
            self.action_error = Some("Add a project first: no project is known yet.".into());
            cx.notify();
            return;
        }
        let mut rows = menus::Rows::default();
        for (ix, (label, path)) in options.into_iter().enumerate() {
            rows.add(
                self.project_row(format!("start-project-{ix}"), label, &path, false),
                menus::Pick::StartWithProject {
                    id: task.id.clone(),
                    project: path,
                },
            );
        }
        self.open_menu(
            menus::Slot::StartProject,
            "Start in project",
            rows,
            window,
            cx,
        );
    }
    /// Sets the project chosen for Start session, then starts.
    pub(super) fn start_with_project(
        &mut self,
        id: String,
        project: String,
        cx: &mut Context<Self>,
    ) {
        let store = self.store.clone();
        let work = cx.background_executor().spawn(async move {
            let patch = monocode_core::today::TaskPatch {
                project: Some(Some(project)),
                ..Default::default()
            };
            locked(&store)?.today_update(&id, &patch, &Actor::User, now_ms())
        });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(task) => cx.emit(Event::StartSession(Box::new(task))),
                    Err(error) => this.action_error = Some(error),
                }
                this.refresh(cx);
            });
        })
        .detach();
    }

    // --- workflow statuses and layout -----------------------------------------------------

    /// A workflow status by id, from the current list (a past day reads the same list).
    pub(super) fn status_of(&self, id: &str) -> Option<&WorkflowStatus> {
        self.statuses.iter().find(|s| s.id == id)
    }
    pub(super) fn set_layout(&mut self, layout: Layout, cx: &mut Context<Self>) {
        if self.layout == layout {
            return;
        }
        self.layout = layout;
        self.layout_chosen = true;
        self.board.adding = None;
        self.board.menu = None;
        self.menus.close();
        let store = self.store.clone();
        cx.background_executor()
            .spawn(async move {
                let _ = locked(&store).and_then(|db| db.save_setting(LAYOUT_KEY, layout.key()));
            })
            .detach();
        cx.notify();
    }
    /// Sets a task's workflow status by hand (status menu, board drop).
    pub(super) fn set_status(&mut self, id: String, status_id: String, cx: &mut Context<Self>) {
        self.act(
            move |store, now| {
                store
                    .today_set_status(&id, &status_id, &Actor::User, now)
                    .map(|_| ())
            },
            cx,
        );
    }
    /// Needs check callout: Keep done.
    pub(super) fn keep_done(&mut self, id: String, cx: &mut Context<Self>) {
        self.act(
            move |store, now| store.today_keep_done(&id, now).map(|_| ()),
            cx,
        );
    }
    pub(super) fn open_statuses(&mut self, cx: &mut Context<Self>) {
        self.menus.close();
        self.tags_dialog = None;
        self.statuses_dialog = Some(self.new_dialog(statuses::Kind::Statuses, cx));
        cx.notify();
    }
    pub(super) fn close_statuses(&mut self, cx: &mut Context<Self>) {
        if self.statuses_dialog.take().is_some() {
            cx.notify();
        }
    }
    pub(super) fn open_tags(&mut self, cx: &mut Context<Self>) {
        self.menus.close();
        self.statuses_dialog = None;
        self.tags_dialog = Some(self.new_dialog(statuses::Kind::Tags, cx));
        cx.notify();
    }
    pub(super) fn close_tags(&mut self, cx: &mut Context<Self>) {
        if self.tags_dialog.take().is_some() {
            cx.notify();
        }
    }
    pub(super) fn add_status(
        &mut self,
        name: String,
        stage: Stage,
        color: String,
        cx: &mut Context<Self>,
    ) {
        self.act(
            move |store, _| store.today_add_status(&name, stage, &color).map(|_| ()),
            cx,
        );
    }
    pub(super) fn update_status(&mut self, id: String, patch: StatusPatch, cx: &mut Context<Self>) {
        self.act(
            move |store, now| store.today_update_status(&id, &patch, now).map(|_| ()),
            cx,
        );
    }
    pub(super) fn move_status(
        &mut self,
        id: String,
        stage: Stage,
        index: usize,
        cx: &mut Context<Self>,
    ) {
        self.act(
            move |store, now| store.today_move_status(&id, stage, index, now).map(|_| ()),
            cx,
        );
    }
    pub(super) fn delete_status(&mut self, id: String, cx: &mut Context<Self>) {
        self.act(
            move |store, now| store.today_delete_status(&id, now).map(|_| ()),
            cx,
        );
    }

    // --- tags --------------------------------------------------------------------------------

    pub(super) fn add_tag(&mut self, name: String, color: String, cx: &mut Context<Self>) {
        self.act(
            move |store, _| store.today_add_tag(&name, &color).map(|_| ()),
            cx,
        );
    }
    pub(super) fn update_tag(&mut self, id: String, patch: TagPatch, cx: &mut Context<Self>) {
        self.act(
            move |store, _| store.today_update_tag(&id, &patch).map(|_| ()),
            cx,
        );
    }
    pub(super) fn move_tag(&mut self, id: String, index: usize, cx: &mut Context<Self>) {
        self.act(
            move |store, _| store.today_move_tag(&id, index).map(|_| ()),
            cx,
        );
    }
    /// Deletes a tag outright; the confirmation is the caller's.
    pub(super) fn delete_tag(&mut self, id: String, cx: &mut Context<Self>) {
        self.act(
            move |store, now| store.today_delete_tag(&id, now).map(|_| ()),
            cx,
        );
    }
    fn narrow(&self) -> bool {
        self.page_width.get() < NARROW_WIDTH
    }
}

/// `Oct 31` for a `YYYY-MM-DD` work day.
pub(super) fn short_day(day: &str) -> String {
    chrono::NaiveDate::parse_from_str(day, "%Y-%m-%d")
        .map(|date| date.format("%b %-d").to_string())
        .unwrap_or_else(|_| day.to_string())
}
/// `Sunday, November 1`.
fn long_day(day: &str) -> String {
    chrono::NaiveDate::parse_from_str(day, "%Y-%m-%d")
        .map(|date| date.format("%A, %B %-d").to_string())
        .unwrap_or_else(|_| day.to_string())
}
/// `HH:MM` local time of an epoch-millisecond instant.
pub(super) fn clock(ms: u64) -> String {
    use chrono::TimeZone;
    chrono::Local
        .timestamp_millis_opt(ms as i64)
        .single()
        .map(|time| time.format("%H:%M").to_string())
        .unwrap_or_default()
}

impl Today {
    /// The latest successful sync: a time for today's, a day and time otherwise.
    fn synced_caption(&self) -> Option<String> {
        let (at, day) = self
            .sources
            .iter()
            .filter_map(|s| Some((s.last_sync_at?, s.last_sync_day.clone()?)))
            .max()?;
        Some(if self.current_day() == Some(day.as_str()) {
            format!("Synced {}", clock(at))
        } else {
            format!("Last synced {}, {}", short_day(&day), clock(at))
        })
    }
    fn page_header(&self, cx: &mut Context<Self>) -> AnyElement {
        let view = self.view.as_ref();
        let subtitle = [
            view.map(|v| long_day(&v.current_day)),
            self.synced_caption(),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" · ");
        let syncing = self.syncing();
        let disabled = !self.can_sync();
        let label = if syncing { "Syncing" } else { "Sync now" };
        div()
            .px(px(20.))
            .pt(px(16.))
            .flex()
            .items_start()
            .gap_3()
            .child(
                div()
                    .min_w_0()
                    .flex_1()
                    .child(
                        div()
                            .text_size(px(20.))
                            .line_height(px(25.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(content(1.))
                            .child("Today"),
                    )
                    .child(
                        div()
                            .debug_selector(|| "today-subtitle".into())
                            .mt(px(2.))
                            .text_size(px(12.))
                            .text_color(content(0.45))
                            .child(subtitle),
                    ),
            )
            .when(self.tab == Tab::Tasks, |d| {
                d.child(self.layout_switch(cx))
                    .child(
                        icon_action("today-statuses", "Statuses", "sliders")
                            .debug_selector(|| "today-statuses".into())
                            .on_click(cx.listener(|this, _, _, cx| this.open_statuses(cx))),
                    )
                    .child(
                        icon_action("today-tags", "Tags", "tag")
                            .debug_selector(|| "today-tags".into())
                            .on_click(cx.listener(|this, _, _, cx| this.open_tags(cx))),
                    )
            })
            .child(
                crate::modal::disabled(
                    icon_action(
                        "today-sync",
                        label,
                        if syncing { "loader" } else { "refresh-cw" },
                    )
                    .debug_selector(|| "today-sync".into()),
                    disabled,
                )
                .when(!disabled, |d| {
                    d.on_click(cx.listener(|this, _, _, cx| this.sync(None, cx)))
                }),
            )
            .into_any_element()
    }

    /// The List / Board switch: two buttons in one pill, the chosen one lit.
    fn layout_switch(&self, cx: &mut Context<Self>) -> AnyElement {
        div()
            .id("today-layout")
            .role(Role::Group)
            .aria_label("Layout")
            .p(px(2.))
            .flex()
            .items_center()
            .gap(px(2.))
            .rounded(px(8.))
            .bg(content(0.05))
            .children(
                [
                    (Layout::List, "today-layout-list", "List", "list-bullet"),
                    (Layout::Board, "today-layout-board", "Board", "dashboard"),
                ]
                .map(|(layout, id, label, glyph)| {
                    let chosen = self.layout == layout;
                    icon_action(id, label, glyph)
                        .debug_selector(move || id.to_string())
                        .h(px(24.))
                        .px(px(8.))
                        .when(chosen, |d| d.bg(content(0.1)).text_color(content(1.)))
                        .aria_selected(chosen)
                        .on_click(cx.listener(move |this, _, _, cx| this.set_layout(layout, cx)))
                }),
            )
            .into_any_element()
    }

    /// The same tab treatment as Inbox's Summary, Code and Checks.
    fn tab_strip(&self, cx: &mut Context<Self>) -> AnyElement {
        div()
            .id("today-tabs")
            .debug_selector(|| "today-tabs".into())
            .role(Role::TabList)
            .aria_label("Today")
            .h(px(36.))
            .px(px(20.))
            .flex()
            .items_stretch()
            .gap(px(16.))
            .children(
                [(Tab::Tasks, "Tasks"), (Tab::Sources, "Sources")].map(|(tab, label)| {
                    plain_action(format!("today-tab-{label}"), label)
                        .role(Role::Tab)
                        .h_full()
                        .px_0()
                        .rounded_none()
                        .hover(|s| s.text_color(content(1.)))
                        .when(self.tab == tab, |d| {
                            d.border_b_2()
                                .border_color(content(1.))
                                .text_color(content(1.))
                        })
                        .on_click(cx.listener(move |this, _, _, cx| this.show_tab(tab, cx)))
                }),
            )
            .into_any_element()
    }

    /// Previous and next, and the days around the one on screen.
    fn day_strip(&self, cx: &mut Context<Self>) -> AnyElement {
        let (Some(shown), Some(current)) = (self.viewed_day(), self.current_day()) else {
            return div().into_any_element();
        };
        let days: Vec<Day> = (-3..=3)
            .filter_map(|delta| add_days(shown, delta))
            .collect();
        div()
            .id("today-days")
            .px(px(14.))
            .pt(px(8.))
            .flex()
            .items_center()
            .gap(px(2.))
            .child(
                icon_button("today-day-prev", "Previous day", "chevron-left")
                    .on_click(cx.listener(|this, _, _, cx| this.step_day(-1, cx))),
            )
            .children(days.into_iter().map(|day| {
                let selected = day == shown;
                let label = if day == current {
                    "Today".to_string()
                } else {
                    short_day(&day)
                };
                let target = day.clone();
                action(format!("today-day-{day}"), label)
                    .when(selected, |d| d.bg(content(0.1)).text_color(content(1.)))
                    .aria_selected(selected)
                    .on_click(
                        cx.listener(move |this, _, _, cx| this.show_day(Some(target.clone()), cx)),
                    )
            }))
            .child(
                icon_button("today-day-next", "Next day", "chevron-right")
                    .on_click(cx.listener(|this, _, _, cx| this.step_day(1, cx))),
            )
            .into_any_element()
    }

    /// The summary line, source warnings and the read-only line of the day on screen.
    fn notices(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let Some(view) = &self.view else {
            return Vec::new();
        };
        let band = || {
            div()
                .mx(px(20.))
                .mt(px(8.))
                .px(px(12.))
                .py(px(8.))
                .rounded_md()
                .border_1()
                .border_color(content(0.1))
                .bg(content(0.05))
                .flex()
                .items_center()
                .gap_2()
        };
        let mut out = Vec::new();
        match view.mode {
            DayMode::Current => {
                let s = if filter::active_count(&self.filters) == 0 {
                    view.summary
                } else {
                    filter::summary(&self.visible_rows())
                };
                let dot = || div().text_color(content(0.3)).child("·");
                out.push(
                    div()
                        .id("today-summary")
                        .debug_selector(|| "today-summary".into())
                        .px(px(20.))
                        .pt(px(10.))
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .text_size(px(13.))
                        .text_color(content(0.6))
                        .child(
                            div()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(content(1.))
                                .child(format!("{} open", s.open)),
                        )
                        .child(dot())
                        .child(format!("{} carried over", s.carried_over))
                        .child(dot())
                        .child(format!("{} to check", s.to_check))
                        .into_any_element(),
                );
                for source in self
                    .sources
                    .iter()
                    .filter(|s| s.last_status == SyncStatus::Failed && !self.retired(s))
                {
                    let at = source.last_attempt_at.map(clock).unwrap_or_default();
                    let from = source
                        .last_sync_day
                        .as_deref()
                        .map_or("No earlier data.".to_string(), |day| {
                            format!("Showing data from {}.", short_day(day))
                        });
                    let id = source.id.clone();
                    let retry_ok = !self.sync_targets(Some(&source.id)).is_empty();
                    out.push(
                        band()
                            .id(format!("today-failed-{}", source.id))
                            .debug_selector(|| "today-failed".into())
                            .child(
                                message(format!("{} sync failed at {at}. {from}", source.name))
                                    .text_color(warning_color())
                                    .p_0()
                                    .flex_1()
                                    .min_w_0(),
                            )
                            .child(crate::modal::disabled(
                                action("today-retry", "Retry").when(retry_ok, |d| {
                                    d.on_click(cx.listener(move |this, _, _, cx| {
                                        this.sync(Some(id.clone()), cx)
                                    }))
                                }),
                                !retry_ok,
                            ))
                            .child(action("today-open-source", "Open source").on_click(
                                cx.listener(|this, _, _, cx| this.show_tab(Tab::Sources, cx)),
                            ))
                            .into_any_element(),
                    );
                }
                let live: Vec<Source> = self
                    .sources
                    .iter()
                    .filter(|s| !self.retired(s))
                    .cloned()
                    .collect();
                let unsynced = monocode_core::today::not_synced(&live, &view.current_day);
                if unsynced {
                    let disabled = !self.can_sync();
                    out.push(
                        band()
                            .id("today-not-synced")
                            .debug_selector(|| "today-not-synced".into())
                            .child(
                                message("Not synced today. What you see may be out of date.")
                                    .text_color(warning_color())
                                    .p_0()
                                    .flex_1()
                                    .min_w_0(),
                            )
                            .child(crate::modal::disabled(
                                action("today-not-synced-sync", "Sync now").when(!disabled, |d| {
                                    d.on_click(cx.listener(|this, _, _, cx| this.sync(None, cx)))
                                }),
                                disabled,
                            ))
                            .into_any_element(),
                    );
                }
            }
            DayMode::Past => out.push(
                message(if view.recorded {
                    format!(
                        "Read-only record of {}. Items keep the state they had that day.",
                        short_day(&view.day)
                    )
                } else {
                    format!("Nothing was recorded on {}.", short_day(&view.day))
                })
                .id("today-read-only")
                .debug_selector(|| "today-read-only".into())
                .px(px(20.))
                .pb_0()
                .into_any_element(),
            ),
            DayMode::Future => {}
        }
        if self.added_hidden && view.mode != DayMode::Past {
            out.push(
                band()
                    .debug_selector(|| "today-added-hidden".into())
                    .child(
                        message("Added. Hidden by the current filters.")
                            .p_0()
                            .flex_1()
                            .min_w_0(),
                    )
                    .child(
                        action("today-added-clear", "Clear filters")
                            .debug_selector(|| "today-added-clear".into())
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.set_filters(Filters::default(), cx)
                            })),
                    )
                    .into_any_element(),
            );
        }
        out
    }

    fn main_column(&self, cx: &mut Context<Self>) -> AnyElement {
        let tasks = self.tab == Tab::Tasks;
        div()
            .min_w_0()
            .min_h_0()
            .flex_1()
            .h_full()
            .flex()
            .flex_col()
            .child(self.page_header(cx))
            .child(self.tab_strip(cx))
            .when(tasks, |d| {
                d.child(self.day_strip(cx)).children(self.notices(cx))
            })
            .child(div().min_h_0().flex_1().child(match (tasks, self.layout) {
                (true, Layout::List) => self.list_view(cx),
                (true, Layout::Board) => self.board_view(cx),
                (false, _) => self.sources_view(),
            }))
            .into_any_element()
    }

    /// The detail beside the list, with a drag handle on its left edge.
    fn side_panel(&self, cx: &mut Context<Self>) -> AnyElement {
        div()
            .id("today-panel")
            .debug_selector(|| "today-panel".into())
            .relative()
            .w(px(self.panel_width.clamp(PANEL_MIN, PANEL_MAX)))
            .h_full()
            .flex_shrink_0()
            .border_l_1()
            .border_color(content(0.07))
            .child(self.detail_view(cx))
            .child(
                div()
                    .id("today-panel-resize")
                    .role(Role::Splitter)
                    .aria_label("Resize details")
                    .absolute()
                    .left(px(-3.))
                    .top_0()
                    .bottom_0()
                    .w(px(6.))
                    .cursor(CursorStyle::ResizeLeftRight)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, event: &gpui::MouseDownEvent, _, cx| {
                            this.resize_start =
                                Some((f32::from(event.position.x), this.panel_width));
                            cx.stop_propagation();
                        }),
                    ),
            )
            .into_any_element()
    }

    /// A narrow window shows the detail alone, with a way back to the list.
    fn detail_page(&self, cx: &mut Context<Self>) -> AnyElement {
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(
                div().px(px(12.)).pt(px(10.)).flex().child(
                    icon_action("today-back", "Back", "chevron-left")
                        .debug_selector(|| "today-back".into())
                        .on_click(cx.listener(|this, _, _, cx| this.select(None, cx))),
                ),
            )
            .child(div().min_h_0().flex_1().child(self.detail_view(cx)))
            .into_any_element()
    }

    fn save_panel_width(&self, cx: &mut Context<Self>) {
        let store = self.store.clone();
        let width = self.panel_width;
        cx.background_executor()
            .spawn(async move {
                let _ = locked(&store)
                    .and_then(|db| db.save_setting(PANEL_WIDTH_KEY, &width.to_string()));
            })
            .detach();
    }
}

impl Render for Today {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let dialog = self.statuses_dialog_view(window, cx);
        let tags_dialog = self.tags_dialog_view(window, cx);
        let narrow = self.narrow();
        let tasks = self.tab == Tab::Tasks;
        let body = if narrow && tasks && self.selected.is_some() {
            self.detail_page(cx)
        } else if tasks && !narrow {
            div()
                .size_full()
                .flex()
                .child(self.main_column(cx))
                .child(self.side_panel(cx))
                .into_any_element()
        } else {
            self.main_column(cx)
        };
        let width = self.page_width.clone();
        let this = cx.entity();
        div()
            .id("today-view")
            .role(Role::Group)
            .aria_label("Today")
            .track_focus(&self.focus)
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, window, cx| {
                if event.keystroke.key == "escape" {
                    // Innermost first: the dialog, then a field (Escape leaves it and
                    // keeps the page), then the narrow detail, then the page itself.
                    if this.asking_to_move() {
                        this.cancel_move(cx);
                    } else if this.statuses_dialog.is_some() {
                        this.finish_statuses(window, cx);
                    } else if this.tags_dialog.is_some() {
                        this.finish_tags(window, cx);
                    } else if !this.focus.is_focused(window) {
                        window.focus(&this.focus, cx);
                        cx.notify();
                    } else if this.narrow() && this.selected.is_some() {
                        this.select(None, cx);
                    } else {
                        cx.emit(Event::Close);
                    }
                    cx.stop_propagation();
                }
            }))
            .on_mouse_move(
                cx.listener(|this, event: &gpui::MouseMoveEvent, window, cx| {
                    if let Some((start, width)) = this.resize_start {
                        let max = (f32::from(window.viewport_size().width) * 0.5)
                            .clamp(PANEL_MIN, PANEL_MAX);
                        // The panel is on the right, so dragging left widens it.
                        this.panel_width =
                            (width + start - f32::from(event.position.x)).clamp(PANEL_MIN, max);
                        cx.notify();
                    }
                }),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    if this.resize_start.take().is_some() {
                        this.save_panel_width(cx);
                        cx.notify();
                    }
                }),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    if this.resize_start.take().is_some() {
                        this.save_panel_width(cx);
                        cx.notify();
                    }
                }),
            )
            .child(
                canvas(
                    move |bounds, window, cx| {
                        let now = f32::from(bounds.size.width);
                        let before = width.replace(now);
                        if (before < NARROW_WIDTH) != (now < NARROW_WIDTH) {
                            window.defer(cx, move |_, cx| this.update(cx, |_, cx| cx.notify()));
                        }
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .size_full(),
            )
            .child(div().relative().min_h_0().flex_1().child(body))
            .when_some(
                self.action_error.clone().or_else(|| self.error.clone()),
                |d, text| d.child(message(text).text_color(danger_color())),
            )
            .children(dialog)
            .children(tags_dialog)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{TestAppContext, VisualTestContext};
    use monocode_core::today::{MonoCaller, SyncItem, SyncRequest};
    use std::sync::{Arc, Mutex};

    fn database() -> Database {
        Arc::new(Mutex::new(
            Store::open(std::path::Path::new(":memory:")).unwrap(),
        ))
    }
    fn add(store: &Database, title: &str) -> Task {
        locked(store)
            .unwrap()
            .today_add(
                &NewTask {
                    title: title.into(),
                    ..Default::default()
                },
                &Actor::User,
                now_ms(),
            )
            .unwrap()
    }
    fn page<'a>(
        cx: &'a mut TestAppContext,
        store: &Database,
    ) -> (Entity<Today>, &'a mut VisualTestContext) {
        let store = store.clone();
        let (today, visual) = cx.add_window_view(|_, cx| Today::new_quiet(store, cx));
        today.update(visual, |today, cx| today.refresh(cx));
        visual.run_until_parked();
        (today, visual)
    }
    fn titles(today: &Entity<Today>, cx: &VisualTestContext) -> Vec<String> {
        today.read_with(cx, |today, _| {
            today
                .view
                .iter()
                .flat_map(|view| view.rows.iter().map(|row| row.task.title.clone()))
                .collect()
        })
    }

    #[gpui::test]
    fn selection_survives_a_refresh_and_a_vanished_task_clears_it(cx: &mut TestAppContext) {
        let store = database();
        let kept = add(&store, "Kept");
        let (today, visual) = page(cx, &store);
        today.update(visual, |today, cx| today.select(Some(kept.id.clone()), cx));
        visual.run_until_parked();
        add(&store, "Another");
        today.update(visual, |today, cx| today.refresh(cx));
        visual.run_until_parked();
        today.read_with(visual, |today, _| {
            assert_eq!(today.selected.as_deref(), Some(kept.id.as_str()));
            assert_eq!(today.visible_rows().len(), 2);
            assert_eq!(today.selected_row().unwrap().task.title, "Kept");
        });
        locked(&store)
            .unwrap()
            .today_dismiss(&kept.id, &Actor::User, now_ms())
            .unwrap();
        today.update(visual, |today, cx| today.refresh(cx));
        visual.run_until_parked();
        today.read_with(visual, |today, _| {
            assert_eq!(today.selected, None);
            assert!(today.activity.is_empty());
        });
    }

    #[gpui::test]
    fn a_refresh_shows_a_task_an_agent_synced_and_counts_it(cx: &mut TestAppContext) {
        let store = database();
        let (today, visual) = page(cx, &store);
        assert!(titles(&today, visual).is_empty());
        let caller = MonoCaller {
            id: "mono-1".into(),
            habit: Some("habit-1".into()),
            ..Default::default()
        };
        let request = SyncRequest {
            source: "github".into(),
            name: Some("GitHub reviews".into()),
            provider: Some("GitHub".into()),
            items: Some(vec![SyncItem {
                key: "github:1".into(),
                title: "Review PR 1".into(),
                ..Default::default()
            }]),
            ..Default::default()
        };
        locked(&store)
            .unwrap()
            .today_sync(&request, &caller, now_ms())
            .unwrap();
        today.update(visual, |today, cx| today.refresh(cx));
        visual.run_until_parked();
        assert_eq!(titles(&today, visual), ["Review PR 1"]);
        today.read_with(visual, |today, _| {
            assert_eq!(today.open_count(), 1);
            assert_eq!(today.sources.len(), 1);
            // A run set the habit, but the roster does not know it yet.
            assert!(!today.can_sync());
        });
    }

    #[gpui::test]
    fn a_task_typed_in_the_field_is_added_to_the_day_on_screen_by_the_user(
        cx: &mut TestAppContext,
    ) {
        let store = database();
        let (today, visual) = page(cx, &store);
        today.update(visual, |today, cx| {
            today.new_task.update(cx, |input, cx| {
                input.set_text("Write the report".into(), cx)
            });
            today.add_task(cx);
        });
        visual.run_until_parked();
        today.read_with(visual, |today, _| {
            let row = &today.visible_rows()[0];
            assert_eq!(row.task.title, "Write the report");
            assert_eq!(row.task.source_id, None);
        });
    }

    #[gpui::test]
    fn the_round_check_completes_a_task_and_takes_it_back(cx: &mut TestAppContext) {
        let store = database();
        let task = add(&store, "Check me");
        let (today, visual) = page(cx, &store);
        let status = |today: &Entity<Today>, visual: &VisualTestContext| {
            today.read_with(visual, |today, _| today.visible_rows()[0].task.status)
        };
        today.update(visual, |today, cx| today.toggle_done(task.id.clone(), cx));
        visual.run_until_parked();
        assert_eq!(status(&today, visual), monocode_core::today::Status::Done);
        today.update(visual, |today, cx| today.toggle_done(task.id.clone(), cx));
        visual.run_until_parked();
        assert_eq!(status(&today, visual), monocode_core::today::Status::Open);
    }

    #[gpui::test]
    fn a_narrow_page_shows_the_detail_alone_with_a_back_action(cx: &mut TestAppContext) {
        let store = database();
        let task = add(&store, "Narrow");
        let (today, visual) = page(cx, &store);
        today.update(visual, |today, cx| today.select(Some(task.id.clone()), cx));
        visual.run_until_parked();
        visual.simulate_resize(gpui::size(px(1200.), px(800.)));
        visual.run_until_parked();
        visual.refresh().unwrap();
        assert!(visual.debug_bounds("today-panel").is_some());
        assert!(visual.debug_bounds("today-back").is_none());

        visual.simulate_resize(gpui::size(px(600.), px(800.)));
        visual.run_until_parked();
        visual.refresh().unwrap();
        assert!(visual.debug_bounds("today-panel").is_none());
        let back = visual.debug_bounds("today-back").unwrap();
        visual.simulate_click(back.center(), Default::default());
        visual.run_until_parked();
        visual.refresh().unwrap();
        today.read_with(visual, |today, _| assert_eq!(today.selected, None));
        assert!(visual.debug_bounds("today-back").is_none());
        assert!(visual.debug_bounds("today-tabs").is_some());
    }

    #[gpui::test]
    fn a_past_day_is_read_only_and_adds_nothing(cx: &mut TestAppContext) {
        let store = database();
        let (today, visual) = page(cx, &store);
        today.update(visual, |today, cx| {
            today.step_day(-1, cx);
            today
                .new_task
                .update(cx, |input, cx| input.set_text("Backfill".into(), cx));
        });
        visual.run_until_parked();
        today.update(visual, |today, cx| today.add_task(cx));
        visual.run_until_parked();
        visual.refresh().unwrap();
        assert!(visual.debug_bounds("today-read-only").is_some());
        assert!(visual.debug_bounds("today-summary").is_none());
        assert!(titles(&today, visual).is_empty());
        today.update(visual, |today, cx| today.show_day(None, cx));
        visual.run_until_parked();
        assert!(titles(&today, visual).is_empty());
    }

    #[gpui::test]
    fn the_rollover_setting_is_saved_and_reloaded(cx: &mut TestAppContext) {
        let store = database();
        let (today, visual) = page(cx, &store);
        today.read_with(visual, |today, _| {
            assert_eq!(today.rollover_minutes(), Some(240))
        });
        today.update(visual, |today, cx| today.set_rollover(360, cx));
        visual.run_until_parked();
        assert_eq!(locked(&store).unwrap().today_rollover_minutes(), Ok(360));
        today.read_with(visual, |today, _| {
            assert_eq!(today.rollover_minutes(), Some(360))
        });
    }

    fn habit_roster(busy: bool) -> Vec<MonoInfo> {
        vec![MonoInfo {
            id: "mono-1".into(),
            name: "Pikachu".into(),
            mascot: String::new(),
            color: 0,
            projects: Vec::new(),
            habits_known: true,
            habits: vec![HabitInfo {
                id: "habit-1".into(),
                name: "Fetch".into(),
                schedule: "Daily".into(),
                enabled: true,
                busy,
            }],
        }]
    }
    fn sync_source(store: &Database, source: &str, mono: &str, habit: &str) {
        let caller = MonoCaller {
            id: mono.into(),
            habit: Some(habit.into()),
            ..Default::default()
        };
        let request = SyncRequest {
            source: source.into(),
            items: Some(Vec::new()),
            ..Default::default()
        };
        locked(store)
            .unwrap()
            .today_sync(&request, &caller, now_ms())
            .unwrap();
    }

    #[gpui::test]
    fn a_failed_action_keeps_its_error_through_the_reload(cx: &mut TestAppContext) {
        let store = database();
        let (today, visual) = page(cx, &store);
        today.update(visual, |today, cx| {
            today.act(|_, _| Err("The store is read-only".into()), cx)
        });
        visual.run_until_parked();
        today.read_with(visual, |today, _| {
            assert_eq!(
                today.action_error.as_deref(),
                Some("The store is read-only")
            );
            assert!(today.view.is_some(), "the reload still ran");
        });
        today.update(visual, |today, cx| today.act(|_, _| Ok(()), cx));
        visual.run_until_parked();
        today.read_with(visual, |today, _| assert_eq!(today.action_error, None));
    }

    #[gpui::test]
    fn a_note_draft_does_not_follow_the_selection(cx: &mut TestAppContext) {
        let store = database();
        let (a, b) = (add(&store, "A"), add(&store, "B"));
        let (today, visual) = page(cx, &store);
        today.update(visual, |today, cx| {
            today.select(Some(a.id.clone()), cx);
            today
                .note
                .update(cx, |input, cx| input.set_text("for A".into(), cx));
            today.select(Some(b.id.clone()), cx);
        });
        today.update(visual, |today, cx| {
            assert_eq!(today.note.update(cx, |input, cx| input.take(cx)), None);
        });
    }

    #[gpui::test]
    fn two_quick_day_steps_move_two_days(cx: &mut TestAppContext) {
        let store = database();
        let (today, visual) = page(cx, &store);
        let current = today.read_with(visual, |today, _| today.current_day().unwrap().to_string());
        today.update(visual, |today, cx| {
            today.step_day(-1, cx);
            today.step_day(-1, cx);
        });
        visual.run_until_parked();
        let shown = today.read_with(visual, |today, _| today.viewed_day().unwrap().to_string());
        assert_eq!(Some(shown), add_days(&current, -2));
    }

    #[gpui::test]
    fn sync_needs_a_habit_the_roster_knows_and_skips_only_the_busy_one(cx: &mut TestAppContext) {
        let store = database();
        sync_source(&store, "a", "mono-1", "habit-1");
        sync_source(&store, "b", "mono-2", "habit-2");
        let (today, visual) = page(cx, &store);
        today.read_with(visual, |today, _| assert!(!today.can_sync()));
        let mut roster = habit_roster(true);
        roster.push(MonoInfo {
            id: "mono-2".into(),
            habits: vec![HabitInfo {
                id: "habit-2".into(),
                name: "Fetch".into(),
                schedule: "Daily".into(),
                enabled: true,
                busy: false,
            }],
            ..roster[0].clone()
        });
        today.update(visual, |today, cx| today.set_roster(roster, cx));
        today.read_with(visual, |today, _| {
            assert!(today.syncing());
            assert!(
                today.can_sync(),
                "one busy source does not block the others"
            );
            assert_eq!(
                today.sync_targets(None),
                [("mono-2".to_string(), "habit-2".to_string())]
            );
        });
        today.update(visual, |today, cx| today.set_roster(Vec::new(), cx));
        today.read_with(visual, |today, _| assert!(!today.can_sync()));
    }

    #[gpui::test]
    fn a_source_whose_mono_or_habit_is_gone_is_retired_only_once_the_roster_loaded(
        cx: &mut TestAppContext,
    ) {
        let store = database();
        sync_source(&store, "a", "mono-1", "habit-1");
        sync_source(&store, "b", "mono-2", "habit-2");
        let (today, visual) = page(cx, &store);
        today.read_with(visual, |today, _| {
            assert!(today.sources.iter().all(|s| !today.retired(s)));
        });
        let mut roster = habit_roster(false);
        roster[0].habits.clear();
        today.update(visual, |today, cx| today.set_roster(roster, cx));
        today.read_with(visual, |today, _| {
            let retired =
                |id: &str| today.retired(today.sources.iter().find(|s| s.id == id).unwrap());
            assert!(retired("a"), "habit removed");
            assert!(retired("b"), "Mono removed");
            // Retry on a failed band must agree: nothing can run.
            assert!(today.sync_targets(Some("a")).is_empty());
        });
        today.update(visual, |today, cx| {
            today.set_roster(habit_roster(false), cx)
        });
        today.read_with(visual, |today, _| {
            let a = today.sources.iter().find(|s| s.id == "a").unwrap();
            assert!(!today.retired(a));
            assert_eq!(today.sync_targets(Some("a")).len(), 1);
        });
    }

    #[gpui::test]
    fn unknown_habits_never_retire_a_source_but_a_removed_mono_does(cx: &mut TestAppContext) {
        let store = database();
        sync_source(&store, "a", "mono-1", "habit-1");
        let (today, visual) = page(cx, &store);
        let mut roster = habit_roster(false);
        roster[0].habits.clear();
        roster[0].habits_known = false;
        today.update(visual, |today, cx| today.set_roster(roster, cx));
        today.read_with(visual, |today, _| {
            assert!(
                !today.retired(&today.sources[0]),
                "habits unknown or unreadable"
            );
        });
        today.update(visual, |today, cx| today.set_roster(Vec::new(), cx));
        today.read_with(visual, |today, _| {
            assert!(today.retired(&today.sources[0]), "the last Mono is gone");
        });
    }

    #[gpui::test]
    fn sync_now_skips_a_paused_habit_but_retry_runs_it(cx: &mut TestAppContext) {
        let store = database();
        sync_source(&store, "a", "mono-1", "habit-1");
        let (today, visual) = page(cx, &store);
        let mut roster = habit_roster(false);
        roster[0].habits[0].enabled = false;
        today.update(visual, |today, cx| today.set_roster(roster, cx));
        today.read_with(visual, |today, _| {
            assert!(today.sync_targets(None).is_empty());
            assert_eq!(today.sync_targets(Some("a")).len(), 1);
        });
    }

    #[gpui::test]
    fn the_header_names_the_day_of_an_old_sync(cx: &mut TestAppContext) {
        let store = database();
        sync_source(&store, "a", "mono-1", "habit-1");
        let (today, visual) = page(cx, &store);
        today.update(visual, |today, cx| {
            assert!(today.synced_caption().unwrap().starts_with("Synced "));
            today.sources[0].last_sync_day = Some("2020-01-02".into());
            assert!(
                today
                    .synced_caption()
                    .unwrap()
                    .starts_with("Last synced Jan 2, ")
            );
            cx.notify();
        });
    }

    fn click(visual: &mut VisualTestContext, selector: &'static str) {
        visual.refresh().unwrap();
        let bounds = visual.debug_bounds(selector).expect(selector);
        visual.simulate_click(bounds.center(), Default::default());
        visual.run_until_parked();
        visual.refresh().unwrap();
    }

    #[gpui::test]
    fn the_header_switch_changes_the_layout_and_the_body_follows(cx: &mut TestAppContext) {
        let store = database();
        add(&store, "Switch me");
        let (today, visual) = page(cx, &store);
        today.read_with(visual, |today, _| assert_eq!(today.layout, Layout::List));
        assert!(visual.debug_bounds("today-new-task").is_some());
        assert!(visual.debug_bounds("today-board-scroll").is_none());
        click(visual, "today-layout-board");
        today.read_with(visual, |today, _| assert_eq!(today.layout, Layout::Board));
        assert!(visual.debug_bounds("today-board-scroll").is_some());
        assert!(visual.debug_bounds("today-new-task").is_none());
        assert!(
            visual.debug_bounds("today-summary").is_some(),
            "the notices stay"
        );
        click(visual, "today-layout-list");
        today.read_with(visual, |today, _| assert_eq!(today.layout, Layout::List));
        assert!(visual.debug_bounds("today-new-task").is_some());
        assert!(visual.debug_bounds("today-sync").is_some());
    }

    #[gpui::test]
    fn the_layout_is_remembered_by_a_new_page_on_the_same_store(cx: &mut TestAppContext) {
        let store = database();
        {
            let (today, visual) = page(cx, &store);
            today.update(visual, |today, cx| today.set_layout(Layout::Board, cx));
            visual.run_until_parked();
        }
        assert_eq!(
            locked(&store)
                .unwrap()
                .setting(LAYOUT_KEY)
                .unwrap()
                .as_deref(),
            Some("board")
        );
        let (today, visual) = page(cx, &store);
        today.read_with(visual, |today, _| assert_eq!(today.layout, Layout::Board));
    }

    #[gpui::test]
    fn the_filters_are_remembered_by_a_new_page_on_the_same_store(cx: &mut TestAppContext) {
        let store = database();
        let tag = locked(&store)
            .unwrap()
            .today_add_tag("Urgent", "#4da3f5")
            .unwrap();
        {
            let (today, visual) = page(cx, &store);
            let tag_id = tag.id.clone();
            today.update(visual, |today, cx| {
                let mut next = today.filters.clone();
                next.statuses.insert("done".into());
                next.statuses.insert("ghost".into());
                next.tags.insert(tag_id);
                next.no_tag = true;
                next.match_all = true;
                next.mono = Some("m".into());
                today.set_filters(next, cx);
            });
            visual.run_until_parked();
        }
        let saved = locked(&store)
            .unwrap()
            .setting(FILTERS_KEY)
            .unwrap()
            .unwrap();
        assert!(saved.contains("\"noTag\":true"), "{saved}");
        let (today, visual) = page(cx, &store);
        today.read_with(visual, |today, _| {
            // The unknown status is dropped; the Mono filter comes back (no roster yet to judge it).
            assert_eq!(today.filters.statuses.len(), 1);
            assert!(today.filters.statuses.contains("done"));
            assert!(today.filters.tags.contains(&tag.id));
            assert!(today.filters.no_tag && today.filters.match_all);
            assert_eq!(today.filters.mono.as_deref(), Some("m"));
        });
    }

    #[gpui::test]
    fn a_stored_mono_filter_is_dropped_only_once_the_roster_is_known(cx: &mut TestAppContext) {
        let store = database();
        locked(&store)
            .unwrap()
            .save_setting(FILTERS_KEY, r#"{"mono":"m1","project":"/p"}"#)
            .unwrap();
        let (today, visual) = page(cx, &store);
        today.read_with(visual, |today, _| {
            assert_eq!(today.filters.mono.as_deref(), Some("m1"));
            assert_eq!(today.filters.project, Some(Some("/p".to_string())));
        });
        today.update(visual, |today, cx| today.set_roster(Vec::new(), cx));
        today.read_with(visual, |today, _| {
            assert_eq!(today.filters.mono, None);
            assert_eq!(today.filters.project, Some(Some("/p".to_string())));
        });
    }

    #[gpui::test]
    fn quick_filter_changes_store_the_newest_value_last(cx: &mut TestAppContext) {
        let store = database();
        let (today, visual) = page(cx, &store);
        today.update(visual, |today, cx| {
            for (ix, id) in ["todo", "in_progress", "done", "todo", "in_progress"]
                .into_iter()
                .enumerate()
            {
                let mut next = today.filters.clone();
                if ix == 3 {
                    next.statuses.clear();
                }
                next.statuses.insert(id.into());
                today.set_filters(next, cx);
            }
        });
        visual.run_until_parked();
        let expected = today.read_with(visual, |today, _| filter::Saved::of(&today.filters));
        let stored = locked(&store)
            .unwrap()
            .setting(FILTERS_KEY)
            .unwrap()
            .unwrap();
        assert_eq!(filter::Saved::parse(&stored), Some(expected));
    }

    #[gpui::test]
    fn match_all_alone_is_not_a_filter_on_screen(cx: &mut TestAppContext) {
        let store = database();
        let (today, visual) = page(cx, &store);
        today.update(visual, |today, cx| {
            let mut next = today.filters.clone();
            next.match_all = true;
            today.set_filters(next, cx);
        });
        visual.run_until_parked();
        visual.refresh().unwrap();
        assert!(visual.debug_bounds("today-filters-clear").is_none());
        assert!(visual.debug_bounds("today-filter-count").is_none());
    }

    #[gpui::test]
    fn a_task_added_under_a_filter_that_hides_it_says_so(cx: &mut TestAppContext) {
        let store = database();
        let (today, visual) = page(cx, &store);
        today.update(visual, |today, cx| {
            let mut next = today.filters.clone();
            next.statuses.insert("done".into());
            today.set_filters(next, cx);
            today
                .new_task
                .update(cx, |input, cx| input.set_text("Hidden one".into(), cx));
            today.add_task(cx);
        });
        visual.run_until_parked();
        visual.refresh().unwrap();
        assert!(visual.debug_bounds("today-added-hidden").is_some());
        click(visual, "today-added-clear");
        today.read_with(visual, |today, _| {
            assert_eq!(today.filters, Filters::default());
            assert!(!today.added_hidden);
        });
        assert!(visual.debug_bounds("today-added-hidden").is_none());
        // A task that passes the filters says nothing.
        today.update(visual, |today, cx| {
            today
                .new_task
                .update(cx, |input, cx| input.set_text("Visible".into(), cx));
            today.add_task(cx);
        });
        visual.run_until_parked();
        visual.refresh().unwrap();
        assert!(visual.debug_bounds("today-added-hidden").is_none());
    }

    #[gpui::test]
    fn a_failed_add_does_not_say_it_was_hidden(cx: &mut TestAppContext) {
        let store = database();
        let (today, visual) = page(cx, &store);
        today.update(visual, |today, cx| {
            let mut next = today.filters.clone();
            next.statuses.insert("done".into());
            today.set_filters(next, cx);
            today.act_add("todo", |_, _| Err("nope".into()), cx);
        });
        visual.run_until_parked();
        today.read_with(visual, |today, _| {
            assert!(!today.added_hidden);
            assert_eq!(today.action_error.as_deref(), Some("nope"));
        });
    }

    #[gpui::test]
    fn pruning_the_filters_clears_the_hidden_flag(cx: &mut TestAppContext) {
        let store = database();
        let (today, visual) = page(cx, &store);
        today.update(visual, |today, cx| {
            let mut next = today.filters.clone();
            next.mono = Some("gone".into());
            today.set_filters(next, cx);
            today.added_hidden = true;
            // The roster does not hold that Mono: the filter goes, and the flag with it.
            today.set_roster(Vec::new(), cx);
            assert_eq!(today.filters.mono, None);
            assert!(!today.added_hidden);
        });
    }

    #[gpui::test]
    fn a_load_rebuilds_an_open_filter_menu(cx: &mut TestAppContext) {
        let store = database();
        let (today, visual) = page(cx, &store);
        click(visual, "today-filter-tag");
        let before = today.read_with(visual, |today, _| today.menus.pick_count());
        assert!(before > 0);
        locked(&store)
            .unwrap()
            .today_add_tag("Fresh", "#4da3f5")
            .unwrap();
        today.update(visual, |today, cx| today.refresh(cx));
        visual.run_until_parked();
        today.read_with(visual, |today, _| {
            assert!(today.menus.is_open());
            assert_eq!(today.menus.pick_count(), before + 1);
        });
    }

    #[gpui::test]
    fn filters_picked_before_the_first_load_lands_are_kept(cx: &mut TestAppContext) {
        let store = database();
        locked(&store)
            .unwrap()
            .save_setting(FILTERS_KEY, r#"{"statuses":["done"]}"#)
            .unwrap();
        let (today, visual) = cx.add_window_view(|_, cx| Today::new_quiet(store.clone(), cx));
        today.update(visual, |today, cx| {
            today.refresh(cx);
            let mut next = today.filters.clone();
            next.statuses.insert("todo".into());
            today.set_filters(next, cx);
        });
        visual.run_until_parked();
        today.read_with(visual, |today, _| {
            assert_eq!(today.filters.statuses.iter().collect::<Vec<_>>(), ["todo"]);
        });
    }

    #[gpui::test]
    fn a_stored_filter_for_a_deleted_status_is_dropped(cx: &mut TestAppContext) {
        let store = database();
        locked(&store)
            .unwrap()
            .save_setting(
                FILTERS_KEY,
                r#"{"statuses":["gone","todo"],"tags":["tag-x"]}"#,
            )
            .unwrap();
        let (today, visual) = page(cx, &store);
        today.read_with(visual, |today, _| {
            assert_eq!(today.filters.statuses.iter().collect::<Vec<_>>(), ["todo"]);
            assert!(today.filters.tags.is_empty());
        });
    }

    #[gpui::test]
    fn the_statuses_action_opens_the_dialog(cx: &mut TestAppContext) {
        let store = database();
        let (today, visual) = page(cx, &store);
        click(visual, "today-statuses");
        today.read_with(visual, |today, _| assert!(today.statuses_dialog.is_some()));
    }

    #[gpui::test]
    fn the_layout_switch_is_only_on_the_tasks_tab(cx: &mut TestAppContext) {
        let store = database();
        let (today, visual) = page(cx, &store);
        today.update(visual, |today, cx| today.show_tab(Tab::Sources, cx));
        visual.run_until_parked();
        visual.refresh().unwrap();
        assert!(visual.debug_bounds("today-layout-board").is_none());
        assert!(visual.debug_bounds("today-statuses").is_none());
        assert!(visual.debug_bounds("today-sync").is_some());
    }

    fn needs_check_page(
        cx: &mut TestAppContext,
    ) -> (Entity<Today>, &mut VisualTestContext, Database, String) {
        let store = database();
        let caller = MonoCaller {
            id: "m1".into(),
            ..Default::default()
        };
        let request = SyncRequest {
            source: "cu".into(),
            name: Some("ClickUp".into()),
            items: Some(vec![SyncItem {
                key: "a".into(),
                title: "Review it".into(),
                ..Default::default()
            }]),
            ..Default::default()
        };
        let now = now_ms();
        {
            let db = locked(&store).unwrap();
            db.today_sync(&request, &caller, now).unwrap();
            db.today_complete("a", &Actor::User, now).unwrap();
            db.today_sync(&request, &caller, now).unwrap();
            let task = db.today_task("a").unwrap().unwrap();
            assert_eq!(task.status, monocode_core::today::Status::NeedsCheck);
        }
        let id = locked(&store).unwrap().today_task("a").unwrap().unwrap().id;
        let (today, visual) = page(cx, &store);
        (today, visual, store, id)
    }

    #[gpui::test]
    fn the_ring_of_a_done_but_source_open_check_takes_the_task_back_to_open(
        cx: &mut TestAppContext,
    ) {
        let (today, visual, store, id) = needs_check_page(cx);
        today.read_with(visual, |today, _| {
            assert_eq!(today.visible_rows()[0].status.stage, Stage::Finished);
        });
        click(visual, "today-check-a");
        let task = locked(&store).unwrap().today_task(&id).unwrap().unwrap();
        assert_eq!(task.status, monocode_core::today::Status::Open);
        assert_eq!(task.check_reason, None);
        assert!(!task.kept_done);
        today.read_with(visual, |today, _| {
            assert_eq!(today.visible_rows()[0].status.id, "todo");
        });
    }

    #[gpui::test]
    fn escape_in_the_dialog_commits_closes_and_does_not_close_today(cx: &mut TestAppContext) {
        let store = database();
        let (today, visual) = page(cx, &store);
        let closed = Rc::new(Cell::new(false));
        let seen = closed.clone();
        let _sub = visual.update(|_, cx| {
            cx.subscribe(&today, move |_, event: &Event, _| {
                if matches!(event, Event::Close) {
                    seen.set(true);
                }
            })
        });
        today.update(visual, |today, cx| today.open_statuses(cx));
        visual.run_until_parked();
        visual.refresh().unwrap();
        visual.simulate_keystrokes("escape");
        visual.run_until_parked();
        today.read_with(visual, |today, _| assert!(today.statuses_dialog.is_none()));
        assert!(!closed.get(), "Escape in the dialog must not close Today");
    }

    #[gpui::test]
    fn a_layout_picked_before_the_first_load_lands_is_kept(cx: &mut TestAppContext) {
        let store = database();
        locked(&store)
            .unwrap()
            .save_setting(LAYOUT_KEY, "list")
            .unwrap();
        let (today, visual) = cx.add_window_view(|_, cx| Today::new_quiet(store.clone(), cx));
        today.update(visual, |today, cx| {
            today.refresh(cx);
            today.set_layout(Layout::Board, cx);
        });
        visual.run_until_parked();
        today.read_with(visual, |today, _| assert_eq!(today.layout, Layout::Board));
    }

    #[gpui::test]
    fn changing_layout_or_tab_closes_the_board_field_and_menus(cx: &mut TestAppContext) {
        let store = database();
        let (today, visual) = page(cx, &store);
        today.update(visual, |today, cx| {
            today.board.menu = Some("todo".into());
            today.set_layout(Layout::Board, cx);
            assert!(today.board.menu.is_none());
            today.board.menu = Some("todo".into());
            today.show_tab(Tab::Sources, cx);
            assert!(today.board.menu.is_none());
            assert!(today.board.adding.is_none());
            assert!(!today.menus.is_open());
        });
    }

    #[gpui::test]
    fn start_session_with_no_project_known_says_so_instead_of_opening_an_empty_menu(
        cx: &mut TestAppContext,
    ) {
        let store = database();
        let task = add(&store, "Lonely");
        let (today, visual) = page(cx, &store);
        visual.update(|window, cx| {
            today.update(cx, |today, cx| {
                today.start_session(task.clone(), window, cx)
            })
        });
        today.read_with(visual, |today, _| {
            assert!(today.action_error.as_deref().unwrap().contains("project"));
            assert!(!today.menus.is_open());
        });
    }
}
