//! The rules of `useTicketBoard` (useTicketBoard.test.ts, .pending.test.ts and
//! .reclassify.test.ts) as tests of the native state: a fake backend stands in
//! for ClickUp, the job queue and the preferences, and the clock is a cell the
//! tests move. Nothing here reaches the network or the user's files.
use super::{
    backend::Patch,
    fake::{Call, TicketFake, clock, event, now, scope, settings, ticket, ticket_with, user},
    state::{Spaces, TicketEvent, TicketView, WorkspacesState},
};
use crate::contributors::jobs::SyncJobs;
use gpui::{AppContext as _, Context, Entity, TestAppContext};
use monocode_core::contributors::{
    model::period::{PeriodKind, PeriodPick},
    ticket_jobs::{TicketJobEvent, TicketJobStatus},
    tickets::{
        board::ALL_START,
        board_state::{scope_fetch_id, window_of},
        tickets::{ExcludedMember, WorkspaceScope},
    },
};
use std::{cell::Cell, rc::Rc, sync::Arc, time::Duration};

pub(super) struct Rig {
    pub(super) jobs: Entity<SyncJobs>,
    pub(super) view: Entity<TicketView>,
    pub(super) fake: Arc<TicketFake>,
}

impl Rig {
    /// A mounted view on the default fake, settled.
    pub(super) fn new(cx: &mut TestAppContext) -> Self {
        Self::with(cx, TicketFake::new(), false)
    }

    /// `hold` keeps every backend answer back until the test delivers it.
    pub(super) fn with(cx: &mut TestAppContext, fake: Arc<TicketFake>, hold: bool) -> Self {
        let cell = Rc::new(Cell::new(now()));
        let jobs = cx.new(SyncJobs::new);
        let view = cx.new({
            let jobs = jobs.clone();
            let read = clock(&cell);
            move |cx| TicketView::new(jobs, read, cx)
        });
        let backend: Arc<dyn super::backend::TicketBackend> = fake.clone();
        view.update(cx, |view, cx| {
            if hold {
                view.hold_backend_calls();
            }
            view.set_backend(backend, cx);
            view.mount(cx);
        });
        cx.run_until_parked();
        Self { jobs, view, fake }
    }

    pub(super) fn act<R>(
        &self,
        cx: &mut TestAppContext,
        f: impl FnOnce(&mut TicketView, &mut Context<TicketView>) -> R,
    ) -> R {
        let out = self.view.update(cx, f);
        cx.run_until_parked();
        out
    }

    pub(super) fn read<R>(&self, cx: &mut TestAppContext, f: impl FnOnce(&TicketView) -> R) -> R {
        self.view.read_with(cx, |view, _| f(view))
    }

    /// A job event arrives, as the core's sink delivers it.
    pub(super) fn emit(&self, cx: &mut TestAppContext, event: TicketJobEvent) {
        self.jobs
            .update(cx, |jobs, cx| jobs.apply_ticket(event, cx));
        cx.run_until_parked();
    }

    /// The Ticket tab is opened again.
    pub(super) fn remount(&self, cx: &mut TestAppContext) {
        self.act(cx, |view, cx| view.mount(cx));
    }

    /// The estimate's debounce passes.
    pub(super) fn debounce(&self, cx: &mut TestAppContext) {
        cx.executor().advance_clock(Duration::from_millis(300));
        cx.run_until_parked();
    }
}

fn status_with(event: Option<TicketJobEvent>, pending: u32) -> TicketJobStatus {
    TicketJobStatus {
        event,
        pending_fetches: pending,
    }
}

/// A rig that is not connected to ClickUp, so nothing is fetched on its own.
fn offline(cx: &mut TestAppContext, configure: impl FnOnce(&TicketFake)) -> Rig {
    let fake = TicketFake::new();
    fake.with(|s| s.connected.default = Ok(false));
    configure(&fake);
    Rig::with(cx, fake, false)
}

fn pending(rig: &Rig, cx: &mut TestAppContext) -> u32 {
    rig.read(cx, |view| view.pending_fetches)
}

fn job_kind(rig: &Rig, cx: &mut TestAppContext) -> Option<String> {
    rig.read(cx, |view| view.job.as_ref().map(|job| job.kind.clone()))
}

// ---- mounting and the first reads ------------------------------------------------

#[gpui::test]
fn a_mounted_view_reads_the_prefs_the_tickets_and_the_connection(cx: &mut TestAppContext) {
    let rig = Rig::new(cx);
    rig.read(cx, |view| {
        assert!(view.prefs_ready);
        assert_eq!(view.connected, Some(true));
        assert_eq!(view.workspaces.len(), 2);
        assert_eq!(view.workspaces_state, WorkspacesState::Ready);
        assert_eq!(view.period.id, "2026-10");
        assert_eq!(view.ai.as_ref().map(|ai| ai.model.as_str()), Some("sonnet"));
        assert_eq!(view.derived.board.total, 1, "the delivered ticket");
        assert_eq!(view.derived.board.members[0].active, 1, "the working one");
    });
    assert_eq!(rig.fake.loads(), 1);
}

#[gpui::test]
fn the_saved_period_is_the_one_shown(cx: &mut TestAppContext) {
    let fake = TicketFake::new();
    fake.with(|s| {
        s.range = PeriodPick {
            kind: PeriodKind::Quarter,
            id: Some("2026-Q2".into()),
        }
    });
    let rig = Rig::with(cx, fake, false);
    rig.read(cx, |view| assert_eq!(view.period.id, "2026-Q2"));
}

#[gpui::test]
fn nothing_is_read_before_the_backend_and_the_mount_are_there(cx: &mut TestAppContext) {
    let fake = TicketFake::new();
    let cell = Rc::new(Cell::new(now()));
    let jobs = cx.new(SyncJobs::new);
    let view = cx.new({
        let read = clock(&cell);
        move |cx| TicketView::new(jobs, read, cx)
    });
    cx.run_until_parked();
    assert!(fake.calls().is_empty());
    // A backend without a mount reads nothing either.
    let backend: Arc<dyn super::backend::TicketBackend> = fake.clone();
    view.update(cx, |view, cx| view.set_backend(backend, cx));
    cx.run_until_parked();
    assert!(fake.calls().is_empty());
    view.update(cx, |view, cx| view.mount(cx));
    cx.run_until_parked();
    assert!(fake.calls().contains(&Call::Connected));
}

#[gpui::test]
fn a_mount_before_the_backend_starts_when_the_backend_arrives(cx: &mut TestAppContext) {
    let fake = TicketFake::new();
    let cell = Rc::new(Cell::new(now()));
    let jobs = cx.new(SyncJobs::new);
    let view = cx.new({
        let read = clock(&cell);
        move |cx| TicketView::new(jobs, read, cx)
    });
    view.update(cx, |view, cx| view.mount(cx));
    cx.run_until_parked();
    assert!(fake.calls().is_empty());
    let backend: Arc<dyn super::backend::TicketBackend> = fake.clone();
    view.update(cx, |view, cx| view.set_backend(backend, cx));
    cx.run_until_parked();
    assert!(fake.calls().contains(&Call::Connected));
    view.read_with(cx, |view, _| assert_eq!(view.derived.board.total, 1));
}

