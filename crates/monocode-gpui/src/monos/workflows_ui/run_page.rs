//! One run up close (`WorkflowRunPage.tsx`): the workflow as it ran, node by node, drawn read-only
//! on the graph canvas, and what the picked node did (`run_details.rs`).
//!
//! The run comes from the runner while it is live and from the runs file otherwise (read off the
//! UI thread by `refresh_workflows`); `run_page_sync` takes whichever is fresher whenever either
//! may have changed. The scene is rebuilt only when the run, the view or a pick changes, never per
//! frame.
use super::super::scheduler::{duration, when};
use super::super::*;
use super::actions;
use super::graph::{GraphCanvas, GraphEvent};
use super::page::run_status;
use super::run_model::*;
use super::view::{alert, notice_line};
use gpui::Div;
use monocode_core::mono::workflows::{
    model::{RunStatus, WorkflowRun},
    run_graph_model::{
        Pick, Picks, RunView, VIEW_KEY, chosen_view, default_run_view, definition_changed,
        has_for_each,
    },
};
use monocode_core::store::Store;
use monocode_core::time::now_ms;

/// This app's own setting for the view of a workflow's runs (the React key is the default).
fn view_setting(workflow: &str) -> String {
    format!("native-workflow-run-view:{workflow}")
}

fn read_view(store: &Store, workflow: &str) -> Option<RunView> {
    let native = store.setting(&view_setting(workflow)).ok().flatten();
    let desktop = store
        .desktop_preferences()
        .and_then(|prefs| prefs.get(&format!("{VIEW_KEY}{workflow}")))
        .map(str::to_string);
    chosen_view(native.as_deref(), desktop.as_deref())
}

/// What the run page keeps while it is open.
#[derive(Default)]
pub(super) struct RunPage {
    /// The `(workflow, run)` the state belongs to.
    pub(super) key: Option<(String, String)>,
    pub(super) run: Option<WorkflowRun>,
    /// The runs file was read: a run that is not there is gone.
    pub(super) loaded: bool,
    /// The workflow was edited after the run took its copy.
    pub(super) changed: bool,
    /// The view the user chose last for this workflow, once read.
    pub(super) chosen: Option<RunView>,
    chosen_read: bool,
    pub(super) picks: Picks,
    /// The node the user picked.
    pub(super) picked: Option<String>,
    /// The node whose details show.
    pub(super) selection: Option<String>,
    pub(super) details: Option<Details>,
    pub(super) details_closed: bool,
    /// Outputs shown whole, by `<node key>:<name>`.
    pub(super) show_all: HashSet<String>,
    pub(super) canvas: Option<Entity<GraphCanvas>>,
    _events: Option<Subscription>,
    /// Where Back from an opened session goes: the session, the page and the tab it came from.
    pub(super) session_back: Option<(i64, Route)>,
}

impl RunPage {
    /// The view being drawn: the chosen one, else the one that fits the run's size.
    pub(super) fn view(&self) -> Option<RunView> {
        let run = self.run.as_ref()?;
        Some(self.chosen.unwrap_or_else(|| default_run_view(run)))
    }
}

fn meta(label: &'static str, value: impl IntoElement) -> Div {
    div()
        .flex()
        .h(px(24.))
        .items_center()
        .gap(px(6.))
        .text_size(px(12.))
        .child(div().text_color(content(0.45)).child(label))
        .child(
            div()
                .flex()
                .items_center()
                .text_color(content(0.85))
                .child(value),
        )
}

impl Monos {
    /// Opens the state for a run the first time its page is drawn.
    fn run_page_open(&mut self, mono: &Mono, workflow: &str, run: &str, cx: &mut Context<Self>) {
        let key = (workflow.to_string(), run.to_string());
        if self.wf.run.key.as_ref() == Some(&key) {
            return;
        }
        #[cfg(test)]
        {
            self.wf.run_opens += 1;
        }
        let canvas = cx.new(|_| GraphCanvas::new(mono.color));
        let events = cx.subscribe(&canvas, |this, _, event: &GraphEvent, cx| {
            this.run_graph_event(event, cx)
        });
        let session_back = self.wf.run.session_back.take();
        self.wf.run = RunPage {
            key: Some(key),
            canvas: Some(canvas),
            _events: Some(events),
            session_back,
            ..RunPage::default()
        };
        self.run_view_read(workflow, cx);
        self.run_page_sync(cx);
    }

