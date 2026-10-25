//! The app-wide model of the Contributors background work. The core reports
//! sync and ticket jobs through sinks that run on worker threads, sometimes
//! while the job manager holds its lock; the sinks only push into unbounded
//! channels (so no event is ever dropped and the sinks never block or call
//! back), and one task per channel feeds this model on the UI thread. The
//! footer chip reads `summary`; the page follows `JobsEvent`.
use gpui::{Context, EventEmitter, Task};
use monocode_core::contributors::{
    api::{self, Contributors},
    jobs::{EventSink, SyncEvent},
    model::{
        sync_indicator::{SyncIndicatorSummary, summarize_sync},
        sync_jobs::{ReconcileTicket, StartJob, SyncJobStore},
    },
    ticket_jobs::{TicketJobEvent, TicketSink},
};
use std::sync::Arc;

/// What subscribers hear besides `cx.notify()` (which fires on every change
/// to the list of jobs).
#[derive(Clone, Debug)]
pub(crate) enum JobsEvent {
    /// A sync ended (done, cancelled or failed; `Cancelled` also for an entry
    /// a reconcile dropped): whoever stored its result can read it.
    Ended(SyncEvent),
    /// Every ticket job event, in order.
    Ticket(TicketJobEvent),
}

struct SyncSink(async_channel::Sender<SyncEvent>);
impl EventSink for SyncSink {
    fn emit(&self, event: &SyncEvent) {
        // Unbounded: never blocks, and only fails once the model is gone.
        let _ = self.0.try_send(event.clone());
    }
}

struct TicketChannel(async_channel::Sender<TicketJobEvent>);
impl TicketSink for TicketChannel {
    fn emit(&self, event: &TicketJobEvent) {
        let _ = self.0.try_send(event.clone());
    }
}

/// Every sync job of every project (`SyncJobStore`) and the last ticket job
/// event, fed by the core's sinks. One per app, so a sync started on the
/// Contributors page is still visible in the footer afterwards.
pub(crate) struct SyncJobs {
    store: SyncJobStore,
    ticket: Option<TicketJobEvent>,
    sync_tx: async_channel::Sender<SyncEvent>,
    ticket_tx: async_channel::Sender<TicketJobEvent>,
    _drain: [Task<()>; 2],
}
impl EventEmitter<JobsEvent> for SyncJobs {}

impl SyncJobs {
    pub(crate) fn new(cx: &mut Context<Self>) -> Self {
        let (sync_tx, sync_rx) = async_channel::unbounded::<SyncEvent>();
        let (ticket_tx, ticket_rx) = async_channel::unbounded::<TicketJobEvent>();
        let sync = cx.spawn(async move |this, cx| {
            while let Ok(event) = sync_rx.recv().await {
                if this
                    .update(cx, |this, cx| this.apply_sync(event, cx))
                    .is_err()
                {
                    break;
                }
            }
        });
        let ticket = cx.spawn(async move |this, cx| {
            while let Ok(event) = ticket_rx.recv().await {
                if this
                    .update(cx, |this, cx| this.apply_ticket(event, cx))
                    .is_err()
                {
                    break;
                }
            }
        });
        Self {
            store: SyncJobStore::new(),
            ticket: None,
            sync_tx,
            ticket_tx,
            _drain: [sync, ticket],
        }
    }

    /// The sink to hand to `Contributors::open`.
    pub(crate) fn sync_sink(&self) -> Arc<dyn EventSink> {
        Arc::new(SyncSink(self.sync_tx.clone()))
    }

    /// The ticket sink to hand to `Contributors::open`.
    pub(crate) fn ticket_sink(&self) -> Arc<dyn TicketSink> {
        Arc::new(TicketChannel(self.ticket_tx.clone()))
    }

    /// What the footer chip shows; `None` when nothing runs or failed.
    pub(crate) fn summary(&self) -> Option<SyncIndicatorSummary> {
        summarize_sync(&self.store.jobs())
    }

    pub(crate) fn apply_sync(&mut self, event: SyncEvent, cx: &mut Context<Self>) {
        let applied = self.store.apply(event);
        if applied.changed {
            cx.notify();
        }
        if let Some(ended) = applied.ended {
            cx.emit(JobsEvent::Ended(ended));
        }
    }