#[gpui::test]
fn an_answer_for_an_earlier_mount_is_dropped(cx: &mut TestAppContext) {
    let rig = Rig::with(cx, TicketFake::new(), true);
    // Everything of the first mount is held back; the tab is opened again.
    rig.remount(cx);
    let first = rig.read(cx, |view| view.held_labels());
    assert!(first.len() > 4, "{first:?}");
    // Only the answers of the second mount count: release the first ones.
    rig.act(cx, |view, cx| {
        for _ in 0..first.len() / 2 {
            let mut held = view.held.remove(0);
            if let Some(run) = held.run.take() {
                run(view, cx);
            }
        }
    });
    rig.read(cx, |view| {
        assert!(
            !view.prefs_ready,
            "the first mount's prefs answer is dropped"
        );
        assert_eq!(view.connected, None);
    });
    rig.act(cx, |view, cx| view.release_all(cx));
    rig.read(cx, |view| {
        assert!(view.prefs_ready);
        assert_eq!(view.connected, Some(true));
    });
}

#[gpui::test]
fn an_unmounted_view_ignores_job_events_and_queues_nothing(cx: &mut TestAppContext) {
    let rig = Rig::new(cx);
    rig.fake.clear_calls();
    // Mounted: the end of an assessment reloads the stored tickets.
    rig.emit(cx, event("assess", "done", 3, 0));
    assert!(rig.fake.loads() >= 1, "{:?}", rig.fake.calls());
    rig.act(cx, |view, cx| view.unmount(cx));
    rig.fake.clear_calls();
    // Hidden (another tab, the page closed): events, a failed fetch, a window
    // coming to the front and a new status read touch nothing.
    rig.emit(cx, event("assess", "done", 4, 0));
    rig.emit(cx, event("fetch", "error", 5, 0));
    rig.emit(cx, event("fetch", "done", 6, 0));
    rig.act(cx, |view, cx| view.resume(cx));
    rig.debounce(cx);
    assert_eq!(rig.fake.calls(), Vec::<Call>::new());
    assert_eq!(rig.fake.fetches(), 0);
    rig.read(cx, |view| assert!(!view.mounted));
}

#[gpui::test]
fn an_unmounted_view_drops_the_answers_still_on_their_way(cx: &mut TestAppContext) {
    let rig = Rig::with(cx, TicketFake::new(), true);
    rig.act(cx, |view, cx| view.unmount(cx));
    rig.act(cx, |view, cx| view.release_all(cx));
    rig.read(cx, |view| {
        assert!(!view.prefs_ready);
        assert_eq!(view.connected, None);
    });
    assert_eq!(rig.fake.fetches(), 0, "{:?}", rig.fake.calls());
}

#[gpui::test]
fn mounting_again_after_an_unmount_reads_everything_again(cx: &mut TestAppContext) {
    let fake = TicketFake::new();
    fake.with(|s| s.fetch.default = Ok(11));
    let rig = Rig::with(cx, fake, false);
    rig.emit(cx, event("fetch", "done", 11, 0));
    rig.act(cx, |view, cx| view.unmount(cx));
    rig.fake.clear_calls();
    rig.remount(cx);
    assert!(rig.fake.calls().contains(&Call::Connected));
    rig.read(cx, |view| {
        assert!(view.mounted);
        assert!(view.prefs_ready);
        assert_eq!(view.connected, Some(true));
    });
    // The window was fetched before the unmount: fetched windows outlive a mount.
    assert_eq!(rig.fake.fetches(), 0);
    rig.emit(cx, event("assess", "done", 9, 0));
    assert!(rig.fake.loads() >= 1);
}

// ---- the connection -------------------------------------------------------------

#[gpui::test]
fn without_a_connection_there_are_no_workspaces_and_no_fetch(cx: &mut TestAppContext) {
    let rig = offline(cx, |_| {});
    rig.read(cx, |view| {
        assert_eq!(view.connected, Some(false));
        assert!(view.workspaces.is_empty());
        assert_eq!(view.workspaces_state, WorkspacesState::Ready);
    });
    assert_eq!(rig.fake.fetches(), 0);
    assert_eq!(rig.fake.count(|c| *c == Call::Workspaces), 0);
}

#[gpui::test]
fn a_connection_error_is_kept_and_retry_checks_again(cx: &mut TestAppContext) {
    let fake = TicketFake::new();
    fake.with(|s| s.connected.once.push_back(Err("network down".into())));
    let rig = Rig::with(cx, fake, false);
    rig.read(cx, |view| {
        assert_eq!(view.connected, None);
        assert_eq!(view.connect_error.as_deref(), Some("network down"));
    });
    rig.act(cx, |view, cx| view.retry(cx));
    rig.read(cx, |view| {
        assert_eq!(view.connected, Some(true));
        assert_eq!(view.connect_error, None);
        assert_eq!(view.workspaces.len(), 2);
    });
}

#[gpui::test]
fn a_failed_workspace_list_is_an_error_and_retry_reads_it_again(cx: &mut TestAppContext) {
    let fake = TicketFake::new();
    fake.with(|s| s.workspaces.once.push_back(Err("403 forbidden".into())));
    let rig = Rig::with(cx, fake, false);
    rig.read(cx, |view| {
        assert_eq!(view.workspaces_state, WorkspacesState::Error);
        assert_eq!(view.workspaces_error.as_deref(), Some("403 forbidden"));
        assert!(view.workspaces.is_empty());
    });
    rig.act(cx, |view, cx| view.retry(cx));
    rig.read(cx, |view| {
        assert_eq!(view.workspaces_state, WorkspacesState::Ready);
        assert_eq!(view.workspaces_error, None);
    });
}

#[gpui::test]
fn the_workspaces_are_loading_until_the_list_answers(cx: &mut TestAppContext) {
    let rig = Rig::with(cx, TicketFake::new(), true);
    rig.act(cx, |view, cx| view.release("connected", cx));
    rig.read(cx, |view| {
        assert_eq!(view.connected, Some(true));
        assert_eq!(view.workspaces_state, WorkspacesState::Loading);
        assert!(!view.unreachable(), "not while the list loads");
    });
    rig.act(cx, |view, cx| view.release("workspaces", cx));
    rig.read(cx, |view| {
        assert_eq!(view.workspaces_state, WorkspacesState::Ready)
    });
}

