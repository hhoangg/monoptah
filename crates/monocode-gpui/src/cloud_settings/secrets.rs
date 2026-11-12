//! The Codespaces secrets block of the project's Cloud settings dialog, shown while the
//! Claude picker is on "Codespaces secret": whether `gh` can drive codespaces, which
//! repository the project belongs to, and the user secrets a codespace for it needs
//! (devcontainer secrets plus CLAUDE_CODE_OAUTH_TOKEN). `model` holds what it says and
//! decides. Every `gh` call runs on the background executor. Secret values live only in
//! the masked inputs until Save hands them to `gh`, then the inputs are emptied.
pub(crate) mod model;
#[cfg(test)]
mod tests;

use crate::{
    cloud_dialog_ui::{alert, button, muted},
    input::{self, Input},
    terminal::{Terminal, TerminalEvent},
    theme::*,
};
use gpui::{
    AnyElement, Context, Entity, FocusHandle, Focusable, FontWeight, IntoElement, Render,
    SharedString, StyleRefinement, Subscription, Window, div, prelude::*, px, rgb,
};
use model::{Gh, Row, Snapshot};
use monocode_core::codespaces::{GhRunner, OAUTH_SECRET_KEY, SecretState};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

/// The outside world the dialog reaches: `gh`, the project's git origin and the `claude` CLI.
#[derive(Clone)]
pub(crate) struct Env {
    pub runner: Arc<dyn GhRunner>,
    pub origin: fn(&Path) -> Option<String>,
    pub claude: fn() -> Option<PathBuf>,
}

fn join_lines(first: Option<String>, second: Option<String>) -> Option<String> {
    match (first, second) {
        (Some(a), Some(b)) => Some(format!("{a}\n{b}")),
        (a, b) => a.or(b),
    }
}

/// What the secrets block tells the dialog around it.
pub(crate) enum SecretsEvent {
    /// Enter in a secret field: the dialog's Save.
    Submitted,
    /// A save the dialog asked for with [`SecretsView::begin_save`] ended; `ok` when every
    /// typed secret reached GitHub.
    SaveFinished { ok: bool },
}

/// What [`SecretsView::begin_save`] did.
pub(crate) enum SaveStart {
    /// Nothing typed: nothing to wait for.
    Nothing,
    /// The secrets are on their way; a [`SecretsEvent::SaveFinished`] follows.
    Started,
    /// Secrets are typed but cannot be saved now; the text says why.
    Blocked(String),
}

pub(crate) struct SecretsView {
    project: String,
    env: Env,
    snapshot: Option<Snapshot>,
    /// One masked input per secret, in `snapshot.rows` order.
    fields: Vec<(String, Entity<Input>)>,
    busy: bool,
    error: Option<String>,
    notice: Option<String>,
    terminal: Option<Entity<Terminal>>,
    terminal_events: Option<Subscription>,
    /// A captured token waits for the running check or save to end, then is saved.
    pending_auto_save: bool,
    /// The running save was asked for by the dialog's Save, which waits for its end.
    dialog_save: bool,
    focus: FocusHandle,
    _events: Vec<Subscription>,
}

impl gpui::EventEmitter<SecretsEvent> for SecretsView {}

impl Focusable for SecretsView {
    fn focus_handle(&self, _: &gpui::App) -> FocusHandle {
        self.focus.clone()
    }
}

impl SecretsView {
    /// Starts empty: the dialog hands it the `gh` state with [`Self::refresh`] once its own
    /// Ready check has run `gh auth status`.
    pub(crate) fn new(project: String, env: Env, cx: &mut Context<Self>) -> Self {
        Self {
            project,
            env,
            snapshot: None,
            fields: Vec::new(),
            busy: false,
            error: None,
            notice: None,
            terminal: None,
            terminal_events: None,
            pending_auto_save: false,
            dialog_save: false,
            focus: cx.focus_handle(),
            _events: Vec::new(),
        }
    }

    /// Reads the secrets' status again, off the UI thread, for the `gh` state the dialog
    /// found. A refresh that arrives while one runs is dropped (the dialog asks again after
    /// every Ready check).
    pub(crate) fn refresh(&mut self, gh: Gh, cx: &mut Context<Self>) {
        if std::mem::replace(&mut self.busy, true) {
            return;
        }
        cx.notify();
        let (env, project) = (self.env.clone(), self.project.clone());
        cx.spawn(async move |this, cx| {
            let snapshot = cx
                .background_executor()
                .spawn(async move {
                    let path = Path::new(&project);
                    let origin = (env.origin)(path);
                    model::load(env.runner.as_ref(), path, origin.as_deref(), gh)
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.busy = false;
                this.apply(snapshot, cx);
                this.run_pending_auto_save(cx);
                cx.notify();
            });
        })
        .detach();
    }

