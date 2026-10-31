//! The board editor (`WorkflowBoard.tsx`): a workflow drawn top to bottom on the graph canvas and
//! edited in place. Edits stay in `BoardState` until Save; an edit made elsewhere (the Mono's
//! CLI) shows up live while there are none and is asked about otherwise. The rules are in core's
//! `board_edit`; this file holds the state, the canvas events, saving and the field handlers.
//! All file access and model probing runs off the UI thread.
use super::super::*;
use super::actions;
use super::graph::{GraphCanvas, GraphEvent, Scene};
use super::page::value_text;
use gpui::Pixels;
use monocode_core::mono::workflows::{
    board_edit::*,
    board_model::{
        BoardNodeData, Flow, FlowKind, NodeType, Point, absolute, blank_workflow, definition_key,
        issues_by_node, to_flow,
    },
    model::{
        InputValue, LoopSpec, Node, NodeKind, Requirement, RequirementKind, SWITCH_DEFAULT,
        Workflow, WorkflowInput,
    },
    models::{ModelCatalog, harnesses_used},
    validate::WorkflowIssue,
};
use monocode_core::time::now_ms;
use std::rc::Rc;

/// Whether `name` may be a case of a Switch whose other cases are `cases`: not blank, not the
/// reserved `default`, not one already there, and not the handle of a line the Switch already has
/// (`taken`), which the case would capture.
fn case_name_usable(name: &str, cases: &[String], taken: &[String]) -> bool {
    !name.trim().is_empty()
        && name != SWITCH_DEFAULT
        && !cases.iter().any(|c| c == name)
        && !taken.iter().any(|h| h == name)
}

/// The handles of the lines that leave the node `id` of `flow`.
fn out_handles_taken(flow: &Flow, id: &str) -> Vec<String> {
    flow.edges
        .iter()
        .filter(|e| e.loop_spec.is_none() && e.source == id)
        .filter_map(|e| e.source_handle.clone())
        .collect()
}

/// The name a new case of a Switch starts with.
fn next_case_name(cases: &[String], taken: &[String]) -> String {
    let mut n = 1;
    loop {
        let name = format!("case_{n}");
        if case_name_usable(&name, cases, taken) {
            return name;
        }
        n += 1;
    }
}

/// What the slide-over of the board shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum InspectorView {
    Node(String),
    Settings,
}

/// The text fields of the board. A `Live` one applies every change; a `Commit` one when it is
/// left or submitted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum BoardField {
    Name,
    Description,
    Title(String),
    Prompt(String),
    Command(String),
    Question(String),
    Condition(String),
    IfQuestion(String),
    SwitchValue(String),
    SwitchQuestion(String),
    List(String),
    EachAs(String),
    AtOnce(String),
    Template(String),
    Limit(String),
    Answer(String, usize),
    /// The condition of the loop leaving a node (None: the loop being asked for).
    LoopCondition(Option<String>),
    Time(String),
    /// Committed: the name of an output field of a node.
    OutputName(String, String),
    /// Committed: the name of a case of a Switch.
    CaseName(String, String),
    /// Committed: the value of a declared input.
    InputValue(String),
    /// Committed: the name of a new input.
    NewInput,
    /// Committed: the name, and the hint, of the requirement at this place.
    RequireName(usize),
    RequireHint(usize),
}

impl BoardField {
    pub(super) fn key(&self) -> String {
        format!("wf-b:{self:?}")
    }

    fn commits(&self) -> bool {
        matches!(
            self,
            Self::OutputName(..)
                | Self::CaseName(..)
                | Self::InputValue(_)
                | Self::RequireName(_)
                | Self::RequireHint(_)
        )
    }
}

/// What a drop-down of the board chooses for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::monos) enum BoardPick {
    Checkout(String),
    NestUnder(String),
    RunsIn(String),
    Effort(String),
    Project(String),
    /// Max rounds of the loop leaving a node (None: the loop being asked for).
    MaxRounds(Option<String>),
    Decider(Option<String>),
    IfDecider(String),
    SwitchDecider(String),
    OutputType(String, String),
    ScheduleKind(String),
    ScheduleHours(String),
    ScheduleDay(String),
    ScheduleMinute(String),
    InputValue(String),
    NewInputType,
    /// The kind of the requirement at this place.
    RequireKind(usize),
}

/// The loop being asked for after a connection back up a branch.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct LoopAsk {
    pub from: String,
    pub to: String,
    pub spec: LoopSpec,
    /// Where the pointer let go, in the window.
    pub at: gpui::Point<Pixels>,
}

