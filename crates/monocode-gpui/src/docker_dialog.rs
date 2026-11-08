//! The project menu's Docker Sandbox setup dialog: whether this machine can reach Docker
//! cloud sandboxes, and the project's repository, Git credential, devcontainer, size and
//! lifetime. `model` holds what it says and decides. The access check runs on the
//! background executor and only reads. The app never runs `sbx login` or `sbx --cloud setup
//! ssh`: the dialog shows the command for the user to run. Save writes this project's config
//! and nothing else: it never provisions a sandbox or calls the cloud.
pub(crate) mod model;
#[cfg(test)]
mod tests;

use crate::{
    cloud_dialog_ui::{alert, choice, code_box, field_box, heading, labelled, muted},
    input::{self, Input},
    modal,
    runtime::{Database, locked},
    theme::*,
};
use gpui::{
    AnyElement, Context, Div, Entity, EventEmitter, FocusHandle, Focusable, FontWeight,
    IntoElement, KeyDownEvent, Render, Role, Subscription, Window, div, prelude::*, px, rgb,
};
use model::{Access, AccountChoice, Field, FieldError, Form};
use monocode_core::{
    cloud_sessions::docker_config::{self, DockerProjectConfig},
    git_credentials::{GitCredentialSummary, GitCredentials, GitRunner, SystemGit},
    sandboxes::{SbxRunner, SystemSbx},
};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

/// The Claude accounts that hold a setup-token or an API key: the ones a sandbox can use.
pub(crate) fn claude_accounts(cx: &gpui::App) -> Vec<AccountChoice> {
    use monocode_core::{accounts::ClaudeCredential, provider::Kind};
    let Some(center) = crate::accounts::get(cx) else {
        return Vec::new();
    };
    let center = center.read(cx);
    center
        .book
        .list(Kind::Claude)
        .into_iter()
        .filter(|account| center.uses_claude_token(&account.id))
        .map(|account| AccountChoice {
            kind: match center.claude_credential(&account.id) {
                Some(ClaudeCredential::ApiKey) => "API key",
                _ => "setup-token",
            },
            id: account.id,
            label: account.label,
        })
        .collect()
}

pub(crate) enum Event {
    Close,
    /// Open the Git credential dialog for this repository, with this profile selected.
    ManageCredentials {
        repo_url: String,
        selected: Option<String>,
    },
}

/// The outside world the dialog reaches: `sbx` (read-only), the project's git origin, the
/// native store and the Claude accounts the app knows.
#[derive(Clone)]
pub(crate) struct Env {
    pub sbx: Arc<dyn SbxRunner>,
    pub git: Arc<dyn GitRunner>,
    pub store: Database,
    pub home: Option<PathBuf>,
    pub origin: fn(&Path) -> Option<String>,
    pub accounts: Vec<AccountChoice>,
}

impl Env {
    pub(crate) fn system(store: Database, accounts: Vec<AccountChoice>) -> Self {
        Self {
            sbx: Arc::new(SystemSbx),
            git: Arc::new(SystemGit),
            store,
            home: monocode_core::accounts::home(),
            origin: crate::codespaces_dialog::model::git_origin,
            accounts,
        }
    }
}

pub(crate) struct DockerDialog {
    project: String,
    name: String,
    env: Env,
    access: Access,
    saved: Option<DockerProjectConfig>,
    form: Form,
    repo: Entity<Input>,
    devcontainer: Entity<Input>,
    base: Entity<Input>,
    credentials: Vec<GitCredentialSummary>,
    /// The first credential list has arrived. Until then a saved credential id cannot be checked.
    credentials_loaded: bool,
    credentials_error: Option<String>,
    error: Option<FieldError>,
    focus: FocusHandle,
    _events: Vec<Subscription>,
}

impl EventEmitter<Event> for DockerDialog {}

impl Focusable for DockerDialog {
    fn focus_handle(&self, _: &gpui::App) -> FocusHandle {
        self.focus.clone()
    }
}

