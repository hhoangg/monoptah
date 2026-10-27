//! The Ask Agent dialog (`AskAgentDialog.tsx`): what the agent will be sent
//! (removable context chips and three options), suggested questions, the
//! question, the provider, and "Start session". The request is built when the
//! button is pressed, from the data on screen at that moment; starting the
//! session is the shell's job and the page hands back the outcome.
use super::{
    github::GithubView,
    ui::{self, Dropdown, Tip},
};
use crate::{
    input::{self, Input},
    modal,
    settings::widgets::secondary_button,
    theme::*,
};
use gpui::{
    AnyElement, App, AppContext as _, Context, Entity, EventEmitter, FocusHandle, Focusable,
    IntoElement, Render, Role, SharedString, Subscription, Window, div, prelude::*, px, rgb,
};
use monocode_core::contributors::model::{
    ask::{
        AskAgentRequest, AskChip, AskChipId, AskContextPart, AskOptions, AskRequestInput,
        ask_context_chips, ask_subtitle, build_ask_request, person_name_of,
    },
    pipeline::ContributorsModel,
    snapshot::{SnapshotScope, snapshot_file_name, suggested_questions},
};
use monocode_core::provider::Kind;
use std::sync::Arc;

pub(crate) const TITLE: &str = "Ask Agent";
pub(crate) const FALLBACK_ERROR: &str = "Could not start the session.";
const CONTEXT_HEADING: &str = "Context sent to the agent";
const SUGGESTIONS_HEADING: &str = "Suggestions";
const MAX_WIDTH: f32 = 560.;

/// The providers the dialog offers and the one preselected: read when the dialog
/// opens so a freshly installed CLI shows up.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct AskChoices {
    pub providers: Vec<Kind>,
    pub default: Option<Kind>,
}

pub(crate) enum AskEvent {
    /// "Start session": the finished request. The dialog waits for the answer.
    Start(Box<AskAgentRequest>),
    /// Cancel, Escape, the X or a press outside, when no start is in flight.
    Close,
}

// ---- rules -----------------------------------------------------------------

/// The chip a checkbox of the context stands for; the fixed chips have none.
pub(crate) fn part_of(id: AskChipId) -> Option<AskContextPart> {
    match id {
        AskChipId::People => Some(AskContextPart::People),
        AskChipId::Scores => Some(AskContextPart::Scores),
        AskChipId::Formula => Some(AskContextPart::Formula),
        AskChipId::Ai => Some(AskContextPart::Ai),
        AskChipId::Project | AskChipId::Branch | AskChipId::Period | AskChipId::Focus => None,
    }
}

/// Removing a chip leaves its part out; pressing it again adds it back.
pub(crate) fn toggle_part(omit: &mut Vec<AskContextPart>, part: AskContextPart) {
    if let Some(at) = omit.iter().position(|p| *p == part) {
        omit.remove(at);
    } else {
        omit.push(part);
    }
}

/// `question.trim().length > 0 && !starting`; JS `trim` also drops a BOM.
pub(crate) fn can_start(question: &str, starting: bool) -> bool {
    !starting
        && !question
            .trim_matches(|c: char| c.is_whitespace() || c == '\u{feff}')
            .is_empty()
}

/// The default provider when it is offered, else the first one offered.
pub(crate) fn preselect(choices: &AskChoices) -> Option<Kind> {
    choices
        .default
        .filter(|kind| choices.providers.contains(kind))
        .or_else(|| choices.providers.first().copied())
}

/// A failure line for the dialog; blank text gets the generic one.
pub(crate) fn error_text(error: &str) -> String {
    let text = error.trim();
    if text.is_empty() {
        FALLBACK_ERROR.into()
    } else {
        text.into()
    }
}

/// The footer hint naming the file the session opens with.
pub(crate) fn attached_hint(file_name: &str) -> String {
    format!("Opens a new session with {file_name} attached")
}

/// What a model gives the dialog; derived once per model, not once per frame.
struct Derived {
    model: Arc<ContributorsModel>,
    chips: Vec<AskChip>,
    suggestions: Vec<String>,
    subtitle: String,
    file_name: String,
}

