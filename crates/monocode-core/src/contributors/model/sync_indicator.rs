//! What the footer sync chip shows for the jobs of every project. Port of
//! `syncIndicator.ts`.

use super::js_round;
use super::sync_jobs::{is_live, step_label};
use crate::contributors::jobs::{JobState, SyncEvent};

#[derive(Clone, Debug, PartialEq)]
pub struct SyncIndicatorSummary {
    /// Combined progress of the running jobs (each weighs the same), or `None` with none.
    pub percent: Option<u8>,
    /// The first job that stopped with an error and was not dismissed.
    pub failed: Option<SyncEvent>,
    /// The job a click opens: the failure when there is one, else the first running job.
    pub target: SyncEvent,
    /// One line per job, for the tooltip.
    pub title: String,
}

fn folder_of(path: &str) -> &str {
    path.rsplit(['/', '\\'])
        .find(|part| !part.is_empty())
        .unwrap_or(path)
}

fn name_of(event: &SyncEvent) -> &str {
    if event.project.is_empty() {
        folder_of(&event.root)
    } else {
        &event.project
    }
}

/// `n.toLocaleString("en-US")` for a whole number.
fn group_thousands(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

fn progress_line(event: &SyncEvent) -> String {
    let prefix = format!("Syncing contributors · {} · ", name_of(event));
    if event.state == JobState::Queued {
        return format!("{prefix}Queued");
    }
    let label = step_label(event.step);
    if event.total == 0 {
        return format!("{prefix}{label}");
    }
    format!(
        "{prefix}{label} {}/{}",
        group_thousands(event.done),
        group_thousands(event.total)
    )
}

fn failure_line(event: &SyncEvent) -> String {
    format!(
        "Contributors sync stopped · {} · {}",
        name_of(event),
        event.error.as_deref().unwrap_or("The sync failed.")
    )
}

/// `None` when nothing runs and nothing failed: the indicator is not shown.
/// `jobs` are the jobs in the order they appeared (`SyncJobStore::jobs`).
pub fn summarize_sync(jobs: &[SyncEvent]) -> Option<SyncIndicatorSummary> {
    let live: Vec<&SyncEvent> = jobs.iter().filter(|e| is_live(e)).collect();
    let failed = jobs.iter().find(|e| e.state == JobState::Error);
    let target = failed.or(live.first().copied())?;
    let percent = (!live.is_empty()).then(|| {
        let sum: f64 = live.iter().map(|e| f64::from(e.percent)).sum();
        js_round(sum / live.len() as f64).clamp(0.0, 100.0) as u8
    });
    let lines: Vec<String> = failed
        .map(failure_line)
        .into_iter()
        .chain(live.iter().map(|e| progress_line(e)))
        .collect();
    Some(SyncIndicatorSummary {
        percent,
        failed: failed.cloned(),
        target: target.clone(),
        title: lines.join("\n"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contributors::jobs::JobState;
    use crate::contributors::sync::SyncStep;

    fn event(patch: impl FnOnce(&mut SyncEvent)) -> SyncEvent {
        let mut e = SyncEvent {
            repo_key: "/r/.git".into(),
            project: "alpha".into(),
            root: "/work/alpha".into(),
            branch: "main".into(),
            from: 1,
            to: 2,
            step: SyncStep::Commits,
            done: 3200,
            total: 12000,
            percent: 40,
            state: JobState::Running,
            error: None,
        };
        patch(&mut e);
        e
    }

    fn lines(summary: &SyncIndicatorSummary) -> Vec<&str> {
        summary.title.split('\n').collect()
    }

    #[test]
    fn summarize_sync_is_null_while_nothing_runs_and_nothing_failed() {
        assert_eq!(summarize_sync(&[]), None);
    }

    #[test]
    fn summarize_sync_shows_one_running_job_as_its_percent_and_one_tooltip_line() {
        let summary = summarize_sync(&[event(|_| {})]).unwrap();
        assert_eq!(summary.percent, Some(40));
        assert_eq!(summary.failed, None);
        assert_eq!(
            summary.title,
            "Syncing contributors · alpha · Commits 3,200/12,000"
        );
        assert_eq!(summary.target.root, "/work/alpha");
    }

    #[test]
    fn summarize_sync_averages_several_jobs_equally_and_lists_each_one() {
        let summary = summarize_sync(&[
            event(|e| e.percent = 10),
            event(|e| {
                e.root = "/work/beta".into();
                e.project = "beta".into();
                e.from = 5;
                e.step = SyncStep::PullRequests;
                e.done = 1;
                e.total = 4;
                e.percent = 31;
            }),
        ])
        .unwrap();
        assert_eq!(summary.percent, Some(21));
        assert_eq!(
            lines(&summary),
            [
                "Syncing contributors · alpha · Commits 3,200/12,000",
                "Syncing contributors · beta · Pull requests 1/4",
            ]
        );
        assert_eq!(summary.target.root, "/work/alpha");
    }

    #[test]
    fn summarize_sync_counts_a_queued_job_as_0_percent_and_says_it_is_queued() {
        let summary = summarize_sync(&[
            event(|e| e.percent = 50),
            event(|e| {
                e.from = 9;
                e.state = JobState::Queued;
                e.step = SyncStep::Counting;
                e.done = 0;
                e.total = 0;
                e.percent = 0;
            }),
        ])
        .unwrap();
        assert_eq!(summary.percent, Some(25));
        assert_eq!(lines(&summary)[1], "Syncing contributors · alpha · Queued");
    }

    #[test]
    fn summarize_sync_leaves_out_the_counts_while_a_step_has_no_total_yet() {
        let summary = summarize_sync(&[event(|e| {
            e.step = SyncStep::Counting;
            e.done = 0;
            e.total = 0;
            e.percent = 1;
        })])
        .unwrap();
        assert_eq!(summary.title, "Syncing contributors · alpha · Counting");
    }

    #[test]
    fn summarize_sync_falls_back_to_the_folder_of_the_path_when_the_project_name_is_unknown() {
        let summary = summarize_sync(&[event(|e| {
            e.project = String::new();
            e.root = "/work/gamma".into();
        })])
        .unwrap();
        assert!(summary.title.contains("· gamma ·"));
        let windows = summarize_sync(&[event(|e| {
            e.project = String::new();
            e.root = "C:\\work\\delta\\".into();
        })])
        .unwrap();
        assert!(windows.title.contains("· delta ·"));
    }

    #[test]
    fn summarize_sync_keeps_a_failure_on_top_with_its_error_until_it_is_dismissed() {
        let failed = event(|e| {
            e.from = 7;
            e.state = JobState::Error;
            e.error = Some("git exploded".into());
            e.percent = 12;
            e.root = "/work/beta".into();
            e.project = "beta".into();
        });
        let summary = summarize_sync(&[event(|_| {}), failed.clone()]).unwrap();
        assert_eq!(summary.failed.as_ref(), Some(&failed));
        assert_eq!(summary.target, failed);
        assert_eq!(summary.percent, Some(40));
        assert_eq!(
            lines(&summary),
            [
                "Contributors sync stopped · beta · git exploded",
                "Syncing contributors · alpha · Commits 3,200/12,000",
            ]
        );
    }

    #[test]
    fn summarize_sync_shows_a_lone_failure_without_a_percent() {
        let summary = summarize_sync(&[event(|e| {
            e.state = JobState::Error;
            e.error = None;
        })])
        .unwrap();
        assert_eq!(summary.percent, None);
        assert_eq!(
            summary.title,
            "Contributors sync stopped · alpha · The sync failed."
        );
    }

    #[test]
    fn summarize_sync_groups_thousands_like_en_us_and_rounds_halves_up() {
        let summary = summarize_sync(&[
            event(|e| {
                e.done = 1_234_567;
                e.total = 2_000_000;
                e.percent = 1;
            }),
            event(|e| {
                e.from = 3;
                e.percent = 2;
            }),
        ])
        .unwrap();
        assert!(summary.title.contains("Commits 1,234,567/2,000,000"));
        // (1 + 2) / 2 = 1.5 -> Math.round gives 2
        assert_eq!(summary.percent, Some(2));
    }
}
