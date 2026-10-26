//! When Mono workflows run: the app side of `useMonoWorkflows.ts`. The native Mono clock ticks every
//! 10 s, so workflows have a clock of their own (60 s, as in TS) plus a waker the `workflows.*`
//! actions ring, so a `run`, `stop` or `resumeRun` acts at once instead of on the next look.
//!
//! A look reads every Mono's workflow files and probes the model catalog, so it runs off the UI
//! thread (`core::tick`); what it found is started here, on the UI thread, through the runner.
//! Only one look goes at a time; one asked for meanwhile follows as soon as it ends. The first look
//! after startup also marks runs a previous session left `running` as interrupted, and deletes
//! the hidden Ask-Mono sessions a previous quit left behind.
use super::{
    workflow_runner::{Posted, WorkflowRunner},
    *,
};
use monocode_core::mono::workflows::{
    models::Probe,
    store::MonoDir,
    tick::{self, Found, Start, Tick},
};
use std::path::Path;

/// How often workflows are looked at besides when asked.
const CHECK_MS: u64 = 60_000;

#[derive(Default)]
pub(super) struct Checks {
    /// A look is going.
    checking: bool,
    /// A look was asked for meanwhile.
    again: bool,
    /// The sweep for interrupted runs went through for every Mono.
    swept: bool,
    asks_swept: bool,
    _tasks: Vec<gpui::Task<()>>,
}

/// What a look over all Monos found.
#[derive(Default)]
pub(super) struct Looked {
    /// The sweep ran (it was asked for) and met no error.
    swept: bool,
    monos: Vec<(String, Found)>,
    errors: Vec<String>,
}

/// One look at every Mono's files. Blocks (files, CLI probes): call it off the UI thread.
/// `monos` is `(id, has a chat)`; `driven` the runs the runner is driving, which the sweep leaves
/// alone.
pub(super) fn look_at(
    root: &Path,
    monos: &[(String, bool)],
    driven: &[(String, String)],
    sweep: bool,
    signals: &monocode_core::mono::workflows::cli::Signals,
    probe: fn(Kind) -> Probe,
    now: u64,
) -> Looked {
    let mut looked = Looked {
        swept: sweep,
        ..Default::default()
    };
    for (id, has_chat) in monos {
        let dir = MonoDir::new(root, id.clone());
        if sweep && let Err(error) = tick::sweep_interrupted(&dir, driven) {
            looked.swept = false;
            looked.errors.push(format!(
                "Could not check for interrupted workflow runs: {error}"
            ));
        }
        let running = |workflow: &str| signals.is_running(workflow);
        let tick = Tick {
            now,
            is_running: &running,
            has_chat: *has_chat,
            probe,
            on_claim: &|workflow: &str| signals.set_running(workflow, true),
        };
        match tick::check(&dir, &tick) {
            Ok(found) if found.starts.is_empty() && found.stops.is_empty() => {}
            Ok(found) => looked.monos.push((id.clone(), found)),
            Err(error) => looked
                .errors
                .push(format!("Could not check Mono workflows: {error}")),
        }
    }
    looked
}

/// Deletes the sessions of Ask-Mono questions. Only ids the roster recorded when the question
/// started are given, so no other session is ever touched. Returns the ids that could not be
/// deleted; one already gone counts as deleted, and a failure does not hold up the others.
pub(super) fn delete_asks(store: &Database, ids: &[i64]) -> Result<Vec<i64>, String> {
    let mut db = locked(store)?;
    let existing: HashSet<i64> = db.sessions()?.into_iter().map(|s| s.id).collect();
    let mut failed = Vec::new();
    for &id in ids {
        if !existing.contains(&id) {
            continue;
        }
        match db.delete(id) {
            Ok(()) => {}
            // A shared store reports an id it no longer knows this way.
            Err(error) if error == "Session no longer exists" => {}
            Err(error) => {
                eprintln!("Could not remove workflow question session {id}: {error}");
                failed.push(id);
            }
        }
    }
    Ok(failed)
}

impl Monos {
    /// Starts the clock and the waker. Called once, when the manager is made.
    pub(super) fn start_workflow_checks(&mut self, cx: &mut Context<Self>) {
        let (wake, woken) = async_channel::bounded::<()>(1);
        self.workflow_signals.set_waker(move || {
            let _ = wake.try_send(());
        });
        let asked = cx.spawn(async move |this, cx| {
            while woken.recv().await.is_ok() {
                if this
                    .update(cx, |this, cx| this.check_workflows(cx))
                    .is_err()
                {
                    break;
                }
            }
        });
        let clock = cx.spawn(async move |this, cx| {
            loop {
                if this
                    .update(cx, |this, cx| this.check_workflows(cx))
                    .is_err()
                {
                    break;
                }
                cx.background_executor()
                    .timer(Duration::from_millis(CHECK_MS))
                    .await;
            }
        });
        self.workflow_checks._tasks = vec![asked, clock];
    }

    /// Looks at every Mono's workflows now: requests in the files, due schedules, runs to resume.
    pub(super) fn check_workflows(&mut self, cx: &mut Context<Self>) {
        // Once the app is quitting nothing new is claimed, started or written.
        if self.workflows.is_quitting() {
            return;
        }
        if self.workflow_checks.checking {
            self.workflow_checks.again = true;
            return;
        }
        self.workflow_checks.checking = true;
        self.workflow_checks.again = false;
        // Whatever was asked for before this look is answered by it.
        self.workflow_signals.take_check_now();
        self.sweep_asks(cx);
        let monos: Vec<(String, bool)> = self
            .roster
            .monos
            .iter()
            .map(|m| (m.id.clone(), m.session_id.is_some()))
            .collect();
        let driven = self.workflows.running();
        let sweep = !self.workflow_checks.swept;
        let signals = self.workflow_signals.clone();
        let root = self.root.clone();
        let probe = self.workflow_live.probe();
        let work = cx.background_executor().spawn(async move {
            look_at(
                &root,
                &monos,
                &driven,
                sweep,
                &signals,
                probe,
                monocode_core::time::now_ms(),
            )
        });
        cx.spawn(async move |this, cx| {
            let looked = work.await;
            let _ = this.update(cx, |this, cx| this.workflows_looked(looked, cx));
        })
        .detach();
    }

