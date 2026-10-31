//! The rules of the Ticket tab: what `useTicketBoard.ts` does, as plain state
//! on `TicketView`. Every outside call goes through the `TicketBackend` on the
//! background executor; every answer comes back through a generation check, so
//! a late answer for an abandoned request (or for an earlier mount) cannot
//! change what the tab shows.
//!
//! A "mount" is what the React view calls mounting: opening the Ticket tab
//! resets everything but the windows already fetched in this app session.
use super::{
    backend::{Named, Patch, TicketBackend},
    export::ExportWait,
};
use crate::contributors::{
    backend::ColorOverrides,
    chart::{self, ChartKind, ChartPrefs},
    data::Clock,
    jobs::{JobsEvent, SyncJobs},
};
use chrono::{DateTime, Local};
use gpui::{Context, Entity, EventEmitter, Subscription, Task};
use monocode_core::contributors::{
    model::period::{Period, PeriodKind, PeriodPick, next_period, pick_of, previous_period},
    ticket_jobs::{TicketEstimate, TicketJobEvent},
    tickets::{
        board::{
            Board, StatusInfo, active_tickets, build_board, excluded_members_of, period_tickets,
            pick_for_run, statuses_of, to_assess_count,
        },
        board_state::{
            fetch_busy, fetch_key, is_end, job_error_after, period_of, scope_fetch_id,
            should_fetch, store_may_have_changed, window_of,
        },
        prefs::TicketBoardSettings,
        report::{
            MatrixRow, MemberTrend, TileStats, TrendUnit, level_matrix, member_trend,
            no_comparison_note, previous_span, tile_stats,
        },
        tickets::{
            AiConfigStatus, DEFAULT_TICKET_POINTS, ExcludedMember, TicketRecord, WorkspaceScope,
        },
    },
};
use std::{
    collections::{HashMap, HashSet},
    rc::Rc,
    sync::Arc,
    time::Duration,
};

/// How long a changed set of tickets to score waits before it is estimated.
const ESTIMATE_DEBOUNCE: Duration = Duration::from_millis(300);
/// Job ends remembered for a command whose answer has not come yet.
const ENDED_LIMIT: usize = 50;

/// What the tab tells its host.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum TicketEvent {
    /// A ClickUp problem sends the user to Settings > Inbox.
    OpenSettings,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WorkspacesState {
    Loading,
    Ready,
    Error,
}

/// What the body of the tab says: a ClickUp problem, a wait, or the board.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum BodyState {
    Problem { message: String, retry: bool },
    Checking,
    LoadingWorkspaces,
    PickWorkspace,
    Board,
}

/// The stored tickets of one window, as `reload` read them.
pub(super) struct Loaded {
    pub key: String,
    pub tickets: Arc<Vec<TicketRecord>>,
}

/// How the period compares with the previous one.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Comparison {
    /// There is nothing to compare with: All time, or no scope.
    Off,
    /// The store is being read.
    Pending,
    /// The store does not hold the whole previous period; nothing is fetched
    /// for it, so the cards say why they do not show a change.
    Missing { note: String },
    Ready {
        /// What the change reads against: "Sep 2026", "the same days of Sep 2026".
        against: String,
        stats: TileStats,
        board: Board,
    },
}

/// Everything the board shows, derived from the loaded tickets, the scope, the
/// points, the coverage and the period. Rebuilt only when one of them changes.
pub(super) struct Derived {
    /// Names this build: whatever is cached from a board is dropped when it moves.
    pub seq: u64,
    pub shown: Vec<TicketRecord>,
    pub board: Board,
    pub statuses: Vec<StatusInfo>,
    pub to_assess: usize,
    pub run_ids: Vec<String>,
    pub run_key: String,
    pub stats: TileStats,
    pub comparison: Comparison,
    pub trend: Arc<MemberTrend>,
    pub matrix: Vec<MatrixRow>,
}

impl Default for Derived {
    fn default() -> Self {
        let board = build_board(&[], &DEFAULT_TICKET_POINTS, &[]);
        Self {
            seq: 0,
            shown: Vec::new(),
            stats: tile_stats(&board),
            board,
            statuses: Vec::new(),
            to_assess: 0,
            run_ids: Vec::new(),
            run_key: String::new(),
            comparison: Comparison::Off,
            trend: Arc::new(MemberTrend {
                unit: TrendUnit::Week,
                buckets: Vec::new(),
                members: Vec::new(),
            }),
            matrix: Vec::new(),
        }
    }
}

/// The previous period as the store held it when it was read.
pub(super) struct PreviousRead {
    /// The workspace, spaces and window it is for.
    pub key: String,
    /// `None`: the store did not cover the window.
    pub tickets: Option<Arc<Vec<TicketRecord>>>,
}

/// The spaces the Scope panel lists, for the workspace it was asked about.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Spaces {
    Loading,
    Ready(Vec<Named>),
    Failed(String),
}

/// Shows a saved file in the file manager; the tests replace it.
pub(super) type Reveal = Rc<dyn Fn(&std::path::Path, &mut gpui::App)>;

/// Which effects ran for which inputs, so a settle runs one only when its
/// inputs changed (the `useEffect` dependency lists).
#[derive(Default)]
struct Effects {
    reload: Option<String>,
    fetch: Option<(String, String, String, bool, Option<bool>)>,
    run_key: Option<String>,
    export: Option<(String, String)>,
    spaces: Option<String>,
    /// The previous period's window, read from the store.
    previous: Option<Option<String>>,
    colors: Option<String>,
}

/// A backend call the tests keep back until they answer it.
#[cfg(test)]
pub(super) struct Held {
    pub label: &'static str,
    pub run: Option<HeldRun>,
}
#[cfg(test)]
type HeldRun = Box<dyn FnOnce(&mut TicketView, &mut Context<TicketView>)>;

