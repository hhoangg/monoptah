//! The GitHub tab of the Contributors page (`ContributorsView.tsx`): toolbar,
//! alerts, the sync panels and the section tabs. The rules live in `data.rs`;
//! the Stats, Leaderboard and Identities bodies are filled in by their own
//! files (`stats.rs`, `leaderboard.rs`, `identities.rs`).
use super::{
    backend::Backend,
    data::{
        Clock, Effects, EstimateSlot, Loaded, Lru, Meta, SaveState, Snapshot, Status, SyncState,
    },
    jobs::{JobsEvent, SyncJobs},
    ui::{self, Dropdown},
};
use crate::{
    popover::{Align, Anchor, AnchorSlot, Side, popover},
    theme::*,
};
use chrono::{DateTime, Local};
use gpui::{
    AnyElement, AnyView, Context, Entity, EventEmitter, FocusHandle, Focusable, IntoElement,
    Render, Role, ScrollHandle, SharedString, Subscription, Task, Window, div, prelude::*, px, rgb,
};
use monocode_core::contributors::{
    RawData,
    model::{
        config::CONFIG_PATH,
        period::{PERIOD_KINDS, PeriodKind, PeriodPick},
        pipeline::ContributorsModel,
        snapshot::SnapshotScope,
    },
};
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

/// The sections under the sync bar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Section {
    Stats,
    Leaderboard,
    Identities,
}
impl Section {
    pub(crate) const ALL: [Section; 3] = [Self::Stats, Self::Leaderboard, Self::Identities];
    pub(crate) fn id(self) -> &'static str {
        match self {
            Self::Stats => "stats",
            Self::Leaderboard => "leaderboard",
            Self::Identities => "identities",
        }
    }
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Stats => "Stats",
            Self::Leaderboard => "Leaderboard",
            Self::Identities => "Identities",
        }
    }
}

/// What the tab tells its host.
#[derive(Clone, Debug)]
pub(crate) enum GithubEvent {
    /// An Ask Agent button was pressed with the data on screen; the page opens the dialog.
    Ask(SnapshotScope),
}

/// A backend call the tests keep back until they answer it.
#[cfg(test)]
pub(super) struct Held {
    pub label: &'static str,
    pub run: Option<HeldRun>,
}
#[cfg(test)]
type HeldRun = Box<dyn FnOnce(&mut GithubView, &mut Context<GithubView>)>;

pub(crate) struct GithubView {
    /// The project as the page knows it.
    pub(super) project: String,
    /// The local folder, empty for a remote project (nothing is read for it).
    pub(super) cwd: String,
    pub(super) backend: Option<Arc<dyn Backend>>,
    pub(super) jobs: Entity<SyncJobs>,
    pub(super) clock: Clock,

    // selection
    pub(super) now: DateTime<Local>,
    pub(super) pick: PeriodPick,
    pub(super) merge: bool,
    pub(super) pick_touched: bool,
    pub(super) merge_touched: bool,
    /// The chart type every chart on the page shares, and whether the user set it
    /// before the saved one was read.
    pub(super) chart_prefs: Entity<super::chart::ChartPrefs>,
    /// A screen reader is attached: the table is not drawn from a cache.
    pub(super) a11y: bool,
    /// The colors picked for people of this project (saved apart from the config).
    pub(super) color_overrides: super::backend::ColorOverrides,
    pub(super) colors_touched: bool,
    pub(super) prefs_ready: bool,
    pub(super) picked_branch: Option<String>,

