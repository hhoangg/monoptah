use super::*;
use gpui::{TestAppContext, VisualTestContext, WindowHandle};
use monocode_core::telegram::{
    EventSink,
    config::{ProjectConfig, TelegramConfig, config_path, save},
};
use std::{cell::RefCell, path::PathBuf, rc::Rc};

const PROJECT: &str = "/work/alpha";
const SECRET: &str = "123456:ABC-secret-token";

struct NoSink;
impl EventSink for NoSink {
    fn emit_to(&self, _: &str, _: &str, _: serde_json::Value) -> bool {
        false
    }
}

/// A bridge over a throwaway config, never started and pointed at a dead local port: nothing
/// here talks to Telegram.
fn bridge(project: Option<ProjectConfig>) -> (Arc<Bridge>, PathBuf) {
    let dir = std::env::temp_dir().join(format!(
        "monoptah-telegram-dialog-{}",
        monocode_core::mono::new_id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let mut config = TelegramConfig::default();
    if let Some(project) = project {
        config.projects.insert(PROJECT.into(), project);
    }
    save(&config_path(&dir), &config).unwrap();
    (
        telegram::open("http://127.0.0.1:1", &dir, Arc::new(NoSink)),
        dir,
    )
}

fn configured() -> ProjectConfig {
    ProjectConfig {
        token: SECRET.into(),
        bot_username: "alpha_bot".into(),
        bot_id: 7,
        ..Default::default()
    }
}

fn connected() -> ProjectConfig {
    ProjectConfig {
        chat_id: Some(-100123),
        chat_title: Some("Alpha team".into()),
        is_forum: Some(true),
        can_manage_topics: Some(true),
        // Off, so switching it never starts a poller.
        enabled: false,
        ..configured()
    }
}

fn open(
    cx: &mut TestAppContext,
    bridge: &Arc<Bridge>,
) -> (WindowHandle<TelegramDialog>, Rc<RefCell<Vec<&'static str>>>) {
    let bridge = bridge.clone();
    let window = cx.update(|cx| {
        input::bind(cx);
        cx.open_window(Default::default(), |_, cx| {
            cx.new(|cx| TelegramDialog::new(PROJECT.into(), "Alpha".into(), bridge, cx))
        })
        .unwrap()
    });
    let events: Rc<RefCell<Vec<&'static str>>> = Default::default();
    let seen = events.clone();
    window
        .update(cx, |_, _, cx| {
            cx.subscribe(&cx.entity(), move |_, _, event: &Event, _| {
                seen.borrow_mut().push(match event {
                    Event::Close => "close",
                    Event::Changed => "changed",
                })
            })
            .detach()
        })
        .unwrap();
    cx.run_until_parked();
    (window, events)
}

fn read<T>(
    window: &WindowHandle<TelegramDialog>,
    cx: &mut TestAppContext,
    f: impl FnOnce(&TelegramDialog, &gpui::App) -> T,
) -> T {
    window.read_with(cx, |dialog, cx| f(dialog, cx)).unwrap()
}

fn act(
    window: &WindowHandle<TelegramDialog>,
    cx: &mut TestAppContext,
    f: impl FnOnce(&mut TelegramDialog, &mut Context<TelegramDialog>),
) {
    window.update(cx, |dialog, _, cx| f(dialog, cx)).unwrap();
    cx.run_until_parked();
}

fn type_token(window: &WindowHandle<TelegramDialog>, cx: &mut TestAppContext, text: &str) {
    window
        .update(cx, |dialog, _, cx| {
            dialog
                .token
                .update(cx, |input, cx| input.set_text(text.into(), cx))
        })
        .unwrap();
}

#[gpui::test]
fn an_unconfigured_project_asks_for_a_token_and_shows_no_group_controls(cx: &mut TestAppContext) {
    let (bridge, dir) = bridge(None);
    let (window, _) = open(cx, &bridge);
    cx.refresh().unwrap();
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    assert!(visual.debug_bounds("telegram-save-token").is_some());
    assert!(visual.debug_bounds("telegram-botfather").is_some());
    assert!(visual.debug_bounds("telegram-switch").is_none());
    assert!(visual.debug_bounds("telegram-disconnect").is_none());
    std::fs::remove_dir_all(dir).ok();
}

#[gpui::test]
fn a_rejected_token_shows_the_error_and_keeps_what_was_typed(cx: &mut TestAppContext) {
    let (bridge, dir) = bridge(None);
    let (window, events) = open(cx, &bridge);
    type_token(&window, cx, "bad");
    act(&window, cx, |dialog, cx| dialog.save_token(cx));
    read(&window, cx, |dialog, cx| {
        assert!(dialog.state.error.as_deref().is_some_and(|e| !e.is_empty()));
        assert!(!dialog.state.busy);
        assert_eq!(dialog.token.read(cx).content, "bad");
        assert!(!dialog.state.status.as_ref().unwrap().configured);
    });
    // The forwarder is told even when it failed, as the original dialog does.
    assert_eq!(*events.borrow(), ["changed"]);
    std::fs::remove_dir_all(dir).ok();
}

#[gpui::test]
fn the_token_field_hides_what_is_typed(cx: &mut TestAppContext) {
    let (bridge, dir) = bridge(None);
    let (window, _) = open(cx, &bridge);
    type_token(&window, cx, SECRET);
    cx.refresh().unwrap();
    read(&window, cx, |dialog, cx| {
        assert!(dialog.token.read(cx).is_secret());
    });
    std::fs::remove_dir_all(dir).ok();
}

#[gpui::test]
fn a_configured_project_offers_the_connect_flow_and_no_delay(cx: &mut TestAppContext) {
    let (bridge, dir) = bridge(Some(configured()));
    let (window, _) = open(cx, &bridge);
    cx.refresh().unwrap();
    read(&window, cx, |dialog, _| {
        let status = dialog.state.status.as_ref().unwrap();
        assert!(status.configured);
        assert_eq!(status.bot_username, "alpha_bot");
    });
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    assert!(visual.debug_bounds("telegram-connect").is_some());
    assert!(visual.debug_bounds("telegram-change-token").is_some());
    assert!(visual.debug_bounds("telegram-delay-300").is_none());
    std::fs::remove_dir_all(dir).ok();
}

#[gpui::test]
fn a_connected_project_shows_the_switch_and_the_nine_delays(cx: &mut TestAppContext) {
    let (bridge, dir) = bridge(Some(connected()));
    let (window, _) = open(cx, &bridge);
    cx.refresh().unwrap();
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    assert!(visual.debug_bounds("telegram-switch").is_some());
    for chip in [
        "telegram-delay-0",
        "telegram-delay-30",
        "telegram-delay-60",
        "telegram-delay-120",
        "telegram-delay-300",
        "telegram-delay-600",
        "telegram-delay-900",
        "telegram-delay-1800",
        "telegram-delay-3600",
    ] {
        assert!(visual.debug_bounds(chip).is_some(), "{chip}");
    }
    assert!(visual.debug_bounds("telegram-delay-90").is_none());
    std::fs::remove_dir_all(dir).ok();
}

#[gpui::test]
fn a_stored_delay_that_is_not_a_preset_gets_its_own_chip(cx: &mut TestAppContext) {
    let (bridge, dir) = bridge(Some(ProjectConfig {
        delay_secs: 90,
        ..connected()
    }));
    let (window, _) = open(cx, &bridge);
    cx.refresh().unwrap();
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    assert!(visual.debug_bounds("telegram-delay-90").is_some());
    std::fs::remove_dir_all(dir).ok();
}

#[gpui::test]
fn choosing_a_delay_saves_it_and_tells_the_forwarder(cx: &mut TestAppContext) {
    let (bridge, dir) = bridge(Some(connected()));
    let (window, events) = open(cx, &bridge);
    act(&window, cx, |dialog, cx| dialog.set_delay(30, cx));
    read(&window, cx, |dialog, _| {
        assert_eq!(dialog.state.status.as_ref().unwrap().delay_secs, 30);
        assert_eq!(dialog.state.error, None);
    });
    assert_eq!(telegram::status(&bridge, PROJECT).delay_secs, 30);
    assert_eq!(*events.borrow(), ["changed"]);
    // Immediately is zero.
    act(&window, cx, |dialog, cx| dialog.set_delay(0, cx));
    assert_eq!(telegram::status(&bridge, PROJECT).delay_secs, 0);
    std::fs::remove_dir_all(dir).ok();
}

#[gpui::test]
fn a_delay_that_cannot_be_saved_keeps_the_shown_one_and_says_why(cx: &mut TestAppContext) {
    let (bridge, dir) = bridge(Some(connected()));
    let (window, events) = open(cx, &bridge);
    act(&window, cx, |dialog, cx| dialog.set_delay(7200, cx));
    read(&window, cx, |dialog, _| {
        assert_eq!(
            dialog.state.error.as_deref(),
            Some("The delay can be at most 3600 seconds")
        );
        assert_eq!(dialog.state.status.as_ref().unwrap().delay_secs, 300);
    });
    assert_eq!(*events.borrow(), ["changed"]);
    std::fs::remove_dir_all(dir).ok();
}

#[gpui::test]
fn the_switch_turns_forwarding_off_and_the_hint_follows(cx: &mut TestAppContext) {
    let (bridge, dir) = bridge(Some(ProjectConfig {
        enabled: true,
        ..connected()
    }));
    let (window, events) = open(cx, &bridge);
    act(&window, cx, |dialog, cx| dialog.set_enabled(false, cx));
    read(&window, cx, |dialog, _| {
        assert!(!dialog.state.status.as_ref().unwrap().enabled);
    });
    assert!(!telegram::status(&bridge, PROJECT).enabled);
    assert_eq!(*events.borrow(), ["changed"]);
    std::fs::remove_dir_all(dir).ok();
}

#[gpui::test]
fn the_poll_picks_up_a_change_made_elsewhere_and_stops_with_the_dialog(cx: &mut TestAppContext) {
    let (bridge, dir) = bridge(Some(ProjectConfig {
        enabled: true,
        ..connected()
    }));
    let (window, events) = open(cx, &bridge);
    telegram::set_enabled(&bridge, PROJECT, false).unwrap();
    telegram::set_delay(&bridge, PROJECT, 60).unwrap();
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(model::POLL_MS));
    cx.run_until_parked();
    read(&window, cx, |dialog, _| {
        let status = dialog.state.status.as_ref().unwrap();
        assert_eq!(status.delay_secs, 60);
        assert!(!status.enabled);
    });
    // Forwarding going off is a change the forwarder must hear about.
    assert_eq!(*events.borrow(), ["changed"]);
    std::fs::remove_dir_all(dir).ok();
}

#[gpui::test]
fn disconnecting_asks_first_then_clears_the_project(cx: &mut TestAppContext) {
    let (bridge, dir) = bridge(Some(connected()));
    let (window, events) = open(cx, &bridge);
    act(&window, cx, |dialog, cx| dialog.ask_disconnect(cx));
    read(&window, cx, |dialog, _| {
        assert!(dialog.state.confirming_disconnect);
        assert!(dialog.state.status.as_ref().unwrap().configured);
    });
    assert!(telegram::status(&bridge, PROJECT).configured, "nothing yet");
    type_token(&window, cx, SECRET);
    act(&window, cx, |dialog, cx| dialog.disconnect(cx));
    read(&window, cx, |dialog, cx| {
        assert!(!dialog.state.confirming_disconnect);
        assert!(!dialog.state.status.as_ref().unwrap().configured);
        assert_eq!(dialog.token.read(cx).content, "");
    });
    assert!(!telegram::status(&bridge, PROJECT).configured);
    assert!(events.borrow().contains(&"changed"));
    std::fs::remove_dir_all(dir).ok();
}

#[gpui::test]
fn changing_the_token_opens_an_empty_field_and_cancel_forgets_it(cx: &mut TestAppContext) {
    let (bridge, dir) = bridge(Some(connected()));
    let (window, _) = open(cx, &bridge);
    act(&window, cx, |dialog, cx| dialog.edit_token(cx));
    type_token(&window, cx, SECRET);
    cx.refresh().unwrap();
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    assert!(visual.debug_bounds("telegram-cancel-token").is_some());
    act(&window, cx, |dialog, cx| dialog.cancel_token_change(cx));
    read(&window, cx, |dialog, cx| {
        assert!(!dialog.state.changing_token);
        assert_eq!(dialog.token.read(cx).content, "");
    });
    // The connection was never touched.
    assert!(telegram::status(&bridge, PROJECT).chat_id.is_some());
    std::fs::remove_dir_all(dir).ok();
}

#[gpui::test]
fn a_webhook_problem_offers_to_remove_it_and_other_problems_do_not(cx: &mut TestAppContext) {
    for (problem, offered) in [
        ("This bot has a webhook", true),
        ("Another app is reading this bot", false),
    ] {
        let (bridge, dir) = bridge(Some(ProjectConfig {
            problem: Some(problem.into()),
            ..connected()
        }));
        let (window, _) = open(cx, &bridge);
        cx.refresh().unwrap();
        let visual = &mut VisualTestContext::from_window(window.into(), cx);
        assert_eq!(
            visual.debug_bounds("telegram-remove-webhook").is_some(),
            offered,
            "{problem}"
        );
        std::fs::remove_dir_all(dir).ok();
    }
}

#[gpui::test]
fn the_copied_command_reaches_the_clipboard_and_clears_itself(cx: &mut TestAppContext) {
    let (bridge, dir) = bridge(Some(connected()));
    let (window, _) = open(cx, &bridge);
    act(&window, cx, |dialog, cx| {
        dialog.copy("/connect@alpha_bot CODE1".into(), cx)
    });
    assert_eq!(
        cx.read_from_clipboard().and_then(|item| item.text()),
        Some("/connect@alpha_bot CODE1".to_string())
    );
    read(&window, cx, |dialog, _| assert!(dialog.state.copied));
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(COPIED_MS));
    cx.run_until_parked();
    read(&window, cx, |dialog, _| assert!(!dialog.state.copied));
    std::fs::remove_dir_all(dir).ok();
}

