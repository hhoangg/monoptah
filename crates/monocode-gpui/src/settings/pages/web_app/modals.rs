//! The Web app dialogs, drawn by the shell over the whole window: the pairing
//! code, "Verify on another device", Unpair and Revoke confirmations.
use super::{mm_ss, verify_link};
use crate::{
    modal,
    settings::{
        SettingsEvent, SettingsView,
        pages::mobile::{qr, qr_canvas},
    },
    theme::*,
};
use gpui::{AnyElement, Context, FontWeight, Window, div, prelude::*, px, rgb};
use std::time::{SystemTime, UNIX_EPOCH};

const QR_SIDE: f32 = 384.;
const QR_PADDING: f32 = 14.;
/// Wide enough for the QR beside the codes.
const PAIR_WIDTH: f32 = 720.;
/// Below QR plus the codes, the codes go under the QR.
const NARROW_WIDTH: f32 = 260.;

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

impl SettingsView {
    pub(in crate::settings) fn web_overlay_open(&self) -> bool {
        self.ctx.web_pairing.is_some()
            || self.web_app.confirm_unpair
            || self.web_app.verify
            || self.web_app.revoking.is_some()
            || self.web_app.confirm.is_some()
    }

    /// The innermost open Web app dialog, over the whole window.
    pub(in crate::settings) fn web_overlay(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let weak = cx.weak_entity();
        let dismiss = move |_: &mut Window, cx: &mut gpui::App| {
            let _ = weak.update(cx, |this, cx| {
                this.dismiss_web_app(cx);
            });
        };
        if let Some(card) = self.confirm_card(window, cx) {
            return Some(modal::overlay("web-confirm", card, dismiss).into_any_element());
        }
        if let Some((id, name)) = self.web_app.revoking.clone() {
            return Some(
                modal::overlay("web-revoke", self.revoke_card(id, name, cx), dismiss)
                    .into_any_element(),
            );
        }
        if self.web_app.confirm_unpair {
            return Some(
                modal::overlay("web-unpair", self.unpair_card(cx), dismiss).into_any_element(),
            );
        }
        if self.web_app.verify {
            let link = self.ctx.web.as_ref().and_then(verify_link)?;
            return Some(
                modal::overlay("web-verify", self.verify_card(link, window, cx), dismiss)
                    .into_any_element(),
            );
        }
        let ticket = self.ctx.web_pairing.clone()?;
        Some(
            modal::overlay("web-pair", self.pair_card(ticket, window, cx), dismiss)
                .into_any_element(),
        )
    }

