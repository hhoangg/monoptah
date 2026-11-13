//! Settings > Cloud > Docker accounts: the accounts Docker Sandbox projects sign in with.
//! One row per account (Docker ID, status, token expiry, projects using it) with Check again,
//! Replace token and Remove, plus an inline form to add one.
//!
//! Every `docker_accounts` call runs on the background executor: replace, remove and check
//! may wait up to about 15 seconds for an exchange in flight. The personal access token
//! lives only in the masked input until Save hands it to the core store, and the input is
//! emptied after a success, on Cancel and when the section is left. It is never put in an
//! error, a log line or the rendered page.
use crate::{
    cloud_dialog_ui::{field_box, labelled, muted},
    input::{self, Input},
    runtime::{Database, locked},
    settings::{
        SettingsView,
        cloud_env::CloudServices,
        pages::cloud_backends::chip,
        widgets::{note, primary_button, secondary_button},
    },
    theme::*,
};
use gpui::{
    AnyElement, Context, Entity, Focusable, FontWeight, Role, SharedString, Subscription, Task,
    Window, div, prelude::*, px,
};
use monocode_core::{
    cloud_sessions::project_cloud::projects_using_docker_account,
    docker_accounts::{DockerAccountStatus, DockerAccountSummary, DockerAccounts, Secret},
    sandboxes::account::sandboxes_pinned_to,
};
use std::path::Path;

pub(crate) const SAVED: &str = "Checked: token works and has Cloud Sandboxes access";
/// The page where a personal access token is created; the hint links to it.
pub(crate) const TOKEN_URL: &str = "https://app.docker.com/settings/personal-access-tokens";
const TOKEN_LINK_TEXT: &str = "app.docker.com/settings/personal-access-tokens";
pub(crate) const HINT: &str = "Tick Allow Cloud Sandboxes access; Repository permissions can stay \"Repo Public Read-only\". Prefer an expiration date. Not your Docker password.";
const FOOTER: &str = "Tokens are stored encrypted in Monoptah's database, never in the Keychain. Access tokens derived from them live only in memory (15 minutes).";
const NO_STORE: &str = "Docker accounts need the Monoptah database, which is not available here.";

pub(in crate::settings) enum AccountList {
    Idle,
    Loading,
    Ready(Vec<AccountRow>),
    Failed(String),
}

pub(in crate::settings) struct AccountRow {
    pub(in crate::settings) summary: DockerAccountSummary,
    /// Folder names of the projects whose sandbox uses the account.
    pub(in crate::settings) used_by: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::settings) enum FormKind {
    Add,
    Replace { id: String, docker_id: String },
}

pub(in crate::settings) struct Form {
    pub(in crate::settings) kind: FormKind,
    /// The form's own Save is running (the label reads "Checking...").
    pub(in crate::settings) checking: bool,
    /// Why the last Save failed. A success closes the form and shows a notice instead.
    pub(in crate::settings) error: Option<String>,
}

pub(in crate::settings) enum RemoveStage {
    Confirm,
    /// Projects still use the account.
    Refused(Vec<String>),
    Removing,
    Failed(String),
}

pub(in crate::settings) struct Removal {
    pub(in crate::settings) id: String,
    pub(in crate::settings) docker_id: String,
    pub(in crate::settings) stage: RemoveStage,
}

pub(in crate::settings) struct Notice {
    pub(in crate::settings) ok: bool,
    pub(in crate::settings) text: String,
}

pub(in crate::settings) struct Inputs {
    pub(in crate::settings) docker_id: Entity<Input>,
    pub(in crate::settings) pat: Entity<Input>,
    pub(in crate::settings) _subscriptions: Vec<Subscription>,
}

pub(in crate::settings) struct DockerState {
    pub(in crate::settings) list: AccountList,
    /// Counts loads so a slow answer to an older one is dropped.
    pub(in crate::settings) generation: u64,
    pub(in crate::settings) task: Option<Task<()>>,
    pub(in crate::settings) form: Option<Form>,
    pub(in crate::settings) inputs: Option<Inputs>,
    pub(in crate::settings) removal: Option<Removal>,
    pub(in crate::settings) notice: Option<Notice>,
    /// Counts form openings and leaves of the page. An action answers to the epoch it
    /// started in; a result from an older one is stale and changes nothing but the list.
    pub(in crate::settings) epoch: u64,
    /// A save, a check or a removal is running. The one flag for all three: every entry
    /// point is blocked while it is set, so `action` is never overwritten while running.
    pub(in crate::settings) busy: bool,
    pub(in crate::settings) action: Option<Task<()>>,
}