impl DockerDialog {
    pub(crate) fn new(project: String, name: String, env: Env, cx: &mut Context<Self>) -> Self {
        let (saved, load_error) =
            match locked(&env.store).and_then(|store| docker_config::load(&store, &project)) {
                Ok(saved) => (saved, None),
                Err(message) => (None, Some(message)),
            };
        // The project's git origin is read later, off the UI thread.
        let form = Form::initial(saved.as_ref(), None);
        let text_field = |label: &'static str,
                          placeholder: &'static str,
                          value: &str,
                          cx: &mut Context<Self>| {
            let value = value.to_owned();
            let input = cx.new(|cx| {
                let mut field = Input::field(label, placeholder, content(0.35), cx);
                field.set_font_size(13.);
                field
            });
            input.update(cx, |input, cx| input.set_text(value, cx));
            input
        };
        let repo = text_field(
            "Repository URL",
            "https://github.com/owner/repo",
            &form.repo_url,
            cx,
        );
        let devcontainer = text_field(
            "Devcontainer configuration path",
            docker_config::DEFAULT_DEVCONTAINER,
            &form.devcontainer,
            cx,
        );
        let base = text_field(
            "Base branch",
            "Default branch of the repository",
            &form.base_ref,
            cx,
        );
        let mut events = Vec::new();
        events.push(cx.subscribe(&repo, |this, _, _: &input::Changed, cx| {
            this.repo_changed(cx)
        }));
        for input in [&devcontainer, &base] {
            events.push(cx.subscribe(input, |_, _, _: &input::Changed, cx| cx.notify()));
        }
        for input in [&repo, &devcontainer, &base] {
            events.push(cx.subscribe(input, |this, _, _: &input::Submitted, cx| this.save(cx)));
        }
        let mut this = Self {
            project,
            name,
            env,
            access: Access::Checking,
            saved,
            form,
            repo,
            devcontainer,
            base,
            credentials: Vec::new(),
            credentials_loaded: false,
            credentials_error: load_error,
            error: None,
            focus: cx.focus_handle(),
            _events: events,
        };
        this.reload_credentials(None, cx);
        this.check_access(cx);
        if this.saved.is_none() {
            this.read_origin(cx);
        }
        this
    }

