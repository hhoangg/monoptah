//! The scheduling cases of `useMonoWorkflows.test.ts`: what one look at a Mono's workflow files
//! finds to start, resume and stop. Real files in temp dirs; the host side (running what was
//! found) is tested in `monocode-gpui`.
use super::engine::{mark_interrupted, start_run};
use super::model::*;
use super::models::Probe;
use super::store::*;
use super::tick::*;
use super::validate::validate_workflow;
use crate::catalog::Model;
use crate::mono::files::AgentFile;
use crate::provider::Kind;
use chrono::{Local, TimeZone};
use serde_json::{json, Value};
use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

const ID: &str = "wf-abcd1234";
const OTHER: &str = "wf-other000";

fn now() -> u64 {
    Local
        .with_ymd_and_hms(2026, 10, 22, 12, 0, 0)
        .single()
        .unwrap()
        .timestamp_millis() as u64
}

fn temp_root() -> PathBuf {
    let root = std::env::temp_dir().join(format!("wf-tick-{}", crate::mono::new_id()));
    std::fs::create_dir_all(&root).unwrap();
    root
}

fn dir(root: &std::path::Path) -> MonoDir {
    MonoDir::new(root, "mono-1")
}

const DAILY: &str = r#"{"id":"trigger","type":"schedule","schedule":{"scheduleKind":"daily","time":"09:00","minute":0,"dayOfWeek":1}}"#;

/// A valid scheduled workflow that is due (its time was a second ago).
fn workflow_with(id: &str, extra: Value) -> Workflow {
    let mut v: Value = serde_json::from_str(&format!(
        r#"{{"id":"{id}","name":"Ship it","inputs":{{}},"inputValues":{{}},
            "nodes":[{DAILY},{{"id":"report","type":"report"}}],
            "edges":[{{"from":"trigger","to":"report"}}],
            "enabled":true,"createdAt":1,"nextRunAt":{}}}"#,
        now() - 1_000
    ))
    .unwrap();
    for (k, x) in extra.as_object().unwrap() {
        if x.is_null() {
            v.as_object_mut().unwrap().remove(k);
        } else {
            v[k] = x.clone();
        }
    }
    super::fixtures::workflow_from(&v.to_string())
}

fn workflow(extra: Value) -> Workflow {
    workflow_with(ID, extra)
}

