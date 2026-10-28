//! Pages that slide away, the clock of a hidden panel, and what a page leaves behind when the
//! Mono changes.
use super::board::BoardField;
use super::board_tests::{do_with, on_board, open_board, ticket};
use super::run_model_tests::{linear, nr, run_of};
use super::tests::{A, click, open_tab, put};
use super::*;
use crate::monos::tests::setup;
use gpui::{TestAppContext, VisualTestContext};
use monocode_core::mono::workflows::store::update_runs;
use serde_json::json;
use std::time::Instant;

#[gpui::test]
fn the_board_that_was_left_is_not_built_again_while_its_page_slides_away(cx: &mut TestAppContext) {
    let s = setup(cx);
    let mut visual = open_board(&s, &ticket(json!({})), cx);
    let visual = &mut visual;
    // With motion on, the page that was left stays drawn while it slides out.
    visual.update(|_, cx| cx.set_reduce_motion(false));
    click(visual, "mono-page-back");
    assert_eq!(
        s.manager.read_with(visual, |m, _| m.route.clone()),
        Some(Route::Workflow(A.into()))
    );
    // The page is still drawn while it slides out; drawing it must not make a board.
    for _ in 0..3 {
        frame(&s, visual);
    }
    assert!(s.manager.read_with(visual, |m, _| m.wf.board.is_none()));
    assert!(s.manager.read_with(visual, |m, _| {
        !m.fields.keys().any(|k| k.starts_with("wf-b:"))
    }));
}

#[gpui::test]
fn the_board_drops_its_field_subscriptions_with_it(cx: &mut TestAppContext) {
    let s = setup(cx);
    let mut visual = open_board(&s, &ticket(json!({})), cx);
    let visual = &mut visual;
    let held = s.manager.read_with(visual, |m, _| {
        m.wf.board.as_ref().map(|b| b.subscriptions())
    });
    assert!(held.is_some_and(|n| n > 0));
    click(visual, "mono-page-back");
    visual.run_until_parked();
    // The pages the board comes back to have made what they keep once; another visit to the
    // board adds nothing.
    let before = s.manager.read_with(visual, |m, _| m.observation_count());
    for _ in 0..2 {
        s.manager.update(visual, |m, cx| {
            m.open_workflow_route(Route::WorkflowBoard(A.into()), cx)
        });
        visual.run_until_parked();
        click(visual, "mono-page-back");
        visual.run_until_parked();
    }
    assert_eq!(
        s.manager.read_with(visual, |m, _| m.observation_count()),
        before
    );
}

/// One frame, drawn 50 ms into the slide: the pages sliding in or out are drawn with it.
fn frame(s: &crate::monos::tests::Setup, visual: &mut VisualTestContext) {
    let details = s.manager.read_with(visual, |m, _| m.details.clone());
    details.update(visual, |d, cx| {
        d.elapsed(std::time::Duration::from_millis(50), cx)
    });
    visual
        .update_window(s.window.into(), |_, w, cx| {
            w.simulate_next_frame(cx);
            w.draw(cx).clear(cx);
        })
        .unwrap();
    visual.run_until_parked();
}

fn two_runs(s: &crate::monos::tests::Setup) {
    let run = |id: &str| {
        let mut r = run_of(
            &linear(),
            "done",
            json!({}),
            json!([nr("trigger", "", "done", json!({}))]),
        );
        r.id = id.into();
        r
    };
    update_runs(&MonoDir::new(&s.root, &s.id), A, |_| {
        Ok((vec![run("run-2"), run("run-1")], ()))
    })
    .unwrap();
}

#[gpui::test]
fn opening_a_second_run_page_while_the_first_slides_away_keeps_one_canvas(cx: &mut TestAppContext) {
    let s = setup(cx);
    put(&s, vec![ticket_free()]);
    two_runs(&s);
    open_tab(&s, cx);
    let visual = &mut VisualTestContext::from_window(s.window.into(), cx);
    visual.run_until_parked();
    s.manager.update(visual, |m, cx| {
        m.open_workflow_route(Route::WorkflowRun(A.into(), "run-1".into()), cx)
    });
    visual.run_until_parked();
    visual.update(|_, cx| cx.set_reduce_motion(false));
    visual.update(|_, cx| cx.set_reduce_motion(false));
    s.manager.update(visual, |m, cx| {
        m.open_workflow_route(Route::WorkflowRun(A.into(), "run-2".into()), cx)
    });
    visual.run_until_parked();
    // Both pages are drawn for a while (one in, one out): the state stays the new run's.
    let opens = s.manager.read_with(visual, |m, _| m.wf.run_opens);
    for _ in 0..4 {
        frame(&s, visual);
    }
    // One build for each run page opened, however many frames drew the page sliding away.
    assert_eq!(
        s.manager.read_with(visual, |m, _| m.wf.run_opens),
        opens,
        "a page sliding away rebuilt the run page"
    );
    assert_eq!(opens, 2, "a page sliding away rebuilt the run page");
    let key = s.manager.read_with(visual, |m, _| m.wf.run.key.clone());
    assert_eq!(key, Some((A.to_string(), "run-2".to_string())));
}

