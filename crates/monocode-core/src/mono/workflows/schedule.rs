//! When a workflow is due. Pure: the clock and the "is a run going" answer are passed in, so the
//! scheduler tick decides what to start and what only has to move on.
use super::model::{NodeKind, Workflow};
use crate::mono::habits::{Schedule, GRACE_MS};

/// The schedule a workflow runs on, when its trigger is a schedule.
fn schedule_of(workflow: &Workflow) -> Option<&Schedule> {
    match &workflow.trigger()?.kind {
        NodeKind::Schedule { schedule } => Some(schedule),
        _ => None,
    }
}

/// What the scheduler tick found: workflows to start, and ones whose schedule only moves on.
#[derive(Debug, PartialEq)]
pub struct Due<'a> {
    pub run: Vec<&'a Workflow>,
    pub advance: Vec<&'a Workflow>,
}

/// Workflows to run now, and ones whose schedule only has to move on.
/// Like habits, one that came due long ago (the app was closed) is not run, and one that
/// comes due while its previous run is going is skipped: it runs again at its next time.
/// A requested run waits for the current one to end instead of being dropped.
pub fn due_workflows<'a>(
    workflows: &'a [Workflow],
    now: u64,
    is_running: impl Fn(&str) -> bool,
) -> Due<'a> {
    let mut due = Due {
        run: Vec::new(),
        advance: Vec::new(),
    };
    for workflow in workflows {
        // Made from a file: nothing starts it, on a schedule or on request, until the user has
        // approved this exact version.
        if workflow.needs_review {
            continue;
        }
        let busy = is_running(&workflow.id);
        if workflow.run_requested {
            if !busy {
                due.run.push(workflow);
            }
            continue;
        }
        if !workflow.enabled || schedule_of(workflow).is_none() {
            continue;
        }
        // Enabled with no next time yet: pick one, do not run.
        let Some(next) = workflow.next_run_at else {
            due.advance.push(workflow);
            continue;
        };
        if next > now {
            continue;
        }
        if !busy && now - next <= GRACE_MS {
            due.run.push(workflow);
        } else {
            due.advance.push(workflow);
        }
    }
    due
}

