use super::*;
use crate::cloud_start_view::{
    CloudStartAction, StartAction,
    fixtures::{failed, ready, starting},
};
use gpui::{TestAppContext, WindowHandle};
use monocode_core::store::Store;
use std::sync::Mutex;

const BASE: &str = "Ask, build, or describe a change\u{2026}";

fn conversation(cx: &mut TestAppContext) -> WindowHandle<Conversation> {
    cx.update(|cx| cx.set_reduce_motion(true));
    let store = Store::open(Path::new(":memory:")).unwrap();
    let mut summary = store.create("claude", "/work/widgets").unwrap();
    summary.cwd = "remote://pending-1".into();
    summary.has_messages = true;
    cx.update(|cx| {
        input::bind(cx);
        cx.open_window(Default::default(), |_, cx| {
            cx.new(|cx| {
                let mut view = Conversation::new(summary, Arc::new(Mutex::new(store)), cx);
                view.loaded = true;
                view
            })
        })
        .unwrap()
    })
}

#[gpui::test]
fn the_placeholder_follows_the_start_and_is_restored_exactly(cx: &mut TestAppContext) {
    let window = conversation(cx);
    let placeholder = |view: &Conversation, cx: &mut gpui::App| {
        view.input.read(cx).placeholder_text().to_string()
    };
    window
        .update(cx, |view, _, cx| {
            assert_eq!(placeholder(view, cx), BASE);
            view.set_cloud_start(Some(starting(2)), cx);
            assert_eq!(
                placeholder(view, cx),
                format!("{BASE} (sent when the codespace is ready)")
            );
            view.set_cloud_start(Some(failed(2, "boom")), cx);
            assert_eq!(
                placeholder(view, cx),
                format!("{BASE} (sent after a successful retry)")
            );
            view.set_cloud_start(Some(starting(3)), cx);
            view.set_cloud_start(Some(ready(1000)), cx);
            assert_eq!(placeholder(view, cx), BASE);
            view.set_cloud_start(None, cx);
            assert_eq!(placeholder(view, cx), BASE);
        })
        .unwrap();
}

#[gpui::test]
fn a_failed_start_is_not_a_pending_launch_but_a_running_one_is(cx: &mut TestAppContext) {
    let window = conversation(cx);
    window
        .update(cx, |view, _, cx| {
            assert!(!view.cloud_launch_pending());
            view.set_cloud_start(Some(starting(2)), cx);
            assert!(view.cloud_launch_pending());
            view.set_cloud_start(Some(failed(2, "boom")), cx);
            assert!(!view.cloud_launch_pending(), "a failed start is not busy");
            // A prompt that waits for a retry does not make a failed start busy either.
            view.remote_state.app_prompt_waiting = true;
            assert!(!view.cloud_launch_pending());
            view.set_cloud_start(Some(starting(3)), cx);
            assert!(view.cloud_launch_pending());
            view.set_cloud_start(Some(ready(1000)), cx);
            assert!(
                view.cloud_launch_pending(),
                "the waiting prompt is still to go out"
            );
        })
        .unwrap();
}

fn draw(visual: &mut gpui::VisualTestContext) {
    visual.run_until_parked();
    visual.refresh().unwrap();
}

#[gpui::test]
fn the_composer_has_no_progress_bar_or_chip(cx: &mut TestAppContext) {
    let window = conversation(cx);
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    for start in [starting(2), failed(2, "boom"), ready(1000)] {
        window
            .update(visual, |view, _, cx| view.set_cloud_start(Some(start), cx))
            .unwrap();
        draw(visual);
        assert!(visual.debug_bounds("composer-start-chip").is_none());
        assert!(visual.debug_bounds("composer-start-progress").is_none());
    }
}