#[gpui::test]
fn coming_back_to_the_window_looks_at_the_connection_again(cx: &mut TestAppContext) {
    let rig = Rig::new(cx);
    let lists = rig.fake.count(|c| *c == Call::Workspaces);
    // Nothing changed: the list is not read again.
    rig.act(cx, |view, cx| view.resume(cx));
    assert_eq!(rig.fake.count(|c| *c == Call::Workspaces), lists);
    // ClickUp was disconnected in Settings.
    rig.fake.with(|s| s.connected.once.push_back(Ok(false)));
    rig.act(cx, |view, cx| view.resume(cx));
    rig.read(cx, |view| {
        assert_eq!(view.connected, Some(false));
        assert!(view.workspaces.is_empty());
    });
    // And connected again: the list is read.
    rig.act(cx, |view, cx| view.resume(cx));
    rig.read(cx, |view| {
        assert_eq!(view.connected, Some(true));
        assert_eq!(view.workspaces.len(), 2);
    });
}

#[gpui::test]
fn a_saved_workspace_the_token_does_not_list_is_unreachable_once_the_list_is_known(
    cx: &mut TestAppContext,
) {
    let fake = TicketFake::new();
    fake.with(|s| {
        s.stored = settings(
            Some(WorkspaceScope {
                workspace_id: "gone".into(),
                name: "Old Co".into(),
                ..scope()
            }),
            50.,
        )
    });
    let rig = Rig::with(cx, fake, false);
    rig.read(cx, |view| assert!(view.unreachable()));
}

// ---- the first fetch ---------------------------------------------------------------

#[gpui::test]
fn the_window_is_fetched_once_it_is_known_to_be_connected(cx: &mut TestAppContext) {
    let rig = Rig::new(cx);
    assert_eq!(rig.fake.fetches(), 1);
    let window = rig.read(cx, |view| window_of(&view.period));
    let calls = rig.fake.calls();
    assert!(
        calls.contains(&Call::Fetch {
            scope: scope(),
            from: window.from,
            to: window.to,
        }),
        "{calls:?}"
    );
}

#[gpui::test]
fn no_fetch_is_queued_without_a_workspace(cx: &mut TestAppContext) {
    let fake = TicketFake::new();
    fake.with(|s| s.stored = settings(None, 100.));
    let rig = Rig::with(cx, fake, false);
    assert_eq!(rig.fake.fetches(), 0);
    rig.read(cx, |view| {
        assert!(view.derived.shown.is_empty());
        assert_eq!(view.loaded.as_ref().map(|l| l.tickets.len()), Some(0));
    });
    assert_eq!(rig.fake.loads(), 0, "nothing is read for no workspace");
}

#[gpui::test]
fn a_window_is_not_fetched_while_a_fetch_runs_and_is_after_it_ends(cx: &mut TestAppContext) {
    let rig = Rig::new(cx);
    assert_eq!(rig.fake.fetches(), 1);
    rig.emit(cx, event("fetch", "running", 1, 1));
    rig.act(cx, |view, cx| view.set_kind(PeriodKind::Quarter, cx));
    assert_eq!(rig.fake.fetches(), 1, "skipped while the first runs");
    rig.emit(cx, event("fetch", "done", 1, 0));
    assert_eq!(rig.fake.fetches(), 2, "the window skipped meanwhile");
    let windows: Vec<(String, String)> = rig
        .fake
        .calls()
        .into_iter()
        .filter_map(|c| match c {
            Call::Fetch { from, to, .. } => Some((from, to)),
            _ => None,
        })
        .collect();
    assert_ne!(windows[0], windows[1]);
}

#[gpui::test]
fn a_running_assessment_does_not_block_a_fetch(cx: &mut TestAppContext) {
    let fake = TicketFake::new();
    fake.with(|s| s.status.default = Ok(status_with(Some(event("assess", "running", 4, 0)), 0)));
    let rig = Rig::with(cx, fake, false);
    assert_eq!(rig.fake.fetches(), 1);
}

#[gpui::test]
fn the_job_error_is_cleared_when_a_fetch_starts(cx: &mut TestAppContext) {
    let rig = Rig::new(cx);
    let mut error = event("fetch", "error", 1, 0);
    error.error = Some("ClickUp said no".into());
    rig.emit(cx, error);
    rig.read(cx, |view| assert_eq!(view.error(), Some("ClickUp said no")));
    rig.act(cx, |view, cx| view.set_kind(PeriodKind::Year, cx));
    rig.read(cx, |view| assert_eq!(view.error(), None));
}

// ---- pending fetches (useTicketBoard.pending.test.ts) -----------------------------------

#[gpui::test]
fn takes_the_count_from_the_backend_on_reopen_with_a_fetch_queued_behind_an_assessment(
    cx: &mut TestAppContext,
) {
    let rig = offline(cx, |fake| {
        fake.with(|s| s.status.default = Ok(status_with(Some(event("assess", "running", 4, 1)), 1)))
    });
    assert_eq!(job_kind(&rig, cx).as_deref(), Some("assess"));
    assert_eq!(pending(&rig, cx), 1);
}

#[gpui::test]
fn takes_two_pending_fetches_on_reopen_not_one(cx: &mut TestAppContext) {
    let rig = offline(cx, |fake| {
        fake.with(|s| s.status.default = Ok(status_with(Some(event("fetch", "running", 4, 2)), 2)))
    });
    assert_eq!(pending(&rig, cx), 2);
}

#[gpui::test]
fn starts_at_zero_when_nothing_is_queued_or_running(cx: &mut TestAppContext) {
    let rig = offline(cx, |_| {});
    assert_eq!(pending(&rig, cx), 0);
}

#[gpui::test]
fn follows_the_count_each_queued_and_ended_event_carries(cx: &mut TestAppContext) {
    let rig = offline(cx, |_| {});
    rig.emit(cx, event("fetch", "queued", 1, 1));
    assert_eq!(pending(&rig, cx), 1);
    rig.emit(cx, event("assess", "running", 2, 1));
    rig.emit(cx, event("fetch", "queued", 3, 2));
    assert_eq!(pending(&rig, cx), 2);
    rig.emit(cx, event("fetch", "done", 1, 1));
    assert_eq!(pending(&rig, cx), 1);
    // The assessment ending leaves the queued fetch pending.
    rig.emit(cx, event("assess", "done", 2, 1));
    assert_eq!(pending(&rig, cx), 1);
    rig.emit(cx, event("fetch", "done", 3, 0));
    assert_eq!(pending(&rig, cx), 0);
}

#[gpui::test]
fn ignores_the_count_of_a_progress_event_which_can_be_stale(cx: &mut TestAppContext) {
    let rig = offline(cx, |_| {});
    rig.emit(cx, event("fetch", "queued", 1, 1));
    rig.emit(cx, event("fetch", "running", 1, 0));
    assert_eq!(pending(&rig, cx), 1);
}

#[gpui::test]
fn a_status_read_that_began_earlier_does_not_overwrite_a_newer_event(cx: &mut TestAppContext) {
    let fake = TicketFake::new();
    fake.with(|s| s.connected.default = Ok(false));
    let rig = Rig::with(cx, fake, true);
    rig.emit(cx, event("fetch", "queued", 1, 3));
    rig.act(cx, |view, cx| view.release("status", cx));
    assert_eq!(pending(&rig, cx), 3);
}