pub(super) struct BoardState {
    /// The route key: the workflow id, or "new".
    pub key: String,
    /// The stored workflow the board last synced (what the draft is a change of).
    pub base: Workflow,
    pub flow: Flow,
    pub meta: Meta,
    pub is_new: bool,
    /// The `definition_key` of the saved version, and of the stored one last seen.
    pub baseline: String,
    pub seen: String,
    pub view: Option<InspectorView>,
    pub loop_ask: Option<LoopAsk>,
    pub leaving: bool,
    pub saving: bool,
    pub show_issues: bool,
    /// An edit made elsewhere; `true` when it stopped a Save, which then waits for a choice.
    pub elsewhere: Option<(Workflow, bool)>,
    pub catalog: ModelCatalog,
    catalog_kinds: Vec<monocode_core::provider::Kind>,
    catalog_rev: u64,
    pub issues: Vec<WorkflowIssue>,
    pub canvas: Entity<GraphCanvas>,
    /// A new key makes the canvas fit its view again.
    pub fit_gen: u64,
    pub add_anchor: AnchorSlot,
    pub more_anchor: AnchorSlot,
    pub area: AnchorSlot,
    pub model_anchor: AnchorSlot,
    /// The type chosen for the next input declared in the settings.
    pub new_input_type: String,
    /// What the settings say is wrong with an input value just typed.
    pub input_problem: Option<String>,
    _events: Subscription,
    /// The subscriptions of the board's text fields; they go with the board.
    subs: Vec<Subscription>,
}

fn is_board(route: &Route) -> Option<String> {
    match route {
        Route::NewWorkflow => Some("new".into()),
        Route::WorkflowBoard(id) => Some(id.clone()),
        _ => None,
    }
}

impl BoardState {
    #[cfg(test)]
    pub(super) fn subscriptions(&self) -> usize {
        self.subs.len()
    }

    pub(super) fn current(&self) -> Workflow {
        current(&self.base, &self.flow, &self.meta)
    }

    pub(super) fn changed(&self) -> bool {
        definition_key(&self.current()) != self.baseline
    }

    /// Enabling goes through the review, never through Save.
    pub(super) fn unreviewed(&self) -> bool {
        self.base.needs_review && self.meta.enabled
    }

    pub(super) fn blocked(&self) -> bool {
        self.meta.enabled && (!self.issues.is_empty() || self.unreviewed())
    }

    pub(super) fn can_save(&self) -> bool {
        (self.changed() || self.is_new) && !self.blocked() && !self.saving
    }

    pub(super) fn node(&self, id: &str) -> Option<&Node> {
        self.flow
            .nodes
            .iter()
            .find(|n| n.id == id)
            .map(|n| &n.data.node)
    }
}

/// How a save ended.
enum Saved {
    Done(Box<Workflow>),
    Changed(Box<Workflow>),
}

impl Monos {
    /// The board for the open route, made the first time its page is drawn. False while the
    /// workflow it edits is not loaded yet.
    pub(super) fn board_open(
        &mut self,
        mono: &Mono,
        route: &Route,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(key) = is_board(route) else {
            return false;
        };
        if self.wf.board.as_ref().is_some_and(|b| b.key == key) {
            return true;
        }
        let (workflow, is_new) = match route {
            Route::WorkflowBoard(id) => match self.mono_workflow(id) {
                Some(w) => (w.clone(), false),
                None => return false,
            },
            _ => (blank_workflow(now_ms()), true),
        };
        let canvas = cx.new(|_| GraphCanvas::new(mono.color));
        canvas.update(cx, |c, _| c.set_editable(Rc::new(|_, _, _| false)));
        let events = cx.subscribe(&canvas, |this, _, event: &GraphEvent, cx| {
            this.board_graph_event(event, cx)
        });
        let baseline = definition_key(&workflow);
        let flow = to_flow(&workflow);
        self.wf.board = Some(BoardState {
            key,
            meta: Meta::of(&workflow),
            flow,
            base: workflow,
            is_new,
            seen: baseline.clone(),
            baseline,
            view: None,
            loop_ask: None,
            leaving: false,
            saving: false,
            show_issues: false,
            elsewhere: None,
            catalog: ModelCatalog::unprobed(),
            catalog_kinds: Vec::new(),
            catalog_rev: 0,
            issues: Vec::new(),
            canvas,
            fit_gen: 0,
            add_anchor: AnchorSlot::default(),
            more_anchor: AnchorSlot::default(),
            area: AnchorSlot::default(),
            model_anchor: AnchorSlot::default(),
            new_input_type: "string".into(),
            input_problem: None,
            _events: events,
            subs: Vec::new(),
        });
        self.board_rebuild(cx);
        true
    }

    /// Drops the board and its fields.
    pub(super) fn board_drop(&mut self) {
        self.wf.board = None;
        self.fields.retain(|key, _| !key.starts_with("wf-b:"));
    }

    /// The Mono's own provider, which a session without one of its own starts with.
    pub(super) fn board_mono_kind(&self, cx: &App) -> Option<monocode_core::provider::Kind> {
        self.chat()
            .map(|chat| monocode_core::provider::Kind::from_id(&chat.read(cx).summary.provider))
    }

