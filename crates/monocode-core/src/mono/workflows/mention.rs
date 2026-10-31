//! `@workflow/<slug>` in a Mono composer: names a workflow so the message carries its facts.
//! A mention attaches context and never runs anything.
use super::model::WorkflowId;
use super::model::{trigger_of, InputType, InputValue, NodeKind, Requirement, RunStatus, Workflow};
use super::ordered::Json;
use super::preflight::Preflight;
use std::collections::HashMap;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkflowSlug {
    pub slug: String,
    pub id: WorkflowId,
    pub name: String,
}

/// What the attached context says about a workflow's requirements.
#[derive(Clone, Debug)]
pub enum RequiresView {
    /// The workflow declares none.
    None,
    /// It declares some, but nobody has checked this machine yet.
    NotChecked(Vec<Requirement>),
    Checked(Preflight),
}

#[derive(Clone, Debug)]
pub struct MentionFacts {
    /// Status and start time of the newest run.
    pub last_run: Option<(RunStatus, u64)>,
    /// The run history could not be read. `lastRun` then says `{"status":"unknown"}` rather than
    /// null, which would claim the workflow never ran.
    pub last_run_unreadable: bool,
    pub requires: RequiresView,
}

const INSTRUCTION: &str = "Referenced workflow (context only: do not run it unless the user's message asks for that; to run it use workflows.run with this id):";

/// Lowercase ASCII letters and digits, every other run becoming one `-`, trimmed of `-`.
fn sanitize(text: &str) -> String {
    let mut out = String::new();
    for c in text.chars().flat_map(char::to_lowercase) {
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            out.push(c);
        } else if !out.is_empty() && !out.ends_with('-') {
            out.push('-');
        }
    }
    out.trim_end_matches('-').to_owned()
}

/// One slug per workflow. Workflows sharing a base slug all take `<base>-<id tail>`, so none keeps
/// the bare slug and deleting a peer cannot hand it to another. A slug that still equals another
/// uses the whole id. The result does not depend on the order of `list`.
pub fn workflow_slugs(list: &[Workflow]) -> Vec<WorkflowSlug> {
    let base = |w: &Workflow| {
        let slug = sanitize(&w.name);
        if slug.is_empty() {
            "workflow".to_owned()
        } else {
            slug
        }
    };
    let joined = |base: &str, id: &str| {
        let id = sanitize(id);
        if id.is_empty() {
            base.to_owned()
        } else {
            format!("{base}-{id}")
        }
    };
    let mut groups = HashMap::<String, usize>::new();
    for w in list {
        *groups.entry(base(w)).or_default() += 1;
    }
    let mut slugs: Vec<String> = list
        .iter()
        .map(|w| {
            let base = base(w);
            if groups[&base] > 1 {
                let id = w.id.strip_prefix("wf-").unwrap_or(&w.id);
                joined(&base, id)
            } else {
                base
            }
        })
        .collect();
    let mut counts = HashMap::<String, usize>::new();
    for slug in &slugs {
        *counts.entry(slug.clone()).or_default() += 1;
    }
    for (slug, w) in slugs.iter_mut().zip(list) {
        if counts[slug.as_str()] > 1 {
            *slug = joined(&base(w), &w.id);
        }
    }
    list.iter()
        .zip(slugs)
        .map(|(w, slug)| WorkflowSlug {
            slug,
            id: w.id.clone(),
            name: w.name.clone(),
        })
        .collect()
}

