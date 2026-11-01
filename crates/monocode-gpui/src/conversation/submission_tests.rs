use super::*;
use gpui::{TestAppContext, WindowHandle};
use monocode_core::{store::Store, transcript::attachments};
use std::sync::Mutex;

fn conversation(cx: &mut TestAppContext) -> WindowHandle<Conversation> {
    let store = Store::open(Path::new(":memory:")).unwrap();
    // Fail after saving the turn, without ever launching a real provider.
    let mut summary = store
        .create("codex", "/missing/optimistic-send-project")
        .unwrap();
    summary.provider = "fixture".into();
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
fn mono_messages_steer_the_ready_provider_once_and_keep_their_send_time(cx: &mut TestAppContext) {
    let window = conversation(cx);
    let (send, receive) = std::sync::mpsc::channel();
    window
        .update(cx, |view, _, cx| {
            view.summary.provider = "codex".into();
            view.configure_mono(
                monocode_core::mono::Mono::default(),
                "/tmp".into(),
                Default::default(),
                cx,
            );
            view.busy = true;
            view.controls = Some(send);
            let message = monocode_core::mono::Message {
                id: "follow-up".into(),
                text: "is".into(),
                sent_at: Some(1234),
                internal: false,
                error: None,
                attachments: vec!["/tmp/image.png".into()],
            };
            assert!(
                !view.mono_dispatch(&message, cx),
                "wait for provider readiness"
            );
            view.follow_ups.ready = true;
            assert!(view.mono_dispatch(&message, cx));
            let control = receive.try_recv().unwrap();
            let Decision::Steer(steer) = control.decision else {
                panic!("expected a real steer");
            };
            assert_eq!(steer.id, message.id);
            assert_eq!(steer.text, "is");
            assert_eq!(steer.sent_at, Some(1234));
            assert_eq!(
                steer.attachments,
                vec![std::path::PathBuf::from("/tmp/image.png")]
            );
            assert!(view.busy);
            assert!(
                !view.mono_dispatch(&message, cx),
                "do not send twice while acknowledgement is pending"
            );
            assert!(receive.try_recv().is_err());
            view.mono_message_delivered(message.id.clone(), cx);
            assert!(
                !view.mono_dispatch(&message, cx),
                "an acknowledged message cannot be resent"
            );
            let second = monocode_core::mono::Message {
                id: "next".into(),
                ..message
            };
            assert!(view.mono_dispatch(&second, cx));
            view.mono_message_failed(second.id.clone(), "Could not steer".into());
            assert_eq!(
                view.mono_take_failures(),
                vec![(second.id, "Could not steer".into())]
            );
            assert!(view.mono_ui.sending.is_none());
        })
        .unwrap();
}

#[gpui::test]
fn send_paints_before_storage_and_keeps_one_message_after_startup_failure(cx: &mut TestAppContext) {
    for text in [
        "First message".to_owned(),
        "🙂 café\n".repeat(400),
        String::new(),
    ] {
        let window = conversation(cx);
        let attachment = std::path::PathBuf::from("/missing/optimistic-attachment.png");
        let sent_at = window
            .update(cx, |view, _, cx| {
                view.attachments.push(attachment.clone());
                view.input
                    .update(cx, |input, cx| input.set_text(text.clone(), cx));
                view.send(cx);
                assert!(view.busy);
                assert!(view.summary.has_messages);
                assert!(view.input.read(cx).content.is_empty());
                assert!(view.attachments.is_empty());
                let rows = view.transcript.read(cx).source_rows();
                assert_eq!(
                    rows.iter()
                        .filter(|row| row.first && row.is_prompt())
                        .count(),
                    1
                );
                assert_eq!(
                    rows.iter()
                        .filter(|row| row.is_prompt())
                        .map(|row| row.text.as_str())
                        .collect::<String>(),
                    text
                );
                assert_eq!(rows.last().unwrap().role, "assistant");
                let manifest = rows
                    .iter()
                    .find(|row| row.role == attachments::ROLE)
                    .unwrap();
                assert_eq!(
                    attachments::paths(manifest).unwrap(),
                    std::slice::from_ref(&attachment)
                );
                assert!(
                    locked(&view.store)
                        .unwrap()
                        .load(view.summary.id)
                        .unwrap()
                        .rows
                        .is_empty(),
                    "the bubble must exist before the background save runs"
                );
                rows[0].sent_at
            })
            .unwrap();
        cx.run_until_parked();
        window
            .update(cx, |view, _, cx| {
                assert!(!view.busy);
                let rows = view.transcript.read(cx).source_rows();
                assert_eq!(
                    rows.iter()
                        .filter(|row| row.first && row.is_prompt())
                        .count(),
                    1
                );
                assert_eq!(
                    rows[0].sent_at, sent_at,
                    "acceptance keeps the original send time"
                );
                assert_eq!(rows.last().unwrap().role, "error");
                assert_eq!(
                    rows,
                    locked(&view.store)
                        .unwrap()
                        .load(view.summary.id)
                        .unwrap()
                        .rows
                );
                // A later send must append after the failed message, not replace it.
                view.input
                    .update(cx, |input, cx| input.set_text("Try again".into(), cx));
                view.send(cx);
                assert_eq!(
                    view.transcript
                        .read(cx)
                        .source_rows()
                        .iter()
                        .filter(|row| row.first && row.is_prompt())
                        .count(),
                    2
                );
            })
            .unwrap();
        cx.run_until_parked();
        window
            .update(cx, |view, _, cx| {
                let rows = view.transcript.read(cx).source_rows();
                assert_eq!(
                    rows.iter()
                        .filter(|row| row.first && row.is_prompt())
                        .count(),
                    2
                );
                assert_eq!(
                    rows,
                    locked(&view.store)
                        .unwrap()
                        .load(view.summary.id)
                        .unwrap()
                        .rows
                );
            })
            .unwrap();
    }
}

#[gpui::test]
fn worktree_setup_shows_the_prompt_immediately_and_preserves_it_on_failure(
    cx: &mut TestAppContext,
) {
    let window = conversation(cx);
    window
        .update(cx, |view, _, cx| {
            view.settings.new_worktree = true;
            view.settings.plan = true;
            view.input.update(cx, |input, cx| {
                input.set_text("Start in a worktree".into(), cx)
            });
            view.send(cx);
            assert!(view.busy && view.preparing_worktree);
            assert_eq!(view.status, "Creating worktree…");
            assert!(view.input.read(cx).content.is_empty());
            assert!(!view.settings.plan);
            assert_eq!(
                view.transcript.read(cx).source_rows()[0].text,
                "Start in a worktree"
            );
            view.input
                .update(cx, |input, cx| input.set_text("Follow up".into(), cx));
            view.send(cx);
            assert_eq!(view.follow_ups.state.messages[0].text, "Follow up");
            assert_eq!(view.follow_ups.state.messages[0].mode, None);
            view.input
                .update(cx, |input, cx| input.set_text("Unsent draft".into(), cx));
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |view, _, cx| {
            assert!(!view.busy && !view.preparing_worktree);
            assert_eq!(view.input.read(cx).content, "Unsent draft");
            assert!(view.follow_ups.state.paused);
            assert_eq!(view.follow_ups.state.messages[0].text, "Follow up");
            let rows = view.transcript.read(cx).source_rows();
            assert_eq!(rows[0].text, "Start in a worktree");
            assert_eq!(rows.last().unwrap().role, "error");
            assert_eq!(
                rows,
                locked(&view.store)
                    .unwrap()
                    .load(view.summary.id)
                    .unwrap()
                    .rows
            );
        })
        .unwrap();
}

#[gpui::test]
fn stopping_before_the_worker_runs_keeps_the_optimistic_message(cx: &mut TestAppContext) {
    for new_worktree in [false, true] {
        let window = conversation(cx);
        window
            .update(cx, |view, _, cx| {
                view.settings.new_worktree = new_worktree;
                view.input.update(cx, |input, cx| {
                    input.set_text("Keep this message".into(), cx)
                });
                view.send(cx);
                view.stop(cx);
                assert!(!view.busy);
                assert_eq!(
                    view.transcript.read(cx).source_rows()[0].text,
                    "Keep this message"
                );
            })
            .unwrap();
        cx.run_until_parked();
        window
            .update(cx, |view, _, cx| {
                assert!(!view.stopping);
                assert!(!view.preparing_worktree);
                let rows = view.transcript.read(cx).source_rows();
                assert_eq!(
                    rows.iter()
                        .filter(|row| row.first && row.is_prompt())
                        .count(),
                    1
                );
                assert_eq!(rows[0].text, "Keep this message");
                assert_eq!(
                    rows,
                    locked(&view.store)
                        .unwrap()
                        .load(view.summary.id)
                        .unwrap()
                        .rows
                );
            })
            .unwrap();
    }
}

/// A real Mono conversation with one stored workflow, its input focused.
fn mono_with_workflow(cx: &mut TestAppContext) -> (WindowHandle<Conversation>, std::path::PathBuf) {
    use monocode_core::mono::workflows::{model::Workflow, store::update_workflows};
    let root = std::env::temp_dir().join(format!("mono-popup-{}", monocode_core::mono::new_id()));
    std::fs::create_dir_all(&root).unwrap();
    update_workflows(
        &monocode_core::mono::workflows::store::MonoDir::new(&root, "mono-a"),
        |mut list| {
            list.push(Workflow::new("wf-popup", "Nightly Deploy"));
            Ok((list, ()))
        },
    )
    .unwrap();
    cx.update(crate::composer_completion::bind);
    let window = conversation(cx);
    window
        .update(cx, |view, window, cx| {
            let mono = monocode_core::mono::Mono {
                id: "mono-a".into(),
                ..Default::default()
            };
            view.configure_mono(mono, root.clone(), Default::default(), cx);
            window.focus(&view.input.focus_handle(cx), cx);
        })
        .unwrap();
    cx.run_until_parked();
    (window, root)
}

#[gpui::test]
fn a_mono_chat_shows_the_workflow_popup_and_enter_picks_before_sending(cx: &mut TestAppContext) {
    let (window, root) = mono_with_workflow(cx);
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    type_into_visual(&window, visual, "@workflow/");
    visual.refresh().unwrap();
    visual.run_until_parked();
    visual.refresh().unwrap();
    assert!(visual.debug_bounds("completion-title-Workflows").is_some());
    let row = visual
        .debug_bounds("completion-row-0")
        .expect("the popup row must exist in the Mono chat");
    let list = visual.debug_bounds("composer-completion-list").unwrap();
    let composer = visual.debug_bounds("mono-composer").unwrap();
    assert!(
        list.bottom() <= composer.top(),
        "popup sits above the composer"
    );
    assert!(list.top() >= gpui::px(0.), "popup is fully visible");
    assert!(row.size.width > gpui::px(0.));
    assert!(list.size.width <= composer.size.width + gpui::px(2.));

    visual.simulate_keystrokes("enter");
    visual.run_until_parked();
    window
        .update(visual, |view, _, cx| {
            assert_eq!(view.input.read(cx).content, "@workflow/nightly-deploy ");
            assert!(!view.summary.has_messages, "the pick must not send");
        })
        .unwrap();
    visual.simulate_keystrokes("enter");
    visual.run_until_parked();
    window
        .update(visual, |view, _, cx| {
            assert!(
                view.input.read(cx).content.is_empty(),
                "the next Enter sends"
            );
        })
        .unwrap();
    let _ = std::fs::remove_dir_all(root);
}

fn type_into_visual(
    window: &WindowHandle<Conversation>,
    cx: &mut gpui::VisualTestContext,
    text: &str,
) {
    window
        .update(cx, |view, window, cx| {
            window.focus(&view.input.focus_handle(cx), cx);
            view.input
                .update(cx, |input, cx| input.set_text(text.into(), cx));
        })
        .unwrap();
    cx.run_until_parked();
}

#[gpui::test]
fn enter_on_a_slash_command_in_a_mono_chat_sends_at_once(cx: &mut TestAppContext) {
    let (window, root) = mono_with_workflow(cx);
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    type_into_visual(&window, visual, "/foo");
    visual.refresh().unwrap();
    assert!(visual.debug_bounds("completion-row-0").is_none());
    assert!(visual.debug_bounds("composer-completion-list").is_none());
    visual.simulate_keystrokes("enter");
    visual.run_until_parked();
    window
        .update(visual, |view, _, cx| {
            assert!(
                view.input.read(cx).content.is_empty(),
                "sent on first Enter"
            );
        })
        .unwrap();
    let _ = std::fs::remove_dir_all(root);
}

#[gpui::test]
fn escape_closes_the_mono_popup_and_tab_picks(cx: &mut TestAppContext) {
    let (window, root) = mono_with_workflow(cx);
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    type_into_visual(&window, visual, "@workflow/");
    visual.refresh().unwrap();
    assert!(visual.debug_bounds("completion-row-0").is_some());
    visual.simulate_keystrokes("escape");
    visual.run_until_parked();
    visual.refresh().unwrap();
    assert!(visual.debug_bounds("completion-row-0").is_none());
    window
        .update(visual, |view, _, cx| {
            assert!(view.settings.mono.is_some());
            assert_eq!(view.input.read(cx).content, "@workflow/");
        })
        .unwrap();
    type_into_visual(&window, visual, "");
    type_into_visual(&window, visual, "@workflow/");
    visual.refresh().unwrap();
    visual.simulate_keystrokes("tab");
    visual.run_until_parked();
    window
        .update(visual, |view, _, cx| {
            assert_eq!(view.input.read(cx).content, "@workflow/nightly-deploy ");
        })
        .unwrap();
    let _ = std::fs::remove_dir_all(root);
}

#[gpui::test]
fn mcp_in_a_mono_chat_sends_like_any_message(cx: &mut TestAppContext) {
    let (window, root) = mono_with_workflow(cx);
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    type_into_visual(&window, visual, "/mcp");
    visual.refresh().unwrap();
    assert!(visual.debug_bounds("composer-completion-list").is_none());
    visual.simulate_keystrokes("enter");
    visual.run_until_parked();
    visual.refresh().unwrap();
    assert!(visual.debug_bounds("composer-completion-list").is_none());
    window
        .update(visual, |view, _, cx| {
            assert!(
                view.input.read(cx).content.is_empty(),
                "sent on first Enter"
            );
        })
        .unwrap();
    let _ = std::fs::remove_dir_all(root);
}
