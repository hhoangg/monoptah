//! Port of the store logic in `workflowStore.test.ts` (the in-memory run registry, stop
//! requests and check-now events are not ported: the native app is one process), on temp dirs
//! instead of mocked Tauri commands, plus checks that both apps read each other's files.
use super::engine::start_run;
use super::fixtures::workflow_from;
use super::model::*;
use super::ordered::{Json, OrderedMap};
use super::store::*;
use crate::mono::files::{self, CONFLICT};
use serde_json::json;
use std::cell::{Cell, RefCell};
use std::path::PathBuf;

const ID: &str = "wf-abcd1234";
const RUNS: &str = "workflow-runs/wf-abcd1234.json";

fn temp_root() -> PathBuf {
    let root = std::env::temp_dir().join(format!("wf-store-{}", crate::mono::new_id()));
    std::fs::create_dir_all(&root).unwrap();
    root
}

fn dir(root: &std::path::Path) -> MonoDir {
    MonoDir::new(root, "mono-1")
}

fn workflow_with(id: &str, extra: serde_json::Value) -> Workflow {
    let mut v = json!({
        "id": id, "name": "Daily", "inputs": {}, "inputValues": {},
        "nodes": [{"id": "trigger", "type": "manual"}, {"id": "report", "type": "report"}],
        "edges": [{"from": "trigger", "to": "report"}],
        "enabled": true, "createdAt": 1
    });
    for (k, x) in extra.as_object().unwrap() {
        v[k] = x.clone();
    }
    workflow_from(&v.to_string())
}

fn workflow() -> Workflow {
    workflow_with(ID, json!({}))
}

fn run(id: &str) -> WorkflowRun {
    let mut run = start_run(&workflow(), RunTrigger::Manual, 1, id);
    run.status = RunStatus::Done;
    run.nodes.clear();
    run
}

/// A finished run with one node whose output is `bytes` long.
fn fat(id: &str, bytes: usize) -> WorkflowRun {
    let mut run = run(id);
    let mut node = NodeRun::new("fetch", "");
    node.status = NodeStatus::Done;
    let mut output = OrderedMap::new();
    output.insert("stdout", Json::String("x".repeat(bytes)));
    node.output = Some(output);
    run.nodes = vec![node];
    run
}

fn put(dir: &MonoDir, path: &str, text: &str) -> String {
    dir.save(path, text, None).unwrap()
}

fn text(dir: &MonoDir, path: &str) -> String {
    dir.read(path).unwrap().text.unwrap_or_default()
}

/// The ids of the entries of a JSON list file.
fn ids(text: &str) -> Vec<String> {
    let v: serde_json::Value = serde_json::from_str(text).unwrap();
    v.as_array()
        .unwrap()
        .iter()
        .map(|e| e["id"].as_str().unwrap_or("").to_string())
        .collect()
}

fn list(workflows: &[Workflow]) -> String {
    let entries: Vec<Json> = workflows
        .iter()
        .map(|w| Json::parse(&serde_json::to_string(w).unwrap()).unwrap())
        .collect();
    format!("{}\n", Json::Array(entries).stringify(true))
}

/// A files API that lets a competing writer in before saves, to make the hash check fail.
struct Racy {
    disk: MonoDir,
    /// Texts a rival writes to workflows.json just before the next saves, in order.
    rivals: RefCell<Vec<String>>,
    /// Every save fails with a conflict.
    always: bool,
    reads: Cell<usize>,
    saves: RefCell<Vec<(String, Option<String>)>>,
}

impl Racy {
    fn new(disk: MonoDir) -> Self {
        Racy {
            disk,
            rivals: RefCell::new(Vec::new()),
            always: false,
            reads: Cell::new(0),
            saves: RefCell::new(Vec::new()),
        }
    }
}

impl MonoFiles for Racy {
    fn read(&self, relative: &str) -> Result<files::AgentFile, String> {
        self.reads.set(self.reads.get() + 1);
        self.disk.read(relative)
    }
    fn save(&self, relative: &str, text: &str, expected: Option<&str>) -> Result<String, String> {
        self.saves
            .borrow_mut()
            .push((relative.to_string(), expected.map(String::from)));
        if self.always {
            return Err(CONFLICT.into());
        }
        let rival = {
            let mut rivals = self.rivals.borrow_mut();
            (!rivals.is_empty()).then(|| rivals.remove(0))
        };
        if let Some(rival) = rival {
            self.disk.save(relative, &rival, None)?;
        }
        self.disk.save(relative, text, expected)
    }
}