fn open_topic(cx: &mut TestAppContext, bridge: &Arc<Bridge>) -> WindowHandle<LinkTopicDialog> {
    let bridge = bridge.clone();
    let window = cx.update(|cx| {
        cx.open_window(Default::default(), |_, cx| {
            cx.new(|cx| {
                LinkTopicDialog::new(PROJECT.into(), "5".into(), "Fix login".into(), bridge, cx)
            })
        })
        .unwrap()
    });
    cx.run_until_parked();
    window
}

fn topic_state<T>(
    window: &WindowHandle<LinkTopicDialog>,
    cx: &mut TestAppContext,
    f: impl FnOnce(&TopicState) -> T,
) -> T {
    window.read_with(cx, |dialog, _| f(&dialog.state)).unwrap()
}

#[gpui::test]
fn the_topic_dialog_shows_the_link_command(cx: &mut TestAppContext) {
    let (bridge, dir) = bridge(Some(connected()));
    let window = open_topic(cx, &bridge);
    let command = topic_state(&window, cx, |state| state.link.clone().unwrap().command);
    assert!(command.starts_with("/link@alpha_bot "), "{command}");
    cx.refresh().unwrap();
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    assert!(visual.debug_bounds("telegram-command").is_some());
    std::fs::remove_dir_all(dir).ok();
}

