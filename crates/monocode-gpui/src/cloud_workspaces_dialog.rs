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
//! unsaved files). The dialog does not run them itself: it emits [`Event::Act`], the shell adds
//! its live busy view and calls [`CloudWorkspacesDialog::run`], and a delete is reported with
//! [`Event::Deleted`] so the shell forgets the sessions exactly like the other delete paths.
pub(crate) mod model;
#[cfg(test)]
pub(crate) mod tests;

use crate::{
    cloud_actions::LiveBusy,
    cloud_dialog_ui::{alert, code_box, heading, muted},
    cloud_mark::tone_color,
    runtime::Database,
    settings::widgets::secondary_button,
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
        workspaces::{WorkspaceOverview, WorkspaceRow},
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
    /// The user chose Stop or confirmed Delete: the shell adds its live busy view and calls
    /// [`CloudWorkspacesDialog::run`].
    Act {
        cloud: CloudRef,
        action: Action,
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
    fn api(&self) -> Result<Arc<CloudApi>, String> {
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
    /// Actions in flight, by workspace key: `true` is a delete.
    pending: HashMap<String, bool>,
    /// The workspace whose Delete asks for confirmation.
    confirm: Option<String>,
    /// The last failed action or browser lookup, by workspace key.
    errors: HashMap<String, String>,
    focus: FocusHandle,
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
        if std::mem::replace(&mut self.loading, true) {
            return;
        }
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
    pub(crate) fn error_of(&self, cloud: &CloudRef) -> Option<&str> {
        self.errors.get(&cloud.key()).map(String::as_str)
    }
    #[cfg(test)]
    pub(crate) fn asks_to_delete(&self, cloud: &CloudRef) -> bool {
        self.confirm.as_deref() == Some(cloud.key().as_str())
    }

    /// Escape: leaves the delete confirmation first. `true` when it did.
    pub(crate) fn cancel_confirm(&mut self, cx: &mut Context<Self>) -> bool {
        let had = self.confirm.take().is_some();
        if had {
            cx.notify();
        }
        had
    }

    pub(crate) fn ask_to_delete(&mut self, cloud: &CloudRef, cx: &mut Context<Self>) {
        if !self.pending.contains_key(&cloud.key()) {
            self.confirm = Some(cloud.key());
            cx.notify();
        }
    }

    /// Stops or deletes `cloud` on a worker thread with the shell's live busy view. The checks
    /// are the app API's: a working session, an archive in progress or unsaved files refuse the
    /// call (the confirmation already showed the unsaved files). A sandbox delete stops it first
    /// and waits until it has stopped.
    pub(crate) fn run(
        &mut self,
        cloud: CloudRef,
        action: Action,
        busy: LiveBusy,
        cx: &mut Context<Self>,
    ) {
        let key = cloud.key();
        if self.pending.contains_key(&key) {
            return;
        }
        self.pending.insert(key.clone(), action == Action::Delete);
        self.errors.remove(&key);
        self.confirm = None;
        cx.notify();
        let env = self.env.clone();
        cx.spawn(async move |this, cx| {
            let target = cloud.clone();
            let result = cx
                .background_executor()
                .spawn(async move {
                    let api = env.api()?;
                    match action {
                        Action::Stop => api.stop_instance(&target, &busy),
                        Action::Delete => api.delete_instance(
                            &target,
                            &busy,
                            DeleteOptions {
                                accept_unsaved: true,
                                settle: true,
                            },
                        ),
                    }
                })
                .await;
            let _ = this.update(cx, |this, cx| this.finish(cloud, action, result, cx));
        })
        .detach();
    }

    fn finish(
        &mut self,
        cloud: CloudRef,
        action: Action,
        result: Result<serde_json::Value, String>,
        cx: &mut Context<Self>,
    ) {
        let key = cloud.key();
        self.pending.remove(&key);
        match result {
            Ok(value) => match action {
                Action::Stop => cx.emit(Event::Stopped(cloud)),
                Action::Delete => {
                    let removed = crate::cloud_actions::deleted("cloud.delete", &value)
                        .map(|(_, ids)| ids)
                        .unwrap_or_default();
                    if let Some(view) = &mut self.view {
                        view.rows.retain(|row| row.cloud != cloud);
                    }
                    cx.emit(Event::Deleted { cloud, removed });
                }
            },
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
        self.errors.remove(&cloud.key());
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
                        this.errors.insert(cloud.key(), error);
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
        let name = cloud.name.clone();
        let key = cloud.key();
        let offers = model::offers(row);
        let working = self.pending.get(&key).copied();
        let confirming = self.confirm.as_deref() == Some(key.as_str());
        let (uses, orphan) = model::uses_line(row);
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
            .border_color(if orphan {
                rgba(0xfbbf2480).into()
            } else {
                content(0.1)
            })
            .when(orphan, |d| d.bg(rgba(0xfbbf2412)))
            .child(title)
            .child(muted(model::detail_line(row)));
        if let Some(age) = model::age_line(row, now) {
            card = card.child(muted(age));
        }
        card = card
            .child(
                div()
                    .id(SharedString::from(id("uses")))
                    .debug_selector({
                        let selector = id(if orphan { "orphan" } else { "uses" });
                        move || selector.clone()
                    })
                    .text_size(px(12.))
                    .line_height(px(18.))
                    .text_color(if orphan {
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
            for line in model::delete_prompt(row) {
                prompt = prompt.child(
                    div()
                        .text_size(px(12.))
                        .line_height(px(18.))
                        .text_color(content(0.85))
                        .child(line),
                );
            }
            let target = cloud.clone();
            let cancel_name = id("cancel");
            return card
                .child(
                    prompt.child(
                        div()
                            .flex()
                            .gap_2()
                            .pt_1()
                            .child(
                                Self::button(
                                    id("confirm-delete"),
                                    "Delete permanently",
                                    true,
                                    false,
                                )
                                .on_click(cx.listener(
                                    move |_, _, _, cx| {
                                        cx.emit(Event::Act {
                                            cloud: target.clone(),
                                            action: Action::Delete,
                                        })
                                    },
                                )),
                            )
                            .child(Self::button(cancel_name, "Cancel", false, false).on_click(
                                cx.listener(|this, _, _, cx| {
                                    this.cancel_confirm(cx);
                                }),
                            )),
                    ),
                )
                .into_any_element();
        }

        let stop_target = cloud.clone();
        let delete_target = cloud.clone();
        let open_target = cloud.clone();
        let mut actions = div().flex().gap_2().pt_1();
        actions = actions
            .child(
                Self::button(id("stop"), "Stop", false, !offers.stop).when(offers.stop, |b| {
                    b.on_click(cx.listener(move |_, _, _, cx| {
                        cx.emit(Event::Act {
                            cloud: stop_target.clone(),
                            action: Action::Stop,
                        })
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
                    if !this.cancel_confirm(cx) {
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
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(Event::Close))),
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
