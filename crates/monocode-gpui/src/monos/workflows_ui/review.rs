//! The review a workflow needs before it may run or be enabled (`WorkflowReviewPage.tsx`), and the
//! sections both it and the import page show (`WorkflowReviewSections.tsx`): what a workflow
//! would run, as it is stored right now, with the characters that hide made visible.
use super::super::*;
use super::actions::{self, Approval, CHANGED, ReviewIntent};
use super::view::{header_button, hint, notice_line, property, section};
use gpui::Div;
use monocode_core::mono::workflows::{
    board_edit::type_label,
    board_model::NodeType,
    model::{Checkout, Node, NodeKind, Workflow, definition_fingerprint},
    models::{ModelCatalog, harnesses_used},
    yaml::{ImportPreview, review_of, visible},
};

/// What the review page shows and what approving applies to.
pub(in crate::monos) struct Review {
    pub(super) id: String,
    pub(super) intent: ReviewIntent,
    /// Where Back and a finished approval go: the workflow page, or the overview.
    pub(super) back: Option<Route>,
    /// What is on screen is what approving applies to.
    pub(super) shown: Workflow,
    /// The fingerprint of the stored definition last looked at.
    pub(super) seen: String,
    pub(super) notice: Option<String>,
    pub(super) busy: bool,
    /// The review of `shown`, and the fingerprint it was made for.
    pub(super) preview: Option<(String, ImportPreview)>,
}

impl Review {
    /// The review of what is shown, once it was made for exactly this definition. Until then the
    /// page draws nothing and approving is off: the user must never approve what they have not
    /// been shown.
    pub(super) fn ready_preview(&self) -> Option<&ImportPreview> {
        self.preview
            .as_ref()
            .filter(|(made_for, _)| *made_for == definition_fingerprint(&self.shown))
            .map(|(_, preview)| preview)
    }
}

/// How the file names a step: its title as written, else its id read as words (runs of `_` are
/// one space).
pub(super) fn node_title(node: &Node) -> String {
    match node.title.as_deref().filter(|t| !t.trim().is_empty()) {
        Some(title) => title.to_string(),
        None => {
            let mut words = String::new();
            for c in node.id.chars() {
                if c != '_' || !words.ends_with(' ') {
                    words.push(if c == '_' { ' ' } else { c });
                }
            }
            let words = words.trim();
            let mut chars = words.chars();
            chars
                .next()
                .map(|first| first.to_uppercase().chain(chars).collect())
                .unwrap_or_default()
        }
    }
}

/// `NODE_LABEL` of the board.
pub(in crate::monos) fn node_label(kind: &NodeKind) -> &'static str {
    type_label(NodeType::of(kind))
}

fn step_label(node: &Node) -> String {
    visible(&node_title(node))
}

fn step_name(preview: &ImportPreview, node_id: &str) -> String {
    match preview.workflow.nodes.iter().find(|n| n.id == node_id) {
        Some(node) => format!("{} ({})", step_label(node), visible(&node.id)),
        None => visible(node_id),
    }
}

/// Where a session step runs and with what, as its file says: nothing set means the Mono's own.
pub(in crate::monos) fn session_detail(preview: &ImportPreview, node: &Node) -> Option<String> {
    let NodeKind::Session {
        harness,
        model,
        effort,
        checkout,
        ..
    } = &node.kind
    else {
        return None;
    };
    let where_ = match checkout {
        Checkout::Root => "Project folder".to_string(),
        Checkout::NewWorktree => "New worktree".to_string(),
        Checkout::WorktreeOf(of) => {
            let source = preview.workflow.nodes.iter().find(|n| &n.id == of);
            format!(
                "Worktree of {}",
                source.map_or_else(|| visible(of), step_label)
            )
        }
    };
    let harness = harness.as_deref().filter(|h| !h.is_empty());
    let model = model.as_deref().filter(|m| !m.is_empty());
    let mut parts = Vec::new();
    if let Some(harness) = harness {
        parts.push(format!("Harness: {}", visible(harness)));
    }
    parts.push(format!(
        "Model: {}",
        match (model, harness) {
            (Some(model), _) => visible(model),
            (None, Some(_)) => "its default".to_string(),
            (None, None) => "Mono default".to_string(),
        }
    ));
    if let Some(effort) = effort.as_deref().filter(|e| !e.is_empty()) {
        parts.push(format!("Effort: {}", visible(effort)));
    }
    parts.push(format!("Checkout: {where_}"));
    Some(parts.join(" \u{b7} "))
}