    // what was read
    pub(super) meta: Option<Meta>,
    pub(super) loads: Lru<Arc<Loaded>>,
    pub(super) load_started: HashMap<String, u64>,
    pub(super) load_inflight: HashMap<String, u64>,
    pub(super) first: Lru<Option<String>>,
    pub(super) first_started: HashMap<String, u64>,
    pub(super) first_inflight: HashMap<String, u64>,
    pub(super) first_head: HashMap<String, String>,
    /// Auto top-ups and repairs already started, per period and branch head: one per head.
    pub(super) topped_up: HashSet<String>,
    pub(super) meta_inflight: Option<(u64, u64)>,
    pub(super) failures: HashMap<String, String>,
    pub(super) save_error: Option<String>,
    pub(super) action_error: Option<String>,
    pub(super) running: usize,
    /// The fetch of `origin` in flight, if any. It counts toward `running`.
    pub(super) fetching: Option<super::data::Fetching>,
    /// The notes about origin, by branch key (`first_key`): its last fetch failed or found the branch gone.
    pub(super) origin_issues: HashMap<String, super::data::OriginIssue>,
    pub(super) estimate: Option<EstimateSlot>,
    pub(super) estimate_gen: u64,
    pub(super) saves: SaveState,
    pub(super) last_shown: Option<(
        Arc<RawData>,
        monocode_core::contributors::model::period::Period,
    )>,
    pub(super) last_auto_reload: Option<i64>,
    pub(super) effects: Effects,
    pub(super) seq: u64,
    pub(super) loaded_ids: u64,

    // the model
    pub(super) model: Option<Arc<ContributorsModel>>,
    pub(super) model_inputs: Option<super::data::ModelInputs>,
    pub(super) model_gen: u64,
    pub(super) model_requested: u64,
    pub(super) model_done: u64,
    /// The model on its way differs only by a color: the page is not stale meanwhile.
    pub(super) model_quiet: bool,

    // the clock
    pub(super) midnight: Option<Task<()>>,
    pub(super) midnight_for: Option<DateTime<Local>>,
    _minute: Task<()>,

    // the view
    pub(super) section: Section,
    pub(super) menu_open: bool,
    pub(super) menu_anchor: AnchorSlot,
    pub(super) menu_focus: FocusHandle,
    pub(super) branch_dd: Dropdown,
    pub(super) focus: FocusHandle,
    /// The formula sidebar of the Leaderboard: drawn over the right 400px of the body.
    pub(super) sidebar: Option<AnyView>,
    /// The chart and the table of the Stats section.
    pub(super) stats_ui: super::stats::StatsUi,
    /// The Activity card: the year read for it and what it shows.
    pub(super) activity: super::activity::ActivityUi,
    /// The person whose detail page replaces the Stats body, if one is open.
    pub(super) detail: Option<String>,
    /// What the detail page keeps: its chart, its numbers and the list's scroll.
    pub(super) detail_ui: super::detail::DetailUi,
    /// The scroll of the page body, so Back puts the list where it was.
    pub(super) scroll: ScrollHandle,
    /// The Leaderboard's "how" link focus and podium width.
    pub(super) leaderboard_ui: super::leaderboard::LeaderboardUi,
    /// The Identities section: an entity that keeps its fields, menu and drag.
    pub(super) identities_ui: Option<Entity<super::identities::IdentitiesView>>,
    _subscriptions: Vec<Subscription>,

    #[cfg(test)]
    pub(super) manual: bool,
    /// With `manual`, hold the answer of the fetch too; by default it is
    /// delivered at once so tests that press Sync need not release it.
    #[cfg(test)]
    pub(super) hold_fetch: bool,
    #[cfg(test)]
    pub(super) held: Vec<Held>,
}
impl EventEmitter<GithubEvent> for GithubView {}
impl Focusable for GithubView {
    fn focus_handle(&self, _: &gpui::App) -> FocusHandle {
        self.focus.clone()
    }
}