    /// Shows a fresh snapshot. The inputs are built once, so typing survives a refresh.
    fn apply(&mut self, snapshot: Snapshot, cx: &mut Context<Self>) {
        for row in &snapshot.rows {
            if self.fields.iter().any(|(key, _)| *key == row.key) {
                continue;
            }
            let input = cx.new(|cx| {
                let mut field = Input::field(row.key.clone(), "Paste the value", content(0.35), cx);
                field.set_font_size(13.);
                field.set_secret(true);
                field
            });
            self._events
                .push(cx.subscribe(&input, |_, _, _: &input::Changed, cx| cx.notify()));
            self._events
                .push(cx.subscribe(&input, |_, _, _: &input::Submitted, cx| {
                    cx.emit(SecretsEvent::Submitted)
                }));
            self.fields.push((row.key.clone(), input));
        }
        self.snapshot = Some(snapshot);
    }

    #[cfg(test)]
    pub(crate) fn first_field_text(&self, cx: &gpui::App) -> String {
        self.fields
            .first()
            .map(|(_, input)| input.read(cx).content.to_string())
            .unwrap_or_default()
    }

    fn typed(&self, cx: &gpui::App) -> Vec<(String, String)> {
        self.fields
            .iter()
            .map(|(key, input)| (key.clone(), input.read(cx).content.to_string()))
            .collect()
    }

    fn has_input(&self, cx: &gpui::App) -> bool {
        !model::to_save(&self.typed(cx)).is_empty()
    }

    /// Whether any secret is typed and not saved yet.
    pub(crate) fn has_typed(&self, cx: &gpui::App) -> bool {
        self.has_input(cx)
    }

    /// The error shown in the block, if any.
    pub(crate) fn error_text(&self) -> Option<String> {
        self.error.clone()
    }

    fn clear(&mut self, key: &str, cx: &mut Context<Self>) {
        if let Some((_, input)) = self.fields.iter().find(|(k, _)| k == key) {
            input.update(cx, |input, cx| input.set_text(String::new(), cx));
        }
    }

    /// gh is ready and the repository is known: the two things a save needs.
    fn gh_ready(&self) -> bool {
        self.snapshot
            .as_ref()
            .is_some_and(|s| s.gh.ready() && s.repo.is_some())
    }

    /// Stores each non-empty field, then empties the ones that were saved and re-reads
    /// the status. A field whose save failed keeps its value so it can be retried.
    #[cfg(test)]
    pub(crate) fn save(&mut self, cx: &mut Context<Self>) {
        self.save_keys(None, false, cx);
    }

    /// The dialog's Save: stores what is typed and ends with [`SecretsEvent::SaveFinished`].
    /// Typed secrets that cannot be saved now are reported, never dropped silently.
    pub(crate) fn begin_save(&mut self, cx: &mut Context<Self>) -> SaveStart {
        if !self.has_input(cx) {
            return SaveStart::Nothing;
        }
        let blocked = if self.busy {
            Some("Still checking GitHub. Press Save again in a moment.".to_owned())
        } else {
            match &self.snapshot {
                None => Some("Still checking GitHub. Press Save again in a moment.".to_owned()),
                Some(snapshot) => snapshot.disabled_reason().map(|reason| {
                    format!(
                        "The Codespaces secrets cannot be saved yet. {}.",
                        reason.trim_end_matches('.')
                    )
                }),
            }
        };
        if let Some(message) = blocked {
            self.error = Some(message.clone());
            cx.notify();
            return SaveStart::Blocked(message);
        }
        self.dialog_save = true;
        self.save_keys(None, false, cx);
        SaveStart::Started
    }

