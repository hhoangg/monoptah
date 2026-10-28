//! The two Telegram dialogs: a project's bot settings and linking a session to a topic.
//! `model` holds what they say and decide; the views here draw it and run the bridge calls it
//! asks for. Every bridge call runs on the background executor.
mod model;
#[cfg(test)]
mod tests;

use crate::{
    input::{self, Input},
    settings::widgets::secondary_button,
    theme::*,
};
use gpui::{
    AnyElement, ClipboardItem, Context, Div, ElementId, Entity, EventEmitter, FocusHandle,
    Focusable, FontWeight, IntoElement, KeyDownEvent, Render, Role, SharedString, Stateful,
    Subscription, Task, Window, div, prelude::*, px, rgb, rgba,
};
use model::{ProjectState, TopicState};
use monocode_core::telegram::{self, Bridge};
use std::{sync::Arc, time::Duration};

pub(crate) enum Event {
    Close,
    /// The project's settings or status changed what the question forwarder may do.
    Changed,
}

pub(crate) enum TopicEvent {
    Close,
}

const BOT_FATHER: &str = "https://t.me/BotFather";
const COPIED_MS: u64 = 1500;

fn muted(text: impl Into<SharedString>) -> Div {
    div()
        .text_size(px(11.))
        .line_height(px(17.))
        .text_color(content(0.45))
        .child(text.into())
}

/// The amber callout the original dialog uses for notes that need attention.
fn callout(text: impl Into<SharedString>) -> Div {
    div()
        .rounded_lg()
        .bg(rgba(0xfbbf241a))
        .px(px(10.))
        .py(px(8.))
        .text_size(px(11.))
        .line_height(px(16.))
        .text_color(if is_light() {
            rgb(0xb45309)
        } else {
            rgb(0xfcd34d)
        })
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

/// A labelled row, set off by a hairline from what is above it.
fn row(label: &'static str, control: impl IntoElement) -> Div {
    div()
        .flex()
        .items_center()
        .justify_between()
        .gap_4()
        .border_t_1()
        .border_color(content(0.08))
        .pt_4()
        .child(
            div()
                .min_w_0()
                .text_size(px(13.))
                .font_weight(FontWeight::MEDIUM)
                .child(label),
        )
        .child(control)
}

fn section() -> Div {
    div()
        .flex()
        .flex_col()
        .gap_3()
        .border_t_1()
        .border_color(content(0.08))
        .pt_4()
}

/// A 36 by 20 switch with a 16 px knob.
fn switch(id: &'static str, label: &'static str, on: bool, disabled: bool) -> Stateful<Div> {
    div()
        .id(id)
        .debug_selector(move || id.into())
        .role(Role::Switch)
        .aria_label(label)
        .aria_toggled(if on {
            gpui::Toggled::True
        } else {
            gpui::Toggled::False
        })
        .relative()
        .h(px(20.))
        .w(px(36.))
        .flex_shrink_0()
        .rounded_full()
        .bg(if on { accent_color() } else { content(0.2) })
        .when(disabled, |d| d.opacity(0.4))
        .when(!disabled, |d| d.cursor_pointer())
        .child(
            div()
                .absolute()
                .top(px(2.))
                .left(px(if on { 18. } else { 2. }))
                .size(px(16.))
                .rounded_full()
                .bg(rgb(0xffffff)),
        )
}

fn chip(id: impl Into<ElementId>, label: &str, selected: bool, disabled: bool) -> Stateful<Div> {
    div()
        .id(id)
        .role(Role::RadioButton)
        .aria_label(label.to_owned())
        .aria_selected(selected)
        .rounded(px(6.))
        .border_1()
        .border_color(if selected { content(0.3) } else { content(0.1) })
        .px(px(8.))
        .py(px(3.))
        .text_size(px(12.))
        .line_height(px(16.))
        .whitespace_nowrap()
        .when(selected, |d| d.bg(content(0.1)).text_color(content(1.)))
        .when(!selected, |d| d.text_color(content(0.6)))
        .when(disabled, |d| d.opacity(0.5))
        .when(!disabled, |d| {
            d.cursor_pointer()
                .hover(|s| s.bg(content(0.07)).text_color(content(1.)))
        })
        .child(label.to_owned())
}

fn code_box(text: &str) -> Div {
    div()
        .min_w_0()
        .flex_1()
        .rounded_md()
        .border_1()
        .border_color(content(0.1))
        .px(px(8.))
        .py(px(4.))
        .text_size(px(12.))
        .line_height(px(18.))
        .child(text.to_owned())
}

fn link_button(id: &'static str, label: &'static str) -> Stateful<Div> {
    div()
        .id(id)
        .debug_selector(move || id.into())
        .role(Role::Link)
        .aria_label(label)
        .cursor_pointer()
        .text_size(px(12.))
        .text_color(content(0.65))
        .hover(|s| s.text_color(content(1.)))
        .child(label)
}

/// `secondary_button` that does nothing while it is disabled.
fn button(
    id: &'static str,
    label: impl Into<SharedString>,
    danger: bool,
    disabled: bool,
) -> Stateful<Div> {
    secondary_button(id, label, danger, disabled).debug_selector(move || id.into())
}

fn modal_header(title: &'static str, description: String, close: Stateful<Div>) -> Div {
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
                        .child(title),
                )
                .child(
                    div()
                        .mt(px(2.))
                        .min_w_0()
                        .truncate()
                        .text_size(px(12.))
                        .line_height(px(16.5))
                        .text_color(content(0.5))
                        .child(description),
                ),
        )
        .child(close.size(px(28.)))
}

