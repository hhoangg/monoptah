//! The real host over a test `Monos`: the `workflowSessionStart.test.ts` cases, hidden Ask-Mono
//! sessions, the report in the chat and the run history file. No provider is ever started.
use super::*;
use crate::monos::tests::{
    Setup, call_from, chat_rows, edit_mono, groups_of, launches_of, session_count, setup,
    started_link,
};
use crate::monos::workflow_runner::Live;
use crate::runtime::locked;
use monocode_core::agent::AccessMode;
use monocode_core::{
    catalog::Model,
    mono::workflows::{
        models::Probe,
        store::{MonoDir, load_runs},
    },
    provider::Kind,
};

fn request(s: &Setup) -> SessionRequest {
    SessionRequest {
        project: s.project.clone(),
        prompt: "Do it".into(),
        harness: None,
        model: None,
        effort: None,
        new_worktree: false,
        worktree_cwd: None,
        parent_session_id: None,
        folder_id: None,
        folder_name: None,
    }
}

/// Starts a session through the host path, with the prompt left in the composer so no provider
/// runs.
fn begin(
    s: &Setup,
    cx: &mut TestAppContext,
    req: SessionRequest,
) -> Result<StartedSession, String> {
    let (tx, rx) = async_channel::bounded(1);
    let asked = cx.update(|cx| {
        s.manager
            .update(cx, |m, cx| m.workflow_session(&s.id, req, true, tx, cx))
    });
    assert!(asked?, "the Mono's chat is loaded");
    cx.run_until_parked();
    rx.try_recv().expect("the start answered")
}

fn local(started: &StartedSession) -> i64 {
    started
        .session_id
        .parse()
        .expect("an unshared store keeps the plain id")
}

fn probe_with(s: &Setup, cx: &mut TestAppContext, probe: fn(Kind) -> Probe) {
    cx.update(|cx| {
        s.manager
            .update(cx, |m, _| m.workflow_live = Live::new().with_probe(probe))
    });
}

fn catalog_model(id: &str, efforts: &[&str]) -> Model {
    Model {
        id: id.into(),
        name: id.into(),
        alias_target: None,
        efforts: efforts.iter().map(|e| e.to_string()).collect(),
        default_effort: None,
        fast: false,
        thinking: false,
        default: false,
        tiers: vec![],
        controls: vec![],
    }
}

fn only_test_model(kind: Kind) -> Probe {
    Probe {
        installed: true,
        models: (kind == Kind::Codex).then(|| vec![catalog_model("test", &[])]),
    }
}

fn efforts_listed(kind: Kind) -> Probe {
    Probe {
        installed: true,
        models: match kind {
            Kind::Grok => Some(vec![catalog_model("grok-4.6", &["low", "high"])]),
            Kind::Codex => Some(vec![catalog_model("test", &[])]),
            _ => None,
        },
    }
}

fn host_of(s: &Setup, cx: &mut TestAppContext) -> Rc<dyn WorkflowHost> {
    cx.update(|cx| s.manager.update(cx, |m, cx| m.workflow_host(cx)))
}

/// Resolves `future` on the foreground executor and keeps its answer.
fn collect<T: 'static>(cx: &mut TestAppContext, future: Fut<T>) -> Rc<RefCell<Option<T>>> {
    let slot = Rc::new(RefCell::new(None));
    let out = slot.clone();
    cx.spawn(async move |_| *out.borrow_mut() = Some(future.await))
        .detach();
    slot
}

// --- workflowSessionStart.test.ts -------------------------------------------------------------

