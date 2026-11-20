use super::remote_update::{
    Origin, Override, State,
    testing::{FAILED, Fake},
};
use super::tests::window;
use super::*;
use gpui::{TestAppContext, WindowHandle};
use monocode_core::app_update::remote::{Mode, Outcome, Pending, Request};
use serde_json::Value;
use std::sync::Arc;

fn device() -> Origin {
    Origin::Device {
        name: "Phone".into(),
    }
}
fn check() -> Request {
    Request {
        mode: Mode::Check,
        force: false,
    }
}
fn install(force: bool) -> Request {
    Request {
        mode: Mode::Install,
        force,
    }
}

/// Runs a request the way the entry points do: the reply first, then the install.
fn run(
    window: &WindowHandle<Shell>,
    cx: &mut TestAppContext,
    origin: Origin,
    request: Request,
) -> Result<Value, String> {
    let task = window
        .update(cx, |shell, _, cx| shell.remote_update(origin, request, cx))
        .unwrap();
    let slot = std::rc::Rc::new(std::cell::RefCell::new(None));
    let sink = slot.clone();
    cx.update(|cx| {
        cx.spawn(async move |_| *sink.borrow_mut() = Some(task.await))
            .detach()
    });
    cx.run_until_parked();
    let reply = slot.borrow_mut().take().expect("the request settled")?;
    if let Some(begin) = reply.begin {
        window
            .update(cx, |shell, _, cx| shell.remote_update_begin(begin, cx))
            .unwrap();
    }
    cx.run_until_parked();
    Ok(reply.value)
}
/// Two requests asked before either settles, as two devices can.
fn run_together(
    window: &WindowHandle<Shell>,
    cx: &mut TestAppContext,
    origins: [Origin; 2],
) -> Vec<Result<Value, String>> {
    let slots: Vec<_> = origins
        .into_iter()
        .map(|origin| {
            let task = window
                .update(cx, |shell, _, cx| {
                    shell.remote_update(origin, install(false), cx)
                })
                .unwrap();
            let slot = std::rc::Rc::new(std::cell::RefCell::new(None));
            let sink = slot.clone();
            cx.update(|cx| {
                cx.spawn(async move |_| *sink.borrow_mut() = Some(task.await))
                    .detach()
            });
            slot
        })
        .collect();
    cx.run_until_parked();
    slots
        .into_iter()
        .map(|slot| {
            let reply = slot.borrow_mut().take().expect("settled");
            reply.map(|reply| {
                if let Some(begin) = reply.begin {
                    window
                        .update(cx, |shell, _, cx| shell.remote_update_begin(begin, cx))
                        .unwrap();
                }
                reply.value
            })
        })
        .collect()
}
fn pushes(cx: &mut TestAppContext) -> Vec<(String, String)> {
    cx.update(|cx| cx.default_global::<State>().pushes.clone())
}
fn mono(id: &str) -> Origin {
    Origin::Mono {
        id: id.into(),
        name: "Ada".into(),
        session: 999,
    }
}

fn use_backend(cx: &mut TestAppContext, fake: &Arc<Fake>) {
    cx.update(|cx| cx.set_global(Override(fake.clone())));
    FAILED.with(|slot| *slot.borrow_mut() = None);
}
fn notices(cx: &mut TestAppContext) -> Vec<String> {
    cx.update(|cx| cx.default_global::<State>().notices.clone())
}
fn stored(window: &WindowHandle<Shell>, cx: &mut TestAppContext) -> Option<Pending> {
    window
        .update(cx, |shell, _, _| {
            Pending::load(&locked(&shell.store).unwrap()).unwrap()
        })
        .unwrap()
}
fn make_busy(window: &WindowHandle<Shell>, cx: &mut TestAppContext, id: i64) {
    window
        .update(cx, |shell, _, cx| {
            shell.sessions[&id].update(cx, |chat, _| chat.busy = true)
        })
        .unwrap();
}