pub(crate) struct TelegramDialog {
    project: String,
    name: String,
    bridge: Arc<Bridge>,
    state: ProjectState,
    token: Entity<Input>,
    focus: FocusHandle,
    polling: bool,
    _poll: Task<()>,
    _copied: Option<Task<()>>,
    _events: Vec<Subscription>,
}

impl EventEmitter<Event> for TelegramDialog {}

impl Focusable for TelegramDialog {
    fn focus_handle(&self, _: &gpui::App) -> FocusHandle {
        self.focus.clone()
    }
}

impl TelegramDialog {
    /// `project` is the project's key in the Telegram config; `name` is what the dialog calls it.
    pub(crate) fn new(
        project: String,
        name: String,
        bridge: Arc<Bridge>,
        cx: &mut Context<Self>,
    ) -> Self {
        let token = cx.new(|cx| {
            let mut field = Input::field("Bot token", "123456:ABC...", content(0.35), cx);
            field.set_font_size(13.);
            field.set_secret(true);
            field
        });
        let events = vec![
            cx.subscribe(&token, |_, _, _: &input::Changed, cx| cx.notify()),
            cx.subscribe(&token, |this, _, _: &input::Submitted, cx| {
                this.save_token(cx)
            }),
        ];
        let mut this = Self {
            project,
            name,
            bridge,
            state: ProjectState::default(),
            token,
            focus: cx.focus_handle(),
            polling: false,
            _poll: Task::ready(()),
            _copied: None,
            _events: events,
        };
        this.refresh(cx);
        this._poll = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(model::POLL_MS))
                    .await;
                if this.update(cx, |this, cx| this.refresh(cx)).is_err() {
                    break;
                }
            }
        });
        this
    }

    /// Reads the status again. A read that started before a change is dropped.
    fn refresh(&mut self, cx: &mut Context<Self>) {
        if std::mem::replace(&mut self.polling, true) {
            return;
        }
        let started = self.state.revision();
        let (bridge, project) = (self.bridge.clone(), self.project.clone());
        cx.spawn(async move |this, cx| {
            let next = cx
                .background_executor()
                .spawn(async move { telegram::status(&bridge, &project) })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.polling = false;
                if this.state.poll_done(started, next) {
                    cx.emit(Event::Changed);
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Runs one bridge call off the UI thread while the dialog is busy, then `done` with its
    /// result. `changed` tells the forwarder its cached status is stale, success or not.
    fn run<T: Send + 'static>(
        &mut self,
        changed: bool,
        work: impl FnOnce(&Arc<Bridge>, &str) -> Result<T, String> + Send + 'static,
        done: impl FnOnce(&mut Self, T, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) {
        if !self.state.begin() {
            return;
        }
        cx.notify();
        let (bridge, project) = (self.bridge.clone(), self.project.clone());
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { work(&bridge, &project) })
                .await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(value) => {
                        this.state.finish(None);
                        done(this, value, cx);
                    }
                    Err(error) => this.state.finish(Some(error)),
                }
                if changed {
                    cx.emit(Event::Changed);
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn clear_token(&mut self, cx: &mut Context<Self>) {
        self.token
            .update(cx, |token, cx| token.set_text(String::new(), cx));
    }

    pub(crate) fn save_token(&mut self, cx: &mut Context<Self>) {
        let token = self.token.read(cx).content.trim().to_owned();
        if token.is_empty() || self.state.busy {
            return;
        }
        self.run(
            true,
            move |bridge, project| telegram::set_token(bridge, project, &token),
            |this, next, cx| {
                this.clear_token(cx);
                this.state.token_saved(next);
            },
            cx,
        );
    }

    pub(crate) fn connect(&mut self, cx: &mut Context<Self>) {
        self.run(
            false,
            telegram::connect_link,
            |this, link, _| this.state.link = Some(link),
            cx,
        );
    }

    pub(crate) fn set_enabled(&mut self, enabled: bool, cx: &mut Context<Self>) {
        self.run(
            true,
            move |bridge, project| telegram::set_enabled(bridge, project, enabled),
            |this, next, _| {
                this.state.status_set(next);
            },
            cx,
        );
    }

    pub(crate) fn set_delay(&mut self, secs: u64, cx: &mut Context<Self>) {
        self.run(
            true,
            move |bridge, project| telegram::set_delay(bridge, project, secs),
            |this, next, _| {
                this.state.status_set(next);
            },
            cx,
        );
    }

    pub(crate) fn remove_webhook(&mut self, cx: &mut Context<Self>) {
        self.run(
            false,
            |bridge, project| {
                telegram::remove_webhook(bridge, project)?;
                Ok(telegram::status(bridge, project))
            },
            |this, next, cx| {
                if this.state.status_set(next) {
                    cx.emit(Event::Changed);
                }
            },
            cx,
        );
    }

    pub(crate) fn disconnect(&mut self, cx: &mut Context<Self>) {
        self.run(
            true,
            |bridge, project| {
                telegram::disconnect(bridge, project);
                Ok(telegram::status(bridge, project))
            },
            |this, next, cx| {
                this.clear_token(cx);
                this.state.disconnected(next);
            },
            cx,
        );
    }

    fn edit_token(&mut self, cx: &mut Context<Self>) {
        self.state.edit_token();
        cx.notify();
    }

    fn cancel_token_change(&mut self, cx: &mut Context<Self>) {
        self.clear_token(cx);
        self.state.cancel_token_change();
        cx.notify();
    }

    fn ask_disconnect(&mut self, cx: &mut Context<Self>) {
        self.cancel_token_change(cx);
        self.state.ask_disconnect();
        cx.notify();
    }

    fn copy(&mut self, text: String, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        self.state.copied = true;
        self._copied = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(COPIED_MS))
                .await;
            let _ = this.update(cx, |this, cx| {
                this.state.copied = false;
                cx.notify();
            });
        }));
        cx.notify();
    }

    fn token_form(&mut self, changing: bool, cx: &mut Context<Self>) -> AnyElement {
        let busy = self.state.busy;
        let empty = self.token.read(cx).content.trim().is_empty();
        self.token.update(cx, |token, _| token.set_read_only(busy));
        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .text_size(px(12.))
                    .text_color(content(0.65))
                    .child("Bot token")
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
                            .child(self.token.clone()),
                    ),
            )
            .when(changing, |d| d.child(muted(model::CHANGE_TOKEN_NOTE)))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(
                        button(
                            "telegram-save-token",
                            if busy { "Saving…" } else { "Save" },
                            false,
                            busy || empty,
                        )
                        .when(!busy && !empty, |b| {
                            b.on_click(cx.listener(|this, _, _, cx| this.save_token(cx)))
                        }),
                    )
                    .when(changing, |d| {
                        d.child(button("telegram-cancel-token", "Cancel", false, busy).when(
                            !busy,
                            |b| {
                                b.on_click(
                                    cx.listener(|this, _, _, cx| this.cancel_token_change(cx)),
                                )
                            },
                        ))
                    })
                    .child(
                        link_button("telegram-botfather", "Create a bot with BotFather")
                            .on_click(|_, _, cx| cx.open_url(BOT_FATHER)),
                    ),
            )
            .into_any_element()
    }

    fn connect_section(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let busy = self.state.busy;
        let link = self.state.link.clone();
        let copied = self.state.copied;
        section()
            .child(
                button("telegram-connect", "Connect a group", false, busy).when(!busy, |b| {
                    b.on_click(cx.listener(|this, _, _, cx| this.connect(cx)))
                }),
            )
            .when_some(link, |d, link| {
                let url = link.url.clone();
                let command = link.fallback.clone();
                d.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .child(
                            button("telegram-open", "Open Telegram", false, false)
                                .on_click(move |_, _, cx| cx.open_url(&url)),
                        )
                        .child(muted("Or send this command in the group:"))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .child(code_box(&link.fallback))
                                .child(
                                    button(
                                        "telegram-copy",
                                        if copied { "Copied" } else { "Copy" },
                                        false,
                                        false,
                                    )
                                    .on_click(cx.listener(
                                        move |this, _, _, cx| this.copy(command.clone(), cx),
                                    )),
                                ),
                        )
                        .child(muted("Waiting for the bot to join a group…")),
                )
            })
            .into_any_element()
    }

    fn connected_section(
        &mut self,
        status: &telegram::TelegramStatus,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let busy = self.state.busy;
        let enabled = status.enabled;
        let chips =
            div()
                .id("telegram-delay")
                .role(Role::RadioGroup)
                .aria_label("Send after")
                .flex()
                .flex_wrap()
                .justify_end()
                .gap(px(4.))
                .max_w(px(250.))
                .children(model::delay_options(status.delay_secs).into_iter().map(
                    |(secs, label)| {
                        let selected = secs == status.delay_secs;
                        chip(
                            SharedString::from(format!("telegram-delay-{secs}")),
                            &label,
                            selected,
                            busy,
                        )
                        .debug_selector(move || format!("telegram-delay-{secs}"))
                        .when(!busy, |c| {
                            c.on_click(cx.listener(move |this, _, _, cx| this.set_delay(secs, cx)))
                        })
                    },
                ));
        let fact = |label: &'static str, value: Option<bool>, on, off| {
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap_4()
                .child(div().text_color(content(0.45)).child(label))
                .child(
                    div()
                        .text_color(content(0.85))
                        .child(model::fact(value, on, off)),
                )
        };
        let mut out = vec![
            div()
                .text_size(px(13.))
                .font_weight(FontWeight::MEDIUM)
                .child(model::connected_line(status))
                .into_any_element(),
            callout(model::OPEN_GROUP_NOTE).into_any_element(),
            row(
                "Forward questions",
                switch("telegram-switch", "Forward questions", enabled, busy).when(!busy, |s| {
                    s.on_click(cx.listener(move |this, _, _, cx| this.set_enabled(!enabled, cx)))
                }),
            )
            .into_any_element(),
        ];
        if !enabled {
            out.push(muted(model::OFF_HINT).into_any_element());
        }
        out.push(row("Send after", chips).into_any_element());
        out.push(
            div()
                .flex()
                .flex_col()
                .gap_1()
                .text_size(px(12.))
                .child(fact("Topics", status.is_forum, "On", "Off"))
                .child(fact(
                    "Bot can manage topics",
                    status.can_manage_topics,
                    "Yes",
                    "No",
                ))
                .into_any_element(),
        );
        if status.can_manage_topics == Some(false) {
            out.push(callout(model::ADMIN_NOTE).into_any_element());
        }
        out
    }

    fn body(&mut self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let mut out: Vec<AnyElement> = vec![
            muted(model::intro(self.state.status.as_ref()))
                .text_size(px(12.))
                .line_height(px(19.))
                .into_any_element(),
        ];
        let Some(status) = self.state.status.clone() else {
            out.push(
                muted("Checking Telegram…")
                    .text_size(px(12.))
                    .into_any_element(),
            );
            return out;
        };
        if !status.configured {
            out.push(self.token_form(false, cx));
            return out;
        }
        let busy = self.state.busy;
        let changing = self.state.changing_token;
        out.push(
            row(
                "Bot",
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(content(0.65))
                            .child(format!("@{}", status.bot_username)),
                    )
                    .when(!changing, |d| {
                        d.child(
                            button("telegram-change-token", "Change token", false, busy)
                                .when(!busy, |b| {
                                    b.on_click(cx.listener(|this, _, _, cx| this.edit_token(cx)))
                                }),
                        )
                    }),
            )
            .border_t_0()
            .pt_0()
            .into_any_element(),
        );
        if changing {
            out.push(self.token_form(true, cx));
        }
        if status.chat_id.is_none() {
            out.push(self.connect_section(cx));
        } else {
            out.extend(self.connected_section(&status, cx));
        }
        if let Some(problem) = status.problem.clone() {
            out.push(
                section()
                    .gap_2()
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(if is_light() {
                                rgb(0xd97706)
                            } else {
                                rgb(0xfcd34d)
                            })
                            .child(problem.clone()),
                    )
                    .when(problem == model::WEBHOOK_PROBLEM, |d| {
                        d.child(div().child(
                            button("telegram-remove-webhook", "Remove webhook", false, busy).when(
                                !busy,
                                |b| {
                                    b.on_click(
                                        cx.listener(|this, _, _, cx| this.remove_webhook(cx)),
                                    )
                                },
                            ),
                        ))
                    })
                    .into_any_element(),
            );
        }
        out.push(if self.state.confirming_disconnect {
            section()
                .gap_2()
                .child(muted(model::DISCONNECT_NOTE))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_3()
                        .child(
                            button("telegram-keep", "Cancel", false, busy).when(!busy, |b| {
                                b.on_click(cx.listener(|this, _, _, cx| {
                                    this.state.confirming_disconnect = false;
                                    cx.notify();
                                }))
                            }),
                        )
                        .child(
                            button("telegram-disconnect-confirm", "Disconnect", true, busy)
                                .when(!busy, |b| {
                                    b.on_click(cx.listener(|this, _, _, cx| this.disconnect(cx)))
                                }),
                        ),
                )
                .into_any_element()
        } else {
            section()
                .child(div().child(
                    button("telegram-disconnect", "Disconnect", true, busy).when(!busy, |b| {
                        b.on_click(cx.listener(|this, _, _, cx| this.ask_disconnect(cx)))
                    }),
                ))
                .into_any_element()
        });
        out
    }
}