#[gpui::test]
fn it_starts_a_full_access_session_without_a_completion_report_and_files_it_in_a_folder(
    cx: &mut TestAppContext,
) {
    let s = setup(cx);
    let mut req = request(&s);
    req.folder_name = Some("Ship it · 2026-10-22 10:00".into());
    let first = begin(&s, cx, req.clone()).unwrap();
    let id = local(&first);

    let launch = launches_of(&s, cx)
        .into_iter()
        .find(|l| l.session_id == id)
        .unwrap();
    assert!(!launch.notify_on_complete && !launch.completed && !launch.dismissed && launch.visible);
    cx.update(|cx| {
        let monos = s.manager.read(cx);
        let view = monos
            .owner
            .upgrade()
            .unwrap()
            .read(cx)
            .mono_session_entity(id)
            .unwrap();
        assert_eq!(view.read(cx).settings.access, AccessMode::FullAccess);
        // Always listed, so the run can be followed from the sidebar, whatever the Mono's setting.
        assert!(
            !monos
                .roster
                .get(&s.id)
                .unwrap()
                .show_started_sessions_in_sidebar
        );
        assert!(!monos.hidden_session(id));
    });
    let groups = groups_of(&s, cx);
    assert_eq!(groups.folders.len(), 1);
    assert_eq!(groups.folders[0].name, "Ship it · 2026-10-22 10:00");
    assert_eq!(groups.folders[0].session_ids, [id]);
    assert_eq!(
        first.folder_id.as_deref(),
        Some(groups.folders[0].id.as_str())
    );

    // The next session of the run reuses that folder.
    req.folder_id = first.folder_id.clone();
    let second = begin(&s, cx, req).unwrap();
    let groups = groups_of(&s, cx);
    assert_eq!(groups.folders.len(), 1);
    assert_eq!(groups.folders[0].session_ids, [id, local(&second)]);
}

#[gpui::test]
fn letting_go_of_a_session_drops_its_watch_and_cached_id(cx: &mut TestAppContext) {
    let s = setup(cx);
    let watched = |cx: &mut TestAppContext| {
        cx.update(|cx| {
            let m = s.manager.read(cx);
            (m.observations.len(), m.workflow_live.kept())
        })
    };
    let (observed, kept) = watched(cx);
    assert_eq!(kept, 0);
    let started = begin(&s, cx, request(&s)).unwrap();
    let (during, kept) = watched(cx);
    assert!(
        kept >= 2,
        "a watch and a wake-up are kept while the run uses it"
    );
    let host = host_of(&s, cx);
    host.release_session(&started.session_id);
    assert_eq!(watched(cx), (observed.max(during), 0));
    // The watch never went into the shared list, so it can end.
    assert_eq!(during, observed);
    // A session can be watched again afterwards.
    let again = host.send(&started.session_id, "Again".into());
    drop(again);
    host.release_session(&started.session_id);
    assert_eq!(watched(cx).1, 0);
}

#[gpui::test]
fn a_session_a_workflow_starts_is_hidden_only_by_the_monos_own_setting(cx: &mut TestAppContext) {
    let s = setup(cx);
    let started = begin(&s, cx, request(&s)).unwrap();
    let id = local(&started);
    // A launch made by `sessions.start` follows the setting; this one does not.
    let normal = {
        let mut input = json!({"prompt": "Other", "draft": true, "notifyOnComplete": false});
        input["project"] = json!(s.project);
        call_from(&s, s.session, "plain", "sessions.start", input, cx)
            .try_recv()
            .unwrap()
            .unwrap()["sessionId"]
            .as_i64()
            .unwrap()
    };
    cx.update(|cx| {
        let monos = s.manager.read(cx);
        assert!(monos.hidden_session(normal));
        assert!(!monos.hidden_session(id));
    });
}

#[gpui::test]
fn a_launch_made_while_the_setting_is_on_is_hidden_again_when_it_goes_off(cx: &mut TestAppContext) {
    let s = setup(cx);
    edit_mono(&s, cx, |m| m.show_started_sessions_in_sidebar = true);
    let mut input = json!({"prompt": "Other", "draft": true, "notifyOnComplete": false});
    input["project"] = json!(s.project);
    let id = call_from(&s, s.session, "plain", "sessions.start", input, cx)
        .try_recv()
        .unwrap()
        .unwrap()["sessionId"]
        .as_i64()
        .unwrap();
    assert!(
        !launches_of(&s, cx)
            .iter()
            .find(|l| l.session_id == id)
            .unwrap()
            .visible
    );
    cx.update(|cx| assert!(!s.manager.read(cx).hidden_session(id)));
    edit_mono(&s, cx, |m| m.show_started_sessions_in_sidebar = false);
    cx.update(|cx| assert!(s.manager.read(cx).hidden_session(id)));
}