impl Default for DockerState {
    fn default() -> Self {
        Self {
            list: AccountList::Idle,
            generation: 0,
            task: None,
            form: None,
            inputs: None,
            removal: None,
            notice: None,
            epoch: 0,
            busy: false,
            action: None,
        }
    }
}

/// The accounts of the store, found under its lock; the lock is released before any call
/// that may wait on the network.
fn accounts_of(store: &Database) -> Result<DockerAccounts, String> {
    DockerAccounts::for_store(&*locked(store)?)
}

fn folder_name(cwd: &str) -> String {
    Path::new(cwd).file_name().map_or_else(
        || cwd.to_owned(),
        |name| name.to_string_lossy().into_owned(),
    )
}

/// What uses the account: the projects (folder names) whose sandbox config names it, then the
/// live sandboxes created in it ("sandbox NAME"; a sandbox stays in its account whatever its
/// project says now).
fn users_of(store: &Database, id: &str) -> Result<Vec<String>, String> {
    let store = locked(store)?;
    let mut users: Vec<String> = projects_using_docker_account(&store, id)?
        .iter()
        .map(|cwd| folder_name(cwd))
        .collect();
    users.extend(
        sandboxes_pinned_to(&store, id)?
            .into_iter()
            .map(|name| format!("{SANDBOX_USER_PREFIX}{name}")),
    );
    Ok(users)
}

/// How a sandbox is listed among the users of an account.
const SANDBOX_USER_PREFIX: &str = "sandbox ";

/// Why an account in use cannot be removed, and what to do first.
fn refusal_text(docker_id: &str, users: &[String]) -> String {
    let has_sandboxes = users.iter().any(|u| u.starts_with(SANDBOX_USER_PREFIX));
    let has_projects = users.iter().any(|u| !u.starts_with(SANDBOX_USER_PREFIX));
    let mut text = format!("{docker_id} is used by {}.", users.join(", "));
    if has_projects {
        text.push_str(
            " Pick another Docker account in the Cloud settings of each of these projects first.",
        );
    }
    if has_sandboxes {
        text.push_str(
            " A sandbox stays in the account that created it: delete the listed sandboxes first.",
        );
    }
    text
}

fn load_rows(store: &Database) -> Result<Vec<AccountRow>, String> {
    let accounts = accounts_of(store)?;
    accounts
        .list()?
        .into_iter()
        .map(|summary| {
            let used_by = users_of(store, &summary.id)?;
            Ok(AccountRow { summary, used_by })
        })
        .collect()
}

/// `YYYY-MM-DD` of Unix milliseconds, or "Not known".
pub(crate) fn expiry_label(expires_at: Option<u64>) -> String {
    let Some(ms) = expires_at else {
        return "Not known".into();
    };
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let days = (ms / 86_400_000) as i64 + 719_468;
    let era = days.div_euclid(146_097);
    let day_of_era = days.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * shifted_month + 2) / 5 + 1;
    let month = if shifted_month < 10 {
        shifted_month + 3
    } else {
        shifted_month - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}")
}

impl SettingsView {
    fn docker_store(&self) -> Option<Database> {
        self.ctx.store.as_ref().map(|handle| handle.0.clone())
    }

