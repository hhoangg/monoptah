//! The Git credential panel, the right-hand side panel of the Cloud settings dialog (Docker
//! Sandbox tab): pick a saved HTTPS profile or add one (GitHub or GitLab.com), check its read
//! access and remove it. It is not a modal: it renders inside the dialog, which stays the only
//! modal, and the dialog owns Escape and focus; Escape closes, in order, the "Confirm remove"
//! question and then the panel itself. `model` holds the decisions. The access token lives only in a masked input until Save hands it to
//! the encrypted store, then the input is emptied; it is never echoed back or logged. Check
//! access runs on the background executor.
mod model;
#[cfg(test)]
mod tests;

use crate::{
    cloud_dialog_ui::{alert, button, choice, field_box, heading, labelled, muted},
    input::{self, Input},
    modal,
    runtime::{Database, locked},
    theme::*,
};
use gpui::{
    AnyElement, Context, Entity, EventEmitter, FocusHandle, Focusable, IntoElement, Render, Role,
    Window, div, prelude::*, px, rgb,
};
use model::CheckLine;
use monocode_core::git_credentials::{
    GitCredentialSummary, GitCredentials, GitPlatform, GitRunner, Secret,
};
use std::sync::Arc;

pub(crate) enum Event {
    Close,
    /// A profile was saved or picked: Cloud settings selects it.
    Chosen(String),
    /// A profile was removed: the saved list the dialog shows is out of date.
    Removed,
}

#[derive(Clone)]
pub(crate) struct Env {
    pub store: Database,
    pub git: Arc<dyn GitRunner>,
}

pub(crate) struct GitCredentialPanel {
    /// The profile to show once the list has been read.
    pending_select: Option<String>,
    env: Env,
    project_name: String,
    repo_url: String,
    profiles: Vec<GitCredentialSummary>,
    /// The profile shown; none while a new one is being added.
    selected: Option<String>,
    platform: GitPlatform,
    /// The user picked the platform: the repository no longer decides it.
    platform_picked: bool,
    label: Entity<Input>,
    username: Entity<Input>,
    token: Entity<Input>,
    /// A save or remove is running.
    busy: bool,
    /// An access check is running.
    checking: bool,
    /// Bumped whenever the selection or platform changes; a check result for an older
    /// value is dropped.
    generation: u64,
    check: Option<CheckLine>,
    error: Option<String>,
    confirm_remove: bool,
    focus: FocusHandle,
    _events: Vec<gpui::Subscription>,
}

impl EventEmitter<Event> for GitCredentialPanel {}

impl Focusable for GitCredentialPanel {
    fn focus_handle(&self, _: &gpui::App) -> FocusHandle {
        self.focus.clone()
    }
}

impl GitCredentialPanel {
    pub(crate) fn new(
        project_name: String,
        repo_url: String,
        selected: Option<String>,
        env: Env,
        cx: &mut Context<Self>,
    ) -> Self {
        let make = |label: &'static str, placeholder: &'static str, cx: &mut Context<Self>| {
            cx.new(|cx| {
                let mut field = Input::field(label, placeholder, content(0.35), cx);
                field.set_font_size(13.);
                field
            })
        };
        let label = make("Profile name", "Work repositories", cx);
        let username = make("Username", "", cx);
        let token = make("Access token", "Paste the token", cx);
        token.update(cx, |input, _| input.set_secret(true));
        let platform = git_credentials_platform_of(&repo_url);
        let mut events = Vec::new();
        for input in [&label, &username, &token] {
            events.push(cx.subscribe(input, |_, _, _: &input::Changed, cx| cx.notify()));
        }
        events.push(cx.subscribe(&token, |this, _, _: &input::Submitted, cx| this.save(cx)));
        let mut this = Self {
            pending_select: selected,
            env,
            project_name,
            repo_url,
            profiles: Vec::new(),
            selected: None,
            platform,
            platform_picked: false,
            label,
            username,
            token,
            busy: false,
            checking: false,
            generation: 0,
            check: None,
            error: None,
            confirm_remove: false,
            focus: cx.focus_handle(),
            _events: events,
        };
        this.reload(cx);
        this
    }

    fn credentials(&self) -> Result<GitCredentials, String> {
        locked(&self.env.store).and_then(|store| GitCredentials::for_store(&store))
    }