    /// Reads the view chosen last for the workflow, off the UI thread.
    fn run_view_read(&mut self, workflow: &str, cx: &mut Context<Self>) {
        let store = self.store.clone();
        let id = workflow.to_string();
        let read = id.clone();
        let task = cx
            .background_executor()
            .spawn(async move { locked(&store).ok().and_then(|db| read_view(&db, &read)) });
        cx.spawn(async move |this, cx| {
            let chosen = task.await;
            let _ = this.update(cx, |this, cx| {
                let page = &mut this.wf.run;
                if page.key.as_ref().map(|k| &k.0) != Some(&id) {
                    return;
                }
                page.chosen_read = true;
                // A choice made before the read came back is the newer one.
                if page.chosen.is_none() {
                    page.chosen = chosen;
                }
                this.run_page_rebuild(cx);
            });
        })
        .detach();
    }

    /// Takes the run from the runner while it has it, else from the runs file, and draws it
    /// again if it differs from the one drawn.
    pub(super) fn run_page_sync(&mut self, cx: &mut Context<Self>) {
        self.run_page_sync_at(now_ms(), cx)
    }

    /// `run_page_sync` as of `now`. A run that is going is drawn again on every look even when
    /// nothing in it changed, because its labels ("Running 3m") move with the clock.
    pub(super) fn run_page_sync_at(&mut self, now: u64, cx: &mut Context<Self>) {
        let Some((workflow, run_id)) = self.wf.run.key.clone() else {
            return;
        };
        let live = self
            .workflows
            .with_run(&workflow, |r| (r.id == run_id).then(|| r.clone()))
            .flatten();
        let (next, loaded) = match (live, self.workflow_runs_of(&workflow)) {
            (Some(run), _) => (Some(run), true),
            (None, Some(runs)) => (runs.iter().find(|r| r.id == run_id).cloned(), true),
            (None, None) => (None, false),
        };
        let changed = match (&next, self.mono_workflow(&workflow)) {
            (Some(run), Some(current)) => definition_changed(run, current),
            _ => false,
        };
        let page = &mut self.wf.run;
        let same = page.run == next && page.loaded == loaded;
        page.changed = changed;
        if same {
            if page
                .run
                .as_ref()
                .is_some_and(|r| r.status == RunStatus::Running)
            {
                self.run_page_rebuild_at(now, cx);
            }
            return;
        }
        page.run = next;
        page.loaded = loaded;
        self.run_page_rebuild_at(now, cx);
        cx.notify();
    }

    /// Draws the run again: the scene, the picked node and its details.
    pub(super) fn run_page_rebuild(&mut self, cx: &mut Context<Self>) {
        self.run_page_rebuild_at(now_ms(), cx)
    }

    pub(super) fn run_page_rebuild_at(&mut self, now: u64, cx: &mut Context<Self>) {
        let page = &mut self.wf.run;
        let (Some(run), Some(view)) = (page.run.as_ref(), page.view()) else {
            page.selection = None;
            page.details = None;
            return;
        };
        page.selection = selection(run, page.picked.as_deref());
        page.details = page
            .selection
            .as_deref()
            .and_then(|key| details_of(run, key));
        // Until the remembered view is read the canvas waits, so it never flashes the other one.
        if !page.chosen_read {
            return;
        }
        let scene = run_scene(run, view, &page.picks, now);
        let selected = page.selection.clone();
        if let Some(canvas) = &page.canvas {
            canvas.update(cx, |canvas, cx| {
                canvas.set_scene(scene, cx);
                canvas.set_selected(selected, cx);
            });
        }
        cx.notify();
    }