fn trigger_label(preview: &ImportPreview) -> String {
    match preview.workflow.nodes.iter().find(|n| n.is_trigger()) {
        None => "No trigger".into(),
        Some(Node {
            kind: NodeKind::Schedule { schedule },
            ..
        }) => schedule.label(),
        Some(_) => "Manual".into(),
    }
}

/// Text exactly as the file has it, in the monospace the app uses for code.
fn verbatim(text: &str) -> Div {
    div()
        .w_full()
        .min_w_0()
        .rounded_md()
        .bg(content(0.05))
        .px_2p5()
        .py_2()
        .font_family(crate::fonts::mono())
        .text_size(px(11.))
        .line_height(px(16.))
        .text_color(content(0.75))
        .child(visible(text))
}

fn labelled(label: impl Into<SharedString>, body: impl IntoElement) -> Div {
    div()
        .w_full()
        .min_w_0()
        .px_2()
        .pb_3()
        .child(
            div()
                .px_0p5()
                .pb_1()
                .text_size(px(11.))
                .line_height(px(16.))
                .text_color(content(0.45))
                .child(label.into()),
        )
        .child(body)
}

fn plain(text: &str) -> Div {
    div()
        .px_2()
        .text_size(px(12.))
        .text_color(content(0.4))
        .child(text.to_string())
}

/// A loop back edge: where it goes and the exact text that decides whether it repeats.
fn loop_row(preview: &ImportPreview, from: &str, to: &str, rounds: f64, decide: &str) -> Div {
    let times = if rounds == 1.0 { "round" } else { "rounds" };
    labelled(
        format!(
            "After {} it may loop back to {}, up to {} {times}",
            step_name(preview, from),
            step_name(preview, to),
            monocode_core::mono::workflows::ordered::js_number_text(rounds),
        ),
        if decide == "mono" {
            div()
                .px_0p5()
                .text_size(px(12.))
                .line_height(px(20.))
                .text_color(content(0.75))
                .child("The Mono decides each time whether it repeats.")
        } else {
            verbatim(decide)
        },
    )
}

