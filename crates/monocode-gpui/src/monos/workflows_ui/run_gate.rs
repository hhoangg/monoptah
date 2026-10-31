//! The dialog that stands between "Run" or "Resume" and the runner when there is something to
//! ask or to check first: the values this run takes (never written back to the workflow) and the
//! requirements the machine has to meet. Checking runs off the UI thread, and only the newest
//! check may answer.
use super::super::scheduler::habit_field;
use super::super::*;
use super::Pick;
use super::actions;
use super::inspector::choices;
use super::page::{basename, value_text};
use super::view::{ERROR, property};
use gpui::{Div, FontWeight};
use monocode_core::mono::workflows::{
    cli::WorkflowHost,
    model::{InputType, InputValue, Requirement, Workflow, WorkflowInput, with_run_values},
    ordered::OrderedMap,
    preflight::{Preflight, union_requires},
};

/// What the dialog is for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum GateMode {
    Run,
    /// Continue the interrupted run with this id.
    Resume(String),
}

/// The open Run or Resume dialog.
pub(super) struct RunGate {
    pub(super) workflow_id: String,
    pub(super) mode: GateMode,
    /// Whether a notice goes under a page's header rather than under the list.
    pub(super) page: bool,
    /// Where it was opened; it is shown only there.
    pub(super) route: Option<Route>,
    /// The workflow whose requirements are checked: the stored one for a run, the run's own
    /// definition with the workflow's requirements added for a resume.
    pub(super) subject: Workflow,
    /// The declared inputs the form asks for (a run only), with the values stored now.
    pub(super) inputs: Vec<(String, WorkflowInput)>,
    pub(super) stored: OrderedMap<InputValue>,
    /// The project and on/off values chosen so far. Text and number values live in `fields`.
    pub(super) values: OrderedMap<InputValue>,
    /// The text and number inputs: `(input name, key of its field)`.
    pub(super) fields: Vec<(String, String)>,
    pub(super) checking: bool,
    pub(super) preflight: Option<Preflight>,
    /// What the runner refused with, shown in the dialog.
    pub(super) notice: Option<String>,
    /// The start asked for and not answered yet: the dialog stays open until it is.
    pub(super) submitting: Option<u64>,
    /// Keeps typing in the dialog; taken once when it is drawn.
    pub(super) focus: Option<gpui::FocusHandle>,
    focus_pending: bool,
    /// The check that may still answer.
    generation: u64,
    /// Typing in a field and submitting it; they go with the dialog.
    subs: Vec<gpui::Subscription>,
}

impl RunGate {
    fn requires(&self) -> &[Requirement] {
        &self.subject.requires
    }

    /// What `form` (the value of each input as the form holds it) gets wrong, one line each.
    pub(super) fn problems(&self, form: &OrderedMap<InputValue>) -> Vec<String> {
        self.inputs
            .iter()
            .filter_map(|(name, input)| actions::input_problem(name, input, form.get(name)))
            .collect()
    }

    /// The values that differ from the stored ones: the only ones the run is given.
    pub(super) fn overrides(
        &self,
        form: &OrderedMap<InputValue>,
    ) -> Option<OrderedMap<InputValue>> {
        let changed: OrderedMap<InputValue> = form
            .iter()
            .filter(|(name, value)| self.stored.get(name) != Some(*value))
            .map(|(name, value)| (name.clone(), value.clone()))
            .collect();
        (!changed.is_empty()).then_some(changed)
    }

    /// Whether the primary button works: nothing is being checked, nothing is missing and the
    /// values fit.
    pub(super) fn ready(&self, form: &OrderedMap<InputValue>) -> bool {
        let met = self.requires().is_empty()
            || self.preflight.as_ref().is_some_and(|check| check.all_met());
        !self.checking && self.submitting.is_none() && met && self.problems(form).is_empty()
    }
}

fn field_key(name: &str) -> String {
    format!("wf-gate:{name}")
}