// ---- fetched windows (useTicketBoard.pending.test.ts) ------------------------------------

#[gpui::test]
fn marks_the_window_fetched_only_when_its_own_fetch_ends_done_cleanly(cx: &mut TestAppContext) {
    let fake = TicketFake::new();
    fake.with(|s| s.fetch.default = Ok(11));
    let rig = Rig::with(cx, fake, false);
    assert_eq!(rig.fake.fetches(), 1);
    // Queued is not fetched: a reopened view would still fetch it.
    rig.remount(cx);
    rig.emit(cx, event("fetch", "queued", 11, 1));
    assert_eq!(rig.fake.fetches(), 2);
    rig.emit(cx, event("fetch", "done", 11, 0));
    rig.fake.clear_calls();
    rig.remount(cx);
    assert_eq!(rig.fake.fetches(), 0, "fetched windows outlive a mount");
}

#[gpui::test]
fn marks_it_when_the_end_arrives_before_the_commands_answer(cx: &mut TestAppContext) {
    let fake = TicketFake::new();
    let rig = Rig::with(cx, fake, true);
    let sink = rig.jobs.read_with(cx, |jobs, _| jobs.ticket_sink());
    rig.fake.with(|s| {
        s.fetch.default = Ok(12);
        s.on_fetch = Some(Arc::new(move |id| {
            sink.emit(&event("fetch", "done", id, 0));
        }));
    });
    // The first mount's answers, then the fetch call: its end is queued at once,
    // its answer is held back.
    rig.act(cx, |view, cx| {
        view.release("status", cx);
        view.release("prefs", cx);
        view.release("ai", cx);
        view.release("connected", cx);
        view.release("workspaces", cx);
    });
    rig.act(cx, |view, cx| view.release("load", cx));
    assert!(rig.read(cx, |view| view.held_labels().contains(&"fetch")));
    cx.run_until_parked();
    // The end was seen: nothing names it yet.
    rig.act(cx, |view, cx| view.release("fetch", cx));
    rig.act(cx, |view, cx| view.release_all(cx));
    rig.fake.clear_calls();
    rig.fake.with(|s| s.on_fetch = None);
    rig.act(cx, |view, _| view.manual = false);
    rig.remount(cx);
    assert_eq!(rig.fake.fetches(), 0, "the window counts as fetched");
}

fn not_marked_after(cx: &mut TestAppContext, end: TicketJobEvent) {
    let fake = TicketFake::new();
    fake.with(|s| s.fetch.default = Ok(13));
    let rig = Rig::with(cx, fake, false);
    assert_eq!(rig.fake.fetches(), 1);
    // The job is seen queued (the view is busy) and then ends: the effect runs
    // again when the busy state changes, and must not fetch the window again.
    rig.emit(cx, event("fetch", "queued", 13, 1));
    rig.emit(cx, end);
    assert_eq!(rig.fake.fetches(), 1, "no retry on its own");
    rig.fake.clear_calls();
    rig.remount(cx);
    assert_eq!(rig.fake.fetches(), 1, "a reopened view fetches it again");
}

#[gpui::test]
fn does_not_mark_the_window_after_an_error_and_does_not_retry_on_its_own(cx: &mut TestAppContext) {
    let mut end = event("fetch", "error", 13, 0);
    end.error = Some("boom".into());
    not_marked_after(cx, end);
}

#[gpui::test]
fn does_not_mark_the_window_after_a_cancel(cx: &mut TestAppContext) {
    not_marked_after(cx, event("fetch", "cancelled", 13, 0));
}

#[gpui::test]
fn does_not_mark_the_window_after_a_done_with_a_failed_link(cx: &mut TestAppContext) {
    let mut end = event("fetch", "done", 13, 0);
    end.failed = 1;
    not_marked_after(cx, end);
}

#[gpui::test]
fn does_not_mark_a_window_because_another_job_ended(cx: &mut TestAppContext) {
    let fake = TicketFake::new();
    fake.with(|s| s.fetch.default = Ok(14));
    let rig = Rig::with(cx, fake, false);
    rig.emit(cx, event("fetch", "done", 99, 0));
    rig.emit(cx, event("assess", "done", 14, 0));
    rig.fake.clear_calls();
    rig.remount(cx);
    assert_eq!(rig.fake.fetches(), 1);
}

#[gpui::test]
fn refresh_adds_nothing_while_that_windows_fetch_is_pending(cx: &mut TestAppContext) {
    let fake = TicketFake::new();
    fake.with(|s| s.fetch.default = Ok(15));
    let rig = Rig::with(cx, fake, false);
    rig.act(cx, |view, cx| view.refresh(cx));
    assert_eq!(rig.fake.fetches(), 1);
    rig.emit(cx, event("fetch", "done", 15, 0));
    rig.act(cx, |view, cx| view.refresh(cx));
    assert_eq!(rig.fake.fetches(), 2);
}

#[gpui::test]
fn refresh_forgets_a_window_whose_fetch_the_command_refused(cx: &mut TestAppContext) {
    let fake = TicketFake::new();
    fake.with(|s| {
        s.connected.default = Ok(false);
        s.fetch.default = Err("busy".into());
    });
    let rig = Rig::with(cx, fake, false);
    rig.act(cx, |view, cx| view.refresh(cx));
    rig.read(cx, |view| assert_eq!(view.error(), Some("busy")));
    rig.fake.with(|s| s.fetch.default = Ok(16));
    rig.act(cx, |view, cx| view.refresh(cx));
    assert_eq!(rig.fake.fetches(), 2);
    rig.read(cx, |view| assert_eq!(view.error(), None));
}

#[gpui::test]
fn refresh_without_a_workspace_does_nothing(cx: &mut TestAppContext) {
    let fake = TicketFake::new();
    fake.with(|s| s.stored = settings(None, 100.));
    let rig = Rig::with(cx, fake, false);
    rig.act(cx, |view, cx| view.refresh(cx));
    assert_eq!(rig.fake.fetches(), 0);
}

#[gpui::test]
fn refresh_after_a_failed_window_tries_it_again(cx: &mut TestAppContext) {
    let fake = TicketFake::new();
    fake.with(|s| s.fetch.default = Ok(20));
    let rig = Rig::with(cx, fake, false);
    let mut end = event("fetch", "error", 20, 0);
    end.error = Some("boom".into());
    rig.emit(cx, end);
    rig.act(cx, |view, cx| view.refresh(cx));
    assert_eq!(rig.fake.fetches(), 2);
}