/// A valid workflow whose `dev` session names `extra` (harness / model fields).
fn with_session(extra: &str) -> Workflow {
    let mut w = workflow(
        json!({"inputs": {"repo": {"type": "project"}}, "inputValues": {"repo": "/code/app"}}),
    );
    w.nodes.insert(
        1,
        super::fixtures::node(&format!(
            r#"{{"id":"dev","type":"session","project":"{{{{inputs.repo}}}}","prompt":"go","checkout":"root",{extra}}}"#
        )),
    );
    w.edges = vec![
        super::fixtures::edge(r#"{"from":"trigger","to":"dev"}"#),
        super::fixtures::edge(r#"{"from":"dev","to":"report"}"#),
    ];
    assert_eq!(validate_workflow(&w), vec![]);
    w
}

fn save(dir: &MonoDir, workflows: &[Workflow]) {
    let list = workflows.to_vec();
    update_workflows(dir, |_| Ok((list.clone(), ()))).unwrap();
}

fn stored(dir: &MonoDir) -> Vec<Workflow> {
    load_workflows(dir).unwrap().workflows
}

fn stored_one(dir: &MonoDir) -> Workflow {
    stored(dir).remove(0)
}

fn probe_nothing(_: Kind) -> Probe {
    Probe {
        installed: true,
        models: None,
    }
}

fn model(id: &str) -> Model {
    Model {
        id: id.into(),
        name: id.into(),
        alias_target: None,
        efforts: vec![],
        default_effort: None,
        fast: false,
        thinking: false,
        default: false,
        tiers: vec![],
        controls: vec![],
    }
}

fn probe_claude(_: Kind) -> Probe {
    Probe {
        installed: true,
        models: Some(vec![model("claude-sonnet-5")]),
    }
}

fn idle(_: &str) -> bool {
    false
}

fn look(files: &impl MonoFiles, has_chat: bool, running: &dyn Fn(&str) -> bool) -> Found {
    look_with(files, has_chat, running, probe_nothing)
}

fn look_with(
    files: &impl MonoFiles,
    has_chat: bool,
    running: &dyn Fn(&str) -> bool,
    probe: fn(Kind) -> Probe,
) -> Found {
    check(
        files,
        &Tick {
            now: now(),
            is_running: running,
            has_chat,
            probe,
            on_claim: &|_| {},
        },
    )
    .unwrap()
}

fn runs_of(found: &Found) -> Vec<(&WorkflowRun, Option<&Failure>)> {
    found
        .starts
        .iter()
        .filter_map(|s| match s {
            Start::Run { run, failure } => Some((run, failure.as_ref())),
            _ => None,
        })
        .collect()
}

fn resumes_of(found: &Found) -> Vec<&WorkflowRun> {
    found
        .starts
        .iter()
        .filter_map(|s| match s {
            Start::Resume { run } => Some(run),
            _ => None,
        })
        .collect()
}

/// Counts what is saved, to see when a look must leave the file alone.
struct Counting {
    disk: MonoDir,
    saves: Cell<usize>,
}

impl Counting {
    fn new(disk: MonoDir) -> Self {
        Self {
            disk,
            saves: Cell::new(0),
        }
    }
}

impl MonoFiles for Counting {
    fn read(&self, relative: &str) -> Result<AgentFile, String> {
        self.disk.read(relative)
    }
    fn save(&self, relative: &str, text: &str, expected: Option<&str>) -> Result<String, String> {
        self.saves.set(self.saves.get() + 1);
        self.disk.save(relative, text, expected)
    }
}

// --- due and requested runs ---------------------------------------------------------------

#[test]
fn a_due_scheduled_workflow_is_claimed_and_started() {
    let root = temp_root();
    let disk = dir(&root);
    save(&disk, &[workflow(json!({}))]);
    let found = look(&disk, true, &idle);
    let runs = runs_of(&found);
    assert_eq!(runs.len(), 1);
    let (run, failure) = runs[0];
    assert!(failure.is_none());
    assert_eq!(run.workflow_id, ID);
    assert_eq!(run.trigger, RunTrigger::Schedule);
    assert_eq!(run.status, RunStatus::Running);
    assert_eq!(run.started_at, now());
    let after = stored_one(&disk);
    assert!(after.next_run_at.unwrap() > now());
    assert!(!after.run_requested);
    assert_eq!(after.last_run_id.as_deref(), Some(run.id.as_str()));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn a_requested_run_starts_even_when_the_schedule_is_not_due() {
    let root = temp_root();
    let disk = dir(&root);
    let later = now() + 3_600_000;
    save(
        &disk,
        &[workflow(json!({"runRequested": true, "nextRunAt": later}))],
    );
    let found = look(&disk, true, &idle);
    let runs = runs_of(&found);
    assert_eq!(runs[0].0.trigger, RunTrigger::Manual);
    let after = stored_one(&disk);
    assert!(!after.run_requested);
    // A requested run leaves the schedule where it was.
    assert_eq!(after.next_run_at, Some(later));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn a_manual_workflow_runs_on_request_and_stays_paused() {
    let root = temp_root();
    let disk = dir(&root);
    let manual = workflow(json!({"runRequested": true, "enabled": false, "nextRunAt": null}));
    let mut manual = manual;
    manual.nodes[0] = super::fixtures::node(r#"{"id":"trigger","type":"manual"}"#);
    save(&disk, &[manual]);
    let found = look(&disk, true, &idle);
    assert_eq!(runs_of(&found)[0].0.trigger, RunTrigger::Manual);
    let after = stored_one(&disk);
    assert!(!after.enabled && !after.run_requested);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn a_workflow_waiting_for_review_does_not_start() {
    let root = temp_root();
    let disk = dir(&root);
    save(
        &disk,
        &[workflow(json!({"runRequested": true, "needsReview": true}))],
    );
    let found = look(&disk, true, &idle);
    assert!(found.starts.is_empty());
    assert!(stored_one(&disk).run_requested);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn a_tick_while_the_previous_run_is_going_moves_the_schedule_and_starts_nothing() {
    let root = temp_root();
    let disk = dir(&root);
    save(&disk, &[workflow(json!({}))]);
    let found = look(&disk, true, &|id| id == ID);
    assert!(found.starts.is_empty());
    let after = stored_one(&disk);
    assert!(after.next_run_at.unwrap() > now());
    assert_eq!(after.last_run_id, None);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn a_requested_run_waits_for_the_current_one_instead_of_being_dropped() {
    let root = temp_root();
    let disk = dir(&root);
    save(&disk, &[workflow(json!({"runRequested": true}))]);
    let found = look(&disk, true, &|id| id == ID);
    assert!(found.starts.is_empty());
    assert!(stored_one(&disk).run_requested);
    // Once it ends the request runs.
    let found = look(&disk, true, &idle);
    assert_eq!(runs_of(&found).len(), 1);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn a_mono_without_a_chat_moves_schedules_on_but_claims_nothing() {
    let root = temp_root();
    let disk = dir(&root);
    save(
        &disk,
        &[
            workflow(json!({"runRequested": true})),
            // Came due so long ago that it is not run, only moved on.
            workflow_with(OTHER, json!({"nextRunAt": now() - 5 * 3_600_000})),
        ],
    );
    let found = look(&disk, false, &idle);
    assert!(found.starts.is_empty());
    let after = stored(&disk);
    assert!(after[0].run_requested);
    assert!(after[1].next_run_at.unwrap() > now());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn only_one_run_is_started_per_workflow_and_other_workflows_go_side_by_side() {
    let root = temp_root();
    let disk = dir(&root);
    save(
        &disk,
        &[workflow(json!({})), workflow_with(OTHER, json!({}))],
    );
    let found = look(&disk, true, &idle);
    let mut ids: Vec<_> = runs_of(&found)
        .iter()
        .map(|(run, _)| run.workflow_id.clone())
        .collect();
    ids.sort();
    assert_eq!(ids, vec![ID.to_string(), OTHER.to_string()]);
    // The next look, with both running, claims neither again.
    let found = look(&disk, true, &|_| true);
    assert!(found.starts.is_empty());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn a_claim_is_checked_again_against_the_latest_file() {
    // The workflow is paused between the look and the claim: nothing is claimed or started.
    struct Pauses {
        disk: MonoDir,
        reads: Cell<usize>,
    }
    impl MonoFiles for Pauses {
        fn read(&self, relative: &str) -> Result<AgentFile, String> {
            if relative == "workflows.json" {
                self.reads.set(self.reads.get() + 1);
                // The second read of the file is the claim's: the user pauses it just before.
                if self.reads.get() == 2 {
                    let mut paused = stored_one(&self.disk);
                    paused.enabled = false;
                    save(&self.disk, &[paused]);
                }
            }
            self.disk.read(relative)
        }
        fn save(&self, r: &str, t: &str, e: Option<&str>) -> Result<String, String> {
            self.disk.save(r, t, e)
        }
    }
    let root = temp_root();
    let disk = dir(&root);
    save(&disk, &[workflow(json!({}))]);
    let pausing = Pauses {
        disk: disk.clone(),
        reads: Cell::new(0),
    };
    let found = look(&pausing, true, &idle);
    assert!(found.starts.is_empty(), "{:?}", runs_of(&found).len());
    assert_eq!(stored_one(&disk).last_run_id, None);
    std::fs::remove_dir_all(root).unwrap();
}

// --- claimed but not started yet -------------------------------------------------------------

#[test]
fn a_workflow_is_reported_claimed_before_its_models_are_probed() {
    static CLAIMED_WHEN_PROBED: AtomicUsize = AtomicUsize::new(0);
    static CLAIMS: AtomicUsize = AtomicUsize::new(0);
    fn probing(kind: Kind) -> Probe {
        CLAIMED_WHEN_PROBED.store(CLAIMS.load(Ordering::SeqCst), Ordering::SeqCst);
        probe_nothing(kind)
    }
    let root = temp_root();
    let disk = dir(&root);
    save(
        &disk,
        &[
            with_session(r#""harness":"claude""#),
            // Not claimed: waits for review.
            workflow_with(OTHER, json!({"runRequested": true, "needsReview": true})),
        ],
    );
    let claimed = RefCell::new(Vec::new());
    let on_claim = |id: &str| {
        CLAIMS.fetch_add(1, Ordering::SeqCst);
        claimed.borrow_mut().push(id.to_string());
    };
    let found = check(
        &disk,
        &Tick {
            now: now(),
            is_running: &idle,
            has_chat: true,
            probe: probing,
            on_claim: &on_claim,
        },
    )
    .unwrap();
    assert_eq!(runs_of(&found).len(), 1);
    assert_eq!(*claimed.borrow(), vec![ID.to_string()]);
    // The probe (slow on a cold cache) ran after the claim was announced.
    assert_eq!(CLAIMED_WHEN_PROBED.load(Ordering::SeqCst), 1);
    std::fs::remove_dir_all(root).unwrap();
}

// --- a workflow that cannot start -----------------------------------------------------------

#[test]
fn an_invalid_workflow_gets_a_failed_run_and_a_message_for_the_chat() {
    let root = temp_root();
    let disk = dir(&root);
    let mut broken = workflow(json!({}));
    broken.nodes.retain(|n| n.id != "report");
    broken.edges.clear();
    assert!(!validate_workflow(&broken).is_empty());
    save(&disk, &[broken]);
    let found = look(&disk, true, &idle);
    let (run, failure) = runs_of(&found)[0];
    assert_eq!(run.status, RunStatus::Failed);
    assert_eq!(run.ended_at, Some(now()));
    assert!(
        run.error
            .as_deref()
            .unwrap()
            .starts_with("The workflow is not valid: "),
        "{:?}",
        run.error
    );
    assert!(run.error.as_deref().unwrap().contains("Report"));
    let failure = failure.unwrap();
    assert_eq!(failure.title, "Ship it · failed");
    assert!(failure
        .text
        .starts_with("## Ship it · failed\n\nThe workflow could not start. "));
    assert!(failure.text.contains(run.error.as_deref().unwrap()));
    // It was claimed all the same: the schedule moved on and does not fail again every look.
    assert!(stored_one(&disk).next_run_at.unwrap() > now());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn a_session_naming_a_model_the_catalog_lacks_fails_the_run_naming_it() {
    let root = temp_root();
    let disk = dir(&root);
    let w = with_session(r#""model":"claude:sonnet-6""#);
    save(&disk, &[w]);
    let found = look_with(&disk, true, &idle, probe_claude);
    let (run, failure) = runs_of(&found)[0];
    assert_eq!(run.status, RunStatus::Failed);
    assert!(
        run.error.as_deref().unwrap().contains(
            "Model \"claude:sonnet-6\" is not available. Pick one in the step's model list."
        ),
        "{:?}",
        run.error
    );
    assert!(failure.unwrap().text.contains("Model \"claude:sonnet-6\""));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn the_catalog_is_only_probed_for_the_harnesses_the_workflow_uses() {
    static ASKED: AtomicUsize = AtomicUsize::new(0);
    fn counting(kind: Kind) -> Probe {
        ASKED.fetch_add(1, Ordering::SeqCst);
        assert_eq!(kind, Kind::Claude);
        probe_nothing(kind)
    }
    let root = temp_root();
    let disk = dir(&root);
    // No session node: nothing to probe.
    save(&disk, &[workflow(json!({}))]);
    look_with(&disk, true, &idle, counting);
    assert_eq!(ASKED.load(Ordering::SeqCst), 0);
    let w = with_session(r#""harness":"claude""#);
    save(&disk, &[w]);
    look_with(&disk, true, &idle, counting);
    assert_eq!(ASKED.load(Ordering::SeqCst), 1);
    std::fs::remove_dir_all(root).unwrap();
}

// --- requests in the file -------------------------------------------------------------------

fn interrupted(id: &str) -> WorkflowRun {
    mark_interrupted(&start_run(
        &workflow(json!({})),
        RunTrigger::Manual,
        1_000,
        id,
    ))
}

fn put_runs(dir: &MonoDir, workflow_id: &str, runs: Vec<WorkflowRun>) {
    update_runs(dir, workflow_id, |_| Ok((runs.clone(), ()))).unwrap();
}

#[test]
fn a_resume_asked_for_in_the_file_resumes_an_interrupted_run() {
    let root = temp_root();
    let disk = dir(&root);
    save(
        &disk,
        &[workflow(
            json!({"enabled": false, "resumeRunId": "run-old"}),
        )],
    );
    put_runs(&disk, ID, vec![interrupted("run-old")]);
    let found = look(&disk, true, &idle);
    let resumes = resumes_of(&found);
    assert_eq!(resumes.len(), 1);
    assert_eq!(resumes[0].id, "run-old");
    assert_eq!(resumes[0].status, RunStatus::Interrupted);
    // The request is answered, so it is not repeated.
    assert_eq!(stored_one(&disk).resume_run_id, None);
    assert!(look(&disk, true, &idle).starts.is_empty());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn a_resume_for_a_run_that_is_not_interrupted_is_answered_and_ignored() {
    let root = temp_root();
    let disk = dir(&root);
    save(
        &disk,
        &[workflow(
            json!({"enabled": false, "resumeRunId": "run-old"}),
        )],
    );
    let mut done = interrupted("run-old");
    done.status = RunStatus::Done;
    put_runs(&disk, ID, vec![done]);
    assert!(look(&disk, true, &idle).starts.is_empty());
    assert_eq!(stored_one(&disk).resume_run_id, None);
    // And one for a run that is not there.
    save(
        &disk,
        &[workflow(
            json!({"enabled": false, "resumeRunId": "run-none"}),
        )],
    );
    assert!(look(&disk, true, &idle).starts.is_empty());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn a_resume_is_refused_while_the_workflow_is_running_or_the_mono_has_no_chat() {
    let root = temp_root();
    let disk = dir(&root);
    put_runs(&disk, ID, vec![interrupted("run-old")]);
    save(
        &disk,
        &[workflow(
            json!({"enabled": false, "resumeRunId": "run-old"}),
        )],
    );
    assert!(look(&disk, true, &|id| id == ID).starts.is_empty());
    save(
        &disk,
        &[workflow(
            json!({"enabled": false, "resumeRunId": "run-old"}),
        )],
    );
    assert!(look(&disk, false, &idle).starts.is_empty());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn a_resume_keeps_the_workflow_from_also_starting_a_scheduled_run() {
    let root = temp_root();
    let disk = dir(&root);
    put_runs(&disk, ID, vec![interrupted("run-old")]);
    // Due, and a resume is asked for: only the resume goes.
    save(&disk, &[workflow(json!({"resumeRunId": "run-old"}))]);
    let found = look(&disk, true, &idle);
    assert_eq!(resumes_of(&found).len(), 1);
    assert!(runs_of(&found).is_empty());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn a_stop_nobody_runs_is_cleared_and_one_for_a_run_that_is_going_is_handed_over() {
    let root = temp_root();
    let disk = dir(&root);
    save(
        &disk,
        &[
            workflow(json!({"enabled": false, "stopRequested": true})),
            workflow_with(OTHER, json!({"enabled": false, "stopRequested": true})),
        ],
    );
    let found = look(&disk, true, &|id| id == OTHER);
    assert_eq!(found.stops, vec![OTHER.to_string()]);
    assert!(stored(&disk).iter().all(|w| !w.stop_requested));
    // Asked for again, nothing is going: nothing to stop.
    save(
        &disk,
        &[workflow(json!({"enabled": false, "stopRequested": true}))],
    );
    assert!(look(&disk, true, &idle).stops.is_empty());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn a_stop_asked_for_before_a_run_starts_is_dropped_at_the_claim() {
    let root = temp_root();
    let disk = dir(&root);
    save(
        &disk,
        &[workflow(
            json!({"runRequested": true, "stopRequested": true}),
        )],
    );
    let found = look(&disk, true, &idle);
    assert_eq!(runs_of(&found).len(), 1);
    assert!(found.stops.is_empty());
    assert!(!stored_one(&disk).stop_requested);
    std::fs::remove_dir_all(root).unwrap();
}

/// Fails every save of workflows.json after the first `ok` ones.
struct Failing {
    disk: MonoDir,
    ok: Cell<usize>,
}

impl MonoFiles for Failing {
    fn read(&self, relative: &str) -> Result<AgentFile, String> {
        self.disk.read(relative)
    }
    fn save(&self, relative: &str, text: &str, expected: Option<&str>) -> Result<String, String> {
        if relative == "workflows.json" {
            if self.ok.get() == 0 {
                return Err("disk full".into());
            }
            self.ok.set(self.ok.get() - 1);
        }
        self.disk.save(relative, text, expected)
    }
}

#[test]
fn a_resume_already_taken_from_the_file_is_still_acted_on_when_a_later_save_fails() {
    let root = temp_root();
    let disk = dir(&root);
    put_runs(&disk, ID, vec![interrupted("run-old")]);
    save(
        &disk,
        &[
            workflow(json!({"enabled": false, "resumeRunId": "run-old"})),
            // Came due long ago: only moved on, which needs a save that fails.
            workflow_with(OTHER, json!({"nextRunAt": now() - 5 * 3_600_000})),
        ],
    );
    // The take is saved; the advance and the claims are not.
    let failing = Failing {
        disk: disk.clone(),
        ok: Cell::new(1),
    };
    let found = look(&failing, true, &idle);
    assert_eq!(resumes_of(&found).len(), 1);
    assert_eq!(stored(&disk)[0].resume_run_id, None);
    std::fs::remove_dir_all(root).unwrap();
}

// --- not touching the file for nothing --------------------------------------------------------

#[test]
fn a_look_with_nothing_to_do_does_not_write() {
    let root = temp_root();
    let counting = Counting::new(dir(&root));
    save(
        &counting.disk,
        &[workflow(json!({"nextRunAt": now() + 3_600_000}))],
    );
    assert!(look(&counting, true, &idle).starts.is_empty());
    assert_eq!(counting.saves.get(), 0);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn a_schedule_that_cannot_move_on_is_not_rewritten_every_look() {
    let root = temp_root();
    let counting = Counting::new(dir(&root));
    // An enabled schedule with a time that is no time and no next run: "advanced" on every look,
    // but advancing it changes nothing.
    let mut broken = workflow(json!({"nextRunAt": null}));
    broken.nodes[0] = super::fixtures::node(
        r#"{"id":"trigger","type":"schedule","schedule":{"scheduleKind":"daily","time":"99:99","minute":0,"dayOfWeek":1}}"#,
    );
    save(&counting.disk, &[broken]);
    for _ in 0..3 {
        assert!(look(&counting, true, &idle).starts.is_empty());
    }
    assert_eq!(counting.saves.get(), 0);
    std::fs::remove_dir_all(root).unwrap();
}

// --- the sweep ------------------------------------------------------------------------------

fn running(id: &str) -> WorkflowRun {
    start_run(&workflow(json!({})), RunTrigger::Manual, 1_000, id)
}

fn statuses(dir: &MonoDir, workflow_id: &str) -> Vec<(String, RunStatus)> {
    load_runs(dir, workflow_id)
        .unwrap()
        .into_iter()
        .map(|r| (r.id, r.status))
        .collect()
}

#[test]
fn runs_left_running_are_marked_interrupted() {
    let root = temp_root();
    let disk = dir(&root);
    save(&disk, &[workflow(json!({"enabled": false}))]);
    let mut done = running("run-done");
    done.status = RunStatus::Done;
    put_runs(&disk, ID, vec![running("run-old"), done]);
    sweep_interrupted(&disk, &[]).unwrap();
    assert_eq!(
        statuses(&disk, ID),
        vec![
            ("run-old".to_string(), RunStatus::Interrupted),
            ("run-done".to_string(), RunStatus::Done)
        ]
    );
    // Nothing else of the run is invented: running nodes went back to pending.
    let run = load_runs(&disk, ID).unwrap().remove(0);
    assert_eq!(run, mark_interrupted(&running("run-old")));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn a_run_the_runner_is_driving_is_left_alone() {
    let root = temp_root();
    let counting = Counting::new(dir(&root));
    save(&counting.disk, &[workflow(json!({"enabled": false}))]);
    put_runs(&counting.disk, ID, vec![running("run-live")]);
    let before = counting.saves.get();
    sweep_interrupted(&counting, &[(ID.to_string(), "run-live".to_string())]).unwrap();
    assert_eq!(counting.saves.get(), before);
    assert_eq!(statuses(&counting.disk, ID)[0].1, RunStatus::Running);
    // Another run of the same workflow that nobody drives is still marked.
    put_runs(
        &counting.disk,
        ID,
        vec![running("run-live"), running("run-gone")],
    );
    sweep_interrupted(&counting, &[(ID.to_string(), "run-live".to_string())]).unwrap();
    let after = statuses(&counting.disk, ID);
    assert_eq!(after[0], ("run-live".to_string(), RunStatus::Running));
    assert_eq!(after[1], ("run-gone".to_string(), RunStatus::Interrupted));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn a_sweep_of_runs_that_were_not_running_writes_nothing() {
    let root = temp_root();
    let counting = Counting::new(dir(&root));
    save(&counting.disk, &[workflow(json!({"enabled": false}))]);
    put_runs(&counting.disk, ID, vec![interrupted("run-1")]);
    let before = counting.saves.get();
    sweep_interrupted(&counting, &[]).unwrap();
    assert_eq!(counting.saves.get(), before);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn a_run_history_that_cannot_be_read_does_not_stop_the_sweep_of_the_others() {
    struct Broken {
        disk: MonoDir,
    }
    impl MonoFiles for Broken {
        fn read(&self, relative: &str) -> Result<AgentFile, String> {
            if relative.contains(ID) {
                return Err("disk error".into());
            }
            self.disk.read(relative)
        }
        fn save(&self, r: &str, t: &str, e: Option<&str>) -> Result<String, String> {
            self.disk.save(r, t, e)
        }
    }
    let root = temp_root();
    let disk = dir(&root);
    save(
        &disk,
        &[
            workflow(json!({"enabled": false})),
            workflow_with(OTHER, json!({"enabled": false})),
        ],
    );
    put_runs(&disk, OTHER, vec![running("run-x")]);
    let broken = Broken { disk: disk.clone() };
    // The workflow file itself reads through `workflows.json`; only the first history fails.
    assert_eq!(sweep_interrupted(&broken, &[]).unwrap_err(), "disk error");
    assert_eq!(statuses(&disk, OTHER)[0].1, RunStatus::Interrupted);
    std::fs::remove_dir_all(root).unwrap();
}
