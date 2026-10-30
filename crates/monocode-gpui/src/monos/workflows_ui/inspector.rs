//! The page that slides over the board: one node's setup, or the workflow's own settings
//! (`NodeInspector.tsx`, `LoopForm`, `ProjectPicker`, the model picker).
use super::super::scheduler::{habit_field, habit_icon_button};
use super::super::*;
use super::Pick;
use super::board::{BoardField, BoardPick, InspectorView};
use super::page::basename;
use super::review::{node_label, node_title};
use super::view::{hint, property, section};
use gpui::{Div, FontWeight};
use monocode_core::mono::workflows::{
    board_edit::*,
    model::{InputType, InputValue, LoopSpec, Node, NodeKind, OutputFieldType, Workflow},
    models::stored_model_id,
    ordered::js_number_text,
};
use monocode_core::provider::Kind;
use monocode_core::store::native_model;

pub(super) const RED: u32 = 0xef4444;

pub(super) fn effort_name(value: &str) -> String {
    match value {
        "default" => "Default",
        "none" => "None",
        "minimal" => "Minimal",
        "low" => "Low",
        "medium" => "Medium",
        "high" => "High",
        "xhigh" => "Extra High",
        "max" => "Max",
        "ultra" => "Ultra",
        "ultracode" => "Ultracode",
        "ultrathink" => "Ultrathink",
        other => other,
    }
    .into()
}

pub(super) fn choices(items: &[(&str, &str)]) -> Vec<(String, String)> {
    items
        .iter()
        .map(|(v, l)| (v.to_string(), l.to_string()))
        .collect()
}

pub(super) const OUTPUT_TYPES: [(&str, &str); 5] = [
    ("string", "Text"),
    ("number", "Number"),
    ("boolean", "Yes / no"),
    ("list", "List"),
    ("object", "Object"),
];
pub(super) const INPUT_TYPES: [(&str, &str); 4] = [
    ("project", "Project"),
    ("string", "Text"),
    ("number", "Number"),
    ("boolean", "On / off"),
];

/// What the model rows of a session show: its harness, and the model in the forms the picker and
/// the file use.
pub(super) struct ModelState {
    pub(super) kind: Kind,
    /// The id the CLI is started with, when known.
    pub(super) native: Option<String>,
    /// The id the file stores for it.
    pub(super) stored: Option<String>,
    pub(super) unset: bool,
}

impl Monos {
    pub(super) fn board_model_state(&self, node: &Node, cx: &App) -> ModelState {
        let NodeKind::Session {
            harness,
            model,
            effort,
            ..
        } = &node.kind
        else {
            return ModelState {
                kind: Kind::Claude,
                native: None,
                stored: None,
                unset: true,
            };
        };
        let mono_kind = self.board_mono_kind(cx);
        let mono_model = self.chat().and_then(|c| c.read(cx).mono_model());
        let from_model = model
            .as_deref()
            .and_then(|m| m.split_once(':'))
            .and_then(|(h, _)| Kind::parse(h));
        let kind = harness
            .as_deref()
            .and_then(Kind::parse)
            .or(from_model)
            .or(mono_kind)
            .unwrap_or(Kind::Claude);
        let native = match model {
            Some(stored) => native_model(stored, kind.id()),
            None if Some(kind) == mono_kind => mono_model,
            None => None,
        };
        let stored = model
            .clone()
            .or_else(|| stored_model_id(kind, native.as_deref()));
        ModelState {
            kind,
            native,
            stored,
            unset: harness.is_none() && model.is_none() && effort.is_none(),
        }
    }