    /// `save` limited to `only` when given; other typed fields are neither saved nor cleared.
    /// With `keep_messages` the error and notice already shown stay, and this save's
    /// outcome is added to them (a deferred auto-save must not hide the save before it).
    fn save_keys(&mut self, only: Option<&[&str]>, keep_messages: bool, cx: &mut Context<Self>) {
        if self.busy || !self.gh_ready() {
            return;
        }
        let Some((repo, gh)) = self
            .snapshot
            .as_ref()
            .and_then(|s| Some((s.repo.clone()?, s.gh.clone())))
        else {
            return;
        };
        let mut typed = self.typed(cx);
        if let Some(only) = only {
            typed.retain(|(key, _)| only.contains(&key.as_str()));
        }
        let entries = model::to_save(&typed);
        if entries.is_empty() {
            return;
        }
        self.busy = true;
        if !keep_messages {
            self.error = None;
            self.notice = None;
        }
        cx.notify();
        let (env, project) = (self.env.clone(), self.project.clone());
        cx.spawn(async move |this, cx| {
            let (outcome, snapshot) = cx
                .background_executor()
                .spawn({
                    let entries = entries.clone();
                    async move {
                        let outcome = model::save(env.runner.as_ref(), &repo, &entries);
                        let path = Path::new(&project);
                        let origin = (env.origin)(path);
                        let snapshot =
                            model::load(env.runner.as_ref(), path, origin.as_deref(), gh);
                        (outcome, snapshot)
                    }
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.busy = false;
                let ok = outcome.failed.is_empty();
                for key in &outcome.saved {
                    if let Some((_, value)) = entries.iter().find(|(k, _)| k == key) {
                        this.clear_if_unchanged(key, value, cx);
                    }
                }
                let saved = (!outcome.saved.is_empty())
                    .then(|| format!("Saved {}.", outcome.saved.join(", ")));
                if keep_messages {
                    this.error = join_lines(this.error.take(), outcome.message());
                    this.notice = join_lines(this.notice.take(), saved);
                } else {
                    this.error = outcome.message();
                    this.notice = saved;
                }
                this.apply(snapshot, cx);
                if std::mem::take(&mut this.dialog_save) {
                    cx.emit(SecretsEvent::SaveFinished { ok });
                }
                if this.pending_auto_save {
                    // That save decides whether the dialog is done.
                    this.run_pending_auto_save(cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Empties a saved field, unless it was changed while the save ran.
    fn clear_if_unchanged(&mut self, key: &str, saved: &str, cx: &mut Context<Self>) {
        let unchanged = self
            .fields
            .iter()
            .find(|(k, _)| k == key)
            .is_some_and(|(_, input)| input.read(cx).content.trim() == saved);
        if unchanged {
            self.clear(key, cx);
        }
    }

    /// Opens a local terminal running `claude setup-token`; the user copies the token it
    /// prints into the field.
    pub(crate) fn generate_token(&mut self, cx: &mut Context<Self>) {
        self.close_terminal(cx);
        let Some(program) = (self.env.claude)() else {
            self.error = Some("Claude Code CLI (claude) is not installed".into());
            cx.notify();
            return;
        };
        let cwd = if Path::new(&self.project).is_dir() {
            PathBuf::from(&self.project)
        } else {
            std::env::temp_dir()
        };
        self.error = None;
        let terminal = cx.new(|cx| {
            Terminal::launch(
                &cwd,
                program.into_os_string(),
                vec!["setup-token".into()],
                Vec::new(),
                Vec::new(),
                cx,
            )
        });
        self.terminal_events = Some(cx.subscribe(&terminal, Self::on_terminal_event));
        self.terminal = Some(terminal);
        cx.notify();
    }

    /// When `claude setup-token` ends having printed a token, puts it in the field,
    /// closes the terminal and saves it. Events from a terminal that is no longer the
    /// dialog's current one are ignored.
    fn on_terminal_event(
        &mut self,
        terminal: Entity<Terminal>,
        event: &TerminalEvent,
        cx: &mut Context<Self>,
    ) {
        let TerminalEvent::Exited { tail, .. } = event else {
            return;
        };
        if self.terminal.as_ref() != Some(&terminal) {
            return;
        }
        let Some(token) = model::captured_token(tail) else {
            return;
        };
        let Some((_, input)) = self
            .fields
            .iter()
            .find(|(key, _)| key == OAUTH_SECRET_KEY)
            .cloned()
        else {
            return;
        };
        input.update(cx, |input, cx| input.set_text(token, cx));
        self.close_terminal(cx);
        self.auto_save(cx);
        cx.notify();
    }

    /// Saves the captured token now, or once the running check or save has ended.
    fn auto_save(&mut self, cx: &mut Context<Self>) {
        if self.busy {
            self.pending_auto_save = true;
        } else {
            self.run_auto_save(false, cx);
        }
    }

    fn run_pending_auto_save(&mut self, cx: &mut Context<Self>) {
        if std::mem::take(&mut self.pending_auto_save) {
            self.run_auto_save(true, cx);
        }
    }

    /// Only the token field is saved: anything else half typed stays untouched.
    fn run_auto_save(&mut self, keep_messages: bool, cx: &mut Context<Self>) {
        if self.gh_ready() {
            self.save_keys(Some(&[OAUTH_SECRET_KEY]), keep_messages, cx);
        } else {
            let waiting = "The token is in the field. Press Save when the problem above is fixed.";
            let before = if keep_messages {
                self.notice.take()
            } else {
                None
            };
            self.notice = join_lines(before, Some(waiting.into()));
        }
    }

    /// The embedded terminal has the keyboard: Escape belongs to it (`claude setup-token`
    /// may need it), never to the dialog.
    pub(crate) fn terminal_focused(&self, window: &Window, cx: &gpui::App) -> bool {
        self.terminal
            .as_ref()
            .is_some_and(|t| t.read(cx).focus_handle(cx).contains_focused(window, cx))
    }

    /// Ends a running `claude setup-token` terminal so a token it prints later is not saved.
    /// Called when the Claude picker leaves "Codespaces secret".
    pub(crate) fn stop_generating(&mut self, cx: &mut Context<Self>) {
        self.pending_auto_save = false;
        if self.terminal.is_some() {
            self.close_terminal(cx);
            cx.notify();
        }
    }

    #[cfg(test)]
    pub(crate) fn generating(&self) -> bool {
        self.terminal.is_some()
    }

    #[cfg(test)]
    pub(crate) fn terminal_entity(&self) -> Option<Entity<Terminal>> {
        self.terminal.clone()
    }

    /// Types `text` into the secret field `key`.
    #[cfg(test)]
    pub(crate) fn set_field(&self, key: &str, text: &str, cx: &mut Context<Self>) {
        if let Some((_, input)) = self.fields.iter().find(|(k, _)| k == key) {
            input.update(cx, |input, cx| input.set_text(text.to_owned(), cx));
        }
    }

    fn close_terminal(&mut self, cx: &mut Context<Self>) {
        self.terminal_events = None;
        if let Some(terminal) = self.terminal.take() {
            terminal.read(cx).kill();
        }
    }

    /// Ends the terminal and drops anything typed; called when the dialog closes.
    pub(crate) fn shutdown(&mut self, cx: &mut Context<Self>) {
        self.pending_auto_save = false;
        self.dialog_save = false;
        self.close_terminal(cx);
        for (key, _) in self.fields.clone() {
            self.clear(&key, cx);
        }
    }

    fn secret_row(
        &mut self,
        row: &Row,
        snapshot: &Snapshot,
        enabled: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some((_, input)) = self.fields.iter().find(|(key, _)| *key == row.key).cloned() else {
            return div().into_any_element();
        };
        let busy = self.busy;
        input.update(cx, |input, cx| {
            input.set_read_only(busy || !enabled);
            input.set_placeholder(
                if enabled {
                    "Paste the value"
                } else {
                    "Unavailable until the problem above is fixed"
                },
                cx,
            );
        });
        let unknown = snapshot.unknown_reason(row);
        let state = row.state;
        let colour = match state {
            Some(SecretState::Set) => rgb(0x34d399),
            Some(SecretState::Missing) => rgb(0xfbbf24),
            Some(SecretState::NotForRepo) => rgb(0xfbbf24),
            None => rgb(0x9ca3af),
        };
        let id = SharedString::from(format!("codespaces-status-{}", row.key));
        div()
            .flex()
            .flex_col()
            .gap(px(6.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_3()
                    .child(
                        div()
                            .min_w_0()
                            .text_size(px(12.))
                            .font_weight(FontWeight::MEDIUM)
                            .child(row.key.clone()),
                    )
                    .child(
                        div()
                            .id(id.clone())
                            .debug_selector(move || id.to_string())
                            .text_size(px(11.))
                            .text_color(colour)
                            .child(model::state_label(state)),
                    ),
            )
            .when(!row.description.is_empty(), |d| {
                d.child(muted(row.description.clone()))
            })
            .when_some(unknown, |d, reason| {
                d.child(
                    div()
                        .id(SharedString::from(format!(
                            "codespaces-unknown-{}",
                            row.key
                        )))
                        .debug_selector({
                            let key = row.key.clone();
                            move || format!("codespaces-unknown-{key}")
                        })
                        .text_size(px(11.))
                        .line_height(px(17.))
                        .text_color(rgb(0xfbbf24))
                        .child(format!("Status unknown: {}", reason.trim_end_matches('.'))),
                )
            })
            .child(
                div()
                    .id(SharedString::from(format!("codespaces-field-{}", row.key)))
                    .debug_selector({
                        let key = row.key.clone();
                        move || format!("codespaces-field-{key}")
                    })
                    .when(!enabled, |d| d.opacity(0.45))
                    .h(px(36.))
                    .min_w_0()
                    .rounded_lg()
                    .border_1()
                    .border_color(content(0.15))
                    .bg(content(0.03))
                    .px_3()
                    .flex()
                    .items_center()
                    .text_color(content(1.))
                    .child(input),
            )
            .when(row.is_claude(), |d| {
                d.child(muted(model::CLAUDE_TOKEN_NOTE)).child(
                    div().child(
                        button(
                            "codespaces-generate",
                            "Generate with claude setup-token",
                            false,
                        )
                        .on_click(cx.listener(|this, _, _, cx| this.generate_token(cx))),
                    ),
                )
            })
            .into_any_element()
    }

    fn terminal_pane(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let terminal = self.terminal.clone()?;
        Some(
            div()
                .flex()
                .flex_col()
                .gap_2()
                .child(muted(
                    "Sign in when asked. The token it prints is saved automatically when it exits.",
                ))
                .child(
                    div()
                        .id("codespaces-terminal")
                        .debug_selector(|| "codespaces-terminal".into())
                        // Tall enough for the whole success screen: Claude Code only draws
                        // the part of a frame that fits, so a shorter pane loses the first
                        // lines of the token.
                        .h(px(400.))
                        .rounded_md()
                        .border_1()
                        .border_color(content(0.1))
                        .overflow_hidden()
                        .child(terminal.cached(StyleRefinement::default().size_full())),
                )
                .child(div().child(
                    button("codespaces-close-terminal", "Close terminal", false).on_click(
                        cx.listener(|this, _, _, cx| {
                            this.close_terminal(cx);
                            cx.notify();
                        }),
                    ),
                ))
                .into_any_element(),
        )
    }

    fn body(&mut self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let mut out: Vec<AnyElement> = vec![
            muted(model::PRIVACY_NOTE)
                .text_size(px(12.))
                .line_height(px(19.))
                .into_any_element(),
        ];
        let Some(snapshot) = self.snapshot.clone() else {
            out.push(
                muted("Checking GitHub CLI…")
                    .text_size(px(12.))
                    .into_any_element(),
            );
            return out;
        };
        if let Some(error) = snapshot.error.clone() {
            out.push(alert("codespaces-load-error", error).into_any_element());
        }
        let disabled = snapshot.disabled_reason();
        let enabled = disabled.is_none();
        if self.busy {
            out.push(
                muted("Checking GitHub…")
                    .text_size(px(12.))
                    .id("codespaces-checking")
                    .debug_selector(|| "codespaces-checking".into())
                    .into_any_element(),
            );
        } else if let Some(reason) = disabled {
            out.push(
                alert(
                    "codespaces-disabled",
                    format!(
                        "The fields below are read-only. {}.",
                        reason.trim_end_matches('.')
                    ),
                )
                .debug_selector(|| "codespaces-disabled".into())
                .into_any_element(),
            );
        }
        for row in &snapshot.rows {
            out.push(self.secret_row(row, &snapshot, enabled, cx));
        }
        out.extend(self.terminal_pane(cx));
        out
    }
}

impl Render for SecretsView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut body = self.body(cx);
        if let Some(error) = self.error.clone() {
            body.push(alert("codespaces-error", error).into_any_element());
        }
        if let Some(notice) = self.notice.clone() {
            body.push(
                div()
                    .id("codespaces-notice")
                    .text_size(px(12.))
                    .text_color(content(0.65))
                    .child(notice)
                    .into_any_element(),
            );
        }
        div()
            .id("codespaces-secrets")
            .debug_selector(|| "codespaces-secrets".into())
            .track_focus(&self.focus)
            .flex()
            .flex_col()
            .gap_3()
            .children(body)
    }
}
