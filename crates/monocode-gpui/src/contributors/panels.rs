//! The sync panels of the GitHub tab (`SyncPanel.tsx`, `GhNotice.tsx`): the
//! page of a period that is not synced, the bar above a synced one, and the
//! note about GitHub data.
use super::{data::Snapshot, github::GithubView, ui::amber};
use crate::{right_panel::format_integer, theme::*};
use gpui::{AnyElement, Context, Role, SharedString, div, prelude::*, px, relative};
use monocode_core::contributors::{
    jobs::{JobState, SyncEvent},
    model::{
        format::relative_time,
        pipeline::ContributorsModel,
        sync_jobs::{iso_seconds, step_label},
    },
    sync::Estimate,
};

fn plural(n: u64, one: &str, many: &str) -> String {
    format!(
        "{} {}",
        format_integer(n as i64),
        if n == 1 { one } else { many }
    )
}

/// "About 120 commits and 9 pull requests to read."
pub(crate) fn estimate_text(estimate: &Estimate) -> String {
    // Commits the store already has need no reading again.
    let commits = estimate.commits.saturating_sub(estimate.stored_commits);
    let mut parts = vec![plural(commits, "commit", "commits")];
    if let Some(prs) = estimate.prs {
        parts.push(plural(prs, "pull request", "pull requests"));
    }
    format!("About {} to read.", parts.join(" and "))
}

/// "Syncing Commits: 3,200 of 12,000"
pub(crate) fn progress_text(job: &SyncEvent) -> String {
    if job.state == JobState::Queued {
        return "Sync queued".into();
    }
    let step = step_label(job.step);
    if job.total > 0 {
        format!(
            "Syncing {step}: {} of {}",
            format_integer(job.done as i64),
            format_integer(job.total as i64)
        )
    } else {
        format!("Syncing {step}")
    }
}

/// The GhNotice sentence: a missing Review score, incomplete GitHub data, or
/// the note as information when GitHub answered.
pub(crate) fn gh_notice_text(available: bool, incomplete: bool, note: Option<&str>) -> String {
    let trimmed = note.map(str::trim).unwrap_or("");
    let reason = trimmed.trim_end_matches(|c: char| c == '.' || c.is_whitespace());
    if incomplete {
        let mut text = String::from("GitHub data is incomplete for this period.");
        if !reason.is_empty() {
            text.push_str(&format!(" {reason}."));
        }
        if !available {
            text.push_str(" Review points are not counted.");
        }
        text
    } else if available {
        trimmed.to_string()
    } else {
        let because = if reason.is_empty() {
            String::new()
        } else {
            format!(": {reason}")
        };
        format!(
            "Review points are not counted because GitHub data is unavailable{because}. PRs are inferred from git history."
        )
    }
}

/// "Checking origin..." with the Cancel button, while only a fetch runs.
fn checking_origin() -> AnyElement {
    div()
        .id("contributors-checking-origin")
        .debug_selector(|| "contributors-checking-origin".into())
        .role(Role::Status)
        .flex()
        .items_center()
        .gap(px(6.))
        .child(crate::right_panel::spinner(
            "contributors-origin-spinner",
            "loader",
            12.,
        ))
        .child("Checking origin...")
        .into_any_element()
}

fn progress_bar(percent: u8) -> AnyElement {
    div()
        .id("contributors-progress")
        .debug_selector(|| "contributors-progress".into())
        .role(Role::ProgressIndicator)
        .aria_label("Sync progress")
        .h(px(4.))
        .w_full()
        .max_w(px(320.))
        .rounded_full()
        .overflow_hidden()
        .bg(content(0.1))
        .child(
            div()
                .h_full()
                .rounded_full()
                .bg(accent_color())
                .w(relative(f32::from(percent.min(100)) / 100.)),
        )
        .into_any_element()
}

impl GithubView {
    fn panel_button(
        &self,
        id: &'static str,
        label: &'static str,
        refresh_icon: bool,
        disabled: bool,
    ) -> gpui::Stateful<gpui::Div> {
        crate::settings::widgets::secondary_button(id, label, false, disabled)
            .debug_selector(move || id.to_string())
            .when(refresh_icon, |d| {
                d.child(icon("refresh-cw", 12.).text_color(content(0.7)))
            })
            .child(label)
    }

