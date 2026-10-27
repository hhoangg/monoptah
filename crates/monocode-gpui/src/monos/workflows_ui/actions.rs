//! What the buttons of the Workflows tab and pages do: port of `workflowActions` in
//! `MonoWorkflows.tsx` and the input saving of `WorkflowPage.tsx`. Pure over a `WorkflowHost`
//! (the Mono's files, or a test host), so the rules are the CLI's: the same refusals, the same
//! review mark, the same issue check. They block (files, CLI probes): call them off the UI thread.
use monocode_core::mono::workflows::{
    cli::WorkflowHost,
    model::{InputValue, NodeKind, Workflow, WorkflowInput, definition_fingerprint, trigger_of},
    models::workflow_issues,
    ordered::OrderedMap,
    validate::WorkflowIssue,
    yaml::input_value_problems,
};

pub(super) const REVIEW_FIRST: &str = "Review this workflow before running or enabling it.";
pub(super) const CHANGED: &str =
    "This workflow changed while you were reviewing it. Review it again.";
pub(super) const RESUME_REVIEW: &str = "Review this workflow before resuming its run.";
pub(super) const RESUME_BUSY: &str =
    "Another run of this workflow is going. Resume this one when it ends.";
const STOP_FIRST: &str = "Stop the workflow before removing it.";
const NOT_CHANGED: &str = "Could not change the workflow.";

/// What approving a reviewed workflow goes on to do: enable it, or run it once.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ReviewIntent {
    Enable,
    Run,
}

/// How approving a reviewed workflow ended.
#[derive(Debug, PartialEq)]
pub(super) enum Approval {
    Done,
    /// The stored definition is not the one that was reviewed; here is the current one.
    Changed(Box<Workflow>),
    Refused(String),
}

/// Why a workflow cannot be run or resumed yet, in the words shown under the list.
pub(super) fn refusal(verb: &str, issues: &[WorkflowIssue]) -> String {
    let Some((first, rest)) = issues.split_first() else {
        return format!("Can't {verb} yet.");
    };
    let reason = first
        .message
        .trim_end_matches(|c: char| c == '.' || c.is_whitespace());
    let more = if rest.is_empty() {
        String::new()
    } else {
        format!(" (and {} more)", rest.len())
    };
    format!("Can't {verb} yet: {reason}{more}.")
}

/// Everything wrong with the workflow's graph and the models its sessions name.
pub(super) fn issues_of(host: &mut impl WorkflowHost, w: &Workflow) -> Vec<WorkflowIssue> {
    let catalog = host.catalog(w);
    workflow_issues(w, &catalog)
}

/// The workflow switched on, its next scheduled run counted from `now`.
fn enabled_now(mut w: Workflow, now: u64) -> Result<Workflow, String> {
    w.enabled = true;
    let next = match trigger_of(&w).map(|n| &n.kind) {
        Some(NodeKind::Schedule { schedule }) => Some(schedule.next(now)?),
        _ => None,
    };
    if next.is_some() {
        w.next_run_at = next;
    }
    Ok(w)
}

/// Edits the workflow `id` in place over what is stored. `edit` returns the workflow to keep
/// (None removes it) and a notice. The notice, or why the save failed, comes back.
fn change<H: WorkflowHost>(
    host: &mut H,
    id: &str,
    mut edit: impl FnMut(&mut H, Workflow) -> Result<(Option<Workflow>, Option<String>), String>,
) -> Option<String> {
    let saved = host.update(|host, list| {
        let mut notice = None;
        let mut kept = Vec::with_capacity(list.len());
        for w in list {
            if w.id != id {
                kept.push(w);
                continue;
            }
            let (next, said) = edit(host, w)?;
            notice = said;
            kept.extend(next);
        }
        Ok((kept, notice))
    });
    saved.unwrap_or_else(|error| {
        eprintln!("Could not change the workflow: {error}");
        Some(NOT_CHANGED.into())
    })
}

