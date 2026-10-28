//! `exportReportFile` (useTicketBoard.test.ts) as tests of the native export:
//! the previous window's coverage, the fetch it waits for, the report, the
//! save dialog, the write and the reveal. The save dialog is the test
//! platform's; the file goes to a temp folder; nothing reaches ClickUp.
use super::{
    export::export_directory,
    fake::{Call, TicketFake, at, event, scope, ticket_with},
    state_tests::Rig,
};
use gpui::TestAppContext;
use monocode_core::contributors::{
    model::period::{PeriodKind, previous_period},
    ticket_jobs::TicketJobStatus,
    tickets::{board_state::window_of, tickets::WorkspaceScope},
};
use std::{
    cell::RefCell,
    path::{Path, PathBuf},
    rc::Rc,
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

/// A unique, empty temp folder; removed when dropped.
struct TempDir(PathBuf);
impl TempDir {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "monocode-ticket-export-{}-{}-{}",
            std::process::id(),
            monocode_core::time::now_ms(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
    fn file(&self) -> PathBuf {
        self.0.join("report.html")
    }
}
impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

struct Export {
    rig: Rig,
    dir: TempDir,
    revealed: Rc<RefCell<Vec<PathBuf>>>,
}

/// Delivered in the previous month: counts against the current one.
fn previous(id: &str) -> monocode_core::contributors::tickets::tickets::TicketRecord {
    ticket_with(id, |t| {
        t.done_at = Some(at(9, 5, 10));
        t.updated_at = at(9, 5, 10);
    })
}

fn rig(cx: &mut TestAppContext, configure: impl FnOnce(&TicketFake)) -> Export {
    let fake = TicketFake::new();
    configure(&fake);
    let rig = Rig::with(cx, fake, false);
    // The first fetch (the window on screen) is not what these tests count.
    rig.fake.clear_calls();
    let revealed: Rc<RefCell<Vec<PathBuf>>> = Rc::default();
    let log = revealed.clone();
    rig.act(cx, |view, _| {
        view.reveal = Rc::new(move |path, _| log.borrow_mut().push(path.to_path_buf()));
    });
    Export {
        rig,
        dir: TempDir::new(),
        revealed,
    }
}

impl Export {
    fn start(&self, cx: &mut TestAppContext) {
        self.rig.act(cx, |view, cx| view.export_report(cx));
    }

    fn timeout(&self, cx: &mut TestAppContext, ms: u64) {
        self.rig.act(cx, |view, _| {
            view.export_timeout = Duration::from_millis(ms)
        });
    }

    /// The user picks the temp file in the save dialog.
    fn save(&self, cx: &mut TestAppContext) {
        assert!(
            cx.did_prompt_for_new_path(),
            "the save dialog was not asked"
        );
        let file = self.dir.file();
        cx.simulate_new_path_selection(move |_| Some(file));
        cx.run_until_parked();
    }

    fn cancel(&self, cx: &mut TestAppContext) {
        assert!(cx.did_prompt_for_new_path());
        cx.simulate_new_path_selection(|_| None);
        cx.run_until_parked();
    }

    /// Starts, saves and reads what was written.
    fn run(&self, cx: &mut TestAppContext) -> String {
        self.start(cx);
        self.save(cx);
        self.written()
    }

    fn written(&self) -> String {
        std::fs::read_to_string(self.dir.file()).expect("a report was written")
    }

    fn view<R>(
        &self,
        cx: &mut TestAppContext,
        f: impl FnOnce(&super::state::TicketView) -> R,
    ) -> R {
        self.rig.read(cx, f)
    }

    fn windows(&self, cx: &mut TestAppContext) -> (String, String) {
        let before = self.view(cx, |view| previous_period(&view.period).unwrap());
        let window = window_of(&before);
        (window.from, window.to)
    }
}

#[gpui::test]
fn it_uses_the_stored_coverage_and_fetches_nothing_when_it_reaches_back_far_enough(
    cx: &mut TestAppContext,
) {
    let export = rig(cx, |fake| {
        fake.with(|s| {
            s.covered.default = Ok(true);
            s.load.default = Ok(vec![previous("p1")]);
        })
    });
    let (from, to) = export.windows(cx);
    let html = export.run(cx);
    let calls = export.rig.fake.calls();
    assert!(
        calls.contains(&Call::Covered {
            workspace: "w1".into(),
            spaces: None,
            from: from.clone(),
            to,
        }),
        "{calls:?}"
    );
    assert_eq!(export.rig.fake.fetches(), 0);
    assert!(calls.contains(&Call::Load {
        workspace: "w1".into(),
        since: from,
    }));
    assert!(html.contains("<html"));
    assert!(html.contains("vs "));
    assert_eq!(*export.revealed.borrow(), [export.dir.file()]);
    export.view(cx, |view| {
        assert!(view.exported);
        assert!(!view.exporting);
        assert_eq!(view.export_error, None);
    });
}

#[gpui::test]
fn it_decides_from_the_backend_coverage_alone(cx: &mut TestAppContext) {
    let export = rig(cx, |fake| {
        fake.with(|s| s.load.default = Ok(vec![previous("p1")]))
    });
    export.timeout(cx, 10);
    export.start(cx);
    cx.executor().advance_clock(Duration::from_millis(10));
    cx.run_until_parked();
    export.save(cx);
    assert!(export.rig.fake.count(|c| matches!(c, Call::Covered { .. })) >= 1);
    assert_eq!(
        export.rig.fake.count(|c| matches!(c, Call::Load { .. })),
        0,
        "nothing in the session decides this"
    );
    assert!(!export.written().contains("vs "));
}

#[gpui::test]
fn it_asks_about_the_scopes_spaces(cx: &mut TestAppContext) {
    let export = rig(cx, |fake| {
        fake.with(|s| {
            s.covered.default = Ok(true);
            s.stored.scope = Some(WorkspaceScope {
                space_ids: Some(vec!["s2".into(), "s1".into()]),
                ..scope()
            });
        })
    });
    let (from, to) = export.windows(cx);
    export.run(cx);
    assert!(export.rig.fake.calls().contains(&Call::Covered {
        workspace: "w1".into(),
        spaces: Some(vec!["s2".into(), "s1".into()]),
        from,
        to,
    }));
}

#[gpui::test]
fn it_fetches_the_previous_window_when_the_coverage_starts_later_waits_then_reads_it_again(
    cx: &mut TestAppContext,
) {
    let export = rig(cx, |fake| {
        fake.with(|s| {
            s.covered.once.push_back(Ok(false));
            s.covered.default = Ok(true);
            s.fetch.default = Ok(7);
            s.load.default = Ok(vec![previous("p1")]);
        })
    });
    let (from, to) = export.windows(cx);
    export.start(cx);
    // Under way: the fetch is queued and the export waits for its end.
    assert!(export.rig.fake.calls().contains(&Call::Fetch {
        scope: scope(),
        from: from.clone(),
        to,
    }));
    assert!(!cx.did_prompt_for_new_path());
    export.view(cx, |view| assert!(view.exporting));
    export.rig.emit(cx, event("fetch", "done", 7, 0));
    export.save(cx);
    assert_eq!(
        export.rig.fake.count(|c| matches!(c, Call::Covered { .. })),
        2
    );
    assert!(export.rig.fake.calls().contains(&Call::Load {
        workspace: "w1".into(),
        since: from,
    }));
    assert!(export.written().contains("vs "));
}

#[gpui::test]
fn an_older_export_does_not_clear_the_wait_or_the_button_of_a_newer_one(cx: &mut TestAppContext) {
    let export = rig(cx, |fake| {
        fake.with(|s| {
            // The first export and the second one both find the window uncovered.
            s.covered.once.push_back(Ok(false));
            s.covered.once.push_back(Ok(false));
            s.covered.default = Ok(true);
            s.fetch.default = Ok(7);
            s.load.default = Ok(vec![previous("p1")]);
        })
    });
    // The first export waits for a fetch that never ends; it gives up soon.
    export.timeout(cx, 10);
    export.start(cx);
    export.view(cx, |view| {
        assert!(view.exporting && view.export_wait.is_some())
    });
    // The tab is opened again, which starts everything over; the second export
    // waits for its own fetch, with the long timeout.
    export.timeout(cx, 120_000);
    export.rig.remount(cx);
    export.view(cx, |view| assert!(!view.exporting));
    export.start(cx);
    export.view(cx, |view| {
        assert!(view.exporting && view.export_wait.is_some())
    });
    // The first export's timeout passes: it must leave the second one alone.
    cx.executor().advance_clock(Duration::from_millis(10));
    cx.run_until_parked();
    export.view(cx, |view| {
        assert!(view.exporting, "the older export re-enabled the button");
        assert!(
            view.export_wait.is_some(),
            "the older export cleared the wait"
        );
    });
    assert!(
        !cx.did_prompt_for_new_path(),
        "the older export asked where to save"
    );
    // The second export still ends with its own fetch.
    export.rig.emit(cx, event("fetch", "done", 7, 0));
    export.save(cx);
    export.view(cx, |view| {
        assert!(view.exported);
        assert!(!view.exporting);
    });
}

#[gpui::test]
fn it_sees_the_end_that_arrives_before_the_commands_answer(cx: &mut TestAppContext) {
    let export = rig(cx, |fake| {
        fake.with(|s| {
            s.covered.once.push_back(Ok(false));
            s.covered.default = Ok(true);
            s.fetch.default = Ok(7);
            s.load.default = Ok(vec![previous("p1")]);
        })
    });
    let sink = export.rig.jobs.read_with(cx, |jobs, _| jobs.ticket_sink());
    export.rig.fake.with(|s| {
        s.on_fetch = Some(std::sync::Arc::new(move |id| {
            sink.emit(&event("fetch", "done", id, 0));
        }))
    });
    export.timeout(cx, 5000);
    // The end is queued inside the fetch call; the export goes on without any
    // further event.
    export.start(cx);
    export.save(cx);
    assert!(export.written().contains("vs "));
}

#[gpui::test]
fn it_leaves_the_comparison_out_when_the_coverage_is_still_short_after_the_fetch(
    cx: &mut TestAppContext,
) {
    let export = rig(cx, |fake| {
        fake.with(|s| {
            s.fetch.default = Ok(7);
            s.load.default = Ok(vec![previous("p1")]);
        })
    });
    let (from, _) = export.windows(cx);
    export.start(cx);
    export.rig.emit(cx, event("fetch", "error", 7, 0));
    export.save(cx);
    // The failed job makes the view read the window on screen; not the previous one.
    let calls = export.rig.fake.calls();
    assert!(
        !calls
            .iter()
            .any(|c| matches!(c, Call::Load { since, .. } if *since == from)),
        "{calls:?}"
    );
    assert!(!export.written().contains("vs "));
}

#[gpui::test]
fn it_leaves_the_comparison_out_when_the_wait_times_out(cx: &mut TestAppContext) {
    let export = rig(cx, |_| {});
    export.timeout(cx, 20);
    export.start(cx);
    assert!(!cx.did_prompt_for_new_path(), "still waiting");
    cx.executor().advance_clock(Duration::from_millis(20));
    cx.run_until_parked();
    export.save(cx);
    assert_eq!(export.rig.fake.fetches(), 1);
    assert_eq!(export.rig.fake.count(|c| matches!(c, Call::Load { .. })), 0);
    assert!(!export.written().contains("vs "));
}

#[gpui::test]
fn it_does_not_wait_for_an_earlier_jobs_end_or_one_that_leaves_fetches_pending(
    cx: &mut TestAppContext,
) {
    let export = rig(cx, |fake| fake.with(|s| s.fetch.default = Ok(7)));
    export.timeout(cx, 60);
    export.start(cx);
    // An earlier job (id 3) ending, and ours ending with another still pending.
    export.rig.emit(cx, event("fetch", "done", 3, 0));
    export.rig.emit(cx, event("fetch", "done", 7, 1));
    cx.executor().advance_clock(Duration::from_millis(59));
    cx.run_until_parked();
    assert!(!cx.did_prompt_for_new_path(), "neither ends the wait");
    cx.executor().advance_clock(Duration::from_millis(1));
    cx.run_until_parked();
    assert!(cx.did_prompt_for_new_path(), "the timeout does");
    export.cancel(cx);
}

#[gpui::test]
fn it_waits_for_a_fetch_queued_behind_its_own(cx: &mut TestAppContext) {
    let export = rig(cx, |fake| {
        fake.with(|s| {
            s.covered.once.push_back(Ok(false));
            s.covered.default = Ok(true);
            s.fetch.default = Ok(7);
            s.load.default = Ok(vec![previous("p1")]);
        })
    });
    export.timeout(cx, 5000);
    export.start(cx);
    export.rig.emit(cx, event("fetch", "done", 7, 1));
    assert!(
        !cx.did_prompt_for_new_path(),
        "another fetch is still pending"
    );
    export.rig.emit(cx, event("fetch", "done", 8, 0));
    export.save(cx);
    assert!(export.written().contains("vs "));
}

#[gpui::test]
fn it_queues_no_fetch_while_an_assessment_is_live_and_says_why_there_is_no_comparison(
    cx: &mut TestAppContext,
) {
    let export = rig(cx, |fake| {
        fake.with(|s| {
            s.load.default = Ok(vec![previous("p1")]);
            s.status.default = Ok(TicketJobStatus {
                event: Some(event("assess", "running", 9, 0)),
                pending_fetches: 0,
            });
        })
    });
    export.start(cx);
    // At once: nothing is waited for.
    export.save(cx);
    assert_eq!(export.rig.fake.fetches(), 0);
    assert_eq!(export.rig.fake.count(|c| matches!(c, Call::Load { .. })), 0);
    let html = export.written();
    assert!(!html.contains("vs "));
    assert!(html.contains("No comparison with Sep 2026"));
}

#[gpui::test]
fn it_shows_the_comparison_and_no_note_when_the_previous_window_is_covered(
    cx: &mut TestAppContext,
) {
    let export = rig(cx, |fake| {
        fake.with(|s| {
            s.covered.default = Ok(true);
            s.load.default = Ok(vec![previous("p1")]);
            // Even with an assessment live: nothing needs fetching.
            s.status.default = Ok(TicketJobStatus {
                event: Some(event("assess", "running", 9, 0)),
                pending_fetches: 0,
            });
        })
    });
    let html = export.run(cx);
    assert!(html.contains("vs "));
    assert!(!html.contains("No comparison with"));
}

#[gpui::test]
fn it_notes_the_missing_comparison_and_queues_nothing_while_a_fetch_is_pending(
    cx: &mut TestAppContext,
) {
    let export = rig(cx, |fake| {
        fake.with(|s| s.load.default = Ok(vec![previous("p1")]))
    });
    export.rig.emit(cx, event("fetch", "queued", 9, 1));
    export.rig.fake.clear_calls();
    let html = export.run(cx);
    assert_eq!(export.rig.fake.fetches(), 0);
    assert_eq!(export.rig.fake.count(|c| matches!(c, Call::Load { .. })), 0);
    assert!(!html.contains("vs "));
    assert!(html.contains("No comparison with Sep 2026"));
}

#[gpui::test]
fn it_skips_the_previous_period_for_all_time(cx: &mut TestAppContext) {
    let export = rig(cx, |_| {});
    export
        .rig
        .act(cx, |view, cx| view.set_kind(PeriodKind::All, cx));
    export.rig.fake.clear_calls();
    export.run(cx);
    assert_eq!(
        export.rig.fake.count(|c| matches!(c, Call::Covered { .. })),
        0
    );
    assert_eq!(export.rig.fake.fetches(), 0);
    assert_eq!(export.rig.fake.count(|c| matches!(c, Call::Load { .. })), 0);
    assert!(Path::new(&export.dir.file()).exists());
}

#[gpui::test]
fn it_treats_a_failing_coverage_read_as_not_covered(cx: &mut TestAppContext) {
    let export = rig(cx, |fake| {
        fake.with(|s| {
            s.covered.default = Err("db busy".into());
            s.load.default = Ok(vec![previous("p1")]);
        })
    });
    export.timeout(cx, 10);
    export.start(cx);
    cx.executor().advance_clock(Duration::from_millis(10));
    cx.run_until_parked();
    export.save(cx);
    assert_eq!(export.rig.fake.count(|c| matches!(c, Call::Load { .. })), 0);
    assert!(!export.written().contains("vs "));
}

#[gpui::test]
fn it_writes_nothing_when_the_dialog_is_cancelled(cx: &mut TestAppContext) {
    let export = rig(cx, |fake| fake.with(|s| s.covered.default = Ok(true)));
    export.start(cx);
    export.cancel(cx);
    assert!(!export.dir.file().exists());
    assert!(export.revealed.borrow().is_empty());
    export.view(cx, |view| {
        assert!(!view.exported, "nothing to say");
        assert_eq!(view.export_error, None);
        assert!(!view.exporting);
    });
}

#[gpui::test]
fn it_reports_an_error_when_the_file_cannot_be_written(cx: &mut TestAppContext) {
    let export = rig(cx, |fake| fake.with(|s| s.covered.default = Ok(true)));
    export.start(cx);
    let missing = export.dir.0.join("missing").join("report.html");
    cx.simulate_new_path_selection(move |_| Some(missing));
    cx.run_until_parked();
    assert!(export.revealed.borrow().is_empty());
    export.view(cx, |view| {
        assert!(!view.exported);
        assert!(view.export_error.is_some());
        assert!(!view.exporting);
    });
}

#[gpui::test]
fn a_failing_read_of_the_previous_tickets_is_the_error(cx: &mut TestAppContext) {
    let export = rig(cx, |fake| {
        fake.with(|s| {
            s.covered.default = Ok(true);
            s.load.default = Err("db busy".into());
        })
    });
    export.start(cx);
    assert!(!cx.did_prompt_for_new_path());
    export.view(cx, |view| {
        assert_eq!(view.export_error.as_deref(), Some("db busy"));
        assert!(!view.exporting);
    });
}

// ---- what the view shows of it ----------------------------------------------------------

#[gpui::test]
fn the_button_state_follows_the_export(cx: &mut TestAppContext) {
    let export = rig(cx, |fake| fake.with(|s| s.covered.default = Ok(true)));
    export.view(cx, |view| {
        assert!(!view.exporting && !view.exported);
    });
    export.start(cx);
    export.view(cx, |view| assert!(view.exporting, "Preparing report..."));
    // A second press while it runs does nothing.
    export.rig.act(cx, |view, cx| view.export_report(cx));
    export.save(cx);
    export.view(cx, |view| {
        assert!(!view.exporting);
        assert!(view.exported, "Report saved.");
    });
}

#[gpui::test]
fn a_new_export_forgets_the_saved_message(cx: &mut TestAppContext) {
    let export = rig(cx, |fake| fake.with(|s| s.covered.default = Ok(true)));
    export.run(cx);
    export.view(cx, |view| assert!(view.exported));
    export.start(cx);
    export.view(cx, |view| {
        assert!(!view.exported);
        assert_eq!(view.export_error, None);
    });
    export.cancel(cx);
}

#[gpui::test]
fn the_saved_message_goes_when_the_period_changes(cx: &mut TestAppContext) {
    let export = rig(cx, |fake| fake.with(|s| s.covered.default = Ok(true)));
    export.run(cx);
    export.view(cx, |view| assert!(view.exported));
    export.rig.act(cx, |view, cx| view.go_previous(cx));
    export.view(cx, |view| assert!(!view.exported));
}

#[gpui::test]
fn an_export_that_finishes_after_the_period_changed_is_for_another_report(cx: &mut TestAppContext) {
    let export = rig(cx, |fake| fake.with(|s| s.covered.default = Ok(true)));
    export.start(cx);
    export.rig.act(cx, |view, cx| view.go_previous(cx));
    export.save(cx);
    export.view(cx, |view| {
        assert!(!view.exporting);
        assert!(!view.exported, "its outcome is dropped");
        assert_eq!(view.export_error, None);
    });
}

#[gpui::test]
fn an_error_of_an_export_for_another_workspace_is_dropped(cx: &mut TestAppContext) {
    let export = rig(cx, |fake| fake.with(|s| s.covered.default = Ok(true)));
    export.start(cx);
    export.rig.act(cx, |view, cx| view.pick_workspace("w2", cx));
    let missing = export.dir.0.join("missing").join("report.html");
    cx.simulate_new_path_selection(move |_| Some(missing));
    cx.run_until_parked();
    export.view(cx, |view| {
        assert_eq!(view.export_error, None);
        assert!(!view.exporting);
    });
}

#[test]
fn the_save_dialog_opens_in_downloads_when_there_is_one_else_at_home() {
    let home = TempDir::new();
    assert_eq!(export_directory(Some(home.0.clone())), home.0);
    std::fs::create_dir(home.0.join("Downloads")).unwrap();
    assert_eq!(
        export_directory(Some(home.0.clone())),
        home.0.join("Downloads")
    );
    assert_eq!(export_directory(None), PathBuf::from("."));
}