    /// Reads the saved profiles off the UI thread.
    fn reload(&mut self, cx: &mut Context<Self>) {
        let credentials = self.credentials();
        cx.spawn(async move |this, cx| {
            let listed = cx
                .background_executor()
                .spawn(async move { credentials.and_then(|c| c.list()) })
                .await;
            let _ = this.update(cx, |this, cx| {
                match listed {
                    Ok(list) => this.profiles = list,
                    Err(message) => {
                        this.profiles.clear();
                        this.error = Some(message);
                    }
                }
                if let Some(id) = this.pending_select.take()
                    && this.profiles.iter().any(|p| p.id == id)
                {
                    this.selected = Some(id);
                }
                if this
                    .selected
                    .as_ref()
                    .is_some_and(|id| !this.profiles.iter().any(|p| &p.id == id))
                {
                    this.selected = None;
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn clear_token(&mut self, cx: &mut Context<Self>) {
        self.token
            .update(cx, |input, cx| input.set_text(String::new(), cx));
    }

    /// Drops anything typed; called when the dialog closes.
    pub(crate) fn shutdown(&mut self, cx: &mut Context<Self>) {
        self.clear_token(cx);
    }

    /// Escape: the "Confirm remove" question closes first. Returns whether something closed.
    pub(crate) fn dismiss_transient(&mut self, cx: &mut Context<Self>) -> bool {
        let asked = std::mem::take(&mut self.confirm_remove);
        if asked {
            cx.notify();
        }
        asked
    }

    /// The dialog's repository URL changed: a check made for the old one no longer applies.
    pub(crate) fn set_repo_url(&mut self, repo_url: String, cx: &mut Context<Self>) {
        if self.repo_url == repo_url {
            return;
        }
        self.repo_url = repo_url;
        // The default platform follows the repository host until the user picks one.
        if !self.platform_picked
            && let Ok((platform, _, _)) =
                monocode_core::git_credentials::normalize_origin(&self.repo_url)
        {
            self.apply_platform(platform, cx);
        }
        self.generation += 1;
        self.checking = false;
        self.check = None;
        cx.notify();
    }

    #[cfg(test)]
    pub(crate) fn confirm_remove_asked(&self) -> bool {
        self.confirm_remove
    }

    #[cfg(test)]
    pub(crate) fn typed(&self, cx: &gpui::App) -> (String, String) {
        (
            self.label.read(cx).content.to_string(),
            self.token.read(cx).content.to_string(),
        )
    }

    /// Where the keyboard goes when the panel opens: the first field of the new-credential
    /// form, or the panel itself when a saved profile is shown (or about to be): that form
    /// has no input, and a field that is not drawn cannot take focus.
    pub(crate) fn focus_form(&self, window: &mut Window, cx: &mut gpui::App) {
        if self.selected.is_some() || self.pending_select.is_some() {
            window.focus(&self.focus, cx);
        } else {
            window.focus(&self.label.focus_handle(cx), cx);
        }
    }

    fn select(&mut self, id: Option<String>, cx: &mut Context<Self>) {
        self.selected = id;
        self.generation += 1;
        self.checking = false;
        self.check = None;
        self.error = None;
        self.confirm_remove = false;
        self.clear_token(cx);
        cx.notify();
    }

    pub(crate) fn set_platform(&mut self, platform: GitPlatform, cx: &mut Context<Self>) {
        self.platform_picked = true;
        self.apply_platform(platform, cx);
    }

    fn apply_platform(&mut self, platform: GitPlatform, cx: &mut Context<Self>) {
        if self.platform == platform {
            return;
        }
        let typed = self.username.read(cx).content.to_string();
        if typed.is_empty() || typed == model::default_username(self.platform) {
            let next = model::default_username(platform).to_owned();
            self.username
                .update(cx, |input, cx| input.set_text(next, cx));
        }
        self.platform = platform;
        self.generation += 1;
        self.checking = false;
        self.check = None;
        cx.notify();
    }

    /// Saves a new profile off the UI thread, or picks the shown one. The token is emptied
    /// once stored.
    pub(crate) fn save(&mut self, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        if let Some(id) = self.selected.clone() {
            cx.emit(Event::Chosen(id));
            return;
        }
        let token = Secret::new(self.token.read(cx).content.trim().to_owned());
        if token.as_str().is_empty() {
            self.error = Some("Paste the access token.".into());
            cx.notify();
            return;
        }
        let (label, username, platform) = (
            self.label.read(cx).content.to_string(),
            self.username.read(cx).content.to_string(),
            self.platform,
        );
        let credentials = self.credentials();
        self.busy = true;
        self.error = None;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    credentials.and_then(|credentials| {
                        model::save_credential(
                            &credentials,
                            &label,
                            platform,
                            &username,
                            token.as_str(),
                        )
                    })
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.busy = false;
                match result {
                    Ok(id) => {
                        this.clear_token(cx);
                        cx.emit(Event::Chosen(id));
                    }
                    Err(message) => this.error = Some(message),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Checks read access of the shown profile, or of the typed token, off the UI thread.
    /// An unsaved token is checked from memory and never stored.
    pub(crate) fn check_access(&mut self, cx: &mut Context<Self>) {
        if self.checking {
            return;
        }
        let platform = self
            .selected
            .as_ref()
            .and_then(|id| self.profiles.iter().find(|p| &p.id == id))
            .map_or(self.platform, |p| p.platform);
        if let Some(problem) = model::repo_problem(&self.repo_url, platform) {
            self.check = Some(CheckLine {
                ok: false,
                text: problem,
            });
            cx.notify();
            return;
        }
        let selected = self.selected.clone();
        let username = self.username.read(cx).content.to_string();
        let token = Secret::new(self.token.read(cx).content.trim().to_owned());
        if selected.is_none() && token.as_str().is_empty() {
            self.check = Some(CheckLine {
                ok: false,
                text: "Paste the access token to check it.".into(),
            });
            cx.notify();
            return;
        }
        let credentials = if selected.is_some() {
            match self.credentials() {
                Ok(credentials) => Some(credentials),
                Err(message) => {
                    self.error = Some(message);
                    cx.notify();
                    return;
                }
            }
        } else {
            None
        };
        self.checking = true;
        self.check = None;
        self.error = None;
        let generation = self.generation;
        cx.notify();
        let (git, repo_url) = (self.env.git.clone(), self.repo_url.clone());
        cx.spawn(async move |this, cx| {
            let line = cx
                .background_executor()
                .spawn(async move {
                    match (selected, credentials) {
                        (Some(id), Some(credentials)) => {
                            model::check_saved(&credentials, git.as_ref(), &id, &repo_url)
                        }
                        _ => model::check_unsaved(
                            git.as_ref(),
                            platform,
                            &username,
                            token,
                            &repo_url,
                        ),
                    }
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                // The selection changed while this ran: the answer is for something else.
                if this.generation != generation {
                    return;
                }
                this.checking = false;
                this.check = Some(line);
                cx.notify();
            });
        })
        .detach();
    }

    pub(crate) fn remove(&mut self, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let Some(id) = self.selected.clone() else {
            return;
        };
        if !self.confirm_remove {
            self.confirm_remove = true;
            cx.notify();
            return;
        }
        let credentials = self.credentials();
        self.busy = true;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { credentials.and_then(|c| c.remove(&id)) })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.busy = false;
                match result {
                    Ok(()) => {
                        this.select(None, cx);
                        this.reload(cx);
                        cx.emit(Event::Removed);
                    }
                    Err(message) => {
                        this.confirm_remove = false;
                        this.error = Some(message);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn platform_row(&self, cx: &mut Context<Self>) -> AnyElement {
        let mut row = labelled("Platform").gap(px(2.));
        for (platform, name) in [
            (GitPlatform::GitHub, "GitHub"),
            (GitPlatform::GitLab, "GitLab"),
        ] {
            row = row.child(
                choice(
                    format!("git-platform-{}", platform.host()),
                    name,
                    self.platform == platform,
                )
                .on_click(cx.listener(move |this, _, _, cx| this.set_platform(platform, cx))),
            );
        }
        row.into_any_element()
    }

    fn form(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let existing = self
            .selected
            .as_ref()
            .and_then(|id| self.profiles.iter().find(|p| &p.id == id));
        if let Some(profile) = existing {
            return vec![
                muted(format!(
                    "{} · {} · user {}",
                    profile.label,
                    profile.platform.host(),
                    profile.username
                ))
                .text_size(px(12.))
                .into_any_element(),
                muted("The token stays encrypted and is never shown again.").into_any_element(),
            ];
        }
        vec![
            labelled("Profile name")
                .child(field_box("git-field-label", &self.label))
                .into_any_element(),
            self.platform_row(cx),
            labelled("Host")
                .child(
                    div()
                        .id("git-host")
                        .debug_selector(|| "git-host".into())
                        .h(px(36.))
                        .flex()
                        .items_center()
                        .px_3()
                        .rounded_lg()
                        .border_1()
                        .border_color(content(0.1))
                        .text_size(px(13.))
                        .text_color(content(0.6))
                        .child(self.platform.host()),
                )
                .into_any_element(),
            labelled("Username")
                .child(field_box("git-field-username", &self.username))
                .child(muted(model::username_hint(self.platform)))
                .into_any_element(),
            labelled("Access token")
                .child(field_box("git-field-token", &self.token))
                .child(muted(model::TOKEN_HINT))
                .child(muted(model::SCOPE_HINT))
                .into_any_element(),
        ]
    }
}

fn git_credentials_platform_of(repo_url: &str) -> GitPlatform {
    monocode_core::git_credentials::normalize_origin(repo_url)
        .map_or(GitPlatform::GitHub, |(platform, _, _)| platform)
}

impl Render for GitCredentialPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let busy = self.busy;
        let checking = self.checking;
        let existing = self.selected.is_some();
        let mut picker = labelled("Saved profile").gap(px(2.));
        for profile in &self.profiles {
            let id = profile.id.clone();
            picker = picker.child(
                choice(
                    format!("git-profile-{}", profile.id),
                    model::label_of(profile),
                    self.selected.as_deref() == Some(profile.id.as_str()),
                )
                .on_click(cx.listener(move |this, _, _, cx| this.select(Some(id.clone()), cx))),
            );
        }
        picker = picker.child(
            choice(
                "git-profile-new",
                "New credential…",
                self.selected.is_none(),
            )
            .on_click(cx.listener(|this, _, _, cx| this.select(None, cx))),
        );
        let mut body = div()
            .id("git-panel-body")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap_4()
            .p_4()
            .child(muted(
                "Select a saved profile or add an HTTPS access token. This credential can be reused across projects.",
            ))
            .child(picker)
            .children(self.form(cx))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(
                        button(
                            "git-check",
                            if checking { "Checking…" } else { "Check access" },
                            checking,
                        )
                        .when(!checking, |b| {
                            b.on_click(cx.listener(|this, _, _, cx| this.check_access(cx)))
                        }),
                    )
                    .child(match &self.check {
                        Some(line) => div()
                            .id("git-check-result")
                            .debug_selector(|| "git-check-result".into())
                            .min_w_0()
                            .text_size(px(12.))
                            .line_height(px(18.))
                            .text_color(if line.ok {
                                rgb(0x34d399)
                            } else {
                                rgb(0xf87171)
                            })
                            .child(line.text.clone()),
                        None => div()
                            .id("git-check-result")
                            .text_size(px(12.))
                            .text_color(content(0.45))
                            .child("Not checked"),
                    }),
            );
        if let Some(message) = self.error.clone() {
            body = body.child(alert("git-error", message));
        }
        let mut actions = modal::actions().mt(px(0.));
        if existing {
            let confirming = self.confirm_remove;
            actions = actions.child(
                modal::danger_ghost_button(
                    "git-remove",
                    if confirming {
                        "Confirm remove"
                    } else {
                        "Remove"
                    },
                )
                .debug_selector(|| "git-remove".into())
                .cursor_pointer()
                .on_click(cx.listener(|this, _, _, cx| this.remove(cx))),
            );
        }
        let footer = div()
            .flex_shrink_0()
            .px_4()
            .pb_4()
            .pt_3()
            .border_t_1()
            .border_color(content(0.08))
            .child(
                actions
                    .child(
                        modal::ghost_button("git-cancel", "Close")
                            .debug_selector(|| "git-cancel".into())
                            .cursor_pointer()
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(Event::Close))),
                    )
                    .child(
                        modal::primary_button(
                            "git-save",
                            if existing {
                                "Use credential"
                            } else {
                                "Save credential"
                            },
                        )
                        .debug_selector(|| "git-save".into())
                        .when(busy, |b| b.opacity(0.4))
                        .cursor_pointer()
                        .on_click(cx.listener(|this, _, _, cx| this.save(cx))),
                    ),
            );
        div()
            .id("git-credential-panel")
            .debug_selector(|| "git-credential-panel".into())
            .role(Role::Group)
            .aria_label("Git credential")
            .track_focus(&self.focus)
            .relative()
            .size_full()
            .min_h_0()
            .flex()
            .flex_col()
            .text_color(content(1.))
            .child(
                div()
                    .flex_shrink_0()
                    .flex()
                    .items_start()
                    .gap_2()
                    .px_4()
                    .pt_3()
                    .child(heading(
                        "Git credential",
                        format!("Reusable HTTPS profile for {}", self.project_name),
                    ))
                    .child(
                        icon_button("git-close", "Close Git credential", "close")
                            .size(px(28.))
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(Event::Close))),
                    ),
            )
            .child(body)
            .child(footer)
    }
}