    pub(super) fn board_inspector(
        &mut self,
        mono: &Mono,
        view: InspectorView,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(b) = self.wf.board.as_ref() else {
            return div().into_any_element();
        };
        let w = b.current();
        let (title, node) = match &view {
            InspectorView::Settings => ("Workflow settings".to_string(), None),
            InspectorView::Node(id) => match w.node(id) {
                Some(n) => (node_label(&n.kind).to_string(), Some(n.clone())),
                None => return div().into_any_element(),
            },
        };
        let mut header = div()
            .h(px(40.))
            .flex_shrink_0()
            .border_b_1()
            .border_color(content(0.1))
            .pl_1p5()
            .pr_3()
            .flex()
            .items_center()
            .gap_1()
            .child(
                div()
                    .id("wf-inspector-back")
                    .debug_selector(|| "wf-inspector-back".into())
                    .role(Role::Button)
                    .aria_label("Back to board")
                    .cursor_pointer()
                    .h(px(24.))
                    .pl_1()
                    .pr_2()
                    .rounded_md()
                    .flex()
                    .items_center()
                    .gap_1()
                    .text_size(px(12.))
                    .text_color(content(0.6))
                    .hover(|s| s.bg(content(0.05)).text_color(content(1.)))
                    .child(icon("chevron-left", 14.))
                    .child("Back to board")
                    .on_click(cx.listener(|this, _, _, cx| this.board_view(None, cx))),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_right()
                    .text_size(px(11.))
                    .text_color(content(0.4))
                    .child(title),
            );
        let body = match &node {
            None => self.board_settings(mono, &w, window, cx),
            Some(n) => {
                let id = n.id.clone();
                header = header.child(
                    icon_button("wf-inspector-delete", "Delete node", "trash")
                        .debug_selector(|| "wf-inspector-delete".into())
                        .on_click(cx.listener(move |this, _, _, cx| this.board_remove(&id, cx))),
                );
                self.board_node_body(mono, &w, n, window, cx)
            }
        };
        div()
            .id("wf-inspector")
            .debug_selector(|| "wf-inspector".into())
            .absolute()
            .inset_0()
            .occlude()
            .bg(floating_fill())
            .flex()
            .flex_col()
            .child(header)
            .child(
                div()
                    .id("wf-inspector-body")
                    .w_full()
                    .min_w_0()
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .flex()
                    .flex_col()
                    .child(body),
            )
            .into_any_element()
    }

    // ---- small parts --------------------------------------------------------------------------

