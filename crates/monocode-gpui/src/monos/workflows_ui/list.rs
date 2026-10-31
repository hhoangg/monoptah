//! The Workflows tab: one row per workflow, with its hover actions (`MonoWorkflows.tsx`).
use super::super::scheduler::{duration, habit_icon_button};
use super::super::*;
use super::actions::{self, ReviewIntent};
use super::requirements::missing_requirements_badge;
use super::view::{needs_review_badge, notice_line};
use monocode_core::mono::workflows::model::{
    NodeKind, NodeStatus, WORKFLOWS_MAX, Workflow, WorkflowRun, trigger_of,
};
use monocode_core::time::now_ms;

/// Branches with a node at work: the group node itself is bookkeeping, not a branch.
pub(in crate::monos) fn active_branches(run: &WorkflowRun) -> usize {
    let groups: HashSet<&str> = run
        .definition
        .nodes
        .iter()
        .filter(|n| matches!(n.kind, NodeKind::ForEach { .. }))
        .map(|n| n.id.as_str())
        .collect();
    let branches: HashSet<&str> = run
        .nodes
        .iter()
        .filter(|n| {
            matches!(n.status, NodeStatus::Running | NodeStatus::WaitingApproval)
                && !groups.contains(n.node_id.as_str())
        })
        .map(|n| n.branch.as_str())
        .collect();
    branches.len().max(1)
}

/// "Weekdays 09:00" for a schedule trigger, "Manual" otherwise.
pub(in crate::monos) fn workflow_trigger_label(workflow: &Workflow) -> String {
    match trigger_of(workflow).map(|n| &n.kind) {
        Some(NodeKind::Schedule { schedule }) => schedule.label(),
        _ => "Manual".into(),
    }
}

/// What the empty tab says.
pub(in crate::monos) fn empty_message(mono: &str) -> String {
    format!("Ask {mono} to chain sessions into a workflow, or create one with +.")
}

/// What a row says under the name.
pub(in crate::monos) fn workflow_subtitle(
    workflow: &Workflow,
    running: Option<(usize, u64)>,
    is_running: bool,
    now: u64,
) -> String {
    if is_running {
        return match running {
            Some((branches, since)) => format!(
                "Running \u{b7} {branches} {} \u{b7} {}",
                if branches == 1 { "branch" } else { "branches" },
                duration(now.saturating_sub(since))
            ),
            None => "Running".into(),
        };
    }
    let label = workflow_trigger_label(workflow);
    if workflow.run_requested {
        "Starting\u{2026}".into()
    } else if workflow.enabled {
        label
    } else {
        format!("Paused \u{b7} {label}")
    }
}

impl Monos {
    /// The tab's buttons beside the tabs: Ask Mono, Import and New, the last two off at the limit.
    pub(in crate::monos) fn workflow_header_actions(
        &self,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let count = self.workflow_count();
        let at_most = count.is_some_and(|n| n >= WORKFLOWS_MAX);
        // Until the list is read the limit is unknown, so adding waits.
        let can_add = count.is_some_and(|n| n < WORKFLOWS_MAX);
        let can_ask = self.chat().is_some();
        vec![
            icon_button_enabled(
                "mono-workflow-ask",
                "Ask Mono to build a workflow",
                "comment-add",
                can_ask,
            )
            .debug_selector(|| "mono-workflow-ask".into())
            .on_click(cx.listener(move |this, _, window, cx| {
                if can_ask {
                    this.ask_mono_for_workflow(window, cx);
                }
            }))
            .into_any_element(),
            icon_button_enabled(
                "mono-workflow-import",
                if at_most {
                    "At most 20 workflows"
                } else {
                    "Import workflow"
                },
                "arrow-down-circle",
                can_add,
            )
            .debug_selector(|| "mono-workflow-import".into())
            .on_click(cx.listener(move |this, _, _, cx| {
                if can_add {
                    this.open_import_page(cx);
                }
            }))
            .into_any_element(),
            icon_button_enabled(
                "mono-workflow-new",
                if at_most {
                    "At most 20 workflows"
                } else {
                    "New workflow"
                },
                "plus",
                can_add,
            )
            .debug_selector(|| "mono-workflow-new".into())
            .on_click(cx.listener(move |this, _, _, cx| {
                if can_add {
                    this.open_workflow_route(Route::NewWorkflow, cx);
                }
            }))
            .into_any_element(),
        ]
    }

    /// Whether the workflow has a run going in the runner.
    pub(in crate::monos) fn workflow_running(&self, id: &str) -> bool {
        self.workflows.is_running(id)
    }