    /// Draws the board again: the issues, the cards and the lines, and asks which models exist.
    pub(super) fn board_rebuild(&mut self, cx: &mut Context<Self>) {
        let mono_kind = self.board_mono_kind(cx);
        let Some(b) = self.wf.board.as_mut() else {
            return;
        };
        let w = b.current();
        b.issues = problems_of(&w, &b.catalog);
        let by_node = issues_by_node(&b.issues);
        let loops: Vec<(&str, &str)> = w
            .edges
            .iter()
            .filter(|e| e.loop_spec.is_some())
            .map(|e| (e.from.as_str(), e.to.as_str()))
            .collect();
        let mut nodes = b.flow.nodes.clone();
        for n in &mut nodes {
            let id = n.id.clone();
            n.data = BoardNodeData {
                issues: by_node.get(&id).cloned().unwrap_or_default(),
                loop_in: loops.iter().any(|(_, to)| *to == id),
                loop_out: loops.iter().any(|(from, _)| *from == id),
                empty: n.kind == FlowKind::Group
                    && !b
                        .flow
                        .nodes
                        .iter()
                        .any(|c| c.parent_id.as_deref() == Some(&id)),
                ..n.data.clone()
            };
        }
        let scene = Scene::build(
            &nodes,
            &b.flow.edges,
            Vec::new(),
            format!("board:{}", b.fit_gen),
        );
        let checked = w.clone();
        let connector: super::graph::Connector =
            Rc::new(move |source, handle, target| can_connect(&checked, source, handle, target));
        let canvas = b.canvas.clone();
        // An inspector over a node that is gone closes.
        if let Some(InspectorView::Node(id)) = &b.view
            && !w.nodes.iter().any(|n| n.id == *id)
        {
            b.view = None;
        }
        let mut kinds = harnesses_used(&w);
        if let Some(kind) = mono_kind
            && !kinds.contains(&kind)
        {
            kinds.push(kind);
        }
        let probe = (b.catalog_kinds != kinds).then(|| {
            b.catalog_kinds = kinds.clone();
            b.catalog_rev += 1;
            b.catalog_rev
        });
        canvas.update(cx, |canvas, cx| {
            canvas.set_connector(connector);
            canvas.set_scene(scene, cx);
        });
        if let Some(rev) = probe {
            let probe = self.workflow_live.probe();
            let work = cx
                .background_executor()
                .spawn(async move { ModelCatalog::for_harnesses(&kinds, probe) });
            cx.spawn(async move |this, cx| {
                let catalog = work.await;
                let _ = this.update(cx, |this, cx| {
                    if let Some(b) = this.wf.board.as_mut()
                        && b.catalog_rev == rev
                    {
                        b.catalog = catalog;
                        this.board_rebuild(cx);
                    }
                });
            })
            .detach();
        }
        cx.notify();
    }

    /// Changes the board's flow and draws it again.
    fn board_flow(&mut self, f: impl FnOnce(&Flow, &Workflow) -> Flow, cx: &mut Context<Self>) {
        let Some(b) = self.wf.board.as_mut() else {
            return;
        };
        let w = b.current();
        b.flow = f(&b.flow, &w);
        self.board_rebuild(cx);
    }

    /// Changes one node and draws the board again, if it changed.
    pub(super) fn board_node_edit(
        &mut self,
        id: &str,
        f: impl FnOnce(&mut Node),
        cx: &mut Context<Self>,
    ) {
        let Some(b) = self.wf.board.as_ref() else {
            return;
        };
        let Some(old) = b.node(id) else {
            return;
        };
        let mut next = old.clone();
        f(&mut next);
        if next == *old {
            return;
        }
        self.board_flow(|flow, _| update_node(flow, &next), cx);
    }

    /// Changes the workflow's own fields and draws the board again.
    pub(super) fn board_meta_edit(&mut self, f: impl FnOnce(&mut Meta), cx: &mut Context<Self>) {
        let Some(b) = self.wf.board.as_mut() else {
            return;
        };
        let before = b.meta.clone();
        f(&mut b.meta);
        if b.meta != before {
            self.board_rebuild(cx);
        }
    }

    pub(super) fn board_view(&mut self, view: Option<InspectorView>, cx: &mut Context<Self>) {
        if let Some(b) = self.wf.board.as_mut() {
            b.view = view;
            cx.notify();
        }
    }

    // ---- the canvas -------------------------------------------------------------------------

