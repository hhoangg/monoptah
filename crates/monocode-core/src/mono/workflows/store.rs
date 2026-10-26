//! A Mono's `workflows.json` and `workflow-runs/<workflow id>.json`, in the format the Tauri app
//! writes (`workflowStore.ts`): both apps read and replace each other's files. Writes go through
//! the Mono files API, so they are atomic and checked against the hash that was read; a save
//! that loses the race reads again and re-applies the edit, three tries in all.
//!
//! Entries this app cannot read (made by a newer version, or edited by hand) are kept as they
//! were and written after the others, and a file that is not a JSON list is never overwritten.

use std::collections::HashSet;
use std::path::PathBuf;

use serde::Serialize;

use super::model::{Workflow, WorkflowRun, WORKFLOW_RUNS_KEPT};
use super::ordered::{Json, OrderedMap};
use super::parse::read_workflow_file;
use super::template::js_trim;
use crate::mono::files::{self, AgentFile, CONFLICT, WORKFLOWS_FILE};

/// Stay under the 2 MiB cap `files.rs` puts on workflows.json.
pub const WORKFLOWS_FILE_BUDGET: usize = 2 * 1024 * 1024;
/// Stay under the 4 MiB cap `files.rs` puts on a runs file.
pub const RUNS_FILE_BUDGET: usize = 3 * 1024 * 1024 + 512 * 1024;
/// What a save that lost every race says (`MonoFileConflict` in the Tauri app).
const CHANGED: &str = "The file changed while you were editing";
const ATTEMPTS: usize = 3;

/// The two calls the store needs from a Mono's files, so tests can interleave a rival writer.
pub trait MonoFiles {
    fn read(&self, relative: &str) -> Result<AgentFile, String>;
    /// Saves over `expected` (the hash that was read); `Err(CONFLICT)` when the file moved.
    fn save(&self, relative: &str, text: &str, expected: Option<&str>) -> Result<String, String>;
}

/// One Mono's folder under the app's `monos/` directory.
#[derive(Clone, Debug)]
pub struct MonoDir {
    root: PathBuf,
    mono: String,
}

impl MonoDir {
    pub fn new(root: impl Into<PathBuf>, mono: impl Into<String>) -> Self {
        Self {
            root: root.into(),
            mono: mono.into(),
        }
    }
}

impl MonoFiles for MonoDir {
    fn read(&self, relative: &str) -> Result<AgentFile, String> {
        files::read(&self.root, &self.mono, None, relative)
    }
    fn save(&self, relative: &str, text: &str, expected: Option<&str>) -> Result<String, String> {
        files::save(&self.root, &self.mono, None, relative, text, expected)
    }
}

fn runs_path(workflow_id: &str) -> String {
    format!("workflow-runs/{workflow_id}.json")
}

/// The file can only be changed through this app's own format; anything else is left for the
/// user to fix.
fn unreadable_error(file: &str, what: &str) -> String {
    format!("{file} could not be read; fix or remove the file before changing {what}")
}

/// An entry as JSON, in the key order the serializer writes.
pub(super) fn to_json<T: Serialize>(value: &T) -> Result<Json, String> {
    let text = serde_json::to_string(value).map_err(|e| e.to_string())?;
    Json::parse(&text).ok_or_else(|| "Could not serialize an entry".to_string())
}

/// The key order `parseSchedule` gives a schedule; `habits::Schedule` serializes `minute` first.
const SCHEDULE_KEYS: [&str; 6] = [
    "scheduleKind",
    "time",
    "minute",
    "dayOfWeek",
    "intervalHours",
    "intervalFrom",
];

/// Puts the schedule of every schedule node of a serialized definition in the order the Tauri
/// app writes, so a file both apps edit does not change layout each time.
fn ts_schedule_order(definition: &mut Json) {
    let Json::Object(definition) = definition else {
        return;
    };
    let Some(Json::Array(nodes)) = definition.get_mut("nodes") else {
        return;
    };
    for node in nodes {
        let Json::Object(node) = node else { continue };
        let Some(Json::Object(schedule)) = node.get_mut("schedule") else {
            continue;
        };
        let mut old = std::mem::take(schedule);
        for key in SCHEDULE_KEYS {
            if let Some(value) = old.remove(key) {
                schedule.insert(key, value);
            }
        }
        for (key, value) in old.iter() {
            schedule.insert(key.clone(), value.clone());
        }
    }
}

pub(super) fn workflow_json(workflow: &Workflow) -> Result<Json, String> {
    let mut json = to_json(workflow)?;
    ts_schedule_order(&mut json);
    Ok(json)
}

fn run_json(run: &WorkflowRun) -> Result<Json, String> {
    let mut json = to_json(run)?;
    if let Json::Object(fields) = &mut json {
        if let Some(definition) = fields.get_mut("definition") {
            ts_schedule_order(definition);
        }
    }
    Ok(json)
}