impl Monos {
    /// Starts a run of the stored workflow `id` now, with `values` for this run only.
    pub(super) fn run_workflow_now(
        &mut self,
        id: &str,
        values: Option<OrderedMap<InputValue>>,
        page: bool,
        cx: &mut Context<Self>,
    ) {
        let id = id.to_string();
        self.workflow_task(
            move |host| actions::run_now(host, &id, values),
            move |this, notice, cx| this.workflow_said(notice, page, cx),
            cx,
        );
    }

    /// Clicking Run on an unflagged workflow: opens the dialog when there are inputs to confirm
    /// or requirements to check. False when the run can start directly.
    pub(super) fn run_gate_for_run(
        &mut self,
        id: &str,
        page: bool,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(w) = self.mono_workflow(id).cloned() else {
            return false;
        };
        if w.inputs.is_empty() && w.requires.is_empty() {
            return false;
        }
        let mut gate = RunGate {
            workflow_id: id.to_string(),
            mode: GateMode::Run,
            page,
            route: self.route.clone(),
            inputs: w
                .inputs
                .iter()
                .map(|(n, i)| (n.clone(), i.clone()))
                .collect(),
            stored: w.input_values.clone(),
            values: w.input_values.clone(),
            fields: Vec::new(),
            checking: false,
            preflight: None,
            notice: None,
            submitting: None,
            focus: None,
            focus_pending: false,
            generation: 0,
            subs: Vec::new(),
            subject: w,
        };
        gate.fields = gate
            .inputs
            .iter()
            .filter(|(_, input)| matches!(input.ty, InputType::String | InputType::Number))
            .map(|(name, _)| (name.clone(), field_key(name)))
            .collect();
        self.open_run_gate(gate, cx);
        true
    }

    /// Clicking Resume: opens the dialog when the run or the workflow requires anything.
    pub(super) fn run_gate_for_resume(
        &mut self,
        id: &str,
        run: &str,
        page: bool,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(w) = self.mono_workflow(id).cloned() else {
            return false;
        };
        let mut subject = self
            .workflow_runs_of(id)
            .and_then(|runs| runs.iter().find(|r| r.id == run))
            .map_or_else(|| w.clone(), |r| r.definition.clone());
        subject.requires = union_requires(&subject.requires, &w.requires);
        if subject.requires.is_empty() {
            return false;
        }
        let gate = RunGate {
            workflow_id: id.to_string(),
            mode: GateMode::Resume(run.to_string()),
            page,
            route: self.route.clone(),
            subject,
            inputs: Vec::new(),
            stored: OrderedMap::new(),
            values: OrderedMap::new(),
            fields: Vec::new(),
            checking: false,
            preflight: None,
            notice: None,
            submitting: None,
            focus: None,
            focus_pending: false,
            generation: 0,
            subs: Vec::new(),
        };
        self.open_run_gate(gate, cx);
        true
    }

    fn open_run_gate(&mut self, gate: RunGate, cx: &mut Context<Self>) {
        // The fields are made again from the stored values each time.
        self.fields.retain(|key, _| !key.starts_with("wf-gate:"));
        let mut gate = gate;
        for (name, key) in gate.fields.clone() {
            let input = gate.inputs.iter().find(|(n, _)| *n == name).map(|(_, i)| i);
            let placeholder = input
                .and_then(|i| i.description.clone())
                .unwrap_or_else(|| name.clone());
            let field = self.field(
                &key,
                &placeholder,
                value_text(gate.stored.get(&name)),
                false,
                cx,
            );
            gate.subs
                .push(cx.subscribe(&field, |_, _, _: &input::Changed, cx| cx.notify()));
            gate.subs
                .push(cx.subscribe(&field, |this, _, _: &input::Submitted, cx| {
                    this.run_gate_submit(cx)
                }));
        }
        gate.focus = Some(cx.focus_handle());
        gate.focus_pending = true;
        self.wf.gate = Some(gate);
        self.run_gate_check(cx);
        cx.notify();
    }

    /// Whether the dialog is open on the page in view.
    pub(crate) fn wf_gate_shown(&self) -> bool {
        self.active.is_some() && self.wf.gate.as_ref().is_some_and(|g| g.route == self.route)
    }

