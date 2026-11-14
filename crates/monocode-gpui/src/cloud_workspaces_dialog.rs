//! The project menu's Cloud workspaces dialog (Cloud setup > Cloud workspaces): every codespace
//! of the project's repository and every Monoptah Docker sandbox, with its state, age, the
//! sessions that use it and an estimated cost, so a failed or forgotten workspace that keeps
//! drawing quota or credit can be stopped or deleted. `model` holds what it says and decides.
//!
//! Loading is cheap and safe: the list comes from `gh codespace list` and `sbx ls` on the
//! background executor, only when the dialog opens or Refresh is pressed, and the core service
//! caches it for 30 s. Nothing here starts a stopped workspace. A backend that cannot be read
//! (CLI missing, signed out) is explained in a notice with the command that fixes it, and the
//! other backend still lists. There is no spinner: the old rows stay put while a refresh runs.
//!
//! Stop and Delete reuse the app API's `cloud.stop` / `cloud.delete` checks (busy sessions,
//! unsaved files) and run as the Docker account a sandbox row was listed under. The dialog does
//! not run them itself: it emits [`Event::Act`], the shell adds its view of what is busy (a copy
//! taken at that moment, see [`CloudWorkspacesDialog::run`]) and calls it, and a delete is
//! reported with [`Event::Deleted`] so the shell forgets the sessions exactly like the other
//! delete paths.
//!
//! Stop asks first when a session is working on the row or the workspace is not this app's (it
//! would cut the turn off, or the work of whoever uses it), and a warning the stop returns (the
//! sessions it interrupted) stays on screen.
//!
//! Delete asks the workspace what would be lost before it asks the user
//! ([`monocode_core::cloud_sessions::CloudSessions::delete_preview`]), shows it, and sends back the
//! lines the user saw: the delete refuses anything new it finds. A workspace that this app's
//! store does not know needs an explicit acknowledgement on top, and the call carries
//! `acknowledged` so the core refuses it without one. The dialog cannot be closed while a stop or
//! delete runs, so its result always reaches the shell.
pub(crate) mod model;
#[cfg(test)]
pub(crate) mod tests;

use crate::{
    cloud_actions::LiveBusy,
    cloud_dialog_ui::{alert, code_box, heading, muted},
    cloud_mark::tone_color,
    runtime::Database,
    settings::widgets::{checkbox, secondary_button},
    theme::*,
};
use gpui::{
    AnyElement, Context, Div, ElementId, EventEmitter, FocusHandle, Focusable, FontWeight,
    IntoElement, KeyDownEvent, Render, Role, SharedString, Stateful, Window, div, prelude::*, px,
    rgb, rgba,
};
use monocode_core::{
    cloud_sessions::{
        CloudRef,
        workspaces::{self, DeletePreview, WorkspaceOverview, WorkspaceRow, WorkspaceTarget},
    },
    mono::cloud_api::{CloudApi, DeleteOptions},
};
use std::{collections::HashMap, sync::Arc};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Action {
    Stop,
    Delete,
}

pub(crate) enum Event {
    Close,
    /// The user chose Stop or confirmed Delete: the shell adds its busy view (a copy) and calls
    /// [`CloudWorkspacesDialog::run`]. `confirmed` is what the user was shown that a delete
    /// would lose.
    Act {
        target: WorkspaceTarget,
        action: Action,
        confirmed: Vec<String>,
        /// The user ticked the acknowledgement the confirmation asked for (a workspace this app
        /// did not create, something to lose, or contents that cannot be read).
        acknowledged: bool,
    },
    /// A workspace was stopped; the shell re-reads its session badges.
    Stopped(CloudRef),
    /// A workspace was deleted, and these sessions went with it.
    Deleted {
        cloud: CloudRef,
        removed: Vec<i64>,
    },
}

/// The outside world the dialog reaches: the app's cloud service and its store.
#[derive(Clone)]
pub(crate) struct Env {
    /// `None` uses the process-wide service ([`CloudApi::shared`]), built on first use off the
    /// UI thread; tests pass their own over fake `gh` and `sbx`.
    pub api: Option<Arc<CloudApi>>,
    pub store: Database,
}