pub(crate) struct AskDialog {
    view: Entity<GithubView>,
    project: String,
    scope: SnapshotScope,
    providers: Vec<Kind>,
    harness: Option<Kind>,
    options: AskOptions,
    question: Entity<Input>,
    starting: bool,
    error: Option<String>,
    provider_dd: Dropdown,
    focus: FocusHandle,
    /// The question takes the keyboard on the first frame (the dialog has no window when made).
    focus_question: bool,
    derived: Option<Derived>,
    _subscriptions: Vec<Subscription>,
}
impl EventEmitter<AskEvent> for AskDialog {}
impl Focusable for AskDialog {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl AskDialog {
    pub(crate) fn new(
        view: Entity<GithubView>,
        project: String,
        scope: SnapshotScope,
        choices: AskChoices,
        cx: &mut Context<Self>,
    ) -> Self {
        let question = cx.new(Input::ask_question);
        let subscriptions = vec![
            cx.subscribe(&question, |this, _, _: &input::Submitted, cx| {
                this.start(cx)
            }),
            cx.observe(&question, |_, _, cx| cx.notify()),
            cx.observe(&view, |_, _, cx| cx.notify()),
        ];
        Self {
            view,
            project,
            scope,
            harness: preselect(&choices),
            providers: choices.providers,
            options: AskOptions::default(),
            question,
            starting: false,
            error: None,
            provider_dd: Dropdown::new(cx),
            focus: cx.focus_handle(),
            focus_question: true,
            derived: None,
            _subscriptions: subscriptions,
        }
    }

    #[cfg(test)]
    pub(crate) fn starting(&self) -> bool {
        self.starting
    }

    #[cfg(test)]
    pub(crate) fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    #[cfg(test)]
    pub(crate) fn harness(&self) -> Option<Kind> {
        self.harness
    }

    #[cfg(test)]
    pub(crate) fn options(&self) -> &AskOptions {
        &self.options
    }

    pub(crate) fn question_text(&self, cx: &App) -> String {
        self.question.read(cx).content.to_string()
    }

    /// The shell could not start the session: show why and let the user retry.
    pub(crate) fn failed(&mut self, error: &str, cx: &mut Context<Self>) {
        self.starting = false;
        self.error = Some(error_text(error));
        self.question.update(cx, |q, _| q.set_read_only(false));
        cx.notify();
    }

    fn toggle_chip(&mut self, part: AskContextPart, cx: &mut Context<Self>) {
        toggle_part(&mut self.options.omit, part);
        cx.notify();
    }

    fn set_question(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        let text = text.to_string();
        self.question.update(cx, |q, cx| q.set_text(text, cx));
        let handle = self.question.read(cx).focus_handle(cx);
        window.focus(&handle, cx);
    }

    /// Built now, not on open, so it matches the data on screen at this moment.
    fn request(&self, cx: &App) -> Result<AskAgentRequest, String> {
        let (model, now) = self
            .view
            .read(cx)
            .ask_inputs()
            .ok_or("The data is no longer on screen.")?;
        Ok(build_ask_request(&AskRequestInput {
            model: &model,
            project: &self.project,
            scope: self.scope.clone(),
            question: &self.question_text(cx),
            options: &self.options,
            harness: self.harness,
            now: &now,
        }))
    }

    pub(crate) fn start(&mut self, cx: &mut Context<Self>) {
        if !can_start(&self.question_text(cx), self.starting) {
            return;
        }
        self.error = None;
        match self.request(cx) {
            Ok(request) => {
                self.starting = true;
                self.question.update(cx, |q, _| q.set_read_only(true));
                cx.emit(AskEvent::Start(Box::new(request)));
            }
            Err(error) => self.error = Some(error_text(&error)),
        }
        cx.notify();
    }

    /// What the dialog shows for the data now: chips, suggestions, subtitle.
    #[cfg(test)]
    pub(super) fn shown(&mut self, cx: &App) -> Option<(Vec<AskChip>, Vec<String>, String)> {
        self.derive(cx)
            .map(|d| (d.chips.clone(), d.suggestions.clone(), d.subtitle.clone()))
    }

    #[cfg(test)]
    pub(super) fn question_input(&self) -> Entity<Input> {
        self.question.clone()
    }

    /// A start in flight cannot be taken back, so closing waits for it.
    pub(crate) fn request_close(&mut self, cx: &mut Context<Self>) {
        if !self.starting {
            cx.emit(AskEvent::Close);
        }
    }