impl GithubView {
    /// `project` is the folder whose history is shown; a remote one stays idle.
    pub(crate) fn new(
        project: String,
        jobs: Entity<SyncJobs>,
        clock: Clock,
        cx: &mut Context<Self>,
    ) -> Self {
        let cwd = if monocode_core::files::remote::is_remote(std::path::Path::new(&project)) {
            String::new()
        } else {
            project.clone()
        };
        let chart_prefs = super::chart::shared_prefs(cx);
        let subscriptions = vec![
            cx.observe(&chart_prefs, |this, _, cx| this.push_stats(cx)),
            // Progress and the job list redraw the panels and may unlock effects.
            cx.observe(&jobs, |this, _, cx| this.settle(cx)),
            cx.subscribe(&jobs, |this, _, event: &JobsEvent, cx| {
                this.job_ended(event, cx)
            }),
        ];
        // "Synced 5m ago" is redrawn every minute.
        let minute = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_secs(60))
                    .await;
                if this.update(cx, |_, cx| cx.notify()).is_err() {
                    break;
                }
            }
        });
        let now = clock();
        Self {
            project,
            cwd,
            backend: None,
            jobs,
            clock,
            now,
            pick: PeriodPick {
                kind: PeriodKind::Days7,
                id: None,
            },
            merge: true,
            pick_touched: false,
            merge_touched: false,
            chart_prefs,
            a11y: false,
            color_overrides: Default::default(),
            colors_touched: false,
            prefs_ready: false,
            picked_branch: None,
            meta: None,
            loads: Lru::default(),
            load_started: HashMap::new(),
            load_inflight: HashMap::new(),
            first: Lru::default(),
            first_started: HashMap::new(),
            first_inflight: HashMap::new(),
            first_head: HashMap::new(),
            topped_up: HashSet::new(),
            meta_inflight: None,
            failures: HashMap::new(),
            save_error: None,
            action_error: None,
            running: 0,
            fetching: None,
            origin_issues: HashMap::new(),
            estimate: None,
            estimate_gen: 0,
            saves: SaveState::default(),
            last_shown: None,
            last_auto_reload: None,
            effects: Effects::default(),
            seq: 0,
            loaded_ids: 0,
            model: None,
            model_inputs: None,
            model_gen: 0,
            model_requested: 0,
            model_done: 0,
            model_quiet: false,
            midnight: None,
            midnight_for: None,
            _minute: minute,
            section: Section::Stats,
            menu_open: false,
            menu_anchor: AnchorSlot::default(),
            menu_focus: cx.focus_handle(),
            branch_dd: Dropdown::new(cx),
            focus: cx.focus_handle(),
            sidebar: None,
            stats_ui: Default::default(),
            activity: Default::default(),
            detail: None,
            detail_ui: Default::default(),
            scroll: ScrollHandle::new(),
            leaderboard_ui: super::leaderboard::LeaderboardUi::new(cx),
            identities_ui: None,
            _subscriptions: subscriptions,
            #[cfg(test)]
            manual: false,
            #[cfg(test)]
            hold_fetch: false,
            #[cfg(test)]
            held: Vec::new(),
        }
    }

    /// The chart type every chart of every page shows.
    pub(super) fn chart_kind(&self, cx: &gpui::App) -> super::chart::ChartKind {
        self.chart_prefs.read(cx).kind
    }

    /// Whether the project is a remote one: no local history to read.
    pub(crate) fn is_remote(&self) -> bool {
        self.cwd.is_empty()
    }

    #[allow(dead_code)] // Used by the tabs (6.8 to 6.12) and the tests.
    /// The project path this view shows.
    pub(crate) fn project(&self) -> &str {
        &self.project
    }

    #[allow(dead_code)] // Used by the tabs (6.8 to 6.12) and the tests.
    pub(crate) fn section(&self) -> Section {
        self.section
    }

    pub(crate) fn set_section(&mut self, section: Section, cx: &mut Context<Self>) {
        if self.section != section {
            self.leave(cx);
            self.drop_detail(cx);
            self.section = section;
            // The formula sidebar belongs to the Leaderboard section.
            self.sidebar = None;
            cx.notify();
        }
    }

    /// The view is hidden or its section changes: an open palette goes.
    pub(crate) fn leave(&mut self, cx: &mut Context<Self>) {
        if let Some(chart) = self.stats_ui.chart.clone() {
            chart.update(cx, |chart, cx| chart.dismiss_palette(cx));
        }
    }

    /// Escape: closes the topmost thing the view opened (the formula sidebar,
    /// else the detail page); whether there was one. The page calls this too, so
    /// the rule holds wherever the focus is. A palette stops its own Escape first.
    pub(crate) fn close_top_layer(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.sidebar.is_some() {
            self.set_sidebar(None, cx);
            window.focus(&self.leaderboard_ui.how_focus, cx);
            true
        } else if self.detail.is_some() {
            self.close_detail(window, cx);
            true
        } else {
            false
        }
    }

    /// The config file is unreadable: every write is blocked.
    pub(crate) fn locked(&self, cx: &gpui::App) -> bool {
        self.snapshot(cx).config_error.is_some()
    }

    /// Saves a changed config for a tab. A refused or failed save is already
    /// shown (the lock banner or the save alert), so the tabs need not handle
    /// the rejection themselves.
    pub(crate) fn save_config_quietly(
        &mut self,
        next: monocode_core::contributors::model::types::ContributorsConfig,
        cx: &mut Context<Self>,
    ) {
        let _ = self.save_config(next, cx);
    }

    /// Reports the scope of an Ask button. Ignored until the data is on
    /// screen: a click before it arrives must not open the dialog later.
    pub(crate) fn open_ask(&mut self, scope: SnapshotScope, cx: &mut Context<Self>) -> bool {
        if !self.ask_ready(cx) {
            return false;
        }
        cx.emit(GithubEvent::Ask(scope));
        true
    }

    pub(crate) fn ask_ready(&self, cx: &gpui::App) -> bool {
        let snap = self.snapshot(cx);
        snap.status == Status::Ready && snap.model.is_some()
    }

    /// The model on screen, and the clock the snapshot is stamped with (Ask Agent
    /// builds its request from them when "Start session" is pressed).
    pub(crate) fn ask_inputs(&self) -> Option<(Arc<ContributorsModel>, DateTime<Local>)> {
        Some((self.model.clone()?, (self.clock)()))
    }

    /// Puts a view over the right edge of the body (the Leaderboard's formula
    /// sidebar), or takes it away with `None`.
    pub(crate) fn set_sidebar(&mut self, view: Option<AnyView>, cx: &mut Context<Self>) {
        self.sidebar = view;
        cx.notify();
    }

    #[cfg(test)]
    pub(super) fn hold_backend_calls(&mut self) {
        self.manual = true;
    }

    #[cfg(test)]
    pub(super) fn hold_fetch_answers(&mut self) {
        self.manual = true;
        self.hold_fetch = true;
    }

    /// Test hook: the labels of the calls held back, in the order they were made.
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

    /// Test hook: the oldest held call with this label.
    #[cfg(test)]
    pub(super) fn release(&mut self, label: &str, cx: &mut Context<Self>) {
        self.release_nth(label, 0, cx);
    }

    /// Test hook: runs every held call in order until none is left.
    #[cfg(test)]
    pub(super) fn release_all(&mut self, cx: &mut Context<Self>) {
        while !self.held.is_empty() {
            let mut held = self.held.remove(0);
            if let Some(run) = held.run.take() {
                run(self, cx);
            }
        }
    }

    // ---- rendering ------------------------------------------------------

    fn toolbar(&mut self, snap: &Snapshot, cx: &mut Context<Self>) -> AnyElement {
        let busy = snap.status == Status::Loading || snap.refreshing;
        let ask_ready = snap.status == Status::Ready && snap.model.is_some();
        let mut branches: Vec<String> = snap.branches.clone();
        if !snap.branch.is_empty() && !branches.contains(&snap.branch) {
            branches.push(snap.branch.clone());
        }
        let selected_branch = branches.iter().position(|b| *b == snap.branch).unwrap_or(0);
        let period_kind = snap.period.kind;
        let steps = !matches!(period_kind, PeriodKind::Days7 | PeriodKind::All);
        let period_options: Vec<(&'static str, &'static str)> = PERIOD_KINDS
            .iter()
            .map(|(kind, label)| (kind.as_str(), *label))
            .collect();
        div()
            .mb_4()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(10.))
            .child(
                div()
                    .mr_auto()
                    .flex()
                    .items_baseline()
                    .child(
                        div()
                            .text_size(px(20.))
                            .line_height(px(25.))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child("Contributors"),
                    )
                    .child(
                        div()
                            .ml_2()
                            .text_size(px(13.))
                            .text_color(content(0.45))
                            .child(monocode_core::search::project_name(&self.project)),
                    ),
            )
            .when(!branches.is_empty(), |d| {
                d.child(ui::select_trigger(
                    "branch",
                    "Branch",
                    snap.branch.clone().into(),
                    &self.branch_dd,
                    cx,
                    |this: &mut Self| &mut this.branch_dd,
                    selected_branch,
                ))
            })
            .child(ui::segmented(
                "period",
                "Period",
                period_kind.as_str(),
                &period_options,
                cx,
                |this: &mut Self, value, cx| {
                    if let Some(kind) = PeriodKind::parse(value) {
                        this.set_kind(kind, cx);
                    }
                },
            ))
            .when(steps, |d| {
                d.child(
                    div()
                        .id("contributors-period-nav")
                        .role(Role::Group)
                        .aria_label("Period navigation")
                        .flex()
                        .items_center()
                        .gap(px(2.))
                        .text_size(px(12.))
                        .child(
                            ui::icon_button(
                                "contributors-previous",
                                "Previous period",
                                "chevron-left",
                                14.,
                                false,
                            )
                            .on_click(cx.listener(|this, _, _, cx| this.go_previous(cx))),
                        )
                        .child(
                            div()
                                .debug_selector(|| "contributors-period-label".into())
                                .min_w(px(72.))
                                .text_center()
                                .child(snap.period.label.clone()),
                        )
                        .child(
                            ui::icon_button(
                                "contributors-next",
                                "Next period",
                                "chevron-right",
                                14.,
                                !snap.has_next,
                            )
                            .when(snap.has_next, |d| {
                                d.on_click(cx.listener(|this, _, _, cx| this.go_next(cx)))
                            }),
                        ),
                )
            })
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .text_size(px(12.))
                    .text_color(content(0.6))
                    .child("Merge identities")
                    .child(ui::toggle(
                        "merge",
                        "Merge identities",
                        snap.merge,
                        cx,
                        |this: &mut Self, on, cx| this.set_merge(on, cx),
                    )),
            )
            .child(
                ui::icon_button_with(
                    "contributors-refresh",
                    "Refresh",
                    if snap.refreshing {
                        crate::right_panel::spinner("contributors-refreshing", "loader", 14.)
                    } else {
                        icon("refresh-cw", 14.)
                            .text_color(content(if busy { 0.35 } else { 0.55 }))
                            .into_any_element()
                    },
                    busy,
                )
                .aria_description("Reload synced data and check origin for new commits")
                .when(!busy, |d| {
                    d.on_click(cx.listener(|this, _, _, cx| this.refresh(true, cx)))
                }),
            )
            .child(
                div().relative().child(self.menu_anchor.probe()).child(
                    ui::icon_button("contributors-more", "More", "more-horizontal", 14., false)
                        .aria_expanded(self.menu_open)
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.menu_open = !this.menu_open;
                            if this.menu_open {
                                window.focus(&this.menu_focus, cx);
                            }
                            cx.notify();
                        })),
                ),
            )
            .child(
                div()
                    .id("contributors-ask")
                    .debug_selector(|| "contributors-ask".into())
                    .role(Role::Button)
                    .aria_label("Ask Agent")
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .rounded_md()
                    .bg(accent_color())
                    .px(px(12.))
                    .py(px(4.))
                    .text_size(px(12.))
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .text_color(rgb(0xffffff))
                    .child(icon("sparkles", 14.).text_color(rgb(0xffffff)))
                    .child("Ask Agent")
                    .map(|d| {
                        if ask_ready {
                            d.cursor_pointer()
                                .hover(|s| s.opacity(0.9))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.open_ask(SnapshotScope::Team, cx);
                                }))
                        } else {
                            d.opacity(0.4)
                        }
                    }),
            )
            .into_any_element()
    }

    /// The red banners above the body.
    fn alerts(&mut self, snap: &Snapshot, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let mut out = Vec::new();
        if let Some(error) = &snap.save_error {
            out.push(
                ui::alert(
                    "contributors-save-error",
                    format!("Could not save {CONFIG_PATH}: {error}"),
                    vec![],
                )
                .into_any_element(),
            );
        }
        if let Some(error) = &snap.job_error {
            out.push(
                ui::alert(
                    "contributors-job-error",
                    format!("The sync stopped: {error}"),
                    vec![self.dismiss_button(cx)],
                )
                .into_any_element(),
            );
        }
        if let Some(error) = &snap.action_error {
            out.push(
                ui::alert("contributors-action-error", error.clone(), vec![]).into_any_element(),
            );
        }
        out
    }

    fn dismiss_button(&self, cx: &mut Context<Self>) -> AnyElement {
        div()
            .id("contributors-dismiss")
            .debug_selector(|| "contributors-dismiss".into())
            .role(Role::Button)
            .aria_label("Dismiss")
            .mt(px(2.))
            .flex_shrink_0()
            .cursor_pointer()
            .child(icon("x", 14.).text_color(ui::red(0.7)))
            .hover(|s| s.opacity(0.8))
            .on_click(cx.listener(|this, _, _, cx| this.dismiss_job_error(cx)))
            .into_any_element()
    }

    /// An alert with a Retry-style button.
    fn retry_alert(
        &self,
        id: &'static str,
        text: String,
        action: &'static str,
        disabled: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let button = crate::settings::widgets::secondary_button(
            SharedString::from(format!("{id}-action")),
            action,
            false,
            disabled,
        )
        .debug_selector(move || format!("{id}-action"))
        .child(icon("refresh-cw", 12.).text_color(content(0.7)))
        .child(action)
        .when(!disabled, |d| {
            d.on_click(cx.listener(|this, _, _, cx| this.refresh(true, cx)))
        })
        .into_any_element();
        ui::alert(id, text, vec![button]).into_any_element()
    }

    fn loading(&self) -> AnyElement {
        div()
            .id("contributors-loading")
            .debug_selector(|| "contributors-loading".into())
            .role(Role::Status)
            .py(px(40.))
            .flex()
            .items_center()
            .gap_2()
            .text_size(px(12.))
            .text_color(content(0.5))
            .child(crate::right_panel::spinner(
                "contributors-loading-spinner",
                "loader",
                16.,
            ))
            .child("Loading synced data...")
            .into_any_element()
    }

    fn section_tabs(&mut self, model: &ContributorsModel, cx: &mut Context<Self>) -> AnyElement {
        let unmapped = model.unmapped.len();
        div()
            .id("contributors-sections")
            .role(Role::TabList)
            .aria_label("Contributors sections")
            .mb_4()
            .flex()
            .gap(px(4.))
            .border_b_1()
            .border_color(content(0.1))
            .children(Section::ALL.into_iter().map(|section| {
                let selected = self.section == section;
                div()
                    .id(SharedString::from(format!(
                        "contributors-section-{}",
                        section.id()
                    )))
                    .debug_selector(move || format!("contributors-section-{}", section.id()))
                    .role(Role::Tab)
                    .aria_label(section.label())
                    .aria_selected(selected)
                    .relative()
                    .h(px(36.))
                    .px(px(12.))
                    .flex()
                    .items_center()
                    .text_size(px(12.))
                    .cursor_pointer()
                    .text_color(content(if selected { 1. } else { 0.5 }))
                    .hover(|s| s.text_color(content(1.)))
                    .child(section.label())
                    .when(section == Section::Identities && unmapped > 0, |d| {
                        d.child(
                            div()
                                .ml(px(6.))
                                .rounded_full()
                                .bg(content(0.1))
                                .px(px(6.))
                                .py(px(2.))
                                .text_size(px(10.))
                                .text_color(content(0.6))
                                .child(format!("{unmapped} unmapped")),
                        )
                    })
                    .when(selected, |d| {
                        d.child(
                            div()
                                .absolute()
                                .left_0()
                                .right_0()
                                .bottom_0()
                                .h(px(2.))
                                .bg(content(1.)),
                        )
                    })
                    .on_click(cx.listener(move |this, _, _, cx| this.set_section(section, cx)))
            }))
            .into_any_element()
    }

    fn no_commits(&self) -> AnyElement {
        div()
            .debug_selector(|| "contributors-no-commits".into())
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
                    .child("No commits in this period"),
            )
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(content(0.5))
                    .child("Try another period or branch."),
            )
            .into_any_element()
    }

    fn remote_panel(&self) -> AnyElement {
        div()
            .debug_selector(|| "contributors-remote".into())
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
                    .child("Contributors works on local projects only"),
            )
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(content(0.5))
                    .child("Open this project on this computer to see its history."),
            )
            .into_any_element()
    }

    /// Everything under the toolbar.
    fn body(&mut self, snap: &Snapshot, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let mut out = self.alerts(snap, cx);
        // Stored data stays on screen; this only says it may be behind.
        if snap.status == Status::Ready
            && let Some(note) = &snap.origin_note
        {
            out.push(
                div()
                    .id("contributors-origin-note")
                    .debug_selector(|| "contributors-origin-note".into())
                    .role(Role::Status)
                    .mb_4()
                    .text_size(px(12.))
                    .text_color(content(0.5))
                    .child(note.clone())
                    .into_any_element(),
            );
        }
        let locked = snap.config_error.is_some();
        match (&snap.status, &snap.model) {
            (Status::Loading, _) => out.push(self.loading()),
            (Status::Error, _) => out.push(
                self.retry_alert(
                    "contributors-load-error",
                    snap.error
                        .clone()
                        .unwrap_or_else(|| "Could not load contributors.".into()),
                    "Retry",
                    snap.refreshing,
                    cx,
                ),
            ),
            (Status::Ready, None) if snap.sync_state == SyncState::NotSynced => {
                out.push(self.not_synced_panel(snap, cx))
            }
            // The data is in; its model is still being built.
            (Status::Ready, None) => out.push(self.loading()),
            (Status::Ready, Some(model)) => {
                let model = model.clone();
                if let Some(error) = &snap.error {
                    out.push(self.retry_alert(
                        "contributors-refresh-error",
                        format!("Could not refresh: {error}"),
                        "Retry",
                        snap.refreshing,
                        cx,
                    ));
                }
                if locked {
                    out.push(self.retry_alert(
                        "contributors-locked",
                        format!(
                            "{CONFIG_PATH} could not be read ({}). Fix the file in your repo, then reload. Editing is disabled until then.",
                            snap.config_error.clone().unwrap_or_default()
                        ),
                        "Reload",
                        snap.refreshing,
                        cx,
                    ));
                }
                if !snap.stale {
                    out.push(self.synced_bar(snap, cx));
                }
                let note = model.raw.gh_note.as_deref().map(str::trim).unwrap_or("");
                if snap.gh_incomplete || !model.raw.gh_available || !note.is_empty() {
                    out.push(self.gh_notice(snap, &model, cx));
                }
                if model.raw.truncated {
                    out.push(
                        div()
                            .debug_selector(|| "contributors-truncated".into())
                            .mb_4()
                            .text_size(px(12.))
                            .text_color(if is_light() {
                                rgb(0xb45309).into()
                            } else {
                                ui::amber(1.)
                            })
                            .child(
                                "The history was too large to read in full, so some numbers may be partial.",
                            )
                            .into_any_element(),
                    );
                }
                // A detail page stands where the whole section was: no tabs over it.
                let detailed = self.detail.is_some() && self.section == Section::Stats;
                if !detailed {
                    out.push(self.section_tabs(&model, cx));
                }
                let body = match self.section {
                    Section::Stats if detailed => self.detail_body(&model, cx),
                    Section::Stats if model.has_activity => self.stats_body(&model, cx),
                    Section::Stats => self.no_commits(),
                    Section::Leaderboard => self.leaderboard_body(&model, cx),
                    Section::Identities => self.identities_body(&model, cx),
                };
                let dim = snap.stale || snap.model_pending;
                out.push(
                    div()
                        .id("contributors-panel")
                        .debug_selector(|| "contributors-panel".into())
                        .role(Role::TabPanel)
                        .when(dim, |d| d.opacity(0.6))
                        .child(body)
                        .into_any_element(),
                );
            }
        }
        out
    }
}