#[gpui::test]
fn a_failed_window_is_retried_when_the_connection_changes(cx: &mut TestAppContext) {
    let fake = TicketFake::new();
    fake.with(|s| s.fetch.default = Ok(21));
    let rig = Rig::with(cx, fake, false);
    let mut end = event("fetch", "error", 21, 0);
    end.error = Some("boom".into());
    rig.emit(cx, end);
    assert_eq!(rig.fake.fetches(), 1);
    // Disconnected, then connected again: the failed window is tried again.
    rig.fake.with(|s| s.connected.once.push_back(Ok(false)));
    rig.act(cx, |view, cx| view.resume(cx));
    assert_eq!(rig.fake.fetches(), 1, "nothing without a connection");
    rig.act(cx, |view, cx| view.resume(cx));
    assert_eq!(rig.fake.fetches(), 2);
}

// ---- reloading ----------------------------------------------------------------------------

#[gpui::test]
fn the_stored_tickets_are_read_again_when_a_job_ends(cx: &mut TestAppContext) {
    let rig = Rig::new(cx);
    let loads = rig.fake.loads();
    for (n, state) in ["done", "cancelled", "error"].into_iter().enumerate() {
        rig.emit(cx, event("assess", state, 30 + n as u64, 0));
        assert_eq!(rig.fake.loads(), loads + n + 1, "{state}");
    }
    rig.emit(cx, event("assess", "running", 40, 0));
    assert_eq!(rig.fake.loads(), loads + 3, "progress does not read");
}

#[gpui::test]
fn the_tickets_of_another_window_are_not_shown_under_this_one(cx: &mut TestAppContext) {
    let rig = Rig::with(cx, TicketFake::new(), true);
    rig.act(cx, |view, cx| view.release("prefs", cx));
    // Prefs known, tickets not read yet: nothing is shown.
    rig.read(cx, |view| {
        assert!(view.tickets().is_empty());
        assert_eq!(view.derived.board.total, 0);
    });
    rig.act(cx, |view, cx| view.release("load", cx));
    rig.read(cx, |view| assert_eq!(view.derived.board.total, 1));
    // Another period: the tickets read for this one are not shown under it.
    rig.act(cx, |view, cx| view.set_kind(PeriodKind::Year, cx));
    rig.read(cx, |view| {
        assert!(view.tickets().is_empty());
        assert_eq!(view.derived.board.total, 0);
    });
}

#[gpui::test]
fn a_late_read_of_an_older_window_does_not_replace_the_newer_one(cx: &mut TestAppContext) {
    let fake = TicketFake::new();
    fake.with(|s| {
        s.connected.default = Ok(false);
        s.load.once.push_back(Ok(vec![ticket("old")]));
        s.load.default = Ok(vec![ticket("new")]);
    });
    let rig = Rig::with(cx, fake, true);
    rig.act(cx, |view, cx| view.release("prefs", cx));
    rig.act(cx, |view, cx| view.set_kind(PeriodKind::Quarter, cx));
    assert_eq!(
        rig.read(cx, |view| view
            .held_labels()
            .iter()
            .filter(|l| **l == "load")
            .count()),
        2
    );
    // The newer read answers first; the older one arrives late.
    rig.act(cx, |view, cx| view.release_nth("load", 1, cx));
    rig.act(cx, |view, cx| view.release_nth("load", 0, cx));
    let id = rig.read(cx, |view| view.tickets()[0].id.clone());
    assert_eq!(id, "new");
}

#[gpui::test]
fn a_failed_read_is_shown_but_a_failed_job_is_not_hidden_by_a_good_one(cx: &mut TestAppContext) {
    let fake = TicketFake::new();
    fake.with(|s| s.load.once.push_back(Err("db busy".into())));
    let rig = Rig::with(cx, fake, false);
    rig.read(cx, |view| assert_eq!(view.error(), Some("db busy")));
    // A job fails; the reload that follows succeeds and does not hide it.
    let mut error = event("assess", "error", 50, 0);
    error.error = Some("model refused".into());
    rig.emit(cx, error);
    rig.read(cx, |view| {
        assert_eq!(view.load_error, None);
        assert_eq!(view.error(), Some("model refused"));
    });
}

#[gpui::test]
fn the_job_error_follows_the_events(cx: &mut TestAppContext) {
    let rig = offline(cx, |_| {});
    let mut error = event("assess", "error", 1, 0);
    error.error = Some("old".into());
    rig.emit(cx, error);
    rig.read(cx, |view| {
        assert_eq!(view.job_error.as_deref(), Some("old"))
    });
    rig.emit(cx, event("assess", "cancelled", 1, 0));
    rig.read(cx, |view| {
        assert_eq!(view.job_error.as_deref(), Some("old"))
    });
    rig.emit(cx, event("assess", "queued", 2, 0));
    rig.read(cx, |view| assert_eq!(view.job_error, None));
}

#[gpui::test]
fn an_event_before_the_tab_is_opened_is_not_heard(cx: &mut TestAppContext) {
    let cell = Rc::new(Cell::new(now()));
    let jobs = cx.new(SyncJobs::new);
    let view = cx.new({
        let jobs = jobs.clone();
        let read = clock(&cell);
        move |cx| TicketView::new(jobs, read, cx)
    });
    jobs.update(cx, |jobs, cx| {
        jobs.apply_ticket(event("assess", "running", 1, 0), cx)
    });
    cx.run_until_parked();
    view.read_with(cx, |view, _| assert!(view.job.is_none()));
}

// ---- reclassifying without reloading (useTicketBoard.reclassify.test.ts) ----------------------

#[gpui::test]
fn lists_every_loaded_status_with_its_type(cx: &mut TestAppContext) {
    let rig = Rig::new(cx);
    rig.read(cx, |view| {
        let statuses: Vec<(&str, &str)> = view
            .derived
            .statuses
            .iter()
            .map(|s| (s.name.as_str(), s.status_type.as_str()))
            .collect();
        assert_eq!(
            statuses,
            [("closed", "closed"), ("ready for testing", "custom")]
        );
    });
}

#[gpui::test]
fn reclassifies_on_a_scope_change_without_loading_or_fetching_again(cx: &mut TestAppContext) {
    let rig = Rig::new(cx);
    let (loads, fetches) = (rig.fake.loads(), rig.fake.fetches());
    assert!(loads > 0);
    let base = scope();
    rig.act(cx, |view, cx| {
        view.set_scope(
            Some(WorkspaceScope {
                delivered_statuses: vec!["ready for testing".into()],
                ..base.clone()
            }),
            cx,
        )
    });
    rig.read(cx, |view| {
        assert_eq!(view.derived.board.total, 2);
        assert_eq!(view.derived.board.members[0].tickets, 2);
        assert_eq!(view.derived.board.members[0].active, 0);
    });
    // Unchecking a done status excludes it; the chosen extra still counts.
    rig.act(cx, |view, cx| {
        view.set_scope(
            Some(WorkspaceScope {
                delivered_statuses: vec!["ready for testing".into()],
                excluded_statuses: vec!["closed".into()],
                ..base.clone()
            }),
            cx,
        )
    });
    rig.read(cx, |view| assert_eq!(view.derived.board.total, 1));
    assert_eq!(
        rig.fake.stored().scope.unwrap().excluded_statuses,
        ["closed"]
    );
    assert_eq!((rig.fake.loads(), rig.fake.fetches()), (loads, fetches));
}