#[gpui::test]
fn it_nests_under_a_session_this_mono_started_and_drops_a_parent_it_does_not_know(
    cx: &mut TestAppContext,
) {
    let s = setup(cx);
    let parent = begin(&s, cx, request(&s)).unwrap();
    let mut req = request(&s);
    req.parent_session_id = Some(parent.session_id.clone());
    let child = begin(&s, cx, req.clone()).unwrap();
    assert_eq!(started_link(&s, local(&child)), Some(local(&parent)));
    cx.update(|cx| {
        let owner = s.manager.read(cx).owner.upgrade().unwrap();
        assert_eq!(
            owner.read(cx).mono_started_by(local(&child)),
            Some(local(&parent))
        );
    });

    // A child cannot be a parent, a session the app forgot is no parent, and nor is one the Mono
    // never started: the session just starts at the top level.
    let plain = locked(&s.store)
        .unwrap()
        .create("codex", &s.project)
        .unwrap()
        .id;
    for parent in [
        child.session_id.clone(),
        "app-gone-dev".into(),
        plain.to_string(),
    ] {
        req.parent_session_id = Some(parent.clone());
        let flat = begin(&s, cx, req.clone()).unwrap_or_else(|e| panic!("{parent}: {e}"));
        assert_eq!(started_link(&s, local(&flat)), None, "{parent}");
    }
}

#[gpui::test]
fn it_starts_a_model_in_its_own_harness_when_the_node_sets_only_a_model(cx: &mut TestAppContext) {
    let s = setup(cx);
    let mut req = request(&s);
    // The Mono's chat runs on codex; grok:grok-4.6 belongs to grok.
    req.model = Some("grok:grok-4.6".into());
    let started = begin(&s, cx, req).unwrap();
    let id = local(&started);
    let launch = launches_of(&s, cx)
        .into_iter()
        .find(|l| l.session_id == id)
        .unwrap();
    assert_eq!(
        (launch.harness.as_str(), launch.model.as_deref()),
        ("grok", Some("grok-4.6"))
    );
    cx.update(|cx| {
        let owner = s.manager.read(cx).owner.upgrade().unwrap();
        let view = owner.read(cx).mono_session_entity(id).unwrap();
        assert_eq!(view.read(cx).summary.provider, "grok");
        assert_eq!(view.read(cx).settings.model.as_deref(), Some("grok-4.6"));
    });
}

#[gpui::test]
fn it_names_the_model_when_the_catalog_no_longer_has_it(cx: &mut TestAppContext) {
    let s = setup(cx);
    probe_with(&s, cx, only_test_model);
    let before = session_count(&s);
    let mut req = request(&s);
    req.harness = Some("codex".into());
    req.model = Some("codex:gone".into());
    assert_eq!(
        begin(&s, cx, req.clone()).err().as_deref(),
        Some("Model \"codex:gone\" is not available. Pick one in the step's model list.")
    );
    assert_eq!(session_count(&s), before, "no session row is left behind");
    // A model the catalog lists starts.
    req.model = Some("codex:test".into());
    assert!(begin(&s, cx, req).is_ok());
}

#[gpui::test]
fn it_names_the_effort_when_the_model_does_not_offer_it(cx: &mut TestAppContext) {
    let s = setup(cx);
    probe_with(&s, cx, efforts_listed);
    let before = session_count(&s);
    let mut req = request(&s);
    req.harness = Some("grok".into());
    req.model = Some("grok:grok-4.6".into());
    req.effort = Some("turbo".into());
    assert_eq!(
        begin(&s, cx, req.clone()).err().as_deref(),
        Some(
            "Effort \"turbo\" is not available for this model. Pick one in the step's model list."
        )
    );
    // A model with no effort setting at all reads the same.
    let mut flat = request(&s);
    flat.harness = Some("codex".into());
    flat.model = Some("codex:test".into());
    flat.effort = Some("high".into());
    assert!(
        begin(&s, cx, flat)
            .err()
            .unwrap()
            .starts_with("Effort \"high\" is not available")
    );
    assert_eq!(session_count(&s), before);

    req.effort = Some("high".into());
    let started = begin(&s, cx, req).unwrap();
    cx.update(|cx| {
        let owner = s.manager.read(cx).owner.upgrade().unwrap();
        let view = owner.read(cx).mono_session_entity(local(&started)).unwrap();
        assert_eq!(view.read(cx).settings.effort.as_deref(), Some("high"));
    });
}