    /// Reads the accounts on a worker thread.
    pub(in crate::settings) fn refresh_docker(&mut self, cx: &mut Context<Self>) {
        self.cloud.docker.generation += 1;
        let generation = self.cloud.docker.generation;
        let Some(store) = self.docker_store() else {
            self.cloud.docker.list = AccountList::Idle;
            self.cloud.docker.task = None;
            cx.notify();
            return;
        };
        if !matches!(self.cloud.docker.list, AccountList::Ready(_)) {
            self.cloud.docker.list = AccountList::Loading;
        }
        let work = cx
            .background_executor()
            .spawn(async move { load_rows(&store) });
        self.cloud.docker.task = Some(cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |this, cx| {
                if this.cloud.docker.generation != generation {
                    return;
                }
                this.cloud.docker.list = match result {
                    Ok(rows) => AccountList::Ready(rows),
                    Err(error) => AccountList::Failed(error),
                };
                cx.notify();
            });
        }));
        cx.notify();
    }

    /// The form's inputs, made on first use.
    fn docker_inputs(&mut self, cx: &mut Context<Self>) -> (Entity<Input>, Entity<Input>) {
        if self.cloud.docker.inputs.is_none() {
            let docker_id = cx.new(|cx| {
                let mut input = Input::field("Docker ID", "your-docker-id", content(0.35), cx);
                input.set_font_size(13.);
                input
            });
            let pat = cx.new(|cx| {
                let mut input =
                    Input::field("Personal access token", "dckr_pat_...", content(0.35), cx);
                input.set_font_size(13.);
                input.set_secret(true);
                input
            });
            let subscriptions = vec![
                cx.subscribe(&docker_id, |this, _, _: &input::Submitted, cx| {
                    this.docker_save(cx)
                }),
                cx.subscribe(&pat, |this, _, _: &input::Submitted, cx| {
                    this.docker_save(cx)
                }),
            ];
            self.cloud.docker.inputs = Some(Inputs {
                docker_id,
                pat,
                _subscriptions: subscriptions,
            });
        }
        let inputs = self.cloud.docker.inputs.as_ref().expect("made above");
        (inputs.docker_id.clone(), inputs.pat.clone())
    }

    fn clear_docker_inputs(&mut self, cx: &mut Context<Self>) {
        if let Some(inputs) = &self.cloud.docker.inputs {
            for input in [&inputs.docker_id, &inputs.pat] {
                input.update(cx, |input, cx| input.set_text(String::new(), cx));
            }
        }
    }

    /// Leaving the page drops what was typed and any open dialog. A save, check or removal
    /// still running is not cancelled (the store call cannot be): its result is stale
    /// because the epoch moved on, so it only refreshes the list, and `busy` stays set until
    /// it ends so nothing overwrites it.
    pub(in crate::settings) fn leave_cloud(&mut self, cx: &mut Context<Self>) {
        self.close_fix(cx);
        self.cloud.docker.epoch += 1;
        self.cloud.docker.form = None;
        self.cloud.docker.removal = None;
        self.cloud.docker.notice = None;
        self.clear_docker_inputs(cx);
    }

    /// Claims the single action slot, or `None` while another action runs. The returned
    /// epoch goes back to [`Self::end_docker_action`].
    fn begin_docker_action(&mut self) -> Option<u64> {
        if self.cloud.docker.busy {
            return None;
        }
        self.cloud.docker.busy = true;
        Some(self.cloud.docker.epoch)
    }

    /// Frees the action slot. `true` when the result still belongs to what is on screen; a
    /// stale result (the page was left meanwhile) only gets the list refreshed.
    fn end_docker_action(&mut self, epoch: u64, cx: &mut Context<Self>) -> bool {
        self.cloud.docker.busy = false;
        let current = self.cloud.docker.epoch == epoch;
        if !current {
            self.refresh_docker(cx);
        }
        current
    }

    pub(in crate::settings) fn open_docker_form(
        &mut self,
        kind: FormKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.cloud.docker.busy {
            return;
        }
        self.cloud.docker.epoch += 1;
        let (docker_id, pat) = self.docker_inputs(cx);
        self.clear_docker_inputs(cx);
        let target = if kind == FormKind::Add {
            &docker_id
        } else {
            &pat
        };
        window.focus(&target.focus_handle(cx), cx);
        self.cloud.docker.form = Some(Form {
            kind,
            checking: false,
            error: None,
        });
        self.cloud.docker.removal = None;
        self.cloud.docker.notice = None;
        cx.notify();
    }

    fn close_docker_form(&mut self, cx: &mut Context<Self>) {
        if self
            .cloud
            .docker
            .form
            .as_ref()
            .is_some_and(|form| form.checking)
        {
            return;
        }
        self.cloud.docker.form = None;
        self.clear_docker_inputs(cx);
        cx.notify();
    }

    /// Add or Replace: checks the token against Docker, then saves it.
    pub(in crate::settings) fn docker_save(&mut self, cx: &mut Context<Self>) {
        if self.cloud.docker.busy {
            return;
        }
        let Some(kind) = self
            .cloud
            .docker
            .form
            .as_ref()
            .map(|form| form.kind.clone())
        else {
            return;
        };
        let Some(store) = self.docker_store() else {
            if let Some(form) = &mut self.cloud.docker.form {
                form.error = Some(NO_STORE.into());
            }
            cx.notify();
            return;
        };
        let Some(inputs) = &self.cloud.docker.inputs else {
            return;
        };
        let docker_id = inputs.docker_id.read(cx).content.trim().to_owned();
        let pat = Secret::new(inputs.pat.read(cx).content.trim().to_owned());
        let Some(epoch) = self.begin_docker_action() else {
            return;
        };
        if let Some(form) = &mut self.cloud.docker.form {
            form.checking = true;
            form.error = None;
        }
        let hub = CloudServices::get(cx).hub;
        let work = cx.background_executor().spawn(async move {
            let accounts = accounts_of(&store)?;
            match kind {
                FormKind::Add => accounts.add(&docker_id, pat.as_str(), &*hub),
                FormKind::Replace { id, .. } => accounts.replace_token(&id, pat.as_str(), &*hub),
            }
        });
        self.cloud.docker.action = Some(cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |this, cx| this.docker_saved(epoch, result, cx));
        }));
        cx.notify();
    }

    /// A success closes the form and reports in the notice, so the Save button cannot be
    /// pressed again on the emptied inputs and replace the success line with a validation
    /// error. A failure keeps the form and what was typed.
    pub(in crate::settings) fn docker_saved(
        &mut self,
        epoch: u64,
        result: Result<DockerAccountSummary, String>,
        cx: &mut Context<Self>,
    ) {
        if !self.end_docker_action(epoch, cx) {
            // Another form may be open now: its inputs are not ours to clear.
            return;
        }
        match result {
            Ok(summary) => {
                self.cloud.docker.form = None;
                self.clear_docker_inputs(cx);
                self.cloud.docker.notice = Some(Notice {
                    ok: summary.status == DockerAccountStatus::Valid,
                    text: if summary.status == DockerAccountStatus::Valid {
                        SAVED.into()
                    } else {
                        format!("{}: {}", summary.docker_id, summary.status.label())
                    },
                });
            }
            Err(error) => {
                if let Some(form) = &mut self.cloud.docker.form {
                    form.checking = false;
                    form.error = Some(error);
                }
            }
        }
        self.refresh_docker(cx);
    }

    /// Re-exchanges the stored token and updates the status.
    pub(in crate::settings) fn docker_check(&mut self, id: String, cx: &mut Context<Self>) {
        let Some(store) = self.docker_store() else {
            return;
        };
        let Some(epoch) = self.begin_docker_action() else {
            return;
        };
        self.cloud.docker.notice = None;
        let hub = CloudServices::get(cx).hub;
        let work = cx
            .background_executor()
            .spawn(async move { accounts_of(&store)?.check(&id, &*hub) });
        self.cloud.docker.action = Some(cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |this, cx| this.docker_checked(epoch, result, cx));
        }));
        cx.notify();
    }

    pub(in crate::settings) fn docker_checked(
        &mut self,
        epoch: u64,
        result: Result<DockerAccountSummary, String>,
        cx: &mut Context<Self>,
    ) {
        if !self.end_docker_action(epoch, cx) {
            return;
        }
        self.cloud.docker.notice = Some(match result {
            Ok(summary) => Notice {
                ok: summary.status == DockerAccountStatus::Valid,
                text: format!("{}: {}", summary.docker_id, summary.status.label()),
            },
            Err(error) => Notice {
                ok: false,
                text: error,
            },
        });
        self.refresh_docker(cx);
    }

    /// Remove: refused with the project names while a project uses the account, otherwise
    /// asks first.
    pub(in crate::settings) fn docker_remove_start(
        &mut self,
        id: String,
        docker_id: String,
        cx: &mut Context<Self>,
    ) {
        if self.cloud.docker.busy {
            return;
        }
        let AccountList::Ready(rows) = &self.cloud.docker.list else {
            return;
        };
        let users = rows
            .iter()
            .find(|row| row.summary.id == id)
            .map(|row| row.used_by.clone())
            .unwrap_or_default();
        self.cloud.docker.epoch += 1;
        self.cloud.docker.form = None;
        self.cloud.docker.notice = None;
        self.clear_docker_inputs(cx);
        self.cloud.docker.removal = Some(Removal {
            id,
            docker_id,
            stage: if users.is_empty() {
                RemoveStage::Confirm
            } else {
                RemoveStage::Refused(users)
            },
        });
        cx.notify();
    }

    /// Confirmed: asks the store again (a project may have chosen the account since the page
    /// was read), then removes.
    pub(in crate::settings) fn docker_remove_confirm(&mut self, cx: &mut Context<Self>) {
        let Some(store) = self.docker_store() else {
            return;
        };
        let Some(removal) = &self.cloud.docker.removal else {
            return;
        };
        if !matches!(removal.stage, RemoveStage::Confirm) {
            return;
        }
        let id = removal.id.clone();
        let Some(epoch) = self.begin_docker_action() else {
            return;
        };
        if let Some(removal) = &mut self.cloud.docker.removal {
            removal.stage = RemoveStage::Removing;
        }
        let asked = id.clone();
        let work = cx.background_executor().spawn(async move {
            let users = users_of(&store, &asked)?;
            if !users.is_empty() {
                return Ok(Err(users));
            }
            accounts_of(&store)?.remove(&asked)?;
            Ok::<_, String>(Ok(()))
        });
        self.cloud.docker.action = Some(cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |this, cx| this.docker_removed(epoch, id, result, cx));
        }));
        cx.notify();
    }

    /// Applies the answer to the removal of `id`, and only to that one.
    pub(in crate::settings) fn docker_removed(
        &mut self,
        epoch: u64,
        id: String,
        result: Result<Result<(), Vec<String>>, String>,
        cx: &mut Context<Self>,
    ) {
        if !self.end_docker_action(epoch, cx) {
            return;
        }
        if self
            .cloud
            .docker
            .removal
            .as_ref()
            .is_some_and(|removal| removal.id == id)
        {
            match result {
                Ok(Ok(())) => self.cloud.docker.removal = None,
                Ok(Err(users)) => {
                    if let Some(removal) = &mut self.cloud.docker.removal {
                        removal.stage = RemoveStage::Refused(users);
                    }
                }
                Err(error) => {
                    if let Some(removal) = &mut self.cloud.docker.removal {
                        removal.stage = RemoveStage::Failed(error);
                    }
                }
            }
        }
        self.refresh_docker(cx);
    }

    pub(in crate::settings) fn docker_group(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let mut rows: Vec<AnyElement> = Vec::new();
        match &self.cloud.docker.list {
            AccountList::Idle => rows.push(note(NO_STORE)),
            AccountList::Loading => rows.push(note("Reading Docker accounts...")),
            AccountList::Failed(error) => rows.push(
                div()
                    .id("docker-accounts-error")
                    .role(Role::Alert)
                    .px_4()
                    .py(px(14.))
                    .text_size(px(12.))
                    .text_color(danger_color())
                    .child(error.clone())
                    .into_any_element(),
            ),
            AccountList::Ready(list) if list.is_empty() => {
                rows.push(note("No Docker accounts yet."))
            }
            AccountList::Ready(_) => {}
        }
        if let AccountList::Ready(list) = &self.cloud.docker.list
            && !list.is_empty()
        {
            rows.push(table_header());
            let busy = self.cloud.docker.busy;
            let items: Vec<_> = list
                .iter()
                .map(|row| {
                    (
                        row.summary.clone(),
                        if row.used_by.is_empty() {
                            "Not used".to_owned()
                        } else {
                            row.used_by.join(", ")
                        },
                    )
                })
                .collect();
            for (summary, used_by) in items {
                rows.push(self.account_row(summary, used_by, busy, cx));
            }
        }
        if let Some(notice) = &self.cloud.docker.notice {
            rows.push(
                div()
                    .px_4()
                    .py(px(10.))
                    .border_t_1()
                    .border_color(content(0.05))
                    .child(chip("docker-notice", notice.ok, notice.text.clone()))
                    .into_any_element(),
            );
        }
        if let Some(removal) = self.removal_panel(cx) {
            rows.push(removal);
        }
        rows.push(self.docker_footer(cx));
        if let Some(form) = self.docker_form(cx) {
            rows.push(form);
        }
        self.group(
            Some("cloud-docker-accounts"),
            false,
            "Docker accounts",
            Some("Used for Docker Sandbox. Each project picks one in its Cloud settings, so several accounts work side by side on this Mac.".into()),
            None,
            rows,
        )
    }

    fn account_row(
        &mut self,
        summary: DockerAccountSummary,
        used_by: String,
        busy: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let docker_id = summary.docker_id.clone();
        let selector = format!("docker-account-{docker_id}");
        let replace_id = summary.id.clone();
        let replace_docker = docker_id.clone();
        let remove_id = summary.id.clone();
        let remove_docker = docker_id.clone();
        let check_id = summary.id.clone();
        let status = summary.status;
        let status_chip = match status {
            DockerAccountStatus::Unchecked => div()
                .text_size(px(12.))
                .text_color(content(0.55))
                .child(status.label())
                .into_any_element(),
            _ => chip(
                "docker-status",
                status == DockerAccountStatus::Valid,
                status.label().into(),
            ),
        };
        let status_selector = format!("docker-status-{docker_id}");
        div()
            .id(SharedString::from(selector.clone()))
            .debug_selector(move || selector.clone())
            .role(Role::ListItem)
            .aria_label(docker_id.clone())
            .px_4()
            .py(px(10.))
            .border_t_1()
            .border_color(content(0.05))
            .flex()
            .items_center()
            .gap_3()
            .child(
                div()
                    .w(px(140.))
                    .min_w_0()
                    .truncate()
                    .text_size(px(13.))
                    .font_weight(FontWeight::MEDIUM)
                    .child(docker_id.clone()),
            )
            .child(
                div()
                    .id(SharedString::from(status_selector.clone()))
                    .debug_selector(move || status_selector.clone())
                    .w(px(210.))
                    .child(status_chip),
            )
            .child(
                div()
                    .w(px(100.))
                    .text_size(px(12.))
                    .text_color(content(0.55))
                    .child(expiry_label(summary.expires_at)),
            )
            .child(
                div()
                    .min_w_0()
                    .flex_1()
                    .text_size(px(12.))
                    .text_color(content(0.55))
                    .child(used_by),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .justify_end()
                    .gap_2()
                    .child(
                        secondary_button(
                            SharedString::from(format!("docker-check-{docker_id}")),
                            "Check again",
                            false,
                            busy,
                        )
                        .debug_selector({
                            let docker_id = docker_id.clone();
                            move || format!("docker-check-{docker_id}")
                        })
                        .child("Check again")
                        .when(!busy, |d| {
                            d.on_click(cx.listener(move |this, _, _, cx| {
                                this.docker_check(check_id.clone(), cx)
                            }))
                        }),
                    )
                    .child(
                        secondary_button(
                            SharedString::from(format!("docker-replace-{docker_id}")),
                            "Replace token",
                            false,
                            busy,
                        )
                        .debug_selector({
                            let docker_id = docker_id.clone();
                            move || format!("docker-replace-{docker_id}")
                        })
                        .child("Replace token")
                        .when(!busy, |d| {
                            d.on_click(cx.listener(move |this, _, window, cx| {
                                this.open_docker_form(
                                    FormKind::Replace {
                                        id: replace_id.clone(),
                                        docker_id: replace_docker.clone(),
                                    },
                                    window,
                                    cx,
                                )
                            }))
                        }),
                    )
                    .child(
                        secondary_button(
                            SharedString::from(format!("docker-remove-{docker_id}")),
                            "Remove",
                            true,
                            busy,
                        )
                        .debug_selector({
                            let docker_id = docker_id.clone();
                            move || format!("docker-remove-{docker_id}")
                        })
                        .child("Remove")
                        .when(!busy, |d| {
                            d.on_click(cx.listener(move |this, _, _, cx| {
                                this.docker_remove_start(
                                    remove_id.clone(),
                                    remove_docker.clone(),
                                    cx,
                                )
                            }))
                        }),
                    ),
            )
            .into_any_element()
    }

    fn removal_panel(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let removal = self.cloud.docker.removal.as_ref()?;
        let (message, can_confirm, working) = match &removal.stage {
            RemoveStage::Confirm => (
                format!(
                    "Remove {}? Its stored token is deleted. Projects do not use it.",
                    removal.docker_id
                ),
                true,
                false,
            ),
            RemoveStage::Refused(users) => (refusal_text(&removal.docker_id, users), false, false),
            RemoveStage::Removing => (format!("Removing {}...", removal.docker_id), false, true),
            RemoveStage::Failed(error) => (error.clone(), false, false),
        };
        let refused = !can_confirm && !working;
        let busy = self.cloud.docker.busy;
        Some(
            div()
                .id("docker-remove-panel")
                .debug_selector(|| "docker-remove-panel".into())
                .role(Role::Group)
                .aria_label(format!("Remove {}", removal.docker_id))
                .border_t_1()
                .border_color(content(0.1))
                .bg(content(0.03))
                .p_4()
                .flex()
                .flex_col()
                .gap_3()
                .child(
                    div()
                        .id("docker-remove-message")
                        .debug_selector(|| "docker-remove-message".into())
                        .role(if refused { Role::Alert } else { Role::Status })
                        .text_size(px(13.))
                        .line_height(px(19.))
                        .text_color(if refused {
                            danger_color()
                        } else {
                            content(0.9)
                        })
                        .child(message),
                )
                .child(
                    div()
                        .flex()
                        .gap_2()
                        .when(can_confirm, |d| {
                            d.child(
                                secondary_button(
                                    "docker-remove-confirm",
                                    "Remove account",
                                    true,
                                    busy,
                                )
                                .debug_selector(|| "docker-remove-confirm".into())
                                .child("Remove account")
                                .when(!busy, |d| {
                                    d.on_click(
                                        cx.listener(|this, _, _, cx| {
                                            this.docker_remove_confirm(cx)
                                        }),
                                    )
                                }),
                            )
                        })
                        .child(
                            secondary_button(
                                "docker-remove-cancel",
                                if can_confirm { "Cancel" } else { "Close" },
                                false,
                                working,
                            )
                            .debug_selector(|| "docker-remove-cancel".into())
                            .child(if can_confirm { "Cancel" } else { "Close" })
                            .when(!working, |d| {
                                d.on_click(cx.listener(|this, _, _, cx| {
                                    this.cloud.docker.removal = None;
                                    cx.notify();
                                }))
                            }),
                        ),
                )
                .into_any_element(),
        )
    }

    fn docker_footer(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let busy = self.cloud.docker.busy;
        let adding = self
            .cloud
            .docker
            .form
            .as_ref()
            .is_some_and(|form| form.kind == FormKind::Add);
        div()
            .px_4()
            .py(px(12.))
            .border_t_1()
            .border_color(content(0.05))
            .flex()
            .items_center()
            .gap_4()
            .child(
                div()
                    .min_w_0()
                    .flex_1()
                    .text_size(px(12.))
                    .line_height(px(18.))
                    .text_color(content(0.45))
                    .child(FOOTER),
            )
            .child(
                secondary_button(
                    "docker-add-button",
                    "Add Docker account",
                    false,
                    self.docker_store().is_none() || busy,
                )
                .debug_selector(|| "docker-add-button".into())
                .child("Add Docker account")
                .when(self.docker_store().is_some() && !adding && !busy, |d| {
                    d.on_click(cx.listener(|this, _, window, cx| {
                        this.open_docker_form(FormKind::Add, window, cx)
                    }))
                }),
            )
            .into_any_element()
    }

    fn docker_form(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let form = self.cloud.docker.form.as_ref()?;
        let kind = form.kind.clone();
        let checking = form.checking;
        let busy = self.cloud.docker.busy;
        let error = form.error.clone();
        let (docker_id_input, pat_input) = self.docker_inputs(cx);
        let save_label = if checking { "Checking..." } else { "Save" };
        let id_field = match &kind {
            FormKind::Add => labelled("Docker ID")
                .child(field_box("docker-form-id", &docker_id_input))
                .into_any_element(),
            FormKind::Replace { docker_id, .. } => labelled("Docker ID")
                .child(
                    div()
                        .id("docker-form-fixed-id")
                        .debug_selector(|| "docker-form-fixed-id".into())
                        .text_size(px(13.))
                        .text_color(content(0.8))
                        .child(docker_id.clone()),
                )
                .into_any_element(),
        };
        let title = match &kind {
            FormKind::Add => "Add Docker account".to_owned(),
            FormKind::Replace { docker_id, .. } => format!("Replace token for {docker_id}"),
        };
        Some(
            div()
                .id("docker-form")
                .debug_selector(|| "docker-form".into())
                .role(Role::Group)
                .aria_label(title.clone())
                .border_t_1()
                .border_color(content(0.1))
                .bg(content(0.03))
                .p_4()
                .flex()
                .flex_col()
                .gap_3()
                .child(
                    div()
                        .text_size(px(13.))
                        .font_weight(FontWeight::MEDIUM)
                        .child(title),
                )
                .child(id_field)
                .child(
                    labelled("Personal access token")
                        .child(field_box("docker-form-pat", &pat_input))
                        .child(
                            div()
                                .id("docker-form-hint")
                                .debug_selector(|| "docker-form-hint".into())
                                .flex()
                                .flex_col()
                                .child(
                                    muted("Create one at").flex().flex_wrap().gap_x_1().child(
                                        div()
                                            .id("docker-token-link")
                                            .debug_selector(|| "docker-token-link".into())
                                            .role(Role::Link)
                                            .aria_label("Open the Docker token page")
                                            .text_color(content(0.7))
                                            .underline()
                                            .cursor_pointer()
                                            .hover(|s| s.text_color(content(1.)))
                                            .child(TOKEN_LINK_TEXT)
                                            .on_click(|_, _, cx| cx.open_url(TOKEN_URL)),
                                    ),
                                )
                                .child(muted(HINT)),
                        ),
                )
                .children(error.map(|error| {
                    div()
                        .id("docker-form-error")
                        .debug_selector(|| "docker-form-error".into())
                        .role(Role::Alert)
                        .text_size(px(12.))
                        .line_height(px(18.))
                        .text_color(danger_color())
                        .child(error)
                        .into_any_element()
                }))
                .child(
                    div()
                        .flex()
                        .justify_end()
                        .gap_2()
                        .child(
                            secondary_button("docker-form-cancel", "Cancel", false, checking)
                                .debug_selector(|| "docker-form-cancel".into())
                                .child("Cancel")
                                .when(!checking, |d| {
                                    d.on_click(
                                        cx.listener(|this, _, _, cx| this.close_docker_form(cx)),
                                    )
                                }),
                        )
                        .child(
                            primary_button("docker-form-save", save_label, busy)
                                .debug_selector(|| "docker-form-save".into())
                                .child(save_label)
                                .when(!busy, |d| {
                                    d.on_click(cx.listener(|this, _, _, cx| this.docker_save(cx)))
                                }),
                        ),
                )
                .into_any_element(),
        )
    }
}

fn table_header() -> AnyElement {
    let cell = |text: &'static str| {
        div()
            .text_size(px(11.))
            .font_weight(FontWeight::MEDIUM)
            .text_color(content(0.45))
            .child(text)
    };
    div()
        .px_4()
        .py(px(8.))
        .flex()
        .items_center()
        .gap_3()
        .child(cell("Docker ID").w(px(140.)))
        .child(cell("Status").w(px(210.)))
        .child(cell("Token expires").w(px(100.)))
        .child(cell("Used by").flex_1())
        .child(div().w(px(260.)))
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::expiry_label;

    #[test]
    fn expiry_is_a_date_or_not_known() {
        assert_eq!(expiry_label(None), "Not known");
        assert_eq!(expiry_label(Some(0)), "1970-01-01");
        // 2027-02-10T00:00:00Z
        assert_eq!(expiry_label(Some(1_802_217_600_000)), "2027-02-10");
        // A leap day.
        assert_eq!(expiry_label(Some(1_709_164_800_000)), "2024-02-29");
    }
}