impl Env {
    pub(crate) fn system(store: Database) -> Self {
        Self { api: None, store }
    }
    pub(crate) fn api(&self) -> Result<Arc<CloudApi>, String> {
        match &self.api {
            Some(api) => Ok(api.clone()),
            None => CloudApi::shared(&self.store),
        }
    }
}

pub(crate) struct CloudWorkspacesDialog {
    project: String,
    name: String,
    env: Env,
    view: Option<WorkspaceOverview>,
    /// A load is running; the rows already shown stay.
    loading: bool,
    /// The cloud service itself could not be built.
    load_error: Option<String>,
    /// Actions in flight, by [`WorkspaceRow::key`]: `true` is a delete.
    pending: HashMap<String, bool>,
    /// The workspace whose Delete asks for confirmation.
    confirm: Option<Confirm>,
    /// The workspace whose Stop asks for confirmation ([`WorkspaceRow::key`]).
    confirm_stop: Option<String>,
    /// What a finished stop or delete warned about, as `(row key, text)`: the sessions a stop
    /// interrupted, a machine that could not be disconnected. Kept until the next action on the
    /// same workspace.
    warnings: Vec<(String, String)>,
    /// A reload was asked for while one was running (an action finished): the running one began
    /// before it and may carry the old state, so it is read again as soon as it ends. Holds
    /// whether that reload is forced.
    reload_queued: Option<bool>,
    /// The last failed action or browser lookup, by [`WorkspaceRow::key`].
    errors: HashMap<String, String>,
    focus: FocusHandle,
}

/// What a Delete confirmation is waiting on.
struct Confirm {
    key: String,
    preview: Preview,
    /// The user ticked the acknowledgement that arms Delete.
    acknowledged: bool,
}

enum Preview {
    /// The workspace is being asked what would be lost.
    Reading,
    Ready(DeletePreview),
    /// What would be lost could not be read: the delete is not offered.
    Failed(String),
}

impl EventEmitter<Event> for CloudWorkspacesDialog {}

impl Focusable for CloudWorkspacesDialog {
    fn focus_handle(&self, _: &gpui::App) -> FocusHandle {
        self.focus.clone()
    }
}

fn now_ms() -> u64 {
    monocode_core::time::now_ms()
}

impl CloudWorkspacesDialog {
    pub(crate) fn new(project: String, name: String, env: Env, cx: &mut Context<Self>) -> Self {
        let mut this = Self {
            project,
            name,
            env,
            view: None,
            loading: false,
            load_error: None,
            pending: HashMap::new(),
            confirm: None,
            confirm_stop: None,
            warnings: Vec::new(),
            reload_queued: None,
            errors: HashMap::new(),
            focus: cx.focus_handle(),
        };
        // Opening is one of the two moments the CLIs are asked; the service answers from its
        // cache when the list is younger than 30 s.
        this.load(false, cx);
        this
    }

    /// The explicit Refresh: asks gh and sbx again.
    pub(crate) fn refresh(&mut self, cx: &mut Context<Self>) {
        self.load(true, cx);
    }

