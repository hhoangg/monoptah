//! Export report: what `exportReportFile` of `useTicketBoard.ts` does. The
//! previous period only counts as complete when the backend's completed
//! fetches cover its whole span for the scope's spaces (the store would
//! otherwise hold some of its tickets and the comparison would be inflated).
//! When it does not, and no fetch is pending, the window is fetched and waited
//! for; if the coverage still falls short, the report is built without
//! previous tickets so the comparison is left out. All time has no previous
//! period. Then: render, save dialog, write (off the UI thread), reveal.
use super::{backend::TicketBackend, state::TicketView};
use chrono::{DateTime, Local};
use gpui::{AsyncApp, Context, WeakEntity};
use monocode_core::contributors::{
    model::period::{Period, previous_period},
    tickets::{
        board_state::{Window, window_of},
        report::{ReportInput, build_report, render_report_html, report_file_name},
        tickets::{AiConfigStatus, Points, TicketRecord, WorkspaceScope},
    },
};
use std::{path::PathBuf, sync::Arc, time::Duration};

/// The wait for the previous window's fetch before giving up.
pub(super) const PREVIOUS_FETCH_TIMEOUT: Duration = Duration::from_secs(120);

/// What an export waits on: the ends of fetch jobs that leave none pending. The
/// view feeds it from its job events; it is registered before the fetch is
/// queued, so no end is missed.
pub(super) struct ExportWait {
    /// Ends seen before the job id is known: the event can beat the command's answer.
    drained: Vec<u64>,
    ours: Option<u64>,
    wake: async_channel::Sender<()>,
    /// The export that registered it (`TicketView::export_gen`).
    pub(super) token: u64,
}
impl ExportWait {
    pub(super) fn new(wake: async_channel::Sender<()>, token: u64) -> Self {
        Self {
            drained: Vec::new(),
            ours: None,
            wake,
            token,
        }
    }

    /// A fetch job ended and left no fetch pending.
    pub(super) fn drained(&mut self, job_id: u64) {
        self.drained.push(job_id);
        self.check();
    }

    /// The command named our job. Anything that drained from it on (a fetch
    /// queued behind ours keeps us waiting for it too) ends the wait.
    fn named(&mut self, job_id: u64) {
        self.ours = Some(job_id);
        self.check();
    }

    fn check(&self) {
        if let Some(ours) = self.ours
            && self.drained.iter().any(|id| *id >= ours)
        {
            let _ = self.wake.try_send(());
        }
    }
}

/// What a report is built from, taken when Export is pressed.
pub(super) struct ExportInput {
    pub scope: WorkspaceScope,
    pub period: Period,
    pub points: Points,
    pub tickets: Arc<Vec<TicketRecord>>,
    pub ai: Option<AiConfigStatus>,
}

/// Where the save dialog opens: Downloads when there is one, else the home folder.
pub(super) fn export_directory(home: Option<PathBuf>) -> PathBuf {
    let Some(home) = home else {
        return PathBuf::from(".");
    };
    let downloads = home.join("Downloads");
    if downloads.is_dir() { downloads } else { home }
}