impl Monos {
    /// What a user reads before a workflow may run: its name and trigger, what it says it does,
    /// its problems, its steps and loops, every shell command verbatim and every prompt. The
    /// import page and the review of a stored workflow both render this, so they cannot drift.
    pub(in crate::monos) fn review_sections(
        &self,
        preview: &ImportPreview,
        problems_hint: &str,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let mut out: Vec<AnyElement> = Vec::new();
        out.push(
            div()
                .w_full()
                .min_w_0()
                .flex_shrink_0()
                .px_4()
                .py_3()
                .flex()
                .flex_col()
                .gap_0p5()
                .child(property("Name", visible(&preview.workflow.name)))
                .child(property("Runs", trigger_label(preview)))
                .into_any_element(),
        );
        if let Some(description) = preview
            .workflow
            .description
            .as_deref()
            .filter(|d| !d.is_empty())
        {
            out.push(
                section("What it does", None)
                    .child(
                        div()
                            .id("mono-workflow-description")
                            .px_2()
                            .text_size(px(12.))
                            .line_height(px(20.))
                            .text_color(content(0.75))
                            .child(visible(description)),
                    )
                    .into_any_element(),
            );
        }
        if !preview.issues.is_empty() {
            out.push(
                section("Problems", None)
                    .id("mono-workflow-problems")
                    .debug_selector(|| "mono-workflow-problems".into())
                    .child(div().px_2().flex().flex_col().gap_1().children(
                        preview.issues.iter().map(|issue| {
                            div()
                                .flex()
                                .items_start()
                                .gap_1p5()
                                .text_size(px(11.))
                                .line_height(px(16.))
                                .text_color(gpui::Hsla::from(rgb(super::view::ERROR)).opacity(0.8))
                                .child(
                                    div()
                                        .mt(px(6.))
                                        .size(px(6.))
                                        .flex_shrink_0()
                                        .rounded_full()
                                        .bg(gpui::Hsla::from(rgb(super::view::ERROR)).opacity(0.7)),
                                )
                                .child(div().min_w_0().child(visible(&issue.message)))
                        }),
                    ))
                    .when(!problems_hint.is_empty(), |d| {
                        d.child(hint(problems_hint.to_string()))
                    })
                    .into_any_element(),
            );
        }
        let steps = preview.workflow.nodes.iter().map(|node| {
            let detail = session_detail(preview, node);
            div()
                .flex()
                .min_w_0()
                .gap_2()
                .child(
                    div()
                        .w(px(96.))
                        .flex_shrink_0()
                        .text_color(content(0.45))
                        .child(node_label(&node.kind)),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_color(content(0.85))
                        .child(step_label(node))
                        .when_some(detail, |d, detail| {
                            d.child(
                                div()
                                    .text_size(px(11.))
                                    .line_height(px(16.))
                                    .text_color(content(0.45))
                                    .child(detail),
                            )
                        }),
                )
        });
        let loops = preview.workflow.edges.iter().filter_map(|edge| {
            edge.loop_spec
                .as_ref()
                .map(|spec| loop_row(preview, &edge.from, &edge.to, spec.max_rounds, &spec.decide))
        });
        out.push(
            section("Steps", None)
                .id("mono-workflow-steps")
                .debug_selector(|| "mono-workflow-steps".into())
                .child(
                    div()
                        .px_2()
                        .pb_1()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .text_size(px(12.))
                        .line_height(px(20.))
                        .children(steps),
                )
                .children(loops)
                .into_any_element(),
        );
        out.push(
            section("Commands this workflow runs", None)
                .id("mono-workflow-commands")
                .debug_selector(|| "mono-workflow-commands".into())
                .when(preview.commands.is_empty(), |d| {
                    d.child(plain("It runs no shell commands."))
                })
                .children(
                    preview
                        .commands
                        .iter()
                        .map(|c| labelled(step_name(preview, &c.node_id), verbatim(&c.command))),
                )
                .into_any_element(),
        );
        if !preview.workflow.requires.is_empty() {
            out.push(
                section("Needs on this machine", None)
                    .id("mono-workflow-requires")
                    .debug_selector(|| "mono-workflow-requires".into())
                    .children(preview.workflow.requires.iter().map(|r| {
                        labelled(
                            format!("{} {}", r.kind.key(), visible(&r.name)),
                            plain(
                                &r.hint
                                    .as_deref()
                                    .map_or_else(|| "No hint.".to_string(), visible),
                            ),
                        )
                    }))
                    .child(hint("Only looked up by name. Nothing here is run."))
                    .into_any_element(),
            );
        }
        let open = !self.wf.prompts_closed;
        let toggle = div()
            .id("mono-workflow-prompts-toggle")
            .debug_selector(|| "mono-workflow-prompts-toggle".into())
            .role(Role::Button)
            .aria_label(if open { "Hide prompts" } else { "Show prompts" })
            .aria_expanded(open)
            .size(px(18.))
            .flex()
            .items_center()
            .justify_center()
            .rounded_sm()
            .text_color(content(0.45))
            .hover(|s| s.bg(content(0.08)).text_color(content(1.)))
            .child(icon(
                if open {
                    "chevron-down"
                } else {
                    "chevron-right"
                },
                12.,
            ))
            .on_click(cx.listener(|this, _, _, cx| {
                this.wf.prompts_closed = !this.wf.prompts_closed;
                cx.notify();
            }))
            .into_any_element();
        out.push(
            section("Prompts", Some(toggle))
                .id("mono-workflow-prompts")
                .debug_selector(|| "mono-workflow-prompts".into())
                .when(open && preview.prompts.is_empty(), |d| {
                    d.child(plain("It sends no prompts."))
                })
                .when(open, |d| {
                    d.children(
                        preview
                            .prompts
                            .iter()
                            .map(|p| labelled(step_name(preview, &p.node_id), verbatim(&p.text))),
                    )
                })
                .into_any_element(),
        );
        out
    }

    /// Opens the review a flagged workflow needs before it can run or be enabled.
    pub(in crate::monos) fn open_workflow_review(
        &mut self,
        id: &str,
        intent: ReviewIntent,
        back: Option<Route>,
        cx: &mut Context<Self>,
    ) {
        let Some(workflow) = self.mono_workflow(id).cloned() else {
            return;
        };
        self.wf.page_notice = None;
        // Prompts are open every time a review is opened.
        self.wf.prompts_closed = false;
        self.wf.review = Some(Review {
            id: id.to_string(),
            intent,
            back,
            seen: definition_fingerprint(&workflow),
            shown: workflow,
            notice: None,
            busy: false,
            preview: None,
        });
        self.route = Some(Route::WorkflowReview(id.to_string(), intent));
        self.review_preview(cx);
        cx.notify();
    }

    /// The stored workflow changed (the Mono edits them too): the screen follows it, and says so.
    pub(in crate::monos) fn review_sync(&mut self, cx: &mut Context<Self>) {
        let Some(review) = &self.wf.review else {
            return;
        };
        let Some(stored) = self.mono_workflow(&review.id).cloned() else {
            return;
        };
        let incoming = definition_fingerprint(&stored);
        if let Some(review) = &mut self.wf.review
            && incoming != review.seen
        {
            review.seen = incoming.clone();
            if incoming != definition_fingerprint(&review.shown) {
                review.shown = stored;
                review.notice = Some(CHANGED.into());
            }
        }
        self.review_preview(cx);
    }