#[gpui::test]
fn it_rejects_a_project_that_is_not_the_monos(cx: &mut TestAppContext) {
    let s = setup(cx);
    let before = session_count(&s);
    let mut req = request(&s);
    req.project = "/code/other".into();
    let (tx, _rx) = async_channel::bounded(1);
    let asked = cx.update(|cx| {
        s.manager
            .update(cx, |m, cx| m.workflow_session(&s.id, req, true, tx, cx))
    });
    assert_eq!(asked, Err("Not one of your projects".into()));
    assert_eq!(session_count(&s), before);
}

#[gpui::test]
fn a_chat_that_is_still_loading_is_waited_for(cx: &mut TestAppContext) {
    let s = setup(cx);
    let unload = |cx: &mut TestAppContext| {
        cx.update(|cx| s.manager.update(cx, |m, _| m.chats.remove(&s.id)));
    };
    // Straight away: not ready, and the load has begun.
    unload(cx);
    let (tx, rx) = async_channel::bounded(1);
    let asked = cx.update(|cx| {
        s.manager.update(cx, |m, cx| {
            m.workflow_session(&s.id, request(&s), true, tx, cx)
        })
    });
    assert_eq!(asked, Ok(false));
    assert!(rx.try_recv().is_err());
    cx.run_until_parked();
    assert!(
        begin(&s, cx, request(&s)).is_ok(),
        "ready once it has loaded"
    );

    // Through the host, the same wait happens by itself.
    unload(cx);
    let host = host_of(&s, cx);
    let answer = collect(cx, host.ask_mono(&s.id, "Ship it?".into()));
    for _ in 0..5 {
        cx.run_until_parked();
        cx.executor().advance_clock(Duration::from_millis(200));
    }
    cx.run_until_parked();
    assert_eq!(ask_session_count(&s, cx), 1);
    let session = ask_session(&s, cx);
    cx.update(|cx| {
        s.manager.update(cx, |m, cx| {
            m.workflow_ask_end(session, Ok("yes".into()), cx)
        })
    });
    cx.run_until_parked();
    assert_eq!(answer.borrow().clone().unwrap(), Ok("yes".to_string()));
}

#[gpui::test]
fn it_says_so_when_the_mono_is_gone(cx: &mut TestAppContext) {
    let s = setup(cx);
    let (tx, _rx) = async_channel::bounded(1);
    let asked = cx.update(|cx| {
        s.manager.update(cx, |m, cx| {
            m.workflow_session("nobody", request(&s), true, tx, cx)
        })
    });
    assert_eq!(asked, Err("The Mono's chat is not available".into()));
}

// --- the host ---------------------------------------------------------------------------------

#[gpui::test]
fn a_followup_into_a_session_with_an_unsent_draft_is_refused(cx: &mut TestAppContext) {
    let s = setup(cx);
    let started = begin(&s, cx, request(&s)).unwrap();
    let host = host_of(&s, cx);
    let answer = collect(cx, host.send(&started.session_id, "Again".into()));
    cx.run_until_parked();
    let outcome = answer.borrow().clone().unwrap();
    assert_eq!(outcome.status, Status::Failed);
    assert_eq!(
        outcome.error.as_deref(),
        Some("Session is busy or has an unsent draft")
    );
    let gone = collect(cx, host.send("424242", "Again".into()));
    cx.run_until_parked();
    assert_eq!(
        gone.borrow().clone().unwrap().error.as_deref(),
        Some("The session is no longer available.")
    );
}

#[gpui::test]
fn a_turn_that_was_never_taken_is_given_up_on_and_an_idle_one_has_ended(cx: &mut TestAppContext) {
    let s = setup(cx);
    let started = begin(&s, cx, request(&s)).unwrap();
    let id = local(&started);
    // Idle and armed: the turn is over.
    let over = cx.update(|cx| {
        s.manager
            .update(cx, |m, cx| m.workflow_settled(id, true, cx))
    });
    let over = collect(cx, over);
    cx.run_until_parked();
    assert_eq!(over.borrow().clone().unwrap().status, Status::Completed);
    // Idle and never seen working: the prompt was not taken, so the wait ends in a failure.
    let never = cx.update(|cx| {
        s.manager
            .update(cx, |m, cx| m.workflow_settled(id, false, cx))
    });
    let never = collect(cx, never);
    cx.run_until_parked();
    assert!(never.borrow().is_none());
    cx.executor().advance_clock(Duration::from_secs(15));
    cx.run_until_parked();
    assert_eq!(
        never.borrow().clone().unwrap().error.as_deref(),
        Some("The session did not start a turn.")
    );
}

