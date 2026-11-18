//! Settings > Connections > Web app: pairing with the web server, the
//! devices that use it and the requests that wait for the user.
mod confirm;
pub(in crate::settings) mod devices;
mod modals;

use super::mobile::Qr;
use crate::{
    input::{self, Input},
    settings::{
        SettingsEvent, SettingsView,
        widgets::{note, red, secondary_button},
    },
    theme::*,
};
use gpui::{AnyElement, Context, Entity, FontWeight, Subscription, Task, div, prelude::*, px, rgb};
use monocode_core::mobile::{WebState, WebStatus, web_link::relay};
use std::{sync::Arc, time::Instant};

/// Where a fresh install pairs.
pub(in crate::settings) const DEFAULT_HOST: &str = "https://zptah.app";
/// How long Pair waits for the code to arrive before it is pressable again.
const PAIRING_WAIT_SECS: u64 = 20;
const EMERALD: u32 = 0x10b981;

#[cfg(test)]
pub(in crate::settings) fn confirm_draft(fingerprint: &str) -> confirm::Draft {
    confirm::Draft::new(fingerprint)
}

/// What the Web app section remembers between frames.
#[derive(Default)]
pub(in crate::settings) struct WebAppState {
    host: Option<Entity<Input>>,
    pub(in crate::settings) host_error: Option<String>,
    /// Since Pair was pressed, until the code arrives.
    pairing_since: Option<Instant>,
    pub(in crate::settings) confirm_unpair: bool,
    /// The "Verify on another device" dialog is open.
    pub(in crate::settings) verify: bool,
    /// The device whose revoke waits for confirmation, as (id, name).
    pub(in crate::settings) revoking: Option<(String, String)>,
    /// The confirmation dialog of a pending device.
    pub(in crate::settings) confirm: Option<confirm::Draft>,
    pub(in crate::settings) copied: bool,
    copied_reset: Option<Task<()>>,
    /// The QR on show, with the exact text it encodes.
    pub(in crate::settings) qr: Option<(String, Arc<Qr>)>,
    _host_events: Vec<Subscription>,
}

/// Validates a Host URL the way the core connects to it: `https://host[:port]`,
/// plus `http://localhost[:port]` when `allow_insecure_localhost` is set
/// (production passes `cfg!(debug_assertions)`). Returns the address without a
/// trailing slash.
pub(in crate::settings) fn validate_host_url(
    raw: &str,
    allow_insecure_localhost: bool,
) -> Result<String, String> {
    let host = raw.trim();
    relay::relay_url(host, "desktop", allow_insecure_localhost)?;
    Ok(host.trim_end_matches('/').to_owned())
}

/// What the status line says.
pub(in crate::settings) fn status_text(status: &WebStatus) -> String {
    if !status.paired {
        return "Not paired".into();
    }
    match status.state {
        WebState::Connected => match status.account.as_deref() {
            Some(account) => format!("Connected to account \u{201c}{account}\u{201d}"),
            None => "Connected".into(),
        },
        WebState::Connecting => "Connecting".into(),
        WebState::Offline => "Offline".into(),
        WebState::Revoked => "Access revoked".into(),
        WebState::Off => "Paired, the web link is off".into(),
    }
}

/// The link the other device opens to compare this desktop's fingerprint.
pub(in crate::settings) fn verify_link(status: &WebStatus) -> Option<String> {
    let host = status.host.as_deref()?.trim_end_matches('/');
    let id = status.desktop_id.as_deref()?;
    let fingerprint = status.fingerprint.as_deref()?;
    Some(format!("{host}/desktops/{id}/verify#fp={fingerprint}"))
}

/// The first 32 hex characters in 8 groups of 4, to compare by eye.
pub(in crate::settings) fn group_fingerprint(fingerprint: &str) -> String {
    let head: Vec<char> = fingerprint.chars().take(32).collect();
    head.chunks(4)
        .map(|group| group.iter().collect::<String>())
        .collect::<Vec<_>>()
        .join(" ")
}

/// `m:ss` for a countdown.
pub(in crate::settings) fn mm_ss(left: u64) -> String {
    format!("{}:{:02}", left / 60, left % 60)
}

impl SettingsView {
    pub(in crate::settings) fn host_field(&mut self, cx: &mut Context<Self>) -> Entity<Input> {
        if let Some(field) = &self.web_app.host {
            return field.clone();
        }
        let field = cx.new(|cx| {
            let mut input = Input::field("Web host URL", DEFAULT_HOST, content(0.35), cx);
            input.set_font_size(12.);
            input.set_text(DEFAULT_HOST.to_owned(), cx);
            input
        });
        self.web_app._host_events = vec![
            cx.subscribe(&field, |this, _, _: &input::Changed, cx| {
                this.web_app.host_error = None;
                cx.notify();
            }),
            cx.subscribe(&field, |this, _, _: &input::Submitted, cx| {
                this.start_web_pair(cx)
            }),
        ];
        self.web_app.host = Some(field.clone());
        field
    }