fn shared_ticket() -> monocode_core::contributors::tickets::tickets::TicketRecord {
    ticket_with("shared", |t| {
        t.assignees = vec![user("a", "Alpha"), user("b", "Beta")];
        t.override_level = Some("S".into());
    })
}

#[gpui::test]
fn applies_an_exclusion_without_loading_or_fetching_again(cx: &mut TestAppContext) {
    let fake = TicketFake::new();
    let mut tickets = super::fake::stored_tickets();
    tickets.push(shared_ticket());
    fake.with(|s| s.load.default = Ok(tickets));
    let rig = Rig::with(cx, fake, false);
    rig.read(cx, |view| {
        let alpha = view
            .derived
            .board
            .members
            .iter()
            .find(|m| m.id == "a")
            .unwrap();
        assert_eq!(alpha.tickets, 2);
    });
    let (loads, fetches) = (rig.fake.loads(), rig.fake.fetches());
    rig.act(cx, |view, cx| {
        view.set_scope(
            Some(WorkspaceScope {
                excluded_members: vec![ExcludedMember {
                    id: "b".into(),
                    name: "Beta".into(),
                }],
                ..scope()
            }),
            cx,
        )
    });
    rig.read(cx, |view| {
        // Alpha keeps the whole shared ticket; Beta is gone from the board.
        let ids: Vec<&str> = view
            .derived
            .board
            .members
            .iter()
            .map(|m| m.id.as_str())
            .collect();
        assert_eq!(ids, ["a"]);
        assert_eq!(view.derived.board.members[0].tickets, 2);
        assert_eq!(view.derived.board.members[0].points, 8.);
        assert_eq!(view.derived.board.total, 2);
    });
    assert_eq!(
        rig.fake.stored().scope.unwrap().excluded_members,
        [ExcludedMember {
            id: "b".into(),
            name: "Beta".into()
        }]
    );
    assert_eq!((rig.fake.loads(), rig.fake.fetches()), (loads, fetches));
}

fn exclude_alpha() -> WorkspaceScope {
    WorkspaceScope {
        excluded_members: vec![ExcludedMember {
            id: "a".into(),
            name: "Alpha".into(),
        }],
        ..scope()
    }
}

#[gpui::test]
fn drops_a_ticket_whose_assignees_are_all_excluded_from_the_count_the_run_and_the_estimate(
    cx: &mut TestAppContext,
) {
    let rig = Rig::new(cx);
    rig.debounce(cx);
    rig.read(cx, |view| {
        assert_eq!(view.derived.board.total, 1);
        assert_eq!(view.derived.run_ids, ["done"]);
        assert!(view.estimate.is_some());
    });
    rig.act(cx, |view, cx| view.set_scope(Some(exclude_alpha()), cx));
    rig.read(cx, |view| {
        assert_eq!(view.derived.board.total, 0);
        assert!(view.derived.board.members.is_empty());
        assert_eq!(view.derived.to_assess, 0);
        assert!(view.derived.run_ids.is_empty());
        assert!(view.estimate.is_none());
    });
    // Restoring brings the ticket back.
    rig.act(cx, |view, cx| view.set_scope(Some(scope()), cx));
    rig.read(cx, |view| {
        assert_eq!(view.derived.board.total, 1);
        assert_eq!(view.derived.run_ids, ["done"]);
    });
}

#[gpui::test]
fn sends_a_restored_members_tickets_to_the_estimate_again(cx: &mut TestAppContext) {
    let rig = Rig::new(cx);
    rig.debounce(cx);
    rig.fake.clear_calls();
    rig.act(cx, |view, cx| view.set_scope(Some(exclude_alpha()), cx));
    rig.debounce(cx);
    assert!(rig.fake.estimates().is_empty());
    rig.act(cx, |view, cx| view.set_scope(Some(scope()), cx));
    rig.debounce(cx);
    assert_eq!(rig.fake.estimates(), [["done"]]);
    rig.read(cx, |view| {
        assert_eq!(view.estimate.as_ref().map(|e| e.input_tokens), Some(12345))
    });
}

#[gpui::test]
fn saves_a_newer_name_of_an_excluded_member_seen_in_the_loaded_tickets(cx: &mut TestAppContext) {
    let rig = Rig::new(cx);
    rig.act(cx, |view, cx| {
        view.set_scope(
            Some(WorkspaceScope {
                excluded_members: vec![
                    ExcludedMember {
                        id: "a".into(),
                        name: "Old Alpha".into(),
                    },
                    ExcludedMember {
                        id: "left".into(),
                        name: "Left Lee".into(),
                    },
                ],
                ..scope()
            }),
            cx,
        )
    });
    assert_eq!(
        rig.fake.stored().scope.unwrap().excluded_members,
        [
            ExcludedMember {
                id: "a".into(),
                name: "Alpha".into()
            },
            ExcludedMember {
                id: "left".into(),
                name: "Left Lee".into()
            },
        ]
    );
}

#[gpui::test]
fn an_override_only_ticket_counts_as_assessed(cx: &mut TestAppContext) {
    let fake = TicketFake::new();
    fake.with(|s| {
        s.load.default = Ok(vec![
            ticket_with("o", |t| t.override_level = Some("L".into())),
            ticket("plain"),
        ])
    });
    let rig = Rig::with(cx, fake, false);
    rig.read(cx, |view| {
        assert_eq!(view.derived.board.assessed, 1);
        assert_eq!(view.derived.board.members[0].points, 32.);
        // 100%: one more to score.
        assert_eq!(view.derived.to_assess, 1);
    });
}

// ---- the estimate ----------------------------------------------------------------------------

#[gpui::test]
fn the_estimate_waits_for_the_set_of_tickets_to_settle(cx: &mut TestAppContext) {
    let rig = Rig::new(cx);
    assert!(rig.fake.estimates().is_empty(), "not before the debounce");
    cx.executor().advance_clock(Duration::from_millis(200));
    cx.run_until_parked();
    assert!(rig.fake.estimates().is_empty());
    rig.debounce(cx);
    assert_eq!(rig.fake.estimates(), [["done"]]);
}

#[gpui::test]
fn a_changed_set_restarts_the_wait_and_only_the_last_is_estimated(cx: &mut TestAppContext) {
    let fake = TicketFake::new();
    fake.with(|s| {
        s.load.default = Ok(vec![ticket("t1"), ticket("t2")]);
        s.stored = settings(Some(scope()), 50.);
    });
    let rig = Rig::with(cx, fake, false);
    cx.executor().advance_clock(Duration::from_millis(200));
    rig.act(cx, |view, cx| view.set_percent(100., cx));
    cx.executor().advance_clock(Duration::from_millis(200));
    cx.run_until_parked();
    assert!(rig.fake.estimates().is_empty());
    rig.debounce(cx);
    assert_eq!(rig.fake.estimates().len(), 1);
    assert_eq!(rig.fake.estimates()[0].len(), 2);
}