pub(crate) struct TicketView {
    pub(super) backend: Option<Arc<dyn TicketBackend>>,
    pub(super) clock: Clock,
    _events: Subscription,
    pub(super) mounted: bool,
    started: bool,
    /// Bumped by every mount: an answer for an earlier one is dropped.
    pub(super) mount_gen: u64,
    pub(super) prefs_ready: bool,
    pub(super) now: DateTime<Local>,
    pub(super) pick: PeriodPick,
    pub(super) period: Period,
    pub(super) settings: TicketBoardSettings,
    pub(super) connected: Option<bool>,
    pub(super) workspaces: Vec<Named>,
    pub(super) workspaces_state: WorkspacesState,
    pub(super) workspaces_error: Option<String>,
    pub(super) connect_error: Option<String>,
    check_gen: u64,
    pub(super) loaded: Option<Loaded>,
    pub(super) job: Option<TicketJobEvent>,
    pub(super) ai: Option<AiConfigStatus>,
    pub(super) estimate: Option<TicketEstimate>,
    estimate_gen: u64,
    estimate_task: Option<Task<()>>,
    /// Kept apart so a successful reload does not hide a failed job.
    pub(super) load_error: Option<String>,
    pub(super) job_error: Option<String>,
    /// Fetch jobs queued or running, as the backend reports them.
    pub(super) pending_fetches: u32,
    /// Bumped on every change, so a status read that began earlier can tell it is stale.
    pending_seq: u64,
    /// Windows fetched from ClickUp in this app session; they outlive the mount.
    pub(super) fetched: HashSet<String>,
    /// Windows with a fetch of ours queued or running.
    pub(super) pending_keys: HashSet<String>,
    /// The window of each fetch job of ours.
    job_keys: HashMap<u64, String>,
    /// Fetch ends seen before the command's answer named them (job id, clean).
    ended_jobs: Vec<(u64, bool)>,
    /// A window whose fetch job failed: not retried on its own until the
    /// connection changes or another window was shown.
    failed: Option<(String, Option<bool>)>,
    /// Reload sequence: a newer read (or a new window) decides what is shown.
    request: u64,
    effects: Effects,
    dirty: bool,
    pub(super) derived: Derived,
    // Preferences are written one at a time, in order.
    writing: bool,
    queued: Option<Patch>,
    // The Scope panel.
    pub(super) scope_open: bool,
    pub(super) spaces: Option<Spaces>,
    spaces_gen: u64,
    // Export.
    pub(super) exporting: bool,
    pub(super) exported: bool,
    pub(super) export_error: Option<String>,
    pub(super) export_run: u64,
    /// Names the export that owns `exporting` and `export_wait`: bumped by
    /// every export and by every mount or unmount, so an older export's end
    /// cannot touch the state of a newer one.
    pub(super) export_gen: u64,
    pub(super) export_wait: Option<ExportWait>,
    pub(super) export_timeout: Duration,
    pub(super) reveal: Reveal,
    pub(super) ui: super::draw::UiState,
    // The overview, the member pages and what they are fed.
    pub(super) chart_prefs: Entity<ChartPrefs>,
    pub(super) overview: super::overview::OverviewUi,
    /// The previous period's tickets, from the store only.
    pub(super) previous: Option<PreviousRead>,
    /// The colors picked for the members of this workspace.
    pub(super) colors: ColorOverrides,
    colors_touched: bool,
    /// Bumped when `colors` change.
    pub(super) colors_gen: u64,
    /// The member whose page is open.
    pub(super) detail: Option<String>,
    pub(super) detail_ui: super::detail::DetailUi,
    /// Counts the builds of `derived`.
    derived_seq: u64,
    // Tests.
    #[cfg(test)]
    pub(super) manual: bool,
    #[cfg(test)]
    pub(super) held: Vec<Held>,
}
impl EventEmitter<TicketEvent> for TicketView {}

/// The text a previous read is told apart by.
fn key_text(key: &(String, String, String)) -> String {
    format!("{}\0{}\0{}", key.0, key.1, key.2)
}

fn error_text(error: impl ToString) -> String {
    error.to_string()
}

impl TicketView {
    pub(crate) fn new(jobs: Entity<SyncJobs>, clock: Clock, cx: &mut Context<Self>) -> Self {
        let events = cx.subscribe(&jobs, |this, _, event: &JobsEvent, cx| {
            if let JobsEvent::Ticket(event) = event {
                this.on_job_event(event.clone(), cx);
            }
        });
        let now = clock();
        let pick = PeriodPick {
            kind: PeriodKind::Month,
            id: None,
        };
        Self {
            backend: None,
            period: period_of(&pick, &now),
            pick,
            now,
            clock,
            _events: events,
            mounted: false,
            started: false,
            mount_gen: 0,
            prefs_ready: false,
            settings: monocode_core::contributors::tickets::prefs::read_settings(None),
            connected: None,
            workspaces: Vec::new(),
            workspaces_state: WorkspacesState::Loading,
            workspaces_error: None,
            connect_error: None,
            check_gen: 0,
            loaded: None,
            job: None,
            ai: None,
            estimate: None,
            estimate_gen: 0,
            estimate_task: None,
            load_error: None,
            job_error: None,
            pending_fetches: 0,
            pending_seq: 0,
            fetched: HashSet::new(),
            pending_keys: HashSet::new(),
            job_keys: HashMap::new(),
            ended_jobs: Vec::new(),
            failed: None,
            request: 0,
            effects: Effects::default(),
            dirty: true,
            derived: Derived::default(),
            writing: false,
            queued: None,
            scope_open: false,
            spaces: None,
            spaces_gen: 0,
            exporting: false,
            exported: false,
            export_error: None,
            export_run: 0,
            export_gen: 0,
            export_wait: None,
            export_timeout: super::export::PREVIOUS_FETCH_TIMEOUT,
            reveal: Rc::new(|path, cx| cx.reveal_path(path)),
            ui: super::draw::UiState::new(cx),
            chart_prefs: chart::shared_prefs(cx),
            overview: super::overview::OverviewUi::default(),
            previous: None,
            colors: ColorOverrides::new(),
            colors_touched: false,
            colors_gen: 0,
            detail: None,
            detail_ui: super::detail::DetailUi::default(),
            derived_seq: 0,
            #[cfg(test)]
            manual: false,
            #[cfg(test)]
            held: Vec::new(),
        }
    }

