//! The project menu's Codespaces setup dialog: whether `gh` can drive codespaces, which
//! repository the project belongs to, and the user secrets a codespace for it needs
//! (devcontainer secrets plus CLAUDE_CODE_OAUTH_TOKEN). `model` holds what it says and
//! decides. Every `gh` call runs on the background executor. Secret values live only in
//! the masked inputs until Save hands them to `gh`, then the inputs are emptied.
mod model;
#[cfg(test)]
mod tests;

use crate::{
    input::{self, Input},
    settings::widgets::secondary_button,
    terminal::Terminal,
    theme::*,
};
use gpui::{
    AnyElement, Context, Div, Entity, EventEmitter, FocusHandle, Focusable, FontWeight,
    IntoElement, KeyDownEvent, Render, Role, SharedString, Stateful, StyleRefinement, Subscription,
    Window, div, prelude::*, px, rgb,
};
use model::{Row, Snapshot};
use monocode_core::codespaces::{GhRunner, SecretState, SystemGh};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

pub(crate) enum Event {
    Close,
}

/// The outside world the dialog reaches: `gh`, the project's git origin and the `claude` CLI.
#[derive(Clone)]
pub(crate) struct Env {
    pub runner: Arc<dyn GhRunner>,
    pub origin: fn(&Path) -> Option<String>,
    pub claude: fn() -> Option<PathBuf>,
}

impl Env {
    pub(crate) fn system() -> Self {
        Self {
            runner: Arc::new(SystemGh),
            origin: model::git_origin,
            claude: || monocode_core::scm::resolve_gui_binary("claude"),
        }
    }
}

fn muted(text: impl Into<SharedString>) -> Div {
    div()
        .text_size(px(11.))
        .line_height(px(17.))
        .text_color(content(0.45))
        .child(text.into())
}

fn alert(id: &'static str, text: impl Into<SharedString>) -> Stateful<Div> {
    div()
        .id(id)
        .role(Role::Alert)
        .text_size(px(12.))
        .line_height(px(18.))
        .text_color(rgb(0xf87171))
        .child(text.into())
}

fn code_box(text: &str) -> Div {
    div()
        .min_w_0()
        .rounded_md()
        .border_1()
        .border_color(content(0.1))
        .px(px(8.))
        .py(px(4.))
        .text_size(px(12.))
        .line_height(px(18.))
        .child(text.to_owned())
}

fn button(id: &'static str, label: impl Into<SharedString>, disabled: bool) -> Stateful<Div> {
    secondary_button(id, label, false, disabled).debug_selector(move || id.into())
}

fn section() -> Div {
    div()
        .flex()
        .flex_col()
        .gap_2()
        .border_t_1()
        .border_color(content(0.08))
        .pt_4()
}

pub(crate) struct CodespacesDialog {
    project: String,
    name: String,
    env: Env,
    snapshot: Option<Snapshot>,
    /// One masked input per secret, in `snapshot.rows` order.
    fields: Vec<(String, Entity<Input>)>,
    busy: bool,
    error: Option<String>,
    notice: Option<String>,
    terminal: Option<Entity<Terminal>>,
    focus: FocusHandle,
    _events: Vec<Subscription>,
}

impl EventEmitter<Event> for CodespacesDialog {}

impl Focusable for CodespacesDialog {
    fn focus_handle(&self, _: &gpui::App) -> FocusHandle {
        self.focus.clone()
    }
}

impl CodespacesDialog {
    pub(crate) fn new(project: String, name: String, env: Env, cx: &mut Context<Self>) -> Self {
        let mut this = Self {
            project,
            name,
            env,
            snapshot: None,
            fields: Vec::new(),
            busy: false,
            error: None,
            notice: None,
            terminal: None,
            focus: cx.focus_handle(),
            _events: Vec::new(),
        };
        this.refresh(cx);
        this
    }

    /// Checks gh and the secrets again, off the UI thread.
    pub(crate) fn refresh(&mut self, cx: &mut Context<Self>) {
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
                    model::load(env.runner.as_ref(), path, origin.as_deref())
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.busy = false;
                this.apply(snapshot, cx);
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
                .push(cx.subscribe(&input, |this, _, _: &input::Submitted, cx| this.save(cx)));
            self.fields.push((row.key.clone(), input));
        }
        self.snapshot = Some(snapshot);
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

    fn clear(&mut self, key: &str, cx: &mut Context<Self>) {
        if let Some((_, input)) = self.fields.iter().find(|(k, _)| k == key) {
            input.update(cx, |input, cx| input.set_text(String::new(), cx));
        }
    }

    fn can_save(&self, cx: &gpui::App) -> bool {
        !self.busy
            && self.has_input(cx)
            && self
                .snapshot
                .as_ref()
                .is_some_and(|s| s.gh.ready() && s.repo.is_some())
    }