    fn run_graph_event(&mut self, event: &GraphEvent, cx: &mut Context<Self>) {
        let page = &mut self.wf.run;
        match event {
            GraphEvent::Select(key) => page.picked = Some(key.clone()),
            GraphEvent::PickItem { group, index } => {
                page.picks.entry(group.clone()).or_default().item = Some(*index);
            }
            GraphEvent::PickRound { group, round } => {
                let pick: &mut Pick = page.picks.entry(group.clone()).or_default();
                pick.round = Some(*round);
                pick.item = None;
            }
            // The run page is read-only: the editing events never come.
            _ => return,
        }
        self.run_page_rebuild(cx);
    }

    /// Lanes side by side, or one item at a time; remembered for the workflow.
    pub(super) fn run_toggle_view(&mut self, cx: &mut Context<Self>) {
        let page = &mut self.wf.run;
        let (Some(view), Some((workflow, _))) = (page.view(), page.key.clone()) else {
            return;
        };
        let next = match view {
            RunView::Lanes => RunView::Single,
            RunView::Single => RunView::Lanes,
        };
        page.chosen = Some(next);
        let store = self.store.clone();
        cx.background_executor()
            .spawn(async move {
                let _ = locked(&store)
                    .and_then(|db| db.save_setting(&view_setting(&workflow), next.name()));
            })
            .detach();
        self.run_page_rebuild(cx);
    }

    fn run_resume(&mut self, cx: &mut Context<Self>) {
        let Some((workflow, run)) = self.wf.run.key.clone() else {
            return;
        };
        self.workflow_said(None, true, cx);
        self.workflow_task(
            move |host| actions::resume_run(host, &workflow, &run),
            |this, notice, cx| this.workflow_said(notice, true, cx),
            cx,
        );
    }

    /// Shows a session the run used, in the Mono's panel.
    pub(super) fn open_run_session(&mut self, session: &str, cx: &mut Context<Self>) {
        self.workflow_said(None, true, cx);
        let opened = session.parse::<i64>().ok().filter(|id| {
            self.owner
                .upgrade()
                .and_then(|owner| owner.update(cx, |owner, cx| owner.mono_ensure_session(*id, cx)))
                .is_some()
        });
        let Some(id) = opened else {
            self.workflow_said(Some("Could not open this session.".into()), true, cx);
            return;
        };
        if let Some(back) = self.route.clone() {
            self.wf.run.session_back = Some((id, back));
        }
        self.route = Some(Route::Session(id));
        self.overview_tab = OverviewTab::Sessions;
        self.show_overview(cx);
        cx.notify();
    }

    /// Where Back goes from the session page: the run page it was opened from, else the overview.
    pub(in crate::monos) fn wf_session_back(&mut self) -> Option<Route> {
        let Some(Route::Session(id)) = self.route else {
            return None;
        };
        match self.wf.run.session_back.take() {
            Some((session, back)) if session == id => {
                self.overview_tab = OverviewTab::Workflows;
                Some(back)
            }
            _ => None,
        }
    }