/// The slugs of `@workflow/<slug>` mentions in what the user typed, once each in order. A mention
/// starts a word and ends the text, whitespace or one of `)]}.!?;,`, so `@workflow/a/b` is not `a`.
pub fn mentioned_slugs(text: &str) -> Vec<String> {
    const TAG: &str = "@workflow/";
    let mut out: Vec<String> = Vec::new();
    for (at, _) in text.match_indices(TAG) {
        if !text[..at]
            .chars()
            .next_back()
            .is_none_or(char::is_whitespace)
        {
            continue;
        }
        // A quoted line is not the user's own words, like `token_at` and the MCP filter treat it.
        let line_start = text[..at].rfind('\n').map_or(0, |i| i + 1);
        if text[line_start..at].trim_start().starts_with('>') {
            continue;
        }
        let rest = &text[at + TAG.len()..];
        let len = rest
            .find(|c: char| !(c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'))
            .unwrap_or(rest.len());
        let ends = rest[len..]
            .chars()
            .next()
            .is_none_or(|c| c.is_whitespace() || ")]}.!?;,".contains(c));
        if len > 0 && ends && !out.iter().any(|s| s == &rest[..len]) {
            out.push(rest[..len].to_owned());
        }
    }
    out
}

fn st(text: impl Into<String>) -> Json {
    Json::String(text.into())
}

fn obj(pairs: Vec<(&str, Json)>) -> Json {
    Json::Object(pairs.into_iter().collect())
}

fn input_type(ty: InputType) -> &'static str {
    InputType::ALL
        .iter()
        .find(|(_, t)| *t == ty)
        .map_or("string", |(name, _)| name)
}

fn requirement_json(r: &Requirement, status: &str, detail: Option<&str>) -> Json {
    let mut pairs = vec![("kind", st(r.kind.key())), ("name", st(&r.name))];
    if let Some(hint) = &r.hint {
        pairs.push(("hint", st(hint)));
    }
    pairs.push(("status", st(status)));
    if let Some(detail) = detail {
        pairs.push(("detail", st(detail)));
    }
    obj(pairs)
}

fn requires_json(view: &RequiresView) -> Json {
    Json::Array(match view {
        RequiresView::None => Vec::new(),
        RequiresView::NotChecked(list) => list
            .iter()
            .map(|r| requirement_json(r, "not checked", None))
            .collect(),
        RequiresView::Checked(pre) => pre
            .statuses
            .iter()
            .map(|s| {
                if s.met {
                    requirement_json(&s.requirement, "met", None)
                } else {
                    requirement_json(&s.requirement, "missing", s.detail.as_deref())
                }
            })
            .collect(),
    })
}

fn workflow_json(w: &Workflow, facts: &MentionFacts) -> Json {
    let trigger = match trigger_of(w).map(|n| &n.kind) {
        None => obj(vec![("type", st("none"))]),
        Some(NodeKind::Schedule { schedule }) => obj(vec![
            ("type", st("schedule")),
            ("schedule", st(schedule.label())),
        ]),
        Some(_) => obj(vec![("type", st("manual"))]),
    };
    let inputs = w
        .inputs
        .iter()
        .map(|(name, input)| {
            let mut pairs = vec![("name", st(name)), ("type", st(input_type(input.ty)))];
            if let Some(description) = &input.description {
                pairs.push(("description", st(description)));
            }
            pairs.push((
                "value",
                match w.input_values.get(name) {
                    Some(InputValue::String(v)) => st(v),
                    Some(InputValue::Number(v)) => Json::Number(*v),
                    Some(InputValue::Bool(v)) => Json::Bool(*v),
                    None => Json::Null,
                },
            ));
            obj(pairs)
        })
        .collect();
    let unreadable = facts.last_run_unreadable;
    let none = if unreadable {
        obj(vec![("status", st("unknown"))])
    } else {
        Json::Null
    };
    let last_run = facts.last_run.map_or(none, |(status, started_at)| {
        let status = serde_json::to_value(status)
            .ok()
            .and_then(|v| v.as_str().map(str::to_owned))
            .unwrap_or_default();
        obj(vec![
            ("status", st(status)),
            ("startedAt", Json::Number(started_at as f64)),
        ])
    });
    obj(vec![
        ("id", st(&w.id)),
        ("name", st(&w.name)),
        ("description", w.description.as_ref().map_or(Json::Null, st)),
        ("trigger", trigger),
        ("enabled", Json::Bool(w.enabled)),
        ("needsReview", Json::Bool(w.needs_review)),
        ("inputs", Json::Array(inputs)),
        ("requires", requires_json(&facts.requires)),
        ("lastRun", last_run),
    ])
}

/// `prompt` plus a context block for each workflow `typed` names. `typed` is what the user wrote,
/// not `prompt`: a mention inside an expanded note body is not the user's. A slug naming no
/// workflow stays plain text.
pub fn attach_workflows(
    prompt: &str,
    typed: &str,
    list: &[Workflow],
    mut facts: impl FnMut(&Workflow) -> MentionFacts,
) -> String {
    let slugs = workflow_slugs(list);
    let mut seen: Vec<&str> = Vec::new();
    let mut blocks = Vec::new();
    for slug in mentioned_slugs(typed) {
        // A slug that two workflows still share resolves to neither.
        let mut holders = slugs.iter().enumerate().filter(|(_, s)| s.slug == slug);
        let (Some((found, _)), None) = (holders.next(), holders.next()) else {
            continue;
        };
        let w = &list[found];
        if seen.contains(&w.id.as_str()) {
            continue;
        }
        seen.push(&w.id);
        blocks.push(format!(
            "{INSTRUCTION}\n{}",
            workflow_json(w, &facts(w)).stringify(false)
        ));
    }
    if blocks.is_empty() {
        return prompt.to_owned();
    }
    format!("{prompt}\n\n---\n{}", blocks.join("\n"))
}
