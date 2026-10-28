//! Brings in a workflow file (`ImportWorkflowPage.tsx`): the user reads what it would run and fills
//! its inputs, and it is saved paused and flagged for review. Everything that reaches an agent or
//! the shell is shown as the file has it.
use super::super::*;
use super::actions::js_number;
use super::view::{alert, header_button, hint, property, section};
use super::{super::scheduler::habit_field, Pick};
use monocode_core::mono::workflows::{
    cli::WorkflowHost,
    model::{InputType, InputValue, WORKFLOWS_MAX, Workflow, WorkflowInput},
    models::{ModelCatalog, Probe, harnesses_used},
    ordered::OrderedMap,
    yaml::{
        ImportPreview, MAX_FILE_BYTES, anchor_node, finish_import, input_value_problems,
        preview_import, visible,
    },
};
use monocode_core::{provider::Kind, time::now_ms};

const TEXT_KEY: &str = "wf-import-text";
const PLACEHOLDER: &str = "Or paste the text of a workflow file here.";
/// What a file picked with "Choose file" may weigh before it is refused (the editor's limit).
const READ_MAX: u64 = 8 * 1024 * 1024;
/// How long typing pauses before the file is read back.
const DEBOUNCE: Duration = Duration::from_millis(250);

/// What the user typed or chose for an input, before it is read as a value.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Entered {
    Text(String),
    Project(String),
    Flag(bool),
}

#[derive(Default)]
pub(in crate::monos) struct Import {
    /// Counts the edits of the text; a preview belongs to one of them.
    pub(super) revision: u64,
    pub(super) shown: u64,
    pub(super) read: Option<Result<ImportPreview, String>>,
    pub(super) file_error: Option<String>,
    /// The next change of the text is the page emptying it after a refused file: the refusal stays.
    pub(super) keep_file_error: bool,
    /// Projects and switches chosen, by `<type>:<name>`: a file that declares the input
    /// differently does not inherit it.
    pub(super) choices: HashMap<String, Entered>,
    /// The text inputs the user has typed in (an erased one is an empty text, not no value).
    pub(super) touched: HashSet<String>,
    pub(super) saving: bool,
    pub(super) error: Option<String>,
    /// How long typing pauses before the preview; tests shorten it.
    pub(super) debounce: Option<Duration>,
}

fn type_name(ty: InputType) -> &'static str {
    match ty {
        InputType::Project => "project",
        InputType::String => "string",
        InputType::Number => "number",
        InputType::Boolean => "boolean",
    }
}

fn type_of(name: &str) -> Option<InputType> {
    Some(match name {
        "project" => InputType::Project,
        "string" => InputType::String,
        "number" => InputType::Number,
        "boolean" => InputType::Boolean,
        _ => return None,
    })
}

fn key_of(ty: InputType, name: &str) -> String {
    format!("{}:{name}", type_name(ty))
}

/// What the user typed or chose, as the values the workflow takes.
pub(in crate::monos) fn values_of(
    inputs: &OrderedMap<WorkflowInput>,
    entered: impl Fn(&str, InputType) -> Option<Entered>,
) -> OrderedMap<InputValue> {
    let mut values = OrderedMap::new();
    for (name, input) in inputs.iter() {
        let value = match (input.ty, entered(name, input.ty)) {
            (InputType::Number, Some(Entered::Text(text))) if !text.trim().is_empty() => {
                InputValue::Number(js_number(&text))
            }
            (InputType::String, Some(Entered::Text(text))) => InputValue::String(text),
            (InputType::Project, Some(Entered::Project(path))) => InputValue::String(path),
            (InputType::Boolean, Some(Entered::Flag(on))) => InputValue::Bool(on),
            _ => continue,
        };
        values.insert(name.clone(), value);
    }
    values
}

