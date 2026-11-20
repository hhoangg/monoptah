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
    web_link::{AccessChange, PairTicket, PendingDevice, PermissionRequest},
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
        SettingsEvent::WebSetTerminalAllowed { device_id, on } => {
            format!("terminal:{device_id}:{on}")
        }
        SettingsEvent::WebAnswerRequest { device_id, approve } => {
            format!("answer:{device_id}:{approve}")
        }
        SettingsEvent::WebSetPolicyFlag { flag, on } => format!("policy:{flag:?}:{on}"),
        SettingsEvent::WebConfirmDevice {
            fingerprint,
            permission,
            projects,
            monos,
        } => format!("confirm:{fingerprint}:{permission:?}:{projects:?}:{monos:?}"),
        SettingsEvent::PhoneSetProjects {
            device_id,
            projects,
        } => format!("phone-projects:{device_id}:{projects:?}"),
        SettingsEvent::WebSetSelection {
            device_id,
            projects,
            monos,
        } => format!("web-selection:{device_id}:{projects:?}:{monos:?}"),
        SettingsEvent::MobileRevoke(id) => format!("mobile-revoke:{id}"),
        SettingsEvent::MobileConfigure { enabled, .. } => format!("mobile-configure:{enabled}"),
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
fn the_activity_log_opens_from_the_web_app_page(cx: &mut TestAppContext) {
    use monocode_core::mobile::audit::{Lifecycle, Log, Transport};
    let dir = std::env::temp_dir().join(format!("monoptah-web-activity-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    Log::new(&dir).record_lifecycle(
        Lifecycle::Confirm,
        "dev-1",
        "abcdef0123456789",
        Transport::Web,
        None,
    );
    let window = preview(cx);
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    // Unpaired: the log is still there (it covers phones too).
    assert!(visual.debug_bounds("web-activity").is_some());
    let data_dir = dir.clone();
    set_web(&window, visual, |ctx| {
        ctx.web = Some(paired());
        ctx.data_dir = Some(data_dir);
    });
    assert!(visual.debug_bounds("web-activity").is_some());
    assert!(visual.debug_bounds("web-activity-list").is_none());
    click(visual, "web-activity-toggle");
    assert!(visual.debug_bounds("web-activity-list").is_some());
    let shown = window
        .update(visual, |preview, _, cx| {
            preview.settings.read(cx).web_app.activity.clone()
        })
        .unwrap()
        .expect("the log was read");
    assert_eq!(shown.len(), 1);
    assert_eq!(shown[0]["event"], "confirm");
    click(visual, "web-activity-toggle");
    assert!(visual.debug_bounds("web-activity-list").is_none());
    // Unpairing keeps it open: the lines written by the unpair are the point.
    click(visual, "web-activity-toggle");
    set_web(&window, visual, |ctx| ctx.web = Some(WebStatus::default()));
    assert!(visual.debug_bounds("web-activity-list").is_some());
    let _ = std::fs::remove_dir_all(&dir);
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
fn the_web_terminal_switch_is_per_device_and_only_for_control(cx: &mut TestAppContext) {
    let window = preview(cx);
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    set_web(&window, visual, |ctx| {
        ctx.web = Some(WebStatus {
            devices: vec![
                device("term-control", Permission::Control),
                device("term-read", Permission::Read),
            ],
            ..paired()
        });
    });
    assert!(visual.debug_bounds("web-terminal-note").is_some());
    // Off until the desktop's user says so: a click asks to turn it on.
    click(visual, "web-terminal-term-control");
    assert_eq!(events(&window, visual), ["terminal:term-control:true"]);
    // A read-only device cannot be given a shell: its button does nothing.
    click(visual, "web-terminal-term-read");
    assert_eq!(events(&window, visual), ["terminal:term-control:true"]);
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

fn waiting() -> PendingDevice {
    PendingDevice {
        fingerprint: FP.into(),
        name: "Browser".into(),
        server_device_id: "srv".into(),
        online: true,
    }
}

fn choices(ids: &[&str]) -> Vec<(String, String)> {
    ids.iter()
        .map(|id| (id.to_string(), format!("Name {id}")))
        .collect()
}

/// A paired desktop with one pending device and a catalog of two projects
/// and one Mono.
fn with_pending(window: &WindowHandle<Preview>, visual: &mut VisualTestContext) {
    set_web(window, visual, |ctx| {
        ctx.web = Some(WebStatus {
            pending: vec![waiting()],
            ..paired()
        });
        ctx.web_projects = choices(&["p1", "p2"]);
        ctx.web_monos = choices(&["m1"]);
    });
}

fn draft_state(
    window: &WindowHandle<Preview>,
    visual: &mut VisualTestContext,
) -> Option<(Permission, bool, bool, bool)> {
    window
        .update(visual, |preview, _, cx| {
            preview.settings.read(cx).web_app.confirm.as_ref().map(|d| {
                (
                    d.permission,
                    d.project_ticked("p1"),
                    d.project_ticked("p2"),
                    d.mono_ticked("m1"),
                )
            })
        })
        .unwrap()
}

#[gpui::test]
fn a_pending_rows_confirm_opens_the_dialog_with_the_defaults_and_confirms_nothing(
    cx: &mut TestAppContext,
) {
    let window = preview(cx);
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    with_pending(&window, visual);
    assert!(visual.debug_bounds("web-confirm-dialog").is_none());
    click(visual, "web-confirm-0123456789abcdef");
    for selector in [
        "web-confirm-dialog",
        "web-confirm-name",
        "web-confirm-device-fp",
        "web-confirm-desktop-fp",
        "web-confirm-account",
        "web-confirm-project-p1",
        "web-confirm-project-p2",
        "web-confirm-mono-m1",
        "settings-web-confirm-permission-read",
        "settings-web-confirm-permission-control",
        "web-confirm-ok",
        "web-confirm-deny",
    ] {
        assert!(visual.debug_bounds(selector).is_some(), "{selector}");
    }
    // Every project ticked, the Mono not, Read-only.
    assert_eq!(
        draft_state(&window, visual),
        Some((Permission::Read, true, true, false))
    );
    // Opening the dialog sent nothing.
    assert!(events(&window, visual).is_empty());
    assert_eq!(
        web::group_fingerprint(FP),
        "0123 4567 89ab cdef 0123 4567 89ab cdef"
    );
    click(visual, "web-confirm-ok");
    assert_eq!(
        events(&window, visual),
        [format!("confirm:{FP}:Read:None:Some([])")]
    );
    assert!(visual.debug_bounds("web-confirm-dialog").is_none());
}

#[gpui::test]
fn the_dialog_sends_exactly_what_was_ticked(cx: &mut TestAppContext) {
    let window = preview(cx);
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    with_pending(&window, visual);
    click(visual, "web-confirm-0123456789abcdef");
    click(visual, "settings-web-confirm-permission-control");
    click(visual, "web-confirm-project-p1");
    click(visual, "web-confirm-mono-m1");
    assert_eq!(
        draft_state(&window, visual),
        Some((Permission::Control, false, true, true))
    );
    click(visual, "web-confirm-ok");
    assert_eq!(
        events(&window, visual),
        [format!(
            "confirm:{FP}:Control:Some([\"p2\"]):Some([\"m1\"])"
        )]
    );
}

#[gpui::test]
fn deny_in_the_dialog_emits_deny_and_closes_it(cx: &mut TestAppContext) {
    let window = preview(cx);
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    with_pending(&window, visual);
    click(visual, "web-confirm-0123456789abcdef");
    click(visual, "web-confirm-deny");
    assert_eq!(events(&window, visual), [format!("deny:{FP}")]);
    assert!(visual.debug_bounds("web-confirm-dialog").is_none());
}

#[gpui::test]
fn the_dialog_closes_when_the_device_is_no_longer_waiting(cx: &mut TestAppContext) {
    let window = preview(cx);
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    with_pending(&window, visual);
    click(visual, "web-confirm-0123456789abcdef");
    assert!(visual.debug_bounds("web-confirm-dialog").is_some());
    set_web(&window, visual, |ctx| ctx.web = Some(paired()));
    assert!(visual.debug_bounds("web-confirm-dialog").is_none());
    assert!(events(&window, visual).is_empty());
}

/// A catalog long enough to outgrow any small window: `n` projects `p00..` and `n` Monos `m00..`.
fn many(window: &WindowHandle<Preview>, visual: &mut VisualTestContext, n: usize) {
    set_web(window, visual, |ctx| {
        ctx.web = Some(WebStatus {
            pending: vec![waiting()],
            ..paired()
        });
        let ids = |prefix: &str| -> Vec<(String, String)> {
            (0..n)
                .map(|ix| (format!("{prefix}{ix:02}"), format!("Name {prefix}{ix:02}")))
                .collect()
        };
        ctx.web_projects = ids("p");
        ctx.web_monos = ids("m");
    });
}

fn resize(visual: &mut VisualTestContext, height: f32) {
    visual.simulate_resize(size(px(1200.), px(height)));
    visual.run_until_parked();
    visual.refresh().unwrap();
    visual.run_until_parked();
}

fn bounds_of(visual: &mut VisualTestContext, selector: &'static str) -> Bounds<gpui::Pixels> {
    visual
        .debug_bounds(selector)
        .unwrap_or_else(|| panic!("{selector} is not drawn"))
}

fn wheel(visual: &mut VisualTestContext, at: gpui::Point<gpui::Pixels>, dy: f32) {
    visual.simulate_event(gpui::ScrollWheelEvent {
        position: at,
        delta: gpui::ScrollDelta::Pixels(point(px(0.), px(dy))),
        modifiers: Default::default(),
        touch_phase: gpui::TouchPhase::Moved,
    });
    visual.run_until_parked();
    visual.refresh().unwrap();
    visual.run_until_parked();
}

fn inside(window_height: f32, inner: Bounds<gpui::Pixels>) -> bool {
    inner.top() >= px(0.) && inner.bottom() <= px(window_height) && inner.left() >= px(0.)
}

#[gpui::test]
fn a_long_catalog_keeps_the_dialog_inside_a_small_window_and_scrolls_the_lists(
    cx: &mut TestAppContext,
) {
    cx.update(|cx| cx.set_reduce_motion(true));
    let window = preview(cx);
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    many(&window, visual, 14);
    // Open it while the page is tall enough to reach the row, then shrink the window.
    click(visual, "web-confirm-0123456789abcdef");
    resize(visual, 600.);

    // The whole card fits, so nothing is clipped at either end.
    let card = bounds_of(visual, "web-confirm-dialog");
    assert!(inside(600., card), "{card:?}");
    // The top (title block and both fingerprints) and the bottom (permission and the two buttons)
    // are all on screen.
    for selector in [
        "web-confirm-name",
        "web-confirm-device-fp",
        "web-confirm-desktop-fp",
        "settings-web-confirm-permission-read",
        "settings-web-confirm-permission-control",
        "web-confirm-deny",
        "web-confirm-ok",
    ] {
        let bounds = bounds_of(visual, selector);
        assert!(inside(600., bounds), "{selector} {bounds:?}");
        assert!(
            bounds.top() >= card.top() && bounds.bottom() <= card.bottom(),
            "{selector} {bounds:?} outside {card:?}"
        );
    }
    // The lists are longer than the room they have: the last Mono is cut off until scrolled to.
    let first_mono = bounds_of(visual, "web-confirm-mono-m00");
    let ok = bounds_of(visual, "web-confirm-ok");
    let permission = bounds_of(visual, "settings-web-confirm-permission-read");
    let last_mono = bounds_of(visual, "web-confirm-mono-m13");
    assert!(last_mono.bottom() > permission.top(), "the lists overflow");
    assert!(first_mono.top() > bounds_of(visual, "web-confirm-desktop-fp").bottom());

    // The wheel over the lists moves the Monos and leaves the header and the footer alone.
    let (name, device_fp) = (
        bounds_of(visual, "web-confirm-name"),
        bounds_of(visual, "web-confirm-device-fp"),
    );
    let over_lists = bounds_of(visual, "web-confirm-project-p00").center();
    wheel(visual, over_lists, -120.);
    let moved = bounds_of(visual, "web-confirm-mono-m00");
    assert!(
        f32::from(first_mono.top() - moved.top()) >= 119.,
        "{first_mono:?} then {moved:?}"
    );
    assert_eq!(bounds_of(visual, "web-confirm-name"), name);
    assert_eq!(bounds_of(visual, "web-confirm-device-fp"), device_fp);
    assert_eq!(bounds_of(visual, "web-confirm-ok"), ok);
    assert_eq!(bounds_of(visual, "web-confirm-dialog"), card);

    // At the end of the lists the last Mono sits fully above the pinned permission.
    wheel(visual, over_lists, -10_000.);
    let last_mono = bounds_of(visual, "web-confirm-mono-m13");
    assert!(
        last_mono.bottom() <= permission.top() && last_mono.top() >= name.bottom(),
        "{last_mono:?} vs {permission:?}"
    );
    // A row scrolled into view can be ticked, and Confirm sends the same selection as ever.
    click(visual, "web-confirm-mono-m13");
    click(visual, "web-confirm-ok");
    assert_eq!(
        events(&window, visual),
        [format!("confirm:{FP}:Read:None:Some([\"m13\"])")]
    );
}

#[gpui::test]
fn the_dialog_fits_down_to_a_very_small_window_with_the_buttons_reachable(cx: &mut TestAppContext) {
    cx.update(|cx| cx.set_reduce_motion(true));
    let window = preview(cx);
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    many(&window, visual, 40);
    click(visual, "web-confirm-0123456789abcdef");
    for height in [800., 620., 560.] {
        resize(visual, height);
        let card = bounds_of(visual, "web-confirm-dialog");
        assert!(inside(height, card), "{height}: {card:?}");
        for selector in ["web-confirm-deny", "web-confirm-ok"] {
            let bounds = bounds_of(visual, selector);
            assert!(inside(height, bounds), "{height}: {selector} {bounds:?}");
        }
    }
    click(visual, "web-confirm-deny");
    assert_eq!(events(&window, visual), [format!("deny:{FP}")]);
}

#[gpui::test]
fn a_short_catalog_keeps_the_dialog_as_tall_as_its_content_and_centred(cx: &mut TestAppContext) {
    cx.update(|cx| cx.set_reduce_motion(true));
    let window = preview(cx);
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    with_pending(&window, visual);
    click(visual, "web-confirm-0123456789abcdef");
    resize(visual, 900.);
    let card = bounds_of(visual, "web-confirm-dialog");
    assert!(inside(900., card), "{card:?}");
    // Not stretched: far shorter than the window, centred in it, rows in their natural place.
    assert!(card.size.height < px(600.), "{card:?}");
    let gap = (card.top() - (px(900.) - card.bottom())).abs();
    assert!(gap < px(1.), "centred: {card:?}");
    let p1 = bounds_of(visual, "web-confirm-project-p1");
    let p2 = bounds_of(visual, "web-confirm-project-p2");
    let mono = bounds_of(visual, "web-confirm-mono-m1");
    // Rows keep their 6px rhythm and the permission follows the last list.
    assert!(p2.top() > p1.top() && mono.top() > p2.bottom());
    assert!(
        bounds_of(visual, "settings-web-confirm-permission-read").top() > mono.bottom(),
        "the permission follows the lists"
    );
    // Wheeling a list that fits moves nothing.
    wheel(visual, p1.center(), -100.);
    assert_eq!(bounds_of(visual, "web-confirm-project-p1"), p1);
    assert_eq!(bounds_of(visual, "web-confirm-dialog"), card);
}

#[gpui::test]
fn the_pairing_and_verify_dialogs_keep_their_buttons_on_screen_in_a_small_window(
    cx: &mut TestAppContext,
) {
    cx.update(|cx| cx.set_reduce_motion(true));
    let window = preview(cx);
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    set_web(&window, visual, |ctx| ctx.web = Some(paired()));
    click(visual, "web-verify");
    resize(visual, 520.);
    let card = bounds_of(visual, "web-verify-dialog");
    assert!(inside(520., card), "{card:?}");
    let close = bounds_of(visual, "web-verify-close");
    assert!(inside(520., close), "{close:?}");
    click(visual, "web-verify-close");
    assert!(visual.debug_bounds("web-verify-dialog").is_none());

    set_web(&window, visual, |ctx| ctx.web_pairing = Some(ticket()));
    let card = bounds_of(visual, "web-pair-dialog");
    assert!(inside(520., card), "{card:?}");
    let cancel = bounds_of(visual, "web-pair-cancel");
    assert!(inside(520., cancel), "{cancel:?}");
    click(visual, "web-pair-cancel");
    assert_eq!(events(&window, visual), ["cancel-pair"]);
}

#[test]
fn selection_is_none_for_all_projects_and_some_for_a_narrowed_list() {
    let projects = choices(&["p1", "p2"]);
    let monos = choices(&["m1", "m2"]);
    let mut draft = web::confirm_draft(FP);
    assert_eq!(draft.selection(&projects, &monos), (None, Some(vec![])));
    draft.toggle_project("p2");
    draft.toggle_mono("m2");
    assert_eq!(
        draft.selection(&projects, &monos),
        (Some(vec!["p1".into()]), Some(vec!["m2".into()]))
    );
    draft.toggle_mono("m1");
    // Every Mono ticked is still an explicit list, not "all".
    assert_eq!(
        draft.selection(&projects, &monos).1,
        Some(vec!["m1".into(), "m2".into()])
    );
    // No Mono at all still sends an empty list, never "all".
    assert_eq!(draft.selection(&projects, &[]).1, Some(vec![]));
}

fn phone(projects: Option<Vec<String>>) -> Device {
    Device {
        kind: DeviceKind::Phone,
        projects,
        server_device_id: None,
        ..device("ph1", Permission::Control)
    }
}

fn with_phone(
    window: &WindowHandle<Preview>,
    visual: &mut VisualTestContext,
    projects: Option<Vec<String>>,
) {
    set_web(window, visual, |ctx| {
        ctx.mobile = Some(monocode_core::mobile::Status {
            devices: vec![phone(projects)],
            ..Default::default()
        });
        ctx.web_projects = choices(&["p1", "p2"]);
    });
}

#[gpui::test]
fn a_phone_with_no_selection_shows_all_projects_and_narrowing_emits_the_rest(
    cx: &mut TestAppContext,
) {
    let window = preview(cx);
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    with_phone(&window, visual, None);
    assert!(visual.debug_bounds("mobile-project-ph1-p1").is_none());
    click(visual, "mobile-projects-ph1");
    click(visual, "mobile-project-ph1-p1");
    assert_eq!(
        events(&window, visual),
        ["phone-projects:ph1:Some([\"p2\"])"]
    );
}

#[gpui::test]
fn a_web_device_row_narrows_its_projects_from_the_desktop(cx: &mut TestAppContext) {
    let window = preview(cx);
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    set_web(&window, visual, |ctx| {
        ctx.web = Some(WebStatus {
            devices: vec![device("dev1", Permission::Control)],
            ..paired()
        });
        ctx.web_projects = choices(&["p1", "p2"]);
    });
    assert!(visual.debug_bounds("web-project-dev1-p1").is_none());
    click(visual, "web-projects-dev1");
    // A device with no saved selection has every project ticked.
    click(visual, "web-project-dev1-p1");
    assert_eq!(
        events(&window, visual),
        ["web-selection:dev1:Some([\"p2\"]):None"]
    );
    click(visual, "web-projects-dev1");
    assert!(visual.debug_bounds("web-project-dev1-p1").is_none());
}

#[gpui::test]
fn a_web_device_row_narrows_its_monos_and_keeps_ids_the_catalog_lost(cx: &mut TestAppContext) {
    let window = preview(cx);
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    set_web(&window, visual, |ctx| {
        let mut row = device("dev1", Permission::Control);
        row.projects = Some(vec!["p1".into(), "gone".into()]);
        row.monos = Some(vec!["m1".into(), "m2".into()]);
        ctx.web = Some(WebStatus {
            devices: vec![row],
            ..paired()
        });
        ctx.web_projects = choices(&["p1", "p2"]);
        ctx.web_monos = choices(&["m1", "m2"]);
    });
    click(visual, "web-projects-dev1");
    click(visual, "web-mono-dev1-m1");
    assert_eq!(
        events(&window, visual),
        ["web-selection:dev1:Some([\"p1\", \"gone\"]):Some([\"m2\"])"]
    );
    // Toggling a project keeps the id the catalog no longer lists.
    click(visual, "web-project-dev1-p2");
    assert_eq!(
        events(&window, visual)[1],
        "web-selection:dev1:Some([\"p1\", \"p2\", \"gone\"]):Some([\"m1\", \"m2\"])"
    );
}

#[gpui::test]
fn recent_access_changes_show_only_when_there_are_some(cx: &mut TestAppContext) {
    let window = preview(cx);
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    set_web(&window, visual, |ctx| {
        ctx.web = Some(WebStatus {
            devices: vec![device("dev1", Permission::Control)],
            ..paired()
        });
    });
    assert!(visual.debug_bounds("web-access-change-0").is_none());
    set_web(&window, visual, |ctx| {
        ctx.web = Some(WebStatus {
            devices: vec![device("dev1", Permission::Control)],
            access_changes: vec![
                AccessChange {
                    at: 1,
                    device_id: "dev1".into(),
                    device_name: "Work laptop".into(),
                    text: "Work laptop now has access to Alpha".into(),
                },
                AccessChange {
                    at: 2,
                    device_id: "dev1".into(),
                    device_name: "Work laptop".into(),
                    text: "Work laptop now has access to Beta".into(),
                },
            ],
            ..paired()
        });
    });
    assert!(visual.debug_bounds("web-access-change-0").is_some());
    assert!(visual.debug_bounds("web-access-change-1").is_some());
    assert!(visual.debug_bounds("web-access-change-2").is_none());
}

#[test]
fn an_access_change_reads_as_a_relative_time() {
    assert_eq!(web::devices::ago_text(95, 100), "just now");
    assert_eq!(web::devices::ago_text(0, 600), "10 min ago");
    assert_eq!(web::devices::ago_text(0, 7200), "2 h ago");
    assert_eq!(web::devices::ago_text(0, 200_000), "2 d ago");
}

#[gpui::test]
fn your_phones_lists_only_phones_and_the_last_phone_ignores_web_rows(cx: &mut TestAppContext) {
    let window = preview(cx);
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    set_web(&window, visual, |ctx| {
        ctx.mobile = Some(monocode_core::mobile::Status {
            devices: vec![device("w1", Permission::Control), phone(None)],
            ..Default::default()
        });
    });
    assert!(visual.debug_bounds("mobile-remove-ph1").is_some());
    assert!(visual.debug_bounds("mobile-remove-w1").is_none());
    // The one phone is the last phone though a web row exists: removing it
    // switches mobile access off.
    click(visual, "mobile-remove-ph1");
    click(visual, "mobile-confirm-remove");
    assert_eq!(
        events(&window, visual),
        ["mobile-revoke:ph1", "mobile-configure:false"]
    );
}

#[gpui::test]
fn ticking_every_project_again_goes_back_to_none(cx: &mut TestAppContext) {
    let window = preview(cx);
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    with_phone(&window, visual, Some(vec!["p2".into()]));
    click(visual, "mobile-projects-ph1");
    click(visual, "mobile-project-ph1-p1");
    assert_eq!(events(&window, visual), ["phone-projects:ph1:None"]);
}

#[gpui::test]
fn the_remote_action_toggles_render_off_and_emit_the_new_state(cx: &mut TestAppContext) {
    let window = preview(cx);
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    assert!(visual.debug_bounds("web-policy-rows").is_none());
    set_web(&window, visual, |ctx| ctx.web = Some(paired()));
    assert!(visual.debug_bounds("web-policy-rows").is_some());
    // Off by default.
    window
        .update(visual, |preview, _, cx| {
            let ctx = &preview.settings.read(cx).ctx;
            assert!(!ctx.web_policy.remote_git_push);
            assert!(!ctx.web_policy.remote_cloud_start);
            assert!(!ctx.web_policy.remote_approvals, "approvals default off");
        })
        .unwrap();
    for selector in [
        "settings-toggle-web-policy-git-push",
        "settings-toggle-web-policy-cloud-start",
        "settings-toggle-web-policy-approvals",
    ] {
        click(visual, selector);
    }
    assert_eq!(
        events(&window, visual),
        [
            "policy:GitPush:true",
            "policy:CloudStart:true",
            "policy:Approvals:true"
        ]
    );
}

#[gpui::test]
fn the_approvals_toggle_that_is_on_emits_off(cx: &mut TestAppContext) {
    let window = preview(cx);
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    set_web(&window, visual, |ctx| {
        ctx.web = Some(paired());
        ctx.web_policy.remote_approvals = true;
    });
    click(visual, "settings-toggle-web-policy-approvals");
    assert_eq!(events(&window, visual), ["policy:Approvals:false"]);
}

#[gpui::test]
fn the_content_toggle_defaults_on_and_emits_off(cx: &mut TestAppContext) {
    let window = preview(cx);
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    set_web(&window, visual, |ctx| ctx.web = Some(paired()));
    window
        .update(visual, |preview, _, cx| {
            assert!(preview.settings.read(cx).ctx.web_policy.show_push_content);
        })
        .unwrap();
    click(visual, "settings-toggle-web-policy-push-content");
    assert_eq!(events(&window, visual), ["policy:PushContent:false"]);
}

#[gpui::test]
fn a_toggle_that_is_on_emits_off(cx: &mut TestAppContext) {
    let window = preview(cx);
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    set_web(&window, visual, |ctx| {
        ctx.web = Some(paired());
        ctx.web_policy.remote_cloud_start = true;
    });
    click(visual, "settings-toggle-web-policy-cloud-start");
    assert_eq!(events(&window, visual), ["policy:CloudStart:false"]);
}
