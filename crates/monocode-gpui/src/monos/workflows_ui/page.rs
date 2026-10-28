//! One workflow up close (`WorkflowPage.tsx`): its trigger, inputs and how its recent runs went.
use super::super::*;
use super::actions;
use super::list::workflow_trigger_label;
use super::view::{needs_review_badge, notice_line, property, section};
use super::{
    super::scheduler::{duration, habit_field, when},
    Pick,
};
use monocode_core::mono::workflows::{
    model::{
        InputType, InputValue, NodeKind, RunStatus, Workflow, WorkflowInput, WorkflowRun,
        trigger_of,
    },
    ordered::js_number_text,
    yaml::{export_path_problem, export_workflow},
};
use monocode_core::time::now_ms;

/// The text an input field shows for a stored value.
pub(in crate::monos) fn value_text(value: Option<&InputValue>) -> String {
    match value {
        None => String::new(),
        Some(InputValue::String(text)) => text.clone(),
        Some(InputValue::Number(n)) => js_number_text(*n),
        Some(InputValue::Bool(on)) => on.to_string(),
    }
}

/// How a run reads in the history: its label and the colour of its dot.
pub(super) fn run_status(status: RunStatus, mono_color: u32) -> (&'static str, gpui::Hsla) {
    match status {
        RunStatus::Running => ("Running", rgb(mono_color).into()),
        RunStatus::Done => ("Finished", gpui::Hsla::from(rgb(0x34d399)).opacity(0.8)),
        RunStatus::DoneWithFailures => (
            "Finished with failures",
            gpui::Hsla::from(rgb(0xfbbf24)).opacity(0.8),
        ),
        RunStatus::Failed => ("Failed", gpui::Hsla::from(rgb(0xef4444)).opacity(0.7)),
        RunStatus::Stopped => ("Stopped", content(0.4)),
        RunStatus::Interrupted => ("Interrupted", gpui::Hsla::from(rgb(0xfbbf24)).opacity(0.8)),
    }
}

fn run_row(
    run: &WorkflowRun,
    workflow: &str,
    mono_color: u32,
    cx: &mut Context<Monos>,
) -> AnyElement {
    let (label, dot) = run_status(run.status, mono_color);
    let now = now_ms();
    let took = match run.ended_at {
        Some(ended) => duration(ended.saturating_sub(run.started_at)),
        None if run.status == RunStatus::Running => duration(now.saturating_sub(run.started_at)),
        None => "-".into(),
    };
    let (workflow, id) = (workflow.to_string(), run.id.clone());
    div()
        .id(SharedString::from(format!("mono-workflow-run-{}", run.id)))
        .debug_selector({
            let id = run.id.clone();
            move || format!("mono-workflow-run-{id}")
        })
        .role(Role::Button)
        .aria_label(label)
        .w_full()
        .min_w_0()
        .flex_shrink_0()
        .px_2()
        .py_2()
        .rounded_md()
        .flex()
        .items_center()
        .gap_2p5()
        .text_size(px(12.))
        .hover(|s| s.bg(content(0.05)))
        .child(if run.status == RunStatus::Running {
            super::view::running_dot(
                SharedString::from(format!("mono-workflow-pulse-{}", run.id)),
                mono_color,
                8.,
            )
        } else {
            div()
                .size(px(8.))
                .flex_shrink_0()
                .rounded_full()
                .bg(dot)
                .into_any_element()
        })
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_color(content(0.85))
                .child(label),
        )
        .child(
            div()
                .flex_shrink_0()
                .text_color(content(0.4))
                .child(format!("{} \u{b7} {took}", when(run.started_at, now))),
        )
        .on_click(cx.listener(move |this, _, _, cx| {
            this.open_workflow_route(Route::WorkflowRun(workflow.clone(), id.clone()), cx)
        }))
        .into_any_element()
}

