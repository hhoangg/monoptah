//! One look at a Mono's workflow files (the tick of `useMonoWorkflows.ts`): which runs to start,
//! which interrupted runs to resume, which runs to stop. It does the file work and the model
//! catalog probing, so it blocks and belongs off the UI thread; starting what it found is the
//! runner's job (`monocode-gpui`).
//!
//! Order within a look, as in TS: requests left in the file (`resumeRunId`, `stopRequested`), then
//! the workflows whose schedule only has to move on, then each due workflow is claimed (re-checked
//! against the newest file inside the save) and turned into a run. A workflow whose definition or
//! models are unusable gets a run that is already failed, with the message for the Mono's chat.
use std::collections::HashSet;

use super::engine::{mark_interrupted, start_run};
use super::model::{with_run_values, RunStatus, RunTrigger, Workflow, WorkflowRun};
use super::models::{harnesses_used, workflow_issues, ModelCatalog, Probe};
use super::preflight::{union_requires, Preflight};
use super::schedule::{after_claim, due_workflows};
use super::store::{
    load_runs, load_workflows, take_requests, update_runs, update_workflows, MonoFiles,
};
use super::yaml::input_value_problems;
use crate::provider::Kind;

/// What a look needs to know.
pub struct Tick<'a> {
    pub now: u64,
    /// Whether a run of the workflow is going (the runner's answer).
    pub is_running: &'a dyn Fn(&str) -> bool,
    /// Workflows are reported in the Mono's chat, so a Mono that has none starts nothing.
    pub has_chat: bool,
    /// How a harness is probed for the model catalog (tests must not reach real CLIs).
    pub probe: fn(Kind) -> Probe,
    /// Called as soon as a workflow is claimed (or a resume of it is taken) from the file, before its catalog is probed (which
    /// can take a while) and before anyone starts the run: the caller marks the workflow as
    /// running, so it cannot be removed, run twice or reported "not running" in between.
    pub on_claim: &'a dyn Fn(&str),
    /// Whether the machine has what a workflow declares in `requires`. Called only for a workflow
    /// that declares something, after its definition and models passed (tests must not reach real
    /// CLIs or shells).
    pub preflight: &'a dyn Fn(&Workflow) -> Preflight,
}

/// The message for the Mono's chat when a run could not even start.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Failure {
    pub title: String,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Start {
    /// A new run, claimed from the file. A run that is not `Running` (failed validation) only has
    /// to be written, with `failure` posted.
    Run {
        run: WorkflowRun,
        failure: Option<Failure>,
    },
    /// An interrupted run asked to continue, as stored.
    Resume { run: WorkflowRun },
    /// An interrupted run asked to continue that may not resume yet (a requirement is missing).
    /// The run stays interrupted; `failure` is for the Mono's chat.
    Refused {
        workflow_id: String,
        workflow_name: String,
        run_id: String,
        failure: Failure,
    },
}

#[derive(Debug, Default, PartialEq)]
pub struct Found {
    pub starts: Vec<Start>,
    /// Workflows with a run going whose stop was asked for.
    pub stops: Vec<String>,
}

fn warn(what: &str, error: &str) {
    eprintln!("{what}: {error}");
}

/// Claims `workflow_id` from the newest file if it is still due. Returns the workflow as it was
/// before the claim (its `run_requested` says why it ran).
fn claim(
    files: &impl MonoFiles,
    now: u64,
    workflow_id: &str,
    run_id: &str,
    busy: &dyn Fn(&str) -> bool,
) -> Result<Option<Workflow>, String> {
    update_workflows(files, |list| {
        let only: Vec<Workflow> = list
            .iter()
            .filter(|w| w.id == workflow_id)
            .cloned()
            .collect();
        let due = due_workflows(&only, now, busy)
            .run
            .first()
            .map(|w| (*w).clone());
        let list = if due.is_some() {
            list.into_iter()
                .map(|w| {
                    if w.id == workflow_id {
                        after_claim(&w, now, Some(run_id))
                    } else {
                        w
                    }
                })
                .collect()
        } else {
            list
        };
        Ok((list, due))
    })
}