impl Render for TelegramDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut body = self.body(cx);
        if let Some(error) = self.state.error.clone() {
            body.push(alert("telegram-error", error).into_any_element());
        }
        let height = window.viewport_size().height.as_f32() - 34.;
        div()
            .id("telegram-dialog")
            .debug_selector(|| "telegram-dialog".into())
            .role(Role::Dialog)
            .aria_label("Telegram")
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
            .child(modal_header(
                "Telegram",
                format!("Answer questions from {} in a Telegram group", self.name),
                icon_button("telegram-close", "Close Telegram settings", "close")
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(Event::Close))),
            ))
            .child(
                div()
                    .id("telegram-dialog-body")
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

pub(crate) struct LinkTopicDialog {
    project: String,
    session: String,
    title: String,
    bridge: Arc<Bridge>,
    state: TopicState,
    focus: FocusHandle,
    _poll: Task<()>,
    _expiry: Option<Task<()>>,
    _copied: Option<Task<()>>,
}

impl EventEmitter<TopicEvent> for LinkTopicDialog {}

impl Focusable for LinkTopicDialog {
    fn focus_handle(&self, _: &gpui::App) -> FocusHandle {
        self.focus.clone()
    }
}

impl LinkTopicDialog {
    /// `session` is the session's id in the Telegram config (the persisted one).
    pub(crate) fn new(
        project: String,
        session: String,
        title: String,
        bridge: Arc<Bridge>,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut this = Self {
            project,
            session,
            title,
            bridge,
            state: TopicState::default(),
            focus: cx.focus_handle(),
            _poll: Task::ready(()),
            _expiry: None,
            _copied: None,
        };
        this.request_code(cx);
        // A successful /link changes the stored binding; the first read is the baseline.
        this._poll = cx.spawn(async move |this, cx| {
            loop {
                let Ok((bridge, project, session)) = this.update(cx, |this, _| {
                    (
                        this.bridge.clone(),
                        this.project.clone(),
                        this.session.clone(),
                    )
                }) else {
                    break;
                };
                let topic = cx
                    .background_executor()
                    .spawn(async move { telegram::session_topic(&bridge, &project, &session) })
                    .await;
                let linked = this.update(cx, |this, cx| {
                    let linked = this.state.topic_read(topic);
                    cx.notify();
                    linked
                });
                if linked.unwrap_or(true) {
                    break;
                }
                cx.background_executor()
                    .timer(Duration::from_millis(model::POLL_MS))
                    .await;
            }
        });
        this
    }

    pub(crate) fn request_code(&mut self, cx: &mut Context<Self>) {
        if !self.state.begin_code() {
            return;
        }
        cx.notify();
        let (bridge, project, session) = (
            self.bridge.clone(),
            self.project.clone(),
            self.session.clone(),
        );
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { telegram::link_code(&bridge, &project, &session) })
                .await;
            let _ = this.update(cx, |this, cx| {
                let ok = result.is_ok();
                this.state.code_done(result);
                if ok {
                    this._expiry = Some(cx.spawn(async move |this, cx| {
                        cx.background_executor()
                            .timer(Duration::from_millis(model::CODE_TTL_MS))
                            .await;
                        let _ = this.update(cx, |this, cx| {
                            this.state.expire();
                            cx.notify();
                        });
                    }));
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn copy(&mut self, text: String, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        self.state.copied = true;
        self._copied = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(COPIED_MS))
                .await;
            let _ = this.update(cx, |this, cx| {
                this.state.copied = false;
                cx.notify();
            });
        }));
        cx.notify();
    }
}