/// Asks for a run. Resolves to why the workflow cannot run yet, if it cannot. Never around the
/// review, whichever screen asked.
pub(super) fn run_now(host: &mut impl WorkflowHost, id: &str) -> Option<String> {
    let notice = change(host, id, |host, w| {
        if w.needs_review {
            return Ok((Some(w), Some(REVIEW_FIRST.into())));
        }
        let issues = issues_of(host, &w);
        if !issues.is_empty() {
            let notice = refusal("run", &issues);
            return Ok((Some(w), Some(notice)));
        }
        let mut next = w;
        next.run_requested = true;
        Ok((Some(next), None))
    });
    if notice.is_none() {
        host.request_check_now();
    }
    notice
}

/// Asks for an interrupted run to continue from where it stopped. Resolves to why it cannot yet.
/// Two runs of one workflow would race over its files, so the request waits for the other to end.
pub(super) fn resume_run(host: &mut impl WorkflowHost, id: &str, run: &str) -> Option<String> {
    let notice = change(host, id, |host, w| {
        if w.needs_review {
            return Ok((Some(w), Some(RESUME_REVIEW.into())));
        }
        if host.is_running(&w.id) {
            return Ok((Some(w), Some(RESUME_BUSY.into())));
        }
        let mut next = w;
        next.resume_run_id = Some(run.to_string());
        Ok((Some(next), None))
    });
    if notice.is_none() {
        host.request_check_now();
    }
    notice
}

/// Pauses an enabled workflow, or resumes a paused one. Resolves to why a paused workflow
/// cannot be resumed yet, if it cannot.
pub(super) fn toggle(host: &mut impl WorkflowHost, id: &str) -> Option<String> {
    change(host, id, |host, w| {
        if w.enabled {
            let mut next = w;
            next.enabled = false;
            return Ok((Some(next), None));
        }
        if w.needs_review {
            return Ok((Some(w), Some(REVIEW_FIRST.into())));
        }
        let issues = issues_of(host, &w);
        if !issues.is_empty() {
            let notice = refusal("resume", &issues);
            return Ok((Some(w), Some(notice)));
        }
        let now = host.now();
        match enabled_now(w.clone(), now) {
            Ok(next) => Ok((Some(next), None)),
            Err(error) => Ok((Some(w), Some(error))),
        }
    })
}

/// The user approving a workflow that needs review: clears the mark and does what they asked,
/// but only if the stored definition still has the `fingerprint` of the one they read. The only
/// way the mark is cleared.
pub(super) fn approve(
    host: &mut impl WorkflowHost,
    id: &str,
    intent: ReviewIntent,
    fingerprint: &str,
) -> Approval {
    let saved = host.update(|host, list| {
        let mut result = Approval::Refused("That workflow is gone.".into());
        let mut kept = Vec::with_capacity(list.len());
        for w in list {
            if w.id != id {
                kept.push(w);
                continue;
            }
            // The user read one definition; a different one (the Mono edited it meanwhile) is
            // not approved.
            if definition_fingerprint(&w) != fingerprint {
                result = Approval::Changed(Box::new(w.clone()));
                kept.push(w);
                continue;
            }
            let issues = issues_of(host, &w);
            if !issues.is_empty() {
                let verb = if intent == ReviewIntent::Run {
                    "run"
                } else {
                    "resume"
                };
                result = Approval::Refused(refusal(verb, &issues));
                kept.push(w);
                continue;
            }
            let mut approved = w.clone();
            approved.needs_review = false;
            let approved = match intent {
                ReviewIntent::Run => {
                    approved.run_requested = true;
                    Ok(approved)
                }
                ReviewIntent::Enable => enabled_now(approved, host.now()),
            };
            match approved {
                Ok(next) => {
                    result = Approval::Done;
                    kept.push(next);
                }
                Err(error) => {
                    result = Approval::Refused(error);
                    kept.push(w);
                }
            }
        }
        Ok((kept, result))
    });
    let result = saved.unwrap_or_else(|error| {
        eprintln!("Could not approve the workflow: {error}");
        Approval::Refused(NOT_CHANGED.into())
    });
    if result == Approval::Done && intent == ReviewIntent::Run {
        host.request_check_now();
    }
    result
}

