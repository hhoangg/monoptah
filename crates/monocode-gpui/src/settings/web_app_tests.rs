//! The Settings > Connections > Web app section and its dialogs.
use super::pages::{mobile::qr, web_app as web};
use super::*;
use gpui::{
    Bounds, Render, TestAppContext, VisualTestContext, WindowBounds, WindowHandle, WindowOptions,
    point, size,
};
use monocode_core::mobile::{
    DeviceKind, Permission, WebState, WebStatus,
    host::Device,
    web_link::{PairTicket, PendingDevice, PermissionRequest},
};
use std::{cell::RefCell, rc::Rc};

const FP: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
const LINK: &str = "https://zptah.app/pair/ABCD-EFGH#fp=0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

/// The same placement as Shell: the page is a child and the dialogs belong
/// to the window.
struct Preview {
    settings: Entity<SettingsView>,
    events: Rc<RefCell<Vec<String>>>,
    _changes: Subscription,
    _events: Subscription,
}

impl Render for Preview {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let overlay = self
            .settings
            .update(cx, |view, cx| view.mobile_overlay(window, cx));
        div()
            .relative()
            .size_full()
            .child(self.settings.clone())
            .children(overlay)
    }
}

/// How a Web event reads in the log; other events are not recorded.
fn tag(event: &SettingsEvent) -> Option<String> {
    Some(match event {
        SettingsEvent::WebPair { host_url } => format!("pair:{host_url}"),
        SettingsEvent::WebCancelPair => "cancel-pair".into(),
        SettingsEvent::WebUnpair => "unpair".into(),
        SettingsEvent::WebDenyDevice(fp) => format!("deny:{fp}"),
        SettingsEvent::WebSetPermission {
            device_id,
            permission,
        } => format!("permission:{device_id}:{permission:?}"),
        SettingsEvent::WebRevokeDevice(id) => format!("revoke:{id}"),
        SettingsEvent::WebAnswerRequest { device_id, approve } => {
            format!("answer:{device_id}:{approve}")
        }
        _ => return None,
    })
}

fn preview(cx: &mut TestAppContext) -> WindowHandle<Preview> {
    let window = cx.update(|cx| {
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                    point(px(0.), px(0.)),
                    size(px(1200.), px(2800.)),
                ))),
                ..Default::default()
            },
            |_, cx| {
                cx.new(|cx| {
                    let settings = cx.new(|cx| {
                        let mut view = SettingsView::new(
                            Section::Connections,
                            Preferences::default(),
                            SettingsContext {
                                mobile: Some(monocode_core::mobile::Status::default()),
                                web: Some(WebStatus::default()),
                                ..Default::default()
                            },
                            cx,
                        );
                        view.discovery_task = None;
                        view.discovery_loading = false;
                        view.mobile.poll = None;
                        view
                    });
                    let events = Rc::new(RefCell::new(Vec::new()));
                    let log = events.clone();
                    let changes = cx.observe(&settings, |_, _, cx| cx.notify());
                    let recorded = cx.subscribe(&settings, move |_, _, event, _| {
                        if let Some(tag) = tag(event) {
                            log.borrow_mut().push(tag);
                        }
                    });
                    Preview {
                        settings,
                        events,
                        _changes: changes,
                        _events: recorded,
                    }
                })
            },
        )
        .unwrap()
    });
    cx.refresh().unwrap();
    cx.run_until_parked();
    window
}

fn set_web(
    window: &WindowHandle<Preview>,
    visual: &mut VisualTestContext,
    f: impl FnOnce(&mut SettingsContext),
) {
    window
        .update(visual, |preview, _, cx| {
            preview.settings.update(cx, |view, cx| {
                f(&mut view.ctx);
                view.web_context_changed(cx);
                cx.notify();
            });
        })
        .unwrap();
    visual.run_until_parked();
    visual.refresh().unwrap();
    visual.run_until_parked();
}

