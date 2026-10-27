//! The Workflows tab of a Mono's overview and its pages: the list, one workflow, its review
//! before it may run, and the import page. Port of `MonoWorkflows.tsx`, `WorkflowPage.tsx`,
//! `WorkflowReviewPage.tsx` and `ImportWorkflowPage.tsx`.
//!
//! The Mono's workflow files are the source of truth and there is no change notification, so the
//! UI reads them again whenever something may have changed: when a page or the tab opens, after
//! every action, when the app's own `workflows.*` actions saved (`reload_files`), and while the
//! tab or a workflow page is shown, on a short clock that also notices a run starting or ending
//! in the runner. All file access and model probing runs off the UI thread. Actions go through
//! `actions`, which follows the same rules as the `workflows.*` CLI actions.
mod actions;
mod board;
#[cfg(test)]
mod board_canvas_tests;
#[cfg(test)]
mod board_tests;
mod board_ui;
mod graph;
mod import;
#[cfg(test)]
mod import_tests;
mod inspector;
mod inspector_parts;
#[cfg(test)]
mod inspector_tests;
mod list;
mod page;
#[cfg(test)]
mod page_tests;
mod review;
#[cfg(test)]
mod review_tests;
mod run_details;
mod run_model;
#[cfg(test)]
mod run_model_tests;
mod run_page;
#[cfg(test)]
mod run_tests;
#[cfg(test)]
mod tests;
mod view;

pub(crate) use actions::ReviewIntent;

use super::*;
use crate::popover::{Align, MenuAction};
use monocode_core::mono::workflows::{
    cli::StoreHost,
    model::{Workflow, WorkflowRun},
    store::{MonoDir, load_runs, load_workflows},
};
use std::collections::BTreeSet;

/// How often the tab looks again while it is shown.
const POLL: Duration = Duration::from_secs(2);
/// A look at the files with nothing running happens every this many polls (a schedule moves
/// `nextRunAt` along without any event).
const IDLE_POLLS: u32 = 15;

/// What a picker on a workflow page chooses for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Pick {
    /// An input of the stored workflow.
    Input(String, String),
    /// An input of the file being imported, by name and declared type.
    Import(String, String),
    /// A drop-down of the board editor.
    Board(board::BoardPick),
}

#[derive(Default)]
pub(super) struct WorkflowsUi {
    /// Each Mono's workflows as last loaded, so switching back shows them at once.
    loaded: HashMap<String, Vec<Workflow>>,
    /// The runs of the workflows that need them (the open page), by `(mono, workflow)`.
    runs: HashMap<(String, String), Vec<WorkflowRun>>,
    requested: HashSet<String>,
    revision: u64,
    /// What a refused action says under the list, and under a page's header.
    notice: Option<String>,
    page_notice: Option<String>,
    poll: Option<gpui::Task<()>>,
    seen_running: Vec<String>,
    /// When the tab or a page was last drawn: the clock stops once nothing draws it.
    last_drawn: Option<std::time::Instant>,
    ticks: u32,
    prompts_closed: bool,
    review: Option<review::Review>,
    import: import::Import,
    /// The text an input field on the workflow page was last given from the stored value.
    seeds: HashMap<String, String>,
    focus: HashMap<String, gpui::FocusHandle>,
    pick: Option<Pick>,
    /// The open run page.
    run: run_page::RunPage,
    /// The open board editor.
    board: Option<board::BoardState>,
    /// How many times a run page built its state (a page sliding away must not add to it).
    #[cfg(test)]
    run_opens: u32,
}

impl Monos {
    /// The active Mono's workflows, once loaded.
    pub(super) fn mono_workflows(&self) -> Option<&Vec<Workflow>> {
        self.active.as_ref().and_then(|id| self.wf.loaded.get(id))
    }

    pub(super) fn mono_workflow(&self, id: &str) -> Option<&Workflow> {
        self.mono_workflows()?.iter().find(|w| w.id == id)
    }

    pub(super) fn workflow_runs_of(&self, id: &str) -> Option<&Vec<WorkflowRun>> {
        self.wf.runs.get(&(self.active.clone()?, id.to_string()))
    }

    /// Whether the Mono's workflows were asked for already.
    pub(super) fn workflows_requested(&self, mono: &str) -> bool {
        self.wf.requested.contains(mono)
    }

    /// How many workflows the active Mono has, once known.
    pub(super) fn workflow_count(&self) -> Option<usize> {
        self.mono_workflows().map(Vec::len)
    }