/// The composer's draft with `text` started at its end, as the app's "prefill" does: kept exactly,
/// a trailing space included, and asking twice does not stack the same start.
pub(in crate::monos) fn append_prefill(draft: &str, text: &str) -> String {
    let value = text.replace("\r\n", "\n").replace('\r', "\n");
    if value.trim().is_empty() || draft.ends_with(&value) {
        return draft.to_string();
    }
    let separator = if draft.is_empty() || draft.ends_with("\n\n") {
        ""
    } else if draft.ends_with('\n') {
        "\n"
    } else {
        "\n\n"
    };
    format!("{draft}{separator}{value}")
}

/// The file read as a preview: the models its sessions name are judged against the catalog of the
/// harnesses it uses. Blocks while a CLI is probed: off the UI thread.
pub(in crate::monos) fn make_preview(
    text: &str,
    probe: fn(Kind) -> Probe,
) -> Result<ImportPreview, String> {
    let first = preview_import(text, &ModelCatalog::unprobed())?;
    let mut named = Workflow::new("wf-preview", first.workflow.name.clone());
    named.nodes = first.workflow.nodes.clone();
    let kinds = harnesses_used(&named);
    if kinds.is_empty() {
        return Ok(first);
    }
    preview_import(text, &ModelCatalog::for_harnesses(&kinds, probe))
}

/// The file at `path` as text, or why it cannot be one and whether the page empties its box. Only
/// a file over the limit does (it gives the model's own refusal); one that cannot be read leaves
/// what was pasted.
fn read_file(path: &std::path::Path) -> Result<String, (bool, String)> {
    let reason = |error: String| (false, format!("Could not read the file: {error}"));
    let meta = std::fs::metadata(path).map_err(|e| reason(e.to_string()))?;
    if meta.len() > READ_MAX {
        return Err(reason("File is too large to edit (maximum 8 MB).".into()));
    }
    let bytes = std::fs::read(path).map_err(|e| reason(e.to_string()))?;
    let text =
        String::from_utf8(bytes).map_err(|_| reason("File is not valid UTF-8 text.".into()))?;
    if text.len() > MAX_FILE_BYTES {
        // The model's own refusal; the text itself stays out of the page.
        return Err((
            true,
            match preview_import(&text, &ModelCatalog::unprobed()) {
                Err(refused) => refused,
                Ok(_) => "Workflow file is too large".into(),
            },
        ));
    }
    Ok(text)
}

fn limit_message() -> String {
    format!("A Mono can have at most {WORKFLOWS_MAX} workflows")
}

impl Monos {
    /// Opens the import page with nothing in it.
    pub(in crate::monos) fn open_import_page(&mut self, cx: &mut Context<Self>) {
        self.clear_workflow_fields();
        // Prompts are open every time the page is opened.
        self.wf.prompts_closed = false;
        let debounce = self.wf.import.debounce;
        self.wf.import = Import {
            debounce,
            ..Default::default()
        };
        self.open_workflow_route(Route::ImportWorkflow, cx);
    }

    fn import_text_field(&mut self, cx: &mut Context<Self>) -> Entity<Input> {
        if let Some(field) = self.fields.get(TEXT_KEY) {
            return field.clone();
        }
        let field = cx.new(|cx| Input::multiline_field("Workflow file text", PLACEHOLDER, cx));
        self.observations
            .push(cx.subscribe(&field, |this, _, _: &input::Changed, cx| {
                this.import_text_changed(cx)
            }));
        self.fields.insert(TEXT_KEY.into(), field.clone());
        field
    }