    /// Makes the review of what is shown, off the UI thread (it asks which models exist).
    fn review_preview(&mut self, cx: &mut Context<Self>) {
        let Some(review) = &self.wf.review else {
            return;
        };
        let fingerprint = definition_fingerprint(&review.shown);
        if review
            .preview
            .as_ref()
            .is_some_and(|(made_for, _)| *made_for == fingerprint)
        {
            return;
        }
        let shown = review.shown.clone();
        let id = review.id.clone();
        let probe = self.workflow_live.probe();
        let work = cx.background_executor().spawn(async move {
            let catalog = ModelCatalog::for_harnesses(&harnesses_used(&shown), probe);
            review_of(&shown, &catalog)
        });
        cx.spawn(async move |this, cx| {
            let made = work.await;
            let _ = this.update(cx, |this, cx| {
                if let Some(review) = &mut this.wf.review
                    && review.id == id
                    && definition_fingerprint(&review.shown) == fingerprint
                {
                    review.preview = Some((fingerprint, made));
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Approves what is on screen. It only counts for the definition shown: if the stored one
    /// changed meanwhile, nothing is approved and the new version is shown to be read again.
    pub(in crate::monos) fn approve_review(&mut self, cx: &mut Context<Self>) {
        let Some(review) = &mut self.wf.review else {
            return;
        };
        // What is approved is the definition the preview on screen was made for.
        let Some(fingerprint) = review
            .preview
            .as_ref()
            .map(|(made_for, _)| made_for.clone())
            .filter(|made_for| *made_for == definition_fingerprint(&review.shown))
        else {
            return;
        };
        if review.busy {
            return;
        }
        review.busy = true;
        review.notice = None;
        let (id, intent) = (review.id.clone(), review.intent);
        cx.notify();
        let (task_id, back) = (id.clone(), review.back.clone());
        self.workflow_task(
            move |host| actions::approve(host, &task_id, intent, &fingerprint),
            move |this, result, cx| {
                let Some(review) = &mut this.wf.review else {
                    return;
                };
                if review.id != id {
                    return;
                }
                review.busy = false;
                match result {
                    Approval::Done => {
                        this.wf.review = None;
                        if matches!(this.route, Some(Route::WorkflowReview(..))) {
                            this.route = back;
                        }
                    }
                    Approval::Changed(stored) => {
                        review.seen = definition_fingerprint(&stored);
                        review.shown = *stored;
                        review.notice = Some(CHANGED.into());
                        this.review_preview(cx);
                    }
                    Approval::Refused(notice) => review.notice = Some(notice),
                }
                cx.notify();
            },
            cx,
        );
    }

    pub(in crate::monos) fn workflow_review_page(
        &mut self,
        id: &str,
        intent: ReviewIntent,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(review) = self.wf.review.as_ref().filter(|r| r.id == id) else {
            // The page is sliding away; nothing to review any more.
            return div().size_full().into_any_element();
        };
        let back = review.back.clone();
        let can_approve = !review.busy && review.ready_preview().is_some();
        let notice = review.notice.clone();
        let sections = match review.ready_preview().cloned() {
            Some(preview) => self.review_sections(
                &preview,
                "It cannot be enabled or run until these are fixed.",
                cx,
            ),
            None => Vec::new(),
        };
        let go_back = back.clone();
        div()
            .size_full()
            .min_w_0()
            .min_h_0()
            .overflow_hidden()
            .flex()
            .flex_col()
            .child(
                self.workflow_header("Review workflow", back, cx)
                    .child(
                        header_button("mono-workflow-review-cancel", "Cancel", false, true)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.wf.review = None;
                                this.route = go_back.clone();
                                this.refresh_workflows(cx);
                                cx.notify();
                            })),
                    )
                    .child(
                        header_button(
                            "mono-workflow-approve",
                            if intent == ReviewIntent::Run {
                                "Approve and run"
                            } else {
                                "Approve and enable"
                            },
                            true,
                            can_approve,
                        )
                        .on_click(cx.listener(|this, _, _, cx| this.approve_review(cx))),
                    ),
            )
            .child(
                div()
                    .id("mono-workflow-review")
                    .debug_selector(|| "mono-workflow-review".into())
                    .w_full()
                    .min_w_0()
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .flex()
                    .flex_col()
                    .children(notice_line("mono-workflow-notice", notice.as_deref()))
                    .children(sections),
            )
            .into_any_element()
    }
}
