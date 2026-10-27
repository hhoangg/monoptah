//! Builders shared by the board and run graph tests, ported from `workflowUiFixtures.ts`.
//! `ticket_workflow` lives in `fixtures.rs`.
use super::fixtures::node;
use super::model::{Edge, Node, RunStatus, RunTrigger, Workflow, WorkflowRun};
use serde_json::{json, Value};
use std::collections::BTreeMap;

pub fn n(v: Value) -> Node {
    node(&v.to_string())
}

pub fn manual() -> Node {
    n(json!({"id": "trigger", "type": "manual"}))
}

pub fn report() -> Node {
    n(json!({"id": "report", "type": "report"}))
}

/// trigger -> report, enabled.
pub fn workflow() -> Workflow {
    let mut w = Workflow::new("wf-aaaaaaaa", "Nightly checks");
    w.nodes = vec![manual(), report()];
    w.edges = vec![Edge::new("trigger", "report")];
    w.enabled = true;
    w.created_at = 1;
    w
}

/// A finished manual run of `workflow()` with no node runs.
pub fn run() -> WorkflowRun {
    WorkflowRun {
        id: "run-1".into(),
        workflow_id: "wf-aaaaaaaa".into(),
        definition: workflow(),
        status: RunStatus::Done,
        started_at: 1_000_000,
        ended_at: Some(1_060_000),
        trigger: RunTrigger::Manual,
        folder_id: None,
        items: BTreeMap::new(),
        nodes: Vec::new(),
        report_posted_at: None,
        error: None,
    }
}
