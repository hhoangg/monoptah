//! The web device rows: pending devices, permission requests and the paired
//! devices with their permission and Revoke.
use super::group_fingerprint;
use crate::{
    settings::{
        SettingsEvent, SettingsView,
        widgets::{note, secondary_button},
    },
    theme::*,
};
use gpui::{AnyElement, Context, FontWeight, Role, SharedString, div, prelude::*, px};
use monocode_core::mobile::{DeviceKind, Permission, WebStatus, policy};
use std::time::{SystemTime, UNIX_EPOCH};

fn permission_label(permission: Permission) -> &'static str {
    match permission {
        Permission::Read => "Read-only",
        Permission::Control => "Control",
    }
}

/// "Last seen 5 min ago", or "Never seen" without a time.
pub(in crate::settings) fn last_seen_text(last_seen: Option<u64>, now: u64) -> String {
    let Some(at) = last_seen else {
        return "Never seen".into();
    };
    let ago = now.saturating_sub(at);
    match ago {
        0..=59 => "Last seen just now".into(),
        60..=3599 => format!("Last seen {} min ago", ago / 60),
        3600..=86_399 => format!("Last seen {} h ago", ago / 3600),
        _ => format!("Last seen {} d ago", ago / 86_400),
    }
}

/// "5 min ago", for an access change.
pub(in crate::settings) fn ago_text(at: u64, now: u64) -> String {
    let ago = now.saturating_sub(at);
    match ago {
        0..=59 => "just now".into(),
        60..=3599 => format!("{} min ago", ago / 60),
        3600..=86_399 => format!("{} h ago", ago / 3600),
        _ => format!("{} d ago", ago / 86_400),
    }
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// A bordered list of rows.
fn list(rows: Vec<AnyElement>) -> AnyElement {
    div()
        .mt_3()
        .overflow_hidden()
        .rounded_xl()
        .border_1()
        .border_color(content(0.1))
        .flex()
        .flex_col()
        .children(rows.into_iter().enumerate().map(|(ix, row)| {
            div()
                .when(ix > 0, |d| d.border_t_1().border_color(content(0.1)))
                .child(row)
        }))
        .into_any_element()
}

fn row_base() -> gpui::Div {
    div()
        .flex()
        .flex_wrap()
        .items_center()
        .gap_3()
        .px_4()
        .py_4()
}

fn heading(text: &'static str) -> AnyElement {
    div()
        .mt_5()
        .text_size(px(13.))
        .font_weight(FontWeight::SEMIBOLD)
        .child(text)
        .into_any_element()
}

fn name_block(name: String, fingerprint: &str, extra: String) -> gpui::Div {
    div()
        .min_w_0()
        .flex_1()
        .child(
            div()
                .truncate()
                .text_size(px(13.))
                .font_weight(FontWeight::MEDIUM)
                .child(name),
        )
        .child(
            div()
                .mt_1()
                .text_size(px(11.))
                .text_color(content(0.5))
                .child(group_fingerprint(fingerprint)),
        )
        .child(
            div()
                .mt_1()
                .text_size(px(12.))
                .text_color(content(0.5))
                .child(extra),
        )
}

impl SettingsView {
    pub(super) fn web_app_devices(
        &mut self,
        status: &WebStatus,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let mut page = Vec::new();
        if !status.pending.is_empty() {
            page.push(heading("Waiting for your confirmation"));
            let rows = status
                .pending
                .iter()
                .map(|device| {
                    let fingerprint = device.fingerprint.clone();
                    let key = fingerprint.chars().take(16).collect::<String>();
                    let (confirm, deny) = (fingerprint.clone(), fingerprint.clone());
                    row_base()
                        .debug_selector({
                            let key = key.clone();
                            move || format!("web-pending-{key}")
                        })
                        .child(name_block(
                            device.name.clone(),
                            &fingerprint,
                            if device.online {
                                "Asks for access, online now".into()
                            } else {
                                "Asks for access, offline".into()
                            },
                        ))
                        .child(
                            secondary_button(
                                SharedString::from(format!("web-confirm-{key}")),
                                "Confirm",
                                false,
                                false,
                            )
                            .debug_selector({
                                let key = key.clone();
                                move || format!("web-confirm-{key}")
                            })
                            .child("Confirm")
                            .on_click(cx.listener(
                                move |this, _, _, cx| this.open_web_confirm(&confirm, cx),
                            )),
                        )
                        .child(
                            secondary_button(
                                SharedString::from(format!("web-deny-{key}")),
                                "Deny",
                                true,
                                false,
                            )
                            .debug_selector({
                                let key = key.clone();
                                move || format!("web-deny-{key}")
                            })
                            .child("Deny")
                            .on_click(cx.listener(
                                move |_, _, _, cx| {
                                    cx.emit(SettingsEvent::WebDenyDevice(deny.clone()));
                                },
                            )),
                        )
                        .into_any_element()
                })
                .collect();
            page.push(list(rows));
        }
        if !status.requests.is_empty() {
            page.push(heading("Asking for more access"));
            let rows = status
                .requests
                .iter()
                .map(|request| {
                    let id = request.device_id.clone();
                    let (approve, deny) = (id.clone(), id.clone());
                    row_base()
                        .debug_selector({
                            let id = id.clone();
                            move || format!("web-request-{id}")
                        })
                        .child(
                            div()
                                .min_w_0()
                                .flex_1()
                                .child(
                                    div()
                                        .truncate()
                                        .text_size(px(13.))
                                        .font_weight(FontWeight::MEDIUM)
                                        .child(request.name.clone()),
                                )
                                .child(
                                    div()
                                        .mt_1()
                                        .text_size(px(12.))
                                        .text_color(content(0.5))
                                        .child(format!(
                                            "Requested: {}",
                                            permission_label(request.requested)
                                        )),
                                ),
                        )
                        .child(
                            secondary_button(
                                SharedString::from(format!("web-approve-{id}")),
                                "Approve",
                                false,
                                false,
                            )
                            .debug_selector({
                                let id = id.clone();
                                move || format!("web-approve-{id}")
                            })
                            .child("Approve")
                            .on_click(cx.listener(
                                move |_, _, _, cx| {
                                    cx.emit(SettingsEvent::WebAnswerRequest {
                                        device_id: approve.clone(),
                                        approve: true,
                                    });
                                },
                            )),
                        )
                        .child(
                            secondary_button(
                                SharedString::from(format!("web-deny-request-{id}")),
                                "Deny",
                                true,
                                false,
                            )
                            .debug_selector({
                                let id = id.clone();
                                move || format!("web-deny-request-{id}")
                            })
                            .child("Deny")
                            .on_click(cx.listener(
                                move |_, _, _, cx| {
                                    cx.emit(SettingsEvent::WebAnswerRequest {
                                        device_id: deny.clone(),
                                        approve: false,
                                    });
                                },
                            )),
                        )
                        .into_any_element()
                })
                .collect();
            page.push(list(rows));
        }
        if !status.access_changes.is_empty() {
            page.push(heading("Recent access changes"));
            let now = unix_now();
            let rows = status
                .access_changes
                .iter()
                .rev()
                .enumerate()
                .map(|(ix, change)| {
                    row_base()
                        .debug_selector(move || format!("web-access-change-{ix}"))
                        .child(
                            div()
                                .min_w_0()
                                .flex_1()
                                .child(
                                    div()
                                        .text_size(px(13.))
                                        .font_weight(FontWeight::MEDIUM)
                                        .child(change.device_name.clone()),
                                )
                                .child(
                                    div()
                                        .mt_1()
                                        .text_size(px(12.))
                                        .text_color(content(0.5))
                                        .child(change.text.clone()),
                                ),
                        )
                        .child(
                            div()
                                .text_size(px(12.))
                                .text_color(content(0.5))
                                .child(ago_text(change.at, now)),
                        )
                        .into_any_element()
                })
                .collect();
            page.push(list(rows));
        }
        page.push(heading("Web devices"));
        let devices: Vec<_> = status
            .devices
            .iter()
            .filter(|device| device.kind == DeviceKind::Web)
            .collect();
        if devices.is_empty() {
            page.push(
                div()
                    .mt_3()
                    .rounded_xl()
                    .border_1()
                    .border_color(content(0.1))
                    .child(note("No browsers are paired yet."))
                    .into_any_element(),
            );
            return page;
        }
        let now = unix_now();
        let rows = devices
            .into_iter()
            .map(|device| {
                let id = device.id.clone();
                let name = device.name.clone();
                let revoke = (id.clone(), name.clone());
                let permission = if device.permission == Permission::Control {
                    "control"
                } else {
                    "read"
                };
                let change_id = id.clone();
                let projects_open = self.web_app.projects_open.as_deref() == Some(id.as_str());
                let projects = projects_open.then(|| self.web_device_projects(device, cx));
                let row = row_base()
                    .debug_selector({
                        let id = id.clone();
                        move || format!("web-device-{id}")
                    })
                    .child(name_block(
                        name,
                        &device.fingerprint,
                        last_seen_text(device.last_seen, now),
                    ))
                    .child(self.segmented(
                        format!("web-perm-{id}"),
                        "Permission",
                        permission,
                        &[("read", "Read-only"), ("control", "Control")],
                        cx,
                        move |_, option, cx| {
                            let permission = if option == "control" {
                                Permission::Control
                            } else {
                                Permission::Read
                            };
                            cx.emit(SettingsEvent::WebSetPermission {
                                device_id: change_id.clone(),
                                permission,
                            });
                        },
                    ))
                    .child(
                        secondary_button(
                            SharedString::from(format!("web-projects-{id}")),
                            "Projects",
                            false,
                            false,
                        )
                        .debug_selector({
                            let id = id.clone();
                            move || format!("web-projects-{id}")
                        })
                        .child("Projects")
                        .on_click({
                            let id = id.clone();
                            cx.listener(move |this, _, _, cx| {
                                this.web_app.projects_open =
                                    (this.web_app.projects_open.as_deref() != Some(id.as_str()))
                                        .then(|| id.clone());
                                cx.notify();
                            })
                        }),
                    )
                    .child({
                        // Off by default, per device, and only for Control: the
                        // desktop alone decides, nothing the server sends turns it on.
                        let control = device.permission == Permission::Control;
                        let allowed = control && policy::terminal_allowed(&id);
                        let label = if allowed {
                            "Web terminal: on"
                        } else {
                            "Web terminal: off"
                        };
                        let toggled = id.clone();
                        secondary_button(
                            SharedString::from(format!("web-terminal-{id}")),
                            label,
                            allowed,
                            !control,
                        )
                        .debug_selector({
                            let id = id.clone();
                            move || format!("web-terminal-{id}")
                        })
                        .child(label)
                        .when(control, |button| {
                            button.on_click(cx.listener(move |_, _, _, cx| {
                                cx.emit(SettingsEvent::WebSetTerminalAllowed {
                                    device_id: toggled.clone(),
                                    on: !allowed,
                                });
                            }))
                        })
                    })
                    .child(
                        secondary_button(
                            SharedString::from(format!("web-revoke-{id}")),
                            "Revoke",
                            true,
                            false,
                        )
                        .debug_selector({
                            let id = id.clone();
                            move || format!("web-revoke-{id}")
                        })
                        .child("Revoke")
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.web_app.revoking = Some(revoke.clone());
                            cx.notify();
                        })),
                    );
                div().child(row).children(projects).into_any_element()
            })
            .collect();
        page.push(list(rows));
        page.push(
            div()
                .mt_2()
                .debug_selector(|| "web-terminal-note".into())
                .child(note(
                    "Web terminal opens a shell on this computer for a browser. It is off for every device until you turn it on here, works only for devices with Control, and each time the browser asks you read it a code from this window. Turning it off, revoking the device or Kill ends the shell at once.",
                ))
                .into_any_element(),
        );
        page
    }

    /// The projects and Monos a web device may see, as two checklists. A
    /// device with no saved selection (`None`) has everything ticked; ticking
    /// everything again goes back to `None`, so it keeps seeing new items.
    fn web_device_projects(
        &self,
        device: &monocode_core::mobile::host::Device,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let projects = self.web_checklist(device, false, cx);
        let monos = self.web_checklist(device, true, cx);
        div()
            .id(SharedString::from(format!(
                "web-project-list-{}",
                device.id
            )))
            .role(Role::Group)
            .aria_label("Projects and Monos this web device can see")
            .flex()
            .flex_col()
            .gap(px(8.))
            .border_t_1()
            .border_color(content(0.1))
            .bg(content(0.03))
            .px_4()
            .py_3()
            .text_size(px(13.))
            .child(projects)
            .child(monos)
            .into_any_element()
    }

    fn web_checklist(
        &self,
        device: &monocode_core::mobile::host::Device,
        monos: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (choices, selected, prefix, title, empty) = if monos {
            (
                self.ctx.web_monos.clone(),
                device.monos.clone(),
                "web-mono",
                "Monos",
                "No Monos to choose from.",
            )
        } else {
            (
                self.ctx.web_projects.clone(),
                device.projects.clone(),
                "web-project",
                "Projects",
                "No projects to choose from.",
            )
        };
        let ticked: Vec<String> = match &selected {
            None => choices.iter().map(|(id, _)| id.clone()).collect(),
            Some(selected) => selected.clone(),
        };
        div()
            .flex()
            .flex_col()
            .gap(px(8.))
            .child(div().text_color(content(0.5)).child(title))
            .when(choices.is_empty(), |d| {
                d.text_color(content(0.5)).child(empty)
            })
            .children(choices.iter().map(|(item, label)| {
                let on = ticked.contains(item);
                let (device_id, item) = (device.id.clone(), item.clone());
                let (all, selected) = (choices.clone(), selected.clone());
                let (projects, device_monos) = (device.projects.clone(), device.monos.clone());
                div()
                    .flex()
                    .items_start()
                    .gap_2()
                    .child(
                        crate::settings::widgets::checkbox(
                            SharedString::from(format!("{prefix}-{device_id}-{item}")),
                            label.clone(),
                            on,
                            false,
                        )
                        .on_click(cx.listener(move |_, _, _, cx| {
                            let next = toggle_selection(&all, &selected, &item, !on);
                            let (projects, monos) = if monos {
                                (projects.clone(), next)
                            } else {
                                (next, device_monos.clone())
                            };
                            cx.emit(SettingsEvent::WebSetSelection {
                                device_id: device_id.clone(),
                                projects,
                                monos,
                            });
                        })),
                    )
                    .child(div().min_w_0().truncate().child(label.clone()))
            }))
            .into_any_element()
    }
}

/// The selection after `toggled` is set to `on`: the catalog items that stay
/// ticked, then the ids the device has that the catalog no longer lists
/// (kept, never dropped by a toggle). `None` (all, including new ones) only
/// when every catalog item is ticked and nothing else is kept.
fn toggle_selection(
    catalog: &[(String, String)],
    selected: &Option<Vec<String>>,
    toggled: &str,
    on: bool,
) -> Option<Vec<String>> {
    let ticked = |id: &str| {
        if id == toggled {
            on
        } else {
            selected
                .as_ref()
                .is_none_or(|list| list.iter().any(|v| v == id))
        }
    };
    let mut next: Vec<String> = catalog
        .iter()
        .map(|(id, _)| id.clone())
        .filter(|id| ticked(id))
        .collect();
    let extras: Vec<String> = selected
        .iter()
        .flatten()
        .filter(|id| !catalog.iter().any(|(known, _)| known == *id))
        .cloned()
        .collect();
    if next.len() == catalog.len() && extras.is_empty() {
        return None;
    }
    next.extend(extras);
    Some(next)
}