    fn load(&mut self, force: bool, cx: &mut Context<Self>) {
        if self.loading {
            self.reload_queued = Some(self.reload_queued.unwrap_or(false) || force);
            return;
        }
        self.loading = true;
        cx.notify();
        let (env, project) = (self.env.clone(), self.project.clone());
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let api = env.api()?;
                    Ok::<_, String>(api.cloud().workspaces_overview(&project, force))
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.loading = false;
                // Asked for again while this ran: this result predates what asked, so it is not
                // shown (nor does it replace the old rows with a stale list).
                if let Some(force) = this.reload_queued.take() {
                    this.load(force, cx);
                    return;
                }
                match result {
                    Ok(view) => {
                        this.load_error = None;
                        this.view = Some(view);
                    }
                    Err(error) => this.load_error = Some(error),
                }
                cx.notify();
            });
        })
        .detach();
    }

    #[cfg(test)]
    pub(crate) fn view(&self) -> Option<&WorkspaceOverview> {
        self.view.as_ref()
    }
    #[cfg(test)]
    pub(crate) fn is_loading(&self) -> bool {
        self.loading
    }
    #[cfg(test)]
    pub(crate) fn error_of(&self, target: &WorkspaceTarget) -> Option<&str> {
        self.errors
            .get(&workspaces::target_key(
                &target.cloud,
                target.account.as_deref(),
            ))
            .map(String::as_str)
    }
    #[cfg(test)]
    pub(crate) fn asks_to_delete(&self, target: &WorkspaceTarget) -> bool {
        self.confirm.as_ref().is_some_and(|c| {
            c.key == workspaces::target_key(&target.cloud, target.account.as_deref())
        })
    }
    #[cfg(test)]
    pub(crate) fn preview(&self) -> Option<&DeletePreview> {
        match &self.confirm.as_ref()?.preview {
            Preview::Ready(preview) => Some(preview),
            _ => None,
        }
    }
    #[cfg(test)]
    pub(crate) fn asks_to_stop(&self, target: &WorkspaceTarget) -> bool {
        self.confirm_stop.as_deref()
            == Some(&workspaces::target_key(
                &target.cloud,
                target.account.as_deref(),
            ))
    }
    /// What finished actions warned about, as `(workspace name, text)`.
    #[cfg(test)]
    pub(crate) fn warnings(&self) -> Vec<String> {
        self.warnings.iter().map(|(_, text)| text.clone()).collect()
    }
    #[cfg(test)]
    pub(crate) fn acknowledged(&self) -> bool {
        self.confirm.as_ref().is_some_and(|c| c.acknowledged)
    }

    /// A stop or delete is running. The dialog stays open until it ends, so its result reaches
    /// the shell.
    pub(crate) fn working(&self) -> bool {
        !self.pending.is_empty()
    }

    /// Escape: leaves the delete confirmation first. `true` when it did.
    pub(crate) fn cancel_confirm(&mut self, cx: &mut Context<Self>) -> bool {
        let had = self.confirm.take().is_some() | self.confirm_stop.take().is_some();
        if had {
            cx.notify();
        }
        had
    }

    /// Delete was pressed: the workspace is asked what would be lost (on a worker thread; a
    /// codespace answers from GitHub, a sandbox is never started to read it) and the
    /// confirmation shows the answer.
    pub(crate) fn ask_to_delete(&mut self, target: &WorkspaceTarget, cx: &mut Context<Self>) {
        let key = workspaces::target_key(&target.cloud, target.account.as_deref());
        if self.pending.contains_key(&key) {
            return;
        }
        self.errors.remove(&key);
        self.confirm_stop = None;
        self.confirm = Some(Confirm {
            key: key.clone(),
            preview: Preview::Reading,
            acknowledged: false,
        });
        cx.notify();
        let env = self.env.clone();
        let asked = target.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { env.api()?.cloud().delete_preview(&asked) })
                .await;
            let _ = this.update(cx, |this, cx| {
                // The user may have left or opened another one meanwhile.
                if let Some(confirm) = this.confirm.as_mut().filter(|c| c.key == key) {
                    confirm.preview = match result {
                        Ok(preview) => Preview::Ready(preview),
                        Err(error) => Preview::Failed(error),
                    };
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn toggle_acknowledgement(&mut self, cx: &mut Context<Self>) {
        if let Some(confirm) = &mut self.confirm {
            confirm.acknowledged = !confirm.acknowledged;
            cx.notify();
        }
    }

    /// Stop was pressed. A row a session is working on, or one this app did not create, asks
    /// first; any other stops at once (it only stops, the state is kept).
    pub(crate) fn stop_pressed(
        &mut self,
        target: &WorkspaceTarget,
        needs_confirmation: bool,
        cx: &mut Context<Self>,
    ) {
        let key = workspaces::target_key(&target.cloud, target.account.as_deref());
        if self.pending.contains_key(&key) {
            return;
        }
        if !needs_confirmation {
            self.confirm = None;
            self.confirm_stop = None;
            cx.emit(Event::Act {
                target: target.clone(),
                action: Action::Stop,
                confirmed: Vec::new(),
                acknowledged: false,
            });
            return;
        }
        self.errors.remove(&key);
        self.confirm = None;
        self.confirm_stop = Some(key);
        cx.notify();
    }

    /// What the Delete button sends: the lines the user saw, once the workspace was read and the
    /// user said what the read asked for. `None` while Delete is not allowed.
    fn confirmed_lines(&self) -> Option<Vec<String>> {
        let confirm = self.confirm.as_ref()?;
        let Preview::Ready(preview) = &confirm.preview else {
            return None;
        };
        let armed = !preview.needs_acknowledgement() || confirm.acknowledged;
        (armed && preview.blockers.is_empty()).then(|| preview.lines.clone())
    }

    /// Stops or deletes `target` on a worker thread. The checks are the app API's: a working
    /// session, a session moved into the workspace, a workspace this app did not create without
    /// `acknowledged`, or anything to lose that `confirmed` does not name refuse a delete, and they
    /// are made again right before it. A sandbox delete stops it first and waits until it has
    /// stopped.
    ///
    /// `busy` is the shell's view of an archive or convert in progress, COPIED when the user
    /// clicked: it does not change during the wait, so an archive that begins after the click is
    /// not seen by the re-check. During a sandbox's wait the protection is the state check (a
    /// sandbox that is running again is not deleted), and the sessions the store and the backend
    /// are read for again.
    pub(crate) fn run(
        &mut self,
        target: WorkspaceTarget,
        action: Action,
        confirmed: Vec<String>,
        acknowledged: bool,
        busy: LiveBusy,
        cx: &mut Context<Self>,
    ) {
        let key = workspaces::target_key(&target.cloud, target.account.as_deref());
        if self.pending.contains_key(&key) {
            return;
        }
        self.pending.insert(key.clone(), action == Action::Delete);
        self.errors.remove(&key);
        self.warnings.retain(|(warned, _)| *warned != key);
        self.confirm = None;
        self.confirm_stop = None;
        cx.notify();
        let env = self.env.clone();
        cx.spawn(async move |this, cx| {
            let acted = target.clone();
            let result = cx
                .background_executor()
                .spawn(async move {
                    let api = env.api()?;
                    match action {
                        Action::Stop => api.stop_instance(&acted, &busy),
                        Action::Delete => api.delete_instance(
                            &acted,
                            &busy,
                            DeleteOptions {
                                confirmed: Some(confirmed),
                                acknowledged,
                                settle: true,
                            },
                        ),
                    }
                })
                .await;
            let _ = this.update(cx, |this, cx| this.finish(target, action, result, cx));
        })
        .detach();
    }

    fn finish(
        &mut self,
        target: WorkspaceTarget,
        action: Action,
        result: Result<serde_json::Value, String>,
        cx: &mut Context<Self>,
    ) {
        let key = workspaces::target_key(&target.cloud, target.account.as_deref());
        self.pending.remove(&key);
        let cloud = target.cloud;
        match result {
            Ok(value) => {
                // What the call warned about (the sessions a stop interrupted, a machine that
                // could not be disconnected) is shown, not dropped.
                if let Some(warning) = value["warning"].as_str() {
                    self.warnings
                        .push((key.clone(), format!("{}: {warning}", cloud.name)));
                }
                match action {
                    Action::Stop => cx.emit(Event::Stopped(cloud)),
                    Action::Delete => {
                        let removed = crate::cloud_actions::deleted("cloud.delete", &value)
                            .map(|(_, ids)| ids)
                            .unwrap_or_default();
                        if let Some(view) = &mut self.view {
                            view.rows.retain(|row| row.key() != key);
                        }
                        cx.emit(Event::Deleted { cloud, removed });
                    }
                }
            }
            Err(error) => {
                self.errors.insert(key, error);
            }
        }
        // What the workspace is now (stopping, or still there after a refusal) comes from the
        // CLIs, not from a guess.
        self.refresh(cx);
        cx.notify();
    }

    /// Opens the codespace in the browser. The address is read through `gh api`, which never
    /// starts it.
    pub(crate) fn open_in_browser(&mut self, cloud: CloudRef, cx: &mut Context<Self>) {
        let env = self.env.clone();
        let key = workspaces::target_key(&cloud, None);
        self.errors.remove(&key);
        cx.spawn(async move |this, cx| {
            let target = cloud.clone();
            let url = cx
                .background_executor()
                .spawn(async move { env.api()?.cloud().workspace_web_url(&target) })
                .await;
            let _ = this.update(cx, |this, cx| {
                match url {
                    Ok(url) => cx.open_url(&url),
                    Err(error) => {
                        this.errors.insert(key, error);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    // ------------------------------------------------------------------ drawing

    fn button(id: String, label: &'static str, danger: bool, disabled: bool) -> Stateful<Div> {
        let selector = id.clone();
        secondary_button(
            ElementId::from(SharedString::from(id)),
            label,
            danger,
            disabled,
        )
        .child(label)
        .debug_selector(move || selector.clone())
    }

    fn notice(notice: &model::Notice) -> AnyElement {
        let text = if notice.problem {
            alert(notice.id, notice.text.clone()).into_any_element()
        } else {
            div()
                .id(notice.id)
                .debug_selector(|| notice.id.into())
                .child(muted(notice.text.clone()))
                .into_any_element()
        };
        let mut column = div().flex().flex_col().gap_1().child(text);
        if let Some(fix) = &notice.fix {
            column = column
                .child(muted("Run this in a terminal, then Refresh:"))
                .child(code_box(fix));
        }
        column.into_any_element()
    }

    fn row(&mut self, row: &WorkspaceRow, now: u64, cx: &mut Context<Self>) -> AnyElement {
        let cloud = row.cloud.clone();
        let target = row.target();
        let name = cloud.name.clone();
        let key = row.key();
        let offers = model::offers(row);
        let working = self.pending.get(&key).copied();
        let confirming = self.confirm.as_ref().is_some_and(|c| c.key == key);
        let confirming_stop = self.confirm_stop.as_deref() == Some(key.as_str());
        let (uses, usage) = model::uses_line(row);
        let orphan = usage == model::Use::Orphan;
        let foreign = usage == model::Use::Foreign;
        let tone = model::state_tone(row.state);
        let id = |part: &str| format!("cloud-ws-{part}-{name}");

        let title = div()
            .flex()
            .items_center()
            .gap_2()
            .child(icon("cloud", 14.).text_color(content(0.6)))
            .child(
                div()
                    .flex_shrink_0()
                    .text_size(px(11.))
                    .text_color(content(0.5))
                    .child(model::backend(row)),
            )
            .child(
                div()
                    .min_w_0()
                    .flex_1()
                    .truncate()
                    .text_size(px(13.))
                    .font_weight(FontWeight::MEDIUM)
                    .child(row.display_name.clone()),
            )
            .child(
                div()
                    .id(SharedString::from(id("state")))
                    .debug_selector({
                        let selector = id("state");
                        move || selector.clone()
                    })
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .text_size(px(12.))
                    .text_color(content(0.7))
                    .child(div().size(px(7.)).rounded_full().bg(tone_color(tone)))
                    .child(row.state.label()),
            );

        let mut card = div()
            .id(SharedString::from(id("row")))
            .debug_selector({
                let selector = id("row");
                move || selector.clone()
            })
            .role(Role::ListItem)
            .aria_label(format!(
                "{} {}: {}",
                model::backend(row),
                row.display_name,
                row.state.label()
            ))
            .flex()
            .flex_col()
            .gap(px(6.))
            .p_3()
            .rounded_lg()
            .border_1()
            .border_color(if orphan || foreign {
                rgba(0xfbbf2480).into()
            } else {
                content(0.1)
            })
            .when(orphan || foreign, |d| d.bg(rgba(0xfbbf2412)))
            .child(title)
            .child(muted(model::detail_line(row)));
        if let Some(git) = model::git_line(row) {
            card = card.child(
                div()
                    .id(SharedString::from(id("git")))
                    .debug_selector({
                        let selector = id("git");
                        move || selector.clone()
                    })
                    .child(muted(git)),
            );
        }
        if let Some(age) = model::age_line(row, now) {
            card = card.child(muted(age));
        }
        card = card
            .child(
                div()
                    .id(SharedString::from(id("uses")))
                    .debug_selector({
                        let selector = id(if orphan {
                            "orphan"
                        } else if foreign {
                            "foreign"
                        } else {
                            "uses"
                        });
                        move || selector.clone()
                    })
                    .text_size(px(12.))
                    .line_height(px(18.))
                    .text_color(if orphan || foreign {
                        rgb(0xfbbf24).into()
                    } else {
                        content(0.7)
                    })
                    .child(uses),
            )
            .child(
                div()
                    .id(SharedString::from(id("cost")))
                    .debug_selector({
                        let selector = id("cost");
                        move || selector.clone()
                    })
                    .text_size(px(12.))
                    .line_height(px(18.))
                    .text_color(content(0.6))
                    .child(model::cost_line(row)),
            );

        if let Some(error) = self.errors.get(&key) {
            card = card.child(alert(id("error"), error.clone()));
        }
        if let Some(deleting) = working {
            return card
                .child(
                    div()
                        .id(SharedString::from(id("working")))
                        .debug_selector({
                            let selector = id("working");
                            move || selector.clone()
                        })
                        .text_size(px(12.))
                        .text_color(content(0.7))
                        .child(model::working_text(&cloud, deleting)),
                )
                .into_any_element();
        }
        if confirming {
            return self.confirm_block(card, row, cx);
        }
        if confirming_stop {
            return self.confirm_stop_block(card, row, cx);
        }

        let stop_target = target.clone();
        // Cutting off a working session, or stopping what someone else may be using, is asked.
        let stop_asks = model::stop_asks(row);
        let delete_target = target.clone();
        let open_target = cloud.clone();
        let mut actions = div().flex().gap_2().pt_1();
        actions = actions
            .child(
                Self::button(id("stop"), "Stop", false, !offers.stop).when(offers.stop, |b| {
                    b.on_click(cx.listener(move |this, _, _, cx| {
                        this.stop_pressed(&stop_target, stop_asks, cx)
                    }))
                }),
            )
            .child(
                Self::button(id("delete"), "Delete", true, !offers.delete).on_click(
                    cx.listener(move |this, _, _, cx| this.ask_to_delete(&delete_target, cx)),
                ),
            );
        if offers.browser {
            actions = actions.child(
                Self::button(id("browser"), "Open in browser", false, false).on_click(
                    cx.listener(move |this, _, _, cx| {
                        this.open_in_browser(open_target.clone(), cx)
                    }),
                ),
            );
        }
        card.child(actions).into_any_element()
    }

    /// The row while its Delete asks: what the workspace says would be lost, the acknowledgement
    /// the read asks for, and Delete (armed only once the read is in and the user answered).
    fn confirm_block(
        &mut self,
        card: Stateful<Div>,
        row: &WorkspaceRow,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let name = row.cloud.name.clone();
        let id = |part: &str| format!("cloud-ws-{part}-{name}");
        let mut prompt = div()
            .id(SharedString::from(id("confirm")))
            .debug_selector({
                let selector = id("confirm");
                move || selector.clone()
            })
            .flex()
            .flex_col()
            .gap_1()
            .p_2()
            .rounded_md()
            .border_1()
            .border_color(rgba(0xf8717166))
            .bg(rgba(0xf871710f));
        let line = |text: String| {
            div()
                .text_size(px(12.))
                .line_height(px(18.))
                .text_color(content(0.85))
                .child(text)
        };
        let armed = self.confirmed_lines();
        let mut acknowledgement = None;
        match self.confirm.as_ref().map(|c| (&c.preview, c.acknowledged)) {
            Some((Preview::Ready(preview), acknowledged)) => {
                for text in model::delete_prompt(row, preview) {
                    prompt = prompt.child(line(text));
                }
                if preview.needs_acknowledgement() && preview.blockers.is_empty() {
                    acknowledgement =
                        Some((model::acknowledgement_label(row, preview), acknowledged));
                }
            }
            Some((Preview::Failed(error), _)) => {
                prompt = prompt.child(
                    div()
                        .id(SharedString::from(id("preview-error")))
                        .debug_selector({
                            let selector = id("preview-error");
                            move || selector.clone()
                        })
                        .child(alert(
                            id("preview-error-text"),
                            format!(
                                "Could not read what would be lost, so Delete is not offered: {error}"
                            ),
                        )),
                );
            }
            _ => {
                prompt = prompt.child(line(model::READING_PROMPT.into()));
            }
        }
        if let Some((label, acknowledged)) = acknowledgement {
            prompt = prompt.child(
                div()
                    .flex()
                    .items_start()
                    .gap_2()
                    .child(
                        checkbox(id("acknowledge"), label.clone(), acknowledged, false).on_click(
                            cx.listener(|this, _, _, cx| this.toggle_acknowledgement(cx)),
                        ),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .line_height(px(18.))
                            .text_color(content(0.85))
                            .child(label),
                    ),
            );
        }
        let target = row.target();
        let confirmed = armed.clone();
        let acknowledged = self.confirm.as_ref().is_some_and(|c| c.acknowledged);
        let delete = Self::button(
            id("confirm-delete"),
            "Delete permanently",
            true,
            armed.is_none(),
        )
        .when_some(confirmed, |button, lines| {
            button.on_click(cx.listener(move |_, _, _, cx| {
                cx.emit(Event::Act {
                    target: target.clone(),
                    action: Action::Delete,
                    confirmed: lines.clone(),
                    acknowledged,
                })
            }))
        });
        card.child(
            prompt.child(div().flex().gap_2().pt_1().child(delete).child(
                Self::button(id("cancel"), "Cancel", false, false).on_click(cx.listener(
                    |this, _, _, cx| {
                        this.cancel_confirm(cx);
                    },
                )),
            )),
        )
        .into_any_element()
    }

    /// The row while its Stop asks: why it asks, and Stop (the state is kept) or Cancel.
    fn confirm_stop_block(
        &mut self,
        card: Stateful<Div>,
        row: &WorkspaceRow,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let name = row.cloud.name.clone();
        let id = |part: &str| format!("cloud-ws-{part}-{name}");
        let mut prompt = div()
            .id(SharedString::from(id("confirm-stop")))
            .debug_selector({
                let selector = id("confirm-stop");
                move || selector.clone()
            })
            .flex()
            .flex_col()
            .gap_1()
            .p_2()
            .rounded_md()
            .border_1()
            .border_color(rgba(0xfbbf2466))
            .bg(rgba(0xfbbf240f));
        for text in model::stop_prompt(row) {
            prompt = prompt.child(
                div()
                    .text_size(px(12.))
                    .line_height(px(18.))
                    .text_color(content(0.85))
                    .child(text),
            );
        }
        let target = row.target();
        let stop = Self::button(id("confirm-stop-button"), "Stop it", false, false).on_click(
            cx.listener(move |_, _, _, cx| {
                cx.emit(Event::Act {
                    target: target.clone(),
                    action: Action::Stop,
                    confirmed: Vec::new(),
                    acknowledged: false,
                })
            }),
        );
        card.child(prompt.child(div().flex().gap_2().pt_1().child(stop).child(
            Self::button(id("cancel-stop"), "Cancel", false, false).on_click(cx.listener(
                |this, _, _, cx| {
                    this.cancel_confirm(cx);
                },
            )),
        )))
        .into_any_element()
    }

    fn body(&mut self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let mut out: Vec<AnyElement> = Vec::new();
        if let Some(error) = self.load_error.clone() {
            out.push(alert("cloud-workspaces-load-error", error).into_any_element());
        }
        let Some(view) = self.view.clone() else {
            if self.load_error.is_none() {
                out.push(
                    div()
                        .id("cloud-workspaces-loading")
                        .debug_selector(|| "cloud-workspaces-loading".into())
                        .child(muted("Reading your codespaces and sandboxes\u{2026}"))
                        .into_any_element(),
                );
            }
            return out;
        };
        out.extend(model::notices(&view).iter().map(Self::notice));
        for (ix, (_, text)) in self.warnings.iter().enumerate() {
            let id = format!("cloud-ws-warning-{ix}");
            out.push(
                div()
                    .id(SharedString::from(id.clone()))
                    .debug_selector(move || id.clone())
                    .role(Role::Alert)
                    .text_size(px(12.))
                    .line_height(px(18.))
                    .text_color(rgb(0xfbbf24))
                    .child(text.clone())
                    .into_any_element(),
            );
        }
        let summary = model::summary_line(&model::summary(&view));
        out.push(
            div()
                .id("cloud-workspaces-summary")
                .debug_selector(|| "cloud-workspaces-summary".into())
                .text_size(px(12.))
                .font_weight(FontWeight::MEDIUM)
                .child(summary)
                .into_any_element(),
        );
        for row in &view.rows {
            out.push(self.row(row, view.loaded_ms, cx));
        }
        out.push(
            div()
                .id("cloud-workspaces-estimate")
                .debug_selector(|| "cloud-workspaces-estimate".into())
                .child(muted(model::ESTIMATE_NOTE))
                .into_any_element(),
        );
        out
    }
}

impl Render for CloudWorkspacesDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let body = self.body(cx);
        let height = window.viewport_size().height.as_f32() - 34.;
        let subtitle = match self.view.as_ref() {
            Some(view) => {
                let ago = now_ms().saturating_sub(view.loaded_ms) / 1000;
                let repo = view.repo.clone().unwrap_or_else(|| self.name.clone());
                format!("{repo} \u{b7} updated {} ago", model::age_text(ago))
            }
            None => self.name.clone(),
        };
        let loading = self.loading;
        let working = self.working();
        div()
            .id("cloud-workspaces-dialog")
            .debug_selector(|| "cloud-workspaces-dialog".into())
            .role(Role::Dialog)
            .aria_label("Cloud workspaces")
            .track_focus(&self.focus)
            .relative()
            .min_h_0()
            .max_h(px(height))
            .flex()
            .flex_col()
            .text_color(content(1.))
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                if event.keystroke.key == "escape" {
                    // While a stop or delete runs the dialog stays, so its result reaches the shell.
                    if !this.working() && !this.cancel_confirm(cx) {
                        cx.emit(Event::Close);
                    }
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
                    .child(heading("Cloud workspaces", subtitle))
                    .child(
                        Self::button(
                            "cloud-ws-refresh".into(),
                            if loading {
                                "Refreshing\u{2026}"
                            } else {
                                "Refresh"
                            },
                            false,
                            loading,
                        )
                        .when(!loading, |b| {
                            b.on_click(cx.listener(|this, _, _, cx| this.refresh(cx)))
                        }),
                    )
                    .child(
                        icon_button("cloud-ws-close", "Close Cloud workspaces", "close")
                            .size(px(28.))
                            .when(working, |b| b.opacity(0.4))
                            .on_click(cx.listener(|this, _, _, cx| {
                                if !this.working() {
                                    cx.emit(Event::Close)
                                }
                            })),
                    ),
            )
            .child(
                div()
                    .id("cloud-workspaces-body")
                    .min_h_0()
                    .overflow_y_scroll()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .p_4()
                    .children(body),
            )
    }
}