    /// Reads the settled context: a code, a notice or a pairing ends the wait.
    pub(in crate::settings) fn web_context_changed(&mut self, _cx: &mut Context<Self>) {
        let paired = self.ctx.web.as_ref().is_some_and(|web| web.paired);
        if self.ctx.web_pairing.is_some() || self.ctx.web_notice.is_some() || paired {
            self.web_app.pairing_since = None;
        }
        if !paired {
            self.web_app.verify = false;
            self.web_app.revoking = None;
        }
        // A device answered elsewhere (denied, confirmed, gone) closes its dialog.
        let waiting = |fingerprint: &str| {
            self.ctx.web.as_ref().is_some_and(|web| {
                web.pending
                    .iter()
                    .any(|device| device.fingerprint == fingerprint)
            })
        };
        if self
            .web_app
            .confirm
            .as_ref()
            .is_some_and(|draft| !waiting(&draft.fingerprint))
        {
            self.web_app.confirm = None;
        }
    }

    /// Leaving the page closes its code, as unmounting the page did.
    pub(in crate::settings) fn leave_web_app(&mut self, cx: &mut Context<Self>) {
        if self.ctx.web_pairing.is_some() {
            cx.emit(SettingsEvent::WebCancelPair);
        }
        self.web_app.pairing_since = None;
        self.web_app.confirm_unpair = false;
        self.web_app.verify = false;
        self.web_app.revoking = None;
        self.web_app.confirm = None;
        self.web_app.host_error = None;
    }

    /// Escape closes the innermost Web app dialog.
    pub(in crate::settings) fn dismiss_web_app(&mut self, cx: &mut Context<Self>) -> bool {
        let state = &mut self.web_app;
        let closed = state.confirm.take().is_some()
            || state.revoking.take().is_some()
            || std::mem::take(&mut state.confirm_unpair)
            || std::mem::take(&mut state.verify);
        if closed {
            cx.notify();
            return true;
        }
        if self.ctx.web_pairing.is_some() {
            cx.emit(SettingsEvent::WebCancelPair);
            return true;
        }
        false
    }

    fn pairing_busy(&self) -> bool {
        self.web_app
            .pairing_since
            .is_some_and(|since| since.elapsed().as_secs() < PAIRING_WAIT_SECS)
    }

    /// Pair: check the address, then ask the shell for a code.
    fn start_web_pair(&mut self, cx: &mut Context<Self>) {
        if self.ctx.web.as_ref().is_none_or(|web| web.paired)
            || self.ctx.web_pairing.is_some()
            || self.pairing_busy()
        {
            return;
        }
        let raw = self.web_app.host.as_ref().map_or_else(
            || DEFAULT_HOST.to_owned(),
            |f| f.read(cx).content.to_string(),
        );
        match validate_host_url(&raw, cfg!(debug_assertions)) {
            Ok(host_url) => {
                self.web_app.host_error = None;
                self.web_app.pairing_since = Some(Instant::now());
                cx.emit(SettingsEvent::WebPair { host_url });
            }
            Err(error) => self.web_app.host_error = Some(error),
        }
        cx.notify();
    }

    /// Confirm on a pending row: opens the dialog with the defaults. Nothing
    /// is confirmed until the user presses Confirm in it.
    pub(in crate::settings) fn open_web_confirm(
        &mut self,
        fingerprint: &str,
        cx: &mut Context<Self>,
    ) {
        let waiting = self.ctx.web.as_ref().is_some_and(|web| {
            web.pending
                .iter()
                .any(|device| device.fingerprint == fingerprint)
        });
        if waiting {
            self.web_app.confirm = Some(confirm::Draft::new(fingerprint));
            cx.notify();
        }
    }

    pub(super) fn web_app_page(&mut self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let mut page = vec![self.web_app_header()];
        let Some(status) = self.ctx.web.clone() else {
            page.push(
                div()
                    .mt_5()
                    .rounded_xl()
                    .border_1()
                    .border_color(content(0.1))
                    .bg(content(0.03))
                    .child(note(
                        "The web app isn\u{2019}t available on this desktop right now.",
                    ))
                    .into_any_element(),
            );
            return page;
        };
        page.push(self.web_app_controls(&status, cx));
        if status.state == WebState::Revoked {
            page.push(banner("web-revoked", "Web app: access revoked, pair again"));
        }
        if let Some(error) = &status.error {
            page.push(banner("web-error", error.clone()));
        }
        if let Some(notice) = self.ctx.web_notice.clone() {
            page.push(banner("web-notice", notice));
        }
        if status.paired {
            page.extend(self.web_app_devices(&status, cx));
        }
        page
    }