#[test]
fn update_workflows_retries_once_on_conflict() {
    let root = temp_root();
    let disk = dir(&root);
    let first = put(&disk, "workflows.json", &list(&[workflow()]));
    let rival_text = list(&[workflow(), workflow_with("wf-rival000", json!({}))]);
    let racy = Racy::new(disk);
    racy.rivals.borrow_mut().push(rival_text.clone());
    let mut edits = 0;
    let result = update_workflows(&racy, |current| {
        edits += 1;
        let count = current.len();
        let mut next = current;
        next.push(workflow_with("wf-zzzz9999", json!({"name": "Second"})));
        Ok((next, count))
    })
    .unwrap();
    // The edit ran again over the rival's version.
    assert_eq!(result, 2);
    assert_eq!(edits, 2);
    assert_eq!(racy.reads.get(), 2);
    let saves = racy.saves.borrow();
    assert_eq!(saves.len(), 2);
    assert_eq!(saves[0].1.as_deref(), Some(first.as_str()));
    assert_eq!(
        saves[1].1.as_deref(),
        Some(files::content_hash(&rival_text).as_str())
    );
    assert_eq!(saves[1].0, "workflows.json");
    assert_eq!(
        ids(&text(&racy.disk, "workflows.json")),
        vec![ID, "wf-rival000", "wf-zzzz9999"]
    );
    assert_eq!(load_workflows(&racy.disk).unwrap().workflows.len(), 3);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn update_workflows_gives_up_after_repeated_conflicts() {
    let root = temp_root();
    let mut racy = Racy::new(dir(&root));
    racy.always = true;
    let error = update_workflows(&racy, |list| Ok((list, 0))).unwrap_err();
    assert!(error.contains("changed"), "{error}");
    assert_eq!(racy.saves.borrow().len(), 3);
    assert_eq!(racy.reads.get(), 3);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn update_workflows_does_not_retry_other_errors() {
    let root = temp_root();
    let disk = dir(&root);
    let racy = Racy::new(disk);
    let error =
        update_workflows(&racy, |_| Err::<(Vec<Workflow>, ()), _>("nope".into())).unwrap_err();
    assert_eq!(error, "nope");
    assert!(racy.saves.borrow().is_empty());
    assert_eq!(racy.reads.get(), 1);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn update_runs_keeps_newest_20() {
    let root = temp_root();
    let disk = dir(&root);
    let existing: Vec<WorkflowRun> = (0..19).map(|i| run(&format!("old{i}"))).collect();
    update_runs(&disk, ID, |_| Ok((existing.clone(), ()))).unwrap();
    update_runs(&disk, ID, |runs| {
        let mut all = vec![run("new1"), run("new2"), run("new3")];
        all.extend(runs);
        Ok((all, ()))
    })
    .unwrap();
    let kept = ids(&text(&disk, RUNS));
    assert_eq!(kept.len(), 20);
    assert_eq!(&kept[..3], ["new1", "new2", "new3"]);
    assert_eq!(kept[19], "old16");
    assert_eq!(
        load_runs(&disk, ID)
            .unwrap()
            .iter()
            .map(|r| r.id.clone())
            .collect::<Vec<_>>(),
        kept
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn update_runs_drops_the_oldest_runs_when_the_file_grows_too_large() {
    let root = temp_root();
    let disk = dir(&root);
    let mib = 1024 * 1024;
    update_runs(&disk, ID, |_| {
        let runs = ["r1", "r2", "r3", "r4", "r5"]
            .map(|id| fat(id, mib))
            .to_vec();
        Ok((runs, ()))
    })
    .unwrap();
    let written = text(&disk, RUNS);
    assert_eq!(ids(&written), vec!["r1", "r2", "r3"]);
    assert!(written.len() <= RUNS_FILE_BUDGET);
    assert_eq!(RUNS_FILE_BUDGET, 3_670_016);

    // The newest run is kept even when it alone is over the budget (but under the host cap).
    update_runs(&disk, ID, |_| {
        Ok((vec![fat("only", 3_700_000), fat("older", mib)], ()))
    })
    .unwrap();
    let written = text(&disk, RUNS);
    assert_eq!(ids(&written), vec!["only"]);
    assert!(written.len() > RUNS_FILE_BUDGET);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn update_runs_fails_loudly_when_one_run_passes_the_host_cap() {
    let root = temp_root();
    let disk = dir(&root);
    let error =
        update_runs(&disk, ID, |_| Ok((vec![fat("huge", 4 * 1024 * 1024)], ()))).unwrap_err();
    assert!(error.contains("too large"), "{error}");
    assert_eq!(disk.read(RUNS).unwrap().text, None);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn load_runs_returns_nothing_for_a_missing_file() {
    let root = temp_root();
    let disk = dir(&root);
    assert!(load_runs(&disk, ID).unwrap().is_empty());
    // Reading does not create the file.
    assert_eq!(disk.read(RUNS).unwrap().text, None);
    assert!(load_runs(&disk, "wf-bad").is_err());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn load_runs_drops_malformed_entries_and_unreadable_files() {
    let root = temp_root();
    let disk = dir(&root);
    let ok = serde_json::to_value(run("ok")).unwrap();
    put(&disk, RUNS, &json!([ok, {"id": "x"}, 3, null]).to_string());
    let runs = load_runs(&disk, ID).unwrap();
    assert_eq!(
        runs.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(),
        ["ok"]
    );
    put(&disk, RUNS, "not json");
    assert!(load_runs(&disk, ID).unwrap().is_empty());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn load_runs_keeps_per_round_history_drops_malformed_records_and_the_output_of_old_ones() {
    let root = temp_root();
    let disk = dir(&root);
    let record = json!({
        "round": 1, "status": "done", "sessionId": "s1",
        "decision": {"value": "loop", "reason": "needs tests"}, "startedAt": 1, "endedAt": 2
    });
    // Runs saved before a round stopped keeping its output still carry it.
    let mut old = record.clone();
    old["output"] = json!({"pr_url": "u1"});
    let mut with = serde_json::to_value(run("with")).unwrap();
    with["nodes"] = json!([
        {"nodeId": "dev", "branch": "", "status": "pending", "round": 2, "history": [old]},
        {"nodeId": "review", "branch": "", "status": "pending", "round": 3, "history": [
            old, {"round": "x", "status": "done"}, {"round": 2, "status": "nope"}, 7, null
        ]},
        {"nodeId": "gone", "branch": "", "status": "pending", "round": 3, "history": [7]},
    ]);
    let mut without = serde_json::to_value(run("without")).unwrap();
    without["nodes"] = json!([{"nodeId": "dev", "branch": "", "status": "done", "round": 1}]);
    put(&disk, RUNS, &json!([with, without]).to_string());
    let runs = load_runs(&disk, ID).unwrap();
    let expected: Vec<RoundRecord> = serde_json::from_value(json!([record])).unwrap();
    assert_eq!(runs[0].nodes[0].history.as_ref(), Some(&expected));
    assert_eq!(runs[0].nodes[1].history.as_ref(), Some(&expected));
    assert_eq!(runs[0].nodes[2].history, None);
    assert_eq!(runs[1].nodes[0].history, None);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn update_runs_writes_the_cleaned_history_back() {
    let root = temp_root();
    let disk = dir(&root);
    let mut with = serde_json::to_value(run("with")).unwrap();
    with["nodes"] = json!([{"nodeId": "dev", "branch": "", "status": "pending", "round": 2,
        "history": [{"round": 1, "status": "done", "output": {"a": 1}}, 7]}]);
    put(&disk, RUNS, &json!([with]).to_string());
    update_runs(&disk, ID, |runs| Ok((runs, ()))).unwrap();
    let value: serde_json::Value = serde_json::from_str(&text(&disk, RUNS)).unwrap();
    assert_eq!(
        value[0]["nodes"][0]["history"],
        json!([{"round": 1, "status": "done"}])
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn update_workflows_refuses_a_list_that_would_pass_the_file_cap_with_a_clear_message() {
    let root = temp_root();
    let disk = dir(&root);
    let before = put(&disk, "workflows.json", "[]");
    let big = workflow_with(
        ID,
        json!({"description": "x".repeat(WORKFLOWS_FILE_BUDGET)}),
    );
    let error = update_workflows(&disk, |_| Ok((vec![big.clone()], ()))).unwrap_err();
    assert_eq!(
        error,
        "The workflows are too large to save (over 2 MiB in total). Remove a workflow or shorten its steps."
    );
    assert_eq!(files::content_hash(&text(&disk, "workflows.json")), before);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn update_workflows_keeps_an_entry_it_cannot_read_untouched() {
    let root = temp_root();
    let disk = dir(&root);
    let strange = json!({
        "id": "wf-strange01", "name": "From a newer app",
        "nodes": [{"id": "trigger", "type": "manual"}, {"id": "x", "type": "teleport", "beam": [1, 2]}],
        "edges": [], "futureField": {"keep": true}
    });
    put(
        &disk,
        "workflows.json",
        &json!([serde_json::to_value(workflow()).unwrap(), strange]).to_string(),
    );
    update_workflows(&disk, |list| {
        let renamed = list
            .into_iter()
            .map(|mut w| {
                w.name = "Renamed".into();
                w
            })
            .collect();
        Ok((renamed, ()))
    })
    .unwrap();
    let written: serde_json::Value = serde_json::from_str(&text(&disk, "workflows.json")).unwrap();
    assert_eq!(written.as_array().unwrap().len(), 2);
    assert_eq!(written[0]["id"], ID);
    assert_eq!(written[0]["name"], "Renamed");
    assert_eq!(written[1], strange);
    // The panel still lists only what it can read.
    let loaded = load_workflows(&disk).unwrap();
    assert_eq!(
        loaded
            .workflows
            .iter()
            .map(|w| w.id.as_str())
            .collect::<Vec<_>>(),
        [ID]
    );
    assert_eq!(loaded.unreadable.len(), 1);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn update_workflows_keeps_unreadable_entries_that_are_not_objects() {
    let root = temp_root();
    let disk = dir(&root);
    put(
        &disk,
        "workflows.json",
        &json!([3, null, serde_json::to_value(workflow()).unwrap(), "text"]).to_string(),
    );
    update_workflows(&disk, |list| Ok((list, ()))).unwrap();
    let written: serde_json::Value = serde_json::from_str(&text(&disk, "workflows.json")).unwrap();
    assert_eq!(
        written,
        json!([serde_json::to_value(workflow()).unwrap(), 3, null, "text"])
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn update_workflows_refuses_to_add_an_id_that_an_unreadable_entry_already_has() {
    let root = temp_root();
    let disk = dir(&root);
    let original = json!([{"id": "wf-zzzz9999", "nodes": "broken"}]).to_string();
    put(&disk, "workflows.json", &original);
    let error = update_workflows(&disk, |mut list| {
        list.push(workflow_with("wf-zzzz9999", json!({})));
        Ok((list, ()))
    })
    .unwrap_err();
    assert_eq!(
        error,
        "workflows.json already has an entry with the id \"wf-zzzz9999\" that could not be read; fix or remove it first"
    );
    assert_eq!(text(&disk, "workflows.json"), original);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn update_workflows_refuses_to_write_over_a_file_that_is_not_valid_json() {
    let root = temp_root();
    let disk = dir(&root);
    put(&disk, "workflows.json", "{ not json");
    let mut called = false;
    let error = update_workflows(&disk, |list| {
        called = true;
        Ok((list, 1))
    })
    .unwrap_err();
    assert_eq!(
        error,
        "workflows.json could not be read; fix or remove the file before changing workflows"
    );
    assert_eq!(text(&disk, "workflows.json"), "{ not json");
    assert!(!called);
    // Reading it gives an empty list rather than an error.
    assert!(load_workflows(&disk).unwrap().workflows.is_empty());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn update_workflows_refuses_to_write_over_a_file_that_is_not_a_list() {
    let root = temp_root();
    let disk = dir(&root);
    put(&disk, "workflows.json", r#"{"workflows": []}"#);
    let error = update_workflows(&disk, |list| Ok((list, 1))).unwrap_err();
    assert!(error.contains("could not be read"), "{error}");
    assert_eq!(text(&disk, "workflows.json"), r#"{"workflows": []}"#);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn update_workflows_treats_a_missing_or_blank_file_as_an_empty_list() {
    let root = temp_root();
    let disk = dir(&root);
    assert!(load_workflows(&disk).unwrap().workflows.is_empty());
    update_workflows(&disk, |_| Ok((vec![workflow()], 1))).unwrap();
    assert_eq!(ids(&text(&disk, "workflows.json")), vec![ID]);
    put(&disk, "workflows.json", "  \n");
    update_workflows(&disk, |_| Ok((vec![workflow()], 1))).unwrap();
    assert_eq!(ids(&text(&disk, "workflows.json")), vec![ID]);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn update_runs_refuses_to_write_over_a_runs_file_that_is_not_valid_json() {
    let root = temp_root();
    let disk = dir(&root);
    put(&disk, RUNS, "[{ broken");
    let error = update_runs(&disk, ID, |runs| Ok((runs, 1))).unwrap_err();
    assert_eq!(
        error,
        "workflow-runs/wf-abcd1234.json could not be read; fix or remove the file before changing the run history"
    );
    assert_eq!(text(&disk, RUNS), "[{ broken");
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn update_runs_keeps_run_entries_it_cannot_read() {
    let root = temp_root();
    let disk = dir(&root);
    let odd = json!({"id": "run-odd", "note": "from a newer app"});
    put(
        &disk,
        RUNS,
        &json!([serde_json::to_value(run("a")).unwrap(), odd]).to_string(),
    );
    update_runs(&disk, ID, |runs| {
        let mut all = vec![run("b")];
        all.extend(runs);
        Ok((all, 1))
    })
    .unwrap();
    assert_eq!(ids(&text(&disk, RUNS)), vec!["b", "a", "run-odd"]);
    let written: serde_json::Value = serde_json::from_str(&text(&disk, RUNS)).unwrap();
    assert_eq!(written[2], odd);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn update_runs_keeps_a_run_this_app_cannot_decode_but_the_other_could_show() {
    // Passes the loose shape check, but the status is not one this app knows.
    let root = temp_root();
    let disk = dir(&root);
    let mut odd = serde_json::to_value(run("odd")).unwrap();
    odd["status"] = json!("paused");
    put(&disk, RUNS, &json!([odd]).to_string());
    assert!(load_runs(&disk, ID).unwrap().is_empty());
    update_runs(&disk, ID, |_| Ok((vec![run("new")], ()))).unwrap();
    let written: serde_json::Value = serde_json::from_str(&text(&disk, RUNS)).unwrap();
    assert_eq!(written[1], odd);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn update_workflows_drops_a_duplicate_id_leftover_together_with_the_removed_workflow() {
    let root = temp_root();
    let disk = dir(&root);
    let twin = json!({"id": ID, "name": "Twin", "nodes": "broken"});
    let other = json!({"id": "wf-other000", "nodes": "broken"});
    let readable = serde_json::to_value(workflow()).unwrap();
    let original = json!([readable, twin, other]).to_string();
    put(&disk, "workflows.json", &original);
    update_workflows(&disk, |_| Ok((Vec::new(), ()))).unwrap();
    // The unreadable entry with another id stays; the twin of the removed workflow goes.
    let written: serde_json::Value = serde_json::from_str(&text(&disk, "workflows.json")).unwrap();
    assert_eq!(written, json!([other]));
    // While the readable one stays, so does its twin.
    put(&disk, "workflows.json", &original);
    update_workflows(&disk, |list| Ok((list, ()))).unwrap();
    let kept: serde_json::Value = serde_json::from_str(&text(&disk, "workflows.json")).unwrap();
    assert_eq!(kept, json!([readable, twin, other]));
    std::fs::remove_dir_all(root).unwrap();
}

// ---- Cross-app parity ---------------------------------------------------------------------

/// What the TS store writes for the same list (generated by running `readWorkflowFile` and
/// `JSON.stringify(list, null, 2)` over the input): every node kind, an unreadable entry from a
/// newer app, non-object leftovers, odd numbers.
const TS_WORKFLOWS: &str = include_str!("fixtures/store_workflows.json");
/// A runs file as the TS store writes it: two runs and an entry it could not read.
const TS_RUNS: &str = include_str!("fixtures/store_runs.json");

#[test]
fn a_file_written_by_the_ts_store_round_trips_byte_identical() {
    let root = temp_root();
    let disk = dir(&root);
    put(&disk, "workflows.json", TS_WORKFLOWS);
    let loaded = load_workflows(&disk).unwrap();
    assert_eq!(loaded.workflows.len(), 1);
    assert_eq!(loaded.unreadable.len(), 4);
    assert_eq!(loaded.hash, files::content_hash(TS_WORKFLOWS));
    update_workflows(&disk, |list| Ok((list, ()))).unwrap();
    assert_eq!(text(&disk, "workflows.json"), TS_WORKFLOWS);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn a_runs_file_written_by_the_ts_store_round_trips_byte_identical() {
    let root = temp_root();
    let disk = dir(&root);
    put(&disk, RUNS, TS_RUNS);
    let runs = load_runs(&disk, ID).unwrap();
    assert_eq!(
        runs.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(),
        ["run-2", "run-1"]
    );
    assert_eq!(runs[0].nodes[1].history.as_ref().map(Vec::len), Some(1));
    assert_eq!(runs[0].items["each"].len(), 2);
    update_runs(&disk, ID, |runs| Ok((runs, ()))).unwrap();
    assert_eq!(text(&disk, RUNS), TS_RUNS);
    // Changing one run leaves the others exactly as they were written.
    update_runs(&disk, ID, |mut runs| {
        runs[1].error = Some("changed".into());
        Ok((runs, ()))
    })
    .unwrap();
    let before: serde_json::Value = serde_json::from_str(TS_RUNS).unwrap();
    let after: serde_json::Value = serde_json::from_str(&text(&disk, RUNS)).unwrap();
    assert_eq!(after[0], before[0]);
    assert_eq!(after[1]["error"], "changed");
    assert_eq!(after[2], before[2]);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn a_workflow_written_here_has_the_ts_layout() {
    let root = temp_root();
    let disk = dir(&root);
    let w = workflow_with(
        ID,
        json!({"description": "d", "nextRunAt": 5, "runRequested": true,
               "inputValues": {"limit": 2.5}, "name": "Q \"x\" é"}),
    );
    update_workflows(&disk, |_| Ok((vec![w.clone()], ()))).unwrap();
    let written = text(&disk, "workflows.json");
    assert!(written.starts_with("[\n  {\n    \"id\": \"wf-abcd1234\",\n    \"name\": \"Q \\\"x\\\" é\",\n    \"description\": \"d\",\n"));
    assert!(written.contains("\"limit\": 2.5"));
    assert!(written.contains("\"createdAt\": 1,\n"));
    assert!(written.ends_with("  }\n]\n"));
    assert_eq!(load_workflows(&disk).unwrap().workflows, vec![w]);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn runs_written_here_load_back_and_look_like_ts_runs() {
    let root = temp_root();
    let disk = dir(&root);
    let mut r = fat("r1", 10);
    r.items.insert(
        "each".into(),
        vec![Json::Number(1.0), Json::String("a".into())],
    );
    r.nodes[0].history = Some(vec![RoundRecord {
        round: 1,
        status: NodeStatus::Done,
        session_id: Some("s1".into()),
        error: None,
        decision: None,
        started_at: Some(1),
        ended_at: None,
    }]);
    update_runs(&disk, ID, |_| Ok((vec![r.clone()], ()))).unwrap();
    let written = text(&disk, RUNS);
    assert!(written.ends_with("}\n]\n"));
    let value: serde_json::Value = serde_json::from_str(&written).unwrap();
    assert_eq!(value[0]["workflowId"], ID);
    assert_eq!(value[0]["items"]["each"], json!([1, "a"]));
    assert_eq!(load_runs(&disk, ID).unwrap(), vec![r]);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn clear_runs_writes_an_empty_list() {
    let root = temp_root();
    let disk = dir(&root);
    update_runs(&disk, ID, |_| Ok((vec![run("a")], ()))).unwrap();
    clear_runs(&disk, ID);
    assert_eq!(text(&disk, RUNS), "[]\n");
    assert!(load_runs(&disk, ID).unwrap().is_empty());
    // Failing is harmless: a bad id or a missing file never panics.
    clear_runs(&disk, "not-an-id");
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn load_workflows_reports_the_hash_of_the_file() {
    let root = temp_root();
    let disk = dir(&root);
    assert_eq!(load_workflows(&disk).unwrap().hash, files::content_hash(""));
    let hash = put(&disk, "workflows.json", &list(&[workflow()]));
    assert_eq!(load_workflows(&disk).unwrap().hash, hash);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn a_schedule_is_written_in_the_ts_key_order() {
    let root = temp_root();
    let disk = dir(&root);
    let w = workflow_with(
        ID,
        json!({"nodes": [
            {"id": "trigger", "type": "schedule", "schedule": {
                "scheduleKind": "interval", "time": "09:30", "minute": 5, "dayOfWeek": 2,
                "intervalHours": 4, "intervalFrom": "2026-01-01"}},
            {"id": "report", "type": "report"}
        ]}),
    );
    let order = "\"scheduleKind\": \"interval\",\n";
    let keys = |text: &str| -> Vec<String> {
        let at = text.find(order).unwrap();
        text[at..]
            .lines()
            .take(6)
            .map(|l| l.trim().split('"').nth(1).unwrap().to_string())
            .collect()
    };
    let expected = [
        "scheduleKind",
        "time",
        "minute",
        "dayOfWeek",
        "intervalHours",
        "intervalFrom",
    ];
    update_workflows(&disk, |_| Ok((vec![w.clone()], ()))).unwrap();
    assert_eq!(keys(&text(&disk, "workflows.json")), expected);
    let mut r = run("r1");
    r.definition = w;
    update_runs(&disk, ID, |_| Ok((vec![r.clone()], ()))).unwrap();
    assert_eq!(keys(&text(&disk, RUNS)), expected);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn update_workflows_does_not_write_when_the_edit_changes_nothing() {
    let root = temp_root();
    let racy = Racy::new(dir(&root));
    put(&racy.disk, "workflows.json", &list(&[workflow()]));
    let result = update_workflows(&racy, |list| Ok((list, 7))).unwrap();
    assert_eq!(result, 7);
    assert!(racy.saves.borrow().is_empty());
    // A real change still writes.
    update_workflows(&racy, |mut list| {
        list[0].name = "Renamed".into();
        Ok((list, ()))
    })
    .unwrap();
    assert_eq!(racy.saves.borrow().len(), 1);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn take_requests_gives_each_resume_to_exactly_one_taker() {
    let root = temp_root();
    let disk = dir(&root);
    put(
        &disk,
        "workflows.json",
        &list(&[workflow_with(ID, json!({"resumeRunId": "run-old"}))]),
    );
    let taken: Vec<_> = (0..3)
        .map(|_| take_requests(&disk, |_| false).unwrap())
        .collect();
    let resumes: Vec<_> = taken.iter().flat_map(|t| t.resumes.clone()).collect();
    assert_eq!(
        resumes,
        vec![ResumeRequest {
            workflow_id: ID.into(),
            run_id: "run-old".into()
        }]
    );
    assert_eq!(
        load_workflows(&disk).unwrap().workflows[0].resume_run_id,
        None
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn take_requests_clears_a_stop_nobody_runs_and_hands_over_one_that_is_going() {
    let root = temp_root();
    let disk = dir(&root);
    put(
        &disk,
        "workflows.json",
        &list(&[
            workflow_with(ID, json!({"stopRequested": true})),
            workflow_with("wf-other000", json!({"stopRequested": true})),
        ]),
    );
    let taken = take_requests(&disk, |id| id == ID).unwrap();
    assert_eq!(taken.stops, vec![ID.to_string()]);
    assert!(taken.resumes.is_empty());
    let after = load_workflows(&disk).unwrap().workflows;
    // Both flags are answered: one is applied, the other was meant for a run that is gone.
    assert!(after.iter().all(|w| !w.stop_requested));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn take_requests_leaves_the_file_alone_when_nothing_was_asked() {
    let root = temp_root();
    let racy = Racy::new(dir(&root));
    put(&racy.disk, "workflows.json", &list(&[workflow()]));
    let taken = take_requests(&racy, |_| true).unwrap();
    assert_eq!(taken, Requests::default());
    assert!(racy.saves.borrow().is_empty());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn last_run_of_tells_never_ran_from_unreadable() {
    let root = temp_root();
    let d = dir(&root);
    assert_eq!(last_run_of(&d, ID), Ok(None));
    d.save(RUNS, "[]", None).unwrap();
    assert_eq!(last_run_of(&d, ID), Ok(None));
    let h = d.read(RUNS).unwrap().hash;
    d.save(RUNS, "{ not json", Some(&h)).unwrap();
    assert!(last_run_of(&d, ID).is_err());
    assert!(load_runs(&d, ID).unwrap().is_empty());
    let h = d.read(RUNS).unwrap().hash;
    d.save(RUNS, "[{\"junk\": true}]", Some(&h)).unwrap();
    assert!(last_run_of(&d, ID).is_err());
}