#[gpui::test]
fn it_reads_a_sessions_directory_reply_and_waiting_state(cx: &mut TestAppContext) {
    let s = setup(cx);
    let started = begin(&s, cx, request(&s)).unwrap();
    let host = host_of(&s, cx);
    assert_eq!(
        host.worktree_of(&started.session_id),
        Some(s.project.clone())
    );
    assert_eq!(host.final_reply(&started.session_id), "");
    assert!(!host.waiting_approval(&started.session_id));
    let info = host.session_info(&started.session_id).unwrap();
    assert_eq!(
        (info.cwd.as_str(), info.harness.as_str()),
        (s.project.as_str(), "codex")
    );
    assert!(host.session_info("424242").is_none());
    assert_eq!(host.worktree_of("424242"), None);
}

#[gpui::test]
fn a_workflow_cannot_stop_a_mono_chat_but_stops_its_own_sessions(cx: &mut TestAppContext) {
    let s = setup(cx);
    let started = begin(&s, cx, request(&s)).unwrap();
    let host = host_of(&s, cx);
    let refused = collect(cx, host.stop(&s.session.to_string()));
    let stopped = collect(cx, host.stop(&started.session_id));
    cx.run_until_parked();
    assert_eq!(
        refused.borrow().clone().unwrap(),
        Err("The session cannot be stopped from a workflow".into())
    );
    assert_eq!(stopped.borrow().clone().unwrap(), Ok(()));
}

#[gpui::test]
fn a_project_input_resolves_to_one_of_the_monos_projects(cx: &mut TestAppContext) {
    let s = setup(cx);
    let host = host_of(&s, cx);
    assert_eq!(
        host.project_path(&s.id, &s.project),
        Some(s.project.clone())
    );
    assert_eq!(
        host.project_path(&s.id, &format!("  {}  ", s.project)),
        Some(s.project.clone())
    );
    let name = std::path::Path::new(&s.project)
        .file_name()
        .unwrap()
        .to_string_lossy()
        .to_string();
    assert_eq!(host.project_path(&s.id, &name), Some(s.project.clone()));
    assert_eq!(host.project_path(&s.id, ""), None);
    assert_eq!(host.project_path(&s.id, "  "), None);
    assert_eq!(host.project_path(&s.id, "/code/other"), None);
    assert_eq!(host.project_path("nobody", &s.project), None);
}

#[gpui::test]
fn a_sessions_approvals_show_on_the_monos_shelf_until_they_are_withdrawn(cx: &mut TestAppContext) {
    let s = setup(cx);
    let started = begin(&s, cx, request(&s)).unwrap();
    let id = local(&started);
    let host = host_of(&s, cx);
    let ask = ApprovalAsk {
        mono_id: s.id.clone(),
        workflow_id: WF.into(),
        workflow_name: "Ship it".into(),
        run_id: "run-1".into(),
        session_id: started.session_id.clone(),
    };
    host.ask_approval(ask.clone());
    host.ask_approval(ask);
    let shelf = |cx: &mut TestAppContext| {
        cx.update(|cx| {
            let monos = s.manager.read(cx);
            (
                monos.workflow_live.attention(&s.id),
                monos.workflow_attention_views(&s.id, cx).len(),
            )
        })
    };
    // It waits on the shelf; the view itself shows only while it has something to answer.
    assert_eq!(shelf(cx), (vec![id], 0));
    host.end_approvals(&started.session_id);
    assert_eq!(shelf(cx), (vec![], 0));
}

#[gpui::test]
fn the_report_lands_in_the_monos_chat_with_a_card_for_each_session(cx: &mut TestAppContext) {
    let s = setup(cx);
    let started = begin(&s, cx, request(&s)).unwrap();
    let host = host_of(&s, cx);
    let info = host.session_info(&started.session_id).unwrap();
    host.post(Posted {
        mono_id: s.id.clone(),
        workflow_id: WF.into(),
        workflow_name: "Ship it".into(),
        run_id: "run-1".into(),
        title: "Ship it · completed".into(),
        text: "## Ship it · completed\n\nall good".into(),
        launches: vec![info],
    });
    cx.run_until_parked();
    let rows = chat_rows(&s);
    assert!(
        rows.iter()
            .any(|r| r == "## Ship it · completed\n\nall good"),
        "{rows:?}"
    );
    let card = rows
        .iter()
        .find(|r| r.contains("\"type\":\"session\""))
        .expect("a session card");
    assert!(card.contains(&format!("\"sessionId\":{}", local(&started))));
}