impl Monos {
    pub(in crate::monos) fn workflow_page(
        &mut self,
        mono: &Mono,
        id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.ensure_workflows(cx);
        let Some(w) = self.mono_workflow(id).cloned() else {
            return div()
                .size_full()
                .min_w_0()
                .flex()
                .flex_col()
                .child(self.workflow_header("Workflow", None, cx))
                .child(
                    div()
                        .p_4()
                        .text_size(px(12.))
                        .text_color(content(0.45))
                        .child("This workflow is no longer available."),
                )
                .into_any_element();
        };
        let running = self.workflow_running(&w.id);
        let starting = !running && w.run_requested;
        let mut info = div()
            .w_full()
            .min_w_0()
            .flex_shrink_0()
            .px_4()
            .py_3()
            .flex()
            .flex_col()
            .gap_0p5();
        if w.needs_review {
            info = info.child(property("Status", needs_review_badge()));
        }
        info = info.child(property(
            "Runs",
            if w.enabled {
                workflow_trigger_label(&w)
            } else {
                "Paused".into()
            },
        ));
        if starting {
            info = info.child(property("Next", "Starting\u{2026}"));
        } else if w.enabled
            && matches!(
                trigger_of(&w).map(|n| &n.kind),
                Some(NodeKind::Schedule { .. })
            )
            && let Some(next) = w.next_run_at
        {
            info = info.child(property("Next", when(next, now_ms())));
        }

        let mut inputs = Vec::new();
        for (name, input) in w.inputs.iter() {
            let field = self.workflow_input_field(mono, &w, name, input, window, cx);
            inputs.push(property(name.clone(), field));
        }

        let runs = self.workflow_runs_of(&w.id).cloned();
        let mut history = section("Recent runs", None);
        match &runs {
            Some(runs) if runs.is_empty() => {
                history = history.child(
                    div()
                        .px_2()
                        .py_3()
                        .text_size(px(12.))
                        .text_color(content(0.4))
                        .child("It hasn't run yet."),
                );
            }
            Some(runs) => {
                for run in runs {
                    history = history.child(run_row(run, &w.id, mono.color, cx));
                }
            }
            None => {}
        }

        let (edit, export_w) = (w.id.clone(), w.clone());
        let export = icon_button("mono-workflow-export", "Export", "share")
            .debug_selector(|| "mono-workflow-export".into())
            .on_click(cx.listener(move |this, _, _, cx| this.workflow_export(&export_w, cx)))
            .into_any_element();
        div()
            .size_full()
            .min_w_0()
            .min_h_0()
            .overflow_hidden()
            .flex()
            .flex_col()
            .child(
                self.workflow_header(&w.name, None, cx)
                    .child(
                        icon_button("mono-workflow-edit", "Edit board", "pencil-edit")
                            .debug_selector(|| "mono-workflow-edit".into())
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.open_workflow_route(Route::WorkflowBoard(edit.clone()), cx)
                            })),
                    )
                    .child(self.workflow_page_actions(&w, Some(export), cx)),
            )
            .child(
                div()
                    .id("mono-workflow-page")
                    .debug_selector(|| "mono-workflow-page".into())
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
                    ))
                    .child(info)
                    .when_some(
                        w.description.as_deref().filter(|d| !d.is_empty()),
                        |d, description| {
                            d.child(
                                section("What it does", None).child(
                                    div()
                                        .px_2()
                                        .text_size(px(12.))
                                        .line_height(px(20.))
                                        .text_color(content(0.75))
                                        .child(description.to_string()),
                                ),
                            )
                        },
                    )
                    .when(!inputs.is_empty(), |d| {
                        d.child(
                            section("Inputs", None)
                                .id("mono-workflow-inputs")
                                .debug_selector(|| "mono-workflow-inputs".into())
                                .child(div().px_2().flex().flex_col().gap_0p5().children(inputs)),
                        )
                    })
                    .child(history),
            )
            .into_any_element()
    }

    /// One declared input, edited in place: a project picker, a switch, or a text box.
    fn workflow_input_field(
        &mut self,
        mono: &Mono,
        w: &Workflow,
        name: &str,
        input: &WorkflowInput,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let value = w.input_values.get(name);
        let id = format!("mono-workflow-input-{name}");
        let pick = Pick::Input(w.id.clone(), name.to_string());
        match input.ty {
            InputType::Project => {
                let current = match value {
                    Some(InputValue::String(path)) => path.clone(),
                    _ => String::new(),
                };
                let mut choices: Vec<(String, String)> = mono
                    .projects
                    .iter()
                    .map(|path| (path.clone(), basename(path)))
                    .collect();
                // A project the Mono no longer has stays choosable, so the value is not hidden.
                if !current.is_empty() && !choices.iter().any(|(path, _)| *path == current) {
                    choices.insert(0, (current.clone(), basename(&current)));
                }
                let selected = choices
                    .iter()
                    .find(|(path, _)| *path == current)
                    .map_or_else(
                        || "Choose a project".to_string(),
                        |(_, label)| label.clone(),
                    );
                self.workflow_picker(id, name.to_string(), selected, choices, current, pick, cx)
            }
            InputType::Boolean => {
                let current = match value {
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
                    vec![("on".into(), "On".into()), ("off".into(), "Off".into())],
                    current.to_string(),
                    pick,
                    cx,
                )
            }
            InputType::String | InputType::Number => {
                let text = value_text(value);
                let key = format!("wf-input:{}:{name}", w.id);
                let placeholder = input
                    .description
                    .clone()
                    .unwrap_or_else(|| name.to_string());
                let fresh = !self.fields.contains_key(&key);
                let field = self.field(&key, &placeholder, text.clone(), false, cx);
                if fresh {
                    let (wid, input_name) = (w.id.clone(), name.to_string());
                    self.observations.push(cx.subscribe(
                        &field,
                        move |this, _, _: &input::Submitted, cx| {
                            this.workflow_commit_input(&wid, &input_name, cx)
                        },
                    ));
                    let (wid, input_name) = (w.id.clone(), name.to_string());
                    self.observations.push(cx.on_blur(
                        &field.focus_handle(cx),
                        window,
                        move |this, _, cx| this.workflow_commit_input(&wid, &input_name, cx),
                    ));
                    self.wf.seeds.insert(key, text);
                } else if self.wf.seeds.get(&key) != Some(&text)
                    && !field.focus_handle(cx).is_focused(window)
                {
                    // A value saved elsewhere replaces what was typed.
                    field.update(cx, |f, cx| f.set_text(text.clone(), cx));
                    self.wf.seeds.insert(key, text);
                }
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

    /// The user left a text or number input: keep what they typed, after checking it.
    pub(in crate::monos) fn workflow_commit_input(
        &mut self,
        workflow: &str,
        name: &str,
        cx: &mut Context<Self>,
    ) {
        let Some(w) = self.mono_workflow(workflow).cloned() else {
            return;
        };
        let Some(input) = w.inputs.get(name).cloned() else {
            return;
        };
        let key = format!("wf-input:{workflow}:{name}");
        let current = value_text(w.input_values.get(name));
        let typed = self.value(&key, cx);
        if typed == current || !self.fields.contains_key(&key) {
            return;
        }
        let value = match input.ty {
            InputType::String => Some(InputValue::String(typed)),
            InputType::Number if typed.trim().is_empty() => None,
            InputType::Number => Some(InputValue::Number(actions::js_number(&typed))),
            _ => return,
        };
        self.workflow_save_input(workflow, name, &input, value, cx);
    }

    /// A project or an on/off chosen from a picker on the page.
    pub(in crate::monos) fn workflow_choose_input(
        &mut self,
        workflow: &str,
        name: &str,
        chosen: &str,
        cx: &mut Context<Self>,
    ) {
        let Some(input) = self
            .mono_workflow(workflow)
            .and_then(|w| w.inputs.get(name))
            .cloned()
        else {
            return;
        };
        let value = match input.ty {
            InputType::Boolean => InputValue::Bool(chosen == "on"),
            _ => InputValue::String(chosen.to_string()),
        };
        self.workflow_save_input(workflow, name, &input, Some(value), cx);
    }

    /// Saves one input value if it fits the declared type; otherwise says what is wrong.
    fn workflow_save_input(
        &mut self,
        workflow: &str,
        name: &str,
        input: &WorkflowInput,
        value: Option<InputValue>,
        cx: &mut Context<Self>,
    ) {
        if let Some(problem) = actions::input_problem(name, input, value.as_ref()) {
            self.workflow_said(Some(problem), true, cx);
            return;
        }
        let Some(value) = value else {
            return;
        };
        self.workflow_said(None, true, cx);
        let (id, name) = (workflow.to_string(), name.to_string());
        self.workflow_task(
            move |host| actions::save_input(host, &id, &name, &value),
            |this, saved, cx| {
                if let Err(error) = saved {
                    eprintln!("Could not save the input: {error}");
                    let message = if error.is_empty() {
                        "Could not save the input.".to_string()
                    } else {
                        error
                    };
                    this.workflow_said(Some(message), true, cx);
                }
            },
            cx,
        );
    }

    /// Saves the shareable definition where the user picks. Says why when the workflow cannot be
    /// shared or the save failed; a saved file or a cancelled dialog clears the notice.
    pub(in crate::monos) fn workflow_export(&mut self, w: &Workflow, cx: &mut Context<Self>) {
        if let Some(problem) = export_path_problem(w) {
            self.workflow_said(
                Some(format!("Can't export this workflow: {problem}.")),
                true,
                cx,
            );
            return;
        }
        let directory = monocode_core::accounts::home().unwrap_or_default();
        let prompt =
            cx.prompt_for_new_path(&directory, Some(&actions::workflow_file_name(&w.name)));
        let w = w.clone();
        cx.spawn(async move |this, cx| {
            let failed = Some("Could not save the workflow file.".to_string());
            let path = match prompt.await {
                Ok(Ok(Some(path))) => path,
                Ok(Ok(None)) => {
                    let _ = this.update(cx, |this, cx| this.workflow_said(None, true, cx));
                    return;
                }
                _ => {
                    let _ = this.update(cx, |this, cx| this.workflow_said(failed, true, cx));
                    return;
                }
            };
            let wrote = cx
                .background_executor()
                .spawn(async move { std::fs::write(path, export_workflow(&w)) })
                .await;
            let notice = wrote.err().map(|error| {
                eprintln!("Could not export the workflow: {error}");
                "Could not save the workflow file.".to_string()
            });
            let _ = this.update(cx, |this, cx| this.workflow_said(notice, true, cx));
        })
        .detach();
    }
}

pub(in crate::monos) fn basename(path: &str) -> String {
    path.trim_end_matches(['/', '\\'])
        .rsplit(['/', '\\'])
        .next()
        .filter(|name| !name.is_empty())
        .unwrap_or(path)
        .to_string()
}