    /// The page of a period the store does not have in full: nothing is shown until Sync.
    pub(super) fn not_synced_panel(
        &mut self,
        snap: &Snapshot,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let label = Self::period_label(snap);
        let body = match &snap.job {
            Some(job) => div()
                .id("contributors-job")
                .role(Role::Status)
                .w_full()
                .pt(px(4.))
                .flex()
                .flex_col()
                .items_center()
                .gap_2()
                .child(
                    div()
                        .debug_selector(|| "contributors-progress-text".into())
                        .text_size(px(12.))
                        .text_color(content(0.6))
                        .child(progress_text(job)),
                )
                .child(progress_bar(if job.state == JobState::Queued {
                    0
                } else {
                    job.percent
                }))
                .child(
                    self.panel_button("contributors-cancel", "Cancel", false, false)
                        .on_click(cx.listener(|this, _, _, cx| this.cancel_sync(cx))),
                )
                .into_any_element(),
            None if snap.fetching => div()
                .flex()
                .flex_col()
                .items_center()
                .gap_2()
                .text_size(px(12.))
                .text_color(content(0.6))
                .child(checking_origin())
                .child(
                    self.panel_button("contributors-cancel", "Cancel", false, false)
                        .on_click(cx.listener(|this, _, _, cx| this.cancel_sync(cx))),
                )
                .into_any_element(),
            None => div()
                .flex()
                .flex_col()
                .items_center()
                .gap_2()
                .when_some(snap.estimate.as_ref(), |d, estimate| {
                    d.child(
                        div()
                            .debug_selector(|| "contributors-estimate".into())
                            .text_size(px(12.))
                            .text_color(content(0.5))
                            .child(estimate_text(estimate)),
                    )
                })
                .when(snap.estimate.is_none(), |d| {
                    d.when_some(snap.estimate_error.as_ref(), |d, error| {
                        d.child(
                            div()
                                .debug_selector(|| "contributors-estimate-error".into())
                                .text_size(px(12.))
                                .text_color(content(0.5))
                                .child(format!("Could not estimate the size: {error}")),
                        )
                    })
                })
                .child(
                    div().pt(px(4.)).child(
                        self.panel_button("contributors-sync", "Sync", true, false)
                            .on_click(cx.listener(|this, _, _, cx| this.start_sync(cx))),
                    ),
                )
                .into_any_element(),
        };
        div()
            .debug_selector(|| "contributors-not-synced".into())
            .flex()
            .flex_col()
            .items_center()
            .gap_2()
            .rounded_xl()
            .border_1()
            .border_color(content(0.1))
            .px_4()
            .py_8()
            .text_center()
            .child(icon("users", 20.).text_color(content(0.35)))
            .child(
                div()
                    .text_size(px(13.))
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .child(format!("No data for {label} yet")),
            )
            .child(body)
            .into_any_element()
    }

    fn period_label(snap: &Snapshot) -> String {
        use monocode_core::contributors::model::period::PeriodKind;
        match snap.period.kind {
            PeriodKind::Days7 => "the last 7 days".into(),
            PeriodKind::All => "all time".into(),
            _ => snap.period.label.clone(),
        }
    }