/// `JSON.stringify(entries, null, 2) + "\n"`.
fn pretty_list(entries: &[String]) -> String {
    if entries.is_empty() {
        return "[]\n".into();
    }
    format!("[\n{}\n]\n", entries.join(",\n"))
}

/// One entry of a pretty list, indented one level (a string never holds a raw newline).
fn list_entry(entry: &Json) -> String {
    format!("  {}", entry.stringify(true).replace('\n', "\n  "))
}

/// The byte length of `pretty_list` over entries of these lengths.
fn list_len(lengths: impl Iterator<Item = usize>) -> usize {
    let (count, sum) = lengths.fold((0, 0), |(n, sum), len| (n + 1, sum + len));
    if count == 0 {
        3
    } else {
        sum + 2 * (count - 1) + 5
    }
}

/// What the workflows file holds right now.
#[derive(Clone, Debug)]
pub struct LoadedWorkflows {
    pub workflows: Vec<Workflow>,
    /// Entries this app cannot read, as written.
    pub unreadable: Vec<Json>,
    /// The hash of the file as read, for a later hash-checked save.
    pub hash: String,
}

/// The Mono's workflows from its folder. A file that is not a JSON list reads as empty.
pub fn load_workflows(files: &impl MonoFiles) -> Result<LoadedWorkflows, String> {
    let file = files.read(WORKFLOWS_FILE)?;
    let current = read_workflow_file(file.text.as_deref().unwrap_or(""));
    let (workflows, unreadable) =
        current.map_or_else(Default::default, |c| (c.workflows, c.unreadable));
    Ok(LoadedWorkflows {
        workflows,
        unreadable,
        hash: file.hash,
    })
}

fn serialize_workflows(workflows: &[Workflow], unreadable: &[&Json]) -> Result<String, String> {
    let mut entries = Vec::with_capacity(workflows.len() + unreadable.len());
    for workflow in workflows {
        entries.push(list_entry(&workflow_json(workflow)?));
    }
    entries.extend(unreadable.iter().map(|raw| list_entry(raw)));
    let text = pretty_list(&entries);
    // Say so here rather than let the host fail with a raw "is too large".
    if text.len() > WORKFLOWS_FILE_BUDGET {
        return Err("The workflows are too large to save (over 2 MiB in total). Remove a workflow or shorten its steps.".into());
    }
    Ok(text)
}

fn id_of(raw: &Json) -> Option<&str> {
    raw.get("id").and_then(Json::as_str)
}

/// Change workflows.json over the version that was read, retrying when the panel or another run
/// saved in between. `edit` gets the readable workflows and returns the new list and a result;
/// it may run more than once. An error from it is returned as is, with nothing written.
pub fn update_workflows<T>(
    files: &impl MonoFiles,
    mut edit: impl FnMut(Vec<Workflow>) -> Result<(Vec<Workflow>, T), String>,
) -> Result<T, String> {
    for attempt in 0.. {
        let file = files.read(WORKFLOWS_FILE)?;
        let current = read_workflow_file(file.text.as_deref().unwrap_or(""))
            .ok_or_else(|| unreadable_error(WORKFLOWS_FILE, "workflows"))?;
        let known: HashSet<&str> = current.workflows.iter().map(|w| w.id.as_str()).collect();
        let (workflows, result) = edit(current.workflows.clone())?;
        // An id the file already uses, in an entry that cannot be read, must not be used again.
        let taken: HashSet<&str> = current.unreadable.iter().filter_map(id_of).collect();
        if let Some(w) = workflows
            .iter()
            .find(|w| !known.contains(w.id.as_str()) && taken.contains(w.id.as_str()))
        {
            return Err(format!(
                "workflows.json already has an entry with the id \"{}\" that could not be read; fix or remove it first",
                w.id
            ));
        }
        // A leftover that repeats the id of a workflow the edit removed would come back as that
        // workflow on the next read, so it goes with it.
        let kept: HashSet<&str> = workflows.iter().map(|w| w.id.as_str()).collect();
        let leftovers: Vec<&Json> = current
            .unreadable
            .iter()
            .filter(|raw| id_of(raw).is_none_or(|id| !known.contains(id) || kept.contains(id)))
            .collect();
        let text = serialize_workflows(&workflows, &leftovers)?;
        // An edit that changes nothing (a schedule that cannot move on is "advanced" every tick)
        // leaves the file alone: no write, no new hash for a rival to lose a race against.
        if file.text.as_deref() == Some(text.as_str()) {
            return Ok(result);
        }
        match files.save(WORKFLOWS_FILE, &text, Some(&file.hash)) {
            Ok(_) => return Ok(result),
            Err(e) if e == CONFLICT && attempt + 1 < ATTEMPTS => {}
            Err(e) if e == CONFLICT => return Err(CHANGED.into()),
            Err(e) => return Err(e),
        }
    }
    unreachable!("the loop returns on its last attempt")
}