    fn web_app_header(&self) -> AnyElement {
        div()
            .relative()
            .mt_10()
            .pt_8()
            .border_t_1()
            .border_color(content(0.1))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .text_size(px(13.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("Web app"),
                    )
                    .child(
                        div()
                            .debug_selector(|| "web-beta".into())
                            .rounded_md()
                            .border_1()
                            .border_color(content(0.1))
                            .bg(content(0.05))
                            .px(px(6.))
                            .py(px(2.))
                            .text_size(px(10.))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(content(0.6))
                            .child("Beta"),
                    ),
            )
            .child(
                div()
                    .mt_1()
                    .text_size(px(12.))
                    .line_height(px(19.5))
                    .text_color(content(0.45))
                    .child(
                        "Use Monoptah from a browser. Your desktop stays the one that runs your work: the browser pairs with it, and you confirm each device here.",
                    ),
            )
            .into_any_element()
    }

    fn web_app_controls(&mut self, status: &WebStatus, cx: &mut Context<Self>) -> AnyElement {
        let paired = status.paired;
        let can_pair = !paired && self.ctx.web_pairing.is_none() && !self.pairing_busy();
        let field = (!paired).then(|| self.host_field(cx));
        let error = self.web_app.host_error.clone();
        let verifiable = verify_link(status).is_some();
        let tone = if status.state == WebState::Connected {
            rgb(EMERALD).into()
        } else {
            content(0.7)
        };
        div()
            .mt_5()
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
                    .child("Host URL")
                    .child(match field {
                        Some(field) => div()
                            .debug_selector(|| "web-host-field".into())
                            .h(px(32.))
                            .rounded_md()
                            .border_1()
                            .border_color(if error.is_some() {
                                red(0.5)
                            } else {
                                content(0.1)
                            })
                            .px_2()
                            .flex()
                            .items_center()
                            .child(field)
                            .into_any_element(),
                        None => div()
                            .debug_selector(|| "web-host-fixed".into())
                            .text_size(px(13.))
                            .text_color(content(1.))
                            .child(status.host.clone().unwrap_or_else(|| DEFAULT_HOST.into()))
                            .into_any_element(),
                    })
                    .children(error.map(|error| {
                        div()
                            .debug_selector(|| "web-host-error".into())
                            .text_color(red(1.))
                            .child(error)
                    })),
            )
            .child(
                div()
                    .debug_selector(|| "web-status".into())
                    .text_size(px(13.))
                    .text_color(tone)
                    .child(status_text(status)),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap_2()
                    .child(
                        secondary_button("web-pair", "Pair", false, !can_pair)
                            .debug_selector(|| "web-pair".into())
                            .child("Pair")
                            .when(can_pair, |d| {
                                d.on_click(cx.listener(|this, _, _, cx| this.start_web_pair(cx)))
                            }),
                    )
                    .when(paired, |d| {
                        d.child(
                            secondary_button("web-unpair", "Unpair", true, false)
                                .debug_selector(|| "web-unpair".into())
                                .child("Unpair")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.web_app.confirm_unpair = true;
                                    cx.notify();
                                })),
                        )
                        .child(
                            secondary_button(
                                "web-verify",
                                "Verify on another device",
                                false,
                                !verifiable,
                            )
                            .debug_selector(|| "web-verify".into())
                            .child("Verify on another device")
                            .when(verifiable, |d| {
                                d.on_click(cx.listener(|this, _, _, cx| {
                                    this.web_app.verify = true;
                                    cx.notify();
                                }))
                            }),
                        )
                    }),
            )
            .child(
                div()
                    .text_size(px(12.))
                    .line_height(px(19.5))
                    .text_color(content(0.45))
                    .child(if paired {
                        "To re-pair, Unpair first, then Pair again."
                    } else {
                        "Pairing shows a code and a link to open in your browser."
                    }),
            )
            .into_any_element()
    }

    pub(in crate::settings) fn web_copy(&mut self, text: String, cx: &mut Context<Self>) {
        cx.write_to_clipboard(gpui::ClipboardItem::new_string(text));
        self.web_app.copied = true;
        self.web_app.copied_reset = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(1500))
                .await;
            let _ = this.update(cx, |this, cx| {
                this.web_app.copied = false;
                cx.notify();
            });
        }));
        cx.notify();
    }
}

/// A red banner for an error or a revoked link.
fn banner(id: &'static str, text: impl Into<gpui::SharedString>) -> AnyElement {
    div()
        .debug_selector(move || id.into())
        .mt_5()
        .rounded_lg()
        .bg(red(0.05))
        .p_3()
        .text_size(px(12.))
        .line_height(px(19.5))
        .text_color(red(1.))
        .child(text.into())
        .into_any_element()
}