#[gpui::test]
fn nothing_to_score_clears_the_estimate(cx: &mut TestAppContext) {
    let rig = Rig::new(cx);
    rig.debounce(cx);
    rig.read(cx, |view| assert!(view.estimate.is_some()));
    rig.act(cx, |view, cx| view.set_percent(0., cx));
    rig.read(cx, |view| {
        assert!(view.estimate.is_none());
        assert_eq!(view.derived.to_assess, 0);
    });
}

#[gpui::test]
fn a_failed_estimate_is_no_estimate(cx: &mut TestAppContext) {
    let fake = TicketFake::new();
    fake.with(|s| s.estimate.default = Err("db busy".into()));
    let rig = Rig::with(cx, fake, false);
    rig.debounce(cx);
    rig.read(cx, |view| assert!(view.estimate.is_none()));
}

#[gpui::test]
fn an_estimate_for_an_older_set_is_dropped(cx: &mut TestAppContext) {
    let fake = TicketFake::new();
    fake.with(|s| s.load.default = Ok(vec![ticket("t1"), ticket("t2")]));
    let rig = Rig::with(cx, fake, false);
    // The call for the first set is made and its answer held back.
    rig.act(cx, |view, _| view.hold_backend_calls());
    rig.debounce(cx);
    assert_eq!(rig.read(cx, |view| view.held_labels()), ["estimate"]);
    rig.act(cx, |view, cx| view.set_percent(50., cx));
    rig.act(cx, |view, cx| view.release("estimate", cx));
    rig.read(cx, |view| assert!(view.estimate.is_none()));
}

// ---- preferences ----------------------------------------------------------------------------------

#[gpui::test]
fn the_coverage_is_rounded_clamped_and_saved(cx: &mut TestAppContext) {
    let rig = Rig::new(cx);
    for (given, shown) in [
        (250., 100.),
        (-5., 0.),
        (f64::NAN, 0.),
        (49.6, 50.),
        (7., 7.),
    ] {
        rig.act(cx, |view, cx| view.set_percent(given, cx));
        rig.read(cx, |view| {
            assert_eq!(view.settings.percent, shown, "{given}")
        });
        assert_eq!(rig.fake.stored().percent, shown);
    }
}

#[gpui::test]
fn saves_are_written_one_at_a_time_in_order_and_later_ones_merge(cx: &mut TestAppContext) {
    let rig = Rig::new(cx);
    rig.fake.clear_calls();
    rig.act(cx, |view, _| view.hold_backend_calls());
    rig.act(cx, |view, cx| view.set_percent(10., cx));
    rig.act(cx, |view, cx| view.set_percent(20., cx));
    rig.act(cx, |view, cx| view.set_percent(30., cx));
    // The first write is under way; the next two wait and merge.
    let patches = |rig: &Rig| -> Vec<Patch> {
        rig.fake
            .calls()
            .into_iter()
            .filter_map(|c| match c {
                Call::Patch(p) => Some(p),
                _ => None,
            })
            .collect()
    };
    assert_eq!(patches(&rig).len(), 1);
    assert_eq!(patches(&rig)[0].percent, Some(10.));
    rig.act(cx, |view, cx| view.release("patch", cx));
    let written = patches(&rig);
    assert_eq!(written.len(), 2);
    assert_eq!(written[1].percent, Some(30.), "20 never reached the disk");
    rig.act(cx, |view, cx| view.release("patch", cx));
    assert_eq!(rig.fake.stored().percent, 30.);
}

#[gpui::test]
fn points_edited_in_settings_are_taken_after_a_save(cx: &mut TestAppContext) {
    let rig = Rig::new(cx);
    rig.fake.with(|s| s.stored.points.0[0] = 5.);
    rig.act(cx, |view, cx| view.set_percent(60., cx));
    rig.read(cx, |view| assert_eq!(view.settings.points.0[0], 5.));
    // And the save kept them.
    assert_eq!(rig.fake.stored().points.0[0], 5.);
}

#[gpui::test]
fn the_period_is_chosen_stepped_and_saved(cx: &mut TestAppContext) {
    let rig = Rig::new(cx);
    rig.act(cx, |view, cx| view.set_kind(PeriodKind::Quarter, cx));
    rig.read(cx, |view| assert_eq!(view.period.id, "2026-Q4"));
    assert_eq!(rig.fake.with(|s| s.range.clone()).kind, PeriodKind::Quarter);
    assert_eq!(
        rig.fake.with(|s| s.range.id.clone()),
        None,
        "the current one follows the calendar"
    );
    rig.act(cx, |view, cx| view.go_previous(cx));
    rig.read(cx, |view| {
        assert_eq!(view.period.id, "2026-Q3");
        assert!(view.has_next());
    });
    assert_eq!(
        rig.fake.with(|s| s.range.id.clone()),
        Some("2026-Q3".to_string())
    );
    rig.act(cx, |view, cx| view.go_next(cx));
    rig.read(cx, |view| {
        assert_eq!(view.period.id, "2026-Q4");
        assert!(!view.has_next());
    });
    // No next period after the current one.
    rig.act(cx, |view, cx| view.go_next(cx));
    rig.read(cx, |view| assert_eq!(view.period.id, "2026-Q4"));
}

#[gpui::test]
fn all_time_reads_from_the_start_of_clickup_and_has_no_previous_period(cx: &mut TestAppContext) {
    let rig = Rig::new(cx);
    rig.act(cx, |view, cx| view.set_kind(PeriodKind::All, cx));
    let calls = rig.fake.calls();
    assert!(
        calls.contains(&Call::Load {
            workspace: "w1".into(),
            since: ALL_START.into(),
        }),
        "{calls:?}"
    );
    rig.act(cx, |view, cx| view.go_previous(cx));
    rig.read(cx, |view| assert_eq!(view.period.kind, PeriodKind::All));
    rig.read(cx, |view| assert!(!view.has_next()));
}

#[gpui::test]
fn picking_a_workspace_sets_a_fresh_scope(cx: &mut TestAppContext) {
    let rig = Rig::new(cx);
    rig.act(cx, |view, cx| view.pick_workspace("w2", cx));
    assert_eq!(
        rig.fake.stored().scope,
        Some(WorkspaceScope {
            workspace_id: "w2".into(),
            name: "Beta Corp".into(),
            space_ids: None,
            excluded_statuses: vec![],
            delivered_statuses: vec![],
            excluded_members: vec![],
        })
    );
    // Reading and fetching follow the new workspace.
    let calls = rig.fake.calls();
    assert!(
        calls
            .iter()
            .any(|c| matches!(c, Call::Load { workspace, .. } if workspace == "w2"))
    );
    assert!(
        calls
            .iter()
            .any(|c| matches!(c, Call::Fetch { scope, .. } if scope.workspace_id == "w2"))
    );
}