#[gpui::test]
fn the_start_panel_shows_the_phase_and_the_newest_log_line_and_hides_at_ready(
    cx: &mut TestAppContext,
) {
    use crate::cloud_start_view::fixtures::as_sandbox;
    let window = conversation(cx);
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    draw(visual);
    assert!(visual.debug_bounds("cloud-start-panel").is_none());
    window
        .update(visual, |view, _, cx| {
            view.set_cloud_start(Some(starting(2)), cx)
        })
        .unwrap();
    draw(visual);
    for id in [
        "cloud-start-panel",
        "cloud-start-panel-header",
        "cloud-start-panel-title",
        "cloud-start-panel-phase",
        "cloud-start-panel-log",
    ] {
        assert!(visual.debug_bounds(id).is_some(), "{id}");
    }
    window
        .update(visual, |view, _, _| {
            let start = view.cloud_start.clone().unwrap();
            assert_eq!(start.panel_title().as_deref(), Some("Starting codespace"));
            assert_eq!(start.panel_subtitle(), Some("Building container"));
            assert_eq!(start.latest_log(), Some("Setting up chromium ..."));
        })
        .unwrap();
    // A new log line shows up live.
    window
        .update(visual, |view, _, cx| {
            let mut next = (*starting(2)).clone();
            crate::cloud_start_view::push_log(&mut next.log, "Pulling image".into());
            view.set_cloud_start(Some(Arc::new(next.clone())), cx);
            assert_eq!(next.latest_log(), Some("Pulling image"));
            assert_eq!(
                view.cloud_start.as_ref().unwrap().latest_log(),
                Some("Pulling image")
            );
        })
        .unwrap();
    draw(visual);
    assert!(visual.debug_bounds("cloud-start-panel-log").is_some());
    // The sandbox words.
    window
        .update(visual, |view, _, cx| {
            view.set_cloud_start(Some(as_sandbox(starting(0))), cx);
            let start = view.cloud_start.clone().unwrap();
            assert_eq!(start.panel_title().as_deref(), Some("Starting sandbox"));
            assert_eq!(start.panel_subtitle(), Some("Creating sandbox"));
        })
        .unwrap();
    // Folding the panel keeps the header and drops the log line.
    let header = visual.debug_bounds("cloud-start-panel-header").unwrap();
    visual.simulate_click(header.center(), Default::default());
    draw(visual);
    assert!(visual.debug_bounds("cloud-start-panel-header").is_some());
    assert!(visual.debug_bounds("cloud-start-panel-log").is_none());
    // Failed: the header stays (error tone, failure headline) with the actions in the chat.
    window
        .update(visual, |view, _, cx| {
            view.set_cloud_start(Some(as_sandbox(failed(2, "boom"))), cx);
            let start = view.cloud_start.clone().unwrap();
            assert_eq!(
                start.panel_title().as_deref(),
                Some("Sandbox failed at Build container")
            );
            assert_eq!(start.panel_subtitle(), None);
        })
        .unwrap();
    draw(visual);
    assert!(visual.debug_bounds("cloud-start-panel").is_some());
    // Ready: gone.
    window
        .update(visual, |view, _, cx| {
            view.set_cloud_start(Some(ready(1000)), cx)
        })
        .unwrap();
    draw(visual);
    assert!(visual.debug_bounds("cloud-start-panel").is_none());
}

#[gpui::test]
fn the_transcript_actions_reach_the_shell(cx: &mut TestAppContext) {
    let window = conversation(cx);
    let seen = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let log = seen.clone();
    let view = window.root(cx).unwrap();
    let _subscription = cx.update(|cx| {
        cx.subscribe(&view, move |_, event: &CloudStartAction, _| {
            log.borrow_mut().push(event.action)
        })
    });
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    window
        .update(visual, |view, _, cx| {
            view.set_cloud_start(Some(failed(2, "boom")), cx)
        })
        .unwrap();
    draw(visual);
    let retry = visual.debug_bounds("cloud-start-retry").unwrap();
    visual.simulate_click(retry.center(), Default::default());
    assert_eq!(*seen.borrow(), vec![StartAction::Retry]);
}

#[gpui::test]
fn startup_hides_the_loading_status_and_preserves_slash_commands(cx: &mut TestAppContext) {
    let window = conversation(cx);
    window
        .update(cx, |view, _, cx| {
            view.set_cloud_start(Some(starting(2)), cx);
            view.status = "Loading conversation…".into();
            view.input
                .update(cx, |input, cx| input.set_text("/plan fix tests".into(), cx));
        })
        .unwrap();
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    visual.refresh().unwrap();
    assert!(visual.debug_bounds("provider-status").is_none());
    window
        .update(visual, |view, _, cx| {
            view.send(cx);
            assert_eq!(view.input.read(cx).content, "/plan fix tests");
            assert_eq!(
                view.status,
                "Commands are available once the codespace is ready"
            );
            assert!(view.cloud_queued().is_none());
        })
        .unwrap();
}

#[gpui::test]
fn the_start_texts_of_the_conversation_follow_the_backend(cx: &mut TestAppContext) {
    use crate::cloud_start_view::fixtures::as_sandbox;
    let window = conversation(cx);
    window
        .update(cx, |view, _, cx| {
            view.set_cloud_start(Some(as_sandbox(starting(2))), cx);
            assert_eq!(view.cloud_start_noun(), "sandbox");
            view.input
                .update(cx, |input, cx| input.set_text("/model".into(), cx));
            view.send_follow_up(false, cx);
            assert_eq!(
                view.status,
                "Commands are available once the sandbox is ready"
            );
            view.set_cloud_start(Some(starting(2)), cx);
            assert_eq!(view.cloud_start_noun(), "codespace");
            view.send_follow_up(false, cx);
            assert_eq!(
                view.status,
                "Commands are available once the codespace is ready"
            );
        })
        .unwrap();
}