#[gpui::test]
fn check_reports_versions_notes_and_running_sessions(cx: &mut TestAppContext) {
    let (window, a, _) = window(cx);
    let fake = Fake::new("0.1.0", Some("0.2.0"));
    use_backend(cx, &fake);
    make_busy(&window, cx, a);
    let value = run(&window, cx, device(), check()).unwrap();
    assert_eq!(value["current"], "0.1.0");
    assert_eq!(value["latest"], "0.2.0");
    assert_eq!(value["updateAvailable"], true);
    assert_eq!(value["notes"], "Better things");
    assert_eq!(value["runningSessions"][0]["id"], a.to_string());
    assert!(fake.installs.lock().unwrap().is_empty());
    assert!(
        notices(cx).is_empty(),
        "a check changes nothing, so it says nothing"
    );
    assert!(stored(&window, cx).is_none());
}

#[gpui::test]
fn install_answers_first_records_the_request_and_tells_the_computer(cx: &mut TestAppContext) {
    let (window, _, _) = window(cx);
    let fake = Fake::new("0.1.0", Some("0.2.0"));
    use_backend(cx, &fake);
    let origin = Origin::Mono {
        id: "mono-1".into(),
        name: "Ada".into(),
        session: 999,
    };
    let value = run(&window, cx, origin, install(false)).unwrap();
    assert_eq!(value["status"], "updating");
    assert_eq!(
        (value["from"].as_str(), value["to"].as_str()),
        (Some("0.1.0"), Some("0.2.0"))
    );
    assert_eq!(*fake.installs.lock().unwrap(), vec!["0.2.0".to_string()]);
    let pending = stored(&window, cx).expect("recorded before the restart");
    assert_eq!(
        (pending.to.as_str(), pending.chat.as_deref()),
        ("0.2.0", Some("mono-1"))
    );
    assert_eq!(pending.requester, "Ada");
    assert_eq!(
        notices(cx),
        vec!["Ada asked to update and restart Monoptah.".to_string()]
    );
    // Only one install at a time.
    let again = run(&window, cx, device(), install(false));
    assert_eq!(again.unwrap_err(), "An update is already in progress");
    assert_eq!(fake.installs.lock().unwrap().len(), 1);
}

#[gpui::test]
fn running_sessions_need_confirmation_and_force_goes_ahead(cx: &mut TestAppContext) {
    let (window, a, _) = window(cx);
    let fake = Fake::new("0.1.0", Some("0.2.0"));
    use_backend(cx, &fake);
    make_busy(&window, cx, a);
    let held = run(&window, cx, device(), install(false)).unwrap();
    assert_eq!(held["status"], "confirmation_required");
    assert_eq!(held["runningSessions"][0]["id"], a.to_string());
    assert!(fake.installs.lock().unwrap().is_empty());
    assert!(stored(&window, cx).is_none());
    // The request itself is still told to the computer.
    assert_eq!(notices(cx).len(), 1);
    let forced = run(&window, cx, device(), install(true)).unwrap();
    assert_eq!(forced["status"], "updating");
    assert_eq!(fake.installs.lock().unwrap().len(), 1);
}

#[gpui::test]
fn the_caller_own_chat_does_not_count_as_running(cx: &mut TestAppContext) {
    let (window, a, _) = window(cx);
    let fake = Fake::new("0.1.0", Some("0.2.0"));
    use_backend(cx, &fake);
    make_busy(&window, cx, a);
    let origin = Origin::Mono {
        id: "m".into(),
        name: "Ada".into(),
        session: a,
    };
    let value = run(&window, cx, origin, install(false)).unwrap();
    assert_eq!(value["status"], "updating");
}

#[gpui::test]
fn an_older_or_equal_feed_is_never_installed(cx: &mut TestAppContext) {
    let (window, _, _) = window(cx);
    for latest in ["0.1.0", "0.0.9"] {
        let fake = Fake::new("0.1.0", Some(latest));
        use_backend(cx, &fake);
        let value = run(&window, cx, device(), install(true)).unwrap();
        assert_eq!(value["status"], "up_to_date");
        assert!(fake.installs.lock().unwrap().is_empty());
        assert!(stored(&window, cx).is_none());
    }
}

#[gpui::test]
fn a_feed_that_cannot_be_read_is_an_error_and_a_check_says_so(cx: &mut TestAppContext) {
    let (window, _, _) = window(cx);
    let mut fake = Fake::new("0.1.0", None);
    Arc::get_mut(&mut fake).unwrap().feed_error = Some("offline".into());
    use_backend(cx, &fake);
    let status = run(&window, cx, device(), check()).unwrap();
    assert_eq!(status["error"], "offline");
    assert_eq!(status["updateAvailable"], false);
    let error = run(&window, cx, device(), install(true)).unwrap_err();
    assert!(error.contains("offline"));
    assert!(stored(&window, cx).is_none());
}