impl Render for GithubView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.a11y = window.is_a11y_active();
        let snap = self.snapshot(cx);
        let content_column = if self.is_remote() {
            vec![self.remote_panel()]
        } else {
            let mut column = vec![self.toolbar(&snap, cx)];
            column.extend(self.body(&snap, cx));
            column
        };
        let branch_list = (self.branch_dd.open && !self.is_remote()).then(|| {
            let mut names = snap.branches.clone();
            if !snap.branch.is_empty() && !names.contains(&snap.branch) {
                names.push(snap.branch.clone());
            }
            ui::select_list(
                "branch",
                "Branch",
                &snap.branch,
                names
                    .into_iter()
                    .map(|name| (name.clone(), SharedString::from(name)))
                    .collect(),
                &self.branch_dd,
                cx,
                |this: &mut Self| &mut this.branch_dd,
                self.focus.clone(),
                |this, value, _, cx| this.set_branch(Some(value), cx),
            )
        });
        let menu = self.menu_open.then(|| self.options_menu(cx));
        div()
            .id("contributors-github")
            .debug_selector(|| "contributors-github".into())
            .role(Role::Region)
            .aria_label("Contributors")
            .track_focus(&self.focus)
            // The formula sidebar closes on any Escape, wherever the focus is
            // (a click in the body moves it to this view), before the page sees it.
            .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, window, cx| {
                if event.keystroke.key != "escape" {
                    return;
                }
                if this.close_top_layer(window, cx) {
                    cx.stop_propagation();
                }
            }))
            .relative()
            .flex_1()
            .min_h_0()
            .min_w_0()
            .flex()
            .flex_col()
            .text_color(content(1.))
            .child(
                div()
                    .id("contributors-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .child(
                        div()
                            .mx_auto()
                            .w_full()
                            .max_w(px(1024.))
                            .px(px(32.))
                            .pt(px(24.))
                            .pb(px(64.))
                            .children(content_column),
                    ),
            )
            .when_some(self.sidebar.clone(), |d, sidebar| {
                d.child(
                    div()
                        .debug_selector(|| "contributors-sidebar-host".into())
                        .absolute()
                        .top_0()
                        .bottom_0()
                        .right_0()
                        .w(px(400.))
                        .max_w_full()
                        .child(sidebar),
                )
            })
            .children(branch_list)
            .children(menu)
    }
}

