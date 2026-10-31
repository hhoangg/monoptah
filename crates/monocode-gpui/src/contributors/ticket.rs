//! The Ticket tab (`TicketContributorsView.tsx` and `useTicketBoard.ts`):
//! ClickUp users ranked by the points of the delivered tickets an AI assessed.
//!
//! - `state`: `TicketView`, the rules of the hook (fetch once per window per
//!   session, job events, pending fetches, reclassify without a refetch).
//! - `export`: Export report (previous-window fetch, render, save, reveal).
//! - `scope`: what the Scope panel's checkboxes show and change (pure).
//! - `draw`, `rows`: the toolbar, score card, leaderboard and member tickets.
//! - `backend`: the seam to ClickUp, the ticket store, the jobs and the prefs.
use super::ContributorsPage;
use gpui::{AnyElement, Context, IntoElement};
use monocode_core::contributors::{
    ticket_jobs::{TicketEstimate, TicketJobEvent},
    tickets::tickets::AiConfigStatus,
};

pub(super) mod backend;
mod detail;
mod draw;
mod export;
#[cfg(test)]
mod export_tests;
#[cfg(test)]
mod fake;
mod overview;
mod part;
mod rows;
mod scope;
mod state;
#[cfg(test)]
mod state_tests;
#[cfg(test)]
mod view_tests;

pub(crate) use state::{TicketEvent, TicketView};
#[cfg(test)]
pub(super) use view_tests::hover_numbers;

#[cfg(test)]
thread_local! {
    /// How many times the tab, a leaderboard row and a ticket row were built.
    pub(super) static BUILT: std::cell::Cell<[usize; 3]> = const { std::cell::Cell::new([0; 3]) };
}
/// Counts one build: 0 the tab, 1 a leaderboard row, 2 a ticket row.
#[cfg(test)]
pub(super) fn built(which: usize) {
    BUILT.with(|n| {
        let mut all = n.get();
        all[which] += 1;
        n.set(all);
    });
}

impl ContributorsPage {
    /// The Ticket tab: one view for the page's life; opening the tab mounts it.
    pub(super) fn ticket_body(&mut self, _cx: &mut Context<Self>) -> AnyElement {
        self.ticket.clone().into_any_element()
    }
}

// ---- the copy -----------------------------------------------------------------

/// Thousands separators; fractions (a share of a ticket) show one decimal at
/// most, rounded half away from zero on the shortest decimal form, like
/// `toLocaleString("en-US", { maximumFractionDigits: 1 })`.
pub(super) fn fmt(n: f64) -> String {
    if !n.is_finite() {
        return if n.is_nan() {
            "NaN".into()
        } else if n > 0. {
            "∞".into()
        } else {
            "-∞".into()
        };
    }
    let negative = n < 0.;
    let text = format!("{}", n.abs());
    let (whole, fraction) = text.split_once('.').unwrap_or((&text, ""));
    let mut digits: Vec<u8> = whole.bytes().map(|b| b - b'0').collect();
    let mut tenth = fraction.bytes().next().map_or(0, |b| b - b'0');
    if fraction.as_bytes().get(1).is_some_and(|b| *b >= b'5') {
        tenth += 1;
        if tenth == 10 {
            tenth = 0;
            let mut at = digits.len();
            loop {
                if at == 0 {
                    digits.insert(0, 1);
                    break;
                }
                at -= 1;
                if digits[at] == 9 {
                    digits[at] = 0;
                } else {
                    digits[at] += 1;
                    break;
                }
            }
        }
    }
    let mut out = String::new();
    for (i, digit) in digits.iter().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push((b'0' + digit) as char);
    }
    if tenth != 0 {
        out.push('.');
        out.push((b'0' + tenth) as char);
    }
    if negative && out != "0" {
        out.insert(0, '-');
    }
    out
}

/// "1 ticket", "4 tickets", "1,200 delivered tickets".
pub(super) fn plural(n: f64, one: &str) -> String {
    format!("{} {}{}", fmt(n), one, if n == 1. { "" } else { "s" })
}