    /// Runs the read-only prerequisite check off the UI thread.
    pub(crate) fn check_access(&mut self, cx: &mut Context<Self>) {
        self.access = Access::Checking;
        cx.notify();
        let (sbx, home) = (self.env.sbx.clone(), self.env.home.clone());
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { model::check_access(sbx.as_ref(), home.as_deref()) })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.access = Access::Done(result);
                cx.notify();
            });
        })
        .detach();
    }

    /// Reads the project's `origin` off the UI thread and prefills the empty repository field.
    fn read_origin(&mut self, cx: &mut Context<Self>) {
        let (origin, project) = (self.env.origin, self.project.clone());
        cx.spawn(async move |this, cx| {
            let found = cx
                .background_executor()
                .spawn(async move { origin(Path::new(&project)) })
                .await;
            let _ = this.update(cx, |this, cx| {
                // Never replace what the user typed, nor a saved config.
                if this.saved.is_some() || !this.repo.read(cx).content.trim().is_empty() {
                    return;
                }
                let url = model::prefill_repo(found.as_deref());
                if !url.is_empty() {
                    this.repo.update(cx, |input, cx| input.set_text(url, cx));
                }
            });
        })
        .detach();
    }

    /// A different host than the picked credential's drops the pick.
    fn repo_changed(&mut self, cx: &mut Context<Self>) {
        let url = self.repo.read(cx).content.to_string();
        if let Some(id) = &self.form.credential_id
            && let Ok((platform, _, _)) = monocode_core::git_credentials::normalize_origin(&url)
            && self
                .credentials
                .iter()
                .any(|c| &c.id == id && c.platform != platform)
        {
            self.form.credential_id = None;
        }
        cx.notify();
    }

    /// Reads the saved Git credentials again, off the UI thread, and selects `select` if given.
    pub(crate) fn reload_credentials(&mut self, select: Option<String>, cx: &mut Context<Self>) {
        let store = self.env.store.clone();
        cx.spawn(async move |this, cx| {
            let listed = cx
                .background_executor()
                .spawn(async move {
                    let credentials =
                        locked(&store).and_then(|store| GitCredentials::for_store(&store))?;
                    credentials.list()
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                match listed {
                    Ok(list) => {
                        this.credentials = list;
                        this.credentials_error = None;
                    }
                    Err(message) => {
                        this.credentials.clear();
                        this.credentials_error = Some(message);
                    }
                }
                this.credentials_loaded = true;
                if let Some(id) = select {
                    this.form.credential_id = Some(id);
                } else if let Some(id) = &this.form.credential_id
                    && !this.credentials.iter().any(|c| &c.id == id)
                {
                    this.form.credential_id = None;
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(crate) fn select_credential(&mut self, id: Option<String>, cx: &mut Context<Self>) {
        self.form.credential_id = id;
        self.error = None;
        cx.notify();
    }

    pub(crate) fn project_name(&self) -> &str {
        &self.name
    }

    pub(crate) fn repo_url(&self, cx: &gpui::App) -> String {
        self.repo.read(cx).content.trim().to_owned()
    }

    fn typed(&self, cx: &gpui::App) -> Form {
        Form {
            repo_url: self.repo.read(cx).content.to_string(),
            devcontainer: self.devcontainer.read(cx).content.to_string(),
            base_ref: self.base.read(cx).content.to_string(),
            ..self.form.clone()
        }
    }

    /// Validates through `docker_config::save` and stores the config. Field errors stay in
    /// the dialog; success closes it.
    pub(crate) fn save(&mut self, cx: &mut Context<Self>) {
        let form = self.typed(cx);
        if !self.credentials_loaded {
            self.error = Some(FieldError {
                field: Field::General,
                message: "Still reading the saved Git credentials. Try again in a moment.".into(),
            });
            cx.notify();
            return;
        }
        // The project config is small and lives in the native store like the app's other
        // settings, so it is read and written on the UI thread.
        let result = locked(&self.env.store)
            .map_err(|message| FieldError {
                field: Field::General,
                message,
            })
            .and_then(|store| {
                model::save(
                    &store,
                    &self.project,
                    &form,
                    &self.credentials,
                    &self.env.accounts,
                    self.saved.as_ref(),
                )
            });
        match result {
            Ok(config) => {
                self.saved = Some(config);
                self.error = None;
                cx.emit(Event::Close);
            }
            Err(error) => {
                self.form = form;
                self.error = Some(error);
                cx.notify();
            }
        }
    }

    #[cfg(test)]
    fn set_fields(&mut self, repo: &str, cx: &mut Context<Self>) {
        self.repo
            .update(cx, |input, cx| input.set_text(repo.to_owned(), cx));
    }

    #[cfg(test)]
    fn field_text(&self, field: Field, cx: &gpui::App) -> String {
        match field {
            Field::Repo => &self.repo,
            Field::Devcontainer => &self.devcontainer,
            _ => &self.base,
        }
        .read(cx)
        .content
        .to_string()
    }

    fn error_for(&self, field: Field) -> Option<AnyElement> {
        let error = self.error.as_ref().filter(|e| e.field == field)?;
        let id = format!("docker-error-{field:?}").to_lowercase();
        Some(alert(id, error.message.clone()).into_any_element())
    }

    fn access_section(&self, cx: &mut Context<Self>) -> Div {
        let checking = self.access == Access::Checking;
        let ready = self.access.is_ready();
        let mut column = div()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_3()
                    .child(
                        div()
                            .text_size(px(13.))
                            .font_weight(FontWeight::MEDIUM)
                            .child("Docker cloud access"),
                    )
                    .child(
                        div()
                            .id("docker-recheck")
                            .debug_selector(|| "docker-recheck".into())
                            .role(Role::Button)
                            .aria_label("Check again")
                            .text_size(px(12.))
                            .text_color(content(0.55))
                            .when(!checking, |d| {
                                d.cursor_pointer()
                                    .hover(|s| s.text_color(content(1.)))
                                    .on_click(cx.listener(|this, _, _, cx| this.check_access(cx)))
                            })
                            .when(checking, |d| d.opacity(0.4))
                            .child("Check again"),
                    ),
            )
            .child(
                div()
                    .id("docker-access-line")
                    .debug_selector(|| "docker-access-line".into())
                    .text_size(px(12.))
                    .line_height(px(18.))
                    .text_color(if ready {
                        rgb(0x34d399).into()
                    } else {
                        content(0.7)
                    })
                    .child(self.access.line()),
            );
        if let Some(fix) = self.access.fix() {
            column = column
                .child(muted("Run this in a terminal, then Check again:"))
                .child(code_box(fix).debug_selector(|| "docker-fix-command".into()));
        }
        column
    }

    fn credential_section(&self, cx: &mut Context<Self>) -> Div {
        let selected = self.form.credential_id.clone();
        let repo = self.repo.read(cx).content.to_string();
        let matching = model::matching_credentials(&self.credentials, &repo);
        let mut column = labelled("Git credential").gap(px(2.));
        for credential in matching {
            let id = credential.id.clone();
            column = column.child(
                choice(
                    format!("docker-credential-{}", credential.id),
                    format!("{} · {}", credential.label, credential.platform.host()),
                    selected.as_deref() == Some(credential.id.as_str()),
                )
                .on_click(
                    cx.listener(move |this, _, _, cx| this.select_credential(Some(id.clone()), cx)),
                ),
            );
        }
        column = column
            .child(
                choice(
                    "docker-credential-none",
                    "None · public repository",
                    selected.is_none(),
                )
                .on_click(cx.listener(|this, _, _, cx| this.select_credential(None, cx))),
            )
            .child(
                choice(
                    "docker-credential-manage",
                    "Add or manage credentials…",
                    false,
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    let event = Event::ManageCredentials {
                        repo_url: this.repo_url(cx),
                        selected: this.form.credential_id.clone(),
                    };
                    cx.emit(event);
                })),
            )
            .child(muted(model::CREDENTIAL_NOTE));
        if let Some(message) = &self.credentials_error {
            column = column.child(alert("docker-credentials-error", message.clone()));
        }
        column.children(self.error_for(Field::Credential))
    }

    fn choices(
        &self,
        label: &'static str,
        prefix: &'static str,
        options: Vec<(String, bool)>,
        pick: fn(&mut Self, usize, &mut Context<Self>),
        cx: &mut Context<Self>,
    ) -> Div {
        let mut column = labelled(label).gap(px(2.));
        for (ix, (text, selected)) in options.into_iter().enumerate() {
            column = column.child(
                choice(format!("docker-{prefix}-{ix}"), text, selected)
                    .on_click(cx.listener(move |this, _, _, cx| pick(this, ix, cx))),
            );
        }
        column
    }

    fn account_section(&self, cx: &mut Context<Self>) -> Div {
        let mut column = labelled("Claude account").gap(px(2.));
        if self.env.accounts.is_empty() {
            column = column.child(muted(model::NO_ACCOUNT_LINE));
        } else {
            column = column.child(
                choice(
                    "docker-account-none",
                    "Choose when creating",
                    self.form.account.is_none(),
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.form.account = None;
                    cx.notify();
                })),
            );
            for account in &self.env.accounts {
                let id = account.id.clone();
                column = column.child(
                    choice(
                        format!("docker-account-{}", account.id),
                        format!("{} · {}", account.label, account.kind),
                        self.form.account.as_deref() == Some(account.id.as_str()),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.form.account = Some(id.clone());
                        cx.notify();
                    })),
                );
            }
        }
        column.child(muted(model::ACCOUNT_NOTE))
    }
}