#[gpui::test]
fn the_run_history_is_written_to_the_workflows_file_and_replaced_in_place(cx: &mut TestAppContext) {
    let s = setup(cx);
    let host = host_of(&s, cx);
    let def = simple(json!({}));
    let mut run = start_run(&def, RunTrigger::Manual, 1_000, "run-1");
    let saved = collect(cx, host.save_run(&s.id, run.clone()));
    cx.run_until_parked();
    assert_eq!(saved.borrow().clone().unwrap(), Ok(()));
    run.status = RunStatus::Done;
    let again = collect(cx, host.save_run(&s.id, run));
    let other = collect(
        cx,
        host.save_run(&s.id, start_run(&def, RunTrigger::Manual, 2_000, "run-2")),
    );
    cx.run_until_parked();
    assert!(again.borrow().clone().unwrap().is_ok() && other.borrow().clone().unwrap().is_ok());
    let runs = load_runs(&MonoDir::new(&s.root, &s.id), WF).unwrap();
    assert_eq!(
        runs.iter()
            .map(|r| (r.id.as_str(), r.status))
            .collect::<Vec<_>>(),
        [("run-2", RunStatus::Running), ("run-1", RunStatus::Done)]
    );
}

#[gpui::test]
fn a_command_nobody_started_cannot_be_cancelled(cx: &mut TestAppContext) {
    let s = setup(cx);
    let host = host_of(&s, cx);
    let answer = collect(cx, host.cancel_command("run-1:nonce:cmd::1"));
    cx.run_until_parked();
    assert_eq!(answer.borrow().clone().unwrap(), Ok(false));
}

// --- Ask Mono ---------------------------------------------------------------------------------

fn ask_session(s: &Setup, cx: &mut TestAppContext) -> i64 {
    cx.update(|cx| {
        let monos = s.manager.read(cx);
        let ids: Vec<_> = monos.workflow_live.asks.keys().copied().collect();
        assert_eq!(ids.len(), 1, "one hidden question session");
        ids[0]
    })
}

#[gpui::test]
fn a_question_runs_in_a_hidden_session_that_leaves_the_habit_slot_alone(cx: &mut TestAppContext) {
    let s = setup(cx);
    let host = host_of(&s, cx);
    let answer = collect(cx, host.ask_mono(&s.id, "Ship it?".into()));
    cx.run_until_parked();
    let session = ask_session(&s, cx);
    cx.update(|cx| {
        let monos = s.manager.read(cx);
        assert_eq!(monos.active_run, None, "habits keep running");
        assert!(monos.running.is_empty());
        assert!(monos.roster.hidden_sessions.contains(&session));
        assert_eq!(monos.workflow_live.asked_by(session), Some(s.id.as_str()));
        assert!(monos.hidden_session(session));
    });
    assert!(
        locked(&s.store)
            .unwrap()
            .sessions()
            .unwrap()
            .iter()
            .any(|x| x.id == session)
    );
    assert!(answer.borrow().is_none());

    cx.update(|cx| {
        s.manager.update(cx, |m, cx| {
            m.workflow_ask_end(session, Ok("the answer".into()), cx)
        })
    });
    cx.run_until_parked();
    assert_eq!(
        answer.borrow().clone().unwrap(),
        Ok("the answer".to_string())
    );
    // The session is removed after.
    cx.update(|cx| {
        let monos = s.manager.read(cx);
        assert!(monos.workflow_live.asks.is_empty());
        assert!(!monos.roster.hidden_sessions.contains(&session));
    });
    assert!(
        !locked(&s.store)
            .unwrap()
            .sessions()
            .unwrap()
            .iter()
            .any(|x| x.id == session)
    );
}