    fn derive(&mut self, cx: &App) -> Option<&Derived> {
        let (model, _) = self.view.read(cx).ask_inputs()?;
        let fresh = self
            .derived
            .as_ref()
            .is_some_and(|d| Arc::ptr_eq(&d.model, &model));
        if !fresh {
            let name = person_name_of(&model, &self.scope);
            self.derived = Some(Derived {
                chips: ask_context_chips(&model, &self.project, &self.scope),
                suggestions: suggested_questions(
                    &self.scope,
                    name.as_deref(),
                    model.previous_period.is_some(),
                ),
                subtitle: ask_subtitle(&self.scope, name.as_deref()),
                file_name: snapshot_file_name(&model.period),
                model,
            });
        }
        self.derived.as_ref()
    }
}

// ---- drawing -----------------------------------------------------------------

fn heading(text: &str) -> gpui::Div {
    div()
        .mb(px(6.))
        .text_size(px(11.))
        .line_height(px(16.))
        .text_color(content(0.5))
        .child(text.to_uppercase())
}

impl AskDialog {
    fn chip(&self, chip: &AskChip, cx: &mut Context<Self>) -> AnyElement {
        let id = chip.id.as_str();
        let part = part_of(chip.id).filter(|_| chip.removable);
        let removed = part.is_some_and(|part| self.options.omit.contains(&part));
        div()
            .id(SharedString::from(format!("ask-chip-{id}")))
            .debug_selector(move || format!("ask-chip-{id}"))
            .flex()
            .items_center()
            .gap_1()
            .rounded_full()
            .border_1()
            .border_color(content(0.1))
            .bg(content(0.03))
            .py(px(2.))
            .pl(px(10.))
            .pr(px(if part.is_some() { 4. } else { 10. }))
            .text_size(px(12.))
            .line_height(px(16.))
            .text_color(content(if removed { 0.35 } else { 1. }))
            .when(removed, |d| d.line_through())
            .when_some(chip.hint, |d, hint| {
                d.tooltip(move |_, cx| cx.new(|_| Tip(hint.into())).into())
            })
            .child(chip.label.clone())
            .when_some(part, |d, part| {
                let verb = if removed { "Add back" } else { "Remove" };
                d.child(
                    div()
                        .id(SharedString::from(format!("ask-chip-toggle-{id}")))
                        .debug_selector(move || format!("ask-chip-toggle-{id}"))
                        .role(Role::Button)
                        .aria_label(format!("{verb} {}", chip.label))
                        .size(px(16.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded_full()
                        .cursor_pointer()
                        .hover(|s| s.bg(content(0.1)))
                        .child(
                            icon(if removed { "add-01" } else { "x" }, 12.)
                                .text_color(content(0.45)),
                        )
                        .on_click(cx.listener(move |this, _, _, cx| this.toggle_chip(part, cx))),
                )
            })
            .into_any_element()
    }

    fn option(
        &self,
        id: &'static str,
        checked: bool,
        label: AnyElement,
        cx: &mut Context<Self>,
        flip: fn(&mut AskOptions),
    ) -> AnyElement {
        div()
            .id(id)
            .debug_selector(move || id.to_string())
            .role(Role::CheckBox)
            .aria_toggled(if checked {
                gpui::Toggled::True
            } else {
                gpui::Toggled::False
            })
            .flex()
            .items_center()
            .gap_2()
            .cursor_pointer()
            .child(
                div()
                    .size(px(14.))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(4.))
                    .border_1()
                    .border_color(if checked {
                        accent_color()
                    } else {
                        content(0.3)
                    })
                    .when(checked, |d| {
                        d.bg(accent_color())
                            .child(icon("check", 10.).text_color(gpui::white()))
                    }),
            )
            .child(label)
            .on_click(cx.listener(move |this, _, _, cx| {
                flip(&mut this.options);
                cx.notify();
            }))
            .into_any_element()
    }

    fn code(text: &'static str) -> gpui::Div {
        div()
            .font_family(crate::fonts::mono())
            .text_size(px(11.))
            .child(text)
    }

    fn options_block(&self, cx: &mut Context<Self>) -> AnyElement {
        let text = |t: &'static str| div().child(t).into_any_element();
        let tools = div()
            .flex()
            .items_center()
            .gap(px(4.))
            .child("Allow the agent to run")
            .child(Self::code("git"))
            .child("/")
            .child(Self::code("gh"))
            .child("in the project to dig deeper")
            .into_any_element();
        div()
            .flex()
            .flex_col()
            .gap(px(6.))
            .child(self.option(
                "ask-option-per-pr",
                self.options.per_pr,
                text("Per-PR breakdown (titles, diff size, iteration, review)"),
                cx,
                |o| o.per_pr = !o.per_pr,
            ))
            .child(self.option(
                "ask-option-raw-commits",
                self.options.raw_commits,
                text("Raw commit list (hash, author, date, files)"),
                cx,
                |o| o.raw_commits = !o.raw_commits,
            ))
            .child(self.option(
                "ask-option-tools",
                self.options.allow_tools,
                tools,
                cx,
                |o| o.allow_tools = !o.allow_tools,
            ))
            .into_any_element()
    }

    fn footer(&mut self, file_name: &str, cx: &mut Context<Self>) -> AnyElement {
        let startable = can_start(&self.question_text(cx), self.starting);
        let starting = self.starting;
        let agent = match (self.providers.len(), self.harness) {
            (0, _) | (_, None) => None,
            (1, Some(kind)) => Some(
                div()
                    .debug_selector(|| "ask-agent-label".into())
                    .rounded_full()
                    .border_1()
                    .border_color(content(0.1))
                    .px(px(10.))
                    .py(px(2.))
                    .text_color(content(0.7))
                    .child(format!("Agent: {}", kind.label()))
                    .into_any_element(),
            ),
            (_, Some(kind)) => {
                let at = self.providers.iter().position(|k| *k == kind).unwrap_or(0);
                Some(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .child(div().text_color(content(0.5)).child("Agent"))
                        .child(ui::select_trigger(
                            "agent",
                            "Agent",
                            SharedString::from(kind.label()),
                            &self.provider_dd,
                            cx,
                            |this: &mut Self| &mut this.provider_dd,
                            at,
                        ))
                        .into_any_element(),
                )
            }
        };
        div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap_2()
            .children(agent)
            .child(
                div()
                    .debug_selector(|| "ask-hint".into())
                    .min_w_0()
                    .flex_1()
                    .text_size(px(11.))
                    .text_color(content(0.45))
                    .child(attached_hint(file_name)),
            )
            .child(
                secondary_button("ask-cancel", "Cancel", false, starting)
                    .debug_selector(|| "ask-cancel".into())
                    .child("Cancel")
                    .when(!starting, |b| {
                        b.on_click(cx.listener(|this, _, _, cx| this.request_close(cx)))
                    }),
            )
            .child(
                div()
                    .id("ask-start")
                    .debug_selector(|| "ask-start".into())
                    .role(Role::Button)
                    .aria_label("Start session")
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .rounded_md()
                    .bg(accent_color())
                    .px(px(12.))
                    .py(px(4.))
                    .text_size(px(12.))
                    .line_height(px(16.))
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .text_color(rgb(0xffffff))
                    .map(|d| {
                        if startable {
                            d.cursor_pointer()
                                .hover(|s| s.opacity(0.9))
                                .on_click(cx.listener(|this, _, _, cx| this.start(cx)))
                        } else {
                            d.opacity(0.4)
                        }
                    })
                    .when(starting, |d| {
                        d.child(crate::right_panel::spinner("ask-starting", "loader", 14.))
                    })
                    .child(if starting {
                        "Starting..."
                    } else {
                        "Start session"
                    }),
            )
            .into_any_element()
    }
}

impl Render for AskDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if std::mem::take(&mut self.focus_question) {
            let handle = self.question.read(cx).focus_handle(cx);
            window.focus(&handle, cx);
        }
        let viewport = window.viewport_size();
        let width = MAX_WIDTH.min(viewport.width.as_f32() - 24.);
        let Some(derived) = self.derive(cx) else {
            return div().into_any_element();
        };
        let (subtitle, file_name) = (derived.subtitle.clone(), derived.file_name.clone());
        let chips: Vec<AskChip> = derived.chips.clone();
        let suggestions = derived.suggestions.clone();