    /// One line above a synced period: when it was synced, and a sync or top-up running.
    pub(super) fn synced_bar(&mut self, snap: &Snapshot, cx: &mut Context<Self>) -> AnyElement {
        let when = snap
            .synced_at
            .and_then(|at| chrono::DateTime::from_timestamp(at, 0))
            .map(|at| {
                relative_time(
                    &iso_seconds(&at.with_timezone(&chrono::Local)),
                    &(self.clock)(),
                )
            })
            .unwrap_or_default();
        div()
            .debug_selector(|| "contributors-synced-bar".into())
            .mb_4()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(10.))
            .text_size(px(12.))
            .text_color(content(0.5))
            .when(!when.is_empty(), |d| {
                d.child(
                    div()
                        .debug_selector(|| "contributors-synced-at".into())
                        .child(format!("Synced {when}")),
                )
            })
            .map(|d| match &snap.job {
                Some(job) => d
                    .child(
                        div()
                            .id("contributors-bar-job")
                            .role(Role::Status)
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .child(crate::right_panel::spinner(
                                "contributors-bar-spinner",
                                "loader",
                                12.,
                            ))
                            .child(
                                div()
                                    .debug_selector(|| "contributors-progress-text".into())
                                    .child(progress_text(job)),
                            ),
                    )
                    .child(
                        self.panel_button("contributors-cancel", "Cancel", false, false)
                            .on_click(cx.listener(|this, _, _, cx| this.cancel_sync(cx))),
                    ),
                None if snap.fetching => d.child(checking_origin()).child(
                    self.panel_button("contributors-cancel", "Cancel", false, false)
                        .on_click(cx.listener(|this, _, _, cx| this.cancel_sync(cx))),
                ),
                None => d.child(
                    self.panel_button("contributors-sync-again", "Sync again", true, false)
                        .on_click(cx.listener(|this, _, _, cx| this.start_sync(cx))),
                ),
            })
            .into_any_element()
    }

    /// Explains a missing Review score when GitHub data is unavailable (a
    /// warning), says the synced GitHub data is incomplete, or shows the note
    /// as information when GitHub answered.
    pub(super) fn gh_notice(
        &mut self,
        snap: &Snapshot,
        model: &ContributorsModel,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let available = model.raw.gh_available;
        let incomplete = snap.gh_incomplete;
        let warning = incomplete || !available;
        let retrying = snap.job.is_some();
        let text = gh_notice_text(available, incomplete, model.raw.gh_note.as_deref());
        let label = if incomplete { "Retry GitHub" } else { "Retry" };
        div()
            .id("contributors-gh-notice")
            .debug_selector(|| "contributors-gh-notice".into())
            .role(Role::Status)
            .aria_label(SharedString::from(text.clone()))
            .mb_4()
            .flex()
            .items_start()
            .gap(px(10.))
            .rounded_xl()
            .border_1()
            .border_color(if warning { amber(0.3) } else { content(0.1) })
            .bg(if warning { amber(0.05) } else { content(0.03) })
            .px(px(14.))
            .py(px(10.))
            .text_size(px(12.))
            .text_color(content(0.7))
            .child(icon("alert-circle", 14.).mt(px(2.)).text_color(if warning {
                amber(1.)
            } else {
                content(0.45)
            }))
            .child(div().min_w_0().flex_1().line_height(px(19.5)).child(text))
            .child(
                self.panel_button("contributors-gh-retry", label, !retrying, retrying)
                    .when(!retrying, |d| {
                        d.on_click(cx.listener(|this, _, _, cx| this.retry_github(cx)))
                    })
                    .when(retrying, |d| {
                        d.child(crate::right_panel::spinner(
                            "contributors-gh-spinner",
                            "loader",
                            12.,
                        ))
                    }),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use monocode_core::contributors::sync::SyncStep;

    fn job(state: JobState, step: SyncStep, done: u64, total: u64) -> SyncEvent {
        SyncEvent {
            repo_key: String::new(),
            project: String::new(),
            root: "/r".into(),
            branch: "main".into(),
            from: 1,
            to: 2,
            step,
            done,
            total,
            percent: 10,
            state,
            error: None,
        }
    }

    fn estimate(commits: u64, stored: u64, prs: Option<u64>) -> Estimate {
        Estimate {
            commits,
            stored_commits: stored,
            first_parent: 30,
            prs,
        }
    }

    // SyncPanel.tsx estimateText.
    #[test]
    fn the_estimate_counts_commits_and_pull_requests_to_read() {
        assert_eq!(
            estimate_text(&estimate(40, 0, Some(9))),
            "About 40 commits and 9 pull requests to read."
        );
        assert_eq!(
            estimate_text(&estimate(1, 0, Some(1))),
            "About 1 commit and 1 pull request to read."
        );
    }

    #[test]
    fn commits_the_store_already_has_are_not_counted() {
        assert_eq!(
            estimate_text(&estimate(40, 25, Some(9))),
            "About 15 commits and 9 pull requests to read."
        );
        assert_eq!(
            estimate_text(&estimate(40, 60, None)),
            "About 0 commits to read."
        );
    }

    #[test]
    fn no_pull_request_count_means_commits_only() {
        assert_eq!(
            estimate_text(&estimate(1200, 0, None)),
            "About 1,200 commits to read."
        );
    }

    #[test]
    fn progress_reads_step_and_counts() {
        assert_eq!(
            progress_text(&job(JobState::Running, SyncStep::Commits, 3200, 12000)),
            "Syncing Commits: 3,200 of 12,000"
        );
        assert_eq!(
            progress_text(&job(JobState::Running, SyncStep::PullRequests, 0, 0)),
            "Syncing Pull requests"
        );
        assert_eq!(
            progress_text(&job(JobState::Running, SyncStep::Github, 2, 9)),
            "Syncing GitHub: 2 of 9"
        );
    }

    #[test]
    fn a_queued_job_says_so() {
        assert_eq!(
            progress_text(&job(JobState::Queued, SyncStep::Counting, 0, 0)),
            "Sync queued"
        );
    }

    // GhNotice.test.ts
    #[test]
    fn unavailable_github_explains_the_missing_review_score() {
        assert_eq!(
            gh_notice_text(false, false, Some("gh is not installed.")),
            "Review points are not counted because GitHub data is unavailable: gh is not installed. PRs are inferred from git history."
        );
        assert_eq!(
            gh_notice_text(false, false, None),
            "Review points are not counted because GitHub data is unavailable. PRs are inferred from git history."
        );
        assert_eq!(
            gh_notice_text(false, false, Some("  ")),
            "Review points are not counted because GitHub data is unavailable. PRs are inferred from git history."
        );
    }

    #[test]
    fn incomplete_github_data_says_so_with_the_reason() {
        assert_eq!(
            gh_notice_text(true, true, Some("Rate limited.")),
            "GitHub data is incomplete for this period. Rate limited."
        );
        assert_eq!(
            gh_notice_text(true, true, None),
            "GitHub data is incomplete for this period."
        );
    }

    #[test]
    fn incomplete_and_unavailable_adds_the_review_note() {
        assert_eq!(
            gh_notice_text(false, true, Some("gh is not installed")),
            "GitHub data is incomplete for this period. gh is not installed. Review points are not counted."
        );
    }

    #[test]
    fn a_note_with_available_github_is_shown_as_it_is() {
        assert_eq!(
            gh_notice_text(true, false, Some("  Only the last 200 PRs were read.  ")),
            "Only the last 200 PRs were read."
        );
    }
}