    pub(super) fn board_graph_event(&mut self, event: &GraphEvent, cx: &mut Context<Self>) {
        if self.wf.board.is_none() {
            return;
        }
        match event {
            GraphEvent::Click(id) => {
                let id = id.clone();
                self.board_flow(|flow, _| select(flow, Some(&id), None), cx);
                self.board_view(Some(InspectorView::Node(id)), cx);
            }
            GraphEvent::Drag { id, to } => {
                let (id, to) = (id.clone(), *to);
                self.board_flow(
                    |flow, _| {
                        let mut out = if flow.nodes.iter().any(|n| n.id == id && n.selected) {
                            flow.clone()
                        } else {
                            select(flow, Some(&id), None)
                        };
                        let parent = out
                            .nodes
                            .iter()
                            .find(|n| n.id == id)
                            .and_then(|n| n.parent_id.clone())
                            .and_then(|p| out.nodes.iter().find(|n| n.id == p).cloned());
                        let origin = parent.map_or(Point::default(), |p| absolute(&out.nodes, &p));
                        if let Some(n) = out.nodes.iter_mut().find(|n| n.id == id) {
                            n.position = Point {
                                x: to.x - origin.x,
                                y: to.y - origin.y,
                            };
                        }
                        out
                    },
                    cx,
                );
            }
            GraphEvent::Drop(id) => {
                let id = id.clone();
                self.board_flow(|flow, _| drag_stop(flow, &id), cx);
            }
            GraphEvent::Connect {
                source,
                handle,
                target,
                at,
            } => {
                let (source, target, handle, at) =
                    (source.clone(), target.clone(), handle.clone(), *at);
                let mut asked = None;
                self.board_flow(
                    |flow, w| match connect(flow, w, &source, &target, handle.as_deref()) {
                        Connected::Edge(out) => out,
                        Connected::Loop => {
                            asked = Some(LoopAsk {
                                from: source.clone(),
                                to: target.clone(),
                                spec: default_loop(),
                                at,
                            });
                            flow.clone()
                        }
                        Connected::Invalid => flow.clone(),
                    },
                    cx,
                );
                if let Some(b) = self.wf.board.as_mut()
                    && asked.is_some()
                {
                    b.loop_ask = asked;
                    cx.notify();
                }
            }
            GraphEvent::EdgeClick(id) => {
                let id = id.clone();
                self.board_flow(|flow, _| select(flow, None, Some(&id)), cx);
            }
            GraphEvent::PaneClick => self.board_flow(|flow, _| select(flow, None, None), cx),
            // Not while the inspector is open: Backspace on one of its buttons must not delete a
            // node.
            GraphEvent::Delete if self.wf.board.as_ref().is_some_and(|b| b.view.is_none()) => {
                self.board_flow(|flow, _| delete_selected(flow), cx)
            }
            _ => {}
        }
    }

    pub(super) fn board_add(&mut self, ty: NodeType, cx: &mut Context<Self>) {
        let mut added = None;
        self.board_flow(
            |flow, w| {
                let (out, id) = add_node(flow, w, ty);
                added = Some(id);
                out
            },
            cx,
        );
        if let (Some(b), Some(id)) = (self.wf.board.as_mut(), added) {
            b.view = Some(InspectorView::Node(id));
            b.fit_gen += 1;
            self.board_rebuild(cx);
        }
    }

    pub(super) fn board_remove(&mut self, id: &str, cx: &mut Context<Self>) {
        let id = id.to_string();
        self.board_flow(|flow, _| remove_node(flow, &id), cx);
        self.board_view(None, cx);
    }

    pub(super) fn board_confirm_loop(&mut self, cx: &mut Context<Self>) {
        let Some(ask) = self.wf.board.as_mut().and_then(|b| b.loop_ask.take()) else {
            return;
        };
        // The board may have changed while the card was open (the target deleted).
        self.board_flow(
            |flow, w| {
                if loop_allowed(w, &ask.from, &ask.to) {
                    add_loop(flow, &ask.from, &ask.to, ask.spec.clone())
                } else {
                    flow.clone()
                }
            },
            cx,
        );
    }

    pub(super) fn board_cancel_loop(&mut self, cx: &mut Context<Self>) {
        if let Some(b) = self.wf.board.as_mut() {
            b.loop_ask = None;
            cx.notify();
        }
    }

    /// Changes the loop being asked for, or the one leaving `node`.
    pub(super) fn board_loop_edit(
        &mut self,
        node: Option<&str>,
        f: impl FnOnce(&LoopSpec) -> LoopSpec,
        cx: &mut Context<Self>,
    ) {
        match node {
            None => {
                if let Some(ask) = self.wf.board.as_mut().and_then(|b| b.loop_ask.as_mut()) {
                    ask.spec = f(&ask.spec);
                    cx.notify();
                }
            }
            Some(id) => {
                let Some(spec) = self.wf.board.as_ref().and_then(|b| {
                    b.flow
                        .edges
                        .iter()
                        .find(|e| e.loop_spec.is_some() && e.source == id)
                        .and_then(|e| e.loop_spec.clone())
                }) else {
                    return;
                };
                let next = f(&spec);
                if next != spec {
                    let id = id.to_string();
                    self.board_flow(|flow, _| set_loop(flow, &id, Some(next)), cx);
                }
            }
        }
    }

    /// A new case at the end of the Switch `id`, with a name no case has.
    pub(super) fn board_add_case(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(board) = self.wf.board.as_ref() else {
            return;
        };
        let Some(NodeKind::Switch { cases, .. }) = board.node(id).map(|n| &n.kind) else {
            return;
        };
        let name = next_case_name(cases, &out_handles_taken(&board.flow, id));
        let id = id.to_string();
        self.board_flow(|flow, _| add_switch_case(flow, &id, &name), cx);
    }