    pub(in crate::monos) fn workflow_run_page(
        &mut self,
        mono: &Mono,
        workflow: &str,
        run_id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.ensure_workflows(cx);
        // A page sliding away while another run opens must not take the state back from it.
        let current =
            matches!(&self.route, Some(Route::WorkflowRun(w, r)) if w == workflow && r == run_id);
        let shown = self.wf.run.key.as_ref() == Some(&(workflow.to_string(), run_id.to_string()));
        if !current && !shown {
            return div().size_full().into_any_element();
        }
        self.run_page_open(mono, workflow, run_id, cx);
        let name = self
            .wf
            .run
            .run
            .as_ref()
            .map_or_else(|| "Run".to_string(), |r| r.definition.name.clone());
        let mut header =
            self.workflow_header(&name, Some(Route::Workflow(workflow.to_string())), cx);
        let run = self.wf.run.run.clone();
        if let Some(run) = &run {
            if has_for_each(run) {
                let lanes = self.wf.run.view() == Some(RunView::Lanes);
                header = header.child(
                    icon_button(
                        "wf-run-view",
                        if lanes {
                            "Show one item at a time"
                        } else {
                            "Show items side by side"
                        },
                        if lanes { "layers" } else { "columns-3" },
                    )
                    .debug_selector(|| "wf-run-view".into())
                    .on_click(cx.listener(|this, _, _, cx| this.run_toggle_view(cx))),
                );
            }
            if run.status == RunStatus::Running {
                let stop = workflow.to_string();
                header = header.child(
                    icon_button("wf-run-stop", "Stop run", "square")
                        .debug_selector(|| "wf-run-stop".into())
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.stop_workflow_run(&stop);
                            cx.notify();
                        })),
                );
            }
            if run.status == RunStatus::Interrupted {
                header = header.child(
                    icon_button("wf-run-resume", "Resume", "play")
                        .debug_selector(|| "wf-run-resume".into())
                        .on_click(cx.listener(|this, _, _, cx| this.run_resume(cx))),
                );
            }
        }
        let body = div()
            .id("mono-workflow-run-page")
            .debug_selector(|| "mono-workflow-run-page".into())
            .w_full()
            .min_w_0()
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .children(notice_line(
                "mono-workflow-notice",
                self.wf.page_notice.as_deref(),
            ));
        let body = match &run {
            Some(run) => body
                .child(self.run_meta(run, mono.color))
                .when(self.wf.run.changed, |d| {
                    d.child(
                        div()
                            .debug_selector(|| "wf-run-changed".into())
                            .flex_shrink_0()
                            .px_4()
                            .pb_3()
                            .text_size(px(11.))
                            .line_height(px(16.))
                            .text_color(content(0.45))
                            .child("The workflow changed since this run started."),
                    )
                })
                .children(run.error.as_ref().map(|error| {
                    alert(error.clone())
                        .id("wf-run-error")
                        .debug_selector(|| "wf-run-error".into())
                        .flex_shrink_0()
                        .px_4()
                        .pt_0()
                        .pb_3()
                }))
                .child(
                    div()
                        .relative()
                        .flex_1()
                        .min_h(px(320.))
                        .children(self.wf.run.canvas.clone()),
                )
                .child(self.run_details(window, mono.color, cx)),
            None if self.wf.run.loaded => body.child(
                div()
                    .debug_selector(|| "wf-run-gone".into())
                    .px_4()
                    .py_6()
                    .text_center()
                    .text_size(px(12.))
                    .text_color(content(0.45))
                    .child("This run is no longer kept."),
            ),
            None => body,
        };
        div()
            .size_full()
            .min_w_0()
            .min_h_0()
            .overflow_hidden()
            .flex()
            .flex_col()
            .child(header)
            .child(body)
            .into_any_element()
    }

    /// Status, when it started and how long it took.
    fn run_meta(&self, run: &WorkflowRun, color: u32) -> Div {
        let (label, dot) = run_status(run.status, color);
        let now = now_ms();
        let took = match run.ended_at {
            Some(ended) => duration(ended.saturating_sub(run.started_at)),
            None if run.status == RunStatus::Running => {
                duration(now.saturating_sub(run.started_at))
            }
            None => "-".into(),
        };
        let marker = if run.status == RunStatus::Running {
            super::view::running_dot("wf-run-pulse", color, 8.)
        } else {
            div()
                .size(px(8.))
                .flex_shrink_0()
                .rounded_full()
                .bg(dot)
                .into_any_element()
        };
        div()
            .debug_selector(|| "wf-run-meta".into())
            .flex_shrink_0()
            .px_4()
            .py_2()
            .flex()
            .flex_wrap()
            .items_center()
            .gap_x_4()
            .gap_y_0p5()
            .child(meta(
                "Status",
                div()
                    .debug_selector(|| "wf-run-status".into())
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .child(marker)
                    .child(label),
            ))
            .child(meta(
                "Started",
                div()
                    .debug_selector(|| "wf-run-started".into())
                    .child(when(run.started_at, now)),
            ))
            .child(meta(
                "Took",
                div().debug_selector(|| "wf-run-took".into()).child(took),
            ))
    }
}