#[gpui::test]
fn a_failure_before_the_restart_is_reported_and_the_app_keeps_running(cx: &mut TestAppContext) {
    let (window, _, _) = window(cx);
    let fake = Fake::new("0.1.0", Some("0.2.0"));
    use_backend(cx, &fake);
    run(&window, cx, device(), install(false)).unwrap();
    assert!(stored(&window, cx).is_some());
    // The updater reports a failed download.
    let failed = FAILED
        .with(|slot| slot.borrow_mut().take())
        .expect("install started");
    cx.update(|cx| failed("disk full".into(), cx));
    cx.run_until_parked();
    assert!(
        stored(&window, cx).is_none(),
        "nothing is left to judge at the next launch"
    );
    assert_eq!(
        notices(cx).last().unwrap(),
        "Update to 0.2.0 failed: disk full"
    );
    assert!(!cx.update(|cx| cx.default_global::<State>().installing));
    // The app is still here to take another request.
    assert_eq!(
        run(&window, cx, device(), install(false)).unwrap()["status"],
        "updating"
    );
}

#[gpui::test]
fn an_install_that_cannot_start_is_reported_at_once(cx: &mut TestAppContext) {
    let (window, _, _) = window(cx);
    let mut fake = Fake::new("0.1.0", Some("0.2.0"));
    Arc::get_mut(&mut fake).unwrap().install_error = Some("busy".into());
    use_backend(cx, &fake);
    let value = run(&window, cx, device(), install(false)).unwrap();
    assert_eq!(value["status"], "updating");
    assert_eq!(notices(cx).last().unwrap(), "Update to 0.2.0 failed: busy");
    assert!(stored(&window, cx).is_none());
}

#[gpui::test]
fn the_next_launch_records_what_became_of_the_request(cx: &mut TestAppContext) {
    let (window, _, _) = window(cx);
    let running = crate::notifications::updater::version();
    for (target, expected) in [
        (running.to_string(), format!("Updated to {running}")),
        (
            "99.0.0".to_string(),
            format!(
                "Update to 99.0.0 failed: the app restarted on version {running} instead of 99.0.0"
            ),
        ),
    ] {
        window
            .update(cx, |shell, _, cx| {
                let pending = Pending {
                    from: "0.0.1".into(),
                    to: target.clone(),
                    kind: "mono".into(),
                    requester: "Ada".into(),
                    chat: None,
                    requested_at: 1,
                };
                pending.save(&locked(&shell.store).unwrap()).unwrap();
                shell.consume_remote_update(cx);
            })
            .unwrap();
        cx.run_until_parked();
        assert_eq!(notices(cx).last().unwrap(), &expected);
        assert!(stored(&window, cx).is_none(), "the record is cleared");
    }
}

#[gpui::test]
fn two_requests_in_flight_start_one_install_and_keep_the_winners_record(cx: &mut TestAppContext) {
    let (window, _, _) = window(cx);
    let fake = Fake::new("0.1.0", Some("0.2.0"));
    use_backend(cx, &fake);
    let results = run_together(&window, cx, [mono("first"), mono("second")]);
    assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1);
    let refused = results.iter().find_map(|r| r.as_ref().err()).unwrap();
    assert_eq!(refused, "An update is already in progress");
    assert_eq!(fake.installs.lock().unwrap().len(), 1);
    // The feed checks run on the background executor, so either request may win. The
    // refused one did not touch the record of the one that runs.
    let winner = if results[0].is_ok() {
        "first"
    } else {
        "second"
    };
    let pending = stored(&window, cx).expect("the running request stays recorded");
    assert_eq!(pending.chat.as_deref(), Some(winner));
    assert!(cx.update(|cx| cx.default_global::<State>().installing));
}