impl GithubView {
    /// The More menu: one item, Clear synced data.
    fn options_menu(&mut self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let weak = cx.weak_entity();
        let dismiss_focus = self.focus.clone();
        let key_focus = self.focus.clone();
        popover(
            "contributors-options",
            Anchor::Slot(self.menu_anchor.clone()),
        )
        .side(Side::Bottom)
        .align(Align::End)
        .width(200.)
        .padding(4.)
        .on_dismiss(move |window, cx| {
            let _ = weak.update(cx, |this, cx| {
                this.menu_open = false;
                window.focus(&dismiss_focus, cx);
                cx.notify();
            });
        })
        .child(
            div()
                .id("contributors-options-list")
                .role(Role::Menu)
                .aria_label("Contributors options")
                .track_focus(&self.menu_focus)
                .on_key_down(
                    cx.listener(move |this, event: &gpui::KeyDownEvent, window, cx| {
                        if event.keystroke.key == "escape" {
                            this.menu_open = false;
                            window.focus(&key_focus, cx);
                            cx.stop_propagation();
                            cx.notify();
                        }
                    }),
                )
                .child(
                    div()
                        .id("contributors-clear")
                        .debug_selector(|| "contributors-clear".into())
                        .role(Role::MenuItem)
                        .aria_label("Clear synced data")
                        .flex()
                        .items_center()
                        .gap(px(10.))
                        .rounded_lg()
                        .px(px(10.))
                        .py(px(6.))
                        .text_size(px(13.))
                        .text_color(content(0.85))
                        .cursor_pointer()
                        .hover(|s| s.bg(content(0.08)))
                        .child(icon("trash", 14.).text_color(content(0.85)))
                        .child("Clear synced data")
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.menu_open = false;
                            this.confirm_clear(window, cx);
                            cx.notify();
                        })),
                ),
        )
        .into_any_element()
    }

    /// A native sheet asks first.
    pub(crate) fn confirm_clear(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let prompt = window.prompt(
            gpui::PromptLevel::Warning,
            "Delete the synced Contributors data of this repository?",
            Some("You can sync it again at any time."),
            &["Delete", "Cancel"],
            cx,
        );
        cx.spawn(async move |this, cx| {
            if prompt.await == Ok(0) {
                let _ = this.update(cx, |this, cx| this.clear_synced_data(cx));
            }
        })
        .detach();
    }
}