    /// The text changed: the old preview and errors no longer describe it. The new preview is made
    /// once typing pauses, off the UI thread, so typing never waits for a large file to be read.
    fn import_text_changed(&mut self, cx: &mut Context<Self>) {
        let text = self.value(TEXT_KEY, cx);
        let imp = &mut self.wf.import;
        imp.revision += 1;
        if !std::mem::take(&mut imp.keep_file_error) {
            imp.file_error = None;
        }
        imp.error = None;
        let revision = imp.revision;
        if text.trim().is_empty() {
            imp.read = None;
            imp.shown = revision;
            cx.notify();
            return;
        }
        let pause = imp.debounce.unwrap_or(DEBOUNCE);
        let probe = self.workflow_live.probe();
        cx.notify();
        cx.spawn(async move |this, cx| {
            if !pause.is_zero() {
                cx.background_executor().timer(pause).await;
            }
            let current = this
                .update(cx, |this, _| this.wf.import.revision == revision)
                .unwrap_or(false);
            if !current {
                return;
            }
            let read = cx
                .background_executor()
                .spawn(async move { make_preview(&text, probe) })
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.wf.import.revision == revision {
                    this.wf.import.read = Some(read);
                    this.wf.import.shown = revision;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// "Choose file": reads a picked workflow file into the same preview.
    fn import_choose_file(&mut self, cx: &mut Context<Self>) {
        let picked = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Import workflow".into()),
        });
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = picked.await else {
                return;
            };
            let Some(path) = paths.into_iter().next() else {
                return;
            };
            let read = cx
                .background_executor()
                .spawn(async move { read_file(&path) })
                .await;
            let _ = this.update(cx, |this, cx| {
                let field = this.import_text_field(cx);
                match read {
                    Ok(text) => field.update(cx, |f, cx| f.set_text(text, cx)),
                    Err((clear, error)) => {
                        this.wf.import.file_error = Some(error);
                        if clear {
                            this.wf.import.keep_file_error = true;
                            field.update(cx, |f, cx| f.set_text(String::new(), cx));
                        }
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// A project or a switch chosen for an input of the file.
    pub(in crate::monos) fn import_choose(
        &mut self,
        name: &str,
        ty: &str,
        chosen: &str,
        cx: &mut Context<Self>,
    ) {
        let Some(ty) = type_of(ty) else {
            return;
        };
        let entered = match ty {
            InputType::Boolean => Entered::Flag(chosen == "on"),
            _ => Entered::Project(chosen.to_string()),
        };
        self.wf.import.choices.insert(key_of(ty, name), entered);
        self.wf.import.error = None;
        cx.notify();
    }

    /// What the user has entered so far, as values.
    pub(super) fn import_values(
        &self,
        inputs: &OrderedMap<WorkflowInput>,
        cx: &App,
    ) -> OrderedMap<InputValue> {
        values_of(inputs, |name, ty| match ty {
            InputType::String | InputType::Number => {
                let key = key_of(ty, name);
                self.wf
                    .import
                    .touched
                    .contains(&key)
                    .then(|| Entered::Text(self.value(&format!("wf-import-in:{key}"), cx)))
            }
            _ => self.wf.import.choices.get(&key_of(ty, name)).cloned(),
        })
    }

    fn import_input_field(
        &mut self,
        mono: &Mono,
        name: &str,
        input: &WorkflowInput,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let id = format!("mono-import-input-{name}");
        // What the file names these with is shown as it is, hidden characters made visible.
        let shown = visible(name);
        let described = input.description.as_deref().map(visible);
        match input.ty {
            InputType::Project => {
                let chosen = match self.wf.import.choices.get(&key_of(input.ty, name)) {
                    Some(Entered::Project(path)) => path.clone(),
                    _ => String::new(),
                };
                let choices: Vec<(String, String)> = mono
                    .projects
                    .iter()
                    .map(|path| (path.clone(), super::page::basename(path)))
                    .collect();
                let selected = choices
                    .iter()
                    .find(|(path, _)| *path == chosen)
                    .map_or_else(
                        || "Choose a project".to_string(),
                        |(_, label)| label.clone(),
                    );
                self.workflow_picker(
                    id,
                    shown,
                    selected,
                    choices,
                    chosen,
                    Pick::Import(name.to_string(), "project".into()),
                    cx,
                )
            }
            InputType::Boolean => {
                let current = match self.wf.import.choices.get(&key_of(input.ty, name)) {
                    Some(Entered::Flag(true)) => "on",
                    Some(Entered::Flag(false)) => "off",
                    _ => "",
                };
                let selected = match current {
                    "on" => "On",
                    "off" => "Off",
                    _ => "Choose",
                };
                self.workflow_picker(
                    id,
                    shown,
                    selected.to_string(),
                    vec![("on".into(), "On".into()), ("off".into(), "Off".into())],
                    current.to_string(),
                    Pick::Import(name.to_string(), "boolean".into()),
                    cx,
                )
            }
            InputType::String | InputType::Number => {
                let key = key_of(input.ty, name);
                let field_key = format!("wf-import-in:{key}");
                let fresh = !self.fields.contains_key(&field_key);
                let field = self.field(
                    &field_key,
                    &described.clone().unwrap_or_else(|| shown.clone()),
                    String::new(),
                    false,
                    cx,
                );
                if fresh {
                    let touched = key.clone();
                    self.observations.push(cx.subscribe(
                        &field,
                        move |this, _, _: &input::Changed, cx| {
                            this.wf.import.touched.insert(touched.clone());
                            this.wf.import.error = None;
                            cx.notify();
                        },
                    ));
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

    pub(in crate::monos) fn import_workflow_page(
        &mut self,
        mono: &Mono,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.ensure_workflows(cx);
        let text = self.import_text_field(cx);
        let read = self.wf.import.read.clone();
        let preview = read.as_ref().and_then(|r| r.as_ref().ok()).cloned();
        let empty = text.read(cx).content.trim().is_empty();
        let inputs = preview
            .as_ref()
            .map(|p| p.workflow.inputs.clone())
            .unwrap_or_default();
        let values = self.import_values(&inputs, cx);
        let problems = if preview.is_some() {
            input_value_problems(&inputs, &values)
        } else {
            Vec::new()
        };
        let full = self.workflow_count().is_some_and(|n| n >= WORKFLOWS_MAX);
        let limit = limit_message();
        let no_project =
            mono.projects.is_empty() && inputs.values().any(|input| input.ty == InputType::Project);
        let imp = &self.wf.import;
        // The preview must be of the text in the box, not of what it said a moment ago.
        let ready = preview.is_some()
            && imp.shown == imp.revision
            && problems.is_empty()
            && !full
            && !imp.saving;
        let (file_error, save_error) = (imp.file_error.clone(), imp.error.clone());
        let body = match &preview {
            Some(preview) => {
                let mut parts = self.review_sections(
                    preview,
                    "You can still import it. It stays paused until these are fixed.",
                    cx,
                );
                if !inputs.is_empty() {
                    let mut fields = Vec::new();
                    for (name, input) in inputs.iter() {
                        let field = self.import_input_field(mono, name, input, cx);
                        let described = input.description.as_deref().map(visible);
                        fields.push(property(
                            visible(name),
                            div()
                                .flex()
                                .flex_col()
                                .gap_1()
                                .py_0p5()
                                .child(field)
                                .when_some(
                                    described.filter(|_| {
                                        matches!(input.ty, InputType::Project | InputType::Boolean)
                                    }),
                                    |d, text| {
                                        d.child(
                                            div()
                                                .text_size(px(11.))
                                                .line_height(px(16.))
                                                .text_color(content(0.35))
                                                .child(text),
                                        )
                                    },
                                ),
                        ));
                    }
                    parts.push(
                        section("Inputs", None)
                            .id("mono-import-inputs")
                            .debug_selector(|| "mono-import-inputs".into())
                            .child(div().px_2().flex().flex_col().gap_0p5().children(fields))
                            .into_any_element(),
                    );
                }
                parts.push(
                    div()
                        .id("mono-import-status")
                        .debug_selector(|| "mono-import-status".into())
                        .w_full()
                        .min_w_0()
                        .flex_shrink_0()
                        .border_t_1()
                        .border_color(content(0.1))
                        .px_2()
                        .pb_4()
                        .children(save_error.clone().map(alert))
                        .when(full, |d| {
                            d.child(hint(format!("{limit}. Remove one to import this.")))
                        })
                        .when(!full && no_project, |d| {
                            d.child(hint("Add a project to this Mono first, to choose it here."))
                        })
                        .when(!full && !no_project && !problems.is_empty(), |d| {
                            d.child(hint(format!(
                                "Fill every input to import. {}.",
                                problems[0]
                            )))
                        })
                        .into_any_element(),
                );
                parts
            }
            None => Vec::new(),
        };
        let choose = div()
            .id("mono-import-choose-file")
            .debug_selector(|| "mono-import-choose-file".into())
            .role(Role::Button)
            .aria_label("Choose file")
            .cursor_pointer()
            .rounded_sm()
            .px_1p5()
            .py_0p5()
            .text_size(px(11.))
            .text_color(content(0.6))
            .hover(|s| s.bg(content(0.08)).text_color(content(1.)))
            .child("Choose file")
            .on_click(cx.listener(|this, _, _, cx| this.import_choose_file(cx)))
            .into_any_element();
        div()
            .size_full()
            .min_w_0()
            .min_h_0()
            .overflow_hidden()
            .flex()
            .flex_col()
            .child(
                self.workflow_header("Import workflow", None, cx).child(
                    header_button("mono-import-paused", "Import paused", true, ready).on_click(
                        cx.listener(move |this, _, _, cx| {
                            if ready {
                                this.import_save(cx);
                            }
                        }),
                    ),
                ),
            )
            .child(
                div()
                    .id("mono-import-page")
                    .debug_selector(|| "mono-import-page".into())
                    .w_full()
                    .min_w_0()
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .flex()
                    .flex_col()
                    .child(
                        section("Workflow file", Some(choose))
                            .child(
                                div()
                                    .id("mono-import-text")
                                    .debug_selector(|| "mono-import-text".into())
                                    .child(habit_field(text)),
                            )
                            .children(file_error.clone().map(alert))
                            .children(match &read {
                                Some(Err(error)) => Some(alert(error.clone())),
                                _ => None,
                            })
                            .when(empty && file_error.is_none(), |d| {
                                d.child(hint(
                                    "Nothing is saved until you import, and an imported workflow starts paused.",
                                ))
                            }),
                    )
                    .children(body),
            )
            .into_any_element()
    }

    /// Saves the previewed file as a new workflow: paused, flagged for review, its interval
    /// schedules counted from today. The limit is checked again over what is on disk.
    fn import_save(&mut self, cx: &mut Context<Self>) {
        let Some(Ok(preview)) = self.wf.import.read.clone() else {
            return;
        };
        let values = self.import_values(&preview.workflow.inputs, cx);
        let imp = &mut self.wf.import;
        if imp.saving {
            return;
        }
        imp.saving = true;
        imp.error = None;
        let now = now_ms();
        let limit = limit_message();
        let task_limit = limit.clone();
        // The Mono the import was started for, whichever is open when it ends.
        let mono = self.active.clone();
        self.workflow_task(
            move |host| -> Result<Workflow, String> {
                let finished = finish_import(&preview, &values, now)?;
                let mut created = finished;
                created.nodes = created
                    .nodes
                    .into_iter()
                    .map(|node| anchor_node(node, now))
                    .collect();
                // It is read again, and approved, before it can run or be enabled.
                created.needs_review = true;
                let saved = created.clone();
                host.update(|_, mut list| {
                    if list.len() >= WORKFLOWS_MAX {
                        return Err(task_limit.clone());
                    }
                    list.push(created.clone());
                    Ok((list, ()))
                })?;
                Ok(saved)
            },
            move |this, saved, cx| {
                this.wf.import.saving = false;
                match saved {
                    Ok(workflow) => {
                        let id = workflow.id.clone();
                        if let Some(mono) = mono.clone() {
                            this.wf.loaded.entry(mono).or_default().push(workflow);
                        }
                        // The import page is replaced by the new workflow's page.
                        if this.route == Some(Route::ImportWorkflow) && this.active == mono {
                            this.wf.page_notice = None;
                            this.route = Some(Route::Workflow(id));
                        }
                        this.changed(cx);
                    }
                    Err(error) => {
                        eprintln!("Could not import the workflow: {error}");
                        this.wf.import.error = Some(if error == limit {
                            format!("{limit}.")
                        } else if error.is_empty() {
                            "Could not save the workflow.".into()
                        } else {
                            error
                        });
                    }
                }
                cx.notify();
            },
            cx,
        );
        cx.notify();
    }
}