#[gpui::test]
fn a_failure_only_clears_its_own_record(cx: &mut TestAppContext) {
    let (window, _, _) = window(cx);
    let fake = Fake::new("0.1.0", Some("0.2.0"));
    use_backend(cx, &fake);
    run(&window, cx, mono("first"), install(false)).unwrap();
    let other = Pending {
        from: "0.1.0".into(),
        to: "0.3.0".into(),
        kind: "mono".into(),
        requester: "Bo".into(),
        chat: Some("second".into()),
        requested_at: 7,
    };
    window
        .update(cx, |shell, _, cx| {
            other.save(&locked(&shell.store).unwrap()).unwrap();
            let stale = Pending {
                requested_at: 1,
                ..other.clone()
            };
            shell.finish_remote_update(&stale, Outcome::Failed("late".into()), cx);
        })
        .unwrap();
    cx.run_until_parked();
    assert_eq!(stored(&window, cx), Some(other));
}

#[gpui::test]
fn every_result_is_pushed_to_control_devices_and_a_failure_shows_in_the_status(
    cx: &mut TestAppContext,
) {
    let (window, _, _) = window(cx);
    let fake = Fake::new("0.1.0", Some("0.2.0"));
    use_backend(cx, &fake);
    // Failure before the restart.
    run(&window, cx, mono("chat-1"), install(false)).unwrap();
    let failed = FAILED.with(|slot| slot.borrow_mut().take()).unwrap();
    cx.update(|cx| failed("disk full".into(), cx));
    cx.run_until_parked();
    assert_eq!(
        pushes(cx),
        vec![(
            "chat-1".to_string(),
            "Update to 0.2.0 failed: disk full".to_string()
        )]
    );
    let status = run(&window, cx, device(), check()).unwrap();
    assert_eq!(status["lastFailure"], "Update to 0.2.0 failed: disk full");
    // The next try clears it.
    run(&window, cx, device(), install(false)).unwrap();
    assert!(
        run(&window, cx, device(), check())
            .unwrap()
            .get("lastFailure")
            .is_none()
    );
    let failed = FAILED.with(|slot| slot.borrow_mut().take()).unwrap();
    cx.update(|cx| failed("y".into(), cx));
    cx.run_until_parked();
    // After the restart, success and failure alike.
    let running = crate::notifications::updater::version();
    for target in [running.to_string(), "99.0.0".to_string()] {
        window
            .update(cx, |shell, _, cx| {
                let pending = Pending {
                    from: "0.0.1".into(),
                    to: target,
                    kind: "mono".into(),
                    requester: "Ada".into(),
                    chat: Some("chat-2".into()),
                    requested_at: 1,
                };
                pending.save(&locked(&shell.store).unwrap()).unwrap();
                shell.consume_remote_update(cx);
            })
            .unwrap();
        cx.run_until_parked();
    }
    let all = pushes(cx);
    assert_eq!(all.len(), 3);
    assert_eq!(all[1].1, format!("Updated to {running}"));
    assert!(all[2].1.starts_with("Update to 99.0.0 failed"));
    // A device that asked has no chat to open: nothing is pushed for it.
    run(&window, cx, device(), install(false)).unwrap();
    let failed = FAILED.with(|slot| slot.borrow_mut().take()).unwrap();
    cx.update(|cx| failed("x".into(), cx));
    cx.run_until_parked();
    assert_eq!(pushes(cx).len(), 3);
}

#[gpui::test]
fn an_abandoned_begin_clears_the_record_and_the_lock(cx: &mut TestAppContext) {
    let (window, _, _) = window(cx);
    let fake = Fake::new("0.1.0", Some("0.2.0"));
    use_backend(cx, &fake);
    let task = window
        .update(cx, |shell, _, cx| {
            shell.remote_update(device(), install(false), cx)
        })
        .unwrap();
    let slot = std::rc::Rc::new(std::cell::RefCell::new(None));
    let sink = slot.clone();
    cx.update(|cx| {
        cx.spawn(async move |_| *sink.borrow_mut() = Some(task.await))
            .detach()
    });
    cx.run_until_parked();
    let begin = slot.borrow_mut().take().unwrap().unwrap().begin.unwrap();
    assert!(stored(&window, cx).is_some());
    let store = window
        .update(cx, |shell, _, _| shell.store.clone())
        .unwrap();
    cx.update(|cx| begin.abandon(&store, cx));
    assert!(stored(&window, cx).is_none());
    assert!(!cx.update(|cx| cx.default_global::<State>().installing));
    assert!(fake.installs.lock().unwrap().is_empty());
}