    fn workflows_looked(&mut self, looked: Looked, cx: &mut Context<Self>) {
        self.workflow_checks.checking = false;
        self.workflow_checks.swept |= looked.swept;
        for error in &looked.errors {
            eprintln!("{error}");
        }
        // The look marked what it claimed as running (so nothing could remove or double-run it
        // while its models were probed); the runs started here keep that mark, the others lose it.
        let claimed: Vec<String> = looked
            .monos
            .iter()
            .flat_map(|(_, found)| &found.starts)
            .map(|start| match start {
                Start::Run { run, .. } | Start::Resume { run } => run.workflow_id.clone(),
            })
            .collect();
        // A look that was going when the app began to quit finds nothing to start.
        if !self.workflows.is_quitting() {
            self.apply_found(looked.monos, cx);
        }
        for workflow_id in claimed {
            if !self.workflows.is_running(&workflow_id) {
                self.workflow_signals.set_running(&workflow_id, false);
            }
        }
        if !self.workflows.is_quitting() && std::mem::take(&mut self.workflow_checks.again) {
            self.check_workflows(cx);
        }
    }

    fn apply_found(&mut self, monos: Vec<(String, Found)>, cx: &mut Context<Self>) {
        for (mono_id, found) in monos {
            // Deleted since the look.
            if self.roster.get(&mono_id).is_none() {
                continue;
            }
            for workflow_id in &found.stops {
                self.stop_workflow_run(workflow_id);
            }
            for start in found.starts {
                match start {
                    Start::Resume { run } => {
                        self.resume_workflow_run(&mono_id, &run, cx);
                    }
                    Start::Run { run, failure } => {
                        // A run that never starts would otherwise only show in the run history.
                        if let Some(failure) = failure {
                            self.workflow_post(
                                Posted {
                                    mono_id: mono_id.clone(),
                                    workflow_id: run.workflow_id.clone(),
                                    workflow_name: run.definition.name.clone(),
                                    run_id: run.id.clone(),
                                    title: failure.title,
                                    text: failure.text,
                                    launches: Vec::new(),
                                },
                                cx,
                            );
                        }
                        self.start_workflow_run(&mono_id, run, cx);
                    }
                }
            }
        }
    }

    /// Ends the sessions of Ask-Mono questions for good. The roster forgets them first and the
    /// queued roster write has to be on disk before any row is deleted: row ids can be reused, so
    /// a quit between the two must never leave an id behind that a later start would delete. The
    /// roster is only written through the manager's ordered writes, so an older snapshot can never
    /// overwrite a newer one. A row that cannot be deleted goes back into the roster, still
    /// hidden, for the next start to try again.
    pub(in crate::monos) fn forget_asks(&mut self, ids: Vec<i64>, cx: &mut Context<Self>) {
        self.roster.hidden_sessions.retain(|id| !ids.contains(id));
        self.roster.ask_sessions.retain(|id| !ids.contains(id));
        let written = self.save_reporting(cx);
        let store = self.store.clone();
        let work = cx.background_executor().spawn(async move {
            let written = written
                .recv()
                .await
                .unwrap_or_else(|_| Err("The roster was not written".into()));
            let failed = match written {
                Ok(()) => delete_asks(&store, &ids),
                Err(error) => Err(error),
            };
            // Whatever could not be deleted (everything, if the roster was not written) is kept.
            failed.unwrap_or_else(|error| {
                eprintln!("Could not remove workflow question sessions: {error}");
                ids
            })
        });
        cx.spawn(async move |this, cx| {
            let keep = work.await;
            if keep.is_empty() {
                return;
            }
            let _ = this.update(cx, |this, cx| {
                for id in keep {
                    if !this.roster.hidden_sessions.contains(&id) {
                        this.roster.hidden_sessions.push(id);
                    }
                    if !this.roster.ask_sessions.contains(&id) {
                        this.roster.ask_sessions.push(id);
                    }
                }
                this.save(cx);
            });
        })
        .detach();
    }

    /// Deletes the Ask-Mono sessions a previous quit left behind (once, at the first look).
    fn sweep_asks(&mut self, cx: &mut Context<Self>) {
        if std::mem::replace(&mut self.workflow_checks.asks_swept, true) {
            return;
        }
        let candidates: Vec<i64> = self
            .roster
            .ask_sessions
            .iter()
            .copied()
            .filter(|id| !self.workflow_live.asks.contains_key(id))
            .collect();
        if candidates.is_empty() {
            return;
        }
        self.forget_asks(candidates, cx);
    }
}

/// What `quit` and `restart` call before they shut the processes down: the workflow runs let go
/// without ending (the next start interrupts them), nothing new starts, and their commands are
/// killed before the app kills every process. It never updates `Monos`, so it is safe to call
/// from inside one of its updates.
pub(crate) fn begin_quit(cx: &mut App) {
    let runner = cx.try_global::<QuitTarget>().map(|t| t.0.clone());
    if let Some(runner) = runner {
        runner.quit();
    }
    monocode_core::mono::workflows::command::cancel_all();
}

/// The runner `begin_quit` reaches (there is one per app).
pub(super) struct QuitTarget(pub(super) WorkflowRunner);
impl gpui::Global for QuitTarget {}

#[cfg(test)]
mod tests;
