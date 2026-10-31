//! Escape must act on exactly one target: the topmost transient surface, or,
//! when there is none, the turn of the conversation in view.
use super::*;
use gpui::{TestAppContext, VisualTestContext, WindowHandle};

fn setup(cx: &mut TestAppContext) -> (WindowHandle<Shell>, i64) {
    cx.update(|cx| {
        crate::input::bind(cx);
        crate::popover::bind(cx);
        crate::image_lightbox::bind(cx);
        super::bind_keys(cx);
        cx.set_reduce_motion(true);
    });
    let (window, _, active) = super::tests::window(cx);
    window
        .update(cx, |shell, _, cx| {
            shell.sessions[&active].update(cx, |s, _| s.busy = true);
        })
        .unwrap();
    cx.run_until_parked();
    (window, active)
}

fn busy(window: WindowHandle<Shell>, id: i64, cx: &mut VisualTestContext) -> bool {
    window
        .update(cx, |shell, _, cx| shell.sessions[&id].read(cx).busy)
        .unwrap()
}

/// Focus as the user would have it: the composer of whatever is in view.
fn focus_view(window: WindowHandle<Shell>, cx: &mut VisualTestContext) {
    window
        .update(cx, |shell, w, cx| shell.focus_active(w, cx))
        .unwrap();
    cx.run_until_parked();
}

fn escape(cx: &mut VisualTestContext) {
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
}

/// Regression guard: with nothing open, Escape stops the session in view.
#[gpui::test]
fn plain_session_view_escape_stops_the_busy_session(cx: &mut TestAppContext) {
    let (window, active) = setup(cx);
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    focus_view(window, visual);
    assert!(busy(window, active, visual));
    escape(visual);
    assert!(!busy(window, active, visual));
}

/// Escape on a session that is not running must not touch it: no stop side
/// effects (status, paused follow-ups) and no other dismissals.
#[gpui::test]
fn idle_session_escape_changes_nothing(cx: &mut TestAppContext) {
    let (window, active) = setup(cx);
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    focus_view(window, visual);
    // A turn that finished, with a queued follow-up that a stop would pause.
    let snapshot = |visual: &mut VisualTestContext| {
        window
            .update(visual, |shell, _, cx| {
                shell.sessions[&active].read(cx).escape_probe()
            })
            .unwrap()
    };
    window
        .update(visual, |shell, _, cx| {
            shell.sessions[&active].update(cx, |s, _| s.finish_for_test("Done"));
        })
        .unwrap();
    let before = snapshot(visual);
    escape(visual);
    assert_eq!(snapshot(visual), before);
    window
        .update(visual, |shell, _, _| {
            assert_eq!(shell.workspace.active, Some(active));
            assert!(shell.settings.is_none() && shell.menu.is_none());
        })
        .unwrap();
}

/// User report 1: a Mono chat in view, a different busy session selected in the sidebar.
#[gpui::test]
fn mono_chat_escape_stops_the_mono_not_the_sidebar_session(cx: &mut TestAppContext) {
    let (window, active) = setup(cx);
    window
        .update(cx, |shell, w, cx| {
            shell
                .mono_manager()
                .update(cx, |m, cx| m.create(Vec::new(), w, cx));
        })
        .unwrap();
    cx.run_until_parked();
    let chat = window
        .update(cx, |shell, w, cx| {
            let monos = shell.mono_manager();
            monos.update(cx, |m, cx| m.dismiss_settings(w, cx));
            let chat = monos.read(cx).chat().expect("the Mono chat is open");
            chat.update(cx, |c, _| c.busy = true);
            chat
        })
        .unwrap();
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    focus_view(window, visual);
    window
        .update(visual, |shell, _, cx| assert!(shell.mono_active(cx)))
        .unwrap();
    escape(visual);
    assert!(!chat.read_with(visual, |c, _| c.busy), "the Mono stops");
    assert!(
        busy(window, active, visual),
        "the sidebar session keeps running"
    );
}