    /// The case `case` of the Switch `id` and the lines that leave by it are removed.
    pub(super) fn board_remove_case(&mut self, id: &str, case: &str, cx: &mut Context<Self>) {
        let (id, case) = (id.to_string(), case.to_string());
        self.fields
            .remove(&BoardField::CaseName(id.clone(), case.clone()).key());
        self.board_flow(|flow, _| remove_switch_case(flow, &id, &case), cx);
    }

    pub(super) fn board_remove_loop(&mut self, node: &str, cx: &mut Context<Self>) {
        let id = node.to_string();
        self.board_flow(|flow, _| set_loop(flow, &id, None), cx);
    }

    // ---- fields -----------------------------------------------------------------------------

    /// A text field of the board, made the first time it is drawn and shown with `value` while
    /// the user is not typing in it.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn board_field(
        &mut self,
        spec: BoardField,
        label: &str,
        placeholder: &str,
        value: &str,
        multi: bool,
        max: Option<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<Input> {
        let key = spec.key();
        if let Some(field) = self.fields.get(&key).cloned() {
            // A committed field keeps what is typed until it is committed; it follows the stored
            // value only when that changed.
            let follows = if spec.commits() || spec == BoardField::NewInput {
                self.wf
                    .seeds
                    .insert(key.clone(), value.to_string())
                    .as_deref()
                    != Some(value)
            } else {
                true
            };
            if follows
                && field.read(cx).content.as_ref() != value
                && !field.focus_handle(cx).is_focused(window)
            {
                field.update(cx, |f, cx| f.set_text(value.to_string(), cx));
            }
            return field;
        }
        self.wf.seeds.insert(key.clone(), value.to_string());
        let field = cx.new(|cx| {
            let mut f = if multi {
                Input::markdown_editor(label, cx)
            } else {
                Input::field(label.to_string(), label.to_string(), content(0.35), cx)
            };
            f.set_font_size(12.);
            f.set_placeholder(placeholder.to_string(), cx);
            if let Some(max) = max {
                f.set_max_chars(max);
            }
            f.set_text(value.to_string(), cx);
            f
        });
        self.fields.insert(key, field.clone());
        let mut subs = Vec::new();
        if spec.commits() {
            let on_submit = spec.clone();
            subs.push(
                cx.subscribe(&field, move |this, _, _: &input::Submitted, cx| {
                    this.board_commit(&on_submit, cx)
                }),
            );
            let on_blur = spec.clone();
            subs.push(
                cx.on_blur(&field.focus_handle(cx), window, move |this, _, cx| {
                    this.board_commit(&on_blur, cx)
                }),
            );
        } else if spec == BoardField::NewInput {
            // A new input is declared by Enter or the + button, never by leaving the field.
            let on_submit = spec.clone();
            subs.push(
                cx.subscribe(&field, move |this, _, _: &input::Submitted, cx| {
                    this.board_commit(&on_submit, cx)
                }),
            );
            subs.push(cx.subscribe(&field, |_, _, _: &input::Changed, cx| cx.notify()));
        } else {
            subs.push(
                cx.subscribe(&field, move |this, input, _: &input::Changed, cx| {
                    let text = input.read(cx).content.to_string();
                    this.board_field_changed(&spec, text, cx);
                }),
            );
        }
        if let Some(b) = self.wf.board.as_mut() {
            b.subs.extend(subs);
        }
        field
    }

