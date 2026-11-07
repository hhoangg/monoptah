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
fn the_chip_and_bar_show_while_starting_and_the_chip_turns_red_on_failure(cx: &mut TestAppContext) {
    let window = conversation(cx);
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    let draw = |visual: &mut gpui::VisualTestContext| {
        visual.run_until_parked();
        visual.refresh().unwrap();
    };
    window
        .update(visual, |view, _, cx| {
            view.set_cloud_start(Some(starting(2)), cx)
        })
        .unwrap();
    draw(visual);
    assert!(visual.debug_bounds("composer-start-chip").is_some());
    let bar = visual.debug_bounds("composer-start-progress").unwrap();
    assert_eq!(bar.size.height, gpui::px(2.));
    window
        .update(visual, |view, _, cx| {
            view.set_cloud_start(Some(failed(2, "boom")), cx)
        })
        .unwrap();
    draw(visual);
    assert!(visual.debug_bounds("composer-start-chip").is_some());
    assert!(visual.debug_bounds("composer-start-progress").is_none());
    window
        .update(visual, |view, _, cx| {
            view.set_cloud_start(Some(ready(1000)), cx)
        })
        .unwrap();
    draw(visual);
    assert!(visual.debug_bounds("composer-start-chip").is_none());
    assert!(visual.debug_bounds("composer-start-progress").is_none());
}

#[gpui::test]
fn the_chip_opens_the_panel_and_the_transcript_actions_reach_the_shell(cx: &mut TestAppContext) {
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
    visual.run_until_parked();
    visual.refresh().unwrap();
    let chip = visual.debug_bounds("composer-start-chip").unwrap();
    visual.simulate_click(chip.center(), Default::default());
    let retry = visual.debug_bounds("cloud-start-retry").unwrap();
    visual.simulate_click(retry.center(), Default::default());
    assert_eq!(
        *seen.borrow(),
        vec![StartAction::ShowPanel, StartAction::Retry]
    );
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