    pub(crate) fn wf_gate_close(&mut self, cx: &mut Context<Self>) {
        self.run_gate_close(cx);
    }

    /// Opens the Run dialog of `w` as a click on Run would.
    #[cfg(test)]
    pub(crate) fn open_gate_for_test(&mut self, w: Workflow, cx: &mut Context<Self>) -> bool {
        let Some(mono) = self.active.clone() else {
            return false;
        };
        let id = w.id.clone();
        self.wf.loaded.insert(mono, vec![w]);
        self.run_gate_for_run(&id, false, cx)
    }

    pub(super) fn run_gate_close(&mut self, cx: &mut Context<Self>) {
        if self.wf.gate.take().is_some() {
            self.fields.retain(|key, _| !key.starts_with("wf-gate:"));
            cx.notify();
        }
    }

    /// The value of every input as the form holds it. A text input is always a value; an empty
    /// number is none.
    pub(super) fn run_gate_form(&self, cx: &App) -> OrderedMap<InputValue> {
        let Some(gate) = self.wf.gate.as_ref() else {
            return OrderedMap::new();
        };
        let mut form = OrderedMap::new();
        for (name, input) in &gate.inputs {
            let value = match input.ty {
                InputType::String => Some(InputValue::String(self.value(&field_key(name), cx))),
                InputType::Number => {
                    let typed = self.value(&field_key(name), cx);
                    (!typed.trim().is_empty())
                        .then(|| InputValue::Number(actions::js_number(&typed)))
                }
                InputType::Project | InputType::Boolean => gate.values.get(name).cloned(),
            };
            if let Some(value) = value {
                form.insert(name.clone(), value);
            }
        }
        form
    }

    /// Checks the requirements again, off the UI thread. An older check that answers late is
    /// ignored.
    pub(super) fn run_gate_check(&mut self, cx: &mut Context<Self>) {
        let form = self.run_gate_form(cx);
        self.wf.gate_generation += 1;
        let generation = self.wf.gate_generation;
        let Some(gate) = self.wf.gate.as_mut() else {
            return;
        };
        gate.generation = generation;
        gate.preflight = None;
        // A refusal belongs to the look that came before; this one may find it fixed.
        gate.notice = None;
        if gate.requires().is_empty() {
            gate.checking = false;
            return;
        }
        gate.checking = true;
        // The values this run would take decide where the MCP servers are looked up.
        let mut subject = gate.subject.clone();
        if gate.problems(&form).is_empty() {
            subject.run_values = gate.overrides(&form);
        }
        let subject = with_run_values(&subject);
        self.workflow_task(
            move |host| host.preflight(&subject),
            move |this, check, cx| this.run_gate_checked(generation, check, cx),
            cx,
        );
    }

    /// A check answered. Only the newest check of the open dialog counts.
    pub(super) fn run_gate_checked(
        &mut self,
        generation: u64,
        check: Preflight,
        cx: &mut Context<Self>,
    ) {
        let Some(gate) = self.wf.gate.as_mut().filter(|g| g.generation == generation) else {
            return;
        };
        gate.checking = false;
        gate.preflight = Some(check);
        cx.notify();
    }

    /// The primary button: asks for the run (or the resume), with the values that differ from the
    /// stored ones as this run's own. The dialog stays until the answer: a refusal is shown in it,
    /// a start closes it.
    pub(super) fn run_gate_submit(&mut self, cx: &mut Context<Self>) {
        let form = self.run_gate_form(cx);
        let Some(gate) = self.wf.gate.as_ref().filter(|g| g.ready(&form)) else {
            return;
        };
        let (id, mode, page) = (gate.workflow_id.clone(), gate.mode.clone(), gate.page);
        let overrides = gate.overrides(&form);
        self.wf.gate_generation += 1;
        let token = self.wf.gate_generation;
        if let Some(gate) = self.wf.gate.as_mut() {
            gate.submitting = Some(token);
            gate.notice = None;
        }
        let asked = id.clone();
        self.workflow_task(
            move |host| match mode {
                GateMode::Run => actions::run_now(host, &id, overrides),
                GateMode::Resume(run) => actions::resume_run(host, &id, &run),
            },
            move |this, notice, cx| this.run_gate_answered(&asked, token, page, notice, cx),
            cx,
        );
        cx.notify();
    }

