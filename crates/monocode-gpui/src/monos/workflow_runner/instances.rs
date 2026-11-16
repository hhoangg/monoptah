//! The cloud instances a run used, for its report: which instance, whether the run created it,
//! and which steps ran on it. Read from the run record, so it holds after a resume too.
use super::CloudInstanceInfo;
use monocode_core::mono::workflows::model::{NodeKind, TargetInstance, WorkflowRun};
use std::collections::BTreeSet;

/// One session of a cloud step.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Use {
    pub node_id: String,
    pub session_id: String,
    /// The kind the step asked for, as written (`codespace` or `sandbox`).
    pub kind: String,
    /// The step created the instance for this session.
    pub created: bool,
}

/// The sessions of the run's cloud steps, in the order of its nodes. Of the sessions one node ran
/// in one branch (one per loop round at most), only the first can have created an instance, and
/// only for a step that asks for a new one: later rounds and `sameAs` steps go on in an instance
/// that exists.
pub(super) fn cloud_uses(run: &WorkflowRun) -> Vec<Use> {
    let mut uses = Vec::new();
    for node_run in &run.nodes {
        let Some(NodeKind::Session {
            target: Some(target),
            ..
        }) = run
            .definition
            .nodes
            .iter()
            .find(|n| n.id == node_run.node_id)
            .map(|n| &n.kind)
        else {
            continue;
        };
        let creates = matches!(target.instance, TargetInstance::New);
        let mut seen = BTreeSet::new();
        let sessions = node_run
            .history
            .iter()
            .flatten()
            .filter_map(|round| round.session_id.as_ref())
            .chain(node_run.interrupted_sessions.iter().flatten())
            .chain(node_run.session_id.iter());
        for id in sessions {
            if !seen.insert(id.clone()) {
                continue;
            }
            uses.push(Use {
                node_id: node_run.node_id.clone(),
                session_id: id.clone(),
                kind: target.kind.clone(),
                created: creates && seen.len() == 1,
            });
        }
    }
    uses
}

/// One instance and everything the run did on it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Listed {
    pub kind: String,
    /// `None` while the instance has no name yet.
    pub name: Option<String>,
    pub created: bool,
    pub nodes: Vec<String>,
}

/// Groups the uses by instance. An instance without a name yet is told apart by its session.
pub(super) fn listed(
    uses: &[Use],
    lookup: impl Fn(&str) -> Option<CloudInstanceInfo>,
) -> Vec<Listed> {
    let mut out: Vec<(String, Listed)> = Vec::new();
    for used in uses {
        let info = lookup(&used.session_id).unwrap_or(CloudInstanceInfo {
            kind: used.kind.clone(),
            name: None,
        });
        let key = match &info.name {
            Some(name) => format!("{}:{name}", info.kind),
            None => format!("session:{}", used.session_id),
        };
        let at = match out.iter().position(|(k, _)| *k == key) {
            Some(at) => at,
            None => {
                out.push((
                    key,
                    Listed {
                        kind: info.kind,
                        name: info.name,
                        created: false,
                        nodes: Vec::new(),
                    },
                ));
                out.len() - 1
            }
        };
        let entry = &mut out[at].1;
        entry.created |= used.created;
        if !entry.nodes.contains(&used.node_id) {
            entry.nodes.push(used.node_id.clone());
        }
    }
    out.into_iter().map(|(_, listed)| listed).collect()
}

fn title(kind: &str) -> &str {
    match kind {
        "codespace" => "Codespace",
        "sandbox" => "Sandbox",
        other => other,
    }
}

/// The lines for the report; `None` when the run used no cloud instance.
pub(super) fn text(listed: &[Listed]) -> Option<String> {
    if listed.is_empty() {
        return None;
    }
    let mut lines = vec!["Cloud instances this run used:".to_string()];
    for instance in listed {
        let name = match &instance.name {
            Some(name) => format!("{} {name}", title(&instance.kind)),
            None => format!(
                "{} (name not known: it is still being created, or its session was removed)",
                title(&instance.kind)
            ),
        };
        let origin = if instance.created {
            "created by this run"
        } else {
            "reused, not created by this run"
        };
        let steps = if instance.nodes.len() == 1 {
            "step"
        } else {
            "steps"
        };
        lines.push(format!(
            "- {name}: {origin}; {steps} {}",
            instance.nodes.join(", ")
        ));
    }
    lines.push(
        "The run left these instances as they are (it never stops or deletes one). Stop or delete them from Cloud workspaces."
            .into(),
    );
    Some(lines.join("\n"))
}

/// Everything the run used: the sessions of its cloud steps, looked up in the host (the record
/// of the run names an instance when its session is gone), plus the instances the record kept
/// as they started, which cover a session the nodes no longer point at.
pub(super) fn all(
    run: &WorkflowRun,
    lookup: impl Fn(&str) -> Option<CloudInstanceInfo>,
) -> Vec<Listed> {
    let kept = &run.cloud_instances;
    let resolve = |session: &str| {
        // An answer without a name (the instance is not named yet) says nothing the record
        // does not.
        lookup(session)
            .filter(|info| info.name.is_some())
            .or_else(|| {
                kept.iter()
                    .find(|i| i.sessions.iter().any(|s| s == session))
                    .map(|i| CloudInstanceInfo {
                        kind: i.kind.clone(),
                        name: Some(i.name.clone()),
                    })
            })
    };
    let mut out = listed(&cloud_uses(run), resolve);
    for instance in kept {
        let found = out
            .iter_mut()
            .find(|l| l.kind == instance.kind && l.name.as_deref() == Some(instance.name.as_str()));
        match found {
            Some(listing) => {
                listing.created |= instance.created;
                for node in &instance.nodes {
                    if !listing.nodes.contains(node) {
                        listing.nodes.push(node.clone());
                    }
                }
            }
            None => out.push(Listed {
                kind: instance.kind.clone(),
                name: Some(instance.name.clone()),
                created: instance.created,
                nodes: instance.nodes.clone(),
            }),
        }
    }
    out
}

/// The report's section on cloud instances, `None` when the run used none.
pub(super) fn report_section(
    run: &WorkflowRun,
    lookup: impl Fn(&str) -> Option<CloudInstanceInfo>,
) -> Option<String> {
    text(&all(run, lookup))
}
