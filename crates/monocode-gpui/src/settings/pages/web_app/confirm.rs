//! The device confirmation dialog: the one place where a browser becomes
//! trusted. It shows what the device claims (its name, untrusted) next to what
//! the desktop computed (both fingerprints), and confirms nothing until the
//! user presses Confirm.
use super::group_fingerprint;
use crate::{
    modal,
    settings::{SettingsEvent, SettingsView, widgets::checkbox},
    theme::*,
};
use gpui::{AnyElement, Context, FontWeight, SharedString, Window, div, prelude::*, px};
use monocode_core::mobile::Permission;
use std::collections::HashSet;

const CARD_WIDTH: f32 = 480.;

/// What the user has changed from the defaults: every project ticked, no
/// Mono ticked, Read-only.
pub(in crate::settings) struct Draft {
    pub(in crate::settings) fingerprint: String,
    pub(in crate::settings) permission: Permission,
    unticked_projects: HashSet<String>,
    ticked_monos: HashSet<String>,
}

impl Draft {
    pub(in crate::settings) fn new(fingerprint: &str) -> Self {
        Self {
            fingerprint: fingerprint.to_owned(),
            permission: Permission::Read,
            unticked_projects: HashSet::new(),
            ticked_monos: HashSet::new(),
        }
    }

    pub(in crate::settings) fn project_ticked(&self, id: &str) -> bool {
        !self.unticked_projects.contains(id)
    }

    pub(in crate::settings) fn mono_ticked(&self, id: &str) -> bool {
        self.ticked_monos.contains(id)
    }

    pub(in crate::settings) fn toggle_project(&mut self, id: &str) {
        if !self.unticked_projects.remove(id) {
            self.unticked_projects.insert(id.to_owned());
        }
    }

    pub(in crate::settings) fn toggle_mono(&mut self, id: &str) {
        if !self.ticked_monos.remove(id) {
            self.ticked_monos.insert(id.to_owned());
        }
    }

    /// The `(projects, monos)` to send for the lists on offer: `None` is
    /// "all of them" (every project ticked), otherwise exactly the ticked ids.
    /// Monos are always exactly the ticked ids, so none ticked is `Some(vec![])`.
    pub(in crate::settings) fn selection(
        &self,
        projects: &[(String, String)],
        monos: &[(String, String)],
    ) -> (Option<Vec<String>>, Option<Vec<String>>) {
        let ticked_projects: Vec<String> = projects
            .iter()
            .filter(|(id, _)| self.project_ticked(id))
            .map(|(id, _)| id.clone())
            .collect();
        let ticked_monos: Vec<String> = monos
            .iter()
            .filter(|(id, _)| self.mono_ticked(id))
            .map(|(id, _)| id.clone())
            .collect();
        (
            (ticked_projects.len() != projects.len()).then_some(ticked_projects),
            // Monos are opt-in: always exactly the ticked ones, never "all",
            // so a Mono made later is not shared.
            Some(ticked_monos),
        )
    }
}

fn section_label(text: &'static str) -> gpui::Div {
    div()
        .mt(px(16.))
        .text_size(px(12.))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(content(0.6))
        .child(text)
}

fn print(selector: &'static str, caption: &'static str, fingerprint: &str) -> AnyElement {
    div()
        .min_w_0()
        .flex_1()
        .child(
            div()
                .text_size(px(11.))
                .text_color(content(0.5))
                .child(caption),
        )
        .child(
            div()
                .debug_selector(move || selector.into())
                .mt(px(2.))
                .text_size(px(13.))
                .line_height(px(19.))
                .font_weight(FontWeight::MEDIUM)
                .child(group_fingerprint(fingerprint)),
        )
        .into_any_element()
}