    /// The runner answered the start asked for by the dialog.
    fn run_gate_answered(
        &mut self,
        workflow: &str,
        token: u64,
        page: bool,
        notice: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let waiting = self
            .wf
            .gate
            .as_ref()
            .is_some_and(|g| g.submitting == Some(token) && g.workflow_id == workflow);
        if !waiting {
            // Cancelled meanwhile: say it where a refused click says it.
            self.workflow_said(notice, page, cx);
            return;
        }
        match notice {
            None => {
                self.workflow_said(None, page, cx);
                self.run_gate_close(cx);
            }
            Some(notice) => {
                if let Some(gate) = self.wf.gate.as_mut() {
                    gate.submitting = None;
                    gate.notice = Some(notice);
                }
                cx.notify();
            }
        }
    }

    /// A project or an on/off picked in the dialog.
    pub(super) fn run_gate_picked(&mut self, name: &str, chosen: &str, cx: &mut Context<Self>) {
        let Some(gate) = self.wf.gate.as_mut() else {
            return;
        };
        let Some(input) = gate.inputs.iter().find(|(n, _)| n == name).map(|(_, i)| i) else {
            return;
        };
        let value = match input.ty {
            InputType::Boolean => InputValue::Bool(chosen == "on"),
            _ => InputValue::String(chosen.to_string()),
        };
        gate.values.insert(name.to_string(), value);
        // The values this run takes decide where MCP servers are looked up.
        self.run_gate_check(cx);
        cx.notify();
    }

    /// One input of the form: a project picker, an on/off picker, or a text box.
    fn run_gate_input(
        &mut self,
        mono: &Mono,
        name: &str,
        input: &WorkflowInput,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let id = format!("wf-gate-input-{name}");
        let pick = Pick::Gate(name.to_string());
        let value = self
            .wf
            .gate
            .as_ref()
            .and_then(|g| g.values.get(name))
            .cloned();
        match input.ty {
            InputType::Project => {
                let current = match &value {
                    Some(InputValue::String(path)) => path.clone(),
                    _ => String::new(),
                };
                let mut options: Vec<(String, String)> = mono
                    .projects
                    .iter()
                    .map(|path| (path.clone(), basename(path)))
                    .collect();
                if !current.is_empty() && !options.iter().any(|(path, _)| *path == current) {
                    options.insert(0, (current.clone(), basename(&current)));
                }
                let selected = options
                    .iter()
                    .find(|(path, _)| *path == current)
                    .map_or_else(|| "Choose a project".to_string(), |(_, l)| l.clone());
                self.workflow_picker(id, name.to_string(), selected, options, current, pick, cx)
            }
            InputType::Boolean => {
                let current = match &value {
                    Some(InputValue::Bool(true)) => "on",
                    Some(InputValue::Bool(false)) => "off",
                    _ => "",
                };
                let selected = match current {
                    "on" => "On",
                    "off" => "Off",
                    _ => "Choose",
                };
                self.workflow_picker(
                    id,
                    name.to_string(),
                    selected.to_string(),
                    choices(&[("on", "On"), ("off", "Off")]),
                    current.to_string(),
                    pick,
                    cx,
                )
            }
            InputType::String | InputType::Number => {
                let Some(field) = self.fields.get(&field_key(name)).cloned() else {
                    return div().into_any_element();
                };
                let selector = id.clone();
                habit_field(field)
                    .id(SharedString::from(id))
                    .debug_selector(move || selector.clone())
                    .h(px(28.))
                    .bg(content(0.1))
                    .into_any_element()
            }
        }
    }