#[gpui::test]
fn picking_the_current_or_an_unknown_workspace_changes_nothing(cx: &mut TestAppContext) {
    let rig = Rig::new(cx);
    rig.fake.clear_calls();
    for id in ["w1", "", "nope"] {
        rig.act(cx, |view, cx| view.pick_workspace(id, cx));
    }
    assert!(rig.fake.calls().is_empty());
}

#[gpui::test]
fn a_fetch_covers_the_spaces_so_narrowing_them_fetches_again(cx: &mut TestAppContext) {
    let rig = Rig::new(cx);
    rig.emit(cx, event("fetch", "done", 1, 0));
    assert_eq!(rig.fake.fetches(), 1);
    rig.act(cx, |view, cx| {
        view.set_scope(
            Some(WorkspaceScope {
                space_ids: Some(vec!["s1".into()]),
                ..scope()
            }),
            cx,
        )
    });
    assert_eq!(rig.fake.fetches(), 2);
    assert_eq!(
        scope_fetch_id(&WorkspaceScope {
            space_ids: Some(vec!["s1".into()]),
            ..scope()
        }),
        "w1:s1"
    );
}

// ---- actions ---------------------------------------------------------------------------------------

#[gpui::test]
fn score_assesses_the_tickets_of_the_next_run(cx: &mut TestAppContext) {
    let rig = Rig::new(cx);
    rig.act(cx, |view, cx| view.score(cx));
    assert!(rig.fake.calls().contains(&Call::Assess {
        ids: vec!["done".into()],
        force: false,
    }));
}

#[gpui::test]
fn score_with_nothing_to_score_does_nothing(cx: &mut TestAppContext) {
    let rig = Rig::new(cx);
    rig.act(cx, |view, cx| view.set_percent(0., cx));
    rig.act(cx, |view, cx| view.score(cx));
    assert_eq!(rig.fake.count(|c| matches!(c, Call::Assess { .. })), 0);
}

#[gpui::test]
fn reassess_forces_the_given_tickets(cx: &mut TestAppContext) {
    let rig = Rig::new(cx);
    rig.act(cx, |view, cx| {
        view.reassess(vec!["done".into(), "x".into()], cx)
    });
    assert!(rig.fake.calls().contains(&Call::Assess {
        ids: vec!["done".into(), "x".into()],
        force: true,
    }));
}

#[gpui::test]
fn a_refused_assessment_is_the_error_and_the_next_action_clears_it(cx: &mut TestAppContext) {
    let rig = Rig::new(cx);
    rig.fake.with(|s| {
        s.assess
            .once
            .push_back(Err("Add an Anthropic API key".into()))
    });
    rig.act(cx, |view, cx| view.score(cx));
    rig.read(cx, |view| {
        assert_eq!(view.error(), Some("Add an Anthropic API key"))
    });
    rig.act(cx, |view, cx| view.score(cx));
    rig.read(cx, |view| assert_eq!(view.error(), None));
}

#[gpui::test]
fn cancel_asks_the_backend_to_stop_the_job(cx: &mut TestAppContext) {
    let rig = Rig::new(cx);
    rig.act(cx, |view, cx| view.cancel(cx));
    assert_eq!(rig.fake.count(|c| *c == Call::Cancel), 1);
}

#[gpui::test]
fn an_override_is_saved_and_the_tickets_are_read_again(cx: &mut TestAppContext) {
    let rig = Rig::new(cx);
    let loads = rig.fake.loads();
    rig.act(cx, |view, cx| {
        view.set_override("done", Some("XL"), Some("big"), cx)
    });
    assert!(rig.fake.calls().contains(&Call::Override {
        id: "done".into(),
        level: Some("XL".into()),
        note: Some("big".into()),
    }));
    assert_eq!(rig.fake.loads(), loads + 1);
}

#[gpui::test]
fn a_refused_override_is_the_error_and_reads_nothing(cx: &mut TestAppContext) {
    let rig = Rig::new(cx);
    let loads = rig.fake.loads();
    rig.fake
        .with(|s| s.overridden.default = Err("Unknown level".into()));
    rig.act(cx, |view, cx| {
        view.set_override("done", Some("Q"), None, cx)
    });
    rig.read(cx, |view| assert_eq!(view.error(), Some("Unknown level")));
    assert_eq!(rig.fake.loads(), loads);
}

// ---- the Scope panel ---------------------------------------------------------------------------------

#[gpui::test]
fn opening_the_scope_lists_the_spaces_of_the_workspace(cx: &mut TestAppContext) {
    let rig = Rig::new(cx);
    rig.read(cx, |view| {
        assert!(view.spaces.is_none(), "not read before it is open")
    });
    rig.act(cx, |view, cx| view.toggle_scope(cx));
    rig.read(cx, |view| {
        assert!(matches!(&view.spaces, Some(Spaces::Ready(list)) if list.len() == 2));
    });
    assert!(rig.fake.calls().contains(&Call::Spaces("w1".into())));
    // Closing drops them; opening reads them again.
    rig.act(cx, |view, cx| view.toggle_scope(cx));
    rig.read(cx, |view| assert!(view.spaces.is_none()));
    rig.act(cx, |view, cx| view.toggle_scope(cx));
    assert_eq!(rig.fake.count(|c| matches!(c, Call::Spaces(_))), 2);
}

#[gpui::test]
fn the_spaces_are_loading_then_failed_when_the_token_cannot_list_them(cx: &mut TestAppContext) {
    let fake = TicketFake::new();
    fake.with(|s| s.spaces.default = Err("403".into()));
    let rig = Rig::with(cx, fake, false);
    rig.act(cx, |view, cx| view.toggle_scope(cx));
    rig.read(cx, |view| {
        assert_eq!(view.spaces, Some(Spaces::Failed("403".into())))
    });
}

#[gpui::test]
fn another_workspace_reads_its_own_spaces(cx: &mut TestAppContext) {
    let rig = Rig::new(cx);
    rig.act(cx, |view, cx| view.toggle_scope(cx));
    rig.act(cx, |view, cx| view.pick_workspace("w2", cx));
    assert!(rig.fake.calls().contains(&Call::Spaces("w2".into())));
}

#[gpui::test]
fn opening_settings_is_the_hosts_business(cx: &mut TestAppContext) {
    let rig = Rig::new(cx);
    let heard = Rc::new(Cell::new(0));
    let count = heard.clone();
    let _subscription = cx.update(|cx| {
        cx.subscribe(&rig.view, move |_, event: &TicketEvent, _| {
            assert_eq!(*event, TicketEvent::OpenSettings);
            count.set(count.get() + 1);
        })
    });
    rig.act(cx, |view, cx| view.open_settings(cx));
    assert_eq!(heard.get(), 1);
}