pub(super) fn runner_label(runner: &str) -> &str {
    match runner {
        "claude-code" => "Claude Code",
        "api" => "Anthropic API",
        other => other,
    }
}

/// "in Oct 2026", or "across all time" for the All period.
pub(super) fn period_phrase(all: bool, label: &str) -> String {
    if all {
        "across all time".into()
    } else {
        format!("in {label}")
    }
}

/// The first line of the score card.
pub(super) fn delivered_line(total: usize, assessed: usize, phrase: &str) -> String {
    format!(
        "{} {phrase}. {} scored, {} not scored yet.",
        plural(total as f64, "delivered ticket"),
        fmt(assessed as f64),
        fmt(total as f64 - assessed as f64)
    )
}

/// What the next run does and costs.
pub(super) fn plan_line(
    to_assess: usize,
    ai: Option<&AiConfigStatus>,
    estimate: Option<&TicketEstimate>,
) -> String {
    if to_assess == 0 {
        return "Coverage already reached. Nothing to score.".into();
    }
    let model = match ai {
        Some(ai) => format!("{} ({})", ai.model, runner_label(&ai.runner)),
        None => "the configured model".into(),
    };
    let tokens = estimate
        .map(|e| {
            format!(
                " About {} input tokens (estimate).",
                fmt((e.input_tokens as f64).round())
            )
        })
        .unwrap_or_default();
    format!(
        "Scoring {} more {} with {model}.{tokens}",
        fmt(to_assess as f64),
        if to_assess == 1 { "ticket" } else { "tickets" }
    )
}

pub(super) fn job_label(job: &TicketJobEvent) -> &'static str {
    if job.kind == "fetch" {
        "Fetching tickets from ClickUp..."
    } else {
        "Scoring tickets..."
    }
}

/// "1/4", "3/4, 1 failed".
pub(super) fn job_counts(job: &TicketJobEvent) -> String {
    let failed = if job.failed > 0 {
        format!(", {} failed", fmt(job.failed as f64))
    } else {
        String::new()
    };
    format!("{}/{}{failed}", fmt(job.done as f64), fmt(job.total as f64))
}

/// The share of the bar that is full.
pub(super) fn job_fraction(job: &TicketJobEvent) -> f32 {
    if job.total > 0 {
        (job.done as f32 / job.total as f32).min(1.)
    } else {
        0.
    }
}

pub(super) fn failed_note(failed: u64) -> String {
    format!(
        "{} could not be scored and {} at 0 points.",
        plural(failed as f64, "ticket"),
        if failed == 1 { "stays" } else { "stay" }
    )
}