    /// The dialog, when it was opened on the page being drawn.
    pub(super) fn run_gate_dialog(
        &mut self,
        mono: &Mono,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if self.wf.gate.as_ref().is_some_and(|g| g.route != self.route) {
            // Another page is on screen; the dialog belonged to the one that was left.
            return None;
        }
        let gate = self.wf.gate.as_mut()?;
        if gate.focus_pending
            && let Some(focus) = gate.focus.clone()
        {
            gate.focus_pending = false;
            window.focus(&focus, cx);
        }
        let focus = gate.focus.clone();
        let submitting = gate.submitting.is_some();
        let gate = &*gate;
        let (title, verb) = match &gate.mode {
            GateMode::Run => (format!("Run {}", gate.subject.name), "Run"),
            GateMode::Resume(_) => (format!("Resume {}", gate.subject.name), "Resume"),
        };
        let inputs = gate.inputs.clone();
        let checking = gate.checking;
        let notice = gate.notice.clone();
        let has_requires = !gate.requires().is_empty();
        let rows = requirement_rows(gate);
        let form = self.run_gate_form(cx);
        let gate = self.wf.gate.as_ref()?;
        let problems = gate.problems(&form);
        let ready = gate.ready(&form);

        let mut fields = div().flex().flex_col().gap_0p5();
        for (name, input) in &inputs {
            let widget = self.run_gate_input(mono, name, input, cx);
            fields = fields.child(property(name.clone(), widget));
        }
        let size = window.viewport_size();
        let weak = cx.weak_entity();
        let width = 420f32.min(size.width.as_f32() - 24.);
        let tall = (size.height.as_f32() - 48.).max(160.);
        Some(
            div()
                .absolute()
                .size_full()
                .child(
                    gpui::deferred(
                        gpui::anchored()
                            .position(gpui::point(px(0.), px(0.)))
                            .child(
                                div()
                                    .w(size.width)
                                    .h(size.height)
                                    .bg(crate::modal::scrim_color())
                                    .occlude(),
                            ),
                    )
                    .with_priority(79),
                )
                .child(
                    popover("wf-run-gate-modal", Anchor::Center)
                        .modal_surface(true)
                        .width(width)
                        .gap(0.)
                        .priority(80)
                        .max_height(tall)
                        .on_dismiss(move |_, cx| {
                            let _ = weak.update(cx, |this, cx| {
                                // A press in a drop-down of the dialog is outside the card, but
                                // not outside the dialog.
                                if this.picker.is_none() {
                                    this.run_gate_close(cx);
                                }
                            });
                        })
                        .child(
                            div()
                                .id("wf-run-gate")
                                .debug_selector(|| "wf-run-gate".into())
                                .role(Role::Dialog)
                                .aria_label(title.clone())
                                .max_h(px(tall))
                                .flex()
                                .flex_col()
                                .when_some(focus, |d, focus| d.track_focus(&focus))
                                .child(
                                    div()
                                        .flex_shrink_0()
                                        .px_4()
                                        .pt_4()
                                        .text_size(px(14.))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(content(1.))
                                        .truncate()
                                        .child(title),
                                )
                                .child(
                                    div()
                                        .id("wf-gate-body")
                                        .debug_selector(|| "wf-gate-body".into())
                                        .flex_1()
                                        .min_h_0()
                                        .overflow_y_scroll()
                                        .px_4()
                                        .py_3()
                                        .flex()
                                        .flex_col()
                                        .gap_3()
                                        .when(!inputs.is_empty(), |d| {
                                            d.child(fields).children(
                                                problems.first().map(|p| line(p.clone(), true)),
                                            )
                                        })
                                        .when(has_requires, |d| {
                                            d.child(requirements_block(
                                                rows,
                                                checking,
                                                checking || submitting,
                                                cx,
                                            ))
                                        })
                                        .children(notice.map(|n| line(n, true))),
                                )
                                .child(
                                    div()
                                        .flex_shrink_0()
                                        .px_4()
                                        .pb_4()
                                        .flex()
                                        .items_center()
                                        .justify_end()
                                        .gap_2()
                                        .child(
                                            crate::modal::ghost_button("wf-gate-cancel", "Cancel")
                                                .debug_selector(|| "wf-gate-cancel".into())
                                                .on_click(cx.listener(|this, _, _, cx| {
                                                    this.run_gate_close(cx)
                                                })),
                                        )
                                        .child(
                                            crate::modal::disabled(
                                                crate::modal::primary_button(
                                                    "wf-gate-submit",
                                                    if submitting {
                                                        "Starting\u{2026}"
                                                    } else {
                                                        verb
                                                    },
                                                )
                                                .debug_selector(|| "wf-gate-submit".into()),
                                                !ready,
                                            )
                                            .on_click(
                                                cx.listener(move |this, _, _, cx| {
                                                    if ready {
                                                        this.run_gate_submit(cx);
                                                    }
                                                }),
                                            ),
                                        ),
                                ),
                        ),
                )
                .into_any_element(),
        )
    }
}