    /// What the runner knows of the workflow's run: its branches at work and when it started.
    pub(super) fn workflow_glance(&self, id: &str) -> Option<(usize, u64)> {
        self.workflows
            .with_run(id, |run| (active_branches(run), run.started_at))
    }

    pub(in crate::monos) fn workflows_list(
        &mut self,
        mono: &Mono,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.ensure_workflows(cx);
        let this = &*self;
        let Some(workflows) = this.wf.loaded.get(&mono.id) else {
            // Blank for the moment it loads, rather than a line the list replaces.
            return div()
                .id("mono-workflows-list")
                .debug_selector(|| "mono-workflows-list".into())
                .size_full()
                .into_any_element();
        };
        if workflows.is_empty() {
            return div()
                .id("mono-workflows-list")
                .debug_selector(|| "mono-workflows-list".into())
                .size_full()
                .child(super::super::panels::empty_state(
                    "git-branch",
                    &empty_message(&mono.label()),
                ))
                .into_any_element();
        }
        let rows = workflows
            .iter()
            .map(|w| this.workflow_row(w, mono, cx))
            .collect::<Vec<_>>();
        let notice = this.wf.notice.clone();
        let gate = self.run_gate_dialog(mono, window, cx);
        div()
            .size_full()
            .min_w_0()
            .relative()
            .child(
                div()
                    .id("mono-workflows-list")
                    .debug_selector(|| "mono-workflows-list".into())
                    .size_full()
                    .min_w_0()
                    .overflow_y_scroll()
                    .p_2()
                    .flex()
                    .flex_col()
                    .gap_px()
                    .children(notice_line("mono-workflow-notice", notice.as_deref()))
                    .children(rows),
            )
            .children(gate)
            .into_any_element()
    }