/// Takes the workflow and its run history away. Resolves to why it cannot be removed, if it
/// cannot: a run left going would keep posting its report with nothing to stop it.
pub(super) fn remove(host: &mut impl WorkflowHost, id: &str) -> Option<String> {
    if host.is_running(id) {
        return Some(STOP_FIRST.into());
    }
    let notice = change(host, id, |_, _| Ok((None, None)));
    if notice.is_none() {
        host.clear_runs(id);
    }
    notice
}

/// What is wrong with the value an input would take, if anything: no value at all, or one that
/// does not fit the declared type.
pub(super) fn input_problem(
    name: &str,
    input: &WorkflowInput,
    value: Option<&InputValue>,
) -> Option<String> {
    let Some(value) = value else {
        return Some(format!("Input \"{name}\" needs a value"));
    };
    let mut inputs = OrderedMap::new();
    inputs.insert(name.to_string(), input.clone());
    let mut values = OrderedMap::new();
    values.insert(name.to_string(), value.clone());
    input_value_problems(&inputs, &values).into_iter().next()
}

/// Keeps `value` for the input `name` of the workflow `id`. The error is the real reason it
/// could not be saved.
pub(super) fn save_input(
    host: &mut impl WorkflowHost,
    id: &str,
    name: &str,
    value: &InputValue,
) -> Result<(), String> {
    host.update(|_, list| {
        let list = list
            .into_iter()
            .map(|mut w| {
                if w.id == id {
                    w.input_values.insert(name.to_string(), value.clone());
                }
                w
            })
            .collect();
        Ok((list, ()))
    })
}

/// `Number(text)` for what a user typed into a number input: NaN when it is not a number.
pub(super) fn js_number(typed: &str) -> f64 {
    let text = typed.trim();
    let (sign, digits) = match text.as_bytes().first() {
        Some(b'-') => (-1.0, &text[1..]),
        Some(b'+') => (1.0, &text[1..]),
        _ => (1.0, text),
    };
    let radix = |prefix: &[&str], radix: u32| {
        prefix
            .iter()
            .find_map(|p| digits.strip_prefix(p))
            .filter(|_| text.len() == digits.len())
            .and_then(|rest| u64::from_str_radix(rest, radix).ok())
            .map(|n| n as f64)
    };
    if let Some(n) = radix(&["0x", "0X"], 16)
        .or_else(|| radix(&["0o", "0O"], 8))
        .or_else(|| radix(&["0b", "0B"], 2))
    {
        return n;
    }
    if digits == "Infinity" {
        return sign * f64::INFINITY;
    }
    // Rust also reads "inf", "nan" and "infinity", which JS does not, and a second sign.
    if digits.is_empty()
        || digits.starts_with(['+', '-'])
        || !digits
            .bytes()
            .all(|b| b.is_ascii_digit() || matches!(b, b'.' | b'e' | b'E' | b'+' | b'-'))
    {
        return f64::NAN;
    }
    digits.parse::<f64>().map_or(f64::NAN, |n| sign * n)
}

/// A file name for the workflow: lowercase words joined by hyphens.
pub(super) fn workflow_file_name(name: &str) -> String {
    let mut slug = String::new();
    for c in name.to_lowercase().chars() {
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            slug.push(c);
        } else if !slug.ends_with('-') {
            slug.push('-');
        }
    }
    let slug: String = slug.trim_matches('-').chars().take(60).collect();
    format!(
        "{}.monoptah-workflow.yaml",
        if slug.is_empty() { "workflow" } else { &slug }
    )
}