impl Render for DockerDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let height = window.viewport_size().height.as_f32() - 34.;
        let compute = self.form.compute;
        let runtime = self.form.runtime;
        let compute_options = model::COMPUTE
            .iter()
            .enumerate()
            .map(|(ix, c)| (c.label.to_owned(), ix == compute))
            .collect();
        let runtime_options = model::RUNTIME
            .iter()
            .enumerate()
            .map(|(ix, r)| (r.label.to_owned(), ix == runtime))
            .collect();
        let body = div()
            .id("docker-dialog-body")
            .min_h_0()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap_4()
            .p_4()
            .child(self.access_section(cx))
            .child(
                labelled("Repository URL")
                    .child(field_box("docker-field-repo", &self.repo))
                    .child(muted(model::REPO_HINT))
                    .children(self.error_for(Field::Repo)),
            )
            .child(self.credential_section(cx))
            .child(
                labelled("Devcontainer configuration (optional)")
                    .child(field_box("docker-field-devcontainer", &self.devcontainer))
                    .child(muted(model::DEVCONTAINER_HINT))
                    .children(self.error_for(Field::Devcontainer)),
            )
            .child(
                labelled("Base branch")
                    .child(field_box("docker-field-base", &self.base))
                    .children(self.error_for(Field::Base)),
            )
            .child(self.choices(
                "Compute",
                "compute",
                compute_options,
                |this, ix, cx| {
                    this.form.compute = ix;
                    cx.notify();
                },
                cx,
            ))
            .child(self.choices(
                "Runtime limit",
                "runtime",
                runtime_options,
                |this, ix, cx| {
                    this.form.runtime = ix;
                    cx.notify();
                },
                cx,
            ))
            .child(self.account_section(cx))
            .children(self.error_for(Field::General));
        let footer = div()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .gap_2()
            .px_4()
            .pb_4()
            .pt_3()
            .border_t_1()
            .border_color(content(0.08))
            .child(
                div()
                    .id("docker-save-note")
                    .debug_selector(|| "docker-save-note".into())
                    .child(muted(model::SAVE_NOTE)),
            )
            .child(
                modal::actions()
                    .mt(px(0.))
                    .child(
                        modal::ghost_button("docker-cancel", "Cancel")
                            .debug_selector(|| "docker-cancel".into())
                            .cursor_pointer()
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(Event::Close))),
                    )
                    .child(
                        modal::primary_button("docker-save", "Save")
                            .debug_selector(|| "docker-save".into())
                            .cursor_pointer()
                            .on_click(cx.listener(|this, _, _, cx| this.save(cx))),
                    ),
            );
        div()
            .id("docker-dialog")
            .debug_selector(|| "docker-dialog".into())
            .role(Role::Dialog)
            .aria_label("Docker Sandbox")
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
                    .child(heading(
                        "Docker Sandbox",
                        format!("Cloud setup for {}", self.name),
                    ))
                    .child(
                        icon_button("docker-close", "Close Docker Sandbox setup", "close")
                            .size(px(28.))
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(Event::Close))),
                    ),
            )
            .child(body)
            .child(footer)
    }
}