    fn revoke_card(&self, id: String, name: String, cx: &mut Context<Self>) -> AnyElement {
        modal::card()
            .id("web-revoke-dialog")
            .debug_selector(|| "web-revoke-dialog".into())
            .role(gpui::Role::Dialog)
            .aria_label("Revoke a web device")
            .child(
                modal::card_body()
                    .child(modal::title(format!("Revoke {name}?")))
                    .child(div().mt(px(8.)).child(modal::body(
                        "It disconnects right away and can\u{2019}t reach this desktop again until you pair it and confirm it here.",
                    )))
                    .child(
                        modal::actions()
                            .child(
                                modal::ghost_button("web-revoke-cancel", "Cancel")
                                    .debug_selector(|| "web-revoke-cancel".into())
                                    .cursor_pointer()
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.web_app.revoking = None;
                                        cx.notify();
                                    })),
                            )
                            .child(
                                modal::danger_button("web-revoke-confirm", "Revoke")
                                    .debug_selector(|| "web-revoke-confirm".into())
                                    .cursor_pointer()
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.web_app.revoking = None;
                                        cx.emit(SettingsEvent::WebRevokeDevice(id.clone()));
                                        cx.notify();
                                    })),
                            ),
                    ),
            )
            .into_any_element()
    }

    fn unpair_card(&self, cx: &mut Context<Self>) -> AnyElement {
        modal::card()
            .id("web-unpair-dialog")
            .debug_selector(|| "web-unpair-dialog".into())
            .role(gpui::Role::Dialog)
            .aria_label("Unpair from the web app")
            .child(
                modal::card_body()
                    .child(modal::title("Unpair from the web app?"))
                    .child(div().mt(px(8.)).child(modal::body(
                        "This revokes the desktop token on the web server and removes every web device. Browsers have to pair again.",
                    )))
                    .child(
                        modal::actions()
                            .child(
                                modal::ghost_button("web-unpair-cancel", "Cancel")
                                    .debug_selector(|| "web-unpair-cancel".into())
                                    .cursor_pointer()
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.web_app.confirm_unpair = false;
                                        cx.notify();
                                    })),
                            )
                            .child(
                                modal::danger_button("web-unpair-confirm", "Unpair")
                                    .debug_selector(|| "web-unpair-confirm".into())
                                    .cursor_pointer()
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.web_app.confirm_unpair = false;
                                        cx.emit(SettingsEvent::WebUnpair);
                                        cx.notify();
                                    })),
                            ),
                    ),
            )
            .into_any_element()
    }

    /// The QR tile for `text`, cached by its exact text.
    fn web_qr_tile(&mut self, text: &str, side: f32, id: &'static str, dim: bool) -> AnyElement {
        let cached = match &self.web_app.qr {
            Some((cached, qr)) if cached == text => Some(qr.clone()),
            _ => None,
        };
        let code = cached.or_else(|| {
            let code = std::sync::Arc::new(qr(text)?);
            self.web_app.qr = Some((text.to_owned(), code.clone()));
            Some(code)
        });
        div()
            .id(id)
            .debug_selector(move || id.into())
            .role(gpui::Role::Image)
            .aria_label("QR code with the link below")
            .size(px(side))
            .flex_shrink_0()
            .overflow_hidden()
            .rounded_2xl()
            .bg(rgb(0xffffff))
            .p(px(QR_PADDING))
            .when_some(code, |d, code| {
                d.child(
                    div()
                        .size_full()
                        .when(dim, |d| d.opacity(0.15))
                        .child(qr_canvas(code)),
                )
            })
            .into_any_element()
    }

    fn link_actions(&self, link: String, cx: &mut Context<Self>) -> AnyElement {
        let (open, copy) = (link.clone(), link);
        div()
            .flex()
            .flex_wrap()
            .gap_2()
            .child(
                crate::settings::widgets::secondary_button(
                    "web-open-browser",
                    "Open in browser",
                    false,
                    false,
                )
                .debug_selector(|| "web-open-browser".into())
                .child("Open in browser")
                .on_click(cx.listener(move |_, _, _, cx| cx.open_url(&open))),
            )
            .child(
                crate::settings::widgets::secondary_button(
                    "web-copy-link",
                    if self.web_app.copied {
                        "Copied"
                    } else {
                        "Copy link"
                    },
                    false,
                    false,
                )
                .debug_selector(|| "web-copy-link".into())
                .child(if self.web_app.copied {
                    "Copied"
                } else {
                    "Copy link"
                })
                .on_click(cx.listener(move |this, _, _, cx| this.web_copy(copy.clone(), cx))),
            )
            .into_any_element()
    }

    fn pair_card(
        &mut self,
        ticket: monocode_core::mobile::web_link::PairTicket,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let left = ticket.expires_at.saturating_sub(unix_now());
        let expired = left == 0;
        let viewport = window.viewport_size();
        let width = PAIR_WIDTH.min((viewport.width.as_f32() - 24.).max(0.));
        let max_height = modal::max_card_height(window);
        let side = QR_SIDE.min((width - 40.).max(0.));
        let narrow = width < NARROW_WIDTH + QR_SIDE;
        let qr = self.web_qr_tile(&ticket.link, side, "web-pair-qr", expired);
        let actions = self.link_actions(ticket.link.clone(), cx);
        let codes = div()
            .min_w(px(220.))
            .flex_1()
            .flex()
            .flex_col()
            .gap_3()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(content(0.5))
                            .child("Your code"),
                    )
                    .child(
                        div()
                            .debug_selector(|| "web-pair-user-code".into())
                            .text_size(px(28.))
                            .line_height(px(34.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(ticket.user_code.clone()),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(content(0.5))
                            .child("Confirmation code"),
                    )
                    .child(
                        div()
                            .debug_selector(|| "web-pair-confirmation".into())
                            .text_size(px(20.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(ticket.confirmation_code.clone()),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .line_height(px(17.))
                            .text_color(content(0.5))
                            .child("Check this code matches the one in your browser"),
                    ),
            )
            .child(
                div()
                    .debug_selector(|| "web-pair-countdown".into())
                    .text_size(px(12.))
                    .text_color(content(0.5))
                    .child(if expired {
                        "Code expired".to_owned()
                    } else {
                        format!("Expires in {}", mm_ss(left))
                    }),
            )
            .child(actions);
        let layout = div()
            .mt_4()
            .flex()
            .gap_5()
            .when(narrow, |d| d.flex_col())
            .when(!narrow, |d| d.flex_wrap())
            .child(qr)
            .child(codes);
        let header = div()
            .child(modal::title("Pair with the web app"))
            .child(div().mt(px(6.)).child(modal::body(
            "Scan the code or open the link in your browser, sign in, and confirm the device here.",
        )));
        let footer = modal::actions().child(
            modal::ghost_button("web-pair-cancel", "Cancel")
                .debug_selector(|| "web-pair-cancel".into())
                .cursor_pointer()
                .on_click(cx.listener(|_, _, _, cx| {
                    cx.emit(SettingsEvent::WebCancelPair);
                })),
        );
        modal::scroll_card("web-pair-scroll", max_height, header, layout, footer)
            .w(px(width))
            .id("web-pair-dialog")
            .debug_selector(|| "web-pair-dialog".into())
            .role(gpui::Role::Dialog)
            .aria_label("Pair with the web app")
            .into_any_element()
    }

    fn verify_card(
        &mut self,
        link: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let viewport = window.viewport_size();
        let width = 520_f32.min((viewport.width.as_f32() - 24.).max(0.));
        let max_height = modal::max_card_height(window);
        let side = QR_SIDE.min((width - 40.).max(0.));
        let qr = self.web_qr_tile(&link, side, "web-verify-qr", false);
        let actions = self.link_actions(link.clone(), cx);
        let header = div()
            .child(modal::title("Verify on another device"))
            .child(div().mt(px(6.)).child(modal::body(
                "Open this link on a second device to check that it shows the same fingerprint as this desktop.",
            )));
        let body = div()
            .child(div().mt_4().flex().justify_center().child(qr))
            .child(
                div()
                    .debug_selector(|| "web-verify-link".into())
                    .mt_3()
                    .text_size(px(11.))
                    .line_height(px(15.))
                    .text_color(content(0.6))
                    .child(link),
            )
            .child(div().mt_3().child(actions));
        let footer = modal::actions().child(
            modal::ghost_button("web-verify-close", "Close")
                .debug_selector(|| "web-verify-close".into())
                .cursor_pointer()
                .on_click(cx.listener(|this, _, _, cx| {
                    this.web_app.verify = false;
                    cx.notify();
                })),
        );
        modal::scroll_card("web-verify-scroll", max_height, header, body, footer)
            .w(px(width))
            .id("web-verify-dialog")
            .debug_selector(|| "web-verify-dialog".into())
            .role(gpui::Role::Dialog)
            .aria_label("Verify this desktop on another device")
            .into_any_element()
    }
}