impl TicketView {
    /// Builds the report of the period on screen and saves it where the user
    /// says. The outcome is dropped when the workspace or the period changed
    /// meanwhile: it is for another report.
    pub(crate) fn export_report(&mut self, cx: &mut Context<Self>) {
        let Some(scope) = self.settings.scope.clone() else {
            return;
        };
        if self.exporting {
            return;
        }
        self.export_run += 1;
        let run = self.export_run;
        self.export_gen += 1;
        let token = self.export_gen;
        self.exporting = true;
        self.exported = false;
        self.export_error = None;
        cx.notify();
        let input = ExportInput {
            scope,
            period: self.period.clone(),
            points: self.settings.points,
            tickets: self.tickets(),
            ai: self.ai.clone(),
        };
        cx.spawn(async move |this, cx| {
            let result = export_flow(&this, cx, input, token).await;
            let _ = this.update(cx, |this, cx| {
                // A remount or a newer export owns the state now.
                if this.export_gen != token {
                    return;
                }
                this.exporting = false;
                if run == this.export_run {
                    match result {
                        Ok(path) => this.exported = path.is_some(),
                        Err(error) => this.export_error = Some(error),
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }
}

async fn background<T: Send + 'static>(
    cx: &mut AsyncApp,
    work: impl FnOnce() -> T + Send + 'static,
) -> T {
    cx.background_executor().spawn(async move { work() }).await
}

/// Whether the store is complete for a window of the scope, as the backend tracks it.
async fn covers(
    backend: &Arc<dyn TicketBackend>,
    scope: &WorkspaceScope,
    window: &Window,
    cx: &mut AsyncApp,
) -> bool {
    let (backend, scope, window) = (backend.clone(), scope.clone(), window.clone());
    background(cx, move || {
        backend
            .window_covered(&scope, &window.from, &window.to)
            .unwrap_or(false)
    })
    .await
}

/// An assessment holds the one job queue, so a fetch queued now would wait for
/// the whole run. Read from the backend; with no fetch pending, whatever job is
/// live is not a fetch.
async fn assessment_live(backend: &Arc<dyn TicketBackend>, cx: &mut AsyncApp) -> bool {
    let backend = backend.clone();
    background(cx, move || {
        backend
            .status()
            .map(|status| status.event.is_some() && status.pending_fetches == 0)
            .unwrap_or(false)
    })
    .await
}

/// Queues a fetch of a window and waits until the fetch jobs have drained: the
/// first fetch-kind end of this job or a later one that leaves none pending.
/// Gives up after the timeout. Resolves without a verdict; the caller reads
/// the coverage. Does nothing when a fetch is already pending, since a fetch
/// queued ahead would make the first end ambiguous.
async fn fetch_and_wait(
    this: &WeakEntity<TicketView>,
    cx: &mut AsyncApp,
    backend: &Arc<dyn TicketBackend>,
    scope: &WorkspaceScope,
    window: &Window,
    timeout: Duration,
    token: u64,
) {
    let (wake, woken) = async_channel::bounded::<()>(4);
    let timer = {
        let wake = wake.clone();
        let sleep = cx.background_executor().timer(timeout);
        cx.background_executor().spawn(async move {
            sleep.await;
            let _ = wake.send(()).await;
        })
    };
    // Registered before the fetch is queued, and the pending count is read
    // after it: no end is missed and no fetch slips in ahead.
    let pending = this
        .update(cx, |this, _| {
            if this.export_gen != token {
                return None;
            }
            this.export_wait = Some(ExportWait::new(wake, token));
            Some(this.pending_fetches)
        })
        .ok()
        .flatten();
    // A stale export has nothing to wait for (and must not take a newer one's wait).
    let Some(pending) = pending else {
        drop(timer);
        return;
    };
    if pending == 0 {
        let (backend, scope, window) = (backend.clone(), scope.clone(), window.clone());
        let queued = background(cx, move || backend.fetch(&scope, &window.from, &window.to)).await;
        if let Ok(id) = queued {
            let _ = this.update(cx, |this, _| {
                if let Some(wait) = &mut this.export_wait
                    && wait.token == token
                {
                    wait.named(id);
                }
            });
            let _ = woken.recv().await;
        }
    }
    drop(timer);
    let _ = this.update(cx, |this, _| {
        if this
            .export_wait
            .as_ref()
            .is_some_and(|wait| wait.token == token)
        {
            this.export_wait = None;
        }
    });
}

/// Whether `token` still names the export the view is running.
fn current(this: &WeakEntity<TicketView>, cx: &mut AsyncApp, token: u64) -> bool {
    this.read_with(cx, |this, _| this.export_gen == token)
        .unwrap_or(false)
}

async fn export_flow(
    this: &WeakEntity<TicketView>,
    cx: &mut AsyncApp,
    input: ExportInput,
    token: u64,
) -> Result<Option<PathBuf>, String> {
    let gone = || "The Ticket Contributors view was closed.".to_string();
    let (backend, now, timeout) = this
        .read_with(cx, |this, _| {
            (this.backend.clone(), (this.clock)(), this.export_timeout)
        })
        .map_err(|_| gone())?;
    let backend = backend.ok_or_else(|| "ClickUp is not ready yet.".to_string())?;
    let before = previous_period(&input.period);
    let mut previous_tickets: Vec<TicketRecord> = Vec::new();
    let mut previous_incomplete = false;
    if let Some(before) = &before {
        let window = window_of(before);
        let mut complete = covers(&backend, &input.scope, &window, cx).await;
        // No fetch is queued behind a live assessment: export at once, without the comparison.
        if !complete && !assessment_live(&backend, cx).await {
            fetch_and_wait(this, cx, &backend, &input.scope, &window, timeout, token).await;
            if !current(this, cx, token) {
                return Ok(None);
            }
            complete = covers(&backend, &input.scope, &window, cx).await;
        }
        if complete {
            let (backend, workspace, from) = (
                backend.clone(),
                input.scope.workspace_id.clone(),
                window.from.clone(),
            );
            previous_tickets = background(cx, move || backend.load(&workspace, &from)).await?;
        } else {
            previous_incomplete = true;
        }
    }
    let name = if input.scope.name.is_empty() {
        input.scope.workspace_id.clone()
    } else {
        input.scope.name.clone()
    };
    let file = report_file_name(&name, &input.period);
    let html = background(cx, move || {
        render_report_html(&build_report(&report_input(
            input,
            name,
            before,
            previous_tickets,
            previous_incomplete,
            now,
        )))
    })
    .await;
    // The tab was closed or opened again meanwhile: there is nobody to ask.
    if !current(this, cx, token) {
        return Ok(None);
    }
    let directory = export_directory(monocode_core::accounts::home());
    let prompt = cx.update(|cx| cx.prompt_for_new_path(&directory, Some(&file)));
    let path = match prompt.await {
        Ok(Ok(Some(path))) => path,
        Ok(Ok(None)) => return Ok(None),
        Ok(Err(error)) => return Err(error.to_string()),
        Err(_) => return Err("The save dialog could not be opened.".into()),
    };
    let target = path.clone();
    background(cx, move || std::fs::write(&target, html))
        .await
        .map_err(|error| error.to_string())?;
    // The file is saved already; failing to reveal it must not report a failed export.
    let reveal = this.read_with(cx, |this, _| this.reveal.clone());
    if let Ok(reveal) = reveal {
        let shown = path.clone();
        cx.update(|cx| reveal(&shown, cx));
    }
    Ok(Some(path))
}

fn report_input(
    input: ExportInput,
    workspace_name: String,
    previous_period: Option<Period>,
    previous_tickets: Vec<TicketRecord>,
    previous_incomplete: bool,
    generated_at: DateTime<Local>,
) -> ReportInput {
    ReportInput {
        workspace_name,
        period: input.period,
        previous_period,
        scope: input.scope,
        points: input.points,
        tickets: input.tickets.to_vec(),
        previous_tickets,
        ai: input.ai,
        generated_at,
        previous_incomplete,
    }
}