fn ticket_free() -> Workflow {
    super::tests::workflow(json!({}))
}

#[gpui::test]
fn the_clock_stops_when_the_panel_is_not_drawn_and_starts_again_when_it_is(
    cx: &mut TestAppContext,
) {
    let s = setup(cx);
    put(&s, vec![ticket_free()]);
    open_tab(&s, cx);
    let visual = &mut VisualTestContext::from_window(s.window.into(), cx);
    visual.run_until_parked();
    assert!(s.manager.read_with(visual, |m, _| m.wf.poll.is_some()));
    // Drawn a moment ago: the clock goes on.
    do_with_manager(&s, visual, |m, cx| assert!(m.workflows_poll(cx)));
    // Not drawn for a long while (the right panel is closed): it stops and reads nothing.
    do_with_manager(&s, visual, |m, cx| {
        m.wf.last_drawn = Some(Instant::now() - std::time::Duration::from_secs(30));
        assert!(!m.workflows_poll(cx));
        assert!(m.wf.poll.is_none());
    });
    // Drawn again: it reads at once and the clock is back.
    put(
        &s,
        vec![
            ticket_free(),
            super::tests::workflow(json!({"id": "wf-bbbbbbbb"})),
        ],
    );
    s.manager.update(visual, |_, cx| cx.notify());
    visual.refresh().unwrap();
    visual.run_until_parked();
    assert!(s.manager.read_with(visual, |m, _| m.wf.poll.is_some()));
    assert_eq!(
        s.manager.read_with(visual, |m, _| m.workflow_count()),
        Some(2)
    );
}

fn do_with_manager(
    s: &crate::monos::tests::Setup,
    visual: &mut VisualTestContext,
    f: impl FnOnce(&mut Monos, &mut Context<Monos>),
) {
    s.manager.update(visual, f);
    visual.run_until_parked();
}

#[gpui::test]
fn a_board_does_not_ask_about_an_edit_while_its_own_save_is_going(cx: &mut TestAppContext) {
    let s = setup(cx);
    let mut visual = open_board(&s, &ticket(json!({})), cx);
    let visual = &mut visual;
    super::board_tests::rename(&s, visual, "Mine");
    put(&s, vec![ticket(json!({"name": "By the Mono"}))]);
    do_with(&s, visual, |m, cx| {
        m.refresh_workflows(cx);
        if let Some(b) = m.wf.board.as_mut() {
            b.saving = true;
        }
    });
    assert!(on_board(&s, visual, |b| b.elsewhere.is_none()));
    // Once the save is over the read is news again.
    do_with(&s, visual, |m, cx| {
        if let Some(b) = m.wf.board.as_mut() {
            b.saving = false;
        }
        m.board_sync(cx);
    });
    assert!(on_board(&s, visual, |b| b.elsewhere.is_some()));
    let _ = BoardField::Name;
}

#[gpui::test]
fn changing_the_mono_leaves_no_board_or_notice_behind(cx: &mut TestAppContext) {
    let s = setup(cx);
    let mut visual = open_board(&s, &ticket(json!({})), cx);
    let visual = &mut visual;
    do_with(&s, visual, |m, cx| {
        m.workflow_said(Some("old".into()), true, cx)
    });
    let handle: gpui::AnyWindowHandle = s.window.into();
    let project = s.project.clone();
    handle
        .update(visual, |_, w, cx| {
            s.manager.update(cx, |m, cx| m.create(vec![project], w, cx))
        })
        .unwrap();
    visual.run_until_parked();
    let first = s.id.clone();
    handle
        .update(visual, |_, w, cx| {
            s.manager.update(cx, |m, cx| m.open(&first, w, cx))
        })
        .unwrap();
    visual.run_until_parked();
    s.manager.read_with(visual, |m, _| {
        assert!(m.wf.board.is_none());
        assert_eq!(m.wf.page_notice, None);
    });
}