    /// The Mono's workflow files, with how this app probes the model catalog.
    pub(super) fn workflow_store(&self, mono: &str) -> StoreHost {
        StoreHost::new(
            MonoDir::new(&self.root, mono),
            self.workflow_signals.clone(),
        )
        .with_probe(self.workflow_live.probe())
    }

    /// Whether the tab or a workflow page is on screen.
    fn workflows_shown(&self) -> bool {
        self.active.is_some()
            && (self.overview_tab == OverviewTab::Workflows
                || matches!(
                    self.route,
                    Some(
                        Route::Workflow(_)
                            | Route::WorkflowReview(..)
                            | Route::WorkflowRun(..)
                            | Route::WorkflowBoard(_)
                            | Route::NewWorkflow
                            | Route::ImportWorkflow
                    )
                ))
    }

    /// Called while the tab or a page is drawn: loads on first sight and keeps looking.
    pub(super) fn ensure_workflows(&mut self, cx: &mut Context<Self>) {
        let Some(mono) = self.active.clone() else {
            return;
        };
        self.wf.last_drawn = Some(std::time::Instant::now());
        // First sight, or the tab shown again after the clock stopped: look at once.
        let first = self.wf.requested.insert(mono);
        if first || self.wf.poll.is_none() {
            self.refresh_workflows(cx);
        }
        if self.wf.poll.is_some() {
            return;
        }
        self.wf.poll = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(POLL).await;
                if this.update(cx, |this, cx| this.workflows_poll(cx)).ok() != Some(true) {
                    break;
                }
            }
        }));
    }

    /// One look of the clock: false once nothing shows workflows any more.
    fn workflows_poll(&mut self, cx: &mut Context<Self>) -> bool {
        // Not drawn for a beat or two (the panel is hidden): stop reading files. Drawing it
        // again starts the clock and reads at once.
        let hidden = self
            .wf
            .last_drawn
            .is_some_and(|at| at.elapsed() > POLL * 2 + Duration::from_millis(500));
        if !self.workflows_shown() || hidden {
            if let Some(poll) = self.wf.poll.take() {
                poll.detach();
            }
            return false;
        }
        // A look asks for a draw, which is how a hidden panel is told from a still one.
        cx.notify();
        // Only this Mono's runs count: another Mono's run changes nothing shown here.
        let running: Vec<String> = self
            .workflows
            .running()
            .into_iter()
            .map(|(workflow, _)| workflow)
            .filter(|workflow| self.mono_workflow(workflow).is_some())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        let changed = running != self.wf.seen_running;
        self.wf.seen_running = running;
        self.wf.ticks = self.wf.ticks.wrapping_add(1);
        let going = !self.wf.seen_running.is_empty();
        if changed || going || self.wf.ticks.is_multiple_of(IDLE_POLLS) {
            self.refresh_workflows(cx);
        }
        if changed || going {
            cx.notify();
        }
        true
    }

    /// Reads the active Mono's workflow files again, off the UI thread.
    pub(super) fn refresh_workflows(&mut self, cx: &mut Context<Self>) {
        let Some(mono) = self.active.clone() else {
            return;
        };
        self.wf.revision += 1;
        let revision = self.wf.revision;
        let dir = MonoDir::new(&self.root, mono.clone());
        // The open page also shows the workflow's recent runs.
        // A run the runner holds is shown from there, not from its file.
        let wanted = match &self.route {
            Some(Route::WorkflowRun(id, run))
                if self
                    .workflows
                    .with_run(id, |r| r.id == *run)
                    .unwrap_or(false) =>
            {
                None
            }
            Some(Route::Workflow(id) | Route::WorkflowRun(id, _)) => Some(id.clone()),
            _ => None,
        };
        let work = cx.background_executor().spawn(async move {
            let workflows = load_workflows(&dir)?.workflows;
            let runs = wanted
                .filter(|id| workflows.iter().any(|w| &w.id == id))
                .and_then(|id| load_runs(&dir, &id).ok().map(|runs| (id, runs)));
            Ok::<_, String>((workflows, runs))
        });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |this, cx| {
                if this.wf.revision != revision {
                    return;
                }
                match result {
                    Ok((workflows, runs)) => this.workflows_loaded(&mono, workflows, runs, cx),
                    Err(error) => eprintln!("Could not read the workflows: {error}"),
                }
            });
        })
        .detach();
    }

    fn workflows_loaded(
        &mut self,
        mono: &str,
        workflows: Vec<Workflow>,
        runs: Option<(String, Vec<WorkflowRun>)>,
        cx: &mut Context<Self>,
    ) {
        let count_changed = self.wf.loaded.get(mono).map(Vec::len) != Some(workflows.len());
        self.wf.runs.retain(|(m, _), _| m != mono);
        if let Some((id, runs)) = runs {
            self.wf.runs.insert((mono.to_string(), id), runs);
        }
        self.wf.loaded.insert(mono.to_string(), workflows);
        if self.active.as_deref() == Some(mono) {
            // A workflow removed while open closes its pages.
            let gone = match &self.route {
                Some(
                    Route::Workflow(id)
                    | Route::WorkflowRun(id, _)
                    | Route::WorkflowBoard(id)
                    | Route::WorkflowReview(id, _),
                ) => self.mono_workflow(id).is_none(),
                _ => false,
            };
            if gone {
                self.route = None;
            }
            // A review that is no longer on screen is dropped, not probed again.
            if !matches!(self.route, Some(Route::WorkflowReview(..))) {
                self.wf.review = None;
            }
            self.review_sync(cx);
            if matches!(self.route, Some(Route::WorkflowRun(..))) {
                self.run_page_sync(cx);
            }
            if matches!(
                self.route,
                Some(Route::WorkflowBoard(_) | Route::NewWorkflow)
            ) {
                self.board_sync(cx);
            }
        }
        if count_changed {
            self.changed(cx);
        } else {
            cx.notify();
        }
    }

    /// Runs `work` over the Mono's workflow files off the UI thread, then `done` here, and reads
    /// the files again.
    pub(super) fn workflow_task<T: Send + 'static>(
        &mut self,
        work: impl FnOnce(&mut StoreHost) -> T + Send + 'static,
        done: impl FnOnce(&mut Self, T, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) {
        let Some(mono) = self.active.clone() else {
            return;
        };
        let mut host = self.workflow_store(&mono);
        let task = cx
            .background_executor()
            .spawn(async move { work(&mut host) });
        cx.spawn(async move |this, cx| {
            let out = task.await;
            let _ = this.update(cx, |this, cx| {
                done(this, out, cx);
                this.refresh_workflows(cx);
                cx.notify();
            });
        })
        .detach();
    }

    /// Shows what a refused action says, or clears it when the action went through.
    pub(super) fn workflow_said(
        &mut self,
        notice: Option<String>,
        page: bool,
        cx: &mut Context<Self>,
    ) {
        if page {
            self.wf.page_notice = notice;
        } else {
            self.wf.notice = notice;
        }
        cx.notify();
    }

    /// The tab is left: its notice goes (React resets it on unmount).
    pub(super) fn wf_leave_tab(&mut self) {
        self.wf.notice = None;
    }

    /// A page is closed: its notice and an open review go.
    pub(super) fn wf_leave_page(&mut self) {
        self.wf.page_notice = None;
        self.wf.run.session_back = None;
        self.board_drop();
        if matches!(self.route, Some(Route::WorkflowReview(..))) {
            self.wf.review = None;
        }
    }

    /// Opens a workflow page and reads its files again.
    pub(super) fn open_workflow_route(&mut self, route: Route, cx: &mut Context<Self>) {
        self.wf.page_notice = None;
        // A run page opened from a link starts fresh (a session's Back returns to the old one).
        if matches!(route, Route::WorkflowRun(..)) {
            self.wf.run.key = None;
        }
        // A board opens fresh from the files.
        if matches!(route, Route::WorkflowBoard(_) | Route::NewWorkflow) {
            self.board_drop();
        }
        self.route = Some(route);
        self.refresh_workflows(cx);
        cx.notify();
    }

    /// The `wf-` fields belong to the workflow pages; they go when a page is opened fresh.
    pub(super) fn clear_workflow_fields(&mut self) {
        self.fields.retain(|key, _| !key.starts_with("wf-"));
        self.wf.seeds.clear();
    }

    /// "Ask Mono to build a workflow": starts a sentence in the Mono's composer for the user to
    /// finish. It never sends.
    pub(super) fn ask_mono_for_workflow(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(chat) = self.chat() else {
            return;
        };
        let input = chat.read(cx).input.clone();
        if input.read(cx).is_composing() {
            return;
        }
        input.update(cx, |input, cx| {
            let draft = input.content.to_string();
            let next = import::append_prefill(&draft, ASK_MONO_WORKFLOW_DRAFT);
            if next != draft {
                input.set_text(next, cx);
            }
        });
        window.focus(&input.focus_handle(cx), cx);
        cx.notify();
    }

    fn workflow_focus(&mut self, key: &str, cx: &mut Context<Self>) -> gpui::FocusHandle {
        self.wf
            .focus
            .entry(key.to_string())
            .or_insert_with(|| cx.focus_handle())
            .clone()
    }

    /// A drop-down of the workflow pages (a project, on or off): the chosen value goes to
    /// `pick` when the menu answers.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn workflow_picker(
        &mut self,
        id: String,
        label: String,
        selected: String,
        choices: Vec<(String, String)>,
        current: String,
        pick: Pick,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let anchor = AnchorSlot::default();
        let focus = self.workflow_focus(&id, cx);
        let keyboard = (
            label.clone(),
            choices.clone(),
            current.clone(),
            pick.clone(),
            anchor.clone(),
        );
        let expanded = self.picker.is_some() && self.wf.pick.as_ref() == Some(&pick);
        let element_id: SharedString = id.clone().into();
        let selector = id.clone();
        let (open_label, open_choices, open_current, open_pick, open_anchor) =
            (label.clone(), choices, current, pick, anchor.clone());
        div()
            .id(element_id)
            .debug_selector(move || selector.clone())
            .role(Role::ComboBox)
            .aria_label(format!("{label}: {selected}"))
            .aria_expanded(expanded)
            .track_focus(&focus)
            .tab_index(0)
            .relative()
            .h(px(28.))
            .max_w_full()
            .flex_shrink_0()
            .px_2()
            .rounded_md()
            .bg(content(0.1))
            .hover(|s| s.bg(content(0.14)))
            .flex()
            .items_center()
            .gap_1()
            .text_size(px(12.))
            .line_height(px(16.))
            .child(anchor.probe())
            .child(div().min_w_0().truncate().child(selected))
            .child(icon("chevron-down", 12.))
            .on_key_down(
                cx.listener(move |this, event: &gpui::KeyDownEvent, window, cx| {
                    if matches!(
                        event.keystroke.key.as_str(),
                        "enter" | "space" | "down" | "up"
                    ) {
                        let (label, choices, current, pick, anchor) = keyboard.clone();
                        this.open_workflow_picker(
                            label, choices, current, pick, anchor, window, cx,
                        );
                        window.prevent_default();
                        cx.stop_propagation();
                    }
                }),
            )
            .on_click(cx.listener(move |this, _, window, cx| {
                this.open_workflow_picker(
                    open_label.clone(),
                    open_choices.clone(),
                    open_current.clone(),
                    open_pick.clone(),
                    open_anchor.clone(),
                    window,
                    cx,
                );
            }))
            .into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    fn open_workflow_picker(
        &mut self,
        label: String,
        choices: Vec<(String, String)>,
        current: String,
        pick: Pick,
        anchor: AnchorSlot,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let items = choices
            .into_iter()
            .map(|(value, text)| {
                MenuAction::new(value.clone(), text)
                    .checked(value == current)
                    .into()
            })
            .collect();
        let menu = cx.new(|cx| {
            Menu::new(label, items, window, cx)
                .width(240.)
                .select_value(&current)
        });
        let target = pick.clone();
        let picked = cx.subscribe(&menu, move |this, _, event: &MenuEvent, cx| {
            if let MenuEvent::Picked(value, _) = event {
                this.workflow_picked(&target, value.to_string(), cx);
            }
            this.picker = None;
            this.wf.pick = None;
            cx.notify();
        });
        self.wf.pick = Some(pick);
        self.picker = Some(OpenPicker {
            menu,
            anchor: Anchor::Slot(anchor),
            width: 240.,
            align: Align::Start,
            _events: vec![picked],
        });
        cx.notify();
    }

    fn workflow_picked(&mut self, pick: &Pick, value: String, cx: &mut Context<Self>) {
        match pick {
            Pick::Input(workflow, name) => self.workflow_choose_input(workflow, name, &value, cx),
            Pick::Import(name, ty) => self.import_choose(name, ty, &value, cx),
            Pick::Board(pick) => self.board_picked(pick, &value, cx),
        }
    }
}

/// What "Ask Mono to build a workflow" starts in the chat, for the user to finish.
pub(super) const ASK_MONO_WORKFLOW_DRAFT: &str = "Help me build a workflow that ";