        let chip_row = div()
            .flex()
            .flex_wrap()
            .gap(px(6.))
            .children(chips.iter().map(|chip| self.chip(chip, cx)));
        let suggestion_rows = suggestions.into_iter().enumerate().map(|(ix, text)| {
            let pick = text.clone();
            div()
                .id(("ask-suggestion", ix))
                .debug_selector(move || format!("ask-suggestion-{ix}"))
                .role(Role::Button)
                .aria_label(text.clone())
                .rounded_md()
                .border_1()
                .border_color(content(0.1))
                .px_2()
                .py_1()
                .text_color(content(0.7))
                .cursor_pointer()
                .hover(|s| s.bg(content(0.05)).text_color(content(1.)))
                .child(text)
                .on_click(
                    cx.listener(move |this, _, window, cx| this.set_question(&pick, window, cx)),
                )
        });
        let suggestion_rows: Vec<_> = suggestion_rows.collect();
        let field = div()
            .debug_selector(|| "ask-question".into())
            .w_full()
            .rounded_md()
            .border_1()
            .border_color(content(0.1))
            .px(px(10.))
            .py_2()
            .when(self.starting, |d| d.opacity(0.6))
            .child(self.question.clone());
        let error = self.error.clone().map(|error| {
            div()
                .id("ask-error")
                .debug_selector(|| "ask-error".into())
                .role(Role::Alert)
                .aria_label(error.clone())
                .rounded_md()
                .border_1()
                .border_color(ui::red(0.3))
                .bg(ui::red(0.05))
                .px(px(10.))
                .py(px(6.))
                .text_color(rgb(ui::RED))
                .child(error)
        });
        let footer = self.footer(&file_name, cx);
        let options = self.options_block(cx);