/// One line of the checklist: what is required, and how it stands.
struct Row {
    name: String,
    /// None while it is being looked up.
    met: Option<bool>,
    detail: Option<String>,
    hint: Option<String>,
}

fn requirement_rows(gate: &RunGate) -> Vec<Row> {
    match &gate.preflight {
        Some(check) => check
            .statuses
            .iter()
            .map(|s| Row {
                name: format!("{} {}", s.requirement.kind.key(), s.requirement.name),
                met: Some(s.met),
                detail: s.detail.clone(),
                hint: s.requirement.hint.clone(),
            })
            .collect(),
        None => gate
            .requires()
            .iter()
            .map(|r| Row {
                name: format!("{} {}", r.kind.key(), r.name),
                met: None,
                detail: None,
                hint: r.hint.clone(),
            })
            .collect(),
    }
}

fn line(text: String, error: bool) -> Div {
    div()
        .text_size(px(12.))
        .line_height(px(18.))
        .text_color(if error {
            gpui::Hsla::from(rgb(ERROR)).opacity(0.8)
        } else {
            content(0.45)
        })
        .child(text)
}

fn requirements_block(
    rows: Vec<Row>,
    checking: bool,
    busy: bool,
    cx: &mut Context<Monos>,
) -> gpui::Stateful<Div> {
    div()
        .id("wf-gate-requirements")
        .debug_selector(|| "wf-gate-requirements".into())
        .flex()
        .flex_col()
        .gap_1()
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .text_size(px(11.))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(content(0.4))
                        .child(if checking {
                            "Requirements \u{b7} checking\u{2026}"
                        } else {
                            "Requirements"
                        }),
                )
                .child(
                    crate::modal::disabled(
                        crate::modal::ghost_button("wf-gate-recheck", "Re-check")
                            .debug_selector(|| "wf-gate-recheck".into()),
                        busy,
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if !busy {
                            this.run_gate_check(cx);
                        }
                    })),
                ),
        )
        .children(rows.into_iter().enumerate().map(|(at, row)| {
            let mark = match row.met {
                Some(true) => icon("check", 12.)
                    .text_color(gpui::Hsla::from(rgb(0x34d399)).opacity(0.8))
                    .into_any_element(),
                Some(false) => icon("close", 12.)
                    .text_color(gpui::Hsla::from(rgb(ERROR)).opacity(0.8))
                    .into_any_element(),
                None => div()
                    .size(px(6.))
                    .rounded_full()
                    .bg(content(0.3))
                    .into_any_element(),
            };
            div()
                .id(SharedString::from(format!("wf-gate-requirement-{at}")))
                .debug_selector(move || format!("wf-gate-requirement-{at}"))
                .flex()
                .items_start()
                .gap_2()
                .text_size(px(12.))
                .line_height(px(18.))
                .child(
                    div()
                        .h(px(18.))
                        .w(px(12.))
                        .flex_shrink_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(mark),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(div().text_color(content(0.85)).child(row.name))
                        .children(
                            row.detail.map(|d| {
                                div().text_size(px(11.)).text_color(content(0.5)).child(d)
                            }),
                        )
                        .children(
                            row.hint.filter(|_| row.met != Some(true)).map(|h| {
                                div().text_size(px(11.)).text_color(content(0.4)).child(h)
                            }),
                        ),
                )
        }))
}