/// The run for a claimed workflow: running, or already failed with the message to post.
fn begin(claimed: &Workflow, tick: &Tick, run_id: &str) -> Start {
    let trigger = if claimed.run_requested {
        RunTrigger::Manual
    } else {
        RunTrigger::Schedule
    };
    // The run sees the values asked for this run laid over the stored ones, and keeps only that.
    let effective = with_run_values(claimed);
    let mut run = start_run(&effective, trigger, tick.now, run_id);
    let catalog = ModelCatalog::for_harnesses(&harnesses_used(&effective), tick.probe);
    let mut messages: Vec<String> = workflow_issues(&effective, &catalog)
        .into_iter()
        .map(|issue| issue.message)
        .collect();
    // The definition may have changed since the values were asked for.
    if claimed.run_values.is_some() {
        messages.extend(input_value_problems(
            &effective.inputs,
            &effective.input_values,
        ));
    }
    let error = if !messages.is_empty() {
        format!("The workflow is not valid: {}", messages.join("; "))
    } else if effective.requires.is_empty() {
        return Start::Run { run, failure: None };
    } else {
        let check = (tick.preflight)(&effective);
        if check.all_met() {
            return Start::Run { run, failure: None };
        }
        format!("Missing requirements: {}", check.summary())
    };
    run.status = RunStatus::Failed;
    run.ended_at = Some(tick.now);
    run.error = Some(error.clone());
    // A run that never starts would otherwise only show in the run history.
    let failure = Failure {
        title: format!("{} \u{b7} failed", claimed.name),
        text: format!(
            "## {} \u{b7} failed\n\nThe workflow could not start. {error}",
            claimed.name
        ),
    };
    Start::Run {
        run,
        failure: Some(failure),
    }
}

/// An interrupted run asked to continue: resumed as stored when what it declared and what the
/// workflow declares now are present, otherwise refused (the run stays interrupted). The stored
/// definition is checked rather than the live one so the run keeps its own steps; the live
/// `requires` are added because they may have grown since.
fn resume_or_refuse(run: WorkflowRun, live: Option<&Workflow>, tick: &Tick) -> Start {
    let requires = union_requires(
        &run.definition.requires,
        live.map_or(&[][..], |w| &w.requires),
    );
    if requires.is_empty() {
        return Start::Resume { run };
    }
    let mut checked = run.definition.clone();
    checked.requires = requires;
    let check = (tick.preflight)(&checked);
    if check.all_met() {
        return Start::Resume { run };
    }
    let name = run.definition.name.clone();
    let failure = Failure {
        title: format!("{name} \u{b7} not resumed"),
        text: format!(
            "## {name} \u{b7} not resumed\n\nThe run could not continue yet and stays interrupted. Missing requirements: {}",
            check.summary()
        ),
    };
    Start::Refused {
        workflow_id: run.workflow_id,
        workflow_name: name,
        run_id: run.id,
        failure,
    }
}