#[gpui::test]
fn a_question_leaves_nothing_behind_and_its_session_goes_only_once_settled(
    cx: &mut TestAppContext,
) {
    let s = setup(cx);
    let observed = cx.update(|cx| s.manager.read(cx).observations.len());
    let host = host_of(&s, cx);
    let answer = collect(cx, host.ask_mono(&s.id, "Ship it?".into()));
    cx.run_until_parked();
    let session = ask_session(&s, cx);
    // A stopped turn is still finishing when the answer arrives.
    let view = cx.update(|cx| s.manager.read(cx).workflow_live.asks[&session].view.clone());
    cx.update(|cx| view.update(cx, |v, _| v.hold_working_for_test(true)));
    cx.update(|cx| {
        s.manager.update(cx, |m, cx| {
            m.workflow_ask_end(session, Ok("yes".into()), cx)
        })
    });
    cx.run_until_parked();
    assert_eq!(answer.borrow().clone().unwrap(), Ok("yes".to_string()));
    let there = || {
        locked(&s.store)
            .unwrap()
            .sessions()
            .unwrap()
            .iter()
            .any(|x| x.id == session)
    };
    assert!(there(), "still writing: the row stays");
    cx.update(|cx| assert!(s.manager.read(cx).roster.hidden_sessions.contains(&session)));
    cx.executor().advance_clock(Duration::from_secs(2));
    cx.run_until_parked();
    assert!(there());
    cx.update(|cx| view.update(cx, |v, _| v.hold_working_for_test(false)));
    cx.executor().advance_clock(Duration::from_secs(1));
    cx.run_until_parked();
    assert!(!there(), "settled: the row goes");
    cx.update(|cx| {
        let m = s.manager.read(cx);
        assert!(!m.roster.hidden_sessions.contains(&session));
        assert_eq!(m.observations.len(), observed);
        assert_eq!(m.workflow_live.kept(), 0);
    });
}

#[gpui::test]
fn a_question_gives_up_a_little_before_the_run_does(cx: &mut TestAppContext) {
    let s = setup(cx);
    let host = host_of(&s, cx);
    let answer = collect(cx, host.ask_mono(&s.id, "Ship it?".into()));
    cx.run_until_parked();
    let session = ask_session(&s, cx);
    cx.executor().advance_clock(Duration::from_secs(9 * 60));
    cx.run_until_parked();
    assert!(answer.borrow().is_none());
    cx.executor().advance_clock(Duration::from_secs(60));
    cx.run_until_parked();
    assert_eq!(
        answer.borrow().clone().unwrap(),
        Err("The Mono did not answer in time".into())
    );
    cx.update(|cx| assert!(s.manager.read(cx).workflow_live.asks.is_empty()));
    assert!(
        !locked(&s.store)
            .unwrap()
            .sessions()
            .unwrap()
            .iter()
            .any(|x| x.id == session)
    );
}

#[gpui::test]
fn the_question_session_reads_the_app_but_cannot_schedule_work(cx: &mut TestAppContext) {
    let s = setup(cx);
    let host = host_of(&s, cx);
    let _answer = collect(cx, host.ask_mono(&s.id, "Ship it?".into()));
    cx.run_until_parked();
    let session = ask_session(&s, cx);
    let add = json!({"name": "Nightly", "instructions": "Check", "schedule": {"kind": "daily", "time": "09:00"}});
    let refused = call_from(&s, session, "h1", "habits.add", add, cx)
        .try_recv()
        .unwrap();
    assert_eq!(refused, Err("A habit run cannot schedule more runs".into()));
    let refused = call_from(&s, session, "w1", "workflows.list", json!({}), cx)
        .try_recv()
        .unwrap();
    assert_eq!(
        refused,
        Err("Only a Mono's own conversation can manage its workflows".into())
    );
    assert!(
        call_from(&s, session, "h2", "habits.list", json!({}), cx)
            .try_recv()
            .unwrap()
            .is_ok()
    );
}

#[gpui::test]
fn a_question_for_a_mono_that_is_gone_fails_at_once(cx: &mut TestAppContext) {
    let s = setup(cx);
    let host = host_of(&s, cx);
    let answer = collect(cx, host.ask_mono("nobody", "Ship it?".into()));
    cx.run_until_parked();
    assert_eq!(
        answer.borrow().clone().unwrap(),
        Err("The Mono's chat is not available".into())
    );
}

// --- the app side of start/resume/stop --------------------------------------------------------