fn click(visual: &mut VisualTestContext, selector: &'static str) {
    let bounds = visual
        .debug_bounds(selector)
        .unwrap_or_else(|| panic!("{selector} is not drawn"));
    visual.simulate_click(bounds.center(), Default::default());
    visual.run_until_parked();
    visual.refresh().unwrap();
    visual.run_until_parked();
}

fn events(window: &WindowHandle<Preview>, visual: &mut VisualTestContext) -> Vec<String> {
    window
        .update(visual, |preview, _, _| preview.events.borrow().clone())
        .unwrap()
}

fn ticket() -> PairTicket {
    PairTicket {
        user_code: "ABCD-EFGH".into(),
        confirmation_code: "7391".into(),
        link: LINK.into(),
        fingerprint: FP.into(),
        expires_at: u64::MAX / 2,
        interval: 2,
    }
}

fn paired() -> WebStatus {
    WebStatus {
        enabled: true,
        state: WebState::Connected,
        paired: true,
        account: Some("alice".into()),
        desktop_id: Some("desk-1".into()),
        fingerprint: Some(FP.into()),
        host: Some("https://zptah.app".into()),
        ..Default::default()
    }
}

fn device(id: &str, permission: Permission) -> Device {
    Device {
        id: id.into(),
        name: "Work laptop".into(),
        fingerprint: FP.into(),
        push: None,
        kind: DeviceKind::Web,
        permission,
        projects: None,
        monos: None,
        server_device_id: Some("srv-1".into()),
        created_at: Some(1),
        last_seen: Some(2),
    }
}

#[gpui::test]
fn unpaired_offers_pair_with_the_default_host(cx: &mut TestAppContext) {
    let window = preview(cx);
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    assert!(visual.debug_bounds("web-beta").is_some());
    assert!(visual.debug_bounds("web-host-field").is_some());
    assert!(visual.debug_bounds("web-unpair").is_none());
    click(visual, "web-pair");
    assert_eq!(events(&window, visual), ["pair:https://zptah.app"]);
}

#[gpui::test]
fn a_bad_host_url_is_refused_inline_and_nothing_is_sent(cx: &mut TestAppContext) {
    let window = preview(cx);
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    window
        .update(visual, |preview, window, cx| {
            preview.settings.update(cx, |view, cx| {
                let field = view.host_field(cx);
                field.update(cx, |input, cx| {
                    input.set_text("ftp://example.com".into(), cx)
                });
                let _ = window;
            });
        })
        .unwrap();
    visual.run_until_parked();
    visual.refresh().unwrap();
    click(visual, "web-pair");
    assert!(events(&window, visual).is_empty());
    assert!(visual.debug_bounds("web-host-error").is_some());
}

#[gpui::test]
fn paired_shows_the_account_and_disables_pair(cx: &mut TestAppContext) {
    let window = preview(cx);
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    set_web(&window, visual, |ctx| ctx.web = Some(paired()));
    assert!(visual.debug_bounds("web-unpair").is_some());
    assert!(visual.debug_bounds("web-host-fixed").is_some());
    assert!(visual.debug_bounds("web-host-field").is_none());
    click(visual, "web-pair");
    assert!(events(&window, visual).is_empty());
    assert_eq!(
        web::status_text(&paired()),
        "Connected to account \u{201c}alice\u{201d}"
    );
}

#[gpui::test]
fn unpair_asks_first_and_then_emits(cx: &mut TestAppContext) {
    let window = preview(cx);
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    set_web(&window, visual, |ctx| ctx.web = Some(paired()));
    click(visual, "web-unpair");
    assert!(events(&window, visual).is_empty());
    assert!(visual.debug_bounds("web-unpair-dialog").is_some());
    click(visual, "web-unpair-cancel");
    assert!(visual.debug_bounds("web-unpair-dialog").is_none());
    assert!(events(&window, visual).is_empty());
    click(visual, "web-unpair");
    click(visual, "web-unpair-confirm");
    assert_eq!(events(&window, visual), ["unpair"]);
}