impl SettingsView {
    /// The dialog for the device being confirmed, or `None` when none is.
    pub(super) fn confirm_card(
        &mut self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let draft = self.web_app.confirm.as_ref()?;
        let web = self.ctx.web.as_ref()?;
        let device = web
            .pending
            .iter()
            .find(|device| device.fingerprint == draft.fingerprint)?;
        let name = monocode_core::mobile::web_link::session::clean_name(&device.name);
        let device_fingerprint = device.fingerprint.clone();
        let own = web.fingerprint.clone().unwrap_or_default();
        let account = web.account.clone();
        let permission = draft.permission;
        let projects = self.ctx.web_projects.clone();
        let monos = self.ctx.web_monos.clone();
        let rows = |list: &[(String, String)], mono: bool| -> Vec<AnyElement> {
            list.iter()
                .map(|(id, label)| {
                    let ticked = if mono {
                        draft.mono_ticked(id)
                    } else {
                        draft.project_ticked(id)
                    };
                    let key = format!("web-confirm-{}-{id}", if mono { "mono" } else { "project" });
                    let id = id.clone();
                    div()
                        .flex()
                        .items_start()
                        .gap_2()
                        .child(
                            checkbox(SharedString::from(key), label.clone(), ticked, false)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    if let Some(draft) = this.web_app.confirm.as_mut() {
                                        if mono {
                                            draft.toggle_mono(&id);
                                        } else {
                                            draft.toggle_project(&id);
                                        }
                                    }
                                    cx.notify();
                                })),
                        )
                        .child(
                            div()
                                .min_w_0()
                                .truncate()
                                .text_size(px(13.))
                                .child(label.clone()),
                        )
                        .into_any_element()
                })
                .collect()
        };
        let project_rows = rows(&projects, false);
        let mono_rows = rows(&monos, true);
        let permission_control = self.segmented(
            "web-confirm-permission",
            "Permission",
            if permission == Permission::Control {
                "control"
            } else {
                "read"
            },
            &[("read", "Read-only"), ("control", "Control")],
            cx,
            |this, option, cx| {
                if let Some(draft) = this.web_app.confirm.as_mut() {
                    draft.permission = if option == "control" {
                        Permission::Control
                    } else {
                        Permission::Read
                    };
                }
                cx.notify();
            },
        );
        let confirm_fingerprint = device_fingerprint.clone();
        let deny_fingerprint = device_fingerprint.clone();
        let header = div()
            .flex()
            .flex_col()
            .child(modal::title("Confirm this device?"))
            .child(
                div()
                    .mt(px(12.))
                    .text_size(px(11.))
                    .text_color(content(0.5))
                    .child("Name sent by the device"),
            )
            .child(
                div()
                    .debug_selector(|| "web-confirm-name".into())
                    .mt(px(2.))
                    .text_size(px(14.))
                    .font_weight(FontWeight::MEDIUM)
                    .child(name),
            )
            .child(
                div()
                    .mt(px(2.))
                    .text_size(px(12.))
                    .text_color(content(0.5))
                    .child("Anyone can pick any name. Trust the fingerprints, not the name."),
            )
            .children(account.map(|account| {
                div()
                    .debug_selector(|| "web-confirm-account".into())
                    .mt(px(10.))
                    .text_size(px(12.))
                    .text_color(content(0.7))
                    .child(format!("Account: {account}"))
            }))
            .child(
                div()
                    .mt(px(16.))
                    .flex()
                    .gap_4()
                    .child(print(
                        "web-confirm-device-fp",
                        "Device",
                        &device_fingerprint,
                    ))
                    .child(print("web-confirm-desktop-fp", "This desktop", &own)),
            )
            .child(
                div()
                    .mt(px(6.))
                    .text_size(px(12.))
                    .line_height(px(18.))
                    .text_color(content(0.6))
                    .child("Compare both with the waiting screen in your browser"),
            );
        // The two lists are the only part that can be long, so they alone scroll.
        let lists = div()
            .debug_selector(|| "web-confirm-lists".into())
            .flex()
            .flex_col()
            .child(section_label("Projects"))
            .child(
                div()
                    .mt(px(6.))
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .children(project_rows),
            )
            .when(!mono_rows.is_empty(), |d| {
                d.child(section_label("Monos")).child(
                    div()
                        .mt(px(6.))
                        .flex()
                        .flex_col()
                        .gap(px(6.))
                        .children(mono_rows),
                )
            });
        // Pinned under the lists: the permission and the way out stay reachable.
        let footer = div()
            .flex()
            .flex_col()
            .child(section_label("Permission"))
            .child(div().mt(px(6.)).child(permission_control))
            .child(
                modal::actions()
                    .child(
                        modal::ghost_button("web-confirm-deny", "Deny")
                            .debug_selector(|| "web-confirm-deny".into())
                            .cursor_pointer()
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.web_app.confirm = None;
                                cx.emit(SettingsEvent::WebDenyDevice(deny_fingerprint.clone()));
                                cx.notify();
                            })),
                    )
                    .child(
                        modal::primary_button("web-confirm-ok", "Confirm")
                            .debug_selector(|| "web-confirm-ok".into())
                            .cursor_pointer()
                            .on_click(cx.listener(move |this, _, _, cx| {
                                let Some(draft) = this.web_app.confirm.take() else {
                                    return;
                                };
                                let (projects, monos) =
                                    draft.selection(&this.ctx.web_projects, &this.ctx.web_monos);
                                cx.emit(SettingsEvent::WebConfirmDevice {
                                    fingerprint: confirm_fingerprint.clone(),
                                    permission: draft.permission,
                                    projects,
                                    monos,
                                });
                                cx.notify();
                            })),
                    ),
            );
        Some(
            modal::scroll_card(
                "web-confirm-scroll",
                modal::max_card_height(window),
                header,
                lists,
                footer,
            )
            .w(px(CARD_WIDTH))
            .id("web-confirm-dialog")
            .debug_selector(|| "web-confirm-dialog".into())
            .role(gpui::Role::Dialog)
            .aria_label("Confirm a web device")
            .into_any_element(),
        )
    }
}