    pub(crate) fn apply_ticket(&mut self, event: TicketJobEvent, cx: &mut Context<Self>) {
        // No notify: the footer chip reads no ticket data, so the shell must not
        // redraw for every progress event. The page follows `JobsEvent::Ticket`.
        self.ticket = Some(event.clone());
        cx.emit(JobsEvent::Ticket(event));
    }

    /// Drops a failed job's entry once the user has seen it; a running job stays.
    pub(crate) fn dismiss(&mut self, event: &SyncEvent, cx: &mut Context<Self>) -> bool {
        let changed = self.store.dismiss(event);
        if changed {
            cx.notify();
        }
        changed
    }

    /// Call before asking the core what runs; pass the ticket to `finish_reconcile`.
    pub(crate) fn begin_reconcile(&self) -> ReconcileTicket {
        self.store.begin_reconcile()
    }

    /// Makes the jobs match the core's listing. Entries the core does not
    /// list are dropped and reported as `Ended` with state `Cancelled`.
    pub(crate) fn finish_reconcile(
        &mut self,
        ticket: ReconcileTicket,
        running: &[SyncEvent],
        cx: &mut Context<Self>,
    ) {
        let before = self.store.jobs();
        let dropped = self.store.finish_reconcile(ticket, running);
        if self.store.jobs() != before {
            cx.notify();
        }
        for event in dropped {
            cx.emit(JobsEvent::Ended(event));
        }
    }

    /// Asks the core what really runs and makes the jobs match it. Run it once
    /// the core is open, after every start and cancel (also when it failed),
    /// and when the window gets focus.
    pub(crate) fn reconcile(&mut self, core: Arc<Contributors>, cx: &mut Context<Self>) {
        self.reconcile_with(move || api::sync_status(&core), cx);
    }

    /// `reconcile` with any way of asking (the page asks through its backend).
    pub(crate) fn reconcile_with(
        &mut self,
        status: impl FnOnce() -> Vec<SyncEvent> + Send + 'static,
        cx: &mut Context<Self>,
    ) {
        let ticket = self.begin_reconcile();
        let work = cx.background_executor().spawn(async move { status() });
        cx.spawn(async move |this, cx| {
            let running = work.await;
            let _ = this.update(cx, |this, cx| this.finish_reconcile(ticket, &running, cx));
        })
        .detach();
    }
}

/// What the page tasks call; the footer chip does not need it.
impl SyncJobs {
    /// Every entry, oldest first.
    #[cfg(test)]
    pub(crate) fn jobs(&self) -> Vec<SyncEvent> {
        self.store.jobs()
    }

    pub(crate) fn job(&self, key: &str) -> Option<&SyncEvent> {
        self.store.get(key)
    }

    /// The latest ticket job event, if any came in.
    #[cfg(test)]
    pub(crate) fn ticket_event(&self) -> Option<&TicketJobEvent> {
        self.ticket.as_ref()
    }

    /// A requested job shows as queued at once; returns its key. Follow with
    /// `api::sync_start`, then `queue_failed` on an error, then `reconcile`.
    pub(crate) fn queue(&mut self, root: &str, job: &StartJob, cx: &mut Context<Self>) -> String {
        let key = self.store.queue(root, job);
        cx.notify();
        key
    }

    /// The core refused the request.
    pub(crate) fn queue_failed(
        &mut self,
        root: &str,
        job: &StartJob,
        error: &str,
        cx: &mut Context<Self>,
    ) {
        self.store.queue_failed(root, job, error);
        cx.notify();
    }