#[gpui::test]
fn the_pairing_dialog_shows_the_codes_and_one_link_everywhere(cx: &mut TestAppContext) {
    let window = preview(cx);
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    set_web(&window, visual, |ctx| ctx.web_pairing = Some(ticket()));
    for selector in [
        "web-pair-dialog",
        "web-pair-qr",
        "web-pair-user-code",
        "web-pair-confirmation",
        "web-pair-countdown",
    ] {
        assert!(visual.debug_bounds(selector).is_some(), "{selector}");
    }
    // The QR encodes exactly the ticket's link.
    let cached = window
        .update(visual, |preview, _, cx| {
            preview.settings.read(cx).web_app.qr.clone()
        })
        .unwrap()
        .expect("a QR was built");
    assert_eq!(cached.0, LINK);
    assert_eq!(*cached.1, qr(LINK).unwrap());
    // Copy link and Open in browser use that same string.
    click(visual, "web-copy-link");
    assert_eq!(
        visual
            .read_from_clipboard()
            .and_then(|item| item.text())
            .as_deref(),
        Some(LINK)
    );
    click(visual, "web-open-browser");
    assert_eq!(visual.opened_url().as_deref(), Some(LINK));
    click(visual, "web-pair-cancel");
    assert_eq!(events(&window, visual), ["cancel-pair"]);
}

#[gpui::test]
fn the_pairing_dialog_closes_when_the_shell_clears_the_ticket(cx: &mut TestAppContext) {
    let window = preview(cx);
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    set_web(&window, visual, |ctx| ctx.web_pairing = Some(ticket()));
    assert!(visual.debug_bounds("web-pair-dialog").is_some());
    set_web(&window, visual, |ctx| {
        ctx.web_pairing = None;
        ctx.web = Some(paired());
    });
    assert!(visual.debug_bounds("web-pair-dialog").is_none());
    assert!(visual.debug_bounds("web-unpair").is_some());
}

#[gpui::test]
fn verify_on_another_device_encodes_the_verify_link(cx: &mut TestAppContext) {
    let window = preview(cx);
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    set_web(&window, visual, |ctx| ctx.web = Some(paired()));
    click(visual, "web-verify");
    assert!(visual.debug_bounds("web-verify-dialog").is_some());
    let expected = format!("https://zptah.app/desktops/desk-1/verify#fp={FP}");
    let cached = window
        .update(visual, |preview, _, cx| {
            preview.settings.read(cx).web_app.qr.clone()
        })
        .unwrap()
        .expect("a QR was built");
    assert_eq!(cached.0, expected);
    click(visual, "web-copy-link");
    assert_eq!(
        visual
            .read_from_clipboard()
            .and_then(|item| item.text())
            .as_deref(),
        Some(expected.as_str())
    );
    click(visual, "web-verify-close");
    assert!(visual.debug_bounds("web-verify-dialog").is_none());
}

#[gpui::test]
fn error_and_revoked_states_show_banners(cx: &mut TestAppContext) {
    let window = preview(cx);
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    assert!(visual.debug_bounds("web-error").is_none());
    set_web(&window, visual, |ctx| {
        ctx.web = Some(WebStatus {
            error: Some("The web server is unreachable".into()),
            ..paired()
        });
    });
    assert!(visual.debug_bounds("web-error").is_some());
    assert!(visual.debug_bounds("web-revoked").is_none());
    set_web(&window, visual, |ctx| {
        ctx.web = Some(WebStatus {
            state: WebState::Revoked,
            ..paired()
        });
    });
    assert!(visual.debug_bounds("web-revoked").is_some());
}