        let body = div()
            .id("ask-body")
            .min_h_0()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap_3()
            .px_4()
            .pb_4()
            .pt_3()
            .text_size(px(12.))
            .line_height(px(16.))
            .text_color(content(1.))
            .child(
                div()
                    .id("ask-context")
                    .role(Role::Group)
                    .aria_label(CONTEXT_HEADING)
                    .child(heading(CONTEXT_HEADING))
                    .child(chip_row),
            )
            .child(options)
            .child(
                div()
                    .id("ask-suggestions")
                    .role(Role::Group)
                    .aria_label(SUGGESTIONS_HEADING)
                    .child(heading(SUGGESTIONS_HEADING))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .items_start()
                            .gap_1()
                            .children(suggestion_rows),
                    ),
            )
            .child(field)
            .children(error)
            .child(footer);

        let header = div()
            .flex_shrink_0()
            .flex()
            .items_start()
            .gap_2()
            .px_4()
            .pt_3()
            .child(
                div()
                    .min_w_0()
                    .flex_1()
                    .child(
                        div()
                            .text_size(px(15.))
                            .line_height(px(20.))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child(TITLE),
                    )
                    .child(
                        div()
                            .debug_selector(|| "ask-subtitle".into())
                            .truncate()
                            .text_size(px(12.))
                            .line_height(px(16.))
                            .text_color(content(0.5))
                            .child(subtitle),
                    ),
            )
            .child(
                div()
                    .id("ask-close")
                    .debug_selector(|| "ask-close".into())
                    .role(Role::Button)
                    .aria_label("Close")
                    .size(px(28.))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_md()
                    .when(!self.starting, |d| {
                        d.cursor_pointer().hover(|s| s.bg(content(0.08)))
                    })
                    .child(icon("x", 14.).text_color(content(0.45)))
                    .on_click(cx.listener(|this, _, _, cx| this.request_close(cx))),
            );

        let card = modal::card()
            .id("ask-dialog")
            .debug_selector(|| "ask-dialog".into())
            .role(Role::Dialog)
            .aria_label(TITLE)
            .track_focus(&self.focus)
            .w(px(width))
            .max_h(px(viewport.height.as_f32() - 32.))
            .flex()
            .flex_col()
            // Escape closes the dialog and nothing behind it, from any control in it.
            .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, _, cx| {
                if event.keystroke.key == "escape" {
                    this.request_close(cx);
                    cx.stop_propagation();
                }
            }))
            .child(
                div()
                    .relative()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .child(header)
                    .child(body),
            );
        let list = (self.provider_dd.open && self.providers.len() > 1).then(|| {
            ui::select_list(
                "agent",
                "Agent",
                self.harness.map(Kind::id).unwrap_or(""),
                self.providers
                    .iter()
                    .map(|kind| (kind.id().to_string(), SharedString::from(kind.label())))
                    .collect(),
                &self.provider_dd,
                cx,
                |this: &mut Self| &mut this.provider_dd,
                self.focus.clone(),
                |this, id, _, cx| {
                    if let Some(kind) = Kind::parse(&id) {
                        this.harness = Some(kind);
                        cx.notify();
                    }
                },
            )
        });
        let weak = cx.weak_entity();
        let overlay = div()
            .absolute()
            .inset_0()
            .child(modal::overlay("ask-agent", card, move |_, cx| {
                let _ = weak.update(cx, |this, cx| this.request_close(cx));
            }))
            .children(list);
        modal::window_overlay(window, overlay).into_any_element()
    }
}