    fn board_field_changed(&mut self, spec: &BoardField, text: String, cx: &mut Context<Self>) {
        let some = |t: &str| (!t.is_empty()).then(|| t.to_string());
        match spec {
            BoardField::Name => self.board_meta_edit(|m| m.name = text, cx),
            BoardField::Description => self.board_meta_edit(|m| m.description = Some(text), cx),
            BoardField::Title(id) => self.board_node_edit(id, |n| n.title = some(&text), cx),
            BoardField::Time(id) => self.board_node_edit(
                id,
                |n| {
                    if let NodeKind::Schedule { schedule } = &mut n.kind {
                        schedule.time = text;
                    }
                },
                cx,
            ),
            BoardField::LoopCondition(node) => self.board_loop_edit(
                node.as_deref(),
                |spec| LoopSpec {
                    decide: text,
                    ..spec.clone()
                },
                cx,
            ),
            BoardField::Answer(id, at) => self.board_node_edit(
                id,
                |n| {
                    if let NodeKind::AskMono { answers, .. } = &mut n.kind
                        && let Some(slot) = answers.get_mut(*at)
                    {
                        *slot = text;
                    }
                },
                cx,
            ),
            BoardField::Limit(id) | BoardField::AtOnce(id) => {
                let number = if text.trim().is_empty() {
                    None
                } else {
                    let n = actions::js_number(&text);
                    if n.is_nan() {
                        return;
                    }
                    Some(n)
                };
                self.board_node_edit(
                    id,
                    |n| match &mut n.kind {
                        NodeKind::Session {
                            timeout_minutes, ..
                        }
                        | NodeKind::Command {
                            timeout_minutes, ..
                        } => *timeout_minutes = number,
                        NodeKind::ForEach { max_parallel, .. } => *max_parallel = number,
                        _ => {}
                    },
                    cx,
                );
            }
            BoardField::Prompt(id)
            | BoardField::Command(id)
            | BoardField::Question(id)
            | BoardField::Condition(id)
            | BoardField::IfQuestion(id)
            | BoardField::SwitchValue(id)
            | BoardField::SwitchQuestion(id)
            | BoardField::List(id)
            | BoardField::EachAs(id)
            | BoardField::Template(id) => self.board_node_edit(
                id,
                |n| match (&spec, &mut n.kind) {
                    (BoardField::Prompt(_), NodeKind::Session { prompt, .. }) => *prompt = text,
                    (BoardField::Command(_), NodeKind::Command { command, .. }) => *command = text,
                    (BoardField::Question(_), NodeKind::AskMono { question, .. }) => {
                        *question = text
                    }
                    (BoardField::Condition(_), NodeKind::If { condition, .. }) => {
                        *condition = Some(text)
                    }
                    (BoardField::IfQuestion(_), NodeKind::If { question, .. }) => {
                        *question = some(&text)
                    }
                    (BoardField::SwitchValue(_), NodeKind::Switch { value, .. }) => {
                        *value = Some(text)
                    }
                    (BoardField::SwitchQuestion(_), NodeKind::Switch { question, .. }) => {
                        *question = some(&text)
                    }
                    (BoardField::List(_), NodeKind::ForEach { list, .. }) => *list = text,
                    (BoardField::EachAs(_), NodeKind::ForEach { item, .. }) => *item = some(&text),
                    (BoardField::Template(_), NodeKind::Report { template }) => {
                        *template = some(&text)
                    }
                    _ => {}
                },
                cx,
            ),
            BoardField::OutputName(..)
            | BoardField::CaseName(..)
            | BoardField::InputValue(_)
            | BoardField::NewInput
            | BoardField::RequireName(_)
            | BoardField::RequireHint(_) => {}
        }
    }

    /// A committed field was left or submitted.
    pub(super) fn board_commit(&mut self, spec: &BoardField, cx: &mut Context<Self>) {
        let typed = self
            .fields
            .get(&spec.key())
            .map(|f| f.read(cx).content.to_string())
            .unwrap_or_default();
        let Some(b) = self.wf.board.as_ref() else {
            return;
        };
        let reset = |this: &mut Self, text: String, cx: &mut Context<Self>| {
            if let Some(f) = this.fields.get(&spec.key()).cloned() {
                f.update(cx, |f, cx| f.set_text(text, cx));
            }
        };
        match spec {
            BoardField::OutputName(node, from) => {
                let Some(NodeKind::Session { output, .. }) = b.node(node).map(|n| &n.kind) else {
                    return;
                };
                let taken = typed != *from && output.contains_key(&typed);
                if typed == *from {
                    return;
                }
                if !usable_name(&typed) || taken {
                    reset(self, from.clone(), cx);
                    return;
                }
                let (from, to) = (from.clone(), typed);
                // The field is rebuilt under its new name.
                self.fields.remove(&spec.key());
                self.board_node_edit(
                    node,
                    |n| {
                        if let NodeKind::Session { output, .. } = &mut n.kind {
                            *output = rename_field(output, &from, &to);
                        }
                    },
                    cx,
                );
            }
            BoardField::CaseName(node, from) => {
                let Some(NodeKind::Switch { cases, .. }) = b.node(node).map(|n| &n.kind) else {
                    return;
                };
                if typed == *from {
                    return;
                }
                // A blank, reserved or taken name goes back to the case it was.
                if !case_name_usable(&typed, cases, &out_handles_taken(&b.flow, node)) {
                    reset(self, from.clone(), cx);
                    return;
                }
                let (node, from) = (node.clone(), from.clone());
                // The field is rebuilt under its new name.
                self.fields.remove(&spec.key());
                self.board_flow(|flow, _| rename_switch_case(flow, &node, &from, &typed), cx);
            }
            BoardField::InputValue(name) => {
                let Some(input) = b.meta.inputs.get(name).cloned() else {
                    return;
                };
                // Left as it was shown: nothing to keep.
                if typed == value_text(b.meta.input_values.get(name)) {
                    return;
                }
                let value = match input.ty {
                    monocode_core::mono::workflows::model::InputType::Number
                        if typed.trim().is_empty() =>
                    {
                        None
                    }
                    monocode_core::mono::workflows::model::InputType::Number => {
                        Some(InputValue::Number(actions::js_number(&typed)))
                    }
                    _ => Some(InputValue::String(typed)),
                };
                self.board_set_input(name, &input, value, cx);
            }
            BoardField::RequireName(at) => {
                let name = typed.trim().to_string();
                let at = *at;
                if b.meta.requires.get(at).is_some_and(|r| r.name != name) {
                    self.board_meta_edit(|m| m.requires[at].name = name, cx);
                }
            }
            BoardField::RequireHint(at) => {
                let hint = (!typed.trim().is_empty()).then(|| typed.trim().to_string());
                let at = *at;
                if b.meta.requires.get(at).is_some_and(|r| r.hint != hint) {
                    self.board_meta_edit(|m| m.requires[at].hint = hint, cx);
                }
            }
            BoardField::NewInput => {
                let name = typed;
                let ty = b.new_input_type.clone();
                if name.is_empty()
                    || input_name_problem(&name, &|n| b.meta.inputs.contains_key(n)).is_some()
                {
                    return;
                }
                self.board_add_input(&name, &ty, cx);
                reset(self, String::new(), cx);
            }
            _ => {}
        }
    }