    /// Stores each non-empty field, then empties the ones that were saved and re-reads
    /// the status. A field whose save failed keeps its value so it can be retried.
    pub(crate) fn save(&mut self, cx: &mut Context<Self>) {
        if !self.can_save(cx) {
            return;
        }
        let Some(repo) = self.snapshot.as_ref().and_then(|s| s.repo.clone()) else {
            return;
        };
        let entries = model::to_save(&self.typed(cx));
        self.busy = true;
        self.error = None;
        self.notice = None;
        cx.notify();
        let (env, project) = (self.env.clone(), self.project.clone());
        cx.spawn(async move |this, cx| {
            let (outcome, snapshot) = cx
                .background_executor()
                .spawn(async move {
                    let outcome = model::save(env.runner.as_ref(), &repo, &entries);
                    let path = Path::new(&project);
                    let origin = (env.origin)(path);
                    let snapshot = model::load(env.runner.as_ref(), path, origin.as_deref());
                    (outcome, snapshot)
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.busy = false;
                for key in &outcome.saved {
                    this.clear(key, cx);
                }
                this.error = outcome.message();
                if !outcome.saved.is_empty() {
                    this.notice = Some(format!("Saved {}.", outcome.saved.join(", ")));
                }
                this.apply(snapshot, cx);
                cx.notify();
            });
        })
        .detach();
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
        self.terminal = Some(terminal);
        cx.notify();
    }

    fn close_terminal(&mut self, cx: &mut Context<Self>) {
        if let Some(terminal) = self.terminal.take() {
            terminal.read(cx).kill();
        }
    }

    /// Ends the terminal and drops anything typed; called when the dialog closes.
    pub(crate) fn shutdown(&mut self, cx: &mut Context<Self>) {
        self.close_terminal(cx);
        for (key, _) in self.fields.clone() {
            self.clear(&key, cx);
        }
    }

    fn status_section(&self, snapshot: &Snapshot) -> Div {
        let gh = &snapshot.gh;
        let mut column = section().child(
            div()
                .text_size(px(13.))
                .font_weight(FontWeight::MEDIUM)
                .child(gh.line()),
        );
        if let Some(fix) = gh.fix() {
            column = column
                .child(muted("Run this in a terminal, then Check again:"))
                .child(code_box(fix));
        }
        column.child(muted(match &snapshot.repo {
            Some(repo) => format!("Repository: {repo}"),
            None => "No GitHub repository found: the project needs a git origin on github.com."
                .to_owned(),
        }))
    }

    fn secret_row(&mut self, row: &Row, enabled: bool, cx: &mut Context<Self>) -> AnyElement {
        let Some((_, input)) = self.fields.iter().find(|(key, _)| *key == row.key).cloned() else {
            return div().into_any_element();
        };
        let busy = self.busy;
        input.update(cx, |input, _| input.set_read_only(busy || !enabled));
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
            .child(
                div()
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
                    "Sign in when asked, then copy the token it prints into the field above.",
                ))
                .child(
                    div()
                        .id("codespaces-terminal")
                        .debug_selector(|| "codespaces-terminal".into())
                        .h(px(200.))
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
        out.push(self.status_section(&snapshot).into_any_element());
        if let Some(error) = snapshot.error.clone() {
            out.push(alert("codespaces-load-error", error).into_any_element());
        }
        let enabled = snapshot.gh.ready() && snapshot.repo.is_some();
        for row in &snapshot.rows {
            out.push(self.secret_row(row, enabled, cx));
        }
        out.extend(self.terminal_pane(cx));
        let save_enabled = self.can_save(cx);
        let busy = self.busy;
        out.push(
            div()
                .flex()
                .items_center()
                .gap_3()
                .child(
                    button(
                        "codespaces-save",
                        if busy { "Working…" } else { "Save" },
                        !save_enabled,
                    )
                    .when(save_enabled, |b| {
                        b.on_click(cx.listener(|this, _, _, cx| this.save(cx)))
                    }),
                )
                .child(
                    button("codespaces-check", "Check again", busy).when(!busy, |b| {
                        b.on_click(cx.listener(|this, _, _, cx| this.refresh(cx)))
                    }),
                )
                .into_any_element(),
        );
        out
    }
}

impl Render for CodespacesDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
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
        let height = window.viewport_size().height.as_f32() - 34.;
        div()
            .id("codespaces-dialog")
            .debug_selector(|| "codespaces-dialog".into())
            .role(Role::Dialog)
            .aria_label("Codespaces setup")
            .track_focus(&self.focus)
            .relative()
            .min_h_0()
            .max_h(px(height))
            .flex()
            .flex_col()
            .text_color(content(1.))
            .on_key_down(cx.listener(|_, event: &KeyDownEvent, window, cx| {
                if event.keystroke.key == "escape" {
                    cx.emit(Event::Close);
                    window.prevent_default();
                    cx.stop_propagation();
                }
            }))
            .child(
                div()
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
                            .pt(px(2.))
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .text_size(px(16.))
                                    .line_height(px(20.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child("Codespaces setup"),
                            )
                            .child(
                                div()
                                    .mt(px(2.))
                                    .min_w_0()
                                    .truncate()
                                    .text_size(px(12.))
                                    .line_height(px(16.5))
                                    .text_color(content(0.5))
                                    .child(format!(
                                        "Secrets for running {} in a GitHub codespace",
                                        self.name
                                    )),
                            ),
                    )
                    .child(
                        icon_button("codespaces-close", "Close Codespaces setup", "close")
                            .size(px(28.))
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(Event::Close))),
                    ),
            )
            .child(
                div()
                    .id("codespaces-dialog-body")
                    .min_h_0()
                    .overflow_y_scroll()
                    .flex()
                    .flex_col()
                    .gap_4()
                    .p_4()
                    .children(body),
            )
    }
}