    /// Forgets every entry of a project, e.g. after clearing its synced data.
    pub(crate) fn forget(&mut self, root: &str, cx: &mut Context<Self>) -> bool {
        let changed = self.store.forget(root);
        if changed {
            cx.notify();
        }
        changed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{AppContext as _, TestAppContext};
    use monocode_core::contributors::jobs::{JobState, SyncEvent};
    use monocode_core::contributors::sync::SyncStep;

    pub(crate) fn event(root: &str, percent: u8) -> SyncEvent {
        SyncEvent {
            repo_key: format!("{root}/.git"),
            project: root.rsplit('/').next().unwrap().into(),
            root: root.into(),
            branch: "main".into(),
            from: 1,
            to: 2,
            step: SyncStep::Commits,
            done: 3200,
            total: 12000,
            percent,
            state: JobState::Running,
            error: None,
        }
    }

    #[gpui::test]
    fn events_from_the_sink_reach_the_model(cx: &mut TestAppContext) {
        let jobs = cx.new(SyncJobs::new);
        let sink = jobs.read_with(cx, |jobs, _| jobs.sync_sink());
        assert!(jobs.read_with(cx, |jobs, _| jobs.summary()).is_none());
        // Worker threads call the sink; the model follows on the UI thread.
        std::thread::spawn(move || sink.emit(&event("/work/alpha", 42)))
            .join()
            .unwrap();
        cx.run_until_parked();
        let summary = jobs.read_with(cx, |jobs, _| jobs.summary()).unwrap();
        assert_eq!(summary.percent, Some(42));
        assert!(summary.failed.is_none());
        assert_eq!(
            summary.title,
            "Syncing contributors · alpha · Commits 3,200/12,000"
        );
    }

    #[gpui::test]
    fn no_event_is_dropped_however_many_arrive_before_the_ui_runs(cx: &mut TestAppContext) {
        let jobs = cx.new(SyncJobs::new);
        let sink = jobs.read_with(cx, |jobs, _| jobs.sync_sink());
        for n in 0..500u64 {
            let mut e = event("/work/alpha", 10);
            e.done = n;
            sink.emit(&e);
        }
        let mut done = event("/work/alpha", 100);
        done.state = JobState::Done;
        sink.emit(&done);
        let ended = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let seen = ended.clone();
        let _watch = cx.update(|cx| {
            cx.subscribe(&jobs, move |_, e: &JobsEvent, _| {
                if let JobsEvent::Ended(event) = e {
                    seen.borrow_mut().push(event.state);
                }
            })
        });
        cx.run_until_parked();
        assert_eq!(*ended.borrow(), vec![JobState::Done]);
        assert!(jobs.read_with(cx, |jobs, _| jobs.summary()).is_none());
    }

    #[gpui::test]
    fn two_jobs_combine_and_each_has_a_tooltip_line(cx: &mut TestAppContext) {
        let jobs = cx.new(SyncJobs::new);
        jobs.update(cx, |jobs, cx| {
            jobs.apply_sync(event("/work/alpha", 20), cx);
            let mut beta = event("/work/beta", 60);
            beta.step = SyncStep::Github;
            beta.done = 1;
            beta.total = 4;
            jobs.apply_sync(beta, cx);
        });
        let summary = jobs.read_with(cx, |jobs, _| jobs.summary()).unwrap();
        assert_eq!(summary.percent, Some(40));
        assert_eq!(
            summary.title.split('\n').collect::<Vec<_>>(),
            [
                "Syncing contributors · alpha · Commits 3,200/12,000",
                "Syncing contributors · beta · GitHub 1/4"
            ]
        );
        assert_eq!(summary.target.root, "/work/alpha");
    }

    #[gpui::test]
    fn a_failure_stays_until_dismissed(cx: &mut TestAppContext) {
        let jobs = cx.new(SyncJobs::new);
        jobs.update(cx, |jobs, cx| {
            jobs.apply_sync(event("/work/alpha", 42), cx);
            let mut failed = event("/work/alpha", 42);
            failed.state = JobState::Error;
            failed.error = Some("git exploded".into());
            jobs.apply_sync(failed, cx);
        });
        let summary = jobs.read_with(cx, |jobs, _| jobs.summary()).unwrap();
        assert_eq!(summary.percent, None);
        assert_eq!(
            summary.title,
            "Contributors sync stopped · alpha · git exploded"
        );
        let failed = summary.failed.unwrap();
        assert!(jobs.update(cx, |jobs, cx| jobs.dismiss(&failed, cx)));
        assert!(jobs.read_with(cx, |jobs, _| jobs.summary()).is_none());
        assert!(jobs.read_with(cx, |jobs, _| jobs.jobs()).is_empty());
    }

    #[gpui::test]
    fn dismissing_a_running_job_leaves_it(cx: &mut TestAppContext) {
        let jobs = cx.new(SyncJobs::new);
        let running = event("/work/alpha", 42);
        jobs.update(cx, |jobs, cx| jobs.apply_sync(running.clone(), cx));
        assert!(!jobs.update(cx, |jobs, cx| jobs.dismiss(&running, cx)));
        assert!(jobs.read_with(cx, |jobs, _| jobs.summary()).is_some());
    }

    #[gpui::test]
    fn a_job_running_before_the_listener_was_up_is_seeded_by_the_listing(cx: &mut TestAppContext) {
        let jobs = cx.new(SyncJobs::new);
        jobs.update(cx, |jobs, cx| {
            let ticket = jobs.begin_reconcile();
            jobs.finish_reconcile(ticket, &[event("/work/alpha", 77)], cx);
        });
        let summary = jobs.read_with(cx, |jobs, _| jobs.summary()).unwrap();
        assert_eq!(summary.percent, Some(77));
    }

    #[gpui::test]
    fn a_job_the_core_no_longer_lists_is_dropped_and_reported_cancelled(cx: &mut TestAppContext) {
        let jobs = cx.new(SyncJobs::new);
        let cancelled = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let seen = cancelled.clone();
        let _watch = cx.update(|cx| {
            cx.subscribe(&jobs, move |_, e: &JobsEvent, _| {
                if let JobsEvent::Ended(event) = e {
                    seen.borrow_mut().push((event.root.clone(), event.state));
                }
            })
        });
        jobs.update(cx, |jobs, cx| {
            jobs.apply_sync(event("/work/alpha", 42), cx);
            let ticket = jobs.begin_reconcile();
            jobs.finish_reconcile(ticket, &[], cx);
        });
        assert!(jobs.read_with(cx, |jobs, _| jobs.summary()).is_none());
        assert_eq!(
            *cancelled.borrow(),
            vec![("/work/alpha".to_string(), JobState::Cancelled)]
        );
    }

    #[gpui::test]
    fn an_answer_older_than_an_ended_job_does_not_revive_it(cx: &mut TestAppContext) {
        let jobs = cx.new(SyncJobs::new);
        let running = event("/work/alpha", 42);
        jobs.update(cx, |jobs, cx| {
            jobs.apply_sync(running.clone(), cx);
            let ticket = jobs.begin_reconcile();
            let mut done = running.clone();
            done.state = JobState::Done;
            jobs.apply_sync(done, cx);
            jobs.finish_reconcile(ticket, &[running], cx);
        });
        assert!(jobs.read_with(cx, |jobs, _| jobs.summary()).is_none());
    }

    #[gpui::test]
    fn ticket_events_reach_subscribers_and_are_remembered(cx: &mut TestAppContext) {
        let jobs = cx.new(SyncJobs::new);
        let sink = jobs.read_with(cx, |jobs, _| jobs.ticket_sink());
        let seen = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let log = seen.clone();
        let _watch = cx.update(|cx| {
            cx.subscribe(&jobs, move |_, e: &JobsEvent, _| {
                if let JobsEvent::Ticket(event) = e {
                    log.borrow_mut().push((event.kind.clone(), event.done));
                }
            })
        });
        for done in [1, 2, 3] {
            sink.emit(&monocode_core::contributors::ticket_jobs::TicketJobEvent {
                kind: "fetch".into(),
                state: "running".into(),
                done,
                total: 3,
                failed: 0,
                error: None,
                job_id: 1,
                pending_fetches: 1,
            });
        }
        cx.run_until_parked();
        assert_eq!(
            *seen.borrow(),
            vec![
                ("fetch".into(), 1),
                ("fetch".into(), 2),
                ("fetch".into(), 3)
            ]
        );
        let last = jobs.read_with(cx, |jobs, _| jobs.ticket_event().cloned());
        assert_eq!(last.unwrap().done, 3);
    }

    #[gpui::test]
    fn ticket_events_do_not_wake_job_list_observers(cx: &mut TestAppContext) {
        let jobs = cx.new(SyncJobs::new);
        let notified = std::rc::Rc::new(std::cell::Cell::new(0));
        let count = notified.clone();
        let _watch = cx.update(|cx| cx.observe(&jobs, move |_, _| count.set(count.get() + 1)));
        let sink = jobs.read_with(cx, |jobs, _| jobs.ticket_sink());
        sink.emit(&monocode_core::contributors::ticket_jobs::TicketJobEvent {
            kind: "fetch".into(),
            state: "running".into(),
            done: 1,
            total: 3,
            failed: 0,
            error: None,
            job_id: 1,
            pending_fetches: 1,
        });
        cx.run_until_parked();
        assert_eq!(notified.get(), 0);
        jobs.update(cx, |jobs, cx| jobs.apply_sync(event("/work/alpha", 5), cx));
        assert_eq!(notified.get(), 1);
    }
}