    #[allow(clippy::too_many_arguments)]
    pub(super) fn text_row(
        &mut self,
        label: &'static str,
        spec: BoardField,
        value: &str,
        placeholder: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let selector = spec.key();
        let field = self.board_field(spec, label, placeholder, value, false, None, window, cx);
        property(
            label,
            habit_field(field)
                .id(SharedString::from(selector.clone()))
                .debug_selector(move || selector.clone())
                .h(px(28.))
                .bg(content(0.1)),
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn long_text(
        &mut self,
        title: &'static str,
        spec: BoardField,
        value: &str,
        placeholder: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let selector = spec.key();
        let field = self.board_field(spec, title, placeholder, value, true, None, window, cx);
        section(title, None).child(
            habit_field(field)
                .id(SharedString::from(selector.clone()))
                .debug_selector(move || selector.clone()),
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn pick_row(
        &mut self,
        label: &'static str,
        pick: BoardPick,
        selected: &str,
        options: Vec<(String, String)>,
        current: &str,
        cx: &mut Context<Self>,
    ) -> Div {
        let id = format!("wf-pick:{pick:?}");
        let picker = self.workflow_picker(
            id,
            label.to_string(),
            selected.to_string(),
            options,
            current.to_string(),
            Pick::Board(pick),
            cx,
        );
        property(label, picker)
    }

    pub(super) fn label_of(options: &[(String, String)], value: &str) -> String {
        options
            .iter()
            .find(|(v, _)| v == value)
            .map_or_else(|| value.to_string(), |(_, l)| l.clone())
    }

    /// A row with a drop-down whose label is the chosen option's.
    pub(super) fn choice_row(
        &mut self,
        label: &'static str,
        pick: BoardPick,
        options: Vec<(String, String)>,
        current: &str,
        cx: &mut Context<Self>,
    ) -> Div {
        let selected = Self::label_of(&options, current);
        self.pick_row(label, pick, &selected, options, current, cx)
    }

    pub(super) fn switch_row(
        &self,
        id: &'static str,
        label: &'static str,
        description: &'static str,
        on: bool,
        change: impl Fn(&mut Self, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) -> Div {
        div()
            .px_2()
            .py_2()
            .flex()
            .items_center()
            .gap_3()
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .text_size(px(13.))
                            .line_height(px(20.))
                            .text_color(content(0.9))
                            .child(label),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .line_height(px(18.))
                            .text_color(content(0.4))
                            .child(description),
                    ),
            )
            .child(
                div()
                    .id(id)
                    .debug_selector(move || id.into())
                    .role(Role::Switch)
                    .aria_label(label)
                    .aria_selected(on)
                    .cursor_pointer()
                    .w(px(36.))
                    .h(px(20.))
                    .flex_shrink_0()
                    .rounded_full()
                    .bg(if on { accent_color() } else { content(0.2) })
                    .child(
                        div()
                            .size(px(16.))
                            .mt(px(2.))
                            .ml(px(if on { 18. } else { 2. }))
                            .rounded_full()
                            .bg(gpui::white()),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| change(this, cx))),
            )
    }

    pub(super) fn section_button(
        id: &'static str,
        label: &'static str,
        glyph: &'static str,
    ) -> gpui::Stateful<Div> {
        habit_icon_button(SharedString::from(id), label, glyph).debug_selector(move || id.into())
    }

    // ---- the loop form --------------------------------------------------------------------------

    /// How many rounds a loop may take and who decides to go round again. `node` is the node the
    /// loop leaves, or None for the loop being asked for.
    pub(super) fn board_loop_form(
        &mut self,
        node: Option<String>,
        spec: &LoopSpec,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let rounds: Vec<(String, String)> =
            (1..=10).map(|n| (n.to_string(), n.to_string())).collect();
        let by_mono = spec.decide == "mono";
        let mut form = div()
            .flex()
            .flex_col()
            .gap_0p5()
            .child(self.choice_row(
                "Max rounds",
                BoardPick::MaxRounds(node.clone()),
                rounds,
                &js_number_text(spec.max_rounds),
                cx,
            ))
            .child(self.choice_row(
                "Decided by",
                BoardPick::Decider(node.clone()),
                choices(&[("mono", "Mono decides"), ("condition", "A condition")]),
                if by_mono { "mono" } else { "condition" },
                cx,
            ));
        if !by_mono {
            form = form.child(self.text_row(
                "Condition",
                BoardField::LoopCondition(node),
                &spec.decide,
                "{{review.verdict}} != \"approve\"",
                window,
                cx,
            ));
        }
        form
    }

    // ---- a node ---------------------------------------------------------------------------------

    pub(super) fn board_node_body(
        &mut self,
        mono: &Mono,
        w: &Workflow,
        node: &Node,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let id = node.id.clone();
        let title_field = self.board_field(
            BoardField::Title(id.clone()),
            "Title",
            &node_title(&Node {
                title: None,
                ..node.clone()
            }),
            node.title.as_deref().unwrap_or(""),
            false,
            Some(80),
            window,
            cx,
        );
        let issues: Vec<String> = self
            .wf
            .board
            .as_ref()
            .map(|b| {
                b.issues
                    .iter()
                    .filter(|i| i.node_id.as_deref() == Some(id.as_str()))
                    .map(|i| i.message.clone())
                    .collect()
            })
            .unwrap_or_default();
        let results = matches!(
            node.kind,
            NodeKind::Session { .. } | NodeKind::Command { .. } | NodeKind::AskMono { .. }
        );
        let fields = self.board_node_fields(mono, w, node, window, cx);
        let loop_out = w
            .edges
            .iter()
            .find(|e| e.loop_spec.is_some() && e.from == id);
        let looped = loop_out.and_then(|e| {
            e.loop_spec.clone().map(|spec| {
                let target = w.node(&e.to).map(node_title).unwrap_or_default();
                (spec, target)
            })
        });
        let loop_section = looped.map(|(spec, target)| {
            let form = self.board_loop_form(Some(id.clone()), &spec, window, cx);
            let remove = id.clone();
            section(
                format!("Loops back to {target}"),
                Some(
                    div()
                        .id("wf-loop-remove")
                        .debug_selector(|| "wf-loop-remove".into())
                        .role(Role::Button)
                        .aria_label("Remove loop")
                        .cursor_pointer()
                        .rounded_sm()
                        .px_1p5()
                        .py_0p5()
                        .text_size(px(11.))
                        .text_color(content(0.6))
                        .hover(|s| s.bg(content(0.08)).text_color(content(1.)))
                        .child("Remove loop")
                        .on_click(
                            cx.listener(move |this, _, _, cx| this.board_remove_loop(&remove, cx)),
                        )
                        .into_any_element(),
                ),
            )
            .child(div().px_2().flex().flex_col().child(form))
        });
        div()
            .flex()
            .flex_col()
            .child(
                div()
                    .px_2()
                    .pt_3()
                    .child(
                        div()
                            .id("wf-inspector-title")
                            .debug_selector(|| "wf-inspector-title".into())
                            .h(px(32.))
                            .px_2()
                            .rounded_md()
                            .hover(|s| s.bg(content(0.05)))
                            .flex()
                            .items_center()
                            .font_weight(FontWeight::MEDIUM)
                            .child(title_field),
                    )
                    .when(results, |d| {
                        d.child(
                            div()
                                .px_2()
                                .pt_0p5()
                                .text_size(px(11.))
                                .line_height(px(16.))
                                .text_color(content(0.35))
                                .child(format!(
                                    "Later steps use its results as {{{{{id}.name}}}}."
                                )),
                        )
                    }),
            )
            .when(!issues.is_empty(), |d| {
                d.child(
                    div()
                        .id("wf-inspector-issues")
                        .debug_selector(|| "wf-inspector-issues".into())
                        .px_4()
                        .pt_2()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .children(issues.into_iter().map(|message| {
                            div()
                                .flex()
                                .items_start()
                                .gap_1p5()
                                .text_size(px(11.))
                                .line_height(px(16.))
                                .text_color(gpui::Hsla::from(rgb(RED)).opacity(0.8))
                                .child(
                                    div()
                                        .mt(px(6.))
                                        .size(px(6.))
                                        .flex_shrink_0()
                                        .rounded_full()
                                        .bg(gpui::Hsla::from(rgb(RED)).opacity(0.7)),
                                )
                                .child(message)
                        })),
                )
            })
            .child(fields)
            .children(loop_section)
            .into_any_element()
    }

    pub(super) fn board_node_fields(
        &mut self,
        mono: &Mono,
        w: &Workflow,
        node: &Node,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let id = node.id.clone();
        let props = || {
            div()
                .w_full()
                .min_w_0()
                .flex_shrink_0()
                .px_4()
                .py_3()
                .flex()
                .flex_col()
                .gap_0p5()
        };
        match &node.kind {
            NodeKind::Schedule { schedule } => {
                let controls = self.board_schedule(&id, schedule, window, cx);
                props()
                    .child(property("Runs", controls))
                    .into_any_element()
            }
            NodeKind::Manual => div()
                .px_4()
                .py_3()
                .text_size(px(12.))
                .line_height(px(20.))
                .text_color(content(0.55))
                .child(
                    "Starts when you use Run now, or when the Mono runs it. Values it needs are set under Workflow settings.",
                )
                .into_any_element(),
            NodeKind::Session {
                prompt,
                checkout,
                output,
                reuse_session,
                nest_under,
                timeout_minutes,
                ..
            } => {
                let project_row = self.board_project_row(mono, w, &id, cx);
                let model = self.board_model_rows(node, cx);
                let others = sessions_except(w, &id);
                let mut worktrees = choices(&[("root", "Project folder"), ("newWorktree", "New worktree")]);
                worktrees.extend(others.iter().map(|s| {
                    (format!("worktree:{}", s.id), format!("Worktree of {}", node_title(s)))
                }));
                let mut nests = choices(&[("", "Nothing")]);
                nests.extend(others.iter().map(|s| (s.id.clone(), node_title(s))));
                let checkout_row = self.choice_row(
                    "Checkout",
                    BoardPick::Checkout(id.clone()),
                    worktrees,
                    &checkout_value(checkout),
                    cx,
                );
                let nest_row = self.choice_row(
                    "Nest under",
                    BoardPick::NestUnder(id.clone()),
                    nests,
                    nest_under.as_deref().unwrap_or(""),
                    cx,
                );
                let limit = self.board_limit_row(
                    &id,
                    *timeout_minutes,
                    "120 min",
                    window,
                    cx,
                );
                let reuse = in_loop(w, &id).then(|| {
                    let on = *reuse_session != Some(false);
                    let toggled = id.clone();
                    self.switch_row(
                        "wf-reuse-session",
                        "Reuse the session",
                        "Keep one session across the rounds of its loop",
                        on,
                        move |this, cx| {
                            this.board_node_edit(
                                &toggled,
                                |n| {
                                    if let NodeKind::Session { reuse_session, .. } = &mut n.kind {
                                        *reuse_session = Some(!on);
                                    }
                                },
                                cx,
                            )
                        },
                        cx,
                    )
                });
                let prompt_section = self.long_text(
                    "Prompt",
                    BoardField::Prompt(id.clone()),
                    prompt,
                    "What the session should do. Use {{inputs.x}} and {{node.field}}.",
                    window,
                    cx,
                );
                let outputs = self.board_outputs(&id, output, window, cx);
                div()
                    .flex()
                    .flex_col()
                    .child(
                        props()
                            .child(project_row)
                            .children(model)
                            .child(checkout_row)
                            .child(nest_row)
                            .child(limit),
                    )
                    .children(reuse.map(|r| div().px_2().child(r)))
                    .child(prompt_section)
                    .child(outputs)
                    .into_any_element()
            }
            NodeKind::Command {
                command,
                cwd,
                timeout_minutes,
                ..
            } => {
                let project_row = self.board_project_row(mono, w, &id, cx);
                let mut runs = choices(&[("root", "Project folder")]);
                runs.extend(sessions_except(w, &id).iter().map(|s| {
                    (format!("worktree:{}", s.id), format!("Worktree of {}", node_title(s)))
                }));
                let runs_row = self.choice_row(
                    "Runs in",
                    BoardPick::RunsIn(id.clone()),
                    runs,
                    &cwd_value(cwd.as_ref()),
                    cx,
                );
                let limit = self.board_limit_row(
                    &id,
                    *timeout_minutes,
                    "10 min",
                    window,
                    cx,
                );
                let command_section = self.long_text(
                    "Command",
                    BoardField::Command(id.clone()),
                    command,
                    "npm test",
                    window,
                    cx,
                );
                let mut out = div()
                    .flex()
                    .flex_col()
                    .child(props().child(project_row).child(runs_row).child(limit))
                    .child(command_section)
                    .child(hint(
                        "Fills in from {{ }} are quoted for the shell. It reports exitCode, stdout and stderr.",
                    ));
                if cfg!(windows) {
                    out = out.child(hint(
                        "Command nodes are not supported on Windows yet, so this step will fail when it runs.",
                    ));
                }
                out.into_any_element()
            }
            NodeKind::AskMono { question, answers } => {
                let q = self.long_text(
                    "Question",
                    BoardField::Question(id.clone()),
                    question,
                    "What the Mono should decide. Use {{node.field}} to include results.",
                    window,
                    cx,
                );
                let rows: Vec<AnyElement> = answers
                    .iter()
                    .enumerate()
                    .map(|(at, answer)| {
                        let spec = BoardField::Answer(id.clone(), at);
                        let selector = spec.key();
                        let field =
                            self.board_field(spec, "Answer", "", answer, false, None, window, cx);
                        let (node_id, remove_id) = (id.clone(), format!("wf-answer-remove-{at}"));
                        div()
                            .flex()
                            .items_center()
                            .gap_1p5()
                            .child(
                                div().flex_1().min_w_0().child(
                                    habit_field(field)
                                        .id(SharedString::from(selector.clone()))
                                        .debug_selector(move || selector.clone())
                                        .h(px(28.))
                                        .bg(content(0.1)),
                                ),
                            )
                            .child(
                                habit_icon_button(
                                    SharedString::from(remove_id.clone()),
                                    "Remove answer",
                                    "close",
                                )
                                .debug_selector(move || remove_id.clone())
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.board_node_edit(
                                        &node_id,
                                        |n| {
                                            if let NodeKind::AskMono { answers, .. } = &mut n.kind
                                                && at < answers.len()
                                            {
                                                answers.remove(at);
                                            }
                                        },
                                        cx,
                                    )
                                })),
                            )
                            .into_any_element()
                    })
                    .collect();
                let add_id = id.clone();
                let add = Self::section_button("wf-answer-add", "Add answer", "plus").on_click(
                    cx.listener(move |this, _, _, cx| {
                        this.board_node_edit(
                            &add_id,
                            |n| {
                                if let NodeKind::AskMono { answers, .. } = &mut n.kind {
                                    answers.push(String::new());
                                }
                            },
                            cx,
                        )
                    }),
                );
                div()
                    .flex()
                    .flex_col()
                    .child(q)
                    .child(
                        section("Answers", Some(add.into_any_element()))
                            .child(div().px_1().flex().flex_col().gap_1().children(rows)),
                    )
                    .into_any_element()
            }
            NodeKind::Switch {
                value,
                cases,
                decide,
                question,
            } => {
                let by_mono = decide.is_some();
                let decider = self.choice_row(
                    "Decided by",
                    BoardPick::SwitchDecider(id.clone()),
                    choices(&[("value", "A value"), ("mono", "Mono decides")]),
                    if by_mono { "mono" } else { "value" },
                    cx,
                );
                let mut rows = props().child(decider);
                if !by_mono {
                    rows = rows.child(self.text_row(
                        "Value",
                        BoardField::SwitchValue(id.clone()),
                        value.as_deref().unwrap_or(""),
                        "{{review.verdict}}",
                        window,
                        cx,
                    ));
                }
                let tail = if by_mono {
                    self.long_text(
                        "Question",
                        BoardField::SwitchQuestion(id.clone()),
                        question.as_deref().unwrap_or(""),
                        "What the Mono should check before choosing a case.",
                        window,
                        cx,
                    )
                    .into_any_element()
                } else {
                    hint("The line that leaves is the case equal to the value, else default.")
                        .into_any_element()
                };
                let cases = self.board_cases(&id, cases, window, cx);
                div()
                    .flex()
                    .flex_col()
                    .child(rows)
                    .child(tail)
                    .child(cases)
                    .into_any_element()
            }
            NodeKind::If {
                condition,
                decide,
                question,
            } => {
                let by_mono = decide.is_some();
                let decider = self.choice_row(
                    "Decided by",
                    BoardPick::IfDecider(id.clone()),
                    choices(&[("condition", "A condition"), ("mono", "Mono decides")]),
                    if by_mono { "mono" } else { "condition" },
                    cx,
                );
                let mut rows = props().child(decider);
                if !by_mono {
                    rows = rows.child(self.text_row(
                        "Condition",
                        BoardField::Condition(id.clone()),
                        condition.as_deref().unwrap_or(""),
                        "{{review.verdict}} == \"approve\"",
                        window,
                        cx,
                    ));
                }
                let tail = if by_mono {
                    self.long_text(
                        "Question",
                        BoardField::IfQuestion(id.clone()),
                        question.as_deref().unwrap_or(""),
                        "What the Mono should check before choosing true or false.",
                        window,
                        cx,
                    )
                    .into_any_element()
                } else {
                    hint("Operators: ==, !=, contains, empty, not empty.").into_any_element()
                };
                div().flex().flex_col().child(rows).child(tail).into_any_element()
            }
            NodeKind::ForEach {
                list,
                item,
                max_parallel,
            } => {
                let at_once = max_parallel.map(js_number_text).unwrap_or_default();
                let rows = props()
                    .child(self.text_row(
                        "List",
                        BoardField::List(id.clone()),
                        list,
                        "{{fetch.tickets}}",
                        window,
                        cx,
                    ))
                    .child(self.text_row(
                        "Each as",
                        BoardField::EachAs(id.clone()),
                        item.as_deref().unwrap_or(""),
                        "item",
                        window,
                        cx,
                    ))
                    .child(self.text_row(
                        "At once",
                        BoardField::AtOnce(id.clone()),
                        &at_once,
                        "3",
                        window,
                        cx,
                    ));
                div()
                    .flex()
                    .flex_col()
                    .child(rows)
                    .child(hint("Drag nodes into the frame on the board to run them once per item."))
                    .into_any_element()
            }
            NodeKind::Report { template } => self
                .long_text(
                    "Report",
                    BoardField::Template(id.clone()),
                    template.as_deref().unwrap_or(""),
                    "Leave empty for a summary of every branch, or write your own with {{node.field}}.",
                    window,
                    cx,
                )
                .into_any_element(),
        }
    }

    pub(super) fn board_limit_row(
        &mut self,
        id: &str,
        value: Option<f64>,
        placeholder: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let text = value.map(js_number_text).unwrap_or_default();
        self.text_row(
            "Time limit",
            BoardField::Limit(id.to_string()),
            &text,
            placeholder,
            window,
            cx,
        )
    }

    pub(super) fn board_project_row(
        &mut self,
        mono: &Mono,
        w: &Workflow,
        id: &str,
        cx: &mut Context<Self>,
    ) -> Div {
        let node = w.node(id);
        let current = node.and_then(|n| project_path(w, n)).unwrap_or_default();
        let mut options: Vec<(String, String)> = mono
            .projects
            .iter()
            .map(|p| (p.clone(), basename(p)))
            .collect();
        if !current.is_empty() && !options.iter().any(|(p, _)| *p == current) {
            options.insert(0, (current.clone(), basename(&current)));
        }
        let selected = options
            .iter()
            .find(|(p, _)| *p == current)
            .map_or_else(|| "Choose a project".to_string(), |(_, l)| l.clone());
        self.pick_row(
            "Project",
            BoardPick::Project(id.to_string()),
            &selected,
            options,
            &current,
            cx,
        )
    }

    // ---- what a drop-down chose ---------------------------------------------------------------------

    pub(super) fn board_picked(&mut self, pick: &BoardPick, value: &str, cx: &mut Context<Self>) {
        let v = value.to_string();
        match pick {
            BoardPick::Checkout(id) => self.board_node_edit(
                id,
                |n| {
                    if let NodeKind::Session { checkout, .. } = &mut n.kind {
                        *checkout = parse_checkout(&v);
                    }
                },
                cx,
            ),
            BoardPick::NestUnder(id) => self.board_node_edit(
                id,
                |n| {
                    if let NodeKind::Session { nest_under, .. } = &mut n.kind {
                        *nest_under = (!v.is_empty()).then(|| v.clone());
                    }
                },
                cx,
            ),
            BoardPick::RunsIn(id) => self.board_node_edit(
                id,
                |n| {
                    if let NodeKind::Command { cwd, .. } = &mut n.kind {
                        *cwd = parse_cwd(&v);
                    }
                },
                cx,
            ),
            BoardPick::Effort(id) => {
                let Some(step) = self.wf.board.as_ref().and_then(|b| b.node(id).cloned()) else {
                    return;
                };
                // An effort belongs to one model, so choosing it fixes the model it was chosen for.
                let state = self.board_model_state(&step, cx);
                self.board_node_edit(
                    id,
                    |n| {
                        if let NodeKind::Session {
                            harness,
                            model,
                            effort,
                            ..
                        } = &mut n.kind
                        {
                            *harness = Some(state.kind.id().to_string());
                            *model = state.stored.clone();
                            *effort = Some(v.clone());
                        }
                    },
                    cx,
                )
            }
            BoardPick::Project(id) => {
                let label = basename(&v);
                self.board_choose_project(id, &v, &label, cx)
            }
            BoardPick::MaxRounds(node) => {
                let rounds = v.parse::<f64>().unwrap_or(3.);
                self.board_loop_edit(
                    node.as_deref(),
                    |s| LoopSpec {
                        max_rounds: rounds,
                        ..s.clone()
                    },
                    cx,
                )
            }
            BoardPick::Decider(node) => {
                self.board_loop_edit(node.as_deref(), |s| loop_decided_by(s, v == "mono"), cx)
            }
            BoardPick::SwitchDecider(id) => {
                self.board_node_edit(id, |n| *n = switch_decided_by(n, v == "mono"), cx)
            }
            BoardPick::IfDecider(id) => {
                self.board_node_edit(id, |n| *n = if_decided_by(n, v == "mono"), cx)
            }
            BoardPick::OutputType(id, field) => self.board_node_edit(
                id,
                |n| {
                    if let NodeKind::Session { output, .. } = &mut n.kind
                        && let Some(f) = output.get_mut(field)
                        && let Some((_, ty)) =
                            OutputFieldType::ALL.iter().find(|(name, _)| *name == v)
                    {
                        f.ty = *ty;
                    }
                },
                cx,
            ),
            BoardPick::ScheduleKind(id) => self.board_node_edit(
                id,
                |n| {
                    if let NodeKind::Schedule { schedule } = &mut n.kind {
                        schedule.schedule_kind = v.clone();
                        schedule.interval_from = None;
                        schedule.interval_hours = if v == "interval" {
                            Some(schedule.interval_hours.unwrap_or(4))
                        } else {
                            None
                        };
                    }
                },
                cx,
            ),
            BoardPick::ScheduleHours(id) => self.board_node_edit(
                id,
                |n| {
                    if let NodeKind::Schedule { schedule } = &mut n.kind {
                        schedule.interval_hours = v.parse().ok();
                    }
                },
                cx,
            ),
            BoardPick::ScheduleDay(id) => self.board_node_edit(
                id,
                |n| {
                    if let NodeKind::Schedule { schedule } = &mut n.kind {
                        schedule.day_of_week = v.parse().unwrap_or(1);
                    }
                },
                cx,
            ),
            BoardPick::ScheduleMinute(id) => self.board_node_edit(
                id,
                |n| {
                    if let NodeKind::Schedule { schedule } = &mut n.kind {
                        schedule.minute = v.parse().unwrap_or(0);
                    }
                },
                cx,
            ),
            BoardPick::InputValue(name) => {
                let Some(input) = self
                    .wf
                    .board
                    .as_ref()
                    .and_then(|b| b.meta.inputs.get(name).cloned())
                else {
                    return;
                };
                let value = match input.ty {
                    InputType::Boolean => InputValue::Bool(v == "on"),
                    _ => InputValue::String(v),
                };
                self.board_set_input(name, &input, Some(value), cx)
            }
            BoardPick::NewInputType => {
                if let Some(b) = self.wf.board.as_mut() {
                    b.new_input_type = v;
                    cx.notify();
                }
            }
        }
    }
}