/// A resume another window (or the CLI) asked for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResumeRequest {
    pub workflow_id: String,
    pub run_id: String,
}

/// What `take_requests` found in the file.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Requests {
    pub resumes: Vec<ResumeRequest>,
    /// Workflows with a run going whose stop was asked for.
    pub stops: Vec<String>,
}

/// Takes the requests left in the workflow file (`takeWorkflowRequests`), clearing them in the
/// same hash-checked save so of several takers exactly one gets each resume. A stop is cleared
/// too: when `is_running` says a run is going it is handed over to be applied (the TS driver
/// leaves it for the run's own poll; here the caller is the driver), otherwise it was meant for a
/// run that is gone. Writes nothing when no request was made.
pub fn take_requests(
    files: &impl MonoFiles,
    is_running: impl Fn(&str) -> bool,
) -> Result<Requests, String> {
    update_workflows(files, |mut list| {
        let mut taken = Requests::default();
        for w in &mut list {
            if let Some(run_id) = w.resume_run_id.take() {
                taken.resumes.push(ResumeRequest {
                    workflow_id: w.id.clone(),
                    run_id,
                });
            }
            if std::mem::take(&mut w.stop_requested) && is_running(&w.id) {
                taken.stops.push(w.id.clone());
            }
        }
        Ok((list, taken))
    })
}

// ---- Runs ---------------------------------------------------------------------------------

/// A run that passed the loose shape check and decoded, with the JSON it was read from so a run
/// the edit leaves alone is written back exactly as it was (fields this app does not know,
/// key order).
struct Stored {
    run: WorkflowRun,
    raw: Json,
}

struct RunsFile {
    runs: Vec<Stored>,
    unreadable: Vec<Json>,
}

const NODE_STATUSES: [&str; 7] = [
    "pending",
    "running",
    "waitingApproval",
    "done",
    "failed",
    "skipped",
    "stopped",
];

fn optional(raw: &OrderedMap<Json>, key: &str, ok: impl Fn(&Json) -> bool) -> bool {
    raw.get(key).is_none_or(ok)
}

fn is_number(v: &Json) -> bool {
    matches!(v, Json::Number(_))
}

fn is_string(v: &Json) -> bool {
    matches!(v, Json::String(_))
}

fn is_round_record(raw: &Json) -> bool {
    let Some(record) = raw.as_object() else {
        return false;
    };
    matches!(record.get("round"), Some(Json::Number(_)))
        && record
            .get("status")
            .and_then(Json::as_str)
            .is_some_and(|s| NODE_STATUSES.contains(&s))
        && optional(record, "sessionId", is_string)
        && optional(record, "error", is_string)
        && optional(record, "startedAt", is_number)
        && optional(record, "endedAt", is_number)
        && optional(record, "decision", |d| {
            d.as_object().is_some_and(|d| {
                matches!(d.get("value"), Some(Json::String(_))) && optional(d, "reason", is_string)
            })
        })
}

fn has_history(node: &Json) -> bool {
    node.as_object().is_some_and(|n| n.contains_key("history"))
}

/// Drops the per-round records of a run that cannot be read; the rest of the run stays as
/// saved. Records saved before rounds stopped keeping their output still carry it; it is dropped.
fn with_clean_history(raw: Json) -> Json {
    let Json::Object(run) = &raw else {
        return raw;
    };
    let Some(Json::Array(nodes)) = run.get("nodes") else {
        return raw;
    };
    if !nodes.iter().any(has_history) {
        return raw;
    }
    let clean_node = |node: &Json| -> Json {
        let Json::Object(fields) = node else {
            return node.clone();
        };
        if !fields.contains_key("history") {
            return node.clone();
        }
        let mut out: OrderedMap<Json> = fields
            .iter()
            .filter(|(k, _)| k.as_str() != "history")
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        let kept: Vec<Json> = match fields.get("history") {
            Some(Json::Array(history)) => history
                .iter()
                .filter(|record| is_round_record(record))
                .map(|record| match record {
                    Json::Object(r) => Json::Object(
                        r.iter()
                            .filter(|(k, _)| k.as_str() != "output")
                            .map(|(k, v)| (k.clone(), v.clone()))
                            .collect(),
                    ),
                    other => other.clone(),
                })
                .collect(),
            _ => Vec::new(),
        };
        if !kept.is_empty() {
            out.insert("history", Json::Array(kept));
        }
        Json::Object(out)
    };
    Json::Object(
        run.iter()
            .map(|(k, v)| match (k.as_str(), v) {
                ("nodes", Json::Array(nodes)) => (
                    k.clone(),
                    Json::Array(nodes.iter().map(clean_node).collect()),
                ),
                _ => (k.clone(), v.clone()),
            })
            .collect(),
    )
}