    /// Keeps a value for a declared input if it fits the type; otherwise says what is wrong.
    pub(super) fn board_set_input(
        &mut self,
        name: &str,
        input: &WorkflowInput,
        value: Option<InputValue>,
        cx: &mut Context<Self>,
    ) {
        let problem = actions::input_problem(name, input, value.as_ref());
        if let Some(b) = self.wf.board.as_mut() {
            b.input_problem = problem.clone();
        }
        match (problem, value) {
            (None, Some(value)) => {
                let name = name.to_string();
                self.board_meta_edit(|m| m.input_values.insert(name, value), cx);
            }
            _ => cx.notify(),
        }
    }

    /// A blank requirement row, a command to start with; its name is typed in the row.
    pub(super) fn board_add_requirement(&mut self, cx: &mut Context<Self>) {
        self.board_meta_edit(
            |m| {
                m.requires.push(Requirement {
                    kind: RequirementKind::Cli,
                    name: String::new(),
                    hint: None,
                })
            },
            cx,
        );
    }

    pub(super) fn board_remove_requirement(&mut self, at: usize, cx: &mut Context<Self>) {
        if self
            .wf
            .board
            .as_ref()
            .is_some_and(|b| at < b.meta.requires.len())
        {
            self.board_meta_edit(|m| drop(m.requires.remove(at)), cx);
        }
    }

    pub(super) fn board_add_input(&mut self, name: &str, ty: &str, cx: &mut Context<Self>) {
        use monocode_core::mono::workflows::model::InputType;
        let ty = InputType::ALL
            .iter()
            .find(|(n, _)| *n == ty)
            .map_or(InputType::String, |(_, t)| *t);
        let name = name.to_string();
        self.board_meta_edit(|m| m.inputs.insert(name, WorkflowInput::new(ty)), cx);
    }

    pub(super) fn board_remove_input(&mut self, name: &str, cx: &mut Context<Self>) {
        let name = name.to_string();
        self.board_meta_edit(
            |m| {
                m.inputs.remove(&name);
                m.input_values.remove(&name);
            },
            cx,
        );
    }

    /// A project chosen for a step: the step names an input holding it.
    pub(super) fn board_choose_project(
        &mut self,
        node: &str,
        path: &str,
        label: &str,
        cx: &mut Context<Self>,
    ) {
        let Some(b) = self.wf.board.as_ref() else {
            return;
        };
        let w = b.current();
        let Some(step) = w.node(node) else {
            return;
        };
        let choice = project_choice(&w, step, path, label);
        let next = choice.node.clone();
        self.board_flow(|flow, _| update_node(flow, &next), cx);
        self.board_meta_edit(
            |m| {
                m.inputs = choice.inputs;
                m.input_values = choice.input_values;
            },
            cx,
        );
    }

    // ---- saving and leaving -----------------------------------------------------------------

    /// Saves the draft over the stored copy, off the UI thread. `paused` saves it switched off.
    pub(super) fn board_save(
        &mut self,
        paused: bool,
        overwrite: bool,
        leave: bool,
        cx: &mut Context<Self>,
    ) {
        let Some(b) = self.wf.board.as_mut() else {
            return;
        };
        if b.saving {
            return;
        }
        let mut next = b.current();
        if paused {
            next.enabled = false;
        }
        let seen = b.seen.clone();
        b.saving = true;
        self.wf.page_notice = None;
        self.workflow_task(
            move |host| {
                use monocode_core::mono::workflows::cli::WorkflowHost;
                host.update(|host, list| {
                    let now = host.now();
                    match save_into(list.clone(), &next, overwrite, &seen, now) {
                        Err(SaveRefusal::Changed(stored)) => Ok((list, Saved::Changed(stored))),
                        Err(SaveRefusal::Message(message)) => Err(message),
                        Ok((saved, result)) => {
                            // The same gate as running it: an enabled workflow with problems is
                            // not saved, whatever the screen thought.
                            if result.enabled {
                                let issues = actions::issues_of(host, &result);
                                if !issues.is_empty() {
                                    return Err(actions::refusal("save", &issues));
                                }
                            }
                            Ok((saved, Saved::Done(Box::new(result))))
                        }
                    }
                })
            },
            move |this, result, cx| this.board_saved(result, leave, cx),
            cx,
        );
        cx.notify();
    }