    // ---- plumbing -------------------------------------------------------

    /// Runs `work` on the background executor and hands its answer to
    /// `finish` on the UI thread, unless the tab was mounted again meanwhile.
    /// The tests can hold the answers back and deliver them in any order.
    pub(super) fn spawn_job<T: Send + 'static>(
        &mut self,
        label: &'static str,
        cx: &mut Context<Self>,
        work: impl FnOnce(Arc<dyn TicketBackend>) -> T + Send + 'static,
        finish: impl FnOnce(&mut Self, T, &mut Context<Self>) + 'static,
    ) {
        let Some(backend) = self.backend.clone() else {
            return;
        };
        let generation = self.mount_gen;
        let finish = move |this: &mut Self, answer: T, cx: &mut Context<Self>| {
            if this.mount_gen == generation {
                finish(this, answer, cx);
            }
        };
        #[cfg(test)]
        if self.manual {
            // The call is made now; only its answer is held back.
            let answer = work(backend);
            self.held.push(Held {
                label,
                run: Some(Box::new(move |this, cx| finish(this, answer, cx))),
            });
            return;
        }
        let _ = label;
        let task = cx.background_executor().spawn(async move { work(backend) });
        cx.spawn(async move |this, cx| {
            let answer = task.await;
            let _ = this.update(cx, |this, cx| finish(this, answer, cx));
        })
        .detach();
    }

    #[cfg(test)]
    pub(super) fn hold_backend_calls(&mut self) {
        self.manual = true;
    }

    #[cfg(test)]
    pub(super) fn held_labels(&self) -> Vec<&'static str> {
        self.held.iter().map(|h| h.label).collect()
    }

    /// Test hook: runs the `nth` held call with this label and applies its answer.
    #[cfg(test)]
    pub(super) fn release_nth(&mut self, label: &str, nth: usize, cx: &mut Context<Self>) {
        let at = self
            .held
            .iter()
            .enumerate()
            .filter(|(_, held)| held.label == label)
            .map(|(at, _)| at)
            .nth(nth)
            .unwrap_or_else(|| panic!("no held {label} call number {nth}"));
        let mut held = self.held.remove(at);
        if let Some(run) = held.run.take() {
            run(self, cx);
        }
    }

    #[cfg(test)]
    pub(super) fn release(&mut self, label: &str, cx: &mut Context<Self>) {
        self.release_nth(label, 0, cx);
    }

    #[cfg(test)]
    pub(super) fn release_all(&mut self, cx: &mut Context<Self>) {
        while !self.held.is_empty() {
            let mut held = self.held.remove(0);
            if let Some(run) = held.run.take() {
                run(self, cx);
            }
        }
    }

    // ---- mounting -------------------------------------------------------

    /// The Ticket tab was opened: everything the view keeps for one showing
    /// starts over, the windows fetched in this session stay fetched.
    pub(crate) fn mount(&mut self, cx: &mut Context<Self>) {
        self.mount_gen += 1;
        self.mounted = true;
        self.started = false;
        self.prefs_ready = false;
        self.now = (self.clock)();
        self.connected = None;
        self.workspaces = Vec::new();
        self.workspaces_state = WorkspacesState::Loading;
        self.workspaces_error = None;
        self.connect_error = None;
        self.loaded = None;
        self.job = None;
        self.ai = None;
        self.estimate = None;
        self.estimate_task = None;
        self.load_error = None;
        self.job_error = None;
        self.pending_fetches = 0;
        self.pending_seq = 0;
        self.pending_keys.clear();
        self.job_keys.clear();
        self.ended_jobs.clear();
        self.failed = None;
        self.effects = Effects::default();
        self.scope_open = false;
        self.spaces = None;
        self.reset_export();
        self.ui.reset();
        self.derived = Derived::default();
        self.previous = None;
        self.colors = ColorOverrides::new();
        self.colors_touched = false;
        self.colors_gen += 1;
        self.detail = None;
        self.drop_detail_ui(cx);
        self.dirty = true;
        self.try_start(cx);
        cx.notify();
    }

    /// The Ticket tab is not shown any more (the other tab, the page closed):
    /// React unmounts it. It listens to nothing, reloads nothing and queues no
    /// fetch until it is mounted again; answers still on their way are dropped.
    pub(crate) fn unmount(&mut self, cx: &mut Context<Self>) {
        if !self.mounted {
            return;
        }
        self.mount_gen += 1;
        self.mounted = false;
        self.started = false;
        self.prefs_ready = false;
        self.estimate_task = None;
        self.reset_export();
        // A palette left open would be open again at the next mount.
        self.dismiss_palettes(cx);
        self.release_detail_images(cx);
        cx.notify();
    }

    /// An export under way belongs to the showing that started it.
    fn reset_export(&mut self) {
        self.export_gen += 1;
        self.exporting = false;
        self.exported = false;
        self.export_error = None;
        self.export_wait = None;
    }

    #[cfg(test)]
    pub(crate) fn is_mounted(&self) -> bool {
        self.mounted
    }

    /// The clock the view reads "now" from (the page's, which the tests move).
    #[cfg(test)]
    pub(crate) fn set_clock(&mut self, clock: Clock) {
        self.clock = clock;
        self.now = (self.clock)();
        self.period = period_of(&self.pick, &self.now);
    }

    /// What the tab reads and writes through. The tests give a fake one.
    pub(crate) fn set_backend(&mut self, backend: Arc<dyn TicketBackend>, cx: &mut Context<Self>) {
        self.backend = Some(backend);
        self.try_start(cx);
        self.flush(cx);
        cx.notify();
    }

    /// The window came back to the front: ClickUp may have been connected or
    /// disconnected in Settings meanwhile.
    pub(crate) fn resume(&mut self, cx: &mut Context<Self>) {
        if self.mounted && self.started {
            self.check_connection(false, cx);
        }
    }

    fn try_start(&mut self, cx: &mut Context<Self>) {
        if !self.mounted || self.started || self.backend.is_none() {
            return;
        }
        self.started = true;
        let now = self.now;
        // A live job from before this mount; an event that already arrived is newer.
        let seq = self.pending_seq;
        self.spawn_job(
            "status",
            cx,
            |backend| backend.status(),
            move |this, status, cx| {
                if let Ok(status) = status {
                    if let Some(event) = status.event
                        && this.job.is_none()
                    {
                        this.job = Some(event);
                    }
                    if this.pending_seq == seq {
                        this.set_pending(status.pending_fetches);
                    }
                }
                this.settle(cx);
            },
        );
        self.spawn_job(
            "prefs",
            cx,
            move |backend| backend.load_prefs(&now),
            |this, (settings, pick), cx| {
                this.settings = settings;
                this.set_pick(pick);
                this.prefs_ready = true;
                this.dirty = true;
                this.settle(cx);
            },
        );
        // The first page to read it decides; the choice is app-wide.
        self.spawn_job(
            "chart-kind",
            cx,
            |backend| backend.load_chart_kind(),
            |this, kind, cx| {
                this.chart_prefs.update(cx, |prefs, cx| {
                    if !prefs.settled {
                        prefs.settled = true;
                        prefs.kind = kind
                            .as_deref()
                            .and_then(ChartKind::parse)
                            .unwrap_or_default();
                        cx.notify();
                    }
                });
            },
        );
        self.spawn_job(
            "ai",
            cx,
            |backend| backend.ai_config(),
            |this, config, cx| {
                if let Ok(config) = config {
                    this.ai = Some(config);
                    cx.notify();
                }
            },
        );
        self.check_connection(true, cx);
    }

    fn set_pick(&mut self, pick: PeriodPick) {
        self.period = period_of(&pick, &self.now);
        self.pick = pick;
        self.dirty = true;
    }

    // ---- ClickUp connection ----------------------------------------------

    /// The connection check and the workspace list; they change when the
    /// token does. `always` reads the list even when nothing changed.
    fn check_connection(&mut self, always: bool, cx: &mut Context<Self>) {
        self.check_gen += 1;
        let generation = self.check_gen;
        self.spawn_job(
            "connected",
            cx,
            |backend| backend.connected(),
            move |this, status, cx| {
                if this.check_gen != generation {
                    return;
                }
                match status {
                    Err(error) => this.connect_error = Some(error_text(error)),
                    Ok(connected) => {
                        let same = this.connected == Some(connected)
                            && this.workspaces_state == WorkspacesState::Ready;
                        this.connected = Some(connected);
                        this.connect_error = None;
                        if !connected {
                            this.workspaces.clear();
                            this.workspaces_state = WorkspacesState::Ready;
                            this.workspaces_error = None;
                        } else if !same || always {
                            // A refresh of a list already shown does not blank the view.
                            if this.workspaces_state != WorkspacesState::Ready {
                                this.workspaces_state = WorkspacesState::Loading;
                            }
                            this.spawn_job(
                                "workspaces",
                                cx,
                                |backend| backend.workspaces(),
                                move |this, list, cx| {
                                    if this.check_gen != generation {
                                        return;
                                    }
                                    match list {
                                        Ok(list) => {
                                            this.workspaces = list;
                                            this.workspaces_state = WorkspacesState::Ready;
                                            this.workspaces_error = None;
                                        }
                                        Err(error) => {
                                            this.workspaces_state = WorkspacesState::Error;
                                            this.workspaces_error = Some(error_text(error));
                                        }
                                    }
                                    this.settle(cx);
                                },
                            );
                        }
                    }
                }
                this.settle(cx);
            },
        );
        cx.notify();
    }

    /// Tries the connection check and the workspace list again after a failure.
    pub(crate) fn retry(&mut self, cx: &mut Context<Self>) {
        self.connected = None;
        self.connect_error = None;
        self.workspaces_state = WorkspacesState::Loading;
        self.workspaces_error = None;
        self.check_connection(true, cx);
    }

    pub(crate) fn open_settings(&mut self, cx: &mut Context<Self>) {
        cx.emit(TicketEvent::OpenSettings);
    }

    // ---- job events -------------------------------------------------------

    fn set_pending(&mut self, count: u32) {
        self.pending_fetches = count;
        self.pending_seq += 1;
    }

    /// A window's fetch ended: only a clean `done` makes it fetched.
    fn settle_window(&mut self, key: String, clean: bool) {
        self.pending_keys.remove(&key);
        if clean {
            self.fetched.insert(key);
        } else {
            self.failed = Some((key, self.connected));
        }
    }

    pub(super) fn on_job_event(&mut self, event: TicketJobEvent, cx: &mut Context<Self>) {
        if !self.mounted {
            return;
        }
        self.job = Some(event.clone());
        // Progress events are sent without the queue lock and can be stale;
        // queuing and ending are exact.
        if event.state != "running" {
            self.set_pending(event.pending_fetches);
        }
        self.job_error = job_error_after(self.job_error.as_deref(), &event);
        if event.kind == "fetch" && is_end(&event) {
            // Only a clean done makes the window fetched; a cancel emits one end
            // per dropped job, each naming its own.
            let clean = event.state == "done" && event.failed == 0;
            match self.job_keys.remove(&event.job_id) {
                Some(key) => self.settle_window(key, clean),
                None => {
                    // The command's answer may still be on its way.
                    self.ended_jobs.retain(|(id, _)| *id != event.job_id);
                    self.ended_jobs.push((event.job_id, clean));
                    if self.ended_jobs.len() > ENDED_LIMIT {
                        self.ended_jobs.remove(0);
                    }
                }
            }
            if let Some(wait) = &mut self.export_wait
                && event.pending_fetches == 0
            {
                wait.drained(event.job_id);
            }
        }
        if store_may_have_changed(&event) {
            self.reload(cx);
            // A fetch may have completed the previous period's window too.
            self.effects.previous = None;
            self.forget_year();
        }
        self.settle(cx);
    }

    // ---- loading ----------------------------------------------------------

    pub(super) fn workspace_id(&self) -> String {
        self.settings
            .scope
            .as_ref()
            .map(|scope| scope.workspace_id.clone())
            .unwrap_or_default()
    }

    pub(super) fn window(&self) -> (String, String) {
        let window = window_of(&self.period);
        (window.from, window.to)
    }

    fn window_key(&self) -> String {
        let (from, to) = self.window();
        fetch_key(&self.workspace_id(), &from, &to)
    }

    /// The stored tickets show at once; the fetch tops them up.
    pub(super) fn reload(&mut self, cx: &mut Context<Self>) {
        let workspace = self.workspace_id();
        let (from, _) = self.window();
        let key = self.window_key();
        self.request += 1;
        let seq = self.request;
        if workspace.is_empty() {
            self.loaded = Some(Loaded {
                key,
                tickets: Arc::new(Vec::new()),
            });
            self.load_error = None;
            self.dirty = true;
            return;
        }
        self.spawn_job(
            "load",
            cx,
            move |backend| backend.load(&workspace, &from),
            move |this, result, cx| {
                // A newer read (or a new window) decides what is shown.
                if seq != this.request {
                    return;
                }
                match result {
                    Ok(tickets) => {
                        this.loaded = Some(Loaded {
                            key,
                            tickets: Arc::new(tickets),
                        });
                        this.load_error = None;
                        this.dirty = true;
                    }
                    Err(error) => this.load_error = Some(error_text(error)),
                }
                this.settle(cx);
            },
        );
    }

    /// Tickets of another window or workspace are not shown under this one.
    pub(super) fn tickets(&self) -> Arc<Vec<TicketRecord>> {
        match &self.loaded {
            Some(loaded) if loaded.key == self.window_key() => loaded.tickets.clone(),
            _ => Arc::new(Vec::new()),
        }
    }

    fn recompute(&mut self) {
        let tickets = self.tickets();
        let (shown, active) = match &self.settings.scope {
            Some(scope) => (
                period_tickets(&tickets, scope, &self.period),
                active_tickets(&tickets, scope, &self.period),
            ),
            None => (Vec::new(), Vec::new()),
        };
        let board = build_board(&shown, &self.settings.points, &active);
        let to_assess = to_assess_count(shown.len(), board.assessed, self.settings.percent);
        let run_ids = pick_for_run(&shown, to_assess);
        self.derived_seq += 1;
        self.derived = Derived {
            seq: self.derived_seq,
            statuses: statuses_of(&tickets),
            run_key: run_ids.join("\0"),
            run_ids,
            to_assess,
            stats: tile_stats(&board),
            comparison: self.comparison(),
            trend: Arc::new(member_trend(&self.period, &shown, &board, &self.now)),
            matrix: level_matrix(&board, &shown),
            board,
            shown,
        };
        self.dirty = false;
    }

    /// The previous period's key: its workspace, spaces and whole window. `None`
    /// when there is nothing to compare with.
    fn previous_key(&self) -> Option<(String, String, String)> {
        let scope = self.settings.scope.as_ref()?;
        let before = previous_span(&self.period, None, &self.now)?;
        let window = window_of(&before.before);
        Some((scope_fetch_id(scope), window.from, window.to))
    }

    /// How the period compares with the previous one, from what the store held.
    fn comparison(&self) -> Comparison {
        let Some(scope) = &self.settings.scope else {
            return Comparison::Off;
        };
        let Some(before) = previous_span(&self.period, None, &self.now) else {
            return Comparison::Off;
        };
        let Some(key) = self.previous_key().map(|k| key_text(&k)) else {
            return Comparison::Off;
        };
        match &self.previous {
            Some(read) if read.key == key => match &read.tickets {
                None => Comparison::Missing {
                    note: no_comparison_note(&before.label),
                },
                Some(tickets) => {
                    let then = period_tickets(tickets, scope, &before.span);
                    let board = build_board(&then, &self.settings.points, &[]);
                    Comparison::Ready {
                        against: before.against,
                        stats: tile_stats(&board),
                        board,
                    }
                }
            },
            _ => Comparison::Pending,
        }
    }

    /// Reads the previous period from the store for the window now on screen.
    /// Never fetches: a window the store does not hold whole is left out.
    fn read_previous(&mut self, cx: &mut Context<Self>) {
        let (Some(scope), Some(key)) = (self.settings.scope.clone(), self.previous_key()) else {
            self.previous = None;
            self.dirty = true;
            return;
        };
        let text = key_text(&key);
        let (from, to) = (key.1, key.2);
        let done = text.clone();
        self.spawn_job(
            "previous",
            cx,
            move |backend| backend.previous(&scope, &from, &to),
            move |this, answer, cx| {
                // The window moved on while this was read.
                if this.previous_key().map(|k| key_text(&k)).as_deref() != Some(done.as_str()) {
                    return;
                }
                this.previous = Some(PreviousRead {
                    key: done,
                    tickets: answer.ok().flatten().map(Arc::new),
                });
                this.dirty = true;
                this.settle(cx);
            },
        );
    }

    /// Reads the colors picked for the members of the workspace now shown.
    fn read_colors(&mut self, cx: &mut Context<Self>) {
        let workspace = self.workspace_id();
        self.colors_touched = false;
        // Another workspace's picks are not this one's, whatever is read next.
        self.colors = ColorOverrides::new();
        self.colors_gen += 1;
        if workspace.is_empty() {
            return;
        }
        let asked = workspace.clone();
        self.spawn_job(
            "colors",
            cx,
            move |backend| backend.load_colors(&workspace),
            move |this, saved, cx| {
                if this.workspace_id() != asked {
                    return;
                }
                // A color picked while this was read wins; the rest is what was saved.
                let picked = std::mem::take(&mut this.colors);
                this.colors = saved;
                this.colors.extend(picked);
                this.colors_gen += 1;
                if this.colors_touched {
                    let colors = this.colors.clone();
                    this.spawn_job(
                        "save-colors",
                        cx,
                        move |backend| backend.save_colors(&asked, &colors),
                        |_, (), _| {},
                    );
                }
                this.settle(cx);
            },
        );
    }

    /// A color picked from the palette for one member; saved for the workspace.
    pub(super) fn set_member_color(
        &mut self,
        member: &str,
        hex: &'static str,
        cx: &mut Context<Self>,
    ) {
        let workspace = self.workspace_id();
        if workspace.is_empty() {
            return;
        }
        self.colors_touched = true;
        self.colors.insert(member.to_string(), hex);
        self.colors_gen += 1;
        let colors = self.colors.clone();
        self.spawn_job(
            "save-colors",
            cx,
            move |backend| backend.save_colors(&workspace, &colors),
            |_, (), _| {},
        );
        self.settle(cx);
    }

    /// The chart type every chart on every page shares; saved for next time.
    pub(super) fn set_chart_kind(&mut self, kind: ChartKind, cx: &mut Context<Self>) {
        let changed = self.chart_prefs.update(cx, |prefs, cx| {
            prefs.settled = true;
            let changed = prefs.kind != kind;
            if changed {
                prefs.kind = kind;
                cx.notify();
            }
            changed
        });
        if changed {
            self.spawn_job(
                "save-chart-kind",
                cx,
                move |backend| backend.save_chart_kind(kind.id()),
                |_, (), _| {},
            );
        }
        self.settle(cx);
    }

    // ---- the effects --------------------------------------------------------

    /// Runs the effects whose inputs changed and redraws. Everything that
    /// changes state calls it afterwards.
    pub(super) fn settle(&mut self, cx: &mut Context<Self>) {
        if !self.mounted || !self.prefs_ready || self.backend.is_none() {
            cx.notify();
            return;
        }
        // An effect can change what the others depend on (a refreshed name
        // changes the scope), so run until nothing moves.
        for _ in 0..8 {
            if !self.settle_once(cx) {
                break;
            }
        }
        // A name refreshed by the effects above is saved too.
        self.flush(cx);
        cx.notify();
    }

    fn settle_once(&mut self, cx: &mut Context<Self>) -> bool {
        let mut again = false;
        let scope = self.settings.scope.clone();
        let (from, to) = self.window();

        // A report saved for another workspace or period is not this one's.
        let export = (self.workspace_id(), self.period.id.clone());
        if self.effects.export.as_ref() != Some(&export) {
            self.effects.export = Some(export);
            self.export_run += 1;
            self.exported = false;
            self.export_error = None;
        }

        let key = self.window_key();
        if self.effects.reload.as_ref() != Some(&key) {
            self.effects.reload = Some(key);
            self.reload(cx);
        }

        // The previous period is only read from the store: it never fetches, and
        // the fetch below does not wait for it.
        let previous = self.previous_key().map(|k| key_text(&k));
        if self.effects.previous.as_ref() != Some(&previous) {
            self.effects.previous = Some(previous);
            self.read_previous(cx);
        }
        let workspace = self.workspace_id();
        if self.effects.colors.as_ref() != Some(&workspace) {
            self.effects.colors = Some(workspace);
            self.read_colors(cx);
        }

        // Without a connection the job would only fail; connecting re-runs this.
        let busy = fetch_busy(self.job.as_ref());
        let fetch_id = scope.as_ref().map(scope_fetch_id).unwrap_or_default();
        let inputs = (
            fetch_id.clone(),
            from.clone(),
            to.clone(),
            busy,
            self.connected,
        );
        if self.effects.fetch.as_ref() != Some(&inputs) {
            self.effects.fetch = Some(inputs);
            self.fetch_effect(scope.as_ref(), &fetch_id, &from, &to, cx);
        }

        if self.dirty {
            self.recompute();
            // An excluded member's name follows ClickUp: save a newer name a loaded ticket carries.
            if let Some(scope) = self.settings.scope.clone() {
                let tickets = self.tickets();
                let refreshed: Option<Vec<ExcludedMember>> =
                    match excluded_members_of(&tickets, &scope) {
                        std::borrow::Cow::Owned(members) => Some(members),
                        std::borrow::Cow::Borrowed(_) => None,
                    };
                if let Some(members) = refreshed {
                    let mut next = scope;
                    next.excluded_members = members;
                    self.apply_scope(Some(next));
                    again = true;
                }
            }
        }

        // The page's year is read from the store when it is open; never fetched.
        self.ensure_year(cx);

        let run_key = self.derived.run_key.clone();
        if self.effects.run_key.as_ref() != Some(&run_key) {
            self.effects.run_key = Some(run_key.clone());
            self.estimate_effect(&run_key, cx);
        }

        // The Scope panel lists the spaces of the workspace it is open on.
        let spaces_for = (self.scope_open && scope.is_some()).then(|| self.workspace_id());
        if self.effects.spaces != spaces_for {
            self.effects.spaces = spaces_for.clone();
            match spaces_for {
                Some(workspace) => self.load_spaces(workspace, cx),
                None => {
                    self.spaces = None;
                    self.spaces_gen += 1;
                }
            }
        }
        again
    }

    fn fetch_effect(
        &mut self,
        scope: Option<&WorkspaceScope>,
        fetch_id: &str,
        from: &str,
        to: &str,
        cx: &mut Context<Self>,
    ) {
        if self.connected != Some(true) {
            self.failed = None;
        }
        let Some(scope) = scope else { return };
        if self.connected != Some(true) {
            return;
        }
        let key = fetch_key(fetch_id, from, to);
        if self
            .failed
            .as_ref()
            .is_some_and(|(failed, _)| *failed != key)
        {
            self.failed = None;
        }
        if self
            .failed
            .as_ref()
            .is_some_and(|(_, connected)| *connected == self.connected)
        {
            return;
        }
        // Re-runs when a running fetch ends, so a window skipped meanwhile is fetched.
        if should_fetch(&key, &self.fetched, self.job.as_ref()) && !self.pending_keys.contains(&key)
        {
            self.job_error = None;
            self.queue_window(
                key,
                scope.clone(),
                from.into(),
                to.into(),
                cx,
                |this, result, _| {
                    if let Err(error) = result {
                        this.job_error = Some(error);
                    }
                },
            );
        }
    }

    /// Queues the fetch of a window and follows it until its job ends.
    fn queue_window(
        &mut self,
        key: String,
        scope: WorkspaceScope,
        from: String,
        to: String,
        cx: &mut Context<Self>,
        done: impl FnOnce(&mut Self, Result<(), String>, &mut Context<Self>) + 'static,
    ) {
        if self.backend.is_none() {
            return;
        }
        self.pending_keys.insert(key.clone());
        self.spawn_job(
            "fetch",
            cx,
            move |backend| backend.fetch(&scope, &from, &to),
            move |this, result, cx| {
                match result {
                    Err(error) => {
                        this.pending_keys.remove(&key);
                        done(this, Err(error), cx);
                    }
                    Ok(id) => {
                        match this.ended_jobs.iter().position(|(ended, _)| *ended == id) {
                            None => {
                                this.job_keys.insert(id, key);
                            }
                            Some(at) => {
                                let (_, clean) = this.ended_jobs.remove(at);
                                this.settle_window(key, clean);
                            }
                        }
                        done(this, Ok(()), cx);
                    }
                }
                this.settle(cx);
            },
        );
    }

    fn estimate_effect(&mut self, run_key: &str, cx: &mut Context<Self>) {
        self.estimate_gen += 1;
        let generation = self.estimate_gen;
        self.estimate_task = None;
        if run_key.is_empty() {
            self.estimate = None;
            return;
        }
        let ids = self.derived.run_ids.clone();
        self.estimate_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(ESTIMATE_DEBOUNCE).await;
            let _ = this.update(cx, |this, cx| {
                if this.estimate_gen != generation {
                    return;
                }
                this.spawn_job(
                    "estimate",
                    cx,
                    move |backend| backend.estimate(&ids),
                    move |this, estimate, cx| {
                        if this.estimate_gen == generation {
                            this.estimate = estimate.ok();
                            cx.notify();
                        }
                    },
                );
            });
        }));
    }

    fn load_spaces(&mut self, workspace: String, cx: &mut Context<Self>) {
        self.spaces_gen += 1;
        let generation = self.spaces_gen;
        self.spaces = Some(Spaces::Loading);
        self.spawn_job(
            "spaces",
            cx,
            move |backend| backend.spaces(&workspace),
            move |this, spaces, cx| {
                if this.spaces_gen == generation {
                    this.spaces = Some(match spaces {
                        Ok(list) => Spaces::Ready(list),
                        Err(error) => Spaces::Failed(error_text(error)),
                    });
                    cx.notify();
                }
            },
        );
    }

    // ---- preferences --------------------------------------------------------

    /// Applies a scope to the state without saving it or settling.
    fn apply_scope(&mut self, scope: Option<WorkspaceScope>) {
        self.settings.scope = scope.clone();
        self.dirty = true;
        self.queue_patch(Patch {
            scope: Some(scope),
            ..Patch::default()
        });
    }

    fn queue_patch(&mut self, patch: Patch) {
        self.queued = Some(match self.queued.take() {
            Some(queued) => queued.then(patch),
            None => patch,
        });
    }

    /// Writes the queued patch, if none is being written.
    fn flush(&mut self, cx: &mut Context<Self>) {
        if self.writing || self.backend.is_none() {
            return;
        }
        let Some(patch) = self.queued.take().filter(|patch| !patch.is_empty()) else {
            return;
        };
        self.writing = true;
        let generation = self.mount_gen;
        let Some(backend) = self.backend.clone() else {
            return;
        };
        let write = move |this: &mut Self, stored: TicketBoardSettings, cx: &mut Context<Self>| {
            this.writing = false;
            // Points are edited in Settings: take what is stored, unless a
            // newer change of ours is still to be written.
            if this.queued.is_none() && this.mount_gen == generation {
                if this.settings.points != stored.points {
                    this.settings.points = stored.points;
                    this.dirty = true;
                }
                this.settle(cx);
            }
            this.flush(cx);
        };
        #[cfg(test)]
        if self.manual {
            let stored = backend.patch_prefs(&patch);
            self.held.push(Held {
                label: "patch",
                run: Some(Box::new(move |this, cx| write(this, stored, cx))),
            });
            return;
        }
        let task = cx
            .background_executor()
            .spawn(async move { backend.patch_prefs(&patch) });
        cx.spawn(async move |this, cx| {
            let stored = task.await;
            let _ = this.update(cx, |this, cx| write(this, stored, cx));
        })
        .detach();
    }

    pub(crate) fn set_scope(&mut self, scope: Option<WorkspaceScope>, cx: &mut Context<Self>) {
        self.apply_scope(scope);
        self.flush(cx);
        self.settle(cx);
    }

    pub(crate) fn set_percent(&mut self, percent: f64, cx: &mut Context<Self>) {
        let percent = if percent.is_finite() {
            percent.round().clamp(0., 100.)
        } else {
            0.
        };
        self.settings.percent = percent;
        self.dirty = true;
        self.queue_patch(Patch {
            percent: Some(percent),
            ..Patch::default()
        });
        self.flush(cx);
        self.settle(cx);
    }

    fn choose(&mut self, pick: PeriodPick, cx: &mut Context<Self>) {
        self.queue_patch(Patch {
            range: Some(pick.clone()),
            ..Patch::default()
        });
        self.set_pick(pick);
        self.flush(cx);
        self.settle(cx);
    }

    pub(crate) fn set_kind(&mut self, kind: PeriodKind, cx: &mut Context<Self>) {
        self.choose(PeriodPick { kind, id: None }, cx);
    }

    pub(crate) fn go_previous(&mut self, cx: &mut Context<Self>) {
        if let Some(before) = previous_period(&self.period) {
            let pick = pick_of(&before, &self.now);
            self.choose(pick, cx);
        }
    }

    pub(crate) fn go_next(&mut self, cx: &mut Context<Self>) {
        if let Some(after) = next_period(&self.period, &self.now) {
            let pick = pick_of(&after, &self.now);
            self.choose(pick, cx);
        }
    }

    pub(crate) fn has_next(&self) -> bool {
        next_period(&self.period, &self.now).is_some()
    }

    /// The Workspace select picked `id`: a fresh scope for it.
    pub(crate) fn pick_workspace(&mut self, id: &str, cx: &mut Context<Self>) {
        if id.is_empty()
            || self
                .settings
                .scope
                .as_ref()
                .is_some_and(|scope| scope.workspace_id == id)
        {
            return;
        }
        let Some(workspace) = self.workspaces.iter().find(|w| w.id == id).cloned() else {
            return;
        };
        self.set_scope(
            Some(WorkspaceScope {
                workspace_id: workspace.id,
                name: workspace.name,
                space_ids: None,
                excluded_statuses: Vec::new(),
                delivered_statuses: Vec::new(),
                excluded_members: Vec::new(),
            }),
            cx,
        );
    }

    pub(crate) fn toggle_scope(&mut self, cx: &mut Context<Self>) {
        self.scope_open = !self.scope_open;
        self.settle(cx);
    }

    // ---- actions ------------------------------------------------------------

    /// Runs an action that can fail with a message the tab shows: the job
    /// error is cleared first.
    fn run_action<T: Send + 'static>(
        &mut self,
        label: &'static str,
        cx: &mut Context<Self>,
        work: impl FnOnce(Arc<dyn TicketBackend>) -> Result<T, String> + Send + 'static,
        done: impl FnOnce(&mut Self, T, &mut Context<Self>) + 'static,
    ) {
        self.job_error = None;
        cx.notify();
        self.spawn_job(label, cx, work, move |this, result, cx| {
            match result {
                Ok(value) => done(this, value, cx),
                Err(error) => this.job_error = Some(error_text(error)),
            }
            cx.notify();
        });
    }

    /// Fetches the window again, unless that window's fetch is already pending.
    pub(crate) fn refresh(&mut self, cx: &mut Context<Self>) {
        self.job_error = None;
        let Some(scope) = self.settings.scope.clone() else {
            cx.notify();
            return;
        };
        let (from, to) = self.window();
        let key = fetch_key(&scope_fetch_id(&scope), &from, &to);
        // Already queued or running for this window: nothing to add.
        if self.pending_keys.contains(&key) {
            cx.notify();
            return;
        }
        self.failed = None;
        self.queue_window(key, scope, from, to, cx, |this, result, _| {
            if let Err(error) = result {
                this.job_error = Some(error);
            }
        });
        cx.notify();
    }

    /// Scores the tickets of the next run.
    pub(crate) fn score(&mut self, cx: &mut Context<Self>) {
        let ids = self.derived.run_ids.clone();
        if ids.is_empty() {
            return;
        }
        self.run_action("assess", cx, move |b| b.assess(&ids, false), |_, _, _| {});
    }

    pub(crate) fn reassess(&mut self, ids: Vec<String>, cx: &mut Context<Self>) {
        self.run_action("assess", cx, move |b| b.assess(&ids, true), |_, _, _| {});
    }

    pub(crate) fn cancel(&mut self, cx: &mut Context<Self>) {
        // The cancel drops queued jobs too; each one's end event settles its window.
        self.run_action(
            "cancel",
            cx,
            |backend| {
                backend.cancel();
                Ok(())
            },
            |_, _, _| {},
        );
    }

    /// Sets (or with `None` clears) the manual level of a ticket.
    pub(crate) fn set_override(
        &mut self,
        ticket_id: &str,
        level: Option<&str>,
        note: Option<&str>,
        cx: &mut Context<Self>,
    ) {
        let (id, level, note) = (
            ticket_id.to_string(),
            level.map(String::from),
            note.map(String::from),
        );
        self.run_action(
            "override",
            cx,
            move |b| b.set_override(&id, level.as_deref(), note.as_deref()),
            |this, _, cx| this.reload(cx),
        );
    }

    // ---- reads for the drawing and the tests --------------------------------

    pub(super) fn unreachable(&self) -> bool {
        self.settings.scope.as_ref().is_some_and(|scope| {
            self.workspaces_state == WorkspacesState::Ready
                && !self.workspaces.iter().any(|w| w.id == scope.workspace_id)
        })
    }

    pub(super) fn fetching(&self) -> bool {
        fetch_busy(self.job.as_ref())
    }

    /// Refresh needs a reachable workspace and no fetch running.
    pub(super) fn refresh_disabled(&self) -> bool {
        self.settings.scope.is_none() || self.unreachable() || self.fetching()
    }

    /// Export needs delivered tickets on screen and nothing fetching or exporting
    /// (a fetch queued behind an assessment counts).
    pub(super) fn export_disabled(&self) -> bool {
        self.refresh_disabled()
            || self.pending_fetches > 0
            || self.exporting
            || self.derived.board.total == 0
    }

    /// What the body says now.
    pub(super) fn body_state(&self) -> BodyState {
        let problem = |message: String, retry: bool| BodyState::Problem { message, retry };
        if self.connected != Some(true)
            && let Some(error) = &self.connect_error
        {
            return problem(format!("Could not reach ClickUp. {error}"), true);
        }
        match self.connected {
            None => return BodyState::Checking,
            Some(false) => {
                return problem(
                    "Connect ClickUp in Settings to score tickets.".into(),
                    false,
                );
            }
            Some(true) => {}
        }
        if self.workspaces_state == WorkspacesState::Error {
            let message = format!(
                "Could not load ClickUp workspaces. {}",
                self.workspaces_error.as_deref().unwrap_or("")
            );
            return problem(message.trim().to_string(), true);
        }
        let Some(scope) = &self.settings.scope else {
            return match self.workspaces_state {
                WorkspacesState::Loading => BodyState::LoadingWorkspaces,
                _ if self.workspaces.is_empty() => {
                    problem("This ClickUp token has no workspaces.".into(), false)
                }
                _ => BodyState::PickWorkspace,
            };
        };
        if self.unreachable() {
            let name = if scope.name.is_empty() {
                &scope.workspace_id
            } else {
                &scope.name
            };
            return problem(
                format!(
                    "This ClickUp token cannot access {name}. Pick another workspace or update the token in Settings."
                ),
                false,
            );
        }
        BodyState::Board
    }

    /// An error to show above the board: a failed read, else a failed job.
    pub(super) fn error(&self) -> Option<&str> {
        self.load_error.as_deref().or(self.job_error.as_deref())
    }
}