/// Runs are written by the app, so only the shape the engine relies on is checked.
fn is_run(raw: &Json) -> bool {
    let Some(run) = raw.as_object() else {
        return false;
    };
    matches!(run.get("id"), Some(Json::String(_)))
        && matches!(run.get("workflowId"), Some(Json::String(_)))
        && matches!(run.get("status"), Some(Json::String(_)))
        && matches!(run.get("startedAt"), Some(Json::Number(_)))
        && matches!(
            run.get("definition"),
            Some(Json::Object(_) | Json::Array(_))
        )
        && matches!(run.get("nodes"), Some(Json::Array(_)))
}

/// None when the text is not a JSON list, so a caller can refuse to overwrite it. A run that
/// passes the shape check but cannot be decoded here (a status or definition from a newer app)
/// is kept with the unreadable entries.
fn read_runs_file(text: &str) -> Option<RunsFile> {
    let mut file = RunsFile {
        runs: Vec::new(),
        unreadable: Vec::new(),
    };
    if js_trim(text).is_empty() {
        return Some(file);
    }
    let Json::Array(entries) = Json::parse(text)? else {
        return None;
    };
    for raw in entries {
        if !is_run(&raw) {
            file.unreadable.push(raw);
            continue;
        }
        let cleaned = with_clean_history(raw.clone());
        match serde_json::from_str::<WorkflowRun>(&cleaned.stringify(false)) {
            Ok(run) => file.runs.push(Stored { run, raw: cleaned }),
            Err(_) => file.unreadable.push(raw),
        }
    }
    Some(file)
}

/// A workflow's runs, newest first (the order of the file). A workflow that never ran has none,
/// and a file that is not a JSON list reads as empty.
pub fn load_runs(files: &impl MonoFiles, workflow_id: &str) -> Result<Vec<WorkflowRun>, String> {
    let file = files.read(&runs_path(workflow_id))?;
    Ok(read_runs_file(file.text.as_deref().unwrap_or(""))
        .map(|f| f.runs.into_iter().map(|s| s.run).collect())
        .unwrap_or_default())
}

/// The newest runs that fit: at most 20 and under the size budget, never fewer than one. Entries
/// that could not be read follow them, untouched.
fn serialize_runs(runs: &[Json], unreadable: &[Json]) -> String {
    let mut kept: Vec<String> = runs
        .iter()
        .take(WORKFLOW_RUNS_KEPT)
        .map(list_entry)
        .collect();
    let rest: Vec<String> = unreadable.iter().map(list_entry).collect();
    while kept.len() > 1 && list_len(kept.iter().chain(&rest).map(String::len)) > RUNS_FILE_BUDGET {
        kept.pop();
    }
    kept.extend(rest);
    pretty_list(&kept)
}

/// Change a workflow's run history the same way, keeping only the newest runs. `edit` gets the
/// runs newest first and returns them the same way.
pub fn update_runs<T>(
    files: &impl MonoFiles,
    workflow_id: &str,
    mut edit: impl FnMut(Vec<WorkflowRun>) -> Result<(Vec<WorkflowRun>, T), String>,
) -> Result<T, String> {
    let path = runs_path(workflow_id);
    for attempt in 0.. {
        let file = files.read(&path)?;
        let current = read_runs_file(file.text.as_deref().unwrap_or(""))
            .ok_or_else(|| unreadable_error(&path, "the run history"))?;
        let mut originals: Vec<Option<Stored>> = current.runs.into_iter().map(Some).collect();
        let (runs, result) = edit(originals.iter().flatten().map(|s| s.run.clone()).collect())?;
        let mut entries = Vec::with_capacity(runs.len());
        for run in &runs {
            let same = originals
                .iter_mut()
                .find(|slot| slot.as_ref().is_some_and(|s| s.run == *run));
            entries.push(match same.and_then(Option::take) {
                Some(stored) => stored.raw,
                None => run_json(run)?,
            });
        }
        let text = serialize_runs(&entries, &current.unreadable);
        match files.save(&path, &text, Some(&file.hash)) {
            Ok(_) => return Ok(result),
            Err(e) if e == CONFLICT && attempt + 1 < ATTEMPTS => {}
            Err(e) if e == CONFLICT => return Err(CHANGED.into()),
            Err(e) => return Err(e),
        }
    }
    unreachable!("the loop returns on its last attempt")
}

/// Empties the run history of a workflow that was removed, so nothing of it stays on disk.
/// Failing to is harmless, so it never fails.
pub fn clear_runs(files: &impl MonoFiles, workflow_id: &str) {
    if let Err(error) = files.save(&runs_path(workflow_id), "[]\n", None) {
        eprintln!("Could not clear the runs of a removed workflow: {error}");
    }
}