#[gpui::test]
fn a_device_row_changes_permission_and_revokes_after_confirming(cx: &mut TestAppContext) {
    let window = preview(cx);
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    set_web(&window, visual, |ctx| {
        ctx.web = Some(WebStatus {
            devices: vec![device("dev1", Permission::Control)],
            ..paired()
        });
    });
    assert!(visual.debug_bounds("web-device-dev1").is_some());
    click(visual, "settings-web-perm-dev1-read");
    assert_eq!(events(&window, visual), ["permission:dev1:Read"]);
    click(visual, "web-revoke-dev1");
    assert_eq!(
        events(&window, visual).len(),
        1,
        "revoke waits for confirmation"
    );
    assert!(visual.debug_bounds("web-revoke-dialog").is_some());
    click(visual, "web-revoke-confirm");
    assert_eq!(
        events(&window, visual),
        ["permission:dev1:Read", "revoke:dev1"]
    );
    assert!(visual.debug_bounds("web-revoke-dialog").is_none());
}

#[gpui::test]
fn pending_rows_show_without_a_connection_and_deny_emits(cx: &mut TestAppContext) {
    let window = preview(cx);
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    set_web(&window, visual, |ctx| {
        ctx.web = Some(WebStatus {
            state: WebState::Offline,
            pending: vec![PendingDevice {
                fingerprint: FP.into(),
                name: "Browser".into(),
                server_device_id: "srv".into(),
                online: false,
            }],
            ..paired()
        });
    });
    assert!(
        visual
            .debug_bounds("web-pending-0123456789abcdef")
            .is_some()
    );
    assert!(
        visual
            .debug_bounds("web-confirm-0123456789abcdef")
            .is_some()
    );
    click(visual, "web-deny-0123456789abcdef");
    assert_eq!(events(&window, visual), [format!("deny:{FP}")]);
}

#[gpui::test]
fn permission_requests_can_be_approved_or_denied(cx: &mut TestAppContext) {
    let window = preview(cx);
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    set_web(&window, visual, |ctx| {
        ctx.web = Some(WebStatus {
            devices: vec![device("dev1", Permission::Read)],
            requests: vec![PermissionRequest {
                device_id: "dev1".into(),
                server_device_id: "srv-1".into(),
                name: "Work laptop".into(),
                current: Permission::Read,
                requested: Permission::Control,
            }],
            ..paired()
        });
    });
    click(visual, "web-approve-dev1");
    click(visual, "web-deny-request-dev1");
    assert_eq!(
        events(&window, visual),
        ["answer:dev1:true", "answer:dev1:false"]
    );
}

#[test]
fn localhost_is_accepted_in_dev_and_refused_in_release() {
    assert_eq!(
        web::validate_host_url("http://localhost:8787", true).as_deref(),
        Ok("http://localhost:8787")
    );
    assert!(web::validate_host_url("http://localhost:8787", false).is_err());
    assert_eq!(
        web::validate_host_url(" https://zptah.app/ ", false).as_deref(),
        Ok("https://zptah.app")
    );
    assert!(web::validate_host_url("http://example.com", true).is_err());
    assert!(web::validate_host_url("https://zptah.app/path", true).is_err());
    assert!(web::validate_host_url("", true).is_err());
}

#[test]
fn small_formatters_read_as_the_mockup() {
    assert_eq!(
        web::group_fingerprint(FP),
        "0123 4567 89ab cdef 0123 4567 89ab cdef"
    );
    assert_eq!(web::mm_ss(600), "10:00");
    assert_eq!(web::mm_ss(65), "1:05");
    assert_eq!(web::devices::last_seen_text(None, 100), "Never seen");
    assert_eq!(
        web::devices::last_seen_text(Some(90), 100),
        "Last seen just now"
    );
    assert_eq!(
        web::devices::last_seen_text(Some(0), 600),
        "Last seen 10 min ago"
    );
    assert_eq!(web::status_text(&WebStatus::default()), "Not paired");
    assert_eq!(
        web::verify_link(&WebStatus {
            host: Some("https://zptah.app/".into()),
            desktop_id: Some("d".into()),
            fingerprint: Some("ff".into()),
            ..Default::default()
        })
        .as_deref(),
        Some("https://zptah.app/desktops/d/verify#fp=ff")
    );
    assert_eq!(web::verify_link(&WebStatus::default()), None);
}