    fn workflow_row(&self, w: &Workflow, mono: &Mono, cx: &mut Context<Self>) -> AnyElement {
        let id = w.id.clone();
        let running = self.workflow_running(&w.id);
        let glance = self.workflow_glance(&w.id);
        let subtitle = workflow_subtitle(w, glance, running, now_ms());
        let starting = !running && w.run_requested;
        let lit = w.enabled || running;
        div()
            .id(SharedString::from(format!("mono-workflow-{id}")))
            .debug_selector({
                let id = id.clone();
                move || format!("mono-workflow-{id}")
            })
            .group("mono-workflow")
            .w_full()
            .min_w_0()
            .flex_shrink_0()
            .rounded_md()
            .flex()
            .items_center()
            .hover(|s| s.bg(content(0.05)))
            .child(
                div()
                    .id(SharedString::from(format!("mono-workflow-open-{id}")))
                    .role(Role::Button)
                    .aria_label(w.name.clone())
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .gap_2p5()
                    .px_2()
                    .py_2()
                    .child(
                        div()
                            .size(px(28.))
                            .flex_shrink_0()
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_md()
                            .bg(content(0.06))
                            .child(if running {
                                super::view::running_dot(
                                    SharedString::from(format!("mono-workflow-pulse-{id}")),
                                    mono.color,
                                    8.,
                                )
                            } else {
                                icon("git-branch", 14.)
                                    .text_color(content(if lit { 0.6 } else { 0.3 }))
                                    .into_any_element()
                            }),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(
                                div()
                                    .flex()
                                    .min_w_0()
                                    .items_center()
                                    .gap_1p5()
                                    .child(
                                        div()
                                            .min_w_0()
                                            .text_size(px(12.))
                                            .line_height(px(20.))
                                            .truncate()
                                            .text_color(content(if lit { 0.85 } else { 0.45 }))
                                            .child(w.name.clone()),
                                    )
                                    .when(w.needs_review, |d| d.child(needs_review_badge()))
                                    .when(self.missing_requirements(w).is_some(), |d| {
                                        d.child(missing_requirements_badge())
                                    }),
                            )
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .line_height(px(16.))
                                    .truncate()
                                    .text_color(content(0.4))
                                    .child(subtitle),
                            ),
                    )
                    .on_click(cx.listener({
                        let id = id.clone();
                        move |this, _, _, cx| {
                            this.open_workflow_route(Route::Workflow(id.clone()), cx)
                        }
                    })),
            )
            .child(
                self.workflow_actions(w, running, starting, false, None, cx)
                    .pr_2(),
            )
            .into_any_element()
    }

    /// Run now, Stop, Pause or Resume, Remove: the row's hover actions, and the page's buttons.
    /// A flagged workflow opens its review instead of running or resuming.
    fn workflow_actions(
        &self,
        w: &Workflow,
        running: bool,
        starting: bool,
        page: bool,
        before_remove: Option<AnyElement>,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let suffix = if page {
            String::new()
        } else {
            format!("-{}", w.id)
        };
        let (run, stop, toggle, remove) = (w.id.clone(), w.id.clone(), w.id.clone(), w.id.clone());
        let (enabled, review) = (w.enabled, w.needs_review);
        let busy = running || starting;
        div()
            .when(!page, |d| {
                d.opacity(0.)
                    .group_hover("mono-workflow", |s| s.opacity(1.))
            })
            .flex_shrink_0()
            .flex()
            .items_center()
            .child(
                habit_icon_button(
                    SharedString::from(format!("mono-workflow-run{suffix}")),
                    if running {
                        "Already running"
                    } else if starting {
                        "Starting"
                    } else {
                        "Run now"
                    },
                    "zap",
                )
                .debug_selector({
                    let suffix = suffix.clone();
                    move || format!("mono-workflow-run{suffix}")
                })
                .when(busy, |d| d.opacity(0.35))
                .on_click(cx.listener(move |this, _, _, cx| {
                    if !busy {
                        this.workflow_run_clicked(&run, review, page, cx);
                    }
                })),
            )
            .when(running, |d| {
                d.child(
                    habit_icon_button(
                        SharedString::from(format!("mono-workflow-stop{suffix}")),
                        "Stop",
                        "square",
                    )
                    .debug_selector({
                        let suffix = suffix.clone();
                        move || format!("mono-workflow-stop{suffix}")
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.stop_workflow_run(&stop);
                        cx.notify();
                    })),
                )
            })
            .child(
                habit_icon_button(
                    SharedString::from(format!("mono-workflow-toggle{suffix}")),
                    if enabled { "Pause" } else { "Resume" },
                    if enabled { "queue-pause" } else { "queue-play" },
                )
                .debug_selector({
                    let suffix = suffix.clone();
                    move || format!("mono-workflow-toggle{suffix}")
                })
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.workflow_toggle_clicked(&toggle, enabled, review, page, cx)
                })),
            )
            .children(before_remove)
            .child(
                habit_icon_button(
                    SharedString::from(format!("mono-workflow-delete{suffix}")),
                    "Remove",
                    "trash",
                )
                .debug_selector(move || format!("mono-workflow-delete{suffix}"))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.workflow_remove_clicked(&remove, page, cx)
                })),
            )
    }

    /// The workflow page's header actions are the row's, always shown.
    pub(in crate::monos) fn workflow_page_actions(
        &self,
        w: &Workflow,
        before_remove: Option<AnyElement>,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let running = self.workflow_running(&w.id);
        let starting = !running && w.run_requested;
        self.workflow_actions(w, running, starting, true, before_remove, cx)
    }

    fn workflow_back(&self, page: bool) -> Option<Route> {
        if page { self.route.clone() } else { None }
    }

    pub(in crate::monos) fn workflow_run_clicked(
        &mut self,
        id: &str,
        review: bool,
        page: bool,
        cx: &mut Context<Self>,
    ) {
        if review {
            let back = self.workflow_back(page);
            self.open_workflow_review(id, ReviewIntent::Run, back, cx);
            return;
        }
        if self.run_gate_for_run(id, page, cx) {
            return;
        }
        self.run_workflow_now(id, None, page, cx);
    }

    pub(in crate::monos) fn workflow_toggle_clicked(
        &mut self,
        id: &str,
        enabled: bool,
        review: bool,
        page: bool,
        cx: &mut Context<Self>,
    ) {
        if review && !enabled {
            let back = self.workflow_back(page);
            self.open_workflow_review(id, ReviewIntent::Enable, back, cx);
            return;
        }
        let id = id.to_string();
        self.workflow_task(
            move |host| actions::toggle(host, &id),
            move |this, notice, cx| this.workflow_said(notice, page, cx),
            cx,
        );
    }

    pub(in crate::monos) fn workflow_remove_clicked(
        &mut self,
        id: &str,
        page: bool,
        cx: &mut Context<Self>,
    ) {
        let id = id.to_string();
        let removed = id.clone();
        self.workflow_task(
            move |host| actions::remove(host, &id),
            move |this, notice, cx| {
                if notice.is_none()
                    && matches!(&this.route, Some(Route::Workflow(open)) if *open == removed)
                {
                    // Once the workflow is gone, its page closes.
                    this.route = None;
                }
                this.workflow_said(notice, page, cx);
            },
            cx,
        );
    }
}