#[gpui::test]
fn a_code_that_cannot_be_made_shows_the_error_and_offers_another_try(cx: &mut TestAppContext) {
    let (bridge, dir) = bridge(Some(configured()));
    let window = open_topic(cx, &bridge);
    topic_state(&window, cx, |state| {
        assert_eq!(
            state.error.as_deref(),
            Some("Connect a Telegram group first")
        );
        assert!(state.link.is_none());
    });
    cx.refresh().unwrap();
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    assert!(visual.debug_bounds("telegram-try-again").is_some());
    assert!(visual.debug_bounds("telegram-command").is_none());
    std::fs::remove_dir_all(dir).ok();
}

#[gpui::test]
fn the_code_expires_after_fifteen_minutes_and_a_new_one_replaces_it(cx: &mut TestAppContext) {
    let (bridge, dir) = bridge(Some(connected()));
    let window = open_topic(cx, &bridge);
    let first = topic_state(&window, cx, |state| state.link.clone().unwrap().code);
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(model::CODE_TTL_MS));
    cx.run_until_parked();
    topic_state(&window, cx, |state| assert!(state.expired));
    cx.refresh().unwrap();
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    assert!(visual.debug_bounds("telegram-new-code").is_some());
    window
        .update(cx, |dialog, _, cx| dialog.request_code(cx))
        .unwrap();
    cx.run_until_parked();
    topic_state(&window, cx, |state| {
        assert!(!state.expired);
        assert_ne!(state.link.as_ref().unwrap().code, first);
    });
    std::fs::remove_dir_all(dir).ok();
}

#[gpui::test]
fn polling_for_the_link_keeps_the_baseline_and_runs_until_closed(cx: &mut TestAppContext) {
    let (bridge, dir) = bridge(Some(connected()));
    let window = open_topic(cx, &bridge);
    for _ in 0..3 {
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(model::POLL_MS));
        cx.run_until_parked();
    }
    topic_state(&window, cx, |state| assert!(!state.linked));
    std::fs::remove_dir_all(dir).ok();
}