    fn board_saved(&mut self, result: Result<Saved, String>, leave: bool, cx: &mut Context<Self>) {
        let Some(b) = self.wf.board.as_mut() else {
            return;
        };
        b.saving = false;
        match result {
            Ok(Saved::Done(saved)) => {
                let key = definition_key(&saved);
                b.is_new = false;
                b.baseline = key.clone();
                b.seen = key;
                b.elsewhere = None;
                b.leaving = false;
                // The save may have anchored an interval schedule; the board takes that, so it
                // is clean.
                for n in &mut b.flow.nodes {
                    let stored = saved.nodes.iter().find(|s| s.id == n.id);
                    if let (
                        NodeKind::Schedule { schedule },
                        Some(NodeKind::Schedule { schedule: kept }),
                    ) = (&mut n.data.node.kind, stored.map(|s| &s.kind))
                        && schedule.interval_from.is_none()
                    {
                        *schedule = kept.clone();
                    }
                }
                b.meta.enabled = saved.enabled;
                b.base = *saved;
                if leave {
                    self.board_back(cx);
                    return;
                }
                self.board_rebuild(cx);
            }
            Ok(Saved::Changed(stored)) => {
                b.elsewhere = Some((*stored, true));
                b.leaving = false;
                cx.notify();
            }
            Err(error) => {
                eprintln!("Could not save the workflow: {error}");
                let notice = if error.is_empty() {
                    "Could not save the workflow.".to_string()
                } else {
                    error
                };
                b.leaving = false;
                self.workflow_said(Some(notice), true, cx);
            }
        }
    }

    /// Takes a stored version in place of the draft.
    pub(super) fn board_load(&mut self, stored: Workflow, cx: &mut Context<Self>) {
        let Some(b) = self.wf.board.as_mut() else {
            return;
        };
        let key = definition_key(&stored);
        b.flow = to_flow(&stored);
        b.meta = Meta::of(&stored);
        b.base = stored;
        b.baseline = key.clone();
        b.seen = key;
        b.elsewhere = None;
        self.board_rebuild(cx);
    }

    /// The stored workflows were read again: follow an edit made elsewhere while the board is
    /// clean, otherwise ask.
    pub(super) fn board_sync(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.wf.board.as_ref().map(|b| b.current().id) else {
            return;
        };
        let Some(stored) = self.mono_workflow(&id).cloned() else {
            return;
        };
        let Some(b) = self.wf.board.as_mut() else {
            return;
        };
        // Our own save is in flight: whatever is read now is not news.
        if b.saving {
            return;
        }
        // A review (or a flag) changes no part of the definition, so the key does not see it.
        if b.base.needs_review != stored.needs_review {
            b.base.needs_review = stored.needs_review;
            cx.notify();
        }
        // A refused save waits for its choice: Save must keep refusing until then.
        if let Some((held, true)) = &mut b.elsewhere {
            *held = stored;
            return;
        }
        let key = definition_key(&stored);
        if key == b.seen {
            return;
        }
        b.seen = key.clone();
        if key == b.baseline {
            return;
        }
        if b.changed() {
            b.elsewhere = Some((stored, false));
            cx.notify();
        } else {
            self.board_load(stored, cx);
        }
    }

    pub(super) fn board_request_back(&mut self, cx: &mut Context<Self>) {
        match self.wf.board.as_mut() {
            Some(b) if b.changed() => {
                b.leaving = true;
                cx.notify();
            }
            _ => self.board_back(cx),
        }
    }

    /// Leaves the board for the page it came from.
    pub(super) fn board_back(&mut self, cx: &mut Context<Self>) {
        let id = self
            .wf
            .board
            .as_ref()
            .map(|b| (b.is_new, b.base.id.clone()));
        self.board_drop();
        self.wf.page_notice = None;
        self.route = match id {
            Some((false, id)) => Some(Route::Workflow(id)),
            _ => None,
        };
        self.refresh_workflows(cx);
        cx.notify();
    }

    pub(super) fn board_run_now(&mut self, cx: &mut Context<Self>) {
        let Some(b) = self.wf.board.as_ref() else {
            return;
        };
        let id = b.base.id.clone();
        if b.base.needs_review {
            let back = self.route.clone();
            self.open_workflow_review(&id, super::ReviewIntent::Run, back, cx);
            return;
        }
        if self.run_gate_for_run(&id, true, cx) {
            return;
        }
        self.run_workflow_now(&id, None, true, cx);
    }

    pub(super) fn board_toggle_enabled(&mut self, cx: &mut Context<Self>) {
        let Some(b) = self.wf.board.as_ref() else {
            return;
        };
        if b.base.needs_review && !b.meta.enabled {
            let (id, back) = (b.base.id.clone(), self.route.clone());
            self.open_workflow_review(&id, super::ReviewIntent::Enable, back, cx);
            return;
        }
        self.board_meta_edit(|m| m.enabled = !m.enabled, cx);
    }
}