/// The workflow with its schedule moved past `now` and its run request answered. A schedule that
/// cannot produce a next time (it fails its own validation) leaves no time, so the tick only
/// keeps trying to pick one and never fires on a stale one.
pub fn after_claim(workflow: &Workflow, now: u64, run_id: Option<&str>) -> Workflow {
    let run_id = run_id.filter(|id| !id.is_empty());
    let mut next = workflow.clone();
    next.run_requested = false;
    // The values were for the run that is being claimed now; they never carry to a later one.
    next.run_values = None;
    // A stop asked for before a run started was meant for an earlier one. Moving the schedule
    // past a workflow whose run is still going must keep a stop written for that run.
    next.stop_requested = workflow.stop_requested && run_id.is_none();
    let keep = workflow.run_requested && workflow.next_run_at.is_some_and(|at| at > now);
    if let Some(schedule) = schedule_of(workflow) {
        if !keep {
            next.next_run_at = schedule.next(now).ok();
        }
    }
    if let Some(id) = run_id {
        next.last_run_id = Some(id.to_string());
    }
    next
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mono::habits::GRACE_MS;
    use crate::mono::workflows::fixtures::workflow_from;
    use chrono::{Local, TimeZone};

    fn now() -> u64 {
        Local
            .with_ymd_and_hms(2026, 10, 22, 12, 0, 0)
            .unwrap()
            .timestamp_millis() as u64
    }

    fn workflow_with(trigger: &str, extra: &str) -> Workflow {
        let mut w = workflow_from(&format!(
            r#"{{"id":"wf-abcd1234","name":"Ship it","inputs":{{}},"inputValues":{{}},
                "nodes":[{trigger}],"edges":[],"enabled":true,"createdAt":0{extra}}}"#
        ));
        w.next_run_at = Some(now() - 1_000);
        w
    }

    const DAILY: &str = r#"{"id":"trigger","type":"schedule","schedule":{"scheduleKind":"daily","time":"09:00","minute":0,"dayOfWeek":1}}"#;

    fn workflow() -> Workflow {
        workflow_with(DAILY, "")
    }

    fn never(_: &str) -> bool {
        false
    }

    mod after_claim_tests {
        use super::*;

        #[test]
        fn a_claim_clears_the_run_request_and_any_stop_asked_for_before_it() {
            let mut w = workflow();
            w.run_requested = true;
            w.stop_requested = true;
            let claimed = after_claim(&w, now(), Some("run-1"));
            assert!(!claimed.run_requested);
            assert!(!claimed.stop_requested);
            assert_eq!(claimed.last_run_id.as_deref(), Some("run-1"));
            assert!(claimed.next_run_at.unwrap() > now());
        }

        #[test]
        fn a_claim_clears_the_run_values_asked_for_that_run() {
            let mut w = workflow();
            w.run_requested = true;
            let mut values = crate::mono::workflows::ordered::OrderedMap::new();
            values.insert(
                "n".to_string(),
                crate::mono::workflows::model::InputValue::Number(1.0),
            );
            w.run_values = Some(values);
            let claimed = after_claim(&w, now(), Some("run-1"));
            assert!(claimed.run_values.is_none());
            assert!(after_claim(&w, now(), None).run_values.is_none());
        }

        #[test]
        fn moving_the_schedule_on_without_a_claim_keeps_a_stop_written_for_the_run_that_is_going() {
            let mut w = workflow();
            w.stop_requested = true;
            let advanced = after_claim(&w, now(), None);
            assert!(advanced.stop_requested);
            assert!(advanced.next_run_at.unwrap() > now());
        }

        #[test]
        fn a_request_made_for_a_later_time_keeps_that_time() {
            let mut w = workflow();
            w.run_requested = true;
            w.next_run_at = Some(now() + 5_000);
            let claimed = after_claim(&w, now(), Some("run-2"));
            assert_eq!(claimed.next_run_at, Some(now() + 5_000));
            assert!(!claimed.run_requested);
        }

        #[test]
        fn a_manual_workflow_has_no_schedule_to_move() {
            let mut w = workflow_with(r#"{"id":"trigger","type":"manual"}"#, "");
            w.run_requested = true;
            w.next_run_at = None;
            let claimed = after_claim(&w, now(), Some("run-3"));
            assert_eq!(claimed.next_run_at, None);
            assert_eq!(claimed.last_run_id.as_deref(), Some("run-3"));
        }

        #[test]
        fn an_empty_run_id_is_no_claim() {
            let mut w = workflow();
            w.stop_requested = true;
            w.last_run_id = Some("earlier".into());
            let moved = after_claim(&w, now(), Some(""));
            assert!(moved.stop_requested);
            assert_eq!(moved.last_run_id.as_deref(), Some("earlier"));
        }

        #[test]
        fn an_interval_schedule_moves_to_its_next_slot() {
            let w = workflow_with(
                r#"{"id":"trigger","type":"schedule","schedule":{"scheduleKind":"interval","time":"00:00","minute":0,"dayOfWeek":1,"intervalHours":6,"intervalFrom":"2026-10-01"}}"#,
                "",
            );
            let moved = after_claim(&w, now(), Some("r")).next_run_at.unwrap();
            let schedule = match &w.trigger().unwrap().kind {
                NodeKind::Schedule { schedule } => schedule.clone(),
                _ => unreachable!(),
            };
            assert_eq!(moved, schedule.next(now()).unwrap());
            assert!(moved > now() && moved - now() <= 6 * 60 * 60 * 1000);
        }

        #[test]
        fn a_schedule_that_cannot_be_computed_leaves_no_time_to_fire_on() {
            let w = workflow_with(
                r#"{"id":"trigger","type":"schedule","schedule":{"scheduleKind":"daily","time":"99:99","minute":0,"dayOfWeek":1}}"#,
                "",
            );
            assert_eq!(after_claim(&w, now(), Some("r")).next_run_at, None);
        }
    }

    mod due_workflows_tests {
        use super::*;

        #[test]
        fn skips_and_advances_a_due_workflow_whose_run_is_still_going() {
            let list = [workflow()];
            let due = due_workflows(&list, now(), |_| true);
            assert!(due.run.is_empty());
            assert_eq!(due.advance.len(), 1);
        }

        #[test]
        fn runs_one_that_came_due_within_the_grace_and_advances_a_stale_one() {
            let mut fresh = workflow();
            fresh.next_run_at = Some(now() - GRACE_MS);
            let mut stale = workflow();
            stale.id = "wf-stale000".into();
            stale.next_run_at = Some(now() - GRACE_MS - 1);
            let list = [fresh, stale];
            let due = due_workflows(&list, now(), never);
            assert_eq!(due.run.len(), 1);
            assert_eq!(due.run[0].id, "wf-abcd1234");
            assert_eq!(due.advance.len(), 1);
            assert_eq!(due.advance[0].id, "wf-stale000");
        }

        #[test]
        fn leaves_one_that_is_not_due_yet_or_not_enabled() {
            let mut later = workflow();
            later.next_run_at = Some(now() + 1);
            let mut off = workflow();
            off.enabled = false;
            let manual = workflow_with(r#"{"id":"trigger","type":"manual"}"#, "");
            let list = [later, off, manual];
            let due = due_workflows(&list, now(), never);
            assert!(due.run.is_empty() && due.advance.is_empty());
        }

        #[test]
        fn picks_a_first_time_for_one_enabled_without_a_next_run() {
            let mut w = workflow();
            w.next_run_at = None;
            let list = [w];
            let due = due_workflows(&list, now(), never);
            assert!(due.run.is_empty());
            assert_eq!(due.advance.len(), 1);
        }

        #[test]
        fn runs_a_requested_workflow_when_idle_and_waits_while_busy() {
            let mut w = workflow();
            w.run_requested = true;
            w.enabled = false;
            w.next_run_at = None;
            let list = [w];
            assert_eq!(due_workflows(&list, now(), never).run.len(), 1);
            let busy = due_workflows(&list, now(), |_| true);
            assert!(busy.run.is_empty() && busy.advance.is_empty());
        }

        #[test]
        fn asks_about_the_workflow_by_id() {
            let list = [workflow()];
            let due = due_workflows(&list, now(), |id| id == "wf-other000");
            assert_eq!(due.run.len(), 1);
        }
    }

    mod a_workflow_that_needs_review {
        use super::*;

        #[test]
        fn is_not_claimed_when_due() {
            let mut due = workflow();
            due.needs_review = true;
            let list = [due];
            let result = due_workflows(&list, now(), never);
            assert!(result.run.is_empty() && result.advance.is_empty());
        }

        #[test]
        fn is_not_claimed_when_a_run_was_requested() {
            let mut asked = workflow();
            asked.needs_review = true;
            asked.enabled = false;
            asked.run_requested = true;
            let list = [asked];
            let result = due_workflows(&list, now(), never);
            assert!(result.run.is_empty() && result.advance.is_empty());
        }

        #[test]
        fn runs_once_the_review_is_done() {
            let mut reviewed = workflow();
            reviewed.needs_review = false;
            let list = [reviewed];
            assert_eq!(due_workflows(&list, now(), never).run.len(), 1);
        }
    }
}
