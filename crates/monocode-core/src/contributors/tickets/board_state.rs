//! Pure rules of the Ticket Contributors view state: the period and window a
//! pick stands for, which fetches to start and what a job event means. Port of
//! the pure helpers of `ticket-contributors/hooks/useTicketBoard.ts`.

use std::collections::HashSet;

use chrono::{DateTime, Local};

use super::board::ALL_START;
use super::tickets::WorkspaceScope;
use crate::contributors::model::period::{
    current_period, parse_iso, resolve_pick, Period, PeriodKind, PeriodPick,
};
use crate::contributors::model::sync_jobs::iso_seconds;
use crate::contributors::ticket_jobs::TicketJobEvent;

/// The period a pick stands for; All time starts at `ALL_START`, not at a first commit.
pub fn period_of(pick: &PeriodPick, now: &DateTime<Local>) -> Period {
    if pick.kind == PeriodKind::All {
        return Period {
            start: parse_iso(ALL_START).expect("ALL_START is a date"),
            ..current_period(PeriodKind::All, now, None)
        };
    }
    resolve_pick(pick, now, None)
}

/// The `from`..`to` strings the ticket commands read for a period.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Window {
    pub from: String,
    pub to: String,
}

pub fn window_of(period: &Period) -> Window {
    Window {
        from: if period.kind == PeriodKind::All {
            ALL_START.to_string()
        } else {
            iso_seconds(&period.start)
        },
        to: iso_seconds(&period.end),
    }
}

pub fn fetch_key(workspace_id: &str, from: &str, to: &str) -> String {
    format!("{workspace_id}\u{1}{from}\u{1}{to}")
}

/// A fetch job is queued or running.
pub fn fetch_busy(job: Option<&TicketJobEvent>) -> bool {
    job.is_some_and(|j| j.kind == "fetch" && (j.state == "queued" || j.state == "running"))
}

/// A window is fetched once per session, and never while another fetch is queued or running.
pub fn should_fetch(key: &str, fetched: &HashSet<String>, job: Option<&TicketJobEvent>) -> bool {
    !fetched.contains(key) && !fetch_busy(job)
}

/// A job event that means the store may have changed (a failed fetch of one
/// workspace still stored the others).
pub fn store_may_have_changed(event: &TicketJobEvent) -> bool {
    matches!(event.state.as_str(), "done" | "cancelled" | "error")
}

/// A job event that ends a job: done, failed or dropped.
pub fn is_end(event: &TicketJobEvent) -> bool {
    event.state != "queued" && event.state != "running"
}

/// The job error after `event`: a failed job sets it, and a new job or a
/// finished one clears it. A cancelled job leaves it, since it says nothing
/// about the failure that came before.
pub fn job_error_after(current: Option<&str>, event: &TicketJobEvent) -> Option<String> {
    match event.state.as_str() {
        "error" => Some(
            event
                .error
                .clone()
                .filter(|e| !e.is_empty())
                .unwrap_or_else(|| "The ticket job failed.".into()),
        ),
        "cancelled" => current.map(String::from),
        _ => None,
    }
}