#[gpui::test]
fn the_app_starts_a_run_through_the_real_host_and_stops_it(cx: &mut TestAppContext) {
    let s = setup(cx);
    let def = workflow(
        vec![
            trigger(),
            json!({"id": "ask", "type": "askMono", "question": "Ship it?", "answers": ["yes", "no"]}),
            report(),
        ],
        vec![edge("trigger", "ask"), edge("ask", "report")],
    );
    let run = start_run(&def, RunTrigger::Manual, 1_000, "run-1");
    let started = cx.update(|cx| {
        s.manager.update(cx, |m, cx| {
            let first = m.start_workflow_run(&s.id, run.clone(), cx);
            let second = m.start_workflow_run(&s.id, run.clone(), cx);
            (first, second)
        })
    });
    assert_eq!(started, (true, false), "one run per workflow");
    cx.run_until_parked();
    cx.update(|cx| {
        let monos = s.manager.read(cx);
        assert!(monos.workflow_signals.is_running(WF));
        assert!(monos.workflows.is_running(WF));
        assert_eq!(monos.active_run, None);
    });
    // The question is waiting in its hidden session; stopping the run abandons it.
    assert_eq!(ask_session_count(&s, cx), 1);
    let stopped = cx.update(|cx| s.manager.update(cx, |m, _| m.stop_workflow_run(WF)));
    assert!(stopped);
    cx.run_until_parked();
    cx.update(|cx| {
        let monos = s.manager.read(cx);
        assert!(!monos.workflows.is_running(WF));
        assert!(!monos.workflow_signals.is_running(WF));
    });
    let runs = load_runs(&MonoDir::new(&s.root, &s.id), WF).unwrap();
    assert_eq!(runs[0].status, RunStatus::Stopped);
    // The stopped run reported once to the Mono's chat.
    assert!(
        chat_rows(&s)
            .iter()
            .any(|r| r.starts_with("## Ship it · stopped")),
        "{:?}",
        chat_rows(&s)
    );
}

fn ask_session_count(s: &Setup, cx: &mut TestAppContext) -> usize {
    cx.update(|cx| s.manager.read(cx).workflow_live.asks.len())
}

// --- commands -----------------------------------------------------------------------------------

#[cfg(unix)]
#[gpui::test]
fn a_command_waits_on_a_thread_of_its_own_so_the_executor_stays_free(cx: &mut TestAppContext) {
    use monocode_core::mono::workflows::command;
    let s = setup(cx);
    cx.executor().allow_parking();
    // `-c` with `/bin/sh`, never the user's login shell.
    let shell = std::env::var_os("SHELL");
    unsafe { std::env::set_var("SHELL", "/nonexistent/monoptah-test-shell") };
    let host = host_of(&s, cx);
    let slot = collect(
        cx,
        host.run_command(
            s.project.clone(),
            "sleep 30".into(),
            120_000,
            "cmd-own-thread".into(),
        ),
    );
    // Were the call made on the executor, this would block until the sleep ended.
    let began = std::time::Instant::now();
    cx.run_until_parked();
    assert!(began.elapsed() < Duration::from_secs(10));
    assert!(slot.borrow().is_none(), "still running");
    // Cancelled before or after the thread registered it, it ends at once either way.
    command::cancel("cmd-own-thread");
    while slot.borrow().is_none() {
        assert!(began.elapsed() < Duration::from_secs(30), "no result");
        cx.run_until_parked();
        std::thread::sleep(Duration::from_millis(20));
    }
    let result = slot.borrow_mut().take().unwrap().unwrap();
    assert!(result.cancelled);
    match shell {
        Some(old) => unsafe { std::env::set_var("SHELL", old) },
        None => unsafe { std::env::remove_var("SHELL") },
    }
}

#[cfg(unix)]
#[gpui::test]
fn a_command_that_cannot_start_reports_why(cx: &mut TestAppContext) {
    let s = setup(cx);
    cx.executor().allow_parking();
    let host = host_of(&s, cx);
    let slot = collect(
        cx,
        host.run_command(
            "relative/dir".into(),
            "true".into(),
            1_000,
            "cmd-bad-cwd".into(),
        ),
    );
    let began = std::time::Instant::now();
    while slot.borrow().is_none() {
        assert!(began.elapsed() < Duration::from_secs(10));
        cx.run_until_parked();
        std::thread::sleep(Duration::from_millis(10));
    }
    let error = slot.borrow_mut().take().unwrap().unwrap_err();
    assert!(error.contains("absolute"), "{error}");
}