/// A job that is queued or running.
pub(super) fn live(job: Option<&TicketJobEvent>) -> bool {
    job.is_some_and(|job| job.state == "queued" || job.state == "running")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_get_thousands_separators_and_one_decimal_at_most() {
        assert_eq!(fmt(0.), "0");
        assert_eq!(fmt(8.), "8");
        assert_eq!(fmt(1200.), "1,200");
        assert_eq!(fmt(1_234_567.), "1,234,567");
        assert_eq!(fmt(12345.), "12,345");
        assert_eq!(fmt(16. / 3.), "5.3");
        assert_eq!(fmt(5.25), "5.3");
        assert_eq!(fmt(0.04), "0");
        assert_eq!(fmt(999.96), "1,000");
        assert_eq!(fmt(9.95), "10");
        assert_eq!(fmt(2.5), "2.5");
    }

    #[test]
    fn a_decimal_rounds_on_its_shortest_form_like_the_browser() {
        // 0.35 is 0.34999999999999997 in binary; the browser still says 0.4.
        assert_eq!(fmt(0.35), "0.4");
        assert_eq!(fmt(1.05), "1.1");
    }

    #[test]
    fn plural_picks_the_word_by_the_count() {
        assert_eq!(plural(1., "ticket"), "1 ticket");
        assert_eq!(plural(4., "ticket"), "4 tickets");
        assert_eq!(plural(0., "ticket"), "0 tickets");
        assert_eq!(plural(1200., "delivered ticket"), "1,200 delivered tickets");
    }

    #[test]
    fn the_score_card_counts_from_the_board_and_says_all_time_once() {
        assert_eq!(
            delivered_line(4, 1, &period_phrase(false, "Oct 2026")),
            "4 delivered tickets in Oct 2026. 1 scored, 3 not scored yet."
        );
        assert_eq!(
            delivered_line(4, 1, &period_phrase(true, "All time")),
            "4 delivered tickets across all time. 1 scored, 3 not scored yet."
        );
        assert_eq!(
            delivered_line(1200, 1, "in Oct 2026"),
            "1,200 delivered tickets in Oct 2026. 1 scored, 1,199 not scored yet."
        );
        assert!(delivered_line(1, 0, "in Oct 2026").starts_with("1 delivered ticket in"));
    }

    fn ai() -> AiConfigStatus {
        AiConfigStatus {
            runner: "claude-code".into(),
            model: "sonnet".into(),
            has_api_key: false,
        }
    }

    #[test]
    fn the_plan_states_what_the_next_run_does() {
        let estimate = TicketEstimate {
            tickets: 1,
            input_tokens: 12345,
        };
        assert_eq!(
            plan_line(1, Some(&ai()), Some(&estimate)),
            "Scoring 1 more ticket with sonnet (Claude Code). About 12,345 input tokens (estimate)."
        );
        assert_eq!(
            plan_line(1100, Some(&ai()), None),
            "Scoring 1,100 more tickets with sonnet (Claude Code)."
        );
        assert_eq!(
            plan_line(3, None, None),
            "Scoring 3 more tickets with the configured model."
        );
        assert_eq!(
            plan_line(0, Some(&ai()), Some(&estimate)),
            "Coverage already reached. Nothing to score."
        );
        let api = AiConfigStatus {
            runner: "api".into(),
            ..ai()
        };
        assert!(plan_line(2, Some(&api), None).contains("sonnet (Anthropic API)"));
    }

    fn job(over: impl FnOnce(&mut TicketJobEvent)) -> TicketJobEvent {
        let mut job = TicketJobEvent {
            kind: "assess".into(),
            state: "running".into(),
            done: 1,
            total: 4,
            failed: 0,
            error: None,
            job_id: 1,
            pending_fetches: 0,
        };
        over(&mut job);
        job
    }

    #[test]
    fn a_job_shows_what_it_does_and_how_far_it_got() {
        assert_eq!(job_label(&job(|_| {})), "Scoring tickets...");
        assert_eq!(
            job_label(&job(|j| j.kind = "fetch".into())),
            "Fetching tickets from ClickUp..."
        );
        assert_eq!(job_counts(&job(|_| {})), "1/4");
        assert_eq!(job_counts(&job(|j| j.failed = 1)), "1/4, 1 failed");
        assert_eq!(job_fraction(&job(|_| {})), 0.25);
        assert_eq!(job_fraction(&job(|j| j.total = 0)), 0.);
        assert_eq!(job_fraction(&job(|j| j.done = 9)), 1.);
    }

    #[test]
    fn tickets_that_could_not_be_scored_stay_at_zero() {
        assert_eq!(
            failed_note(1),
            "1 ticket could not be scored and stays at 0 points."
        );
        assert_eq!(
            failed_note(2),
            "2 tickets could not be scored and stay at 0 points."
        );
    }

    #[test]
    fn only_a_queued_or_running_job_is_live() {
        assert!(!live(None));
        for (state, expected) in [
            ("queued", true),
            ("running", true),
            ("done", false),
            ("error", false),
            ("cancelled", false),
        ] {
            assert_eq!(
                live(Some(&job(|j| j.state = state.into()))),
                expected,
                "{state}"
            );
        }
    }
}