/// The workspace plus its chosen spaces: what a fetch actually covers.
pub fn scope_fetch_id(scope: &WorkspaceScope) -> String {
    match &scope.space_ids {
        None => scope.workspace_id.clone(),
        Some(spaces) => {
            let mut sorted: Vec<&str> = spaces.iter().map(String::as_str).collect();
            sorted.sort_by(|a, b| crate::contributors::model::js_cmp(a, b));
            format!("{}:{}", scope.workspace_id, sorted.join(","))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contributors::model::js_iso;
    use crate::contributors::model::period::{current_period, PeriodKind};
    use crate::contributors::model::test_fixtures::local_at;
    use crate::contributors::tickets::board::{build_board, period_tickets, to_assess_count};
    use crate::contributors::tickets::test_fixtures::{strings, ticket_with, user};
    use crate::contributors::tickets::tickets::{Level, DEFAULT_TICKET_POINTS};

    fn now() -> DateTime<Local> {
        local_at(2026, 9, 17, 12, 0)
    }

    fn event(state: &str) -> TicketJobEvent {
        event_of(state, |_| {})
    }

    fn event_of(state: &str, patch: impl FnOnce(&mut TicketJobEvent)) -> TicketJobEvent {
        let mut e = TicketJobEvent {
            kind: "fetch".into(),
            state: state.into(),
            done: 1,
            total: 2,
            failed: 1,
            error: None,
            job_id: 1,
            pending_fetches: 0,
        };
        patch(&mut e);
        e
    }

    fn scope() -> WorkspaceScope {
        WorkspaceScope {
            workspace_id: "w1".into(),
            name: "Workspace".into(),
            space_ids: None,
            excluded_statuses: vec![],
            delivered_statuses: vec![],
            excluded_members: vec![],
        }
    }

    fn pick(kind: PeriodKind, id: Option<&str>) -> PeriodPick {
        PeriodPick {
            kind,
            id: id.map(String::from),
        }
    }

    fn no_seconds(t: &DateTime<Local>) -> String {
        js_iso(t).replace(".000Z", "Z")
    }

    // windowOf

    #[test]
    fn window_of_sends_the_selected_period_as_iso_seconds() {
        let period = current_period(PeriodKind::Month, &now(), None);
        assert_eq!(
            window_of(&period),
            Window {
                from: no_seconds(&period.start),
                to: no_seconds(&period.end),
            }
        );
    }

    #[test]
    fn window_of_starts_all_time_at_all_start_not_at_a_first_commit() {
        let period = period_of(&pick(PeriodKind::All, None), &now());
        assert_eq!(period.kind, PeriodKind::All);
        assert_eq!(window_of(&period).from, ALL_START);
        assert_eq!(js_iso(&period.start), "2015-01-01T00:00:00.000Z");
        assert!(period.end > now());
    }

    #[test]
    fn window_of_builds_a_non_all_period_from_its_pick() {
        assert_eq!(
            period_of(&pick(PeriodKind::Month, Some("2026-08")), &now()).id,
            "2026-08"
        );
        assert_eq!(
            period_of(&pick(PeriodKind::Month, None), &now()).id,
            "2026-10"
        );
    }

    // fetchKey and shouldFetch

    fn set(items: &[&str]) -> HashSet<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn fetch_key_keys_a_fetch_by_workspace_and_window() {
        assert_eq!(fetch_key("w1", "a", "b"), fetch_key("w1", "a", "b"));
        assert_ne!(fetch_key("w1", "a", "b"), fetch_key("w2", "a", "b"));
        assert_ne!(fetch_key("w1", "a", "b"), fetch_key("w1", "a", "c"));
        assert_eq!(fetch_key("w1", "a", "b"), "w1\u{1}a\u{1}b");
    }

    #[test]
    fn should_fetch_fetches_a_window_that_was_not_fetched_yet() {
        assert!(should_fetch("k", &set(&[]), None));
        assert!(should_fetch("k", &set(&["other"]), Some(&event("done"))));
    }

    #[test]
    fn should_fetch_skips_a_window_that_was_fetched_in_this_session() {
        assert!(!should_fetch("k", &set(&["k"]), None));
    }

    #[test]
    fn should_fetch_skips_while_a_fetch_is_queued_or_running() {
        assert!(!should_fetch("k", &set(&[]), Some(&event("queued"))));
        assert!(!should_fetch("k", &set(&[]), Some(&event("running"))));
    }

    #[test]
    fn should_fetch_does_not_let_a_running_assessment_block_a_fetch() {
        let assessing = event_of("running", |e| e.kind = "assess".into());
        assert!(should_fetch("k", &set(&[]), Some(&assessing)));
    }

    // jobErrorAfter

    #[test]
    fn job_error_after_keeps_the_error_of_a_failed_job_so_a_reload_cannot_hide_it() {
        let failed = event_of("error", |e| e.error = Some("ClickUp said no".into()));
        assert_eq!(
            job_error_after(None, &failed).as_deref(),
            Some("ClickUp said no")
        );
        assert_eq!(
            job_error_after(None, &event("error")).as_deref(),
            Some("The ticket job failed.")
        );
    }

    #[test]
    fn job_error_after_clears_the_error_when_a_new_job_starts_or_one_finishes() {
        for state in ["queued", "running", "done"] {
            assert_eq!(job_error_after(Some("old"), &event(state)), None, "{state}");
        }
    }

    #[test]
    fn job_error_after_leaves_the_error_alone_on_cancel() {
        assert_eq!(
            job_error_after(Some("old"), &event("cancelled")).as_deref(),
            Some("old")
        );
    }

    // loaded tickets

    #[test]
    fn loaded_tickets_count_an_override_only_ticket_as_assessed() {
        let period = current_period(PeriodKind::Month, &now(), None);
        let ticket = |id: &str, level: Option<&str>| {
            ticket_with(id, |t| {
                t.status = "complete".into();
                t.updated_at = "2026-10-05T10:00:00".into();
                t.done_at = Some("2026-10-05T10:00:00".into());
                t.assignees = vec![user("u1")];
                t.override_level = level.map(String::from);
            })
        };
        let shown = period_tickets(
            &[ticket("o", Some("L")), ticket("plain", None)],
            &scope(),
            &period,
        );
        let board = build_board(&shown, &DEFAULT_TICKET_POINTS, &[]);
        assert_eq!(board.assessed, 1);
        assert_eq!(board.members[0].points, DEFAULT_TICKET_POINTS[Level::L]);
        assert_eq!(to_assess_count(shown.len(), board.assessed, 100.0), 1);
    }

    // scopeFetchId

    #[test]
    fn scope_fetch_id_ignores_the_status_choices_so_changing_them_never_refetches() {
        let id = scope_fetch_id(&scope());
        let mut changed = scope();
        changed.excluded_statuses = strings(&["dropped"]);
        changed.delivered_statuses = strings(&["ready for testing"]);
        assert_eq!(scope_fetch_id(&changed), id);
        let mut spaced = scope();
        spaced.space_ids = Some(strings(&["s1"]));
        assert_ne!(scope_fetch_id(&spaced), id);
    }

    // not in the TS suite

    #[test]
    fn scope_fetch_id_is_the_workspace_alone_or_with_its_sorted_spaces() {
        assert_eq!(scope_fetch_id(&scope()), "w1");
        let mut spaced = scope();
        spaced.space_ids = Some(strings(&["s2", "s10", "s1"]));
        assert_eq!(scope_fetch_id(&spaced), "w1:s1,s10,s2");
        spaced.space_ids = Some(vec![]);
        assert_eq!(scope_fetch_id(&spaced), "w1:");
    }

    #[test]
    fn job_end_helpers_follow_the_hook() {
        assert!(!is_end(&event("queued")));
        assert!(!is_end(&event("running")));
        for state in ["done", "error", "cancelled"] {
            assert!(is_end(&event(state)), "{state}");
            assert!(store_may_have_changed(&event(state)), "{state}");
        }
        assert!(!store_may_have_changed(&event("running")));
        assert!(fetch_busy(Some(&event("queued"))));
        assert!(!fetch_busy(Some(&event("done"))));
        assert!(!fetch_busy(None));
    }
}