/// Looks at the Mono's workflows once. A request or workflow that cannot be handled is reported
/// on stderr and skipped, so one bad entry does not hold up the others; an error is returned
/// only when the workflow file itself cannot be read.
pub fn check(files: &impl MonoFiles, tick: &Tick) -> Result<Found, String> {
    let mut found = Found::default();
    let mut workflows = load_workflows(files)?.workflows;
    if workflows
        .iter()
        .any(|w| w.resume_run_id.is_some() || w.stop_requested)
    {
        match take_requests(files, tick.is_running) {
            Ok(taken) => {
                found.stops = taken.stops;
                for request in taken.resumes {
                    // A Mono without a chat has nowhere to report, and a workflow that is going
                    // is not resumed over itself.
                    if !tick.has_chat || (tick.is_running)(&request.workflow_id) {
                        continue;
                    }
                    match load_runs(files, &request.workflow_id) {
                        Ok(runs) => {
                            if let Some(run) = runs.into_iter().find(|r| r.id == request.run_id) {
                                if run.status == RunStatus::Interrupted {
                                    (tick.on_claim)(&request.workflow_id);
                                    let live =
                                        workflows.iter().find(|w| w.id == request.workflow_id);
                                    found.starts.push(resume_or_refuse(run, live, tick));
                                }
                            }
                        }
                        Err(error) => warn("Could not resume the workflow run", &error),
                    }
                }
            }
            Err(error) => warn("Could not read the workflow requests", &error),
        }
        // What was taken is acted on even if the file cannot be read again.
        match load_workflows(files) {
            Ok(loaded) => workflows = loaded.workflows,
            Err(error) => {
                warn("Could not read the workflows again", &error);
                return Ok(found);
            }
        }
    }
    // A workflow about to resume is busy as far as a new run is concerned.
    let resuming: HashSet<String> = found
        .starts
        .iter()
        .filter_map(|s| match s {
            Start::Resume { run } => Some(run.workflow_id.clone()),
            _ => None,
        })
        .collect();
    let busy = |id: &str| (tick.is_running)(id) || resuming.contains(id);
    let due = due_workflows(&workflows, tick.now, busy);
    // An invalid schedule has no next time to move to, so it is "advanced" on every look: only
    // write when something would change.
    if due
        .advance
        .iter()
        .any(|w| after_claim(w, tick.now, None) != **w)
    {
        if let Err(error) = update_workflows(files, |list| {
            let stale: HashSet<String> = due_workflows(&list, tick.now, busy)
                .advance
                .iter()
                .map(|w| w.id.clone())
                .collect();
            let list = list
                .into_iter()
                .map(|w| {
                    if stale.contains(&w.id) {
                        after_claim(&w, tick.now, None)
                    } else {
                        w
                    }
                })
                .collect();
            Ok((list, ()))
        }) {
            warn("Could not move workflow schedules on", &error);
        }
    }
    if !tick.has_chat {
        return Ok(found);
    }
    let to_run: Vec<String> = due.run.iter().map(|w| w.id.clone()).collect();
    for id in to_run {
        let run_id = uuid::Uuid::new_v4().to_string();
        match claim(files, tick.now, &id, &run_id, &busy) {
            Ok(Some(claimed)) => {
                (tick.on_claim)(&id);
                found.starts.push(begin(&claimed, tick, &run_id));
            }
            Ok(None) => {}
            Err(error) => warn(&format!("Workflow {id} could not start"), &error),
        }
    }
    Ok(found)
}

/// Marks the runs a previous session left `running` as interrupted, so they can be resumed.
/// `driven` lists the `(workflow id, run id)` the runner is driving right now, which are left
/// alone. Tries every workflow; the first error is returned after the others were done.
pub fn sweep_interrupted(
    files: &impl MonoFiles,
    driven: &[(String, String)],
) -> Result<(), String> {
    let mut first_error = None;
    let mut note = |error: String| {
        first_error.get_or_insert(error);
    };
    for workflow in load_workflows(files)?.workflows {
        let mine = |run_id: &str| {
            driven
                .iter()
                .any(|(wf, run)| *wf == workflow.id && run == run_id)
        };
        let leftover = match load_runs(files, &workflow.id) {
            Ok(runs) => runs
                .iter()
                .any(|r| r.status == RunStatus::Running && !mine(&r.id)),
            Err(error) => {
                note(error);
                continue;
            }
        };
        if !leftover {
            continue;
        }
        let swept = update_runs(files, &workflow.id, |runs| {
            let runs = runs
                .into_iter()
                .map(|r| {
                    if r.status == RunStatus::Running && !mine(&r.id) {
                        mark_interrupted(&r)
                    } else {
                        r
                    }
                })
                .collect();
            Ok((runs, ()))
        });
        if let Err(error) = swept {
            note(error);
        }
    }
    first_error.map_or(Ok(()), Err)
}