#[gpui::test]
fn mono_chat_escape_with_an_idle_mono_leaves_the_sidebar_session_running(cx: &mut TestAppContext) {
    let (window, active) = setup(cx);
    window
        .update(cx, |shell, w, cx| {
            shell
                .mono_manager()
                .update(cx, |m, cx| m.create(Vec::new(), w, cx));
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |shell, w, cx| {
            shell
                .mono_manager()
                .update(cx, |m, cx| m.dismiss_settings(w, cx));
        })
        .unwrap();
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    focus_view(window, visual);
    escape(visual);
    assert!(busy(window, active, visual));
}

/// User report 2: Settings page.
#[gpui::test]
fn settings_escape_never_stops_the_session(cx: &mut TestAppContext) {
    let (window, active) = setup(cx);
    window
        .update(cx, |shell, w, cx| {
            shell.open_settings(crate::settings::Section::General, w, cx)
        })
        .unwrap();
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    escape(visual);
    window
        .update(visual, |shell, _, _| assert!(shell.settings.is_none()))
        .unwrap();
    assert!(
        busy(window, active, visual),
        "closing settings only closes settings"
    );
    escape(visual);
    assert!(!busy(window, active, visual), "a second Escape is the stop");
}

#[gpui::test]
fn settings_management_dialog_escape_never_stops_the_session(cx: &mut TestAppContext) {
    let (window, active) = setup(cx);
    window
        .update(cx, |shell, w, cx| {
            shell.open_settings(crate::settings::Section::Mcp, w, cx)
        })
        .unwrap();
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    let add = visual.debug_bounds("settings-add-mcp").unwrap();
    visual.simulate_click(add.center(), Default::default());
    visual.run_until_parked();
    assert!(visual.debug_bounds("settings-management-dialog").is_some());
    escape(visual);
    assert!(visual.debug_bounds("settings-management-dialog").is_none());
    window
        .update(visual, |shell, _, _| assert!(shell.settings.is_some()))
        .unwrap();
    assert!(busy(window, active, visual));
    escape(visual);
    window
        .update(visual, |shell, _, _| assert!(shell.settings.is_none()))
        .unwrap();
    assert!(busy(window, active, visual));
}

/// User report 3: dialogs and overlays. Run twice: with the overlay holding
/// focus, and with focus back on the composer while the overlay stays open.
fn overlay_case(
    cx: &mut TestAppContext,
    open: impl Fn(&mut Shell, &mut Window, &mut Context<Shell>),
    is_open: impl Fn(&Shell, &App) -> bool,
) {
    for refocus in [false, true] {
        let (window, active) = setup(cx);
        let visual = &mut VisualTestContext::from_window(window.into(), cx);
        focus_view(window, visual);
        window
            .update(visual, |shell, w, cx| open(shell, w, cx))
            .unwrap();
        visual.run_until_parked();
        if refocus {
            focus_view(window, visual);
        }
        window
            .update(visual, |shell, _, cx| {
                assert!(is_open(shell, cx), "overlay opened (refocus {refocus})")
            })
            .unwrap();
        escape(visual);
        window
            .update(visual, |shell, _, cx| {
                assert!(!is_open(shell, cx), "overlay closed (refocus {refocus})")
            })
            .unwrap();
        assert!(
            busy(window, active, visual),
            "Escape only closed the overlay (refocus {refocus})"
        );
        escape(visual);
        assert!(!busy(window, active, visual), "the next Escape stops");
    }
}

#[gpui::test]
fn delete_dialog_escape_does_not_stop_the_session(cx: &mut TestAppContext) {
    overlay_case(
        cx,
        |shell, w, cx| {
            let id = shell.workspace.active.unwrap();
            shell.open_delete_dialog(vec![id], Vec::new(), w, cx)
        },
        |shell, _| shell.delete_dialog.is_some(),
    );
}

#[gpui::test]
fn link_dialog_escape_does_not_stop_the_session(cx: &mut TestAppContext) {
    overlay_case(
        cx,
        |shell, w, cx| {
            let id = shell.workspace.active.unwrap();
            shell.open_link_dialog(id, w, cx)
        },
        |shell, _| shell.link_dialog.is_some(),
    );
}

#[gpui::test]
fn action_dialog_escape_does_not_stop_the_session(cx: &mut TestAppContext) {
    overlay_case(
        cx,
        |shell, w, cx| shell.open_action_dialog(None, w, cx),
        |shell, _| shell.action_dialog.is_some(),
    );
}

#[gpui::test]
fn quick_open_escape_does_not_stop_the_session(cx: &mut TestAppContext) {
    overlay_case(
        cx,
        |shell, w, cx| shell.open_quick_open(false, w, cx),
        |shell, _| shell.quick_open.is_some(),
    );
}

#[gpui::test]
fn provider_prompt_escape_does_not_stop_the_session(cx: &mut TestAppContext) {
    overlay_case(
        cx,
        |shell, w, cx| {
            shell.installed_providers = Some(monocode_core::provider::Kind::ALL.to_vec());
            shell.open_provider_prompt(None, w, cx)
        },
        |shell, _| shell.provider_prompt.is_some(),
    );
}

#[gpui::test]
fn whats_new_overlay_escape_does_not_stop_the_session(cx: &mut TestAppContext) {
    overlay_case(
        cx,
        |shell, w, cx| {
            shell.open_whats_new("1.0.0".into(), cx);
            let focus = shell
                .notification_state
                .whats_new
                .as_ref()
                .unwrap()
                .2
                .clone();
            w.focus(&focus, cx);
        },
        |shell, _| shell.notification_state.whats_new.is_some(),
    );
}

#[gpui::test]
fn project_background_dialog_escape_does_not_stop_the_session(cx: &mut TestAppContext) {
    overlay_case(
        cx,
        |shell, w, cx| shell.open_background_dialog("/work/monocode".into(), w, cx),
        |shell, _| shell.background_dialog.is_some(),
    );
}

#[gpui::test]
fn resource_dialog_escape_does_not_stop_the_session(cx: &mut TestAppContext) {
    overlay_case(
        cx,
        |shell, w, cx| shell.resources.update(cx, |m, cx| m.open(w, cx)),
        |shell, cx| shell.resources.read(cx).is_open(),
    );
}

#[gpui::test]
fn image_lightbox_escape_does_not_stop_the_session(cx: &mut TestAppContext) {
    overlay_case(
        cx,
        |_, w, cx| {
            w.dispatch_action(
                Box::new(crate::image_lightbox::Open {
                    source: crate::image_lightbox::Source::Path("/missing.png".into()),
                    alt: "x".into(),
                }),
                cx,
            )
        },
        |shell, _| shell.image_lightbox.is_some(),
    );
}

/// Pane focus, not the tab key, decides what Escape stops.
fn split_with_two_running(cx: &mut TestAppContext) -> (WindowHandle<Shell>, i64, i64) {
    let (window, b) = setup(cx);
    let a = window
        .update(cx, |shell, _, cx| {
            let a = *shell.sessions.keys().find(|id| **id != b).unwrap();
            shell.sessions[&a].update(cx, |s, _| s.busy = true);
            shell.workspace.place(b, a, crate::pane_layout::Edge::Right);
            shell.workspace.open(b);
            a
        })
        .unwrap();
    (window, a, b)
}

#[gpui::test]
fn split_tab_escape_stops_the_focused_pane_only(cx: &mut TestAppContext) {
    let (window, a, b) = split_with_two_running(cx);
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    focus_view(window, visual);
    window
        .update(visual, |shell, _, _| {
            assert_eq!(shell.workspace.active, Some(b));
            assert_eq!(shell.workspace.tab_of(a), shell.workspace.tab_of(b));
        })
        .unwrap();
    escape(visual);
    assert!(!busy(window, b, visual), "the focused pane stops");
    assert!(busy(window, a, visual), "its neighbour keeps running");
}

#[gpui::test]
fn focused_mono_pane_in_a_split_escape_stops_that_mono(cx: &mut TestAppContext) {
    let (window, active) = setup(cx);
    window
        .update(cx, |shell, w, cx| {
            shell
                .mono_manager()
                .update(cx, |m, cx| m.create(Vec::new(), w, cx));
        })
        .unwrap();
    cx.run_until_parked();
    let (mono, id) = window
        .update(cx, |shell, w, cx| {
            shell
                .mono_manager()
                .update(cx, |m, cx| m.dismiss_settings(w, cx));
            let tab = shell.active_title_tab(cx).unwrap();
            let TitleTab::Mono { id, .. } = tab.clone() else {
                panic!("expected a Mono")
            };
            let pane = shell
                .workspace
                .place_mono(tab, active, crate::pane_layout::Edge::Right)
                .expect("the Mono joined the split");
            shell.open_mono_pane(pane, w, cx);
            let chat = shell.mono_manager().read(cx).open_chat(&id).unwrap();
            chat.update(cx, |c, _| c.busy = true);
            (chat, id)
        })
        .unwrap();
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    window
        .update(visual, |shell, _, cx| {
            assert!(!shell.mono_active(cx), "the Mono lives in a pane");
            assert_eq!(shell.workspace.mono_pane(&id), shell.workspace.active);
        })
        .unwrap();
    escape(visual);
    assert!(
        !mono.read_with(visual, |c, _| c.busy),
        "the Mono pane stops"
    );
    assert!(
        busy(window, active, visual),
        "the session pane keeps running"
    );
}

#[gpui::test]
fn focused_pane_without_a_session_escape_stops_nothing(cx: &mut TestAppContext) {
    let (window, a, b) = split_with_two_running(cx);
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    // The focused pane is not a conversation (a file pane has no session).
    window
        .update(visual, |shell, _, _| shell.mono_forget_session(b))
        .unwrap();
    focus_view(window, visual);
    escape(visual);
    assert!(
        busy(window, a, visual),
        "the unfocused neighbour is not a target"
    );
}

/// A text selection in the transcript is cleared by Escape, not the turn.
#[gpui::test]
fn transcript_selection_escape_clears_it_and_keeps_the_turn(cx: &mut TestAppContext) {
    let (window, active) = setup(cx);
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    focus_view(window, visual);
    let transcript = window
        .update(visual, |shell, _, cx| {
            let t = shell.sessions[&active].read(cx).transcript.clone();
            t.update(cx, |t, _| t.select_for_test());
            t
        })
        .unwrap();
    assert!(transcript.read_with(visual, |t, _| t.has_selection()));
    escape(visual);
    assert!(!transcript.read_with(visual, |t, _| t.has_selection()));
    assert!(
        busy(window, active, visual),
        "Escape only cleared the selection"
    );
    escape(visual);
    assert!(!busy(window, active, visual), "the next Escape stops");
}

#[gpui::test]
fn provider_sign_in_modal_escape_closes_it_only(cx: &mut TestAppContext) {
    use crate::accounts::Accounts;
    overlay_case(
        cx,
        |shell, _, cx| {
            if crate::accounts::get(cx).is_none() {
                crate::accounts::install(shell.store.clone(), cx);
            }
            let accounts: gpui::Entity<Accounts> = crate::accounts::get(cx).unwrap();
            accounts.update(cx, |a, cx| a.open_modal_signin_for_test(cx));
        },
        |_, cx| crate::accounts::get(cx).is_some_and(|a| a.read(cx).has_modal_auth()),
    );
}

/// The Run dialog of a Mono: Escape closes it and never stops the Mono's turn. The Workspace
/// binding runs before the dialog's own key handling, so the shell must route it.
#[gpui::test]
fn run_dialog_escape_closes_it_and_leaves_the_busy_mono_running(cx: &mut TestAppContext) {
    let (window, _active) = setup(cx);
    window
        .update(cx, |shell, w, cx| {
            shell
                .mono_manager()
                .update(cx, |m, cx| m.create(Vec::new(), w, cx));
        })
        .unwrap();
    cx.run_until_parked();
    let (chat, opened) = window
        .update(cx, |shell, w, cx| {
            let monos = shell.mono_manager();
            monos.update(cx, |m, cx| m.dismiss_settings(w, cx));
            let chat = monos.read(cx).chat().expect("the Mono chat is open");
            chat.update(cx, |c, _| c.busy = true);
            let workflow = monocode_core::mono::workflows::parse::parse_workflow(
                &monocode_core::mono::workflows::ordered::Json::from(&serde_json::json!({
                    "id": "wf-aaaaaaaa", "name": "Nightly", "enabled": true, "createdAt": 1,
                    "inputs": {"area": {"type": "string"}}, "inputValues": {"area": "src"},
                    "nodes": [{"id": "trigger", "type": "manual"}, {"id": "report", "type": "report"}],
                    "edges": [{"from": "trigger", "to": "report"}],
                })),
            )
            .expect("a workflow");
            let opened = monos.update(cx, |m, cx| m.open_gate_for_test(workflow, cx));
            (chat, opened)
        })
        .unwrap();
    assert!(opened, "the dialog opens");
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    focus_view(window, visual);
    let shown = |visual: &mut VisualTestContext| {
        window
            .update(visual, |shell, _, cx| {
                shell.mono_manager().read(cx).wf_gate_shown()
            })
            .unwrap()
    };
    assert!(shown(visual));
    escape(visual);
    assert!(!shown(visual), "Escape closes the dialog");
    assert!(
        chat.read_with(visual, |c, _| c.busy),
        "and does not stop the Mono"
    );
}