impl Render for LinkTopicDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let state = &self.state;
        let busy = state.busy;
        let mut body: Vec<AnyElement> = Vec::new();
        if state.linked {
            body.push(
                div()
                    .text_size(px(13.))
                    .font_weight(FontWeight::MEDIUM)
                    .child("Linked")
                    .into_any_element(),
            );
            body.push(
                muted("Questions from this session will be posted in that topic.")
                    .text_size(px(12.))
                    .into_any_element(),
            );
            body.push(
                div()
                    .child(
                        button("telegram-topic-done", "Done", false, false)
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(TopicEvent::Close))),
                    )
                    .into_any_element(),
            );
        } else if let Some(link) = state.link.clone() {
            let command = link.command.clone();
            let mut column = div()
                .flex()
                .flex_col()
                .gap_2()
                .child(
                    div()
                        .text_size(px(12.))
                        .line_height(px(19.))
                        .text_color(content(0.65))
                        .child("Send this in the topic you want to use:"),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(code_box(&link.command).debug_selector(|| "telegram-command".into()))
                        .child(
                            button(
                                "telegram-topic-copy",
                                if state.copied { "Copied" } else { "Copy" },
                                false,
                                false,
                            )
                            .on_click(
                                cx.listener(move |this, _, _, cx| this.copy(command.clone(), cx)),
                            ),
                        ),
                );
            column = if state.expired {
                column.child(
                    div()
                        .flex()
                        .items_center()
                        .gap_3()
                        .child(
                            div()
                                .text_size(px(12.))
                                .text_color(if is_light() {
                                    rgb(0xd97706)
                                } else {
                                    rgb(0xfcd34d)
                                })
                                .child("This code has expired."),
                        )
                        .child(
                            button("telegram-new-code", "New code", false, busy).when(!busy, |b| {
                                b.on_click(cx.listener(|this, _, _, cx| this.request_code(cx)))
                            }),
                        ),
                )
            } else {
                column.child(muted(
                    "Waiting for the command… The code works for 15 minutes.",
                ))
            };
            body.push(column.into_any_element());
        } else if state.creating() {
            body.push(
                muted("Creating a code…")
                    .text_size(px(12.))
                    .into_any_element(),
            );
        }
        if let Some(error) = state.error.clone() {
            body.push(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(alert("telegram-topic-error", error))
                    .when(state.link.is_none() && !state.linked, |d| {
                        d.child(
                            button("telegram-try-again", "Try again", false, busy)
                                .when(!busy, |b| {
                                    b.on_click(cx.listener(|this, _, _, cx| this.request_code(cx)))
                                }),
                        )
                    })
                    .into_any_element(),
            );
        }
        let height = window.viewport_size().height.as_f32() - 34.;
        div()
            .id("telegram-topic-dialog")
            .debug_selector(|| "telegram-topic-dialog".into())
            .role(Role::Dialog)
            .aria_label("Link Telegram topic")
            .track_focus(&self.focus)
            .relative()
            .min_h_0()
            .max_h(px(height))
            .flex()
            .flex_col()
            .text_color(content(1.))
            .on_key_down(cx.listener(|_, event: &KeyDownEvent, window, cx| {
                if event.keystroke.key == "escape" {
                    cx.emit(TopicEvent::Close);
                    window.prevent_default();
                    cx.stop_propagation();
                }
            }))
            .child(modal_header(
                "Link Telegram topic",
                format!("Use an existing topic for {}", self.title),
                icon_button("telegram-topic-close", "Close link dialog", "close")
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(TopicEvent::Close))),
            ))
            .child(
                div()
                    .id("telegram-topic-body")
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
